//! The engine between the interface and the servers.
//!
//! Each account has its own worker thread (IMAP, Outlook through Microsoft Graph, Gmail through its API), its own cache ([`mailalm_store::Store`]) and its own
//! connection: one slow or failing account never holds up another (PLAN.md, section 4.3). The
//! interface sends [`Command`]s and reads [`Event`]s; it never waits on the network.
//!
//! A worker shows the cache first, then syncs with the server. Changes (read, flag, move, delete)
//! are made in the cache at once and kept in order until the server has them, so they also work
//! offline (C7).
//!
//! Calendars, tasks and contacts (phase 6) go the same way ([`PimCommand`]): the Outlook and Gmail
//! workers keep the provider's in the cache and sync them at start, every 15 minutes and on
//! demand; an IMAP account has a calendar and a task list kept on this computer, and sends and
//! answers invitations by mail (iMIP). What the interface works out from every account's cache
//! (reminders, clashes, addresses to suggest) is in [`agenda`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use mailalm_mime::{AttachmentData, Outgoing, OutgoingAttachment, Parsed};
use mailalm_model::pim::ical::{Invitation, Method};
use mailalm_model::pim::{Calendar, Contact, Event as CalendarEvent, Response, Task, TaskList, ZoneRules};
use mailalm_model::{Account, AccountId, Change, Folder, Provider, Summary};
use mailalm_safety::Report;

pub mod agenda;
mod gmail;
mod ids;
mod itip;
mod local;
mod outlook;
mod pim_sync;
pub mod secrets;
mod signin;
mod worker;

pub use local::is_calendar_part;
pub use signin::{OAuthAccount, google_available, google_sign_in, microsoft_available, microsoft_sign_in};

/// The time zone rules the workers read and write times with (the app's zone database).
pub type Zones = Arc<dyn ZoneRules + Send + Sync>;

/// What the interface asks about an account's calendars, tasks and contacts (phase 6). Changes
/// are made in the cache at once (the interface hears of them by [`Event::Calendars`] and
/// [`Event::Tasks`]) and sent to the provider in order, now or once it can be reached.
#[derive(Debug)]
pub enum PimCommand {
    /// Syncs the calendars, tasks and contacts now (also done at start and every 15 minutes).
    Sync,
    /// The calendar page shows `[from, to)` (UTC seconds): its events are read if the cache
    /// doesn't keep them.
    Range { from: i64, to: i64 },
    /// Makes an event (one without a remote id) or changes one. `notify`: its guests hear of it
    /// (the providers always tell them; an IMAP account sends the invitation by mail).
    SaveEvent { event: Box<CalendarEvent>, notify: bool },
    /// Deletes an event (or one occurrence); `notify`: its guests hear of it.
    DeleteEvent { event: Box<CalendarEvent>, notify: bool },
    /// Answers an invitation: `event` as the calendar has it, or as the invitation says (found
    /// in the calendar by its UID). `send`: the organizer hears of it, with `comment`;
    /// `proposal`: another time proposed (start, end).
    Respond { event: Box<CalendarEvent>, answer: Response, send: bool, comment: String, proposal: Option<(i64, i64)> },
    /// Takes in, after the user's confirmation, an update or a cancellation of a meeting, or a
    /// guest's answer, received by mail (an IMAP account's calendar; the providers take them in
    /// by themselves, and a cancellation removes the event there).
    ApplyInvitation { method: Method, events: Vec<CalendarEvent> },
    /// Makes a task (one without a remote id) or changes one (done or not, too).
    SaveTask(Box<Task>),
    DeleteTask(Box<Task>),
    /// Decodes the calendar part (attachment `index`) of a message the reading pane shows: the
    /// answer is [`Event::Invitation`]. Only that part is decoded, and read as an invitation.
    Invitation { folder: String, uid: u32, index: usize },
}

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
    /// The user asked to review attachment `index` of a message (PLAN.md, section 10.1): it is
    /// decoded from the cached message and looked at ([`mailalm_safety::assess`]); the answer is
    /// [`Event::Attachment`]. Never sent by anything but the user's click.
    Attachment { folder: String, uid: u32, index: usize },
    /// Signs in again (the password was changed).
    Reconnect,
    /// The account's blocked and allowed senders, kept on the server (Gmail filters). Other
    /// accounts answer with an error.
    Senders(SenderCommand),
    /// Calendars, tasks, contacts, invitations.
    Pim(PimCommand),
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
    /// An attachment the user asked to review, with what MailALM found about it; or why it could
    /// not be read.
    Attachment { account: AccountId, folder: String, uid: u32, index: usize, result: Result<Box<Reviewed>, String> },
    /// The account's calendars and their events, as the cache has them (after a sync or a change).
    Calendars { account: AccountId, calendars: Vec<Calendar>, events: Vec<CalendarEvent> },
    /// The account's task lists and their tasks, as the cache has them.
    Tasks { account: AccountId, lists: Vec<TaskList>, tasks: Vec<Task> },
    /// The account's contacts, as the cache has them.
    Contacts { account: AccountId, contacts: Vec<Contact> },
    /// The calendar part of a message, read as an invitation ([`PimCommand::Invitation`]); or why
    /// it could not be read.
    Invitation { account: AccountId, folder: String, uid: u32, index: usize, result: Result<Box<Invitation>, String> },
    /// Something done that the user should hear of ("The answer was sent to Zoe").
    Notice { account: AccountId, text: String },
}

/// An attachment decoded for its review, and what MailALM found about it.
#[derive(Debug)]
pub struct Reviewed {
    pub data: AttachmentData,
    pub report: Report,
}

/// What MailALM lets a message weigh before sending it, by provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeLimit {
    /// The attachments together (what the providers announce: 25 MB for Gmail and most IMAP
    /// providers).
    Attachments(usize),
    /// The whole written message (Outlook: Microsoft Graph's `sendMail` takes about 4 MB per
    /// request, and the message goes in base64, a third larger).
    Message(usize),
}

/// The attachments' limit for Gmail and IMAP accounts: 25 MB.
pub const ATTACHMENTS_LIMIT: usize = 25 * 1024 * 1024;
/// The largest written message an Outlook account sends: 3 MB, which Graph receives as 4 MB of
/// base64. Larger messages need Graph's upload sessions, not done yet: they are refused.
pub const OUTLOOK_MESSAGE_LIMIT: usize = 3 * 1000 * 1000;

impl SizeLimit {
    pub fn of(provider: Provider) -> Self {
        match provider {
            Provider::Outlook => SizeLimit::Message(OUTLOOK_MESSAGE_LIMIT),
            Provider::Gmail | Provider::Imap(_) => SizeLimit::Attachments(ATTACHMENTS_LIMIT),
        }
    }

    /// What counts against the limit for a message with this text and these attachments
    /// (estimated for a whole message), and the limit.
    pub fn measure(self, text: &str, attachments: &[OutgoingAttachment]) -> (usize, usize) {
        match self {
            SizeLimit::Attachments(limit) => (mailalm_mime::attachments_size(attachments), limit),
            SizeLimit::Message(limit) => (mailalm_mime::estimated_size(text, attachments), limit),
        }
    }
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
            | Event::SenderRules { account, .. }
            | Event::Attachment { account, .. }
            | Event::Calendars { account, .. }
            | Event::Tasks { account, .. }
            | Event::Contacts { account, .. }
            | Event::Invitation { account, .. }
            | Event::Notice { account, .. } => *account,
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
    zones: Zones,
    workers: HashMap<AccountId, WorkerHandle>,
}

/// What a worker is started with besides its account.
pub(crate) struct Start {
    pub(crate) base: PathBuf,
    pub(crate) commands: Receiver<Command>,
    pub(crate) events: Sender<Event>,
    pub(crate) wake: Wake,
    pub(crate) zones: Zones,
}

impl Engine {
    /// `base` is the cache's folder (one sub-folder per account); `zones`, the time zone rules
    /// events are read and written with.
    pub fn new(base: PathBuf, wake: Wake, zones: Zones) -> Self {
        let (events_tx, events) = mpsc::channel();
        Self { base, events_tx, events, wake, zones, workers: HashMap::new() }
    }

    /// Starts an account's worker: IMAP, Outlook (Microsoft Graph) or Gmail (its API).
    pub fn start(&mut self, account: Account) {
        self.stop(account.id);
        let (tx, rx) = mpsc::channel();
        let start = Start { base: self.base.clone(), commands: rx, events: self.events_tx.clone(), wake: Arc::clone(&self.wake), zones: Arc::clone(&self.zones) };
        let id = account.id;
        let provider = account.provider;
        let thread = std::thread::Builder::new()
            .name(format!("mailalm-account-{}", id.0))
            .spawn(move || match provider {
                Provider::Outlook => outlook::run(account, start),
                Provider::Gmail => gmail::run(account, start),
                _ => worker::run(account, start),
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
