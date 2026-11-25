//! Files MailALM writes from an attachment, at the user's request: a copy saved where they chose,
//! and the copies opened with Windows, in a folder of their own that is emptied at start and at
//! exit.
//!
//! Both carry Windows' Mark-of-the-Web: the `Zone.Identifier` stream that browsers and mail
//! programs add to what they download (zone 3, the internet). With it, SmartScreen checks a
//! program before it runs, and Office opens the document in Protected View.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The Mark-of-the-Web: the content of the `Zone.Identifier` stream.
pub const ZONE_IDENTIFIER: &str = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=about:internet\r\n";

/// The longest name kept for a copy opened with Windows.
const MAX_STEM: usize = 60;

/// The path of a file's `Zone.Identifier` stream (`<path>:Zone.Identifier`).
pub fn zone_stream(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(":Zone.Identifier");
    PathBuf::from(s)
}

/// Whether the mark is on the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Written,
    /// The drive keeps no streams (FAT, some network drives), or this is not Windows.
    NotWritten,
}

/// Puts the Mark-of-the-Web on a file (Windows only).
pub fn mark(path: &Path) -> Mark {
    if !cfg!(windows) {
        return Mark::NotWritten;
    }
    match fs::write(zone_stream(path), ZONE_IDENTIFIER) {
        Ok(()) => Mark::Written,
        Err(_) => Mark::NotWritten,
    }
}

/// Writes `bytes` to `path` (replacing what is there), then the Mark-of-the-Web.
pub fn save_marked(path: &Path, bytes: &[u8]) -> io::Result<Mark> {
    {
        let mut f = fs::File::create(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    Ok(mark(path))
}

/// A name for a copy: the name without its extension, letters and digits kept, the rest made
/// `_` or left out, at most [`MAX_STEM`] characters; `attachment` when nothing is left.
pub fn safe_stem(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let s: String = stem
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() && !c.is_control() {
                Some(c)
            } else if matches!(c, '-' | '_' | ' ' | '.' | '(' | ')') {
                Some('_')
            } else {
                None
            }
        })
        .take(MAX_STEM)
        .collect();
    let s = s.trim_matches('_').to_owned();
    if s.is_empty() { "attachment".to_owned() } else { s }
}

/// The folder of the copies opened with Windows. Emptied when it is opened and when it is
/// dropped (MailALM's exit). A copy still open in another program when MailALM exits stays until
/// the next start.
#[derive(Debug)]
pub struct TempFolder {
    dir: PathBuf,
    next: u64,
}

impl TempFolder {
    /// The folder at `dir`, emptied of what an earlier run left.
    pub fn new(dir: PathBuf) -> Self {
        let t = TempFolder { dir, next: 1 };
        t.clear();
        t
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Removes the folder and what is in it (best effort: a file still open elsewhere stays).
    pub fn clear(&self) {
        // `remove_dir_all` does not follow links out of the folder.
        let _ = fs::remove_dir_all(&self.dir);
    }

    /// Writes a copy named after `name`, with `extension` (which the caller took from the
    /// allowlist, never from the message), and marks it. Each copy gets a number of its own.
    pub fn copy(&mut self, name: &str, extension: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.dir)?;
        let ext: String = extension.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        let path = self.dir.join(format!("{}-{}.{}", self.next, safe_stem(name), ext));
        self.next += 1;
        save_marked(&path, bytes)?;
        Ok(path)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        self.clear();
    }
}
