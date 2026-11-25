//! The contacts page (K1): every account's contacts (Outlook's and Google's, read here), searched
//! by name, address, company or phone; a click on an address writes to it from that account. An
//! IMAP account has no address book of its own yet: the people seen in its mail are offered in
//! the recipient fields.

use egui::{Button, Frame, Margin, RichText, ScrollArea, TextEdit, Ui};
use mailalm_model::{AccountId, Person};

use crate::agenda::{Ctx, Pim};
use crate::i18n::{tr, trf};
use crate::theme as t;

/// A contact line's height.
const ROW_H: f32 = 46.0;

#[derive(Default)]
pub struct ContactsView {
    search: String,
    /// The account shown; `None`: all of them.
    account: Option<AccountId>,
    /// The contacts shown (account, index), and what they were worked out from.
    shown: Vec<(AccountId, usize)>,
    shown_key: Option<(u64, String, Option<AccountId>)>,
}

impl Pim {
    /// The search, in the command bar.
    pub fn contacts_commands(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(tr("Contacts")).size(15.0).strong());
        ui.separator();
        ui.add(TextEdit::singleline(&mut self.contacts.search).desired_width(320.0).hint_text(tr("Search: a name, an address, a company, a phone")));
        if !self.contacts.search.is_empty() && ui.small_button(tr("Clear")).clicked() {
            self.contacts.search.clear();
        }
    }

    /// The accounts, in the folder pane.
    pub fn contacts_sidebar(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("ADDRESS BOOKS")).size(11.0).strong().color(t::faint()));
        let total: usize = self.data.values().map(|d| d.contacts.len()).sum();
        if ui.add(Button::selectable(self.contacts.account.is_none(), trf!("All accounts  ({count})", count = total))).clicked() {
            self.contacts.account = None;
        }
        for a in cx.ws.accounts() {
            let n = self.data.get(&a.id).map_or(0, |d| d.contacts.len());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(4.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, t::account(a.color));
                if ui.add(Button::selectable(self.contacts.account == Some(a.id), format!("{}  ({n})", a.name))).clicked() {
                    self.contacts.account = Some(a.id);
                }
            });
        }
        ui.add_space(12.0);
        ui.label(RichText::new(tr("Read from Outlook and Google; change them there. IMAP accounts: the people you wrote to are offered when you write.")).size(11.0).color(t::faint()));
    }

    /// The contacts found.
    pub fn contacts_page(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        let v = &mut self.contacts;
        let key = (self.generation, v.search.trim().to_lowercase(), v.account);
        if v.shown_key.as_ref() != Some(&key) {
            let q = &key.1;
            v.shown = Vec::new();
            for a in cx.ws.accounts().iter().filter(|a| key.2.is_none_or(|id| id == a.id)) {
                let Some(d) = self.data.get(&a.id) else { continue };
                for (i, c) in d.contacts.iter().enumerate() {
                    let hit = q.is_empty()
                        || c.display().to_lowercase().contains(q.as_str())
                        || c.emails.iter().any(|e| e.to_lowercase().contains(q.as_str()))
                        || c.organization.to_lowercase().contains(q.as_str())
                        || c.phones.iter().any(|p| p.contains(q.as_str()));
                    if hit {
                        v.shown.push((a.id, i));
                    }
                }
            }
            let data = &self.data;
            v.shown.sort_by_cached_key(|(a, i)| data.get(a).and_then(|d| d.contacts.get(*i)).map(|c| c.display().to_lowercase()).unwrap_or_default());
            v.shown_key = Some(key);
        }
        let mut write_to: Option<(AccountId, Person)> = None;
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(20, 14))).show(ui, |ui| {
            let shown = &self.contacts.shown;
            let count = if shown.len() == 1 { tr("1 contact").to_owned() } else { trf!("{count} contacts", count = shown.len()) };
            ui.label(RichText::new(count).size(12.0).color(t::faint()));
            ui.add_space(6.0);
            if shown.is_empty() {
                ui.label(RichText::new(tr("No contact here. Outlook and Gmail accounts show their contacts once synced.")).color(t::faint()));
                return;
            }
            ScrollArea::vertical().id_salt("mailalm-contacts").auto_shrink([false, false]).show_rows(ui, ROW_H, shown.len(), |ui, rows| {
                for (a, i) in shown.get(rows).unwrap_or_default() {
                    let Some(c) = self.data.get(a).and_then(|d| d.contacts.get(*i)) else { continue };
                    ui.horizontal(|ui| {
                        ui.set_height(ROW_H - 4.0);
                        let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, ROW_H - 10.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 1.0, cx.color(*a));
                        ui.vertical(|ui| {
                            let name = c.display();
                            ui.label(RichText::new(&name).strong());
                            let mut detail: Vec<String> = Vec::new();
                            if !c.organization.is_empty() {
                                detail.push(if c.job_title.is_empty() { c.organization.clone() } else { format!("{}, {}", c.job_title, c.organization) });
                            }
                            detail.extend(c.phones.iter().take(2).cloned());
                            if !detail.is_empty() {
                                ui.label(RichText::new(detail.join(" · ")).size(12.0).color(t::faint()));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for e in c.emails.iter().take(3).rev() {
                                if ui.add(Button::new(RichText::new(e).color(t::accent_text())).frame_when_inactive(false)).on_hover_text(trf!("Write to {address} from {account}", address = e, account = cx.name(*a))).clicked() {
                                    write_to = Some((*a, Person { name: c.display(), address: e.clone() }));
                                }
                            }
                        });
                    });
                }
            });
        });
        if write_to.is_some() {
            self.compose_to = write_to;
        }
    }
}
