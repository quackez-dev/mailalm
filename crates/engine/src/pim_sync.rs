//! The calendars, tasks and contacts of an account whose provider keeps them (Outlook through
//! Microsoft Graph, Gmail through Google's APIs): what the Outlook and Gmail workers share
//! (PLAN.md, sections 7 and 14: A1, A2, K1, C7).
//!
//! The cache is shown first. A sync reads the calendars and their events (Google: what changed
//! since each calendar's sync token, series with their rules; Graph: the events of a window, from
//! 90 days back to 400 days on, every occurrence on its own), the task lists and their tasks, and
//! the contacts. The calendar page may ask for another range (`Range`): it is read then, for the
//! calendars that keep a window.
//!
//! A change the user makes goes into the cache at once and into a queue (`pim/pending.json`),
//! sent in order when the provider can be reached. An event not yet made on the server is known by
//! its UID, a task by a local key; once made, the later changes in the queue name it by the
//! provider's id. A change the provider refuses is said, dropped, and the cache synced again.

use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mailalm_mime::Outgoing;
use mailalm_model::pim::ical::Method;
use mailalm_model::pim::{Calendar, Contact, Event as CalendarEvent, Remote, Response, Task, TaskList};
use mailalm_model::{AccountId, Person};
use mailalm_net::NetError;
use mailalm_net::oauth::now_unix;
use mailalm_store::{PimChange, Store};

use crate::{Event, PimCommand, Zones, itip, local};

/// How often the calendars, tasks and contacts are synced.
pub(crate) const SYNC_EVERY: Duration = Duration::from_secs(15 * 60);
/// The window of events read from a provider that gives windows (Graph): from 90 days back to
/// 400 days on.
const PAST: i64 = 90 * 86_400;
const FUTURE: i64 = 400 * 86_400;
/// The longest range the calendar page may ask for at once.
const MAX_RANGE: i64 = 5 * 365 * 86_400;
/// The most events kept for one calendar, and the most other ranges remembered.
const MAX_KEPT_EVENTS: usize = 50_000;
const MAX_RANGES: usize = 24;
/// What a task not yet made on the server has for an id.
const LOCAL_KEY: &str = "mailalm-local-";

/// 32 hexadecimal digits that differ from one call to the next (time, a counter, and the
/// system's randomness through `RandomState`'s seed).
pub(crate) fn random_hex() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut a = RandomState::new().build_hasher();
    a.write_u128(nanos);
    a.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    let first = a.finish();
    let mut b = RandomState::new().build_hasher();
    b.write_u64(first);
    format!("{first:016x}{:016x}", b.finish())
}

/// A key for a task not yet made on the server.
pub(crate) fn local_key() -> String {
    format!("{LOCAL_KEY}{}", random_hex())
}

pub(crate) fn is_local_key(id: &str) -> bool {
    id.starts_with(LOCAL_KEY)
}

/// A new event's iCalendar UID, at the account's domain.
pub(crate) fn new_uid(address: &str) -> String {
    let domain = address.rsplit_once('@').map(|(_, d)| d.trim()).filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')).unwrap_or("mailalm.invalid");
    format!("{}@{domain}", random_hex())
}

/// The window of events read now: from 90 days back to 400 days on, from midnight UTC.
pub(crate) fn window(now: i64) -> (i64, i64) {
    let day = now.div_euclid(86_400).saturating_mul(86_400);
    (day.saturating_sub(PAST), day.saturating_add(FUTURE))
}

fn overlaps(e: &CalendarEvent, (from, to): (i64, i64)) -> bool {
    e.start < to && e.end.max(e.start.saturating_add(1)) > from
}

/// Whether two events are the same one: by the provider's id, else (not yet on the server) by UID
/// and occurrence.
pub(crate) fn same_event(a: &CalendarEvent, b: &CalendarEvent) -> bool {
    if !a.remote.id.is_empty() || !b.remote.id.is_empty() {
        return a.remote.id == b.remote.id;
    }
    a.uid == b.uid && a.recurrence_id == b.recurrence_id
}

/// Puts `e` in `events`, in place of the same one if it is there.
pub(crate) fn upsert_event(events: &mut Vec<CalendarEvent>, e: CalendarEvent) {
    match events.iter_mut().find(|x| same_event(x, &e)) {
        Some(x) => *x = e,
        None => events.push(e),
    }
}

pub(crate) fn upsert_task(tasks: &mut Vec<Task>, t: Task) {
    match tasks.iter_mut().find(|x| x.remote.id == t.remote.id) {
        Some(x) => *x = t,
        None => tasks.push(t),
    }
}

/// The event a change is about.
fn event_of_mut(c: &mut PimChange) -> Option<&mut CalendarEvent> {
    match c {
        PimChange::SaveEvent { event, .. } | PimChange::DeleteEvent { event, .. } | PimChange::Respond { event, .. } => Some(event),
        PimChange::SaveTask { .. } | PimChange::DeleteTask { .. } => None,
    }
}

/// The cached event an invitation or an answer is about: by the provider's id, else by UID (the
/// occurrence named, or the series). A series a provider keeps as occurrences (Graph) is answered
/// through its master's id.
pub(crate) fn find_event(store: &Store, e: &CalendarEvent) -> Option<CalendarEvent> {
    let all = store.all_events();
    if !e.remote.id.is_empty() {
        return all.into_iter().find(|x| x.remote.id == e.remote.id);
    }
    if e.uid.is_empty() {
        return None;
    }
    let same_uid = |x: &&CalendarEvent| x.uid == e.uid;
    if let Some(x) = all.iter().filter(same_uid).find(|x| x.recurrence_id == e.recurrence_id) {
        return Some(x.clone());
    }
    // The series, as one of its occurrences names it.
    if e.recurrence_id.is_none() {
        return all.iter().filter(same_uid).find_map(|x| {
            let series = x.series.clone()?;
            Some(CalendarEvent { remote: Remote::new(&series, ""), recurrence_id: None, series: None, ..x.clone() })
        });
    }
    None
}

/// The calendar new events go to: the primary one, else the first that can be changed.
pub(crate) fn default_calendar(store: &Store) -> Option<String> {
    let calendars = store.calendars();
    calendars.iter().find(|c| c.primary && c.can_edit).or_else(|| calendars.iter().find(|c| c.can_edit)).map(|c| c.id.clone())
}

/// The list new tasks go to: the default one, else the first.
pub(crate) fn default_list(store: &Store) -> Option<String> {
    let lists = store.task_lists();
    lists.iter().find(|l| l.default).or_else(|| lists.first()).map(|l| l.id.clone())
}

/// The cache's calendars and events, as the interface is told them.
pub(crate) fn calendars_event(store: &Store, account: AccountId) -> Event {
    Event::Calendars { account, calendars: store.calendars(), events: store.all_events() }
}

pub(crate) fn tasks_event(store: &Store, account: AccountId) -> Event {
    Event::Tasks { account, lists: store.task_lists(), tasks: store.all_tasks() }
}

pub(crate) fn contacts_event(store: &Store, account: AccountId) -> Event {
    Event::Contacts { account, contacts: store.contacts() }
}

/// Changes still waiting for the server, applied over a calendar's events just read.
fn overlay_events(store: &Store, calendar: &str, events: &mut Vec<CalendarEvent>) {
    for c in store.pim_pending() {
        match c {
            PimChange::SaveEvent { event, .. } if event.calendar == calendar => upsert_event(events, event),
            PimChange::DeleteEvent { event, .. } if event.calendar == calendar => events.retain(|x| !same_event(x, &event)),
            PimChange::Respond { event, answer, .. } if event.calendar == calendar => {
                if let Some(x) = events.iter_mut().find(|x| same_event(x, &event)) {
                    x.my_response = Some(answer);
                }
            }
            _ => {}
        }
    }
}

/// Changes still waiting for the server, applied over a list's tasks just read.
fn overlay_tasks(store: &Store, list: &str, tasks: &mut Vec<Task>) {
    for c in store.pim_pending() {
        match c {
            PimChange::SaveTask { task } if task.list == list => upsert_task(tasks, task),
            PimChange::DeleteTask { task } if task.list == list => tasks.retain(|x| x.remote.id != task.remote.id),
            _ => {}
        }
    }
}

/// Which part of the provider's data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Calendar,
    Tasks,
    Contacts,
}

/// What a calendar's sync brought.
pub(crate) enum CalendarSync {
    /// Every event of the window (Graph): they replace the cached ones there.
    Window(Vec<CalendarEvent>),
    /// What changed since the token given (Google; everything when none was given), and the
    /// token for the next time.
    Delta { changed: Vec<CalendarEvent>, removed: Vec<String>, token: String },
}

/// What this module keeps of a worker between two calls.
#[derive(Debug, Default)]
pub(crate) struct Memo {
    /// Problems already told the user (each once per run).
    told: HashSet<String>,
    /// The queue is being sent (guards against sending it twice).
    flushing: bool,
    /// Changes were queued while the worker was busy: send them once it is free.
    pub(crate) dirty: bool,
}

/// What a worker whose provider keeps the calendars gives this module: its cache, its way to tell
/// the interface, its connection, and the provider's calls (each through the worker's own
/// sign-in handling).
pub(crate) trait PimWorker {
    fn who(&self) -> AccountId;
    fn cache(&self) -> &Store;
    fn tell(&self, e: Event);
    fn zone_rules(&self) -> Zones;
    fn my_address(&self) -> String;
    /// The provider's name, as messages say it ("Outlook", "Google").
    fn name(&self) -> &'static str;
    /// Connects if needed: `false` when offline or refused (already said).
    fn reach(&mut self) -> bool;
    /// Whether the connection is still up (after a failure: a refusal, not the network).
    fn is_online(&self) -> bool;
    /// Whether the provider lets MailALM read this part (the permissions granted).
    fn allowed(&self, _part: Part) -> bool {
        true
    }
    fn memo(&mut self) -> &mut Memo;
    fn explain(&self, e: &NetError) -> String;

    fn calendars(&mut self) -> Result<Vec<Calendar>, NetError>;
    /// A calendar's events: the changes since `token` (none: everything), or the window.
    fn sync_calendar(&mut self, calendar: &Calendar, token: Option<&str>, window: (i64, i64)) -> Result<CalendarSync, NetError>;
    /// A calendar's events that overlap `[from, to)`.
    fn events_in(&mut self, calendar: &Calendar, from: i64, to: i64) -> Result<Vec<CalendarEvent>, NetError>;
    fn create_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError>;
    fn update_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError>;
    fn delete_event(&mut self, event: &CalendarEvent, notify: bool) -> Result<(), NetError>;
    fn respond(&mut self, event: &CalendarEvent, answer: Response, send: bool, comment: &str, proposal: Option<(i64, i64)>) -> Result<(), NetError>;
    /// Sends a message (a meeting message the provider's calendar can't send).
    fn send_mail(&mut self, message: &Outgoing) -> Result<(), NetError>;
    fn task_lists(&mut self) -> Result<Vec<TaskList>, NetError>;
    fn tasks(&mut self, list: &str) -> Result<Vec<Task>, NetError>;
    fn create_task(&mut self, task: &Task) -> Result<Task, NetError>;
    fn update_task(&mut self, task: &Task) -> Result<Task, NetError>;
    fn delete_task(&mut self, task: &Task) -> Result<(), NetError>;
    fn contacts(&mut self) -> Result<Vec<Contact>, NetError>;
}

/// The cache, as the interface is told it at start.
pub(crate) fn show_cache<W: PimWorker>(w: &W) {
    w.tell(calendars_event(w.cache(), w.who()));
    w.tell(tasks_event(w.cache(), w.who()));
    w.tell(contacts_event(w.cache(), w.who()));
}

/// Tells a problem once per run; offline, the account's link already says it.
fn problem<W: PimWorker>(w: &mut W, what: &str, e: &NetError) {
    if !w.is_online() {
        return;
    }
    let text = format!("{what}: {}", w.explain(e));
    if w.memo().told.insert(text.clone()) {
        w.tell(Event::Error { account: w.who(), text });
    }
}

/// Tells something once per run.
fn note_once<W: PimWorker>(w: &mut W, text: String) {
    if w.memo().told.insert(text.clone()) {
        w.tell(Event::Notice { account: w.who(), text });
    }
}

/// Puts a change at the end of the queue.
fn queue<W: PimWorker>(w: &mut W, change: PimChange) {
    let store = w.cache().clone();
    let mut pending = store.pim_pending();
    pending.push(change);
    if let Err(e) = store.save_pim_pending(&pending) {
        w.tell(Event::Error { account: w.who(), text: format!("The change could not be kept for {}: {e}", w.name()) });
    }
    w.memo().dirty = true;
}

/// A command's work: the change made in the cache and queued now, the rest (`Sync`, `Range`)
/// given back to be done when the worker is free. Never waits on the network.
pub(crate) fn take_local<W: PimWorker>(w: &mut W, command: PimCommand) -> Option<PimCommand> {
    let store = w.cache().clone();
    let account = w.who();
    let refuse = |w: &W, text: &str| w.tell(Event::Error { account: w.who(), text: text.to_owned() });
    match command {
        PimCommand::Sync | PimCommand::Range { .. } => return Some(command),
        PimCommand::Invitation { folder, uid, index } => {
            let zones = w.zone_rules();
            w.tell(local::invitation(&store, account, &folder, uid, index, &*zones));
            return None;
        }
        PimCommand::SaveEvent { event, notify } => {
            let mut e = *event;
            if e.calendar.is_empty() {
                match default_calendar(&store) {
                    Some(c) => e.calendar = c,
                    None => {
                        refuse(&*w, "This account has no calendar MailALM can add to yet: wait for its sync.");
                        return None;
                    }
                }
            }
            if !store.calendars().iter().any(|c| c.id == e.calendar && c.can_edit) {
                refuse(&*w, "This calendar can't be changed.");
                return None;
            }
            if e.uid.is_empty() {
                e.uid = new_uid(&w.my_address());
            }
            let mut events = store.events(&e.calendar);
            upsert_event(&mut events, e.clone());
            let _ = store.save_events(&e.calendar, &events);
            queue(w, PimChange::SaveEvent { event: e, notify });
            w.tell(calendars_event(&store, account));
        }
        PimCommand::DeleteEvent { event, notify } => {
            let e = *event;
            let mut events = store.events(&e.calendar);
            events.retain(|x| !same_event(x, &e));
            let _ = store.save_events(&e.calendar, &events);
            if e.remote.id.is_empty() {
                // Never made on the server: its making is forgotten.
                let mut pending = store.pim_pending();
                pending.retain(|c| !matches!(c, PimChange::SaveEvent { event, .. } if event.remote.id.is_empty() && event.uid == e.uid));
                let _ = store.save_pim_pending(&pending);
            } else {
                queue(w, PimChange::DeleteEvent { event: e, notify });
            }
            w.tell(calendars_event(&store, account));
        }
        PimCommand::Respond { event, answer, send, comment, proposal } => {
            let me = w.my_address();
            let target = match find_event(&store, &event) {
                Some(mut cached) => {
                    cached.my_response = Some(answer);
                    if let Some(a) = cached.attendees.iter_mut().find(|a| a.is(&me)) {
                        a.response = answer;
                    }
                    // The event, or every occurrence of the series answered.
                    let mut events = store.events(&cached.calendar);
                    for x in events.iter_mut().filter(|x| x.uid == cached.uid && (cached.recurrence_id.is_none() || x.recurrence_id == cached.recurrence_id)) {
                        x.my_response = Some(answer);
                    }
                    let _ = store.save_events(&cached.calendar, &events);
                    cached
                }
                None => *event,
            };
            queue(w, PimChange::Respond { event: target, answer, send, comment, proposal });
            w.tell(calendars_event(&store, account));
        }
        PimCommand::ApplyInvitation { method, events } => {
            if method != Method::Cancel {
                // The provider takes updates and answers in by itself: read what it has now.
                return Some(PimCommand::Sync);
            }
            for e in events {
                let probe = CalendarEvent { remote: Remote::default(), ..e };
                while let Some(found) = find_event(&store, &probe).filter(|f| !f.remote.id.is_empty()) {
                    let mut kept = store.events(&found.calendar);
                    let before = kept.len();
                    kept.retain(|x| !same_event(x, &found) && !(x.uid == found.uid && probe.recurrence_id.is_none()));
                    let _ = store.save_events(&found.calendar, &kept);
                    queue(w, PimChange::DeleteEvent { event: found, notify: false });
                    if kept.len() == before {
                        break;
                    }
                }
            }
            w.tell(calendars_event(&store, account));
        }
        PimCommand::SaveTask(task) => {
            let mut t = *task;
            if t.list.is_empty() {
                match default_list(&store) {
                    Some(l) => t.list = l,
                    None => {
                        refuse(&*w, "This account has no task list yet: wait for its sync.");
                        return None;
                    }
                }
            }
            if t.remote.id.is_empty() {
                t.remote = Remote::new(&local_key(), "");
            }
            let mut tasks = store.tasks(&t.list);
            upsert_task(&mut tasks, t.clone());
            let _ = store.save_tasks(&t.list, &tasks);
            queue(w, PimChange::SaveTask { task: t });
            w.tell(tasks_event(&store, account));
        }
        PimCommand::DeleteTask(task) => {
            let t = *task;
            let mut tasks = store.tasks(&t.list);
            tasks.retain(|x| x.remote.id != t.remote.id);
            let _ = store.save_tasks(&t.list, &tasks);
            if is_local_key(&t.remote.id) {
                let mut pending = store.pim_pending();
                pending.retain(|c| !matches!(c, PimChange::SaveTask { task } if task.remote.id == t.remote.id));
                let _ = store.save_pim_pending(&pending);
            } else {
                queue(w, PimChange::DeleteTask { task: t });
            }
            w.tell(tasks_event(&store, account));
        }
    }
    None
}

/// A command, done now: the change made and sent, or the sync or range read.
pub(crate) fn handle<W: PimWorker>(w: &mut W, command: PimCommand) {
    match take_local(w, command) {
        Some(PimCommand::Sync) => sync(w),
        Some(PimCommand::Range { from, to }) => range(w, from, to),
        Some(_) => {}
        None => flush(w),
    }
}

/// Sends the waiting changes, then syncs again if the provider refused one (the cache shows what
/// it has).
pub(crate) fn flush<W: PimWorker>(w: &mut W) {
    w.memo().dirty = false;
    if flush_now(w) {
        sync(w);
    }
}

/// Sends the waiting changes, in order. A change the provider refuses is said and dropped; a
/// network failure keeps it and the rest for next time. Returns whether one was refused.
fn flush_now<W: PimWorker>(w: &mut W) -> bool {
    let store = w.cache().clone();
    if w.memo().flushing || store.pim_pending().is_empty() || !w.reach() {
        return false;
    }
    w.memo().flushing = true;
    let mut pending = store.pim_pending();
    let (mut refused, mut events_changed, mut tasks_changed) = (false, false, false);
    while let Some(change) = pending.first().cloned() {
        let is_task = matches!(change, PimChange::SaveTask { .. } | PimChange::DeleteTask { .. });
        match send(w, &store, &change, &mut pending) {
            Ok(()) => {
                pending.remove(0);
            }
            Err(e) if w.is_online() || matches!(e, NetError::Invalid(_)) => {
                let what = if is_task { "a task" } else { "the calendar" };
                w.tell(Event::Error { account: w.who(), text: format!("{} refused a change to {what}: {}", w.name(), w.explain(&e)) });
                pending.remove(0);
                forget_unmade(&store, &change);
                refused = true;
            }
            Err(_) => break,
        }
        if let Err(e) = store.save_pim_pending(&pending) {
            w.tell(Event::Error { account: w.who(), text: format!("The changes waiting for {} could not be kept: {e}", w.name()) });
        }
        if is_task {
            tasks_changed = true;
        } else {
            events_changed = true;
        }
    }
    w.memo().flushing = false;
    if events_changed {
        w.tell(calendars_event(&store, w.who()));
    }
    if tasks_changed {
        w.tell(tasks_event(&store, w.who()));
    }
    refused
}

/// An event or a task the provider refused to make: out of the cache (it was never anywhere else).
fn forget_unmade(store: &Store, change: &PimChange) {
    match change {
        PimChange::SaveEvent { event, .. } if event.remote.id.is_empty() => {
            let mut events = store.events(&event.calendar);
            events.retain(|x| !(x.remote.id.is_empty() && x.uid == event.uid));
            let _ = store.save_events(&event.calendar, &events);
        }
        PimChange::SaveTask { task } if is_local_key(&task.remote.id) => {
            let mut tasks = store.tasks(&task.list);
            tasks.retain(|x| x.remote.id != task.remote.id);
            let _ = store.save_tasks(&task.list, &tasks);
        }
        _ => {}
    }
}

/// One change to the provider. `pending` is the queue (this change first): later changes to an
/// event or a task just made are made to name it by the provider's id.
fn send<W: PimWorker>(w: &mut W, store: &Store, change: &PimChange, pending: &mut [PimChange]) -> Result<(), NetError> {
    match change {
        PimChange::SaveEvent { event, .. } if event.remote.id.is_empty() => {
            let made = w.create_event(event)?;
            let mut events = store.events(&event.calendar);
            events.retain(|x| !(x.remote.id.is_empty() && x.uid == event.uid));
            upsert_event(&mut events, made.clone());
            let _ = store.save_events(&event.calendar, &events);
            for c in pending.iter_mut().skip(1) {
                if let Some(e) = event_of_mut(c)
                    && e.remote.id.is_empty()
                    && e.uid == event.uid
                {
                    e.remote = made.remote.clone();
                    e.uid = made.uid.clone();
                }
            }
            Ok(())
        }
        PimChange::SaveEvent { event, .. } => {
            let back = w.update_event(event)?;
            let mut events = store.events(&event.calendar);
            upsert_event(&mut events, back);
            let _ = store.save_events(&event.calendar, &events);
            Ok(())
        }
        PimChange::DeleteEvent { event, notify } => {
            if event.remote.id.is_empty() {
                Ok(())
            } else {
                w.delete_event(event, *notify)
            }
        }
        PimChange::Respond { event, answer, send, comment, proposal } => {
            let target = if event.remote.id.is_empty() { find_event(store, event) } else { Some(event.clone()) };
            match target.filter(|t| !t.remote.id.is_empty()) {
                Some(t) => w.respond(&t, *answer, *send, comment, *proposal),
                None if *send => {
                    // The provider's calendar doesn't have it: the answer goes by mail.
                    let me = Person { name: String::new(), address: w.my_address() };
                    let zones = w.zone_rules();
                    let message = itip::reply(event, &me, *answer, *proposal, comment, &*zones, now_unix())
                        .ok_or_else(|| NetError::Invalid("the invitation names no organizer to answer".to_owned()))?;
                    w.send_mail(&message)?;
                    w.tell(Event::Notice {
                        account: w.who(),
                        text: format!("This meeting isn't in {}'s calendar: the answer was sent to its organizer by mail.", w.name()),
                    });
                    Ok(())
                }
                None => Ok(()),
            }
        }
        PimChange::SaveTask { task } if is_local_key(&task.remote.id) => {
            let made = w.create_task(task)?;
            let mut tasks = store.tasks(&task.list);
            tasks.retain(|x| x.remote.id != task.remote.id);
            upsert_task(&mut tasks, made.clone());
            let _ = store.save_tasks(&task.list, &tasks);
            for c in pending.iter_mut().skip(1) {
                if let PimChange::SaveTask { task: t } | PimChange::DeleteTask { task: t } = c
                    && t.remote.id == task.remote.id
                {
                    t.remote = made.remote.clone();
                }
            }
            Ok(())
        }
        PimChange::SaveTask { task } => {
            let back = w.update_task(task)?;
            let mut tasks = store.tasks(&task.list);
            upsert_task(&mut tasks, back);
            let _ = store.save_tasks(&task.list, &tasks);
            Ok(())
        }
        PimChange::DeleteTask { task } => {
            if is_local_key(&task.remote.id) {
                Ok(())
            } else {
                w.delete_task(task)
            }
        }
    }
}

/// Syncs the calendars, tasks and contacts (each part the provider lets MailALM read), after
/// sending the waiting changes.
pub(crate) fn sync<W: PimWorker>(w: &mut W) {
    if !w.reach() {
        return;
    }
    flush_now(w);
    let account = w.who();
    w.tell(Event::Activity { account, text: Some("Syncing the calendars and tasksâ€¦".to_owned()) });
    let name = w.name();
    if w.allowed(Part::Calendar) {
        sync_calendars(w);
    } else {
        note_once(w, format!("{name} did not give MailALM access to this account's calendar: sign in again from the account's settings and allow it."));
    }
    if w.allowed(Part::Tasks) {
        sync_tasks(w);
    } else {
        note_once(w, format!("{name} did not give MailALM access to this account's tasks: sign in again from the account's settings and allow them."));
    }
    if w.allowed(Part::Contacts) {
        match w.contacts() {
            Ok(contacts) => {
                let store = w.cache().clone();
                let _ = store.save_contacts(&contacts);
                w.tell(contacts_event(&store, account));
            }
            Err(e) => problem(w, "The contacts could not be synced", &e),
        }
    }
    w.tell(Event::Activity { account, text: None });
}

fn sync_calendars<W: PimWorker>(w: &mut W) {
    let store = w.cache().clone();
    let calendars = match w.calendars() {
        Ok(c) => c,
        Err(e) => {
            problem(w, "The calendars could not be synced", &e);
            return;
        }
    };
    let _ = store.save_calendars(&calendars);
    let _ = store.forget_calendars_except(&calendars);
    let mut state = store.pim_state();
    state.tokens.retain(|id, _| calendars.iter().any(|c| &c.id == id));
    let win = window(now_unix());
    let mut windowed = false;
    for cal in &calendars {
        let mut token = state.tokens.get(&cal.id).cloned();
        let mut answer = w.sync_calendar(cal, token.as_deref(), win);
        if matches!(answer, Err(NetError::Resync(_))) {
            // The provider forgot where the last sync ended: from the start.
            token = None;
            state.tokens.remove(&cal.id);
            answer = w.sync_calendar(cal, None, win);
        }
        // Events not yet on the server come back from the queue (`overlay_events`): one whose
        // making was refused is gone.
        let mut events: Vec<CalendarEvent> = store.events(&cal.id).into_iter().filter(|e| !e.remote.id.is_empty()).collect();
        match answer {
            Ok(CalendarSync::Window(fresh)) => {
                events.retain(|e| !overlaps(e, win));
                events.extend(fresh);
                state.tokens.remove(&cal.id);
                windowed = true;
            }
            Ok(CalendarSync::Delta { changed, removed, token: next }) => {
                if token.is_none() {
                    events.clear();
                }
                let removed: HashSet<String> = removed.into_iter().collect();
                events.retain(|e| !removed.contains(&e.remote.id));
                for e in changed {
                    upsert_event(&mut events, e);
                }
                // What the calendar can show: series and their occurrences, and the single events
                // from the window's start on.
                events.retain(|e| e.recurrence.is_some() || e.recurrence_id.is_some() || e.end >= win.0);
                state.tokens.insert(cal.id.clone(), next);
            }
            Err(e) => {
                problem(w, &format!("The calendar {} could not be synced", cal.name), &e);
                if !w.is_online() {
                    break;
                }
                continue;
            }
        }
        overlay_events(&store, &cal.id, &mut events);
        events.truncate(MAX_KEPT_EVENTS);
        let _ = store.save_events(&cal.id, &events);
    }
    if windowed {
        state.window = Some(win);
        state.ranges.retain(|(a, b)| !(win.0 <= *a && *b <= win.1));
    }
    let _ = store.save_pim_state(&state);
    w.tell(calendars_event(&store, w.who()));
}

fn sync_tasks<W: PimWorker>(w: &mut W) {
    let store = w.cache().clone();
    let lists = match w.task_lists() {
        Ok(l) => l,
        Err(e) => {
            problem(w, "The task lists could not be synced", &e);
            return;
        }
    };
    let _ = store.save_task_lists(&lists);
    let _ = store.forget_lists_except(&lists);
    for list in &lists {
        let fresh = match w.tasks(&list.id) {
            Ok(t) => t,
            Err(e) => {
                problem(w, &format!("The task list {} could not be synced", list.name), &e);
                if !w.is_online() {
                    break;
                }
                continue;
            }
        };
        // Tasks not yet made on the server come back from the queue.
        let mut tasks = fresh;
        overlay_tasks(&store, &list.id, &mut tasks);
        let _ = store.save_tasks(&list.id, &tasks);
    }
    w.tell(tasks_event(&store, w.who()));
}

/// Reads `[from, to)` for the calendars that keep a window, unless it was read already.
fn range<W: PimWorker>(w: &mut W, from: i64, to: i64) {
    if to <= from || to.saturating_sub(from) > MAX_RANGE {
        return;
    }
    let store = w.cache().clone();
    let mut state = store.pim_state();
    let calendars: Vec<Calendar> = store.calendars().into_iter().filter(|c| !state.tokens.contains_key(&c.id)).collect();
    // A calendar synced with a token has every event already.
    if calendars.is_empty() || state.covers(from, to) || !w.reach() {
        return;
    }
    for cal in &calendars {
        match w.events_in(cal, from, to) {
            Ok(fresh) => {
                let mut events = store.events(&cal.id);
                events.retain(|e| !e.remote.id.is_empty() && (e.recurrence.is_some() || !overlaps(e, (from, to))));
                events.extend(fresh);
                overlay_events(&store, &cal.id, &mut events);
                events.truncate(MAX_KEPT_EVENTS);
                let _ = store.save_events(&cal.id, &events);
            }
            Err(e) => {
                problem(w, &format!("The calendar {} could not be read", cal.name), &e);
                return;
            }
        }
    }
    state.ranges.push((from, to));
    if state.ranges.len() > MAX_RANGES {
        state.ranges.remove(0);
    }
    let _ = store.save_pim_state(&state);
    w.tell(calendars_event(&store, w.who()));
}

#[cfg(test)]
mod tests;
