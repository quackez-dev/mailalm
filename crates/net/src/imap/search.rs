//! MailALM's own: the server's search in a folder (`UID SEARCH`, RFC 3501 section 6.4.4) from
//! the search box's query, and making a folder (`CREATE`, for `MailALM/Snoozed`).
//!
//! The words go as quoted strings when they are printable ASCII, else as literals with
//! `CHARSET UTF-8` (which a server may refuse: its `NO` is then the answer). Control characters
//! never go. The terms the server can't judge (attachments, categories, folder, account) are left
//! to the caller, who checks them on what is found.

use std::io::{Read, Write};

use mailalm_model::query::{Term, civil_from_days};
use mailalm_model::Query;

use super::{Client, Part, mailbox_arg, quote};
use crate::{NetError, Result};

/// The most UIDs one search gives (the newest).
pub(super) const MAX_FOUND: usize = 1000;
/// The longest word sent, in characters.
const MAX_WORD: usize = 200;
/// The most terms sent.
const MAX_TERMS: usize = 20;

/// A piece of a `UID SEARCH` command being built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Arg {
    Text(String),
    Literal(Vec<u8>),
}

/// The criteria of `query` as `UID SEARCH` arguments, and whether one goes as UTF-8. `None` when
/// none of its terms is one the server can search for.
pub(super) fn criteria(query: &Query) -> Option<(Vec<Arg>, bool)> {
    let mut args: Vec<Arg> = Vec::new();
    let mut utf8 = false;
    let mut word = |args: &mut Vec<Arg>, key: &str, w: &str| {
        let w: String = w.chars().filter(|c| !c.is_control()).take(MAX_WORD).collect();
        args.push(Arg::Text(format!(" {key} ")));
        if w.chars().all(|c| (' '..='~').contains(&c)) {
            args.push(Arg::Text(quote(&w)));
        } else {
            utf8 = true;
            args.push(Arg::Literal(w.into_bytes()));
        }
    };
    let mut terms = 0;
    for t in &query.terms {
        if terms >= MAX_TERMS {
            break;
        }
        terms += 1;
        match t {
            Term::Text(w) => word(&mut args, "TEXT", w),
            Term::From(w) => word(&mut args, "FROM", w),
            Term::Subject(w) => word(&mut args, "SUBJECT", w),
            Term::To(w) => {
                args.push(Arg::Text(" OR".into()));
                word(&mut args, "TO", w);
                word(&mut args, "CC", w);
            }
            Term::Unread => args.push(Arg::Text(" UNSEEN".into())),
            Term::Read => args.push(Arg::Text(" SEEN".into())),
            Term::Flagged => args.push(Arg::Text(" FLAGGED".into())),
            Term::Before(d) => args.push(Arg::Text(format!(" SENTBEFORE {}", imap_date(*d)))),
            Term::After(d) => args.push(Arg::Text(format!(" SENTSINCE {}", imap_date(*d)))),
            Term::HasAttachment | Term::In(_) | Term::Account(_) | Term::Category(_) => terms -= 1,
        }
    }
    if args.is_empty() {
        return None;
    }
    let head = if utf8 { "UID SEARCH CHARSET UTF-8" } else { "UID SEARCH" };
    args.insert(0, Arg::Text(head.into()));
    Some((args, utf8))
}

/// A day as IMAP writes it (`1-Feb-1994`).
fn imap_date(t: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let month = usize::try_from(m - 1).ok().and_then(|i| MONTHS.get(i)).copied().unwrap_or("Jan");
    format!("{d}-{month}-{:04}", y.clamp(1, 9999))
}

impl<S: Read + Write> Client<S> {
    /// `UID SEARCH` in the selected folder for `query`: the UIDs found, the newest
    /// [`MAX_FOUND`], in order. [`NetError::Invalid`] (nothing sent) when nothing in the query is
    /// for the server.
    pub(super) fn search_query(&mut self, query: &Query) -> Result<Vec<u32>> {
        let (args, _) = criteria(query).ok_or_else(|| NetError::Invalid("nothing in the search the server can look for".into()))?;
        let parts: Vec<Part<'_>> = args
            .iter()
            .map(|a| match a {
                Arg::Text(t) => Part::Text(t.as_str()),
                Arg::Literal(b) => Part::Literal(b.as_slice()),
            })
            .collect();
        let mut found = super::messages::searched(self.cmd(&parts)?);
        if found.len() > MAX_FOUND {
            found.drain(..found.len() - MAX_FOUND);
        }
        Ok(found)
    }

    /// `UID SEARCH HEADER Message-ID "<id>"` in the selected folder (a draft saved before, to
    /// replace it): the UIDs found. An id that isn't one (a space, a quote, a control) is refused
    /// before anything is sent.
    pub(super) fn search_message_id(&mut self, message_id: &str) -> Result<Vec<u32>> {
        let id = message_id.trim().trim_start_matches('<').trim_end_matches('>');
        if id.is_empty() || id.len() > 900 || !id.chars().all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '<' | '>')) {
            return Err(NetError::Invalid("not a Message-ID".into()));
        }
        let cmd = format!("UID SEARCH HEADER Message-ID \"<{id}>\"");
        Ok(super::messages::searched(self.cmd(&[Part::Text(&cmd)])?))
    }

    /// `CREATE` a folder (its name as the server knows it).
    pub(super) fn create(&mut self, wire: &str) -> Result<()> {
        if wire.is_empty() || wire.chars().any(char::is_control) {
            return Err(NetError::Invalid("not a folder name".into()));
        }
        self.cmd(&[Part::Text(&format!("CREATE {}", mailbox_arg(wire)))]).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(args: &[Arg]) -> String {
        args.iter()
            .map(|a| match a {
                Arg::Text(t) => t.clone(),
                Arg::Literal(b) => format!("{{{}}}<{}>", b.len(), String::from_utf8_lossy(b)),
            })
            .collect()
    }

    #[test]
    fn criteria_from_the_query() {
        let q = Query::parse(r#"from:zoe "good shape" to:alex is:unread is:flagged after:2026-11-01 before:2026-12-01 has:attachment in:work"#);
        let (args, utf8) = criteria(&q).unwrap();
        assert!(!utf8);
        assert_eq!(
            text(&args),
            r#"UID SEARCH FROM "zoe" TEXT "good shape" OR TO "alex" CC "alex" UNSEEN FLAGGED SENTSINCE 1-Nov-2026 SENTBEFORE 1-Dec-2026"#
        );
        // Words outside ASCII go as literals, in UTF-8; controls and quotes never break out.
        let q = Query::parse("subject:réunion");
        let (args, utf8) = criteria(&q).unwrap();
        assert!(utf8);
        assert_eq!(text(&args), "UID SEARCH CHARSET UTF-8 SUBJECT {8}<réunion>");
        let q = Query { terms: vec![Term::Text("a\"b\\c\r\nd".into())] };
        assert_eq!(text(&criteria(&q).unwrap().0), r#"UID SEARCH TEXT "a\"b\\cd""#);
        // Nothing for the server.
        assert!(criteria(&Query::parse("has:attachment in:inbox account:x category:y")).is_none());
        assert_eq!(imap_date(0), "1-Jan-1970");
    }
}
