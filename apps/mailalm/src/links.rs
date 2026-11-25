//! What a link in a received message really does (PLAN.md section 10.2), worked out before it is
//! opened: the real domain (a user name in front of it, an IP address, international letters
//! shown in punycode), look-alike names (alphabets mixed, Cyrillic or Greek letters that look like
//! Latin ones), and a link whose text names another domain than the one it goes to. Only `http`,
//! `https` and `mailto` links get here (the editor's model keeps no other). Pure, tested.

use crate::i18n::{tr, trf};

/// What opening a link does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opens {
    /// A web page, in the browser.
    Web(String),
    /// A new message (a `mailto:` link).
    Mail { to: String, subject: String, body: String },
}

/// A link, checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkCheck {
    /// What opening it does; `None` when it cannot be opened at all.
    pub opens: Option<Opens>,
    /// The real domain, as MailALM shows it: international letters in punycode (`xn--…`).
    pub domain: String,
    /// The same domain in its international letters, when it has some.
    pub unicode: Option<String>,
    /// What the user should know before opening it.
    pub warnings: Vec<String>,
}


/// The longest address looked at.
const MAX_URL: usize = 4096;

/// Checks the link to `target`, shown as `text` in the message.
pub fn check(target: &str, text: &str) -> LinkCheck {
    let target = target.trim();
    if target.len() > MAX_URL {
        return LinkCheck { warnings: vec![tr("The link's address is too long to be checked: it is not opened.").to_owned()], ..LinkCheck::default() };
    }
    let lower = target.get(..8).unwrap_or(target).to_ascii_lowercase();
    if lower.starts_with("mailto:") {
        return check_mailto(target, text);
    }
    let Some((https, rest)) = strip_ci(target, "https://").map(|r| (true, r)).or_else(|| strip_ci(target, "http://").map(|r| (false, r))) else {
        return LinkCheck { warnings: vec![tr("Only http, https and mailto links are opened.").to_owned()], ..LinkCheck::default() };
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let mut warnings = Vec::new();
    let host_port = match authority.rsplit_once('@') {
        Some((user, host)) => {
            warnings.push(trf!("The address starts with \"{user}@\", which is not where it goes: the real site is the name after the @.", user = clip(user, 60)));
            host
        }
        None => authority,
    };
    let host = host_of(host_port);
    let Some(site) = examine_host(&host, &mut warnings) else {
        return LinkCheck { warnings: vec![tr("The link has no site name: it is not opened.").to_owned()], ..LinkCheck::default() };
    };
    if !https {
        warnings.push(tr("The link is plain http: the page and what you type there are not protected.").to_owned());
    }
    if let Some(shown) = text_domain(text) {
        let shown_ascii = ascii_host(&shown).unwrap_or_else(|| shown.clone());
        if registrable(&shown_ascii) != registrable(&site.ascii) {
            warnings.push(trf!("The link's text shows {shown}, but it goes to {site}.", shown = shown, site = site.ascii));
        }
    } else if let Some(address) = text_address(text) {
        warnings.push(trf!("The link's text shows the address {address}, but it opens the web site {site}.", address = address, site = site.ascii));
    }
    LinkCheck { opens: Some(Opens::Web(target.to_owned())), domain: site.ascii, unicode: site.unicode, warnings }
}

/// A `mailto:` link: the address it writes to, and whether its text shows another.
fn check_mailto(target: &str, text: &str) -> LinkCheck {
    let Some((to, subject, body)) = parse_mailto(target) else {
        return LinkCheck { warnings: vec![tr("This mail link has no valid address: it is not opened.").to_owned()], ..LinkCheck::default() };
    };
    let mut warnings = Vec::new();
    let domain_raw = to.rsplit_once('@').map(|(_, d)| d.to_owned()).unwrap_or_default();
    let site = examine_host(&domain_raw.to_lowercase(), &mut warnings);
    let domain = site.as_ref().map_or_else(|| domain_raw.to_ascii_lowercase(), |s| s.ascii.clone());
    if let Some(address) = text_address(text) {
        if !address.eq_ignore_ascii_case(&to) {
            warnings.push(trf!("The link's text shows {address}, but the message would go to {to}.", address = address, to = to));
        }
    } else if let Some(shown) = text_domain(text) {
        let shown_ascii = ascii_host(&shown).unwrap_or_else(|| shown.clone());
        if registrable(&shown_ascii) != registrable(&domain) {
            warnings.push(trf!("The link's text shows {shown}, but the message would go to {to}.", shown = shown, to = to));
        }
    }
    LinkCheck { opens: Some(Opens::Mail { to, subject, body }), domain, unicode: site.and_then(|s| s.unicode), warnings }
}

/// A `mailto:` address: the first recipient, and the subject and body it asks for (decoded).
pub fn parse_mailto(url: &str) -> Option<(String, String, String)> {
    let rest = strip_ci(url.trim(), "mailto:")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut to = percent_decode(path).split(',').next().unwrap_or("").trim().to_owned();
    let (mut subject, mut body) = (String::new(), String::new());
    for pair in query.split('&').take(32) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(&v.replace('+', "%2B"));
        match k.to_ascii_lowercase().as_str() {
            "subject" => subject = v.chars().filter(|c| !c.is_control()).take(500).collect(),
            "body" => body = v.chars().filter(|c| !c.is_control() || *c == '\n').take(20_000).collect(),
            "to" if to.is_empty() => to = v.split(',').next().unwrap_or("").trim().to_owned(),
            _ => {}
        }
    }
    mailalm_mime::address_is_valid(&to).then_some((to, subject, body))
}

/// `%XX` sequences decoded (bytes, then UTF-8 with replacement).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%'
            && let (Some(h), Some(l)) = (bytes.get(i + 1).and_then(|c| (*c as char).to_digit(16)), bytes.get(i + 2).and_then(|c| (*c as char).to_digit(16)))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
            continue;
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|p| p.eq_ignore_ascii_case(prefix)).and_then(|_| s.get(prefix.len()..))
}

fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).filter(|c| !c.is_control()).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// The host of `host[:port]` (or `[v6]:port`), percent-decoded, lower case, with the dots
/// browsers read as dots made plain, and no final dot.
fn host_of(host_port: &str) -> String {
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or("").to_owned()
    } else {
        match host_port.rsplit_once(':') {
            Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h.to_owned(),
            _ => host_port.to_owned(),
        }
    };
    percent_decode(&host).chars().map(|c| if matches!(c, '\u{3002}' | '\u{ff0e}' | '\u{ff61}') { '.' } else { c }).collect::<String>().to_lowercase().trim_end_matches('.').to_owned()
}

/// A site's name as MailALM shows it.
struct Site {
    ascii: String,
    unicode: Option<String>,
}

/// Looks at a host: an IP address, invisible or unusual characters, international letters,
/// look-alike letters. `None` when there is no host.
fn examine_host(host: &str, warnings: &mut Vec<String>) -> Option<Site> {
    if host.is_empty() {
        return None;
    }
    if host.parse::<std::net::IpAddr>().is_ok() || (host.contains(':') && !host.contains('.')) {
        warnings.push(trf!("The link goes to an IP address ({host}), not to a named site.", host = host));
        return Some(Site { ascii: host.to_owned(), unicode: None });
    }
    if host.chars().any(|c| c.is_control() || c.is_whitespace() || is_invisible(c)) {
        warnings.push(tr("The site's name holds invisible characters.").to_owned());
    }
    if host.chars().any(|c| ('\u{ff00}'..='\u{ffef}').contains(&c)) {
        warnings.push(tr("The site's name is written in wide (full-width) characters.").to_owned());
    }
    let unicode = unicode_host(host);
    let ascii = ascii_host(&unicode).unwrap_or_else(|| host.to_owned());
    let international = !unicode.is_ascii();
    if international {
        warnings.push(trf!("The site's name uses international letters ({unicode}): MailALM shows it as {ascii}.", unicode = unicode, ascii = ascii));
    }
    for label in unicode.split('.') {
        match look_alike(label) {
            LookAlike::None => {}
            LookAlike::Mixed(scripts) => {
                warnings.push(trf!("The site's name mixes alphabets ({scripts}) in \"{label}\": it may imitate another site's name.", scripts = scripts, label = label));
            }
            LookAlike::Imitates(latin) => {
                warnings.push(trf!("\"{label}\" is written with letters that look like Latin ones: it looks like \"{latin}\", but it is another name.", label = label, latin = latin));
            }
        }
    }
    Some(Site { ascii, unicode: international.then_some(unicode) })
}

/// Characters that change how text reads without showing.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}' | '\u{00ad}' | '\u{061c}')
}

/// The alphabet a letter belongs to, for the look-alike check.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    Latin,
    Cyrillic,
    Greek,
    Other,
}

fn script_of(c: char) -> Option<Script> {
    if !c.is_alphabetic() {
        return None;
    }
    Some(match c {
        'a'..='z' | 'A'..='Z' | '\u{00c0}'..='\u{024f}' | '\u{1e00}'..='\u{1eff}' => Script::Latin,
        '\u{0400}'..='\u{052f}' | '\u{1c80}'..='\u{1c8f}' | '\u{2de0}'..='\u{2dff}' | '\u{a640}'..='\u{a69f}' => Script::Cyrillic,
        '\u{0370}'..='\u{03ff}' | '\u{1f00}'..='\u{1fff}' => Script::Greek,
        _ => Script::Other,
    })
}

fn script_name(s: Script) -> &'static str {
    match s {
        Script::Latin => tr("Latin"),
        Script::Cyrillic => tr("Cyrillic"),
        Script::Greek => tr("Greek"),
        Script::Other => tr("other"),
    }
}

/// The Latin letter a Cyrillic or Greek one looks like (lower case), if it looks like one.
fn latin_twin(c: char) -> Option<char> {
    Some(match c {
        // Cyrillic.
        'а' => 'a',
        'в' => 'b',
        'е' | 'ё' | 'ҽ' => 'e',
        'һ' => 'h',
        'і' | 'ї' => 'i',
        'ј' => 'j',
        'к' => 'k',
        'ӏ' => 'l',
        'м' => 'm',
        'н' => 'h',
        'о' => 'o',
        'р' => 'p',
        'ԛ' => 'q',
        'с' | 'ҫ' => 'c',
        'ѕ' => 's',
        'т' => 't',
        'у' | 'ү' => 'y',
        'ԝ' => 'w',
        'х' => 'x',
        'ԁ' => 'd',
        'ɡ' => 'g',
        // Greek.
        'α' => 'a',
        'β' => 'b',
        'ε' => 'e',
        'η' => 'n',
        'ι' => 'i',
        'κ' => 'k',
        'ν' => 'v',
        'ο' => 'o',
        'ρ' => 'p',
        'τ' => 't',
        'υ' => 'u',
        'χ' => 'x',
        'γ' => 'y',
        'ω' => 'w',
        _ => return None,
    })
}

/// What a label of a site's name may be imitating.
#[derive(Debug, PartialEq, Eq)]
enum LookAlike {
    None,
    /// Letters of several alphabets among Latin, Cyrillic and Greek (their names).
    Mixed(String),
    /// Every letter Cyrillic or Greek and each one looking like a Latin letter: what it reads as.
    Imitates(String),
}

fn look_alike(label: &str) -> LookAlike {
    let label = label.to_lowercase();
    let mut scripts: Vec<Script> = Vec::new();
    for s in label.chars().filter_map(script_of) {
        if s != Script::Other && !scripts.contains(&s) {
            scripts.push(s);
        }
    }
    if scripts.len() > 1 {
        let names: Vec<&str> = scripts.iter().map(|s| script_name(*s)).collect();
        return LookAlike::Mixed(names.join(tr(" and ")));
    }
    if scripts.first().is_some_and(|s| matches!(s, Script::Cyrillic | Script::Greek)) {
        let twins: Option<String> = label.chars().map(|c| if script_of(c).is_some() { latin_twin(c) } else { Some(c) }).collect();
        if let Some(latin) = twins {
            return LookAlike::Imitates(latin);
        }
    }
    LookAlike::None
}

/// A host written with its xn-- labels decoded (lower case).
fn unicode_host(host: &str) -> String {
    host.split('.')
        .map(|label| match strip_ci(label, "xn--").and_then(punycode_decode) {
            Some(decoded) => decoded.to_lowercase(),
            None => label.to_lowercase(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// A host with its labels outside ASCII in punycode (`xn--…`); `None` if one cannot be.
pub fn ascii_host(host: &str) -> Option<String> {
    let labels: Option<Vec<String>> = host
        .split('.')
        .map(|label| if label.is_ascii() { Some(label.to_ascii_lowercase()) } else { punycode_encode(&label.to_lowercase()).map(|p| format!("xn--{p}")) })
        .collect();
    labels.map(|l| l.join("."))
}

// ---- Punycode (RFC 3492) ----

const BASE: u32 = 36;
const T_MIN: u32 = 1;
const T_MAX: u32 = 26;
const SKEW: u32 = 38;
const DAMP: u32 = 700;
const INITIAL_BIAS: u32 = 72;
const INITIAL_N: u32 = 128;
/// The longest label encoded or decoded.
const MAX_LABEL: usize = 256;

fn adapt(delta: u32, points: u32, first: bool) -> u32 {
    let mut delta = if first { delta / DAMP } else { delta / 2 };
    delta += delta / points.max(1);
    let mut k = 0;
    while delta > ((BASE - T_MIN) * T_MAX) / 2 {
        delta /= BASE - T_MIN;
        k += BASE;
    }
    k + (((BASE - T_MIN + 1) * delta) / (delta + SKEW))
}

fn threshold(k: u32, bias: u32) -> u32 {
    if k <= bias {
        T_MIN
    } else if k >= bias + T_MAX {
        T_MAX
    } else {
        k - bias
    }
}

fn encode_digit(d: u32) -> Option<char> {
    match d {
        0..=25 => char::from_u32(u32::from(b'a') + d),
        26..=35 => char::from_u32(u32::from(b'0') + d - 26),
        _ => None,
    }
}

fn decode_digit(c: char) -> Option<u32> {
    match c {
        'a'..='z' => Some(c as u32 - 'a' as u32),
        'A'..='Z' => Some(c as u32 - 'A' as u32),
        '0'..='9' => Some(c as u32 - '0' as u32 + 26),
        _ => None,
    }
}

/// A label in punycode (without `xn--`).
pub fn punycode_encode(input: &str) -> Option<String> {
    let chars: Vec<u32> = input.chars().map(u32::from).collect();
    if chars.len() > MAX_LABEL {
        return None;
    }
    let mut out: String = input.chars().filter(char::is_ascii).collect();
    let basic = u32::try_from(out.len()).ok()?;
    let mut handled = basic;
    if basic > 0 {
        out.push('-');
    }
    let total = u32::try_from(chars.len()).ok()?;
    let (mut n, mut delta, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    while handled < total {
        let m = chars.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &c in &chars {
            if c < n {
                delta = delta.checked_add(1)?;
            }
            if c == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = threshold(k, bias);
                    if q < t {
                        break;
                    }
                    out.push(encode_digit(t + (q - t) % (BASE - t))?);
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                out.push(encode_digit(q)?);
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

/// A punycode label (without `xn--`) decoded; `None` if it is not valid punycode.
pub fn punycode_decode(input: &str) -> Option<String> {
    if input.len() > MAX_LABEL || !input.is_ascii() {
        return None;
    }
    let (basic, rest) = match input.rfind('-') {
        Some(i) => (input.get(..i)?, input.get(i + 1..)?),
        None => ("", input),
    };
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (INITIAL_N, 0u32, INITIAL_BIAS);
    let mut digits = rest.chars();
    let mut more = !rest.is_empty();
    while more {
        let old = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let d = decode_digit(digits.next()?)?;
            i = i.checked_add(d.checked_mul(w)?)?;
            let t = threshold(k, bias);
            if d < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = u32::try_from(out.len()).ok()? + 1;
        bias = adapt(i - old, len, old == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        let c = char::from_u32(n)?;
        if c.is_ascii() || out.len() >= MAX_LABEL {
            return None;
        }
        out.insert(usize::try_from(i).ok()?, c);
        i += 1;
        more = digits.as_str().chars().next().is_some();
    }
    Some(out.into_iter().collect())
}

// ---- What a link's text names ----

/// The domain a link's text shows, if it reads as a web address or a domain (`www.bank.com`,
/// `https://bank.com/login`, `bank.com`): lower case, without `www.`.
fn text_domain(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches(['.', ',', ';', ':', ')', ']', '>', '!', '?']).trim_start_matches(['(', '[', '<']);
    if t.is_empty() || t.chars().any(char::is_whitespace) || t.contains('@') {
        return None;
    }
    let rest = strip_ci(t, "https://").or_else(|| strip_ci(t, "http://")).unwrap_or(t);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_of(authority);
    let labels: Vec<&str> = host.split('.').collect();
    let tld = labels.last().copied().unwrap_or("");
    let looks_like = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && ((tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic)) || tld.starts_with("xn--"));
    if !looks_like {
        return None;
    }
    Some(host.strip_prefix("www.").unwrap_or(&host).to_owned())
}

/// The mail address a link's text shows, if it reads as one.
fn text_address(text: &str) -> Option<String> {
    let t = text.trim().trim_start_matches("mailto:").trim_matches(['<', '>', '(', ')', '.', ',', ';']);
    (mailalm_mime::address_is_valid(t) && !t.chars().any(char::is_whitespace)).then(|| t.to_owned())
}

/// Second-level names under which anyone can register a name (`bank.co.uk`): the site is then
/// three labels long.
const SHARED_SECOND_LEVEL: [&str; 10] = ["co", "com", "net", "org", "gov", "edu", "ac", "gc", "qc", "on"];

/// The part of a host its owner registered (approximately: the last two labels, three under a
/// shared second level of a country's domain), for comparing a link's text and target.
fn registrable(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let take = match (labels.get(n.wrapping_sub(2)), labels.last()) {
        (Some(second), Some(tld)) if n >= 3 && tld.len() == 2 && SHARED_SECOND_LEVEL.contains(second) => 3,
        _ => 2,
    };
    labels[n.saturating_sub(take)..].join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punycode_both_ways() {
        for (unicode, puny) in [("bücher", "bcher-kva"), ("münchen", "mnchen-3ya"), ("аpple", "pple-43d"), ("пример", "e1afmkfd"), ("日本語", "wgv71a119e")] {
            assert_eq!(punycode_encode(unicode).as_deref(), Some(puny), "{unicode}");
            assert_eq!(punycode_decode(puny).as_deref(), Some(unicode), "{puny}");
        }
        assert_eq!(ascii_host("www.аpple.com").as_deref(), Some("www.xn--pple-43d.com"));
        assert_eq!(unicode_host("XN--PPLE-43D.com"), "аpple.com");
        // Not punycode: refused, never a panic.
        for bad in ["", "-", "abc-!!", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", "a-9999999999", "é"] {
            let _ = punycode_decode(bad);
        }
        assert_eq!(punycode_decode("ab-").as_deref(), Some("ab"), "only basic letters");
        assert_eq!(punycode_decode("é"), None);
    }

    #[test]
    fn look_alike_names() {
        assert_eq!(look_alike("apple"), LookAlike::None);
        assert_eq!(look_alike("bücher"), LookAlike::None);
        assert_eq!(look_alike("аpple"), LookAlike::Mixed("Cyrillic and Latin".into()));
        assert_eq!(look_alike("раураl"), LookAlike::Mixed("Cyrillic and Latin".into()));
        assert_eq!(look_alike("рауа"), LookAlike::Imitates("paya".into()));
        assert_eq!(look_alike("οκ"), LookAlike::Imitates("ok".into()));
        assert_eq!(look_alike("пример"), LookAlike::None, "Cyrillic that reads as Cyrillic");
        assert_eq!(look_alike("αpple"), LookAlike::Mixed("Greek and Latin".into()));
        assert_eq!(look_alike("日本"), LookAlike::None);
    }

    #[test]
    fn a_plain_link() {
        let c = check("https://www.bank.example/login?x=1", "Sign in");
        assert_eq!(c.opens, Some(Opens::Web("https://www.bank.example/login?x=1".into())));
        assert_eq!((c.domain.as_str(), c.unicode.as_deref()), ("www.bank.example", None));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        // The text names the same site.
        assert!(check("https://login.bank.example/", "www.bank.example").warnings.is_empty());
        assert!(check("https://bank.co.uk/a", "https://www.bank.co.uk").warnings.is_empty());
    }

    #[test]
    fn a_link_whose_text_names_another_site() {
        let c = check("https://evil.example/bank", "https://www.bank.example/login");
        assert_eq!(c.warnings.len(), 1);
        assert!(c.warnings[0].contains("bank.example") && c.warnings[0].contains("evil.example"), "{:?}", c.warnings);
        assert!(!check("https://evil.co.uk/", "bank.co.uk").warnings.is_empty());
        assert!(check("https://x.example/", "Click here.").warnings.is_empty(), "words are no domain");
        assert!(check("https://x.example/", "write to help@bank.example").warnings.is_empty());
        assert!(!check("https://x.example/", "help@bank.example").warnings.is_empty(), "an address shown, a site opened");
    }

    #[test]
    fn hidden_hosts_are_said() {
        let c = check("https://www.bank.example@evil.example:8443/x", "bank");
        assert_eq!(c.domain, "evil.example");
        assert!(c.warnings.iter().any(|w| w.contains("www.bank.example@")), "{:?}", c.warnings);
        let c = check("http://192.168.0.1/admin", "router");
        assert!(c.warnings.iter().any(|w| w.contains("IP address")) && c.warnings.iter().any(|w| w.contains("plain http")), "{:?}", c.warnings);
        let c = check("https://www.аpple.com/", "www.apple.com");
        assert_eq!((c.domain.as_str(), c.unicode.as_deref()), ("www.xn--pple-43d.com", Some("www.аpple.com")));
        assert!(c.warnings.iter().any(|w| w.contains("international letters")));
        assert!(c.warnings.iter().any(|w| w.contains("mixes alphabets")));
        assert!(c.warnings.iter().any(|w| w.contains("text shows")), "the text's apple.com is another site: {:?}", c.warnings);
        let c = check("https://xn--80ak6aa92e.com/", "");
        assert!(c.warnings.iter().any(|w| w.contains("look like Latin")), "{:?}", c.warnings);
        assert!(check("https://bank%2eexample/", "").domain == "bank.example");
        assert!(check("https://bank\u{3002}example/", "").domain == "bank.example");
        assert_eq!(check("javascript:alert(1)", "x").opens, None);
        assert_eq!(check("https:///path", "x").opens, None);
    }

    #[test]
    fn mail_links() {
        let c = check("mailto:help@bank.example?subject=Hi%20there&body=Line%201%0ALine%202", "help@bank.example");
        assert_eq!(c.opens, Some(Opens::Mail { to: "help@bank.example".into(), subject: "Hi there".into(), body: "Line 1\nLine 2".into() }));
        assert_eq!(c.domain, "bank.example");
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert!(!check("mailto:thief@evil.example", "help@bank.example").warnings.is_empty());
        assert!(!check("mailto:thief@evil.example", "www.bank.example").warnings.is_empty());
        assert_eq!(check("mailto:?subject=x", "x").opens, None);
        assert_eq!(parse_mailto("MAILTO:a@b.example,c@d.example").map(|m| m.0), Some("a@b.example".into()));
        assert_eq!(parse_mailto("mailto:?to=a@b.example").map(|m| m.0), Some("a@b.example".into()));
    }

    #[test]
    fn registrable_parts() {
        assert_eq!(registrable("a.b.bank.example"), "bank.example");
        assert_eq!(registrable("www.bank.co.uk"), "bank.co.uk");
        assert_eq!(registrable("videotron.qc.ca"), "videotron.qc.ca");
        assert_eq!(registrable("example"), "example");
    }
}
