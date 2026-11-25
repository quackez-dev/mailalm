//! An Outlook item as a standard message (RFC 5322 and MIME), from its properties (MS-OXCMSG,
//! MS-OXPROPS).
//!
//! The header: the original one when the file kept it (`PidTagTransportMessageHeaders`, with a
//! `From` field), less its `Content-*` and `MIME-Version` fields; else one written from the item's
//! sender, recipients, subject, date and message IDs. The body: the text and the HTML the item
//! keeps, then its attachments: files, and attached messages rebuilt the same way, at most
//! [`MAX_EMBEDDED`] deep. Everything taken from the file is cleaned before it goes in a header
//! (control characters, line ends, brackets), and the files of one item are capped in number and
//! in bytes ([`Budget`]).

use std::rc::Rc;

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use mail_builder::headers::date::Date;
use mail_builder::mime::MimePart;
use mailalm_model::{Flags, Person};
use mailalm_pst::{Attachment, AttachmentData, Message, Properties, PropertyValue, Row, Table};

use crate::{MAX_MESSAGE, Skip};

// Properties of a message (MS-OXPROPS).
const MESSAGE_CLASS: u16 = 0x001A;
const SUBJECT: u16 = 0x0037;
const CLIENT_SUBMIT_TIME: u16 = 0x0039;
const SENT_REPRESENTING_NAME: u16 = 0x0042;
const SENT_REPRESENTING_EMAIL: u16 = 0x0065;
const TRANSPORT_HEADERS: u16 = 0x007D;
const SENDER_NAME: u16 = 0x0C1A;
const SENDER_EMAIL: u16 = 0x0C1F;
const DELIVERY_TIME: u16 = 0x0E06;
const MESSAGE_FLAGS: u16 = 0x0E07;
const BODY: u16 = 0x1000;
const HTML: u16 = 0x1013;
const INTERNET_MESSAGE_ID: u16 = 0x1035;
const INTERNET_REFERENCES: u16 = 0x1039;
const IN_REPLY_TO: u16 = 0x1042;
const LAST_VERB_EXECUTED: u16 = 0x1081;
const FLAG_STATUS: u16 = 0x1090;
const CREATION_TIME: u16 = 0x3007;
const INTERNET_CODEPAGE: u16 = 0x3FDE;
const MESSAGE_CODEPAGE: u16 = 0x3FFD;
const SENDER_SMTP: u16 = 0x5D01;
const SENT_REPRESENTING_SMTP: u16 = 0x5D02;
// Of a recipient.
const RECIPIENT_TYPE: u16 = 0x0C15;
const DISPLAY_NAME: u16 = 0x3001;
const EMAIL_ADDRESS: u16 = 0x3003;
const SMTP_ADDRESS: u16 = 0x39FE;
// Of an attachment.
const ATTACH_FILENAME: u16 = 0x3704;
const ATTACH_LONG_FILENAME: u16 = 0x3707;
const ATTACH_MIME_TAG: u16 = 0x370E;
const ATTACH_CONTENT_ID: u16 = 0x3712;

/// Attached messages in attached messages… followed this deep; deeper ones are left out.
pub(crate) const MAX_EMBEDDED: usize = 8;
/// Attachments one item (its attached messages' included) brings at most.
const MAX_PARTS: usize = 1000;
/// Recipients of one item kept at most.
const MAX_RECIPIENTS: usize = 10_000;
/// The longest original header kept; a longer one is passed over for one made from the
/// properties.
const MAX_HEADERS: usize = 256 << 10;
/// The longest message ID kept, in characters.
const MAX_ID: usize = 900;
/// Seconds between 1601-01-01 (a FILETIME's start) and 1970-01-01.
const FILETIME_TO_UNIX: i64 = 11_644_473_600;

/// What the files of one item may still bring: attachments, and their bytes (a third less than
/// [`MAX_MESSAGE`], what base64 makes of them fits).
pub(crate) struct Budget {
    parts: usize,
    bytes: usize,
}

impl Budget {
    pub(crate) fn new() -> Self {
        Budget { parts: MAX_PARTS, bytes: MAX_MESSAGE / 4 * 3 }
    }
}

/// The item's message class (`IPM.Note`…), empty when it has none.
pub(crate) fn message_class(m: &Message) -> String {
    text(m.properties().get(MESSAGE_CLASS), 1252).unwrap_or_default()
}

/// The marks an item keeps: read, a draft (not sent), flagged for follow-up, replied to.
pub(crate) fn flags(p: &Properties) -> Flags {
    let message_flags = int(p.get(MESSAGE_FLAGS)).unwrap_or(1);
    Flags {
        seen: message_flags & 0x01 != 0,
        draft: message_flags & 0x08 != 0,
        flagged: int(p.get(FLAG_STATUS)) == Some(2),
        // Reply, reply to all.
        answered: matches!(int(p.get(LAST_VERB_EXECUTED)), Some(102 | 103)),
    }
}

/// The message of item `m`, `depth` attached messages down. Files that can't be read, past the
/// budget or past [`MAX_EMBEDDED`] are left out and counted in `left_out`.
pub(crate) fn rebuild(m: &Rc<Message>, depth: usize, budget: &mut Budget, left_out: &mut u64) -> Result<Vec<u8>, Skip> {
    let p = m.properties();
    let cp = code_page(p);
    let mut b = MessageBuilder::new();
    let html = text(p.get(HTML), cp).filter(|h| !h.trim().is_empty());
    if let Some(t) = text(p.get(BODY), cp).filter(|t| !t.trim().is_empty()) {
        b = b.text_body(t);
    }
    let has_html = html.is_some();
    if let Some(h) = html {
        b = b.html_body(h);
    }
    attachments(m, &mut b, has_html, depth, budget, left_out);
    let raw = match original_header(p, cp) {
        Some(header) => {
            let mut out = header.into_bytes();
            out.extend_from_slice(b"MIME-Version: 1.0\r\n");
            b.write_body(&mut out).map_err(|_| Skip::Unreadable)?;
            out
        }
        None => own_header(b, m, p, cp).write_to_vec().map_err(|_| Skip::Unreadable)?,
    };
    if raw.len() > MAX_MESSAGE { Err(Skip::TooLarge) } else { Ok(raw) }
}

/// The original header, as the item kept it, without what describes the original body (its
/// `Content-*` fields and `MIME-Version`), its lines ended by CRLF; `None` when there is none, it
/// is too long, or it has no `From` field.
fn original_header(p: &Properties, cp: u32) -> Option<String> {
    let h = text(p.get(TRANSPORT_HEADERS), cp)?;
    if h.len() > MAX_HEADERS {
        return None;
    }
    let mut out = String::with_capacity(h.len() + 64);
    let (mut keep, mut started, mut has_from) = (false, false, false);
    for line in h.split('\n') {
        let line: String = line.chars().filter(|c| *c == '\t' || !c.is_control()).collect();
        if line.trim().is_empty() {
            // The header ends at its first empty line.
            if started {
                break;
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if keep {
                out.push_str(&line);
                out.push_str("\r\n");
            }
            continue;
        }
        let name = line.split_once(':').map(|(n, _)| n).unwrap_or("");
        if name.is_empty() || !name.bytes().all(|c| (33..=126).contains(&c)) {
            // Not a header field: dropped, with any line folded under it.
            keep = false;
            continue;
        }
        started = true;
        let lower = name.to_ascii_lowercase();
        keep = !(lower.starts_with("content-") || lower == "mime-version");
        has_from |= lower == "from";
        if keep {
            out.push_str(&line);
            out.push_str("\r\n");
        }
    }
    has_from.then_some(out)
}

/// A header made from the item's properties.
fn own_header(mut b: MessageBuilder<'static>, m: &Message, p: &Properties, cp: u32) -> MessageBuilder<'static> {
    let from = sender(p, cp);
    if let Some(a) = address(&from) {
        b = b.from(a);
    }
    let [to, cc, bcc] = recipients(m.recipient_table(), cp);
    let list = |people: &[Person]| people.iter().filter_map(address).collect::<Vec<_>>();
    let (to, cc, bcc) = (list(&to), list(&cc), list(&bcc));
    if !to.is_empty() {
        b = b.to(to);
    }
    if !cc.is_empty() {
        b = b.cc(cc);
    }
    if !bcc.is_empty() {
        b = b.bcc(bcc);
    }
    if let Some(s) = text(p.get(SUBJECT), cp) {
        b = b.subject(mailalm_mime::clean_line(&s));
    }
    if let Some(t) = time(p.get(CLIENT_SUBMIT_TIME)).or_else(|| time(p.get(DELIVERY_TIME))).or_else(|| time(p.get(CREATION_TIME))) {
        b = b.date(Date::new(t));
    }
    let id = text(p.get(INTERNET_MESSAGE_ID), cp).and_then(|s| ids(&s).into_iter().next()).unwrap_or_else(|| mailalm_mime::message_id(&from.address));
    b = b.message_id(id);
    if let Some(r) = text(p.get(IN_REPLY_TO), cp).and_then(|s| ids(&s).into_iter().next()) {
        b = b.in_reply_to(r);
    }
    let references = text(p.get(INTERNET_REFERENCES), cp).map(|s| ids(&s)).unwrap_or_default();
    if !references.is_empty() {
        b = b.references(references);
    }
    b
}

/// The item's sender, as its `From` names it: the one it was sent for, else the one who sent it;
/// the first of their addresses that is an e-mail address (an Exchange item names its people by
/// an X.500 path too).
fn sender(p: &Properties, cp: u32) -> Person {
    let name = text(p.get(SENT_REPRESENTING_NAME), cp).or_else(|| text(p.get(SENDER_NAME), cp)).unwrap_or_default();
    let address = [SENT_REPRESENTING_SMTP, SENT_REPRESENTING_EMAIL, SENDER_SMTP, SENDER_EMAIL]
        .iter()
        .filter_map(|id| text(p.get(*id), cp))
        .map(|a| clean_address(&a))
        .find(|a| a.contains('@'))
        .unwrap_or_default();
    Person { name: mailalm_mime::clean_line(&name), address }
}

/// The recipients, in To, Cc and Bcc.
fn recipients(table: Option<&Table>, cp: u32) -> [Vec<Person>; 3] {
    let mut out: [Vec<Person>; 3] = Default::default();
    let Some(t) = table else { return out };
    let col = |id: u16| t.column(id);
    let (kind_col, name_col, smtp_col, email_col) = (col(RECIPIENT_TYPE), col(DISPLAY_NAME), col(SMTP_ADDRESS), col(EMAIL_ADDRESS));
    let cell = |row: &Row, c: Option<usize>| c.and_then(|c| t.value(row, c));
    for row in t.rows_matrix().take(MAX_RECIPIENTS) {
        let slot = match int(cell(row, kind_col).as_ref()) {
            Some(2) => 1,
            Some(3) => 2,
            _ => 0,
        };
        let name = text(cell(row, name_col).as_ref(), cp).unwrap_or_default();
        let address = [smtp_col, email_col]
            .into_iter()
            .filter_map(|c| text(cell(row, c).as_ref(), cp))
            .map(|a| clean_address(&a))
            .find(|a| a.contains('@'))
            .unwrap_or_default();
        let person = Person { name: mailalm_mime::clean_line(&name), address };
        if !person.name.is_empty() || !person.address.is_empty() {
            if let Some(list) = out.get_mut(slot) {
                list.push(person);
            }
        }
    }
    out
}

/// A person as a header names them; `None` with neither a name nor an address. Without an
/// e-mail address (an Exchange path), the name alone.
fn address(p: &Person) -> Option<Address<'static>> {
    match (p.name.is_empty(), p.address.is_empty()) {
        (true, true) => None,
        (true, false) => Some(Address::from(p.address.clone())),
        (false, _) => Some(Address::from((p.name.clone(), p.address.clone()))),
    }
}

/// An address as a header takes it: no space, control character, bracket, quote, comma or
/// semicolon; at most 254 characters.
fn clean_address(a: &str) -> String {
    a.trim()
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '<' | '>' | '(' | ')' | ',' | ';' | '"' | '\\' | '[' | ']'))
        .take(254)
        .collect()
}

/// The message IDs in `s` (`<a@b> <c@d>`), without their brackets, each cleaned (visible ASCII,
/// no bracket) and at most [`MAX_ID`] characters; at most 100 of them.
fn ids(s: &str) -> Vec<String> {
    s.split(|c: char| c.is_whitespace() || c == ',' || c == '<' || c == '>')
        .map(|id| id.chars().filter(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '\\')).take(MAX_ID).collect::<String>())
        .filter(|id| id.contains('@'))
        .take(100)
        .collect()
}

/// The item's attachments, put on `b`: each file, and each attached message rebuilt.
fn attachments(m: &Rc<Message>, b: &mut MessageBuilder<'static>, has_html: bool, depth: usize, budget: &mut Budget, left_out: &mut u64) {
    let Some(table) = m.attachment_table() else { return };
    let rows: Vec<u32> = table.rows_matrix().map(|r| r.id()).collect();
    for nid in rows {
        if budget.parts == 0 {
            *left_out += 1;
            continue;
        }
        budget.parts -= 1;
        let Ok(mut a) = Attachment::read(m.clone(), nid) else {
            *left_out += 1;
            continue;
        };
        let (name, content_type, cid) = {
            let p = a.properties();
            let name = [ATTACH_LONG_FILENAME, ATTACH_FILENAME, DISPLAY_NAME].iter().filter_map(|id| text(p.get(*id), 1252)).find(|n| !n.trim().is_empty()).unwrap_or_default();
            let content_type = text(p.get(ATTACH_MIME_TAG), 1252).and_then(|t| clean_type(&t));
            let cid = text(p.get(ATTACH_CONTENT_ID), 1252).and_then(|c| ids_or_token(&c));
            (name, content_type, cid)
        };
        let part = match a.take_data() {
            Some(AttachmentData::Binary(bytes)) => {
                let data = bytes.0;
                if data.len() > budget.bytes {
                    *left_out += 1;
                    continue;
                }
                budget.bytes -= data.len();
                let name = mailalm_mime::outgoing_name(&name);
                let content_type = content_type.unwrap_or_else(|| mailalm_mime::content_type_for(&name).to_owned());
                let part = MimePart::new(content_type, data);
                match cid {
                    // A picture the HTML shows.
                    Some(cid) if has_html => part.inline().cid(cid),
                    _ => part.attachment(name),
                }
            }
            Some(AttachmentData::Message(inner)) => {
                if depth + 1 > MAX_EMBEDDED {
                    *left_out += 1;
                    continue;
                }
                let raw = match rebuild(&inner, depth + 1, budget, left_out) {
                    Ok(raw) if raw.len() <= budget.bytes => raw,
                    _ => {
                        *left_out += 1;
                        continue;
                    }
                };
                budget.bytes -= raw.len();
                let subject = text(inner.properties().get(SUBJECT), code_page(inner.properties())).unwrap_or_default();
                let stem = if name.trim().is_empty() { subject } else { name };
                let name = mailalm_mime::outgoing_name(&format!("{}.eml", stem.trim_end_matches(".eml").trim_end_matches(".msg")));
                // Sent as it is (RFC 2046 allows no other encoding for a message).
                let encoding = if raw.is_ascii() { "7bit" } else { "8bit" };
                MimePart::new("message/rfc822", raw).attachment(name).transfer_encoding(encoding)
            }
            // A link to a file elsewhere: nothing to bring.
            None => {
                *left_out += 1;
                continue;
            }
        };
        b.attachments.get_or_insert_with(Vec::new).push(part);
    }
}

/// A declared type kept only when it is one (`type/subtype`, the characters a token has),
/// lowercased; at most 127 characters.
fn clean_type(t: &str) -> Option<String> {
    let t = t.trim().to_ascii_lowercase();
    let token = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "!#$&^_.+-".contains(c));
    let (top, sub) = t.split_once('/')?;
    (t.len() <= 127 && token(top) && token(sub)).then_some(t.clone())
}

/// A Content-ID as the part's header takes it: without brackets, visible ASCII only, at most
/// [`MAX_ID`] characters; `None` when nothing is left.
fn ids_or_token(c: &str) -> Option<String> {
    let id: String = c.chars().filter(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '\\')).take(MAX_ID).collect();
    (!id.is_empty()).then_some(id)
}

/// The code page the item's 8-bit strings and HTML are in.
fn code_page(p: &Properties) -> u32 {
    [INTERNET_CODEPAGE, MESSAGE_CODEPAGE].iter().filter_map(|id| int(p.get(*id))).find(|cp| *cp > 0).and_then(|cp| u32::try_from(cp).ok()).unwrap_or(1252)
}

fn int(v: Option<&PropertyValue>) -> Option<i32> {
    match v? {
        PropertyValue::Integer32(n) => Some(*n),
        PropertyValue::Integer16(n) => Some(i32::from(*n)),
        _ => None,
    }
}

/// A time (a FILETIME) as seconds since 1970; `None` before 1970 or after 9999.
fn time(v: Option<&PropertyValue>) -> Option<i64> {
    match v? {
        PropertyValue::Time(ft) => {
            let t = ft.div_euclid(10_000_000) - FILETIME_TO_UNIX;
            (0..=253_402_300_799).contains(&t).then_some(t)
        }
        _ => None,
    }
}

/// A string value: UTF-16, or 8-bit text (or bytes, for HTML) in code page `cp`; without NULs.
pub(crate) fn text(v: Option<&PropertyValue>, cp: u32) -> Option<String> {
    let s = match v? {
        PropertyValue::Unicode(s) => String::from_utf16_lossy(s.buffer()),
        PropertyValue::String8(b) | PropertyValue::Binary(b) => decode(b.buffer(), cp),
        _ => return None,
    };
    Some(s.replace('\0', ""))
}

/// 8-bit text in code page `cp`: UTF-8 (65001), else Windows-1252 (which reads ISO-8859-1 and
/// ASCII too). Text in another code page reads as UTF-8 when it is valid UTF-8, else as
/// Windows-1252 (its letters outside ASCII may then be wrong).
fn decode(b: &[u8], cp: u32) -> String {
    match cp {
        65001 => String::from_utf8_lossy(b).into_owned(),
        1252 | 28591 | 20127 | 0 => windows_1252(b),
        _ => match std::str::from_utf8(b) {
            Ok(s) => s.to_owned(),
            Err(_) => windows_1252(b),
        },
    }
}

/// Windows-1252 text.
fn windows_1252(b: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š',
        '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    b.iter().map(|&c| if (0x80..0xA0).contains(&c) { HIGH.get(usize::from(c - 0x80)).copied().unwrap_or('?') } else { char::from(c) }).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn eight_bit_text_reads_in_its_code_page() {
        assert_eq!(decode(b"caf\xe9 \x80", 1252), "café €");
        assert_eq!(decode("café".as_bytes(), 65001), "café");
        assert_eq!(decode("café".as_bytes(), 1251), "café", "valid UTF-8 in another code page");
        assert_eq!(decode(b"\x93q\x94", 1251), "“q”");
    }

    #[test]
    fn ids_are_cleaned() {
        assert_eq!(ids("<a@b.example> <c@d.example>\r\n\t<x y@z>"), vec!["a@b.example", "c@d.example", "y@z"]);
        assert!(ids("no ids here").is_empty());
        assert_eq!(ids_or_token("<pic\r\n1@x>").as_deref(), Some("pic1@x"));
        assert_eq!(clean_type("Image/PNG"), Some("image/png".to_owned()));
        assert_eq!(clean_type("text/html\r\nX-Evil: 1"), None);
        assert_eq!(clean_type("nonsense"), None);
        assert_eq!(clean_address(" <a@b.example>\r\n"), "a@b.example");
    }

    #[test]
    fn times_are_filetimes() {
        // 2026-11-25 00:00:00 UTC.
        let unix = 1_795_564_800i64;
        let ft = (unix + FILETIME_TO_UNIX) * 10_000_000;
        assert_eq!(time(Some(&PropertyValue::Time(ft))), Some(unix));
        assert_eq!(time(Some(&PropertyValue::Time(0))), None, "before 1970");
        assert_eq!(time(Some(&PropertyValue::Integer32(5))), None);
    }
}
