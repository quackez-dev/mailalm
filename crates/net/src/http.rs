// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/http.rs. MIT OR Apache-2.0.
// Changed: https only (plain http to this computer in tests alone); no redirects followed, no event streams, no downloads into files, no "internet only" connections; one kept connection opened again once when the server had closed it, never for a request that may have been acted on; a percent-encoder of our own; interim (1xx) answers skipped.

//! A small blocking HTTP/1.1 client (RFC 9112) for the web services MailALM talks to: Microsoft's
//! and Google's sign-in services (OAuth), Microsoft Graph and the Gmail API.
//!
//! Every connection is TLS through the same strict setup as IMAP ([`stream::tls`]); plain
//! `http://` exists only in tests, to a server on this computer. `Content-Length` and `chunked`
//! bodies; no redirect is ever followed (a 3xx is an answer like any other). Answers are hostile
//! input: the head and the body are capped, every number is checked.

use std::io::{self, ErrorKind, Read, Write};

use crate::stream::{self, Stream};
use crate::{NetError, Result};

mod body;

/// Longest header block accepted.
const MAX_HEAD: usize = 64 << 10;
/// Most header fields kept from one answer.
const MAX_HEADERS: usize = 200;
/// Most interim (1xx) answers skipped before the real one.
const MAX_INTERIM: usize = 5;

/// A parsed `https://host[:port]/path?query` address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Url {
    pub(crate) https: bool,
    pub(crate) host: String,
    pub(crate) port: u16,
    /// Path and query, starting with `/`.
    pub(crate) path: String,
}

impl Url {
    pub(crate) fn parse(s: &str) -> Result<Url> {
        let s = s.trim();
        let bad = || NetError::Protocol(format!("not a web address: {}", s.chars().take(80).collect::<String>()));
        let (https, rest) = if let Some(r) = strip_prefix_ci(s, "https://") {
            (true, r)
        } else if let Some(r) = strip_prefix_ci(s, "http://") {
            (false, r)
        } else {
            return Err(bad());
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if authority.is_empty() || authority.contains('@') {
            return Err(bad());
        }
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or_else(bad)?;
            let port = match after.strip_prefix(':') {
                Some(p) => p.parse().map_err(|_| bad())?,
                None if after.is_empty() => default_port(https),
                None => return Err(bad()),
            };
            (h.to_string(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), p.parse().map_err(|_| bad())?),
                None => (authority.to_string(), default_port(https)),
            }
        };
        // A host name or an IP address: nothing that could change the request it goes into.
        if host.is_empty() || port == 0 || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) {
            return Err(bad());
        }
        let tail = tail.split('#').next().unwrap_or("");
        let path = if tail.is_empty() {
            "/".to_string()
        } else if tail.starts_with('?') {
            format!("/{tail}")
        } else {
            tail.to_string()
        };
        Ok(Url { https, host: host.to_ascii_lowercase(), port, path: spaces_escaped(path) })
    }
}

/// A path with its spaces and controls percent-encoded rather than sent raw (each of their UTF-8
/// bytes); the path itself, untouched, when it has none.
fn spaces_escaped(path: String) -> String {
    let raw = |c: char| c == ' ' || c.is_control();
    if !path.contains(raw) {
        return path;
    }
    let mut out = String::with_capacity(path.len().saturating_add(16));
    let mut utf8 = [0u8; 4];
    for c in path.chars() {
        if raw(c) {
            // Neither is unreserved: every byte comes out as `%XX`.
            out.push_str(&pct(c.encode_utf8(&mut utf8)));
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a [`Url`]'s host is this computer: the one place plain http is let through, to a
/// test server.
#[cfg(test)]
pub(crate) fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn default_port(https: bool) -> u16 {
    if https { 443 } else { 80 }
}

fn strip_prefix_ci<'a>(s: &'a str, p: &str) -> Option<&'a str> {
    s.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).and_then(|_| s.get(p.len()..))
}

/// An answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Response {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        header_in(&self.headers, name)
    }

    pub(crate) fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn header_in<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// A request. Its `Debug` leaves out the header values and the body: an `Authorization` header
/// or a form with a code or refresh token in it never goes into a log.
#[derive(Clone)]
pub(crate) struct Request {
    pub(crate) method: &'static str,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
    /// Largest body accepted in the answer.
    pub(crate) max: usize,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Request").field("method", &self.method).field("headers", &names).field("body_len", &self.body.len()).finish()
    }
}

impl Request {
    pub(crate) fn new(method: &'static str, url: &str, max: usize) -> Request {
        Request { method, url: url.into(), headers: Vec::new(), body: Vec::new(), max }
    }

    pub(crate) fn get(url: &str, max: usize) -> Request {
        Request::new("GET", url, max)
    }

    pub(crate) fn post(url: &str, content_type: &str, body: Vec<u8>, max: usize) -> Request {
        Request { body, ..Request::new("POST", url, max) }.header("Content-Type", content_type)
    }

    pub(crate) fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// Why a request failed, and how far it got: whether the whole request went out (the server
/// may have acted on it), and whether the connection turned out to be closed before any answer
/// came (a kept connection the server had let go while it was idle).
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) error: NetError,
    pub(crate) sent: bool,
    pub(crate) stale: bool,
}

impl Failure {
    fn unsent(error: NetError) -> Failure {
        Failure { error, sent: false, stale: false }
    }
}

impl From<Failure> for NetError {
    fn from(f: Failure) -> NetError {
        f.error
    }
}

/// A connection to `url`'s server, through TLS. Plain http only in tests, to this computer.
fn connect(url: &Url) -> Result<Stream> {
    if !url.https {
        return plain(url);
    }
    let sock = stream::tcp(&url.host, url.port)?;
    stream::tls(sock, &url.host)
}

#[cfg(not(test))]
fn plain(_url: &Url) -> Result<Stream> {
    Err(NetError::Tls("MailALM goes to the web over https only".into()))
}

#[cfg(test)]
fn plain(url: &Url) -> Result<Stream> {
    if !is_loopback(&url.host) {
        return Err(NetError::Tls("plain http, in tests, goes to this computer only".into()));
    }
    Ok(Stream::Plain(stream::tcp(&url.host, url.port)?))
}

/// Send a request on a connection of its own (`Connection: close`).
pub(crate) fn send(req: &Request) -> Result<Response> {
    let url = checked(req)?;
    let mut conn = Conn::open(&url)?;
    Ok(conn.send(req, &url, false)?)
}

/// A connection that can carry several requests to one server, one after the other.
pub(crate) struct Conn {
    s: Stream,
    url: Url,
    /// No other request can go on it: the server said it closes it, an answer had no end but the
    /// connection's, or an exchange failed part-way.
    closing: bool,
}

impl Conn {
    pub(crate) fn open(url: &Url) -> Result<Conn> {
        Ok(Conn { s: connect(url)?, url: url.clone(), closing: false })
    }

    /// Whether this connection reaches `u`'s server.
    pub(crate) fn reaches(&self, u: &Url) -> bool {
        u.https == self.url.https && u.host == self.url.host && u.port == self.url.port
    }

    /// Whether another request can go on it.
    pub(crate) fn usable(&self) -> bool {
        !self.closing
    }

    /// Send `req` (for `url`, which this connection reaches) and read its answer. With `keep`,
    /// the connection is asked to stay open for the next one.
    pub(crate) fn send(&mut self, req: &Request, url: &Url, keep: bool) -> std::result::Result<Response, Failure> {
        if self.closing || !self.reaches(url) {
            return Err(Failure::unsent(NetError::Protocol("internal: a request on the wrong connection".into())));
        }
        // The worst is assumed until a whole answer has been read: after an error part-way, where
        // the next answer would start is unknown.
        self.closing = true;
        let mut watch = Watch { s: &mut self.s, got: false, closed: false };
        if let Err(e) = write_request(&mut watch, req, url, keep) {
            return Err(Failure { error: e, sent: false, stale: watch.closed });
        }
        match read_answer(&mut watch, req.max, req.method == "HEAD") {
            Ok((resp, cut)) => {
                let length_known = resp.header("Content-Length").is_some() || resp.header("Transfer-Encoding").is_some();
                self.closing = !keep
                    || cut
                    || resp.header("Connection").is_some_and(|c| c.eq_ignore_ascii_case("close"))
                    || (!length_known && resp.status != 204 && resp.status != 304);
                Ok(resp)
            }
            Err(e) => Err(Failure { error: e, sent: true, stale: watch.closed && !watch.got }),
        }
    }
}

/// One kept connection, for a client that sends many requests to one server (Graph): the next
/// request goes on it while it can carry one, else on a new one.
#[derive(Default)]
pub(crate) struct Agent {
    conn: Option<Conn>,
}

impl Agent {
    /// Send `req`. When a kept connection turns out to have been closed by the server before any
    /// answer came, and `again` allows it, the request goes once more, on a new connection.
    /// `again` is false for a request that must never go twice (sending mail).
    pub(crate) fn send(&mut self, req: &Request, again: bool) -> std::result::Result<Response, Failure> {
        let url = checked(req).map_err(Failure::unsent)?;
        let kept = self.conn.take().filter(|c| c.usable() && c.reaches(&url));
        let reused = kept.is_some();
        let mut conn = match kept {
            Some(c) => c,
            None => Conn::open(&url).map_err(Failure::unsent)?,
        };
        let answer = match conn.send(req, &url, true) {
            Err(f) if reused && again && f.stale => {
                log::debug!("{}: the kept connection had been closed; sending again on a new one", url.host);
                conn = Conn::open(&url).map_err(Failure::unsent)?;
                conn.send(req, &url, true)
            }
            other => other,
        };
        if answer.is_ok() && conn.usable() {
            self.conn = Some(conn);
        }
        answer
    }

    /// Let the kept connection go: the next request opens a new one.
    pub(crate) fn close(&mut self) {
        self.conn = None;
    }
}

/// A connection watched for whether any of an answer came, and whether it was found closed
/// (end of stream, reset) before that.
struct Watch<'a, S> {
    s: &'a mut S,
    got: bool,
    closed: bool,
}

fn is_closed(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof)
}

impl<S: Read> Read for Watch<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.s.read(buf) {
            Ok(0) => {
                self.closed = true;
                Ok(0)
            }
            Ok(n) => {
                self.got = true;
                Ok(n)
            }
            Err(e) => {
                self.closed |= is_closed(&e);
                Err(e)
            }
        }
    }
}

impl<S: Write> Write for Watch<'_, S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let r = self.s.write(buf);
        if let Err(e) = &r {
            self.closed |= is_closed(e);
        }
        r
    }
    fn flush(&mut self) -> io::Result<()> {
        let r = self.s.flush();
        if let Err(e) = &r {
            self.closed |= is_closed(e);
        }
        r
    }
}

/// A header field name as RFC 9110 has them (a token), and a value without line ends.
fn header_ok(name: &str, value: &str) -> bool {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    !name.is_empty() && name.chars().all(token) && !value.contains(['\r', '\n', '\0'])
}

/// The request's address, once its headers are known to be headers: checked before any
/// connection is opened.
fn checked(req: &Request) -> Result<Url> {
    if !req.headers.iter().all(|(k, v)| header_ok(k, v)) {
        return Err(NetError::Invalid("a request header that isn't one".into()));
    }
    Url::parse(&req.url)
}

fn write_request<S: Write>(s: &mut S, req: &Request, url: &Url, keep: bool) -> Result<()> {
    let host = if url.host.contains(':') { format!("[{}]", url.host) } else { url.host.clone() };
    let host = if url.port == default_port(url.https) { host } else { format!("{host}:{}", url.port) };
    let mut head = format!("{} {} HTTP/1.1\r\nHost: {host}\r\nConnection: {}\r\n", req.method, url.path, if keep { "keep-alive" } else { "close" });
    head.push_str(&format!("User-Agent: MailALM/{}\r\n", env!("CARGO_PKG_VERSION")));
    if header_in(&req.headers, "Accept").is_none() {
        head.push_str("Accept: */*\r\n");
    }
    for (k, v) in &req.headers {
        if !header_ok(k, v) {
            return Err(NetError::Invalid("a request header that isn't one".into()));
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !req.body.is_empty() || matches!(req.method, "POST" | "PUT" | "PATCH") {
        head.push_str(&format!("Content-Length: {}\r\n", req.body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(&req.body)?;
    s.flush()?;
    Ok(())
}

/// One answer's status and headers, read on from `buf` (what came before), and whatever came
/// after them.
#[allow(clippy::type_complexity)]
fn read_head<S: Read>(s: &mut S, mut buf: Vec<u8>) -> Result<(u16, Vec<(String, String)>, Vec<u8>)> {
    let mut tmp = [0u8; 16 << 10];
    let mut from = 0;
    let head_end = loop {
        if let Some(p) = buf.get(from..).and_then(|b| b.windows(4).position(|w| w == b"\r\n\r\n")) {
            break from + p;
        }
        if buf.len() > MAX_HEAD {
            return Err(NetError::TooLarge("the answer's header is too long".into()));
        }
        from = buf.len().saturating_sub(3);
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Err(NetError::Closed("the server closed the connection before answering".into()));
        }
        buf.extend_from_slice(tmp.get(..n).unwrap_or_default());
    };
    let head = String::from_utf8_lossy(buf.get(..head_end).unwrap_or_default()).into_owned();
    let rest: Vec<u8> = buf.get(head_end + 4..).map(<[u8]>::to_vec).unwrap_or_default();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(NetError::Protocol(format!("not an HTTP/1.x answer: {}", crate::clip(&status_line.chars().take(60).collect::<String>()))));
    }
    let status: u16 = parts
        .next()
        .and_then(|c| c.parse().ok())
        .filter(|c| (100..600).contains(c))
        .ok_or_else(|| NetError::Protocol(format!("bad status line: {}", crate::clip(&status_line.chars().take(60).collect::<String>()))))?;
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).take(MAX_HEADERS).collect();
    Ok((status, headers, rest))
}

/// The answer and its body (at most `max` bytes), read whole, and whether the connection can
/// carry no other answer though this one came whole.
fn read_answer<S: Read>(s: &mut S, max: usize, head_only: bool) -> Result<(Response, bool)> {
    let mut rest = Vec::new();
    for _ in 0..=MAX_INTERIM {
        let (status, headers, after) = read_head(s, rest)?;
        rest = after;
        if status == 101 {
            return Err(NetError::Protocol("the server switched protocols".into()));
        }
        // An interim answer (100 Continue, 103 Early Hints): the real one follows.
        if (100..200).contains(&status) {
            continue;
        }
        let mut resp = Response { status, headers, body: Vec::new() };
        if head_only || status == 204 || status == 304 {
            return Ok((resp, !rest.is_empty()));
        }
        let mut ahead = body::Ahead::new(s, rest);
        let both = resp.header("Content-Length").is_some() && resp.header("Transfer-Encoding").is_some();
        body::copy_body(&mut ahead, &resp.headers, u64::try_from(max).unwrap_or(u64::MAX), &mut resp.body)?;
        // Both lengths (RFC 9112 §6.3: the chunks count, and the connection is closed after), or
        // bytes after the answer that nobody asked for.
        let cut = ahead.cut() || ahead.leftover() || both;
        return Ok((resp, cut));
    }
    Err(NetError::Protocol("too many interim answers".into()))
}

/// `application/x-www-form-urlencoded` (for OAuth token requests).
pub(crate) fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    pairs.iter().map(|(k, v)| format!("{}={}", pct(k), pct(v))).collect::<Vec<_>>().join("&").into_bytes()
}

/// Percent-encode everything but the unreserved characters (RFC 3986 §2.3), byte by byte of the
/// UTF-8, in upper-case hex.
pub(crate) fn pct(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len().saturating_mul(3));
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX.get(usize::from(b >> 4)).copied().unwrap_or(b'0')));
            out.push(char::from(HEX.get(usize::from(b & 15)).copied().unwrap_or(b'0')));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
