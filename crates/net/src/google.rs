//! A Gmail account's calendars (Google Calendar API v3), tasks (Google Tasks API v1) and
//! contacts (People API v1, read only), over HTTPS with the account's OAuth bearer token
//! ([`crate::oauth`]).
//!
//! One kept connection per service (`www.googleapis.com`, `tasks.googleapis.com`,
//! `people.googleapis.com`). Listings go on by page tokens, never by links: nothing the server
//! sends is followed as an address. A busy answer (429, 503, a 403 for a rate limit) is tried
//! again after its `Retry-After`, capped, as the Gmail client does ([`crate::gmail`]); a request
//! that makes something (an event, a task) is not sent again once it may have been acted on.
//! Everything Google sends is hostile input: ids and tokens are checked, texts cleaned (an
//! event's HTML description made text) and capped, links checked, listings capped.

use std::time::Duration;

use mailalm_model::pim::{BusyBlock, Calendar, Event, EventStatus, Response, ZoneRules, busy_blocks, date_time_text};
use serde_json::{Value, json};

use crate::gmail::{Busy, backoff_secs, busy, error_detail, rate_limited};
use crate::http::{self, Agent, Request};
use crate::{NetError, Result, clip};

mod convert;
mod people;
mod tasks;

#[cfg(test)]
mod tests;

/// The Calendar API's address.
pub const CALENDAR_URL: &str = "https://www.googleapis.com/calendar/v3";
/// The Tasks API's address.
pub const TASKS_URL: &str = "https://tasks.googleapis.com/tasks/v1";
/// The People API's address.
pub const PEOPLE_URL: &str = "https://people.googleapis.com/v1";
/// Largest JSON answer accepted.
const MAX_JSON: usize = 16 << 20;
/// Pages followed for one listing.
const MAX_PAGES: usize = 2000;
/// Attempts per request while Google answers that it is busy.
const BUSY_ROUNDS: u32 = 4;
/// Most calendars and events (in one window, or one sync) taken.
const MAX_CALENDARS: usize = 500;
const MAX_EVENTS: usize = 20_000;
/// The longest window of events asked for at once.
const MAX_WINDOW: i64 = 5 * 365 * 86_400;
/// Longest page or sync token taken.
const MAX_TOKEN: usize = 4096;

/// Which of Google's services a request goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Api {
    Calendar,
    Tasks,
    People,
}

/// One service: its address, and the connection kept to it.
struct Service {
    base: String,
    agent: Agent,
}

impl Service {
    fn at(base: &str) -> Service {
        Service { base: base.trim_end_matches('/').to_string(), agent: Agent::default() }
    }
}

/// What changed in a calendar since a sync token ([`GoogleClient::sync_events`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventSync {
    /// Events new or changed: series with their rules, changed or cancelled occurrences on their
    /// own (with their `series` and `recurrence_id`).
    pub changed: Vec<Event>,
    /// Ids of events deleted.
    pub removed: Vec<String>,
    /// Keep it to ask for the next changes.
    pub token: String,
}

/// A Google session for the calendars, tasks and contacts: the access token, and one connection
/// kept per service. Its `Debug` shows no token.
pub struct GoogleClient {
    token: String,
    calendar: Service,
    tasks: Service,
    people: Service,
}

impl std::fmt::Debug for GoogleClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GoogleClient(<redacted>)")
    }
}

impl GoogleClient {
    /// A session with this access token. Nothing is sent yet.
    pub fn new(access_token: &str) -> GoogleClient {
        GoogleClient { token: access_token.to_string(), calendar: Service::at(CALENDAR_URL), tasks: Service::at(TASKS_URL), people: Service::at(PEOPLE_URL) }
    }

    /// A session with a test server for all three services (plain http to this computer is let
    /// through in tests): `/calendar/v3`, `/tasks/v1` and `/v1` on it.
    #[cfg(test)]
    pub(crate) fn with_base(base: &str, access_token: &str) -> GoogleClient {
        let base = base.trim_end_matches('/');
        GoogleClient {
            token: access_token.to_string(),
            calendar: Service::at(&format!("{base}/calendar/v3")),
            tasks: Service::at(&format!("{base}/tasks/v1")),
            people: Service::at(&format!("{base}/v1")),
        }
    }

    /// Carry a new access token from now on (the old one ran out).
    pub fn set_token(&mut self, access_token: &str) {
        self.token = access_token.to_string();
    }

    fn base(&self, api: Api) -> &str {
        match api {
            Api::Calendar => &self.calendar.base,
            Api::Tasks => &self.tasks.base,
            Api::People => &self.people.base,
        }
    }

    /// The account's calendars (its own, and those it subscribed to), at most
    /// [`MAX_CALENDARS`].
    pub fn calendars(&mut self) -> Result<Vec<Calendar>> {
        let first = format!("{}/users/me/calendarList?maxResults=250", self.base(Api::Calendar));
        let items = self.pages(Api::Calendar, &first, "items", MAX_CALENDARS)?;
        let mut out: Vec<Calendar> = Vec::new();
        for c in items.iter().filter_map(convert::calendar_of) {
            if !out.iter().any(|x| x.id == c.id) {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// The events of a calendar that overlap `[from, to)` (UTC seconds), each occurrence of a
    /// series on its own (`singleEvents=true`), in order, through every page, at most
    /// [`MAX_EVENTS`].
    pub fn events(&mut self, calendar_id: &str, from: i64, to: i64) -> Result<Vec<Event>> {
        let id = checked(calendar_id, "a calendar")?;
        if to <= from || to.saturating_sub(from) > MAX_WINDOW {
            return Err(NetError::Invalid("not a window of events Google Calendar gives".into()));
        }
        let first = format!(
            "{}/calendars/{}/events?timeMin={}Z&timeMax={}Z&singleEvents=true&orderBy=startTime&maxResults=2500",
            self.base(Api::Calendar),
            http::pct(id),
            date_time_text(from),
            date_time_text(to)
        );
        let items = self.pages(Api::Calendar, &first, "items", MAX_EVENTS)?;
        Ok(items.iter().filter_map(|v| convert::event_of(v, id)).filter(|e| e.status != EventStatus::Cancelled).collect())
    }

    /// What changed in a calendar since `token` (from the last sync); `None`: everything, from
    /// the start (series with their rules, and their changed or cancelled occurrences). A token
    /// Google no longer knows (410) is [`NetError::Resync`]: start again with `None`.
    pub fn sync_events(&mut self, calendar_id: &str, token: Option<&str>) -> Result<EventSync> {
        let id = checked(calendar_id, "a calendar")?;
        let mut url = format!("{}/calendars/{}/events?maxResults=2500", self.base(Api::Calendar), http::pct(id));
        if let Some(t) = token {
            if !token_ok(t) {
                return Err(NetError::Invalid("not a sync token".into()));
            }
            url.push_str(&format!("&syncToken={}", http::pct(t)));
        }
        let mut out = EventSync::default();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut this = url.clone();
            if let Some(p) = &page {
                this.push_str(&format!("&pageToken={}", http::pct(p)));
            }
            let resp = self.call(Api::Calendar, Request::get(&this, MAX_JSON))?;
            if resp.status == 410 {
                let detail = error_detail(serde_json::from_slice::<Value>(&resp.body).ok().as_ref());
                return Err(self.hide(NetError::Resync(format!("HTTP 410 {detail}").trim().to_string())));
            }
            self.checked(&resp)?;
            let v = json_of(&resp)?;
            for item in list(&v, "items")? {
                let Some(e) = convert::event_of(item, id) else { continue };
                // A deleted event; a cancelled occurrence stays, it takes its place in the series.
                if e.status == EventStatus::Cancelled && e.series.is_none() {
                    out.removed.push(e.remote.id);
                } else {
                    out.changed.push(e);
                }
                if out.changed.len() + out.removed.len() > MAX_EVENTS {
                    return Err(NetError::TooLarge(format!("a calendar with more than {MAX_EVENTS} events")));
                }
            }
            match (next_token(&v, "nextPageToken")?, next_token(&v, "nextSyncToken")?) {
                (Some(p), _) => page = Some(p),
                (None, Some(t)) => {
                    out.token = t;
                    return Ok(out);
                }
                (None, None) => return Err(NetError::Protocol("Google Calendar ended a sync without its token".into())),
            }
        }
        Err(NetError::TooLarge("a sync of too many pages".into()))
    }

    /// One event; `Ok(None)` when it is gone (404, 410).
    pub fn event(&mut self, calendar_id: &str, event_id: &str) -> Result<Option<Event>> {
        let cal = checked(calendar_id, "a calendar")?;
        let id = checked(event_id, "an event")?;
        let url = format!("{}/calendars/{}/events/{}", self.base(Api::Calendar), http::pct(cal), http::pct(id));
        let resp = self.call(Api::Calendar, Request::get(&url, MAX_JSON))?;
        if matches!(resp.status, 404 | 410) {
            return Ok(None);
        }
        self.checked(&resp)?;
        convert::event_of(&json_of(&resp)?, cal).map(Some).ok_or_else(|| NetError::Protocol("Google Calendar sent an event that isn't one".into()))
    }

    /// Make an event in a calendar; Google invites its guests (`sendUpdates=all`). A Meet link is
    /// asked for with `online: Meet` (no link): Google makes it (`conferenceData.createRequest`),
    /// perhaps a little later (the event comes back with the request pending). Not sent again
    /// once it may have been acted on (an event made twice is two events).
    pub fn create_event(&mut self, calendar_id: &str, event: &Event, zones: &dyn ZoneRules) -> Result<Event> {
        let cal = checked(calendar_id, "a calendar")?.to_string();
        let body = convert::event_json(event, zones, true, &request_id()?)?;
        let url = format!("{}/calendars/{}/events?conferenceDataVersion=1&sendUpdates=all", self.base(Api::Calendar), http::pct(&cal));
        let resp = self.exchange(Api::Calendar, json_request("POST", &url, &body)?, true, false).map_err(|f| self.hide(f.error))?;
        self.checked(&resp)?;
        convert::event_of(&json_of(&resp)?, &cal).ok_or_else(|| NetError::Protocol("Google Calendar made the event but sent it back unreadable".into()))
    }

    /// Change an event (`event.calendar`, `event.remote.id`); its guests hear of it. A series'
    /// rule goes too, unless it is one occurrence.
    pub fn update_event(&mut self, event: &Event, zones: &dyn ZoneRules) -> Result<Event> {
        let cal = checked(&event.calendar, "a calendar")?.to_string();
        let id = checked(&event.remote.id, "an event")?.to_string();
        let whole_series = event.series.is_none() && event.recurrence_id.is_none();
        let body = convert::event_json(event, zones, whole_series, &request_id()?)?;
        let url = format!("{}/calendars/{}/events/{}?conferenceDataVersion=1&sendUpdates=all", self.base(Api::Calendar), http::pct(&cal), http::pct(&id));
        let resp = self.call(Api::Calendar, json_request("PATCH", &url, &body)?)?;
        self.checked(&resp)?;
        convert::event_of(&json_of(&resp)?, &cal).ok_or_else(|| NetError::Protocol("Google Calendar changed the event but sent it back unreadable".into()))
    }

    /// Delete an event (or one occurrence); with `notify`, its guests hear of it. One already
    /// gone counts as done.
    pub fn delete_event(&mut self, calendar_id: &str, event_id: &str, notify: bool) -> Result<()> {
        let cal = checked(calendar_id, "a calendar")?;
        let id = checked(event_id, "an event")?;
        let updates = if notify { "all" } else { "none" };
        let url = format!("{}/calendars/{}/events/{}?sendUpdates={updates}", self.base(Api::Calendar), http::pct(cal), http::pct(id));
        let resp = self.call(Api::Calendar, Request::new("DELETE", &url, MAX_JSON))?;
        if matches!(resp.status, 404 | 410) {
            return Ok(());
        }
        self.checked(&resp)
    }

    /// Answer an invitation: the user's own guest entry gets `answer` (and `comment`), patched
    /// alone (`attendeesOmitted`); with `send`, the organizer hears of it. Google's API has no
    /// way to propose another time: a `proposal` is refused before anything is sent.
    pub fn respond(&mut self, calendar_id: &str, event_id: &str, answer: Response, send: bool, comment: &str, proposal: Option<(i64, i64)>) -> Result<()> {
        let cal = checked(calendar_id, "a calendar")?.to_string();
        let id = checked(event_id, "an event")?.to_string();
        if proposal.is_some() {
            return Err(NetError::Invalid("Google Calendar's API can't propose another time".into()));
        }
        let status = match answer {
            Response::Accepted => "accepted",
            Response::Tentative => "tentative",
            Response::Declined => "declined",
            Response::NeedsAction => return Err(NetError::Invalid("no answer to send".into())),
        };
        // The user's own entry: Google lists it alone when asked for one guest.
        let base = format!("{}/calendars/{}/events/{}", self.base(Api::Calendar), http::pct(&cal), http::pct(&id));
        let v = self.get_json(Api::Calendar, &format!("{base}?maxAttendees=1&fields=attendees"))?;
        let me = v
            .get("attendees")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().take(10).find(|x| x.get("self").and_then(Value::as_bool) == Some(true)))
            .and_then(|x| x.get("email").and_then(Value::as_str))
            .and_then(mailalm_model::pim::clean_address)
            .ok_or_else(|| NetError::Server("Google Calendar doesn't list the user as a guest of this event".into()))?;
        let mut entry = json!({"email": me, "responseStatus": status});
        let comment = mailalm_model::pim::clean_text(comment, 2000);
        if !comment.is_empty() {
            entry["comment"] = json!(comment);
        }
        let updates = if send { "all" } else { "none" };
        let body = json!({"attendeesOmitted": true, "attendees": [entry]});
        let resp = self.call(Api::Calendar, json_request("PATCH", &format!("{base}?sendUpdates={updates}"), &body)?)?;
        self.checked(&resp)
    }

    /// The user's busy time in `[from, to)` from the calendars they own or can change: every
    /// event that takes time, merged ([`busy_blocks`]).
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

    /// Every item of a listing (its `key` array), page by page (`pageToken`), at most `limit`.
    fn pages(&mut self, api: Api, first: &str, key: &str, limit: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = first.to_string();
            if let Some(p) = &page {
                url.push_str(&format!("&pageToken={}", http::pct(p)));
            }
            let v = self.get_json(api, &url)?;
            out.extend(list(&v, key)?.iter().take(limit.saturating_sub(out.len())).cloned());
            if out.len() >= limit {
                break;
            }
            match next_token(&v, "nextPageToken")? {
                Some(p) => page = Some(p),
                None => break,
            }
        }
        Ok(out)
    }

    fn get_json(&mut self, api: Api, url: &str) -> Result<Value> {
        let resp = self.call(api, Request::get(url, MAX_JSON))?;
        self.checked(&resp)?;
        json_of(&resp)
    }

    /// A request that reads, or changes things the same way twice: a busy answer of any kind
    /// sends it again.
    fn call(&mut self, api: Api, req: Request) -> Result<http::Response> {
        self.exchange(api, req, true, true).map_err(|f| self.hide(f.error))
    }

    /// `req` with the token and `Accept`, sent again while Google answers that it is busy (after
    /// `Retry-After`, up to [`BUSY_ROUNDS`] attempts): always when it turned the request away,
    /// and when it failed on the way only with `failed_again`. `again`: whether a kept connection
    /// found closed may carry it once more on a new one.
    fn exchange(&mut self, api: Api, req: Request, again: bool, failed_again: bool) -> std::result::Result<http::Response, http::Failure> {
        let req = req.header("Authorization", &format!("Bearer {}", self.token)).header("Accept", "application/json");
        let agent = match api {
            Api::Calendar => &mut self.calendar.agent,
            Api::Tasks => &mut self.tasks.agent,
            Api::People => &mut self.people.agent,
        };
        let mut round = 0;
        loop {
            round += 1;
            let resp = agent.send(&req, again)?;
            let retry = match busy(&resp) {
                Some(Busy::TurnedAway) => true,
                Some(Busy::Failed) => failed_again,
                None => false,
            };
            if !retry || round >= BUSY_ROUNDS {
                return Ok(resp);
            }
            log::debug!("Google is busy (HTTP {}): waiting", resp.status);
            std::thread::sleep(Duration::from_secs(backoff_secs(resp.header("Retry-After"), round)));
        }
    }

    /// The answer's status as MailALM's error, the token blanked out of it.
    fn checked(&self, resp: &http::Response) -> Result<()> {
        check(resp).map_err(|e| self.hide(e))
    }

    /// The error with the access token blanked out of its text.
    fn hide(&self, e: NetError) -> NetError {
        e.without(&[self.token.as_str()])
    }
}

/// An id checked before it goes into a request (percent-encoded): printable ASCII, not empty, of
/// a sane length.
fn checked<'a>(id: &'a str, what: &str) -> Result<&'a str> {
    if convert::id_ok(id) { Ok(id) } else { Err(NetError::Invalid(format!("not {what} id: {}", clip(id)))) }
}

/// Whether `t` can be a page or sync token: printable ASCII, not empty, of a sane length (it goes
/// back, percent-encoded, into the next request's query).
fn token_ok(t: &str) -> bool {
    (1..=MAX_TOKEN).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_graphic())
}

/// A listing's `key` array; none at all is an empty listing.
fn list<'a>(v: &'a Value, key: &str) -> Result<&'a [Value]> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(a) => a.as_array().map(Vec::as_slice).ok_or_else(|| NetError::Protocol("a Google listing without its list".into())),
    }
}

/// A page's next token (`key`), checked.
fn next_token(v: &Value, key: &str) -> Result<Option<String>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) if token_ok(t) => Ok(Some(t.clone())),
        Some(_) => Err(NetError::Protocol("Google sent a page or sync token that isn't one".into())),
    }
}

/// A request with a JSON body.
fn json_request(method: &'static str, url: &str, body: &Value) -> Result<Request> {
    let bytes = serde_json::to_vec(body).map_err(|e| NetError::Invalid(e.to_string()))?;
    Ok(Request { body: bytes, ..Request::new(method, url, MAX_JSON) }.header("Content-Type", "application/json"))
}

/// A new conference request's id: 16 random bytes in hex (Google makes one meeting per id).
fn request_id() -> Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| NetError::Io("no random numbers from the system".into()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn json_of(resp: &http::Response) -> Result<Value> {
    if resp.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&resp.body).map_err(|_| NetError::Protocol(format!("Google sent JSON that can't be read (HTTP {})", resp.status)))
}

/// An answer's status as MailALM's error, from its JSON error when it has one.
fn check(resp: &http::Response) -> Result<()> {
    if resp.ok() {
        return Ok(());
    }
    status_error(resp.status, serde_json::from_slice::<Value>(&resp.body).ok().as_ref())
}

/// Whether a Google error says the token lacks the permission asked for (the user unticked it on
/// the consent screen): signing in again fixes it.
fn missing_scope(body: Option<&Value>) -> bool {
    let error = body.and_then(|v| v.get("error"));
    let reasons = error.and_then(|e| e.get("errors")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let message = error.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or_default();
    reasons.iter().take(20).filter_map(|e| e.get("reason").and_then(Value::as_str)).any(|r| r.eq_ignore_ascii_case("insufficientPermissions"))
        || message.contains("insufficient authentication scopes")
}

/// A status (and its JSON error) as MailALM's error: 401 is the token refused, a 403 for a
/// permission not granted asks to sign in again; busy and failing are tried again later;
/// anything else is a refusal.
fn status_error(status: u16, body: Option<&Value>) -> Result<()> {
    let detail = error_detail(body);
    let with = |what: String| if detail.is_empty() { what } else { format!("{what}: {detail}") };
    match status {
        200..=299 => Ok(()),
        401 => Err(NetError::Auth(with("Google refused the access token (it ran out or was revoked)".into()))),
        403 if missing_scope(body) => Err(NetError::Auth(with("Google didn't give MailALM this permission; sign in again and tick every box".into()))),
        429 | 500..=599 => Err(NetError::Protocol(with(format!("Google is busy (HTTP {status}); try again later")))),
        403 if rate_limited(body) => Err(NetError::Protocol(with("Google is busy (HTTP 403, too many requests); try again later".into()))),
        s => Err(NetError::Server(with(format!("HTTP {s}")))),
    }
}
