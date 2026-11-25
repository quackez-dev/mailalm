//! The numbers of a folder's messages, for the accounts whose server names messages by long ids
//! (Outlook through Graph, Gmail through its API).
//!
//! The interface and the cache name messages by small numbers; such a server by its own ids. Each
//! folder keeps the pairs (`ids.json`), and gives the next number from `FolderState::uidnext`.

use std::collections::HashMap;

use mailalm_store::Store;

/// A folder's numbers and ids, both ways.
pub(crate) struct Ids {
    pub(crate) by_uid: HashMap<u32, String>,
    pub(crate) by_id: HashMap<String, u32>,
    pub(crate) next: u32,
}

impl Ids {
    pub(crate) fn load(store: &Store, path: &str, next: u32) -> Self {
        let pairs = store.ids(path);
        let next = pairs.iter().map(|(u, _)| u.saturating_add(1)).max().unwrap_or(1).max(next).max(1);
        let by_id = pairs.iter().map(|(u, i)| (i.clone(), *u)).collect();
        Self { by_uid: pairs.into_iter().collect(), by_id, next }
    }

    /// The message's number, given now if it has none.
    pub(crate) fn number(&mut self, id: &str) -> u32 {
        if let Some(u) = self.by_id.get(id) {
            return *u;
        }
        let u = self.next;
        self.next = self.next.saturating_add(1);
        self.by_uid.insert(u, id.to_owned());
        self.by_id.insert(id.to_owned(), u);
        u
    }

    pub(crate) fn forget(&mut self, uid: u32) {
        if let Some(id) = self.by_uid.remove(&uid) {
            self.by_id.remove(&id);
        }
    }

    pub(crate) fn pairs(&self) -> Vec<(u32, String)> {
        let mut v: Vec<(u32, String)> = self.by_uid.iter().map(|(u, i)| (*u, i.clone())).collect();
        v.sort_unstable_by_key(|p| p.0);
        v
    }
}
