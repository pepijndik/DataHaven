use futures_util::StreamExt;
use mtp_rs::mtp::{DateTime, Error as MtpError, MtpDevice, NewObjectInfo, Storage, UploadError, UtcOffset};
use mtp_rs::ObjectHandle;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio_util::io::ReaderStream;

/// Albums live in this folder on the phone. Each album is one subfolder.
pub const ROOT_FOLDER: &str = "DCIM/Fotografie";
/// The start of the message shown when another process holds the phone. The UI offers a fix when it sees it.
pub const BUSY_MESSAGE: &str = "The phone is in use by another app";

const MEDIA_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "webp", "avif", "gif", "dng", "mp4", "mov", "m4v", "3gp", "mkv", "webm",
];
const UPLOAD_CHUNK: usize = 1024 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Only one process can hold the USB connection, so every phone operation takes turns.
pub(crate) static PHONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CANCEL_SEND: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AndroidDevice {
    pub id: String,
    pub manufacturer: String,
    pub model: String,
    pub serial_number: Option<String>,
    pub location_id: u64,
    pub vendor_id: u16,
    pub product_id: u16,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidStorage {
    pub id: u64,
    pub description: String,
    pub capacity: u64,
    pub free: u64,
    pub writable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidAlbum {
    pub name: String,
    pub files: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidSendIssue {
    pub file: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidSendResult {
    pub sent: u32,
    pub skipped: u32,
    pub failed: u32,
    /// A different file with the same name is already on the phone. It is left alone.
    pub conflicts: u32,
    pub bytes: u64,
    pub destination: String,
    pub dry_run: bool,
    pub cancelled: bool,
    pub issues: Vec<AndroidSendIssue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidProgress {
    pub file: String,
    pub done: u32,
    pub total: u32,
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendRequest {
    pub device_id: String,
    pub storage_id: Option<u64>,
    pub source: String,
    pub album: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

pub fn detect() -> Result<Vec<AndroidDevice>, String> {
    MtpDevice::list_devices()
        .map(|devices| {
            devices
                .into_iter()
                .map(|device| AndroidDevice {
                    id: device.serial_number.clone().unwrap_or_else(|| format!("mtp-{}", device.location_id)),
                    manufacturer: device.manufacturer.unwrap_or_else(|| "Unknown manufacturer".to_string()),
                    model: device.product.unwrap_or_else(|| "Android device".to_string()),
                    serial_number: device.serial_number,
                    location_id: device.location_id,
                    vendor_id: device.vendor_id,
                    product_id: device.product_id,
                })
                .collect()
        })
        .map_err(|error| friendly(&error))
}

/// Calls `on_change` with the connected phones whenever one is plugged in or removed.
/// Falls back to polling when the OS does not offer USB hotplug notifications.
pub fn watch(on_change: impl Fn(Vec<AndroidDevice>) + Send + Sync + 'static) {
    match mtp_rs::mtp::watch_devices() {
        Ok(mut watch) => {
            tauri::async_runtime::spawn(async move {
                while watch.next().await.is_some() {
                    on_change(detect().unwrap_or_default());
                }
            });
        }
        Err(error) => {
            log::warn!("USB hotplug unavailable, polling for phones instead: {error}");
            std::thread::spawn(move || {
                let mut last = None;
                loop {
                    let devices = detect().unwrap_or_default();
                    if last.as_ref() != Some(&devices) {
                        on_change(devices.clone());
                        last = Some(devices);
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
            });
        }
    }
}

/// Error messages a person can act on.
pub(crate) fn friendly(error: &MtpError) -> String {
    match error {
        MtpError::ExclusiveAccess => format!(
            "{BUSY_MESSAGE}. Quit Android File Transfer, Image Capture or Photos and try again{}",
            if cfg!(target_os = "macos") { ", or use \"Free the phone\" to stop the macOS camera service." } else { "." }
        ),
        MtpError::PermissionDenied => "No permission to open the phone. On Linux, add a udev rule for MTP devices.".to_string(),
        MtpError::Disconnected => "The phone was disconnected. Reconnect it, unlock it and choose File transfer.".to_string(),
        MtpError::AccessDenied => "The phone refused access. Unlock it and choose File transfer in the USB notification.".to_string(),
        other => other.to_string(),
    }
}

/// Stops the macOS camera service and Android File Transfer, which grab phones as soon as they connect.
pub fn free_device() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    for process in ["ptpcamerad", "Android File Transfer Agent", "Android File Transfer"] {
        let _ = std::process::Command::new("pkill").args(["-x", process]).status();
    }
    Ok(())
}

pub fn cancel_send() {
    CANCEL_SEND.store(true, Ordering::SeqCst);
}

pub(crate) struct Phone {
    pub(crate) device: MtpDevice,
    pub(crate) storage: Storage,
}

impl Phone {
    pub(crate) async fn open(device_id: &str, storage_id: Option<u64>) -> Result<Self, String> {
        let device = match MtpDevice::open_by_serial(device_id).await {
            Ok(device) => device,
            Err(error) => match device_id.strip_prefix("mtp-").and_then(|id| id.parse::<u64>().ok()) {
                Some(location) => MtpDevice::open_by_location(location).await.map_err(|error| friendly(&error))?,
                None if matches!(error, MtpError::NotFound) => return Err("Phone not found. Reconnect it and choose File transfer.".to_string()),
                None => return Err(friendly(&error)),
            },
        };
        let storages = device.storages().await.map_err(|error| friendly(&error))?;
        // A remembered storage can disappear (SD card removed), then the internal storage is used.
        let chosen = storage_id.and_then(|id| storages.iter().position(|storage| storage.id().0 == id && storage.info().is_writable));
        let index = chosen
            .or_else(|| storages.iter().position(|storage| storage.info().is_writable))
            .ok_or_else(|| "No writable storage on the phone. Unlock it and allow file access.".to_string())?;
        let storage = storages.into_iter().nth(index).expect("index comes from this list");
        Ok(Self { device, storage })
    }

    pub(crate) async fn close(self) {
        let _ = self.device.close().await;
    }

    /// Finds `path` folder by folder, ignoring case. Missing folders are created when `create` is set.
    pub(crate) async fn folder(&self, path: &str, create: bool) -> Result<Option<Option<ObjectHandle>>, String> {
        let mut parent = None;
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let existing = self
                .storage
                .list_objects(parent)
                .await
                .map_err(|error| friendly(&error))?
                .into_iter()
                .find(|object| object.is_folder() && object.filename.eq_ignore_ascii_case(part));
            parent = Some(match existing {
                Some(folder) => folder.handle,
                None if create => self.storage.create_folder(parent, part).await.map_err(|error| friendly(&error))?,
                None => return Ok(None),
            });
        }
        Ok(Some(parent))
    }
}

pub async fn list_storages(device_id: &str) -> Result<Vec<AndroidStorage>, String> {
    let _turn = PHONE.lock().await;
    let device = Phone::open(device_id, None).await?.device;
    let storages = device.storages().await.map_err(|error| friendly(&error));
    let _ = device.close().await;
    Ok(storages?
        .iter()
        .map(|storage| {
            let info = storage.info();
            AndroidStorage {
                id: storage.id().0,
                description: info.description.clone(),
                capacity: info.total_capacity,
                free: info.free_space,
                writable: info.is_writable,
            }
        })
        .collect())
}

pub async fn list_albums(device_id: &str, storage_id: Option<u64>) -> Result<Vec<AndroidAlbum>, String> {
    let _turn = PHONE.lock().await;
    let phone = Phone::open(device_id, storage_id).await?;
    let result = async {
        let Some(root) = phone.folder(ROOT_FOLDER, false).await? else { return Ok(Vec::new()) };
        let mut albums = Vec::new();
        for folder in phone.storage.list_objects(root).await.map_err(|error| friendly(&error))?.into_iter().filter(|object| object.is_folder()) {
            let files = phone
                .storage
                .list_objects(Some(folder.handle))
                .await
                .map(|objects| objects.iter().filter(|object| object.is_file()).count() as u32)
                .unwrap_or(0);
            albums.push(AndroidAlbum { name: folder.filename, files });
        }
        albums.sort_by_key(|album| album.name.to_lowercase());
        Ok(albums)
    }
    .await;
    phone.close().await;
    result
}

pub async fn create_album(device_id: &str, storage_id: Option<u64>, name: &str) -> Result<(), String> {
    let name = clean_album_name(name)?;
    let _turn = PHONE.lock().await;
    let phone = Phone::open(device_id, storage_id).await?;
    let result = phone.folder(&format!("{ROOT_FOLDER}/{name}"), true).await.map(|_| ());
    phone.close().await;
    result
}

pub async fn rename_album(device_id: &str, storage_id: Option<u64>, name: &str, new_name: &str) -> Result<(), String> {
    let new_name = clean_album_name(new_name)?;
    let _turn = PHONE.lock().await;
    let phone = Phone::open(device_id, storage_id).await?;
    let result = async {
        let root = phone.folder(ROOT_FOLDER, true).await?.flatten();
        let albums = phone.storage.list_objects(root).await.map_err(|error| friendly(&error))?;
        let album = albums
            .iter()
            .find(|object| object.is_folder() && object.filename.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("Album {name} not found."))?;
        let taken = albums.iter().any(|object| object.is_folder() && object.handle != album.handle && object.filename.eq_ignore_ascii_case(&new_name));
        if taken {
            return Err(format!("An album named {new_name} already exists."));
        }
        phone.storage.rename(album.handle, &new_name).await.map_err(|error| friendly(&error))
    }
    .await;
    phone.close().await;
    result
}

/// The name the user gave the phone in Android settings (MTP DeviceFriendlyName, 0xD402),
/// falling back to the MTP model (e.g. "SM-G998B"). Returns None when the USB interface is busy.
pub async fn display_name(device_id: &str) -> Option<String> {
    use mtp_rs::ptp::{unpack_string, DevicePropertyCode, PtpDevice};

    let _turn = PHONE.lock().await;
    let device = match PtpDevice::open_by_serial(device_id).await {
        Ok(device) => device,
        Err(_) => {
            let location = device_id.strip_prefix("mtp-")?.parse::<u64>().ok()?;
            PtpDevice::open_by_location(location).await.ok()?
        }
    };
    let model = device.get_device_info().await.ok().map(|info| info.model.trim().to_string());
    let session = device.open_session().await.ok()?;
    let data = session.get_device_prop_value(DevicePropertyCode::Unknown(0xD402)).await.ok();
    let _ = session.close().await;
    data.and_then(|data| unpack_string(&data).ok())
        .map(|(name, _)| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .or(model.filter(|model| !model.is_empty()))
}

/// Copies new photos and videos from `source` (subfolders included) into an album on the phone.
/// Files already on the phone with the same name and size are skipped and nothing on the phone is
/// deleted, except a partial upload this app left behind itself.
pub async fn send(request: SendRequest, state_dir: &Path, on_progress: impl Fn(AndroidProgress) + Send + Sync) -> Result<AndroidSendResult, String> {
    CANCEL_SEND.store(false, Ordering::SeqCst);
    let album = request.album.as_deref().filter(|album| !album.trim().is_empty()).map(clean_album_name).transpose()?;
    let folder = match &album {
        Some(album) => format!("{ROOT_FOLDER}/{album}"),
        None => ROOT_FOLDER.to_string(),
    };
    let source = PathBuf::from(&request.source);
    let (files, scan_issues) = tokio::task::spawn_blocking(move || scan_media(&source)).await.map_err(|error| error.to_string())?;
    let names = phone_names(&files);

    let mut result = AndroidSendResult { destination: folder.clone(), dry_run: request.dry_run, issues: scan_issues, ..AndroidSendResult::default() };
    let _turn = PHONE.lock().await;
    let phone = Phone::open(&request.device_id, request.storage_id).await?;
    let outcome = async {
        let parent = phone.folder(&folder, !request.dry_run).await?;
        let existing: HashMap<String, (ObjectHandle, u64)> = match parent {
            Some(parent) => phone
                .storage
                .list_objects(parent)
                .await
                .map_err(|error| friendly(&error))?
                .into_iter()
                .filter(|object| object.is_file())
                .map(|object| (object.filename.to_lowercase(), (object.handle, object.size)))
                .collect(),
            None => HashMap::new(),
        };

        let pending_key = |name: &str| format!("{}|{}|{folder}|{}", request.device_id, phone.storage.id().0, name.to_lowercase());
        let mut pending = PendingUploads::load(state_dir);
        let mut uploads = Vec::new();
        for (file, name) in files.iter().zip(&names) {
            match existing.get(&name.to_lowercase()) {
                Some((_, size)) if *size == file.size => result.skipped += 1,
                // A partial copy from an earlier run that was interrupted: replace it.
                Some((handle, _)) if pending.contains(&pending_key(name)) => uploads.push((file, name, Some(*handle))),
                Some(_) => {
                    result.conflicts += 1;
                    result.issues.push(AndroidSendIssue {
                        file: file.relative.clone(),
                        message: format!("A different file named {name} is already in this album. It was left alone."),
                    });
                }
                None => uploads.push((file, name, None)),
            }
        }

        let total = uploads.len() as u32;
        let total_bytes: u64 = uploads.iter().map(|(file, _, _)| file.size).sum();
        if request.dry_run {
            result.sent = total;
            result.bytes = total_bytes;
            return Ok(());
        }
        let Some(parent) = parent else { return Ok(()) };

        let mut last_emit = Instant::now() - PROGRESS_INTERVAL;
        for (index, (file, name, stale)) in uploads.into_iter().enumerate() {
            if CANCEL_SEND.load(Ordering::SeqCst) {
                result.cancelled = true;
                break;
            }
            on_progress(AndroidProgress { file: file.relative.clone(), done: index as u32, total, bytes: result.bytes, total_bytes });
            if let Some(handle) = stale
                && let Err(error) = phone.storage.delete(handle).await {
                    result.failed += 1;
                    result.issues.push(AndroidSendIssue { file: file.relative.clone(), message: format!("Could not replace the unfinished copy: {}", friendly(&error)) });
                    continue;
                }

            let key = pending_key(name);
            pending.insert(&key);
            let sent_before = result.bytes;
            let upload = async {
                let handle = tokio::fs::File::open(&file.path).await.map_err(|error| UploadError { source: MtpError::Io { message: format!("Could not read the file: {error}") }, partial: None })?;
                let stream = ReaderStream::with_capacity(handle, UPLOAD_CHUNK);
                let mut info = NewObjectInfo::file(name.clone(), file.size);
                if let Some(modified) = DateTime::from_unix_seconds(file.modified_seconds, UtcOffset::UTC) {
                    info = info.with_modified(modified);
                }
                phone
                    .storage
                    .upload_with_progress(parent, info, stream, |progress| {
                        if last_emit.elapsed() >= PROGRESS_INTERVAL {
                            last_emit = Instant::now();
                            on_progress(AndroidProgress { file: file.relative.clone(), done: index as u32, total, bytes: sent_before + progress.bytes_transferred, total_bytes });
                        }
                        if CANCEL_SEND.load(Ordering::SeqCst) { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
                    })
                    .await
            }
            .await;

            match upload {
                Ok(_) => {
                    pending.remove(&key);
                    result.sent += 1;
                    result.bytes += file.size;
                }
                Err(UploadError { source: error, partial }) => {
                    let cleaned = match partial {
                        Some(handle) => phone.storage.delete(handle).await.is_ok(),
                        None => true,
                    };
                    if cleaned { pending.remove(&key); }
                    if matches!(error, MtpError::Cancelled) {
                        result.cancelled = true;
                        break;
                    }
                    result.failed += 1;
                    result.issues.push(AndroidSendIssue { file: file.relative.clone(), message: friendly(&error) });
                    if error.is_disconnected() {
                        break;
                    }
                }
            }
        }
        on_progress(AndroidProgress { file: String::new(), done: total, total, bytes: result.bytes, total_bytes });
        Ok::<(), String>(())
    }
    .await;
    phone.close().await;
    outcome.map(|()| result)
}

#[derive(Debug)]
struct SourceMedia {
    path: PathBuf,
    relative: String,
    size: u64,
    modified_seconds: i64,
}

/// Lists photos and videos in `root`, sorted by relative path. Unreadable folders are reported, not fatal. Symlinks are not followed.
fn scan_media(root: &Path) -> (Vec<SourceMedia>, Vec<AndroidSendIssue>) {
    let mut files = Vec::new();
    let mut issues = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                issues.push(AndroidSendIssue { file: directory.display().to_string(), message: format!("Folder could not be read: {error}") });
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let Ok(file_type) = entry.file_type() else { continue };
            if hidden || file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() && is_media(&path) {
                let Ok(metadata) = entry.metadata() else { continue };
                let modified_seconds = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |duration| duration.as_secs() as i64);
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                files.push(SourceMedia { path, relative, size: metadata.len(), modified_seconds });
            }
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    (files, issues)
}

fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| MEDIA_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Albums are flat on the phone. When two source files share a name, both get their folder
/// added ("IMG_0001 (2023 Italy).jpg") so neither overwrites or hides the other.
fn phone_names(files: &[SourceMedia]) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for file in files {
        *counts.entry(base_name(&file.relative).to_lowercase()).or_default() += 1;
    }
    files
        .iter()
        .map(|file| {
            let name = base_name(&file.relative);
            let parent = file.relative.rsplit_once('/').map(|(parent, _)| parent.replace('/', " "));
            match parent {
                Some(parent) if counts[&name.to_lowercase()] > 1 => match name.rsplit_once('.') {
                    Some((stem, extension)) => format!("{stem} ({parent}).{extension}"),
                    None => format!("{name} ({parent})"),
                },
                _ => name.to_string(),
            }
        })
        .collect()
}

fn base_name(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

/// What the "Send to phone" button remembers, so a send is one click.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PhoneSettings {
    pub source: Option<String>,
    pub album: Option<String>,
    pub storage_id: Option<u64>,
}

#[derive(Default, Deserialize, Serialize)]
struct PhoneSettingsFile {
    /// Used for a phone that has never been set up, so a new or reset phone works right away.
    last: PhoneSettings,
    devices: HashMap<String, PhoneSettings>,
}

fn settings_path(state_dir: &Path) -> PathBuf {
    state_dir.join("android-settings.json")
}

fn read_settings_file(state_dir: &Path) -> PhoneSettingsFile {
    std::fs::read_to_string(settings_path(state_dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn load_settings(state_dir: &Path, device_id: &str) -> PhoneSettings {
    let mut file = read_settings_file(state_dir);
    file.devices.remove(device_id).unwrap_or(file.last)
}

pub fn save_settings(state_dir: &Path, device_id: &str, settings: PhoneSettings) -> Result<(), String> {
    let mut file = read_settings_file(state_dir);
    file.last = settings.clone();
    file.devices.insert(device_id.to_string(), settings);
    std::fs::create_dir_all(state_dir).map_err(|error| error.to_string())?;
    let temporary = settings_path(state_dir).with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_string_pretty(&file).map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
    std::fs::rename(temporary, settings_path(state_dir)).map_err(|error| error.to_string())
}

/// Album names become folder names on the phone, so keep them to one safe path segment.
fn clean_album_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let invalid = name.is_empty()
        || name.len() > 100
        || name.starts_with('.')
        || name.chars().any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control());
    if invalid { Err("Use a simple album name without slashes or special characters.".to_string()) } else { Ok(name.to_string()) }
}

/// Uploads that started but did not finish. A crash or unplugged cable leaves a truncated file on
/// the phone. Only files listed here may be replaced, so the user's own files are never deleted.
struct PendingUploads {
    path: PathBuf,
    keys: HashSet<String>,
}

impl PendingUploads {
    fn load(state_dir: &Path) -> Self {
        let path = state_dir.join("android-pending-uploads.json");
        let keys = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Self { path, keys }
    }

    fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    fn insert(&mut self, key: &str) {
        if self.keys.insert(key.to_string()) { self.save(); }
    }

    fn remove(&mut self, key: &str) {
        if self.keys.remove(key) { self.save(); }
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string(&self.keys) {
            let _ = std::fs::write(&self.path, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(relative: &str) -> SourceMedia {
        SourceMedia { path: PathBuf::from(relative), relative: relative.to_string(), size: 1, modified_seconds: 0 }
    }

    #[test]
    fn duplicate_names_from_different_folders_get_their_folder_added() {
        let files = vec![media("2023/Italy/IMG_0001.JPG"), media("2024/img_0001.jpg"), media("2024/IMG_0002.jpg"), media("cover.png")];
        assert_eq!(phone_names(&files), vec!["IMG_0001 (2023 Italy).JPG", "img_0001 (2024).jpg", "IMG_0002.jpg", "cover.png"]);
    }

    #[test]
    fn scan_skips_hidden_files_symlinks_and_non_media_and_reports_unreadable_folders() {
        let root = std::env::temp_dir().join(format!("lbs-android-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("b/clip.MOV"), b"v").unwrap();
        std::fs::write(root.join("a.jpg"), b"p").unwrap();
        std::fs::write(root.join("notes.txt"), b"t").unwrap();
        std::fs::write(root.join("._a.jpg"), b"").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("b/loop")).unwrap();

        let (files, issues) = scan_media(&root);
        let names: Vec<&str> = files.iter().map(|file| file.relative.as_str()).collect();
        assert_eq!(names, vec!["a.jpg", "b/clip.MOV"]);
        assert!(issues.is_empty());

        let (files, issues) = scan_media(&root.join("missing"));
        assert!(files.is_empty());
        assert_eq!(issues.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_uploads_survive_a_restart() {
        let directory = std::env::temp_dir().join(format!("lbs-android-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let mut pending = PendingUploads::load(&directory);
        pending.insert("phone|1|DCIM/Fotografie|clip.mov");
        assert!(PendingUploads::load(&directory).contains("phone|1|DCIM/Fotografie|clip.mov"));
        pending.remove("phone|1|DCIM/Fotografie|clip.mov");
        assert!(!PendingUploads::load(&directory).contains("phone|1|DCIM/Fotografie|clip.mov"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn settings_are_remembered_per_phone_with_the_last_used_as_default() {
        let directory = std::env::temp_dir().join(format!("lbs-android-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(load_settings(&directory, "pixel"), PhoneSettings::default());

        let italy = PhoneSettings { source: Some("/Volumes/T7/Exports".to_string()), album: Some("Italy".to_string()), storage_id: Some(65537) };
        save_settings(&directory, "pixel", italy.clone()).unwrap();
        let home = PhoneSettings { album: Some("Home".to_string()), ..italy.clone() };
        save_settings(&directory, "galaxy", home.clone()).unwrap();

        assert_eq!(load_settings(&directory, "pixel"), italy);
        assert_eq!(load_settings(&directory, "new-phone"), home);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn album_names_stay_one_safe_folder() {
        assert_eq!(clean_album_name("  Italy 2024 ").unwrap(), "Italy 2024");
        assert!(clean_album_name("../DCIM").is_err());
        assert!(clean_album_name(".hidden").is_err());
    }
}
