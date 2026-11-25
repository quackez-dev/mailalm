use super::*;
use crate::pim::tests::{Toronto, at};
use crate::pim::{MeetingProvider, UtcOnly, occurrences};

/// An invitation as Outlook.com sends one: a Windows zone defined in the file, escapes, a folded
/// line, a Teams link in the notes, a reminder.
const OUTLOOK: &str = "BEGIN:VCALENDAR\r\n\
METHOD:REQUEST\r\n\
PRODID:Microsoft Exchange Server 2010\r\n\
VERSION:2.0\r\n\
BEGIN:VTIMEZONE\r\n\
TZID:Eastern Standard Time\r\n\
BEGIN:STANDARD\r\n\
DTSTART:16010101T020000\r\n\
TZOFFSETFROM:-0400\r\n\
TZOFFSETTO:-0500\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH=11\r\n\
END:STANDARD\r\n\
BEGIN:DAYLIGHT\r\n\
DTSTART:16010101T020000\r\n\
TZOFFSETFROM:-0500\r\n\
TZOFFSETTO:-0400\r\n\
RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=2SU;BYMONTH=3\r\n\
END:DAYLIGHT\r\n\
END:VTIMEZONE\r\n\
BEGIN:VEVENT\r\n\
ORGANIZER;CN=Zoe Park:mailto:zoe@studio.example\r\n\
ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;CN=Me:mailto:me@outlook.example\r\n\
ATTENDEE;CUTYPE=ROOM;ROLE=NON-PARTICIPANT;CN=\"Room 4, East\":mailto:room4@studio.example\r\n\
ATTENDEE;CN=Bad:mailto:not an address\r\n\
DESCRIPTION;LANGUAGE=en-US:Agenda:\\nMix review\\n\\nJoin: https://teams.micro\r\n \
soft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0\r\n\
UID:040000008200E00074C5B7101A82E008\r\n\
SUMMARY;LANGUAGE=en-US:Mix review\\, part 2\r\n\
DTSTART;TZID=Eastern Standard Time:20261125T090000\r\n\
DTEND;TZID=Eastern Standard Time:20261125T100000\r\n\
CLASS:PUBLIC\r\n\
DTSTAMP:20261120T150000Z\r\n\
TRANSP:OPAQUE\r\n\
STATUS:CONFIRMED\r\n\
SEQUENCE:2\r\n\
LOCATION;LANGUAGE=en-US:Microsoft Teams Meeting\r\n\
X-MICROSOFT-CDO-BUSYSTATUS:TENTATIVE\r\n\
BEGIN:VALARM\r\n\
DESCRIPTION:REMINDER\r\n\
TRIGGER;RELATED=START:-PT15M\r\n\
ACTION:DISPLAY\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

/// An invitation as Google Calendar sends one: UTC times, a weekly series with an occurrence taken
/// out, a Meet link.
const GOOGLE: &str = "BEGIN:VCALENDAR\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\nVERSION:2.0\nCALSCALE:GREGORIAN\nMETHOD:REQUEST\n\
BEGIN:VEVENT\nDTSTART:20261102T140000Z\nDTEND:20261102T143000Z\nRRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4\nEXDATE:20261109T140000Z\n\
DTSTAMP:20261101T120000Z\nORGANIZER;CN=Ann:mailto:ann@gmail.example\nUID:abc123@google.com\n\
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=OPT-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Ann;X-NUM-GUESTS=0:mailto:ann@gmail.example\n\
X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\nSUMMARY:Standup\nTRANSP:TRANSPARENT\nEND:VEVENT\nEND:VCALENDAR\n";

fn outlook_event() -> Event {
    parse(OUTLOOK, &UtcOnly).events.into_iter().next().unwrap()
}

#[test]
fn an_outlook_invitation_is_read() {
    let inv = parse(OUTLOOK, &UtcOnly);
    assert_eq!(inv.method, Some(Method::Request));
    assert_eq!(inv.events.len(), 1);
    let e = &inv.events[0];
    assert_eq!(e.uid, "040000008200E00074C5B7101A82E008");
    assert_eq!(e.title, "Mix review, part 2");
    // 09:00 in New York's winter, from the file's own zone (the rules don't know it).
    assert_eq!((e.start, e.end, e.all_day), (at(2026, 11, 25, 14, 0), at(2026, 11, 25, 15, 0), false));
    assert_eq!(e.time_zone, "Eastern Standard Time");
    assert_eq!(e.description, "Agenda:\nMix review\n\nJoin: https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0");
    assert_eq!(e.location, "Microsoft Teams Meeting");
    assert_eq!(e.organizer.as_ref().map(|o| (o.email.as_str(), o.name.as_str())), Some(("zoe@studio.example", "Zoe Park")));
    let guests: Vec<(&str, &str, AttendeeKind, Response)> = e.attendees.iter().map(|a| (a.email.as_str(), a.name.as_str(), a.kind, a.response)).collect();
    assert_eq!(
        guests,
        [
            ("me@outlook.example", "Me", AttendeeKind::Required, Response::NeedsAction),
            ("room4@studio.example", "Room 4, East", AttendeeKind::Resource, Response::NeedsAction),
        ]
    );
    assert_eq!((e.busy, e.status, e.sequence), (BusyStatus::Tentative, EventStatus::Confirmed, 2));
    assert_eq!(e.reminders, [15]);
    assert_eq!(e.online.as_ref().map(|o| o.provider), Some(MeetingProvider::Teams));
    // With rules that know New York, the zone is kept under its IANA name.
    let e = parse(OUTLOOK, &Toronto).events.into_iter().next().unwrap();
    assert_eq!((e.start, e.time_zone.as_str()), (at(2026, 11, 25, 14, 0), "America/New_York"));
}

#[test]
fn a_google_invitation_is_read() {
    let inv = parse(GOOGLE, &Toronto);
    let e = &inv.events[0];
    assert_eq!((e.start, e.end, e.time_zone.as_str()), (at(2026, 11, 2, 14, 0), at(2026, 11, 2, 14, 30), "UTC"));
    assert_eq!(e.recurrence.as_ref().map(Recurrence::to_rrule).as_deref(), Some("FREQ=WEEKLY;COUNT=4;BYDAY=MO"));
    assert_eq!(e.exdates, [at(2026, 11, 9, 14, 0)]);
    assert_eq!(e.online.as_ref().map(|o| (o.provider, o.url.as_str())), Some((MeetingProvider::Meet, "https://meet.google.com/abc-defg-hij")));
    assert_eq!((e.attendees[0].kind, e.attendees[0].response), (AttendeeKind::Optional, Response::Accepted));
    assert_eq!(e.busy, BusyStatus::Free);
    let starts: Vec<i64> = occurrences(&inv.events, at(2026, 11, 1, 0, 0), at(2026, 12, 1, 0, 0), &Toronto).iter().map(|o| o.start).collect();
    assert_eq!(starts, [at(2026, 11, 2, 14, 0), at(2026, 11, 16, 14, 0), at(2026, 11, 23, 14, 0)]);
}

#[test]
fn a_cancellation_cancels() {
    let text = "BEGIN:VCALENDAR\r\nMETHOD:CANCEL\r\nBEGIN:VEVENT\r\nUID:x1\r\nDTSTART;VALUE=DATE:20261224\r\nRECURRENCE-ID;VALUE=DATE:20261224\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let inv = parse(text, &UtcOnly);
    assert_eq!(inv.method, Some(Method::Cancel));
    let e = &inv.events[0];
    assert_eq!((e.status, e.all_day, e.start, e.end), (EventStatus::Cancelled, true, at(2026, 12, 24, 0, 0), at(2026, 12, 25, 0, 0)));
    assert_eq!(e.recurrence_id, Some(at(2026, 12, 24, 0, 0)));
}

/// What a mail carries is hostile: nesting, numbers, sizes, missing parts, junk.
#[test]
fn hostile_calendars_are_capped() {
    let event = |uid: &str| format!("BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20261125T090000Z\r\nEND:VEVENT\r\n");
    let deep = format!("BEGIN:VCALENDAR\r\n{}{}", "BEGIN:X\r\n".repeat(100), event("deep"));
    assert!(parse(&deep, &UtcOnly).events.is_empty());
    let many = format!("BEGIN:VCALENDAR\r\n{}END:VCALENDAR\r\n", (0..600).map(|i| event(&format!("e{i}"))).collect::<String>());
    assert_eq!(parse(&many, &UtcOnly).events.len(), MAX_EVENTS);
    let guests: String = (0..600).map(|i| format!("ATTENDEE:mailto:g{i}@x.example\r\n")).collect();
    let crowded = format!("BEGIN:VEVENT\r\nUID:c\r\nDTSTART:20261125T090000Z\r\n{guests}END:VEVENT\r\n");
    assert_eq!(parse(&crowded, &UtcOnly).events[0].attendees.len(), MAX_ATTENDEES);
    // No UID, or no start that reads: no event.
    let broken = "BEGIN:VEVENT\r\nDTSTART:20261125T090000Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART:20261325T090000Z\r\nEND:VEVENT\r\n";
    assert!(parse(broken, &UtcOnly).events.is_empty());
    // An event inside an event is not the calendar's, and changes nothing of the outer one.
    let nested = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:outer\r\nDTSTART:20261125T090000Z\r\nSUMMARY:Real\r\nBEGIN:VEVENT\r\nUID:inner\r\nSUMMARY:evil\r\nEND:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let inv = parse(nested, &UtcOnly);
    assert_eq!(inv.events.iter().map(|e| (e.uid.as_str(), e.title.as_str())).collect::<Vec<_>>(), [("outer", "Real")]);
    // Past the size read, nothing more.
    let big = format!("BEGIN:VCALENDAR\r\nX-FILL:{}\r\n{}", "a".repeat(MAX_INPUT), event("late"));
    assert!(parse(&big, &UtcOnly).events.is_empty());
    // Junk, controls, an END without a BEGIN, text that hides itself.
    let junk = "\u{0}\u{ff}BEGIN:VEVENT:::;;;\"\"\"\r\nEND:VTODO\r\nBEGIN:VEVENT\r\nUID:j\u{7}1\r\nDTSTART:20261125T090000Z\r\nSUMMARY:a\u{202e}b\u{1b}[2J\r\nEND:VEVENT\r\n";
    let inv = parse(junk, &UtcOnly);
    assert_eq!(inv.events.iter().map(|e| (e.uid.as_str(), e.title.as_str())).collect::<Vec<_>>(), [("j1", "ab [2J")]);
    assert!(parse("", &UtcOnly).events.is_empty());
    // A duration past what a number holds is none; a huge one stops at the end of time.
    for (d, longer) in [("P99999999999999999999D", false), ("P999999999999999D", true)] {
        let text = format!("BEGIN:VEVENT\r\nUID:x\r\nDTSTART:20261125T090000Z\r\nDURATION:{d}\r\nEND:VEVENT");
        let e = parse(&text, &UtcOnly).events.into_iter().next().unwrap();
        assert_eq!(e.end > e.start, longer, "{d}");
    }
}

#[test]
fn values_are_read() {
    assert_eq!(date_time("20261125"), Some((at(2026, 11, 25, 0, 0), true, false)));
    assert_eq!(date_time("20261125T090000Z"), Some((at(2026, 11, 25, 9, 0), false, true)));
    assert_eq!(date_time("20261125T090000"), Some((at(2026, 11, 25, 9, 0), false, false)));
    for bad in ["20261325", "20260230", "20261125T250000", "2026112", "20261125T0900", "2026-11-25", "２０２６1125"] {
        assert_eq!(date_time(bad), None, "{bad:?}");
    }
    assert_eq!((duration("-PT15M"), duration("P1W"), duration("P1DT2H"), duration("15M"), duration("P1X")), (Some(-900), Some(604_800), Some(93_600), None, None));
    assert_eq!((utc_offset("-0500"), utc_offset("+053000"), utc_offset("0500"), utc_offset("+99")), (Some(-18_000), Some(19_800), None, None));
    assert_eq!(unescape("a\\, b\\; c\\nd\\\\"), "a, b; c\nd\\");
    assert_eq!(escape("a, b; c\r\nd\\"), "a\\, b\\; c\\nd\\\\");
}

fn series() -> Event {
    Event {
        uid: "series-1@mailalm".into(),
        start: at(2026, 11, 2, 14, 0),
        end: at(2026, 11, 2, 15, 0),
        time_zone: "America/Toronto".into(),
        recurrence: Recurrence::parse("FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T235959Z"),
        exdates: vec![at(2026, 11, 9, 14, 0)],
        organizer: Some(Attendee::new("me@videotron.example", "")),
        attendees: vec![Attendee::new("me@videotron.example", ""), Attendee::new("zoe@studio.example", "Zoe")],
        title: "Weekly; sync, notes".into(),
        description: "Line 1\nLine 2".into(),
        online: OnlineMeeting::at("https://jitsi.example.org/room-42"),
        sequence: 1,
        ..Event::default()
    }
}

/// An invitation written for an IMAP account: the series in its zone with a definition made
/// from the rules, read back the same, with those rules or with the definition alone.
#[test]
fn an_invitation_is_written_and_read_back() {
    let now = at(2026, 11, 24, 12, 0);
    let e = series();
    let text = request(std::slice::from_ref(&e), &Toronto, now).unwrap();
    for want in [
        "METHOD:REQUEST\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:America/Toronto\r\n",
        "BEGIN:DAYLIGHT\r\nDTSTART:20260308T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\r\nTZOFFSETFROM:-0500\r\nTZOFFSETTO:-0400\r\n",
        "BEGIN:STANDARD\r\nDTSTART:20261101T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\n",
        "DTSTART;TZID=America/Toronto:20261102T090000\r\n",
        "RRULE:FREQ=WEEKLY;UNTIL=20261231T235959Z;BYDAY=MO\r\n",
        "EXDATE;TZID=America/Toronto:20261109T090000\r\n",
        "SUMMARY:Weekly\\; sync\\, notes\r\n",
        "ORGANIZER:mailto:me@videotron.example\r\n",
        "SEQUENCE:1\r\n",
        "DTSTAMP:20261124T120000Z\r\n",
        "CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO:https://jitsi.example.org/room-42\r\n",
    ] {
        assert!(text.contains(want), "{want:?} in\n{text}");
    }
    let unfolded = text.replace("\r\n ", "");
    assert!(unfolded.contains("ATTENDEE;CN=Zoe;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:zoe@studio.example\r\n"), "{text}");
    // The organizer isn't invited to their own meeting.
    assert!(!unfolded.contains("ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:me@videotron.example"));
    assert!(text.split("\r\n").all(|l| l.len() <= 75), "{text}");
    for rules in [&Toronto as &dyn ZoneRules, &UtcOnly] {
        let back = parse(&text, rules);
        assert_eq!(back.method, Some(Method::Request));
        let b = &back.events[0];
        assert_eq!((b.start, b.end, b.time_zone.as_str()), (e.start, e.end, "America/Toronto"));
        assert_eq!((&b.recurrence, &b.exdates), (&e.recurrence, &e.exdates));
        assert_eq!((b.uid.as_str(), b.title.as_str(), b.sequence), ("series-1@mailalm", "Weekly; sync, notes", 1));
        assert_eq!(b.description, "Line 1\nLine 2\n\nJoin: https://jitsi.example.org/room-42");
        assert_eq!(b.online.as_ref().map(|o| o.provider), Some(MeetingProvider::Link));
        assert_eq!(b.attendees.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["zoe@studio.example"]);
        // Summer time or not, 09:00 in Toronto.
        let starts: Vec<i64> = occurrences(&back.events, at(2026, 11, 1, 0, 0), at(2026, 11, 17, 0, 0), &Toronto).iter().map(|o| o.start).collect();
        assert_eq!(starts, [at(2026, 11, 2, 14, 0), at(2026, 11, 16, 14, 0)]);
    }
    // A single meeting goes in UTC, with no zone.
    let single = Event { recurrence: None, exdates: Vec::new(), ..series() };
    let text = request(&[single], &Toronto, now).unwrap();
    assert!(text.contains("DTSTART:20261102T140000Z\r\n") && !text.contains("VTIMEZONE"), "{text}");
    // No organizer, no UID, no address: nothing.
    assert!(request(&[Event { organizer: None, ..series() }], &Toronto, now).is_none());
    assert!(request(&[Event { uid: "\r\n".into(), ..series() }], &Toronto, now).is_none());
    assert!(request(&[Event { organizer: Some(Attendee::new("nobody", "")), ..series() }], &Toronto, now).is_none());
    assert!(request(&[], &Toronto, now).is_none());
}

#[test]
fn a_cancellation_is_written() {
    let text = cancel(&series(), &Toronto, at(2026, 11, 24, 12, 0)).unwrap();
    assert!(text.contains("METHOD:CANCEL\r\n") && text.contains("STATUS:CANCELLED\r\n"), "{text}");
    let back = parse(&text, &Toronto);
    assert_eq!((back.method, back.events[0].status), (Some(Method::Cancel), EventStatus::Cancelled));
}

/// A guest's answers: accept (with a comment), and a proposal of another time.
#[test]
fn answers_are_written() {
    let e = outlook_event();
    let me = Attendee::new("me@outlook.example", "Me");
    let now = at(2026, 11, 24, 12, 0);
    let text = reply(&e, &me, Response::Accepted, None, "See you\nthere", now).unwrap();
    for want in [
        "METHOD:REPLY\r\n",
        "UID:040000008200E00074C5B7101A82E008\r\n",
        "SEQUENCE:2\r\n",
        "DTSTART:20261125T140000Z\r\n",
        "COMMENT:See you\\nthere\r\n",
        "ORGANIZER;CN=Zoe Park:mailto:zoe@studio.example\r\n",
    ] {
        assert!(text.contains(want), "{want:?} in\n{text}");
    }
    assert!(text.replace("\r\n ", "").contains("ATTENDEE;CN=Me;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED:mailto:me@outlook.example\r\n"), "{text}");
    let back = parse(&text, &UtcOnly);
    assert_eq!((back.method, back.comment.as_str()), (Some(Method::Reply), "See you\nthere"));
    assert_eq!(back.events[0].attendees.iter().map(|a| (a.email.as_str(), a.response)).collect::<Vec<_>>(), [("me@outlook.example", Response::Accepted)]);

    let new_time = (at(2026, 11, 26, 15, 0), at(2026, 11, 26, 16, 0));
    let text = reply(&e, &me, Response::Tentative, Some(new_time), "", now).unwrap();
    assert!(text.contains("METHOD:COUNTER\r\n") && text.contains("DTSTART:20261126T150000Z\r\n") && text.contains("DTEND:20261126T160000Z\r\n"), "{text}");
    assert!(text.contains("PARTSTAT=TENTATIVE") && !text.contains("COMMENT"), "{text}");
    let back = parse(&text, &UtcOnly);
    assert_eq!((back.method, back.events[0].start), (Some(Method::Counter), new_time.0));
    // No answer is no reply; someone who isn't an address can't answer.
    assert!(reply(&e, &me, Response::NeedsAction, None, "", now).is_none());
    assert!(reply(&e, &Attendee::new("me", ""), Response::Declined, None, "", now).is_none());
}

/// Text from the user can't start lines of its own: line ends are escaped, quotes and controls
/// taken out of parameters, long lines folded on characters.
#[test]
fn written_text_stays_in_its_line() {
    let mut e = series();
    e.title = format!("Hi\r\nBEGIN:VEVENT\r\nUID:evil {}", "é".repeat(100));
    e.attendees.push(Attendee::new("x@y.example", "A \"quoted\"; name:\r\nX"));
    let text = request(&[e], &Toronto, 0).unwrap();
    let back = parse(&text, &Toronto);
    assert_eq!(back.events.len(), 1);
    assert!(back.events[0].title.starts_with("Hi BEGIN:VEVENT UID:evil é"), "{}", back.events[0].title);
    assert!(back.events[0].attendees.iter().any(|a| a.email == "x@y.example" && a.name == "A quoted; name:X"), "{:?}", back.events[0].attendees);
    assert!(text.split("\r\n").all(|l| l.len() <= 75));
}
