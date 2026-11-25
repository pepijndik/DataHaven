use serde::Serialize;
use std::process::Command;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MountedVolume {
    pub name: String,
    pub path: String,
    pub filesystem: String,
    pub capacity: Option<u64>,
    pub available: Option<u64>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeHealth {
    pub path: String,
    pub level: String,
    pub status: String,
    pub files_checked: u64,
    pub bytes_read: u64,
    pub errors: Vec<String>,
    pub duration_ms: u128,
    pub note: String,
}

/// Drives and network shares the user can back up to: everything mounted under `/Volumes` on macOS,
/// under `/media`, `/run/media` and `/mnt` on Linux, and every drive letter except the system drive
/// on Windows. The startup disk is left out.
pub fn detect() -> Vec<MountedVolume> {
    let mut volumes: Vec<MountedVolume> = mount_points().into_iter().filter_map(|(path, filesystem)| volume_at(&path, filesystem)).collect();
    volumes.sort_by_key(|volume| volume.name.to_lowercase());
    volumes
}

#[cfg(target_os = "macos")]
fn mount_points() -> Vec<(std::path::PathBuf, Option<String>)> {
    let Ok(entries) = fs::read_dir("/Volumes") else { return Vec::new() };
    entries
        .flatten()
        // "Macintosh HD" is a symlink to /, and hidden entries are system mounts such as Time Machine snapshots.
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()) && !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| (entry.path(), None))
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn mount_points() -> Vec<(std::path::PathBuf, Option<String>)> {
    let Ok(mounts) = fs::read_to_string("/proc/self/mounts") else { return Vec::new() };
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (_, mount, filesystem) = (fields.next()?, fields.next()?, fields.next()?);
            // /proc/mounts escapes spaces as \040.
            let mount = mount.replace("\\040", " ");
            ["/media/", "/run/media/", "/mnt/"].iter().any(|prefix| mount.starts_with(prefix)).then(|| (std::path::PathBuf::from(mount), Some(filesystem.to_string())))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn volume_at(path: &Path, _filesystem: Option<String>) -> Option<MountedVolume> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid C string and a writable, correctly sized statfs.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    // SAFETY: the kernel fills these with NUL terminated strings.
    let mounted_on = unsafe { std::ffi::CStr::from_ptr(stats.f_mntonname.as_ptr()) }.to_string_lossy().into_owned();
    // A plain folder left behind in /Volumes reports the startup disk as its mount. Only real mount points count.
    if Path::new(&mounted_on) != path {
        return None;
    }
    // System volumes such as Recovery are mounted "nobrowse": Finder and Disk Utility hide them, so we do too.
    if hidden_by_system(stats.f_flags) {
        return None;
    }
    let filesystem = unsafe { std::ffi::CStr::from_ptr(stats.f_fstypename.as_ptr()) }.to_string_lossy().into_owned();
    let block = u64::from(stats.f_bsize);
    Some(MountedVolume {
        name: path.file_name()?.to_string_lossy().into_owned(),
        path: path.to_string_lossy().into_owned(),
        filesystem,
        capacity: Some(stats.f_blocks * block),
        available: Some(stats.f_bavail * block),
    })
}

/// The `MNT_DONTBROWSE` mount flag, set on system volumes (Recovery, Preboot, VM, Update) that macOS hides.
#[cfg(target_os = "macos")]
const MNT_DONTBROWSE: u32 = 0x0010_0000;

#[cfg(target_os = "macos")]
fn hidden_by_system(flags: u32) -> bool {
    flags & MNT_DONTBROWSE != 0
}

#[cfg(all(unix, not(target_os = "macos")))]
fn volume_at(path: &Path, filesystem: Option<String>) -> Option<MountedVolume> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid C string and a writable, correctly sized statvfs.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block = stats.f_frsize as u64;
    Some(MountedVolume {
        name: path.file_name()?.to_string_lossy().into_owned(),
        path: path.to_string_lossy().into_owned(),
        filesystem: filesystem.unwrap_or_default(),
        capacity: Some(stats.f_blocks as u64 * block),
        available: Some(stats.f_bavail as u64 * block),
    })
}

/// The system drive (`C:`), which plays the part of the startup disk on Windows.
#[cfg(windows)]
fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string()).to_uppercase()
}

#[cfg(windows)]
fn mount_points() -> Vec<(std::path::PathBuf, Option<String>)> {
    // SAFETY: GetLogicalDrives takes no arguments and returns a bit per drive letter.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    let system = system_drive();
    (0..26u8)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| format!("{}:", (b'A' + bit) as char))
        .filter(|letter| *letter != system && letter.as_str() != "A:" && letter.as_str() != "B:")
        .map(|letter| (std::path::PathBuf::from(format!("{letter}\\")), None))
        .collect()
}

#[cfg(windows)]
fn volume_at(path: &Path, _filesystem: Option<String>) -> Option<MountedVolume> {
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW};
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a NUL terminated UTF-16 path that outlives every call below.
    let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
    if !matches!(kind, DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_REMOTE) {
        return None; // CD drives, RAM disks and unknown devices
    }
    let mut label = [0u16; 261];
    let mut filesystem = [0u16; 261];
    // SAFETY: both buffers are writable and their sizes are passed along. Fails for an empty card reader.
    let ok = unsafe { GetVolumeInformationW(wide.as_ptr(), label.as_mut_ptr(), label.len() as u32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), filesystem.as_mut_ptr(), filesystem.len() as u32) };
    if ok == 0 {
        return None;
    }
    let text = |buffer: &[u16]| String::from_utf16_lossy(&buffer[..buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len())]);
    let letter = path.to_string_lossy().trim_end_matches('\\').to_string();
    let label = text(&label);
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: the output pointers are valid u64s; the last one may be null.
    let sized = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } != 0;
    Some(MountedVolume {
        name: if label.is_empty() { format!("Drive {letter}") } else { format!("{label} ({letter})") },
        path: path.to_string_lossy().into_owned(),
        filesystem: text(&filesystem).to_lowercase(),
        capacity: sized.then_some(total),
        available: sized.then_some(free),
    })
}

/// Paths compared the way the system does: either slash, and without case on Windows.
fn normalized(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.trim_end_matches('/');
    if cfg!(windows) { path.to_lowercase() } else { path.to_string() }
}

/// Whether `path` is the volume at `volume_path` or inside it.
pub fn volume_contains(volume_path: &str, path: &str) -> bool {
    if volume_path.is_empty() || path.is_empty() {
        return false;
    }
    let volume = normalized(volume_path);
    let path = normalized(path);
    path == volume || path.starts_with(&format!("{volume}/"))
}

/// The name of the external drive (or network share) a path is on, connected or not.
/// `None` for paths on this computer's own disk.
pub fn external_drive_name(path: &str) -> Option<String> {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            let letter = path[..2].to_uppercase();
            #[cfg(windows)]
            if letter == system_drive() {
                return None;
            }
            return Some(format!("Drive {letter}"));
        }
        return path.starts_with("\\\\").then(|| path.trim_start_matches('\\').split('\\').next().unwrap_or("Network").to_string());
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        ["Volumes", name, ..] => Some((*name).to_string()),
        ["media", _, name, ..] | ["run", "media", _, name, ..] | ["mnt", name, ..] => Some((*name).to_string()),
        _ => None,
    }
}

/// The last part of a path, with either kind of slash.
pub fn last_component(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

pub fn health(path: String, level: String) -> VolumeHealth {
    let started = Instant::now();
    let mut files_checked = 0;
    let mut bytes_read = 0;
    let mut errors = Vec::new();
    let root = std::path::PathBuf::from(&path);

    if !root.is_dir() {
        return VolumeHealth { path, level, status: "failed".to_string(), files_checked, bytes_read, errors: vec!["Volume is not mounted or is not a directory".to_string()], duration_ms: started.elapsed().as_millis(), note: "Reconnect the drive and try again.".to_string() };
    }

    if level == "quick" {
        if let Err(error) = fs::read_dir(&root) { errors.push(error.to_string()); }
    } else {
        inspect_directory(&root, &level, &mut files_checked, &mut bytes_read, &mut errors);
    }

    let status = if errors.is_empty() { "healthy" } else { "warning" };
    let note = match level.as_str() {
        "quick" => "Mount and directory access checked. This does not test every file.",
        "medium" => "A sample from each file was read. Use Full for a complete read test.",
        _ => "Every readable file was read without writing to the drive.",
    };
    VolumeHealth { path, level, status: status.to_string(), files_checked, bytes_read, errors, duration_ms: started.elapsed().as_millis(), note: note.to_string() }
}

fn inspect_directory(path: &std::path::Path, level: &str, files_checked: &mut u64, bytes_read: &mut u64, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(path) { Ok(entries) => entries, Err(error) => { errors.push(format!("{}: {}", path.display(), error)); return; } };
    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() { inspect_directory(&child, level, files_checked, bytes_read, errors); continue; }
        if !file_type.is_file() { continue; }
        *files_checked += 1;
        let result = File::open(&child).and_then(|mut file| {
            let mut buffer = vec![0_u8; 1024 * 1024];
            let mut total = 0_u64;
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 { break; }
                total += read as u64;
                if level == "medium" { break; }
            }
            Ok(total)
        });
        match result { Ok(read) => *bytes_read += read, Err(error) => errors.push(format!("{}: {}", child.display(), error)) }
    }
}

/// What the app knows about a destination folder, so the UI can offer the right options.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationInfo {
    pub filesystem: Option<String>,
    /// APFS: snapshots (clones) work here.
    pub snapshots_supported: bool,
    pub external: bool,
    pub volume_name: Option<String>,
    pub mount_point: Option<String>,
    /// The folder is the root of its volume (a volume made for one job, like Carbon Copy Cloner uses).
    pub is_volume_root: bool,
    /// Carbon Copy Cloner snapshots exist on this volume, so it holds a CCC backup.
    pub ccc_backup: bool,
    /// A separate APFS volume can be added next to this one.
    pub can_create_volume: bool,
}

pub fn inspect_destination(path: &str) -> DestinationInfo {
    let filesystem = crate::snapshots::filesystem(Path::new(path));
    let Some(info) = disk_info(path) else { return DestinationInfo { snapshots_supported: filesystem.as_deref() == Some("apfs"), filesystem, ..DestinationInfo::default() } };
    let text = |key: &str| info.get(key).and_then(|value| value.as_str()).map(ToString::to_string);
    let mount_point = text("MountPoint");
    let apfs = filesystem.as_deref() == Some("apfs");
    let external = info.get("Internal").and_then(|value| value.as_bool()) == Some(false);
    let ccc_backup = mount_point.as_deref().is_some_and(|mount| {
        Command::new("diskutil").args(["apfs", "listSnapshots", mount]).output().is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("com.bombich.ccc"))
    });
    DestinationInfo {
        snapshots_supported: apfs,
        external,
        volume_name: text("VolumeName"),
        is_volume_root: mount_point.as_deref().is_some_and(|mount| same_path(mount, path)),
        mount_point,
        ccc_backup,
        can_create_volume: apfs && external && text("APFSContainerReference").is_some(),
        filesystem,
    }
}

/// Adds an APFS volume for one job to the container that holds `path`, like Carbon Copy Cloner does.
/// Volumes share the free space of the container, so nothing is reserved. Returns the new mount point.
pub fn create_job_volume(path: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 60 || name.starts_with('.') || name.contains(['/', ':']) {
        return Err("Use a short volume name without slashes or colons.".to_string());
    }
    if Path::new("/Volumes").join(name).exists() {
        return Err(format!("A volume named {name} is already connected. Pick another name."));
    }
    let info = disk_info(path).ok_or("Could not read the drive. Is it still connected?")?;
    if info.get("FilesystemType").and_then(|value| value.as_str()) != Some("apfs") {
        return Err("This drive is not APFS, so it cannot hold extra volumes. Erasing it as APFS in Disk Utility would remove everything on it.".to_string());
    }
    if info.get("Internal").and_then(|value| value.as_bool()) != Some(false) {
        return Err("Separate volumes are only made on external drives.".to_string());
    }
    let container = info.get("APFSContainerReference").and_then(|value| value.as_str()).ok_or("The drive has no APFS container.")?;

    let output = Command::new("diskutil").args(["apfs", "addVolume", container, "APFS", name]).output().map_err(|error| format!("Could not start diskutil: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(text.lines().rev().find(|line| line.contains("Error")).unwrap_or("diskutil could not add the volume").trim().to_string());
    }
    let disk = text
        .lines()
        .find_map(|line| line.strip_prefix("Disk from APFS operation: "))
        .map(str::trim)
        .ok_or("The volume was made but could not be found.")?
        .to_string();

    let mount_point = |disk: &str| disk_info(disk).and_then(|info| info.get("MountPoint").and_then(|value| value.as_str()).filter(|mount| !mount.is_empty()).map(ToString::to_string));
    if let Some(mount) = mount_point(&disk) {
        return Ok(mount);
    }
    let _ = Command::new("diskutil").args(["mount", &disk]).status();
    mount_point(&disk).ok_or_else(|| format!("The volume {name} was made but did not mount. Mount it in Disk Utility."))
}

/// `diskutil info -plist` as JSON.
fn disk_info(target: &str) -> Option<serde_json::Value> {
    use std::io::Write;
    use std::process::Stdio;
    let plist = Command::new("diskutil").args(["info", "-plist", target]).output().ok().filter(|output| output.status.success())?.stdout;
    let mut plutil = Command::new("plutil").args(["-convert", "json", "-o", "-", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    plutil.stdin.take()?.write_all(&plist).ok()?;
    let output = plutil.wait_with_output().ok()?;
    serde_json::from_slice(&output.stdout).ok()
}

fn same_path(left: &str, right: &str) -> bool {
    let canonical = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| Path::new(path).to_path_buf());
    canonical(left) == canonical(right)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn detects_every_drive_mounted_under_volumes() {
        let detected = detect();
        let expected: Vec<String> = fs::read_dir("/Volumes")
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_type().unwrap().is_dir() && !entry.file_name().to_string_lossy().starts_with('.'))
            .filter(|entry| {
                // Hidden system volumes (Recovery and the like) are left out, as in Finder.
                let c_path = std::ffi::CString::new(entry.path().as_os_str().as_encoded_bytes()).unwrap();
                let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
                (unsafe { libc::statfs(c_path.as_ptr(), &mut stats) } == 0) && !hidden_by_system(stats.f_flags)
            })
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(detected.len(), expected.len(), "{detected:?}");
        assert!(detected.iter().all(|volume| volume.capacity.unwrap_or(0) > 0 && !volume.filesystem.is_empty()));
        assert!(detected.iter().all(|volume| volume.path != "/"), "the startup disk is left out");
    }

    /// Makes a throwaway 2 GB sparse APFS disk image (it uses almost no real space), adds a job volume, and cleans up.
    /// Run with `cargo test -- --ignored job_volume` on a Mac.
    #[test]
    #[ignore]
    fn creates_a_job_volume_on_an_apfs_disk_image() {
        let root = std::env::temp_dir().join(format!("lbs-volume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let image = root.join("disk");
        let mount = root.join("mnt");
        assert!(Command::new("hdiutil").args(["create", "-size", "2g", "-type", "SPARSE", "-fs", "APFS", "-volname", "LBSTest"]).arg(&image).status().unwrap().success());
        assert!(Command::new("hdiutil").args(["attach", "-nobrowse", "-mountpoint"]).arg(&mount).arg(root.join("disk.sparseimage")).status().unwrap().success());

        let info = inspect_destination(&mount.to_string_lossy());
        assert!(info.snapshots_supported && info.is_volume_root && info.can_create_volume && !info.ccc_backup, "{info:?}");
        let name = format!("LBS Job {}", std::process::id());
        let created = create_job_volume(&mount.to_string_lossy(), &name);
        let _ = Command::new("hdiutil").args(["detach", "-force"]).arg(&mount).status();
        if let Ok(volume) = &created {
            let _ = Command::new("diskutil").args(["unmount", "force", volume]).status();
        }
        let _ = std::fs::remove_dir_all(&root);
        assert!(created.unwrap().ends_with(&name));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod hidden_volume_tests {
    use super::*;

    #[test]
    fn hidden_system_volumes_are_skipped() {
        // Flags of /Volumes/Recovery (local, journaled, nobrowse) and of a plugged in drive.
        assert!(hidden_by_system(0x0010_0000 | 0x0000_1000 | 0x0080_0000));
        assert!(!hidden_by_system(0x0000_1000 | 0x0000_0008));
        let names: Vec<String> = detect().into_iter().map(|volume| volume.name).collect();
        assert!(!names.iter().any(|name| name == "Recovery" || name == "Preboot"), "{names:?}");
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn paths_on_drives() {
        assert!(volume_contains("/Volumes/T7", "/Volumes/T7/Photos"));
        assert!(volume_contains("/Volumes/T7", "/Volumes/T7"));
        assert!(!volume_contains("/Volumes/T7", "/Volumes/T7 Shield"));
        assert_eq!(last_component("/Users/me/Pictures/"), "Pictures");
        assert_eq!(last_component("D:\\Photos\\2025"), "2025");
        if cfg!(windows) {
            assert!(volume_contains("D:\\", "d:\\Photos"));
            assert_eq!(external_drive_name("E:\\Backup").as_deref(), Some("Drive E:"));
        } else {
            assert_eq!(external_drive_name("/Volumes/T7 Shield/Lightroom").as_deref(), Some("T7 Shield"));
            assert_eq!(external_drive_name("/Users/me/Pictures"), None);
            assert_eq!(external_drive_name("/run/media/me/Card/DCIM").as_deref(), Some("Card"));
        }
    }
}
