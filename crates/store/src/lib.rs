//! The local cache (PLAN.md, sections 6, 10.3 and C7): what MailALM shows and searches while
//! offline.
//!
//! One folder per account, under the user's local data folder:
//!
//! ```text
//! <base>/<account id>/
//!   folders.json              the folder list
//!   pending.json              changes not yet confirmed by the server, in order
//!   f-<key>/state.json        the folder's path, UIDVALIDITY and UIDNEXT
//!   f-<key>/index.json        the message summaries
//!   f-<key>/ids.json          Outlook: each message's local number and its id on the server
//!   f-<key>/<uid>.eml         a message, as the server sent it
//!   f-<key>/<uid>.txt         its text, for the search
//! ```
//!
//! `<key>` is a hash of the folder's path, so no name sent by a server ever becomes a file name.
//! Files are replaced in one step (written beside, then renamed): a crash leaves the old file or
//! the new one, never half of one. The cache is not encrypted (decision Q8): BitLocker and the
//! Windows session protect it.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use mailalm_model::{AccountId, Change, Folder, Summary};
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

/// What the cache remembers of a folder between two syncs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderState {
    pub path: String,
    pub uidvalidity: u32,
    pub uidnext: u32,
    /// Outlook (Graph): where the next sync of the folder starts (its delta link). `uidnext` is
    /// then the next local number to give a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
}

/// One account's cache.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens (and creates) the cache of an account under `base`.
    pub fn open(base: &Path, account: AccountId) -> io::Result<Self> {
        let root = base.join(account.0.to_string());
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Removes the whole cache of an account (when the account is removed).
    pub fn delete_all(self) -> io::Result<()> {
        match fs::remove_dir_all(&self.root) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn folders(&self) -> Vec<Folder> {
        read_json(&self.root.join("folders.json")).unwrap_or_default()
    }

    pub fn save_folders(&self, folders: &[Folder]) -> io::Result<()> {
        write_json(&self.root.join("folders.json"), &folders)
    }

    /// Removes the cache of folders the server no longer lists.
    pub fn forget_folders_except(&self, keep: &[Folder]) -> io::Result<()> {
        let keep: Vec<String> = keep.iter().map(|f| dir_name(&f.path)).collect();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("f-") && entry.path().is_dir() && !keep.contains(&name) {
                fs::remove_dir_all(entry.path())?;
            }
        }
        Ok(())
    }

    fn folder_dir(&self, path: &str) -> PathBuf {
        self.root.join(dir_name(path))
    }

    pub fn folder_state(&self, path: &str) -> Option<FolderState> {
        read_json::<FolderState>(&self.folder_dir(path).join("state.json")).filter(|s| s.path == path)
    }

    pub fn save_folder_state(&self, state: &FolderState) -> io::Result<()> {
        let dir = self.folder_dir(&state.path);
        fs::create_dir_all(&dir)?;
        write_json(&dir.join("state.json"), state)
    }

    /// Forgets everything kept for a folder (its UIDVALIDITY changed: every UID is new).
    pub fn clear_folder(&self, path: &str) -> io::Result<()> {
        match fs::remove_dir_all(self.folder_dir(path)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The folder's summaries, newest first.
    pub fn summaries(&self, path: &str) -> Vec<Summary> {
        read_json(&self.folder_dir(path).join("index.json")).unwrap_or_default()
    }

    pub fn save_summaries(&self, path: &str, summaries: &[Summary]) -> io::Result<()> {
        let dir = self.folder_dir(path);
        fs::create_dir_all(&dir)?;
        write_json(&dir.join("index.json"), &summaries)
    }

    /// A message as the server sent it, if it is in the cache.
    pub fn message(&self, path: &str, uid: u32) -> Option<Vec<u8>> {
        fs::read(self.folder_dir(path).join(format!("{uid}.eml"))).ok()
    }

    pub fn has_message(&self, path: &str, uid: u32) -> bool {
        self.folder_dir(path).join(format!("{uid}.eml")).is_file()
    }

    /// Keeps a message and its text (for the search).
    pub fn save_message(&self, path: &str, uid: u32, raw: &[u8], text: &str) -> io::Result<()> {
        let dir = self.folder_dir(path);
        fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(format!("{uid}.eml")), raw)?;
        write_atomic(&dir.join(format!("{uid}.txt")), text.as_bytes())
    }

    /// The text kept for the search, if the message is in the cache.
    pub fn text(&self, path: &str, uid: u32) -> Option<String> {
        fs::read_to_string(self.folder_dir(path).join(format!("{uid}.txt"))).ok()
    }

    /// Forgets messages (moved, deleted, or gone from the server).
    pub fn remove_messages(&self, path: &str, uids: &[u32]) -> io::Result<()> {
        let dir = self.folder_dir(path);
        for uid in uids {
            for ext in ["eml", "txt"] {
                match fs::remove_file(dir.join(format!("{uid}.{ext}"))) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Outlook (Graph): the local number of each message of the folder and its id on the server.
    pub fn ids(&self, path: &str) -> Vec<(u32, String)> {
        read_json(&self.folder_dir(path).join("ids.json")).unwrap_or_default()
    }

    pub fn save_ids(&self, path: &str, ids: &[(u32, String)]) -> io::Result<()> {
        let dir = self.folder_dir(path);
        fs::create_dir_all(&dir)?;
        write_json(&dir.join("ids.json"), &ids)
    }

    /// Changes waiting for the server, oldest first.
    pub fn pending(&self) -> Vec<Change> {
        read_json(&self.root.join("pending.json")).unwrap_or_default()
    }

    pub fn save_pending(&self, changes: &[Change]) -> io::Result<()> {
        write_json(&self.root.join("pending.json"), &changes)
    }
}

/// The name of a folder's cache directory: `f-` and a hash of its path (FNV-1a, 64 bits). Stable
/// from one run to the next; `state.json` keeps the path, checked on reading.
pub fn dir_name(path: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("f-{h:016x}")
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    write_atomic(path, &bytes)
}

/// Writes a file beside the target, flushes it to the disk, then renames it over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailalm_model::{FolderRole, Person};

    /// A fresh directory for one test, under the system's temp folder.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailalm-store-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn folder(path: &str) -> Folder {
        Folder { path: path.into(), wire: path.into(), delimiter: Some('/'), role: FolderRole::Other, total: 0, unread: 0 }
    }

    #[test]
    fn folders_summaries_and_messages_round_trip() {
        let base = temp("round-trip");
        let s = Store::open(&base, AccountId(3)).expect("open");
        s.save_folders(&[folder("INBOX"), folder("Work/2026")]).expect("folders");
        assert_eq!(s.folders().len(), 2);

        let sum = Summary { uid: 7, subject: "Hello".into(), from: Person { name: "Zoe".into(), address: "z@x.example".into() }, ..Summary::default() };
        s.save_summaries("Work/2026", std::slice::from_ref(&sum)).expect("summaries");
        assert_eq!(s.summaries("Work/2026"), vec![sum]);
        assert!(s.summaries("INBOX").is_empty());

        s.save_message("Work/2026", 7, b"raw", "text").expect("message");
        assert!(s.has_message("Work/2026", 7));
        assert_eq!(s.message("Work/2026", 7).as_deref(), Some(&b"raw"[..]));
        assert_eq!(s.text("Work/2026", 7).as_deref(), Some("text"));
        s.remove_messages("Work/2026", &[7, 8]).expect("remove");
        assert!(!s.has_message("Work/2026", 7));

        let state = FolderState { path: "Work/2026".into(), uidvalidity: 5, uidnext: 9, delta: Some("https://graph.example/delta".into()) };
        s.save_ids("Work/2026", &[(1, "AAMk-1".into())]).expect("ids");
        assert_eq!(s.ids("Work/2026"), vec![(1, "AAMk-1".to_string())]);
        s.save_folder_state(&state).expect("state");
        assert_eq!(s.folder_state("Work/2026"), Some(state));
        s.clear_folder("Work/2026").expect("clear");
        assert_eq!(s.folder_state("Work/2026"), None);

        s.save_folders(&[folder("INBOX")]).expect("folders");
        s.save_summaries("Gone", &[]).expect("summaries");
        s.forget_folders_except(&s.folders()).expect("forget");
        assert!(!s.root().join(dir_name("Gone")).exists());

        s.delete_all().expect("delete");
        assert!(!base.join("3").exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn pending_changes_keep_their_order() {
        let base = temp("pending");
        let s = Store::open(&base, AccountId(1)).expect("open");
        let changes = vec![
            Change::SetSeen { folder: "INBOX".into(), uids: vec![1, 2], seen: true },
            Change::Move { folder: "INBOX".into(), uids: vec![2], to: "Archive".into() },
        ];
        s.save_pending(&changes).expect("save");
        assert_eq!(s.pending(), changes);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn folder_names_never_reach_the_disk() {
        for hostile in ["../../Windows", "CON", "a\\b", "名前", ""] {
            let d = dir_name(hostile);
            assert!(d.starts_with("f-") && d.len() == 18 && d[2..].chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(dir_name("INBOX"), dir_name("Inbox"));
    }

    #[test]
    fn a_damaged_file_reads_as_empty() {
        let base = temp("damaged");
        let s = Store::open(&base, AccountId(1)).expect("open");
        fs::write(s.root().join("folders.json"), b"{not json").expect("write");
        assert!(s.folders().is_empty());
        let _ = fs::remove_dir_all(&base);
    }
}
