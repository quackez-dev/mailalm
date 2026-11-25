//! Gmail against a local test server answering canned JSON.

use std::time::{Duration, Instant};

use mailalm_model::Person;
use serde_json::{Value, json};

use super::*;
use crate::http::tests::serve;

/// An answer the client won't keep its connection for (each canned answer is a connection).
fn answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

/// [`answer`], with a `Retry-After` of one second.
fn busy_answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nRetry-After: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .into_bytes()
}

fn ok(body: &Value) -> Vec<u8> {
    answer("200 OK", &body.to_string())
}

/// A client for a test server, with a token that must never show in an error.
fn client(base: &str) -> GmailClient {
    GmailClient::with_base(base, "tok-secret-xyz")
}

/// The JSON body of a request the test server saw.
fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

#[test]
fn profile_labels_and_ids() {
    let (base, seen) = serve(vec![
        ok(&json!({"emailAddress": "zoe@gmail.example", "messagesTotal": 12, "historyId": "987654321"})),
        ok(&json!({"labels": [
            {"id": "INBOX", "name": "INBOX", "type": "system"},
            {"id": "Label_12", "name": "Work\u{202e}/Q1\r\n", "type": "user"},
            {"id": "bad id", "name": "dropped", "type": "user"},
            {"id": "Label_12", "name": "twice", "type": "user"},
            {"name": "no id"},
            {"id": "Label_13", "name": " \u{200b} "},
            {"id": "CATEGORY_SOCIAL", "name": "CATEGORY_SOCIAL", "type": "system"},
        ]})),
        // Two pages: an id that isn't one dropped, one that came twice kept once, the limit
        // reached before the next page.
        ok(&json!({"messages": [{"id": "18c1"}, {"id": "18c0"}, {"id": "not/an id"}], "nextPageToken": "08123456789/+="})),
        ok(&json!({"messages": [{"id": "18c0"}, {"id": "18bf"}, {"id": "18be"}, {"id": "18bd"}], "nextPageToken": "p3"})),
        // The spam: nothing in it, so no list at all.
        ok(&json!({"resultSizeEstimate": 0})),
        // A page token that isn't one.
        ok(&json!({"messages": [{"id": "1"}], "nextPageToken": "bad token\r\n"})),
    ]);
    let mut c = client(&base);
    assert_eq!(c.profile().unwrap(), Profile { address: "zoe@gmail.example".into(), history_id: 987_654_321 });
    let labels = c.labels().unwrap();
    let got: Vec<(&str, &str, LabelKind)> = labels.iter().map(|l| (l.id.as_str(), l.name.as_str(), l.kind)).collect();
    assert_eq!(
        got,
        [
            ("INBOX", "INBOX", LabelKind::System),
            ("Label_12", "Work/Q1", LabelKind::User),
            ("Label_13", "Label_13", LabelKind::User),
            ("CATEGORY_SOCIAL", "CATEGORY_SOCIAL", LabelKind::System),
        ]
    );
    assert_eq!(c.list_ids(&["INBOX", "Label_12"], 4).unwrap(), ["18c1", "18c0", "18bf", "18be"]);
    assert!(c.list_ids(&["SPAM"], 100).unwrap().is_empty());
    assert!(matches!(c.list_ids(&[], 10), Err(NetError::Protocol(m)) if m.contains("page token")));
    // Nothing sent for a label id that isn't one, or for no message at all.
    assert!(matches!(c.list_ids(&["INBOX", "a&b=c"], 10), Err(NetError::Invalid(_))));
    assert!(c.list_ids(&["INBOX"], 0).unwrap().is_empty());
    assert_eq!(format!("{c:?}"), "GmailClient(<redacted>)");

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 6);
    assert!(seen[0].starts_with("GET /gmail/v1/users/me/profile HTTP/1.1\r\n"), "{}", seen[0]);
    assert!(seen[0].contains("Authorization: Bearer tok-secret-xyz\r\n") && seen[0].contains("Accept: application/json\r\n"), "{}", seen[0]);
    assert!(seen[1].starts_with("GET /gmail/v1/users/me/labels "), "{}", seen[1]);
    let list = "GET /gmail/v1/users/me/messages?maxResults=";
    assert!(seen[2].starts_with(&format!("{list}4&fields=messages/id,nextPageToken&labelIds=INBOX&labelIds=Label_12 ")), "{}", seen[2]);
    assert!(
        seen[3].starts_with(&format!("{list}2&fields=messages/id,nextPageToken&labelIds=INBOX&labelIds=Label_12&pageToken=08123456789%2F%2B%3D ")),
        "{}",
        seen[3]
    );
    assert!(seen[4].starts_with(&format!("{list}100&fields=messages/id,nextPageToken&labelIds=SPAM&includeSpamTrash=true ")), "{}", seen[4]);
    assert!(seen[5].starts_with(&format!("{list}10&fields=messages/id,nextPageToken ")), "{}", seen[5]);
}

#[test]
fn metadata_makes_a_summary() {
    let first = json!({
        "id": "18c1",
        "threadId": "18c1",
        "labelIds": ["INBOX", "STARRED", "CATEGORY_PERSONAL", "bad label!"],
        "snippet": "Tom &amp; Jerry&#39;s &lt;b&gt;plan&lt;/b&gt; &quot;now&quot; &#x263A; &#8212; &bogus; &#xD800; & done\u{202e}",
        "sizeEstimate": 5_000_000_000u64,
        "internalDate": "1763906700000",
        "payload": {"mimeType": "multipart/mixed", "headers": [
            // A line end in a value starts no field of its own.
            {"name": "Subject", "value": "=?utf-8?q?R=C3=A9union?= du\r\nFrom: evil@x.example\r\n lundi"},
            {"name": "from", "value": "=?utf-8?q?Zo=C3=A9_Park?= <zoe@studio.example>"},
            {"name": "To", "value": "me@gmail.example, \"Ann B\" <ann@x.example>"},
            {"name": "Cc", "value": "=?UTF-8?B?w4lyaWM=?= <eric@y.example>"},
            {"name": "Date", "value": "Sun, 23 Nov 2025 14:05:00 +0000"},
            {"name": "Message-ID", "value": "<abc@x.example>"},
            {"name": "Reply-To", "value": "not asked for <r@x.example>"},
        ]},
    });
    let second = json!({
        "id": "18c0",
        "labelIds": ["UNREAD", "DRAFT"],
        "sizeEstimate": 2048,
        "internalDate": "1763906700000",
        "snippet": "",
        "payload": {"mimeType": "text/plain", "headers": [
            {"name": "CONTENT-TYPE", "value": "multipart/mixed; boundary=\"b1\""},
            {"name": "Subject", "value": "Hi"},
        ]},
    });
    let (base, seen) = serve(vec![
        ok(&first),
        ok(&second),
        answer("404 Not Found", r#"{"error":{"code":404,"message":"Requested entity was not found.","status":"NOT_FOUND"}}"#),
        answer("401 Unauthorized", r#"{"error":{"code":401,"message":"Invalid Credentials tok-secret-xyz","status":"UNAUTHENTICATED"}}"#),
        ok(&json!([1, 2])),
    ]);
    let mut c = client(&base);
    let m = c.metadata("18c1").unwrap().unwrap();
    assert_eq!(m.id, "18c1");
    assert_eq!(m.labels, ["INBOX", "STARRED", "CATEGORY_PERSONAL"]);
    let s = &m.summary;
    assert_eq!(s.from, Person { name: "Zoé Park".into(), address: "zoe@studio.example".into() });
    assert_eq!(s.to, [Person { name: String::new(), address: "me@gmail.example".into() }, Person { name: "Ann B".into(), address: "ann@x.example".into() }]);
    assert_eq!(s.cc, [Person { name: "Éric".into(), address: "eric@y.example".into() }]);
    assert!(s.subject.starts_with("Réunion du") && s.subject.contains("From: evil@x.example") && !s.subject.contains('\r'), "{}", s.subject);
    assert_eq!(s.date, mailalm_model::query::days_from_civil(2025, 11, 23) * 86_400 + 14 * 3600 + 5 * 60);
    assert!(s.flags.seen && s.flags.flagged && !s.flags.draft && !s.flags.answered);
    assert!(s.has_attachments && s.uid == 0 && s.size == u32::MAX);
    assert_eq!(s.preview, "Tom & Jerry's <b>plan</b> \"now\" \u{263a} \u{2014} &bogus; &#xD800; & done");
    assert_eq!(s.message_id, "abc@x.example");

    let m = c.metadata("18c0").unwrap().unwrap();
    let s = &m.summary;
    assert_eq!((s.subject.as_str(), s.date, s.size, s.preview.as_str()), ("Hi", 1_763_906_700, 2048, ""));
    assert!(!s.flags.seen && s.flags.draft && !s.flags.flagged && s.has_attachments, "{s:?}");
    assert_eq!(s.from, Person::default());

    assert_eq!(c.metadata("18bf").unwrap(), None);
    let e = c.metadata("18be").unwrap_err();
    assert!(matches!(&e, NetError::Auth(m) if m.contains("Invalid Credentials ***") && !m.contains("tok-secret")), "{e}");
    assert!(matches!(c.metadata("18bd"), Err(NetError::Protocol(_))));
    // Nothing sent for an id that isn't one.
    let long = "a".repeat(300);
    for bad in ["", "18c1/../x", "18c1?format=raw", long.as_str()] {
        assert!(matches!(c.metadata(bad), Err(NetError::Invalid(_))), "{bad}");
    }

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 5);
    let fields = "metadataHeaders=From&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Subject&metadataHeaders=Date&metadataHeaders=Message-ID&metadataHeaders=In-Reply-To&metadataHeaders=References&metadataHeaders=Content-Type";
    assert!(seen[0].starts_with(&format!("GET /gmail/v1/users/me/messages/18c1?format=metadata&{fields} HTTP/1.1\r\n")), "{}", seen[0]);
}

#[test]
fn downloads_and_changes() {
    let mime = b"From: a@b.example\r\nSubject: hi\r\n\r\nbody \xfb\xff\xbf!!\r\n";
    let raw = crate::base64::encode_url(mime);
    // Gmail's base64url, unpadded, with both of its own characters.
    assert!(raw.len() % 4 != 0 && raw.contains("-_-_"), "{raw}");
    let too_large = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", 60 << 20);
    let gone = r#"{"error":{"code":404,"message":"Requested entity was not found.","status":"NOT_FOUND"}}"#;
    let (base, seen) = serve(vec![
        ok(&json!({ "raw": raw })),
        answer("404 Not Found", gone),
        ok(&json!({"raw": "not base64!"})),
        too_large.into_bytes(),
        answer("204 No Content", ""),
        answer("204 No Content", ""),
        // The bin: one gone already, one moved.
        answer("404 Not Found", gone),
        ok(&json!({"id": "a2", "labelIds": ["TRASH"]})),
        ok(&json!({"id": "a1", "labelIds": ["INBOX"]})),
        answer("400 Bad Request", r#"{"error":{"code":400,"message":"Invalid label: Label_9","status":"INVALID_ARGUMENT"}}"#),
    ]);
    let mut c = client(&base);
    assert_eq!(c.download("18c1").unwrap().as_deref(), Some(&mime[..]));
    assert_eq!(c.download("18c2").unwrap(), None);
    assert!(matches!(c.download("18c3"), Err(NetError::Protocol(m)) if m.contains("base64url")));
    assert!(matches!(c.download("18c4"), Err(NetError::TooLarge(_))));
    let ids: Vec<String> = (0..1001).map(|i| format!("{i:x}")).collect();
    c.modify(&ids, &["STARRED"], &["UNREAD", "INBOX"]).unwrap();
    c.trash(&["a1".to_string(), "a2".to_string()]).unwrap();
    c.untrash(&["a1".to_string()]).unwrap();
    let e = c.modify(&["a1".to_string()], &["Label_9"], &[]).unwrap_err();
    assert!(matches!(&e, NetError::Server(m) if m == "HTTP 400: Invalid label: Label_9"), "{e}");
    // Nothing sent for nothing to do, or for an id that isn't one (not even for the good ones).
    c.modify(&[], &["STARRED"], &[]).unwrap();
    c.modify(&ids, &[], &[]).unwrap();
    assert!(matches!(c.modify(&["a1".to_string()], &["bad label"], &[]), Err(NetError::Invalid(_))));
    assert!(matches!(c.modify(&["a1".to_string(), "a 2".to_string()], &["STARRED"], &[]), Err(NetError::Invalid(_))));
    assert!(matches!(c.trash(&["ok".to_string(), "bad id".to_string()]), Err(NetError::Invalid(_))));
    assert!(matches!(c.download(""), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 10);
    assert!(seen[0].starts_with("GET /gmail/v1/users/me/messages/18c1?format=raw&fields=raw "), "{}", seen[0]);
    assert!(seen[4].starts_with("POST /gmail/v1/users/me/messages/batchModify ") && seen[4].contains("Content-Type: application/json\r\n"), "{}", seen[4]);
    let first = body_of(&seen[4]);
    assert_eq!(first["ids"].as_array().unwrap().len(), 1000);
    assert_eq!((&first["ids"][0], &first["addLabelIds"], &first["removeLabelIds"]), (&json!("0"), &json!(["STARRED"]), &json!(["UNREAD", "INBOX"])));
    assert_eq!(body_of(&seen[5])["ids"], json!(["3e8"]));
    assert!(seen[6].starts_with("POST /gmail/v1/users/me/messages/a1/trash "), "{}", seen[6]);
    assert!(seen[6].contains("Content-Length: 0\r\n") && !seen[6].contains("Content-Type"), "{}", seen[6]);
    assert!(seen[7].starts_with("POST /gmail/v1/users/me/messages/a2/trash "), "{}", seen[7]);
    assert!(seen[8].starts_with("POST /gmail/v1/users/me/messages/a1/untrash "), "{}", seen[8]);
    assert_eq!(body_of(&seen[9]), json!({"ids": ["a1"], "addLabelIds": ["Label_9"], "removeLabelIds": []}));
}

/// Only MailALM's own filters are read: from one address, into the spam, into the bin, or never
/// into the spam, and nothing else that does anything.
#[test]
fn filters() {
    let mine = |id: &str, from: &str, action: Value| json!({"id": id, "criteria": {"from": from}, "action": action});
    let spam = json!({"addLabelIds": ["SPAM"]});
    let (base, seen) = serve(vec![
        ok(&json!({"filter": [
            mine("ANe1Bmj1", "spam@bad.example", spam.clone()),
            mine("ANe1Bmj2", " x@bad.example ", json!({"addLabelIds": ["TRASH"], "removeLabelIds": []})),
            mine("ANe1Bmj3", "@friends.example", json!({"removeLabelIds": ["SPAM"]})),
            // Fields that say nothing don't count.
            {"id": "ANe1Bmj4", "criteria": {"from": "q@bad.example", "hasAttachment": false, "size": 0}, "action": spam},
            // Not MailALM's: another criterion, another label, a search for a sender, a
            // forward, another action, a search instead of a sender, an id that isn't one, a
            // sender that would be a negation.
            {"id": "X1", "criteria": {"from": "a@b.example", "subject": "hi"}, "action": spam},
            mine("X2", "a@b.example", json!({"addLabelIds": ["SPAM", "Label_1"]})),
            mine("X3", "a@b.example OR c@d.example", json!({"addLabelIds": ["TRASH"]})),
            mine("X4", "a@b.example", json!({"addLabelIds": ["TRASH"], "forward": "me@x.example"})),
            mine("X5", "a@b.example", json!({"removeLabelIds": ["INBOX"]})),
            {"id": "X6", "criteria": {"query": "from:a@b.example"}, "action": spam},
            mine("bad id!", "a@b.example", spam.clone()),
            mine("X7", "-a@b.example", spam.clone()),
            {"id": "X8", "action": spam},
        ]})),
        // No filter at all: no list.
        ok(&json!({})),
        answer("400 Bad Request", r#"{"error":{"code":400,"message":"Invalid label SPAM in AddLabelIds","status":"INVALID_ARGUMENT","errors":[{"reason":"invalidArgument"}]}}"#),
        ok(&json!({"id": "ANe1New", "criteria": {"from": "spam@bad.example"}, "action": {"addLabelIds": ["TRASH"]}})),
        ok(&json!({"id": "ANe1Ok", "criteria": {"from": "ok@good.example"}, "action": {"removeLabelIds": ["SPAM"]}})),
        answer("204 No Content", ""),
        answer("404 Not Found", r#"{"error":{"code":404,"message":"Filter not found","status":"NOT_FOUND"}}"#),
    ]);
    let mut c = client(&base);
    let filter = |id: &str, from: &str, action: FilterAction| Filter { id: id.into(), from: from.into(), action };
    assert_eq!(
        c.filters().unwrap(),
        [
            filter("ANe1Bmj1", "spam@bad.example", FilterAction::Spam),
            filter("ANe1Bmj2", "x@bad.example", FilterAction::Trash),
            filter("ANe1Bmj3", "@friends.example", FilterAction::NeverSpam),
            filter("ANe1Bmj4", "q@bad.example", FilterAction::Spam),
        ]
    );
    assert!(c.filters().unwrap().is_empty());
    // Gmail refusing the spam: a refusal, so the engine can ask for the bin instead.
    let e = c.create_filter(" spam@bad.example ", FilterAction::Spam).unwrap_err();
    assert!(matches!(&e, NetError::Server(m) if m == "HTTP 400: Invalid label SPAM in AddLabelIds"), "{e}");
    assert_eq!(c.create_filter("spam@bad.example", FilterAction::Trash).unwrap(), filter("ANe1New", "spam@bad.example", FilterAction::Trash));
    assert_eq!(c.create_filter("ok@good.example", FilterAction::NeverSpam).unwrap(), filter("ANe1Ok", "ok@good.example", FilterAction::NeverSpam));
    c.delete_filter("ANe1New").unwrap();
    c.delete_filter("ANe1Gone").unwrap();
    // Nothing sent for an address a filter can't name, or an id that isn't one.
    let long = format!("{}@x.example", "a".repeat(320));
    for bad in ["", "a b@x.example", "\"q\"@x.example", "nobody", "a@", "x@y@z.example", "-a@b.example", "a@b.example)", "é@x.example", "a@b\r\n.example", long.as_str()] {
        assert!(matches!(c.create_filter(bad, FilterAction::Spam), Err(NetError::Invalid(_))), "{bad}");
    }
    assert!(matches!(c.delete_filter("bad id"), Err(NetError::Invalid(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 7);
    assert!(seen[0].starts_with("GET /gmail/v1/users/me/settings/filters "), "{}", seen[0]);
    assert!(seen[2].starts_with("POST /gmail/v1/users/me/settings/filters "), "{}", seen[2]);
    assert_eq!(body_of(&seen[2]), json!({"criteria": {"from": "spam@bad.example"}, "action": {"addLabelIds": ["SPAM"]}}));
    assert_eq!(body_of(&seen[3])["action"], json!({"addLabelIds": ["TRASH"]}));
    assert_eq!(body_of(&seen[4])["action"], json!({"removeLabelIds": ["SPAM"]}));
    assert!(seen[5].starts_with("DELETE /gmail/v1/users/me/settings/filters/ANe1New "), "{}", seen[5]);
    assert!(seen[6].starts_with("DELETE /gmail/v1/users/me/settings/filters/ANe1Gone "), "{}", seen[6]);
}

/// Sending never goes twice once it may have gone: only an answer that turned it away lets it go
/// again.
#[test]
fn sending() {
    let mime = b"From: me@gmail.example\r\nTo: a@b.example\r\nBcc: c@d.example\r\nSubject: hi\r\n\r\nbody\r\n";
    let sent = json!({"id": "18d0", "threadId": "18d0", "labelIds": ["SENT"]});
    let (base, seen) = serve(vec![
        ok(&sent),
        busy_answer("429 Too Many Requests", r#"{"error":{"code":429,"message":"Too many requests","status":"RESOURCE_EXHAUSTED"}}"#),
        ok(&sent),
        answer("500 Internal Server Error", r#"{"error":{"code":500,"message":"Backend Error","status":"INTERNAL"}}"#),
        // Gone without a word once the message went: it may have been sent.
        Vec::new(),
        answer("400 Bad Request", r#"{"error":{"code":400,"message":"Invalid To header","status":"INVALID_ARGUMENT"}}"#),
    ]);
    let mut c = client(&base);
    c.send_mime(mime).unwrap();
    let started = Instant::now();
    c.send_mime(mime).unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(matches!(c.send_mime(mime), Err(NetError::Unconfirmed(m)) if m.contains("500")));
    assert!(matches!(c.send_mime(mime), Err(NetError::Unconfirmed(_))));
    assert!(matches!(c.send_mime(mime), Err(NetError::Server(m)) if m == "HTTP 400: Invalid To header"));
    assert!(matches!(c.send_mime(b""), Err(NetError::Invalid(_))));
    assert!(matches!(c.send_mime(&vec![b'a'; MAX_SEND + 1]), Err(NetError::TooLarge(_))));

    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 6);
    for s in &seen {
        assert!(s.starts_with("POST /upload/gmail/v1/users/me/messages/send?uploadType=media HTTP/1.1\r\n"), "{s}");
        assert!(s.contains("Content-Type: message/rfc822\r\n") && s.contains("Authorization: Bearer tok-secret-xyz\r\n"), "{s}");
        assert!(s.ends_with(std::str::from_utf8(mime).unwrap()), "{s}");
    }
}

/// Busy answers (429, 503, a 403 for a rate limit) go again after `Retry-After`; a refusal and a
/// refused token don't.
#[test]
fn busy_and_refused() {
    let (base, seen) = serve(vec![
        busy_answer("403 Forbidden", r#"{"error":{"code":403,"message":"User-rate limit exceeded.","errors":[{"reason":"userRateLimitExceeded"}]}}"#),
        ok(&json!({"emailAddress": "zoe@gmail.example", "historyId": 5})),
        answer("403 Forbidden", r#"{"error":{"code":403,"message":"Request had insufficient authentication scopes.","status":"PERMISSION_DENIED","errors":[{"reason":"insufficientPermissions"}]}}"#),
        answer("401 Unauthorized", r#"{"error":{"code":401,"message":"Request had invalid authentication credentials.","status":"UNAUTHENTICATED"}}"#),
        busy_answer("503 Service Unavailable", ""),
        ok(&json!({"labels": []})),
    ]);
    let mut c = client(&base);
    let started = Instant::now();
    assert_eq!(c.profile().unwrap(), Profile { address: "zoe@gmail.example".into(), history_id: 5 });
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(matches!(c.labels(), Err(NetError::Server(m)) if m == "HTTP 403: Request had insufficient authentication scopes."));
    let e = c.profile().unwrap_err();
    assert!(e.is_auth() && e.to_string().contains("Google refused the access token"), "{e}");
    c.set_token("tok-two");
    assert!(c.labels().unwrap().is_empty());
    let seen = seen.join().unwrap();
    assert_eq!(seen.len(), 6);
    assert!(seen[5].contains("Authorization: Bearer tok-two\r\n"), "{}", seen[5]);

    // What each status means.
    let rate = json!({"error": {"code": 403, "errors": [{"reason": "rateLimitExceeded"}]}});
    assert!(matches!(status_error(401, None), Err(NetError::Auth(_))));
    for status in [429, 500, 502, 503, 504] {
        assert!(matches!(status_error(status, None), Err(NetError::Protocol(m)) if m.contains("busy")), "{status}");
    }
    assert!(matches!(status_error(403, Some(&rate)), Err(NetError::Protocol(m)) if m.contains("busy")));
    assert!(matches!(status_error(403, None), Err(NetError::Server(m)) if m == "HTTP 403"));
    assert!(matches!(status_error(404, Some(&json!({"error": {"status": "NOT_FOUND"}}))), Err(NetError::Server(m)) if m == "HTTP 404: NOT_FOUND"));
    assert!(status_error(204, None).is_ok());
    let resp = |status: u16, body: &Value| Response { status, headers: Vec::new(), body: body.to_string().into_bytes() };
    assert_eq!(busy(&resp(503, &Value::Null)), Some(Busy::TurnedAway));
    assert_eq!(busy(&resp(403, &rate)), Some(Busy::TurnedAway));
    assert_eq!(busy(&resp(502, &Value::Null)), Some(Busy::Failed));
    assert_eq!(busy(&resp(403, &json!({"error": {"errors": [{"reason": "dailyLimitExceeded"}]}}))), None);
    assert_eq!(busy(&resp(400, &Value::Null)), None);
    assert_eq!((backoff_secs(Some("90"), 1), backoff_secs(Some("0"), 1), backoff_secs(None, 3), backoff_secs(Some("x"), 1)), (20, 1, 6, 2));
}

#[test]
fn ids_text_and_numbers() {
    for good in ["18c1a2b3c4d5e6f7", "INBOX", "Label_12", "CATEGORY_SOCIAL", "a-b"] {
        assert!(convert::id_ok(good), "{good}");
    }
    let long = "a".repeat(257);
    for bad in ["", "a b", "a/b", "a%2F", "é", "a\r\n", long.as_str()] {
        assert!(!convert::id_ok(bad), "{bad}");
    }
    assert!(convert::filter_id_ok("ANe1Bmj=.x-_") && !convert::filter_id_ok("a b") && !convert::filter_id_ok("a/b"));
    assert!(convert::page_token_ok("0812/+=") && !convert::page_token_ok("") && !convert::page_token_ok("a b") && !convert::page_token_ok(&"a".repeat(1025)));
    assert!(convert::address_ok("o'brien@x.example") && convert::address_ok("@domain.example"));
    assert_eq!(convert::entities("&amp;amp; &#128512; &#x; &#1114112; a&b &AMP; &"), "&amp; \u{1f600} &#x; &#1114112; a&b &AMP; &");
    assert_eq!(convert::entities("&lt;&gt;&quot;&apos;&nbsp;&#39;&#X41;é&amp;"), "<>\"' 'Aé&");
    assert_eq!(convert::clean("  a\u{200f}b\u{0}c\u{1b}[31m  ", 100), "ab c [31m");
    assert_eq!(convert::clean(&"é".repeat(50), 10), "é".repeat(10));
    assert_eq!((convert::number(Some(&json!("123"))), convert::number(Some(&json!(5)))), (Some(123), Some(5)));
    for bad in [json!("-1"), json!("x"), json!(-1), json!(null)] {
        assert_eq!(convert::number(Some(&bad)), None, "{bad}");
    }
}
