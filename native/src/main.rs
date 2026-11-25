//! DataHaven, native window. Built with egui on top of the shared Rust engine:
//! no web view, no JavaScript, so it starts fast and stays light while backups run.

// No console window next to the app on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod editor;
mod history;
mod icons;
mod iphone;
mod model;
mod phone;
mod style;
mod views;
mod wizard;

use eframe::egui;

fn main() -> eframe::Result {
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/window-icon.png"))
            .ok();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("DataHaven")
        .with_inner_size([1240.0, 860.0])
        .with_min_inner_size([900.0, 640.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "DataHaven",
        options,
        Box::new(|creation| {
            style::apply(&creation.egui_ctx);
            Ok(Box::new(app::App::new(creation.egui_ctx.clone())))
        }),
    )
}
