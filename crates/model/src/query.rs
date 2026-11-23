//! The search box's language (PLAN.md, section 6): words, "quoted phrases" and operators
//! `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `is:read`, `is:flagged`,
//! `before:YYYY-MM-DD`, `after:YYYY-MM-DD`, `in:<folder>`. Every term must match. Matching ignores
//! case.

use crate::mail::Summary;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// Anywhere: sender, recipients, subject, text.
    Text(String),
    From(String),
    To(String),
    Subject(String),
    HasAttachment,
    Unread,
    Read,
    Flagged,
    /// Strictly before this day (seconds since 1970, UTC midnight).
    Before(i64),
    /// On or after this day.
    After(i64),
    /// In a folder whose path contains this.
    In(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    /// Reads what was typed. An operator it does not know, or a date it cannot read, is searched
    /// as plain text.
    pub fn parse(input: &str) -> Self {
        let mut terms = Vec::new();
        for token in tokens(input) {
            let (op, value) = match token.split_once(':') {
                Some((op, value)) if !value.is_empty() => (op.to_ascii_lowercase(), value.to_owned()),
                _ => {
                    terms.push(Term::Text(token.to_lowercase()));
                    continue;
                }
            };
            let lower = value.to_lowercase();
            let term = match (op.as_str(), lower.as_str()) {
                ("from", _) => Some(Term::From(lower)),
                ("to", _) => Some(Term::To(lower)),
                ("subject", _) => Some(Term::Subject(lower)),
                ("has", "attachment" | "attachments") => Some(Term::HasAttachment),
                ("is", "unread") => Some(Term::Unread),
                ("is", "read") => Some(Term::Read),
                ("is", "flagged") => Some(Term::Flagged),
                ("before", _) => day(&lower).map(Term::Before),
                ("after", _) => day(&lower).map(Term::After),
                ("in", _) => Some(Term::In(lower)),
                _ => None,
            };
            terms.push(term.unwrap_or_else(|| Term::Text(token.to_lowercase())));
        }
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether a message matches. `folder` is its folder's path; `text` is its text when the body
    /// is in the cache (lowercase or not).
    pub fn matches(&self, s: &Summary, folder: &str, text: Option<&str>) -> bool {
        let text_lower = text.map(str::to_lowercase);
        self.terms.iter().all(|t| match t {
            Term::Text(w) => {
                contains(&s.subject, w)
                    || person_matches(&s.from, w)
                    || s.to.iter().chain(&s.cc).any(|p| person_matches(p, w))
                    || contains(&s.preview, w)
                    || text_lower.as_deref().is_some_and(|b| b.contains(w.as_str()))
            }
            Term::From(w) => person_matches(&s.from, w),
            Term::To(w) => s.to.iter().chain(&s.cc).any(|p| person_matches(p, w)),
            Term::Subject(w) => contains(&s.subject, w),
            Term::HasAttachment => s.has_attachments,
            Term::Unread => !s.flags.seen,
            Term::Read => s.flags.seen,
            Term::Flagged => s.flags.flagged,
            Term::Before(d) => s.date < *d,
            Term::After(d) => s.date >= *d,
            Term::In(f) => contains(folder, f),
        })
    }
}

fn contains(hay: &str, needle_lower: &str) -> bool {
    hay.to_lowercase().contains(needle_lower)
}

fn person_matches(p: &crate::mail::Person, w: &str) -> bool {
    contains(&p.name, w) || contains(&p.address, w)
}

/// Splits on spaces, keeping "quoted phrases" (also after an operator: `subject:"two words"`)
/// whole, without their quotes.
fn tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `YYYY-MM-DD` as seconds since 1970 at UTC midnight.
pub fn day(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=9999).contains(&y) {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86_400)
}

/// Days since 1970-01-01 for a date of the proleptic Gregorian calendar (H. Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date (year, month, day) of a day count since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{Flags, Person};

    fn msg() -> Summary {
        Summary {
            uid: 1,
            from: Person { name: "Zoe Park".into(), address: "zoe@studio.example".into() },
            to: vec![Person { name: String::new(), address: "alex@example.com".into() }],
            subject: "Storyboard review".into(),
            date: day("2026-11-20").unwrap_or(0) + 3600,
            flags: Flags { seen: false, flagged: true, ..Flags::default() },
            has_attachments: true,
            preview: "The animatic is in good shape".into(),
            ..Summary::default()
        }
    }

    #[test]
    fn words_and_operators() {
        let q = Query::parse(r#"from:zoe subject:"storyboard review" has:attachment is:unread"#);
        assert_eq!(q.terms.len(), 4);
        assert!(q.matches(&msg(), "INBOX", None));
        assert!(!Query::parse("is:read").matches(&msg(), "INBOX", None));
        assert!(Query::parse("animatic").matches(&msg(), "INBOX", None));
        assert!(Query::parse("to:alex@").matches(&msg(), "INBOX", None));
        assert!(!Query::parse("from:camille").matches(&msg(), "INBOX", None));
    }

    #[test]
    fn body_text_is_searched_when_cached() {
        assert!(!Query::parse("turnarounds").matches(&msg(), "INBOX", None));
        assert!(Query::parse("turnarounds").matches(&msg(), "INBOX", Some("Here are the TURNAROUNDS")));
    }

    #[test]
    fn dates() {
        assert!(Query::parse("after:2026-11-20").matches(&msg(), "INBOX", None));
        assert!(Query::parse("before:2026-11-21").matches(&msg(), "INBOX", None));
        assert!(!Query::parse("before:2026-11-20").matches(&msg(), "INBOX", None));
        // A date it cannot read is searched as text, and so finds nothing here.
        assert!(!Query::parse("before:soon").matches(&msg(), "INBOX", None));
    }

    #[test]
    fn folder_filter() {
        assert!(Query::parse("in:inbox").matches(&msg(), "INBOX", None));
        assert!(!Query::parse("in:archive").matches(&msg(), "INBOX", None));
    }

    #[test]
    fn calendar_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        for z in [-1, 0, 59, 11_016, 11_017, 20_000, 20_778] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }
}
