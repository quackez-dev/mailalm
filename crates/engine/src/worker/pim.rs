//! An IMAP account's calendar and task list, kept on this computer only (PLAN.md, section 4.1),
//! and its meetings: invitations, updates and cancellations sent to the guests, and answers sent
//! to organizers, by mail through the account's SMTP server (iMIP, RFC 6047).
//!
//! An invitation received enters the calendar only when the user answers it (accepted or
//! tentative); an update or a cancellation is taken in only on the user's confirmation
//! (`ApplyInvitation`). The cache changes at once; a meeting message that can't be sent (offline)
//! is said, and the event stays as saved.

use mailalm_mime::Outgoing;
use mailalm_model::pim::ical::Method;
use mailalm_model::pim::{BusyStatus, Calendar, Event as CalendarEvent, EventStatus, LOCAL, Remote, Response, Task, TaskList};
use mailalm_model::{FolderRole, Person};
use mailalm_net::oauth::now_unix;
use mailalm_store::Store;

use super::{Worker, describe};
use crate::pim_sync::{calendars_event, local_key, new_uid, same_event, tasks_event, upsert_event, upsert_task};
use crate::{Event, PimCommand, itip, local, secrets};

/// The account's own calendar and task list, made the first time.
fn ensure(store: &Store) {
    if !store.calendars().iter().any(|c| c.id == LOCAL) {
        let calendar = Calendar { id: LOCAL.into(), name: "Calendar".into(), can_edit: true, primary: true, ..Calendar::default() };
        let _ = store.save_calendars(&[calendar]);
    }
    if !store.task_lists().iter().any(|l| l.id == LOCAL) {
        let _ = store.save_task_lists(&[TaskList { id: LOCAL.into(), remote: Remote::default(), name: "Tasks".into(), default: true }]);
    }
}

/// Whether two versions of a meeting differ in what its guests must hear of again.
fn changed_for_guests(a: &CalendarEvent, b: &CalendarEvent) -> bool {
    a.start != b.start || a.end != b.end || a.all_day != b.all_day || a.location != b.location || a.recurrence != b.recurrence || a.title != b.title
}

/// Saves an event in the account's calendar: a new one gets a UID; a meeting gets the user as its
/// organizer, and a higher sequence when its guests must hear of a change. Returns it as saved.
fn save_event(store: &Store, mut e: CalendarEvent, me: &str) -> CalendarEvent {
    e.calendar = LOCAL.into();
    e.remote = Remote::default();
    if e.uid.is_empty() {
        e.uid = new_uid(me);
    }
    let mut events = store.events(LOCAL);
    if e.is_meeting() && e.organizer.is_none() {
        e.organizer = Some(mailalm_model::pim::Attendee::new(me, ""));
        e.is_organizer = true;
    }
    if let Some(old) = events.iter().find(|x| same_event(x, &e))
        && e.is_meeting()
        && e.is_organizer
        && changed_for_guests(old, &e)
    {
        e.sequence = old.sequence.max(e.sequence).saturating_add(1);
    }
    upsert_event(&mut events, e.clone());
    let _ = store.save_events(LOCAL, &events);
    e
}

/// Removes an event (all of a series' occurrences kept apart too, when it is the series).
fn delete_event(store: &Store, e: &CalendarEvent) -> Option<CalendarEvent> {
    let mut events = store.events(LOCAL);
    let found = events.iter().find(|x| same_event(x, e)).cloned();
    events.retain(|x| !same_event(x, e) && !(e.recurrence_id.is_none() && x.uid == e.uid));
    let _ = store.save_events(LOCAL, &events);
    found
}

/// The user's answer to an invitation: accepted or tentative, it is in the calendar with that
/// answer; declined, it is not.
fn respond(store: &Store, invitation: &CalendarEvent, answer: Response, me: &str) {
    let mut events = store.events(LOCAL);
    let existing = events.iter().find(|x| x.uid == invitation.uid && x.recurrence_id == invitation.recurrence_id).cloned();
    if answer == Response::Declined {
        events.retain(|x| !(x.uid == invitation.uid && (invitation.recurrence_id.is_none() || x.recurrence_id == invitation.recurrence_id)));
    } else {
        let mut e = existing.unwrap_or_else(|| invitation.clone());
        e.calendar = LOCAL.into();
        e.remote = Remote::default();
        e.is_organizer = false;
        e.my_response = Some(answer);
        if let Some(a) = e.attendees.iter_mut().find(|a| a.is(me)) {
            a.response = answer;
        }
        if answer == Response::Tentative && e.busy == BusyStatus::Busy {
            e.busy = BusyStatus::Tentative;
        } else if answer == Response::Accepted && e.busy == BusyStatus::Tentative {
            e.busy = BusyStatus::Busy;
        }
        if e.status == EventStatus::Tentative {
            e.status = EventStatus::Confirmed;
        }
        upsert_event(&mut events, e);
    }
    let _ = store.save_events(LOCAL, &events);
}

/// An update, a cancellation or a guest's answer received by mail, taken in after the user's
/// confirmation. Returns how many events it changed.
fn apply_invitation(store: &Store, method: Method, received: &[CalendarEvent]) -> usize {
    let mut events = store.events(LOCAL);
    let mut changed = 0;
    for r in received {
        let matches = |x: &CalendarEvent| x.uid == r.uid && (r.recurrence_id.is_none() || x.recurrence_id == r.recurrence_id);
        match method {
            Method::Cancel => {
                let before = events.len();
                events.retain(|x| !matches(x));
                changed += before - events.len();
            }
            Method::Reply => {
                // The guest's answer, in the meeting the user organizes.
                for x in events.iter_mut().filter(|x| matches(&**x) && x.is_organizer) {
                    for a in &r.attendees {
                        if let Some(mine) = x.attendees.iter_mut().find(|m| m.is(&a.email)) {
                            mine.response = a.response;
                            changed += 1;
                        }
                    }
                }
            }
            _ => {
                // An update of a meeting already in the calendar: its new details, the user's
                // answer kept.
                if let Some(x) = events.iter_mut().find(|x| x.uid == r.uid && x.recurrence_id == r.recurrence_id) {
                    let answer = x.my_response;
                    let reminders = x.reminders.clone();
                    *x = CalendarEvent { calendar: LOCAL.into(), remote: Remote::default(), my_response: answer, is_organizer: false, reminders, ..r.clone() };
                    changed += 1;
                }
            }
        }
    }
    let _ = store.save_events(LOCAL, &events);
    changed
}

/// Saves a task in the account's list (a new one gets a local key).
fn save_task(store: &Store, mut t: Task) {
    t.list = LOCAL.into();
    if t.remote.id.is_empty() {
        t.remote = Remote::new(&local_key(), "");
    }
    let mut tasks = store.tasks(LOCAL);
    upsert_task(&mut tasks, t);
    let _ = store.save_tasks(LOCAL, &tasks);
}

fn delete_task(store: &Store, t: &Task) {
    let mut tasks = store.tasks(LOCAL);
    tasks.retain(|x| x.remote.id != t.remote.id);
    let _ = store.save_tasks(LOCAL, &tasks);
}

impl Worker {
    fn me(&self) -> Person {
        Person { name: String::new(), address: self.account.address.clone() }
    }

    /// The calendar and the tasks, as the cache has them, before any network.
    pub(super) fn pim_show_cache(&self) {
        ensure(&self.store);
        self.emit(calendars_event(&self.store, self.id));
        self.emit(tasks_event(&self.store, self.id));
    }

    /// A calendar or task command: all of it here, meeting messages sent at once.
    pub(super) fn pim(&mut self, command: PimCommand) {
        ensure(&self.store);
        let me = self.me();
        let now = now_unix();
        match command {
            // Nothing to read: this computer has it all.
            PimCommand::Sync | PimCommand::Range { .. } => return,
            PimCommand::Invitation { folder, uid, index } => {
                self.emit(local::invitation(&self.store, self.id, &folder, uid, index, &*self.zones));
                return;
            }
            PimCommand::SaveEvent { event, notify } => {
                let saved = save_event(&self.store, *event, &me.address);
                self.emit(calendars_event(&self.store, self.id));
                if notify && saved.is_organizer && saved.is_meeting() {
                    let message = itip::request(&saved, &me, &*self.zones, now);
                    self.send_meeting(message, "The invitation");
                }
            }
            PimCommand::DeleteEvent { event, notify } => {
                let removed = delete_event(&self.store, &event);
                self.emit(calendars_event(&self.store, self.id));
                if let Some(e) = removed.filter(|e| notify && e.is_organizer && e.is_meeting()) {
                    let message = itip::cancel(&e, &me, &*self.zones, now);
                    self.send_meeting(message, "The cancellation");
                }
            }
            PimCommand::Respond { event, answer, send, comment, proposal } => {
                respond(&self.store, &event, answer, &me.address);
                self.emit(calendars_event(&self.store, self.id));
                if send {
                    let message = itip::reply(&event, &me, answer, proposal, &comment, &*self.zones, now);
                    self.send_meeting(message, "The answer");
                }
            }
            PimCommand::ApplyInvitation { method, events } => {
                let n = apply_invitation(&self.store, method, &events);
                self.emit(calendars_event(&self.store, self.id));
                let text = if n == 0 { "Nothing in the calendar needed this change.".to_owned() } else { "The calendar was updated.".to_owned() };
                self.emit(Event::Notice { account: self.id, text });
            }
            PimCommand::SaveTask(task) => {
                save_task(&self.store, *task);
                self.emit(tasks_event(&self.store, self.id));
            }
            PimCommand::DeleteTask(task) => {
                delete_task(&self.store, &task);
                self.emit(tasks_event(&self.store, self.id));
            }
        }
    }

    /// Sends a meeting message now and says how it went; `what` names it ("The invitation").
    fn send_meeting(&mut self, message: Option<Outgoing>, what: &str) {
        let Some(message) = message else {
            self.error(format!("{what} could not be written: the meeting needs an organizer and guests with valid addresses."));
            return;
        };
        self.activity(Some(format!("Sending \"{}\"…", message.subject)));
        match self.send_now(&message) {
            Ok(()) => {
                let n = message.to.len();
                self.emit(Event::Notice { account: self.id, text: format!("{what} was sent to {n} {}.", if n == 1 { "person" } else { "people" }) });
            }
            Err(e) => self.error(format!("{what} was not sent (the calendar keeps the change): {e}")),
        }
        self.activity(None);
    }

    /// Sends a message by SMTP, then keeps a copy in Sent (as for any message sent).
    fn send_now(&mut self, message: &Outgoing) -> Result<(), String> {
        let server = self.account.smtp.clone().ok_or("The account has no outgoing (SMTP) server.")?;
        let (raw, _) = mailalm_mime::build(message)?;
        let password = secrets::password(self.id)?.ok_or("No password is kept for this account.")?;
        mailalm_net::smtp::send(&server, self.account.login(), &password, &message.from.address, &message.recipients(), &raw).map_err(|e| describe(&e))?;
        if let Some(sent) = self.folder_with_role(FolderRole::Sent) {
            let wire = sent.wire.clone();
            if let Some(s) = self.connect()
                && let Err(e) = s.append(&wire, &["\\Seen"], &raw)
            {
                log::debug!("no copy of a meeting message kept in Sent: {e}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailalm_model::AccountId;
    use mailalm_model::pim::Attendee;

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("mailalm-imap-pim-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Store::open(&dir, AccountId(1)).expect("store");
        ensure(&s);
        (s, dir)
    }

    fn invitation() -> CalendarEvent {
        CalendarEvent {
            uid: "abc@google.com".into(),
            title: "Standup".into(),
            start: 1_795_600_000,
            end: 1_795_601_800,
            organizer: Some(Attendee::new("ann@gmail.example", "Ann")),
            attendees: vec![Attendee::new("ann@gmail.example", "Ann"), Attendee::new("me@videotron.ca", "")],
            my_response: Some(Response::NeedsAction),
            reminders: vec![10],
            ..CalendarEvent::default()
        }
    }

    #[test]
    fn the_local_calendar_and_list_exist_once() {
        let (s, dir) = store("ensure");
        ensure(&s);
        assert_eq!(s.calendars().len(), 1);
        assert!(s.calendars()[0].can_edit && s.calendars()[0].primary);
        assert_eq!(s.task_lists().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_invitation_enters_the_calendar_only_when_accepted() {
        let (s, dir) = store("respond");
        respond(&s, &invitation(), Response::Declined, "me@videotron.ca");
        assert!(s.events(LOCAL).is_empty(), "declined: never added");
        respond(&s, &invitation(), Response::Tentative, "me@videotron.ca");
        let e = s.events(LOCAL);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].my_response, Some(Response::Tentative));
        assert_eq!(e[0].busy, BusyStatus::Tentative);
        assert_eq!(e[0].attendee("me@videotron.ca").map(|a| a.response), Some(Response::Tentative));
        respond(&s, &invitation(), Response::Accepted, "me@videotron.ca");
        assert_eq!(s.events(LOCAL).len(), 1, "the same event, answered again");
        respond(&s, &invitation(), Response::Declined, "me@videotron.ca");
        assert!(s.events(LOCAL).is_empty(), "declined later: taken out");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn updates_cancellations_and_answers_are_taken_in() {
        let (s, dir) = store("apply");
        respond(&s, &invitation(), Response::Accepted, "me@videotron.ca");
        let moved = CalendarEvent { start: 1_795_603_600, end: 1_795_605_400, sequence: 1, reminders: Vec::new(), ..invitation() };
        assert_eq!(apply_invitation(&s, Method::Request, std::slice::from_ref(&moved)), 1);
        let e = s.events(LOCAL);
        assert_eq!(e[0].start, 1_795_603_600);
        assert_eq!(e[0].my_response, Some(Response::Accepted), "the answer stays");
        assert_eq!(e[0].reminders, vec![10], "the user's reminders stay");
        assert_eq!(apply_invitation(&s, Method::Cancel, &[invitation()]), 1);
        assert!(s.events(LOCAL).is_empty());

        // A meeting the user organizes: a guest's answer is recorded.
        let mine = save_event(&s, CalendarEvent { title: "Mix".into(), start: 10, end: 20, attendees: vec![Attendee::new("zoe@studio.example", "Zoe")], ..CalendarEvent::default() }, "me@videotron.ca");
        assert!(mine.is_organizer && mine.organizer.as_ref().is_some_and(|o| o.is("me@videotron.ca")));
        let mut answer = mine.clone();
        answer.attendees = vec![Attendee { response: Response::Accepted, ..Attendee::new("zoe@studio.example", "") }];
        assert_eq!(apply_invitation(&s, Method::Reply, &[answer]), 1);
        assert_eq!(s.events(LOCAL)[0].attendee("zoe@studio.example").map(|a| a.response), Some(Response::Accepted));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_meeting_changed_for_its_guests_gets_a_new_sequence() {
        let (s, dir) = store("sequence");
        let first = save_event(&s, CalendarEvent { title: "Mix".into(), start: 10, end: 20, attendees: vec![Attendee::new("zoe@studio.example", "")], ..CalendarEvent::default() }, "me@videotron.ca");
        assert_eq!(first.sequence, 0);
        assert!(first.uid.ends_with("@videotron.ca"));
        let moved = save_event(&s, CalendarEvent { start: 30, end: 40, ..first.clone() }, "me@videotron.ca");
        assert_eq!(moved.sequence, 1);
        let same = save_event(&s, CalendarEvent { description: "notes".into(), ..moved.clone() }, "me@videotron.ca");
        assert_eq!(same.sequence, 1, "notes alone don't ask the guests again");
        assert_eq!(s.events(LOCAL).len(), 1);
        assert!(delete_event(&s, &same).is_some());
        assert!(s.events(LOCAL).is_empty());

        save_task(&s, Task { title: "Mix the trailer".into(), ..Task::default() });
        let t = s.tasks(LOCAL).remove(0);
        assert!(!t.remote.id.is_empty());
        save_task(&s, Task { completed: true, ..t.clone() });
        assert!(s.tasks(LOCAL)[0].completed);
        delete_task(&s, &t);
        assert!(s.tasks(LOCAL).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
