//! What the interface knows of each account, kept up to date from the engine's events.

use std::collections::HashMap;

use mailalm_engine::{Event, Link, Reviewed, SenderRule};
use mailalm_mime::{HtmlBody, Parsed};
use mailalm_model::later::{FollowUp, Snoozed};
use mailalm_model::rules::Rule;
use mailalm_model::{AccountId, Folder, FolderRole, Summary};

/// One account, as the interface shows it.
#[derive(Debug)]
pub struct AccountState {
    pub folders: Vec<Folder>,
    /// Each folder's summaries, newest first.
    pub summaries: HashMap<String, Vec<Summary>>,
    pub link: Link,
    pub link_detail: String,
    pub activity: Option<String>,
    /// The senders blocked or allowed on the server (Gmail filters), once listed.
    pub sender_rules: Option<Vec<SenderRule>>,
    /// The account's rules (the server's, then MailALM's own), once listed (C5).
    pub rules: Option<Vec<Rule>>,
    /// The snoozed messages and follow-ups, once listed (C2).
    pub snoozed: Vec<Snoozed>,
    pub follow_ups: Vec<FollowUp>,
}

impl Default for AccountState {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            summaries: HashMap::new(),
            link: Link::Idle,
            link_detail: String::new(),
            activity: None,
            sender_rules: None,
            rules: None,
            snoozed: Vec::new(),
            follow_ups: Vec::new(),
        }
    }
}

impl AccountState {
    pub fn folder_with_role(&self, role: FolderRole) -> Option<&Folder> {
        self.folders.iter().find(|f| f.role == role)
    }

    pub fn inbox(&self) -> Option<&Folder> {
        self.folder_with_role(FolderRole::Inbox)
    }

    pub fn folder(&self, path: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.path == path)
    }

    pub fn summary(&self, folder: &str, uid: u32) -> Option<&Summary> {
        self.summaries.get(folder).and_then(|v| v.iter().find(|s| s.uid == uid))
    }

    pub fn unread_inbox(&self) -> u32 {
        self.inbox().map_or(0, |f| f.unread)
    }
}

/// The message open in the reading pane.
#[derive(Debug)]
pub struct Reading {
    pub key: MessageKey,
    pub parsed: Option<Box<Parsed>>,
    /// Its HTML version, until the reading pane takes it.
    pub html: Option<Box<HtmlBody>>,
    pub error: Option<String>,
}

/// A message: its account, its folder and its UID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageKey {
    pub account: AccountId,
    pub folder: String,
    pub uid: u32,
}

/// Every account's state, from the events.
#[derive(Debug, Default)]
pub struct Mailboxes {
    pub accounts: HashMap<AccountId, AccountState>,
    /// Goes up at every change of folders or summaries: the message list is rebuilt then.
    pub generation: u64,
}

/// What an event means for the rest of the interface, beyond the state kept here.
pub enum Outcome {
    None,
    Message { key: MessageKey, parsed: Box<Parsed>, html: Option<Box<HtmlBody>> },
    MessageUnavailable { key: MessageKey, reason: String },
    Notice(String),
    Error(String),
    SendFailed { account: AccountId, text: String, message: Box<mailalm_mime::Outgoing> },
    SearchResults { account: AccountId, id: u64, hits: Vec<(String, Summary)> },
    /// An attachment the user asked to review.
    Attachment { key: MessageKey, index: usize, result: Result<Box<Reviewed>, String> },
    /// A compose window's draft saved on the server, or not.
    Draft { compose: u64, result: Result<String, String> },
    /// The cached messages a rule being edited would touch.
    RulePreview { account: AccountId, id: u64, hits: Vec<(String, Summary)> },
    /// No answer came to a followed message by its date.
    FollowUpDue { account: AccountId, follow_up: Box<FollowUp> },
    /// What a server's search found, or why it couldn't search.
    ServerResults { account: AccountId, id: u64, result: Result<Vec<(String, Summary)>, String> },
}

impl Mailboxes {
    pub fn account(&self, id: AccountId) -> Option<&AccountState> {
        self.accounts.get(&id)
    }

    pub fn forget(&mut self, id: AccountId) {
        self.accounts.remove(&id);
        self.generation += 1;
    }

    pub fn apply(&mut self, e: Event) -> Outcome {
        let account = e.account();
        let state = self.accounts.entry(account).or_default();
        match e {
            Event::Folders { folders, .. } => {
                state.summaries.retain(|path, _| folders.iter().any(|f| &f.path == path));
                state.folders = folders;
                self.generation += 1;
                Outcome::None
            }
            Event::Summaries { folder, summaries, .. } => {
                state.summaries.insert(folder, summaries);
                self.generation += 1;
                Outcome::None
            }
            Event::Message { folder, uid, parsed, html, .. } => Outcome::Message { key: MessageKey { account, folder, uid }, parsed, html },
            Event::Draft { compose, result, .. } => Outcome::Draft { compose, result },
            Event::MessageUnavailable { folder, uid, reason, .. } => Outcome::MessageUnavailable { key: MessageKey { account, folder, uid }, reason },
            Event::Link { link, detail, .. } => {
                state.link = link;
                state.link_detail = detail;
                Outcome::None
            }
            Event::Activity { text, .. } => {
                state.activity = text;
                Outcome::None
            }
            Event::Error { text, .. } => Outcome::Error(text),
            Event::Sent { subject, .. } => Outcome::Notice(format!("Sent: {subject}")),
            Event::SendFailed { text, message, .. } => Outcome::SendFailed { account, text, message },
            Event::SearchResults { id, hits, .. } => Outcome::SearchResults { account, id, hits },
            Event::SenderRules { rules, .. } => {
                state.sender_rules = Some(rules);
                Outcome::None
            }
            Event::Attachment { folder, uid, index, result, .. } => Outcome::Attachment { key: MessageKey { account, folder, uid }, index, result },
            Event::Notice { text, .. } => Outcome::Notice(text),
            // Calendars, tasks, contacts and invitations are the agenda's (`agenda::Pim::take`).
            Event::Calendars { .. } | Event::Tasks { .. } | Event::Contacts { .. } | Event::Invitation { .. } => Outcome::None,
            Event::Rules { rules, .. } => {
                state.rules = Some(rules);
                Outcome::None
            }
            Event::Later { snoozed, follow_ups, .. } => {
                state.snoozed = snoozed;
                state.follow_ups = follow_ups;
                Outcome::None
            }
            Event::RulePreview { id, hits, .. } => Outcome::RulePreview { account, id, hits },
            Event::FollowUpDue { follow_up, .. } => Outcome::FollowUpDue { account, follow_up },
            Event::ServerResults { id, result, .. } => Outcome::ServerResults { account, id, result },
        }
    }
}
