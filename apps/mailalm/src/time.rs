//! Dates as the user reads them, in the computer's time zone.

use std::sync::OnceLock;

use chrono::{DateTime, Offset, TimeZone, Timelike};
use mailalm_model::query::civil_from_days;

/// The system's time zone (IANA name, read once), or UTC when it cannot be known.
fn zone() -> Option<chrono_tz::Tz> {
    static ZONE: OnceLock<Option<chrono_tz::Tz>> = OnceLock::new();
    *ZONE.get_or_init(|| iana_time_zone::get_timezone().ok().and_then(|name| name.parse::<chrono_tz::Tz>().ok()))
}

/// Seconds to add to a UTC time for the local time at that moment.
pub fn offset(utc: i64) -> i64 {
    let Some(tz) = zone() else { return 0 };
    let Some(dt) = DateTime::from_timestamp(utc, 0) else { return 0 };
    i64::from(tz.offset_from_utc_datetime(&dt.naive_utc()).fix().local_minus_utc())
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// The local day number (days since 1970-01-01) of a UTC time.
pub fn local_day(utc: i64) -> i64 {
    (utc + offset(utc)).div_euclid(86_400)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

fn hm(utc: i64) -> String {
    let local = utc + offset(utc);
    match DateTime::from_timestamp(local, 0) {
        Some(d) => format!("{:02}:{:02}", d.hour(), d.minute()),
        None => String::new(),
    }
}

fn month(m: i64) -> &'static str {
    usize::try_from(m - 1).ok().and_then(|i| MONTHS.get(i)).copied().unwrap_or("")
}

fn weekday(day: i64) -> &'static str {
    usize::try_from(day.rem_euclid(7)).ok().and_then(|i| DAYS.get(i)).copied().unwrap_or("")
}

/// For the message list: the time today, the weekday this week, the date otherwise.
pub fn short(utc: i64) -> String {
    if utc <= 0 {
        return String::new();
    }
    let today = local_day(now());
    let day = local_day(utc);
    let (y, m, d) = civil_from_days(day);
    let (ty, _, _) = civil_from_days(today);
    if day == today {
        hm(utc)
    } else if today - day < 7 && day < today {
        format!("{} {}", weekday(day), hm(utc))
    } else if y == ty {
        format!("{} {d}", month(m))
    } else {
        format!("{y}-{m:02}-{d:02}")
    }
}

/// For the reading pane: "Fri 2026-11-20 14:30".
pub fn long(utc: i64) -> String {
    if utc <= 0 {
        return String::new();
    }
    let day = local_day(utc);
    let (y, m, d) = civil_from_days(day);
    format!("{} {y}-{m:02}-{d:02} {}", weekday(day), hm(utc))
}

/// The system's zone name (IANA), or "UTC" when it cannot be known.
pub fn zone_name() -> String {
    zone().map_or_else(|| "UTC".to_owned(), |z| z.name().to_owned())
}

/// The wall-clock time here at a UTC instant, as seconds since 1970 (as if it were UTC).
pub fn local(utc: i64) -> i64 {
    utc.saturating_add(offset(utc))
}

/// The UTC instant of a wall-clock time here (seconds since 1970, as if UTC).
pub fn utc_of_local(local: i64) -> i64 {
    mailalm_model::pim::utc_time(&crate::zones::TzRules, "", local)
}

/// The UTC instant of a local day's midnight.
pub fn day_start(day: i64) -> i64 {
    utc_of_local(day.saturating_mul(86_400))
}

/// 0 for Monday to 6 for Sunday.
pub fn weekday_index(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

const LONG_MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const LONG_DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const SHORT_DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn month_name(m: i64) -> &'static str {
    usize::try_from(m - 1).ok().and_then(|i| LONG_MONTHS.get(i)).copied().unwrap_or("")
}

pub fn month_short(m: i64) -> &'static str {
    month(m)
}

pub fn day_name(day: i64) -> &'static str {
    usize::try_from(weekday_index(day)).ok().and_then(|i| LONG_DAYS.get(i)).copied().unwrap_or("")
}

pub fn day_short(day: i64) -> &'static str {
    usize::try_from(weekday_index(day)).ok().and_then(|i| SHORT_DAYS.get(i)).copied().unwrap_or("")
}

/// A day as `2026-11-25`.
pub fn date(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The time of day here, `14:05`.
pub fn clock(utc: i64) -> String {
    let s = local(utc).rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

/// An event's time as the calendar shows it: "Wed 2026-11-25 14:00 – 15:00", over several days
/// "Wed 2026-11-25 14:00 – Thu 2026-11-26 10:00"; an all-day event's dates ("Wed 2026-11-25, all
/// day", "Wed 2026-11-25 – Fri 2026-11-27").
pub fn span(start: i64, end: i64, all_day: bool) -> String {
    if all_day {
        let first = start.div_euclid(86_400);
        let last = (end.div_euclid(86_400) - 1).max(first);
        return if first == last {
            format!("{} {}, all day", day_short(first), date(first))
        } else {
            format!("{} {} – {} {}", day_short(first), date(first), day_short(last), date(last))
        };
    }
    let (a, b) = (local_day(start), local_day(end.saturating_sub(1).max(start)));
    if a == b {
        format!("{} {} {} – {}", day_short(a), date(a), clock(start), clock(end))
    } else {
        format!("{} {} {} – {} {} {}", day_short(a), date(a), clock(start), day_short(b), date(b), clock(end))
    }
}

/// `HH:MM` (or `H:MM`) as seconds into the day.
pub fn parse_clock(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (i64, i64) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 3600 + m * 60)
}

/// The groups of the message list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    ThisMonth,
    Older,
}

impl Group {
    pub fn of(utc: i64) -> Self {
        let today = local_day(now());
        let day = local_day(utc);
        let ago = today - day;
        // Monday starts the week: 1970-01-01 was a Thursday (day 0), so Monday is (day + 3) % 7 == 0.
        let since_monday = (today + 3).rem_euclid(7);
        let (ty, tm, _) = civil_from_days(today);
        let (y, m, _) = civil_from_days(day);
        if ago <= 0 {
            Group::Today
        } else if ago == 1 {
            Group::Yesterday
        } else if ago <= since_monday {
            Group::ThisWeek
        } else if ago <= since_monday + 7 {
            Group::LastWeek
        } else if y == ty && m == tm {
            Group::ThisMonth
        } else {
            Group::Older
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Today => "TODAY",
            Group::Yesterday => "YESTERDAY",
            Group::ThisWeek => "EARLIER THIS WEEK",
            Group::LastWeek => "LAST WEEK",
            Group::ThisMonth => "EARLIER THIS MONTH",
            Group::Older => "OLDER",
        }
    }
}
