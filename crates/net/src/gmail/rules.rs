//! MailALM's own: Gmail's filters as MailALM's rules ([`mailalm_model::rules`]), beyond the
//! sender filters ([`super::Filter`], which the blocked and allowed senders list keeps); making
//! a label (`MailALM/Snoozed`); and Gmail's search (`q=`).
//!
//! A filter has criteria (`from`, `to`, `subject`, `query`, `hasAttachment`…) and an action
//! (labels added and taken off, a forwarding address). As MailALM's rule: "skip the Inbox" with a
//! label is a move to that label's folder, without one a move to All Mail; a label added is a
//! category; `STARRED` a flag, `TRASH` a deletion, `UNREAD` taken off "mark read". Criteria
//! written in Gmail's search language (`OR`, quotes, brackets, `-`, an operator) and every other
//! criterion or action make the rule read only. Gmail can't change a filter: the engine makes
//! the new one, then deletes the old.

use mailalm_model::query::Term;
use mailalm_model::rules::{Action, Condition, Host, MAX_TEXT, Rule, Where};
use mailalm_model::{Folder, Query};
use serde_json::{Map, Value, json};

use super::convert::{self, clean, filter_id_ok, label_list, says_nothing};
use super::{GmailClient, Label, MAX_FILTERS, json_of, json_request};
use crate::http;
use crate::{NetError, Result, clip};

#[cfg(test)]
mod tests;

/// The longest search sent, in characters.
const MAX_QUERY: usize = 500;

/// Whether a label id is a user's label (Gmail's are `Label_<n>`).
fn user_label(id: &str) -> bool {
    id.starts_with("Label_")
}

/// Whether a criterion's text says only words: nothing Gmail's search language reads as more
/// (quotes, brackets, `OR`, `AND`, a word starting with `-`; and in `query`, a `:` operator).
fn plain(s: &str, query: bool) -> bool {
    !s.contains(['"', '(', ')', '{', '}', '[', ']', '\\'])
        && !(query && s.contains(':'))
        && !s.split_whitespace().any(|w| w == "OR" || w == "AND" || w.starts_with('-') || w == "|")
}

/// A Gmail filter as MailALM's rule; `None` without a usable id, and for the sender filters
/// MailALM's blocked and allowed senders list keeps.
pub(super) fn rule_of(f: &Value, folders: &[Folder]) -> Option<Rule> {
    let id = f.get("id").and_then(Value::as_str).filter(|id| filter_id_ok(id))?;
    if convert::filter_of(f).is_some() {
        return None;
    }
    let mut rule = Rule::new("", Where::Server, Vec::new(), Vec::new());
    rule.id = id.to_owned();
    let empty = Map::new();
    let criteria = f.get("criteria").and_then(Value::as_object).unwrap_or(&empty);
    for (k, v) in criteria.iter().take(32) {
        if says_nothing(v) {
            continue;
        }
        let text = v.as_str().map(|s| clean(s, MAX_TEXT)).filter(|s| !s.is_empty());
        let word = |query: bool| text.clone().filter(|t| plain(t, query)).map(|t| vec![t]);
        let c = match k.as_str() {
            "from" => word(false).map(Condition::From),
            "to" => word(false).map(Condition::To),
            "subject" => word(false).map(Condition::Subject),
            "query" => word(true).map(Condition::Words),
            "hasAttachment" if v.as_bool() == Some(true) => Some(Condition::HasAttachment),
            _ => None,
        };
        match c {
            Some(c) => rule.conditions.push(c),
            None => rule.unsupported.push(format!("criterion {}", key_name(k))),
        }
    }
    let action = f.get("action").and_then(Value::as_object).unwrap_or(&empty);
    let mut add: Vec<&str> = Vec::new();
    let mut remove: Vec<&str> = Vec::new();
    for (k, v) in action.iter().take(32) {
        if says_nothing(v) {
            continue;
        }
        match (k.as_str(), label_list(v)) {
            ("addLabelIds", Some(l)) => add = l,
            ("removeLabelIds", Some(l)) => remove = l,
            ("forward", _) => match v.as_str().map(|s| clean(s, 320)).filter(|a| mailalm_model::pim::address_ok(a)) {
                Some(a) => rule.actions.push(Action::Forward(vec![a])),
                None => rule.unsupported.push("action forward".to_owned()),
            },
            _ => rule.unsupported.push(format!("action {}", key_name(k))),
        }
    }
    let archived = remove.contains(&"INBOX");
    let mut moved = false;
    for l in &remove {
        match *l {
            "INBOX" => {}
            "UNREAD" => rule.actions.push(Action::MarkRead),
            other => rule.unsupported.push(format!("takes off the label {}", key_name(other))),
        }
    }
    for l in &add {
        let folder = folders.iter().find(|f| f.wire == *l);
        match (*l, folder) {
            ("STARRED", _) => rule.actions.push(Action::Flag),
            ("TRASH", _) => rule.actions.push(Action::Delete),
            ("SPAM", Some(f)) if !moved => {
                moved = true;
                rule.actions.push(Action::MoveTo(f.path.clone()));
            }
            (id, Some(f)) if user_label(id) && archived && !moved => {
                moved = true;
                rule.actions.push(Action::MoveTo(f.path.clone()));
            }
            (id, Some(f)) if user_label(id) => rule.actions.push(Action::Category(f.path.clone())),
            (other, _) => rule.unsupported.push(format!("adds the label {}", key_name(other))),
        }
    }
    if archived && !moved {
        match folders.iter().find(|f| f.wire.is_empty()) {
            Some(all) => rule.actions.push(Action::MoveTo(all.path.clone())),
            None => rule.unsupported.push("skips the Inbox".to_owned()),
        }
    }
    Some(rule)
}

/// A key or label of Gmail's as `unsupported` says it: letters, digits and `_`, clipped.
fn key_name(k: &str) -> String {
    k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(60).collect()
}

/// A rule as a Gmail filter (`criteria` and `action`). The rule is checked first
/// ([`Rule::check`]); a criterion Gmail would read as its search language, two of a kind, or a
/// folder Gmail can't put mail in is refused.
pub(super) fn filter_json(rule: &Rule, folders: &[Folder]) -> Result<Value> {
    rule.check(Host::Gmail).map_err(NetError::Invalid)?;
    let mut criteria = Map::new();
    for c in &rule.conditions {
        let first = || c.words().first().cloned().unwrap_or_default();
        let (key, value, query) = match c {
            Condition::From(_) => ("from", json!(first()), false),
            Condition::To(_) => ("to", json!(first()), false),
            Condition::Subject(_) => ("subject", json!(first()), false),
            Condition::Words(_) => ("query", json!(first()), true),
            Condition::HasAttachment => ("hasAttachment", json!(true), false),
            Condition::Body(_) => return Err(NetError::Invalid("a Gmail filter looks at the whole message".into())),
        };
        if value.as_str().is_some_and(|s| !plain(s, query)) {
            return Err(NetError::Invalid(format!("Gmail would read \"{}\" as its search language: use plain words", clip(value.as_str().unwrap_or_default()))));
        }
        if criteria.insert(key.to_owned(), value).is_some() {
            return Err(NetError::Invalid("a Gmail filter takes one criterion of each kind".into()));
        }
    }
    let mut add: Vec<String> = Vec::new();
    let mut remove: Vec<String> = Vec::new();
    let mut forward: Option<String> = None;
    let folder = |path: &str| folders.iter().find(|f| f.path == path).ok_or_else(|| NetError::Invalid(format!("the folder {} no longer exists", clip(path))));
    for a in &rule.actions {
        match a {
            Action::MarkRead => remove.push("UNREAD".into()),
            Action::Flag => add.push("STARRED".into()),
            Action::Delete => add.push("TRASH".into()),
            Action::Category(path) => {
                let f = folder(path.as_str())?;
                if !user_label(&f.wire) {
                    return Err(NetError::Invalid(format!("{} isn't a label of yours", clip(path))));
                }
                add.push(f.wire.clone());
            }
            Action::MoveTo(path) => {
                let f = folder(path.as_str())?;
                match f.wire.as_str() {
                    "" => remove.push("INBOX".into()),
                    "SPAM" | "TRASH" => add.push(f.wire.clone()),
                    w if user_label(w) => {
                        add.push(f.wire.clone());
                        remove.push("INBOX".into());
                    }
                    _ => return Err(NetError::Invalid(format!("a Gmail filter can't put mail in {}", clip(path)))),
                }
            }
            Action::Forward(to) => forward = to.first().cloned(),
        }
    }
    let mut action = Map::new();
    if !add.is_empty() {
        action.insert("addLabelIds".into(), json!(add));
    }
    if !remove.is_empty() {
        action.insert("removeLabelIds".into(), json!(remove));
    }
    if let Some(f) = forward {
        action.insert("forward".into(), json!(f));
    }
    Ok(json!({"criteria": Value::Object(criteria), "action": Value::Object(action)}))
}

/// A word of the search for Gmail's `q`: controls and the characters of its language taken out,
/// a leading `-` too; in quotes when it has a space or a `:`. Empty when nothing is left.
fn q_word(w: &str) -> String {
    let w: String = w.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '(' | ')' | '{' | '}' | '[' | ']' | '\\')).collect();
    let w = w.trim().trim_start_matches('-').trim();
    if w.is_empty() {
        String::new()
    } else if w.contains(|c: char| c.is_whitespace() || c == ':') || w == "OR" || w == "AND" {
        format!("\"{w}\"")
    } else {
        w.to_owned()
    }
}

/// The search box's query in Gmail's search language (`q`): words, `from:`, `to:` (or `cc:`),
/// `subject:`, `has:attachment`, `is:unread`, `is:read`, `is:starred`, `before:` and `after:`
/// (Unix seconds), `category:`. `None` when nothing in it is for the server.
pub(super) fn gmail_q(query: &Query) -> Option<String> {
    fn with(parts: &mut Vec<String>, op: &str, w: &str) {
        let w = q_word(w);
        if !w.is_empty() {
            parts.push(format!("{op}{w}"));
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for t in &query.terms {
        match t {
            Term::Text(w) => with(&mut parts, "", w),
            Term::From(w) => with(&mut parts, "from:", w),
            Term::Subject(w) => with(&mut parts, "subject:", w),
            Term::Category(w) => with(&mut parts, "category:", w),
            Term::To(w) => {
                let w = q_word(w);
                if !w.is_empty() {
                    parts.push(format!("{{to:{w} cc:{w}}}"));
                }
            }
            Term::HasAttachment => parts.push("has:attachment".into()),
            Term::Unread => parts.push("is:unread".into()),
            Term::Read => parts.push("is:read".into()),
            Term::Flagged => parts.push("is:starred".into()),
            Term::Before(d) => parts.push(format!("before:{d}")),
            Term::After(d) => parts.push(format!("after:{}", d.saturating_sub(1))),
            Term::In(_) | Term::Account(_) => {}
        }
    }
    let mut out = String::new();
    for p in parts {
        if out.len() + p.len() + 1 > MAX_QUERY {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&p);
    }
    (!out.is_empty()).then_some(out)
}

impl GmailClient {
    /// The mailbox's filters as rules, but the sender filters ([`GmailClient::filters`]); at most
    /// 1000. `folders` names the labels.
    pub fn rules(&mut self, folders: &[Folder]) -> Result<Vec<Rule>> {
        let v = self.get_json(&format!("{}/settings/filters", self.api()))?;
        let items: &[Value] = match v.get("filter") {
            None | Some(Value::Null) => &[],
            Some(f) => f.as_array().map(Vec::as_slice).ok_or_else(|| NetError::Protocol("a filter list without its list".into()))?,
        };
        Ok(items.iter().take(MAX_FILTERS).filter_map(|f| rule_of(f, folders)).collect())
    }

    /// Make a filter from a rule. A filter made twice is two filters: only an answer that turned
    /// the request away before anything was done lets it go again. The rule as made comes back.
    pub fn create_rule(&mut self, rule: &Rule, folders: &[Folder]) -> Result<Rule> {
        let body = filter_json(rule, folders)?;
        let req = json_request(&format!("{}/settings/filters", self.api()), &body)?;
        let resp = self.exchange(req, true, false).map_err(|f| self.hide(f.error))?;
        self.checked(&resp)?;
        let made = json_of(&resp)?;
        let id = made.get("id").and_then(Value::as_str).filter(|id| filter_id_ok(id)).ok_or_else(|| NetError::Protocol("Gmail made the filter but sent no id for it".into()))?;
        Ok(rule_of(&made, folders).unwrap_or_else(|| Rule { id: id.to_owned(), ..rule.clone() }))
    }

    /// Delete a filter made from a rule. One already gone counts as done.
    pub fn delete_rule(&mut self, id: &str) -> Result<()> {
        self.delete_filter(id)
    }

    /// Make a label (shown in the label list and on messages). One of that name already there is
    /// Gmail's refusal ([`NetError::Server`], HTTP 409): read the labels again.
    pub fn create_label(&mut self, name: &str) -> Result<Label> {
        let name = clean(name, 225);
        if name.is_empty() {
            return Err(NetError::Invalid("not a label name".into()));
        }
        let body = json!({"name": name, "labelListVisibility": "labelShow", "messageListVisibility": "show"});
        let resp = self.exchange(json_request(&format!("{}/labels", self.api()), &body)?, true, false).map_err(|f| self.hide(f.error))?;
        self.checked(&resp)?;
        convert::label_of(&json_of(&resp)?).ok_or_else(|| NetError::Protocol("Gmail made the label but sent it back unreadable".into()))
    }

    /// Gmail's search (`q`), in one label or everywhere but the spam and the bin: the ids found,
    /// newest first, at most `limit`. [`NetError::Invalid`] (nothing sent) when nothing in the
    /// query is for the server.
    pub fn search_ids(&mut self, query: &Query, label: Option<&str>, limit: usize) -> Result<Vec<String>> {
        let q = gmail_q(query).ok_or_else(|| NetError::Invalid("nothing in the search the server can look for".into()))?;
        let mut params = format!("&q={}", http::pct(&q));
        if let Some(l) = label {
            if !convert::id_ok(l) {
                return Err(NetError::Invalid(format!("not a label id: {}", clip(l))));
            }
            params.push_str(&format!("&labelIds={}", http::pct(l)));
            if matches!(l, "SPAM" | "TRASH") {
                params.push_str("&includeSpamTrash=true");
            }
        }
        self.list(&params, limit)
    }
}
