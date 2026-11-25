//! The calendars, tasks and contacts of every account, as the interface keeps them from the
//! engine's events (phase 6), and what the pages, windows and panels that show them share: the
//! calendar page (`calendar.rs`), the event windows (`event_window.rs`), the reminders
//! (`reminders.rs`), the tasks (`tasks.rs`), the contacts (`contacts.rs`), the invitations in the
//! reading pane (`invitation.rs`) and the addresses offered in the recipient fields
//! (`autocomplete.rs`).
//!
//! Everything here came from a server or from mail: the model's readers cleaned and capped it;
//! the pages show it as text, and a link only by its domain until the user clicks it.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use egui::Color32;
use mailalm_engine::agenda::{AddressBook, Source};
use mailalm_engine::{Command, Engine, Event as EngineEvent, PimCommand};
use mailalm_model::pim::{Calendar, Contact, Event as CalendarEvent, Task, TaskList, web_link};
use mailalm_model::{Account, AccountId, Person, Provider, Workspace};

use crate::calendar::CalendarView;
use crate::contacts::ContactsView;
use crate::event_window::EventEditor;
use crate::i18n::tr;
use crate::invitation::InvitationState;
use crate::mailbox::{Mailboxes, MessageKey};
use crate::reminders::Reminders;
use crate::tasks::TasksView;
use crate::theme as t;

/// How often the addresses offered are gathered again from the mail cache (at most).
const BOOK_EVERY: Duration = Duration::from_secs(10);

/// One account's calendars, events, task lists, tasks and contacts, as its cache has them.
#[derive(Debug, Default)]
pub struct AccountPim {
    pub calendars: Vec<Calendar>,
    pub events: Vec<CalendarEvent>,
    pub lists: Vec<TaskList>,
    pub tasks: Vec<Task>,
    pub contacts: Vec<Contact>,
}

impl AccountPim {
    pub fn calendar(&self, id: &str) -> Option<&Calendar> {
        self.calendars.iter().find(|c| c.id == id)
    }
}

/// What the pages need from the main window.
pub struct Ctx<'a> {
    pub ws: &'a Workspace,
    pub engine: &'a Engine,
}

impl Ctx<'_> {
    pub fn account(&self, id: AccountId) -> Option<&Account> {
        self.ws.account(id)
    }

    pub fn color(&self, id: AccountId) -> Color32 {
        self.account(id).map_or(t::faint(), |a| t::account(a.color))
    }

    pub fn name(&self, id: AccountId) -> String {
        self.account(id).map(|a| a.name.clone()).unwrap_or_default()
    }

    pub fn provider(&self, id: AccountId) -> Option<Provider> {
        self.account(id).map(|a| a.provider)
    }

    /// Sends a calendar or task command to the account's worker; `false` when it isn't running.
    pub fn send(&self, id: AccountId, c: PimCommand) -> bool {
        self.engine.send(id, Command::Pim(c))
    }
}

/// The pages of the navigation bar besides Mail (PLAN.md, section 12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PimPage {
    Calendar,
    Contacts,
    Tasks,
}

impl PimPage {
    pub fn title(self) -> &'static str {
        match self {
            PimPage::Calendar => tr("Calendar"),
            PimPage::Contacts => tr("Contacts"),
            PimPage::Tasks => tr("Tasks"),
        }
    }
}

/// The calendar an event can go to: an account's calendar that can be changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarChoice {
    pub account: AccountId,
    pub calendar: String,
    pub label: String,
    pub provider: Provider,
    /// Its account's address (the organizer of the meetings planned there).
    pub address: String,
}

/// The calendars, tasks and contacts of every account, and the pages and windows that show them.
#[derive(Default)]
pub struct Pim {
    pub data: BTreeMap<AccountId, AccountPim>,
    /// Goes up at every change of the data: what is worked out from it is worked out again.
    pub generation: u64,
    pub calendar: CalendarView,
    pub editors: Vec<EventEditor>,
    pub next_editor: u64,
    pub tasks: TasksView,
    pub contacts: ContactsView,
    /// The invitations read from messages: (message, part) → what was read.
    pub invitations: HashMap<(MessageKey, usize), InvitationState>,
    pub reminders: Reminders,
    /// The addresses offered in the recipient fields.
    pub book: AddressBook,
    book_key: (u64, u64),
    book_at: Option<Instant>,
    /// What to say in the status bar (text, whether it is a problem).
    pub notices: Vec<(String, bool)>,
    /// A message to write to someone (from the contacts page): the account and the person.
    pub compose_to: Option<(AccountId, Person)>,
}

impl Pim {
    pub fn new() -> Pim {
        Pim { reminders: Reminders::load(), ..Pim::default() }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.notices.push((text.into(), false));
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.notices.push((text.into(), true));
    }

    /// Takes the engine's calendar, task, contact and invitation events; gives the others back.
    pub fn take(&mut self, e: EngineEvent) -> Option<EngineEvent> {
        match e {
            EngineEvent::Calendars { account, calendars, events } => {
                let d = self.data.entry(account).or_default();
                d.calendars = calendars;
                d.events = events;
            }
            EngineEvent::Tasks { account, lists, tasks } => {
                let d = self.data.entry(account).or_default();
                d.lists = lists;
                d.tasks = tasks;
            }
            EngineEvent::Contacts { account, contacts } => self.data.entry(account).or_default().contacts = contacts,
            EngineEvent::Invitation { account, folder, uid, index, result } => {
                self.invitations.insert((MessageKey { account, folder, uid }, index), InvitationState::read(result));
                return None;
            }
            other => return Some(other),
        }
        self.generation += 1;
        None
    }

    /// A page's commands, in the command bar.
    pub fn commands(&mut self, ui: &mut egui::Ui, page: PimPage, cx: &Ctx<'_>) {
        match page {
            PimPage::Calendar => self.calendar_commands(ui, cx),
            PimPage::Contacts => self.contacts_commands(ui),
            PimPage::Tasks => self.tasks_commands(ui, cx),
        }
    }

    /// A page's part of the folder pane: calendars, address books, task lists.
    pub fn sidebar(&mut self, ui: &mut egui::Ui, page: PimPage, cx: &Ctx<'_>) {
        match page {
            PimPage::Calendar => self.calendar_sidebar(ui, cx),
            PimPage::Contacts => self.contacts_sidebar(ui, cx),
            PimPage::Tasks => self.tasks_sidebar(ui, cx),
        }
    }

    /// A page.
    pub fn page(&mut self, ui: &mut egui::Ui, page: PimPage, cx: &Ctx<'_>) {
        match page {
            PimPage::Calendar => self.calendar_page(ui, cx),
            PimPage::Contacts => self.contacts_page(ui, cx),
            PimPage::Tasks => self.tasks_page(ui, cx),
        }
    }

    /// Forgets a removed account.
    pub fn forget(&mut self, id: AccountId) {
        self.data.remove(&id);
        self.editors.retain(|e| e.account() != id);
        self.invitations.retain(|(k, _), _| k.account != id);
        self.generation += 1;
    }

    /// Gathers the addresses offered again, when the contacts or the mail changed (at most every
    /// ten seconds): every account's contacts, then the people seen in the cached mail.
    pub fn refresh_book(&mut self, boxes: &Mailboxes) {
        let key = (self.generation, boxes.generation);
        if key == self.book_key || self.book_at.is_some_and(|at| at.elapsed() < BOOK_EVERY) {
            return;
        }
        let mut book = AddressBook::new();
        for d in self.data.values() {
            for c in &d.contacts {
                let name = c.display();
                for e in &c.emails {
                    book.add(&name, e, true);
                }
            }
        }
        for state in boxes.accounts.values() {
            for list in state.summaries.values() {
                for s in list {
                    for p in std::iter::once(&s.from).chain(&s.to).chain(&s.cc) {
                        book.add(&p.name, &p.address, false);
                    }
                }
            }
        }
        self.book = book;
        self.book_key = key;
        self.book_at = Some(Instant::now());
    }
}

/// Every account's data, as the agenda's computations take it.
pub fn sources<'a>(data: &'a BTreeMap<AccountId, AccountPim>, ws: &Workspace) -> Vec<Source<'a>> {
    ws.accounts()
        .iter()
        .filter_map(|a| data.get(&a.id).map(|d| Source { account: a.id, calendars: &d.calendars, events: &d.events, tasks: &d.tasks }))
        .collect()
}

/// The calendars that can be changed, of every account, each account's primary one first.
pub fn choices(data: &BTreeMap<AccountId, AccountPim>, ws: &Workspace) -> Vec<CalendarChoice> {
    let mut out = Vec::new();
    for a in ws.accounts() {
        let Some(d) = data.get(&a.id) else { continue };
        let mut cals: Vec<&Calendar> = d.calendars.iter().filter(|c| c.can_edit).collect();
        cals.sort_by_key(|c| !c.primary);
        for c in cals {
            out.push(CalendarChoice {
                account: a.id,
                calendar: c.id.clone(),
                label: format!("{} · {}", a.name, c.name),
                provider: a.provider,
                address: a.address.clone(),
            });
        }
    }
    out
}

/// The colour an event is drawn with: its calendar's when it has one, else its account's.
pub fn calendar_color(cx: &Ctx<'_>, account: AccountId, calendar: Option<&Calendar>) -> Color32 {
    calendar.and_then(|c| c.color).map_or_else(|| cx.color(account), t::account)
}

/// A link's host, as shown in place of the link itself.
pub fn link_domain(url: &str) -> Option<String> {
    let url = web_link(url)?;
    let rest = url.get(8..)?;
    Some(rest.split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase())
}

/// Opens a link of an event in the browser, on the user's click only: `https` links that pass
/// the model's check alone.
pub fn open_link(url: &str) -> Result<(), String> {
    let url = web_link(url).ok_or(tr("This is not a secure web link: MailALM doesn't open it."))?;
    crate::accounts::open_in_browser(&url)
}
