//! The rules, the snoozed messages and follow-ups, and the search, with an account played by the
//! test: its changes are made in its cache at once, a message moved gets a new number.

use std::cell::RefCell;
use std::sync::mpsc;

use mailalm_model::Person;
use mailalm_model::later::Mark;
use mailalm_model::rules::{Action, Condition, Where};
use mailalm_model::{Flags, FolderScope};
use mailalm_net::oauth::now_unix;

use super::*;
use crate::{LaterCommand, RuleCommand, drafts, find, later, rules};

struct Fake {
    store: Store,
    folders: Vec<Folder>,
    told: RefCell<Vec<Event>>,
    changes: Vec<Change>,
    next_uid: u32,
    server_rules: Vec<Rule>,
    /// The drafts saved: (Message-ID, the draft it replaced).
    drafts: Vec<(String, Option<String>)>,
}

fn folder(path: &str, role: FolderRole) -> Folder {
    Folder { path: path.into(), wire: path.into(), delimiter: Some('/'), role, total: 0, unread: 0 }
}

fn msg(uid: u32, from: &str, subject: &str, id: &str) -> Summary {
    Summary {
        uid,
        from: Person { name: String::new(), address: from.into() },
        subject: subject.into(),
        date: now_unix(),
        message_id: id.into(),
        flags: Flags { seen: true, ..Flags::default() },
        ..Summary::default()
    }
}

impl Fake {
    fn new(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("mailalm-engine-p8-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::open(&dir, AccountId(1)).expect("store");
        let folders = vec![
            folder("INBOX", FolderRole::Inbox),
            folder("Sent", FolderRole::Sent),
            folder("Trash", FolderRole::Trash),
            folder("News", FolderRole::Other),
        ];
        Fake { store, folders, told: RefCell::new(Vec::new()), changes: Vec::new(), next_uid: 100, server_rules: Vec::new(), drafts: Vec::new() }
    }

    fn put(&self, path: &str, s: Summary) {
        let mut all = self.store.summaries(path);
        all.push(s);
        self.store.save_summaries(path, &all).expect("summaries");
    }

    fn told(&self) -> Vec<Event> {
        self.told.borrow_mut().drain(..).collect()
    }
}

impl MailWorker for Fake {
    fn who(&self) -> AccountId {
        AccountId(1)
    }

    fn cache(&self) -> &Store {
        &self.store
    }

    fn store_draft(&mut self, _raw: &[u8], message_id: &str, replace: Option<&str>) -> Result<String, String> {
        self.drafts.push((message_id.to_owned(), replace.map(str::to_owned)));
        Ok(message_id.to_owned())
    }

    fn drop_draft(&mut self, draft: &str) -> Result<(), String> {
        self.drafts.retain(|(id, _)| id != draft);
        Ok(())
    }

    fn tell(&self, e: Event) {
        self.told.borrow_mut().push(e);
    }

    fn me(&self) -> (String, String) {
        ("Work".into(), "alex@example.com".into())
    }

    fn mail_folders(&self) -> Vec<Folder> {
        self.folders.clone()
    }

    fn change(&mut self, change: Change) {
        self.changes.push(change.clone());
        let mut at = self.store.summaries(change.folder());
        match &change {
            Change::SetSeen { uids, seen, .. } => at.iter_mut().filter(|s| uids.contains(&s.uid)).for_each(|s| s.flags.seen = *seen),
            Change::SetFlagged { uids, flagged, .. } => at.iter_mut().filter(|s| uids.contains(&s.uid)).for_each(|s| s.flags.flagged = *flagged),
            Change::Move { uids, to, .. } => {
                let mut moved: Vec<Summary> = at.iter().filter(|s| uids.contains(&s.uid)).cloned().collect();
                at.retain(|s| !uids.contains(&s.uid));
                for s in &mut moved {
                    s.uid = self.next_uid;
                    self.next_uid += 1;
                }
                let mut dest = self.store.summaries(to);
                dest.extend(moved);
                self.store.save_summaries(to, &dest).expect("summaries");
            }
            Change::Delete { .. } | Change::MarkJunk { .. } => {}
        }
        self.store.save_summaries(change.folder(), &at).expect("summaries");
    }

    fn refresh(&mut self, _path: &str, _commands: &Receiver<Command>) {}

    fn reach(&mut self) -> bool {
        true
    }

    fn rule_host(&self) -> Option<Host> {
        Some(Host::Outlook)
    }

    fn server_rules(&mut self) -> Result<Vec<Rule>, String> {
        Ok(self.server_rules.clone())
    }

    fn save_server_rule(&mut self, rule: &Rule) -> Result<Rule, String> {
        let mut r = rule.clone();
        if r.id.is_empty() {
            r.id = "srv-1".into();
        }
        self.server_rules.retain(|x| x.id != r.id);
        self.server_rules.push(r.clone());
        Ok(r)
    }

    fn delete_server_rule(&mut self, id: &str) -> Result<(), String> {
        self.server_rules.retain(|x| x.id != id);
        Ok(())
    }

    fn make_snooze_folder(&mut self) -> Result<Folder, String> {
        let f = folder("MailALM/Snoozed", FolderRole::Other);
        self.folders.push(f.clone());
        Ok(f)
    }

    fn server_search(&mut self, _query: &Query, _folders: &[Folder], _whole: bool) -> Result<Vec<(String, Summary)>, String> {
        let mut s = msg(77, "zoe@studio.example", "From the server", "s77");
        s.flags.seen = false;
        Ok(vec![("INBOX".into(), s)])
    }
}

fn news_rule() -> Rule {
    Rule::new("News", Where::Local, vec![Condition::From(vec!["news@".into()])], vec![Action::MarkRead, Action::MoveTo("News".into())])
}

#[test]
fn own_rules_run_on_new_inbox_mail_only() {
    let mut f = Fake::new("rules-run");
    f.put("INBOX", Summary { flags: Flags::default(), ..msg(1, "news@shop.example", "Old sale", "n1") });
    f.store.save_rules(&[news_rule()]).expect("rules");
    // The first run only says where the Inbox stands: old mail is never swept.
    rules::run_new(&mut f, "INBOX");
    assert!(f.changes.is_empty());
    f.put("INBOX", Summary { flags: Flags::default(), ..msg(2, "news@shop.example", "Sale", "n2") });
    f.put("INBOX", msg(3, "zoe@studio.example", "Storyboards", "z3"));
    rules::run_new(&mut f, "INBOX");
    assert_eq!(
        f.changes,
        vec![
            Change::SetSeen { folder: "INBOX".into(), uids: vec![2], seen: true },
            Change::Move { folder: "INBOX".into(), uids: vec![2], to: "News".into() },
        ]
    );
    assert_eq!(f.store.summaries("News").len(), 1);
    // Nothing new: nothing done; a message a snooze brought back is left alone.
    rules::run_new(&mut f, "INBOX");
    assert_eq!(f.changes.len(), 2);
    let back = msg(4, "news@shop.example", "Back from snooze", "n4");
    let mut later = f.store.later();
    later.woke(Mark::of(&back));
    f.store.save_later(&later).expect("later");
    f.put("INBOX", back);
    rules::run_new(&mut f, "INBOX");
    assert_eq!(f.changes.len(), 2);
}

#[test]
fn rules_are_listed_saved_deleted_and_previewed() {
    let mut f = Fake::new("rules-edit");
    rules::handle(&mut f, RuleCommand::Save(Box::new(news_rule())));
    let local_id = match f.told().pop() {
        Some(Event::Rules { rules, .. }) => {
            assert_eq!(rules.len(), 1);
            assert!(rules[0].id.starts_with("local-"), "{}", rules[0].id);
            rules[0].id.clone()
        }
        other => panic!("{other:?}"),
    };
    let server = Rule { place: Where::Server, ..news_rule() };
    rules::handle(&mut f, RuleCommand::Save(Box::new(server)));
    match f.told().pop() {
        Some(Event::Rules { rules, .. }) => assert_eq!(rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["srv-1", local_id.as_str()]),
        other => panic!("{other:?}"),
    }
    // Refused: a forwarding in MailALM's own rules. Said, and the list given again.
    let fwd = Rule { actions: vec![Action::Forward(vec!["me@elsewhere.example".into()])], forward_confirmed: true, ..news_rule() };
    rules::handle(&mut f, RuleCommand::Save(Box::new(fwd)));
    let told = f.told();
    assert!(matches!(told.first(), Some(Event::Error { text, .. }) if text.contains("never forward")), "{told:?}");
    rules::handle(&mut f, RuleCommand::Delete { id: local_id, place: Where::Local });
    rules::handle(&mut f, RuleCommand::Delete { id: "srv-1".into(), place: Where::Server });
    match f.told().pop() {
        Some(Event::Rules { rules, .. }) => assert!(rules.is_empty()),
        other => panic!("{other:?}"),
    }
    // The preview: the cached messages of the folders asked for.
    f.put("INBOX", msg(1, "news@shop.example", "Sale", "n1"));
    f.put("INBOX", msg(2, "zoe@studio.example", "Hi", "z2"));
    f.put("News", msg(3, "news@shop.example", "Old sale", "n3"));
    rules::handle(&mut f, RuleCommand::Preview { id: 9, rule: Box::new(news_rule()), folders: FolderScope::Folder("INBOX".into()) });
    match f.told().pop() {
        Some(Event::RulePreview { id: 9, hits, .. }) => assert_eq!(hits.iter().map(|(p, s)| (p.as_str(), s.uid)).collect::<Vec<_>>(), [("INBOX", 1)]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn snoozed_messages_come_back_unread() {
    let mut f = Fake::new("snooze");
    let (_tx, commands) = mpsc::channel::<Command>();
    f.put("INBOX", msg(5, "zoe@studio.example", "Storyboards", "m5"));
    let now = now_unix();
    later::handle(&mut f, LaterCommand::Snooze { folder: "INBOX".into(), uid: 5, until: now + 3600 }, &commands);
    assert_eq!(f.changes, vec![Change::Move { folder: "INBOX".into(), uids: vec![5], to: "MailALM/Snoozed".into() }]);
    assert!(f.store.summaries("INBOX").is_empty());
    assert!(matches!(f.told().pop(), Some(Event::Later { snoozed, .. }) if snoozed.len() == 1));
    // Not yet.
    later::tick(&mut f, &commands);
    assert_eq!(f.changes.len(), 1);
    // Its time came: back to the Inbox, unread, and remembered as woken.
    let mut l = f.store.later();
    l.snoozed[0].until = now - 1;
    f.store.save_later(&l).expect("later");
    later::tick(&mut f, &commands);
    assert_eq!(
        f.changes[1..],
        [
            Change::SetSeen { folder: "MailALM/Snoozed".into(), uids: vec![100], seen: false },
            Change::Move { folder: "MailALM/Snoozed".into(), uids: vec![100], to: "INBOX".into() },
        ]
    );
    let inbox = f.store.summaries("INBOX");
    assert_eq!((inbox.len(), inbox[0].message_id.as_str(), inbox[0].flags.seen), (1, "m5", false));
    let l = f.store.later();
    assert!(l.snoozed.is_empty() && l.was_woken(&inbox[0]));
    // A time gone by is refused.
    f.told();
    later::handle(&mut f, LaterCommand::Snooze { folder: "INBOX".into(), uid: inbox[0].uid, until: now - 5 }, &commands);
    assert!(matches!(f.told().first(), Some(Event::Error { .. })));
}

#[test]
fn follow_ups_remind_once_unless_answered() {
    let mut f = Fake::new("follow-up");
    let (_tx, commands) = mpsc::channel::<Command>();
    let now = now_unix();
    f.put("Sent", msg(3, "alex@example.com", "Quote", "q1"));
    f.put("Sent", msg(4, "alex@example.com", "Contract", "q2"));
    later::handle(&mut f, LaterCommand::FollowUp { folder: "Sent".into(), uid: 3, by: now - 1 }, &commands);
    later::handle(&mut f, LaterCommand::FollowUp { folder: "Sent".into(), uid: 4, by: now - 1 }, &commands);
    // Zoe answered the contract; my own message in the quote's thread is no answer.
    f.put("INBOX", Summary { in_reply_to: "q2".into(), ..msg(9, "zoe@studio.example", "Re: Contract", "r9") });
    f.put("Sent", Summary { references: vec!["q1".into()], ..msg(10, "alex@example.com", "Re: Quote", "r10") });
    f.told();
    later::tick(&mut f, &commands);
    let due: Vec<String> = f.told().into_iter().filter_map(|e| if let Event::FollowUpDue { follow_up, .. } = e { Some(follow_up.mark.message_id) } else { None }).collect();
    assert_eq!(due, ["q1"]);
    let l = f.store.later();
    assert_eq!(l.follow_ups.len(), 1, "the answered one is gone");
    assert!(l.follow_ups[0].fired);
    // Given once.
    later::tick(&mut f, &commands);
    assert!(!f.told().iter().any(|e| matches!(e, Event::FollowUpDue { .. })));
    later::handle(&mut f, LaterCommand::Dismiss(Box::new(l.follow_ups[0].mark.clone())), &commands);
    assert!(f.store.later().follow_ups.is_empty());
}

#[test]
fn find_searches_the_cache_then_the_server() {
    let mut f = Fake::new("find");
    f.put("INBOX", msg(1, "zoe@studio.example", "Storyboards", "z1"));
    f.put("News", msg(2, "zoe@studio.example", "Newsletter", "z2"));
    find::handle(&mut f, 5, "from:zoe", &FolderScope::Folder("INBOX".into()), true);
    let told = f.told();
    assert!(matches!(&told[0], Event::SearchResults { id: 5, hits, .. } if hits.len() == 1 && hits[0].0 == "INBOX"), "{told:?}");
    assert!(matches!(&told[1], Event::ServerResults { id: 5, result: Ok(hits), .. } if hits.len() == 1 && hits[0].1.subject == "From the server"), "{told:?}");
    // Another account asked for: nothing searched, the server not asked.
    find::handle(&mut f, 6, "account:home from:zoe", &FolderScope::Account, true);
    let told = f.told();
    assert_eq!(told.len(), 1);
    assert!(matches!(&told[0], Event::SearchResults { hits, .. } if hits.is_empty()));
    // What the server can't judge is checked again on what it found.
    find::handle(&mut f, 7, "zoe is:read", &FolderScope::Account, true);
    let told = f.told();
    assert!(matches!(&told[0], Event::SearchResults { hits, .. } if hits.len() == 2), "{told:?}");
    assert!(matches!(&told[1], Event::ServerResults { result: Ok(hits), .. } if hits.is_empty()), "{told:?}");
}

/// A draft is written (without any recipient yet), saved in place of the one before, said to
/// the compose window, and deleted once sent.
#[test]
fn drafts_are_saved_replaced_and_deleted() {
    let mut w = Fake::new("drafts");
    let (_tx, rx) = mpsc::channel::<Command>();
    let message = mailalm_mime::Outgoing { from: Person { name: String::new(), address: "alex@example.com".into() }, subject: "Plan".into(), ..Default::default() };
    drafts::save(&mut w, 7, &message, None, &rx);
    let told = w.told();
    let Some(Event::Draft { compose: 7, result: Ok(first), .. }) = told.first() else { panic!("{told:?}") };
    assert!(first.ends_with("@example.com"), "named by its Message-ID");
    drafts::save(&mut w, 7, &message, Some(first.as_str()), &rx);
    assert_eq!(w.drafts.last().and_then(|(_, replaced)| replaced.clone()).as_deref(), Some(first.as_str()));
    let told = w.told();
    let Some(Event::Draft { result: Ok(second), .. }) = told.first() else { panic!("{told:?}") };
    drafts::delete(&mut w, second, &rx);
    assert!(!w.drafts.iter().any(|(id, _)| id == second));
    // An address that isn't one: said, nothing saved.
    let bad = mailalm_mime::Outgoing { to: vec![Person { name: String::new(), address: "nope".into() }], ..message };
    drafts::save(&mut w, 8, &bad, None, &rx);
    assert!(matches!(w.told().first(), Some(Event::Draft { compose: 8, result: Err(e), .. }) if e.contains("nope")));
}
