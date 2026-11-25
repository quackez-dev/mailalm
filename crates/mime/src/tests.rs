use super::*;

const PLAIN: &[u8] = b"From: Zoe Park <zoe@studio.example>\r\n\
To: Alex Morin <alex@example.com>, sam@example.com\r\n\
Cc: \"Roy, Camille\" <camille@example.com>\r\n\
Subject: =?UTF-8?Q?Storyboard_r=C3=A9vision?=\r\n\
Date: Fri, 20 Nov 2026 09:30:00 -0500\r\n\
Message-ID: <abc123@studio.example>\r\n\
References: <root@studio.example>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Hi Alex,\r\n\
\r\n\
The animatic is in good shape.\r\n";

const MIXED: &str = "From: print@shop.example\r\n\
To: alex@example.com\r\n\
Subject: Proof\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"XX\"\r\n\
\r\n\
--XX\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><body><p>Your <b>proof</b> is ready.</p><script>alert(1)</script></body></html>\r\n\
--XX\r\n\
Content-Type: application/pdf; name=\"proof.pdf\"\r\n\
Content-Disposition: attachment; filename=\"..\\\\..\\\\proof\u{202e}fdp.exe\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--XX--\r\n";

#[test]
fn a_plain_message_is_read() {
    let p = parse(PLAIN).expect("a message");
    assert_eq!(p.from, Person { name: "Zoe Park".into(), address: "zoe@studio.example".into() });
    assert_eq!(p.to.len(), 2);
    assert_eq!(p.cc[0].name, "Roy, Camille");
    assert_eq!(p.subject, "Storyboard révision");
    assert!(p.date.is_some());
    assert_eq!(p.message_id, "abc123@studio.example");
    assert_eq!(p.references, vec!["root@studio.example".to_owned()]);
    assert!(p.text.contains("The animatic is in good shape."));
    assert!(!p.text.contains('\r'));
    assert!(!p.from_html);
    assert!(p.attachments.is_empty());
}

#[test]
fn the_date_is_read_in_utc() {
    let p = parse(PLAIN).expect("a message");
    // 2026-11-20 14:30:00 UTC.
    let day = mailalm_model::query::days_from_civil(2026, 11, 20) * 86_400;
    assert_eq!(p.date, Some(day + 14 * 3600 + 30 * 60));
}

#[test]
fn html_becomes_text_and_attachments_are_only_listed() {
    let p = parse(MIXED.as_bytes()).expect("a message");
    assert!(p.from_html);
    assert!(p.text.contains("proof"));
    assert!(!p.text.contains("<b>"));
    assert!(!p.text.contains("alert(1)"), "scripts are not text: {:?}", p.text);
    assert_eq!(p.attachments.len(), 1);
    let a = &p.attachments[0];
    assert_eq!(a.declared_type, "application/pdf");
    // No folder, no direction override.
    assert!(!a.name.contains('\\') && !a.name.contains('/'));
    assert!(!a.name.contains('\u{202e}'));
    assert_eq!(a.name, "prooffdp.exe");
    assert_eq!(a.size, 9);
}

#[test]
fn a_header_block_makes_a_summary() {
    let header = &PLAIN[..PLAIN.windows(4).position(|w| w == b"\r\n\r\n").map_or(PLAIN.len(), |i| i + 4)];
    let s = summary_from_header(42, header, Flags { seen: true, ..Flags::default() }, 1234, Some(7));
    assert_eq!(s.uid, 42);
    assert_eq!(s.from.display(), "Zoe Park");
    assert_eq!(s.subject, "Storyboard révision");
    assert!(s.date > 7);
    assert!(s.flags.seen);
    assert_eq!(s.size, 1234);
    assert!(!s.has_attachments);

    let mixed_header = b"From: a@b.example\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n";
    let m = summary_from_header(1, mixed_header, Flags::default(), 1, Some(99));
    assert!(m.has_attachments);
    // No Date header: the server's arrival date.
    assert_eq!(m.date, 99);
}

#[test]
fn header_text_is_cleaned() {
    assert_eq!(clean_line("  Invoice\u{202e}fdp.exe \r\n\t paid "), "Invoicefdp.exe paid");
    assert_eq!(clean_line("a\u{200b}b"), "ab");
    assert_eq!(clean_text("one\r\ntwo\u{7}\tthree"), "one\ntwo\tthree");
}

#[test]
fn file_names_lose_folders_and_forbidden_characters() {
    assert_eq!(clean_file_name("C:\\Windows\\evil.dll"), "evil.dll");
    assert_eq!(clean_file_name("../../etc/passwd"), "passwd");
    assert_eq!(clean_file_name("what?.pdf. . "), "what_.pdf");
    assert_eq!(clean_file_name("a:b|c.txt"), "a_b_c.txt");
}

#[test]
fn previews_are_short_and_on_one_line() {
    let p = preview("Hello\n\n> quoted\n-----\nworld");
    assert_eq!(p, "Hello quoted world");
    let long = "word ".repeat(100);
    let p = preview(&long);
    assert!(p.chars().count() <= PREVIEW_CHARS + 1);
    assert!(p.ends_with('…'));
}

#[test]
fn recipients_are_read_from_what_was_typed() {
    let r = parse_recipients("Alex Morin <alex@example.com>; \"Roy, Camille\" <camille@example.com>, sam@example.com").expect("valid");
    assert_eq!(r.len(), 3);
    assert_eq!(r[0], Person { name: "Alex Morin".into(), address: "alex@example.com".into() });
    assert_eq!(r[1].name, "Roy, Camille");
    assert_eq!(r[2].address, "sam@example.com");
    assert!(parse_recipients("not an address").is_err());
    assert!(parse_recipients("").expect("empty is fine").is_empty());
    assert_eq!(format_person(&r[1]), "\"Roy, Camille\" <camille@example.com>");
}

#[test]
fn an_outgoing_message_round_trips_without_bcc_or_host_name() {
    let out = Outgoing {
        from: Person { name: "Alex Morin".into(), address: "alex@videotron.ca".into() },
        to: vec![Person { name: "Zoe".into(), address: "zoe@studio.example".into() }],
        cc: vec![],
        bcc: vec![Person { name: String::new(), address: "secret@example.com".into() }],
        subject: "Réunion".into(),
        text: "Bonjour,\nÀ demain.".into(),
        in_reply_to: Some("abc123@studio.example".into()),
        references: vec!["root@studio.example".into(), "abc123@studio.example".into()],
    };
    let (raw, id) = build(&out).expect("built");
    assert!(id.ends_with("@videotron.ca"));
    let text = String::from_utf8_lossy(&raw);
    assert!(!text.contains("secret@example.com"), "Bcc stays out of the header");
    assert!(text.contains(&format!("<{id}>")));
    let back = parse(&raw).expect("parses back");
    assert_eq!(back.subject, "Réunion");
    assert_eq!(back.from.address, "alex@videotron.ca");
    assert_eq!(back.to[0].address, "zoe@studio.example");
    assert!(back.text.contains("À demain."));
    assert_eq!(back.references.len(), 2);
    assert_eq!(out.recipients(), vec!["zoe@studio.example".to_owned(), "secret@example.com".to_owned()]);

    // For an API (Graph, Gmail), which takes the recipients from the headers: Bcc is written.
    let (raw, _) = build_for_api(&out).expect("built");
    let text = String::from_utf8_lossy(&raw);
    assert!(text.lines().any(|l| l.starts_with("Bcc:") && l.contains("secret@example.com")), "{text}");
}

#[test]
fn a_message_needs_valid_recipients() {
    let mut out = Outgoing { from: Person { name: String::new(), address: "a@b.example".into() }, ..Outgoing::default() };
    assert!(build(&out).is_err());
    out.to = vec![Person { name: String::new(), address: "bad address".into() }];
    assert!(build(&out).is_err());
}

#[test]
fn message_ids_differ() {
    assert_ne!(message_id("a@b.example"), message_id("a@b.example"));
    assert!(message_id("nobody").ends_with("@mailalm.invalid"));
}

#[test]
fn replies_go_to_the_right_people() {
    let p = parse(PLAIN).expect("a message");
    let me = Person { name: "Alex".into(), address: "ALEX@example.com".into() };
    let r = reply(&p, &me, false, "On Fri, Nov 20,");
    assert_eq!(r.to.len(), 1);
    assert_eq!(r.to[0].address, "zoe@studio.example");
    assert!(r.cc.is_empty());
    assert_eq!(r.subject, "Re: Storyboard révision");
    assert_eq!(r.in_reply_to.as_deref(), Some("abc123@studio.example"));
    assert_eq!(r.references, vec!["root@studio.example".to_owned(), "abc123@studio.example".to_owned()]);
    assert!(r.text.contains("> The animatic is in good shape."));

    let all = reply(&p, &me, true, "On Fri, Nov 20,");
    let cc: Vec<_> = all.cc.iter().map(|x| x.address.as_str()).collect();
    assert_eq!(cc, ["sam@example.com", "camille@example.com"], "everyone but me");

    let f = forward(&p, &me, "Fri, Nov 20");
    assert!(f.to.is_empty());
    assert_eq!(f.subject, "Fwd: Storyboard révision");
    assert!(f.text.contains("Forwarded message"));
    assert_eq!(reply(&parse(b"Subject: Re: hi\r\nFrom: a@b.example\r\n\r\nx").expect("msg"), &me, false, "").subject, "Re: hi");
}
