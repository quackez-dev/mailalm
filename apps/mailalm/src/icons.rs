//! Small line icons, drawn with the painter (no image or icon font to embed).
//!
//! Symbols in text need a font that has them. egui's fonts have neither ▾ (U+25BE) nor ✓ (U+2713):
//! they show as empty boxes. Such symbols are drawn here instead.

use egui::{Atom, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Check,
    Minimize,
    Maximize,
    Close,
    Refresh,
    Settings,
    Lock,
    Plus,
}

/// Draws `icon` centred in `rect`.
pub fn paint(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = Stroke::new(1.6, color);
    let line = |a: Pos2, b: Pos2| {
        painter.line_segment([a, b], s);
    };
    match icon {
        Icon::ChevronDown => {
            line(c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0));
            line(c + vec2(0.0, 2.0), c + vec2(4.0, -2.0));
        }
        Icon::ChevronLeft => {
            line(c + vec2(2.0, -4.0), c + vec2(-2.0, 0.0));
            line(c + vec2(-2.0, 0.0), c + vec2(2.0, 4.0));
        }
        Icon::ChevronRight => {
            line(c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0));
            line(c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0));
        }
        Icon::Check => {
            line(c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0));
            line(c + vec2(-1.0, 3.0), c + vec2(4.0, -3.0));
        }
        Icon::Minimize => line(c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)),
        Icon::Maximize => {
            let r = Rect::from_center_size(c, Vec2::splat(9.0));
            line(r.left_top(), r.right_top());
            line(r.right_top(), r.right_bottom());
            line(r.right_bottom(), r.left_bottom());
            line(r.left_bottom(), r.left_top());
        }
        Icon::Close => {
            line(c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0));
            line(c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0));
        }
        Icon::Refresh => {
            // Most of a circle, then the arrow head where it ends.
            let r = 6.0;
            let mut prev = None;
            for step in 0..=14 {
                let a = 0.6 + step as f32 * (5.2 / 14.0);
                let p = c + vec2(a.cos() * r, a.sin() * r);
                if let Some(q) = prev {
                    line(q, p);
                }
                prev = Some(p);
            }
            let tip = c + vec2(0.6f32.cos() * r, 0.6f32.sin() * r);
            line(tip, tip + vec2(-4.0, 0.5));
            line(tip, tip + vec2(0.5, -4.0));
        }
        Icon::Settings => {
            for (dy, knob) in [(-4.0, 2.0), (0.0, -3.0), (4.0, 1.0)] {
                line(c + vec2(-6.0, dy), c + vec2(6.0, dy));
                painter.circle_filled(c + vec2(knob, dy), 1.8, color);
            }
        }
        Icon::Lock => {
            let body = Rect::from_min_size(c + vec2(-4.0, -1.0), vec2(8.0, 6.0));
            painter.rect_filled(body, 1.0, color);
            painter.circle_stroke(c + vec2(0.0, -2.5), 2.6, Stroke::new(1.4, color));
        }
        Icon::Plus => {
            line(c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0));
            line(c + vec2(0.0, -5.0), c + vec2(0.0, 5.0));
        }
    }
}

/// The icon as a piece of a widget's label, e.g. `ui.menu_button(("Move", icons::atom(Icon::ChevronDown)), …)`.
/// It takes the label's text colour.
pub fn atom(icon: Icon) -> Atom<'static> {
    Atom::paint(Vec2::splat(10.0), move |ui, args| paint(ui.painter(), args.rect, icon, args.fallback_text_color))
}

/// A flat icon button with a tooltip. `hover_fill` is drawn under the icon on hover.
pub fn button(ui: &mut Ui, icon: Icon, size: Vec2, color: Color32, hover_fill: Color32, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, hover_fill);
    }
    paint(ui.painter(), rect, icon, color);
    response.on_hover_text(tooltip)
}
