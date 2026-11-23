//! Account passwords, in the system's keychain only (PLAN.md, section 10.3): the Credential
//! Manager on Windows. Never in a file, never in a log line, never in an error message.

use mailalm_model::AccountId;

const SERVICE: &str = "MailALM";

fn entry(account: AccountId) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &format!("account-{}|imap", account.0)).map_err(|e| format!("The system keychain cannot be used: {e}"))
}

/// The account's password. `Ok(None)` when none is kept.
pub fn password(account: AccountId) -> Result<Option<String>, String> {
    match entry(account)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("The password could not be read from the system keychain: {e}")),
    }
}

pub fn set_password(account: AccountId, password: &str) -> Result<(), String> {
    entry(account)?.set_password(password).map_err(|e| format!("The password could not be kept in the system keychain: {e}"))
}

pub fn delete_password(account: AccountId) -> Result<(), String> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("The password could not be removed from the system keychain: {e}")),
    }
}
