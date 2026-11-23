//! The colours of the validated design (dark theme). The light theme comes with the Appearance
//! settings.

use egui::Color32;

pub const PANEL: Color32 = Color32::from_rgb(0x1a, 0x1e, 0x23);
pub const PANEL2: Color32 = Color32::from_rgb(0x1f, 0x24, 0x2a);
pub const RAISED: Color32 = Color32::from_rgb(0x25, 0x2b, 0x32);
pub const LINE: Color32 = Color32::from_rgb(0x2e, 0x35, 0x3d);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xeb, 0xef);
pub const MUTED: Color32 = Color32::from_rgb(0xa7, 0xb0, 0xba);
pub const FAINT: Color32 = Color32::from_rgb(0x8c, 0x96, 0xa1);
pub const ACCENT: Color32 = Color32::from_rgb(0x2a, 0xb7, 0xb0);
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0x5e, 0xd6, 0xcf);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x04, 0x20, 0x1e);
pub const TITLE: Color32 = Color32::from_rgb(0x0e, 0x6b, 0x69);
pub const TITLE_TEXT: Color32 = Color32::WHITE;
pub const SEL: Color32 = Color32::from_rgb(0x1c, 0x3a, 0x3b);
pub const OK: Color32 = Color32::from_rgb(0x7d, 0xd7, 0xa1);
pub const WARN: Color32 = Color32::from_rgb(0xf4, 0xc4, 0x6a);
pub const WARN_BG: Color32 = Color32::from_rgb(0x3b, 0x2c, 0x10);
pub const DANGER: Color32 = Color32::from_rgb(0xff, 0x9d, 0x96);

/// An account's colour.
pub fn account(c: mailalm_model::Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// MailALM's look on egui's dark theme.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = RAISED;
    v.window_stroke = egui::Stroke::new(1.0, LINE);
    v.extreme_bg_color = PANEL2;
    v.faint_bg_color = PANEL2;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT_TEXT;
    v.selection.bg_fill = SEL;
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT_TEXT);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.hovered.weak_bg_fill = SEL;
    v.widgets.hovered.bg_fill = SEL;
    v.widgets.active.weak_bg_fill = SEL;
    v.widgets.active.bg_fill = SEL;
    ctx.set_visuals_of(egui::Theme::Dark, v);
}
