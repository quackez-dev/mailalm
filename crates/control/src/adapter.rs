//! The app's side of a call: MailALM implements [`Backend`] on top of its engine, and calls
//! [`handle`] for each [`Incoming`] request (on the UI thread, which never blocks on the channel).
//!
//! [`handle`] checks the arguments ([`Call::parse`]) and the accounts hidden from the MCP, runs
//! reads and safe writes at once, and returns sensitive actions and triage proposals as a
//! [`PendingCall`]: MailALM shows [`PendingCall::describe`] (or its own view of
//! [`PendingCall::call`]) and calls [`PendingCall::approve`], [`PendingCall::approve_lines`] or
//! [`PendingCall::decline`]. The sensitive methods of [`Backend`] take a [`Confirmed`], which
//! only an approval makes. Every call goes to the [`Journal`].
//!
//! Hidden accounts are enforced twice: a call that names one is refused, and whatever a backend
//! returns about a hidden account (a search over every account, the calendar…) is dropped.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::catalog::Tool;
use crate::journal::{Confirmation, Journal, JournalEntry, Outcome};
use crate::server::Incoming;
use crate::types::*;
use crate::wire::{ErrorCode, ToolError};

/// A call whose arguments have been read and checked.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    ListAccounts,
    ListFolders(AccountArgs),
    Search(Search),
    ReadMessage(MessageArgs),
    ReadThread(MessageArgs),
    ListAttachments(MessageArgs),
    ReadCalendar(ReadCalendar),
    Availability(Availability),
    ListTasks(ListTasks),
    ListContacts(ListContacts),
    CreateDraft(MessageDraft),
    MoveMessages(MoveMessages),
    MarkMessages(MarkMessages),
    AddCategory(AddCategory),
    CreateEvent(CreateEvent),
    CreateTask(CreateTask),
    SendMessage(SendMessage),
    DeleteMessages(Messages),
    BlockSender(BlockSender),
    AnswerInvitation(AnswerInvitation),
    ChangeRule(ChangeRule),
    ProposeTriage(ProposeTriage),
}

/// Read and check `T` from a request's `args` (`null` counts as `{}`).
fn args<T: DeserializeOwned + Validate>(args: Value) -> Result<T, ToolError> {
    let args = match args {
        Value::Null => Value::Object(serde_json::Map::new()),
        Value::Object(_) => args,
        // serde would read a struct from an array too.
        _ => return Err(ToolError::bad_args("the arguments must be a JSON object")),
    };
    let parsed: T = serde_json::from_value(args).map_err(|e| ToolError::bad_args(e.to_string()))?;
    parsed.validate()?;
    Ok(parsed)
}

impl Call {
    /// The call `tool` makes with `args`, once they fit its schema and its checks.
    pub fn parse(tool: Tool, a: Value) -> Result<Call, ToolError> {
        Ok(match tool {
            Tool::ListAccounts => {
                args::<NoArgs>(a)?;
                Call::ListAccounts
            }
            Tool::ListFolders => Call::ListFolders(args(a)?),
            Tool::Search => Call::Search(args(a)?),
            Tool::ReadMessage => Call::ReadMessage(args(a)?),
            Tool::ReadThread => Call::ReadThread(args(a)?),
            Tool::ListAttachments => Call::ListAttachments(args(a)?),
            Tool::ReadCalendar => Call::ReadCalendar(args(a)?),
            Tool::Availability => Call::Availability(args(a)?),
            Tool::ListTasks => Call::ListTasks(args(a)?),
            Tool::ListContacts => Call::ListContacts(args(a)?),
            Tool::CreateDraft => Call::CreateDraft(args(a)?),
            Tool::MoveMessages => Call::MoveMessages(args(a)?),
            Tool::MarkMessages => Call::MarkMessages(args(a)?),
            Tool::AddCategory => Call::AddCategory(args(a)?),
            Tool::CreateEvent => Call::CreateEvent(args(a)?),
            Tool::CreateTask => Call::CreateTask(args(a)?),
            Tool::SendMessage => Call::SendMessage(args(a)?),
            Tool::DeleteMessages => Call::DeleteMessages(args(a)?),
            Tool::BlockSender => Call::BlockSender(args(a)?),
            Tool::AnswerInvitation => Call::AnswerInvitation(args(a)?),
            Tool::ChangeRule => Call::ChangeRule(args(a)?),
            Tool::ProposeTriage => Call::ProposeTriage(args(a)?),
        })
    }

    pub fn tool(&self) -> Tool {
        match self {
            Call::ListAccounts => Tool::ListAccounts,
            Call::ListFolders(_) => Tool::ListFolders,
            Call::Search(_) => Tool::Search,
            Call::ReadMessage(_) => Tool::ReadMessage,
            Call::ReadThread(_) => Tool::ReadThread,
            Call::ListAttachments(_) => Tool::ListAttachments,
            Call::ReadCalendar(_) => Tool::ReadCalendar,
            Call::Availability(_) => Tool::Availability,
            Call::ListTasks(_) => Tool::ListTasks,
            Call::ListContacts(_) => Tool::ListContacts,
            Call::CreateDraft(_) => Tool::CreateDraft,
            Call::MoveMessages(_) => Tool::MoveMessages,
            Call::MarkMessages(_) => Tool::MarkMessages,
            Call::AddCategory(_) => Tool::AddCategory,
            Call::CreateEvent(_) => Tool::CreateEvent,
            Call::CreateTask(_) => Tool::CreateTask,
            Call::SendMessage(_) => Tool::SendMessage,
            Call::DeleteMessages(_) => Tool::DeleteMessages,
            Call::BlockSender(_) => Tool::BlockSender,
            Call::AnswerInvitation(_) => Tool::AnswerInvitation,
            Call::ChangeRule(_) => Tool::ChangeRule,
            Call::ProposeTriage(_) => Tool::ProposeTriage,
        }
    }

    /// Every account the call names, without repeats. Empty for a call over every visible
    /// account (whose answer is filtered instead).
    pub fn accounts(&self) -> Vec<u32> {
        let mut all: Vec<u32> = match self {
            Call::ListAccounts => Vec::new(),
            Call::ListFolders(a) => vec![a.account],
            Call::Search(s) => match &s.scope {
                SearchScope::All => Vec::new(),
                SearchScope::Accounts { accounts } => accounts.clone(),
                SearchScope::Folder { account, .. } => vec![*account],
            },
            Call::ReadMessage(m) | Call::ReadThread(m) | Call::ListAttachments(m) => vec![m.message.account],
            Call::ReadCalendar(r) | Call::Availability(r) => r.accounts.clone().unwrap_or_default(),
            Call::ListTasks(t) => t.account.into_iter().collect(),
            Call::ListContacts(c) => c.account.into_iter().collect(),
            Call::CreateDraft(d) | Call::SendMessage(SendMessage(d)) => {
                let mut v = vec![d.account];
                v.extend(d.in_reply_to.as_ref().map(|m| m.account));
                v
            }
            Call::MoveMessages(m) => accounts_of(&m.messages),
            Call::MarkMessages(m) => accounts_of(&m.messages),
            Call::AddCategory(m) => accounts_of(&m.messages),
            Call::DeleteMessages(m) => accounts_of(&m.messages),
            Call::CreateEvent(e) => vec![e.account],
            Call::CreateTask(t) => vec![t.account],
            Call::BlockSender(b) => vec![b.account],
            Call::AnswerInvitation(i) => vec![i.account],
            Call::ChangeRule(r) => vec![r.account],
            Call::ProposeTriage(t) => t.proposals.iter().map(|p| p.message.account).collect(),
        };
        all.sort_unstable();
        all.dedup();
        all
    }

    /// What the call will do, in plain words: what MailALM shows before a sensitive action (the
    /// whole message for a send), and what the journal keeps.
    pub fn describe(&self) -> String {
        match self {
            Call::ListAccounts => "List the accounts".into(),
            Call::ListFolders(a) => format!("List the folders of account {}", a.account),
            Call::Search(s) => {
                let scope = match &s.scope {
                    SearchScope::All => "every visible account".to_string(),
                    SearchScope::Accounts { accounts } => format!("accounts {}", list(accounts)),
                    SearchScope::Folder { account, folder } => format!("folder “{folder}” of account {account}"),
                };
                format!("Search “{}” in {scope}", s.query)
            }
            Call::ReadMessage(m) => format!("Read message {}", message(&m.message)),
            Call::ReadThread(m) => format!("Read the thread of message {}", message(&m.message)),
            Call::ListAttachments(m) => format!("List the attachments of message {}", message(&m.message)),
            Call::ReadCalendar(r) => format!("Read the calendar of {} from {} to {} (UTC seconds)", accounts_text(&r.accounts), r.start, r.end),
            Call::Availability(r) => format!("Read the availability of {} from {} to {} (UTC seconds)", accounts_text(&r.accounts), r.start, r.end),
            Call::ListTasks(t) => {
                let which = if t.include_completed { "all tasks" } else { "open tasks" };
                format!("List the {which} of {}", account_text(t.account))
            }
            Call::ListContacts(c) => format!("List the contacts matching “{}” of {}", c.query, account_text(c.account)),
            Call::CreateDraft(d) => format!("Save a draft\n{}", draft(d)),
            Call::MoveMessages(m) => format!("Move {} to “{}”", messages(&m.messages), m.folder),
            Call::MarkMessages(m) => {
                let mut what = Vec::new();
                if let Some(r) = m.read {
                    what.push(if r { "read" } else { "unread" });
                }
                if let Some(f) = m.flagged {
                    what.push(if f { "flagged" } else { "not flagged" });
                }
                format!("Mark {} {}", messages(&m.messages), what.join(" and "))
            }
            Call::AddCategory(m) => format!("Add the category “{}” to {}", m.category, messages(&m.messages)),
            Call::CreateEvent(e) => {
                let place = if e.location.is_empty() { String::new() } else { format!(" at “{}”", e.location) };
                format!(
                    "Create the tentative event “{}”{place} in account {}, from {} to {} (UTC seconds){}",
                    e.title,
                    e.account,
                    e.start,
                    e.end,
                    if e.all_day { ", all day" } else { "" }
                )
            }
            Call::CreateTask(t) => format!("Create the task “{}” in account {}", t.title, t.account),
            Call::SendMessage(SendMessage(d)) => format!("Send this message\n{}", draft(d)),
            Call::DeleteMessages(m) => format!("Move {} to the trash", messages(&m.messages)),
            Call::BlockSender(b) => format!("Block the sender {} on account {}", b.address, b.account),
            Call::AnswerInvitation(i) => {
                let verb = match i.response {
                    InvitationResponse::Accept => "Accept",
                    InvitationResponse::Tentative => "Tentatively accept",
                    InvitationResponse::Decline => "Decline",
                };
                let which = match i.occurrence {
                    Some(o) => format!("the occurrence starting at {o} (UTC seconds) of"),
                    None => "the whole of".to_string(),
                };
                let notify = if i.notify_organizer { "telling the organizer" } else { "without telling the organizer" };
                let comment = if i.comment.is_empty() { String::new() } else { format!(", with the note “{}”", i.comment) };
                format!("{verb} {which} the invitation {} on account {}, {notify}{comment}", i.event_uid, i.account)
            }
            Call::ChangeRule(r) => match &r.change {
                RuleChange::Create { rule: x } => format!("Create on account {} the rule {}", r.account, rule(x)),
                RuleChange::Update { id, rule: x } => format!("Replace on account {} the rule {id} with {}", r.account, rule(x)),
                RuleChange::Delete { id } => format!("Delete the rule {id} of account {}", r.account),
            },
            Call::ProposeTriage(t) => {
                let mut s = format!("Triage of {} messages", t.proposals.len());
                for (i, p) in t.proposals.iter().enumerate() {
                    let actions: Vec<String> = p.actions.iter().map(triage_action).collect();
                    s.push_str(&format!("\n{}. {}: {}", i + 1, message(&p.message), actions.join(", ")));
                    if !p.reason.is_empty() {
                        s.push_str(&format!(" ({})", p.reason));
                    }
                }
                s
            }
        }
    }
}

fn accounts_of(messages: &[MessageRef]) -> Vec<u32> {
    messages.iter().map(|m| m.account).collect()
}

fn list(accounts: &[u32]) -> String {
    accounts.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
}

fn accounts_text(accounts: &Option<Vec<u32>>) -> String {
    match accounts {
        Some(a) => format!("accounts {}", list(a)),
        None => "every visible account".into(),
    }
}

fn account_text(account: Option<u32>) -> String {
    account.map_or_else(|| "every visible account".into(), |a| format!("account {a}"))
}

fn message(m: &MessageRef) -> String {
    format!("{}/{}/{}", m.account, m.folder, m.uid)
}

fn messages(m: &[MessageRef]) -> String {
    match m {
        [one] => format!("message {}", message(one)),
        _ => format!("{} messages of account {}", m.len(), m.first().map_or(0, |x| x.account)),
    }
}

fn draft(d: &MessageDraft) -> String {
    let mut s = format!("From: account {}\nTo: {}", d.account, d.to.join(", "));
    if !d.cc.is_empty() {
        s.push_str(&format!("\nCc: {}", d.cc.join(", ")));
    }
    if !d.bcc.is_empty() {
        s.push_str(&format!("\nBcc: {}", d.bcc.join(", ")));
    }
    s.push_str(&format!("\nSubject: {}", d.subject));
    if let Some(m) = &d.in_reply_to {
        s.push_str(&format!("\nIn reply to: {}", message(m)));
    }
    s.push_str("\n\n");
    s.push_str(&d.body);
    s
}

fn rule(r: &Rule) -> String {
    let when: Vec<String> = r
        .conditions
        .iter()
        .map(|c| {
            let field = match c.field {
                RuleField::From => "from",
                RuleField::To => "to",
                RuleField::Subject => "subject",
                RuleField::Body => "body",
            };
            format!("{field} contains “{}”", c.contains)
        })
        .collect();
    let then: Vec<String> = r
        .actions
        .iter()
        .map(|a| match a {
            RuleAction::MoveTo { folder } => format!("move to “{folder}”"),
            RuleAction::MarkRead => "mark read".into(),
            RuleAction::Flag => "flag".into(),
            RuleAction::Category { category } => format!("add the category “{category}”"),
            RuleAction::Delete => "delete".into(),
        })
        .collect();
    let stop = if r.stop { ", then stop" } else { "" };
    format!("“{}”: when {}, {}{stop}", r.name, when.join(" and "), then.join(", "))
}

fn triage_action(a: &TriageAction) -> String {
    match a {
        TriageAction::Move { folder } => format!("move to “{folder}”"),
        TriageAction::Category { category } => format!("category “{category}”"),
        TriageAction::MarkRead => "mark read".into(),
        TriageAction::Flag => "flag".into(),
        TriageAction::Delete => "delete".into(),
        TriageAction::BlockSender => "block the sender".into(),
    }
}

/// Proof that the user confirmed a sensitive action in MailALM. Only an approval
/// ([`PendingCall::approve`], [`PendingCall::approve_lines`]) makes one.
#[derive(Debug)]
pub struct Confirmed {
    _private: (),
}

/// What MailALM does for each tool, on top of its engine. Every method gets checked arguments,
/// on accounts the MCP may see. Mail content goes in the answers' `external_content`.
///
/// A provider that cannot do something answers [`ToolError::not_supported`]; a missing message
/// or folder, [`ToolError::not_found`].
pub trait Backend {
    /// The MCP may see this account (MailALM's per-account MCP setting). False for an account
    /// that does not exist.
    fn is_visible(&self, account: u32) -> bool;

    fn list_accounts(&mut self) -> Result<AccountsAnswer, ToolError>;
    fn list_folders(&mut self, args: &AccountArgs) -> Result<FoldersAnswer, ToolError>;
    fn search(&mut self, args: &Search) -> Result<SearchAnswer, ToolError>;
    fn read_message(&mut self, args: &MessageArgs) -> Result<MessageAnswer, ToolError>;
    fn read_thread(&mut self, args: &MessageArgs) -> Result<ThreadAnswer, ToolError>;
    /// Names, types and sizes only: never the content.
    fn list_attachments(&mut self, args: &MessageArgs) -> Result<AttachmentsAnswer, ToolError>;
    fn read_calendar(&mut self, args: &ReadCalendar) -> Result<CalendarAnswer, ToolError>;
    fn availability(&mut self, args: &Availability) -> Result<AvailabilityAnswer, ToolError>;
    fn list_tasks(&mut self, args: &ListTasks) -> Result<TasksAnswer, ToolError>;
    fn list_contacts(&mut self, args: &ListContacts) -> Result<ContactsAnswer, ToolError>;

    fn create_draft(&mut self, args: &MessageDraft) -> Result<Done, ToolError>;
    fn move_messages(&mut self, args: &MoveMessages) -> Result<Done, ToolError>;
    fn mark_messages(&mut self, args: &MarkMessages) -> Result<Done, ToolError>;
    fn add_category(&mut self, args: &AddCategory) -> Result<Done, ToolError>;
    /// Tentative, without guests.
    fn create_event(&mut self, args: &CreateEvent) -> Result<Done, ToolError>;
    fn create_task(&mut self, args: &CreateTask) -> Result<Done, ToolError>;

    fn send_message(&mut self, args: &SendMessage, confirmed: &Confirmed) -> Result<Done, ToolError>;
    /// To the trash.
    fn delete_messages(&mut self, args: &Messages, confirmed: &Confirmed) -> Result<Done, ToolError>;
    fn block_sender(&mut self, args: &BlockSender, confirmed: &Confirmed) -> Result<Done, ToolError>;
    fn answer_invitation(&mut self, args: &AnswerInvitation, confirmed: &Confirmed) -> Result<Done, ToolError>;
    fn change_rule(&mut self, args: &ChangeRule, confirmed: &Confirmed) -> Result<Done, ToolError>;
    /// Apply the proposals the user accepted (`accepted`: their places in `args.proposals`,
    /// sorted, without repeats); the answer has an outcome for every proposal.
    fn apply_triage(&mut self, args: &ProposeTriage, accepted: &[usize], confirmed: &Confirmed) -> Result<TriageAnswer, ToolError>;
}

/// The user's approval of a pending call, and the triage lines accepted (`None`: all).
struct Approval {
    confirmed: Confirmed,
    lines: Option<Vec<usize>>,
}

fn needs_confirmation() -> ToolError {
    ToolError::new(ErrorCode::NeedsConfirmation, "this action needs the user's confirmation in MailALM")
}

/// The approval's proof, or the error a sensitive action gets without one.
fn confirmed(approval: Option<&Approval>) -> Result<&Confirmed, ToolError> {
    approval.map(|a| &a.confirmed).ok_or_else(needs_confirmation)
}

fn encode<T: Serialize>(answer: &T) -> Result<Value, ToolError> {
    serde_json::to_value(answer).map_err(|e| ToolError::failed(format!("cannot encode the answer: {e}")))
}

/// Refuse a call that names an account hidden from the MCP.
fn check_visible(backend: &dyn Backend, call: &Call) -> Result<(), ToolError> {
    match call.accounts().into_iter().find(|&a| !backend.is_visible(a)) {
        Some(a) => Err(ToolError::new(
            ErrorCode::AccountHidden,
            format!("account {a} is not available to the MCP (hidden in MailALM's settings, or no such account)"),
        )),
        None => Ok(()),
    }
}

/// Run a read or a safe write on `backend`: the accounts are checked first. A sensitive action or
/// a triage is refused ([`ErrorCode::NeedsConfirmation`]): it goes through [`handle`] and an
/// approval.
pub fn dispatch(backend: &mut dyn Backend, call: &Call) -> Result<Value, ToolError> {
    check_visible(&*backend, call)?;
    run(backend, call, None)
}

fn run(backend: &mut dyn Backend, call: &Call, approval: Option<&Approval>) -> Result<Value, ToolError> {
    match call {
        Call::ListAccounts => {
            let mut a = backend.list_accounts()?;
            a.accounts.retain(|x| backend.is_visible(x.account));
            encode(&a)
        }
        Call::ListFolders(x) => encode(&backend.list_folders(x)?),
        Call::Search(x) => {
            let mut a = backend.search(x)?;
            a.external_content.data.retain(|m| backend.is_visible(m.message.account));
            a.external_content.data.truncate(x.limit());
            encode(&a)
        }
        Call::ReadMessage(x) => encode(&backend.read_message(x)?),
        Call::ReadThread(x) => {
            let mut a = backend.read_thread(x)?;
            a.external_content.data.retain(|m| backend.is_visible(m.message.account));
            encode(&a)
        }
        Call::ListAttachments(x) => encode(&backend.list_attachments(x)?),
        Call::ReadCalendar(x) => {
            let mut a = backend.read_calendar(x)?;
            a.external_content.data.retain(|e| backend.is_visible(e.account));
            encode(&a)
        }
        Call::Availability(x) => {
            let mut a = backend.availability(x)?;
            a.busy.retain(|b| backend.is_visible(b.account));
            encode(&a)
        }
        Call::ListTasks(x) => {
            let mut a = backend.list_tasks(x)?;
            a.external_content.data.retain(|t| backend.is_visible(t.account));
            encode(&a)
        }
        Call::ListContacts(x) => {
            let mut a = backend.list_contacts(x)?;
            a.external_content.data.retain(|c| backend.is_visible(c.account));
            a.external_content.data.truncate(x.limit());
            encode(&a)
        }
        Call::CreateDraft(x) => encode(&backend.create_draft(x)?),
        Call::MoveMessages(x) => encode(&backend.move_messages(x)?),
        Call::MarkMessages(x) => encode(&backend.mark_messages(x)?),
        Call::AddCategory(x) => encode(&backend.add_category(x)?),
        Call::CreateEvent(x) => encode(&backend.create_event(x)?),
        Call::CreateTask(x) => encode(&backend.create_task(x)?),
        Call::SendMessage(x) => encode(&backend.send_message(x, confirmed(approval)?)?),
        Call::DeleteMessages(x) => encode(&backend.delete_messages(x, confirmed(approval)?)?),
        Call::BlockSender(x) => encode(&backend.block_sender(x, confirmed(approval)?)?),
        Call::AnswerInvitation(x) => encode(&backend.answer_invitation(x, confirmed(approval)?)?),
        Call::ChangeRule(x) => encode(&backend.change_rule(x, confirmed(approval)?)?),
        Call::ProposeTriage(x) => {
            let approval = approval.ok_or_else(needs_confirmation)?;
            let mut lines: Vec<usize> = match &approval.lines {
                Some(l) => l.iter().copied().filter(|&i| i < x.proposals.len()).collect(),
                None => (0..x.proposals.len()).collect(),
            };
            lines.sort_unstable();
            lines.dedup();
            encode(&backend.apply_triage(x, &lines, &approval.confirmed)?)
        }
    }
}

/// The journal's line for a call.
fn entry(tool: Tool, accounts: Vec<u32>, result: &Result<Value, ToolError>, confirmation: Confirmation, detail: String) -> JournalEntry {
    let outcome = match result {
        Ok(_) => Outcome::Ok,
        Err(e) => Outcome::Error(e.code),
    };
    JournalEntry::now(tool, accounts, outcome, confirmation, detail)
}

/// Take one request from the server: check it, run it at once, or hand back a sensitive action
/// or a triage for the user to confirm. Never blocks on the channel; the backend's own work is
/// what takes time.
pub fn handle(backend: &mut dyn Backend, journal: &mut Journal, mut incoming: Incoming) -> Option<PendingCall> {
    let tool = incoming.tool();
    if !incoming.start() {
        // The connection gave up waiting: it was never run, and the client was told so.
        journal.record(JournalEntry::now(tool, Vec::new(), Outcome::Expired, Confirmation::NotNeeded, String::new()));
        return None;
    }
    let call = match Call::parse(tool, incoming.take_args()) {
        Ok(c) => c,
        Err(e) => {
            let result = Err(e);
            journal.record(entry(tool, Vec::new(), &result, Confirmation::NotNeeded, String::new()));
            incoming.answer(result);
            return None;
        }
    };
    if let Err(e) = check_visible(&*backend, &call) {
        let result = Err(e);
        journal.record(entry(tool, call.accounts(), &result, Confirmation::NotNeeded, call.describe()));
        incoming.answer(result);
        return None;
    }
    if tool.needs_confirmation() {
        return Some(PendingCall { incoming, call });
    }
    if !incoming.begin_run() {
        journal.record(JournalEntry::now(tool, call.accounts(), Outcome::Expired, Confirmation::NotNeeded, call.describe()));
        return None;
    }
    let result = run(backend, &call, None);
    journal.record(entry(tool, call.accounts(), &result, Confirmation::NotNeeded, call.describe()));
    incoming.answer(result);
    None
}

/// A sensitive action or a triage, waiting for the user's decision. The connection waits too, up
/// to its limit: after it, [`PendingCall::is_expired`] is true and an approval does nothing.
pub struct PendingCall {
    incoming: Incoming,
    call: Call,
}

impl PendingCall {
    /// The call, to show the user exactly what will be done.
    pub fn call(&self) -> &Call {
        &self.call
    }

    /// [`Call::describe`].
    pub fn describe(&self) -> String {
        self.call.describe()
    }

    /// The agent stopped waiting: the dialog can close, nothing will be done.
    pub fn is_expired(&self) -> bool {
        self.incoming.is_cancelled()
    }

    /// The user confirmed: do it (a triage: every line).
    pub fn approve(self, backend: &mut dyn Backend, journal: &mut Journal) {
        self.finish(backend, journal, None);
    }

    /// The user accepted these lines of a triage (their places in `proposals`); for another
    /// action, the same as [`PendingCall::approve`].
    pub fn approve_lines(self, backend: &mut dyn Backend, journal: &mut Journal, lines: &[usize]) {
        self.finish(backend, journal, Some(lines.to_vec()));
    }

    /// The user said no: nothing is done, the agent is told.
    pub fn decline(self, journal: &mut Journal) {
        let result = Err(ToolError::new(ErrorCode::Declined, "the user declined this in MailALM; nothing was done"));
        journal.record(entry(self.call.tool(), self.call.accounts(), &result, Confirmation::Declined, self.call.describe()));
        self.incoming.answer(result);
    }

    fn finish(self, backend: &mut dyn Backend, journal: &mut Journal, lines: Option<Vec<usize>>) {
        let (tool, accounts, detail) = (self.call.tool(), self.call.accounts(), self.call.describe());
        if !self.incoming.begin_run() {
            journal.record(JournalEntry::now(tool, accounts, Outcome::Expired, Confirmation::Given, detail));
            return;
        }
        // The settings may have changed while the dialog was open.
        let result = match check_visible(&*backend, &self.call) {
            Ok(()) => run(backend, &self.call, Some(&Approval { confirmed: Confirmed { _private: () }, lines })),
            Err(e) => Err(e),
        };
        journal.record(entry(tool, accounts, &result, Confirmation::Given, detail));
        self.incoming.answer(result);
    }
}

#[cfg(test)]
mod tests;
