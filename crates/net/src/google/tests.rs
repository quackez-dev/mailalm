//! Google Calendar, Tasks and People against a local test server answering canned JSON.

use mailalm_model::pim::{Attendee, AttendeeKind, MeetingProvider, OnlineMeeting, Recurrence, Remote, Task, Weekday};
use mailalm_model::query::days_from_civil;
use serde_json::json;

use super::*;
use crate::http::tests::serve;

fn answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn ok(body: &Value) -> Vec<u8> {
    answer("200 OK", &body.to_string())
}

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
            "" | "America/Toronto" => Some(-5 * 3600),
            z if mailalm_model::pim::is_utc_name(z) => Some(0),
            _ => None,
        }
    }
}

fn google_event() -> Value {
    json!({
        "id": "ev1", "iCalUID": "ev1@google.com", "etag": "\"3181\"", "status": "confirmed", "summary": "Design review",
        "description": "<b>Bring</b> the boards<br>and notes", "location": "Room 4",
        "start": {"dateTime": "2026-11-25T09:00:00-05:00", "timeZone": "America/Toronto"},
        "end": {"dateTime": "2026-11-25T10:00:00-05:00", "timeZone": "America/Toronto"},
        "organizer": {"email": "zoe@gmail.example", "displayName": "Zoe"},
        "attendees": [
            {"email": "zoe@gmail.example", "organizer": true, "responseStatus": "accepted"},
            {"email": "me@gmail.example", "self": true, "responseStatus": "tentative"},
            {"email": "room@resource.calendar.google.example", "resource": true, "responseStatus": "accepted"},
            {"email": "bad address"},
        ],
        "conferenceData": {"entryPoints": [{"entryPointType": "phone", "uri": "tel:+1-555"}, {"entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij"}],
                           "conferenceSolution": {"key": {"type": "hangoutsMeet"}}},
        "reminders": {"useDefault": false, "overrides": [{"method": "popup", "minutes": 10}, {"method": "email", "minutes": 60}]},
        "sequence": 2,
    })
}

/// The calendar list and a window of events, page by page; events converted; a page token that
/// isn't one refused.
#[test]
fn calendars_and_events_are_read() {
    let (base, seen) = serve(vec![
        ok(&json!({"items": [
            {"id": "primary@gmail.example", "summary": "Me", "backgroundColor": "#9fe1e7", "accessRole": "owner", "primary": true, "etag": "\"1\"",
             "defaultReminders": [{"method": "popup", "minutes": 30}], "conferenceProperties": {"allowedConferenceSolutionTypes": ["hangoutsMeet"]}},
            {"id": "en.canadian#holiday@group.v.calendar.google.example", "summary": "Holidays", "summaryOverride": "Fériés\u{202e}", "accessRole": "reader"},
        ], "nextPageToken": "p2"})),
        ok(&json!({"items": [{"id": "bad id", "summary": "x"}, {"id": "team@group.calendar.google.example", "accessRole": "writer"}]})),
        ok(&json!({"items": [
            google_event(),
            {"id": "ev2", "status": "confirmed", "summary": "Holiday", "start": {"date": "2026-12-25"}, "end": {"date": "2026-12-26"},
             "transparency": "transparent", "reminders": {"useDefault": true}},
        ], "nextPageToken": "page/2+"})),
        ok(&json!({"items": [
            {"id": "ev3_20261126T140000Z", "recurringEventId": "ev3", "originalStartTime": {"dateTime": "2026-11-26T14:00:00Z"},
             "start": {"dateTime": "2026-11-26T15:00:00Z"}, "end": {"dateTime": "2026-11-26T15:30:00Z"}, "summary": "Standup",
             "organizer": {"email": "me@gmail.example", "self": true}, "hangoutLink": "https://meet.google.com/xyz-abcd-efg", "eventType": "outOfOffice"},
            {"id": "ev4", "status": "cancelled"},
            {"id": "bad id", "start": {"date": "2026-12-25"}},
        ]})),
        ok(&json!({"items": [], "nextPageToken": "a token with spaces"})),
        answer("401 Unauthorized", r#"{"error":{"code":401,"message":"Token tok-secret-xyz expired","status":"UNAUTHENTICATED"}}"#),
    ]);
    let mut c = GoogleClient::with_base(&base, "tok-secret-xyz");
    let cals = c.calendars().unwrap();
    let got: Vec<(&str, &str, bool, bool)> = cals.iter().map(|c| (c.id.as_str(), c.name.as_str(), c.can_edit, c.primary)).collect();
    assert_eq!(
        got,
        [
            ("primary@gmail.example", "Me", true, true),
            ("en.canadian#holiday@group.v.calendar.google.example", "Fériés", false, false),
            ("team@group.calendar.google.example", "team@group.calendar.google.example", true, false),
        ]
    );
    assert_eq!((cals[0].color, cals[0].meetings.as_slice(), cals[0].default_reminders.as_slice()), (Some(mailalm_model::Rgb(0x9f, 0xe1, 0xe7)), &[MeetingProvider::Meet][..], &[30u32][..]));

    let events = c.events("primary@gmail.example", at(2026, 11, 1, 0, 0), at(2027, 1, 1, 0, 0)).unwrap();
    assert_eq!(events.iter().map(|e| e.remote.id.as_str()).collect::<Vec<_>>(), ["ev1", "ev2", "ev3_20261126T140000Z"]);
    let e = &events[0];
    assert_eq!((e.uid.as_str(), e.calendar.as_str(), e.remote.version.as_str()), ("ev1@google.com", "primary@gmail.example", "\"3181\""));
    assert_eq!((e.start, e.end, e.time_zone.as_str(), e.location.as_str()), (at(2026, 11, 25, 14, 0), at(2026, 11, 25, 15, 0), "America/Toronto", "Room 4"));
    assert!(e.description.contains("Bring") && e.description.contains("boards") && e.description.contains("notes") && !e.description.contains('<'), "{}", e.description);
    let guests: Vec<(&str, Response, AttendeeKind)> = e.attendees.iter().map(|a| (a.email.as_str(), a.response, a.kind)).collect();
    assert_eq!(
        guests,
        [
            ("zoe@gmail.example", Response::Accepted, AttendeeKind::Required),
            ("me@gmail.example", Response::Tentative, AttendeeKind::Required),
            ("room@resource.calendar.google.example", Response::Accepted, AttendeeKind::Resource),
        ]
    );
    assert_eq!((e.is_organizer, e.my_response, e.sequence), (false, Some(Response::Tentative), 2));
    assert_eq!(e.online, Some(OnlineMeeting { provider: MeetingProvider::Meet, url: "https://meet.google.com/abc-defg-hij".into() }));
    assert_eq!((e.reminders.as_slice(), e.default_reminders), (&[10u32, 60][..], false));
    let h = &events[1];
    assert_eq!((h.all_day, h.start, h.end, h.busy, h.default_reminders, h.time_zone.as_str()), (true, at(2026, 12, 25, 0, 0), at(2026, 12, 26, 0, 0), mailalm_model::pim::BusyStatus::Free, true, ""));
    let o = &events[2];
    assert_eq!((o.series.as_deref(), o.recurrence_id, o.is_organizer, o.my_response), (Some("ev3"), Some(at(2026, 11, 26, 14, 0)), true, None));
    assert_eq!((o.busy, o.online.as_ref().map(|m| m.provider)), (mailalm_model::pim::BusyStatus::OutOfOffice, Some(MeetingProvider::Meet)));

    assert!(matches!(c.events("primary@gmail.example", at(2026, 11, 1, 0, 0), at(2026, 12, 1, 0, 0)), Err(NetError::Protocol(m)) if m.contains("token")));
    let e = c.calendars().unwrap_err();
    assert!(matches!(&e, NetError::Auth(m) if m.contains("***") && !m.contains("tok-secret")), "{e}");
    assert!(matches!(c.events("a b", 0, 10), Err(NetError::Invalid(_))));
    assert!(matches!(c.events("primary", 10, 5), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 6);
    assert!(seen[0].starts_with("GET /calendar/v3/users/me/calendarList?maxResults=250 "), "{}", seen[0]);
    assert!(seen[0].contains("Authorization: Bearer tok-secret-xyz\r\n") && seen[0].contains("Accept: application/json\r\n"), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /calendar/v3/users/me/calendarList?maxResults=250&pageToken=p2 "), "{}", seen[1]);
    assert!(
        seen[2].starts_with(
            "GET /calendar/v3/calendars/primary%40gmail.example/events?timeMin=2026-11-01T00:00:00Z&timeMax=2027-01-01T00:00:00Z&singleEvents=true&orderBy=startTime&maxResults=2500 "
        ),
        "{}",
        seen[2]
    );
    assert!(seen[3].contains("&pageToken=page%2F2%2B "), "{}", seen[3]);
}

/// A full sync (a series, a cancelled occurrence, a deleted event), then the next one, which
/// Google no longer knows: a resync.
#[test]
fn calendars_sync_by_token() {
    let (base, seen) = serve(vec![
        ok(&json!({"items": [
            {"id": "ser1", "iCalUID": "ser1@google.com", "summary": "Weekly",
             "start": {"dateTime": "2026-11-02T09:00:00-05:00", "timeZone": "America/Toronto"}, "end": {"dateTime": "2026-11-02T10:00:00-05:00", "timeZone": "America/Toronto"},
             "recurrence": ["EXDATE;TZID=America/Toronto:20261109T090000", "RRULE:FREQ=WEEKLY;BYDAY=MO"]},
            {"id": "ser1_20261116T140000Z", "status": "cancelled", "recurringEventId": "ser1",
             "originalStartTime": {"dateTime": "2026-11-16T09:00:00-05:00", "timeZone": "America/Toronto"}},
        ], "nextPageToken": "n1"})),
        ok(&json!({"items": [{"id": "gone1", "status": "cancelled"}], "nextSyncToken": "sync/1+="})),
        answer("410 Gone", r#"{"error":{"code":410,"message":"Sync token is no longer valid, a full sync is required.","errors":[{"reason":"fullSyncRequired"}]}}"#),
        ok(&json!({"items": []})),
    ]);
    let mut c = GoogleClient::with_base(&base, "tok");
    let sync = c.sync_events("primary", None).unwrap();
    assert_eq!((sync.token.as_str(), sync.removed.as_slice()), ("sync/1+=", &["gone1".to_string()][..]));
    assert_eq!(sync.changed.len(), 2);
    let series = &sync.changed[0];
    assert_eq!(series.recurrence.as_ref().map(|r| r.by_day.clone()), Some(vec![(0, Weekday::Mo)]));
    let gone = &sync.changed[1];
    assert_eq!((gone.status, gone.series.as_deref(), gone.recurrence_id, gone.start), (EventStatus::Cancelled, Some("ser1"), Some(at(2026, 11, 16, 14, 0)), at(2026, 11, 16, 14, 0)));
    // The series, its cancelled occurrence taken out.
    let starts: Vec<i64> = mailalm_model::pim::occurrences(&sync.changed, at(2026, 11, 1, 0, 0), at(2026, 11, 24, 0, 0), &November).iter().map(|o| o.start).collect();
    assert_eq!(starts, [at(2026, 11, 2, 14, 0), at(2026, 11, 9, 14, 0), at(2026, 11, 23, 14, 0)]);

    let e = c.sync_events("primary", Some(&sync.token)).unwrap_err();
    assert!(matches!(&e, NetError::Resync(m) if m.contains("410")) && !e.is_fatal(), "{e}");
    assert!(matches!(c.sync_events("primary", None), Err(NetError::Protocol(m)) if m.contains("without its token")));
    assert!(matches!(c.sync_events("primary", Some("bad token")), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 4);
    assert!(seen[0].starts_with("GET /calendar/v3/calendars/primary/events?maxResults=2500 "), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /calendar/v3/calendars/primary/events?maxResults=2500&pageToken=n1 "), "{}", seen[1]);
    assert!(seen[2].starts_with("GET /calendar/v3/calendars/primary/events?maxResults=2500&syncToken=sync%2F1%2B%3D "), "{}", seen[2]);
}

fn new_event() -> Event {
    Event {
        calendar: "primary".into(),
        title: "Planning".into(),
        start: at(2026, 11, 25, 14, 0),
        end: at(2026, 11, 25, 15, 0),
        time_zone: "America/Toronto".into(),
        recurrence: Recurrence::parse("FREQ=WEEKLY;BYDAY=WE;COUNT=4"),
        organizer: Some(Attendee::new("me@gmail.example", "")),
        attendees: vec![Attendee::new("me@gmail.example", ""), Attendee { kind: AttendeeKind::Optional, ..Attendee::new("zoe@studio.example", "Zoe") }],
        reminders: vec![10],
        online: Some(OnlineMeeting::request(MeetingProvider::Meet)),
        ..Event::default()
    }
}

/// A meeting with a Meet link asked for, its guests told; an occurrence changed; an event deleted
/// quietly; a Teams link refused before anything is sent.
#[test]
fn events_are_written() {
    let mut made = google_event();
    made["conferenceData"] = json!({"createRequest": {"requestId": "x", "status": {"statusCode": "pending"}}});
    let (base, seen) = serve(vec![ok(&made), ok(&google_event()), answer("410 Gone", "")]);
    let mut c = GoogleClient::with_base(&base, "tok");
    let e = c.create_event("primary", &new_event(), &November).unwrap();
    assert_eq!(e.online, Some(OnlineMeeting::request(MeetingProvider::Meet)), "the link is still being made");
    let teams = Event { online: Some(OnlineMeeting::request(MeetingProvider::Teams)), ..new_event() };
    assert!(matches!(c.create_event("primary", &teams, &November), Err(NetError::Invalid(_))));
    let occurrence = Event { remote: Remote::new("ev3_20261126T140000Z", ""), series: Some("ev3".into()), recurrence_id: Some(1), online: None, ..new_event() };
    c.update_event(&occurrence, &November).unwrap();
    c.delete_event("primary", "ev1", false).unwrap();
    assert!(matches!(c.update_event(&new_event(), &November), Err(NetError::Invalid(_))), "no id");

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].starts_with("POST /calendar/v3/calendars/primary/events?conferenceDataVersion=1&sendUpdates=all "), "{}", seen[0]);
    let sent = body(&seen[0]);
    assert_eq!(sent["start"], json!({"dateTime": "2026-11-25T14:00:00Z", "timeZone": "America/Toronto"}));
    assert_eq!(sent["recurrence"], json!(["RRULE:FREQ=WEEKLY;COUNT=4;BYDAY=WE"]));
    assert_eq!(sent["attendees"], json!([{"email": "zoe@studio.example", "displayName": "Zoe", "optional": true}]));
    assert_eq!(sent["reminders"], json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": 10}]}));
    assert_eq!(sent["conferenceData"]["createRequest"]["conferenceSolutionKey"], json!({"type": "hangoutsMeet"}));
    let id = sent["conferenceData"]["createRequest"]["requestId"].as_str().unwrap();
    assert!(id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
    assert_eq!((sent["transparency"].as_str(), sent["summary"].as_str()), (Some("opaque"), Some("Planning")));
    assert!(seen[1].starts_with("PATCH /calendar/v3/calendars/primary/events/ev3_20261126T140000Z?conferenceDataVersion=1&sendUpdates=all "), "{}", seen[1]);
    assert!(body(&seen[1]).get("recurrence").is_none() && body(&seen[1]).get("conferenceData").is_none());
    assert!(seen[2].starts_with("DELETE /calendar/v3/calendars/primary/events/ev1?sendUpdates=none "), "{}", seen[2]);
}

/// Answering patches the user's own guest entry alone; Google's API has no counter-proposal.
#[test]
fn invitations_are_answered() {
    let (base, seen) = serve(vec![
        ok(&json!({"attendees": [{"email": "me@gmail.example", "self": true, "responseStatus": "needsAction"}]})),
        ok(&google_event()),
        ok(&json!({"attendees": [{"email": "zoe@gmail.example", "organizer": true}]})),
    ]);
    let mut c = GoogleClient::with_base(&base, "tok");
    c.respond("primary", "ev1", Response::Accepted, true, "See you\u{7} there", None).unwrap();
    assert!(matches!(c.respond("primary", "ev1", Response::Declined, false, "", None), Err(NetError::Server(_))));
    assert!(matches!(c.respond("primary", "ev1", Response::Tentative, true, "", Some((1, 2))), Err(NetError::Invalid(_))));
    assert!(matches!(c.respond("primary", "ev1", Response::NeedsAction, true, "", None), Err(NetError::Invalid(_))));
    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].starts_with("GET /calendar/v3/calendars/primary/events/ev1?maxAttendees=1&fields=attendees "), "{}", seen[0]);
    assert!(seen[1].starts_with("PATCH /calendar/v3/calendars/primary/events/ev1?sendUpdates=all "), "{}", seen[1]);
    assert_eq!(body(&seen[1]), json!({"attendeesOmitted": true, "attendees": [{"email": "me@gmail.example", "responseStatus": "accepted", "comment": "See you there"}]}));
}

#[test]
fn tasks_are_read_and_written() {
    let (base, seen) = serve(vec![
        ok(&json!({"items": [{"id": "MDE", "title": "My Tasks", "etag": "\"e1\""}, {"id": "MDI", "title": "Errands"}, {"title": "no id"}]})),
        ok(&json!({"items": [
            {"id": "t1", "title": "Pay rent", "notes": "before\nthe 1st", "status": "needsAction", "due": "2026-12-01T00:00:00.000Z"},
            {"id": "t2", "deleted": true},
        ], "nextPageToken": "x"})),
        ok(&json!({"items": [{"id": "t3", "title": "Done", "status": "completed", "completed": "2026-11-20T10:00:00.000Z"}]})),
        ok(&json!({"id": "t9", "title": "New", "status": "needsAction"})),
        ok(&json!({"id": "t9", "title": "New", "status": "needsAction"})),
        answer("204 No Content", ""),
    ]);
    let mut c = GoogleClient::with_base(&base, "tok");
    let lists = c.task_lists().unwrap();
    assert_eq!(lists.iter().map(|l| (l.id.as_str(), l.name.as_str(), l.default)).collect::<Vec<_>>(), [("MDE", "My Tasks", true), ("MDI", "Errands", false)]);
    let tasks = c.tasks("MDE").unwrap();
    assert_eq!(tasks.iter().map(|t| t.remote.id.as_str()).collect::<Vec<_>>(), ["t1", "t3"]);
    assert_eq!((tasks[0].notes.as_str(), tasks[0].due, tasks[0].completed), ("before\nthe 1st", Some(at(2026, 12, 1, 0, 0)), false));
    assert_eq!((tasks[1].completed, tasks[1].completed_at), (true, Some(at(2026, 11, 20, 10, 0))));
    let new = Task { list: "MDE".into(), title: "New".into(), due: Some(at(2026, 12, 3, 0, 0)), reminder: Some(5), ..Task::default() };
    assert_eq!(c.create_task("MDE", &new).unwrap().remote.id, "t9");
    assert!(!c.complete_task("MDE", "t9", false).unwrap().completed);
    c.delete_task("MDE", "t9").unwrap();
    assert!(matches!(c.tasks(""), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("GET /tasks/v1/users/@me/lists?maxResults=100 "), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /tasks/v1/lists/MDE/tasks?maxResults=100&showCompleted=true&showHidden=true "), "{}", seen[1]);
    assert!(seen[2].contains("&pageToken=x "), "{}", seen[2]);
    assert!(seen[3].starts_with("POST /tasks/v1/lists/MDE/tasks "), "{}", seen[3]);
    assert_eq!(body(&seen[3]), json!({"title": "New", "notes": "", "status": "needsAction", "due": "2026-12-03T00:00:00.000Z", "completed": null}));
    assert!(seen[4].starts_with("PATCH /tasks/v1/lists/MDE/tasks/t9 "), "{}", seen[4]);
    assert_eq!(body(&seen[4]), json!({"status": "needsAction", "completed": null}));
    assert!(seen[5].starts_with("DELETE /tasks/v1/lists/MDE/tasks/t9 "), "{}", seen[5]);
}

#[test]
fn contacts_are_read() {
    let (base, seen) = serve(vec![
        ok(&json!({"connections": [
            {"resourceName": "people/c1", "etag": "%E1",
             "names": [{"displayName": "Other", "metadata": {"primary": false}}, {"displayName": "Ana Lima", "givenName": "Ana", "familyName": "Lima", "metadata": {"primary": true}}],
             "emailAddresses": [{"value": "ana@x.example"}, {"value": "ANA@x.example"}, {"value": "bad"}],
             "phoneNumbers": [{"value": "+1 555 0100"}], "organizations": [{"name": "Sound Lab", "title": "Mixer"}]},
            {"resourceName": "../../evil", "names": [{"displayName": "x"}]},
            {"resourceName": "people/c2"},
        ], "nextPageToken": "pp", "totalPeople": 3})),
        ok(&json!({"connections": [{"resourceName": "people/c3", "emailAddresses": [{"value": "x@y.example"}]}]})),
    ]);
    let mut c = GoogleClient::with_base(&base, "tok");
    let people = c.contacts().unwrap();
    assert_eq!(people.iter().map(|p| (p.remote.id.as_str(), p.display())).collect::<Vec<_>>(), [("people/c1", "Ana Lima".to_string()), ("people/c3", "x@y.example".to_string())]);
    let p = &people[0];
    assert_eq!((p.given_name.as_str(), p.emails.as_slice(), p.phones.as_slice()), ("Ana", &["ana@x.example".to_string()][..], &["+1 555 0100".to_string()][..]));
    assert_eq!((p.organization.as_str(), p.job_title.as_str()), ("Sound Lab", "Mixer"));
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("GET /v1/people/me/connections?personFields=names,emailAddresses,phoneNumbers,organizations&pageSize=1000 "), "{}", seen[0]);
    assert!(seen[1].contains("&pageToken=pp "), "{}", seen[1]);
}

#[test]
fn errors_from_statuses() {
    assert!(matches!(status_error(401, None), Err(NetError::Auth(_))));
    let scope = json!({"error": {"code": 403, "message": "Request had insufficient authentication scopes.", "errors": [{"reason": "insufficientPermissions"}]}});
    assert!(matches!(status_error(403, Some(&scope)), Err(NetError::Auth(m)) if m.contains("sign in again")));
    let rate = json!({"error": {"errors": [{"reason": "rateLimitExceeded"}]}});
    assert!(matches!(status_error(403, Some(&rate)), Err(NetError::Protocol(m)) if m.contains("busy")));
    assert!(matches!(status_error(403, None), Err(NetError::Server(m)) if m == "HTTP 403"));
    assert!(matches!(status_error(503, None), Err(NetError::Protocol(_))));
    assert!(matches!(status_error(404, Some(&json!({"error": {"message": "Not Found"}}))), Err(NetError::Server(m)) if m == "HTTP 404: Not Found"));
    assert!(status_error(204, None).is_ok());
    assert!(token_ok("CAESBgjB") && !token_ok("") && !token_ok("a b") && !token_ok(&"a".repeat(5000)));
    assert!(convert::id_ok("en.canadian#holiday@group.v.calendar.google.com") && !convert::id_ok("a b") && !convert::id_ok(""));
    assert_eq!(format!("{:?}", GoogleClient::new("tok")), "GoogleClient(<redacted>)");
}
