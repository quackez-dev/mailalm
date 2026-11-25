//! MailALM's settings: the accounts (without their passwords, which are in the system's keychain)
//! in `%APPDATA%\MailALM\settings.json`; the cache in `%LOCALAPPDATA%\MailALM\cache`.

use std::path::PathBuf;

use mailalm_model::{Account, AccountId};
use serde::{Deserialize, Serialize};

use crate::i18n::Language;
use crate::signatures::Writing;
use crate::theme::ThemeChoice;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// The next account's id (ids are never reused: a cache left behind is never mistaken for a
    /// new account's).
    #[serde(default)]
    pub next_id: u32,
    /// Searches kept as search folders (PLAN.md, section 6).
    #[serde(default)]
    pub saved_searches: Vec<mailalm_model::SavedSearch>,
    /// Each account's signature, templates and quick replies (C4).
    #[serde(default)]
    pub writing: Vec<Writing>,
    /// How long a message sent waits, with its Undo button (C1): 5 to 30 seconds; 0 is the
    /// default (10).
    #[serde(default)]
    pub undo_send_seconds: u32,
    /// New messages are written in plain text rather than rich text.
    #[serde(default)]
    pub plain_text: bool,
    /// Antidote's Anti-Oups! checks each message before it is sent (when Antidote is installed).
    #[serde(default)]
    pub anti_oups: bool,
    /// Senders whose pictures from the internet are loaded without asking (addresses, lower
    /// case).
    #[serde(default)]
    pub picture_senders: Vec<String>,
    /// Dark, light, or as Windows is set.
    #[serde(default)]
    pub theme: ThemeChoice,
    /// The interface's language (English by default).
    #[serde(default)]
    pub language: Language,
    /// MCP access is on: MailALM listens on 127.0.0.1 for `mailalm-mcp` (PLAN.md, section 11). Off
    /// by default.
    #[serde(default)]
    pub mcp_enabled: bool,
    /// The accounts the MCP may not see.
    #[serde(default)]
    pub mcp_hidden: Vec<AccountId>,
}

impl Settings {
    fn dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("MailALM"))
    }

    fn file() -> Option<PathBuf> {
        Self::dir().map(|d| d.join("settings.json"))
    }

    /// The cache's folder.
    pub fn cache_dir() -> PathBuf {
        dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("MailALM").join("cache")
    }

    /// The settings saved, or none. A damaged file is kept aside (`settings.json.damaged`) so
    /// nothing is lost, and MailALM starts without accounts.
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::file() else { return (Self::default(), None) };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => return (Self::default(), Some(format!("The settings could not be read: {e}"))),
        };
        match serde_json::from_slice::<Settings>(&bytes) {
            Ok(mut s) => {
                let max = s.accounts.iter().map(|a| a.id.0 + 1).max().unwrap_or(0);
                s.next_id = s.next_id.max(max).max(1);
                (s, None)
            }
            Err(e) => {
                let mut aside = path.clone().into_os_string();
                aside.push(".damaged");
                let _ = std::fs::rename(&path, PathBuf::from(aside));
                (Self { next_id: 1, ..Self::default() }, Some(format!("The settings file was damaged and was set aside: {e}")))
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::dir().ok_or("The settings folder is unknown.")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("The settings could not be saved: {e}"))?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("The settings could not be saved: {e}"))?;
        mailalm_store::write_atomic(&dir.join("settings.json"), &bytes).map_err(|e| format!("The settings could not be saved: {e}"))
    }

    /// A new account id.
    pub fn take_id(&mut self) -> AccountId {
        let id = AccountId(self.next_id.max(1));
        self.next_id = id.0 + 1;
        id
    }

    pub fn upsert(&mut self, account: Account) {
        match self.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(a) => *a = account,
            None => self.accounts.push(account),
        }
    }

    pub fn remove(&mut self, id: AccountId) {
        self.accounts.retain(|a| a.id != id);
        self.writing.retain(|w| w.account != id);
        self.mcp_hidden.retain(|a| *a != id);
        // A search kept for this account alone has nothing left to search.
        self.saved_searches.retain(|s| s.accounts != mailalm_model::SearchScope::Accounts(vec![id]));
        for s in &mut self.saved_searches {
            if let mailalm_model::SearchScope::Accounts(ids) = &mut s.accounts {
                ids.retain(|a| *a != id);
            }
        }
    }

    /// Whether the MCP may see this account (it exists and isn't hidden).
    pub fn mcp_visible(&self, id: AccountId) -> bool {
        self.accounts.iter().any(|a| a.id == id) && !self.mcp_hidden.contains(&id)
    }

    /// Shows or hides an account from the MCP.
    pub fn set_mcp_visible(&mut self, id: AccountId, visible: bool) {
        self.mcp_hidden.retain(|a| *a != id);
        if !visible {
            self.mcp_hidden.push(id);
            self.mcp_hidden.sort();
        }
    }

    /// The undo delay, in seconds.
    pub fn undo_delay(&self) -> u32 {
        crate::send_queue::undo_delay(self.undo_send_seconds)
    }

    /// An account's signature, templates and quick replies, if it has any.
    pub fn writing(&self, id: AccountId) -> Option<&Writing> {
        self.writing.iter().find(|w| w.account == id)
    }

    /// An account's signature, templates and quick replies, made empty the first time.
    pub fn writing_mut(&mut self, id: AccountId) -> &mut Writing {
        let at = match self.writing.iter().position(|w| w.account == id) {
            Some(at) => at,
            None => {
                self.writing.push(Writing::new(id));
                self.writing.len() - 1
            }
        };
        &mut self.writing[at]
    }

    /// Whether this sender's pictures from the internet load without asking.
    pub fn pictures_approved(&self, address: &str) -> bool {
        let a = address.trim().to_lowercase();
        !a.is_empty() && self.picture_senders.iter().any(|s| *s == a)
    }

    /// Approves (or no longer approves) a sender's pictures.
    pub fn set_pictures_approved(&mut self, address: &str, approved: bool) {
        let a = address.trim().to_lowercase();
        self.picture_senders.retain(|s| *s != a);
        if approved && !a.is_empty() {
            self.picture_senders.push(a);
            self.picture_senders.sort();
        }
    }
}

#[cfg(test)]
mod tests {
    use mailalm_model::{FolderScope, SavedSearch, SearchScope};

    use super::*;

    /// Saved searches, the theme, the language and the MCP choices are kept in the settings file;
    /// a file written before them reads with their defaults.
    #[test]
    fn saved_searches_and_new_choices_are_kept() {
        let mut s = Settings { next_id: 3, ..Settings::default() };
        let saved = SavedSearch::new("Invoices", "from:billing has:attachment", SearchScope::Accounts(vec![AccountId(2)]), FolderScope::WithSubfolders("INBOX".into()), true);
        s.saved_searches.extend(saved);
        s.theme = ThemeChoice::Light;
        s.language = Language::French;
        s.mcp_enabled = true;
        s.set_mcp_visible(AccountId(2), false);
        let json = serde_json::to_vec(&s).expect("written");
        let back: Settings = serde_json::from_slice(&json).expect("read");
        assert_eq!(back.saved_searches, s.saved_searches);
        assert_eq!((back.theme, back.language, back.mcp_enabled), (ThemeChoice::Light, Language::French, true));
        assert_eq!(back.mcp_hidden, vec![AccountId(2)]);

        let old: Settings = serde_json::from_str(r#"{"accounts":[],"next_id":1}"#).expect("old file");
        assert!(old.saved_searches.is_empty() && !old.mcp_enabled && old.mcp_hidden.is_empty());
        assert_eq!((old.theme, old.language), (ThemeChoice::Dark, Language::English));
    }

    #[test]
    fn removing_an_account_forgets_its_searches_and_mcp_choice() {
        let mut s = Settings::default();
        s.saved_searches.extend(SavedSearch::new("Mine", "x", SearchScope::Accounts(vec![AccountId(4)]), FolderScope::Account, false));
        s.saved_searches.extend(SavedSearch::new("Both", "y", SearchScope::Accounts(vec![AccountId(4), AccountId(5)]), FolderScope::Account, false));
        s.set_mcp_visible(AccountId(4), false);
        s.remove(AccountId(4));
        assert_eq!(s.saved_searches.len(), 1);
        assert_eq!(s.saved_searches.first().map(|x| x.accounts.clone()), Some(SearchScope::Accounts(vec![AccountId(5)])));
        assert!(s.mcp_hidden.is_empty());
        assert!(!s.mcp_visible(AccountId(4)), "no such account");
    }
}
