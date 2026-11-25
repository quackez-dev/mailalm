//! The reading pane: the message's header, its security banners (the sender that fails DMARC,
//! the pictures from the internet), its attachments (locked until reviewed), its invitation, and
//! its text (`crate::reading`); leaving its mailing list after a confirmation that says exactly
//! what happens (S4).

use egui::{Align, Align2, Color32, FontId, Frame, Id, Layout, Margin, RichText, ScrollArea, Sense, Stroke, Ui, Vec2};
use mailalm_engine::{Command, UnsubscribeHow};
use mailalm_mime::{AttachmentInfo, Outgoing, Unsubscribe};
use mailalm_model::Person;

use super::{MailApp, avatar_color, initials, people, pill, size_text};
use crate::agenda::Ctx;
use crate::attachments::{self, Review};
use crate::icons::{self, Icon};
use crate::mailbox::MessageKey;
use crate::reading::{MessageView, ViewAction};
use crate::theme as t;
use crate::time;

/// The red of the DMARC banner.
const ALERT_BG: Color32 = Color32::from_rgb(0x3b, 0x16, 0x16);

/// A mailing list to leave, waiting for the user's confirmation.
pub(super) struct LeaveList {
    key: MessageKey,
    how: Unsubscribe,
    /// The address the account writes from.
    me: String,
}

impl MailApp {
    /// The view of the message being read, made when it has arrived.
    fn refresh_view(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.reading else {
            self.message_view = None;
            return;
        };
        let Some(parsed) = &r.parsed else { return };
        if self.message_view.as_ref().is_some_and(|v| v.key == r.key) {
            return;
        }
        self.next_view += 1;
        let mut view = MessageView::new(r.key.clone(), r.html.take(), self.next_view);
        if self.settings.pictures_approved(&parsed.from.address) {
            view.load_pictures(ctx);
        }
        self.message_view = Some(view);
    }

    pub(super) fn reading_pane(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.refresh_view(&ctx);
        // Review… clicked: which attachment, from whom, and the message's sender checks.
        let mut review_request: Option<(usize, AttachmentInfo, Person, mailalm_safety::Authentication)> = None;
        let mut action: Option<&'static str> = None;
        let mut view_action: Option<ViewAction> = None;
        // The agenda draws a message's invitation, and the view the text; they are taken out
        // while the pane borrows the rest.
        let mut pim = std::mem::take(&mut self.pim);
        let mut view = self.message_view.take();
        if let Some(v) = &mut view {
            v.poll();
        }
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(24, 18))).show(ui, |ui| {
            let Some(r) = &self.reading else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Select a message").color(t::faint()));
                });
                return;
            };
            let summary = self.summary_of(&r.key).cloned();
            let account = self.account(r.key.account).cloned();
            let Some(parsed) = &r.parsed else {
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    match &r.error {
                        Some(e) => {
                            ui.label(RichText::new(e).color(t::danger()));
                        }
                        None => {
                            ui.spinner();
                            ui.label(RichText::new("Loading the message…").color(t::faint()));
                        }
                    }
                });
                return;
            };
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let subject = if parsed.subject.is_empty() { "(no subject)" } else { parsed.subject.as_str() };
                    ui.label(RichText::new(subject).size(20.0).strong());
                    if summary.as_ref().is_some_and(|s| s.flags.flagged) {
                        ui.label(RichText::new("⚑").size(16.0).color(t::danger()));
                    }
                    if let Some(a) = &account {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            Frame::new().fill(t::raised()).stroke(Stroke::new(1.0, t::line())).corner_radius(12).inner_margin(Margin::symmetric(10, 3)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 4.0, t::account(a.color));
                                    ui.label(RichText::new(format!("{} · {}", a.name, a.address)).size(12.0));
                                });
                            });
                        });
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let from = parsed.from.display().to_owned();
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 20.0, avatar_color(&parsed.from.address));
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, initials(&from), FontId::proportional(14.0), Color32::WHITE);
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&from).strong());
                            if !parsed.from.address.is_empty() && parsed.from.address != from {
                                ui.label(RichText::new(format!("<{}>", parsed.from.address)).color(t::muted()));
                            }
                        });
                        let date = parsed.date.or(summary.as_ref().map(|s| s.date)).unwrap_or(0);
                        ui.label(RichText::new(time::long(date)).size(12.0).color(t::muted()));
                        if !parsed.to.is_empty() {
                            ui.label(RichText::new(format!("To: {}", people(&parsed.to))).size(12.0).color(t::muted()));
                        }
                        if !parsed.cc.is_empty() {
                            ui.label(RichText::new(format!("Cc: {}", people(&parsed.cc))).size(12.0).color(t::muted()));
                        }
                    });
                });
                ui.add_space(10.0);
                let auth = mailalm_safety::authentication(parsed.authentication_results.as_deref());
                if let Some(alert) = mailalm_safety::dmarc_alert(&auth, &parsed.from.address) {
                    Frame::new().fill(ALERT_BG).stroke(Stroke::new(1.0, t::danger())).corner_radius(6).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(format!("This message may not come from {}", alert.domain)).strong().color(t::danger()));
                        let mut text = format!("It says it comes from {}, but that domain's DMARC check failed", alert.domain);
                        if let Some(by) = &alert.checked_by {
                            text.push_str(&format!(" (checked by {by})"));
                        }
                        if let Some(action) = &alert.action {
                            text.push_str(&format!("; the domain asks for such mail to be handled as: {action}"));
                        }
                        text.push_str(". Don't trust its links, its attachments or what it asks of you.");
                        ui.label(RichText::new(text).size(12.5).color(t::text()));
                    });
                    ui.add_space(8.0);
                }
                ui.horizontal_wrapped(|ui| {
                    let formatted = view.as_ref().is_some_and(MessageView::formatted);
                    if formatted {
                        pill(ui, "Formatted by MailALM · no web content or scripts", t::muted(), t::raised());
                    } else if parsed.from_html {
                        pill(ui, "Shown as text · no web content, pictures or scripts loaded", t::muted(), t::raised());
                    }
                    attachments::auth_pills(ui, &auth);
                });
                if let Some(v) = &mut view
                    && v.key == r.key
                {
                    ui.add_space(6.0);
                    if let Some(a) = v.pictures_banner(ui, &parsed.from.address) {
                        view_action = Some(a);
                    }
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for label in ["Reply", "Reply all", "Forward"] {
                        if ui.button(label).clicked() {
                            action = Some(label);
                        }
                    }
                    if parsed.unsubscribe.is_some() && ui.button("Unsubscribe…").on_hover_text("Leave the mailing list this message came from").clicked() {
                        action = Some("Unsubscribe");
                    }
                });
                ui.add_space(12.0);
                ui.separator();
                if !parsed.attachments.is_empty() {
                    let n = parsed.attachments.len();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        icons::paint(ui.painter(), r, Icon::Lock, t::warn());
                        ui.label(RichText::new(format!("{n} attachment{} · locked until you review each one", if n > 1 { "s" } else { "" })).size(12.0).color(t::muted()));
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for (index, a) in parsed.attachments.iter().enumerate() {
                            let reviewed = self.reviewed.contains(&(r.key.clone(), index));
                            Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(8).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                                    ui.painter().rect_filled(rect, 6.0, if reviewed { t::raised() } else { t::warn_bg() });
                                    icons::paint(ui.painter(), rect, if reviewed { Icon::Check } else { Icon::Lock }, attachments::lock_color(reviewed));
                                    ui.vertical(|ui| {
                                        ui.label(RichText::new(&a.name).strong());
                                        ui.label(RichText::new(format!("{} · {} (declared)", size_text(a.size), a.declared_type)).size(12.0).color(t::faint()));
                                    });
                                    let review = ui.button("Review…").on_hover_text("Check what this file really is, then preview, save or open it");
                                    if review.clicked() {
                                        review_request = Some((index, a.clone(), parsed.from.clone(), auth.clone()));
                                    }
                                });
                            });
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                }
                let cx = Ctx { ws: &self.ws, engine: &self.engine };
                pim.invitation_panel(ui, &cx, &r.key, parsed);
                ui.add_space(12.0);
                match &mut view {
                    Some(v) if v.key == r.key => v.body(ui, &parsed.text),
                    _ => {
                        ui.add(egui::Label::new(RichText::new(&parsed.text).size(14.0)).selectable(true).wrap());
                    }
                }
            });
        });
        if let Some(v) = &mut view
            && let Some(a) = v.link_dialog(&ctx)
        {
            view_action = Some(a);
        }
        self.pim = pim;
        self.message_view = view;
        match action {
            Some("Reply") => self.reply(false, false),
            Some("Reply all") => self.reply(true, false),
            Some("Forward") => self.reply(false, true),
            Some("Unsubscribe") => self.ask_unsubscribe(),
            _ => {}
        }
        if let Some(a) = view_action {
            self.view_action(a);
        }
        if let Some((index, info, sender, auth)) = review_request
            && let Some(key) = self.reading.as_ref().map(|r| r.key.clone())
        {
            self.start_review(key, index, info, sender, auth);
        }
        self.unsubscribe_dialog(&ctx);
    }

    fn view_action(&mut self, a: ViewAction) {
        match a {
            ViewAction::OpenWeb(url) => {
                if let Err(e) = crate::accounts::open_web_link(&url) {
                    self.warn(format!("The link was not opened: {e}."));
                }
            }
            ViewAction::Write { to, subject, body } => {
                let Some(account) = self.reading.as_ref().map(|r| r.key.account) else { return };
                let me = self.me(account);
                let o = Outgoing { from: me, to: vec![Person { name: String::new(), address: to }], subject, text: body, ..Outgoing::default() };
                self.compose(account, o, None);
            }
            ViewAction::ApproveSender => {
                let Some(sender) = self.reading.as_ref().and_then(|r| r.parsed.as_ref()).map(|p| p.from.address.clone()) else { return };
                self.settings.set_pictures_approved(&sender, true);
                match self.settings.save() {
                    Ok(()) => self.say(format!("The pictures of {sender}'s messages will load without asking. Settings, Writing and reading lists the senders approved.")),
                    Err(e) => self.warn(e),
                }
            }
        }
    }

    /// Asks the account's worker for one attachment, and opens its approval dialog.
    pub(super) fn start_review(&mut self, key: MessageKey, index: usize, info: AttachmentInfo, sender: Person, auth: mailalm_safety::Authentication) {
        let command = Command::Attachment { folder: key.folder.clone(), uid: key.uid, index };
        if self.send(key.account, command) {
            self.review = Some(Review::new(key, index, info, sender, auth));
        } else {
            self.warn("The account is not running: the attachment cannot be read.");
        }
    }

    fn ask_unsubscribe(&mut self) {
        let Some(r) = &self.reading else { return };
        let Some(how) = r.parsed.as_ref().and_then(|p| p.unsubscribe.clone()) else { return };
        let me = self.me(r.key.account).address;
        self.leave_list = Some(LeaveList { key: r.key.clone(), how, me });
    }

    /// The confirmation to leave a mailing list: it says exactly what MailALM will do.
    fn unsubscribe_dialog(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.leave_list else { return };
        let mut chosen: Option<Option<UnsubscribeHow>> = None;
        let mut web: Option<String> = None;
        let m = egui::Modal::new(Id::unique("mailalm-unsubscribe")).show(ctx, |ui| {
            ui.set_width(500.0);
            ui.heading("Leave this mailing list?");
            ui.add_space(8.0);
            if let Some(url) = &l.how.one_click {
                let host = Unsubscribe::host_of(url);
                ui.label(RichText::new("In one click").strong());
                ui.label(format!(
                    "MailALM sends one request to {host}: a POST holding only the words \"List-Unsubscribe=One-Click\" (no cookie, nothing about you). The list then stops writing to {}.",
                    l.me
                ));
                if ui.button(format!("Unsubscribe at {host}")).clicked() {
                    chosen = Some(Some(UnsubscribeHow::OneClick));
                }
                ui.add_space(8.0);
            }
            if let Some(mail) = &l.how.mailto {
                ui.label(RichText::new("By mail").strong());
                let subject = if mail.subject.is_empty() { "(no subject)".to_owned() } else { format!("\"{}\"", mail.subject) };
                ui.label(format!("MailALM sends a message from {} to {}, subject {subject}, as the list asks.", l.me, mail.to));
                if ui.button("Send the unsubscribe message").clicked() {
                    chosen = Some(Some(UnsubscribeHow::Mail));
                }
                ui.add_space(8.0);
            }
            if l.how.one_click.is_none()
                && l.how.mailto.is_none()
                && let Some(url) = &l.how.web
            {
                ui.label(RichText::new("On its web page").strong());
                ui.label(format!("This list unsubscribes on its web page, at {}. MailALM opens it in your browser; finish there.", Unsubscribe::host_of(url)));
                if ui.button("Open the page").clicked() {
                    web = Some(url.clone());
                }
                ui.add_space(8.0);
            }
            if ui.button("Cancel").clicked() {
                chosen = Some(None);
            }
        });
        if m.should_close() && chosen.is_none() && web.is_none() {
            chosen = Some(None);
        }
        if let Some(url) = web {
            self.leave_list = None;
            if let Err(e) = crate::accounts::open_web_link(&url) {
                self.warn(format!("The page was not opened: {e}."));
            }
            return;
        }
        let Some(choice) = chosen else { return };
        let Some(l) = self.leave_list.take() else { return };
        if let Some(how) = choice {
            let command = Command::Unsubscribe { folder: l.key.folder.clone(), uid: l.key.uid, how };
            if self.engine.send(l.key.account, command) {
                self.say("Unsubscribing…");
            } else {
                self.warn("The account is not running: nothing was sent.");
            }
        }
    }
}
