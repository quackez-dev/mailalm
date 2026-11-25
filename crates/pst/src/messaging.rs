// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/pst/src/messaging.rs. MIT OR Apache-2.0.
// Changed: read-only: the bare store a writer starts from, the name-to-ID map (`named.rs`), the
// property subsets (`message_properties`), the hidden (associated) contents and the tables kept
// once read (`OnceCell`) are left out: a folder's tables are read when asked for, and say whether
// the folder has none or one that can't be read.

//! The top layer (MS-PST 2.4): the store, its folders, messages and attachments.

mod object;

use std::io;
use std::rc::Rc;

pub use object::{Attachment, AttachmentData, Message};

use crate::ltp::{Heap, Properties, PropertyValue, Table};
use crate::ndb::*;
use crate::util::{invalid, short, u32_at};

/// The message store's node (MS-PST 2.4.1).
const NID_MESSAGE_STORE: u32 = 0x21;

/// `PidTagIpmSubtreeEntryId`.
const IPM_SUBTREE: u16 = 0x35E0;

/// The set of properties of a node, read from its data and subnode trees.
pub(crate) fn node_properties(pst: &PstFile, data: u64, sub: u64) -> io::Result<(Properties, SubNodes)> {
    let heap = Heap::read(pst, data)?;
    let subs = pst.subnodes(sub)?;
    let props = crate::ltp::read_properties(pst, &heap, &subs)?;
    Ok((props, subs))
}

/// A data file's message store: where its folders are.
pub struct PstStore {
    pst: Rc<PstFile>,
    props: Properties,
}

impl PstStore {
    /// The store of the file `pst`.
    pub fn read(pst: Rc<PstFile>) -> io::Result<Rc<PstStore>> {
        let node = pst.node(NID_MESSAGE_STORE)?;
        let (props, _) = node_properties(&pst, node.data, node.sub)?;
        Ok(Rc::new(PstStore { pst, props }))
    }

    pub fn properties(&self) -> &Properties {
        &self.props
    }

    /// The node of the folder that holds the mailbox's folders (the IPM subtree).
    pub fn ipm_subtree(&self) -> io::Result<u32> {
        match self.props.get(IPM_SUBTREE) {
            Some(PropertyValue::Binary(b)) => node_of_entry_id(b.buffer()),
            _ => Err(invalid("the store doesn't say where its folders are")),
        }
    }

    /// The folder at node `nid`.
    pub fn open_folder(&self, nid: u32) -> io::Result<PstFolder> {
        if !matches!(nid & NID_TYPE_MASK, NID_TYPE_NORMAL_FOLDER | NID_TYPE_SEARCH_FOLDER) {
            return Err(invalid(format!("node {nid:#x} isn't a folder")));
        }
        let node = self.pst.node(nid)?;
        let (props, _) = node_properties(&self.pst, node.data, node.sub)?;
        Ok(PstFolder { pst: self.pst.clone(), nid, props })
    }
}

/// The node an entry ID names: its last four bytes (MS-PST 2.4.3.2).
pub(crate) fn node_of_entry_id(b: &[u8]) -> io::Result<u32> {
    if b.len() != 24 {
        return Err(invalid("an entry ID of the wrong size"));
    }
    u32_at(b, 20).map_err(|_| short())
}

/// A folder.
pub struct PstFolder {
    pst: Rc<PstFile>,
    nid: u32,
    props: Properties,
}

impl PstFolder {
    pub fn node(&self) -> u32 {
        self.nid
    }

    pub fn properties(&self) -> &Properties {
        &self.props
    }

    /// The table of the node beside this folder's, of the given type, read now: `None` when the
    /// folder has no such node, an error when it has one that can't be read.
    fn read_table(&self, kind: u32) -> io::Result<Option<Table>> {
        let node = match self.pst.node((self.nid & !NID_TYPE_MASK) | kind) {
            Ok(node) => node,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        Table::read(self.pst.clone(), node.data, node.sub).map(Some)
    }

    /// The folders in this one: `None` only when the folder has no such table.
    pub fn read_hierarchy_table(&self) -> io::Result<Option<Table>> {
        self.read_table(NID_TYPE_HIERARCHY_TABLE)
    }

    /// The messages in this one, told apart as [`PstFolder::read_hierarchy_table`] tells them.
    pub fn read_contents_table(&self) -> io::Result<Option<Table>> {
        self.read_table(NID_TYPE_CONTENTS_TABLE)
    }
}
