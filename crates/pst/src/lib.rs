// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/pst/src/lib.rs. MIT OR Apache-2.0.
// Changed: read-only: the writer (`write/`, about 4,000 lines), the name-to-ID map, the fuzzing
// switch and the browser (wasm) switches are left out; see `audits/mailalm/reprise-pst.md` in
// CraftTools for every change.

//! Outlook data files (`.pst`, and `.ost` which has the same layout), as Microsoft's open
//! specification [MS-PST] lays them out: **read only**, in the Unicode format and the older ANSI
//! one. MailALM never writes into a data file: [`PstFile::open`] opens it for reading.
//!
//! Three layers, as the specification has them: `ndb` (pages, blocks, the node and block
//! B-trees), `ltp` (heaps, property sets, tables) and `messaging` (folders, messages,
//! attachments). One set of types serves both formats: IDs and offsets are held 64 bits wide and
//! only the code that reads them knows an ANSI file's are 32.
//!
//! Everything read is hostile until checked: nothing here panics on any file, every size is
//! capped (one value at [`DEFAULT_MAX_VALUE`] unless the caller sets another), every tree walk goes
//! down level by level and ends, every CRC is checked.
//!
//! [MS-PST]: https://learn.microsoft.com/en-us/openspecs/office_file_formats/ms-pst/

mod crc;
mod crypt;
mod ltp;
mod messaging;
mod ndb;
mod util;

pub use crc::crc;
pub use ltp::{Bytes, Column, GuidValue, ObjectRef, Properties, PropertyType, PropertyValue, Row, Table, Utf16};
pub use messaging::{Attachment, AttachmentData, Message, PstFolder, PstStore};
pub use ndb::{DEFAULT_MAX_VALUE, Format, PstFile, ReadSeek};
