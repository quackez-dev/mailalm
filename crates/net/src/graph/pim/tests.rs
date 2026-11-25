//! Graph's calendars, To Do and contacts against a local test server answering canned JSON.

use mailalm_model::pim::{Attendee, AttendeeKind, BusyStatus, EventStatus, Freq, OnlineMeeting, Recurrence, Remote, Until, Weekday};
use mailalm_model::query::days_from_civil;
use serde_json::json;

use super::*;
use crate::http::tests::serve;

/// An answer the client won't keep its connection for (each canned answer is a connection).
fn answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn ok(body: &Value) -> Vec<u8> {
    answer("200 OK", &body.to_string())
}

/// A test server's address as Graph's (`…/v1.0`), and a client for it.
fn client(base: &str) -> (String, GraphClient) {
    let graph = format!("{base}/v1.0");
    let c = GraphClient::with_base(&graph, "tok-secret-xyz");
    (graph, c)
}

/// [`serve`], with answers that can name the server's own address (a nextLink).
fn serve_with_links(make: impl FnOnce(&str) -> Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let answers = make(&base);
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for a in answers {
            let s = crate::http::tests::accept_within(&l, crate::http::tests::TEST_SERVER_WAIT).expect("a request");
            let mut reader = std::io::BufReader::new(s.try_clone().unwrap());
            seen.push(crate::http::tests::read_request(&mut reader).expect("a whole request"));
            let _ = std::io::Write::write_all(&mut &s, &a);
        }
        seen
    });
    (base, h)
}

/// The JSON body of a request the server saw.
fn body(seen: &str) -> Value {
    serde_json::from_str(seen.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn at(y: i64, m: i64, d: i64, h: i64, mi: i64) -> i64 {
    days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60
}

/// Toronto in November (UTC−5), the viewer there too; UTC.
struct November;

impl ZoneRules for November {
    fn offset(&self, zone: &str, _utc: i64) -> Option<i64> {
        match zone {
            "" | "America/Toronto" | "America/New_York" => Some(-5 * 3600),
            z if mailalm_model::pim::is_utc_name(z) => Some(0),
            _ => None,
        }
    }
}

const PREFER_SENT: &str = "Prefer: IdType=\"ImmutableId\", outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"\r\n";

fn graph_event() -> Value {
    json!({
        "id": "E1", "iCalUId": "uid-1", "changeKey": "ck1", "subject": "Mix\u{7} review",
        "body": {"contentType": "text", "content": "Bring\r\nstems\u{202e}"},
        "start": {"dateTime": "2026-11-25T14:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-11-25T15:00:00.0000000", "timeZone": "UTC"},
        "isAllDay": false, "originalStartTimeZone": "Eastern Standard Time", "location": {"displayName": "Lab"},
        "attendees": [
            {"type": "required", "status": {"response": "tentativelyAccepted"}, "emailAddress": {"name": "Me", "address": "me@outlook.example"}},
            {"type": "resource", "status": {"response": "none"}, "emailAddress": {"name": "Room", "address": "room@x.example"}},
            {"type": "optional", "emailAddress": {"address": "not an address"}},
        ],
        "organizer": {"emailAddress": {"name": "Ana", "address": "ana@contoso.example"}},
        "isOrganizer": false, "responseStatus": {"response": "tentativelyAccepted"},
        "type": "singleInstance", "showAs": "tentative", "isReminderOn": true, "reminderMinutesBeforeStart": 15,
        "isOnlineMeeting": true, "onlineMeetingProvider": "teamsForBusiness",
        "onlineMeeting": {"joinUrl": "https://teams.microsoft.com/l/meetup-join/19%3ameeting"},
    })
}

/// Calendars through two pages; events of a window through two pages, converted; a link to
/// another host refused; one event and its series.
#[test]
fn calendars_and_events_are_read() {
    let (base, seen) = serve_with_links(|base| {
        let graph = format!("{base}/v1.0");
        vec![
            ok(&json!({"value": [
                {"id": "C1", "name": "Calendar", "hexColor": "#1A2B3C", "canEdit": true, "isDefaultCalendar": true, "changeKey": "k1",
                 "allowedOnlineMeetingProviders": ["teamsForBusiness"]},
                {"id": "bad id", "name": "x"},
                {"id": "C2", "name": "Birthdays\u{202e}", "hexColor": "", "canEdit": false},
            ], "@odata.nextLink": format!("{graph}/me/calendars?$skiptoken=2")})),
            ok(&json!({"value": [{"id": "C1", "name": "dup"}, {"id": "C3", "name": ""}]})),
            ok(&json!({"value": [
                graph_event(),
                {"id": "E2", "iCalUId": "uid-2", "subject": "Standup",
                 "start": {"dateTime": "2026-11-26T14:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-26T14:15:00.0000000", "timeZone": "UTC"},
                 "type": "occurrence", "seriesMasterId": "S1", "originalStart": "2026-11-26T14:00:00Z", "isOrganizer": true,
                 "responseStatus": {"response": "organizer"}, "onlineMeeting": {"joinUrl": "javascript:alert(1)"},
                 "location": {"displayName": "https://meet.google.com/abc-defg-hij"}},
            ], "@odata.nextLink": format!("{graph}/me/calendars/C1/calendarView?$skiptoken=x")})),
            ok(&json!({"value": [
                {"id": "E3", "subject": "Holiday", "isAllDay": true, "isCancelled": true,
                 "start": {"dateTime": "2026-12-25T00:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-12-26T00:00:00.0000000", "timeZone": "UTC"}},
                {"id": "bad id", "start": {"dateTime": "2026-12-25T00:00:00", "timeZone": "UTC"}},
                {"id": "E4"},
            ]})),
            ok(&json!({"value": [], "@odata.nextLink": "https://evil.example/v1.0/me/calendarView"})),
            ok(&json!({"id": "S1", "iCalUId": "uid-2", "type": "seriesMaster", "subject": "Standup",
                "start": {"dateTime": "2026-11-26T14:00:00.0000000", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-26T14:15:00.0000000", "timeZone": "UTC"},
                "recurrence": {"pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["thursday"], "firstDayOfWeek": "sunday"},
                               "range": {"type": "endDate", "startDate": "2026-11-26", "endDate": "2026-12-31"}}})),
            answer("404 Not Found", r#"{"error":{"code":"ErrorItemNotFound","message":"gone"}}"#),
        ]
    });
    let (_, mut c) = client(&base);
    let cals = c.calendars().unwrap();
    let got: Vec<(&str, &str, bool, bool)> = cals.iter().map(|c| (c.id.as_str(), c.name.as_str(), c.can_edit, c.primary)).collect();
    assert_eq!(got, [("C1", "Calendar", true, true), ("C2", "Birthdays", false, false), ("C3", "Calendar", false, false)]);
    assert_eq!(cals[0].color, Some(mailalm_model::Rgb(0x1a, 0x2b, 0x3c)));
    assert_eq!((cals[0].meetings.as_slice(), cals[0].remote.version.as_str()), (&[MeetingProvider::Teams][..], "k1"));
    assert!(cals[1].color.is_none() && cals[1].meetings.is_empty());

    let events = c.events("C1", at(2026, 11, 1, 0, 0), at(2026, 12, 31, 0, 0)).unwrap();
    assert_eq!(events.iter().map(|e| e.remote.id.as_str()).collect::<Vec<_>>(), ["E1", "E2", "E3"]);
    let e = &events[0];
    assert_eq!((e.uid.as_str(), e.calendar.as_str(), e.title.as_str(), e.remote.version.as_str()), ("uid-1", "C1", "Mix review", "ck1"));
    assert_eq!((e.start, e.end, e.time_zone.as_str()), (at(2026, 11, 25, 14, 0), at(2026, 11, 25, 15, 0), "America/New_York"));
    assert_eq!((e.description.as_str(), e.location.as_str()), ("Bring\nstems", "Lab"));
    let guests: Vec<(&str, Response, AttendeeKind)> = e.attendees.iter().map(|a| (a.email.as_str(), a.response, a.kind)).collect();
    assert_eq!(guests, [("me@outlook.example", Response::Tentative, AttendeeKind::Required), ("room@x.example", Response::NeedsAction, AttendeeKind::Resource)]);
    assert_eq!(e.organizer.as_ref().map(|o| o.email.as_str()), Some("ana@contoso.example"));
    assert_eq!((e.is_organizer, e.my_response, e.busy, e.reminders.as_slice()), (false, Some(Response::Tentative), BusyStatus::Tentative, &[15u32][..]));
    assert_eq!(e.online, Some(OnlineMeeting { provider: MeetingProvider::Teams, url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting".into() }));
    let o = &events[1];
    assert_eq!((o.series.as_deref(), o.recurrence_id, o.is_organizer, o.my_response), (Some("S1"), Some(at(2026, 11, 26, 14, 0)), true, None));
    assert_eq!(o.online.as_ref().map(|m| m.provider), Some(MeetingProvider::Meet), "the javascript: link refused, the place's taken");
    let h = &events[2];
    assert_eq!((h.uid.as_str(), h.all_day, h.start, h.end, h.status), ("E3", true, at(2026, 12, 25, 0, 0), at(2026, 12, 26, 0, 0), EventStatus::Cancelled));
    assert!(h.time_zone.is_empty());

    assert!(matches!(c.events("C1", at(2026, 11, 1, 0, 0), at(2026, 12, 1, 0, 0)), Err(NetError::Protocol(m)) if m.contains("lead back")));
    let master = c.event("C1", "S1").unwrap().unwrap();
    let rule = master.recurrence.unwrap();
    assert_eq!((rule.freq, rule.by_day.as_slice(), rule.until), (Freq::Weekly, &[(0, Weekday::Th)][..], Some(Until::Date(days_from_civil(2026, 12, 31)))));
    assert_eq!(c.event("C1", "S1").unwrap(), None);
    // Nothing sent for an id that isn't one, or a window that isn't one.
    assert!(matches!(c.events("a b", 0, 10), Err(NetError::Invalid(_))));
    assert!(matches!(c.events("C1", 10, 10), Err(NetError::Invalid(_))));
    assert!(matches!(c.event("C1", ""), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 7);
    assert!(seen[0].starts_with(&format!("GET /v1.0/me/calendars?$top=100&$select={CALENDAR_FIELDS} ")), "{}", seen[0]);
    assert!(seen[0].contains(PREFER_SENT) && seen[0].contains("Authorization: Bearer tok-secret-xyz\r\n"), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /v1.0/me/calendars?$skiptoken=2 "), "{}", seen[1]);
    assert!(
        seen[2].starts_with("GET /v1.0/me/calendars/C1/calendarView?startDateTime=2026-11-01T00:00:00Z&endDateTime=2026-12-31T00:00:00Z&$top=100&$select="),
        "{}",
        seen[2]
    );
    assert!(seen[3].starts_with("GET /v1.0/me/calendars/C1/calendarView?$skiptoken=x "), "{}", seen[3]);
    assert!(seen[5].starts_with("GET /v1.0/me/events/S1?$select="), "{}", seen[5]);
}

fn new_event() -> Event {
    Event {
        uid: String::new(),
        calendar: "C1".into(),
        title: "Planning".into(),
        start: at(2026, 11, 25, 14, 0),
        end: at(2026, 11, 25, 15, 0),
        time_zone: "America/Toronto".into(),
        description: "Notes".into(),
        recurrence: Recurrence::parse("FREQ=WEEKLY;BYDAY=WE;COUNT=4"),
        organizer: Some(Attendee::new("me@outlook.example", "Me")),
        attendees: vec![Attendee::new("me@outlook.example", "Me"), Attendee { kind: AttendeeKind::Optional, ..Attendee::new("zoe@studio.example", "Zoe") }],
        reminders: vec![10],
        online: Some(OnlineMeeting::request(MeetingProvider::Teams)),
        ..Event::default()
    }
}

/// A meeting with a Teams link, written in its zone, with its series; refused before anything
/// is sent when the calendar makes no Teams meetings, or for a Meet link.
#[test]
fn events_are_written() {
    let made = {
        let mut v = graph_event();
        v["id"] = json!("NEW1");
        v
    };
    let (base, seen) = serve(vec![
        ok(&json!({"allowedOnlineMeetingProviders": ["teamsForBusiness", "skypeForConsumer"]})),
        answer("201 Created", &made.to_string()),
        ok(&json!({"allowedOnlineMeetingProviders": ["skypeForConsumer"]})),
        ok(&json!({"allowedOnlineMeetingProviders": ["teamsForBusiness"]})),
        answer("400 Bad Request", r#"{"error":{"code":"ErrorInvalidRequest","message":"Online meetings are not supported for this account."}}"#),
        ok(&made),
        answer("204 No Content", ""),
        answer("404 Not Found", r#"{"error":{"code":"ErrorItemNotFound","message":"gone"}}"#),
    ]);
    let (_, mut c) = client(&base);
    let e = c.create_event("C1", &new_event(), &November).unwrap();
    assert_eq!((e.remote.id.as_str(), e.calendar.as_str()), ("NEW1", "C1"));
    let refused = c.create_event("C1", &new_event(), &November).unwrap_err();
    assert!(matches!(&refused, NetError::Invalid(m) if m.contains("can't make Teams meetings (it offers: skypeForConsumer)")), "{refused}");
    let refused = c.create_event("C1", &new_event(), &November).unwrap_err();
    assert!(matches!(&refused, NetError::Server(m) if m.contains("Teams") && m.contains("not supported")), "{refused}");
    let meet = Event { online: Some(OnlineMeeting::request(MeetingProvider::Meet)), ..new_event() };
    assert!(matches!(c.create_event("C1", &meet, &November), Err(NetError::Invalid(_))));
    // One occurrence changed: no rule goes.
    let occurrence = Event {
        remote: Remote::new("OCC1", ""),
        series: Some("S1".into()),
        recurrence_id: Some(at(2026, 12, 2, 14, 0)),
        online: None,
        ..new_event()
    };
    c.update_event(&occurrence, &November).unwrap();
    c.delete_event("E1").unwrap();
    c.delete_event("E1").unwrap();
    assert!(matches!(c.delete_event("a/b c"), Err(NetError::Invalid(_))));
    assert!(matches!(c.update_event(&new_event(), &November), Err(NetError::Invalid(_))), "no id: nothing to change");

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 8);
    assert!(seen[0].starts_with("GET /v1.0/me/calendars/C1?$select=allowedOnlineMeetingProviders "), "{}", seen[0]);
    assert!(seen[1].starts_with("POST /v1.0/me/calendars/C1/events "), "{}", seen[1]);
    let sent = body(&seen[1]);
    assert_eq!(sent["subject"], "Planning");
    assert_eq!(sent["start"], json!({"dateTime": "2026-11-25T09:00:00", "timeZone": "Eastern Standard Time"}));
    assert_eq!(sent["end"], json!({"dateTime": "2026-11-25T10:00:00", "timeZone": "Eastern Standard Time"}));
    assert_eq!((sent["isOnlineMeeting"].as_bool(), sent["onlineMeetingProvider"].as_str()), (Some(true), Some("teamsForBusiness")));
    assert_eq!(sent["attendees"], json!([{"type": "optional", "emailAddress": {"address": "zoe@studio.example", "name": "Zoe"}}]));
    assert_eq!((sent["isReminderOn"].as_bool(), sent["reminderMinutesBeforeStart"].as_u64()), (Some(true), Some(10)));
    assert_eq!(sent["body"], json!({"contentType": "text", "content": "Notes"}));
    assert_eq!(
        sent["recurrence"],
        json!({"pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["wednesday"], "firstDayOfWeek": "monday"},
               "range": {"type": "numbered", "startDate": "2026-11-25", "numberOfOccurrences": 4}})
    );
    assert!(seen[5].starts_with("PATCH /v1.0/me/events/OCC1 "), "{}", seen[5]);
    assert!(body(&seen[5]).get("recurrence").is_none() && body(&seen[5]).get("isOnlineMeeting").is_none(), "{}", seen[5]);
    assert!(seen[6].starts_with("DELETE /v1.0/me/events/E1 "), "{}", seen[6]);
}

/// Answers to invitations: accepted with a comment, tentative with another time, declined
/// without telling; what Graph doesn't take is refused before anything is sent.
#[test]
fn invitations_are_answered() {
    let (base, seen) = serve(vec![answer("202 Accepted", ""), answer("202 Accepted", ""), answer("202 Accepted", "")]);
    let (_, mut c) = client(&base);
    c.respond("E1", Response::Accepted, true, "See you\u{7} there", None).unwrap();
    c.respond("E1", Response::Tentative, false, "", Some((at(2026, 11, 26, 15, 0), at(2026, 11, 26, 16, 0)))).unwrap();
    c.respond("E1", Response::Declined, false, "", None).unwrap();
    assert!(matches!(c.respond("E1", Response::Accepted, true, "", Some((1, 2))), Err(NetError::Invalid(_))));
    assert!(matches!(c.respond("E1", Response::Declined, true, "", Some((2, 1))), Err(NetError::Invalid(_))));
    assert!(matches!(c.respond("E1", Response::NeedsAction, true, "", None), Err(NetError::Invalid(_))));
    assert!(matches!(c.respond("", Response::Accepted, true, "", None), Err(NetError::Invalid(_))));
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("POST /v1.0/me/events/E1/accept "), "{}", seen[0]);
    assert_eq!(body(&seen[0]), json!({"sendResponse": true, "comment": "See you there"}));
    assert!(seen[1].starts_with("POST /v1.0/me/events/E1/tentativelyAccept "), "{}", seen[1]);
    assert_eq!(
        body(&seen[1]),
        json!({"sendResponse": true, "comment": "", "proposedNewTime": {
            "start": {"dateTime": "2026-11-26T15:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-26T16:00:00", "timeZone": "UTC"}}})
    );
    assert!(seen[2].starts_with("POST /v1.0/me/events/E1/decline "), "{}", seen[2]);
    assert_eq!(body(&seen[2]), json!({"sendResponse": false, "comment": ""}));
}

/// Busy time from the calendars the user can change only.
#[test]
fn free_busy_from_own_calendars() {
    let ev = |id: &str, from: &str, to: &str, show: &str| {
        json!({"id": id, "start": {"dateTime": from, "timeZone": "UTC"}, "end": {"dateTime": to, "timeZone": "UTC"}, "showAs": show})
    };
    let (base, seen) = serve(vec![
        ok(&json!({"value": [{"id": "C1", "name": "Mine", "canEdit": true}, {"id": "C2", "name": "Holidays", "canEdit": false}]})),
        ok(&json!({"value": [
            ev("A", "2026-11-24T14:00:00", "2026-11-24T15:00:00", "busy"),
            ev("B", "2026-11-24T14:30:00", "2026-11-24T16:00:00", "busy"),
            ev("F", "2026-11-24T18:00:00", "2026-11-24T19:00:00", "free"),
        ]})),
    ]);
    let (_, mut c) = client(&base);
    let blocks = c.free_busy(at(2026, 11, 24, 0, 0), at(2026, 11, 25, 0, 0), &November).unwrap();
    assert_eq!(blocks, [BusyBlock { start: at(2026, 11, 24, 14, 0), end: at(2026, 11, 24, 16, 0), status: BusyStatus::Busy }]);
    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].starts_with("GET /v1.0/me/calendars/C1/calendarView?"), "{}", seen[1]);
}

/// To Do: the lists, a list's tasks through two pages, a task made, done, changed and deleted.
#[test]
fn tasks_are_read_and_written() {
    let task = json!({"id": "T9", "title": "New", "status": "notStarted"});
    let (base, seen) = serve_with_links(|base| {
        let graph = format!("{base}/v1.0");
        vec![
            ok(&json!({"value": [
                {"id": "L1", "displayName": "Tasks", "wellknownListName": "defaultList"},
                {"id": "L2", "displayName": "Groceries\r\n", "wellknownListName": "none"},
                {"displayName": "no id"},
            ]})),
            ok(&json!({"value": [
                {"id": "T1", "@odata.etag": "W/\"x\"", "title": "Pay rent", "status": "notStarted",
                 "body": {"content": "before\r\nthe 1st", "contentType": "text"},
                 "dueDateTime": {"dateTime": "2026-12-01T05:00:00.0000000", "timeZone": "UTC"},
                 "isReminderOn": true, "reminderDateTime": {"dateTime": "2026-11-30T14:00:00.0000000", "timeZone": "UTC"}},
            ], "@odata.nextLink": format!("{graph}/me/todo/lists/L1/tasks?$skip=1")})),
            ok(&json!({"value": [
                {"id": "T2", "title": "Done", "status": "completed", "completedDateTime": {"dateTime": "2026-11-20T00:00:00.0000000", "timeZone": "UTC"},
                 "isReminderOn": false, "reminderDateTime": {"dateTime": "2026-11-19T00:00:00", "timeZone": "UTC"}},
            ]})),
            answer("201 Created", &task.to_string()),
            ok(&json!({"id": "T9", "title": "New", "status": "completed"})),
            ok(&task),
            answer("404 Not Found", ""),
        ]
    });
    let (_, mut c) = client(&base);
    let lists = c.task_lists().unwrap();
    assert_eq!(lists.iter().map(|l| (l.id.as_str(), l.name.as_str(), l.default)).collect::<Vec<_>>(), [("L1", "Tasks", true), ("L2", "Groceries", false)]);
    let tasks = c.tasks("L1").unwrap();
    assert_eq!(tasks.len(), 2);
    let t = &tasks[0];
    assert_eq!((t.list.as_str(), t.remote.id.as_str(), t.remote.version.as_str(), t.title.as_str()), ("L1", "T1", "W/\"x\"", "Pay rent"));
    assert_eq!((t.notes.as_str(), t.due, t.reminder, t.completed), ("before\nthe 1st", Some(at(2026, 12, 1, 0, 0)), Some(at(2026, 11, 30, 14, 0)), false));
    assert_eq!((tasks[1].completed, tasks[1].completed_at, tasks[1].reminder), (true, Some(at(2026, 11, 20, 0, 0)), None));
    let new = Task { list: "L1".into(), title: "New".into(), due: Some(at(2026, 12, 3, 0, 0)), ..Task::default() };
    let made = c.create_task("L1", &new).unwrap();
    assert_eq!(made.remote.id, "T9");
    assert!(c.complete_task("L1", "T9", true).unwrap().completed);
    c.update_task(&Task { remote: Remote::new("T9", ""), notes: "more".into(), ..new.clone() }).unwrap();
    c.delete_task("L1", "T9").unwrap();
    assert!(matches!(c.tasks(""), Err(NetError::Invalid(_))));
    assert!(matches!(c.update_task(&new), Err(NetError::Invalid(_))), "no id");

    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("GET /v1.0/me/todo/lists?$top=100 "), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /v1.0/me/todo/lists/L1/tasks?$top=100 ") && seen[2].starts_with("GET /v1.0/me/todo/lists/L1/tasks?$skip=1 "));
    assert!(seen[3].starts_with("POST /v1.0/me/todo/lists/L1/tasks "), "{}", seen[3]);
    assert_eq!(
        body(&seen[3]),
        json!({"title": "New", "body": {"contentType": "text", "content": ""}, "status": "notStarted",
               "dueDateTime": {"dateTime": "2026-12-03T00:00:00", "timeZone": "UTC"}, "isReminderOn": false, "reminderDateTime": null})
    );
    assert!(seen[4].starts_with("PATCH /v1.0/me/todo/lists/L1/tasks/T9 "), "{}", seen[4]);
    assert_eq!(body(&seen[4]), json!({"status": "completed"}));
    assert_eq!(body(&seen[5])["body"]["content"], "more");
    assert!(seen[6].starts_with("DELETE /v1.0/me/todo/lists/L1/tasks/T9 "), "{}", seen[6]);
}

#[test]
fn contacts_are_read() {
    let (base, seen) = serve_with_links(|base| {
        vec![
            ok(&json!({"value": [
                {"id": "P1", "changeKey": "c1", "displayName": "Ana Lima", "givenName": "Ana", "surname": "Lima",
                 "emailAddresses": [{"name": "Ana", "address": "ana@contoso.example"}, {"address": "ANA@contoso.example"}, {"address": "bad"}],
                 "businessPhones": ["+1 555 0100"], "homePhones": [], "mobilePhone": "+1 555 0101", "companyName": "Sound Lab", "jobTitle": "Mixer"},
                {"id": "P2"},
            ], "@odata.nextLink": format!("{base}/v1.0/me/contacts?$skip=2")})),
            ok(&json!({"value": [{"id": "P3", "emailAddresses": [{"address": "x@y.example"}]}]})),
        ]
    });
    let (_, mut c) = client(&base);
    let people = c.contacts().unwrap();
    assert_eq!(people.len(), 2);
    let p = &people[0];
    assert_eq!((p.remote.id.as_str(), p.name.as_str(), p.emails.as_slice()), ("P1", "Ana Lima", &["ana@contoso.example".to_string()][..]));
    assert_eq!((p.phones.as_slice(), p.organization.as_str(), p.job_title.as_str()), (&["+1 555 0100".to_string(), "+1 555 0101".to_string()][..], "Sound Lab", "Mixer"));
    assert_eq!(people[1].display(), "x@y.example");
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with(&format!("GET /v1.0/me/contacts?$top=100&$select={CONTACT_FIELDS} ")), "{}", seen[0]);
}

/// Graph's recurrence patterns as rules and back (MailCraft's cases, on MailALM's rules).
#[test]
fn recurrences_both_ways() {
    let rule = |r: &Value| convert::rrule_of(r).and_then(|s| Recurrence::parse(&s)).map(|r| r.to_rrule());
    let weekly = json!({"pattern": {"type": "weekly", "interval": 2, "daysOfWeek": ["monday", "wednesday"], "firstDayOfWeek": "sunday"},
                        "range": {"type": "endDate", "startDate": "2026-10-01", "endDate": "2026-12-31"}});
    assert_eq!(rule(&weekly).as_deref(), Some("FREQ=WEEKLY;INTERVAL=2;UNTIL=20261231;BYDAY=MO,WE;WKST=SU"));
    let monthly = json!({"pattern": {"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["friday"], "index": "last"},
                         "range": {"type": "numbered", "numberOfOccurrences": 6}});
    assert_eq!(rule(&monthly).as_deref(), Some("FREQ=MONTHLY;COUNT=6;BYDAY=-1FR"));
    let yearly = json!({"pattern": {"type": "absoluteYearly", "month": 3, "dayOfMonth": 14}, "range": {"type": "noEnd"}});
    assert_eq!(rule(&yearly).as_deref(), Some("FREQ=YEARLY;BYMONTHDAY=14;BYMONTH=3"));
    let last_day = json!({"pattern": {"type": "absoluteMonthly", "dayOfMonth": 31}, "range": {"type": "noEnd"}});
    assert_eq!(rule(&last_day).as_deref(), Some("FREQ=MONTHLY;BYMONTHDAY=28,29,30,31;BYSETPOS=-1"));
    for odd in [json!({"pattern": {"type": "hourly"}}), json!({"pattern": {"type": "absoluteYearly", "month": 13, "dayOfMonth": 1}}), json!({})] {
        assert_eq!(convert::rrule_of(&odd), None, "{odd}");
    }
    // And back.
    let start_day = days_from_civil(2026, 10, 5);
    let r = convert::recurrence_of(&Recurrence::parse("FREQ=WEEKLY;BYDAY=MO,WE;INTERVAL=2;UNTIL=20261231T235959Z").unwrap(), start_day).unwrap();
    assert_eq!(r["pattern"]["daysOfWeek"], json!(["monday", "wednesday"]));
    assert_eq!((r["pattern"]["interval"].as_u64(), r["range"]["endDate"].as_str(), r["range"]["startDate"].as_str()), (Some(2), Some("2026-12-31"), Some("2026-10-05")));
    let r = convert::recurrence_of(&Recurrence::parse("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1").unwrap(), start_day).unwrap();
    assert_eq!((r["pattern"]["type"].as_str(), r["pattern"]["index"].as_str()), (Some("relativeMonthly"), Some("last")));
    let r = convert::recurrence_of(&Recurrence::new(Freq::Weekly), start_day).unwrap();
    assert_eq!(r["pattern"]["daysOfWeek"], json!(["monday"]), "the start's own day");
    assert_eq!(r["range"]["type"], "noEnd");
}

/// An event goes into the cache and comes back the same.
#[test]
fn events_keep_in_the_cache() {
    let e = convert::event_of(&graph_event(), "C1").unwrap();
    let text = serde_json::to_string(&e).unwrap();
    assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), e);
    let full = new_event();
    assert_eq!(serde_json::from_str::<Event>(&serde_json::to_string(&full).unwrap()).unwrap(), full);
    assert_eq!(format!("{:?}", GraphClient::with_base("http://127.0.0.1:1/v1.0", "tok")), "GraphClient(<redacted>)");
}
