//! What an Outlook account's worker gives the rules, the snoozed messages, the search and the
//! unsubscribing (`MailWorker`): the Inbox's rules through Graph's `messageRules`, the folder
//! `MailALM/Snoozed` made with Graph, and Graph's `$search` (the whole mailbox at once, else
//! folder by folder). A message the server found is numbered in its folder as the sync numbers
//! it, so it opens like the others.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;

use mailalm_model::later::{SNOOZE_LEAF, SNOOZE_PARENT, snooze_folder};
use mailalm_model::rules::{Host, Rule};
use mailalm_model::{AccountId, Change, Folder, Query, Summary};
use mailalm_net::NetError;
use mailalm_net::graph::Found;
use mailalm_store::Store;

use super::{Outlook, describe};
use crate::ids::Ids;
use crate::mail_worker::{MAX_SERVER_FOLDERS, MAX_SERVER_HITS, MailWorker};
use crate::{Command, Event};

impl MailWorker for Outlook {
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
        (self.name.clone(), self.address.clone())
    }

    fn mail_folders(&self) -> Vec<Folder> {
        self.folders.clone()
    }

    fn change(&mut self, change: Change) {
        self.apply_local(change);
        if self.online() {
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
        Some(Host::Outlook)
    }

    fn server_rules(&mut self) -> Result<Vec<Rule>, String> {
        let folders = self.folders.clone();
        self.call(|c| c.rules(&folders)).map_err(|e| describe(&e))
    }

    fn save_server_rule(&mut self, rule: &Rule) -> Result<Rule, String> {
        let folders = self.folders.clone();
        let saved = if rule.id.is_empty() { self.call(|c| c.create_rule(rule, &folders)) } else { self.call(|c| c.update_rule(rule, &folders)) };
        saved.map_err(|e| describe(&e))
    }

    fn delete_server_rule(&mut self, id: &str) -> Result<(), String> {
        self.call(|c| c.delete_rule(id)).map_err(|e| describe(&e))
    }

    fn make_snooze_folder(&mut self) -> Result<Folder, String> {
        let parent = match self.folders.iter().find(|f| f.path.eq_ignore_ascii_case(SNOOZE_PARENT)).map(|f| f.wire.clone()) {
            Some(id) => id,
            None => match self.call(|c| c.create_folder(None, SNOOZE_PARENT)) {
                Ok(id) => id,
                // Made meanwhile: the list says where.
                Err(NetError::Server(_)) => {
                    if !self.read_folder_list() {
                        return Err("the folder list could not be read again".to_owned());
                    }
                    self.folders
                        .iter()
                        .find(|f| f.path.eq_ignore_ascii_case(SNOOZE_PARENT))
                        .map(|f| f.wire.clone())
                        .ok_or_else(|| format!("Outlook refused to make the folder {SNOOZE_PARENT}"))?
                }
                Err(e) => return Err(describe(&e)),
            },
        };
        match self.call(|c| c.create_folder(Some(&parent), SNOOZE_LEAF)) {
            Ok(_) | Err(NetError::Server(_)) => {}
            Err(e) => return Err(describe(&e)),
        }
        if !self.read_folder_list() {
            return Err("the folder list could not be read again".to_owned());
        }
        snooze_folder(&self.folders).cloned().ok_or_else(|| format!("Outlook didn't make {SNOOZE_PARENT}/{SNOOZE_LEAF}"))
    }

    fn server_search(&mut self, query: &Query, folders: &[Folder], whole: bool) -> Result<Vec<(String, Summary)>, String> {
        let mut found: Vec<Found> = Vec::new();
        if whole {
            found = self.call(|c| c.search(query, None, MAX_SERVER_HITS)).map_err(|e| describe(&e))?;
        } else {
            for f in folders.iter().take(MAX_SERVER_FOLDERS) {
                let wire = f.wire.clone();
                found.extend(self.call(|c| c.search(query, Some(&wire), MAX_SERVER_HITS)).map_err(|e| describe(&e))?);
                if found.len() >= MAX_SERVER_HITS {
                    break;
                }
            }
        }
        Ok(self.number_found(found))
    }
}

impl Outlook {
    /// The messages Graph found, numbered in their folders as the sync numbers them (the new
    /// numbers kept), the cache's summary taken when it has one. A message in a folder MailALM
    /// doesn't list is left out.
    fn number_found(&self, found: Vec<Found>) -> Vec<(String, Summary)> {
        let mut numbers: HashMap<String, Ids> = HashMap::new();
        let mut cached: HashMap<String, HashMap<u32, Summary>> = HashMap::new();
        let mut out = Vec::new();
        for f in found {
            let Some(folder) = self.folders.iter().find(|x| x.wire == f.folder_id) else { continue };
            let path = folder.path.clone();
            let store = &self.store;
            let ids = numbers.entry(path.clone()).or_insert_with(|| {
                let next = store.folder_state(&path).map_or(1, |s| s.uidnext);
                Ids::load(store, &path, next)
            });
            let uid = ids.number(&f.message.id);
            let known = cached.entry(path.clone()).or_insert_with(|| store.summaries(&path).into_iter().map(|s| (s.uid, s)).collect());
            let summary = known.get(&uid).cloned().unwrap_or_else(|| Summary { uid, ..f.message.summary });
            out.push((path, summary));
        }
        for (path, ids) in numbers {
            let _ = self.store.save_ids(&path, &ids.pairs());
        }
        out
    }
}
