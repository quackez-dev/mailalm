//! Where a search looks and what it found (PLAN.md, section 6): the scope chosen beside the search
//! box (the current account, accounts chosen, or all of them; then the current folder, with its
//! subfolders, or the whole account), the searches kept as search folders, and the server's
//! results put below the cache's.
//!
//! Pure: the interface (`ui/search.rs`) sends what [`resolve`] gives to the accounts' workers.

use mailalm_model::{AccountId, FolderScope, SavedSearch, SearchScope, Summary};

use crate::i18n::{tr, tr_key, trf};

/// Which accounts a search covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccountsChoice {
    /// The account that has the focus (every account in the unified views).
    #[default]
    Current,
    /// Those ticked in the scope menu.
    Chosen,
    All,
}

/// Which folders of an account a search covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FoldersChoice {
    /// The folder shown (in its account; the other accounts are searched whole).
    Current,
    /// The folder shown and the folders under it.
    WithSubfolders,
    #[default]
    Account,
}

/// The scope chosen beside the search box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub accounts: AccountsChoice,
    /// The accounts ticked, for [`AccountsChoice::Chosen`].
    pub chosen: Vec<AccountId>,
    pub folders: FoldersChoice,
    /// The servers are asked too; their messages come "from the server".
    pub server: bool,
}

/// The operators the search box knows, as the hint under it lists them (shown with `tr`).
pub const OPERATORS: &str = tr_key("from:  to:  subject:  has:attachment  is:unread  is:read  is:flagged  before:2026-12-31  after:2026-01-01  in:folder  account:name  category:name  \"a phrase\"");

/// The most searches kept as search folders.
pub const MAX_SAVED: usize = 100;

/// What a search covers: each account searched and its folders. `accounts` are the accounts
/// configured, in their order; `current` is the account with the focus and the folder shown in it
/// (none in the unified views).
pub fn resolve(scope: &Scope, accounts: &[AccountId], current: Option<(AccountId, Option<&str>)>) -> Vec<(AccountId, FolderScope)> {
    let ids: Vec<AccountId> = match scope.accounts {
        AccountsChoice::Current => match current {
            Some((id, _)) if accounts.contains(&id) => vec![id],
            _ => accounts.to_vec(),
        },
        AccountsChoice::Chosen => accounts.iter().copied().filter(|a| scope.chosen.contains(a)).collect(),
        AccountsChoice::All => accounts.to_vec(),
    };
    ids.into_iter()
        .map(|id| {
            let folder = match current {
                Some((c, Some(path))) if c == id => match scope.folders {
                    FoldersChoice::Current => FolderScope::Folder(path.to_owned()),
                    FoldersChoice::WithSubfolders => FolderScope::WithSubfolders(path.to_owned()),
                    FoldersChoice::Account => FolderScope::Account,
                },
                _ => FolderScope::Account,
            };
            (id, folder)
        })
        .collect()
}

/// A search kept as a search folder, from what [`resolve`] gave: its accounts (all of them when
/// every account was searched) and its folders when it searched one account. `None` with an empty
/// name or query.
pub fn to_saved(name: &str, query: &str, targets: &[(AccountId, FolderScope)], all_accounts: &[AccountId], server: bool) -> Option<SavedSearch> {
    let ids: Vec<AccountId> = targets.iter().map(|(a, _)| *a).collect();
    let accounts = if !all_accounts.is_empty() && all_accounts.iter().all(|a| ids.contains(a)) { SearchScope::AllAccounts } else { SearchScope::Accounts(ids) };
    let folders = match targets {
        [(_, f)] => f.clone(),
        _ => FolderScope::Account,
    };
    SavedSearch::new(name, query, accounts, folders, server)
}

/// What a search folder covers now: its accounts that still exist, each with its folders.
pub fn resolve_saved(s: &SavedSearch, accounts: &[AccountId]) -> Vec<(AccountId, FolderScope)> {
    accounts.iter().copied().filter(|a| s.accounts.covers(*a)).map(|a| (a, s.folders.clone())).collect()
}

/// Adds a search folder; refused with a name already taken (case ignored) or past [`MAX_SAVED`].
pub fn add_saved(list: &mut Vec<SavedSearch>, s: SavedSearch) -> Result<(), String> {
    if list.len() >= MAX_SAVED {
        return Err(trf!("MailALM keeps at most {count} search folders: delete one first.", count = MAX_SAVED));
    }
    if name_taken(list, &s.name, None) {
        return Err(trf!("A search folder is already named \"{name}\".", name = s.name));
    }
    list.push(s);
    Ok(())
}

/// Renames search folder `index`; refused with an empty name or one already taken.
pub fn rename_saved(list: &mut [SavedSearch], index: usize, name: &str) -> Result<(), String> {
    let cleaned = mailalm_model::pim::clean_line(name, mailalm_model::query::MAX_SAVED_NAME);
    if cleaned.is_empty() {
        return Err(tr("A search folder needs a name.").to_owned());
    }
    if name_taken(list, &cleaned, Some(index)) {
        return Err(trf!("A search folder is already named \"{name}\".", name = cleaned));
    }
    let s = list.get_mut(index).ok_or(tr("This search folder is no longer there."))?;
    s.name = cleaned;
    Ok(())
}

fn name_taken(list: &[SavedSearch], name: &str, except: Option<usize>) -> bool {
    list.iter().enumerate().any(|(i, s)| Some(i) != except && s.name.to_lowercase() == name.to_lowercase())
}

/// One message found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub account: AccountId,
    pub folder: String,
    pub summary: Summary,
}

impl Hit {
    fn same(&self, other: &Hit) -> bool {
        self.account == other.account
            && ((self.folder == other.folder && self.summary.uid == other.summary.uid)
                || (!self.summary.message_id.is_empty() && self.summary.message_id == other.summary.message_id))
    }
}

/// The server's messages that the cache's results don't already have (same message, or same
/// Message-ID in the account), newest first: they are shown below the cache's, "from the server".
pub fn server_only(local: &[Hit], server: Vec<Hit>) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    for h in server {
        if !local.iter().any(|l| l.same(&h)) && !out.iter().any(|o| o.same(&h)) {
            out.push(h);
        }
    }
    out.sort_by(|a, b| b.summary.date.cmp(&a.summary.date));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: AccountId = AccountId(1);
    const B: AccountId = AccountId(2);
    const C: AccountId = AccountId(3);

    fn hit(account: AccountId, folder: &str, uid: u32, id: &str, date: i64) -> Hit {
        Hit { account, folder: folder.into(), summary: Summary { uid, message_id: id.into(), date, ..Summary::default() } }
    }

    #[test]
    fn the_scope_resolves_to_accounts_and_folders() {
        let all = [A, B, C];
        let current = Some((B, Some("Work")));
        let scope = |accounts, folders| Scope { accounts, chosen: vec![C, A], folders, server: false };
        assert_eq!(resolve(&scope(AccountsChoice::Current, FoldersChoice::Current), &all, current), vec![(B, FolderScope::Folder("Work".into()))]);
        assert_eq!(resolve(&scope(AccountsChoice::Current, FoldersChoice::WithSubfolders), &all, current), vec![(B, FolderScope::WithSubfolders("Work".into()))]);
        // Chosen accounts keep the accounts' order; the folder only narrows its own account.
        assert_eq!(resolve(&scope(AccountsChoice::Chosen, FoldersChoice::Current), &all, current), vec![(A, FolderScope::Account), (C, FolderScope::Account)]);
        let every = resolve(&scope(AccountsChoice::All, FoldersChoice::Current), &all, current);
        assert_eq!(every, vec![(A, FolderScope::Account), (B, FolderScope::Folder("Work".into())), (C, FolderScope::Account)]);
        // The unified view has no current account: "current" is every account, whole.
        assert_eq!(resolve(&scope(AccountsChoice::Current, FoldersChoice::Current), &all, None).len(), 3);
        // A removed account chosen before is left out.
        let gone = Scope { accounts: AccountsChoice::Chosen, chosen: vec![AccountId(9)], ..Scope::default() };
        assert!(resolve(&gone, &all, current).is_empty());
    }

    #[test]
    fn search_folders_keep_their_scope() {
        let all = [A, B, C];
        let one = to_saved("Work mail", "from:zoe", &[(B, FolderScope::WithSubfolders("Work".into()))], &all, true).expect("saved");
        assert_eq!(one.accounts, SearchScope::Accounts(vec![B]));
        assert_eq!(one.folders, FolderScope::WithSubfolders("Work".into()));
        assert_eq!(resolve_saved(&one, &all), vec![(B, FolderScope::WithSubfolders("Work".into()))]);
        let every = to_saved("Invoices", "invoice", &[(A, FolderScope::Account), (B, FolderScope::Folder("x".into())), (C, FolderScope::Account)], &all, false).expect("saved");
        assert_eq!((every.accounts.clone(), every.folders.clone()), (SearchScope::AllAccounts, FolderScope::Account));
        // An account added later is covered by "all accounts".
        assert_eq!(resolve_saved(&every, &[A, B, C, AccountId(4)]).len(), 4);
        assert!(to_saved(" ", "x", &[(A, FolderScope::Account)], &all, false).is_none());
    }

    #[test]
    fn search_folders_are_added_renamed_and_unique() {
        let mut list = Vec::new();
        let s = |name: &str| SavedSearch::new(name, "q", SearchScope::AllAccounts, FolderScope::Account, false).expect("saved");
        assert!(add_saved(&mut list, s("Invoices")).is_ok());
        assert!(add_saved(&mut list, s("Receipts")).is_ok());
        assert!(add_saved(&mut list, s("invoices")).is_err(), "names are unique, case ignored");
        assert!(rename_saved(&mut list, 1, "INVOICES").is_err());
        assert!(rename_saved(&mut list, 1, "  ").is_err());
        assert!(rename_saved(&mut list, 1, "Paid\r\nbills").is_ok());
        assert_eq!(list.get(1).map(|x| x.name.as_str()), Some("Paid bills"));
        assert!(rename_saved(&mut list, 0, "Invoices").is_ok(), "its own name again");
        assert!(rename_saved(&mut list, 7, "Other").is_err());
        for n in list.len()..MAX_SAVED {
            assert!(add_saved(&mut list, s(&format!("S{n}"))).is_ok());
        }
        assert!(add_saved(&mut list, s("One more")).is_err());
    }

    #[test]
    fn server_results_go_below_without_repeats() {
        let local = vec![hit(A, "INBOX", 1, "m1@x", 50), hit(B, "INBOX", 7, "", 40)];
        let server = vec![
            hit(A, "INBOX", 1, "m1@x", 50),
            hit(A, "Archive", 9, "m1@x", 50),
            hit(B, "INBOX", 7, "", 40),
            hit(B, "Archive", 3, "old@x", 10),
            hit(A, "Archive", 4, "new@x", 90),
            hit(A, "Archive", 4, "new@x", 90),
        ];
        let only = server_only(&local, server);
        let ids: Vec<(u32, i64)> = only.iter().map(|h| (h.summary.uid, h.summary.date)).collect();
        assert_eq!(ids, vec![(4, 90), (3, 10)]);
    }
}
