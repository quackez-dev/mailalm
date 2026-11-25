//! `mailalm-mcp`: MailALM's MCP server (PLAN.md, section 11). An agent starts it and talks to it
//! over stdio (JSON-RPC 2.0, MCP); every tool call goes to the running MailALM over its control
//! channel: TCP on 127.0.0.1 only, with the key MailALM writes to `%LOCALAPPDATA%\MailALM` while
//! its MCP access is turned on (off by default). `mailalm-mcp` itself opens no port, reads no
//! mail and keeps nothing: when MailALM is not running, or its MCP access is off, every call
//! says so.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod rpc;
mod tools;

#[cfg(test)]
mod tests;

use std::process::ExitCode;

use mailalm_control::Client;

fn main() -> ExitCode {
    if std::env::args_os().len() > 1 {
        eprintln!(
            "mailalm-mcp {}: MailALM's MCP server over stdio. It takes no arguments: register it as a stdio MCP server, \
             then turn MCP access on in MailALM's settings.",
            env!("CARGO_PKG_VERSION")
        );
        return ExitCode::from(2);
    }
    let forward: Box<dyn tools::Forward> = match Client::from_profile() {
        Ok(client) => Box::new(client),
        Err(e) => Box::new(tools::Unavailable(e.to_string())),
    };
    let mut server = rpc::Server::new(forward);
    match server.serve(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mailalm-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
