use crate::filters::{Filters, PatternSet};
use crate::manifest::{Manifest, RecordUpdate};
use crate::manifest::Snapshot;
use crate::phone::{self, Phones};
use crate::profile::{BackupMode, BackupProfile, DestinationKind, SourceKind, Verification, Versioning};
use crate::scan::{Differ, ScannedFile, Walker};
use crate::transfer::{CopiedFile, CopyError, Transfer};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The UI only shows a summary, so huge plans are not sent over IPC in full.
const MAX_REPORTED_ENTRIES: usize = 2000;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanAction {
    Copy,
    Update,
    #[default]
    Skip,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    pub action: PlanAction,
    pub source_path: String,
    pub destination_path: String,
    pub relative_path: String,
    pub size: u64,
    pub reason: String,
    #[serde(skip)]
    pub source_id: String,
    #[serde(skip)]
    pub modified_ns: i64,
    #[serde(skip)]
    pub verify_hash: bool,
    #[serde(skip)]
    pub known_hash: Option<String>,
    /// The destination already matches but the manifest has no record of it yet.
    #[serde(skip)]
    pub record_metadata: bool,
    /// The file lives on a phone (MTP object handle, valid for this run only).
    #[serde(skip)]
    pub phone_source: Option<u64>,
    /// The destination is on a phone. Holds the handle of the copy that an update replaces.
    #[serde(skip)]
    pub phone_destination: Option<Option<u64>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupWarning {
    pub level: String,
    pub message: String,
    pub path: Option<String>,
}

impl BackupWarning {
    pub fn warning(message: impl Into<String>, path: Option<&Path>) -> Self {
        Self { level: "warning".to_string(), message: message.into(), path: path.map(|path| path.display().to_string()) }
    }

    pub fn danger(message: impl Into<String>, path: Option<&Path>) -> Self {
        Self { level: "danger".to_string(), message: message.into(), path: path.map(|path| path.display().to_string()) }
    }

    fn info(message: impl Into<String>) -> Self {
        Self { level: "info".to_string(), message: message.into(), path: None }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupTotals {
    pub scanned_files: u64,
    pub files_to_write: u64,
    pub files_skipped: u64,
    pub bytes_to_write: u64,
    pub bytes_skipped: u64,
    pub files_excluded: u64,
    /// Files in the backup whose source is gone. They are kept, never deleted.
    pub missing_from_source: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPlan {
    pub profile_id: String,
    pub destination_root: String,
    pub created_at: String,
    /// Only files that will be written, capped at `MAX_REPORTED_ENTRIES`.
    pub entries: Vec<PlanEntry>,
    pub entries_truncated: bool,
    pub warnings: Vec<BackupWarning>,
    pub totals: BackupTotals,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupRunResult {
    #[serde(flatten)]
    pub plan: BackupPlan,
    pub completed_at: String,
    pub copied_files: u64,
    pub archived_files: u64,
    pub failed_files: u64,
    pub cancelled: bool,
    pub errors: Vec<BackupWarning>,
    /// Name of the snapshot made after this run, when the job uses snapshots.
    pub snapshot: Option<String>,
}

/// Everything a run needs, including the full work list that is not sent to the UI.
pub struct PreparedPlan {
    pub plan: BackupPlan,
    work: Vec<PlanEntry>,
    adopted: Vec<PlanEntry>,
    missing: Vec<RecordUpdate>,
}

#[derive(Clone)]
pub struct RunControl {
    paused: Arc<(Mutex<bool>, Condvar)>,
    cancelled: Arc<AtomicBool>,
}

impl RunControl {
    pub fn new() -> Self {
        Self { paused: Arc::new((Mutex::new(false), Condvar::new())), cancelled: Arc::new(AtomicBool::new(false)) }
    }

    pub fn pause(&self) {
        if let Ok(mut paused) = self.paused.0.lock() { *paused = true; }
    }

    pub fn resume(&self) {
        if let Ok(mut paused) = self.paused.0.lock() {
            *paused = false;
            self.paused.1.notify_all();
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.resume();
    }

    /// Blocks while paused. Returns true when the run is cancelled.
    pub fn wait(&self) -> bool {
        let Ok(mut paused) = self.paused.0.lock() else { return true };
        while *paused && !self.is_cancelled() {
            paused = match self.paused.1.wait(paused) { Ok(value) => value, Err(_) => return true };
        }
        self.is_cancelled()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.0.lock().map(|paused| *paused).unwrap_or(false)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub run_id: String,
    /// `scanning`, `copying`, `finished`, or a control event (`paused`, `resumed`, `cancelled`).
    pub event: String,
    pub path: Option<String>,
    pub discovered_files: u64,
    pub evaluated_files: u64,
    pub completed_files: u64,
    pub total_files: u64,
    pub copied_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: u64,
    pub bytes_per_second: u64,
}

impl BackupProgress {
    pub fn control_event(run_id: String, event: &str) -> Self {
        Self { run_id, event: event.to_string(), path: None, discovered_files: 0, evaluated_files: 0, completed_files: 0, total_files: 0, copied_bytes: 0, total_bytes: 0, elapsed_seconds: 0, bytes_per_second: 0 }
    }
}

/// Shared counters that workers update and a ticker thread turns into throttled progress events.
pub struct ProgressTracker {
    run_id: String,
    started: Instant,
    phase: Mutex<&'static str>,
    current: Mutex<Option<String>>,
    pub discovered: AtomicU64,
    pub completed_files: AtomicU64,
    pub total_files: AtomicU64,
    pub copied_bytes: AtomicU64,
    pub total_bytes: AtomicU64,
}

impl ProgressTracker {
    fn new(run_id: String) -> Self {
        Self {
            run_id,
            started: Instant::now(),
            phase: Mutex::new("scanning"),
            current: Mutex::new(None),
            discovered: AtomicU64::new(0),
            completed_files: AtomicU64::new(0),
            total_files: AtomicU64::new(0),
            copied_bytes: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
        }
    }

    pub fn set_current(&self, path: &str) {
        if let Ok(mut current) = self.current.lock() { *current = Some(path.to_string()); }
    }

    fn set_phase(&self, phase: &'static str) {
        if let Ok(mut current) = self.phase.lock() { *current = phase; }
    }

    fn snapshot(&self) -> BackupProgress {
        let elapsed = self.started.elapsed();
        let copied_bytes = self.copied_bytes.load(Ordering::Relaxed);
        let discovered = self.discovered.load(Ordering::Relaxed);
        BackupProgress {
            run_id: self.run_id.clone(),
            event: self.phase.lock().map(|phase| phase.to_string()).unwrap_or_default(),
            path: self.current.lock().ok().and_then(|current| current.clone()),
            discovered_files: discovered,
            evaluated_files: discovered,
            completed_files: self.completed_files.load(Ordering::Relaxed),
            total_files: self.total_files.load(Ordering::Relaxed),
            copied_bytes,
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            elapsed_seconds: elapsed.as_secs(),
            bytes_per_second: (copied_bytes as f64 / elapsed.as_secs_f64().max(0.001)) as u64,
        }
    }
}

pub fn plan(profile: &BackupProfile, manifest: &Manifest) -> Result<PreparedPlan, String> {
    prepare(profile, manifest, &AtomicU64::new(0), &mut Phones::new())
}

/// The backup folder on a phone, listed once per run.
struct PhoneDestination {
    device_id: String,
    folder: String,
    /// Lowercased path inside the backup folder, to object handle and size.
    files: HashMap<String, (u64, u64)>,
}

fn prepare(profile: &BackupProfile, manifest: &Manifest, discovered: &AtomicU64, phones: &mut Phones) -> Result<PreparedPlan, String> {
    let mut warnings = Vec::new();
    let skip_live_catalogs = profile.guard_enabled("lightroom-closed") && lightroom_is_running();
    if skip_live_catalogs {
        warnings.push(BackupWarning::warning("Lightroom Classic appears to be running. Live catalog files will be skipped.", None));
    }

    profile.check_layout()?;
    let local_destination = !matches!(profile.destination_kind, DestinationKind::QnapRsync | DestinationKind::Android);
    if local_destination {
        check_destination(profile)?;
    }
    if profile.mode == BackupMode::SafeMirror && profile.versioning == Versioning::Snapshots && !(local_destination && uses_snapshots(profile)) {
        warnings.push(BackupWarning::info("Snapshots need an APFS drive. Older versions go to the _Archive folder instead."));
    }
    if !local_destination && profile.sources.iter().any(|source| source.kind == SourceKind::Android) {
        return Err("Phone folders can only be backed up to a drive or a mounted folder.".to_string());
    }
    let phone_destination = match profile.destination_kind {
        DestinationKind::Android => Some(list_phone_destination(profile, phones)?),
        _ => None,
    };

    let filters = Filters::new(&profile.filters)?;
    let important = PatternSet::new(&profile.important_patterns())?;
    let snapshot = manifest.snapshot(&profile.id)?;
    let differ = Differ {
        snapshot: &snapshot,
        hashed: (profile.verification == Verification::ImportantHash).then_some(&important),
        hash_everything: profile.verification == Verification::FullHash,
        check_destination: profile.check_destination,
    };
    let walker = Walker { filters: &filters, skip_live_catalogs, discovered };

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut scanned_roots = Vec::new();
    let mut excluded = 0;
    let mut symlinks = 0;
    let mut remote_files = 0;
    let phone_differ = Differ { hashed: None, hash_everything: false, ..differ };
    let wants_hash = |relative: &str| differ.hash_everything || differ.hashed.is_some_and(|patterns| patterns.is_match(relative));
    for source in &profile.sources {
        if source.kind == SourceKind::Android {
            let Some((root, files, handles)) = list_phone_source(source, phones, &filters, discovered, &mut excluded, &mut warnings) else { continue };
            entries.extend(files.iter().map(|file| {
                let mut entry = phone_differ.diff(file, &source.id, &profile.destination_base(&source.id));
                entry.phone_source = handles.get(&file.relative).copied();
                // The phone copy is hashed while downloading and checked after writing.
                entry.verify_hash = wants_hash(&file.relative);
                entry
            }));
            seen.extend(files.into_iter().map(|file| file.path.to_string_lossy().into_owned()));
            scanned_roots.push((PathBuf::from(root), profile.destination_base(&source.id)));
            continue;
        }
        let source_root = PathBuf::from(&source.path);
        if source.path.trim().is_empty() || !source_root.is_dir() {
            warnings.push(BackupWarning::warning("Source folder does not exist or is not a folder.", Some(&source_root)));
            continue;
        }
        let walked = walker.walk(&source_root);
        warnings.extend(walked.warnings);
        excluded += walked.excluded;
        symlinks += walked.symlinks;
        if let Some(destination) = &phone_destination {
            entries.extend(walked.files.iter().map(|file| diff_to_phone(file, &source.id, &profile.destination_relative(&source.id, &file.relative), destination, &snapshot)));
            scanned_roots.push((source_root, PathBuf::from(phone::uri(&destination.device_id, &destination.folder, &profile.destination_relative(&source.id, "")))));
        } else if local_destination {
            let base = profile.destination_base(&source.id);
            entries.extend(walked.files.par_iter().map(|file| differ.diff(file, &source.id, &base)).collect::<Vec<_>>());
            scanned_roots.push((source_root, base));
        } else {
            remote_files += walked.files.len() as u64;
        }
        seen.extend(walked.files.into_iter().map(|file| file.path.to_string_lossy().into_owned()));
    }
    if symlinks > 0 {
        warnings.push(BackupWarning::info(format!("{symlinks} symbolic link(s) were not followed.")));
    }

    if !local_destination {
        warnings.push(BackupWarning::info(format!("{remote_files} file(s) found. rsync compares them with the NAS during the run.")));
    }

    if phone_destination.is_some() && entries.iter().any(|entry| matches!(entry.action, PlanAction::Update)) {
        warnings.push(BackupWarning::info("Changed files are replaced on the phone. Phones keep no archive of older versions."));
    }

    let missing = if scanned_roots.is_empty() { Vec::new() } else { missing_from_source(profile, &snapshot, &seen, &scanned_roots) };
    if !missing.is_empty() {
        warnings.push(BackupWarning::info(format!("{} file(s) were removed from the source. Their backup copies are kept.", missing.len())));
    }

    let mut totals = BackupTotals { scanned_files: remote_files, files_excluded: excluded, missing_from_source: missing.len() as u64, ..BackupTotals::default() };
    for entry in &entries {
        totals.scanned_files += 1;
        if matches!(entry.action, PlanAction::Skip) {
            totals.files_skipped += 1;
            totals.bytes_skipped += entry.size;
        } else {
            totals.files_to_write += 1;
            totals.bytes_to_write += entry.size;
        }
    }

    let (work, skipped): (Vec<PlanEntry>, Vec<PlanEntry>) = entries.into_iter().partition(|entry| !matches!(entry.action, PlanAction::Skip));
    let adopted = skipped.into_iter().filter(|entry| entry.record_metadata).collect();
    Ok(PreparedPlan {
        plan: BackupPlan {
            profile_id: profile.id.clone(),
            destination_root: profile.destination_root.clone(),
            created_at: timestamp(),
            entries: work.iter().take(MAX_REPORTED_ENTRIES).cloned().collect(),
            entries_truncated: work.len() > MAX_REPORTED_ENTRIES,
            warnings,
            totals,
        },
        work,
        adopted,
        missing,
    })
}

type PhoneListing = (String, Vec<ScannedFile>, HashMap<String, u64>);

/// Lists a phone folder as scanned files. Problems become warnings so other sources still run.
fn list_phone_source(
    source: &crate::profile::ProfileSource,
    phones: &mut Phones,
    filters: &Filters,
    discovered: &AtomicU64,
    excluded: &mut u64,
    warnings: &mut Vec<BackupWarning>,
) -> Option<PhoneListing> {
    let folder = source.path.trim_matches('/');
    let session = match phones.session(&source.device_id, source.storage_id) {
        Ok(session) => session,
        Err(error) => {
            warnings.push(BackupWarning::warning(error, Some(Path::new(folder))));
            return None;
        }
    };
    let listed = match session.list_files(folder) {
        Ok(Some(files)) => files,
        Ok(None) => {
            warnings.push(BackupWarning::info(format!("The phone has no {folder} folder. It was skipped.")));
            return None;
        }
        Err(error) => {
            warnings.push(BackupWarning::warning(error, Some(Path::new(folder))));
            return None;
        }
    };

    let device_id = session.device_id.clone();
    let mut files = Vec::new();
    let mut handles = HashMap::new();
    for file in listed {
        let pruned = file.relative.match_indices('/').any(|(index, _)| filters.prunes_directory(&file.relative[..index]));
        if pruned || !filters.includes_file(&file.relative) {
            *excluded += 1;
            continue;
        }
        discovered.fetch_add(1, Ordering::Relaxed);
        handles.insert(file.relative.clone(), file.handle);
        files.push(ScannedFile { path: PathBuf::from(phone::uri(&device_id, folder, &file.relative)), relative: file.relative, size: file.size, modified_ns: file.modified_ns });
    }
    Some((phone::uri(&device_id, folder, ""), files, handles))
}

fn list_phone_destination(profile: &BackupProfile, phones: &mut Phones) -> Result<PhoneDestination, String> {
    let folder = profile.destination_root.trim_matches('/').to_string();
    if folder.is_empty() {
        return Err("Choose a folder on the phone, for example Backup.".to_string());
    }
    let target = profile.phone.clone().unwrap_or_default();
    let session = phones.session(&target.device_id, target.storage_id)?;
    let files = session
        .list_files(&folder)?
        .unwrap_or_default()
        .into_iter()
        .map(|file| (file.relative.to_lowercase(), (file.handle, file.size)))
        .collect();
    Ok(PhoneDestination { device_id: session.device_id.clone(), folder, files })
}

/// Phones report sizes reliably but not modification times, so the phone copy is compared by size
/// and the manifest tells whether the source changed since it was sent.
fn diff_to_phone(file: &ScannedFile, source_id: &str, relative: &str, destination: &PhoneDestination, snapshot: &Snapshot) -> PlanEntry {
    let source_path = file.path.to_string_lossy().into_owned();
    let destination_path = phone::uri(&destination.device_id, &destination.folder, relative);
    let existing = destination.files.get(&relative.to_lowercase());
    let record = snapshot.get(&source_path, &destination_path);
    let (action, reason, record_metadata) = match (existing, record) {
        (None, _) => (PlanAction::Copy, "missing from phone", false),
        (Some((_, size)), _) if *size != file.size => (PlanAction::Update, "size changed", false),
        (Some(_), Some(record)) if record.size == file.size && record.modified_ns == Some(file.modified_ns) => (PlanAction::Skip, "unchanged", false),
        (Some(_), Some(_)) => (PlanAction::Update, "modified time changed", false),
        (Some(_), None) => (PlanAction::Skip, "unchanged", true),
    };
    PlanEntry {
        action,
        source_path,
        destination_path,
        relative_path: file.relative.clone(),
        size: file.size,
        reason: reason.to_string(),
        source_id: source_id.to_string(),
        modified_ns: file.modified_ns,
        record_metadata,
        phone_destination: Some(existing.map(|(handle, _)| *handle)),
        ..PlanEntry::default()
    }
}

/// Refuses destinations that would silently write to the wrong place.
fn check_destination(profile: &BackupProfile) -> Result<(), String> {
    if profile.destination_root.trim().is_empty() {
        return Err("Choose a destination folder first.".to_string());
    }
    let destination = Path::new(&profile.destination_root);
    // An unplugged drive leaves /Volumes/<name> missing. Creating it would fill the system disk instead.
    if !destination.is_dir() {
        return Err(format!("The destination {} is not available. Connect the drive and try again.", destination.display()));
    }
    let destination = destination.canonicalize().map_err(|error| error.to_string())?;
    for source in profile.sources.iter().filter(|source| !source.path.trim().is_empty()) {
        if let Ok(source) = Path::new(&source.path).canonicalize()
            && (destination.starts_with(&source) || source.starts_with(&destination)) {
                return Err(format!("The destination and source {} overlap. Pick a destination outside the source folder.", source.display()));
            }
    }
    Ok(())
}

fn missing_from_source(profile: &BackupProfile, snapshot: &crate::manifest::Snapshot, seen: &HashSet<String>, roots: &[(PathBuf, PathBuf)]) -> Vec<RecordUpdate> {
    snapshot
        .iter()
        .filter(|((source, destination), _)| {
            !seen.contains(source)
                && roots.iter().any(|(source_root, destination_root)| Path::new(source).starts_with(source_root) && Path::new(destination).starts_with(destination_root))
                // Still on disk means it was excluded by a filter, not removed.
                && !Path::new(source).exists()
        })
        .map(|((source, destination), record)| RecordUpdate {
            profile_id: profile.id.clone(),
            source_path: source.clone(),
            destination_path: destination.clone(),
            size: record.size,
            modified_ns: record.modified_ns.unwrap_or_default(),
            hash: record.hash.clone(),
            status: "source-missing",
            recorded_at: timestamp(),
        })
        .collect()
}

pub fn run(
    profile: BackupProfile,
    manifest: Manifest,
    control: RunControl,
    run_id: String,
    cached_plan: Option<Arc<PreparedPlan>>,
    emit: &(dyn Fn(BackupProgress) + Sync),
) -> Result<BackupRunResult, String> {
    let tracker = ProgressTracker::new(run_id);
    let finished = AtomicBool::new(false);

    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Relaxed) {
                if !control.is_paused() { emit(tracker.snapshot()); }
                std::thread::sleep(PROGRESS_INTERVAL);
            }
        });
        let result = match profile.destination_kind {
            DestinationKind::QnapRsync => run_rsync(&profile, &control, &tracker),
            _ => run_local(&profile, &manifest, &control, &tracker, cached_plan),
        };
        finished.store(true, Ordering::Relaxed);
        tracker.set_phase("finished");
        emit(tracker.snapshot());
        result
    })
}

fn run_local(profile: &BackupProfile, manifest: &Manifest, control: &RunControl, tracker: &ProgressTracker, cached_plan: Option<Arc<PreparedPlan>>) -> Result<BackupRunResult, String> {
    let mut phones = Phones::new();
    // Phone object handles are only valid while the phone stays connected, so phone jobs always scan again.
    let prepared = match cached_plan.filter(|_| !profile.uses_phone()) {
        Some(plan) => {
            check_destination(profile)?;
            tracker.discovered.store(plan.plan.totals.scanned_files, Ordering::Relaxed);
            plan
        }
        None => Arc::new(prepare(profile, manifest, &tracker.discovered, &mut phones)?),
    };
    let writer = manifest.writer()?;
    let now = timestamp();
    for entry in &prepared.adopted {
        writer.record(record_update(profile, entry, entry.modified_ns, entry.known_hash.clone(), &now));
    }
    for missing in &prepared.missing {
        writer.record(missing.clone());
    }

    tracker.total_files.store(prepared.work.len() as u64, Ordering::Relaxed);
    tracker.total_bytes.store(prepared.plan.totals.bytes_to_write, Ordering::Relaxed);
    tracker.set_phase("copying");

    let destination_root = PathBuf::from(&profile.destination_root);
    let archive_root = destination_root.join("_Archive").join(archive_run_name());
    // With snapshots, replaced files stay in the previous snapshot, so nothing goes to _Archive.
    let snapshots = profile.mode == BackupMode::SafeMirror && profile.versioning == Versioning::Snapshots && uses_snapshots(profile);
    let transfer = Transfer {
        control,
        copied_bytes: &tracker.copied_bytes,
        archive_root: (profile.mode == BackupMode::SafeMirror && !snapshots).then_some(archive_root.as_path()),
        destination_root: &destination_root,
        preserve_metadata: profile.preserve_mac_metadata,
    };
    let archived_files = AtomicU64::new(0);
    let failed_files = AtomicU64::new(0);
    let errors = Mutex::new(Vec::new());

    let finished = |entry: &PlanEntry, result: Result<CopiedFile, CopyError>| match result {
        Ok(copied) => {
            writer.record(RecordUpdate { size: copied.size, ..record_update(profile, entry, copied.modified_ns, copied.hash, &timestamp()) });
            tracker.completed_files.fetch_add(1, Ordering::Relaxed);
            if copied.archived { archived_files.fetch_add(1, Ordering::Relaxed); }
        }
        Err(CopyError::Cancelled) => {}
        Err(CopyError::Failed(message)) => {
            failed_files.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut list) = errors.lock() { list.push(BackupWarning::danger(message, Some(Path::new(&entry.source_path)))); }
        }
    };

    // A phone has one USB connection, so its files go one by one while local files copy in parallel next to it.
    let (phone_work, local_work): (Vec<&PlanEntry>, Vec<&PlanEntry>) =
        prepared.work.iter().partition(|entry| entry.phone_source.is_some() || entry.phone_destination.is_some());
    let pool = rayon::ThreadPoolBuilder::new().num_threads(profile.parallel_copies()).build().map_err(|error| error.to_string())?;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            pool.install(|| {
                local_work.par_iter().for_each(|entry| {
                    if control.wait() { return; }
                    tracker.set_current(&entry.source_path);
                    finished(entry, transfer.copy(entry));
                });
            });
        });
        for entry in phone_work {
            if control.wait() { break; }
            tracker.set_current(&entry.source_path);
            finished(entry, copy_phone_entry(profile, entry, &transfer, &mut phones, control, &tracker.copied_bytes));
        }
    });
    drop(phones);
    writer.finish();

    let mut errors = errors.into_inner().unwrap_or_default();
    let changed = tracker.completed_files.load(Ordering::Relaxed) > 0 || !prepared.adopted.is_empty();
    let mut snapshot = None;
    if snapshots && !control.is_cancelled() && (changed || crate::snapshots::list(&destination_root).is_empty()) {
        tracker.set_phase("snapshot");
        let name = archive_run_name();
        match crate::snapshots::create(&destination_root, &name) {
            Ok(_) => snapshot = Some(name),
            Err(error) => errors.push(BackupWarning::warning(format!("Could not make a snapshot: {error}"), None)),
        }
    }
    if let Some(days) = profile.archive_keep_days.filter(|_| !control.is_cancelled() && profile.destination_kind != DestinationKind::Android) {
        let pruned = if snapshots {
            crate::snapshots::prune(&destination_root, days, unix_seconds()).map(|_| ())
        } else {
            prune_archive(&destination_root.join("_Archive"), days)
        };
        if let Err(error) = pruned {
            errors.push(BackupWarning::warning(format!("Could not clean up old versions: {error}"), None));
        }
    }

    let plan = Arc::try_unwrap(prepared).map(|prepared| prepared.plan).unwrap_or_else(|shared| shared.plan.clone());
    Ok(BackupRunResult {
        plan,
        completed_at: timestamp(),
        copied_files: tracker.completed_files.load(Ordering::Relaxed),
        archived_files: archived_files.load(Ordering::Relaxed),
        failed_files: failed_files.load(Ordering::Relaxed),
        cancelled: control.is_cancelled(),
        errors,
        snapshot,
    })
}

fn uses_snapshots(profile: &BackupProfile) -> bool {
    profile.destination_kind != DestinationKind::Android && crate::snapshots::supported(Path::new(&profile.destination_root))
}

fn copy_phone_entry(profile: &BackupProfile, entry: &PlanEntry, transfer: &Transfer, phones: &mut Phones, control: &RunControl, copied_bytes: &AtomicU64) -> Result<CopiedFile, CopyError> {
    if let Some(handle) = entry.phone_source {
        let source = profile.sources.iter().find(|source| source.id == entry.source_id).ok_or_else(|| CopyError::Failed("Source is no longer in the job".to_string()))?;
        let session = phones.session(&source.device_id, source.storage_id).map_err(CopyError::Failed)?;
        return transfer.copy_remote(entry, |partial| session.download(handle, partial, entry.verify_hash, control, copied_bytes));
    }

    let target = profile.phone.clone().unwrap_or_default();
    let session = phones.session(&target.device_id, target.storage_id).map_err(CopyError::Failed)?;
    let source = Path::new(&entry.source_path);
    let metadata = fs::metadata(source)?;
    let modified_ns = crate::scan::modified_ns(&metadata);
    let relative = profile.destination_relative(&entry.source_id, &entry.relative_path);
    let (directory, name) = relative.rsplit_once('/').unwrap_or(("", relative.as_str()));
    let root = profile.destination_root.trim_matches('/');
    let folder = if directory.is_empty() { root.to_string() } else { format!("{root}/{directory}") };
    session.upload(&folder, name, source, metadata.len(), modified_ns, entry.phone_destination.flatten(), control, copied_bytes)?;
    Ok(CopiedFile { archived: false, size: metadata.len(), modified_ns, hash: None })
}

fn run_rsync(profile: &BackupProfile, control: &RunControl, tracker: &ProgressTracker) -> Result<BackupRunResult, String> {
    let skip_live_catalogs = profile.guard_enabled("lightroom-closed") && lightroom_is_running();
    tracker.set_phase("copying");
    let outcome = crate::rsync::run(profile, control, tracker, skip_live_catalogs)?;
    let mut warnings = Vec::new();
    if skip_live_catalogs {
        warnings.push(BackupWarning::warning("Lightroom Classic appears to be running. Live catalog files were skipped.", None));
    }
    Ok(BackupRunResult {
        plan: BackupPlan {
            profile_id: profile.id.clone(),
            destination_root: profile.destination_root.clone(),
            created_at: timestamp(),
            entries: Vec::new(),
            entries_truncated: false,
            warnings,
            totals: BackupTotals { files_to_write: outcome.transferred_files, ..BackupTotals::default() },
        },
        completed_at: timestamp(),
        copied_files: outcome.transferred_files,
        archived_files: 0,
        failed_files: outcome.errors.len() as u64,
        cancelled: control.is_cancelled(),
        errors: outcome.errors,
        snapshot: None,
    })
}

fn record_update(profile: &BackupProfile, entry: &PlanEntry, modified_ns: i64, hash: Option<String>, recorded_at: &str) -> RecordUpdate {
    RecordUpdate {
        profile_id: profile.id.clone(),
        source_path: entry.source_path.clone(),
        destination_path: entry.destination_path.clone(),
        size: entry.size,
        modified_ns,
        hash,
        status: "copied",
        recorded_at: recorded_at.to_string(),
    }
}

/// Deletes archive runs older than `days`. Run folders are named `YYYY-MM-DD_HHMMSS` (older versions used unix seconds).
fn prune_archive(archive: &Path, days: u32) -> std::io::Result<()> {
    let Ok(entries) = fs::read_dir(archive) else { return Ok(()) };
    let cutoff = unix_seconds().saturating_sub(u64::from(days) * 86_400);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(created) = parse_archive_run_name(&name) else { continue };
        if created < cutoff && entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

pub fn archive_run_name() -> String {
    let seconds = unix_seconds();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time = seconds % 86_400;
    format!("{year:04}-{month:02}-{day:02}_{:02}{:02}{:02}", time / 3600, time % 3600 / 60, time % 60)
}

pub fn parse_archive_run_name(name: &str) -> Option<u64> {
    if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()) {
        return name.parse().ok();
    }
    let (date, time) = name.split_once('_')?;
    let mut parts = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let time: u64 = time.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(days as u64 * 86_400 + time / 10_000 * 3600 + time / 100 % 100 * 60 + time % 100)
}

/// Howard Hinnant's date algorithms, which avoid a date crate for two conversions.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or_default()
}

fn timestamp() -> String {
    unix_seconds().to_string()
}

fn lightroom_is_running() -> bool {
    cfg!(target_os = "macos") && Command::new("pgrep").args(["-x", "Adobe Lightroom Classic"]).status().map(|status| status.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("lbs-backup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn profile(root: &Path) -> BackupProfile {
        serde_json::from_value(serde_json::json!({
            "id": "job",
            "destinationRoot": root.join("destination"),
            "sources": [{ "id": "photos", "path": root.join("source") }],
            "verification": "important-hash",
            "importantPatterns": ["*.xmp"],
        })).unwrap()
    }

    fn run_once(profile: &BackupProfile, manifest: &Manifest) -> BackupRunResult {
        run(profile.clone(), manifest.clone(), RunControl::new(), "run".to_string(), None, &|_| {}).expect("run should succeed")
    }

    #[test]
    fn second_run_skips_everything_and_edits_are_archived() {
        let root = temp_dir("incremental");
        fs::create_dir_all(root.join("source/2024")).unwrap();
        fs::create_dir_all(root.join("destination")).unwrap();
        fs::write(root.join("source/2024/IMG_1.CR3"), b"raw").unwrap();
        fs::write(root.join("source/2024/IMG_1.xmp"), b"rating=3").unwrap();
        let manifest = Manifest::open(&root.join("state")).unwrap();
        let profile = profile(&root);

        let first = run_once(&profile, &manifest);
        assert_eq!((first.copied_files, first.failed_files), (2, 0), "{:?}", first.errors);

        let second = plan(&profile, &manifest).unwrap().plan;
        assert_eq!(second.totals.files_to_write, 0, "{:?}", second.entries);

        // Same size and the timestamp restored: only the hash notices the edit.
        let sidecar = root.join("source/2024/IMG_1.xmp");
        let modified = fs::metadata(&sidecar).unwrap().modified().unwrap();
        fs::write(&sidecar, b"rating=5").unwrap();
        fs::File::options().write(true).open(&sidecar).unwrap().set_modified(modified).unwrap();
        let third = run_once(&profile, &manifest);
        assert_eq!((third.copied_files, third.archived_files), (1, 1));
        assert_eq!(fs::read(root.join("destination/photos/2024/IMG_1.xmp")).unwrap(), b"rating=5");

        fs::remove_file(root.join("source/2024/IMG_1.CR3")).unwrap();
        let fourth = plan(&profile, &manifest).unwrap().plan;
        assert_eq!(fourth.totals.missing_from_source, 1);
        assert!(root.join("destination/photos/2024/IMG_1.CR3").exists(), "source deletions are never propagated");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_a_destination_that_is_not_mounted_or_inside_the_source() {
        let root = temp_dir("destination");
        fs::create_dir_all(root.join("source/nested")).unwrap();
        let manifest = Manifest::open(&root.join("state")).unwrap();
        let mut profile = profile(&root);

        assert!(plan(&profile, &manifest).err().unwrap().contains("not available"));
        profile.destination_root = root.join("source/nested").to_string_lossy().into_owned();
        assert!(plan(&profile, &manifest).err().unwrap().contains("overlap"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backs_up_a_phone_camera_roll_incrementally() {
        let root = temp_dir("phone-source");
        fs::create_dir_all(root.join("phone/DCIM/Camera/.thumbnails")).unwrap();
        fs::create_dir_all(root.join("destination")).unwrap();
        fs::write(root.join("phone/DCIM/Camera/IMG_1.jpg"), b"photo-one").unwrap();
        fs::write(root.join("phone/DCIM/Camera/VID_1.mp4"), b"video-bytes").unwrap();
        fs::write(root.join("phone/DCIM/Camera/.thumbnails/IMG_1.jpg"), b"thumb").unwrap();
        let location = crate::phone::tests::virtual_phone("camera-roll-test", root.join("phone"));
        let manifest = Manifest::open(&root.join("state")).unwrap();
        let profile: BackupProfile = serde_json::from_value(serde_json::json!({
            "id": "phone", "destinationRoot": root.join("destination"), "verification": "full-hash",
            "filters": [{ "pattern": "**/.thumbnails/**", "action": "exclude" }],
            "sources": [
                { "id": "camera", "path": "DCIM/Camera", "kind": "android", "deviceId": "camera-roll-test" },
                { "id": "screenshots", "path": "Pictures/Screenshots", "kind": "android", "deviceId": "camera-roll-test" }
            ],
        })).unwrap();

        let first = run_once(&profile, &manifest);
        assert_eq!((first.copied_files, first.failed_files), (2, 0), "{:?}", first.errors);
        assert_eq!(first.plan.totals.files_excluded, 1);
        assert!(first.plan.warnings.iter().any(|warning| warning.message.contains("Pictures/Screenshots")));
        assert_eq!(fs::read(root.join("destination/camera/VID_1.mp4")).unwrap(), b"video-bytes");
        let phone_time = fs::metadata(root.join("phone/DCIM/Camera/IMG_1.jpg")).unwrap().modified().unwrap();
        let copy_time = fs::metadata(root.join("destination/camera/IMG_1.jpg")).unwrap().modified().unwrap();
        assert!(phone_time.duration_since(copy_time).unwrap_or_else(|error| error.duration()).as_secs() < 2);

        assert_eq!(plan(&profile, &manifest).unwrap().plan.totals.files_to_write, 0);

        // Freeing space on the phone never deletes the backup.
        fs::remove_file(root.join("phone/DCIM/Camera/IMG_1.jpg")).unwrap();
        crate::phone::tests::rescan("camera-roll-test");
        let after_delete = plan(&profile, &manifest).unwrap().plan;
        assert_eq!(after_delete.totals.missing_from_source, 1);
        assert!(root.join("destination/camera/IMG_1.jpg").exists());

        mtp_rs::unregister_virtual_device(location);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sends_folders_to_a_backup_folder_on_the_phone() {
        let root = temp_dir("phone-destination");
        fs::create_dir_all(root.join("source/2024")).unwrap();
        fs::write(root.join("source/2024/IMG_1.jpg"), b"photo-one").unwrap();
        let location = crate::phone::tests::virtual_phone("phone-destination-test", root.join("phone"));
        let manifest = Manifest::open(&root.join("state")).unwrap();
        let profile: BackupProfile = serde_json::from_value(serde_json::json!({
            "id": "to-phone", "destinationKind": "android", "destinationRoot": "Backup",
            "phone": { "deviceId": "phone-destination-test" },
            "sources": [{ "id": "photos", "path": root.join("source") }],
        })).unwrap();

        let first = run_once(&profile, &manifest);
        assert_eq!((first.copied_files, first.failed_files), (1, 0), "{:?}", first.errors);
        assert_eq!(fs::read(root.join("phone/Backup/photos/2024/IMG_1.jpg")).unwrap(), b"photo-one");
        assert_eq!(plan(&profile, &manifest).unwrap().plan.totals.files_to_write, 0);

        fs::write(root.join("source/2024/IMG_1.jpg"), b"photo-one-edited").unwrap();
        let second = run_once(&profile, &manifest);
        assert_eq!((second.copied_files, second.failed_files), (1, 0), "{:?}", second.errors);
        crate::phone::tests::rescan("phone-destination-test");
        assert_eq!(fs::read(root.join("phone/Backup/photos/2024/IMG_1.jpg")).unwrap(), b"photo-one-edited");
        assert_eq!(plan(&profile, &manifest).unwrap().plan.totals.files_to_write, 0);

        mtp_rs::unregister_virtual_device(location);
        fs::remove_dir_all(root).unwrap();
    }

    /// Copies `source` the way Carbon Copy Cloner does: same tree, same modification times.
    fn ccc_style_copy(source: &Path, destination: &Path) {
        for entry in fs::read_dir(source).unwrap().flatten() {
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target).unwrap();
                ccc_style_copy(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
                fs::File::options().write(true).open(&target).unwrap().set_modified(entry.metadata().unwrap().modified().unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn mirror_layout_takes_over_an_existing_ccc_backup_without_copying() {
        let root = temp_dir("mirror");
        fs::create_dir_all(root.join("source/Video/2024")).unwrap();
        fs::create_dir_all(root.join("destination")).unwrap();
        fs::write(root.join("source/Video/2024/clip.mov"), b"frames").unwrap();
        fs::write(root.join("source/notes.txt"), b"notes").unwrap();
        ccc_style_copy(&root.join("source"), &root.join("destination"));
        fs::write(root.join("source/new.jpg"), b"new").unwrap();
        let manifest = Manifest::open(&root.join("state")).unwrap();
        let mut profile = profile(&root);
        profile.layout = crate::profile::Layout::Mirror;
        profile.verification = Verification::Metadata;

        let result = run_once(&profile, &manifest);
        assert_eq!(result.plan.totals.files_skipped, 2, "existing CCC files are recognised");
        assert_eq!(result.copied_files, 1);
        assert_eq!(fs::read(root.join("destination/new.jpg")).unwrap(), b"new");
        assert!(!root.join("destination/photos").exists(), "mirror writes into the root, not a source folder");

        profile.sources.push(profile.sources[0].clone());
        assert!(plan(&profile, &manifest).err().unwrap().contains("exactly one source"));
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn snapshot_versioning_keeps_every_run_without_an_archive_folder() {
        let root = temp_dir("snapshot-runs");
        fs::create_dir_all(root.join("source")).unwrap();
        fs::create_dir_all(root.join("destination")).unwrap();
        fs::write(root.join("source/IMG_1.xmp"), b"rating=3").unwrap();
        let manifest = Manifest::open(&root.join("state")).unwrap();
        let mut profile = profile(&root);
        profile.versioning = Versioning::Snapshots;
        profile.verification = Verification::Metadata;

        let first = run_once(&profile, &manifest);
        let first_snapshot = first.snapshot.expect("first run makes a snapshot");
        assert!(run_once(&profile, &manifest).snapshot.is_none(), "nothing changed, no new snapshot");

        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(root.join("source/IMG_1.xmp"), b"rating=5!").unwrap();
        let third = run_once(&profile, &manifest);
        let third_snapshot = third.snapshot.expect("a change makes a snapshot");
        assert_ne!(first_snapshot, third_snapshot);
        assert_eq!(third.archived_files, 0);
        assert!(!root.join("destination/_Archive").exists());
        assert_eq!(fs::read(root.join(format!("destination/_Snapshots/{first_snapshot}/photos/IMG_1.xmp"))).unwrap(), b"rating=3");
        assert_eq!(fs::read(root.join("destination/photos/IMG_1.xmp")).unwrap(), b"rating=5!");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_run_names_round_trip() {
        let name = archive_run_name();
        let parsed = parse_archive_run_name(&name).unwrap();
        assert!(unix_seconds() - parsed < 5, "{name}");
        assert_eq!(parse_archive_run_name("1700000000"), Some(1_700_000_000));
        assert_eq!(parse_archive_run_name("2023-11-14_221320"), Some(1_700_000_000));
    }
}
