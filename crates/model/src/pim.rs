//! Calendars, events, tasks and contacts (PLAN.md, sections 7, 14: A1, A2, A3, K1): what the
//! network layer reads from Microsoft Graph and Google, what the cache keeps, and what MailALM
//! writes in meeting messages for IMAP accounts (iMIP, [`ical`]).
//!
//! Times are UTC instants in Unix seconds. A timed event also keeps the zone its times were given
//! in (an IANA or a Windows name, as the provider gave it), so a series repeats at its wall-clock
//! time there ([`recurrence`]). An all-day event's start and end are the UTC midnights of its
//! first day and of the day after its last: dates, the same wherever the user is.
//!
//! Everything here may have come from a server or from a mail (an invitation): the readers clean
//! text ([`clean_line`], [`clean_text`]), check addresses and links, and cap what they keep.

use serde::{Deserialize, Serialize};

use crate::Rgb;
use crate::query::{civil_from_days, days_from_civil};

pub mod ical;
pub mod recurrence;
mod zones;

pub use recurrence::{Freq, MAX_OCCURRENCES, Occurrence, Recurrence, Until, UtcOnly, Weekday, ZoneRules, local_time, occurrences, starts, utc_time};
pub use zones::{iana_of_windows, is_utc_name, windows_of_iana};

/// The calendar, task list or address book kept on this computer only (an IMAP account's).
pub const LOCAL: &str = "local";

/// Longest title, place or name kept, in characters.
pub const MAX_LINE: usize = 1000;
/// Longest description or task notes kept, in characters.
pub const MAX_TEXT: usize = 32_000;
/// Most guests kept on one event.
pub const MAX_ATTENDEES: usize = 500;
/// Most reminders kept on one event.
pub const MAX_REMINDERS: usize = 10;
/// Longest reminder, in minutes (four weeks: Outlook's and Google's own limit).
pub const MAX_REMINDER_MINUTES: u32 = 40_320;
/// Longest online meeting link kept.
pub const MAX_LINK: usize = 2048;

/// Where an item lives on the provider's side: its id there, and the version the provider last
/// gave it (Graph's `changeKey`, Google's `etag`). Both empty for an item kept here only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Remote {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

impl Remote {
    pub fn new(id: &str, version: &str) -> Remote {
        Remote { id: id.to_string(), version: version.to_string() }
    }

    /// Kept here only: the provider has never had it.
    pub fn is_local(&self) -> bool {
        self.id.is_empty()
    }
}

/// Which service makes an online meeting's link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeetingProvider {
    /// Google Meet (a Gmail account's calendar makes it).
    Meet,
    /// Microsoft Teams (a Microsoft account's calendar makes it, when the account offers it).
    Teams,
    /// A link the user gives (a Zoom or Jitsi room…).
    Link,
}

/// An event's online meeting. An empty `url` with [`MeetingProvider::Meet`] or
/// [`MeetingProvider::Teams`] asks the provider to make one (when an event is created or
/// changed), or says it is being made (Google answers "pending" for a while).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineMeeting {
    pub provider: MeetingProvider,
    #[serde(default)]
    pub url: String,
}

impl OnlineMeeting {
    /// A meeting the provider is to make.
    pub fn request(provider: MeetingProvider) -> OnlineMeeting {
        OnlineMeeting { provider, url: String::new() }
    }

    /// A meeting at this link, its provider told by its host; `None` when it isn't a web link
    /// ([`web_link`]).
    pub fn at(url: &str) -> Option<OnlineMeeting> {
        let url = web_link(url)?;
        Some(OnlineMeeting { provider: provider_of_link(&url), url })
    }
}

/// A calendar of an account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    /// MailALM's key for it: the provider's id, or [`LOCAL`].
    pub id: String,
    #[serde(default)]
    pub remote: Remote,
    pub name: String,
    /// The colour the provider gives it, if any (the account's colour otherwise).
    #[serde(default)]
    pub color: Option<Rgb>,
    /// Events can be added and changed in it.
    #[serde(default)]
    pub can_edit: bool,
    /// The account's main calendar: new events and accepted invitations go there.
    #[serde(default)]
    pub primary: bool,
    /// The online meetings the provider makes for events of this calendar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meetings: Vec<MeetingProvider>,
    /// The reminders its events have unless they say otherwise (Google's `defaultReminders`), in
    /// minutes before the start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_reminders: Vec<u32>,
}

/// A guest's answer to an invitation (iCalendar's `PARTSTAT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Response {
    #[default]
    NeedsAction,
    Accepted,
    Tentative,
    Declined,
}

impl Response {
    /// iCalendar's name for it.
    pub fn partstat(self) -> &'static str {
        match self {
            Response::NeedsAction => "NEEDS-ACTION",
            Response::Accepted => "ACCEPTED",
            Response::Tentative => "TENTATIVE",
            Response::Declined => "DECLINED",
        }
    }

    /// From iCalendar's name (any case); anything else is no answer yet.
    pub fn from_partstat(s: &str) -> Response {
        match s.trim().to_ascii_uppercase().as_str() {
            "ACCEPTED" => Response::Accepted,
            "TENTATIVE" => Response::Tentative,
            "DECLINED" => Response::Declined,
            _ => Response::NeedsAction,
        }
    }
}

/// What a guest is to a meeting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttendeeKind {
    #[default]
    Required,
    Optional,
    /// A room or equipment.
    Resource,
}

/// Someone invited to an event (or its organizer).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attendee {
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub response: Response,
    #[serde(default)]
    pub kind: AttendeeKind,
}

impl Attendee {
    pub fn new(email: &str, name: &str) -> Attendee {
        Attendee { email: email.to_string(), name: name.to_string(), ..Attendee::default() }
    }

    /// Whether this is `address` (case ignored).
    pub fn is(&self, address: &str) -> bool {
        self.email.trim().eq_ignore_ascii_case(address.trim())
    }
}

/// How an event shows the user's time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BusyStatus {
    Free,
    Tentative,
    #[default]
    Busy,
    OutOfOffice,
}

impl BusyStatus {
    /// Which of two weighs more where they overlap: out of office, then busy, then tentative.
    fn weight(self) -> u8 {
        match self {
            BusyStatus::Free => 0,
            BusyStatus::Tentative => 1,
            BusyStatus::Busy => 2,
            BusyStatus::OutOfOffice => 3,
        }
    }
}

/// Whether an event takes place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventStatus {
    #[default]
    Confirmed,
    Tentative,
    Cancelled,
}

/// An event, a series (with its [`Recurrence`]), or one occurrence of a series (with its
/// `recurrence_id`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Its iCalendar `UID`: the same for the organizer and every guest, and for every occurrence
    /// of a series.
    pub uid: String,
    /// The calendar it is in ([`Calendar::id`]); empty for one read from an invitation.
    #[serde(default)]
    pub calendar: String,
    #[serde(default)]
    pub remote: Remote,
    #[serde(default)]
    pub title: String,
    /// UTC seconds; for an all-day event, the UTC midnight of its first day.
    pub start: i64,
    /// Exclusive; for an all-day event, the UTC midnight of the day after its last.
    pub end: i64,
    #[serde(default)]
    pub all_day: bool,
    /// The zone its times were given in, as given (`America/Toronto`, `Eastern Standard Time`,
    /// `UTC`); empty: not said (the viewer's own, for a time without a zone). A series repeats at
    /// its wall-clock time there.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time_zone: String,
    #[serde(default)]
    pub location: String,
    /// Plain text.
    #[serde(default)]
    pub description: String,
    /// How a series repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    /// Occurrences taken out of the series (their starts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exdates: Vec<i64>,
    /// For one occurrence of a series: the series' id with the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    /// For one occurrence of a series: when it was to start, as the series has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Attendee>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
    /// The user organizes it (or it is no meeting).
    #[serde(default)]
    pub is_organizer: bool,
    /// The user's own answer, when the user is a guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_response: Option<Response>,
    #[serde(default)]
    pub busy: BusyStatus,
    /// Reminders, in minutes before the start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reminders: Vec<u32>,
    /// The calendar's default reminders apply ([`Calendar::default_reminders`]).
    #[serde(default)]
    pub default_reminders: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<OnlineMeeting>,
    #[serde(default)]
    pub status: EventStatus,
    /// Its iCalendar `SEQUENCE`: raised by the organizer at each change guests must hear of.
    #[serde(default)]
    pub sequence: u32,
}

impl Event {
    /// Whether it is a meeting: it has an organizer or guests.
    pub fn is_meeting(&self) -> bool {
        self.organizer.is_some() || !self.attendees.is_empty()
    }

    /// The guest with this address.
    pub fn attendee(&self, address: &str) -> Option<&Attendee> {
        self.attendees.iter().find(|a| a.is(address))
    }

    /// How long it lasts, in seconds (never negative).
    pub fn duration(&self) -> i64 {
        self.end.saturating_sub(self.start).max(0)
    }

    /// Whether the user's time is taken: not free, not cancelled, not declined.
    pub fn takes_time(&self) -> bool {
        self.busy != BusyStatus::Free && self.status != EventStatus::Cancelled && self.my_response != Some(Response::Declined)
    }
}

/// A list of tasks (Microsoft To Do's lists, Google Tasks' task lists).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskList {
    /// MailALM's key for it: the provider's id, or [`LOCAL`].
    pub id: String,
    #[serde(default)]
    pub remote: Remote,
    pub name: String,
    /// The account's default list (To Do's "Tasks", Google's "My Tasks").
    #[serde(default)]
    pub default: bool,
}

/// A task.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The list it is in ([`TaskList::id`]).
    pub list: String,
    #[serde(default)]
    pub remote: Remote,
    pub title: String,
    /// Plain text.
    #[serde(default)]
    pub notes: String,
    /// The day it is due: the UTC midnight of that day (both providers keep a date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<i64>,
    /// When to remind (UTC seconds). Google Tasks has no reminders through its API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reminder: Option<i64>,
    #[serde(default)]
    pub completed: bool,
    /// When it was completed (UTC seconds), when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

/// A contact: what completing addresses and the contact list need.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    #[serde(default)]
    pub remote: Remote,
    /// The name as shown.
    pub name: String,
    #[serde(default)]
    pub given_name: String,
    #[serde(default)]
    pub family_name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    #[serde(default)]
    pub phones: Vec<String>,
    #[serde(default)]
    pub organization: String,
    #[serde(default)]
    pub job_title: String,
}

impl Contact {
    /// The name as shown: its name, else given and family names, else its first address.
    pub fn display(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.clone();
        }
        let both = format!("{} {}", self.given_name.trim(), self.family_name.trim());
        if !both.trim().is_empty() {
            return both.trim().to_string();
        }
        self.emails.first().cloned().unwrap_or_default()
    }
}

/// A stretch of the user's time taken, from every calendar shown together (PLAN.md, A3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusyBlock {
    pub start: i64,
    pub end: i64,
    pub status: BusyStatus,
}

/// The user's time taken in `[from, to)` by these events (any calendars, any accounts): every
/// occurrence that takes time ([`Event::takes_time`]), overlapping ones merged into one block
/// that has the weightier status (out of office, busy, tentative), in order. An all-day event
/// takes its days in the viewer's zone (`zones`' empty name).
pub fn busy_blocks(events: &[Event], from: i64, to: i64, zones: &dyn ZoneRules) -> Vec<BusyBlock> {
    let mut blocks: Vec<BusyBlock> = Vec::new();
    for o in occurrences(events, from, to, zones) {
        let Some(e) = events.get(o.event) else { continue };
        if !e.takes_time() {
            continue;
        }
        let (start, end) = if e.all_day { (utc_time(zones, "", o.start), utc_time(zones, "", o.end)) } else { (o.start, o.end) };
        let (start, end) = (start.max(from), end.min(to));
        if end > start {
            blocks.push(BusyBlock { start, end, status: e.busy });
        }
    }
    merge_blocks(blocks)
}

/// Blocks put in order, those that overlap or touch with the same status made one, and a lighter
/// block's part under a weightier one taken out.
pub fn merge_blocks(mut blocks: Vec<BusyBlock>) -> Vec<BusyBlock> {
    blocks.retain(|b| b.end > b.start);
    // The points where something starts or ends; between two, the weightiest status there.
    let mut cuts: Vec<i64> = blocks.iter().flat_map(|b| [b.start, b.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<BusyBlock> = Vec::new();
    for w in cuts.windows(2) {
        let [a, b] = [w.first().copied().unwrap_or(0), w.get(1).copied().unwrap_or(0)];
        let status = blocks.iter().filter(|x| x.start <= a && x.end >= b).map(|x| x.status).max_by_key(|s| s.weight());
        let Some(status) = status else { continue };
        match out.last_mut() {
            Some(last) if last.end == a && last.status == status => last.end = b,
            _ => out.push(BusyBlock { start: a, end: b, status }),
        }
    }
    out
}

/// The blocks that overlap `[start, end)`: what a meeting at that time would clash with.
pub fn conflicts(blocks: &[BusyBlock], start: i64, end: i64) -> Vec<BusyBlock> {
    blocks.iter().filter(|b| b.start < end && b.end > start).copied().collect()
}

// ----- text, addresses and links from servers and mail -----

/// Whether a character can change how the text around it reads without showing itself: the
/// direction marks and overrides, and zero-width characters.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{061c}')
}

/// One line of text as MailALM shows it: invisible characters dropped, controls (a line end, a
/// terminal escape) and runs of spaces made one space, trimmed, at most `max` characters.
pub fn clean_line(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    let mut n = 0;
    for c in s.chars().take(max.saturating_mul(4).saturating_add(64)) {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if is_invisible(c) {
            continue;
        }
        if n >= max {
            break;
        }
        if space {
            out.push(' ');
            n += 1;
            space = false;
            if n >= max {
                break;
            }
        }
        out.push(c);
        n += 1;
    }
    out
}

/// A text of several lines as MailALM shows it: line ends made `\n`, tabs kept, other controls
/// and invisible characters dropped, trailing space trimmed, at most `max` characters.
pub fn clean_text(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    let mut chars = s.chars().take(max.saturating_mul(4).saturating_add(64)).peekable();
    while let Some(c) = chars.next() {
        if n >= max {
            break;
        }
        match c {
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    out.push('\n');
                    n += 1;
                }
            }
            '\n' | '\t' => {
                out.push(c);
                n += 1;
            }
            c if c.is_control() || is_invisible(c) => {}
            c => {
                out.push(c);
                n += 1;
            }
        }
    }
    out.trim_end().to_string()
}

/// Whether `a` looks like an address: one `@` with something on each side, a dot in the domain,
/// no spaces, controls or brackets, at most 254 characters. The server has the last word.
pub fn address_ok(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && a.len() <= 254
        && !a.chars().any(|c| c.is_whitespace() || c.is_control() || is_invisible(c) || matches!(c, '<' | '>' | '(' | ')' | ',' | ';' | ':' | '"' | '[' | ']' | '\\'))
}

/// An address as kept: trimmed, a `mailto:` taken off; `None` when it isn't one ([`address_ok`]).
pub fn clean_address(a: &str) -> Option<String> {
    let a = a.trim();
    let a = match a.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("mailto:") => a.get(7..).unwrap_or("").trim(),
        _ => a,
    };
    address_ok(a).then(|| a.to_string())
}

/// A web link MailALM keeps to show (never follows by itself): `https://` with a host, no user
/// name before the host, no spaces, quotes or brackets, at most [`MAX_LINK`] characters.
pub fn web_link(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = s.get(..8).filter(|p| p.eq_ignore_ascii_case("https://")).and(s.get(8..))?;
    if s.len() > MAX_LINK || s.chars().any(|c| c.is_whitespace() || c.is_control() || is_invisible(c) || matches!(c, '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}')) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.split(':').next().unwrap_or("");
    let host_ok = !authority.contains('@')
        && host.contains('.')
        && host.len() <= 253
        && host.split('.').all(|l| !l.is_empty() && l.len() <= 63)
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    host_ok.then(|| s.to_string())
}

/// The host of a web link, lower case.
fn host_of(link: &str) -> String {
    let rest = link.split_once("://").map_or(link, |(_, r)| r);
    rest.split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase()
}

/// Whether `host` is `domain` or under it.
fn under(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|h| h.ends_with('.'))
}

/// Which service a meeting link is for, by its host.
pub fn provider_of_link(link: &str) -> MeetingProvider {
    let host = host_of(link);
    if host == "meet.google.com" {
        MeetingProvider::Meet
    } else if under(&host, "teams.microsoft.com") || under(&host, "teams.live.com") {
        MeetingProvider::Teams
    } else {
        MeetingProvider::Link
    }
}

/// The first link to a Google Meet, Teams or Zoom meeting in some text (a place, notes), as
/// invitations from other calendars carry them.
pub fn find_meeting_link(text: &str) -> Option<OnlineMeeting> {
    let mut rest = text;
    for _ in 0..200 {
        let at = find_ci(rest, "https://")?;
        let tail = rest.get(at..).unwrap_or("");
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '"' | '<' | '>' | ')' | ']')).unwrap_or(tail.len());
        let candidate = tail.get(..end).unwrap_or("").trim_end_matches(['.', ',', ';']);
        if let Some(m) = OnlineMeeting::at(candidate) {
            let host = host_of(&m.url);
            if m.provider != MeetingProvider::Link || under(&host, "zoom.us") {
                return Some(m);
            }
        }
        rest = tail.get(8..).unwrap_or("");
    }
    None
}

/// Where `needle` (ASCII) is in `hay`, ignoring ASCII case.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let n = needle.len();
    hay.char_indices().map(|(i, _)| i).find(|&i| hay.get(i..i + n).is_some_and(|s| s.eq_ignore_ascii_case(needle)))
}

/// A colour written `#rrggbb` (any case).
pub fn parse_hex_color(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    Some(Rgb(byte(0)?, byte(2)?, byte(4)?))
}

// ----- dates as the providers write them -----

/// `YYYY-MM-DD` as a day count since 1970-01-01.
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to).filter(|x| x.bytes().all(|c| c.is_ascii_digit()))?.parse().ok() };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) || d > recurrence::month_len(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// An ISO 8601 / RFC 3339 date and time (`2026-11-23T14:05:00Z`, with fractions of a second or
/// not, `Z`, an offset, or nothing, read as UTC) in Unix seconds.
pub fn parse_date_time(t: &str) -> Option<i64> {
    let t = t.trim();
    let b = t.as_bytes();
    let day = parse_date(t.get(..10)?)?;
    let sep = |at: usize, ok: &[u8]| b.get(at).is_some_and(|c| ok.contains(c));
    if !(sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { t.get(from..to).filter(|s| s.bytes().all(|c| c.is_ascii_digit()))?.parse().ok() };
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut rest = t.get(19..)?;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 12 {
            return None;
        }
        rest = frac.get(digits..)?;
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return None,
            };
            let hm = rest.get(1..)?.replace(':', "");
            if hm.len() != 4 || !hm.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let (oh, om): (i64, i64) = (hm.get(..2)?.parse().ok()?, hm.get(2..)?.parse().ok()?);
            if oh > 23 || om > 59 {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
    };
    Some(day * 86_400 + h * 3600 + mi * 60 + s - offset)
}

/// A day count as `YYYY-MM-DD`.
pub fn date_text(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Seconds since 1970 as `YYYY-MM-DDTHH:MM:SS` (no zone: the reader says which).
pub fn date_time_text(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{}T{:02}:{:02}:{:02}", date_text(t.div_euclid(86_400)), s / 3600, s % 3600 / 60, s % 60)
}

/// The UTC midnight of the day an instant is in, rounded to the nearest midnight (an all-day
/// event a provider gave at a zone's midnight in UTC: `2026-11-25T05:00:00Z` is the 25th).
pub fn nearest_midnight(t: i64) -> i64 {
    t.saturating_add(43_200).div_euclid(86_400).saturating_mul(86_400)
}

#[cfg(test)]
mod tests;
