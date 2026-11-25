//! What the interface works out from every account's cache, offline too: the reminders that are
//! due (PLAN.md, section 7), the clashes between calendars and the time taken in all of them (A3),
//! and the addresses to suggest while typing a recipient (K1).
//!
//! Pure: no I/O, no clock (the caller gives `now`), no zone database (the caller gives its rules).

use std::collections::{BTreeMap, HashMap};

use mailalm_model::pim::{BusyBlock, BusyStatus, Calendar, Event, EventStatus, MAX_REMINDER_MINUTES, Response, Task, ZoneRules, busy_blocks, occurrences, utc_time};
use mailalm_model::{AccountId, Person};

/// How long after an event ends its reminder still shows, if it was neither dismissed nor
/// snoozed (MailALM was not running when it was due).
const EVENT_GRACE: i64 = 86_400;
/// How long a task's reminder still shows after it was due.
const TASK_GRACE: i64 = 30 * 86_400;
/// How long a dismissal or a snooze is remembered after it no longer matters.
const KEEP: i64 = 60 * 86_400;
/// The most reminders shown at once.
const MAX_DUE: usize = 200;

/// One account's calendars, events and tasks, as the interface keeps them.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    pub account: AccountId,
    pub calendars: &'a [Calendar],
    pub events: &'a [Event],
    pub tasks: &'a [Task],
}

/// What a reminder is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reminded {
    /// An occurrence of an event (`event` indexes the source's events).
    Event { event: usize, start: i64, end: i64, all_day: bool, location: String },
    /// A task (`task` indexes the source's tasks).
    Task { task: usize, due: Option<i64> },
}

/// A reminder that is due.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    /// Names this reminder (account, item, occurrence, time before): what dismissing and snoozing
    /// keep.
    pub key: String,
    pub account: AccountId,
    pub title: String,
    /// When it rang (or rings again, after a snooze), UTC seconds.
    pub at: i64,
    pub what: Reminded,
}

/// The reminders dismissed and snoozed, by key: kept by the interface (on this computer) between
/// runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReminderState {
    /// Dismissed, with when they had rung.
    pub dismissed: BTreeMap<String, i64>,
    /// Snoozed, until when.
    pub snoozed: BTreeMap<String, i64>,
}

impl ReminderState {
    pub fn dismiss(&mut self, due: &Due) {
        self.snoozed.remove(&due.key);
        self.dismissed.insert(due.key.clone(), due.at);
    }

    pub fn snooze(&mut self, due: &Due, until: i64) {
        self.snoozed.insert(due.key.clone(), until);
    }

    /// Forgets what no longer matters (rung or snoozed more than 60 days ago).
    pub fn prune(&mut self, now: i64) {
        self.dismissed.retain(|_, at| *at > now.saturating_sub(KEEP));
        self.snoozed.retain(|_, until| *until > now.saturating_sub(KEEP));
    }
}

/// The reminders of these accounts due at `now`, oldest first: each reminder of each occurrence of
/// an event that takes place (not cancelled, not declined) whose time has come and which ended
/// less than a day ago, and each reminder of a task not done that came in the last 30 days; less
/// those dismissed, and those snoozed until later (a snoozed one rings again at its new time).
/// An event without reminders of its own takes its calendar's defaults when it says so. An
/// all-day event's reminders count from its first day's midnight in the viewer's zone.
pub fn due(sources: &[Source<'_>], state: &ReminderState, now: i64, zones: &dyn ZoneRules) -> Vec<Due> {
    let mut out: Vec<Due> = Vec::new();
    let ahead = i64::from(MAX_REMINDER_MINUTES).saturating_mul(60).saturating_add(86_400 * 2);
    for s in sources {
        for o in occurrences(s.events, now.saturating_sub(EVENT_GRACE + 86_400 * 2), now.saturating_add(ahead), zones) {
            let Some(e) = s.events.get(o.event) else { continue };
            if e.status == EventStatus::Cancelled || e.my_response == Some(Response::Declined) {
                continue;
            }
            let (start, end) = if e.all_day { (utc_time(zones, "", o.start), utc_time(zones, "", o.end)) } else { (o.start, o.end) };
            if end <= now.saturating_sub(EVENT_GRACE) {
                continue;
            }
            let minutes: &[u32] = if e.default_reminders && e.reminders.is_empty() {
                s.calendars.iter().find(|c| c.id == e.calendar).map(|c| c.default_reminders.as_slice()).unwrap_or_default()
            } else {
                &e.reminders
            };
            for m in minutes.iter().filter(|m| **m <= MAX_REMINDER_MINUTES) {
                let rings = start.saturating_sub(i64::from(*m).saturating_mul(60));
                let id = if e.remote.id.is_empty() { e.uid.as_str() } else { e.remote.id.as_str() };
                let key = format!("e|{}|{}|{id}|{}|{m}", s.account.0, e.calendar, o.start);
                let what = Reminded::Event { event: o.event, start, end, all_day: e.all_day, location: e.location.clone() };
                push_due(&mut out, state, now, key, s.account, &e.title, rings, what);
            }
        }
        for (i, t) in s.tasks.iter().enumerate() {
            let Some(rings) = t.reminder else { continue };
            if t.completed || rings < now.saturating_sub(TASK_GRACE) {
                continue;
            }
            let key = format!("t|{}|{}|{}|{rings}", s.account.0, t.list, t.remote.id);
            push_due(&mut out, state, now, key, s.account, &t.title, rings, Reminded::Task { task: i, due: t.due });
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.key.cmp(&b.key)));
    out.dedup_by(|a, b| a.key == b.key);
    out.truncate(MAX_DUE);
    out
}

#[allow(clippy::too_many_arguments)]
fn push_due(out: &mut Vec<Due>, state: &ReminderState, now: i64, key: String, account: AccountId, title: &str, rings: i64, what: Reminded) {
    if state.dismissed.contains_key(&key) {
        return;
    }
    let at = match state.snoozed.get(&key) {
        Some(until) if *until > now => return,
        Some(until) => *until,
        None if rings > now => return,
        None => rings,
    };
    out.push(Due { key, account, title: title.to_owned(), at, what });
}

/// An occurrence that takes time in a calendar, overlapping a time asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clash {
    pub account: AccountId,
    pub calendar: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub status: BusyStatus,
}

/// What `[start, end)` clashes with in every calendar of every account: each occurrence that
/// takes time (not free, not cancelled, not declined) and overlaps it, in order; an event of UID
/// `skip` (the one being planned or answered) left out. An all-day event takes its days in the
/// viewer's zone.
pub fn clashes(sources: &[Source<'_>], start: i64, end: i64, skip: &str, zones: &dyn ZoneRules) -> Vec<Clash> {
    let mut out = Vec::new();
    if end <= start {
        return out;
    }
    for s in sources {
        // Two days more each way: an all-day event's dates are UTC's, the viewer's day differs.
        for o in occurrences(s.events, start.saturating_sub(2 * 86_400), end.saturating_add(2 * 86_400), zones) {
            let Some(e) = s.events.get(o.event) else { continue };
            if !e.takes_time() || (!skip.is_empty() && e.uid == skip) {
                continue;
            }
            let (a, b) = if e.all_day { (utc_time(zones, "", o.start), utc_time(zones, "", o.end)) } else { (o.start, o.end) };
            if a < end && b > start {
                out.push(Clash { account: s.account, calendar: e.calendar.clone(), title: e.title.clone(), start: a, end: b, all_day: e.all_day, status: e.busy });
            }
        }
    }
    out.sort_by_key(|c| (c.start, c.end));
    out
}

/// The time taken in `[from, to)` in every calendar of every account, merged (A3).
pub fn busy(sources: &[Source<'_>], from: i64, to: i64, zones: &dyn ZoneRules) -> Vec<BusyBlock> {
    let mut blocks: Vec<BusyBlock> = Vec::new();
    for s in sources {
        blocks.extend(busy_blocks(s.events, from, to, zones));
    }
    mailalm_model::pim::merge_blocks(blocks)
}

/// The addresses offered while typing a recipient: every account's contacts, then the people seen
/// in mail (K1). One entry per address (case ignored); a contact's name wins over a name seen in
/// a header.
#[derive(Clone, Debug, Default)]
pub struct AddressBook {
    entries: Vec<Entry>,
    by_address: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
struct Entry {
    person: Person,
    name_lower: String,
    address_lower: String,
    contact: bool,
}

/// The most addresses kept.
const MAX_BOOK: usize = 100_000;

impl AddressBook {
    pub fn new() -> AddressBook {
        AddressBook::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds an address (checked) with a name; `contact`: from an address book, not from mail.
    pub fn add(&mut self, name: &str, address: &str, contact: bool) {
        let address = address.trim();
        if !mailalm_model::pim::address_ok(address) {
            return;
        }
        let name = mailalm_model::pim::clean_line(name, 200);
        let key = address.to_lowercase();
        if let Some(&i) = self.by_address.get(&key) {
            if let Some(e) = self.entries.get_mut(i)
                && (contact && !e.contact || e.person.name.is_empty())
                && !name.is_empty()
            {
                e.name_lower = name.to_lowercase();
                e.person.name = name;
                e.contact |= contact;
            }
            return;
        }
        if self.entries.len() >= MAX_BOOK {
            return;
        }
        self.by_address.insert(key.clone(), self.entries.len());
        self.entries.push(Entry { name_lower: name.to_lowercase(), person: Person { name, address: address.to_owned() }, address_lower: key, contact });
    }

    /// Up to `limit` people for what is typed: an address or a word of a name that starts with it
    /// first (contacts before addresses seen in mail), then those that contain it.
    pub fn suggest(&self, typed: &str, limit: usize) -> Vec<Person> {
        let t = typed.trim().to_lowercase();
        if t.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut ranked: Vec<(u8, &Entry)> = Vec::new();
        for e in &self.entries {
            let starts = e.address_lower.starts_with(&t) || e.name_lower.starts_with(&t) || e.name_lower.split_whitespace().any(|w| w.starts_with(&t));
            let rank = match (starts, e.contact) {
                (true, true) => 0,
                (true, false) => 1,
                (false, _) if e.address_lower.contains(&t) || e.name_lower.contains(&t) => 2 + u8::from(!e.contact),
                _ => continue,
            };
            ranked.push((rank, e));
        }
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name_lower.cmp(&b.1.name_lower)).then_with(|| a.1.address_lower.cmp(&b.1.address_lower)));
        ranked.into_iter().take(limit).map(|(_, e)| e.person.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailalm_model::pim::{Freq, Recurrence, Remote, UtcOnly};

    const H: i64 = 3600;
    /// 2026-11-25 00:00 UTC.
    const DAY: i64 = 1_795_564_800;

    fn event(uid: &str, start: i64, minutes: &[u32]) -> Event {
        Event { uid: uid.into(), calendar: "c".into(), remote: Remote::new(uid, ""), title: uid.into(), start, end: start + H, reminders: minutes.to_vec(), ..Event::default() }
    }

    fn source<'a>(events: &'a [Event], tasks: &'a [Task], calendars: &'a [Calendar]) -> Source<'a> {
        Source { account: AccountId(1), calendars, events, tasks }
    }

    #[test]
    fn reminders_ring_when_their_time_comes() {
        let events = [event("mix", DAY + 10 * H, &[15]), event("later", DAY + 20 * H, &[15])];
        let s = [source(&events, &[], &[])];
        let mut state = ReminderState::default();
        assert!(due(&s, &state, DAY + 9 * H, &UtcOnly).is_empty(), "too early");
        let now = DAY + 10 * H - 10 * 60;
        let d = due(&s, &state, now, &UtcOnly);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].title, "mix");
        assert_eq!(d[0].at, DAY + 10 * H - 15 * 60);

        // Snoozed: gone until its new time, then back.
        state.snooze(&d[0], now + 5 * 60);
        assert!(due(&s, &state, now + 60, &UtcOnly).is_empty());
        let again = due(&s, &state, now + 6 * 60, &UtcOnly);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].at, now + 5 * 60);

        // Dismissed: gone for good.
        state.dismiss(&again[0]);
        assert!(due(&s, &state, now + 30 * 60, &UtcOnly).is_empty());
        // Long after the event, nothing rings any more, dismissed or not.
        assert!(due(&s, &ReminderState::default(), DAY + 3 * 86_400, &UtcOnly).is_empty());
        state.prune(DAY + 100 * 86_400);
        assert!(state.dismissed.is_empty() && state.snoozed.is_empty());
    }

    #[test]
    fn each_occurrence_of_a_series_has_its_reminder() {
        let mut daily = event("standup", DAY + 9 * H, &[10]);
        daily.recurrence = Some(Recurrence::new(Freq::Daily));
        let s = [source(std::slice::from_ref(&daily), &[], &[])];
        let mut state = ReminderState::default();
        let first = due(&s, &state, DAY + 9 * H - 5 * 60, &UtcOnly);
        assert_eq!(first.len(), 1);
        state.dismiss(&first[0]);
        // The next day's occurrence rings on its own.
        let next = due(&s, &state, DAY + 86_400 + 9 * H - 5 * 60, &UtcOnly);
        assert_eq!(next.len(), 1);
        assert_ne!(next[0].key, first[0].key);
        assert!(matches!(next[0].what, Reminded::Event { start, .. } if start == DAY + 86_400 + 9 * H));
    }

    #[test]
    fn declined_and_cancelled_events_and_done_tasks_stay_quiet() {
        let mut declined = event("no", DAY + 10 * H, &[15]);
        declined.my_response = Some(Response::Declined);
        let mut cancelled = event("off", DAY + 10 * H, &[15]);
        cancelled.status = EventStatus::Cancelled;
        let events = [declined, cancelled];
        let tasks = [
            Task { list: "L".into(), remote: Remote::new("t1", ""), title: "pay".into(), reminder: Some(DAY + 8 * H), ..Task::default() },
            Task { list: "L".into(), remote: Remote::new("t2", ""), title: "done".into(), reminder: Some(DAY + 8 * H), completed: true, ..Task::default() },
        ];
        let d = due(&[source(&events, &tasks, &[])], &ReminderState::default(), DAY + 10 * H, &UtcOnly);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].title, "pay");
        assert!(matches!(d[0].what, Reminded::Task { task: 0, .. }));
    }

    #[test]
    fn calendar_defaults_apply_when_the_event_says_so() {
        let mut e = event("g", DAY + 10 * H, &[]);
        e.default_reminders = true;
        let calendars = [Calendar { id: "c".into(), default_reminders: vec![30], ..Calendar::default() }];
        let d = due(&[source(std::slice::from_ref(&e), &[], &calendars)], &ReminderState::default(), DAY + 10 * H - 20 * 60, &UtcOnly);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].at, DAY + 10 * H - 30 * 60);
    }

    #[test]
    fn clashes_come_from_every_account_but_not_the_event_itself() {
        let a = [event("mix", DAY + 10 * H, &[])];
        let mut free = event("lunch", DAY + 10 * H, &[]);
        free.busy = BusyStatus::Free;
        let b = [event("call", DAY + 10 * H + 1800, &[]), free, event("own", DAY + 10 * H, &[])];
        let sources = [source(&a, &[], &[]), Source { account: AccountId(2), calendars: &[], events: &b, tasks: &[] }];
        let c = clashes(&sources, DAY + 10 * H + 900, DAY + 11 * H, "own", &UtcOnly);
        let titles: Vec<&str> = c.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["mix", "call"], "free time and the event itself don't clash");
        assert_eq!(c[1].account, AccountId(2));
        assert!(clashes(&sources, DAY + 12 * H, DAY + 13 * H, "", &UtcOnly).is_empty());

        let blocks = busy(&sources, DAY, DAY + 86_400, &UtcOnly);
        assert_eq!(blocks.len(), 1, "merged: {blocks:?}");
        assert_eq!((blocks[0].start, blocks[0].end), (DAY + 10 * H, DAY + 11 * H + 1800));
    }

    #[test]
    fn suggestions_prefer_contacts_and_prefixes() {
        let mut book = AddressBook::new();
        book.add("Zoe Park", "zoe@studio.example", false);
        book.add("", "zack@example.com", false);
        book.add("Zoé Park (Studio)", "ZOE@studio.example", true);
        book.add("Anna Zorn", "anna@example.com", true);
        book.add("bad", "not an address", true);
        book.add("Liz", "liz@zoetrope.example", false);
        assert_eq!(book.len(), 4);
        let names: Vec<String> = book.suggest("zo", 10).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Anna Zorn", "Zoé Park (Studio)", "Liz"], "contacts first; a word of a name counts");
        assert_eq!(book.suggest("ZACK", 10).len(), 1);
        assert!(book.suggest("  ", 10).is_empty());
        assert_eq!(book.suggest("example", 1).len(), 1);
    }
}
