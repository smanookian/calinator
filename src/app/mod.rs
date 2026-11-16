//! The TUI: state, keys, main loop.

mod accounts;
mod dialogs;
mod draw;
pub mod input;
mod prefs;
pub mod theme;

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Duration as Dur, NaiveDate, NaiveDateTime, Timelike, Utc};
use chrono_tz::Tz;
use crossterm::event::{self, Event as TermEvent, KeyEvent, KeyEventKind, MouseButton, MouseEventKind};
use musinator_themes::Theme;

use crate::accounts::Account;
use crate::daemon::Msg;
use crate::edit::{self, Changes, Scope, Target};
use crate::keys::{Action, Keymap};
use crate::model::{Calendar, Event, Occurrence, Partstat, new_id};
use crate::recur;
use crate::settings::Settings;
use crate::store::Store;
use crate::tz;
pub use dialogs::Dialog;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Day,
    Week,
    Month,
    Agenda,
}

impl View {
    pub fn from_id(s: &str) -> View {
        match s {
            "day" => View::Day,
            "month" => View::Month,
            "agenda" => View::Agenda,
            _ => View::Week,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            View::Day => "day",
            View::Week => "week",
            View::Month => "month",
            View::Agenda => "agenda",
        }
    }

    /// The view to open with: the start setting, or with "last" the view
    /// that was open when Calinator closed.
    fn on_start(settings: &Settings, store: &Store) -> View {
        match settings.start_view.as_str() {
            "last" => View::from_id(&store.meta("last_view").ok().flatten().unwrap_or_default()),
            s => View::from_id(s),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Side,
    Main,
    Details,
}

pub struct App {
    pub store: Store,
    pub events: Vec<Event>,
    pub calendars: Vec<Calendar>,
    pub settings: Settings,
    pub keys: Keymap,
    pub theme: Theme,
    watch: theme::OmarchyWatch,
    pub zone: Tz,
    pub view: View,
    pub focus: Focus,
    /// Cursor day.
    pub day: NaiveDate,
    /// First day of the agenda view.
    pub agenda_from: NaiveDate,
    /// Cursor time in day/week views, minutes after midnight.
    pub slot: u32,
    /// First minute shown in day/week views.
    pub top: u32,
    /// Occurrences around the visible range.
    pub occs: Vec<Occurrence>,
    /// Selected occurrence (index in `occs`).
    pub sel: Option<usize>,
    /// Selected row in the left pane's calendar list.
    pub side_sel: usize,
    pub dialog: Option<Dialog>,
    pub status: Option<(String, Instant)>,
    /// Details pane shown (wide screens).
    pub wide: bool,
    /// Main pane area from the last draw, for mouse clicks.
    pub hit: draw::Hits,
    pub quit: bool,
    /// Messages from the background thread (sync, reminders, subscriptions).
    background: Option<Receiver<Msg>>,
    /// Ask the background thread to sync now.
    sync_now: Option<Sender<()>>,
    /// A sync round is running.
    pub syncing: bool,
    pub accounts: Vec<Account>,
    /// Edits the server replaced (see the conflicts list).
    pub conflicts: usize,
    /// Changes waiting for upload.
    pub pending: usize,
}

impl App {
    pub fn new(store: Store) -> Self {
        let settings = Settings::load(&store);
        let zone = settings.zone();
        let today = tz::local(Utc::now(), zone).date();
        let mut app = App {
            events: store.events().unwrap_or_default(),
            calendars: store.calendars().unwrap_or_default(),
            keys: Keymap::new(&settings.keys),
            theme: theme::load(&settings.theme),
            watch: theme::OmarchyWatch::new(),
            zone,
            view: View::on_start(&settings, &store),
            focus: Focus::Main,
            day: today,
            agenda_from: today,
            slot: (tz::local(Utc::now(), zone).hour() * 60).max(settings.day_start_hour * 60),
            top: settings.day_start_hour * 60,
            occs: Vec::new(),
            sel: None,
            side_sel: 0,
            dialog: None,
            status: None,
            wide: false,
            hit: Default::default(),
            quit: false,
            background: None,
            sync_now: None,
            syncing: false,
            accounts: Vec::new(),
            conflicts: 0,
            pending: 0,
            settings,
            store,
        };
        app.reload();
        app.select_at_cursor();
        app
    }

    pub fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    pub fn today(&self) -> NaiveDate {
        tz::local(self.now(), self.zone).date()
    }

    pub fn slot_minutes(&self) -> u32 {
        60 / self.settings.rows_per_hour.clamp(1, 4) as u32
    }

    pub fn calendar(&self, id: &str) -> Option<&Calendar> {
        self.calendars.iter().find(|c| c.id == id)
    }

    pub fn event(&self, id: &str) -> Option<&Event> {
        self.events.iter().find(|e| e.id == id)
    }

    pub fn selected(&self) -> Option<(&Occurrence, &Event)> {
        let o = self.occs.get(self.sel?)?;
        Some((o, self.event(&o.event_id)?))
    }

    /// First day of the week holding `d`.
    pub fn week_start(&self, d: NaiveDate) -> NaiveDate {
        let from_mon = d.weekday().num_days_from_monday() as i64;
        let back = if self.settings.week_starts_monday { from_mon } else { (from_mon + 1) % 7 };
        d - Dur::days(back)
    }

    /// Days shown by the current view.
    pub fn visible_days(&self) -> (NaiveDate, NaiveDate) {
        match self.view {
            View::Day => (self.day, self.day + Dur::days(1)),
            View::Week => {
                let s = self.week_start(self.day);
                (s, s + Dur::days(7))
            }
            View::Month => {
                let first = self.day.with_day(1).expect("day 1");
                let s = self.week_start(first);
                (s, s + Dur::days(42))
            }
            View::Agenda => (self.agenda_from, self.agenda_from + Dur::days(self.settings.agenda_days.max(1))),
        }
    }

    /// Reloads occurrences for the visible range (and keeps the selection).
    pub fn refresh(&mut self) {
        let keep = self.selected().map(|(o, _)| (o.event_id.clone(), o.start));
        let (a, b) = self.visible_days();
        let from = tz::day_bounds(a, self.zone).0;
        let to = tz::day_bounds(b, self.zone).0;
        let on: Vec<&str> = self.calendars.iter().filter(|c| c.enabled).map(|c| c.id.as_str()).collect();
        let shown: Vec<Event> = self.events.iter().filter(|e| on.contains(&e.calendar.as_str())).cloned().collect();
        self.occs = recur::expand(&shown, from, to, self.zone);
        self.sel = keep.and_then(|(id, st)| self.occs.iter().position(|o| o.event_id == id && o.start == st));
    }

    pub fn reload(&mut self) {
        self.events = self.store.events().unwrap_or_default();
        self.calendars = self.store.calendars().unwrap_or_default();
        self.accounts = self.store.accounts().unwrap_or_default();
        self.conflicts = self.store.conflicts().map(|c| c.len()).unwrap_or(0);
        self.pending = self.store.pending().map(|p| p.iter().map(|(_, n)| *n as usize).sum()).unwrap_or(0);
        self.refresh();
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Occurrences on a day, all-day first.
    pub fn on_day(&self, d: NaiveDate) -> Vec<usize> {
        (0..self.occs.len()).filter(|i| self.occs[*i].on_day(d, self.zone)).collect()
    }

    /// Selects the event under the cursor (day/week: covering the slot; else first of the day).
    pub fn select_at_cursor(&mut self) {
        let ids = self.on_day(self.day);
        self.sel = match self.view {
            View::Day | View::Week => {
                let at = tz::resolve(self.day.and_hms_opt(self.slot / 60, self.slot % 60, 0).expect("slot"), self.zone);
                let end = at + Dur::minutes(self.slot_minutes() as i64);
                ids.into_iter().find(|i| {
                    let o = &self.occs[*i];
                    !o.all_day && o.start < end && (o.end > at || o.start == at)
                })
            }
            _ => ids.first().copied(),
        };
    }

    /// Moves the cursor to a day; reloads when it leaves the visible range.
    pub fn go_to(&mut self, d: NaiveDate) {
        let (a, b) = self.visible_days();
        self.day = d;
        if d < a || d >= b {
            self.agenda_from = d;
            self.refresh();
        }
        self.select_at_cursor();
    }

    fn set_view(&mut self, v: View) {
        self.view = v;
        let _ = self.store.set_meta("last_view", v.id());
        self.agenda_from = self.day;
        self.refresh();
        self.select_at_cursor();
    }

    /// Jumps to the next (`dir = 1`) or previous event in time.
    fn step_event(&mut self, dir: i32) {
        let cur = self.selected().map(|(o, _)| (o.start, o.event_id.clone()));
        let cursor = tz::resolve(self.day.and_hms_opt(self.slot / 60, self.slot % 60, 0).expect("slot"), self.zone);
        let pos = cur.as_ref().map(|(s, _)| *s).unwrap_or(cursor);
        let cur_idx = self.sel;
        let pick = if dir > 0 {
            (0..self.occs.len())
                .filter(|i| Some(*i) != cur_idx && (self.occs[*i].start > pos || (self.occs[*i].start == pos && Some(*i) > cur_idx)))
                .min_by_key(|i| (self.occs[*i].start, *i))
        } else {
            (0..self.occs.len())
                .filter(|i| Some(*i) != cur_idx && (self.occs[*i].start < pos || (self.occs[*i].start == pos && Some(*i) < cur_idx)))
                .max_by_key(|i| (self.occs[*i].start, *i))
        };
        let Some(i) = pick else {
            // Look past the visible range.
            let from = if dir > 0 { pos + Dur::seconds(1) } else { pos - Dur::days(366) };
            let to = if dir > 0 { pos + Dur::days(366) } else { pos };
            let on: Vec<&str> = self.calendars.iter().filter(|c| c.enabled).map(|c| c.id.as_str()).collect();
            let shown: Vec<Event> = self.events.iter().filter(|e| on.contains(&e.calendar.as_str())).cloned().collect();
            let found = recur::expand(&shown, from, to, self.zone);
            let next = if dir > 0 {
                found.iter().filter(|o| o.start > pos).min_by_key(|o| o.start)
            } else {
                found.iter().filter(|o| o.start < pos).max_by_key(|o| o.start)
            };
            if let Some(o) = next.cloned() {
                self.jump_to(&o);
            } else {
                self.flash(if dir > 0 { "No later events" } else { "No earlier events" });
            }
            return;
        };
        let o = self.occs[i].clone();
        self.jump_to(&o);
    }

    /// Shows and selects an occurrence.
    pub fn jump_to(&mut self, o: &Occurrence) {
        let local = tz::local(o.start, self.zone);
        let (a, b) = self.visible_days();
        self.day = o.days.map(|(a, _)| a).unwrap_or(local.date());
        if self.day < a || self.day >= b {
            self.agenda_from = self.day;
        }
        if !o.all_day {
            let step = self.slot_minutes();
            self.slot = (local.hour() * 60 + local.minute()) / step * step;
        }
        self.refresh();
        self.sel = self.occs.iter().position(|x| x.event_id == o.event_id && x.start == o.start);
    }

    fn move_slot(&mut self, steps: i32) {
        let step = self.slot_minutes() as i32;
        let m = self.slot as i32 + steps * step;
        if m < 0 {
            self.slot = (1440 - step) as u32;
            self.go_to(self.day - Dur::days(1));
        } else if m >= 1440 {
            self.slot = 0;
            self.go_to(self.day + Dur::days(1));
        } else {
            self.slot = m as u32;
            self.select_at_cursor();
        }
    }

    fn period(&mut self, dir: i64) {
        let d = match self.view {
            View::Day => self.day + Dur::days(dir),
            View::Week => self.day + Dur::days(7 * dir),
            View::Agenda => self.agenda_from + Dur::days(self.settings.agenda_days.max(1) * dir),
            View::Month => {
                let m0 = self.day.year() * 12 + self.day.month0() as i32 + dir as i32;
                let (y, m) = (m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1);
                let last = NaiveDate::from_ymd_opt(y, m, 1)
                    .and_then(|f| f.checked_add_months(chrono::Months::new(1)))
                    .map(|n| n.pred_opt().expect("day").day())
                    .unwrap_or(28);
                NaiveDate::from_ymd_opt(y, m, self.day.day().min(last)).expect("valid")
            }
        };
        self.go_to(d);
    }

    pub fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.store) {
            self.flash(format!("Could not save settings: {e}"));
        }
    }

    /// Re-reads everything settings change: keys, zone, theme.
    pub fn apply_settings(&mut self) {
        self.keys = Keymap::new(&self.settings.keys);
        self.zone = self.settings.zone();
        self.theme = theme::load(&self.settings.theme);
        self.save_settings();
        self.refresh();
        self.select_at_cursor();
    }

    pub fn default_calendar(&self) -> Option<&Calendar> {
        self.calendars.iter().find(|c| c.default && !c.read_only).or_else(|| self.calendars.iter().find(|c| !c.read_only))
    }

    /// A new event at the cursor.
    pub fn new_event(&self) -> Event {
        let start = match self.view {
            View::Day | View::Week => self.day.and_hms_opt(self.slot / 60, self.slot % 60, 0).expect("slot"),
            _ => self.day.and_hms_opt(self.settings.day_start_hour.max(9), 0, 0).expect("hour"),
        };
        Event {
            id: new_id(),
            uid: new_id(),
            calendar: self.default_calendar().map(|c| c.id.clone()).unwrap_or_default(),
            start,
            end: start + Dur::minutes(self.settings.default_duration),
            tz: self.zone.name().into(),
            reminders: if self.settings.default_reminder >= 0 { vec![self.settings.default_reminder] } else { vec![] },
            updated: self.now(),
            ..Default::default()
        }
    }

    /// Saves changes and reloads. `notify`: guests are to be told (used by sync).
    pub fn commit(&mut self, mut ch: Changes, notify: Option<bool>, msg: &str) {
        ch.notify = notify == Some(true);
        let synced = ch
            .upsert
            .iter()
            .map(|e| e.calendar.as_str())
            .chain(ch.delete.iter().filter_map(|id| self.event(id).map(|e| e.calendar.as_str())))
            .any(|c| self.calendar(c).is_some_and(|c| c.remote.is_some()));
        match self.store.apply(&ch) {
            Ok(()) => {
                self.reload();
                if synced {
                    self.request_sync();
                }
                let note = match notify {
                    Some(true) => " · guests will be told",
                    Some(false) => " · guests not told",
                    None => "",
                };
                self.flash(format!("{msg}{note}"));
            }
            Err(e) => self.flash(format!("Could not save: {e}")),
        }
    }

    /// The picked occurrence as an edit target.
    pub fn target(o: &Occurrence) -> Target {
        Target { event_id: o.event_id.clone(), original: o.original }
    }

    /// Saves an edited event: asks for the scope (repeating) and whether to
    /// tell guests, then commits.
    pub fn save_edit(&mut self, target: Option<Target>, edited: Event) {
        let Some(target) = target else {
            // Brand new event.
            let msg = format!("Added “{}”", edited.title);
            let guests = edited.has_guests();
            let ch = Changes::new(vec![edited], vec![]);
            if guests {
                self.dialog = Some(Dialog::Notify { changes: ch, msg, sel: 0 });
            } else {
                self.commit(ch, None, &msg);
            }
            return;
        };
        let group = self.group_of(&target.event_id);
        if target.original.is_some() && group.iter().any(|e| e.is_series()) {
            self.dialog = Some(Dialog::Scope { pending: dialogs::Pending::Edit(target, Box::new(edited)), sel: 0 });
        } else {
            self.finish(dialogs::Pending::Edit(target, Box::new(edited)), Scope::All);
        }
    }

    fn group_of(&self, event_id: &str) -> Vec<Event> {
        let Some(e) = self.event(event_id) else { return vec![] };
        self.events.iter().filter(|x| x.uid == e.uid).cloned().collect()
    }

    /// Runs an edit or delete with a scope; asks about guests when needed.
    pub fn finish(&mut self, p: dialogs::Pending, scope: Scope) {
        let now = self.now();
        let (ch, guests, msg) = match &p {
            dialogs::Pending::Edit(t, e) => {
                let group = self.group_of(&t.event_id);
                let had = group.iter().any(|x| x.has_guests());
                let mut ch = edit::edit(&group, t, e, scope, self.zone, now);
                move_group(&mut ch, &group, &e.calendar);
                (ch, had || e.has_guests(), format!("Saved “{}”", e.title))
            }
            dialogs::Pending::Delete(t) => {
                let group = self.group_of(&t.event_id);
                let title = self.event(&t.event_id).map(|e| e.title.clone()).unwrap_or_default();
                // Guests only hear about a delete from the organizer.
                let organizer = group.iter().any(|x| x.has_guests() && x.my_status().is_none());
                (edit::delete(&group, t, scope, self.zone, now), organizer, format!("Deleted “{title}”"))
            }
        };
        if guests {
            self.dialog = Some(Dialog::Notify { changes: ch, msg, sel: 0 });
        } else {
            self.commit(ch, None, &msg);
        }
    }

    /// Asks the background thread to sync now.
    pub fn request_sync(&self) {
        if let Some(tx) = &self.sync_now {
            let _ = tx.send(());
        }
    }

    pub fn ask_delete(&mut self) {
        let Some((o, e)) = self.selected() else { return self.flash("No event selected") };
        if self.calendar(&e.calendar).is_some_and(|c| c.read_only) {
            return self.flash("This calendar is read-only");
        }
        let target = Self::target(o);
        let repeating = o.recurring && self.group_of(&o.event_id).iter().any(|x| x.is_series());
        let title = e.title.clone();
        self.dialog = Some(if repeating {
            Dialog::Scope { pending: dialogs::Pending::Delete(target), sel: 0 }
        } else {
            Dialog::ConfirmDelete { target, title }
        });
    }

    /// Answers an invite for the whole event (series included).
    pub fn rsvp(&mut self, event_id: &str, answer: Partstat) {
        let group = self.group_of(event_id);
        let now = self.now();
        let upsert: Vec<Event> = group
            .into_iter()
            .map(|mut e| {
                for a in e.attendees.iter_mut().filter(|a| a.me) {
                    a.status = answer;
                }
                e.updated = now;
                e
            })
            .collect();
        // The answer goes to the organizer.
        self.commit(Changes::new(upsert, vec![]), Some(true), "");
        self.flash(format!("Answered {}", answer.label()));
    }

    pub fn toggle_calendar(&mut self, i: usize) {
        if let Some(c) = self.calendars.get_mut(i) {
            c.enabled = !c.enabled;
            let c = c.clone();
            if let Err(e) = self.store.save_calendar(&c) {
                self.flash(format!("Could not save: {e}"));
            }
            self.refresh();
            self.select_at_cursor();
        }
    }

    pub fn open_link(&mut self, url: &str) {
        match std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.flash("Opening the call…"),
            Err(e) => self.flash(format!("Could not open link: {e}")),
        }
    }

    pub fn key(&mut self, k: KeyEvent) {
        if self.dialog.is_some() {
            return dialogs::key(self, k);
        }
        let Some(a) = self.keys.action(&k) else { return };
        if self.focus == Focus::Side {
            match a {
                Action::Down => return self.side_sel = (self.side_sel + 1).min(self.calendars.len().saturating_sub(1)),
                Action::Up => return self.side_sel = self.side_sel.saturating_sub(1),
                Action::ToggleCalendar | Action::Open => return self.toggle_calendar(self.side_sel),
                _ => {}
            }
        }
        match a {
            Action::Quit => {
                if self.settings.confirm_quit {
                    self.dialog = Some(Dialog::Quit);
                } else {
                    self.quit = true;
                }
            }
            Action::Back => {
                if self.focus != Focus::Main {
                    self.focus = Focus::Main;
                }
            }
            Action::NextPane | Action::PrevPane => {
                let order: Vec<Focus> =
                    if self.wide { vec![Focus::Side, Focus::Main, Focus::Details] } else { vec![Focus::Side, Focus::Main] };
                let i = order.iter().position(|f| *f == self.focus).unwrap_or(1);
                let n = order.len();
                self.focus = order[if a == Action::NextPane { (i + 1) % n } else { (i + n - 1) % n }];
            }
            Action::Left => match self.view {
                View::Agenda => self.period(-1),
                _ => self.go_to(self.day - Dur::days(1)),
            },
            Action::Right => match self.view {
                View::Agenda => self.period(1),
                _ => self.go_to(self.day + Dur::days(1)),
            },
            Action::Down => match self.view {
                View::Day | View::Week => self.move_slot(1),
                View::Month => self.go_to(self.day + Dur::days(7)),
                View::Agenda => self.step_event(1),
            },
            Action::Up => match self.view {
                View::Day | View::Week => self.move_slot(-1),
                View::Month => self.go_to(self.day - Dur::days(7)),
                View::Agenda => self.step_event(-1),
            },
            Action::PageDown | Action::PageUp => {
                let rows = self.hit.grid_rows.max(4) as i32 / 2;
                self.move_slot(if a == Action::PageDown { rows } else { -rows });
            }
            Action::NextEvent => self.step_event(1),
            Action::PrevEvent => self.step_event(-1),
            Action::PrevPeriod => self.period(-1),
            Action::NextPeriod => self.period(1),
            Action::Today => {
                let now = tz::local(self.now(), self.zone);
                let step = self.slot_minutes();
                self.slot = (now.hour() * 60 + now.minute()) / step * step;
                self.go_to(now.date());
            }
            Action::GoTo => self.dialog = Some(Dialog::GoTo { input: Default::default() }),
            Action::ViewDay => self.set_view(View::Day),
            Action::ViewWeek => self.set_view(View::Week),
            Action::ViewMonth => self.set_view(View::Month),
            Action::ViewAgenda => self.set_view(View::Agenda),
            Action::Search => self.dialog = Some(Dialog::Search { input: Default::default(), sel: 0 }),
            Action::QuickAdd => self.dialog = Some(Dialog::QuickAdd { input: Default::default() }),
            Action::NewEvent => {
                let e = self.new_event();
                self.dialog = Some(Dialog::Form(Box::new(dialogs::Form::new(self, e, None))));
            }
            Action::Open => {
                if self.view == View::Month && self.sel.is_none() {
                    self.set_view(View::Day);
                } else if self.sel.is_none() {
                    // Day/week: pick the first event of the day (all-day ones too,
                    // which the time cursor can't reach).
                    match self.on_day(self.day).first() {
                        Some(i) => self.sel = Some(*i),
                        None => self.flash(format!("No events on this day. {} adds one", self.keys.hint(Action::QuickAdd))),
                    }
                } else if self.wide {
                    self.focus = Focus::Details;
                } else {
                    self.dialog = Some(Dialog::Details { scroll: 0 });
                }
            }
            Action::Edit => {
                let Some((o, e)) = self.selected() else {
                    return self.flash("No event picked: enter picks one on this day, } jumps to the next");
                };
                if self.calendar(&e.calendar).is_some_and(|c| c.read_only) {
                    return self.flash("This calendar is read-only");
                }
                let (o, e) = (o.clone(), e.clone());
                self.dialog = Some(Dialog::Form(Box::new(dialogs::Form::for_occurrence(self, &o, &e))));
            }
            Action::Delete => self.ask_delete(),
            Action::Join => match self.selected().and_then(|(_, e)| e.video_link()) {
                Some(url) => self.open_link(&url),
                None => self.flash("No video link in this event"),
            },
            Action::Invites => self.dialog = Some(Dialog::Invites { sel: 0 }),
            Action::Sync => {
                if self.accounts.is_empty() {
                    self.flash(format!("No online accounts yet. Add one with {}", self.keys.hint(Action::Accounts)));
                } else {
                    self.request_sync();
                    self.flash("Syncing…");
                }
            }
            Action::Accounts => self.dialog = Some(Dialog::Accounts(accounts::AccountsUi::default())),
            Action::Conflicts => self.dialog = Some(Dialog::Conflicts { sel: 0 }),
            Action::ToggleCalendar => {
                self.focus = Focus::Side;
            }
            Action::Themes => self.dialog = Some(Dialog::Themes(prefs::ThemePicker::new(self))),
            Action::Settings => self.dialog = Some(Dialog::Settings(prefs::SettingsUi::default())),
            Action::Help => self.dialog = Some(Dialog::Help { scroll: 0 }),
            Action::RsvpYes | Action::RsvpNo | Action::RsvpMaybe => {}
        }
    }

    fn mouse(&mut self, m: crossterm::event::MouseEvent) {
        if self.dialog.is_some() {
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((d, slot)) = self.hit.at(m.column, m.row) {
                    self.focus = Focus::Main;
                    if let Some(s) = slot {
                        self.slot = s;
                    }
                    self.go_to(d);
                    if let Some(i) = self.hit.event_at(m.column, m.row) {
                        self.sel = Some(i);
                    }
                }
            }
            MouseEventKind::ScrollDown => self.move_slot(2),
            MouseEventKind::ScrollUp => self.move_slot(-2),
            _ => {}
        }
    }

    fn tick(&mut self) {
        if self.watch.changed() && self.settings.theme == musinator_themes::omarchy::ID {
            self.theme = theme::load(&self.settings.theme);
        }
        if self.status.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(5)) {
            self.status = None;
        }
        // Sync, reminders or subscriptions finished a round.
        let msgs: Vec<Msg> = self.background.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for m in msgs {
            match m {
                Msg::Syncing => self.syncing = true,
                Msg::Done(_) => {
                    self.syncing = false;
                    self.reload();
                }
            }
        }
        // A login check finished on its thread.
        if matches!(self.dialog, Some(Dialog::Accounts(_)))
            && let Some(Dialog::Accounts(mut ui)) = self.dialog.take()
        {
            accounts::poll(self, &mut ui);
            if self.dialog.is_none() {
                self.dialog = Some(Dialog::Accounts(ui));
            }
        }
    }
}

/// Moving an event to another calendar moves its whole series with it.
fn move_group(ch: &mut Changes, group: &[Event], to: &str) {
    if group.iter().all(|e| e.calendar == to) {
        return;
    }
    for e in ch.upsert.iter_mut() {
        e.calendar = to.to_string();
    }
    for g in group {
        if !ch.upsert.iter().any(|u| u.id == g.id) && !ch.delete.contains(&g.id) {
            ch.upsert.push(Event { calendar: to.to_string(), ..g.clone() });
        }
    }
}

/// Wall time, rounded to the minute.
pub fn now_local(zone: Tz) -> NaiveDateTime {
    tz::local(Utc::now(), zone).with_second(0).and_then(|t| t.with_nanosecond(0)).unwrap_or_default()
}

/// Runs the app on the calendar file `db`, showing `message` first (e.g. the
/// result of an import). Reminders and subscriptions run on a background
/// thread, so the screen never waits for them.
pub fn run(store: Store, db: &Path, message: Option<String>) -> std::io::Result<()> {
    let mut app = App::new(store);
    if let Some(m) = message {
        app.flash(m);
    } else if app.accounts.is_empty() && app.store.meta("welcomed").ok().flatten().is_none() {
        // First run: offer to add an online calendar.
        app.dialog = Some(Dialog::Accounts(accounts::AccountsUi::welcome()));
    }
    let (tx, rx) = channel();
    app.sync_now = Some(crate::daemon::Background::spawn(db, tx));
    app.background = Some(rx);
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    let mut last_day = app.today();
    let result = (|| {
        while !app.quit {
            if app.today() != last_day {
                // Past midnight: someone looking at today keeps seeing today.
                if app.day == last_day {
                    app.go_to(app.today());
                }
                last_day = app.today();
                app.refresh();
            }
            app.tick();
            terminal.draw(|f| draw::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    TermEvent::Key(k) if k.kind != KeyEventKind::Release => app.key(k),
                    TermEvent::Mouse(m) => app.mouse(m),
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    result
}
