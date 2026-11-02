//! Drawing: left pane, day/week/month/agenda, details, status bar.

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use musinator_themes::Rgb;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::theme::{calendar_color, event_colors};
use super::{App, Focus, View};
use crate::keys::Action;
use crate::model::{Event, Occurrence};
use crate::{quickadd, recur, tz};

pub fn c(x: Rgb) -> Color {
    Color::Rgb(x.0, x.1, x.2)
}

/// Cut `s` to `w` columns, adding "…" when cut.
pub fn fit(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

pub fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, w: u16, st: Style) -> u16 {
    if w == 0 {
        return x;
    }
    let s = fit(s, w as usize);
    buf.set_stringn(x, y, &s, w as usize, st).0
}

/// Right-aligned text ending at `right` (exclusive). Returns its start.
pub fn put_right(buf: &mut Buffer, right: u16, y: u16, s: &str, max: u16, st: Style) -> u16 {
    let s = fit(s, max as usize);
    let w = s.width() as u16;
    let x = right.saturating_sub(w);
    buf.set_stringn(x, y, &s, w as usize, st);
    x
}

/// Paints `area` with `bg` and clears what was drawn there.
pub fn fill(buf: &mut Buffer, area: Rect, bg: Rgb) {
    let area = area.intersection(buf.area);
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf[(x, y)].set_symbol(" ").set_bg(c(bg));
        }
    }
}

pub(super) fn border(app: &App) -> BorderType {
    if app.settings.round_corners { BorderType::Rounded } else { BorderType::Plain }
}

/// A pane: full border (accent when focused) and a filled title bar with
/// key hints. Returns the content area under the title bar.
pub(super) fn pane(f: &mut Frame, app: &App, area: Rect, title: &str, hints: &str, focused: bool) -> Rect {
    let t = &app.theme;
    let edge = if focused { t.accent } else { t.border };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border(app))
        .border_style(Style::default().fg(c(edge)).bg(c(t.bg)))
        .style(Style::default().bg(c(t.bg)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 || inner.width < 4 {
        return inner;
    }
    let bar = Rect { height: 1, ..inner };
    let (bg, tfg, hfg) = if focused { (t.accent, t.on_accent, t.on_accent) } else { (t.title_bar, t.fg, t.muted) };
    let buf = f.buffer_mut();
    fill(buf, bar, bg);
    let hw = hints.width() as u16;
    let room = bar.width.saturating_sub(2);
    let title_w = title.width() as u16;
    let show_hints = hw > 0 && title_w + hw + 4 <= room;
    put(
        buf,
        bar.x + 1,
        bar.y,
        title,
        room.saturating_sub(if show_hints { hw + 2 } else { 0 }),
        Style::default().fg(c(tfg)).bg(c(bg)).add_modifier(Modifier::BOLD),
    );
    if show_hints {
        put_right(buf, bar.x + bar.width - 1, bar.y, hints, hw, Style::default().fg(c(hfg)).bg(c(bg)));
    }
    Rect { y: inner.y + 1, height: inner.height - 1, ..inner }
}

/// "a add  x delete" from actions, with the current keys.
pub(super) fn hints(app: &App, items: &[(Action, &str)]) -> String {
    items.iter().map(|(a, l)| format!("{} {l}", app.keys.hint(*a))).collect::<Vec<_>>().join("  ")
}

pub use crate::fmt::{countdown, time_label};

/// "9am – 10:30am" for an occurrence in `zone`; "all day" for all-day ones.
pub fn span_label(o: &Occurrence, zone: chrono_tz::Tz, h24: bool) -> String {
    if let Some((a, b)) = o.days {
        let days = (b - a).num_days();
        return if days <= 1 { "all day".into() } else { format!("all day · {days} days") };
    }
    let s = tz::local(o.start, zone);
    let e = tz::local(o.end, zone);
    let mut out = format!("{} – {}", time_label(s.time(), h24), time_label(e.time(), h24));
    if e.date() != s.date() {
        out = format!("{} – {} {}", time_label(s.time(), h24), e.format("%a"), time_label(e.time(), h24));
    }
    out
}

/// Geometry of the main pane from the last draw, for mouse clicks.
#[derive(Default)]
pub struct Hits {
    /// Day columns: (x, width, day).
    pub cols: Vec<(u16, u16, NaiveDate)>,
    /// Time grid: first row y, rows, minutes per row, first minute.
    pub grid_y: u16,
    pub grid_rows: u16,
    pub step: u32,
    pub top: u32,
    /// Month cells: (rect, day).
    pub cells: Vec<(Rect, NaiveDate)>,
    /// Event blocks: (rect, occurrence index).
    pub events: Vec<(Rect, usize)>,
}

impl Hits {
    fn inside(r: &Rect, x: u16, y: u16) -> bool {
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    }

    /// Day (and time slot, in time grids) under a click.
    pub fn at(&self, x: u16, y: u16) -> Option<(NaiveDate, Option<u32>)> {
        if let Some((_, d)) = self.cells.iter().find(|(r, _)| Self::inside(r, x, y)) {
            return Some((*d, None));
        }
        let (_, _, d) = self.cols.iter().find(|(cx, w, _)| x >= *cx && x < cx + w)?;
        let slot = (y >= self.grid_y && y < self.grid_y + self.grid_rows).then(|| self.top + (y - self.grid_y) as u32 * self.step);
        Some((*d, slot))
    }

    pub fn event_at(&self, x: u16, y: u16) -> Option<usize> {
        self.events.iter().find(|(r, _)| Self::inside(r, x, y)).map(|(_, i)| *i)
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let t = app.theme.clone();
    fill(f.buffer_mut(), area, t.bg);
    f.buffer_mut().set_style(area, Style::default().fg(c(t.fg)));
    if area.height < 10 || area.width < 50 {
        put(f.buffer_mut(), 0, 0, "Window too small", area.width, Style::default().fg(c(t.muted)));
        return;
    }
    let main = Rect { height: area.height - 1, ..area };
    let status = Rect { y: area.y + area.height - 1, height: 1, ..area };
    let side_w = 28.min(main.width / 4).max(24);
    app.wide = main.width >= 140;
    if !app.wide && app.focus == Focus::Details {
        app.focus = Focus::Main;
    }
    let details_w = if app.wide { (main.width / 4).clamp(36, 48) } else { 0 };
    let side = Rect { width: side_w, ..main };
    let mid_x = main.x + side_w + 1;
    let mid_w = main.width.saturating_sub(side_w + 1 + if app.wide { details_w + 1 } else { 0 });
    let mid = Rect { x: mid_x, width: mid_w, ..main };

    left_pane(f, app, side);
    match app.view {
        View::Day | View::Week => time_grid(f, app, mid),
        View::Month => month(f, app, mid),
        View::Agenda => agenda(f, app, mid),
    }
    if app.wide {
        details_pane(f, app, Rect { x: mid_x + mid_w + 1, width: details_w, ..main });
    }
    status_bar(f, app, status);
    super::dialogs::draw(f, app);
}

// ---------- Left pane ----------

fn left_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Side;
    let h = hints(app, &[(Action::ToggleCalendar, "show")]);
    let inner = pane(f, app, area, "Calendars", if focused { &h } else { "" }, focused);
    let t = app.theme.clone();
    let today = app.today();
    let buf = f.buffer_mut();
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut y = inner.y + 1;
    let bottom = inner.y + inner.height;

    // Mini month.
    let first = app.day.with_day(1).expect("day 1");
    put(buf, x, y, &first.format("%B %Y").to_string(), w, Style::default().fg(c(t.fg)).add_modifier(Modifier::BOLD));
    y += 1;
    let names = if app.settings.week_starts_monday { "Mo Tu We Th Fr Sa Su" } else { "Su Mo Tu We Th Fr Sa" };
    put(buf, x, y, names, w, Style::default().fg(c(t.muted)));
    y += 1;
    let start = app.week_start(first);
    let (va, vb) = app.visible_days();
    for week in 0..6 {
        if y >= bottom {
            break;
        }
        let row_start = start + Duration::days(week * 7);
        if week > 0 && row_start.month() != first.month() {
            break;
        }
        for i in 0..7 {
            let d = row_start + Duration::days(i);
            let cx = x + i as u16 * 3;
            if cx + 2 > x + w {
                break;
            }
            let mut st = Style::default().fg(c(if d.month() == first.month() { t.fg } else { t.muted }));
            if d >= va && d < vb && app.view != View::Agenda {
                st = st.bg(c(t.selection_dim)).fg(c(t.selection_fg));
            }
            if d == today {
                st = st.fg(c(t.accent)).add_modifier(Modifier::BOLD);
                if d >= va && d < vb {
                    st = st.fg(c(musinator_themes::legible(t.accent, t.selection_dim, 4.5)));
                }
            }
            if d == app.day {
                st = Style::default().bg(c(t.accent)).fg(c(t.on_accent)).add_modifier(Modifier::BOLD);
            }
            put(buf, cx, y, &format!("{:>2}", d.day()), 2, st);
        }
        y += 1;
    }
    y += 1;

    // Calendars.
    if y < bottom {
        put(buf, x, y, "▾ Calendars", w, Style::default().fg(c(t.fg)).add_modifier(Modifier::BOLD));
        y += 1;
    }
    let cals = app.calendars.clone();
    for (i, cal) in cals.iter().enumerate() {
        if y >= bottom {
            break;
        }
        let sel = focused && i == app.side_sel;
        let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let bg = if sel { t.selection } else { t.bg };
        if sel {
            fill(buf, row, bg);
            put(buf, inner.x, y, "→", 1, Style::default().fg(c(t.marker)).bg(c(bg)));
        }
        let dot = if cal.enabled { "●" } else { "○" };
        let dot_fg = if cal.enabled { calendar_color(&t, &cal.color) } else { t.muted };
        put(buf, x + 1, y, dot, 1, Style::default().fg(c(musinator_themes::legible(dot_fg, bg, 3.0))).bg(c(bg)));
        let fg = if sel {
            t.selection_fg
        } else if cal.enabled {
            t.fg
        } else {
            t.muted
        };
        let mut name = cal.name.clone();
        if cal.read_only {
            name += " (read-only)";
        } else if cal.account == "local" && !app.accounts.is_empty() {
            // Next to synced calendars, say plainly which ones stay here.
            name += " (this computer)";
        }
        put(buf, x + 3, y, &name, w.saturating_sub(4), Style::default().fg(c(fg)).bg(c(bg)));
        y += 1;
    }
    y += 1;

    // Upcoming.
    if y + 1 >= bottom {
        return;
    }
    put(buf, x, y, "▾ Upcoming", w, Style::default().fg(c(t.fg)).add_modifier(Modifier::BOLD));
    y += 1;
    let now = app.now();
    let on: Vec<&str> = app.calendars.iter().filter(|c| c.enabled).map(|c| c.id.as_str()).collect();
    let shown: Vec<Event> = app.events.iter().filter(|e| on.contains(&e.calendar.as_str())).cloned().collect();
    let soon = recur::expand(&shown, now, now + Duration::days(7), app.zone);
    let soon: Vec<&Occurrence> = soon.iter().filter(|o| o.end > now && !(o.all_day && o.days.is_some_and(|(a, _)| a < today))).collect();
    if soon.is_empty() {
        put(buf, x + 1, y, "Nothing in the next 7 days", w.saturating_sub(1), Style::default().fg(c(t.muted)));
    }
    let mut sorted = soon;
    sorted.sort_by_key(|o| (o.start, o.all_day));
    for o in sorted {
        if y >= bottom {
            break;
        }
        let local = tz::local(o.start, app.zone);
        let day = o.days.map(|(a, _)| a).unwrap_or(local.date());
        let when = if day == today {
            if o.all_day { "today".into() } else { time_label(local.time(), app.settings.clock_24h) }
        } else if o.all_day {
            day.format("%a").to_string()
        } else {
            format!("{} {}", day.format("%a"), time_label(local.time(), app.settings.clock_24h))
        };
        let title = app.event(&o.event_id).map(|e| e.title.clone()).unwrap_or_default();
        let color = app.calendar(&o.calendar).map(|cal| calendar_color(&t, &cal.color)).unwrap_or(t.accent);
        put(buf, x + 1, y, "▎", 1, Style::default().fg(c(color)));
        let ww = when.width() as u16;
        put(buf, x + 3, y, &title, w.saturating_sub(ww + 5), Style::default().fg(c(t.fg)));
        put_right(buf, x + w, y, &when, ww, Style::default().fg(c(t.muted)));
        y += 1;
    }
}

// ---------- Day / week ----------

/// Side-by-side columns for overlapping timed events: (index, column, columns).
pub fn columns(spans: &[(usize, u32, u32)]) -> Vec<(usize, usize, usize)> {
    let mut v: Vec<(usize, u32, u32)> = spans.to_vec();
    v.sort_by_key(|(i, s, e)| (*s, std::cmp::Reverse(*e), *i));
    let mut out: Vec<(usize, usize, usize)> = Vec::new();
    let mut cluster: Vec<(usize, usize)> = Vec::new(); // (pos in out, col)
    let mut ends: Vec<u32> = Vec::new(); // end per column in this cluster
    let mut cluster_end = 0;
    for (i, s, e) in v {
        if !cluster.is_empty() && s >= cluster_end {
            let n = ends.len();
            for (p, _) in &cluster {
                out[*p] = (out[*p].0, out[*p].1, n);
            }
            cluster.clear();
            ends.clear();
        }
        let col = match ends.iter().position(|end| *end <= s) {
            Some(cix) => {
                ends[cix] = e;
                cix
            }
            None => {
                ends.push(e);
                ends.len() - 1
            }
        };
        cluster_end = if cluster.is_empty() { e } else { cluster_end.max(e) };
        out.push((i, col, 0));
        cluster.push((out.len() - 1, col));
    }
    let n = ends.len();
    for (p, _) in &cluster {
        out[*p] = (out[*p].0, out[*p].1, n);
    }
    out
}

/// Start and end minute of a timed occurrence on `day` (clipped to the day).
fn minutes_on(o: &Occurrence, day: NaiveDate, zone: chrono_tz::Tz) -> (u32, u32) {
    let (ds, de) = tz::day_bounds(day, zone);
    let s = o.start.max(ds);
    let e = o.end.min(de);
    let m = |t: chrono::DateTime<chrono::Utc>| {
        let l = tz::local(t, zone);
        if l.date() > day { 1440 } else { l.hour() * 60 + l.minute() }
    };
    let (a, b) = (m(s), if o.end >= de { 1440 } else { m(e) });
    (a, b.max(a))
}

fn time_grid(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let (first, last) = app.visible_days();
    let mut days: Vec<NaiveDate> = first.iter_days().take_while(|d| *d < last).collect();
    if app.view == View::Week && !app.settings.show_weekends {
        days.retain(|d| d.weekday().num_days_from_monday() < 5 || *d == app.day);
    }
    let title = match app.view {
        View::Day => app.day.format("%A, %B %-d, %Y").to_string(),
        _ => {
            let a = days[0];
            let b = *days.last().expect("days");
            if a.month() == b.month() {
                format!("Week {} · {} {} – {}, {}", a.iso_week().week(), a.format("%b"), a.day(), b.day(), b.year())
            } else {
                format!("Week {} · {} – {}", a.iso_week().week(), a.format("%b %-d"), b.format("%b %-d, %Y"))
            }
        }
    };
    let focused = app.focus == Focus::Main;
    let h = hints(
        app,
        &[
            (Action::QuickAdd, "add"),
            (Action::NewEvent, "form"),
            (Action::Open, "pick/open"),
            (Action::NextEvent, "next event"),
            (Action::Edit, "edit"),
        ],
    );
    let inner = pane(f, app, area, &title, &h, focused);
    let zone = app.zone;
    let second = app.settings.second_zone();
    let h24 = app.settings.clock_24h;
    let gutter: u16 = if h24 { 6 } else { 7 };
    let gutter2: u16 = if second.is_some() { gutter } else { 0 };
    let grid_x = inner.x + gutter + gutter2;
    let grid_w = inner.width.saturating_sub(gutter + gutter2);
    let n = days.len() as u16;
    let col_w = (grid_w / n.max(1)).max(4);
    let today = app.today();
    let buf = f.buffer_mut();

    app.hit = Hits::default();
    // Header: day names.
    let mut y = inner.y;
    if let Some(z2) = second {
        put(buf, inner.x, y, &fit(&short_zone(z2), gutter as usize - 1), gutter - 1, Style::default().fg(c(t.muted)));
        put(buf, inner.x + gutter2, y, &fit(&short_zone(zone), gutter as usize - 1), gutter - 1, Style::default().fg(c(t.muted)));
    }
    for (i, d) in days.iter().enumerate() {
        let x = grid_x + i as u16 * col_w;
        let w = if i as u16 == n - 1 { grid_x + grid_w - x } else { col_w };
        app.hit.cols.push((x, w, *d));
        let label = if app.view == View::Day { d.format("%a %-d").to_string() } else { format!("{} {}", d.format("%a"), d.day()) };
        let mut st = Style::default().fg(c(if d.weekday().num_days_from_monday() >= 5 { t.muted } else { t.fg }));
        if *d == today {
            st = Style::default().fg(c(t.accent)).add_modifier(Modifier::BOLD);
        }
        if *d == app.day && focused {
            fill(buf, Rect { x, y, width: w.saturating_sub(1), height: 1 }, t.selection_dim);
            st = st.bg(c(t.selection_dim));
            if *d != today {
                st = st.fg(c(t.selection_fg));
            } else {
                st = st.fg(c(musinator_themes::legible(t.accent, t.selection_dim, 4.5)));
            }
        }
        put(buf, x + 1, y, &label, w.saturating_sub(2), st);
    }
    y += 1;

    // All-day row.
    let all_day: Vec<Vec<usize>> = days.iter().map(|d| app.on_day(*d).into_iter().filter(|i| app.occs[*i].all_day).collect()).collect();
    let most = all_day.iter().map(Vec::len).max().unwrap_or(0);
    let all_rows = most.min(3) as u16;
    for (di, list) in all_day.iter().enumerate() {
        let (x, w, _) = app.hit.cols[di];
        for (k, i) in list.iter().enumerate().take(all_rows as usize) {
            let ry = y + k as u16;
            if k as u16 == all_rows - 1 && list.len() > all_rows as usize {
                put(buf, x + 1, ry, &format!("+{} more", list.len() - k), w.saturating_sub(2), Style::default().fg(c(t.muted)));
                break;
            }
            let o = &app.occs[*i];
            let sel = app.sel == Some(*i);
            let color = app.calendar(&o.calendar).map(|cal| cal.color.clone()).unwrap_or_default();
            let (bg, fg) = event_colors(&t, &color, sel);
            let r = Rect { x, y: ry, width: w.saturating_sub(1), height: 1 };
            fill(buf, r, bg);
            let title = app.event(&o.event_id).map(|e| e.title.clone()).unwrap_or_default();
            put(
                buf,
                x + 1,
                ry,
                &title,
                w.saturating_sub(2),
                Style::default().fg(c(fg)).bg(c(bg)).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() }),
            );
            app.hit.events.push((r, *i));
        }
    }
    if all_rows > 0 {
        put(buf, inner.x, y, "all day", gutter + gutter2, Style::default().fg(c(t.muted)));
        y += all_rows;
    }
    // Separator.
    for x in inner.x..inner.x + inner.width {
        buf[(x, y)].set_symbol("─").set_fg(c(t.border));
    }
    y += 1;

    // Grid.
    let rows = (inner.y + inner.height).saturating_sub(y);
    let step = app.slot_minutes();
    let span = rows as u32 * step;
    if app.slot < app.top {
        app.top = app.slot;
    } else if app.slot >= app.top + span {
        app.top = (app.slot + step).saturating_sub(span);
    }
    app.top = app.top.min(1440u32.saturating_sub(span)) / step * step;
    let top = app.top;
    app.hit.grid_y = y;
    app.hit.grid_rows = rows;
    app.hit.step = step;
    app.hit.top = top;

    for r in 0..rows {
        let m = top + r as u32 * step;
        let ry = y + r;
        if m.is_multiple_of(60) && m < 1440 {
            let lt = NaiveTime::from_hms_opt(m / 60, 0, 0).expect("hour");
            let label = if h24 { format!("{:02}:00", m / 60) } else { time_label(lt, false) };
            put_right(buf, inner.x + gutter2 + gutter - 1, ry, &label, gutter - 1, Style::default().fg(c(t.muted)));
            if let Some(z2) = second {
                let at = tz::resolve(app.day.and_time(lt), zone);
                let other = tz::local(at, z2).time();
                let l2 = if h24 { other.format("%H:%M").to_string() } else { time_label(other, false) };
                put_right(buf, inner.x + gutter - 1, ry, &l2, gutter - 1, Style::default().fg(c(t.muted)));
            }
            // Faint hour line.
            for (x, w, _) in &app.hit.cols {
                for xx in *x..x + w.saturating_sub(1) {
                    buf[(xx, ry)].set_symbol("┈").set_fg(c(t.bg.mix(t.border, 0.6)));
                }
            }
        }
    }
    // Cursor slot (an empty time; a picked event shows its own highlight).
    if focused
        && app.sel.is_none()
        && app.slot >= top
        && app.slot < top + span
        && let Some((x, w, _)) = app.hit.cols.iter().find(|(_, _, d)| *d == app.day)
    {
        let ry = y + ((app.slot - top) / step) as u16;
        let r = Rect { x: *x, y: ry, width: w.saturating_sub(1), height: 1 };
        fill(buf, r, t.selection_dim);
        let label = time_label(NaiveTime::from_hms_opt(app.slot / 60, app.slot % 60, 0).expect("slot"), h24);
        put(buf, x + 1, ry, &label, w.saturating_sub(2), Style::default().fg(c(t.muted)).bg(c(t.selection_dim)));
    }
    // Now line (under the events).
    let now = tz::local(app.now(), zone);
    let now_m = now.hour() * 60 + now.minute();
    let today_shown = app.hit.cols.iter().any(|(_, _, d)| *d == today);
    let now_row = (today_shown && now_m >= top && now_m < top + span).then(|| y + ((now_m - top) / step) as u16);
    if let (Some(ry), Some((x, w, _))) = (now_row, app.hit.cols.iter().find(|(_, _, d)| *d == today)) {
        for xx in *x..x + w.saturating_sub(1) {
            buf[(xx, ry)].set_symbol("─").set_fg(c(t.error));
        }
    }

    // Events.
    let cols = app.hit.cols.clone();
    for (x, w, d) in &cols {
        let timed: Vec<(usize, u32, u32)> = app
            .on_day(*d)
            .into_iter()
            .filter(|i| !app.occs[*i].all_day)
            .map(|i| {
                let (s, e) = minutes_on(&app.occs[i], *d, zone);
                (i, s, e.max(s + step))
            })
            .collect();
        let inner_w = w.saturating_sub(1);
        for (i, col, ncols) in columns(&timed) {
            let (_, s, e) = *timed.iter().find(|x| x.0 == i).expect("span");
            if e <= top || s >= top + span {
                continue;
            }
            let r0 = (s.max(top) - top) / step;
            let r1 = (e.min(top + span) - top).div_ceil(step);
            let cw = (inner_w / ncols as u16).max(1);
            let ex = x + col as u16 * cw;
            let ew = if col + 1 == ncols { x + inner_w - ex } else { cw.saturating_sub(1).max(1) };
            let r = Rect { x: ex, y: y + r0 as u16, width: ew, height: (r1 - r0).max(1) as u16 };
            let o = &app.occs[i];
            let sel = app.sel == Some(i);
            let ev = app.event(&o.event_id);
            let color = app.calendar(&o.calendar).map(|cal| cal.color.clone()).unwrap_or_default();
            let (bg, fg) = event_colors(&t, &color, sel);
            fill(buf, r, bg);
            let bar = calendar_color(&t, &color);
            for yy in r.y..r.y + r.height {
                buf[(r.x, yy)].set_symbol("▎").set_fg(c(if sel { fg } else { musinator_themes::legible(bar, bg, 3.0) }));
            }
            let st = Style::default().fg(c(fg)).bg(c(bg));
            let title = ev.map(|e| e.title.clone()).unwrap_or_default();
            let tentative = ev.is_some_and(|e| e.my_status() == Some(crate::model::Partstat::NeedsAction));
            let title = if tentative { format!("? {title}") } else { title };
            put(buf, r.x + 1, r.y, &title, r.width.saturating_sub(1), st.add_modifier(Modifier::BOLD));
            if r.height >= 2 {
                let mut sub = span_label(o, zone, h24);
                if let Some(loc) = ev.map(|e| e.location.as_str()).filter(|l| !l.is_empty()) {
                    sub = format!("{sub} · {loc}");
                }
                put(buf, r.x + 1, r.y + 1, &sub, r.width.saturating_sub(1), st);
            }
            app.hit.events.push((r, i));
        }
    }
    // Now marker in the gutter, on top.
    if let Some(ry) = now_row {
        // "10:26pm" is one column too wide for the gutter: "10:26p".
        let mut label = time_label(now.time(), h24);
        if label.width() as u16 > gutter - 1 {
            label.pop();
        }
        put_right(
            buf,
            inner.x + gutter2 + gutter - 1,
            ry,
            &label,
            gutter - 1,
            Style::default().fg(c(t.error)).add_modifier(Modifier::BOLD),
        );
    }
}

fn short_zone(z: chrono_tz::Tz) -> String {
    tz::abbrev(chrono::Utc::now(), z)
}

// ---------- Month ----------

fn month(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let title = app.day.format("%B %Y").to_string();
    let focused = app.focus == Focus::Main;
    let h = hints(
        app,
        &[
            (Action::QuickAdd, "add"),
            (Action::Open, "open"),
            (Action::PrevPeriod, "prev"),
            (Action::NextPeriod, "next"),
            (Action::Today, "today"),
        ],
    );
    let inner = pane(f, app, area, &title, &h, focused);
    let (first, _) = app.visible_days();
    let today = app.today();
    let h24 = app.settings.clock_24h;
    let zone = app.zone;
    app.hit = Hits::default();
    let buf = f.buffer_mut();
    let cw = inner.width / 7;
    for i in 0..7u16 {
        let d = first + Duration::days(i as i64);
        put(buf, inner.x + i * cw + 1, inner.y, &d.format("%a").to_string(), cw.saturating_sub(1), Style::default().fg(c(t.muted)));
    }
    let gy = inner.y + 1;
    let ch = (inner.height.saturating_sub(1)) / 6;
    for week in 0..6u16 {
        for i in 0..7u16 {
            let d = first + Duration::days((week * 7 + i) as i64);
            let x = inner.x + i * cw;
            let y = gy + week * ch;
            let w = if i == 6 { inner.x + inner.width - x } else { cw };
            let cell = Rect { x, y, width: w, height: ch };
            app.hit.cells.push((cell, d));
            let is_sel = d == app.day;
            let cell_bg = if is_sel && focused { t.selection_dim } else { t.bg };
            if is_sel {
                fill(buf, Rect { x, y, width: w.saturating_sub(1), height: ch.saturating_sub(0) }, cell_bg);
            }
            // Cell edges.
            for yy in y..y + ch {
                if i > 0 {
                    buf[(x, yy)].set_symbol("│").set_fg(c(t.border));
                }
            }
            if week > 0 {
                for xx in x..x + w {
                    buf[(xx, y)].set_symbol("─").set_fg(c(t.border));
                }
            }
            let other = d.month() != app.day.month();
            let mut st = Style::default().fg(c(if other { t.muted } else { t.fg })).bg(c(cell_bg));
            if d == today {
                st = Style::default().fg(c(t.on_accent)).bg(c(t.accent)).add_modifier(Modifier::BOLD);
            }
            let ny = if week > 0 { y + 1 } else { y };
            put(buf, x + 1, ny, &format!("{:>2}", d.day()), 2, st);
            let list = app.on_day(d);
            let room = (y + ch).saturating_sub(ny + 1);
            for (k, idx) in list.iter().enumerate() {
                let ry = ny + 1 + k as u16;
                if k as u16 >= room {
                    break;
                }
                if k as u16 == room - 1 && list.len() > room as usize {
                    put(
                        buf,
                        x + 2,
                        ry,
                        &format!("+{} more", list.len() - k),
                        w.saturating_sub(3),
                        Style::default().fg(c(t.muted)).bg(c(cell_bg)),
                    );
                    break;
                }
                let o = &app.occs[*idx];
                let sel = app.sel == Some(*idx);
                let color = app.calendar(&o.calendar).map(|cal| cal.color.clone()).unwrap_or_default();
                let title = app.event(&o.event_id).map(|e| e.title.clone()).unwrap_or_default();
                let r = Rect { x: x + 1, y: ry, width: w.saturating_sub(2), height: 1 };
                if o.all_day || sel {
                    let (bg, fg) = event_colors(&t, &color, sel);
                    fill(buf, r, bg);
                    put(buf, r.x + 1, ry, &title, r.width.saturating_sub(1), Style::default().fg(c(fg)).bg(c(bg)));
                } else {
                    let bar = musinator_themes::legible(calendar_color(&t, &color), cell_bg, 3.0);
                    put(buf, r.x, ry, "▎", 1, Style::default().fg(c(bar)).bg(c(cell_bg)));
                    let tm = time_label(tz::local(o.start, zone).time(), h24);
                    let tw = tm.width() as u16;
                    put(buf, r.x + 1, ry, &tm, tw, Style::default().fg(c(t.muted)).bg(c(cell_bg)));
                    put(buf, r.x + 2 + tw, ry, &title, r.width.saturating_sub(tw + 2), Style::default().fg(c(t.fg)).bg(c(cell_bg)));
                }
                app.hit.events.push((r, *idx));
            }
        }
    }
}

// ---------- Agenda ----------

fn agenda(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let (a, b) = app.visible_days();
    let title = format!("Agenda · {} – {}", a.format("%b %-d"), (b - Duration::days(1)).format("%b %-d, %Y"));
    let focused = app.focus == Focus::Main;
    let h = hints(app, &[(Action::QuickAdd, "add"), (Action::Open, "open"), (Action::Search, "search")]);
    let inner = pane(f, app, area, &title, &h, focused);
    let today = app.today();
    let zone = app.zone;
    let h24 = app.settings.clock_24h;
    app.hit = Hits::default();

    // Rows: day headers and events.
    enum Row {
        Day(NaiveDate),
        Ev(usize),
    }
    let mut rows = Vec::new();
    for d in a.iter_days().take_while(|d| *d < b) {
        let list = app.on_day(d);
        if list.is_empty() {
            continue;
        }
        rows.push(Row::Day(d));
        rows.extend(list.into_iter().map(Row::Ev));
    }
    let buf = f.buffer_mut();
    if rows.is_empty() {
        put(
            buf,
            inner.x + 2,
            inner.y + 1,
            "No events. Press a to add one.",
            inner.width.saturating_sub(3),
            Style::default().fg(c(t.muted)),
        );
        return;
    }
    let height = inner.height as usize;
    let sel_row = rows.iter().position(|r| matches!(r, Row::Ev(i) if Some(*i) == app.sel)).unwrap_or(0);
    let skip = sel_row.saturating_sub(height.saturating_sub(3));
    let time_w: u16 = if h24 { 13 } else { 17 };
    for (k, row) in rows.iter().enumerate().skip(skip).take(height) {
        let y = inner.y + (k - skip) as u16;
        match row {
            Row::Day(d) => {
                let label = if *d == today {
                    format!("Today · {}", d.format("%A, %B %-d"))
                } else if *d == today + Duration::days(1) {
                    format!("Tomorrow · {}", d.format("%A, %B %-d"))
                } else {
                    d.format("%A, %B %-d").to_string()
                };
                let fg = if *d == today { t.accent } else { t.fg };
                put(buf, inner.x + 1, y, &label, inner.width.saturating_sub(2), Style::default().fg(c(fg)).add_modifier(Modifier::BOLD));
            }
            Row::Ev(i) => {
                let o = &app.occs[*i];
                let sel = app.sel == Some(*i);
                let r = Rect { x: inner.x, y, width: inner.width, height: 1 };
                let bg = if sel { if focused { t.selection } else { t.selection_dim } } else { t.bg };
                fill(buf, r, bg);
                if sel {
                    put(buf, inner.x, y, "→", 1, Style::default().fg(c(t.marker)).bg(c(bg)));
                }
                let color = app.calendar(&o.calendar).map(|cal| cal.color.clone()).unwrap_or_default();
                let dot = musinator_themes::legible(calendar_color(&t, &color), bg, 3.0);
                let fg = if sel { t.selection_fg } else { t.fg };
                let time = if o.all_day { "all day".to_string() } else { span_label(o, zone, h24) };
                put(buf, inner.x + 2, y, &time, time_w, Style::default().fg(c(if sel { t.selection_fg } else { t.muted })).bg(c(bg)));
                put(buf, inner.x + 3 + time_w, y, "●", 1, Style::default().fg(c(dot)).bg(c(bg)));
                let ev = app.event(&o.event_id);
                let mut text = ev.map(|e| e.title.clone()).unwrap_or_default();
                if let Some(loc) = ev.map(|e| e.location.as_str()).filter(|l| !l.is_empty()) {
                    text = format!("{text}  · {loc}");
                }
                if o.recurring {
                    text += "  ↻";
                }
                put(
                    buf,
                    inner.x + 5 + time_w,
                    y,
                    &text,
                    inner.width.saturating_sub(time_w + 6),
                    Style::default().fg(c(fg)).bg(c(bg)).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() }),
                );
                app.hit.events.push((r, *i));
            }
        }
    }
}

// ---------- Details ----------

/// Word-wrap to `w` columns.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > w {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

/// Text lines for the details pane and popup: (text, style kind).
pub fn details_lines(app: &App, o: &Occurrence, e: &Event, width: u16) -> Vec<(String, Style)> {
    let t = &app.theme;
    let w = width as usize;
    let fg = Style::default().fg(c(t.fg));
    let muted = Style::default().fg(c(t.muted));
    let bold = fg.add_modifier(Modifier::BOLD);
    let mut out: Vec<(String, Style)> = Vec::new();
    for l in wrap(&e.title, w) {
        out.push((l, bold));
    }
    let zone = app.zone;
    let h24 = app.settings.clock_24h;
    let day = o.days.map(|(a, _)| a).unwrap_or(tz::local(o.start, zone).date());
    out.push((format!("{} · {}", day.format("%a %b %-d, %Y"), span_label(o, zone, h24)), fg));
    if !o.all_day {
        let own = recur::event_zone(e, zone);
        if own != zone {
            let s = tz::local(o.start, own);
            let en = tz::local(o.end, own);
            out.push((
                format!("{} – {} {} ({})", time_label(s.time(), h24), time_label(en.time(), h24), tz::abbrev(o.start, own), own.name()),
                muted,
            ));
        } else {
            out.push((format!("{} ({})", tz::abbrev(o.start, zone), zone.name()), muted));
        }
    }
    let rule = app.events.iter().find(|x| x.uid == e.uid && x.is_series()).and_then(|m| m.rrule.clone());
    if let Some(r) = rule {
        out.push((format!("↻ {}", recur::describe(&r)), muted));
    }
    out.push((String::new(), fg));
    if let Some(cal) = app.calendar(&e.calendar) {
        out.push((format!("Calendar  {}", cal.name), fg));
    }
    if !e.location.is_empty() {
        for (i, l) in wrap(&format!("Where     {}", e.location), w).into_iter().enumerate() {
            out.push((if i == 0 { l } else { format!("          {l}") }, fg));
        }
    }
    if let Some(v) = e.video_link() {
        out.push((
            format!("Call      {} ({} join)", fit(&v, w.saturating_sub(20)), app.keys.hint(Action::Join)),
            Style::default().fg(c(t.accent)),
        ));
    }
    if !e.reminders.is_empty() {
        let r: Vec<String> = e.reminders.iter().map(|m| format!("{} before", quickadd::reminder_text(*m))).collect();
        out.push((format!("Remind    {}", r.join(", ")), fg));
    }
    if !e.attendees.is_empty() || e.organizer.is_some() {
        out.push((String::new(), fg));
        out.push(("Guests".into(), bold));
        if let Some(org) = &e.organizer {
            // Google names the calendar itself as organizer for events made in
            // a secondary calendar: show the calendar, not its address.
            let who = if org.ends_with("@group.calendar.google.com") {
                app.calendar(&e.calendar).map(|c| format!("{} (you)", c.name)).unwrap_or_else(|| "you".into())
            } else {
                org.clone()
            };
            out.push((format!("  {who} (organizer)"), muted));
        }
        for a in &e.attendees {
            let name = if a.name.is_empty() { a.email.clone() } else { a.name.clone() };
            let me = if a.me { " (you)" } else { "" };
            let mark = match a.status {
                crate::model::Partstat::Accepted => "✓",
                crate::model::Partstat::Declined => "✗",
                crate::model::Partstat::Tentative => "?",
                crate::model::Partstat::NeedsAction => "·",
            };
            out.push((fit(&format!("  {mark} {name}{me} · {}", a.status.label()), w), fg));
        }
        if e.my_status() == Some(crate::model::Partstat::NeedsAction) {
            out.push((format!("  Answer: {} invites", app.keys.hint(Action::Invites)), Style::default().fg(c(t.accent))));
        }
    }
    if !e.description.is_empty() {
        out.push((String::new(), fg));
        for l in wrap(&e.description, w) {
            out.push((l, fg));
        }
    }
    out
}

fn details_pane(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Details;
    let h = hints(app, &[(Action::Edit, "edit"), (Action::Delete, "delete")]);
    let inner = pane(f, app, area, "Details", &h, focused);
    let t = app.theme.clone();
    let buf = f.buffer_mut();
    let Some((o, e)) = app.selected() else {
        put(buf, inner.x + 1, inner.y + 1, "No event selected", inner.width.saturating_sub(2), Style::default().fg(c(t.muted)));
        return;
    };
    let color = app.calendar(&o.calendar).map(|cal| calendar_color(&t, &cal.color)).unwrap_or(t.accent);
    let lines = details_lines(app, o, e, inner.width.saturating_sub(3));
    for (k, (l, st)) in lines.iter().enumerate() {
        let y = inner.y + 1 + k as u16;
        if y >= inner.y + inner.height {
            break;
        }
        if k == 0 {
            put(buf, inner.x + 1, y, "▎", 1, Style::default().fg(c(color)));
        }
        put(buf, inner.x + 2, y, l, inner.width.saturating_sub(3), *st);
    }
}

// ---------- Status bar ----------

fn status_bar(f: &mut Frame, app: &mut App, r: Rect) {
    let t = app.theme.clone();
    let buf = f.buffer_mut();
    fill(buf, r, t.status_bar);
    let left = match app.status.as_ref() {
        Some((msg, _)) => format!(" {msg}"),
        None => {
            // Text only: the status bar fill guarantees contrast for fg, not for colors.
            let mut parts: Vec<String> = if app.accounts.is_empty() {
                vec!["● On this computer".into()]
            } else if app.syncing {
                vec!["↻ syncing".into()]
            } else {
                app.accounts.iter().map(|a| format!("{} {}", if a.error.is_some() { "⚠" } else { "●" }, a.name)).collect()
            };
            if let Some(e) = app.accounts.iter().find_map(|a| a.error.as_ref()) {
                parts.push(fit(e, 50));
            }
            if app.pending > 0 && !app.accounts.is_empty() {
                parts.push(format!("{} waiting to sync", app.pending));
            }
            if app.conflicts > 0 {
                parts.push(format!(
                    "{} {} conflict{}",
                    app.keys.hint(Action::Conflicts),
                    app.conflicts,
                    if app.conflicts == 1 { "" } else { "s" }
                ));
            }
            let now = app.now();
            let on: Vec<&str> = app.calendars.iter().filter(|c| c.enabled).map(|c| c.id.as_str()).collect();
            let shown: Vec<Event> = app.events.iter().filter(|e| on.contains(&e.calendar.as_str())).cloned().collect();
            let next = recur::expand(&shown, now, now + Duration::days(2), app.zone)
                .into_iter()
                .filter(|o| !o.all_day && o.start >= now - Duration::minutes(5))
                .min_by_key(|o| o.start);
            if let Some(o) = next {
                let title = app.event(&o.event_id).map(|e| e.title.clone()).unwrap_or_default();
                parts.push(format!("next: {} {}", fit(&title, 30), countdown((o.start - now).num_minutes())));
            }
            let invites = app
                .events
                .iter()
                .filter(|e| e.my_status() == Some(crate::model::Partstat::NeedsAction) && e.recurrence_id.is_none())
                .count();
            if invites > 0 {
                parts.push(format!("{} {invites} invite{}", app.keys.hint(Action::Invites), if invites == 1 { "" } else { "s" }));
            }
            format!(" {}", parts.join(" · "))
        }
    };
    let mut items: Vec<String> =
        [(Action::Search, "search"), (Action::Themes, "themes"), (Action::Settings, "settings"), (Action::Help, "help")]
            .iter()
            .map(|(a, l)| format!("{} {l}", app.keys.hint(*a)))
            .collect();
    let lw = left.width() as u16;
    let mut right = items.join(" · ");
    while !items.is_empty() && lw + right.width() as u16 + 4 > r.width {
        items.pop();
        right = items.join(" · ");
    }
    let rw = right.width() as u16;
    put(buf, r.x, r.y, &left, r.width.saturating_sub(rw + 3), Style::default().fg(c(t.fg)).bg(c(t.status_bar)));
    if rw > 0 {
        put_right(buf, r.x + r.width - 1, r.y, &right, rw, Style::default().fg(c(t.muted)).bg(c(t.status_bar)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlaps_sit_side_by_side() {
        // A 9-11, B 10-12, C 11-12, D 13-14.
        let spans = [(0, 540, 660), (1, 600, 720), (2, 660, 720), (3, 780, 840)];
        let mut got = columns(&spans);
        got.sort();
        assert_eq!(got, [(0, 0, 2), (1, 1, 2), (2, 0, 2), (3, 0, 1)]);
    }
}
