//! MailALM desktop app.
//!
//! IMAP, Outlook (Graph) and Gmail accounts: reading, sending, folders, the local cache, search in
//! the cache, changes made offline, and attachments (reviewed one by one before anything is done
//! with them; attached to messages sent). The calendars, reminders, invitations, tasks and
//! contacts of every account (phase 6). Writing in rich text with signatures, templates, quick
//! replies, drafts on the server, undo send, scheduled send and Antidote; HTML messages shown
//! formatted, their links checked and their pictures from the internet blocked (phase 7). The
//! search with its scope and its search folders, the rules, snooze and follow-ups (phase 8); the
//! MCP access with its confirmations and assisted triage (phase 9); import and export, and the
//! mail kept on this PC only (phase 10).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod accounts;
mod agenda;
mod antidote_ui;
mod attachments;
mod autocomplete;
mod calendar;
mod compose;
mod contacts;
mod event_window;
mod i18n;
mod icons;
mod invitation;
mod later;
mod links;
mod local;
mod mailbox;
mod mcp;
mod pictures;
mod preview;
mod reading;
mod reminders;
mod rule_form;
mod search;
mod send_queue;
mod settings;
mod signatures;
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
    // The theme chosen in the settings is applied by the app as it starts.
    eframe::run_native("MailALM", options, Box::new(|cc| Ok(Box::new(ui::MailApp::new(&cc.egui_ctx)))))
}
