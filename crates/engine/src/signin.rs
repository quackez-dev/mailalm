//! Signing in to an account in the browser, on the provider's own page: Microsoft for Hotmail and
//! Outlook.com (phase 3), Google for Gmail (phase 4). The password is typed there, never in
//! MailALM; what comes back is a refresh token, kept in the keychain.

use std::time::Duration;

use mailalm_model::Provider;
use mailalm_net::NetError;
use mailalm_net::gmail::GmailClient;
use mailalm_net::graph::GraphClient;
use mailalm_net::oauth;

/// How long the browser sign-in may take.
const SIGN_IN_WAIT: Duration = Duration::from_secs(5 * 60);

/// What a browser sign-in gives: who signed in, and what to keep in the keychain.
pub struct OAuthAccount {
    pub provider: Provider,
    pub address: String,
    pub name: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for OAuthAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthAccount").field("provider", &self.provider).field("address", &self.address).field("name", &self.name).finish_non_exhaustive()
    }
}

fn describe(who: &str, e: &NetError) -> String {
    if e.is_auth() { format!("{who} refused the sign-in: {e}") } else { e.to_string() }
}

/// Whether this build can sign in to Microsoft (its client id was set when it was built).
pub fn microsoft_available() -> bool {
    oauth::microsoft().is_some()
}

/// Whether this build can sign in to Google (its client id and secret were set when it was built).
pub fn google_available() -> bool {
    oauth::google().is_some()
}

/// Signs in to Microsoft in the browser (`open` shows the page), then asks Graph who signed in.
/// Blocking, up to five minutes: run it on a thread.
pub fn microsoft_sign_in(login_hint: &str, open: &mut dyn FnMut(&str) -> Result<(), String>) -> Result<OAuthAccount, String> {
    let provider = oauth::microsoft().ok_or("This copy of MailALM was built without Microsoft sign-in.")?;
    let tokens = oauth::authorize(&provider, login_hint, open, SIGN_IN_WAIT).map_err(|e| describe("Microsoft", &e))?;
    let refresh_token = tokens.refresh.clone().ok_or("Microsoft did not allow MailALM to stay signed in (no refresh token).")?;
    let me = GraphClient::new(&tokens.access).me().map_err(|e| format!("Signed in, but the mailbox could not be read: {}", describe("Microsoft", &e)))?;
    let address = if me.address.trim().is_empty() { tokens.user.clone().unwrap_or_default() } else { me.address };
    if address.trim().is_empty() {
        return Err("Microsoft did not say which account signed in.".to_owned());
    }
    Ok(OAuthAccount { provider: Provider::Outlook, address: address.trim().to_owned(), name: me.name, refresh_token })
}

/// Signs in to Google in the browser (`open` shows the page), checks that every permission was
/// given (Google's page lets each one be unticked), then asks Gmail who signed in. Blocking, up
/// to five minutes: run it on a thread.
pub fn google_sign_in(login_hint: &str, open: &mut dyn FnMut(&str) -> Result<(), String>) -> Result<OAuthAccount, String> {
    let provider = oauth::google().ok_or("This copy of MailALM was built without Google sign-in.")?;
    let tokens = oauth::authorize(&provider, login_hint, open, SIGN_IN_WAIT).map_err(|e| describe("Google", &e))?;
    if !oauth::GOOGLE_SCOPE.split_whitespace().all(|s| tokens.granted(s)) {
        return Err("Google gave MailALM only part of the permissions it needs (reading and changing mail, filters). Sign in again and tick every box on Google's page.".to_owned());
    }
    let refresh_token = tokens.refresh.clone().ok_or("Google did not allow MailALM to stay signed in (no refresh token).")?;
    let profile =
        GmailClient::new(&tokens.access).profile().map_err(|e| format!("Signed in, but the mailbox could not be read: {}", describe("Google", &e)))?;
    let address = if profile.address.trim().is_empty() { tokens.user.clone().unwrap_or_default() } else { profile.address };
    if address.trim().is_empty() {
        return Err("Google did not say which account signed in.".to_owned());
    }
    Ok(OAuthAccount { provider: Provider::Gmail, address: address.trim().to_owned(), name: String::new(), refresh_token })
}
