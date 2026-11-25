//! Writing a message, in its own window (PLAN.md, sections 8 and 12). Plain text in this phase,
//! with attachments (Attach… or dropped on the window); the rich editor (WordCraft), signatures
//! and Antidote come in phase 7.

use std::io::Read;
use std::path::{Path, PathBuf};

use egui::{Align, Frame, Layout, Margin, RichText, Stroke, TextEdit, ViewportBuilder, ViewportCommand, ViewportId};
use mailalm_engine::SizeLimit;
use mailalm_mime::{Outgoing, OutgoingAttachment, format_person, mentions_attachment, parse_recipients};
use mailalm_model::{AccountId, Person};

use crate::attachments::size_text;
use crate::icons::{self, Icon};
use crate::theme as t;

/// The largest file read to be attached (the provider's limit is checked before sending).
const MAX_FILE: u64 = 40 * 1024 * 1024;
/// The most files attached to one message.
const MAX_FILES: usize = 100;

pub struct Compose {
    id: u64,
    pub account: AccountId,
    from: Person,
    to: String,
    cc: String,
    bcc: String,
    show_cc: bool,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
    references: Vec<String>,
    attachments: Vec<OutgoingAttachment>,
    error: Option<String>,
    /// The user asked to close a window with something in it.
    confirm_discard: bool,
    /// The text mentions an attachment and there is none: asked before sending.
    confirm_no_attachment: bool,
    focus_set: bool,
}

pub enum ComposeAction {
    None,
    Send(Box<Outgoing>),
    Discard,
}

impl Compose {
    /// A window for a new message, a reply or a forward (`o` starts it), or a message that failed
    /// to send (`error` says why).
    pub fn new(id: u64, account: AccountId, o: &Outgoing, error: Option<String>) -> Self {
        let list = |v: &[Person]| v.iter().map(format_person).collect::<Vec<_>>().join(", ");
        Self {
            id,
            account,
            from: o.from.clone(),
            to: list(&o.to),
            cc: list(&o.cc),
            bcc: list(&o.bcc),
            show_cc: !o.cc.is_empty() || !o.bcc.is_empty(),
            subject: o.subject.clone(),
            body: o.text.clone(),
            in_reply_to: o.in_reply_to.clone(),
            references: o.references.clone(),
            attachments: o.attachments.clone(),
            error,
            confirm_discard: false,
            confirm_no_attachment: false,
            focus_set: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.to.trim().is_empty() && self.cc.trim().is_empty() && self.subject.trim().is_empty() && self.body.trim().is_empty() && self.attachments.is_empty()
    }

    fn outgoing(&self) -> Result<Outgoing, String> {
        let to = parse_recipients(&self.to)?;
        let cc = parse_recipients(&self.cc)?;
        let bcc = parse_recipients(&self.bcc)?;
        if to.is_empty() && cc.is_empty() && bcc.is_empty() {
            return Err("Add at least one recipient.".to_owned());
        }
        Ok(Outgoing {
            from: self.from.clone(),
            to,
            cc,
            bcc,
            subject: self.subject.trim().to_owned(),
            text: self.body.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            attachments: self.attachments.clone(),
        })
    }

    /// What is wrong with the message's weight for this account, if anything.
    fn over_limit(&self, limit: SizeLimit) -> Option<String> {
        let (used, max) = limit.measure(&self.body, &self.attachments);
        if used <= max {
            return None;
        }
        Some(match limit {
            SizeLimit::Attachments(_) => format!("The attachments weigh {}: this account sends at most {}. Remove one, or share the file by a link.", size_text(used), size_text(max)),
            SizeLimit::Message(_) => format!(
                "The message would weigh about {}: MailALM sends at most {} from an Outlook account for now. Remove or shrink an attachment, or share the file by a link.",
                size_text(used),
                size_text(max)
            ),
        })
    }

    /// Checks the message and hands it on to be sent; asks first when the text speaks of an
    /// attachment and none is attached (unless `anyway`).
    fn try_send(&mut self, limit: SizeLimit, anyway: bool) -> ComposeAction {
        let o = match self.outgoing() {
            Ok(o) => o,
            Err(e) => {
                self.error = Some(e);
                return ComposeAction::None;
            }
        };
        if let Some(e) = self.over_limit(limit) {
            self.error = Some(e);
            return ComposeAction::None;
        }
        if !anyway && o.attachments.is_empty() && (mentions_attachment(&o.text) || mentions_attachment(&o.subject)) {
            self.confirm_no_attachment = true;
            return ComposeAction::None;
        }
        ComposeAction::Send(Box::new(o))
    }

    /// Adds files from the computer; a file that cannot be attached is said.
    fn attach(&mut self, paths: &[PathBuf]) {
        let mut problems: Vec<String> = Vec::new();
        for path in paths {
            if self.attachments.len() >= MAX_FILES {
                problems.push(format!("At most {MAX_FILES} files can be attached."));
                break;
            }
            match read_file(path) {
                Ok((name, bytes)) => self.attachments.push(OutgoingAttachment::new(&name, bytes)),
                Err(e) => problems.push(e),
            }
        }
        self.confirm_no_attachment = false;
        self.error = (!problems.is_empty()).then(|| problems.join(" "));
    }

    /// Shows the window. `account_label` names the account the message goes out from; `limit`
    /// is what its provider takes.
    pub fn show(&mut self, ctx: &egui::Context, account_label: &str, limit: SizeLimit) -> ComposeAction {
        let title = if self.subject.trim().is_empty() { "New message".to_owned() } else { self.subject.trim().to_owned() };
        let builder = ViewportBuilder::default()
            .with_title(format!("{title} · MailALM"))
            .with_app_id("mailalm")
            .with_inner_size([780.0, 640.0])
            .with_min_inner_size([480.0, 360.0])
            .with_drag_and_drop(true);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(("mailalm-compose", self.id)), builder, |ui, _class| {
            let close_requested = ui.ctx().input(|i| i.viewport().close_requested());
            if close_requested {
                if self.is_empty() {
                    return ComposeAction::Discard;
                }
                ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm_discard = true;
            }
            // Files dropped on this window: attached. Only their paths are taken here; each file
            // is read with a size cap (`read_file`).
            let (dropped, hovering): (Vec<PathBuf>, bool) =
                ui.ctx().input(|i| (i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect(), !i.raw.hovered_files.is_empty()));
            if !dropped.is_empty() {
                self.attach(&dropped);
            }
            let mut action = ComposeAction::None;
            egui::CentralPanel::default().frame(Frame::new().fill(t::PANEL).inner_margin(Margin::symmetric(18, 14))).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Send").strong().color(t::ON_ACCENT)).fill(t::ACCENT)).clicked() {
                        action = self.try_send(limit, false);
                    }
                    if ui.button("Attach…").clicked()
                        && let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files()
                    {
                        self.attach(&paths);
                    }
                    if ui.button("Discard").clicked() {
                        if self.is_empty() {
                            action = ComposeAction::Discard;
                        } else {
                            self.confirm_discard = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Plain text · drop files on this window to attach them").size(11.5).color(t::FAINT));
                    });
                });
                if self.confirm_discard {
                    ui.add_space(6.0);
                    Frame::new().fill(t::WARN_BG).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Discard this message? What you wrote is lost.").color(t::WARN));
                            if ui.button("Discard").clicked() {
                                action = ComposeAction::Discard;
                            }
                            if ui.button("Keep writing").clicked() {
                                self.confirm_discard = false;
                            }
                        });
                    });
                }
                if self.confirm_no_attachment {
                    ui.add_space(6.0);
                    Frame::new().fill(t::WARN_BG).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Your message mentions an attachment, but nothing is attached.").color(t::WARN));
                            if ui.button("Send anyway").clicked() {
                                self.confirm_no_attachment = false;
                                action = self.try_send(limit, true);
                            }
                            if ui.button("Attach a file…").clicked() {
                                self.confirm_no_attachment = false;
                                if let Some(paths) = rfd::FileDialog::new().set_title("Attach files").pick_files() {
                                    self.attach(&paths);
                                }
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirm_no_attachment = false;
                            }
                        });
                    });
                }
                if let Some(e) = &self.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(t::DANGER));
                }
                ui.add_space(10.0);
                egui::Grid::new(("mailalm-compose-fields", self.id)).num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label(RichText::new("From").color(t::MUTED));
                    ui.label(format!("{} · {}", account_label, format_person(&self.from)));
                    ui.end_row();
                    ui.label(RichText::new("To").color(t::MUTED));
                    ui.horizontal(|ui| {
                        let to = ui.add(TextEdit::singleline(&mut self.to).desired_width(ui.available_width() - 70.0).hint_text("name@example.com, …"));
                        if !self.focus_set && self.to.is_empty() {
                            to.request_focus();
                            self.focus_set = true;
                        }
                        if !self.show_cc && ui.small_button("Cc / Bcc").clicked() {
                            self.show_cc = true;
                        }
                    });
                    ui.end_row();
                    if self.show_cc {
                        ui.label(RichText::new("Cc").color(t::MUTED));
                        ui.add(TextEdit::singleline(&mut self.cc).desired_width(f32::INFINITY));
                        ui.end_row();
                        ui.label(RichText::new("Bcc").color(t::MUTED));
                        ui.add(TextEdit::singleline(&mut self.bcc).desired_width(f32::INFINITY));
                        ui.end_row();
                    }
                    ui.label(RichText::new("Subject").color(t::MUTED));
                    ui.add(TextEdit::singleline(&mut self.subject).desired_width(f32::INFINITY));
                    ui.end_row();
                });
                self.attachment_list(ui, limit);
                ui.add_space(10.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let body = ui.add_sized(
                        [ui.available_width(), ui.available_height().max(240.0)],
                        TextEdit::multiline(&mut self.body).lock_focus(true).desired_width(f32::INFINITY),
                    );
                    if !self.focus_set {
                        body.request_focus();
                        self.focus_set = true;
                    }
                });
            });
            if hovering {
                let screen = ui.ctx().content_rect();
                let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("mailalm-compose-drop", self.id))));
                painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
                painter.text(screen.center(), egui::Align2::CENTER_CENTER, "Drop to attach", egui::FontId::proportional(22.0), t::TEXT);
            }
            action
        })
    }

    /// The attached files, each with its remove button, and their weight against the account's
    /// limit.
    fn attachment_list(&mut self, ui: &mut egui::Ui, limit: SizeLimit) {
        if self.attachments.is_empty() {
            return;
        }
        ui.add_space(8.0);
        let mut remove: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            for (n, a) in self.attachments.iter().enumerate() {
                Frame::new().fill(t::PANEL2).stroke(Stroke::new(1.0, t::LINE)).corner_radius(6).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&a.name).strong());
                        ui.label(RichText::new(size_text(a.bytes.len())).size(12.0).color(t::FAINT));
                        if icons::button(ui, Icon::Close, egui::vec2(18.0, 18.0), t::MUTED, t::SEL, "Remove this attachment").clicked() {
                            remove = Some(n);
                        }
                    });
                });
            }
        });
        if let Some(n) = remove
            && n < self.attachments.len()
        {
            self.attachments.remove(n);
            self.error = None;
        }
        let (used, max) = limit.measure(&self.body, &self.attachments);
        let what = match limit {
            SizeLimit::Attachments(_) => "Attachments",
            SizeLimit::Message(_) => "Message, about",
        };
        let color = if used > max { t::DANGER } else { t::FAINT };
        ui.label(RichText::new(format!("{what} {} of {} at most for this account", size_text(used), size_text(max))).size(12.0).color(color));
    }
}

/// A file from the computer to attach: its name and its bytes, at most [`MAX_FILE`].
fn read_file(path: &Path) -> Result<(String, Vec<u8>), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".to_owned());
    let meta = std::fs::metadata(path).map_err(|e| format!("{name} could not be read: {e}."))?;
    if !meta.is_file() {
        return Err(format!("{name} is a folder: attach the files in it instead."));
    }
    if meta.len() > MAX_FILE {
        return Err(format!("{name} is larger than {} MB: share it by a link instead.", MAX_FILE >> 20));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{name} could not be read: {e}."))?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(format!("{name} is larger than {} MB: share it by a link instead.", MAX_FILE >> 20));
    }
    Ok((name, bytes))
}
