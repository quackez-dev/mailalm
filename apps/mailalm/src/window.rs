//! The window without the system's frame: moving it from the title bar, and resizing it from its
//! edges.

use egui::viewport::ResizeDirection;
use egui::{CursorIcon, Id, Rect, Sense, Ui, ViewportCommand};

/// Width of the band along each edge that resizes the window.
const EDGE: f32 = 5.0;

/// Lets the user resize the window from its edges and corners. Call it last in the frame, so the
/// bands are above everything else.
pub fn resize_edges(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    if maximized {
        return;
    }
    let r = ctx.content_rect();
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let bands = [
        (ResizeDirection::North, Rect::from_min_max(egui::pos2(l + EDGE, t), egui::pos2(rt - EDGE, t + EDGE)), CursorIcon::ResizeVertical),
        (ResizeDirection::South, Rect::from_min_max(egui::pos2(l + EDGE, b - EDGE), egui::pos2(rt - EDGE, b)), CursorIcon::ResizeVertical),
        (ResizeDirection::West, Rect::from_min_max(egui::pos2(l, t + EDGE), egui::pos2(l + EDGE, b - EDGE)), CursorIcon::ResizeHorizontal),
        (ResizeDirection::East, Rect::from_min_max(egui::pos2(rt - EDGE, t + EDGE), egui::pos2(rt, b - EDGE)), CursorIcon::ResizeHorizontal),
        (ResizeDirection::NorthWest, Rect::from_min_max(egui::pos2(l, t), egui::pos2(l + EDGE, t + EDGE)), CursorIcon::ResizeNwSe),
        (ResizeDirection::SouthEast, Rect::from_min_max(egui::pos2(rt - EDGE, b - EDGE), egui::pos2(rt, b)), CursorIcon::ResizeNwSe),
        (ResizeDirection::NorthEast, Rect::from_min_max(egui::pos2(rt - EDGE, t), egui::pos2(rt, t + EDGE)), CursorIcon::ResizeNeSw),
        (ResizeDirection::SouthWest, Rect::from_min_max(egui::pos2(l, b - EDGE), egui::pos2(l + EDGE, b)), CursorIcon::ResizeNeSw),
    ];
    for (n, (dir, rect, cursor)) in bands.into_iter().enumerate() {
        let response = ui.interact(rect, Id::unique(("mailalm-resize", n)), Sense::drag());
        if response.hovered() || response.dragged() {
            ctx.set_cursor_icon(cursor);
        }
        if response.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

/// Moves the window when the title bar is dragged; a double click maximizes or restores it.
pub fn title_bar_drag(ui: &mut Ui, rect: Rect) {
    let response = ui.interact(rect, Id::unique("mailalm-title-drag"), Sense::click_and_drag());
    let ctx = ui.ctx();
    if response.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        toggle_maximized(ctx);
    }
}

pub fn toggle_maximized(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}
