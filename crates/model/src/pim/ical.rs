// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/model/src/pim/ical.rs (content lines: unfolding, splitting, unescaping, escaping, folding; DATE and DATE-TIME values; durations) and crates/model/src/pim/ical/zones.rs (a VTIMEZONE's offsets). MIT OR Apache-2.0.
// Changed: reads meeting messages into MailALM's `Event` (no tasks, notes, files, kept unknown lines or calendar subscriptions); every text cleaned and capped, components nested at most 16 deep, 500 events, 1000 dates; zones through the caller's `ZoneRules`, Windows names through a table, the file's own VTIMEZONE last; writes the iTIP messages MailALM sends (REQUEST, CANCEL, REPLY, COUNTER, RFC 5546) rather than whole calendars, its VTIMEZONE made from the caller's zone rules.

//! iCalendar (RFC 5545) as meeting messages carry it (iTIP, RFC 5546; by mail, iMIP, RFC 6047):
//! reading an invitation, an answer or a cancellation, and writing the ones MailALM sends for an
//! IMAP account.
//!
//! What a message holds is hostile: at most [`MAX_INPUT`] bytes are read, components nest at most
//! [`MAX_DEPTH`] deep, at most [`MAX_EVENTS`] events and [`super::MAX_ATTENDEES`] guests each are
//! kept, every text is cleaned and capped, every address checked. A time in a named zone is read
//! in it: the caller's zone rules when they know the name (a Windows name through
//! [`super::iana_of_windows`]), else the file's own `VTIMEZONE`, else the viewer's zone.
//!
//! Written messages put a timed event in UTC, but a series in its zone (with a `VTIMEZONE` made
//! from the caller's rules), so it repeats at its wall-clock time for every guest.

use super::{
    Attendee, AttendeeKind, BusyStatus, Event, EventStatus, MAX_ATTENDEES, MAX_LINE, MAX_REMINDER_MINUTES, MAX_REMINDERS, MAX_TEXT, OnlineMeeting, Recurrence, Response, Until, Weekday, ZoneRules, clean_address, clean_line, clean_text, find_meeting_link, iana_of_windows,
    is_utc_name, utc_time,
};
use crate::query::{civil_from_days, days_from_civil};

/// The longest calendar text read.
pub const MAX_INPUT: usize = 4 << 20;
/// The most content lines read, and the longest one (unfolded).
const MAX_LINES: usize = 100_000;
const MAX_LINE_BYTES: usize = 256 << 10;
/// The most parameters kept on one line.
const MAX_PARAMS: usize = 32;
/// The most events read from one text.
pub const MAX_EVENTS: usize = 500;
/// How deep components may nest.
pub const MAX_DEPTH: usize = 16;
/// The most excluded dates kept on one event.
const MAX_DATES: usize = 1000;
/// The most zones read from one text, and the most parts (standard, daylight) each.
const MAX_ZONES: usize = 50;
const MAX_ZONE_PARTS: usize = 100;
/// The longest UID and zone name kept.
const MAX_UID: usize = 1000;
const MAX_ZONE_NAME: usize = 100;

/// What a meeting message asks (`METHOD`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Publish,
    /// An invitation, or a change to one.
    Request,
    /// A guest's answer.
    Reply,
    Add,
    /// The meeting, or some of its occurrences, called off.
    Cancel,
    Refresh,
    /// A guest proposes another time.
    Counter,
    DeclineCounter,
}

impl Method {
    fn parse(s: &str) -> Option<Method> {
        Some(match s.trim().to_ascii_uppercase().as_str() {
            "PUBLISH" => Method::Publish,
            "REQUEST" => Method::Request,
            "REPLY" => Method::Reply,
            "ADD" => Method::Add,
            "CANCEL" => Method::Cancel,
            "REFRESH" => Method::Refresh,
            "COUNTER" => Method::Counter,
            "DECLINECOUNTER" => Method::DeclineCounter,
            _ => return None,
        })
    }

    /// Its name in a calendar text, and in the `method` parameter of a `text/calendar` part.
    pub fn code(self) -> &'static str {
        match self {
            Method::Publish => "PUBLISH",
            Method::Request => "REQUEST",
            Method::Reply => "REPLY",
            Method::Add => "ADD",
            Method::Cancel => "CANCEL",
            Method::Refresh => "REFRESH",
            Method::Counter => "COUNTER",
            Method::DeclineCounter => "DECLINECOUNTER",
        }
    }
}

/// A meeting message's calendar: what it asks, its events (a series and its changed occurrences,
/// or one event), and the sender's comment (an answer's `COMMENT`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Invitation {
    /// `None` when it says none, or one RFC 5546 doesn't have.
    pub method: Option<Method>,
    pub events: Vec<Event>,
    pub comment: String,
}

// ----- content lines -----

/// A content line: name (upper case), parameters (names upper case, values unquoted), value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Line {
    fn param(&self, n: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
    }
}

/// Unfold (RFC 5545 §3.1) and split into content lines, within the caps.
fn lines(text: &str) -> Vec<Line> {
    // At a character's start (a cut inside one would read nothing).
    let mut end = MAX_INPUT.min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let text = text.get(..end).unwrap_or_default();
    let mut unfolded: Vec<String> = Vec::new();
    for l in text.split('\n') {
        let l = l.strip_suffix('\r').unwrap_or(l);
        match (l.strip_prefix(' ').or_else(|| l.strip_prefix('\t')), unfolded.last_mut()) {
            (Some(rest), Some(prev)) => {
                if prev.len().saturating_add(rest.len()) <= MAX_LINE_BYTES {
                    prev.push_str(rest);
                }
            }
            _ => {
                if unfolded.len() >= MAX_LINES {
                    break;
                }
                unfolded.push(l.chars().take(MAX_LINE_BYTES).collect());
            }
        }
    }
    unfolded.iter().filter_map(|raw| parse_line(raw)).collect()
}

fn parse_line(raw: &str) -> Option<Line> {
    // The value starts at the first ':' outside double quotes.
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in raw.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = raw.get(..colon)?;
    let value = raw.get(colon + 1..)?.to_string();
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in head.chars() {
        match c {
            '"' => q = !q,
            ';' if !q => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let mut it = parts.into_iter();
    let name = it.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = it
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string())))
        .take(MAX_PARAMS)
        .collect();
    Some(Line { name, params, value })
}

/// A TEXT value with its escapes undone.
pub fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// A TEXT value escaped for writing.
pub fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace(['\n', '\r'], "\\n")
}

/// Whether a character may stand in a content line: any but the control characters, of which
/// only HTAB is allowed (RFC 5545 §3.1).
fn allowed(c: char) -> bool {
    c == '\t' || !c.is_control()
}

/// Fold a content line at 75 octets (RFC 5545 §3.1), on character boundaries, a character it
/// may not hold ([`allowed`]) left out.
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars().filter(|c| allowed(*c)) {
        let n = c.len_utf8();
        if width + n > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += n;
    }
    out.push_str("\r\n");
}

// ----- dates and times -----

/// `YYYYMMDD` as days since 1970-01-01.
fn date_days(s: &str) -> Option<i64> {
    if !s.get(..8)?.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let m: i64 = s.get(4..6)?.parse().ok()?;
    let d: i64 = s.get(6..8)?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=9999).contains(&y) && d >= 1 && d <= super::recurrence::month_len(y, m)).then(|| days_from_civil(y, m, d))
}

/// A DATE or DATE-TIME value: (local or UTC seconds, a date, UTC).
pub(crate) fn date_time(v: &str) -> Option<(i64, bool, bool)> {
    let v = v.trim();
    let days = date_days(v)?;
    let Some(t) = v.get(8..).filter(|r| !r.is_empty()) else { return Some((days * 86_400, true, false)) };
    let t = t.strip_prefix(['T', 't'])?;
    let utc = t.ends_with(['Z', 'z']);
    let digits = t.trim_end_matches(['Z', 'z']);
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i64 = digits.get(0..2)?.parse().ok()?;
    let mi: i64 = digits.get(2..4)?.parse().ok()?;
    let s: i64 = digits.get(4..6)?.parse().ok()?;
    if h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some((days * 86_400 + h * 3600 + mi * 60 + s, false, utc))
}

/// A `DURATION` (`P1W`, `PT1H30M`, `-P1D`) in seconds.
pub fn duration(v: &str) -> Option<i64> {
    let v = v.trim();
    let (sign, v) = match v.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix(['P', 'p'])?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut time = false;
    for c in v.chars().take(64) {
        match c {
            '0'..='9' => num.push(c),
            'T' | 't' => time = true,
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total = total.saturating_add(n.saturating_mul(match (unit.to_ascii_uppercase(), time) {
                    ('W', _) => 604_800,
                    ('D', _) => 86_400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                }));
            }
        }
    }
    num.is_empty().then_some(sign * total)
}

/// `YYYYMMDDTHHMMSSZ`.
fn utc_stamp(t: i64) -> String {
    format!("{}Z", local_stamp(t))
}

/// `YYYYMMDDTHHMMSS`.
fn local_stamp(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{}T{:02}{:02}{:02}", day_stamp(t.div_euclid(86_400)), s / 3600, s % 3600 / 60, s % 60)
}

/// `YYYYMMDD`.
fn day_stamp(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}{m:02}{d:02}")
}

/// A `UTC-OFFSET` (`-0500`, `+0530`, `+013045`) in seconds.
fn utc_offset(v: &str) -> Option<i64> {
    let v = v.trim();
    let sign = match v.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = v.get(1..)?;
    if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let h: i64 = digits.get(0..2)?.parse().ok()?;
    let m: i64 = digits.get(2..4)?.parse().ok()?;
    let s: i64 = digits.get(4..).filter(|x| !x.is_empty()).map_or(Some(0), |x| x.parse().ok())?;
    (h <= 18 && m <= 59 && s <= 59).then_some(sign * (h * 3600 + m * 60 + s))
}

/// `+HHMM` for an offset in seconds.
fn offset_text(o: i64) -> String {
    let sign = if o < 0 { '-' } else { '+' };
    let a = o.abs();
    format!("{sign}{:02}{:02}", a / 3600, a % 3600 / 60)
}

// ----- zones a text defines -----

/// One part of a `VTIMEZONE` (its standard or daylight time): the offset it brings, the one it
/// takes over from, and when it starts: once (`since`, wall clock), or every year from then in a
/// month on an nth weekday at `time`.
#[derive(Clone, Debug)]
struct ZonePart {
    to: i64,
    from: Option<i64>,
    yearly: Option<(i64, i64, Weekday)>,
    time: i64,
    since: i64,
}

/// A zone as a `VTIMEZONE` defines it.
#[derive(Clone, Debug, Default)]
struct VZone {
    parts: Vec<ZonePart>,
}

impl VZone {
    fn read(block: &[Line]) -> VZone {
        let mut z = VZone::default();
        let mut part: Option<ZonePart> = None;
        for l in block {
            let kind = matches!(l.value.trim().to_ascii_uppercase().as_str(), "STANDARD" | "DAYLIGHT");
            match l.name.as_str() {
                "BEGIN" if kind => part = Some(ZonePart { to: 0, from: None, yearly: None, time: 0, since: i64::MIN }),
                "END" if kind => {
                    if z.parts.len() < MAX_ZONE_PARTS {
                        z.parts.extend(part.take());
                    }
                }
                "TZOFFSETTO" => {
                    if let (Some(p), Some(o)) = (part.as_mut(), utc_offset(&l.value)) {
                        p.to = o;
                    }
                }
                "TZOFFSETFROM" => {
                    if let (Some(p), Some(o)) = (part.as_mut(), utc_offset(&l.value)) {
                        p.from = Some(o);
                    }
                }
                "DTSTART" => {
                    if let (Some(p), Some((t, _, _))) = (part.as_mut(), date_time(&l.value)) {
                        p.since = t;
                        p.time = t.rem_euclid(86_400);
                    }
                }
                "RRULE" => {
                    if let (Some(p), Some(r)) = (part.as_mut(), Recurrence::parse(&l.value)) {
                        if let (Some(m), Some((nth, wd))) = (r.by_month.first(), r.by_day.first()) {
                            p.yearly = Some((i64::from(*m), if *nth == 0 { 1 } else { i64::from(*nth) }, *wd));
                        }
                    }
                }
                _ => {}
            }
        }
        z
    }

    /// The offset in force at a UTC instant: the part that started last before it.
    fn offset_at(&self, utc: i64) -> Option<i64> {
        let mut best: Option<(i64, i64)> = None;
        let (y, _, _) = civil_from_days(utc.div_euclid(86_400));
        for p in &self.parts {
            // Its starts as wall-clock times, then in UTC (under the offset it takes over from).
            let starts: Vec<i64> = match p.yearly {
                Some((month, nth, wd)) => [y - 1, y]
                    .iter()
                    .filter_map(|&yy| super::recurrence::nth_weekday(yy, month, nth, wd).map(|d| d.saturating_mul(86_400).saturating_add(p.time)))
                    .filter(|t| *t >= p.since)
                    .collect(),
                None if p.since > i64::MIN => vec![p.since],
                None => Vec::new(),
            };
            for t in starts.into_iter().map(|t| t.saturating_sub(p.from.unwrap_or(p.to))).filter(|t| *t <= utc) {
                if best.is_none_or(|(bt, _)| t > bt) {
                    best = Some((t, p.to));
                }
            }
        }
        best.map(|(_, o)| o).or_else(|| self.parts.first().map(|p| p.from.unwrap_or(p.to)))
    }
}

/// The zones a text defines, by their `TZID`.
#[derive(Default)]
struct FileZones {
    zones: Vec<(String, VZone)>,
}

impl FileZones {
    fn read(all: &[Line]) -> FileZones {
        let mut zones = Vec::new();
        let mut i = 0;
        while let Some(l) = all.get(i) {
            i += 1;
            if l.name != "BEGIN" || !l.value.trim().eq_ignore_ascii_case("VTIMEZONE") {
                continue;
            }
            let from = i;
            while let Some(m) = all.get(i) {
                i += 1;
                if m.name == "END" && m.value.trim().eq_ignore_ascii_case("VTIMEZONE") {
                    break;
                }
            }
            let block = all.get(from..i).unwrap_or_default();
            // Its own TZID, the first line that names one.
            let id = block.iter().find(|l| l.name == "TZID").map(|l| zone_name(&l.value)).unwrap_or_default();
            if !id.is_empty() && zones.len() < MAX_ZONES {
                zones.push((id, VZone::read(block)));
            }
        }
        FileZones { zones }
    }

    fn offset(&self, zone: &str, utc: i64) -> Option<i64> {
        self.zones.iter().find(|(id, v)| id == zone && !v.parts.is_empty()).and_then(|(_, v)| v.offset_at(utc))
    }
}

/// The caller's rules (a Windows name through the table), then the text's own zones.
struct Rules<'a> {
    outer: &'a dyn ZoneRules,
    file: &'a FileZones,
}

impl ZoneRules for Rules<'_> {
    fn offset(&self, zone: &str, utc: i64) -> Option<i64> {
        if zone.is_empty() {
            return self.outer.offset("", utc);
        }
        self.outer.offset(zone, utc).or_else(|| iana_of_windows(zone).and_then(|z| self.outer.offset(z, utc))).or_else(|| self.file.offset(zone, utc))
    }
}

/// A zone name as kept: cleaned, at most [`MAX_ZONE_NAME`] characters, its quotes taken off.
fn zone_name(v: &str) -> String {
    clean_line(v.trim().trim_matches('"'), MAX_ZONE_NAME)
}

/// The name an event keeps for the zone its times were in: the IANA name for a Windows one the
/// caller's rules know under it, else the name as written.
fn kept_zone(zone: &str, outer: &dyn ZoneRules, at: i64) -> String {
    if outer.offset(zone, at).is_none()
        && let Some(iana) = iana_of_windows(zone)
        && outer.offset(iana, at).is_some()
    {
        return iana.to_string();
    }
    zone.to_string()
}

/// A DATE or DATE-TIME property: (UTC seconds, a date, the zone it was in). A local time is in its
/// `TZID`, else the viewer's zone.
fn time_of(l: &Line, value: &str, rules: &Rules<'_>) -> Option<(i64, bool, String)> {
    let date_only = l.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE"));
    let (t, all_day, utc) = date_time(value)?;
    if all_day || date_only {
        return Some((t.div_euclid(86_400).saturating_mul(86_400), true, String::new()));
    }
    if utc {
        return Some((t, false, "UTC".into()));
    }
    let zone = l.param("TZID").map(zone_name).unwrap_or_default();
    let at = utc_time(rules, &zone, t);
    Some((at, false, kept_zone(&zone, rules.outer, at)))
}

// ----- reading -----

/// What an alarm's `TRIGGER` says: so long before (or after) the start or end, or an instant.
#[derive(Clone, Copy, Debug, Default)]
struct Trigger {
    relative: Option<i64>,
    from_end: bool,
    at: Option<i64>,
}

/// An event being read.
#[derive(Default)]
struct Reading {
    e: Event,
    has_start: bool,
    end: Option<i64>,
    duration: Option<i64>,
    status: Option<EventStatus>,
    transparent: bool,
    show_as: Option<BusyStatus>,
    online: Option<OnlineMeeting>,
    triggers: Vec<Trigger>,
    comment: String,
}

impl Reading {
    fn line(&mut self, l: &Line, rules: &Rules<'_>) {
        let text = |max: usize| clean_line(&unescape(&l.value), max);
        match l.name.as_str() {
            "UID" => self.e.uid = l.value.chars().filter(|c| allowed(*c) && *c != '\t').take(MAX_UID).collect::<String>().trim().to_string(),
            "SUMMARY" => self.e.title = text(MAX_LINE),
            "LOCATION" => self.e.location = text(MAX_LINE),
            "DESCRIPTION" => self.e.description = clean_text(&unescape(&l.value), MAX_TEXT),
            "COMMENT" if self.comment.is_empty() => self.comment = clean_text(&unescape(&l.value), MAX_LINE),
            "DTSTART" => {
                if let Some((t, all_day, zone)) = time_of(l, &l.value, rules) {
                    (self.e.start, self.e.all_day, self.e.time_zone, self.has_start) = (t, all_day, zone, true);
                }
            }
            "DTEND" => self.end = time_of(l, &l.value, rules).map(|(t, _, _)| t),
            "DURATION" => self.duration = duration(&l.value),
            "RRULE" if self.e.recurrence.is_none() => self.e.recurrence = Recurrence::parse(&l.value),
            "EXDATE" => {
                for v in l.value.split(',').take(MAX_DATES) {
                    if self.e.exdates.len() >= MAX_DATES {
                        break;
                    }
                    if let Some((t, _, _)) = time_of(l, v, rules) {
                        self.e.exdates.push(t);
                    }
                }
            }
            "RECURRENCE-ID" => self.e.recurrence_id = time_of(l, &l.value, rules).map(|(t, _, _)| t),
            "ORGANIZER" => self.e.organizer = person(l),
            "ATTENDEE" => {
                if self.e.attendees.len() < MAX_ATTENDEES
                    && let Some(a) = person(l)
                    && !self.e.attendees.iter().any(|x| x.is(&a.email))
                {
                    self.e.attendees.push(a);
                }
            }
            "STATUS" => {
                self.status = match l.value.trim().to_ascii_uppercase().as_str() {
                    "TENTATIVE" => Some(EventStatus::Tentative),
                    "CANCELLED" => Some(EventStatus::Cancelled),
                    "CONFIRMED" => Some(EventStatus::Confirmed),
                    _ => None,
                }
            }
            "SEQUENCE" => self.e.sequence = l.value.trim().parse::<u32>().unwrap_or(0).min(1_000_000),
            "TRANSP" => self.transparent = l.value.trim().eq_ignore_ascii_case("TRANSPARENT"),
            "X-MICROSOFT-CDO-BUSYSTATUS" => {
                self.show_as = match l.value.trim().to_ascii_uppercase().as_str() {
                    "FREE" => Some(BusyStatus::Free),
                    "TENTATIVE" => Some(BusyStatus::Tentative),
                    "BUSY" => Some(BusyStatus::Busy),
                    "OOF" => Some(BusyStatus::OutOfOffice),
                    _ => None,
                }
            }
            "X-MICROSOFT-SKYPETEAMSMEETINGURL" | "X-GOOGLE-CONFERENCE" | "CONFERENCE" if self.online.is_none() => {
                self.online = OnlineMeeting::at(&l.value);
            }
            _ => {}
        }
    }

    /// A line of one of its alarms.
    fn alarm_line(&mut self, l: &Line) {
        if l.name != "TRIGGER" || self.triggers.len() >= MAX_REMINDERS {
            return;
        }
        let absolute = l.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"));
        let trigger = if absolute {
            date_time(&l.value).filter(|(_, d, utc)| !*d && *utc).map(|(t, _, _)| Trigger { at: Some(t), ..Trigger::default() })
        } else {
            duration(&l.value).map(|d| Trigger { relative: Some(d), from_end: l.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END")), at: None })
        };
        self.triggers.extend(trigger);
    }

    fn finish(mut self) -> Option<(Event, String)> {
        if self.e.uid.is_empty() || !self.has_start {
            return None;
        }
        let e = &mut self.e;
        let one_day = if e.all_day { 86_400 } else { 0 };
        e.end = match (self.end, self.duration) {
            (Some(end), _) => end,
            (None, Some(d)) => e.start.saturating_add(d.max(0)),
            (None, None) => e.start.saturating_add(one_day),
        };
        if e.end < e.start.saturating_add(one_day) {
            e.end = e.start.saturating_add(one_day);
        }
        e.status = self.status.unwrap_or_default();
        e.busy = self.show_as.unwrap_or(if self.transparent { BusyStatus::Free } else { BusyStatus::Busy });
        e.online = self.online.or_else(|| find_meeting_link(&e.location)).or_else(|| find_meeting_link(&e.description));
        for t in &self.triggers {
            let before = match (t.at, t.relative) {
                (Some(at), _) => e.start.saturating_sub(at),
                (None, Some(d)) => {
                    let base = if t.from_end { e.end } else { e.start };
                    e.start.saturating_sub(base.saturating_add(d))
                }
                (None, None) => continue,
            };
            let Ok(minutes) = u32::try_from(before.max(0) / 60) else { continue };
            if minutes <= MAX_REMINDER_MINUTES && !e.reminders.contains(&minutes) {
                e.reminders.push(minutes);
            }
        }
        e.reminders.sort_unstable();
        Some((self.e, self.comment))
    }
}

/// `ORGANIZER` or `ATTENDEE`: its address (`mailto:`), name, answer, role and kind; `None`
/// without an address.
fn person(l: &Line) -> Option<Attendee> {
    let email = clean_address(&l.value)?;
    let kind = match (l.param("CUTYPE").map(str::to_ascii_uppercase).as_deref(), l.param("ROLE").map(str::to_ascii_uppercase).as_deref()) {
        (Some("ROOM" | "RESOURCE"), _) => AttendeeKind::Resource,
        (_, Some("OPT-PARTICIPANT" | "NON-PARTICIPANT")) => AttendeeKind::Optional,
        _ => AttendeeKind::Required,
    };
    Some(Attendee {
        email,
        name: l.param("CN").map(|n| clean_line(n, 300)).unwrap_or_default(),
        response: l.param("PARTSTAT").map(Response::from_partstat).unwrap_or_default(),
        kind,
    })
}

/// Read a meeting message's calendar (a `text/calendar` part, or an `.ics` file). Times in a zone
/// are read with `zones` (the app's zone rules), else the text's own definitions; floating ones
/// are the viewer's. Events without a UID or a start are left out. A cancellation's events are
/// cancelled when they don't say otherwise.
pub fn parse(text: &str, zones: &dyn ZoneRules) -> Invitation {
    let all = lines(text);
    let file = FileZones::read(&all);
    let rules = Rules { outer: zones, file: &file };
    let mut out = Invitation::default();
    // The components open around the line.
    let mut open: Vec<String> = Vec::new();
    let mut event: Option<Reading> = None;
    for l in &all {
        match l.name.as_str() {
            "BEGIN" => {
                let name = l.value.trim().to_ascii_uppercase();
                if open.len() >= MAX_DEPTH {
                    break;
                }
                // An event inside the calendar (or a bare one); an event inside another
                // component, or inside an event, is not one of the calendar's.
                if name == "VEVENT" && open.iter().all(|c| c == "VCALENDAR") && event.is_none() {
                    event = Some(Reading::default());
                }
                open.push(name);
            }
            "END" => {
                let name = l.value.trim().to_ascii_uppercase();
                // An END that ends nothing open is passed over.
                let Some(at) = open.iter().rposition(|c| *c == name) else { continue };
                open.truncate(at);
                if name == "VEVENT" && !open.iter().any(|c| c == "VEVENT") {
                    if let Some(done) = event.take().and_then(Reading::finish)
                        && out.events.len() < MAX_EVENTS
                    {
                        if out.comment.is_empty() {
                            out.comment = done.1;
                        }
                        out.events.push(done.0);
                    }
                }
            }
            _ => match (open.last().map(String::as_str), event.as_mut()) {
                (Some("VEVENT"), Some(r)) if open.iter().filter(|c| *c == "VEVENT").count() == 1 => r.line(l, &rules),
                (Some("VALARM"), Some(r)) if open.len() >= 2 && open.get(open.len() - 2).is_some_and(|c| c == "VEVENT") => r.alarm_line(l),
                (Some("VCALENDAR"), _) if l.name == "METHOD" => out.method = Method::parse(&l.value),
                _ => {}
            },
        }
    }
    if out.method == Some(Method::Cancel) {
        for e in &mut out.events {
            if e.status == EventStatus::Confirmed {
                e.status = EventStatus::Cancelled;
            }
        }
    }
    out
}

// ----- writing -----

/// A parameter value written as it may stand: no quotes or controls, quoted when it holds `;`,
/// `:` or `,`.
fn param_value(v: &str) -> String {
    let v: String = v.chars().filter(|c| !c.is_control() && *c != '"').take(300).collect();
    if v.contains([';', ':', ',']) { format!("\"{v}\"") } else { v }
}

/// `ORGANIZER` or `ATTENDEE` with its parameters; `None` when the address isn't one.
fn mailbox(prop: &str, a: &Attendee, partstat: Option<Response>, rsvp: bool) -> Option<String> {
    let email = clean_address(&a.email)?;
    let mut s = prop.to_string();
    if !a.name.trim().is_empty() {
        s.push_str(&format!(";CN={}", param_value(a.name.trim())));
    }
    if prop == "ATTENDEE" {
        let (cutype, role) = match a.kind {
            AttendeeKind::Required => ("INDIVIDUAL", "REQ-PARTICIPANT"),
            AttendeeKind::Optional => ("INDIVIDUAL", "OPT-PARTICIPANT"),
            AttendeeKind::Resource => ("RESOURCE", "NON-PARTICIPANT"),
        };
        s.push_str(&format!(";CUTYPE={cutype};ROLE={role}"));
    }
    if let Some(p) = partstat {
        s.push_str(&format!(";PARTSTAT={}", p.partstat()));
    }
    if rsvp {
        s.push_str(";RSVP=TRUE");
    }
    s.push_str(&format!(":mailto:{email}"));
    Some(s)
}

/// How a written event's times are put: in UTC, as dates, or as wall-clock times in a zone.
#[derive(Clone, Copy)]
enum Times<'a> {
    Utc,
    Dates,
    Zone(&'a str, &'a dyn ZoneRules),
}

impl Times<'_> {
    fn line(&self, prop: &str, t: i64) -> String {
        match self {
            Times::Utc => format!("{prop}:{}", utc_stamp(t)),
            Times::Dates => format!("{prop};VALUE=DATE:{}", day_stamp(t.div_euclid(86_400))),
            Times::Zone(zone, zones) => format!("{prop};TZID={}:{}", param_value(zone), local_stamp(super::local_time(*zones, zone, t))),
        }
    }
}

/// The zone a series is written in: its own (a Windows name as its IANA one), when the caller's
/// rules know it and it isn't UTC.
fn series_zone<'a>(e: &'a Event, zones: &dyn ZoneRules) -> Option<&'a str> {
    if e.all_day || e.recurrence.is_none() {
        return None;
    }
    let name = iana_of_windows(&e.time_zone).unwrap_or(e.time_zone.as_str());
    (!name.trim().is_empty() && !is_utc_name(name) && zones.offset(name, e.start).is_some()).then_some(name)
}

/// The rule as it may be written with these times: `UNTIL` a date with dates, else an instant
/// (RFC 5545 §3.3.10).
fn rule_for(r: &Recurrence, times: Times<'_>) -> Recurrence {
    let mut r = r.clone();
    r.until = r.until.map(|u| match (u, times) {
        (Until::At(t), Times::Dates) => Until::Date(t.div_euclid(86_400)),
        (Until::Local(t), Times::Dates) => Until::Date(t.div_euclid(86_400)),
        (u @ Until::Date(_), Times::Dates) => u,
        (u @ Until::At(_), _) => u,
        (Until::Local(t), Times::Zone(zone, zones)) => Until::At(utc_time(zones, zone, t)),
        (Until::Local(t), Times::Utc) => Until::At(t),
        (Until::Date(d), Times::Zone(zone, zones)) => Until::At(utc_time(zones, zone, d.saturating_mul(86_400).saturating_add(86_399))),
        (Until::Date(d), Times::Utc) => Until::At(d.saturating_mul(86_400).saturating_add(86_399)),
    });
    r
}

/// A `VTIMEZONE` for `zone` from the caller's rules, as it is in `year`: its changes between
/// standard and summer time found in the rules and written as the yearly rule they follow (on
/// the nth, or last, weekday of their month).
fn vtimezone(zone: &str, year: i64, zones: &dyn ZoneRules) -> Vec<String> {
    let off = |t: i64| zones.offset(zone, t).unwrap_or(0);
    let start = days_from_civil(year, 1, 1) * 86_400;
    let end = days_from_civil(year + 1, 1, 1) * 86_400;
    let mut changes: Vec<(i64, i64, i64)> = Vec::new();
    let (mut t, mut before) = (start, off(start));
    while t < end && changes.len() < 4 {
        let next = (t + 86_400).min(end);
        let after = off(next);
        if after != before {
            // The first second with the new offset.
            let (mut lo, mut hi) = (t, next);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if off(mid) == before { lo = mid } else { hi = mid }
            }
            changes.push((hi, before, after));
            before = after;
        }
        t = next;
    }
    let mut out = vec!["BEGIN:VTIMEZONE".to_string(), format!("TZID:{}", zone.chars().filter(|c| !c.is_control()).collect::<String>())];
    if changes.is_empty() {
        let o = off(start);
        out.extend(["BEGIN:STANDARD".into(), "DTSTART:19700101T000000".into(), format!("TZOFFSETFROM:{}", offset_text(o)), format!("TZOFFSETTO:{}", offset_text(o)), "END:STANDARD".into()]);
    }
    for (at, from, to) in changes {
        // The wall-clock time it happens at, under the offset it ends.
        let local = at + from;
        let day = local.div_euclid(86_400);
        let (_, m, d) = civil_from_days(day);
        let nth = if d + 7 > super::recurrence::month_len(year, m) { -1 } else { (d - 1) / 7 + 1 };
        let kind = if to > from { "DAYLIGHT" } else { "STANDARD" };
        out.push(format!("BEGIN:{kind}"));
        out.push(format!("DTSTART:{}", local_stamp(local)));
        out.push(format!("RRULE:FREQ=YEARLY;BYMONTH={m};BYDAY={nth}{}", Weekday::of_day(day).code()));
        out.push(format!("TZOFFSETFROM:{}", offset_text(from)));
        out.push(format!("TZOFFSETTO:{}", offset_text(to)));
        out.push(format!("END:{kind}"));
    }
    out.push("END:VTIMEZONE".into());
    out
}

/// A calendar text: its head, `METHOD`, these zone definitions and these events' lines, folded.
fn calendar(method: Method, zones_lines: &[String], events: &[Vec<String>]) -> String {
    let mut out = String::new();
    for l in ["BEGIN:VCALENDAR", "PRODID:-//MailALM//MailALM//EN", "VERSION:2.0", "CALSCALE:GREGORIAN"] {
        fold(l, &mut out);
    }
    fold(&format!("METHOD:{}", method.code()), &mut out);
    for l in zones_lines {
        fold(l, &mut out);
    }
    for e in events {
        for l in e {
            fold(l, &mut out);
        }
    }
    fold("END:VCALENDAR", &mut out);
    out
}

/// A UID as it is written back: as the organizer wrote it, but for line breaks and controls.
fn uid_line(uid: &str) -> Option<String> {
    let uid: String = uid.chars().filter(|c| allowed(*c) && *c != '\t').take(MAX_UID).collect();
    (!uid.trim().is_empty()).then(|| format!("UID:{}", uid.trim()))
}

/// The lines an invitation (or a cancellation) gives one event: everything the guests need.
fn invitation_lines(e: &Event, status: EventStatus, zones: &dyn ZoneRules, now: i64) -> Option<Vec<String>> {
    let organizer = e.organizer.as_ref()?;
    let mut v = vec!["BEGIN:VEVENT".to_string(), uid_line(&e.uid)?, format!("DTSTAMP:{}", utc_stamp(now)), format!("SEQUENCE:{}", e.sequence)];
    let times = match series_zone(e, zones) {
        _ if e.all_day => Times::Dates,
        Some(zone) => Times::Zone(zone, zones),
        None => Times::Utc,
    };
    v.push(times.line("DTSTART", e.start));
    v.push(times.line("DTEND", e.end.max(e.start)));
    if let Some(r) = e.recurrence_id {
        v.push(times.line("RECURRENCE-ID", r));
    }
    if let Some(r) = &e.recurrence
        && e.recurrence_id.is_none()
    {
        v.push(format!("RRULE:{}", rule_for(r, times).to_rrule()));
        for x in e.exdates.iter().take(MAX_DATES) {
            v.push(times.line("EXDATE", *x));
        }
    }
    v.push(format!("SUMMARY:{}", escape(&e.title)));
    if !e.location.is_empty() {
        v.push(format!("LOCATION:{}", escape(&e.location)));
    }
    let mut description = e.description.clone();
    if let Some(url) = e.online.as_ref().map(|o| o.url.as_str()).filter(|u| !u.is_empty()) {
        v.push(format!("CONFERENCE;VALUE=URI;FEATURE=AUDIO,VIDEO:{url}"));
        if !description.contains(url) {
            let sep = if description.trim().is_empty() { "" } else { "\n\n" };
            description = format!("{}{sep}Join: {url}", description.trim_end());
        }
    }
    if !description.is_empty() {
        v.push(format!("DESCRIPTION:{}", escape(&description)));
    }
    v.push(mailbox("ORGANIZER", organizer, None, false)?);
    for a in e.attendees.iter().take(MAX_ATTENDEES).filter(|a| !a.is(&organizer.email)) {
        v.extend(mailbox("ATTENDEE", a, Some(Response::NeedsAction), true));
    }
    v.push(format!(
        "STATUS:{}",
        match status {
            EventStatus::Confirmed => "CONFIRMED",
            EventStatus::Tentative => "TENTATIVE",
            EventStatus::Cancelled => "CANCELLED",
        }
    ));
    let (transp, cdo) = match e.busy {
        BusyStatus::Free => ("TRANSPARENT", "FREE"),
        BusyStatus::Tentative => ("OPAQUE", "TENTATIVE"),
        BusyStatus::Busy => ("OPAQUE", "BUSY"),
        BusyStatus::OutOfOffice => ("OPAQUE", "OOF"),
    };
    v.push(format!("TRANSP:{transp}"));
    v.push(format!("X-MICROSOFT-CDO-BUSYSTATUS:{cdo}"));
    v.push("END:VEVENT".into());
    Some(v)
}

/// The zone definitions the events written in a zone need, each once.
fn zone_definitions(events: &[Event], zones: &dyn ZoneRules) -> Vec<String> {
    let mut done: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for e in events {
        let Some(zone) = series_zone(e, zones) else { continue };
        if done.contains(&zone) || done.len() >= MAX_ZONES {
            continue;
        }
        done.push(zone);
        out.extend(vtimezone(zone, civil_from_days(e.start.div_euclid(86_400)).0, zones));
    }
    out
}

/// An invitation (`METHOD:REQUEST`) for these events (a meeting, or a series and its changed
/// occurrences), from their organizer to their guests, stamped `now`. The organizer raises
/// `sequence` for a change guests must answer again. `None` when an event has no organizer, no
/// UID, or an organizer address that isn't one.
pub fn request(events: &[Event], zones: &dyn ZoneRules, now: i64) -> Option<String> {
    if events.is_empty() {
        return None;
    }
    let lines: Option<Vec<Vec<String>>> = events.iter().take(MAX_EVENTS).map(|e| invitation_lines(e, e.status, zones, now)).collect();
    Some(calendar(Method::Request, &zone_definitions(events, zones), &lines?))
}

/// A cancellation (`METHOD:CANCEL`) of a meeting (or of one occurrence, with its
/// `recurrence_id`), from its organizer to its guests. `None` as for [`request`].
pub fn cancel(event: &Event, zones: &dyn ZoneRules, now: i64) -> Option<String> {
    let lines = invitation_lines(event, EventStatus::Cancelled, zones, now)?;
    Some(calendar(Method::Cancel, &zone_definitions(std::slice::from_ref(event), zones), &[lines]))
}

/// A guest's answer to an invitation, from `me` to its organizer: `REPLY` with `answer`
/// (accepted, tentative, declined); with `proposal` (a new start and end), a `COUNTER` proposing
/// that time, the guest's answer meanwhile in it. `comment` goes to the organizer. `None` when
/// the event has no organizer or no UID, an address isn't one, or the answer is no answer.
pub fn reply(event: &Event, me: &Attendee, answer: Response, proposal: Option<(i64, i64)>, comment: &str, now: i64) -> Option<String> {
    if answer == Response::NeedsAction {
        return None;
    }
    let organizer = event.organizer.as_ref()?;
    let method = if proposal.is_some() { Method::Counter } else { Method::Reply };
    let times = if event.all_day && proposal.is_none() { Times::Dates } else { Times::Utc };
    let mut v = vec!["BEGIN:VEVENT".to_string(), uid_line(&event.uid)?, format!("DTSTAMP:{}", utc_stamp(now)), format!("SEQUENCE:{}", event.sequence)];
    if let Some(r) = event.recurrence_id {
        v.push(if event.all_day { Times::Dates.line("RECURRENCE-ID", r) } else { Times::Utc.line("RECURRENCE-ID", r) });
    }
    let (start, end) = proposal.unwrap_or((event.start, event.end));
    v.push(times.line("DTSTART", start));
    v.push(times.line("DTEND", end.max(start)));
    v.push(format!("SUMMARY:{}", escape(&event.title)));
    if method == Method::Counter && !event.location.is_empty() {
        v.push(format!("LOCATION:{}", escape(&event.location)));
    }
    v.push(mailbox("ORGANIZER", organizer, None, false)?);
    let mine = Attendee { response: answer, ..me.clone() };
    v.push(mailbox("ATTENDEE", &mine, Some(answer), false)?);
    let comment = clean_text(comment, MAX_LINE);
    if !comment.is_empty() {
        v.push(format!("COMMENT:{}", escape(&comment)));
    }
    v.push("END:VEVENT".into());
    Some(calendar(method, &[], &[v]))
}

#[cfg(test)]
mod tests;
