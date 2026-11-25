//! "Send to iPhone": adds the new photos and videos of a folder to an album in the Mac Photos app.
//! An iPhone cannot take files into an album over the cable from another app, so the album is
//! filled on the Mac and reaches the iPhone through iCloud Photos (or a Finder sync).
//! Which files were sent is remembered per album, so each run only adds what is new.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const FILE: &str = "iphone-sync.json";
/// Files handed to Photos per import, so progress moves and one bad file only fails its batch.
const BATCH: usize = 20;
pub const DEFAULT_ALBUM: &str = "From DataHaven";

const MEDIA: [&str; 22] = [
    "jpg", "jpeg", "heic", "heif", "png", "gif", "tif", "tiff", "webp", "dng", "cr2", "cr3", "nef",
    "arw", "raf", "orf", "rw2", "mov", "mp4", "m4v", "avi", "3gp",
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    pub modified: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IphoneSettings {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub album: String,
    /// Album name → path inside the source → what the file looked like when it was sent.
    #[serde(default)]
    pub sent: BTreeMap<String, BTreeMap<String, Stamp>>,
}

impl IphoneSettings {
    pub fn album_name(&self) -> &str {
        if self.album.trim().is_empty() {
            DEFAULT_ALBUM
        } else {
            self.album.trim()
        }
    }
}

pub fn load(directory: &Path) -> IphoneSettings {
    std::fs::read(directory.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(directory: &Path, settings: &IphoneSettings) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let temporary = directory.join(format!("{FILE}.tmp"));
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, directory.join(FILE)).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IphoneProgress {
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub total_bytes: u64,
    pub file: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IphoneResult {
    pub dry_run: bool,
    pub sent: usize,
    pub skipped: usize,
    pub failed: usize,
    pub bytes: u64,
    pub cancelled: bool,
    pub album: String,
    pub errors: Vec<String>,
}

pub fn summary(result: &IphoneResult) -> String {
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let bytes = crate::style::format_bytes(result.bytes);
    if result.dry_run {
        return if result.sent == 0 {
            format!(
                "Nothing new. {} file{} already in {}.",
                result.skipped,
                plural(result.skipped),
                result.album
            )
        } else {
            format!(
                "{} new file{} to add ({bytes}), {} already in {}.",
                result.sent,
                plural(result.sent),
                result.skipped,
                result.album
            )
        };
    }
    let mut text = format!(
        "{}{} added to {} ({bytes}), {} already there",
        if result.cancelled { "Stopped. " } else { "" },
        result.sent,
        result.album,
        result.skipped
    );
    if result.failed > 0 {
        text.push_str(&format!(", {} failed", result.failed));
    }
    text.push('.');
    text
}

fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| MEDIA.contains(&extension.to_ascii_lowercase().as_str()))
}

/// Every photo and video under `root` (hidden files and folders skipped), sorted by path.
pub fn media_files(root: &Path) -> Vec<(PathBuf, String, Stamp)> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file() && is_media(&path) {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or(0);
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                found.push((
                    path,
                    relative,
                    Stamp {
                        size: metadata.len(),
                        modified,
                    },
                ));
            }
        }
    }
    found.sort_by(|a, b| a.1.cmp(&b.1));
    found
}

/// Files that are new or changed since they were last sent to `album`.
pub fn new_files(
    files: Vec<(PathBuf, String, Stamp)>,
    sent: Option<&BTreeMap<String, Stamp>>,
) -> (Vec<(PathBuf, String, Stamp)>, usize) {
    let total = files.len();
    let new: Vec<_> = files
        .into_iter()
        .filter(|(_, relative, stamp)| sent.and_then(|sent| sent.get(relative)) != Some(stamp))
        .collect();
    let skipped = total - new.len();
    (new, skipped)
}

/// The AppleScript that imports its arguments (album name, then file paths) into a Photos album,
/// making the album first when it does not exist yet. Paths travel as arguments, so no quoting is needed.
pub const IMPORT_SCRIPT: &str = r#"on run argv
    set albumName to item 1 of argv
    set theFiles to {}
    repeat with i from 2 to count of argv
        set end of theFiles to (POSIX file (item i of argv)) as alias
    end repeat
    with timeout of 3600 seconds
        tell application "Photos"
            if not (exists album albumName) then make new album named albumName
            import theFiles into album albumName skip check duplicates true
        end tell
    end timeout
end run"#;

fn import(album: &str, files: &[&Path]) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Sending to an iPhone needs a Mac with the Photos app.".to_string());
    }
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(IMPORT_SCRIPT)
        .arg(album)
        .args(files)
        .output()
        .map_err(|error| format!("Could not start Photos: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if text.contains("-1743") || text.contains("Not authorized") {
        return Err("DataHaven may not control Photos yet. Allow it in System Settings, Privacy & Security, Automation.".to_string());
    }
    Err(if text.is_empty() {
        "Photos could not import these files.".to_string()
    } else {
        text
    })
}

/// Finds what is new in the source and, unless `dry_run`, adds it to the album batch by batch.
/// Returns the result and the updated record of sent files.
pub fn send(
    mut settings: IphoneSettings,
    dry_run: bool,
    cancel: Arc<AtomicBool>,
    on_progress: impl Fn(IphoneProgress),
) -> (Result<IphoneResult, String>, IphoneSettings) {
    let Some(source) = settings.source.clone().filter(|source| !source.is_empty()) else {
        return (
            Err("Choose the folder with the photos first.".to_string()),
            settings,
        );
    };
    if !Path::new(&source).is_dir() {
        return (
            Err("That folder is not available. Is the drive connected?".to_string()),
            settings,
        );
    }
    let album = settings.album_name().to_string();
    let (new, skipped) = new_files(media_files(Path::new(&source)), settings.sent.get(&album));
    let total_bytes = new.iter().map(|(_, _, stamp)| stamp.size).sum();
    let mut result = IphoneResult {
        dry_run,
        skipped,
        album: album.clone(),
        ..Default::default()
    };
    if dry_run {
        result.sent = new.len();
        result.bytes = total_bytes;
        return (Ok(result), settings);
    }
    let mut progress = IphoneProgress {
        total: new.len(),
        total_bytes,
        ..Default::default()
    };
    for batch in new.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            result.cancelled = true;
            break;
        }
        progress.file = batch[0].1.clone();
        on_progress(progress.clone());
        let paths: Vec<&Path> = batch.iter().map(|(path, _, _)| path.as_path()).collect();
        match import(&album, &paths) {
            Ok(()) => {
                let record = settings.sent.entry(album.clone()).or_default();
                for (_, relative, stamp) in batch {
                    record.insert(relative.clone(), *stamp);
                    result.sent += 1;
                    result.bytes += stamp.size;
                }
            }
            Err(error) => {
                // Permission problems fail every batch the same way, so stop at the first one.
                let stop =
                    error.contains("may not control Photos") || error.contains("needs a Mac");
                result.failed += batch.len();
                result.errors.push(error);
                if stop {
                    break;
                }
            }
        }
        progress.done += batch.len();
        progress.bytes += batch.iter().map(|(_, _, stamp)| stamp.size).sum::<u64>();
        on_progress(progress.clone());
    }
    if result.sent == 0 && result.failed > 0 {
        let error = result.errors.first().cloned().unwrap_or_default();
        return (Err(error), settings);
    }
    (Ok(result), settings)
}

// ---- the panel ------------------------------------------------------------------------------

use crate::app::{App, Event, pick_folder};
use crate::style::{self, muted};
use eframe::egui::{self, RichText};

pub enum IphoneEvent {
    Progress(IphoneProgress),
    Done {
        dry_run: bool,
        result: Result<IphoneResult, String>,
        settings: IphoneSettings,
    },
}

#[derive(Default)]
pub struct IphonePanel {
    pub loaded: bool,
    pub settings: IphoneSettings,
    pub sending: bool,
    pub checking: bool,
    pub progress: Option<IphoneProgress>,
    pub result: Option<IphoneResult>,
    pub error: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub editing: bool,
}

pub fn handle(app: &mut App, event: IphoneEvent) {
    match event {
        IphoneEvent::Progress(progress) => app.iphone.progress = Some(progress),
        IphoneEvent::Done {
            dry_run,
            result,
            settings,
        } => {
            let panel = &mut app.iphone;
            panel.sending = false;
            panel.checking = false;
            panel.progress = None;
            if !dry_run {
                panel.settings = settings;
                if let Err(error) = save(&app.data_dir, &panel.settings) {
                    panel.error = Some(format!("Could not remember what was sent: {error}"));
                }
            }
            match result {
                Ok(result) => {
                    app.message = summary(&result);
                    if let Some(error) = result.errors.first() {
                        app.iphone.error = Some(error.clone());
                    }
                    app.iphone.result = Some(result);
                }
                Err(error) => {
                    app.message = error.clone();
                    app.iphone.error = Some(error);
                }
            }
        }
    }
}

fn ensure_loaded(app: &mut App) {
    if !app.iphone.loaded {
        app.iphone.loaded = true;
        app.iphone.settings = load(&app.data_dir);
        app.iphone.editing = app.iphone.settings.source.is_none();
    }
}

fn remember(app: &mut App) {
    app.iphone.result = None;
    if let Err(error) = save(&app.data_dir, &app.iphone.settings) {
        app.iphone.error = Some(format!("Could not save: {error}"));
    }
}

pub fn start(app: &mut App, dry_run: bool) {
    ensure_loaded(app);
    if app.iphone.sending || app.iphone.checking {
        return;
    }
    if dry_run {
        app.iphone.checking = true
    } else {
        app.iphone.sending = true
    }
    app.iphone.error = None;
    app.iphone.result = None;
    app.iphone.cancel = Arc::new(AtomicBool::new(false));
    let settings = app.iphone.settings.clone();
    let cancel = app.iphone.cancel.clone();
    let sender = app.sender.clone();
    let ctx = app.ctx.clone();
    app.spawn(move || {
        let on_progress = move |progress: IphoneProgress| {
            let _ = sender.send(Event::Iphone(IphoneEvent::Progress(progress)));
            ctx.request_repaint();
        };
        let (result, settings) = send(settings, dry_run, cancel, on_progress);
        Event::Iphone(IphoneEvent::Done {
            dry_run,
            result,
            settings,
        })
    });
}

/// One line for the sidebar.
pub fn status_line(app: &mut App) -> String {
    ensure_loaded(app);
    if app.iphone.sending {
        return "Adding to Photos…".to_string();
    }
    match &app.iphone.settings.source {
        Some(_) => format!("Send to {}", app.iphone.settings.album_name()),
        None => "Not set up yet".to_string(),
    }
}

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    ensure_loaded(app);
    style::surface().inner_margin(egui::Margin::same(24)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(style::eyebrow("iPhone, through the Photos app"));
        let album = app.iphone.settings.album_name().to_string();
        ui.label(style::title(&format!("Album \"{album}\"")));
        ui.label(muted("New photos are added to this album in Photos on this Mac. With iCloud Photos on, the album shows up on your iPhone by itself."));
        ui.add_space(10.0);

        if !cfg!(target_os = "macos") {
            ui.label(RichText::new("This needs a Mac with the Photos app.").color(style::SUN));
            return;
        }

        if app.iphone.editing {
            setup(app, ui);
        } else {
            let busy = app.iphone.sending || app.iphone.checking;
            let fraction = app.iphone.progress.as_ref().map(|progress| if progress.total_bytes > 0 { (progress.bytes as f32 / progress.total_bytes as f32).min(1.0) } else if progress.total > 0 { progress.done as f32 / progress.total as f32 } else { 0.0 }).unwrap_or(0.0);
            let label = if app.iphone.sending { format!("Adding to {album}… {}%", (fraction * 100.0).round()) } else { format!("Send to {album}") };
            if ui.add_enabled(!busy, style::primary_button(label).min_size(egui::vec2(ui.available_width(), 44.0))).clicked() {
                start(app, false);
            }
            ui.label(muted(format!("From {}", app.iphone.settings.source.clone().unwrap_or_default())));
            if app.iphone.sending {
                style::track(ui, fraction, 6.0);
                if let Some(progress) = &app.iphone.progress {
                    ui.label(muted(format!("{} of {}: {}", progress.done, progress.total, progress.file)));
                }
                if ui.add(style::text_button("Stop after this batch")).clicked() {
                    app.iphone.cancel.store(true, Ordering::Relaxed);
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!app.iphone.checking, style::text_button(if app.iphone.checking { "Checking…" } else { "What's new?" })).clicked() {
                        start(app, true);
                    }
                    if ui.add(style::text_button("Change folder or album")).clicked() {
                        app.iphone.editing = true;
                    }
                });
            }
        }
        if let Some(error) = &app.iphone.error {
            ui.label(RichText::new(error).size(12.0).color(style::CORAL));
        }
        if let Some(result) = &app.iphone.result {
            ui.label(muted(summary(result)));
        }
    });
}

fn setup(app: &mut App, ui: &mut egui::Ui) {
    ui.label(muted("1. Which folder has the photos?"));
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for volume in &app.volumes {
            if ui.add(style::ghost_button(&volume.name)).clicked() {
                picked = Some(Some(volume.path.clone()));
            }
        }
        if ui.add(style::ghost_button("This Mac")).clicked() {
            picked = Some(None);
        }
    });
    if let Some(start_at) = picked {
        let start_at = start_at.or_else(|| app.iphone.settings.source.clone());
        if let Some(folder) = pick_folder(start_at.as_deref()) {
            app.iphone.settings.source = Some(folder);
            remember(app);
        }
    }
    if let Some(source) = &app.iphone.settings.source {
        ui.label(
            RichText::new(source)
                .size(11.0)
                .monospace()
                .color(style::MUTED),
        );
    }
    ui.label(muted("2. Album name in Photos (made if it doesn't exist)"));
    let mut album = app.iphone.settings.album.clone();
    let response = ui.add(
        egui::TextEdit::singleline(&mut album)
            .hint_text(DEFAULT_ALBUM)
            .desired_width(280.0),
    );
    if album != app.iphone.settings.album {
        app.iphone.settings.album = album;
    }
    if response.lost_focus() {
        remember(app);
    }
    ui.add_space(6.0);
    if ui
        .add_enabled(
            app.iphone.settings.source.is_some(),
            style::primary_button("Done"),
        )
        .clicked()
    {
        app.iphone.editing = false;
        remember(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_new_or_changed_media() {
        let root = std::env::temp_dir().join(format!("lbs-iphone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("2025/.hidden")).unwrap();
        std::fs::write(root.join("2025/IMG_1.HEIC"), b"one").unwrap();
        std::fs::write(root.join("2025/clip.mov"), b"video").unwrap();
        std::fs::write(root.join("2025/notes.txt"), b"skip").unwrap();
        std::fs::write(root.join("2025/.hidden/IMG_2.jpg"), b"skip").unwrap();
        let files = media_files(&root);
        let names: Vec<&str> = files
            .iter()
            .map(|(_, relative, _)| relative.as_str())
            .collect();
        assert_eq!(names, ["2025/IMG_1.HEIC", "2025/clip.mov"]);

        let mut sent = BTreeMap::new();
        sent.insert("2025/IMG_1.HEIC".to_string(), files[0].2);
        sent.insert(
            "2025/clip.mov".to_string(),
            Stamp {
                size: 1,
                modified: 0,
            },
        );
        let (new, skipped) = new_files(files, Some(&sent));
        assert_eq!(skipped, 1);
        assert_eq!(new.len(), 1, "a changed file is sent again");
        assert_eq!(new[0].1, "2025/clip.mov");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dry_run_counts_without_touching_photos() {
        let root = std::env::temp_dir().join(format!("lbs-iphone-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.jpg"), b"12345").unwrap();
        let settings = IphoneSettings {
            source: Some(root.display().to_string()),
            ..Default::default()
        };
        let (result, after) = send(
            settings.clone(),
            true,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        );
        let result = result.unwrap();
        assert_eq!(
            (
                result.sent,
                result.skipped,
                result.bytes,
                result.album.as_str()
            ),
            (1, 0, 5, DEFAULT_ALBUM)
        );
        assert_eq!(after, settings, "a check remembers nothing");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn settings_round_trip() {
        let directory =
            std::env::temp_dir().join(format!("lbs-iphone-settings-{}", std::process::id()));
        let mut settings = IphoneSettings {
            source: Some("/x".into()),
            album: "Holiday".into(),
            ..Default::default()
        };
        settings.sent.entry("Holiday".into()).or_default().insert(
            "a.jpg".into(),
            Stamp {
                size: 3,
                modified: 9,
            },
        );
        save(&directory, &settings).unwrap();
        assert_eq!(load(&directory), settings);
        let _ = std::fs::remove_dir_all(directory);
    }
}
