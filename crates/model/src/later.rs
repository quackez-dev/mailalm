//! Snoozed messages and follow-ups (PLAN.md, section 14.1, C2), kept by MailALM: no provider has
//! an API for them.
//!
//! - **Snooze**: the message goes to a folder of MailALM's (`MailALM/Snoozed`, a label of that
//!   name for Gmail), and comes back to the Inbox, unread, at the time chosen. It is found again
//!   there by its Message-ID (or, without one, by its date and subject): a message changes its
//!   number when it moves.
//! - **Follow-up**: a reminder at a date if no answer came in the message's thread by then. The
//!   thread is followed through the cache: a message that names it in `In-Reply-To` or
//!   `References`, or that the provider puts in the same thread, from someone else than the
//!   account.
//!
//! Both are kept in the account's cache, and checked by its worker every minute and at start.

use serde::{Deserialize, Serialize};

use crate::mail::{Folder, Person, Summary};

/// The folder (Gmail: the label) snoozed messages wait in: `MailALM`, and under it `Snoozed`.
pub const SNOOZE_PARENT: &str = "MailALM";
pub const SNOOZE_LEAF: &str = "Snoozed";
/// The most snoozed messages and follow-ups kept for an account.
pub const MAX_KEPT: usize = 5_000;
/// The messages woken lately, remembered so that MailALM's rules don't move them again.
pub const MAX_WOKEN: usize = 200;

/// Whether a folder is the one snoozed messages wait in: `MailALM/Snoozed` (under the Inbox too,
/// for servers that keep every folder there: `INBOX.MailALM.Snoozed`).
pub fn is_snooze_folder(f: &Folder) -> bool {
    let Some(d) = f.delimiter else { return false };
    let mut parts = f.path.rsplit(d);
    let leaf = parts.next().unwrap_or_default();
    let parent = parts.next().unwrap_or_default();
    leaf.eq_ignore_ascii_case(SNOOZE_LEAF) && parent.eq_ignore_ascii_case(SNOOZE_PARENT)
}

/// The folder snoozed messages wait in, among an account's.
pub fn snooze_folder(folders: &[Folder]) -> Option<&Folder> {
    folders.iter().find(|f| is_snooze_folder(f))
}

/// How a message is found again after it moved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// Without angle brackets; empty when it has none.
    pub message_id: String,
    pub date: i64,
    pub subject: String,
    pub from: Person,
}

impl Mark {
    pub fn of(s: &Summary) -> Mark {
        Mark { message_id: s.message_id.clone(), date: s.date, subject: s.subject.clone(), from: s.from.clone() }
    }

    /// Whether `s` is the message: by Message-ID, else by date and subject.
    pub fn is(&self, s: &Summary) -> bool {
        if !self.message_id.is_empty() {
            return s.message_id == self.message_id;
        }
        s.message_id.is_empty() && s.date == self.date && s.subject == self.subject
    }
}

/// A message snoozed until a time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snoozed {
    pub mark: Mark,
    /// When it comes back (UTC seconds).
    pub until: i64,
    /// When it was snoozed.
    pub since: i64,
}

/// A reminder if no answer came in a message's thread by a date.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUp {
    pub mark: Mark,
    /// The provider's thread id, if it has one.
    #[serde(default)]
    pub thread: String,
    /// Where the message was when the follow-up was set (to open it from the reminder).
    pub folder: String,
    pub uid: u32,
    /// When the reminder comes if no answer came (UTC seconds).
    pub by: i64,
    /// When the follow-up was set: answers before it don't count.
    pub since: i64,
    /// The reminder was given: kept until the user dismisses it.
    #[serde(default)]
    pub fired: bool,
}

/// What an account keeps of both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Later {
    #[serde(default)]
    pub snoozed: Vec<Snoozed>,
    #[serde(default)]
    pub follow_ups: Vec<FollowUp>,
    /// The messages brought back lately: MailALM's rules leave them alone.
    #[serde(default)]
    pub woken: Vec<Mark>,
}

impl Later {
    /// The snoozed messages whose time has come, the earliest first.
    pub fn due_snoozed(&self, now: i64) -> Vec<Snoozed> {
        let mut due: Vec<Snoozed> = self.snoozed.iter().filter(|s| s.until <= now).cloned().collect();
        due.sort_by_key(|s| s.until);
        due
    }

    /// The follow-ups whose date has come and whose reminder wasn't given yet.
    pub fn due_follow_ups(&self, now: i64) -> Vec<FollowUp> {
        self.follow_ups.iter().filter(|f| !f.fired && f.by <= now).cloned().collect()
    }

    /// Snoozes a message (again, at the new time, if it already was).
    pub fn snooze(&mut self, s: Snoozed) {
        self.snoozed.retain(|x| x.mark != s.mark);
        if self.snoozed.len() < MAX_KEPT {
            self.snoozed.push(s);
        }
    }

    /// Follows a message up (again, at the new date, if it already was).
    pub fn follow(&mut self, f: FollowUp) {
        self.follow_ups.retain(|x| x.mark != f.mark);
        if self.follow_ups.len() < MAX_KEPT {
            self.follow_ups.push(f);
        }
    }

    /// Remembers a message woken, for the rules.
    pub fn woke(&mut self, m: Mark) {
        self.woken.retain(|x| *x != m);
        self.woken.push(m);
        if self.woken.len() > MAX_WOKEN {
            let extra = self.woken.len() - MAX_WOKEN;
            self.woken.drain(..extra);
        }
    }

    /// Whether a message was woken lately.
    pub fn was_woken(&self, s: &Summary) -> bool {
        self.woken.iter().any(|m| m.is(s))
    }
}

/// Whether an answer to a followed message came: a message of the cache, from someone else than
/// `me`, dated after the follow-up was set (a day of margin for clocks and delays), that names
/// the message in `In-Reply-To` or `References` or shares its provider's thread.
pub fn answered<'a>(f: &FollowUp, me: &str, messages: impl IntoIterator<Item = &'a Summary>) -> bool {
    let id = f.mark.message_id.as_str();
    messages.into_iter().any(|m| {
        let in_thread = (!id.is_empty() && (m.in_reply_to == id || m.references.iter().any(|r| r == id)))
            || (!f.thread.is_empty() && m.thread == f.thread);
        in_thread && !f.mark.is(m) && !m.from.address.eq_ignore_ascii_case(me.trim()) && m.date >= f.since.saturating_sub(86_400)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FolderRole;

    fn folder(path: &str, d: char) -> Folder {
        Folder { path: path.into(), wire: path.into(), delimiter: Some(d), role: FolderRole::Other, total: 0, unread: 0 }
    }

    fn sent() -> Summary {
        Summary {
            uid: 4,
            from: Person { name: "Alex".into(), address: "alex@example.com".into() },
            subject: "Quote for the storyboards".into(),
            date: 1_000_000,
            message_id: "q1@example.com".into(),
            ..Summary::default()
        }
    }

    fn follow_up() -> FollowUp {
        FollowUp { mark: Mark::of(&sent()), thread: String::new(), folder: "Sent".into(), uid: 4, by: 1_300_000, since: 1_000_000, fired: false }
    }

    #[test]
    fn the_snooze_folder_is_found_under_any_delimiter() {
        assert!(is_snooze_folder(&folder("MailALM/Snoozed", '/')));
        assert!(is_snooze_folder(&folder("INBOX.MailALM.Snoozed", '.')));
        assert!(!is_snooze_folder(&folder("Snoozed", '/')));
        assert!(!is_snooze_folder(&folder("Other/Snoozed", '/')));
        let all = [folder("INBOX", '/'), folder("MailALM", '/'), folder("MailALM/Snoozed", '/')];
        assert_eq!(snooze_folder(&all).map(|f| f.path.as_str()), Some("MailALM/Snoozed"));
    }

    #[test]
    fn snoozed_messages_come_due_in_order() {
        let mut later = Later::default();
        let mark = |id: &str| Mark { message_id: id.into(), ..Mark::default() };
        later.snooze(Snoozed { mark: mark("b"), until: 200, since: 0 });
        later.snooze(Snoozed { mark: mark("a"), until: 100, since: 0 });
        later.snooze(Snoozed { mark: mark("c"), until: 900, since: 0 });
        // Snoozed again: the new time replaces the old.
        later.snooze(Snoozed { mark: mark("c"), until: 150, since: 0 });
        assert_eq!(later.snoozed.len(), 3);
        let due: Vec<String> = later.due_snoozed(199).into_iter().map(|s| s.mark.message_id).collect();
        assert_eq!(due, ["a", "c"]);
        assert!(later.due_snoozed(99).is_empty());
    }

    #[test]
    fn a_message_is_found_again_by_id_or_by_date_and_subject() {
        let s = sent();
        assert!(Mark::of(&s).is(&Summary { uid: 99, ..s.clone() }));
        assert!(!Mark::of(&s).is(&Summary { message_id: "other@x".into(), ..s.clone() }));
        let no_id = Summary { message_id: String::new(), ..s.clone() };
        assert!(Mark::of(&no_id).is(&Summary { uid: 12, ..no_id.clone() }));
        assert!(!Mark::of(&no_id).is(&Summary { subject: "Re: x".into(), ..no_id.clone() }));
    }

    #[test]
    fn follow_ups_see_answers_in_the_thread() {
        let f = follow_up();
        let me = "Alex@Example.com";
        let reply = Summary {
            uid: 8,
            from: Person { name: "Zoe".into(), address: "zoe@studio.example".into() },
            date: 1_100_000,
            in_reply_to: "q1@example.com".into(),
            message_id: "r1@studio.example".into(),
            ..Summary::default()
        };
        assert!(answered(&f, me, [&sent(), &reply]));
        // Further down the thread: by References.
        let later_reply = Summary { in_reply_to: "other@x".into(), references: vec!["q1@example.com".into(), "other@x".into()], ..reply.clone() };
        assert!(answered(&f, me, [&later_reply]));
        // My own message in the thread, the message itself, an old one and a stranger's are no answer.
        let mine = Summary { from: Person { name: String::new(), address: "alex@example.com".into() }, ..reply.clone() };
        assert!(!answered(&f, me, [&mine, &sent()]));
        let old = Summary { date: 800_000, ..reply.clone() };
        assert!(!answered(&f, me, [&old]));
        let stranger = Summary { in_reply_to: "else@x".into(), ..reply.clone() };
        assert!(!answered(&f, me, [&stranger]));
        // The provider's thread.
        let f = FollowUp { thread: "AAQk-thread".into(), ..follow_up() };
        assert!(answered(&f, me, [&Summary { in_reply_to: String::new(), thread: "AAQk-thread".into(), ..reply }]));
    }

    #[test]
    fn follow_ups_fire_once_and_woken_messages_are_remembered() {
        let mut later = Later::default();
        later.follow(follow_up());
        assert_eq!(later.due_follow_ups(1_300_000).len(), 1);
        assert!(later.due_follow_ups(1_299_999).is_empty());
        if let Some(f) = later.follow_ups.first_mut() {
            f.fired = true;
        }
        assert!(later.due_follow_ups(2_000_000).is_empty());
        for n in 0..(MAX_WOKEN + 5) {
            later.woke(Mark { message_id: format!("m{n}"), ..Mark::default() });
        }
        assert_eq!(later.woken.len(), MAX_WOKEN);
        assert!(!later.was_woken(&Summary { message_id: "m0".into(), ..Summary::default() }));
        assert!(later.was_woken(&Summary { message_id: format!("m{}", MAX_WOKEN + 4), ..Summary::default() }));
    }
}
