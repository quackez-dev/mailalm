//! A Gmail account's calendars (Google Calendar, with Meet links), tasks (Google Tasks) and
//! contacts (People API, read only), on the mail's sign-in (`pim_sync` does the rest). A
//! calendar is synced by its sync token: series with their rules, and what changed since.
//!
//! Google's consent screen lets the user untick the calendar, tasks or contacts: a part not
//! granted is not asked for (its page says so), and a refusal of one of them never stops the mail.

use std::sync::Arc;

use mailalm_mime::Outgoing;
use mailalm_model::pim::{Calendar, Contact, Event as CalendarEvent, Response, Task, TaskList};
use mailalm_model::{AccountId, Person};
use mailalm_net::NetError;
use mailalm_net::google::GoogleClient;
use mailalm_net::oauth::now_unix;
use mailalm_store::Store;

use super::{Gmail, describe};
use crate::pim_sync::{CalendarSync, Memo, Part, PimWorker};
use crate::{Event, Zones, itip};

/// The permissions each part needs (as `mailalm_net::oauth` asks for them).
const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar";
const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";
const CONTACTS_SCOPE: &str = "https://www.googleapis.com/auth/contacts.readonly";

impl Gmail {
    /// Runs one Calendar, Tasks or People call. Refused once (the access token ran out early):
    /// signed in again and tried once more. A second refusal is returned without stopping the
    /// account (Google refuses a permission the user didn't give). A network failure drops the
    /// connection, as for the mail.
    fn gcall<T>(&mut self, mut step: impl FnMut(&mut GoogleClient) -> Result<T, NetError>) -> Result<T, NetError> {
        for attempt in 0..2 {
            if self.connect().is_none() {
                return Err(NetError::Io("offline".to_owned()));
            }
            let Some(g) = self.google.as_mut() else { return Err(NetError::Io("offline".to_owned())) };
            match step(g) {
                Err(e) if e.is_auth() && attempt == 0 => {
                    // The token is taken for expired: the next connect asks for a new one.
                    self.tokens = None;
                }
                Err(e) => {
                    if e.is_fatal() && !matches!(e, NetError::TooLarge(_)) {
                        self.client = None;
                    }
                    return Err(e);
                }
                ok => return ok,
            }
        }
        Err(NetError::Auth("Google refused the sign-in".to_owned()))
    }
}

impl PimWorker for Gmail {
    fn who(&self) -> AccountId {
        self.id
    }

    fn cache(&self) -> &Store {
        &self.store
    }

    fn tell(&self, e: Event) {
        self.emit(e);
    }

    fn zone_rules(&self) -> Zones {
        Arc::clone(&self.zones)
    }

    fn my_address(&self) -> String {
        self.address.clone()
    }

    fn name(&self) -> &'static str {
        "Google"
    }

    fn reach(&mut self) -> bool {
        self.connect().is_some()
    }

    fn is_online(&self) -> bool {
        self.online()
    }

    fn allowed(&self, part: Part) -> bool {
        let scope = match part {
            Part::Calendar => CALENDAR_SCOPE,
            Part::Tasks => TASKS_SCOPE,
            Part::Contacts => CONTACTS_SCOPE,
        };
        self.tokens.as_ref().is_none_or(|t| t.granted(scope))
    }

    fn memo(&mut self) -> &mut Memo {
        &mut self.pim
    }

    fn explain(&self, e: &NetError) -> String {
        describe(e)
    }

    fn calendars(&mut self) -> Result<Vec<Calendar>, NetError> {
        self.gcall(|g| g.calendars())
    }

    fn sync_calendar(&mut self, calendar: &Calendar, token: Option<&str>, (from, to): (i64, i64)) -> Result<CalendarSync, NetError> {
        match self.gcall(|g| g.sync_events(&calendar.id, token)) {
            Ok(s) => Ok(CalendarSync::Delta { changed: s.changed, removed: s.removed, token: s.token }),
            // Too many events to keep them all: the window alone, each occurrence on its own.
            Err(NetError::TooLarge(_)) => self.events_in(calendar, from, to).map(CalendarSync::Window),
            Err(e) => Err(e),
        }
    }

    fn events_in(&mut self, calendar: &Calendar, from: i64, to: i64) -> Result<Vec<CalendarEvent>, NetError> {
        self.gcall(|g| g.events(&calendar.id, from, to))
    }

    fn create_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError> {
        let zones = Arc::clone(&self.zones);
        self.gcall(|g| g.create_event(&event.calendar, event, &*zones))
    }

    fn update_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError> {
        let zones = Arc::clone(&self.zones);
        self.gcall(|g| g.update_event(event, &*zones))
    }

    fn delete_event(&mut self, event: &CalendarEvent, notify: bool) -> Result<(), NetError> {
        self.gcall(|g| g.delete_event(&event.calendar, &event.remote.id, notify))
    }

    fn respond(&mut self, event: &CalendarEvent, answer: Response, send: bool, comment: &str, proposal: Option<(i64, i64)>) -> Result<(), NetError> {
        if proposal.is_some() {
            // Google Calendar's API can't propose another time: the proposal goes by mail
            // (an iTIP COUNTER), as Google Calendar's own pages send it.
            let me = Person { name: String::new(), address: self.address.clone() };
            let message = itip::reply(event, &me, answer, proposal, comment, &*self.zones, now_unix())
                .ok_or_else(|| NetError::Invalid("this meeting names no organizer to send the proposal to".to_owned()))?;
            self.send_mail(&message)?;
            self.emit(Event::Notice { account: self.id, text: "Google Calendar can't propose another time itself: the proposal was sent to the organizer by mail.".to_owned() });
            return Ok(());
        }
        self.gcall(|g| g.respond(&event.calendar, &event.remote.id, answer, send, comment, None))
    }

    fn send_mail(&mut self, message: &Outgoing) -> Result<(), NetError> {
        let (raw, _) = mailalm_mime::build_for_api(message).map_err(NetError::Invalid)?;
        self.call(|c| c.send_mime(&raw))
    }

    fn task_lists(&mut self) -> Result<Vec<TaskList>, NetError> {
        self.gcall(|g| g.task_lists())
    }

    fn tasks(&mut self, list: &str) -> Result<Vec<Task>, NetError> {
        self.gcall(|g| g.tasks(list))
    }

    fn create_task(&mut self, task: &Task) -> Result<Task, NetError> {
        self.gcall(|g| g.create_task(&task.list, task))
    }

    fn update_task(&mut self, task: &Task) -> Result<Task, NetError> {
        self.gcall(|g| g.update_task(task))
    }

    fn delete_task(&mut self, task: &Task) -> Result<(), NetError> {
        self.gcall(|g| g.delete_task(&task.list, &task.remote.id))
    }

    fn contacts(&mut self) -> Result<Vec<Contact>, NetError> {
        self.gcall(|g| g.contacts())
    }
}
