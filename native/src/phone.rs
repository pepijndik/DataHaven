//! The "Send to phone" panel: one click sends a folder of photos to an album on an Android phone.

use crate::app::{pick_folder, App, Event};
use crate::style::{self, format_bytes, muted};
use backup_engine::android::{self, AndroidAlbum, AndroidProgress, AndroidSendResult, AndroidStorage, PhoneSettings, SendRequest};
use backup_engine::runtime;
use eframe::egui::{self, RichText};

pub enum PhoneEvent {
    Loaded { device_id: String, settings: Option<PhoneSettings>, name: Option<Option<String>>, storages: Result<Vec<AndroidStorage>, String>, albums: Result<Vec<AndroidAlbum>, String> },
    Albums { device_id: String, albums: Vec<AndroidAlbum> },
    Progress(AndroidProgress),
    Sent { device_id: String, dry_run: bool, result: Result<AndroidSendResult, String> },
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AlbumMode {
    New,
    Rename,
}

#[derive(Default)]
pub struct PhonePanel {
    pub device_id: Option<String>,
    pub name: Option<String>,
    pub settings: PhoneSettings,
    pub storages: Vec<AndroidStorage>,
    pub albums: Vec<AndroidAlbum>,
    pub setup: bool,
    pub album_input: Option<(AlbumMode, String)>,
    pub sending: bool,
    pub checking: bool,
    pub progress: Option<AndroidProgress>,
    pub result: Option<AndroidSendResult>,
    pub error: Option<String>,
}

pub fn send_summary(result: &AndroidSendResult) -> String {
    let plural = |count: u32| if count == 1 { "" } else { "s" };
    if result.dry_run {
        return if result.sent == 0 {
            format!("Nothing new. {} file{} already on the phone.", result.skipped, if result.skipped == 1 { " is" } else { "s are" })
        } else {
            format!("{} new file{} to send ({}), {} already on the phone.", result.sent, plural(result.sent), format_bytes(result.bytes), result.skipped)
        };
    }
    let mut parts = vec![format!("{} sent ({})", result.sent, format_bytes(result.bytes)), format!("{} already on the phone", result.skipped)];
    if result.failed > 0 {
        parts.push(format!("{} failed", result.failed));
    }
    if result.conflicts > 0 {
        parts.push(format!("{} skipped because of a name clash", result.conflicts));
    }
    format!("{}{}.", if result.cancelled { "Stopped. " } else { "" }, parts.join(", "))
}

/// Every call opens the USB connection, so phone work runs one call after the other on one thread.
fn load(app: &App, device_id: String, first_time: bool, storage_id: Option<u64>) {
    let directory = app.data_dir.clone();
    app.spawn(move || {
        let settings = first_time.then(|| android::load_settings(&directory, &device_id));
        let storage = settings.as_ref().map(|settings| settings.storage_id).unwrap_or(storage_id);
        let name = if first_time { Some(runtime::block_on(android::display_name(&device_id))) } else { None };
        let storages = runtime::block_on(android::list_storages(&device_id));
        let albums = runtime::block_on(android::list_albums(&device_id, storage));
        Event::Phone(PhoneEvent::Loaded { device_id, settings, name, storages, albums })
    });
}

pub fn handle(app: &mut App, event: PhoneEvent) {
    let panel = &mut app.phone;
    match event {
        PhoneEvent::Loaded { device_id, settings, name, storages, albums } => {
            if panel.device_id.as_deref() != Some(device_id.as_str()) {
                return;
            }
            if let Some(settings) = settings {
                panel.setup = settings.source.is_none();
                panel.settings = settings;
            }
            if let Some(name) = name {
                panel.name = name;
            }
            match storages.and_then(|storages| albums.map(|albums| (storages, albums))) {
                Ok((storages, albums)) => {
                    panel.storages = storages;
                    panel.albums = albums;
                    panel.error = None;
                }
                Err(error) => {
                    panel.error = Some(error.clone());
                    app.message = error;
                }
            }
        }
        PhoneEvent::Albums { device_id, albums } => {
            if panel.device_id.as_deref() == Some(device_id.as_str()) {
                panel.albums = albums;
            }
        }
        PhoneEvent::Progress(progress) => panel.progress = Some(progress),
        PhoneEvent::Sent { device_id, dry_run, result } => {
            panel.sending = false;
            panel.checking = false;
            panel.progress = None;
            match result {
                Ok(result) => {
                    app.message = send_summary(&result);
                    panel.result = Some(result);
                    if !dry_run {
                        let _ = android::save_settings(&app.data_dir, &device_id, panel.settings.clone());
                        let storage = panel.settings.storage_id;
                        load(app, device_id, false, storage);
                    }
                }
                Err(error) => {
                    panel.error = Some(error.clone());
                    app.message = error;
                }
            }
        }
        PhoneEvent::Error(error) => {
            panel.error = Some(error.clone());
            app.message = error;
        }
    }
}

/// Keeps the panel on a connected phone and loads it when the phone changes.
fn follow_devices(app: &mut App) {
    let connected = app.phone.device_id.as_ref().is_some_and(|id| app.phones.iter().any(|device| &device.id == id));
    if connected {
        return;
    }
    let next = app.phones.first().map(|device| device.id.clone());
    select_device(app, next);
}

fn select_device(app: &mut App, device_id: Option<String>) {
    app.phone = PhonePanel { device_id: device_id.clone(), ..PhonePanel::default() };
    if let Some(id) = device_id {
        load(app, id, true, None);
    }
}

fn remember(app: &mut App, settings: PhoneSettings) {
    app.phone.settings = settings.clone();
    app.phone.result = None;
    if let Some(id) = &app.phone.device_id {
        if let Err(error) = android::save_settings(&app.data_dir, id, settings) {
            app.phone.error = Some(format!("Could not save the phone settings: {error}"));
        }
    }
}

fn send(app: &mut App, dry_run: bool) {
    let (Some(device_id), Some(source)) = (app.phone.device_id.clone(), app.phone.settings.source.clone()) else { return };
    if dry_run { app.phone.checking = true } else { app.phone.sending = true }
    app.phone.result = None;
    app.phone.error = None;
    let request = SendRequest { device_id: device_id.clone(), storage_id: app.phone.settings.storage_id, source, album: app.phone.settings.album.clone(), dry_run };
    let directory = app.data_dir.clone();
    let sender = app.sender.clone();
    let ctx = app.ctx.clone();
    app.spawn(move || {
        let on_progress = move |progress: AndroidProgress| {
            let _ = sender.send(Event::Phone(PhoneEvent::Progress(progress)));
            ctx.request_repaint();
        };
        let result = runtime::block_on(android::send(request, &directory, on_progress));
        Event::Phone(PhoneEvent::Sent { device_id, dry_run, result })
    });
}

fn save_album(app: &mut App, mode: AlbumMode, value: String) {
    let name = value.trim().to_string();
    let Some(device_id) = app.phone.device_id.clone() else { return };
    if name.is_empty() {
        return;
    }
    let storage = app.phone.settings.storage_id;
    let current = app.phone.settings.album.clone();
    let mut settings = app.phone.settings.clone();
    settings.album = Some(name.clone());
    remember(app, settings);
    app.spawn(move || {
        let changed = match mode {
            AlbumMode::New => runtime::block_on(android::create_album(&device_id, storage, &name)),
            AlbumMode::Rename => match current {
                Some(old) if old != name => runtime::block_on(android::rename_album(&device_id, storage, &old, &name)),
                _ => Ok(()),
            },
        };
        match changed.and_then(|()| runtime::block_on(android::list_albums(&device_id, storage))) {
            Ok(albums) => Event::Phone(PhoneEvent::Albums { device_id, albums }),
            Err(error) => Event::Phone(PhoneEvent::Error(format!("Album change failed: {error}"))),
        }
    });
}

pub fn panel(app: &mut App, ui: &mut egui::Ui) {
    follow_devices(app);
    style::surface().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let Some(device) = app.phone.device_id.as_ref().and_then(|id| app.phones.iter().find(|device| &device.id == id)).cloned() else {
            ui.label(style::eyebrow("Phone"));
            ui.label(style::title("No phone connected"));
            ui.add_space(6.0);
            ui.label(muted("Plug in your Android phone, unlock it and choose \"File transfer\" in the USB notification."));
            return;
        };

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(style::eyebrow("Send to phone"));
                let name = app.phone.name.clone().unwrap_or_else(|| style::title_case(&device.manufacturer));
                ui.label(style::title(&name));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| style::dot(ui, style::LEAF));
        });

        if app.phones.len() > 1 {
            let mut chosen = device.id.clone();
            egui::ComboBox::from_id_salt("phone-device").selected_text(format!("{} {}", style::title_case(&device.manufacturer), device.model)).show_ui(ui, |ui| {
                for item in &app.phones {
                    ui.selectable_value(&mut chosen, item.id.clone(), format!("{} {}", style::title_case(&item.manufacturer), item.model));
                }
            });
            if chosen != device.id && !app.phone.sending {
                select_device(app, Some(chosen));
                return;
            }
        }

        let album_label = app.phone.settings.album.clone().filter(|album| !album.is_empty()).unwrap_or_else(|| "Fotografie".to_string());
        if app.phone.setup {
            setup(app, ui);
        } else {
            let percent = app.phone.progress.as_ref().map(|progress| {
                if progress.total_bytes > 0 { (progress.bytes as f32 / progress.total_bytes as f32).min(1.0) } else if progress.total > 0 { progress.done as f32 / progress.total as f32 } else { 0.0 }
            });
            let label = if app.phone.sending { format!("Sending to {album_label} {}%", (percent.unwrap_or(0.0) * 100.0).round()) } else { format!("Send to {album_label}") };
            let busy = app.phone.sending || app.phone.checking;
            if ui.add_enabled(!busy, style::primary_button(label).min_size(egui::vec2(ui.available_width(), 40.0))).clicked() {
                send(app, false);
            }
            ui.label(muted(format!("From {}", app.phone.settings.source.clone().unwrap_or_default())));
            if app.phone.sending {
                style::track(ui, percent.unwrap_or(0.0), 6.0);
                if let Some(progress) = &app.phone.progress {
                    if !progress.file.is_empty() {
                        ui.label(muted(format!("{} of {}: {}", progress.done + 1, progress.total, progress.file)));
                    }
                }
                if ui.add(style::text_button("Stop")).clicked() {
                    android::cancel_send();
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!app.phone.checking, style::text_button(if app.phone.checking { "Checking" } else { "What's new?" })).clicked() {
                        send(app, true);
                    }
                    if ui.add(style::text_button("Change")).clicked() {
                        app.phone.setup = true;
                    }
                });
            }
        }

        if let Some(error) = app.phone.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&error).size(12.0).color(style::CORAL));
                if error.starts_with(android::BUSY_MESSAGE) && ui.add(style::text_button("Free the phone")).clicked() {
                    let _ = android::free_device();
                    app.phone.error = None;
                    if let Some(id) = app.phone.device_id.clone() {
                        load(app, id, false, app.phone.settings.storage_id);
                    }
                }
            });
        }
        if let Some(result) = &app.phone.result {
            ui.label(muted(send_summary(result)));
            for issue in result.issues.iter().take(5) {
                ui.label(RichText::new(format!("{}  {}", issue.file, issue.message)).size(11.0).color(style::MUTED));
            }
            if result.issues.len() > 5 {
                ui.label(muted(format!("and {} more", result.issues.len() - 5)));
            }
        }
        if !app.phone.sending && ui.add(style::text_button("Back up this phone to a drive")).clicked() {
            app.create_phone_job(Some(device));
        }
    });
}

fn setup(app: &mut App, ui: &mut egui::Ui) {
    ui.label(muted("1. Where are the photos and videos?"));
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for volume in &app.volumes {
            if ui.add(style::ghost_button(&volume.name)).clicked() {
                picked = Some(Some(volume.path.clone()));
            }
        }
        if ui.add(style::ghost_button(if cfg!(target_os = "macos") { "This Mac" } else { "This computer" })).clicked() {
            picked = Some(None);
        }
    });
    if let Some(start) = picked {
        let start = start.or_else(|| app.phone.settings.source.clone());
        if let Some(folder) = pick_folder(start.as_deref()) {
            let settings = PhoneSettings { source: Some(folder), ..app.phone.settings.clone() };
            remember(app, settings);
        }
    }
    if let Some(source) = &app.phone.settings.source {
        ui.label(RichText::new(source).size(11.0).monospace().color(style::MUTED));
    }

    ui.label(muted(format!("2. Which album on the phone? ({})", android::ROOT_FOLDER)));
    if let Some((mode, mut text)) = app.phone.album_input.clone() {
        let response = ui.add(egui::TextEdit::singleline(&mut text).hint_text(if mode == AlbumMode::New { "New album name" } else { "Album name" }));
        response.request_focus();
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if escape {
            app.phone.album_input = None;
        } else if enter || response.lost_focus() {
            app.phone.album_input = None;
            save_album(app, mode, text);
        } else {
            app.phone.album_input = Some((mode, text));
        }
    } else {
        let mut album = app.phone.settings.album.clone().unwrap_or_default();
        let mut options: Vec<(String, String)> = app.phone.albums.iter().map(|item| (item.name.clone(), format!("{} ({})", item.name, item.files))).collect();
        if !album.is_empty() && !options.iter().any(|(name, _)| name.eq_ignore_ascii_case(&album)) {
            options.push((album.clone(), format!("{album} (0)")));
        }
        ui.horizontal_wrapped(|ui| {
            let selected = if album.is_empty() { "Fotografie (no album)".to_string() } else { album.clone() };
            egui::ComboBox::from_id_salt("phone-album").selected_text(selected).show_ui(ui, |ui| {
                ui.selectable_value(&mut album, String::new(), "Fotografie (no album)");
                for (name, label) in &options {
                    ui.selectable_value(&mut album, name.clone(), label);
                }
            });
            if ui.add(style::ghost_button("New album")).clicked() {
                app.phone.album_input = Some((AlbumMode::New, String::new()));
            }
            if app.phone.settings.album.as_deref().is_some_and(|album| !album.is_empty()) && ui.add(style::ghost_button("Rename")).clicked() {
                app.phone.album_input = Some((AlbumMode::Rename, app.phone.settings.album.clone().unwrap_or_default()));
            }
        });
        let current = app.phone.settings.album.clone().unwrap_or_default();
        if album != current {
            let settings = PhoneSettings { album: (!album.is_empty()).then_some(album), ..app.phone.settings.clone() };
            remember(app, settings);
        }
    }

    let writable: Vec<AndroidStorage> = app.phone.storages.iter().filter(|storage| storage.writable).cloned().collect();
    if writable.len() > 1 {
        ui.label(muted("3. Phone storage"));
        let current = app.phone.settings.storage_id.unwrap_or(writable[0].id);
        let mut chosen = current;
        let label = |storage: &AndroidStorage| format!("{} ({} free)", if storage.description.is_empty() { "Storage" } else { &storage.description }, format_bytes(storage.free));
        let selected = writable.iter().find(|storage| storage.id == current).map(label).unwrap_or_default();
        egui::ComboBox::from_id_salt("phone-storage").selected_text(selected).show_ui(ui, |ui| {
            for storage in &writable {
                ui.selectable_value(&mut chosen, storage.id, label(storage));
            }
        });
        if chosen != current {
            let settings = PhoneSettings { storage_id: Some(chosen), ..app.phone.settings.clone() };
            remember(app, settings);
            if let Some(id) = app.phone.device_id.clone() {
                load(app, id, false, Some(chosen));
            }
        }
    }
    if ui.add_enabled(app.phone.settings.source.is_some(), style::primary_button("Done")).clicked() {
        app.phone.setup = false;
    }
}
