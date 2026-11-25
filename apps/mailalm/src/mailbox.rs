//! What the interface knows of each account, kept up to date from the engine's events.

use std::collections::HashMap;

use mailalm_engine::{Event, Link, Reviewed, SenderRule};
use mailalm_mime::Parsed;
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
}

impl Default for AccountState {
    fn default() -> Self {
        Self { folders: Vec::new(), summaries: HashMap::new(), link: Link::Idle, link_detail: String::new(), activity: None, sender_rules: None }
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
    Message { key: MessageKey, parsed: Box<Parsed> },
    MessageUnavailable { key: MessageKey, reason: String },
    Notice(String),
    Error(String),
    SendFailed { account: AccountId, text: String, message: Box<mailalm_mime::Outgoing> },
    SearchResults { account: AccountId, id: u64, hits: Vec<(String, Summary)> },
    /// An attachment the user asked to review.
    Attachment { key: MessageKey, index: usize, result: Result<Box<Reviewed>, String> },
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
            Event::Message { folder, uid, parsed, .. } => Outcome::Message { key: MessageKey { account, folder, uid }, parsed },
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
            // Rules, snoozed messages, follow-ups and server results: their pages come next.
            Event::Rules { .. } | Event::RulePreview { .. } | Event::Later { .. } | Event::FollowUpDue { .. } | Event::ServerResults { .. } => Outcome::None,
        }
    }
}
