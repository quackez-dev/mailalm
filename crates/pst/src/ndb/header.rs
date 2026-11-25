// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/pst/src/ndb/header.rs. MIT OR Apache-2.0.
// Changed: read-only: writing the header back (`to_bytes`), and the fields only the writer used
// (the allocation maps, the next page, block and node IDs, the raw bytes), are left out; the tests
// build their header by hand.

//! The file's header (MS-PST 2.2.2.6), which says what kind of file it is and where it all begins.

use std::io;

use crate::crc::matches;
use crate::crypt::Crypt;
use crate::util::{invalid, short, u8_at, u16_at, u32_at, u64_at};

/// The file's two formats: Outlook 2003 and later, or Outlook 97–2002.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Unicode,
    Ansi,
}

/// A block or page and where it is (`BREF`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bref {
    pub(crate) bid: u64,
    pub(crate) ib: u64,
}

/// `ROOT`: where the two B-trees start.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Root {
    pub(crate) nbt: Bref,
    pub(crate) bbt: Bref,
}

#[derive(Clone, Debug)]
pub(crate) struct Header {
    pub(crate) format: Format,
    pub(crate) crypt: Crypt,
    pub(crate) root: Root,
}

/// Bytes read to take in either format's header: a Unicode one's, the longer.
pub(crate) const HEADER_READ: usize = 564;

/// `dwMagic` and `wMagicClient`.
const MAGIC: &[u8; 4] = b"!BDN";
const CLIENT: &[u8; 2] = b"SM";
const CRC_START: usize = 8;
const CRC_PARTIAL: usize = 471;
const CRC_FULL: usize = 516;

impl Format {
    /// Whether IDs and offsets are 64 bits wide.
    pub(crate) fn wide(self) -> bool {
        self == Format::Unicode
    }

    /// Bytes in a page before its trailer.
    pub(crate) fn page_data(self) -> usize {
        if self.wide() { 496 } else { 500 }
    }

    /// The room for a B-tree page's entries, before its counts (`rgentries`, MS-PST 2.2.2.7.7.1).
    pub(crate) fn bt_entries(self) -> usize {
        if self.wide() { 488 } else { 496 }
    }

    /// A subnode block's header, before its entries (MS-PST 2.2.2.8.3.3): a Unicode one is padded
    /// to 8 bytes.
    pub(crate) fn sub_header(self) -> usize {
        if self.wide() { 8 } else { 4 }
    }

    /// An ID's or an offset's size.
    pub(crate) fn id_size(self) -> usize {
        if self.wide() { 8 } else { 4 }
    }

    /// A block's trailer.
    pub(crate) fn block_trailer(self) -> usize {
        if self.wide() { 16 } else { 12 }
    }

    /// The most data a block holds.
    pub(crate) fn block_data(self) -> usize {
        crate::ndb::MAX_BLOCK - self.block_trailer()
    }
}

impl Header {
    /// `(ROOT, bCryptMethod)`'s offsets.
    fn offsets(format: Format) -> (usize, usize) {
        if format.wide() { (180, 513) } else { (164, 461) }
    }

    pub(crate) fn parse(b: &[u8]) -> io::Result<Header> {
        if b.get(..4) != Some(&MAGIC[..]) || b.get(8..10) != Some(&CLIENT[..]) {
            return Err(invalid("not an Outlook data file"));
        }
        let format = match u16_at(b, 10)? {
            14 | 15 => Format::Ansi,
            // 23 or more (MS-PST 2.2.2.6); from 36, files keep 4 KiB pages, which it doesn't lay out.
            23..=35 => Format::Unicode,
            v => return Err(invalid(format!("data file version {v} isn't one this reads"))),
        };
        if !matches(b.get(CRC_START..CRC_START + CRC_PARTIAL).ok_or_else(short)?, u32_at(b, 4)?) {
            return Err(invalid("the file's header is damaged"));
        }
        if format.wide() && !matches(b.get(CRC_START..CRC_START + CRC_FULL).ok_or_else(short)?, u32_at(b, 524)?) {
            return Err(invalid("the file's header is damaged"));
        }
        let (root_at, crypt_at) = Self::offsets(format);
        let crypt = Crypt::from_byte(u8_at(b, crypt_at)?).ok_or_else(|| invalid("an unknown block encoding"))?;
        let root = if format.wide() { Self::root_wide(b, root_at)? } else { Self::root_narrow(b, root_at)? };
        Ok(Header { format, crypt, root })
    }

    fn root_wide(b: &[u8], at: usize) -> io::Result<Root> {
        Ok(Root {
            nbt: Bref { bid: u64_at(b, at + 36)?, ib: u64_at(b, at + 44)? },
            bbt: Bref { bid: u64_at(b, at + 52)?, ib: u64_at(b, at + 60)? },
        })
    }

    fn root_narrow(b: &[u8], at: usize) -> io::Result<Root> {
        let w = |o: usize| u32_at(b, at + o).map(u64::from);
        Ok(Root { nbt: Bref { bid: w(20)?, ib: w(24)? }, bbt: Bref { bid: w(28)?, ib: w(32)? } })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::crc::crc;

    /// A valid header of `format`, its B-trees at `nbt` and `bbt`, encoded with `crypt` (0, 1, 2).
    pub(crate) fn header(format: Format, nbt: Bref, bbt: Bref, crypt: u8) -> Vec<u8> {
        let len = if format.wide() { HEADER_READ } else { 512 };
        let mut b = vec![0u8; len];
        b[..4].copy_from_slice(MAGIC);
        b[8..10].copy_from_slice(CLIENT);
        b[10..12].copy_from_slice(&(if format.wide() { 23u16 } else { 14u16 }).to_le_bytes());
        let (root_at, crypt_at) = Header::offsets(format);
        b[crypt_at] = crypt;
        if format.wide() {
            for (o, v) in [(36, nbt.bid), (44, nbt.ib), (52, bbt.bid), (60, bbt.ib)] {
                b[root_at + o..root_at + o + 8].copy_from_slice(&v.to_le_bytes());
            }
        } else {
            for (o, v) in [(20, nbt.bid), (24, nbt.ib), (28, bbt.bid), (32, bbt.ib)] {
                b[root_at + o..root_at + o + 4].copy_from_slice(&u32::try_from(v).unwrap().to_le_bytes());
            }
        }
        let partial = crc(&b[CRC_START..CRC_START + CRC_PARTIAL]);
        b[4..8].copy_from_slice(&partial.to_le_bytes());
        if format.wide() {
            let full = crc(&b[CRC_START..CRC_START + CRC_FULL]);
            b[524..528].copy_from_slice(&full.to_le_bytes());
        }
        b
    }

    #[test]
    fn a_header_reads_where_its_trees_are() {
        for format in [Format::Unicode, Format::Ansi] {
            let (nbt, bbt) = (Bref { bid: 0x20, ib: 0x4600 }, Bref { bid: 0x24, ib: 0x4800 });
            let h = Header::parse(&header(format, nbt, bbt, 1)).unwrap();
            assert_eq!((h.format, h.crypt, h.root.nbt, h.root.bbt), (format, Crypt::Permute, nbt, bbt));
        }
    }

    #[test]
    fn a_damaged_header_is_refused() {
        for format in [Format::Unicode, Format::Ansi] {
            let mut b = header(format, Bref::default(), Bref::default(), 0);
            b[100] ^= 1;
            assert!(Header::parse(&b).is_err());
        }
        assert!(Header::parse(b"not a pst at all").is_err());
        assert!(Header::parse(&[]).is_err());
        // An encoding no file has.
        assert!(Header::parse(&header(Format::Unicode, Bref::default(), Bref::default(), 9)).is_err());
    }
}
