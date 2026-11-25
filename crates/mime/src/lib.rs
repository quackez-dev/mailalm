//! Reading and writing mail.
//!
//! - [`parse`] reads a whole message for the reading pane: who, when, the text (HTML converted to
//!   text, PLAN.md section 10.2: no web engine), and the **list** of attachments. It never decodes an
//!   attachment's content: what is done with an attachment is decided by the user (section 10.1).
//! - [`attachment`] decodes one attachment, only when the user asks to review it.
//! - [`summary_from_header`] reads the header block the sync fetches for the message list.
//! - [`build`] writes an outgoing message, with its attachments, with a Message-ID that names the
//!   sender's domain, never this computer's name.
//!
//! Everything read here comes from strangers: header text is cleaned of control and direction
//! characters before it is shown ([`clean_line`]), and the HTML converted is capped in size.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mail_parser::{AddressList, MessagePart, MessageParser, PartKind};
use mailalm_model::{Flags, Person, Summary};

/// The most HTML converted to text: a larger body is cut (with a note) before conversion, which
/// keeps a hostile message from holding the reading pane.
pub const MAX_HTML: usize = 2 * 1024 * 1024;
/// The most text kept for the reading pane.
pub const MAX_TEXT: usize = 1024 * 1024;
/// How long a preview in the message list is, in characters.
pub const PREVIEW_CHARS: usize = 160;
/// The width HTML is laid out to as text. The reading pane wraps lines itself, so this only
/// decides where html2text breaks paragraphs it has to break (tables).
const HTML_WIDTH: usize = 200;
/// The largest attachment decoded for a review: a larger one is refused before it is decoded.
pub const MAX_ATTACHMENT: usize = 64 << 20;
/// The most of an `Authentication-Results` header kept.
const MAX_AUTH_HEADER: usize = 16 << 10;
/// The longest announced name kept (for the check on hidden characters).
const MAX_RAW_NAME: usize = 1024;

/// One attachment, as listed: never opened, never decoded here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentInfo {
    /// The name the message gives it (cleaned), or "attachment-<n>".
    pub name: String,
    /// The type the message declares (`application/pdf`…): not trusted, the real type is checked
    /// from the content at approval time (phase 5).
    pub declared_type: String,
    /// The decoded size in bytes.
    pub size: usize,
}

/// A message read for the reading pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub from: Person,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub reply_to: Vec<Person>,
    pub subject: String,
    /// Seconds since 1970 (UTC), from the Date header.
    pub date: Option<i64>,
    /// Without angle brackets.
    pub message_id: String,
    pub references: Vec<String>,
    /// The body as text.
    pub text: String,
    /// The text was converted from HTML.
    pub from_html: bool,
    pub attachments: Vec<AttachmentInfo>,
    /// The value of the first (topmost) `Authentication-Results` header: the sender checks the
    /// receiving provider made (read by `mailalm_safety::authentication`).
    pub authentication_results: Option<String>,
}

/// One attachment's content, decoded for a review the user asked for.
#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentData {
    pub info: AttachmentInfo,
    /// The name as the message announces it, before cleaning (hidden characters included), at
    /// most 1024 characters; empty when it has none.
    pub raw_name: String,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for AttachmentData {
    /// Without the content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachmentData").field("info", &self.info).field("raw_name", &self.raw_name).field("bytes", &self.bytes.len()).finish()
    }
}

/// Reads a whole message. `None` when it is not a message at all.
pub fn parse(raw: &[u8]) -> Option<Parsed> {
    let parser = MessageParser::default();
    let m = parser.parse(raw)?;
    let mut p = Parsed {
        from: m.from().and_then(|l| people(&l).into_iter().next()).unwrap_or_default(),
        to: m.to().map(|l| people(&l)).unwrap_or_default(),
        cc: m.cc().map(|l| people(&l)).unwrap_or_default(),
        reply_to: m.reply_to().map(|l| people(&l)).unwrap_or_default(),
        subject: clean_line(m.subject().unwrap_or("")),
        date: m.date().map(|d| d.to_timestamp()),
        message_id: m.message_id().map(clean_id).unwrap_or_default(),
        references: m.references().map(|l| l.iter().map(clean_id).collect()).unwrap_or_default(),
        authentication_results: m
            .headers()
            .all("Authentication-Results")
            .next()
            .map(|h| String::from_utf8_lossy(h.raw_value()).chars().take(MAX_AUTH_HEADER).collect()),
        ..Parsed::default()
    };

    // The text: the first part of the text body; an HTML part is converted with html2text.
    if let Some(part) = m.text_body().next() {
        match part.kind() {
            PartKind::Html => {
                if let Some(html) = part.text() {
                    p.text = html_to_text(&html);
                    p.from_html = true;
                }
            }
            PartKind::Text => p.text = part.text().map(|t| t.into_owned()).unwrap_or_default(),
            _ => {}
        }
    } else if let Some(html) = m.body_html(0) {
        p.text = html_to_text(&html);
        p.from_html = true;
    }
    p.text = clean_text(&p.text);

    for (n, part) in m.attachments().enumerate() {
        p.attachments.push(attachment_info(n, &part));
    }
    Some(p)
}

/// How an attachment is listed: its cleaned name (or `attachment-<n + 1>`), its declared type,
/// its decoded size.
fn attachment_info(n: usize, part: &MessagePart<'_>) -> AttachmentInfo {
    let declared_type = part
        .content_type()
        .map(|c| match c.subtype() {
            Some(sub) => format!("{}/{}", c.ctype(), sub),
            None => c.ctype().to_owned(),
        })
        .unwrap_or_else(|| "application/octet-stream".to_owned())
        .to_ascii_lowercase();
    let name = part.attachment_name().map(clean_file_name).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("attachment-{}", n + 1));
    AttachmentInfo { name, declared_type: clean_line(&declared_type), size: part.decoded_len() }
}

/// Decodes attachment `index` (in the order [`parse`] lists them) of a whole message: only when
/// the user asks to review it (PLAN.md, section 10.1). One larger than [`MAX_ATTACHMENT`] is
/// refused before it is decoded. The error is shown to the user as it is.
pub fn attachment(raw: &[u8], index: usize) -> Result<AttachmentData, String> {
    let parser = MessageParser::default();
    let m = parser.parse(raw).ok_or("This message could not be read.")?;
    let part = m.attachments().nth(index).ok_or("This attachment is no longer in the message.")?;
    let info = attachment_info(index, &part);
    if info.size > MAX_ATTACHMENT {
        return Err(format!("This attachment is larger than {} MB: MailALM does not decode it.", MAX_ATTACHMENT >> 20));
    }
    let raw_name = part.attachment_name().map(|n| n.chars().take(MAX_RAW_NAME).collect()).unwrap_or_default();
    let bytes = part.decoded().into_owned();
    if bytes.len() > MAX_ATTACHMENT {
        return Err(format!("This attachment is larger than {} MB: MailALM does not decode it.", MAX_ATTACHMENT >> 20));
    }
    Ok(AttachmentData { info, raw_name, bytes })
}

/// The summary the message list needs, from the header block the sync fetched. `internal_date`
/// (the server's arrival date) stands in for a missing or unreadable Date header.
pub fn summary_from_header(uid: u32, header: &[u8], flags: Flags, size: u32, internal_date: Option<i64>) -> Summary {
    let parser = MessageParser::default();
    let mut s = Summary { uid, flags, size, date: internal_date.unwrap_or(0), ..Summary::default() };
    if let Some(m) = parser.parse(header) {
        s.from = m.from().and_then(|l| people(&l).into_iter().next()).unwrap_or_default();
        s.to = m.to().map(|l| people(&l)).unwrap_or_default();
        s.cc = m.cc().map(|l| people(&l)).unwrap_or_default();
        s.subject = clean_line(m.subject().unwrap_or(""));
        if let Some(d) = m.date() {
            s.date = d.to_timestamp();
        }
        s.message_id = m.message_id().map(clean_id).unwrap_or_default();
        s.has_attachments = m.content_type().is_some_and(|c| {
            c.ctype().eq_ignore_ascii_case("multipart") && c.subtype().is_some_and(|sub| sub.eq_ignore_ascii_case("mixed"))
        });
    }
    s
}

/// Completes a summary once the whole message is in the cache: its preview and whether it really
/// has attachments.
pub fn complete_summary(s: &mut Summary, parsed: &Parsed) {
    s.preview = preview(&parsed.text);
    s.has_attachments = !parsed.attachments.is_empty();
}

/// The first words of a text, on one line.
pub fn preview(text: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        // Lines of quoted text and separators say little.
        if word.chars().all(|c| matches!(c, '>' | '-' | '_' | '=' | '*')) {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
            count += 1;
        }
        for c in word.chars() {
            if count >= PREVIEW_CHARS {
                out.push('…');
                return out;
            }
            out.push(c);
            count += 1;
        }
    }
    out
}

fn people(list: &AddressList<'_>) -> Vec<Person> {
    list.mailboxes()
        .map(|mb| Person { name: clean_line(mb.name().unwrap_or("")), address: clean_line(mb.address().unwrap_or("")) })
        .filter(|p| !p.address.is_empty() || !p.name.is_empty())
        .collect()
}

fn clean_id(id: &str) -> String {
    clean_line(id.trim().trim_start_matches('<').trim_end_matches('>'))
}

/// Whether a character can change how the text around it reads without showing itself: the
/// direction marks and overrides (which can turn `exe.pdf` into `fdp.exe`), and zero-width
/// characters.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{061c}')
}

/// One line of header text as it is shown: no control or invisible characters, spaces collapsed.
pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() {
            space = !out.is_empty();
            continue;
        }
        if is_invisible(c) {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    out
}

/// A body text as it is shown: line ends unified, control and invisible characters removed (tabs
/// kept), capped at [`MAX_TEXT`].
pub fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(MAX_TEXT));
    for c in s.chars() {
        if out.len() >= MAX_TEXT {
            out.push_str("\n\n[MailALM: the rest of this message is too long to show.]");
            break;
        }
        match c {
            '\r' => {}
            '\n' | '\t' => out.push(c),
            c if c.is_control() || is_invisible(c) => {}
            c => out.push(c),
        }
    }
    out
}

/// An attachment's name as it is shown and suggested when it is saved: a file name only (no
/// folder, no drive), without characters Windows forbids in a name.
pub fn clean_file_name(name: &str) -> String {
    let line = clean_line(name);
    let leaf = line.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = leaf.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') { '_' } else { c }).collect();
    cleaned.trim().trim_end_matches(['.', ' ']).to_owned()
}

/// HTML as text, with html2text; a body over [`MAX_HTML`] is cut first.
fn html_to_text(html: &str) -> String {
    let (html, cut) = if html.len() > MAX_HTML {
        let mut end = MAX_HTML;
        while !html.is_char_boundary(end) {
            end -= 1;
        }
        (&html[..end], true)
    } else {
        (html, false)
    };
    let mut text = match html2text::from_read(html.as_bytes(), HTML_WIDTH) {
        Ok(t) => t,
        // html2text could not lay it out: mail-parser's own plain conversion.
        Err(_) => mail_parser::html_to_text(html),
    };
    if cut {
        text.push_str("\n\n[MailALM: the rest of this message is too long to show.]");
    }
    text
}

/// A message to send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outgoing {
    pub from: Person,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub bcc: Vec<Person>,
    pub subject: String,
    pub text: String,
    /// The message replied to, without angle brackets.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Files the user attached.
    pub attachments: Vec<OutgoingAttachment>,
}

/// A file attached to a message being written.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct OutgoingAttachment {
    /// The file's name, as the recipient sees it.
    pub name: String,
    /// Its type, guessed from its extension ([`content_type_for`]).
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl OutgoingAttachment {
    /// An attachment named `name` (made safe for a header), its type guessed from the name.
    pub fn new(name: &str, bytes: Vec<u8>) -> Self {
        let name = outgoing_name(name);
        let content_type = content_type_for(&name).to_owned();
        OutgoingAttachment { name, content_type, bytes }
    }
}

impl fmt::Debug for OutgoingAttachment {
    /// Without the content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingAttachment").field("name", &self.name).field("content_type", &self.content_type).field("bytes", &self.bytes.len()).finish()
    }
}

impl Outgoing {
    /// Every address the message goes to (To, Cc and Bcc), for the SMTP envelope.
    pub fn recipients(&self) -> Vec<String> {
        self.to.iter().chain(&self.cc).chain(&self.bcc).map(|p| p.address.clone()).collect()
    }

    /// The attachments' total size, in bytes.
    pub fn attachments_size(&self) -> usize {
        attachments_size(&self.attachments)
    }

    /// About how large the written message is ([`estimated_size`]).
    pub fn estimated_size(&self) -> usize {
        estimated_size(&self.text, &self.attachments)
    }
}

/// The attachments' total size, in bytes.
pub fn attachments_size(attachments: &[OutgoingAttachment]) -> usize {
    attachments.iter().fold(0usize, |n, a| n.saturating_add(a.bytes.len()))
}

/// About how large a message with this text and these attachments is once written, a little
/// more rather than less: base64 makes an attachment a third larger (76 characters and a line
/// end for each 57 bytes), plus each part's header and the message's.
pub fn estimated_size(text: &str, attachments: &[OutgoingAttachment]) -> usize {
    let parts = attachments.iter().fold(0usize, |n, a| n.saturating_add(a.bytes.len().div_ceil(57).saturating_mul(78)).saturating_add(400));
    parts.saturating_add(text.len().saturating_mul(2)).saturating_add(2048)
}

/// An attachment's name as it is written in the message: no folder, no control or invisible
/// character, no quote or backslash, at most 200 characters; `attachment` when nothing is left.
pub fn outgoing_name(name: &str) -> String {
    let leaf = clean_file_name(name);
    let s: String = leaf.chars().filter(|c| !matches!(c, '"' | '\\')).take(200).collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "attachment".to_owned() } else { s }
}

/// The type of a file to send, from its extension; `application/octet-stream` when unknown.
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "htm" | "html" => "text/html",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        "gz" | "tgz" => "application/gzip",
        "doc" | "dot" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" | "pps" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "rtf" => "application/rtf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        // A message is sent as a file (`message/rfc822` may not be base64-encoded).
        _ => "application/octet-stream",
    }
}

/// Whether the text the user wrote mentions an attachment (English or French): "attach…",
/// "pièce jointe", "ci-joint", "joint(e)(s)". Quoted lines (`>`) and a forwarded message are not
/// the user's words and are skipped.
pub fn mentions_attachment(text: &str) -> bool {
    let mut own = String::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("---------- Forwarded message") || t.starts_with("-----Original Message") {
            break;
        }
        if t.starts_with('>') {
            continue;
        }
        own.push_str(&line.to_lowercase());
        own.push('\n');
    }
    if own.contains("attach") || own.contains("ci-joint") || own.contains("ci joint") {
        return true;
    }
    own.split(|c: char| !c.is_alphanumeric()).any(|w| matches!(w, "joint" | "jointe" | "joints" | "jointes"))
}

/// Writes the message (RFC 5322, plain text; `multipart/mixed` with its attachments) for SMTP. The
/// Bcc header is not written: Bcc recipients are only in the envelope. Returns the message and its
/// Message-ID.
pub fn build(out: &Outgoing) -> Result<(Vec<u8>, String), String> {
    write(out, false)
}

/// Writes the message for a provider's API (Microsoft Graph `sendMail`, Gmail `messages.send`),
/// which has no envelope: it takes the recipients from the To, Cc and Bcc headers, and removes
/// the Bcc header from what it delivers. Without it, Bcc recipients would get nothing.
pub fn build_for_api(out: &Outgoing) -> Result<(Vec<u8>, String), String> {
    write(out, true)
}

fn write(out: &Outgoing, with_bcc: bool) -> Result<(Vec<u8>, String), String> {
    if out.to.is_empty() && out.cc.is_empty() && out.bcc.is_empty() {
        return Err("The message has no recipient.".to_owned());
    }
    for p in std::iter::once(&out.from).chain(&out.to).chain(&out.cc).chain(&out.bcc) {
        if !address_is_valid(&p.address) {
            return Err(format!("\"{}\" is not an e-mail address.", p.address));
        }
    }
    let id = message_id(&out.from.address);
    let mut b = mail_builder::MessageBuilder::new()
        .from(addr(&out.from))
        .subject(out.subject.clone())
        .message_id(id.clone())
        .text_body(normalize_newlines(&out.text));
    if !out.to.is_empty() {
        b = b.to(out.to.iter().map(addr).collect::<Vec<_>>());
    }
    if !out.cc.is_empty() {
        b = b.cc(out.cc.iter().map(addr).collect::<Vec<_>>());
    }
    if with_bcc && !out.bcc.is_empty() {
        b = b.bcc(out.bcc.iter().map(addr).collect::<Vec<_>>());
    }
    if let Some(r) = &out.in_reply_to {
        b = b.in_reply_to(r.clone());
    }
    if !out.references.is_empty() {
        b = b.references(out.references.clone());
    }
    for a in &out.attachments {
        // The name again through `outgoing_name`: a message that came back from a failed send
        // keeps what it had.
        let name = outgoing_name(&a.name);
        let content_type = if a.content_type.is_empty() || !a.content_type.bytes().all(|c| c.is_ascii_graphic()) {
            content_type_for(&name).to_owned()
        } else {
            a.content_type.clone()
        };
        b = b.attachment(content_type, name, a.bytes.clone());
    }
    let raw = b.write_to_vec().map_err(|e| format!("The message could not be written: {e}"))?;
    Ok((raw, id))
}

fn addr(p: &Person) -> mail_builder::headers::address::Address<'static> {
    if p.name.trim().is_empty() {
        mail_builder::headers::address::Address::from(p.address.clone())
    } else {
        mail_builder::headers::address::Address::from((p.name.clone(), p.address.clone()))
    }
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// A Message-ID unique to this message: time, a counter and a per-run random key, at the sender's
/// domain. Never the computer's name (mail-builder's default).
pub fn message_id(from: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let domain = from.rsplit_once('@').map(|(_, d)| d).filter(|d| !d.is_empty()).unwrap_or("mailalm.invalid");
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    // RandomState is seeded by the system's randomness: the key differs from run to run.
    let mut h = RandomState::new().build_hasher();
    h.write_u128(nanos);
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    format!("{:x}.{:016x}@{}", nanos, h.finish(), domain)
}

/// A plain check of an address: one `@`, something on each side, a dot in the domain, no spaces
/// or brackets. The server has the last word.
pub fn address_is_valid(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && a.len() <= 254
        && !a.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '(' | ')' | ',' | ';' | '"' | '[' | ']' | '\\'))
}

/// Reads a list of recipients typed by the user: `Name <a@b.c>, d@e.f; …`.
pub fn parse_recipients(input: &str) -> Result<Vec<Person>, String> {
    let mut out = Vec::new();
    for item in split_recipients(input) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let person = match (item.rfind('<'), item.rfind('>')) {
            (Some(open), Some(close)) if open < close => Person {
                name: item[..open].trim().trim_matches('"').trim().to_owned(),
                address: item[open + 1..close].trim().to_owned(),
            },
            _ => Person { name: String::new(), address: item.to_owned() },
        };
        if !address_is_valid(&person.address) {
            return Err(format!("\"{}\" is not an e-mail address.", item));
        }
        out.push(person);
    }
    Ok(out)
}

/// Splits on commas and semicolons outside quotes and angle brackets.
fn split_recipients(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut angle) = (false, false);
    for c in input.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            '<' if !quoted => {
                angle = true;
                cur.push(c);
            }
            '>' if !quoted => {
                angle = false;
                cur.push(c);
            }
            ',' | ';' if !quoted && !angle => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// A person as typed in a recipient field.
pub fn format_person(p: &Person) -> String {
    if p.name.trim().is_empty() {
        p.address.clone()
    } else if p.name.contains([',', ';', '<', '>', '"']) {
        format!("\"{}\" <{}>", p.name.replace('"', ""), p.address)
    } else {
        format!("{} <{}>", p.name, p.address)
    }
}

/// What a reply starts with: to the sender (Reply-To first), or to everyone but me, the subject
/// with "Re: ", the thread's references and the quoted text.
pub fn reply(p: &Parsed, me: &Person, all: bool, date_line: &str) -> Outgoing {
    let first: Vec<Person> = if p.reply_to.is_empty() { vec![p.from.clone()] } else { p.reply_to.clone() };
    let mine = |x: &Person| x.address.eq_ignore_ascii_case(&me.address);
    let mut to: Vec<Person> = first.into_iter().filter(|x| !mine(x)).collect();
    let mut cc = Vec::new();
    if all {
        for x in p.to.iter().chain(&p.cc) {
            if !mine(x) && !to.iter().chain(&cc).any(|y: &Person| y.address.eq_ignore_ascii_case(&x.address)) {
                cc.push(x.clone());
            }
        }
    }
    if to.is_empty() {
        // Replying to my own message: to its recipients.
        to = p.to.iter().filter(|x| !mine(x)).cloned().collect();
    }
    let mut references = p.references.clone();
    if !p.message_id.is_empty() {
        references.push(p.message_id.clone());
    }
    Outgoing {
        from: me.clone(),
        to,
        cc,
        subject: prefixed("Re: ", &p.subject),
        text: format!("\n\n{} {} wrote:\n{}", date_line, p.from.display(), quote(&p.text)),
        in_reply_to: (!p.message_id.is_empty()).then(|| p.message_id.clone()),
        references,
        ..Outgoing::default()
    }
}

/// What a forward starts with: no recipient, "Fwd: ", and the original message's header and text.
/// The original's attachments are not carried over: the user attaches what should go on.
pub fn forward(p: &Parsed, me: &Person, date_line: &str) -> Outgoing {
    let list = |v: &[Person]| v.iter().map(format_person).collect::<Vec<_>>().join(", ");
    let mut header = format!("\n\n---------- Forwarded message ----------\nFrom: {}\nDate: {}\nSubject: {}\nTo: {}\n", format_person(&p.from), date_line, p.subject, list(&p.to));
    if !p.cc.is_empty() {
        header.push_str(&format!("Cc: {}\n", list(&p.cc)));
    }
    Outgoing { from: me.clone(), subject: prefixed("Fwd: ", &p.subject), text: format!("{header}\n{}", p.text), ..Outgoing::default() }
}

fn prefixed(prefix: &str, subject: &str) -> String {
    let lower = subject.to_lowercase();
    let p = prefix.trim().to_lowercase();
    if lower.starts_with(&p) || (prefix == "Fwd: " && (lower.starts_with("fw:") || lower.starts_with("tr:"))) || (prefix == "Re: " && lower.starts_with("re :")) {
        subject.to_owned()
    } else {
        format!("{prefix}{subject}")
    }
}

fn quote(text: &str) -> String {
    text.lines().map(|l| if l.starts_with('>') { format!(">{l}") } else { format!("> {l}") }).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests;
