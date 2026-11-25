//! Sending (C1, PLAN.md section 8): the compose windows and what they ask, the undo bar (a
//! message sent waits for its delay, with a countdown, Undo and Send now), the scheduled messages
//! (sent when due; those whose time passed while MailALM was closed are sent at start, and said),
//! and the drafts kept on the server.

use std::time::Duration;

use egui::{Align2, Frame, Id, Margin, RichText, Stroke, Ui, vec2};
use mailalm_compose::Document;
use mailalm_engine::{Command, SizeLimit};
use mailalm_mime::Outgoing;
use mailalm_model::AccountId;

use super::MailApp;
use crate::compose::{AccountChoice, Compose, ComposeAction, ComposeCtx};
use crate::send_queue::{Schedule, Scheduled, UndoQueue, sent_late};
use crate::settings::Settings;
use crate::theme as t;
use crate::time;

/// A message waiting for its undo delay, with its window to bring back.
pub(super) struct Waiting {
    compose: Box<Compose>,
    message: Box<Outgoing>,
}

/// What the main window keeps about sending.
pub(super) struct Sending {
    undo: UndoQueue<Waiting>,
    schedule: Schedule,
    /// Scheduled messages sent at start, their time passed while MailALM was closed.
    late: Vec<String>,
    /// Antidote's connector is installed (looked at once, at start).
    pub antidote: bool,
}

impl Sending {
    /// The scheduled messages kept in the cache folder; what went wrong reading them.
    pub(super) fn load() -> (Sending, Option<String>) {
        let (schedule, problem) = Schedule::load(Settings::cache_dir().join("scheduled.json"));
        (Sending { undo: UndoQueue::default(), schedule, late: Vec::new(), antidote: mailalm_antidote::installed().is_ok() }, problem)
    }
}

impl MailApp {
    /// A compose window for `o` from `account`: `doc` is its text when the caller built it (a new
    /// message, a reply, a forward). The account's signature goes in.
    pub(super) fn compose(&mut self, account: AccountId, o: Outgoing, doc: Option<Document>) {
        let id = self.next_compose;
        self.next_compose += 1;
        let mut c = Compose::new(id, account, &o, None, doc, self.settings.plain_text);
        c.put_signature(self.settings.writing(account));
        self.composes.push(c);
        self.create_menu = None;
    }

    pub(super) fn compose_windows(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.composes.is_empty() || !self.pim.editors.is_empty() {
            self.pim.refresh_book(&self.boxes);
        }
        let accounts: Vec<AccountChoice> = self
            .ws
            .accounts()
            .iter()
            .map(|a| AccountChoice { id: a.id, name: a.name.clone(), address: a.address.clone(), color: t::account(a.color), limit: SizeLimit::of(a.provider) })
            .collect();
        let mut results: Vec<(usize, ComposeAction)> = Vec::new();
        {
            let book = &self.pim.book;
            let suggest = |typed: &str| book.suggest(typed, 8);
            let cx = ComposeCtx { accounts: &accounts, writing: &self.settings.writing, suggest: &suggest, antidote: self.sending.antidote, anti_oups: self.settings.anti_oups };
            for (n, c) in self.composes.iter_mut().enumerate() {
                if c.closing {
                    continue;
                }
                let a = c.show(&ctx, &cx);
                if !matches!(a, ComposeAction::None) {
                    results.push((n, a));
                }
            }
        }
        // From the last, so that the indexes stay right as windows go.
        for (n, a) in results.into_iter().rev() {
            match a {
                ComposeAction::None => {}
                ComposeAction::Send(message) => {
                    let mut compose = Box::new(self.composes.remove(n));
                    compose.closing = true;
                    let delay = self.settings.undo_delay();
                    let now = ctx.input(|i| i.time);
                    let subject = message.subject.clone();
                    self.sending.undo.push(Waiting { compose, message }, now, f64::from(delay));
                    self.say(format!("\"{subject}\" goes in {delay} seconds: Undo brings it back."));
                }
                ComposeAction::Schedule(message, at) => {
                    let draft = self.composes.get(n).and_then(|c| c.draft.clone());
                    let account = self.composes.get(n).map_or(AccountId(0), |c| c.account);
                    // A draft on another account than the sender's stays where it is.
                    let draft_id = draft.filter(|(a, _)| *a == account).map(|(_, id)| id);
                    match self.sending.schedule.add(account, at, &message, draft_id) {
                        Ok(_) => {
                            let mut c = self.composes.remove(n);
                            c.close_antidote();
                            self.say(format!("\"{}\" will go on {}. MailALM sends it then if it is running, else as soon as it starts.", message.subject, time::long(at)));
                        }
                        Err(e) => {
                            if let Some(c) = self.composes.get_mut(n) {
                                c.reopen_with_error(e);
                            }
                        }
                    }
                }
                ComposeAction::SaveDraft(message) => self.save_draft(n, message),
                ComposeAction::Close(message) => {
                    if let Some(c) = self.composes.get_mut(n) {
                        c.close_antidote();
                    }
                    self.save_draft(n, message);
                }
                ComposeAction::Discard => {
                    let mut c = self.composes.remove(n);
                    c.close_antidote();
                    if let Some((account, id)) = c.draft.take() {
                        self.engine.send(account, Command::DeleteDraft(id));
                    }
                }
            }
        }
    }

    /// Asks the account's worker to keep window `n`'s draft, in place of the one saved before.
    fn save_draft(&mut self, n: usize, message: Box<Outgoing>) {
        let Some(c) = self.composes.get_mut(n) else { return };
        let account = c.account;
        let replace = match c.draft.take() {
            Some((a, id)) if a == account => Some(id),
            Some((a, id)) => {
                // Saved before from another account: deleted there.
                self.engine.send(a, Command::DeleteDraft(id));
                None
            }
            None => None,
        };
        let command = Command::SaveDraft { compose: c.id, message, replace: replace.clone() };
        if !self.engine.send(account, command) {
            c.draft = replace.map(|id| (account, id));
            if c.closing {
                c.reopen_with_error("The account is not running: the draft could not be kept on the server.".to_owned());
            } else {
                c.draft_saved(Err("The account is not running: the draft could not be kept on the server.".to_owned()));
            }
        }
    }

    /// A draft saved, or not: said in its window; a window closed is forgotten once its draft is
    /// kept, and shown again if it could not be.
    pub(super) fn draft_saved(&mut self, compose: u64, result: Result<String, String>) {
        let Some(n) = self.composes.iter().position(|c| c.id == compose) else {
            if let Err(e) = result {
                self.warn(e);
            }
            return;
        };
        let closing = self.composes.get(n).is_some_and(|c| c.closing);
        match (closing, result) {
            (true, Ok(_)) => {
                let c = self.composes.remove(n);
                self.say(format!("\"{}\" was kept in Drafts.", c.subject().trim()));
            }
            (true, Err(e)) => {
                if let Some(c) = self.composes.get_mut(n) {
                    c.reopen_with_error(e);
                }
            }
            (false, result) => {
                if let Some(c) = self.composes.get_mut(n) {
                    c.draft_saved(result);
                }
            }
        }
    }

    /// The undo delays and the scheduled times: what is due goes. Then the undo bar, the
    /// scheduled list and the late messages.
    pub(super) fn sending_tick(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        for w in self.sending.undo.take_due(now) {
            self.send_waiting(w);
        }
        let utc = time::now();
        match self.sending.schedule.take_due(utc) {
            Ok(due) => {
                for s in due {
                    self.send_scheduled(s, utc);
                }
            }
            Err(e) => self.warn(e),
        }
        if !self.sending.undo.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if !self.sending.schedule.list().is_empty() {
            ctx.request_repaint_after(Duration::from_secs(15));
        }
        self.send_bar(&ctx);
        self.late_dialog(&ctx);
    }

    /// A message whose undo delay passed: to the account's worker, and its draft deleted.
    fn send_waiting(&mut self, w: Waiting) {
        let Waiting { mut compose, message } = w;
        let account = compose.account;
        let subject = message.subject.clone();
        if self.engine.send(account, Command::Send(message)) {
            compose.close_antidote();
            if let Some((a, id)) = compose.draft.take() {
                self.engine.send(a, Command::DeleteDraft(id));
            }
            self.say(format!("Sending \"{subject}\"…"));
        } else {
            compose.reopen_with_error("The account is not running: the message was not sent.".to_owned());
            self.composes.push(*compose);
        }
    }

    /// A scheduled message whose time came (or passed while MailALM was closed).
    fn send_scheduled(&mut self, s: Scheduled, now: i64) {
        let Some(message) = s.message.outgoing() else {
            self.warn(format!("The message scheduled for {} was damaged on disk and was not sent.", time::long(s.at)));
            return;
        };
        let subject = message.subject.clone();
        if self.engine.send(s.account, Command::Send(Box::new(message.clone()))) {
            if let Some(id) = s.draft {
                self.engine.send(s.account, Command::DeleteDraft(id));
            }
            if sent_late(s.at, now) {
                self.sending.late.push(format!("\"{subject}\", planned for {}", time::long(s.at)));
            } else {
                self.say(format!("Scheduled message sent: \"{subject}\"."));
            }
        } else {
            let id = self.next_compose;
            self.next_compose += 1;
            let text = format!("The message scheduled for {} was not sent: its account is not running.", time::long(s.at));
            let plain = message.html.is_none();
            self.composes.push(Compose::new(id, s.account, &message, Some(text.clone()), None, plain));
            self.warn(text);
        }
    }

    /// The messages waiting for their undo delay (countdown, Undo, Send now) and the scheduled
    /// ones, above the status bar.
    fn send_bar(&mut self, ctx: &egui::Context) {
        let waiting: Vec<(u64, String, f64)> =
            self.sending.undo.iter().map(|p| (p.id, p.item.message.subject.clone(), self.sending.undo.remaining(p.id, ctx.input(|i| i.time)).unwrap_or(0.0))).collect();
        let scheduled: Vec<(u64, String, i64)> = self.sending.schedule.list().iter().map(|s| (s.id, s.message.subject.clone(), s.at)).collect();
        if waiting.is_empty() && scheduled.is_empty() {
            return;
        }
        let mut undo: Option<u64> = None;
        let mut now: Option<u64> = None;
        let mut edit: Option<u64> = None;
        let mut send_scheduled: Option<u64> = None;
        egui::Area::new(Id::unique("mailalm-send-bar")).order(egui::Order::Foreground).anchor(Align2::CENTER_BOTTOM, vec2(0.0, -36.0)).show(ctx, |ui| {
            Frame::new().fill(t::RAISED).stroke(Stroke::new(1.0, t::LINE)).corner_radius(8).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                for (id, subject, left) in &waiting {
                    ui.horizontal(|ui| {
                        let subject = if subject.trim().is_empty() { "(no subject)" } else { subject.as_str() };
                        ui.label(RichText::new(format!("Sending \"{subject}\" in {} s", left.ceil() as u64)).strong());
                        if ui.button("Undo").clicked() {
                            undo = Some(*id);
                        }
                        if ui.small_button("Send now").clicked() {
                            now = Some(*id);
                        }
                    });
                }
                if !scheduled.is_empty() {
                    let _ = ui.menu_button(format!("{} scheduled message{}", scheduled.len(), if scheduled.len() == 1 { "" } else { "s" }), |ui| {
                        ui.label(RichText::new("Sent by MailALM at their time if it is running, else as soon as it starts.").size(11.5).color(t::FAINT));
                        for (id, subject, at) in &scheduled {
                            ui.horizontal(|ui| {
                                ui.label(format!("{} · {}", time::long(*at), if subject.trim().is_empty() { "(no subject)" } else { subject.as_str() }));
                                if ui.small_button("Edit").on_hover_text("Take it out of the schedule, back in its window").clicked() {
                                    edit = Some(*id);
                                    ui.close();
                                }
                                if ui.small_button("Send now").clicked() {
                                    send_scheduled = Some(*id);
                                    ui.close();
                                }
                            });
                        }
                    });
                }
            });
        });
        if let Some(id) = undo
            && let Some(w) = self.sending.undo.take(id)
        {
            let mut c = *w.compose;
            c.reopen("Not sent. Change it and send it again, or discard it.");
            self.composes.push(c);
        }
        if let Some(id) = now
            && let Some(w) = self.sending.undo.take(id)
        {
            self.send_waiting(w);
        }
        for (id, at_once) in [(edit, false), (send_scheduled, true)] {
            let Some(id) = id else { continue };
            match self.sending.schedule.take(id) {
                Ok(Some(s)) if at_once => self.send_scheduled(Scheduled { at: time::now(), ..s }, time::now()),
                Ok(Some(s)) => match s.message.outgoing() {
                    Some(message) => {
                        let cid = self.next_compose;
                        self.next_compose += 1;
                        let mut c = Compose::new(cid, s.account, &message, None, None, message.html.is_none());
                        c.draft = s.draft.map(|d| (s.account, d));
                        c.reopen("Taken out of the schedule: send it, or schedule it again.");
                        self.composes.push(c);
                    }
                    None => self.warn("This scheduled message was damaged on disk."),
                },
                Ok(None) => {}
                Err(e) => self.warn(e),
            }
        }
    }

    /// The scheduled messages sent at start, their time passed while MailALM was closed.
    fn late_dialog(&mut self, ctx: &egui::Context) {
        if self.sending.late.is_empty() {
            return;
        }
        let mut ok = false;
        let m = egui::Modal::new(Id::unique("mailalm-sent-late")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Scheduled messages sent late");
            ui.add_space(6.0);
            ui.label("Their time passed while MailALM was closed: MailALM sent them as it started, just now.");
            ui.add_space(6.0);
            for line in &self.sending.late {
                ui.label(RichText::new(line).strong());
            }
            ui.add_space(10.0);
            if ui.button("OK").clicked() {
                ok = true;
            }
        });
        if ok || m.should_close() {
            self.sending.late.clear();
        }
    }

    /// Forgets what an account removed had waiting.
    pub(super) fn forget_sending(&mut self, account: AccountId) {
        if let Err(e) = self.sending.schedule.forget(account) {
            self.warn(e);
        }
        loop {
            // The id is taken first: the search's borrow ends before the message is taken out.
            let next = self.sending.undo.iter().find(|p| p.item.compose.account == account).map(|p| p.id);
            let Some(id) = next else { break };
            let _ = self.sending.undo.take(id);
        }
    }
}
