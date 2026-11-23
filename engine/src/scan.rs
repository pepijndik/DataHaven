use crate::backup::{BackupWarning, PlanAction, PlanEntry};
use crate::filters::{Filters, PatternSet};
use crate::manifest::{ManifestRecord, Snapshot};
use rayon::prelude::*;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// FAT and exFAT store modification times in 2 second steps, and SMB can round them.
const MTIME_TOLERANCE_NS: i64 = 2_000_000_000;

pub struct ScannedFile {
    pub path: PathBuf,
    pub relative: String,
    pub size: u64,
    pub modified_ns: i64,
}

#[derive(Default)]
pub struct Walked {
    pub files: Vec<ScannedFile>,
    pub warnings: Vec<BackupWarning>,
    pub excluded: u64,
    pub symlinks: u64,
}

impl Walked {
    fn merge(mut self, other: Walked) -> Walked {
        self.files.extend(other.files);
        self.warnings.extend(other.warnings);
        self.excluded += other.excluded;
        self.symlinks += other.symlinks;
        self
    }
}

pub struct Walker<'a> {
    pub filters: &'a Filters,
    pub skip_live_catalogs: bool,
    pub discovered: &'a AtomicU64,
}

impl Walker<'_> {
    /// Walks a source folder in parallel. Folders matching an exclude are never entered and symlinks are never followed.
    pub fn walk(&self, root: &Path) -> Walked {
        self.walk_directory(root, "")
    }

    fn walk_directory(&self, directory: &Path, relative: &str) -> Walked {
        let mut walked = Walked::default();
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) => {
                walked.warnings.push(BackupWarning::warning(format!("Could not read folder: {error}"), Some(directory)));
                return walked;
            }
        };

        let mut subdirectories = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    walked.warnings.push(BackupWarning::warning(format!("Could not read folder entry: {error}"), Some(directory)));
                    continue;
                }
            };
            let Ok(file_type) = entry.file_type() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() { name } else { format!("{relative}/{name}") };

            if file_type.is_symlink() {
                walked.symlinks += 1;
            } else if file_type.is_dir() {
                if self.filters.prunes_directory(&child_relative) {
                    walked.excluded += 1;
                } else {
                    subdirectories.push((entry.path(), child_relative));
                }
            } else if file_type.is_file() {
                let path = entry.path();
                if !self.filters.includes_file(&child_relative) {
                    walked.excluded += 1;
                    continue;
                }
                if self.skip_live_catalogs && is_live_catalog_file(&path) {
                    walked.warnings.push(BackupWarning::warning("Skipped a live Lightroom catalog file. Close Lightroom Classic to back it up.", Some(&path)));
                    continue;
                }
                match entry.metadata() {
                    Ok(metadata) => {
                        self.discovered.fetch_add(1, Ordering::Relaxed);
                        walked.files.push(ScannedFile { path, relative: child_relative, size: metadata.len(), modified_ns: modified_ns(&metadata) });
                    }
                    Err(error) => walked.warnings.push(BackupWarning::warning(format!("Could not read file details: {error}"), Some(&path))),
                }
            }
        }

        subdirectories
            .into_par_iter()
            .map(|(path, child_relative)| self.walk_directory(&path, &child_relative))
            .reduce(Walked::default, Walked::merge)
            .merge(walked)
    }
}

pub struct Differ<'a> {
    pub snapshot: &'a Snapshot,
    /// Files whose content is hashed. `None` disables hashing.
    pub hashed: Option<&'a PatternSet>,
    pub hash_everything: bool,
    pub check_destination: bool,
}

impl Differ<'_> {
    /// `destination_base` is the folder this source is copied into.
    pub fn diff(&self, file: &ScannedFile, source_id: &str, destination_base: &Path) -> PlanEntry {
        let destination = destination_base.join(&file.relative);
        let source_path = file.path.to_string_lossy().into_owned();
        let destination_path = destination.to_string_lossy().into_owned();
        let verify_hash = self.hash_everything || self.hashed.is_some_and(|patterns| patterns.is_match(&file.relative));
        let record = self.snapshot.get(&source_path, &destination_path);
        let entry = |action, reason: &str, hash: Option<String>, record_metadata| PlanEntry {
            action,
            source_path: source_path.clone(),
            destination_path: destination_path.clone(),
            relative_path: file.relative.clone(),
            size: file.size,
            reason: reason.to_string(),
            source_id: source_id.to_string(),
            modified_ns: file.modified_ns,
            verify_hash,
            known_hash: hash,
            record_metadata,
            ..PlanEntry::default()
        };

        // Fast path: the manifest saw this exact file go out, so the destination is not touched.
        if let Some(record) = record.filter(|record| unchanged_since(record, file) && !self.check_destination) {
            return match (verify_hash, &record.hash) {
                (true, Some(expected)) => match hash_file(&file.path) {
                    Ok(actual) if &actual == expected => entry(PlanAction::Skip, "unchanged (hash verified)", None, false),
                    Ok(actual) => entry(PlanAction::Update, "content changed", Some(actual), false),
                    Err(error) => entry(PlanAction::Update, &format!("could not hash source: {error}"), None, false),
                },
                _ => entry(PlanAction::Skip, "unchanged", None, false),
            };
        }

        let destination_metadata = match fs::metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return entry(PlanAction::Copy, "missing from destination", None, false),
            Err(error) => return entry(PlanAction::Copy, &format!("destination unreadable: {error}"), None, false),
        };
        if destination_metadata.len() != file.size {
            return entry(PlanAction::Update, "size changed", None, false);
        }
        if (modified_ns(&destination_metadata) - file.modified_ns).abs() > MTIME_TOLERANCE_NS {
            return entry(PlanAction::Update, "modified time changed", None, false);
        }

        // The destination matches but the manifest does not know it yet (first run, moved manifest, older app version).
        if verify_hash {
            let known = record.and_then(|record| record.hash.clone().filter(|_| unchanged_since(record, file)));
            let source_hash = match hash_file(&file.path) {
                Ok(hash) => hash,
                Err(error) => return entry(PlanAction::Update, &format!("could not hash source: {error}"), None, false),
            };
            let destination_hash = match known {
                Some(hash) => Ok(hash),
                None => hash_file(&destination),
            };
            if destination_hash.as_ref().ok() != Some(&source_hash) {
                return entry(PlanAction::Update, "content differs", Some(source_hash), false);
            }
            return entry(PlanAction::Skip, "unchanged (hash verified)", Some(source_hash), true);
        }
        entry(PlanAction::Skip, "unchanged", None, record.is_none_or(|record| !unchanged_since(record, file)))
    }
}

fn unchanged_since(record: &ManifestRecord, file: &ScannedFile) -> bool {
    record.size == file.size && record.modified_ns == Some(file.modified_ns)
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap_rayon(path)?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn modified_ns(metadata: &Metadata) -> i64 {
    metadata.modified().map(system_time_ns).unwrap_or_default()
}

pub fn system_time_ns(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i64,
        Err(error) => -(error.duration().as_nanos() as i64),
    }
}

pub fn ns_to_system_time(ns: i64) -> SystemTime {
    if ns >= 0 {
        UNIX_EPOCH + std::time::Duration::from_nanos(ns as u64)
    } else {
        UNIX_EPOCH - std::time::Duration::from_nanos(ns.unsigned_abs())
    }
}

pub fn is_live_catalog_file(path: &Path) -> bool {
    let name = path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let is_catalog = name.ends_with(".lrcat") || name.ends_with(".lrcat-wal") || name.ends_with(".lrcat-shm") || name.ends_with(".lrcat-journal");
    // Lightroom's own catalog backups live in a "Backups" folder and are safe to copy.
    let in_backups = path.components().any(|component| matches!(component, Component::Normal(name) if name == "Backups"));
    is_catalog && !in_backups
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn temp_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("lbs-scan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn walk_prunes_excluded_folders_and_skips_symlinks() {
        let root = temp_dir("walk");
        fs::create_dir_all(root.join("2024/Main Previews.lrdata/0")).unwrap();
        fs::write(root.join("2024/Main Previews.lrdata/0/a.lrprev"), b"x").unwrap();
        fs::write(root.join("2024/IMG_1.CR3"), b"raw").unwrap();
        fs::write(root.join(".DS_Store"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("2024/loop")).unwrap();

        let filters = Filters::new(&[crate::profile::BackupFilter { pattern: "**/*Previews.lrdata/**".to_string(), action: "exclude".to_string() }]).unwrap();
        let discovered = AtomicU64::new(0);
        let walked = Walker { filters: &filters, skip_live_catalogs: false, discovered: &discovered }.walk(&root);

        let names: Vec<&str> = walked.files.iter().map(|file| file.relative.as_str()).collect();
        assert_eq!(names, vec!["2024/IMG_1.CR3"]);
        assert_eq!(discovered.load(Ordering::Relaxed), 1);
        #[cfg(unix)]
        assert_eq!(walked.symlinks, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_catalog_detection_ignores_lightroom_backups() {
        assert!(is_live_catalog_file(Path::new("/lr/Main.lrcat")));
        assert!(is_live_catalog_file(Path::new("/lr/Main.lrcat-wal")));
        assert!(!is_live_catalog_file(Path::new("/lr/Backups/2024-01-01/Main.lrcat")));
        assert!(!is_live_catalog_file(Path::new("/lr/photo.dng")));
    }

    #[test]
    fn diff_tolerates_coarse_destination_timestamps() {
        let root = temp_dir("diff");
        let source = root.join("source/a.jpg");
        let destination = root.join("destination/photos/a.jpg");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&source, b"jpeg").unwrap();
        fs::write(&destination, b"jpeg").unwrap();
        let source_time = SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options().write(true).open(&source).unwrap().set_modified(source_time).unwrap();
        fs::File::options().write(true).open(&destination).unwrap().set_modified(source_time + std::time::Duration::from_millis(1500)).unwrap();

        let manifest = Manifest::open(&root.join("state")).unwrap();
        let snapshot = manifest.snapshot("job").unwrap();
        let differ = Differ { snapshot: &snapshot, hashed: None, hash_everything: false, check_destination: false };
        let metadata = fs::metadata(&source).unwrap();
        let file = ScannedFile { path: source.clone(), relative: "a.jpg".to_string(), size: metadata.len(), modified_ns: modified_ns(&metadata) };

        let entry = differ.diff(&file, "photos", &root.join("destination/photos"));
        assert!(matches!(entry.action, PlanAction::Skip), "{}", entry.reason);
        assert!(entry.record_metadata, "an unknown but matching destination is adopted into the manifest");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diff_detects_changed_content_with_identical_metadata_when_hashing() {
        let root = temp_dir("hash");
        let source = root.join("source/Main.lrcat");
        let destination = root.join("destination/lr/Main.lrcat");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&source, b"catalog-v2").unwrap();
        fs::write(&destination, b"catalog-v1").unwrap();
        let time = SystemTime::now();
        fs::File::options().write(true).open(&source).unwrap().set_modified(time).unwrap();
        fs::File::options().write(true).open(&destination).unwrap().set_modified(time).unwrap();

        let manifest = Manifest::open(&root.join("state")).unwrap();
        let snapshot = manifest.snapshot("job").unwrap();
        let patterns = PatternSet::new(&["*.lrcat".to_string()]).unwrap();
        let differ = Differ { snapshot: &snapshot, hashed: Some(&patterns), hash_everything: false, check_destination: false };
        let metadata = fs::metadata(&source).unwrap();
        let file = ScannedFile { path: source, relative: "Main.lrcat".to_string(), size: metadata.len(), modified_ns: modified_ns(&metadata) };

        let entry = differ.diff(&file, "lr", &root.join("destination/lr"));
        assert!(matches!(entry.action, PlanAction::Update));
        assert_eq!(entry.reason, "content differs");
        fs::remove_dir_all(root).unwrap();
    }
}
