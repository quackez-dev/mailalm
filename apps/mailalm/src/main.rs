//! MailALM desktop app.
//!
//! IMAP, Outlook (Graph) and Gmail accounts: reading, sending, folders, the local cache, search in
//! the cache, changes made offline, and attachments (reviewed one by one before anything is done
//! with them; attached to messages sent). The calendars, reminders, invitations, tasks and
//! contacts of every account (phase 6).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod accounts;
mod agenda;
mod attachments;
mod autocomplete;
mod calendar;
mod compose;
mod contacts;
mod event_window;
mod icons;
mod invitation;
mod mailbox;
mod preview;
mod reminders;
mod settings;
mod tasks;
mod theme;
mod time;
mod ui;
mod window;
mod zones;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MailALM")
            .with_app_id("mailalm")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            // MailALM draws its own title bar (the design's teal bar with the window buttons).
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "MailALM",
        options,
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(ui::MailApp::new(&cc.egui_ctx)))
        }),
    )
}
