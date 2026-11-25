//! The Settings page: the list of jobs, the job editor and storage health checks.

use crate::app::{App, pick_folder};
use crate::model::{AutoRun, Filter, Job, PhoneTarget, Qnap, Source};
use crate::style::{self, format_bytes, muted};
use backup_engine::android::AndroidDevice;
use backup_engine::volumes::MountedVolume;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

const PHONE_FOLDERS: [(&str, &str, &str); 4] = [
    ("camera", "Camera", "DCIM/Camera"),
    ("screenshots", "Screenshots", "Pictures/Screenshots"),
    (
        "whatsapp",
        "WhatsApp media",
        "Android/media/com.whatsapp/WhatsApp/Media",
    ),
    ("downloads", "Downloads", "Download"),
];

/// Things the editor asks the app to do after drawing.
enum Action {
    Remove(String),
    CreateVolume { root: String, name: String },
    Reveal(String),
    LoadQnapFolders,
    CopyToDrive,
}

/// The Settings tab of the selected backup.
pub fn job_settings(app: &mut App, ui: &mut egui::Ui) {
    if let Some(index) = app.selected_index() {
        let mut job = app.jobs[index].clone();
        if app.editor.job_id != job.id {
            app.editor = crate::app::EditorState {
                job_id: job.id.clone(),
                ..Default::default()
            };
            app.qnap_folders.clear();
        }
        let root = job.destination_root.clone();
        let local = matches!(job.destination_kind.as_str(), "local" | "qnap-smb");
        let destination = if local {
            app.destination(&root)
                .map(|state| (state.info.clone(), state.snapshots.clone()))
        } else {
            None
        };
        let mut actions = Vec::new();
        let context = EditorContext {
            volumes: &app.volumes,
            phones: &app.phones,
            qnap_folders: &app.qnap_folders,
            destination,
        };
        job_editor(ui, &mut job, &context, &mut app.editor, &mut actions);
        if job != app.jobs[index] {
            app.jobs[index] = job.clone();
            app.mark_dirty();
        }
        for action in actions {
            match action {
                Action::Remove(id) => app.remove_job(&id),
                Action::CreateVolume { root, name } => {
                    app.create_volume(job.id.clone(), root, name)
                }
                Action::Reveal(path) => app.reveal(&path),
                Action::LoadQnapFolders => app.load_qnap_folders(),
                Action::CopyToDrive => app.add_drive_copy(),
            }
        }
    }
}

struct EditorContext<'a> {
    volumes: &'a [MountedVolume],
    phones: &'a [AndroidDevice],
    qnap_folders: &'a [String],
    destination: Option<(
        Option<backup_engine::volumes::DestinationInfo>,
        Vec<backup_engine::snapshots::Snapshot>,
    )>,
}

fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    style::surface().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).size(15.0).family(style::bold()));
        ui.add_space(4.0);
        add(ui);
    });
    ui.add_space(10.0);
}

fn field_label(ui: &mut egui::Ui, label: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(label)
            .size(12.0)
            .family(style::bold())
            .color(style::MUTED),
    );
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(style::MUTED));
}

fn volume_for<'a>(path: &str, volumes: &'a [MountedVolume]) -> Option<&'a MountedVolume> {
    volumes
        .iter()
        .find(|volume| backup_engine::volumes::volume_contains(&volume.path, path))
}

/// A folder shown as drive and path, with a picker. Returns a newly picked path, and whether remove was clicked.
fn path_row(
    ui: &mut egui::Ui,
    id: &str,
    path: &str,
    volumes: &[MountedVolume],
    placeholder: &str,
    removable: bool,
) -> (Option<String>, bool) {
    let mut picked = None;
    let mut removed = false;
    let volume = volume_for(path, volumes);
    let offline = backup_engine::volumes::external_drive_name(path).is_some() && volume.is_none();
    egui::Frame::new()
        .fill(style::WASH)
        .stroke(Stroke::new(1.0, style::LINE))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.push_id(id, |ui| {
                ui.horizontal(|ui| {
                    crate::icons::tile(
                        ui,
                        if volume.is_some() || offline {
                            crate::icons::Icon::Drive
                        } else {
                            crate::icons::Icon::Folder
                        },
                        if offline {
                            style::Tone::Ink
                        } else {
                            style::Tone::Leaf
                        },
                        30.0,
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        if path.is_empty() {
                            ui.label(muted(placeholder));
                        } else {
                            let name =
                                volume.map(|volume| volume.name.clone()).unwrap_or_else(|| {
                                    if offline {
                                        backup_engine::volumes::external_drive_name(path)
                                            .unwrap_or_else(|| "Drive".to_string())
                                    } else if cfg!(target_os = "macos") {
                                        "This Mac".to_string()
                                    } else {
                                        "This computer".to_string()
                                    }
                                });
                            let inside = volume
                                .map(|volume| {
                                    path.get(volume.path.len()..)
                                        .unwrap_or("")
                                        .trim_start_matches(['/', '\\'])
                                        .to_string()
                                })
                                .unwrap_or_else(|| path.to_string());
                            ui.label(RichText::new(name).family(style::bold()));
                            ui.add(
                                egui::Label::new(muted(if offline {
                                    "Drive not connected".to_string()
                                } else if inside.is_empty() {
                                    "Whole drive".to_string()
                                } else {
                                    inside
                                }))
                                .truncate(),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if removable
                            && ui
                                .add(style::ghost_button("×"))
                                .on_hover_text("Remove")
                                .clicked()
                        {
                            removed = true;
                        }
                        if ui
                            .add(style::ghost_button(if path.is_empty() {
                                "Choose"
                            } else {
                                "Change"
                            }))
                            .clicked()
                        {
                            picked = pick_folder(Some(path));
                        }
                    });
                });
                if path.is_empty() && !volumes.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for volume in volumes {
                            if ui.add(style::ghost_button(volume.name.clone())).clicked() {
                                picked = pick_folder(Some(&volume.path));
                            }
                        }
                    });
                }
            });
        });
    (picked, removed)
}

fn phone_select(ui: &mut egui::Ui, id: &str, value: &mut String, phones: &[AndroidDevice]) {
    let name = |device: &AndroidDevice| {
        format!(
            "{} {}",
            style::title_case(&device.manufacturer),
            device.model
        )
    };
    let selected = if value.is_empty() {
        "Whichever phone is connected".to_string()
    } else {
        phones
            .iter()
            .find(|device| &device.id == value)
            .map(name)
            .unwrap_or_else(|| "Saved phone (not connected)".to_string())
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(240.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(value, String::new(), "Whichever phone is connected");
            for device in phones {
                ui.selectable_value(value, device.id.clone(), name(device));
            }
        });
}

fn job_editor(
    ui: &mut egui::Ui,
    job: &mut Job,
    context: &EditorContext,
    editor: &mut crate::app::EditorState,
    actions: &mut Vec<Action>,
) {
    card(ui, "Name", |ui| {
        let name_label = ui.label(muted("Shown in the list on the left."));
        ui.add(
            egui::TextEdit::singleline(&mut job.name)
                .font(egui::FontId::new(15.0, style::bold()))
                .desired_width(ui.available_width().min(460.0)),
        )
        .labelled_by(name_label.id);
    });
    card(ui, "What to back up", |ui| sources(ui, job, context));
    card(ui, "Where the backup goes", |ui| {
        destination(ui, job, context, editor, actions)
    });
    card(ui, "Automatic backups", |ui| {
        let auto = job.auto_run.get_or_insert_with(AutoRun::default);
        let mut connected = auto.when_connected.unwrap_or(false);
        if style::toggle_row(
            ui,
            &mut connected,
            "Run when connected",
            Some("Starts as soon as the drive (or phone) is plugged in."),
        ) {
            auto.when_connected = Some(connected);
        }
        field_label(ui, "Repeat while connected");
        let mut hours = auto.every_hours.unwrap_or(0);
        if style::segmented(
            ui,
            &mut hours,
            &[
                (0, "Off", true, None),
                (1, "Hourly", true, None),
                (6, "Every 6 h", true, None),
                (24, "Daily", true, None),
                (168, "Weekly", true, None),
            ],
        ) {
            auto.every_hours = (hours > 0).then_some(hours);
        }
        if job.auto_run == Some(AutoRun::default()) {
            job.auto_run = None;
        }
    });
    card(ui, "Older versions", |ui| {
        versions(ui, job, context, actions)
    });
    if job.destination_kind == "qnap-rsync" {
        card(ui, "QNAP connection", |ui| qnap(ui, job, context, actions));
    }

    style::surface().show(ui, |ui| {
        ui.set_width(ui.available_width());
        let header = egui::CollapsingHeader::new(
            RichText::new("Advanced options")
                .size(15.0)
                .family(style::bold()),
        )
        .id_salt(("advanced", &job.id))
        .default_open(editor.advanced_open);
        let state = header.show(ui, |ui| advanced(ui, job, editor));
        editor.advanced_open = state.fully_open();
    });
    ui.add_space(10.0);

    ui.horizontal(|ui| {
        if ui
            .add(style::text_button("Make a copy on another drive"))
            .clicked()
        {
            actions.push(Action::CopyToDrive);
        }
    });
    ui.horizontal(|ui| {
        if ui.add(style::danger_button("Delete this backup")).clicked() {
            actions.push(Action::Remove(job.id.clone()));
        }
        ui.label(muted("Files already backed up stay on the drive."));
    });
}

fn sources(ui: &mut egui::Ui, job: &mut Job, context: &EditorContext) {
    let count = job.sources.len();
    let mut remove = None;
    for index in 0..count {
        let source = &mut job.sources[index];
        if source.is_phone() {
            egui::Frame::new()
                .fill(style::WASH)
                .stroke(Stroke::new(1.0, style::LINE))
                .corner_radius(CornerRadius::same(7))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        crate::icons::tile(ui, crate::icons::Icon::Phone, style::Tone::Sky, 30.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut source.path)
                                .hint_text("DCIM/Camera")
                                .desired_width(220.0),
                        );
                        let mut device = source.device_id.clone().unwrap_or_default();
                        phone_select(
                            ui,
                            &format!("source-phone-{index}"),
                            &mut device,
                            context.phones,
                        );
                        if source.device_id.as_deref().unwrap_or("") != device {
                            source.device_id = Some(device);
                        }
                        if count > 1 {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(style::ghost_button("×"))
                                        .on_hover_text("Remove")
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                },
                            );
                        }
                    });
                });
        } else {
            let (picked, removed) = path_row(
                ui,
                &format!("source-{index}"),
                &source.path,
                context.volumes,
                "Choose a folder or a whole drive",
                count > 1,
            );
            if let Some(path) = picked {
                source.label = match backup_engine::volumes::last_component(&path) {
                    "" => "Folder".to_string(),
                    name => name.to_string(),
                };
                source.path = path;
            }
            if removed {
                remove = Some(index);
            }
        }
    }
    if let Some(index) = remove {
        job.sources.remove(index);
    }
    ui.horizontal(|ui| {
        let add_label = ui.label(muted("Add"));
        if ui.add(style::ghost_button("+ Folder")).clicked() {
            let id = job.unique_source_id("source");
            job.sources.push(Source {
                id,
                label: "Folder".into(),
                category: "other".into(),
                kind: Some("folder".into()),
                ..Default::default()
            });
        }
        let unused: Vec<_> = PHONE_FOLDERS
            .iter()
            .filter(|(_, _, path)| {
                !job.sources
                    .iter()
                    .any(|source| source.is_phone() && source.path == *path)
            })
            .collect();
        if !unused.is_empty() {
            let phone_device = job
                .sources
                .iter()
                .find(|source| source.is_phone())
                .and_then(|source| source.device_id.clone())
                .unwrap_or_default();
            let mut chosen: Option<(&str, &str, &str)> = None;
            egui::ComboBox::from_id_salt("add-phone-folder")
                .selected_text("+ Phone folder")
                .show_ui(ui, |ui| {
                    for (id, label, path) in &unused {
                        if ui.selectable_label(false, *label).clicked() {
                            chosen = Some((id, label, path));
                        }
                    }
                })
                .response
                .labelled_by(add_label.id);
            if let Some((id, label, path)) = chosen {
                let id = job.unique_source_id(id);
                job.sources.push(Source {
                    id,
                    label: label.into(),
                    path: path.into(),
                    category: "photos".into(),
                    kind: Some("android".into()),
                    device_id: Some(phone_device),
                    ..Default::default()
                });
            }
        }
    });
}

fn destination(
    ui: &mut egui::Ui,
    job: &mut Job,
    context: &EditorContext,
    editor: &mut crate::app::EditorState,
    actions: &mut Vec<Action>,
) {
    let mut kind = if job.destination_kind == "qnap-smb" {
        "local".to_string()
    } else {
        job.destination_kind.clone()
    };
    let mut options = vec![
        ("local".to_string(), "Drive or folder", true, None),
        ("android".to_string(), "Phone", true, None),
    ];
    if job.destination_kind == "qnap-rsync" {
        options.push(("qnap-rsync".to_string(), "QNAP", true, None));
    }
    if style::segmented(ui, &mut kind, &options) {
        let was_phone = job.destination_kind == "android";
        job.destination_root = if kind == "android" {
            "Backup".to_string()
        } else if was_phone {
            String::new()
        } else {
            job.destination_root.clone()
        };
        if kind == "android" && job.phone.is_none() {
            job.phone = Some(PhoneTarget::default());
        }
        job.destination_kind = kind;
    }
    ui.add_space(6.0);

    let info = context
        .destination
        .as_ref()
        .and_then(|(info, _)| info.clone());
    let single = job.sources.len() == 1;
    match job.destination_kind.as_str() {
        "android" => {
            ui.horizontal(|ui| {
                crate::icons::tile(ui, crate::icons::Icon::Phone, style::Tone::Sky, 30.0);
                ui.add(
                    egui::TextEdit::singleline(&mut job.destination_root)
                        .hint_text("Backup")
                        .desired_width(220.0),
                );
                let phone = job.phone.get_or_insert_with(PhoneTarget::default);
                phone_select(
                    ui,
                    "destination-phone",
                    &mut phone.device_id,
                    context.phones,
                );
            });
        }
        "qnap-rsync" => {}
        _ => {
            let (picked, _) = path_row(
                ui,
                "destination",
                &job.destination_root,
                context.volumes,
                "Choose where the backup goes",
                false,
            );
            if let Some(path) = picked {
                job.destination_root = path;
            }
        }
    }

    if let Some(info) = &info {
        ui.horizontal_wrapped(|ui| {
            if let Some(filesystem) = &info.filesystem {
                style::chip(
                    ui,
                    &filesystem.to_uppercase(),
                    Color32::from_rgb(0xe8, 0xe5, 0xdc),
                    style::INK,
                );
            }
            if let Some(free) = volume_for(&job.destination_root, context.volumes)
                .and_then(|volume| volume.available)
            {
                style::chip(
                    ui,
                    &format!("{} free", format_bytes(free)),
                    Color32::from_rgb(0xe8, 0xe5, 0xdc),
                    style::INK,
                );
            }
            if info.is_volume_root {
                style::chip(
                    ui,
                    "Whole volume",
                    Color32::from_rgb(0xe8, 0xe5, 0xdc),
                    style::INK,
                );
            }
            if info.ccc_backup {
                style::chip(
                    ui,
                    "Carbon Copy Cloner backup",
                    style::LEAF_SOFT,
                    style::LEAF,
                );
            }
        });
        if info.ccc_backup && job.layout() != "mirror" {
            egui::Frame::new().fill(style::LEAF_SOFT).corner_radius(CornerRadius::same(7)).inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label("This volume holds a Carbon Copy Cloner backup. Continue it as an exact mirror: files already there are not copied again.");
                if ui.add_enabled(single, style::primary_button("Continue CCC backup")).clicked() {
                    job.layout = Some("mirror".into());
                }
            });
        }
    }

    if job.destination_kind != "android" {
        field_label(ui, "Layout");
        let mut layout = job.layout().to_string();
        let mirror_hint = if single {
            None
        } else {
            Some("A mirror copies exactly one source")
        };
        if style::segmented(
            ui,
            &mut layout,
            &[
                ("per-source".to_string(), "Folder per source", true, None),
                ("mirror".to_string(), "Exact mirror", single, mirror_hint),
            ],
        ) {
            job.layout = Some(layout);
        }
        hint(
            ui,
            if job.layout() == "mirror" {
                "The destination becomes an exact copy of the source."
            } else {
                "Each source gets its own folder in the destination."
            },
        );
    }

    if let Some(info) = info.filter(|info| info.can_create_volume && !info.is_volume_root) {
        match &mut editor.volume_name {
            None => {
                if ui
                    .add(style::text_button(format!(
                        "+ Give this job its own volume on {}",
                        info.volume_name.as_deref().unwrap_or("this drive")
                    )))
                    .clicked()
                {
                    let name: String = job.name.replace(['/', ':'], " ").chars().take(60).collect();
                    editor.volume_name = Some(name);
                }
            }
            Some(name) => {
                let mut cancel = false;
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(name).desired_width(220.0));
                    let ready = !editor.creating_volume && !name.trim().is_empty();
                    if ui
                        .add_enabled(
                            ready,
                            style::primary_button(if editor.creating_volume {
                                "Making"
                            } else {
                                "Create volume"
                            }),
                        )
                        .clicked()
                    {
                        actions.push(Action::CreateVolume {
                            root: job.destination_root.clone(),
                            name: name.trim().to_string(),
                        });
                    }
                    if ui
                        .add(style::ghost_button("×"))
                        .on_hover_text("Cancel")
                        .clicked()
                    {
                        cancel = true;
                    }
                });
                if cancel {
                    editor.volume_name = None;
                }
            }
        }
    }
    if let Some(status) = &editor.volume_status {
        hint(ui, status);
    }
}

fn versions(ui: &mut egui::Ui, job: &mut Job, context: &EditorContext, actions: &mut Vec<Action>) {
    let local = matches!(job.destination_kind.as_str(), "local" | "qnap-smb");
    let info = context
        .destination
        .as_ref()
        .and_then(|(info, _)| info.clone());
    let possible = local
        && info
            .as_ref()
            .map(|info| info.snapshots_supported)
            .unwrap_or(true);
    let current = if job.mode == "append-only" {
        "off"
    } else if job.versioning.as_deref() == Some("snapshots") && possible {
        "snapshots"
    } else {
        "archive"
    };
    let mut value = current.to_string();
    let hint_text = if possible {
        None
    } else {
        Some("Needs an APFS drive")
    };
    if style::segmented(
        ui,
        &mut value,
        &[
            ("snapshots".to_string(), "Snapshots", possible, hint_text),
            ("archive".to_string(), "Archive folder", true, None),
            ("off".to_string(), "Don't keep", true, None),
        ],
    ) {
        job.mode = if value == "off" {
            "append-only".into()
        } else {
            "safe-mirror".into()
        };
        job.versioning = Some(if value == "snapshots" {
            "snapshots".into()
        } else {
            "archive".into()
        });
    }
    hint(
        ui,
        match value.as_str() {
            "snapshots" => {
                "After every run the backup is cloned to _Snapshots. Only changes take extra space."
            }
            "archive" => {
                "Files that change are moved to _Archive before the new version is written."
            }
            _ => "Changed files are overwritten. Deleted source files still stay in the backup.",
        },
    );
    if value != "off" {
        field_label(ui, "Keep for");
        let mut days = job.archive_keep_days.unwrap_or(0);
        if style::segmented(
            ui,
            &mut days,
            &[
                (30, "30 days", true, None),
                (90, "90 days", true, None),
                (365, "1 year", true, None),
                (0, "Forever", true, None),
            ],
        ) {
            job.archive_keep_days = (days > 0).then_some(days);
        }
    }
    if let Some((_, snapshots)) = &context.destination {
        for snapshot in snapshots.iter().take(5) {
            ui.horizontal(|ui| {
                ui.label(style::format_date_time(snapshot.created_at));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(style::ghost_button(if cfg!(target_os = "macos") {
                            "Show in Finder"
                        } else {
                            "Open folder"
                        }))
                        .clicked()
                    {
                        actions.push(Action::Reveal(snapshot.path.clone()));
                    }
                });
            });
        }
        if snapshots.len() > 5 {
            hint(ui, &format!("and {} older", snapshots.len() - 5));
        }
    }
}

fn qnap(ui: &mut egui::Ui, job: &mut Job, context: &EditorContext, actions: &mut Vec<Action>) {
    let settings = job.qnap.get_or_insert_with(Qnap::default);
    egui::Grid::new("qnap-grid")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Host");
            ui.add(egui::TextEdit::singleline(&mut settings.host).hint_text("qnap.local"));
            ui.end_row();
            ui.label("Username");
            ui.text_edit_singleline(&mut settings.username);
            ui.end_row();
            ui.label("Share");
            ui.text_edit_singleline(&mut settings.share);
            ui.end_row();
            ui.label("SSH port");
            ui.add(egui::DragValue::new(&mut settings.ssh_port).range(1..=65535));
            ui.end_row();
        });
    field_label(ui, "Folder on the NAS");
    if context.qnap_folders.is_empty() {
        ui.add(egui::TextEdit::singleline(&mut job.destination_root).hint_text("/share/Backups"));
    } else {
        let selected = if job.destination_root.is_empty() {
            "Choose a folder".to_string()
        } else {
            job.destination_root.clone()
        };
        egui::ComboBox::from_id_salt("qnap-folder")
            .selected_text(selected)
            .width(320.0)
            .show_ui(ui, |ui| {
                for folder in context.qnap_folders {
                    ui.selectable_value(&mut job.destination_root, folder.clone(), folder);
                }
            });
    }
    if ui.add(style::ghost_button("Load folders")).clicked() {
        actions.push(Action::LoadQnapFolders);
    }
}

fn advanced(ui: &mut egui::Ui, job: &mut Job, editor: &mut crate::app::EditorState) {
    field_label(ui, "Skip");
    let filters = job.filters.get_or_insert_with(Vec::new);
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        if filters.is_empty() {
            hint(ui, "Nothing is skipped.");
        }
        for (index, filter) in filters.iter().enumerate() {
            let include = filter.action == "include";
            let fill = if include {
                style::LEAF_SOFT
            } else {
                Color32::from_rgb(0xe8, 0xe5, 0xdc)
            };
            egui::Frame::new()
                .fill(fill)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(8, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let text =
                            format!("{}{}", if include { "Only " } else { "" }, filter.pattern);
                        let label = ui.label(RichText::new(text).size(11.0).monospace());
                        if !filter.note.is_empty() {
                            label.on_hover_text(&filter.note);
                        }
                        if ui
                            .add(egui::Button::new(RichText::new("×").size(10.0)).frame(false))
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
        }
    });
    if let Some(index) = remove {
        filters.remove(index);
    }
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut editor.filter_input)
                .hint_text("Skip files or folders, e.g. *.tmp or **/Cache/**")
                .desired_width(320.0),
        );
        let entered =
            response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let value = editor.filter_input.trim().to_string();
        if (ui
            .add_enabled(!value.is_empty(), style::ghost_button("+ Skip"))
            .clicked()
            || entered)
            && !value.is_empty()
            && !filters.iter().any(|filter| filter.pattern == value)
        {
            filters.push(Filter {
                id: crate::model::new_id("exclude"),
                pattern: value,
                action: "exclude".into(),
                note: String::new(),
                extra: Default::default(),
            });
            editor.filter_input.clear();
        }
    });
    if job.filters.as_ref().is_some_and(Vec::is_empty) {
        job.filters = None;
    }

    field_label(ui, "Checking files");
    let mut verification = job.verification.clone();
    if style::segmented(
        ui,
        &mut verification,
        &[
            ("metadata".to_string(), "Size and date", true, None),
            ("important-hash".to_string(), "Hash catalogs", true, None),
            ("full-hash".to_string(), "Hash everything", true, None),
        ],
    ) {
        job.verification = verification;
    }
    hint(
        ui,
        "Hashing reads the whole file, so it is slower but catches silent changes.",
    );

    field_label(ui, "Files at the same time");
    let mut parallel = job.parallel_copies.unwrap_or(4);
    if style::segmented(
        ui,
        &mut parallel,
        &[
            (1, "1", true, None),
            (2, "2", true, None),
            (4, "4", true, None),
            (8, "8", true, None),
        ],
    ) {
        job.parallel_copies = Some(parallel);
    }
    hint(ui, "1 or 2 for hard disks and video, 4 to 8 for SSDs.");
    ui.add_space(6.0);

    let mut guard = job.guard("lightroom-closed");
    if style::toggle_row(
        ui,
        &mut guard,
        "Skip live Lightroom catalogs",
        Some("Catalog files are left out while Lightroom Classic is open."),
    ) {
        job.set_guard("lightroom-closed", guard);
    }
    let mut check = job.check_destination.unwrap_or(false);
    if style::toggle_row(
        ui,
        &mut check,
        "Check every file on the destination",
        Some("Slower. Notices files removed from the backup by hand."),
    ) {
        job.check_destination = Some(check);
    }
    style::toggle_row(
        ui,
        &mut job.preserve_mac_metadata,
        "Keep Finder tags and labels",
        None,
    );
}
