//! Import and export (C6): Settings → Import and export, the export entries of More ▾ (a message
//! as `.eml` or PDF, the folder shown as `.mbox`), the window of the job under way (its progress
//! and Cancel), and "On this PC" in the folder pane: the mail imported, kept on this computer
//! only, which no server ever sees.

use egui::{Frame, Id, Margin, RichText, Stroke, Ui};
use mailalm_store::Store;

use super::{MailApp, View, folder_row, section};
use crate::local::{self, Job, LOCAL, LOCAL_NAME, Source};
use crate::mailbox::MessageKey;
use crate::settings::Settings;
use crate::theme as t;
use crate::time;

/// An export asked from More ▾, done once the menu is closed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Export {
    Eml,
    Pdf,
    Mbox,
}

/// The imports' and exports' state.
#[derive(Default)]
pub(in crate::ui) struct PortingState {
    job: Option<Job>,
    /// The end of the last job: its title, and what it did or why it stopped.
    last: Option<(String, Result<String, String>)>,
    /// eM Client's help is shown.
    em_client: bool,
    /// A folder of "On this PC" to delete, waiting for the user's confirmation.
    delete: Option<String>,
    /// An export chosen in More ▾.
    export: Option<Export>,
}

impl MailApp {
    /// A cache to read from: an account's, or the mail kept on this PC.
    fn store_of(&self, account: mailalm_model::AccountId) -> Result<Store, String> {
        let opened = if account == LOCAL { self.local.store() } else { Store::open(&Settings::cache_dir(), account) };
        opened.map_err(|e| format!("MailALM's cache can't be opened: {e}"))
    }

    fn start_import(&mut self, source: Source) {
        if self.porting.job.is_some() {
            self.say("An import or an export is already under way.");
            return;
        }
        match self.local.import(source, std::sync::Arc::clone(&self.wake)) {
            Ok(job) => {
                self.porting.last = None;
                self.porting.job = Some(job);
            }
            Err(e) => self.warn(e),
        }
    }

    /// The export entries of More ▾ (in its menu).
    pub(super) fn export_menu(&mut self, ui: &mut Ui) {
        let message = self.selected.is_some();
        let folder = matches!(self.view, View::Folder(..));
        if ui.add_enabled(message, egui::Button::new("Save as .eml…")).on_hover_text("The message as the server sent it").clicked() {
            self.porting.export = Some(Export::Eml);
            ui.close();
        }
        if ui.add_enabled(message, egui::Button::new("Save as PDF…")).on_hover_text("Its header and its text").clicked() {
            self.porting.export = Some(Export::Pdf);
            ui.close();
        }
        if ui.add_enabled(folder, egui::Button::new("Export this folder as .mbox…")).on_hover_text("The messages of the folder MailALM has cached").clicked() {
            self.porting.export = Some(Export::Mbox);
            ui.close();
        }
    }

    /// Asks where to save, then exports.
    fn run_export(&mut self, what: Export) {
        match what {
            Export::Eml | Export::Pdf => {
                let Some(key) = self.selected.clone() else { return };
                if let Err(e) = self.export_message(&key, what) {
                    self.warn(e);
                }
            }
            Export::Mbox => {
                let View::Folder(id, folder) = self.view.clone() else { return };
                let Some(path) = self.folder_path(id, &folder) else { return };
                if self.porting.job.is_some() {
                    self.say("An import or an export is already under way.");
                    return;
                }
                let name = self.boxes.account(id).and_then(|s| s.folder(&path)).map_or_else(|| path.clone(), |f| f.name().to_owned());
                let suggested = mailalm_porting::eml::file_name(&name, "mbox");
                let Some(to) = rfd::FileDialog::new().set_title("Export the folder as .mbox").set_file_name(suggested).add_filter("mbox", &["mbox"]).save_file() else { return };
                let result = self.store_of(id).and_then(|store| local::export_mbox(store, path, to, std::sync::Arc::clone(&self.wake)));
                match result {
                    Ok(job) => {
                        self.porting.last = None;
                        self.porting.job = Some(job);
                    }
                    Err(e) => self.warn(e),
                }
            }
        }
    }

    fn export_message(&mut self, key: &MessageKey, what: Export) -> Result<(), String> {
        let store = self.store_of(key.account)?;
        let summary = self.summary_of(key).cloned().unwrap_or_default();
        let (ext, title) = if what == Export::Pdf { ("pdf", "Save the message as PDF") } else { ("eml", "Save the message as .eml") };
        let suggested = mailalm_porting::eml::file_name(&summary.subject, ext);
        let Some(to) = rfd::FileDialog::new().set_title(title).set_file_name(suggested).add_filter(ext, &[ext]).save_file() else { return Ok(()) };
        let not_cached = "This message is not in MailALM's cache yet: open it once, then save it.";
        if what == Export::Pdf {
            let raw = store.message(&key.folder, key.uid).ok_or(not_cached)?;
            mailalm_porting::pdf::export(&raw, &time::long(summary.date), &to).map_err(|e| format!("The PDF could not be written: {e}"))?;
        } else {
            if !store.has_message(&key.folder, key.uid) {
                return Err(not_cached.to_owned());
            }
            mailalm_porting::eml::export(&store, &key.folder, key.uid, &to).map_err(|e| format!("The message could not be saved: {e}"))?;
        }
        self.say(format!("Saved as {}.", to.display()));
        Ok(())
    }

    /// The Import and export section of the settings.
    pub(super) fn porting_settings(&mut self, ui: &mut Ui) {
        ui.add_space(20.0);
        ui.label(RichText::new("IMPORT AND EXPORT").size(11.0).strong().color(t::faint()));
        ui.add_space(6.0);
        let busy = self.porting.job.is_some();
        let mut chosen: Option<Source> = None;
        let mut export = false;
        Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(8).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width().min(760.0));
            ui.label(format!(
                "Imported mail goes to \"{LOCAL_NAME}\", a folder tree kept on this computer only: nothing is sent to a server. Each file is read with the limits received mail has; damaged or oversized messages are left out and counted."
            ));
            ui.add_space(6.0);
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Import .eml files…").clicked()
                        && let Some(files) = rfd::FileDialog::new().set_title("Import .eml files").add_filter("Messages", &["eml"]).pick_files()
                    {
                        chosen = Some(Source::EmlFiles(files));
                    }
                    if ui.button("Import a folder of .eml files…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_title("Import a folder of .eml files").pick_folder()
                    {
                        chosen = Some(Source::EmlTree(dir));
                    }
                    if ui.button("Import an .mbox file…").clicked()
                        && let Some(file) = rfd::FileDialog::new().set_title("Import an .mbox file").pick_file()
                    {
                        chosen = Some(Source::MboxFile(file));
                    }
                    if ui.button("Import a Thunderbird profile or a folder of .mbox files…").on_hover_text("Thunderbird: its profile's Mail\\Local Folders or ImapMail\\<server> folder").clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_title("Import a Thunderbird mail folder").pick_folder()
                    {
                        chosen = Some(Source::MboxTree(dir));
                    }
                    if ui.button("Import an Outlook data file (.pst, .ost)…").clicked()
                        && let Some(file) = rfd::FileDialog::new().set_title("Import an Outlook data file").add_filter("Outlook data files", &["pst", "ost"]).pick_file()
                    {
                        chosen = Some(Source::Outlook(file));
                    }
                    if ui.button("From eM Client…").clicked() {
                        self.porting.em_client = !self.porting.em_client;
                    }
                });
            });
            if self.porting.em_client {
                ui.label(RichText::new(mailalm_porting::EM_CLIENT_HELP).color(t::accent_text()));
            }
            if busy {
                ui.label(RichText::new("An import or an export is under way: its window shows how far it is.").size(12.0).color(t::muted()));
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Export: a message as .eml or PDF, or the folder shown as .mbox, from More ▾ in the command bar.").size(12.0).color(t::muted()));
            if matches!(self.view, View::Folder(..)) && ui.add_enabled(!busy, egui::Button::new(format!("Export {} as .mbox…", self.view_name()))).clicked() {
                export = true;
            }
            ui.label(
                RichText::new("Mail kept on this PC is read, searched and moved between its own folders; moving it to an account's server isn't possible yet.")
                    .size(11.5)
                    .color(t::faint()),
            );
        });
        if let Some(source) = chosen {
            self.start_import(source);
        }
        if export {
            self.run_export(Export::Mbox);
        }
    }

    /// "On this PC" in the folder pane, when it holds mail (or an import is under way).
    pub(super) fn local_pane(&mut self, ui: &mut Ui) {
        let folders: Vec<(String, String, usize, u32)> =
            self.boxes.account(LOCAL).map(|s| s.folders.iter().map(|f| (f.path.clone(), f.name().to_owned(), f.depth(), f.unread)).collect()).unwrap_or_default();
        if folders.is_empty() && self.porting.job.is_none() {
            return;
        }
        section(ui, &LOCAL_NAME.to_uppercase());
        if folders.is_empty() {
            ui.label(RichText::new("  Importing…").size(12.0).color(t::faint()));
        }
        let current = match &self.view {
            View::Folder(LOCAL, Some(p)) => Some(p.clone()),
            _ => None,
        };
        let mut open: Option<String> = None;
        for (path, name, depth, unread) in folders {
            let row = folder_row(ui, 10.0 + 14.0 * depth as f32, current.as_deref() == Some(path.as_str()), &name, unread);
            if row.clicked() {
                open = Some(path.clone());
            }
            row.context_menu(|ui| {
                if ui.button("Delete this folder…").clicked() {
                    self.porting.delete = Some(path.clone());
                    ui.close();
                }
            });
        }
        if let Some(path) = open {
            self.set_view(View::Folder(LOCAL, Some(path)));
        }
    }

    /// The window of the job under way, or of its end; the folder deletion's confirmation; the
    /// export chosen in More ▾.
    pub(super) fn porting_window(&mut self, ui: &mut Ui) {
        if let Some(what) = self.porting.export.take() {
            self.run_export(what);
        }
        let ctx = ui.ctx().clone();
        let running = self.porting.job.as_ref().map(|j| (j.title.clone(), j.status()));
        if let Some((title, status)) = running {
            if let Some(end) = status.finished.clone() {
                self.porting.last = Some((title, end));
                self.porting.job = None;
                // What came in is shown at once in the folder of this PC shown.
                if let View::Folder(LOCAL, Some(p)) = &self.view {
                    self.local.send(mailalm_engine::Command::View(p.clone()));
                }
            } else {
                let mut cancel = false;
                let (fraction, text) = if status.exported.1 > 0 {
                    let (done, total) = status.exported;
                    (done as f32 / total as f32, format!("{done} of {total} messages"))
                } else {
                    let p = status.progress;
                    let fraction = if p.total > 0 { (p.done as f64 / p.total as f64) as f32 } else { 0.0 };
                    (fraction, format!("{} messages in, {} left out", p.messages, p.skipped))
                };
                let _ = egui::Window::new(title.as_str()).id(Id::unique("mailalm-porting-job")).collapsible(false).resizable(false).default_width(420.0).show(&ctx, |ui| {
                    ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).text(text));
                    ui.label(RichText::new("You can go on working meanwhile.").size(12.0).color(t::muted()));
                    if ui.button("Cancel").on_hover_text("What came in before stays; an export writes no file").clicked() {
                        cancel = true;
                    }
                });
                if cancel && let Some(job) = &self.porting.job {
                    job.cancel();
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(300));
            }
        }
        let mut close = false;
        if let Some((title, end)) = &self.porting.last {
            let _ = egui::Window::new(title.as_str()).id(Id::unique("mailalm-porting-end")).collapsible(false).resizable(false).default_width(460.0).show(&ctx, |ui| {
                match end {
                    Ok(text) => ui.label(text.as_str()),
                    Err(e) => ui.label(RichText::new(e.as_str()).color(t::danger())),
                };
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        }
        if close {
            self.porting.last = None;
        }
        if let Some(path) = self.porting.delete.clone() {
            let mut answer: Option<bool> = None;
            let m = egui::Modal::new(Id::unique("mailalm-delete-local-folder")).show(&ctx, |ui| {
                ui.set_width(440.0);
                ui.heading("Delete this folder?");
                ui.add_space(6.0);
                ui.label(format!("\"{path}\", the folders under it and their messages are removed from this PC. They can't be brought back (the files they were imported from are untouched)."));
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
                self.porting.delete = None;
                if yes {
                    if self.selected.as_ref().is_some_and(|k| k.account == LOCAL) {
                        self.selected = None;
                        self.reading = None;
                    }
                    if let Err(e) = self.local.delete_folder(path, std::sync::Arc::clone(&self.wake)) {
                        self.warn(e);
                    }
                }
            }
        }
    }
}
