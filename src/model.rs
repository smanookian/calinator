//! Calendars, events and the occurrences shown on screen.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    /// Shown in the views.
    pub enabled: bool,
    /// New events go here.
    pub default: bool,
    /// Account id it belongs to; `local` for calendars that live only here,
    /// `webcal:<url>` for subscriptions.
    pub account: String,
    pub read_only: bool,
    /// The calendar on the server (CalDAV URL, Google id) for synced calendars.
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Confirmed,
    Tentative,
    Cancelled,
}

/// A guest's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Partstat {
    #[default]
    NeedsAction,
    Accepted,
    Tentative,
    Declined,
}

impl Partstat {
    pub fn label(self) -> &'static str {
        match self {
            Partstat::NeedsAction => "no answer",
            Partstat::Accepted => "yes",
            Partstat::Tentative => "maybe",
            Partstat::Declined => "no",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attendee {
    pub email: String,
    pub name: String,
    pub status: Partstat,
    /// This attendee is the user (their own answer).
    pub me: bool,
}

/// One stored event: a single event, a repeating series (has `rrule` or
/// `rdates`), or an override of one series instance (has `recurrence_id`).
///
/// Times are wall-clock times in `tz` (an IANA name). All-day events use
/// midnight and an exclusive end date, and ignore `tz`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Event {
    /// Row id, unique per stored event.
    pub id: String,
    /// iCalendar UID: shared by a series and its overrides.
    pub uid: String,
    pub calendar: String,
    pub title: String,
    pub location: String,
    pub description: String,
    pub all_day: bool,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub tz: String,
    /// RRULE value without the `RRULE:` prefix.
    pub rrule: Option<String>,
    pub rdates: Vec<NaiveDateTime>,
    pub exdates: Vec<NaiveDateTime>,
    /// Override: the original start of the instance it replaces (wall time
    /// in the series' zone).
    pub recurrence_id: Option<NaiveDateTime>,
    pub organizer: Option<String>,
    pub attendees: Vec<Attendee>,
    /// Minutes before start.
    pub reminders: Vec<i64>,
    pub status: Status,
    /// Video call link (Meet, Zoom, Teams…).
    pub video: Option<String>,
    pub updated: DateTime<Utc>,
    pub sequence: u32,
}

impl Default for Event {
    fn default() -> Self {
        let midnight = NaiveDate::from_ymd_opt(2000, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap_or_default();
        Event {
            id: String::new(),
            uid: String::new(),
            calendar: String::new(),
            title: String::new(),
            location: String::new(),
            description: String::new(),
            all_day: false,
            start: midnight,
            end: midnight,
            tz: "UTC".into(),
            rrule: None,
            rdates: Vec::new(),
            exdates: Vec::new(),
            recurrence_id: None,
            organizer: None,
            attendees: Vec::new(),
            reminders: Vec::new(),
            status: Status::Confirmed,
            video: None,
            updated: DateTime::<Utc>::default(),
            sequence: 0,
        }
    }
}

impl Event {
    pub fn is_series(&self) -> bool {
        self.recurrence_id.is_none() && (self.rrule.is_some() || !self.rdates.is_empty())
    }

    /// Has guests other than the user.
    pub fn has_guests(&self) -> bool {
        self.attendees.iter().any(|a| !a.me)
    }

    /// The user's own answer, when they are a guest (not the organizer).
    pub fn my_status(&self) -> Option<Partstat> {
        self.attendees.iter().find(|a| a.me).map(|a| a.status)
    }

    /// A video link from the `video` field, else the first one found in location or description.
    pub fn video_link(&self) -> Option<String> {
        if let Some(v) = &self.video {
            return Some(v.clone());
        }
        const HOSTS: [&str; 5] = ["meet.google.com", "zoom.us", "teams.microsoft.com", "whereby.com", "meet.jit.si"];
        [&self.location, &self.description].into_iter().flat_map(|t| t.split_whitespace()).find_map(|w| {
            let w = w.trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | '"' | ',' | '.'));
            (w.starts_with("https://") && HOSTS.iter().any(|h| w.contains(h))).then(|| w.to_string())
        })
    }
}

pub fn new_id() -> String {
    ulid::Ulid::generate().to_string().to_lowercase()
}

/// One instance on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    /// Stored event that produced it (series or override or single).
    pub event_id: String,
    pub uid: String,
    pub calendar: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    /// All-day only: first day and exclusive last day.
    pub days: Option<(NaiveDate, NaiveDate)>,
    /// Part of a repeating series (instance or override).
    pub recurring: bool,
    /// Original start of this instance in the series (wall time in the event's zone).
    pub original: Option<NaiveDateTime>,
}

impl Occurrence {
    /// Shown on this day in `zone`.
    pub fn on_day(&self, day: NaiveDate, zone: chrono_tz::Tz) -> bool {
        if let Some((a, b)) = self.days {
            return a <= day && day < b;
        }
        let (s, e) = crate::tz::day_bounds(day, zone);
        self.start < e && (self.end > s || (self.end == self.start && self.start >= s))
    }
}
