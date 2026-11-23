// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/imap.rs (with `quote` from crates/net/src/protocols.rs and `days_from_civil` from crates/model/src/dates.rs). MIT OR Apache-2.0.
// Changed: MailALM's blocking `ImapSession` over a private generic client; TLS or required STARTTLS only (PREAUTH in the clear refused); no QRESYNC, quota, IDLE or new-mail tracking.

//! A small IMAP4rev1 client (RFC 3501).
//!
//! Framing is done here ([`conn`]): a response is one line plus any `{n}` literals it announces.
//! Parsing is `imap-proto`'s. Server input is hostile: line and literal sizes and each command's
//! responses are capped, and nothing indexes unchecked.

use std::collections::HashSet;
use std::io::{Read, Write};

use imap_proto::{Capability, Response, ResponseCode, Status};
use mailalm_model::{Folder, Security, Server};

use crate::stream::{self, Stream};
use crate::{NetError, Result, clip};

mod conn;
mod folders;
mod messages;
mod quotes;
mod sign_in;
mod utf7;
use conn::Conn;

/// Largest literal accepted (a message source), and largest message appended.
const MAX_LITERAL: usize = 64 << 20;

/// What `SELECT` reports about a folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mailbox {
    pub uidvalidity: u32,
    /// The UID the next message will get; 0 if the server didn't say.
    pub uidnext: u32,
    /// How many messages the folder holds.
    pub exists: u32,
}

/// One message's header and what the list needs about it, from `UID FETCH`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fetched {
    pub uid: u32,
    pub flags: Vec<String>,
    /// The size on the server, in bytes (`RFC822.SIZE`).
    pub size: u32,
    /// When the server received it (`INTERNALDATE`), in Unix seconds.
    pub internal_date: Option<i64>,
    /// The message's header, as the server sent it.
    pub header: Vec<u8>,
}

/// A signed-in IMAP session over TLS. Blocking: for a worker thread.
pub struct ImapSession {
    client: Client<Stream>,
}

impl ImapSession {
    /// TLS or STARTTLS (STARTTLS required: fail if not offered), then sign-in with LOGIN or
    /// AUTHENTICATE PLAIN, never over clear text.
    pub fn connect(server: &Server, login: &str, password: &str) -> Result<Self> {
        let mut client = Client::connect(server)?;
        // Never reached otherwise; checked all the same before the password goes out.
        if !client.conn.stream().is_tls() {
            return Err(NetError::Tls("the connection is not encrypted".into()));
        }
        if !client.preauth {
            client.login(login, password)?;
        }
        Ok(ImapSession { client })
    }

    /// `LIST "" "*"` (with SPECIAL-USE roles when the server has them, else
    /// [`FolderRole::from_name`](mailalm_model::FolderRole::from_name)); `\Noselect` and
    /// `\NonExistent` folders are left out; `path` is the name decoded from modified UTF-7, `wire`
    /// the name as the server knows it; `total` and `unread` are 0.
    pub fn folders(&mut self) -> Result<Vec<Folder>> {
        self.client.folders()
    }

    /// `STATUS (MESSAGES UNSEEN)`: (messages, unseen).
    pub fn status(&mut self, wire: &str) -> Result<(u32, u32)> {
        self.client.status(wire)
    }

    /// `SELECT` (read-write).
    pub fn select(&mut self, wire: &str) -> Result<Mailbox> {
        self.client.select(wire)
    }

    /// `UID SEARCH ALL` in the selected folder: its UIDs, in order, each once.
    pub fn uids(&mut self) -> Result<Vec<u32>> {
        self.client.uids()
    }

    /// `UID FETCH <set> (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])`, in chunks; a UID
    /// the server no longer has is just absent.
    pub fn fetch_headers(&mut self, uids: &[u32]) -> Result<Vec<Fetched>> {
        self.client.fetch_headers(uids)
    }

    /// `UID FETCH <set> (UID FLAGS)`, in chunks.
    pub fn fetch_flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>> {
        self.client.fetch_flags(uids)
    }

    /// `UID FETCH uid (BODY.PEEK[])`: the whole message, `None` if it is gone. More than 64 MiB is
    /// refused ([`NetError::TooLarge`], after which the session is to be dropped).
    pub fn fetch_message(&mut self, uid: u32) -> Result<Option<Vec<u8>>> {
        self.client.fetch_message(uid)
    }

    /// `UID STORE +FLAGS.SILENT` (`on`) or `-FLAGS.SILENT`, one flag (`\Seen`, `\Flagged`…).
    pub fn set_flag(&mut self, uids: &[u32], flag: &str, on: bool) -> Result<()> {
        self.client.set_flag(uids, flag, on)
    }

    /// `UID MOVE` when the server has MOVE; else `UID COPY`, `+FLAGS \Deleted`, then `UID EXPUNGE`
    /// (UIDPLUS) or `EXPUNGE` sparing what others marked deleted.
    pub fn move_to(&mut self, uids: &[u32], dest_wire: &str) -> Result<()> {
        self.client.move_to(uids, dest_wire)
    }

    /// `+FLAGS \Deleted`, then `UID EXPUNGE` (UIDPLUS) or `EXPUNGE` sparing what others marked
    /// deleted.
    pub fn delete(&mut self, uids: &[u32]) -> Result<()> {
        self.client.delete(uids)
    }

    /// `APPEND` with flags (such as `\Seen`), for the Sent folder.
    pub fn append(&mut self, wire: &str, flags: &[&str], raw: &[u8]) -> Result<()> {
        self.client.append(wire, flags, raw)
    }

    pub fn noop(&mut self) -> Result<()> {
        self.client.noop()
    }

    /// `LOGOUT`, whatever the server answers; the connection closes.
    pub fn logout(mut self) {
        self.client.logout();
    }
}

/// A piece of a command: plain text, or a literal sent after the server's `+` go-ahead.
enum Part<'a> {
    Text(&'a str),
    Literal(&'a [u8]),
}

/// A text protocol's quoted string: in double quotes, with `\` and `"` escaped.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// An IMAP `astring` argument: quoted when printable ASCII, else none (it goes as a literal,
/// 8-bit allowed).
fn quoted(s: &str) -> Option<String> {
    s.chars().all(|c| (' '..='~').contains(&c)).then(|| quote(s))
}

/// Mailbox names are 7-bit (modified UTF-7) and go quoted. One a server listed in UTF-8 goes
/// quoted too, whole: its letters outside ASCII were left out, which named another mailbox
/// (`Büro` became `Bro`). Control characters never go: a line end would start a command.
fn mailbox_arg(name: &str) -> String {
    quoted(name).unwrap_or_else(|| quote(&name.chars().filter(|c| !c.is_control()).collect::<String>()))
}

/// Compress UIDs into an IMAP sequence set (`1:4,7,9:12`).
fn uid_set(uids: &[u32]) -> String {
    let mut v: Vec<u32> = uids.iter().copied().filter(|u| *u > 0).collect();
    v.sort_unstable();
    v.dedup();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&start) = v.get(i) {
        let mut end = start;
        while v.get(i + 1).is_some_and(|&n| Some(n) == end.checked_add(1)) {
            i += 1;
            end = v.get(i).copied().unwrap_or(end);
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
        i += 1;
    }
    out
}

/// UIDs (0 left out), sorted, each once, as sequence sets of at most `n` UIDs each: a long list
/// goes in several commands, none of them a line too long for the server.
fn uid_sets(uids: &[u32], n: usize) -> Vec<String> {
    let mut v: Vec<u32> = uids.iter().copied().filter(|u| *u > 0).collect();
    v.sort_unstable();
    v.dedup();
    v.chunks(n.max(1)).map(uid_set).collect()
}

/// Parse an INTERNALDATE (`14-Oct-2026 09:30:00 +0200`) to Unix seconds. The server's numbers
/// are digits only, the year from 1 to 9999 (RFC 3501 gives it four digits).
fn parse_internal_date(s: &str) -> Option<i64> {
    // Digits only: `parse` alone takes a sign, and an unbounded year overflowed the day count.
    let num = |x: &str| -> Option<i64> { (!x.is_empty() && x.len() <= 4 && x.bytes().all(|b| b.is_ascii_digit())).then(|| x.parse().ok())? };
    let s = s.trim();
    let (date, rest) = s.split_once(' ')?;
    let mut d = date.trim().split('-');
    let day = num(d.next()?.trim())?;
    let mon = d.next()?;
    let year = num(d.next()?)?;
    let month =
        ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| m.eq_ignore_ascii_case(mon))? as u32
            + 1;
    let mut parts = rest.split_whitespace();
    let time = parts.next()?;
    let zone = parts.next().unwrap_or("+0000");
    let mut t = time.split(':');
    let h = num(t.next()?)?;
    let m = num(t.next()?)?;
    let sec = num(t.next().unwrap_or("0"))?;
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let z = zone.trim_start_matches(['+', '-']);
    let zh = num(z.get(..2)?)?;
    let zm = num(z.get(2..4)?)?;
    if !(1..=31).contains(&day) || year < 1 || h > 23 || m > 59 || sec > 60 {
        return None;
    }
    let day = u32::try_from(day).ok()?;
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + h * 3600 + m * 60 + sec - sign * (zh * 3600 + zm * 60))
}

/// (year, month, day) → days since 1970-01-01. A year too far out for an `i64` of days gives the
/// nearest end (`i64::MIN` or `i64::MAX`), never a wrapped day: callers that read a year from a
/// server bound it themselves.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y.saturating_sub(1) } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m.clamp(1, 12));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d.clamp(1, 31)) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let far = if era < 0 { i64::MIN } else { i64::MAX };
    era.checked_mul(146_097).and_then(|e| e.checked_add(doe - 719_468)).unwrap_or(far)
}

/// A status response's text, cut to a length an error message can hold.
fn info(o: &imap_proto::Outcome<'_>) -> String {
    clip(o.information.as_deref().unwrap_or(""))
}

/// An IMAP session, signed in or not yet, on any stream (the tests' is in memory).
struct Client<S> {
    conn: Conn<S>,
    caps: HashSet<String>,
    /// The server greeted with `PREAUTH`: signed in already, with no password asked.
    preauth: bool,
}

fn caps_from(list: &[Capability<'_>]) -> HashSet<String> {
    list.iter()
        .map(|c| match c {
            Capability::Imap4rev1 => "IMAP4REV1".to_string(),
            Capability::Auth(a) => format!("AUTH={}", a.to_ascii_uppercase()),
            Capability::Atom(a) => a.to_ascii_uppercase(),
        })
        .collect()
}

impl Client<Stream> {
    /// Connect and read the greeting, upgrading with STARTTLS when the server is set so.
    fn connect(server: &Server) -> Result<Client<Stream>> {
        let sock = stream::tcp(&server.host, server.port)?;
        match server.security {
            Security::Tls => Client::from_stream(stream::tls(sock, &server.host)?),
            Security::StartTls => {
                let mut c = Client::from_stream(Stream::Plain(sock))?;
                c.ask_starttls()?;
                let mut c = Client::new(stream::starttls(c.conn.into_inner(), &server.host)?);
                // Capabilities learned before TLS must be discarded (RFC 3501 §6.2.1).
                c.capability()?;
                Ok(c)
            }
        }
    }
}

impl<S: Read + Write> Client<S> {
    /// A session on `s` that knows nothing of the server yet.
    fn new(s: S) -> Client<S> {
        Client { conn: Conn::new(s), caps: HashSet::new(), preauth: false }
    }

    /// Wrap a connected stream and read the server greeting.
    fn from_stream(s: S) -> Result<Client<S>> {
        let mut c = Client::new(s);
        // The greeting has the same deadline and budget as a command.
        c.conn.begin();
        match c.conn.read()? {
            Ok(Response::Data { status, outcome }) if matches!(status, Status::Ok | Status::PreAuth) => {
                c.preauth = status == Status::PreAuth;
                if let Some(ResponseCode::Capabilities(list)) = &outcome.code {
                    c.caps = caps_from(list);
                }
                Ok(c)
            }
            Ok(Response::Data { status: Status::Bye, outcome }) => Err(NetError::Closed(info(&outcome))),
            Ok(other) => Err(NetError::Protocol(format!("unexpected greeting: {}", clip(&format!("{other:?}"))))),
            Err(raw) => Err(NetError::Protocol(format!("unexpected greeting: {}", clip(&String::from_utf8_lossy(&raw))))),
        }
    }

    /// STARTTLS asked for on the connection in the clear, before anything else is said: refused
    /// when the server doesn't offer it (never a fall back to the clear), when it says it signed
    /// in already (`PREAUTH` in the clear), and when anything came after its go-ahead (plain text
    /// that would be read as if it came over TLS).
    fn ask_starttls(&mut self) -> Result<()> {
        if self.preauth {
            return Err(NetError::Tls("the server signed in before STARTTLS (PREAUTH in the clear)".into()));
        }
        if !self.has("STARTTLS") {
            self.capability()?;
        }
        if !self.has("STARTTLS") {
            return Err(NetError::Tls("the server does not offer STARTTLS".into()));
        }
        self.cmd(&[Part::Text("STARTTLS")])?;
        if self.conn.unread() != 0 {
            return Err(NetError::Tls("unexpected data before TLS negotiation".into()));
        }
        Ok(())
    }

    fn has(&self, cap: &str) -> bool {
        self.caps.contains(&cap.to_ascii_uppercase())
    }

    fn capability(&mut self) -> Result<()> {
        let rs = self.cmd(&[Part::Text("CAPABILITY")])?;
        for r in rs {
            if let Response::Capabilities(list) = r {
                self.caps = caps_from(&list);
            }
        }
        Ok(())
    }

    /// Run a command: its untagged responses, then its tagged completion.
    fn cmd(&mut self, parts: &[Part<'_>]) -> Result<Vec<Response<'static>>> {
        self.conn.run(parts)
    }

    fn noop(&mut self) -> Result<()> {
        self.cmd(&[Part::Text("NOOP")]).map(drop)
    }

    fn logout(&mut self) {
        let _ = self.cmd(&[Part::Text("LOGOUT")]);
    }
}

#[cfg(test)]
mod tests;
