//! How to leave a mailing list, from a message's `List-Unsubscribe` (RFC 2369) and
//! `List-Unsubscribe-Post` (RFC 8058) header fields (PLAN.md, section 10.2, S4).
//!
//! Both come from the sender, so from anyone: they are read as hostile text. Only what MailALM can
//! do safely is kept, and nothing is ever done here:
//!
//! - **one click**: an `https://` address, when `List-Unsubscribe-Post` says exactly
//!   `List-Unsubscribe=One-Click`. MailALM POSTs that body to it, only after the user confirmed
//!   (`mailalm_net::one_click_unsubscribe`);
//! - **by mail**: a `mailto:` address, with the subject and text it names (capped, one address,
//!   no other field);
//! - **a web page**: an `https://` (or `http://`) address, which the interface opens in the
//!   browser after confirmation, like any link.
//!
//! Nothing proves the fields are the list's (RFC 8058 wants them covered by a DKIM signature,
//! which MailALM doesn't check): the confirmation shows the domain that will be contacted.

use mail_parser::Message;

/// The most of each field read, in bytes.
const MAX_FIELD: usize = 4096;
/// The most addresses looked at in `List-Unsubscribe`.
const MAX_ITEMS: usize = 10;
/// The longest web address kept.
const MAX_URL: usize = 2048;
/// The longest subject and text of an unsubscribe message.
const MAX_SUBJECT: usize = 200;
const MAX_BODY: usize = 1000;

/// How to leave the list a message came from. At least one of the three is there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unsubscribe {
    /// RFC 8058: the `https://` address to POST `List-Unsubscribe=One-Click` to.
    pub one_click: Option<String>,
    /// A message to send from the account.
    pub mailto: Option<Mailto>,
    /// A page to open in the browser (https, else http).
    pub web: Option<String>,
}

/// The unsubscribe message a `mailto:` address asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mailto {
    pub to: String,
    /// "unsubscribe" when the address names none.
    pub subject: String,
    pub body: String,
}

impl Unsubscribe {
    /// The host the one-click POST, or the web page, goes to (shown in the confirmation).
    pub fn host_of(url: &str) -> &str {
        let rest = url.split_once("://").map_or("", |(_, r)| r);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        authority.rsplit_once(':').map_or(authority, |(h, port)| if port.bytes().all(|b| b.is_ascii_digit()) { h } else { authority })
    }
}

/// The message's fields, read ([`parse_unsubscribe`]).
pub(crate) fn unsubscribe_of(m: &Message<'_>) -> Option<Unsubscribe> {
    let headers = m.headers();
    let first = |name: &str| headers.all(name).next().map(|h| field_text(h.raw_value()));
    let list = first("List-Unsubscribe")?;
    let post = first("List-Unsubscribe-Post");
    parse_unsubscribe(&list, post.as_deref())
}

/// A field's raw value as text: at most [`MAX_FIELD`] bytes, lines unfolded.
fn field_text(raw: &[u8]) -> String {
    let raw = raw.get(..raw.len().min(MAX_FIELD)).unwrap_or_default();
    String::from_utf8_lossy(raw).chars().map(|c| if c == '\r' || c == '\n' { ' ' } else { c }).collect()
}

/// Reads `List-Unsubscribe` (the `<…>` addresses, the first of each kind kept) and
/// `List-Unsubscribe-Post`. `None` when nothing usable is there.
pub fn parse_unsubscribe(list: &str, post: Option<&str>) -> Option<Unsubscribe> {
    let list: String = list.chars().take(MAX_FIELD).collect();
    let one_click_asked = post.is_some_and(|p| p.trim().eq_ignore_ascii_case("List-Unsubscribe=One-Click"));
    let mut out = Unsubscribe::default();
    let mut https: Option<String> = None;
    let mut http: Option<String> = None;
    for item in bracketed(&list).into_iter().take(MAX_ITEMS) {
        // RFC 2369: white space inside the brackets is to be ignored.
        let uri: String = item.chars().filter(|c| !c.is_whitespace()).collect();
        let lower = uri.to_ascii_lowercase();
        if lower.starts_with("https://") {
            if https.is_none() && web_ok(&uri, "https://") {
                https = Some(uri);
            }
        } else if lower.starts_with("http://") {
            if http.is_none() && web_ok(&uri, "http://") {
                http = Some(uri);
            }
        } else if lower.starts_with("mailto:") && out.mailto.is_none() {
            out.mailto = mailto(uri.get("mailto:".len()..).unwrap_or_default());
        }
    }
    if one_click_asked {
        out.one_click = https.clone();
    }
    out.web = https.or(http);
    (out.one_click.is_some() || out.mailto.is_some() || out.web.is_some()).then_some(out)
}

/// The texts between `<` and `>`, outside comments.
fn bracketed(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    let mut comment = 0usize;
    for c in s.chars() {
        if let Some(item) = cur.as_mut() {
            match c {
                '>' => {
                    out.push(std::mem::take(item));
                    cur = None;
                }
                // A bracket that never closed: start over from this one.
                '<' => item.clear(),
                c => item.push(c),
            }
            continue;
        }
        match c {
            '(' => comment += 1,
            ')' => comment = comment.saturating_sub(1),
            '<' if comment == 0 => cur = Some(String::new()),
            _ => {}
        }
    }
    out
}

/// Whether `url` is a web address MailALM may contact or show: of the scheme, printable ASCII,
/// a host of letters, digits, dots and dashes (a port allowed), no user name, of a sane length.
fn web_ok(url: &str, scheme: &str) -> bool {
    if url.len() > MAX_URL || !url.bytes().all(|b| b.is_ascii_graphic()) || url.contains('\\') {
        return false;
    }
    let rest = url.get(scheme.len()..).unwrap_or_default();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with(['.', '-'])
        && !host.ends_with('.')
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A `mailto:` address's message (RFC 6068): its first address, its `subject` and `body`,
/// percent-decoded and cleaned; every other field left out. `None` without a usable address.
fn mailto(rest: &str) -> Option<Mailto> {
    let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
    let to = decode(to.split(',').next().unwrap_or_default());
    let to = to.trim();
    if !crate::address_is_valid(to) {
        return None;
    }
    let mut subject = String::new();
    let mut body = String::new();
    for pair in query.split('&').take(20) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k.eq_ignore_ascii_case("subject") && subject.is_empty() {
            subject = crate::clean_line(&decode(v)).chars().take(MAX_SUBJECT).collect();
        } else if k.eq_ignore_ascii_case("body") && body.is_empty() {
            body = crate::clean_text(&decode(v)).chars().take(MAX_BODY).collect();
        }
    }
    if subject.trim().is_empty() {
        subject = "unsubscribe".to_owned();
    }
    Some(Mailto { to: to.to_owned(), subject, body })
}

/// Percent-decoding (`%XX` as a byte; anything else as it is), read as UTF-8.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = |at: usize| b.get(at).and_then(|h| char::from(*h).to_digit(16));
        match (c, hex(i + 1), hex(i + 2)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(u8::try_from(hi * 16 + lo).unwrap_or(b'?'));
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_click_mail_and_web() {
        let u = parse_unsubscribe(
            "<mailto:leave@lists.example?subject=Unsubscribe%20me&body=Please%0D%0Aremove>, <https://lists.example/u?t=abc>",
            Some(" List-Unsubscribe=One-Click "),
        )
        .unwrap();
        assert_eq!(u.one_click.as_deref(), Some("https://lists.example/u?t=abc"));
        assert_eq!(u.web.as_deref(), Some("https://lists.example/u?t=abc"));
        assert_eq!(u.mailto, Some(Mailto { to: "leave@lists.example".into(), subject: "Unsubscribe me".into(), body: "Please\nremove".into() }));
        assert_eq!(Unsubscribe::host_of("https://lists.example:8443/u"), "lists.example");
    }

    #[test]
    fn one_click_needs_the_exact_post_field_and_https() {
        let list = "<https://lists.example/u>";
        assert!(parse_unsubscribe(list, None).unwrap().one_click.is_none());
        assert!(parse_unsubscribe(list, Some("List-Unsubscribe=Yes")).unwrap().one_click.is_none());
        let http = parse_unsubscribe("<http://lists.example/u>", Some("List-Unsubscribe=One-Click")).unwrap();
        assert!(http.one_click.is_none());
        assert_eq!(http.web.as_deref(), Some("http://lists.example/u"));
    }

    #[test]
    fn hostile_fields() {
        // Nothing usable.
        for bad in [
            "",
            "no brackets at all https://x.example/",
            "<javascript:alert(1)>",
            "<file:///C:/Windows/system32>",
            "<https://user@evil.example/>",
            "<https://evil.example\\@good.example/>",
            "<https://exa mple.com/>",
            "<https:///nohost>",
            "<https://-x.example/>",
            "<https://x.example:99999999/>",
            "(<https://in.a.comment/>)",
            "<mailto:not-an-address>",
            "<mailto:>",
        ] {
            let got = parse_unsubscribe(bad, Some("List-Unsubscribe=One-Click"));
            // White space inside brackets is ignored: "exa mple.com" reads as "example.com".
            if bad.contains("exa mple") {
                assert_eq!(got.and_then(|u| u.web).as_deref(), Some("https://example.com/"));
                continue;
            }
            assert_eq!(got, None, "{bad}");
        }
        // A mailto naming several addresses, other fields and controls: the first address, the
        // subject cleaned, nothing else.
        let u = parse_unsubscribe("<mailto:a@lists.example,b@else.example?cc=c@x.example&bcc=d@x.example&subject=%1B[31mStop%E2%80%AE>", None).unwrap();
        assert_eq!(u.mailto, Some(Mailto { to: "a@lists.example".into(), subject: "[31mStop".into(), body: String::new() }));
        // A long field and many items: capped, the first of each kind kept.
        let many = format!("{}<https://first.example/>", "<https://x.example/>".repeat(500));
        assert_eq!(parse_unsubscribe(&many, None).unwrap().web.as_deref(), Some("https://x.example/"));
        let long = format!("<https://x.example/{}>", "a".repeat(5000));
        assert_eq!(parse_unsubscribe(&long, None), None);
        // A bracket that never closed doesn't swallow the next item.
        let u = parse_unsubscribe("<broken, <https://ok.example/u>", None).unwrap();
        assert_eq!(u.web.as_deref(), Some("https://ok.example/u"));
        // A bad percent sequence stays as it is.
        assert_eq!(decode("100%zz%41"), "100%zzA");
    }

    #[test]
    fn read_from_a_message() {
        let raw = b"From: news@lists.example\r\nTo: alex@example.com\r\nSubject: News\r\nList-Unsubscribe: <https://lists.example/u/1>,\r\n <mailto:u@lists.example>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\r\nHello\r\n";
        let p = crate::parse(raw).unwrap();
        let u = p.unsubscribe.unwrap();
        assert_eq!(u.one_click.as_deref(), Some("https://lists.example/u/1"));
        assert_eq!(u.mailto.map(|m| m.to).as_deref(), Some("u@lists.example"));
        let plain = crate::parse(b"From: a@b.example\r\nSubject: x\r\n\r\nHi\r\n").unwrap();
        assert!(plain.unsubscribe.is_none());
    }
}
