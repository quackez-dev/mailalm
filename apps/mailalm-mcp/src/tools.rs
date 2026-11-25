// Adapted from WordCraft (https://github.com/storytold/wordcraft), commit ca7f262a597c75da4a157b730c68aa2db8d12734, crates/mcp/src/tools.rs (ToolResult, the tool entry of tools/list). MIT OR Apache-2.0.
// Changed: MailALM's catalog (mailalm-control) instead of WordCraft's commands; no images; arguments checked before they are sent; answers with mail content preceded by the untrusted-content notice; annotations from the tool's kind.

//! The MCP tools: their entries in `tools/list` (from MailALM's catalog) and `tools/call`, which
//! checks the arguments and forwards the call to MailALM.

use mailalm_control::types::EXTERNAL_NOTICE;
use mailalm_control::{Call, Client, ClientError, Kind, Tool};
use serde_json::{Value, json};

/// Where tool calls go: the running MailALM ([`Client`]), or a stand-in in tests.
pub(crate) trait Forward {
    fn forward(&mut self, tool: Tool, args: &Value) -> Result<Value, ClientError>;
}

impl Forward for Client {
    fn forward(&mut self, tool: Tool, args: &Value) -> Result<Value, ClientError> {
        self.call(tool, args)
    }
}

/// No local data folder to find MailALM in: every call says why.
pub(crate) struct Unavailable(pub String);

impl Forward for Unavailable {
    fn forward(&mut self, _: Tool, _: &Value) -> Result<Value, ClientError> {
        Err(ClientError::NotRunning(self.0.clone()))
    }
}

/// A `tools/call` result.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
}

fn text(t: impl Into<String>) -> Value {
    json!({"type": "text", "text": t.into()})
}

impl ToolResult {
    fn ok(content: Vec<Value>) -> ToolResult {
        ToolResult { content, is_error: false }
    }

    fn error(t: impl Into<String>) -> ToolResult {
        ToolResult { content: vec![text(t)], is_error: true }
    }

    pub(crate) fn to_value(&self) -> Value {
        json!({"content": self.content, "isError": self.is_error})
    }
}

/// Every tool's entry in `tools/list`.
pub(crate) fn definitions() -> Value {
    Value::Array(Tool::ALL.into_iter().map(definition).collect())
}

fn definition(tool: Tool) -> Value {
    json!({
        "name": tool.name(),
        "title": tool.title(),
        "description": tool.description(),
        "inputSchema": tool.input_schema(),
        "annotations": {
            "title": tool.title(),
            "readOnlyHint": tool.is_read_only(),
            "destructiveHint": tool.kind() == Kind::Sensitive,
            "idempotentHint": tool.is_read_only(),
            "openWorldHint": false,
        },
    })
}

/// Check the arguments, forward the call, and turn MailALM's answer into a tool result.
pub(crate) fn call(forward: &mut dyn Forward, tool: Tool, args: Value) -> ToolResult {
    let args = if args.is_null() { json!({}) } else { args };
    // The same checks MailALM makes, so a mistake is told at once and never reaches it.
    if let Err(e) = Call::parse(tool, args.clone()) {
        return ToolResult::error(format!("invalid arguments for {}: {}", tool.name(), e.message));
    }
    match forward.forward(tool, &args) {
        Ok(answer) => {
            let body = serde_json::to_string_pretty(&answer).unwrap_or_else(|_| answer.to_string());
            if tool.returns_external_content() {
                ToolResult::ok(vec![text(EXTERNAL_NOTICE), text(body)])
            } else {
                ToolResult::ok(vec![text(body)])
            }
        }
        Err(e) => ToolResult::error(e.to_string()),
    }
}
