//! The main window, as the validated mockups draw it: title bar, command bar, folders (accounts
//! in an accordion), message list, reading pane and status bar.

use egui::{
    Align, Align2, Button, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Painter, Pos2, Rect, Response, RichText, ScrollArea,
    Sense, Stroke, TextEdit, Ui, Vec2, ViewportCommand, pos2, vec2,
};
use mailalm_model::{AccountId, Focus, NewMail, SearchScope, Workspace};

use crate::icons::{self, Icon};
use crate::sample::{self, Group, Message};
use crate::theme as t;
use crate::window;

pub struct MailApp {
    ws: Workspace,
    messages: Vec<Message>,
    selected: Option<usize>,
    /// Where the "New mail" menu opens, while it is open.
    create_menu: Option<Pos2>,
    /// The menu opened during this frame: the click that opened it must not close it.
    create_menu_just_opened: bool,
    search: String,
    /// The last thing MailALM has to say, in the status bar.
    notice: Option<String>,
}

impl MailApp {
    pub fn new(ws: Workspace, messages: Vec<Message>) -> Self {
        let mut app = Self {
            ws,
            messages,
            selected: None,
            create_menu: None,
            create_menu_just_opened: false,
            search: String::new(),
            notice: None,
        };
        app.selected = app.visible().first().copied();
        app
    }

    /// The messages of the current view: the focused account's, or every account's.
    fn visible(&self) -> Vec<usize> {
        let focus = self.ws.focus();
        (0..self.messages.len())
            .filter(|&i| match focus {
                Focus::Unified => self.ws.account(self.messages[i].account).is_some(),
                Focus::Account(id) => self.messages[i].account == id,
            })
            .collect()
    }

    fn unread(&self, account: Option<AccountId>) -> usize {
        self.messages
            .iter()
            .filter(|m| m.unread && self.ws.account(m.account).is_some() && account.is_none_or(|id| m.account == id))
            .count()
    }

    fn color_of(&self, id: AccountId) -> Color32 {
        self.ws.account(id).map_or(t::FAINT, |a| t::account(a.color))
    }

    fn focus_account(&mut self, id: AccountId) {
        if self.ws.focus_account(id) {
            self.after_focus();
        }
    }

    fn focus_unified(&mut self) {
        if self.ws.focus_unified() {
            self.after_focus();
        }
    }

    fn after_focus(&mut self) {
        let visible = self.visible();
        if !self.selected.is_some_and(|s| visible.contains(&s)) {
            self.selected = visible.first().copied();
        }
        self.create_menu = None;
    }

    fn say(&mut self, text: impl Into<String>) {
        self.notice = Some(text.into());
    }

    fn view_name(&self) -> String {
        match self.ws.focused_account() {
            Some(a) => format!("{} · Inbox", a.name),
            None => "Unified inbox".to_owned(),
        }
    }

    fn scope_label(&self) -> String {
        match self.ws.default_search_scope() {
            SearchScope::AllAccounts => "All accounts".to_owned(),
            SearchScope::Accounts(ids) => ids.iter().filter_map(|&id| self.ws.account(id)).map(|a| a.name.clone()).collect::<Vec<_>>().join(", "),
        }
    }

    fn open_create_menu(&mut self, at: Pos2) {
        self.create_menu = Some(at);
        self.create_menu_just_opened = true;
    }
}

impl eframe::App for MailApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.title_bar(ui);
        self.command_bar(ui);
        self.status_bar(ui);
        self.folder_pane(ui);
        self.message_list(ui);
        self.reading_pane(ui);
        self.create_menu(ui);
        window::resize_edges(ui);
    }
}

// ---- title bar --------------------------------------------------------------------------------

impl MailApp {
    fn title_bar(&mut self, ui: &mut Ui) {
        let title = format!("{} · {} unread", self.view_name(), self.unread(self.ws.focused_account().map(|a| a.id)));
        egui::Panel::top("title-bar")
            .exact_size(34.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(Frame::new().fill(t::TITLE))
            .show(ui, |ui| {
                let bar = ui.max_rect();
                window::title_bar_drag(ui, bar);
                ui.horizontal_centered(|ui| {
                    ui.add_space(12.0);
                    let (logo, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    ui.painter().rect_filled(logo, 6.0, Color32::WHITE);
                    ui.painter().text(logo.center(), Align2::CENTER_CENTER, "M", FontId::proportional(13.0), t::TITLE);
                    ui.add(egui::Label::new(RichText::new("MailALM").color(t::TITLE_TEXT).strong().size(14.0)).selectable(false));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let size = vec2(44.0, 34.0);
                        let hover = Color32::from_rgba_unmultiplied(255, 255, 255, 40);
                        let ctx = ui.ctx().clone();
                        if icons::button(ui, Icon::Close, size, t::TITLE_TEXT, Color32::from_rgb(0xc4, 0x2b, 0x1c), "Close").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                        if icons::button(ui, Icon::Maximize, size, t::TITLE_TEXT, hover, "Maximize or restore").clicked() {
                            window::toggle_maximized(&ctx);
                        }
                        if icons::button(ui, Icon::Minimize, size, t::TITLE_TEXT, hover, "Minimize").clicked() {
                            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                        }
                        ui.add_space(6.0);
                        let small = vec2(32.0, 28.0);
                        if icons::button(ui, Icon::Settings, small, t::TITLE_TEXT, hover, "Settings").clicked() {
                            self.say("Settings arrive with the first real accounts (phase 2).");
                        }
                        if icons::button(ui, Icon::Refresh, small, t::TITLE_TEXT, hover, "Send and receive all").clicked() {
                            self.say("Sample mailbox: there is nothing to send or receive.");
                        }
                        ui.centered_and_justified(|ui| {
                            ui.add(egui::Label::new(RichText::new(title).color(t::TITLE_TEXT).size(12.0)).selectable(false).truncate());
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
    ui.painter().rect_filled(r, 0.0, t::LINE);
    ui.add_space(4.0);
}

fn flat(ui: &mut Ui, label: &str) -> Response {
    ui.add(Button::new(RichText::new(label).color(t::TEXT)).frame_when_inactive(false))
}

impl MailApp {
    fn command_bar(&mut self, ui: &mut Ui) {
        egui::Panel::top("command-bar")
            .resizable(false)
            .frame(Frame::new().fill(t::PANEL).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.new_mail_button(ui);
                    separator(ui);
                    for label in ["Reply", "Reply all", "Forward"] {
                        if flat(ui, label).clicked() {
                            self.say(format!("{label}: the compose window arrives in phase 7."));
                        }
                    }
                    separator(ui);
                    for label in ["Archive", "Delete"] {
                        if flat(ui, label).clicked() {
                            self.say(format!("{label}: sample mailbox, nothing was changed."));
                        }
                    }
                    let mut junk: Option<&str> = None;
                    let _ = ui.menu_button("Junk ▾", |ui| {
                        for item in ["Report junk", "Block sender", "Unsubscribe", "Not junk"] {
                            if ui.button(item).clicked() {
                                junk = Some(item);
                                ui.close();
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Outlook and Gmail: done on the account's server.").size(11.0).color(t::OK));
                    });
                    if let Some(item) = junk {
                        self.say(format!("{item}: arrives with real accounts (phases 3 and 4)."));
                    }
                    separator(ui);
                    for label in ["Move ▾", "Snooze ▾", "Flag ▾", "More ▾"] {
                        let _ = ui.menu_button(label, |ui| {
                            ui.label(RichText::new("Arrives with real accounts.").color(t::FAINT));
                        });
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let scope = self.scope_label();
                        ui.add(TextEdit::singleline(&mut self.search).hint_text("Search — from:, subject:, has:attachment").desired_width(260.0));
                        ui.label(RichText::new(scope).size(12.0).color(t::MUTED))
                            .on_hover_text("Search scope: the focused account, or all accounts in the unified inbox.");
                    });
                });
            });
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
                let fill = |r: &Response| if r.hovered() { t::ACCENT_TEXT } else { t::ACCENT };
                p.rect_filled(main_rect, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, fill(&main));
                p.rect_filled(arrow_rect, CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, fill(&arrow));
                p.line_segment(
                    [arrow_rect.left_top() + vec2(0.0, 6.0), arrow_rect.left_bottom() - vec2(0.0, 6.0)],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(0, 0, 0, 70)),
                );
                icons::paint(p, Rect::from_center_size(pos2(main_rect.left() + 16.0, main_rect.center().y), Vec2::splat(14.0)), Icon::Plus, t::ON_ACCENT);
                let text_x = main_rect.left() + 30.0;
                match &sender {
                    Some(name) => {
                        p.text(pos2(text_x, main_rect.center().y - 7.0), Align2::LEFT_CENTER, "New mail", FontId::proportional(13.5), t::ON_ACCENT);
                        p.text(pos2(text_x, main_rect.center().y + 8.0), Align2::LEFT_CENTER, name, FontId::proportional(11.0), t::ON_ACCENT);
                    }
                    None => {
                        p.text(pos2(text_x, main_rect.center().y), Align2::LEFT_CENTER, "New mail", FontId::proportional(13.5), t::ON_ACCENT);
                    }
                }
                icons::paint(p, arrow_rect, Icon::ChevronDown, t::ON_ACCENT);
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
                NewMail::From(_) => {
                    let name = sender.unwrap_or_default();
                    self.say(format!("New mail from {name}: the compose window arrives in phase 7."));
                }
                NewMail::ChooseAccount => self.open_create_menu(main.rect.left_bottom()),
                NewMail::NoAccount => self.say("Add an account first."),
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
        let mut chosen: Option<String> = None;
        let area = egui::Area::new(Id::unique("mailalm-create-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor + vec2(0.0, 4.0))
            .show(&ctx, |ui| {
                Frame::new()
                    .fill(t::RAISED)
                    .stroke(Stroke::new(1.0, t::LINE))
                    .corner_radius(10)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(330.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let (heading, color) = match focused {
                            Some(_) => ("NEW MAIL FROM", t::FAINT),
                            None => ("CHOOSE THE ACCOUNT TO SEND FROM", t::ACCENT_TEXT),
                        };
                        ui.add_space(4.0);
                        ui.label(RichText::new(heading).size(11.0).strong().color(color));
                        ui.add_space(4.0);
                        for (id, name, address, dot) in &accounts {
                            let current = focused == Some(*id);
                            let right = if current { "current ✓" } else { "" };
                            if menu_row(ui, Some(*dot), name, address, right, current).clicked() {
                                chosen = Some(format!("New mail from {name}: the compose window arrives in phase 7."));
                            }
                        }
                        if menu_row(ui, None, "New mail from a template…", "", "", false).clicked() {
                            chosen = Some("Templates arrive in phase 7.".to_owned());
                        }
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(4.0);
                        for (label, keys, phase) in [
                            ("New event", "Ctrl+Shift+E", "6"),
                            ("New meeting", "", "6"),
                            ("New task", "Ctrl+Shift+K", "6"),
                            ("New contact", "Ctrl+Shift+C", "8"),
                        ] {
                            if menu_row(ui, None, label, "", keys, false).clicked() {
                                chosen = Some(format!("{label}: arrives in phase {phase}."));
                            }
                        }
                    });
            });
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(text) = chosen {
            self.say(text);
            self.create_menu = None;
        } else if escape || (!self.create_menu_just_opened && area.response.clicked_elsewhere()) {
            self.create_menu = None;
        }
        self.create_menu_just_opened = false;
    }
}

/// One line of a menu: an optional colour dot, a label, a detail in grey, and a note on the right.
fn menu_row(ui: &mut Ui, dot: Option<Color32>, label: &str, detail: &str, right: &str, current: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let p = ui.painter_at(rect);
    if current {
        p.rect_filled(rect, 6.0, t::SEL);
    } else if response.hovered() {
        p.rect_filled(rect, 6.0, t::PANEL2);
    }
    let mut x = rect.left() + 10.0;
    if let Some(c) = dot {
        p.circle_filled(pos2(x + 4.0, rect.center().y), 4.5, c);
        x += 16.0;
    }
    let label_rect = p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), t::TEXT);
    if !detail.is_empty() {
        p.text(pos2(label_rect.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, detail, FontId::proportional(12.0), t::FAINT);
    }
    if !right.is_empty() {
        p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, right, FontId::proportional(11.5), t::ACCENT_TEXT);
    }
    response
}

// ---- folder pane ------------------------------------------------------------------------------

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(11.0).strong().color(t::FAINT));
    ui.add_space(2.0);
}

/// A folder line: `indent` from the left, selected or not, with its count on the right.
fn folder_row(ui: &mut Ui, indent: f32, selected: bool, label: &str, count: usize) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    let p = ui.painter_at(rect);
    if selected {
        p.rect_filled(rect, 6.0, t::SEL);
    } else if response.hovered() {
        p.rect_filled(rect, 6.0, t::RAISED);
    }
    p.text(pos2(rect.left() + indent, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), t::TEXT);
    if count > 0 {
        p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, count.to_string(), FontId::proportional(12.5), t::ACCENT_TEXT);
    }
    response
}

impl MailApp {
    fn folder_pane(&mut self, ui: &mut Ui) {
        egui::Panel::left("folders")
            .exact_size(264.0)
            .resizable(false)
            .frame(Frame::new().fill(t::PANEL).inner_margin(Margin::symmetric(8, 8)))
            .show(ui, |ui| {
                egui::Panel::bottom("modules").resizable(false).frame(Frame::new().inner_margin(Margin::symmetric(0, 6))).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (label, active) in [("Mail", true), ("Calendar", false), ("Contacts", false), ("Tasks", false)] {
                            if ui.add(Button::selectable(active, label)).clicked() && !active {
                                self.say(format!("{label} arrives in a later phase."));
                            }
                        }
                    });
                });
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if self.ws.unified_available() {
                        section(ui, "FAVORITES");
                        let unified = self.ws.focus() == Focus::Unified;
                        if folder_row(ui, 10.0, unified, "Unified inbox", self.unread(None)).clicked() {
                            self.focus_unified();
                        }
                        if folder_row(ui, 10.0, false, "Flagged, all accounts", 0).clicked() {
                            self.say("Flagged messages arrive with real accounts.");
                        }
                    }
                    let ids: Vec<AccountId> = self.ws.accounts().iter().map(|a| a.id).collect();
                    for id in ids {
                        self.account_block(ui, id);
                    }
                });
            });
    }

    /// An account: its header (click: focus, expand, collapse the others), then its folders when
    /// it has the focus.
    fn account_block(&mut self, ui: &mut Ui, id: AccountId) {
        let Some(account) = self.ws.account(id) else { return };
        let (name, address, provider) = (account.name.clone(), account.address.clone(), account.provider);
        let color = t::account(account.color);
        let expanded = self.ws.is_expanded(id);
        let unread = self.unread(Some(id));
        ui.add_space(6.0);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
        let p = ui.painter_at(rect);
        if expanded {
            p.rect_filled(rect, 4.0, t::PANEL2);
        } else if response.hovered() {
            p.rect_filled(rect, 4.0, t::RAISED);
        }
        p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 1.0, color);
        let chevron = if expanded { Icon::ChevronDown } else { Icon::ChevronRight };
        icons::paint(&p, Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(12.0)), chevron, t::MUTED);
        p.text(pos2(rect.left() + 28.0, rect.top() + 13.0), Align2::LEFT_CENTER, &name, FontId::proportional(13.5), t::TEXT);
        p.text(pos2(rect.left() + 28.0, rect.top() + 29.0), Align2::LEFT_CENTER, &address, FontId::proportional(11.0), t::FAINT);
        if unread > 0 {
            p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, unread.to_string(), FontId::proportional(13.0), t::ACCENT_TEXT);
        }
        let response = response.on_hover_text(if expanded { "This account has the focus" } else { "Give this account the focus" });
        if response.clicked() {
            self.focus_account(id);
        }
        if expanded {
            for (n, (folder, count)) in sample::folders(provider).iter().enumerate() {
                if folder_row(ui, 30.0, n == 0, folder, *count as usize).clicked() && n > 0 {
                    self.say(format!("{folder}: folders other than the inbox arrive with real accounts."));
                }
            }
        }
    }
}

// ---- message list -----------------------------------------------------------------------------

impl MailApp {
    fn message_list(&mut self, ui: &mut Ui) {
        let visible = self.visible();
        let (title, subtitle, swatch) = match self.ws.focused_account() {
            Some(a) => (format!("{} · Inbox", a.name), a.address.clone(), t::account(a.color)),
            None => ("Unified inbox".to_owned(), format!("{} accounts", self.ws.accounts().len()), t::ACCENT),
        };
        let mark_accounts = self.ws.unified_available();
        egui::Panel::left("messages")
            .exact_size(400.0)
            .resizable(false)
            .frame(Frame::new().fill(t::PANEL2))
            .show(ui, |ui| {
                Frame::new().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().rect_filled(r, 3.0, swatch);
                        ui.label(RichText::new(title).size(17.0).strong());
                        ui.label(RichText::new(subtitle).size(12.0).color(t::FAINT));
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let _ = ui.add(Button::selectable(true, "Focused"));
                        if ui.add(Button::selectable(false, "Other")).clicked() {
                            self.say("Focused and Other follow Outlook and Gmail once accounts are connected.");
                        }
                    });
                });
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for group in Group::ALL {
                        let items: Vec<usize> = visible.iter().copied().filter(|&i| self.messages[i].group == group).collect();
                        if items.is_empty() {
                            continue;
                        }
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                        ui.painter().text(pos2(r.left() + 14.0, r.center().y), Align2::LEFT_CENTER, group.label(), FontId::proportional(11.0), t::FAINT);
                        for i in items {
                            if self.message_row(ui, i, mark_accounts).clicked() {
                                self.selected = Some(i);
                            }
                        }
                    }
                });
            });
    }

    fn message_row(&self, ui: &mut Ui, i: usize, mark_account: bool) -> Response {
        let m = &self.messages[i];
        let height = if m.chip.is_some() { 84.0 } else { 64.0 };
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let p = ui.painter_at(rect);
        let selected = self.selected == Some(i);
        if selected {
            p.rect_filled(rect, 0.0, t::SEL);
        } else if response.hovered() {
            p.rect_filled(rect, 0.0, t::RAISED);
        }
        if mark_account {
            p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 0.0, self.color_of(m.account));
        }
        p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t::LINE));
        let avatar = pos2(rect.left() + 27.0, rect.top() + 24.0);
        p.circle_filled(avatar, 16.0, m.avatar);
        p.text(avatar, Align2::CENTER_CENTER, m.initials, FontId::proportional(12.0), Color32::WHITE);

        let x = rect.left() + 52.0;
        let accent = if m.unread { t::ACCENT_TEXT } else { t::TEXT };
        let time_rect = p.text(pos2(rect.right() - 12.0, rect.top() + 14.0), Align2::RIGHT_CENTER, m.time, FontId::proportional(12.0), if m.unread { t::ACCENT_TEXT } else { t::FAINT });
        let clip = |right: f32| -> Painter { p.with_clip_rect(Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom()))) };
        clip(time_rect.left() - 8.0).text(pos2(x, rect.top() + 14.0), Align2::LEFT_CENTER, m.from, FontId::proportional(13.5), t::TEXT);
        let mut marks = String::new();
        if !m.attachments.is_empty() {
            marks.push('📎');
        }
        if m.flagged {
            marks.push('⚑');
        }
        let marks_left = if marks.is_empty() {
            rect.right() - 12.0
        } else {
            p.text(pos2(rect.right() - 12.0, rect.top() + 32.0), Align2::RIGHT_CENTER, &marks, FontId::proportional(12.0), t::FAINT).left() - 6.0
        };
        clip(marks_left).text(pos2(x, rect.top() + 32.0), Align2::LEFT_CENTER, m.subject, FontId::proportional(13.0), accent);
        clip(rect.right() - 12.0).text(pos2(x, rect.top() + 49.0), Align2::LEFT_CENTER, m.preview, FontId::proportional(12.0), t::FAINT);
        if let Some(chip) = m.chip {
            let galley = p.layout_no_wrap(chip.to_owned(), FontId::proportional(11.0), t::MUTED);
            let chip_rect = Rect::from_min_size(pos2(x, rect.top() + 60.0), galley.size() + vec2(14.0, 6.0));
            p.rect_filled(chip_rect, 9.0, t::RAISED);
            p.galley(chip_rect.min + vec2(7.0, 3.0), galley, t::MUTED);
        }
        response
    }
}

// ---- reading pane -----------------------------------------------------------------------------

/// A rounded label on a coloured background.
fn pill(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    Frame::new().fill(bg).corner_radius(6).inner_margin(Margin::symmetric(10, 4)).show(ui, |ui| {
        ui.label(RichText::new(text).size(12.0).color(fg));
    });
}

impl MailApp {
    fn reading_pane(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().frame(Frame::new().fill(t::PANEL).inner_margin(Margin::symmetric(24, 18))).show(ui, |ui| {
            let Some(i) = self.selected else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Select a message").color(t::FAINT));
                });
                return;
            };
            let mut said: Option<String> = None;
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let m = &self.messages[i];
                let account = self.ws.account(m.account);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(m.subject).size(20.0).strong());
                    if m.flagged {
                        ui.label(RichText::new("⚑").size(16.0).color(t::DANGER));
                    }
                    if let Some(a) = account {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            Frame::new().fill(t::RAISED).stroke(Stroke::new(1.0, t::LINE)).corner_radius(12).inner_margin(Margin::symmetric(10, 3)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 4.0, t::account(a.color));
                                    ui.label(RichText::new(format!("{} · {}", a.name, a.address)).size(12.0));
                                });
                            });
                        });
                    }
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 20.0, m.avatar);
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, m.initials, FontId::proportional(14.0), Color32::WHITE);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(m.from).strong());
                            ui.label(RichText::new(format!("<{}>", m.address)).color(t::MUTED));
                        });
                        ui.label(RichText::new(format!("{} · {}", m.time, "To: Alex Morin")).size(12.0).color(t::MUTED));
                    });
                });
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    pill(ui, "Sample message · sender checks run on real mail", t::MUTED, t::RAISED);
                    pill(ui, "Remote pictures blocked", t::MUTED, t::RAISED);
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for label in ["Reply", "Reply all", "Forward", "Snooze", "Block sender"] {
                        if ui.button(label).clicked() {
                            said = Some(format!("{label}: arrives with real accounts."));
                        }
                    }
                });
                ui.add_space(12.0);
                ui.separator();
                if !m.attachments.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                        icons::paint(ui.painter(), r, Icon::Lock, t::WARN);
                        ui.label(
                            RichText::new(format!("{} attachment{} · locked until you review each one", m.attachments.len(), if m.attachments.len() > 1 { "s" } else { "" }))
                                .size(12.0)
                                .color(t::MUTED),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for a in &m.attachments {
                            Frame::new().fill(t::PANEL2).stroke(Stroke::new(1.0, t::LINE)).corner_radius(8).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                                    ui.painter().rect_filled(r, 6.0, t::WARN_BG);
                                    icons::paint(ui.painter(), r, Icon::Lock, t::WARN);
                                    ui.vertical(|ui| {
                                        ui.label(RichText::new(a.name).strong());
                                        ui.label(RichText::new(format!("{} · {}", a.kind, a.size)).size(12.0).color(t::FAINT));
                                    });
                                    if ui.button("Review…").clicked() {
                                        said = Some(format!("{}: attachment review arrives in phase 5. Nothing was opened, decoded or previewed.", a.name));
                                    }
                                });
                            });
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                }
                ui.add_space(12.0);
                ui.label(RichText::new(m.body).size(14.0));
            });
            if let Some(text) = said {
                self.say(text);
            }
        });
    }
}

// ---- status bar -------------------------------------------------------------------------------

impl MailApp {
    fn status_bar(&mut self, ui: &mut Ui) {
        let visible = self.visible();
        let unread = visible.iter().filter(|&&i| self.messages[i].unread).count();
        let notice = self.notice.clone();
        let accounts: Vec<(String, Color32)> = self.ws.accounts().iter().map(|a| (a.name.clone(), t::account(a.color))).collect();
        egui::Panel::bottom("status-bar")
            .exact_size(26.0)
            .resizable(false)
            .frame(Frame::new().fill(t::PANEL).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(format!("Items: {} · Unread: {unread}", visible.len())).size(12.0).color(t::FAINT));
                    if let Some(text) = notice {
                        ui.add_space(12.0);
                        ui.label(RichText::new(text).size(12.0).color(t::ACCENT_TEXT));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new("Sample mailbox · no account connected").size(12.0).color(t::FAINT));
                        for (name, color) in accounts.iter().rev() {
                            ui.add_space(10.0);
                            ui.label(RichText::new(name).size(12.0).color(t::FAINT));
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(7.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, *color);
                        }
                    });
                });
            });
    }
}
