//! Settings → MCP (PLAN.md, section 11): MCP access on or off (off by default), which starts or
//! stops the control server on 127.0.0.1 (its key and port files in the profile); the command
//! line to give an MCP client; the accounts the MCP may see; the journal of calls. Each frame,
//! the requests waiting are taken without blocking (`try_next`) and answered from
//! [`crate::mcp::AppBackend`]; a sensitive action waits in a dialog that shows exactly what will be
//! done (approve or decline), and a triage proposal (M2) as a list with a box per line.

use std::collections::VecDeque;
use std::time::Duration;

use egui::{Frame, Id, Margin, RichText, ScrollArea, Stroke, Ui};
use mailalm_control::types::{ProposeTriage, TriageAction};
use mailalm_control::{Call, Confirmation, ControlServer, Journal, JournalEntry, Kind, Outcome, PendingCall, Tool};

use super::MailApp;
use crate::mcp::{AppBackend, MailSource, StoreSource, journal_line};
use crate::settings::Settings;
use crate::theme as t;
use crate::time;

/// Requests taken in one frame at most (the others wait for the next).
const PER_FRAME: usize = 16;
/// Journal lines shown.
const JOURNAL_SHOWN: usize = 200;

/// The MCP access's state.
#[derive(Default)]
pub(in crate::ui) struct McpState {
    server: Option<ControlServer>,
    journal: Journal,
    /// Sensitive actions and triages waiting for the user, oldest first.
    pending: VecDeque<PendingCall>,
    /// The lines ticked of the triage shown (the first pending call).
    ticked: Vec<bool>,
    /// Why the server could not start.
    error: Option<String>,
}

/// The `mailalm-mcp` program beside MailALM's own.
fn mcp_program() -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) { "mailalm-mcp.exe" } else { "mailalm-mcp" };
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(name)))
}

/// One triage action in words.
fn action_text(a: &TriageAction) -> String {
    match a {
        TriageAction::Move { folder } => format!("move to “{folder}”"),
        TriageAction::Category { category } => format!("category “{category}” (not possible yet)"),
        TriageAction::MarkRead => "mark as read".to_owned(),
        TriageAction::Flag => "flag".to_owned(),
        TriageAction::Delete => "move to the Trash".to_owned(),
        TriageAction::BlockSender => "block the sender (Gmail filter)".to_owned(),
    }
}

impl MailApp {
    /// Runs one call's work with the backend over the app's state.
    fn with_backend<R>(&mut self, f: impl FnOnce(&mut AppBackend<'_>, &mut Journal) -> R) -> R {
        let source = StoreSource { base: Settings::cache_dir() };
        let zones = crate::zones::TzRules;
        let mut backend = AppBackend {
            accounts: self.ws.accounts(),
            hidden: &self.settings.mcp_hidden,
            pim: &self.pim.data,
            mail: &source,
            commands: &self.engine,
            zones: &zones,
            done: Vec::new(),
        };
        let r = f(&mut backend, &mut self.mcp.journal);
        let done = std::mem::take(&mut backend.done);
        for text in done {
            self.say(text);
        }
        r
    }

    /// Starts listening (MCP access turned on).
    pub(super) fn start_mcp(&mut self) {
        if self.mcp.server.is_some() {
            return;
        }
        let Some(dir) = mailalm_control::profile_dir() else {
            self.mcp.error = Some("The local application data folder is unknown: MCP access can't start.".to_owned());
            return;
        };
        let wake = std::sync::Arc::clone(&self.wake);
        match ControlServer::start(&dir, move || wake()) {
            Ok(server) => {
                self.mcp.server = Some(server);
                self.mcp.error = None;
            }
            Err(e) => self.mcp.error = Some(format!("MCP access could not start: {e}")),
        }
    }

    /// Stops listening; what waited for the user is declined.
    fn stop_mcp(&mut self) {
        for p in std::mem::take(&mut self.mcp.pending) {
            p.decline(&mut self.mcp.journal);
        }
        if let Some(s) = self.mcp.server.take() {
            s.stop();
        }
    }

    /// The requests waiting: answered, or put aside for the user's decision. Never blocks.
    pub(super) fn mcp_tick(&mut self, ui: &mut Ui) {
        let Some(server) = &self.mcp.server else { return };
        let incoming: Vec<mailalm_control::Incoming> = std::iter::from_fn(|| server.try_next()).take(PER_FRAME).collect();
        if incoming.len() == PER_FRAME {
            ui.ctx().request_repaint();
        }
        for request in incoming {
            let waiting = self.with_backend(|backend, journal| mailalm_control::handle(backend, journal, request));
            if let Some(p) = waiting {
                self.mcp.pending.push_back(p);
            }
        }
        // The agent stopped waiting: the dialog goes, nothing is done.
        let before = self.mcp.pending.len();
        let mut kept = VecDeque::new();
        for p in std::mem::take(&mut self.mcp.pending) {
            if p.is_expired() {
                let c = p.call();
                self.mcp.journal.record(JournalEntry::now(c.tool(), c.accounts(), Outcome::Expired, Confirmation::NotNeeded, p.describe()));
            } else {
                kept.push_back(p);
            }
        }
        self.mcp.pending = kept;
        if self.mcp.pending.len() < before {
            self.mcp.ticked.clear();
            self.say("An agent stopped waiting for your answer: nothing was done.");
        }
        if !self.mcp.pending.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }

    fn account_label(&self, id: u32) -> String {
        self.ws.accounts().iter().find(|a| a.id.0 == id).map_or_else(|| format!("account {id}"), |a| format!("{} ({})", a.name, a.address))
    }

    /// The dialog of the first call waiting for the user.
    pub(super) fn mcp_dialogs(&mut self, ui: &mut Ui) {
        let Some(p) = self.mcp.pending.front() else { return };
        let call = p.call().clone();
        let describe = p.describe();
        let accounts: Vec<String> = call.accounts().into_iter().map(|a| self.account_label(a)).collect();
        let mut decision: Option<Option<Vec<usize>>> = None;
        let mut declined = false;
        match &call {
            Call::ProposeTriage(triage) => {
                if self.mcp.ticked.len() != triage.proposals.len() {
                    self.mcp.ticked = vec![true; triage.proposals.len()];
                }
                let lines = self.triage_lines(triage);
                let ticked = &mut self.mcp.ticked;
                let _ = egui::Modal::new(Id::unique("mailalm-mcp-triage")).show(ui.ctx(), |ui| {
                    ui.set_width(640.0);
                    ui.heading("An agent proposes a triage");
                    ui.label(RichText::new("Tick the lines to apply. Nothing is done until you choose; the subjects and senders below come from the messages, the reasons from the agent.").size(12.0).color(t::muted()));
                    ui.add_space(6.0);
                    ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                        for (i, (message, actions, reason)) in lines.iter().enumerate() {
                            if let Some(on) = ticked.get_mut(i) {
                                ui.horizontal(|ui| {
                                    ui.checkbox(on, "");
                                    ui.vertical(|ui| {
                                        ui.label(RichText::new(message).strong());
                                        ui.label(RichText::new(actions).color(t::accent_text()));
                                        if !reason.is_empty() {
                                            ui.label(RichText::new(format!("Agent's reason: {reason}")).size(12.0).color(t::faint()));
                                        }
                                    });
                                });
                                ui.separator();
                            }
                        }
                    });
                    ui.add_space(6.0);
                    let n = ticked.iter().filter(|x| **x).count();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(n > 0, egui::Button::new(format!("Apply the {n} ticked"))).clicked() {
                            decision = Some(Some(ticked.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect()));
                        }
                        if ui.button("Apply all").clicked() {
                            decision = Some(None);
                        }
                        if ui.button("Decline").clicked() {
                            declined = true;
                        }
                    });
                });
            }
            _ => {
                let _ = egui::Modal::new(Id::unique("mailalm-mcp-confirm")).show(ui.ctx(), |ui| {
                    ui.set_width(560.0);
                    ui.heading(format!("An agent asks: {}", call.tool().title()));
                    ui.label(RichText::new("MailALM does exactly this, and nothing else, if you approve:").size(12.0).color(t::muted()));
                    if !accounts.is_empty() {
                        ui.label(RichText::new(format!("Account: {}", accounts.join(", "))).size(12.0).color(t::faint()));
                    }
                    ui.add_space(6.0);
                    Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(6).inner_margin(Margin::same(10)).show(ui, |ui| {
                        ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&describe).monospace()).wrap());
                        });
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Approve").clicked() {
                            decision = Some(None);
                        }
                        if ui.button("Decline").clicked() {
                            declined = true;
                        }
                    });
                });
            }
        }
        if declined && let Some(p) = self.mcp.pending.pop_front() {
            p.decline(&mut self.mcp.journal);
            self.mcp.ticked.clear();
            self.say("Declined: the agent was told nothing was done.");
        } else if let Some(lines) = decision
            && let Some(p) = self.mcp.pending.pop_front()
        {
            self.mcp.ticked.clear();
            self.with_backend(|backend, journal| match lines {
                Some(l) => p.approve_lines(backend, journal, &l),
                None => p.approve(backend, journal),
            });
        }
    }

    /// Each triage line: the message (from the cache: sender and subject), its actions, the
    /// agent's reason.
    fn triage_lines(&self, triage: &ProposeTriage) -> Vec<(String, String, String)> {
        let source = StoreSource { base: Settings::cache_dir() };
        triage
            .proposals
            .iter()
            .map(|p| {
                let m = &p.message;
                let summary = source.summaries(mailalm_model::AccountId(m.account), &m.folder).into_iter().find(|s| s.uid == m.uid);
                let message = match summary {
                    Some(s) => format!("{} — {} ({}, {})", s.from.display(), if s.subject.is_empty() { "(no subject)" } else { s.subject.as_str() }, m.folder, self.account_label(m.account)),
                    None => format!("message {} in {} ({}), not in MailALM's cache", m.uid, m.folder, self.account_label(m.account)),
                };
                let actions = p.actions.iter().map(action_text).collect::<Vec<_>>().join(", ");
                (message, actions, p.reason.clone())
            })
            .collect()
    }

    /// The MCP section of the settings.
    pub(super) fn mcp_settings(&mut self, ui: &mut Ui) {
        ui.add_space(20.0);
        ui.label(RichText::new("MCP (AI AGENTS)").size(11.0).strong().color(t::faint()));
        ui.add_space(6.0);
        let mut changed = false;
        let mut toggle: Option<bool> = None;
        let mut clear = false;
        let program = mcp_program();
        Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width().min(760.0));
            let mut on = self.settings.mcp_enabled;
            if ui.checkbox(&mut on, "Let an AI agent use MailALM through MCP").changed() {
                toggle = Some(on);
            }
            ui.label(
                RichText::new(
                    "Off by default. While it is on, MailALM listens on 127.0.0.1 only, with a new random key at each start written in your profile; the agent's program (mailalm-mcp) reads it. Mail is handed to the agent marked as untrusted data; attachments' content never is. Sending, deleting, blocking, answering an invitation and changing a rule wait for your approval here; a triage proposal shows as a list you tick.",
                )
                .size(11.5)
                .color(t::faint()),
            );
            match &self.mcp.server {
                Some(s) => {
                    ui.label(RichText::new(format!("Listening on 127.0.0.1:{} · key and port in {}", s.port(), s.dir().display())).size(12.0).color(t::ok()));
                    if s.refused() > 0 {
                        ui.label(RichText::new(format!("{} connections refused (wrong key, or not from this PC)", s.refused())).size(12.0).color(t::warn()));
                    }
                }
                None if self.settings.mcp_enabled => {
                    ui.label(RichText::new("Not listening.").size(12.0).color(t::warn()));
                }
                None => {}
            }
            if let Some(e) = &self.mcp.error {
                ui.label(RichText::new(e).color(t::danger()));
            }
            ui.add_space(6.0);
            ui.label(RichText::new("The program to give your MCP client (stdio, no arguments):").size(12.0));
            match &program {
                Some(path) => {
                    let text = path.display().to_string();
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(&text).monospace()).selectable(true).wrap());
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(text.clone());
                        }
                    });
                    let config = serde_json::json!({ "mcpServers": { "mailalm": { "command": text } } });
                    let config = serde_json::to_string_pretty(&config).unwrap_or_default();
                    ui.label(RichText::new("For a client configured with JSON (Claude Desktop and others):").size(11.5).color(t::faint()));
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(&config).monospace().size(11.5)).selectable(true));
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(config.clone());
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("mailalm-mcp.exe, in MailALM's folder.").monospace());
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Accounts the MCP may see").size(12.0));
            let accounts: Vec<(mailalm_model::AccountId, String)> = self.ws.accounts().iter().map(|a| (a.id, format!("{} · {}", a.name, a.address))).collect();
            for (id, name) in accounts {
                let mut visible = self.settings.mcp_visible(id);
                if ui.checkbox(&mut visible, name).changed() {
                    self.settings.set_mcp_visible(id, visible);
                    changed = true;
                }
            }
            ui.add_space(6.0);
            let confirmed: Vec<&str> = Tool::ALL.iter().filter(|t| matches!(t.kind(), Kind::Sensitive | Kind::Triage)).map(|t| t.title()).collect();
            ui.label(RichText::new(format!("Asked here first: {}.", confirmed.join(", "))).size(11.5).color(t::faint()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("JOURNAL · {} calls", self.mcp.journal.len())).size(11.0).strong().color(t::faint()));
                if !self.mcp.journal.is_empty() && ui.small_button("Clear").clicked() {
                    clear = true;
                }
            });
            if self.mcp.journal.is_empty() {
                ui.label(RichText::new("No call yet. The journal is kept until MailALM closes.").size(12.0).color(t::faint()));
            }
            let names = |a: u32| self.ws.accounts().iter().find(|x| x.id.0 == a).map(|x| x.name.clone());
            ScrollArea::vertical().id_salt("mailalm-mcp-journal").max_height(300.0).show(ui, |ui| {
                for e in self.mcp.journal.entries().rev().take(JOURNAL_SHOWN) {
                    let line = journal_line(e, names, time::long);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&line.time).size(11.5).color(t::faint()));
                        ui.label(RichText::new(line.tool).size(12.0).strong());
                        ui.label(RichText::new(&line.accounts).size(11.5).color(t::muted()));
                        ui.label(RichText::new(&line.outcome).size(11.5).color(if line.ok { t::ok() } else { t::warn() }));
                        if !line.confirmation.is_empty() {
                            ui.label(RichText::new(line.confirmation).size(11.5).color(t::accent_text()));
                        }
                    });
                    if !line.detail.is_empty() {
                        ui.label(RichText::new(&line.detail).size(11.0).color(t::faint()));
                    }
                }
            });
        });
        if clear {
            self.mcp.journal.clear();
        }
        if let Some(on) = toggle {
            self.settings.mcp_enabled = on;
            changed = true;
            if on {
                self.start_mcp();
            } else {
                self.stop_mcp();
                self.mcp.error = None;
            }
        }
        if changed && let Err(e) = self.settings.save() {
            self.warn(e);
        }
    }
}
