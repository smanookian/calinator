//! `.ics` files and `webcal:` links: import once, or subscribe (a read-only
//! calendar refreshed in the background).

use std::time::Duration;

use chrono_tz::Tz;

use crate::edit::Changes;
use crate::ical;
use crate::model::{Calendar, Event, new_id};
use crate::store::Store;

/// Subscribed calendars keep their link in `account` as `webcal:<https url>`.
pub const PREFIX: &str = "webcal:";

/// `webcal://x` → `https://x`.
pub fn http_url(link: &str) -> String {
    match link.strip_prefix("webcal://").or_else(|| link.strip_prefix("webcals://")) {
        Some(rest) => format!("https://{rest}"),
        None => link.to_string(),
    }
}

pub fn is_link(s: &str) -> bool {
    ["webcal://", "webcals://", "https://", "http://"].iter().any(|p| s.starts_with(p))
}

/// Downloads a calendar (30 s limit).
pub fn fetch(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    let mut resp = agent.get(url).call().map_err(|e| format!("could not download {url}: {e}"))?;
    resp.body_mut().with_config().limit(50 * 1024 * 1024).read_to_string().map_err(|e| format!("could not read {url}: {e}"))
}

const COLORS: [&str; 6] = ["#94e2d5", "#f5c2e7", "#74c7ec", "#f9e2af", "#cba6f7", "#a6e3a1"];

fn new_calendar(store: &Store, name: &str, account: &str, read_only: bool) -> Result<Calendar, String> {
    let n = store.calendars().map_err(|e| e.to_string())?.len();
    let cal = Calendar {
        id: new_id(),
        name: name.into(),
        color: COLORS[n % COLORS.len()].into(),
        enabled: true,
        default: false,
        account: account.into(),
        read_only,
        remote: None,
    };
    store.save_calendar(&cal).map_err(|e| e.to_string())?;
    Ok(cal)
}

/// Imports a file's events into a new calendar named after the file.
pub fn import_file(store: &mut Store, path: &std::path::Path, zone: Tz) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let fallback = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into());
    let name = ical::calendar_name(&text).unwrap_or(fallback);
    let cal = new_calendar(store, &name, "local", false)?;
    let events = ical::events(&text, &cal.id, &[], zone);
    if events.is_empty() {
        let _ = store.delete_calendar(&cal.id);
        return Err("No events found in this file".into());
    }
    let n = events.len();
    store.apply(&Changes::new(events, vec![])).map_err(|e| e.to_string())?;
    Ok(format!("Imported {n} event{} into “{name}”", if n == 1 { "" } else { "s" }))
}

/// Subscribes to a link: a read-only calendar, filled now.
pub fn subscribe(store: &mut Store, link: &str, zone: Tz) -> Result<String, String> {
    let url = http_url(link);
    if let Some(c) = store.calendars().map_err(|e| e.to_string())?.into_iter().find(|c| c.account == format!("{PREFIX}{url}")) {
        return Err(format!("Already subscribed as “{}”", c.name));
    }
    let text = fetch(&url)?;
    let name =
        ical::calendar_name(&text).unwrap_or_else(|| url.rsplit('/').next().unwrap_or("Subscribed").trim_end_matches(".ics").to_string());
    let cal = new_calendar(store, &name, &format!("{PREFIX}{url}"), true)?;
    let n = replace(store, &cal, &text, zone)?;
    Ok(format!("Subscribed to “{name}” ({n} events). It updates every hour."))
}

/// Replaces a subscribed calendar's events with the downloaded ones.
fn replace(store: &mut Store, cal: &Calendar, text: &str, zone: Tz) -> Result<usize, String> {
    let new: Vec<Event> = ical::events(text, &cal.id, &[], zone);
    let old: Vec<String> = store.events().map_err(|e| e.to_string())?.into_iter().filter(|e| e.calendar == cal.id).map(|e| e.id).collect();
    let n = new.len();
    store.apply(&Changes::new(new, old)).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Downloads every subscribed calendar again. Errors per calendar.
pub fn refresh_all(store: &mut Store, zone: Tz) -> Vec<String> {
    let cals: Vec<Calendar> = store.calendars().unwrap_or_default().into_iter().filter(|c| c.account.starts_with(PREFIX)).collect();
    let mut errors = Vec::new();
    for cal in cals {
        let url = &cal.account[PREFIX.len()..];
        if let Err(e) = fetch(url).and_then(|t| replace(store, &cal, &t, zone)) {
            errors.push(format!("{}: {e}", cal.name));
        }
    }
    errors
}
