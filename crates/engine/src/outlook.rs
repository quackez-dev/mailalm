//! One Outlook account's worker (Hotmail, Outlook.com): its cache, its Microsoft Graph client, its
//! queue of changes (PLAN.md, section 5.1).
//!
//! Graph names messages by long ids; the interface and the cache by small numbers. Each folder
//! keeps the pairs (`ids.json`), and gives the next number from `FolderState::uidnext`. Ids are
//! Graph's immutable ones: a message keeps its id when it moves.
//!
//! A folder is synced with Graph's delta query: the first pass lists every message, later passes
//! only what changed since, from the delta link kept in the folder's state.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use mailalm_mime::Outgoing;
use mailalm_model::{Account, AccountId, Change, Folder, FolderRole, Summary};
use mailalm_net::NetError;
use mailalm_net::graph::{DeltaNext, GraphClient};
use mailalm_net::oauth::{self, Provider, Tokens};
use mailalm_store::{FolderState, Store};

use crate::ids::Ids;
use crate::local::{self, sort_newest_first};
use crate::{Command, Event, Link, Wake, secrets};

/// How often the Inbox and the open folder are checked for new mail.
const CHECK_EVERY: Duration = Duration::from_secs(3 * 60);
/// How often every folder is synced.
const SYNC_ALL_EVERY: Duration = Duration::from_secs(30 * 60);
/// Messages fetched whole ahead of time (for the search): in the Inbox, in other folders. Graph
/// lists no sizes: messages with attachments wait until they are opened.
const PREFETCH_INBOX: usize = 100;
const PREFETCH_OTHER: usize = 20;
/// Delta pages between two saves of a folder being synced for the first time.
const SAVE_EVERY_PAGES: usize = 10;
/// The most delta pages one sync of a folder reads (200 messages each).
const MAX_PAGES: usize = 2_000;

pub fn run(account: Account, base: PathBuf, commands: Receiver<Command>, events: Sender<Event>, wake: Wake) {
    let store = match Store::open(&base, account.id) {
        Ok(s) => s,
        Err(e) => {
            let _ = events.send(Event::Error { account: account.id, text: format!("The cache of {} could not be opened: {e}", account.name) });
            wake();
            return;
        }
    };
    let mut w = Outlook {
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

struct Outlook {
    id: AccountId,
    store: Store,
    events: Sender<Event>,
    wake: Wake,
    client: Option<GraphClient>,
    tokens: Option<Tokens>,
    folders: Vec<Folder>,
    viewed: Option<String>,
    /// Microsoft refused the sign-in: no new attempt until `Reconnect`.
    refused: bool,
    stop: bool,
    flushing: bool,
    queued: Vec<Command>,
}

/// An error as the user reads it. Never holds a token: the network layer never puts one in.
pub(crate) fn describe(e: &NetError) -> String {
    if e.is_auth() { format!("Microsoft refused the sign-in: {e}") } else { e.to_string() }
}

impl Outlook {
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
                    self.sync_folder(&f, commands);
                    self.activity(None);
                }
            }
            Command::Open { folder, uid } => self.open(&folder, uid),
            Command::Attachment { folder, uid, index } => self.emit(local::review(&self.store, self.id, &folder, uid, index)),
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
            Command::Senders(_) => self.error("Microsoft's API cannot change Outlook.com's blocked senders: block a sender in Outlook.com's settings (Mail, Junk email)."),
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
                Command::Attachment { folder, uid, index } => self.emit(local::review(&self.store, self.id, &folder, uid, index)),
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

    /// The Graph client with a fresh access token, signing in if needed. `None` when offline or
    /// refused (already reported).
    fn connect(&mut self) -> Option<&mut GraphClient> {
        let fresh = self.tokens.as_ref().is_some_and(|t| !t.expired());
        if self.client.is_none() || !fresh {
            if self.refused {
                return None;
            }
            let Some(provider) = oauth::microsoft() else {
                self.refused = true;
                self.link(Link::SignInRefused, "This copy of MailALM was built without Microsoft sign-in.");
                return None;
            };
            let first = self.client.is_none();
            if first {
                self.link(Link::Connecting, "Signing in to Microsoft…");
            }
            match self.refresh(&provider) {
                Ok(t) => {
                    if let Some(c) = self.client.as_mut() {
                        c.set_token(&t.access);
                    } else {
                        self.client = Some(GraphClient::new(&t.access));
                    }
                    self.tokens = Some(t);
                    if first {
                        self.link(Link::Online, "graph.microsoft.com");
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

    /// A new access token from the refresh token kept in the keychain. Microsoft may hand out a
    /// new refresh token at the same time: it replaces the old one.
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

    /// Runs one Graph call. Refused once (the access token ran out early): signed in again and
    /// tried once more. A network error drops the client (the next call reconnects).
    fn call<T>(&mut self, mut step: impl FnMut(&mut GraphClient) -> Result<T, NetError>) -> Result<T, NetError> {
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
        Err(NetError::Auth("Microsoft refused the sign-in".to_owned()))
    }

    fn online(&self) -> bool {
        self.client.is_some()
    }

    // ---- Sync ----

    fn sync_all(&mut self, commands: &Receiver<Command>) {
        if self.connect().is_none() {
            return;
        }
        self.activity(Some("Reading the folder list…".to_owned()));
        let mut folders = match self.call(GraphClient::folders) {
            Ok(f) => f,
            Err(e) => {
                self.error(format!("The folder list could not be read: {}", describe(&e)));
                self.activity(None);
                return;
            }
        };
        mailalm_model::sort_folders(&mut folders);
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
            self.sync_folder(&f, commands);
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
        let state = self.store.folder_state(&path).unwrap_or_else(|| FolderState { path: path.clone(), ..FolderState::default() });
        let mut summaries = self.store.summaries(&path);
        let mut ids = Ids::load(&self.store, &path, state.uidnext);
        let mut link = state.delta.clone();
        let mut full = link.is_none();
        // A full pass lists every message: those it doesn't list are gone.
        let mut listed: HashSet<u32> = HashSet::new();
        let mut restarted = false;
        let mut pages = 0;
        let done_link = loop {
            if pages >= MAX_PAGES {
                self.error(format!("{} has more messages than MailALM reads in one sync; the rest comes at the next one.", folder.name()));
                break None;
            }
            pages += 1;
            let wire = folder.wire.clone();
            let from = link.clone();
            let page = match self.call(|c| c.delta(&wire, from.as_deref())) {
                Ok(p) => p,
                Err(NetError::Resync(_)) if !restarted => {
                    // Graph forgot where the last sync ended: start the folder over.
                    restarted = true;
                    let _ = self.store.clear_folder(&path);
                    summaries.clear();
                    ids = Ids::load(&self.store, &path, 1);
                    link = None;
                    full = true;
                    listed.clear();
                    continue;
                }
                Err(e) => {
                    if self.online() {
                        self.error(format!("{} could not be synced: {}", folder.name(), describe(&e)));
                    }
                    break None;
                }
            };
            for id in &page.removed {
                if let Some(uid) = ids.by_id.get(id).copied() {
                    summaries.retain(|s| s.uid != uid);
                    let _ = self.store.remove_messages(&path, &[uid]);
                    ids.forget(uid);
                }
            }
            for m in page.changed {
                let uid = ids.number(&m.id);
                listed.insert(uid);
                let mut s = m.summary;
                s.uid = uid;
                match summaries.iter_mut().find(|o| o.uid == uid) {
                    Some(old) => {
                        // Graph's preview is short and plain; one taken from the whole message stays.
                        if !old.preview.is_empty() && s.preview.is_empty() {
                            s.preview = std::mem::take(&mut old.preview);
                        }
                        *old = s;
                    }
                    None => summaries.push(s),
                }
            }
            match page.next {
                DeltaNext::Done(d) => break Some(d),
                DeltaNext::More(next) => {
                    link = Some(next);
                    if full && (pages <= 2 || pages % SAVE_EVERY_PAGES == 0) {
                        self.activity(Some(format!("Syncing {}… {} messages", folder.name(), summaries.len())));
                        self.save_folder(&path, &mut summaries, &ids);
                    }
                    if self.serve_waiting(commands) {
                        local::overlay_pending(&self.store, &path, &mut summaries);
                    }
                    if self.stop {
                        break None;
                    }
                }
            }
        };

        if full && done_link.is_some() {
            let gone: Vec<u32> = summaries.iter().map(|s| s.uid).filter(|u| !listed.contains(u)).collect();
            if !gone.is_empty() {
                summaries.retain(|s| listed.contains(&s.uid));
                let _ = self.store.remove_messages(&path, &gone);
                for u in gone {
                    ids.forget(u);
                }
            }
        }
        self.save_folder(&path, &mut summaries, &ids);
        if let Some(d) = &done_link {
            let _ = self.store.save_folder_state(&FolderState { path: path.clone(), uidvalidity: 1, uidnext: ids.next, delta: Some(d.clone()) });
        } else if full {
            // An unfinished first pass starts over next time; the numbers given so far are kept.
            let _ = self.store.save_folder_state(&FolderState { path: path.clone(), uidvalidity: 1, uidnext: ids.next, delta: None });
        }
        self.update_counts(&path, &summaries);
        if done_link.is_some() {
            self.prefetch(folder, &summaries, &ids, commands);
            self.flush_pending();
        }
    }

    /// Saves and shows a folder's messages, with the changes still waiting for the server.
    fn save_folder(&self, path: &str, summaries: &mut Vec<Summary>, ids: &Ids) {
        local::overlay_pending(&self.store, path, summaries);
        sort_newest_first(summaries);
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
            .filter(|s| !s.has_attachments && !self.store.has_message(&path, s.uid))
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
                self.sync_folder(&f, commands);
            }
            self.activity(None);
        }
    }

    /// The folder a change puts messages in, to sync it after.
    fn moved_to(&self, change: &Change) -> Option<String> {
        match change {
            Change::Move { to, .. } => Some(to.clone()),
            Change::MarkJunk { junk, .. } => self.folder_with_role(if *junk { FolderRole::Junk } else { FolderRole::Inbox }).map(|f| f.path),
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

    /// Sends the waiting changes, in order. A change Graph refuses is dropped (and said); a
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
            let dest = match &change {
                Change::Move { to, .. } => self.folder(to).map(|f| f.wire),
                _ => None,
            };
            let result = if ids.is_empty() {
                // Messages the server no longer has: nothing to send.
                Ok(())
            } else {
                match &change {
                    Change::SetSeen { seen, .. } => self.call(|c| c.set_read(&ids, *seen)),
                    Change::SetFlagged { flagged, .. } => self.call(|c| c.set_flagged(&ids, *flagged)),
                    Change::Move { to, .. } => match dest.as_deref() {
                        Some(d) => self.call(|c| c.move_to(&ids, d)),
                        None => Err(NetError::Invalid(format!("the folder {to} no longer exists"))),
                    },
                    Change::Delete { .. } => self.call(|c| c.delete(&ids)),
                    Change::MarkJunk { junk, .. } => self.call(|c| c.mark_junk(&ids, *junk)),
                }
            };
            match result {
                Ok(()) => {
                    pending.remove(0);
                    if !local::removed_by(&change).is_empty() {
                        let kept: Vec<(u32, String)> = self.store.ids(&path).into_iter().filter(|(u, _)| !uids.contains(u)).collect();
                        let _ = self.store.save_ids(&path, &kept);
                    }
                }
                Err(e) if self.online() || matches!(e, NetError::Invalid(_)) => {
                    self.error(format!("Outlook refused a change in {path}: {}", describe(&e)));
                    pending.remove(0);
                }
                Err(_) => break,
            }
            let _ = self.store.save_pending(&pending);
        }
    }

    // ---- Sending ----

    fn send_message(&mut self, message: Outgoing, commands: &Receiver<Command>) {
        // Graph takes the recipients from the headers: Bcc is written (and removed by Graph).
        let (raw, _id) = match mailalm_mime::build_for_api(&message) {
            Ok(r) => r,
            Err(e) => {
                self.emit(Event::SendFailed { account: self.id, text: e, message: Box::new(message) });
                return;
            }
        };
        // Graph's sendMail takes about 4 MB per request, and the message goes in base64. Larger
        // ones need an upload session, which MailALM does not do yet.
        if raw.len() > crate::OUTLOOK_MESSAGE_LIMIT {
            let text = format!(
                "The message was not sent: it weighs {:.1} MB, and MailALM sends at most {:.0} MB from an Outlook account for now. Remove or shrink an attachment, or share the file by a link.",
                raw.len() as f64 / 1e6,
                crate::OUTLOOK_MESSAGE_LIMIT as f64 / 1e6
            );
            self.emit(Event::SendFailed { account: self.id, text, message: Box::new(message) });
            return;
        }
        self.activity(Some(format!("Sending \"{}\"…", message.subject)));
        if let Err(e) = self.call(|c| c.send_mime(&raw)) {
            self.activity(None);
            let text = format!("The message was not sent: {}", describe(&e));
            self.emit(Event::SendFailed { account: self.id, text, message: Box::new(message) });
            return;
        }
        self.emit(Event::Sent { account: self.id, subject: message.subject.clone() });
        // Outlook keeps the copy in Sent Items itself.
        if let Some(sent) = self.folder_with_role(FolderRole::Sent) {
            self.sync_folder(&sent, commands);
        }
        self.activity(None);
    }
}

enum Refresh {
    /// Microsoft refused: sign in again.
    Refused(String),
    /// No network, or Microsoft busy: tried again later.
    Offline(String),
}
