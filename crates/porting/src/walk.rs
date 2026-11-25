//! Walking a folder tree the user picked (`.eml` exports, a Thunderbird profile): no link or
//! junction is followed, and the depth, the folders and the files are capped.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{MAX_DEPTH, MAX_FILES, MAX_FOLDERS, folder_name};

/// A file found, and the folders it is in, from the root down (their names as `dir_name` made
/// them, then cleaned by [`folder_name`]).
pub(crate) struct Found {
    pub(crate) path: PathBuf,
    pub(crate) dirs: Vec<String>,
    /// The file's own name (lossy, for a name to show).
    pub(crate) name: String,
    pub(crate) len: u64,
}

/// What a walk found, and how much it passed over (links, too deep, too many, unreadable).
pub(crate) struct Walk {
    pub(crate) files: Vec<Found>,
    pub(crate) left_out: u64,
}

/// The files under `root`, folder by folder (each folder's entries in order of their names, its
/// files before its subfolders). `dir_name` gives the folder name a directory's name stands
/// for (`Inbox.sbd` is `Inbox` for Thunderbird). An error only when `root` can't be listed.
pub(crate) fn walk(root: &Path, dir_name: &dyn Fn(&str) -> String) -> io::Result<Walk> {
    let mut out = Walk { files: Vec::new(), left_out: 0 };
    let mut folders = 0usize;
    let mut todo: Vec<(PathBuf, Vec<String>)> = vec![(root.to_path_buf(), Vec::new())];
    let mut first = true;
    while let Some((dir, names)) = todo.pop() {
        let listed = match fs::read_dir(&dir) {
            Ok(listed) => listed,
            Err(e) if first => return Err(e),
            Err(_) => {
                out.left_out += 1;
                continue;
            }
        };
        first = false;
        let mut entries: Vec<fs::DirEntry> = Vec::new();
        for entry in listed {
            match entry {
                Ok(e) if entries.len() < MAX_FILES => entries.push(e),
                _ => out.left_out += 1,
            }
        }
        entries.sort_by_key(|e| e.file_name());
        let mut subfolders = Vec::new();
        for entry in entries {
            // The entry itself, not what a link points to.
            let Ok(kind) = entry.file_type() else {
                out.left_out += 1;
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_symlink() {
                out.left_out += 1;
            } else if kind.is_dir() {
                if names.len() + 1 > MAX_DEPTH || folders >= MAX_FOLDERS {
                    out.left_out += 1;
                    continue;
                }
                folders += 1;
                let mut inner = names.clone();
                inner.push(folder_name(&dir_name(&name)));
                subfolders.push((entry.path(), inner));
            } else if kind.is_file() {
                if out.files.len() >= MAX_FILES {
                    out.left_out += 1;
                    continue;
                }
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.files.push(Found { path: entry.path(), dirs: names.clone(), name, len });
            }
        }
        // Last first, so they come off the stack in order.
        todo.extend(subfolders.into_iter().rev());
    }
    Ok(out)
}
