//! Account secrets, in the system's keychain only (PLAN.md, section 10.3): the Credential Manager on
//! Windows. Never in a file, never in a log line, never in an error message.
//!
//! - an IMAP account's password;
//! - an Outlook account's refresh token (Microsoft sign-in). Access tokens are never kept: they are
//!   fetched from the refresh token when needed and live in memory only.

use mailalm_model::AccountId;

const SERVICE: &str = "MailALM";

/// The Credential Manager holds at most 2,560 bytes per entry; a refresh token can be longer, so
/// it is kept in parts of this size.
const PART: usize = 2_000;
/// The most parts a token may take (16 KB: far more than Microsoft's tokens).
const MAX_PARTS: usize = 8;

fn entry_named(user: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, user).map_err(|e| format!("The system keychain cannot be used: {e}"))
}

fn entry(account: AccountId) -> Result<keyring::Entry, String> {
    entry_named(&format!("account-{}|imap", account.0))
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
    delete(&entry(account)?)
}

fn delete(e: &keyring::Entry) -> Result<(), String> {
    match e.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("A secret could not be removed from the system keychain: {e}")),
    }
}

fn token_count_entry(account: AccountId) -> Result<keyring::Entry, String> {
    entry_named(&format!("account-{}|oauth-parts", account.0))
}

fn token_part_entry(account: AccountId, n: usize) -> Result<keyring::Entry, String> {
    entry_named(&format!("account-{}|oauth-{n}", account.0))
}

/// The account's refresh token (an Outlook account). `Ok(None)` when none is kept.
pub fn refresh_token(account: AccountId) -> Result<Option<String>, String> {
    let read_failed = |e: keyring::Error| format!("The sign-in could not be read from the system keychain: {e}");
    let count = match token_count_entry(account)?.get_password() {
        Ok(c) => c.trim().parse::<usize>().ok().filter(|c| (1..=MAX_PARTS).contains(c)),
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(e) => return Err(read_failed(e)),
    };
    let Some(count) = count else { return Ok(None) };
    let mut token = Vec::new();
    for n in 0..count {
        match token_part_entry(account, n)?.get_secret() {
            Ok(part) => token.extend_from_slice(&part),
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(read_failed(e)),
        }
    }
    Ok(String::from_utf8(token).ok().filter(|t| !t.is_empty()))
}

/// Keeps the account's refresh token (replacing the one before).
pub fn set_refresh_token(account: AccountId, token: &str) -> Result<(), String> {
    let parts: Vec<&[u8]> = token.as_bytes().chunks(PART).collect();
    if parts.is_empty() || parts.len() > MAX_PARTS {
        return Err("The sign-in Microsoft returned is not one MailALM can keep.".to_owned());
    }
    let failed = |e: keyring::Error| format!("The sign-in could not be kept in the system keychain: {e}");
    for (n, part) in parts.iter().enumerate() {
        token_part_entry(account, n)?.set_secret(part).map_err(failed)?;
    }
    // The count last: a token half written is never read as a whole one.
    token_count_entry(account)?.set_password(&parts.len().to_string()).map_err(failed)?;
    for n in parts.len()..MAX_PARTS {
        delete(&token_part_entry(account, n)?)?;
    }
    Ok(())
}

pub fn delete_refresh_token(account: AccountId) -> Result<(), String> {
    delete(&token_count_entry(account)?)?;
    for n in 0..MAX_PARTS {
        delete(&token_part_entry(account, n)?)?;
    }
    Ok(())
}

/// Everything kept for an account (when it is removed).
pub fn forget(account: AccountId) -> Result<(), String> {
    delete_password(account)?;
    delete_refresh_token(account)
}
