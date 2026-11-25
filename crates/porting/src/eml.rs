//! Messages as `.eml` files: one message's source, byte for byte, in a file of its own.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use mailalm_model::Flags;
use mailalm_store::Store;

use crate::{Cancel, Item, MAX_MESSAGE, Report, Run, Sink, Skip, mbox, walk};

/// Writes message `uid` of `folder`, as the cache keeps it (the source the server sent), to the
/// file `to`: the same bytes, written beside and renamed over it in one step.
pub fn export(store: &Store, folder: &str, uid: u32, to: &Path) -> io::Result<()> {
    let raw = store
        .message(folder, uid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "This message is not in the cache yet: open it once, then export it."))?;
    mailalm_store::write_atomic(to, &raw)
}

/// A file name to suggest for a message saved as `extension` (`eml`, `pdf`): its subject made a
/// safe file name (no folder, no character Windows refuses, not a device name such as `CON`), at
/// most 80 characters; `message` when nothing is left.
pub fn file_name(subject: &str, extension: &str) -> String {
    // A subject's slashes are no folders.
    let flat: String = subject.chars().map(|c| if matches!(c, '/' | '\\') { '_' } else { c }).collect();
    let cleaned = mailalm_mime::clean_file_name(&flat);
    let stem: String = cleaned.chars().filter(|c| !c.is_control()).take(80).collect();
    let stem = stem.trim().trim_end_matches(['.', ' ']).to_owned();
    let stem = if stem.is_empty() { "message".to_owned() } else { stem };
    let device = stem.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (device.len() == 4 && (device.starts_with("COM") || device.starts_with("LPT")) && device.ends_with(|c: char| c.is_ascii_digit()));
    let stem = if reserved { format!("_{stem}") } else { stem };
    format!("{stem}.{extension}")
}

/// Whether `raw` starts as a message does: its first line is a header field (`Name: value`),
/// after an mbox `From ` line if it has one. A picture or a document renamed `.eml` doesn't.
pub fn looks_like_message(raw: &[u8]) -> bool {
    let mut lines = raw.split(|b| *b == b'\n');
    let mut first = lines.next().unwrap_or_default();
    if first.starts_with(b"From ") {
        first = lines.next().unwrap_or_default();
    }
    let Some(colon) = first.iter().position(|b| *b == b':') else { return false };
    let name = first.get(..colon).unwrap_or_default();
    !name.is_empty() && name.len() <= 200 && name.iter().all(|b| (33..=126).contains(b))
}

/// Reads a file of at most [`MAX_MESSAGE`] bytes; `None` when it is larger.
fn read_capped(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = File::open(path)?;
    let mut raw = Vec::new();
    file.take(MAX_MESSAGE as u64 + 1).read_to_end(&mut raw)?;
    Ok((raw.len() <= MAX_MESSAGE).then_some(raw))
}

/// Imports `.eml` files the user picked, into `folder`. A file that can't be read, is larger than
/// [`MAX_MESSAGE`] or is not a message is left out and counted.
pub fn import_files(paths: &[PathBuf], folder: &[String], sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let mut run = Run::new(sink, cancel);
    run.progress.total = paths.iter().map(|p| p.metadata().map(|m| m.len()).unwrap_or(0)).fold(0u64, u64::saturating_add);
    for path in paths {
        if run.stopped() {
            break;
        }
        let len = path.metadata().map(|m| m.len()).unwrap_or(0);
        one(&mut run, path, folder.to_vec())?;
        run.progress.done = run.progress.done.saturating_add(len);
    }
    run.report.folders = 1;
    Ok(run.finish())
}

/// Imports a folder tree of `.eml` files (an export of eM Client or of another client): each
/// folder's `.eml` files go into the folder of the same path under the top. Other files are
/// passed over.
pub fn import_tree(root: &Path, sink: &mut dyn Sink, cancel: &Cancel) -> Result<Report, String> {
    let found = walk::walk(root, &|name: &str| name.to_owned()).map_err(|e| format!("This folder can't be read: {e}"))?;
    let mut run = Run::new(sink, cancel);
    run.report.left_out = found.left_out;
    let files: Vec<_> = found.files.into_iter().filter(|f| f.name.to_ascii_lowercase().ends_with(".eml")).collect();
    run.progress.total = files.iter().map(|f| f.len).fold(0u64, u64::saturating_add);
    let mut folders: Vec<&[String]> = Vec::new();
    for f in &files {
        if run.stopped() {
            break;
        }
        if !folders.contains(&f.dirs.as_slice()) {
            folders.push(&f.dirs);
        }
        one(&mut run, &f.path, f.dirs.clone())?;
        run.progress.done = run.progress.done.saturating_add(f.len);
    }
    run.report.folders = folders.len() as u64;
    Ok(run.finish())
}

/// One `.eml` file into `folder`.
fn one(run: &mut Run<'_, '_>, path: &Path, folder: Vec<String>) -> Result<(), String> {
    match read_capped(path) {
        Ok(Some(raw)) if looks_like_message(&raw) => {
            let flags = mbox::marks(&raw).flags.unwrap_or(Flags { seen: true, ..Flags::default() });
            run.message(Item { folder, raw, flags })
        }
        Ok(Some(_)) => {
            run.skip(Skip::NotAMessage);
            Ok(())
        }
        Ok(None) => {
            run.skip(Skip::TooLarge);
            Ok(())
        }
        Err(_) => {
            run.skip(Skip::Unreadable);
            Ok(())
        }
    }
}
