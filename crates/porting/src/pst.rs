//! Outlook data files (`.pst`, `.ost`): their mail, folder by folder, each item a standard
//! message again.
//!
//! The file is read with `mailalm-pst` (MailCraft's reader, read only: the file is opened for
//! reading and never written). The folders under the top of the mailbox (the IPM subtree) become
//! folder paths; search folders are passed over (their messages are in other folders). Each mail
//! item becomes a message (`rebuild`): its original header when the file kept it, else one made
//! from its properties, with its text, its HTML and its attachments, attached messages included.
//! Contacts, appointments, tasks and notes are not mail: they are counted and left out.
//!
//! What a data file holds is hostile like any received mail: one value is read up to
//! [`MAX_MESSAGE`], the folders are walked at most [`MAX_FOLDERS`], [`MAX_DEPTH`] deep, each once,
//! and a message that comes out larger than [`MAX_MESSAGE`] is left out.
//!
//! Not read: the files of Outlook 2013 and later that keep 4 KiB pages (version 36, most `.ost`
//! files of recent Outlook versions), which the reader refuses; a body kept only as compressed
//! Rich Text (the message comes in without that body).

mod rebuild;

use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use mailalm_pst::{Message, PstFile, PstStore};

use crate::{Cancel, Item, MAX_DEPTH, MAX_FOLDERS, MAX_MESSAGE, Report, Run, Sink, Skip, folder_name};

/// A folder node's type: a search folder (its low five bits).
const NID_TYPE_SEARCH_FOLDER: u32 = 0x03;

/// Imports the mail of the Outlook data file at `path`. An error when it is not one, or its
/// store or its folder list can't be read.
pub fn import(path: &Path, sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let file = PstFile::open(path).map_err(|e| opening_error(&e))?;
    file.set_max_value_size(MAX_MESSAGE as u64);
    import_from(file, sink, cancel)
}

/// Imports the mail of a data file already open (the tests read one from memory).
pub fn import_from(file: PstFile, sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let store = PstStore::read(Rc::new(file)).map_err(|e| format!("This Outlook data file is damaged: {e}"))?;
    let top = store.ipm_subtree().map_err(|e| format!("This Outlook data file is damaged: {e}"))?;
    let mut run = Run::new(sink, cancel);
    let mut seen: HashSet<u32> = HashSet::new();
    // Each folder to read: its node, the path of the folder it is in, and whether it is the top.
    let mut todo: Vec<(u32, Vec<String>, bool)> = vec![(top, Vec::new(), true)];
    let mut listed = 1usize;
    while let Some((nid, parent, is_top)) = todo.pop() {
        if run.stopped() {
            break;
        }
        // A folder named twice (a damaged or hostile file names its own parent) is read once.
        if !seen.insert(nid) || nid & 0x1F == NID_TYPE_SEARCH_FOLDER {
            run.report.left_out += 1;
            continue;
        }
        let Ok(folder) = store.open_folder(nid) else {
            run.report.left_out += 1;
            continue;
        };
        let mut path = parent;
        if !is_top {
            path.push(folder_name(&folder.properties().display_name().unwrap_or_default()));
        }
        run.report.folders += 1;
        match folder.read_contents_table() {
            Ok(Some(table)) => {
                let ids: Vec<u32> = table.rows_matrix().map(|r| r.id()).collect();
                drop(table);
                run.progress.total = run.progress.total.saturating_add(ids.len() as u64);
                for id in ids {
                    if run.stopped() {
                        break;
                    }
                    one(&mut run, &store, id, &path)?;
                    run.progress.done += 1;
                }
            }
            Ok(None) => {}
            Err(_) => run.report.left_out += 1,
        }
        match folder.read_hierarchy_table() {
            Ok(Some(table)) => {
                let mut children = Vec::new();
                for row in table.rows_matrix() {
                    if path.len() >= MAX_DEPTH || listed >= MAX_FOLDERS {
                        run.report.left_out += 1;
                        continue;
                    }
                    listed += 1;
                    children.push((row.id(), path.clone(), false));
                }
                // Last first, so they come off the stack in order.
                todo.extend(children.into_iter().rev());
            }
            Ok(None) => {}
            Err(_) => run.report.left_out += 1,
        }
    }
    Ok(run.finish())
}

/// What the user is told when the file can't be opened.
fn opening_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        Some(32 | 33) => "Outlook (or another program) has this data file open: close it, and try again.".to_owned(),
        _ => format!("This is not an Outlook data file that MailALM can read, or it is damaged ({e})."),
    }
}

/// The message class of a mail item: a note (an e-mail), a post, a meeting request or answer, a
/// delivery report. Contacts, appointments, tasks, notes and the rest are not mail.
fn is_mail(class: &str) -> bool {
    let c = class.trim().to_ascii_lowercase();
    c.is_empty() || c == "ipm" || c.starts_with("ipm.note") || c.starts_with("ipm.post") || c.starts_with("ipm.schedule.meeting") || c.starts_with("report.")
}

/// One item of a folder.
fn one(run: &mut Run<'_, '_>, store: &Rc<PstStore>, nid: u32, folder: &[String]) -> Result<(), String> {
    let Ok(message) = Message::read(store.clone(), nid) else {
        run.skip(Skip::Unreadable);
        return Ok(());
    };
    if !is_mail(&rebuild::message_class(&message)) {
        run.skip(Skip::NotMail);
        return Ok(());
    }
    let mut budget = rebuild::Budget::new();
    let mut left_out = 0u64;
    match rebuild::rebuild(&message, 0, &mut budget, &mut left_out) {
        Ok(raw) => {
            run.report.left_out += left_out;
            let flags = rebuild::flags(message.properties());
            run.message(Item { folder: folder.to_vec(), raw, flags })
        }
        Err(why) => {
            run.skip(why);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
