//! The search with its folders (PLAN.md, section 6): the cache first, at once, then, when asked,
//! the server (Graph `$search`, Gmail `q`, IMAP `SEARCH`), whose messages come apart, "from the
//! server". What a server can't judge (marks, attachments, categories, the folder) is checked
//! again on what it found.

use mailalm_model::{FolderScope, Query};

use crate::mail_worker::{MAX_SERVER_HITS, MailWorker};
use crate::{Event, local};

pub(crate) fn handle<W: MailWorker>(w: &mut W, id: u64, query: &str, scope: &FolderScope, server: bool) {
    let q = Query::parse(query);
    let (name, address) = w.me();
    let wanted = q.wants_account(&name, &address);
    let folders: Vec<_> = scope.select(&w.mail_folders()).into_iter().cloned().collect();
    let hits = if wanted { local::search_parsed(w.cache(), &folders, &q) } else { Vec::new() };
    w.tell(Event::SearchResults { account: w.who(), id, hits });
    if !server || !wanted || q.is_empty() {
        return;
    }
    let result = if w.reach() {
        w.server_search(&q, &folders, *scope == FolderScope::Account).map(|mut found| {
            let check = q.metadata_only();
            found.retain(|(folder, s)| check.matches(s, folder, None));
            found.sort_by(|a, b| b.1.date.cmp(&a.1.date));
            found.truncate(MAX_SERVER_HITS);
            found
        })
    } else {
        Err("The server can't be reached: only the messages in MailALM were searched.".to_owned())
    };
    w.tell(Event::ServerResults { account: w.who(), id, result });
}
