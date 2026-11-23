//! Point in time versions of a backup, made with APFS clones.
//!
//! Real APFS snapshots need an entitlement Apple only grants on request, so a snapshot here is a
//! folder `_Snapshots/<run>` holding a clone of the backup. Cloning shares all data blocks, so a
//! snapshot costs only metadata until the backup changes. Because the engine replaces files with a
//! rename instead of writing into them, the old blocks stay with the snapshot. Snapshots are plain
//! folders: browse them in Finder and copy back whatever you need.

use serde::Serialize;
use std::ffi::CString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const SNAPSHOT_FOLDER: &str = "_Snapshots";

/// Top level names that are never part of a snapshot.
const SKIPPED: [&str; 8] = [SNAPSHOT_FOLDER, "_Archive", ".Spotlight-V100", ".fseventsd", ".Trashes", ".TemporaryItems", ".DocumentRevisions-V100", ".DS_Store"];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub name: String,
    pub path: String,
    /// Unix seconds, from the folder name.
    pub created_at: u64,
}

/// True when the folder is on APFS, the only file system with clones.
pub fn supported(path: &Path) -> bool {
    filesystem(path).is_some_and(|name| name == "apfs")
}

#[cfg(target_os = "macos")]
pub fn filesystem(path: &Path) -> Option<String> {
    let path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid C string and `stats` is a properly sized, writable statfs.
    if unsafe { libc::statfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    // SAFETY: the kernel writes a NUL terminated name into f_fstypename.
    let name = unsafe { std::ffi::CStr::from_ptr(stats.f_fstypename.as_ptr()) };
    Some(name.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "macos"))]
pub fn filesystem(_path: &Path) -> Option<String> {
    None
}

/// Clones the current backup in `root` into `_Snapshots/<name>`.
pub fn create(root: &Path, name: &str) -> io::Result<PathBuf> {
    let folder = root.join(SNAPSHOT_FOLDER);
    fs::create_dir_all(&folder)?;
    // Keep Spotlight from indexing every version of every photo.
    let _ = fs::write(folder.join(".metadata_never_index"), b"");

    let snapshot = folder.join(name);
    if snapshot.exists() {
        // Two runs in the same second: the existing snapshot already holds this state.
        return Ok(snapshot);
    }
    let staging = folder.join(format!(".{name}.incomplete"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let skipped = SKIPPED.iter().any(|skipped| name == *skipped) || name.to_string_lossy().ends_with(crate::transfer::PARTIAL_SUFFIX);
        if !skipped {
            clone(&entry.path(), &staging.join(&name))?;
        }
    }
    // Only a finished clone gets its final name, so a half made snapshot is never mistaken for a good one.
    fs::rename(&staging, &snapshot)?;
    Ok(snapshot)
}

pub fn list(root: &Path) -> Vec<Snapshot> {
    let Ok(entries) = fs::read_dir(root.join(SNAPSHOT_FOLDER)) else { return Vec::new() };
    let mut snapshots: Vec<Snapshot> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let created_at = crate::backup::parse_archive_run_name(&name)?;
            Some(Snapshot { path: entry.path().to_string_lossy().into_owned(), name, created_at })
        })
        .collect();
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created_at));
    snapshots
}

/// Deletes snapshots older than `days`, always keeping the newest one, and leftovers of interrupted snapshots.
pub fn prune(root: &Path, days: u32, now: u64) -> io::Result<usize> {
    let cutoff = now.saturating_sub(u64::from(days) * 86_400);
    let mut removed = 0;
    for snapshot in list(root).into_iter().skip(1).filter(|snapshot| snapshot.created_at < cutoff) {
        fs::remove_dir_all(&snapshot.path)?;
        removed += 1;
    }
    if let Ok(entries) = fs::read_dir(root.join(SNAPSHOT_FOLDER)) {
        for entry in entries.flatten().filter(|entry| entry.file_name().to_string_lossy().ends_with(".incomplete")) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(removed)
}

/// From `<sys/clonefile.h>`: do not follow a symlink given as the source.
#[cfg(target_os = "macos")]
const CLONE_NOFOLLOW: u32 = 0x0001;

#[cfg(target_os = "macos")]
fn clone(source: &Path, destination: &Path) -> io::Result<()> {
    let source = CString::new(source.as_os_str().as_encoded_bytes()).map_err(io::Error::other)?;
    let destination = CString::new(destination.as_os_str().as_encoded_bytes()).map_err(io::Error::other)?;
    // SAFETY: both arguments are valid C strings. clonefile copies folders recursively, copy on write.
    if unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), CLONE_NOFOLLOW) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn clone(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Snapshots need an APFS drive on macOS"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn snapshots_keep_old_versions_after_the_backup_changes() {
        let root = std::env::temp_dir().join(format!("lbs-snapshots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("photos/2024")).unwrap();
        fs::create_dir_all(root.join("_Archive/old")).unwrap();
        fs::write(root.join("photos/2024/IMG_1.xmp"), b"rating=3").unwrap();
        assert!(supported(&root), "the temp folder is expected on APFS");

        create(&root, "2026-01-01_120000").unwrap();
        // The engine replaces files by renaming a new copy over them.
        fs::write(root.join("photos/2024/IMG_1.xmp.lbpart"), b"rating=5").unwrap();
        fs::rename(root.join("photos/2024/IMG_1.xmp.lbpart"), root.join("photos/2024/IMG_1.xmp")).unwrap();
        create(&root, "2026-01-02_120000").unwrap();

        let snapshots = list(&root);
        assert_eq!(snapshots.iter().map(|snapshot| snapshot.name.as_str()).collect::<Vec<_>>(), vec!["2026-01-02_120000", "2026-01-01_120000"]);
        assert_eq!(fs::read(root.join("_Snapshots/2026-01-01_120000/photos/2024/IMG_1.xmp")).unwrap(), b"rating=3");
        assert_eq!(fs::read(root.join("_Snapshots/2026-01-02_120000/photos/2024/IMG_1.xmp")).unwrap(), b"rating=5");
        assert!(!root.join("_Snapshots/2026-01-02_120000/_Archive").exists());
        assert!(!root.join("_Snapshots/2026-01-02_120000/_Snapshots").exists());

        // Pruning keeps the newest even when everything is old.
        assert_eq!(prune(&root, 1, crate::backup::parse_archive_run_name("2026-03-01_000000").unwrap()).unwrap(), 1);
        assert_eq!(list(&root).len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
