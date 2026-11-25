//! The tasks page (PLAN.md, section 7, A1): each account's lists (Microsoft To Do, Google Tasks,
//! or the list kept on this computer for an IMAP account) and their tasks: a new one added on a
//! line, done or not with a click, and a task's title, notes, due day and reminder edited beside
//! the list (Google Tasks has no reminder in its API: MailALM doesn't offer one there).

use egui::{Button, Frame, Key, Margin, RichText, ScrollArea, TextEdit, Ui};
use mailalm_engine::PimCommand;
use mailalm_model::pim::{MAX_LINE, MAX_TEXT, Task, clean_line, clean_text, parse_date};
use mailalm_model::{AccountId, Provider};

use crate::agenda::{Ctx, Pim};
use crate::theme as t;
use crate::time;

#[derive(Default)]
pub struct TasksView {
    /// The list shown: its account and id.
    selected: Option<(AccountId, String)>,
    show_done: bool,
    new_title: String,
    editing: Option<TaskEditor>,
}

/// A task being edited, beside the list.
struct TaskEditor {
    account: AccountId,
    task: Task,
    title: String,
    notes: String,
    due: String,
    reminder_date: String,
    reminder_time: String,
    error: Option<String>,
    confirm_delete: bool,
}

impl TaskEditor {
    fn new(account: AccountId, task: Task) -> TaskEditor {
        let due = task.due.map(|d| time::date(d.div_euclid(86_400))).unwrap_or_default();
        let (reminder_date, reminder_time) = match task.reminder {
            Some(r) => (time::date(time::local_day(r)), time::clock(r)),
            None => (String::new(), String::new()),
        };
        TaskEditor { account, title: task.title.clone(), notes: task.notes.clone(), due, reminder_date, reminder_time, task, error: None, confirm_delete: false }
    }

    /// The task the fields describe.
    fn build(&self, reminders: bool) -> Result<Task, String> {
        let title = clean_line(&self.title, MAX_LINE);
        if title.is_empty() {
            return Err("Give the task a title.".to_owned());
        }
        let due = if self.due.trim().is_empty() { None } else { Some(parse_date(&self.due).ok_or("The due day is not a date: write it as 2026-11-25.")? * 86_400) };
        let reminder = if !reminders || self.reminder_date.trim().is_empty() {
            None
        } else {
            let day = parse_date(&self.reminder_date).ok_or("The reminder's day is not a date: write it as 2026-11-25.")?;
            let at = if self.reminder_time.trim().is_empty() { 9 * 3600 } else { time::parse_clock(&self.reminder_time).ok_or("The reminder's time is not a time: write it as 09:00.")? };
            Some(time::utc_of_local(day * 86_400 + at))
        };
        Ok(Task { title, notes: clean_text(&self.notes, MAX_TEXT), due, reminder, ..self.task.clone() })
    }
}

impl Pim {
    /// The list shown: the one chosen, else the focused account's default one, else the first.
    fn current_list(&self, cx: &Ctx<'_>) -> Option<(AccountId, String)> {
        if let Some((a, l)) = &self.tasks.selected
            && self.data.get(a).is_some_and(|d| d.lists.iter().any(|x| &x.id == l))
        {
            return Some((*a, l.clone()));
        }
        let focused = cx.ws.focused_account().map(|a| a.id);
        let ordered = cx.ws.accounts().iter().map(|a| a.id).filter(|id| Some(*id) == focused).chain(cx.ws.accounts().iter().map(|a| a.id));
        for id in ordered {
            if let Some(d) = self.data.get(&id) {
                let list = d.lists.iter().find(|l| l.default).or_else(|| d.lists.first());
                if let Some(l) = list {
                    return Some((id, l.id.clone()));
                }
            }
        }
        None
    }

    /// The tasks' commands, in the command bar.
    pub fn tasks_commands(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        if ui.add(Button::new(RichText::new("New task").strong().color(t::on_accent())).fill(t::accent())).clicked() {
            self.new_task(cx, None);
        }
        ui.separator();
        ui.checkbox(&mut self.tasks.show_done, "Show completed tasks");
    }

    /// Opens a new task beside the list, in `account`'s default list (else the list shown).
    pub fn new_task(&mut self, cx: &Ctx<'_>, account: Option<AccountId>) {
        if let Some(a) = account
            && let Some(l) = self.data.get(&a).and_then(|d| d.lists.iter().find(|l| l.default).or_else(|| d.lists.first()))
        {
            self.tasks.selected = Some((a, l.id.clone()));
        }
        match self.current_list(cx) {
            Some((a, l)) => {
                self.tasks.selected = Some((a, l.clone()));
                self.tasks.editing = Some(TaskEditor::new(a, Task { list: l, ..Task::default() }));
            }
            None => self.warn("No task list yet: wait for the accounts' tasks to be read."),
        }
    }

    /// The lists of every account, in the folder pane.
    pub fn tasks_sidebar(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        let current = self.current_list(cx);
        ui.add_space(6.0);
        ui.label(RichText::new("TASK LISTS").size(11.0).strong().color(t::faint()));
        for a in cx.ws.accounts() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(4.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 1.0, t::account(a.color));
                ui.label(RichText::new(&a.name).strong());
            });
            let Some(d) = self.data.get(&a.id) else {
                ui.label(RichText::new("   Waiting for the tasks…").size(12.0).color(t::faint()));
                continue;
            };
            let mut picked: Option<String> = None;
            for l in &d.lists {
                let open = d.tasks.iter().filter(|x| x.list == l.id && !x.completed).count();
                let on = current.as_ref().is_some_and(|(ca, cl)| *ca == a.id && *cl == l.id);
                let label = if open > 0 { format!("{}  ({open})", l.name) } else { l.name.clone() };
                if ui.add(Button::selectable(on, label)).clicked() {
                    picked = Some(l.id.clone());
                }
            }
            if let Some(l) = picked {
                self.tasks.selected = Some((a.id, l));
                self.tasks.editing = None;
            }
        }
    }

    /// The tasks of the list shown, and the task edited.
    pub fn tasks_page(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        let current = self.current_list(cx);
        let mut sends: Vec<(AccountId, PimCommand)> = Vec::new();
        let mut edit: Option<(AccountId, Task)> = None;
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(20, 14))).show(ui, |ui| {
            if let Some(ed) = &mut self.tasks.editing {
                let reminders = cx.provider(ed.account) != Some(Provider::Gmail);
                let mut close = false;
                egui::Panel::right("mailalm-task-editor")
                    .resizable(true)
                    .default_size(340.0)
                    .frame(Frame::new().fill(t::panel2()).inner_margin(Margin::symmetric(14, 12)))
                    .show(ui, |ui| task_editor(ui, ed, reminders, &mut sends, &mut close));
                if close {
                    self.tasks.editing = None;
                }
            }
            let Some((account, list_id)) = current.clone() else {
                ui.label(RichText::new("No task list yet. Outlook accounts show Microsoft To Do, Gmail accounts Google Tasks, and IMAP accounts a list kept on this computer.").color(t::faint()));
                return;
            };
            let Some(d) = self.data.get(&account) else { return };
            let list_name = d.lists.iter().find(|l| l.id == list_id).map_or_else(String::new, |l| l.name.clone());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(r, 3.0, cx.color(account));
                ui.label(RichText::new(&list_name).size(18.0).strong());
                ui.label(RichText::new(cx.name(account)).size(12.0).color(t::faint()));
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let field = ui.add(TextEdit::singleline(&mut self.tasks.new_title).desired_width(ui.available_width() - 70.0).hint_text("Add a task, then Enter"));
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if (ui.button("Add").clicked() || enter) && !self.tasks.new_title.trim().is_empty() {
                    let task = Task { list: list_id.clone(), title: clean_line(&self.tasks.new_title, MAX_LINE), ..Task::default() };
                    sends.push((account, PimCommand::SaveTask(Box::new(task))));
                    self.tasks.new_title.clear();
                    field.request_focus();
                }
            });
            ui.add_space(8.0);
            let today = time::local_day(time::now());
            let mut tasks: Vec<&Task> = d.tasks.iter().filter(|x| x.list == list_id && (self.tasks.show_done || !x.completed)).collect();
            tasks.sort_by_key(|x| (x.completed, x.due.unwrap_or(i64::MAX), x.title.to_lowercase()));
            if tasks.is_empty() {
                ui.label(RichText::new("Nothing to do here.").color(t::faint()));
            }
            ScrollArea::vertical().id_salt("mailalm-tasks").auto_shrink([false, false]).show(ui, |ui| {
                for task in tasks {
                    ui.horizontal(|ui| {
                        let mut done = task.completed;
                        if ui.checkbox(&mut done, "").on_hover_text(if done { "Mark as not done" } else { "Mark as done" }).changed() {
                            let changed = Task { completed: done, completed_at: if done { Some(time::now()) } else { None }, ..task.clone() };
                            sends.push((account, PimCommand::SaveTask(Box::new(changed))));
                        }
                        let title = if done { RichText::new(&task.title).strikethrough().color(t::faint()) } else { RichText::new(&task.title) };
                        if ui.add(Button::new(title).frame_when_inactive(false)).on_hover_text("Edit").clicked() {
                            edit = Some((account, task.clone()));
                        }
                        if let Some(due) = task.due {
                            let day = due.div_euclid(86_400);
                            let late = !task.completed && day < today;
                            ui.label(RichText::new(format!("due {}", time::date(day))).size(12.0).color(if late { t::danger() } else { t::muted() }));
                        }
                        if let Some(r) = task.reminder {
                            ui.label(RichText::new(format!("reminder {} {}", time::date(time::local_day(r)), time::clock(r))).size(12.0).color(t::faint()));
                        }
                    });
                }
            });
        });
        if let Some((a, task)) = edit {
            self.tasks.editing = Some(TaskEditor::new(a, task));
        }
        for (a, c) in sends {
            if !cx.send(a, c) {
                self.warn("The account is not running: the task was not changed.");
            }
        }
    }
}

/// The task edited: title, notes, due day, reminder; Save, Delete, Close.
fn task_editor(ui: &mut Ui, ed: &mut TaskEditor, reminders: bool, sends: &mut Vec<(AccountId, PimCommand)>, close: &mut bool) {
    let new = ed.task.remote.id.is_empty();
    ui.label(RichText::new(if new { "New task" } else { "Task" }).size(16.0).strong());
    ui.add_space(6.0);
    ui.label(RichText::new("Title").color(t::muted()));
    ui.add(TextEdit::singleline(&mut ed.title).desired_width(f32::INFINITY));
    ui.label(RichText::new("Due").color(t::muted()));
    ui.add(TextEdit::singleline(&mut ed.due).desired_width(110.0).hint_text("2026-11-25"));
    ui.label(RichText::new("Reminder").color(t::muted()));
    if reminders {
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut ed.reminder_date).desired_width(110.0).hint_text("2026-11-25"));
            ui.add(TextEdit::singleline(&mut ed.reminder_time).desired_width(56.0).hint_text("09:00"));
        });
    } else {
        ui.label(RichText::new("Google Tasks has no reminders in its API.").size(12.0).color(t::faint()));
    }
    ui.label(RichText::new("Notes").color(t::muted()));
    ui.add(TextEdit::multiline(&mut ed.notes).desired_width(f32::INFINITY).desired_rows(6));
    if let Some(e) = &ed.error {
        ui.label(RichText::new(e.as_str()).color(t::danger()));
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.add(Button::new(RichText::new("Save").strong().color(t::on_accent())).fill(t::accent())).clicked() {
            match ed.build(reminders) {
                Ok(task) => {
                    sends.push((ed.account, PimCommand::SaveTask(Box::new(task))));
                    *close = true;
                }
                Err(e) => ed.error = Some(e),
            }
        }
        if !new && ui.button("Delete…").clicked() {
            ed.confirm_delete = true;
        }
        if ui.button("Close").clicked() {
            *close = true;
        }
    });
    if ed.confirm_delete {
        Frame::new().fill(t::warn_bg()).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.label(RichText::new("Delete this task?").color(t::warn()));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    sends.push((ed.account, PimCommand::DeleteTask(Box::new(ed.task.clone()))));
                    *close = true;
                }
                if ui.button("Keep it").clicked() {
                    ed.confirm_delete = false;
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fields_make_the_task() {
        let mut ed = TaskEditor::new(AccountId(1), Task { list: "L".into(), ..Task::default() });
        assert!(ed.build(true).is_err(), "a title is needed");
        ed.title = "Pay the studio".into();
        ed.due = "2026-11-30".into();
        let t = ed.build(true).expect("a task");
        assert_eq!(t.due.map(|d| d % 86_400), Some(0), "a day, at midnight UTC");
        assert_eq!(t.list, "L");
        ed.reminder_date = "2026-11-29".into();
        ed.reminder_time = "25:00".into();
        assert!(ed.build(true).is_err());
        assert!(ed.build(false).expect("no reminder asked").reminder.is_none());
    }
}
