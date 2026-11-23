//! Writing a message, in its own window (PLAN.md, section 12). Plain text in this phase; the rich
//! editor (WordCraft), attachments, signatures and Antidote come in phases 5 and 7.

use egui::{Align, Frame, Layout, Margin, RichText, TextEdit, ViewportBuilder, ViewportCommand, ViewportId};
use mailalm_mime::{Outgoing, format_person, parse_recipients};
use mailalm_model::{AccountId, Person};

use crate::theme as t;

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
    error: Option<String>,
    /// The user asked to close a window with something in it.
    confirm_discard: bool,
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
            error,
            confirm_discard: false,
            focus_set: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.to.trim().is_empty() && self.cc.trim().is_empty() && self.subject.trim().is_empty() && self.body.trim().is_empty()
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
        })
    }

    /// Shows the window. `account_label` names the account the message goes out from.
    pub fn show(&mut self, ctx: &egui::Context, account_label: &str) -> ComposeAction {
        let title = if self.subject.trim().is_empty() { "New message".to_owned() } else { self.subject.trim().to_owned() };
        let builder = ViewportBuilder::default()
            .with_title(format!("{title} · MailALM"))
            .with_app_id("mailalm")
            .with_inner_size([780.0, 640.0])
            .with_min_inner_size([480.0, 360.0]);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(("mailalm-compose", self.id)), builder, |ui, _class| {
            let close_requested = ui.ctx().input(|i| i.viewport().close_requested());
            if close_requested {
                if self.is_empty() {
                    return ComposeAction::Discard;
                }
                ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm_discard = true;
            }
            let mut action = ComposeAction::None;
            egui::CentralPanel::default().frame(Frame::new().fill(t::PANEL).inner_margin(Margin::symmetric(18, 14))).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Send").strong().color(t::ON_ACCENT)).fill(t::ACCENT)).clicked() {
                        match self.outgoing() {
                            Ok(o) => action = ComposeAction::Send(Box::new(o)),
                            Err(e) => self.error = Some(e),
                        }
                    }
                    if ui.button("Discard").clicked() {
                        if self.is_empty() {
                            action = ComposeAction::Discard;
                        } else {
                            self.confirm_discard = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Plain text · attachments arrive in phase 5").size(11.5).color(t::FAINT));
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
            action
        })
    }
}
