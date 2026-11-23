//! The window. A light sidebar lists your backups (plus Drives and Phone), and the main area shows
//! one thing at a time: the selected backup, the New backup steps, or your drives.

use crate::app::{App, Busy, Tab, View};
use crate::model::Job;
use crate::style::{self, format_bytes, format_count, muted, Tone};
use crate::wizard::{self, Kind, Wizard};
use backup_engine::backup::BackupProgress;
use backup_engine::volumes::MountedVolume;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if app.view == View::Backup && app.selected_job().is_none() {
        app.view = View::NewBackup;
    }
    if app.view == View::Phone && app.phones.is_empty() {
        app.view = View::Backup;
    }

    egui::Panel::left("sidebar")
        .exact_size(250.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(style::SIDEBAR).inner_margin(Margin { left: 16, right: 16, top: 22, bottom: 16 }))
        .show(ui, |ui| sidebar(app, ui));

    egui::CentralPanel::default_margins().frame(egui::Frame::new().fill(style::WASH).inner_margin(Margin::symmetric(40, 30))).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width().min(900.0);
            ui.set_max_width(width);
            match app.view {
                View::Backup => backup_page(app, ui),
                View::NewBackup => wizard::page(app, ui),
                View::Drives => drives_page(app, ui),
                View::Phone => {
                    ui.label(style::display("Phone"));
                    ui.label(muted("Send photos from your computer to an album on your phone."));
                    ui.add_space(18.0);
                    crate::phone::panel(app, ui);
                }
            }
        });
    });
}

// ---- sidebar -------------------------------------------------------------------------------

fn kind_label(job: &Job) -> (&'static str, &'static str, Tone) {
    if job.sources.iter().any(|source| source.is_phone()) {
        return ("Phone", "Mo", Tone::Sky);
    }
    match wizard::kind_of(job) {
        Kind::Photos => ("Photos", "Ph", Tone::Leaf),
        Kind::Videos => ("Videos", "Vi", Tone::Sky),
        Kind::Lightroom => ("Lightroom", "Lr", Tone::Sun),
        Kind::WholeDrive => ("Whole drive", "Dr", Tone::Ink),
    }
}

/// One short line about a backup: running, last backup, or what is missing.
fn status_line(app: &App, job: &Job) -> (String, Color32) {
    if app.run.as_ref().is_some_and(|run| run.job_id == job.id) {
        return ("Backing up now…".to_string(), style::LEAF);
    }
    if !job.is_configured() {
        return ("Needs setting up".to_string(), style::SUN);
    }
    match job.last_run_at {
        Some(seconds) => (format!("Backed up {}", style::relative_time(seconds, crate::model::unix_now())), style::MUTED),
        None => ("Not backed up yet".to_string(), style::SUN),
    }
}

fn nav_row(ui: &mut egui::Ui, active: bool, add: impl FnOnce(&mut egui::Ui)) -> bool {
    let response = egui::Frame::new()
        .fill(if active { style::PAPER } else { Color32::TRANSPARENT })
        .stroke(if active { Stroke::new(1.0, style::LINE) } else { Stroke::NONE })
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        })
        .response
        .interact(egui::Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn sidebar(app: &mut App, ui: &mut egui::Ui) {
    if app.icon.is_none() {
        app.icon = crate::app::load_icon(ui.ctx());
    }
    ui.horizontal(|ui| {
        if let Some(icon) = &app.icon {
            ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(32.0, 32.0)));
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new("DataHaven").size(16.0).family(style::bold()));
            ui.label(RichText::new("Photos, videos and Lightroom").size(11.0).color(style::MUTED));
        });
    });
    ui.add_space(18.0);
    if ui.add(style::primary_button("+  New backup").min_size(egui::vec2(ui.available_width(), 40.0))).clicked() {
        app.open_wizard(Wizard::new(Kind::Photos));
    }
    ui.add_space(18.0);

    ui.label(style::eyebrow("Your backups"));
    ui.add_space(2.0);
    let rows: Vec<(String, String, (&str, &str, Tone), (String, Color32))> = app.jobs.iter().map(|job| (job.id.clone(), job.name.clone(), kind_label(job), status_line(app, job))).collect();
    if rows.is_empty() {
        ui.label(muted("None yet."));
    }
    egui::ScrollArea::vertical().max_height((ui.available_height() - 190.0).max(120.0)).auto_shrink([false, true]).show(ui, |ui| {
        for (id, name, (_, glyph, tone), (status, color)) in rows {
            let active = app.view == View::Backup && app.selected == id;
            let clicked = nav_row(ui, active, |ui| {
                ui.horizontal(|ui| {
                    style::badge_icon(ui, glyph, tone, 30.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(egui::Label::new(RichText::new(&name).size(13.0).family(style::bold())).truncate());
                        ui.add(egui::Label::new(RichText::new(&status).size(11.0).color(color)).truncate());
                    });
                });
            });
            if clicked {
                app.select(&id);
                app.tab = Tab::Status;
            }
        }
    });

    ui.add_space(10.0);
    ui.label(style::eyebrow("Devices"));
    ui.add_space(2.0);
    let drives = app.volumes.len();
    if nav_row(ui, app.view == View::Drives, |ui| {
        ui.horizontal(|ui| {
            style::badge_icon(ui, "D", Tone::Ink, 30.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new("Drives").size(13.0).family(style::bold()));
                ui.label(RichText::new(match drives { 0 => "None connected".to_string(), 1 => "1 connected".to_string(), n => format!("{n} connected") }).size(11.0).color(style::MUTED));
            });
        });
    }) {
        app.view = View::Drives;
    }
    if let Some(device) = app.phones.first().cloned() {
        if nav_row(ui, app.view == View::Phone, |ui| {
            ui.horizontal(|ui| {
                style::badge_icon(ui, "P", Tone::Sky, 30.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new("Phone").size(13.0).family(style::bold()));
                    ui.add(egui::Label::new(RichText::new(format!("{} {}", style::title_case(&device.manufacturer), device.model)).size(11.0).color(style::MUTED)).truncate());
                });
            });
        }) {
            app.view = View::Phone;
        }
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.add(egui::Label::new(RichText::new(&app.message).size(11.0).color(style::MUTED)).wrap());
    });
}

// ---- backup page ---------------------------------------------------------------------------

fn backup_page(app: &mut App, ui: &mut egui::Ui) {
    let Some(job) = app.selected_job().cloned() else { return };
    let (kind, _, _) = kind_label(&job);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 220.0);
            ui.add(egui::Label::new(style::display(&job.name)).truncate());
            ui.label(muted(format!("{kind} backup")));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut tab = app.tab;
            if style::segmented(ui, &mut tab, &[(Tab::Settings, "Settings", true, None), (Tab::Status, "Backup", true, None)]) {
                app.tab = tab;
            }
        });
    });
    ui.add_space(18.0);
    match app.tab {
        Tab::Status => status_tab(app, ui, &job),
        Tab::Settings => crate::editor::job_settings(app, ui),
    }
}

fn place(path: &str, volumes: &[MountedVolume]) -> (String, String, bool) {
    if path.is_empty() {
        return ("Not chosen".to_string(), "Choose it in Settings".to_string(), false);
    }
    if let Some(volume) = volumes.iter().find(|volume| path == volume.path || path.starts_with(&format!("{}/", volume.path))) {
        let inside = path[volume.path.len()..].trim_start_matches('/');
        let detail = if inside.is_empty() { "Whole drive".to_string() } else { inside.to_string() };
        return (volume.name.clone(), detail, true);
    }
    if let Some(rest) = path.strip_prefix("/Volumes/") {
        let name = rest.split('/').next().unwrap_or(rest).to_string();
        return (name, "Not connected".to_string(), false);
    }
    let name = if cfg!(target_os = "macos") { "This Mac" } else { "This computer" };
    (name.to_string(), path.to_string(), true)
}

/// A drive tile in the From → To picture.
fn place_tile(ui: &mut egui::Ui, caption: &str, name: &str, detail: &str, ready: bool, tone: Tone) {
    egui::Frame::new().fill(style::WASH).corner_radius(CornerRadius::same(10)).inner_margin(Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(style::eyebrow(caption));
        ui.horizontal(|ui| {
            style::badge_icon(ui, &name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), tone, 36.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(RichText::new(name).size(15.0).family(style::bold())).truncate());
                ui.add(egui::Label::new(muted(detail)).truncate());
            });
        });
        ui.horizontal(|ui| {
            style::dot(ui, if ready { style::LEAF } else { Color32::from_rgb(0xb9, 0xbd, 0xb6) });
            ui.label(RichText::new(if ready { "Connected" } else { "Not connected" }).size(12.0).color(if ready { style::LEAF } else { style::MUTED }));
        });
    });
}

fn arrow(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 116.0), egui::Sense::hover());
    let center = rect.center();
    let stroke = Stroke::new(2.5, style::LEAF);
    let left = center - egui::vec2(12.0, 0.0);
    let right = center + egui::vec2(12.0, 0.0);
    ui.painter().line_segment([left, right], stroke);
    ui.painter().line_segment([right, right + egui::vec2(-7.0, -7.0)], stroke);
    ui.painter().line_segment([right, right + egui::vec2(-7.0, 7.0)], stroke);
}

/// Why this backup cannot start right now, in words a person can act on.
fn blocker(app: &App, job: &Job) -> Option<String> {
    if !job.is_configured() {
        return Some("Choose what to back up and where, in Settings.".to_string());
    }
    if app.run.is_some() {
        return Some("Another backup is running. This one can start when it is done.".to_string());
    }
    if app.busy.is_some() {
        return Some("One moment…".to_string());
    }
    if !app.job_available(job) {
        if job.sources.iter().any(|source| source.is_phone()) && app.phones.is_empty() {
            return Some("Connect the phone to back up.".to_string());
        }
        let (name, _, ready) = place(&job.destination_root, &app.volumes);
        if !ready && job.destination_kind == "local" {
            return Some(format!("Connect {name} to back up."));
        }
        if let Some(source) = job.sources.iter().find(|source| !source.is_phone() && !place(&source.path, &app.volumes).2) {
            return Some(format!("Connect {} to back up.", place(&source.path, &app.volumes).0));
        }
        return Some("A drive is missing. Connect it to back up.".to_string());
    }
    None
}

fn status_tab(app: &mut App, ui: &mut egui::Ui, job: &Job) {
    let running = app.run.as_ref().filter(|run| run.job_id == job.id).map(|run| run.paused);
    style::surface().inner_margin(Margin::same(24)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let from = match job.sources.as_slice() {
            [] => ("Not chosen".to_string(), "Choose it in Settings".to_string(), false),
            [one] if one.is_phone() => (one.label.clone(), "On the phone".to_string(), !app.phones.is_empty()),
            [one] => place(&one.path, &app.volumes),
            [first, rest @ ..] => {
                let (name, _, ready) = if first.is_phone() { (first.label.clone(), String::new(), !app.phones.is_empty()) } else { place(&first.path, &app.volumes) };
                (name, format!("and {} more", rest.len()), ready)
            }
        };
        let to = match job.destination_kind.as_str() {
            "qnap-rsync" => ("NAS".to_string(), job.destination_root.clone(), true),
            "android" => ("Phone".to_string(), job.destination_root.clone(), !app.phones.is_empty()),
            _ => place(&job.destination_root, &app.volumes),
        };
        let (_, _, tone) = kind_label(job);
        let width = ui.available_width();
        ui.horizontal_top(|ui| {
            let tile = (width - 60.0) / 2.0;
            let down = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(egui::vec2(tile, 0.0), down, |ui| place_tile(ui, "From", &from.0, &from.1, from.2, tone));
            ui.allocate_ui_with_layout(egui::vec2(44.0, 0.0), down, arrow);
            ui.allocate_ui_with_layout(egui::vec2(tile, 0.0), down, |ui| place_tile(ui, "To", &to.0, &to.1, to.2, Tone::Ink));
        });
        ui.add_space(20.0);

        if let Some(paused) = running {
            running_block(app, ui, paused);
            return;
        }

        let failed = app.run_result.as_ref().filter(|result| result.plan.profile_id == job.id).map(|result| result.failed_files).unwrap_or(0);
        let (headline, color) = if failed > 0 {
            (format!("{failed} files could not be backed up"), style::CORAL)
        } else {
            match job.last_run_at {
                Some(seconds) => (format!("Backed up {}", style::relative_time(seconds, crate::model::unix_now())), style::INK),
                None => ("Not backed up yet".to_string(), style::INK),
            }
        };
        ui.horizontal(|ui| {
            if failed == 0 && job.last_run_at.is_some() {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 13.0, style::LEAF_SOFT);
                style::check_mark(ui.painter(), rect.center(), 15.0, style::LEAF);
            }
            ui.label(RichText::new(headline).size(20.0).family(style::bold()).color(color));
        });
        let auto = job.auto_run.as_ref();
        let auto_text = match (auto.and_then(|auto| auto.when_connected).unwrap_or(false), auto.and_then(|auto| auto.every_hours).unwrap_or(0)) {
            (true, 0) => "Runs by itself when the drive is connected.".to_string(),
            (true, hours) => format!("Runs by itself when the drive is connected, and every {}.", hours_text(hours)),
            (false, 0) => "Only runs when you press Back up now.".to_string(),
            (false, hours) => format!("Runs by itself every {} while the drive is connected.", hours_text(hours)),
        };
        ui.label(muted(auto_text));
        ui.add_space(14.0);

        let blocked = blocker(app, job);
        ui.horizontal(|ui| {
            let go = egui::Button::new(RichText::new("Back up now").size(16.0).family(style::bold()).color(style::PAPER)).fill(style::LEAF).corner_radius(CornerRadius::same(10)).min_size(egui::vec2(200.0, 48.0));
            if ui.add_enabled(blocked.is_none(), go).clicked() {
                app.run_backup(&job.id, false);
            }
            let previewing = app.busy == Some(Busy::Plan);
            let preview = style::secondary_button(if previewing { "Checking…" } else { "See what will be copied" }).min_size(egui::vec2(0.0, 48.0));
            if ui.add_enabled(job.is_configured() && app.busy.is_none() && app.run.is_none(), preview).clicked() {
                app.make_plan();
            }
            if previewing {
                ui.spinner();
            }
        });
        if let Some(reason) = blocked {
            ui.label(RichText::new(reason).size(13.0).color(style::SUN));
        }
        if job.is_configured() && auto.and_then(|auto| auto.when_connected) != Some(true) && ui.add(style::text_button("Turn on automatic backups")).clicked() {
            if let Some(stored) = app.jobs.iter_mut().find(|stored| stored.id == job.id) {
                stored.auto_run.get_or_insert_with(Default::default).when_connected = Some(true);
            }
            app.mark_dirty();
            app.message = "Automatic backups are on. It runs when the drive is connected.".to_string();
        }
    });
    ui.add_space(16.0);
    result_block(app, ui, job);
}

fn hours_text(hours: u32) -> String {
    match hours {
        1 => "hour".to_string(),
        24 => "day".to_string(),
        168 => "week".to_string(),
        hours => format!("{hours} hours"),
    }
}

fn running_block(app: &mut App, ui: &mut egui::Ui, paused: bool) {
    let progress = app.progress.clone().unwrap_or_else(|| BackupProgress::control_event(String::new(), "scanning"));
    let fraction = if progress.total_bytes > 0 {
        (progress.copied_bytes as f32 / progress.total_bytes as f32).min(1.0)
    } else if progress.total_files > 0 {
        progress.completed_files as f32 / progress.total_files as f32
    } else {
        0.0
    };
    let scanning = progress.event == "scanning";
    ui.horizontal(|ui| {
        let title = if paused { "Paused".to_string() } else if scanning { "Looking for changes…".to_string() } else { format!("Backing up… {}%", (fraction * 100.0).round()) };
        ui.label(RichText::new(title).size(20.0).family(style::bold()));
        if !paused {
            ui.spinner();
        }
    });
    ui.add_space(6.0);
    style::track(ui, if scanning { 0.0 } else { fraction }, 10.0);
    ui.add_space(4.0);
    let detail = if scanning {
        format!("{} files checked", format_count(progress.discovered_files))
    } else {
        format!(
            "{} of {} files, {} of {}, {}/s",
            format_count(progress.completed_files),
            format_count(progress.total_files),
            format_bytes(progress.copied_bytes),
            format_bytes(progress.total_bytes),
            format_bytes(progress.bytes_per_second)
        )
    };
    ui.label(muted(detail));
    if let Some(path) = &progress.path {
        ui.add(egui::Label::new(RichText::new(path).size(12.0).monospace().color(style::MUTED)).truncate());
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.add(style::secondary_button(if paused { "Resume" } else { "Pause" }).min_size(egui::vec2(110.0, 42.0))).clicked() {
            app.pause_or_resume();
        }
        if ui.add(style::secondary_button("Stop").min_size(egui::vec2(110.0, 42.0))).clicked() {
            app.cancel_run();
        }
        ui.label(muted("A stopped backup continues where it left off next time."));
    });
}

fn result_block(app: &mut App, ui: &mut egui::Ui, job: &Job) {
    let result = app.run_result.as_ref().filter(|result| result.plan.profile_id == job.id);
    let plan = app.plan.as_ref().filter(|plan| plan.profile_id == job.id);
    let Some(plan) = result.map(|result| &result.plan).or(plan) else { return };
    style::surface().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(style::title(if result.is_some() { "Last backup" } else { "What will be copied" }));
        ui.add_space(8.0);
        ui.columns(3, |cols| {
            let number = |ui: &mut egui::Ui, value: String, label: &str| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.label(RichText::new(value).size(22.0).family(style::bold()));
                    ui.label(muted(label));
                });
            };
            match result {
                Some(result) => {
                    number(&mut cols[0], format_count(result.copied_files), "files copied");
                    number(&mut cols[1], format_count(plan.totals.files_skipped), "already up to date");
                    number(&mut cols[2], format_count(result.failed_files), "problems");
                }
                None => {
                    number(&mut cols[0], format_count(plan.totals.files_to_write), "files to copy");
                    number(&mut cols[1], format_bytes(plan.totals.bytes_to_write), "to copy");
                    number(&mut cols[2], format_count(plan.totals.files_skipped), "already up to date");
                }
            }
        });
        let messages: Vec<String> = result.map(|result| result.errors.iter().map(|error| format!("{}{}", error.message, error.path.as_ref().map(|path| format!(" ({path})")).unwrap_or_default())).collect()).unwrap_or_default();
        for message in messages.iter().take(5) {
            ui.label(RichText::new(message).size(12.0).color(style::CORAL));
        }
        for warning in plan.warnings.iter().filter(|warning| warning.level != "info").take(3) {
            ui.label(RichText::new(&warning.message).size(12.0).color(style::SUN));
        }
    });
}

// ---- drives page ---------------------------------------------------------------------------

fn drives_page(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(style::display("Drives"));
            ui.label(muted("Drives connected to this computer. Back one up, or keep your backups on it."));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(app.busy.is_none(), style::secondary_button(if app.busy == Some(Busy::Volumes) { "Refreshing…" } else { "Refresh" })).clicked() {
                app.refresh_volumes();
            }
        });
    });
    ui.add_space(18.0);
    if app.volumes.is_empty() {
        style::surface().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(style::title("No drives connected"));
            ui.label(muted("Plug in a drive. It shows up here within a few seconds."));
        });
    }
    let idle = app.busy.is_none();
    let mut back_up = None;
    let mut keep_here = None;
    let mut check = None;
    for volume in app.volumes.clone() {
        style::surface().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                style::badge_icon(ui, &volume.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), Tone::Ink, 40.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(&volume.name).size(16.0).family(style::bold()));
                    let space = match (volume.capacity, volume.available) {
                        (Some(capacity), Some(free)) => format!("{} free of {}, {}", format_bytes(free), format_bytes(capacity), volume.filesystem.to_uppercase()),
                        (_, Some(free)) => format!("{} free, {}", format_bytes(free), volume.filesystem.to_uppercase()),
                        _ => volume.filesystem.to_uppercase(),
                    };
                    ui.label(muted(space));
                });
            });
            if let (Some(capacity), Some(free)) = (volume.capacity, volume.available) {
                if capacity > 0 {
                    style::track(ui, 1.0 - free as f32 / capacity as f32, 6.0);
                }
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add(style::secondary_button("Back up this drive")).clicked() {
                    back_up = Some(volume.path.clone());
                }
                if ui.add(style::secondary_button("Keep backups here")).clicked() {
                    keep_here = Some(volume.path.clone());
                }
                ui.add_space(12.0);
                ui.label(muted("Check health:"));
                for (level, label) in [("quick", "Quick"), ("medium", "Medium"), ("full", "Full")] {
                    if ui.add_enabled(idle, style::text_button(label)).clicked() {
                        check = Some((volume.path.clone(), level));
                    }
                }
            });
            if let Some(health) = app.health.as_ref().filter(|health| health.path == volume.path) {
                let healthy = health.status == "healthy";
                let (fill, color) = if healthy { (style::LEAF_SOFT, style::LEAF) } else { (style::CORAL_SOFT, style::CORAL) };
                egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(if healthy { "This drive looks healthy" } else { "This drive needs attention" }).family(style::bold()).color(color));
                    ui.label(muted(format!("{} files read ({}) in {:.1} s. {}", format_count(health.files_checked), format_bytes(health.bytes_read), health.duration_ms as f64 / 1000.0, health.note)));
                    for error in &health.errors {
                        ui.label(RichText::new(error).monospace().size(11.0));
                    }
                });
            }
        });
        ui.add_space(10.0);
    }
    if app.busy == Some(Busy::Health) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(muted("Checking the drive…"));
        });
    }
    if let Some(path) = back_up {
        app.open_wizard(Wizard { source: path, ..Wizard::new(Kind::WholeDrive) });
    }
    if let Some(path) = keep_here {
        app.open_wizard(Wizard { destination: path, ..Wizard::new(Kind::Photos) });
    }
    if let Some((path, level)) = check {
        app.check_health(path, level);
    }
}
