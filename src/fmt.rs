//! Short text for times, shared by the app, reminders and the CLI.

use chrono::{NaiveTime, Timelike};

/// "9:30am" / "09:30".
pub fn time_label(t: NaiveTime, h24: bool) -> String {
    if h24 {
        return t.format("%H:%M").to_string();
    }
    let (pm, h) = t.hour12();
    let ampm = if pm { "pm" } else { "am" };
    if t.minute() == 0 { format!("{h}{ampm}") } else { format!("{h}:{:02}{ampm}", t.minute()) }
}

/// "in 25m", "in 2h 5m", "now".
pub fn countdown(mins: i64) -> String {
    match mins {
        m if m <= 0 => "now".into(),
        m if m < 60 => format!("in {m}m"),
        m if m < 1440 => {
            if m % 60 == 0 {
                format!("in {}h", m / 60)
            } else {
                format!("in {}h {}m", m / 60, m % 60)
            }
        }
        m => format!("in {}d", m / 1440),
    }
}
