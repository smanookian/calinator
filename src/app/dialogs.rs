//! Floating dialogs: quick add, event form, repeat scope, guests, delete,
//! details, search, go to date, invites, help, quit.

use chrono::Duration;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear};
use unicode_width::UnicodeWidthStr;

use super::draw::{border, c, details_lines, fill, fit, put, put_right, span_label, time_label};
use super::input::Input;
use super::{App, now_local};
use crate::edit::{Changes, Scope, Target};
use crate::keys::{ACTIONS, Action};
use crate::model::{Attendee, Event, Occurrence, Partstat};
use crate::{quickadd, recur, tz};

pub enum Pending {
    Edit(Target, Box<Event>),
    Delete(Target),
}

pub enum Dialog {
    QuickAdd {
        input: Input,
    },
    Form(Box<Form>),
    /// "This event / this and following / all events".
    Scope {
        pending: Pending,
        sel: usize,
    },
    /// "Send update to guests?"
    Notify {
        changes: Changes,
        msg: String,
        sel: usize,
    },
    ConfirmDelete {
        target: Target,
        title: String,
    },
    Details {
        scroll: usize,
    },
    Search {
        input: Input,
        sel: usize,
    },
    GoTo {
        input: Input,
    },
    Invites {
        sel: usize,
    },
    Themes(super::prefs::ThemePicker),
    Settings(super::prefs::SettingsUi),
    Help {
        scroll: usize,
    },
    Accounts(super::accounts::AccountsUi),
    Conflicts {
        sel: usize,
    },
    Quit,
}

// ---------- Event form ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Calendar,
    StartDay,
    StartTime,
    EndDay,
    EndTime,
    AllDay,
    Zone,
    Repeat,
    Location,
    Video,
    Guests,
    Remind,
    Notes,
}

const FIELDS: [(Field, &str); 14] = [
    (Field::Title, "Title"),
    (Field::Calendar, "Calendar"),
    (Field::StartDay, "Starts"),
    (Field::StartTime, "Start time"),
    (Field::EndDay, "Ends"),
    (Field::EndTime, "End time"),
    (Field::AllDay, "All day"),
    (Field::Zone, "Time zone"),
    (Field::Repeat, "Repeat"),
    (Field::Location, "Where"),
    (Field::Video, "Call link"),
    (Field::Guests, "Guests"),
    (Field::Remind, "Remind"),
    (Field::Notes, "Notes"),
];

pub struct Form {
    base: Event,
    target: Option<Target>,
    /// The picked instance is part of a series (repeat edits apply to the series).
    repeating: bool,
    inputs: Vec<Input>,
    calendar: usize,
    all_day: bool,
    sel: usize,
    err: Option<String>,
    /// Repeat as first shown, and the rule behind it.
    orig_repeat: (String, Option<String>),
}

fn date_text(d: chrono::NaiveDate) -> String {
    d.format("%b %-d %Y").to_string().to_lowercase()
}

impl Form {
    pub fn new(app: &App, e: Event, target: Option<Target>) -> Self {
        Self::build(app, e, target, None)
    }

    /// The form for a picked occurrence: times of that instance, repeat of the series.
    pub fn for_occurrence(app: &App, o: &Occurrence, e: &Event) -> Self {
        let mut base = e.clone();
        let zone = recur::event_zone(e, app.zone);
        if let Some((a, b)) = o.days {
            base.start = a.and_hms_opt(0, 0, 0).expect("midnight");
            base.end = b.and_hms_opt(0, 0, 0).expect("midnight");
        } else {
            base.start = tz::local(o.start, zone);
            base.end = tz::local(o.end, zone);
        }
        let series = app.events.iter().find(|x| x.uid == e.uid && x.is_series()).cloned();
        Self::build(app, base, Some(App::target(o)), series)
    }

    fn build(app: &App, e: Event, target: Option<Target>, series: Option<Event>) -> Self {
        let h24 = app.settings.clock_24h;
        let rule = series.as_ref().and_then(|s| s.rrule.clone()).or_else(|| e.rrule.clone());
        let end_day = if e.all_day { e.end.date() - Duration::days(1) } else { e.end.date() };
        let guests: Vec<String> = e.attendees.iter().filter(|a| !a.me).map(|a| a.email.clone()).collect();
        let text = |f: Field| -> String {
            match f {
                Field::Title => e.title.clone(),
                Field::StartDay => date_text(e.start.date()),
                Field::StartTime => time_label(e.start.time(), h24),
                Field::EndDay => date_text(end_day.max(e.start.date())),
                Field::EndTime => time_label(e.end.time(), h24),
                Field::Zone => {
                    if e.all_day {
                        String::new()
                    } else {
                        e.tz.clone()
                    }
                }
                Field::Repeat => rule.as_deref().map(recur::describe).unwrap_or_default(),
                Field::Location => e.location.clone(),
                Field::Video => e.video.clone().unwrap_or_default(),
                Field::Guests => guests.join(", "),
                Field::Remind => e.reminders.iter().map(|m| quickadd::reminder_text(*m)).collect::<Vec<_>>().join(", "),
                Field::Notes => e.description.clone(),
                Field::Calendar | Field::AllDay => String::new(),
            }
        };
        let inputs = FIELDS.iter().map(|(f, _)| Input::new(&text(*f))).collect();
        let calendar = app.calendars.iter().position(|c| c.id == e.calendar).unwrap_or(0);
        let orig_repeat = (rule.as_deref().map(recur::describe).unwrap_or_default(), rule.clone());
        Form { all_day: e.all_day, repeating: series.is_some(), base: e, target, inputs, calendar, sel: 0, err: None, orig_repeat }
    }

    fn field(&self) -> Field {
        FIELDS[self.sel].0
    }

    fn text(&self, f: Field) -> &str {
        let i = FIELDS.iter().position(|(x, _)| *x == f).expect("field");
        &self.inputs[i].text
    }

    fn hidden(&self, f: Field) -> bool {
        self.all_day && matches!(f, Field::StartTime | Field::EndTime | Field::Zone)
    }

    /// The event the form describes, or what to fix.
    fn to_event(&self, app: &App) -> Result<Event, String> {
        let today = app.today();
        let mut e = self.base.clone();
        e.title = self.text(Field::Title).trim().to_string();
        if e.title.is_empty() {
            return Err("Add a title".into());
        }
        let cal = app.calendars.get(self.calendar).ok_or("Add a calendar first (Settings)")?;
        if cal.read_only {
            return Err(format!("“{}” is read-only", cal.name));
        }
        e.calendar = cal.id.clone();
        let sd = quickadd::parse_date(self.text(Field::StartDay), today).ok_or("Start day: try “nov 3 2026” or “tomorrow”")?;
        let ed = quickadd::parse_date(self.text(Field::EndDay), today).ok_or("End day: try “nov 3 2026” or “tomorrow”")?;
        e.all_day = self.all_day;
        if self.all_day {
            if ed < sd {
                return Err("The event ends before it starts".into());
            }
            e.start = sd.and_hms_opt(0, 0, 0).expect("midnight");
            e.end = (ed + Duration::days(1)).and_hms_opt(0, 0, 0).expect("midnight");
        } else {
            let st = quickadd::parse_time(self.text(Field::StartTime)).ok_or("Start time: try “9:30” or “3pm”")?;
            let et = quickadd::parse_time(self.text(Field::EndTime)).ok_or("End time: try “10:30” or “4pm”")?;
            e.start = sd.and_time(st);
            e.end = ed.and_time(et);
            if e.end < e.start {
                if ed == sd {
                    // "11pm – 1am" on one day means past midnight.
                    e.end += Duration::days(1);
                } else {
                    return Err("The event ends before it starts".into());
                }
            }
            let z = self.text(Field::Zone).trim();
            e.tz = if z.is_empty() {
                app.zone.name().into()
            } else {
                z.parse::<chrono_tz::Tz>().map_err(|_| format!("Unknown time zone “{z}”. Try “Europe/Berlin”"))?.name().into()
            };
        }
        e.rrule = if self.text(Field::Repeat) == self.orig_repeat.0 {
            // Unchanged: keep the exact rule (the text may not show every detail).
            self.orig_repeat.1.clone()
        } else {
            quickadd::parse_repeat(self.text(Field::Repeat)).ok_or("Repeat: try “every week”, “every mon and wed” or leave empty")?
        };
        e.location = self.text(Field::Location).trim().to_string();
        let v = self.text(Field::Video).trim();
        e.video = (!v.is_empty()).then(|| v.to_string());
        let mut guests = Vec::new();
        for g in self.text(Field::Guests).split(',').map(str::trim).filter(|g| !g.is_empty()) {
            if !g.contains('@') {
                return Err(format!("“{g}” is not an email address"));
            }
            let old = self.base.attendees.iter().find(|a| a.email.eq_ignore_ascii_case(g));
            guests.push(old.cloned().unwrap_or(Attendee {
                email: g.to_string(),
                name: String::new(),
                status: Partstat::NeedsAction,
                me: false,
            }));
        }
        guests.extend(self.base.attendees.iter().filter(|a| a.me).cloned());
        e.attendees = guests;
        e.reminders = quickadd::parse_reminders(self.text(Field::Remind)).ok_or("Remind: try “10m, 1h, 1d”")?;
        e.description = self.text(Field::Notes).to_string();
        e.updated = app.now();
        Ok(e)
    }
}

fn form_key(app: &mut App, mut form: Box<Form>, k: KeyEvent) -> Option<Dialog> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let step = |form: &mut Form, d: i32| {
        let n = FIELDS.len() as i32;
        let mut i = form.sel as i32;
        loop {
            i = (i + d).rem_euclid(n);
            if !form.hidden(FIELDS[i as usize].0) {
                break;
            }
        }
        form.sel = i as usize;
    };
    match k.code {
        KeyCode::Esc => return None,
        KeyCode::Tab | KeyCode::Down => step(&mut form, 1),
        KeyCode::BackTab | KeyCode::Up => step(&mut form, -1),
        KeyCode::Enter => {
            return match form.to_event(app) {
                Ok(e) => {
                    app.dialog = None;
                    app.save_edit(form.target.clone(), e);
                    app.dialog.take()
                }
                Err(msg) => {
                    form.err = Some(msg);
                    Some(Dialog::Form(form))
                }
            };
        }
        KeyCode::Char('s') if ctrl => return form_key(app, form, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        _ => match form.field() {
            Field::Calendar => {
                let n = app.calendars.len().max(1);
                match k.code {
                    KeyCode::Left => form.calendar = (form.calendar + n - 1) % n,
                    KeyCode::Right | KeyCode::Char(' ') => form.calendar = (form.calendar + 1) % n,
                    _ => {}
                }
            }
            Field::AllDay => {
                if matches!(k.code, KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) {
                    form.all_day = !form.all_day;
                }
            }
            _ => {
                form.inputs[form.sel].key(&k);
            }
        },
    }
    form.err = None;
    Some(Dialog::Form(form))
}

// ---------- Keys ----------

pub fn key(app: &mut App, k: KeyEvent) {
    let Some(d) = app.dialog.take() else { return };
    let back = app.keys.is(Action::Back, &k) || k.code == KeyCode::Esc;
    let next = match d {
        Dialog::Form(f) => form_key(app, f, k),
        Dialog::QuickAdd { mut input } => match k.code {
            KeyCode::Esc => None,
            KeyCode::Enter => {
                if input.text.trim().is_empty() {
                    return;
                }
                match quick_event(app, &input.text) {
                    Ok(e) => {
                        app.save_edit(None, e);
                        app.dialog.take()
                    }
                    Err(msg) => {
                        app.flash(msg);
                        Some(Dialog::QuickAdd { input })
                    }
                }
            }
            KeyCode::Tab => {
                // Open the full form with what was typed.
                match quick_event(app, &input.text) {
                    Ok(e) => Some(Dialog::Form(Box::new(Form::new(app, e, None)))),
                    Err(_) => Some(Dialog::QuickAdd { input }),
                }
            }
            _ => {
                input.key(&k);
                Some(Dialog::QuickAdd { input })
            }
        },
        Dialog::Scope { pending, mut sel } => match k.code {
            _ if back => None,
            KeyCode::Down | KeyCode::Char('j') => {
                sel = (sel + 1) % 3;
                Some(Dialog::Scope { pending, sel })
            }
            KeyCode::Up | KeyCode::Char('k') => {
                sel = (sel + 2) % 3;
                Some(Dialog::Scope { pending, sel })
            }
            KeyCode::Char('1') | KeyCode::Char('2') | KeyCode::Char('3') | KeyCode::Enter => {
                let pick = match k.code {
                    KeyCode::Char(ch) => (ch as u8 - b'1') as usize,
                    _ => sel,
                };
                app.finish(pending, [Scope::This, Scope::Following, Scope::All][pick]);
                app.dialog.take()
            }
            _ => Some(Dialog::Scope { pending, sel }),
        },
        Dialog::Notify { changes, msg, mut sel } => match k.code {
            _ if back => {
                app.flash("Not saved");
                None
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l') => {
                sel = 1 - sel;
                Some(Dialog::Notify { changes, msg, sel })
            }
            KeyCode::Char('y') | KeyCode::Char('n') | KeyCode::Enter => {
                let tell = match k.code {
                    KeyCode::Char('y') => true,
                    KeyCode::Char('n') => false,
                    _ => sel == 0,
                };
                app.commit(changes, Some(tell), &msg);
                None
            }
            _ => Some(Dialog::Notify { changes, msg, sel }),
        },
        Dialog::ConfirmDelete { target, title } => match k.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                app.finish(Pending::Delete(target), Scope::All);
                app.dialog.take()
            }
            _ if back || k.code == KeyCode::Char('n') => None,
            _ => Some(Dialog::ConfirmDelete { target, title }),
        },
        Dialog::Details { scroll } => {
            if back || app.keys.is(Action::Open, &k) {
                None
            } else if app.keys.is(Action::Down, &k) {
                Some(Dialog::Details { scroll: scroll + 1 })
            } else if app.keys.is(Action::Up, &k) {
                Some(Dialog::Details { scroll: scroll.saturating_sub(1) })
            } else if app.keys.action(&k).is_some_and(|a| matches!(a, Action::Edit | Action::Delete | Action::Join | Action::Invites)) {
                // Run it as on the main screen (the popup is already closed).
                app.key(k);
                app.dialog.take()
            } else {
                Some(Dialog::Details { scroll })
            }
        }
        Dialog::Search { mut input, mut sel } => match k.code {
            KeyCode::Esc => None,
            KeyCode::Down => {
                sel += 1;
                Some(Dialog::Search { input, sel })
            }
            KeyCode::Up => {
                sel = sel.saturating_sub(1);
                Some(Dialog::Search { input, sel })
            }
            KeyCode::Enter => {
                let hits = search(app, &input.text);
                if let Some(o) = hits.get(sel.min(hits.len().saturating_sub(1))) {
                    let o = o.clone();
                    app.jump_to(&o);
                    None
                } else {
                    Some(Dialog::Search { input, sel })
                }
            }
            _ => {
                input.key(&k);
                sel = 0;
                Some(Dialog::Search { input, sel })
            }
        },
        Dialog::GoTo { mut input } => match k.code {
            KeyCode::Esc => None,
            KeyCode::Enter => match quickadd::parse_date(&input.text, app.today()) {
                Some(d) => {
                    app.go_to(d);
                    None
                }
                None => {
                    app.flash("Try “nov 20”, “fri” or “2027-01-05”");
                    Some(Dialog::GoTo { input })
                }
            },
            _ => {
                input.key(&k);
                Some(Dialog::GoTo { input })
            }
        },
        Dialog::Invites { mut sel } => {
            let list = invites(app);
            let answer = if app.keys.is(Action::RsvpYes, &k) {
                Some(Partstat::Accepted)
            } else if app.keys.is(Action::RsvpNo, &k) {
                Some(Partstat::Declined)
            } else if app.keys.is(Action::RsvpMaybe, &k) {
                Some(Partstat::Tentative)
            } else {
                None
            };
            if let (Some(a), Some(o)) = (answer, list.get(sel)) {
                let id = o.event_id.clone();
                app.rsvp(&id, a);
                Some(Dialog::Invites { sel: sel.min(invites(app).len().saturating_sub(1)) })
            } else if back {
                None
            } else if app.keys.is(Action::Down, &k) {
                sel = (sel + 1).min(list.len().saturating_sub(1));
                Some(Dialog::Invites { sel })
            } else if app.keys.is(Action::Up, &k) {
                sel = sel.saturating_sub(1);
                Some(Dialog::Invites { sel })
            } else if app.keys.is(Action::Open, &k) {
                if let Some(o) = list.get(sel) {
                    let o = o.clone();
                    app.jump_to(&o);
                }
                None
            } else {
                Some(Dialog::Invites { sel })
            }
        }
        Dialog::Themes(p) => super::prefs::theme_key(app, p, k),
        Dialog::Accounts(ui) => super::accounts::key(app, ui, k),
        Dialog::Conflicts { sel } => super::accounts::conflicts_key(app, sel, k),
        Dialog::Settings(s) => super::prefs::settings_key(app, s, k),
        Dialog::Help { scroll } => {
            if back || app.keys.is(Action::Help, &k) || app.keys.is(Action::Quit, &k) {
                None
            } else if app.keys.is(Action::Down, &k) || app.keys.is(Action::PageDown, &k) {
                Some(Dialog::Help { scroll: scroll + if app.keys.is(Action::PageDown, &k) { 10 } else { 1 } })
            } else if app.keys.is(Action::Up, &k) || app.keys.is(Action::PageUp, &k) {
                Some(Dialog::Help { scroll: scroll.saturating_sub(if app.keys.is(Action::PageUp, &k) { 10 } else { 1 }) })
            } else {
                Some(Dialog::Help { scroll })
            }
        }
        Dialog::Quit => match k.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                app.quit = true;
                None
            }
            _ => None,
        },
    };
    if app.dialog.is_none() {
        app.dialog = next;
    }
}

/// Quick-add text → a new event.
pub fn quick_event(app: &App, text: &str) -> Result<Event, String> {
    let now = now_local(app.zone);
    // "@work" picks the calendar (first writable one whose name contains it).
    let mut calendar = None;
    let mut words = Vec::new();
    for w in text.split_whitespace() {
        match w.strip_prefix('@').filter(|n| !n.is_empty()) {
            Some(name) => {
                let n = name.to_lowercase();
                let c = app
                    .calendars
                    .iter()
                    .find(|c| !c.read_only && c.name.to_lowercase().contains(&n))
                    .ok_or_else(|| format!("No calendar matches @{name}"))?;
                calendar = Some(c.id.clone());
            }
            None => words.push(w),
        }
    }
    let p = quickadd::parse(&words.join(" "), now);
    if p.title.trim().is_empty() {
        return Err("Add a title".into());
    }
    let mut e = app.new_event();
    e.title = p.title.clone();
    if let Some(c) = calendar {
        e.calendar = c;
    }
    let day = p.date.unwrap_or(match app.view {
        super::View::Agenda => now.date(),
        _ => app.day,
    });
    e.rrule = p.rrule.clone();
    match (p.start, p.all_day) {
        (Some(st), false) => {
            e.start = day.and_time(st);
            e.end = match (p.end, p.minutes) {
                (Some(en), _) => {
                    let mut end = day.and_time(en);
                    if end <= e.start {
                        end += Duration::days(1);
                    }
                    end
                }
                (None, Some(m)) => e.start + Duration::minutes(m),
                (None, None) => e.start + Duration::minutes(app.settings.default_duration),
            };
        }
        _ => {
            e.all_day = true;
            e.start = day.and_hms_opt(0, 0, 0).expect("midnight");
            e.end = e.start + Duration::days(1);
        }
    }
    Ok(e)
}

/// Events matching a search, each once: its next occurrence (else its last).
fn search(app: &App, q: &str) -> Vec<Occurrence> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let now = app.now();
    let mut out = Vec::new();
    for e in app
        .events
        .iter()
        .filter(|e| e.recurrence_id.is_none() && [&e.title, &e.location, &e.description].iter().any(|t| t.to_lowercase().contains(&q)))
    {
        let group: Vec<Event> = app.events.iter().filter(|x| x.uid == e.uid).cloned().collect();
        let ahead = recur::expand(&group, now - Duration::days(1), now + Duration::days(730), app.zone);
        let o = ahead.into_iter().find(|o| o.end >= now).or_else(|| recur::expand(&group, now - Duration::days(3650), now, app.zone).pop());
        out.extend(o);
    }
    out.sort_by_key(|o| ((o.start < now) as u8, if o.start < now { -o.start.timestamp() } else { o.start.timestamp() }));
    out
}

/// Unanswered invites from now on, each once.
fn invites(app: &App) -> Vec<Occurrence> {
    let now = app.now();
    let mut out: Vec<Occurrence> = app
        .events
        .iter()
        .filter(|e| e.my_status() == Some(Partstat::NeedsAction) && e.recurrence_id.is_none())
        .filter_map(|e| {
            let group: Vec<Event> = app.events.iter().filter(|x| x.uid == e.uid).cloned().collect();
            recur::expand(&group, now - Duration::hours(1), now + Duration::days(730), app.zone).into_iter().next()
        })
        .collect();
    out.sort_by_key(|o| o.start);
    out
}

// ---------- Drawing ----------

/// A centered floating box with a shadow and a title bar. Returns the inside.
pub(super) fn frame(f: &mut Frame, app: &App, w: u16, h: u16, title: &str, hints: &str) -> Rect {
    let area = f.area();
    let w = w.min(area.width.saturating_sub(4));
    let h = h.min(area.height.saturating_sub(2));
    let r = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 3, width: w, height: h };
    let t = &app.theme;
    if app.settings.shadow {
        let s = Rect {
            x: r.x + 2,
            y: r.y + 1,
            width: w.min(area.width.saturating_sub(r.x + 2)),
            height: h.min(area.height.saturating_sub(r.y + 1)),
        };
        let buf = f.buffer_mut();
        for y in s.y..s.y + s.height {
            for x in s.x..s.x + s.width {
                let cell = &mut buf[(x, y)];
                cell.set_bg(c(t.shadow));
                cell.set_fg(c(t.bg.mix(t.muted, 0.3)));
            }
        }
    }
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border(app))
        .border_style(Style::default().fg(c(t.accent)).bg(c(t.dialog_bg)))
        .style(Style::default().bg(c(t.dialog_bg)).fg(c(t.fg)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let bar = Rect { height: 1, ..inner };
    let buf = f.buffer_mut();
    fill(buf, bar, t.accent);
    put(
        buf,
        bar.x + 1,
        bar.y,
        title,
        bar.width.saturating_sub(2),
        Style::default().fg(c(t.on_accent)).bg(c(t.accent)).add_modifier(Modifier::BOLD),
    );
    let hw = hints.width() as u16;
    if hw > 0 && title.width() as u16 + hw + 4 < bar.width {
        put_right(buf, bar.x + bar.width - 1, bar.y, hints, hw, Style::default().fg(c(t.on_accent)).bg(c(t.accent)));
    }
    Rect { x: inner.x + 1, y: inner.y + 2, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(3) }
}

pub(super) fn draw_input(f: &mut Frame, app: &App, x: u16, y: u16, w: u16, input: &Input, active: bool) {
    let t = &app.theme;
    let buf = f.buffer_mut();
    let bg = if active { t.selection } else { t.dialog_bg };
    fill(buf, Rect { x, y, width: w, height: 1 }, bg);
    // Keep the cursor in view.
    let chars: Vec<char> = input.text.chars().collect();
    let room = w.saturating_sub(1) as usize;
    let start = input.cursor.saturating_sub(room);
    let shown: String = chars[start.min(chars.len())..].iter().collect();
    put(buf, x, y, &shown, w, Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)));
    if active {
        let cx =
            x + UnicodeWidthStr::width(chars[start.min(chars.len())..input.cursor.min(chars.len())].iter().collect::<String>().as_str())
                as u16;
        if cx < x + w {
            f.set_cursor_position((cx, y));
        }
    }
}

/// Preview line for quick add.
fn preview(app: &App, text: &str) -> String {
    if text.trim().is_empty() {
        return "Try: lunch with Sarah tomorrow 1pm".into();
    }
    match quick_event(app, text) {
        Err(m) => m,
        Ok(e) => {
            let mut s = format!("“{}” · {}", e.title, e.start.format("%a %b %-d"));
            if e.all_day {
                s += " · all day";
            } else {
                s += &format!(
                    " · {} – {}",
                    time_label(e.start.time(), app.settings.clock_24h),
                    time_label(e.end.time(), app.settings.clock_24h)
                );
            }
            if let Some(r) = &e.rrule {
                s += &format!(" · {}", recur::describe(r));
            }
            if let Some(cal) = app.calendar(&e.calendar) {
                s += &format!(" · {}", cal.name);
            }
            s
        }
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let Some(d) = app.dialog.take() else { return };
    let t = app.theme.clone();
    let fg = Style::default().fg(c(t.fg)).bg(c(t.dialog_bg));
    let muted = Style::default().fg(c(t.muted)).bg(c(t.dialog_bg));
    let accent = Style::default().fg(c(t.accent)).bg(c(t.dialog_bg));
    match &d {
        Dialog::QuickAdd { input } => {
            let r = frame(f, app, 74, 8, "Quick add", "enter save  tab more details  esc close");
            draw_input(f, app, r.x, r.y, r.width, input, true);
            let p = preview(app, &input.text);
            put(f.buffer_mut(), r.x, r.y + 2, &p, r.width, accent);
            put(f.buffer_mut(), r.x, r.y + 3, "@name picks the calendar · tab opens the full form (guests, calendar…)", r.width, muted);
        }
        Dialog::Form(form) => {
            let title = if form.target.is_some() { "Edit event" } else { "New event" };
            let rows = FIELDS.iter().filter(|(fl, _)| !form.hidden(*fl)).count() as u16;
            let r = frame(f, app, 76, rows + 6, title, "enter save  tab next  esc cancel");
            let label_w = 12;
            let mut y = r.y;
            for (i, (fl, name)) in FIELDS.iter().enumerate() {
                if form.hidden(*fl) {
                    continue;
                }
                let active = i == form.sel;
                put(f.buffer_mut(), r.x, y, name, label_w, if active { accent.add_modifier(Modifier::BOLD) } else { muted });
                let x = r.x + label_w + 1;
                let w = r.width.saturating_sub(label_w + 1);
                match fl {
                    Field::Calendar => {
                        let name = app.calendars.get(form.calendar).map(|c| c.name.clone()).unwrap_or_else(|| "none".into());
                        let bg = if active { t.selection } else { t.dialog_bg };
                        fill(f.buffer_mut(), Rect { x, y, width: w, height: 1 }, bg);
                        put(
                            f.buffer_mut(),
                            x,
                            y,
                            &format!("‹ {name} ›"),
                            w,
                            Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)),
                        );
                    }
                    Field::AllDay => {
                        let bg = if active { t.selection } else { t.dialog_bg };
                        fill(f.buffer_mut(), Rect { x, y, width: w, height: 1 }, bg);
                        put(
                            f.buffer_mut(),
                            x,
                            y,
                            if form.all_day { "[x] yes" } else { "[ ] no" },
                            w,
                            Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)),
                        );
                    }
                    _ => draw_input(f, app, x, y, w, &form.inputs[i], active),
                }
                y += 1;
            }
            let note = form.err.clone().unwrap_or_else(|| match form.field() {
                Field::Repeat => "every week · every mon and wed · every month, 5 times · until dec 1".into(),
                Field::Zone => "e.g. America/New_York · empty = your zone".into(),
                Field::Remind => "e.g. 10m, 1h, 1d · empty = none".into(),
                Field::Guests => "emails, comma between · you will be asked before guests are told".into(),
                Field::Calendar | Field::AllDay => "space or ←/→ to change".into(),
                _ if form.repeating => "Repeating event: you choose which events change when you save".into(),
                _ => String::new(),
            });
            let st = if form.err.is_some() {
                Style::default().fg(c(musinator_themes::legible(t.error, t.dialog_bg, 4.5))).bg(c(t.dialog_bg))
            } else {
                muted
            };
            put(f.buffer_mut(), r.x, y + 1, &note, r.width, st);
        }
        Dialog::Scope { pending, sel } => {
            let verb = match pending {
                Pending::Edit(..) => "Change",
                Pending::Delete(_) => "Delete",
            };
            let r = frame(f, app, 46, 9, &format!("{verb} repeating event"), "esc cancel");
            for (i, s) in [Scope::This, Scope::Following, Scope::All].iter().enumerate() {
                let y = r.y + i as u16;
                let active = i == *sel;
                let bg = if active { t.selection } else { t.dialog_bg };
                fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 1 }, bg);
                put(f.buffer_mut(), r.x, y, if active { "→" } else { " " }, 1, Style::default().fg(c(t.marker)).bg(c(bg)));
                put(
                    f.buffer_mut(),
                    r.x + 2,
                    y,
                    &format!("{}  {}", i + 1, s.label()),
                    r.width.saturating_sub(2),
                    Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)),
                );
            }
        }
        Dialog::Notify { msg, sel, .. } => {
            let r = frame(f, app, 52, 8, "Send update to guests?", "esc don't save");
            put(f.buffer_mut(), r.x, r.y, &fit(msg, r.width as usize), r.width, fg);
            put(f.buffer_mut(), r.x, r.y + 1, "This event has guests.", r.width, muted);
            for (i, label) in ["y  Send", "n  Don't send"].iter().enumerate() {
                let x = r.x + i as u16 * 16;
                let active = i == *sel;
                let bg = if active { t.selection } else { t.dialog_bg };
                put(
                    f.buffer_mut(),
                    x,
                    r.y + 3,
                    &format!(" {label} "),
                    15,
                    Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)),
                );
            }
        }
        Dialog::ConfirmDelete { title, .. } => {
            let r = frame(f, app, 52, 6, "Delete event?", "y delete  esc keep");
            put(f.buffer_mut(), r.x, r.y, &format!("“{title}”"), r.width, fg);
        }
        Dialog::Details { scroll } => {
            if let Some((o, e)) = app.selected() {
                let lines = details_lines(app, o, e, 60);
                let h = (lines.len() as u16 + 5).min(f.area().height.saturating_sub(4));
                let hints = super::draw::hints(app, &[(Action::Edit, "edit"), (Action::Delete, "delete"), (Action::Back, "close")]);
                let r = frame(f, app, 66, h, "Event", &hints);
                for (k, (l, st)) in lines.iter().skip(*scroll).enumerate() {
                    if k as u16 > r.height {
                        break;
                    }
                    put(f.buffer_mut(), r.x, r.y - 1 + k as u16, l, r.width, st.bg(c(t.dialog_bg)));
                }
            }
        }
        Dialog::Search { input, sel } => {
            let hits = search(app, &input.text);
            let r = frame(f, app, 76, 18, "Search events", "↑/↓ pick  enter go  esc close");
            draw_input(f, app, r.x, r.y, r.width, input, true);
            if hits.is_empty() && !input.text.trim().is_empty() {
                put(f.buffer_mut(), r.x, r.y + 2, "No events found", r.width, muted);
            }
            let sel = (*sel).min(hits.len().saturating_sub(1));
            for (k, o) in hits.iter().enumerate().take(r.height.saturating_sub(2) as usize) {
                let y = r.y + 2 + k as u16;
                let active = k == sel;
                let bg = if active { t.selection } else { t.dialog_bg };
                fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 1 }, bg);
                let local = tz::local(o.start, app.zone);
                let when = o
                    .days
                    .map(|(a, _)| a.format("%a %b %-d %Y").to_string())
                    .unwrap_or_else(|| format!("{} {}", local.format("%a %b %-d %Y"), time_label(local.time(), app.settings.clock_24h)));
                let title = app.event(&o.event_id).map(|e| e.title.clone()).unwrap_or_default();
                let st = Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg));
                put(f.buffer_mut(), r.x + 1, y, &title, r.width.saturating_sub(24), st);
                put_right(f.buffer_mut(), r.x + r.width, y, &when, 22, st);
            }
        }
        Dialog::GoTo { input } => {
            let r = frame(f, app, 50, 6, "Go to date", "enter go  esc close");
            draw_input(f, app, r.x, r.y, r.width, input, true);
            put(f.buffer_mut(), r.x, r.y + 1, "e.g. nov 20 · fri · 2027-01-05", r.width, muted);
        }
        Dialog::Invites { sel } => {
            let list = invites(app);
            let h = super::draw::hints(app, &[(Action::RsvpYes, "yes"), (Action::RsvpMaybe, "maybe"), (Action::RsvpNo, "no")]);
            let r = frame(f, app, 72, (list.len() as u16 * 2 + 5).clamp(7, 20), "Invites", &h);
            if list.is_empty() {
                put(f.buffer_mut(), r.x, r.y, "No open invites. All answered.", r.width, muted);
            }
            for (k, o) in list.iter().enumerate() {
                let y = r.y + k as u16 * 2;
                if y + 1 > r.y + r.height {
                    break;
                }
                let active = k == *sel;
                let bg = if active { t.selection } else { t.dialog_bg };
                fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 2 }, bg);
                let st = Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg));
                let Some(e) = app.event(&o.event_id) else { continue };
                put(f.buffer_mut(), r.x + 1, y, &e.title, r.width.saturating_sub(2), st.add_modifier(Modifier::BOLD));
                let local = tz::local(o.start, app.zone);
                let from = e.organizer.clone().unwrap_or_default();
                let line = format!("{} · {} · from {}", local.format("%a %b %-d"), span_label(o, app.zone, app.settings.clock_24h), from);
                put(f.buffer_mut(), r.x + 1, y + 1, &line, r.width.saturating_sub(2), st);
            }
        }
        Dialog::Themes(p) => super::prefs::draw_themes(f, app, p),
        Dialog::Settings(s) => super::prefs::draw_settings(f, app, s),
        Dialog::Help { scroll } => {
            let mut lines: Vec<(String, bool)> = Vec::new();
            let mut group = "";
            for i in ACTIONS {
                if i.group != group {
                    if !group.is_empty() {
                        lines.push((String::new(), false));
                    }
                    group = i.group;
                    lines.push((if group == "Invites" { "Invites (in the invites tray)".into() } else { group.to_string() }, true));
                }
                let keys: Vec<String> = app.keys.keys(i.action).iter().map(|k| k.to_string()).collect();
                lines.push((format!("  {:<14} {}", keys.join(" "), i.label), false));
            }
            lines.push((String::new(), false));
            lines.push(("Change keys: Settings → Keys".into(), true));
            let r = frame(f, app, 60, f.area().height.saturating_sub(4), "Help", "j/k scroll  esc close");
            let max = lines.len().saturating_sub(r.height as usize + 1);
            let s = (*scroll).min(max);
            for (k, (l, head)) in lines.iter().skip(s).enumerate() {
                if k as u16 > r.height {
                    break;
                }
                put(f.buffer_mut(), r.x, r.y - 1 + k as u16, l, r.width, if *head { accent.add_modifier(Modifier::BOLD) } else { fg });
            }
        }
        Dialog::Accounts(ui) => super::accounts::draw(f, app, ui),
        Dialog::Conflicts { sel } => super::accounts::draw_conflicts(f, app, *sel),
        Dialog::Quit => {
            let r = frame(f, app, 40, 5, "Quit Calinator?", "y quit  esc stay");
            put(f.buffer_mut(), r.x, r.y - 1, "Everything is saved.", r.width, muted);
        }
    }
    app.dialog = Some(d);
}
