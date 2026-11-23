//! MailALM desktop app.
//!
//! Phase 1: the window and its layout, drawn from the validated mockups, over a sample mailbox.
//! No account is connected and nothing touches the network.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod icons;
mod sample;
mod theme;
mod ui;
mod window;

fn main() -> eframe::Result {
    // `--one-account`: the sample mailbox with a single account (no unified view).
    let one_account = std::env::args().skip(1).any(|a| a == "--one-account");
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
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(ui::MailApp::new(sample::workspace(one_account), sample::messages())))
        }),
    )
}
