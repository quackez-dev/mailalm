//! Outlook.com's calendars, Microsoft To Do's tasks and the contacts through Microsoft Graph
//! (v1.0), on the same session as the mail ([`GraphClient`]).
//!
//! Times come in UTC (`Prefer: outlook.timezone="UTC"`), with each event's own zone kept from
//! `originalStartTimeZone` (a Windows name, kept as its IANA one when the table has it). Texts
//! come as plain text (`Prefer: outlook.body-content-type="text"`). Events are read by
//! `calendarView` (a window, each occurrence of a series on its own); an event is written with
//! its times in its zone when the zone is known, else in UTC.
//!
//! Everything Graph sends is hostile input: ids are checked before they go into a request or the
//! cache, texts are cleaned and capped, links followed only back to Graph, listings capped.

use mailalm_model::pim::{BusyBlock, Calendar, Contact, Event, MeetingProvider, Response, Task, TaskList, ZoneRules, busy_blocks, date_time_text};
use serde_json::{Value, json};

use super::{GraphClient, MAX_JSON, check, convert::id_ok, seg};
use crate::http::Request;
use crate::{NetError, Result, clip};

mod convert;

#[cfg(test)]
mod tests;

/// Sent on every call here: immutable ids, times in UTC, texts as plain text.
const PREFER_PIM: &str = "IdType=\"ImmutableId\", outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"";
/// Most calendars, task lists, events (in one window), tasks (in one list) and contacts taken.
const MAX_CALENDARS: usize = 500;
const MAX_LISTS: usize = 500;
const MAX_EVENTS: usize = 20_000;
const MAX_TASKS: usize = 20_000;
const MAX_CONTACTS: usize = 50_000;
/// The longest window of events asked for at once (Graph's `calendarView` takes about five
/// years at most).
const MAX_WINDOW: i64 = 5 * 365 * 86_400;
const CALENDAR_FIELDS: &str = "id,name,hexColor,canEdit,isDefaultCalendar,changeKey,allowedOnlineMeetingProviders";
const EVENT_FIELDS: &str = "id,iCalUId,changeKey,subject,body,start,end,isAllDay,originalStartTimeZone,location,attendees,organizer,isOrganizer,\
responseStatus,recurrence,type,seriesMasterId,originalStart,isCancelled,showAs,isReminderOn,reminderMinutesBeforeStart,isOnlineMeeting,\
onlineMeetingProvider,onlineMeeting";
const CONTACT_FIELDS: &str = "id,changeKey,displayName,givenName,surname,emailAddresses,businessPhones,homePhones,mobilePhone,companyName,jobTitle";

/// An id checked before it goes into a request.
fn checked_id<'a>(id: &'a str, what: &str) -> Result<&'a str> {
    if id_ok(id) { Ok(id) } else { Err(NetError::Invalid(format!("not {what} id: {}", clip(id)))) }
}

/// A request with a JSON body.
fn json_request(method: &'static str, url: &str, body: &Value) -> Result<Request> {
    let bytes = serde_json::to_vec(body).map_err(|e| NetError::Invalid(e.to_string()))?;
    Ok(Request { body: bytes, ..Request::new(method, url, MAX_JSON) }.header("Content-Type", "application/json"))
}

impl GraphClient {
    /// The account's calendars: its own and those shared with it, at most [`MAX_CALENDARS`].
    pub fn calendars(&mut self) -> Result<Vec<Calendar>> {
        let url = format!("{}/me/calendars?$top=100&$select={CALENDAR_FIELDS}", self.base);
        let items = self.all_with(&url, MAX_CALENDARS, PREFER_PIM)?;
        let mut out: Vec<Calendar> = Vec::new();
        for c in items.iter().filter_map(convert::calendar_of) {
            if !out.iter().any(|x| x.id == c.id) {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// The events of a calendar that overlap `[from, to)` (UTC seconds): single events, and each
    /// occurrence of a series on its own (`calendarView`), through every page, at most
    /// [`MAX_EVENTS`].
    pub fn events(&mut self, calendar_id: &str, from: i64, to: i64) -> Result<Vec<Event>> {
        let id = checked_id(calendar_id, "a calendar")?;
        if to <= from || to.saturating_sub(from) > MAX_WINDOW {
            return Err(NetError::Invalid("not a window of events Microsoft Graph gives".into()));
        }
        let url = format!(
            "{}/me/calendars/{}/calendarView?startDateTime={}Z&endDateTime={}Z&$top=100&$select={EVENT_FIELDS}",
            self.base,
            seg(id),
            date_time_text(from),
            date_time_text(to)
        );
        let items = self.all_with(&url, MAX_EVENTS, PREFER_PIM)?;
        Ok(items.iter().filter_map(|v| convert::event_of(v, id)).collect())
    }

    /// One event (a series' master gives its recurrence); `Ok(None)` when it is gone (404).
    pub fn event(&mut self, calendar_id: &str, event_id: &str) -> Result<Option<Event>> {
        let cal = checked_id(calendar_id, "a calendar")?;
        let id = checked_id(event_id, "an event")?;
        let url = format!("{}/me/events/{}?$select={EVENT_FIELDS}", self.base, seg(id));
        let resp = self.call(Request::get(&url, MAX_JSON), "application/json", PREFER_PIM)?;
        if resp.status == 404 {
            return Ok(None);
        }
        check(&resp).map_err(|e| self.hide(e))?;
        let v = super::json_of(&resp)?;
        convert::event_of(&v, cal).map(Some).ok_or_else(|| NetError::Protocol("Microsoft Graph sent an event that isn't one".into()))
    }

    /// Make an event in a calendar; Graph invites its guests. A Teams meeting is asked for with
    /// `online: Teams` (no link): when the calendar doesn't offer Teams meetings (personal
    /// accounts may not), nothing is made and the error says so. Not sent again once it went out
    /// (an event made twice is two events), unless Graph turned it away unread.
    pub fn create_event(&mut self, calendar_id: &str, event: &Event, zones: &dyn ZoneRules) -> Result<Event> {
        let cal = checked_id(calendar_id, "a calendar")?.to_string();
        let teams = event.online.as_ref().is_some_and(|o| o.provider == MeetingProvider::Teams && o.url.is_empty());
        if teams {
            self.check_teams(&cal)?;
        }
        let body = convert::event_json(event, zones, true)?;
        let url = format!("{}/me/calendars/{}/events", self.base, seg(&cal));
        let resp = self.exchange(json_request("POST", &url, &body)?, "application/json", PREFER_PIM, &[429, 503], true).map_err(|f| self.hide(f.error))?;
        if teams && (400..500).contains(&resp.status) && resp.status != 401 {
            let e = check(&resp).err().map(|e| self.hide(e).to_string()).unwrap_or_default();
            return Err(NetError::Server(format!("Microsoft refused the event with a Teams meeting (this account may not offer Teams meetings): {e}")));
        }
        check(&resp).map_err(|e| self.hide(e))?;
        let made = convert::event_of(&super::json_of(&resp)?, &cal).ok_or_else(|| NetError::Protocol("Microsoft Graph made the event but sent it back unreadable".into()))?;
        if teams && made.online.as_ref().is_none_or(|o| o.url.is_empty()) {
            log::info!("Microsoft Graph made the event without a Teams link");
        }
        Ok(made)
    }

    /// Whether this calendar makes Teams meetings (its `allowedOnlineMeetingProviders`).
    fn check_teams(&mut self, calendar_id: &str) -> Result<()> {
        let url = format!("{}/me/calendars/{}?$select=allowedOnlineMeetingProviders", self.base, seg(calendar_id));
        let v = self.get_json_with(&url, PREFER_PIM)?;
        let allowed: Vec<String> = v
            .get("allowedOnlineMeetingProviders")
            .and_then(Value::as_array)
            .map(|a| a.iter().take(20).filter_map(Value::as_str).map(|s| clip(s)).collect())
            .unwrap_or_default();
        if allowed.iter().any(|p| p.eq_ignore_ascii_case("teamsForBusiness")) {
            return Ok(());
        }
        let offered = if allowed.is_empty() { "none".to_string() } else { allowed.join(", ") };
        Err(NetError::Invalid(format!("this Microsoft account's calendar can't make Teams meetings (it offers: {offered})")))
    }

    /// Change an event (by its id, `event.remote.id`): what it says, its times, its guests (Graph
    /// tells them); a series' rule too, unless it is one occurrence. Its new version comes back.
    pub fn update_event(&mut self, event: &Event, zones: &dyn ZoneRules) -> Result<Event> {
        let id = checked_id(&event.remote.id, "an event")?.to_string();
        let whole_series = event.series.is_none() && event.recurrence_id.is_none();
        if event.online.as_ref().is_some_and(|o| o.provider == MeetingProvider::Teams && o.url.is_empty()) && !event.calendar.is_empty() {
            let cal = checked_id(&event.calendar, "a calendar")?.to_string();
            self.check_teams(&cal)?;
        }
        let body = convert::event_json(event, zones, whole_series)?;
        let url = format!("{}/me/events/{}", self.base, seg(&id));
        let resp = self.call(json_request("PATCH", &url, &body)?, "application/json", PREFER_PIM)?;
        check(&resp).map_err(|e| self.hide(e))?;
        convert::event_of(&super::json_of(&resp)?, &event.calendar).ok_or_else(|| NetError::Protocol("Microsoft Graph changed the event but sent it back unreadable".into()))
    }

    /// Delete an event (or one occurrence of a series); for a meeting the user organizes, Graph
    /// sends its guests the cancellation. One already gone counts as done.
    pub fn delete_event(&mut self, event_id: &str) -> Result<()> {
        let id = checked_id(event_id, "an event")?;
        let url = format!("{}/me/events/{}", self.base, seg(id));
        let resp = self.call(Request::new("DELETE", &url, MAX_JSON), "application/json", PREFER_PIM)?;
        if resp.status == 404 {
            return Ok(());
        }
        check(&resp).map_err(|e| self.hide(e))
    }

    /// Answer an invitation: accept, accept tentatively, or decline; with `send`, the organizer
    /// hears of it, with `comment`. `proposal` (a new start and end) proposes another time, with
    /// a tentative answer or a refusal only (Graph's rule), and is always sent.
    pub fn respond(&mut self, event_id: &str, answer: Response, send: bool, comment: &str, proposal: Option<(i64, i64)>) -> Result<()> {
        let id = checked_id(event_id, "an event")?;
        let action = match answer {
            Response::Accepted => "accept",
            Response::Tentative => "tentativelyAccept",
            Response::Declined => "decline",
            Response::NeedsAction => return Err(NetError::Invalid("no answer to send".into())),
        };
        let mut body = json!({"sendResponse": send || proposal.is_some(), "comment": mailalm_model::pim::clean_text(comment, 2000)});
        if let Some((start, end)) = proposal {
            if answer == Response::Accepted {
                return Err(NetError::Invalid("Microsoft Graph takes another time only with a tentative answer or a refusal".into()));
            }
            if end <= start {
                return Err(NetError::Invalid("the time proposed ends before it starts".into()));
            }
            body["proposedNewTime"] = json!({
                "start": {"dateTime": date_time_text(start), "timeZone": "UTC"},
                "end": {"dateTime": date_time_text(end), "timeZone": "UTC"},
            });
        }
        let url = format!("{}/me/events/{}/{action}", self.base, seg(id));
        let resp = self.call(json_request("POST", &url, &body)?, "application/json", PREFER_PIM)?;
        check(&resp).map_err(|e| self.hide(e))
    }

    /// The user's busy time in `[from, to)` from their own calendars (those they can change):
    /// every event that takes time, merged ([`busy_blocks`]).
    pub fn free_busy(&mut self, from: i64, to: i64, zones: &dyn ZoneRules) -> Result<Vec<BusyBlock>> {
        let mut events = Vec::new();
        for c in self.calendars()?.into_iter().filter(|c| c.can_edit) {
            events.extend(self.events(&c.id, from, to)?);
            if events.len() >= MAX_EVENTS {
                break;
            }
        }
        Ok(busy_blocks(&events, from, to, zones))
    }

    /// Microsoft To Do's lists, at most [`MAX_LISTS`].
    pub fn task_lists(&mut self) -> Result<Vec<TaskList>> {
        let items = self.all_with(&format!("{}/me/todo/lists?$top=100", self.base), MAX_LISTS, PREFER_PIM)?;
        Ok(items.iter().filter_map(convert::task_list_of).collect())
    }

    /// The tasks of a list, done ones too, at most [`MAX_TASKS`].
    pub fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        let list = checked_id(list_id, "a task list")?;
        let items = self.all_with(&format!("{}/me/todo/lists/{}/tasks?$top=100", self.base, seg(list)), MAX_TASKS, PREFER_PIM)?;
        Ok(items.iter().filter_map(|v| convert::task_of(v, list)).collect())
    }

    /// Make a task in a list. Not sent again once it went out, unless Graph turned it away unread.
    pub fn create_task(&mut self, list_id: &str, task: &Task) -> Result<Task> {
        let list = checked_id(list_id, "a task list")?.to_string();
        let url = format!("{}/me/todo/lists/{}/tasks", self.base, seg(&list));
        let req = json_request("POST", &url, &convert::task_json(task))?;
        let resp = self.exchange(req, "application/json", PREFER_PIM, &[429, 503], true).map_err(|f| self.hide(f.error))?;
        check(&resp).map_err(|e| self.hide(e))?;
        convert::task_of(&super::json_of(&resp)?, &list).ok_or_else(|| NetError::Protocol("Microsoft To Do made the task but sent it back unreadable".into()))
    }

    /// Change a task (`task.list`, `task.remote.id`): its title, notes, due day, reminder, done.
    pub fn update_task(&mut self, task: &Task) -> Result<Task> {
        let list = checked_id(&task.list, "a task list")?.to_string();
        let id = checked_id(&task.remote.id, "a task")?.to_string();
        self.patch_task(&list, &id, &convert::task_json(task))
    }

    /// Mark a task done, or not done.
    pub fn complete_task(&mut self, list_id: &str, task_id: &str, done: bool) -> Result<Task> {
        let list = checked_id(list_id, "a task list")?.to_string();
        let id = checked_id(task_id, "a task")?.to_string();
        self.patch_task(&list, &id, &json!({"status": if done { "completed" } else { "notStarted" }}))
    }

    fn patch_task(&mut self, list: &str, id: &str, body: &Value) -> Result<Task> {
        let url = format!("{}/me/todo/lists/{}/tasks/{}", self.base, seg(list), seg(id));
        let resp = self.call(json_request("PATCH", &url, body)?, "application/json", PREFER_PIM)?;
        check(&resp).map_err(|e| self.hide(e))?;
        convert::task_of(&super::json_of(&resp)?, list).ok_or_else(|| NetError::Protocol("Microsoft To Do changed the task but sent it back unreadable".into()))
    }

    /// Delete a task. One already gone counts as done.
    pub fn delete_task(&mut self, list_id: &str, task_id: &str) -> Result<()> {
        let list = checked_id(list_id, "a task list")?;
        let id = checked_id(task_id, "a task")?;
        let url = format!("{}/me/todo/lists/{}/tasks/{}", self.base, seg(list), seg(id));
        let resp = self.call(Request::new("DELETE", &url, MAX_JSON), "application/json", PREFER_PIM)?;
        if resp.status == 404 {
            return Ok(());
        }
        check(&resp).map_err(|e| self.hide(e))
    }

    /// The contacts (the default folder), at most [`MAX_CONTACTS`]; one without a name or an
    /// address is left out.
    pub fn contacts(&mut self) -> Result<Vec<Contact>> {
        let url = format!("{}/me/contacts?$top=100&$select={CONTACT_FIELDS}", self.base);
        let items = self.all_with(&url, MAX_CONTACTS, PREFER_PIM)?;
        Ok(items.iter().filter_map(convert::contact_of).collect())
    }
}
