//! Import and export (PLAN.md, section 14.1, C6).
//!
//! **Export**, from the local cache:
//! - a message as `.eml`: its source as the server sent it, byte for byte ([`eml::export`]);
//! - a folder as `.mbox` (mboxrd: `From ` lines escaped, a `From_` line with the sender and the
//!   date before each message, [`mbox::export`]);
//! - a message as PDF: its header and its text on A4 pages ([`pdf::render`], with egui's own font
//!   embedded; no system file is read).
//!
//! **Import**, from files that are not trusted, with the caps mail reception has ([`MAX_MESSAGE`],
//! [`MAX_ITEMS`], [`MAX_DEPTH`]):
//! - `.eml` files, or a folder tree of them ([`eml::import_files`], [`eml::import_tree`]);
//! - `.mbox` files (mboxo or mboxrd), or a Thunderbird profile's folder tree (`Inbox`,
//!   `Inbox.sbd/…`, [`mbox::import_file`], [`mbox::import_tree`]);
//! - Outlook data files, `.pst` and `.ost` ([`pst::import`]), read only: each mail item becomes a
//!   standard message again, rebuilt from its properties when the file keeps no original header.
//! - **eM Client**: its databases are not read. eM Client exports a folder, or all of them, with
//!   *File → Export* as `.eml` files or `.mbox` files, which are imported as above
//!   ([`EM_CLIENT_HELP`]).
//!
//! An import hands each message to a [`Sink`] as an [`Item`] (its folder's path and its source),
//! with its [`Progress`] every so often, and stops when [`Cancel`] is raised. Where the messages go
//! is the app's choice: a local folder of the cache ([`add_to_store`]), or a server's folder.

pub mod eml;
pub mod mbox;
pub mod pdf;
pub mod pst;
mod walk;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mailalm_model::Flags;
use mailalm_store::Store;

/// The largest message imported, as the IMAP reception takes at most (`MAX_LITERAL` in
/// `mailalm-net`): a larger one is left out and counted.
pub const MAX_MESSAGE: usize = 64 << 20;
/// Messages one import hands over at most: past it the import stops, and says so.
pub const MAX_ITEMS: u64 = 1_000_000;
/// Folders nested deeper than this are not read (a folder tree, a data file's folders).
pub const MAX_DEPTH: usize = 32;
/// Folders one import reads at most.
pub const MAX_FOLDERS: usize = 10_000;
/// Files one folder tree is read for at most.
pub const MAX_FILES: usize = 100_000;
/// The longest folder name kept, in characters.
const MAX_NAME: usize = 255;
/// How often the progress is told, in messages.
const PROGRESS_EVERY: u64 = 32;

/// What the app tells the user who wants to bring their eM Client mail into MailALM.
pub const EM_CLIENT_HELP: &str = "MailALM does not read eM Client's own files. In eM Client, choose File → Export, then \"EML files\" or \"MBOX files\" (one folder, or all of them), and import what it wrote: a folder of .eml files with \"Import .eml\", or the .mbox files with \"Import .mbox\".";

/// One message read from a file: where it was, its source, and its marks when the file keeps
/// them (an Outlook item's read and flagged marks; `seen` for the others).
#[derive(Clone, PartialEq, Eq)]
pub struct Item {
    /// The folder's path in what was imported, from its top (each name cleaned: no control
    /// characters, no `/` or `\`, at most 255 characters); empty for messages at the top.
    pub folder: Vec<String>,
    /// The message (RFC 5322), at most [`MAX_MESSAGE`] bytes.
    pub raw: Vec<u8>,
    pub flags: Flags,
}

impl fmt::Debug for Item {
    /// Without the message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Item").field("folder", &self.folder).field("raw", &self.raw.len()).field("flags", &self.flags).finish()
    }
}

/// How far an import has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Messages handed over, and those left out.
    pub messages: u64,
    pub skipped: u64,
    /// The work done and the work there is: bytes for `.eml` and `.mbox` files, items for a data
    /// file (whose `total` grows as its folders are listed).
    pub done: u64,
    pub total: u64,
}

/// Where an import's messages go.
pub trait Sink {
    /// One message. An error stops the import, which returns it.
    fn message(&mut self, item: Item) -> Result<(), String>;
    /// How far the import has got, every so often and at its end.
    fn progress(&mut self, _progress: &Progress) {}
}

impl<F: FnMut(Item) -> Result<(), String>> Sink for F {
    fn message(&mut self, item: Item) -> Result<(), String> {
        self(item)
    }
}

/// Raised from another thread to stop an import (or an export): what was handed over stays.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Why a message was left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skip {
    /// Larger than [`MAX_MESSAGE`].
    TooLarge,
    /// Not a message (a file that is not one, an empty entry of an mbox).
    NotAMessage,
    /// Damaged: it could not be read.
    Unreadable,
    /// An Outlook item that is not mail (a contact, an appointment, a task, a note).
    NotMail,
    /// A message Thunderbird had deleted but still kept in its file (until the folder is
    /// compacted).
    Deleted,
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Messages handed over.
    pub messages: u64,
    /// Messages left out, by reason.
    pub skipped: BTreeMap<Skip, u64>,
    /// Folders read.
    pub folders: u64,
    /// Folders, files or attachments not read because a cap was reached (depth, count, size),
    /// or that could not be read; the messages that came in are complete otherwise.
    pub left_out: u64,
    /// Stopped by [`Cancel`].
    pub cancelled: bool,
    /// Stopped at [`MAX_ITEMS`].
    pub stopped_at_limit: bool,
}

impl Report {
    /// Messages left out, all reasons together.
    pub fn skipped_total(&self) -> u64 {
        self.skipped.values().sum()
    }

    pub(crate) fn skip(&mut self, why: Skip) {
        *self.skipped.entry(why).or_insert(0) += 1;
    }
}

/// An import under way: its report and its progress, and the rules every importer follows
/// (cancelled, the cap on items, the progress told every so often).
pub(crate) struct Run<'a, 's> {
    sink: &'a mut (dyn Sink + 's),
    cancel: &'a Cancel,
    pub(crate) report: Report,
    pub(crate) progress: Progress,
}

impl<'a, 's> Run<'a, 's> {
    pub(crate) fn new(sink: &'a mut (dyn Sink + 's), cancel: &'a Cancel) -> Self {
        Run { sink, cancel, report: Report::default(), progress: Progress::default() }
    }

    /// Whether to stop now (cancelled, or the cap on items reached); the report says why.
    pub(crate) fn stopped(&mut self) -> bool {
        if self.cancel.is_cancelled() {
            self.report.cancelled = true;
        }
        if self.report.messages >= MAX_ITEMS {
            self.report.stopped_at_limit = true;
        }
        self.report.cancelled || self.report.stopped_at_limit
    }

    /// Hands a message over.
    pub(crate) fn message(&mut self, item: Item) -> Result<(), String> {
        self.sink.message(item)?;
        self.report.messages += 1;
        self.progress.messages = self.report.messages;
        self.tick();
        Ok(())
    }

    /// Leaves a message out.
    pub(crate) fn skip(&mut self, why: Skip) {
        self.report.skip(why);
        self.progress.skipped = self.report.skipped_total();
        self.tick();
    }

    fn tick(&mut self) {
        if (self.progress.messages + self.progress.skipped).is_multiple_of(PROGRESS_EVERY) {
            self.sink.progress(&self.progress);
        }
    }

    /// Tells the progress now, and gives the report.
    pub(crate) fn finish(self) -> Report {
        self.sink.progress(&self.progress);
        self.report
    }
}

/// A folder's name as an [`Item`] keeps it: control and invisible characters removed, `/` and `\`
/// made `-`, at most [`MAX_NAME`] characters; `Folder` when nothing is left.
pub fn folder_name(name: &str) -> String {
    let line = mailalm_mime::clean_line(name);
    let s: String = line.chars().map(|c| if matches!(c, '/' | '\\') { '-' } else { c }).take(MAX_NAME).collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "Folder".to_owned() } else { s }
}

/// Adds imported messages to `folder` of an account's cache, a folder kept only on this computer
/// (see [`Store::import_messages`]): each is read for its summary and its search text as received
/// mail is. Their new UIDs.
pub fn add_to_store(store: &Store, folder: &str, messages: Vec<(Vec<u8>, Flags)>) -> io::Result<Vec<u32>> {
    let batch = messages
        .into_iter()
        .map(|(raw, flags)| {
            let size = u32::try_from(raw.len()).unwrap_or(u32::MAX);
            let mut summary = mailalm_mime::summary_from_header(0, &raw, flags, size, None);
            let text = match mailalm_mime::parse(&raw) {
                Some(parsed) => {
                    mailalm_mime::complete_summary(&mut summary, &parsed);
                    parsed.text
                }
                None => String::new(),
            };
            (raw, summary, text)
        })
        .collect();
    store.import_messages(folder, batch)
}

#[cfg(test)]
mod tests;
