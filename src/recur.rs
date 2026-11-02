//! Repeating events: turn stored events into the occurrences in a time window.
//! RRULE, RDATE, EXDATE and overrides; instances keep their wall-clock time
//! across DST in the event's own zone.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use rrule::RRuleSet;

use crate::model::{Event, Occurrence, Status};
use crate::tz;

/// Safety stop for rules like `FREQ=MINUTELY` far in the past.
const MAX_STEPS: usize = 200_000;

/// The zone a stored event's times are in. All-day events use UTC midnights.
pub fn event_zone(ev: &Event, user: Tz) -> Tz {
    if ev.all_day { Tz::UTC } else { tz::parse(&ev.tz, user) }
}

fn stamp(t: NaiveDateTime) -> String {
    t.format("%Y%m%dT%H%M%S").to_string()
}

/// `UNTIL` must be UTC when the start has a zone. Rewrites date-only and
/// floating values (common from servers) to the UTC instant they mean.
fn normalize_until(rule: &str, zone: Tz, all_day: bool) -> String {
    rule.split(';')
        .map(|part| {
            let Some(v) = part.strip_prefix("UNTIL=") else { return part.to_string() };
            if v.ends_with('Z') {
                return part.to_string();
            }
            let local = if let Ok(d) = NaiveDate::parse_from_str(v, "%Y%m%d") {
                d.and_hms_opt(23, 59, 59)
            } else {
                NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()
            };
            match local {
                Some(l) if all_day => format!("UNTIL={}Z", stamp(l)),
                Some(l) => format!("UNTIL={}Z", stamp(tz::resolve(l, zone).naive_utc())),
                None => part.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// The rule set of a series, ready for the `rrule` crate.
pub fn rule_set(ev: &Event, user: Tz) -> Option<RRuleSet> {
    let zone = event_zone(ev, user);
    let tzid = zone.name();
    let mut text = format!("DTSTART;TZID={tzid}:{}\n", stamp(ev.start));
    match &ev.rrule {
        Some(r) => text += &format!("RRULE:{}\n", normalize_until(r, zone, ev.all_day)),
        // RDATE-only series: a rule that yields just the start.
        None => text += "RRULE:FREQ=DAILY;COUNT=1\n",
    }
    let list = |v: &[NaiveDateTime]| v.iter().map(|t| stamp(*t)).collect::<Vec<_>>().join(",");
    if !ev.rdates.is_empty() {
        text += &format!("RDATE;TZID={tzid}:{}\n", list(&ev.rdates));
    }
    if !ev.exdates.is_empty() {
        text += &format!("EXDATE;TZID={tzid}:{}\n", list(&ev.exdates));
    }
    text.trim_end().parse().ok()
}

/// Wall-time starts of a series' instances (in the event's zone), first to
/// last, stopping after `until` (an instant).
pub fn instance_starts(ev: &Event, user: Tz, until: DateTime<Utc>) -> Vec<NaiveDateTime> {
    let Some(set) = rule_set(ev, user) else { return vec![ev.start] };
    let mut out = Vec::new();
    for t in set.into_iter().take(MAX_STEPS) {
        if t.with_timezone(&Utc) > until {
            break;
        }
        out.push(t.naive_local());
    }
    out
}

fn occurrence(ev: &Event, local: NaiveDateTime, zone: Tz, recurring: bool, original: Option<NaiveDateTime>) -> Occurrence {
    let len = ev.end - ev.start;
    if ev.all_day {
        let first = local.date();
        let days = len.num_days().max(1);
        let last = first + Duration::days(days);
        let mid = |d: NaiveDate| DateTime::<Utc>::from_naive_utc_and_offset(d.and_hms_opt(0, 0, 0).expect("midnight"), Utc);
        return Occurrence {
            event_id: ev.id.clone(),
            uid: ev.uid.clone(),
            calendar: ev.calendar.clone(),
            start: mid(first),
            end: mid(last),
            all_day: true,
            days: Some((first, last)),
            recurring,
            original,
        };
    }
    Occurrence {
        event_id: ev.id.clone(),
        uid: ev.uid.clone(),
        calendar: ev.calendar.clone(),
        start: tz::resolve(local, zone),
        end: tz::resolve(local + len, zone),
        all_day: false,
        days: None,
        recurring,
        original,
    }
}

fn overlaps(o: &Occurrence, from: DateTime<Utc>, to: DateTime<Utc>, user: Tz) -> bool {
    match o.days {
        // All-day spans whole days in the user's zone.
        Some((a, b)) => {
            let s = tz::day_bounds(a, user).0;
            let e = tz::day_bounds(b, user).0;
            s < to && e > from
        }
        None => o.start < to && (o.end > from || (o.end == o.start && o.start >= from)),
    }
}

/// Every occurrence touching `[from, to)`, sorted by start. Cancelled
/// events and instances are left out.
pub fn expand(events: &[Event], from: DateTime<Utc>, to: DateTime<Utc>, user: Tz) -> Vec<Occurrence> {
    // Overrides by series uid → original start.
    let mut overridden: HashMap<&str, HashSet<NaiveDateTime>> = HashMap::new();
    for e in events.iter().filter(|e| e.recurrence_id.is_some()) {
        overridden.entry(e.uid.as_str()).or_default().insert(e.recurrence_id.expect("checked"));
    }
    let series_uids: HashSet<&str> = events.iter().filter(|e| e.is_series()).map(|e| e.uid.as_str()).collect();

    let mut out = Vec::new();
    for ev in events {
        let zone = event_zone(ev, user);
        if ev.is_series() {
            if ev.status == Status::Cancelled {
                continue;
            }
            let skip = overridden.get(ev.uid.as_str());
            // Instances that start up to one event-length (plus a day for
            // all-day zone slack) before the window can still overlap it.
            let slack = (ev.end - ev.start) + Duration::days(1);
            let Some(set) = rule_set(ev, user) else { continue };
            for t in set.into_iter().take(MAX_STEPS) {
                let at = t.with_timezone(&Utc);
                if at >= to + Duration::days(1) {
                    break;
                }
                if at + slack < from {
                    continue;
                }
                let local = t.naive_local();
                if skip.is_some_and(|s| s.contains(&local)) {
                    continue;
                }
                let o = occurrence(ev, local, zone, true, Some(local));
                if overlaps(&o, from, to, user) {
                    out.push(o);
                }
            }
        } else {
            if ev.status == Status::Cancelled {
                continue;
            }
            let recurring = ev.recurrence_id.is_some() && series_uids.contains(ev.uid.as_str());
            let o = occurrence(ev, ev.start, zone, recurring, ev.recurrence_id);
            if overlaps(&o, from, to, user) {
                out.push(o);
            }
        }
    }
    out.sort_by_key(|o| (!o.all_day, o.start, o.end));
    out
}

/// Plain-words summary of an RRULE ("every week on Mon, Wed").
pub fn describe(rule: &str) -> String {
    let mut freq = "";
    let mut interval = 1;
    let mut days: Vec<&str> = Vec::new();
    let mut count = None;
    let mut until = None;
    for part in rule.split(';') {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        match k {
            "FREQ" => freq = v,
            "INTERVAL" => interval = v.parse().unwrap_or(1),
            "BYDAY" => days = v.split(',').collect(),
            "COUNT" => count = v.parse::<u32>().ok(),
            "UNTIL" => until = NaiveDate::parse_from_str(&v[..v.len().min(8)], "%Y%m%d").ok(),
            _ => {}
        }
    }
    let unit = match freq {
        "DAILY" => "day",
        "WEEKLY" => "week",
        "MONTHLY" => "month",
        "YEARLY" => "year",
        _ => return rule.to_string(),
    };
    let mut s = if interval == 1 { format!("every {unit}") } else { format!("every {interval} {unit}s") };
    if !days.is_empty() {
        let names: Vec<String> = days
            .iter()
            .map(|d| {
                let (n, code) = d.split_at(d.len().saturating_sub(2));
                let name = match code {
                    "MO" => "Mon",
                    "TU" => "Tue",
                    "WE" => "Wed",
                    "TH" => "Thu",
                    "FR" => "Fri",
                    "SA" => "Sat",
                    "SU" => "Sun",
                    _ => code,
                };
                match n {
                    "" => name.to_string(),
                    "-1" => format!("last {name}"),
                    n => format!("{n}. {name}"),
                }
            })
            .collect();
        s += &format!(" on {}", names.join(", "));
    }
    if let Some(c) = count {
        s += &format!(", {c} times");
    }
    if let Some(u) = until {
        s += &format!(", until {}", u.format("%b %-d %Y"));
    }
    s
}
