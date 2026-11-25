//! The queue of changes, their sending and the syncs, with a provider played by the test.

use std::cell::RefCell;
use std::sync::Arc;

use mailalm_model::pim::{Attendee, UtcOnly};

use super::*;

/// A provider that answers from memory, and remembers what it was asked.
struct Fake {
    store: Store,
    online: bool,
    told: RefCell<Vec<Event>>,
    calls: Vec<String>,
    memo: Memo,
    made: u32,
    delta: Option<CalendarSync>,
}

impl Fake {
    fn new(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("mailalm-pim-sync-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir, AccountId(1)).expect("store");
        let cal = Calendar { id: "primary".into(), remote: Remote::new("primary", ""), name: "Calendar".into(), can_edit: true, primary: true, ..Calendar::default() };
        store.save_calendars(&[cal]).expect("calendars");
        store.save_task_lists(&[TaskList { id: "L".into(), remote: Remote::new("L", ""), name: "Tasks".into(), default: true }]).expect("lists");
        Fake { store, online: false, told: RefCell::new(Vec::new()), calls: Vec::new(), memo: Memo::default(), made: 0, delta: None }
    }

    fn errors(&self) -> Vec<String> {
        self.told.borrow().iter().filter_map(|e| if let Event::Error { text, .. } = e { Some(text.clone()) } else { None }).collect()
    }

    fn offline<T>(&self) -> Result<T, NetError> {
        Err(NetError::Io("offline".into()))
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.store.root().parent().unwrap_or(self.store.root()));
    }
}

impl PimWorker for Fake {
    fn who(&self) -> AccountId {
        AccountId(1)
    }
    fn cache(&self) -> &Store {
        &self.store
    }
    fn tell(&self, e: Event) {
        self.told.borrow_mut().push(e);
    }
    fn zone_rules(&self) -> Zones {
        Arc::new(UtcOnly)
    }
    fn my_address(&self) -> String {
        "me@gmail.example".into()
    }
    fn name(&self) -> &'static str {
        "Fake"
    }
    fn reach(&mut self) -> bool {
        self.online
    }
    fn is_online(&self) -> bool {
        self.online
    }
    fn memo(&mut self) -> &mut Memo {
        &mut self.memo
    }
    fn explain(&self, e: &NetError) -> String {
        e.to_string()
    }
    fn calendars(&mut self) -> Result<Vec<Calendar>, NetError> {
        if !self.online {
            return self.offline();
        }
        Ok(self.store.calendars())
    }
    fn sync_calendar(&mut self, _calendar: &Calendar, token: Option<&str>, _window: (i64, i64)) -> Result<CalendarSync, NetError> {
        self.calls.push(format!("sync {}", token.unwrap_or("-")));
        self.delta.take().ok_or_else(|| NetError::Server("nothing".into()))
    }
    fn events_in(&mut self, _calendar: &Calendar, from: i64, to: i64) -> Result<Vec<CalendarEvent>, NetError> {
        self.calls.push(format!("range {from} {to}"));
        Ok(Vec::new())
    }
    fn create_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError> {
        if !self.online {
            return self.offline();
        }
        self.made += 1;
        self.calls.push(format!("create {}", event.title));
        if event.title == "refused" {
            return Err(NetError::Invalid("no Teams here".into()));
        }
        Ok(CalendarEvent { uid: format!("srv-{}", self.made), remote: Remote::new(&format!("R{}", self.made), "v1"), ..event.clone() })
    }
    fn update_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError> {
        self.calls.push(format!("update {} {}", event.remote.id, event.title));
        if event.title == "refused" {
            return Err(NetError::Server("no".into()));
        }
        Ok(event.clone())
    }
    fn delete_event(&mut self, event: &CalendarEvent, notify: bool) -> Result<(), NetError> {
        self.calls.push(format!("delete {} {notify}", event.remote.id));
        Ok(())
    }
    fn respond(&mut self, event: &CalendarEvent, answer: Response, send: bool, _comment: &str, _proposal: Option<(i64, i64)>) -> Result<(), NetError> {
        self.calls.push(format!("respond {} {} {send}", event.remote.id, answer.partstat()));
        Ok(())
    }
    fn send_mail(&mut self, message: &Outgoing) -> Result<(), NetError> {
        self.calls.push(format!("mail {} {}", message.to.first().map(|p| p.address.as_str()).unwrap_or(""), message.calendar.as_ref().map(|c| c.method.as_str()).unwrap_or("")));
        Ok(())
    }
    fn task_lists(&mut self) -> Result<Vec<TaskList>, NetError> {
        Ok(self.store.task_lists())
    }
    fn tasks(&mut self, _list: &str) -> Result<Vec<Task>, NetError> {
        Ok(Vec::new())
    }
    fn create_task(&mut self, task: &Task) -> Result<Task, NetError> {
        self.made += 1;
        self.calls.push(format!("create task {}", task.title));
        Ok(Task { remote: Remote::new(&format!("T{}", self.made), ""), ..task.clone() })
    }
    fn update_task(&mut self, task: &Task) -> Result<Task, NetError> {
        self.calls.push(format!("update task {} {}", task.remote.id, task.completed));
        Ok(task.clone())
    }
    fn delete_task(&mut self, task: &Task) -> Result<(), NetError> {
        self.calls.push(format!("delete task {}", task.remote.id));
        Ok(())
    }
    fn contacts(&mut self) -> Result<Vec<Contact>, NetError> {
        Ok(vec![Contact { name: "Zoe".into(), emails: vec!["zoe@studio.example".into()], ..Contact::default() }])
    }
}

fn new_event(title: &str) -> Box<CalendarEvent> {
    Box::new(CalendarEvent { title: title.into(), start: 1_000_000, end: 1_003_600, ..CalendarEvent::default() })
}

#[test]
fn an_event_made_offline_waits_then_goes_with_its_later_changes() {
    let mut w = Fake::new("offline-then-online");
    handle(&mut w, PimCommand::SaveEvent { event: new_event("Mix"), notify: true });
    // In the cache at once, in the primary calendar, with a UID at the account's domain.
    let cached = w.store.events("primary");
    assert_eq!(cached.len(), 1);
    assert!(cached[0].uid.ends_with("@gmail.example") && cached[0].remote.id.is_empty());
    assert_eq!(w.store.pim_pending().len(), 1, "kept for later");

    // Changed again before the provider had it: the same event, still waiting.
    let mut changed = cached[0].clone();
    changed.title = "Mix, take 2".into();
    handle(&mut w, PimCommand::SaveEvent { event: Box::new(changed), notify: true });
    assert_eq!(w.store.events("primary").len(), 1);
    assert_eq!(w.store.pim_pending().len(), 2);

    // Online: made, then changed under the provider's id.
    w.online = true;
    flush(&mut w);
    assert_eq!(w.calls, ["create Mix", "update R1 Mix, take 2"]);
    assert!(w.store.pim_pending().is_empty());
    let cached = w.store.events("primary");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].remote.id, "R1");
    assert_eq!(cached[0].title, "Mix, take 2");
}

#[test]
fn deleting_an_event_never_made_forgets_it() {
    let mut w = Fake::new("delete-unmade");
    handle(&mut w, PimCommand::SaveEvent { event: new_event("Mix"), notify: false });
    let e = w.store.events("primary").remove(0);
    handle(&mut w, PimCommand::DeleteEvent { event: Box::new(e), notify: true });
    assert!(w.store.events("primary").is_empty());
    assert!(w.store.pim_pending().is_empty(), "nothing to tell the provider");
}

#[test]
fn a_refused_change_is_said_and_dropped() {
    let mut w = Fake::new("refused");
    let e = CalendarEvent { uid: "u".into(), calendar: "primary".into(), remote: Remote::new("R9", ""), title: "refused".into(), start: 1, end: 2, ..CalendarEvent::default() };
    w.store.save_events("primary", std::slice::from_ref(&e)).expect("events");
    w.online = true;
    w.delta = Some(CalendarSync::Delta { changed: Vec::new(), removed: Vec::new(), token: "t1".into() });
    handle(&mut w, PimCommand::SaveEvent { event: Box::new(e), notify: false });
    assert!(w.store.pim_pending().is_empty());
    assert!(w.errors().iter().any(|t| t.contains("Fake refused a change to the calendar")), "{:?}", w.errors());
    // The cache was synced again after the refusal.
    assert!(w.calls.iter().any(|c| c.starts_with("sync")), "{:?}", w.calls);
}

#[test]
fn an_event_the_provider_refuses_to_make_leaves_the_cache() {
    let mut w = Fake::new("refused-creation");
    w.online = true;
    handle(&mut w, PimCommand::SaveEvent { event: new_event("refused"), notify: false });
    assert!(w.store.events("primary").is_empty(), "no phantom event");
    assert!(w.store.pim_pending().is_empty());
    assert!(w.errors().iter().any(|t| t.contains("no Teams here")), "{:?}", w.errors());
}

#[test]
fn an_answer_to_an_invitation_the_calendar_lacks_goes_by_mail() {
    let mut w = Fake::new("respond-by-mail");
    w.online = true;
    let invitation = CalendarEvent {
        uid: "abc@google.com".into(),
        title: "Standup".into(),
        start: 10,
        end: 20,
        organizer: Some(Attendee::new("ann@gmail.example", "Ann")),
        attendees: vec![Attendee::new("me@gmail.example", "")],
        ..CalendarEvent::default()
    };
    handle(&mut w, PimCommand::Respond { event: Box::new(invitation.clone()), answer: Response::Accepted, send: true, comment: String::new(), proposal: None });
    assert_eq!(w.calls, ["mail ann@gmail.example REPLY"]);

    // In the calendar: answered there, by its id.
    let cached = CalendarEvent { calendar: "primary".into(), remote: Remote::new("G7", ""), my_response: Some(Response::NeedsAction), ..invitation.clone() };
    w.store.save_events("primary", &[cached]).expect("events");
    w.calls.clear();
    handle(&mut w, PimCommand::Respond { event: Box::new(invitation), answer: Response::Tentative, send: false, comment: String::new(), proposal: None });
    assert_eq!(w.calls, ["respond G7 TENTATIVE false"]);
    assert_eq!(w.store.events("primary")[0].my_response, Some(Response::Tentative));
}

#[test]
fn a_cancellation_removes_the_event_without_telling_the_guests() {
    let mut w = Fake::new("cancel");
    let e = CalendarEvent { uid: "abc@google.com".into(), calendar: "primary".into(), remote: Remote::new("G7", ""), start: 1, end: 2, ..CalendarEvent::default() };
    w.store.save_events("primary", std::slice::from_ref(&e)).expect("events");
    w.online = true;
    let cancelled = CalendarEvent { remote: Remote::default(), calendar: String::new(), ..e };
    handle(&mut w, PimCommand::ApplyInvitation { method: Method::Cancel, events: vec![cancelled] });
    assert!(w.store.events("primary").is_empty());
    assert_eq!(w.calls, ["delete G7 false"]);
}

#[test]
fn a_task_made_offline_is_made_then_completed_under_its_new_id() {
    let mut w = Fake::new("tasks");
    handle(&mut w, PimCommand::SaveTask(Box::new(Task { title: "Mix the trailer".into(), ..Task::default() })));
    let t = w.store.tasks("L").remove(0);
    assert!(is_local_key(&t.remote.id));
    handle(&mut w, PimCommand::SaveTask(Box::new(Task { completed: true, ..t })));
    assert_eq!(w.store.tasks("L").len(), 1);
    w.online = true;
    flush(&mut w);
    assert_eq!(w.calls, ["create task Mix the trailer", "update task T1 true"]);
    assert_eq!(w.store.tasks("L")[0].remote.id, "T1");
    assert!(w.store.tasks("L")[0].completed);
}

#[test]
fn a_delta_updates_and_removes_and_keeps_its_token() {
    let mut w = Fake::new("delta");
    let now = now_unix();
    let old = CalendarEvent { uid: "a".into(), calendar: "primary".into(), remote: Remote::new("A", ""), start: now, end: now + 60, ..CalendarEvent::default() };
    let gone = CalendarEvent { uid: "b".into(), remote: Remote::new("B", ""), ..old.clone() };
    w.store.save_events("primary", &[old.clone(), gone]).expect("events");
    let mut state = w.store.pim_state();
    state.tokens.insert("primary".into(), "t0".into());
    w.store.save_pim_state(&state).expect("state");
    let renamed = CalendarEvent { title: "renamed".into(), ..old };
    w.delta = Some(CalendarSync::Delta { changed: vec![renamed], removed: vec!["B".into()], token: "t1".into() });
    w.online = true;
    handle(&mut w, PimCommand::Sync);
    assert!(w.calls.contains(&"sync t0".to_owned()), "{:?}", w.calls);
    let events = w.store.events("primary");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "renamed");
    assert_eq!(w.store.pim_state().tokens.get("primary").map(String::as_str), Some("t1"));
    assert_eq!(w.store.contacts().len(), 1, "the contacts came too");

    // A calendar synced by token has every event: no range is read for it.
    w.calls.clear();
    handle(&mut w, PimCommand::Range { from: now - 400 * 86_400, to: now - 300 * 86_400 });
    assert!(w.calls.is_empty());
}

#[test]
fn a_window_replaces_what_it_covers_and_ranges_are_read_once() {
    let mut w = Fake::new("window");
    let now = now_unix();
    let inside = CalendarEvent { uid: "a".into(), calendar: "primary".into(), remote: Remote::new("A", ""), start: now, end: now + 60, ..CalendarEvent::default() };
    let far = CalendarEvent { uid: "f".into(), remote: Remote::new("F", ""), start: now + 900 * 86_400, end: now + 900 * 86_400 + 60, ..inside.clone() };
    w.store.save_events("primary", &[inside, far]).expect("events");
    let fresh = CalendarEvent { uid: "n".into(), calendar: "primary".into(), remote: Remote::new("N", ""), start: now + 3600, end: now + 7200, ..CalendarEvent::default() };
    w.delta = Some(CalendarSync::Window(vec![fresh]));
    w.online = true;
    handle(&mut w, PimCommand::Sync);
    let ids: Vec<String> = w.store.events("primary").into_iter().map(|e| e.remote.id).collect();
    assert_eq!(ids, ["F", "N"], "the window's events replaced, the far one kept");

    w.calls.clear();
    let (from, to) = (now + 600 * 86_400, now + 630 * 86_400);
    handle(&mut w, PimCommand::Range { from, to });
    handle(&mut w, PimCommand::Range { from, to });
    assert_eq!(w.calls.len(), 1, "read once: {:?}", w.calls);
    // Inside the window: nothing to read.
    handle(&mut w, PimCommand::Range { from: now, to: now + 86_400 });
    assert_eq!(w.calls.len(), 1);
}

#[test]
fn uids_and_keys_are_new_each_time() {
    assert_ne!(new_uid("a@b.example"), new_uid("a@b.example"));
    assert!(new_uid("a@b.example").ends_with("@b.example"));
    assert!(new_uid("nobody").ends_with("@mailalm.invalid"));
    assert!(new_uid("x@evil\r\nBcc: y").ends_with("@mailalm.invalid"));
    assert!(is_local_key(&local_key()) && !is_local_key("AAMkAD"));
    let (a, b) = window(1_795_600_000);
    assert_eq!(a % 86_400, 0);
    assert_eq!(b - a, PAST + FUTURE);
}
