//! What every kind of account does the same way with its cache: changes shown before the server
//! has them, and the search.

use mailalm_model::pim::ZoneRules;
use mailalm_model::pim::ical::{self, Invitation};
use mailalm_model::{AccountId, Change, Folder, Query, Summary};
use mailalm_store::Store;

use crate::{Event, Reviewed};

/// The most results a search returns per account.
const MAX_HITS: usize = 1_000;

/// A change as the message list shows it: flags changed, or messages gone from the folder.
pub(crate) fn apply_to(summaries: &mut Vec<Summary>, change: &Change) {
    match change {
        Change::SetSeen { uids, seen, .. } => summaries.iter_mut().filter(|s| uids.contains(&s.uid)).for_each(|s| s.flags.seen = *seen),
        Change::SetFlagged { uids, flagged, .. } => summaries.iter_mut().filter(|s| uids.contains(&s.uid)).for_each(|s| s.flags.flagged = *flagged),
        Change::Move { uids, .. } | Change::Delete { uids, .. } | Change::MarkJunk { uids, .. } => summaries.retain(|s| !uids.contains(&s.uid)),
    }
}

/// The messages a change takes out of its folder.
pub(crate) fn removed_by(change: &Change) -> &[u32] {
    match change {
        Change::Move { uids, .. } | Change::Delete { uids, .. } | Change::MarkJunk { uids, .. } => uids,
        Change::SetSeen { .. } | Change::SetFlagged { .. } => &[],
    }
}

/// Changes not yet on the server, applied over what it just said.
pub(crate) fn overlay_pending(store: &Store, path: &str, summaries: &mut Vec<Summary>) {
    for change in store.pending().iter().filter(|c| c.folder() == path) {
        apply_to(summaries, change);
    }
}

/// Searches the cache of an account, newest first.
pub(crate) fn search(store: &Store, folders: &[Folder], query: &str) -> Vec<(String, Summary)> {
    let q = Query::parse(query);
    let mut hits = Vec::new();
    if !q.is_empty() {
        let needs_text = q.terms.iter().any(|t| matches!(t, mailalm_model::query::Term::Text(_)));
        'folders: for f in folders {
            for s in store.summaries(&f.path) {
                let text = if needs_text { store.text(&f.path, s.uid) } else { None };
                if q.matches(&s, &f.path, text.as_deref()) {
                    hits.push((f.path.clone(), s));
                    if hits.len() >= MAX_HITS {
                        break 'folders;
                    }
                }
            }
        }
    }
    hits.sort_by(|a, b| b.1.date.cmp(&a.1.date));
    hits
}

/// An attachment the user asked to review: decoded from the message in the cache (the reading
/// pane opened it, so it is there) and looked at. Nothing else ever decodes an attachment.
pub(crate) fn review(store: &Store, account: AccountId, folder: &str, uid: u32, index: usize) -> Event {
    let result = match store.message(folder, uid) {
        None => Err("This message is no longer in the cache: open it again, then review the attachment.".to_owned()),
        Some(raw) => mailalm_mime::attachment(&raw, index).map(|data| {
            let report = mailalm_safety::assess(&mailalm_safety::Input {
                raw_name: &data.raw_name,
                name: &data.info.name,
                declared_type: &data.info.declared_type,
                bytes: &data.bytes,
            });
            Box::new(Reviewed { data, report })
        }),
    };
    Event::Attachment { account, folder: folder.to_owned(), uid, index, result }
}

/// The largest calendar part read as an invitation.
const MAX_INVITATION: usize = 1 << 20;

/// Whether a part of a message, as listed, is a calendar (an invitation, an answer, a
/// cancellation): `text/calendar`, `application/ics`, or a name ending in `.ics`.
pub fn is_calendar_part(declared_type: &str, name: &str) -> bool {
    matches!(declared_type.trim().to_ascii_lowercase().as_str(), "text/calendar" | "application/ics") || name.trim().to_ascii_lowercase().ends_with(".ics")
}

/// The calendar part of a message the reading pane shows, decoded (that part only, at most
/// [`MAX_INVITATION`] bytes) and read as an invitation with MailALM's own iCalendar reader, which
/// caps and cleans everything it keeps. Nothing else of the message is decoded.
pub(crate) fn invitation(store: &Store, account: AccountId, folder: &str, uid: u32, index: usize, zones: &dyn ZoneRules) -> Event {
    let read = || -> Result<Box<Invitation>, String> {
        let raw = store.message(folder, uid).ok_or("This message is no longer in the cache: open it again.")?;
        let parsed = mailalm_mime::parse(&raw).ok_or("This message could not be read.")?;
        let info = parsed.attachments.get(index).ok_or("This part is no longer in the message.")?;
        if !is_calendar_part(&info.declared_type, &info.name) {
            return Err("This part of the message is not a calendar.".to_owned());
        }
        if info.size > MAX_INVITATION {
            return Err(format!("The calendar in this message is larger than {} KB: MailALM does not read it.", MAX_INVITATION >> 10));
        }
        let data = mailalm_mime::attachment(&raw, index)?;
        let bytes = data.bytes.get(..MAX_INVITATION.min(data.bytes.len())).unwrap_or_default();
        let invitation = ical::parse(&String::from_utf8_lossy(bytes), zones);
        if invitation.events.is_empty() {
            return Err("The calendar in this message has no event MailALM can read.".to_owned());
        }
        Ok(Box::new(invitation))
    };
    Event::Invitation { account, folder: folder.to_owned(), uid, index, result: read() }
}

pub(crate) fn sort_newest_first(s: &mut [Summary]) {
    s.sort_by(|a, b| b.date.cmp(&a.date).then(b.uid.cmp(&a.uid)));
}

/// The counts the folder list shows, from the summaries.
pub(crate) fn counts(summaries: &[Summary]) -> (u32, u32) {
    let total = u32::try_from(summaries.len()).unwrap_or(u32::MAX);
    let unread = u32::try_from(summaries.iter().filter(|s| !s.flags.seen).count()).unwrap_or(u32::MAX);
    (total, unread)
}
