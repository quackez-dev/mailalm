// Adapted from WordCraft (https://github.com/storytold/wordcraft), commit ca7f262a597c75da4a157b730c68aa2db8d12734, crates/mcp/src/backend.rs (Remote: the key looked up from the app's file, the key sent only to a peer at 127.0.0.1, an unknown outcome never sent again). MIT OR Apache-2.0.
// Changed: always 127.0.0.1 at the port from MailALM's port file (no address to give, no environment key, no lazy connection kept); a new connection and a fresh read of the key and port for every call; nothing is ever sent twice; MailALM's request and answer format; errors say what to do when MailALM is not running or its MCP access is off.

//! The client side, for `mailalm-mcp`: one call, one connection to `127.0.0.1` at the port in
//! MailALM's port file, with the key from its key file (both read again for every call, so a
//! restart of MailALM, which writes a new key, needs nothing).

use std::fmt;
use std::io::{BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::catalog::Tool;
use crate::key::{self, ControlError};
use crate::server::{QUEUE_WAIT, RUN_WAIT, peer_allowed};
use crate::wire::{Answer, Line, MAX_ANSWER_LINE, RequestOut, ToolError, read_line};

/// Time to connect to MailALM.
const CONNECT_WAIT: Duration = Duration::from_secs(2);
/// Time to wait for an answer: MailALM's own limits, with some room.
const ANSWER_WAIT: Duration = Duration::from_secs(QUEUE_WAIT.as_secs() + RUN_WAIT.as_secs() + 30);

/// What to tell the agent when MailALM cannot be reached.
const NOT_RUNNING: &str = "MailALM is not running, or its MCP access is turned off (MailALM settings, MCP). Start MailALM and turn it on, then try again";

/// Why a call failed.
#[derive(Debug)]
pub enum ClientError {
    /// MailALM is not running, or its MCP access is off: no key or port file, or nothing
    /// listening. Nothing was done.
    NotRunning(String),
    /// The key or port file is damaged. Nothing was done.
    BadFiles(String),
    /// MailALM answered with an error (wrong arguments, hidden account, declined…).
    Tool(ToolError),
    /// The connection broke after the request was sent: a change may have been made.
    OutcomeUnknown(String),
    /// MailALM's answer could not be read.
    BadAnswer(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::NotRunning(why) => write!(f, "{NOT_RUNNING} ({why})"),
            ClientError::BadFiles(why) => write!(f, "MailALM's MCP files are damaged ({why}); turning MCP access off and on again in MailALM writes them anew"),
            ClientError::Tool(e) => write!(f, "{e}"),
            ClientError::OutcomeUnknown(why) => write!(f, "outcome unknown: {why}; it may have been done, check in MailALM before trying again"),
            ClientError::BadAnswer(why) => write!(f, "MailALM's answer could not be read: {why}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Reaches the running MailALM.
#[derive(Debug)]
pub struct Client {
    dir: PathBuf,
    next_id: u64,
}

impl Client {
    /// A client for the MailALM whose files are in `dir`.
    pub fn new(dir: PathBuf) -> Client {
        Client { dir, next_id: 1 }
    }

    /// A client for the MailALM of this user (`%LOCALAPPDATA%\MailALM`).
    pub fn from_profile() -> Result<Client, ControlError> {
        key::profile_dir().map(Client::new).ok_or(ControlError::NoProfileDir)
    }

    /// Call `tool` with `args` and return its answer. Never sent twice.
    pub fn call(&mut self, tool: Tool, args: &Value) -> Result<Value, ClientError> {
        let files = |e: ControlError| if e.is_missing_file() { ClientError::NotRunning(e.to_string()) } else { ClientError::BadFiles(e.to_string()) };
        let port = key::read_port(&self.dir).map_err(files)?;
        let key = key::read_key(&self.dir).map_err(files)?;
        let at = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let stream = TcpStream::connect_timeout(&at, CONNECT_WAIT).map_err(|e| ClientError::NotRunning(format!("127.0.0.1:{port}: {e}")))?;
        // The key goes to 127.0.0.1 only.
        match stream.peer_addr() {
            Ok(peer) if peer_allowed(peer) => {}
            _ => return Err(ClientError::NotRunning(format!("127.0.0.1:{port} did not answer from 127.0.0.1"))),
        }
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(ANSWER_WAIT));
        let _ = stream.set_write_timeout(Some(CONNECT_WAIT));

        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        let hex = key.to_hex();
        let request = RequestOut { key: &hex, id, tool: tool.name(), args };
        let mut line = serde_json::to_string(&request).map_err(|e| ClientError::BadAnswer(format!("the request could not be encoded: {e}")))?;
        line.push('\n');

        let lost = |what: String| {
            if tool.is_read_only() { ClientError::NotRunning(format!("{what}; nothing was changed")) } else { ClientError::OutcomeUnknown(what) }
        };
        let mut out = &stream;
        out.write_all(line.as_bytes()).and_then(|()| out.flush()).map_err(|e| lost(format!("the request could not be sent: {e}")))?;
        let mut reader = BufReader::new(&stream);
        let text = match read_line(&mut reader, MAX_ANSWER_LINE) {
            Ok(Line::Text(t)) => t,
            Ok(Line::Eof) => return Err(lost("MailALM closed the connection without answering".into())),
            Ok(Line::TooLong) => return Err(ClientError::BadAnswer(format!("longer than {MAX_ANSWER_LINE} bytes"))),
            Ok(Line::NotUtf8) => return Err(ClientError::BadAnswer("not UTF-8".into())),
            Err(e) => return Err(lost(format!("no answer: {e}"))),
        };
        let answer: Answer = serde_json::from_str(&text).map_err(|e| ClientError::BadAnswer(e.to_string()))?;
        if answer.id != id && answer.id != 0 {
            return Err(ClientError::BadAnswer(format!("an answer to request {} instead of {id}", answer.id)));
        }
        answer.into_result().map_err(ClientError::Tool)
    }
}
