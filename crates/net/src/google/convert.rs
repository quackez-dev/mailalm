//! Google's JSON (Calendar, Tasks, People) as MailALM's calendars, events, tasks and contacts,
//! and back. Everything read came from the server: ids are checked, texts cleaned and capped (an
//! HTML description made text), addresses and links checked, numbers held to sane ranges.

use mailalm_model::pim::{
    Attendee, AttendeeKind, BusyStatus, Calendar, Contact, Event, EventStatus, MAX_ATTENDEES, MAX_LINE, MAX_REMINDER_MINUTES, MAX_REMINDERS, MAX_TEXT,
    MeetingProvider, OnlineMeeting, Remote, Response, Task, TaskList, Until, ZoneRules, clean_address, clean_line, clean_text, date_text, date_time_text,
    find_meeting_link, iana_of_windows, is_utc_name, nearest_midnight, parse_date, parse_date_time, parse_hex_color, utc_time,
};
use serde_json::{Value, json};

use crate::{NetError, Result};

/// Longest id taken (calendar ids are addresses; event, task and list ids are shorter).
const MAX_ID: usize = 1024;
/// The most emails and phone numbers kept on one contact.
const MAX_CONTACT_ITEMS: usize = 20;
/// The most reminders Google takes on one event.
const MAX_GOOGLE_REMINDERS: usize = 5;

/// Whether `id` can be a Google id: printable ASCII (it goes percent-encoded into a URL path),
/// not empty, of a sane length.
pub(super) fn id_ok(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_graphic())
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn truth(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

/// A version string (`etag`): printable ASCII, at most 512 characters, else none.
fn version(v: &Value) -> String {
    let e = s(v, "etag");
    if e.len() <= 512 && e.bytes().all(|b| b.is_ascii_graphic()) { e.to_string() } else { String::new() }
}

/// A calendar list entry; `None` without a usable id.
pub(super) fn calendar_of(v: &Value) -> Option<Calendar> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let name = [s(v, "summaryOverride"), s(v, "summary")].iter().map(|n| clean_line(n, 255)).find(|n| !n.is_empty()).unwrap_or_else(|| clean_line(id, 255));
    let solutions = v.get("conferenceProperties").and_then(|c| c.get("allowedConferenceSolutionTypes")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let meet = solutions.iter().take(10).filter_map(Value::as_str).any(|t| t == "hangoutsMeet");
    let default_reminders: Vec<u32> = v
        .get("defaultReminders")
        .and_then(Value::as_array)
        .map(|a| a.iter().take(MAX_REMINDERS).filter_map(|r| r.get("minutes").and_then(Value::as_u64)).filter_map(|m| u32::try_from(m).ok()).filter(|m| *m <= MAX_REMINDER_MINUTES).collect())
        .unwrap_or_default();
    Some(Calendar {
        id: id.to_string(),
        remote: Remote::new(id, &version(v)),
        name,
        color: parse_hex_color(s(v, "backgroundColor")),
        can_edit: matches!(s(v, "accessRole"), "owner" | "writer"),
        primary: truth(v, "primary"),
        meetings: if meet { vec![MeetingProvider::Meet] } else { Vec::new() },
        default_reminders,
    })
}

/// `{"date": "2026-11-25"}` or `{"dateTime": "2026-11-25T09:00:00-05:00", "timeZone": "America/Toronto"}`:
/// (UTC seconds, a date, the zone named).
fn when(v: &Value) -> Option<(i64, bool, String)> {
    if let Some(d) = v.get("date").and_then(Value::as_str) {
        return Some((parse_date(d)?.saturating_mul(86_400), true, String::new()));
    }
    let t = parse_date_time(v.get("dateTime")?.as_str()?)?;
    Some((t, false, clean_line(s(v, "timeZone"), 100)))
}

/// Whether a description is HTML (Google Calendar keeps the formatting typed in its pages so).
fn looks_html(text: &str) -> bool {
    let head: String = text.chars().take(MAX_TEXT).collect::<String>().to_ascii_lowercase();
    ["<br", "<p>", "<p ", "<b>", "<i>", "<u>", "<a ", "<div", "<span", "<ul", "<ol", "<li", "</"].iter().any(|t| head.contains(t))
}

/// A description as plain text.
fn description_of(text: &str) -> String {
    if looks_html(text) { clean_text(&mailalm_mime::text_of_html(text), MAX_TEXT) } else { clean_text(text, MAX_TEXT) }
}

fn response_of(r: &str) -> Response {
    match r {
        "accepted" => Response::Accepted,
        "tentative" => Response::Tentative,
        "declined" => Response::Declined,
        _ => Response::NeedsAction,
    }
}

/// A guest or the organizer (`email`, `displayName`); `None` without an address.
fn person(v: &Value) -> Option<Attendee> {
    let email = clean_address(s(v, "email"))?;
    let kind = if truth(v, "resource") {
        AttendeeKind::Resource
    } else if truth(v, "optional") {
        AttendeeKind::Optional
    } else {
        AttendeeKind::Required
    };
    Some(Attendee { email, name: clean_line(s(v, "displayName"), 300), response: response_of(s(v, "responseStatus")), kind })
}

/// The event's online meeting: its conference's video entry, its Meet link, a Meet still being
/// made, else a meeting link in its place or notes.
fn online_of(v: &Value, location: &str, description: &str) -> Option<OnlineMeeting> {
    let conference = v.get("conferenceData");
    let video = conference
        .and_then(|c| c.get("entryPoints"))
        .and_then(Value::as_array)
        .and_then(|a| a.iter().take(20).find(|p| s(p, "entryPointType") == "video"))
        .and_then(|p| OnlineMeeting::at(s(p, "uri")));
    let pending = conference
        .and_then(|c| c.get("createRequest"))
        .and_then(|r| r.get("status"))
        .is_some_and(|st| s(st, "statusCode") == "pending");
    video
        .or_else(|| OnlineMeeting::at(s(v, "hangoutLink")))
        .or_else(|| pending.then(|| OnlineMeeting::request(MeetingProvider::Meet)))
        .or_else(|| find_meeting_link(location))
        .or_else(|| find_meeting_link(description))
}

/// An event from Google Calendar in calendar `calendar`; `None` without a usable id or start.
/// A cancelled one may come with its id alone (a deleted event, in a sync), or with its series and
/// first start alone (a cancelled occurrence): it starts and ends there (at 0 without one).
pub(super) fn event_of(v: &Value, calendar: &str) -> Option<Event> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let status = match s(v, "status") {
        "cancelled" => EventStatus::Cancelled,
        "tentative" => EventStatus::Tentative,
        _ => EventStatus::Confirmed,
    };
    let original = v.get("originalStartTime").and_then(when);
    let (start, all_day, zone) = match v.get("start").and_then(when) {
        Some(x) => x,
        None if status == EventStatus::Cancelled => original.clone().unwrap_or((0, false, String::new())),
        None => return None,
    };
    let one_day = if all_day { 86_400 } else { 0 };
    let end = v.get("end").and_then(when).map_or(start, |(t, _, _)| t).max(start.saturating_add(one_day));
    let location = clean_line(s(v, "location"), MAX_LINE);
    let description = description_of(s(v, "description"));
    let organizer_value = v.get("organizer");
    let is_organizer = organizer_value.is_none_or(|o| truth(o, "self"));
    let attendees_values = v.get("attendees").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut my_response = None;
    let mut attendees = Vec::new();
    for a in attendees_values.iter().take(MAX_ATTENDEES) {
        let Some(p) = person(a) else { continue };
        if truth(a, "self") {
            my_response = Some(p.response);
        }
        attendees.push(p);
    }
    let reminders = v.get("reminders");
    let uid = clean_line(s(v, "iCalUID"), 1000);
    let recurrence = v
        .get("recurrence")
        .and_then(Value::as_array)
        .and_then(|lines| lines.iter().take(50).filter_map(Value::as_str).find(|l| l.get(..6).is_some_and(|p| p.eq_ignore_ascii_case("RRULE:"))))
        .and_then(mailalm_model::pim::Recurrence::parse);
    Some(Event {
        uid: if uid.is_empty() { id.to_string() } else { uid },
        calendar: calendar.to_string(),
        remote: Remote::new(id, &version(v)),
        title: clean_line(s(v, "summary"), MAX_LINE),
        start,
        end,
        all_day,
        time_zone: if all_day { String::new() } else { zone },
        online: online_of(v, &location, &description),
        location,
        description,
        recurrence,
        exdates: Vec::new(),
        series: v.get("recurringEventId").and_then(Value::as_str).filter(|i| id_ok(i)).map(str::to_string),
        recurrence_id: original.map(|(t, _, _)| t),
        organizer: organizer_value.and_then(person),
        attendees,
        is_organizer,
        my_response: if is_organizer { None } else { my_response },
        busy: if s(v, "eventType") == "outOfOffice" {
            BusyStatus::OutOfOffice
        } else if s(v, "transparency") == "transparent" {
            BusyStatus::Free
        } else {
            BusyStatus::Busy
        },
        reminders: reminders
            .and_then(|r| r.get("overrides"))
            .and_then(Value::as_array)
            .map(|a| a.iter().take(MAX_REMINDERS).filter_map(|r| r.get("minutes").and_then(Value::as_u64)).filter_map(|m| u32::try_from(m).ok()).filter(|m| *m <= MAX_REMINDER_MINUTES).collect())
            .unwrap_or_default(),
        default_reminders: reminders.is_some_and(|r| truth(r, "useDefault")),
        status,
        sequence: v.get("sequence").and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
    })
}

/// The zone an event's times are given in for Google: an IANA name (a Windows one as its IANA
/// one); UTC for a series without one (Google wants a zone with a rule); `None` otherwise.
fn google_zone(e: &Event, zones: &dyn ZoneRules) -> Option<String> {
    if e.all_day {
        return None;
    }
    let tz = e.time_zone.trim();
    let name = iana_of_windows(tz).unwrap_or(tz);
    let known = !name.is_empty() && !is_utc_name(name) && zones.offset(name, e.start).is_some();
    if known {
        Some(name.to_string())
    } else if e.recurrence.is_some() || is_utc_name(name) {
        Some("UTC".into())
    } else {
        None
    }
}

/// An event as Google's JSON, to make or change it. `with_recurrence`: the series' rule goes too
/// (not for one occurrence). `request_id`: the id of a Meet link asked for. A Teams link can't be
/// made by Google: refused.
pub(super) fn event_json(e: &Event, zones: &dyn ZoneRules, with_recurrence: bool, request_id: &str) -> Result<Value> {
    let zone = google_zone(e, zones);
    let time = |t: i64| {
        if e.all_day {
            json!({"date": date_text(t.div_euclid(86_400))})
        } else {
            match &zone {
                Some(z) => json!({"dateTime": format!("{}Z", date_time_text(t)), "timeZone": z}),
                None => json!({"dateTime": format!("{}Z", date_time_text(t))}),
            }
        }
    };
    let end = if e.all_day { e.end.max(e.start.saturating_add(86_400)) } else { e.end.max(e.start) };
    let attendees: Vec<Value> = e
        .attendees
        .iter()
        .take(MAX_ATTENDEES)
        .filter(|a| e.organizer.as_ref().is_none_or(|o| !o.is(&a.email)))
        .filter_map(|a| {
            let address = clean_address(&a.email)?;
            let mut g = json!({"email": address});
            if !a.name.trim().is_empty() {
                g["displayName"] = json!(a.name);
            }
            match a.kind {
                AttendeeKind::Optional => g["optional"] = json!(true),
                AttendeeKind::Resource => g["resource"] = json!(true),
                AttendeeKind::Required => {}
            }
            Some(g)
        })
        .collect();
    let reminders = if e.default_reminders {
        json!({"useDefault": true})
    } else {
        let overrides: Vec<Value> = e.reminders.iter().take(MAX_GOOGLE_REMINDERS).map(|m| json!({"method": "popup", "minutes": m})).collect();
        json!({"useDefault": false, "overrides": overrides})
    };
    let mut location = e.location.clone();
    let mut description = e.description.clone();
    let mut v = json!({
        "summary": e.title,
        "start": time(e.start),
        "end": time(end),
        "transparency": if e.busy == BusyStatus::Free { "transparent" } else { "opaque" },
        "attendees": attendees,
        "reminders": reminders,
    });
    match &e.online {
        Some(OnlineMeeting { provider: MeetingProvider::Meet, url }) if url.is_empty() => {
            v["conferenceData"] = json!({"createRequest": {"requestId": request_id, "conferenceSolutionKey": {"type": "hangoutsMeet"}}});
        }
        Some(OnlineMeeting { provider: MeetingProvider::Teams, url }) if url.is_empty() => {
            return Err(NetError::Invalid("a Google account can't make a Teams meeting".into()));
        }
        // A link of the user's own: the place, or with the notes.
        Some(OnlineMeeting { provider: MeetingProvider::Link, url }) if !url.is_empty() => {
            if location.trim().is_empty() {
                location = url.clone();
            } else if !location.contains(url.as_str()) && !description.contains(url.as_str()) {
                let sep = if description.trim().is_empty() { "" } else { "\n\n" };
                description = format!("{}{sep}Join: {url}", description.trim_end());
            }
        }
        _ => {}
    }
    v["location"] = json!(location);
    v["description"] = json!(description);
    if with_recurrence && let Some(r) = &e.recurrence {
        // `UNTIL` as Google takes it: a date with dates, else an instant.
        let mut r = r.clone();
        let tz = zone.clone().unwrap_or_default();
        r.until = r.until.map(|u| match (u, e.all_day) {
            (Until::At(t) | Until::Local(t), true) => Until::Date(t.div_euclid(86_400)),
            (u @ Until::Date(_), true) | (u @ Until::At(_), false) => u,
            (Until::Local(t), false) => Until::At(utc_time(zones, &tz, t)),
            (Until::Date(d), false) => Until::At(utc_time(zones, &tz, d.saturating_mul(86_400).saturating_add(86_399))),
        });
        v["recurrence"] = json!([format!("RRULE:{}", r.to_rrule())]);
    }
    Ok(v)
}

/// A task list; `default` for the first one Google lists (its "My Tasks").
pub(super) fn task_list_of(v: &Value, default: bool) -> Option<TaskList> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let name = clean_line(s(v, "title"), 255);
    Some(TaskList { id: id.to_string(), remote: Remote::new(id, &version(v)), name: if name.is_empty() { "Tasks".into() } else { name }, default })
}

/// A task in list `list`; `None` without a usable id, or deleted.
pub(super) fn task_of(v: &Value, list: &str) -> Option<Task> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    if truth(v, "deleted") {
        return None;
    }
    let completed = s(v, "status") == "completed";
    Some(Task {
        list: list.to_string(),
        remote: Remote::new(id, &version(v)),
        title: clean_line(s(v, "title"), MAX_LINE),
        notes: clean_text(s(v, "notes"), MAX_TEXT),
        // A day (Google keeps the date alone, at midnight UTC).
        due: parse_date_time(s(v, "due")).map(nearest_midnight),
        reminder: None,
        completed,
        completed_at: if completed { parse_date_time(s(v, "completed")) } else { None },
    })
}

/// A task as Google's JSON, to make or change it. The reminder doesn't go: the Tasks API has
/// none.
pub(super) fn task_json(t: &Task) -> Value {
    let mut v = json!({
        "title": t.title,
        "notes": t.notes,
        "status": if t.completed { "completed" } else { "needsAction" },
        "due": t.due.map(|d| format!("{}T00:00:00.000Z", date_text(d.div_euclid(86_400)))),
    });
    if !t.completed {
        v["completed"] = Value::Null;
    }
    v
}

/// A connection from the People API; `None` without a resource name of its shape, or with
/// neither a name nor an address.
pub(super) fn contact_of(v: &Value) -> Option<Contact> {
    let resource = s(v, "resourceName");
    let ok = resource
        .strip_prefix("people/")
        .is_some_and(|r| (1..=200).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
    if !ok {
        return None;
    }
    let names = v.get("names").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let primary = |x: &&Value| x.get("metadata").is_some_and(|m| truth(m, "primary"));
    let name = names.iter().take(20).find(primary).or_else(|| names.first());
    let mut emails: Vec<String> = Vec::new();
    for e in v.get("emailAddresses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(MAX_CONTACT_ITEMS) {
        if let Some(a) = clean_address(s(e, "value"))
            && !emails.iter().any(|x| x.eq_ignore_ascii_case(&a))
        {
            emails.push(a);
        }
    }
    let mut phones: Vec<String> = Vec::new();
    for p in v.get("phoneNumbers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(MAX_CONTACT_ITEMS) {
        let p = clean_line(s(p, "value"), 64);
        if !p.is_empty() && !phones.contains(&p) {
            phones.push(p);
        }
    }
    let org = v.get("organizations").and_then(Value::as_array).and_then(|a| a.first());
    let c = Contact {
        remote: Remote::new(resource, &version(v)),
        name: name.map(|n| clean_line(s(n, "displayName"), 300)).unwrap_or_default(),
        given_name: name.map(|n| clean_line(s(n, "givenName"), 300)).unwrap_or_default(),
        family_name: name.map(|n| clean_line(s(n, "familyName"), 300)).unwrap_or_default(),
        emails,
        phones,
        organization: org.map(|o| clean_line(s(o, "name"), 300)).unwrap_or_default(),
        job_title: org.map(|o| clean_line(s(o, "title"), 300)).unwrap_or_default(),
    };
    (!c.display().is_empty()).then_some(c)
}
