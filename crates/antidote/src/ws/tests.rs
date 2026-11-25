use std::thread;

use super::*;
use crate::test_server::{FakeAntidote, listen, server_frame};

const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";
const ACCEPT: &str = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";

#[test]
fn accept_value_of_the_rfc() {
    // RFC 6455 §1.3.
    assert_eq!(accept_for(KEY), ACCEPT);
}

#[test]
fn client_frames_are_masked() {
    let mask = [1, 2, 3, 4];
    assert_eq!(encode_frame(Opcode::Text, b"Hi", mask), vec![0x81, 0x82, 1, 2, 3, 4, b'H' ^ 1, b'i' ^ 2]);
    let frame = encode_frame(Opcode::Binary, &[0u8; 200], mask);
    assert_eq!(frame.len(), 8 + 200);
    assert_eq!(&frame[..4], &[0x82, 0xFE, 0x00, 200]);
    assert_eq!(&frame[4..8], &mask);
    assert_eq!(&frame[8..12], &mask);
    let frame = encode_frame(Opcode::Text, &vec![7u8; 70_000], mask);
    assert_eq!(frame.len(), 14 + 70_000);
    assert_eq!(&frame[..10], &[0x81, 0xFF, 0, 0, 0, 0, 0, 0x01, 0x11, 0x70]);
    assert_eq!(encode_frame(Opcode::Close, &[], mask), vec![0x88, 0x80, 1, 2, 3, 4]);
}

#[test]
fn server_frames_are_read_whole_or_not_at_all() {
    let bytes = server_frame(0x81, b"Bonjour");
    for cut in 0..bytes.len() {
        assert_eq!(parse_frame(&bytes[..cut], 100).unwrap(), None);
    }
    let (frame, used) = parse_frame(&bytes, 100).unwrap().unwrap();
    assert_eq!(used, bytes.len());
    assert_eq!(frame, Frame { fin: true, opcode: Opcode::Text, payload: b"Bonjour".to_vec() });
    // 16-bit and 64-bit lengths, one frame behind the other.
    let mut two = server_frame(0x02, &[9u8; 300]);
    two.extend(server_frame(0x80, &[8u8; 70_000]));
    let (first, used) = parse_frame(&two, MAX_MESSAGE).unwrap().unwrap();
    assert_eq!((first.fin, first.opcode, first.payload.len(), used), (false, Opcode::Binary, 300, 304));
    let (second, used) = parse_frame(&two[used..], MAX_MESSAGE).unwrap().unwrap();
    assert_eq!((second.fin, second.opcode, second.payload.len(), used), (true, Opcode::Continuation, 70_000, 70_010));
}

#[test]
fn bad_server_frames_are_refused() {
    fn refused(bytes: &[u8]) -> bool {
        parse_frame(bytes, 1000).is_err()
    }
    assert!(refused(&[0x81, 0x82, 1, 2, 3, 4, 0, 0]), "masked");
    assert!(refused(&[0xC1, 0x00]), "RSV1 (no compression was agreed)");
    assert!(refused(&[0x83, 0x00]), "opcode 3");
    assert!(refused(&[0x8B, 0x00]), "opcode 0xB");
    assert!(refused(&[0x09, 0x00]), "fragmented ping");
    assert!(refused(&server_frame(0x89, &[0u8; 126])), "long ping");
    // A length over the limit is refused from the header, before any payload.
    assert!(refused(&[0x81, 126, 0x03, 0xE9]), "1001 bytes");
    assert!(refused(&[0x81, 127, 0x80, 0, 0, 0, 0, 0, 0, 0]), "top bit");
    assert!(parse_frame(&[0x81, 127, 0, 0, 0, 0, 0x01, 0, 0, 0x01], MAX_MESSAGE).is_err(), "16 MiB + 1");
    assert_eq!(parse_frame(&[0x81, 126, 0x03, 0xE8], 1000).unwrap(), None, "1000 bytes: only incomplete");
}

fn frame(fin: bool, opcode: Opcode, payload: &[u8]) -> Frame {
    Frame { fin, opcode, payload: payload.to_vec() }
}

#[test]
fn fragments_are_put_together_around_control_frames() {
    let mut a = Assembler::default();
    assert_eq!(a.on_frame(frame(false, Opcode::Text, b"Bon")).unwrap(), None);
    assert_eq!(a.on_frame(frame(true, Opcode::Ping, b"p")).unwrap(), Some(Event::Ping(b"p".to_vec())));
    assert_eq!(a.on_frame(frame(false, Opcode::Continuation, b"jou")).unwrap(), None);
    assert_eq!(a.on_frame(frame(true, Opcode::Pong, b"")).unwrap(), None);
    assert_eq!(
        a.on_frame(frame(true, Opcode::Continuation, "r à tous".as_bytes())).unwrap(),
        Some(Event::Message("Bonjour à tous".into()))
    );
    // A character cut between two fragments; binary is read as text, as the Python does.
    let e = "é".as_bytes();
    assert_eq!(a.on_frame(frame(false, Opcode::Binary, &e[..1])).unwrap(), None);
    assert_eq!(a.on_frame(frame(true, Opcode::Continuation, &e[1..])).unwrap(), Some(Event::Message("é".into())));
    assert_eq!(a.on_frame(frame(true, Opcode::Close, &[0x03, 0xE8])).unwrap(), Some(Event::Close(Some(1000))));
}

#[test]
fn bad_sequences_are_refused() {
    assert!(Assembler::default().on_frame(frame(true, Opcode::Continuation, b"x")).is_err());
    let mut a = Assembler::default();
    a.on_frame(frame(false, Opcode::Text, b"a")).unwrap();
    assert!(a.on_frame(frame(true, Opcode::Text, b"b")).is_err());
    assert!(Assembler::default().on_frame(frame(true, Opcode::Text, &[0xFF, 0xFE])).is_err());
    let mut a = Assembler::default();
    a.on_frame(frame(false, Opcode::Text, &vec![b'a'; MAX_MESSAGE])).unwrap();
    assert!(a.on_frame(frame(true, Opcode::Continuation, b"b")).is_err());
}

#[test]
fn close_codes() {
    assert_eq!(close_code(&[]).unwrap(), None);
    assert_eq!(close_code(&[0x03, 0xE8]).unwrap(), Some(1000));
    assert_eq!(close_code(b"\x03\xE9bye").unwrap(), Some(1001));
    assert!(close_code(&[0x03]).is_err());
    assert!(close_code(&[0x03, 0xE8, 0xFF]).is_err());
}

fn answer(lines: &[&str]) -> Vec<u8> {
    format!("{}\r\n\r\n", lines.join("\r\n")).into_bytes()
}

#[test]
fn handshake_answers() {
    let accept = format!("Sec-WebSocket-Accept: {ACCEPT}");
    let good = ["HTTP/1.1 101 Switching Protocols", "Upgrade: websocket", "Connection: Upgrade", accept.as_str()];
    assert!(check_response(&answer(&good), KEY).is_ok());
    let lax = ["HTTP/1.1 101 OK", "upgrade: WebSocket", "connection: keep-alive, upgrade", "Server: x", accept.as_str()];
    assert!(check_response(&answer(&lax), KEY).is_ok());
    fn refused(lines: &[&str]) -> bool {
        check_response(&answer(lines), KEY).is_err()
    }
    assert!(refused(&["HTTP/1.1 200 OK", good[1], good[2], good[3]]));
    assert!(refused(&["HTTP/1.0 101 OK", good[1], good[2], good[3]]));
    assert!(refused(&[good[0], good[2], good[3]]), "no Upgrade");
    assert!(refused(&[good[0], good[1], good[3]]), "no Connection");
    assert!(refused(&[good[0], good[1], good[2]]), "no accept");
    assert!(refused(&[good[0], good[1], good[2], "Sec-WebSocket-Accept: AAAAAAAAAAAAAAAAAAAAAAAAAAA="]));
    assert!(refused(&[good[0], good[1], good[2], good[3], good[3]]), "two accepts");
    assert!(refused(&[good[0], good[1], good[2], good[3], "Sec-WebSocket-Extensions: permessage-deflate"]));
    assert!(refused(&[good[0], good[1], good[2], good[3], "Sec-WebSocket-Protocol: chat"]));
    assert!(refused(&[good[0], good[1], good[2], good[3], "pas un en-tête"]));
    assert!(check_response(b"HTTP/1.1 101 \xFF\r\n\r\n", KEY).is_err());
}

/// The next message, waiting at most 20 s.
fn next(ws: &mut WebSocket) -> Incoming {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(incoming) = ws.poll().unwrap() {
            return incoming;
        }
        assert!(Instant::now() < deadline, "nothing came");
    }
}

#[test]
fn messages_pings_and_close_from_the_server() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        // A message right behind the handshake's answer, in the same write.
        let mut fake = FakeAntidote::accept(&listener, &server_frame(0x81, b"tout de suite"));
        fake.send_raw(&server_frame(0x89, b"ping?"));
        assert_eq!(fake.frame(), (0x8A, b"ping?".to_vec()));
        let mut parts = server_frame(0x01, b"en ");
        parts.extend(server_frame(0x80, b"deux"));
        fake.send_raw(&parts);
        fake.close();
    });
    let mut ws = WebSocket::connect(port).unwrap();
    assert_eq!(next(&mut ws), Incoming::Message("tout de suite".into()));
    assert_eq!(next(&mut ws), Incoming::Message("en deux".into()));
    assert_eq!(next(&mut ws), Incoming::Closed);
    server.join().unwrap();
}

#[test]
fn messages_and_close_from_mailalm() {
    let (listener, port) = listen();
    let long = "x".repeat(70_000);
    let expected = long.clone();
    let server = thread::spawn(move || {
        let mut fake = FakeAntidote::accept(&listener, &[]);
        assert_eq!(fake.frame(), (0x81, "Allô".as_bytes().to_vec()));
        assert_eq!(fake.frame(), (0x81, expected.into_bytes()));
        assert_eq!(fake.frame(), (0x88, 1000u16.to_be_bytes().to_vec()));
        fake.send_raw(&server_frame(0x88, &1000u16.to_be_bytes()));
    });
    let mut ws = WebSocket::connect(port).unwrap();
    ws.send_text("Allô").unwrap();
    ws.send_text(&long).unwrap();
    ws.close();
    server.join().unwrap();
}

#[test]
fn a_masked_server_frame_is_an_error() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        let mut fake = FakeAntidote::accept(&listener, &[]);
        fake.send_raw(&[0x81, 0x81, 1, 2, 3, 4, b'a' ^ 1]);
    });
    let mut ws = WebSocket::connect(port).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let got = loop {
        match ws.poll() {
            Ok(None) if Instant::now() < deadline => {}
            other => break other,
        }
    };
    assert!(matches!(got, Err(Error::Protocol(_))), "{got:?}");
    server.join().unwrap();
}

#[test]
fn a_wrong_accept_is_refused() {
    let (listener, port) = listen();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf).unwrap();
        // The RFC's example accept, not the one for MailALM's random key.
        stream
            .write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {ACCEPT}\r\n\r\n").as_bytes())
            .unwrap();
        // Wait for MailALM to hang up.
        let _ = stream.read(&mut buf);
    });
    assert!(matches!(WebSocket::connect(port), Err(Error::Protocol(_))));
    server.join().unwrap();
}

#[test]
fn nothing_listening() {
    let (listener, port) = listen();
    drop(listener);
    assert!(matches!(WebSocket::connect(port), Err(Error::Io(_))));
    assert!(matches!(WebSocket::connect(0), Err(Error::Io(_))));
}
