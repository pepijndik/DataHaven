//! The window, laid out around one idea: a backup is a source, an arrow and a destination.
//!
//! A toolbar runs along the top (New backup, History, Send to phone, Drives, and what is running).
//! The sidebar lists the backups with a status mark, the connected drives and the phones. The main
//! area shows the selected backup as a flow you can click: pick the source, pick the destination,
//! and set the schedule, filters and older versions right where they apply. Under the flow, a plan
//! says in plain words what the next run will do, and a bar at the bottom holds "Back up now".

use crate::app::{App, Busy, PhoneTab, Tab, View};
use crate::history::{Outcome, RunRecord};
use crate::icons::{self, Icon};
use crate::model::{AutoRun, Job};
use crate::style::{self, Tone, format_bytes, format_count, muted};
use crate::wizard::{self, Kind, Wizard};
use backup_engine::backup::BackupProgress;
use backup_engine::volumes::{external_drive_name, volume_contains, MountedVolume};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Margin, Pos2, Rect, RichText, Stroke, StrokeKind,
};

const SIDEBAR_WIDTH: f32 = 256.0;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    if app.view == View::Backup && app.selected_job().is_none() {
        app.view = View::NewBackup;
    }
    if app.run.is_some() {
        // Keeps the moving dots on the arrow smooth while a backup runs.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }

    let toolbar = egui::Panel::top("toolbar")
        .exact_size(58.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(style::PAPER).inner_margin(Margin {
            left: 16,
            right: 20,
            top: 6,
            bottom: 6,
        }))
        .show(ui, |ui| toolbar(app, ui));
    let line = toolbar.response.rect;
    ui.painter()
        .hline(line.x_range(), line.bottom(), Stroke::new(1.0, style::LINE));

    egui::Panel::left("sidebar")
        .exact_size(SIDEBAR_WIDTH)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(style::SIDEBAR)
                .inner_margin(Margin {
                    left: 12,
                    right: 12,
                    top: 16,
                    bottom: 12,
                }),
        )
        .show(ui, |ui| sidebar(app, ui));

    if app.view == View::Backup {
        if let Some(job) = app.selected_job().cloned() {
            let bar = egui::Panel::bottom("actions")
                .exact_size(88.0)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(style::PAPER)
                        .inner_margin(Margin::symmetric(32, 14)),
                )
                .show(ui, |ui| action_bar(app, ui, &job));
            let top = bar.response.rect;
            ui.painter()
                .hline(top.x_range(), top.top(), Stroke::new(1.0, style::LINE));
        }
    }

    egui::CentralPanel::default_margins()
        .frame(
            egui::Frame::new()
                .fill(style::WASH)
                .inner_margin(Margin::symmetric(32, 24)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let width = ui.available_width().min(960.0);
                    ui.set_max_width(width);
                    match app.view {
                        View::Backup => backup_page(app, ui),
                        View::NewBackup => wizard::page(app, ui),
                        View::Drives => drives_page(app, ui),
                        View::History => history_page(app, ui),
                        View::Phone => phone_page(app, ui),
                    }
                });
        });
}

// ---- small pieces ----------------------------------------------------------------------------

fn kind_info(job: &Job) -> (&'static str, Icon, Tone) {
    if job.sources.iter().any(|source| source.is_phone()) {
        return ("Phone", Icon::Phone, Tone::Sky);
    }
    match wizard::kind_of(job) {
        Kind::Photos => ("Photos", Icon::Folder, Tone::Leaf),
        Kind::Videos => ("Videos", Icon::Play, Tone::Sky),
        Kind::Lightroom => ("Lightroom", Icon::Layers, Tone::Sun),
        Kind::WholeDrive => ("Whole drive", Icon::Drive, Tone::Ink),
    }
}

use icons::tile as icon_tile;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Running,
    Good,
    Attention,
    Failed,
    Unset,
}

impl Mark {
    fn color(self) -> Color32 {
        match self {
            Mark::Running | Mark::Good => style::LEAF,
            Mark::Attention => style::SUN,
            Mark::Failed => style::CORAL,
            Mark::Unset => Color32::from_rgb(0xb9, 0xbd, 0xb6),
        }
    }
}

/// A small status mark: a check, an exclamation, a cross, or a turning ring while running.
fn paint_mark(painter: &egui::Painter, center: Pos2, radius: f32, mark: Mark, time: f64) {
    let color = mark.color();
    painter.circle_filled(center, radius + 1.5, style::PAPER);
    match mark {
        Mark::Running => {
            painter.circle_stroke(center, radius - 1.0, Stroke::new(1.5, style::LEAF_SOFT));
            let start = (time * 4.0) as f32;
            let points: Vec<Pos2> = (0..=12)
                .map(|step| start + step as f32 * 0.35)
                .map(|angle| center + egui::vec2(angle.cos(), angle.sin()) * (radius - 1.0))
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(2.0, color)));
        }
        Mark::Good => {
            painter.circle_filled(center, radius, color);
            style::check_mark(painter, center, radius * 1.3, style::PAPER);
        }
        Mark::Attention => {
            painter.circle_filled(center, radius, color);
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -radius * 0.5),
                    center + egui::vec2(0.0, radius * 0.1),
                ],
                Stroke::new(radius * 0.28, style::PAPER),
            );
            painter.circle_filled(
                center + egui::vec2(0.0, radius * 0.45),
                radius * 0.15,
                style::PAPER,
            );
        }
        Mark::Failed => {
            painter.circle_filled(center, radius, color);
            let d = radius * 0.4;
            let stroke = Stroke::new(radius * 0.25, style::PAPER);
            painter.line_segment(
                [center + egui::vec2(-d, -d), center + egui::vec2(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(-d, d), center + egui::vec2(d, -d)],
                stroke,
            );
        }
        Mark::Unset => {
            painter.circle_stroke(center, radius - 1.0, Stroke::new(1.5, color));
        }
    }
}

/// How a backup is doing, as a mark and one short line.
fn job_status(app: &App, job: &Job) -> (Mark, String) {
    if app.run.as_ref().is_some_and(|run| run.job_id == job.id) {
        let percent = app
            .progress
            .as_ref()
            .filter(|progress| progress.total_bytes > 0)
            .map(|progress| {
                format!(
                    " {}%",
                    (progress.copied_bytes * 100 / progress.total_bytes).min(100)
                )
            })
            .unwrap_or_default();
        return (Mark::Running, format!("Backing up now…{percent}"));
    }
    if !job.is_configured() {
        return (Mark::Unset, "Needs setting up".to_string());
    }
    if let Some(record) = app.last_run(&job.id) {
        let when = style::relative_time(record.started_at, crate::model::unix_now());
        return match record.outcome() {
            Outcome::Success => (Mark::Good, format!("Backed up {when}")),
            Outcome::Warning => (
                Mark::Attention,
                format!("{} problems, {when}", record.failed_files),
            ),
            Outcome::Stopped => (Mark::Attention, format!("Stopped {when}")),
            Outcome::Failed => (Mark::Failed, format!("Failed {when}")),
        };
    }
    match job.last_run_at {
        Some(seconds) => (
            Mark::Good,
            format!(
                "Backed up {}",
                style::relative_time(seconds, crate::model::unix_now())
            ),
        ),
        None => (Mark::Attention, "Not backed up yet".to_string()),
    }
}

/// A clickable row with a hover and selected state. Returns the click response.
fn row(
    ui: &mut egui::Ui,
    active: bool,
    height: f32,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let fill = if active {
        style::PAPER
    } else if response.hovered() {
        Color32::from_rgba_unmultiplied(255, 255, 255, 120)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(9),
        fill,
        if active {
            Stroke::new(1.0, style::LINE)
        } else {
            Stroke::NONE
        },
        StrokeKind::Inside,
    );
    if active {
        let bar = Rect::from_min_size(
            rect.left_top() + egui::vec2(0.0, 10.0),
            egui::vec2(3.0, height - 20.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), style::LEAF);
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(10.0, 6.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.style_mut().interaction.selectable_labels = false;
    add(&mut child);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn two_lines(ui: &mut egui::Ui, top: &str, bottom: &str, bottom_color: Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add(egui::Label::new(RichText::new(top).size(13.0).family(style::bold())).truncate());
        ui.add(egui::Label::new(RichText::new(bottom).size(11.0).color(bottom_color)).truncate());
    });
}

/// Paints one line of text, cut with "…" when it is wider than `max_width`.
fn paint_text(
    painter: &egui::Painter,
    anchor: Pos2,
    align: egui::Align2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width);
    let galley = painter.layout_job(job);
    let rect = align.anchor_size(anchor, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        format_count(count),
        if count == 1 { one } else { many }
    )
}

fn duration_text(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3_599 => format!("{} min {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} min", seconds / 3_600, (seconds % 3_600) / 60),
    }
}

// ---- toolbar ---------------------------------------------------------------------------------

/// An icon above a short label, like a classic Mac toolbar.
fn tool_button(ui: &mut egui::Ui, icon: Icon, label: &str, active: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(78.0, 46.0), egui::Sense::click());
    let fill = if active {
        style::LEAF_SOFT
    } else if response.hovered() {
        style::WASH
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    let color = if active { style::LEAF } else { style::INK };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(
            rect.center_top() + egui::vec2(0.0, 15.0),
            egui::vec2(20.0, 20.0),
        ),
        icon,
        color,
    );
    paint_text(
        ui.painter(),
        rect.center_bottom() - egui::vec2(0.0, 5.0),
        egui::Align2::CENTER_BOTTOM,
        label,
        FontId::new(11.0, style::bold()),
        color,
        rect.width(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    if app.icon.is_none() {
        app.icon = crate::app::load_icon(ui.ctx());
    }
    ui.horizontal_centered(|ui| {
        ui.set_width(SIDEBAR_WIDTH - 16.0);
        if let Some(icon) = &app.icon {
            ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(28.0, 28.0)));
        }
        ui.label(RichText::new("DataHaven").size(16.0).family(style::bold()));
    });
    let full = ui.max_rect();
    let mut tools = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                full.min + egui::vec2(SIDEBAR_WIDTH - 4.0, 0.0),
                full.max,
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut tools;
    ui.spacing_mut().item_spacing.x = 2.0;
    if tool_button(ui, Icon::Plus, "New backup", app.view == View::NewBackup) {
        app.open_wizard(Wizard::new(Kind::Photos));
    }
    if tool_button(ui, Icon::Clock, "History", app.view == View::History) {
        app.view = View::History;
    }
    if tool_button(ui, Icon::Phone, "Send to phone", app.view == View::Phone) {
        if !app.phones.is_empty() {
            app.phone_tab = PhoneTab::Android;
        }
        app.view = View::Phone;
    }
    if tool_button(ui, Icon::Drive, "Drives", app.view == View::Drives) {
        app.view = View::Drives;
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        activity(app, ui)
    });
}

/// What the app is doing right now, at the right of the toolbar.
fn activity(app: &mut App, ui: &mut egui::Ui) {
    let time = ui.input(|input| input.time);
    let (mark, title, detail) = if let Some(run) = &app.run {
        let name = app
            .jobs
            .iter()
            .find(|job| job.id == run.job_id)
            .map(|job| job.name.clone())
            .unwrap_or_default();
        let detail = match &app.progress {
            Some(progress) if progress.event != "scanning" && progress.total_bytes > 0 => format!(
                "{}% · {}/s",
                (progress.copied_bytes * 100 / progress.total_bytes).min(100),
                format_bytes(progress.bytes_per_second)
            ),
            _ => "Looking for changes…".to_string(),
        };
        (
            if run.paused {
                Mark::Attention
            } else {
                Mark::Running
            },
            if run.paused {
                format!("Paused: {name}")
            } else {
                name
            },
            detail,
        )
    } else if app.phone.sending || app.iphone.sending {
        (
            Mark::Running,
            "Sending to phone".to_string(),
            "Photos are being copied".to_string(),
        )
    } else {
        let marks: Vec<Mark> = app
            .jobs
            .iter()
            .filter(|job| job.is_configured())
            .map(|job| job_status(app, job).0)
            .collect();
        let attention = marks
            .iter()
            .filter(|mark| matches!(mark, Mark::Attention | Mark::Failed))
            .count();
        match (marks.len(), attention) {
            (0, _) => (
                Mark::Unset,
                "No backups yet".to_string(),
                "Make one with New backup".to_string(),
            ),
            (_, 0) => (
                Mark::Good,
                "All backups are fine".to_string(),
                "Nothing running".to_string(),
            ),
            (_, 1) => (
                Mark::Attention,
                "1 backup needs a look".to_string(),
                "Nothing running".to_string(),
            ),
            (_, count) => (
                Mark::Attention,
                format!("{count} backups need a look"),
                "Nothing running".to_string(),
            ),
        }
    };
    let pill = ui.allocate_ui_with_layout(
        egui::vec2(280.0, 44.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            egui::Frame::new()
                .fill(style::WASH)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(256.0);
                    ui.style_mut().interaction.selectable_labels = false;
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                        paint_mark(ui.painter(), rect.center(), 8.0, mark, time);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.set_max_width(220.0);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add(
                                egui::Label::new(
                                    RichText::new(title).size(12.0).family(style::bold()),
                                )
                                .truncate(),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(detail).size(11.0).color(style::MUTED),
                                )
                                .truncate(),
                            );
                        });
                    });
                })
        },
    );
    if let Some(run) = &app.run {
        let job_id = run.job_id.clone();
        if pill
            .inner
            .response
            .interact(egui::Sense::click())
            .on_hover_text("Show this backup")
            .clicked()
        {
            app.select(&job_id);
            app.tab = Tab::Status;
        }
    }
}

// ---- sidebar ---------------------------------------------------------------------------------

fn section(ui: &mut egui::Ui, title: &str, count: Option<usize>) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(
            RichText::new(title.to_uppercase())
                .size(10.5)
                .family(style::bold())
                .color(style::MUTED),
        );
        if let Some(count) = count {
            ui.label(
                RichText::new(count.to_string())
                    .size(10.5)
                    .color(style::MUTED),
            );
        }
    });
}

fn sidebar(app: &mut App, ui: &mut egui::Ui) {
    let time = ui.input(|input| input.time);
    ui.spacing_mut().item_spacing.y = 2.0;
    section(ui, "Backups", Some(app.jobs.len()));
    let rows: Vec<(String, String, Icon, Tone, Mark, String)> = app
        .jobs
        .iter()
        .map(|job| {
            let (_, icon, tone) = kind_info(job);
            let (mark, line) = job_status(app, job);
            (job.id.clone(), job.name.clone(), icon, tone, mark, line)
        })
        .collect();
    if rows.is_empty() {
        ui.label(muted("   None yet."));
    }
    egui::ScrollArea::vertical()
        .id_salt("sidebar-jobs")
        .max_height((ui.available_height() - 250.0).max(140.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (id, name, icon, tone, mark, line) in rows {
                let active = matches!(app.view, View::Backup)
                    && app.selected_job().is_some_and(|job| job.id == id);
                let clicked = row(ui, active, 50.0, |ui| {
                    let tile = icon_tile(ui, icon, tone, 32.0);
                    paint_mark(
                        ui.painter(),
                        tile.right_bottom() - egui::vec2(2.0, 2.0),
                        6.0,
                        mark,
                        time,
                    );
                    ui.add_space(4.0);
                    let color = match mark {
                        Mark::Attention => style::SUN,
                        Mark::Failed => style::CORAL,
                        Mark::Running => style::LEAF,
                        _ => style::MUTED,
                    };
                    two_lines(ui, &name, &line, color);
                })
                .clicked();
                if clicked {
                    app.select(&id);
                    app.tab = Tab::Status;
                }
            }
        });

    ui.add_space(8.0);
    section(ui, "Drives", Some(app.volumes.len()));
    if app.volumes.is_empty() {
        ui.label(muted("   None connected"));
    }
    for volume in app.volumes.clone() {
        let free = match (volume.capacity, volume.available) {
            (Some(capacity), Some(free)) => {
                format!("{} free of {}", format_bytes(free), format_bytes(capacity))
            }
            (_, Some(free)) => format!("{} free", format_bytes(free)),
            _ => volume.filesystem.to_uppercase(),
        };
        let used = match (volume.capacity, volume.available) {
            (Some(capacity), Some(free)) if capacity > 0 => {
                Some(1.0 - free as f32 / capacity as f32)
            }
            _ => None,
        };
        if row(ui, false, 40.0, |ui| {
            icons::show(ui, Icon::Drive, 22.0, style::MUTED);
            ui.add_space(2.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(
                    egui::Label::new(RichText::new(&volume.name).size(12.5).family(style::bold()))
                        .truncate(),
                );
                ui.add(
                    egui::Label::new(RichText::new(&free).size(10.5).color(style::MUTED))
                        .truncate(),
                );
                if let Some(used) = used {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width().min(150.0), 3.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(2),
                        Color32::from_rgb(0xd6, 0xd9, 0xd2),
                    );
                    let mut filled = rect;
                    filled.set_width(rect.width() * used.clamp(0.0, 1.0));
                    ui.painter().rect_filled(
                        filled,
                        CornerRadius::same(2),
                        if used > 0.9 { style::SUN } else { style::SKY },
                    );
                }
            });
        })
        .clicked()
        {
            app.view = View::Drives;
        }
    }

    ui.add_space(8.0);
    section(ui, "Phones", None);
    let android_line = match app.phones.first() {
        Some(device) => format!(
            "{} {}",
            style::title_case(&device.manufacturer),
            device.model
        ),
        None => "Plug in to send photos".to_string(),
    };
    let android_ready = !app.phones.is_empty();
    if row(
        ui,
        app.view == View::Phone && app.phone_tab == PhoneTab::Android,
        40.0,
        |ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
            icons::paint(
                ui.painter(),
                rect,
                Icon::Phone,
                if android_ready {
                    style::LEAF
                } else {
                    style::MUTED
                },
            );
            ui.add_space(2.0);
            two_lines(
                ui,
                "Android",
                &android_line,
                if android_ready {
                    style::LEAF
                } else {
                    style::MUTED
                },
            );
        },
    )
    .clicked()
    {
        app.phone_tab = PhoneTab::Android;
        app.view = View::Phone;
    }
    let iphone_line = crate::iphone::status_line(app);
    if row(
        ui,
        app.view == View::Phone && app.phone_tab == PhoneTab::Iphone,
        40.0,
        |ui| {
            icons::show(ui, Icon::Phone, 22.0, style::MUTED);
            ui.add_space(2.0);
            two_lines(ui, "iPhone (Photos)", &iphone_line, style::MUTED);
        },
    )
    .clicked()
    {
        app.phone_tab = PhoneTab::Iphone;
        app.view = View::Phone;
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.add(egui::Label::new(RichText::new(&app.message).size(11.0).color(style::MUTED)).wrap());
    });
}

// ---- the backup page -------------------------------------------------------------------------

fn backup_page(app: &mut App, ui: &mut egui::Ui) {
    let Some(index) = app.selected_index() else {
        return;
    };
    let job = app.jobs[index].clone();
    let (kind, icon, tone) = kind_info(&job);
    ui.horizontal(|ui| {
        icon_tile(ui, icon, tone, 44.0);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 230.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut name = job.name.clone();
            let edit = egui::TextEdit::singleline(&mut name)
                .font(FontId::new(24.0, style::bold()))
                .frame(egui::Frame::NONE)
                .desired_width(ui.available_width());
            if ui.add(edit).on_hover_text("Click to rename").changed() {
                app.update_job(&job.id, |stored| stored.name = name);
            }
            ui.label(muted(format!("{kind} backup")));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut tab = app.tab;
            if style::segmented(
                ui,
                &mut tab,
                &[
                    (Tab::Settings, "All settings", true, None),
                    (Tab::Status, "Overview", true, None),
                ],
            ) {
                app.tab = tab;
            }
        });
    });
    ui.add_space(16.0);
    match app.tab {
        Tab::Status => {
            flow_card(app, ui, &job);
            ui.add_space(14.0);
            plan_card(app, ui, &job);
            ui.add_space(14.0);
            last_run_card(app, ui, &job);
        }
        Tab::Settings => crate::editor::job_settings(app, ui),
    }
}

/// Where a path lives: (place name, detail, connected).
fn place(path: &str, volumes: &[MountedVolume]) -> (String, String, bool) {
    if path.is_empty() {
        return ("Choose…".to_string(), "Not chosen yet".to_string(), false);
    }
    if let Some(volume) = volumes
        .iter()
        .find(|volume| volume_contains(&volume.path, path))
    {
        let inside = path.get(volume.path.len()..).unwrap_or("").trim_start_matches(['/', '\\']);
        let detail = if inside.is_empty() {
            "Whole drive".to_string()
        } else {
            inside.to_string()
        };
        return (volume.name.clone(), detail, true);
    }
    if let Some(name) = external_drive_name(path) {
        return (name, "Not connected".to_string(), false);
    }
    let name = if cfg!(target_os = "macos") {
        "This Mac"
    } else {
        "This computer"
    };
    (name.to_string(), path.to_string(), true)
}

struct Endpoint {
    icon: Icon,
    name: String,
    detail: String,
    ready: bool,
    space: Option<String>,
}

fn source_endpoint(app: &App, job: &Job) -> Endpoint {
    let from_path = |path: &str| {
        let (name, detail, ready) = place(path, &app.volumes);
        let on_drive = external_drive_name(path).is_some();
        let icon = if path.is_empty() {
            Icon::Folder
        } else if on_drive {
            Icon::Drive
        } else {
            Icon::Computer
        };
        (icon, name, detail, ready)
    };
    let (icon, name, detail, ready) = match job.sources.as_slice() {
        [] => from_path(""),
        [one] if one.is_phone() => (
            Icon::Phone,
            one.label.clone(),
            format!("On the phone: {}", one.path),
            !app.phones.is_empty(),
        ),
        [one] => from_path(&one.path),
        [first, rest @ ..] => {
            let (icon, name, _, ready) = if first.is_phone() {
                (
                    Icon::Phone,
                    "Phone".to_string(),
                    String::new(),
                    !app.phones.is_empty(),
                )
            } else {
                from_path(&first.path)
            };
            (
                icon,
                name,
                format!("{} and {} more", first.label, rest.len()),
                ready,
            )
        }
    };
    Endpoint {
        icon,
        name,
        detail,
        ready,
        space: None,
    }
}

fn destination_endpoint(app: &App, job: &Job) -> Endpoint {
    match job.destination_kind.as_str() {
        "qnap-rsync" => {
            let host = job
                .qnap
                .as_ref()
                .map(|qnap| qnap.host.clone())
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| "NAS".to_string());
            Endpoint {
                icon: Icon::Nas,
                name: host,
                detail: job.destination_root.clone(),
                ready: true,
                space: None,
            }
        }
        "android" => Endpoint {
            icon: Icon::Phone,
            name: "Phone".to_string(),
            detail: job.destination_root.clone(),
            ready: !app.phones.is_empty(),
            space: None,
        },
        _ => {
            let (name, detail, ready) = place(&job.destination_root, &app.volumes);
            let volume = app
                .volumes
                .iter()
                .find(|volume| volume_contains(&volume.path, &job.destination_root));
            let space = volume.and_then(|volume| {
                volume
                    .available
                    .map(|free| format!("{} free", format_bytes(free)))
            });
            let icon = if external_drive_name(&job.destination_root).is_some() {
                Icon::Drive
            } else {
                Icon::Folder
            };
            Endpoint {
                icon,
                name,
                detail,
                ready,
                space,
            }
        }
    }
}

/// The big clickable source or destination. Returns its response, to hang a menu on.
fn endpoint_tile(
    ui: &mut egui::Ui,
    caption: &str,
    endpoint: &Endpoint,
    tone: Tone,
    height: f32,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    let stroke = if hovered {
        Stroke::new(1.5, style::LEAF)
    } else {
        Stroke::new(1.0, style::LINE)
    };
    painter.rect(
        rect,
        CornerRadius::same(14),
        if hovered {
            Color32::from_rgb(0xfa, 0xfb, 0xf9)
        } else {
            style::WASH
        },
        stroke,
        StrokeKind::Inside,
    );
    paint_text(
        painter,
        rect.left_top() + egui::vec2(16.0, 14.0),
        egui::Align2::LEFT_TOP,
        caption,
        FontId::new(11.0, style::bold()),
        style::MUTED,
        200.0,
    );
    // A small chevron says "click to choose"; the word appears on hover.
    let chevron_color = if hovered { style::LEAF } else { style::MUTED };
    let tip = rect.right_top() + egui::vec2(-20.0, 21.0);
    painter.add(egui::Shape::line(
        vec![
            tip + egui::vec2(-4.0, -2.5),
            tip + egui::vec2(0.0, 1.5),
            tip + egui::vec2(4.0, -2.5),
        ],
        Stroke::new(1.6, chevron_color),
    ));
    if hovered {
        paint_text(
            painter,
            rect.right_top() + egui::vec2(-30.0, 14.0),
            egui::Align2::RIGHT_TOP,
            "Change",
            FontId::new(11.0, style::bold()),
            chevron_color,
            120.0,
        );
    }

    let (strong, soft) = tone.colors();
    let disk = Rect::from_center_size(
        rect.center_top() + egui::vec2(0.0, 66.0),
        egui::vec2(76.0, 76.0),
    );
    painter.rect_filled(
        disk,
        CornerRadius::same(22),
        if endpoint.ready {
            soft
        } else {
            Color32::from_rgb(0xea, 0xeb, 0xe7)
        },
    );
    icons::paint(
        painter,
        disk.shrink(16.0),
        endpoint.icon,
        if endpoint.ready { strong } else { style::MUTED },
    );
    let text_width = rect.width() - 32.0;
    paint_text(
        painter,
        rect.center_top() + egui::vec2(0.0, 114.0),
        egui::Align2::CENTER_TOP,
        &endpoint.name,
        FontId::new(17.0, style::bold()),
        style::INK,
        text_width,
    );
    paint_text(
        painter,
        rect.center_top() + egui::vec2(0.0, 138.0),
        egui::Align2::CENTER_TOP,
        &endpoint.detail,
        FontId::new(12.0, egui::FontFamily::Proportional),
        style::MUTED,
        text_width,
    );
    let state = match (&endpoint.space, endpoint.ready) {
        (Some(space), true) => format!("Connected · {space}"),
        (None, true) => "Connected".to_string(),
        (_, false) if endpoint.name == "Choose…" => "Click to choose".to_string(),
        (_, false) => "Not connected".to_string(),
    };
    let color = if endpoint.ready {
        style::LEAF
    } else {
        style::SUN
    };
    let text_rect = paint_text(
        painter,
        rect.center_top() + egui::vec2(6.0, 160.0),
        egui::Align2::CENTER_TOP,
        &state,
        FontId::new(11.5, style::bold()),
        color,
        text_width - 20.0,
    );
    painter.circle_filled(text_rect.left_center() - egui::vec2(8.0, 0.0), 3.5, color);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// The arrow between source and destination. Dots travel along it while this backup runs.
fn flow_arrow(ui: &mut egui::Ui, width: f32, running: Option<bool>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 40.0), egui::Sense::hover());
    let painter = ui.painter();
    let y = rect.center().y;
    let left = rect.left() + 6.0;
    let right = rect.right() - 6.0;
    let color = if running.is_some() {
        style::LEAF
    } else {
        Color32::from_rgb(0xa9, 0xb3, 0xab)
    };
    painter.line_segment(
        [Pos2::new(left, y), Pos2::new(right, y)],
        Stroke::new(2.0, color),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(right + 2.0, y),
            Pos2::new(right - 9.0, y - 6.5),
            Pos2::new(right - 9.0, y + 6.5),
        ],
        color,
        Stroke::NONE,
    ));
    if let Some(paused) = running {
        let time = ui.input(|input| input.time) as f32;
        let span = right - left - 12.0;
        for step in 0..3 {
            let offset = if paused {
                step as f32 / 3.0
            } else {
                ((time * 0.6) + step as f32 / 3.0).fract()
            };
            painter.circle_filled(Pos2::new(left + offset * span, y), 4.0, style::LEAF);
        }
    }
}

/// A small pill button under the flow (schedule, filters, older versions).
fn option_chip(
    ui: &mut egui::Ui,
    icon: Icon,
    title: &str,
    value: &str,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        CornerRadius::same(10),
        style::PAPER,
        Stroke::new(1.0, if hovered { style::LEAF } else { style::LINE }),
        StrokeKind::Inside,
    );
    let icon_rect = Rect::from_center_size(
        rect.left_center() + egui::vec2(20.0, 0.0),
        egui::vec2(18.0, 18.0),
    );
    icons::paint(
        ui.painter(),
        icon_rect,
        icon,
        if hovered { style::LEAF } else { style::MUTED },
    );
    let text_left = rect.left() + 38.0;
    let max = rect.right() - text_left - 10.0;
    paint_text(
        ui.painter(),
        Pos2::new(text_left, rect.top() + 7.0),
        egui::Align2::LEFT_TOP,
        title,
        FontId::new(10.5, style::bold()),
        style::MUTED,
        max,
    );
    paint_text(
        ui.painter(),
        Pos2::new(text_left, rect.top() + 22.0),
        egui::Align2::LEFT_TOP,
        value,
        FontId::new(12.5, style::bold()),
        style::INK,
        max,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn menu_item(ui: &mut egui::Ui, icon: Icon, title: &str, detail: &str) -> bool {
    let response = row(ui, false, 40.0, |ui| {
        icons::show(ui, icon, 20.0, style::MUTED);
        ui.add_space(2.0);
        two_lines(ui, title, detail, style::MUTED);
    });
    response.clicked()
}

fn schedule_text(auto: Option<&AutoRun>) -> String {
    let connected = auto.and_then(|auto| auto.when_connected).unwrap_or(false);
    let every = auto.and_then(|auto| auto.every_hours).unwrap_or(0);
    let repeat = match every {
        1 => "hourly".to_string(),
        24 => "daily".to_string(),
        168 => "weekly".to_string(),
        hours => format!("every {hours} h"),
    };
    match (connected, every) {
        (false, 0) => "Manually".to_string(),
        (true, 0) => "When connected".to_string(),
        (false, _) => style::title_case(&repeat),
        (true, _) => format!("When connected, {repeat}"),
    }
}

fn versions_text(job: &Job) -> String {
    let keep = match job.archive_keep_days {
        Some(days) if days >= 365 && days % 365 == 0 => {
            format!("{} year{}", days / 365, if days == 365 { "" } else { "s" })
        }
        Some(days) => format!("{days} days"),
        None => "forever".to_string(),
    };
    if job.mode == "append-only" {
        "Off".to_string()
    } else if job.versioning.as_deref() == Some("snapshots") {
        format!("Snapshots, {keep}")
    } else {
        format!("Archive, {keep}")
    }
}

fn filter_text(job: &Job) -> String {
    let rules = job
        .filters
        .as_ref()
        .map(|filters| filters.len())
        .unwrap_or(0);
    match rules {
        0 => "Everything".to_string(),
        1 => "1 rule".to_string(),
        count => format!("{count} rules"),
    }
}

enum FlowAction {
    Source(String),
    SourceFolder,
    Destination(String),
    DestinationFolder,
    OpenSettings { advanced: bool },
}

fn flow_card(app: &mut App, ui: &mut egui::Ui, job: &Job) {
    let running = app
        .run
        .as_ref()
        .filter(|run| run.job_id == job.id)
        .map(|run| run.paused);
    let source = source_endpoint(app, job);
    let destination = destination_endpoint(app, job);
    let (_, _, tone) = kind_info(job);
    if matches!(job.destination_kind.as_str(), "local" | "qnap-smb") {
        app.destination(&job.destination_root.clone());
    }
    let snapshots_possible = app
        .destinations
        .get(&job.destination_root)
        .and_then(|state| state.info.as_ref())
        .map(|info| info.snapshots_supported)
        .unwrap_or(true);
    let mut action = None;
    let mut job_after = job.clone();

    style::surface()
        .inner_margin(Margin::same(22))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let width = ui.available_width();
            let middle = 150.0;
            let side = (width - middle - 2.0 * ui.spacing().item_spacing.x) / 2.0;
            let down = egui::Layout::top_down(egui::Align::Center);
            ui.horizontal_top(|ui| {
                // Source
                ui.allocate_ui_with_layout(egui::vec2(side, 0.0), down, |ui| {
                    let response = endpoint_tile(ui, "SOURCE", &source, tone, 186.0);
                    let several =
                        job.sources.len() > 1 || job.sources.iter().any(|source| source.is_phone());
                    egui::Popup::menu(&response)
                        .width(side.min(320.0))
                        .show(|ui| {
                            ui.label(style::eyebrow("Back up from"));
                            if several {
                                ui.label(muted("This backup has several sources or a phone."));
                                if menu_item(ui, Icon::Gear, "Edit the sources", "In All settings")
                                {
                                    action = Some(FlowAction::OpenSettings { advanced: false });
                                }
                                return;
                            }
                            for volume in &app.volumes {
                                if menu_item(ui, Icon::Drive, &volume.name, "The whole drive") {
                                    action = Some(FlowAction::Source(volume.path.clone()));
                                }
                            }
                            if menu_item(
                                ui,
                                Icon::Folder,
                                "Choose a folder…",
                                "Anywhere on this computer",
                            ) {
                                action = Some(FlowAction::SourceFolder);
                            }
                        });
                    ui.add_space(8.0);
                    let filters =
                        option_chip(ui, Icon::Filter, "WHAT'S INCLUDED", &filter_text(job), side);
                    if filters
                        .on_hover_text("Choose files and folders to skip")
                        .clicked()
                    {
                        action = Some(FlowAction::OpenSettings { advanced: true });
                    }
                });
                // The arrow and the schedule
                ui.allocate_ui_with_layout(egui::vec2(middle, 0.0), down, |ui| {
                    ui.add_space(66.0);
                    flow_arrow(ui, middle, running);
                    ui.add_space(14.0);
                    let response = option_chip(
                        ui,
                        Icon::Clock,
                        "RUNS",
                        &schedule_text(job.auto_run.as_ref()),
                        middle,
                    );
                    egui::Popup::menu(&response)
                        .width(300.0)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            ui.label(style::eyebrow("When should it run?"));
                            let auto = job_after.auto_run.get_or_insert_with(AutoRun::default);
                            let mut connected = auto.when_connected.unwrap_or(false);
                            if style::toggle_row(
                                ui,
                                &mut connected,
                                "When the drive is connected",
                                Some("Starts by itself as soon as it is plugged in."),
                            ) {
                                auto.when_connected = Some(connected);
                            }
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("Repeat while connected")
                                    .size(12.0)
                                    .family(style::bold())
                                    .color(style::MUTED),
                            );
                            let mut hours = auto.every_hours.unwrap_or(0);
                            if style::segmented(
                                ui,
                                &mut hours,
                                &[
                                    (0, "Off", true, None),
                                    (1, "Hourly", true, None),
                                    (24, "Daily", true, None),
                                    (168, "Weekly", true, None),
                                ],
                            ) {
                                auto.every_hours = (hours > 0).then_some(hours);
                            }
                            if job_after.auto_run == Some(AutoRun::default()) {
                                job_after.auto_run = None;
                            }
                        });
                });
                // Destination
                ui.allocate_ui_with_layout(egui::vec2(side, 0.0), down, |ui| {
                    let response = endpoint_tile(ui, "DESTINATION", &destination, Tone::Ink, 186.0);
                    egui::Popup::menu(&response)
                        .width(side.min(320.0))
                        .show(|ui| {
                            ui.label(style::eyebrow("Back up to"));
                            for volume in &app.volumes {
                                let free = volume
                                    .available
                                    .map(|free| format!("{} free", format_bytes(free)))
                                    .unwrap_or_else(|| volume.filesystem.to_uppercase());
                                if menu_item(ui, Icon::Drive, &volume.name, &free) {
                                    action = Some(FlowAction::Destination(volume.path.clone()));
                                }
                            }
                            if menu_item(
                                ui,
                                Icon::Folder,
                                "Choose a folder…",
                                "A folder on a drive or this computer",
                            ) {
                                action = Some(FlowAction::DestinationFolder);
                            }
                            if menu_item(ui, Icon::Nas, "Phone or NAS…", "Set up in All settings")
                            {
                                action = Some(FlowAction::OpenSettings { advanced: false });
                            }
                        });
                    ui.add_space(8.0);
                    let response = option_chip(
                        ui,
                        Icon::Layers,
                        "OLDER VERSIONS",
                        &versions_text(job),
                        side,
                    );
                    egui::Popup::menu(&response)
                        .width(320.0)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .show(|ui| {
                            ui.label(style::eyebrow("When a file changes, keep the old one?"));
                            let current = if job_after.mode == "append-only" {
                                "off"
                            } else if job_after.versioning.as_deref() == Some("snapshots")
                                && snapshots_possible
                            {
                                "snapshots"
                            } else {
                                "archive"
                            };
                            let mut value = current.to_string();
                            let hint = if snapshots_possible {
                                None
                            } else {
                                Some("Needs an APFS drive")
                            };
                            if style::segmented(
                                ui,
                                &mut value,
                                &[
                                    ("archive".to_string(), "Archive folder", true, None),
                                    (
                                        "snapshots".to_string(),
                                        "Snapshots",
                                        snapshots_possible,
                                        hint,
                                    ),
                                    ("off".to_string(), "Don't keep", true, None),
                                ],
                            ) {
                                job_after.mode = if value == "off" {
                                    "append-only".into()
                                } else {
                                    "safe-mirror".into()
                                };
                                job_after.versioning = Some(if value == "snapshots" {
                                    "snapshots".into()
                                } else {
                                    "archive".into()
                                });
                            }
                            if value != "off" {
                                ui.label(
                                    RichText::new("Keep them for")
                                        .size(12.0)
                                        .family(style::bold())
                                        .color(style::MUTED),
                                );
                                let mut days = job_after.archive_keep_days.unwrap_or(0);
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
                                    job_after.archive_keep_days = (days > 0).then_some(days);
                                }
                            }
                        });
                });
            });
        });

    if job_after != *job {
        app.update_job(&job.id, |stored| *stored = job_after);
    }
    match action {
        Some(FlowAction::Source(path)) => app.set_source(&job.id, path),
        Some(FlowAction::SourceFolder) => {
            let start = job.sources.first().map(|source| source.path.clone());
            if let Some(path) = crate::app::pick_folder(start.as_deref()) {
                app.set_source(&job.id, path);
            }
        }
        Some(FlowAction::Destination(path)) => app.set_destination(&job.id, path),
        Some(FlowAction::DestinationFolder) => {
            if let Some(path) = crate::app::pick_folder(Some(&job.destination_root)) {
                app.set_destination(&job.id, path);
            }
        }
        Some(FlowAction::OpenSettings { advanced }) => {
            app.tab = Tab::Settings;
            app.editor = crate::app::EditorState {
                job_id: job.id.clone(),
                advanced_open: advanced,
                ..Default::default()
            };
        }
        None => {}
    }
}

/// A filter pattern as a person would say it: `**/Cache/**` becomes "Cache", `*.tmp` becomes ".tmp files".
fn friendly_pattern(pattern: &str) -> String {
    let trimmed = pattern
        .trim_start_matches("**/")
        .trim_end_matches("/**")
        .trim_end_matches('/');
    if let Some(extension) = trimmed.strip_prefix("*.") {
        return format!(".{extension} files");
    }
    let name = trimmed.trim_matches('*');
    if name.is_empty() {
        pattern.to_string()
    } else {
        name.to_string()
    }
}

/// What the next run will do, in plain sentences.
pub fn plan_lines(app: &App, job: &Job) -> Vec<(Icon, String)> {
    let mut lines = Vec::new();
    if !job.is_configured() {
        lines.push((
            Icon::Gear,
            "Choose a source and a destination above. Then this explains what the backup will do."
                .to_string(),
        ));
        return lines;
    }
    let source = source_endpoint(app, job);
    let destination = destination_endpoint(app, job);
    let from = if source.detail.is_empty() || source.detail == "Whole drive" {
        source.name.clone()
    } else {
        format!("{} ({})", source.name, source.detail)
    };
    let to = if destination.detail == "Whole drive" {
        destination.name.clone()
    } else {
        format!("{} ({})", destination.name, destination.detail)
    };
    let copy = if job.layout() == "mirror" {
        format!("Copies new and changed files from {from} to {to}, keeping the same folders.")
    } else {
        format!("Copies new and changed files from {from} to {to}, each source in its own folder.")
    };
    lines.push((Icon::Play, copy));
    lines.push((Icon::Drive, "Files that haven't changed are skipped. Files you delete from the source stay in the backup.".to_string()));
    let keep = match job.archive_keep_days {
        Some(days) => format!("for {days} days"),
        None => "for good".to_string(),
    };
    let versions = if job.mode == "append-only" {
        "Changed files are overwritten; older versions are not kept.".to_string()
    } else if job.versioning.as_deref() == Some("snapshots") {
        format!(
            "After each run, a snapshot of the whole backup is saved in _Snapshots and kept {keep}."
        )
    } else {
        format!(
            "Before a file is replaced, the old version is moved to the _Archive folder and kept {keep}."
        )
    };
    lines.push((Icon::Layers, versions));
    let check = match job.verification.as_str() {
        "full-hash" => "Every copied file is read back and compared with a checksum.",
        "important-hash" => {
            "Important files, like catalogs, are read back and compared with a checksum."
        }
        _ => "Files are compared by size and date, which is fast.",
    };
    lines.push((Icon::Filter, check.to_string()));
    let skipped: Vec<String> = job
        .filters
        .iter()
        .flatten()
        .filter(|filter| filter.action == "exclude")
        .map(|filter| friendly_pattern(&filter.pattern))
        .collect();
    if !skipped.is_empty() {
        let examples = skipped
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if skipped.len() > 3 {
            format!(" and {} more", skipped.len() - 3)
        } else {
            String::new()
        };
        lines.push((Icon::Filter, format!("Skips {examples}{more}.")));
    }
    if job.guard("lightroom-closed") {
        lines.push((
            Icon::Layers,
            "Waits until Lightroom is closed, so the catalog is copied in one piece.".to_string(),
        ));
    }
    let schedule = match schedule_text(job.auto_run.as_ref()).as_str() {
        "Manually" => "Runs when you press Back up now.".to_string(),
        "When connected" => format!("Runs by itself when {} is connected.", destination.name),
        other => format!("Runs by itself: {}.", other.to_lowercase()),
    };
    lines.push((Icon::Clock, schedule));
    if let Some(plan) = app.plan.as_ref().filter(|plan| plan.profile_id == job.id) {
        lines.push((
            Icon::Play,
            format!(
                "Right now that means {} ({}) to copy and {} already up to date.",
                plural(plan.totals.files_to_write, "file", "files"),
                format_bytes(plan.totals.bytes_to_write),
                format_count(plan.totals.files_skipped)
            ),
        ));
    }
    lines
}

fn plan_card(app: &mut App, ui: &mut egui::Ui, job: &Job) {
    let lines = plan_lines(app, job);
    style::surface().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(style::title("What happens when it runs"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let previewing = app.busy == Some(Busy::Plan);
                if previewing {
                    ui.spinner();
                }
                let label = if previewing {
                    "Counting…"
                } else {
                    "Count the changes"
                };
                if ui
                    .add_enabled(
                        job.is_configured() && app.busy.is_none() && app.run.is_none(),
                        style::ghost_button(label),
                    )
                    .on_hover_text("Looks for changes without copying anything")
                    .clicked()
                {
                    app.make_plan();
                }
            });
        });
        ui.add_space(6.0);
        for (number, (icon, text)) in lines.iter().enumerate() {
            ui.horizontal_top(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 11.0, style::WASH);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    (number + 1).to_string(),
                    FontId::new(11.0, style::bold()),
                    style::MUTED,
                );
                let _ = icon;
                ui.add(egui::Label::new(RichText::new(text).size(13.5)).wrap());
            });
        }
        if let Some(plan) = app.plan.as_ref().filter(|plan| plan.profile_id == job.id) {
            for warning in plan
                .warnings
                .iter()
                .filter(|warning| warning.level != "info")
                .take(3)
            {
                ui.label(RichText::new(&warning.message).size(12.0).color(style::SUN));
            }
        }
    });
}

fn last_run_card(app: &mut App, ui: &mut egui::Ui, job: &Job) {
    let record = app.last_run(&job.id).cloned();
    let errors: Vec<String> = app
        .run_result
        .as_ref()
        .filter(|result| result.plan.profile_id == job.id)
        .map(|result| {
            result
                .errors
                .iter()
                .map(|error| {
                    format!(
                        "{}{}",
                        error.message,
                        error
                            .path
                            .as_ref()
                            .map(|path| format!(" ({path})"))
                            .unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let Some(record) = record else {
        // Runs from before the history existed still show their numbers.
        if let Some(result) = app
            .run_result
            .as_ref()
            .filter(|result| result.plan.profile_id == job.id)
        {
            style::surface().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(style::title("Last backup"));
                numbers(
                    ui,
                    &[
                        (format_count(result.copied_files), "files copied"),
                        (
                            format_count(result.plan.totals.files_skipped),
                            "already up to date",
                        ),
                        (format_count(result.failed_files), "problems"),
                    ],
                );
            });
        }
        return;
    };
    let mut show_all = false;
    style::surface().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
            paint_mark(
                ui.painter(),
                rect.center(),
                10.0,
                outcome_mark(record.outcome()),
                0.0,
            );
            ui.label(style::title("Last backup"));
            ui.label(muted(format!(
                "{}, took {}{}",
                style::format_date_time(record.started_at),
                duration_text(record.duration_seconds),
                if record.automatic {
                    ", started by itself"
                } else {
                    ""
                }
            )));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                show_all = ui.add(style::text_button("All runs")).clicked();
            });
        });
        ui.add_space(4.0);
        if let Some(error) = &record.error {
            ui.label(RichText::new(error).size(12.5).color(style::CORAL));
            return;
        }
        numbers(
            ui,
            &[
                (format_count(record.copied_files), "files copied"),
                (format_bytes(record.copied_bytes), "written"),
                (format_count(record.skipped_files), "already up to date"),
                (format_count(record.failed_files), "problems"),
            ],
        );
        for message in errors.iter().take(5) {
            ui.label(RichText::new(message).size(12.0).color(style::CORAL));
        }
    });
    if show_all {
        app.history_only_selected = true;
        app.view = View::History;
    }
}

fn numbers(ui: &mut egui::Ui, values: &[(String, &str)]) {
    ui.columns(values.len(), |cols| {
        for (col, (value, label)) in cols.iter_mut().zip(values) {
            col.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new(value).size(21.0).family(style::bold()));
                ui.label(muted(*label));
            });
        }
    });
}

fn outcome_mark(outcome: Outcome) -> Mark {
    match outcome {
        Outcome::Success => Mark::Good,
        Outcome::Warning | Outcome::Stopped => Mark::Attention,
        Outcome::Failed => Mark::Failed,
    }
}

// ---- the bottom bar --------------------------------------------------------------------------

/// Why this backup cannot start right now, in words a person can act on.
fn blocker(app: &App, job: &Job) -> Option<String> {
    if !job.is_configured() {
        return Some("Choose a source and a destination first.".to_string());
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
        if let Some(source) = job
            .sources
            .iter()
            .find(|source| !source.is_phone() && !place(&source.path, &app.volumes).2)
        {
            return Some(format!(
                "Connect {} to back up.",
                place(&source.path, &app.volumes).0
            ));
        }
        return Some("A drive is missing. Connect it to back up.".to_string());
    }
    None
}

fn action_bar(app: &mut App, ui: &mut egui::Ui, job: &Job) {
    let time = ui.input(|input| input.time);
    if let Some(paused) = app
        .run
        .as_ref()
        .filter(|run| run.job_id == job.id)
        .map(|run| run.paused)
    {
        running_bar(app, ui, paused);
        return;
    }
    let (mark, line) = job_status(app, job);
    let blocked = blocker(app, job);
    ui.horizontal_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
        paint_mark(ui.painter(), rect.center(), 13.0, mark, time);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width() - 360.0);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(
                egui::Label::new(RichText::new(&line).size(16.0).family(style::bold())).truncate(),
            );
            let sub = blocked.clone().unwrap_or_else(|| {
                match schedule_text(job.auto_run.as_ref()).as_str() {
                    "Manually" => "Only runs when you press Back up now.".to_string(),
                    other => format!("Also runs by itself: {}.", other.to_lowercase()),
                }
            });
            ui.add(
                egui::Label::new(RichText::new(sub).size(12.5).color(if blocked.is_some() {
                    style::SUN
                } else {
                    style::MUTED
                }))
                .truncate(),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let go = egui::Button::new(
                RichText::new("Back up now")
                    .size(16.0)
                    .family(style::bold())
                    .color(style::PAPER),
            )
            .fill(style::LEAF)
            .corner_radius(CornerRadius::same(10))
            .min_size(egui::vec2(190.0, 50.0));
            if ui.add_enabled(blocked.is_none(), go).clicked() {
                app.run_backup(&job.id, false);
            }
            let previewing = app.busy == Some(Busy::Plan);
            let preview =
                style::secondary_button(if previewing { "Counting…" } else { "Preview" })
                    .min_size(egui::vec2(110.0, 50.0));
            if ui
                .add_enabled(
                    job.is_configured() && app.busy.is_none() && app.run.is_none(),
                    preview,
                )
                .on_hover_text("See what would be copied, without copying")
                .clicked()
            {
                app.make_plan();
            }
        });
    });
}

fn running_bar(app: &mut App, ui: &mut egui::Ui, paused: bool) {
    let progress = app
        .progress
        .clone()
        .unwrap_or_else(|| BackupProgress::control_event(String::new(), "scanning"));
    let fraction = if progress.total_bytes > 0 {
        (progress.copied_bytes as f32 / progress.total_bytes as f32).min(1.0)
    } else if progress.total_files > 0 {
        progress.completed_files as f32 / progress.total_files as f32
    } else {
        0.0
    };
    let scanning = progress.event == "scanning";
    ui.horizontal_centered(|ui| {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 250.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            let title = if paused {
                "Paused".to_string()
            } else if scanning {
                "Looking for changes…".to_string()
            } else {
                format!("Backing up… {}%", (fraction * 100.0).round())
            };
            let detail = if scanning {
                format!("{} files checked", format_count(progress.discovered_files))
            } else {
                format!(
                    "{} of {} files · {} of {} · {}/s",
                    format_count(progress.completed_files),
                    format_count(progress.total_files),
                    format_bytes(progress.copied_bytes),
                    format_bytes(progress.total_bytes),
                    format_bytes(progress.bytes_per_second)
                )
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(15.0).family(style::bold()));
                ui.label(muted(detail));
            });
            style::track(ui, if scanning { 0.0 } else { fraction }, 8.0);
            if let Some(path) = &progress.path {
                ui.add(
                    egui::Label::new(
                        RichText::new(path)
                            .size(11.0)
                            .monospace()
                            .color(style::MUTED),
                    )
                    .truncate(),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(style::secondary_button("Stop").min_size(egui::vec2(100.0, 44.0)))
                .on_hover_text("A stopped backup continues where it left off next time")
                .clicked()
            {
                app.cancel_run();
            }
            if ui
                .add(
                    style::secondary_button(if paused { "Resume" } else { "Pause" })
                        .min_size(egui::vec2(100.0, 44.0)),
                )
                .clicked()
            {
                app.pause_or_resume();
            }
        });
    });
}

// ---- history ---------------------------------------------------------------------------------

const HISTORY_COLUMNS: [(&str, f32); 6] = [
    ("", 28.0),
    ("Backup", 0.0),
    ("Started", 150.0),
    ("Took", 90.0),
    ("Copied", 110.0),
    ("Result", 130.0),
];

fn history_row(ui: &mut egui::Ui, cells: impl FnOnce(&mut egui::Ui, usize, f32) + Copy) {
    let total = ui.available_width();
    let fixed: f32 = HISTORY_COLUMNS.iter().map(|(_, width)| width).sum();
    let flexible = (total - fixed - 12.0).max(120.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (index, (_, width)) in HISTORY_COLUMNS.iter().enumerate() {
            let width = if *width == 0.0 { flexible } else { *width };
            ui.allocate_ui_with_layout(
                egui::vec2(width, 34.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_width(width);
                    cells(ui, index, width);
                },
            );
        }
    });
}

fn history_page(app: &mut App, ui: &mut egui::Ui) {
    let selected = app
        .selected_job()
        .map(|job| (job.id.clone(), job.name.clone()));
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(style::display("History"));
            ui.label(muted("Every backup run, newest first."));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some((_, name)) = &selected {
                let mut only = app.history_only_selected;
                let label = format!("Only {name}");
                if style::segmented(
                    ui,
                    &mut only,
                    &[
                        (true, label.as_str(), true, None),
                        (false, "All backups", true, None),
                    ],
                ) {
                    app.history_only_selected = only;
                }
            }
        });
    });
    ui.add_space(16.0);
    let filter = selected
        .filter(|_| app.history_only_selected)
        .map(|(id, _)| id);
    let records: Vec<RunRecord> = app
        .history
        .iter()
        .filter(|record| filter.as_ref().is_none_or(|id| &record.job_id == id))
        .take(200)
        .cloned()
        .collect();

    let copied: u64 = records.iter().map(|record| record.copied_bytes).sum();
    let problems = records
        .iter()
        .filter(|record| record.outcome() != Outcome::Success)
        .count();
    if !records.is_empty() {
        ui.horizontal(|ui| {
            for (value, label) in [
                (records.len().to_string(), "runs"),
                (format_bytes(copied), "written"),
                (problems.to_string(), "with problems"),
            ] {
                egui::Frame::new()
                    .fill(style::PAPER)
                    .stroke(Stroke::new(1.0, style::LINE))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(value).size(18.0).family(style::bold()));
                            ui.label(muted(label));
                        });
                    });
            }
        });
        ui.add_space(12.0);
    }

    let mut open = None;
    style::surface()
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if records.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    icons::show(ui, Icon::Clock, 36.0, style::MUTED);
                    ui.label(style::title("No runs yet"));
                    ui.label(muted(
                        "Each time a backup runs, it is listed here with what it copied.",
                    ));
                });
                ui.add_space(20.0);
                return;
            }
            history_row(ui, |ui, index, _| {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(HISTORY_COLUMNS[index].0.to_uppercase())
                        .size(10.5)
                        .family(style::bold())
                        .color(style::MUTED),
                );
            });
            let line = ui.cursor().top();
            ui.painter()
                .hline(ui.max_rect().x_range(), line, Stroke::new(1.0, style::LINE));
            let time = ui.input(|input| input.time);
            for record in &records {
                let top = ui.cursor().top();
                let response = ui.interact(
                    Rect::from_min_size(
                        Pos2::new(ui.max_rect().left(), top),
                        egui::vec2(ui.available_width(), 38.0),
                    ),
                    ui.scope_id()
                        .with(("history-run", &record.job_id, record.started_at)),
                    egui::Sense::click(),
                );
                if response.hovered() {
                    ui.painter()
                        .rect_filled(response.rect, CornerRadius::same(6), style::WASH);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                history_row(ui, |ui, index, width| {
                    ui.style_mut().interaction.selectable_labels = false;
                    ui.add_space(6.0);
                    match index {
                        0 => {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            paint_mark(
                                ui.painter(),
                                rect.center(),
                                7.5,
                                outcome_mark(record.outcome()),
                                time,
                            );
                        }
                        1 => {
                            ui.set_max_width(width - 12.0);
                            let route = format!("{}  to  {}", record.source, record.destination);
                            two_lines(ui, &record.job_name, &route, style::MUTED);
                        }
                        2 => {
                            two_lines(
                                ui,
                                &style::format_date_time(record.started_at),
                                if record.automatic {
                                    "Started by itself"
                                } else {
                                    "Started by you"
                                },
                                style::MUTED,
                            );
                        }
                        3 => {
                            ui.label(
                                RichText::new(duration_text(record.duration_seconds)).size(12.5),
                            );
                        }
                        4 => {
                            two_lines(
                                ui,
                                &format_bytes(record.copied_bytes),
                                &plural(record.copied_files, "file", "files"),
                                style::MUTED,
                            );
                        }
                        _ => {
                            let (text, color) = match record.outcome() {
                                Outcome::Success => ("Done".to_string(), style::LEAF),
                                Outcome::Warning => {
                                    (format!("{} problems", record.failed_files), style::SUN)
                                }
                                Outcome::Stopped => ("Stopped".to_string(), style::SUN),
                                Outcome::Failed => ("Failed".to_string(), style::CORAL),
                            };
                            let label = ui.label(
                                RichText::new(text)
                                    .size(12.5)
                                    .family(style::bold())
                                    .color(color),
                            );
                            if let Some(error) = &record.error {
                                label.on_hover_text(error);
                            }
                        }
                    }
                });
                if response.clicked() {
                    open = Some(record.job_id.clone());
                }
            }
        });
    if let Some(id) = open.filter(|id| app.jobs.iter().any(|job| &job.id == id)) {
        app.select(&id);
        app.tab = Tab::Status;
    }
}

// ---- send to phone ---------------------------------------------------------------------------

fn phone_page(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(style::display("Send to phone"));
            ui.label(muted(
                "One click puts the new photos of a folder into an album on your phone.",
            ));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut tab = app.phone_tab;
            if style::segmented(
                ui,
                &mut tab,
                &[
                    (PhoneTab::Iphone, "iPhone", true, None),
                    (PhoneTab::Android, "Android", true, None),
                ],
            ) {
                app.phone_tab = tab;
            }
        });
    });
    ui.add_space(16.0);
    match app.phone_tab {
        PhoneTab::Android => crate::phone::panel(app, ui),
        PhoneTab::Iphone => crate::iphone::panel(app, ui),
    }
}

// ---- drives ----------------------------------------------------------------------------------

fn drives_page(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(style::display("Drives"));
            ui.label(muted("Drives connected to this computer. Back one up, keep your backups on it, or check its health."));
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
            ui.label(muted(
                "Plug in a drive. It shows up here within a few seconds.",
            ));
        });
    }
    let idle = app.busy.is_none();
    let mut back_up = None;
    let mut keep_here = None;
    let mut check = None;
    for volume in app.volumes.clone() {
        let used_by: Vec<String> = app
            .jobs
            .iter()
            .filter(|job| volume_contains(&volume.path, &job.destination_root))
            .map(|job| job.name.clone())
            .collect();
        style::surface().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                icon_tile(ui, Icon::Drive, Tone::Ink, 44.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(&volume.name).size(16.0).family(style::bold()));
                    let space = match (volume.capacity, volume.available) {
                        (Some(capacity), Some(free)) => format!(
                            "{} free of {}, {}",
                            format_bytes(free),
                            format_bytes(capacity),
                            volume.filesystem.to_uppercase()
                        ),
                        (_, Some(free)) => format!(
                            "{} free, {}",
                            format_bytes(free),
                            volume.filesystem.to_uppercase()
                        ),
                        _ => volume.filesystem.to_uppercase(),
                    };
                    ui.label(muted(space));
                    if !used_by.is_empty() {
                        ui.label(
                            RichText::new(format!("Holds: {}", used_by.join(", ")))
                                .size(12.0)
                                .color(style::LEAF),
                        );
                    }
                });
            });
            if let (Some(capacity), Some(free)) = (volume.capacity, volume.available) {
                if capacity > 0 {
                    style::track(ui, 1.0 - free as f32 / capacity as f32, 6.0);
                }
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(style::secondary_button("Back up this drive"))
                    .clicked()
                {
                    back_up = Some(volume.path.clone());
                }
                if ui
                    .add(style::secondary_button("Keep backups here"))
                    .clicked()
                {
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
            if let Some(health) = app
                .health
                .as_ref()
                .filter(|health| health.path == volume.path)
            {
                let healthy = health.status == "healthy";
                let (fill, color) = if healthy {
                    (style::LEAF_SOFT, style::LEAF)
                } else {
                    (style::CORAL_SOFT, style::CORAL)
                };
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(if healthy {
                                "This drive looks healthy"
                            } else {
                                "This drive needs attention"
                            })
                            .family(style::bold())
                            .color(color),
                        );
                        ui.label(muted(format!(
                            "{} files read ({}) in {:.1} s. {}",
                            format_count(health.files_checked),
                            format_bytes(health.bytes_read),
                            health.duration_ms as f64 / 1000.0,
                            health.note
                        )));
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
        app.open_wizard(Wizard {
            source: path,
            ..Wizard::new(Kind::WholeDrive)
        });
    }
    if let Some(path) = keep_here {
        app.open_wizard(Wizard {
            destination: path,
            ..Wizard::new(Kind::Photos)
        });
    }
    if let Some((path, level)) = check {
        app.check_health(path, level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_versions_read_plainly() {
        assert_eq!(schedule_text(None), "Manually");
        assert_eq!(
            schedule_text(Some(&AutoRun {
                when_connected: Some(true),
                every_hours: None
            })),
            "When connected"
        );
        assert_eq!(
            schedule_text(Some(&AutoRun {
                when_connected: Some(true),
                every_hours: Some(24)
            })),
            "When connected, daily"
        );
        assert_eq!(
            schedule_text(Some(&AutoRun {
                when_connected: None,
                every_hours: Some(168)
            })),
            "Weekly"
        );
        let job = Job {
            mode: "safe-mirror".into(),
            archive_keep_days: Some(30),
            ..Default::default()
        };
        assert_eq!(versions_text(&job), "Archive, 30 days");
        assert_eq!(
            versions_text(&Job {
                mode: "append-only".into(),
                ..Default::default()
            }),
            "Off"
        );
        assert_eq!(duration_text(75), "1 min 15 s");
        assert_eq!(
            friendly_pattern("**/*Previews.lrdata/**"),
            "Previews.lrdata"
        );
        assert_eq!(friendly_pattern("**/Cache/**"), "Cache");
        assert_eq!(friendly_pattern("*.tmp"), ".tmp files");
    }

    #[test]
    fn plan_names_source_destination_and_schedule() {
        let ctx = egui::Context::default();
        let app = crate::app::tests::sample_app(&ctx, "plan");
        let job = app.jobs[0].clone();
        let lines: Vec<String> = plan_lines(&app, &job)
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        assert!(
            lines[0].contains("/Users/me/Pictures/Lightroom") && lines[0].contains("T7 Shield"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("press Back up now")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("312 files")),
            "the counted changes are included: {lines:?}"
        );
    }
}
