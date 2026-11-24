//! Graph against a local test server answering canned JSON.

use serde_json::json;

use super::*;
use crate::http::tests::{serve, serve_sessions};

/// An answer the client won't keep its connection for (each canned answer is a connection).
fn answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn ok(body: &Value) -> Vec<u8> {
    answer("200 OK", &body.to_string())
}

/// A test server's address, and a client for it as for Graph (`…/v1.0`).
fn client(base: &str) -> (String, GraphClient) {
    let graph = format!("{base}/v1.0");
    let c = GraphClient::with_base(&graph, "tok-secret-xyz");
    (graph, c)
}

/// A `$batch` answer: (status, body) per request, numbered from 1.
fn batch_reply(entries: &[(u16, Value)]) -> Vec<u8> {
    let responses: Vec<Value> =
        entries.iter().enumerate().map(|(i, (status, body))| json!({"id": (i + 1).to_string(), "status": status, "body": body})).collect();
    ok(&json!({ "responses": responses }))
}

fn folder(id: &str, name: &str, children: u64, total: u64, unread: u64) -> Value {
    json!({"id": id, "displayName": name, "childFolderCount": children, "totalItemCount": total, "unreadItemCount": unread})
}

/// The whole folder list: roles from the well-known names, paths through the children, names
/// cleaned, a name taken twice made unique, and the pane's order.
#[test]
fn folders_map_to_the_model() {
    let not_found = json!({"error": {"code": "ErrorFolderNotFound", "message": "not found"}});
    let (base, seen) = serve_with_links(move |base| {
        vec![
            batch_reply(&[
                (200, json!({"id": "IN"})),
                (200, json!({"id": "DR"})),
                (200, json!({"id": "SE"})),
                (200, json!({"id": "DE"})),
                (200, json!({"id": "JU"})),
                (404, not_found.clone()),
            ]),
            ok(&json!({"value": [
                folder("IN", "Inbox", 1, 120, 3),
                folder("DR", "Drafts", 0, 2, 0),
                folder("SE", "Sent Items", 0, 50, 0),
                folder("DE", "Deleted Items", 0, 7, 1),
                folder("JU", "Junk Email", 0, 9, 9),
                folder("PR", "Projects/2026", 1, 0, 0),
                folder("BIDI", "\u{202e}evil\r\n", 0, 0, 0),
                {"displayName": "no id"},
            ], "@odata.nextLink": format!("{base}/v1.0/me/mailFolders?$skiptoken=2")})),
            ok(&json!({"value": [folder("EMPTY", " \u{200b} ", 0, 0, 0), folder("DUP", "Drafts", 0, 1, 0)]})),
            ok(&json!({"value": [folder("WK", "Work", 0, 4, 2)]})),
            ok(&json!({"value": [folder("Q1", "Q1", 0, 0, 0)]})),
        ]
    });
    let (_, mut c) = client(&base);
    let folders = c.folders().unwrap();
    let got: Vec<(&str, &str, FolderRole, u32, u32)> = folders.iter().map(|f| (f.path.as_str(), f.wire.as_str(), f.role, f.total, f.unread)).collect();
    assert_eq!(
        got,
        [
            ("Inbox", "IN", FolderRole::Inbox, 120, 3),
            ("Drafts", "DR", FolderRole::Drafts, 2, 0),
            ("Sent Items", "SE", FolderRole::Sent, 50, 0),
            ("Junk Email", "JU", FolderRole::Junk, 9, 9),
            ("Deleted Items", "DE", FolderRole::Trash, 7, 1),
            ("(unnamed)", "EMPTY", FolderRole::Other, 0, 0),
            ("Drafts (2)", "DUP", FolderRole::Other, 1, 0),
            ("evil", "BIDI", FolderRole::Other, 0, 0),
            ("Inbox/Work", "WK", FolderRole::Other, 4, 2),
            ("Projects\u{2215}2026", "PR", FolderRole::Other, 0, 0),
            ("Projects\u{2215}2026/Q1", "Q1", FolderRole::Other, 0, 0),
        ]
    );
    assert!(folders.iter().all(|f| f.delimiter == Some('/')));
    assert_eq!(folders.iter().find(|f| f.wire == "Q1").map(|f| (f.name(), f.depth())), Some(("Q1", 1)));
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("POST /v1.0/$batch HTTP/1.1"), "{}", seen[0]);
    assert!(seen[0].contains("\"url\":\"/me/mailFolders/inbox?$select=id\"") && seen[0].contains("ImmutableId"), "{}", seen[0]);
    assert!(seen[1].starts_with(&format!("GET /v1.0/me/mailFolders?$top=100&$select={FOLDER_FIELDS} ")), "{}", seen[1]);
    assert!(seen[1].contains("Authorization: Bearer tok-secret-xyz\r\n") && seen[1].contains("Prefer: IdType=\"ImmutableId\"\r\n"), "{}", seen[1]);
    assert!(seen[2].starts_with("GET /v1.0/me/mailFolders?$skiptoken=2 "), "{}", seen[2]);
    assert!(seen[3].starts_with("GET /v1.0/me/mailFolders/IN/childFolders?$top=100"), "{}", seen[3]);
    assert!(seen[4].starts_with("GET /v1.0/me/mailFolders/PR/childFolders?$top=100"), "{}", seen[4]);
}

/// [`serve`], with answers that can name the server's own address (a nextLink).
fn serve_with_links(make: impl FnOnce(&str) -> Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let answers = make(&base);
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for a in answers {
            let s = crate::http::tests::accept_within(&l, crate::http::tests::TEST_SERVER_WAIT).expect("a request");
            let mut reader = std::io::BufReader::new(s.try_clone().unwrap());
            seen.push(crate::http::tests::read_request(&mut reader).expect("a whole request"));
            let _ = std::io::Write::write_all(&mut &s, &a);
        }
        seen
    });
    (base, h)
}

fn message() -> Value {
    json!({
        "id": "M1",
        "subject": "Hello\u{202e}\r\nworld",
        "from": {"emailAddress": {"name": "Zoe Park", "address": "zoe@studio.example"}},
        "toRecipients": [{"emailAddress": {"name": "", "address": "me@outlook.example"}}, {"emailAddress": {}}],
        "ccRecipients": [{"emailAddress": {"name": "Ann", "address": "ann@x.example"}}],
        "receivedDateTime": "2026-11-23T14:05:00Z",
        "sentDateTime": "2026-11-23T14:04:00Z",
        "isRead": true,
        "flag": {"flagStatus": "flagged"},
        "isDraft": "false",
        "hasAttachments": true,
        "bodyPreview": "Line one\r\n\r\n  Line\ttwo",
        "internetMessageId": "<abc@x.example>",
    })
}

#[test]
fn delta_pages() {
    let (base, seen) = serve_with_links(|base| {
        let graph = format!("{base}/v1.0");
        vec![
            ok(&json!({"value": [
                message(),
                {"id": "GONE", "@removed": {"reason": "deleted"}},
                {"id": "bad id", "subject": "dropped"},
                {"id": "M2", "sender": {"emailAddress": {"address": "s@x.example"}}, "sentDateTime": "2026-11-23T09:04:00-05:00", "isRead": "true"},
            ], "@odata.nextLink": format!("{graph}/me/mailfolders('IN')/messages/delta?$skiptoken=abc")})),
            ok(&json!({"value": [], "@odata.deltaLink": format!("{graph}/me/mailfolders('IN')/messages/delta?$deltatoken=xyz")})),
            ok(&json!({"value": [], "@odata.nextLink": "https://evil.example/v1.0/me/messages"})),
            answer("410 Gone", ""),
            answer("400 Bad Request", r#"{"error":{"code":"SyncStateNotFound","message":"The sync state was not found."}}"#),
            answer("401 Unauthorized", r#"{"error":{"code":"InvalidAuthenticationToken","message":"Token tok-secret-xyz has expired."}}"#),
            ok(&json!({"value": []})),
        ]
    });
    let (graph, mut c) = client(&base);
    let page = c.delta("IN", None).unwrap();
    assert_eq!(page.removed, ["GONE"]);
    assert_eq!(page.changed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["M1", "M2"]);
    let s = &page.changed[0].summary;
    assert_eq!((s.from.name.as_str(), s.from.address.as_str()), ("Zoe Park", "zoe@studio.example"));
    assert_eq!(s.to.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(), ["me@outlook.example"]);
    assert_eq!(s.cc.len(), 1);
    assert_eq!(s.subject, "Hello  world");
    assert_eq!(s.date, mailalm_model::query::days_from_civil(2026, 11, 23) * 86_400 + 14 * 3600 + 5 * 60);
    assert!(s.flags.seen && s.flags.flagged && !s.flags.draft && !s.flags.answered);
    assert!(s.has_attachments && s.uid == 0 && s.size == 0);
    assert_eq!(s.preview, "Line one Line two");
    assert_eq!(s.message_id, "abc@x.example");
    let s = &page.changed[1].summary;
    assert_eq!(s.from.address, "s@x.example", "the sender, without a from");
    assert_eq!(s.date, mailalm_model::query::days_from_civil(2026, 11, 23) * 86_400 + 14 * 3600 + 4 * 60, "the sent date, its offset applied");
    assert!(s.flags.seen);
    let next = match &page.next {
        DeltaNext::More(next) => next.clone(),
        other => panic!("{other:?}"),
    };
    assert!(next.ends_with("$skiptoken=abc"));

    let page = c.delta("IN", Some(&next)).unwrap();
    assert_eq!(page.next, DeltaNext::Done(format!("{graph}/me/mailfolders('IN')/messages/delta?$deltatoken=xyz")));
    // A link that leads elsewhere is neither followed nor given back.
    assert!(matches!(c.delta("IN", Some(&next)), Err(NetError::Protocol(m)) if m.contains("lead back")));
    assert!(matches!(c.delta("IN", Some("https://evil.example/v1.0/x")), Err(NetError::Protocol(_))));
    assert!(matches!(c.delta("IN", Some(&format!("{graph}evil.example/x"))), Err(NetError::Protocol(_))));
    // An expired delta link: the pass starts again; not a broken connection.
    let e = c.delta("IN", Some(&next)).unwrap_err();
    assert!(matches!(&e, NetError::Resync(m) if m.contains("410")) && !e.is_fatal(), "{e}");
    assert!(matches!(c.delta("IN", Some(&next)), Err(NetError::Resync(m)) if m.contains("sync state")));
    // The token refused: an Auth error that doesn't repeat it.
    let e = c.delta("IN", Some(&next)).unwrap_err();
    assert!(matches!(&e, NetError::Auth(m) if m.contains("***") && !m.contains("tok-secret")), "{e}");
    assert!(matches!(c.delta("IN", Some(&next)), Err(NetError::Protocol(m)) if m.contains("neither")));
    assert!(matches!(c.delta("bad id", None), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 7);
    assert!(seen[0].starts_with(&format!("GET /v1.0/me/mailFolders/IN/messages/delta?$select={DELTA_FIELDS}&$orderby=receivedDateTime%20desc ")), "{}", seen[0]);
    assert!(seen[0].contains("Prefer: IdType=\"ImmutableId\", odata.maxpagesize=200\r\n"), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /v1.0/me/mailfolders('IN')/messages/delta?$skiptoken=abc "), "{}", seen[1]);
}

#[test]
fn dates_from_graph() {
    let day = mailalm_model::query::days_from_civil(2026, 11, 23) * 86_400;
    for (text, at) in [
        ("2026-11-23T14:05:00Z", day + 50_700),
        ("2026-11-23T14:05:00.0000000Z", day + 50_700),
        ("2026-11-23T14:05:00", day + 50_700),
        ("2026-11-23T09:05:00-05:00", day + 50_700),
        ("2026-11-23T15:35:00+0130", day + 50_700),
    ] {
        assert_eq!(convert::utc_seconds(text), Some(at), "{text}");
    }
    for bad in ["", "2026-11-23", "2026-13-01T00:00:00Z", "2026-11-23T24:00:00Z", "2026-11-23T14:05:00.Z", "2026-11-23T14:05:00+5", "２０２６-11-23T14:05:00Z", "0000-01-01T00:00:00Z"] {
        assert_eq!(convert::utc_seconds(bad), None, "{bad}");
    }
}

#[test]
fn text_from_graph_is_cleaned() {
    assert_eq!(convert::clean_text("  a\u{200f}b\u{0}c\u{1b}[31m  ", 100), "ab c [31m");
    assert_eq!(convert::clean_text(&"é".repeat(50), 10), "é".repeat(10));
    assert_eq!(convert::folder_name("a/b"), "a\u{2215}b");
    assert_eq!(convert::folder_name("\u{feff}"), "(unnamed)");
    assert!(convert::id_ok("AAMkAD-_=+/") && !convert::id_ok("") && !convert::id_ok("a b") && !convert::id_ok(&"a".repeat(2000)));
}

/// Changes go as `$batch`es of at most 20; a message gone counts as done for a delete, a move or
/// a junk report, not for a mark; entries Graph was busy for go again on their own.
#[test]
fn batches() {
    let gone = json!({"error": {"code": "ErrorItemNotFound", "message": "The specified object was not found in the store."}});
    let busy = |after: &str| json!({"id": "2", "status": 429, "headers": {"retry-after": after}, "body": {}});
    let ids = |n: usize| (0..n).map(|i| format!("M{i}")).collect::<Vec<String>>();
    let (base, seen) = serve(vec![
        // delete: one gone.
        batch_reply(&[(204, Value::Null), (404, gone.clone())]),
        // set_read: one gone, refused.
        batch_reply(&[(200, json!({})), (404, gone.clone()), (200, json!({}))]),
        // move_to: 21 messages, two batches.
        batch_reply(&vec![(201, json!({})); 20]),
        batch_reply(&[(201, json!({}))]),
        // mark_junk: the second busy, then done on its own.
        ok(&json!({"responses": [{"id": "1", "status": 200, "body": {}}, busy("1")]})),
        ok(&json!({"responses": [{"id": "2", "status": 200, "body": {}}]})),
        // set_flagged: an answer missing.
        ok(&json!({"responses": [{"id": "1", "status": 200, "body": {}}]})),
        // delete: the token refused in an entry.
        batch_reply(&[(401, json!({"error": {"code": "InvalidAuthenticationToken", "message": "expired"}}))]),
    ]);
    let (_, mut c) = client(&base);
    c.delete(&ids(2)).unwrap();
    let e = c.set_read(&ids(3), true).unwrap_err();
    assert!(matches!(&e, NetError::Server(m) if m.starts_with("1 of 3 changes were refused: HTTP 404")), "{e}");
    c.move_to(&ids(21), "ARCHIVE").unwrap();
    c.mark_junk(&ids(2), true).unwrap();
    assert!(matches!(c.set_flagged(&ids(2), true), Err(NetError::Protocol(m)) if m.contains("unanswered")));
    assert!(matches!(c.delete(&ids(1)), Err(NetError::Auth(_))));
    // Nothing sent for nothing, or for an id that isn't one.
    c.delete(&[]).unwrap();
    assert!(matches!(c.delete(&["a b".to_string()]), Err(NetError::Invalid(_))));
    assert!(matches!(c.move_to(&ids(1), ""), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 8);
    let body = |i: usize| -> Value { serde_json::from_str(seen[i].split("\r\n\r\n").nth(1).unwrap()).unwrap() };
    assert_eq!(body(0)["requests"][1], json!({"id": "2", "method": "DELETE", "url": "/me/messages/M1", "headers": {"Prefer": "IdType=\"ImmutableId\""}}));
    assert_eq!(body(1)["requests"][0]["body"], json!({"isRead": true}));
    assert_eq!(body(1)["requests"][0]["headers"]["Content-Type"], "application/json");
    assert_eq!(body(2)["requests"].as_array().unwrap().len(), 20);
    assert_eq!(body(2)["requests"][0]["url"], "/me/messages/M0/move");
    assert_eq!(body(2)["requests"][0]["body"], json!({"destinationId": "ARCHIVE"}));
    assert_eq!(body(3)["requests"][0]["url"], "/me/messages/M20/move");
    assert_eq!(body(4)["requests"][1]["url"], "/me/messages/M1/reportMessage");
    assert_eq!(body(4)["requests"][1]["body"], json!({"IsMessageMoveRequested": true, "ReportAction": "junk"}));
    // Only the busy one went again.
    assert_eq!(body(5)["requests"].as_array().unwrap().len(), 1);
    assert_eq!(body(5)["requests"][0]["id"], "2");
    assert_eq!(body(6)["requests"][0]["body"], json!({"flag": {"flagStatus": "flagged"}}));
}

#[test]
fn downloads_me_and_sending() {
    let mime = b"From: a@b.example\r\nSubject: hi\r\n\r\nbody\r\n";
    let (base, seen) = serve_sessions(vec![
        vec![[format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", mime.len()).as_bytes(), &mime[..]].concat()],
        vec![answer("404 Not Found", r#"{"error":{"code":"ErrorItemNotFound","message":"gone"}}"#)],
        vec![answer("503 Service Unavailable", "")],
        vec![answer("200 OK", r#"{"mail":null,"userPrincipalName":"zoe@outlook.example","displayName":"Zoe\u0007 Park"}"#)],
        vec![answer("202 Accepted", "")],
        vec![answer("400 Bad Request", r#"{"error":{"code":"ErrorMimeContentInvalidBase64String","message":"Invalid base64 string for MIME content."}}"#)],
        // Gone without a word once the message went: it may have been sent.
        vec![Vec::new()],
        vec![answer("403 Forbidden", r#"{"error":{"code":"ErrorAccessDenied","message":"Access is denied."}}"#)],
    ]);
    let (_, mut c) = client(&base);
    assert_eq!(c.download("M1").unwrap().as_deref(), Some(&mime[..]));
    assert_eq!(c.download("M2").unwrap(), None);
    // Busy, then the user (the 503's Retry-After is absent: two seconds' wait).
    let started = std::time::Instant::now();
    let me = c.me().unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(me, Me { address: "zoe@outlook.example".into(), name: "Zoe  Park".into() });
    c.send_mime(mime).unwrap();
    assert!(matches!(c.send_mime(mime), Err(NetError::Server(m)) if m.contains("Invalid base64")));
    assert!(matches!(c.send_mime(mime), Err(NetError::Unconfirmed(_))));
    assert!(matches!(c.send_mime(b""), Err(NetError::Invalid(_))));
    assert!(matches!(c.me(), Err(NetError::Server(m)) if m == "HTTP 403: Access is denied."));
    assert_eq!(format!("{c:?}"), "GraphClient(<redacted>)");

    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("GET /v1.0/me/messages/M1/$value ") && seen[0].contains("Accept: */*\r\n"), "{}", seen[0]);
    assert!(seen[3].starts_with("GET /v1.0/me?$select=mail,userPrincipalName,displayName "), "{}", seen[3]);
    assert!(seen[4].starts_with("POST /v1.0/me/sendMail ") && seen[4].contains("Content-Type: text/plain\r\n"), "{}", seen[4]);
    assert!(seen[4].ends_with(&crate::base64::encode(mime)), "{}", seen[4]);
}

#[test]
fn errors_from_statuses() {
    assert!(matches!(status_error(401, None), Err(NetError::Auth(_))));
    for busy in [429, 503, 504, 500] {
        assert!(matches!(status_error(busy, None), Err(NetError::Protocol(_))), "{busy}");
    }
    let denied = json!({"error": {"code": "ErrorAccessDenied", "message": "Access is denied."}});
    assert!(matches!(status_error(403, Some(&denied)), Err(NetError::Server(m)) if m == "HTTP 403: Access is denied."));
    assert!(matches!(status_error(409, Some(&json!({"error": {"code": "Conflict"}}))), Err(NetError::Server(m)) if m == "HTTP 409: Conflict"));
    assert!(status_error(204, None).is_ok());
    for headers in [json!({"Retry-after": "7"}), json!({"RETRY-AFTER": 7}), json!({"retry-After": "7"})] {
        assert_eq!(entry_retry_after(&json!({ "headers": headers })).as_deref(), Some("7"));
    }
    assert_eq!((backoff_secs(Some("90"), 1), backoff_secs(Some("0"), 1), backoff_secs(None, 3), backoff_secs(Some("x"), 1)), (20, 1, 6, 2));
    let nested = json!({"error": {"code": "BadRequest", "innerError": {"code": "syncStateInvalid"}}});
    assert!(convert::error_codes(Some(&nested)).iter().any(|c| resync_code(c)));
    assert!(!resync_code("ErrorItemNotFound"));
}
