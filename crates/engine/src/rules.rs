//! The rules of an account (C5), whatever its kind: listed (the server's, then MailALM's own),
//! saved and deleted where they run, previewed over the cache; and MailALM's own run on new mail
//! in the Inbox after each sync of it.
//!
//! MailALM's own rules see only the Inbox's messages newer than the last they saw
//! ([`RulesMark`]): the first run, and a new Inbox, start from where the Inbox is, so rules never
//! sweep old mail by themselves. A message a snooze brought back is left alone.

use mailalm_model::FolderRole;
use mailalm_model::rules::{self, Condition, Host, LOCAL_PREFIX, Places, Rule, Where};
use mailalm_store::RulesMark;

use crate::mail_worker::{MailWorker, folder_with_role};
use crate::pim_sync::random_hex;
use crate::{Event, RuleCommand};

/// The most hits a preview lists.
const MAX_PREVIEW: usize = 1_000;
/// The most new messages MailALM's rules look at after one sync (the rest are taken as seen).
const MAX_NEW: usize = 2_000;

pub(crate) fn handle<W: MailWorker>(w: &mut W, command: RuleCommand) {
    match command {
        RuleCommand::List => {}
        RuleCommand::Save(rule) => {
            if let Err(e) = save(w, *rule) {
                w.tell(Event::Error { account: w.who(), text: e });
            }
        }
        RuleCommand::Delete { id, place } => {
            if let Err(e) = delete(w, &id, place) {
                w.tell(Event::Error { account: w.who(), text: e });
            }
        }
        RuleCommand::Preview { id, rule, folders } => {
            let hits = preview(&*w, &rule, &folders);
            w.tell(Event::RulePreview { account: w.who(), id, hits });
            return;
        }
    }
    list(w);
}

/// Tells the interface the rules: the server's, read now (an error said apart), then MailALM's.
fn list<W: MailWorker>(w: &mut W) {
    let mut all = Vec::new();
    if w.rule_host().is_some() {
        if w.reach() {
            match w.server_rules() {
                Ok(r) => all.extend(r),
                Err(e) => w.tell(Event::Error { account: w.who(), text: format!("The server's rules could not be read: {e}") }),
            }
        } else {
            w.tell(Event::Error { account: w.who(), text: "The server can't be reached: only MailALM's own rules are shown.".to_owned() });
        }
    }
    all.extend(w.cache().rules());
    w.tell(Event::Rules { account: w.who(), rules: all });
}

fn save<W: MailWorker>(w: &mut W, mut rule: Rule) -> Result<(), String> {
    match rule.place {
        Where::Local => {
            rule.check(Host::Local)?;
            let mut kept = w.cache().rules();
            if rule.id.is_empty() {
                rule.id = format!("{LOCAL_PREFIX}{}", random_hex());
                kept.push(rule);
            } else {
                let slot = kept.iter_mut().find(|r| r.id == rule.id).ok_or("This rule is no longer there.")?;
                *slot = rule;
            }
            w.cache().save_rules(&kept).map_err(|e| format!("The rule could not be kept: {e}"))
        }
        Where::Server => {
            let host = w.rule_host().ok_or("This account's server keeps no rules MailALM can reach: make it a MailALM rule.")?;
            rule.check(host)?;
            if !w.reach() {
                return Err("The server can't be reached: its rules can be changed once MailALM is online.".to_owned());
            }
            w.save_server_rule(&rule).map(drop).map_err(|e| format!("The rule could not be saved: {e}"))
        }
    }
}

fn delete<W: MailWorker>(w: &mut W, id: &str, place: Where) -> Result<(), String> {
    match place {
        Where::Local => {
            let mut kept = w.cache().rules();
            kept.retain(|r| r.id != id);
            w.cache().save_rules(&kept).map_err(|e| format!("The rule could not be deleted: {e}"))
        }
        Where::Server => {
            if w.rule_host().is_none() {
                return Err("This account's server keeps no rules.".to_owned());
            }
            if !w.reach() {
                return Err("The server can't be reached: its rules can be changed once MailALM is online.".to_owned());
            }
            w.delete_server_rule(id).map_err(|e| format!("The rule could not be deleted: {e}"))
        }
    }
}

/// The cached messages the rule would touch in these folders, newest first.
fn preview<W: MailWorker>(w: &W, rule: &Rule, scope: &mailalm_model::FolderScope) -> Vec<(String, mailalm_model::Summary)> {
    let needs_text = rule.conditions.iter().any(|c| matches!(c, Condition::Body(_) | Condition::Words(_)));
    let store = w.cache();
    let mut hits = Vec::new();
    'folders: for f in scope.select(&w.mail_folders()) {
        for s in store.summaries(&f.path) {
            let text = if needs_text { store.text(&f.path, s.uid) } else { None };
            if !rules::preview(rule, [(f.path.as_str(), &s, text.as_deref())]).is_empty() {
                hits.push((f.path.clone(), s));
                if hits.len() >= MAX_PREVIEW {
                    break 'folders;
                }
            }
        }
    }
    hits.sort_by(|a, b| b.1.date.cmp(&a.1.date));
    hits
}

/// After a sync of the Inbox: MailALM's own rules on the messages new to them.
pub(crate) fn run_new<W: MailWorker>(w: &mut W, inbox: &str) {
    let store = w.cache();
    let mut summaries = store.summaries(inbox);
    let top = summaries.iter().map(|s| s.uid).max().unwrap_or(0);
    let since = match store.rules_mark() {
        Some(m) if m.inbox == inbox && m.uid <= top => m.uid,
        // The first run, another Inbox, or its numbers started over: from here on.
        _ => {
            let _ = store.save_rules_mark(&RulesMark { inbox: inbox.to_owned(), uid: top });
            return;
        }
    };
    if top == since {
        return;
    }
    let kept = store.rules();
    let later = store.later();
    let trash = folder_with_role(&*w, FolderRole::Trash).map(|f| f.path);
    let places = Places { inbox, trash: trash.as_deref() };
    summaries.retain(|s| s.uid > since && !later.was_woken(s));
    summaries.sort_by_key(|s| s.uid);
    summaries.truncate(MAX_NEW);
    let needs_text = kept.iter().any(|r| r.conditions.iter().any(|c| matches!(c, Condition::Body(_) | Condition::Words(_))));
    let mut changes = Vec::new();
    if kept.iter().any(|r| r.place == Where::Local && r.enabled) {
        for s in &summaries {
            let text = if needs_text { store.text(inbox, s.uid) } else { None };
            changes.extend(rules::changes_for(&kept, places, s, text.as_deref()));
        }
    }
    let _ = store.save_rules_mark(&RulesMark { inbox: inbox.to_owned(), uid: top });
    for c in changes {
        w.change(c);
    }
}
