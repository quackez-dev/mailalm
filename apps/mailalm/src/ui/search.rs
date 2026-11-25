//! The search (PLAN.md, section 6): the box with its scope menu (the current account, accounts
//! ticked, or all of them; the current folder, with its subfolders, or the whole account; the
//! servers asked too), the operators' hint, the server's results below the cache's, and the
//! searches kept as search folders in the folder pane (opened, renamed, deleted).

use egui::{Id, Key, RichText, TextEdit, Ui};
use mailalm_engine::Command;
use mailalm_model::{AccountId, FolderScope, Summary};

use super::{MailApp, Page, SearchState, View, folder_row, section};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::local::{LOCAL, LOCAL_NAME};
use crate::search::{self, AccountsChoice, FoldersChoice, Hit, OPERATORS, Scope};
use crate::theme as t;

/// The search box's state beyond its text.
#[derive(Default)]
pub(in crate::ui) struct SearchUi {
    pub scope: Scope,
    /// The name being typed to keep the search shown as a search folder.
    saving: Option<String>,
    /// The search folder being renamed, and the name typed.
    renaming: Option<(usize, String)>,
    /// What went wrong saving or renaming, shown in its dialog.
    error: Option<String>,
}

impl MailApp {
    /// Every account a search can cover, and the mail on this PC when there is some (last).
    fn searchable(&self) -> Vec<AccountId> {
        let mut ids: Vec<AccountId> = self.ws.accounts().iter().map(|a| a.id).collect();
        if self.boxes.account(LOCAL).is_some_and(|s| !s.folders.is_empty()) {
            ids.push(LOCAL);
        }
        ids
    }

    fn name_of(&self, id: AccountId) -> String {
        if id == LOCAL { tr(LOCAL_NAME).to_owned() } else { self.account(id).map(|a| a.name.clone()).unwrap_or_default() }
    }

    /// What the scope chosen covers now.
    fn search_targets(&self) -> Vec<(AccountId, FolderScope)> {
        let current: Option<(AccountId, Option<String>)> = match &self.view {
            View::Folder(id, folder) => Some((*id, self.folder_path(*id, folder))),
            _ => self.ws.focused_account().map(|a| (a.id, None)),
        };
        search::resolve(&self.search_ui.scope, &self.searchable(), current.as_ref().map(|(id, p)| (*id, p.as_deref())))
    }

    /// The scope's label on its menu button.
    fn scope_label(&self) -> String {
        let s = &self.search_ui.scope;
        let accounts = match s.accounts {
            AccountsChoice::Current => match &self.view {
                View::Folder(id, _) => self.name_of(*id),
                _ => self.ws.focused_account().map_or_else(|| tr("All accounts").to_owned(), |a| a.name.clone()),
            },
            AccountsChoice::Chosen => trf!("{count} accounts", count = s.chosen.len()),
            AccountsChoice::All => tr("All accounts").to_owned(),
        };
        let folders = match s.folders {
            FoldersChoice::Current => Some(tr("this folder")),
            FoldersChoice::WithSubfolders => Some(tr("this folder and under")),
            FoldersChoice::Account => None,
        };
        let mut label = accounts;
        if let Some(f) = folders {
            label.push_str(" · ");
            label.push_str(f);
        }
        if s.server {
            label.push_str(" · ");
            label.push_str(tr("+ server"));
        }
        label
    }

    /// The search box and its scope menu, right to left in the command bar.
    pub(super) fn search_bar(&mut self, ui: &mut Ui) {
        if self.search_state.is_some() && ui.small_button(tr("Clear")).on_hover_text(tr("Back to the folder")).clicked() {
            self.clear_search();
        }
        if self.search_state.as_ref().is_some_and(|s| s.name.is_none()) && ui.small_button(tr("Keep…")).on_hover_text(tr("Keep this search as a search folder")).clicked() {
            self.search_ui.saving = Some(String::new());
            self.search_ui.error = None;
        }
        self.scope_menu(ui);
        let edit = ui.add(TextEdit::singleline(&mut self.search).hint_text(tr("Search — from:, subject:, has:attachment")).desired_width(240.0)).on_hover_text(tr(OPERATORS));
        if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            self.run_search();
        }
        if edit.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.clear_search();
        }
    }

    fn scope_menu(&mut self, ui: &mut Ui) {
        let label = self.scope_label();
        let accounts: Vec<(AccountId, String)> = self.searchable().into_iter().map(|id| (id, self.name_of(id))).collect();
        let in_folder = matches!(self.view, View::Folder(..));
        let scope = &mut self.search_ui.scope;
        let _ = ui.menu_button((RichText::new(label).size(12.0).color(t::muted()), icons::atom(Icon::ChevronDown)), |ui| {
            ui.label(RichText::new(tr("ACCOUNTS")).size(11.0).strong().color(t::faint()));
            ui.radio_value(&mut scope.accounts, AccountsChoice::Current, tr("The current account"));
            ui.radio_value(&mut scope.accounts, AccountsChoice::Chosen, tr("The accounts ticked below"));
            if scope.accounts == AccountsChoice::Chosen {
                ui.indent("mailalm-search-accounts", |ui| {
                    for (id, name) in &accounts {
                        let mut on = scope.chosen.contains(id);
                        if ui.checkbox(&mut on, name).changed() {
                            scope.chosen.retain(|a| a != id);
                            if on {
                                scope.chosen.push(*id);
                            }
                        }
                    }
                });
            }
            ui.radio_value(&mut scope.accounts, AccountsChoice::All, tr("All accounts"));
            ui.separator();
            ui.label(RichText::new(tr("FOLDERS")).size(11.0).strong().color(t::faint()));
            ui.add_enabled_ui(in_folder, |ui| {
                ui.radio_value(&mut scope.folders, FoldersChoice::Current, tr("The current folder"));
                ui.radio_value(&mut scope.folders, FoldersChoice::WithSubfolders, tr("The current folder and its subfolders"));
            });
            ui.radio_value(&mut scope.folders, FoldersChoice::Account, tr("The whole account"));
            if !in_folder {
                ui.label(RichText::new(tr("Open a folder to search it alone.")).size(11.0).color(t::faint()));
            }
            ui.separator();
            ui.checkbox(&mut scope.server, tr("Search the server too"))
                .on_hover_text(tr("Outlook and Gmail search the whole mailbox, an IMAP server its folders: what they find that MailALM doesn't have comes below, \"from the server\"."));
            ui.separator();
            ui.label(RichText::new(tr("OPERATORS")).size(11.0).strong().color(t::faint()));
            ui.label(RichText::new(tr(OPERATORS)).size(11.5).color(t::muted()));
        });
    }

    /// Runs what the search box says, over the scope chosen.
    pub(super) fn run_search(&mut self) {
        let query = self.search.trim().to_owned();
        if query.is_empty() {
            self.clear_search();
            return;
        }
        let targets = self.search_targets();
        if targets.is_empty() {
            self.say(tr("No account is ticked in the search's scope."));
            return;
        }
        let server = self.search_ui.scope.server;
        self.start_search(query, targets, server, None);
    }

    fn start_search(&mut self, query: String, targets: Vec<(AccountId, FolderScope)>, server: bool, name: Option<String>) {
        let id = self.next_search;
        self.next_search += 1;
        let (mut waiting, mut server_waiting) = (0, 0);
        for (account, folders) in &targets {
            let ask = server && *account != LOCAL;
            if self.send(*account, Command::Find { id, query: query.clone(), folders: folders.clone(), server: ask }) {
                waiting += 1;
                if ask {
                    server_waiting += 1;
                }
            }
        }
        self.search_state = Some(SearchState {
            id,
            query,
            name,
            waiting,
            hits: Vec::new(),
            server_waiting,
            server: Vec::new(),
            server_errors: Vec::new(),
            targets,
            server_asked: server,
        });
        self.view = View::Search;
        self.page = Page::Mail;
    }

    pub(super) fn clear_search(&mut self) {
        self.search.clear();
        self.search_state = None;
        if self.view == View::Search {
            self.view = match self.ws.focused_account() {
                Some(a) => View::Folder(a.id, None),
                None => View::Unified,
            };
        }
    }

    /// A server's results: below the cache's, without what the cache found already.
    pub(super) fn server_results(&mut self, account: AccountId, id: u64, result: Result<Vec<(String, Summary)>, String>) {
        let name = self.name_of(account);
        let Some(s) = &mut self.search_state else { return };
        if s.id != id {
            return;
        }
        s.server_waiting = s.server_waiting.saturating_sub(1);
        match result {
            Ok(found) => {
                let hit = |(account, folder, summary): (AccountId, String, Summary)| Hit { account, folder, summary };
                let local: Vec<Hit> = s.hits.iter().cloned().map(hit).collect();
                let all: Vec<Hit> = s.server.drain(..).map(hit).chain(found.into_iter().map(|(folder, summary)| Hit { account, folder, summary })).collect();
                s.server = search::server_only(&local, all).into_iter().map(|h| (h.account, h.folder, h.summary)).collect();
            }
            Err(e) => s.server_errors.push(format!("{name}: {e}")),
        }
    }

    /// The message list's subtitle in a search.
    pub(super) fn search_subtitle(&self) -> String {
        let Some(s) = &self.search_state else { return String::new() };
        if s.waiting > 0 {
            return tr("searching…").to_owned();
        }
        let mut text = trf!("{count} found in MailALM", count = s.hits.len());
        if s.server_waiting > 0 {
            text.push_str(" · ");
            text.push_str(tr("asking the servers…"));
        } else if !s.server.is_empty() {
            text.push_str(" · ");
            text.push_str(&trf!("{count} more from the server", count = s.server.len()));
        }
        if let Some(e) = s.server_errors.first() {
            text.push_str(&format!(" · {e}"));
        }
        text
    }

    // ---- search folders ----

    /// The search folders in the folder pane, and their dialogs.
    pub(super) fn saved_searches_pane(&mut self, ui: &mut Ui) {
        let names: Vec<String> = self.settings.saved_searches.iter().map(|s| s.name.clone()).collect();
        if !names.is_empty() {
            section(ui, tr("SEARCH FOLDERS"));
        }
        let open_name = match (&self.view, &self.search_state) {
            (View::Search, Some(s)) => s.name.clone(),
            _ => None,
        };
        let mut open: Option<usize> = None;
        let mut delete: Option<usize> = None;
        for (i, name) in names.iter().enumerate() {
            let row = folder_row(ui, 10.0, open_name.as_deref() == Some(name.as_str()), name, 0).on_hover_text(tr("A search folder: right-click to rename or delete it"));
            if row.clicked() {
                open = Some(i);
            }
            row.context_menu(|ui| {
                if ui.button(tr("Rename…")).clicked() {
                    self.search_ui.renaming = Some((i, name.clone()));
                    self.search_ui.error = None;
                    ui.close();
                }
                if ui.button(tr("Delete")).clicked() {
                    delete = Some(i);
                    ui.close();
                }
            });
        }
        if let Some(i) = open {
            self.open_saved(i);
        }
        if let Some(i) = delete
            && i < self.settings.saved_searches.len()
        {
            let gone = self.settings.saved_searches.remove(i);
            self.keep_settings(trf!("The search folder \"{name}\" was deleted.", name = gone.name));
        }
        self.search_dialogs(ui.ctx());
    }

    fn open_saved(&mut self, i: usize) {
        let Some(s) = self.settings.saved_searches.get(i).cloned() else { return };
        let targets = search::resolve_saved(&s, &self.searchable());
        if targets.is_empty() {
            self.say(tr("The accounts this search folder covered are gone."));
            return;
        }
        self.search = s.query.clone();
        self.start_search(s.query, targets, s.server, Some(s.name));
    }

    /// Saves the settings, and says it (or why it couldn't).
    fn keep_settings(&mut self, done: String) {
        match self.settings.save() {
            Ok(()) => self.say(done),
            Err(e) => self.warn(e),
        }
    }

    fn search_dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut name) = self.search_ui.saving.take() {
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-save-search")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading(tr("Keep as a search folder"));
                ui.label(RichText::new(tr("It appears under the accounts and searches again, the same way, when opened.")).color(t::muted()));
                ui.add_space(6.0);
                let field = ui.add(TextEdit::singleline(&mut name).hint_text(tr("Name")).desired_width(f32::INFINITY));
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    answer = Some(true);
                }
                if let Some(e) = &self.search_ui.error {
                    ui.label(RichText::new(e).color(t::danger()));
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("Keep")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            if m.should_close() && answer.is_none() {
                answer = Some(false);
            }
            match answer {
                None => self.search_ui.saving = Some(name),
                Some(false) => self.search_ui.error = None,
                Some(true) => {
                    let all = self.searchable();
                    let made = self.search_state.as_ref().and_then(|s| search::to_saved(&name, &s.query, &s.targets, &all, s.server_asked));
                    let result = match made {
                        None => Err(tr("A search folder needs a name.").to_owned()),
                        Some(saved) => search::add_saved(&mut self.settings.saved_searches, saved),
                    };
                    match result {
                        Ok(()) => {
                            let name = name.trim().to_owned();
                            if let Some(s) = &mut self.search_state {
                                s.name = Some(name.clone());
                            }
                            self.search_ui.error = None;
                            self.keep_settings(trf!("The search folder \"{name}\" was made.", name = name));
                        }
                        Err(e) => {
                            self.search_ui.error = Some(e);
                            self.search_ui.saving = Some(name);
                        }
                    }
                }
            }
        }
        if let Some((index, mut name)) = self.search_ui.renaming.take() {
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-rename-search")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading(tr("Rename the search folder"));
                ui.add_space(6.0);
                let field = ui.add(TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    answer = Some(true);
                }
                if let Some(e) = &self.search_ui.error {
                    ui.label(RichText::new(e).color(t::danger()));
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("Rename")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        answer = Some(false);
                    }
                });
            });
            if m.should_close() && answer.is_none() {
                answer = Some(false);
            }
            match answer {
                None => self.search_ui.renaming = Some((index, name)),
                Some(false) => self.search_ui.error = None,
                Some(true) => {
                    let old = self.settings.saved_searches.get(index).map(|s| s.name.clone());
                    match search::rename_saved(&mut self.settings.saved_searches, index, &name) {
                        Ok(()) => {
                            let new = self.settings.saved_searches.get(index).map(|s| s.name.clone()).unwrap_or_default();
                            if let Some(s) = &mut self.search_state
                                && s.name.is_some()
                                && s.name == old
                            {
                                s.name = Some(new.clone());
                            }
                            self.search_ui.error = None;
                            self.keep_settings(trf!("The search folder is now \"{name}\".", name = new));
                        }
                        Err(e) => {
                            self.search_ui.error = Some(e);
                            self.search_ui.renaming = Some((index, name));
                        }
                    }
                }
            }
        }
    }
}
