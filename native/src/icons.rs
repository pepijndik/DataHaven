//! Small line icons drawn with the painter, so the window needs no icon font or image files.

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Shape, Stroke, StrokeKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Drive,
    Folder,
    Computer,
    Phone,
    Nas,
    Plus,
    Clock,
    Filter,
    Layers,
    Play,
    Gear,
}

/// Draws `icon` centred in `rect`, scaled to its smaller side.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let size = rect.width().min(rect.height());
    let c = rect.center();
    let s = size / 24.0; // the icons are designed on a 24 unit grid
    let p = |x: f32, y: f32| Pos2::new(c.x + (x - 12.0) * s, c.y + (y - 12.0) * s);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(p(x0, y0), p(x1, y1));
    let stroke = Stroke::new((1.6 * s).max(1.2), color);
    let radius = |units: f32| CornerRadius::same((units * s).round().clamp(0.0, 255.0) as u8);
    match icon {
        Icon::Drive => {
            painter.rect_stroke(
                r(3.0, 7.0, 21.0, 17.0),
                radius(2.5),
                stroke,
                StrokeKind::Middle,
            );
            painter.line_segment([p(3.0, 13.0), p(21.0, 13.0)], stroke);
            painter.circle_filled(p(17.5, 15.0), 1.1 * s, color);
        }
        Icon::Folder => {
            let points = vec![
                p(3.0, 6.5),
                p(9.5, 6.5),
                p(11.5, 8.5),
                p(21.0, 8.5),
                p(21.0, 18.5),
                p(3.0, 18.5),
            ];
            painter.add(Shape::closed_line(points, stroke));
        }
        Icon::Computer => {
            painter.rect_stroke(
                r(3.0, 4.5, 21.0, 16.0),
                radius(2.0),
                stroke,
                StrokeKind::Middle,
            );
            painter.line_segment([p(12.0, 16.0), p(12.0, 19.5)], stroke);
            painter.line_segment([p(8.0, 19.5), p(16.0, 19.5)], stroke);
        }
        Icon::Phone => {
            painter.rect_stroke(
                r(7.0, 3.0, 17.0, 21.0),
                radius(2.5),
                stroke,
                StrokeKind::Middle,
            );
            painter.line_segment([p(10.5, 18.0), p(13.5, 18.0)], stroke);
        }
        Icon::Nas => {
            painter.rect_stroke(
                r(4.0, 4.0, 20.0, 11.0),
                radius(2.0),
                stroke,
                StrokeKind::Middle,
            );
            painter.rect_stroke(
                r(4.0, 13.0, 20.0, 20.0),
                radius(2.0),
                stroke,
                StrokeKind::Middle,
            );
            painter.circle_filled(p(16.5, 7.5), 1.0 * s, color);
            painter.circle_filled(p(16.5, 16.5), 1.0 * s, color);
        }
        Icon::Plus => {
            painter.line_segment([p(12.0, 5.0), p(12.0, 19.0)], stroke);
            painter.line_segment([p(5.0, 12.0), p(19.0, 12.0)], stroke);
        }
        Icon::Clock => {
            painter.circle_stroke(p(12.0, 12.0), 8.5 * s, stroke);
            painter.line_segment([p(12.0, 12.0), p(12.0, 7.0)], stroke);
            painter.line_segment([p(12.0, 12.0), p(15.5, 14.0)], stroke);
        }
        Icon::Filter => {
            let points = vec![
                p(4.0, 5.5),
                p(20.0, 5.5),
                p(14.0, 12.5),
                p(14.0, 18.5),
                p(10.0, 20.0),
                p(10.0, 12.5),
            ];
            painter.add(Shape::closed_line(points, stroke));
        }
        Icon::Layers => {
            painter.rect_stroke(
                r(7.0, 4.0, 20.0, 15.0),
                radius(2.0),
                stroke,
                StrokeKind::Middle,
            );
            painter.line_segment([p(4.0, 8.0), p(4.0, 20.0)], stroke);
            painter.line_segment([p(4.0, 20.0), p(16.0, 20.0)], stroke);
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![p(8.0, 5.0), p(19.0, 12.0), p(8.0, 19.0)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Gear => {
            painter.circle_stroke(p(12.0, 12.0), 3.0 * s, stroke);
            for step in 0..8 {
                let angle = step as f32 * std::f32::consts::FRAC_PI_4;
                let (sin, cos) = angle.sin_cos();
                painter.line_segment(
                    [
                        p(12.0 + cos * 5.5, 12.0 + sin * 5.5),
                        p(12.0 + cos * 8.5, 12.0 + sin * 8.5),
                    ],
                    stroke,
                );
            }
        }
    }
}

/// A soft rounded square in the tone's colours with the icon in it. Returns its rectangle.
pub fn tile(ui: &mut egui::Ui, icon: Icon, tone: crate::style::Tone, size: f32) -> Rect {
    let (strong, soft) = tone.colors();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same((size * 0.28) as u8), soft);
    paint(ui.painter(), rect.shrink(size * 0.22), icon, strong);
    rect
}

/// Allocates a square of `size` and draws `icon` in it.
pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint(ui.painter(), rect, icon, color);
    response
}
