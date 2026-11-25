// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/exchange/graph.rs. MIT OR Apache-2.0.
// Changed: MailALM's API (folders with their paths, delta pages, MIME, marks, moves, deletions, junk reports, sending); changes by delta query instead of listings; Graph's own links only; one kept connection; batch entries judged one by one; 410 and expired sync states as a resync; calendars, To Do and contacts in graph/pim.rs (Graph recurrences adapted from MailCraft's graph_pim.rs, the rest MailALM's own); no search, automatic replies, folder changes or imports.

//! Outlook.com mail through Microsoft Graph (v1.0), over HTTPS with an OAuth bearer token
//! ([`crate::oauth`]): the signed-in user, the mail folders, a folder's changes by delta query,
//! a message's MIME source, marks (read, flagged), moves, deletions, junk reports, and sending a
//! MIME message; and the calendars, Microsoft To Do and the contacts (`pim.rs`).
//!
//! Ids are asked for in their immutable form (`Prefer: IdType="ImmutableId"`), so a message keeps
//! its id when it moves. Several small changes go as one `$batch`. Everything Graph sends is
//! hostile input: sizes are capped, text is cleaned, and the links it gives to follow are
//! followed only when they lead back to Graph itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use mailalm_model::{Folder, FolderRole, Summary};
use serde_json::{Value, json};

use crate::http::{self, Agent, Request, Response};
use crate::{NetError, Result, clip};

mod convert;
mod pim;

#[cfg(test)]
mod tests;

/// Microsoft Graph's address.
pub const GRAPH_URL: &str = "https://graph.microsoft.com/v1.0";
/// Largest JSON answer accepted.
const MAX_JSON: usize = 32 << 20;
/// Largest message source downloaded, or sent.
const MAX_MIME: usize = 32 << 20;
/// Requests per `$batch` (Graph's own limit).
const BATCH: usize = 20;
/// Attempts per request (or batch entry) while Graph answers that it is busy.
const BUSY_ROUNDS: u32 = 4;
/// Longest single wait in seconds, whatever `Retry-After` says.
const BUSY_WAIT_CAP: u64 = 20;
/// The answers that mean "busy, try again after a while" (throttled, unavailable, timed out).
const BUSY: [u16; 3] = [429, 503, 504];
/// Pages followed for one listing.
const MAX_PAGES: usize = 200;
/// Folders listed at most, and how deep.
const MAX_FOLDERS: usize = 5000;
const MAX_DEPTH: usize = 32;
/// Entries taken from one delta page (MailALM asks for 200).
const MAX_DELTA_PAGE: usize = 1000;
/// Longest link (nextLink, deltaLink) followed.
const MAX_LINK: usize = 16 << 10;
const FOLDER_FIELDS: &str = "id,displayName,parentFolderId,childFolderCount,totalItemCount,unreadItemCount";
const DELTA_FIELDS: &str =
    "id,subject,from,sender,toRecipients,ccRecipients,receivedDateTime,sentDateTime,isRead,flag,isDraft,hasAttachments,bodyPreview,internetMessageId";
/// Sent on every call, and inside each `$batch` entry, where the outer header doesn't count.
const PREFER: &str = "IdType=\"ImmutableId\"";
const PREFER_DELTA: &str = "IdType=\"ImmutableId\", odata.maxpagesize=200";
/// Graph's well-known folders MailALM gives a role to.
const ROLES: [(&str, FolderRole); 6] = [
    ("inbox", FolderRole::Inbox),
    ("drafts", FolderRole::Drafts),
    ("sentitems", FolderRole::Sent),
    ("deleteditems", FolderRole::Trash),
    ("junkemail", FolderRole::Junk),
    ("archive", FolderRole::Archive),
];

/// The signed-in user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    /// The mailbox's address (`mail`, else `userPrincipalName`).
    pub address: String,
    pub name: String,
}

/// One page of a folder's changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeltaPage {
    /// Messages new to the folder or changed in it.
    pub changed: Vec<GraphMessage>,
    /// Ids of messages gone from the folder (deleted, or moved out).
    pub removed: Vec<String>,
    pub next: DeltaNext,
}

/// Where the pass goes on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeltaNext {
    /// More pages: ask for this link (Graph's nextLink) now.
    More(String),
    /// The pass is over: keep this link (Graph's deltaLink) to ask for the next changes.
    Done(String),
}

/// A message as a delta page has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphMessage {
    /// Its immutable id.
    pub id: String,
    /// What the message list needs (its `uid` is 0: the engine numbers messages).
    pub summary: Summary,
}

/// One request of a `$batch`: the method, the URL from the version on (`/me/messages/…`), a
/// JSON body.
struct Call {
    method: &'static str,
    url: String,
    body: Option<Value>,
}

/// A Graph session: the access token, and one connection kept open. Its `Debug` shows no token.
pub struct GraphClient {
    base: String,
    token: String,
    agent: Agent,
}

impl std::fmt::Debug for GraphClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GraphClient(<redacted>)")
    }
}

impl GraphClient {
    /// A session with this access token. Nothing is sent yet.
    pub fn new(access_token: &str) -> GraphClient {
        GraphClient::at(GRAPH_URL, access_token)
    }

    /// A session with a test server for Graph (plain http to this computer is let through in tests).
    #[cfg(test)]
    pub(crate) fn with_base(base: &str, access_token: &str) -> GraphClient {
        GraphClient::at(base, access_token)
    }

    fn at(base: &str, access_token: &str) -> GraphClient {
        GraphClient { base: base.trim_end_matches('/').to_string(), token: access_token.to_string(), agent: Agent::default() }
    }

    /// Carry a new access token from now on (the old one ran out).
    pub fn set_token(&mut self, access_token: &str) {
        self.token = access_token.to_string();
    }

    /// The signed-in user: `mail`, else `userPrincipalName`; and `displayName`.
    pub fn me(&mut self) -> Result<Me> {
        let v = self.get_json(&format!("{}/me?$select=mail,userPrincipalName,displayName", self.base))?;
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(|s| convert::clean_text(s, 320)).filter(|s| !s.is_empty());
        let address = text("mail")
            .or_else(|| text("userPrincipalName"))
            .filter(|a| a.contains('@') && !a.contains(char::is_whitespace))
            .ok_or_else(|| NetError::Protocol("Microsoft Graph didn't say which mailbox this is".into()))?;
        let name = v.get("displayName").and_then(Value::as_str).map(|s| convert::clean_text(s, 300)).unwrap_or_default();
        Ok(Me { address, name })
    }

    /// Every mail folder (through each folder's children), as MailALM's model has them: the path
    /// is the display names from the top joined with `/`, the wire name the folder's id; the
    /// well-known folders get their roles. At most [`MAX_FOLDERS`], [`MAX_DEPTH`] deep.
    pub fn folders(&mut self) -> Result<Vec<Folder>> {
        let role_of = self.roles()?;
        let mut out: Vec<Folder> = Vec::new();
        let mut ids: HashSet<String> = HashSet::new();
        let mut paths: HashSet<String> = HashSet::new();
        let top = self.all(&format!("{}/me/mailFolders?$top=100&$select={FOLDER_FIELDS}", self.base), MAX_FOLDERS)?;
        // Each folder, with its parent's path and its depth.
        let mut queue: VecDeque<(Value, String, usize)> = top.into_iter().map(|f| (f, String::new(), 0)).collect();
        while let Some((f, parent, depth)) = queue.pop_front() {
            if out.len() >= MAX_FOLDERS {
                break;
            }
            let Some(id) = f.get("id").and_then(Value::as_str).filter(|id| convert::id_ok(id)) else { continue };
            if !ids.insert(id.to_string()) {
                continue;
            }
            let name = convert::folder_name(f.get("displayName").and_then(Value::as_str).unwrap_or_default());
            let path = unique(if parent.is_empty() { name } else { format!("{parent}/{name}") }, &paths);
            paths.insert(path.clone());
            let children = f.get("childFolderCount").and_then(Value::as_u64).unwrap_or(0);
            if children > 0 && depth + 1 < MAX_DEPTH && out.len() + queue.len() < MAX_FOLDERS {
                let left = MAX_FOLDERS.saturating_sub(out.len() + queue.len());
                let url = format!("{}/me/mailFolders/{}/childFolders?$top=100&$select={FOLDER_FIELDS}", self.base, seg(id));
                for child in self.all(&url, left)? {
                    queue.push_back((child, path.clone(), depth + 1));
                }
            }
            let count = |k: &str| f.get(k).and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
            out.push(Folder {
                role: role_of.get(id).copied().unwrap_or(FolderRole::Other),
                total: count("totalItemCount"),
                unread: count("unreadItemCount"),
                path,
                wire: id.to_string(),
                delimiter: Some('/'),
            });
        }
        mailalm_model::sort_folders(&mut out);
        Ok(out)
    }

    /// The ids of the well-known folders, with their roles (one `$batch`); a folder the mailbox
    /// doesn't have is left out.
    fn roles(&mut self) -> Result<HashMap<String, FolderRole>> {
        let calls: Vec<Call> =
            ROLES.iter().map(|(name, _)| Call { method: "GET", url: format!("/me/mailFolders/{name}?$select=id"), body: None }).collect();
        let mut role_of = HashMap::new();
        for ((_, role), (status, body)) in ROLES.iter().zip(self.batch(&calls)?) {
            if status == 401 {
                return Err(status_error(status, Some(&body)).err().unwrap_or_else(|| NetError::Auth("refused".into())));
            }
            if let Some(id) = body.get("id").and_then(Value::as_str).filter(|id| status == 200 && convert::id_ok(id)) {
                role_of.insert(id.to_string(), *role);
            }
        }
        Ok(role_of)
    }

    /// One page of a folder's changes. `link`: `None` starts a full pass (every message in the
    /// folder, newest first); else the nextLink or deltaLink a page gave before. An expired or
    /// unknown deltaLink is [`NetError::Resync`]: start again with `None`.
    pub fn delta(&mut self, folder_id: &str, link: Option<&str>) -> Result<DeltaPage> {
        let url = match link {
            None => {
                if !convert::id_ok(folder_id) {
                    return Err(NetError::Invalid("not a folder id".into()));
                }
                format!(
                    "{}/me/mailFolders/{}/messages/delta?$select={DELTA_FIELDS}&$orderby=receivedDateTime%20desc",
                    self.base,
                    seg(folder_id)
                )
            }
            Some(l) if self.ours(l) => l.to_string(),
            Some(_) => return Err(NetError::Protocol("a delta link that doesn't lead to Microsoft Graph".into())),
        };
        let resp = self.call(Request::get(&url, MAX_JSON), "application/json", PREFER_DELTA)?;
        let body: Option<Value> = serde_json::from_slice(&resp.body).ok();
        if resp.status == 410 || (!resp.ok() && convert::error_codes(body.as_ref()).iter().any(|c| resync_code(c))) {
            return Err(self.hide(NetError::Resync(format!("HTTP {} {}", resp.status, error_detail(body.as_ref())).trim().to_string())));
        }
        check(&resp).map_err(|e| self.hide(e))?;
        let v = body.ok_or_else(|| NetError::Protocol("Microsoft Graph sent a delta page that isn't JSON".into()))?;
        let items = v.get("value").and_then(Value::as_array).ok_or_else(|| NetError::Protocol("a delta page without its list".into()))?;
        if items.len() > MAX_DELTA_PAGE {
            return Err(NetError::TooLarge(format!("a delta page of {} messages", items.len())));
        }
        let mut page = DeltaPage { changed: Vec::new(), removed: Vec::new(), next: DeltaNext::Done(String::new()) };
        for m in items {
            let Some(id) = m.get("id").and_then(Value::as_str).filter(|id| convert::id_ok(id)) else { continue };
            if m.get("@removed").is_some() {
                page.removed.push(id.to_string());
            } else {
                page.changed.push(GraphMessage { id: id.to_string(), summary: convert::summary_of(m) });
            }
        }
        let next = v.get("@odata.nextLink").and_then(Value::as_str);
        let delta = v.get("@odata.deltaLink").and_then(Value::as_str);
        page.next = match (next, delta) {
            (Some(next), _) if self.ours(next) => DeltaNext::More(next.to_string()),
            (None, Some(delta)) if self.ours(delta) => DeltaNext::Done(delta.to_string()),
            (None, None) => return Err(NetError::Protocol("a delta page with neither a next link nor a delta link".into())),
            _ => return Err(NetError::Protocol("a link that doesn't lead back to Microsoft Graph".into())),
        };
        Ok(page)
    }

    /// The message as MIME (`/me/messages/{id}/$value`); `Ok(None)` when it is gone (404).
    pub fn download(&mut self, id: &str) -> Result<Option<Vec<u8>>> {
        if !convert::id_ok(id) {
            return Err(NetError::Invalid("not a message id".into()));
        }
        let url = format!("{}/me/messages/{}/$value", self.base, seg(id));
        let resp = self.call(Request::get(&url, MAX_MIME), "*/*", PREFER)?;
        if resp.status == 404 {
            return Ok(None);
        }
        check(&resp).map_err(|e| self.hide(e))?;
        Ok(Some(resp.body))
    }

    /// Mark messages read or unread.
    pub fn set_read(&mut self, ids: &[String], read: bool) -> Result<()> {
        self.each(ids, false, |id| Call { method: "PATCH", url: format!("/me/messages/{}", seg(id)), body: Some(json!({"isRead": read})) })
    }

    /// Flag messages, or take their flag off.
    pub fn set_flagged(&mut self, ids: &[String], flagged: bool) -> Result<()> {
        let status = if flagged { "flagged" } else { "notFlagged" };
        self.each(ids, false, |id| Call { method: "PATCH", url: format!("/me/messages/{}", seg(id)), body: Some(json!({"flag": {"flagStatus": status}})) })
    }

    /// Move messages to another folder. Nothing is returned: immutable ids stay the same across
    /// moves. A message already gone counts as done.
    pub fn move_to(&mut self, ids: &[String], folder_id: &str) -> Result<()> {
        if !convert::id_ok(folder_id) {
            return Err(NetError::Invalid("not a folder id".into()));
        }
        self.each(ids, true, |id| Call {
            method: "POST",
            url: format!("/me/messages/{}/move", seg(id)),
            body: Some(json!({"destinationId": folder_id})),
        })
    }

    /// Delete messages (Graph's DELETE: out of the folder, into the mailbox's Recoverable Items).
    /// A message already gone counts as done.
    pub fn delete(&mut self, ids: &[String]) -> Result<()> {
        self.each(ids, true, |id| Call { method: "DELETE", url: format!("/me/messages/{}", seg(id)), body: None })
    }

    /// Report messages as junk (moved to the Junk Email folder) or as not junk (moved back out of
    /// it, to the Inbox), which trains Outlook.com's filter.
    ///
    /// This is v1.0's `reportMessage`. The `markAsJunk` and `markAsNotJunk` actions the plan
    /// named were only ever in Graph's beta, and Microsoft deprecated them for `reportMessage`
    /// (they stopped answering on 2025-12-30). `reportMessage`'s documentation doesn't say that
    /// it blocks or unblocks the sender, as `markAsJunk` did.
    pub fn mark_junk(&mut self, ids: &[String], junk: bool) -> Result<()> {
        let action = if junk { "junk" } else { "notJunk" };
        self.each(ids, true, |id| Call {
            method: "POST",
            url: format!("/me/messages/{}/reportMessage", seg(id)),
            body: Some(json!({"IsMessageMoveRequested": true, "ReportAction": action})),
        })
    }

    /// Send a MIME message (`POST /me/sendMail`, its source in base64 as `text/plain`). Graph
    /// keeps the copy in Sent Items itself. Sent on a connection of its own and never sent again
    /// once it went out: an answer that doesn't come, or a time-out, is
    /// [`NetError::Unconfirmed`] (it may have been sent).
    pub fn send_mime(&mut self, raw: &[u8]) -> Result<()> {
        if raw.is_empty() {
            return Err(NetError::Invalid("an empty message".into()));
        }
        if raw.len() > MAX_MIME {
            return Err(NetError::TooLarge(format!("the message is larger than {} MB", MAX_MIME >> 20)));
        }
        self.agent.close();
        let req = Request::post(&format!("{}/me/sendMail", self.base), "text/plain", crate::base64::encode(raw).into_bytes(), MAX_JSON);
        // Throttled or unavailable: nothing was done, it can go again. Timed out at the gateway:
        // it may have gone.
        match self.exchange(req, "application/json", PREFER, &[429, 503], false) {
            Err(f) if f.sent => Err(NetError::Unconfirmed(self.hide(f.error).to_string())),
            Err(f) => Err(self.hide(f.error)),
            Ok(resp) if resp.status == 504 => Err(NetError::Unconfirmed("Microsoft Graph timed out (HTTP 504)".into())),
            Ok(resp) => check(&resp).map_err(|e| self.hide(e)),
        }
    }

    /// The same change for each message, through `$batch`. `gone_ok`: a message that is no
    /// longer there (404) counts as done.
    fn each(&mut self, ids: &[String], gone_ok: bool, call: impl Fn(&str) -> Call) -> Result<()> {
        if let Some(bad) = ids.iter().find(|id| !convert::id_ok(id)) {
            return Err(NetError::Invalid(format!("not a message id: {}", clip(bad))));
        }
        if ids.is_empty() {
            return Ok(());
        }
        let calls: Vec<Call> = ids.iter().map(|id| call(id)).collect();
        let answers = self.batch(&calls)?;
        let (mut refused, mut busy) = (0usize, 0usize);
        let mut first: Option<String> = None;
        for (status, body) in &answers {
            match *status {
                200..=299 => {}
                404 if gone_ok => {}
                401 => return Err(self.hide(status_error(401, Some(body)).err().unwrap_or_else(|| NetError::Auth("refused".into())))),
                s if BUSY.contains(&s) || s >= 500 => busy += 1,
                s => {
                    refused += 1;
                    if first.is_none() {
                        first = Some(format!("HTTP {s} {}", error_detail(Some(body))).trim().to_string());
                    }
                }
            }
        }
        let total = answers.len();
        if busy > 0 {
            // Tried again later, whole: the rest is done already, and doing it again is harmless.
            return Err(NetError::Protocol(format!("Microsoft Graph was too busy for {busy} of {total} changes")));
        }
        if refused > 0 {
            return Err(self.hide(NetError::Server(format!("{refused} of {total} changes were refused: {}", first.unwrap_or_default()))));
        }
        Ok(())
    }

    /// Run requests through `$batch`, [`BATCH`] at a time: (status, body) per request, in order.
    /// Entries Graph was too busy for go again on their own after the longest `Retry-After` among
    /// them, up to [`BUSY_ROUNDS`] times; one still busy then is returned as such.
    fn batch(&mut self, calls: &[Call]) -> Result<Vec<(u16, Value)>> {
        let mut out = Vec::with_capacity(calls.len());
        for chunk in calls.chunks(BATCH) {
            let mut results: Vec<Option<(u16, Value)>> = vec![None; chunk.len()];
            let mut todo: Vec<usize> = (0..chunk.len()).collect();
            let mut round = 0;
            while !todo.is_empty() && round < BUSY_ROUNDS {
                round += 1;
                let entries: Vec<Value> = todo.iter().filter_map(|&i| chunk.get(i).map(|c| batch_entry(i, c))).collect();
                let body = serde_json::to_vec(&json!({ "requests": entries })).map_err(|e| NetError::Invalid(e.to_string()))?;
                let reply = self.post_json(&format!("{}/$batch", self.base), body)?;
                let by_id: HashMap<&str, &Value> = reply
                    .get("responses")
                    .and_then(Value::as_array)
                    .map(|rs| rs.iter().take(BATCH * 2).filter_map(|r| Some((r.get("id")?.as_str()?, r))).collect())
                    .unwrap_or_default();
                let mut busy = Vec::new();
                let mut pause = 0;
                for &i in &todo {
                    let unanswered = || NetError::Protocol("Microsoft Graph left part of a batch unanswered".into());
                    let r = by_id.get((i + 1).to_string().as_str()).copied().ok_or_else(unanswered)?;
                    let status =
                        r.get("status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok()).filter(|s| (100..600).contains(s)).ok_or_else(unanswered)?;
                    if BUSY.contains(&status) {
                        busy.push(i);
                        pause = pause.max(backoff_secs(entry_retry_after(r).as_deref(), round));
                    }
                    if let Some(slot) = results.get_mut(i) {
                        *slot = Some((status, r.get("body").cloned().unwrap_or(Value::Null)));
                    }
                }
                todo = busy;
                if !todo.is_empty() && round < BUSY_ROUNDS {
                    std::thread::sleep(Duration::from_secs(pause));
                }
            }
            out.extend(results.into_iter().map(|r| r.unwrap_or((429, Value::Null))));
        }
        Ok(out)
    }

    /// Every item of a listing, following its nextLinks (to Graph only), at most `limit`.
    fn all(&mut self, first: &str, limit: usize) -> Result<Vec<Value>> {
        self.all_with(first, limit, PREFER)
    }

    /// [`GraphClient::all`], each page asked for with this `Prefer` header.
    fn all_with(&mut self, first: &str, limit: usize, prefer: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut url = first.to_string();
        for _ in 0..MAX_PAGES {
            let v = self.get_json_with(&url, prefer)?;
            let items = v.get("value").and_then(Value::as_array).ok_or_else(|| NetError::Protocol("a Graph listing without its list".into()))?;
            out.extend(items.iter().take(limit.saturating_sub(out.len())).cloned());
            if out.len() >= limit {
                break;
            }
            match v.get("@odata.nextLink").and_then(Value::as_str) {
                Some(next) if self.ours(next) => url = next.to_string(),
                Some(_) => return Err(NetError::Protocol("a link that doesn't lead back to Microsoft Graph".into())),
                None => break,
            }
        }
        Ok(out)
    }

    fn get_json(&mut self, url: &str) -> Result<Value> {
        self.get_json_with(url, PREFER)
    }

    fn get_json_with(&mut self, url: &str, prefer: &str) -> Result<Value> {
        let resp = self.call(Request::get(url, MAX_JSON), "application/json", prefer)?;
        check(&resp).map_err(|e| self.hide(e))?;
        json_of(&resp)
    }

    fn post_json(&mut self, url: &str, body: Vec<u8>) -> Result<Value> {
        let resp = self.call(Request::post(url, "application/json", body, MAX_JSON), "application/json", PREFER)?;
        check(&resp).map_err(|e| self.hide(e))?;
        json_of(&resp)
    }

    /// Whether `link` leads back to Graph (in tests, to the test server): only such links are
    /// followed.
    fn ours(&self, link: &str) -> bool {
        link.len() <= MAX_LINK
            && link.strip_prefix(self.base.as_str()).is_some_and(|rest| rest.starts_with('/'))
            && !link.chars().any(|c| c.is_whitespace() || c.is_control())
    }

    /// A request as every Graph call sends it, through the limits: its answer, whatever its status.
    fn call(&mut self, req: Request, accept: &str, prefer: &str) -> Result<Response> {
        self.exchange(req, accept, prefer, &BUSY, true).map_err(|f| self.hide(f.error))
    }

    /// `req` with the token, `Accept` and `Prefer`, sent again while its answer is one of `busy`
    /// (after `Retry-After`, up to [`BUSY_ROUNDS`] attempts). `again`: whether a kept connection
    /// found closed may carry it once more on a new one.
    fn exchange(&mut self, req: Request, accept: &str, prefer: &str, busy: &[u16], again: bool) -> std::result::Result<Response, http::Failure> {
        let req = req.header("Authorization", &format!("Bearer {}", self.token)).header("Accept", accept).header("Prefer", prefer);
        let mut round = 0;
        loop {
            round += 1;
            let resp = self.agent.send(&req, again)?;
            if !busy.contains(&resp.status) || round >= BUSY_ROUNDS {
                return Ok(resp);
            }
            log::debug!("Microsoft Graph is busy (HTTP {}): waiting", resp.status);
            std::thread::sleep(Duration::from_secs(backoff_secs(resp.header("Retry-After"), round)));
        }
    }

    /// The error with the access token blanked out of its text.
    fn hide(&self, e: NetError) -> NetError {
        e.without(&[self.token.as_str()])
    }
}

/// A path not yet taken: `path`, else `path (2)`, `path (3)`…
fn unique(path: String, taken: &HashSet<String>) -> String {
    if !taken.contains(&path) {
        return path;
    }
    (2..=MAX_FOLDERS + 1).map(|n| format!("{path} ({n})")).find(|p| !taken.contains(p)).unwrap_or(path)
}

/// An id as a URL path segment.
fn seg(id: &str) -> String {
    http::pct(id)
}

fn json_of(resp: &Response) -> Result<Value> {
    if resp.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&resp.body).map_err(|_| NetError::Protocol(format!("Microsoft Graph sent JSON that can't be read (HTTP {})", resp.status)))
}

/// An answer's status as MailALM's error, from its JSON error when it has one.
fn check(resp: &Response) -> Result<()> {
    if resp.ok() {
        return Ok(());
    }
    status_error(resp.status, serde_json::from_slice::<Value>(&resp.body).ok().as_ref())
}

/// What Graph's error says: its message, else its code; clipped.
fn error_detail(body: Option<&Value>) -> String {
    let error = body.and_then(|v| v.get("error"));
    let message = error.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or_default();
    let code = error.and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or_default();
    clip(if message.trim().is_empty() { code } else { message })
}

/// A status (and its JSON error) as MailALM's error: 401 is the token refused (it ran out or was
/// revoked); busy and failing are tried again later; anything else is a refusal.
fn status_error(status: u16, body: Option<&Value>) -> Result<()> {
    let detail = error_detail(body);
    let with = |what: String| if detail.is_empty() { what } else { format!("{what}: {detail}") };
    match status {
        200..=299 => Ok(()),
        401 => Err(NetError::Auth(with("Microsoft refused the access token (it ran out or was revoked)".into()))),
        429 | 503 | 504 => Err(NetError::Protocol(format!("Microsoft Graph is busy (HTTP {status}); try again later"))),
        500..=599 => Err(NetError::Protocol(with(format!("Microsoft Graph failed (HTTP {status})")))),
        s => Err(NetError::Server(with(format!("HTTP {s}")))),
    }
}

/// An error code that says a delta link is no good any more (Graph's documentation names
/// `syncStateNotFound`; the others are Exchange's and other Graph services' words for it).
fn resync_code(code: &str) -> bool {
    ["syncStateNotFound", "syncStateInvalid", "resyncRequired", "resyncApplyDifferences", "resyncUploadDifferences"]
        .iter()
        .any(|c| c.eq_ignore_ascii_case(code))
}

/// One `$batch` entry: numbered from 1, with the [`PREFER`] header (and a JSON body if any).
fn batch_entry(i: usize, call: &Call) -> Value {
    let mut headers = serde_json::Map::new();
    headers.insert("Prefer".into(), json!(PREFER));
    let mut e = json!({"id": (i + 1).to_string(), "method": call.method, "url": call.url});
    if let Some(b) = &call.body {
        headers.insert("Content-Type".into(), json!("application/json"));
        e["body"] = b.clone();
    }
    e["headers"] = Value::Object(headers);
    e
}

/// A batch entry's `Retry-After`, whatever case the key has and whether it's a string or number.
fn entry_retry_after(r: &Value) -> Option<String> {
    let headers = r.get("headers")?.as_object()?;
    match headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("Retry-After"))?.1 {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Seconds to wait before attempt `round + 1`: a `Retry-After` in seconds if there is one, else
/// 2 s per round so far; between 1 and [`BUSY_WAIT_CAP`].
fn backoff_secs(hint: Option<&str>, round: u32) -> u64 {
    let fallback = 2 * u64::from(round.max(1));
    hint.and_then(|h| h.trim().parse().ok()).unwrap_or(fallback).clamp(1, BUSY_WAIT_CAP)
}
