//! An invitation in the reading pane (PLAN.md, section 7): when a message has a calendar part (an
//! invitation, an update, a cancellation, an answer), that part alone is decoded at the reading
//! pane's request and read with MailALM's own iCalendar reader (`mailalm_model::pim::ical`), which
//! cleans and caps everything. The panel shows the date, time, place, link (by its domain, opened
//! only on a click), organizer, guests, and what it clashes with in every calendar of every
//! account; then Accept, Tentative, Decline and Propose another time, the reply to the organizer
//! ticked by default. Nothing enters the calendar before the user answers; an update or a
//! cancellation shows what changes, and is applied only once confirmed.

use egui::{Button, Frame, Margin, RichText, Stroke, TextEdit, Ui};
use mailalm_engine::{Command, PimCommand, is_calendar_part};
use mailalm_mime::Parsed;
use mailalm_model::pim::ical::{Invitation, Method};
use mailalm_model::pim::{Event as CalendarEvent, Response, parse_date};
use mailalm_model::Provider;

use crate::agenda::{Ctx, Pim, link_domain, open_link, sources};
use crate::calendar::{busy_label, repeat_text, response_label};
use crate::mailbox::MessageKey;
use crate::theme as t;
use crate::time;
use crate::zones::TzRules;

/// What was read of a message's calendar part.
pub enum InvitationState {
    Loading,
    Failed(String),
    Ready(Box<Ready>),
}

/// An invitation read, and the user's choices on it.
pub struct Ready {
    invitation: Invitation,
    send: bool,
    comment: String,
    proposing: bool,
    proposal_date: String,
    proposal_start: String,
    proposal_end: String,
    confirm: bool,
    /// What is wrong with the time proposed.
    problem: Option<String>,
    /// What was done (the panel then only says so).
    done: Option<String>,
}

impl InvitationState {
    pub fn read(result: Result<Box<Invitation>, String>) -> InvitationState {
        match result {
            Err(e) => InvitationState::Failed(e),
            Ok(invitation) => {
                let main = main_event(&invitation).cloned().unwrap_or_default();
                let (date, start, end) = if main.all_day {
                    (time::date(main.start.div_euclid(86_400)), "09:00".to_owned(), "10:00".to_owned())
                } else {
                    (time::date(time::local_day(main.start)), time::clock(main.start), time::clock(main.end))
                };
                InvitationState::Ready(Box::new(Ready {
                    invitation: *invitation,
                    send: true,
                    comment: String::new(),
                    proposing: false,
                    proposal_date: date,
                    proposal_start: start,
                    proposal_end: end,
                    confirm: false,
                    problem: None,
                    done: None,
                }))
            }
        }
    }
}

/// The event an invitation is about: its series (or single event), else its first.
fn main_event(inv: &Invitation) -> Option<&CalendarEvent> {
    inv.events.iter().find(|e| e.recurrence_id.is_none()).or_else(|| inv.events.first())
}

/// What a calendar's update changes, in words.
fn changes(old: &CalendarEvent, new: &CalendarEvent) -> Vec<String> {
    let mut out = Vec::new();
    if old.start != new.start || old.end != new.end || old.all_day != new.all_day {
        out.push(format!("Time: {} → {}", time::span(old.start, old.end, old.all_day), time::span(new.start, new.end, new.all_day)));
    }
    if old.location != new.location {
        out.push(format!("Place: {} → {}", if old.location.is_empty() { "(none)" } else { &old.location }, if new.location.is_empty() { "(none)" } else { &new.location }));
    }
    if old.title != new.title {
        out.push(format!("Title: {} → {}", old.title, new.title));
    }
    if old.recurrence != new.recurrence {
        out.push("Repetition changed".to_owned());
    }
    out
}

/// What the user did on the panel, done once it is drawn.
enum Action {
    Respond(Response, Option<(i64, i64)>),
    Apply(Method),
    Open(String),
}

impl Pim {
    /// The invitation panel of the message shown in the reading pane, if it has a calendar part
    /// (asked of the account's worker the first time).
    pub fn invitation_panel(&mut self, ui: &mut Ui, cx: &Ctx<'_>, key: &MessageKey, parsed: &Parsed) {
        let Some(index) = parsed.attachments.iter().position(|a| is_calendar_part(&a.declared_type, &a.name)) else { return };
        let k = (key.clone(), index);
        if !self.invitations.contains_key(&k) {
            let asked = cx.engine.send(key.account, Command::Pim(PimCommand::Invitation { folder: key.folder.clone(), uid: key.uid, index }));
            self.invitations.insert(k.clone(), if asked { InvitationState::Loading } else { InvitationState::Failed("The account is not running.".to_owned()) });
        }
        let me = cx.account(key.account).map(|a| a.address.clone()).unwrap_or_default();
        let provider = cx.provider(key.account);
        let all = sources(&self.data, cx.ws);
        let mut actions: Vec<Action> = Vec::new();
        ui.add_space(10.0);
        Frame::new().fill(t::RAISED).stroke(Stroke::new(1.0, t::ACCENT)).corner_radius(8).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let Some(state) = self.invitations.get_mut(&k) else { return };
            let r = match state {
                InvitationState::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Reading the invitation…").color(t::MUTED));
                    });
                    return;
                }
                InvitationState::Failed(e) => {
                    ui.label(RichText::new(format!("This message carries a calendar MailALM could not read: {e}")).color(t::WARN));
                    return;
                }
                InvitationState::Ready(r) => r,
            };
            let Some(ev) = main_event(&r.invitation).cloned() else { return };
            let method = r.invitation.method.unwrap_or(Method::Request);
            let existing = self.data.get(&key.account).and_then(|d| d.events.iter().find(|e| e.uid == ev.uid && (ev.recurrence_id.is_none() || e.recurrence_id == ev.recurrence_id))).cloned();
            let i_organize = ev.organizer.as_ref().is_some_and(|o| o.is(&me));
            let heading = match method {
                Method::Request if existing.is_some() => "Meeting update",
                Method::Request => "Meeting invitation",
                Method::Cancel => "Meeting cancelled",
                Method::Reply => "Answer to your meeting",
                Method::Counter => "New time proposed",
                _ => "Calendar",
            };
            ui.label(RichText::new(heading.to_uppercase()).size(11.0).strong().color(t::ACCENT_TEXT));
            ui.label(RichText::new(if ev.title.is_empty() { "(no title)" } else { ev.title.as_str() }).size(16.0).strong());
            ui.label(time::span(ev.start, ev.end, ev.all_day));
            if let Some(rule) = &ev.recurrence {
                ui.label(RichText::new(repeat_text(rule)).size(12.0).color(t::MUTED));
            }
            if !ev.location.is_empty() {
                ui.label(format!("Where: {}", ev.location));
            }
            if let Some(url) = ev.online.as_ref().map(|o| o.url.clone()).filter(|u| !u.is_empty())
                && let Some(domain) = link_domain(&url)
            {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("Online meeting at {domain}"));
                    if ui.button("Open in the browser").on_hover_text(url.as_str()).clicked() {
                        actions.push(Action::Open(url.clone()));
                    }
                });
            }
            if let Some(o) = &ev.organizer {
                ui.label(RichText::new(format!("Organizer: {}", if o.name.is_empty() { o.email.clone() } else { format!("{} <{}>", o.name, o.email) })).size(12.5));
            }
            if !ev.attendees.is_empty() {
                let names: Vec<String> = ev.attendees.iter().take(6).map(|a| if a.name.is_empty() { a.email.clone() } else { a.name.clone() }).collect();
                let more = if ev.attendees.len() > 6 { format!(" and {} more", ev.attendees.len() - 6) } else { String::new() };
                ui.label(RichText::new(format!("Guests: {}{more}", names.join(", "))).size(12.0).color(t::MUTED));
            }
            ui.label(RichText::new(format!("Shows as {}", busy_label(ev.busy).to_lowercase())).size(12.0).color(t::FAINT));

            // What it changes, for an update of a meeting already in the calendar.
            if let (Method::Request, Some(old)) = (method, &existing) {
                let diff = changes(old, &ev);
                if !diff.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new("What changes:").color(t::WARN));
                    for d in diff {
                        ui.label(RichText::new(format!("• {d}")).size(12.0));
                    }
                }
            }

            // Clashes with every calendar of every account.
            if matches!(method, Method::Request | Method::Counter) {
                let (a, b) = if ev.all_day { (time::day_start(ev.start.div_euclid(86_400)), time::day_start(ev.end.div_euclid(86_400))) } else { (ev.start, ev.end) };
                let clashes = mailalm_engine::agenda::clashes(&all, a, b, &ev.uid, &TzRules);
                ui.add_space(4.0);
                if clashes.is_empty() {
                    ui.label(RichText::new("No clash in your calendars, all accounts.").size(12.5).color(t::OK));
                } else {
                    ui.label(RichText::new(format!("Clashes with {} event{}:", clashes.len(), if clashes.len() > 1 { "s" } else { "" })).color(t::WARN));
                    for c in clashes.iter().take(8) {
                        ui.label(RichText::new(format!("• {} — {} · {}", c.title, time::span(c.start, c.end, false), cx.name(c.account))).size(12.0));
                    }
                }
            }
            ui.add_space(8.0);
            if let Some(done) = &r.done {
                ui.label(RichText::new(done.as_str()).color(t::OK));
                return;
            }
            let mine = existing.as_ref().and_then(|e| e.my_response).or_else(|| ev.attendee(&me).map(|a| a.response));
            match method {
                Method::Request if i_organize => {
                    ui.label(RichText::new("You organize this meeting.").color(t::MUTED));
                }
                Method::Request => {
                    if let Some(m) = mine.filter(|m| *m != Response::NeedsAction) {
                        ui.label(RichText::new(format!("Your answer so far: {}", response_label(m))).size(12.5));
                    }
                    if existing.is_some() && matches!(provider, Some(Provider::Imap(_))) {
                        if ui.button("Apply the update to my calendar").clicked() {
                            actions.push(Action::Apply(Method::Request));
                        }
                    } else if existing.is_some() {
                        ui.label(RichText::new("The provider has updated the calendar already.").size(12.0).color(t::FAINT));
                    }
                    ui.horizontal(|ui| {
                        for (label, answer) in [("Accept", Response::Accepted), ("Tentative", Response::Tentative), ("Decline", Response::Declined)] {
                            if ui.add(Button::new(label)).clicked() {
                                actions.push(Action::Respond(answer, None));
                            }
                        }
                        if ui.button("Propose another time…").clicked() {
                            r.proposing = !r.proposing;
                        }
                    });
                    ui.checkbox(&mut r.send, "Send a reply to the organizer");
                    if r.send {
                        ui.add(TextEdit::singleline(&mut r.comment).desired_width(f32::INFINITY).hint_text("A note for the organizer (optional)"));
                    }
                    if r.proposing {
                        ui.horizontal(|ui| {
                            ui.label("Propose");
                            ui.add(TextEdit::singleline(&mut r.proposal_date).desired_width(100.0));
                            ui.label("from");
                            ui.add(TextEdit::singleline(&mut r.proposal_start).desired_width(56.0));
                            ui.label("to");
                            ui.add(TextEdit::singleline(&mut r.proposal_end).desired_width(56.0));
                            if ui.button("Send the proposal").clicked() {
                                match proposal(&r.proposal_date, &r.proposal_start, &r.proposal_end) {
                                    Some(p) => actions.push(Action::Respond(Response::Tentative, Some(p))),
                                    None => r.problem = Some("Write the day as 2026-11-25 and the times as 14:00, the end after the start.".to_owned()),
                                }
                            }
                        });
                        if let Some(p) = &r.problem {
                            ui.label(RichText::new(p.as_str()).color(t::DANGER));
                        }
                        ui.label(RichText::new("Your answer is tentative meanwhile; the proposal always goes to the organizer.").size(11.5).color(t::FAINT));
                    }
                }
                Method::Cancel => match &existing {
                    Some(_) if !r.confirm => {
                        if ui.button("Remove it from my calendar…").clicked() {
                            r.confirm = true;
                        }
                    }
                    Some(_) => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Remove this meeting from your calendar?").color(t::WARN));
                            if ui.button("Remove").clicked() {
                                actions.push(Action::Apply(Method::Cancel));
                            }
                            if ui.button("Keep it").clicked() {
                                r.confirm = false;
                            }
                        });
                    }
                    None => {
                        ui.label(RichText::new("It is not in your calendar.").color(t::FAINT));
                    }
                },
                Method::Reply => {
                    for a in ev.attendees.iter().take(10) {
                        ui.label(format!("{} {}", if a.name.is_empty() { &a.email } else { &a.name }, response_label(a.response)));
                    }
                    match (&existing, provider) {
                        (Some(e), Some(Provider::Imap(_))) if e.is_organizer => {
                            if ui.button("Record the answer in my calendar").clicked() {
                                actions.push(Action::Apply(Method::Reply));
                            }
                        }
                        (Some(_), Some(Provider::Outlook | Provider::Gmail)) => {
                            ui.label(RichText::new("The provider records the answer in the calendar by itself.").size(12.0).color(t::FAINT));
                        }
                        _ => {}
                    }
                }
                Method::Counter => {
                    ui.label(RichText::new("To take this time, change the meeting's time in the calendar: its guests are told.").size(12.0).color(t::FAINT));
                }
                _ => {}
            }
        });
        if actions.is_empty() {
            return;
        }
        let Some(InvitationState::Ready(r)) = self.invitations.get_mut(&k) else { return };
        let Some(ev) = main_event(&r.invitation).cloned() else { return };
        let mut opened: Option<Result<(), String>> = None;
        for a in actions {
            match a {
                Action::Respond(answer, proposal) => {
                    let command = PimCommand::Respond { event: Box::new(ev.clone()), answer, send: r.send || proposal.is_some(), comment: r.comment.clone(), proposal };
                    r.done = Some(if cx.send(key.account, command) {
                        match (proposal, r.send) {
                            (Some(_), _) => "Your proposal is being sent to the organizer.".to_owned(),
                            (None, true) => format!("Your answer ({}) is being sent to the organizer.", response_label(answer)),
                            (None, false) => format!("Answered {} without telling the organizer.", response_label(answer)),
                        }
                    } else {
                        "The account is not running: nothing was done.".to_owned()
                    });
                }
                Action::Apply(method) => {
                    let events = r.invitation.events.clone();
                    r.done = Some(if cx.send(key.account, PimCommand::ApplyInvitation { method, events }) { "Your calendar is being updated.".to_owned() } else { "The account is not running: nothing was done.".to_owned() });
                }
                Action::Open(url) => opened = Some(open_link(&url)),
            }
        }
        if let Some(Err(e)) = opened {
            self.warn(e);
        }
    }
}

/// A time proposed: a day and two times, here; `None` when one isn't one, or it ends first.
fn proposal(date: &str, start: &str, end: &str) -> Option<(i64, i64)> {
    let day = parse_date(date)?;
    let (a, b) = (time::parse_clock(start)?, time::parse_clock(end)?);
    let (s, e) = (time::utc_of_local(day * 86_400 + a), time::utc_of_local(day * 86_400 + b));
    (e > s).then_some((s, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailalm_model::pim::UtcOnly;

    const GOOGLE: &str = "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nDTSTART:20261125T140000Z\nDTEND:20261125T150000Z\nUID:abc@google.com\n\
ORGANIZER;CN=Ann:mailto:ann@gmail.example\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@videotron.ca\nSUMMARY:Standup\n\
LOCATION:Room 4\nEND:VEVENT\nEND:VCALENDAR\n";

    #[test]
    fn an_invitation_read_offers_its_own_time_to_change() {
        let inv = mailalm_model::pim::ical::parse(GOOGLE, &UtcOnly);
        let state = InvitationState::read(Ok(Box::new(inv)));
        let InvitationState::Ready(r) = state else { panic!("read") };
        assert!(r.send, "the reply is ticked by default");
        assert_eq!(main_event(&r.invitation).map(|e| e.title.as_str()), Some("Standup"));
        assert!(parse_date(&r.proposal_date).is_some());
        assert!(matches!(InvitationState::read(Err("no".into())), InvitationState::Failed(_)));
    }

    #[test]
    fn an_update_says_what_changes() {
        let old = CalendarEvent { title: "Standup".into(), start: 0, end: 3600, location: "Room 4".into(), ..CalendarEvent::default() };
        let new = CalendarEvent { start: 3600, end: 7200, location: "Room 5".into(), ..old.clone() };
        let c = changes(&old, &new);
        assert_eq!(c.len(), 2);
        assert!(c[1].contains("Room 4 → Room 5"));
        assert!(proposal("2026-11-25", "15:00", "14:00").is_none());
        assert!(proposal("2026-11-25", "14:00", "15:00").is_some());
    }
}
