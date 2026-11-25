//! MailALM's own: Outlook.com's inbox rules through Microsoft Graph (`messageRules` of the Inbox,
//! v1.0), as MailALM's rules ([`mailalm_model::rules`]); the search of the mailbox (`$search`);
//! and making a folder (`MailALM/Snoozed`).
//!
//! A rule's folder is Graph's folder id; MailALM's is a path: the folder list gives one for the
//! other. A rule with something the model can't say (an exception, a copy, an importance, a
//! redirection…) is read with that listed in `unsupported`, and only ever deleted. Graph's
//! `fromAddresses` and `sentToAddresses` are read as "from" and "to" conditions on those
//! addresses, written back as `senderContains` and `recipientContains`.
//!
//! Everything Graph sends is hostile input: ids checked, texts cleaned and capped, lists capped.

use mailalm_model::query::{Term, civil_from_days};
use mailalm_model::rules::{Action, Condition, Host, MAX_TEXT, MAX_WORDS, Rule, Where};
use mailalm_model::{Folder, Query};
use serde_json::{Map, Value, json};

use super::convert::{self, clean_text, id_ok};
use super::{DELTA_FIELDS, GraphClient, GraphMessage, MAX_JSON, PREFER, check, json_of, seg};
use crate::http::{self, Request};
use crate::{NetError, Result, clip};

#[cfg(test)]
mod tests;

/// Most rules read.
const MAX_RULES: usize = 1000;
/// Most keys of a rule's conditions or actions looked at.
const MAX_KEYS: usize = 64;
/// Most messages a search gives.
const MAX_FOUND: usize = 250;
/// The longest search sent, in characters.
const MAX_SEARCH: usize = 400;

/// A message the server's search found, and the id of the folder it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub folder_id: String,
    pub message: GraphMessage,
}

/// Whether a value says nothing: absent in effect (null, false, zero, empty).
fn says_nothing(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_u64() == Some(0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.values().all(says_nothing),
    }
}

/// A key of Graph's, as `unsupported` says it: letters and digits, clipped.
fn key_name(k: &str) -> String {
    k.chars().filter(char::is_ascii_alphanumeric).take(60).collect()
}

/// A list of words (strings), cleaned; `None` when it isn't one, or is longer than a condition
/// takes.
fn words(v: &Value) -> Option<Vec<String>> {
    let a = v.as_array().filter(|a| a.len() <= MAX_WORDS)?;
    let mut out = Vec::new();
    for s in a {
        let w = clean_text(s.as_str()?, MAX_TEXT);
        if !w.is_empty() {
            out.push(w);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A list of recipients' addresses (`[{"emailAddress": {"address"}}]`); `None` when it isn't one.
fn addresses(v: &Value) -> Option<Vec<String>> {
    let a = v.as_array().filter(|a| a.len() <= MAX_WORDS)?;
    let mut out = Vec::new();
    for r in a {
        let address = clean_text(r.get("emailAddress")?.get("address")?.as_str()?, 320);
        if !address.is_empty() {
            out.push(address);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A boolean as Graph sends it (a string `"true"` too).
fn truth(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A Graph `messageRule` as MailALM's rule; `None` without a usable id.
pub(super) fn rule_of(v: &Value, folders: &[Folder]) -> Option<Rule> {
    let id = v.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let name = clean_text(v.get("displayName").and_then(Value::as_str).unwrap_or_default(), MAX_TEXT);
    let mut rule = Rule {
        id: id.to_owned(),
        name: if name.is_empty() { "(unnamed)".to_owned() } else { name },
        place: Where::Server,
        enabled: v.get("isEnabled").is_none_or(truth),
        conditions: Vec::new(),
        actions: Vec::new(),
        stop: false,
        forward_confirmed: false,
        unsupported: Vec::new(),
    };
    if v.get("isReadOnly").is_some_and(truth) {
        rule.unsupported.push("Outlook keeps it read only".to_owned());
    }
    if v.get("exceptions").is_some_and(|e| !says_nothing(e)) {
        rule.unsupported.push("exceptions".to_owned());
    }
    let empty = Map::new();
    let conditions = v.get("conditions").and_then(Value::as_object).unwrap_or(&empty);
    for (k, val) in conditions.iter().take(MAX_KEYS) {
        if says_nothing(val) {
            continue;
        }
        let c = match k.as_str() {
            "senderContains" => words(val).map(Condition::From),
            "fromAddresses" => addresses(val).map(Condition::From),
            "recipientContains" => words(val).map(Condition::To),
            "sentToAddresses" => addresses(val).map(Condition::To),
            "subjectContains" => words(val).map(Condition::Subject),
            "bodyContains" => words(val).map(Condition::Body),
            "bodyOrSubjectContains" => words(val).map(Condition::Words),
            "hasAttachments" if truth(val) => Some(Condition::HasAttachment),
            _ => None,
        };
        match c {
            Some(c) => rule.conditions.push(c),
            None => rule.unsupported.push(format!("condition {}", key_name(k))),
        }
    }
    let actions = v.get("actions").and_then(Value::as_object).unwrap_or(&empty);
    for (k, val) in actions.iter().take(MAX_KEYS) {
        if says_nothing(val) {
            continue;
        }
        let mut known = true;
        match k.as_str() {
            "markAsRead" if truth(val) => rule.actions.push(Action::MarkRead),
            "delete" if truth(val) => rule.actions.push(Action::Delete),
            "stopProcessingRules" if truth(val) => rule.stop = true,
            "moveToFolder" => match val.as_str().and_then(|id| folders.iter().find(|f| f.wire == id)) {
                Some(f) => rule.actions.push(Action::MoveTo(f.path.clone())),
                None => rule.unsupported.push("moves to a folder MailALM doesn't know".to_owned()),
            },
            "assignCategories" => match words(val) {
                Some(cats) => rule.actions.extend(cats.into_iter().map(Action::Category)),
                None => known = false,
            },
            "forwardTo" => match addresses(val) {
                Some(to) => rule.actions.push(Action::Forward(to)),
                None => known = false,
            },
            _ => known = false,
        }
        if !known {
            rule.unsupported.push(format!("action {}", key_name(k)));
        }
    }
    Some(rule)
}

/// A rule as Graph's `messageRule` (with its `sequence` when given). The rule is checked first
/// ([`Rule::check`]); a folder MailALM doesn't know, or two conditions of a kind Graph has once,
/// is refused.
pub(super) fn rule_json(rule: &Rule, folders: &[Folder], sequence: Option<u64>) -> Result<Value> {
    rule.check(Host::Outlook).map_err(NetError::Invalid)?;
    let mut conditions = Map::new();
    for c in &rule.conditions {
        let (key, value) = match c {
            Condition::From(w) => ("senderContains", json!(w)),
            Condition::To(w) => ("recipientContains", json!(w)),
            Condition::Subject(w) => ("subjectContains", json!(w)),
            Condition::Body(w) => ("bodyContains", json!(w)),
            Condition::Words(w) => ("bodyOrSubjectContains", json!(w)),
            Condition::HasAttachment => ("hasAttachments", json!(true)),
        };
        if conditions.insert(key.to_owned(), value).is_some() {
            return Err(NetError::Invalid("Outlook takes one condition of each kind: put the words together in one".into()));
        }
    }
    let mut actions = Map::new();
    let mut categories: Vec<&str> = Vec::new();
    let mut forward: Vec<Value> = Vec::new();
    for a in &rule.actions {
        match a {
            Action::MoveTo(path) => {
                let f = folders.iter().find(|f| f.path == *path).ok_or_else(|| NetError::Invalid(format!("the folder {} no longer exists", clip(path))))?;
                actions.insert("moveToFolder".into(), json!(f.wire));
            }
            Action::MarkRead => {
                actions.insert("markAsRead".into(), json!(true));
            }
            Action::Delete => {
                actions.insert("delete".into(), json!(true));
            }
            Action::Category(c) => categories.push(c),
            Action::Forward(to) => forward.extend(to.iter().map(|a| json!({"emailAddress": {"address": a}}))),
            // Refused by the check above.
            Action::Flag => return Err(NetError::Invalid("Outlook's server rules can't flag a message".into())),
        }
    }
    if !categories.is_empty() {
        actions.insert("assignCategories".into(), json!(categories));
    }
    if !forward.is_empty() {
        actions.insert("forwardTo".into(), Value::Array(forward));
    }
    actions.insert("stopProcessingRules".into(), json!(rule.stop));
    let mut v = json!({
        "displayName": rule.name,
        "isEnabled": rule.enabled,
        "conditions": Value::Object(conditions),
        "actions": Value::Object(actions),
    });
    if let Some(n) = sequence {
        v["sequence"] = json!(n);
    }
    Ok(v)
}

/// The search box's query as KQL for Graph's `$search`: words, `from:`, `to:`, `subject:`,
/// `hasattachments:true`, `received` dates. Values keep only letters, digits and `@ . - _ + '`
/// (anything else splits them into words): nothing can close the quotes or add an operator.
/// `None` when nothing in it is for the server.
pub(super) fn kql(query: &Query) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut add = |prefix: &str, w: &str| {
        for piece in w.split(|c: char| !(c.is_alphanumeric() || "@.-_+'".contains(c))).filter(|p| !p.is_empty()) {
            parts.push(format!("{prefix}{piece}"));
        }
    };
    let date = |t: i64| {
        let (y, m, d) = civil_from_days(t.div_euclid(86_400));
        format!("{y:04}-{m:02}-{d:02}")
    };
    for t in &query.terms {
        match t {
            Term::Text(w) => add("", w),
            Term::From(w) => add("from:", w),
            Term::To(w) => add("to:", w),
            Term::Subject(w) => add("subject:", w),
            Term::HasAttachment => add("hasattachments:", "true"),
            Term::Before(d) => add("received<", &date(*d)),
            Term::After(d) => add("received>=", &date(*d)),
            Term::Unread | Term::Read | Term::Flagged | Term::In(_) | Term::Account(_) | Term::Category(_) => {}
        }
    }
    let mut out = String::new();
    for p in parts {
        if out.len() + p.len() + 1 > MAX_SEARCH {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&p);
    }
    (!out.is_empty()).then_some(out)
}

impl GraphClient {
    /// The Inbox's rules, in Graph's order (`sequence`), at most [`MAX_RULES`]; `folders` names
    /// the folders they move mail to.
    pub fn rules(&mut self, folders: &[Folder]) -> Result<Vec<Rule>> {
        let mut items = self.rules_raw()?;
        items.sort_by_key(|v| v.get("sequence").and_then(Value::as_u64).unwrap_or(u64::MAX));
        Ok(items.iter().filter_map(|v| rule_of(v, folders)).collect())
    }

    fn rules_raw(&mut self) -> Result<Vec<Value>> {
        self.all(&format!("{}/me/mailFolders/inbox/messageRules", self.base), MAX_RULES)
    }

    /// Make a rule, after the Inbox's others (Graph wants its `sequence`). Not sent again once it
    /// went out, unless Graph turned it away unread. The rule as Graph made it comes back.
    pub fn create_rule(&mut self, rule: &Rule, folders: &[Folder]) -> Result<Rule> {
        rule.check(Host::Outlook).map_err(NetError::Invalid)?;
        let next = self.rules_raw()?.iter().filter_map(|v| v.get("sequence").and_then(Value::as_u64)).max().unwrap_or(0).saturating_add(1);
        let body = rule_json(rule, folders, Some(next))?;
        let url = format!("{}/me/mailFolders/inbox/messageRules", self.base);
        let req = Request::post(&url, "application/json", to_bytes(&body)?, MAX_JSON);
        let resp = self.exchange(req, "application/json", PREFER, &[429, 503], true).map_err(|f| self.hide(f.error))?;
        check(&resp).map_err(|e| self.hide(e))?;
        rule_of(&json_of(&resp)?, folders).ok_or_else(|| NetError::Protocol("Microsoft Graph made the rule but sent it back unreadable".into()))
    }

    /// Change a rule (`rule.id`): its name, whether it is on, its conditions and actions.
    pub fn update_rule(&mut self, rule: &Rule, folders: &[Folder]) -> Result<Rule> {
        let id = checked(&rule.id)?;
        let body = rule_json(rule, folders, None)?;
        let url = format!("{}/me/mailFolders/inbox/messageRules/{}", self.base, seg(id));
        let req = Request { body: to_bytes(&body)?, ..Request::new("PATCH", &url, MAX_JSON) }.header("Content-Type", "application/json");
        let resp = self.call(req, "application/json", PREFER)?;
        check(&resp).map_err(|e| self.hide(e))?;
        rule_of(&json_of(&resp)?, folders).ok_or_else(|| NetError::Protocol("Microsoft Graph changed the rule but sent it back unreadable".into()))
    }

    /// Delete a rule. One already gone counts as done.
    pub fn delete_rule(&mut self, id: &str) -> Result<()> {
        let id = checked(id)?;
        let url = format!("{}/me/mailFolders/inbox/messageRules/{}", self.base, seg(id));
        let resp = self.call(Request::new("DELETE", &url, MAX_JSON), "application/json", PREFER)?;
        if resp.status == 404 {
            return Ok(());
        }
        check(&resp).map_err(|e| self.hide(e))
    }

    /// Make a mail folder, at the top (`parent: None`) or under a folder: its id. A folder of
    /// that name already there is Graph's refusal ([`NetError::Server`], HTTP 409): read the
    /// folders again.
    pub fn create_folder(&mut self, parent: Option<&str>, name: &str) -> Result<String> {
        let name = clean_text(name, MAX_TEXT);
        if name.is_empty() || name.contains('/') {
            return Err(NetError::Invalid("not a folder name".into()));
        }
        let url = match parent {
            None => format!("{}/me/mailFolders", self.base),
            Some(p) => format!("{}/me/mailFolders/{}/childFolders", self.base, seg(checked(p)?)),
        };
        let req = Request::post(&url, "application/json", to_bytes(&json!({"displayName": name}))?, MAX_JSON);
        let resp = self.exchange(req, "application/json", PREFER, &[429, 503], true).map_err(|f| self.hide(f.error))?;
        check(&resp).map_err(|e| self.hide(e))?;
        json_of(&resp)?
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| id_ok(id))
            .map(str::to_owned)
            .ok_or_else(|| NetError::Protocol("Microsoft Graph made the folder but sent no id for it".into()))
    }

    /// Graph's search of the mailbox (`$search`), or of one folder: the messages found, with their
    /// folder's id, at most `limit` (and [`MAX_FOUND`]). [`NetError::Invalid`] (nothing sent) when
    /// nothing in the query is for the server.
    pub fn search(&mut self, query: &Query, folder_id: Option<&str>, limit: usize) -> Result<Vec<Found>> {
        let kql = kql(query).ok_or_else(|| NetError::Invalid("nothing in the search the server can look for".into()))?;
        let scope = match folder_id {
            None => "/me/messages".to_owned(),
            Some(f) => format!("/me/mailFolders/{}/messages", seg(checked(f)?)),
        };
        let url = format!("{}{scope}?$search={}&$top=50&$select={DELTA_FIELDS},parentFolderId", self.base, http::pct(&format!("\"{kql}\"")));
        let items = self.all(&url, limit.min(MAX_FOUND))?;
        Ok(items
            .iter()
            .filter_map(|m| {
                let id = m.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
                let folder_id = m.get("parentFolderId").and_then(Value::as_str).filter(|id| id_ok(id))?;
                Some(Found { folder_id: folder_id.to_owned(), message: GraphMessage { id: id.to_owned(), summary: convert::summary_of(m) } })
            })
            .collect())
    }
}

/// An id checked before it goes into a request.
fn checked(id: &str) -> Result<&str> {
    if id_ok(id) { Ok(id) } else { Err(NetError::Invalid(format!("not an id: {}", clip(id)))) }
}

fn to_bytes(v: &Value) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|e| NetError::Invalid(e.to_string()))
}
