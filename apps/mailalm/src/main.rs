//! MailALM desktop app.
//!
//! Phase 2: IMAP accounts (Vidéotron, Bell, others): reading, sending, folders, the local cache,
//! search in the cache, and changes made offline.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod accounts;
mod compose;
mod icons;
mod mailbox;
mod settings;
mod theme;
mod time;
mod ui;
mod window;

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
