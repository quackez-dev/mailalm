// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/lib.rs. MIT OR Apache-2.0.
// Changed: only IMAP, SMTP and TLS are left; the error type is reduced to what they need, holds text only, and can blank out a secret.

//! MailALM's network layer, all of it blocking (it runs on a worker thread):
//!
//! * [`ImapSession`]: a small, strict IMAP4rev1 client (RFC 3501) on top of `imap-proto`'s parser.
//!   Server input is hostile: line, literal and per-command sizes are capped.
//! * [`smtp::send`]: message submission through `lettre` (RFC 6409, implicit TLS per RFC 8314, or
//!   STARTTLS, required).
//!
//! Every connection is TLS, checked against Mozilla's roots and the system's, less what Windows
//! distrusts. There is no exception path: no certificate trusted by hand, no clear text. A password
//! goes only over such a connection, and never into an error message, a log line or a `Debug`.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod imap;
pub mod smtp;
mod stream;

pub use imap::{Fetched, ImapSession, Mailbox};

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// The network failed: no route, no such server, a timeout, a dropped connection.
    #[error("network: {0}")]
    Io(String),
    /// The connection could not be secured: certificate refused, handshake failed, STARTTLS not
    /// offered or tampered with.
    #[error("secure connection: {0}")]
    Tls(String),
    /// The server refused the sign-in (wrong user name or password, or no method MailALM uses).
    #[error("sign-in failed: {0}")]
    Auth(String),
    /// The server refused a command (IMAP `NO` or `BAD`, an SMTP 4xx or 5xx reply).
    #[error("the server refused: {0}")]
    Server(String),
    /// The server's response could not be understood.
    #[error("unexpected server response: {0}")]
    Protocol(String),
    /// The server sent more than MailALM takes (a line, a literal, a command's responses), or a
    /// message is too large to send.
    #[error("too large: {0}")]
    TooLarge(String),
    /// The server closed the connection (`BYE` or end of stream).
    #[error("the server closed the connection: {0}")]
    Closed(String),
    /// What MailALM was asked to send is not valid (an address, a flag): nothing was sent.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The whole message went to the SMTP server, which never said whether it took it: it may have
    /// been sent. Never to be sent again without the user saying so (that is how mail arrives twice).
    #[error("the server didn't say whether it took the message ({0}): it may have been sent")]
    Unconfirmed(String),
}

impl From<std::io::Error> for NetError {
    fn from(e: std::io::Error) -> Self {
        NetError::Io(e.to_string())
    }
}

impl NetError {
    /// Whether the server refused the sign-in: the password (or user name) is to be asked again.
    pub fn is_auth(&self) -> bool {
        matches!(self, NetError::Auth(_))
    }

    /// Whether the connection is unusable after this error: the session is to be dropped and
    /// opened again.
    pub fn is_fatal(&self) -> bool {
        matches!(self, NetError::Io(_) | NetError::Tls(_) | NetError::Closed(_) | NetError::Protocol(_) | NetError::TooLarge(_))
    }

    /// The error with each of `secrets` (three characters or more) blanked out of its text: a
    /// server that echoes what it was sent never puts the password in an error message.
    pub(crate) fn without(self, secrets: &[&str]) -> NetError {
        let clean = |mut text: String| {
            for secret in secrets {
                if secret.len() >= 3 && text.contains(*secret) {
                    text = text.replace(*secret, "***");
                }
            }
            text
        };
        match self {
            NetError::Io(t) => NetError::Io(clean(t)),
            NetError::Tls(t) => NetError::Tls(clean(t)),
            NetError::Auth(t) => NetError::Auth(clean(t)),
            NetError::Server(t) => NetError::Server(clean(t)),
            NetError::Protocol(t) => NetError::Protocol(clean(t)),
            NetError::TooLarge(t) => NetError::TooLarge(clean(t)),
            NetError::Closed(t) => NetError::Closed(clean(t)),
            NetError::Invalid(t) => NetError::Invalid(clean(t)),
            NetError::Unconfirmed(t) => NetError::Unconfirmed(clean(t)),
        }
    }
}

pub type Result<T> = std::result::Result<T, NetError>;

/// A server's words cut to a length an error message can hold, control characters (a line end, a
/// terminal escape) turned into spaces.
pub(crate) fn clip(text: &str) -> String {
    const MAX: usize = 300;
    let text = text.trim();
    let mut out: String = text.chars().take(MAX).map(|c| if c.is_control() { ' ' } else { c }).collect();
    if text.chars().nth(MAX).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_say_what_they_are() {
        assert!(NetError::Auth("no".into()).is_auth());
        assert!(!NetError::Server("no".into()).is_auth());
        assert!(NetError::TooLarge("x".into()).is_fatal() && !NetError::Server("x".into()).is_fatal());
        let e = NetError::Server("LOGIN \"zoe\" \"hunter22\" refused".into()).without(&["hunter22", ""]);
        assert_eq!(e.to_string(), "the server refused: LOGIN \"zoe\" \"***\" refused");
    }

    #[test]
    fn server_words_are_clipped() {
        assert_eq!(clip("  a\r\nb\x1b[31m "), "a  b [31m");
        let long = "é".repeat(400);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), 301);
        assert!(clipped.ends_with('…'));
    }
}
