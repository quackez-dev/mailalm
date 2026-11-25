// Adapted from MailCraft (https://github.com/ZIDAN44/mailcraft), commit c3510d13, crates/net/src/protocols/imap/tests.rs. MIT OR Apache-2.0.
// Changed: through MailALM's methods; no NTLM, QRESYNC, IDLE or text-first tests; new tests for STARTTLS, folder roles, moves, flags, appends and the password kept out of errors.

use std::io::{self, Read, Write};

use mailalm_model::FolderRole;

use super::conn::{self, Conn, Limits, literal_len};
use super::quotes::plain_quotes;
use super::*;

/// A canned server: reads are served from `input`; writes are recorded.
struct Script {
    input: io::Cursor<Vec<u8>>,
    written: Vec<u8>,
    /// The most bytes one read gives.
    step: usize,
}

impl Script {
    fn new(s: &[u8]) -> Self {
        // A few bytes at a time, to exercise framing across reads.
        Script { input: io::Cursor::new(s.to_vec()), written: Vec::new(), step: 7 }
    }
}

impl Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.step);
        self.input.read(&mut buf[..n])
    }
}

impl Write for Script {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What the client wrote, as text.
fn sent(c: Client<Script>) -> String {
    String::from_utf8_lossy(&c.conn.into_inner().written).into_owned()
}

#[test]
fn sets_and_dates() {
    assert_eq!(uid_set(&[5, 1, 2, 3, 9, 10, 3, 0]), "1:3,5,9:10");
    assert_eq!(uid_set(&[]), "");
    assert_eq!(uid_set(&[u32::MAX, u32::MAX - 1]), format!("{}:{}", u32::MAX - 1, u32::MAX));
    assert_eq!(uid_sets(&[9, 1, 2, 3, 7, 0, 2], 2), ["1:2", "3,7", "9"]);
    assert!(uid_sets(&[0], 200).is_empty());
    assert_eq!(parse_internal_date("14-Oct-2026 09:30:00 +0200"), Some(1_791_963_000));
    assert_eq!(parse_internal_date(" 4-Jan-1970 00:00:00 -0100"), Some(3 * 86_400 + 3600));
    assert_eq!(parse_internal_date("junk"), None);
    assert_eq!(parse_internal_date("99-Oct-2026 09:30:00 +0200"), None);
    assert_eq!(parse_internal_date("31-Dec-9999 23:59:59 +0000"), Some(253_402_300_799));
    assert_eq!(parse_internal_date("01-Jan-0001 00:00:00 +0000"), Some(-62_135_596_800));
    assert_eq!(quoted("a\"b\\c").as_deref(), Some("\"a\\\"b\\\\c\""));
    assert_eq!(quoted("pässword"), None);
}

#[test]
fn literals() {
    assert_eq!(literal_len(b"* 1 FETCH (BODY[] {12}"), Some(12));
    assert_eq!(literal_len(b"A1 APPEND x {3+}"), Some(3));
    assert_eq!(literal_len(b"* OK {abc}"), None);
    assert_eq!(literal_len(b"* OK done"), None);
    assert_eq!(literal_len(b"{99999999999999999}"), None);
}

/// A password that can't be quoted (not ASCII) goes as a literal: its bytes are sent only once
/// the server asks for them with `+`. A server that answers the command instead has refused the
/// sign-in, and never sees the password.
#[test]
fn a_literal_login_needs_its_continuation() {
    let refused = b"* OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] ready\r\nM1 NO not like that\r\n";
    let mut c = Client::from_stream(Script::new(refused)).unwrap();
    assert!(c.has("move") && !c.has("CONDSTORE"), "the greeting's capabilities");
    assert!(matches!(c.login("zoe@x", "pässword"), Err(NetError::Auth(m)) if m == "not like that"));
    assert_eq!(c.conn.into_inner().written, b"M1 LOGIN \"zoe@x\" {9}\r\n", "the password held back");

    let server = b"* OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] ready\r\n+ go on\r\nM1 OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS CONDSTORE] logged in\r\n";
    let mut c = Client::from_stream(Script::new(server)).unwrap();
    c.login("zoe@x", "pässword").unwrap();
    assert!(c.has("CONDSTORE"), "the capabilities said at sign-in");
    assert_eq!(c.conn.into_inner().written, "M1 LOGIN \"zoe@x\" {9}\r\npässword\r\n".as_bytes());
}

/// With SASL-IR, `AUTHENTICATE PLAIN` in one line; and a server that echoes what it was sent
/// never puts the password, or the token it goes in, in the error.
#[test]
fn the_password_never_shows_in_an_error() {
    let s = b"* OK hi\r\nM1 NO LOGIN \"zoe\" \"hunter22\" refused\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    let e = c.login("zoe", "hunter22").err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.starts_with("sign-in failed: ") && e.contains("\"***\"") && !e.contains("hunter22"), "{e}");

    let s = b"* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN SASL-IR] hi\r\nM1 NO bad token AHpvZQBodW50ZXIyMg== for hunter22\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    let e = c.login("zoe", "hunter22").err();
    assert!(e.as_ref().is_some_and(NetError::is_auth), "{e:?}");
    let e = e.map(|e| e.to_string()).unwrap_or_default();
    assert!(!e.contains("hunter22") && !e.contains("AHpvZQBodW50ZXIyMg=="), "{e}");
    assert_eq!(sent(c), "M1 AUTHENTICATE PLAIN AHpvZQBodW50ZXIyMg==\r\n");

    // No password with LOGIN, and no PLAIN to take it: said, nothing sent.
    let mut c = Client::from_stream(Script::new(b"* OK [CAPABILITY IMAP4rev1 AUTH=GSSAPI LOGINDISABLED] hi\r\n")).unwrap();
    assert!(matches!(c.login("zoe", "hunter22"), Err(NetError::Auth(m)) if m.contains("GSSAPI")));
    assert!(c.conn.into_inner().written.is_empty());
}

#[test]
fn login_quoted_then_fetch_headers() {
    let server = b"* OK hi\r\n\
M1 OK [CAPABILITY IMAP4rev1 MOVE UIDPLUS] logged in\r\n\
* 1 FETCH (UID 7 FLAGS (\\Seen) RFC822.SIZE 12 INTERNALDATE \"14-Oct-2026 09:30:00 +0200\" BODY[HEADER] {12}\r\nSubject: x\r\n)\r\n\
* 2 FETCH (FLAGS (\\Flagged))\r\n\
M2 OK done\r\n";
    let mut c = Client::from_stream(Script::new(server)).unwrap();
    c.login("zoe@x", "pw").unwrap();
    assert!(c.has("UIDPLUS"));
    let f = c.fetch_headers(&[8, 7]).unwrap();
    let want = Fetched { uid: 7, flags: vec!["\\Seen".to_string()], size: 12, internal_date: Some(1_791_963_000), header: b"Subject: x\r\n".to_vec() };
    assert_eq!(f, [want]);
    assert_eq!(sent(c), "M1 LOGIN \"zoe@x\" \"pw\"\r\nM2 UID FETCH 7:8 (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])\r\n");
}

#[test]
fn errors_and_hostile_input() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 NO [AUTHENTICATIONFAILED] bad password\r\n")).unwrap();
    assert!(matches!(c.login("u", "p"), Err(NetError::Auth(m)) if m.contains("bad password")));
    assert!(matches!(Client::from_stream(Script::new(b"* BYE go away\r\n")), Err(NetError::Closed(_))));
    assert!(Client::from_stream(Script::new(b"")).is_err());
    assert!(Client::from_stream(Script::new(b"garbage\r\n")).is_err());
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* 1 FETCH (BODY[] {999999999999}\r\n")).unwrap();
    assert!(matches!(c.fetch_message(1), Err(NetError::TooLarge(_))));
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* WEIRD stuff\r\nM1 OK fine\r\n")).unwrap();
    assert!(c.noop().is_ok());
    // Bare LF line ends are tolerated.
    let mut c = Client::from_stream(Script::new(b"* OK hi\n* SEARCH 3 1 2\nM1 OK done\n")).unwrap();
    assert_eq!(c.uids().unwrap(), vec![1, 2, 3]);
    // A refusal is the server's, in its words.
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 BAD no folder selected\r\n")).unwrap();
    assert!(matches!(c.uids(), Err(NetError::Server(m)) if m == "no folder selected"));
}

/// Found in review: bare LF line ends were mended across the whole response, literals too, so a
/// literal holding bare LFs outgrew its `{n}` and the response was dropped; and a response ending
/// in CRLF was never mended, so a bare LF before its literal stayed.
#[test]
fn bare_lf_line_ends_are_mended_outside_literals_only() {
    let s = b"* OK hi\r\n\
* 1 FETCH (UID 7 BODY[] {6}\r\na\nb\nc\n)\n\
* 2 FETCH (UID 8 BODY[] {3}\nx\r\n)\r\n\
M1 OK done\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    assert_eq!(c.fetch_message(7).unwrap().as_deref(), Some(&b"a\nb\nc\n"[..]));
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    assert_eq!(c.fetch_message(8).unwrap().as_deref(), Some(&b"x\r\n"[..]));
}

/// A response code followed straight by a non-ASCII character (found by MailCraft's fuzzing):
/// imap-proto sliced inside the character and panicked, so a server could crash the client.
#[test]
fn a_code_right_before_non_ascii_text_never_panics() {
    let c = Client::from_stream(Script::new(b"* OK [CAPABILITY IMAP4rev1]\xd7\n")).unwrap();
    assert!(c.has("IMAP4REV1"));
    let mut c = Client::from_stream(Script::new("* OK hi\r\nM1 NO [ALERT]é quota full\r\n".as_bytes())).unwrap();
    assert!(matches!(c.noop(), Err(NetError::Server(m)) if m.contains("é quota full")), "the text is kept whole");
    let mut c = Client::from_stream(Script::new("* OK hi\r\n+ [X]ü\r\nM1 OK [APPENDUID 7 9]ö\r\n".as_bytes())).unwrap();
    assert!(c.append("Drafts", &[], b"x").is_ok());
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n+[UIDNEXT 9]\xff\nM1 OK done\r\n")).unwrap();
    assert!(c.append("Drafts", &[], b"x").is_ok(), "a continuation without its space");
    // Inside a quoted string the bracket isn't a code: left as it was.
    assert_eq!(conn::space_after_code(b"* 1 FETCH (X \"[a]\xc3\xa9\")\r\n".to_vec()), b"* 1 FETCH (X \"[a]\xc3\xa9\")\r\n");
}

/// A response imap-proto panics on (`INTERNALDATE NIL`, found in review: it unwraps the date)
/// is skipped like any other it doesn't understand, and the command carries on.
#[test]
fn a_response_the_parser_panics_on_is_skipped() {
    let s = b"* OK hi\r\n* 1 FETCH (UID 5 FLAGS (\\Seen) INTERNALDATE NIL)\r\n* 2 FETCH (UID 6 FLAGS (\\Seen))\r\nM1 OK done\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    let got = c.fetch_flags(&[5, 6]).unwrap();
    assert_eq!(got, [(6, vec!["\\Seen".to_string()])]);
}

/// An INTERNALDATE with a year far out of range (found in review: the day count overflowed, a
/// panic in debug builds and a garbage date in release) or a negative time is refused.
#[test]
fn a_hostile_internal_date_is_refused() {
    for bad in [
        "01-Jan-9000000000000000000 00:00:00 +0000",
        "01-Jan--9000000000000000000 00:00:00 +0000",
        "01-Jan-10000 00:00:00 +0000",
        "01-Jan-0 00:00:00 +0000",
        "01-Mar-9223372036854775807 00:00:00 +0000",
        "01-Jan-2026 -1:00:00 +0000",
        "01-Jan-2026 10:-5:00 +0000",
        "01-Jan-2026 10:00:-1 +0000",
        "01-Jan-2026 +1:00:00 +0000",
        "01-Jan-2026 10:00:00 +00-5",
    ] {
        assert_eq!(parse_internal_date(bad), None, "{bad}");
    }
    let s = b"* OK hi\r\n* 1 FETCH (UID 5 INTERNALDATE \"01-Jan-9000000000000000000 00:00:00 +0000\" BODY[HEADER] {3}\r\nX:\n)\r\nM1 OK done\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    let got = c.fetch_headers(&[5]).unwrap();
    assert_eq!(got.iter().map(|f| (f.uid, f.internal_date)).collect::<Vec<_>>(), [(5, None)]);
}

/// A hostile server's lists nested a hundred thousand deep don't overflow the stack (they did,
/// in imap-proto's parser, from a thousand), even on a thread with Windows' 1 MB main stack;
/// real nesting parses.
#[test]
fn deeply_nested_lists_dont_overflow_the_stack() {
    let h = std::thread::Builder::new().stack_size(1 << 20).spawn(|| {
        let mut out = Vec::new();
        for depth in [30usize, conn::MAX_NESTING - 1, conn::MAX_NESTING + 1, 1000, 100_000] {
            let mut s = b"* OK ready\r\n* 1 FETCH (UID 1 BODYSTRUCTURE ".to_vec();
            s.extend(std::iter::repeat_n(b'(', depth));
            s.extend(b"\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 1 1");
            s.extend(std::iter::repeat_n(b')', depth));
            s.extend(b")\r\nM1 OK done\r\n");
            let mut c = Client::from_stream(Script::new(&s)).unwrap();
            out.push((depth, c.fetch_flags(&[1]).is_ok()));
        }
        out
    });
    let out = h.unwrap().join().expect("no overflow");
    assert!(out.iter().all(|(_, ok)| *ok), "{out:?}");
    // The count skips quoted strings and literals.
    assert_eq!(conn::nesting(b"* 1 FETCH (UID 1 BODY[] {5}\r\n(((((\r\n FLAGS (\"((\" \\Seen))"), 2);
}

/// A folder's `UID SEARCH ALL` is answered in one line: past 1 MB (some 130,000 messages) it
/// was refused, the connection dropped, and the account signed in again without end.
#[test]
fn a_big_folders_search_line_is_read() {
    let mut s = b"* OK hi\r\n* SEARCH".to_vec();
    for u in 1..=200_000u32 {
        s.extend(format!(" {u}").as_bytes());
    }
    s.extend(b"\r\nM1 OK done\r\n");
    assert!(s.len() > 1 << 20);
    let mut c = Client::from_stream(Script { step: 16 << 10, ..Script::new(&s) }).unwrap();
    assert_eq!(c.uids().unwrap().len(), 200_000);
}

/// Responses are framed the same however the server's bytes come: each look at the buffer takes
/// up where the last stopped, inside a line, before a literal or inside one.
#[test]
fn responses_are_framed_the_same_however_the_bytes_come() {
    let s = b"* OK hi\r\n\
* 1 FETCH (UID 7 BODY[] {12}\r\nSubject: x\r\n BODY[HEADER] {3}\r\nabc)\r\n\
* SEARCH 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16\r\n\
* 2 FETCH (BODY[] {0}\r\n)\r\n\
M1 OK done\r\n";
    let frames = |step: usize| {
        let mut c = Conn::new(Script { step, ..Script::new(s) });
        std::iter::from_fn(|| c.read_raw().ok()).collect::<Vec<_>>()
    };
    let whole = frames(s.len());
    assert_eq!(whole.len(), 5);
    assert_eq!(whole.concat(), s);
    for step in 1..=40 {
        assert_eq!(frames(step), whole, "{step} bytes a read");
    }
}

/// A line whose literal hasn't all come is read once: as the literal's bytes arrive only they are
/// waited for. Shown by blanking the line in the buffer once it was read: the response is framed
/// all the same.
#[test]
fn a_line_waiting_for_its_literal_is_not_read_again() {
    let line = b"* 1 FETCH (UID 7 BODY[] {10}\r\n";
    let mut c = Conn::new(Script { step: 1 << 10, ..Script::new(&[&line[..], b"ab"].concat()) });
    assert!(matches!(c.read_raw(), Err(NetError::Closed(_))), "the literal isn't whole yet");
    c.buf.get_mut(c.pos..c.pos + line.len()).unwrap().fill(b'x');
    c.buf.extend_from_slice(b"\ncdefghi)\r\n* 2 EXPUNGE\r\n");
    assert_eq!(c.read_raw().unwrap().len(), line.len() + 10 + 3);
    assert_eq!(c.read_raw().unwrap(), b"* 2 EXPUNGE\r\n");
    // The command's byte budget is still asked about as the literal comes.
    let mut c = Conn::new(Script { step: 1 << 10, ..Script::new(&[&line[..], b"ab"].concat()) });
    assert!(matches!(c.read_raw(), Err(NetError::Closed(_))));
    c.set_limits(Limits { bytes: line.len() + 9, ..Default::default() });
    assert!(matches!(c.read_raw(), Err(NetError::TooLarge(_))));
}

/// A mailbox a server lists in UTF-8 is named whole (`Büro` went as `Bro`, another mailbox),
/// and never with a line end in it.
#[test]
fn a_utf8_mailbox_name_is_sent_whole() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 NO x\r\nM2 NO x\r\n")).unwrap();
    assert!(c.select("B\u{fc}ro \"x\"").is_err());
    assert!(c.select("a\r\nM9 DELETE b\u{fc}").is_err());
    assert_eq!(sent(c), "M1 SELECT \"B\u{fc}ro \\\"x\\\"\"\r\nM2 SELECT \"aM9 DELETE b\u{fc}\"\r\n");
}

/// Without UIDPLUS, EXPUNGE takes every message marked `\Deleted`: the ones another program
/// marked and left are unmarked while it runs, so only what was asked for goes.
#[test]
fn a_plain_expunge_spares_what_others_marked() {
    let s = b"* OK hi\r\nM1 OK done\r\n* SEARCH 4 9\r\nM2 OK done\r\nM3 OK done\r\nM4 OK done\r\nM5 OK done\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    c.delete(&[7]).unwrap();
    let want = "M1 UID STORE 7 +FLAGS.SILENT (\\Deleted)\r\nM2 UID SEARCH DELETED NOT UID 7\r\nM3 UID STORE 4,9 -FLAGS.SILENT (\\Deleted)\r\nM4 EXPUNGE\r\nM5 UID STORE 4,9 +FLAGS.SILENT (\\Deleted)\r\n";
    assert_eq!(sent(c), want);
}

/// A flag change the server tells of while headers are fetched (with QRESYNC it names the UID)
/// is no header; nor is a header for a message that wasn't asked for.
#[test]
fn a_flag_change_told_in_passing_makes_no_message() {
    let s = b"* OK hi\r\n* 1 FETCH (UID 5 FLAGS () RFC822.SIZE 20 BODY[HEADER] {15}\r\nSubject: five\r\n)\r\n* 9 FETCH (UID 300 FLAGS (\\Seen) MODSEQ (991))\r\n* 10 FETCH (UID 301 BODY[HEADER] {2}\r\nX:)\r\nM1 OK done\r\n";
    let mut c = Client::from_stream(Script::new(s)).unwrap();
    let got = c.fetch_headers(&[5]).unwrap();
    assert_eq!(got.iter().map(|f| (f.uid, f.header.clone())).collect::<Vec<_>>(), [(5, b"Subject: five\r\n".to_vec())]);
}

/// Found against a real server: a folder it lists as `"Back\\slash"` or `"Say \"hi\""` was
/// shown, and asked for, with its backslashes still in (imap-proto hands quoted strings back
/// unread), so it couldn't be opened or filed into. The delimiter stays as the parser wants it.
#[test]
fn a_quoted_name_is_read_without_its_escapes() {
    let server = br#"* OK hi
* LIST (\HasNoChildren) "\\" "Back\\slash"
* LIST (\HasNoChildren) "/" "Say \"hi\""
* LIST (\HasNoChildren) "/" "\\"
* LIST (\HasNoChildren) "/" "Plain name"
* LIST (\Noselect) NIL "Flat\\name"
* LIST (\HasNoChildren) "/" {5}
Lit\"
M1 OK done
"#;
    let mut c = Client::from_stream(Script::new(server)).unwrap();
    let folders = c.folders().unwrap();
    let names: Vec<&str> = folders.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, [r"Back\slash", r#"Say "hi""#, r"\", "Plain name", r#"Lit\""#], "a folder that can't be opened is left out");
    assert_eq!(folders[0].delimiter, Some('\\'));
    // Asked for by the name it has, escaped once.
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 OK done\r\n")).unwrap();
    c.select(r"Back\slash").unwrap();
    assert!(sent(c).contains(r#"SELECT "Back\\slash""#));

    let same = |raw: &[u8]| assert_eq!(plain_quotes(raw.to_vec()), raw, "{}", String::from_utf8_lossy(raw));
    // A status response's text isn't strings; nor is a quote that never closes; a literal's
    // bytes are the message's own; a delimiter outside a listing is one character.
    same(br#"M1 NO "a\\b" isn't there"#);
    same(br#"+ "a\\b""#);
    same(br#"* 1 FETCH (X "a\\b)"#);
    same(b"* 1 FETCH (BODY[] {8}\r\n\"a\\\\b\" x)\r\n");
    let mixed = [&br#"* 1 FETCH (X ("a \"b\".pdf" "c") BODY[] {3}"#[..], b"\r\n", br#""\\ Y "d\\e")"#, b"\r\n"].concat();
    let fixed = [&b"* 1 FETCH (X ({9}\r\n"[..], br#"a "b".pdf "c") BODY[] {3}"#, b"\r\n", br#""\\ Y {3}"#, b"\r\n", br"d\e)", b"\r\n"].concat();
    assert_eq!(plain_quotes(mixed), fixed);
    // Junk never panics.
    for junk in [&br"\"[..], br#""\"#, br#"* LIST "\""#, b"* X \"\\\r\n", br#""\"\"\""#, b"* X {99999999999}\r\n\"\\\\\""] {
        let _ = plain_quotes(junk.to_vec());
    }
}

/// Roles from the server's special-use marks, else from the usual names; one folder per role, the
/// one the server marks first; names decoded from modified UTF-7, the server's kept for commands.
#[test]
fn folders_have_their_roles_and_decoded_names() {
    let server = b"* OK [CAPABILITY IMAP4rev1 SPECIAL-USE LIST-EXTENDED] hi\r\n\
* LIST (\\HasNoChildren) \".\" INBOX\r\n\
* LIST (\\HasNoChildren) \".\" \"INBOX.Sent Items\"\r\n\
* LIST (\\HasNoChildren \\Sent) \".\" \"INBOX.Sent\"\r\n\
* LIST (\\Noselect \\HasChildren) \".\" \"Archives\"\r\n\
* LIST (\\HasNoChildren \\NonExistent) \".\" \"Gone\"\r\n\
* LIST (\\HasNoChildren) \".\" \"&AMk-l&AOk-ments supprim&AOk-s\"\r\n\
* LIST (\\HasNoChildren \\Spam) \".\" \"Pourriels\"\r\n\
* LIST (\\HasNoChildren) \".\" \"INBOX.Sent\"\r\n\
M1 OK done\r\n";
    let mut c = Client::from_stream(Script::new(server)).unwrap();
    let folders = c.folders().unwrap();
    let got: Vec<(&str, &str, FolderRole)> = folders.iter().map(|f| (f.path.as_str(), f.wire.as_str(), f.role)).collect();
    assert_eq!(
        got,
        [
            ("INBOX", "INBOX", FolderRole::Inbox),
            ("INBOX.Sent Items", "INBOX.Sent Items", FolderRole::Other),
            ("INBOX.Sent", "INBOX.Sent", FolderRole::Sent),
            ("Éléments supprimés", "&AMk-l&AOk-ments supprim&AOk-s", FolderRole::Trash),
            ("Pourriels", "Pourriels", FolderRole::Junk),
        ]
    );
    assert!(folders.iter().all(|f| f.delimiter == Some('.') && f.total == 0 && f.unread == 0));
    assert_eq!(sent(c), "M1 LIST \"\" \"*\" RETURN (SPECIAL-USE)\r\n");
}

/// A server that answers in its own language: a completion in words outside ASCII is the
/// command done all the same, and what its codes say is read.
#[test]
fn a_completion_in_another_language_is_still_done() {
    let mut c = Client::from_stream(Script::new("* OK hi\r\nM1 OK Terminé avec succès\r\n".as_bytes())).unwrap();
    assert!(c.noop().is_ok());
    let mut c = Client::from_stream(Script::new("* OK hi\r\n+ go\r\nM1 OK [APPENDUID 7 9] gespeichert für später\r\n".as_bytes())).unwrap();
    assert!(c.append("Drafts", &[], b"x").is_ok());
    let server =
        "* OK hi\r\n* 3 EXISTS\r\n* OK [UIDVALIDITY 5] gültig\r\n* OK [UIDNEXT 9] nächste\r\nM1 OK [READ-WRITE] ausgewählt für Änderungen\r\n";
    let mut c = Client::from_stream(Script::new(server.as_bytes())).unwrap();
    assert_eq!(c.select("INBOX").unwrap(), Mailbox { uidvalidity: 5, uidnext: 9, exists: 3 });
}

#[test]
fn status_counts_messages_and_unseen() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* STATUS \"Sent\" (MESSAGES 12 UNSEEN 2)\r\nM1 OK done\r\n")).unwrap();
    assert_eq!(c.status("Sent").unwrap(), (12, 2));
    assert_eq!(sent(c), "M1 STATUS \"Sent\" (MESSAGES UNSEEN)\r\n");
}

/// STARTTLS is the first thing asked on the connection in the clear; capabilities are asked for
/// when the greeting didn't name them.
#[test]
fn starttls_is_asked_for_before_anything_else() {
    let mut c = Client::from_stream(Script::new(b"* OK [CAPABILITY IMAP4rev1 STARTTLS LOGINDISABLED] hi\r\nM1 OK begin TLS\r\n")).unwrap();
    c.ask_starttls().unwrap();
    assert_eq!(sent(c), "M1 STARTTLS\r\n");
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* CAPABILITY IMAP4rev1 STARTTLS\r\nM1 OK\r\nM2 OK begin\r\n")).unwrap();
    c.ask_starttls().unwrap();
    assert_eq!(sent(c), "M1 CAPABILITY\r\nM2 STARTTLS\r\n");
}

/// A server without STARTTLS, or one that says it signed in already in the clear: the client
/// stops rather than go on in the clear, and says nothing more.
#[test]
fn starttls_never_falls_back_to_the_clear() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* CAPABILITY IMAP4rev1\r\nM1 OK\r\n")).unwrap();
    assert!(matches!(c.ask_starttls(), Err(NetError::Tls(m)) if m.contains("STARTTLS")));
    assert_eq!(sent(c), "M1 CAPABILITY\r\n", "nothing else went out");
    let mut c = Client::from_stream(Script::new(b"* PREAUTH welcome\r\n")).unwrap();
    assert!(c.preauth);
    assert!(matches!(c.ask_starttls(), Err(NetError::Tls(m)) if m.contains("PREAUTH")));
    assert!(c.conn.into_inner().written.is_empty());
}

/// Plain text after the server's go-ahead and before TLS would be read as if it came over TLS:
/// the client stops.
#[test]
fn starttls_stops_at_data_slipped_in_before_tls() {
    let s = b"* OK [CAPABILITY IMAP4rev1 STARTTLS] hi\r\nM1 OK begin\r\n* OK slipped in\r\n";
    let mut c = Client::from_stream(Script { step: s.len(), ..Script::new(s) }).unwrap();
    let why = c.ask_starttls().err().map(|e| e.to_string()).unwrap_or_default();
    assert!(why.contains("unexpected data before TLS negotiation"), "{why}");
}

#[test]
fn moving_uses_move_or_copies_then_expunges() {
    let mut c = Client::from_stream(Script::new(b"* OK [CAPABILITY IMAP4rev1 MOVE] hi\r\nM1 OK moved\r\n")).unwrap();
    c.move_to(&[3, 1, 2], "Archive").unwrap();
    assert_eq!(sent(c), "M1 UID MOVE 1:3 \"Archive\"\r\n");
    let mut c = Client::from_stream(Script::new(b"* OK [CAPABILITY IMAP4rev1 UIDPLUS] hi\r\nM1 OK copied\r\nM2 OK\r\nM3 OK\r\n")).unwrap();
    c.move_to(&[5], "Trash").unwrap();
    assert_eq!(sent(c), "M1 UID COPY 5 \"Trash\"\r\nM2 UID STORE 5 +FLAGS.SILENT (\\Deleted)\r\nM3 UID EXPUNGE 5\r\n");
    // Nothing to move or delete: nothing said.
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n")).unwrap();
    c.move_to(&[], "Trash").unwrap();
    c.delete(&[0]).unwrap();
    assert!(c.conn.into_inner().written.is_empty());
}

/// A flag that would change the command is refused before anything is sent; a long list of UIDs
/// goes in several commands.
#[test]
fn flags_are_checked_and_stored_in_chunks() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n")).unwrap();
    for bad in ["", "\\", "\\Seen)", "a b", "\\Seen\r\nM9 LOGOUT", "\"x\"", "x{3}"] {
        assert!(matches!(c.set_flag(&[1], bad, true), Err(NetError::Invalid(_))), "{bad:?}");
        assert!(matches!(c.append("Sent", &[bad], b"x"), Err(NetError::Invalid(_))), "{bad:?}");
    }
    assert!(c.conn.into_inner().written.is_empty());

    let uids: Vec<u32> = (1..=1001).map(|u| u * 2).collect();
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 OK\r\nM2 OK\r\nM3 OK\r\n")).unwrap();
    c.set_flag(&uids, "\\Seen", false).unwrap();
    let sent = sent(c);
    assert_eq!(sent.lines().count(), 3);
    assert!(sent.starts_with("M1 UID STORE 2,4,6,"), "{sent}");
    assert!(sent.lines().all(|l| l.ends_with(" -FLAGS.SILENT (\\Seen)")), "{sent}");
    assert!(sent.ends_with("M3 UID STORE 2002 -FLAGS.SILENT (\\Seen)\r\n"), "{sent}");
}

/// A message goes after the server's go-ahead, with its flags; refused instead, it never goes.
#[test]
fn append_waits_for_the_go_ahead() {
    let raw = b"Subject: s\r\n\r\nbody\r\n";
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n+ go\r\nM1 OK done\r\n")).unwrap();
    c.append("Sent", &["\\Seen"], raw).unwrap();
    assert_eq!(sent(c), "M1 APPEND \"Sent\" (\\Seen) {20}\r\nSubject: s\r\n\r\nbody\r\n\r\n");
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 NO [TRYCREATE] no such mailbox\r\n")).unwrap();
    assert!(matches!(c.append("Nope", &[], raw), Err(NetError::Server(_))));
    assert_eq!(sent(c), "M1 APPEND \"Nope\" () {20}\r\n");
}

#[test]
fn a_message_comes_whole_or_not_at_all() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* 3 FETCH (UID 9 BODY[] {5}\r\nhello)\r\nM1 OK done\r\n")).unwrap();
    assert_eq!(c.fetch_message(9).unwrap().as_deref(), Some(&b"hello"[..]));
    assert_eq!(sent(c), "M1 UID FETCH 9 (UID BODY.PEEK[])\r\n");
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\nM1 OK done\r\n")).unwrap();
    assert_eq!(c.fetch_message(9).unwrap(), None, "gone");
    // Over 64 MiB: refused before it is read.
    let s = format!("* OK hi\r\n* 3 FETCH (UID 9 BODY[] {{{}}}\r\n", MAX_LITERAL + 1);
    let mut c = Client::from_stream(Script::new(s.as_bytes())).unwrap();
    assert!(matches!(c.fetch_message(9), Err(NetError::TooLarge(_))));
}

/// The server's search: a word outside ASCII goes as a literal once the server says go on, in
/// UTF-8; and a folder made for MailALM (one already there is the server's `NO`).
#[test]
fn search_and_create() {
    let server = "* OK hi\r\n+ go on\r\n* SEARCH 3 9 12\r\nM1 OK done\r\nM2 OK created\r\nM3 NO it is there already\r\n";
    let mut c = Client::from_stream(Script::new(server.as_bytes())).unwrap();
    let found = c.search_query(&mailalm_model::Query::parse("subject:réunion from:zoe has:attachment")).unwrap();
    assert_eq!(found, vec![3, 9, 12]);
    c.create("INBOX.MailALM.Snoozed").unwrap();
    assert!(matches!(c.create("MailALM/Snoozed"), Err(NetError::Server(_))));
    assert!(matches!(c.create("a\r\nb"), Err(NetError::Invalid(_))));
    assert!(matches!(c.search_query(&mailalm_model::Query::parse("has:attachment")), Err(NetError::Invalid(_))));
    assert_eq!(
        sent(c),
        "M1 UID SEARCH CHARSET UTF-8 SUBJECT {8}\r\nréunion FROM \"zoe\"\r\nM2 CREATE \"INBOX.MailALM.Snoozed\"\r\nM3 CREATE \"MailALM/Snoozed\"\r\n"
    );
}

/// A draft saved before is found by its Message-ID (to replace it); an id that would change the
/// command is never sent.
#[test]
fn a_draft_is_found_by_its_message_id() {
    let mut c = Client::from_stream(Script::new(b"* OK hi\r\n* SEARCH 41\r\nM1 OK done\r\nM2 OK done\r\n")).unwrap();
    assert_eq!(c.search_message_id("<abc.123@videotron.ca>").unwrap(), vec![41]);
    assert_eq!(c.search_message_id("abc.124@videotron.ca").unwrap(), Vec::<u32>::new());
    for bad in ["", "<>", "a b@x", "a\"@x", "a\r\nM9 LOGOUT", "é@x", "a\\b@x"] {
        assert!(matches!(c.search_message_id(bad), Err(NetError::Invalid(_))), "{bad:?}");
    }
    assert_eq!(
        sent(c),
        "M1 UID SEARCH HEADER Message-ID \"<abc.123@videotron.ca>\"\r\nM2 UID SEARCH HEADER Message-ID \"<abc.124@videotron.ca>\"\r\n"
    );
}
