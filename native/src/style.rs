//! Colours, fonts and small building blocks for a simple, light look.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle};

// A calm, light palette: white cards on a soft grey window, one green accent.
pub const INK: Color32 = Color32::from_rgb(0x1d, 0x22, 0x1f);
pub const MUTED: Color32 = Color32::from_rgb(0x6b, 0x71, 0x6c);
pub const PAPER: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const WASH: Color32 = Color32::from_rgb(0xf3, 0xf4, 0xf1);
pub const LINE: Color32 = Color32::from_rgb(0xe1, 0xe4, 0xde);
pub const LEAF: Color32 = Color32::from_rgb(0x2f, 0x7d, 0x5b);
pub const LEAF_SOFT: Color32 = Color32::from_rgb(0xe3, 0xf1, 0xea);
pub const SUN: Color32 = Color32::from_rgb(0xb5, 0x76, 0x1c);
pub const SUN_SOFT: Color32 = Color32::from_rgb(0xf8, 0xec, 0xd6);
pub const SKY: Color32 = Color32::from_rgb(0x3d, 0x6e, 0x8c);
pub const SKY_SOFT: Color32 = Color32::from_rgb(0xe2, 0xee, 0xf4);
pub const CORAL: Color32 = Color32::from_rgb(0xb4, 0x4d, 0x3e);
pub const CORAL_SOFT: Color32 = Color32::from_rgb(0xf8, 0xe3, 0xdf);
pub const SIDEBAR: Color32 = Color32::from_rgb(0xe9, 0xeb, 0xe6);

#[derive(Clone, Copy)]
pub enum Tone {
    Sun,
    Leaf,
    Sky,
    Ink,
}

impl Tone {
    pub fn colors(self) -> (Color32, Color32) {
        match self {
            Tone::Sun => (SUN, SUN_SOFT),
            Tone::Leaf => (LEAF, LEAF_SOFT),
            Tone::Sky => (SKY, SKY_SOFT),
            Tone::Ink => (Color32::from_rgb(0x60, 0x65, 0x5d), Color32::from_rgb(0xe4, 0xe4, 0xdc)),
        }
    }
}

/// The bold face used for titles, labels and buttons.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// The serif display face of the big page titles.
pub fn serif() -> FontFamily {
    FontFamily::Name("display".into())
}

pub fn serif_italic() -> FontFamily {
    FontFamily::Name("display-italic".into())
}

/// Uses the fonts of the original (Avenir Next and Baskerville) when the system has them,
/// and egui's built in font otherwise. Every family is always defined, so text never fails to draw.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut load = |key: &str, path: &str, index: u32| -> bool {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = index;
                fonts.font_data.insert(key.to_string(), std::sync::Arc::new(data));
                true
            }
            Err(_) => false,
        }
    };
    let avenir = "/System/Library/Fonts/Avenir Next.ttc";
    let baskerville = "/System/Library/Fonts/Supplemental/Baskerville.ttc";
    let regular = load("avenir-medium", avenir, 5);
    let demi = load("avenir-demi", avenir, 2);
    let serif = load("baskerville", baskerville, 0);
    let serif_italic = load("baskerville-italic", baskerville, 2);

    let base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: Option<&str>| -> Vec<String> { first.map(str::to_string).into_iter().chain(base.iter().cloned()).collect() };
    fonts.families.insert(FontFamily::Proportional, with(regular.then_some("avenir-medium")));
    fonts.families.insert(bold(), with(demi.then_some("avenir-demi")));
    fonts.families.insert(self::serif(), with(serif.then_some("baskerville")));
    fonts.families.insert(self::serif_italic(), with(serif_italic.then_some("baskerville-italic")));
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_visuals(egui::Visuals::light());
    ctx.global_style_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size.y = 34.0;
        let visuals = &mut style.visuals;
        visuals.panel_fill = WASH;
        visuals.window_fill = PAPER;
        visuals.extreme_bg_color = PAPER;
        visuals.faint_bg_color = PAPER;
        visuals.override_text_color = Some(INK);
        visuals.hyperlink_color = LEAF;
        visuals.selection.bg_fill = LEAF_SOFT;
        visuals.selection.stroke = Stroke::new(1.0, LEAF);
        let radius = CornerRadius::same(8);
        for widget in [&mut visuals.widgets.inactive, &mut visuals.widgets.hovered, &mut visuals.widgets.active, &mut visuals.widgets.open, &mut visuals.widgets.noninteractive] {
            widget.corner_radius = radius;
        }
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        visuals.widgets.inactive.bg_fill = PAPER;
        visuals.widgets.inactive.weak_bg_fill = PAPER;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(0xf7, 0xf8, 0xf6);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(0xf7, 0xf8, 0xf6);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xc4, 0xc9, 0xc2));
        visuals.widgets.active.bg_fill = LEAF_SOFT;
        visuals.widgets.active.weak_bg_fill = LEAF_SOFT;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, LEAF);
    });
}

/// A white card with a soft line.
pub fn surface() -> egui::Frame {
    egui::Frame::new().fill(PAPER).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20))
}

/// A small grey label above a value or a section.
pub fn eyebrow(text: &str) -> RichText {
    RichText::new(text).size(12.0).family(bold()).color(MUTED)
}

pub fn title(text: &str) -> RichText {
    RichText::new(text).size(17.0).family(bold()).color(INK)
}

/// The page title.
pub fn display(text: &str) -> RichText {
    RichText::new(text).size(26.0).family(bold()).color(INK)
}

pub fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(13.0).color(MUTED)
}

pub fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).size(14.0).family(bold()).color(PAPER)).fill(LEAF).stroke(Stroke::NONE).corner_radius(CornerRadius::same(8)).min_size(egui::vec2(0.0, 38.0))
}

pub fn secondary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).size(14.0).family(bold()).color(INK)).fill(PAPER).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(8)).min_size(egui::vec2(0.0, 38.0))
}

pub fn ghost_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).size(13.0).color(INK)).fill(PAPER).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(8))
}

pub fn text_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).size(13.0).family(bold()).color(LEAF)).frame(false)
}

pub fn danger_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).size(13.0).family(bold()).color(CORAL)).frame(false)
}

/// A drawn check mark (the built in font has no check glyph).
pub fn check_mark(painter: &egui::Painter, center: egui::Pos2, size: f32, color: Color32) {
    let stroke = Stroke::new(size * 0.16, color);
    let points = [center + egui::vec2(-0.32, 0.02) * size, center + egui::vec2(-0.08, 0.26) * size, center + egui::vec2(0.34, -0.22) * size];
    painter.line_segment([points[0], points[1]], stroke);
    painter.line_segment([points[1], points[2]], stroke);
}

/// A filled dot, used for connected drives and phones.
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// A rounded square with a letter, standing in for the job icons of the original.
pub fn badge_icon(ui: &mut egui::Ui, glyph: &str, tone: Tone, size: f32) {
    let (strong, soft) = tone.colors();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(12), soft);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, FontId::proportional(size * 0.45), strong);
}

/// A thin progress bar in leaf green.
pub fn track(ui: &mut egui::Ui, fraction: f32, height: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let radius = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, radius, Color32::from_rgb(0xe4, 0xe2, 0xda));
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    if filled.width() > 0.0 {
        ui.painter().rect_filled(filled, radius, LEAF);
    }
}

/// A small pill with text.
pub fn chip(ui: &mut egui::Ui, text: &str, fill: Color32, color: Color32) {
    egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
        ui.label(RichText::new(text).size(11.0).family(bold()).color(color));
    });
}

/// A row of mutually exclusive choices, like the segmented controls of the original.
/// Options are (value, label, enabled, hover hint). Returns true when the value changed.
pub fn segmented<T: PartialEq + Clone>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str, bool, Option<&str>)]) -> bool {
    let mut changed = false;
    egui::Frame::new().fill(Color32::from_rgb(0xe8, 0xe5, 0xdc)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(3)).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (option, label, enabled, hint) in options {
                let active = option == value;
                let text = RichText::new(*label).size(12.0).family(bold()).color(if active { INK } else { MUTED });
                let button = egui::Button::new(text).fill(if active { PAPER } else { Color32::TRANSPARENT }).stroke(if active { Stroke::new(1.0, LINE) } else { Stroke::NONE }).corner_radius(CornerRadius::same(6));
                let mut response = ui.add_enabled(*enabled, button);
                if let Some(hint) = hint {
                    response = response.on_hover_text(*hint).on_disabled_hover_text(*hint);
                }
                if response.clicked() && !active {
                    *value = option.clone();
                    changed = true;
                }
            }
        });
    });
    changed
}

/// A switch with a label and an optional explanation. Returns true when it was flipped.
pub fn toggle_row(ui: &mut egui::Ui, on: &mut bool, label: &str, detail: Option<&str>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(label).family(bold()));
            if let Some(detail) = detail {
                ui.label(muted(detail));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, on).changed();
        });
    });
    changed
}

/// An iOS style switch.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(36.0, 20.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let progress = ui.ctx().animate_bool_responsive(response.id, *on);
    let fill = if *on { LEAF } else { Color32::from_rgb(0xc9, 0xc7, 0xbe) };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), progress);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, PAPER);
    response
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let index = ((bytes as f64).ln() / 1024f64.ln()).floor().min((UNITS.len() - 1) as f64) as usize;
    let value = bytes as f64 / 1024f64.powi(index as i32);
    if value >= 10.0 { format!("{value:.0} {}", UNITS[index]) } else { format!("{value:.1} {}", UNITS[index]) }
}

/// Adds thousands separators, like `toLocaleString`.
pub fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub fn title_case(value: &str) -> String {
    value
        .to_lowercase()
        .split(' ')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

/// Local calendar time for unix seconds.
pub fn local_time(seconds: u64) -> LocalTime {
    let time = seconds as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r only writes into `parts`.
    unsafe { libc::localtime_r(&time, &mut parts) };
    LocalTime { year: parts.tm_year + 1900, month: parts.tm_mon as u32 + 1, day: parts.tm_mday as u32, hour: parts.tm_hour as u32, minute: parts.tm_min as u32 }
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago", or a date for older times.
pub fn relative_time(seconds: u64, now: u64) -> String {
    let ago = now.saturating_sub(seconds);
    let plural = |value: u64, unit: &str| format!("{value} {unit}{} ago", if value == 1 { "" } else { "s" });
    match ago {
        0..=59 => "just now".to_string(),
        60..=3_599 => plural(ago / 60, "minute"),
        3_600..=86_399 => plural(ago / 3_600, "hour"),
        86_400..=604_799 => plural(ago / 86_400, "day"),
        _ => format!("on {}", format_date_time(seconds)),
    }
}

/// "8 Oct 2025, 14:05"
pub fn format_date_time(seconds: u64) -> String {
    let time = local_time(seconds);
    format!("{} {} {}, {:02}:{:02}", time.day, &MONTHS[(time.month as usize + 11) % 12][..3], time.year, time.hour, time.minute)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_original() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(50 * 1024 * 1024), "50 MB");
        assert_eq!(format_count(1234567), "1,234,567");
        assert_eq!(format_count(12), "12");
        assert_eq!(title_case("SAMSUNG electronics"), "Samsung Electronics");
        assert_eq!(relative_time(1000, 1030), "just now");
        assert_eq!(relative_time(1000, 1000 + 120), "2 minutes ago");
        assert_eq!(relative_time(1000, 1000 + 3600), "1 hour ago");
        assert_eq!(relative_time(1000, 1000 + 3 * 86_400), "3 days ago");
    }
}
