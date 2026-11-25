//! What every kind of account does the same way with its cache: changes shown before the server
//! has them, and the search.

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

pub(crate) fn sort_newest_first(s: &mut [Summary]) {
    s.sort_by(|a, b| b.date.cmp(&a.date).then(b.uid.cmp(&a.uid)));
}

/// The counts the folder list shows, from the summaries.
pub(crate) fn counts(summaries: &[Summary]) -> (u32, u32) {
    let total = u32::try_from(summaries.len()).unwrap_or(u32::MAX);
    let unread = u32::try_from(summaries.iter().filter(|s| !s.flags.seen).count()).unwrap_or(u32::MAX);
    (total, unread)
}
