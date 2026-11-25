//! Gmail's JSON as MailALM's model: a message's summary, a label, a filter, ids. Everything here
//! came from the server: strings are cleaned (no control, direction or zero-width characters)
//! and clipped, numbers checked, ids held to what Gmail's look like.
//!
//! A message's header fields are read by the same reader as the IMAP sync's
//! ([`mailalm_mime::summary_from_header`]): address lists, encoded words (RFC 2047) and dates
//! come out the same whichever way the mail came.

use mailalm_model::{Flags, Summary};
use serde_json::{Map, Value};

use super::{Filter, FilterAction, Label, LabelKind};

/// The header fields asked for with a message's metadata: what the message list shows.
/// `In-Reply-To` and `References` follow the thread, for the follow-ups.
pub(super) const HEADERS: [&str; 9] = ["From", "To", "Cc", "Subject", "Date", "Message-ID", "In-Reply-To", "References", "Content-Type"];

/// Longest message or label id taken (Gmail's are 16 hex digits, `INBOX`, `Label_123`…).
const MAX_ID: usize = 256;
/// Longest filter id taken.
const MAX_FILTER_ID: usize = 1024;
/// Longest page token taken.
const MAX_PAGE_TOKEN: usize = 1024;
/// Header fields taken from one message, and the most of each one's value (in characters).
const MAX_FIELDS: usize = 32;
const MAX_FIELD: usize = 8 << 10;
/// Labels taken from one message.
const MAX_MESSAGE_LABELS: usize = 500;
/// Most of a snippet looked at (Gmail's are about 200 characters).
const MAX_SNIPPET: usize = 2000;
/// Latest `internalDate` taken, in milliseconds: the end of the year 9999.
const MAX_DATE_MS: i64 = 253_402_300_799_999;

/// Whether `id` can be a Gmail message or label id: ASCII letters, digits, `_` and `-`, not
/// empty, of a sane length. It goes into URL paths and queries, and into the cache.
pub(super) fn id_ok(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `id` can be a filter id: as [`id_ok`], with `=` and `.` too (an opaque token).
pub(super) fn filter_id_ok(id: &str) -> bool {
    (1..=MAX_FILTER_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'=' | b'.'))
}

/// Whether `t` can be a page token: printable ASCII, not empty, of a sane length (it goes back,
/// percent-encoded, into the next page's query).
pub(super) fn page_token_ok(t: &str) -> bool {
    (1..=MAX_PAGE_TOKEN).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether `a` is an address a filter of MailALM's can name: printable ASCII, one `@` with a
/// domain after it (nothing before it blocks the whole domain), at most 320 characters, and
/// nothing Gmail's search syntax reads as more than a word (quotes, brackets, a leading `-`, a
/// `:`, a `,`).
pub(super) fn address_ok(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else { return false };
    a.len() <= 320
        && !domain.is_empty()
        && !domain.contains('@')
        && !local.starts_with('-')
        && a.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'(' | b')' | b'{' | b'}' | b'[' | b']' | b'<' | b'>' | b',' | b';' | b':' | b'\\'))
}

/// Server text as MailALM shows it: control and invisible characters dropped, spaces made one,
/// at most `max` characters.
pub(super) fn clean(s: &str, max: usize) -> String {
    let capped: String = s.chars().take(max.saturating_mul(4).saturating_add(64)).collect();
    mailalm_mime::clean_line(&capped).chars().take(max).collect::<String>().trim_end().to_string()
}

/// A whole number as Google's JSON has it: a string of digits (its 64-bit numbers), or a number.
pub(super) fn number(v: Option<&Value>) -> Option<u64> {
    match v? {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// A label from the label list; `None` without a usable id.
pub(super) fn label_of(l: &Value) -> Option<Label> {
    let id = l.get("id").and_then(Value::as_str).filter(|id| id_ok(id))?;
    let name = clean(l.get("name").and_then(Value::as_str).unwrap_or_default(), 255);
    let system = l.get("type").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("system"));
    Some(Label {
        id: id.to_string(),
        name: if name.is_empty() { id.to_string() } else { name },
        kind: if system { LabelKind::System } else { LabelKind::User },
    })
}

/// A message's label ids, those that are ids, at most [`MAX_MESSAGE_LABELS`].
pub(super) fn labels_of(m: &Value) -> Vec<String> {
    m.get("labelIds")
        .and_then(Value::as_array)
        .map(|a| a.iter().take(MAX_MESSAGE_LABELS).filter_map(Value::as_str).filter(|l| id_ok(l)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// The header block the metadata's fields make: only the fields asked for, under their own
/// names, each value on one line (a line end in it would start a field of its own) and capped.
fn header_block(headers: Option<&Value>) -> Vec<u8> {
    let mut out = String::new();
    let fields = headers.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    for h in fields.iter().take(MAX_FIELDS) {
        let name = h.get("name").and_then(Value::as_str).unwrap_or_default().trim();
        let Some(known) = HEADERS.iter().find(|k| k.eq_ignore_ascii_case(name)) else { continue };
        let value: String =
            h.get("value").and_then(Value::as_str).unwrap_or_default().chars().take(MAX_FIELD).map(|c| if c.is_control() { ' ' } else { c }).collect();
        out.push_str(known);
        out.push_str(": ");
        out.push_str(value.trim());
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// `internalDate` (milliseconds since 1970, as a string) in seconds.
fn internal_date(v: Option<&Value>) -> Option<i64> {
    let ms = match v? {
        Value::String(s) => s.trim().parse::<i64>().ok()?,
        Value::Number(n) => n.as_i64()?,
        _ => return None,
    };
    (0..=MAX_DATE_MS).contains(&ms).then_some(ms / 1000)
}

/// A message's summary from its metadata and its `labels`: the marks from the labels (`UNREAD`,
/// `STARRED`, `DRAFT`; Gmail has no answered mark), the date from the Date field else the
/// arrival, the size Gmail estimates, the preview from Gmail's snippet. `uid` is 0 (the engine
/// numbers messages).
pub(super) fn summary_of(m: &Value, labels: &[String]) -> Summary {
    let has = |l: &str| labels.iter().any(|x| x == l);
    let flags = Flags { seen: !has("UNREAD"), flagged: has("STARRED"), answered: false, draft: has("DRAFT") };
    let size = m.get("sizeEstimate").and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
    let payload = m.get("payload");
    let mut s = mailalm_mime::summary_from_header(0, &header_block(payload.and_then(|p| p.get("headers"))), flags, size, internal_date(m.get("internalDate")));
    let mime_type = payload.and_then(|p| p.get("mimeType")).and_then(Value::as_str).unwrap_or_default();
    s.has_attachments |= mime_type.trim().eq_ignore_ascii_case("multipart/mixed");
    let snippet: String = m.get("snippet").and_then(Value::as_str).unwrap_or_default().chars().take(MAX_SNIPPET).collect();
    s.preview = mailalm_mime::preview(&mailalm_mime::clean_line(&entities(&snippet)));
    s.thread = m.get("threadId").and_then(Value::as_str).filter(|t| id_ok(t)).map(str::to_owned).unwrap_or_default();
    s.categories = labels.iter().filter_map(|l| category_name(l)).map(str::to_owned).collect();
    s
}

/// The name of an Inbox category's label (`CATEGORY_PROMOTIONS` is Promotions), as the search's
/// `category:` and the folder pane name them.
pub(super) fn category_name(label: &str) -> Option<&'static str> {
    match label {
        "CATEGORY_PERSONAL" => Some("Primary"),
        "CATEGORY_SOCIAL" => Some("Social"),
        "CATEGORY_PROMOTIONS" => Some("Promotions"),
        "CATEGORY_UPDATES" => Some("Updates"),
        "CATEGORY_FORUMS" => Some("Forums"),
        _ => None,
    }
}

/// Text with its HTML character references decoded, as Gmail escapes its snippets (`&amp;`,
/// `&lt;`, `&gt;`, `&quot;`, `&#39;`, and any by number). One it doesn't know is left as it is.
pub(super) fn entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at + 1..).unwrap_or_default();
        // A reference is short, and ends with `;`.
        let decoded = after.bytes().take(12).position(|b| b == b';').and_then(|end| Some((entity(after.get(..end)?)?, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = after.get(end + 1..).unwrap_or_default();
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character a reference's name (between `&` and `;`) stands for.
fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let digits = name.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) => u32::from_str_radix(hex, 16).ok()?,
                Some(_) => return None,
                None if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => digits.parse().ok()?,
                None => return None,
            };
            char::from_u32(code)
        }
    }
}

/// Whether a value says nothing: absent in effect (null, false, zero, empty).
pub(super) fn says_nothing(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_u64() == Some(0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// The label ids of an action's list, when it is a short list of strings.
pub(super) fn label_list(v: &Value) -> Option<Vec<&str>> {
    let a = v.as_array().filter(|a| a.len() <= 10)?;
    a.iter().map(Value::as_str).collect()
}

/// The keys of `o` that say something, with their values; `None` when one of them isn't
/// `allowed`.
fn only<'a>(o: &'a Map<String, Value>, allowed: &[&str]) -> Option<Vec<(&'a str, &'a Value)>> {
    let mut out = Vec::new();
    for (k, v) in o {
        if says_nothing(v) {
            continue;
        }
        if !allowed.contains(&k.as_str()) {
            return None;
        }
        out.push((k.as_str(), v));
    }
    Some(out)
}

/// A filter of the shape MailALM makes (see [`super::GmailClient::filters`]); `None` for any
/// other.
pub(super) fn filter_of(f: &Value) -> Option<Filter> {
    let id = f.get("id").and_then(Value::as_str).filter(|id| filter_id_ok(id))?;
    let criteria = only(f.get("criteria")?.as_object()?, &["from"])?;
    let [(_, from)] = criteria.as_slice() else { return None };
    let from = from.as_str()?.trim();
    if !address_ok(from) {
        return None;
    }
    let mut add = Vec::new();
    let mut remove = Vec::new();
    for (k, v) in only(f.get("action")?.as_object()?, &["addLabelIds", "removeLabelIds"])? {
        match k {
            "addLabelIds" => add = label_list(v)?,
            _ => remove = label_list(v)?,
        }
    }
    let action = match (add.as_slice(), remove.as_slice()) {
        (["SPAM"], []) => FilterAction::Spam,
        (["TRASH"], []) => FilterAction::Trash,
        ([], ["SPAM"]) => FilterAction::NeverSpam,
        _ => return None,
    };
    Some(Filter { id: id.to_string(), from: from.to_string(), action })
}
