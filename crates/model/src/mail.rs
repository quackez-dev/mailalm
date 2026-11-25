//! Servers, folders and message summaries: what the network layer, the cache and the interface
//! share about mail.

use serde::{Deserialize, Serialize};

/// How a connection to a server is protected. There is no clear-text option: MailALM never sends a
/// password, or reads mail, over an unprotected connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Security {
    /// TLS from the first byte (IMAP 993, SMTP 465).
    Tls,
    /// A clear-text greeting, then STARTTLS, required: the session stops if the server does not
    /// offer it (IMAP 143, SMTP 587).
    StartTls,
}

/// One server of an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub security: Security,
}

impl Server {
    pub fn new(host: &str, port: u16, security: Security) -> Self {
        Self { host: host.to_owned(), port, security }
    }

    /// A host name a connection may be attempted to: letters, digits, dots and dashes, as typed by
    /// the user or set by a preset. Rejects spaces, schemes (`https://`), paths and ports.
    pub fn host_is_valid(&self) -> bool {
        let h = self.host.as_str();
        !h.is_empty()
            && h.len() <= 253
            && !h.starts_with('.')
            && !h.ends_with('.')
            && !h.contains("..")
            && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && self.port != 0
    }
}

/// What a folder is for, from the server's SPECIAL-USE marks (RFC 6154) or its usual name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FolderRole {
    Inbox,
    Drafts,
    Sent,
    Archive,
    Junk,
    Trash,
    Other,
}

impl FolderRole {
    /// The role a folder plays when the server does not mark it: from its usual names, English
    /// and French.
    pub fn from_name(path: &str, delimiter: Option<char>) -> Self {
        if path.eq_ignore_ascii_case("INBOX") {
            return FolderRole::Inbox;
        }
        let leaf = match delimiter {
            Some(d) => path.rsplit(d).next().unwrap_or(path),
            None => path,
        };
        let leaf = leaf.to_lowercase();
        match leaf.as_str() {
            "drafts" | "draft" | "brouillons" => FolderRole::Drafts,
            "sent" | "sent items" | "sent messages" | "sent mail" | "envoyés" | "éléments envoyés" | "messages envoyés" => {
                FolderRole::Sent
            }
            "archive" | "archives" => FolderRole::Archive,
            "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" | "courrier indésirable" | "pourriel" | "indésirables" => {
                FolderRole::Junk
            }
            "trash" | "deleted" | "deleted items" | "deleted messages" | "corbeille" | "éléments supprimés" => FolderRole::Trash,
            _ => FolderRole::Other,
        }
    }

    /// Where the role's folder comes in the folder pane: the usual ones first, in this order.
    pub fn rank(self) -> u8 {
        match self {
            FolderRole::Inbox => 0,
            FolderRole::Drafts => 1,
            FolderRole::Sent => 2,
            FolderRole::Archive => 3,
            FolderRole::Junk => 4,
            FolderRole::Trash => 5,
            FolderRole::Other => 6,
        }
    }
}

/// One folder of an account, as the server lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    /// The folder's name on the server (modified UTF-7 decoded), the key everywhere else.
    pub path: String,
    /// The name as sent to the server (modified UTF-7), for the commands that name it.
    pub wire: String,
    pub delimiter: Option<char>,
    pub role: FolderRole,
    /// Messages in the folder, and unread ones, as last synced.
    pub total: u32,
    pub unread: u32,
}

impl Folder {
    /// The folder's own name, without its parents.
    pub fn name(&self) -> &str {
        match self.delimiter {
            Some(d) => self.path.rsplit(d).next().unwrap_or(&self.path),
            None => &self.path,
        }
    }

    /// How deep the folder is (0 for a top-level folder).
    pub fn depth(&self) -> usize {
        match self.delimiter {
            Some(d) => self.path.matches(d).count(),
            None => 0,
        }
    }
}

/// Orders folders as the folder pane shows them: by role, then by path.
pub fn sort_folders(folders: &mut [Folder]) {
    folders.sort_by(|a, b| a.role.rank().cmp(&b.role.rank()).then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase())));
}

/// The marks of a message, as IMAP keeps them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub draft: bool,
}

impl Flags {
    /// From IMAP's flag names (`\Seen`, `\Flagged`…), case-insensitive; others are ignored.
    pub fn from_imap<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut f = Flags::default();
        for n in names {
            match n.to_ascii_lowercase().as_str() {
                "\\seen" => f.seen = true,
                "\\flagged" => f.flagged = true,
                "\\answered" => f.answered = true,
                "\\draft" => f.draft = true,
                _ => {}
            }
        }
        f
    }
}

/// One person in a header: the name, if any, and the address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    pub address: String,
}

impl Person {
    /// The name, or the address when there is no name.
    pub fn display(&self) -> &str {
        if self.name.trim().is_empty() { &self.address } else { &self.name }
    }
}

/// What the message list needs about a message, kept in the cache.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// The message's UID in its folder (valid with the folder's UIDVALIDITY).
    pub uid: u32,
    pub from: Person,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub subject: String,
    /// Seconds since 1970 (UTC): the Date header, or the server's arrival date without one.
    pub date: i64,
    pub flags: Flags,
    /// The size on the server, in bytes.
    pub size: u32,
    /// A multipart/mixed message, or one whose body is known to carry attachments.
    pub has_attachments: bool,
    /// The first words of the text, once the body is in the cache.
    pub preview: String,
    pub message_id: String,
    /// The message it answers (its `In-Reply-To`, without angle brackets): the follow-ups look
    /// for answers by it (C2).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub in_reply_to: String,
    /// The messages of its thread before it (`References`, without angle brackets), at most
    /// [`MAX_REFERENCES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<String>,
    /// The provider's own thread id, when it has one (Graph's `conversationId`, Gmail's
    /// `threadId`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thread: String,
    /// The provider's categories (Outlook's categories; Gmail's Inbox categories: Primary,
    /// Social…), for the search's `category:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

/// The most `References` kept in a summary (the newest ones: the thread's root comes first, and
/// is kept too).
pub const MAX_REFERENCES: usize = 50;

/// A `References` list as a summary keeps it: the first (the thread's root) and the newest,
/// [`MAX_REFERENCES`] at most.
pub fn capped_references(mut refs: Vec<String>) -> Vec<String> {
    refs.retain(|r| !r.is_empty());
    if refs.len() > MAX_REFERENCES {
        let tail = refs.split_off(refs.len() - (MAX_REFERENCES - 1));
        refs.truncate(1);
        refs.extend(tail);
    }
    refs
}

/// A change made while offline, or not yet confirmed by the server. Sent in order at the next
/// connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    SetSeen { folder: String, uids: Vec<u32>, seen: bool },
    SetFlagged { folder: String, uids: Vec<u32>, flagged: bool },
    /// Moves to another folder of the same account.
    Move { folder: String, uids: Vec<u32>, to: String },
    /// Removes for good (from the Trash, or when there is no Trash).
    Delete { folder: String, uids: Vec<u32> },
    /// Outlook (Graph `reportMessage`): reports the messages to Outlook.com as junk and moves them
    /// to Junk, or as not junk and moves them back to the Inbox. IMAP moves them.
    MarkJunk { folder: String, uids: Vec<u32>, junk: bool },
}

impl Change {
    /// The folder the change applies to.
    pub fn folder(&self) -> &str {
        match self {
            Change::SetSeen { folder, .. }
            | Change::SetFlagged { folder, .. }
            | Change::Move { folder, .. }
            | Change::Delete { folder, .. }
            | Change::MarkJunk { folder, .. } => folder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_roles_from_usual_names() {
        assert_eq!(FolderRole::from_name("inbox", Some('/')), FolderRole::Inbox);
        assert_eq!(FolderRole::from_name("INBOX.Sent", Some('.')), FolderRole::Sent);
        assert_eq!(FolderRole::from_name("Corbeille", Some('/')), FolderRole::Trash);
        assert_eq!(FolderRole::from_name("Courrier indésirable", None), FolderRole::Junk);
        assert_eq!(FolderRole::from_name("Projects/Sent", Some('/')), FolderRole::Sent);
        assert_eq!(FolderRole::from_name("Receipts", Some('/')), FolderRole::Other);
    }

    #[test]
    fn folder_name_and_depth() {
        let f = Folder {
            path: "INBOX/Work/2026".into(),
            wire: "INBOX/Work/2026".into(),
            delimiter: Some('/'),
            role: FolderRole::Other,
            total: 0,
            unread: 0,
        };
        assert_eq!(f.name(), "2026");
        assert_eq!(f.depth(), 2);
    }

    #[test]
    fn folders_sort_by_role_then_name() {
        let f = |p: &str, role| Folder { path: p.into(), wire: p.into(), delimiter: Some('/'), role, total: 0, unread: 0 };
        let mut v = vec![f("b", FolderRole::Other), f("Trash", FolderRole::Trash), f("INBOX", FolderRole::Inbox), f("A", FolderRole::Other)];
        sort_folders(&mut v);
        let order: Vec<_> = v.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(order, ["INBOX", "Trash", "A", "b"]);
    }

    #[test]
    fn references_are_capped_keeping_the_root() {
        let refs: Vec<String> = (0..80).map(|n| format!("m{n}@x")).collect();
        let kept = capped_references(refs);
        assert_eq!(kept.len(), MAX_REFERENCES);
        assert_eq!(kept.first().map(String::as_str), Some("m0@x"));
        assert_eq!(kept.last().map(String::as_str), Some("m79@x"));
        assert_eq!(kept.get(1).map(String::as_str), Some("m31@x"));
        assert_eq!(capped_references(vec![String::new(), "a@b".into()]), ["a@b"]);
    }

    #[test]
    fn flags_from_imap_names() {
        let f = Flags::from_imap(["\\Seen", "\\FLAGGED", "$Junk"]);
        assert!(f.seen && f.flagged && !f.answered && !f.draft);
    }

    #[test]
    fn server_hosts_are_checked() {
        assert!(Server::new("imap.videotron.ca", 993, Security::Tls).host_is_valid());
        assert!(!Server::new("https://imap.example.com", 993, Security::Tls).host_is_valid());
        assert!(!Server::new("imap example.com", 993, Security::Tls).host_is_valid());
        assert!(!Server::new("", 993, Security::Tls).host_is_valid());
        assert!(!Server::new("imap.example.com", 0, Security::Tls).host_is_valid());
    }
}
