// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/stream.rs. MIT OR Apache-2.0.
// Changed: one strict TLS check for every connection (no certificate trusted by hand), no IDLE timeouts, no "internet only" connections for web pictures; the STARTTLS check for early data is done by the IMAP client.

//! Connections: TCP with timeouts, wrapped in TLS checked against Mozilla's roots and the
//! system's, less what the system distrusts ([`trust`]).

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::{NetError, Result};

pub(crate) mod trust;

/// How long a connection may take to open.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long one read or write may wait.
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-address budget when more candidates remain. With a black-holed IPv6 route each AAAA
/// record used to cost the full [`CONNECT_TIMEOUT`] before any A record was tried.
const SHORT_TRY: Duration = Duration::from_secs(3);

/// Set the first time an IPv4 address connects after an IPv6 one of the same host didn't.
static V4_LEADS: AtomicBool = AtomicBool::new(false);

/// A connection: in the clear only until STARTTLS, then TLS.
pub(crate) enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Stream {
    /// Whether the connection is encrypted.
    pub(crate) fn is_tls(&self) -> bool {
        matches!(self, Stream::Tls(_))
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// What every TLS connection trusts: the bundled Mozilla roots and the system's, less what the
/// system distrusts ([`trust`]). SMTP's lettre checks the same way ([`trust::check_chain`]).
fn tls_config() -> Result<Arc<ClientConfig>> {
    static CONFIG: OnceLock<std::result::Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG.get_or_init(|| trust::system_verifier().and_then(trust::client_config)).clone().map_err(NetError::Tls)
}

/// Interleave address families (RFC 8305 §4): sort by each address's rank among those of its
/// own family, ties going to the leading family, which is the resolver's first unless
/// `prefer_v4`. Order within a family is kept.
fn attempt_order(addrs: Vec<SocketAddr>, prefer_v4: bool) -> Vec<SocketAddr> {
    let lead_is_v6 = match addrs.first() {
        Some(a) => a.is_ipv6() && !prefer_v4,
        None => return addrs,
    };
    let mut count = [0usize; 2];
    let mut ranked: Vec<(usize, bool, SocketAddr)> = addrs
        .into_iter()
        .map(|a| {
            let fam = usize::from(a.is_ipv6());
            let rank = count[fam];
            count[fam] += 1;
            (rank, a.is_ipv6() != lead_is_v6, a)
        })
        .collect();
    ranked.sort_by_key(|(rank, trailing, _)| (*rank, *trailing));
    ranked.into_iter().map(|(_, _, a)| a).collect()
}

/// Open a TCP connection, trying each resolved address in turn, with read and write timeouts.
pub(crate) fn tcp(host: &str, port: u16) -> Result<TcpStream> {
    // A name that doesn't resolve is almost always a mistyped or guessed server name.
    let addrs = (host, port).to_socket_addrs().map_err(|e| {
        NetError::Io(format!("no server called {host} could be found: check the server name, and that this computer is online ({e})"))
    })?;
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address"));
    let found: Vec<SocketAddr> = addrs.take(64).collect();
    let candidates = attempt_order(found, V4_LEADS.load(Ordering::Relaxed));
    let mut v6_missed = false;
    let mut left = candidates.len();
    for a in &candidates {
        left -= 1;
        let budget = if left == 0 { CONNECT_TIMEOUT } else { SHORT_TRY };
        match TcpStream::connect_timeout(a, budget) {
            Ok(s) => {
                if v6_missed && a.is_ipv4() {
                    V4_LEADS.store(true, Ordering::Relaxed);
                }
                s.set_read_timeout(Some(IO_TIMEOUT))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => {
                v6_missed |= a.is_ipv6();
                last = e;
            }
        }
    }
    Err(NetError::Io(format!("{host}:{port}: {last}")))
}

/// Wrap a TCP connection in TLS, verifying the certificate against `host`. The handshake is
/// finished here, so a certificate problem is a TLS error, not a later I/O one.
pub(crate) fn tls(sock: TcpStream, host: &str) -> Result<Stream> {
    let name = ServerName::try_from(host.to_string()).map_err(|e| NetError::Tls(format!("{host}: {e}")))?;
    let conn = ClientConnection::new(tls_config()?, name).map_err(|e| NetError::Tls(e.to_string()))?;
    let mut s = StreamOwned::new(conn, sock);
    while s.conn.is_handshaking() {
        if let Err(e) = s.conn.complete_io(&mut s.sock) {
            return Err(NetError::Tls(handshake_failed(host, &e)));
        }
    }
    Ok(Stream::Tls(Box::new(s)))
}

/// The STARTTLS upgrade once the server has agreed (and the client has checked that nothing came
/// after its go-ahead): `plain`'s connection wrapped in TLS as [`tls`] does.
pub(crate) fn starttls(plain: Stream, host: &str) -> Result<Stream> {
    let Stream::Plain(sock) = plain else {
        return Err(NetError::Tls("connection already encrypted".into()));
    };
    tls(sock, host)
}

/// What a handshake that failed says. A server that hangs up on the hello, or answers that they
/// have no encryption in common, most likely offers only what's older than MailALM uses (TLS 1.0
/// or 1.1, or TLS 1.2 with CBC ciphers alone): said so, with the server's name.
fn handshake_failed(host: &str, e: &io::Error) -> String {
    use rustls::{AlertDescription as A, Error as E};
    let hung_up = matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted | io::ErrorKind::UnexpectedEof);
    let refused = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<E>())
        .is_some_and(|r| matches!(r, E::AlertReceived(A::HandshakeFailure | A::ProtocolVersion | A::InsufficientSecurity) | E::PeerIncompatible(_)));
    if !(hung_up || refused) {
        return e.to_string();
    }
    log::warn!("{host}: the secure connection failed as it began: {e}");
    format!(
        "{host} ended the connection as it was being secured ({e}): it may offer only older encryption than MailALM \
         uses (TLS 1.0 or 1.1, or TLS 1.2 without an ECDHE cipher with AES-GCM or ChaCha20)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_attempts_alternate_ip_versions() {
        let p = |s: &str| -> SocketAddr { s.parse().unwrap() };
        let (x1, x2, x3) = (p("198.51.100.7:993"), p("198.51.100.8:993"), p("198.51.100.9:993"));
        let (y1, y2) = (p("[2001:db8:0:1::7]:993"), p("[2001:db8:0:1::8]:993"));
        // Resolver led with AAAA: v6, v4, v6, v4, then the spare v4.
        assert_eq!(attempt_order(vec![y1, y2, x1, x2, x3], false), vec![y1, x1, y2, x2, x3]);
        // After a v6 miss, A records lead.
        assert_eq!(attempt_order(vec![y1, y2, x1, x2, x3], true), vec![x1, y1, x2, y2, x3]);
        // Resolver led with A: unchanged lead.
        assert_eq!(attempt_order(vec![x1, y1, y2], false), vec![x1, y1, y2]);
        assert_eq!(attempt_order(vec![], true), Vec::<SocketAddr>::new());
    }
}
