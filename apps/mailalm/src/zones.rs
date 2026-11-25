//! The time zone rules events are read and written with: chrono-tz's database (IANA names;
//! Windows names through the model's table), and the computer's own zone for a time without one.

use chrono::{DateTime, Offset, TimeZone};
use mailalm_model::pim::{ZoneRules, iana_of_windows, is_utc_name};

use crate::time;

/// The zone database, as the model's `ZoneRules`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TzRules;

impl ZoneRules for TzRules {
    fn offset(&self, zone: &str, utc: i64) -> Option<i64> {
        let zone = zone.trim();
        if zone.is_empty() {
            return Some(time::offset(utc));
        }
        if is_utc_name(zone) {
            return Some(0);
        }
        let tz: chrono_tz::Tz = zone.parse().ok().or_else(|| iana_of_windows(zone).and_then(|z| z.parse().ok()))?;
        let at = DateTime::from_timestamp(utc, 0)?;
        Some(i64::from(tz.offset_from_utc_datetime(&at.naive_utc()).fix().local_minus_utc()))
    }
}

/// Whether the database knows a zone name.
pub fn known(zone: &str) -> bool {
    TzRules.offset(zone, 0).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_follow_their_rules() {
        // 2026-07-01 12:00 UTC: summer time in Toronto (UTC-4), winter in Sydney (UTC+10).
        let july = 1_782_907_200;
        assert_eq!(TzRules.offset("America/Toronto", july), Some(-4 * 3600));
        assert_eq!(TzRules.offset("Australia/Sydney", july), Some(10 * 3600));
        assert_eq!(TzRules.offset("Eastern Standard Time", july), Some(-4 * 3600), "a Windows name");
        assert_eq!(TzRules.offset("UTC", july), Some(0));
        assert_eq!(TzRules.offset("Not/A_Zone", july), None);
        assert!(known("Europe/Paris") && !known("Nowhere"));
    }
}
