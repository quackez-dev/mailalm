//! A small Outlook data file made here, byte by byte as MS-PST lays one out (Unicode format, no
//! block encoding), read whole, then cut short, damaged and made to name its own folders.
#![allow(clippy::unwrap_used)]

use std::io::Cursor;

use mailalm_pst::{PstFile, crc};

use super::*;

/// A heap allocation's ID, in the heap's first block.
fn hid(index: u32) -> u32 {
    index << 5
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// A heap of one block: its root is allocation 1.
fn heap(client: u8, allocations: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 12];
    let mut offsets = vec![12u16];
    for a in allocations {
        b.extend_from_slice(a);
        offsets.push(u16::try_from(b.len()).unwrap());
    }
    if b.len() % 2 == 1 {
        b.push(0);
    }
    let map = u16::try_from(b.len()).unwrap();
    b.extend(u16::try_from(allocations.len()).unwrap().to_le_bytes());
    b.extend(0u16.to_le_bytes());
    for o in offsets {
        b.extend(o.to_le_bytes());
    }
    b[0..2].copy_from_slice(&map.to_le_bytes());
    b[2] = 0xEC;
    b[3] = client;
    b[4..8].copy_from_slice(&hid(1).to_le_bytes());
    b
}

/// A property's value.
enum V<'a> {
    Int(i32),
    Text(&'a str),
    Bin(&'a [u8]),
    Time(i64),
}

/// A set of properties (a Property Context).
fn pc(props: &[(u16, V<'_>)]) -> Vec<u8> {
    let mut sorted: Vec<&(u16, V<'_>)> = props.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    let (mut values, mut records): (Vec<Vec<u8>>, Vec<u8>) = (Vec::new(), Vec::new());
    for (id, v) in sorted {
        records.extend(id.to_le_bytes());
        let (kind, inline) = match v {
            V::Int(n) => (0x0003u16, Some(u32::from_le_bytes(n.to_le_bytes()))),
            V::Text(s) => {
                values.push(utf16(s));
                (0x001F, None)
            }
            V::Bin(b) => {
                values.push(b.to_vec());
                (0x0102, None)
            }
            V::Time(t) => {
                values.push(t.to_le_bytes().to_vec());
                (0x0040, None)
            }
        };
        records.extend(kind.to_le_bytes());
        let slot = inline.unwrap_or_else(|| hid(2 + u32::try_from(values.len()).unwrap()));
        records.extend(slot.to_le_bytes());
    }
    let mut header = vec![0xB5, 2, 6, 0];
    header.extend(hid(2).to_le_bytes());
    let mut allocations = vec![header, records];
    allocations.extend(values);
    heap(0xBC, &allocations)
}

/// A table cell.
enum C<'a> {
    Int(i32),
    Text(&'a str),
}

/// A table (a Table Context) of columns `(property, type)`, every cell 4 bytes, the first column
/// each row's ID.
fn tc(columns: &[(u16, u16)], rows: &[Vec<C<'_>>]) -> Vec<u8> {
    let n = columns.len();
    let data_end = 4 * n;
    let width = data_end + n.div_ceil(8);
    let (mut values, mut row_bytes): (Vec<Vec<u8>>, Vec<u8>) = (Vec::new(), Vec::new());
    for row in rows {
        let mut r = vec![0u8; width];
        for (i, c) in row.iter().enumerate() {
            let v = match c {
                C::Int(x) => u32::from_le_bytes(x.to_le_bytes()),
                C::Text(s) => {
                    values.push(utf16(s));
                    hid(2 + u32::try_from(values.len()).unwrap())
                }
            };
            r[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
            r[data_end + i / 8] |= 0x80 >> (i % 8);
        }
        row_bytes.extend(r);
    }
    let mut info = vec![0x7C, u8::try_from(n).unwrap()];
    let (end, w) = (u16::try_from(data_end).unwrap(), u16::try_from(width).unwrap());
    for x in [end, end, end, w] {
        info.extend(x.to_le_bytes());
    }
    info.extend(0u32.to_le_bytes());
    info.extend((if rows.is_empty() { 0 } else { hid(2) }).to_le_bytes());
    info.extend(0u32.to_le_bytes());
    for (i, (id, kind)) in columns.iter().enumerate() {
        info.extend((u32::from(*kind) | (u32::from(*id) << 16)).to_le_bytes());
        info.extend(u16::try_from(4 * i).unwrap().to_le_bytes());
        info.push(4);
        info.push(u8::try_from(i).unwrap());
    }
    let mut allocations = vec![info, row_bytes];
    allocations.extend(values);
    heap(0x7C, &allocations)
}

/// A table of row IDs only (a folder's contents or subfolders, a message's attachments).
fn ids_table(ids: &[u32]) -> Vec<u8> {
    let rows: Vec<Vec<C<'_>>> = ids.iter().map(|id| vec![C::Int(i32::from_le_bytes(id.to_le_bytes()))]).collect();
    tc(&[(0x67F2, 0x0003)], &rows)
}

/// A subnode block of `(node, data block, subnode block)` entries.
fn subnodes(entries: &[(u32, u64, u64)]) -> Vec<u8> {
    let mut b = vec![0x02, 0];
    b.extend(u16::try_from(entries.len()).unwrap().to_le_bytes());
    b.extend(0u32.to_le_bytes());
    for (nid, data, sub) in entries {
        b.extend(u64::from(*nid).to_le_bytes());
        b.extend(data.to_le_bytes());
        b.extend(sub.to_le_bytes());
    }
    b
}

/// A data file being made: its blocks and its nodes.
#[derive(Default)]
struct Maker {
    blocks: Vec<(u64, Vec<u8>)>,
    nodes: Vec<(u32, u64, u64)>,
}

const NBT_AT: usize = 1024;
const BBT_AT: usize = 1536;
const NBT_BID: u64 = 0x1000;
const BBT_BID: u64 = 0x1004;

impl Maker {
    fn block(&mut self, data: Vec<u8>) -> u64 {
        let bid = 4 * (self.blocks.len() as u64 + 1);
        self.blocks.push((bid, data));
        bid
    }

    fn node(&mut self, nid: u32, data: Vec<u8>, sub: u64) {
        let bid = self.block(data);
        self.nodes.push((nid, bid, sub));
    }

    /// A B-tree leaf page of `entries`, each `size` bytes.
    fn page(entries: &[Vec<u8>], size: usize, ptype: u8, bid: u64) -> Vec<u8> {
        let mut p = vec![0u8; 512];
        for (i, e) in entries.iter().enumerate() {
            p[i * size..i * size + e.len()].copy_from_slice(e);
        }
        p[488] = u8::try_from(entries.len()).unwrap();
        p[489] = u8::try_from(488 / size).unwrap();
        p[490] = u8::try_from(size).unwrap();
        p[491] = 0;
        p[496] = ptype;
        p[497] = ptype;
        p[504..512].copy_from_slice(&bid.to_le_bytes());
        let sum = crc(&p[..496]);
        p[500..504].copy_from_slice(&sum.to_le_bytes());
        p
    }

    fn bytes(mut self) -> Vec<u8> {
        let mut out = vec![0u8; 2048];
        let mut bbt = Vec::new();
        for (bid, data) in &self.blocks {
            let at = out.len() as u64;
            let total = (data.len() + 16).div_ceil(64) * 64;
            let mut b = vec![0u8; total];
            b[..data.len()].copy_from_slice(data);
            let t = total - 16;
            b[t..t + 2].copy_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
            b[t + 4..t + 8].copy_from_slice(&crc(data).to_le_bytes());
            b[t + 8..t + 16].copy_from_slice(&bid.to_le_bytes());
            out.extend(b);
            let mut e = bid.to_le_bytes().to_vec();
            e.extend(at.to_le_bytes());
            e.extend(u16::try_from(data.len()).unwrap().to_le_bytes());
            e.extend(1u16.to_le_bytes());
            e.extend(0u32.to_le_bytes());
            bbt.push(e);
        }
        self.nodes.sort_by_key(|(nid, _, _)| *nid);
        let nbt: Vec<Vec<u8>> = self
            .nodes
            .iter()
            .map(|(nid, data, sub)| {
                let mut e = u64::from(*nid).to_le_bytes().to_vec();
                e.extend(data.to_le_bytes());
                e.extend(sub.to_le_bytes());
                e.extend(0u32.to_le_bytes());
                e.extend(0u32.to_le_bytes());
                e
            })
            .collect();
        out[NBT_AT..NBT_AT + 512].copy_from_slice(&Self::page(&nbt, 32, 0x81, NBT_BID));
        out[BBT_AT..BBT_AT + 512].copy_from_slice(&Self::page(&bbt, 24, 0x80, BBT_BID));
        // The header (MS-PST 2.2.2.6, Unicode): its B-trees, no encoding, its two CRCs.
        out[..4].copy_from_slice(b"!BDN");
        out[8..10].copy_from_slice(b"SM");
        out[10..12].copy_from_slice(&23u16.to_le_bytes());
        for (at, v) in [(216, NBT_BID), (224, NBT_AT as u64), (232, BBT_BID), (240, BBT_AT as u64)] {
            out[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
        out[513] = 0;
        let partial = crc(&out[8..8 + 471]);
        out[4..8].copy_from_slice(&partial.to_le_bytes());
        let full = crc(&out[8..8 + 516]);
        out[524..528].copy_from_slice(&full.to_le_bytes());
        out
    }
}

const ROOT: u32 = 0x122;
const INBOX: u32 = 0x8022;

/// The node of a folder's table of `kind` (0x0D its subfolders, 0x0E its messages).
fn beside(folder: u32, kind: u32) -> u32 {
    (folder & !0x1F) | kind
}

/// A data file: the top folder, an Inbox under it with a message made from its properties (a
/// recipient, a file attached), one with its original header, and a contact. With `cyclic`, the
/// Inbox also names the top folder and itself as its subfolders.
fn sample(cyclic: bool) -> Vec<u8> {
    let mut m = Maker::default();
    let mut entry_id = vec![0u8; 20];
    entry_id.extend(ROOT.to_le_bytes());
    m.node(0x21, pc(&[(0x35E0, V::Bin(&entry_id)), (0x3001, V::Text("Personal Folders"))]), 0);
    m.node(ROOT, pc(&[(0x3001, V::Text("Top of Personal Folders"))]), 0);
    m.node(beside(ROOT, 0x0D), ids_table(&[INBOX]), 0);
    m.node(INBOX, pc(&[(0x3001, V::Text("Inbox"))]), 0);
    m.node(beside(INBOX, 0x0E), ids_table(&[0x20_0004, 0x20_0024, 0x20_0044]), 0);
    if cyclic {
        m.node(beside(INBOX, 0x0D), ids_table(&[ROOT, INBOX]), 0);
    }
    // Message A: no original header; a recipient and a file.
    let recipients = tc(
        &[(0x67F2, 0x0003), (0x0C15, 0x0003), (0x3001, 0x001F), (0x39FE, 0x001F)],
        &[vec![C::Int(0), C::Int(1), C::Text("Bob"), C::Text("bob@example.org")], vec![C::Int(1), C::Int(2), C::Text("Carl"), C::Text("carl@example.org")]],
    );
    let recipients = m.block(recipients);
    let attachments = m.block(ids_table(&[0x8005]));
    let file = m.block(pc(&[(0x3705, V::Int(1)), (0x3701, V::Bin(b"attached bytes")), (0x3707, V::Text("notes.txt")), (0x370E, V::Text("text/plain"))]));
    let subs = m.block(subnodes(&[(0x0671, attachments, 0), (0x0692, recipients, 0), (0x8005, file, 0)]));
    // 2026-11-25 10:00:00 UTC as a FILETIME.
    let sent = (1_795_600_800i64 + 11_644_473_600) * 10_000_000;
    m.node(
        0x20_0004,
        pc(&[
            (0x001A, V::Text("IPM.Note")),
            (0x0037, V::Text("Hello Zoé")),
            (0x0039, V::Time(sent)),
            (0x0042, V::Text("Ann")),
            (0x0065, V::Text("ann@example.org")),
            (0x0E07, V::Int(1)),
            (0x1000, V::Text("The body.\r\nFrom the start.\r\n")),
            (0x1035, V::Text("<a1@example.org>")),
        ]),
        subs,
    );
    // Message B: its original header, unread and flagged.
    let header = "Received: from mx.example.org\r\nFrom: Bob <bob@example.org>\r\nTo: me@example.org\r\nSubject: Original\r\nContent-Type: text/plain; charset=us-ascii\r\nMIME-Version: 1.0\r\nX-Folded: one\r\n two\r\n\r\n";
    m.node(0x20_0024, pc(&[(0x001A, V::Text("IPM.Note")), (0x007D, V::Text(header)), (0x1000, V::Text("B's body")), (0x0E07, V::Int(0)), (0x1090, V::Int(2))]), 0);
    // A contact: not mail.
    m.node(0x20_0044, pc(&[(0x001A, V::Text("IPM.Contact")), (0x3001, V::Text("Carol"))]), 0);
    m.bytes()
}

/// Imports `bytes` as a data file: the report and the items, or the error.
fn import_bytes(bytes: &[u8]) -> Result<(Report, Vec<Item>), String> {
    let file = PstFile::read_from(Box::new(Cursor::new(bytes.to_vec()))).map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    let mut sink = |item: Item| -> Result<(), String> {
        items.push(item);
        Ok(())
    };
    let report = import_from(file, &mut sink, &Cancel::new())?;
    Ok((report, items))
}

#[test]
fn a_data_file_reads_whole() {
    let (report, items) = import_bytes(&sample(false)).unwrap();
    assert_eq!(items.len(), 2, "{report:?}");
    assert_eq!(report.skipped.get(&Skip::NotMail), Some(&1));
    assert_eq!(report.folders, 2);
    assert!(items.iter().all(|i| i.folder == ["Inbox"]));

    // A: rebuilt from its properties.
    let a = &items[0];
    assert!(a.flags.seen && !a.flags.flagged);
    let p = mailalm_mime::parse(&a.raw).unwrap();
    assert_eq!(p.subject, "Hello Zoé");
    assert_eq!((p.from.name.as_str(), p.from.address.as_str()), ("Ann", "ann@example.org"));
    assert_eq!(p.to.iter().map(|t| t.address.as_str()).collect::<Vec<_>>(), ["bob@example.org"]);
    assert_eq!(p.cc.iter().map(|t| t.address.as_str()).collect::<Vec<_>>(), ["carl@example.org"]);
    assert_eq!(p.message_id, "a1@example.org");
    assert_eq!(p.date, Some(1_795_600_800));
    assert!(p.text.contains("The body.") && p.text.contains("From the start."), "{}", p.text);
    assert_eq!(p.attachments.len(), 1);
    assert_eq!(p.attachments[0].name, "notes.txt");
    assert_eq!(mailalm_mime::attachment(&a.raw, 0).unwrap().bytes, b"attached bytes");

    // B: its original header, less what described its original body.
    let b = &items[1];
    assert!(!b.flags.seen && b.flags.flagged);
    let text = String::from_utf8_lossy(&b.raw);
    assert!(text.starts_with("Received: from mx.example.org\r\nFrom: Bob <bob@example.org>\r\n"), "{text}");
    assert!(text.contains("X-Folded: one\r\n two\r\n"));
    assert!(!text.contains("us-ascii"), "the original Content-Type is gone");
    assert_eq!(text.matches("MIME-Version").count(), 1);
    let p = mailalm_mime::parse(&b.raw).unwrap();
    assert_eq!((p.subject.as_str(), p.from.address.as_str()), ("Original", "bob@example.org"));
    assert!(p.text.contains("B's body"));
}

#[test]
fn folders_that_name_each_other_are_read_once() {
    let (report, items) = import_bytes(&sample(true)).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(report.folders, 2);
    assert!(report.left_out >= 2, "{report:?}");
}

#[test]
fn a_file_cut_short_anywhere_ends() {
    let file = sample(true);
    for len in (0..file.len()).step_by(7) {
        if let Ok((_, items)) = import_bytes(&file[..len]) {
            assert!(items.len() <= 2);
        }
    }
}

/// A small generator, the same run each time.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
    }
}

#[test]
fn damage_anywhere_ends() {
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let file = sample(true);
    for _ in 0..1500 {
        let mut damaged = file.clone();
        for _ in 0..=random.below(16) {
            let at = random.below(damaged.len());
            damaged[at] = random.next().to_le_bytes()[0];
        }
        if let Ok((_, items)) = import_bytes(&damaged) {
            assert!(items.len() <= 2);
        }
    }
}

#[test]
fn what_is_not_a_data_file_is_an_error() {
    assert!(import_bytes(b"").is_err());
    assert!(import_bytes(&[0u8; 4096]).is_err());
    assert!(import_bytes(b"From someone\r\nSubject: an mbox\r\n").is_err());
    let missing = std::env::temp_dir().join(format!("mailalm-porting-none-{}.pst", std::process::id()));
    let mut sink = |_: Item| -> Result<(), String> { Ok(()) };
    assert!(import(&missing, &mut sink, &Cancel::new()).is_err());
}

#[test]
fn a_cancelled_import_stops() {
    let file = PstFile::read_from(Box::new(Cursor::new(sample(false)))).unwrap();
    let cancel = Cancel::new();
    cancel.cancel();
    let mut sink = |_: Item| -> Result<(), String> { Err("nothing should come".to_owned()) };
    let report = import_from(file, &mut sink, &cancel).unwrap();
    assert!(report.cancelled && report.messages == 0);
}

#[test]
fn mail_classes() {
    for mail in ["", "IPM.Note", "ipm.note.SMIME", "IPM.Schedule.Meeting.Request", "REPORT.IPM.Note.NDR", "IPM.Post"] {
        assert!(is_mail(mail), "{mail}");
    }
    for other in ["IPM.Contact", "IPM.Appointment", "IPM.Task", "IPM.StickyNote", "IPM.DistList"] {
        assert!(!is_mail(other), "{other}");
    }
}
