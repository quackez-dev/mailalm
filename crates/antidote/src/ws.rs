//! A minimal WebSocket client (RFC 6455) for Antidote's local server, written for MailALM.
//!
//! * Plain TCP to the loopback only: `127.0.0.1`, then `::1` (Druide's Python connects to
//!   `localhost`, which can be either). [`WebSocket::connect`] takes a port, never a host.
//! * The handshake's `Sec-WebSocket-Accept` is checked (SHA-1 of the key and the RFC's GUID); no
//!   extension and no subprotocol is accepted.
//! * Frames from the server must be unmasked, with no reserved bit; frames from MailALM are
//!   masked with fresh random keys. Text and binary messages (both read as UTF-8, as the Python
//!   does), fragmented or not; ping answered by pong; close answered by close.
//! * A frame or a message is at most [`MAX_MESSAGE`] bytes: a larger length is refused as soon as
//!   the frame's header is read, before its payload is buffered.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};

use crate::protocol::MAX_MESSAGE;
use crate::{Error, base64, clip};

/// RFC 6455 §1.3.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HANDSHAKE: usize = 16 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long [`WebSocket::poll`] waits for data: the session thread looks at the UI's commands in
/// between.
const POLL: Duration = Duration::from_millis(50);
/// How long [`WebSocket::close`] waits for the server's close.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl Opcode {
    fn from_bits(bits: u8) -> Option<Self> {
        Some(match bits {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xA => Opcode::Pong,
            _ => return None,
        })
    }

    fn bits(self) -> u8 {
        match self {
            Opcode::Continuation => 0x0,
            Opcode::Text => 0x1,
            Opcode::Binary => 0x2,
            Opcode::Close => 0x8,
            Opcode::Ping => 0x9,
            Opcode::Pong => 0xA,
        }
    }

    fn is_control(self) -> bool {
        matches!(self, Opcode::Close | Opcode::Ping | Opcode::Pong)
    }
}

/// One frame from the server.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) fin: bool,
    pub(crate) opcode: Opcode,
    pub(crate) payload: Vec<u8>,
}

fn protocol(why: impl Into<String>) -> Error {
    Error::Protocol(why.into())
}

fn io_error(e: io::Error) -> Error {
    Error::Io(e.to_string())
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// Reads one server frame from the front of `buf`: the frame and the bytes it took, or `Ok(None)`
/// while it is incomplete. A length over `max_payload` is refused from the header alone.
pub(crate) fn parse_frame(buf: &[u8], max_payload: usize) -> Result<Option<(Frame, usize)>, Error> {
    let (Some(&b0), Some(&b1)) = (buf.first(), buf.get(1)) else { return Ok(None) };
    if b0 & 0x70 != 0 {
        return Err(protocol("a frame with reserved bits set"));
    }
    let opcode = Opcode::from_bits(b0 & 0x0F).ok_or_else(|| protocol(format!("unknown opcode {:#x}", b0 & 0x0F)))?;
    let fin = b0 & 0x80 != 0;
    if b1 & 0x80 != 0 {
        return Err(protocol("a masked frame from the server"));
    }
    let (len, header) = match b1 & 0x7F {
        126 => {
            let Some(bytes) = buf.get(2..4).and_then(|b| <[u8; 2]>::try_from(b).ok()) else { return Ok(None) };
            (u64::from(u16::from_be_bytes(bytes)), 4)
        }
        127 => {
            let Some(bytes) = buf.get(2..10).and_then(|b| <[u8; 8]>::try_from(b).ok()) else { return Ok(None) };
            let len = u64::from_be_bytes(bytes);
            if len >> 63 != 0 {
                return Err(protocol("a frame length with its top bit set"));
            }
            (len, 10)
        }
        len => (u64::from(len), 2),
    };
    if opcode.is_control() && (!fin || len > 125) {
        return Err(protocol("a fragmented or long control frame"));
    }
    let len = usize::try_from(len)
        .ok()
        .filter(|l| *l <= max_payload)
        .ok_or_else(|| protocol(format!("a frame of {len} bytes, over the limit of {max_payload}")))?;
    let Some(payload) = buf.get(header..header + len) else { return Ok(None) };
    Ok(Some((Frame { fin, opcode, payload: payload.to_vec() }, header + len)))
}

/// A whole, masked frame from the client (RFC 6455 §5.3).
pub(crate) fn encode_frame(opcode: Opcode, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len().saturating_add(14));
    out.push(0x80 | opcode.bits());
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if let Ok(len) = u16::try_from(len) {
        out.push(0x80 | 126);
        out.extend_from_slice(&len.to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    out
}

/// What a frame means once fragments are put together.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Message(String),
    Ping(Vec<u8>),
    /// The server closes, with this status code if it gave one.
    Close(Option<u16>),
}

/// Puts fragmented messages together, at most [`MAX_MESSAGE`] bytes.
#[derive(Default)]
pub(crate) struct Assembler {
    partial: Option<Vec<u8>>,
}

impl Assembler {
    pub(crate) fn on_frame(&mut self, frame: Frame) -> Result<Option<Event>, Error> {
        match frame.opcode {
            Opcode::Text | Opcode::Binary => {
                if self.partial.is_some() {
                    return Err(protocol("a new message inside a fragmented one"));
                }
                if frame.fin {
                    return utf8(frame.payload).map(Some);
                }
                self.partial = Some(frame.payload);
                Ok(None)
            }
            Opcode::Continuation => {
                let Some(partial) = self.partial.as_mut() else {
                    return Err(protocol("a continuation frame outside a message"));
                };
                if partial.len().saturating_add(frame.payload.len()) > MAX_MESSAGE {
                    return Err(protocol(format!("a message of more than {MAX_MESSAGE} bytes")));
                }
                partial.extend_from_slice(&frame.payload);
                if !frame.fin {
                    return Ok(None);
                }
                utf8(self.partial.take().unwrap_or_default()).map(Some)
            }
            Opcode::Ping => Ok(Some(Event::Ping(frame.payload))),
            Opcode::Pong => Ok(None),
            Opcode::Close => close_code(&frame.payload).map(|code| Some(Event::Close(code))),
        }
    }
}

fn utf8(bytes: Vec<u8>) -> Result<Event, Error> {
    String::from_utf8(bytes).map(Event::Message).map_err(|_| protocol("a message that is not UTF-8"))
}

/// The status code of a close frame (RFC 6455 §5.5.1): none, or two bytes and a UTF-8 reason.
pub(crate) fn close_code(payload: &[u8]) -> Result<Option<u16>, Error> {
    match payload {
        [] => Ok(None),
        [_] => Err(protocol("a close frame of one byte")),
        [high, low, reason @ ..] => {
            if std::str::from_utf8(reason).is_err() {
                return Err(protocol("a close reason that is not UTF-8"));
            }
            Ok(Some(u16::from_be_bytes([*high, *low])))
        }
    }
}

/// `Sec-WebSocket-Accept` for `key`: base64 of the SHA-1 of the key and the GUID (§4.2.2).
pub(crate) fn accept_for(key: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, format!("{key}{GUID}").as_bytes());
    base64::encode(digest.as_ref())
}

/// Checks the server's answer to the handshake (§4.1): `101`, `Upgrade: websocket`, `Connection:
/// Upgrade`, the right accept value, and no extension or subprotocol (none was asked for).
pub(crate) fn check_response(head: &[u8], key: &str) -> Result<(), Error> {
    let head = std::str::from_utf8(head).map_err(|_| protocol("a handshake answer that is not text"))?;
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let mut words = status.split(' ');
    if words.next() != Some("HTTP/1.1") || words.next() != Some("101") {
        return Err(protocol(format!("the WebSocket was refused: {}", clip(status))));
    }
    let (mut upgrade, mut connection, mut accept) = (false, false, None);
    for line in lines.filter(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(protocol(format!("a bad header line: {}", clip(line))));
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("upgrade") {
            upgrade = value.eq_ignore_ascii_case("websocket");
        } else if name.eq_ignore_ascii_case("connection") {
            connection = value.split(',').any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
        } else if name.eq_ignore_ascii_case("sec-websocket-accept") {
            if accept.replace(value).is_some() {
                return Err(protocol("two Sec-WebSocket-Accept headers"));
            }
        } else if (name.eq_ignore_ascii_case("sec-websocket-extensions") || name.eq_ignore_ascii_case("sec-websocket-protocol"))
            && !value.is_empty()
        {
            return Err(protocol(format!("an extension or subprotocol that was not asked for: {}", clip(line))));
        }
    }
    if !upgrade || !connection {
        return Err(protocol("a handshake answer without Upgrade: websocket"));
    }
    if accept != Some(accept_for(key).as_str()) {
        return Err(protocol("a wrong Sec-WebSocket-Accept"));
    }
    Ok(())
}

/// What [`WebSocket::poll`] got.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Incoming {
    Message(String),
    /// The server closed the WebSocket, or the connection.
    Closed,
}

/// A WebSocket to Antidote's server on this PC.
pub(crate) struct WebSocket {
    stream: TcpStream,
    /// Bytes read and not yet made into frames.
    buf: Vec<u8>,
    assembler: Assembler,
    rng: SystemRandom,
    close_sent: bool,
}

impl WebSocket {
    /// Connects to `ws://localhost:{port}/` on `127.0.0.1`, or `::1` if nothing answers there.
    pub(crate) fn connect(port: u16) -> Result<Self, Error> {
        if port == 0 {
            return Err(Error::Io("port 0".into()));
        }
        let mut last = String::new();
        for ip in [IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)] {
            match TcpStream::connect_timeout(&SocketAddr::new(ip, port), CONNECT_TIMEOUT) {
                Ok(stream) => return Self::handshake(stream, port),
                Err(e) => last = e.to_string(),
            }
        }
        Err(Error::Io(format!("nothing answers on port {port} of this PC: {last}")))
    }

    fn handshake(mut stream: TcpStream, port: u16) -> Result<Self, Error> {
        if !stream.peer_addr().map_err(io_error)?.ip().is_loopback() {
            return Err(Error::Io("not a connection to this PC".into()));
        }
        // Small messages both ways: no need to wait for more to send.
        let _ = stream.set_nodelay(true);
        stream.set_write_timeout(Some(WRITE_TIMEOUT)).map_err(io_error)?;
        let rng = SystemRandom::new();
        let mut nonce = [0u8; 16];
        rng.fill(&mut nonce).map_err(|_| Error::Io("no random numbers".into()))?;
        let key = base64::encode(&nonce);
        // The headers websocket-client (what Druide's Python uses) sends for `ws://localhost:PORT`.
        let request = format!(
            "GET / HTTP/1.1\r\nHost: localhost:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nOrigin: http://localhost:{port}\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).map_err(io_error)?;

        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut buf = Vec::new();
        let end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            if buf.len() > MAX_HANDSHAKE {
                return Err(protocol("a handshake answer that is too long"));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Io("no answer to the WebSocket handshake".into()));
            }
            stream.set_read_timeout(Some(left)).map_err(io_error)?;
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => return Err(Error::Io("closed during the WebSocket handshake".into())),
                Ok(n) => buf.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if is_timeout(&e) => return Err(Error::Io("no answer to the WebSocket handshake".into())),
                Err(e) => return Err(io_error(e)),
            }
        };
        check_response(buf.get(..end).unwrap_or_default(), &key)?;
        // Frames may have come right behind the answer.
        let rest = buf.split_off(end);
        stream.set_read_timeout(Some(POLL)).map_err(io_error)?;
        Ok(Self { stream, buf: rest, assembler: Assembler::default(), rng, close_sent: false })
    }

    pub(crate) fn send_text(&mut self, text: &str) -> Result<(), Error> {
        self.send(Opcode::Text, text.as_bytes())
    }

    fn send(&mut self, opcode: Opcode, payload: &[u8]) -> Result<(), Error> {
        let mut mask = [0u8; 4];
        self.rng.fill(&mut mask).map_err(|_| Error::Io("no random numbers".into()))?;
        self.stream.write_all(&encode_frame(opcode, payload, mask)).map_err(io_error)
    }

    /// The next message from the server, or `Ok(None)` if nothing whole came within [`POLL`].
    /// Pings are answered here.
    pub(crate) fn poll(&mut self) -> Result<Option<Incoming>, Error> {
        loop {
            while let Some((frame, used)) = parse_frame(&self.buf, MAX_MESSAGE)? {
                self.buf.drain(..used);
                match self.assembler.on_frame(frame)? {
                    None => {}
                    Some(Event::Message(text)) => return Ok(Some(Incoming::Message(text))),
                    Some(Event::Ping(payload)) => self.send(Opcode::Pong, &payload)?,
                    Some(Event::Close(code)) => {
                        self.answer_close(code);
                        return Ok(Some(Incoming::Closed));
                    }
                }
            }
            let mut chunk = [0u8; 64 * 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(Some(Incoming::Closed)),
                Ok(n) => self.buf.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                Err(e) if is_timeout(&e) => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(e)),
            }
        }
    }

    /// Echoes the server's close (§5.5.1) and drops the connection.
    fn answer_close(&mut self, code: Option<u16>) {
        if !self.close_sent {
            self.close_sent = true;
            let payload = code.map(|c| c.to_be_bytes().to_vec()).unwrap_or_default();
            let _ = self.send(Opcode::Close, &payload);
        }
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    /// Closes the WebSocket (status 1000): sends close, waits a moment for the server's, and drops
    /// the connection.
    pub(crate) fn close(&mut self) {
        if !self.close_sent {
            self.close_sent = true;
            if self.send(Opcode::Close, &1000u16.to_be_bytes()).is_ok() {
                let deadline = Instant::now() + CLOSE_WAIT;
                while Instant::now() < deadline {
                    match self.poll() {
                        Ok(Some(Incoming::Closed)) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
        }
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests;
