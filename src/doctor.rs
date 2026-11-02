//! `calinator doctor`: checks the calendar file, zone, keyring, reminders,
//! daemon and theme, in plain words.

use std::path::Path;

use crate::settings::Settings;
use crate::store::Store;

pub struct Check {
    pub ok: bool,
    pub name: &'static str,
    pub detail: String,
}

fn check(ok: bool, name: &'static str, detail: impl Into<String>) -> Check {
    Check { ok, name, detail: detail.into() }
}

/// Is a D-Bus name on the session bus?
fn bus_has(name: &str) -> Result<bool, String> {
    let conn = zbus::blocking::Connection::session().map_err(|e| format!("no session bus: {e}"))?;
    let proxy = zbus::blocking::fdo::DBusProxy::new(&conn).map_err(|e| e.to_string())?;
    let n = zbus::names::BusName::try_from(name).map_err(|e| e.to_string())?;
    proxy.name_has_owner(n).map_err(|e| e.to_string())
}

pub fn run(db: &Path) -> Vec<Check> {
    let mut out = Vec::new();
    let store = match Store::open(db) {
        Ok(s) => s,
        Err(e) => {
            out.push(check(false, "Calendar file", format!("{}: {e}", db.display())));
            return out;
        }
    };
    let cals = store.calendars().unwrap_or_default();
    let events = store.events().unwrap_or_default();
    out.push(check(true, "Calendar file", format!("{} · {} calendars, {} events", db.display(), cals.len(), events.len())));

    let s = Settings::load(&store);
    let zone = s.zone();
    let src = if s.timezone.is_empty() { "from this computer" } else { "set in Settings" };
    out.push(check(true, "Time zone", format!("{} ({src})", zone.name())));

    let accounts = store.accounts().unwrap_or_default();
    if accounts.is_empty() {
        out.push(check(true, "Accounts", "none yet: calendars live on this computer. Add one in the app with @"));
    }
    for a in &accounts {
        let secret = crate::accounts::get_secret(&a.id);
        let (ok, detail) = match (&a.error, secret) {
            (_, Err(e)) => (false, e),
            (_, Ok(None)) => (false, "password missing from the keyring; add the account again".into()),
            (Some(e), _) => (false, e.clone()),
            (None, _) => (
                true,
                a.last_sync
                    .map(|t| format!("synced {}", crate::tz::local(t, zone).format("%b %-d %H:%M")))
                    .unwrap_or_else(|| "not synced yet".into()),
            ),
        };
        out.push(check(ok, "Account", format!("{} ({}): {detail}", a.name, a.kind.label())));
    }
    let waiting: i64 = store.pending().unwrap_or_default().iter().map(|(_, n)| n).sum();
    if waiting > 0 {
        out.push(check(
            true,
            "Waiting to sync",
            format!("{waiting} change{} (they go up at the next sync)", if waiting == 1 { "" } else { "s" }),
        ));
    }
    let conflicts = store.conflicts().unwrap_or_default().len();
    if conflicts > 0 {
        out.push(check(true, "Conflicts", format!("{conflicts} kept (see ! in the app)")));
    }
    let subs: Vec<&str> = cals.iter().filter(|c| c.account.starts_with(crate::webcal::PREFIX)).map(|c| c.name.as_str()).collect();
    if !subs.is_empty() {
        out.push(check(true, "Subscriptions", subs.join(", ")));
    }

    match bus_has("org.freedesktop.secrets") {
        Ok(true) => out.push(check(true, "Keyring", "system keyring is running (passwords and sign-ins go here)")),
        Ok(false) => out.push(check(false, "Keyring", "no system keyring found. Install gnome-keyring or KeePassXC (Secret Service)")),
        Err(e) => out.push(check(false, "Keyring", e)),
    }
    match bus_has("org.freedesktop.Notifications") {
        Ok(true) => out.push(check(true, "Notifications", if s.notifications { "on" } else { "off in Settings" })),
        Ok(false) => out.push(check(false, "Notifications", "no notification service (mako, dunst…) is running")),
        Err(e) => out.push(check(false, "Notifications", e)),
    }
    if crate::daemon::is_active() {
        out.push(check(true, "Reminder service", "running: reminders work with the app closed"));
    } else {
        out.push(check(false, "Reminder service", "not running: reminders only while the app is open. Fix: calinator daemon --install"));
    }

    let theme = crate::app::theme::load(&s.theme);
    let weak: Vec<String> = musinator_themes::contrast_report(&theme)
        .into_iter()
        .filter(|(_, r, m)| r + 1e-9 < *m)
        .map(|(p, r, m)| format!("{p} {r:.1} < {m}"))
        .collect();
    let live = if s.theme == musinator_themes::omarchy::ID {
        if musinator_themes::omarchy::theme_dir().is_some() {
            " · follows Omarchy live"
        } else {
            " · Omarchy not found, using the default"
        }
    } else {
        ""
    };
    out.push(check(
        weak.is_empty(),
        "Theme",
        if weak.is_empty() { format!("{}{live} · contrast OK", theme.label) } else { format!("{}: {}", theme.label, weak.join(", ")) },
    ));
    out.push(check(true, "Bar", if s.show_in_bar { "on (calinator --bar)" } else { "off in Settings" }));
    out
}
