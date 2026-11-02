//! Editing and deleting repeating events with three scopes: this event, this
//! and following, all events. Pure functions: they return the rows to save
//! and delete, so the same code serves the app, the CLI and sync.

use chrono::{DateTime, Datelike, Duration, NaiveDateTime, Utc};
use chrono_tz::Tz;

use crate::model::{Event, new_id};
use crate::recur::{event_zone, instance_starts};
use crate::tz;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    This,
    Following,
    All,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::This => "This event",
            Scope::Following => "This and following events",
            Scope::All => "All events",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub upsert: Vec<Event>,
    pub delete: Vec<String>,
    /// Guests may be emailed about this change (set after asking the user).
    pub notify: bool,
}

impl Changes {
    pub fn new(upsert: Vec<Event>, delete: Vec<String>) -> Self {
        Changes { upsert, delete, notify: false }
    }
}

/// What the user picked: the stored row it came from and, for repeating
/// events, the instance's original start.
#[derive(Clone, Debug)]
pub struct Target {
    pub event_id: String,
    pub original: Option<NaiveDateTime>,
}

fn find<'a>(group: &'a [Event], id: &str) -> Option<&'a Event> {
    group.iter().find(|e| e.id == id)
}

fn master(group: &[Event]) -> Option<&Event> {
    group.iter().find(|e| e.is_series())
}

fn touch(mut e: Event, now: DateTime<Utc>) -> Event {
    e.updated = now;
    e.sequence += 1;
    e
}

/// RRULE with `key` removed.
fn without(rule: &str, keys: &[&str]) -> String {
    rule.split(';').filter(|p| !keys.iter().any(|k| p.starts_with(&format!("{k}=")))).collect::<Vec<_>>().join(";")
}

fn rule_value<'a>(rule: &'a str, key: &str) -> Option<&'a str> {
    rule.split(';').find_map(|p| p.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// Moves weekday codes in `BYDAY` and days in `BYMONTHDAY` by `days`, so a
/// weekly Monday series moved to Tuesday repeats on Tuesdays.
fn shift_rule(rule: &str, days: i64) -> String {
    if days == 0 {
        return rule.to_string();
    }
    const CODES: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
    rule.split(';')
        .map(|p| {
            if let Some(v) = p.strip_prefix("BYDAY=") {
                let moved: Vec<String> = v
                    .split(',')
                    .map(|d| {
                        let (n, code) = d.split_at(d.len().saturating_sub(2));
                        match CODES.iter().position(|c| *c == code) {
                            // Ordinal days ("1MO") only move within plain weekly rules.
                            Some(i) if n.is_empty() => CODES[(i as i64 + days).rem_euclid(7) as usize].to_string(),
                            _ => d.to_string(),
                        }
                    })
                    .collect();
                format!("BYDAY={}", moved.join(","))
            } else if let Some(v) = p.strip_prefix("BYMONTHDAY=") {
                let moved: Vec<String> = v
                    .split(',')
                    .map(|d| match d.parse::<i64>() {
                        Ok(n) if n > 0 => (((n - 1 + days).rem_euclid(31)) + 1).to_string(),
                        _ => d.to_string(),
                    })
                    .collect();
                format!("BYMONTHDAY={}", moved.join(","))
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Copies the user's edits (everything but identity and series data) onto `base`.
fn apply_fields(base: &mut Event, edited: &Event) {
    base.calendar = edited.calendar.clone();
    base.title = edited.title.clone();
    base.location = edited.location.clone();
    base.description = edited.description.clone();
    base.all_day = edited.all_day;
    base.tz = edited.tz.clone();
    base.organizer = edited.organizer.clone();
    base.attendees = edited.attendees.clone();
    base.reminders = edited.reminders.clone();
    base.status = edited.status;
    base.video = edited.video.clone();
}

/// The master truncated so its last instance is before `original`.
/// Returns the truncated master and how many instances it keeps.
fn truncate(m: &Event, original: NaiveDateTime, user: Tz) -> (Event, usize) {
    let zone = event_zone(m, user);
    let cut = if m.all_day { DateTime::<Utc>::from_naive_utc_and_offset(original, Utc) } else { tz::resolve(original, zone) }
        - Duration::seconds(1);
    let kept = instance_starts(m, user, cut).len();
    let mut t = m.clone();
    if let Some(r) = &m.rrule {
        let base = without(r, &["COUNT", "UNTIL"]);
        let until = if m.all_day {
            format!("UNTIL={}", (original.date() - Duration::days(1)).format("%Y%m%d"))
        } else {
            format!("UNTIL={}Z", cut.format("%Y%m%dT%H%M%S"))
        };
        t.rrule = Some(if base.is_empty() { until } else { format!("{base};{until}") });
    }
    t.rdates.retain(|d| *d < original);
    t.exdates.retain(|d| *d < original);
    (t, kept)
}

/// Saves an edit. `edited` holds the new field values, and `start`/`end` of
/// the picked instance as wall time in `edited.tz`.
pub fn edit(group: &[Event], target: &Target, edited: &Event, scope: Scope, user: Tz, now: DateTime<Utc>) -> Changes {
    let Some(row) = find(group, &target.event_id) else { return Changes::default() };
    let series = master(group);
    let (Some(m), Some(original)) = (series, target.original) else {
        // A single event, or an override whose series is gone.
        let mut e = row.clone();
        apply_fields(&mut e, edited);
        e.start = edited.start;
        e.end = edited.end;
        e.rrule = edited.rrule.clone();
        return Changes::new(vec![touch(e, now)], vec![]);
    };

    match scope {
        Scope::This => {
            let mut o = if row.recurrence_id.is_some() {
                row.clone()
            } else {
                let mut o = m.clone();
                o.id = new_id();
                o.rrule = None;
                o.rdates.clear();
                o.exdates.clear();
                o.recurrence_id = Some(original);
                o
            };
            apply_fields(&mut o, edited);
            o.start = edited.start;
            o.end = edited.end;
            Changes::new(vec![touch(o, now)], vec![])
        }
        Scope::All => edit_all(group, m, original, edited, now),
        Scope::Following if original <= m.start => edit_all(group, m, original, edited, now),
        Scope::Following => {
            let (old, kept) = truncate(m, original, user);
            let mut tail = m.clone();
            tail.id = new_id();
            tail.uid = new_id();
            apply_fields(&mut tail, edited);
            let delta = edited.start - original;
            tail.start = edited.start;
            tail.end = edited.end;
            let rule = if edited.rrule == m.rrule { m.rrule.clone() } else { edited.rrule.clone() };
            tail.rrule = rule.map(|r| {
                let r = shift_rule(&r, delta.num_days());
                match rule_value(&r, "COUNT").and_then(|c| c.parse::<usize>().ok()) {
                    Some(c) if edited.rrule == m.rrule => format!("{};COUNT={}", without(&r, &["COUNT"]), c.saturating_sub(kept).max(1)),
                    _ => r,
                }
            });
            tail.rdates = m.rdates.iter().filter(|d| **d >= original).map(|d| *d + delta).collect();
            tail.exdates = m.exdates.iter().filter(|d| **d >= original).map(|d| *d + delta).collect();
            let mut ch = Changes::new(vec![touch(old, now), touch(tail.clone(), now)], vec![]);
            for o in group.iter().filter(|e| e.recurrence_id.is_some_and(|r| r >= original)) {
                if o.recurrence_id == Some(original) {
                    // The picked instance is now the first of the new series.
                    ch.delete.push(o.id.clone());
                    continue;
                }
                let mut moved = o.clone();
                moved.uid = tail.uid.clone();
                moved.recurrence_id = o.recurrence_id.map(|r| r + delta);
                ch.upsert.push(touch(moved, now));
            }
            ch
        }
    }
}

fn edit_all(group: &[Event], m: &Event, original: NaiveDateTime, edited: &Event, now: DateTime<Utc>) -> Changes {
    let delta = edited.start - original;
    let mut s = m.clone();
    apply_fields(&mut s, edited);
    s.start = m.start + delta;
    s.end = s.start + (edited.end - edited.start);
    s.rrule = edited.rrule.clone().map(|r| if edited.rrule == m.rrule { shift_rule(&r, delta.num_days()) } else { r });
    s.rdates = m.rdates.iter().map(|d| *d + delta).collect();
    s.exdates = m.exdates.iter().map(|d| *d + delta).collect();
    let mut ch = Changes::new(vec![touch(s, now)], vec![]);
    for o in group.iter().filter(|e| e.recurrence_id.is_some()) {
        if delta.is_zero() {
            continue;
        }
        let mut moved = o.clone();
        moved.recurrence_id = o.recurrence_id.map(|r| r + delta);
        ch.upsert.push(touch(moved, now));
    }
    ch
}

/// Deletes with a scope.
pub fn delete(group: &[Event], target: &Target, scope: Scope, user: Tz, now: DateTime<Utc>) -> Changes {
    let Some(row) = find(group, &target.event_id) else { return Changes::default() };
    let (Some(m), Some(original)) = (master(group), target.original) else {
        return Changes::new(vec![], vec![row.id.clone()]);
    };
    let all = Changes::new(vec![], group.iter().map(|e| e.id.clone()).collect());
    match scope {
        Scope::All => all,
        Scope::Following if original <= m.start => all,
        Scope::This => {
            let mut s = m.clone();
            if !s.exdates.contains(&original) {
                s.exdates.push(original);
            }
            let delete = group.iter().filter(|e| e.recurrence_id == Some(original)).map(|e| e.id.clone()).collect();
            Changes::new(vec![touch(s, now)], delete)
        }
        Scope::Following => {
            let (old, _) = truncate(m, original, user);
            let delete = group.iter().filter(|e| e.recurrence_id.is_some_and(|r| r >= original)).map(|e| e.id.clone()).collect();
            Changes::new(vec![touch(old, now)], delete)
        }
    }
}

/// Weekday of a date as an RRULE code.
pub fn weekday_code(d: chrono::NaiveDate) -> &'static str {
    ["MO", "TU", "WE", "TH", "FR", "SA", "SU"][d.weekday().num_days_from_monday() as usize]
}
