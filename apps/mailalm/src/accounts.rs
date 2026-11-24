//! Adding and changing an account (PLAN.md, section 4): the dialog that signs in once before the
//! account is kept. IMAP accounts (Vidéotron, Bell, other) sign in with a password; Hotmail and
//! Outlook.com accounts in the browser, on Microsoft's page (phase 3). Gmail comes in phase 4.

use std::sync::mpsc::{self, Receiver};

use egui::{Align, ComboBox, Grid, Id, Layout, Modal, RichText, TextEdit, Ui};
use mailalm_engine::MicrosoftAccount;
use mailalm_model::{Account, AccountId, ImapPreset, Provider, Rgb, Security, Server};

use crate::theme as t;

/// Domains served by Microsoft and Google, whose accounts use their own APIs (phases 3 and 4).
const OUTLOOK_DOMAINS: [&str; 6] = ["hotmail.com", "outlook.com", "live.ca", "live.com", "msn.com", "hotmail.ca"];
const GMAIL_DOMAINS: [&str; 2] = ["gmail.com", "googlemail.com"];

fn domain(address: &str) -> String {
    address.rsplit_once('@').map(|(_, d)| d.trim().to_ascii_lowercase()).unwrap_or_default()
}

/// The dialog's state.
pub struct AccountForm {
    /// The account being changed, or `None` for a new one.
    editing: Option<Account>,
    name: String,
    address: String,
    username: String,
    password: String,
    imap_host: String,
    imap_port: String,
    imap_security: Security,
    smtp_host: String,
    smtp_port: String,
    smtp_security: Security,
    /// The servers were typed by the user: the address no longer changes them.
    servers_typed: bool,
    show_servers: bool,
    checking: Option<Receiver<Result<Checked, String>>>,
    error: Option<String>,
    /// The account checked, waiting for the sign-in's answer.
    candidate: Option<Account>,
}

/// What the dialog ended with.
pub enum FormResult {
    Open,
    Cancelled,
    /// The account to keep, and its new secret if any (to put in the keychain).
    Done(Account, Option<Secret>),
}

/// What goes into the keychain for an account.
pub enum Secret {
    /// An IMAP account's password.
    Password(String),
    /// An Outlook account's Microsoft sign-in (refresh token).
    RefreshToken(String),
}

/// What a successful sign-in check gave.
enum Checked {
    Imap,
    Microsoft(MicrosoftAccount),
}

impl AccountForm {
    pub fn new() -> Self {
        Self {
            editing: None,
            name: String::new(),
            address: String::new(),
            username: String::new(),
            password: String::new(),
            imap_host: String::new(),
            imap_port: "993".to_owned(),
            imap_security: Security::Tls,
            smtp_host: String::new(),
            smtp_port: "465".to_owned(),
            smtp_security: Security::Tls,
            servers_typed: false,
            show_servers: false,
            checking: None,
            error: None,
            candidate: None,
        }
    }

    pub fn edit(account: &Account) -> Self {
        let mut f = Self::new();
        f.name = account.name.clone();
        f.address = account.address.clone();
        f.username = account.username.clone();
        if let Some(s) = &account.imap {
            f.imap_host = s.host.clone();
            f.imap_port = s.port.to_string();
            f.imap_security = s.security;
        }
        if let Some(s) = &account.smtp {
            f.smtp_host = s.host.clone();
            f.smtp_port = s.port.to_string();
            f.smtp_security = s.security;
        }
        f.servers_typed = true;
        f.show_servers = true;
        f.editing = Some(account.clone());
        f
    }

    /// Fills the servers from the preset the address calls for, until the user types their own.
    fn follow_address(&mut self) {
        if self.servers_typed || self.editing.is_some() {
            return;
        }
        if let Some((imap, smtp)) = ImapPreset::for_address(&self.address).servers() {
            self.imap_host = imap.host;
            self.imap_port = imap.port.to_string();
            self.imap_security = imap.security;
            self.smtp_host = smtp.host;
            self.smtp_port = smtp.port.to_string();
            self.smtp_security = smtp.security;
        }
    }

    /// A Hotmail or Outlook.com account: it signs in on Microsoft's page, not with a password.
    fn is_outlook(&self) -> bool {
        match &self.editing {
            Some(a) => a.provider == Provider::Outlook,
            None => OUTLOOK_DOMAINS.contains(&domain(&self.address).as_str()),
        }
    }

    /// What the address's domain allows in this phase.
    fn unsupported(&self) -> Option<&'static str> {
        let d = domain(&self.address);
        if self.is_outlook() && !mailalm_engine::microsoft_available() {
            Some("This copy of MailALM was built without Microsoft sign-in: Hotmail and Outlook.com accounts cannot be added.")
        } else if GMAIL_DOMAINS.contains(&d.as_str()) {
            Some("Gmail accounts connect through Google's API (phase 4).")
        } else {
            None
        }
    }

    /// The account the form describes, or what is wrong with it.
    fn account(&self, id: AccountId, color: Rgb) -> Result<Account, String> {
        let address = self.address.trim().to_owned();
        if !mailalm_mime::address_is_valid(&address) {
            return Err("Enter the account's e-mail address.".to_owned());
        }
        if let Some(why) = self.unsupported() {
            return Err(why.to_owned());
        }
        let port = |s: &str, what: &str| s.trim().parse::<u16>().ok().filter(|p| *p != 0).ok_or(format!("The {what} port is not a number between 1 and 65535."));
        let imap = Server::new(self.imap_host.trim(), port(&self.imap_port, "IMAP")?, self.imap_security);
        let smtp = Server::new(self.smtp_host.trim(), port(&self.smtp_port, "SMTP")?, self.smtp_security);
        if !imap.host_is_valid() {
            return Err("The IMAP server's name is not valid (a name like imap.example.com).".to_owned());
        }
        if !smtp.host_is_valid() {
            return Err("The SMTP server's name is not valid (a name like smtp.example.com).".to_owned());
        }
        let preset = ImapPreset::for_address(&address);
        let preset = match preset.servers() {
            Some((pi, ps)) if pi == imap && ps == smtp => preset,
            _ => ImapPreset::Custom,
        };
        let name = if self.name.trim().is_empty() { Provider::Imap(preset).label().to_owned() } else { self.name.trim().to_owned() };
        Ok(Account {
            id,
            name,
            address,
            provider: Provider::Imap(preset),
            color,
            username: self.username.trim().to_owned(),
            imap: Some(imap),
            smtp: Some(smtp),
        })
    }

    /// Shows the dialog. `new_id` and `color` are used for a new account.
    pub fn show(&mut self, ctx: &egui::Context, new_id: AccountId, color: Rgb) -> FormResult {
        // The sign-in's answer.
        if let Some(rx) = &self.checking
            && let Ok(result) = rx.try_recv()
        {
            self.checking = None;
            match result {
                Ok(Checked::Imap) => {
                    if let Some(account) = self.candidate.take() {
                        let password = (!self.password.is_empty()).then(|| Secret::Password(std::mem::take(&mut self.password)));
                        return FormResult::Done(account, password);
                    }
                }
                Ok(Checked::Microsoft(m)) => match self.outlook_account(&m, new_id, color) {
                    Ok(account) => return FormResult::Done(account, Some(Secret::RefreshToken(m.refresh_token))),
                    Err(e) => self.error = Some(e),
                },
                Err(e) => self.error = Some(e),
            }
        }

        let mut result = FormResult::Open;
        let busy = self.checking.is_some();
        let outlook = self.is_outlook();
        let title = if self.editing.is_some() { "Account settings" } else { "Add an account" };
        let modal = Modal::new(Id::unique("mailalm-account-form")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(title);
            ui.add_space(4.0);
            let intro = if outlook {
                "Hotmail or Outlook.com. MailALM opens Microsoft's sign-in page in your browser: the password is typed there, never in MailALM."
            } else {
                "Vidéotron, Bell, Hotmail, Outlook.com or any IMAP account. MailALM signs in once to check the settings before keeping them."
            };
            ui.label(RichText::new(intro).size(12.0).color(t::MUTED));
            ui.add_space(10.0);
            ui.add_enabled_ui(!busy, |ui| self.fields(ui));
            if let Some(why) = self.unsupported() {
                ui.add_space(6.0);
                ui.label(RichText::new(why).color(t::WARN));
            }
            if let Some(e) = &self.error {
                ui.add_space(6.0);
                ui.label(RichText::new(e).color(t::DANGER));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if busy {
                    ui.spinner();
                    let waiting = if outlook { "Waiting for the sign-in in your browser…" } else { "Signing in…" };
                    ui.label(RichText::new(waiting).color(t::MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if outlook {
                        let label = if self.editing.is_some() { "Sign in again" } else { "Sign in with Microsoft" };
                        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                            self.submit_outlook(ctx);
                        }
                        if self.editing.is_some() && ui.add_enabled(!busy, egui::Button::new("Save")).clicked() {
                            self.save_outlook(&mut result);
                        }
                    } else {
                        let label = if self.editing.is_some() { "Save" } else { "Sign in and add" };
                        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                            self.submit(ctx, new_id, color, &mut result);
                        }
                    }
                    // A sign-in in the browser can be left: its answer is then ignored.
                    if ui.add_enabled(!busy || outlook, egui::Button::new("Cancel")).clicked() {
                        result = FormResult::Cancelled;
                    }
                });
            });
        });
        if modal.should_close() && (!busy || outlook) && matches!(result, FormResult::Open) {
            result = FormResult::Cancelled;
        }
        result
    }

    fn fields(&mut self, ui: &mut Ui) {
        let outlook = self.is_outlook();
        Grid::new("mailalm-account-fields").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("E-mail address");
            let addr = ui.add_enabled(self.editing.is_none(), TextEdit::singleline(&mut self.address).hint_text("you@videotron.ca").desired_width(320.0));
            if addr.changed() {
                self.follow_address();
            }
            ui.end_row();
            if !outlook {
                ui.label("Password");
                let hint = if self.editing.is_some() { "Unchanged" } else { "" };
                ui.add(TextEdit::singleline(&mut self.password).password(true).hint_text(hint).desired_width(320.0));
                ui.end_row();
            }
            ui.label("Name in MailALM");
            ui.add(TextEdit::singleline(&mut self.name).hint_text("Vidéotron, Bell, Outlook, Work…").desired_width(320.0));
            ui.end_row();
        });
        if outlook {
            return;
        }
        ui.add_space(6.0);
        let label = if self.show_servers { "Hide the servers" } else { "Servers…" };
        if ui.small_button(label).clicked() {
            self.show_servers = !self.show_servers;
        }
        if self.show_servers {
            ui.add_space(6.0);
            let mut typed = false;
            Grid::new("mailalm-account-servers").num_columns(4).spacing([8.0, 8.0]).show(ui, |ui| {
                ui.label("User name");
                typed |= ui.add(TextEdit::singleline(&mut self.username).hint_text("The address").desired_width(220.0)).changed();
                ui.end_row();
                ui.label("Incoming (IMAP)");
                typed |= ui.add(TextEdit::singleline(&mut self.imap_host).hint_text("imap.example.com").desired_width(220.0)).changed();
                typed |= ui.add(TextEdit::singleline(&mut self.imap_port).desired_width(48.0)).changed();
                typed |= security_combo(ui, "imap-security", &mut self.imap_security);
                ui.end_row();
                ui.label("Outgoing (SMTP)");
                typed |= ui.add(TextEdit::singleline(&mut self.smtp_host).hint_text("smtp.example.com").desired_width(220.0)).changed();
                typed |= ui.add(TextEdit::singleline(&mut self.smtp_port).desired_width(48.0)).changed();
                typed |= security_combo(ui, "smtp-security", &mut self.smtp_security);
                ui.end_row();
            });
            if typed {
                self.servers_typed = true;
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("Every connection is encrypted: TLS, or STARTTLS which MailALM requires. There is no unencrypted option.")
                    .size(11.5)
                    .color(t::FAINT),
            );
        }
    }

    /// The Outlook account a Microsoft sign-in gives. Changing an account, the same account must
    /// have signed in (another one picked on Microsoft's page is refused).
    fn outlook_account(&self, m: &MicrosoftAccount, new_id: AccountId, color: Rgb) -> Result<Account, String> {
        let name = self.name.trim();
        match &self.editing {
            Some(a) => {
                if !a.address.eq_ignore_ascii_case(&m.address) {
                    return Err(format!("You signed in to Microsoft as {}, not as {}. Sign in again with {}.", m.address, a.address, a.address));
                }
                let mut a = a.clone();
                if !name.is_empty() {
                    a.name = name.to_owned();
                }
                Ok(a)
            }
            None => Ok(Account {
                id: new_id,
                name: if name.is_empty() { Provider::Outlook.label().to_owned() } else { name.to_owned() },
                address: m.address.clone(),
                provider: Provider::Outlook,
                color,
                username: String::new(),
                imap: None,
                smtp: None,
            }),
        }
    }

    /// Opens Microsoft's sign-in page in the browser and waits for it on a thread.
    fn submit_outlook(&mut self, ctx: &egui::Context) {
        self.error = None;
        if let Some(why) = self.unsupported() {
            self.error = Some(why.to_owned());
            return;
        }
        let hint = self.address.trim().to_owned();
        if !hint.is_empty() && !mailalm_mime::address_is_valid(&hint) {
            self.error = Some("Enter the account's e-mail address.".to_owned());
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("mailalm-microsoft-sign-in".to_owned()).spawn(move || {
            let mut open = |url: &str| open_in_browser(url);
            let _ = tx.send(mailalm_engine::microsoft_sign_in(&hint, &mut open).map(Checked::Microsoft));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.error = Some("The sign-in could not be started.".to_owned());
            return;
        }
        self.checking = Some(rx);
    }

    /// Keeps an Outlook account's new name, without signing in again.
    fn save_outlook(&mut self, result: &mut FormResult) {
        if let Some(a) = &self.editing {
            let mut a = a.clone();
            let name = self.name.trim();
            if !name.is_empty() {
                a.name = name.to_owned();
            }
            *result = FormResult::Done(a, None);
        }
    }

    fn submit(&mut self, ctx: &egui::Context, new_id: AccountId, color: Rgb, result: &mut FormResult) {
        self.error = None;
        let (id, color) = match &self.editing {
            Some(a) => (a.id, a.color),
            None => (new_id, color),
        };
        let account = match self.account(id, color) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        if self.password.is_empty() {
            if self.editing.is_some() {
                // Same password: the servers are checked at the next sync.
                *result = FormResult::Done(account, None);
            } else {
                self.error = Some("Enter the account's password.".to_owned());
            }
            return;
        }
        let (tx, rx) = mpsc::channel();
        let check = account.clone();
        let password = self.password.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("mailalm-sign-in-check".to_owned()).spawn(move || {
            let _ = tx.send(mailalm_engine::test_sign_in(&check, &password).map(|()| Checked::Imap));
            ctx.request_repaint();
        });
        if spawned.is_err() {
            self.error = Some("The sign-in could not be started.".to_owned());
            return;
        }
        self.candidate = Some(account);
        self.checking = Some(rx);
    }
}

fn security_combo(ui: &mut Ui, id: &str, value: &mut Security) -> bool {
    let before = *value;
    ComboBox::from_id_salt(id)
        .selected_text(match value {
            Security::Tls => "TLS",
            Security::StartTls => "STARTTLS",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(value, Security::Tls, "TLS");
            ui.selectable_value(value, Security::StartTls, "STARTTLS");
        });
    before != *value
}

/// Shows a web page in the default browser (Windows' URL handler). Only the sign-in pages MailALM
/// builds itself come here: `https://` addresses alone.
fn open_in_browser(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("not a secure web address".to_owned());
    }
    std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .spawn()
        .map(drop)
        .map_err(|e| format!("the browser could not be opened: {e}"))
}
