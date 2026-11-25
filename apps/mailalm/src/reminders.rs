//! The reminders (PLAN.md, section 7), as eM Client shows them: worked out from every account's
//! cache every 30 seconds, offline too (`mailalm_engine::agenda::due`); a window lists those that
//! are due, each with Snooze (5 minutes to a week) and Dismiss, and Dismiss all; the main window
//! asks for the user's attention when a new one rings. They ring only while MailALM runs. What was
//! dismissed or snoozed is kept on this computer (`reminders.json` beside the cache).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Align, ComboBox, Frame, Layout, Margin, RichText, ScrollArea, Sense, Stroke, UserAttentionType, ViewportBuilder, ViewportCommand, ViewportId, vec2};
use mailalm_engine::agenda::{self, Due, Reminded, ReminderState};
use serde::{Deserialize, Serialize};

use crate::agenda::{Ctx, Pim, sources};
use crate::i18n::{tr, tr_key, trf};
use crate::settings::Settings;
use crate::theme as t;
use crate::time;
use crate::zones::TzRules;

/// How often the reminders are worked out again.
const CHECK_EVERY: Duration = Duration::from_secs(30);
/// The snoozes offered: label (translated where shown), seconds.
const SNOOZES: [(&str, i64); 8] = [
    (tr_key("5 minutes"), 300),
    (tr_key("10 minutes"), 600),
    (tr_key("15 minutes"), 900),
    (tr_key("30 minutes"), 1800),
    (tr_key("1 hour"), 3600),
    (tr_key("2 hours"), 7200),
    (tr_key("1 day"), 86_400),
    (tr_key("1 week"), 604_800),
];

/// What is kept on disk.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    dismissed: BTreeMap<String, i64>,
    #[serde(default)]
    snoozed: BTreeMap<String, i64>,
}

#[derive(Default)]
pub struct Reminders {
    state: ReminderState,
    pub due: Vec<Due>,
    last_check: Option<Instant>,
    last_generation: u64,
    /// The window is shown.
    pub open: bool,
    /// Reminders that rang already (the user's attention is asked once for each).
    rung: HashSet<String>,
    /// The snooze chosen on each line, and for all.
    chosen: HashMap<String, usize>,
    all: usize,
}

fn path() -> PathBuf {
    Settings::cache_dir().join("reminders.json")
}

impl Reminders {
    /// What was dismissed and snoozed in earlier runs.
    pub fn load() -> Reminders {
        let saved: Saved = std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut state = ReminderState { dismissed: saved.dismissed, snoozed: saved.snoozed };
        state.prune(time::now());
        Reminders { state, ..Reminders::default() }
    }

    fn save(&self) -> Result<(), String> {
        let saved = Saved { dismissed: self.state.dismissed.clone(), snoozed: self.state.snoozed.clone() };
        let bytes = serde_json::to_vec(&saved).map_err(|e| e.to_string())?;
        if let Some(dir) = path().parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        mailalm_store::write_atomic(&path(), &bytes).map_err(|e| trf!("The reminders could not be kept: {error}", error = e))
    }
}

/// A length of time, short: "5 min", "2 h", "3 days".
fn length(s: i64) -> String {
    let s = s.max(0);
    if s < 3600 {
        trf!("{minutes} min", minutes = (s + 59) / 60)
    } else if s < 2 * 86_400 {
        trf!("{hours} h", hours = (s + 1800) / 3600)
    } else {
        trf!("{count} days", count = (s + 43_200) / 86_400)
    }
}

/// When a reminder's event is, from now.
fn when(d: &Due, now: i64) -> String {
    match &d.what {
        Reminded::Event { start, end, all_day, .. } => {
            if *all_day {
                let day = time::local_day(*start);
                trf!("{weekday} {date}, all day", weekday = time::day_short(day), date = time::date(day))
            } else if *start > now {
                trf!("in {length} ({time})", length = length(start - now), time = time::clock(*start))
            } else if *end > now {
                trf!("now, until {time}", time = time::clock(*end))
            } else {
                trf!("ended {length} ago", length = length(now - end))
            }
        }
        Reminded::Task { due: Some(day), .. } => trf!("task, due {date}", date = time::date(day.div_euclid(86_400))),
        Reminded::Task { due: None, .. } => tr("task").to_owned(),
    }
}

impl Pim {
    /// Works the reminders out every 30 seconds (and when the calendars change), asks for the
    /// user's attention when a new one rings, and shows the window while some are due.
    pub fn reminders_tick(&mut self, ctx: &egui::Context, cx: &Ctx<'_>) {
        let r = &mut self.reminders;
        let stale = r.last_check.is_none_or(|at| at.elapsed() >= CHECK_EVERY) || r.last_generation != self.generation;
        if stale {
            let now = time::now();
            r.due = agenda::due(&sources(&self.data, cx.ws), &r.state, now, &TzRules);
            r.last_check = Some(Instant::now());
            r.last_generation = self.generation;
            let fresh: Vec<String> = r.due.iter().map(|d| format!("{}@{}", d.key, d.at)).filter(|k| !r.rung.contains(k)).collect();
            if !fresh.is_empty() {
                r.rung.extend(fresh);
                r.open = true;
                ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::RequestUserAttention(UserAttentionType::Informational));
            }
        }
        ctx.request_repaint_after(CHECK_EVERY);
        if r.open && !r.due.is_empty() {
            self.reminders_window(ctx, cx);
        }
    }

    fn reminders_window(&mut self, ctx: &egui::Context, cx: &Ctx<'_>) {
        let now = time::now();
        let n = self.reminders.due.len();
        let count = if n > 1 { trf!("{count} reminders", count = n) } else { trf!("{count} reminder", count = n) };
        let builder = ViewportBuilder::default()
            .with_title(format!("{count} · MailALM"))
            .with_app_id("mailalm")
            .with_inner_size([560.0, 400.0])
            .with_min_inner_size([420.0, 240.0])
            .with_always_on_top();
        let mut snooze: Vec<(Due, i64)> = Vec::new();
        let mut dismiss: Vec<Due> = Vec::new();
        let mut closed = false;
        let r = &mut self.reminders;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("mailalm-reminders"), builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                closed = true;
            }
            egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(14, 12))).show(ui, |ui| {
                egui::Panel::bottom("mailalm-reminders-all").frame(Frame::new().inner_margin(Margin::symmetric(0, 8))).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ComboBox::from_id_salt("mailalm-reminders-snooze-all").selected_text(SNOOZES.get(r.all).map_or(tr("5 minutes"), |s| tr(s.0))).show_ui(ui, |ui| {
                            for (i, (label, _)) in SNOOZES.iter().enumerate() {
                                ui.selectable_value(&mut r.all, i, tr(*label));
                            }
                        });
                        if ui.button(tr("Snooze all")).clicked() {
                            let secs = SNOOZES.get(r.all).map_or(300, |s| s.1);
                            snooze.extend(r.due.iter().map(|d| (d.clone(), now + secs)));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(tr("Dismiss all")).clicked() {
                                dismiss.extend(r.due.iter().cloned());
                            }
                        });
                    });
                });
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for d in &r.due {
                        Frame::new().fill(t::panel2()).stroke(Stroke::new(1.0, t::line())).corner_radius(6).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let (bar, _) = ui.allocate_exact_size(vec2(4.0, 34.0), Sense::hover());
                                ui.painter().rect_filled(bar, 1.0, cx.color(d.account));
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(if d.title.is_empty() { tr("(no title)") } else { d.title.as_str() }).strong());
                                    let mut line = format!("{} · {}", when(d, now), cx.name(d.account));
                                    if let Reminded::Event { location, .. } = &d.what
                                        && !location.is_empty()
                                    {
                                        line.push_str(&format!(" · {location}"));
                                    }
                                    ui.label(RichText::new(line).size(12.0).color(t::muted()));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.button(tr("Dismiss")).clicked() {
                                        dismiss.push(d.clone());
                                    }
                                    let choice = r.chosen.entry(d.key.clone()).or_insert(0);
                                    if ui.button(tr("Snooze")).clicked() {
                                        snooze.push((d.clone(), now + SNOOZES.get(*choice).map_or(300, |s| s.1)));
                                    }
                                    ComboBox::from_id_salt(("mailalm-reminder-snooze", d.key.as_str())).width(100.0).selected_text(SNOOZES.get(*choice).map_or(tr("5 minutes"), |s| tr(s.0))).show_ui(ui, |ui| {
                                        for (i, (label, _)) in SNOOZES.iter().enumerate() {
                                            ui.selectable_value(choice, i, tr(*label));
                                        }
                                    });
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
            });
        });
        let r = &mut self.reminders;
        if closed {
            r.open = false;
        }
        if snooze.is_empty() && dismiss.is_empty() {
            return;
        }
        for (d, until) in &snooze {
            r.state.snooze(d, *until);
        }
        for d in &dismiss {
            r.state.dismiss(d);
        }
        r.state.prune(now);
        r.last_check = None;
        let saved = r.save();
        if let Err(e) = saved {
            self.warn(e);
        }
    }

    /// Opens the reminders window again (from the calendar's command bar).
    pub fn show_reminders(&mut self) {
        self.reminders.open = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_short() {
        assert_eq!(length(60), "1 min");
        assert_eq!(length(90 * 60), "2 h");
        assert_eq!(length(3 * 86_400), "3 days");
    }
}
