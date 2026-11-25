// Partly adapted from WordCraft (https://github.com/storytold/wordcraft), commit ca7f262a597c75da4a157b730c68aa2db8d12734, the tests of apps/wordcraft/src/control_server.rs. MIT OR Apache-2.0.
// Changed: MailALM's server, client and request format; one request per connection; the key and port files; the confirmation timeouts.

use super::*;
use crate::client::{Client, ClientError};
use serde_json::json;
use std::io::BufRead;
use std::sync::Mutex;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailalm-control-server-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn small() -> Limits {
    Limits { first_line: Duration::from_secs(5), queue: Duration::from_millis(400), run: Duration::from_millis(900), max_connections: 8 }
}

fn boot(name: &str, limits: Limits) -> (ControlServer, PathBuf) {
    let dir = temp_dir(name);
    let server = ControlServer::start_with(&dir, Arc::new(|| {}), limits).unwrap();
    (server, dir)
}

/// A stand-in for MailALM's UI thread: every request goes to `answer`. Returns the number of
/// requests that reached it, and a handle that stops it (and the server) when dropped.
struct App {
    seen: Arc<AtomicUsize>,
    refused: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl App {
    fn run(server: ControlServer, answer: impl Fn(Incoming) + Send + 'static) -> App {
        let seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let refused = server.refused.clone();
        let (s, st) = (seen.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                if let Some(i) = server.next_timeout(Duration::from_millis(20)) {
                    s.fetch_add(1, Ordering::SeqCst);
                    answer(i);
                }
            }
        });
        App { seen, refused, stop, thread: Some(thread) }
    }

    fn seen(&self) -> usize {
        self.seen.load(Ordering::SeqCst)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Answers every request with its tool's name.
fn echo(i: Incoming) {
    assert!(i.start() && i.begin_run());
    let name = i.tool().name();
    i.answer(Ok(json!({"tool": name})));
}

/// One raw connection: send `bytes`, read the answer line (`None` if the server closed without
/// one).
fn raw(port: u16, bytes: &[u8]) -> Option<Value> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(bytes).unwrap();
    // The request is complete: the server reads no more after it.
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut line = String::new();
    BufReader::new(&s).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn key_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(key::KEY_FILE)).unwrap()
}

#[test]
fn a_request_with_the_key_reaches_the_app_and_gets_its_answer() {
    let (server, dir) = boot("good", Limits::DEFAULT);
    let port = server.port();
    assert_eq!(key::read_port(&dir).unwrap(), port);
    assert_eq!(key_of(&dir).len(), 64);
    let app = App::run(server, echo);
    let mut client = Client::new(dir.clone());
    assert_eq!(client.call(Tool::ListAccounts, &json!({})).unwrap(), json!({"tool": "list_accounts"}));
    assert_eq!(client.call(Tool::Search, &json!({"query": "x"})).unwrap(), json!({"tool": "search"}));
    assert_eq!(app.seen(), 2);
    // A raw request: the answer carries the request's id.
    let line = format!("{}\n", json!({"key": key_of(&dir), "id": 42, "tool": "list_folders", "args": {"account": 1}}));
    assert_eq!(raw(port, line.as_bytes()).unwrap(), json!({"id": 42, "ok": {"tool": "list_folders"}}));
    assert_eq!(app.refused.load(Ordering::SeqCst), 0);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wrong_or_missing_key_is_refused_and_counted() {
    let (server, dir) = boot("wrong", Limits::DEFAULT);
    let port = server.port();
    let app = App::run(server, echo);
    let good = key_of(&dir);
    let mut other = good.clone().into_bytes();
    other[0] = if other[0] == b'0' { b'1' } else { b'0' };
    let other = String::from_utf8(other).unwrap();
    for key in [json!(other), json!(good.to_uppercase()), json!(&good[..63]), json!(""), json!(7), Value::Null] {
        let line = format!("{}\n", json!({"key": key, "id": 5, "tool": "list_accounts", "args": {}}));
        let answer = raw(port, line.as_bytes()).unwrap();
        assert_eq!(answer["error"]["code"], "unauthorized", "{key}");
        assert_eq!(answer["id"], 5);
    }
    let answer = raw(port, b"{\"id\":6,\"tool\":\"list_accounts\"}\n").unwrap();
    assert_eq!(answer["error"]["code"], "unauthorized");
    // The key in the arguments does not count.
    let line = format!("{}\n", json!({"id": 7, "tool": "list_accounts", "args": {"key": good}}));
    assert_eq!(raw(port, line.as_bytes()).unwrap()["error"]["code"], "unauthorized");
    assert_eq!(app.seen(), 0, "nothing reached the app");
    assert_eq!(app.refused.load(Ordering::SeqCst), 8);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_oversize_line_is_refused() {
    let (server, dir) = boot("oversize", Limits::DEFAULT);
    let port = server.port();
    let app = App::run(server, echo);
    let mut big = format!(r#"{{"key":"{}","id":1,"tool":"search","args":{{"query":""#, key_of(&dir)).into_bytes();
    big.resize(MAX_REQUEST_LINE + 10, b'x');
    big.extend_from_slice(b"\"}}\n");
    let answer = raw(port, &big).unwrap();
    assert_eq!(answer["error"]["code"], "line_too_long");
    assert_eq!(app.seen(), 0);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_requests_are_refused() {
    let (server, dir) = boot("malformed", Limits::DEFAULT);
    let port = server.port();
    let app = App::run(server, echo);
    let key = key_of(&dir);
    assert_eq!(raw(port, b"not json\n").unwrap()["error"]["code"], "bad_request");
    assert_eq!(raw(port, b"\xff\xfe\n").unwrap()["error"]["code"], "bad_request");
    // What a web page's fetch() to this port sends, even with the key in the body.
    let body = format!("{}\n", json!({"key": key, "id": 1, "tool": "send_message", "args": {}}));
    let http = format!("POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    // The server closes after the first line: the error may be lost in the reset.
    if let Some(answer) = raw(port, http.as_bytes()) {
        assert_eq!(answer["error"]["code"], "bad_request");
    }
    // With the key: a missing field, an unknown field, an unknown tool.
    let line = format!("{}\n", json!({"key": key, "id": 2, "args": {}}));
    assert_eq!(raw(port, line.as_bytes()).unwrap()["error"]["code"], "bad_request");
    let line = format!("{}\n", json!({"key": key, "id": 3, "tool": "search", "args": {}, "extra": 1}));
    assert_eq!(raw(port, line.as_bytes()).unwrap()["error"]["code"], "bad_request");
    let line = format!("{}\n", json!({"key": key, "id": 4, "tool": "run_program", "args": {}}));
    let answer = raw(port, line.as_bytes()).unwrap();
    assert_eq!((answer["id"].clone(), answer["error"]["code"].clone()), (json!(4), json!("unknown_tool")));
    // A connection that closes without a line gets nothing, and harms nothing.
    assert_eq!(raw(port, b""), None);
    assert_eq!(app.seen(), 0);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_127_0_0_1_is_a_peer() {
    let peer = |s: &str| peer_allowed(s.parse::<SocketAddr>().unwrap());
    assert!(peer("127.0.0.1:50000"));
    assert!(peer("[::ffff:127.0.0.1]:50000"));
    for other in ["127.0.0.2:50000", "[::1]:50000", "192.168.1.10:50000", "10.0.0.1:50000", "0.0.0.0:50000", "[fe80::1]:50000"] {
        assert!(!peer(other), "{other}");
    }
}

#[test]
fn a_request_the_app_never_takes_is_never_run() {
    let (server, dir) = boot("queue-timeout", small());
    let mut client = Client::new(dir.clone());
    match client.call(Tool::MarkMessages, &json!({})) {
        Err(ClientError::Tool(e)) => assert_eq!(e.code, ErrorCode::TimeoutNotRun, "{e}"),
        other => panic!("{other:?}"),
    }
    // The app gets to it late: it must not run it.
    let late = server.next_timeout(Duration::from_secs(5)).unwrap();
    assert!(!late.start());
    assert!(late.is_cancelled());
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_confirmation_left_unanswered_is_never_run() {
    let (server, dir) = boot("confirm-timeout", small());
    let held: Arc<Mutex<Vec<Incoming>>> = Arc::new(Mutex::new(Vec::new()));
    let h = held.clone();
    // The app takes the request (shows the dialog) and the user never decides.
    let app = App::run(server, move |i| {
        assert!(i.start());
        h.lock().unwrap().push(i);
    });
    let mut client = Client::new(dir.clone());
    match client.call(Tool::SendMessage, &json!({"account": 1, "to": ["a@b.ca"]})) {
        Err(ClientError::Tool(e)) => {
            assert_eq!(e.code, ErrorCode::TimeoutNotRun, "{e}");
            assert!(e.message.contains("confirm"), "{e}");
        }
        other => panic!("{other:?}"),
    }
    // The user says yes too late: it must not run.
    let i = held.lock().unwrap().pop().unwrap();
    assert!(!i.begin_run());
    assert!(i.is_cancelled());
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_request_dropped_while_running_has_an_unknown_outcome() {
    let (server, dir) = boot("dropped", small());
    let app = App::run(server, |i| {
        assert!(i.start() && i.begin_run());
        drop(i);
    });
    let mut client = Client::new(dir.clone());
    match client.call(Tool::DeleteMessages, &json!({})) {
        Err(ClientError::Tool(e)) => assert_eq!(e.code, ErrorCode::OutcomeUnknown, "{e}"),
        other => panic!("{other:?}"),
    }
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stopping_removes_the_files_and_the_client_says_mailalm_is_not_running() {
    let (server, dir) = boot("stop", Limits::DEFAULT);
    let port = server.port();
    server.stop();
    assert!(!dir.join(key::KEY_FILE).exists() && !dir.join(key::PORT_FILE).exists());
    assert!(TcpStream::connect_timeout(&SocketAddr::new(LOOPBACK, port), Duration::from_millis(500)).is_err(), "still listening");
    let mut client = Client::new(dir.clone());
    let e = client.call(Tool::ListAccounts, &json!({})).unwrap_err();
    assert!(matches!(e, ClientError::NotRunning(_)), "{e:?}");
    assert!(e.to_string().contains("MailALM is not running"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn each_start_writes_a_new_key() {
    let (first, dir) = boot("rotate", Limits::DEFAULT);
    let old = key_of(&dir);
    let second = ControlServer::start_with(&dir, Arc::new(|| {}), Limits::DEFAULT).unwrap();
    let new = key_of(&dir);
    assert_ne!(old, new);
    // The first server stops without removing the second one's files.
    drop(first);
    assert_eq!(key_of(&dir), new);
    assert_eq!(key::read_port(&dir).unwrap(), second.port());
    drop(second);
    assert!(!dir.join(key::KEY_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn connections_are_limited() {
    let (server, dir) = boot("limit", Limits { max_connections: 2, ..Limits::DEFAULT });
    let port = server.port();
    let app = App::run(server, echo);
    let idle: Vec<TcpStream> = (0..2).map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(300));
    let third = TcpStream::connect(("127.0.0.1", port)).unwrap();
    third.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 1];
    assert!(matches!((&third).read(&mut buf), Ok(0) | Err(_)), "the third connection was served");
    drop(idle);
    std::thread::sleep(Duration::from_millis(300));
    let mut client = Client::new(dir.clone());
    assert_eq!(client.call(Tool::ListAccounts, &json!({})).unwrap(), json!({"tool": "list_accounts"}));
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_silent_connection_is_closed() {
    let (server, dir) = boot("silent", Limits { first_line: Duration::from_millis(300), ..Limits::DEFAULT });
    let port = server.port();
    let app = App::run(server, echo);
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let t0 = Instant::now();
    // A slow drip of bytes does not keep it open.
    let mut w = s.try_clone().unwrap();
    let drip = std::thread::spawn(move || {
        for _ in 0..30 {
            if w.write_all(b" ").is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let mut buf = [0u8; 16];
    assert!(matches!((&s).read(&mut buf), Ok(0) | Err(_)));
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    let _ = drip.join();
    assert_eq!(app.seen(), 0);
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}
