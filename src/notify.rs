//! Desktop notifications (freedesktop D-Bus: mako, dunst, GNOME, KDE) with
//! Join / Snooze buttons.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use zbus::zvariant::Value;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
}

/// What the user clicked on a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Click {
    Join,
    Snooze,
}

/// Which reminder a notification is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub event_id: String,
    /// Start of the occurrence, RFC 3339.
    pub start: String,
    pub video: Option<String>,
}

pub struct Notifier {
    proxy: NotificationsProxyBlocking<'static>,
    sent: Arc<Mutex<HashMap<u32, Sent>>>,
    /// Clicks on notifications this notifier sent.
    pub clicks: Receiver<(Sent, Click)>,
}

impl Notifier {
    /// Connects to the session bus. Err when there is no notification service.
    pub fn new() -> Result<Self, String> {
        let conn = zbus::blocking::Connection::session().map_err(|e| format!("no desktop session bus: {e}"))?;
        let proxy = NotificationsProxyBlocking::new(&conn).map_err(|e| format!("no notification service: {e}"))?;
        let sent: Arc<Mutex<HashMap<u32, Sent>>> = Arc::default();
        let (tx, clicks) = channel();
        let listener = proxy.clone();
        let ids = sent.clone();
        std::thread::spawn(move || {
            let Ok(signals) = listener.receive_action_invoked() else { return };
            for s in signals {
                let Ok(args) = s.args() else { continue };
                let click = match args.action_key.as_str() {
                    "snooze" => Click::Snooze,
                    "join" => Click::Join,
                    _ => continue,
                };
                let what = ids.lock().ok().and_then(|m| m.get(&args.id).cloned());
                if let Some(what) = what
                    && tx.send((what, click)).is_err()
                {
                    return;
                }
            }
        });
        Ok(Notifier { proxy, sent, clicks })
    }

    /// Shows a reminder. It stays until clicked or dismissed.
    pub fn send(&self, what: Sent, title: &str, body: &str, snooze_minutes: i64) -> Result<(), String> {
        let snooze = format!("Snooze {snooze_minutes} min");
        let mut actions = vec!["snooze", snooze.as_str()];
        if what.video.is_some() {
            actions.extend(["join", "Join call"]);
        }
        let mut hints = HashMap::new();
        hints.insert("desktop-entry", Value::from("calinator"));
        hints.insert("category", Value::from("x-calinator.reminder"));
        let id = self
            .proxy
            .notify("Calinator", 0, "x-office-calendar", title, body, &actions, hints, 0)
            .map_err(|e| format!("notification failed: {e}"))?;
        if let Ok(mut m) = self.sent.lock() {
            m.insert(id, what);
        }
        Ok(())
    }
}
