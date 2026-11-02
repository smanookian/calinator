//! Actions and their keys. Every action is remappable (Settings → Keys).
//! Same design as Todonator's keymap. Keys in the "Invites" group only work
//! in the invites tray, so they may reuse keys of other groups.

use std::collections::HashMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    Left,
    Right,
    Down,
    Up,
    NextEvent,
    PrevEvent,
    PrevPeriod,
    NextPeriod,
    Today,
    GoTo,
    PageDown,
    PageUp,
    NextPane,
    PrevPane,
    Back,
    ViewDay,
    ViewWeek,
    ViewMonth,
    ViewAgenda,
    Search,
    QuickAdd,
    NewEvent,
    Open,
    Edit,
    Delete,
    Join,
    Invites,
    Sync,
    Accounts,
    Conflicts,
    ToggleCalendar,
    RsvpYes,
    RsvpNo,
    RsvpMaybe,
    Themes,
    Settings,
    Help,
    Quit,
}

pub struct ActionInfo {
    pub action: Action,
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub keys: &'static [&'static str],
}

macro_rules! actions {
    ($($a:ident $id:literal $group:literal $label:literal [$($k:literal),*];)*) => {
        pub const ACTIONS: &[ActionInfo] = &[$(ActionInfo { action: Action::$a, id: $id, label: $label, group: $group, keys: &[$($k),*] }),*];
    };
}

actions! {
    Left "left" "Move" "Previous day" ["h", "left"];
    Right "right" "Move" "Next day" ["l", "right"];
    Down "down" "Move" "Later / down" ["j", "down"];
    Up "up" "Move" "Earlier / up" ["k", "up"];
    NextEvent "next_event" "Move" "Next event" ["}", "ctrl+n"];
    PrevEvent "prev_event" "Move" "Previous event" ["{", "ctrl+p"];
    PrevPeriod "prev_period" "Move" "Previous week / month" ["["];
    NextPeriod "next_period" "Move" "Next week / month" ["]"];
    Today "today" "Move" "Today" ["t"];
    GoTo "go_to" "Move" "Go to a date" ["G"];
    PageDown "page_down" "Move" "Scroll down" ["pagedown", "ctrl+d"];
    PageUp "page_up" "Move" "Scroll up" ["pageup", "ctrl+u"];
    NextPane "next_pane" "Move" "Next pane" ["tab"];
    PrevPane "prev_pane" "Move" "Previous pane" ["backtab"];
    Back "back" "Move" "Back / close" ["esc"];
    ViewDay "view_day" "Views" "Day" ["d"];
    ViewWeek "view_week" "Views" "Week" ["w"];
    ViewMonth "view_month" "Views" "Month" ["m"];
    ViewAgenda "view_agenda" "Views" "Agenda" ["g"];
    Search "search" "Views" "Search events" ["/"];
    QuickAdd "quick_add" "Events" "Quick add" ["a"];
    NewEvent "new_event" "Events" "New event (full form)" ["A"];
    Open "open" "Events" "Open event" ["enter"];
    Edit "edit" "Events" "Edit event" ["e"];
    Delete "delete" "Events" "Delete event" ["x", "delete"];
    Join "join" "Events" "Join video call" ["J"];
    Invites "invites" "Events" "Invites" ["i"];
    Sync "sync" "Events" "Sync now" ["s"];
    Accounts "accounts" "Events" "Accounts" ["@"];
    Conflicts "conflicts" "Events" "Sync conflicts" ["!"];
    ToggleCalendar "toggle_calendar" "Calendars" "Show / hide calendar" ["space"];
    RsvpYes "rsvp_yes" "Invites" "Answer yes" ["y"];
    RsvpNo "rsvp_no" "Invites" "Answer no" ["n"];
    RsvpMaybe "rsvp_maybe" "Invites" "Answer maybe" ["m"];
    Themes "themes" "App" "Themes" ["T"];
    Settings "settings" "App" "Settings" ["S"];
    Help "help" "App" "Help: all keys" ["?"];
    Quit "quit" "App" "Quit" ["q", "ctrl+c"];
}

pub fn info(a: Action) -> &'static ActionInfo {
    ACTIONS.iter().find(|i| i.action == a).expect("action info")
}

/// Two actions can be active at the same time (so must not share a key).
fn same_context(a: Action, b: Action) -> bool {
    let tray = |x: Action| info(x).group == "Invites";
    tray(a) == tray(b) || matches!(a, Action::Back | Action::Up | Action::Down) || matches!(b, Action::Back | Action::Up | Action::Down)
}

/// A key with modifiers, normalised so `shift+j` and `J` compare equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Chord {
    pub fn from_event(e: &KeyEvent) -> Self {
        let mut mods = e.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let code = match e.code {
            KeyCode::Char(c) => {
                // Shift is already in the character.
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::Char(c)
            }
            KeyCode::BackTab => {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            c => c,
        };
        Self { code, mods }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = KeyModifiers::NONE;
        let mut parts: Vec<&str> = s.split('+').collect();
        // "+" alone, or "ctrl++".
        let key = if s.ends_with("++") || s == "+" {
            parts.retain(|p| !p.is_empty());
            "+"
        } else {
            parts.pop()?
        };
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" => mods |= KeyModifiers::CONTROL,
                "alt" => mods |= KeyModifiers::ALT,
                "shift" => mods |= KeyModifiers::SHIFT,
                "" => {}
                _ => return None,
            }
        }
        let code = match key.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => KeyCode::F(f[1..].parse().ok()?),
            _ if key.chars().count() == 1 => {
                let c = key.chars().next()?;
                if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_alphabetic() {
                    mods.remove(KeyModifiers::SHIFT);
                    KeyCode::Char(c.to_ascii_uppercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            _ => return None,
        };
        if let KeyCode::Char(c) = code
            && c.is_ascii_alphabetic()
            && mods.contains(KeyModifiers::CONTROL)
        {
            return Some(Self { code: KeyCode::Char(c.to_ascii_lowercase()), mods });
        }
        Some(Self { code, mods })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl+")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            f.write_str("alt+")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            f.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("shift+tab"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            KeyCode::F(n) => write!(f, "f{n}"),
            _ => f.write_str("?"),
        }
    }
}

/// Config spelling of a chord (ASCII, parseable).
pub fn chord_id(c: &Chord) -> String {
    let s = c.to_string();
    s.replace('←', "left").replace('→', "right").replace('↑', "up").replace('↓', "down").replace("shift+tab", "backtab")
}

#[derive(Clone, Debug)]
pub struct Keymap {
    by_action: HashMap<Action, Vec<Chord>>,
}

impl Keymap {
    /// Defaults, with user overrides (`action id -> list of keys`).
    pub fn new(overrides: &HashMap<String, Vec<String>>) -> Self {
        let by_action = ACTIONS
            .iter()
            .map(|i| {
                let keys: Vec<Chord> = match overrides.get(i.id) {
                    Some(v) => v.iter().filter_map(|k| Chord::parse(k)).collect(),
                    None => i.keys.iter().filter_map(|k| Chord::parse(k)).collect(),
                };
                (i.action, keys)
            })
            .collect();
        Self { by_action }
    }

    /// The main-screen action for a key (invite keys are left out).
    pub fn action(&self, e: &KeyEvent) -> Option<Action> {
        let c = Chord::from_event(e);
        ACTIONS.iter().filter(|i| i.group != "Invites").map(|i| i.action).find(|a| self.keys(*a).contains(&c))
    }

    /// The key is bound to `a`.
    pub fn is(&self, a: Action, e: &KeyEvent) -> bool {
        self.keys(a).contains(&Chord::from_event(e))
    }

    pub fn keys(&self, a: Action) -> &[Chord] {
        self.by_action.get(&a).map(Vec::as_slice).unwrap_or(&[])
    }

    /// First key as text, for hints ("space", "/").
    pub fn hint(&self, a: Action) -> String {
        self.keys(a).first().map(|c| c.to_string()).unwrap_or_else(|| "—".into())
    }

    /// Another action active at the same time that already uses this chord.
    pub fn conflict(&self, c: &Chord, not: Action) -> Option<Action> {
        ACTIONS.iter().map(|i| i.action).find(|a| *a != not && same_context(*a, not) && self.keys(*a).contains(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn defaults_and_remap() {
        let k = Keymap::new(&HashMap::new());
        // Terminals send uppercase letters and symbols with SHIFT set.
        assert_eq!(k.action(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT)), Some(Action::NewEvent));
        assert_eq!(k.action(&ev(KeyCode::Char('?'), KeyModifiers::SHIFT)), Some(Action::Help));
        // "m" is month on the main screen and "maybe" in the invites tray.
        assert_eq!(k.action(&ev(KeyCode::Char('m'), KeyModifiers::NONE)), Some(Action::ViewMonth));
        assert!(k.is(Action::RsvpMaybe, &ev(KeyCode::Char('m'), KeyModifiers::NONE)));
        let mut o = HashMap::new();
        o.insert("view_week".to_string(), vec!["ctrl+w".to_string()]);
        let k = Keymap::new(&o);
        assert_eq!(k.action(&ev(KeyCode::Char('w'), KeyModifiers::NONE)), None);
        assert_eq!(k.action(&ev(KeyCode::Char('w'), KeyModifiers::CONTROL)), Some(Action::ViewWeek));
        for i in ACTIONS {
            for key in i.keys {
                let c = Chord::parse(key).unwrap_or_else(|| panic!("bad default key {key}"));
                assert_eq!(Chord::parse(&chord_id(&c)), Some(c), "{key}");
            }
        }
    }

    #[test]
    fn no_default_conflicts() {
        let k = Keymap::new(&HashMap::new());
        for i in ACTIONS {
            for c in k.keys(i.action) {
                assert_eq!(k.conflict(c, i.action), None, "{} clashes on {c}", i.id);
            }
        }
    }
}
