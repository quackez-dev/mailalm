//! The rule editor's form (C5): what the user types, turned into a [`Rule`] and checked where it
//! is to run before it is sent to the account's worker. Pure: the page is `ui/rules.rs`.
//!
//! - A rule runs on the account's server when the provider keeps rules (Outlook's inbox rules,
//!   Gmail's filters), else in MailALM ([`default_place`]).
//! - Forwarding is only for server rules, and only once the user confirmed it: the editor asks,
//!   with what it means, before the rule is saved ([`RuleForm::needs_forward_confirmation`]).
//! - No action runs a program: the model has none.

use mailalm_model::rules::{Action, Condition, Host, Rule, Where};
use mailalm_model::{Provider, Summary};

/// What a condition looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondKind {
    From,
    To,
    Subject,
    Body,
    Words,
    HasAttachment,
}

impl CondKind {
    pub const ALL: [CondKind; 6] = [CondKind::From, CondKind::To, CondKind::Subject, CondKind::Body, CondKind::Words, CondKind::HasAttachment];

    pub fn label(self) -> &'static str {
        match self {
            CondKind::From => "From contains",
            CondKind::To => "To or Cc contains",
            CondKind::Subject => "Subject contains",
            CondKind::Body => "Text contains",
            CondKind::Words => "Subject or text contains",
            CondKind::HasAttachment => "Has an attachment",
        }
    }
}

/// What an action does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActKind {
    MoveTo,
    MarkRead,
    Flag,
    Category,
    Delete,
    Forward,
}

impl ActKind {
    pub const ALL: [ActKind; 6] = [ActKind::MoveTo, ActKind::MarkRead, ActKind::Flag, ActKind::Category, ActKind::Delete, ActKind::Forward];

    pub fn label(self) -> &'static str {
        match self {
            ActKind::MoveTo => "Move to the folder",
            ActKind::MarkRead => "Mark as read",
            ActKind::Flag => "Flag",
            ActKind::Category => "Add the category (Gmail: label)",
            ActKind::Delete => "Move to the Trash",
            ActKind::Forward => "Forward to",
        }
    }
}

/// One condition as typed: words separated by commas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CondRow {
    pub kind: CondKind,
    pub words: String,
}

/// One action as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActRow {
    pub kind: ActKind,
    pub value: String,
}

/// The editor's form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleForm {
    /// Empty for a new rule.
    pub id: String,
    pub name: String,
    pub place: Where,
    pub enabled: bool,
    pub stop: bool,
    pub conditions: Vec<CondRow>,
    pub actions: Vec<ActRow>,
    /// The user confirmed the forwarding (asked by the editor).
    pub forward_confirmed: bool,
}

/// Where a new rule of an account runs: on its server when the provider keeps rules.
pub fn default_place(provider: Provider) -> Where {
    if host_of(provider).is_some() { Where::Server } else { Where::Local }
}

/// Who keeps an account's server rules, if anyone MailALM can reach.
pub fn host_of(provider: Provider) -> Option<Host> {
    match provider {
        Provider::Outlook => Some(Host::Outlook),
        Provider::Gmail => Some(Host::Gmail),
        Provider::Imap(_) => None,
    }
}

/// Where a rule runs, as the list says it.
pub fn place_text(place: Where, provider: Provider) -> &'static str {
    match (place, provider) {
        (Where::Local, _) => "In MailALM, on new mail in the Inbox, while MailALM runs",
        (Where::Server, Provider::Outlook) => "On Outlook.com's server (an inbox rule), even when MailALM is closed",
        (Where::Server, Provider::Gmail) => "On Gmail's server (a filter), even when MailALM is closed",
        (Where::Server, Provider::Imap(_)) => "On the server",
    }
}

/// Comma-separated words, trimmed, empty ones left out.
fn words(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

impl RuleForm {
    /// A new rule for an account of this provider: one empty condition, one empty move.
    pub fn new(provider: Provider) -> RuleForm {
        RuleForm {
            id: String::new(),
            name: String::new(),
            place: default_place(provider),
            enabled: true,
            stop: false,
            conditions: vec![CondRow { kind: CondKind::From, words: String::new() }],
            actions: vec![ActRow { kind: ActKind::MoveTo, value: String::new() }],
            forward_confirmed: false,
        }
    }

    /// A new rule for the messages of this one's sender ("Move ▾ → Create a rule from this
    /// message"): the folder to move them to is left for the user to choose.
    pub fn from_message(provider: Provider, s: &Summary) -> RuleForm {
        let who = if s.from.address.trim().is_empty() { s.from.name.trim() } else { s.from.address.trim() };
        let name = mailalm_model::pim::clean_line(&format!("Messages from {}", s.from.display()), mailalm_model::rules::MAX_TEXT);
        RuleForm { name, conditions: vec![CondRow { kind: CondKind::From, words: who.to_owned() }], ..RuleForm::new(provider) }
    }

    /// A rule of the list, to change it (one made elsewhere that MailALM can't show isn't).
    pub fn from_rule(r: &Rule) -> RuleForm {
        let conditions = r
            .conditions
            .iter()
            .map(|c| {
                let kind = match c {
                    Condition::From(_) => CondKind::From,
                    Condition::To(_) => CondKind::To,
                    Condition::Subject(_) => CondKind::Subject,
                    Condition::Body(_) => CondKind::Body,
                    Condition::Words(_) => CondKind::Words,
                    Condition::HasAttachment => CondKind::HasAttachment,
                };
                CondRow { kind, words: c.words().join(", ") }
            })
            .collect();
        let actions = r
            .actions
            .iter()
            .map(|a| match a {
                Action::MoveTo(p) => ActRow { kind: ActKind::MoveTo, value: p.clone() },
                Action::MarkRead => ActRow { kind: ActKind::MarkRead, value: String::new() },
                Action::Flag => ActRow { kind: ActKind::Flag, value: String::new() },
                Action::Category(c) => ActRow { kind: ActKind::Category, value: c.clone() },
                Action::Delete => ActRow { kind: ActKind::Delete, value: String::new() },
                Action::Forward(to) => ActRow { kind: ActKind::Forward, value: to.join(", ") },
            })
            .collect();
        RuleForm {
            id: r.id.clone(),
            name: r.name.clone(),
            place: r.place,
            enabled: r.enabled,
            stop: r.stop,
            conditions,
            actions,
            forward_confirmed: r.forward_confirmed,
        }
    }

    /// The rule forwards mail.
    pub fn forwards(&self) -> bool {
        self.actions.iter().any(|a| a.kind == ActKind::Forward)
    }

    /// The editor must ask the user to confirm the forwarding before the rule is saved.
    pub fn needs_forward_confirmation(&self) -> bool {
        self.forwards() && !self.forward_confirmed
    }

    /// The rule as typed, without checking it.
    pub fn rule(&self) -> Rule {
        let conditions = self
            .conditions
            .iter()
            .map(|c| {
                let w = words(&c.words);
                match c.kind {
                    CondKind::From => Condition::From(w),
                    CondKind::To => Condition::To(w),
                    CondKind::Subject => Condition::Subject(w),
                    CondKind::Body => Condition::Body(w),
                    CondKind::Words => Condition::Words(w),
                    CondKind::HasAttachment => Condition::HasAttachment,
                }
            })
            .collect();
        let actions = self
            .actions
            .iter()
            .map(|a| match a.kind {
                ActKind::MoveTo => Action::MoveTo(a.value.trim().to_owned()),
                ActKind::MarkRead => Action::MarkRead,
                ActKind::Flag => Action::Flag,
                ActKind::Category => Action::Category(a.value.trim().to_owned()),
                ActKind::Delete => Action::Delete,
                ActKind::Forward => Action::Forward(words(&a.value)),
            })
            .collect();
        Rule {
            id: self.id.clone(),
            name: self.name.trim().to_owned(),
            place: self.place,
            enabled: self.enabled,
            conditions,
            actions,
            stop: self.stop,
            forward_confirmed: self.forward_confirmed,
            unsupported: Vec::new(),
        }
    }

    /// The rule, checked for where it runs on an account of this provider; else why not, as the
    /// user reads it.
    pub fn build(&self, provider: Provider) -> Result<Rule, String> {
        let host = match self.place {
            Where::Local => Host::Local,
            Where::Server => host_of(provider).ok_or("This account's server keeps no rules MailALM can reach: make it a MailALM rule.")?,
        };
        if self.forwards() && self.place == Where::Local {
            return Err("MailALM's own rules never forward mail: forwarding is only for a rule on the server.".to_owned());
        }
        if self.needs_forward_confirmation() {
            return Err("Forwarding sends your mail to another address: confirm it first.".to_owned());
        }
        let rule = self.rule();
        rule.check(host)?;
        Ok(rule)
    }
}

#[cfg(test)]
mod tests {
    use mailalm_model::{ImapPreset, Person};

    use super::*;

    const IMAP: Provider = Provider::Imap(ImapPreset::Videotron);

    fn news(provider: Provider) -> RuleForm {
        RuleForm {
            name: "News".into(),
            conditions: vec![CondRow { kind: CondKind::From, words: "news@shop.example".into() }],
            actions: vec![ActRow { kind: ActKind::MoveTo, value: " News ".into() }, ActRow { kind: ActKind::MarkRead, value: String::new() }],
            ..RuleForm::new(provider)
        }
    }

    #[test]
    fn rules_run_on_the_server_when_the_provider_keeps_them() {
        assert_eq!(RuleForm::new(Provider::Outlook).place, Where::Server);
        assert_eq!(RuleForm::new(Provider::Gmail).place, Where::Server);
        assert_eq!(RuleForm::new(IMAP).place, Where::Local);
        let r = news(IMAP).build(IMAP).expect("a local rule");
        assert_eq!(r.actions.first(), Some(&Action::MoveTo("News".into())));
        assert!(news(Provider::Outlook).build(Provider::Outlook).is_ok());
        let server_on_imap = RuleForm { place: Where::Server, ..news(IMAP) };
        assert!(server_on_imap.build(IMAP).is_err());
    }

    #[test]
    fn the_editor_refuses_what_the_host_cant_keep() {
        assert!(RuleForm { name: " ".into(), ..news(Provider::Outlook) }.build(Provider::Outlook).is_err(), "Outlook needs a name");
        assert!(RuleForm { name: String::new(), ..news(Provider::Gmail) }.build(Provider::Gmail).is_ok(), "a Gmail filter has none");
        assert!(RuleForm { conditions: vec![], ..news(IMAP) }.build(IMAP).is_err(), "no condition");
        let empty_words = RuleForm { conditions: vec![CondRow { kind: CondKind::Subject, words: " , ".into() }], ..news(IMAP) };
        assert!(empty_words.build(IMAP).is_err());
        let two = RuleForm { conditions: vec![CondRow { kind: CondKind::From, words: "a@x.example, b@x.example".into() }], ..news(Provider::Gmail) };
        assert!(two.build(Provider::Gmail).is_err(), "Gmail: one word per condition");
        assert!(two.build(Provider::Outlook).is_ok());
        let flag = RuleForm { actions: vec![ActRow { kind: ActKind::Flag, value: String::new() }], ..news(Provider::Outlook) };
        assert!(flag.build(Provider::Outlook).is_err(), "Outlook's rules don't flag");
        let category = RuleForm { actions: vec![ActRow { kind: ActKind::Category, value: "Red".into() }], ..news(IMAP) };
        assert!(category.build(IMAP).is_err(), "MailALM's rules set no category");
        let no_folder = RuleForm { actions: vec![ActRow { kind: ActKind::MoveTo, value: "  ".into() }], ..news(IMAP) };
        assert!(no_folder.build(IMAP).is_err());
    }

    #[test]
    fn forwarding_needs_a_server_rule_and_a_confirmation() {
        let fwd = RuleForm { actions: vec![ActRow { kind: ActKind::Forward, value: "me@elsewhere.example".into() }], ..news(Provider::Outlook) };
        assert!(fwd.needs_forward_confirmation());
        assert!(fwd.build(Provider::Outlook).is_err());
        let confirmed = RuleForm { forward_confirmed: true, ..fwd.clone() };
        let rule = confirmed.build(Provider::Outlook).expect("confirmed");
        assert!(rule.forward_confirmed);
        assert_eq!(rule.actions, vec![Action::Forward(vec!["me@elsewhere.example".into()])]);
        assert!(RuleForm { place: Where::Local, ..confirmed.clone() }.build(Provider::Outlook).is_err(), "never locally");
        let bad = RuleForm { actions: vec![ActRow { kind: ActKind::Forward, value: "not an address".into() }], ..confirmed };
        assert!(bad.build(Provider::Outlook).is_err());
    }

    #[test]
    fn a_rule_comes_back_as_it_was_typed_and_from_a_message() {
        let form = news(Provider::Outlook);
        let rule = form.build(Provider::Outlook).expect("rule");
        let again = RuleForm::from_rule(&rule);
        assert_eq!(again.rule(), rule);
        let s = Summary { from: Person { name: "Shop News".into(), address: "news@shop.example".into() }, ..Summary::default() };
        let from = RuleForm::from_message(Provider::Gmail, &s);
        assert_eq!(from.name, "Messages from Shop News");
        assert_eq!(from.conditions, vec![CondRow { kind: CondKind::From, words: "news@shop.example".into() }]);
        assert_eq!(from.place, Where::Server);
        assert!(from.build(Provider::Gmail).is_err(), "the folder is still to choose");
    }
}
