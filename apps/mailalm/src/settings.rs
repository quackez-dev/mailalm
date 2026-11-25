//! MailALM's settings: the accounts (without their passwords, which are in the system's keychain)
//! in `%APPDATA%\MailALM\settings.json`; the cache in `%LOCALAPPDATA%\MailALM\cache`.

use std::path::PathBuf;

use mailalm_model::{Account, AccountId};
use serde::{Deserialize, Serialize};

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
                (Self { accounts: Vec::new(), next_id: 1, saved_searches: Vec::new() }, Some(format!("The settings file was damaged and was set aside: {e}")))
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
    }
}
