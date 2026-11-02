//! Themes from Musinator's theme crate, and live match-omarchy.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use musinator_themes::{Theme, omarchy};

/// A theme by id. `match-omarchy` reads the desktop's current colors; falls
/// back to the default theme when Omarchy (or the id) is missing.
pub fn load(id: &str) -> Theme {
    if id == omarchy::ID
        && let Some(t) = omarchy::current()
    {
        return t;
    }
    musinator_themes::by_id(id).or_else(|| musinator_themes::by_id(musinator_themes::DEFAULT_ID)).expect("default theme exists")
}

/// Every theme for the picker: Match Omarchy first (when installed).
pub fn all() -> Vec<Theme> {
    let mut v: Vec<Theme> = omarchy::current().into_iter().collect();
    v.extend(musinator_themes::builtin());
    v
}

/// Notices an Omarchy theme switch by checking the theme paths' change times.
pub struct OmarchyWatch {
    paths: Vec<PathBuf>,
    stamp: Vec<Option<SystemTime>>,
    checked: Instant,
}

fn stamps(paths: &[PathBuf]) -> Vec<Option<SystemTime>> {
    // The link itself (theme swapped) and what it points to (files edited).
    paths
        .iter()
        .flat_map(|p| [std::fs::symlink_metadata(p).and_then(|m| m.modified()).ok(), std::fs::metadata(p).and_then(|m| m.modified()).ok()])
        .collect()
}

impl Default for OmarchyWatch {
    fn default() -> Self {
        Self::new()
    }
}

impl OmarchyWatch {
    pub fn new() -> Self {
        let paths = omarchy::watch_paths();
        let stamp = stamps(&paths);
        OmarchyWatch { paths, stamp, checked: Instant::now() }
    }

    /// True once after the Omarchy theme changed. Cheap; checks at most once a second.
    pub fn changed(&mut self) -> bool {
        if self.checked.elapsed() < Duration::from_secs(1) {
            return false;
        }
        self.checked = Instant::now();
        if self.paths.is_empty() {
            self.paths = omarchy::watch_paths();
        }
        let now = stamps(&self.paths);
        if now != self.stamp {
            self.stamp = now;
            return true;
        }
        false
    }
}

/// Fill and text for an event block in a calendar's color. Unselected
/// blocks are a tint of the color; the text is nudged until it reads well.
pub fn event_colors(t: &Theme, color: &str, selected: bool) -> (musinator_themes::Rgb, musinator_themes::Rgb) {
    let cal = musinator_themes::Rgb::hex(color).unwrap_or(t.accent);
    let bg = if selected { cal } else { t.bg.mix(cal, 0.30) };
    let fg = if selected { if bg.luminance() > 0.35 { t.bg } else { t.fg } } else { t.fg };
    (bg, musinator_themes::legible(fg, bg, 4.5))
}

/// Color of a calendar's dot and bar on the plain background.
pub fn calendar_color(t: &Theme, color: &str) -> musinator_themes::Rgb {
    musinator_themes::legible(musinator_themes::Rgb::hex(color).unwrap_or(t.accent), t.bg, 3.0)
}
