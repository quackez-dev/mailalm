// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/imap/sign_in.rs (with `encode` from crates/model/src/encoding/base64.rs). MIT OR Apache-2.0.
// Changed: password only (no NTLM, no XOAUTH2), no ENABLE QRESYNC or CONDSTORE after it; the password and the PLAIN token are blanked out of any error.

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
        let token = (self.has("AUTH=PLAIN") && self.has("SASL-IR")).then(|| base64(format!("\0{user}\0{pass}").as_bytes()));
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

/// `data` in base64 (RFC 4648 §4): the standard alphabet, padded with `=`, on one line.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3).saturating_mul(4));
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        // A chunk of k bytes gives k + 1 characters; `=` fills the group of four.
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    /// RFC 4648 §10's test vectors, and every byte value.
    #[test]
    fn base64_rfc_vectors() {
        for (plain, coded) in
            [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")]
        {
            assert_eq!(base64(plain.as_bytes()), coded, "{plain}");
        }
        assert_eq!(base64(b"\0user\0pass"), "AHVzZXIAcGFzcw==");
        assert_eq!(base64(&[0xFB, 0xFF, 0xBF]), "+/+/");
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64(&all).len(), 344);
    }
}
