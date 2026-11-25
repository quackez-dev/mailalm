//! The calendars, events, tasks and contacts of an account (phase 6), beside its mail:
//!
//! ```text
//! <base>/<account id>/pim/
//!   calendars.json   the calendars
//!   e-<key>.json     a calendar's events: series with their rules, single events, occurrences
//!   lists.json       the task lists
//!   t-<key>.json     a list's tasks
//!   contacts.json    the contacts
//!   state.json       where the syncs left off (Google's sync tokens, the window of events read)
//!   pending.json     changes not yet on the server, in order (C7)
//! ```
//!
//! `<key>` is a hash of the calendar's or list's id, so no id sent by a server ever becomes a file
//! name; each file keeps the id it is for, checked on reading. An IMAP account's calendar and task
//! list are kept here only (`mailalm_model::pim::LOCAL`).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use mailalm_model::pim::{Calendar, Contact, Event, Response, Task, TaskList};
use serde::{Deserialize, Serialize};

use super::{Store, read_json, write_json};

/// Where the syncs of an account's calendars left off.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PimState {
    /// Google: each calendar's sync token (by calendar id). A calendar with a token has all its
    /// events in the cache (its series with their rules).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tokens: BTreeMap<String, String>,
    /// The window of events read last (UTC seconds), for a provider that gives a window (Graph).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<(i64, i64)>,
    /// Other ranges read since, at the calendar page's request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<(i64, i64)>,
}

impl PimState {
    /// Whether `[from, to)` was read: inside the window or a range read since.
    pub fn covers(&self, from: i64, to: i64) -> bool {
        self.window.iter().chain(self.ranges.iter()).any(|(a, b)| *a <= from && to <= *b)
    }
}

/// A change to a calendar or a task list not yet on the server, kept in order (C7). An event or
/// task the server has never had carries no remote id (a task: a local key, see the engine).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PimChange {
    /// Makes the event (no remote id yet) or changes it. `notify`: its guests hear of it.
    SaveEvent { event: Event, notify: bool },
    DeleteEvent { event: Event, notify: bool },
    /// The user's answer to an invitation.
    Respond { event: Event, answer: Response, send: bool, comment: String, proposal: Option<(i64, i64)> },
    /// Makes the task (a local key as its id) or changes it (done or not, too).
    SaveTask { task: Task },
    DeleteTask { task: Task },
}

/// A calendar's events, with the calendar they are for.
#[derive(Serialize, Deserialize)]
struct EventsFile {
    calendar: String,
    events: Vec<Event>,
}

/// A list's tasks, with the list they are for.
#[derive(Serialize, Deserialize)]
struct TasksFile {
    list: String,
    tasks: Vec<Task>,
}

/// A file name for an id: `prefix`, a hash of the id (FNV-1a, 64 bits), `.json`.
fn file_name(prefix: &str, id: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{prefix}-{h:016x}.json")
}

impl Store {
    fn pim_dir(&self) -> PathBuf {
        self.root.join("pim")
    }

    fn pim_read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Option<T> {
        read_json(&self.pim_dir().join(name))
    }

    fn pim_write<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> io::Result<()> {
        let dir = self.pim_dir();
        fs::create_dir_all(&dir)?;
        write_json(&dir.join(name), value)
    }

    /// Removes the files of `prefix` that aren't among `keep`.
    fn pim_forget(&self, prefix: &str, keep: &[String]) -> io::Result<()> {
        let dir = self.pim_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let start = format!("{prefix}-");
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&start) && name.ends_with(".json") && !keep.contains(&name) {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    pub fn calendars(&self) -> Vec<Calendar> {
        self.pim_read("calendars.json").unwrap_or_default()
    }

    pub fn save_calendars(&self, calendars: &[Calendar]) -> io::Result<()> {
        self.pim_write("calendars.json", calendars)
    }

    /// The events kept for a calendar.
    pub fn events(&self, calendar: &str) -> Vec<Event> {
        self.pim_read::<EventsFile>(&file_name("e", calendar)).filter(|f| f.calendar == calendar).map(|f| f.events).unwrap_or_default()
    }

    pub fn save_events(&self, calendar: &str, events: &[Event]) -> io::Result<()> {
        self.pim_write(&file_name("e", calendar), &EventsFile { calendar: calendar.to_owned(), events: events.to_vec() })
    }

    /// The events of every calendar the cache lists.
    pub fn all_events(&self) -> Vec<Event> {
        self.calendars().iter().flat_map(|c| self.events(&c.id)).collect()
    }

    /// Removes the events of calendars no longer listed.
    pub fn forget_calendars_except(&self, keep: &[Calendar]) -> io::Result<()> {
        let names: Vec<String> = keep.iter().map(|c| file_name("e", &c.id)).collect();
        self.pim_forget("e", &names)
    }

    pub fn task_lists(&self) -> Vec<TaskList> {
        self.pim_read("lists.json").unwrap_or_default()
    }

    pub fn save_task_lists(&self, lists: &[TaskList]) -> io::Result<()> {
        self.pim_write("lists.json", lists)
    }

    /// The tasks kept for a list.
    pub fn tasks(&self, list: &str) -> Vec<Task> {
        self.pim_read::<TasksFile>(&file_name("t", list)).filter(|f| f.list == list).map(|f| f.tasks).unwrap_or_default()
    }

    pub fn save_tasks(&self, list: &str, tasks: &[Task]) -> io::Result<()> {
        self.pim_write(&file_name("t", list), &TasksFile { list: list.to_owned(), tasks: tasks.to_vec() })
    }

    /// The tasks of every list the cache lists.
    pub fn all_tasks(&self) -> Vec<Task> {
        self.task_lists().iter().flat_map(|l| self.tasks(&l.id)).collect()
    }

    /// Removes the tasks of lists no longer listed.
    pub fn forget_lists_except(&self, keep: &[TaskList]) -> io::Result<()> {
        let names: Vec<String> = keep.iter().map(|l| file_name("t", &l.id)).collect();
        self.pim_forget("t", &names)
    }

    pub fn contacts(&self) -> Vec<Contact> {
        self.pim_read("contacts.json").unwrap_or_default()
    }

    pub fn save_contacts(&self, contacts: &[Contact]) -> io::Result<()> {
        self.pim_write("contacts.json", contacts)
    }

    pub fn pim_state(&self) -> PimState {
        self.pim_read("state.json").unwrap_or_default()
    }

    pub fn save_pim_state(&self, state: &PimState) -> io::Result<()> {
        self.pim_write("state.json", state)
    }

    /// Changes to calendars and tasks waiting for the server, oldest first.
    pub fn pim_pending(&self) -> Vec<PimChange> {
        self.pim_read("pending.json").unwrap_or_default()
    }

    pub fn save_pim_pending(&self, changes: &[PimChange]) -> io::Result<()> {
        self.pim_write("pending.json", changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailalm_model::AccountId;
    use mailalm_model::pim::{LOCAL, Remote};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailalm-store-pim-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn calendar(id: &str) -> Calendar {
        Calendar { id: id.into(), remote: Remote::new(id, ""), name: id.into(), can_edit: true, ..Calendar::default() }
    }

    fn event(uid: &str, calendar: &str) -> Event {
        Event { uid: uid.into(), calendar: calendar.into(), title: "Mix".into(), start: 100, end: 200, ..Event::default() }
    }

    #[test]
    fn calendars_events_tasks_and_contacts_round_trip() {
        let base = temp("round-trip");
        let s = Store::open(&base, AccountId(4)).expect("open");
        assert!(s.calendars().is_empty() && s.all_events().is_empty() && s.contacts().is_empty());

        s.save_calendars(&[calendar("work@group.example"), calendar(LOCAL)]).expect("calendars");
        s.save_events("work@group.example", &[event("a", "work@group.example")]).expect("events");
        s.save_events(LOCAL, &[event("b", LOCAL), event("c", LOCAL)]).expect("events");
        assert_eq!(s.events(LOCAL).len(), 2);
        assert_eq!(s.all_events().len(), 3);

        let list = TaskList { id: "L1".into(), name: "Tasks".into(), default: true, ..TaskList::default() };
        s.save_task_lists(std::slice::from_ref(&list)).expect("lists");
        let task = Task { list: "L1".into(), title: "Mix the trailer".into(), due: Some(86_400), ..Task::default() };
        s.save_tasks("L1", std::slice::from_ref(&task)).expect("tasks");
        assert_eq!(s.all_tasks(), vec![task.clone()]);

        let contact = Contact { name: "Zoe".into(), emails: vec!["zoe@studio.example".into()], ..Contact::default() };
        s.save_contacts(std::slice::from_ref(&contact)).expect("contacts");
        assert_eq!(s.contacts(), vec![contact]);

        let state = PimState { tokens: [("work@group.example".to_owned(), "tok".to_owned())].into_iter().collect(), window: Some((0, 1000)), ranges: vec![(2000, 3000)] };
        s.save_pim_state(&state).expect("state");
        assert_eq!(s.pim_state(), state);
        assert!(state.covers(10, 900) && state.covers(2000, 3000) && !state.covers(900, 2100));

        let pending = vec![
            PimChange::SaveEvent { event: event("d", LOCAL), notify: true },
            PimChange::Respond { event: event("a", "work@group.example"), answer: Response::Accepted, send: true, comment: String::new(), proposal: None },
            PimChange::SaveTask { task: task.clone() },
            PimChange::DeleteTask { task },
        ];
        s.save_pim_pending(&pending).expect("pending");
        assert_eq!(s.pim_pending(), pending);

        // A calendar no longer listed loses its events; the others keep theirs.
        s.save_calendars(&[calendar(LOCAL)]).expect("calendars");
        s.forget_calendars_except(&s.calendars()).expect("forget");
        assert!(s.events("work@group.example").is_empty());
        assert_eq!(s.events(LOCAL).len(), 2);
        s.forget_lists_except(&[]).expect("forget");
        assert!(s.tasks("L1").is_empty());

        // The mail's folders are forgotten without touching the calendars.
        s.forget_folders_except(&[]).expect("forget folders");
        assert_eq!(s.events(LOCAL).len(), 2);
        s.delete_all().expect("delete");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn ids_never_reach_the_disk() {
        for hostile in ["../../Windows/x", "CON", "a\\b", "名前", ""] {
            let n = file_name("e", hostile);
            assert!(n.starts_with("e-") && n.ends_with(".json") && n.len() == 23, "{n}");
            assert!(n[2..18].chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn a_file_for_another_id_reads_as_empty() {
        let base = temp("other-id");
        let s = Store::open(&base, AccountId(1)).expect("open");
        s.save_events("a", &[event("x", "a")]).expect("events");
        // The file of "a" copied as the file of "b": it says it is for "a", so "b" has nothing.
        let dir = s.root().join("pim");
        fs::copy(dir.join(file_name("e", "a")), dir.join(file_name("e", "b"))).expect("copy");
        assert!(s.events("b").is_empty());
        fs::write(dir.join("calendars.json"), b"{not json").expect("write");
        assert!(s.calendars().is_empty());
        let _ = fs::remove_dir_all(&base);
    }
}
