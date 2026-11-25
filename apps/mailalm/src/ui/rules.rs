//! Settings → Rules (C5): each account's rules in one list (the server's first, with where each
//! runs; one made elsewhere that MailALM can't show is read only and can only be deleted), the
//! editor (conditions, actions, where it runs), the preview of the cached messages a rule would
//! touch before it is saved, and the explicit confirmation a forwarding needs. A rule never runs a
//! program: the model has no such action. "Move ▾ → Create a rule from this message" opens the
//! editor with the sender filled in.

use egui::{Frame, Id, Margin, RichText, ScrollArea, Stroke, TextEdit, Ui};
use mailalm_engine::{Command, RuleCommand};
use mailalm_model::rules::{Rule, Where};
use mailalm_model::{AccountId, FolderScope, Provider, Summary};

use super::{MailApp, Page};
use crate::rule_form::{ActKind, ActRow, CondKind, CondRow, RuleForm, host_of, place_text};
use crate::theme as t;
use crate::time;

/// The most preview lines shown (the count says how many there are).
const PREVIEW_SHOWN: usize = 50;

/// The editor open.
struct Editing {
    account: AccountId,
    form: RuleForm,
    error: Option<String>,
    /// The preview asked, and what came back.
    preview: Option<(u64, Option<Vec<(String, Summary)>>)>,
    /// The preview looks in every folder, not only the Inbox.
    preview_everywhere: bool,
}

/// The rules section's state.
#[derive(Default)]
pub(in crate::ui) struct RulesPage {
    account: Option<AccountId>,
    editing: Option<Editing>,
    next_preview: u64,
    /// A rule to delete, waiting for the user's confirmation.
    delete: Option<(AccountId, Rule)>,
    /// The forwarding's confirmation is open.
    confirm_forward: bool,
    /// The accounts whose rules were asked for since MailALM started (Refresh asks again).
    listed: Vec<AccountId>,
}

impl RulesPage {
    /// A preview came back.
    pub(in crate::ui) fn preview_arrived(&mut self, account: AccountId, id: u64, hits: Vec<(String, Summary)>) {
        if let Some(e) = &mut self.editing
            && e.account == account
            && let Some((asked, found)) = &mut e.preview
            && *asked == id
        {
            *found = Some(hits);
        }
    }

    /// An account was removed.
    pub(in crate::ui) fn forget(&mut self, account: AccountId) {
        if self.account == Some(account) {
            self.account = None;
        }
        if self.editing.as_ref().is_some_and(|e| e.account == account) {
            self.editing = None;
        }
        if self.delete.as_ref().is_some_and(|(a, _)| *a == account) {
            self.delete = None;
        }
        self.listed.retain(|a| *a != account);
    }
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width().min(760.0));
        add(ui);
    });
}

/// What the rule does, in a line.
fn actions_text(r: &Rule) -> String {
    use mailalm_model::rules::Action;
    r.actions
        .iter()
        .map(|a| match a {
            Action::MoveTo(p) => format!("move to {p}"),
            Action::MarkRead => "mark as read".to_owned(),
            Action::Flag => "flag".to_owned(),
            Action::Category(c) => format!("category {c}"),
            Action::Delete => "move to the Trash".to_owned(),
            Action::Forward(to) => format!("forward to {}", to.join(", ")),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the editor asks of the main window.
enum Ask {
    List,
    Preview(Box<Rule>, FolderScope),
    Save,
    Close,
}

impl MailApp {
    /// "Move ▾ → Create a rule from this message": the editor, in Settings, with the sender.
    pub(super) fn rule_from_message(&mut self) {
        let Some(key) = self.selected.clone() else {
            self.say("Select a message first.");
            return;
        };
        let (Some(provider), Some(s)) = (self.account(key.account).map(|a| a.provider), self.summary_of(&key).cloned()) else {
            self.say("A rule is made for a message of an account.");
            return;
        };
        self.rules.account = Some(key.account);
        self.rules.editing = Some(Editing { account: key.account, form: RuleForm::from_message(provider, &s), error: None, preview: None, preview_everywhere: false });
        if !self.rules.listed.contains(&key.account) {
            self.rules.listed.push(key.account);
        }
        self.engine.send(key.account, Command::Rules(RuleCommand::List));
        self.page = Page::Settings;
        self.say("Choose what the rule does with this sender's messages, then save it (Settings, Rules).");
    }

    /// The Rules section of the settings.
    pub(super) fn rules_settings(&mut self, ui: &mut Ui) {
        let accounts: Vec<(AccountId, String, Provider)> = self.ws.accounts().iter().map(|a| (a.id, format!("{} · {}", a.name, a.address), a.provider)).collect();
        ui.add_space(20.0);
        ui.label(RichText::new("RULES").size(11.0).strong().color(t::faint()));
        ui.add_space(6.0);
        let Some(first) = accounts.first().map(|a| a.0) else {
            ui.label(RichText::new("Add an account first.").color(t::faint()));
            return;
        };
        let current = self.rules.account.filter(|a| accounts.iter().any(|(id, ..)| id == a)).unwrap_or(first);
        let provider = accounts.iter().find(|(id, ..)| *id == current).map_or(Provider::Gmail, |(.., p)| *p);
        self.rules.account = Some(current);
        let mut ask: Vec<(AccountId, Ask)> = Vec::new();
        if !self.rules.listed.contains(&current) {
            // Asked once when the account is first shown; Refresh asks again.
            ask.push((current, Ask::List));
        }
        let rules: Option<Vec<Rule>> = self.boxes.account(current).and_then(|s| s.rules.clone());
        let folders: Vec<String> = self.boxes.account(current).map(|s| s.folders.iter().map(|f| f.path.clone()).collect()).unwrap_or_default();
        let inbox = self.boxes.account(current).and_then(|s| s.inbox()).map(|f| f.path.clone());
        let mut new_rule = false;
        let mut edit: Option<Rule> = None;
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Account").color(t::muted()));
                let shown = accounts.iter().find(|(id, ..)| *id == current).map(|(_, n, _)| n.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("mailalm-rules-account").selected_text(shown).show_ui(ui, |ui| {
                    for (id, name, _) in &accounts {
                        if ui.selectable_label(*id == current, name).clicked() && *id != current {
                            self.rules.account = Some(*id);
                            self.rules.editing = None;
                            ask.push((*id, Ask::List));
                        }
                    }
                });
                if ui.button("Refresh").clicked() {
                    ask.push((current, Ask::List));
                }
                if ui.button("New rule…").clicked() {
                    new_rule = true;
                }
            });
            let server = match host_of(provider) {
                Some(_) => "Rules run on the account's server when it keeps them (Outlook's inbox rules, Gmail's filters): they work while MailALM is closed.",
                None => "An IMAP server keeps no rules MailALM can reach: MailALM runs this account's rules itself, on new mail in the Inbox, while it runs.",
            };
            ui.label(RichText::new(server).size(11.5).color(t::faint()));
            ui.add_space(6.0);
            match &rules {
                None => {
                    ui.label(RichText::new("Reading the rules…").color(t::muted()));
                }
                Some(list) if list.is_empty() => {
                    ui.label(RichText::new("No rule yet.").color(t::muted()));
                }
                Some(list) => {
                    // The server's first, as they run.
                    let mut ordered: Vec<&Rule> = list.iter().filter(|r| r.place == Where::Server).collect();
                    ordered.extend(list.iter().filter(|r| r.place == Where::Local));
                    for r in ordered {
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(r.title()).strong());
                                    if !r.enabled {
                                        ui.label(RichText::new("off").size(11.0).color(t::warn()));
                                    }
                                });
                                ui.label(RichText::new(format!("Then: {}{}", actions_text(r), if r.stop { "; then stop" } else { "" })).size(12.0).color(t::muted()));
                                ui.label(RichText::new(place_text(r.place, provider)).size(11.5).color(t::faint()));
                                if r.read_only() {
                                    ui.label(
                                        RichText::new(format!("Made elsewhere with what MailALM can't show ({}): it can only be deleted here.", r.unsupported.join(", ")))
                                            .size(11.5)
                                            .color(t::warn()),
                                    );
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Delete…").clicked() {
                                    self.rules.delete = Some((current, r.clone()));
                                }
                                if !r.read_only() && ui.small_button("Edit…").clicked() {
                                    edit = Some(r.clone());
                                }
                            });
                        });
                    }
                }
            }
            if new_rule {
                self.rules.editing = Some(Editing { account: current, form: RuleForm::new(provider), error: None, preview: None, preview_everywhere: false });
            }
            if let Some(r) = &edit {
                self.rules.editing = Some(Editing { account: current, form: RuleForm::from_rule(r), error: None, preview: None, preview_everywhere: false });
            }
            if let Some(e) = &mut self.rules.editing
                && e.account == current
            {
                ui.add_space(10.0);
                if let Some(a) = editor(ui, e, provider, &folders, &mut self.rules.next_preview, inbox.as_deref()) {
                    ask.push((current, a));
                }
            }
        });
        for (id, a) in ask {
            match a {
                Ask::List => {
                    if !self.rules.listed.contains(&id) {
                        self.rules.listed.push(id);
                    }
                    self.engine.send(id, Command::Rules(RuleCommand::List));
                }
                Ask::Preview(rule, folders) => {
                    let pid = self.rules.next_preview;
                    if let Some(e) = &mut self.rules.editing {
                        e.preview = Some((pid, None));
                    }
                    self.engine.send(id, Command::Rules(RuleCommand::Preview { id: pid, rule, folders }));
                }
                Ask::Save => self.save_rule(provider),
                Ask::Close => self.rules.editing = None,
            }
        }
        self.rules_dialogs(ui.ctx(), provider);
    }

    /// Saves the rule edited, once the forwarding (if any) is confirmed.
    fn save_rule(&mut self, provider: Provider) {
        let Some(e) = &mut self.rules.editing else { return };
        if e.form.needs_forward_confirmation() && e.form.place == Where::Server {
            self.rules.confirm_forward = true;
            return;
        }
        match e.form.build(provider) {
            Ok(rule) => {
                let account = e.account;
                let name = rule.title();
                if self.engine.send(account, Command::Rules(RuleCommand::Save(Box::new(rule)))) {
                    self.rules.editing = None;
                    self.say(format!("Saving the rule \"{name}\"… the list shows it once the account has it."));
                } else if let Some(e) = &mut self.rules.editing {
                    e.error = Some("The account is not running: the rule was not saved.".to_owned());
                }
            }
            Err(err) => e.error = Some(err),
        }
    }

    fn rules_dialogs(&mut self, ctx: &egui::Context, provider: Provider) {
        if self.rules.confirm_forward {
            let to = self
                .rules
                .editing
                .as_ref()
                .map(|e| e.form.actions.iter().filter(|a| a.kind == ActKind::Forward).map(|a| a.value.clone()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-confirm-forward")).show(ctx, |ui| {
                ui.set_width(500.0);
                ui.heading("Forward these messages?");
                ui.add_space(6.0);
                ui.label(format!(
                    "This rule makes the account's server send a copy of every message it matches to {to}, from now on, even while MailALM is closed, without asking again."
                ));
                ui.label(RichText::new("Mail leaves your account this way without you seeing it: make sure the address is yours or someone you trust, and that the conditions are narrow.").color(t::warn()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Forward these messages").clicked() {
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
                self.rules.confirm_forward = false;
                if yes {
                    if let Some(e) = &mut self.rules.editing {
                        e.form.forward_confirmed = true;
                    }
                    self.save_rule(provider);
                }
            }
        }
        if let Some((account, rule)) = self.rules.delete.clone() {
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-delete-rule")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.heading("Delete this rule?");
                ui.add_space(6.0);
                ui.label(RichText::new(rule.title()).strong());
                ui.label(match rule.place {
                    Where::Server => "It is deleted from the account's server: it stops running there.",
                    Where::Local => "MailALM stops running it.",
                });
                ui.add_space(8.0);
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
                self.rules.delete = None;
                if yes && !self.engine.send(account, Command::Rules(RuleCommand::Delete { id: rule.id.clone(), place: rule.place })) {
                    self.warn("The account is not running: the rule was not deleted.");
                }
            }
        }
    }
}

/// The editor; what it asks of the main window.
fn editor(ui: &mut Ui, e: &mut Editing, provider: Provider, folders: &[String], next_preview: &mut u64, inbox: Option<&str>) -> Option<Ask> {
    let mut ask = None;
    let server_ok = host_of(provider).is_some();
    Frame::new().fill(t::raised()).corner_radius(8).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.label(RichText::new(if e.form.id.is_empty() { "New rule" } else { "Change the rule" }).strong());
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(TextEdit::singleline(&mut e.form.name).desired_width(320.0).hint_text(if provider == Provider::Gmail { "Gmail filters have no name" } else { "" }));
        });
        ui.horizontal(|ui| {
            ui.label("Runs");
            ui.add_enabled_ui(server_ok && e.form.id.is_empty(), |ui| {
                ui.radio_value(&mut e.form.place, Where::Server, "On the server");
            });
            ui.add_enabled_ui(e.form.id.is_empty(), |ui| {
                ui.radio_value(&mut e.form.place, Where::Local, "In MailALM");
            });
        });
        ui.label(RichText::new(place_text(e.form.place, provider)).size(11.5).color(t::faint()));
        ui.horizontal(|ui| {
            ui.checkbox(&mut e.form.enabled, "On");
            ui.checkbox(&mut e.form.stop, "Then stop: the rules after it don't run on these messages");
        });
        ui.add_space(6.0);
        ui.label(RichText::new("WHEN EVERY CONDITION HOLDS (words separated by commas: any of them)").size(11.0).color(t::faint()));
        let mut remove: Option<usize> = None;
        for (i, c) in e.form.conditions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("mailalm-rule-cond", i)).selected_text(c.kind.label()).width(190.0).show_ui(ui, |ui| {
                    for k in CondKind::ALL {
                        ui.selectable_value(&mut c.kind, k, k.label());
                    }
                });
                if c.kind != CondKind::HasAttachment {
                    ui.add(TextEdit::singleline(&mut c.words).desired_width(300.0));
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            e.form.conditions.remove(i);
        }
        if ui.small_button("Add a condition").clicked() {
            e.form.conditions.push(CondRow { kind: CondKind::Subject, words: String::new() });
        }
        ui.add_space(6.0);
        ui.label(RichText::new("THEN").size(11.0).color(t::faint()));
        let mut remove: Option<usize> = None;
        for (i, a) in e.form.actions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("mailalm-rule-act", i)).selected_text(a.kind.label()).width(190.0).show_ui(ui, |ui| {
                    for k in ActKind::ALL {
                        ui.selectable_value(&mut a.kind, k, k.label());
                    }
                });
                match a.kind {
                    ActKind::MoveTo => {
                        let shown = if a.value.is_empty() { "Choose a folder".to_owned() } else { a.value.clone() };
                        egui::ComboBox::from_id_salt(("mailalm-rule-folder", i)).selected_text(shown).width(260.0).show_ui(ui, |ui| {
                            for f in folders {
                                ui.selectable_value(&mut a.value, f.clone(), f.as_str());
                            }
                        });
                    }
                    ActKind::Category => {
                        ui.add(TextEdit::singleline(&mut a.value).desired_width(200.0).hint_text("Category or label"));
                    }
                    ActKind::Forward => {
                        ui.add(TextEdit::singleline(&mut a.value).desired_width(260.0).hint_text("address@example.com"));
                    }
                    _ => {}
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            e.form.actions.remove(i);
        }
        if ui.small_button("Add an action").clicked() {
            e.form.actions.push(ActRow { kind: ActKind::MarkRead, value: String::new() });
        }
        if e.form.forwards() {
            ui.label(
                RichText::new("Forwarding is only for a rule on the server, and MailALM asks you to confirm it when you save: it sends your mail to another address.")
                    .size(11.5)
                    .color(t::warn()),
            );
        }
        ui.label(RichText::new("A rule never starts a program: MailALM has no such action.").size(11.0).color(t::faint()));
        if let Some(err) = &e.error {
            ui.label(RichText::new(err).color(t::danger()));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Preview the messages it touches").on_hover_text("Looks through the messages MailALM has cached; nothing is changed").clicked() {
                let folders = match (e.preview_everywhere, inbox) {
                    (false, Some(p)) => FolderScope::Folder(p.to_owned()),
                    _ => FolderScope::Account,
                };
                *next_preview += 1;
                ask = Some(Ask::Preview(Box::new(e.form.rule()), folders));
            }
            ui.checkbox(&mut e.preview_everywhere, "in every folder, not only the Inbox");
        });
        match &e.preview {
            Some((_, None)) => {
                ui.label(RichText::new("Looking through the cached messages…").color(t::muted()));
            }
            Some((_, Some(hits))) => {
                ui.label(RichText::new(format!("{} cached message{} would be touched", hits.len(), if hits.len() == 1 { "" } else { "s" })).strong());
                ScrollArea::vertical().id_salt("mailalm-rule-preview").max_height(200.0).show(ui, |ui| {
                    for (folder, s) in hits.iter().take(PREVIEW_SHOWN) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(time::short(s.date)).size(12.0).color(t::faint()));
                            ui.label(RichText::new(s.from.display()).size(12.5));
                            ui.label(RichText::new(if s.subject.is_empty() { "(no subject)" } else { s.subject.as_str() }).size(12.5).color(t::muted()));
                            ui.label(RichText::new(folder).size(11.5).color(t::faint()));
                        });
                    }
                });
            }
            None => {}
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Save the rule").clicked() {
                ask = Some(Ask::Save);
            }
            if ui.button("Cancel").clicked() {
                ask = Some(Ask::Close);
            }
        });
    });
    ask
}
