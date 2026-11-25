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

/// The thread's ids, for the follow-ups: from the header block, else from the whole message.
#[test]
fn summaries_keep_the_thread() {
    let header = b"From: zoe@studio.example\r\nMessage-ID: <r2@studio.example>\r\nIn-Reply-To: <q1@example.com>\r\nReferences: <root@x> <q1@example.com>\r\n\r\n";
    let s = summary_from_header(1, header, Flags::default(), 1, None);
    assert_eq!(s.in_reply_to, "q1@example.com");
    assert_eq!(s.references, ["root@x", "q1@example.com"]);
    let p = parse(header).expect("a message");
    assert_eq!(p.in_reply_to, "q1@example.com");
    let mut bare = Summary::default();
    complete_summary(&mut bare, &p);
    assert_eq!((bare.in_reply_to.as_str(), bare.references.len()), ("q1@example.com", 2));
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
        attachments: vec![],
        calendar: None,
        html: None,
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
fn attachments_are_sent_and_read_back() {
    let pdf = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\nbinary \x00\x01\x02 content\n".to_vec();
    let out = Outgoing {
        from: Person { name: "Alex".into(), address: "alex@videotron.ca".into() },
        to: vec![Person { name: String::new(), address: "zoe@studio.example".into() }],
        subject: "Plans".into(),
        text: "Here are the plans.".into(),
        attachments: vec![
            OutgoingAttachment::new("C:\\Users\\alex\\Plans, révision 2.pdf", pdf.clone()),
            OutgoingAttachment::new("notes.txt", b"line one\r\nline two\r\n".to_vec()),
        ],
        ..Outgoing::default()
    };
    assert_eq!(out.attachments[0].name, "Plans, révision 2.pdf", "no folder");
    assert_eq!(out.attachments[0].content_type, "application/pdf");
    assert_eq!(out.attachments_size(), pdf.len() + 20);
    let (raw, _) = build(&out).expect("built");
    assert!(raw.len() <= out.estimated_size(), "{} > {}", raw.len(), out.estimated_size());
    let text = String::from_utf8_lossy(&raw);
    assert!(text.to_ascii_lowercase().contains("multipart/mixed"), "{text}");

    let back = parse(&raw).expect("parses back");
    assert!(back.text.contains("Here are the plans."));
    assert_eq!(back.attachments.len(), 2);
    assert_eq!(back.attachments[0].name, "Plans, révision 2.pdf");
    assert_eq!(back.attachments[0].declared_type, "application/pdf");
    assert_eq!(back.attachments[0].size, pdf.len());
    assert_eq!(back.attachments[1].name, "notes.txt");

    let first = attachment(&raw, 0).expect("decoded");
    assert_eq!(first.bytes, pdf);
    assert_eq!(first.raw_name, "Plans, révision 2.pdf");
    assert_eq!(first.info, back.attachments[0]);
    assert_eq!(attachment(&raw, 1).expect("decoded").bytes, b"line one\r\nline two\r\n");
    assert!(attachment(&raw, 2).is_err());
    assert!(attachment(b"", 0).is_err());

    // The same through an API (Bcc written), still with the attachments.
    let (raw, _) = build_for_api(&out).expect("built");
    assert_eq!(parse(&raw).expect("parses back").attachments.len(), 2);
}

#[test]
fn a_meeting_message_carries_its_calendar_part() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:u1@videotron.ca\r\nDTSTART:20261125T140000Z\r\nDTEND:20261125T150000Z\r\nSUMMARY:Mix\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let mut out = Outgoing {
        from: Person { name: "Alex".into(), address: "alex@videotron.ca".into() },
        to: vec![Person { name: String::new(), address: "zoe@studio.example".into() }],
        subject: "Invitation: Mix".into(),
        text: "You are invited.".into(),
        calendar: Some(CalendarPart { method: "request".into(), text: ics.into() }),
        ..Outgoing::default()
    };
    let (raw, _) = build(&out).expect("built");
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(text.contains("multipart/alternative"), "{text}");
    assert!(text.contains("text/calendar") && text.contains("method=request"), "{text}");
    let back = parse(&raw).expect("parses back");
    assert!(back.text.contains("You are invited."));
    assert_eq!(back.attachments.len(), 1, "the calendar is listed as a part");
    assert_eq!(back.attachments[0].declared_type, "text/calendar");
    let part = attachment(&raw, 0).expect("decoded");
    assert!(String::from_utf8_lossy(&part.bytes).contains("UID:u1@videotron.ca"));

    // With a file too: mixed, the calendar first.
    out.attachments = vec![OutgoingAttachment::new("agenda.txt", b"1. Mix".to_vec())];
    let (raw, _) = build(&out).expect("built");
    let back = parse(&raw).expect("parses back");
    assert_eq!(back.attachments.len(), 2);
    assert_eq!(back.attachments[1].name, "agenda.txt");

    // A method that isn't one, or a text that isn't a calendar, is refused.
    out.calendar = Some(CalendarPart { method: "REQUEST; x=y".into(), text: ics.into() });
    assert!(build(&out).is_err());
    out.calendar = Some(CalendarPart { method: "REQUEST".into(), text: "hello".into() });
    assert!(build(&out).is_err());
}

#[test]
fn an_attachment_keeps_its_announced_name_for_the_checks() {
    let a = attachment(MIXED.as_bytes(), 0).expect("decoded");
    assert_eq!(a.bytes, b"%PDF-1.4\n");
    assert_eq!(a.info.name, "prooffdp.exe");
    assert!(a.raw_name.contains('\u{202e}'), "the hidden character is still there: {:?}", a.raw_name);
    assert!(format!("{a:?}").len() < 300, "Debug leaves the content out");
}

#[test]
fn the_first_authentication_results_header_is_kept() {
    let raw = b"Authentication-Results: mx.google.com;\r\n dkim=pass header.i=@example.com;\r\n spf=pass smtp.mailfrom=a@example.com\r\n\
Authentication-Results: forged.example; dmarc=pass\r\n\
From: a@example.com\r\nSubject: x\r\n\r\nbody\r\n";
    let p = parse(raw).expect("a message");
    let h = p.authentication_results.expect("the header");
    assert!(h.contains("mx.google.com") && h.contains("dkim=pass"), "{h}");
    assert!(!h.contains("forged"));
    assert_eq!(parse(PLAIN).expect("a message").authentication_results, None);
}

#[test]
fn a_forgotten_attachment_is_noticed() {
    assert!(mentions_attachment("Please see the attached file."));
    assert!(mentions_attachment("I attach the plans"));
    assert!(mentions_attachment("Voici la pièce jointe."));
    assert!(mentions_attachment("Ci-joint le contrat."));
    assert!(mentions_attachment("Le document est joint."));
    assert!(mentions_attachment("Vous trouverez en PJ les fichiers joints"));
    assert!(!mentions_attachment("See you tomorrow.\n\n> I attached the file\n> pièce jointe"));
    assert!(!mentions_attachment("Hello\n\n---------- Forwarded message ----------\nSee attachment"));
    assert!(!mentions_attachment("Rejoint-nous au parc"), "a word that only contains \"joint\"");
}

#[test]
fn names_and_types_of_files_sent() {
    assert_eq!(outgoing_name("C:\\a\\b\"c\".pdf"), "b_c_.pdf");
    assert_eq!(outgoing_name("..."), "attachment");
    assert_eq!(outgoing_name("report\u{202e}fdp.exe"), "reportfdp.exe");
    assert_eq!(content_type_for("Photo.JPG"), "image/jpeg");
    assert_eq!(content_type_for("budget.xlsx"), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
    assert_eq!(content_type_for("mail.eml"), "application/octet-stream");
    assert_eq!(content_type_for("noextension"), "application/octet-stream");
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

/// A 1 x 1 PNG's first bytes (the content does not matter to the MIME writer).
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";

fn rich(attachments: Vec<OutgoingAttachment>, content_id: &str) -> Outgoing {
    Outgoing {
        from: Person { name: "Alex".into(), address: "alex@videotron.ca".into() },
        to: vec![Person { name: String::new(), address: "zoe@studio.example".into() }],
        subject: "Rich".into(),
        text: "Hello you\n- one".into(),
        html: Some(HtmlBody {
            html: format!("<html><body><p>Hello <b>you</b></p><ul><li>one</li></ul><p><img src=\"cid:{content_id}\"></p></body></html>"),
            images: vec![InlineImage { content_id: content_id.into(), content_type: "image/png".into(), bytes: PNG.to_vec() }],
        }),
        attachments,
        ..Outgoing::default()
    }
}

#[test]
fn an_html_message_with_a_picture_round_trips() {
    let out = rich(vec![OutgoingAttachment::new("notes.txt", b"n".to_vec())], "pic1.x@mailalm");
    let (raw, _) = build(&out).expect("built");
    assert!(raw.len() <= out.estimated_size(), "{} > {}", raw.len(), out.estimated_size());
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    for kind in ["multipart/mixed", "multipart/alternative", "multipart/related", "text/plain", "text/html", "image/png"] {
        assert!(text.contains(kind), "{kind} in {text}");
    }
    assert!(text.contains("<pic1.x@mailalm>"), "the Content-ID header: {text}");
    // The alternative comes before the attachment, the text before the HTML.
    assert!(text.find("multipart/alternative").expect("alternative") < text.find("notes.txt").expect("file"));
    assert!(text.find("text/plain").expect("plain") < text.find("text/html").expect("html"));

    let back = parse(&raw).expect("parses back");
    assert!(back.text.contains("Hello you"), "{:?}", back.text);
    assert!(!back.from_html, "the reading pane shows the plain-text alternative");
    assert!(back.attachments.iter().any(|a| a.name == "notes.txt"));
    let html = html_body(&raw).expect("the HTML part");
    assert!(html.contains("cid:pic1.x@mailalm") && html.contains("<b>you</b>"), "{html}");
    let images = related_images(&raw);
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].content_id, "pic1.x@mailalm");
    assert_eq!(images[0].content_type, "image/png");
    assert_eq!(images[0].bytes, PNG);
    assert!(format!("{:?}", images[0]).len() < 200, "Debug leaves the content out");
}

#[test]
fn an_html_message_without_attachments_is_an_alternative() {
    let out = rich(Vec::new(), "pic2@mailalm");
    let (raw, _) = build_for_api(&out).expect("built");
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(!text.contains("multipart/mixed"), "{text}");
    let header_end = text.find("\r\n\r\n").expect("a header");
    assert!(text[..header_end].contains("multipart/alternative"), "the message itself is the alternative: {text}");

    // Without pictures, no multipart/related.
    let mut plain_html = rich(Vec::new(), "pic3@mailalm");
    if let Some(h) = plain_html.html.as_mut() {
        h.images.clear();
    }
    let (raw, _) = build(&plain_html).expect("built");
    assert!(!String::from_utf8_lossy(&raw).to_ascii_lowercase().contains("multipart/related"));
    assert_eq!(html_body(PLAIN), None, "a plain message has no HTML");
    assert!(related_images(PLAIN).is_empty());
}

#[test]
fn a_picture_with_an_unsafe_content_id_or_type_is_refused() {
    assert!(build(&rich(Vec::new(), "bad id\"><script>")).is_err());
    let mut wrong_type = rich(Vec::new(), "ok@mailalm");
    if let Some(h) = wrong_type.html.as_mut() {
        h.images[0].content_type = "text/html".into();
    }
    assert!(build(&wrong_type).is_err());
}
