use crate::backup::{PlanAction, PlanEntry, RunControl};
use std::fs::{self, File, FileTimes};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const CHUNK_SIZE: usize = 8 * 1024 * 1024;
/// Files up to this size use the OS copy (clonefile on APFS, copy_file_range on Linux) when nothing needs hashing.
const NATIVE_COPY_LIMIT: u64 = 256 * 1024 * 1024;
const RESUME_CHECK_BYTES: u64 = 1024 * 1024;
pub const PARTIAL_SUFFIX: &str = ".lbpart";

pub enum CopyError {
    Cancelled,
    Failed(String),
}

impl From<io::Error> for CopyError {
    fn from(error: io::Error) -> Self {
        CopyError::Failed(error.to_string())
    }
}

pub struct CopiedFile {
    pub archived: bool,
    pub size: u64,
    pub modified_ns: i64,
    pub hash: Option<String>,
}

pub struct Transfer<'a> {
    pub control: &'a RunControl,
    pub copied_bytes: &'a AtomicU64,
    /// Where replaced files go. `None` replaces in place.
    pub archive_root: Option<&'a Path>,
    /// The destination folder of the job, so archived files keep their place inside it.
    pub destination_root: &'a Path,
    pub preserve_metadata: bool,
}

impl Transfer<'_> {
    /// Copies into `<destination>.lbpart`, flushes it to disk, then swaps it in.
    /// A partial file left by a cancelled or crashed run is resumed when it still matches the source.
    pub fn copy(&self, entry: &PlanEntry) -> Result<CopiedFile, CopyError> {
        let source = Path::new(&entry.source_path);
        let destination = PathBuf::from(&entry.destination_path);
        let partial = PathBuf::from(format!("{}{PARTIAL_SUFFIX}", entry.destination_path));
        let source_metadata = fs::metadata(source)?;
        let size = source_metadata.len();
        let source_modified = source_metadata.modified()?;

        let parent = destination.parent().ok_or_else(|| CopyError::Failed("Destination has no parent folder".to_string()))?;
        fs::create_dir_all(parent)?;

        let offset = resumable_offset(source, &partial, size, source_modified)?;
        let hash = if offset == 0 && !entry.verify_hash && size <= NATIVE_COPY_LIMIT {
            if self.control.wait() { return Err(CopyError::Cancelled); }
            let _ = fs::remove_file(&partial);
            fs::copy(source, &partial)?;
            self.copied_bytes.fetch_add(size, Ordering::Relaxed);
            None
        } else {
            self.copy_chunked(source, &partial, offset, entry.verify_hash)?
        };

        self.finish(entry, &partial, size, source_modified, hash, Some(source))
    }

    /// Copies a file that is not on a local disk (a phone). `fetch` writes the whole file into the
    /// partial path it is given and returns the content hash when one was requested.
    pub fn copy_remote(&self, entry: &PlanEntry, fetch: impl FnOnce(&Path) -> Result<Option<String>, CopyError>) -> Result<CopiedFile, CopyError> {
        let destination = Path::new(&entry.destination_path);
        let partial = PathBuf::from(format!("{}{PARTIAL_SUFFIX}", entry.destination_path));
        let parent = destination.parent().ok_or_else(|| CopyError::Failed("Destination has no parent folder".to_string()))?;
        fs::create_dir_all(parent)?;
        if self.control.wait() { return Err(CopyError::Cancelled); }
        let _ = fs::remove_file(&partial);
        let hash = fetch(&partial)?;
        self.finish(entry, &partial, entry.size, crate::scan::ns_to_system_time(entry.modified_ns), hash, None)
    }

    /// Stamps, flushes and verifies the partial copy, archives the current backup file, then swaps the copy in.
    fn finish(&self, entry: &PlanEntry, partial: &Path, size: u64, modified: std::time::SystemTime, hash: Option<String>, source: Option<&Path>) -> Result<CopiedFile, CopyError> {
        let destination = Path::new(&entry.destination_path);
        // Read only is enough for setting times and syncing, and works when the source was locked (read only) and fs::copy kept that.
        let written = File::open(partial)?;
        written.set_times(FileTimes::new().set_modified(modified).set_accessed(modified))?;
        written.sync_all()?;
        if written.metadata()?.len() != size {
            let _ = fs::remove_file(partial);
            return Err(CopyError::Failed("Source changed while copying. It will be retried next run.".to_string()));
        }
        drop(written);

        if let Some(expected) = &hash {
            let actual = crate::scan::hash_file(partial)?;
            if &actual != expected {
                let _ = fs::remove_file(partial);
                return Err(CopyError::Failed("Verification failed: the written copy does not match the source.".to_string()));
            }
        }
        if let (true, Some(source)) = (self.preserve_metadata, source) {
            copy_extended_attributes(source, partial);
        }

        let archived = match self.archive_root {
            Some(archive_root) if matches!(entry.action, PlanAction::Update) && destination.exists() => {
                let inside = destination.strip_prefix(self.destination_root).unwrap_or_else(|_| Path::new(&entry.relative_path));
                archive(destination, &archive_root.join(inside))?;
                true
            }
            _ => false,
        };
        fs::rename(partial, destination)?;
        Ok(CopiedFile { archived, size, modified_ns: crate::scan::system_time_ns(modified), hash })
    }

    fn copy_chunked(&self, source: &Path, partial: &Path, offset: u64, hash: bool) -> Result<Option<String>, CopyError> {
        let mut input = File::open(source)?;
        let mut hasher = hash.then(blake3::Hasher::new);
        if let (Some(hasher), true) = (hasher.as_mut(), offset > 0) {
            // The resumed prefix is already on disk but still has to be part of the hash.
            hasher.update_reader((&mut input).take(offset))?;
        }
        input.seek(SeekFrom::Start(offset))?;
        let mut output = File::options().create(true).write(true).append(offset > 0).truncate(offset == 0).open(partial)?;

        let mut buffer = vec![0_u8; CHUNK_SIZE];
        loop {
            if self.control.wait() {
                output.flush()?;
                return Err(CopyError::Cancelled);
            }
            let read = match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            output.write_all(&buffer[..read])?;
            if let Some(hasher) = hasher.as_mut() { hasher.update(&buffer[..read]); }
            self.copied_bytes.fetch_add(read as u64, Ordering::Relaxed);
        }
        output.flush()?;
        Ok(hasher.map(|hasher| hasher.finalize().to_hex().to_string()))
    }
}

/// How much of an existing partial copy can be kept. The partial is discarded when the source
/// changed after it was written or when its last megabyte does not match the source.
fn resumable_offset(source: &Path, partial: &Path, source_size: u64, source_modified: std::time::SystemTime) -> io::Result<u64> {
    let metadata = match fs::metadata(partial) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let length = metadata.len();
    let written_after_source_change = metadata.modified().map(|modified| modified >= source_modified).unwrap_or(false);
    if length == 0 || length >= source_size || !written_after_source_change || !tails_match(source, partial, length)? {
        fs::remove_file(partial)?;
        return Ok(0);
    }
    Ok(length)
}

fn tails_match(source: &Path, partial: &Path, length: u64) -> io::Result<bool> {
    let check = length.min(RESUME_CHECK_BYTES);
    let read_tail = |path: &Path| -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(length - check))?;
        let mut buffer = vec![0_u8; check as usize];
        file.read_exact(&mut buffer)?;
        Ok(buffer)
    };
    Ok(read_tail(source)? == read_tail(partial)?)
}

/// Moves the current backup copy into the archive. A rename on the same volume is instant, even for large videos.
fn archive(current: &Path, archive_path: &Path) -> io::Result<()> {
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(current, archive_path) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(current, archive_path)?;
            let modified = fs::metadata(current)?.modified()?;
            File::options().write(true).open(archive_path)?.set_modified(modified)
        }
    }
}

/// Keeps Finder tags, color labels and other extended attributes. Failures are not fatal, many NAS shares do not support them.
fn copy_extended_attributes(source: &Path, destination: &Path) {
    if !xattr::SUPPORTED_PLATFORM {
        return;
    }
    let Ok(names) = xattr::list(source) else { return };
    for name in names {
        if name.to_string_lossy() == "com.apple.quarantine" {
            continue;
        }
        if let Ok(Some(value)) = xattr::get(source, &name) {
            let _ = xattr::set(destination, &name, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn temp_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("lbs-transfer-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn plan_entry(root: &Path, action: PlanAction, verify_hash: bool) -> PlanEntry {
        PlanEntry {
            action,
            source_path: root.join("source/clip.mov").to_string_lossy().into_owned(),
            destination_path: root.join("destination/video/clip.mov").to_string_lossy().into_owned(),
            relative_path: "clip.mov".to_string(),
            size: 0,
            reason: String::new(),
            source_id: "video".to_string(),
            modified_ns: 0,
            verify_hash,
            ..PlanEntry::default()
        }
    }

    fn write_source(root: &Path, content: &[u8], modified: SystemTime) {
        fs::create_dir_all(root.join("source")).unwrap();
        fs::write(root.join("source/clip.mov"), content).unwrap();
        File::options().write(true).open(root.join("source/clip.mov")).unwrap().set_modified(modified).unwrap();
    }

    #[test]
    fn copy_keeps_the_source_modified_time_so_the_next_run_skips_it() {
        let root = temp_dir("mtime");
        let modified = SystemTime::now() - Duration::from_secs(86_400);
        write_source(&root, b"frames", modified);
        let control = RunControl::new();
        let bytes = AtomicU64::new(0);
        let transfer = Transfer { control: &control, copied_bytes: &bytes, archive_root: None, destination_root: &root.join("destination"), preserve_metadata: true };

        for verify_hash in [false, true] {
            let copied = transfer.copy(&plan_entry(&root, PlanAction::Copy, verify_hash)).ok().expect("copy should succeed");
            let destination = fs::metadata(root.join("destination/video/clip.mov")).unwrap();
            assert_eq!(destination.modified().unwrap(), modified);
            assert_eq!(copied.hash.is_some(), verify_hash);
        }
        assert!(!root.join("destination/video/clip.mov.lbpart").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn copies_locked_read_only_sources() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("readonly");
        write_source(&root, b"locked", SystemTime::now() - Duration::from_secs(60));
        fs::set_permissions(root.join("source/clip.mov"), fs::Permissions::from_mode(0o444)).unwrap();
        let control = RunControl::new();
        let bytes = AtomicU64::new(0);
        let transfer = Transfer { control: &control, copied_bytes: &bytes, archive_root: None, destination_root: &root.join("destination"), preserve_metadata: true };
        assert!(transfer.copy(&plan_entry(&root, PlanAction::Copy, false)).is_ok());
        assert_eq!(fs::read(root.join("destination/video/clip.mov")).unwrap(), b"locked");
        fs::set_permissions(root.join("destination/video/clip.mov"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn update_moves_the_previous_copy_into_the_archive() {
        let root = temp_dir("archive");
        write_source(&root, b"new", SystemTime::now() - Duration::from_secs(10));
        fs::create_dir_all(root.join("destination/video")).unwrap();
        fs::write(root.join("destination/video/clip.mov"), b"old").unwrap();
        let control = RunControl::new();
        let bytes = AtomicU64::new(0);
        let archive_root = root.join("destination/_Archive/run");
        let transfer = Transfer { control: &control, copied_bytes: &bytes, archive_root: Some(&archive_root), destination_root: &root.join("destination"), preserve_metadata: false };

        let copied = transfer.copy(&plan_entry(&root, PlanAction::Update, false)).ok().unwrap();
        assert!(copied.archived);
        assert_eq!(fs::read(root.join("destination/video/clip.mov")).unwrap(), b"new");
        assert_eq!(fs::read(archive_root.join("video/clip.mov")).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resumes_a_matching_partial_and_discards_a_stale_one() {
        let root = temp_dir("resume");
        let content: Vec<u8> = (0..300_000_u32).map(|value| (value % 251) as u8).collect();
        write_source(&root, &content, SystemTime::now() - Duration::from_secs(60));
        let partial = root.join("destination/video/clip.mov.lbpart");
        fs::create_dir_all(partial.parent().unwrap()).unwrap();

        fs::write(&partial, &content[..100_000]).unwrap();
        assert_eq!(resumable_offset(&root.join("source/clip.mov"), &partial, content.len() as u64, SystemTime::now() - Duration::from_secs(60)).unwrap(), 100_000);

        let mut corrupted = content[..100_000].to_vec();
        corrupted[99_999] ^= 0xff;
        fs::write(&partial, &corrupted).unwrap();
        assert_eq!(resumable_offset(&root.join("source/clip.mov"), &partial, content.len() as u64, SystemTime::now() - Duration::from_secs(60)).unwrap(), 0);
        assert!(!partial.exists());

        fs::write(&partial, &content[..100_000]).unwrap();
        let control = RunControl::new();
        let bytes = AtomicU64::new(0);
        let transfer = Transfer { control: &control, copied_bytes: &bytes, archive_root: None, destination_root: &root.join("destination"), preserve_metadata: false };
        let copied = transfer.copy(&plan_entry(&root, PlanAction::Copy, true)).ok().unwrap();
        assert_eq!(fs::read(root.join("destination/video/clip.mov")).unwrap(), content);
        assert_eq!(copied.hash.unwrap(), blake3::hash(&content).to_hex().to_string());
        assert_eq!(bytes.load(Ordering::Relaxed), 200_000);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_runs_stop_without_replacing_the_destination() {
        let root = temp_dir("cancel");
        write_source(&root, b"frames", SystemTime::now());
        let control = RunControl::new();
        control.cancel();
        let bytes = AtomicU64::new(0);
        let transfer = Transfer { control: &control, copied_bytes: &bytes, archive_root: None, destination_root: &root.join("destination"), preserve_metadata: false };
        assert!(matches!(transfer.copy(&plan_entry(&root, PlanAction::Copy, true)), Err(CopyError::Cancelled)));
        assert!(!root.join("destination/video/clip.mov").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
