//! Mail rules (PLAN.md, section 14.1, C5): one model for the rules kept on the server (Outlook's
//! inbox rules through Graph `messageRules`, Gmail's filters) and those MailALM runs itself on new
//! mail in the Inbox while it runs.
//!
//! A rule is conditions, all of which must hold (each one holds when any of its words is found),
//! then actions. The actions are those of the plan: move, mark read, flag, category (a label for
//! Gmail), delete (to the Trash). **A rule never runs a program**, and nothing here can say so;
//! forwarding exists only with [`Rule::forward_confirmed`], set by the interface when the user
//! confirms it at creation, and only for server rules.
//!
//! A server rule the model can't express (an exception, a condition or an action MailALM doesn't
//! know) is read anyway, with what it doesn't understand listed in [`Rule::unsupported`]: it is
//! shown read only ("made elsewhere") and can only be deleted.
//!
//! [`Rule::check`] says whether a rule can be saved where it says it runs; [`Rule::matches`] and
//! [`preview`] are what MailALM's own rules, and the preview of any rule, do with the cache.

use serde::{Deserialize, Serialize};

use crate::mail::{Change, Summary};
use crate::pim::{address_ok, clean_line};

/// Where a rule runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Where {
    /// On the account's server (Outlook, Gmail): it runs even when MailALM is closed.
    Server,
    /// In MailALM, on new mail in the Inbox, while MailALM runs.
    Local,
}

/// Who keeps a rule, which decides what it may say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// Outlook.com, through Graph's `messageRules`.
    Outlook,
    /// A Gmail filter.
    Gmail,
    /// MailALM itself.
    Local,
}

/// What a rule looks at. Each holds when any of its words is found (ignoring case).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// The sender's name or address.
    From(Vec<String>),
    /// A recipient's name or address (To or Cc).
    To(Vec<String>),
    Subject(Vec<String>),
    /// The text of the message (the preview when the text isn't in the cache).
    Body(Vec<String>),
    /// The subject or the text.
    Words(Vec<String>),
    HasAttachment,
}

/// What a rule does. There is no action that runs a program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Moves the message to this folder (its path in MailALM).
    MoveTo(String),
    MarkRead,
    Flag,
    /// Adds this category (Outlook) or label (Gmail, by its name).
    Category(String),
    /// Moves the message to the Trash.
    Delete,
    /// Forwards it to these addresses: only with [`Rule::forward_confirmed`], on the server.
    Forward(Vec<String>),
}

/// A rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// The server's id, or MailALM's for its own rules; empty for a rule not saved yet.
    #[serde(default)]
    pub id: String,
    /// Empty for a Gmail filter (Gmail gives them none): [`Rule::title`] says what it does then.
    #[serde(default)]
    pub name: String,
    pub place: Where,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// The rules after this one don't run on a message it matched.
    #[serde(default)]
    pub stop: bool,
    /// The user confirmed the forwarding when the rule was made (the interface asks it).
    #[serde(default)]
    pub forward_confirmed: bool,
    /// What a rule made elsewhere has that MailALM can't show or keep: such a rule is read only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

fn yes() -> bool {
    true
}

/// Most conditions and actions in a rule, words in a condition, addresses forwarded to.
pub const MAX_CONDITIONS: usize = 10;
pub const MAX_ACTIONS: usize = 10;
pub const MAX_WORDS: usize = 20;
pub const MAX_FORWARD: usize = 10;
/// Longest name, word or folder, in characters.
pub const MAX_TEXT: usize = 255;

/// The prefix of the ids of MailALM's own rules.
pub const LOCAL_PREFIX: &str = "local-";

impl Condition {
    /// The words of the condition (none for [`Condition::HasAttachment`]).
    pub fn words(&self) -> &[String] {
        match self {
            Condition::From(w) | Condition::To(w) | Condition::Subject(w) | Condition::Body(w) | Condition::Words(w) => w,
            Condition::HasAttachment => &[],
        }
    }

    /// Whether it holds for a message; `text` is its text when the cache has it.
    pub fn holds(&self, s: &Summary, text: Option<&str>) -> bool {
        let in_body = |w: &str| match text {
            Some(t) => has(t, w),
            None => has(&s.preview, w),
        };
        match self {
            Condition::From(w) => any_word(w, |x| has(&s.from.name, x) || has(&s.from.address, x)),
            Condition::To(w) => any_word(w, |x| s.to.iter().chain(&s.cc).any(|p| has(&p.name, x) || has(&p.address, x))),
            Condition::Subject(w) => any_word(w, |x| has(&s.subject, x)),
            Condition::Body(w) => any_word(w, in_body),
            Condition::Words(w) => any_word(w, |x| has(&s.subject, x) || in_body(x)),
            Condition::HasAttachment => s.has_attachments,
        }
    }
}

/// Whether `found` holds for any of the words (given to it in lowercase).
fn any_word(words: &[String], found: impl Fn(&str) -> bool) -> bool {
    words.iter().any(|w| found(&w.to_lowercase()))
}

/// Whether `field` contains `lower` (already lowercase), ignoring case.
fn has(field: &str, lower: &str) -> bool {
    field.to_lowercase().contains(lower)
}

impl Rule {
    /// A new rule, enabled, not saved yet.
    pub fn new(name: &str, place: Where, conditions: Vec<Condition>, actions: Vec<Action>) -> Rule {
        Rule {
            id: String::new(),
            name: clean_line(name, MAX_TEXT),
            place,
            enabled: true,
            conditions,
            actions,
            stop: false,
            forward_confirmed: false,
            unsupported: Vec::new(),
        }
    }

    /// Made elsewhere with something MailALM can't show: shown, never changed (only deleted).
    pub fn read_only(&self) -> bool {
        !self.unsupported.is_empty()
    }

    /// Whether every condition holds for a message (a rule without conditions holds for all).
    pub fn matches(&self, s: &Summary, text: Option<&str>) -> bool {
        self.conditions.iter().all(|c| c.holds(s, text))
    }

    /// The name, or for a rule without one (a Gmail filter) a short account of its conditions.
    pub fn title(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        let part = |what: &str, w: &[String]| format!("{what} {}", w.join(" or "));
        let parts: Vec<String> = self
            .conditions
            .iter()
            .map(|c| match c {
                Condition::From(w) => part("from", w),
                Condition::To(w) => part("to", w),
                Condition::Subject(w) => part("subject has", w),
                Condition::Body(w) => part("text has", w),
                Condition::Words(w) => part("has the words", w),
                Condition::HasAttachment => "has an attachment".to_owned(),
            })
            .collect();
        let title = if parts.is_empty() { "Every message".to_owned() } else { parts.join(", ") };
        clean_line(&title, MAX_TEXT)
    }

    /// Whether the rule can be saved by `host`, as it is; else why not, as the user reads it.
    pub fn check(&self, host: Host) -> Result<(), String> {
        if self.read_only() {
            return Err("This rule was made elsewhere with something MailALM can't show: it can only be deleted.".into());
        }
        match (host, self.place) {
            (Host::Local, Where::Server) | (Host::Outlook | Host::Gmail, Where::Local) => {
                return Err("internal: the rule is not for this place".into());
            }
            _ => {}
        }
        if clean_line(&self.name, MAX_TEXT) != self.name {
            return Err("The rule's name must be one line of at most 255 characters.".into());
        }
        if host != Host::Gmail && self.name.is_empty() {
            return Err("The rule needs a name.".into());
        }
        if self.conditions.len() > MAX_CONDITIONS {
            return Err(format!("A rule has at most {MAX_CONDITIONS} conditions."));
        }
        if self.conditions.is_empty() && host != Host::Outlook {
            return Err("The rule needs a condition: a rule for every message is easy to make by mistake.".into());
        }
        if self.actions.is_empty() {
            return Err("The rule needs an action.".into());
        }
        if self.actions.len() > MAX_ACTIONS {
            return Err(format!("A rule has at most {MAX_ACTIONS} actions."));
        }
        for c in &self.conditions {
            let words = c.words();
            if !matches!(c, Condition::HasAttachment) && words.is_empty() {
                return Err("A condition needs a word to look for.".into());
            }
            if words.len() > MAX_WORDS {
                return Err(format!("A condition has at most {MAX_WORDS} words."));
            }
            if words.iter().any(|w| w.is_empty() || clean_line(w, MAX_TEXT) != *w) {
                return Err("Each word must be one line of at most 255 characters, not empty.".into());
            }
        }
        let mut moves = 0;
        for a in &self.actions {
            match a {
                Action::MoveTo(p) | Action::Category(p) if p.is_empty() || clean_line(p, MAX_TEXT) != *p => {
                    return Err("A folder or category must be one line of at most 255 characters.".into());
                }
                Action::Forward(to) => {
                    if !self.forward_confirmed {
                        return Err("Forwarding needs the user's confirmation when the rule is made.".into());
                    }
                    if to.is_empty() || to.len() > MAX_FORWARD || !to.iter().all(|a| address_ok(a)) {
                        return Err(format!("Forwarding needs from 1 to {MAX_FORWARD} e-mail addresses."));
                    }
                }
                _ => {}
            }
            if matches!(a, Action::MoveTo(_) | Action::Delete) {
                moves += 1;
            }
        }
        if moves > 1 {
            return Err("A rule moves a message to one place only.".into());
        }
        match host {
            Host::Outlook => {
                if self.actions.contains(&Action::Flag) {
                    return Err("Outlook's server rules can't flag a message: make it a MailALM rule instead.".into());
                }
            }
            Host::Gmail => {
                if !self.enabled {
                    return Err("A Gmail filter can't be turned off: delete it instead.".into());
                }
                if self.stop {
                    return Err("Gmail runs every filter: \"stop\" can't be kept.".into());
                }
                if self.conditions.iter().any(|c| matches!(c, Condition::Body(_))) {
                    return Err("A Gmail filter looks at the whole message: use \"has the words\".".into());
                }
                if self.conditions.iter().any(|c| c.words().len() > 1) {
                    return Err("A Gmail filter takes one word or phrase per condition.".into());
                }
                let forwards: usize = self.actions.iter().map(|a| if let Action::Forward(to) = a { to.len() } else { 0 }).sum();
                if forwards > 1 {
                    return Err("A Gmail filter forwards to one address.".into());
                }
            }
            Host::Local => {
                if self.actions.iter().any(|a| matches!(a, Action::Forward(_))) {
                    return Err("MailALM's own rules never forward mail: make it a server rule.".into());
                }
                if self.actions.iter().any(|a| matches!(a, Action::Category(_))) {
                    return Err("MailALM's own rules can't set a category: make it a server rule.".into());
                }
            }
        }
        Ok(())
    }
}

/// Where MailALM's own rules put messages: the account's Inbox and Trash.
#[derive(Clone, Copy, Debug)]
pub struct Places<'a> {
    pub inbox: &'a str,
    /// `None` when the account has no Trash: a rule that deletes then does nothing.
    pub trash: Option<&'a str>,
}

/// What MailALM's own rules do with a new message of the Inbox: the changes, in order (marks
/// first, then the one move). Rules run in their order; a disabled rule, or one that isn't
/// MailALM's, doesn't; one with `stop` that matched ends the run.
pub fn changes_for(rules: &[Rule], places: Places<'_>, s: &Summary, text: Option<&str>) -> Vec<Change> {
    let (mut seen, mut flagged, mut to): (bool, bool, Option<String>) = (false, false, None);
    for r in rules.iter().filter(|r| r.place == Where::Local && r.enabled && !r.read_only()) {
        if !r.matches(s, text) {
            continue;
        }
        for a in &r.actions {
            match a {
                Action::MarkRead => seen = true,
                Action::Flag => flagged = true,
                Action::MoveTo(p) if to.is_none() && p != places.inbox => to = Some(p.clone()),
                Action::Delete if to.is_none() => to = places.trash.map(str::to_owned),
                _ => {}
            }
        }
        if r.stop {
            break;
        }
    }
    let folder = places.inbox.to_owned();
    let uids = vec![s.uid];
    let mut out = Vec::new();
    if seen && !s.flags.seen {
        out.push(Change::SetSeen { folder: folder.clone(), uids: uids.clone(), seen: true });
    }
    if flagged && !s.flags.flagged {
        out.push(Change::SetFlagged { folder: folder.clone(), uids: uids.clone(), flagged: true });
    }
    if let Some(to) = to {
        out.push(Change::Move { folder, uids, to });
    }
    out
}

/// The messages a rule would touch, among those given (folder path, summary, text when cached):
/// the preview the editor shows before a rule is saved.
pub fn preview<'a>(rule: &Rule, messages: impl IntoIterator<Item = (&'a str, &'a Summary, Option<&'a str>)>) -> Vec<(String, u32)> {
    messages.into_iter().filter(|(_, s, text)| rule.matches(s, *text)).map(|(f, s, _)| (f.to_owned(), s.uid)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{Flags, Person};

    fn msg(uid: u32, from: &str, subject: &str) -> Summary {
        Summary {
            uid,
            from: Person { name: "Billing Dept".into(), address: from.into() },
            to: vec![Person { name: String::new(), address: "alex@example.com".into() }],
            subject: subject.into(),
            preview: "Your invoice is attached".into(),
            ..Summary::default()
        }
    }

    fn news() -> Rule {
        Rule::new("News", Where::Local, vec![Condition::From(vec!["NEWS@".into(), "promo@".into()])], vec![Action::MarkRead, Action::MoveTo("News".into())])
    }

    #[test]
    fn conditions_all_hold_any_word_each() {
        let r = news();
        assert!(r.matches(&msg(1, "news@shop.example", "Sale"), None));
        assert!(r.matches(&msg(1, "promo@shop.example", "Sale"), None));
        assert!(!r.matches(&msg(1, "zoe@studio.example", "Sale"), None));
        let both = Rule { conditions: vec![Condition::From(vec!["shop".into()]), Condition::Subject(vec!["sale".into()])], ..news() };
        assert!(both.matches(&msg(1, "news@shop.example", "Big SALE"), None));
        assert!(!both.matches(&msg(1, "news@shop.example", "Newsletter"), None));
        // The text when cached, else the preview.
        let body = Condition::Body(vec!["invoice".into()]);
        assert!(body.holds(&msg(1, "a@b.c", "x"), None));
        assert!(!body.holds(&msg(1, "a@b.c", "x"), Some("Nothing here")));
        assert!(Condition::Words(vec!["sale".into()]).holds(&msg(1, "a@b.c", "Sale"), Some("")));
        assert!(Condition::To(vec!["alex".into()]).holds(&msg(1, "a@b.c", ""), None));
        let mut s = msg(1, "a@b.c", "");
        assert!(!Condition::HasAttachment.holds(&s, None));
        s.has_attachments = true;
        assert!(Condition::HasAttachment.holds(&s, None));
    }

    #[test]
    fn local_rules_make_changes_marks_first_then_one_move() {
        let places = Places { inbox: "INBOX", trash: Some("Trash") };
        let flag_all = Rule::new("Flag", Where::Local, vec![Condition::Subject(vec!["sale".into()])], vec![Action::Flag, Action::Delete]);
        let rules = vec![news(), flag_all];
        let s = msg(7, "news@shop.example", "Sale");
        assert_eq!(
            changes_for(&rules, places, &s, None),
            vec![
                Change::SetSeen { folder: "INBOX".into(), uids: vec![7], seen: true },
                Change::SetFlagged { folder: "INBOX".into(), uids: vec![7], flagged: true },
                // The first rule's move wins over the second's delete.
                Change::Move { folder: "INBOX".into(), uids: vec![7], to: "News".into() },
            ]
        );
        // "stop" ends the run; a disabled rule and a server rule never run here.
        let stopping = Rule { stop: true, ..news() };
        let off = Rule { enabled: false, ..Rule::new("Off", Where::Local, vec![Condition::From(vec!["news".into()])], vec![Action::Flag]) };
        let server = Rule::new("Srv", Where::Server, vec![Condition::From(vec!["news".into()])], vec![Action::Flag]);
        let changes = changes_for(&[off, server, stopping, Rule::new("F", Where::Local, vec![Condition::From(vec!["news".into()])], vec![Action::Flag])], places, &s, None);
        assert_eq!(changes.len(), 2);
        assert!(!changes.iter().any(|c| matches!(c, Change::SetFlagged { .. })));
        // Deleting goes to the Trash, or nowhere without one; a read message isn't marked again.
        let del = Rule::new("Del", Where::Local, vec![Condition::From(vec!["news".into()])], vec![Action::Delete, Action::MarkRead]);
        let read = Summary { flags: Flags { seen: true, ..Flags::default() }, ..s.clone() };
        assert_eq!(changes_for(std::slice::from_ref(&del), places, &read, None), vec![Change::Move { folder: "INBOX".into(), uids: vec![7], to: "Trash".into() }]);
        assert!(changes_for(&[del], Places { inbox: "INBOX", trash: None }, &read, None).is_empty());
    }

    #[test]
    fn the_preview_lists_what_would_be_touched() {
        let a = msg(1, "news@shop.example", "Sale");
        let b = msg(2, "zoe@studio.example", "Storyboards");
        let c = msg(3, "promo@shop.example", "Last day");
        let hits = preview(&news(), [("INBOX", &a, None), ("INBOX", &b, None), ("Archive", &c, Some("text"))]);
        assert_eq!(hits, vec![("INBOX".to_owned(), 1), ("Archive".to_owned(), 3)]);
    }

    #[test]
    fn what_each_host_takes() {
        assert!(news().check(Host::Local).is_ok());
        assert!(news().check(Host::Outlook).is_err(), "a local rule isn't the server's");
        let server = Rule { place: Where::Server, ..news() };
        assert!(server.check(Host::Outlook).is_ok());
        assert!(server.check(Host::Gmail).is_err(), "two words in one condition");
        let one = Rule { conditions: vec![Condition::From(vec!["news@".into()])], ..server.clone() };
        assert!(one.check(Host::Gmail).is_ok());
        assert!(Rule { stop: true, ..one.clone() }.check(Host::Gmail).is_err());
        assert!(Rule { enabled: false, ..one.clone() }.check(Host::Gmail).is_err());
        assert!(Rule { actions: vec![Action::Flag], ..server.clone() }.check(Host::Outlook).is_err(), "Outlook rules don't flag");
        assert!(Rule { actions: vec![Action::Flag], ..one.clone() }.check(Host::Gmail).is_ok());
        // Forwarding: confirmed, addresses, never locally.
        let fwd = Rule { actions: vec![Action::Forward(vec!["me@elsewhere.example".into()])], ..server.clone() };
        assert!(fwd.check(Host::Outlook).is_err());
        let fwd = Rule { forward_confirmed: true, ..fwd };
        assert!(fwd.check(Host::Outlook).is_ok());
        assert!(Rule { actions: vec![Action::Forward(vec!["not an address".into()])], ..fwd.clone() }.check(Host::Outlook).is_err());
        assert!(Rule { place: Where::Local, ..fwd }.check(Host::Local).is_err());
        assert!(Rule { actions: vec![Action::Category("Red".into())], ..news() }.check(Host::Local).is_err());
        // Sizes and shapes.
        assert!(Rule { conditions: vec![], ..news() }.check(Host::Local).is_err());
        assert!(Rule { conditions: vec![], ..server.clone() }.check(Host::Outlook).is_ok());
        assert!(Rule { actions: vec![], ..news() }.check(Host::Local).is_err());
        assert!(Rule { actions: vec![Action::Delete, Action::MoveTo("X".into())], ..news() }.check(Host::Local).is_err());
        assert!(Rule { conditions: vec![Condition::Subject(vec!["a\r\nb".into()])], ..news() }.check(Host::Local).is_err());
        assert!(Rule { name: "two\nlines".into(), ..news() }.check(Host::Local).is_err());
        assert!(Rule { unsupported: vec!["an exception".into()], ..news() }.check(Host::Local).is_err());
    }

    #[test]
    fn titles() {
        assert_eq!(news().title(), "News");
        let gmail = Rule { name: String::new(), conditions: vec![Condition::From(vec!["a@b.c".into()]), Condition::HasAttachment], ..news() };
        assert_eq!(gmail.title(), "from a@b.c, has an attachment");
    }
}
