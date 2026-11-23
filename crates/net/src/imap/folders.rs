// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/imap/folders.rs (with `FolderRole::from_attribute` from crates/model/src/folder.rs). MIT OR Apache-2.0.
// Changed: MailALM's `Folder` (name decoded from modified UTF-7, the server's name kept as `wire`); `\Noselect` and `\NonExistent` folders left out; `SELECT` gives a `Mailbox`; no QRESYNC, no create, rename, delete or empty.

//! Folders: listed with their roles, counted and selected.

use std::collections::HashSet;
use std::io::{Read, Write};

use imap_proto::{MailboxDatum, NameAttribute, Response, ResponseCode, StatusAttribute};
use mailalm_model::{Folder, FolderRole};

use super::{Client, Mailbox, Part, mailbox_arg, utf7};
use crate::Result;

impl<S: Read + Write> Client<S> {
    /// All folders that can be opened, with their special-use roles (RFC 6154), else the roles
    /// their names suggest.
    pub(super) fn folders(&mut self) -> Result<Vec<Folder>> {
        let cmd = if self.has("SPECIAL-USE") && self.has("LIST-EXTENDED") { "LIST \"\" \"*\" RETURN (SPECIAL-USE)" } else { "LIST \"\" \"*\"" };
        let rs = self.cmd(&[Part::Text(cmd)])?;
        let mut out: Vec<Folder> = Vec::new();
        // Each folder once, whatever the server lists twice.
        let mut listed: HashSet<String> = HashSet::new();
        // Which roles the server gave (RFC 6154), not guessed from a name.
        let mut flagged = HashSet::new();
        for r in rs {
            let Response::MailboxData(MailboxDatum::List(l)) = r else { continue };
            let delimiter = l.delimiter.as_deref().and_then(|d| d.chars().next());
            let wire = l.name.to_string();
            let mut role = None;
            let mut selectable = true;
            for a in &l.name_attributes {
                match a {
                    NameAttribute::NoSelect => selectable = false,
                    NameAttribute::Extension(e) if e.eq_ignore_ascii_case("\\NonExistent") => selectable = false,
                    NameAttribute::Sent => role = Some(FolderRole::Sent),
                    NameAttribute::Drafts => role = Some(FolderRole::Drafts),
                    NameAttribute::Trash => role = Some(FolderRole::Trash),
                    NameAttribute::Junk => role = Some(FolderRole::Junk),
                    NameAttribute::Archive => role = Some(FolderRole::Archive),
                    NameAttribute::All => role = role.or(Some(FolderRole::Archive)),
                    NameAttribute::Extension(e) => role = role.or(role_from_attribute(e)),
                    _ => {}
                }
            }
            if !selectable || !listed.insert(wire.clone()) {
                continue;
            }
            let inbox = wire.eq_ignore_ascii_case("INBOX");
            let path = if inbox { "INBOX".to_string() } else { utf7::decode(&wire) };
            if role.is_some() && !inbox {
                flagged.insert(wire.clone());
            }
            let role = if inbox { FolderRole::Inbox } else { role.unwrap_or_else(|| FolderRole::from_name(&path, delimiter)) };
            out.push(Folder { path, wire, delimiter, role, total: 0, unread: 0 });
        }
        one_per_role(&mut out, &flagged);
        Ok(out)
    }

    /// `SELECT`, read-write: the folder's UIDVALIDITY, next UID and message count.
    pub(super) fn select(&mut self, wire: &str) -> Result<Mailbox> {
        let rs = self.cmd(&[Part::Text(&format!("SELECT {}", mailbox_arg(wire)))])?;
        let mut m = Mailbox::default();
        for r in rs {
            match r {
                Response::MailboxData(MailboxDatum::Exists(n)) => m.exists = n,
                Response::Data { outcome, .. } | Response::Done { outcome, .. } => match outcome.code {
                    Some(ResponseCode::UidValidity(v)) => m.uidvalidity = v,
                    Some(ResponseCode::UidNext(v)) => m.uidnext = v,
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(m)
    }

    /// `STATUS` counts for a folder: (messages, unseen).
    pub(super) fn status(&mut self, wire: &str) -> Result<(u32, u32)> {
        let cmd = format!("STATUS {} (MESSAGES UNSEEN)", mailbox_arg(wire));
        let rs = self.cmd(&[Part::Text(&cmd)])?;
        let (mut total, mut unseen) = (0, 0);
        for r in rs {
            if let Response::MailboxData(MailboxDatum::Status { status, .. }) = r {
                for s in status {
                    match s {
                        StatusAttribute::Messages(n) => total = n,
                        StatusAttribute::Unseen(n) => unseen = n,
                        _ => {}
                    }
                }
            }
        }
        Ok((total, unseen))
    }
}

/// The role a special-use attribute imap-proto doesn't name gives (`\Spam`, as some servers
/// write `\Junk`).
fn role_from_attribute(attr: &str) -> Option<FolderRole> {
    match attr.trim_start_matches('\\').to_ascii_lowercase().as_str() {
        "sent" => Some(FolderRole::Sent),
        "drafts" => Some(FolderRole::Drafts),
        "trash" => Some(FolderRole::Trash),
        "junk" | "spam" => Some(FolderRole::Junk),
        "archive" | "all" => Some(FolderRole::Archive),
        _ => None,
    }
}

/// One folder per special role, the rest ordinary folders: the one the server flags for it, else
/// the first listed. Servers don't list it first (Dovecot lists cPanel's flagged `INBOX.Sent`
/// after `INBOX.Sent Items` made by another program: MailCraft's issue #19, where sent mail's
/// copies went in the one nobody looks at).
fn one_per_role(folders: &mut [Folder], flagged: &HashSet<String>) {
    let mut order: Vec<usize> = (0..folders.len()).collect();
    order.sort_by_key(|&i| folders.get(i).map(|f| !flagged.contains(&f.wire)));
    let mut seen = HashSet::new();
    for i in order {
        if let Some(f) = folders.get_mut(i)
            && f.role != FolderRole::Other
            && !seen.insert(f.role)
        {
            f.role = FolderRole::Other;
        }
    }
}
