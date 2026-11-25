//! What the main window does for the calendars, tasks and contacts (`agenda.rs` and its pages):
//! new events and tasks from the "New mail" menu and the shortcuts, the event windows, the
//! reminders, and what the agenda has to say or to write.

use egui::Ui;
use mailalm_mime::Outgoing;

use super::{MailApp, Page};
use crate::agenda::{Ctx, PimPage};
use crate::time;

impl MailApp {
    /// A new event or meeting, from the next half hour, in the focused account's calendar.
    pub(super) fn new_event(&mut self, meeting: bool) {
        let cx = Ctx { ws: &self.ws, engine: &self.engine };
        let account = self.ws.focused_account().map(|a| a.id);
        self.pim.new_event(&cx, account, crate::calendar::next_half_hour(time::now()), false, meeting);
        self.create_menu = None;
    }

    /// A new task, on the tasks page.
    pub(super) fn new_task(&mut self) {
        self.page = Page::Pim(PimPage::Tasks);
        let cx = Ctx { ws: &self.ws, engine: &self.engine };
        let account = self.ws.focused_account().map(|a| a.id);
        self.pim.new_task(&cx, account);
        self.create_menu = None;
    }

    /// The event windows, the reminders, and what the agenda has to say or to write.
    pub(super) fn pim_windows(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let cx = Ctx { ws: &self.ws, engine: &self.engine };
        self.pim.event_windows(&ctx, &cx);
        self.pim.reminders_tick(&ctx, &cx);
        for (text, problem) in std::mem::take(&mut self.pim.notices) {
            if problem {
                self.warn(text);
            } else {
                self.say(text);
            }
        }
        if let Some((account, person)) = self.pim.compose_to.take() {
            let me = self.me(account);
            self.compose(account, Outgoing { from: me, to: vec![person], ..Outgoing::default() });
        }
    }
}
