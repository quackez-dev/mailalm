//! Gmail through the Gmail API (v1), over HTTPS with an OAuth bearer token ([`crate::oauth`]):
//! the mailbox's address, its labels, the ids of the messages under labels, a message's header
//! fields or its whole source, label changes, the bin, sending a MIME message, and the filters
//! MailALM blocks and allows senders with.
//!
//! Gmail has labels rather than folders: a message is in the Inbox because it has the `INBOX`
//! label, in the spam because it has `SPAM`, and its id never changes. Listings go on by page
//! tokens, not links: nothing the server sends is ever followed as an address. Everything Gmail
//! sends is hostile input: sizes are capped, text is cleaned, ids are checked before they are
//! used or kept.

use std::collections::HashSet;
use std::time::Duration;

use mailalm_model::Summary;
use serde_json::{Value, json};

use crate::http::{self, Agent, Request, Response};
use crate::{NetError, Result, clip};

mod convert;

#[cfg(test)]
mod tests;

/// The Gmail API's address.
pub const GMAIL_URL: &str = "https://gmail.googleapis.com";
/// Largest JSON answer accepted for a listing, a message's header fields or a setting.
const MAX_JSON: usize = 16 << 20;
/// Largest answer accepted for a message's source (base64url, inside JSON)…
const MAX_RAW_JSON: usize = 48 << 20;
/// …and the largest source taken once decoded.
const MAX_RAW: usize = 36 << 20;
/// Largest message sent (Gmail's own limit for an upload).
const MAX_SEND: usize = 35 << 20;
/// Ids per `batchModify` (Gmail's own limit).
const MODIFY_BATCH: usize = 1000;
/// Ids asked for per page of a listing (Gmail's most).
const PAGE: usize = 500;
/// Pages followed for one listing.
const MAX_PAGES: usize = 2000;
/// Labels taken from the label list.
const MAX_LABELS: usize = 10_000;
/// Filters looked at (Gmail keeps at most 1000).
const MAX_FILTERS: usize = 1000;
/// Attempts per request while Gmail answers that it is busy.
const BUSY_ROUNDS: u32 = 4;
/// Longest single wait in seconds, whatever `Retry-After` says.
const BUSY_WAIT_CAP: u64 = 20;

/// The mailbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Its address.
    pub address: String,
    /// Where its history stands now: changes are asked for from here on.
    pub history_id: u64,
}

/// Whether a label is one of Gmail's own (`INBOX`, `SPAM`, `CATEGORY_SOCIAL`…) or the user's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelKind {
    System,
    User,
}

/// A label: its id (what requests name), and its name as shown (a user's nested labels have
/// `/` in it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub kind: LabelKind,
}

/// A message as its metadata has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GmailMessage {
    pub id: String,
    /// The ids of its labels.
    pub labels: Vec<String>,
    /// What the message list needs (its `uid` is 0: the engine numbers messages).
    pub summary: Summary,
}

/// What one of MailALM's filters does with the sender's mail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterAction {
    /// Into the spam (`addLabelIds: ["SPAM"]`).
    Spam,
    /// Into the bin (`addLabelIds: ["TRASH"]`).
    Trash,
    /// Never into the spam (`removeLabelIds: ["SPAM"]`).
    NeverSpam,
}

/// A filter of the shape MailALM makes: mail from one address, one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub id: String,
    pub from: String,
    pub action: FilterAction,
}

/// A Gmail session: the access token, and one connection kept open. Its `Debug` shows no token.
pub struct GmailClient {
    base: String,
    token: String,
    agent: Agent,
}

impl std::fmt::Debug for GmailClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GmailClient(<redacted>)")
    }
}

/// A busy answer: the request was turned away before anything was done (throttled, unavailable),
/// or failed on the way (it may have been done).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Busy {
    TurnedAway,
    Failed,
}

impl GmailClient {
    /// A session with this access token. Nothing is sent yet.
    pub fn new(access_token: &str) -> GmailClient {
        GmailClient::at(GMAIL_URL, access_token)
    }

    /// A session with a test server for Gmail (plain http to this computer is let through in tests).
    #[cfg(test)]
    pub(crate) fn with_base(base: &str, access_token: &str) -> GmailClient {
        GmailClient::at(base, access_token)
    }

    fn at(base: &str, access_token: &str) -> GmailClient {
        GmailClient { base: base.trim_end_matches('/').to_string(), token: access_token.to_string(), agent: Agent::default() }
    }

    /// Carry a new access token from now on (the old one ran out).
    pub fn set_token(&mut self, access_token: &str) {
        self.token = access_token.to_string();
    }

    /// The signed-in user's mailbox, as every request names it.
    fn api(&self) -> String {
        format!("{}/gmail/v1/users/me", self.base)
    }

    /// The mailbox's address and where its history stands.
    pub fn profile(&mut self) -> Result<Profile> {
        let v = self.get_json(&format!("{}/profile", self.api()))?;
        let address = v
            .get("emailAddress")
            .and_then(Value::as_str)
            .map(|a| convert::clean(a, 320))
            .filter(|a| a.contains('@') && !a.contains(' '))
            .ok_or_else(|| NetError::Protocol("Gmail didn't say which mailbox this is".into()))?;
        let history_id = convert::number(v.get("historyId")).ok_or_else(|| NetError::Protocol("Gmail sent no history id".into()))?;
        Ok(Profile { address, history_id })
    }

    /// Every label, the system's and the user's; one with an id that isn't one is left out. At
    /// most [`MAX_LABELS`].
    pub fn labels(&mut self) -> Result<Vec<Label>> {
        let v = self.get_json(&format!("{}/labels", self.api()))?;
        let items = v.get("labels").and_then(Value::as_array).ok_or_else(|| NetError::Protocol("a label list without its list".into()))?;
        let mut ids = HashSet::new();
        Ok(items.iter().filter_map(convert::label_of).filter(|l| ids.insert(l.id.clone())).take(MAX_LABELS).collect())
    }

    /// The ids of the messages that have every label in `labels` (none: all mail but the spam
    /// and the bin), newest first, at most `limit`, through as many pages as that takes.
    pub fn list_ids(&mut self, labels: &[&str], limit: usize) -> Result<Vec<String>> {
        if let Some(bad) = labels.iter().find(|l| !convert::id_ok(l)) {
            return Err(NetError::Invalid(format!("not a label id: {}", clip(bad))));
        }
        let mut query = String::new();
        for l in labels {
            query.push_str(&format!("&labelIds={}", http::pct(l)));
        }
        // Gmail leaves the spam and the bin out of every listing unless asked.
        if labels.iter().any(|l| matches!(*l, "SPAM" | "TRASH")) {
            query.push_str("&includeSpamTrash=true");
        }
        let mut out: Vec<String> = Vec::new();
        // A message that moved up while the pages were read comes twice: kept once.
        let mut taken: HashSet<String> = HashSet::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let left = limit.saturating_sub(out.len());
            if left == 0 {
                break;
            }
            let mut url = format!("{}/messages?maxResults={}&fields=messages/id,nextPageToken{query}", self.api(), left.min(PAGE));
            if let Some(t) = &token {
                url.push_str(&format!("&pageToken={}", http::pct(t)));
            }
            let v = self.get_json(&url)?;
            // A listing with nothing in it has no list at all.
            let items: &[Value] = match v.get("messages") {
                None | Some(Value::Null) => &[],
                Some(m) => m.as_array().map(Vec::as_slice).ok_or_else(|| NetError::Protocol("a message listing without its list".into()))?,
            };
            if items.len() > PAGE * 2 {
                return Err(NetError::TooLarge(format!("a page of {} messages", items.len())));
            }
            for m in items {
                if out.len() >= limit {
                    break;
                }
                let Some(id) = m.get("id").and_then(Value::as_str).filter(|id| convert::id_ok(id)) else { continue };
                if taken.insert(id.to_string()) {
                    out.push(id.to_string());
                }
            }
            match v.get("nextPageToken").and_then(Value::as_str) {
                Some(t) if convert::page_token_ok(t) => token = Some(t.to_string()),
                Some(_) => return Err(NetError::Protocol("Gmail sent a page token that isn't one".into())),
                None => break,
            }
        }
        Ok(out)
    }

    /// A message's labels and the header fields the message list needs; `Ok(None)` when it is
    /// gone (404).
    pub fn metadata(&mut self, id: &str) -> Result<Option<GmailMessage>> {
        check_id(id)?;
        let fields: String = convert::HEADERS.iter().map(|h| format!("&metadataHeaders={h}")).collect();
        let url = format!("{}/messages/{}?format=metadata{fields}", self.api(), http::pct(id));
        let resp = self.call(Request::get(&url, MAX_JSON))?;
        if resp.status == 404 {
            return Ok(None);
        }
        self.checked(&resp)?;
        let v = json_of(&resp)?;
        if !v.is_object() {
            return Err(NetError::Protocol("Gmail sent a message that isn't one".into()));
        }
        let labels = convert::labels_of(&v);
        let summary = convert::summary_of(&v, &labels);
        Ok(Some(GmailMessage { id: id.to_string(), labels, summary }))
    }

    /// The message's source (`format=raw`, base64url decoded); `Ok(None)` when it is gone (404).
    pub fn download(&mut self, id: &str) -> Result<Option<Vec<u8>>> {
        check_id(id)?;
        let url = format!("{}/messages/{}?format=raw&fields=raw", self.api(), http::pct(id));
        let resp = self.call(Request::get(&url, MAX_RAW_JSON))?;
        if resp.status == 404 {
            return Ok(None);
        }
        self.checked(&resp)?;
        let v = json_of(&resp)?;
        let raw = v.get("raw").and_then(Value::as_str).ok_or_else(|| NetError::Protocol("Gmail sent a message without its source".into()))?;
        if raw.len() / 4 * 3 > MAX_RAW {
            return Err(NetError::TooLarge(format!("the message is larger than {} MB", MAX_RAW >> 20)));
        }
        let source = crate::base64::decode(raw).ok_or_else(|| NetError::Protocol("Gmail sent a message source that isn't base64url".into()))?;
        Ok(Some(source))
    }

    /// Add the labels `add` to the messages, and take `remove` off them (`batchModify`,
    /// [`MODIFY_BATCH`] messages per request). Every id is checked before anything is sent.
    pub fn modify(&mut self, ids: &[String], add: &[&str], remove: &[&str]) -> Result<()> {
        check_ids(ids)?;
        if let Some(bad) = add.iter().chain(remove).find(|l| !convert::id_ok(l)) {
            return Err(NetError::Invalid(format!("not a label id: {}", clip(bad))));
        }
        if ids.is_empty() || (add.is_empty() && remove.is_empty()) {
            return Ok(());
        }
        let url = format!("{}/messages/batchModify", self.api());
        for chunk in ids.chunks(MODIFY_BATCH) {
            let body = json!({"ids": chunk, "addLabelIds": add, "removeLabelIds": remove});
            let resp = self.call(json_request(&url, &body)?)?;
            self.checked(&resp)?;
        }
        Ok(())
    }

    /// Put messages in the bin (Gmail empties it after 30 days). A message already gone counts
    /// as done.
    pub fn trash(&mut self, ids: &[String]) -> Result<()> {
        self.each(ids, "trash")
    }

    /// Take messages out of the bin. A message already gone counts as done.
    pub fn untrash(&mut self, ids: &[String]) -> Result<()> {
        self.each(ids, "untrash")
    }

    /// `POST messages/{id}/{action}` (no body) for each message, one by one; 404 counts as done.
    fn each(&mut self, ids: &[String], action: &str) -> Result<()> {
        check_ids(ids)?;
        for id in ids {
            let url = format!("{}/messages/{}/{action}", self.api(), http::pct(id));
            let resp = self.call(Request::new("POST", &url, MAX_JSON))?;
            if resp.status != 404 {
                self.checked(&resp)?;
            }
        }
        Ok(())
    }

    /// Send a MIME message (`messages.send`, uploaded as `message/rfc822`). Gmail keeps the copy
    /// in Sent itself, and takes the Bcc header out of what it delivers. Sent on a connection of
    /// its own and never sent again once it went out: an answer that doesn't come, a time-out or
    /// a failure on Google's side is [`NetError::Unconfirmed`] (it may have been sent). Only an
    /// answer that turns it away before anything was done (throttled, unavailable) lets it go
    /// again.
    pub fn send_mime(&mut self, raw: &[u8]) -> Result<()> {
        if raw.is_empty() {
            return Err(NetError::Invalid("an empty message".into()));
        }
        if raw.len() > MAX_SEND {
            return Err(NetError::TooLarge(format!("the message is larger than {} MB", MAX_SEND >> 20)));
        }
        self.agent.close();
        let url = format!("{}/upload/gmail/v1/users/me/messages/send?uploadType=media", self.base);
        let req = Request::post(&url, "message/rfc822", raw.to_vec(), MAX_JSON);
        match self.exchange(req, false, false) {
            Err(f) if f.sent => Err(NetError::Unconfirmed(self.hide(f.error).to_string())),
            Err(f) => Err(self.hide(f.error)),
            Ok(resp) if busy(&resp) == Some(Busy::Failed) => Err(NetError::Unconfirmed(format!("Gmail failed (HTTP {})", resp.status))),
            Ok(resp) => self.checked(&resp),
        }
    }

    /// The mailbox's filters that MailALM makes: mail from one address, into the spam, into the
    /// bin, or never into the spam, and nothing else. Every other filter is left out (and left
    /// alone).
    pub fn filters(&mut self) -> Result<Vec<Filter>> {
        let v = self.get_json(&format!("{}/settings/filters", self.api()))?;
        // A mailbox without filters has no list at all.
        let items: &[Value] = match v.get("filter") {
            None | Some(Value::Null) => &[],
            Some(f) => f.as_array().map(Vec::as_slice).ok_or_else(|| NetError::Protocol("a filter list without its list".into()))?,
        };
        Ok(items.iter().take(MAX_FILTERS).filter_map(convert::filter_of).collect())
    }

    /// Make a filter for mail from `from`. Gmail refusing it (HTTP 400: an action it doesn't
    /// take) is [`NetError::Server`], so that another action can be tried.
    pub fn create_filter(&mut self, from: &str, action: FilterAction) -> Result<Filter> {
        let from = from.trim();
        if !convert::address_ok(from) {
            return Err(NetError::Invalid(format!("not an address a filter can name: {}", clip(from))));
        }
        let what = match action {
            FilterAction::Spam => json!({"addLabelIds": ["SPAM"]}),
            FilterAction::Trash => json!({"addLabelIds": ["TRASH"]}),
            FilterAction::NeverSpam => json!({"removeLabelIds": ["SPAM"]}),
        };
        let req = json_request(&format!("{}/settings/filters", self.api()), &json!({"criteria": {"from": from}, "action": what}))?;
        // A filter made twice is two filters: only an answer that turned the request away before
        // anything was done lets it go again.
        let resp = self.exchange(req, true, false).map_err(|f| self.hide(f.error))?;
        self.checked(&resp)?;
        let v = json_of(&resp)?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| convert::filter_id_ok(id))
            .ok_or_else(|| NetError::Protocol("Gmail made the filter but sent no id for it".into()))?;
        Ok(Filter { id: id.to_string(), from: from.to_string(), action })
    }

    /// Delete a filter. One already gone counts as done.
    pub fn delete_filter(&mut self, id: &str) -> Result<()> {
        if !convert::filter_id_ok(id) {
            return Err(NetError::Invalid(format!("not a filter id: {}", clip(id))));
        }
        let url = format!("{}/settings/filters/{}", self.api(), http::pct(id));
        let resp = self.call(Request::new("DELETE", &url, MAX_JSON))?;
        if resp.status == 404 {
            return Ok(());
        }
        self.checked(&resp)
    }

    fn get_json(&mut self, url: &str) -> Result<Value> {
        let resp = self.call(Request::get(url, MAX_JSON))?;
        self.checked(&resp)?;
        json_of(&resp)
    }

    /// A request as most Gmail calls send it, through the limits (a busy answer of any kind goes
    /// again: the request reads, or changes things the same way twice): its answer, whatever its
    /// status.
    fn call(&mut self, req: Request) -> Result<Response> {
        self.exchange(req, true, true).map_err(|f| self.hide(f.error))
    }

    /// `req` with the token and `Accept`, sent again while Gmail answers that it is busy (after
    /// `Retry-After`, up to [`BUSY_ROUNDS`] attempts): always when it turned the request away,
    /// and when it failed on the way only with `failed_again`. `again`: whether a kept connection
    /// found closed may carry it once more on a new one.
    fn exchange(&mut self, req: Request, again: bool, failed_again: bool) -> std::result::Result<Response, http::Failure> {
        let req = req.header("Authorization", &format!("Bearer {}", self.token)).header("Accept", "application/json");
        let mut round = 0;
        loop {
            round += 1;
            let resp = self.agent.send(&req, again)?;
            let retry = match busy(&resp) {
                Some(Busy::TurnedAway) => true,
                Some(Busy::Failed) => failed_again,
                None => false,
            };
            if !retry || round >= BUSY_ROUNDS {
                return Ok(resp);
            }
            log::debug!("Gmail is busy (HTTP {}): waiting", resp.status);
            std::thread::sleep(Duration::from_secs(backoff_secs(resp.header("Retry-After"), round)));
        }
    }

    /// The answer's status as MailALM's error, the token blanked out of it.
    fn checked(&self, resp: &Response) -> Result<()> {
        check(resp).map_err(|e| self.hide(e))
    }

    /// The error with the access token blanked out of its text.
    fn hide(&self, e: NetError) -> NetError {
        e.without(&[self.token.as_str()])
    }
}

/// A message id checked before it goes into a request.
fn check_id(id: &str) -> Result<()> {
    if convert::id_ok(id) { Ok(()) } else { Err(NetError::Invalid(format!("not a message id: {}", clip(id)))) }
}

/// Every message id checked before anything is sent.
fn check_ids(ids: &[String]) -> Result<()> {
    ids.iter().try_for_each(|id| check_id(id))
}

/// A POST with a JSON body.
fn json_request(url: &str, body: &Value) -> Result<Request> {
    let bytes = serde_json::to_vec(body).map_err(|e| NetError::Invalid(e.to_string()))?;
    Ok(Request::post(url, "application/json", bytes, MAX_JSON))
}

fn json_of(resp: &Response) -> Result<Value> {
    if resp.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&resp.body).map_err(|_| NetError::Protocol(format!("Gmail sent JSON that can't be read (HTTP {})", resp.status)))
}

/// Whether the answer says Gmail is busy, and how: throttled (429, or 403 with a rate-limit
/// reason) or unavailable (503) turned the request away; another 5xx failed on the way.
pub(crate) fn busy(resp: &Response) -> Option<Busy> {
    match resp.status {
        429 | 503 => Some(Busy::TurnedAway),
        403 if rate_limited(serde_json::from_slice::<Value>(&resp.body).ok().as_ref()) => Some(Busy::TurnedAway),
        500..=599 => Some(Busy::Failed),
        _ => None,
    }
}

/// Whether a Google error names a rate limit as its reason (Gmail says so with a 403 too).
pub(crate) fn rate_limited(body: Option<&Value>) -> bool {
    body.and_then(|v| v.get("error"))
        .and_then(|e| e.get("errors"))
        .and_then(Value::as_array)
        .is_some_and(|errors| {
            errors.iter().take(20).filter_map(|e| e.get("reason").and_then(Value::as_str)).any(|r| {
                r.eq_ignore_ascii_case("rateLimitExceeded") || r.eq_ignore_ascii_case("userRateLimitExceeded")
            })
        })
}

/// An answer's status as MailALM's error, from its JSON error when it has one.
fn check(resp: &Response) -> Result<()> {
    if resp.ok() {
        return Ok(());
    }
    status_error(resp.status, serde_json::from_slice::<Value>(&resp.body).ok().as_ref())
}

/// What Google's error says: its message, else its status, else its first reason; clipped.
pub(crate) fn error_detail(body: Option<&Value>) -> String {
    let error = body.and_then(|v| v.get("error"));
    let text = |k: &str| error.and_then(|e| e.get(k)).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let reason = || {
        error.and_then(|e| e.get("errors")).and_then(Value::as_array).and_then(|a| a.first()).and_then(|e| e.get("reason")).and_then(Value::as_str)
    };
    clip(text("message").or_else(|| text("status")).or_else(reason).unwrap_or_default())
}

/// A status (and its JSON error) as MailALM's error: 401 is the token refused (it ran out or was
/// revoked); busy and failing are tried again later; anything else is a refusal.
fn status_error(status: u16, body: Option<&Value>) -> Result<()> {
    let detail = error_detail(body);
    let with = |what: String| if detail.is_empty() { what } else { format!("{what}: {detail}") };
    match status {
        200..=299 => Ok(()),
        401 => Err(NetError::Auth(with("Google refused the access token (it ran out or was revoked)".into()))),
        429 | 500..=599 => Err(NetError::Protocol(with(format!("Gmail is busy (HTTP {status}); try again later")))),
        403 if rate_limited(body) => Err(NetError::Protocol(with("Gmail is busy (HTTP 403, too many requests); try again later".into()))),
        s => Err(NetError::Server(with(format!("HTTP {s}")))),
    }
}

/// Seconds to wait before attempt `round + 1`: a `Retry-After` in seconds if there is one, else
/// 2 s per round so far; between 1 and [`BUSY_WAIT_CAP`].
pub(crate) fn backoff_secs(hint: Option<&str>, round: u32) -> u64 {
    let fallback = 2 * u64::from(round.max(1));
    hint.and_then(|h| h.trim().parse().ok()).unwrap_or(fallback).clamp(1, BUSY_WAIT_CAP)
}
