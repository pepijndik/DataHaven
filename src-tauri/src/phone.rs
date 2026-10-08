//! Phones as a source or destination of the backup engine. The engine is synchronous, mtp-rs is
//! async, so every call is driven to completion on the current (blocking) thread.

use crate::android::{friendly, Phone, PHONE};
use crate::backup::RunControl;
use crate::transfer::CopyError;
use mtp_rs::mtp::{ByteRange, DateTime, Error as MtpError, NewObjectInfo, UploadError, UtcOffset};
use mtp_rs::ObjectHandle;
use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio_util::io::ReaderStream;

const UPLOAD_CHUNK: usize = 1024 * 1024;

/// How the manifest names a file on a phone. Storage ids are left out because they can change between connections.
pub fn uri(device_id: &str, folder: &str, relative: &str) -> String {
    let folder = folder.trim_matches('/');
    match (folder.is_empty(), relative.is_empty()) {
        (true, _) => format!("android://{device_id}/{relative}"),
        (false, true) => format!("android://{device_id}/{folder}"),
        (false, false) => format!("android://{device_id}/{folder}/{relative}"),
    }
}

fn block<F: Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

pub struct PhoneFile {
    pub handle: u64,
    /// Path inside the listed folder, with `/` separators.
    pub relative: String,
    pub size: u64,
    pub modified_ns: i64,
}

/// The phones used by one run. Holding the shared phone lock keeps the UI from opening the same USB connection meanwhile.
pub struct Phones {
    sessions: HashMap<String, PhoneSession>,
    /// The phone picked for jobs that use "whichever phone is connected".
    any_phone: Option<String>,
    turn: Option<tokio::sync::MutexGuard<'static, ()>>,
}

impl Phones {
    pub fn new() -> Self {
        Self { sessions: HashMap::new(), any_phone: None, turn: None }
    }

    /// Opens a phone once per run. An empty `device_id` means the first connected phone, so a job keeps working with a new phone.
    pub fn session(&mut self, device_id: &str, storage_id: Option<u64>) -> Result<&PhoneSession, String> {
        let device_id = if device_id.trim().is_empty() {
            match &self.any_phone {
                Some(id) => id.clone(),
                None => {
                    let id = crate::android::detect()?
                        .into_iter()
                        .next()
                        .map(|device| device.id)
                        .ok_or("No phone is connected. Plug it in, unlock it and choose File transfer.")?;
                    self.any_phone = Some(id.clone());
                    id
                }
            }
        } else {
            device_id.to_string()
        };
        let key = format!("{device_id}|{storage_id:?}");
        if !self.sessions.contains_key(&key) {
            if self.turn.is_none() {
                self.turn = Some(PHONE.blocking_lock());
            }
            let phone = block(Phone::open(&device_id, storage_id))?;
            self.sessions.insert(key.clone(), PhoneSession { device_id, phone: Some(phone), folders: Mutex::new(HashMap::new()) });
        }
        Ok(&self.sessions[&key])
    }
}

impl Drop for Phones {
    fn drop(&mut self) {
        for (_, mut session) in self.sessions.drain() {
            if let Some(phone) = session.phone.take() {
                block(phone.close());
            }
        }
    }
}

pub struct PhoneSession {
    pub device_id: String,
    phone: Option<Phone>,
    /// Folder handles by path, so uploads do not list the same folders again.
    folders: Mutex<HashMap<String, Option<ObjectHandle>>>,
}

impl PhoneSession {
    fn phone(&self) -> &Phone {
        self.phone.as_ref().expect("phone is open until the session is dropped")
    }

    /// Every file below `folder`. `None` when the folder does not exist on the phone.
    pub fn list_files(&self, folder: &str) -> Result<Option<Vec<PhoneFile>>, String> {
        let phone = self.phone();
        block(async {
            let Some(root) = phone.folder(folder, false).await? else { return Ok(None) };
            let objects = phone.storage.list_objects_recursive(root).await.map_err(|error| friendly(&error))?;
            let names: HashMap<ObjectHandle, (&str, ObjectHandle)> = objects.iter().map(|object| (object.handle, (object.filename.as_str(), object.parent))).collect();
            let local_offset = local_utc_offset();

            let files = objects
                .iter()
                .filter(|object| object.is_file())
                .map(|object| {
                    let mut parts = vec![object.filename.as_str()];
                    let mut parent = object.parent;
                    while Some(parent) != root {
                        let Some((name, next)) = names.get(&parent) else { break };
                        parts.push(name);
                        parent = *next;
                    }
                    parts.reverse();
                    let modified_seconds = object
                        .modified
                        .or(object.created)
                        .and_then(|date| date.to_unix_seconds_with_fallback(local_offset))
                        .unwrap_or_default();
                    PhoneFile { handle: object.handle.0, relative: parts.join("/"), size: object.size, modified_ns: modified_seconds * 1_000_000_000 }
                })
                .collect();
            Ok(Some(files))
        })
    }

    /// Streams a phone file into `partial`. Pausing waits between files, because holding the phone mid-transfer can time out.
    pub fn download(&self, handle: u64, partial: &Path, hash: bool, control: &RunControl, copied_bytes: &AtomicU64) -> Result<Option<String>, CopyError> {
        let storage = &self.phone().storage;
        block(async {
            let mut download = storage.download(ObjectHandle(handle), ByteRange::Full).await.map_err(|error| CopyError::Failed(friendly(&error)))?;
            let mut output = std::fs::File::create(partial)?;
            let mut hasher = hash.then(blake3::Hasher::new);
            while let Some(chunk) = download.next_chunk().await {
                if control.is_cancelled() {
                    let _ = download.cancel(Duration::from_secs(2)).await;
                    return Err(CopyError::Cancelled);
                }
                let chunk = chunk.map_err(|error| CopyError::Failed(friendly(&error)))?;
                output.write_all(&chunk)?;
                if let Some(hasher) = hasher.as_mut() { hasher.update(&chunk); }
                copied_bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            output.flush()?;
            Ok(hasher.map(|hasher| hasher.finalize().to_hex().to_string()))
        })
    }

    /// Uploads `source` as `folder/name`, creating folders as needed. `replace` is deleted first:
    /// phones cannot overwrite in place and only files inside the backup folder are ever replaced.
    #[allow(clippy::too_many_arguments)]
    pub fn upload(&self, folder: &str, name: &str, source: &Path, size: u64, modified_ns: i64, replace: Option<u64>, control: &RunControl, copied_bytes: &AtomicU64) -> Result<(), CopyError> {
        let phone = self.phone();
        block(async {
            let parent = self.folder_handle(folder).await.map_err(CopyError::Failed)?;
            if let Some(handle) = replace {
                phone.storage.delete(ObjectHandle(handle)).await.map_err(|error| CopyError::Failed(friendly(&error)))?;
            }
            let file = tokio::fs::File::open(source).await?;
            let mut info = NewObjectInfo::file(name, size);
            if let Some(modified) = DateTime::from_unix_seconds(modified_ns.div_euclid(1_000_000_000), UtcOffset::UTC) {
                info = info.with_modified(modified);
            }
            let mut reported = 0;
            let result = phone
                .storage
                .upload_with_progress(parent, info, ReaderStream::with_capacity(file, UPLOAD_CHUNK), |progress| {
                    copied_bytes.fetch_add(progress.bytes_transferred.saturating_sub(reported), Ordering::Relaxed);
                    reported = progress.bytes_transferred;
                    if control.is_cancelled() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
                })
                .await;
            match result {
                Ok(_) => Ok(()),
                Err(UploadError { source, partial }) => {
                    if let Some(handle) = partial {
                        let _ = phone.storage.delete(handle).await;
                    }
                    if matches!(source, MtpError::Cancelled) { Err(CopyError::Cancelled) } else { Err(CopyError::Failed(friendly(&source))) }
                }
            }
        })
    }

    async fn folder_handle(&self, folder: &str) -> Result<Option<ObjectHandle>, String> {
        if let Some(handle) = self.folders.lock().ok().and_then(|folders| folders.get(folder).copied()) {
            return Ok(handle);
        }
        let handle = self.phone().folder(folder, true).await?.flatten();
        if let Ok(mut folders) = self.folders.lock() {
            folders.insert(folder.to_string(), handle);
        }
        Ok(handle)
    }
}

/// Android reports times without a zone, in the phone's local time. The Mac is almost always in the same zone.
fn local_utc_offset() -> UtcOffset {
    static OFFSET: OnceLock<UtcOffset> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        let output = std::process::Command::new("date").arg("+%z").output().ok();
        let text = output.map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string()).unwrap_or_default();
        parse_offset(&text).and_then(UtcOffset::from_minutes).unwrap_or(UtcOffset::UTC)
    })
}

fn parse_offset(text: &str) -> Option<i16> {
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let hours: i16 = digits[..2].parse().ok()?;
    let minutes: i16 = digits[2..].parse().ok()?;
    Some(sign * (hours * 60 + minutes))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use mtp_rs::{VirtualDeviceConfig, VirtualStorageConfig};
    use std::path::PathBuf;

    /// A fake phone backed by a folder, visible to `MtpDevice::list_devices` and `open_by_serial`.
    pub fn virtual_phone(serial: &str, backing_dir: PathBuf) -> u64 {
        std::fs::create_dir_all(&backing_dir).unwrap();
        let config = VirtualDeviceConfig {
            serial: serial.to_string(),
            storages: vec![VirtualStorageConfig { description: "Internal Storage".into(), capacity: 1 << 30, backing_dir, read_only: false }],
            event_poll_interval: Duration::ZERO,
            watch_backing_dirs: false,
            // Like a real phone: zoneless dates in the local time zone.
            utc_offset: local_utc_offset(),
            ..Default::default()
        };
        mtp_rs::register_virtual_device(&config).location_id
    }

    /// The virtual phone indexes its folder when opened. Tests that change files behind its back re-index.
    pub fn rescan(serial: &str) {
        let _ = mtp_rs::rescan_virtual_device(serial);
    }

    #[test]
    fn parses_utc_offsets() {
        assert_eq!(parse_offset("+0200"), Some(120));
        assert_eq!(parse_offset("-0530"), Some(-330));
        assert_eq!(parse_offset("garbage"), None);
    }

    #[test]
    fn uris_name_phone_files_stably() {
        assert_eq!(uri("pixel", "/DCIM/Camera/", "2024/a.jpg"), "android://pixel/DCIM/Camera/2024/a.jpg");
        assert_eq!(uri("pixel", "DCIM/Camera", ""), "android://pixel/DCIM/Camera");
    }

    #[test]
    fn lists_downloads_and_uploads_on_a_virtual_phone() {
        let root = std::env::temp_dir().join(format!("lbs-phone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("phone/DCIM/Camera/2024")).unwrap();
        std::fs::write(root.join("phone/DCIM/Camera/2024/IMG_1.jpg"), b"jpeg-bytes").unwrap();
        let location = virtual_phone("phone-session-test", root.join("phone"));

        let mut phones = Phones::new();
        let session = phones.session("phone-session-test", None).unwrap();
        let files = session.list_files("DCIM/Camera").unwrap().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!((files[0].relative.as_str(), files[0].size), ("2024/IMG_1.jpg", 10));
        assert!(session.list_files("DCIM/Missing").unwrap().is_none());

        let control = RunControl::new();
        let bytes = AtomicU64::new(0);
        let partial = root.join("download.part");
        let hash = session.download(files[0].handle, &partial, true, &control, &bytes).ok().unwrap();
        assert_eq!(std::fs::read(&partial).unwrap(), b"jpeg-bytes");
        assert_eq!(hash.unwrap(), blake3::hash(b"jpeg-bytes").to_hex().to_string());

        session.upload("Backup/photos/2024", "IMG_1.jpg", &partial, 10, 0, None, &control, &bytes).ok().unwrap();
        assert_eq!(std::fs::read(root.join("phone/Backup/photos/2024/IMG_1.jpg")).unwrap(), b"jpeg-bytes");

        drop(phones);
        mtp_rs::unregister_virtual_device(location);
        std::fs::remove_dir_all(root).unwrap();
    }
}
