use super::*;
use crate::server::Waiter;
use crate::wire::Answer;
use serde_json::json;

/// Accounts 1 and 2 are visible, 3 is hidden from the MCP.
#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    triage_lines: Vec<usize>,
}

fn summary(account: u32) -> MessageSummary {
    MessageSummary {
        message: MessageRef { account, folder: "INBOX".into(), uid: account * 10 },
        from: Party { name: "Someone".into(), address: "someone@example.org".into() },
        subject: format!("from account {account}"),
        date: 0,
        unread: true,
        flagged: false,
        has_attachments: false,
        preview: String::new(),
    }
}

fn done(what: &str) -> Done {
    Done { summary: what.into(), ..Done::default() }
}

impl Fake {
    fn note(&mut self, what: &str) {
        self.calls.push(what.to_string());
    }
}

impl Backend for Fake {
    fn is_visible(&self, account: u32) -> bool {
        matches!(account, 1 | 2)
    }
    fn list_accounts(&mut self) -> Result<AccountsAnswer, ToolError> {
        self.note("list_accounts");
        let info = |account: u32| AccountInfo { account, name: format!("A{account}"), address: format!("a{account}@example.org"), provider: "IMAP".into() };
        Ok(AccountsAnswer { accounts: vec![info(1), info(2), info(3)] })
    }
    fn list_folders(&mut self, args: &AccountArgs) -> Result<FoldersAnswer, ToolError> {
        self.note("list_folders");
        Ok(FoldersAnswer { account: args.account, folders: vec![] })
    }
    fn search(&mut self, _: &Search) -> Result<SearchAnswer, ToolError> {
        self.note("search");
        Ok(SearchAnswer { external_content: External::new(vec![summary(1), summary(3), summary(2)]), truncated: false })
    }
    fn read_message(&mut self, _: &MessageArgs) -> Result<MessageAnswer, ToolError> {
        Err(ToolError::not_found("no such message"))
    }
    fn read_thread(&mut self, _: &MessageArgs) -> Result<ThreadAnswer, ToolError> {
        Err(ToolError::not_found("no such message"))
    }
    fn list_attachments(&mut self, _: &MessageArgs) -> Result<AttachmentsAnswer, ToolError> {
        Err(ToolError::not_found("no such message"))
    }
    fn read_calendar(&mut self, _: &ReadCalendar) -> Result<CalendarAnswer, ToolError> {
        Ok(CalendarAnswer { external_content: External::new(vec![]) })
    }
    fn availability(&mut self, _: &Availability) -> Result<AvailabilityAnswer, ToolError> {
        self.note("availability");
        let busy = |account: u32| BusyInfo { account, start: 0, end: 60, status: "busy".into() };
        Ok(AvailabilityAnswer { busy: vec![busy(3), busy(2)] })
    }
    fn list_tasks(&mut self, _: &ListTasks) -> Result<TasksAnswer, ToolError> {
        Ok(TasksAnswer { external_content: External::new(vec![]) })
    }
    fn list_contacts(&mut self, _: &ListContacts) -> Result<ContactsAnswer, ToolError> {
        Ok(ContactsAnswer { external_content: External::new(vec![]), truncated: false })
    }
    fn create_draft(&mut self, _: &MessageDraft) -> Result<Done, ToolError> {
        self.note("create_draft");
        Ok(done("draft saved"))
    }
    fn move_messages(&mut self, _: &MoveMessages) -> Result<Done, ToolError> {
        self.note("move_messages");
        Ok(done("moved"))
    }
    fn mark_messages(&mut self, _: &MarkMessages) -> Result<Done, ToolError> {
        self.note("mark_messages");
        Ok(done("marked"))
    }
    fn add_category(&mut self, _: &AddCategory) -> Result<Done, ToolError> {
        Err(ToolError::not_supported("no categories on IMAP"))
    }
    fn create_event(&mut self, _: &CreateEvent) -> Result<Done, ToolError> {
        Ok(done("event"))
    }
    fn create_task(&mut self, _: &CreateTask) -> Result<Done, ToolError> {
        Ok(done("task"))
    }
    fn send_message(&mut self, _: &SendMessage, _: &Confirmed) -> Result<Done, ToolError> {
        self.note("send_message");
        Ok(done("sent"))
    }
    fn delete_messages(&mut self, _: &Messages, _: &Confirmed) -> Result<Done, ToolError> {
        self.note("delete_messages");
        Ok(done("deleted"))
    }
    fn block_sender(&mut self, _: &BlockSender, _: &Confirmed) -> Result<Done, ToolError> {
        self.note("block_sender");
        Ok(done("blocked"))
    }
    fn answer_invitation(&mut self, _: &AnswerInvitation, _: &Confirmed) -> Result<Done, ToolError> {
        self.note("answer_invitation");
        Ok(done("answered"))
    }
    fn change_rule(&mut self, _: &ChangeRule, _: &Confirmed) -> Result<Done, ToolError> {
        self.note("change_rule");
        Ok(done("rule changed"))
    }
    fn apply_triage(&mut self, args: &ProposeTriage, accepted: &[usize], _: &Confirmed) -> Result<TriageAnswer, ToolError> {
        self.note("apply_triage");
        self.triage_lines = accepted.to_vec();
        let outcomes = (0..args.proposals.len())
            .map(|i| {
                let ok = accepted.contains(&i);
                TriageOutcome { index: u32::try_from(i).unwrap_or(u32::MAX), accepted: ok, applied: ok, error: String::new() }
            })
            .collect();
        Ok(TriageAnswer { outcomes })
    }
}

fn request(tool: Tool, args: Value) -> (Incoming, Waiter) {
    Incoming::queued(9, tool, args)
}

fn answer_of(waiter: &Waiter) -> Answer {
    waiter.rx.try_recv().unwrap()
}

fn msg(account: u32) -> Value {
    json!({"account": account, "folder": "INBOX", "uid": 1})
}

/// Arguments that fit every tool, built from its required fields.
fn sample(tool: Tool) -> Value {
    match tool {
        Tool::ListAccounts => json!({}),
        Tool::ListFolders => json!({"account": 1}),
        Tool::Search => json!({"query": "invoice", "scope": {"kind": "folder", "account": 1, "folder": "INBOX"}, "limit": 5}),
        Tool::ReadMessage | Tool::ReadThread | Tool::ListAttachments => json!({"message": msg(1)}),
        Tool::ReadCalendar | Tool::Availability => json!({"accounts": [1, 2], "start": 0, "end": 86_400}),
        Tool::ListTasks => json!({"account": 1, "include_completed": true}),
        Tool::ListContacts => json!({"query": "ann", "limit": 3}),
        Tool::CreateDraft | Tool::SendMessage => json!({
            "account": 1, "to": ["a@b.ca"], "cc": ["c@d.ca"], "bcc": [], "subject": "Hi", "body": "Hello", "in_reply_to": msg(1)
        }),
        Tool::MoveMessages => json!({"messages": [msg(1)], "folder": "Archive"}),
        Tool::MarkMessages => json!({"messages": [msg(1)], "read": true, "flagged": false}),
        Tool::AddCategory => json!({"messages": [msg(1)], "category": "Bills"}),
        Tool::CreateEvent => json!({
            "account": 1, "calendar": "c1", "title": "Lunch", "start": 0, "end": 3600, "all_day": false, "location": "Here", "description": "x"
        }),
        Tool::CreateTask => json!({"account": 1, "list": "l1", "title": "Pay", "notes": "n", "due": 0}),
        Tool::DeleteMessages => json!({"messages": [msg(1), msg(1)]}),
        Tool::BlockSender => json!({"account": 1, "address": "spam@example.org"}),
        Tool::AnswerInvitation => json!({
            "account": 1, "event_uid": "u1", "occurrence": 0, "response": "tentative", "comment": "maybe", "notify_organizer": false
        }),
        Tool::ChangeRule => json!({"account": 1, "change": {"operation": "update", "id": "r1", "rule": {
            "name": "News", "conditions": [{"field": "from", "contains": "news"}],
            "actions": [{"kind": "move_to", "folder": "News"}, {"kind": "mark_read"}], "stop": true
        }}}),
        Tool::ProposeTriage => json!({"proposals": [
            {"message": msg(1), "actions": [{"kind": "move", "folder": "Archive"}], "reason": "old"},
            {"message": msg(2), "actions": [{"kind": "delete"}, {"kind": "block_sender"}]},
            {"message": msg(1), "actions": [{"kind": "category", "category": "Bills"}, {"kind": "mark_read"}, {"kind": "flag"}]},
        ]}),
    }
}

#[test]
fn every_tool_parses_arguments_its_schema_describes() {
    for tool in Tool::ALL {
        let args = sample(tool);
        let call = Call::parse(tool, args.clone()).unwrap_or_else(|e| panic!("{tool:?}: {e}"));
        assert_eq!(call.tool(), tool);
        assert!(!call.describe().is_empty());
        // Every field used is in the schema, every required field is used.
        let schema = tool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        for k in args.as_object().unwrap().keys() {
            assert!(props.contains_key(k), "{tool:?}: `{k}` is not in the schema");
        }
        for r in schema["required"].as_array().unwrap() {
            assert!(args.get(r.as_str().unwrap()).is_some(), "{tool:?}: required `{r}` missing from the sample");
        }
    }
    // No arguments at all is `{}`.
    assert_eq!(Call::parse(Tool::ListAccounts, Value::Null).unwrap(), Call::ListAccounts);
}

#[test]
fn a_read_runs_at_once_and_is_journaled() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    let (incoming, waiter) = request(Tool::ListFolders, json!({"account": 2}));
    assert!(handle(&mut b, &mut j, incoming).is_none());
    assert_eq!(answer_of(&waiter).into_result().unwrap(), json!({"account": 2, "folders": []}));
    let e = j.entries().next_back().unwrap();
    assert_eq!((e.tool, e.outcome, e.confirmation, e.accounts.clone()), (Tool::ListFolders, Outcome::Ok, Confirmation::NotNeeded, vec![2]));
}

#[test]
fn hidden_accounts_are_refused_and_filtered_out() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    // Named: refused before the backend sees it.
    for (tool, args) in [
        (Tool::ListFolders, json!({"account": 3})),
        (Tool::ReadMessage, json!({"message": msg(3)})),
        (Tool::Search, json!({"query": "x", "scope": {"kind": "accounts", "accounts": [1, 3]}})),
        (Tool::SendMessage, json!({"account": 3, "to": ["a@b.ca"]})),
        (Tool::ProposeTriage, json!({"proposals": [{"message": msg(1), "actions": [{"kind": "flag"}]}, {"message": msg(3), "actions": [{"kind": "flag"}]}]})),
        (Tool::ListFolders, json!({"account": 99})),
    ] {
        let (incoming, waiter) = request(tool, args);
        assert!(handle(&mut b, &mut j, incoming).is_none(), "{tool:?}");
        assert_eq!(answer_of(&waiter).into_result().unwrap_err().code, ErrorCode::AccountHidden, "{tool:?}");
    }
    assert!(b.calls.is_empty(), "{:?}", b.calls);
    assert_eq!(j.entries().last().unwrap().outcome, Outcome::Error(ErrorCode::AccountHidden));

    // Over every account: what the backend returns about a hidden one is dropped.
    let (incoming, waiter) = request(Tool::Search, json!({"query": "x"}));
    handle(&mut b, &mut j, incoming);
    let v = answer_of(&waiter).into_result().unwrap();
    let accounts: Vec<u64> = v["external_content"]["data"].as_array().unwrap().iter().map(|m| m["message"]["account"].as_u64().unwrap()).collect();
    assert_eq!(accounts, vec![1, 2]);
    assert_eq!(v["external_content"]["notice"], EXTERNAL_NOTICE);

    let (incoming, waiter) = request(Tool::ListAccounts, json!({}));
    handle(&mut b, &mut j, incoming);
    assert_eq!(answer_of(&waiter).into_result().unwrap()["accounts"].as_array().unwrap().len(), 2);

    let (incoming, waiter) = request(Tool::Availability, json!({"start": 0, "end": 60}));
    handle(&mut b, &mut j, incoming);
    assert_eq!(answer_of(&waiter).into_result().unwrap()["busy"], json!([{"account": 2, "start": 0, "end": 60, "status": "busy"}]));
}

#[test]
fn bad_arguments_never_reach_the_backend() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    for (tool, args) in [
        (Tool::ListFolders, json!({})),
        (Tool::ListFolders, json!({"account": "one"})),
        (Tool::Search, json!({"query": "x", "unknown": true})),
        (Tool::MoveMessages, json!({"messages": [msg(1), msg(2)], "folder": "Archive"})),
        (Tool::CreateDraft, json!({"account": 1, "subject": "a\r\nBcc: spy@example.org"})),
        (Tool::SendMessage, json!({"account": 1, "to": []})),
        (Tool::ListAccounts, json!([])),
    ] {
        let (incoming, waiter) = request(tool, args.clone());
        assert!(handle(&mut b, &mut j, incoming).is_none());
        assert_eq!(answer_of(&waiter).into_result().unwrap_err().code, ErrorCode::BadArgs, "{tool:?} {args}");
    }
    assert!(b.calls.is_empty(), "{:?}", b.calls);
    assert!(j.entries().all(|e| e.outcome == Outcome::Error(ErrorCode::BadArgs)));
}

#[test]
fn a_sensitive_action_waits_for_the_users_confirmation() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    let (incoming, waiter) = request(Tool::SendMessage, sample(Tool::SendMessage));
    let pending = handle(&mut b, &mut j, incoming).unwrap();
    assert!(b.calls.is_empty(), "nothing is sent before the user confirms");
    assert!(waiter.rx.try_recv().is_err(), "the agent is still waiting");
    let text = pending.describe();
    for part in ["Send this message", "To: a@b.ca", "Cc: c@d.ca", "Subject: Hi", "Hello"] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert!(!pending.is_expired());
    pending.approve(&mut b, &mut j);
    assert_eq!(b.calls, vec!["send_message"]);
    assert_eq!(answer_of(&waiter).into_result().unwrap()["summary"], "sent");
    let e = j.entries().last().unwrap();
    assert_eq!((e.outcome, e.confirmation), (Outcome::Ok, Confirmation::Given));
}

#[test]
fn a_declined_action_is_not_done() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    for tool in [Tool::DeleteMessages, Tool::BlockSender, Tool::AnswerInvitation, Tool::ChangeRule, Tool::ProposeTriage] {
        let (incoming, waiter) = request(tool, sample(tool));
        let pending = handle(&mut b, &mut j, incoming).unwrap_or_else(|| panic!("{tool:?} ran without confirmation"));
        pending.decline(&mut j);
        assert_eq!(answer_of(&waiter).into_result().unwrap_err().code, ErrorCode::Declined);
        let e = j.entries().last().unwrap();
        assert_eq!((e.tool, e.confirmation), (tool, Confirmation::Declined));
    }
    assert!(b.calls.is_empty(), "{:?}", b.calls);
}

#[test]
fn a_late_approval_does_nothing() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    let (incoming, waiter) = request(Tool::DeleteMessages, sample(Tool::DeleteMessages));
    let pending = handle(&mut b, &mut j, incoming).unwrap();
    // The connection's time is up before the user decides.
    assert!(waiter.give_up());
    assert!(pending.is_expired());
    pending.approve(&mut b, &mut j);
    assert!(b.calls.is_empty());
    assert_eq!(j.entries().last().unwrap().outcome, Outcome::Expired);
    // And a request the app takes after the connection gave up is not run either.
    let (incoming, waiter) = request(Tool::MarkMessages, sample(Tool::MarkMessages));
    assert!(waiter.give_up());
    assert!(handle(&mut b, &mut j, incoming).is_none());
    assert!(b.calls.is_empty());
}

#[test]
fn a_triage_applies_the_accepted_lines_only() {
    let (mut b, mut j) = (Fake::default(), Journal::default());
    let (incoming, waiter) = request(Tool::ProposeTriage, sample(Tool::ProposeTriage));
    let pending = handle(&mut b, &mut j, incoming).unwrap();
    let text = pending.describe();
    assert!(text.contains("1. 1/INBOX/1: move to “Archive” (old)") && text.contains("2. 2/INBOX/1: delete, block the sender"), "{text}");
    pending.approve_lines(&mut b, &mut j, &[2, 0, 2, 7]);
    assert_eq!(b.triage_lines, vec![0, 2]);
    let v = answer_of(&waiter).into_result().unwrap();
    let applied: Vec<bool> = v["outcomes"].as_array().unwrap().iter().map(|o| o["applied"].as_bool().unwrap()).collect();
    assert_eq!(applied, vec![true, false, true]);

    // All at once.
    let (incoming, _waiter) = request(Tool::ProposeTriage, sample(Tool::ProposeTriage));
    handle(&mut b, &mut j, incoming).unwrap().approve(&mut b, &mut j);
    assert_eq!(b.triage_lines, vec![0, 1, 2]);
}

#[test]
fn dispatch_refuses_what_needs_a_confirmation() {
    let mut b = Fake::default();
    for tool in Tool::ALL.into_iter().filter(|t| t.needs_confirmation()) {
        let call = Call::parse(tool, sample(tool)).unwrap();
        assert_eq!(dispatch(&mut b, &call).unwrap_err().code, ErrorCode::NeedsConfirmation, "{tool:?}");
    }
    assert!(b.calls.is_empty());
    let call = Call::parse(Tool::MoveMessages, sample(Tool::MoveMessages)).unwrap();
    assert_eq!(dispatch(&mut b, &call).unwrap()["summary"], "moved");
    // A provider's refusal comes through as it is.
    let call = Call::parse(Tool::AddCategory, sample(Tool::AddCategory)).unwrap();
    assert_eq!(dispatch(&mut b, &call).unwrap_err().code, ErrorCode::NotSupported);
}
