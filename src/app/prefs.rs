//! Settings (every setting, calendars and keys) and the live theme picker.

use crossterm::event::{KeyCode, KeyEvent};
use musinator_themes::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::dialogs::{Dialog, frame};
use super::draw::{c, fill, put, put_right};
use super::input::Input;
use super::{App, theme};
use crate::keys::{ACTIONS, Action, Chord, chord_id};
use crate::model::{Calendar, new_id};

// ---------- Themes ----------

pub struct ThemePicker {
    themes: Vec<Theme>,
    sel: usize,
    /// Theme id to go back to on esc.
    orig: String,
}

impl ThemePicker {
    pub fn new(app: &App) -> Self {
        let themes = theme::all();
        let sel = themes.iter().position(|t| t.id == app.settings.theme).unwrap_or(0);
        ThemePicker { themes, sel, orig: app.settings.theme.clone() }
    }
}

pub fn theme_key(app: &mut App, mut p: ThemePicker, k: KeyEvent) -> Option<Dialog> {
    let n = p.themes.len();
    if app.keys.is(Action::Back, &k) {
        app.theme = theme::load(&p.orig);
        return None;
    }
    if app.keys.is(Action::Open, &k) {
        app.settings.theme = p.themes[p.sel].id.clone();
        app.save_settings();
        app.flash(format!("Theme: {}", p.themes[p.sel].label));
        return None;
    }
    if app.keys.is(Action::Down, &k) {
        p.sel = (p.sel + 1) % n;
    } else if app.keys.is(Action::Up, &k) {
        p.sel = (p.sel + n - 1) % n;
    } else {
        return Some(Dialog::Themes(p));
    }
    // Live preview.
    app.theme = p.themes[p.sel].clone();
    Some(Dialog::Themes(p))
}

pub fn draw_themes(f: &mut Frame, app: &App, p: &ThemePicker) {
    let t = &app.theme;
    let h = (p.themes.len() as u16 + 5).min(f.area().height.saturating_sub(4));
    let r = frame(f, app, 44, h, "Themes", "enter keep  esc back");
    let rows = r.height as usize + 1;
    let skip = p.sel.saturating_sub(rows.saturating_sub(2));
    for (k, th) in p.themes.iter().enumerate().skip(skip).take(rows) {
        let y = r.y - 1 + (k - skip) as u16;
        let active = k == p.sel;
        let bg = if active { t.selection } else { t.dialog_bg };
        fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 1 }, bg);
        put(f.buffer_mut(), r.x, y, if active { "→" } else { " " }, 1, Style::default().fg(c(t.marker)).bg(c(bg)));
        let label = if th.id == musinator_themes::omarchy::ID { format!("{} (live)", th.label) } else { th.label.clone() };
        put(
            f.buffer_mut(),
            r.x + 2,
            y,
            &label,
            r.width.saturating_sub(10),
            Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg)),
        );
        // Swatch.
        for (i, col) in [th.bg, th.accent, th.fg, th.good].iter().enumerate() {
            put(f.buffer_mut(), r.x + r.width - 8 + i as u16 * 2, y, "██", 2, Style::default().fg(c(*col)).bg(c(bg)));
        }
    }
}

// ---------- Settings ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Theme,
    StartView,
    WeekStart,
    Clock,
    RowsPerHour,
    DayStart,
    Zone,
    SecondZone,
    Length,
    Reminder,
    Weekends,
    AgendaDays,
    Notifications,
    Bar,
    Corners,
    Shadow,
    ConfirmQuit,
    Keys,
    Calendar(usize),
    NewCalendar,
}

const COLORS: [&str; 10] = ["#89b4fa", "#a6e3a1", "#f38ba8", "#fab387", "#f9e2af", "#cba6f7", "#94e2d5", "#f5c2e7", "#74c7ec", "#b4befe"];
const REMINDERS: [i64; 9] = [-1, 0, 5, 10, 15, 30, 60, 120, 1440];

#[derive(Default)]
pub struct SettingsUi {
    sel: usize,
    /// Text being typed for a zone or a new calendar name.
    editing: Option<Input>,
    /// Keys page.
    keys: Option<KeysUi>,
}

struct KeysUi {
    sel: usize,
    /// Waiting for the new key.
    capture: bool,
    note: Option<String>,
}

fn rows(app: &App) -> Vec<Row> {
    let mut v = vec![
        Row::Theme,
        Row::StartView,
        Row::WeekStart,
        Row::Clock,
        Row::RowsPerHour,
        Row::DayStart,
        Row::Zone,
        Row::SecondZone,
        Row::Length,
        Row::Reminder,
        Row::Weekends,
        Row::AgendaDays,
        Row::Notifications,
        Row::Bar,
        Row::Corners,
        Row::Shadow,
        Row::ConfirmQuit,
        Row::Keys,
    ];
    v.extend((0..app.calendars.len()).map(Row::Calendar));
    v.push(Row::NewCalendar);
    v
}

fn yes(b: bool) -> String {
    if b { "on".into() } else { "off".into() }
}

fn label(app: &App, r: Row) -> (String, String) {
    let s = &app.settings;
    match r {
        Row::Theme => ("Theme".into(), app.theme.label.clone()),
        Row::StartView => ("Start in".into(), format!("{} view", s.start_view)),
        Row::WeekStart => ("Week starts on".into(), if s.week_starts_monday { "Monday".into() } else { "Sunday".into() }),
        Row::Clock => ("Clock".into(), if s.clock_24h { "24-hour".into() } else { "12-hour (am/pm)".into() }),
        Row::RowsPerHour => ("Rows per hour".into(), s.rows_per_hour.to_string()),
        Row::DayStart => ("Day view starts at".into(), format!("{}:00", s.day_start_hour)),
        Row::Zone => {
            ("Time zone".into(), if s.timezone.is_empty() { format!("computer ({})", app.zone.name()) } else { s.timezone.clone() })
        }
        Row::SecondZone => ("Second time zone".into(), if s.second_timezone.is_empty() { "off".into() } else { s.second_timezone.clone() }),
        Row::Length => ("New event length".into(), format!("{} min", s.default_duration)),
        Row::Reminder => (
            "Default reminder".into(),
            if s.default_reminder < 0 { "none".into() } else { format!("{} before", crate::quickadd::reminder_text(s.default_reminder)) },
        ),
        Row::Weekends => ("Weekends in week view".into(), yes(s.show_weekends)),
        Row::AgendaDays => ("Agenda shows".into(), format!("{} days", s.agenda_days)),
        Row::Notifications => ("Desktop notifications".into(), yes(s.notifications)),
        Row::Bar => ("Show in Omarchy bar".into(), yes(s.show_in_bar)),
        Row::Corners => ("Round corners".into(), yes(s.round_corners)),
        Row::Shadow => ("Dialog shadow".into(), yes(s.shadow)),
        Row::ConfirmQuit => ("Ask before quitting".into(), yes(s.confirm_quit)),
        Row::Keys => ("Keys".into(), "change…".into()),
        Row::Calendar(i) => {
            let cal = &app.calendars[i];
            (format!("  {}", cal.name), if cal.default { "default".into() } else { String::new() })
        }
        Row::NewCalendar => ("  + New calendar".into(), String::new()),
    }
}

fn cycle<T: PartialEq + Copy>(list: &[T], cur: T, d: i32) -> T {
    let i = list.iter().position(|x| *x == cur).unwrap_or(0) as i32;
    list[(i + d).rem_euclid(list.len() as i32) as usize]
}

/// Changes a row by `d` (←/→, space, enter).
fn change(app: &mut App, ui: &mut SettingsUi, r: Row, d: i32) -> Option<Dialog> {
    let s = &mut app.settings;
    match r {
        Row::Theme => return Some(Dialog::Themes(ThemePicker::new(app))),
        Row::StartView => {
            let views = ["week", "day", "month", "agenda"];
            let i = views.iter().position(|v| *v == s.start_view).unwrap_or(0) as i32;
            s.start_view = views[(i + d).rem_euclid(4) as usize].into();
        }
        Row::WeekStart => s.week_starts_monday = !s.week_starts_monday,
        Row::Clock => s.clock_24h = !s.clock_24h,
        Row::RowsPerHour => s.rows_per_hour = cycle(&[1, 2, 4], s.rows_per_hour, d),
        Row::DayStart => s.day_start_hour = (s.day_start_hour as i32 + d).rem_euclid(24) as u32,
        Row::Zone | Row::SecondZone => {
            let cur = if r == Row::Zone { s.timezone.clone() } else { s.second_timezone.clone() };
            ui.editing = Some(Input::new(&cur));
            return None;
        }
        Row::Length => s.default_duration = (s.default_duration + 15 * d as i64).clamp(15, 480),
        Row::Reminder => s.default_reminder = cycle(&REMINDERS, s.default_reminder, d),
        Row::Weekends => s.show_weekends = !s.show_weekends,
        Row::AgendaDays => s.agenda_days = (s.agenda_days + 7 * d as i64).clamp(7, 91),
        Row::Notifications => s.notifications = !s.notifications,
        Row::Bar => s.show_in_bar = !s.show_in_bar,
        Row::Corners => s.round_corners = !s.round_corners,
        Row::Shadow => s.shadow = !s.shadow,
        Row::ConfirmQuit => s.confirm_quit = !s.confirm_quit,
        Row::Keys => {
            ui.keys = Some(KeysUi { sel: 0, capture: false, note: None });
            return None;
        }
        Row::Calendar(i) => {
            if d == 0 {
                // Enter: make default.
                for (k, cal) in app.calendars.iter_mut().enumerate() {
                    cal.default = k == i;
                }
            } else {
                let cal = &mut app.calendars[i];
                cal.color = cycle(&COLORS, COLORS.iter().copied().find(|x| *x == cal.color).unwrap_or(COLORS[0]), d).into();
            }
            let cals = app.calendars.clone();
            for cal in &cals {
                let _ = app.store.save_calendar(cal);
            }
            app.refresh();
            return None;
        }
        Row::NewCalendar => {
            ui.editing = Some(Input::default());
            return None;
        }
    }
    app.apply_settings();
    None
}

pub fn settings_key(app: &mut App, mut ui: SettingsUi, k: KeyEvent) -> Option<Dialog> {
    if let Some(mut keys) = ui.keys.take() {
        if keys_key(app, &mut keys, k) {
            ui.keys = Some(keys);
        }
        return Some(Dialog::Settings(ui));
    }
    let list = rows(app);
    let row = list[ui.sel.min(list.len() - 1)];
    if let Some(mut input) = ui.editing.take() {
        match k.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                let text = input.text.trim().to_string();
                match row {
                    Row::Zone | Row::SecondZone => {
                        if !text.is_empty() && text.parse::<chrono_tz::Tz>().is_err() {
                            app.flash(format!("Unknown time zone “{text}”. Try “Europe/Berlin”"));
                            ui.editing = Some(input);
                            return Some(Dialog::Settings(ui));
                        }
                        if row == Row::Zone {
                            app.settings.timezone = text;
                        } else {
                            app.settings.second_timezone = text;
                        }
                        app.apply_settings();
                    }
                    Row::NewCalendar if !text.is_empty() => {
                        let cal = Calendar {
                            id: new_id(),
                            name: text.clone(),
                            color: COLORS[app.calendars.len() % COLORS.len()].into(),
                            enabled: true,
                            default: app.calendars.is_empty(),
                            account: "local".into(),
                            read_only: false,
                            remote: None,
                        };
                        let _ = app.store.save_calendar(&cal);
                        app.reload();
                        app.flash(format!("Added calendar “{text}”"));
                    }
                    _ => {}
                }
            }
            _ => {
                input.key(&k);
                ui.editing = Some(input);
            }
        }
        return Some(Dialog::Settings(ui));
    }
    if app.keys.is(Action::Back, &k) || app.keys.is(Action::Settings, &k) {
        return None;
    }
    if app.keys.is(Action::Down, &k) {
        ui.sel = (ui.sel + 1) % list.len();
    } else if app.keys.is(Action::Up, &k) {
        ui.sel = (ui.sel + list.len() - 1) % list.len();
    } else if app.keys.is(Action::Right, &k) || k.code == KeyCode::Char(' ') {
        if let Some(d) = change(app, &mut ui, row, 1) {
            return Some(d);
        }
    } else if app.keys.is(Action::Left, &k) {
        if let Some(d) = change(app, &mut ui, row, -1) {
            return Some(d);
        }
    } else if app.keys.is(Action::Open, &k) {
        if let Some(d) = change(app, &mut ui, row, if matches!(row, Row::Calendar(_)) { 0 } else { 1 }) {
            return Some(d);
        }
    } else if app.keys.is(Action::Delete, &k)
        && let Row::Calendar(i) = row
    {
        let cal = app.calendars[i].clone();
        if app.events.iter().any(|e| e.calendar == cal.id) {
            app.flash(format!("“{}” has events; delete or move them first", cal.name));
        } else {
            let _ = app.store.delete_calendar(&cal.id);
            app.reload();
            app.flash(format!("Deleted calendar “{}”", cal.name));
        }
    }
    Some(Dialog::Settings(ui))
}

/// Handles a key on the keys page; false closes the page.
fn keys_key(app: &mut App, ui: &mut KeysUi, k: KeyEvent) -> bool {
    let info = &ACTIONS[ui.sel];
    if ui.capture {
        ui.capture = false;
        if k.code == KeyCode::Esc {
            ui.note = None;
            return true;
        }
        let chord = Chord::from_event(&k);
        if let Some(other) = app.keys.conflict(&chord, info.action) {
            ui.note = Some(format!("{chord} is used by “{}”. Pick another key.", crate::keys::info(other).label));
            return true;
        }
        app.settings.keys.insert(info.id.into(), vec![chord_id(&chord)]);
        app.apply_settings();
        ui.note = Some(format!("{} → {chord}", info.label));
        return true;
    }
    match k.code {
        KeyCode::Esc => return false,
        KeyCode::Down | KeyCode::Char('j') => ui.sel = (ui.sel + 1) % ACTIONS.len(),
        KeyCode::Up | KeyCode::Char('k') => ui.sel = (ui.sel + ACTIONS.len() - 1) % ACTIONS.len(),
        KeyCode::Enter => {
            ui.capture = true;
            ui.note = Some(format!("Press the new key for “{}” (esc cancels)", info.label));
        }
        KeyCode::Backspace => {
            app.settings.keys.remove(info.id);
            app.apply_settings();
            ui.note = Some(format!("{}: back to default", info.label));
        }
        _ => {}
    }
    true
}

pub fn draw_settings(f: &mut Frame, app: &App, ui: &SettingsUi) {
    let t = &app.theme;
    let h = f.area().height.saturating_sub(4).min(32);
    if let Some(keys) = &ui.keys {
        let r = frame(f, app, 64, h, "Settings · Keys", "enter change  backspace default  esc back");
        let rows = r.height.saturating_sub(1) as usize;
        let skip = keys.sel.saturating_sub(rows.saturating_sub(2));
        for (k, info) in ACTIONS.iter().enumerate().skip(skip).take(rows) {
            let y = r.y - 1 + (k - skip) as u16;
            let active = k == keys.sel;
            let bg = if active { t.selection } else { t.dialog_bg };
            fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 1 }, bg);
            let st = Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg));
            put(f.buffer_mut(), r.x + 1, y, &format!("{:<9} {}", info.group, info.label), r.width.saturating_sub(18), st);
            let ks: Vec<String> = app.keys.keys(info.action).iter().map(|c| c.to_string()).collect();
            let custom = app.settings.keys.contains_key(info.id);
            put_right(f.buffer_mut(), r.x + r.width, y, &format!("{}{}", ks.join(" "), if custom { " *" } else { "" }), 16, st);
        }
        if let Some(n) = &keys.note {
            put(f.buffer_mut(), r.x, r.y + r.height, n, r.width, Style::default().fg(c(t.accent)).bg(c(t.dialog_bg)));
        }
        return;
    }
    let list = rows(app);
    let r = frame(f, app, 64, h, "Settings", "←/→ change  enter pick  esc close");
    let n = r.height as usize;
    let skip = ui.sel.saturating_sub(n.saturating_sub(2));
    let mut cal_header_drawn = false;
    let mut y = r.y - 1;
    for (k, row) in list.iter().enumerate().skip(skip) {
        if y >= r.y + r.height {
            break;
        }
        if matches!(row, Row::Calendar(_) | Row::NewCalendar) && !cal_header_drawn {
            cal_header_drawn = true;
            put(
                f.buffer_mut(),
                r.x,
                y,
                "Calendars (←/→ color · enter default · x delete)",
                r.width,
                Style::default().fg(c(t.accent)).bg(c(t.dialog_bg)).add_modifier(Modifier::BOLD),
            );
            y += 1;
        }
        let active = k == ui.sel;
        let bg = if active { t.selection } else { t.dialog_bg };
        fill(f.buffer_mut(), Rect { x: r.x, y, width: r.width, height: 1 }, bg);
        let st = Style::default().fg(c(if active { t.selection_fg } else { t.fg })).bg(c(bg));
        let (name, value) = label(app, *row);
        put(f.buffer_mut(), r.x, y, if active { "→" } else { " " }, 1, Style::default().fg(c(t.marker)).bg(c(bg)));
        put(f.buffer_mut(), r.x + 2, y, &name, 28, st);
        if let Row::Calendar(i) = row {
            let col = theme::calendar_color(t, &app.calendars[*i].color);
            put(f.buffer_mut(), r.x + 1, y, "●", 1, Style::default().fg(c(musinator_themes::legible(col, bg, 3.0))).bg(c(bg)));
        }
        if active && let Some(input) = &ui.editing {
            let x = r.x + 31;
            let w = r.width.saturating_sub(31);
            fill(f.buffer_mut(), Rect { x, y, width: w, height: 1 }, t.dialog_bg);
            put(f.buffer_mut(), x, y, &input.text, w, Style::default().fg(c(t.fg)).bg(c(t.dialog_bg)).add_modifier(Modifier::UNDERLINED));
            f.set_cursor_position((x + unicode_width::UnicodeWidthStr::width(input.text.as_str()) as u16, y));
        } else {
            put_right(f.buffer_mut(), r.x + r.width, y, &value, r.width.saturating_sub(31), st);
        }
        y += 1;
    }
}
