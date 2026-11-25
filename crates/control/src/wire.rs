// Adapted from WordCraft (https://github.com/storytold/wordcraft), commit ca7f262a597c75da4a157b730c68aa2db8d12734, crates/mcp/src/server.rs (read_bounded_line) and apps/wordcraft/src/control_server.rs (discard_rest_of_line). MIT OR Apache-2.0.
// Changed: the request and answer formats are MailALM's (`key`, `id`, `tool`, `args`; `ok` or a coded `error`); a line over the cap is reported without being skipped, and skipping is bounded by the caller; a line that is not UTF-8 is a value, not an I/O error.

//! The wire format: one JSON object per line, one request per connection.
//!
//! A request (client to MailALM):
//!
//! ```json
//! {"key": "<64 hex>", "id": 1, "tool": "search", "args": {"query": "invoice"}}
//! ```
//!
//! Its answer (MailALM to client), `ok` with the tool's answer, or `error`:
//!
//! ```json
//! {"id": 1, "ok": {"external_content": {"notice": "…", "data": []}, "truncated": false}}
//! {"id": 1, "error": {"code": "account_hidden", "message": "…"}}
//! ```
//!
//! A request line is at most [`MAX_REQUEST_LINE`] bytes, an answer line [`MAX_ANSWER_LINE`].

use std::fmt;
use std::io::{self, BufRead, Read};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest request line MailALM reads (4 MiB: a draft's text is the biggest argument).
pub const MAX_REQUEST_LINE: usize = 4 << 20;
/// Longest answer line the client reads (a thread's text).
pub const MAX_ANSWER_LINE: usize = 16 << 20;

/// A request as the client writes it.
#[derive(Serialize)]
pub(crate) struct RequestOut<'a> {
    pub key: &'a str,
    pub id: u64,
    pub tool: &'a str,
    pub args: &'a Value,
}

/// A request as MailALM reads it, once the key has matched.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestIn {
    #[allow(dead_code, reason = "checked before this is read; kept so that deny_unknown_fields accepts it")]
    pub key: String,
    pub id: u64,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

/// Why a request failed. The client sees the code and the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No key, or not the key of the running MailALM.
    Unauthorized,
    /// Not a request (not JSON, missing fields, not UTF-8).
    BadRequest,
    /// The request line is longer than [`MAX_REQUEST_LINE`].
    LineTooLong,
    /// No tool by that name.
    UnknownTool,
    /// The arguments do not fit the tool.
    BadArgs,
    /// The account is hidden from the MCP in MailALM's settings (or does not exist).
    AccountHidden,
    /// No such message, folder, event…
    NotFound,
    /// The account's provider cannot do it.
    NotSupported,
    /// A sensitive action reached the backend without the user's confirmation.
    NeedsConfirmation,
    /// The user said no in MailALM.
    Declined,
    /// MailALM did not take the request in time: it was not run, and never will be.
    TimeoutNotRun,
    /// MailALM started the request but did not answer in time: it may have been done.
    OutcomeUnknown,
    /// Anything else (the server's error, an offline account…).
    Failed,
}

/// A coded error with a message for the agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
}

impl ToolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ToolError {
        ToolError { code, message: message.into() }
    }

    pub fn bad_args(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::BadArgs, message)
    }

    pub fn not_found(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::NotFound, message)
    }

    pub fn not_supported(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::NotSupported, message)
    }

    pub fn failed(message: impl Into<String>) -> ToolError {
        ToolError::new(ErrorCode::Failed, message)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = serde_json::to_value(self.code).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        write!(f, "{code}: {}", self.message)
    }
}

impl std::error::Error for ToolError {}

/// One answer line. Exactly one of `ok` and `error` is set; `id` is the request's, or 0 when the
/// request could not be read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
}

impl Answer {
    pub fn ok(id: u64, value: Value) -> Answer {
        Answer { id, ok: Some(value), error: None }
    }

    pub fn error(id: u64, error: ToolError) -> Answer {
        Answer { id, ok: None, error: Some(error) }
    }

    pub fn from_result(id: u64, result: Result<Value, ToolError>) -> Answer {
        match result {
            Ok(v) => Answer::ok(id, v),
            Err(e) => Answer::error(id, e),
        }
    }

    /// The tool's answer, or its error.
    pub fn into_result(self) -> Result<Value, ToolError> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(self.ok.unwrap_or(Value::Null)),
        }
    }

    /// The answer as one line, without its line end.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(r#"{{"id":{},"error":{{"code":"failed","message":"the answer could not be encoded"}}}}"#, self.id)
        })
    }
}

/// One line read with a cap.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// The stream ended before any byte.
    Eof,
    /// A line, without its `\n` or `\r\n`. A last line without a line end counts.
    Text(String),
    /// More than the cap before a line end; the rest of the line is still in the stream.
    TooLong,
    /// Not UTF-8.
    NotUtf8,
}

/// Read one line of at most `max` bytes (line end excluded), never buffering more than `max + 1`.
pub fn read_line(reader: &mut impl BufRead, max: usize) -> io::Result<Line> {
    let mut buf = Vec::new();
    let cap = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    reader.by_ref().take(cap).read_until(b'\n', &mut buf)?;
    if buf.is_empty() {
        return Ok(Line::Eof);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    } else if buf.len() > max {
        return Ok(Line::TooLong);
    }
    Ok(String::from_utf8(buf).map_or(Line::NotUtf8, Line::Text))
}

/// Skip the rest of a line, at most `limit` bytes. True when its end was reached.
pub fn skip_line(reader: &mut impl BufRead, limit: usize) -> bool {
    let mut left = limit;
    loop {
        let (used, done) = match reader.fill_buf() {
            Ok([]) | Err(_) => return false,
            Ok(chunk) => match chunk.iter().position(|&c| c == b'\n') {
                Some(i) => (i + 1, true),
                None => (chunk.len(), false),
            },
        };
        reader.consume(used);
        if done {
            return true;
        }
        left = left.saturating_sub(used);
        if left == 0 {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lines(input: &[u8], max: usize) -> Vec<Line> {
        let mut r = io::BufReader::with_capacity(4, input);
        let mut out = Vec::new();
        loop {
            match read_line(&mut r, max).unwrap() {
                Line::Eof => return out,
                Line::TooLong => {
                    out.push(Line::TooLong);
                    skip_line(&mut r, usize::MAX);
                }
                l => out.push(l),
            }
        }
    }

    #[test]
    fn lines_are_capped() {
        let t = |s: &str| Line::Text(s.to_string());
        assert_eq!(lines(b"ab\r\ncd\n\nlast", 4), vec![t("ab"), t("cd"), t(""), t("last")]);
        assert_eq!(lines(b"abcd\nabcde\nok\n", 4), vec![t("abcd"), Line::TooLong, t("ok")]);
        assert_eq!(lines(b"\xff\xfe\nok\n", 4), vec![Line::NotUtf8, t("ok")]);
    }

    #[test]
    fn skipping_is_bounded() {
        let data = vec![b'x'; 100];
        let mut r = io::BufReader::with_capacity(8, &data[..]);
        assert!(!skip_line(&mut r, 20));
        let mut r = io::BufReader::with_capacity(8, &b"xxxx\nnext\n"[..]);
        assert!(skip_line(&mut r, 20));
        assert_eq!(read_line(&mut r, 10).unwrap(), Line::Text("next".into()));
    }

    #[test]
    fn answers_carry_ok_or_a_coded_error() {
        let ok = Answer::ok(3, json!({"a": 1}));
        assert_eq!(serde_json::to_value(&ok).unwrap(), json!({"id": 3, "ok": {"a": 1}}));
        let err = Answer::error(4, ToolError::new(ErrorCode::AccountHidden, "hidden"));
        assert_eq!(err.to_line(), r#"{"id":4,"error":{"code":"account_hidden","message":"hidden"}}"#);
        let back: Answer = serde_json::from_str(&err.to_line()).unwrap();
        assert_eq!(back.into_result().unwrap_err().code, ErrorCode::AccountHidden);
        assert_eq!(ToolError::new(ErrorCode::TimeoutNotRun, "late").to_string(), "timeout_not_run: late");
    }
}
