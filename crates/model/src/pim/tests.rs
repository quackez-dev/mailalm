use super::recurrence::nth_weekday;
use super::*;

/// Toronto's rules as they are since 2007 (and New York's, the same), for the tests: UTC−5, and
/// UTC−4 from the second Sunday of March at 2:00 to the first Sunday of November at 2:00. The
/// viewer is in Toronto too.
pub(crate) struct Toronto;

impl ZoneRules for Toronto {
    fn offset(&self, zone: &str, utc: i64) -> Option<i64> {
        match zone {
            "" | "America/Toronto" | "America/New_York" => {}
            z if is_utc_name(z) => return Some(0),
            _ => return None,
        }
        let (y, _, _) = civil_from_days(utc.div_euclid(86_400));
        let summer = nth_weekday(y, 3, 2, Weekday::Su)? * 86_400 + 7 * 3600;
        let winter = nth_weekday(y, 11, 1, Weekday::Su)? * 86_400 + 6 * 3600;
        Some(if (summer..winter).contains(&utc) { -4 * 3600 } else { -5 * 3600 })
    }
}

/// A UTC time.
pub(crate) fn at(y: i64, m: i64, d: i64, h: i64, mi: i64) -> i64 {
    days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60
}

fn ev(uid: &str, start: i64, len: i64) -> Event {
    Event { uid: uid.into(), start, end: start + len, ..Event::default() }
}

#[test]
fn rules_read_and_write() {
    let r = Recurrence::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;UNTIL=20261231T235959Z;WKST=SU").unwrap();
    assert_eq!((r.freq, r.interval, r.week_start), (Freq::Weekly, 2, Weekday::Su));
    assert_eq!(r.by_day, [(0, Weekday::Mo), (0, Weekday::We)]);
    assert_eq!(r.until, Some(Until::At(at(2026, 12, 31, 23, 59) + 59)));
    assert_eq!(r.to_rrule(), "FREQ=WEEKLY;INTERVAL=2;UNTIL=20261231T235959Z;BYDAY=MO,WE;WKST=SU");
    let r = Recurrence::parse("RRULE:freq=monthly;byday=-1FR;count=6").unwrap();
    assert_eq!((r.by_day.as_slice(), r.count), (&[(-1, Weekday::Fr)][..], Some(6)));
    assert_eq!(r.to_rrule(), "FREQ=MONTHLY;COUNT=6;BYDAY=-1FR");
    let r = Recurrence::parse("FREQ=YEARLY;BYMONTH=11;BYDAY=+4TH;UNTIL=20301231").unwrap();
    assert_eq!((r.by_month.as_slice(), r.until), (&[11u8][..], Some(Until::Date(days_from_civil(2030, 12, 31)))));
    assert_eq!(r.to_rrule(), "FREQ=YEARLY;UNTIL=20301231;BYDAY=4TH;BYMONTH=11");
    assert_eq!(Recurrence::parse("FREQ=DAILY;X-NAME=1").map(|r| r.freq), Some(Freq::Daily));
    // Outside the subset, or not a rule: not read.
    for bad in [
        "",
        "junk",
        "FREQ=HOURLY",
        "FREQ=DAILY;BYHOUR=9",
        "FREQ=YEARLY;BYWEEKNO=20",
        "FREQ=DAILY;INTERVAL=0",
        "FREQ=DAILY;COUNT=0",
        "FREQ=WEEKLY;BYDAY=XX",
        "FREQ=MONTHLY;BYMONTHDAY=32",
        "FREQ=DAILY;UNTIL=2026-12-31",
        "INTERVAL=2",
        "FREQ=WEEKLY;BYDAY=MO,,é",
    ] {
        assert_eq!(Recurrence::parse(bad), None, "{bad:?}");
    }
    assert_eq!(Recurrence::parse(&format!("FREQ=DAILY;X-A={}", "a".repeat(2000))), None);
    assert_eq!(Recurrence::parse("FREQ=DAILY;COUNT=99999999").and_then(|r| r.count), Some(20_000));
}

#[test]
fn weekdays_and_months() {
    assert_eq!(Weekday::of_day(0), Weekday::Th);
    assert_eq!(Weekday::of_day(days_from_civil(2026, 11, 24)), Weekday::Tu);
    assert_eq!(nth_weekday(2026, 3, 2, Weekday::Su), Some(days_from_civil(2026, 3, 8)));
    assert_eq!(nth_weekday(2026, 1, -1, Weekday::Fr), Some(days_from_civil(2026, 1, 30)));
    assert_eq!(nth_weekday(2026, 2, 5, Weekday::Mo), None);
    assert_eq!((recurrence::month_len(2024, 2), recurrence::month_len(2026, 2), recurrence::month_len(2026, 12)), (29, 28, 31));
}

/// A weekly series repeats at 09:00 in Toronto, before and after summer time starts.
#[test]
fn a_series_keeps_its_wall_clock_time() {
    let mut e = ev("w", at(2026, 3, 2, 14, 0), 3600);
    e.time_zone = "America/Toronto".into();
    e.recurrence = Recurrence::parse("FREQ=WEEKLY;BYDAY=MO");
    let got = starts(&e, at(2026, 3, 1, 0, 0), at(2026, 4, 1, 0, 0), &Toronto);
    assert_eq!(got, [at(2026, 3, 2, 14, 0), at(2026, 3, 9, 13, 0), at(2026, 3, 16, 13, 0), at(2026, 3, 23, 13, 0), at(2026, 3, 30, 13, 0)]);
    // In UTC, it stays at 14:00 UTC.
    e.time_zone = "UTC".into();
    assert_eq!(starts(&e, at(2026, 3, 1, 0, 0), at(2026, 3, 10, 0, 0), &Toronto), [at(2026, 3, 2, 14, 0), at(2026, 3, 9, 14, 0)]);
    assert_eq!(utc_time(&Toronto, "America/Toronto", at(2026, 11, 25, 9, 0)), at(2026, 11, 25, 14, 0));
    assert_eq!(local_time(&Toronto, "America/Toronto", at(2026, 7, 1, 13, 0)), at(2026, 7, 1, 9, 0));
    // A zone nobody knows is the viewer's.
    assert_eq!(local_time(&Toronto, "Mars/Olympus", at(2026, 7, 1, 13, 0)), at(2026, 7, 1, 9, 0));
}

#[test]
fn monthly_and_yearly_rules() {
    let mut e = ev("m", at(2026, 1, 30, 10, 0), 3600);
    e.time_zone = "UTC".into();
    e.recurrence = Recurrence::parse("FREQ=MONTHLY;BYDAY=-1FR;COUNT=3");
    assert_eq!(starts(&e, at(2026, 1, 1, 0, 0), at(2027, 1, 1, 0, 0), &UtcOnly), [at(2026, 1, 30, 10, 0), at(2026, 2, 27, 10, 0), at(2026, 3, 27, 10, 0)]);
    // A birthday on the 29th of February comes in leap years only.
    let mut b = ev("b", at(2024, 2, 29, 0, 0), 86_400);
    b.all_day = true;
    b.recurrence = Recurrence::parse("FREQ=YEARLY");
    assert_eq!(starts(&b, at(2024, 1, 1, 0, 0), at(2029, 1, 1, 0, 0), &Toronto), [at(2024, 2, 29, 0, 0), at(2028, 2, 29, 0, 0)]);
    // The second Tuesday, every other month, until a date.
    let mut t = ev("t", at(2026, 1, 13, 15, 0), 3600);
    t.recurrence = Recurrence::parse("FREQ=MONTHLY;INTERVAL=2;BYDAY=2TU;UNTIL=20260601");
    assert_eq!(starts(&t, at(2026, 1, 1, 0, 0), at(2027, 1, 1, 0, 0), &UtcOnly), [at(2026, 1, 13, 15, 0), at(2026, 3, 10, 15, 0), at(2026, 5, 12, 15, 0)]);
    // The last weekday of the month.
    let mut w = ev("lw", at(2026, 1, 30, 12, 0), 60);
    w.recurrence = Recurrence::parse("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=2");
    assert_eq!(starts(&w, at(2026, 1, 1, 0, 0), at(2027, 1, 1, 0, 0), &UtcOnly), [at(2026, 1, 30, 12, 0), at(2026, 2, 27, 12, 0)]);
}

/// Occurrences taken out, moved and cancelled; a series with an exception of its own.
#[test]
fn occurrences_with_exceptions() {
    let start = at(2026, 11, 2, 9, 0);
    let mut series = ev("d", start, 1800);
    series.recurrence = Recurrence::parse("FREQ=DAILY;COUNT=5");
    series.exdates = vec![start + 86_400];
    let mut moved = ev("d", start + 3 * 86_400 + 3600, 1800);
    moved.recurrence_id = Some(start + 3 * 86_400);
    let mut gone = ev("d", start + 4 * 86_400, 1800);
    gone.recurrence_id = Some(start + 4 * 86_400);
    gone.status = EventStatus::Cancelled;
    let mut other = ev("d", start + 2 * 86_400, 600);
    other.calendar = "elsewhere".into();
    let list = vec![series, moved, gone, other];
    let got: Vec<(usize, i64, i64)> = occurrences(&list, start - 86_400, start + 10 * 86_400, &UtcOnly).iter().map(|o| (o.event, o.start, o.end)).collect();
    assert_eq!(
        got,
        [
            (0, start, start + 1800),
            (0, start + 2 * 86_400, start + 2 * 86_400 + 1800),
            (3, start + 2 * 86_400, start + 2 * 86_400 + 600),
            (1, start + 3 * 86_400 + 3600, start + 3 * 86_400 + 5400),
        ]
    );
    // Only what overlaps the window.
    let got = occurrences(&list, start + 2 * 86_400 + 1000, start + 3 * 86_400, &UtcOnly);
    assert_eq!(got.iter().map(|o| o.start).collect::<Vec<_>>(), [start + 2 * 86_400]);
    assert!(occurrences(&list, start, start, &UtcOnly).is_empty());
}

/// Google's cancelled occurrence names its series by the series' id, without its UID.
#[test]
fn an_occurrence_can_name_its_series_by_id() {
    let start = at(2026, 11, 2, 14, 0);
    let mut series = ev("ser1@google.com", start, 3600);
    series.remote = Remote::new("ser1", "");
    series.recurrence = Recurrence::parse("FREQ=WEEKLY;COUNT=3");
    let mut gone = ev("ser1_20261109T140000Z", start + 7 * 86_400, 0);
    gone.series = Some("ser1".into());
    gone.recurrence_id = Some(start + 7 * 86_400);
    gone.status = EventStatus::Cancelled;
    let got: Vec<i64> = occurrences(&[series, gone], start, start + 30 * 86_400, &UtcOnly).iter().map(|o| o.start).collect();
    assert_eq!(got, [start, start + 14 * 86_400]);
}

/// Expansion stops: a series without an end gives at most so many occurrences; a rule that never
/// matches ends.
#[test]
fn expansion_is_capped() {
    let mut e = ev("forever", 0, 60);
    e.recurrence = Recurrence::parse("FREQ=DAILY");
    let got = occurrences(std::slice::from_ref(&e), at(2000, 1, 1, 0, 0), at(2100, 1, 1, 0, 0), &UtcOnly);
    assert_eq!(got.len(), MAX_OCCURRENCES);
    assert_eq!(got.first().map(|o| o.start), Some(at(2000, 1, 1, 0, 0)));
    let mut never = ev("never", at(2026, 2, 1, 9, 0), 60);
    never.recurrence = Recurrence::parse("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30");
    assert!(starts(&never, 0, i64::MAX, &UtcOnly).is_empty());
    let mut huge = ev("huge", i64::MAX - 100, 60);
    huge.recurrence = Recurrence::parse("FREQ=YEARLY;INTERVAL=10000");
    assert!(starts(&huge, 0, i64::MAX, &UtcOnly).len() <= 1);
}

#[test]
fn busy_time_from_every_calendar() {
    let h = |hour: i64, min: i64| at(2026, 11, 24, hour, min);
    let mut a = ev("a", h(9, 0), 3600);
    a.calendar = "work".into();
    let mut b = ev("b", h(9, 30), 5400);
    b.busy = BusyStatus::Tentative;
    let mut c = ev("c", h(10, 30), 5400);
    c.busy = BusyStatus::OutOfOffice;
    let mut d = ev("d", h(13, 0), 3600);
    d.busy = BusyStatus::Free;
    let mut e = ev("e", h(15, 0), 3600);
    e.my_response = Some(Response::Declined);
    let mut f = ev("f", h(16, 0), 3600);
    f.status = EventStatus::Cancelled;
    let blocks = busy_blocks(&[a, b, c, d, e, f], h(0, 0), h(23, 0), &UtcOnly);
    assert_eq!(
        blocks,
        [
            BusyBlock { start: h(9, 0), end: h(10, 0), status: BusyStatus::Busy },
            BusyBlock { start: h(10, 0), end: h(10, 30), status: BusyStatus::Tentative },
            BusyBlock { start: h(10, 30), end: h(12, 0), status: BusyStatus::OutOfOffice },
        ]
    );
    assert_eq!(conflicts(&blocks, h(11, 30), h(13, 0)).len(), 1);
    assert!(conflicts(&blocks, h(12, 0), h(13, 0)).is_empty());
    // An all-day event takes the viewer's day.
    let mut holiday = ev("h", at(2026, 12, 25, 0, 0), 86_400);
    holiday.all_day = true;
    let blocks = busy_blocks(&[holiday], at(2026, 12, 20, 0, 0), at(2026, 12, 30, 0, 0), &Toronto);
    assert_eq!(blocks, [BusyBlock { start: at(2026, 12, 25, 5, 0), end: at(2026, 12, 26, 5, 0), status: BusyStatus::Busy }]);
}

#[test]
fn text_addresses_and_links_are_checked() {
    assert_eq!(clean_line("  a\u{200f}b\u{0}c\u{1b}[31m  ", 100), "ab c [31m");
    assert_eq!(clean_line(&"é".repeat(50), 10), "é".repeat(10));
    assert_eq!(clean_text("a\r\nb\rc\u{7}\td  \n", 100), "a\nb\nc\td");
    assert_eq!(clean_text(&"x\n".repeat(100), 5), "x\nx\nx");
    assert!(address_ok("zoe@studio.example") && !address_ok("zoe@studio") && !address_ok("a b@x.example") && !address_ok("<a@x.example>"));
    assert_eq!(clean_address(" MAILTO:zoe@studio.example ").as_deref(), Some("zoe@studio.example"));
    assert_eq!(clean_address("mailto:"), None);
    assert!(web_link("https://meet.google.com/abc-defg-hij").is_some());
    for bad in ["http://x.example/", "https://user@evil.example/", "javascript:alert(1)", "https://x.example/a b", "https://localhost/", "https://x.example/\u{202e}"] {
        assert_eq!(web_link(bad), None, "{bad:?}");
    }
    assert_eq!(provider_of_link("https://meet.google.com/abc"), MeetingProvider::Meet);
    assert_eq!(provider_of_link("https://teams.microsoft.com/l/meetup-join/x"), MeetingProvider::Teams);
    assert_eq!(provider_of_link("https://teams.live.com/meet/9"), MeetingProvider::Teams);
    assert_eq!(provider_of_link("https://teams.microsoft.com.evil.example/x"), MeetingProvider::Link);
    let m = find_meeting_link("Join: https://US02WEB.zoom.us/j/123?pwd=x. See https://teams.microsoft.com/l/x").unwrap();
    assert_eq!((m.provider, m.url.as_str()), (MeetingProvider::Link, "https://US02WEB.zoom.us/j/123?pwd=x"));
    assert_eq!(find_meeting_link("See https://example.com/page"), None);
    assert_eq!(parse_hex_color("#1A2b3C"), Some(Rgb(0x1a, 0x2b, 0x3c)));
    assert_eq!(parse_hex_color("1a2b3c"), None);
}

#[test]
fn dates_as_providers_write_them() {
    let day = at(2026, 11, 23, 0, 0);
    for (text, t) in [
        ("2026-11-23T14:05:00Z", day + 50_700),
        ("2026-11-23T14:05:00.1234567Z", day + 50_700),
        ("2026-11-23T14:05:00", day + 50_700),
        ("2026-11-23T09:05:00-05:00", day + 50_700),
        ("2026-11-23T15:35:00+0130", day + 50_700),
    ] {
        assert_eq!(parse_date_time(text), Some(t), "{text}");
    }
    for bad in ["", "2026-11-23", "2026-02-30T00:00:00Z", "2026-11-23T24:00:00Z", "2026-11-23T14:05:00.Z", "2026-11-23T14:05:00+5", "２０２６-11-23T14:05:00Z"] {
        assert_eq!(parse_date_time(bad), None, "{bad}");
    }
    assert_eq!(parse_date("2026-11-23"), Some(day / 86_400));
    assert_eq!(parse_date("2026-11-31"), None);
    assert_eq!(date_time_text(day + 50_700), "2026-11-23T14:05:00");
    assert_eq!(date_text(day / 86_400), "2026-11-23");
    assert_eq!((nearest_midnight(day + 5 * 3600), nearest_midnight(day - 3600)), (day, day));
}

#[test]
fn zone_names() {
    assert_eq!(iana_of_windows("eastern standard time"), Some("America/New_York"));
    assert_eq!(iana_of_windows("Romance Standard Time"), Some("Europe/Paris"));
    assert_eq!(iana_of_windows("Nowhere Standard Time"), None);
    assert_eq!(windows_of_iana("America/New_York"), Some("Eastern Standard Time"));
    assert!(is_utc_name("UTC") && is_utc_name("tzone://Microsoft/Utc") && !is_utc_name("GMT Standard Time") && !is_utc_name(""));
}

#[test]
fn events_say_what_they_are() {
    let mut e = ev("x", 100, 50);
    assert!(!e.is_meeting() && e.takes_time() && e.duration() == 50);
    e.attendees.push(Attendee::new("Zoe@Studio.example", "Zoe"));
    assert!(e.is_meeting() && e.attendee("zoe@studio.example").is_some());
    e.end = 0;
    assert_eq!(e.duration(), 0);
    assert_eq!(Response::from_partstat("tentative"), Response::Tentative);
    assert_eq!(Response::from_partstat("x-other"), Response::NeedsAction);
    let c = Contact { emails: vec!["a@b.example".into()], given_name: " Ann ".into(), ..Contact::default() };
    assert_eq!(c.display(), "Ann");
    assert!(Remote::default().is_local() && !Remote::new("1", "").is_local());
}
