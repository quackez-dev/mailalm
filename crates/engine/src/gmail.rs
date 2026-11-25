//! One Gmail account's worker: its cache, its Gmail API client, its queue of changes, its
//! blocked and allowed senders (PLAN.md, section 5.2).
//!
//! Gmail has labels, not folders: MailALM shows the usual system labels (Inbox, Starred, Sent,
//! Drafts, Spam, Trash), the Inbox categories (Primary, Social, Promotions, Updates, Forums), All
//! Mail (every message but Spam and Trash) and the user's labels, each as a folder. A message with
//! several labels is in several folders, with a number in each (`ids.json`, as for Outlook).
//!
//! A folder is synced by listing its message ids (newest first, at most [`MAX_LISTED`]), with
//! its unread and starred ones: what is no longer listed is gone, marks follow the lists, and only
//! messages new to the folder are read in detail. The mailbox's history id gates it: a folder
//! synced at the history id Gmail still reports has nothing new, and is not listed again.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use mailalm_mime::Outgoing;
use mailalm_model::{Account, AccountId, Change, Folder, FolderRole, Summary};
use mailalm_net::NetError;
use mailalm_net::gmail::{FilterAction, GmailClient, LabelKind};
use mailalm_net::oauth::{self, Provider, Tokens};
use mailalm_store::{FolderState, Store};

use crate::ids::Ids;
use crate::local::{self, sort_newest_first};
use crate::{Command, Event, Link, SenderCommand, SenderRule, SenderRuleKind, Wake, secrets};

/// How often the Inbox and the open folder are checked for new mail.
const CHECK_EVERY: Duration = Duration::from_secs(3 * 60);
/// How often every folder is synced.
const SYNC_ALL_EVERY: Duration = Duration::from_secs(30 * 60);
/// Messages fetched whole ahead of time (for the search): in the Inbox, in other folders; only
/// those without attachments and at most [`PREFETCH_MAX_SIZE`] bytes.
const PREFETCH_INBOX: usize = 100;
const PREFETCH_OTHER: usize = 20;
const PREFETCH_MAX_SIZE: u32 = 512 << 10;
/// The newest messages of a folder MailALM keeps.
const MAX_LISTED: usize = 5_000;
/// Messages read in detail in one sync of a folder; the rest come at the next sync.
const MAX_FETCH: usize = 1_000;
/// Messages read in detail between two saves of the folder.
const SAVE_EVERY: usize = 100;
/// Message details kept in memory, so a message with several labels is read once.
const MAX_DETAILS: usize = 20_000;
/// All Mail is no label: its wire name is empty, and listing it names no label.
const ALL_MAIL: &str = "All Mail";
/// The system labels shown as folders: id, name, role.
const SYSTEM: [(&str, &str, FolderRole); 11] = [
    ("INBOX", "Inbox", FolderRole::Inbox),
    ("STARRED", "Starred", FolderRole::Other),
    ("SENT", "Sent", FolderRole::Sent),
    ("DRAFT", "Drafts", FolderRole::Drafts),
    ("SPAM", "Spam", FolderRole::Junk),
    ("TRASH", "Trash", FolderRole::Trash),
    ("CATEGORY_PERSONAL", "Primary", FolderRole::Other),
    ("CATEGORY_SOCIAL", "Social", FolderRole::Other),
    ("CATEGORY_PROMOTIONS", "Promotions", FolderRole::Other),
    ("CATEGORY_UPDATES", "Updates", FolderRole::Other),
    ("CATEGORY_FORUMS", "Forums", FolderRole::Other),
];

pub fn run(account: Account, base: PathBuf, commands: Receiver<Command>, events: Sender<Event>, wake: Wake) {
    let store = match Store::open(&base, account.id) {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(Event::Error { account: account.id, text: format!("The cache of {} could not be opened: {e}", account.name) });
            wake();
            return;
        }
    };
    let mut w = Gmail {
        id: account.id,
        store,
        events,
        wake,
        client: None,
        tokens: None,
        folders: Vec::new(),
        viewed: None,
        refused: false,
        stop: false,
        flushing: false,
        queued: Vec::new(),
        details: HashMap::new(),
    };
    w.show_cache();
    w.sync_all(&commands);
    let mut last_check = Instant::now();
    let mut last_full = Instant::now();
    while !w.stop {
        let cmd = match w.queued.pop() {
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
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

struct Gmail {
    id: AccountId,
    store: Store,
    events: Sender<Event>,
    wake: Wake,
    client: Option<GmailClient>,
    tokens: Option<Tokens>,
    folders: Vec<Folder>,
    viewed: Option<String>,
    /// Google refused the sign-in: no new attempt until `Reconnect`.
    refused: bool,
    stop: bool,
    flushing: bool,
    queued: Vec<Command>,
    /// The details of the messages read lately, by Gmail id: a message in several folders (the
    /// Inbox, its category, All Mail) is read once.
    details: HashMap<String, Summary>,
}

/// An error as the user reads it. Never holds a token: the network layer never puts one in.
fn describe(e: &NetError) -> String {
    if e.is_auth() { format!("Google refused the sign-in: {e}") } else { e.to_string() }
}

/// A label MailALM may take off a message that leaves its folder: the Inbox, a category, a
/// user's label. Sent, Drafts and Starred stay (a message is still sent, a draft, starred).
fn removable(wire: &str) -> bool {
    wire == "INBOX" || wire.starts_with("CATEGORY_") || (!wire.is_empty() && !SYSTEM.iter().any(|(id, _, _)| *id == wire))
}

/// A path not yet taken: `path`, else `path (2)`, `path (3)`…
fn unique(path: String, taken: &HashSet<String>) -> String {
    if !taken.contains(&path) {
        return path;
    }
    (2..10_000).map(|n| format!("{path} ({n})")).find(|p| !taken.contains(p)).unwrap_or(path)
}

impl Gmail {
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
                    let history = self.history();
                    self.sync_folder(&f, history, commands);
                    self.activity(None);
                }
            }
            Command::Open { folder, uid } => self.open(&folder, uid),
            Command::Apply(change) => self.apply(change, commands),
            Command::Send(message) => self.send_message(*message, commands),
            Command::Search { id, query } => {
                let hits = local::search(&self.store, &self.folders, &query);
                self.emit(Event::SearchResults { account: self.id, id, hits });
            }
            Command::Reconnect => {
                self.refused = false;
                self.client = None;
                self.tokens = None;
                self.sync_all(commands);
            }
            Command::Senders(c) => self.senders(c),
            Command::Stop => self.stop = true,
        }
    }

    /// During a long sync: what the user is waiting for is done now (see the IMAP worker).
    fn serve_waiting(&mut self, commands: &Receiver<Command>) -> bool {
        let mut served = false;
        while let Ok(c) = commands.try_recv() {
            served = true;
            match c {
                Command::Open { folder, uid } => self.open(&folder, uid),
                Command::Search { id, query } => {
                    let hits = local::search(&self.store, &self.folders, &query);
                    self.emit(Event::SearchResults { account: self.id, id, hits });
                }
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

    // ---- Signing in ----

    /// The Gmail client with a fresh access token, signing in if needed. `None` when offline or
    /// refused (already reported).
    fn connect(&mut self) -> Option<&mut GmailClient> {
        let fresh = self.tokens.as_ref().is_some_and(|t| !t.expired());
        if self.client.is_none() || !fresh {
            if self.refused {
                return None;
            }
            let Some(provider) = oauth::google() else {
                self.refused = true;
                self.link(Link::SignInRefused, "This copy of MailALM was built without Google sign-in.");
                return None;
            };
            let first = self.client.is_none();
            if first {
                self.link(Link::Connecting, "Signing in to Google…");
            }
            match self.refresh(&provider) {
                Ok(t) => {
                    if let Some(c) = self.client.as_mut() {
                        c.set_token(&t.access);
                    } else {
                        self.client = Some(GmailClient::new(&t.access));
                    }
                    self.tokens = Some(t);
                    if first {
                        self.link(Link::Online, "gmail.googleapis.com");
                        self.flush_pending();
                    }
                }
                Err(Refresh::Refused(text)) => {
                    self.refused = true;
                    self.client = None;
                    self.tokens = None;
                    self.link(Link::SignInRefused, text);
                    return None;
                }
                Err(Refresh::Offline(text)) => {
                    self.client = None;
                    self.tokens = None;
                    self.link(Link::Offline, text);
                    return None;
                }
            }
        }
        self.client.as_mut()
    }

    /// A new access token from the refresh token kept in the keychain. Google keeps the same
    /// refresh token; if it ever sends a new one, it replaces the old one.
    fn refresh(&self, provider: &Provider) -> Result<Tokens, Refresh> {
        let kept = match secrets::refresh_token(self.id) {
            Ok(Some(t)) => t,
            Ok(None) => return Err(Refresh::Refused("MailALM is not signed in to this account. Sign in again from the account's settings.".to_owned())),
            Err(e) => return Err(Refresh::Offline(e)),
        };
        match oauth::refresh(provider, &kept) {
            Ok(t) => {
                if let Some(new) = t.refresh.as_deref()
                    && new != kept
                    && let Err(e) = secrets::set_refresh_token(self.id, new)
                {
                    self.error(e);
                }
                Ok(t)
            }
            Err(e) if e.is_auth() => Err(Refresh::Refused(format!("{}. Sign in again from the account's settings.", describe(&e)))),
            Err(e) => Err(Refresh::Offline(describe(&e))),
        }
    }

    /// Runs one Gmail call. Refused once (the access token ran out early): signed in again and
    /// tried once more. A network error drops the client (the next call reconnects).
    fn call<T>(&mut self, mut step: impl FnMut(&mut GmailClient) -> Result<T, NetError>) -> Result<T, NetError> {
        for attempt in 0..2 {
            let Some(c) = self.connect() else { return Err(NetError::Io("offline".to_owned())) };
            match step(c) {
                Err(e) if e.is_auth() && attempt == 0 => {
                    // The token is taken for expired: the next connect asks for a new one.
                    self.tokens = None;
                }
                Err(e) => {
                    if e.is_fatal() {
                        self.client = None;
                    }
                    if e.is_auth() {
                        self.refused = true;
                        self.client = None;
                        self.link(Link::SignInRefused, format!("{}. Sign in again from the account's settings.", describe(&e)));
                    }
                    return Err(e);
                }
                ok => return ok,
            }
        }
        Err(NetError::Auth("Google refused the sign-in".to_owned()))
    }

    fn online(&self) -> bool {
        self.client.is_some()
    }

    /// The mailbox's history id now: it changes with anything that changes in the mailbox.
    /// `None` when it can't be read (every folder is then listed).
    fn history(&mut self) -> Option<u64> {
        self.call(GmailClient::profile).ok().map(|p| p.history_id)
    }

    // ---- Sync ----

    /// The labels as MailALM's folders: the system ones it shows, All Mail, the user's.
    fn folders_of(&mut self) -> Result<Vec<Folder>, NetError> {
        let labels = self.call(GmailClient::labels)?;
        let mut out: Vec<Folder> = Vec::new();
        let mut taken: HashSet<String> = HashSet::new();
        let folder = |path: String, wire: &str, role: FolderRole| Folder { path, wire: wire.to_owned(), delimiter: Some('/'), role, total: 0, unread: 0 };
        for (id, name, role) in SYSTEM {
            // The categories are always there; the other system labels only as Gmail lists them.
            if id.starts_with("CATEGORY_") || labels.iter().any(|l| l.id == id) {
                taken.insert(name.to_owned());
                out.push(folder(name.to_owned(), id, role));
            }
        }
        taken.insert(ALL_MAIL.to_owned());
        out.push(folder(ALL_MAIL.to_owned(), "", FolderRole::Archive));
        for l in labels.iter().filter(|l| l.kind == LabelKind::User) {
            let name = l.name.trim_matches('/').trim();
            if name.is_empty() {
                continue;
            }
            let path = unique(name.to_owned(), &taken);
            taken.insert(path.clone());
            out.push(folder(path, &l.id, FolderRole::Other));
        }
        // The counts are MailALM's own, from the cache.
        for f in &mut out {
            let (total, unread) = local::counts(&self.store.summaries(&f.path));
            f.total = total;
            f.unread = unread;
        }
        mailalm_model::sort_folders(&mut out);
        Ok(out)
    }

    fn sync_all(&mut self, commands: &Receiver<Command>) {
        if self.connect().is_none() {
            return;
        }
        self.activity(Some("Reading the labels…".to_owned()));
        let history = self.history();
        let folders = match self.folders_of() {
            Ok(f) => f,
            Err(e) => {
                self.error(format!("The labels could not be read: {}", describe(&e)));
                self.activity(None);
                return;
            }
        };
        self.folders = folders;
        let _ = self.store.save_folders(&self.folders);
        let _ = self.store.forget_folders_except(&self.folders);
        self.emit(Event::Folders { account: self.id, folders: self.folders.clone() });

        let mut order: Vec<Folder> = self.folders.clone();
        let viewed = self.viewed.clone();
        order.sort_by_key(|f| (f.role != FolderRole::Inbox, Some(&f.path) != viewed.as_ref()));
        for f in order {
            if self.stop || !self.online() && self.connect().is_none() {
                break;
            }
            self.sync_folder(&f, history, commands);
        }
        self.activity(None);
    }

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
        let history = self.history();
        for p in paths {
            if let Some(f) = self.folder(&p) {
                self.sync_folder(&f, history, commands);
            }
        }
        self.activity(None);
    }

    /// Syncs one folder. `history`: the mailbox's history id read just before (`None`: unknown).
    fn sync_folder(&mut self, folder: &Folder, history: Option<u64>, commands: &Receiver<Command>) {
        let path = folder.path.clone();
        let state = self.store.folder_state(&path).unwrap_or_else(|| FolderState { path: path.clone(), ..FolderState::default() });
        let mark = history.map(|h| h.to_string());
        if mark.is_some() && state.delta == mark {
            // Nothing changed in the mailbox since this folder's last complete sync.
            return;
        }
        self.activity(Some(format!("Syncing {}…", folder.name())));
        let wire = folder.wire.clone();
        let labels: Vec<&str> = if wire.is_empty() { Vec::new() } else { vec![wire.as_str()] };
        let listed = match self.call(|c| c.list_ids(&labels, MAX_LISTED)) {
            Ok(l) => l,
            Err(e) => {
                if self.online() {
                    self.error(format!("{} could not be synced: {}", folder.name(), describe(&e)));
                }
                return;
            }
        };
        let unread_labels: Vec<&str> = labels.iter().copied().chain(["UNREAD"]).collect();
        let unread = match self.call(|c| c.list_ids(&unread_labels, MAX_LISTED)) {
            Ok(u) => u,
            Err(e) => {
                if self.online() {
                    self.error(format!("{} could not be synced: {}", folder.name(), describe(&e)));
                }
                return;
            }
        };
        let starred = if wire == "STARRED" {
            listed.clone()
        } else {
            let starred_labels: Vec<&str> = labels.iter().copied().chain(["STARRED"]).collect();
            match self.call(|c| c.list_ids(&starred_labels, MAX_LISTED)) {
                Ok(s) => s,
                Err(e) => {
                    if self.online() {
                        self.error(format!("{} could not be synced: {}", folder.name(), describe(&e)));
                    }
                    return;
                }
            }
        };
        let listed_set: HashSet<&str> = listed.iter().map(String::as_str).collect();
        let unread_set: HashSet<&str> = unread.iter().map(String::as_str).collect();
        let starred_set: HashSet<&str> = starred.iter().map(String::as_str).collect();
        // A list cut at its limit says nothing of the messages past it.
        let unread_whole = unread.len() < MAX_LISTED;
        let starred_whole = starred.len() < MAX_LISTED;

        let mut ids = Ids::load(&self.store, &path, state.uidnext);
        let mut summaries = self.store.summaries(&path);
        // Gone from the folder (or past the newest MAX_LISTED).
        let gone: Vec<u32> =
            summaries.iter().map(|s| s.uid).filter(|u| ids.by_uid.get(u).is_none_or(|id| !listed_set.contains(id.as_str()))).collect();
        if !gone.is_empty() {
            summaries.retain(|s| !gone.contains(&s.uid));
            let _ = self.store.remove_messages(&path, &gone);
        }
        // New to the folder: read in detail, newest first, unless another folder has just read
        // them. Those a waiting change takes out of the folder are not.
        let mut have: HashSet<u32> = summaries.iter().map(|s| s.uid).collect();
        for c in self.store.pending().iter().filter(|c| c.folder() == path) {
            have.extend(local::removed_by(c).iter().copied());
        }
        let new: Vec<String> = listed.iter().filter(|id| ids.by_id.get(id.as_str()).is_none_or(|u| !have.contains(u))).cloned().collect();
        let mut complete = true;
        let mut fetched = 0usize;
        for id in &new {
            // A save after every SAVE_EVERY messages read from Gmail.
            let mut save = false;
            let summary = match self.details.get(id.as_str()).cloned() {
                Some(known) => Some(known),
                None => {
                    if fetched >= MAX_FETCH {
                        // The rest comes at the next sync.
                        complete = false;
                        break;
                    }
                    fetched += 1;
                    save = fetched % SAVE_EVERY == 0;
                    match self.call(|c| c.metadata(id)) {
                        Ok(Some(m)) => {
                            self.remember(id, &m.summary);
                            Some(m.summary)
                        }
                        Ok(None) => None,
                        Err(e) => {
                            if self.online() {
                                self.error(format!("{} could not be synced: {}", folder.name(), describe(&e)));
                            }
                            complete = false;
                            break;
                        }
                    }
                }
            };
            if let Some(mut s) = summary {
                s.uid = ids.number(id);
                summaries.push(s);
            }
            if save {
                self.activity(Some(format!("Syncing {}… {} messages", folder.name(), summaries.len())));
                self.save_folder(&path, &mut summaries, &mut ids);
                if self.serve_waiting(commands) {
                    local::overlay_pending(&self.store, &path, &mut summaries);
                }
                if self.stop {
                    complete = false;
                    break;
                }
            }
        }
        // Marks, from the lists: a message read or starred elsewhere is shown so.
        for s in &mut summaries {
            let Some(id) = ids.by_uid.get(&s.uid) else { continue };
            let is_unread = unread_set.contains(id.as_str());
            if unread_whole || is_unread {
                s.flags.seen = !is_unread;
            }
            let is_starred = starred_set.contains(id.as_str());
            if starred_whole || is_starred {
                s.flags.flagged = is_starred;
            }
        }
        self.save_folder(&path, &mut summaries, &mut ids);
        // The history id is kept only once the folder is whole: else the next sync goes on.
        let delta = if complete { mark } else { None };
        let _ = self.store.save_folder_state(&FolderState { path: path.clone(), uidvalidity: 1, uidnext: ids.next, delta });
        self.update_counts(&path, &summaries);
        if complete {
            self.prefetch(folder, &summaries, &ids, commands);
            self.flush_pending();
        }
    }

    /// Keeps a message's details for the other folders it is in.
    fn remember(&mut self, id: &str, summary: &Summary) {
        if self.details.len() >= MAX_DETAILS {
            self.details.clear();
        }
        self.details.insert(id.to_owned(), summary.clone());
    }

    /// Saves and shows a folder's messages, with the changes still waiting for the server. The
    /// numbers kept are those of the messages shown and of the changes waiting (which need them).
    fn save_folder(&self, path: &str, summaries: &mut Vec<Summary>, ids: &mut Ids) {
        local::overlay_pending(&self.store, path, summaries);
        sort_newest_first(summaries);
        let mut keep: HashSet<u32> = summaries.iter().map(|s| s.uid).collect();
        for c in self.store.pending().iter().filter(|c| c.folder() == path) {
            keep.extend(local::removed_by(c).iter().copied());
        }
        let unused: Vec<u32> = ids.by_uid.keys().copied().filter(|u| !keep.contains(u)).collect();
        for u in unused {
            ids.forget(u);
        }
        let _ = self.store.save_ids(path, &ids.pairs());
        let _ = self.store.save_summaries(path, summaries);
        self.emit(Event::Summaries { account: self.id, folder: path.to_owned(), summaries: summaries.clone() });
    }

    /// Fetches the newest messages whole, for the search (and a longer preview).
    fn prefetch(&mut self, folder: &Folder, summaries: &[Summary], ids: &Ids, commands: &Receiver<Command>) {
        let path = folder.path.clone();
        let limit = if folder.role == FolderRole::Inbox { PREFETCH_INBOX } else { PREFETCH_OTHER };
        let wanted: Vec<(u32, String)> = summaries
            .iter()
            .take(limit)
            .filter(|s| !s.has_attachments && s.size <= PREFETCH_MAX_SIZE && !self.store.has_message(&path, s.uid))
            .filter_map(|s| ids.by_uid.get(&s.uid).map(|id| (s.uid, id.clone())))
            .collect();
        for (uid, id) in wanted {
            match self.call(|c| c.download(&id)) {
                Ok(Some(raw)) => {
                    if let Some(parsed) = mailalm_mime::parse(&raw) {
                        let _ = self.store.save_message(&path, uid, &raw, &parsed.text);
                    }
                }
                Ok(None) => {}
                Err(_) => break,
            }
            let _ = self.serve_waiting(commands);
            if self.stop || !self.online() {
                break;
            }
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

    /// The server's id of a message of the cache.
    fn id_of(&self, path: &str, uid: u32) -> Option<String> {
        self.store.ids(path).into_iter().find(|(u, _)| *u == uid).map(|(_, id)| id)
    }

    // ---- Reading ----

    fn open(&mut self, path: &str, uid: u32) {
        let unavailable = |w: &Self, reason: String| {
            w.emit(Event::MessageUnavailable { account: w.id, folder: path.to_owned(), uid, reason });
        };
        let raw = match self.store.message(path, uid) {
            Some(raw) => raw,
            None => {
                let Some(id) = self.id_of(path, uid) else {
                    unavailable(self, "This message is no longer in the folder.".to_owned());
                    return;
                };
                match self.call(|c| c.download(&id)) {
                    Ok(Some(raw)) => raw,
                    Ok(None) => {
                        unavailable(self, "This message is no longer on the server.".to_owned());
                        return;
                    }
                    Err(e) => {
                        let reason = if self.online() {
                            describe(&e)
                        } else {
                            format!("This message is not in the cache, and the server cannot be reached: {}", describe(&e))
                        };
                        unavailable(self, reason);
                        return;
                    }
                }
            }
        };
        let Some(parsed) = mailalm_mime::parse(&raw) else {
            unavailable(self, "This message could not be read.".to_owned());
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

    fn apply(&mut self, change: Change, commands: &Receiver<Command>) {
        let moved = self.moved_to(&change);
        self.apply_local(change);
        if self.connect().is_some() {
            self.flush_pending();
            if let Some(f) = moved.and_then(|to| self.folder(&to)) {
                let history = self.history();
                self.sync_folder(&f, history, commands);
            }
            self.activity(None);
        }
    }

    /// The folder a change puts messages in, to sync it after.
    fn moved_to(&self, change: &Change) -> Option<String> {
        match change {
            Change::Move { to, .. } => Some(to.clone()),
            Change::MarkJunk { junk, .. } => self.folder_with_role(if *junk { FolderRole::Junk } else { FolderRole::Inbox }).map(|f| f.path),
            Change::Delete { .. } => self.folder_with_role(FolderRole::Trash).map(|f| f.path),
            _ => None,
        }
    }

    /// Makes the change in the cache now and queues it for the server. The ids of messages taken
    /// out of the folder are kept until the server has the change (it needs them).
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
        self.emit(Event::Summaries { account: self.id, folder: path, summaries });

        let mut pending = self.store.pending();
        pending.push(change);
        if let Err(e) = self.store.save_pending(&pending) {
            self.error(format!("The change could not be kept for the server: {e}"));
        }
    }

    fn flush_pending(&mut self) {
        if self.flushing || self.store.pending().is_empty() {
            return;
        }
        self.flushing = true;
        self.flush_pending_now();
        self.flushing = false;
    }

    /// Sends the waiting changes, in order. A change Gmail refuses is dropped (and said); a
    /// network failure keeps it and the rest for next time.
    fn flush_pending_now(&mut self) {
        let mut pending = self.store.pending();
        while let Some(change) = pending.first().cloned() {
            let path = change.folder().to_owned();
            let all = self.store.ids(&path);
            let uids: &[u32] = match &change {
                Change::SetSeen { uids, .. } | Change::SetFlagged { uids, .. } | Change::Move { uids, .. } | Change::Delete { uids, .. } | Change::MarkJunk { uids, .. } => {
                    uids
                }
            };
            let ids: Vec<String> = all.iter().filter(|(u, _)| uids.contains(u)).map(|(_, id)| id.clone()).collect();
            // Messages the server no longer has: nothing to send.
            let result = if ids.is_empty() { Ok(()) } else { self.send_change(&change, &ids) };
            match result {
                Ok(()) => {
                    pending.remove(0);
                    if !local::removed_by(&change).is_empty() {
                        let kept: Vec<(u32, String)> = self.store.ids(&path).into_iter().filter(|(u, _)| !uids.contains(u)).collect();
                        let _ = self.store.save_ids(&path, &kept);
                    }
                }
                Err(e) if self.online() || matches!(e, NetError::Invalid(_)) => {
                    self.error(format!("Gmail refused a change in {path}: {}", describe(&e)));
                    pending.remove(0);
                }
                Err(_) => break,
            }
            let _ = self.store.save_pending(&pending);
        }
    }

    /// One change as Gmail's labels have it.
    fn send_change(&mut self, change: &Change, ids: &[String]) -> Result<(), NetError> {
        match change {
            Change::SetSeen { seen: true, .. } => self.call(|c| c.modify(ids, &[], &["UNREAD"])),
            Change::SetSeen { seen: false, .. } => self.call(|c| c.modify(ids, &["UNREAD"], &[])),
            Change::SetFlagged { flagged: true, .. } => self.call(|c| c.modify(ids, &["STARRED"], &[])),
            Change::SetFlagged { flagged: false, .. } => self.call(|c| c.modify(ids, &[], &["STARRED"])),
            // Reported as spam: Gmail's own "Report spam" is the SPAM label.
            Change::MarkJunk { junk: true, .. } => self.call(|c| c.modify(ids, &["SPAM"], &["INBOX"])),
            Change::MarkJunk { junk: false, .. } => self.call(|c| c.modify(ids, &["INBOX"], &["SPAM"])),
            // MailALM may not delete Gmail messages for good (it asks only for gmail.modify): to
            // the Trash, which Gmail empties after 30 days.
            Change::Delete { .. } => self.call(|c| c.trash(ids)),
            Change::Move { folder, to, .. } => {
                let from = self.folder(folder).ok_or_else(|| NetError::Invalid(format!("the folder {folder} no longer exists")))?;
                let to = self.folder(to).ok_or_else(|| NetError::Invalid(format!("the folder {to} no longer exists")))?;
                self.move_between(&from, &to, ids)
            }
        }
    }

    /// A move from one folder to another: labels added and taken off.
    fn move_between(&mut self, from: &Folder, to: &Folder, ids: &[String]) -> Result<(), NetError> {
        if to.role == FolderRole::Trash {
            return self.call(|c| c.trash(ids));
        }
        if matches!(to.wire.as_str(), "SENT" | "DRAFT") {
            return Err(NetError::Invalid(format!("Gmail cannot put a message in {}", to.name())));
        }
        let add: Vec<&str> = if to.wire.is_empty() { Vec::new() } else { vec![to.wire.as_str()] };
        if from.role == FolderRole::Trash {
            self.call(|c| c.untrash(ids))?;
            return if add.is_empty() { Ok(()) } else { self.call(|c| c.modify(ids, &add, &[])) };
        }
        let mut remove: Vec<&str> = Vec::new();
        if to.role == FolderRole::Junk {
            // As "Report spam": out of the Inbox too.
            remove.push("INBOX");
        } else if from.role == FolderRole::Junk {
            remove.push("SPAM");
        }
        if removable(&from.wire) && !remove.contains(&from.wire.as_str()) {
            remove.push(from.wire.as_str());
        }
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        self.call(|c| c.modify(ids, &add, &remove))
    }

    // ---- Senders ----

    /// Blocks, allows or forgets a sender with a Gmail filter, then lists MailALM's filters.
    fn senders(&mut self, command: SenderCommand) {
        if self.connect().is_none() {
            self.error("Gmail cannot be reached: the blocked senders can be changed once MailALM is online.");
            return;
        }
        let done = match command {
            SenderCommand::List => Ok(()),
            SenderCommand::Block(address) => self.set_sender(&address, true),
            SenderCommand::Allow(address) => self.set_sender(&address, false),
            SenderCommand::Remove(id) => self.call(|c| c.delete_filter(&id)).map_err(|e| format!("The rule could not be removed: {}", describe(&e))),
        };
        if let Err(e) = done {
            self.error(e);
        }
        match self.call(GmailClient::filters) {
            Ok(filters) => {
                let rules = filters
                    .into_iter()
                    .map(|f| SenderRule {
                        id: f.id,
                        address: f.from,
                        kind: match f.action {
                            FilterAction::Spam => SenderRuleKind::Spam,
                            FilterAction::Trash => SenderRuleKind::Trash,
                            FilterAction::NeverSpam => SenderRuleKind::NeverSpam,
                        },
                    })
                    .collect();
                self.emit(Event::SenderRules { account: self.id, rules });
            }
            Err(e) => self.error(format!("Gmail's filters could not be read: {}", describe(&e))),
        }
    }

    /// One filter for the address: to Spam (else to the Trash, if Gmail refuses Spam as a
    /// filter's action) when blocking, never to Spam when allowing. MailALM's filters for the
    /// same address that say otherwise are removed; one that already says so is kept.
    fn set_sender(&mut self, address: &str, block: bool) -> Result<(), String> {
        let address = address.trim().to_owned();
        let existing = self.call(GmailClient::filters).map_err(|e| format!("Gmail's filters could not be read: {}", describe(&e)))?;
        let same: Vec<_> = existing.into_iter().filter(|f| f.from.eq_ignore_ascii_case(&address)).collect();
        let wanted = |a: FilterAction| if block { a != FilterAction::NeverSpam } else { a == FilterAction::NeverSpam };
        let already = same.iter().any(|f| wanted(f.action));
        for f in same.iter().filter(|f| !wanted(f.action)) {
            self.call(|c| c.delete_filter(&f.id)).map_err(|e| format!("The rule for {address} could not be changed: {}", describe(&e)))?;
        }
        if already {
            return Ok(());
        }
        if !block {
            return self.call(|c| c.create_filter(&address, FilterAction::NeverSpam)).map(drop).map_err(|e| format!("{address} could not be allowed: {}", describe(&e)));
        }
        match self.call(|c| c.create_filter(&address, FilterAction::Spam)) {
            Ok(_) => Ok(()),
            // Gmail refused Spam as the filter's action (PLAN.md, Q6): to the Trash instead.
            Err(NetError::Server(_)) => {
                self.call(|c| c.create_filter(&address, FilterAction::Trash)).map(drop).map_err(|e| format!("{address} could not be blocked: {}", describe(&e)))
            }
            Err(e) => Err(format!("{address} could not be blocked: {}", describe(&e))),
        }
    }

    // ---- Sending ----

    fn send_message(&mut self, message: Outgoing, commands: &Receiver<Command>) {
        // Gmail takes the recipients from the headers: Bcc is written (and removed by Gmail).
        let (raw, _id) = match mailalm_mime::build_for_api(&message) {
            Ok(r) => r,
            Err(e) => {
                self.emit(Event::SendFailed { account: self.id, text: e, message: Box::new(message) });
                return;
            }
        };
        self.activity(Some(format!("Sending \"{}\"…", message.subject)));
        if let Err(e) = self.call(|c| c.send_mime(&raw)) {
            self.activity(None);
            let text = format!("The message was not sent: {}", describe(&e));
            self.emit(Event::SendFailed { account: self.id, text, message: Box::new(message) });
            return;
        }
        self.emit(Event::Sent { account: self.id, subject: message.subject.clone() });
        // Gmail keeps the copy in Sent itself.
        if let Some(sent) = self.folder_with_role(FolderRole::Sent) {
            let history = self.history();
            self.sync_folder(&sent, history, commands);
        }
        self.activity(None);
    }
}

enum Refresh {
    /// Google refused: sign in again.
    Refused(String),
    /// No network, or Google busy: tried again later.
    Offline(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_taken_off_a_message_that_leaves() {
        assert!(removable("INBOX"));
        assert!(removable("CATEGORY_PROMOTIONS"));
        assert!(removable("Label_12"));
        assert!(!removable("SENT"));
        assert!(!removable("DRAFT"));
        assert!(!removable("STARRED"));
        assert!(!removable(""));
    }

    #[test]
    fn folder_names_stay_unique() {
        let taken: HashSet<String> = ["Social".to_owned(), "Social (2)".to_owned()].into_iter().collect();
        assert_eq!(unique("Social".to_owned(), &taken), "Social (3)");
        assert_eq!(unique("Work".to_owned(), &taken), "Work");
    }
}
