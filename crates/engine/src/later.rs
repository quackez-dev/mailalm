//! Snoozed messages and follow-ups (C2), whatever the account's kind: no provider has an API for
//! them, so MailALM keeps them in the account's cache and checks them every minute and at start.
//!
//! A snoozed message is moved to `MailALM/Snoozed` (made on the server the first time: a folder,
//! or a Gmail label), and at its time moved back to the Inbox, unread. It is found again by its
//! mark ([`Mark`]): its number changes when it moves, and the folder is synced to see it there.
//! A follow-up whose date came gives [`Event::FollowUpDue`] unless an answer came in its thread
//! (from someone else than the account) since it was set.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use mailalm_model::later::{FollowUp, Later, Mark, Snoozed, answered, snooze_folder};
use mailalm_model::{Change, FolderRole, Summary};
use mailalm_net::oauth::now_unix;

use crate::mail_worker::{MailWorker, folder_with_role};
use crate::{Command, Event, LaterCommand};

/// How often the workers look for what is due.
pub(crate) const EVERY: Duration = Duration::from_secs(60);
/// How long after its time a snoozed message the server's folder doesn't have is looked for
/// before it is given up (moved or deleted elsewhere).
const GIVE_UP_AFTER: i64 = 3600;

pub(crate) fn handle<W: MailWorker>(w: &mut W, command: LaterCommand, commands: &Receiver<Command>) {
    let done = match command {
        LaterCommand::List => Ok(()),
        LaterCommand::Snooze { folder, uid, until } => snooze(w, &folder, uid, until),
        LaterCommand::Wake(mark) => {
            let mut later = w.cache().later();
            let now = now_unix();
            for s in later.snoozed.iter_mut().filter(|s| s.mark == *mark) {
                s.until = s.until.min(now);
            }
            save(&*w, &later);
            tick(w, commands);
            Ok(())
        }
        LaterCommand::FollowUp { folder, uid, by } => follow_up(w, &folder, uid, by),
        LaterCommand::Dismiss(mark) => {
            let mut later = w.cache().later();
            later.follow_ups.retain(|f| f.mark != *mark);
            save(&*w, &later);
            Ok(())
        }
    };
    if let Err(e) = done {
        w.tell(Event::Error { account: w.who(), text: e });
    }
    show(&*w);
}

/// Tells the interface what is snoozed and followed.
pub(crate) fn show<W: MailWorker>(w: &W) {
    let later = w.cache().later();
    w.tell(Event::Later { account: w.who(), snoozed: later.snoozed, follow_ups: later.follow_ups });
}

fn save<W: MailWorker>(w: &W, later: &Later) {
    if let Err(e) = w.cache().save_later(later) {
        w.tell(Event::Error { account: w.who(), text: format!("The snoozed messages and follow-ups could not be kept: {e}") });
    }
}

fn summary_of<W: MailWorker>(w: &W, folder: &str, uid: u32) -> Result<Summary, String> {
    w.cache().summaries(folder).into_iter().find(|s| s.uid == uid).ok_or_else(|| "This message is no longer in the folder.".to_owned())
}

fn snooze<W: MailWorker>(w: &mut W, folder: &str, uid: u32, until: i64) -> Result<(), String> {
    let now = now_unix();
    if until <= now {
        return Err("A message is snoozed until a time to come.".to_owned());
    }
    let s = summary_of(&*w, folder, uid)?;
    let target = match snooze_folder(&w.mail_folders()).cloned() {
        Some(f) => f,
        None => {
            if !w.reach() {
                return Err("Snoozing needs the server the first time, to make the MailALM/Snoozed folder: try again once online.".to_owned());
            }
            w.make_snooze_folder().map_err(|e| format!("The MailALM/Snoozed folder could not be made: {e}"))?
        }
    };
    let mut later = w.cache().later();
    later.snooze(Snoozed { mark: Mark::of(&s), until, since: now });
    save(&*w, &later);
    if target.path != folder {
        w.change(Change::Move { folder: folder.to_owned(), uids: vec![uid], to: target.path });
    }
    Ok(())
}

fn follow_up<W: MailWorker>(w: &mut W, folder: &str, uid: u32, by: i64) -> Result<(), String> {
    let s = summary_of(&*w, folder, uid)?;
    let mut later = w.cache().later();
    later.follow(FollowUp { mark: Mark::of(&s), thread: s.thread.clone(), folder: folder.to_owned(), uid, by, since: now_unix(), fired: false });
    save(&*w, &later);
    Ok(())
}

/// Every minute and at start: the snoozed messages whose time came go back to the Inbox; the
/// follow-ups whose date came are judged.
pub(crate) fn tick<W: MailWorker>(w: &mut W, commands: &Receiver<Command>) {
    let now = now_unix();
    let mut later = w.cache().later();
    let mut changed = false;
    let due = later.due_snoozed(now);
    if !due.is_empty() {
        changed |= wake(w, &mut later, &due, now, commands);
    }
    let due = later.due_follow_ups(now);
    if !due.is_empty() {
        let (_, me) = w.me();
        let all: Vec<Summary> = w.mail_folders().iter().flat_map(|f| w.cache().summaries(&f.path)).collect();
        for f in due {
            if answered(&f, &me, &all) {
                later.follow_ups.retain(|x| x.mark != f.mark);
            } else {
                if let Some(x) = later.follow_ups.iter_mut().find(|x| x.mark == f.mark) {
                    x.fired = true;
                }
                w.tell(Event::FollowUpDue { account: w.who(), follow_up: Box::new(FollowUp { fired: true, ..f }) });
            }
            changed = true;
        }
    }
    if changed {
        save(&*w, &later);
        show(&*w);
    }
}

/// Brings the due snoozed messages back to the Inbox, unread. Whether anything changed.
fn wake<W: MailWorker>(w: &mut W, later: &mut Later, due: &[Snoozed], now: i64, commands: &Receiver<Command>) -> bool {
    let folders = w.mail_folders();
    let (Some(snoozed), Some(inbox)) = (snooze_folder(&folders).cloned(), folder_with_role(&*w, FolderRole::Inbox)) else {
        // Not listed yet (offline at start): next time.
        return false;
    };
    let find = |w: &W, m: &Mark| w.cache().summaries(&snoozed.path).into_iter().find(|s| m.is(s)).map(|s| s.uid);
    // Moved there since the last sync: its number there is known once the folder is synced.
    let mut synced = false;
    if due.iter().any(|s| find(&*w, &s.mark).is_none()) && w.reach() {
        w.refresh(&snoozed.path, commands);
        synced = true;
    }
    let mut changed = false;
    for s in due {
        match find(&*w, &s.mark) {
            Some(uid) => {
                w.change(Change::SetSeen { folder: snoozed.path.clone(), uids: vec![uid], seen: false });
                w.change(Change::Move { folder: snoozed.path.clone(), uids: vec![uid], to: inbox.path.clone() });
                later.snoozed.retain(|x| x.mark != s.mark);
                later.woke(s.mark.clone());
                changed = true;
            }
            // Not in the folder the server just gave, an hour after its time (moved or deleted
            // elsewhere): given up, and said. Offline, it is looked for again later.
            None if synced && s.until < now - GIVE_UP_AFTER => {
                later.snoozed.retain(|x| x.mark != s.mark);
                changed = true;
                let what = if s.mark.subject.is_empty() { "A snoozed message".to_owned() } else { format!("\"{}\"", s.mark.subject) };
                w.tell(Event::Notice { account: w.who(), text: format!("{what} was no longer in MailALM/Snoozed: it wasn't brought back.") });
            }
            None => {}
        }
    }
    changed
}
