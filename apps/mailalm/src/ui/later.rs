//! Snooze and follow-ups (C2): the Snooze ▾ menu, "Remind me if no reply" in Flag ▾, the
//! "Snoozed and follow-ups" view with Wake now and Dismiss, and the window of the follow-ups
//! whose date came without an answer. MailALM keeps them in each account's cache (no provider has
//! an API for them); the account's worker moves the messages and checks every minute.

use egui::{Frame, Id, Margin, RichText, ScrollArea, TextEdit, Ui};
use mailalm_engine::{Command, LaterCommand};
use mailalm_model::AccountId;
use mailalm_model::later::{FollowUp, Mark};

use super::{MailApp, Page, View};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::later::{Preset, follow_up_presets, picked, snooze_presets};
use crate::local::LOCAL;
use crate::mailbox::MessageKey;
use crate::theme as t;
use crate::time;

/// What the snooze and follow-up menus and windows keep between frames.
#[derive(Default)]
pub(in crate::ui) struct LaterUi {
    snooze_date: String,
    snooze_clock: String,
    follow_date: String,
    follow_clock: String,
    /// The accounts the lists were asked of (asked again when one is added).
    asked: usize,
    /// The window of follow-ups due was closed; a new one opens it again.
    hidden: bool,
}

/// What the user chose in a menu.
enum Chosen {
    Snooze(i64),
    FollowUp(i64),
}

/// A menu's presets and its date picker; the UTC time chosen.
fn presets_ui(ui: &mut Ui, presets: &[Preset], date: &mut String, clock: &mut String, button: &str) -> Option<i64> {
    let mut chosen = None;
    for p in presets {
        let utc = time::utc_of_local(p.local);
        if ui.button(format!("{}  ·  {}", tr(p.label), time::long(utc))).clicked() {
            chosen = Some(utc);
            ui.close();
        }
    }
    ui.separator();
    ui.label(RichText::new(tr("Pick a date and time")).size(11.0).strong().color(t::faint()));
    ui.horizontal(|ui| {
        ui.add(TextEdit::singleline(date).hint_text("2026-12-31").desired_width(90.0));
        ui.add(TextEdit::singleline(clock).hint_text("09:00").desired_width(50.0));
        let at = picked(date, clock, time::local(time::now()));
        if ui.add_enabled(at.is_some(), egui::Button::new(button)).clicked()
            && let Some(local) = at
        {
            chosen = Some(time::utc_of_local(local));
            ui.close();
        }
    });
    chosen
}

impl MailApp {
    /// The message selected, if it is an account's (the mail on this PC has no snooze).
    fn later_target(&self) -> Option<MessageKey> {
        self.selected.clone().filter(|k| k.account != LOCAL)
    }

    /// Snooze ▾ in the command bar.
    pub(super) fn snooze_menu(&mut self, ui: &mut Ui) {
        let target = self.later_target();
        let mut chosen: Option<Chosen> = None;
        let LaterUi { snooze_date, snooze_clock, .. } = &mut self.later;
        let _ = ui.menu_button((tr("Snooze"), icons::atom(Icon::ChevronDown)), |ui| {
            if target.is_none() {
                ui.label(RichText::new(tr("Select a message of an account first.")).color(t::faint()));
                return;
            }
            ui.label(RichText::new(tr("The message leaves the Inbox and comes back, unread, at the time chosen.")).size(11.0).color(t::muted()));
            let presets = snooze_presets(time::local(time::now()));
            if let Some(at) = presets_ui(ui, &presets, snooze_date, snooze_clock, tr("Snooze")) {
                chosen = Some(Chosen::Snooze(at));
            }
        });
        if let (Some(Chosen::Snooze(until)), Some(key)) = (chosen, target) {
            self.snooze(&key, until);
        }
    }

    /// "Remind me if no reply" in Flag ▾ (a submenu).
    pub(super) fn follow_up_menu(&mut self, ui: &mut Ui) {
        let target = self.later_target();
        let mut chosen: Option<Chosen> = None;
        let LaterUi { follow_date, follow_clock, .. } = &mut self.later;
        let _ = ui.menu_button(tr("Remind me if no reply"), |ui| {
            if target.is_none() {
                ui.label(RichText::new(tr("Select a message of an account first.")).color(t::faint()));
                return;
            }
            ui.label(RichText::new(tr("MailALM reminds you then, unless someone answered in this thread.")).size(11.0).color(t::muted()));
            let presets = follow_up_presets(time::local(time::now()));
            if let Some(at) = presets_ui(ui, &presets, follow_date, follow_clock, tr("Remind me")) {
                chosen = Some(Chosen::FollowUp(at));
            }
        });
        if let (Some(Chosen::FollowUp(by)), Some(key)) = (chosen, target) {
            let command = Command::Later(LaterCommand::FollowUp { folder: key.folder.clone(), uid: key.uid, by });
            if self.engine.send(key.account, command) {
                self.say(trf!("MailALM reminds you on {date} if no answer came in this thread by then (while it runs, or at its next start).", date = time::long(by)));
            } else {
                self.warn(tr("The account is not running: no reminder was set."));
            }
        }
    }

    fn snooze(&mut self, key: &MessageKey, until: i64) {
        let command = Command::Later(LaterCommand::Snooze { folder: key.folder.clone(), uid: key.uid, until });
        if !self.engine.send(key.account, command) {
            self.warn(tr("The account is not running: the message was not snoozed."));
            return;
        }
        self.select_after_removal(key);
        self.drop_hit(key);
        let until = time::long(until);
        self.say(trf!("Snoozed until {date}: the message waits in MailALM/Snoozed and comes back to the Inbox, unread, then (if MailALM is closed, at its next start).", date = until));
    }

    /// Shows the snoozed messages and follow-ups, asked again of every account.
    pub(super) fn open_snoozed(&mut self) {
        for a in self.ws.accounts() {
            self.engine.send(a.id, Command::Later(LaterCommand::List));
        }
        self.view = View::Snoozed;
        self.page = Page::Mail;
        self.selected = None;
        self.reading = None;
    }

    /// A follow-up's date came without an answer: said, and its window shown.
    pub(super) fn follow_up_due(&mut self, account: AccountId, f: FollowUp) {
        let name = self.account(account).map(|a| a.name.clone()).unwrap_or_default();
        let text = if f.mark.subject.trim().is_empty() {
            trf!("{account}: no reply yet to a message.", account = name)
        } else {
            trf!("{account}: no reply yet to \"{subject}\".", account = name, subject = f.mark.subject)
        };
        self.say(text);
        self.later.hidden = false;
    }

    /// Opens the message a follow-up is about, where it was when it was set.
    fn open_follow_up(&mut self, account: AccountId, f: &FollowUp) {
        self.set_view(View::Folder(account, Some(f.folder.clone())));
        self.select(MessageKey { account, folder: f.folder.clone(), uid: f.uid });
    }

    /// The "Snoozed and follow-ups" view.
    pub(super) fn snoozed_page(&mut self, ui: &mut Ui) {
        let accounts: Vec<(AccountId, String, egui::Color32)> = self.ws.accounts().iter().map(|a| (a.id, a.name.clone(), t::account(a.color))).collect();
        let mut wake: Option<(AccountId, Mark)> = None;
        let mut dismiss: Option<(AccountId, Mark)> = None;
        let mut open: Option<(AccountId, FollowUp)> = None;
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(24, 18))).show(ui, |ui| {
            ui.label(RichText::new(tr("Snoozed and follow-ups")).size(20.0).strong());
            ui.label(
                RichText::new(tr("Kept by MailALM in each account's cache: a snoozed message waits in MailALM/Snoozed on the server; both are checked every minute while MailALM runs."))
                    .size(12.0)
                    .color(t::faint()),
            );
            ui.add_space(10.0);
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut any = false;
                for (id, name, color) in &accounts {
                    let Some(s) = self.boxes.account(*id) else { continue };
                    if s.snoozed.is_empty() && s.follow_ups.is_empty() {
                        continue;
                    }
                    any = true;
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 3.0, *color);
                        ui.label(RichText::new(name).strong());
                    });
                    let mut snoozed = s.snoozed.clone();
                    snoozed.sort_by_key(|x| x.until);
                    for x in &snoozed {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr("Snoozed")).size(11.0).color(t::accent_text()));
                            let subject = if x.mark.subject.is_empty() { tr("(no subject)") } else { x.mark.subject.as_str() };
                            ui.label(subject);
                            ui.label(RichText::new(trf!("{sender} · back {date}", sender = x.mark.from.display(), date = time::long(x.until))).size(12.0).color(t::faint()));
                            if ui.small_button(tr("Wake now")).clicked() {
                                wake = Some((*id, x.mark.clone()));
                            }
                        });
                    }
                    let mut follow = s.follow_ups.clone();
                    follow.sort_by_key(|x| x.by);
                    for f in &follow {
                        ui.horizontal(|ui| {
                            let (tag, color) = if f.fired { (tr("No reply"), t::warn()) } else { (tr("Follow-up"), t::accent_text()) };
                            ui.label(RichText::new(tag).size(11.0).color(color));
                            let subject = if f.mark.subject.is_empty() { tr("(no subject)") } else { f.mark.subject.as_str() };
                            ui.label(subject);
                            ui.label(RichText::new(trf!("reminder {date}", date = time::long(f.by))).size(12.0).color(t::faint()));
                            if ui.small_button(tr("Open")).clicked() {
                                open = Some((*id, f.clone()));
                            }
                            if ui.small_button(tr("Dismiss")).clicked() {
                                dismiss = Some((*id, f.mark.clone()));
                            }
                        });
                    }
                    ui.add_space(10.0);
                }
                if !any {
                    ui.label(RichText::new(tr("Nothing is snoozed or followed. Snooze ▾ puts a message aside; Flag ▾, Remind me if no reply, follows one up.")).color(t::faint()));
                }
            });
        });
        if let Some((id, mark)) = wake {
            if self.engine.send(id, Command::Later(LaterCommand::Wake(Box::new(mark)))) {
                self.say(tr("The message is coming back to the Inbox."));
            } else {
                self.warn(tr("The account is not running."));
            }
        }
        if let Some((id, mark)) = dismiss {
            self.engine.send(id, Command::Later(LaterCommand::Dismiss(Box::new(mark))));
        }
        if let Some((id, f)) = open {
            self.open_follow_up(id, &f);
        }
    }

    /// Asks every account for its lists when one is added (at start too), and shows the
    /// follow-ups due.
    pub(super) fn later_windows(&mut self, ui: &mut Ui) {
        let n = self.ws.accounts().len();
        if self.later.asked != n {
            for a in self.ws.accounts() {
                self.engine.send(a.id, Command::Later(LaterCommand::List));
            }
            self.later.asked = n;
        }
        if self.later.hidden {
            return;
        }
        let due: Vec<(AccountId, String, FollowUp)> = self
            .ws
            .accounts()
            .iter()
            .filter_map(|a| self.boxes.account(a.id).map(|s| (a, s)))
            .flat_map(|(a, s)| s.follow_ups.iter().filter(|f| f.fired).map(|f| (a.id, a.name.clone(), f.clone())).collect::<Vec<_>>())
            .collect();
        if due.is_empty() {
            return;
        }
        let mut open: Option<(AccountId, FollowUp)> = None;
        let mut dismiss: Option<(AccountId, Mark)> = None;
        let mut close = false;
        egui::Window::new(tr("No reply yet")).id(Id::unique("mailalm-follow-ups")).collapsible(true).resizable(true).default_width(460.0).show(ui.ctx(), |ui| {
            ui.label(RichText::new(tr("No answer came in these threads by the date you chose.")).size(12.0).color(t::muted()));
            ui.add_space(4.0);
            ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (id, name, f) in &due {
                    ui.horizontal(|ui| {
                        let subject = if f.mark.subject.is_empty() { tr("(no subject)") } else { f.mark.subject.as_str() };
                        ui.label(RichText::new(subject).strong());
                        ui.label(RichText::new(format!("{name} · {}", time::long(f.by))).size(12.0).color(t::faint()));
                        if ui.small_button(tr("Open")).clicked() {
                            open = Some((*id, f.clone()));
                        }
                        if ui.small_button(tr("Dismiss")).clicked() {
                            dismiss = Some((*id, f.mark.clone()));
                        }
                    });
                }
            });
            ui.add_space(4.0);
            if ui.button(tr("Later")).on_hover_text(tr("Hide this window; the reminders stay in Snoozed and follow-ups")).clicked() {
                close = true;
            }
        });
        if let Some((id, f)) = open {
            self.page = Page::Mail;
            self.open_follow_up(id, &f);
        }
        if let Some((id, mark)) = dismiss {
            self.engine.send(id, Command::Later(LaterCommand::Dismiss(Box::new(mark))));
        }
        if close {
            self.later.hidden = true;
        }
    }
}
