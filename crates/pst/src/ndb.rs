// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/pst/src/ndb.rs
// and crates/pst/src/ndb/sizes.rs. MIT OR Apache-2.0.
// Changed: read-only: what only the writer used (the map pages, the node types it creates) is
// left out; `sizes.rs` is folded in.

//! The file's lowest layer (MS-PST 2.2): its header, the two B-trees and the blocks they name.

mod block;
mod btree;
mod file;
mod header;

pub(crate) use block::SubNodes;
pub use file::{DEFAULT_MAX_VALUE, PstFile, ReadSeek};
pub use header::Format;
pub(crate) use header::Bref;

/// A node's type: the low five bits of its ID (MS-PST 2.2.2.1).
pub(crate) const NID_TYPE_MASK: u32 = 0x1F;
pub(crate) const NID_TYPE_NORMAL_FOLDER: u32 = 0x02;
pub(crate) const NID_TYPE_SEARCH_FOLDER: u32 = 0x03;
pub(crate) const NID_TYPE_NORMAL_MESSAGE: u32 = 0x04;
pub(crate) const NID_TYPE_ATTACHMENT: u32 = 0x05;
pub(crate) const NID_TYPE_ASSOC_MESSAGE: u32 = 0x08;
pub(crate) const NID_TYPE_HIERARCHY_TABLE: u32 = 0x0D;
pub(crate) const NID_TYPE_CONTENTS_TABLE: u32 = 0x0E;
pub(crate) const NID_TYPE_ATTACHMENT_TABLE: u32 = 0x11;
pub(crate) const NID_TYPE_RECIPIENT_TABLE: u32 = 0x12;

/// A block whose ID has this bit lists other blocks, and isn't encoded (MS-PST 2.2.2.2).
pub(crate) const BID_INTERNAL: u64 = 0x2;

/// A page is 512 bytes (MS-PST 2.2.2.7).
pub(crate) const PAGE: usize = 512;
/// A block and its trailer take at most 8 KiB (MS-PST 2.2.2.8).
pub(crate) const MAX_BLOCK: usize = 8192;

// Page types (`ptype`).
pub(crate) const PT_BBT: u8 = 0x80;
pub(crate) const PT_NBT: u8 = 0x81;

// Block types (`btype`, MS-PST 2.2.2.8.3).
pub(crate) const BT_DATA: u8 = 0x01;
pub(crate) const BT_SUBNODE: u8 = 0x02;
