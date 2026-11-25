//! A message's text in the reading pane (PLAN.md section 10.2). An HTML message is shown
//! formatted: converted by the editor's import (`mailalm_compose::import_message_html`: no web
//! engine, nothing run, nothing fetched) and drawn by the editor's widget, read-only. A plain-text
//! message is shown as it is.
//!
//! - Links: only `http`, `https` and `mailto` ones are links at all (the editor's model keeps no
//!   other). Pointed at, they say their real domain and what is odd about them (`links`); clicked,
//!   they ask first, then open the browser or a new message.
//! - Pictures from the internet are blocked: a banner offers to load them for this message, or
//!   always for this sender; even then only some are fetched (`pictures`).

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Frame, Id, Margin, RichText, Stroke};
use mailalm_compose::{Editor, InlineImage, RemoteImage, RichEditor, import_message_html};
use mailalm_mime::HtmlBody;

use crate::compose::editor_images;
use crate::links::{self, LinkCheck, Opens};
use crate::mailbox::MessageKey;
use crate::pictures::{self, Fetched};
use crate::theme as t;

/// What the reading pane is asked to do.
pub enum ViewAction {
    /// Open this web address in the browser (the user confirmed).
    OpenWeb(String),
    /// Write a message (a `mailto:` link the user confirmed).
    Write { to: String, subject: String, body: String },
    /// Load this sender's pictures from now on.
    ApproveSender,
}

/// A message's text as the reading pane shows it.
pub struct MessageView {
    pub key: MessageKey,
    html: Option<Box<HtmlBody>>,
    /// The pictures the message carries (`cid:`).
    provided: Vec<InlineImage>,
    /// The HTML, converted (`None`: plain text).
    editor: Option<Editor>,
    remote: Vec<RemoteImage>,
    loaded: Vec<(String, InlineImage)>,
    failed: usize,
    loading: Option<Receiver<Fetched>>,
    /// Loading was asked for (for this message, or by its sender's approval).
    asked: bool,
    serial: u64,
    /// A link clicked, waiting for the user's confirmation.
    confirm: Option<(String, LinkCheck)>,
}

impl MessageView {
    /// The view of a message; `html` is its HTML version, when it has one.
    pub fn new(key: MessageKey, html: Option<Box<HtmlBody>>, serial: u64) -> Self {
        let provided = html.as_ref().map(|h| editor_images(&h.images)).unwrap_or_default();
        let mut v = MessageView { key, html, provided, editor: None, remote: Vec::new(), loaded: Vec::new(), failed: 0, loading: None, asked: false, serial, confirm: None };
        v.convert();
        v
    }

    fn convert(&mut self) {
        let Some(h) = &self.html else { return };
        let imported = import_message_html(&h.html, &self.provided, &self.loaded);
        self.remote = imported.remote;
        self.editor = Some(Editor::from_document(imported.document));
    }

    /// Whether the message is shown formatted (it has an HTML version).
    pub fn formatted(&self) -> bool {
        self.editor.is_some()
    }

    /// The message's HTML version, to quote it in a reply or a forward.
    pub fn html(&self) -> Option<&HtmlBody> {
        self.html.as_deref()
    }

    /// Starts fetching the message's web pictures (those [`pictures::plan`] allows).
    pub fn load_pictures(&mut self, ctx: &egui::Context) {
        if self.asked {
            return;
        }
        self.asked = true;
        let plan = pictures::plan(&self.remote);
        if !plan.fetch.is_empty() {
            self.loading = Some(pictures::start(ctx, plan.fetch, self.serial));
        }
    }

    /// Takes in the pictures fetched since the last frame.
    pub fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        let mut got = false;
        loop {
            match rx.try_recv() {
                Ok((url, Ok(image))) => {
                    self.loaded.push((url, image));
                    got = true;
                }
                Ok((url, Err(e))) => {
                    log::info!("a picture of the message was not shown ({}): {e}", url.chars().take(120).collect::<String>());
                    self.failed += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.loading = None;
                    break;
                }
            }
        }
        if got {
            self.convert();
        }
    }

    /// The banner about the pictures from the internet; `sender` is who sent the message.
    pub fn pictures_banner(&mut self, ui: &mut egui::Ui, sender: &str) -> Option<ViewAction> {
        if self.remote.is_empty() && self.loaded.is_empty() {
            return None;
        }
        let plan = pictures::plan(&self.remote);
        let mut action = None;
        Frame::new().fill(t::PANEL2).stroke(Stroke::new(1.0, t::LINE)).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut notes: Vec<String> = Vec::new();
                if plan.trackers > 0 {
                    notes.push(format!("{} tracking picture{} never load{}", plan.trackers, plural(plan.trackers), if plan.trackers == 1 { "s" } else { "" }));
                }
                if plan.insecure > 0 {
                    notes.push(format!("{} not over https, not loaded", plan.insecure));
                }
                if plan.over > 0 {
                    notes.push(format!("{} beyond the first {}, not loaded", plan.over, pictures::MAX_PICTURES));
                }
                if self.loading.is_some() {
                    ui.spinner();
                    ui.label(RichText::new(format!("Loading the pictures… {} of {}", self.loaded.len() + self.failed, plan.fetch.len() + self.loaded.len())).size(12.5));
                } else if self.asked {
                    let mut text = format!("{} picture{} from the internet shown", self.loaded.len(), plural(self.loaded.len()));
                    if self.failed > 0 {
                        text.push_str(&format!(", {} could not be", self.failed));
                    }
                    ui.label(RichText::new(text).size(12.5).color(t::MUTED));
                } else if plan.fetch.is_empty() {
                    ui.label(RichText::new("Pictures from the internet are blocked.").size(12.5).color(t::MUTED));
                } else {
                    ui.label(RichText::new(format!("Pictures from the internet are blocked ({}).", plan.fetch.len())).size(12.5).strong());
                    if ui.small_button("Load for this message").on_hover_text("Fetches them now: their servers see that the message was opened, and from where.").clicked() {
                        self.load_pictures(ui.ctx());
                    }
                    if !sender.is_empty() && ui.small_button("Always for this sender").on_hover_text(format!("Loads the pictures of {sender}'s messages without asking (Settings, Writing and reading)")).clicked() {
                        self.load_pictures(ui.ctx());
                        action = Some(ViewAction::ApproveSender);
                    }
                }
                if !notes.is_empty() {
                    ui.label(RichText::new(format!("· {}", notes.join(" · "))).size(11.5).color(t::FAINT));
                }
            });
        });
        action
    }

    /// The text: formatted, with its links checked, or plain.
    pub fn body(&mut self, ui: &mut egui::Ui, text: &str) {
        let Some(editor) = &mut self.editor else {
            ui.add(egui::Label::new(RichText::new(text).size(14.0)).selectable(true).wrap());
            return;
        };
        let response = RichEditor::new(editor).id(Id::unique(("mailalm-reading", &self.key))).read_only(true).min_height(60.0).font_size(14.0).show(ui);
        if let Some(hit) = editor.hovered_link().cloned() {
            let check = links::check(hit.link.as_str(), &hit.text);
            response.on_hover_ui_at_pointer(|ui| {
                ui.set_max_width(420.0);
                ui.label(RichText::new(format!("Goes to {}", check.domain)).strong());
                for w in &check.warnings {
                    ui.label(RichText::new(w).size(12.0).color(t::WARN));
                }
                ui.label(RichText::new(clip(hit.link.as_str(), 200)).size(11.0).color(t::FAINT));
            });
        }
        if let Some(hit) = editor.take_clicked_link() {
            let check = links::check(hit.link.as_str(), &hit.text);
            self.confirm = Some((hit.link.as_str().to_owned(), check));
        }
    }

    /// The confirmation a clicked link asks for.
    pub fn link_dialog(&mut self, ctx: &egui::Context) -> Option<ViewAction> {
        let (url, check) = self.confirm.as_ref()?;
        let mut answer: Option<bool> = None;
        let mail = matches!(check.opens, Some(Opens::Mail { .. }));
        let m = egui::Modal::new(Id::unique("mailalm-confirm-link")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(if mail { "Write to this address?" } else { "Open this link?" });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(if mail { "Domain:" } else { "Site:" }).color(t::MUTED));
                ui.label(RichText::new(&check.domain).strong().size(15.0));
            });
            if let Some(u) = &check.unicode {
                ui.label(RichText::new(format!("written in the message as {u}")).size(12.0).color(t::FAINT));
            }
            ui.add(egui::Label::new(RichText::new(clip(url, 600)).size(11.5).color(t::FAINT)).selectable(true).wrap());
            if !check.warnings.is_empty() {
                ui.add_space(6.0);
                Frame::new().fill(t::WARN_BG).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                    for w in &check.warnings {
                        ui.label(RichText::new(w).color(t::WARN));
                    }
                });
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                match &check.opens {
                    Some(Opens::Web(_)) => {
                        if ui.button("Open in the browser").clicked() {
                            answer = Some(true);
                        }
                    }
                    Some(Opens::Mail { .. }) => {
                        if ui.button("Write a message").clicked() {
                            answer = Some(true);
                        }
                    }
                    None => {
                        ui.label(RichText::new("This link cannot be opened.").color(t::DANGER));
                    }
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if m.should_close() && answer.is_none() {
            answer = Some(false);
        }
        let yes = answer?;
        let (_, check) = self.confirm.take()?;
        if !yes {
            return None;
        }
        match check.opens? {
            Opens::Web(url) => Some(ViewAction::OpenWeb(url)),
            Opens::Mail { to, subject, body } => Some(ViewAction::Write { to, subject, body }),
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// At most `n` characters of `s`, without control characters.
fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().filter(|c| !c.is_control()).take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}
