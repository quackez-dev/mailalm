//! What an IMAP account's worker gives the rules, the snoozed messages, the search and the
//! unsubscribing (`MailWorker`). Its server keeps no rules MailALM can reach (ManageSieve is not
//! used): only MailALM's own run. `MailALM/Snoozed` is made with `CREATE`, under the Inbox when
//! the server keeps every folder there; the server's search is `UID SEARCH`, folder by folder.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use mailalm_model::later::{SNOOZE_LEAF, SNOOZE_PARENT, snooze_folder};
use mailalm_model::rules::{Host, Rule};
use mailalm_model::{AccountId, Change, Flags, Folder, FolderRole, Query, Summary};
use mailalm_net::NetError;
use mailalm_store::Store;

use super::Worker;
use crate::mail_worker::{MAX_SERVER_FOLDERS, MAX_SERVER_HITS, MailWorker};
use crate::{Command, Event};

/// The name `MailALM/Snoozed` takes on this server: with its delimiter, under the Inbox when
/// every other folder is there (`INBOX.MailALM.Snoozed`).
pub(super) fn snooze_name(folders: &[Folder]) -> String {
    let d = folders.iter().find(|f| f.role == FolderRole::Inbox).and_then(|f| f.delimiter).or_else(|| folders.iter().find_map(|f| f.delimiter)).unwrap_or('/');
    let others: Vec<&Folder> = folders.iter().filter(|f| f.role != FolderRole::Inbox).collect();
    let prefix = format!("INBOX{d}");
    let under_inbox = !others.is_empty() && others.iter().all(|f| f.wire.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)));
    let name = format!("{SNOOZE_PARENT}{d}{SNOOZE_LEAF}");
    if under_inbox { format!("{prefix}{name}") } else { name }
}

impl MailWorker for Worker {
    fn who(&self) -> AccountId {
        self.id
    }

    fn cache(&self) -> &Store {
        &self.store
    }

    fn tell(&self, e: Event) {
        self.emit(e);
    }

    fn me(&self) -> (String, String) {
        (self.account.name.clone(), self.account.address.clone())
    }

    fn mail_folders(&self) -> Vec<Folder> {
        self.folders.clone()
    }

    fn change(&mut self, change: Change) {
        self.apply_local(change);
        if self.session.is_some() {
            self.flush_pending();
        }
    }

    fn refresh(&mut self, path: &str, commands: &Receiver<Command>) {
        if let Some(f) = self.folder(path) {
            self.sync_folder(&f, commands);
            self.activity(None);
        }
    }

    fn reach(&mut self) -> bool {
        self.connect().is_some()
    }

    fn rule_host(&self) -> Option<Host> {
        None
    }

    fn server_rules(&mut self) -> Result<Vec<Rule>, String> {
        Ok(Vec::new())
    }

    fn save_server_rule(&mut self, _rule: &Rule) -> Result<Rule, String> {
        Err("this account's server keeps no rules MailALM can reach".to_owned())
    }

    fn delete_server_rule(&mut self, _id: &str) -> Result<(), String> {
        Err("this account's server keeps no rules MailALM can reach".to_owned())
    }

    fn make_snooze_folder(&mut self) -> Result<Folder, String> {
        let name = snooze_name(&self.folders);
        let made = match self.connect() {
            Some(s) => s.create(&name),
            None => return Err("offline".to_owned()),
        };
        match made {
            Ok(()) => {}
            // Already there (the server says so with a NO): the list says where.
            Err(NetError::Server(_)) => {}
            Err(e) => return Err(self.failed(&e)),
        }
        if !self.read_folder_list() {
            return Err("the folder list could not be read again".to_owned());
        }
        snooze_folder(&self.folders).cloned().ok_or_else(|| format!("the server didn't make {name}"))
    }

    fn server_search(&mut self, query: &Query, folders: &[Folder], _whole: bool) -> Result<Vec<(String, Summary)>, String> {
        let mut out: Vec<(String, Summary)> = Vec::new();
        for f in folders.iter().take(MAX_SERVER_FOLDERS) {
            let uids = self.in_folder(&f.path, |s| s.search(query))?;
            if uids.is_empty() {
                continue;
            }
            let mut uids = uids;
            // The newest first: a search's answer comes in order.
            uids.reverse();
            uids.truncate(MAX_SERVER_HITS);
            let cached: HashMap<u32, Summary> = self.store.summaries(&f.path).into_iter().map(|s| (s.uid, s)).collect();
            let missing: Vec<u32> = uids.iter().copied().filter(|u| !cached.contains_key(u)).collect();
            let mut fetched: HashMap<u32, Summary> = HashMap::new();
            if !missing.is_empty() {
                for m in self.in_folder(&f.path, |s| s.fetch_headers(&missing))? {
                    let flags = Flags::from_imap(m.flags.iter().map(String::as_str));
                    fetched.insert(m.uid, mailalm_mime::summary_from_header(m.uid, &m.header, flags, m.size, m.internal_date));
                }
            }
            for u in uids {
                if let Some(s) = cached.get(&u).or_else(|| fetched.get(&u)) {
                    out.push((f.path.clone(), s.clone()));
                }
            }
            if out.len() >= MAX_SERVER_HITS {
                break;
            }
        }
        // Newest first, as the cache's search gives them.
        out.sort_by(|a, b| b.1.date.cmp(&a.1.date).then(b.1.uid.cmp(&a.1.uid)));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str, d: char, role: FolderRole) -> Folder {
        Folder { path: path.into(), wire: path.into(), delimiter: Some(d), role, total: 0, unread: 0 }
    }

    #[test]
    fn the_snooze_folder_follows_the_servers_ways() {
        let flat = [folder("INBOX", '/', FolderRole::Inbox), folder("Sent", '/', FolderRole::Sent)];
        assert_eq!(snooze_name(&flat), "MailALM/Snoozed");
        let under = [folder("INBOX", '.', FolderRole::Inbox), folder("INBOX.Sent", '.', FolderRole::Sent), folder("INBOX.Trash", '.', FolderRole::Trash)];
        assert_eq!(snooze_name(&under), "INBOX.MailALM.Snoozed");
        assert_eq!(snooze_name(&[folder("INBOX", '.', FolderRole::Inbox)]), "MailALM.Snoozed");
        assert_eq!(snooze_name(&[]), "MailALM/Snoozed");
    }
}
