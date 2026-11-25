//! "New backup": three steps from nothing to a ready drive backup.
//! 1. What: photos, videos, a Lightroom library or a whole drive.
//! 2. From: a connected drive or a folder.
//! 3. To: the backup drive.

use crate::app::{App, pick_folder};
use crate::model::{self, Filter, Job, Source};
use crate::style::{self, Tone, format_bytes, muted};
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Photos,
    Videos,
    Lightroom,
    WholeDrive,
}

impl Kind {
    pub const ALL: [Kind; 4] = [
        Kind::Photos,
        Kind::Videos,
        Kind::Lightroom,
        Kind::WholeDrive,
    ];

    fn label(self) -> &'static str {
        match self {
            Kind::Photos => "Photos",
            Kind::Videos => "Videos",
            Kind::Lightroom => "Lightroom",
            Kind::WholeDrive => "Whole drive",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Kind::Photos => "Photo folders and archives, many files at once",
            Kind::Videos => "Large video files, copied a few at a time",
            Kind::Lightroom => "Catalog, sidecars and originals, catalogs hash checked",
            Kind::WholeDrive => "Everything on a drive, system folders skipped",
        }
    }

    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Kind::Photos => Icon::Folder,
            Kind::Videos => Icon::Play,
            Kind::Lightroom => Icon::Layers,
            Kind::WholeDrive => Icon::Drive,
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            Kind::Photos => Tone::Leaf,
            Kind::Videos => Tone::Sky,
            Kind::Lightroom => Tone::Sun,
            Kind::WholeDrive => Tone::Ink,
        }
    }

    fn preset(self) -> &'static str {
        match self {
            Kind::Photos | Kind::WholeDrive => "media-archive",
            Kind::Videos => "video-archive",
            Kind::Lightroom => "lightroom-optimized",
        }
    }

    fn category(self) -> &'static str {
        match self {
            Kind::Photos => "photos",
            Kind::Videos => "video",
            Kind::Lightroom => "catalog",
            Kind::WholeDrive => "other",
        }
    }

    fn source_hint(self) -> &'static str {
        match self {
            Kind::Photos => "the drive or folder with your photos",
            Kind::Videos => "the drive or folder with your videos",
            Kind::Lightroom => "the folder with your Lightroom catalog",
            Kind::WholeDrive => "the drive to back up",
        }
    }
}

/// Folders that belong to the operating system or to an earlier backup, never worth copying.
const SYSTEM_FOLDERS: [&str; 8] = [
    ".Spotlight-V100",
    ".fseventsd",
    ".Trashes",
    ".TemporaryItems",
    ".DocumentRevisions-V100",
    ".DS_Store",
    "_Archive",
    "_Snapshots",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Wizard {
    pub kind: Kind,
    pub source: String,
    pub destination: String,
    pub error: Option<String>,
}

impl Wizard {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            source: String::new(),
            destination: String::new(),
            error: None,
        }
    }

    /// Why the job cannot be made yet, or `None` when it can.
    pub fn problem(&self) -> Option<&'static str> {
        if self.source.is_empty() {
            return Some("Choose what to back up.");
        }
        if self.destination.is_empty() {
            return Some("Choose the backup drive.");
        }
        let inside = |inner: &str, outer: &str| {
            inner == outer || inner.starts_with(&format!("{}/", outer.trim_end_matches('/')))
        };
        if inside(&self.destination, &self.source) || inside(&self.source, &self.destination) {
            return Some("The backup drive must be a different place than what you back up.");
        }
        None
    }
}

fn last_part(path: &str) -> String {
    match backup_engine::volumes::last_component(path) {
        "" => "Drive".to_string(),
        name => name.to_string(),
    }
}

/// A source id that is a safe folder name, used as the folder inside the backup drive.
fn folder_id(name: &str) -> String {
    let id: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "source".to_string()
    } else {
        id
    }
}

/// Turns the answers into a job, based on the matching preset.
pub fn build_job(wizard: &Wizard, presets: &[Job]) -> Job {
    let preset = presets
        .iter()
        .find(|preset| preset.id == wizard.kind.preset())
        .cloned()
        .unwrap_or_default();
    let source_name = last_part(&wizard.source);
    let destination_name = last_part(&wizard.destination);
    let mut job = Job {
        id: model::new_id(match wizard.kind {
            Kind::Photos => "photos",
            Kind::Videos => "videos",
            Kind::Lightroom => "lightroom",
            Kind::WholeDrive => "drive",
        }),
        name: format!(
            "{} on {source_name} to {destination_name}",
            wizard.kind.label()
        ),
        description: format!(
            "{}: {source_name} backed up to {destination_name}.",
            wizard.kind.label()
        ),
        destination_kind: "local".to_string(),
        destination_root: wizard.destination.clone(),
        preset: Some(wizard.kind.preset().to_string()),
        sources: vec![Source {
            id: folder_id(&source_name),
            label: source_name.clone(),
            path: wizard.source.clone(),
            category: wizard.kind.category().to_string(),
            kind: Some("folder".into()),
            ..Default::default()
        }],
        last_run_at: None,
        ..preset
    };
    if job.mode.is_empty() {
        job.mode = "safe-mirror".into();
    }
    if job.verification.is_empty() {
        job.verification = "metadata".into();
    }
    if wizard.kind == Kind::WholeDrive {
        job.name = format!("{source_name} to {destination_name}");
        job.parallel_copies = Some(4);
        let filters = job.filters.get_or_insert_with(Vec::new);
        for name in SYSTEM_FOLDERS {
            if !filters.iter().any(|filter| filter.pattern == name) {
                filters.push(Filter {
                    id: format!("system-{}", folder_id(name)),
                    pattern: name.to_string(),
                    action: "exclude".into(),
                    note: "System or backup folder".into(),
                    extra: Default::default(),
                });
            }
        }
    }
    job
}

pub fn kind_of(job: &Job) -> Kind {
    let category = job
        .sources
        .first()
        .map(|source| source.category.as_str())
        .unwrap_or("");
    match (job.preset.as_deref(), category) {
        (_, "catalog") | (Some("lightroom-optimized"), _) => Kind::Lightroom,
        (_, "video") | (Some("video-archive"), _) => Kind::Videos,
        (_, "other") if job.id.starts_with("drive-") => Kind::WholeDrive,
        _ => Kind::Photos,
    }
}

/// A drive or folder choice: connected drives as buttons, plus "Choose folder".
fn place_picker(
    ui: &mut egui::Ui,
    id: &str,
    app_volumes: &[backup_engine::volumes::MountedVolume],
    value: &mut String,
    show_free: bool,
) {
    ui.push_id(id, |ui| {
        ui.horizontal_wrapped(|ui| {
            for volume in app_volumes {
                let active = *value == volume.path;
                let text = if show_free {
                    volume
                        .available
                        .map(|free| format!("{}  ({} free)", volume.name, format_bytes(free)))
                        .unwrap_or_else(|| volume.name.clone())
                } else {
                    volume.name.clone()
                };
                let button = egui::Button::new(RichText::new(text).size(12.0).color(if active {
                    style::PAPER
                } else {
                    style::INK
                }))
                .fill(if active { style::LEAF } else { style::PAPER })
                .stroke(Stroke::new(
                    1.0,
                    if active { style::LEAF } else { style::LINE },
                ))
                .corner_radius(CornerRadius::same(6));
                if ui.add(button).clicked() {
                    *value = volume.path.clone();
                }
            }
            if ui.add(style::ghost_button("Choose folder…")).clicked() {
                if let Some(folder) = pick_folder(Some(value.as_str())) {
                    *value = folder;
                }
            }
        });
        if !value.is_empty() && !app_volumes.iter().any(|volume| volume.path == *value) {
            ui.label(
                RichText::new(value.as_str())
                    .size(11.0)
                    .monospace()
                    .color(style::MUTED),
            );
        }
    });
}

fn step(ui: &mut egui::Ui, number: &str, title: &str) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 11.0, style::LEAF);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            number,
            egui::FontId::new(12.0, style::bold()),
            style::PAPER,
        );
        ui.label(RichText::new(title).size(14.0).family(style::bold()));
    });
}

/// The New backup page.
pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let mut wizard = app
        .wizard
        .clone()
        .unwrap_or_else(|| Wizard::new(Kind::Photos));
    let mut close = false;
    let mut create = false;
    let can_cancel = app.jobs.iter().any(Job::is_configured);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(style::display(if can_cancel {
                "New backup"
            } else {
                "Welcome"
            }));
            ui.label(muted(if can_cancel {
                "Three steps. Nothing is copied until you press Back up now."
            } else {
                "Let's set up your first backup. Three steps, nothing is copied until you say so."
            }));
        });
        if can_cancel {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                if ui.add(style::secondary_button("Cancel")).clicked() {
                    close = true;
                }
            });
        }
    });
    ui.add_space(18.0);
    style::surface()
        .inner_margin(Margin::same(24))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            step(ui, "1", "What do you want to back up?");
            let columns = if ui.available_width() > 700.0 { 4 } else { 2 };
            for chunk in Kind::ALL.chunks(columns) {
                ui.columns(columns, |cols| {
                    for (column, kind) in chunk.iter().enumerate() {
                        let ui = &mut cols[column];
                        let active = wizard.kind == *kind;
                        let response = egui::Frame::new()
                            .fill(if active {
                                style::LEAF_SOFT
                            } else {
                                style::WASH
                            })
                            .stroke(Stroke::new(
                                if active { 2.0 } else { 1.0 },
                                if active { style::LEAF } else { style::LINE },
                            ))
                            .corner_radius(CornerRadius::same(8))
                            .inner_margin(Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    crate::icons::tile(ui, kind.icon(), kind.tone(), 34.0);
                                    ui.label(
                                        RichText::new(kind.label())
                                            .size(14.0)
                                            .family(style::bold()),
                                    );
                                    ui.label(muted(kind.detail()));
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if response.clicked() {
                            wizard.kind = *kind;
                        }
                    }
                });
            }

            step(ui, "2", &format!("From: {}", wizard.kind.source_hint()));
            place_picker(ui, "wizard-source", &app.volumes, &mut wizard.source, false);

            step(ui, "3", "To: the backup drive");
            let backup_drives: Vec<_> = app
                .volumes
                .iter()
                .filter(|volume| volume.path != wizard.source)
                .cloned()
                .collect();
            place_picker(
                ui,
                "wizard-destination",
                &backup_drives,
                &mut wizard.destination,
                true,
            );
            if app.volumes.is_empty() {
                ui.label(muted(
                    "No external drives are connected. Plug in a drive, or choose a folder.",
                ));
            }

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let problem = wizard.problem();
                if ui
                    .add_enabled(problem.is_none(), style::primary_button("Create backup"))
                    .clicked()
                {
                    create = true;
                }
                if let Some(problem) = problem {
                    ui.label(muted(problem));
                } else {
                    ui.label(muted("Ready. You can start it on the next screen."));
                }
            });
            if let Some(error) = &wizard.error {
                ui.label(RichText::new(error).size(12.0).color(style::CORAL));
            }
        });

    ui.add_space(14.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(muted("Something else?"));
        if ui.add(style::text_button("Back up a phone")).clicked() {
            let phone = app.phones.first().cloned();
            app.wizard = None;
            app.create_phone_job(phone);
        }
        if ui
            .add(style::text_button("Back up to a QNAP NAS"))
            .clicked()
        {
            app.wizard = None;
            app.create_qnap_job();
        }
    });

    if close {
        app.wizard = None;
        app.view = crate::app::View::Backup;
    } else if create {
        let job = build_job(&wizard, &model::presets(&app.data_dir));
        app.wizard = None;
        let name = job.name.clone();
        app.selected = job.id.clone();
        app.jobs.push(job);
        app.mark_dirty();
        app.view = crate::app::View::Backup;
        app.tab = crate::app::Tab::Status;
        app.message = format!("{name} is ready. Press Back up now to start.");
    } else if app.wizard.as_ref() != Some(&wizard) {
        app.wizard = Some(wizard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn answers_become_a_runnable_job_from_the_right_preset() {
        let presets = model::presets(Path::new("/nonexistent"));
        let mut wizard = Wizard::new(Kind::Videos);
        assert_eq!(wizard.problem(), Some("Choose what to back up."));
        wizard.source = "/Volumes/Footage".into();
        wizard.destination = "/Volumes/Footage/Backup".into();
        assert!(
            wizard.problem().is_some(),
            "a backup inside its own source is refused"
        );
        wizard.destination = "/Volumes/T7 Shield".into();
        assert_eq!(wizard.problem(), None);

        let job = build_job(&wizard, &presets);
        assert_eq!(job.preset.as_deref(), Some("video-archive"));
        assert_eq!(job.parallel_copies, Some(2));
        assert_eq!(job.sources[0].path, "/Volumes/Footage");
        assert_eq!(job.sources[0].id, "Footage");
        assert_eq!(job.destination_root, "/Volumes/T7 Shield");
        assert!(job.is_configured());
        job.engine_profile().unwrap();
        assert_eq!(kind_of(&job), Kind::Videos);
    }

    #[test]
    fn whole_drive_skips_system_folders_and_lightroom_protects_catalogs() {
        let presets = model::presets(Path::new("/nonexistent"));
        let mut wizard = Wizard {
            kind: Kind::WholeDrive,
            source: "/Volumes/Photo Drive".into(),
            destination: "/Volumes/Backup".into(),
            error: None,
        };
        let job = build_job(&wizard, &presets);
        let filters = job.filters.clone().unwrap();
        assert!(
            filters
                .iter()
                .any(|filter| filter.pattern == ".Spotlight-V100")
        );
        assert_eq!(job.sources[0].id, "Photo-Drive");
        assert_eq!(kind_of(&job), Kind::WholeDrive);
        job.engine_profile().unwrap();

        wizard.kind = Kind::Lightroom;
        let job = build_job(&wizard, &presets);
        assert_eq!(job.sources[0].category, "catalog");
        assert!(
            job.engine_profile()
                .unwrap()
                .guard_enabled("lightroom-closed")
        );
    }
}
