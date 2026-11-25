//! The search box's language (PLAN.md, section 6): words, "quoted phrases" and operators
//! `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `is:read`, `is:flagged` (or
//! `is:starred`), `before:YYYY-MM-DD`, `after:YYYY-MM-DD`, `in:<folder>`, `account:<name or
//! address>`, `category:<name>`. Every term must match. Matching ignores case.
//!
//! Where a search looks is said apart from the words: which accounts ([`crate::SearchScope`])
//! and which of their folders ([`FolderScope`]). A search can be kept as a search folder
//! ([`SavedSearch`]).

use serde::{Deserialize, Serialize};

use crate::SearchScope;
use crate::mail::{Folder, Summary};

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
    /// In an account whose name or address contains this (decided per account:
    /// [`Query::wants_account`]).
    Account(String),
    /// With a category (Outlook's categories, Gmail's Inbox categories) that contains this.
    Category(String),
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
                ("is", "flagged" | "starred") => Some(Term::Flagged),
                ("before", _) => day(&lower).map(Term::Before),
                ("after", _) => day(&lower).map(Term::After),
                ("in", _) => Some(Term::In(lower)),
                ("account", _) => Some(Term::Account(lower)),
                ("category", _) => Some(Term::Category(lower)),
                _ => None,
            };
            terms.push(term.unwrap_or_else(|| Term::Text(token.to_lowercase())));
        }
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether an account is searched at all: every `account:` term is in its name or its
    /// address. With none, every account is.
    pub fn wants_account(&self, name: &str, address: &str) -> bool {
        self.terms.iter().all(|t| match t {
            Term::Account(w) => contains(name, w) || contains(address, w),
            _ => true,
        })
    }

    /// Whether the query asks for words in the text (a message's text is then read from the
    /// cache to match it).
    pub fn needs_text(&self) -> bool {
        self.terms.iter().any(|t| matches!(t, Term::Text(_)))
    }

    /// The terms a server's search may not have judged (marks, attachments, categories, folder):
    /// what is checked again on the messages it found.
    pub fn metadata_only(&self) -> Query {
        let terms = self
            .terms
            .iter()
            .filter(|t| matches!(t, Term::HasAttachment | Term::Unread | Term::Read | Term::Flagged | Term::In(_) | Term::Category(_)))
            .cloned()
            .collect();
        Query { terms }
    }

    /// Whether a message matches. `folder` is its folder's path; `text` is its text when the body
    /// is in the cache (lowercase or not). `account:` terms are the account's to judge
    /// ([`Query::wants_account`]): they match here.
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
            Term::Account(_) => true,
            Term::Category(c) => s.categories.iter().any(|x| contains(x, c)),
        })
    }
}

/// Which folders of an account a search covers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolderScope {
    /// This folder only (the one being looked at).
    Folder(String),
    /// This folder and every folder under it.
    WithSubfolders(String),
    /// Every folder of the account.
    #[default]
    Account,
}

impl FolderScope {
    /// Whether the scope covers this folder.
    pub fn covers(&self, f: &Folder) -> bool {
        match self {
            FolderScope::Account => true,
            FolderScope::Folder(p) => f.path == *p,
            FolderScope::WithSubfolders(p) => {
                f.path == *p
                    || f.delimiter.is_some_and(|d| f.path.strip_prefix(p.as_str()).is_some_and(|rest| rest.starts_with(d)))
            }
        }
    }

    /// The folders of `folders` the scope covers, in their order.
    pub fn select<'a>(&self, folders: &'a [Folder]) -> Vec<&'a Folder> {
        folders.iter().filter(|f| self.covers(f)).collect()
    }
}

/// A search kept as a search folder: its words, its accounts, its folders. Kept in the app's
/// settings; shown under the accounts and run again when opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearch {
    /// The name shown in the folder pane.
    pub name: String,
    /// What was typed, operators included.
    pub query: String,
    #[serde(default)]
    pub accounts: SearchScope,
    #[serde(default)]
    pub folders: FolderScope,
    /// Whether the servers are asked too when it is opened.
    #[serde(default)]
    pub server: bool,
}

/// The longest name and query of a saved search, in characters.
pub const MAX_SAVED_NAME: usize = 255;
pub const MAX_SAVED_QUERY: usize = 1000;

impl SavedSearch {
    /// A saved search with its name and query cleaned to one line and capped. `None` with an
    /// empty name or query.
    pub fn new(name: &str, query: &str, accounts: SearchScope, folders: FolderScope, server: bool) -> Option<SavedSearch> {
        let name = crate::pim::clean_line(name, MAX_SAVED_NAME);
        let query = crate::pim::clean_line(query, MAX_SAVED_QUERY);
        (!name.is_empty() && !query.is_empty()).then_some(SavedSearch { name, query, accounts, folders, server })
    }

    pub fn parsed(&self) -> Query {
        Query::parse(&self.query)
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
    fn every_operator_of_the_plan() {
        let q = Query::parse(r#"from:zoe to:alex subject:review has:attachment is:unread is:starred before:2026-12-01 after:2026-11-01 in:inbox account:work category:promo "good shape""#);
        assert_eq!(
            q.terms,
            vec![
                Term::From("zoe".into()),
                Term::To("alex".into()),
                Term::Subject("review".into()),
                Term::HasAttachment,
                Term::Unread,
                Term::Flagged,
                Term::Before(day("2026-12-01").unwrap()),
                Term::After(day("2026-11-01").unwrap()),
                Term::In("inbox".into()),
                Term::Account("work".into()),
                Term::Category("promo".into()),
                Term::Text("good shape".into()),
            ]
        );
        let mut m = msg();
        assert!(!q.matches(&m, "INBOX", None), "no category yet");
        m.categories = vec!["Promotions".into()];
        assert!(q.matches(&m, "INBOX", None));
        // Unknown operators and empty values are words.
        assert_eq!(Query::parse("size:big is:").terms, vec![Term::Text("size:big".into()), Term::Text("is:".into())]);
    }

    #[test]
    fn accounts_are_chosen_by_name_or_address() {
        let q = Query::parse("account:videotron hello");
        assert!(q.wants_account("Vidéotron", "alex@videotron.ca"));
        assert!(!q.wants_account("Work", "alex@outlook.com"));
        assert!(Query::parse("hello").wants_account("Work", "alex@outlook.com"));
        // The account's term doesn't hold a message back in the account searched.
        assert!(Query::parse("account:work").matches(&msg(), "INBOX", None));
    }

    #[test]
    fn what_a_server_search_is_checked_again_for() {
        let q = Query::parse("from:zoe hello is:unread has:attachment before:2026-12-01 category:x in:arch");
        assert_eq!(q.metadata_only().terms, vec![Term::Unread, Term::HasAttachment, Term::Category("x".into()), Term::In("arch".into())]);
        assert!(q.needs_text() && !Query::parse("from:zoe").needs_text());
    }

    #[test]
    fn folder_scopes() {
        let f = |p: &str| Folder { path: p.into(), wire: p.into(), delimiter: Some('/'), role: crate::FolderRole::Other, total: 0, unread: 0 };
        let all = [f("INBOX"), f("Work"), f("Work/2026"), f("Workshop"), f("Archive")];
        let paths = |s: &FolderScope| s.select(&all).iter().map(|f| f.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths(&FolderScope::Folder("Work".into())), ["Work"]);
        assert_eq!(paths(&FolderScope::WithSubfolders("Work".into())), ["Work", "Work/2026"]);
        assert_eq!(paths(&FolderScope::Account).len(), 5);
    }

    #[test]
    fn saved_searches_are_cleaned() {
        let s = SavedSearch::new(" Invoices\r\n", "from:billing\u{202e} has:attachment", SearchScope::AllAccounts, FolderScope::Account, true).unwrap();
        assert_eq!((s.name.as_str(), s.query.as_str()), ("Invoices", "from:billing has:attachment"));
        assert_eq!(s.parsed().terms.len(), 2);
        assert!(SavedSearch::new(" ", "x", SearchScope::AllAccounts, FolderScope::Account, false).is_none());
        assert!(SearchScope::Accounts(vec![crate::AccountId(2)]).covers(crate::AccountId(2)));
        assert!(!SearchScope::Accounts(vec![]).covers(crate::AccountId(2)));
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
