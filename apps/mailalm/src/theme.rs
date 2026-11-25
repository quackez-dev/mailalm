//! The colours of the validated design, dark and light (Settings, Appearance: dark, light, or as
//! Windows is set). Each colour is a function: it gives the colour of the theme shown now, which
//! [`sync`] reads from egui at the start of every frame.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Color32, Theme, ThemePreference};
use serde::{Deserialize, Serialize};

use crate::i18n::tr;

/// The theme the user chose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    /// As Windows is set (dark or light apps).
    System,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];

    /// Its name, in the interface's language.
    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Dark => tr("Dark"),
            ThemeChoice::Light => tr("Light"),
            ThemeChoice::System => tr("As Windows is set"),
        }
    }
}

/// Every colour of one theme.
struct Palette {
    panel: Color32,
    panel2: Color32,
    raised: Color32,
    line: Color32,
    text: Color32,
    muted: Color32,
    faint: Color32,
    accent: Color32,
    accent_text: Color32,
    on_accent: Color32,
    title: Color32,
    title_text: Color32,
    sel: Color32,
    ok: Color32,
    warn: Color32,
    warn_bg: Color32,
    danger: Color32,
    /// Behind an alert (a sender failing DMARC).
    danger_bg: Color32,
    /// Time taken as "out of office".
    away: Color32,
}

const DARK: Palette = Palette {
    panel: Color32::from_rgb(0x1a, 0x1e, 0x23),
    panel2: Color32::from_rgb(0x1f, 0x24, 0x2a),
    raised: Color32::from_rgb(0x25, 0x2b, 0x32),
    line: Color32::from_rgb(0x2e, 0x35, 0x3d),
    text: Color32::from_rgb(0xe8, 0xeb, 0xef),
    muted: Color32::from_rgb(0xa7, 0xb0, 0xba),
    faint: Color32::from_rgb(0x8c, 0x96, 0xa1),
    accent: Color32::from_rgb(0x2a, 0xb7, 0xb0),
    accent_text: Color32::from_rgb(0x5e, 0xd6, 0xcf),
    on_accent: Color32::from_rgb(0x04, 0x20, 0x1e),
    title: Color32::from_rgb(0x0e, 0x6b, 0x69),
    title_text: Color32::WHITE,
    sel: Color32::from_rgb(0x1c, 0x3a, 0x3b),
    ok: Color32::from_rgb(0x7d, 0xd7, 0xa1),
    warn: Color32::from_rgb(0xf4, 0xc4, 0x6a),
    warn_bg: Color32::from_rgb(0x3b, 0x2c, 0x10),
    danger: Color32::from_rgb(0xff, 0x9d, 0x96),
    danger_bg: Color32::from_rgb(0x3b, 0x16, 0x16),
    away: Color32::from_rgb(0xb8, 0x93, 0xff),
};

const LIGHT: Palette = Palette {
    panel: Color32::from_rgb(0xf3, 0xf5, 0xf7),
    panel2: Color32::from_rgb(0xff, 0xff, 0xff),
    raised: Color32::from_rgb(0xe7, 0xeb, 0xef),
    line: Color32::from_rgb(0xd3, 0xd9, 0xdf),
    text: Color32::from_rgb(0x1b, 0x1f, 0x24),
    muted: Color32::from_rgb(0x4a, 0x54, 0x5e),
    faint: Color32::from_rgb(0x66, 0x70, 0x7b),
    accent: Color32::from_rgb(0x1f, 0x9e, 0x98),
    accent_text: Color32::from_rgb(0x0b, 0x6e, 0x69),
    on_accent: Color32::WHITE,
    title: Color32::from_rgb(0x0e, 0x6b, 0x69),
    title_text: Color32::WHITE,
    sel: Color32::from_rgb(0xd2, 0xec, 0xea),
    ok: Color32::from_rgb(0x1e, 0x7d, 0x45),
    warn: Color32::from_rgb(0x8f, 0x5b, 0x00),
    warn_bg: Color32::from_rgb(0xfb, 0xef, 0xd2),
    danger: Color32::from_rgb(0xb8, 0x32, 0x27),
    danger_bg: Color32::from_rgb(0xfb, 0xe4, 0xe1),
    away: Color32::from_rgb(0x6b, 0x3f, 0xc0),
};

/// The light theme is shown now.
static LIGHT_SHOWN: AtomicBool = AtomicBool::new(false);

fn palette() -> &'static Palette {
    if LIGHT_SHOWN.load(Ordering::Relaxed) { &LIGHT } else { &DARK }
}

pub fn panel() -> Color32 {
    palette().panel
}
pub fn panel2() -> Color32 {
    palette().panel2
}
pub fn raised() -> Color32 {
    palette().raised
}
pub fn line() -> Color32 {
    palette().line
}
pub fn text() -> Color32 {
    palette().text
}
pub fn muted() -> Color32 {
    palette().muted
}
pub fn faint() -> Color32 {
    palette().faint
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn accent_text() -> Color32 {
    palette().accent_text
}
pub fn on_accent() -> Color32 {
    palette().on_accent
}
pub fn title() -> Color32 {
    palette().title
}
pub fn title_text() -> Color32 {
    palette().title_text
}
pub fn sel() -> Color32 {
    palette().sel
}
pub fn ok() -> Color32 {
    palette().ok
}
pub fn warn() -> Color32 {
    palette().warn
}
pub fn warn_bg() -> Color32 {
    palette().warn_bg
}
pub fn danger() -> Color32 {
    palette().danger
}
pub fn danger_bg() -> Color32 {
    palette().danger_bg
}
pub fn away() -> Color32 {
    palette().away
}

/// An account's colour.
pub fn account(c: mailalm_model::Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// egui's look for one palette.
fn visuals(p: &Palette, mut v: egui::Visuals) -> egui::Visuals {
    v.panel_fill = p.panel;
    v.window_fill = p.raised;
    v.window_stroke = egui::Stroke::new(1.0, p.line);
    v.extreme_bg_color = p.panel2;
    v.faint_bg_color = p.panel2;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent_text;
    v.selection.bg_fill = p.sel;
    v.selection.stroke = egui::Stroke::new(1.0, p.accent_text);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.line);
    v.widgets.inactive.weak_bg_fill = p.raised;
    v.widgets.inactive.bg_fill = p.raised;
    v.widgets.hovered.weak_bg_fill = p.sel;
    v.widgets.hovered.bg_fill = p.sel;
    v.widgets.active.weak_bg_fill = p.sel;
    v.widgets.active.bg_fill = p.sel;
    v
}

/// MailALM's look on egui's dark and light themes, and the theme chosen.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, egui::Visuals::dark()));
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, egui::Visuals::light()));
    ctx.set_theme(match choice {
        ThemeChoice::Dark => ThemePreference::Dark,
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::System => ThemePreference::System,
    });
    sync(ctx);
}

/// Which theme egui shows now (it follows Windows under [`ThemeChoice::System`]): the colours
/// follow it. Called at the start of every frame.
pub fn sync(ctx: &egui::Context) {
    LIGHT_SHOWN.store(ctx.theme() == Theme::Light, Ordering::Relaxed);
}
