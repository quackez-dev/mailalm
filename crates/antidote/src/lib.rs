//! MailALM's bridge to Antidote (Druide informatique), the protocol of Druide's integration API
//! (version 2) written in Rust:
//!
//! * finding Antidote: on Windows, the registry names the folder of `AgentConnectixConsole.exe`,
//!   which is launched with `--api`, without a shell or a window, and answers the port of its
//!   local WebSocket server ([`Error::Unavailable`] on other systems);
//! * a minimal WebSocket client (RFC 6455), plain TCP to the loopback only (`127.0.0.1`, then
//!   `::1`): there is no way to name another host;
//! * the conversation: MailALM asks Antidote to open its corrector, dictionaries or guides, or to
//!   run Anti-Oups! on a message; Antidote then asks for the text and sends back its corrections.
//!
//! The editor implements [`Texteur`] on its compose buffer. A [`Session`] runs the conversation on
//! a thread of its own and hands Antidote's requests to the UI through a channel: the UI calls
//! [`Session::process`] when it is woken, which never blocks. Everything Antidote sends is checked
//! before it reaches the editor: sizes, zone ids, positions (converted from Antidote's [`Unit`] to
//! byte offsets, never inside a character), the characters of a replacement.
//!
//! No text leaves the PC: the WebSocket ends in the Antidote installed on it.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod base64;
mod locate;
mod offsets;
mod protocol;
mod session;
mod ws;

#[cfg(test)]
mod test_server;

pub use locate::installed;
pub use offsets::Unit;
pub use protocol::{AntiOupsReply, Configuration, Detection, DocumentType, StyleKind, Tool};
pub use session::{Session, Style, Texteur, Zone};

/// Why a session with Antidote could not start, or ended.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// This system has no Antidote integration MailALM knows of (it is Windows only).
    #[error("Antidote is not available on this system")]
    Unavailable,
    /// Antidote, or its AgentConnectix, is not installed where the registry says.
    #[error("Antidote is not installed: {0}")]
    NotInstalled(String),
    /// `AgentConnectixConsole --api` failed, gave no answer in time, or no port.
    #[error("Antidote's console: {0}")]
    Console(String),
    /// The connection to Antidote failed or dropped.
    #[error("connection to Antidote: {0}")]
    Io(String),
    /// Antidote sent something the protocol does not allow, or too large.
    #[error("Antidote broke the protocol: {0}")]
    Protocol(String),
}

/// `text` made safe for an error message or a log line: control characters blanked, cut short.
pub(crate) fn clip(text: &str) -> String {
    const MAX: usize = 120;
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
    fn clipped_text_has_no_control_characters() {
        assert_eq!(clip("a\r\nb\u{1b}[31m"), "a  b [31m");
        let long = "é".repeat(200);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), 121);
        assert!(clipped.ends_with('…'));
    }
}
