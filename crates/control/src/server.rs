// Adapted from WordCraft (https://github.com/storytold/wordcraft), commit ca7f262a597c75da4a157b730c68aa2db8d12734, apps/wordcraft/src/control_server.rs (accept thread, Admission, Timed, serve) and crates/ui-egui/src/control.rs (ControlRequest with its waiter). MIT OR Apache-2.0.
// Changed: no egui (a wake callback instead of request_repaint); the port is picked by the system and written to a file; one request per connection, so no `auth` method and no idle connections; every connection counts against one limit; peers other than 127.0.0.1 are refused; a request is checked against the tool catalog before it reaches the app; a request the app has taken (a confirmation dialog) waits longer than one still queued, and the app cannot start running it once the connection has given up; the accept loop polls a stop flag, and stopping removes the key and port files.

//! The server side, embedded in MailALM: [`ControlServer::start`] listens on `127.0.0.1` while
//! the MCP is turned on.
//!
//! * The listener binds `127.0.0.1` on a port the system picks; the port and a new key go to the
//!   files ([`crate::key`]). Stopping (or dropping) the server removes them.
//! * Each connection gets its own thread, at most [`MAX_CONNECTIONS`] at once; a peer whose address
//!   is not `127.0.0.1` is closed at once and counted.
//! * A connection sends one request line within [`FIRST_LINE`] (one deadline for the whole line,
//!   so a slow drip of bytes does not extend it), gets one answer line, and is closed.
//! * The key is checked first, in constant time; a wrong key is answered `unauthorized`, logged
//!   and counted. Then the JSON shape and the tool name; nothing else reaches the app.
//! * The request goes to the app as an [`Incoming`] through a channel, and the app's wake
//!   callback runs (`egui::Context::request_repaint`). The connection waits for the answer:
//!   [`QUEUE_WAIT`] for the app to take it, then [`RUN_WAIT`] in all for it to be done (a
//!   confirmation dialog included). A request the app never took is never run.

use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::catalog::Tool;
use crate::key::{self, ControlError, Key};
use crate::wire::{Answer, ErrorCode, Line, MAX_REQUEST_LINE, RequestIn, ToolError, read_line, skip_line};

/// Connections served at once; more are closed at once.
pub const MAX_CONNECTIONS: usize = 8;
/// Time for a connection to send its whole request line.
pub const FIRST_LINE: Duration = Duration::from_secs(10);
/// Time for the app to take a request; after it, the request is never run.
pub const QUEUE_WAIT: Duration = Duration::from_secs(30);
/// Time for the app to answer a request it took (the user's confirmation included); after it,
/// a request not yet started is never run, one already running has an unknown outcome.
pub const RUN_WAIT: Duration = Duration::from_secs(300);
/// How often the accept loop looks at the stop flag.
const POLL: Duration = Duration::from_millis(100);
/// Time to write an answer.
const WRITE_WAIT: Duration = Duration::from_secs(10);
/// Most bytes of an over-long request read and dropped, so the client can read the error.
const SKIP_LIMIT: usize = 64 << 20;

/// The only address MailALM listens on and accepts peers from.
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Whether a peer may be served: its address is 127.0.0.1 (also as `::ffff:127.0.0.1`). Other
/// loopback addresses (127.0.0.2, ::1) are not MailALM's.
pub fn peer_allowed(peer: SocketAddr) -> bool {
    peer.ip().to_canonical() == LOOPBACK
}

/// The server's time and count budgets (small ones in tests).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub first_line: Duration,
    pub queue: Duration,
    pub run: Duration,
    pub max_connections: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits { first_line: FIRST_LINE, queue: QUEUE_WAIT, run: RUN_WAIT, max_connections: MAX_CONNECTIONS };
}

// The life of a request, shared by the app's `Incoming` and the connection's waiter.
const QUEUED: u8 = 0;
/// The app took it (it may be showing a confirmation).
const STARTED: u8 = 1;
/// The app is doing it.
const RUNNING: u8 = 2;
/// The connection gave up before it ran: it never will.
const CANCELLED: u8 = 3;

/// A request for the app: an authenticated call to a known tool. The app calls [`Incoming::start`]
/// when it takes it, [`Incoming::begin_run`] right before doing it, and answers with
/// [`Incoming::answer`]. [`crate::handle`] does all of this.
#[derive(Debug)]
pub struct Incoming {
    id: u64,
    tool: Tool,
    args: Value,
    reply: Sender<Answer>,
    state: Arc<AtomicU8>,
}

impl Incoming {
    /// A request and the waiter its connection keeps.
    pub(crate) fn queued(id: u64, tool: Tool, args: Value) -> (Incoming, Waiter) {
        let (reply, rx) = channel();
        let state = Arc::new(AtomicU8::new(QUEUED));
        (Incoming { id, tool, args, reply, state: state.clone() }, Waiter { id, rx, state })
    }

    /// The request's id, as the client gave it.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// The arguments as sent (not checked yet: [`crate::Call::parse`] does it).
    pub fn args(&self) -> &Value {
        &self.args
    }

    pub(crate) fn take_args(&mut self) -> Value {
        std::mem::take(&mut self.args)
    }

    /// The app takes the request. False when the connection already gave up: it must not run.
    pub fn start(&self) -> bool {
        self.state.compare_exchange(QUEUED, STARTED, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// The app is about to do it (after the user's confirmation, for a sensitive action). False
    /// when the connection gave up meanwhile: it must not run.
    pub fn begin_run(&self) -> bool {
        self.state.compare_exchange(STARTED, RUNNING, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// The connection gave up on it: it was not run and must not be.
    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CANCELLED
    }

    /// Send the answer (dropped if the connection is gone).
    pub fn answer(self, result: Result<Value, ToolError>) {
        let _ = self.reply.send(Answer::from_result(self.id, result));
    }
}

/// The connection's side of a request: waits for the app's answer within the limits.
#[derive(Debug)]
pub(crate) struct Waiter {
    id: u64,
    pub(crate) rx: Receiver<Answer>,
    state: Arc<AtomicU8>,
}

impl Waiter {
    fn cancel_if(&self, from: u8) -> bool {
        self.state.compare_exchange(from, CANCELLED, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// What the connection does when its time is up, before the app has started running it.
    #[cfg(test)]
    pub(crate) fn give_up(&self) -> bool {
        self.cancel_if(QUEUED) || self.cancel_if(STARTED)
    }

    fn not_run(&self, message: &str) -> Answer {
        Answer::error(self.id, ToolError::new(ErrorCode::TimeoutNotRun, message))
    }

    /// The answer when the app dropped the request without one.
    fn dropped(&self) -> Answer {
        if self.state.load(Ordering::SeqCst) == RUNNING {
            Answer::error(self.id, ToolError::new(ErrorCode::OutcomeUnknown, "MailALM stopped while doing it: it may have been done"))
        } else {
            Answer::error(self.id, ToolError::failed("MailALM closed the request without doing it"))
        }
    }

    pub(crate) fn wait(self, limits: &Limits) -> Answer {
        match self.rx.recv_timeout(limits.queue) {
            Ok(a) => return a,
            Err(RecvTimeoutError::Disconnected) => return self.dropped(),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if self.cancel_if(QUEUED) {
            return self.not_run("MailALM did not take the request in time (it may be busy); it was not run and never will be");
        }
        match self.rx.recv_timeout(limits.run.saturating_sub(limits.queue)) {
            Ok(a) => return a,
            Err(RecvTimeoutError::Disconnected) => return self.dropped(),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if self.cancel_if(STARTED) {
            return self.not_run("the user did not confirm in time; nothing was done and nothing will be");
        }
        // Running: one last short wait for the result.
        self.rx.recv_timeout(WRITE_WAIT).unwrap_or_else(|_| {
            Answer::error(self.id, ToolError::new(ErrorCode::OutcomeUnknown, "MailALM did not answer in time: it may have been done; check before trying again"))
        })
    }
}

/// Wakes the app when a request arrives (`egui::Context::request_repaint`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// What every connection thread shares.
#[derive(Clone)]
struct Shared {
    key: Arc<Key>,
    calls: Sender<Incoming>,
    wake: Waker,
    stop: Arc<AtomicBool>,
    refused: Arc<AtomicU64>,
    open: Arc<AtomicUsize>,
    limits: Limits,
}

/// A running control server. Dropping it stops it and removes the key and port files.
pub struct ControlServer {
    calls: Receiver<Incoming>,
    port: u16,
    dir: PathBuf,
    key: Key,
    stop: Arc<AtomicBool>,
    refused: Arc<AtomicU64>,
    accept: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ControlServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlServer").field("port", &self.port).field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl ControlServer {
    /// Listen on `127.0.0.1` (a port the system picks), write a new key and the port to `dir`
    /// (normally [`crate::profile_dir`]), and serve on a thread of its own. `wake` runs whenever a
    /// request is waiting for the app.
    pub fn start(dir: &Path, wake: impl Fn() + Send + Sync + 'static) -> Result<ControlServer, ControlError> {
        Self::start_with(dir, Arc::new(wake), Limits::DEFAULT)
    }

    pub(crate) fn start_with(dir: &Path, wake: Waker, limits: Limits) -> Result<ControlServer, ControlError> {
        let listener = TcpListener::bind(SocketAddr::new(LOOPBACK, 0)).map_err(ControlError::Bind)?;
        let port = listener.local_addr().map_err(ControlError::Bind)?.port();
        listener.set_nonblocking(true).map_err(ControlError::Bind)?;
        let key = key::write_key(dir)?;
        if let Err(e) = key::write_port(dir, port) {
            key::remove_files(dir, &key);
            return Err(e);
        }
        let (tx, calls) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let refused = Arc::new(AtomicU64::new(0));
        let shared = Shared {
            key: Arc::new(key.clone()),
            calls: tx,
            wake,
            stop: stop.clone(),
            refused: refused.clone(),
            open: Arc::new(AtomicUsize::new(0)),
            limits,
        };
        let accept = std::thread::Builder::new().name("mailalm-control".into()).spawn(move || accept_loop(&listener, &shared));
        let accept = match accept {
            Ok(handle) => handle,
            Err(e) => {
                key::remove_files(dir, &key);
                return Err(ControlError::Thread(e));
            }
        };
        log::info!("MCP control: listening on 127.0.0.1:{port}, key in {}", dir.join(key::KEY_FILE).display());
        Ok(ControlServer { calls, port, dir: dir.to_path_buf(), key, stop, refused, accept: Some(accept) })
    }

    /// The port on 127.0.0.1.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The folder of the key and port files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The next request for the app, if one is waiting. Never blocks: for the UI thread.
    pub fn try_next(&self) -> Option<Incoming> {
        self.calls.try_recv().ok()
    }

    /// The next request, waiting at most `wait` (for a thread of the app's own).
    pub fn next_timeout(&self, wait: Duration) -> Option<Incoming> {
        self.calls.recv_timeout(wait).ok()
    }

    /// Connections refused so far: wrong or missing key, or a peer other than 127.0.0.1.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::SeqCst)
    }

    /// Stop listening and remove the key and port files. Requests still waiting are answered
    /// that MailALM closed them; one already running ends as it does.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        key::remove_files(&self.dir, &self.key);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
            log::info!("MCP control: stopped");
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Counts one open connection until it ends.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(open: &Arc<AtomicUsize>, max: usize) -> Option<Slot> {
        if open.fetch_add(1, Ordering::SeqCst) >= max {
            open.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot(open.clone()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn accept_loop(listener: &TcpListener, shared: &Shared) {
    while !shared.stop.load(Ordering::SeqCst) {
        let (stream, peer) = match listener.accept() {
            Ok(c) => c,
            Err(e) => {
                if e.kind() != io::ErrorKind::WouldBlock {
                    log::warn!("MCP control: accept failed: {e}");
                }
                std::thread::sleep(POLL);
                continue;
            }
        };
        if !peer_allowed(peer) {
            shared.refused.fetch_add(1, Ordering::SeqCst);
            log::warn!("MCP control: a connection from {peer} was refused (only 127.0.0.1 is served)");
            continue;
        }
        let Some(slot) = Slot::take(&shared.open, shared.limits.max_connections) else {
            log::warn!("MCP control: too many connections, one closed");
            continue;
        };
        let conn = shared.clone();
        // A thread that cannot start closes this connection only (the slot is freed with it).
        if let Err(e) = std::thread::Builder::new().name("mailalm-control-conn".into()).spawn(move || serve(stream, &conn, slot)) {
            log::warn!("MCP control: no thread for a connection: {e}");
        }
    }
}

/// The read side of a connection with one overall deadline: each read waits only for the time
/// left, so a slow drip of bytes cannot extend it.
struct Deadline {
    stream: TcpStream,
    until: Instant,
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or(io::ErrorKind::TimedOut)?;
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf)
    }
}

fn serve(stream: TcpStream, shared: &Shared, _slot: Slot) {
    // On some systems an accepted socket inherits the listener's non-blocking mode.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(WRITE_WAIT));
    let Ok(read) = stream.try_clone() else { return };
    let mut out = stream;
    let mut reader = BufReader::new(Deadline { stream: read, until: Instant::now() + shared.limits.first_line });
    let answer = match read_line(&mut reader, MAX_REQUEST_LINE) {
        // Closed, or nothing in time: nothing to answer.
        Err(_) | Ok(Line::Eof) => return,
        Ok(Line::TooLong) => {
            let answer = Answer::error(0, ToolError::new(ErrorCode::LineTooLong, format!("a request line is at most {MAX_REQUEST_LINE} bytes")));
            send(&mut out, &answer);
            // Read what is left of the line, so closing does not reset the connection before the
            // client has read the error.
            reader.get_mut().until = Instant::now() + Duration::from_secs(2);
            skip_line(&mut reader, SKIP_LIMIT);
            return;
        }
        Ok(Line::NotUtf8) => Answer::error(0, ToolError::new(ErrorCode::BadRequest, "the request is not UTF-8")),
        Ok(Line::Text(line)) => answer_line(&line, shared),
    };
    send(&mut out, &answer);
}

fn send(out: &mut TcpStream, answer: &Answer) {
    let mut line = answer.to_line();
    line.push('\n');
    let _ = out.write_all(line.as_bytes()).and_then(|()| out.flush());
}

/// The answer to one request line.
fn answer_line(line: &str, shared: &Shared) -> Answer {
    // Not JSON (an HTTP request from a web page, say): refused before anything else.
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Answer::error(0, ToolError::new(ErrorCode::BadRequest, "not a JSON request"));
    };
    let id = value.get("id").and_then(Value::as_u64).unwrap_or(0);
    let given = value.get("key").and_then(Value::as_str).unwrap_or("");
    if !shared.key.matches(given) {
        shared.refused.fetch_add(1, Ordering::SeqCst);
        log::warn!("MCP control: a request without the right key was refused");
        return Answer::error(id, ToolError::new(ErrorCode::Unauthorized, "unauthorized"));
    }
    let request: RequestIn = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => return Answer::error(id, ToolError::new(ErrorCode::BadRequest, format!("not a request: {e}"))),
    };
    let Some(tool) = Tool::from_name(&request.tool) else {
        let name: String = request.tool.chars().take(64).collect();
        return Answer::error(request.id, ToolError::new(ErrorCode::UnknownTool, format!("no tool named {name:?}")));
    };
    let (incoming, waiter) = Incoming::queued(request.id, tool, request.args);
    if shared.calls.send(incoming).is_err() {
        return Answer::error(request.id, ToolError::failed("MailALM is closing its MCP access"));
    }
    (shared.wake)();
    waiter.wait(&shared.limits)
}

#[cfg(test)]
mod tests;
