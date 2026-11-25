// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/imap/messages.rs. MIT OR Apache-2.0.
// Changed: MailALM's API on top (headers, flags and messages fetched in chunks, only the UIDs asked for); flags checked before they go; no search by text here (imap/search.rs, MailALM's own, uses `searched`), no COPYUID or APPENDUID, no dated APPEND, no MODSEQ.

//! Messages by UID: searched, fetched, flagged, moved, expunged and appended.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use imap_proto::{AttributeValue, MailboxDatum, Response};

use super::{Client, Fetched, MAX_LITERAL, Part, mailbox_arg, parse_internal_date, uid_set, uid_sets};
use crate::{NetError, Result, clip};

/// UIDs per `UID FETCH` of headers.
const HEADER_CHUNK: usize = 200;
/// UIDs per `UID FETCH` of flags, and per `UID STORE`, `UID MOVE` or `UID COPY`.
const FLAG_CHUNK: usize = 500;

/// The UIDs a `UID SEARCH` found, in order, each once.
pub(super) fn searched(rs: Vec<Response<'static>>) -> Vec<u32> {
    let mut out = Vec::new();
    for r in rs {
        if let Response::MailboxData(MailboxDatum::Search(v)) = r {
            out.extend(v);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// What one message's `FETCH` responses said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Attrs {
    uid: u32,
    flags: Option<Vec<String>>,
    size: u32,
    internal_date: Option<i64>,
    header: Option<Vec<u8>>,
    body: Option<Vec<u8>>,
}

/// A flag that can go in a command as it is: `\` and an atom, or a keyword atom. Anything else
/// (a space, a parenthesis, a line end) would change the command, and is refused.
fn checked_flag(flag: &str) -> Result<&str> {
    let name = flag.strip_prefix('\\').unwrap_or(flag);
    let atom = |c: char| c.is_ascii_graphic() && !matches!(c, '(' | ')' | '{' | '%' | '*' | '"' | '\\' | ']');
    if name.is_empty() || name.len() > 128 || !name.chars().all(atom) {
        return Err(NetError::Invalid(format!("not a flag: {}", clip(flag))));
    }
    Ok(flag)
}

impl<S: Read + Write> Client<S> {
    /// `UID SEARCH ALL` in the selected folder.
    pub(super) fn uids(&mut self) -> Result<Vec<u32>> {
        self.uid_search("ALL")
    }

    fn uid_search(&mut self, criteria: &str) -> Result<Vec<u32>> {
        let cmd = format!("UID SEARCH {criteria}");
        Ok(searched(self.cmd(&[Part::Text(&cmd)])?))
    }

    /// Headers, flags, sizes and arrival dates of `uids`, in chunks. Only messages asked for, and
    /// with a header, come back: a flag change the server tells of in passing is no message.
    pub(super) fn fetch_headers(&mut self, uids: &[u32]) -> Result<Vec<Fetched>> {
        let asked: HashSet<u32> = uids.iter().copied().collect();
        let mut out = Vec::new();
        for set in uid_sets(uids, HEADER_CHUNK) {
            for f in self.uid_fetch(&set, "UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER]")? {
                if let (true, Some(header)) = (asked.contains(&f.uid), f.header) {
                    out.push(Fetched { uid: f.uid, flags: f.flags.unwrap_or_default(), size: f.size, internal_date: f.internal_date, header });
                }
            }
        }
        Ok(out)
    }

    /// The flags of `uids`, in chunks: each message asked for that the server still has.
    pub(super) fn fetch_flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>> {
        let asked: HashSet<u32> = uids.iter().copied().collect();
        let mut out = Vec::new();
        for set in uid_sets(uids, FLAG_CHUNK) {
            for f in self.uid_fetch(&set, "UID FLAGS")? {
                if let (true, Some(flags)) = (asked.contains(&f.uid), f.flags) {
                    out.push((f.uid, flags));
                }
            }
        }
        Ok(out)
    }

    /// Message `uid`'s whole source, `None` when the server no longer has it. A literal over
    /// [`MAX_LITERAL`] is refused before it is read ([`NetError::TooLarge`]).
    pub(super) fn fetch_message(&mut self, uid: u32) -> Result<Option<Vec<u8>>> {
        if uid == 0 {
            return Ok(None);
        }
        let got = self.uid_fetch(&uid.to_string(), "UID BODY.PEEK[]")?;
        Ok(got.into_iter().find(|f| f.uid == uid).and_then(|f| f.body))
    }

    /// `UID FETCH set (items)`.
    fn uid_fetch(&mut self, set: &str, items: &str) -> Result<Vec<Attrs>> {
        if set.is_empty() {
            return Ok(Vec::new());
        }
        let rs = self.cmd(&[Part::Text(&format!("UID FETCH {set} ({items})"))])?;
        let mut out: Vec<Attrs> = Vec::new();
        // Each UID's place in `out` (looked for in it, a big folder's flags were a pass over the
        // answer per message).
        let mut at: HashMap<u32, usize> = HashMap::new();
        for r in rs {
            let Response::Fetch(_, attrs) = r else { continue };
            let f = fetched(attrs);
            // Unsolicited FETCHes without a UID (flag pushes for other messages) are skipped.
            if f.uid == 0 {
                continue;
            }
            if let Some(existing) = at.get(&f.uid).and_then(|&i| out.get_mut(i)) {
                existing.flags = f.flags.or(existing.flags.take());
                existing.header = f.header.or(existing.header.take());
                existing.body = f.body.or(existing.body.take());
                existing.size = existing.size.max(f.size);
                existing.internal_date = existing.internal_date.or(f.internal_date);
            } else {
                at.insert(f.uid, out.len());
                out.push(f);
            }
        }
        Ok(out)
    }

    /// Add (`on`) or remove one flag on `uids`, silently, in chunks.
    pub(super) fn set_flag(&mut self, uids: &[u32], flag: &str, on: bool) -> Result<()> {
        let flag = checked_flag(flag)?;
        for set in uid_sets(uids, FLAG_CHUNK) {
            self.uid_store(&set, on, flag)?;
        }
        Ok(())
    }

    /// Add (`add`) or remove flags, silently.
    fn uid_store(&mut self, set: &str, add: bool, flags: &str) -> Result<()> {
        if set.is_empty() {
            return Ok(());
        }
        let cmd = format!("UID STORE {set} {}FLAGS.SILENT ({flags})", if add { "+" } else { "-" });
        self.cmd(&[Part::Text(&cmd)]).map(drop)
    }

    /// Move messages: `UID MOVE` (RFC 6851), or COPY, `\Deleted` and an expunge of those alone.
    pub(super) fn move_to(&mut self, uids: &[u32], dest: &str) -> Result<()> {
        let dest = mailbox_arg(dest);
        for set in uid_sets(uids, FLAG_CHUNK) {
            if self.has("MOVE") {
                self.cmd(&[Part::Text(&format!("UID MOVE {set} {dest}"))])?;
                continue;
            }
            self.cmd(&[Part::Text(&format!("UID COPY {set} {dest}"))])?;
            self.uid_store(&set, true, "\\Deleted")?;
            self.expunge(&set)?;
        }
        Ok(())
    }

    /// Delete messages for good: `\Deleted`, then an expunge of those alone.
    pub(super) fn delete(&mut self, uids: &[u32]) -> Result<()> {
        for set in uid_sets(uids, FLAG_CHUNK) {
            self.uid_store(&set, true, "\\Deleted")?;
            self.expunge(&set)?;
        }
        Ok(())
    }

    /// Expunge exactly `set` (UIDPLUS), or everything marked `\Deleted` without it.
    fn expunge(&mut self, set: &str) -> Result<()> {
        if self.has("UIDPLUS") {
            return self.cmd(&[Part::Text(&format!("UID EXPUNGE {set}"))]).map(drop);
        }
        // Plain EXPUNGE takes everything marked `\Deleted`: what another program marked and left
        // (Thunderbird's "just mark it as deleted") is unmarked for the moment, so it stays.
        let others = uid_set(&self.uid_search(&format!("DELETED NOT UID {set}"))?);
        self.uid_store(&others, false, "\\Deleted")?;
        let done = self.cmd(&[Part::Text("EXPUNGE")]).map(drop);
        self.uid_store(&others, true, "\\Deleted")?;
        done
    }

    /// Store a message in a folder, with `flags` (such as `\Seen`).
    pub(super) fn append(&mut self, wire: &str, flags: &[&str], raw: &[u8]) -> Result<()> {
        if raw.len() > MAX_LITERAL {
            return Err(NetError::TooLarge(format!("a message of {} bytes", raw.len())));
        }
        let flags = flags.iter().copied().map(checked_flag).collect::<Result<Vec<&str>>>()?.join(" ");
        let head = format!("APPEND {} ({flags}) ", mailbox_arg(wire));
        self.cmd(&[Part::Text(&head), Part::Literal(raw)]).map(drop)
    }
}

/// One FETCH response's attributes.
fn fetched(attrs: Vec<AttributeValue<'_>>) -> Attrs {
    let mut f = Attrs::default();
    for a in attrs {
        match a {
            AttributeValue::Uid(u) => f.uid = u,
            AttributeValue::Flags(fl) => f.flags = Some(fl.iter().map(|x| x.to_string()).collect()),
            AttributeValue::Rfc822Size(n) => f.size = n,
            AttributeValue::InternalDate(d) => f.internal_date = parse_internal_date(&d),
            AttributeValue::BodySection { section: None, data, .. } | AttributeValue::Rfc822(data) => f.body = data.map(|d| d.into_owned()),
            AttributeValue::BodySection { section: Some(_), data, .. } | AttributeValue::Rfc822Header(data) => {
                f.header = data.map(|d| d.into_owned())
            }
            _ => {}
        }
    }
    f
}
