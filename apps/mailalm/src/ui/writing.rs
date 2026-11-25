//! The Settings sections about writing and reading (C1, C4, PLAN.md sections 8 to 10.2): each
//! account's signature, templates and quick replies; the undo delay and plain or rich text; the
//! Antidote options; the senders whose pictures load without asking.

use egui::{Frame, Id, Margin, RichText, Stroke, TextEdit, Ui};
use mailalm_compose::{Document, Editor, RichEditor, ToolbarRequest, ToolbarState, import_html, to_html, toolbar};
use mailalm_model::AccountId;

use super::MailApp;
use crate::send_queue::{UNDO_MAX, UNDO_MIN};
use crate::signatures::{MAX_SAVED, QuickReply, Template};
use crate::theme as t;

/// What is being edited.
#[derive(Clone, Copy, PartialEq, Eq)]
enum What {
    Signature,
    /// A template (`None`: a new one).
    Template(Option<usize>),
    QuickReply(Option<usize>),
}

struct Editing {
    account: AccountId,
    what: What,
    name: String,
    subject: String,
    editor: Editor,
    toolbar: ToolbarState,
    /// A quick reply's text.
    text: String,
    error: Option<String>,
}

/// The state of the writing sections.
#[derive(Default)]
pub(super) struct WritingPage {
    account: Option<AccountId>,
    editing: Option<Editing>,
}

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(20.0);
    ui.label(RichText::new(text).size(11.0).strong().color(t::FAINT));
    ui.add_space(6.0);
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new().fill(t::PANEL2).stroke(Stroke::new(1.0, t::LINE)).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width().min(720.0));
        add(ui);
    });
}

impl MailApp {
    pub(super) fn writing_settings(&mut self, ui: &mut Ui) {
        let accounts: Vec<(AccountId, String)> = self.ws.accounts().iter().map(|a| (a.id, format!("{} · {}", a.name, a.address))).collect();
        let mut changed = false;

        section(ui, "SIGNATURES, TEMPLATES AND QUICK REPLIES");
        if accounts.is_empty() {
            ui.label(RichText::new("Add an account first.").color(t::FAINT));
        } else {
            let current = self.writing_page.account.filter(|a| accounts.iter().any(|(id, _)| id == a)).unwrap_or(accounts[0].0);
            self.writing_page.account = Some(current);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Account").color(t::MUTED));
                    let shown = accounts.iter().find(|(id, _)| *id == current).map(|(_, n)| n.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("mailalm-writing-account").selected_text(shown).show_ui(ui, |ui| {
                        for (id, name) in &accounts {
                            if ui.selectable_label(*id == current, name).clicked() {
                                self.writing_page.account = Some(*id);
                                self.writing_page.editing = None;
                            }
                        }
                    });
                });
                ui.add_space(8.0);
                changed |= self.writing_lists(ui, current);
                if self.writing_page.editing.is_some() {
                    ui.add_space(8.0);
                    changed |= self.writing_editor(ui);
                }
            });
        }

        section(ui, "SENDING");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Undo send: a message waits");
                let mut delay = self.settings.undo_delay();
                if ui.add(egui::Slider::new(&mut delay, UNDO_MIN..=UNDO_MAX).suffix(" s")).changed() {
                    self.settings.undo_send_seconds = delay;
                    changed = true;
                }
                ui.label("before it goes, with an Undo button.");
            });
            if ui.checkbox(&mut self.settings.plain_text, "Write new messages in plain text (no formatting or pictures)").changed() {
                changed = true;
            }
            ui.label(
                RichText::new("A message scheduled for later is kept on this PC and sent by MailALM at its time if it is running, else as soon as it starts.")
                    .size(11.5)
                    .color(t::FAINT),
            );
        });

        section(ui, "ANTIDOTE");
        card(ui, |ui| {
            if self.sending.antidote {
                ui.label(RichText::new("Antidote is installed: the compose window has its Corrector, Dictionaries and Guides.").color(t::OK));
            } else {
                ui.label(RichText::new("Antidote's connector was not found on this PC: its buttons are hidden.").color(t::MUTED));
            }
            ui.add_enabled_ui(self.sending.antidote, |ui| {
                if ui.checkbox(&mut self.settings.anti_oups, "Check each message with Anti-Oups! before it goes").changed() {
                    changed = true;
                }
            });
            ui.label(RichText::new("The text goes to the Antidote installed on this PC only (127.0.0.1), never elsewhere.").size(11.5).color(t::FAINT));
        });

        section(ui, "PICTURES FROM THE INTERNET");
        card(ui, |ui| {
            ui.label(RichText::new("Blocked in every message, except for these senders. Tracking pictures never load.").size(12.0).color(t::MUTED));
            if self.settings.picture_senders.is_empty() {
                ui.label(RichText::new("No sender approved: use \"Always for this sender\" on a message.").size(12.0).color(t::FAINT));
            }
            let mut remove: Option<String> = None;
            for s in &self.settings.picture_senders {
                ui.horizontal(|ui| {
                    ui.label(s);
                    if ui.small_button("Remove").clicked() {
                        remove = Some(s.clone());
                    }
                });
            }
            if let Some(s) = remove {
                self.settings.set_pictures_approved(&s, false);
                changed = true;
            }
        });

        if changed && let Err(e) = self.settings.save() {
            self.warn(e);
        }
    }

    /// The signature, the templates and the quick replies of `account`, each with its buttons.
    fn writing_lists(&mut self, ui: &mut Ui, account: AccountId) -> bool {
        let w = self.settings.writing(account).cloned();
        let signature = w.as_ref().map(|w| w.signature.clone()).unwrap_or_default();
        let templates = w.as_ref().map(|w| w.templates.clone()).unwrap_or_default();
        let replies = w.as_ref().map(|w| w.quick_replies.clone()).unwrap_or_default();
        let mut open: Option<(What, Document, String, String, String)> = None;
        let mut delete: Option<What> = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Signature").strong());
            let preview = mailalm_compose::plain_text(&import_html(&signature, &[]));
            let preview = if preview.trim().is_empty() { "none".to_owned() } else { preview.lines().take(2).collect::<Vec<_>>().join(" · ") };
            ui.label(RichText::new(preview).color(t::FAINT));
            if ui.small_button("Edit…").clicked() {
                open = Some((What::Signature, import_html(&signature, &[]), String::new(), String::new(), String::new()));
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Templates").strong());
        for (i, tpl) in templates.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&tpl.name);
                ui.label(RichText::new(&tpl.subject).color(t::FAINT));
                if ui.small_button("Edit…").clicked() {
                    open = Some((What::Template(Some(i)), import_html(&tpl.html, &[]), tpl.name.clone(), tpl.subject.clone(), String::new()));
                }
                if ui.small_button("Delete").clicked() {
                    delete = Some(What::Template(Some(i)));
                }
            });
        }
        if ui.small_button("New template…").clicked() {
            open = Some((What::Template(None), Document::new(), String::new(), String::new(), String::new()));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Quick replies").strong());
        for (i, q) in replies.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&q.name);
                ui.label(RichText::new(q.text.lines().next().unwrap_or("")).color(t::FAINT));
                if ui.small_button("Edit…").clicked() {
                    open = Some((What::QuickReply(Some(i)), Document::new(), q.name.clone(), String::new(), q.text.clone()));
                }
                if ui.small_button("Delete").clicked() {
                    delete = Some(What::QuickReply(Some(i)));
                }
            });
        }
        if ui.small_button("New quick reply…").clicked() {
            open = Some((What::QuickReply(None), Document::new(), String::new(), String::new(), String::new()));
        }
        if let Some((what, doc, name, subject, text)) = open {
            self.writing_page.editing =
                Some(Editing { account, what, name, subject, editor: Editor::from_document(doc), toolbar: ToolbarState::default(), text, error: None });
        }
        match delete {
            Some(What::Template(Some(i))) => {
                let w = self.settings.writing_mut(account);
                if i < w.templates.len() {
                    w.templates.remove(i);
                }
                self.writing_page.editing = None;
                true
            }
            Some(What::QuickReply(Some(i))) => {
                let w = self.settings.writing_mut(account);
                if i < w.quick_replies.len() {
                    w.quick_replies.remove(i);
                }
                self.writing_page.editing = None;
                true
            }
            _ => false,
        }
    }

    /// The signature, template or quick reply being edited; whether it was saved.
    fn writing_editor(&mut self, ui: &mut Ui) -> bool {
        let Some(e) = &mut self.writing_page.editing else { return false };
        let (mut save, mut cancel) = (false, false);
        Frame::new().fill(t::PANEL).stroke(Stroke::new(1.0, t::LINE)).corner_radius(6).inner_margin(Margin::same(10)).show(ui, |ui| {
            let title = match e.what {
                What::Signature => "Signature",
                What::Template(_) => "Template",
                What::QuickReply(_) => "Quick reply",
            };
            ui.label(RichText::new(title).strong());
            if e.what != What::Signature {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(TextEdit::singleline(&mut e.name).desired_width(240.0));
                });
            }
            if matches!(e.what, What::Template(_)) {
                ui.horizontal(|ui| {
                    ui.label("Subject");
                    ui.add(TextEdit::singleline(&mut e.subject).desired_width(f32::INFINITY));
                });
            }
            if matches!(e.what, What::QuickReply(_)) {
                ui.add(TextEdit::multiline(&mut e.text).desired_rows(4).desired_width(f32::INFINITY));
            } else {
                if toolbar(ui, &mut e.editor, &mut e.toolbar) == Some(ToolbarRequest::InsertImage) {
                    e.error = Some("Pictures are not kept in signatures and templates yet.".to_owned());
                }
                Frame::new().fill(t::PANEL2).inner_margin(Margin::same(6)).show(ui, |ui| {
                    RichEditor::new(&mut e.editor).id(Id::unique("mailalm-writing-editor")).min_height(110.0).show(ui);
                });
            }
            if let Some(err) = &e.error {
                ui.label(RichText::new(err).color(t::DANGER));
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.writing_page.editing = None;
            return false;
        }
        if !save {
            return false;
        }
        let html = if e.editor.document().is_empty() { String::new() } else { to_html(e.editor.document()) };
        if html.len() > MAX_SAVED || e.text.len() > MAX_SAVED {
            e.error = Some("This is too long to keep (256 KB at most).".to_owned());
            return false;
        }
        if e.what != What::Signature && e.name.trim().is_empty() {
            e.error = Some("Give it a name.".to_owned());
            return false;
        }
        let (account, what, name, subject, text) = (e.account, e.what, e.name.trim().to_owned(), e.subject.trim().to_owned(), e.text.clone());
        let w = self.settings.writing_mut(account);
        match what {
            What::Signature => w.signature = html,
            What::Template(at) => {
                let tpl = Template { name, subject, html };
                match at.and_then(|i| w.templates.get_mut(i)) {
                    Some(slot) => *slot = tpl,
                    None => w.templates.push(tpl),
                }
            }
            What::QuickReply(at) => {
                let q = QuickReply { name, text };
                match at.and_then(|i| w.quick_replies.get_mut(i)) {
                    Some(slot) => *slot = q,
                    None => w.quick_replies.push(q),
                }
            }
        }
        self.writing_page.editing = None;
        true
    }
}
