//! MailALM's control channel: how `mailalm-mcp` (the MCP server an agent starts) reaches the
//! running MailALM (PLAN.md, section 11; decision of 2026-11-25).
//!
//! * **Channel**: TCP on `127.0.0.1` only, one JSON line per request and one per answer, one
//!   request per connection ([`wire`]). MailALM listens only while the MCP is turned on in its
//!   settings (off by default), on a port the system picks.
//! * **Key**: at every start, MailALM writes 32 fresh random bytes (64 hexadecimal characters) to
//!   `%LOCALAPPDATA%\MailALM\control-key` and the port to `control-port` ([`key`]). Every request
//!   carries the key; it is compared in constant time, and a wrong one is refused and logged.
//!   Only peers whose address is `127.0.0.1` are served.
//! * **Server** ([`ControlServer`]): its own threads; each authenticated request goes to the app
//!   through a channel ([`Incoming`]), and the connection waits for the app's answer with a
//!   timeout. The UI thread never blocks.
//! * **Client** ([`Client`]): what `mailalm-mcp` uses.
//! * **Catalog** ([`Tool`], [`types`]): the tools, their arguments, answers and JSON schemas, shared
//!   by both sides. Mail content is always returned inside [`types::External`], marked as
//!   untrusted text.
//! * **Adapter** ([`Backend`], [`handle`], [`PendingCall`]): the app implements one method per tool;
//!   the adapter checks the arguments and the accounts hidden from the MCP, and holds sensitive
//!   actions until the user confirms them in MailALM.
//! * **Journal** ([`Journal`]): every call, kept in memory for the app to show.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod adapter;
pub mod catalog;
pub mod client;
pub mod journal;
pub mod key;
pub mod server;
pub mod types;
pub mod wire;

pub use adapter::{Backend, Call, Confirmed, PendingCall, dispatch, handle};
pub use catalog::{Kind, Tool};
pub use client::{Client, ClientError};
pub use journal::{Confirmation, Journal, JournalEntry, Outcome};
pub use key::{ControlError, Key, profile_dir};
pub use server::{ControlServer, Incoming};
pub use wire::{Answer, ErrorCode, ToolError};
