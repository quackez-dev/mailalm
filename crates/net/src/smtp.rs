// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/smtp.rs. MIT OR Apache-2.0.
// Changed: password sign-in only (AUTH PLAIN or LOGIN), always over TLS that lettre checks and MailALM's verifier checks again; no NTLM, XOAUTH2, clear text or certificate trusted by hand; no Gmail app-password message.

//! Message submission (RFC 6409) through `lettre`: implicit TLS (RFC 8314, port 465) or STARTTLS
//! (port 587), required either way. The password goes only once the connection is encrypted and
//! the server's certificates have passed both checks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::{Credentials, DEFAULT_MECHANISMS};
use lettre::transport::smtp::client::{Certificate, SmtpConnection, TlsParameters};
use lettre::transport::smtp::commands::{Data, Mail, Rcpt};
use lettre::transport::smtp::extension::{ClientId, Extension, MailBodyParameter, MailParameter};
use mailalm_model::{Security, Server};
use rustls::pki_types::CertificateDer;

use crate::stream::trust::{self, Distrusting};
use crate::{NetError, Result, clip};

/// How long the server may take to answer.
const TIMEOUT: Duration = Duration::from_secs(60);

/// How long it may take over a message it's been sent (RFC 5321 §4.5.3.2.6: ten minutes).
const SENT_TIMEOUT: Duration = Duration::from_secs(600);

/// The most of a server's addresses tried.
const MAX_ADDRESSES: usize = 8;

/// Implicit TLS or required STARTTLS as `server.security` says; `AUTH PLAIN` or `LOGIN` only
/// over TLS. Submit `raw` (an RFC 5322 message) from `from` to `recipients`.
pub fn send(server: &Server, login: &str, password: &str, from: &str, recipients: &[String], raw: &[u8]) -> Result<()> {
    let from: Address = from.trim().parse().map_err(|e| NetError::Invalid(format!("sender {}: {e}", clip(from))))?;
    let mut to = Vec::with_capacity(recipients.len());
    for r in recipients {
        to.push(r.trim().parse::<Address>().map_err(|e| NetError::Invalid(format!("recipient {}: {e}", clip(r))))?);
    }
    if to.is_empty() {
        return Err(NetError::Invalid("no recipients".into()));
    }
    let envelope = Envelope::new(Some(from), to).map_err(|e| NetError::Invalid(e.to_string()))?;
    let verifier = trust::system_verifier().map_err(NetError::Tls)?;
    let sign_in = SignIn { login, password };
    submit(server, &sign_in, &envelope, raw, recipients, trust::system_roots(), &verifier)
}

/// Who signs in. No `Debug`: the password is never printed.
struct SignIn<'a> {
    login: &'a str,
    password: &'a str,
}

/// One message over one connection, as lettre's `SmtpTransport` sends it (connect and EHLO,
/// STARTTLS, AUTH, MAIL / RCPT / DATA, QUIT), with the server's certificates checked again by
/// `verifier` between the handshake and the sign-in: lettre's own check doesn't look at what the
/// system distrusts, and only so are a server or intermediate certificate Windows distrusts
/// refused before the password goes out.
fn submit(
    server: &Server,
    sign_in: &SignIn<'_>,
    envelope: &Envelope,
    raw: &[u8],
    recipients: &[String],
    roots: &[CertificateDer<'static>],
    verifier: &Distrusting,
) -> Result<()> {
    let failed = |e: lettre::transport::smtp::Error| failure(e, recipients);
    let host = server.host.as_str();
    // lettre checks the certificate against Mozilla's roots and the system's (`roots`).
    let tls = tls_parameters(host, roots)?;
    // Each of the server's addresses in turn, as lettre would try them, greeted with this
    // computer's own address on the way to it.
    let addrs: Vec<SocketAddr> = (host, server.port)
        .to_socket_addrs()
        .map_err(|e| NetError::Io(format!("no server called {host} could be found ({e})")))?
        .take(MAX_ADDRESSES)
        .collect();
    let implicit = match server.security {
        Security::Tls => Some(&tls),
        Security::StartTls => None,
    };
    let mut tried = None;
    let mut connected = None;
    for addr in addrs {
        let hello = hello_for(addr);
        match SmtpConnection::connect(addr, Some(TIMEOUT), &hello, implicit, None) {
            Ok(c) => {
                connected = Some((c, hello));
                break;
            }
            Err(e) => tried = Some(e),
        }
    }
    let Some((mut conn, hello)) = connected else {
        return Err(match tried {
            Some(e) => failed(e),
            None => NetError::Io(format!("{host}: no address")),
        });
    };
    // STARTTLS, required: lettre refuses a server that doesn't offer it, and data the server
    // slipped in after its go-ahead.
    if server.security == Security::StartTls
        && let Err(e) = conn.starttls(&tls, &hello)
    {
        conn.abort();
        return Err(NetError::Tls(clip(&e.to_string())));
    }
    if !conn.is_encrypted() {
        conn.abort();
        return Err(NetError::Tls("the connection to the outgoing server is not encrypted".into()));
    }
    // The certificates again, as every connection's are checked (Windows' distrusted ones too).
    let checked = conn
        .certificate_chain()
        .map_err(|e| NetError::Tls(clip(&e.to_string())))
        .and_then(|chain| trust::check_chain(verifier, &chain, host).map_err(|e| NetError::Tls(clip(&e.to_string()))));
    if let Err(e) = checked {
        conn.abort();
        return Err(e);
    }
    if let Err(e) = authenticate(&mut conn, sign_in, recipients) {
        conn.abort();
        return Err(e);
    }
    let sent = hand_over(&mut conn, envelope, raw, recipients);
    conn.abort();
    sent
}

/// Sign in with the password, `PLAIN` or `LOGIN` (lettre's), once the connection is secured. A
/// server that offers neither is not given the password some other way.
fn authenticate(conn: &mut SmtpConnection, sign_in: &SignIn<'_>, recipients: &[String]) -> Result<()> {
    let info = conn.server_info();
    if !DEFAULT_MECHANISMS.iter().any(|m| info.supports_auth_mechanism(*m)) {
        return Err(NetError::Auth("the outgoing server offers no password sign-in (AUTH PLAIN or LOGIN)".into()));
    }
    let credentials = Credentials::new(sign_in.login.to_string(), sign_in.password.to_string());
    conn.auth(DEFAULT_MECHANISMS, &credentials).map(drop).map_err(|e| failure(e, recipients).without(&[sign_in.password]))
}

/// MAIL, RCPT, DATA and the message, as lettre's `SmtpConnection::send` goes through them, one
/// at a time so it's known how far it got. Up to DATA nothing is sent, and a failure can be
/// tried again. Once the message itself has gone, only the server's answer says what became of
/// it: with none (it stopped answering, the connection dropped), the message may be on its way,
/// and trying again by itself is how it arrives twice.
fn hand_over(conn: &mut SmtpConnection, envelope: &Envelope, raw: &[u8], recipients: &[String]) -> Result<()> {
    let failed = |e: lettre::transport::smtp::Error| failure(e, recipients);
    let lacks = |what: &str| NetError::Server(format!("the server does not support {what}"));
    let mut options = Vec::new();
    let ascii = |a: &Address| AsRef::<str>::as_ref(a).is_ascii();
    if !envelope.from().is_none_or(ascii) || !envelope.to().iter().all(ascii) {
        if !conn.server_info().supports_feature(Extension::SmtpUtfEight) {
            return Err(lacks("addresses outside ASCII (SMTPUTF8)"));
        }
        options.push(MailParameter::SmtpUtfEight);
    }
    if !raw.is_ascii() {
        if !conn.server_info().supports_feature(Extension::EightBitMime) {
            return Err(lacks("text outside ASCII (8BITMIME)"));
        }
        options.push(MailParameter::Body(MailBodyParameter::EightBitMime));
    }
    // Turned away at MAIL, it's the sender the server refuses: said, with the address.
    conn.command(Mail::new(envelope.from().cloned(), options)).map_err(|e| match failed(e) {
        NetError::Server(msg) => match envelope.from() {
            Some(from) => NetError::Server(format!("the server refused the sender, {from}: {msg}")),
            None => NetError::Server(msg),
        },
        other => other,
    })?;
    for to in envelope.to() {
        conn.command(Rcpt::new(to.clone(), Vec::new())).map_err(failed)?;
    }
    conn.command(Data).map_err(failed)?;
    // The server may check the whole message before it answers (RFC 5321 §4.5.3.2.6: wait ten
    // minutes).
    conn.set_timeout(Some(SENT_TIMEOUT))?;
    match conn.message(raw) {
        Ok(_) => Ok(()),
        // An answer, whatever it says, is the server's word on the message.
        Err(e) if e.status().is_some() => Err(failed(e)),
        Err(e) => Err(NetError::Unconfirmed(clip(&e.to_string()))),
    }
}

/// What MailALM calls itself in its EHLO to `server` (RFC 5321 §4.1.4): the address literal of
/// this computer's address on the way there, as Thunderbird sends, since a desktop has no name
/// the world knows. lettre's own, without a host name, is `[127.0.0.1]`, and some providers'
/// spam filters drop mail greeted so after taking it (MailCraft's issue #50). The address is the
/// one the system would send from: a socket "connected" without sending anything says it.
fn hello_for(server: SocketAddr) -> ClientId {
    let any: SocketAddr = if server.is_ipv4() { (Ipv4Addr::UNSPECIFIED, 0).into() } else { (Ipv6Addr::UNSPECIFIED, 0).into() };
    let local = UdpSocket::bind(any).and_then(|s| {
        s.connect(server)?;
        s.local_addr()
    });
    match local.map(|a| a.ip()) {
        Ok(IpAddr::V4(ip)) if !ip.is_unspecified() => ClientId::Ipv4(ip),
        Ok(IpAddr::V6(ip)) if !ip.is_unspecified() => ip.to_ipv4_mapped().map_or(ClientId::Ipv6(ip), ClientId::Ipv4),
        // No way to the server: the connection fails anyway.
        _ => ClientId::default(),
    }
}

/// What a failure from lettre means: a refusal (the sign-in's or the message's), or no answer at
/// all (no network, a refused connection, a TLS failure, a timeout).
fn failure(e: lettre::transport::smtp::Error, recipients: &[String]) -> NetError {
    let msg = clip(&e.to_string());
    if e.is_permanent() {
        let code = e.status().and_then(|c| c.to_string().parse::<u16>().ok()).unwrap_or(550);
        refusal(code, &msg, recipients)
    } else if e.is_transient() {
        // 4xx: the server asks to try again later.
        NetError::Server(msg)
    } else if e.is_tls() {
        NetError::Tls(msg)
    } else {
        NetError::Io(msg)
    }
}

/// TLS for `host`: lettre's bundled Mozilla roots and `roots` (the system's, which IMAP trusts
/// too), lettre's own check left on. lettre fails the whole build on a root it can't read, so
/// `roots` holds only usable ones ([`trust::system_roots`]).
fn tls_parameters(host: &str, roots: &[CertificateDer<'static>]) -> Result<TlsParameters> {
    let mut p = TlsParameters::builder(host.to_string());
    for der in roots {
        if let Ok(c) = Certificate::from_der(der.to_vec()) {
            p = p.add_root_certificate(c);
        }
    }
    p.build_rustls().map_err(|e| NetError::Tls(e.to_string()))
}

/// What a 5xx reply means, by its code (RFC 4954 §6, RFC 3463), never by words in its text
/// ("553 5.7.1 sender not owned by authenticated user" is a refusal of the message):
/// - 530 / 534 / 535 / 538, or the enhanced codes 5.7.8 (bad credentials) and 5.7.9 (mechanism
///   too weak): the sign-in was refused.
/// - anything else (550/551/552/553/554 on MAIL, RCPT or DATA): this message is refused,
///   naming the recipients the server's text names.
fn refusal(code: u16, msg: &str, recipients: &[String]) -> NetError {
    // Enhanced status codes stand alone in the text, maybe wrapped in `( )` or followed by `:`.
    let says = |status: &str| msg.split(|c: char| c.is_whitespace() || ":()".contains(c)).any(|token| token == status);
    if matches!(code, 530 | 534 | 535 | 538) || says("5.7.8") || says("5.7.9") {
        return NetError::Auth(msg.to_string());
    }
    let lower = msg.to_ascii_lowercase();
    let named: Vec<String> =
        recipients.iter().map(|r| r.trim().to_ascii_lowercase()).filter(|r| !r.is_empty() && lower.contains(r.as_str())).collect();
    if named.is_empty() { NetError::Server(msg.to_string()) } else { NetError::Server(format!("{} refused: {msg}", named.join(", "))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::trust::test_certs::{internal_ca, unreadable};
    use crate::stream::trust::{Distrusted, usable_roots};

    /// A refusal is classified by its reply code, not by the word "auth" in its text.
    #[test]
    fn refusals_are_classified_by_code() {
        let to = ["bob@y.example".to_string(), "ann@y.example".to_string()];
        let class = |code: u16, text: &str| refusal(code, text, &to);
        // Sign-in refusals.
        for (code, text) in [
            (535, "5.7.8 Username and Password not accepted"),
            (534, "5.7.9 Please log in with your web browser"),
            (530, "5.7.0 Authentication required"),
            (550, "5.7.8 authentication credentials invalid"),
        ] {
            let e = class(code, text);
            assert!(e.is_auth(), "{code} {text}: {e:?}");
        }
        // The message refused.
        for (code, text) in [
            (553, "5.7.1 sender not owned by authenticated user"),
            (554, "5.7.1 relay access denied: authentication needed for relaying elsewhere"),
            (552, "5.3.4 message too big"),
        ] {
            let e = class(code, text);
            assert!(matches!(e, NetError::Server(_)), "{code} {text}: {e:?}");
        }
        // The refused recipient is named when the server says which.
        let e = class(550, "5.1.1 <Ann@y.example>: Recipient address rejected: User unknown");
        assert!(matches!(&e, NetError::Server(m) if m.contains("ann@y.example") && !m.contains("bob@")), "{e:?}");
    }

    /// Nothing goes to the network for an address that isn't one: refused first.
    #[test]
    fn rejects_bad_addresses_before_connecting() {
        let server = Server::new("127.0.0.1", 1, Security::Tls);
        assert!(matches!(send(&server, "a@b.c", "pw", "not an address", &["a@b.c".into()], b""), Err(NetError::Invalid(_))));
        assert!(matches!(send(&server, "a@b.c", "pw", "a@b.c", &[], b""), Err(NetError::Invalid(_))));
        assert!(matches!(send(&server, "a@b.c", "pw", "a@b.c", &["a@b.c".into(), "nope".into()], b""), Err(NetError::Invalid(_))));
    }

    /// The system's roots reach SMTP's TLS, and one the store holds but rustls can't read
    /// doesn't stop sending: given to lettre as it is, it failed every message to every server.
    #[test]
    fn an_unreadable_root_in_the_store_does_not_stop_sending() {
        let store = vec![unreadable(), internal_ca()];
        assert!(tls_parameters("mail.corp.test", &usable_roots(store.clone(), &Distrusted::new())).is_ok());
        assert!(tls_parameters("mail.corp.test", &[]).is_ok());
        // Why the store is sorted first: lettre refuses the lot.
        assert!(tls_parameters("mail.corp.test", &store).is_err());
    }
}
