//! The engine between the interface and the servers.
//!
//! Each account has its own worker thread (IMAP, Outlook through Microsoft Graph, Gmail through its API), its own cache ([`mailalm_store::Store`]) and its own
//! connection: one slow or failing account never holds up another (PLAN.md, section 4.3). The
//! interface sends [`Command`]s and reads [`Event`]s; it never waits on the network.
//!
//! A worker shows the cache first, then syncs with the server. Changes (read, flag, move, delete)
//! are made in the cache at once and kept in order until the server has them, so they also work
//! offline (C7).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use mailalm_mime::{Outgoing, Parsed};
use mailalm_model::{Account, AccountId, Change, Folder, Provider, Summary};

mod gmail;
mod ids;
mod local;
mod outlook;
pub mod secrets;
mod signin;
mod worker;

pub use signin::{OAuthAccount, google_available, google_sign_in, microsoft_available, microsoft_sign_in};

/// What the interface asks of an account's worker.
#[derive(Debug)]
pub enum Command {
    /// Syncs every folder (at start, and on ↻).
    SyncAll,
    /// The user opened this folder: sync it first.
    View(String),
    /// Opens a message: from the cache, or fetched from the server.
    Open { folder: String, uid: u32 },
    /// A change made by the user.
    Apply(Change),
    /// Sends a message, then keeps a copy in the Sent folder.
    Send(Box<Outgoing>),
    /// Searches the cache. `id` comes back with the results.
    Search { id: u64, query: String },
    /// Signs in again (the password was changed).
    Reconnect,
    /// The account's blocked and allowed senders, kept on the server (Gmail filters). Other
    /// accounts answer with an error.
    Senders(SenderCommand),
    Stop,
}

/// What is asked about the senders an account blocks or allows on its server.
#[derive(Debug)]
pub enum SenderCommand {
    /// Lists them: the answer is [`Event::SenderRules`].
    List,
    /// Blocks this address: its messages go to Spam (or, if Gmail refuses that, to the Trash).
    Block(String),
    /// Never sends this address to Spam.
    Allow(String),
    /// Removes the rule with this id.
    Remove(String),
}

/// What a rule on the server does with a sender's messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SenderRuleKind {
    /// Sent to Spam.
    Spam,
    /// Sent to the Trash (Gmail refused Spam as a filter's action).
    Trash,
    /// Never sent to Spam.
    NeverSpam,
}

/// One rule on the server about a sender, made by MailALM (or of the same shape).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenderRule {
    /// The server's id for it, to remove it.
    pub id: String,
    pub address: String,
    pub kind: SenderRuleKind,
}

/// How the account's connection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Not connected yet, or between two syncs.
    Idle,
    Connecting,
    Online,
    /// The last attempt failed (no network, server down): the cache is shown.
    Offline,
    /// The server refused the password: nothing is tried until it is changed.
    SignInRefused,
}

/// What a worker tells the interface.
#[derive(Debug)]
pub enum Event {
    Folders { account: AccountId, folders: Vec<Folder> },
    /// The folder's summaries, newest first.
    Summaries { account: AccountId, folder: String, summaries: Vec<Summary> },
    Message { account: AccountId, folder: String, uid: u32, parsed: Box<Parsed> },
    /// A message could not be shown (gone from the server, or offline and not in the cache).
    MessageUnavailable { account: AccountId, folder: String, uid: u32, reason: String },
    Link { account: AccountId, link: Link, detail: String },
    /// What the worker is doing ("Syncing Inbox…"); `None` when it is done.
    Activity { account: AccountId, text: Option<String> },
    /// Something failed that the user should know about.
    Error { account: AccountId, text: String },
    Sent { account: AccountId, subject: String },
    /// The message was not sent: it comes back so nothing typed is lost.
    SendFailed { account: AccountId, text: String, message: Box<Outgoing> },
    SearchResults { account: AccountId, id: u64, hits: Vec<(String, Summary)> },
    /// The senders the account blocks or allows on its server, after [`SenderCommand`]s.
    SenderRules { account: AccountId, rules: Vec<SenderRule> },
}

impl Event {
    pub fn account(&self) -> AccountId {
        match self {
            Event::Folders { account, .. }
            | Event::Summaries { account, .. }
            | Event::Message { account, .. }
            | Event::MessageUnavailable { account, .. }
            | Event::Link { account, .. }
            | Event::Activity { account, .. }
            | Event::Error { account, .. }
            | Event::Sent { account, .. }
            | Event::SendFailed { account, .. }
            | Event::SearchResults { account, .. }
            | Event::SenderRules { account, .. } => *account,
        }
    }
}

/// Wakes the interface when an event is waiting.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

struct WorkerHandle {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

/// The workers of every account.
pub struct Engine {
    base: PathBuf,
    events_tx: Sender<Event>,
    events: Receiver<Event>,
    wake: Wake,
    workers: HashMap<AccountId, WorkerHandle>,
}

impl Engine {
    /// `base` is the cache's folder (one sub-folder per account).
    pub fn new(base: PathBuf, wake: Wake) -> Self {
        let (events_tx, events) = mpsc::channel();
        Self { base, events_tx, events, wake, workers: HashMap::new() }
    }

    /// Starts an account's worker: IMAP, Outlook (Microsoft Graph) or Gmail (its API).
    pub fn start(&mut self, account: Account) {
        self.stop(account.id);
        let (tx, rx) = mpsc::channel();
        let events = self.events_tx.clone();
        let wake = Arc::clone(&self.wake);
        let base = self.base.clone();
        let id = account.id;
        let provider = account.provider;
        let thread = std::thread::Builder::new()
            .name(format!("mailalm-account-{}", id.0))
            .spawn(move || match provider {
                Provider::Outlook => outlook::run(account, base, rx, events, wake),
                Provider::Gmail => gmail::run(account, base, rx, events, wake),
                _ => worker::run(account, base, rx, events, wake),
            })
            .ok();
        if thread.is_none() {
            let _ = self.events_tx.send(Event::Error { account: id, text: "The account's worker could not be started.".to_owned() });
        }
        self.workers.insert(id, WorkerHandle { commands: tx, thread });
    }

    /// Stops an account's worker (it finishes what it is doing first).
    pub fn stop(&mut self, id: AccountId) {
        if let Some(mut w) = self.workers.remove(&id) {
            let _ = w.commands.send(Command::Stop);
            // Not joined: a worker in the middle of a network call ends on its own.
            drop(w.thread.take());
        }
    }

    pub fn is_running(&self, id: AccountId) -> bool {
        self.workers.contains_key(&id)
    }

    /// Sends a command to an account's worker. `false` if the account has none.
    pub fn send(&self, id: AccountId, command: Command) -> bool {
        self.workers.get(&id).is_some_and(|w| w.commands.send(command).is_ok())
    }

    /// The events waiting, oldest first.
    pub fn poll(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// Removes an account: stops its worker, forgets its password or sign-in and, if asked, its
    /// cache.
    pub fn remove(&mut self, id: AccountId, forget_cache: bool) -> Result<(), String> {
        self.stop(id);
        secrets::forget(id)?;
        if forget_cache {
            let store = mailalm_store::Store::open(&self.base, id).map_err(|e| format!("The account's cache could not be opened: {e}"))?;
            store.delete_all().map_err(|e| format!("The account's cache could not be removed: {e}"))?;
        }
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        for (_, w) in self.workers.drain() {
            let _ = w.commands.send(Command::Stop);
        }
    }
}

/// Signs in once with the given settings, to check them before an account is added. Blocking:
/// run it on a thread. The error is shown to the user as it is.
pub fn test_sign_in(account: &Account, password: &str) -> Result<(), String> {
    let server = account.imap.as_ref().ok_or("The account has no IMAP server.")?;
    let session = mailalm_net::ImapSession::connect(server, account.login(), password).map_err(|e| worker::describe(&e))?;
    session.logout();
    Ok(())
}
