// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/oauth.rs. MIT OR Apache-2.0.
// Changed: Microsoft personal accounts (for Graph) and Google (for the Gmail API) only; the client id, and Google's client secret, are baked in at build time only (no run-time override); no XOAUTH2; the redirect is http://localhost on 127.0.0.1 and ::1 for Microsoft, http://127.0.0.1 for Google; tokens checked before use; codes, tokens and the client secret blanked out of any error.

//! Signing in with OAuth 2.0: to a Microsoft personal account (Outlook.com, Hotmail, Live), for
//! Microsoft Graph, and to a Google account, for the Gmail API.
//!
//! The authorization-code flow for native apps (RFC 8252): the system browser opens the
//! provider's sign-in page, which sends the browser back to a one-shot listener on this computer
//! (`http://localhost:{port}` for Microsoft, `http://127.0.0.1:{port}` for Google); the code it
//! carries is exchanged for tokens with PKCE (RFC 7636), so a code caught on the way is useless to
//! anyone else. The refresh token is what the keychain keeps; access tokens are fetched from it as
//! needed and never stored.
//!
//! Providers issue tokens only to registered apps: the registration's client id is baked into
//! the build (`MAILALM_MICROSOFT_CLIENT_ID`; `MAILALM_GOOGLE_CLIENT_ID` and
//! `MAILALM_GOOGLE_CLIENT_SECRET`). Without them, that provider's sign-in isn't offered. Google
//! asks desktop apps for their client secret too, though it can't stay secret in a program anyone
//! can take apart; it is still kept out of every error and `Debug`.

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ring::rand::SecureRandom;
use serde_json::Value;

use crate::http::{self, Request, form, pct};
use crate::{NetError, Result, clip};

/// What MailALM asks Microsoft for: reading and changing mail, sending it, the mailbox's
/// settings (automatic replies, blocked senders), who the user is, the calendars (events,
/// invitations), Microsoft To Do's tasks and the contacts, and a refresh token. A token from
/// before the calendar, tasks and contacts were asked for can't be refreshed with this scope:
/// the account signs in again.
pub const MICROSOFT_SCOPE: &str = "https://graph.microsoft.com/User.Read https://graph.microsoft.com/Mail.ReadWrite \
https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/MailboxSettings.ReadWrite \
https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Tasks.ReadWrite \
https://graph.microsoft.com/Contacts.ReadWrite offline_access";

/// What MailALM asks Google for: reading, changing and sending mail, short of deleting it for
/// good (`gmail.modify`), the mailbox's basic settings, its filters (`gmail.settings.basic`), the
/// calendars (`calendar`: events, invitations, Meet links), Google Tasks (`tasks`), and the
/// contacts, read only (`contacts.readonly`: they complete addresses). Google's consent screen
/// lets the user untick any of them: see [`Tokens::granted`].
pub const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.settings.basic \
https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/tasks https://www.googleapis.com/auth/contacts.readonly";

/// What the browser sign-in asks for besides the provider's scope: who signed in (an ID token).
/// Refreshing asks for the mail permissions alone.
const IDENTITY_SCOPE: &str = "openid email profile";

/// An OAuth provider's endpoints and the app's registration with it. Its `Debug` doesn't show the
/// client secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Provider {
    pub name: String,
    pub auth_url: String,
    pub token_url: String,
    /// The permissions asked for, separated by spaces.
    pub scope: String,
    pub client_id: String,
    /// Sent with both token requests when there is one (Google's desktop apps have one).
    pub client_secret: Option<String>,
    /// The redirect's host: `localhost` (the listener on 127.0.0.1 and ::1), or `127.0.0.1` (on
    /// IPv4 alone), as the provider takes it.
    pub redirect_host: &'static str,
    /// Query parameters added to the sign-in page's address, each starting with `&`.
    pub extra_auth: &'static str,
    /// Whether the token requests name the scope (Microsoft's do; Google takes it from the grant).
    pub refresh_scope: bool,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("name", &self.name)
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("scope", &self.scope)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "<redacted>"))
            .field("redirect_host", &self.redirect_host)
            .field("extra_auth", &self.extra_auth)
            .field("refresh_scope", &self.refresh_scope)
            .finish()
    }
}

/// Microsoft (personal accounts only, as the app is registered), when this build has a client
/// id: `MAILALM_MICROSOFT_CLIENT_ID` at build time, trimmed, not empty. There is no run-time
/// override: what the program signs in as is decided when it is built.
pub fn microsoft() -> Option<Provider> {
    microsoft_with(option_env!("MAILALM_MICROSOFT_CLIENT_ID"))
}

fn microsoft_with(client_id: Option<&str>) -> Option<Provider> {
    let client_id = client_id.map(str::trim).filter(|id| !id.is_empty())?;
    Some(Provider {
        name: "Microsoft".into(),
        auth_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
        token_url: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".into(),
        scope: MICROSOFT_SCOPE.into(),
        client_id: client_id.into(),
        client_secret: None,
        redirect_host: "localhost",
        extra_auth: "&response_mode=query",
        refresh_scope: true,
    })
}

/// Google, when this build has both a client id and a client secret: `MAILALM_GOOGLE_CLIENT_ID`
/// and `MAILALM_GOOGLE_CLIENT_SECRET` at build time, trimmed, not empty. No run-time override.
pub fn google() -> Option<Provider> {
    google_with(option_env!("MAILALM_GOOGLE_CLIENT_ID"), option_env!("MAILALM_GOOGLE_CLIENT_SECRET"))
}

fn google_with(client_id: Option<&str>, client_secret: Option<&str>) -> Option<Provider> {
    let client_id = client_id.map(str::trim).filter(|id| !id.is_empty())?;
    let client_secret = client_secret.map(str::trim).filter(|s| !s.is_empty())?;
    Some(Provider {
        name: "Google".into(),
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
        token_url: "https://oauth2.googleapis.com/token".into(),
        scope: GOOGLE_SCOPE.into(),
        client_id: client_id.into(),
        client_secret: Some(client_secret.into()),
        // Google's guide for desktop apps names the loopback address rather than `localhost`.
        redirect_host: "127.0.0.1",
        // A refresh token (`offline`), given again at every sign-in (`consent`): without
        // `consent`, a second sign-in to the same account gets none.
        extra_auth: "&access_type=offline&prompt=consent",
        refresh_scope: false,
    })
}

/// Tokens from the provider. Its `Debug` shows none of them.
#[derive(Clone)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// When the access token runs out, in Unix seconds, a minute early: wall-clock time, which
    /// goes on while the computer sleeps (an `Instant` doesn't on every system).
    pub expires_at: i64,
    /// Who signed in, from the ID token (its `email`, else `preferred_username`): only after a
    /// browser sign-in, which asks for it.
    pub user: Option<String>,
    /// The permissions granted, separated by spaces, when the answer named them (its `scope`).
    pub scope: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokens(<redacted>)")
    }
}

impl Tokens {
    /// Whether the access token has run out (or is about to: see `expires_at`).
    pub fn expired(&self) -> bool {
        now_unix() >= self.expires_at
    }

    /// Whether `scope` (one permission) was granted: Google's consent screen lets the user untick
    /// some. True when the answer didn't say which were granted (all asked for, then).
    pub fn granted(&self, scope: &str) -> bool {
        match &self.scope {
            None => true,
            Some(granted) => granted.split_whitespace().any(|g| g == scope),
        }
    }
}

/// The wall-clock time in Unix seconds.
pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

fn random(n: usize) -> Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    ring::rand::SystemRandom::new().fill(&mut v).map_err(|_| NetError::Protocol("no randomness available".into()))?;
    Ok(v)
}

/// The S256 challenge of a PKCE verifier (RFC 7636 §4.2).
fn challenge_of(verifier: &str) -> String {
    crate::base64::encode_url(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref())
}

/// A PKCE verifier (32 random bytes, base64url: 43 characters) and its S256 challenge.
fn pkce() -> Result<(String, String)> {
    let verifier = crate::base64::encode_url(&random(32)?);
    let challenge = challenge_of(&verifier);
    Ok((verifier, challenge))
}

/// Longest ID token looked into.
const MAX_ID_TOKEN: usize = 32 << 10;
/// Longest access or refresh token taken.
const MAX_TOKEN: usize = 64 << 10;
/// Most of the granted scope kept (in characters).
const MAX_SCOPE: usize = 8 << 10;

/// The address in an ID token (a JWT: its middle part is base64url JSON). It came straight from
/// the provider over TLS, so its signature isn't checked; it's only ever shown, or compared with
/// the account's address.
fn id_token_user(id_token: &str) -> Option<String> {
    if id_token.len() > MAX_ID_TOKEN {
        return None;
    }
    let claims = crate::base64::decode(id_token.split('.').nth(1)?)?;
    let v: Value = serde_json::from_slice(&claims).ok()?;
    ["email", "preferred_username"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .find(|a| a.contains('@') && a.len() <= 320 && !a.chars().any(|c| c.is_control() || c.is_whitespace()))
        .map(str::to_string)
}

/// A token as it can go into a header: printable ASCII, not empty, of a sane length.
fn token_ok(t: &str) -> bool {
    !t.is_empty() && t.len() <= MAX_TOKEN && t.bytes().all(|b| b.is_ascii_graphic())
}

/// One request to the token endpoint: `fields`, the client id (and secret, if any), and `scope`
/// when the provider takes it. Codes, verifiers, refresh tokens and the client secret sent are
/// blanked out of any error (a server that echoes what it was sent).
fn token_request(p: &Provider, fields: &[(&str, &str)], scope: &str) -> Result<Tokens> {
    let mut secrets: Vec<&str> = fields.iter().filter(|(k, _)| matches!(*k, "code" | "code_verifier" | "refresh_token")).map(|(_, v)| *v).collect();
    let mut pairs: Vec<(&str, &str)> = vec![("client_id", p.client_id.as_str())];
    if let Some(secret) = p.client_secret.as_deref() {
        pairs.push(("client_secret", secret));
        secrets.push(secret);
    }
    pairs.extend_from_slice(fields);
    if p.refresh_scope {
        pairs.push(("scope", scope));
    }
    let req = Request::post(&p.token_url, "application/x-www-form-urlencoded", form(&pairs), 1 << 20).header("Accept", "application/json");
    token_answer(p, &req).map_err(|e| e.without(&secrets))
}

fn token_answer(p: &Provider, req: &Request) -> Result<Tokens> {
    let r = http::send(req)?;
    let busy = r.status >= 500 || r.status == 429;
    let Ok(v) = serde_json::from_slice::<Value>(&r.body) else {
        return Err(NetError::Protocol(format!("{} answered HTTP {} without JSON", p.name, r.status)));
    };
    let code = v.get("error").and_then(Value::as_str);
    if !r.ok() || code.is_some() {
        let what = clip(v.get("error_description").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or("refused"));
        // The service failing, busy or throttling refused nothing: tried again later (taken for a
        // refusal, it would ask the user to sign in again).
        if busy || matches!(code, Some("server_error" | "temporarily_unavailable")) {
            return Err(NetError::Protocol(format!("{} couldn't answer now (HTTP {}): {what}", p.name, r.status)));
        }
        // A revoked or expired grant, a refused client: signing in again.
        return Err(NetError::Auth(format!("{}: {what}", p.name)));
    }
    let access = v
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|t| token_ok(t))
        .ok_or_else(|| NetError::Protocol(format!("{} sent no usable access token", p.name)))?;
    let refresh = match v.get("refresh_token").and_then(Value::as_str) {
        Some(t) if token_ok(t) => Some(t.to_string()),
        Some(_) => return Err(NetError::Protocol(format!("{} sent a refresh token that isn't one", p.name))),
        None => None,
    };
    let expires = v
        .get("expires_in")
        .and_then(|e| e.as_u64().or_else(|| e.as_str().and_then(|s| s.trim().parse().ok())))
        .unwrap_or(3600)
        .clamp(60, 86_400);
    Ok(Tokens {
        access: access.to_string(),
        refresh,
        user: v.get("id_token").and_then(Value::as_str).and_then(id_token_user),
        scope: v.get("scope").and_then(Value::as_str).map(|s| s.chars().take(MAX_SCOPE).collect()),
        // A minute early, so a token never runs out mid-request.
        expires_at: now_unix().saturating_add(i64::try_from(expires - 60).unwrap_or(0)),
    })
}

/// A fresh access token from a refresh token (and, from Microsoft, a new refresh token: the
/// caller keeps it in place of the old one; Google sends none, the old one goes on).
pub fn refresh(p: &Provider, refresh_token: &str) -> Result<Tokens> {
    token_request(p, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)], &p.scope)
}

/// The query parameters of a request target (`/?code=…&state=…`).
fn query_params(target: &str) -> Vec<(String, String)> {
    let q = target.split_once('?').map_or("", |(_, q)| q);
    let q = q.split('#').next().unwrap_or("");
    q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (unpct(k), unpct(&v.replace('+', " ")))).take(64).collect()
}

/// Percent-decoding; a `%` not followed by two hex digits is kept as it is.
fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (c, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How long one connection to the redirect listener has to send its request line. A browser
/// sends it at once; one that sends nothing (a speculative connection) is let go after this,
/// so the redirect waiting behind it isn't held up for long.
const REQUEST_LINE_WAIT: Duration = Duration::from_secs(2);

/// The longest request line read from the redirect listener.
const REQUEST_LINE_MAX: usize = 8192;

/// The most read of one request to the redirect listener: its line and its headers.
const REQUEST_MAX: usize = 64 * 1024;

/// The first line of a request on `sock`, or `None` when the connection doesn't send a whole
/// one (at most [`REQUEST_LINE_MAX`] bytes) within `wait`, or fails.
///
/// The request's headers are read to their end too (within `wait`, at most [`REQUEST_MAX`]
/// bytes in all): a connection closed with part of the request unread can be reset, and the
/// browser then shows an error instead of the page written to it.
fn request_line(sock: &TcpStream, wait: Duration) -> Option<String> {
    sock.set_nonblocking(false).ok()?;
    let until = Instant::now().checked_add(wait)?;
    let mut got: Vec<u8> = Vec::new();
    // Where the line ends in `got`, once it has.
    let mut line_end: Option<usize> = None;
    let mut buf = [0u8; 1024];
    loop {
        let chunk = until
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .and_then(|left| sock.set_read_timeout(Some(left)).ok())
            .and_then(|()| (&*sock).read(&mut buf).ok())
            .and_then(|n| buf.get(..n))
            .filter(|c| !c.is_empty());
        // Out of time, closed or failed: the line counts if it came whole.
        let Some(chunk) = chunk else {
            break;
        };
        // An empty line ends the headers; it may have begun in the read before.
        let from = got.len().saturating_sub(2);
        got.extend_from_slice(chunk);
        if line_end.is_none() {
            line_end = got.iter().position(|&b| b == b'\n');
            if line_end.unwrap_or(got.len()) > REQUEST_LINE_MAX {
                return None;
            }
        }
        let tail = got.get(from..).unwrap_or_default();
        let ended = tail.windows(2).any(|w| w == b"\n\n") || tail.windows(3).any(|w| w == b"\n\r\n");
        if line_end.is_some() && (ended || got.len() >= REQUEST_MAX) {
            break;
        }
    }
    let line = got.get(..line_end?)?;
    Some(String::from_utf8_lossy(line).into_owned())
}

/// What one connection to the redirect listener brought: nothing that counts (`None`: a stray
/// request, a half one, a silent connection), the code, or the provider's refusal.
fn redirect_answer(sock: &TcpStream, state: &str, wait: Duration) -> Option<std::result::Result<String, String>> {
    let line = request_line(sock, wait)?;
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let params = query_params(target);
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    // Stray requests (a favicon) are ignored; only the redirect with our state counts.
    if get("state").as_deref() != Some(state) {
        let _ = (&*sock).write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    }
    let outcome = match (get("code"), get("error")) {
        (Some(code), _) if !code.is_empty() && code.len() <= 8192 => Ok(code),
        (_, error) => {
            let error = error.unwrap_or_else(|| "no code".into());
            let detail = get("error_description").map(|d| format!(": {d}")).unwrap_or_default();
            Err(clip(&format!("{error}{detail}")))
        }
    };
    let (status, page) = match outcome {
        Ok(_) => ("200 OK", "You're signed in to MailALM. You can close this window."),
        Err(_) => ("400 Bad Request", "MailALM wasn't allowed to sign in. You can close this window and try again."),
    };
    let body = format!("<!doctype html><meta charset=utf-8><title>MailALM</title><p style=\"font:16px sans-serif;margin:3em\">{page}</p>");
    let _ = (&*sock).write_all(
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    Some(outcome)
}

/// Run the browser sign-in: `open` shows the provider's page (the system browser), the browser
/// comes back to a listener on this computer, and the code becomes tokens. Fails after
/// `timeout` without an answer.
pub fn authorize(p: &Provider, login_hint: &str, open: &mut dyn FnMut(&str) -> std::result::Result<(), String>, timeout: Duration) -> Result<Tokens> {
    authorize_with(p, login_hint, open, timeout, REQUEST_LINE_WAIT)
}

/// [`authorize`], with how long each connection to the listener may take to send its request line.
fn authorize_with(
    p: &Provider,
    login_hint: &str,
    open: &mut dyn FnMut(&str) -> std::result::Result<(), String>,
    timeout: Duration,
    line_wait: Duration,
) -> Result<Tokens> {
    // Microsoft's app is registered with `http://localhost` (it takes any port for it). A browser
    // may take `localhost` for 127.0.0.1 or for ::1: both listen, on the same port. Google's
    // redirect names 127.0.0.1 itself: IPv4 alone listens.
    let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = v4.local_addr()?.port();
    let v6 = if p.redirect_host == "localhost" { TcpListener::bind((Ipv6Addr::LOCALHOST, port)).ok() } else { None };
    let listeners: Vec<TcpListener> = std::iter::once(v4).chain(v6).collect();
    for l in &listeners {
        l.set_nonblocking(true)?;
    }
    let redirect = format!("http://{}:{port}", p.redirect_host);
    let state = crate::base64::encode_url(&random(16)?);
    let (verifier, challenge) = pkce()?;
    let mut url = format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={state}&code_challenge={challenge}&code_challenge_method=S256{}",
        p.auth_url,
        pct(&p.client_id),
        pct(&redirect),
        pct(&format!("{} {IDENTITY_SCOPE}", p.scope)),
        p.extra_auth,
    );
    if !login_hint.trim().is_empty() {
        url.push_str(&format!("&login_hint={}", pct(login_hint.trim())));
    }
    open(&url).map_err(|e| NetError::Protocol(format!("the browser couldn't be opened: {}", clip(&e))))?;
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| NetError::Invalid("sign-in wait too long".into()))?;
    let code = 'wait: loop {
        if Instant::now() > deadline {
            return Err(NetError::Auth("the sign-in wasn't finished in time".into()));
        }
        let mut idle = true;
        for l in &listeners {
            match l.accept() {
                Ok((sock, _)) => {
                    idle = false;
                    // One connection that sends nothing, half a line or fails is that
                    // connection's trouble: the redirect may be waiting behind it.
                    match redirect_answer(&sock, &state, line_wait) {
                        Some(Ok(code)) => break 'wait code,
                        Some(Err(why)) => return Err(NetError::Auth(format!("the {} sign-in was cancelled or refused ({why})", p.name))),
                        None => {}
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                // A connection that went away before it was taken.
                Err(e) if matches!(e.kind(), ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted) => idle = false,
                Err(e) => return Err(e.into()),
            }
        }
        if idle {
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    drop(listeners);
    let scope = format!("{} {IDENTITY_SCOPE}", p.scope);
    token_request(
        p,
        &[("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", redirect.as_str()), ("code_verifier", verifier.as_str())],
        &scope,
    )
}

#[cfg(test)]
mod tests;
