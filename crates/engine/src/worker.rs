//! One account's worker: its cache, its IMAP connection, its queue of changes.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use mailalm_mime::Outgoing;
use mailalm_model::{Account, AccountId, Change, Flags, Folder, FolderRole, Summary, sort_folders};
use mailalm_net::{ImapSession, Mailbox, NetError};
use mailalm_store::{FolderState, Store};

use crate::local::{self, sort_newest_first};
use crate::{Command, Event, Link, Wake, secrets};

/// How often the Inbox and the open folder are checked for new mail.
const CHECK_EVERY: Duration = Duration::from_secs(3 * 60);
/// How often every folder is synced.
const SYNC_ALL_EVERY: Duration = Duration::from_secs(30 * 60);
/// Headers fetched per round trip, newest first.
const HEADER_BATCH: usize = 300;
/// The newest messages whose flags are refreshed at each sync (older ones at a full sync).
const FLAG_WINDOW: usize = 2_000;
/// Messages fetched whole ahead of time (for their preview and the search): in the Inbox, in other
/// folders, and the largest such message.
const PREFETCH_INBOX: usize = 100;
const PREFETCH_OTHER: usize = 20;
const PREFETCH_MAX_SIZE: u32 = 2 * 1024 * 1024;

pub fn run(account: Account, base: PathBuf, commands: Receiver<Command>, events: Sender<Event>, wake: Wake) {
    let store = match Store::open(&base, account.id) {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(Event::Error { account: account.id, text: format!("The cache of {} could not be opened: {e}", account.name) });
            wake();
            return;
        }
    };
    let mut w = Worker {
        id: account.id,
        account,
        store,
        events,
        wake,
        session: None,
        selected: None,
        folders: Vec::new(),
        viewed: None,
        refused: false,
        stop: false,
        flushing: false,
        queued: Vec::new(),
    };
    w.show_cache();
    w.sync_all(&commands);
    let mut last_check = Instant::now();
    let mut last_full = Instant::now();
    while !w.stop {
        let next = w.queued.pop();
        let cmd = match next {
            Some(c) => Ok(c),
            None => commands.recv_timeout(Duration::from_secs(20)),
        };
        match cmd {
            Ok(c) => w.handle(c, &commands),
            Err(RecvTimeoutError::Timeout) => {
                if last_full.elapsed() >= SYNC_ALL_EVERY {
                    last_full = Instant::now();
                    last_check = Instant::now();
                    w.sync_all(&commands);
                } else if last_check.elapsed() >= CHECK_EVERY {
                    last_check = Instant::now();
                    w.check_new(&commands);
                } else if let Some(s) = w.session.as_mut() {
                    // Keeps the connection alive; a dead one is dropped and reopened next time.
                    if s.noop().is_err() {
                        w.drop_session();
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(s) = w.session.take() {
        s.logout();
    }
}

struct Worker {
    id: AccountId,
    account: Account,
    store: Store,
    events: Sender<Event>,
    wake: Wake,
    session: Option<ImapSession>,
    /// The folder selected on the connection.
    selected: Option<String>,
    folders: Vec<Folder>,
    /// The folder the user is looking at.
    viewed: Option<String>,
    /// The server refused the password: no new attempt until `Reconnect`.
    refused: bool,
    stop: bool,
    /// The waiting changes are being sent (guards against sending them twice).
    flushing: bool,
    /// Commands taken from the channel during a long sync, to run after it (the oldest last,
    /// taken first).
    queued: Vec<Command>,
}

/// An error as the user reads it. Never contains a password: the network layer never puts one in.
pub fn describe(e: &NetError) -> String {
    if e.is_auth() { format!("The server refused the sign-in: {e}") } else { e.to_string() }
}

impl Worker {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
        (self.wake)();
    }

    fn activity(&self, text: Option<String>) {
        self.emit(Event::Activity { account: self.id, text });
    }

    fn link(&self, link: Link, detail: impl Into<String>) {
        self.emit(Event::Link { account: self.id, link, detail: detail.into() });
    }

    fn error(&self, text: impl Into<String>) {
        self.emit(Event::Error { account: self.id, text: text.into() });
    }

    /// Shows what the cache has, before any network.
    fn show_cache(&mut self) {
        self.folders = self.store.folders();
        if self.folders.is_empty() {
            return;
        }
        self.emit(Event::Folders { account: self.id, folders: self.folders.clone() });
        for f in self.folders.clone() {
            let summaries = self.store.summaries(&f.path);
            if !summaries.is_empty() {
                self.emit(Event::Summaries { account: self.id, folder: f.path.clone(), summaries });
            }
        }
    }

    fn handle(&mut self, c: Command, commands: &Receiver<Command>) {
        match c {
            Command::SyncAll => self.sync_all(commands),
            Command::View(folder) => {
                self.viewed = Some(folder.clone());
                if let Some(f) = self.folder(&folder) {
                    self.sync_folder(&f, commands);
                    self.activity(None);
                }
            }
            Command::Open { folder, uid } => self.open(&folder, uid),
            Command::Apply(change) => self.apply(change, commands),
            Command::Send(message) => self.send_message(*message, commands),
            Command::Search { id, query } => self.search(id, &query),
            Command::Reconnect => {
                self.refused = false;
                self.drop_session();
                self.sync_all(commands);
            }
            Command::Senders(_) => self.error("Blocking a sender on an IMAP account arrives with the rules (phase 8)."),
            Command::Stop => self.stop = true,
        }
    }

    /// During a long sync: answers what the user is waiting for right away. Opening a message and
    /// searching are done now; a change is made in the cache now and sent to the server after the
    /// sync; the rest waits for the end of the sync. The sync selects its folder again by itself
    /// (`in_folder`).
    /// Returns whether anything was done (the sync then catches up with it).
    fn serve_waiting(&mut self, commands: &Receiver<Command>) -> bool {
        let mut served = false;
        while let Ok(c) = commands.try_recv() {
            served = true;
            match c {
                Command::Open { folder, uid } => self.open(&folder, uid),
                Command::Search { id, query } => self.search(id, &query),
                Command::Apply(change) => self.apply_local(change),
                Command::Stop => {
                    self.stop = true;
                    return served;
                }
                other => self.queued.insert(0, other),
            }
        }
        served
    }

    fn folder(&self, path: &str) -> Option<Folder> {
        self.folders.iter().find(|f| f.path == path).cloned()
    }

    fn folder_with_role(&self, role: FolderRole) -> Option<Folder> {
        self.folders.iter().find(|f| f.role == role).cloned()
    }

    fn drop_session(&mut self) {
        self.session = None;
        self.selected = None;
    }

    /// The connection, opened if needed. `None` when offline or refused (already reported).
    fn connect(&mut self) -> Option<&mut ImapSession> {
        if self.session.is_none() {
            if self.refused {
                return None;
            }
            let Some(server) = self.account.imap.clone() else {
                self.link(Link::Offline, "The account has no IMAP server.");
                return None;
            };
            let password = match secrets::password(self.id) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    self.refused = true;
                    self.link(Link::SignInRefused, "No password is kept for this account. Enter it in the account's settings.");
                    return None;
                }
                Err(e) => {
                    self.link(Link::Offline, e);
                    return None;
                }
            };
            self.link(Link::Connecting, format!("Connecting to {}…", server.host));
            match ImapSession::connect(&server, self.account.login(), &password) {
                Ok(s) => {
                    self.session = Some(s);
                    self.selected = None;
                    self.link(Link::Online, server.host.clone());
                    self.flush_pending();
                }
                Err(e) if e.is_auth() => {
                    self.refused = true;
                    self.link(Link::SignInRefused, describe(&e));
                    return None;
                }
                Err(e) => {
                    self.link(Link::Offline, describe(&e));
                    return None;
                }
            }
        }
        self.session.as_mut()
    }

    /// Selects a folder (again, for its current counts), connecting if needed.
    fn select(&mut self, path: &str) -> Result<Mailbox, String> {
        let wire = self.folder(path).map(|f| f.wire).unwrap_or_else(|| path.to_owned());
        self.selected = None;
        let Some(s) = self.connect() else { return Err("offline".to_owned()) };
        let r = s.select(&wire);
        match r {
            Ok(m) => {
                self.selected = Some(path.to_owned());
                Ok(m)
            }
            Err(e) => Err(self.failed(&e)),
        }
    }

    /// Runs one IMAP step with the folder `path` selected. A network error drops the connection
    /// (the next step reconnects) and is returned.
    fn in_folder<T>(&mut self, path: &str, step: impl FnOnce(&mut ImapSession) -> Result<T, NetError>) -> Result<T, String> {
        if self.session.is_none() || self.selected.as_deref() != Some(path) {
            self.select(path)?;
        }
        let Some(s) = self.session.as_mut() else { return Err("offline".to_owned()) };
        let r = step(s);
        r.map_err(|e| self.failed(&e))
    }

    /// An IMAP error as text. The server answering NO or BAD leaves the connection usable; any
    /// other error drops it.
    fn failed(&mut self, e: &NetError) -> String {
        if !matches!(e, NetError::Server(_)) {
            self.drop_session();
        }
        describe(e)
    }

    // ---- Sync ----

    fn sync_all(&mut self, commands: &Receiver<Command>) {
        if self.connect().is_none() {
            return;
        }
        self.activity(Some("Reading the folder list…".to_owned()));
        let listed = match self.session.as_mut().map(ImapSession::folders) {
            Some(Ok(f)) => f,
            Some(Err(e)) => {
                self.error(format!("The folder list could not be read: {}", describe(&e)));
                self.drop_session();
                self.activity(None);
                return;
            }
            None => return,
        };
        let mut folders = listed;
        // Keeps the counts known so far until each folder is synced again.
        for f in &mut folders {
            if let Some(old) = self.folders.iter().find(|o| o.path == f.path) {
                f.total = old.total;
                f.unread = old.unread;
            }
        }
        sort_folders(&mut folders);
        self.folders = folders;
        let _ = self.store.save_folders(&self.folders);
        let _ = self.store.forget_folders_except(&self.folders);
        self.emit(Event::Folders { account: self.id, folders: self.folders.clone() });

        // The Inbox, then the folder the user is looking at, then the others.
        let mut order: Vec<Folder> = self.folders.clone();
        let viewed = self.viewed.clone();
        order.sort_by_key(|f| (f.role != FolderRole::Inbox, Some(&f.path) != viewed.as_ref()));
        for f in order {
            if self.stop || self.session.is_none() && self.connect().is_none() {
                break;
            }
            self.sync_folder(&f, commands);
        }
        self.activity(None);
    }

    /// The Inbox and the open folder, between two full syncs.
    fn check_new(&mut self, commands: &Receiver<Command>) {
        let mut paths: Vec<String> = self.folder_with_role(FolderRole::Inbox).map(|f| f.path).into_iter().collect();
        if let Some(v) = self.viewed.clone()
            && !paths.contains(&v)
        {
            paths.push(v);
        }
        if paths.is_empty() {
            self.sync_all(commands);
            return;
        }
        for p in paths {
            if let Some(f) = self.folder(&p) {
                self.sync_folder(&f, commands);
            }
        }
        self.activity(None);
    }

    fn sync_folder(&mut self, folder: &Folder, commands: &Receiver<Command>) {
        let path = folder.path.clone();
        self.activity(Some(format!("Syncing {}…", folder.name())));
        let mailbox = match self.select(&path) {
            Ok(m) => m,
            Err(e) => {
                if self.session.is_some() {
                    self.error(format!("{} could not be opened: {e}", folder.name()));
                }
                return;
            }
        };

        let mut summaries = self.store.summaries(&path);
        let state = self.store.folder_state(&path);
        if state.as_ref().is_some_and(|s| s.uidvalidity != mailbox.uidvalidity) {
            // Every UID of the folder changed meaning: start it over.
            let _ = self.store.clear_folder(&path);
            summaries.clear();
        }

        let server: Vec<u32> = match self.in_folder(&path, ImapSession::uids) {
            Ok(u) => u,
            Err(e) => {
                self.error(format!("{} could not be read: {e}", folder.name()));
                return;
            }
        };
        let on_server: HashSet<u32> = server.iter().copied().collect();

        // Gone from the server: gone from the cache.
        let gone: Vec<u32> = summaries.iter().map(|s| s.uid).filter(|u| !on_server.contains(u)).collect();
        if !gone.is_empty() {
            summaries.retain(|s| on_server.contains(&s.uid));
            let _ = self.store.remove_messages(&path, &gone);
        }

        // Flags of the newest known messages (all of them when few).
        let known: HashSet<u32> = summaries.iter().map(|s| s.uid).collect();
        let mut window: Vec<u32> = known.iter().copied().collect();
        window.sort_unstable_by(|a, b| b.cmp(a));
        window.truncate(FLAG_WINDOW);
        if !window.is_empty() {
            match self.in_folder(&path, |s| s.fetch_flags(&window)) {
                Ok(flags) => {
                    let map: HashMap<u32, Flags> = flags.into_iter().map(|(u, f)| (u, Flags::from_imap(f.iter().map(String::as_str)))).collect();
                    for s in &mut summaries {
                        if let Some(f) = map.get(&s.uid) {
                            s.flags = *f;
                        }
                    }
                }
                Err(e) => {
                    self.error(format!("{} could not be read: {e}", folder.name()));
                    return;
                }
            }
        }
        // Changes still waiting for the server win over what it says.
        local::overlay_pending(&self.store, &path, &mut summaries);

        // New messages, newest first, by batches shown as they come.
        let mut new: Vec<u32> = server.iter().copied().filter(|u| !known.contains(u)).collect();
        new.sort_unstable_by(|a, b| b.cmp(a));
        let total_new = new.len();
        let mut done = 0;
        for batch in new.chunks(HEADER_BATCH) {
            if total_new > HEADER_BATCH {
                self.activity(Some(format!("Syncing {}… {done} of {total_new}", folder.name())));
            }
            match self.in_folder(&path, |s| s.fetch_headers(batch)) {
                Ok(fetched) => {
                    for f in fetched {
                        let flags = Flags::from_imap(f.flags.iter().map(String::as_str));
                        summaries.push(mailalm_mime::summary_from_header(f.uid, &f.header, flags, f.size, f.internal_date));
                    }
                }
                Err(e) => {
                    self.error(format!("{} could not be read: {e}", folder.name()));
                    break;
                }
            }
            done += batch.len();
            // Shown as they come at first, then every few thousand (a large folder is not written
            // out again for every batch).
            if done <= 3 * HEADER_BATCH || done % (10 * HEADER_BATCH) == 0 {
                sort_newest_first(&mut summaries);
                let _ = self.store.save_summaries(&path, &summaries);
                self.emit(Event::Summaries { account: self.id, folder: path.clone(), summaries: summaries.clone() });
            }
            if self.serve_waiting(commands) {
                self.catch_up(&path, &mut summaries);
            }
            if self.stop || self.session.is_none() {
                sort_newest_first(&mut summaries);
                let _ = self.store.save_summaries(&path, &summaries);
                return;
            }
        }

        self.catch_up(&path, &mut summaries);
        sort_newest_first(&mut summaries);
        let _ = self.store.save_summaries(&path, &summaries);
        let _ = self.store.save_folder_state(&FolderState { path: path.clone(), uidvalidity: mailbox.uidvalidity, uidnext: mailbox.uidnext, delta: None });
        self.update_counts(&path, &summaries);
        self.emit(Event::Summaries { account: self.id, folder: path.clone(), summaries: summaries.clone() });

        self.prefetch(folder, &mut summaries, commands);
        self.flush_pending();
    }

    /// What happened to the folder while the sync was serving the user: changes made in the cache
    /// (still waiting for the server), and previews of messages opened.
    fn catch_up(&self, path: &str, summaries: &mut Vec<Summary>) {
        local::overlay_pending(&self.store, path, summaries);
        let cached: HashMap<u32, Summary> = self.store.summaries(path).into_iter().filter(|s| !s.preview.is_empty()).map(|s| (s.uid, s)).collect();
        for s in summaries.iter_mut().filter(|s| s.preview.is_empty()) {
            if let Some(c) = cached.get(&s.uid) {
                s.preview = c.preview.clone();
                s.has_attachments = c.has_attachments;
            }
        }
    }

    /// Fetches the newest messages whole, for their preview and the search.
    fn prefetch(&mut self, folder: &Folder, summaries: &mut Vec<Summary>, commands: &Receiver<Command>) {
        let path = folder.path.clone();
        let limit = if folder.role == FolderRole::Inbox { PREFETCH_INBOX } else { PREFETCH_OTHER };
        let wanted: Vec<u32> = summaries
            .iter()
            .take(limit)
            .filter(|s| s.size <= PREFETCH_MAX_SIZE && !self.store.has_message(&path, s.uid))
            .map(|s| s.uid)
            .collect();
        if wanted.is_empty() {
            return;
        }
        let mut changed = false;
        for uid in wanted {
            match self.in_folder(&path, |s| s.fetch_message(uid)) {
                Ok(Some(raw)) => {
                    if let Some(parsed) = mailalm_mime::parse(&raw) {
                        let _ = self.store.save_message(&path, uid, &raw, &parsed.text);
                        if let Some(s) = summaries.iter_mut().find(|s| s.uid == uid) {
                            mailalm_mime::complete_summary(s, &parsed);
                            changed = true;
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => break,
            }
            let _ = self.serve_waiting(commands);
            if self.stop || self.session.is_none() {
                break;
            }
        }
        if changed {
            self.catch_up(&path, summaries);
            let _ = self.store.save_summaries(&path, summaries);
            self.emit(Event::Summaries { account: self.id, folder: path, summaries: summaries.clone() });
        }
    }

    fn update_counts(&mut self, path: &str, summaries: &[Summary]) {
        let (total, unread) = local::counts(summaries);
        if let Some(f) = self.folders.iter_mut().find(|f| f.path == path)
            && (f.total != total || f.unread != unread)
        {
            f.total = total;
            f.unread = unread;
            let _ = self.store.save_folders(&self.folders);
            self.emit(Event::Folders { account: self.id, folders: self.folders.clone() });
        }
    }

    // ---- Reading ----

    fn open(&mut self, path: &str, uid: u32) {
        let raw = match self.store.message(path, uid) {
            Some(raw) => raw,
            None => match self.in_folder(path, |s| s.fetch_message(uid)) {
                Ok(Some(raw)) => raw,
                Ok(None) => {
                    self.emit(Event::MessageUnavailable { account: self.id, folder: path.to_owned(), uid, reason: "This message is no longer on the server.".to_owned() });
                    return;
                }
                Err(e) => {
                    let reason = if self.session.is_none() { format!("This message is not in the cache, and the server cannot be reached: {e}") } else { e };
                    self.emit(Event::MessageUnavailable { account: self.id, folder: path.to_owned(), uid, reason });
                    return;
                }
            },
        };
        let Some(parsed) = mailalm_mime::parse(&raw) else {
            self.emit(Event::MessageUnavailable { account: self.id, folder: path.to_owned(), uid, reason: "This message could not be read.".to_owned() });
            return;
        };
        if !self.store.has_message(path, uid) {
            let _ = self.store.save_message(path, uid, &raw, &parsed.text);
            let mut summaries = self.store.summaries(path);
            if let Some(s) = summaries.iter_mut().find(|s| s.uid == uid) {
                mailalm_mime::complete_summary(s, &parsed);
                let _ = self.store.save_summaries(path, &summaries);
                self.emit(Event::Summaries { account: self.id, folder: path.to_owned(), summaries });
            }
        }
        self.emit(Event::Message { account: self.id, folder: path.to_owned(), uid, parsed: Box::new(parsed) });
    }

    // ---- Changes ----

    /// Makes the change in the cache, queues it for the server, sends it if online, and syncs the
    /// folder messages were moved to.
    fn apply(&mut self, change: Change, commands: &Receiver<Command>) {
        let moved = self.moved_to(&change);
        self.apply_local(change);
        if self.connect().is_some() {
            self.flush_pending();
            if let Some(f) = moved.and_then(|to| self.folder(&to)) {
                self.sync_folder(&f, commands);
            }
            self.activity(None);
        }
    }

    /// Makes the change in the cache now (the interface shows it at once), and queues it for the
    /// server.
    fn apply_local(&mut self, change: Change) {
        let path = change.folder().to_owned();
        let mut summaries = self.store.summaries(&path);
        local::apply_to(&mut summaries, &change);
        let gone = local::removed_by(&change);
        if !gone.is_empty() {
            let _ = self.store.remove_messages(&path, gone);
        }
        let _ = self.store.save_summaries(&path, &summaries);
        self.update_counts(&path, &summaries);
        self.emit(Event::Summaries { account: self.id, folder: path.clone(), summaries });

        let mut pending = self.store.pending();
        pending.push(change);
        if let Err(e) = self.store.save_pending(&pending) {
            self.error(format!("The change could not be kept for the server: {e}"));
        }
    }

    /// Sends the waiting changes, in order. A change the server refuses is dropped (and said);
    /// a network failure keeps it and the rest for next time.
    fn flush_pending(&mut self) {
        if self.flushing || self.store.pending().is_empty() {
            return;
        }
        self.flushing = true;
        self.flush_pending_now();
        self.flushing = false;
    }

    fn flush_pending_now(&mut self) {
        let mut pending = self.store.pending();
        while let Some(change) = pending.first().cloned() {
            let path = change.folder().to_owned();
            let dest_wire = match &change {
                Change::Move { to, .. } => Some(self.folder(to).map(|f| f.wire).unwrap_or_else(|| to.clone())),
                // IMAP has no junk command: the messages are moved to Junk, or back to the Inbox.
                Change::MarkJunk { .. } => self.moved_to(&change).and_then(|to| self.folder(&to)).map(|f| f.wire),
                _ => None,
            };
            let result = self.in_folder(&path, |s| match &change {
                Change::SetSeen { uids, seen, .. } => s.set_flag(uids, "\\Seen", *seen),
                Change::SetFlagged { uids, flagged, .. } => s.set_flag(uids, "\\Flagged", *flagged),
                Change::Move { uids, .. } => s.move_to(uids, dest_wire.as_deref().unwrap_or_default()),
                Change::Delete { uids, .. } => s.delete(uids),
                Change::MarkJunk { uids, .. } => match dest_wire.as_deref() {
                    Some(to) => s.move_to(uids, to),
                    None => Err(NetError::Invalid("this account has no folder to move the messages to".to_owned())),
                },
            });
            match result {
                Ok(()) => {
                    pending.remove(0);
                }
                Err(e) if self.session.is_some() => {
                    // The server said no: telling the user is all that can be done.
                    self.error(format!("The server refused a change in {path}: {e}"));
                    pending.remove(0);
                }
                Err(_) => break,
            }
            let _ = self.store.save_pending(&pending);
        }
    }

    // ---- Sending ----

    fn send_message(&mut self, message: Outgoing, commands: &Receiver<Command>) {
        let fail = |w: &Self, text: String, message: Outgoing| {
            w.emit(Event::SendFailed { account: w.id, text, message: Box::new(message) });
        };
        let Some(server) = self.account.smtp.clone() else {
            fail(self, "The account has no outgoing (SMTP) server.".to_owned(), message);
            return;
        };
        let (raw, _id) = match mailalm_mime::build(&message) {
            Ok(r) => r,
            Err(e) => {
                fail(self, e, message);
                return;
            }
        };
        let password = match secrets::password(self.id) {
            Ok(Some(p)) => p,
            Ok(None) => {
                fail(self, "No password is kept for this account.".to_owned(), message);
                return;
            }
            Err(e) => {
                fail(self, e, message);
                return;
            }
        };
        self.activity(Some(format!("Sending \"{}\"…", message.subject)));
        let recipients = message.recipients();
        if let Err(e) = mailalm_net::smtp::send(&server, self.account.login(), &password, &message.from.address, &recipients, &raw) {
            self.activity(None);
            fail(self, format!("The message was not sent: {}", describe(&e)), message);
            return;
        }
        self.emit(Event::Sent { account: self.id, subject: message.subject.clone() });

        // A copy in the Sent folder (Vidéotron and Bell do not keep one themselves).
        if let Some(sent) = self.folder_with_role(FolderRole::Sent) {
            let wire = sent.wire.clone();
            let kept = match self.connect() {
                Some(s) => s.append(&wire, &["\\Seen"], &raw).map_err(|e| describe(&e)),
                None => Err("offline".to_owned()),
            };
            match kept {
                Ok(()) => self.sync_folder(&sent, commands),
                Err(e) => self.error(format!("The message was sent, but no copy could be kept in {}: {e}", sent.name())),
            }
        }
        self.activity(None);
    }

    // ---- Search ----

    fn search(&mut self, id: u64, query: &str) {
        let hits = local::search(&self.store, &self.folders, query);
        self.emit(Event::SearchResults { account: self.id, id, hits });
    }

    /// The folder a change puts messages in, to sync it after.
    fn moved_to(&self, change: &Change) -> Option<String> {
        match change {
            Change::Move { to, .. } => Some(to.clone()),
            Change::MarkJunk { junk, .. } => self.folder_with_role(if *junk { FolderRole::Junk } else { FolderRole::Inbox }).map(|f| f.path),
            _ => None,
        }
    }
}
