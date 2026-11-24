// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/http/tests.rs. MIT OR Apache-2.0.
// Changed: the local test server can answer several requests on one connection; tests for what MailALM keeps (no redirects, https only, kept connections); no measurements, no event streams.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use super::*;

/// How long a test's server waits for its next connection, and for each read or write on one,
/// before it gives up: a client that never comes fails the test that waits on the server, rather
/// than hanging the whole run.
pub(crate) const TEST_SERVER_WAIT: Duration = Duration::from_secs(60);

/// The next connection to `l` within `wait`, its reads and writes timing out after as long;
/// `None` when none came in that time.
pub(crate) fn accept_within(l: &TcpListener, wait: Duration) -> Option<TcpStream> {
    l.set_nonblocking(true).ok()?;
    let end = Instant::now() + wait;
    loop {
        match l.accept() {
            Ok((s, _)) => {
                // Taken from a listener that doesn't block, it wouldn't either (on Windows).
                s.set_nonblocking(false).ok()?;
                s.set_read_timeout(Some(wait)).ok()?;
                s.set_write_timeout(Some(wait)).ok()?;
                return Some(s);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock && Instant::now() < end => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return None,
        }
    }
}

/// One request off `reader`: its head and body as text; `None` when the connection ends first.
pub(crate) fn read_request<R: BufRead>(reader: &mut R) -> Option<String> {
    let mut req = String::new();
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        req.push_str(&line);
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    req.push_str("\r\n");
    req.push_str(&String::from_utf8_lossy(&body));
    Some(req)
}

/// Serve `responses` (one per connection, in order) on 127.0.0.1; returns the base URL and the
/// requests received (head, an empty line, then the body).
pub(crate) fn serve(responses: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    serve_sessions(responses.into_iter().map(|r| vec![r]).collect())
}

/// Serve `sessions` on 127.0.0.1: each is a connection, answering its requests in turn with its
/// responses, then closed. A request that doesn't come within [`TEST_SERVER_WAIT`] fails the
/// server's thread, and so whoever joins it, saying how many came.
pub(crate) fn serve_sessions(sessions: Vec<Vec<Vec<u8>>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let all: usize = sessions.iter().map(Vec::len).sum();
        for session in sessions {
            let Some(mut s) = accept_within(&l, TEST_SERVER_WAIT) else {
                panic!("the test server was sent {} of its {all} requests: no other came", seen.len());
            };
            let mut reader = BufReader::new(s.try_clone().unwrap());
            for r in session {
                let Some(req) = read_request(&mut reader) else {
                    panic!("the test server was sent {} of its {all} requests: a connection ended", seen.len());
                };
                seen.push(req);
                // The client may have hung up on an answer it refused.
                let _ = s.write_all(&r);
            }
        }
        seen
    });
    (base, h)
}

fn get(url: &str, max: usize) -> Result<Response> {
    send(&Request::get(url, max))
}

#[test]
fn urls() {
    let u = Url::parse("HTTPS://Graph.Microsoft.com/v1.0/me?x=1#frag").unwrap();
    assert_eq!((u.https, u.host.as_str(), u.port, u.path.as_str()), (true, "graph.microsoft.com", 443, "/v1.0/me?x=1"));
    assert_eq!(Url::parse("http://h:8080").unwrap().path, "/");
    assert_eq!(Url::parse("http://localhost:8080?code=a").unwrap().path, "/?code=a");
    assert_eq!(Url::parse("http://[::1]:99/x").unwrap().host, "::1");
    assert_eq!(Url::parse("https://h/a b\u{85}").unwrap().path, "/a%20b%C2%85");
    // Graph's links name folders this way.
    assert_eq!(Url::parse("https://graph.microsoft.com/v1.0/me/mailfolders('AQ=')/messages/delta?$skiptoken=G").unwrap().host, "graph.microsoft.com");
    for bad in ["ftp://h/", "http://", "http://user@h/", "http://h:0/", "http://h:x/", "javascript:alert(1)", "http://h h/", "https://h\r\nX: y/"] {
        assert!(Url::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(pct("a b&c=é~-._"), "a%20b%26c%3D%C3%A9~-._");
    assert_eq!(form(&[("a", "1 2"), ("b", "/")]), b"a=1%202&b=%2F".to_vec());
}

/// Plain http goes nowhere but to this computer, and only in tests.
#[test]
fn plain_http_is_for_this_computer_only() {
    assert!(matches!(get("http://example.com/", 10), Err(NetError::Tls(_))));
    assert!(is_loopback("127.0.0.1") && is_loopback("::1") && !is_loopback("127.0.0.2"));
}

#[test]
fn lengths_chunks_and_limits() {
    let (base, h) = serve(vec![
        b"HTTP/1.1 302 Found\r\nLocation: https://elsewhere.example/\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhello".to_vec(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 999999\r\n\r\nxx".to_vec(),
        b"HTTP/1.0 200 OK\r\n\r\nuntil the end".to_vec(),
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\nabc".to_vec(),
        b"garbage\r\n\r\n".to_vec(),
    ]);
    // A redirect is an answer: nothing follows it.
    let r = get(&format!("{base}/one"), 1000).unwrap();
    assert_eq!((r.status, r.header("location")), (302, Some("https://elsewhere.example/")));
    let r = get(&base, 1000).unwrap();
    assert_eq!((r.status, r.body.as_slice(), r.header("content-type")), (200, &b"hello"[..], Some("text/plain")));
    assert_eq!(get(&base, 1000).unwrap().body, b"Wikipedia");
    assert!(matches!(get(&base, 1000), Err(NetError::TooLarge(m)) if m.contains("larger")));
    assert_eq!(get(&base, 1000).unwrap().body, b"until the end");
    let r = send(
        &Request::post(&format!("{base}/token"), "application/x-www-form-urlencoded", form(&[("code", "x y")]), 100)
            .header("Authorization", "Bearer t"),
    )
    .unwrap();
    assert!(r.ok() && r.status == 201 && r.body == b"ok", "the interim answer skipped");
    assert!(matches!(get(&base, 1000), Err(NetError::Protocol(m)) if m.contains("Content-Length")));
    assert!(get(&base, 1000).is_err());
    let seen = h.join().unwrap();
    assert!(seen[0].starts_with("GET /one HTTP/1.1\r\nHost: 127.0.0.1:"), "{}", seen[0]);
    assert!(seen[0].contains("Connection: close\r\n"), "{}", seen[0]);
    assert!(seen[5].starts_with("POST /token ") && seen[5].contains("Authorization: Bearer t") && seen[5].ends_with("code=x%20y"), "{}", seen[5]);
    // Header injection is refused before anything is sent.
    assert!(matches!(send(&Request::get(&base, 10).header("X", "a\r\nEvil: 1")), Err(NetError::Invalid(_))));
    assert!(send(&Request::get(&base, 10).header("X: y", "a")).is_err());
}

/// A chunked answer carrying `n` bytes of `x`, in chunks of `size`.
fn chunked(n: usize, size: usize) -> Vec<u8> {
    let mut v = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    let mut left = n;
    while left > 0 {
        let c = left.min(size);
        v.extend(format!("{c:x}\r\n").as_bytes());
        v.extend(std::iter::repeat_n(b'x', c));
        v.extend(b"\r\n");
        left -= c;
    }
    v.extend(b"0\r\n\r\n");
    v
}

/// A chunked body is capped on its own bytes, not its framing: a body under the cap sent in small
/// chunks is taken whole, one byte more is refused.
#[test]
fn a_chunked_body_is_capped_by_its_size_not_its_framing() {
    let max = 100_000;
    let (base, h) = serve(vec![chunked(max, 1), chunked(max + 1, 1)]);
    let r = get(&base, max).unwrap();
    assert!(r.body.len() == max && r.body.iter().all(|b| *b == b'x'), "{} bytes", r.body.len());
    assert!(matches!(get(&base, max), Err(NetError::TooLarge(m)) if m.contains("larger")), "one byte past the cap");
    let _ = h.join();
}

#[test]
fn chunk_framing_is_checked() {
    assert_eq!(body::chunk_size(b"1aF;name=value").unwrap(), 0x1af);
    assert_eq!(body::chunk_size(b"10 ").unwrap(), 16);
    for bad in [&b""[..], b"+7", b" 7", b"0x7", b"g", b"ffffffffffffffffff"] {
        assert!(body::chunk_size(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
    // A chunk without its CRLF, and a size line that never ends.
    let (base, h) = serve(vec![
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabcXY0\r\n\r\n".to_vec(),
        [&b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"[..], &[b'0'; 2000]].concat(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab".to_vec(),
    ]);
    assert!(matches!(get(&base, 100), Err(NetError::Protocol(m)) if m.contains("chunk end")));
    assert!(matches!(get(&base, 100), Err(NetError::Protocol(_))));
    assert!(matches!(get(&base, 100), Err(NetError::Closed(_))));
    let _ = h.join();
}

/// A chunked answer whose connection ends before its trailer does is whole, but nothing more
/// goes on that connection: the next request would be read from the middle of nothing.
#[test]
fn a_connection_cut_in_the_trailer_isnt_used_again() {
    let head = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n";
    for (end, usable) in [("0\r\n", false), ("0\r\nX-Sum: 1\r\n", false), ("0\r\n\r\n", true)] {
        let (base, h) = serve(vec![format!("{head}{end}").into_bytes()]);
        let url = Url::parse(&base).unwrap();
        let mut conn = Conn::open(&url).unwrap();
        assert_eq!(conn.send(&Request::get(&base, 100), &url, true).unwrap().body, b"abc", "{end:?}");
        assert_eq!(conn.usable(), usable, "{end:?}");
        h.join().unwrap();
    }
}

/// A kept connection carries the next request; when the server has closed it in between, the
/// request goes again on a new one, unless it must never go twice.
#[test]
fn a_kept_connection_is_reused_and_opened_again_once() {
    let ok = |body: &str| format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes();
    let (base, h) = serve_sessions(vec![vec![ok("one"), ok("two")], vec![ok("three")]]);
    let mut agent = Agent::default();
    assert_eq!(agent.send(&Request::get(&format!("{base}/1"), 100), true).unwrap().body, b"one");
    assert_eq!(agent.send(&Request::get(&format!("{base}/2"), 100), true).unwrap().body, b"two");
    // The server closed the first connection after its two answers.
    assert_eq!(agent.send(&Request::get(&format!("{base}/3"), 100), true).unwrap().body, b"three");
    let seen = h.join().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].contains("Connection: keep-alive\r\n"), "{}", seen[0]);

    // A request that must not go twice: the failure is reported, saying it went out.
    let (base, h) = serve_sessions(vec![vec![ok("one")]]);
    let mut agent = Agent::default();
    assert_eq!(agent.send(&Request::get(&base, 100), true).unwrap().body, b"one");
    h.join().unwrap();
    let f = agent.send(&Request::post(&base, "text/plain", b"mail".to_vec(), 100), false).unwrap_err();
    assert!(f.stale, "{:?}", f.error);
}

/// A request's `Debug` shows no header value and no body: a token never reaches a log.
#[test]
fn a_requests_debug_hides_its_secrets() {
    let r = Request::post("https://h/token", "application/x-www-form-urlencoded", b"refresh_token=r-secret".to_vec(), 10)
        .header("Authorization", "Bearer t-secret");
    let d = format!("{r:?}");
    assert!(!d.contains("secret") && d.contains("Authorization"), "{d}");
}

/// A test server that takes a request and closes without a word.
#[test]
fn a_silent_server_is_a_closed_connection() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let h = std::thread::spawn(move || {
        let s = accept_within(&l, TEST_SERVER_WAIT).unwrap();
        let mut reader = BufReader::new(s);
        let _ = read_request(&mut reader);
    });
    assert!(matches!(get(&base, 10), Err(NetError::Closed(_)) | Err(NetError::Io(_))));
    h.join().unwrap();
}
