//! The main window, as the validated mockups draw it: title bar, command bar, folders (accounts
//! in an accordion), message list, reading pane and status bar. IMAP accounts (phase 2), Outlook
//! accounts through Microsoft Graph (phase 3) and Gmail accounts through the Gmail API (phase 4).
//! The reading pane (`ui/reading.rs`), the compose windows and what waits to be sent
//! (`ui/sending.rs`) and the writing settings (`ui/writing.rs`) have modules of their own; so do
//! the search with its scope and search folders (`ui/search.rs`), the rules (`ui/rules.rs`),
//! snooze and follow-ups (`ui/later.rs`), the MCP access (`ui/mcp.rs`), import, export and the
//! mail kept on this PC (`ui/porting.rs`), and the appearance and language (`ui/appearance.rs`).

use std::collections::HashSet;
use std::sync::Arc;

use egui::{
    Align, Align2, Button, Color32, CornerRadius, FontId, Frame, Id, Key, Layout, Margin, Painter, Pos2, Rect, Response, RichText, ScrollArea,
    Sense, Stroke, Ui, Vec2, ViewportCommand, pos2, vec2,
};
use mailalm_engine::{Command, Engine, Link, SenderCommand, SenderRuleKind};
use mailalm_mime::{Outgoing, format_person};
use mailalm_model::{Account, AccountId, Change, Focus, FolderRole, NewMail, Person, Provider, Summary, Workspace, account_color};
use mailalm_safety::files::TempFolder;

use crate::accounts::{AccountForm, FormResult, Secret};
use crate::agenda::{Ctx, Pim, PimPage};
use crate::attachments::{Review, ReviewAction};
use crate::compose::{self, Compose};
use crate::icons::{self, Icon};
use crate::local::{LOCAL, LocalBox};
use crate::mailbox::{Mailboxes, MessageKey, Outcome, Reading};
use crate::reading::MessageView;
use crate::settings::Settings;
use crate::theme as t;
use crate::time::{self, Group};
use crate::window;

mod appearance;
mod later;
mod mcp;
mod pim;
mod porting;
mod reading;
mod rules;
mod search;
mod sending;
mod writing;

/// Starting widths of the folder pane and the message list. The splitters between the three
/// columns change them; the reading pane takes what is left.
const FOLDERS_WIDTH: f32 = 264.0;
const MESSAGES_WIDTH: f32 = 400.0;
const ROW_HEIGHT: f32 = 64.0;
const GROUP_HEIGHT: f32 = 26.0;

/// What the message list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
enum View {
    /// A folder of an account; `None` is its Inbox.
    Folder(AccountId, Option<String>),
    /// Every account's Inbox.
    Unified,
    /// Flagged messages of every account.
    Flagged,
    /// The results of the last search.
    Search,
    /// The snoozed messages and the follow-ups of every account.
    Snoozed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Mail,
    Settings,
    /// Calendar, Contacts or Tasks (`agenda.rs` and its pages).
    Pim(PimPage),
}

struct SearchState {
    id: u64,
    query: String,
    /// The search folder opened, if it is one.
    name: Option<String>,
    /// Accounts that have not answered yet.
    waiting: usize,
    hits: Vec<(AccountId, String, Summary)>,
    /// Accounts whose server has not answered yet.
    server_waiting: usize,
    /// What the servers found that the cache doesn't have, shown below "from the server".
    server: Vec<(AccountId, String, Summary)>,
    /// Why a server could not search.
    server_errors: Vec<String>,
    /// Where it looked, and whether the servers were asked: what a search folder keeps.
    targets: Vec<(AccountId, mailalm_model::FolderScope)>,
    server_asked: bool,
}

/// The `source` of a search hit in a [`Row::Message`].
const HIT: usize = usize::MAX;
/// The `source` of a hit from a server.
const SERVER_HIT: usize = usize::MAX - 1;

/// One line of the message list: a message (where its summary is) or a group's title.
#[derive(Clone, Copy)]
enum Row {
    Group(Group),
    /// A title of its own ("FROM THE SERVER").
    Heading(&'static str),
    /// `source` indexes `ListCache::sources` (or is [`HIT`] or [`SERVER_HIT`] for a search hit);
    /// `pos` is the summary's place in that folder's list (or in the hits).
    Message { source: usize, pos: usize },
}

#[derive(Default)]
struct ListCache {
    key: Option<(View, u64, u64, usize, usize)>,
    sources: Vec<(AccountId, String)>,
    rows: Vec<Row>,
    tops: Vec<f32>,
    /// The messages in list order, for moving the selection.
    order: Vec<MessageKey>,
    unread: usize,
}

pub struct MailApp {
    settings: Settings,
    ws: Workspace,
    engine: Engine,
    boxes: Mailboxes,
    view: View,
    page: Page,
    selected: Option<MessageKey>,
    reading: Option<Reading>,
    list: ListCache,
    /// Where the "New mail" menu opens, while it is open.
    create_menu: Option<Pos2>,
    /// The menu opened during this frame: the click that opened it must not close it.
    create_menu_just_opened: bool,
    search: String,
    search_state: Option<SearchState>,
    next_search: u64,
    /// The last thing MailALM has to say, in the status bar (and whether it is a problem).
    notice: Option<(String, bool)>,
    form: Option<AccountForm>,
    confirm_remove: Option<AccountId>,
    confirm_delete: Option<MessageKey>,
    /// The account whose blocked and allowed senders are shown in Settings.
    senders_open: Option<AccountId>,
    composes: Vec<Compose>,
    next_compose: u64,
    /// The attachment being reviewed (its approval dialog is open).
    review: Option<Review>,
    /// The attachments reviewed in this session (their lock is shown open).
    reviewed: HashSet<(MessageKey, usize)>,
    /// The copies of attachments opened with Windows: emptied at start and at exit.
    temp: TempFolder,
    /// Calendars, reminders, invitations, tasks and contacts (phase 6).
    pim: Pim,
    /// The text of the message being read (its HTML converted, its pictures, its links).
    message_view: Option<MessageView>,
    next_view: u64,
    /// A mailing list to leave, waiting for the user's confirmation.
    leave_list: Option<reading::LeaveList>,
    /// The undo delays, the scheduled messages.
    sending: sending::Sending,
    /// The writing settings being edited.
    writing_page: writing::WritingPage,
    /// Wakes the interface (for the threads of the MCP, the imports and exports).
    wake: mailalm_engine::Wake,
    /// The mail kept on this PC only (imported mail).
    local: LocalBox,
    /// The search's scope, the search folder being named or renamed.
    search_ui: search::SearchUi,
    /// The rules page and its editor.
    rules: rules::RulesPage,
    /// Snooze and follow-ups: the date picked, the reminders due.
    later: later::LaterUi,
    /// The MCP access: its server, the journal, what waits for the user's confirmation.
    mcp: mcp::McpState,
    /// The import or export under way.
    porting: porting::PortingState,
}

impl MailApp {
    pub fn new(ctx: &egui::Context) -> Self {
        let (settings, problem) = Settings::load();
        let (sending, schedule_problem) = sending::Sending::load();
        let problem = problem.or(schedule_problem);
        t::apply(ctx, settings.theme);
        let wake_ctx = ctx.clone();
        let wake: mailalm_engine::Wake = Arc::new(move || wake_ctx.request_repaint());
        let zones: mailalm_engine::Zones = Arc::new(crate::zones::TzRules);
        let mut engine = Engine::new(Settings::cache_dir(), Arc::clone(&wake), zones);
        let local = LocalBox::start(Settings::cache_dir(), Arc::clone(&wake));
        for a in &settings.accounts {
            engine.start(a.clone());
        }
        let ws = Workspace::new(settings.accounts.clone());
        let view = match ws.focused_account() {
            Some(a) => View::Folder(a.id, None),
            None => View::Unified,
        };
        let mcp_on = settings.mcp_enabled;
        let mut app = Self {
            settings,
            ws,
            engine,
            boxes: Mailboxes::default(),
            view,
            page: Page::Mail,
            selected: None,
            reading: None,
            list: ListCache::default(),
            create_menu: None,
            create_menu_just_opened: false,
            search: String::new(),
            search_state: None,
            next_search: 1,
            notice: problem.map(|p| (p, true)),
            form: None,
            confirm_remove: None,
            confirm_delete: None,
            senders_open: None,
            composes: Vec::new(),
            next_compose: 1,
            review: None,
            reviewed: HashSet::new(),
            temp: TempFolder::new(Settings::cache_dir().join("open")),
            pim: Pim::new(),
            message_view: None,
            next_view: 0,
            leave_list: None,
            sending,
            writing_page: writing::WritingPage::default(),
            wake,
            local,
            search_ui: search::SearchUi::default(),
            rules: rules::RulesPage::default(),
            later: later::LaterUi::default(),
            mcp: mcp::McpState::default(),
            porting: porting::PortingState::default(),
        };
        // MCP access left on when MailALM was last closed listens again.
        if mcp_on {
            app.start_mcp();
        }
        app
    }

    /// Sends a command to an account's worker, or to the thread of the mail kept on this PC.
    fn send(&self, id: AccountId, command: Command) -> bool {
        if id == LOCAL { self.local.send(command) } else { self.engine.send(id, command) }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), false));
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some((text.into(), true));
    }

    fn account(&self, id: AccountId) -> Option<&Account> {
        self.ws.account(id)
    }

    fn color_of(&self, id: AccountId) -> Color32 {
        self.account(id).map_or(t::faint(), |a| t::account(a.color))
    }

    /// The folder path a view of an account shows: the one chosen, or its Inbox.
    fn folder_path(&self, id: AccountId, folder: &Option<String>) -> Option<String> {
        match folder {
            Some(p) => Some(p.clone()),
            None => self.boxes.account(id).and_then(|s| s.inbox()).map(|f| f.path.clone()),
        }
    }

    fn view_name(&self) -> String {
        match &self.view {
            View::Folder(id, folder) => {
                let account = if *id == LOCAL { crate::local::LOCAL_NAME } else { self.account(*id).map_or("", |a| a.name.as_str()) }.to_owned();
                let folder = self
                    .folder_path(*id, folder)
                    .and_then(|p| self.boxes.account(*id).and_then(|s| s.folder(&p)).map(|f| display_name(f.role, f.name())))
                    .unwrap_or_else(|| "Inbox".to_owned());
                format!("{account} · {folder}")
            }
            View::Unified => "Unified inbox".to_owned(),
            View::Flagged => "Flagged, all accounts".to_owned(),
            View::Search => match &self.search_state {
                Some(s) => match &s.name {
                    Some(name) => format!("Search folder: {name}"),
                    None => format!("Search: {}", s.query),
                },
                None => "Search".to_owned(),
            },
            View::Snoozed => "Snoozed and follow-ups".to_owned(),
        }
    }

    fn open_create_menu(&mut self, at: Pos2) {
        self.create_menu = Some(at);
        self.create_menu_just_opened = true;
    }

    fn focus_account(&mut self, id: AccountId) {
        if self.ws.focus_account(id) {
            self.set_view(View::Folder(id, None));
        }
    }

    fn focus_unified(&mut self) {
        if self.ws.focus_unified() {
            self.set_view(View::Unified);
        }
    }

    fn set_view(&mut self, view: View) {
        if let View::Folder(id, folder) = &view
            && let Some(path) = self.folder_path(*id, folder)
        {
            self.send(*id, Command::View(path));
        }
        self.view = view;
        self.create_menu = None;
        self.page = Page::Mail;
    }

    // ---- events ----

    fn poll(&mut self) {
        let events: Vec<mailalm_engine::Event> = self.engine.poll().into_iter().chain(self.local.poll()).collect();
        for e in events {
            let Some(e) = self.pim.take(e) else { continue };
            match self.boxes.apply(e) {
                Outcome::None => {}
                Outcome::Message { key, parsed, html } => {
                    if let Some(r) = &mut self.reading
                        && r.key == key
                    {
                        r.parsed = Some(parsed);
                        r.html = html;
                        r.error = None;
                        if self.message_view.as_ref().is_some_and(|v| v.key == key) {
                            // Opened again: shown anew.
                            self.message_view = None;
                        }
                    }
                }
                Outcome::Draft { compose, result } => self.draft_saved(compose, result),
                Outcome::MessageUnavailable { key, reason } => {
                    if let Some(r) = &mut self.reading
                        && r.key == key
                    {
                        r.error = Some(reason);
                    }
                }
                Outcome::Notice(text) => self.say(text),
                Outcome::Error(text) => self.warn(text),
                Outcome::SendFailed { account, text, message } => {
                    // Nothing typed is lost: the message comes back in its window.
                    let id = self.next_compose;
                    self.next_compose += 1;
                    let plain = message.html.is_none();
                    self.composes.push(Compose::new(id, account, &message, Some(text.clone()), None, plain));
                    self.warn(text);
                }
                Outcome::Attachment { key, index, result } => {
                    if let Some(r) = &mut self.review
                        && r.key == key
                        && r.index == index
                    {
                        if result.is_ok() {
                            self.reviewed.insert((key, index));
                        }
                        r.loaded(result);
                    }
                }
                Outcome::SearchResults { account, id, hits } => {
                    if let Some(s) = &mut self.search_state
                        && s.id == id
                    {
                        s.hits.extend(hits.into_iter().map(|(folder, summary)| (account, folder, summary)));
                        s.hits.sort_by(|a, b| b.2.date.cmp(&a.2.date));
                        s.waiting = s.waiting.saturating_sub(1);
                    }
                }
                Outcome::ServerResults { account, id, result } => self.server_results(account, id, result),
                Outcome::RulePreview { account, id, hits } => self.rules.preview_arrived(account, id, hits),
                Outcome::FollowUpDue { account, follow_up } => self.follow_up_due(account, *follow_up),
            }
        }
        // A view on a folder the account no longer has falls back to its Inbox.
        if let View::Folder(id, Some(p)) = &self.view
            && self.boxes.account(*id).is_some_and(|s| !s.folders.is_empty() && s.folder(p).is_none())
        {
            self.view = View::Folder(*id, None);
        }
    }

    // ---- the message list's content ----

    fn rebuild_list(&mut self) {
        let search_len = self.search_state.as_ref().map_or(0, |s| s.hits.len());
        let server_len = self.search_state.as_ref().map_or(0, |s| s.server.len());
        let search_id = self.search_state.as_ref().map_or(0, |s| s.id);
        let key = (self.view.clone(), self.boxes.generation, search_id, search_len, server_len);
        if self.list.key.as_ref() == Some(&key) {
            return;
        }
        let mut sources: Vec<(AccountId, String)> = Vec::new();
        match &self.view {
            View::Folder(id, folder) => {
                if let Some(p) = self.folder_path(*id, folder) {
                    sources.push((*id, p));
                }
            }
            View::Unified => {
                for a in self.ws.accounts() {
                    if let Some(f) = self.boxes.account(a.id).and_then(|s| s.inbox()) {
                        sources.push((a.id, f.path.clone()));
                    }
                }
            }
            View::Flagged => {
                for a in self.ws.accounts() {
                    if let Some(s) = self.boxes.account(a.id) {
                        for f in &s.folders {
                            if !matches!(f.role, FolderRole::Trash | FolderRole::Junk) {
                                sources.push((a.id, f.path.clone()));
                            }
                        }
                    }
                }
            }
            View::Search | View::Snoozed => {}
        }

        // (date, source, pos) of every message shown.
        let mut entries: Vec<(i64, usize, usize)> = Vec::new();
        // The server's hits, shown below the cache's.
        let mut server: Vec<(i64, usize, usize)> = Vec::new();
        let flagged_only = self.view == View::Flagged;
        if self.view == View::Search {
            if let Some(s) = &self.search_state {
                entries.extend(s.hits.iter().enumerate().map(|(pos, h)| (h.2.date, HIT, pos)));
                server.extend(s.server.iter().enumerate().map(|(pos, h)| (h.2.date, SERVER_HIT, pos)));
            }
        } else {
            for (n, (id, path)) in sources.iter().enumerate() {
                if let Some(list) = self.boxes.account(*id).and_then(|s| s.summaries.get(path)) {
                    entries.extend(list.iter().enumerate().filter(|(_, s)| !flagged_only || s.flags.flagged).map(|(pos, s)| (s.date, n, pos)));
                }
            }
        }
        if sources.len() > 1 || self.view == View::Search {
            entries.sort_by(|a, b| b.0.cmp(&a.0));
        }

        let mut rows = Vec::with_capacity(entries.len() + 8);
        let mut tops = Vec::with_capacity(entries.len() + 8);
        let mut order = Vec::with_capacity(entries.len());
        let mut y = 0.0;
        let mut last_group = None;
        let mut unread = 0;
        let first_server = entries.len();
        server.sort_by(|a, b| b.0.cmp(&a.0));
        entries.extend(server);
        for (n, (date, source, pos)) in entries.into_iter().enumerate() {
            if n == first_server && source == SERVER_HIT {
                rows.push(Row::Heading("FROM THE SERVER"));
                tops.push(y);
                y += GROUP_HEIGHT;
                last_group = None;
            }
            let group = Group::of(date);
            if last_group != Some(group) {
                rows.push(Row::Group(group));
                tops.push(y);
                y += GROUP_HEIGHT;
                last_group = Some(group);
            }
            rows.push(Row::Message { source, pos });
            tops.push(y);
            y += ROW_HEIGHT;
            if let Some((account, folder, s)) = self.lookup(&sources, source, pos) {
                if !s.flags.seen {
                    unread += 1;
                }
                order.push(MessageKey { account, folder: folder.to_owned(), uid: s.uid });
            }
        }
        tops.push(y);
        self.list = ListCache { key: Some(key), sources, rows, tops, order, unread };
    }

    /// A message of the list: its account, folder and summary.
    fn lookup<'a>(&'a self, sources: &'a [(AccountId, String)], source: usize, pos: usize) -> Option<(AccountId, &'a str, &'a Summary)> {
        if source == HIT || source == SERVER_HIT {
            let s = self.search_state.as_ref()?;
            let h = if source == HIT { s.hits.get(pos)? } else { s.server.get(pos)? };
            return Some((h.0, h.1.as_str(), &h.2));
        }
        let (id, path) = sources.get(source)?;
        let s = self.boxes.account(*id)?.summaries.get(path)?.get(pos)?;
        Some((*id, path.as_str(), s))
    }

    fn summary_of(&self, key: &MessageKey) -> Option<&Summary> {
        if let Some(s) = self.boxes.account(key.account).and_then(|a| a.summary(&key.folder, key.uid)) {
            return Some(s);
        }
        let s = self.search_state.as_ref()?;
        s.hits.iter().chain(&s.server).find(|h| h.0 == key.account && h.1 == key.folder && h.2.uid == key.uid).map(|h| &h.2)
    }

    // ---- actions on messages ----

    fn select(&mut self, key: MessageKey) {
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        let unread = self.summary_of(&key).is_some_and(|s| !s.flags.seen);
        self.send(key.account, Command::Open { folder: key.folder.clone(), uid: key.uid });
        if unread {
            self.send(key.account, Command::Apply(Change::SetSeen { folder: key.folder.clone(), uids: vec![key.uid], seen: true }));
            self.mark_hit(&key, |s| s.flags.seen = true);
        }
        self.reading = Some(Reading { key: key.clone(), parsed: None, html: None, error: None });
        self.selected = Some(key);
    }

    /// Keeps a search hit in step with a change made to its message.
    fn mark_hit(&mut self, key: &MessageKey, f: impl Fn(&mut Summary)) {
        if let Some(s) = &mut self.search_state {
            for h in s.hits.iter_mut().chain(s.server.iter_mut()).filter(|h| h.0 == key.account && h.1 == key.folder && h.2.uid == key.uid) {
                f(&mut h.2);
            }
            // The list is rebuilt from the hits.
            self.list.key = None;
        }
    }

    /// Moves the selection by `step` lines of the list.
    fn move_selection(&mut self, step: isize) {
        let order = &self.list.order;
        if order.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|k| order.iter().position(|o| o == k));
        let next = match at {
            Some(i) => i.saturating_add_signed(step).min(order.len() - 1),
            None => 0,
        };
        if let Some(k) = order.get(next).cloned() {
            self.select(k);
        }
    }

    /// After a message leaves the list: the next one is selected.
    fn select_after_removal(&mut self, removed: &MessageKey) {
        let order = &self.list.order;
        let next = order.iter().position(|o| o == removed).and_then(|i| order.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| order.get(j)))).cloned();
        self.selected = None;
        self.reading = None;
        if let Some(k) = next {
            self.select(k);
        }
    }

    fn move_selected(&mut self, role: FolderRole) {
        let Some(key) = self.selected.clone() else {
            self.say("Select a message first.");
            return;
        };
        let target = self.boxes.account(key.account).and_then(|s| s.folder_with_role(role)).map(|f| f.path.clone());
        match target {
            Some(to) if to == key.folder => self.say("The message is already there."),
            Some(to) => self.move_to(&key, to),
            None => {
                let what = match role {
                    FolderRole::Archive => "an Archive",
                    FolderRole::Junk => "a Junk",
                    FolderRole::Trash => "a Trash",
                    FolderRole::Inbox => "an Inbox",
                    _ => "such a",
                };
                self.say(format!("This account has no {what} folder."));
            }
        }
    }

    /// The Junk menu: 0 report junk, 1 not junk, 2 block sender, 3 never block sender. Outlook
    /// reports the message to Outlook.com (Graph `reportMessage`, which also moves it); Gmail
    /// reports it as spam (its SPAM label) and blocks or allows senders with Gmail filters; IMAP
    /// moves the message.
    fn junk_action(&mut self, n: usize) {
        let Some(key) = self.selected.clone() else {
            self.say("Select a message first.");
            return;
        };
        let provider = self.account(key.account).map(|a| a.provider);
        let reports = matches!(provider, Some(Provider::Outlook | Provider::Gmail));
        match (n, provider) {
            (0, _) if reports => self.mark_junk(&key, true),
            (0, _) => self.move_selected(FolderRole::Junk),
            (1, _) if reports => self.mark_junk(&key, false),
            (1, _) => self.move_selected(FolderRole::Inbox),
            (2, Some(Provider::Gmail)) => self.set_sender(&key, true),
            (3, Some(Provider::Gmail)) => self.set_sender(&key, false),
            (2, Some(Provider::Outlook)) => self.say(
                "Microsoft's API cannot change Outlook.com's blocked senders: block this sender in Outlook.com's settings (Mail, Junk email), or make a rule that moves its messages to Junk (Move, Create a rule from this message).",
            ),
            (2, Some(Provider::Imap(_))) => {
                self.say("An IMAP server keeps no blocked senders: make a MailALM rule that moves this sender's messages to Junk.");
                self.rule_from_message();
            }
            (_, None) => self.say("Mail kept on this PC has no junk folder."),
            (_, _) => self.say("Only a blocked sender can be unblocked: Settings, Rules lists the rules about senders."),
        }
    }

    fn mark_junk(&mut self, key: &MessageKey, junk: bool) {
        let in_folder = self.boxes.account(key.account).and_then(|s| s.folder(&key.folder)).map(|f| f.role);
        if !junk && in_folder != Some(FolderRole::Junk) {
            self.say("Only a message in Junk can be marked as not junk.");
            return;
        }
        if junk && in_folder == Some(FolderRole::Junk) {
            self.say("The message is already in Junk.");
            return;
        }
        let gmail = self.account(key.account).is_some_and(|a| a.provider == Provider::Gmail);
        self.select_after_removal(key);
        self.send(key.account, Command::Apply(Change::MarkJunk { folder: key.folder.clone(), uids: vec![key.uid], junk }));
        self.drop_hit(key);
        self.say(match (gmail, junk) {
            (true, true) => "Reported to Gmail as spam, and moved to Spam.",
            (true, false) => "Reported to Gmail as not spam, and moved back to the Inbox.",
            (false, true) => "Reported to Outlook.com as junk, and moved to Junk.",
            (false, false) => "Reported to Outlook.com as not junk, and moved back to the Inbox.",
        });
    }

    /// Gmail: blocks the sender of the message (a filter to Spam, and the message reported as
    /// spam), or makes sure its messages never go to Spam (a filter too).
    fn set_sender(&mut self, key: &MessageKey, block: bool) {
        let from = self
            .boxes
            .account(key.account)
            .and_then(|s| s.summaries.get(&key.folder))
            .and_then(|v| v.iter().find(|s| s.uid == key.uid))
            .map(|s| s.from.address.trim().to_owned())
            .filter(|a| mailalm_mime::address_is_valid(a));
        let Some(from) = from else {
            self.say("This message has no sender address.");
            return;
        };
        let in_junk = self.boxes.account(key.account).and_then(|s| s.folder(&key.folder)).is_some_and(|f| f.role == FolderRole::Junk);
        if block {
            self.engine.send(key.account, Command::Senders(SenderCommand::Block(from.clone())));
            if !in_junk {
                self.mark_junk(key, true);
            }
            self.say(format!("{from} is blocked: a Gmail filter sends its messages to Spam. Settings lists the blocked senders."));
        } else {
            self.engine.send(key.account, Command::Senders(SenderCommand::Allow(from.clone())));
            if in_junk {
                self.mark_junk(key, false);
            }
            self.say(format!("Gmail will never send {from}'s messages to Spam (a Gmail filter). Settings lists the allowed senders."));
        }
    }

    fn move_to(&mut self, key: &MessageKey, to: String) {
        self.select_after_removal(key);
        self.send(key.account, Command::Apply(Change::Move { folder: key.folder.clone(), uids: vec![key.uid], to }));
        self.drop_hit(key);
    }

    fn drop_hit(&mut self, key: &MessageKey) {
        if let Some(s) = &mut self.search_state {
            s.hits.retain(|h| !(h.0 == key.account && h.1 == key.folder && h.2.uid == key.uid));
            s.server.retain(|h| !(h.0 == key.account && h.1 == key.folder && h.2.uid == key.uid));
        }
    }

    fn delete_selected(&mut self) {
        let Some(key) = self.selected.clone() else {
            self.say("Select a message first.");
            return;
        };
        let state = self.boxes.account(key.account);
        let in_trash = state.and_then(|s| s.folder(&key.folder)).is_some_and(|f| f.role == FolderRole::Trash);
        if in_trash && self.account(key.account).is_some_and(|a| a.provider == Provider::Gmail) {
            // MailALM asks Google only for gmail.modify, which cannot delete for good.
            self.say("Gmail empties its Trash by itself after 30 days: MailALM does not delete Gmail messages for good.");
            return;
        }
        let trash = state.and_then(|s| s.folder_with_role(FolderRole::Trash)).map(|f| f.path.clone());
        match trash {
            Some(trash) if !in_trash => self.move_to(&key, trash),
            // In the Trash, or no Trash at all: deleted for good, after a confirmation.
            _ => self.confirm_delete = Some(key),
        }
    }

    fn set_flag(&mut self, flagged: bool) {
        let Some(key) = self.selected.clone() else {
            self.say("Select a message first.");
            return;
        };
        self.send(key.account, Command::Apply(Change::SetFlagged { folder: key.folder.clone(), uids: vec![key.uid], flagged }));
        self.mark_hit(&key, |s| s.flags.flagged = flagged);
    }

    fn set_seen(&mut self, seen: bool) {
        let Some(key) = self.selected.clone() else {
            self.say("Select a message first.");
            return;
        };
        self.send(key.account, Command::Apply(Change::SetSeen { folder: key.folder.clone(), uids: vec![key.uid], seen }));
        self.mark_hit(&key, |s| s.flags.seen = seen);
    }

    // ---- writing ----

    fn me(&self, id: AccountId) -> Person {
        Person { name: String::new(), address: self.account(id).map(|a| a.address.clone()).unwrap_or_default() }
    }

    fn new_mail_from(&mut self, account: AccountId) {
        let me = self.me(account);
        self.compose(account, Outgoing { from: me, ..Outgoing::default() }, Some(compose::new_document()));
    }

    /// A new message from one of the account's templates.
    fn new_mail_from_template(&mut self, account: AccountId, index: usize) {
        let Some(tpl) = self.settings.writing(account).and_then(|w| w.templates.get(index)).cloned() else { return };
        let me = self.me(account);
        let mut doc = mailalm_compose::import_html(&tpl.html, &[]);
        doc.paragraphs.push(mailalm_compose::Paragraph::default());
        self.compose(account, Outgoing { from: me, subject: tpl.subject, ..Outgoing::default() }, Some(doc));
    }

    fn reply(&mut self, all: bool, forward: bool) {
        let Some(r) = &self.reading else {
            self.say("Select a message first.");
            return;
        };
        let Some(parsed) = &r.parsed else {
            self.say("The message is still loading.");
            return;
        };
        // Mail kept on this PC is answered from the focused account (or the first one).
        let account = if r.key.account == LOCAL {
            match self.ws.focused_account().or_else(|| self.ws.accounts().first()) {
                Some(a) => a.id,
                None => {
                    self.say("Add an account to answer from.");
                    return;
                }
            }
        } else {
            r.key.account
        };
        let date = parsed.date.or_else(|| self.summary_of(&r.key).map(|s| s.date)).unwrap_or(0);
        let me = self.me(account);
        let long = time::long(date);
        // The original's HTML, as the reading pane has it: quoted with its formatting.
        let html = self.message_view.as_ref().filter(|v| v.key == r.key).and_then(MessageView::html);
        let original = compose::document_of(&parsed.text, html);
        let (o, doc) = if forward {
            let mut header = vec![
                "---------- Forwarded message ----------".to_owned(),
                format!("From: {}", format_person(&parsed.from)),
                format!("Date: {long}"),
                format!("Subject: {}", parsed.subject),
                format!("To: {}", people(&parsed.to)),
            ];
            if !parsed.cc.is_empty() {
                header.push(format!("Cc: {}", people(&parsed.cc)));
            }
            (mailalm_mime::forward(parsed, &me, &long), compose::forward_document(&header, original))
        } else {
            let attribution = format!("On {long}, {} wrote:", parsed.from.display());
            (mailalm_mime::reply(parsed, &me, all, &format!("On {long},")), compose::reply_document(&attribution, original))
        };
        self.compose(account, o, Some(doc));
    }

    // ---- accounts ----

    fn finish_form(&mut self, account: Account, secret: Option<Secret>) {
        let kept = match &secret {
            Some(Secret::Password(p)) => mailalm_engine::secrets::set_password(account.id, p),
            Some(Secret::RefreshToken(t)) => mailalm_engine::secrets::set_refresh_token(account.id, t),
            None => Ok(()),
        };
        if let Err(e) = kept {
            self.warn(e);
            return;
        }
        let new = self.account(account.id).is_none();
        self.settings.upsert(account.clone());
        if let Err(e) = self.settings.save() {
            self.warn(e);
        }
        if new {
            self.ws.add_account(account.clone());
            self.say(format!("{} was added. Its mail is being synced.", account.name));
        } else {
            // The workspace keeps the focus; only the account's details change.
            let focus = self.ws.focus();
            let accounts: Vec<Account> = self.settings.accounts.clone();
            self.ws = Workspace::new(accounts);
            match focus {
                Focus::Account(id) => {
                    self.ws.focus_account(id);
                }
                Focus::Unified => {
                    self.ws.focus_unified();
                }
            }
            self.say(format!("{} was saved.", account.name));
        }
        self.engine.start(account.clone());
        if new {
            self.focus_account(account.id);
        }
    }

    fn remove_account(&mut self, id: AccountId) {
        let name = self.account(id).map(|a| a.name.clone()).unwrap_or_default();
        if let Err(e) = self.engine.remove(id, true) {
            self.warn(e);
        }
        self.settings.remove(id);
        if let Err(e) = self.settings.save() {
            self.warn(e);
        }
        self.ws.remove_account(id);
        self.boxes.forget(id);
        self.pim.forget(id);
        if self.selected.as_ref().is_some_and(|k| k.account == id) {
            self.selected = None;
            self.reading = None;
        }
        self.composes.retain(|c| c.account != id);
        self.forget_sending(id);
        if self.review.as_ref().is_some_and(|r| r.key.account == id) {
            self.review = None;
        }
        self.reviewed.retain(|(k, _)| k.account != id);
        self.rules.forget(id);
        self.view = match self.ws.focused_account() {
            Some(a) => View::Folder(a.id, None),
            None => View::Unified,
        };
        self.say(format!("{name} was removed from MailALM. Its mail on the server is untouched."));
    }
}

impl eframe::App for MailApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        t::sync(ui.ctx());
        self.poll();
        self.mcp_tick(ui);
        self.rebuild_list();
        self.keys(ui);
        self.title_bar(ui);
        self.command_bar(ui);
        self.status_bar(ui);
        self.folder_pane(ui);
        match self.page {
            Page::Mail if self.ws.accounts().is_empty() && !matches!(self.view, View::Folder(LOCAL, _)) => self.welcome(ui),
            Page::Mail if self.view == View::Snoozed => self.snoozed_page(ui),
            Page::Mail => {
                self.message_list(ui);
                self.reading_pane(ui);
            }
            Page::Settings => self.settings_page(ui),
            Page::Pim(p) => {
                let cx = Ctx { ws: &self.ws, engine: &self.engine };
                self.pim.page(ui, p, &cx);
            }
        }
        self.create_menu(ui);
        self.dialogs(ui);
        self.compose_windows(ui);
        self.sending_tick(ui);
        self.pim_windows(ui);
        self.later_windows(ui);
        self.porting_window(ui);
        self.mcp_dialogs(ui);
        window::resize_edges(ui);
    }
}

// ---- keyboard ---------------------------------------------------------------------------------

impl MailApp {
    fn keys(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let (new_event, new_task) = ctx.input(|i| {
            let both = i.modifiers.ctrl && i.modifiers.shift;
            (both && i.key_pressed(Key::E), both && i.key_pressed(Key::K))
        });
        if new_event && !self.ws.accounts().is_empty() {
            self.new_event(false);
        }
        if new_task && !self.ws.accounts().is_empty() {
            self.new_task();
        }
        if ctx.egui_wants_keyboard_input() || self.page != Page::Mail || self.form.is_some() || self.review.is_some() {
            return;
        }
        let (down, up, delete) = ctx.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp), i.key_pressed(Key::Delete)));
        if down {
            self.move_selection(1);
        }
        if up {
            self.move_selection(-1);
        }
        if delete && self.selected.is_some() {
            self.delete_selected();
        }
    }
}

// ---- title bar --------------------------------------------------------------------------------

impl MailApp {
    fn title_bar(&mut self, ui: &mut Ui) {
        let unread: u32 = match &self.view {
            View::Folder(id, folder) => self
                .folder_path(*id, folder)
                .and_then(|p| self.boxes.account(*id).and_then(|s| s.folder(&p)).map(|f| f.unread))
                .unwrap_or(0),
            _ => u32::try_from(self.list.unread).unwrap_or(u32::MAX),
        };
        let title = match self.page {
            _ if self.ws.accounts().is_empty() => "No account yet".to_owned(),
            Page::Pim(p) => p.title().to_owned(),
            _ => format!("{} · {unread} unread", self.view_name()),
        };
        egui::Panel::top("title-bar")
            .exact_size(34.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(t::title()))
            .show(ui, |ui| {
                let bar = ui.max_rect();
                window::title_bar_drag(ui, bar);
                ui.horizontal_centered(|ui| {
                    ui.add_space(12.0);
                    let (logo, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    ui.painter().rect_filled(logo, 6.0, Color32::WHITE);
                    ui.painter().text(logo.center(), Align2::CENTER_CENTER, "M", FontId::proportional(13.0), t::title());
                    ui.add(egui::Label::new(RichText::new("MailALM").color(t::title_text()).strong().size(14.0)).selectable(false));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let size = vec2(44.0, 34.0);
                        let hover = Color32::from_rgba_unmultiplied(255, 255, 255, 40);
                        let ctx = ui.ctx().clone();
                        if icons::button(ui, Icon::Close, size, t::title_text(), Color32::from_rgb(0xc4, 0x2b, 0x1c), "Close").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        if icons::button(ui, Icon::Maximize, size, t::title_text(), hover, "Maximize or restore").clicked() {
                            window::toggle_maximized(&ctx);
                        }
                        if icons::button(ui, Icon::Minimize, size, t::title_text(), hover, "Minimize").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                        }
                        ui.add_space(6.0);
                        let small = vec2(32.0, 28.0);
                        if icons::button(ui, Icon::Settings, small, t::title_text(), hover, "Settings").clicked() {
                            self.page = if self.page == Page::Settings { Page::Mail } else { Page::Settings };
                            self.create_menu = None;
                        }
                        if icons::button(ui, Icon::Refresh, small, t::title_text(), hover, "Send and receive all").clicked() {
                            let ids: Vec<AccountId> = self.ws.accounts().iter().map(|a| a.id).collect();
                            for id in ids {
                                self.engine.send(id, Command::SyncAll);
                            }
                            self.say("Checking every account for new mail…");
                        }
                        ui.centered_and_justified(|ui| {
                            ui.add(egui::Label::new(RichText::new(title).color(t::title_text()).size(12.0)).selectable(false).truncate());
                        });
                    });
                });
            });
    }
}

// ---- command bar ------------------------------------------------------------------------------

fn separator(ui: &mut Ui) {
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t::line());
    ui.add_space(4.0);
}

fn flat(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(RichText::new(label).color(t::text())).frame_when_inactive(false))
}

impl MailApp {
    fn command_bar(&mut self, ui: &mut Ui) {
        egui::Panel::top("command-bar")
            .resizable(false)
            .frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // The calendar, contacts and tasks have their own commands.
                    if let Page::Pim(p) = self.page {
                        let cx = Ctx { ws: &self.ws, engine: &self.engine };
                        self.pim.commands(ui, p, &cx);
                        return;
                    }
                    self.new_mail_button(ui);
                    separator(ui);
                    if flat(ui, "Reply").clicked() {
                        self.reply(false, false);
                    }
                    if flat(ui, "Reply all").clicked() {
                        self.reply(true, false);
                    }
                    if flat(ui, "Forward").clicked() {
                        self.reply(false, true);
                    }
                    separator(ui);
                    if flat(ui, "Archive").clicked() {
                        self.move_selected(FolderRole::Archive);
                    }
                    if flat(ui, "Delete").clicked() {
                        self.delete_selected();
                    }
                    let mut junk: Option<usize> = None;
                    let _ = ui.menu_button(("Junk", icons::atom(Icon::ChevronDown)), |ui| {
                        for (n, item) in ["Report junk", "Not junk", "Block sender", "Never block sender"].into_iter().enumerate() {
                            if ui.button(item).clicked() {
                                junk = Some(n);
                                ui.close();
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Outlook: reported to Outlook.com, which moves the message.").size(11.0).color(t::muted()));
                        ui.label(RichText::new("Gmail: reported to Gmail as spam; senders are blocked or allowed by Gmail filters.").size(11.0).color(t::muted()));
                        ui.label(RichText::new("IMAP: moved to the server's Junk folder.").size(11.0).color(t::muted()));
                    });
                    if let Some(n) = junk {
                        self.junk_action(n);
                    }
                    separator(ui);
                    self.move_menu(ui);
                    self.snooze_menu(ui);
                    let mut flag: Option<bool> = None;
                    let _ = ui.menu_button(("Flag", icons::atom(Icon::ChevronDown)), |ui| {
                        if ui.button("Flag").clicked() {
                            flag = Some(true);
                            ui.close();
                        }
                        if ui.button("Clear the flag").clicked() {
                            flag = Some(false);
                            ui.close();
                        }
                        ui.separator();
                        self.follow_up_menu(ui);
                        ui.separator();
                        ui.label(RichText::new("Categories").color(t::faint()))
                            .on_hover_text("Setting Outlook categories and Gmail labels on a message isn't possible in MailALM yet: the search finds them (category:name).");
                    });
                    if let Some(f) = flag {
                        self.set_flag(f);
                    }
                    let mut more: Option<usize> = None;
                    let _ = ui.menu_button(("More", icons::atom(Icon::ChevronDown)), |ui| {
                        for (n, item) in ["Mark as read", "Mark as unread", "Sync this folder now"].into_iter().enumerate() {
                            if ui.button(item).clicked() {
                                more = Some(n);
                                ui.close();
                            }
                        }
                        ui.separator();
                        self.export_menu(ui);
                    });
                    match more {
                        Some(0) => self.set_seen(true),
                        Some(1) => self.set_seen(false),
                        Some(_) => {
                            if let View::Folder(id, folder) = self.view.clone()
                                && let Some(p) = self.folder_path(id, &folder)
                            {
                                self.send(id, Command::View(p));
                            }
                        }
                        None => {}
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.search_bar(ui);
                    });
                });
            });
    }

    fn move_menu(&mut self, ui: &mut Ui) {
        let account = self.selected.as_ref().map(|k| k.account);
        let current = self.selected.as_ref().map(|k| k.folder.clone());
        let folders: Vec<(String, String, usize)> = account
            .and_then(|a| self.boxes.account(a))
            .map(|s| s.folders.iter().map(|f| (f.path.clone(), display_name(f.role, f.name()), f.depth())).collect())
            .unwrap_or_default();
        let mut chosen: Option<String> = None;
        let mut make_rule = false;
        let can_rule = account.is_some_and(|a| a != LOCAL);
        let _ = ui.menu_button(("Move", icons::atom(Icon::ChevronDown)), |ui| {
            if folders.is_empty() {
                ui.label(RichText::new("Select a message first.").color(t::faint()));
                return;
            }
            ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for (path, name, depth) in &folders {
                    let label = format!("{}{}", "    ".repeat(*depth), name);
                    if ui.add_enabled(current.as_deref() != Some(path.as_str()), Button::new(label)).clicked() {
                        chosen = Some(path.clone());
                        ui.close();
                    }
                }
            });
            if can_rule {
                ui.separator();
                if ui.button("Create a rule from this message…").on_hover_text("A rule for this sender's messages, in Settings, Rules").clicked() {
                    make_rule = true;
                    ui.close();
                }
            }
        });
        if let (Some(to), Some(key)) = (chosen, self.selected.clone()) {
            self.move_to(&key, to);
        }
        if make_rule {
            self.rule_from_message();
        }
    }

    /// "New mail", with the focused account's name under it, and its arrow.
    fn new_mail_button(&mut self, ui: &mut Ui) {
        let action = self.ws.new_mail();
        let sender = match action {
            NewMail::From(id) => self.ws.account(id).map(|a| a.name.clone()),
            NewMail::ChooseAccount | NewMail::NoAccount => None,
        };
        let (main, arrow) = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let h = 38.0;
                let (main_rect, main) = ui.allocate_exact_size(vec2(122.0, h), Sense::click());
                let (arrow_rect, arrow) = ui.allocate_exact_size(vec2(26.0, h), Sense::click());
                let p = ui.painter();
                let fill = |r: &Response| if r.hovered() { t::accent_text() } else { t::accent() };
                p.rect_filled(main_rect, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, fill(&main));
                p.rect_filled(arrow_rect, CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, fill(&arrow));
                p.line_segment(
                    [arrow_rect.left_top() + vec2(0.0, 6.0), arrow_rect.left_bottom() - vec2(0.0, 6.0)],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 70)),
                );
                icons::paint(p, Rect::from_center_size(pos2(main_rect.left() + 16.0, main_rect.center().y), Vec2::splat(14.0)), Icon::Plus, t::on_accent());
                let text_x = main_rect.left() + 30.0;
                match &sender {
                    Some(name) => {
                        p.text(pos2(text_x, main_rect.center().y - 7.0), Align2::LEFT_CENTER, "New mail", FontId::proportional(13.5), t::on_accent());
                        p.text(pos2(text_x, main_rect.center().y + 8.0), Align2::LEFT_CENTER, name, FontId::proportional(11.0), t::on_accent());
                    }
                    None => {
                        p.text(pos2(text_x, main_rect.center().y), Align2::LEFT_CENTER, "New mail", FontId::proportional(13.5), t::on_accent());
                    }
                }
                icons::paint(p, arrow_rect, Icon::ChevronDown, t::on_accent());
                (main, arrow)
            })
            .inner;
        let main = match &sender {
            Some(name) => main.on_hover_text(format!("New mail from {name}")),
            None => main.on_hover_text("New mail: choose the account to send from"),
        };
        let arrow = arrow.on_hover_text("New mail from another account, event, meeting, task or contact");
        if main.clicked() {
            match action {
                NewMail::From(id) => self.new_mail_from(id),
                NewMail::ChooseAccount => self.open_create_menu(main.rect.left_bottom()),
                NewMail::NoAccount => self.form = Some(AccountForm::new()),
            }
        }
        if arrow.clicked() {
            if self.create_menu.is_some() {
                self.create_menu = None;
            } else {
                self.open_create_menu(main.rect.left_bottom());
            }
        }
    }

    /// The "New mail" menu: the account to write from, then the other things to create.
    fn create_menu(&mut self, ui: &mut Ui) {
        let Some(anchor) = self.create_menu else { return };
        let ctx = ui.ctx().clone();
        let focused = self.ws.focused_account().map(|a| a.id);
        let accounts: Vec<(AccountId, String, String, Color32)> =
            self.ws.accounts().iter().map(|a| (a.id, a.name.clone(), a.address.clone(), t::account(a.color))).collect();
        let mut from: Option<AccountId> = None;
        let mut chosen: Option<String> = None;
        let mut create: Option<&'static str> = None;
        let mut template: Option<(AccountId, usize)> = None;
        // The templates of the focused account, or of every account in the unified inbox.
        let templates: Vec<(AccountId, usize, String, String)> = accounts
            .iter()
            .filter(|(id, ..)| focused.is_none_or(|f| f == *id))
            .flat_map(|(id, name, ..)| {
                self.settings.writing(*id).map(|w| w.templates.iter().enumerate().map(|(i, tpl)| (*id, i, tpl.name.clone(), name.clone())).collect::<Vec<_>>()).unwrap_or_default()
            })
            .collect();
        let area = egui::Area::new(Id::unique("mailalm-create-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor + vec2(0.0, 4.0))
            .show(&ctx, |ui| {
                Frame::new()
                    .fill(t::raised())
                    .stroke(Stroke::new(1.0, t::line()))
                    .corner_radius(10)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(330.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let (heading, color) = match focused {
                            Some(_) => ("NEW MAIL FROM", t::faint()),
                            None => ("CHOOSE THE ACCOUNT TO SEND FROM", t::accent_text()),
                        };
                        ui.add_space(4.0);
                        ui.label(RichText::new(heading).size(11.0).strong().color(color));
                        ui.add_space(4.0);
                        for (id, name, address, dot) in &accounts {
                            let current = focused == Some(*id);
                            let right = if current { "current" } else { "" };
                            if menu_row(ui, Some(*dot), name, address, right, current).clicked() {
                                from = Some(*id);
                            }
                        }
                        if accounts.is_empty() && menu_row(ui, None, "Add an account…", "", "", false).clicked() {
                            chosen = Some("add".to_owned());
                        }
                        for (id, i, name, account) in &templates {
                            let detail = if focused.is_none() { account.as_str() } else { "" };
                            if menu_row(ui, None, &format!("From the template {name}"), detail, "", false).clicked() {
                                template = Some((*id, *i));
                            }
                        }
                        if templates.is_empty() && !accounts.is_empty() && menu_row(ui, None, "New mail from a template…", "", "", false).clicked() {
                            chosen = Some("No template yet: add them in Settings, Signatures, templates and quick replies.".to_owned());
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                        for (label, keys) in [("New event", "Ctrl+Shift+E"), ("New meeting", ""), ("New task", "Ctrl+Shift+K"), ("New contact", "")] {
                            if menu_row(ui, None, label, "", keys, false).clicked() {
                                create = Some(label);
                            }
                        }
                    });
            });
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if let Some(what) = create {
            match what {
                "New event" => self.new_event(false),
                "New meeting" => self.new_event(true),
                "New task" => self.new_task(),
                _ => self.say("Contacts are read from Outlook and Google for now: add one there. Adding contacts in MailALM arrives in phase 8."),
            }
            self.create_menu = None;
        } else if let Some(id) = from {
            self.new_mail_from(id);
            self.create_menu = None;
        } else if let Some((id, i)) = template {
            self.new_mail_from_template(id, i);
            self.create_menu = None;
        } else if let Some(text) = chosen {
            if text == "add" {
                self.form = Some(AccountForm::new());
            } else {
                self.say(text);
            }
            self.create_menu = None;
        } else if escape || (!self.create_menu_just_opened && area.response.clicked_elsewhere()) {
            self.create_menu = None;
        }
        self.create_menu_just_opened = false;
    }
}

/// One line of a menu: an optional colour dot, a label, a detail in grey, and a note on the right,
/// followed by a check mark on the current line.
fn menu_row(ui: &mut Ui, dot: Option<Color32>, label: &str, detail: &str, right: &str, current: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let p = ui.painter_at(rect);
    if current {
        p.rect_filled(rect, 6.0, t::sel());
    } else if response.hovered() {
        p.rect_filled(rect, 6.0, t::panel2());
    }
    let mut x = rect.left() + 10.0;
    if let Some(c) = dot {
        p.circle_filled(pos2(x + 4.0, rect.center().y), 4.5, c);
        x += 16.0;
    }
    let label_rect = p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), t::text());
    if !detail.is_empty() {
        p.text(pos2(label_rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, detail, FontId::proportional(12.0), t::faint());
    }
    let mut right_edge = rect.right() - 10.0;
    if current {
        icons::paint(&p, Rect::from_center_size(pos2(right_edge - 5.0, rect.center().y), Vec2::splat(10.0)), Icon::Check, t::accent_text());
        right_edge -= 16.0;
    }
    if !right.is_empty() {
        p.text(pos2(right_edge, rect.center().y), Align2::RIGHT_CENTER, right, FontId::proportional(11.5), t::accent_text());
    }
    response
}

// ---- folder pane ------------------------------------------------------------------------------

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(11.0).strong().color(t::faint()));
    ui.add_space(2.0);
}

/// A folder line: `indent` from the left, selected or not, with its count on the right.
fn folder_row(ui: &mut Ui, indent: f32, selected: bool, label: &str, count: u32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 6.0, t::sel());
    } else if response.hovered() {
        p.rect_filled(rect, 6.0, t::raised());
    }
    let count_left = if count > 0 {
        p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(12.5), t::accent_text()).left() - 6.0
    } else {
        rect.right() - 6.0
    };
    p.with_clip_rect(Rect::from_min_max(rect.min, pos2(count_left, rect.bottom())))
        .text(pos2(rect.left() + indent, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), t::text());
    response
}

/// The name a folder is shown with: the usual ones in English, the others as the server names them.
fn display_name(role: FolderRole, name: &str) -> String {
    match role {
        FolderRole::Inbox => "Inbox".to_owned(),
        FolderRole::Drafts => "Drafts".to_owned(),
        FolderRole::Sent => "Sent".to_owned(),
        FolderRole::Archive => "Archive".to_owned(),
        FolderRole::Junk => "Junk".to_owned(),
        FolderRole::Trash => "Trash".to_owned(),
        FolderRole::Other => name.to_owned(),
    }
}

fn link_color(link: Link) -> Color32 {
    match link {
        Link::Online => t::ok(),
        Link::Connecting | Link::Idle => t::warn(),
        Link::Offline => t::faint(),
        Link::SignInRefused => t::danger(),
    }
}

impl MailApp {
    fn folder_pane(&mut self, ui: &mut Ui) {
        // Resizable: the line on its right edge is a splitter.
        egui::Panel::left("folders")
            .resizable(true)
            .default_size(FOLDERS_WIDTH)
            .size_range(200.0..=420.0)
            .frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(8, 8)))
            .show(ui, |ui| {
                egui::Panel::bottom("modules").resizable(false).frame(Frame::new().inner_margin(Margin::symmetric(0, 6))).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let pages = [("Mail", Page::Mail), ("Calendar", Page::Pim(PimPage::Calendar)), ("Contacts", Page::Pim(PimPage::Contacts)), ("Tasks", Page::Pim(PimPage::Tasks))];
                        for (label, page) in pages {
                            let active = self.page == page || (page == Page::Mail && self.page == Page::Settings);
                            if ui.add(Button::selectable(active, label)).clicked() && !active {
                                self.page = page;
                                self.create_menu = None;
                            }
                        }
                    });
                });
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if let Page::Pim(p) = self.page {
                        let cx = Ctx { ws: &self.ws, engine: &self.engine };
                        self.pim.sidebar(ui, p, &cx);
                        return;
                    }
                    if self.ws.accounts().is_empty() {
                        section(ui, "ACCOUNTS");
                        if ui.button("Add an account…").clicked() {
                            self.form = Some(AccountForm::new());
                        }
                        self.local_pane(ui);
                        return;
                    }
                    if self.ws.unified_available() {
                        section(ui, "FAVORITES");
                        let unread: u32 = self.ws.accounts().iter().filter_map(|a| self.boxes.account(a.id)).map(|s| s.unread_inbox()).sum();
                        if folder_row(ui, 10.0, self.view == View::Unified, "Unified inbox", unread).clicked() {
                            self.focus_unified();
                        }
                        if folder_row(ui, 10.0, self.view == View::Flagged, "Flagged, all accounts", 0).clicked() {
                            self.ws.focus_unified();
                            self.set_view(View::Flagged);
                        }
                    }
                    let ids: Vec<AccountId> = self.ws.accounts().iter().map(|a| a.id).collect();
                    for id in ids {
                        self.account_block(ui, id);
                    }
                    self.saved_searches_pane(ui);
                    let snoozed: usize = self.boxes.accounts.values().map(|s| s.snoozed.len() + s.follow_ups.len()).sum();
                    section(ui, "LATER");
                    let count = u32::try_from(snoozed).unwrap_or(u32::MAX);
                    if folder_row(ui, 10.0, self.view == View::Snoozed, "Snoozed and follow-ups", count).clicked() {
                        self.open_snoozed();
                    }
                    self.local_pane(ui);
                });
            });
    }

    /// An account: its header (click: focus, expand, collapse the others), then its folders when
    /// it has the focus.
    fn account_block(&mut self, ui: &mut Ui, id: AccountId) {
        let Some(account) = self.ws.account(id) else { return };
        let (name, address) = (account.name.clone(), account.address.clone());
        let color = t::account(account.color);
        let expanded = self.ws.is_expanded(id);
        let state = self.boxes.account(id);
        let unread = state.map_or(0, |s| s.unread_inbox());
        let (link, detail) = state.map_or((Link::Idle, String::new()), |s| (s.link, s.link_detail.clone()));
        ui.add_space(6.0);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
        let p = ui.painter_at(rect);
        if expanded {
            p.rect_filled(rect, 4.0, t::panel2());
        } else if response.hovered() {
            p.rect_filled(rect, 4.0, t::raised());
        }
        p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 1.0, color);
        let chevron = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };
        icons::paint(&p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(12.0)), chevron, t::muted());
        let name_rect = p.text(pos2(rect.left() + 28.0, rect.top() + 13.0), Align2::LEFT_CENTER, &name, FontId::proportional(13.5), t::text());
        p.circle_filled(pos2(name_rect.right() + 8.0, name_rect.center().y), 3.5, link_color(link));
        p.text(pos2(rect.left() + 28.0, rect.top() + 29.0), Align2::LEFT_CENTER, &address, FontId::proportional(11.0), t::faint());
        if unread > 0 {
            p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, unread.to_string(), FontId::proportional(13.0), t::accent_text());
        }
        let status = match link {
            Link::Online => format!("Connected to {detail}"),
            Link::Connecting => detail.clone(),
            Link::Idle => "Not connected yet".to_owned(),
            Link::Offline => format!("Offline: {detail}"),
            Link::SignInRefused => detail.clone(),
        };
        let response = response.on_hover_text(status);
        if response.clicked() {
            self.focus_account(id);
        }
        let browser = self.ws.account(id).is_some_and(|a| matches!(a.provider, Provider::Outlook | Provider::Gmail));
        let fix = if browser { "Sign-in refused: sign in again…" } else { "Sign-in refused: check the password…" };
        if link == Link::SignInRefused && ui.small_button(fix).clicked() {
            if let Some(a) = self.ws.account(id) {
                self.form = Some(AccountForm::edit(a));
            }
        }
        if expanded {
            let folders: Vec<(String, String, usize, u32)> = self
                .boxes
                .account(id)
                .map(|s| s.folders.iter().map(|f| (f.path.clone(), display_name(f.role, f.name()), f.depth(), f.unread)).collect())
                .unwrap_or_default();
            if folders.is_empty() {
                ui.label(RichText::new("  Waiting for the server…").size(12.0).color(t::faint()));
            }
            let current = match &self.view {
                View::Folder(v, folder) if *v == id => self.folder_path(id, folder),
                _ => None,
            };
            for (path, label, depth, unread) in folders {
                let selected = current.as_deref() == Some(path.as_str());
                if folder_row(ui, 30.0 + 14.0 * depth as f32, selected, &label, unread).clicked() {
                    self.set_view(View::Folder(id, Some(path)));
                }
            }
        }
    }
}

// ---- message list -----------------------------------------------------------------------------

/// The colour of a sender's avatar, from their address.
fn avatar_color(address: &str) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(0x3d, 0x7e, 0xd6),
        Color32::from_rgb(0xc2, 0x6a, 0x3a),
        Color32::from_rgb(0x8a, 0x5c, 0xd1),
        Color32::from_rgb(0x2f, 0x9e, 0x8f),
        Color32::from_rgb(0xb8, 0x4a, 0x6e),
        Color32::from_rgb(0x6a, 0x8f, 0x2f),
        Color32::from_rgb(0xa0, 0x7a, 0x1e),
        Color32::from_rgb(0x4a, 0x6f, 0x8f),
    ];
    let h = address.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    COLORS[(h % 8) as usize]
}

fn initials(name: &str) -> String {
    let mut out: String = name.split_whitespace().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).take(2).collect();
    if out.is_empty() {
        out.push('?');
    }
    out.to_uppercase()
}

impl MailApp {
    fn message_list(&mut self, ui: &mut Ui) {
        let (title, subtitle, swatch) = match &self.view {
            View::Folder(id, _) => (self.view_name(), self.account(*id).map(|a| a.address.clone()).unwrap_or_default(), self.color_of(*id)),
            View::Unified => ("Unified inbox".to_owned(), format!("{} accounts", self.ws.accounts().len()), t::accent()),
            View::Flagged => ("Flagged".to_owned(), "all accounts".to_owned(), t::accent()),
            View::Search => (self.view_name(), self.search_subtitle(), t::accent()),
            View::Snoozed => (self.view_name(), String::new(), t::accent()),
        };
        // Search results always show their account's colour (a server's hits most of all).
        let mark_accounts = (self.ws.unified_available() && !matches!(self.view, View::Folder(..))) || self.view == View::Search;
        let mut clicked: Option<MessageKey> = None;
        // Resizable: the line between the list and the reading pane is a splitter.
        egui::Panel::left("messages")
            .resizable(true)
            .default_size(MESSAGES_WIDTH)
            .size_range(300.0..=720.0)
            .frame(Frame::new().fill(t::panel2()))
            .show(ui, |ui| {
                Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().rect_filled(r, 3.0, swatch);
                        ui.add(egui::Label::new(RichText::new(title).size(17.0).strong()).truncate());
                    });
                    ui.label(RichText::new(subtitle).size(12.0).color(t::faint()));
                });
                if self.list.rows.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        let text = match &self.view {
                            View::Search => "Nothing found in the cached mail.",
                            View::Flagged => "No flagged message.",
                            _ => "No message here yet.",
                        };
                        ui.label(RichText::new(text).color(t::faint()));
                    });
                    return;
                }
                let total = self.list.tops.last().copied().unwrap_or(0.0);
                ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
                    ui.set_min_height(total);
                    let origin = ui.max_rect().min;
                    let width = ui.available_width();
                    let first = self.list.tops.partition_point(|y| *y + ROW_HEIGHT < viewport.min.y);
                    for i in first..self.list.rows.len() {
                        let top = self.list.tops[i];
                        if top > viewport.max.y {
                            break;
                        }
                        let height = self.list.tops.get(i + 1).copied().unwrap_or(top) - top;
                        let rect = Rect::from_min_size(origin + vec2(0.0, top), vec2(width, height));
                        match self.list.rows[i] {
                            Row::Group(g) => {
                                ui.painter().text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, g.label(), FontId::proportional(11.0), t::faint());
                            }
                            Row::Heading(text) => {
                                ui.painter().rect_filled(rect, 0.0, t::raised());
                                ui.painter().text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, text, FontId::proportional(11.0), t::accent_text());
                            }
                            Row::Message { source, pos } => {
                                let Some((account, folder, s)) = self.lookup(&self.list.sources, source, pos) else { continue };
                                let key = MessageKey { account, folder: folder.to_owned(), uid: s.uid };
                                let response = ui.interact(rect, ui.scope_id().with(("mailalm-row", account.0, folder, s.uid)), Sense::click());
                                let selected = self.selected.as_ref() == Some(&key);
                                let stripe = mark_accounts.then(|| self.color_of(account));
                                message_row(ui.painter(), rect, s, selected, response.hovered(), stripe);
                                if response.clicked() {
                                    clicked = Some(key);
                                }
                            }
                        }
                    }
                });
            });
        if let Some(k) = clicked {
            self.select(k);
        }
    }
}

fn message_row(painter: &Painter, rect: Rect, m: &Summary, selected: bool, hovered: bool, stripe: Option<Color32>) {
    let p = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    if selected {
        p.rect_filled(rect, 0.0, t::sel());
    } else if hovered {
        p.rect_filled(rect, 0.0, t::raised());
    }
    if let Some(c) = stripe {
        p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, c);
    }
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t::line()));
    let unread = !m.flags.seen;
    let from = m.from.display();
    let avatar = pos2(rect.left() + 27.0, rect.top() + 24.0);
    p.circle_filled(avatar, 16.0, avatar_color(&m.from.address));
    p.text(avatar, Align2::CENTER_CENTER, initials(from), FontId::proportional(12.0), Color32::WHITE);
    if unread {
        p.circle_filled(pos2(rect.left() + 7.0, rect.top() + 14.0), 3.0, t::accent_text());
    }

    let x = rect.left() + 52.0;
    let accent = if unread { t::accent_text() } else { t::text() };
    let time_rect = p.text(pos2(rect.right() - 12.0, rect.top() + 14.0), Align2::RIGHT_CENTER, time::short(m.date), FontId::proportional(12.0), if unread { t::accent_text() } else { t::faint() });
    let clip = |right: f32| -> Painter { p.with_clip_rect(Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom())).intersect(p.clip_rect())) };
    let from_font = if unread { FontId::proportional(13.5) } else { FontId::proportional(13.0) };
    clip(time_rect.left() - 8.0).text(pos2(x, rect.top() + 14.0), Align2::LEFT_CENTER, from, from_font, t::text());
    let mut marks = String::new();
    if m.has_attachments {
        marks.push('📎');
    }
    if m.flags.flagged {
        marks.push('⚑');
    }
    let marks_left = if marks.is_empty() {
        rect.right() - 12.0
    } else {
        let color = if m.flags.flagged { t::danger() } else { t::faint() };
        p.text(pos2(rect.right() - 12.0, rect.top() + 32.0), Align2::RIGHT_CENTER, &marks, FontId::proportional(12.0), color).left() - 6.0
    };
    let subject = if m.subject.is_empty() { "(no subject)" } else { m.subject.as_str() };
    clip(marks_left).text(pos2(x, rect.top() + 32.0), Align2::LEFT_CENTER, subject, FontId::proportional(13.0), accent);
    clip(rect.right() - 12.0).text(pos2(x, rect.top() + 49.0), Align2::LEFT_CENTER, &m.preview, FontId::proportional(12.0), t::faint());
}

// ---- reading pane -----------------------------------------------------------------------------

/// A rounded label on a coloured background.
fn pill(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    Frame::new().fill(bg).corner_radius(6).inner_margin(Margin::symmetric(10, 4)).show(ui, |ui| {
        ui.label(RichText::new(text).size(12.0).color(fg));
    });
}

fn people(list: &[Person]) -> String {
    list.iter().map(mailalm_mime::format_person).collect::<Vec<_>>().join(", ")
}

fn size_text(bytes: usize) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} bytes"),
    }
}

impl MailApp {
    fn welcome(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel())).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(120.0);
                ui.label(RichText::new("Welcome to MailALM").size(24.0).strong());
                ui.add_space(8.0);
                ui.label(RichText::new("Add your Vidéotron, Bell, Hotmail, Outlook.com, Gmail or other IMAP account to start.").color(t::muted()));
                ui.add_space(16.0);
                if ui.add(Button::new(RichText::new("Add an account").strong().color(t::on_accent())).fill(t::accent())).clicked() {
                    self.form = Some(AccountForm::new());
                }
            });
        });
    }

    fn settings_page(&mut self, ui: &mut Ui) {
        let mut edit: Option<AccountId> = None;
        let mut remove: Option<AccountId> = None;
        let mut add = false;
        let mut back = false;
        let mut senders: Option<AccountId> = None;
        let mut sender_command: Option<(AccountId, SenderCommand)> = None;
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(28, 20))).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Back to Mail").clicked() {
                    back = true;
                }
                ui.label(RichText::new("Settings").size(20.0).strong());
            });
            ui.add_space(14.0);
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.label(RichText::new("ACCOUNTS").size(11.0).strong().color(t::faint()));
                ui.add_space(6.0);
                for a in self.ws.accounts() {
                    let state = self.boxes.account(a.id);
                    Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                        ui.set_width(ui.available_width().min(720.0));
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(4.0, 34.0), Sense::hover());
                            ui.painter().rect_filled(r, 2.0, t::account(a.color));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&a.name).strong());
                                let servers = match (&a.imap, &a.smtp) {
                                    (Some(i), Some(s)) => format!("{} · IMAP {}:{} · SMTP {}:{}", a.address, i.host, i.port, s.host, s.port),
                                    _ if a.provider == Provider::Outlook => format!("{} · Outlook.com (Microsoft Graph)", a.address),
                                    _ if a.provider == Provider::Gmail => format!("{} · Gmail (Gmail API)", a.address),
                                    _ => a.address.clone(),
                                };
                                ui.label(RichText::new(servers).size(12.0).color(t::faint()));
                                if let Some(s) = state {
                                    ui.label(RichText::new(match s.link {
                                        Link::Online => format!("Connected · {} folders", s.folders.len()),
                                        Link::SignInRefused => s.link_detail.clone(),
                                        Link::Offline => format!("Offline: {}", s.link_detail),
                                        Link::Connecting | Link::Idle => "Connecting…".to_owned(),
                                    }).size(12.0).color(link_color(s.link)));
                                }
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Remove…").clicked() {
                                    remove = Some(a.id);
                                }
                                if ui.button("Edit…").clicked() {
                                    edit = Some(a.id);
                                }
                                if a.provider == Provider::Gmail {
                                    let label = if self.senders_open == Some(a.id) { "Hide senders" } else { "Blocked senders…" };
                                    if ui.button(label).clicked() {
                                        senders = Some(a.id);
                                    }
                                }
                            });
                        });
                        if self.senders_open == Some(a.id) {
                            ui.add_space(8.0);
                            ui.label(RichText::new("Gmail filters made by MailALM (a sender's address, to Spam, to the Trash, or never to Spam). Other filters are left alone, in Gmail's settings.").size(11.5).color(t::faint()));
                            ui.add_space(4.0);
                            match state.and_then(|s| s.sender_rules.as_ref()) {
                                None => {
                                    ui.label(RichText::new("Reading Gmail's filters…").size(12.0).color(t::muted()));
                                }
                                Some(rules) if rules.is_empty() => {
                                    ui.label(RichText::new("No sender is blocked or allowed. Use Junk, Block sender on a message.").size(12.0).color(t::muted()));
                                }
                                Some(rules) => {
                                    for r in rules {
                                        ui.horizontal(|ui| {
                                            let what = match r.kind {
                                                SenderRuleKind::Spam => "blocked: to Spam",
                                                SenderRuleKind::Trash => "blocked: to the Trash",
                                                SenderRuleKind::NeverSpam => "allowed: never to Spam",
                                            };
                                            ui.label(RichText::new(&r.address).size(12.5));
                                            ui.label(RichText::new(what).size(12.0).color(t::muted()));
                                            if ui.small_button("Remove").clicked() {
                                                sender_command = Some((a.id, SenderCommand::Remove(r.id.clone())));
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    });
                    ui.add_space(6.0);
                }
                if ui.button("Add an account…").clicked() {
                    add = true;
                }
                self.writing_settings(ui);
                self.rules_settings(ui);
                self.porting_settings(ui);
                self.mcp_settings(ui);
                self.appearance_settings(ui);
                ui.add_space(20.0);
                ui.label(RichText::new("ABOUT").size(11.0).strong().color(t::faint()));
                ui.add_space(6.0);
                ui.label(RichText::new(format!("MailALM {} · MIT OR Apache-2.0", env!("CARGO_PKG_VERSION"))).color(t::muted()));
                ui.label(RichText::new("Passwords and Microsoft and Google sign-ins are kept in the Windows Credential Manager. The mail cache is in your local application data folder.").size(12.0).color(t::faint()));
            });
        });
        if back {
            self.page = Page::Mail;
        }
        if let Some(id) = senders {
            if self.senders_open == Some(id) {
                self.senders_open = None;
            } else {
                self.senders_open = Some(id);
                self.engine.send(id, Command::Senders(SenderCommand::List));
            }
        }
        if let Some((id, c)) = sender_command {
            self.engine.send(id, Command::Senders(c));
        }
        if add {
            self.form = Some(AccountForm::new());
        }
        if let Some(id) = edit
            && let Some(a) = self.account(id)
        {
            self.form = Some(AccountForm::edit(a));
        }
        if remove.is_some() {
            self.confirm_remove = remove;
        }
    }

    fn dialogs(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let close_review = match &mut self.review {
            Some(review) => matches!(review.show(&ctx, &mut self.temp), ReviewAction::Close),
            None => false,
        };
        if close_review {
            self.review = None;
        }
        if let Some(form) = &mut self.form {
            let id = AccountId(self.settings.next_id.max(1));
            let color = account_color(self.settings.accounts.len());
            match form.show(&ctx, id, color) {
                FormResult::Open => {}
                FormResult::Cancelled => self.form = None,
                FormResult::Done(account, secret) => {
                    self.form = None;
                    let mut account = account;
                    if self.account(account.id).is_none() {
                        account.id = self.settings.take_id();
                    }
                    self.finish_form(account, secret);
                }
            }
        }
        if let Some(id) = self.confirm_remove {
            let name = self.account(id).map(|a| a.name.clone()).unwrap_or_default();
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-confirm-remove")).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.heading(format!("Remove {name}?"));
                ui.add_space(6.0);
                ui.label("MailALM forgets the account, its password or sign-in and its cached mail on this PC. The mail on the server is not touched.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
            if m.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(yes) = answer {
                self.confirm_remove = None;
                if yes {
                    self.remove_account(id);
                }
            }
        }
        if let Some(key) = self.confirm_delete.clone() {
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-confirm-delete")).show(&ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Delete for good?");
                ui.add_space(6.0);
                if key.account == LOCAL {
                    ui.label("The message is removed from this PC. It cannot be brought back.");
                } else {
                    ui.label("The message is removed from the server. It cannot be brought back.");
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
            if m.should_close() && answer.is_none() {
                answer = Some(false);
            }
            if let Some(yes) = answer {
                self.confirm_delete = None;
                if yes {
                    self.select_after_removal(&key);
                    self.send(key.account, Command::Apply(Change::Delete { folder: key.folder.clone(), uids: vec![key.uid] }));
                    self.drop_hit(&key);
                }
            }
        }
    }
}

// ---- status bar -------------------------------------------------------------------------------

impl MailApp {
    fn status_bar(&mut self, ui: &mut Ui) {
        let items = self.list.order.len();
        let unread = self.list.unread;
        let notice = self.notice.clone();
        let activity: Option<String> = self.ws.accounts().iter().find_map(|a| {
            self.boxes.account(a.id).and_then(|s| s.activity.clone()).map(|text| format!("{}: {text}", a.name))
        });
        let accounts: Vec<(String, Color32, Link)> = self
            .ws
            .accounts()
            .iter()
            .map(|a| (a.name.clone(), t::account(a.color), self.boxes.account(a.id).map_or(Link::Idle, |s| s.link)))
            .collect();
        egui::Panel::bottom("status-bar")
            .exact_size(26.0)
            .resizable(false)
            .frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(format!("Items: {items} · Unread: {unread}")).size(12.0).color(t::faint()));
                    if let Some(text) = activity {
                        ui.add_space(12.0);
                        ui.spinner();
                        ui.label(RichText::new(text).size(12.0).color(t::muted()));
                    }
                    if let Some((text, problem)) = notice {
                        ui.add_space(12.0);
                        let label = ui.add(egui::Label::new(RichText::new(&text).size(12.0).color(if problem { t::danger() } else { t::accent_text() })).truncate().sense(Sense::click()));
                        if label.on_hover_text("Click to dismiss").clicked() {
                            self.notice = None;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for (name, color, link) in accounts.iter().rev() {
                            ui.add_space(10.0);
                            ui.label(RichText::new(name).size(12.0).color(t::faint()));
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, *color);
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 2.5, link_color(*link));
                        }
                    });
                });
            });
    }
}
