//! Which reminders are due. Each fires exactly once, from the app or the
//! daemon, whoever runs (the store remembers what was shown).

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;

use crate::model::{Event, Partstat};
use crate::notify::{Click, Notifier, Sent};
use crate::settings::Settings;
use crate::store::Store;
use crate::{recur, tz};

/// A reminder missed while the computer slept still fires if the event has
/// not ended and the reminder is this recent.
const CATCH_UP: i64 = 6;

/// Snooze length in minutes.
pub const SNOOZE_MINUTES: i64 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Due {
    pub event_id: String,
    pub start: DateTime<Utc>,
    pub title: String,
    pub body: String,
    pub video: Option<String>,
    /// Unique per event, occurrence and reminder, so a moved event fires again.
    pub key: String,
    pub at: DateTime<Utc>,
}

/// "in 10 min · 9:30am – 9:45am · Room 4B".
fn body(e: &Event, start: DateTime<Utc>, end: DateTime<Utc>, all_day: bool, now: DateTime<Utc>, zone: Tz, h24: bool) -> String {
    let mins = (start - now).num_minutes();
    let when = if all_day {
        "today, all day".to_string()
    } else if mins <= 0 {
        "now".to_string()
    } else {
        crate::fmt::countdown(mins)
    };
    let mut parts = vec![when];
    if !all_day {
        let s = tz::local(start, zone).time();
        let en = tz::local(end, zone).time();
        parts.push(format!("{} – {}", crate::fmt::time_label(s, h24), crate::fmt::time_label(en, h24)));
    }
    if !e.location.is_empty() {
        parts.push(e.location.clone());
    }
    parts.join(" · ")
}

/// Reminders and snoozes whose moment passed (recently) up to `now`.
pub fn due(events: &[Event], snoozes: &[(String, DateTime<Utc>)], s: &Settings, now: DateTime<Utc>) -> Vec<Due> {
    let zone = s.zone();
    let horizon = events.iter().flat_map(|e| e.reminders.iter()).copied().max().unwrap_or(0).max(0);
    let occs = recur::expand(events, now - Duration::hours(CATCH_UP), now + Duration::minutes(horizon) + Duration::days(1), zone);
    let mut out = Vec::new();
    for o in occs {
        let Some(e) = events.iter().find(|e| e.id == o.event_id) else { continue };
        if e.my_status() == Some(Partstat::Declined) || o.end <= now {
            continue;
        }
        // All-day events: reminders count from 9am of the day.
        let base = match o.days {
            Some((d, _)) => tz::resolve(d.and_hms_opt(9, 0, 0).expect("9am"), zone),
            None => o.start,
        };
        let mut push = |key: String, at: DateTime<Utc>| {
            out.push(Due {
                event_id: e.id.clone(),
                start: o.start,
                title: e.title.clone(),
                body: body(e, o.start, o.end, o.all_day, now, zone, s.clock_24h),
                video: e.video_link(),
                key,
                at,
            })
        };
        for m in &e.reminders {
            let at = base - Duration::minutes(*m);
            if at <= now && at > now - Duration::hours(CATCH_UP) {
                push(format!("{}@{}#{m}", e.id, o.start.to_rfc3339()), at);
            }
        }
        let skey = snooze_key(&e.id, o.start);
        for (k, until) in snoozes {
            if *k == skey && *until <= now {
                push(format!("{skey}@{}", until.to_rfc3339()), *until);
            }
        }
    }
    out.sort_by_key(|d| d.at);
    out
}

pub fn snooze_key(event_id: &str, start: DateTime<Utc>) -> String {
    format!("snooze:{event_id}@{}", start.to_rfc3339())
}

/// Shows reminders that are due and not shown yet. Returns how many.
pub fn check(store: &Store, notifier: &Notifier, now: DateTime<Utc>) -> Result<usize, String> {
    let s = Settings::load(store);
    if !s.notifications {
        return Ok(0);
    }
    let on: Vec<String> = store.calendars().map_err(|e| e.to_string())?.into_iter().filter(|c| c.enabled).map(|c| c.id).collect();
    let events: Vec<Event> = store.events().map_err(|e| e.to_string())?.into_iter().filter(|e| on.contains(&e.calendar)).collect();
    let snoozes = store.snoozes().map_err(|e| e.to_string())?;
    let mut shown = 0;
    for d in due(&events, &snoozes, &s, now) {
        // Claim first, so the app and the daemon never both show it.
        if !store.claim_fired(&d.key, now).map_err(|e| e.to_string())? {
            continue;
        }
        let what = Sent { event_id: d.event_id.clone(), start: d.start.to_rfc3339(), video: d.video.clone() };
        notifier.send(what, &d.title, &d.body, SNOOZE_MINUTES)?;
        shown += 1;
    }
    store.prune_fired(now - Duration::days(30)).map_err(|e| e.to_string())?;
    Ok(shown)
}

/// Handles a click on a notification.
pub fn on_click(store: &Store, what: &Sent, click: Click, now: DateTime<Utc>) -> Result<String, String> {
    match click {
        Click::Snooze => {
            let start = DateTime::parse_from_rfc3339(&what.start).map_err(|e| e.to_string())?.with_timezone(&Utc);
            store.add_snooze(&snooze_key(&what.event_id, start), now + Duration::minutes(SNOOZE_MINUTES)).map_err(|e| e.to_string())?;
            Ok(format!("snoozed for {SNOOZE_MINUTES} min"))
        }
        Click::Join => {
            let url = what.video.as_deref().ok_or("no call link")?;
            std::process::Command::new("xdg-open").arg(url).spawn().map_err(|e| e.to_string())?;
            Ok("opened the call".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    fn ev(start: &str, reminders: Vec<i64>) -> Event {
        let s = NaiveDateTime::parse_from_str(start, "%Y-%m-%d %H:%M").unwrap();
        Event {
            id: "e".into(),
            uid: "u".into(),
            title: "Standup".into(),
            start: s,
            end: s + Duration::minutes(30),
            tz: "UTC".into(),
            reminders,
            ..Default::default()
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap().and_utc()
    }

    fn settings() -> Settings {
        Settings { timezone: "UTC".into(), ..Default::default() }
    }

    #[test]
    fn fires_after_its_moment_and_only_while_the_event_has_not_ended() {
        let e = ev("2026-11-02 09:30", vec![10]);
        assert!(due(std::slice::from_ref(&e), &[], &settings(), at("2026-11-02 09:19")).is_empty());
        let d = due(std::slice::from_ref(&e), &[], &settings(), at("2026-11-02 09:20"));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].body.split(" · ").next(), Some("in 10m"));
        assert!(due(&[e], &[], &settings(), at("2026-11-02 10:00")).is_empty());
    }

    #[test]
    fn each_reminder_fires_once_across_processes() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.claim_fired("k", at("2026-11-02 09:20")).unwrap());
        assert!(!s.claim_fired("k", at("2026-11-02 09:20")).unwrap());
    }

    #[test]
    fn snooze_fires_again_later() {
        let e = ev("2026-11-02 09:30", vec![10]);
        let key = snooze_key("e", at("2026-11-02 09:30"));
        let sn = vec![(key, at("2026-11-02 09:25"))];
        assert_eq!(due(std::slice::from_ref(&e), &sn, &settings(), at("2026-11-02 09:24")).len(), 1);
        assert_eq!(due(&[e], &sn, &settings(), at("2026-11-02 09:25")).len(), 2);
    }
}
