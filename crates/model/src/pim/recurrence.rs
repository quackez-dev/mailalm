// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/model/src/pim/ical/recurrence.rs. MIT OR Apache-2.0.
// Changed: a structured rule (MailALM's `Recurrence`, serialized for the cache) instead of the RRULE text; daily, weekly, monthly and yearly rules only (no hourly, minutely or secondly ones, no BYHOUR, BYMINUTE, BYSECOND: such a rule is not read); no RDATE; time zones through a `ZoneRules` the caller gives (the model keeps no zone database); overrides matched by RECURRENCE-ID and the series' UID or provider id; no work counters.

//! How a series repeats: a subset of iCalendar's `RRULE` (RFC 5545 §3.3.10), read, written, and
//! expanded over a window.
//!
//! The subset is what calendars use day to day and what Microsoft Graph's and Google's
//! recurrences say: `FREQ` daily, weekly, monthly or yearly, with `INTERVAL`, `COUNT`, `UNTIL`,
//! `BYDAY` (with ordinals for monthly and yearly rules), `BYMONTHDAY`, `BYMONTH`, `BYSETPOS` and
//! `WKST`. A rule with anything else (hourly rules, `BYHOUR`, `BYWEEKNO`…) is not read: its event
//! shows once, at its start.
//!
//! A timed series repeats at its wall-clock time in its zone (09:00 stays 09:00 when summer time
//! starts), each occurrence turned into UTC on its own date, through the [`ZoneRules`] the caller
//! gives (the app's zone database). An all-day series repeats on dates. Expansion is capped: at
//! most [`MAX_OCCURRENCES`] occurrences per event and window, [`MAX_STEPS`] candidates looked at.

use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use super::{Event, EventStatus, is_utc_name};
use crate::query::{civil_from_days, days_from_civil};

/// The most occurrences produced for one event in one expansion.
pub const MAX_OCCURRENCES: usize = 2000;
/// The most starts a rule is followed to: its `COUNT` is capped there.
const MAX_STARTS: usize = MAX_OCCURRENCES * 10;
/// The most candidate days looked at for one event (a rule that rarely matches).
const MAX_STEPS: usize = 50_000;
/// The longest rule read.
const MAX_RULE: usize = 1024;
/// The largest offset from UTC taken from zone rules (UTC−12 to UTC+14 exist; a little more).
const MAX_OFFSET: i64 = 18 * 3600;

/// The time zone rules the caller knows: the app's zone database.
pub trait ZoneRules {
    /// Seconds east of UTC in `zone` at the UTC instant `utc`; `None` for a zone these rules don't
    /// know. The empty name is the viewer's own zone (where a time without a zone is).
    fn offset(&self, zone: &str, utc: i64) -> Option<i64>;
}

/// Rules that know UTC alone (and take the viewer to be there): for tests, and for times that
/// are all UTC anyway.
#[derive(Clone, Copy, Debug, Default)]
pub struct UtcOnly;

impl ZoneRules for UtcOnly {
    fn offset(&self, zone: &str, _utc: i64) -> Option<i64> {
        (zone.trim().is_empty() || is_utc_name(zone)).then_some(0)
    }
}

/// The offset in `zone` at `utc`: the rules', else the viewer's, else none; at most
/// [`MAX_OFFSET`] either way.
fn offset_at(zones: &dyn ZoneRules, zone: &str, utc: i64) -> i64 {
    zones.offset(zone, utc).or_else(|| zones.offset("", utc)).unwrap_or(0).clamp(-MAX_OFFSET, MAX_OFFSET)
}

/// The wall-clock time in `zone` at the UTC instant `utc`, as seconds since 1970 (as if it were
/// UTC). A zone the rules don't know is taken as the viewer's.
pub fn local_time(zones: &dyn ZoneRules, zone: &str, utc: i64) -> i64 {
    utc.saturating_add(offset_at(zones, zone, utc))
}

/// The UTC instant of the wall-clock time `local` in `zone`. A time that summer time skips
/// comes out an hour on; one it repeats, the first of the two.
pub fn utc_time(zones: &dyn ZoneRules, zone: &str, local: i64) -> i64 {
    let first = local.saturating_sub(offset_at(zones, zone, local));
    let second = local.saturating_sub(offset_at(zones, zone, first));
    // Of the two guesses, the one that reads back as `local`, the earlier when both do.
    let back = |t: i64| local_time(zones, zone, t) == local;
    match (back(first), back(second)) {
        (true, true) => first.min(second),
        (true, false) => first,
        _ => second,
    }
}

/// A day of the week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Weekday {
    Mo,
    Tu,
    We,
    Th,
    Fr,
    Sa,
    Su,
}

impl Weekday {
    pub const ALL: [Weekday; 7] = [Weekday::Mo, Weekday::Tu, Weekday::We, Weekday::Th, Weekday::Fr, Weekday::Sa, Weekday::Su];

    /// 0 for Monday to 6 for Sunday.
    pub fn index(self) -> i64 {
        match self {
            Weekday::Mo => 0,
            Weekday::Tu => 1,
            Weekday::We => 2,
            Weekday::Th => 3,
            Weekday::Fr => 4,
            Weekday::Sa => 5,
            Weekday::Su => 6,
        }
    }

    /// The day of index `i` (taken modulo 7).
    pub fn from_index(i: i64) -> Weekday {
        usize::try_from(i.rem_euclid(7)).ok().and_then(|i| Weekday::ALL.get(i)).copied().unwrap_or(Weekday::Mo)
    }

    /// The weekday of a day count since 1970-01-01 (a Thursday).
    pub fn of_day(day: i64) -> Weekday {
        Weekday::from_index(day.saturating_add(3))
    }

    /// iCalendar's two letters (`MO`).
    pub fn code(self) -> &'static str {
        match self {
            Weekday::Mo => "MO",
            Weekday::Tu => "TU",
            Weekday::We => "WE",
            Weekday::Th => "TH",
            Weekday::Fr => "FR",
            Weekday::Sa => "SA",
            Weekday::Su => "SU",
        }
    }

    /// From iCalendar's two letters, any case.
    pub fn from_code(s: &str) -> Option<Weekday> {
        Weekday::ALL.iter().find(|w| w.code().eq_ignore_ascii_case(s.trim())).copied()
    }
}

/// How often a rule repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    fn code(self) -> &'static str {
        match self {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    }
}

/// When a rule stops (its `UNTIL`, inclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Until {
    /// An instant (UTC seconds): `UNTIL=20261231T235959Z`.
    At(i64),
    /// A wall-clock time where the event is (seconds, as if UTC): `UNTIL=20261231T090000`.
    Local(i64),
    /// The end of a day (a day count since 1970-01-01): `UNTIL=20261231`.
    Date(i64),
}

/// A recurrence rule (the subset this module reads).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recurrence {
    pub freq: Freq,
    /// Every so many periods (at least 1).
    #[serde(default = "one")]
    pub interval: u32,
    /// How many occurrences in all (from the first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Until>,
    /// `BYDAY`: (ordinal, weekday); ordinal 0 is every such weekday, 1 the first, -1 the last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_day: Vec<(i8, Weekday)>,
    /// `BYMONTHDAY`: 1 to 31, or -1 (the last) to -31.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_month_day: Vec<i8>,
    /// `BYMONTH`: 1 to 12.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_month: Vec<u8>,
    /// `BYSETPOS`: which of each period's days count (1 the first, -1 the last).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub by_set_pos: Vec<i16>,
    /// `WKST`: the day weeks start on (which weeks an every-other-week rule takes).
    #[serde(default = "monday")]
    pub week_start: Weekday,
}

fn one() -> u32 {
    1
}

fn monday() -> Weekday {
    Weekday::Mo
}

impl Recurrence {
    /// A rule repeating every period, without an end.
    pub fn new(freq: Freq) -> Recurrence {
        Recurrence {
            freq,
            interval: 1,
            count: None,
            until: None,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            week_start: Weekday::Mo,
        }
    }

    /// An `RRULE` value (`FREQ=WEEKLY;BYDAY=MO,WE`, an `RRULE:` before it taken off). `None` for
    /// a rule outside the subset, or one that isn't a rule.
    pub fn parse(rule: &str) -> Option<Recurrence> {
        let rule = rule.trim();
        if rule.len() > MAX_RULE {
            return None;
        }
        let rule = match rule.get(..6) {
            Some(p) if p.eq_ignore_ascii_case("RRULE:") => rule.get(6..).unwrap_or(""),
            _ => rule,
        };
        let mut freq = None;
        let mut r = Recurrence::new(Freq::Daily);
        for part in rule.split(';').filter(|p| !p.trim().is_empty()) {
            let (k, v) = part.split_once('=')?;
            let v = v.trim();
            match k.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v.to_ascii_uppercase().as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None,
                    })
                }
                "INTERVAL" => r.interval = v.parse::<u32>().ok().filter(|n| *n >= 1)?.min(10_000),
                "COUNT" => r.count = Some(v.parse::<u32>().ok().filter(|n| *n >= 1)?.min(u32::try_from(MAX_STARTS).unwrap_or(u32::MAX))),
                "UNTIL" => r.until = Some(until_of(v)?),
                "BYDAY" => r.by_day = list(v, by_day_of)?,
                "BYMONTHDAY" => r.by_month_day = list(v, |d| d.parse::<i8>().ok().filter(|d| *d != 0 && (-31..=31).contains(d)))?,
                "BYMONTH" => r.by_month = list(v, |m| m.parse::<u8>().ok().filter(|m| (1..=12).contains(m)))?,
                "BYSETPOS" => r.by_set_pos = list(v, |p| p.parse::<i16>().ok().filter(|p| *p != 0 && (-366..=366).contains(p)))?,
                "WKST" => r.week_start = Weekday::from_code(v)?,
                // A part with no meaning here (an `X-` one) is passed over; one that would
                // change the occurrences is not.
                k if k.starts_with("X-") => {}
                _ => return None,
            }
        }
        r.freq = freq?;
        Some(r)
    }

    /// The rule as an `RRULE` value.
    pub fn to_rrule(&self) -> String {
        let mut out = format!("FREQ={}", self.freq.code());
        if self.interval > 1 {
            out.push_str(&format!(";INTERVAL={}", self.interval));
        }
        if let Some(c) = self.count {
            out.push_str(&format!(";COUNT={c}"));
        }
        match self.until {
            Some(Until::At(t)) => out.push_str(&format!(";UNTIL={}Z", compact(t))),
            Some(Until::Local(t)) => out.push_str(&format!(";UNTIL={}", compact(t))),
            Some(Until::Date(d)) => out.push_str(&format!(";UNTIL={}", compact_date(d))),
            None => {}
        }
        if !self.by_day.is_empty() {
            let days: Vec<String> =
                self.by_day.iter().map(|(n, w)| if *n == 0 { w.code().to_string() } else { format!("{n}{}", w.code()) }).collect();
            out.push_str(&format!(";BYDAY={}", days.join(",")));
        }
        let join = |v: Vec<String>| v.join(",");
        if !self.by_month_day.is_empty() {
            out.push_str(&format!(";BYMONTHDAY={}", join(self.by_month_day.iter().map(i8::to_string).collect())));
        }
        if !self.by_month.is_empty() {
            out.push_str(&format!(";BYMONTH={}", join(self.by_month.iter().map(u8::to_string).collect())));
        }
        if !self.by_set_pos.is_empty() {
            out.push_str(&format!(";BYSETPOS={}", join(self.by_set_pos.iter().map(i16::to_string).collect())));
        }
        if self.week_start != Weekday::Mo {
            out.push_str(&format!(";WKST={}", self.week_start.code()));
        }
        out
    }
}

/// A comma-separated list, each item read by `item`, at most 64 of them; `None` if one isn't.
fn list<T>(v: &str, item: impl Fn(&str) -> Option<T>) -> Option<Vec<T>> {
    let items: Vec<&str> = v.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if items.is_empty() || items.len() > 64 {
        return None;
    }
    items.into_iter().map(item).collect()
}

/// A `BYDAY` item: `MO`, `2TU`, `-1FR`, `+3WE`.
fn by_day_of(d: &str) -> Option<(i8, Weekday)> {
    let split = d.len().checked_sub(2)?;
    let (n, w) = (d.get(..split)?, d.get(split..)?);
    let nth: i8 = if n.is_empty() || n == "+" { 0 } else { n.trim_start_matches('+').parse().ok()? };
    if !(-53..=53).contains(&nth) {
        return None;
    }
    Some((nth, Weekday::from_code(w)?))
}

/// An `UNTIL` value: a date, a wall-clock time, or a UTC time.
fn until_of(v: &str) -> Option<Until> {
    let (t, all_day, utc) = super::ical::date_time(v)?;
    Some(match (all_day, utc) {
        (true, _) => Until::Date(t.div_euclid(86_400)),
        (false, true) => Until::At(t),
        (false, false) => Until::Local(t),
    })
}

/// `YYYYMMDDTHHMMSS`.
fn compact(t: i64) -> String {
    let s = t.rem_euclid(86_400);
    format!("{}T{:02}{:02}{:02}", compact_date(t.div_euclid(86_400)), s / 3600, s % 3600 / 60, s % 60)
}

/// `YYYYMMDD`.
fn compact_date(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}{m:02}{d:02}")
}

// ----- the calendar's arithmetic -----

/// How many days month `m` (1 to 12) of year `y` has.
pub fn month_len(y: i64, m: i64) -> i64 {
    let (ny, nm) = if m >= 12 { (y.saturating_add(1), 1) } else { (y, m.max(1) + 1) };
    days_from_civil(ny, nm, 1) - days_from_civil(y, m.clamp(1, 12), 1)
}

/// The day of a month (`md` 1-based, negative from the end), if the month has it.
fn month_day(y: i64, m: i64, md: i64) -> Option<i64> {
    let len = month_len(y, m);
    let d = if md > 0 { md } else { len + 1 + md };
    (1..=len).contains(&d).then(|| days_from_civil(y, m, 1) + d - 1)
}

/// The `nth` (1-based, negative from the end) `weekday` of a month, if it has one.
pub fn nth_weekday(y: i64, m: i64, nth: i64, weekday: Weekday) -> Option<i64> {
    let first = days_from_civil(y, m, 1);
    let len = month_len(y, m);
    let day = if nth > 0 {
        let to = (weekday.index() - Weekday::of_day(first).index()).rem_euclid(7);
        first + to + 7 * (nth - 1)
    } else if nth < 0 {
        let last = first + len - 1;
        let back = (Weekday::of_day(last).index() - weekday.index()).rem_euclid(7);
        last - back - 7 * (-nth - 1)
    } else {
        return None;
    };
    (first..first + len).contains(&day).then_some(day)
}

/// Every `weekday` of a month.
fn every_weekday(y: i64, m: i64, weekday: Weekday) -> Vec<i64> {
    (1..=5).filter_map(|n| nth_weekday(y, m, n, weekday)).collect()
}

// ----- the rule's days -----

impl Recurrence {
    /// Whether a day is one of `BYDAY`'s weekdays (any ordinal), when it limits.
    fn weekday_ok(&self, day: i64) -> bool {
        let wd = Weekday::of_day(day);
        self.by_day.is_empty() || self.by_day.iter().any(|(_, w)| *w == wd)
    }

    /// Whether a day is one of `BYMONTHDAY`'s, when it limits.
    fn month_day_ok(&self, day: i64) -> bool {
        let (y, m, _) = civil_from_days(day);
        self.by_month_day.is_empty() || self.by_month_day.iter().any(|md| month_day(y, m, i64::from(*md)) == Some(day))
    }

    /// The days of month `m` that `BYMONTHDAY` and `BYDAY` give (both: the days in both, as
    /// Friday the 13th); neither: day `fallback` of the month.
    fn days_in_month(&self, y: i64, m: i64, fallback: i64) -> Vec<i64> {
        let by_month_day: Vec<i64> = self.by_month_day.iter().filter_map(|md| month_day(y, m, i64::from(*md))).collect();
        let mut by_day: Vec<i64> = Vec::new();
        for (nth, w) in &self.by_day {
            if *nth == 0 {
                by_day.extend(every_weekday(y, m, *w));
            } else {
                by_day.extend(nth_weekday(y, m, i64::from(*nth), *w));
            }
        }
        match (self.by_month_day.is_empty(), self.by_day.is_empty()) {
            (false, false) => by_month_day.into_iter().filter(|d| by_day.contains(d)).collect(),
            (false, true) => by_month_day,
            (true, false) => by_day,
            (true, true) => month_day(y, m, fallback).into_iter().collect(),
        }
    }

    /// The candidate days of the period containing `day`, in order, picked by `BYSETPOS`.
    fn days_in_period(&self, day: i64, start_day: i64) -> Vec<i64> {
        let (y, m, _) = civil_from_days(day);
        let (_, sm, sd) = civil_from_days(start_day);
        let mut v: Vec<i64> = match self.freq {
            Freq::Daily => [day].into_iter().filter(|d| self.weekday_ok(*d) && self.month_day_ok(*d)).collect(),
            Freq::Weekly if self.by_day.is_empty() => vec![day],
            Freq::Weekly => {
                // The week containing `day`, from the day weeks start on.
                let ws = self.week_start.index();
                let first = day - (Weekday::of_day(day).index() - ws).rem_euclid(7);
                self.by_day.iter().map(|(_, w)| first + (w.index() - ws).rem_euclid(7)).filter(|d| self.month_day_ok(*d)).collect()
            }
            Freq::Monthly => self.days_in_month(y, m, sd),
            Freq::Yearly => {
                let months: Vec<i64> = if self.by_month.is_empty() { vec![sm] } else { self.by_month.iter().map(|m| i64::from(*m)).collect() };
                months.into_iter().flat_map(|mo| self.days_in_month(y, mo, sd)).collect()
            }
        };
        if !self.by_month.is_empty() {
            v.retain(|d| {
                let mo = civil_from_days(*d).1;
                self.by_month.iter().any(|x| i64::from(*x) == mo)
            });
        }
        v.sort_unstable();
        v.dedup();
        self.pick(v)
    }

    /// Those of a period's days (in order) that `BYSETPOS` picks; all without it.
    fn pick(&self, v: Vec<i64>) -> Vec<i64> {
        if self.by_set_pos.is_empty() {
            return v;
        }
        let nth = |pos: i16| -> Option<i64> {
            let n = usize::from(pos.unsigned_abs());
            let index = if pos > 0 { n.checked_sub(1)? } else { v.len().checked_sub(n)? };
            v.get(index).copied()
        };
        let mut chosen: Vec<i64> = self.by_set_pos.iter().filter_map(|p| nth(*p)).collect();
        chosen.sort_unstable();
        chosen.dedup();
        chosen
    }

    /// How many periods after the one containing `start_day` to begin looking from, for starts
    /// on or after `day`: two before the one containing it (a week's days reach back before its
    /// first, a zone's day differs from UTC's), none before the first.
    fn periods_before(&self, start_day: i64, day: i64) -> i64 {
        let (sy, sm, _) = civil_from_days(start_day);
        let (y, m, _) = civil_from_days(day);
        let interval = i64::from(self.interval.max(1));
        let periods = match self.freq {
            Freq::Daily => day.saturating_sub(start_day) / interval,
            Freq::Weekly => day.saturating_sub(start_day) / interval.saturating_mul(7),
            Freq::Monthly => y.saturating_sub(sy).saturating_mul(12).saturating_add(m - sm) / interval,
            Freq::Yearly => y.saturating_sub(sy) / interval,
        };
        periods.saturating_sub(2).max(0)
    }

    /// The first day of the period `k` periods after the one containing `start_day`.
    fn period(&self, start_day: i64, k: i64) -> Option<i64> {
        let (y, m, _) = civil_from_days(start_day);
        let step = k.checked_mul(i64::from(self.interval.max(1)))?;
        match self.freq {
            Freq::Daily => start_day.checked_add(step),
            Freq::Weekly => start_day.checked_add(step.checked_mul(7)?),
            Freq::Monthly => {
                let total = (m - 1).checked_add(step)?;
                let yy = y.checked_add(total.div_euclid(12))?;
                (yy <= 9999).then(|| days_from_civil(yy, total.rem_euclid(12) + 1, 1))
            }
            Freq::Yearly => {
                let yy = y.checked_add(step)?;
                (yy <= 9999).then(|| days_from_civil(yy, 1, 1))
            }
        }
    }
}

/// A walk through a series' starts: those taken, and when it ends.
struct Walk<'a> {
    rule: &'a Recurrence,
    before: i64,
    out: Vec<i64>,
    /// Starts the rule made: `COUNT` counts them.
    produced: usize,
    /// Candidates looked at: at most [`MAX_STEPS`].
    steps: usize,
}

impl Walk<'_> {
    /// One more candidate looked at: `Break` past [`MAX_STEPS`].
    fn step(&mut self) -> ControlFlow<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }

    /// Whether a start at the wall-clock time `local` (`at` in UTC) is past the rule's `UNTIL`
    /// or the window: the walk ends there.
    fn ended(&self, local: i64, at: i64) -> bool {
        let past_until = match self.rule.until {
            Some(Until::At(u)) => at > u,
            Some(Until::Local(u)) => local > u,
            Some(Until::Date(d)) => local > d.saturating_mul(86_400).saturating_add(86_399),
            None => false,
        };
        past_until || at >= self.before
    }

    /// `at` made by the rule: kept, unless `COUNT` has been made already. `Break` once the
    /// series or [`MAX_STARTS`] is done.
    fn take(&mut self, at: i64) -> ControlFlow<()> {
        self.produced += 1;
        if self.rule.count.is_some_and(|c| self.produced > usize::try_from(c).unwrap_or(usize::MAX)) {
            return ControlFlow::Break(());
        }
        self.out.push(at);
        if self.out.len() >= MAX_STARTS { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }
}

/// Every start of `e` before `before`, from near `from` (a rule without `COUNT` is looked at from
/// two periods before the window; with one, from its first start, which `COUNT` counts from): its
/// own start alone without a rule. Occurrences taken out (`exdates`) are left out.
pub fn starts(e: &Event, from: i64, before: i64, zones: &dyn ZoneRules) -> Vec<i64> {
    let Some(rule) = &e.recurrence else {
        return if e.start < before && !e.exdates.contains(&e.start) { vec![e.start] } else { Vec::new() };
    };
    // An all-day series repeats on dates; a timed one at its wall-clock time in its zone.
    let local = |t: i64| if e.all_day { t } else { local_time(zones, &e.time_zone, t) };
    let to_utc = |t: i64| if e.all_day { t } else { utc_time(zones, &e.time_zone, t) };
    let local_start = local(e.start);
    let start_day = local_start.div_euclid(86_400);
    let time = local_start.rem_euclid(86_400);
    let mut walk = Walk { rule, before, out: Vec::new(), produced: 0, steps: 0 };
    let first = if rule.count.is_some() { 0 } else { rule.periods_before(start_day, from.div_euclid(86_400).saturating_sub(1)) };
    let mut k = first;
    'periods: while let Some(p) = rule.period(start_day, k) {
        k += 1;
        // A period wholly past the window ends it (eight days: two for any zone, six for a
        // week's days before the start's own weekday).
        if p.checked_mul(86_400).is_none_or(|t| t.saturating_sub(8 * 86_400) >= before) {
            break;
        }
        let days = rule.days_in_period(if k == 1 { start_day } else { p }, start_day);
        if days.is_empty() && walk.step().is_break() {
            break;
        }
        for day in days {
            if walk.step().is_break() {
                break 'periods;
            }
            let Some(at_local) = day.checked_mul(86_400).and_then(|d| d.checked_add(time)) else { break 'periods };
            if at_local < local_start {
                continue;
            }
            // The first is the event's own start, whatever its zone makes of it.
            let t = if at_local == local_start { e.start } else { to_utc(at_local) };
            if walk.ended(at_local, t) || walk.take(t).is_break() {
                break 'periods;
            }
        }
    }
    walk.out.retain(|s| !e.exdates.contains(s));
    walk.out
}

/// One occurrence of an event: which event (an index into the list given) and when.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub event: usize,
    pub start: i64,
    pub end: i64,
}

/// The occurrences of `events` that overlap `[from, to)`, by start: each event once, each series
/// expanded ([`starts`], at most [`MAX_OCCURRENCES`] each). An occurrence the list has on its own
/// (in the same calendar, with the series' UID or naming the series' provider id as its
/// `series`, and its `recurrence_id`) replaces the series' one it names; cancelled ones show
/// nowhere.
pub fn occurrences(events: &[Event], from: i64, to: i64, zones: &dyn ZoneRules) -> Vec<Occurrence> {
    let mut out = Vec::new();
    if to <= from {
        return out;
    }
    // The occurrences each series has on their own, gathered once, by the series' UID and by
    // its provider id (Google's cancelled occurrences carry no UID).
    let mut moved: std::collections::HashMap<(&str, &str), Vec<i64>> = std::collections::HashMap::new();
    for e in events {
        if let Some(r) = e.recurrence_id {
            moved.entry((e.calendar.as_str(), e.uid.as_str())).or_default().push(r);
            if let Some(series) = e.series.as_deref()
                && series != e.uid
            {
                moved.entry((e.calendar.as_str(), series)).or_default().push(r);
            }
        }
    }
    let none = Vec::new();
    for (i, e) in events.iter().enumerate() {
        if e.status == EventStatus::Cancelled {
            continue;
        }
        let len = e.duration();
        let overlaps = |s: i64| s < to && s.saturating_add(len.max(1)) > from;
        if e.recurrence_id.is_some() || e.recurrence.is_none() {
            if overlaps(e.start) && !e.exdates.contains(&e.start) {
                out.push(Occurrence { event: i, start: e.start, end: e.start.saturating_add(len) });
            }
            continue;
        }
        let by_uid = moved.get(&(e.calendar.as_str(), e.uid.as_str())).unwrap_or(&none);
        let by_id = if e.remote.id.is_empty() || e.remote.id == e.uid { &none } else { moved.get(&(e.calendar.as_str(), e.remote.id.as_str())).unwrap_or(&none) };
        let mut n = 0;
        for s in starts(e, from.saturating_sub(len), to, zones) {
            if s >= to {
                break;
            }
            if !overlaps(s) || by_uid.contains(&s) || by_id.contains(&s) {
                continue;
            }
            out.push(Occurrence { event: i, start: s, end: s.saturating_add(len) });
            n += 1;
            if n >= MAX_OCCURRENCES {
                break;
            }
        }
    }
    out.sort_by_key(|o| (o.start, o.event));
    out
}
