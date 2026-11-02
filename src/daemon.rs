//! Background work: sync, reminders and subscribed calendars. Runs inside
//! the app (on its own thread, so the screen never waits) and as
//! `calinator daemon`, a systemd user service, so the bar stays fresh and
//! reminders show when the app is closed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use chrono::Utc;

use crate::notify::Notifier;
use crate::settings::Settings;
use crate::store::Store;
use crate::sync::runner::{self, Outcome};
use crate::{remind, webcal};

pub const UNIT: &str = "calinator.service";

/// Subscribed calendars are downloaded again this often.
const REFRESH: Duration = Duration::from_secs(3600);
/// Online accounts sync this often (changes on the phone show up within a minute).
const SYNC_EVERY: Duration = Duration::from_secs(30);
/// Rounds for reminders.
const TICK: Duration = Duration::from_secs(15);

/// Messages from the background thread to the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Syncing,
    /// A round finished: calendar data may have changed. Short summaries.
    Done(Vec<String>),
}

/// State kept between background rounds.
pub struct Background {
    db: PathBuf,
    notifier: Option<Notifier>,
    refreshed: Option<Instant>,
    synced: Option<Instant>,
}

impl Background {
    pub fn new(db: &Path) -> Self {
        // Quiet: inside the app, stderr would draw over the screen.
        let notifier = Notifier::new().ok();
        Background { db: db.to_path_buf(), notifier, refreshed: None, synced: None }
    }

    /// Is a sync due (or asked for)?
    fn sync_due(&self, now: bool) -> bool {
        now || self.synced.is_none_or(|t| t.elapsed() >= SYNC_EVERY)
    }

    /// One round: sync (when due or `sync_now`), reminders, clicks,
    /// subscriptions. Returns log lines, and whether calendar data may have changed.
    pub fn step(&mut self, sync_now: bool, syncing: impl Fn()) -> (Vec<String>, bool) {
        let mut log = Vec::new();
        let mut changed = false;
        let mut store = match Store::open(&self.db) {
            Ok(s) => s,
            Err(e) => return (vec![format!("cannot open the calendar file: {e}")], false),
        };
        let has_accounts = store.accounts().is_ok_and(|a| !a.is_empty());
        if has_accounts && self.sync_due(sync_now) {
            self.synced = Some(Instant::now());
            syncing();
            // Done or not, the app hears back (it shows "syncing" until then).
            changed = true;
            match runner::sync_all(&self.db, None) {
                Ok(Outcome::Done(results)) => log.extend(results.iter().map(runner::summary)),
                Ok(Outcome::Busy) => log.push("another Calinator is syncing; skipped this round".into()),
                Ok(Outcome::NoAccounts) => {}
                Err(e) => log.push(format!("sync: {e}")),
            }
        }
        if self.notifier.is_none() {
            self.notifier = Notifier::new().ok();
        }
        if let Some(n) = &self.notifier {
            match remind::check(&store, n, Utc::now()) {
                Ok(0) => {}
                Ok(k) => log.push(format!("showed {k} reminder{}", if k == 1 { "" } else { "s" })),
                Err(e) => log.push(format!("reminders: {e}")),
            }
            while let Ok((what, click)) = n.clicks.try_recv() {
                match remind::on_click(&store, &what, click, Utc::now()) {
                    Ok(m) => log.push(m),
                    Err(e) => log.push(format!("notification click: {e}")),
                }
            }
        }
        if self.refreshed.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.refreshed = Some(Instant::now());
            let has_subs = store.calendars().unwrap_or_default().iter().any(|c| c.account.starts_with(webcal::PREFIX));
            if has_subs {
                let zone = Settings::load(&store).zone();
                let errors = webcal::refresh_all(&mut store, zone);
                changed = true;
                log.extend(errors.into_iter().map(|e| format!("subscription: {e}")));
            }
        }
        (log, changed)
    }

    /// Runs rounds on a thread. Send `()` on the returned sender to sync now.
    /// Stops when the app is gone.
    pub fn spawn(db: &Path, to_app: Sender<Msg>) -> Sender<()> {
        let (tx, rx): (Sender<()>, Receiver<()>) = channel();
        let db = db.to_path_buf();
        std::thread::spawn(move || {
            let mut bg = Background::new(&db);
            let mut now = false;
            loop {
                let tell = to_app.clone();
                let (log, changed) = bg.step(now, move || {
                    let _ = tell.send(Msg::Syncing);
                });
                if changed && to_app.send(Msg::Done(log)).is_err() {
                    return;
                }
                now = match rx.recv_timeout(TICK) {
                    Ok(()) => {
                        // Several quick edits: one sync.
                        while rx.try_recv().is_ok() {}
                        true
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
            }
        });
        tx
    }
}

/// `calinator daemon`: rounds every 15 s, forever. Logs to stderr (the journal).
pub fn run(db: &Path) -> ! {
    eprintln!("calinator daemon: sync and reminders on");
    let mut bg = Background::new(db);
    loop {
        for l in bg.step(false, || {}).0 {
            eprintln!("{l}");
        }
        std::thread::sleep(TICK);
    }
}

/// Is the background service running?
pub fn is_active() -> bool {
    std::process::Command::new("systemctl").args(["--user", "is-active", "--quiet", UNIT]).status().is_ok_and(|s| s.success())
}

fn unit_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("systemd/user").join(UNIT))
}

fn unit_text(exe: &Path, db: Option<&Path>) -> String {
    let db_arg = db.map(|d| format!(" --db {}", d.display())).unwrap_or_default();
    format!(
        "[Unit]\nDescription=Calinator reminders\nAfter=graphical-session.target\n\n\
         [Service]\nExecStart={}{db_arg} daemon\nRestart=on-failure\nRestartSec=30\n\n\
         [Install]\nWantedBy=default.target\n",
        exe.display()
    )
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let st = std::process::Command::new("systemctl").arg("--user").args(args).status().map_err(|e| format!("systemctl: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("systemctl --user {} failed", args.join(" "))) }
}

/// Writes the systemd user unit and starts it. `db` only when not the default.
pub fn install(db: Option<&Path>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find this program: {e}"))?;
    let path = unit_path().ok_or("HOME is not set")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, unit_text(&exe, db)).map_err(|e| format!("{}: {e}", path.display()))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", UNIT])?;
    Ok(format!("Installed and started {UNIT}. Logs: journalctl --user -u calinator"))
}

/// Stops the service and removes the unit.
pub fn uninstall() -> Result<String, String> {
    let _ = systemctl(&["disable", "--now", UNIT]);
    if let Some(p) = unit_path() {
        let _ = std::fs::remove_file(p);
    }
    systemctl(&["daemon-reload"])?;
    Ok(format!("Stopped and removed {UNIT}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_runs_this_binary_with_custom_db() {
        let u = unit_text(Path::new("/usr/bin/calinator"), Some(Path::new("/tmp/c.db")));
        assert!(u.contains("ExecStart=/usr/bin/calinator --db /tmp/c.db daemon\n"));
        assert!(unit_text(Path::new("/usr/bin/calinator"), None).contains("ExecStart=/usr/bin/calinator daemon\n"));
    }
}
