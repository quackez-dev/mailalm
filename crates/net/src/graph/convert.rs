// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/exchange/graph/flags.rs (`flags`) and crates/net/src/exchange/graph.rs (`address`). MIT OR Apache-2.0.
// Changed: MailALM's Summary straight from Graph's fields (no header block, no follow-up days or reminders; the conversation and categories kept since phase 8); every string cleaned of controls and invisible characters and clipped; our own ISO 8601 reader.

//! Graph's JSON as MailALM's model: a message's summary, a folder's name, ids. Everything here
//! came from the server: strings are cleaned (no control, direction or zero-width characters)
//! and clipped, numbers checked.

use mailalm_model::{Flags, Person, Summary};
use serde_json::Value;

/// Longest id taken (Graph's are about 150 characters).
const MAX_ID: usize = 1024;

/// Whether `id` can be a Graph id: printable ASCII, not empty, of a sane length. It goes into
/// URL paths (percent-encoded) and into the cache.
pub(super) fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_graphic())
}

/// Whether a character can change how the text around it reads without showing itself: the
/// direction marks and overrides, and zero-width characters.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{061c}')
}

/// Server text as MailALM shows it: invisible characters dropped, controls (a line end, a
/// terminal escape) turned into spaces, trimmed, at most `max` characters.
pub(super) fn clean_text(s: &str, max: usize) -> String {
    let cleaned: String =
        s.chars().filter(|c| !is_invisible(*c)).map(|c| if c.is_control() { ' ' } else { c }).take(max.saturating_mul(2).saturating_add(64)).collect();
    cleaned.trim().chars().take(max).collect::<String>().trim_end().to_string()
}

/// A folder's display name as a part of a path: [`clean_text`], a `/` (the path's delimiter)
/// turned into `∕` (U+2215), and "(unnamed)" for a name that is left empty.
pub(super) fn folder_name(display: &str) -> String {
    let name = clean_text(display, 255).replace('/', "\u{2215}");
    if name.is_empty() { "(unnamed)".into() } else { name }
}

/// The first words of a message: [`clean_text`], with runs of spaces made one, at most 200
/// characters.
fn preview(s: &str) -> String {
    let cleaned = clean_text(s, 400);
    let mut out = String::with_capacity(cleaned.len());
    for word in cleaned.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out.chars().take(200).collect::<String>().trim_end().to_string()
}

/// `{"emailAddress": {"name", "address"}}` as a person; `None` without either.
fn person(v: Option<&Value>) -> Option<Person> {
    let e = v?.get("emailAddress")?;
    let text = |k: &str, max: usize| e.get(k).and_then(Value::as_str).map(|s| clean_text(s, max)).unwrap_or_default();
    let p = Person { name: text("name", 300), address: text("address", 320) };
    (!p.name.is_empty() || !p.address.is_empty()).then_some(p)
}

/// A list of recipients, at most 500 of them.
fn people(v: Option<&Value>) -> Vec<Person> {
    v.and_then(Value::as_array).map(|a| a.iter().take(500).filter_map(|r| person(Some(r))).collect()).unwrap_or_default()
}

/// A boolean as Graph sends it (its own examples have `"isRead": "true"` too).
fn truth(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A message's marks: read, flagged, draft. Graph has no answered mark here.
fn flags(m: &Value) -> Flags {
    let status = m.get("flag").and_then(|f| f.get("flagStatus")).and_then(Value::as_str).unwrap_or_default();
    Flags { seen: truth(m.get("isRead")), flagged: status.eq_ignore_ascii_case("flagged"), answered: false, draft: truth(m.get("isDraft")) }
}

/// An ISO 8601 date and time as Graph gives it (`2026-11-23T14:05:00Z`, with fractions of a
/// second or not, `Z`, an offset, or nothing for UTC) in Unix seconds.
pub(super) fn utc_seconds(t: &str) -> Option<i64> {
    let t = t.trim();
    let b = t.as_bytes();
    let num = |from: usize, to: usize| -> Option<i64> { t.get(from..to).filter(|s| s.bytes().all(|c| c.is_ascii_digit()))?.parse().ok() };
    let sep = |at: usize, ok: &[u8]| b.get(at).is_some_and(|c| ok.contains(c));
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1900..=9999).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut rest = t.get(19..)?;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = frac.get(digits..)?;
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return None,
            };
            let hm = rest.get(1..)?.replace(':', "");
            if hm.len() != 4 || !hm.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let (oh, om): (i64, i64) = (hm.get(..2)?.parse().ok()?, hm.get(2..)?.parse().ok()?);
            if oh > 23 || om > 59 {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
    };
    Some(mailalm_model::query::days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset)
}

/// A string field of `m`, empty when it has none.
fn str_of<'a>(m: &'a Value, k: &str) -> &'a str {
    m.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// A message's summary from its delta entry. `uid` is 0 (the engine numbers messages) and `size`
/// 0 (Graph lists none).
pub(super) fn summary_of(m: &Value) -> Summary {
    let date = |k: &str| m.get(k).and_then(Value::as_str).and_then(utc_seconds);
    let id = str_of(m, "internetMessageId").trim();
    Summary {
        uid: 0,
        from: person(m.get("from")).or_else(|| person(m.get("sender"))).unwrap_or_default(),
        to: people(m.get("toRecipients")),
        cc: people(m.get("ccRecipients")),
        subject: clean_text(str_of(m, "subject"), 1000),
        date: date("receivedDateTime").or_else(|| date("sentDateTime")).unwrap_or(0),
        flags: flags(m),
        size: 0,
        has_attachments: truth(m.get("hasAttachments")),
        preview: preview(str_of(m, "bodyPreview")),
        // Kept without its angle brackets, as the MIME reader keeps it.
        message_id: clean_text(id.trim_start_matches('<').trim_end_matches('>'), 1000),
        // Graph's listings have no In-Reply-To or References: the whole message gives them
        // (`complete_summary`); the conversation is the thread meanwhile.
        in_reply_to: String::new(),
        references: Vec::new(),
        thread: Some(str_of(m, "conversationId")).filter(|c| id_ok(c)).map(str::to_owned).unwrap_or_default(),
        categories: m
            .get("categories")
            .and_then(Value::as_array)
            .map(|a| a.iter().take(50).filter_map(Value::as_str).map(|c| clean_text(c, 255)).filter(|c| !c.is_empty()).collect())
            .unwrap_or_default(),
    }
}

/// Every error code in Graph's error (its `code`, and each `innerError`'s, a few levels deep).
pub(super) fn error_codes(body: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = body.and_then(|v| v.get("error"));
    for _ in 0..5 {
        let Some(e) = at else { break };
        if let Some(code) = e.get("code").and_then(Value::as_str) {
            out.push(clip_code(code));
        }
        at = e.get("innerError").or_else(|| e.get("innererror"));
    }
    out
}

fn clip_code(code: &str) -> String {
    code.chars().take(100).collect()
}
