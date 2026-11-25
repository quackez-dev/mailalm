//! The approval dialog of an attachment (PLAN.md, sections 10.1 and 12): a dialog over the main
//! window that shows what MailALM found before anything is done with the file, and the actions
//! allowed.
//!
//! - Nothing happens on its own: the attachment is decoded when the user clicks Review, and only
//!   that one. The preview is drawn only when they click Preview.
//! - **Preview** is MailALM's own (pictures, PDF pages, text: `preview.rs`); no program is
//!   started.
//! - **Save as…** writes the file where the user chooses, with the Mark-of-the-Web.
//! - **Open with Windows** is offered only for what `mailalm_safety` allows (PDF, pictures, .txt,
//!   Office documents without macros, with no warning), after a second confirmation. A marked
//!   copy is written to MailALM's own folder, named with the allowed extension, and handed to
//!   Windows: `rundll32 shell32.dll,ShellExec_RunDLL <copy>`, started directly (no command
//!   shell, no command line built from the file's name).

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Button, Color32, Frame, Id, Margin, RichText, ScrollArea, Stroke, TextureHandle, TextureOptions, Ui, Vec2};
use mailalm_engine::Reviewed;
use mailalm_mime::AttachmentInfo;
use mailalm_model::Person;
use mailalm_safety::files::{Mark, TempFolder, save_marked};
use mailalm_safety::{Authentication, Check, Preview, Report, Severity, Verdict};

use crate::mailbox::MessageKey;
use crate::preview::{self, PDF_PAGES, Rendered};
use crate::theme as t;

/// An attachment under review.
pub struct Review {
    pub key: MessageKey,
    pub index: usize,
    info: AttachmentInfo,
    sender: Person,
    auth: Authentication,
    state: State,
}

enum State {
    /// Waiting for the account's worker to decode it.
    Loading,
    Failed(String),
    Ready(Box<Ready>),
}

struct Ready {
    bytes: Arc<Vec<u8>>,
    report: Report,
    preview: PreviewState,
    /// Asking "open it with Windows?" a second time.
    confirm_open: bool,
    /// What the last action did (and whether it went wrong).
    note: Option<(String, bool)>,
}

enum PreviewState {
    Off,
    Working(Receiver<Rendered>),
    Image(TextureHandle),
    Pages(Vec<TextureHandle>, usize),
    Text(String),
    Failed(String),
}

pub enum ReviewAction {
    None,
    Close,
}

impl Review {
    /// A review of attachment `index` of the message `key`, waiting for its content.
    pub fn new(key: MessageKey, index: usize, info: AttachmentInfo, sender: Person, auth: Authentication) -> Self {
        Review { key, index, info, sender, auth, state: State::Loading }
    }

    /// The worker's answer.
    pub fn loaded(&mut self, result: Result<Box<Reviewed>, String>) {
        self.state = match result {
            Ok(r) => {
                let Reviewed { data, report } = *r;
                self.info = data.info;
                State::Ready(Box::new(Ready { bytes: Arc::new(data.bytes), report, preview: PreviewState::Off, confirm_open: false, note: None }))
            }
            Err(e) => State::Failed(e),
        };
    }

    /// Shows the dialog. `temp` is the folder of copies opened with Windows.
    pub fn show(&mut self, ctx: &egui::Context, temp: &mut TempFolder) -> ReviewAction {
        if let State::Ready(r) = &mut self.state {
            poll_preview(ctx, r, &self.info.name);
        }
        let mut close = false;
        let modal = egui::Modal::new(Id::unique("mailalm-attachment-review")).show(ctx, |ui| {
            ui.set_width(680.0);
            self.header(ui);
            ui.add_space(10.0);
            match &mut self.state {
                State::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Reading the attachment from the message…").color(t::muted()));
                    });
                }
                State::Failed(e) => {
                    ui.label(RichText::new(e.as_str()).color(t::danger()));
                }
                State::Ready(r) => ready(ui, r, &self.info, temp),
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        if close || modal.should_close() { ReviewAction::Close } else { ReviewAction::None }
    }

    /// Name, size, sender, sender checks.
    fn header(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
            crate::icons::paint(ui.painter(), r, crate::icons::Icon::Lock, t::warn());
            ui.label(RichText::new("Review an attachment").size(17.0).strong());
        });
        ui.add_space(8.0);
        egui::Grid::new("mailalm-review-facts").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
            fact(ui, "Name", &self.info.name);
            fact(ui, "Size", &size_text(self.info.size));
            fact(ui, "Declared type", &self.info.declared_type);
            let from = if self.sender.name.is_empty() {
                self.sender.address.clone()
            } else {
                format!("{} <{}>", self.sender.name, self.sender.address)
            };
            fact(ui, "From", &from);
            ui.label(RichText::new("Sender checks").color(t::muted()));
            ui.vertical(|ui| {
                auth_pills(ui, &self.auth);
                let by = match (&self.auth.checked_by, self.auth.found) {
                    (_, false) => "The message has no Authentication-Results header: the sender was not checked.".to_owned(),
                    (Some(by), true) => format!("As checked by {by}, the server that received the message."),
                    (None, true) => "As checked by the server that received the message.".to_owned(),
                };
                ui.label(RichText::new(by).size(11.5).color(t::faint()));
            });
            ui.end_row();
        });
    }
}

fn fact(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).color(t::muted()));
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

/// The SPF, DKIM and DMARC results as three small pills (the reading pane shows them too).
pub fn auth_pills(ui: &mut Ui, a: &Authentication) {
    ui.horizontal_wrapped(|ui| {
        for (name, check) in [("SPF", &a.spf), ("DKIM", &a.dkim), ("DMARC", &a.dmarc)] {
            auth_pill(ui, name, check);
        }
    });
}

fn auth_pill(ui: &mut Ui, name: &str, c: &Check) {
    let (fg, bg) = match c.verdict {
        Verdict::Pass => (t::ok(), t::raised()),
        Verdict::Fail => (t::danger(), t::warn_bg()),
        Verdict::Other => (t::warn(), t::raised()),
        Verdict::Missing => (t::faint(), t::raised()),
    };
    Frame::new().fill(bg).corner_radius(10).inner_margin(Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.label(RichText::new(format!("{name} {}", c.word)).size(12.0).color(fg));
    });
}

pub fn size_text(bytes: usize) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} bytes"),
    }
}

/// The checks' results, the actions and the preview.
fn ready(ui: &mut Ui, r: &mut Ready, info: &AttachmentInfo, temp: &mut TempFolder) {
    egui::Grid::new("mailalm-review-content").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Content").color(t::muted()));
        ui.label(RichText::new(format!("{} (found by MailALM from the file itself)", r.report.real.label())).strong());
        ui.end_row();
    });
    ui.add_space(8.0);
    if r.report.warnings.is_empty() {
        ui.label(RichText::new("No warning.").color(t::ok()));
    }
    for w in &r.report.warnings {
        let (fg, mark) = match w.severity() {
            Severity::Danger => (t::danger(), "Danger"),
            Severity::Caution => (t::warn(), "Caution"),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(mark).size(12.0).strong().color(fg));
            ui.label(RichText::new(w.text()).color(fg));
        });
    }
    ui.add_space(10.0);

    let policy = r.report.policy.clone();
    ui.horizontal_wrapped(|ui| {
        let preview_label = match policy.preview {
            Some(Preview::Pdf) => format!("Preview (first {PDF_PAGES} pages)"),
            _ => "Preview".to_owned(),
        };
        let preview_button =
            ui.add_enabled(policy.preview.is_some() && matches!(r.preview, PreviewState::Off | PreviewState::Failed(_)), Button::new(RichText::new(preview_label)));
        let preview_button =
            if policy.preview.is_none() { preview_button.on_disabled_hover_text("MailALM previews pictures, PDFs and text only.") } else { preview_button };
        if preview_button.clicked()
            && let Some(kind) = policy.preview
        {
            r.preview = PreviewState::Working(preview::start(ui.ctx(), kind, Arc::clone(&r.bytes)));
        }
        if ui.button("Save as…").clicked() {
            r.confirm_open = false;
            r.note = save_as(&info.name, &r.bytes);
        }
        let open = ui.add_enabled(policy.open.is_ok(), Button::new(RichText::new("Open with Windows…")));
        let open = match &policy.open {
            Err(why) => open.on_disabled_hover_text(why.as_str()),
            Ok(_) => open.on_hover_text("Opens a copy with the program Windows uses for this type, after you confirm."),
        };
        if open.clicked() {
            r.confirm_open = true;
            r.note = None;
        }
    });
    if let Err(why) = &policy.open {
        ui.label(RichText::new(format!("Open with Windows: {why}")).size(11.5).color(t::faint()));
    }

    if r.confirm_open
        && let Ok(ext) = policy.open
    {
        ui.add_space(8.0);
        Frame::new().fill(t::warn_bg()).corner_radius(6).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "Open \"{}\" with the program Windows uses for .{ext} files? Once it is open, MailALM cannot protect you: a marked copy is opened, so Office shows it in Protected View.",
                    info.name
                ))
                .color(t::warn()),
            );
            ui.horizontal(|ui| {
                if ui.button("Open it").clicked() {
                    r.confirm_open = false;
                    r.note = Some(match temp.copy(&info.name, ext, &r.bytes) {
                        Ok(path) => match open_with_windows(&path) {
                            Ok(()) => (format!("Opened with Windows (a copy, in {}).", temp.dir().display()), false),
                            Err(e) => (e, true),
                        },
                        Err(e) => (format!("The copy could not be written: {e}"), true),
                    });
                }
                if ui.button("Cancel").clicked() {
                    r.confirm_open = false;
                }
            });
        });
    }
    if let Some((text, problem)) = &r.note {
        ui.add_space(6.0);
        ui.label(RichText::new(text.as_str()).color(if *problem { t::danger() } else { t::accent_text() }));
    }

    if !matches!(r.preview, PreviewState::Off) {
        ui.add_space(10.0);
        Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(6).inner_margin(Margin::same(8)).show(ui, |ui| {
            ScrollArea::vertical().max_height(440.0).auto_shrink([false, true]).show(ui, |ui| show_preview(ui, &r.preview));
        });
    }
}

/// Picks up the preview thread's answer and makes its textures.
fn poll_preview(ctx: &egui::Context, r: &mut Ready, name: &str) {
    let PreviewState::Working(rx) = &r.preview else { return };
    r.preview = match rx.try_recv() {
        Ok(Rendered::Image(img)) => PreviewState::Image(ctx.load_texture(format!("mailalm-preview-{name}"), img, TextureOptions::LINEAR)),
        Ok(Rendered::Pages(pages, total)) => PreviewState::Pages(
            pages.into_iter().enumerate().map(|(n, p)| ctx.load_texture(format!("mailalm-preview-{name}-{n}"), p, TextureOptions::LINEAR)).collect(),
            total,
        ),
        Ok(Rendered::Text(text)) => PreviewState::Text(text),
        Ok(Rendered::Failed(e)) => PreviewState::Failed(e),
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => PreviewState::Failed("The preview stopped: the file could not be drawn.".to_owned()),
    };
}

fn show_preview(ui: &mut Ui, p: &PreviewState) {
    let width = ui.available_width();
    let fit = |size: Vec2| if size.x > width && size.x > 0.0 { size * (width / size.x) } else { size };
    match p {
        PreviewState::Off => {}
        PreviewState::Working(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Drawing the preview…").color(t::muted()));
            });
        }
        PreviewState::Image(tex) => {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), fit(tex.size_vec2()))));
        }
        PreviewState::Pages(pages, total) => {
            for tex in pages {
                ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), fit(tex.size_vec2()))));
                ui.add_space(6.0);
            }
            if *total > pages.len() {
                ui.label(RichText::new(format!("{} of {total} pages shown. Save the PDF to read the rest.", pages.len())).color(t::faint()));
            }
        }
        PreviewState::Text(text) => {
            ui.add(egui::Label::new(RichText::new(text.as_str()).monospace().size(12.5)).wrap().selectable(true));
        }
        PreviewState::Failed(e) => {
            ui.label(RichText::new(e.as_str()).color(t::danger()));
        }
    }
}

/// Asks where to save the attachment and writes it there, marked as coming from the internet.
/// The note to show, if anything was done.
fn save_as(name: &str, bytes: &[u8]) -> Option<(String, bool)> {
    let path = rfd::FileDialog::new().set_title("Save attachment").set_file_name(name).save_file()?;
    Some(match save_marked(&path, bytes) {
        Ok(Mark::Written) => (format!("Saved as {}, marked as coming from the internet.", path.display()), false),
        Ok(Mark::NotWritten) => (
            format!("Saved as {}, but that drive cannot keep the \"from the internet\" mark: be careful when you open it.", path.display()),
            true,
        ),
        Err(e) => (format!("It could not be saved: {e}"), true),
    })
}

/// Hands a file to Windows, which opens it with the program set for its extension. rundll32
/// from the system folder, started directly: no command shell reads the path.
#[cfg(windows)]
fn open_with_windows(path: &Path) -> Result<(), String> {
    let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("C:\\Windows"));
    let rundll32 = system.join("System32").join("rundll32.exe");
    std::process::Command::new(rundll32)
        .arg("shell32.dll,ShellExec_RunDLL")
        .arg(path)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Windows could not open it: {e}"))
}

#[cfg(not(windows))]
fn open_with_windows(_path: &Path) -> Result<(), String> {
    Err("Opening with another program is done on Windows only.".to_owned())
}

/// The colour a review's lock takes in the reading pane.
pub fn lock_color(reviewed: bool) -> Color32 {
    if reviewed { t::ok() } else { t::warn() }
}
