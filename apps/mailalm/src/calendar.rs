//! The calendar page (PLAN.md, sections 7 and 12): day, work week, week, month and list views of
//! every account's calendars, each in its colours and each shown or hidden; the details of the
//! event clicked; and, on demand, the time taken in every calendar of every account drawn over the
//! days (A3).
//!
//! Click an event: its details. Double-click it: its window. Double-click an empty time: a new
//! event there. A link of an event shows its domain; it opens in the browser only on a click.

use std::collections::{BTreeMap, HashSet};

use egui::{Align2, Button, Color32, FontId, Frame, Id, Margin, Painter, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use mailalm_engine::PimCommand;
use mailalm_model::AccountId;
use mailalm_model::pim::{BusyBlock, BusyStatus, Event as CalendarEvent, Freq, Recurrence, Response, Until, occurrences};
use mailalm_model::query::{civil_from_days, days_from_civil};

use crate::agenda::{AccountPim, Ctx, Pim, calendar_color, link_domain, open_link, sources};
use crate::i18n::{tr, trf};
use crate::icons::{self, Icon};
use crate::theme as t;
use crate::time;
use crate::zones::TzRules;

/// An hour's height in the day and week views; the time column's width; the day header's height;
/// an all-day line's height, and the most such lines shown.
const HOUR_H: f32 = 44.0;
const TIME_W: f32 = 56.0;
const HEAD_H: f32 = 40.0;
const ALL_DAY_H: f32 = 20.0;
const MAX_ALL_DAY: usize = 4;
/// How many days the list view shows.
const LIST_DAYS: i64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Day,
    WorkWeek,
    Week,
    Month,
    List,
}

impl Mode {
    const ALL: [Mode; 5] = [Mode::Day, Mode::WorkWeek, Mode::Week, Mode::Month, Mode::List];

    fn label(self) -> &'static str {
        match self {
            Mode::Day => tr("Day"),
            Mode::WorkWeek => tr("Work week"),
            Mode::Week => tr("Week"),
            Mode::Month => tr("Month"),
            Mode::List => tr("List"),
        }
    }
}

/// The event whose details are shown: its account, calendar, identity and occurrence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    pub account: AccountId,
    pub calendar: String,
    pub uid: String,
    pub remote: String,
    pub recurrence_id: Option<i64>,
    pub start: i64,
    pub end: i64,
}

/// One occurrence shown.
#[derive(Clone, Debug)]
struct Item {
    account: AccountId,
    /// Its event, in the account's events.
    event: usize,
    start: i64,
    end: i64,
    all_day: bool,
    title: String,
    calendar: String,
    /// Not answered yet, or answered "tentative": drawn lighter.
    tentative: bool,
}

impl Item {
    /// Its title as shown: "(no title)" in the language shown when it has none.
    fn shown_title(&self) -> &str {
        if self.title.is_empty() { tr("(no title)") } else { &self.title }
    }

    /// The local days it covers: `[first, last]`.
    fn days(&self) -> (i64, i64) {
        if self.all_day {
            let first = self.start.div_euclid(86_400);
            (first, (self.end.div_euclid(86_400) - 1).max(first))
        } else {
            (time::local_day(self.start), time::local_day(self.end.saturating_sub(1).max(self.start)))
        }
    }
}

pub struct CalendarView {
    pub mode: Mode,
    /// The local day the view shows (or starts its week or month from).
    pub day: i64,
    hidden: HashSet<(AccountId, String)>,
    hidden_rev: u64,
    /// The time taken in every calendar is drawn over the days (A3).
    pub overlay: bool,
    selected: Option<Selected>,
    confirm_delete: bool,
    /// Ranges asked of the workers already.
    asked: HashSet<(AccountId, i64, i64)>,
    items: Vec<Item>,
    items_key: Option<(u64, i64, i64, u64)>,
    busy: Vec<BusyBlock>,
    busy_key: Option<(u64, i64, i64)>,
    scrolled: bool,
}

impl Default for CalendarView {
    fn default() -> Self {
        CalendarView {
            mode: Mode::Week,
            day: time::local_day(time::now()),
            hidden: HashSet::new(),
            hidden_rev: 0,
            overlay: false,
            selected: None,
            confirm_delete: false,
            asked: HashSet::new(),
            items: Vec::new(),
            items_key: None,
            busy: Vec::new(),
            busy_key: None,
            scrolled: false,
        }
    }
}

/// What the user asked for on the page, done once the page is drawn.
enum Action {
    Select(Option<Selected>),
    New { start: i64, all_day: bool },
    Edit(Selected),
    Delete(Selected),
    Respond(Selected, Response),
    Open(String),
}

fn monday(day: i64) -> i64 {
    day - time::weekday_index(day)
}

fn first_of_month(day: i64) -> i64 {
    let (y, m, _) = civil_from_days(day);
    days_from_civil(y, m, 1)
}

/// A month later or earlier (`step` months), on its first day.
fn add_months(day: i64, step: i64) -> i64 {
    let (y, m, _) = civil_from_days(day);
    let total = y * 12 + (m - 1) + step;
    days_from_civil(total.div_euclid(12), total.rem_euclid(12) + 1, 1)
}

/// A colour made see-through.
fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// How a rule repeats, in words.
pub fn repeat_text(r: &Recurrence) -> String {
    let n = r.interval;
    let mut out = match (n > 1, r.freq) {
        (true, Freq::Daily) => trf!("Repeats every {count} days", count = n),
        (true, Freq::Weekly) => trf!("Repeats every {count} weeks", count = n),
        (true, Freq::Monthly) => trf!("Repeats every {count} months", count = n),
        (true, Freq::Yearly) => trf!("Repeats every {count} years", count = n),
        (false, Freq::Daily) => tr("Repeats daily").to_owned(),
        (false, Freq::Weekly) => tr("Repeats weekly").to_owned(),
        (false, Freq::Monthly) => tr("Repeats monthly").to_owned(),
        (false, Freq::Yearly) => tr("Repeats yearly").to_owned(),
    };
    if !r.by_day.is_empty() {
        let days: Vec<&str> = r.by_day.iter().map(|(_, w)| w.code()).collect();
        out.push_str(&format!(" ({})", days.join(", ")));
    }
    match (r.count, r.until) {
        (Some(c), _) => trf!("{rule}, {count} times", rule = out, count = c),
        (None, Some(Until::Date(d))) => trf!("{rule}, until {date}", rule = out, date = time::date(d)),
        (None, Some(Until::At(at) | Until::Local(at))) => trf!("{rule}, until {date}", rule = out, date = time::date(at.div_euclid(86_400))),
        (None, None) => out,
    }
}

pub fn busy_label(b: BusyStatus) -> &'static str {
    match b {
        BusyStatus::Free => tr("Free"),
        BusyStatus::Tentative => tr("Tentative"),
        BusyStatus::Busy => tr("Busy"),
        BusyStatus::OutOfOffice => tr("Out of office"),
    }
}

pub fn response_label(r: Response) -> &'static str {
    match r {
        Response::NeedsAction => tr("not answered"),
        Response::Accepted => tr("accepted"),
        Response::Tentative => tr("tentative"),
        Response::Declined => tr("declined"),
    }
}

/// The colour of time taken, by how it is taken.
pub fn busy_color(b: BusyStatus) -> Color32 {
    match b {
        BusyStatus::Free => t::faint(),
        BusyStatus::Tentative => t::warn(),
        BusyStatus::Busy => t::danger(),
        BusyStatus::OutOfOffice => t::away(),
    }
}

/// The event an occurrence or a selection names.
fn find<'a>(data: &'a BTreeMap<AccountId, AccountPim>, sel: &Selected) -> Option<(&'a AccountPim, &'a CalendarEvent)> {
    let d = data.get(&sel.account)?;
    let e = d.events.iter().find(|e| {
        if sel.remote.is_empty() { e.remote.id.is_empty() && e.uid == sel.uid && e.recurrence_id == sel.recurrence_id } else { e.remote.id == sel.remote && e.calendar == sel.calendar }
    })?;
    Some((d, e))
}

impl CalendarView {
    /// The first local day shown and how many.
    fn days(&self) -> (i64, i64) {
        match self.mode {
            Mode::Day => (self.day, 1),
            Mode::WorkWeek => (monday(self.day), 5),
            Mode::Week => (monday(self.day), 7),
            Mode::Month => (monday(first_of_month(self.day)), 42),
            Mode::List => (self.day, LIST_DAYS),
        }
    }

    fn step(&mut self, forward: bool) {
        let s = if forward { 1 } else { -1 };
        self.day = match self.mode {
            Mode::Day => self.day + s,
            Mode::WorkWeek | Mode::Week => self.day + 7 * s,
            Mode::Month => add_months(self.day, s),
            Mode::List => self.day + 30 * s,
        };
    }

    fn title(&self) -> String {
        let (first, n) = self.days();
        match self.mode {
            Mode::Day => {
                let (y, m, d) = civil_from_days(self.day);
                trf!("{weekday}, {month} {day}, {year}", weekday = time::day_name(self.day), month = time::month_name(m), day = d, year = y)
            }
            Mode::WorkWeek | Mode::Week => {
                let last = first + n - 1;
                let ((y1, m1, d1), (y2, m2, d2)) = (civil_from_days(first), civil_from_days(last));
                if m1 == m2 {
                    trf!("{month} {first} – {last}, {year}", month = time::month_short(m1), first = d1, last = d2, year = y2)
                } else if y1 == y2 {
                    trf!("{month1} {first} – {month2} {last}, {year}", month1 = time::month_short(m1), first = d1, month2 = time::month_short(m2), last = d2, year = y2)
                } else {
                    trf!("{month1} {first}, {year1} – {month2} {last}, {year2}", month1 = time::month_short(m1), first = d1, year1 = y1, month2 = time::month_short(m2), last = d2, year2 = y2)
                }
            }
            Mode::Month => {
                let (y, m, _) = civil_from_days(self.day);
                format!("{} {y}", time::month_name(m))
            }
            Mode::List => trf!("From {weekday} {date}", weekday = time::day_short(self.day), date = time::date(self.day)),
        }
    }

    /// Asks the workers once for the events of the days shown (those outside what they keep are
    /// read from the provider).
    fn ask(&mut self, cx: &Ctx<'_>) {
        let (first, n) = self.days();
        let (from, to) = (time::day_start(first - 7), time::day_start(first + n + 7));
        for a in cx.ws.accounts() {
            if a.provider.has_server_calendar() && self.asked.insert((a.id, first, n)) {
                cx.send(a.id, PimCommand::Range { from, to });
            }
        }
    }

    /// The occurrences of the days shown, and the time taken there, worked out again when the
    /// calendars or the days change.
    fn refresh(&mut self, data: &BTreeMap<AccountId, AccountPim>, cx: &Ctx<'_>, generation: u64) {
        let (first, n) = self.days();
        let (from, to) = (time::day_start(first), time::day_start(first + n));
        let key = (generation, first, n, self.hidden_rev);
        if self.items_key != Some(key) {
            let mut items = Vec::new();
            for a in cx.ws.accounts() {
                let Some(d) = data.get(&a.id) else { continue };
                for o in occurrences(&d.events, from - 2 * 86_400, to + 2 * 86_400, &TzRules) {
                    let Some(e) = d.events.get(o.event) else { continue };
                    if self.hidden.contains(&(a.id, e.calendar.clone())) {
                        continue;
                    }
                    let item = Item {
                        account: a.id,
                        event: o.event,
                        start: o.start,
                        end: o.end,
                        all_day: e.all_day,
                        title: e.title.clone(),
                        calendar: e.calendar.clone(),
                        tentative: matches!(e.my_response, Some(Response::NeedsAction | Response::Tentative)) || e.busy == BusyStatus::Tentative,
                    };
                    let (a_day, b_day) = item.days();
                    if a_day < first + n && b_day >= first {
                        items.push(item);
                    }
                }
            }
            items.sort_by_key(|i| (!i.all_day, i.start, i.end));
            self.items = items;
            self.items_key = Some(key);
        }
        if self.overlay && self.busy_key != Some((generation, from, to)) {
            self.busy = mailalm_engine::agenda::busy(&sources(data, cx.ws), from, to, &TzRules);
            self.busy_key = Some((generation, from, to));
        }
    }

    fn selection(&self, it: &Item, data: &BTreeMap<AccountId, AccountPim>) -> Option<Selected> {
        let e = data.get(&it.account)?.events.get(it.event)?;
        Some(Selected { account: it.account, calendar: e.calendar.clone(), uid: e.uid.clone(), remote: e.remote.id.clone(), recurrence_id: e.recurrence_id, start: it.start, end: it.end })
    }

    fn is_selected(&self, it: &Item, data: &BTreeMap<AccountId, AccountPim>) -> bool {
        self.selected.as_ref().is_some_and(|s| self.selection(it, data).is_some_and(|x| x == *s))
    }

    fn color(&self, it: &Item, data: &BTreeMap<AccountId, AccountPim>, cx: &Ctx<'_>) -> (Color32, Color32) {
        let cal = data.get(&it.account).and_then(|d| d.calendar(&it.calendar));
        (cx.color(it.account), calendar_color(cx, it.account, cal))
    }
}

/// An event's block: its colours, title and time, outlined when selected.
fn chip(p: &Painter, rect: Rect, colors: (Color32, Color32), title: &str, sub: &str, selected: bool, tentative: bool) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    p.rect_filled(rect, 4.0, alpha(colors.0, if tentative { 40 } else { 90 }));
    p.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 1.0, colors.1);
    if selected {
        p.rect_stroke(rect, 4.0, Stroke::new(1.5, t::text()), StrokeKind::Inside);
    }
    p.text(pos2(rect.left() + 7.0, rect.top() + 3.0), Align2::LEFT_TOP, title, FontId::proportional(12.5), t::text());
    if !sub.is_empty() && rect.height() >= 34.0 {
        p.text(pos2(rect.left() + 7.0, rect.top() + 19.0), Align2::LEFT_TOP, sub, FontId::proportional(11.0), t::muted());
    }
}

impl Pim {
    /// The calendar's commands, in the command bar: new event or meeting, today, previous, next,
    /// the period shown, the views, and the availability of every calendar.
    pub fn calendar_commands(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        let mut new: Option<bool> = None;
        if ui.add(Button::new(RichText::new(tr("New event")).strong().color(t::on_accent())).fill(t::accent())).clicked() {
            new = Some(false);
        }
        if ui.button(tr("New meeting")).clicked() {
            new = Some(true);
        }
        ui.separator();
        let v = &mut self.calendar;
        if ui.button(tr("Today")).clicked() {
            v.day = time::local_day(time::now());
        }
        if icons::button(ui, Icon::ChevronLeft, vec2(26.0, 24.0), t::text(), t::raised(), tr("Previous")).clicked() {
            v.step(false);
        }
        if icons::button(ui, Icon::ChevronRight, vec2(26.0, 24.0), t::text(), t::raised(), tr("Next")).clicked() {
            v.step(true);
        }
        ui.label(RichText::new(v.title()).size(15.0).strong());
        ui.separator();
        for m in Mode::ALL {
            if ui.add(Button::selectable(v.mode == m, m.label())).clicked() {
                v.mode = m;
            }
        }
        ui.separator();
        ui.checkbox(&mut v.overlay, tr("Availability, all accounts")).on_hover_text(tr("Draws the time taken in every calendar of every account, hidden ones too, beside each day."));
        let due = self.reminders.due.len();
        if due > 0 && !self.reminders.open && ui.button(trf!("Reminders ({count})", count = due)).clicked() {
            self.show_reminders();
        }
        if let Some(meeting) = new {
            let start = next_half_hour(time::now());
            self.new_event(cx, None, start, false, meeting);
        }
    }

    /// The calendars of every account, in the folder pane: each shown or hidden.
    pub fn calendar_sidebar(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        ui.add_space(6.0);
        ui.label(RichText::new(tr("CALENDARS")).size(11.0).strong().color(t::faint()));
        for a in cx.ws.accounts() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                ui.painter().rect_filled(r, 1.0, t::account(a.color));
                ui.label(RichText::new(&a.name).strong());
            });
            let calendars = self.data.get(&a.id).map(|d| d.calendars.clone()).unwrap_or_default();
            if calendars.is_empty() {
                ui.label(RichText::new(tr("   Waiting for the calendar…")).size(12.0).color(t::faint()));
            }
            for c in calendars {
                let key = (a.id, c.id.clone());
                let mut shown = !self.calendar.hidden.contains(&key);
                let color = calendar_color(cx, a.id, Some(&c));
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, color);
                    if ui.checkbox(&mut shown, c.name.as_str()).changed() {
                        if shown {
                            self.calendar.hidden.remove(&key);
                        } else {
                            self.calendar.hidden.insert(key.clone());
                        }
                        self.calendar.hidden_rev += 1;
                    }
                });
            }
        }
        ui.add_space(12.0);
        ui.separator();
        ui.label(RichText::new(tr("IMAP accounts keep their calendar on this computer; invitations are sent and answered by mail.")).size(11.0).color(t::faint()));
    }

    /// The calendar page: the view chosen, and the details of the event selected.
    pub fn calendar_page(&mut self, ui: &mut Ui, cx: &Ctx<'_>) {
        self.calendar.ask(cx);
        self.calendar.refresh(&self.data, cx, self.generation);
        let mut actions: Vec<Action> = Vec::new();
        egui::CentralPanel::default().frame(Frame::new().fill(t::panel()).inner_margin(Margin::symmetric(10, 8))).show(ui, |ui| {
            if let Some(sel) = self.calendar.selected.clone() {
                egui::Panel::right("mailalm-calendar-details")
                    .resizable(true)
                    .default_size(320.0)
                    .size_range(240.0..=520.0)
                    .frame(Frame::new().fill(t::panel2()).inner_margin(Margin::symmetric(14, 12)))
                    .show(ui, |ui| details(ui, &self.data, cx, &sel, &mut self.calendar.confirm_delete, &mut actions));
            }
            let (first, n) = self.calendar.days();
            match self.calendar.mode {
                Mode::Day | Mode::WorkWeek | Mode::Week => grid(ui, &mut self.calendar, &self.data, cx, first, n, &mut actions),
                Mode::Month => month(ui, &mut self.calendar, &self.data, cx, first, &mut actions),
                Mode::List => list(ui, &self.calendar, &self.data, cx, &mut actions),
            }
        });
        for a in actions {
            self.act(a, cx);
        }
    }

    fn act(&mut self, a: Action, cx: &Ctx<'_>) {
        match a {
            Action::Select(s) => {
                self.calendar.selected = s;
                self.calendar.confirm_delete = false;
            }
            Action::New { start, all_day } => self.new_event(cx, None, start, all_day, false),
            Action::Edit(sel) => {
                if let Some((_, e)) = find(&self.data, &sel) {
                    let e = e.clone();
                    self.edit_event(cx, sel.account, &e);
                }
            }
            Action::Delete(sel) => {
                let Some((_, e)) = find(&self.data, &sel) else { return };
                let notify = e.is_organizer && e.is_meeting();
                if cx.send(sel.account, PimCommand::DeleteEvent { event: Box::new(e.clone()), notify }) {
                    self.say(if notify { tr("The event was deleted; its guests are told.") } else { tr("The event was deleted.") });
                }
                self.calendar.selected = None;
                self.calendar.confirm_delete = false;
            }
            Action::Respond(sel, answer) => {
                let Some((_, e)) = find(&self.data, &sel) else { return };
                let event = Box::new(e.clone());
                if cx.send(sel.account, PimCommand::Respond { event, answer, send: true, comment: String::new(), proposal: None }) {
                    self.say(trf!("Your answer ({response}) is being sent to the organizer.", response = response_label(answer)));
                }
            }
            Action::Open(url) => {
                if let Err(e) = open_link(&url) {
                    self.warn(e);
                }
            }
        }
    }
}

/// The next half hour from now, for a new event.
pub fn next_half_hour(now: i64) -> i64 {
    let local = time::local(now);
    let rounded = (local.div_euclid(1800) + 1) * 1800;
    time::utc_of_local(rounded)
}

/// The day, work week and week views: a header of days, a band of all-day events, then the hours.
fn grid(ui: &mut Ui, view: &mut CalendarView, data: &BTreeMap<AccountId, AccountPim>, cx: &Ctx<'_>, first: i64, n: i64, actions: &mut Vec<Action>) {
    let width = ui.available_width();
    let cols = n.max(1) as f32;
    let col_w = ((width - TIME_W) / cols).max(30.0);
    let today = time::local_day(time::now());
    let mut goto_day: Option<i64> = None;

    // The days.
    let (head, _) = ui.allocate_exact_size(vec2(width, HEAD_H), Sense::hover());
    for i in 0..n {
        let d = first + i;
        let x = head.left() + TIME_W + i as f32 * col_w;
        let (_, m, dd) = civil_from_days(d);
        let color = if d == today { t::accent_text() } else { t::muted() };
        let p = ui.painter();
        p.text(pos2(x + 6.0, head.top() + 11.0), Align2::LEFT_CENTER, time::day_short(d), FontId::proportional(11.5), color);
        p.text(pos2(x + 6.0, head.top() + 28.0), Align2::LEFT_CENTER, format!("{dd} {}", time::month_short(m)), FontId::proportional(15.0), if d == today { t::accent_text() } else { t::text() });
        let r = Rect::from_min_size(pos2(x, head.top()), vec2(col_w, HEAD_H));
        if ui.interact(r, Id::unique(("mailalm-calendar-head", d)), Sense::click()).on_hover_text(tr("Show this day")).clicked() {
            goto_day = Some(d);
        }
    }

    // All-day events, a few lines per day.
    let per_day: Vec<Vec<usize>> = (0..n)
        .map(|i| {
            let d = first + i;
            view.items.iter().enumerate().filter(|(_, it)| it.all_day && it.days().0 <= d && d <= it.days().1).map(|(k, _)| k).take(MAX_ALL_DAY).collect()
        })
        .collect();
    let rows = per_day.iter().map(Vec::len).max().unwrap_or(0);
    if rows > 0 {
        let (band, _) = ui.allocate_exact_size(vec2(width, rows as f32 * ALL_DAY_H + 4.0), Sense::hover());
        ui.painter().text(pos2(band.left() + 4.0, band.top() + 10.0), Align2::LEFT_CENTER, tr("all day"), FontId::proportional(10.5), t::faint());
        for (i, list) in per_day.iter().enumerate() {
            for (row, k) in list.iter().enumerate() {
                let Some(it) = view.items.get(*k) else { continue };
                let rect = Rect::from_min_size(pos2(band.left() + TIME_W + i as f32 * col_w + 1.0, band.top() + 2.0 + row as f32 * ALL_DAY_H), vec2(col_w - 2.0, ALL_DAY_H - 2.0));
                let response = ui.interact(rect, Id::unique(("mailalm-calendar-allday", it.account.0, it.event, it.start, i)), Sense::click());
                chip(ui.painter(), rect, view.color(it, data, cx), it.shown_title(), "", view.is_selected(it, data), it.tentative);
                item_clicks(&response, view, it, data, actions);
            }
        }
    }

    // The hours.
    let mut area = ScrollArea::vertical().id_salt("mailalm-calendar-grid").auto_shrink([false, false]);
    if !view.scrolled {
        area = area.vertical_scroll_offset(7.0 * HOUR_H);
        view.scrolled = true;
    }
    area.show(ui, |ui| {
        let (rect, bg) = ui.allocate_exact_size(vec2(width, 24.0 * HOUR_H), Sense::click());
        let p = ui.painter_at(rect);
        for h in 0..24 {
            let y = rect.top() + h as f32 * HOUR_H;
            p.line_segment([pos2(rect.left() + TIME_W, y), pos2(rect.right(), y)], Stroke::new(1.0, t::line()));
            p.text(pos2(rect.left() + TIME_W - 8.0, y + 2.0), Align2::RIGHT_TOP, format!("{h:02}:00"), FontId::proportional(11.0), t::faint());
        }
        for i in 0..=n {
            let x = rect.left() + TIME_W + i as f32 * col_w;
            p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, t::line()));
        }
        let now = time::now();
        // y of an instant in a day's column.
        let y_of = |d: i64, at: i64| rect.top() + ((time::local(at) - d * 86_400) as f32 / 3600.0).clamp(0.0, 24.0) * HOUR_H;
        for i in 0..n {
            let d = first + i;
            let x = rect.left() + TIME_W + i as f32 * col_w;
            if d == today {
                p.rect_filled(Rect::from_min_size(pos2(x, rect.top()), vec2(col_w, rect.height())), 0.0, alpha(t::accent(), 12));
                let y = y_of(d, now);
                p.line_segment([pos2(x, y), pos2(x + col_w, y)], Stroke::new(2.0, t::danger()));
            }
            // The time taken in every calendar, beside the day (A3).
            if view.overlay {
                let (ds, de) = (time::day_start(d), time::day_start(d + 1));
                for b in view.busy.iter().filter(|b| b.start < de && b.end > ds) {
                    let (y0, y1) = (y_of(d, b.start.max(ds)), y_of(d, b.end.min(de)));
                    let r = Rect::from_min_max(pos2(x + 1.0, y0), pos2(x + col_w - 1.0, y1.max(y0 + 2.0)));
                    p.rect_filled(r, 0.0, alpha(busy_color(b.status), 22));
                    p.rect_filled(Rect::from_min_max(pos2(x + col_w - 6.0, y0), pos2(x + col_w - 1.0, y1.max(y0 + 2.0))), 1.0, busy_color(b.status));
                }
            }
        }
        if bg.clicked() {
            actions.push(Action::Select(None));
        }
        if bg.double_clicked()
            && let Some(pos) = bg.interact_pointer_pos()
        {
            let col = ((pos.x - rect.left() - TIME_W) / col_w).floor();
            if col >= 0.0 && col < cols {
                let d = first + col as i64;
                let half_hours = ((pos.y - rect.top()) / (HOUR_H / 2.0)).floor().clamp(0.0, 47.0) as i64;
                actions.push(Action::New { start: time::utc_of_local(d * 86_400 + half_hours * 1800), all_day: false });
            }
        }
        // Timed events, side by side where they overlap.
        for i in 0..n {
            let d = first + i;
            let x = rect.left() + TIME_W + i as f32 * col_w;
            let (ds, de) = (time::day_start(d), time::day_start(d + 1));
            let mut segs: Vec<(usize, f32, f32)> = view
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| !it.all_day && it.start < de && it.end.max(it.start + 1) > ds)
                .map(|(k, it)| {
                    let y0 = y_of(d, it.start.max(ds));
                    (k, y0, y_of(d, it.end.min(de)).max(y0 + 18.0))
                })
                .collect();
            segs.sort_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)));
            let lanes = lanes(&segs);
            for (n_seg, (k, y0, y1)) in segs.iter().enumerate() {
                let Some(it) = view.items.get(*k) else { continue };
                let (lane, count) = lanes.get(n_seg).copied().unwrap_or((0, 1));
                let w = (col_w - 4.0) / count.max(1) as f32;
                let r = Rect::from_min_max(pos2(x + 2.0 + lane as f32 * w, *y0 + 1.0), pos2(x + 2.0 + (lane + 1) as f32 * w - 1.0, *y1 - 1.0));
                let response = ui.interact(r, Id::unique(("mailalm-calendar-item", it.account.0, it.event, it.start, d)), Sense::click());
                let sub = format!("{} – {}", time::clock(it.start), time::clock(it.end));
                chip(&p, r, view.color(it, data, cx), it.shown_title(), &sub, view.is_selected(it, data), it.tentative);
                item_clicks(&response, view, it, data, actions);
            }
        }
    });
    if let Some(d) = goto_day {
        view.mode = Mode::Day;
        view.day = d;
    }
}

/// Each block's lane and the number of lanes of the overlapping group it is in, for blocks sorted
/// by their top.
fn lanes(segs: &[(usize, f32, f32)]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(segs.len());
    let mut ends: Vec<f32> = Vec::new();
    let mut group_start = 0;
    let mut group_end = f32::MIN;
    for (k, (_, y0, y1)) in segs.iter().enumerate() {
        if *y0 >= group_end {
            let count = ends.len().max(1);
            for o in out.iter_mut().skip(group_start) {
                o.1 = count;
            }
            ends.clear();
            group_start = k;
            group_end = *y1;
        } else {
            group_end = group_end.max(*y1);
        }
        let lane = match ends.iter().position(|e| *e <= *y0) {
            Some(l) => {
                if let Some(e) = ends.get_mut(l) {
                    *e = *y1;
                }
                l
            }
            None => {
                ends.push(*y1);
                ends.len() - 1
            }
        };
        out.push((lane, 1));
    }
    let count = ends.len().max(1);
    for o in out.iter_mut().skip(group_start) {
        o.1 = count;
    }
    out
}

/// A click selects an event; a double click opens its window.
fn item_clicks(response: &egui::Response, view: &CalendarView, it: &Item, data: &BTreeMap<AccountId, AccountPim>, actions: &mut Vec<Action>) {
    if response.double_clicked() {
        if let Some(s) = view.selection(it, data) {
            actions.push(Action::Edit(s));
        }
    } else if response.clicked() {
        actions.push(Action::Select(view.selection(it, data)));
    }
}

/// The month view: six weeks, each day with the events it has.
fn month(ui: &mut Ui, view: &mut CalendarView, data: &BTreeMap<AccountId, AccountPim>, cx: &Ctx<'_>, first: i64, actions: &mut Vec<Action>) {
    let width = ui.available_width();
    let col_w = width / 7.0;
    let (_, this_month, _) = civil_from_days(view.day);
    let today = time::local_day(time::now());
    let (head, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    for i in 0..7 {
        ui.painter().text(pos2(head.left() + i as f32 * col_w + 6.0, head.center().y), Align2::LEFT_CENTER, time::day_short(first + i), FontId::proportional(11.5), t::muted());
    }
    let height = ui.available_height().max(6.0 * 72.0);
    let row_h = height / 6.0;
    let (rect, bg) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let p = ui.painter_at(rect);
    let lines = ((row_h - 24.0) / 16.0).floor().max(1.0) as usize;
    let mut goto_day: Option<i64> = None;
    for w in 0..6 {
        for i in 0..7 {
            let d = first + w * 7 + i;
            let cell = Rect::from_min_size(pos2(rect.left() + i as f32 * col_w, rect.top() + w as f32 * row_h), vec2(col_w, row_h));
            let (_, m, dd) = civil_from_days(d);
            if m != this_month {
                p.rect_filled(cell, 0.0, t::panel2());
            }
            p.rect_stroke(cell, 0.0, Stroke::new(1.0, t::line()), StrokeKind::Inside);
            let num_color = if d == today { t::accent_text() } else if m == this_month { t::text() } else { t::faint() };
            let num = p.text(pos2(cell.left() + 6.0, cell.top() + 10.0), Align2::LEFT_CENTER, dd.to_string(), FontId::proportional(12.5), num_color);
            if ui.interact(num.expand(3.0), Id::unique(("mailalm-calendar-month-day", d)), Sense::click()).on_hover_text(tr("Show this day")).clicked() {
                goto_day = Some(d);
            }
            let here: Vec<usize> = view.items.iter().enumerate().filter(|(_, it)| it.days().0 <= d && d <= it.days().1).map(|(k, _)| k).collect();
            for (row, k) in here.iter().take(lines).enumerate() {
                let Some(it) = view.items.get(*k) else { continue };
                let line = Rect::from_min_size(pos2(cell.left() + 3.0, cell.top() + 22.0 + row as f32 * 16.0), vec2(col_w - 6.0, 15.0));
                let response = ui.interact(line, Id::unique(("mailalm-calendar-month-item", it.account.0, it.event, it.start, d)), Sense::click());
                let colors = view.color(it, data, cx);
                let lp = p.with_clip_rect(line);
                if view.is_selected(it, data) || response.hovered() {
                    lp.rect_filled(line, 3.0, t::raised());
                }
                lp.circle_filled(pos2(line.left() + 4.0, line.center().y), 3.5, colors.1);
                let label = if it.all_day { it.shown_title().to_owned() } else { format!("{} {}", time::clock(it.start), it.shown_title()) };
                lp.text(pos2(line.left() + 11.0, line.center().y), Align2::LEFT_CENTER, label, FontId::proportional(11.5), if it.tentative { t::muted() } else { t::text() });
                item_clicks(&response, view, it, data, actions);
            }
            if here.len() > lines {
                let more = p.text(pos2(cell.left() + 8.0, cell.bottom() - 9.0), Align2::LEFT_CENTER, trf!("+{count} more", count = here.len() - lines), FontId::proportional(11.0), t::accent_text());
                if ui.interact(more.expand(2.0), Id::unique(("mailalm-calendar-month-more", d)), Sense::click()).clicked() {
                    goto_day = Some(d);
                }
            }
        }
    }
    if bg.clicked() {
        actions.push(Action::Select(None));
    }
    if bg.double_clicked()
        && let Some(pos) = bg.interact_pointer_pos()
    {
        let (i, w) = (((pos.x - rect.left()) / col_w).floor().clamp(0.0, 6.0) as i64, ((pos.y - rect.top()) / row_h).floor().clamp(0.0, 5.0) as i64);
        actions.push(Action::New { start: time::utc_of_local((first + w * 7 + i) * 86_400 + 9 * 3600), all_day: false });
    }
    if let Some(d) = goto_day {
        view.mode = Mode::Day;
        view.day = d;
    }
}

/// The list view: the coming days' events, day by day.
fn list(ui: &mut Ui, view: &CalendarView, data: &BTreeMap<AccountId, AccountPim>, cx: &Ctx<'_>, actions: &mut Vec<Action>) {
    ScrollArea::vertical().id_salt("mailalm-calendar-list").auto_shrink([false, false]).show(ui, |ui| {
        let (first, n) = view.days();
        let mut any = false;
        for d in first..first + n {
            let here: Vec<&Item> = view.items.iter().filter(|it| it.days().0 <= d && d <= it.days().1).collect();
            if here.is_empty() {
                continue;
            }
            any = true;
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{} {}", time::day_name(d), time::date(d))).strong().color(t::accent_text()));
            for it in here {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                let p = ui.painter_at(rect);
                if view.is_selected(it, data) {
                    p.rect_filled(rect, 4.0, t::sel());
                } else if response.hovered() {
                    p.rect_filled(rect, 4.0, t::raised());
                }
                let when = if it.all_day { tr("all day").to_owned() } else { format!("{} – {}", time::clock(it.start), time::clock(it.end)) };
                p.text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, when, FontId::proportional(12.0), t::muted());
                let colors = view.color(it, data, cx);
                p.circle_filled(pos2(rect.left() + 118.0, rect.center().y), 4.0, colors.1);
                let title = p.text(pos2(rect.left() + 130.0, rect.center().y), Align2::LEFT_CENTER, it.shown_title(), FontId::proportional(13.0), t::text());
                p.text(pos2(title.right() + 10.0, rect.center().y), Align2::LEFT_CENTER, cx.name(it.account), FontId::proportional(11.5), t::faint());
                item_clicks(&response, view, it, data, actions);
            }
        }
        if !any {
            ui.add_space(30.0);
            ui.label(RichText::new(trf!("Nothing in the {days} days from {date}.", days = LIST_DAYS, date = time::date(first))).color(t::faint()));
        }
    });
}

/// The details of the event selected: when, where, who, its link (by its domain), and what can be
/// done with it.
fn details(ui: &mut Ui, data: &BTreeMap<AccountId, AccountPim>, cx: &Ctx<'_>, sel: &Selected, confirm_delete: &mut bool, actions: &mut Vec<Action>) {
    let Some((d, e)) = find(data, sel) else {
        ui.label(RichText::new(tr("This event is no longer in the calendar.")).color(t::faint()));
        if ui.button(tr("Close")).clicked() {
            actions.push(Action::Select(None));
        }
        return;
    };
    let cal = d.calendar(&e.calendar);
    ScrollArea::vertical().id_salt("mailalm-calendar-details-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(if e.title.is_empty() { tr("(no title)") } else { e.title.as_str() }).size(17.0).strong()).wrap());
        });
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, calendar_color(cx, sel.account, cal));
            ui.label(RichText::new(format!("{} · {}", cx.name(sel.account), cal.map_or(tr("Calendar"), |c| c.name.as_str()))).size(12.0).color(t::muted()));
        });
        ui.add_space(6.0);
        ui.label(time::span(sel.start, sel.end, e.all_day));
        if let Some(r) = &e.recurrence {
            ui.label(RichText::new(repeat_text(r)).size(12.0).color(t::muted()));
        } else if e.recurrence_id.is_some() {
            ui.label(RichText::new(tr("One occurrence of a series")).size(12.0).color(t::muted()));
        }
        if !e.location.is_empty() {
            ui.label(trf!("Where: {location}", location = e.location));
        }
        if let Some(o) = &e.online {
            match link_domain(&o.url) {
                Some(domain) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(trf!("Online meeting at {domain}", domain = domain));
                        if ui.button(tr("Open in the browser")).on_hover_text(o.url.as_str()).clicked() {
                            actions.push(Action::Open(o.url.clone()));
                        }
                    });
                }
                None => {
                    ui.label(RichText::new(tr("Online meeting: the link is being made by the provider.")).size(12.0).color(t::faint()));
                }
            }
        }
        ui.label(RichText::new(trf!("Shows as: {status}", status = busy_label(e.busy))).size(12.0).color(t::muted()));
        if let Some(o) = &e.organizer {
            ui.add_space(6.0);
            ui.label(RichText::new(trf!("Organizer: {organizer}", organizer = if o.name.is_empty() { o.email.clone() } else { format!("{} <{}>", o.name, o.email) })).size(12.5));
        }
        if !e.attendees.is_empty() {
            ui.label(RichText::new(trf!("{count} guests", count = e.attendees.len())).size(12.0).color(t::faint()));
            for a in e.attendees.iter().take(30) {
                let who = if a.name.is_empty() { a.email.clone() } else { format!("{} <{}>", a.name, a.email) };
                ui.label(RichText::new(format!("• {who} — {}", response_label(a.response))).size(12.0));
            }
            if e.attendees.len() > 30 {
                ui.label(RichText::new(trf!("… and {count} more", count = e.attendees.len() - 30)).size(12.0).color(t::faint()));
            }
        }
        let can_edit = cal.is_some_and(|c| c.can_edit);
        if let Some(mine) = e.my_response.filter(|_| !e.is_organizer && can_edit) {
            ui.add_space(8.0);
            ui.label(RichText::new(trf!("Your answer: {response}", response = response_label(mine))).strong());
            ui.horizontal(|ui| {
                for (label, answer) in [(tr("Accept"), Response::Accepted), (tr("Tentative"), Response::Tentative), (tr("Decline"), Response::Declined)] {
                    if ui.add_enabled(mine != answer, Button::new(label)).on_hover_text(tr("Answer, and tell the organizer")).clicked() {
                        actions.push(Action::Respond(sel.clone(), answer));
                    }
                }
            });
        }
        if !e.description.trim().is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.add(egui::Label::new(RichText::new(&e.description).size(13.0)).selectable(true).wrap());
        }
        ui.add_space(10.0);
        ui.separator();
        ui.horizontal(|ui| {
            if can_edit && (e.is_organizer || !e.is_meeting()) && ui.button(tr("Edit…")).clicked() {
                actions.push(Action::Edit(sel.clone()));
            }
            if can_edit && ui.button(tr("Delete…")).clicked() {
                *confirm_delete = true;
            }
            if ui.button(tr("Close")).clicked() {
                actions.push(Action::Select(None));
            }
        });
        if *confirm_delete {
            Frame::new().fill(t::warn_bg()).corner_radius(6).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                let question = if e.recurrence.is_some() { tr("Delete this event and every occurrence of its series?") } else { tr("Delete this event?") };
                ui.label(RichText::new(question).color(t::warn()));
                if e.is_organizer && e.is_meeting() {
                    ui.label(RichText::new(tr("Its guests are told it is cancelled.")).size(12.0).color(t::warn()));
                }
                ui.horizontal(|ui| {
                    if ui.button(tr("Delete")).clicked() {
                        actions.push(Action::Delete(sel.clone()));
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        *confirm_delete = false;
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_blocks_share_their_width() {
        // a and b overlap; c overlaps b only; d is alone.
        let segs = [(0, 0.0, 40.0), (1, 10.0, 60.0), (2, 45.0, 80.0), (3, 100.0, 120.0)];
        let l = lanes(&segs);
        assert_eq!(l, [(0, 2), (1, 2), (0, 2), (0, 1)]);
    }

    #[test]
    fn months_move_by_their_first_day() {
        let nov25 = days_from_civil(2026, 11, 25);
        assert_eq!(add_months(nov25, 1), days_from_civil(2026, 12, 1));
        assert_eq!(add_months(nov25, 2), days_from_civil(2027, 1, 1));
        assert_eq!(add_months(nov25, -11), days_from_civil(2025, 12, 1));
        assert_eq!(time::weekday_index(monday(nov25)), 0);
        let r = Recurrence { interval: 2, count: Some(5), ..Recurrence::new(Freq::Weekly) };
        assert_eq!(repeat_text(&r), "Repeats every 2 weeks, 5 times");
    }
}
