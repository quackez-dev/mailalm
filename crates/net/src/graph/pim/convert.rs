// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/exchange/graph_pim.rs (`rrule_of`, `recurrence_of`: Graph's recurrence patterns as RRULEs and back). MIT OR Apache-2.0.
// Changed: read into and written from MailALM's `Recurrence`; the range's start is the series' first day where it is; an end that isn't a date is no end; the rest (calendars, events, tasks, contacts) is MailALM's own, every field from Graph cleaned and capped.

//! Graph's JSON as MailALM's calendars, events, To Do tasks and contacts, and back. Everything
//! read came from the server: ids are checked, texts cleaned and capped, addresses and links
//! checked, numbers held to sane ranges.

use mailalm_model::pim::{
    Attendee, AttendeeKind, BusyStatus, Calendar, Contact, Event, EventStatus, MAX_ATTENDEES, MAX_LINE, MAX_REMINDER_MINUTES, MAX_TEXT, MeetingProvider,
    OnlineMeeting, Recurrence, Remote, Response, Task, TaskList, ZoneRules, clean_address, clean_line, clean_text, date_text, date_time_text,
    find_meeting_link, iana_of_windows, is_utc_name, local_time, nearest_midnight, parse_date_time, parse_hex_color, windows_of_iana,
};
use mailalm_model::query::civil_from_days;
use serde_json::{Value, json};

use super::super::convert::id_ok;
use crate::{NetError, Result};

/// The most emails and phone numbers kept on one contact.
const MAX_CONTACT_ITEMS: usize = 20;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// A version string (`changeKey`): printable ASCII, at most 512 characters, else none.
fn version(v: &Value) -> String {
    let k = s(v, "changeKey");
    if k.len() <= 512 && k.bytes().all(|b| b.is_ascii_graphic()) { k.to_string() } else { String::new() }
}

/// `{"dateTime": "2026-11-25T14:00:00.0000000", "timeZone": "UTC"}` in Unix seconds. MailALM asks
/// for UTC; a time in another zone is read as UTC too (Graph doesn't send one when asked so).
fn when(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    let t = parse_date_time(v.get("dateTime")?.as_str()?)?;
    let zone = s(v, "timeZone");
    if !zone.is_empty() && !is_utc_name(zone) {
        log::debug!("Microsoft Graph sent a time in another zone than UTC");
    }
    Some(t)
}

/// A boolean as Graph sends it (`true`, or `"true"`).
fn truth(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A calendar from `me/calendars`; `None` without a usable id.
pub(super) fn calendar_of(v: &Value) -> Option<Calendar> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let name = clean_line(s(v, "name"), 255);
    let allowed = v.get("allowedOnlineMeetingProviders").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let teams = allowed.iter().take(20).filter_map(Value::as_str).any(|p| p.eq_ignore_ascii_case("teamsForBusiness"));
    Some(Calendar {
        id: id.to_string(),
        remote: Remote::new(id, &version(v)),
        name: if name.is_empty() { "Calendar".into() } else { name },
        color: parse_hex_color(s(v, "hexColor")),
        can_edit: truth(v.get("canEdit")),
        primary: truth(v.get("isDefaultCalendar")),
        meetings: if teams { vec![MeetingProvider::Teams] } else { Vec::new() },
        default_reminders: Vec::new(),
    })
}

/// A guest's (or the user's own) answer as Graph names it.
fn response_of(r: &str) -> Response {
    match r {
        "accepted" | "organizer" => Response::Accepted,
        "tentativelyAccepted" => Response::Tentative,
        "declined" => Response::Declined,
        _ => Response::NeedsAction,
    }
}

/// `{"emailAddress": {"name", "address"}}` as a person; `None` without an address.
fn person(v: &Value) -> Option<Attendee> {
    let e = v.get("emailAddress")?;
    let email = clean_address(s(e, "address"))?;
    Some(Attendee { email, name: clean_line(s(e, "name"), 300), ..Attendee::default() })
}

const DAYS: [(&str, &str); 7] =
    [("sunday", "SU"), ("monday", "MO"), ("tuesday", "TU"), ("wednesday", "WE"), ("thursday", "TH"), ("friday", "FR"), ("saturday", "SA")];
const INDEX: [(&str, i32); 5] = [("first", 1), ("second", 2), ("third", 3), ("fourth", 4), ("last", -1)];

fn day_code(name: &str) -> Option<&'static str> {
    DAYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| *c)
}

fn day_name(code: &str) -> Option<&'static str> {
    DAYS.iter().find(|(_, c)| c.eq_ignore_ascii_case(code)).map(|(n, _)| *n)
}

/// A Graph recurrence (pattern and range) as an RRULE; `None` for one it can't say.
pub(super) fn rrule_of(r: &Value) -> Option<String> {
    let p = r.get("pattern")?;
    let interval = p.get("interval").and_then(Value::as_u64).unwrap_or(1).clamp(1, 10_000);
    let days: Vec<&str> =
        p.get("daysOfWeek").and_then(Value::as_array).map(|a| a.iter().take(7).filter_map(Value::as_str).filter_map(day_code).collect()).unwrap_or_default();
    let index = INDEX.iter().find(|(n, _)| *n == s(p, "index")).map_or(1, |(_, i)| *i);
    let month = p.get("month").and_then(Value::as_u64).filter(|m| (1..=12).contains(m));
    let mday = p.get("dayOfMonth").and_then(Value::as_u64).filter(|d| (1..=31).contains(d));
    // The nth of one weekday is that weekday's nth; of several ("weekday", "weekend day",
    // "day"), the nth day that is one of them, not the nth of each.
    let by_index = match days.as_slice() {
        [_] => format!("{index}{}", days.join(",")),
        _ => format!("{};BYSETPOS={index}", days.join(",")),
    };
    // A day past the 28th is the month's last where it has no such day, as Exchange has it.
    let by_mday = |d: u64| if (29..=31).contains(&d) { format!("{};BYSETPOS=-1", (28..=d).map(|x| x.to_string()).collect::<Vec<_>>().join(",")) } else { d.to_string() };
    let week_start = day_code(s(p, "firstDayOfWeek")).filter(|d| *d != "MO" && interval > 1).map(|d| format!(";WKST={d}")).unwrap_or_default();
    let mut rule = match s(p, "type") {
        "daily" => "FREQ=DAILY".to_string(),
        "weekly" if !days.is_empty() => format!("FREQ=WEEKLY;BYDAY={}{week_start}", days.join(",")),
        "weekly" => "FREQ=WEEKLY".to_string(),
        "absoluteMonthly" => format!("FREQ=MONTHLY;BYMONTHDAY={}", by_mday(mday?)),
        "relativeMonthly" if !days.is_empty() => format!("FREQ=MONTHLY;BYDAY={by_index}"),
        "absoluteYearly" => format!("FREQ=YEARLY;BYMONTH={};BYMONTHDAY={}", month?, mday?),
        "relativeYearly" if !days.is_empty() => format!("FREQ=YEARLY;BYMONTH={};BYDAY={by_index}", month?),
        _ => return None,
    };
    if interval > 1 {
        rule.push_str(&format!(";INTERVAL={interval}"));
    }
    let range = r.get("range").cloned().unwrap_or(Value::Null);
    match s(&range, "type") {
        "endDate" => {
            let end = s(&range, "endDate").replace('-', "");
            if end.len() == 8 && end.bytes().all(|c| c.is_ascii_digit()) {
                rule.push_str(&format!(";UNTIL={end}"));
            }
        }
        "numbered" => {
            if let Some(n) = range.get("numberOfOccurrences").and_then(Value::as_u64).filter(|n| *n > 0) {
                rule.push_str(&format!(";COUNT={}", n.min(20_000)));
            }
        }
        _ => {}
    }
    Some(rule)
}

/// A rule as a Graph recurrence, its range starting on `start_day` (the series' first day where
/// its times are given); `None` for a rule Graph can't say.
pub(super) fn recurrence_of(r: &Recurrence, start_day: i64) -> Option<Value> {
    let rule = r.to_rrule();
    let parts: Vec<(String, String)> =
        rule.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase())).collect();
    let get = |k: &str| parts.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
    let interval: u64 = get("INTERVAL").and_then(|v| v.parse().ok()).unwrap_or(1);
    let byday: Vec<(i32, &str)> = get("BYDAY")
        .map(|v| {
            v.split(',')
                .filter_map(|d| {
                    let code = d.get(d.len().saturating_sub(2)..)?;
                    let n = d.get(..d.len().saturating_sub(2)).and_then(|n| if n.is_empty() { Some(0) } else { n.parse().ok() })?;
                    Some((n, day_name(code)?))
                })
                .collect()
        })
        .unwrap_or_default();
    let (y, m, d) = civil_from_days(start_day);
    // The nth of several weekdays is said with `BYSETPOS` (its days without a number); the 29th
    // to 31st, or the month's last, as those days and `BYSETPOS=-1`: its largest.
    let set_pos = get("BYSETPOS").and_then(|v| v.parse::<i32>().ok());
    let byday: Vec<(i32, &str)> = match set_pos {
        Some(nth) if byday.iter().all(|(n, _)| *n == 0) => byday.into_iter().map(|(_, d)| (nth, d)).collect(),
        _ => byday,
    };
    let month_day = get("BYMONTHDAY").and_then(|v| v.split(',').filter_map(|d| d.trim().parse::<u32>().ok()).max());
    let index = |n: i32| INDEX.iter().find(|(_, i)| *i == n).map_or("first", |(name, _)| *name);
    let days: Vec<&str> = byday.iter().map(|(_, d)| *d).collect();
    let pattern = match get("FREQ")? {
        "DAILY" => json!({"type": "daily", "interval": interval}),
        "WEEKLY" => {
            let start_weekday = DAYS.get(usize::try_from((start_day + 4).rem_euclid(7)).unwrap_or(0)).map_or("monday", |(n, _)| *n);
            let first = get("WKST").and_then(day_name).unwrap_or("monday");
            json!({"type": "weekly", "interval": interval, "daysOfWeek": if days.is_empty() { vec![start_weekday] } else { days }, "firstDayOfWeek": first})
        }
        "MONTHLY" => match byday.first() {
            Some((n, _)) => json!({"type": "relativeMonthly", "interval": interval, "daysOfWeek": days, "index": index(*n)}),
            None => json!({"type": "absoluteMonthly", "interval": interval, "dayOfMonth": month_day.unwrap_or(u32::try_from(d).unwrap_or(1))}),
        },
        "YEARLY" => {
            let month = get("BYMONTH").and_then(|v| v.parse::<u32>().ok()).unwrap_or(u32::try_from(m).unwrap_or(1));
            match byday.first() {
                Some((n, _)) => json!({"type": "relativeYearly", "interval": interval, "month": month, "daysOfWeek": days, "index": index(*n)}),
                None => json!({"type": "absoluteYearly", "interval": interval, "month": month, "dayOfMonth": month_day.unwrap_or(u32::try_from(d).unwrap_or(1))}),
            }
        }
        _ => return None,
    };
    let start_date = format!("{y:04}-{m:02}-{d:02}");
    // Its eight digits (`YYYYMMDD`), read without slicing a character.
    let until = get("UNTIL").and_then(|u| u.get(..8)).filter(|u| u.bytes().all(|b| b.is_ascii_digit()));
    let range = if let Some((uy, um, ud)) = until.and_then(|u| Some((u.get(..4)?, u.get(4..6)?, u.get(6..8)?))) {
        json!({"type": "endDate", "startDate": start_date, "endDate": format!("{uy}-{um}-{ud}")})
    } else if let Some(n) = get("COUNT").and_then(|v| v.parse::<u64>().ok()) {
        json!({"type": "numbered", "startDate": start_date, "numberOfOccurrences": n})
    } else {
        json!({"type": "noEnd", "startDate": start_date})
    };
    Some(json!({"pattern": pattern, "range": range}))
}

/// An event from Graph (a single one, an occurrence or exception of a series, or a series'
/// master), in calendar `calendar`; `None` without a usable id or start.
pub(super) fn event_of(v: &Value, calendar: &str) -> Option<Event> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let all_day = truth(v.get("isAllDay"));
    let mut start = when(v.get("start"))?;
    let mut end = when(v.get("end")).unwrap_or(start).max(start);
    if all_day {
        // Dates: a zone's midnight in UTC is that day's.
        start = nearest_midnight(start);
        end = nearest_midnight(end).max(start.saturating_add(86_400));
    }
    let uid = clean_line(s(v, "iCalUId"), 1000);
    let zone = clean_line(s(v, "originalStartTimeZone"), 100);
    let kind = s(v, "type");
    let occurrence = matches!(kind, "occurrence" | "exception");
    let is_organizer = truth(v.get("isOrganizer"));
    let attendees: Vec<Attendee> = v
        .get("attendees")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_ATTENDEES)
                .filter_map(|x| {
                    let mut p = person(x)?;
                    p.response = response_of(x.get("status").map(|st| s(st, "response")).unwrap_or_default());
                    p.kind = match s(x, "type") {
                        "optional" => AttendeeKind::Optional,
                        "resource" => AttendeeKind::Resource,
                        _ => AttendeeKind::Required,
                    };
                    Some(p)
                })
                .collect()
        })
        .unwrap_or_default();
    let location = clean_line(v.get("location").map(|l| s(l, "displayName")).unwrap_or_default(), MAX_LINE);
    let description = clean_text(v.get("body").map(|b| s(b, "content")).unwrap_or_default(), MAX_TEXT);
    let join = v.get("onlineMeeting").map(|m| s(m, "joinUrl")).and_then(OnlineMeeting::at);
    let online = match join {
        Some(mut m) => {
            if s(v, "onlineMeetingProvider").eq_ignore_ascii_case("teamsForBusiness") {
                m.provider = MeetingProvider::Teams;
            }
            Some(m)
        }
        None => find_meeting_link(&location).or_else(|| find_meeting_link(&description)),
    };
    let reminders = if truth(v.get("isReminderOn")) {
        v.get("reminderMinutesBeforeStart").and_then(Value::as_u64).and_then(|m| u32::try_from(m).ok()).filter(|m| *m <= MAX_REMINDER_MINUTES).into_iter().collect()
    } else {
        Vec::new()
    };
    let recurrence_id = if occurrence {
        v.get("originalStart").and_then(Value::as_str).and_then(parse_date_time).map(|t| if all_day { nearest_midnight(t) } else { t })
    } else {
        None
    };
    Some(Event {
        uid: if uid.is_empty() { id.to_string() } else { uid },
        calendar: calendar.to_string(),
        remote: Remote::new(id, &version(v)),
        title: clean_line(s(v, "subject"), MAX_LINE),
        start,
        end,
        all_day,
        time_zone: if all_day { String::new() } else { iana_of_windows(&zone).map_or(zone, str::to_string) },
        location,
        description,
        recurrence: if kind == "seriesMaster" { v.get("recurrence").and_then(rrule_of).and_then(|r| Recurrence::parse(&r)) } else { None },
        exdates: Vec::new(),
        series: if occurrence { v.get("seriesMasterId").and_then(Value::as_str).filter(|i| id_ok(i)).map(str::to_string) } else { None },
        recurrence_id,
        organizer: v.get("organizer").and_then(person),
        attendees,
        is_organizer,
        my_response: if is_organizer { None } else { v.get("responseStatus").map(|r| response_of(s(r, "response"))) },
        busy: match s(v, "showAs") {
            "free" => BusyStatus::Free,
            "tentative" => BusyStatus::Tentative,
            "oof" => BusyStatus::OutOfOffice,
            _ => BusyStatus::Busy,
        },
        reminders,
        default_reminders: false,
        online,
        status: if truth(v.get("isCancelled")) { EventStatus::Cancelled } else { EventStatus::Confirmed },
        sequence: 0,
    })
}

/// The zone an event's times are written in for Graph: its Windows name, and the IANA zone the
/// caller's rules read it with, when they know it; `None`: UTC.
fn graph_zone<'a>(e: &'a Event, zones: &dyn ZoneRules) -> Option<(&'static str, &'a str)> {
    let tz = e.time_zone.trim();
    if e.all_day || tz.is_empty() || is_utc_name(tz) {
        return None;
    }
    let (windows, iana) = match iana_of_windows(tz) {
        Some(iana) => (windows_of_iana(iana)?, iana),
        None => (windows_of_iana(tz)?, tz),
    };
    zones.offset(iana, e.start).is_some().then_some((windows, iana))
}

/// An event as Graph's JSON, to make or change it. `with_recurrence`: the series' rule goes too
/// (not for one occurrence). A Meet link can't be made by Graph: refused.
pub(super) fn event_json(e: &Event, zones: &dyn ZoneRules, with_recurrence: bool) -> Result<Value> {
    let zone = graph_zone(e, zones);
    let local = |t: i64| zone.map_or(t, |(_, iana)| local_time(zones, iana, t));
    let tz = zone.map_or("UTC", |(w, _)| w);
    let time = |t: i64| {
        if e.all_day {
            json!({"dateTime": format!("{}T00:00:00", date_text(t.div_euclid(86_400))), "timeZone": "UTC"})
        } else {
            json!({"dateTime": date_time_text(local(t)), "timeZone": tz})
        }
    };
    let end = if e.all_day { e.end.max(e.start.saturating_add(86_400)) } else { e.end.max(e.start) };
    let mut location = e.location.clone();
    let mut body = e.description.clone();
    let attendees: Vec<Value> = e
        .attendees
        .iter()
        .take(MAX_ATTENDEES)
        .filter(|a| e.organizer.as_ref().is_none_or(|o| !o.is(&a.email)))
        .filter_map(|a| {
            let address = clean_address(&a.email)?;
            let kind = match a.kind {
                AttendeeKind::Required => "required",
                AttendeeKind::Optional => "optional",
                AttendeeKind::Resource => "resource",
            };
            Some(json!({"type": kind, "emailAddress": {"address": address, "name": a.name}}))
        })
        .collect();
    let show_as = match e.busy {
        BusyStatus::Free => "free",
        BusyStatus::Tentative => "tentative",
        BusyStatus::Busy => "busy",
        BusyStatus::OutOfOffice => "oof",
    };
    let mut v = json!({
        "subject": e.title,
        "isAllDay": e.all_day,
        "start": time(e.start),
        "end": time(end),
        "showAs": show_as,
        "isReminderOn": !e.reminders.is_empty(),
        "attendees": attendees,
    });
    if let Some(m) = e.reminders.first() {
        v["reminderMinutesBeforeStart"] = json!(m);
    }
    match &e.online {
        Some(OnlineMeeting { provider: MeetingProvider::Teams, url }) if url.is_empty() => {
            v["isOnlineMeeting"] = json!(true);
            v["onlineMeetingProvider"] = json!("teamsForBusiness");
        }
        Some(OnlineMeeting { provider: MeetingProvider::Meet, url }) if url.is_empty() => {
            return Err(NetError::Invalid("a Microsoft account can't make a Google Meet meeting".into()));
        }
        // A link of the user's own: the place, or with the notes.
        Some(OnlineMeeting { provider: MeetingProvider::Link, url }) if !url.is_empty() => {
            if location.trim().is_empty() {
                location = url.clone();
            } else if !location.contains(url.as_str()) && !body.contains(url.as_str()) {
                let sep = if body.trim().is_empty() { "" } else { "\n\n" };
                body = format!("{}{sep}Join: {url}", body.trim_end());
            }
        }
        _ => {}
    }
    v["location"] = json!({"displayName": location});
    v["body"] = json!({"contentType": "text", "content": body});
    if with_recurrence {
        let start_day = if e.all_day { e.start.div_euclid(86_400) } else { local(e.start).div_euclid(86_400) };
        v["recurrence"] = e.recurrence.as_ref().and_then(|r| recurrence_of(r, start_day)).unwrap_or(Value::Null);
    }
    Ok(v)
}

/// A To Do list; `None` without a usable id.
pub(super) fn task_list_of(v: &Value) -> Option<TaskList> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let name = clean_line(s(v, "displayName"), 255);
    Some(TaskList {
        id: id.to_string(),
        remote: Remote::new(id, ""),
        name: if name.is_empty() { "Tasks".into() } else { name },
        default: s(v, "wellknownListName").eq_ignore_ascii_case("defaultList"),
    })
}

/// A To Do task in list `list`; `None` without a usable id.
pub(super) fn task_of(v: &Value, list: &str) -> Option<Task> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let completed = s(v, "status").eq_ignore_ascii_case("completed");
    let etag = s(v, "@odata.etag");
    Some(Task {
        list: list.to_string(),
        remote: Remote::new(id, if etag.len() <= 512 && etag.bytes().all(|b| b.is_ascii_graphic()) { etag } else { "" }),
        title: clean_line(s(v, "title"), MAX_LINE),
        notes: clean_text(v.get("body").map(|b| s(b, "content")).unwrap_or_default(), MAX_TEXT),
        // A day: the midnight nearest to the time To Do gives it.
        due: when(v.get("dueDateTime")).map(nearest_midnight),
        reminder: if truth(v.get("isReminderOn")) { when(v.get("reminderDateTime")) } else { None },
        completed,
        completed_at: if completed { when(v.get("completedDateTime")) } else { None },
    })
}

/// A task as To Do's JSON, to make or change it.
pub(super) fn task_json(t: &Task) -> Value {
    let utc = |x: i64| json!({"dateTime": date_time_text(x), "timeZone": "UTC"});
    json!({
        "title": t.title,
        "body": {"contentType": "text", "content": t.notes},
        "status": if t.completed { "completed" } else { "notStarted" },
        "dueDateTime": t.due.map(|d| utc(d.div_euclid(86_400).saturating_mul(86_400))),
        "isReminderOn": t.reminder.is_some(),
        "reminderDateTime": t.reminder.map(utc),
    })
}

/// A contact; `None` without a usable id, or with neither a name nor an address.
pub(super) fn contact_of(v: &Value) -> Option<Contact> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let mut emails: Vec<String> = Vec::new();
    for a in v.get("emailAddresses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().take(MAX_CONTACT_ITEMS) {
        if let Some(e) = clean_address(s(a, "address"))
            && !emails.iter().any(|x| x.eq_ignore_ascii_case(&e))
        {
            emails.push(e);
        }
    }
    let mut phones: Vec<String> = Vec::new();
    let lists = ["businessPhones", "homePhones"].map(|k| v.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default());
    for p in lists.iter().flat_map(|l| l.iter()).filter_map(Value::as_str).chain(std::iter::once(s(v, "mobilePhone"))) {
        let p = clean_line(p, 64);
        if !p.is_empty() && phones.len() < MAX_CONTACT_ITEMS && !phones.contains(&p) {
            phones.push(p);
        }
    }
    let c = Contact {
        remote: Remote::new(id, &version(v)),
        name: clean_line(s(v, "displayName"), 300),
        given_name: clean_line(s(v, "givenName"), 300),
        family_name: clean_line(s(v, "surname"), 300),
        emails,
        phones,
        organization: clean_line(s(v, "companyName"), 300),
        job_title: clean_line(s(v, "jobTitle"), 300),
    };
    (!c.display().is_empty()).then_some(c)
}
