//! User settings, saved in the store so the app, CLI and daemon agree.

use serde::{Deserialize, Serialize};

use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Theme id (`match-omarchy` follows the desktop live).
    pub theme: String,
    /// Round pane corners (else square).
    pub round_corners: bool,
    /// Soft shadow under dialogs.
    pub shadow: bool,
    /// View on start: day, week, month, agenda.
    pub start_view: String,
    /// Week starts on Monday (else Sunday).
    pub week_starts_monday: bool,
    /// 24-hour clock (else am/pm).
    pub clock_24h: bool,
    /// Rows per hour in day and week views (1, 2 or 4).
    pub rows_per_hour: u16,
    /// First hour shown when the day/week view opens.
    pub day_start_hour: u32,
    /// Zone to show times in; empty = the computer's zone.
    pub timezone: String,
    /// A second zone column in day and week views; empty = off.
    pub second_timezone: String,
    /// Length of new events, in minutes.
    pub default_duration: i64,
    /// Reminder for new events, in minutes before (none when negative).
    pub default_reminder: i64,
    /// Show weekends in the week view.
    pub show_weekends: bool,
    /// Days shown in the agenda view.
    pub agenda_days: i64,
    /// Show desktop notifications for reminders.
    pub notifications: bool,
    /// Show the next event in the Omarchy bar (`calinator --bar`).
    pub show_in_bar: bool,
    /// Ask before the app closes.
    pub confirm_quit: bool,
    /// Remapped keys: action id → keys. Missing actions use the defaults.
    pub keys: std::collections::HashMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: if musinator_themes::omarchy::theme_dir().is_some() {
                musinator_themes::omarchy::ID.into()
            } else {
                musinator_themes::DEFAULT_ID.into()
            },
            round_corners: true,
            shadow: true,
            start_view: "week".into(),
            week_starts_monday: true,
            clock_24h: false,
            rows_per_hour: 2,
            day_start_hour: 8,
            timezone: String::new(),
            second_timezone: String::new(),
            default_duration: 60,
            default_reminder: 10,
            show_weekends: true,
            agenda_days: 14,
            notifications: true,
            show_in_bar: true,
            confirm_quit: false,
            keys: Default::default(),
        }
    }
}

const KEY: &str = "settings";

impl Settings {
    /// Saved settings; defaults for anything missing or unreadable.
    pub fn load(store: &Store) -> Self {
        store.meta(KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, store: &Store) -> crate::store::Result<()> {
        store.set_meta(KEY, &serde_json::to_string(self).unwrap_or_default())
    }

    /// The zone times are shown in.
    pub fn zone(&self) -> chrono_tz::Tz {
        if self.timezone.is_empty() { crate::tz::system() } else { crate::tz::parse(&self.timezone, crate::tz::system()) }
    }

    pub fn second_zone(&self) -> Option<chrono_tz::Tz> {
        self.second_timezone.parse().ok()
    }
}
