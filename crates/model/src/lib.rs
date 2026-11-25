//! MailALM data model: accounts, providers and the rules of the workspace; calendars, events,
//! tasks and contacts, and meeting messages ([`pim`]).
//!
//! Pure Rust, no I/O (serde only): the desktop app, the network layer, the cache, the MCP server and
//! the tests share it.
//!
//! The workspace rules (PLAN.md, section 4.3):
//! - the account that has the focus is expanded, every other one is collapsed;
//! - "New mail" writes from the focused account; in the unified view there is no default sender,
//!   so the user always chooses;
//! - the unified view exists only when more than one account is configured;
//! - a search covers the focused account by default, every account in the unified view.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod mail;
pub mod pim;
pub mod query;

pub use mail::{Change, Flags, Folder, FolderRole, Person, Security, Server, Summary, sort_folders};
pub use query::Query;

use serde::{Deserialize, Serialize};

/// An account's identity: stable, kept in the settings, and the name of its cache folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(pub u32);

/// Built-in server settings for classic IMAP providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImapPreset {
    Videotron,
    Bell,
    /// Settings typed by the user.
    Custom,
}

impl ImapPreset {
    /// The preset's IMAP and SMTP servers; `None` for settings typed by the user. They are shown,
    /// and can be changed, before the account is added.
    ///
    /// To be confirmed with each provider's current help pages (PLAN.md, section 4.1): Vidéotron
    /// and Bell have both changed their outgoing servers over the years.
    pub fn servers(self) -> Option<(Server, Server)> {
        match self {
            ImapPreset::Videotron => Some((
                Server::new("imap.videotron.ca", 993, Security::Tls),
                Server::new("smtp.videotron.ca", 465, Security::Tls),
            )),
            ImapPreset::Bell => Some((
                Server::new("imap.bell.net", 993, Security::Tls),
                Server::new("smtphm.sympatico.ca", 587, Security::StartTls),
            )),
            ImapPreset::Custom => None,
        }
    }

    /// The preset an address's domain calls for.
    pub fn for_address(address: &str) -> Self {
        let domain = address.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
        match domain.as_str() {
            "videotron.ca" => ImapPreset::Videotron,
            "bell.net" | "sympatico.ca" => ImapPreset::Bell,
            _ => ImapPreset::Custom,
        }
    }
}

/// Who hosts the account, which decides what the server can do for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    /// Hotmail, Outlook.com, Live: Microsoft Graph.
    Outlook,
    /// Gmail: the Gmail, Calendar, Tasks and People APIs.
    Gmail,
    /// IMAP and SMTP.
    Imap(ImapPreset),
}

impl Provider {
    /// The provider's name as the interface shows it.
    pub fn label(self) -> &'static str {
        match self {
            Provider::Outlook => "Outlook",
            Provider::Gmail => "Gmail",
            Provider::Imap(ImapPreset::Videotron) => "Vidéotron",
            Provider::Imap(ImapPreset::Bell) => "Bell",
            Provider::Imap(ImapPreset::Custom) => "IMAP",
        }
    }

    /// Blocking a sender is done on the account's server (Graph `markAsJunk`, a Gmail filter),
    /// not only in MailALM.
    pub fn blocks_on_server(self) -> bool {
        matches!(self, Provider::Outlook | Provider::Gmail)
    }

    /// A Google Meet link can be created for a meeting organized from this account.
    pub fn creates_meet(self) -> bool {
        matches!(self, Provider::Gmail)
    }

    /// A Teams link can be asked for a meeting organized from this account (if Microsoft allows
    /// it for a personal account).
    pub fn creates_teams(self) -> bool {
        matches!(self, Provider::Outlook)
    }

    /// The provider keeps the calendar; otherwise MailALM keeps a local one.
    pub fn has_server_calendar(self) -> bool {
        matches!(self, Provider::Outlook | Provider::Gmail)
    }
}

/// An sRGB colour, the account's mark everywhere in the interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// One configured account. Its password is never here: it is in the system's keychain.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    /// Short name chosen by the user ("Outlook", "Work"…).
    pub name: String,
    pub address: String,
    pub provider: Provider,
    pub color: Rgb,
    /// The name the servers know the account by (usually the address).
    #[serde(default)]
    pub username: String,
    /// IMAP and SMTP servers, for an IMAP account.
    #[serde(default)]
    pub imap: Option<Server>,
    #[serde(default)]
    pub smtp: Option<Server>,
}

impl Account {
    /// The name to sign in with: the username, or the address when it is empty.
    pub fn login(&self) -> &str {
        if self.username.trim().is_empty() { &self.address } else { &self.username }
    }
}

/// A colour for the n-th account added, from a palette that reads on the dark theme.
pub fn account_color(n: usize) -> Rgb {
    const PALETTE: [Rgb; 8] = [
        Rgb(0x5b, 0x9b, 0xff),
        Rgb(0xf2, 0x91, 0x4f),
        Rgb(0xb8, 0x93, 0xff),
        Rgb(0xe3, 0xb5, 0x3f),
        Rgb(0x4f, 0xc9, 0x8c),
        Rgb(0xff, 0x6f, 0x91),
        Rgb(0x4f, 0xc3, 0xd9),
        Rgb(0xc9, 0xd1, 0x5b),
    ];
    PALETTE[n % PALETTE.len()]
}

/// What has the focus in the folder pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The unified inbox (only with more than one account).
    Unified,
    Account(AccountId),
}

/// What "New mail" does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewMail {
    /// Opens a message from this account.
    From(AccountId),
    /// Asks the user which account to send from (the unified view has no default sender).
    ChooseAccount,
    /// No account is configured: nothing to send from.
    NoAccount,
}

/// Which accounts a search covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchScope {
    AllAccounts,
    Accounts(Vec<AccountId>),
}

/// The configured accounts and what has the focus.
#[derive(Clone, Debug)]
pub struct Workspace {
    accounts: Vec<Account>,
    focus: Focus,
}

impl Workspace {
    /// The first account starts with the focus.
    pub fn new(accounts: Vec<Account>) -> Self {
        let focus = accounts.first().map_or(Focus::Unified, |a| Focus::Account(a.id));
        Self { accounts, focus }
    }

    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    pub fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// The focused account, `None` in the unified view.
    pub fn focused_account(&self) -> Option<&Account> {
        match self.focus {
            Focus::Account(id) => self.account(id),
            Focus::Unified => None,
        }
    }

    /// The unified view is offered only with more than one account.
    pub fn unified_available(&self) -> bool {
        self.accounts.len() > 1
    }

    /// Gives the focus to the unified view. Refused (returns `false`) with one account or none.
    pub fn focus_unified(&mut self) -> bool {
        if !self.unified_available() {
            return false;
        }
        self.focus = Focus::Unified;
        true
    }

    /// Gives the focus to an account, which expands it and collapses the others. Refused for an
    /// unknown account.
    pub fn focus_account(&mut self, id: AccountId) -> bool {
        if self.account(id).is_none() {
            return false;
        }
        self.focus = Focus::Account(id);
        true
    }

    /// Only the focused account is expanded.
    pub fn is_expanded(&self, id: AccountId) -> bool {
        self.focus == Focus::Account(id)
    }

    /// What "New mail" does now.
    pub fn new_mail(&self) -> NewMail {
        match self.focus {
            _ if self.accounts.is_empty() => NewMail::NoAccount,
            Focus::Account(id) => NewMail::From(id),
            Focus::Unified => NewMail::ChooseAccount,
        }
    }

    /// The scope a new search starts with: the focused account, or all of them.
    pub fn default_search_scope(&self) -> SearchScope {
        match self.focus {
            Focus::Account(id) => SearchScope::Accounts(vec![id]),
            Focus::Unified => SearchScope::AllAccounts,
        }
    }

    /// Adds an account. With a second account the unified view becomes available, but the focus
    /// stays where it is.
    pub fn add_account(&mut self, account: Account) {
        let first = self.accounts.is_empty();
        let id = account.id;
        self.accounts.push(account);
        if first {
            self.focus = Focus::Account(id);
        }
    }

    /// Removes an account. If it had the focus, the first remaining account takes it; and an
    /// account left alone always has it (no unified view with one account).
    pub fn remove_account(&mut self, id: AccountId) -> Option<Account> {
        let at = self.accounts.iter().position(|a| a.id == id)?;
        let removed = self.accounts.remove(at);
        let lost_focus = self.focus == Focus::Account(id);
        if lost_focus || (self.focus == Focus::Unified && !self.unified_available()) {
            self.focus = self.accounts.first().map_or(Focus::Unified, |a| Focus::Account(a.id));
        }
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: u32, provider: Provider) -> Account {
        Account {
            id: AccountId(id),
            name: provider.label().to_owned(),
            address: format!("user{id}@example.com"),
            provider,
            color: Rgb(0, 0, 0),
            username: String::new(),
            imap: None,
            smtp: None,
        }
    }

    fn four() -> Workspace {
        Workspace::new(vec![
            account(1, Provider::Outlook),
            account(2, Provider::Gmail),
            account(3, Provider::Imap(ImapPreset::Videotron)),
            account(4, Provider::Imap(ImapPreset::Bell)),
        ])
    }

    #[test]
    fn the_first_account_starts_with_the_focus() {
        let w = four();
        assert_eq!(w.focus(), Focus::Account(AccountId(1)));
        assert!(w.is_expanded(AccountId(1)));
    }

    #[test]
    fn only_the_focused_account_is_expanded() {
        let mut w = four();
        assert!(w.focus_account(AccountId(3)));
        let expanded: Vec<_> = w.accounts().iter().filter(|a| w.is_expanded(a.id)).map(|a| a.id).collect();
        assert_eq!(expanded, vec![AccountId(3)]);
    }

    #[test]
    fn the_unified_view_collapses_every_account() {
        let mut w = four();
        assert!(w.focus_unified());
        assert!(w.accounts().iter().all(|a| !w.is_expanded(a.id)));
    }

    #[test]
    fn new_mail_writes_from_the_focused_account() {
        let mut w = four();
        w.focus_account(AccountId(2));
        assert_eq!(w.new_mail(), NewMail::From(AccountId(2)));
    }

    #[test]
    fn new_mail_in_the_unified_view_asks_for_the_account() {
        let mut w = four();
        w.focus_unified();
        assert_eq!(w.new_mail(), NewMail::ChooseAccount);
    }

    #[test]
    fn no_unified_view_with_a_single_account() {
        let mut w = Workspace::new(vec![account(1, Provider::Outlook)]);
        assert!(!w.unified_available());
        assert!(!w.focus_unified());
        assert_eq!(w.focus(), Focus::Account(AccountId(1)));
        assert_eq!(w.new_mail(), NewMail::From(AccountId(1)));
    }

    #[test]
    fn no_account_means_nothing_to_send_from() {
        let w = Workspace::new(Vec::new());
        assert_eq!(w.new_mail(), NewMail::NoAccount);
        assert!(!w.unified_available());
    }

    #[test]
    fn an_unknown_account_cannot_take_the_focus() {
        let mut w = four();
        assert!(!w.focus_account(AccountId(99)));
        assert_eq!(w.focus(), Focus::Account(AccountId(1)));
    }

    #[test]
    fn search_starts_on_the_focused_account_or_on_all() {
        let mut w = four();
        w.focus_account(AccountId(4));
        assert_eq!(w.default_search_scope(), SearchScope::Accounts(vec![AccountId(4)]));
        w.focus_unified();
        assert_eq!(w.default_search_scope(), SearchScope::AllAccounts);
    }

    #[test]
    fn removing_the_focused_account_moves_the_focus_to_the_first() {
        let mut w = four();
        w.focus_account(AccountId(3));
        assert!(w.remove_account(AccountId(3)).is_some());
        assert_eq!(w.focus(), Focus::Account(AccountId(1)));
    }

    #[test]
    fn the_last_account_left_takes_the_focus_from_the_unified_view() {
        let mut w = Workspace::new(vec![account(1, Provider::Outlook), account(2, Provider::Gmail)]);
        w.focus_unified();
        w.remove_account(AccountId(1));
        assert_eq!(w.focus(), Focus::Account(AccountId(2)));
    }

    #[test]
    fn adding_the_first_account_gives_it_the_focus() {
        let mut w = Workspace::new(Vec::new());
        w.add_account(account(7, Provider::Gmail));
        assert_eq!(w.focus(), Focus::Account(AccountId(7)));
    }

    #[test]
    fn server_capabilities_follow_the_provider() {
        assert!(Provider::Outlook.blocks_on_server());
        assert!(Provider::Gmail.blocks_on_server());
        assert!(!Provider::Imap(ImapPreset::Bell).blocks_on_server());
        assert!(Provider::Gmail.creates_meet() && !Provider::Outlook.creates_meet());
        assert!(Provider::Outlook.creates_teams() && !Provider::Gmail.creates_teams());
    }

    #[test]
    fn presets_follow_the_address() {
        assert_eq!(ImapPreset::for_address("a@VIDEOTRON.ca"), ImapPreset::Videotron);
        assert_eq!(ImapPreset::for_address("a@bell.net"), ImapPreset::Bell);
        assert_eq!(ImapPreset::for_address("a@example.com"), ImapPreset::Custom);
        let (imap, smtp) = ImapPreset::Bell.servers().unwrap_or_else(|| (Server::new("", 0, Security::Tls), Server::new("", 0, Security::Tls)));
        assert!(imap.host_is_valid() && smtp.host_is_valid());
        assert!(ImapPreset::Custom.servers().is_none());
    }

    #[test]
    fn the_login_is_the_address_by_default() {
        let mut a = account(1, Provider::Imap(ImapPreset::Bell));
        assert_eq!(a.login(), "user1@example.com");
        a.username = "alex".into();
        assert_eq!(a.login(), "alex");
    }
}
