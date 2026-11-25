//! Gmail's filters as rules, both ways, labels made and the search, against a local test server.

use mailalm_model::FolderRole;

use super::*;
use crate::http::tests::serve;

fn answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn ok(body: &Value) -> Vec<u8> {
    answer("200 OK", &body.to_string())
}

fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

/// The folders the Gmail worker makes of the labels.
fn folders() -> Vec<Folder> {
    let f = |path: &str, wire: &str, role| Folder { path: path.into(), wire: wire.into(), delimiter: Some('/'), role, total: 0, unread: 0 };
    vec![
        f("Inbox", "INBOX", FolderRole::Inbox),
        f("Spam", "SPAM", FolderRole::Junk),
        f("Trash", "TRASH", FolderRole::Trash),
        f("All Mail", "", FolderRole::Archive),
        f("Shops", "Label_7", FolderRole::Other),
        f("Red", "Label_8", FolderRole::Other),
    ]
}

fn shops() -> Value {
    json!({
        "id": "ANe1Bmj-x=",
        "criteria": {"from": "news@shop.example", "hasAttachment": true, "excludeChats": false},
        "action": {"addLabelIds": ["Label_7", "STARRED"], "removeLabelIds": ["INBOX", "UNREAD"]},
    })
}

#[test]
fn filters_read_as_rules_and_back() {
    let r = rule_of(&shops(), &folders()).unwrap();
    assert_eq!(r.id, "ANe1Bmj-x=");
    assert_eq!((r.name.as_str(), r.place, r.enabled, r.stop), ("", Where::Server, true, false));
    assert_eq!(r.conditions, vec![Condition::From(vec!["news@shop.example".into()]), Condition::HasAttachment]);
    assert_eq!(r.actions, vec![Action::MarkRead, Action::MoveTo("Shops".into()), Action::Flag]);
    assert!(!r.read_only());
    let back = filter_json(&r, &folders()).unwrap();
    assert_eq!(
        back,
        json!({
            "criteria": {"from": "news@shop.example", "hasAttachment": true},
            "action": {"addLabelIds": ["Label_7", "STARRED"], "removeLabelIds": ["UNREAD", "INBOX"]},
        })
    );
    let mut again = back;
    again["id"] = json!("ANe1Bmj-x=");
    assert_eq!(rule_of(&again, &folders()), Some(r));
    // A label added without skipping the Inbox is a category; skipping it without one, All Mail.
    let cat = json!({"id": "F2", "criteria": {"query": "invoice march"}, "action": {"addLabelIds": ["Label_8", "TRASH"]}});
    let r = rule_of(&cat, &folders()).unwrap();
    assert_eq!((r.conditions.clone(), r.actions.clone()), (vec![Condition::Words(vec!["invoice march".into()])], vec![Action::Category("Red".into()), Action::Delete]));
    let archive = json!({"id": "F3", "criteria": {"to": "me+lists@gmail.example"}, "action": {"removeLabelIds": ["INBOX"]}});
    assert_eq!(rule_of(&archive, &folders()).unwrap().actions, vec![Action::MoveTo("All Mail".into())]);
    assert_eq!(rule_of(&archive, &folders()).unwrap().title(), "to me+lists@gmail.example");
}

#[test]
fn sender_filters_stay_with_the_senders_list_and_odd_ones_are_read_only() {
    let sender = json!({"id": "S1", "criteria": {"from": "spam@x.example"}, "action": {"addLabelIds": ["SPAM"]}});
    assert_eq!(rule_of(&sender, &folders()), None);
    let odd = json!({
        "id": "F9",
        "criteria": {"query": "from:(a OR b)", "negatedQuery": "x", "size": 1000, "sizeComparison": "larger", "subject": "\"quoted\""},
        "action": {"addLabelIds": ["IMPORTANT", "Label_99"], "removeLabelIds": ["SPAM"], "forward": "me@elsewhere.example"},
    });
    let r = rule_of(&odd, &folders()).unwrap();
    for part in [
        "criterion query",
        "criterion negatedQuery",
        "criterion size",
        "criterion sizeComparison",
        "criterion subject",
        "takes off the label SPAM",
        "adds the label IMPORTANT",
        "adds the label Label_99",
    ] {
        assert!(r.unsupported.iter().any(|u| u == part), "{part}: {:?}", r.unsupported);
    }
    assert_eq!(r.actions, vec![Action::Forward(vec!["me@elsewhere.example".into()])]);
    assert!(r.read_only() && filter_json(&r, &folders()).is_err());
    assert!(rule_of(&json!({"id": "bad id!", "criteria": {"to": "a"}}), &folders()).is_none());
}

#[test]
fn what_gmail_is_not_sent() {
    let base = Rule::new("", Where::Server, vec![Condition::From(vec!["news@shop.example".into()])], vec![Action::MarkRead]);
    assert!(filter_json(&base, &folders()).is_ok());
    let q = Rule { conditions: vec![Condition::Words(vec!["from:boss".into()])], ..base.clone() };
    assert!(filter_json(&q, &folders()).is_err(), "an operator in the words");
    let or = Rule { conditions: vec![Condition::From(vec!["a OR b".into()])], ..base.clone() };
    assert!(filter_json(&or, &folders()).is_err());
    let two = Rule { conditions: vec![Condition::From(vec!["a".into()]), Condition::From(vec!["b".into()])], ..base.clone() };
    assert!(filter_json(&two, &folders()).is_err());
    assert!(filter_json(&Rule { actions: vec![Action::Category("Inbox".into())], ..base.clone() }, &folders()).is_err());
    assert!(filter_json(&Rule { actions: vec![Action::MoveTo("Inbox".into())], ..base.clone() }, &folders()).is_err());
    assert!(filter_json(&Rule { actions: vec![Action::MoveTo("Gone".into())], ..base.clone() }, &folders()).is_err());
    let spam = filter_json(&Rule { actions: vec![Action::MoveTo("Spam".into())], ..base.clone() }, &folders()).unwrap();
    assert_eq!(spam["action"], json!({"addLabelIds": ["SPAM"]}));
    let fwd = Rule { actions: vec![Action::Forward(vec!["me@elsewhere.example".into()])], forward_confirmed: true, ..base };
    assert_eq!(filter_json(&fwd, &folders()).unwrap()["action"], json!({"forward": "me@elsewhere.example"}));
}

#[test]
fn filters_labels_and_search_on_the_server() {
    let sender = json!({"id": "S1", "criteria": {"from": "spam@x.example"}, "action": {"addLabelIds": ["SPAM"]}});
    let made = json!({"id": "NEW1", "criteria": {"from": "news@shop.example"}, "action": {"removeLabelIds": ["UNREAD"]}});
    let (base, seen) = serve(vec![
        ok(&json!({"filter": [shops(), sender]})),
        ok(&made),
        answer("404 Not Found", r#"{"error":{"code":404,"message":"Filter not found"}}"#),
        ok(&json!({"id": "Label_20", "name": "MailALM/Snoozed", "type": "user"})),
        answer("409 Conflict", r#"{"error":{"code":409,"message":"Label name exists or conflicts"}}"#),
        ok(&json!({"messages": [{"id": "18c1"}, {"id": "bad/id"}, {"id": "18c0"}]})),
    ]);
    let mut c = GmailClient::with_base(&base, "tok-secret-xyz");
    let rules = c.rules(&folders()).unwrap();
    assert_eq!(rules.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["ANe1Bmj-x="], "the sender filter left to its list");
    let new = Rule::new("", Where::Server, vec![Condition::From(vec!["news@shop.example".into()])], vec![Action::MarkRead]);
    let r = c.create_rule(&new, &folders()).unwrap();
    assert_eq!((r.id.as_str(), r.actions.as_slice()), ("NEW1", &[Action::MarkRead][..]));
    c.delete_rule("NEW1").unwrap();
    assert_eq!(c.create_label("MailALM/Snoozed").unwrap().id, "Label_20");
    assert!(matches!(c.create_label("MailALM/Snoozed"), Err(NetError::Server(_))));
    let q = Query::parse(r#"from:zoe "storyboard quote" is:unread before:2026-12-01"#);
    assert_eq!(c.search_ids(&q, Some("INBOX"), 50).unwrap(), ["18c1", "18c0"]);
    assert!(matches!(c.search_ids(&Query::parse("in:inbox"), None, 50), Err(NetError::Invalid(_))));
    assert!(matches!(c.create_rule(&Rule { place: Where::Local, ..new }, &folders()), Err(NetError::Invalid(_))));
    let seen = seen.join().unwrap();
    assert!(seen[0].starts_with("GET /gmail/v1/users/me/settings/filters "), "{}", seen[0]);
    assert!(seen[1].starts_with("POST /gmail/v1/users/me/settings/filters "), "{}", seen[1]);
    assert_eq!(body_of(&seen[1]), json!({"criteria": {"from": "news@shop.example"}, "action": {"removeLabelIds": ["UNREAD"]}}));
    assert!(seen[2].starts_with("DELETE /gmail/v1/users/me/settings/filters/NEW1 "), "{}", seen[2]);
    assert_eq!(body_of(&seen[3]), json!({"name": "MailALM/Snoozed", "labelListVisibility": "labelShow", "messageListVisibility": "show"}));
    let before = mailalm_model::query::day("2026-12-01").unwrap();
    let q = http::pct(&format!("from:zoe \"storyboard quote\" is:unread before:{before}"));
    assert!(
        seen[5].starts_with(&format!("GET /gmail/v1/users/me/messages?maxResults=50&fields=messages/id,nextPageToken&q={q}&labelIds=INBOX ")),
        "{}",
        seen[5]
    );
}

#[test]
fn the_search_cannot_be_broken_out_of() {
    let q = Query { terms: vec![Term::Text("a\" OR (b)".into()), Term::From("-zoe {x}".into()), Term::To("al:ex".into()), Term::Text("OR".into()), Term::Text("\r\n".into())] };
    assert_eq!(gmail_q(&q).as_deref(), Some(r#""a OR b" from:"zoe x" {to:"al:ex" cc:"al:ex"} "OR""#));
    let long = Query { terms: (0..300).map(|n| Term::Text(format!("word{n}"))).collect() };
    assert!(gmail_q(&long).is_some_and(|q| q.len() <= MAX_QUERY));
    assert_eq!(gmail_q(&Query::parse("category:promotions has:attachment")).as_deref(), Some("category:promotions has:attachment"));
}
