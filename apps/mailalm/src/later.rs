//! The times the Snooze ▾ and "Remind me if no reply" menus offer (C2), worked out from the time
//! here. Pure: times are wall-clock seconds here ("local", as if UTC); the interface turns them
//! into UTC instants with `time::utc_of_local`.

use crate::i18n::tr;
use crate::time::weekday_index;

/// One choice of a menu: its label and its time (local).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub label: &'static str,
    pub local: i64,
}

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
/// "Later today" is offered until this hour (it must still be today, and not the night).
const LATEST_TODAY: i64 = 21 * HOUR;

/// The Snooze ▾ choices at local time `now`: later today (three hours on, at the next full hour,
/// while it is still before 21:00), tomorrow morning (8:00), this weekend (Saturday 9:00, from
/// Monday to Friday), next week (Monday 8:00).
pub fn snooze_presets(now: i64) -> Vec<Preset> {
    let day = now.div_euclid(DAY);
    let weekday = weekday_index(day);
    let mut out = Vec::new();
    let later = (now + 3 * HOUR).div_euclid(HOUR) * HOUR + if (now + 3 * HOUR).rem_euclid(HOUR) == 0 { 0 } else { HOUR };
    if later - day * DAY <= LATEST_TODAY {
        out.push(Preset { label: tr("Later today"), local: later });
    }
    out.push(Preset { label: tr("Tomorrow morning"), local: (day + 1) * DAY + 8 * HOUR });
    if weekday < 5 {
        out.push(Preset { label: tr("This weekend"), local: (day + 5 - weekday) * DAY + 9 * HOUR });
    }
    out.push(Preset { label: tr("Next week"), local: (day + 7 - weekday) * DAY + 8 * HOUR });
    out
}

/// The "Remind me if no reply" choices at local time `now`: tomorrow, in three days, in a week,
/// each at 9:00.
pub fn follow_up_presets(now: i64) -> Vec<Preset> {
    let day = now.div_euclid(DAY);
    vec![
        Preset { label: tr("Tomorrow"), local: (day + 1) * DAY + 9 * HOUR },
        Preset { label: tr("In 3 days"), local: (day + 3) * DAY + 9 * HOUR },
        Preset { label: tr("In a week"), local: (day + 7) * DAY + 9 * HOUR },
    ]
}

/// A date (`YYYY-MM-DD`) and a time (`HH:MM`) typed by the user, as a local time; `None` when
/// either can't be read, or when it isn't after `now`.
pub fn picked(date: &str, clock: &str, now: i64) -> Option<i64> {
    let day = mailalm_model::query::day(date.trim())?;
    let at = day + crate::time::parse_clock(clock)?;
    (at > now).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wednesday 2026-11-25 at `h:m`, local.
    fn wednesday(h: i64, m: i64) -> i64 {
        mailalm_model::query::day("2026-11-25").unwrap_or(0) + h * HOUR + m * 60
    }

    fn at(date: &str, h: i64) -> i64 {
        mailalm_model::query::day(date).unwrap_or(0) + h * HOUR
    }

    #[test]
    fn snooze_presets_on_a_weekday() {
        let p = snooze_presets(wednesday(10, 30));
        let got: Vec<(&str, i64)> = p.iter().map(|x| (x.label, x.local)).collect();
        assert_eq!(
            got,
            vec![
                ("Later today", at("2026-11-25", 14)),
                ("Tomorrow morning", at("2026-11-26", 8)),
                ("This weekend", at("2026-11-28", 9)),
                ("Next week", at("2026-11-30", 8)),
            ]
        );
        // On the hour: three hours on exactly.
        assert_eq!(snooze_presets(wednesday(11, 0)).first().map(|x| x.local), Some(at("2026-11-25", 14)));
    }

    #[test]
    fn late_evenings_and_weekends_offer_less() {
        let evening = snooze_presets(wednesday(19, 5));
        assert!(evening.iter().all(|x| x.label != "Later today"), "22:00 is too late");
        assert!(snooze_presets(wednesday(18, 0)).iter().any(|x| x.label == "Later today"), "21:00 is still today");
        let saturday = snooze_presets(at("2026-11-28", 10));
        assert!(saturday.iter().all(|x| x.label != "This weekend"));
        assert_eq!(saturday.last().map(|x| x.local), Some(at("2026-11-30", 8)), "next Monday");
        let sunday = snooze_presets(at("2026-11-29", 10));
        assert_eq!(sunday.last().map(|x| x.local), Some(at("2026-11-30", 8)));
        let monday = snooze_presets(at("2026-11-30", 7));
        assert_eq!(monday.last().map(|x| x.local), Some(at("2026-12-07", 8)), "a week on");
    }

    #[test]
    fn follow_ups_and_picked_times() {
        let f = follow_up_presets(wednesday(16, 0));
        assert_eq!(f.iter().map(|x| x.local).collect::<Vec<_>>(), vec![at("2026-11-26", 9), at("2026-11-28", 9), at("2026-12-02", 9)]);
        let now = wednesday(10, 0);
        assert_eq!(picked("2026-12-01", "7:30", now), Some(at("2026-12-01", 7) + 30 * 60));
        assert_eq!(picked("2026-11-25", "09:00", now), None, "in the past");
        assert_eq!(picked("2026-13-01", "09:00", now), None);
        assert_eq!(picked("2026-12-01", "25:00", now), None);
    }
}
