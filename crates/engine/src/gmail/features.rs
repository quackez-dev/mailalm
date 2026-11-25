//! What a Gmail account's worker gives the rules, the snoozed messages, the search and the
//! unsubscribing (`MailWorker`): Gmail's filters as rules (the sender filters stay with the
//! blocked and allowed senders), the label `MailALM/Snoozed`, and Gmail's search (`q`), whose
//! messages are read for their header fields, at most [`MAX_DETAILS_READ`] of them.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;

use mailalm_model::later::{SNOOZE_LEAF, SNOOZE_PARENT, snooze_folder};
use mailalm_model::rules::{Host, Rule};
use mailalm_model::{AccountId, Change, Folder, FolderRole, Query, Summary};
use mailalm_net::NetError;
use mailalm_store::Store;

use super::{Gmail, describe};
use crate::ids::Ids;
use crate::mail_worker::{MAX_SERVER_FOLDERS, MAX_SERVER_HITS, MailWorker};
use crate::{Command, Event};

/// The most messages found by the server whose header fields are read (one request each).
const MAX_DETAILS_READ: usize = 100;

impl MailWorker for Gmail {
    fn who(&self) -> AccountId {
        self.id
    }

    fn cache(&self) -> &Store {
        &self.store
    }

    /// A Gmail draft (`drafts.create`, or `drafts.update` of the one saved before). The draft is
    /// named by Gmail's draft id.
    fn store_draft(&mut self, raw: &[u8], _message_id: &str, replace: Option<&str>) -> Result<String, String> {
        self.call(|c| c.save_draft(raw, replace)).map_err(|e| describe(&e))
    }

    fn drop_draft(&mut self, draft: &str) -> Result<(), String> {
        self.call(|c| c.delete_draft(draft)).map_err(|e| describe(&e))
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
            let history = self.history();
            self.sync_folder(&f, history, commands);
            self.activity(None);
        }
    }

    fn reach(&mut self) -> bool {
        self.connect().is_some()
    }

    fn rule_host(&self) -> Option<Host> {
        Some(Host::Gmail)
    }

    fn server_rules(&mut self) -> Result<Vec<Rule>, String> {
        let folders = self.folders.clone();
        self.call(|c| c.rules(&folders)).map_err(|e| describe(&e))
    }

    /// Gmail can't change a filter: the new one is made, then the old one deleted.
    fn save_server_rule(&mut self, rule: &Rule) -> Result<Rule, String> {
        let folders = self.folders.clone();
        let made = self.call(|c| c.create_rule(rule, &folders)).map_err(|e| describe(&e))?;
        if !rule.id.is_empty()
            && let Err(e) = self.call(|c| c.delete_rule(&rule.id))
        {
            self.error(format!("The filter was changed, but its old version could not be removed: {}", describe(&e)));
        }
        Ok(made)
    }

    fn delete_server_rule(&mut self, id: &str) -> Result<(), String> {
        self.call(|c| c.delete_rule(id)).map_err(|e| describe(&e))
    }

    fn make_snooze_folder(&mut self) -> Result<Folder, String> {
        // The parent first, as Gmail's own interface makes nested labels; one already there is
        // Gmail's refusal, which is fine.
        let full = format!("{SNOOZE_PARENT}/{SNOOZE_LEAF}");
        for name in [SNOOZE_PARENT, full.as_str()] {
            match self.call(|c| c.create_label(name)) {
                Ok(_) | Err(NetError::Server(_)) => {}
                Err(e) => return Err(describe(&e)),
            }
        }
        if !self.read_folder_list() {
            return Err("the labels could not be read again".to_owned());
        }
        snooze_folder(&self.folders).cloned().ok_or_else(|| format!("Gmail didn't make the label {full}"))
    }

    fn server_search(&mut self, query: &Query, folders: &[Folder], whole: bool) -> Result<Vec<(String, Summary)>, String> {
        // (label searched, the folder its messages are shown in)
        let places: Vec<(Option<String>, Option<String>)> = if whole {
            vec![(None, None)]
        } else {
            folders.iter().take(MAX_SERVER_FOLDERS).map(|f| ((!f.wire.is_empty()).then(|| f.wire.clone()), Some(f.path.clone()))).collect()
        };
        let mut wanted: Vec<(String, Option<String>)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (label, path) in places {
            let ids = self.call(|c| c.search_ids(query, label.as_deref(), MAX_SERVER_HITS)).map_err(|e| describe(&e))?;
            wanted.extend(ids.into_iter().filter(|id| seen.insert(id.clone())).map(|id| (id, path.clone())));
            if wanted.len() >= MAX_DETAILS_READ {
                break;
            }
        }
        wanted.truncate(MAX_DETAILS_READ);
        let mut found: Vec<(String, String, Summary)> = Vec::new();
        for (id, path) in wanted {
            let (labels, summary) = match self.call(|c| c.metadata(&id)) {
                Ok(Some(m)) => {
                    self.remember(&id, &m.summary);
                    (m.labels, m.summary)
                }
                Ok(None) => continue,
                Err(e) => return Err(describe(&e)),
            };
            let Some(path) = path.or_else(|| self.folder_of(&labels)) else { continue };
            found.push((path, id, summary));
        }
        Ok(self.number_found(found))
    }
}

impl Gmail {
    /// The folder a message found everywhere is shown in: the Inbox, else one of its labels,
    /// else Spam or Trash, else All Mail.
    fn folder_of(&self, labels: &[String]) -> Option<String> {
        let has = |l: &str| labels.iter().any(|x| x == l);
        let by_wire = |w: &str| self.folders.iter().find(|f| f.wire == w).map(|f| f.path.clone());
        if has("INBOX") {
            return self.folders.iter().find(|f| f.role == FolderRole::Inbox).map(|f| f.path.clone());
        }
        labels
            .iter()
            .filter(|l| l.starts_with("Label_"))
            .find_map(|l| by_wire(l.as_str()))
            .or_else(|| ["SPAM", "TRASH"].into_iter().filter(|l| has(*l)).find_map(by_wire))
            .or_else(|| by_wire(""))
    }

    /// The messages found, numbered in their folders as the sync numbers them (the new numbers
    /// kept until the folder's next sync), the cache's summary taken when it has one.
    fn number_found(&self, found: Vec<(String, String, Summary)>) -> Vec<(String, Summary)> {
        let mut numbers: HashMap<String, Ids> = HashMap::new();
        let mut cached: HashMap<String, HashMap<u32, Summary>> = HashMap::new();
        let mut out = Vec::new();
        let store = &self.store;
        for (path, id, summary) in found {
            let ids = numbers.entry(path.clone()).or_insert_with(|| {
                let next = store.folder_state(&path).map_or(1, |s| s.uidnext);
                Ids::load(store, &path, next)
            });
            let uid = ids.number(&id);
            let known = cached.entry(path.clone()).or_insert_with(|| store.summaries(&path).into_iter().map(|s| (s.uid, s)).collect());
            let summary = known.get(&uid).cloned().unwrap_or(Summary { uid, ..summary });
            out.push((path, summary));
        }
        for (path, ids) in numbers {
            let _ = store.save_ids(&path, &ids.pairs());
        }
        out
    }
}
