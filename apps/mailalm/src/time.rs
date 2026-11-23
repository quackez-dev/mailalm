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
