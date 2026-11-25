//! Mail kept on this PC only, and the imports and exports (C6, PLAN.md section 14.1).
//!
//! **On this PC** is a mailbox no server syncs: imported mail goes there, in a folder tree of its
//! own, kept in the cache under the reserved account number 0 (real accounts start at 1). Its
//! thread answers the same [`Command`]s as an account's worker, with the same [`Event`]s, so the
//! message list, the reading pane, the attachment review and the search work on it unchanged:
//! opening, marking, deleting, moving between its folders, searching. Nothing in it ever goes to
//! a server.
//!
//! **Imports** read files that are not trusted, through `mailalm_porting` (capped as received mail
//! is) on a thread of their own, with their progress and a Cancel; each message is read for its
//! summary and search text as received mail is ([`mailalm_porting::add_to_store`]). **Exports**
//! write from the cache: a message as `.eml` or PDF, a folder as `.mbox` (with its progress and a
//! Cancel).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use mailalm_engine::{Command, Event, PimCommand, Reviewed, Wake};
use mailalm_model::{AccountId, Change, Flags, Folder, FolderRole, FolderScope, Query, Summary, sort_folders};
use mailalm_porting::{Cancel, Item, Progress, Report, Sink};
use mailalm_store::Store;

use crate::i18n::{tr, tr_key, trf};

/// The account number of the mail kept on this PC (never a real account's: those start at 1).
pub const LOCAL: AccountId = AccountId(0);
/// The name the folder pane gives it: kept in English, as the search's `account:` operator matches
/// it, and shown with `tr(LOCAL_NAME)`.
pub const LOCAL_NAME: &str = tr_key("On this PC");
/// Its folders' delimiter.
const DELIMITER: char = '/';
/// Messages handed to the cache at once during an import.
const BATCH: usize = 200;
/// Bytes kept before they are handed to the cache.
const BATCH_BYTES: usize = 64 << 20;
/// The most results a search of this mailbox gives.
const MAX_HITS: usize = 1_000;

/// The mailbox's thread: commands in, events out (taken with the engine's).
pub struct LocalBox {
    commands: Sender<Command>,
    events: Receiver<Event>,
    base: PathBuf,
}

impl LocalBox {
    /// Starts its thread over the cache folder `base`; `wake` runs when an event waits.
    pub fn start(base: PathBuf, wake: Wake) -> LocalBox {
        let (commands, rx) = mpsc::channel::<Command>();
        let (tx, events) = mpsc::channel::<Event>();
        let dir = base.clone();
        let started = std::thread::Builder::new().name("mailalm-on-this-pc".into()).spawn(move || run(&dir, &rx, &tx, &wake));
        if let Err(e) = started {
            log::warn!("On this PC: no thread: {e}");
        }
        let b = LocalBox { commands, events, base };
        b.send(Command::SyncAll);
        b
    }

    /// Sends a command to the thread; `false` when it stopped.
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }

    /// The events waiting, oldest first.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Its cache.
    pub fn store(&self) -> std::io::Result<Store> {
        Store::open(&self.base, LOCAL)
    }

    /// A clone of the command channel, for an import to tell the thread when it is done.
    fn sender(&self) -> Sender<Command> {
        self.commands.clone()
    }

    /// Deletes a folder of this mailbox, the folders under it and their messages, on a thread of
    /// its own (an import can be large); the folder list is shown again after.
    pub fn delete_folder(&self, path: String, wake: Wake) -> Result<(), String> {
        let store = self.store().map_err(|e| trf!("The mail kept on this PC can't be opened: {error}", error = e))?;
        let back = self.sender();
        std::thread::Builder::new()
            .name("mailalm-delete-folder".into())
            .spawn(move || {
                if let Err(e) = delete_folder(&store, &path) {
                    log::warn!("On this PC: {e}");
                }
                let _ = back.send(Command::SyncAll);
                wake();
            })
            .map(drop)
            .map_err(|e| trf!("The folder could not be deleted: {error}", error = e))
    }
}

/// A folder of this mailbox as the folder list keeps it.
fn folder(path: &str) -> Folder {
    Folder { path: path.to_owned(), wire: path.to_owned(), delimiter: Some(DELIMITER), role: FolderRole::Other, total: 0, unread: 0 }
}

/// The folders with their counts, from their summaries.
fn folders_with_counts(store: &Store) -> Vec<Folder> {
    let mut list = store.folders();
    for f in &mut list {
        let s = store.summaries(&f.path);
        f.total = u32::try_from(s.len()).unwrap_or(u32::MAX);
        f.unread = u32::try_from(s.iter().filter(|m| !m.flags.seen).count()).unwrap_or(u32::MAX);
    }
    list
}

fn run(base: &Path, commands: &Receiver<Command>, events: &Sender<Event>, wake: &Wake) {
    let tell = |e: Event| {
        let _ = events.send(e);
        wake();
    };
    let store = match Store::open(base, LOCAL) {
        Ok(s) => s,
        Err(e) => {
            tell(Event::Error { account: LOCAL, text: trf!("The mail kept on this PC can't be opened: {error}", error = e) });
            return;
        }
    };
    let error = |text: String| Event::Error { account: LOCAL, text };
    while let Ok(command) = commands.recv() {
        match command {
            Command::Stop => return,
            Command::SyncAll | Command::Reconnect => tell(Event::Folders { account: LOCAL, folders: folders_with_counts(&store) }),
            Command::View(path) => {
                tell(Event::Summaries { account: LOCAL, folder: path.clone(), summaries: store.summaries(&path) });
            }
            Command::Open { folder, uid } => tell(open(&store, folder, uid)),
            Command::Apply(change) => {
                let touched: Vec<String> = match apply(&store, &change) {
                    Ok(t) => t,
                    Err(e) => {
                        tell(error(e));
                        vec![change.folder().to_owned()]
                    }
                };
                for path in touched {
                    tell(Event::Summaries { account: LOCAL, folder: path.clone(), summaries: store.summaries(&path) });
                }
                tell(Event::Folders { account: LOCAL, folders: folders_with_counts(&store) });
            }
            Command::Attachment { folder, uid, index } => tell(review(&store, folder, uid, index)),
            Command::Search { id, query } => tell(Event::SearchResults { account: LOCAL, id, hits: search(&store, &query, &FolderScope::Account) }),
            Command::Find { id, query, folders, server } => {
                tell(Event::SearchResults { account: LOCAL, id, hits: search(&store, &query, &folders) });
                if server {
                    tell(Event::ServerResults { account: LOCAL, id, result: Err(trf!("{place} has no server to search.", place = tr(LOCAL_NAME))) });
                }
            }
            Command::Pim(PimCommand::Invitation { folder, uid, index }) => tell(Event::Invitation {
                account: LOCAL,
                folder,
                uid,
                index,
                result: Err(tr("Invitations in mail kept on this PC are not read: answer them from the account they came to.").to_owned()),
            }),
            _ => tell(error(trf!("This can't be done with mail kept on this PC ({place}): it has no server.", place = tr(LOCAL_NAME)))),
        }
    }
}

/// A message opened from the cache: what the reading pane shows.
fn open(store: &Store, folder: String, uid: u32) -> Event {
    let Some(raw) = store.message(&folder, uid) else {
        return Event::MessageUnavailable { account: LOCAL, folder, uid, reason: tr("This message is no longer on this PC.").to_owned() };
    };
    match mailalm_mime::parse(&raw) {
        Some(parsed) => {
            let html = mailalm_mime::html_body(&raw).map(|html| Box::new(mailalm_mime::HtmlBody { html, images: mailalm_mime::related_images(&raw) }));
            Event::Message { account: LOCAL, folder, uid, parsed: Box::new(parsed), html }
        }
        None => Event::MessageUnavailable { account: LOCAL, folder, uid, reason: tr("This message could not be read.").to_owned() },
    }
}

/// An attachment the user asked to review, decoded from the message on this PC and looked at.
fn review(store: &Store, folder: String, uid: u32, index: usize) -> Event {
    let result = match store.message(&folder, uid) {
        None => Err(tr("This message is no longer on this PC.").to_owned()),
        Some(raw) => mailalm_mime::attachment(&raw, index).map(|data| {
            let report = mailalm_safety::assess(&mailalm_safety::Input { raw_name: &data.raw_name, name: &data.info.name, declared_type: &data.info.declared_type, bytes: &data.bytes });
            Box::new(Reviewed { data, report })
        }),
    };
    Event::Attachment { account: LOCAL, folder, uid, index, result }
}

/// A change made by the user, done in the cache at once; the folders it touched.
fn apply(store: &Store, change: &Change) -> Result<Vec<String>, String> {
    let saved = |path: &str, list: &[Summary]| store.save_summaries(path, list).map_err(|e| trf!("The change could not be kept: {error}", error = e));
    match change {
        Change::SetSeen { folder, uids, seen } => {
            let mut list = store.summaries(folder);
            list.iter_mut().filter(|s| uids.contains(&s.uid)).for_each(|s| s.flags.seen = *seen);
            saved(folder, &list)?;
            Ok(vec![folder.clone()])
        }
        Change::SetFlagged { folder, uids, flagged } => {
            let mut list = store.summaries(folder);
            list.iter_mut().filter(|s| uids.contains(&s.uid)).for_each(|s| s.flags.flagged = *flagged);
            saved(folder, &list)?;
            Ok(vec![folder.clone()])
        }
        Change::Delete { folder, uids } => {
            let mut list = store.summaries(folder);
            list.retain(|s| !uids.contains(&s.uid));
            saved(folder, &list)?;
            store.remove_messages(folder, uids).map_err(|e| trf!("The message could not be removed: {error}", error = e))?;
            Ok(vec![folder.clone()])
        }
        Change::Move { folder, uids, to } => {
            if !store.folders().iter().any(|f| f.path == *to) {
                return Err(tr("Mail kept on this PC moves between its own folders only.").to_owned());
            }
            let mut list = store.summaries(folder);
            let mut batch = Vec::new();
            for s in list.iter().filter(|s| uids.contains(&s.uid)) {
                if let Some(raw) = store.message(folder, s.uid) {
                    let text = store.text(folder, s.uid).unwrap_or_default();
                    batch.push((raw, s.clone(), text));
                }
            }
            store.import_messages(to, batch).map_err(|e| trf!("The message could not be moved: {error}", error = e))?;
            list.retain(|s| !uids.contains(&s.uid));
            saved(folder, &list)?;
            store.remove_messages(folder, uids).map_err(|e| trf!("The message could not be moved: {error}", error = e))?;
            Ok(vec![folder.clone(), to.clone()])
        }
        Change::MarkJunk { .. } => Err(tr("Mail kept on this PC has no junk folder: delete the message instead.").to_owned()),
    }
}

/// Searches the folders of this mailbox, newest first.
fn search(store: &Store, query: &str, scope: &FolderScope) -> Vec<(String, Summary)> {
    let q = Query::parse(query);
    if q.is_empty() || !q.wants_account(LOCAL_NAME, "") {
        return Vec::new();
    }
    let folders = store.folders();
    let mut hits = Vec::new();
    'folders: for f in scope.select(&folders) {
        for s in store.summaries(&f.path) {
            let text = if q.needs_text() { store.text(&f.path, s.uid) } else { None };
            if q.matches(&s, &f.path, text.as_deref()) {
                hits.push((f.path.clone(), s));
                if hits.len() >= MAX_HITS {
                    break 'folders;
                }
            }
        }
    }
    hits.sort_by(|a, b| b.1.date.cmp(&a.1.date));
    hits
}

/// Removes a folder of this mailbox and every folder under it, with their messages.
fn delete_folder(store: &Store, path: &str) -> Result<(), String> {
    let scope = FolderScope::WithSubfolders(path.to_owned());
    let all = store.folders();
    let (gone, kept): (Vec<Folder>, Vec<Folder>) = all.into_iter().partition(|f| scope.covers(f));
    store.save_folders(&kept).map_err(|e| trf!("The folder could not be deleted: {error}", error = e))?;
    for f in gone {
        store.clear_folder(&f.path).map_err(|e| trf!("The folder could not be deleted: {error}", error = e))?;
    }
    Ok(())
}

// ---- folder names ------------------------------------------------------------------------------

/// A path and every folder above it: `A/B/C` gives `A`, `A/B`, `A/B/C`.
pub fn with_parents(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for part in path.split(DELIMITER).filter(|p| !p.is_empty()) {
        if !cur.is_empty() {
            cur.push(DELIMITER);
        }
        cur.push_str(part);
        out.push(cur.clone());
    }
    out
}

/// The path of an imported message's folder, under the import's top folder (the parts were
/// cleaned by the importer: no delimiter in them).
pub fn import_path(root: &str, parts: &[String]) -> String {
    let mut path = root.to_owned();
    for p in parts {
        path.push(DELIMITER);
        path.push_str(p);
    }
    path
}

/// A top folder's name not taken yet in this mailbox: `wanted`, else `wanted (2)`, `(3)`…
pub fn unique_root(existing: &[Folder], wanted: &str) -> String {
    let taken = |name: &str| existing.iter().any(|f| f.path.eq_ignore_ascii_case(name));
    if !taken(wanted) {
        return wanted.to_owned();
    }
    (2..).map(|n| format!("{wanted} ({n})")).find(|n| !taken(n)).unwrap_or_else(|| wanted.to_owned())
}

// ---- imports and exports ------------------------------------------------------------------------

/// What is imported.
#[derive(Clone, Debug)]
pub enum Source {
    /// `.eml` files picked one by one.
    EmlFiles(Vec<PathBuf>),
    /// A folder tree of `.eml` files (eM Client's export, another program's).
    EmlTree(PathBuf),
    MboxFile(PathBuf),
    /// A Thunderbird profile's mail folder, or a folder of `.mbox` files.
    MboxTree(PathBuf),
    /// An Outlook data file (`.pst`, `.ost`).
    Outlook(PathBuf),
}

impl Source {
    /// The name of the top folder its messages go in.
    fn root_name(&self) -> String {
        let stem = |p: &Path| p.file_stem().map(|s| mailalm_porting::folder_name(&s.to_string_lossy())).unwrap_or_else(|| "Imported".to_owned());
        let dir = |p: &Path| p.file_name().map(|s| mailalm_porting::folder_name(&s.to_string_lossy())).unwrap_or_else(|| "Imported".to_owned());
        match self {
            Source::EmlFiles(_) => "Imported messages".to_owned(),
            Source::EmlTree(p) | Source::MboxTree(p) => dir(p),
            Source::MboxFile(p) | Source::Outlook(p) => stem(p),
        }
    }

    /// The title of its import's window ("Importing …").
    pub fn import_title(&self) -> String {
        let file = |p: &Path| p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match self {
            Source::EmlFiles(f) if f.len() == 1 => tr("Importing 1 .eml file").to_owned(),
            Source::EmlFiles(f) => trf!("Importing {count} .eml files", count = f.len()),
            Source::EmlTree(p) => trf!("Importing the .eml files of {name}", name = file(p)),
            Source::MboxFile(p) => trf!("Importing the mbox file {name}", name = file(p)),
            Source::MboxTree(p) => trf!("Importing the mbox files of {name}", name = file(p)),
            Source::Outlook(p) => trf!("Importing the Outlook data file {name}", name = file(p)),
        }
    }
}

/// How a job is going, shared with the interface.
#[derive(Clone, Debug, Default)]
pub struct JobStatus {
    pub progress: Progress,
    /// Messages done and their number, for an export.
    pub exported: (usize, usize),
    /// The end: what it did, or why it stopped.
    pub finished: Option<Result<String, String>>,
}

/// An import or an export under way on its own thread.
pub struct Job {
    pub title: String,
    pub status: Arc<Mutex<JobStatus>>,
    cancel: Cancel,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// A copy of how it is going.
    pub fn status(&self) -> JobStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Where an import's messages go: this mailbox's cache, in batches.
struct ToStore<'a> {
    store: &'a Store,
    root: String,
    batch: BTreeMap<String, Vec<(Vec<u8>, Flags)>>,
    count: usize,
    bytes: usize,
    folders: BTreeSet<String>,
    status: &'a Mutex<JobStatus>,
}

impl ToStore<'_> {
    fn flush(&mut self) -> Result<(), String> {
        for (path, messages) in std::mem::take(&mut self.batch) {
            mailalm_porting::add_to_store(self.store, &path, messages).map_err(|e| trf!("The messages could not be kept on this PC: {error}", error = e))?;
            self.folders.extend(with_parents(&path));
        }
        self.count = 0;
        self.bytes = 0;
        // The folder list now, so what came in is shown even if the import stops.
        let mut list = self.store.folders();
        for p in &self.folders {
            if !list.iter().any(|f| f.path == *p) {
                list.push(folder(p));
            }
        }
        sort_folders(&mut list);
        self.store.save_folders(&list).map_err(|e| trf!("The folder list could not be kept: {error}", error = e))
    }
}

impl Sink for ToStore<'_> {
    fn message(&mut self, item: Item) -> Result<(), String> {
        let path = import_path(&self.root, &item.folder);
        self.bytes = self.bytes.saturating_add(item.raw.len());
        self.batch.entry(path).or_default().push((item.raw, item.flags));
        self.count += 1;
        if self.count >= BATCH || self.bytes >= BATCH_BYTES {
            self.flush()?;
        }
        Ok(())
    }

    fn progress(&mut self, progress: &Progress) {
        if let Ok(mut s) = self.status.lock() {
            s.progress = *progress;
        }
    }
}

/// What an import's report says, in a sentence.
pub fn report_text(r: &Report, root: &str) -> String {
    let place = tr(LOCAL_NAME);
    // Each clause after the first carries its "; " (French puts a no-break space before it).
    let mut s = if r.messages == 1 {
        trf!("1 message imported into {place} › {folder}", place = place, folder = root)
    } else {
        trf!("{count} messages imported into {place} › {folder}", count = r.messages, place = place, folder = root)
    };
    let skipped = r.skipped_total();
    if skipped > 0 {
        s.push_str(&trf!("; {count} left out (too large, damaged, not mail, or deleted in Thunderbird)", count = skipped));
    }
    if r.left_out > 0 {
        s.push_str(&trf!("; {count} folders or files not read (too deep, too many, or unreadable)", count = r.left_out));
    }
    if r.cancelled {
        s.push_str(tr("; stopped: what came in before stays"));
    }
    if r.stopped_at_limit {
        s.push_str(tr("; stopped at a million messages"));
    }
    s.push('.');
    s
}

impl LocalBox {
    /// Imports `source` into a new top folder of this mailbox, on a thread of its own.
    pub fn import(&self, source: Source, wake: Wake) -> Result<Job, String> {
        let store = self.store().map_err(|e| trf!("The mail kept on this PC can't be opened: {error}", error = e))?;
        let root = unique_root(&store.folders(), &source.root_name());
        let status = Arc::new(Mutex::new(JobStatus::default()));
        let cancel = Cancel::new();
        let job = Job { title: source.import_title(), status: Arc::clone(&status), cancel: cancel.clone() };
        let back = self.sender();
        let started = std::thread::Builder::new().name("mailalm-import".into()).spawn(move || {
            let mut sink = ToStore { store: &store, root: root.clone(), batch: BTreeMap::new(), count: 0, bytes: 0, folders: BTreeSet::new(), status: &status };
            let result = match &source {
                Source::EmlFiles(paths) => mailalm_porting::eml::import_files(paths, &[], &mut sink, &cancel),
                Source::EmlTree(p) => mailalm_porting::eml::import_tree(p, &mut sink, &cancel),
                Source::MboxFile(p) => mailalm_porting::mbox::import_file(p, &[], &mut sink, &cancel),
                Source::MboxTree(p) => mailalm_porting::mbox::import_tree(p, &mut sink, &cancel),
                Source::Outlook(p) => mailalm_porting::pst::import(p, &mut sink, &cancel),
            };
            // What is still in the batch goes in too (also after a cancel or an error).
            let kept = sink.flush();
            let end = match (result, kept) {
                (Ok(report), Ok(())) => Ok(report_text(&report, &root)),
                (Err(e), _) | (Ok(_), Err(e)) => Err(e),
            };
            if let Ok(mut s) = status.lock() {
                s.finished = Some(end);
            }
            let _ = back.send(Command::SyncAll);
            wake();
        });
        started.map_err(|e| trf!("The import could not start: {error}", error = e))?;
        Ok(job)
    }
}

/// Writes folder `folder` of a cache (an account's, or this mailbox's) to the mbox file `to`, on a
/// thread of its own.
pub fn export_mbox(store: Store, folder: String, to: PathBuf, wake: Wake) -> Result<Job, String> {
    let status = Arc::new(Mutex::new(JobStatus::default()));
    let cancel = Cancel::new();
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let job = Job { title: trf!("Exporting to {file}", file = name), status: Arc::clone(&status), cancel: cancel.clone() };
    let started = std::thread::Builder::new().name("mailalm-export".into()).spawn(move || {
        let mut progress = |done: usize, total: usize| {
            if let Ok(mut s) = status.lock() {
                s.exported = (done, total);
            }
        };
        let end = match mailalm_porting::mbox::export(&store, &folder, &to, &cancel, &mut progress) {
            Ok(e) if e.cancelled => Ok(tr("Export cancelled: no file was written.").to_owned()),
            Ok(e) if e.missing > 0 => Ok(trf!("{count} messages written to {path}; {missing} not in MailALM's cache (never opened) were left out: open them, or sync the folder, then export again.", count = e.written, path = to.display(), missing = e.missing)),
            Ok(e) => Ok(trf!("{count} messages written to {path}.", count = e.written, path = to.display())),
            Err(e) => Err(trf!("The folder could not be exported: {error}", error = e)),
        };
        if let Ok(mut s) = status.lock() {
            s.finished = Some(end);
        }
        wake();
    });
    started.map_err(|e| trf!("The export could not start: {error}", error = e))?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_folders_and_their_parents() {
        assert_eq!(with_parents("Archive/2026/Work"), vec!["Archive", "Archive/2026", "Archive/2026/Work"]);
        assert_eq!(with_parents("Top"), vec!["Top"]);
        assert!(with_parents("").is_empty());
        assert_eq!(import_path("backup", &["Inbox".to_owned(), "Work".to_owned()]), "backup/Inbox/Work");
        assert_eq!(import_path("backup", &[]), "backup");
    }

    #[test]
    fn each_import_gets_a_top_folder_of_its_own() {
        let existing = vec![folder("backup"), folder("Backup (2)"), folder("backup/Inbox")];
        assert_eq!(unique_root(&existing, "Outlook"), "Outlook");
        assert_eq!(unique_root(&existing, "Backup"), "Backup (3)");
        assert_eq!(Source::Outlook(PathBuf::from("C:/x/archive 2019.pst")).root_name(), "archive 2019");
        assert_eq!(Source::MboxTree(PathBuf::from("C:/profile/Local Folders")).root_name(), "Local Folders");
        assert_eq!(Source::EmlFiles(vec![]).root_name(), "Imported messages");
    }

    #[test]
    fn the_report_says_what_was_left_out() {
        let mut r = Report { messages: 3, left_out: 1, cancelled: true, ..Report::default() };
        r.skipped.insert(mailalm_porting::Skip::TooLarge, 2);
        let text = report_text(&r, "backup");
        assert!(text.starts_with("3 messages imported into On this PC › backup; 2 left out"));
        assert!(text.contains("1 folders or files not read") && text.contains("stopped"));
        assert_eq!(report_text(&Report { messages: 1, ..Report::default() }, "x"), "1 message imported into On this PC › x.");
    }

    /// The mailbox's thread does what an account's worker does, in the cache only.
    #[test]
    fn changes_are_made_in_the_cache() {
        let base = std::env::temp_dir().join(format!("mailalm-local-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = Store::open(&base, LOCAL).expect("store");
        store.save_folders(&[folder("A"), folder("A/B"), folder("C")]).expect("folders");
        let raw = b"From: a@b.example\r\nSubject: Hello\r\nMessage-ID: <x@y>\r\n\r\nBody text\r\n".to_vec();
        let uids = mailalm_porting::add_to_store(&store, "A/B", vec![(raw.clone(), Flags::default()), (raw, Flags::default())]).expect("added");
        assert_eq!(uids, vec![1, 2]);
        assert_eq!(apply(&store, &Change::SetSeen { folder: "A/B".into(), uids: vec![1], seen: true }), Ok(vec!["A/B".to_owned()]));
        assert_eq!(store.summaries("A/B").iter().filter(|s| s.flags.seen).count(), 1);
        assert!(apply(&store, &Change::Move { folder: "A/B".into(), uids: vec![2], to: "Elsewhere".into() }).is_err());
        assert!(apply(&store, &Change::Move { folder: "A/B".into(), uids: vec![2], to: "C".into() }).is_ok());
        assert_eq!(store.summaries("C").len(), 1);
        assert!(store.message("A/B", 2).is_none());
        assert_eq!(search(&store, "hello", &FolderScope::Account).len(), 2);
        assert_eq!(search(&store, "hello", &FolderScope::Folder("C".into())).len(), 1);
        assert!(matches!(open(&store, "C".into(), 1), Event::Message { .. }));
        assert!(matches!(open(&store, "C".into(), 9), Event::MessageUnavailable { .. }));
        delete_folder(&store, "A").expect("deleted");
        assert_eq!(store.folders().iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["C"]);
        assert!(store.summaries("A/B").is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }
}
