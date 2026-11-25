//! The event window (PLAN.md, sections 7 and 12): a separate window, like the compose window, to
//! plan an event or a meeting: its calendar (any account's), title, times and zone, place,
//! notes, guests (the contacts offered), reminder, repetition, online meeting (Google Meet from a
//! Gmail calendar, Teams from an Outlook one when Microsoft offers it, or a link) and how it shows
//! the user's time. What it clashes with in every calendar of every account is shown while it is
//! planned (A3).

use egui::{Align, Button, ComboBox, Frame, Grid, Id, Layout, Margin, RichText, ScrollArea, TextEdit, Ui, ViewportBuilder, ViewportCommand, ViewportId};
use mailalm_engine::PimCommand;
use mailalm_engine::agenda::{self, Clash, Source};
use mailalm_model::pim::{
    Attendee, BusyStatus, Event as CalendarEvent, Freq, MAX_LINE, MAX_TEXT, MeetingProvider, OnlineMeeting, Recurrence, Until, clean_line, clean_text, local_time, parse_date, utc_time,
};
use mailalm_model::{AccountId, Person, Provider};

use crate::agenda::{CalendarChoice, Ctx, Pim, choices, link_domain, sources};
use crate::autocomplete;
use crate::calendar::{busy_label, repeat_text};
use crate::theme as t;
use crate::time;
use crate::zones::{self, TzRules};

/// The reminders offered: label, minutes before.
const REMINDERS: [(&str, u32); 9] = [
    ("At the start", 0),
    ("5 minutes before", 5),
    ("10 minutes before", 10),
    ("15 minutes before", 15),
    ("30 minutes before", 30),
    ("1 hour before", 60),
    ("2 hours before", 120),
    ("1 day before", 1440),
    ("1 week before", 10_080),
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reminder {
    None,
    Minutes(u32),
    /// The calendar's own (Google).
    CalendarDefault,
    /// What the event had, not among the choices.
    Kept(Vec<u32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Repeat {
    Never,
    Every(Freq),
    /// The event's own rule, not among the choices.
    Kept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RepeatEnd {
    Never,
    After,
    Until,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Online {
    None,
    Meet,
    Teams,
    Link,
    /// The meeting the event has (made by its provider).
    Kept,
}

pub enum EditorAction {
    None,
    Close,
    Save(AccountId, Box<CalendarEvent>, bool),
    Delete(AccountId, Box<CalendarEvent>, bool),
}

/// One event being planned or changed, in its window.
pub struct EventEditor {
    id: u64,
    account: AccountId,
    calendar: String,
    /// The event as it was, when changing one.
    original: Option<CalendarEvent>,
    title: String,
    all_day: bool,
    start_date: String,
    start_time: String,
    end_date: String,
    end_time: String,
    zone: String,
    location: String,
    description: String,
    guests: String,
    reminder: Reminder,
    repeat: Repeat,
    interval: u32,
    repeat_end: RepeatEnd,
    count: u32,
    until: String,
    online: Online,
    link: String,
    busy: BusyStatus,
    notify: bool,
    error: Option<String>,
    confirm_delete: bool,
    confirm_close: bool,
    /// What the fields said when the window opened (to ask before closing over changes).
    initial: String,
    clashes: Vec<Clash>,
    clash_key: Option<(i64, i64, u64)>,
}

/// The wall-clock date and time of `t` in `zone` (the viewer's when empty).
fn split(t: i64, zone: &str) -> (String, String) {
    let local = local_time(&TzRules, zone, t);
    (time::date(local.div_euclid(86_400)), {
        let s = local.rem_euclid(86_400);
        format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
    })
}

impl EventEditor {
    fn blank(id: u64, choice: &CalendarChoice) -> EventEditor {
        EventEditor {
            id,
            account: choice.account,
            calendar: choice.calendar.clone(),
            original: None,
            title: String::new(),
            all_day: false,
            start_date: String::new(),
            start_time: String::new(),
            end_date: String::new(),
            end_time: String::new(),
            zone: time::zone_name(),
            location: String::new(),
            description: String::new(),
            guests: String::new(),
            reminder: Reminder::Minutes(15),
            repeat: Repeat::Never,
            interval: 1,
            repeat_end: RepeatEnd::Never,
            count: 10,
            until: String::new(),
            online: Online::None,
            link: String::new(),
            busy: BusyStatus::Busy,
            notify: true,
            error: None,
            confirm_delete: false,
            confirm_close: false,
            initial: String::new(),
            clashes: Vec::new(),
            clash_key: None,
        }
    }

    /// A new event in `choice`'s calendar, from `start` (an hour, or the day).
    fn new(id: u64, choice: &CalendarChoice, start: i64, all_day: bool) -> EventEditor {
        let mut ed = EventEditor::blank(id, choice);
        ed.set_times(start, if all_day { start + 86_400 } else { start + 3600 }, all_day);
        ed.until = ed.start_date.clone();
        ed.initial = ed.snapshot();
        ed
    }

    /// The window of an event of `account`.
    fn edit(id: u64, account: AccountId, e: &CalendarEvent) -> EventEditor {
        let choice = CalendarChoice { account, calendar: e.calendar.clone(), label: String::new(), provider: Provider::Gmail, address: String::new() };
        let mut ed = EventEditor::blank(id, &choice);
        ed.zone = if e.all_day || e.time_zone.trim().is_empty() || !zones::known(&e.time_zone) { time::zone_name() } else { e.time_zone.clone() };
        ed.set_times(e.start, e.end, e.all_day);
        ed.title = e.title.clone();
        ed.location = e.location.clone();
        ed.description = e.description.clone();
        let organizer = e.organizer.as_ref().map(|o| o.email.clone()).unwrap_or_default();
        ed.guests = e.attendees.iter().filter(|a| !a.is(&organizer)).map(|a| mailalm_mime::format_person(&Person { name: a.name.clone(), address: a.email.clone() })).collect::<Vec<_>>().join(", ");
        ed.reminder = match e.reminders.as_slice() {
            [] if e.default_reminders => Reminder::CalendarDefault,
            [] => Reminder::None,
            [m] if REMINDERS.iter().any(|(_, x)| x == m) => Reminder::Minutes(*m),
            other => Reminder::Kept(other.to_vec()),
        };
        ed.repeat = match &e.recurrence {
            None => Repeat::Never,
            Some(r) if r.by_day.is_empty() && r.by_month_day.is_empty() && r.by_month.is_empty() && r.by_set_pos.is_empty() && !matches!(r.until, Some(Until::At(_) | Until::Local(_))) => {
                ed.interval = r.interval.max(1);
                match (r.count, r.until) {
                    (Some(n), _) => {
                        ed.repeat_end = RepeatEnd::After;
                        ed.count = n;
                    }
                    (None, Some(Until::Date(d))) => {
                        ed.repeat_end = RepeatEnd::Until;
                        ed.until = time::date(d);
                    }
                    _ => {}
                }
                Repeat::Every(r.freq)
            }
            Some(_) => Repeat::Kept,
        };
        if ed.until.is_empty() {
            ed.until = ed.start_date.clone();
        }
        ed.online = match &e.online {
            None => Online::None,
            Some(o) if o.provider == MeetingProvider::Link => {
                ed.link = o.url.clone();
                Online::Link
            }
            Some(_) => Online::Kept,
        };
        ed.busy = e.busy;
        ed.original = Some(e.clone());
        ed.initial = ed.snapshot();
        ed
    }

    pub fn account(&self) -> AccountId {
        self.account
    }

    /// Whether this window shows `e` of `account`.
    fn shows(&self, account: AccountId, e: &CalendarEvent) -> bool {
        self.account == account && self.original.as_ref().is_some_and(|o| o.uid == e.uid && o.remote.id == e.remote.id && o.recurrence_id == e.recurrence_id)
    }

    fn set_times(&mut self, start: i64, end: i64, all_day: bool) {
        self.all_day = all_day;
        if all_day {
            let first = start.div_euclid(86_400);
            let last = (end.div_euclid(86_400) - 1).max(first);
            (self.start_date, self.end_date) = (time::date(first), time::date(last));
            (self.start_time, self.end_time) = ("09:00".to_owned(), "10:00".to_owned());
        } else {
            (self.start_date, self.start_time) = split(start, &self.zone);
            (self.end_date, self.end_time) = split(end, &self.zone);
        }
    }

    /// Every field, as one text (to see whether anything changed).
    fn snapshot(&self) -> String {
        format!(
            "{:?}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{:?}|{:?}|{}|{:?}|{}|{}|{:?}|{}|{:?}",
            self.account,
            self.calendar,
            self.title,
            self.all_day,
            self.start_date,
            self.start_time,
            self.end_date,
            self.end_time,
            self.zone,
            self.location,
            self.description,
            self.reminder,
            self.repeat,
            self.interval,
            self.repeat_end,
            self.count,
            self.until,
            self.online,
            self.guests,
            self.busy
        )
    }

    /// The times typed, in UTC: (start, end).
    fn times(&self) -> Result<(i64, i64), String> {
        let first = parse_date(&self.start_date).ok_or("The start date is not a date: write it as 2026-11-25.")?;
        let last = parse_date(&self.end_date).ok_or("The end date is not a date: write it as 2026-11-25.")?;
        if self.all_day {
            if last < first {
                return Err("The event ends before it starts.".to_owned());
            }
            return Ok((first * 86_400, (last + 1) * 86_400));
        }
        let a = time::parse_clock(&self.start_time).ok_or("The start time is not a time: write it as 14:30.")?;
        let b = time::parse_clock(&self.end_time).ok_or("The end time is not a time: write it as 15:30.")?;
        let zone = self.zone.trim();
        if !zone.is_empty() && !zones::known(zone) {
            return Err(format!("MailALM doesn't know the time zone \"{zone}\": write a name like America/Toronto."));
        }
        let (start, end) = (utc_time(&TzRules, zone, first * 86_400 + a), utc_time(&TzRules, zone, last * 86_400 + b));
        if end <= start {
            return Err("The event ends before it starts.".to_owned());
        }
        Ok((start, end))
    }

    /// The event the fields describe.
    fn build(&self, me: &str) -> Result<CalendarEvent, String> {
        let (start, end) = self.times()?;
        let mut e = self.original.clone().unwrap_or_else(|| CalendarEvent { calendar: self.calendar.clone(), is_organizer: true, ..CalendarEvent::default() });
        e.title = clean_line(&self.title, MAX_LINE);
        (e.start, e.end, e.all_day) = (start, end, self.all_day);
        e.time_zone = if self.all_day { String::new() } else { self.zone.trim().to_owned() };
        e.location = clean_line(&self.location, MAX_LINE);
        e.description = clean_text(&self.description, MAX_TEXT);
        let people = mailalm_mime::parse_recipients(&self.guests)?;
        let mut guests: Vec<Attendee> = Vec::new();
        for p in people {
            if p.address.eq_ignore_ascii_case(me) || guests.iter().any(|g| g.is(&p.address)) {
                continue;
            }
            // A guest already invited keeps their answer.
            let kept = e.attendees.iter().find(|a| a.is(&p.address)).cloned();
            guests.push(kept.unwrap_or_else(|| Attendee::new(&p.address, &p.name)));
        }
        if let Some(o) = e.organizer.clone().filter(|o| e.attendees.iter().any(|a| a.is(&o.email))) {
            // The organizer stays among the guests as the provider listed them.
            guests.insert(0, e.attendees.iter().find(|a| a.is(&o.email)).cloned().unwrap_or(o));
        }
        e.attendees = guests;
        (e.reminders, e.default_reminders) = match &self.reminder {
            Reminder::None => (Vec::new(), false),
            Reminder::Minutes(m) => (vec![*m], false),
            Reminder::CalendarDefault => (Vec::new(), true),
            Reminder::Kept(v) => (v.clone(), false),
        };
        if e.recurrence_id.is_none() {
            e.recurrence = match self.repeat {
                Repeat::Never => None,
                Repeat::Kept => self.original.as_ref().and_then(|o| o.recurrence.clone()),
                Repeat::Every(freq) => {
                    let mut r = Recurrence::new(freq);
                    r.interval = self.interval.clamp(1, 999);
                    match self.repeat_end {
                        RepeatEnd::Never => {}
                        RepeatEnd::After => r.count = Some(self.count.clamp(1, 999)),
                        RepeatEnd::Until => r.until = Some(Until::Date(parse_date(&self.until).ok_or("The last day of the repetition is not a date: write it as 2026-12-31.")?)),
                    }
                    Some(r)
                }
            };
        }
        e.online = match self.online {
            Online::None => None,
            Online::Meet => Some(OnlineMeeting::request(MeetingProvider::Meet)),
            Online::Teams => Some(OnlineMeeting::request(MeetingProvider::Teams)),
            Online::Link => Some(OnlineMeeting::at(&self.link).ok_or("The meeting link must be a secure web address (https://…).")?),
            Online::Kept => self.original.as_ref().and_then(|o| o.online.clone()),
        };
        e.busy = self.busy;
        Ok(e)
    }

    /// Shows the window. `choices` are the calendars that can be changed, `sources` every
    /// account's calendars (for the clashes), `suggest` the addresses offered.
    pub fn show(&mut self, ctx: &egui::Context, choices: &[CalendarChoice], sources: &[Source<'_>], generation: u64, suggest: &dyn Fn(&str) -> Vec<Person>) -> EditorAction {
        let title = if self.title.trim().is_empty() { if self.original.is_some() { "Event" } else { "New event" }.to_owned() } else { self.title.trim().to_owned() };
        let builder = ViewportBuilder::default().with_title(format!("{title} · MailALM")).with_app_id("mailalm").with_inner_size([660.0, 760.0]).with_min_inner_size([480.0, 420.0]);
        ctx.show_viewport_immediate(ViewportId::from_hash_of(("mailalm-event", self.id)), builder, |ui, _class| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                if self.snapshot() == self.initial {
                    return EditorAction::Close;
                }
                ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm_close = true;
            }
            self.update_clashes(sources, generation);
            let mut action = EditorAction::None;
            egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(18, 14))).show(ui, |ui| {
                action = self.body(ui, choices, suggest);
            });
            action
        })
    }

    fn update_clashes(&mut self, sources: &[Source<'_>], generation: u64) {
        let Ok((start, end)) = self.times() else {
            self.clashes.clear();
            self.clash_key = None;
            return;
        };
        if self.clash_key == Some((start, end, generation)) {
            return;
        }
        let uid = self.original.as_ref().map(|o| o.uid.clone()).unwrap_or_default();
        self.clashes = agenda::clashes(sources, start, end, &uid, &TzRules);
        self.clash_key = Some((start, end, generation));
    }

    fn body(&mut self, ui: &mut Ui, choices: &[CalendarChoice], suggest: &dyn Fn(&str) -> Vec<Person>) -> EditorAction {
        let mut action = EditorAction::None;
        let choice = choices.iter().find(|c| c.account == self.account && c.calendar == self.calendar).cloned();
        let provider = choice.as_ref().map(|c| c.provider);
        let me = choice.as_ref().map(|c| c.address.clone()).unwrap_or_default();
        let is_meeting_guest = self.original.as_ref().is_some_and(|o| o.is_meeting() && !o.is_organizer);
        ui.horizontal(|ui| {
            let save = ui.add_enabled(choice.is_some() && !is_meeting_guest, Button::new(RichText::new("Save").strong().color(t::on_accent())).fill(t::accent()));
            if save.clicked() {
                match self.build(&me) {
                    Ok(e) => {
                        let notify = self.notify && e.is_meeting();
                        action = EditorAction::Save(self.account, Box::new(e), notify);
                    }
                    Err(err) => self.error = Some(err),
                }
            }
            if ui.button("Cancel").clicked() {
                if self.snapshot() == self.initial {
                    action = EditorAction::Close;
                } else {
                    self.confirm_close = true;
                }
            }
            if self.original.is_some() && ui.button("Delete…").clicked() {
                self.confirm_delete = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(c) = &choice {
                    ui.label(RichText::new(&c.label).size(12.0).color(t::muted()));
                }
            });
        });
        if is_meeting_guest {
            ui.label(RichText::new("You are a guest of this meeting: only its organizer can change it. Answer it from the calendar.").color(t::warn()));
        }
        if self.confirm_close {
            warn_frame(ui, |ui| {
                ui.label(RichText::new("Close without saving? Your changes are lost.").color(t::warn()));
                if ui.button("Close").clicked() {
                    action = EditorAction::Close;
                }
                if ui.button("Keep editing").clicked() {
                    self.confirm_close = false;
                }
            });
        }
        if self.confirm_delete
            && let Some(o) = &self.original
        {
            let tell = o.is_organizer && o.is_meeting();
            warn_frame(ui, |ui| {
                ui.label(RichText::new(if o.recurrence.is_some() { "Delete this event and every occurrence of its series?" } else { "Delete this event?" }).color(t::warn()));
                if ui.button("Delete").clicked() {
                    action = EditorAction::Delete(self.account, Box::new(o.clone()), tell);
                }
                if ui.button("Keep it").clicked() {
                    self.confirm_delete = false;
                }
            });
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e.as_str()).color(t::danger()));
        }
        ui.add_space(8.0);
        ScrollArea::vertical().id_salt(("mailalm-event-scroll", self.id)).auto_shrink([false, false]).show(ui, |ui| {
            self.fields(ui, choices, provider, suggest);
            self.clash_list(ui);
        });
        action
    }

    fn fields(&mut self, ui: &mut Ui, choices: &[CalendarChoice], provider: Option<Provider>, suggest: &dyn Fn(&str) -> Vec<Person>) {
        let id = self.id;
        Grid::new(("mailalm-event-fields", id)).num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Calendar").color(t::muted()));
            let current = choices.iter().find(|c| c.account == self.account && c.calendar == self.calendar).map_or_else(|| "(a calendar that can't be changed)".to_owned(), |c| c.label.clone());
            if self.original.is_some() {
                ui.label(current);
            } else {
                let mut picked: Option<(AccountId, String)> = None;
                ComboBox::from_id_salt(("mailalm-event-calendar", id)).selected_text(current).width(360.0).show_ui(ui, |ui| {
                    for c in choices {
                        let on = c.account == self.account && c.calendar == self.calendar;
                        if ui.selectable_label(on, c.label.as_str()).clicked() {
                            picked = Some((c.account, c.calendar.clone()));
                        }
                    }
                });
                if let Some((a, c)) = picked {
                    self.account = a;
                    self.calendar = c;
                    // A meeting made by one provider can't be asked of another.
                    if matches!(self.online, Online::Meet | Online::Teams) {
                        self.online = Online::None;
                    }
                }
            }
            ui.end_row();

            ui.label(RichText::new("Title").color(t::muted()));
            ui.add(TextEdit::singleline(&mut self.title).desired_width(f32::INFINITY).hint_text("What is it?"));
            ui.end_row();

            ui.label("");
            if ui.checkbox(&mut self.all_day, "All day").changed() {
                self.check_dates();
            }
            ui.end_row();

            ui.label(RichText::new("Starts").color(t::muted()));
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.start_date).desired_width(100.0).hint_text("2026-11-25"));
                if !self.all_day {
                    ui.add(TextEdit::singleline(&mut self.start_time).desired_width(56.0).hint_text("14:00"));
                }
            });
            ui.end_row();
            ui.label(RichText::new("Ends").color(t::muted()));
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.end_date).desired_width(100.0).hint_text("2026-11-25"));
                if !self.all_day {
                    ui.add(TextEdit::singleline(&mut self.end_time).desired_width(56.0).hint_text("15:00"));
                }
            });
            ui.end_row();
            if !self.all_day {
                ui.label(RichText::new("Time zone").color(t::muted()));
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.zone).desired_width(200.0).hint_text("America/Toronto"));
                    let z = self.zone.trim();
                    if !z.is_empty() && !zones::known(z) {
                        ui.label(RichText::new("unknown zone").size(12.0).color(t::danger()));
                    }
                });
                ui.end_row();
            }

            ui.label(RichText::new("Location").color(t::muted()));
            ui.add(TextEdit::singleline(&mut self.location).desired_width(f32::INFINITY).hint_text("A place, a room"));
            ui.end_row();

            ui.label(RichText::new("Online meeting").color(t::muted()));
            ui.vertical(|ui| self.online_choice(ui, provider));
            ui.end_row();

            ui.label(RichText::new("Guests").color(t::muted()));
            ui.vertical(|ui| {
                let width = ui.available_width();
                autocomplete::field(ui, Id::unique(("mailalm-event-guests", id)), &mut self.guests, width, "name@example.com, …", suggest);
                let hint = match provider {
                    Some(Provider::Outlook) => "Outlook sends the invitations.",
                    Some(Provider::Gmail) => "Google Calendar sends the invitations.",
                    _ => "MailALM sends the invitations by mail from this account.",
                };
                ui.label(RichText::new(hint).size(11.5).color(t::faint()));
            });
            ui.end_row();

            ui.label(RichText::new("Reminder").color(t::muted()));
            self.reminder_choice(ui, provider);
            ui.end_row();

            ui.label(RichText::new("Repeat").color(t::muted()));
            ui.vertical(|ui| self.repeat_choice(ui));
            ui.end_row();

            ui.label(RichText::new("Show as").color(t::muted()));
            ComboBox::from_id_salt(("mailalm-event-busy", id)).selected_text(busy_label(self.busy)).show_ui(ui, |ui| {
                for b in [BusyStatus::Free, BusyStatus::Tentative, BusyStatus::Busy, BusyStatus::OutOfOffice] {
                    ui.selectable_value(&mut self.busy, b, busy_label(b));
                }
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Description").color(t::muted()));
        ui.add(TextEdit::multiline(&mut self.description).desired_width(f32::INFINITY).desired_rows(6).hint_text("Notes, plain text"));
        if !self.guests.trim().is_empty() {
            ui.checkbox(&mut self.notify, "Send the invitation (or its update) to the guests");
        }
    }

    /// Keeps the end on or after the start when the event becomes all-day.
    fn check_dates(&mut self) {
        if let (Some(a), Some(b)) = (parse_date(&self.start_date), parse_date(&self.end_date))
            && b < a
        {
            self.end_date = self.start_date.clone();
        }
    }

    fn online_choice(&mut self, ui: &mut Ui, provider: Option<Provider>) {
        let mut options: Vec<(Online, String)> = vec![(Online::None, "None".to_owned())];
        if let Some(o) = self.original.as_ref().and_then(|o| o.online.as_ref()).filter(|o| o.provider != MeetingProvider::Link) {
            let what = match link_domain(&o.url) {
                Some(d) => format!("Keep the meeting ({d})"),
                None => "Keep the meeting being made".to_owned(),
            };
            options.push((Online::Kept, what));
        }
        match provider {
            Some(Provider::Gmail) => options.push((Online::Meet, "Google Meet".to_owned())),
            Some(Provider::Outlook) => options.push((Online::Teams, "Microsoft Teams".to_owned())),
            _ => {}
        }
        options.push((Online::Link, "A link…".to_owned()));
        let current = options.iter().find(|(o, _)| *o == self.online).map_or_else(|| "None".to_owned(), |(_, l)| l.clone());
        ComboBox::from_id_salt(("mailalm-event-online", self.id)).selected_text(current).width(260.0).show_ui(ui, |ui| {
            for (o, label) in &options {
                ui.selectable_value(&mut self.online, *o, label.as_str());
            }
        });
        match self.online {
            Online::Teams => {
                ui.label(RichText::new("Microsoft makes the Teams link if this account offers it; if it doesn't, saving says so and nothing is made.").size(11.5).color(t::faint()));
            }
            Online::Meet => {
                ui.label(RichText::new("Google makes the Meet link when the event is saved.").size(11.5).color(t::faint()));
            }
            Online::Link => {
                ui.add(TextEdit::singleline(&mut self.link).desired_width(f32::INFINITY).hint_text("https://…"));
                if let Some(d) = link_domain(&self.link) {
                    ui.label(RichText::new(format!("Guests will see a link to {d}.")).size(11.5).color(t::faint()));
                }
            }
            Online::None | Online::Kept => {}
        }
    }

    fn reminder_choice(&mut self, ui: &mut Ui, provider: Option<Provider>) {
        let mut options: Vec<(Reminder, String)> = vec![(Reminder::None, "None".to_owned())];
        options.extend(REMINDERS.iter().map(|(l, m)| (Reminder::Minutes(*m), (*l).to_owned())));
        if provider == Some(Provider::Gmail) {
            options.push((Reminder::CalendarDefault, "The calendar's own".to_owned()));
        }
        if let Reminder::Kept(v) = &self.reminder {
            let list: Vec<String> = v.iter().map(|m| format!("{m} min")).collect();
            options.push((self.reminder.clone(), format!("As it was ({})", list.join(", "))));
        }
        let current = options.iter().find(|(r, _)| *r == self.reminder).map_or_else(|| "None".to_owned(), |(_, l)| l.clone());
        ComboBox::from_id_salt(("mailalm-event-reminder", self.id)).selected_text(current).width(220.0).show_ui(ui, |ui| {
            for (r, label) in &options {
                ui.selectable_value(&mut self.reminder, r.clone(), label.as_str());
            }
        });
    }

    fn repeat_choice(&mut self, ui: &mut Ui) {
        if self.original.as_ref().is_some_and(|o| o.recurrence_id.is_some()) {
            ui.label(RichText::new("One occurrence of a series: changes apply to it alone.").size(12.0).color(t::faint()));
            return;
        }
        let label = |r: Repeat| match r {
            Repeat::Never => "Never".to_owned(),
            Repeat::Every(Freq::Daily) => "Every day".to_owned(),
            Repeat::Every(Freq::Weekly) => "Every week".to_owned(),
            Repeat::Every(Freq::Monthly) => "Every month".to_owned(),
            Repeat::Every(Freq::Yearly) => "Every year".to_owned(),
            Repeat::Kept => "As it was".to_owned(),
        };
        let mut options = vec![Repeat::Never, Repeat::Every(Freq::Daily), Repeat::Every(Freq::Weekly), Repeat::Every(Freq::Monthly), Repeat::Every(Freq::Yearly)];
        if self.original.as_ref().is_some_and(|o| o.recurrence.is_some()) && self.repeat == Repeat::Kept {
            options.push(Repeat::Kept);
        }
        ui.horizontal(|ui| {
            ComboBox::from_id_salt(("mailalm-event-repeat", self.id)).selected_text(label(self.repeat)).show_ui(ui, |ui| {
                for r in options {
                    ui.selectable_value(&mut self.repeat, r, label(r));
                }
            });
            if matches!(self.repeat, Repeat::Every(_)) {
                ui.label("every");
                ui.add(egui::DragValue::new(&mut self.interval).range(1..=99));
                let unit = match self.repeat {
                    Repeat::Every(Freq::Daily) => "day(s)",
                    Repeat::Every(Freq::Weekly) => "week(s)",
                    Repeat::Every(Freq::Monthly) => "month(s)",
                    _ => "year(s)",
                };
                ui.label(unit);
            }
        });
        match self.repeat {
            Repeat::Kept => {
                if let Some(r) = self.original.as_ref().and_then(|o| o.recurrence.as_ref()) {
                    ui.label(RichText::new(repeat_text(r)).size(12.0).color(t::faint()));
                }
            }
            Repeat::Every(_) => {
                ui.horizontal(|ui| {
                    ui.label("Ends");
                    ComboBox::from_id_salt(("mailalm-event-repeat-end", self.id))
                        .selected_text(match self.repeat_end {
                            RepeatEnd::Never => "never",
                            RepeatEnd::After => "after",
                            RepeatEnd::Until => "on",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.repeat_end, RepeatEnd::Never, "never");
                            ui.selectable_value(&mut self.repeat_end, RepeatEnd::After, "after");
                            ui.selectable_value(&mut self.repeat_end, RepeatEnd::Until, "on");
                        });
                    match self.repeat_end {
                        RepeatEnd::After => {
                            ui.add(egui::DragValue::new(&mut self.count).range(1..=999));
                            ui.label("times");
                        }
                        RepeatEnd::Until => {
                            ui.add(TextEdit::singleline(&mut self.until).desired_width(100.0).hint_text("2026-12-31"));
                        }
                        RepeatEnd::Never => {}
                    }
                });
            }
            Repeat::Never => {}
        }
    }

    /// What the event clashes with in every calendar of every account.
    fn clash_list(&self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.separator();
        if self.times().is_err() {
            return;
        }
        if self.clashes.is_empty() {
            ui.label(RichText::new("No clash in your calendars, all accounts.").size(12.5).color(t::ok()));
            return;
        }
        ui.label(RichText::new(format!("Clashes with {} event{} in your calendars:", self.clashes.len(), if self.clashes.len() > 1 { "s" } else { "" })).color(t::warn()));
        for c in self.clashes.iter().take(20) {
            ui.label(RichText::new(format!("• {} — {} ({})", c.title, time::span(c.start, c.end, false), busy_label(c.status))).size(12.0));
        }
    }
}

fn warn_frame(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    Frame::new().fill(t::warn_bg()).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
        ui.horizontal_wrapped(add);
    });
}

impl Pim {
    /// Opens a window for a new event or meeting, from `start` (an hour, or the day), in
    /// `account`'s calendar (else the focused account's, else the first one that can be changed).
    pub fn new_event(&mut self, cx: &Ctx<'_>, account: Option<AccountId>, start: i64, all_day: bool, meeting: bool) {
        let list = choices(&self.data, cx.ws);
        let wanted = account.or_else(|| cx.ws.focused_account().map(|a| a.id));
        let Some(choice) = list.iter().find(|c| Some(c.account) == wanted).or_else(|| list.first()) else {
            self.warn("No calendar can be added to yet: wait for the accounts' calendars to be read.");
            return;
        };
        let id = self.next_editor;
        self.next_editor += 1;
        let mut ed = EventEditor::new(id, choice, start, all_day);
        if meeting {
            ed.online = match choice.provider {
                Provider::Gmail => Online::Meet,
                Provider::Outlook => Online::Teams,
                Provider::Imap(_) => Online::None,
            };
            ed.initial = ed.snapshot();
        }
        self.editors.push(ed);
    }

    /// Opens the window of an event of `account` (once).
    pub fn edit_event(&mut self, _cx: &Ctx<'_>, account: AccountId, e: &CalendarEvent) {
        if self.editors.iter().any(|ed| ed.shows(account, e)) {
            return;
        }
        let id = self.next_editor;
        self.next_editor += 1;
        self.editors.push(EventEditor::edit(id, account, e));
    }

    /// Shows the event windows, and sends what they save or delete.
    pub fn event_windows(&mut self, ctx: &egui::Context, cx: &Ctx<'_>) {
        if self.editors.is_empty() {
            return;
        }
        let list = choices(&self.data, cx.ws);
        let srcs = sources(&self.data, cx.ws);
        let book = &self.book;
        let suggest = |typed: &str| book.suggest(typed, 8);
        let generation = self.generation;
        let mut done: Vec<usize> = Vec::new();
        let mut sends: Vec<(AccountId, PimCommand, &'static str)> = Vec::new();
        for (n, ed) in self.editors.iter_mut().enumerate() {
            match ed.show(ctx, &list, &srcs, generation, &suggest) {
                EditorAction::None => {}
                EditorAction::Close => done.push(n),
                EditorAction::Save(account, event, notify) => {
                    let text = if notify { "The event was saved; its guests are invited." } else { "The event was saved." };
                    sends.push((account, PimCommand::SaveEvent { event, notify }, text));
                    done.push(n);
                }
                EditorAction::Delete(account, event, notify) => {
                    sends.push((account, PimCommand::DeleteEvent { event, notify }, "The event was deleted."));
                    done.push(n);
                }
            }
        }
        drop(srcs);
        for n in done.into_iter().rev() {
            if n < self.editors.len() {
                self.editors.remove(n);
            }
        }
        for (account, command, text) in sends {
            if cx.send(account, command) {
                self.say(text);
            } else {
                self.warn("The account is not running: nothing was changed.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice() -> CalendarChoice {
        CalendarChoice { account: AccountId(1), calendar: "local".into(), label: "Bell · Calendar".into(), provider: Provider::Imap(mailalm_model::ImapPreset::Bell), address: "me@bell.net".into() }
    }

    #[test]
    fn the_fields_make_the_event() {
        let mut ed = EventEditor::new(1, &choice(), 1_795_600_800, false);
        ed.zone = "UTC".into();
        ed.start_date = "2026-11-25".into();
        ed.start_time = "14:00".into();
        ed.end_date = "2026-11-25".into();
        ed.end_time = "15:30".into();
        ed.title = "Mix\nreview".into();
        ed.guests = "Zoe <zoe@studio.example>, me@bell.net".into();
        ed.repeat = Repeat::Every(Freq::Weekly);
        ed.repeat_end = RepeatEnd::After;
        ed.count = 4;
        ed.reminder = Reminder::Minutes(10);
        let e = ed.build("me@bell.net").expect("an event");
        assert_eq!(e.title, "Mix review", "one line");
        assert_eq!(e.end - e.start, 5400);
        assert_eq!(e.start % 86_400, 14 * 3600);
        assert_eq!(e.attendees.len(), 1, "not the organizer");
        assert_eq!(e.recurrence.as_ref().and_then(|r| r.count), Some(4));
        assert_eq!(e.reminders, vec![10]);
        assert!(e.is_organizer);

        ed.end_time = "13:00".into();
        assert!(ed.build("me@bell.net").is_err(), "ends before it starts");
        ed.end_time = "15:30".into();
        ed.zone = "Mars/Olympus".into();
        assert!(ed.build("me@bell.net").is_err(), "unknown zone");
        ed.zone = "UTC".into();
        ed.online = Online::Link;
        ed.link = "http://insecure.example/room".into();
        assert!(ed.build("me@bell.net").is_err(), "https only");
        ed.link = "https://zoom.us/j/123".into();
        assert!(ed.build("me@bell.net").is_ok());
    }

    #[test]
    fn an_all_day_event_takes_whole_days() {
        let mut ed = EventEditor::new(2, &choice(), 1_795_564_800, true);
        ed.start_date = "2026-11-25".into();
        ed.end_date = "2026-11-26".into();
        let e = ed.build("me@bell.net").expect("an event");
        assert!(e.all_day && e.time_zone.is_empty());
        assert_eq!(e.end - e.start, 2 * 86_400);
        let reopened = EventEditor::edit(3, AccountId(1), &e);
        assert_eq!((reopened.start_date.as_str(), reopened.end_date.as_str()), ("2026-11-25", "2026-11-26"));
        assert_eq!(reopened.snapshot(), reopened.initial);
    }
}
