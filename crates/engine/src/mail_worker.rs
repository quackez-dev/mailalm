//! What the rules, the snoozed messages and follow-ups, the search and the unsubscribing need of an
//! account's worker, whatever its kind (IMAP, Outlook, Gmail): `rules`, `later`, `find` and
//! `unsubscribe` are written once against it.

use std::sync::mpsc::Receiver;

use mailalm_model::rules::{Host, Rule};
use mailalm_model::{AccountId, Change, Folder, FolderRole, Query, Summary};
use mailalm_store::Store;

use crate::{Command, Event};

/// The most messages one server search gives (per account).
pub(crate) const MAX_SERVER_HITS: usize = 200;
/// The most folders searched one by one on the server.
pub(crate) const MAX_SERVER_FOLDERS: usize = 30;

pub(crate) trait MailWorker {
    fn who(&self) -> AccountId;
    fn cache(&self) -> &Store;
    fn tell(&self, e: Event);
    /// The account's name and address.
    fn me(&self) -> (String, String);
    fn mail_folders(&self) -> Vec<Folder>;
    /// A change made by MailALM (a rule, a snoozed message): in the cache now, sent to the server
    /// now if it is reached, else with the other waiting changes.
    fn change(&mut self, change: Change);
    /// Syncs a folder now (to see messages moved into it).
    fn refresh(&mut self, path: &str, commands: &Receiver<Command>);
    /// Whether the server is reached now (connecting if needed).
    fn reach(&mut self) -> bool;
    /// Who keeps rules on the account's server; `None`: none (IMAP).
    fn rule_host(&self) -> Option<Host>;
    /// The server's rules, read now. The errors are as the user reads them.
    fn server_rules(&mut self) -> Result<Vec<Rule>, String>;
    /// Makes (no id) or changes a rule on the server: the rule as the server keeps it.
    fn save_server_rule(&mut self, rule: &Rule) -> Result<Rule, String>;
    fn delete_server_rule(&mut self, id: &str) -> Result<(), String>;
    /// Makes `MailALM/Snoozed` on the server and reads the folders again: that folder.
    fn make_snooze_folder(&mut self) -> Result<Folder, String>;
    /// The server's search in `folders` (`whole`: the whole account was asked, which a server
    /// may search at once): the messages found, numbered as the cache numbers them, newest first.
    fn server_search(&mut self, query: &Query, folders: &[Folder], whole: bool) -> Result<Vec<(String, Summary)>, String>;
}

/// The folder of a role, among the worker's.
pub(crate) fn folder_with_role<W: MailWorker + ?Sized>(w: &W, role: FolderRole) -> Option<Folder> {
    w.mail_folders().into_iter().find(|f| f.role == role)
}

#[cfg(test)]
mod tests;
