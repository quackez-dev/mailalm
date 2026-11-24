// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/imap/sign_in.rs. MIT OR Apache-2.0.
// Changed: password only (no NTLM, no XOAUTH2), no ENABLE QRESYNC or CONDSTORE after it; the password and the PLAIN token are blanked out of any error; the base64 encoder is in src/base64.rs.

//! Signing in with a password: `AUTHENTICATE PLAIN` with SASL-IR when offered, else `LOGIN`.
//! Only ever over TLS ([`super::ImapSession::connect`]).

use std::io::{Read, Write};

use imap_proto::{Response, ResponseCode};

use super::{Client, Part, caps_from, quoted};
use crate::{NetError, Result};

/// A sign-in the server refused (`NO`, `BAD`) as a refused sign-in, in the server's words, or in
/// `unsaid` when it gave none.
fn auth_error(e: NetError, unsaid: &str) -> NetError {
    match e {
        NetError::Server(m) => NetError::Auth(if m.is_empty() { unsaid.into() } else { m }),
        other => other,
    }
}

impl<S: Read + Write> Client<S> {
    /// The sign-in methods the server names (`AUTH=…`), for saying what it offers.
    fn offered_sign_ins(&self) -> String {
        let mut names: Vec<&str> = self.caps.iter().filter_map(|c| c.strip_prefix("AUTH=")).collect();
        // The `LOGIN` command, when it's allowed, and `AUTH=LOGIN` are the same to the user.
        if !self.has("LOGINDISABLED") {
            names.push("LOGIN");
        }
        names.sort_unstable();
        names.dedup();
        if names.is_empty() { "none".into() } else { names.join(", ") }
    }

    /// Sign in: `AUTHENTICATE PLAIN` with SASL-IR when offered, else `LOGIN`. A user name or
    /// password that isn't printable ASCII goes as a literal, sent only once the server says `+`.
    pub(super) fn login(&mut self, user: &str, pass: &str) -> Result<()> {
        let token = (self.has("AUTH=PLAIN") && self.has("SASL-IR")).then(|| crate::base64::encode(format!("\0{user}\0{pass}").as_bytes()));
        let res = if let Some(token) = &token {
            self.cmd(&[Part::Text("AUTHENTICATE PLAIN "), Part::Text(token)])
        } else if self.has("LOGINDISABLED") {
            return Err(NetError::Auth(format!("the server does not take a password with LOGIN; it offers: {}", self.offered_sign_ins())));
        } else {
            let (u, p) = (quoted(user), quoted(pass));
            let mut parts = vec![Part::Text("LOGIN ")];
            match &u {
                Some(q) => parts.push(Part::Text(q)),
                None => parts.push(Part::Literal(user.as_bytes())),
            }
            parts.push(Part::Text(" "));
            match &p {
                Some(q) => parts.push(Part::Text(q)),
                None => parts.push(Part::Literal(pass.as_bytes())),
            }
            self.cmd(&parts)
        };
        let secrets = [pass, token.as_deref().unwrap_or_default()];
        let rs = res.map_err(|e| auth_error(e, "the server rejected the user name or password").without(&secrets))?;
        self.after_login(&rs)
    }

    /// After signing in: the new capabilities, from the completion or asked for.
    fn after_login(&mut self, rs: &[Response<'static>]) -> Result<()> {
        for r in rs {
            if let Response::Done { outcome, .. } = r
                && let Some(ResponseCode::Capabilities(list)) = &outcome.code
            {
                self.caps = caps_from(list);
                return Ok(());
            }
        }
        self.capability()
    }
}
