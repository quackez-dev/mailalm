//! A fake Antidote for the tests: a WebSocket server on 127.0.0.1 that a test drives step by step.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ws::accept_for;

/// A listener on a free port of 127.0.0.1.
pub(crate) fn listen() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// A frame from the server: first byte `b0`, unmasked.
pub(crate) fn server_frame(b0: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![b0];
    let len = payload.len();
    if len < 126 {
        out.push(len as u8);
    } else if let Ok(len) = u16::try_from(len) {
        out.push(126);
        out.extend_from_slice(&len.to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

pub(crate) struct FakeAntidote {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl FakeAntidote {
    /// Accepts MailALM's connection and answers its handshake, with `extra` right behind.
    pub(crate) fn accept(listener: &TcpListener, extra: &[u8]) -> Self {
        let (mut stream, peer) = listener.accept().unwrap();
        assert!(peer.ip().is_loopback());
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut buf = Vec::new();
        let end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0, "MailALM closed during the handshake");
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8(buf[..end].to_vec()).unwrap();
        let port = stream.local_addr().unwrap().port();
        assert!(head.starts_with("GET / HTTP/1.1\r\n"), "{head}");
        assert!(head.contains(&format!("\r\nHost: localhost:{port}\r\n")), "{head}");
        assert!(head.contains(&format!("\r\nOrigin: http://localhost:{port}\r\n")), "{head}");
        assert!(head.contains("\r\nSec-WebSocket-Version: 13\r\n"), "{head}");
        let key = head.split("\r\n").find_map(|line| line.strip_prefix("Sec-WebSocket-Key: ")).unwrap().to_string();
        assert_eq!(key.len(), 24, "16 random bytes in base64");
        let mut answer = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_for(&key)
        )
        .into_bytes();
        answer.extend_from_slice(extra);
        stream.write_all(&answer).unwrap();
        Self { stream, buf: buf[end..].to_vec() }
    }

    /// The next frame from MailALM, unmasked: its first byte and its payload.
    pub(crate) fn frame(&mut self) -> (u8, Vec<u8>) {
        loop {
            if let Some((b0, payload, used)) = client_frame(&self.buf) {
                self.buf.drain(..used);
                return (b0, payload);
            }
            let mut chunk = [0u8; 64 * 1024];
            let n = self.stream.read(&mut chunk).unwrap();
            assert!(n > 0, "MailALM closed the connection");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// The next message from MailALM: one text frame holding one packet; the JSON inside.
    pub(crate) fn message(&mut self) -> Value {
        let (b0, payload) = self.frame();
        assert_eq!(b0, 0x81, "a whole text frame");
        let packet: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(packet["idPaquet"], 0);
        assert_eq!(packet["totalPaquet"], 1);
        serde_json::from_str(packet["donnees"].as_str().unwrap()).unwrap()
    }

    pub(crate) fn send_raw(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).unwrap();
    }

    /// Sends `message` in one packet numbered from 1 (as the Python reads them), in one text frame.
    pub(crate) fn send(&mut self, message: &Value) {
        let packet = json!({ "idPaquet": 1, "totalPaquet": 1, "donnees": message.to_string() }).to_string();
        self.send_raw(&server_frame(0x81, packet.as_bytes()));
    }

    /// Waits until MailALM drops the connection (or 20 s).
    pub(crate) fn wait_for_hang_up(&mut self) {
        let mut chunk = [0u8; 4096];
        while matches!(self.stream.read(&mut chunk), Ok(n) if n > 0) {}
    }

    /// Closes (1000) and checks that MailALM echoes it.
    pub(crate) fn close(&mut self) {
        self.send_raw(&server_frame(0x88, &1000u16.to_be_bytes()));
        assert_eq!(self.frame(), (0x88, 1000u16.to_be_bytes().to_vec()));
    }
}

/// One masked frame from the front of `buf`, unmasked: first byte, payload, bytes taken.
fn client_frame(buf: &[u8]) -> Option<(u8, Vec<u8>, usize)> {
    if buf.len() < 2 {
        return None;
    }
    assert!(buf[1] & 0x80 != 0, "frames from MailALM are masked");
    let (len, at) = match buf[1] & 0x7F {
        126 => {
            if buf.len() < 4 {
                return None;
            }
            (usize::from(u16::from_be_bytes([buf[2], buf[3]])), 4)
        }
        127 => {
            if buf.len() < 10 {
                return None;
            }
            (usize::try_from(u64::from_be_bytes(buf[2..10].try_into().unwrap())).unwrap(), 10)
        }
        len => (usize::from(len), 2),
    };
    if buf.len() < at + 4 + len {
        return None;
    }
    let mask = &buf[at..at + 4];
    let payload = buf[at + 4..at + 4 + len].iter().enumerate().map(|(i, b)| b ^ mask[i % 4]).collect();
    Some((buf[0], payload, at + 4 + len))
}
