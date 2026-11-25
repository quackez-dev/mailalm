//! Outlook's rules, the search and folder making, both ways and against a local test server.

use mailalm_model::FolderRole;
use serde_json::json;

use super::*;
use crate::http::tests::serve;

fn answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn ok(body: &Value) -> Vec<u8> {
    answer("200 OK", &body.to_string())
}

fn client(base: &str) -> GraphClient {
    GraphClient::with_base(&format!("{base}/v1.0"), "tok-secret-xyz")
}

fn folders() -> Vec<Folder> {
    let f = |path: &str, wire: &str, role| Folder { path: path.into(), wire: wire.into(), delimiter: Some('/'), role, total: 0, unread: 0 };
    vec![f("Inbox", "IN", FolderRole::Inbox), f("Inbox/News", "NEWS", FolderRole::Other), f("Deleted Items", "DE", FolderRole::Trash)]
}

fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn graph_rule() -> Value {
    json!({
        "id": "AQAAAJ5dZp8=",
        "displayName": "Newsletters\u{202e}",
        "sequence": 2,
        "isEnabled": true,
        "conditions": {
            "senderContains": ["news@", "promo@"],
            "subjectContains": ["sale"],
            "hasAttachments": false,
            "bodyOrSubjectContains": [],
        },
        "actions": {"moveToFolder": "NEWS", "markAsRead": true, "assignCategories": ["Red", "Shops"], "stopProcessingRules": true},
    })
}

#[test]
fn graph_rules_read_as_the_model() {
    let r = rule_of(&graph_rule(), &folders()).unwrap();
    assert_eq!(
        r,
        Rule {
            id: "AQAAAJ5dZp8=".into(),
            name: "Newsletters".into(),
            place: Where::Server,
            enabled: true,
            conditions: vec![Condition::From(vec!["news@".into(), "promo@".into()]), Condition::Subject(vec!["sale".into()])],
            // In the order of Graph's keys.
            actions: vec![Action::Category("Red".into()), Action::Category("Shops".into()), Action::MarkRead, Action::MoveTo("Inbox/News".into())],
            stop: true,
            forward_confirmed: false,
            unsupported: vec![],
        }
    );
    assert!(!r.read_only());
    // Back to Graph: the same conditions and actions, the folder by its id.
    let back = rule_json(&r, &folders(), Some(7)).unwrap();
    assert_eq!(
        back,
        json!({
            "displayName": "Newsletters",
            "isEnabled": true,
            "sequence": 7,
            "conditions": {"senderContains": ["news@", "promo@"], "subjectContains": ["sale"]},
            "actions": {"moveToFolder": "NEWS", "markAsRead": true, "assignCategories": ["Red", "Shops"], "stopProcessingRules": true},
        })
    );
    // And read again, the same rule.
    let mut again = back.clone();
    again["id"] = json!("AQAAAJ5dZp8=");
    assert_eq!(rule_of(&again, &folders()), Some(r));
}

#[test]
fn rules_made_elsewhere_are_read_only() {
    let odd = json!({
        "id": "R2",
        "displayName": "Odd",
        "isReadOnly": true,
        "conditions": {"fromAddresses": [{"emailAddress": {"name": "Zoe", "address": "zoe@studio.example"}}], "headerContains": ["X-Spam"], "importance": "high"},
        "exceptions": {"subjectContains": ["urgent"]},
        "actions": {"moveToFolder": "GONE", "copyToFolder": "NEWS", "markImportance": "low", "forwardTo": [{"emailAddress": {"address": "me@elsewhere.example"}}], "permanentDelete": true},
    });
    let r = rule_of(&odd, &folders()).unwrap();
    assert_eq!(r.conditions, vec![Condition::From(vec!["zoe@studio.example".into()])]);
    assert_eq!(r.actions, vec![Action::Forward(vec!["me@elsewhere.example".into()])]);
    assert!(!r.forward_confirmed, "a forwarding read from the server is confirmed again before it is saved");
    for part in ["Outlook keeps it read only", "exceptions", "condition headerContains", "condition importance", "moves to a folder MailALM doesn't know", "action copyToFolder", "action markImportance", "action permanentDelete"] {
        assert!(r.unsupported.iter().any(|u| u == part), "{part}: {:?}", r.unsupported);
    }
    assert!(r.read_only());
    assert!(rule_json(&r, &folders(), None).is_err(), "never written back");
    // No id, a hostile id, a list too long: left out, or unsupported.
    assert!(rule_of(&json!({"displayName": "x"}), &folders()).is_none());
    assert!(rule_of(&json!({"id": "a b\r\n"}), &folders()).is_none());
    let long: Vec<String> = (0..30).map(|n| format!("w{n}")).collect();
    let r = rule_of(&json!({"id": "R3", "displayName": "", "conditions": {"subjectContains": long}, "actions": {"delete": true}}), &folders()).unwrap();
    assert_eq!((r.name.as_str(), r.unsupported.as_slice()), ("(unnamed)", &["condition subjectContains".to_owned()][..]));
}

#[test]
fn what_graph_is_not_sent() {
    let base = rule_of(&graph_rule(), &folders()).unwrap();
    // A folder that isn't there, two conditions of a kind, a flag.
    assert!(rule_json(&Rule { actions: vec![Action::MoveTo("Nowhere".into())], ..base.clone() }, &folders(), None).is_err());
    let two = Rule { conditions: vec![Condition::From(vec!["a".into()]), Condition::From(vec!["b".into()])], ..base.clone() };
    assert!(rule_json(&two, &folders(), None).is_err());
    assert!(rule_json(&Rule { actions: vec![Action::Flag], ..base.clone() }, &folders(), None).is_err());
    // A forwarding confirmed goes as Graph's recipients.
    let fwd = Rule { actions: vec![Action::Forward(vec!["me@elsewhere.example".into()])], forward_confirmed: true, stop: false, ..base };
    let v = rule_json(&fwd, &folders(), None).unwrap();
    assert_eq!(v["actions"], json!({"forwardTo": [{"emailAddress": {"address": "me@elsewhere.example"}}], "stopProcessingRules": false}));
}

#[test]
fn rules_on_the_server() {
    let second = json!({"id": "R1", "displayName": "First", "sequence": 1, "conditions": {"subjectContains": ["x"]}, "actions": {"delete": true}});
    let made = json!({"id": "R9", "displayName": "Newsletters", "sequence": 3, "conditions": {"senderContains": ["news@"]}, "actions": {"markAsRead": true}});
    let (base, seen) = serve(vec![
        ok(&json!({"value": [graph_rule(), second.clone()]})),
        ok(&json!({"value": [graph_rule(), second]})),
        answer("201 Created", &made.to_string()),
        ok(&made),
        answer("404 Not Found", r#"{"error":{"code":"ErrorItemNotFound","message":"gone"}}"#),
    ]);
    let mut c = client(&base);
    let rules = c.rules(&folders()).unwrap();
    assert_eq!(rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["R1", "AQAAAJ5dZp8="], "in Graph's order");
    let new = Rule::new("Newsletters", Where::Server, vec![Condition::From(vec!["news@".into()])], vec![Action::MarkRead]);
    assert_eq!(c.create_rule(&new, &folders()).unwrap().id, "R9");
    let changed = Rule { id: "R9".into(), ..new.clone() };
    assert_eq!(c.update_rule(&changed, &folders()).unwrap().name, "Newsletters");
    c.delete_rule("R9").unwrap();
    assert!(matches!(c.delete_rule("bad id"), Err(NetError::Invalid(_))));
    assert!(matches!(c.create_rule(&Rule { place: Where::Local, ..new }, &folders()), Err(NetError::Invalid(_))));
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("GET /v1.0/me/mailFolders/inbox/messageRules "), "{}", seen[0]);
    assert!(seen[2].starts_with("POST /v1.0/me/mailFolders/inbox/messageRules "), "{}", seen[2]);
    assert_eq!(body_of(&seen[2])["sequence"], json!(3), "after the others");
    assert!(seen[3].starts_with("PATCH /v1.0/me/mailFolders/inbox/messageRules/R9 "), "{}", seen[3]);
    assert!(body_of(&seen[3]).get("sequence").is_none());
    assert!(seen[4].starts_with("DELETE /v1.0/me/mailFolders/inbox/messageRules/R9 "), "{}", seen[4]);
}

#[test]
fn folders_are_made_and_the_mailbox_searched() {
    let found = json!({"value": [
        {"id": "M1", "parentFolderId": "IN", "subject": "Quote", "from": {"emailAddress": {"name": "Zoe", "address": "zoe@studio.example"}},
         "receivedDateTime": "2026-11-20T10:00:00Z", "isRead": false, "conversationId": "CONV1", "categories": ["Red\u{202e}", ""]},
        {"id": "M2", "subject": "no folder"},
        {"id": "bad id", "parentFolderId": "IN"},
    ]});
    let (base, seen) = serve(vec![
        answer("201 Created", r#"{"id":"MAILALM"}"#),
        answer("201 Created", r#"{"id":"SNOOZED"}"#),
        answer("409 Conflict", r#"{"error":{"code":"ErrorFolderExists","message":"exists"}}"#),
        ok(&found),
    ]);
    let mut c = client(&base);
    assert_eq!(c.create_folder(None, "MailALM").unwrap(), "MAILALM");
    assert_eq!(c.create_folder(Some("MAILALM"), "Snoozed").unwrap(), "SNOOZED");
    assert!(matches!(c.create_folder(None, "MailALM"), Err(NetError::Server(_))));
    assert!(matches!(c.create_folder(None, "a/b"), Err(NetError::Invalid(_))));
    let q = Query::parse(r#"from:zoe "storyboard quote" has:attachment after:2026-11-01 is:unread"#);
    let hits = c.search(&q, Some("IN"), 100).unwrap();
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!((hit.folder_id.as_str(), hit.message.id.as_str(), hit.message.summary.subject.as_str()), ("IN", "M1", "Quote"));
    assert_eq!((hit.message.summary.thread.as_str(), hit.message.summary.categories.as_slice()), ("CONV1", &["Red".to_owned()][..]));
    assert!(matches!(c.search(&Query::parse("is:unread"), None, 10), Err(NetError::Invalid(_))));
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("POST /v1.0/me/mailFolders ") && body_of(&seen[0]) == json!({"displayName": "MailALM"}), "{}", seen[0]);
    assert!(seen[1].starts_with("POST /v1.0/me/mailFolders/MAILALM/childFolders "), "{}", seen[1]);
    let kql = http::pct("\"from:zoe storyboard quote hasattachments:true received>=2026-11-01\"");
    assert!(seen[3].starts_with(&format!("GET /v1.0/me/mailFolders/IN/messages?$search={kql}&$top=50&$select={DELTA_FIELDS},parentFolderId ")), "{}", seen[3]);
}

#[test]
fn the_search_cannot_be_broken_out_of() {
    let q = Query { terms: vec![Term::Text("a\" OR from:x\\ (b)".into()), Term::Subject("ré union".into())] };
    assert_eq!(kql(&q).as_deref(), Some("a OR from x b subject:ré subject:union"));
    let long = Query { terms: (0..200).map(|n| Term::Text(format!("word{n}"))).collect() };
    assert!(kql(&long).is_some_and(|k| k.len() <= MAX_SEARCH));
}
