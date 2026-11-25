use std::sync::{Arc, Mutex};

use mailalm_control::types::EXTERNAL_NOTICE;
use mailalm_control::{ClientError, ErrorCode, Tool, ToolError};
use serde_json::{Value, json};

use crate::rpc::{PROTOCOL_VERSION, Server};
use crate::tools::Forward;

type Seen = Arc<Mutex<Vec<(Tool, Value)>>>;

/// A stand-in for MailALM: records the calls, answers with `reply`.
struct Fake {
    seen: Seen,
    reply: fn(Tool) -> Result<Value, ClientError>,
}

impl Forward for Fake {
    fn forward(&mut self, tool: Tool, args: &Value) -> Result<Value, ClientError> {
        self.seen.lock().unwrap().push((tool, args.clone()));
        (self.reply)(tool)
    }
}

fn server(reply: fn(Tool) -> Result<Value, ClientError>) -> (Server, Seen) {
    let seen = Seen::default();
    (Server::new(Box::new(Fake { seen: seen.clone(), reply })), seen)
}

fn echo(tool: Tool) -> Result<Value, ClientError> {
    Ok(json!({"external_content": {"notice": EXTERNAL_NOTICE, "data": []}, "tool": tool.name()}))
}

fn ask(s: &mut Server, msg: Value) -> Value {
    serde_json::from_str(&s.handle_line(&msg.to_string()).unwrap()).unwrap()
}

fn call(s: &mut Server, name: &str, arguments: Value) -> Value {
    ask(s, json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}))
}

#[test]
fn initialize_negotiates_the_version() {
    let (mut s, _) = server(echo);
    let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}}}));
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "mailalm");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert!(r["result"]["instructions"].as_str().unwrap().contains("untrusted"));
    let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}));
    assert_eq!((r["id"].clone(), r["result"]["protocolVersion"].clone()), (json!("a"), json!(PROTOCOL_VERSION)));
    let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
    assert_eq!(r["result"], json!({}));
}

#[test]
fn notifications_and_responses_get_no_answer() {
    let (mut s, seen) = server(echo);
    for msg in [
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 1}}),
        json!({"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "list_accounts"}}),
        json!({"jsonrpc": "2.0", "id": 9, "result": {}}),
    ] {
        assert_eq!(s.handle_line(&msg.to_string()), None, "{msg}");
    }
    assert_eq!(s.handle_line("   "), None);
    assert!(seen.lock().unwrap().is_empty(), "a notification never runs a tool");
}

#[test]
fn malformed_messages_get_json_rpc_errors() {
    let (mut s, _) = server(echo);
    let r: Value = serde_json::from_str(&s.handle_line("{nope").unwrap()).unwrap();
    assert_eq!((r["id"].clone(), r["error"]["code"].clone()), (Value::Null, json!(-32700)));
    assert_eq!(ask(&mut s, json!([]))["error"]["code"], -32600);
    assert_eq!(ask(&mut s, json!(3))["error"]["code"], -32600);
    assert_eq!(ask(&mut s, json!({"jsonrpc": "2.0", "id": {"x": 1}, "method": "ping"}))["error"]["code"], -32600);
    assert_eq!(ask(&mut s, json!({"jsonrpc": "2.0", "id": 4}))["error"]["code"], -32600);
    assert_eq!(ask(&mut s, json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"}))["error"]["code"], -32601);
    let r = ask(&mut s, json!([{"jsonrpc": "2.0", "id": 6, "method": "ping"}, {"jsonrpc": "2.0", "method": "notifications/initialized"}]));
    assert_eq!(r, json!([{"jsonrpc": "2.0", "id": 6, "result": {}}]));
}

#[test]
fn tools_list_is_the_catalog() {
    let (mut s, _) = server(echo);
    let r = ask(&mut s, json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let tools = r["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), Tool::ALL.len());
    for (t, entry) in Tool::ALL.iter().zip(tools) {
        assert_eq!(entry["name"], t.name());
        assert_eq!(entry["inputSchema"]["type"], "object");
        assert_eq!(entry["annotations"]["readOnlyHint"], t.is_read_only());
    }
    let send = tools.iter().find(|t| t["name"] == "send_message").unwrap();
    assert_eq!((send["annotations"]["readOnlyHint"].clone(), send["annotations"]["destructiveHint"].clone()), (json!(false), json!(true)));
    assert!(send["description"].as_str().unwrap().contains("confirms"));
}

#[test]
fn a_call_is_forwarded_and_mail_content_is_marked() {
    let (mut s, seen) = server(echo);
    let r = call(&mut s, "search", json!({"query": "invoice"}));
    assert_eq!(r["result"]["isError"], false);
    let content = r["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["text"], EXTERNAL_NOTICE, "the notice comes first");
    let body: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["tool"], "search");
    assert_eq!(seen.lock().unwrap().as_slice(), &[(Tool::Search, json!({"query": "invoice"}))]);
    // A tool without mail content: just the answer. No arguments counts as {}.
    let r = call(&mut s, "list_accounts", Value::Null);
    assert_eq!(r["result"]["content"].as_array().unwrap().len(), 1);
    assert_eq!(seen.lock().unwrap().last().unwrap(), &(Tool::ListAccounts, json!({})));
}

#[test]
fn invalid_arguments_never_reach_mailalm() {
    let (mut s, seen) = server(echo);
    for (name, args) in [
        ("search", json!({})),
        ("list_folders", json!({"account": -1})),
        ("send_message", json!({"account": 1, "to": ["not an address"]})),
        ("send_message", json!({"account": 1, "to": ["a@b.ca"], "subject": "x\r\nBcc: spy@example.org"})),
        ("move_messages", json!({"messages": [], "folder": "Archive"})),
        ("change_rule", json!({"account": 1, "change": {"operation": "create", "rule": {"name": "f", "conditions": [{"field": "from", "contains": "a"}], "actions": [{"kind": "forward", "to": "x@y.ca"}]}}})),
    ] {
        let r = call(&mut s, name, args);
        assert_eq!(r["result"]["isError"], true, "{name}");
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("invalid arguments"), "{r}");
    }
    assert!(seen.lock().unwrap().is_empty());
    // An unknown tool is a JSON-RPC error.
    assert_eq!(call(&mut s, "run_program", json!({}))["error"]["code"], -32602);
}

#[test]
fn mailalm_not_running_is_said_clearly() {
    let (mut s, _) = server(|_| Err(ClientError::NotRunning("no port file".into())));
    let r = call(&mut s, "list_accounts", json!({}));
    assert_eq!(r["result"]["isError"], true);
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("MailALM is not running") && text.contains("MCP access"), "{text}");
}

#[test]
fn mailalm_errors_come_through() {
    let (mut s, _) = server(|_| Err(ClientError::Tool(ToolError::new(ErrorCode::Declined, "the user declined this in MailALM; nothing was done"))));
    let r = call(&mut s, "send_message", json!({"account": 1, "to": ["a@b.ca"], "subject": "s", "body": "b"}));
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().starts_with("declined: the user declined"), "{r}");
    let (mut s, _) = server(|_| Err(ClientError::Tool(ToolError::new(ErrorCode::AccountHidden, "account 3 is not available to the MCP"))));
    let r = call(&mut s, "list_folders", json!({"account": 3}));
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("account_hidden"), "{r}");
}

#[test]
fn serve_answers_line_by_line() {
    let (mut s, _) = server(echo);
    let mut input = Vec::new();
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\r\n");
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");
    input.extend_from_slice(&vec![b'x'; mailalm_control::wire::MAX_REQUEST_LINE + 5]);
    input.extend_from_slice(b"\n\xff\n");
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}");
    let mut output = Vec::new();
    s.serve(&input[..], &mut output).unwrap();
    let lines: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let summary: Vec<(Value, Value)> = lines.iter().map(|l| (l["id"].clone(), l["error"]["code"].clone())).collect();
    assert_eq!(summary, vec![(json!(1), Value::Null), (Value::Null, json!(-32600)), (Value::Null, json!(-32700)), (json!(2), Value::Null)]);
}
