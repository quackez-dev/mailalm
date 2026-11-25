//! Drafts kept on the account's server (PLAN.md section 8), the same way for every kind of
//! account: the compose window asks to save one (on demand, and when it is closed with something
//! in it), saved again in place of the last one, and deleted once the message is sent or
//! discarded. Where it goes is the worker's ([`MailWorker::store_draft`]): IMAP `APPEND` to the
//! Drafts folder with `\Draft`, a Graph message made from MIME (which Graph puts in Drafts), a
//! Gmail draft.

use std::sync::mpsc::Receiver;

use mailalm_mime::Outgoing;
use mailalm_model::FolderRole;

use crate::mail_worker::{MailWorker, folder_with_role};
use crate::{Command, Event};

/// Saves `message` as a draft in place of `replace`; [`Event::Draft`] says how it went. The
/// Drafts folder is synced after, to show it.
pub(crate) fn save<W: MailWorker>(w: &mut W, compose: u64, message: &Outgoing, replace: Option<&str>, commands: &Receiver<Command>) {
    let result = match mailalm_mime::build_draft(message) {
        Ok((raw, message_id)) if w.reach() => w.store_draft(&raw, &message_id, replace),
        Ok(_) => Err("the server cannot be reached".to_owned()),
        Err(e) => Err(e),
    };
    let saved = result.is_ok();
    w.tell(Event::Draft { account: w.who(), compose, result: result.map_err(|e| format!("The draft was not saved on the server: {e}")) });
    if saved && let Some(drafts) = folder_with_role(&*w, FolderRole::Drafts) {
        w.refresh(&drafts.path, commands);
    }
}

/// Deletes a draft saved before; a failure is said (the draft then stays in Drafts).
pub(crate) fn delete<W: MailWorker>(w: &mut W, draft: &str, commands: &Receiver<Command>) {
    let done = if w.reach() { w.drop_draft(draft) } else { Err("the server cannot be reached".to_owned()) };
    match done {
        Ok(()) => {
            if let Some(drafts) = folder_with_role(&*w, FolderRole::Drafts) {
                w.refresh(&drafts.path, commands);
            }
        }
        Err(e) => w.tell(Event::Error { account: w.who(), text: format!("The saved draft could not be deleted from Drafts: {e}") }),
    }
}

/// The HTML version of a raw message and the pictures it names (`cid:`), for the reading pane;
/// `None` when it has none. Nothing in it is run or fetched: the interface converts it.
pub(crate) fn html_of(raw: &[u8]) -> Option<Box<mailalm_mime::HtmlBody>> {
    let html = mailalm_mime::html_body(raw)?;
    Some(Box::new(mailalm_mime::HtmlBody { html, images: mailalm_mime::related_images(raw) }))
}
