//! The sender checks of a message (SPF, DKIM, DMARC), from its `Authentication-Results` header
//! (RFC 8601).
//!
//! Only the first such header is read: the one on top, added by the provider that received the
//! message. Headers further down were written before, possibly by the sender, and prove nothing.
//! A provider that adds none (some IMAP providers) leaves the sender's own, if any, on top: the
//! dialog says which server claims the results (`checked_by`) when the header names it.

/// The most of the header read.
const MAX_HEADER: usize = 16 << 10;
/// The deepest comment nesting followed.
const MAX_DEPTH: usize = 32;
/// The most results read from one header.
const MAX_RESULTS: usize = 64;

/// A check's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// `pass` (and Microsoft's `bestguesspass`).
    Pass,
    /// `fail`, `softfail`, `permerror`.
    Fail,
    /// Anything else: `neutral`, `temperror`, `policy`…
    Other,
    /// No result for this check (`none`, or not in the header).
    Missing,
}

/// One check: its outcome, and the word the header used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub verdict: Verdict,
    /// Lowercase, letters and digits only; `none` when missing.
    pub word: String,
}

impl Check {
    fn missing() -> Self {
        Check { verdict: Verdict::Missing, word: "none".to_owned() }
    }

    fn of(word: &str) -> Self {
        let word: String = word.chars().filter(char::is_ascii_alphanumeric).take(20).collect::<String>().to_ascii_lowercase();
        let verdict = match word.as_str() {
            "pass" | "bestguesspass" => Verdict::Pass,
            "fail" | "softfail" | "hardfail" | "permerror" => Verdict::Fail,
            "none" | "" => Verdict::Missing,
            _ => Verdict::Other,
        };
        if word.is_empty() { Check::missing() } else { Check { verdict, word } }
    }
}

/// The message's sender checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authentication {
    /// The message has an `Authentication-Results` header.
    pub found: bool,
    /// The server that wrote it (`mx.google.com`), when the header names it.
    pub checked_by: Option<String>,
    pub spf: Check,
    pub dkim: Check,
    pub dmarc: Check,
}

impl Default for Authentication {
    fn default() -> Self {
        Authentication { found: false, checked_by: None, spf: Check::missing(), dkim: Check::missing(), dmarc: Check::missing() }
    }
}

/// Reads the value of the message's first `Authentication-Results` header (`None`: it has none).
///
/// DKIM passes when any signature passes; SPF and DMARC take the first result.
pub fn authentication(header: Option<&str>) -> Authentication {
    let Some(header) = header.filter(|h| !h.trim().is_empty()) else { return Authentication::default() };
    let value: String = header.chars().take(MAX_HEADER).collect();
    let parts = split_results(&without_comments(&value));
    let mut a = Authentication { found: true, ..Authentication::default() };
    let mut results = parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).peekable();
    // The first part names the server, unless it already is a result (Outlook.com writes none).
    if let Some(first) = results.peek()
        && !first.contains('=')
    {
        a.checked_by = first
            .split_whitespace()
            .next()
            .map(|id| id.chars().filter(char::is_ascii_graphic).take(100).collect::<String>())
            .filter(|id| !id.is_empty());
        results.next();
    }
    for part in results.take(MAX_RESULTS) {
        let Some(token) = part.split_whitespace().next() else { continue };
        let Some((method, result)) = token.split_once('=') else { continue };
        let method = method.split('/').next().unwrap_or("").trim().to_ascii_lowercase();
        let check = Check::of(result.trim().trim_matches('"'));
        match method.as_str() {
            "spf" if a.spf.verdict == Verdict::Missing => a.spf = check,
            "dmarc" if a.dmarc.verdict == Verdict::Missing => a.dmarc = check,
            "dkim" if a.dkim.verdict == Verdict::Missing || (check.verdict == Verdict::Pass && a.dkim.verdict != Verdict::Pass) => a.dkim = check,
            _ => {}
        }
    }
    a
}

/// The header without its comments (`(…)`, nested, outside quoted strings), each replaced by a
/// space.
fn without_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            escaped = false;
            if depth == 0 {
                out.push(c);
            }
            continue;
        }
        match c {
            '\\' if quoted || depth > 0 => {
                escaped = true;
                if depth == 0 {
                    out.push(c);
                }
            }
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push(c);
            }
            '(' if !quoted => {
                if depth == 0 {
                    out.push(' ');
                }
                depth = (depth + 1).min(MAX_DEPTH);
            }
            ')' if !quoted && depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            _ => out.push(c),
        }
    }
    out
}

/// The parts between semicolons outside quoted strings.
fn split_results(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}
