//! Messages waiting to go (C1, PLAN.md section 8):
//!
//! - [`UndoQueue`]: a message sent waits a few seconds (5 to 30, set in Settings) with a countdown
//!   and an Undo button that brings its window back; then it goes to the account's worker. Kept in
//!   memory only: a message still waiting when MailALM closes is not sent.
//! - [`Schedule`]: a message to send at a date and time, kept in the cache folder
//!   (`scheduled.json`, attachments included) until then. MailALM sends it at that time if it is
//!   running; one whose time passed while MailALM was closed is sent at the next start, and the
//!   interface says so. No provider's own deferred delivery is used.

use std::path::PathBuf;

use mailalm_mime::{HtmlBody, InlineImage, Outgoing, OutgoingAttachment};
use mailalm_model::{AccountId, Person};
use serde::{Deserialize, Serialize};

use crate::i18n::trf;

/// The undo delay's bounds and default, in seconds.
pub const UNDO_MIN: u32 = 5;
pub const UNDO_MAX: u32 = 30;
pub const UNDO_DEFAULT: u32 = 10;

/// The undo delay set (0: never set): within [`UNDO_MIN`]..=[`UNDO_MAX`].
pub fn undo_delay(set: u32) -> u32 {
    if set == 0 { UNDO_DEFAULT } else { set.clamp(UNDO_MIN, UNDO_MAX) }
}

/// One message waiting for its undo delay to pass.
pub struct Pending<T> {
    pub id: u64,
    /// When it goes, on the interface's clock (seconds).
    pub due: f64,
    pub item: T,
}

/// The messages waiting for their undo delay, in the order they were sent.
pub struct UndoQueue<T> {
    items: Vec<Pending<T>>,
    next: u64,
}

impl<T> Default for UndoQueue<T> {
    fn default() -> Self {
        UndoQueue { items: Vec::new(), next: 1 }
    }
}

impl<T> UndoQueue<T> {
    /// Puts `item` in the queue at `now`, to go `delay` seconds later: its id.
    pub fn push(&mut self, item: T, now: f64, delay: f64) -> u64 {
        let id = self.next;
        self.next += 1;
        self.items.push(Pending { id, due: now + delay.max(0.0), item });
        id
    }

    /// The seconds left before `id` goes.
    pub fn remaining(&self, id: u64, now: f64) -> Option<f64> {
        self.items.iter().find(|p| p.id == id).map(|p| (p.due - now).max(0.0))
    }

    /// Takes out what is due at `now`, oldest first.
    pub fn take_due(&mut self, now: f64) -> Vec<T> {
        let (due, kept): (Vec<Pending<T>>, Vec<Pending<T>>) = std::mem::take(&mut self.items).into_iter().partition(|p| p.due <= now);
        self.items = kept;
        due.into_iter().map(|p| p.item).collect()
    }

    /// Takes `id` out (Undo, or Send now).
    pub fn take(&mut self, id: u64) -> Option<T> {
        let at = self.items.iter().position(|p| p.id == id)?;
        Some(self.items.remove(at).item)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Pending<T>> {
        self.items.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

// ---- Scheduled messages ----

/// A message to send later, as it is kept on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub from: Person,
    #[serde(default)]
    pub to: Vec<Person>,
    #[serde(default)]
    pub cc: Vec<Person>,
    #[serde(default)]
    pub bcc: Vec<Person>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<StoredPart>,
    #[serde(default)]
    pub html: Option<String>,
    /// The HTML's pictures (`name` is the content id).
    #[serde(default)]
    pub images: Vec<StoredPart>,
}

/// A file or a picture, its bytes in base64.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPart {
    pub name: String,
    pub content_type: String,
    pub base64: String,
}

impl Stored {
    pub fn of(o: &Outgoing) -> Stored {
        let part = |name: &str, content_type: &str, bytes: &[u8]| StoredPart { name: name.to_owned(), content_type: content_type.to_owned(), base64: base64_encode(bytes) };
        Stored {
            from: o.from.clone(),
            to: o.to.clone(),
            cc: o.cc.clone(),
            bcc: o.bcc.clone(),
            subject: o.subject.clone(),
            text: o.text.clone(),
            in_reply_to: o.in_reply_to.clone(),
            references: o.references.clone(),
            attachments: o.attachments.iter().map(|a| part(&a.name, &a.content_type, &a.bytes)).collect(),
            html: o.html.as_ref().map(|h| h.html.clone()),
            images: o.html.as_ref().map(|h| h.images.iter().map(|i| part(&i.content_id, &i.content_type, &i.bytes)).collect()).unwrap_or_default(),
        }
    }

    /// The message to send; `None` if a part's bytes are damaged.
    pub fn outgoing(&self) -> Option<Outgoing> {
        let attachments = self
            .attachments
            .iter()
            .map(|p| Some(OutgoingAttachment { name: p.name.clone(), content_type: p.content_type.clone(), bytes: base64_decode(&p.base64)? }))
            .collect::<Option<Vec<_>>>()?;
        let html = match &self.html {
            Some(html) => {
                let images = self
                    .images
                    .iter()
                    .map(|p| Some(InlineImage { content_id: p.name.clone(), content_type: p.content_type.clone(), bytes: base64_decode(&p.base64)? }))
                    .collect::<Option<Vec<_>>>()?;
                Some(HtmlBody { html: html.clone(), images })
            }
            None => None,
        };
        Some(Outgoing {
            from: self.from.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            text: self.text.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
            attachments,
            calendar: None,
            html,
        })
    }
}

/// A message scheduled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheduled {
    pub id: u64,
    pub account: AccountId,
    /// When to send it (UTC seconds).
    pub at: i64,
    pub message: Stored,
    /// The draft of it kept on the server, deleted once it is sent.
    #[serde(default)]
    pub draft: Option<String>,
}

/// The scheduled messages, kept in a file.
#[derive(Debug, Default)]
pub struct Schedule {
    items: Vec<Scheduled>,
    path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct File {
    #[serde(default)]
    items: Vec<Scheduled>,
}

impl Schedule {
    /// The messages kept in `path`; a damaged file is set aside (`.damaged`) and said.
    pub fn load(path: PathBuf) -> (Schedule, Option<String>) {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Schedule { items: Vec::new(), path: Some(path) }, None),
            Err(e) => return (Schedule { items: Vec::new(), path: Some(path) }, Some(trf!("The scheduled messages could not be read: {error}", error = e))),
        };
        match serde_json::from_slice::<File>(&bytes) {
            Ok(f) => (Schedule { items: f.items, path: Some(path) }, None),
            Err(e) => {
                let mut aside = path.clone().into_os_string();
                aside.push(".damaged");
                let _ = std::fs::rename(&path, PathBuf::from(aside));
                (Schedule { items: Vec::new(), path: Some(path) }, Some(trf!("The scheduled messages' file was damaged and was set aside: {error}", error = e)))
            }
        }
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| trf!("The scheduled messages could not be saved: {error}", error = e))?;
        }
        let bytes = serde_json::to_vec(&File { items: self.items.clone() }).map_err(|e| trf!("The scheduled messages could not be saved: {error}", error = e))?;
        mailalm_store::write_atomic(path, &bytes).map_err(|e| trf!("The scheduled messages could not be saved: {error}", error = e))
    }

    pub fn list(&self) -> &[Scheduled] {
        &self.items
    }

    /// Schedules `message` from `account` at `at`; kept on disk before this returns.
    pub fn add(&mut self, account: AccountId, at: i64, message: &Outgoing, draft: Option<String>) -> Result<u64, String> {
        let id = self.items.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        self.items.push(Scheduled { id, account, at, message: Stored::of(message), draft });
        self.items.sort_by_key(|s| s.at);
        if let Err(e) = self.save() {
            self.items.retain(|s| s.id != id);
            return Err(e);
        }
        Ok(id)
    }

    /// Takes out the messages due at `now` (the file is written again).
    pub fn take_due(&mut self, now: i64) -> Result<Vec<Scheduled>, String> {
        if !self.items.iter().any(|s| s.at <= now) {
            return Ok(Vec::new());
        }
        let (due, kept) = split_due(std::mem::take(&mut self.items), now);
        self.items = kept;
        self.save()?;
        Ok(due)
    }

    /// Takes one out (to edit it again, or send it now).
    pub fn take(&mut self, id: u64) -> Result<Option<Scheduled>, String> {
        let Some(at) = self.items.iter().position(|s| s.id == id) else { return Ok(None) };
        let item = self.items.remove(at);
        self.save()?;
        Ok(Some(item))
    }

    /// Forgets an account's messages (the account was removed).
    pub fn forget(&mut self, account: AccountId) -> Result<(), String> {
        let before = self.items.len();
        self.items.retain(|s| s.account != account);
        if self.items.len() != before { self.save() } else { Ok(()) }
    }
}

/// The messages due at `now` (oldest first) and the others.
pub fn split_due(items: Vec<Scheduled>, now: i64) -> (Vec<Scheduled>, Vec<Scheduled>) {
    let (mut due, kept): (Vec<Scheduled>, Vec<Scheduled>) = items.into_iter().partition(|s| s.at <= now);
    due.sort_by_key(|s| s.at);
    (due, kept)
}

/// Whether a message due at `at` and sent at `now` is late: its time passed while MailALM was
/// closed (more than two minutes ago), which the interface says.
pub fn sent_late(at: i64, now: i64) -> bool {
    now - at > 120
}

// ---- base64 (RFC 4648), for the bytes kept in the file ----

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let index = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap_or(0);
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| ALPHABET.iter().position(|a| *a == c).and_then(|p| u32::try_from(p).ok());
    let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if clean.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for chunk in clean.chunks(4) {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { value(*c)? };
            n = (n << 6) | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(bytes.get(..3 - pad)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_undo_delay_is_kept_in_bounds() {
        assert_eq!((undo_delay(0), undo_delay(1), undo_delay(12), undo_delay(99)), (10, 5, 12, 30));
    }

    #[test]
    fn a_message_waits_its_delay_and_can_be_taken_back() {
        let mut q: UndoQueue<&str> = UndoQueue::default();
        let a = q.push("a", 100.0, 10.0);
        let b = q.push("b", 103.0, 10.0);
        assert_eq!(q.remaining(a, 104.0), Some(6.0));
        assert!(q.take_due(109.9).is_empty());
        assert_eq!(q.take_due(110.0), vec!["a"]);
        assert_eq!(q.remaining(a, 110.0), None);
        assert_eq!(q.take(b), Some("b"), "undone");
        assert!(q.take_due(1000.0).is_empty() && q.is_empty());
        let c = q.push("c", 0.0, 30.0);
        assert_ne!(c, a);
        assert_eq!(q.remaining(c, 40.0), Some(0.0));
    }

    fn message() -> Outgoing {
        Outgoing {
            from: Person { name: "Alex".into(), address: "alex@videotron.ca".into() },
            to: vec![Person { name: String::new(), address: "zoe@studio.example".into() }],
            bcc: vec![Person { name: String::new(), address: "boss@example.com".into() }],
            subject: "Plan".into(),
            text: "See the plan.".into(),
            in_reply_to: Some("a@b".into()),
            references: vec!["a@b".into()],
            attachments: vec![OutgoingAttachment::new("plan.pdf", b"%PDF-1.7 \x00\xff\x10".to_vec())],
            html: Some(HtmlBody {
                html: "<p>See the <b>plan</b>.</p>".into(),
                images: vec![InlineImage { content_id: "p@x".into(), content_type: "image/png".into(), bytes: vec![137, 80, 78, 71, 0, 1, 2] }],
            }),
            ..Outgoing::default()
        }
    }

    #[test]
    fn a_message_is_kept_whole() {
        let m = message();
        assert_eq!(Stored::of(&m).outgoing(), Some(m));
        let mut damaged = Stored::of(&message());
        damaged.attachments[0].base64 = "!!!".into();
        assert_eq!(damaged.outgoing(), None);
    }

    #[test]
    fn scheduled_messages_survive_a_restart_and_go_when_due() {
        let dir = std::env::temp_dir().join(format!("mailalm-schedule-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("scheduled.json");
        let (mut s, problem) = Schedule::load(path.clone());
        assert!(problem.is_none() && s.list().is_empty());
        let late = s.add(AccountId(2), 2_000, &message(), Some("r-1".into())).expect("kept");
        let early = s.add(AccountId(1), 1_000, &message(), None).expect("kept");
        assert_eq!(s.list().iter().map(|x| x.id).collect::<Vec<_>>(), vec![early, late], "in time order");

        // MailALM closed and started again at 1 500: the first one is due, and late.
        let (mut s, problem) = Schedule::load(path.clone());
        assert!(problem.is_none());
        let due = s.take_due(1_500).expect("written");
        assert_eq!(due.iter().map(|x| x.id).collect::<Vec<_>>(), vec![early]);
        assert!(sent_late(due[0].at, 1_500) && !sent_late(2_000, 2_060));
        assert_eq!(due[0].message.outgoing(), Some(message()));
        let (mut s, _) = Schedule::load(path.clone());
        assert_eq!(s.list().len(), 1, "the one sent is no longer kept");
        assert!(s.take_due(1_999).expect("written").is_empty());
        assert_eq!(s.take(late).expect("written").map(|x| x.draft), Some(Some("r-1".into())));
        assert!(s.list().is_empty());

        // A damaged file is set aside, never a crash.
        std::fs::write(&path, b"{not json").expect("written");
        let (s, problem) = Schedule::load(path.clone());
        assert!(s.list().is_empty() && problem.is_some());
        assert!(dir.join("scheduled.json.damaged").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn due_messages_come_oldest_first() {
        let item = |id: u64, at: i64| Scheduled { id, account: AccountId(1), at, message: Stored::default(), draft: None };
        let (due, kept) = split_due(vec![item(1, 30), item(2, 10), item(3, 50), item(4, 20)], 30);
        assert_eq!(due.iter().map(|s| s.id).collect::<Vec<_>>(), vec![2, 4, 1]);
        assert_eq!(kept.iter().map(|s| s.id).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn base64_both_ways() {
        for (raw, text) in [(&b""[..], ""), (b"f", "Zg=="), (b"fo", "Zm8="), (b"foo", "Zm9v"), (b"foobar", "Zm9vYmFy"), (b"\xff\xfe\x00", "//4A")] {
            assert_eq!(base64_encode(raw), text);
            assert_eq!(base64_decode(text).as_deref(), Some(raw));
        }
        assert_eq!(base64_decode("Zm9"), None);
        assert_eq!(base64_decode("Z==="), None);
        assert_eq!(base64_decode("Zm!v"), None);
    }
}
