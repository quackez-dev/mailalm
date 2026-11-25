//! An Outlook account's calendars, Microsoft To Do tasks and contacts, through Microsoft Graph on
//! the mail's session (`pim_sync` does the rest). Graph gives a calendar's events as a window,
//! every occurrence of a series on its own; it tells a meeting's guests by itself, and makes a
//! Teams meeting when the account offers one (else its refusal says why).

use std::sync::Arc;

use mailalm_mime::Outgoing;
use mailalm_model::AccountId;
use mailalm_model::pim::{Calendar, Contact, Event as CalendarEvent, Response, Task, TaskList};
use mailalm_net::NetError;
use mailalm_store::Store;

use super::{Outlook, describe};
use crate::pim_sync::{CalendarSync, Memo, PimWorker};
use crate::{Event, Zones};

impl PimWorker for Outlook {
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
        "Outlook"
    }

    fn reach(&mut self) -> bool {
        self.connect().is_some()
    }

    fn is_online(&self) -> bool {
        self.online()
    }

    fn memo(&mut self) -> &mut Memo {
        &mut self.pim
    }

    fn explain(&self, e: &NetError) -> String {
        describe(e)
    }

    fn calendars(&mut self) -> Result<Vec<Calendar>, NetError> {
        self.call(|c| c.calendars())
    }

    fn sync_calendar(&mut self, calendar: &Calendar, _token: Option<&str>, (from, to): (i64, i64)) -> Result<CalendarSync, NetError> {
        self.events_in(calendar, from, to).map(CalendarSync::Window)
    }

    fn events_in(&mut self, calendar: &Calendar, from: i64, to: i64) -> Result<Vec<CalendarEvent>, NetError> {
        self.call(|c| c.events(&calendar.id, from, to))
    }

    fn create_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError> {
        let zones = Arc::clone(&self.zones);
        self.call(|c| c.create_event(&event.calendar, event, &*zones))
    }

    fn update_event(&mut self, event: &CalendarEvent) -> Result<CalendarEvent, NetError> {
        let zones = Arc::clone(&self.zones);
        self.call(|c| c.update_event(event, &*zones))
    }

    fn delete_event(&mut self, event: &CalendarEvent, _notify: bool) -> Result<(), NetError> {
        self.call(|c| c.delete_event(&event.remote.id))
    }

    fn respond(&mut self, event: &CalendarEvent, answer: Response, send: bool, comment: &str, proposal: Option<(i64, i64)>) -> Result<(), NetError> {
        // Graph takes another time only with a tentative answer or a refusal.
        let answer = if proposal.is_some() && answer == Response::Accepted { Response::Tentative } else { answer };
        self.call(|c| c.respond(&event.remote.id, answer, send, comment, proposal))
    }

    fn send_mail(&mut self, message: &Outgoing) -> Result<(), NetError> {
        let (raw, _) = mailalm_mime::build_for_api(message).map_err(NetError::Invalid)?;
        self.call(|c| c.send_mime(&raw))
    }

    fn task_lists(&mut self) -> Result<Vec<TaskList>, NetError> {
        self.call(|c| c.task_lists())
    }

    fn tasks(&mut self, list: &str) -> Result<Vec<Task>, NetError> {
        self.call(|c| c.tasks(list))
    }

    fn create_task(&mut self, task: &Task) -> Result<Task, NetError> {
        self.call(|c| c.create_task(&task.list, task))
    }

    fn update_task(&mut self, task: &Task) -> Result<Task, NetError> {
        self.call(|c| c.update_task(task))
    }

    fn delete_task(&mut self, task: &Task) -> Result<(), NetError> {
        self.call(|c| c.delete_task(&task.list, &task.remote.id))
    }

    fn contacts(&mut self) -> Result<Vec<Contact>, NetError> {
        self.call(|c| c.contacts())
    }
}
