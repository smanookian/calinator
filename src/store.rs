//! Local store: one SQLite file. The app, the CLI and the daemon share it.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::edit::Changes;
use crate::model::{Calendar, Event};

pub type Result<T, E = rusqlite::Error> = std::result::Result<T, E>;

pub struct Store {
    pub(crate) db: Connection,
}

/// (uid, calendar) pairs where these uids have events now.
fn places(tx: &rusqlite::Transaction, uids: &BTreeSet<String>) -> Result<HashSet<(String, String)>> {
    let mut q = tx.prepare("SELECT DISTINCT calendar FROM events WHERE uid = ?1")?;
    let mut out = HashSet::new();
    for u in uids {
        for c in q.query_map([u], |r| r.get::<_, String>(0))? {
            out.insert((u.clone(), c?));
        }
    }
    Ok(out)
}

/// `$XDG_DATA_HOME/calinator`, created on demand, readable only by the user.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    let d = base.join("calinator");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    d
}

/// Calendar data is personal: only the user may read the file (and its
/// SQLite side files, which live next to it).
fn private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    for ext in ["db-wal", "db-shm"] {
        let side = path.with_extension(ext);
        if side.exists() {
            let _ = std::fs::set_permissions(side, std::fs::Permissions::from_mode(0o600));
        }
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS calendars (id TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS events (id TEXT PRIMARY KEY, uid TEXT NOT NULL, calendar TEXT NOT NULL, json TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS events_uid ON events(uid);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS fired (key TEXT PRIMARY KEY, at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS snoozes (key TEXT PRIMARY KEY, until TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (id TEXT PRIMARY KEY, json TEXT NOT NULL);
-- One row per object (UID) in a synced calendar. rev > synced_rev: local
-- changes wait for upload. deleted: deleted here, delete on the server.
-- raw: the object as the server sent it; base: its events (JSON).
CREATE TABLE IF NOT EXISTS sync_objects (
  uid TEXT NOT NULL, calendar TEXT NOT NULL, href TEXT, etag TEXT NOT NULL DEFAULT '', raw TEXT,
  base TEXT, rev INTEGER NOT NULL DEFAULT 0, synced_rev INTEGER NOT NULL DEFAULT 0,
  deleted INTEGER NOT NULL DEFAULT 0, notify INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (uid, calendar));
CREATE INDEX IF NOT EXISTS sync_objects_href ON sync_objects(calendar, href);
-- Edits the server's version replaced: kept so nothing is lost.
CREATE TABLE IF NOT EXISTS conflicts (id TEXT PRIMARY KEY, json TEXT NOT NULL);
";

fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serializable")
}

fn bad(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

impl Store {
    pub fn open_default() -> Result<Self> {
        Self::open(&data_dir().join("calinator.db"))
    }

    pub fn open(path: &std::path::Path) -> Result<Self> {
        let db = Connection::open(path)?;
        private(path);
        // The daemon and the app write at the same time.
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.execute_batch(SCHEMA)?;
        Ok(Store { db })
    }

    pub fn calendars(&self) -> Result<Vec<Calendar>> {
        let mut q = self.db.prepare("SELECT json FROM calendars ORDER BY rowid")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect()
    }

    pub fn save_calendar(&self, c: &Calendar) -> Result<()> {
        self.db.execute(
            "INSERT INTO calendars (id, json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            params![c.id, to_json(c)],
        )?;
        Ok(())
    }

    /// Deletes a calendar and its events.
    pub fn delete_calendar(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM events WHERE calendar = ?1", [id])?;
        self.db.execute("DELETE FROM calendars WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn events(&self) -> Result<Vec<Event>> {
        let mut q = self.db.prepare("SELECT json FROM events")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect()
    }

    pub fn event(&self, id: &str) -> Result<Option<Event>> {
        let s: Option<String> = self.db.query_row("SELECT json FROM events WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        s.map(|s| serde_json::from_str(&s).map_err(bad)).transpose()
    }

    /// A series with its overrides.
    pub fn group(&self, uid: &str) -> Result<Vec<Event>> {
        let mut q = self.db.prepare("SELECT json FROM events WHERE uid = ?1")?;
        let rows = q.query_map([uid], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect()
    }

    pub fn save_event(&self, e: &Event) -> Result<()> {
        self.db.execute(
            "INSERT INTO events (id, uid, calendar, json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET uid = excluded.uid, calendar = excluded.calendar, json = excluded.json",
            params![e.id, e.uid, e.calendar, to_json(e)],
        )?;
        Ok(())
    }

    pub fn delete_event(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM events WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Saves a set of local changes in one transaction. Objects in synced
    /// calendars are queued for upload (or delete) by the sync engine.
    pub fn apply(&mut self, ch: &Changes) -> Result<()> {
        let tx = self.db.transaction()?;
        let mut uids: BTreeSet<String> = ch.upsert.iter().map(|e| e.uid.clone()).collect();
        for id in &ch.delete {
            if let Some(u) = tx.query_row("SELECT uid FROM events WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional()? {
                uids.insert(u);
            }
        }
        let before = places(&tx, &uids)?;
        for e in &ch.upsert {
            tx.execute(
                "INSERT INTO events (id, uid, calendar, json) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET uid = excluded.uid, calendar = excluded.calendar, json = excluded.json",
                params![e.id, e.uid, e.calendar, to_json(e)],
            )?;
        }
        for id in &ch.delete {
            tx.execute("DELETE FROM events WHERE id = ?1", [id])?;
        }
        let after = places(&tx, &uids)?;
        let synced: HashSet<String> = {
            let mut q = tx.prepare("SELECT id FROM calendars WHERE json_extract(json, '$.remote') IS NOT NULL")?;
            q.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_>>()?
        };
        for (uid, cal) in before.union(&after).filter(|(_, c)| synced.contains(c)) {
            let gone = !after.contains(&(uid.clone(), cal.clone()));
            tx.execute(
                "INSERT INTO sync_objects (uid, calendar, rev, synced_rev, deleted, notify) VALUES (?1, ?2, 1, 0, ?3, ?4)
                 ON CONFLICT(uid, calendar) DO UPDATE SET rev = rev + 1, deleted = excluded.deleted, notify = MAX(notify, excluded.notify)",
                params![uid, cal, gone, ch.notify],
            )?;
        }
        tx.commit()
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Makes a local "Personal" calendar when there is none.
    pub fn ensure_calendar(&self) -> Result<()> {
        if self.calendars()?.is_empty() {
            self.save_calendar(&Calendar {
                id: "personal".into(),
                name: "Personal".into(),
                color: "#89b4fa".into(),
                enabled: true,
                default: true,
                account: "local".into(),
                read_only: false,
                remote: None,
            })?;
        }
        Ok(())
    }

    /// Marks a reminder as shown. False when someone already showed it.
    pub fn claim_fired(&self, key: &str, at: DateTime<Utc>) -> Result<bool> {
        Ok(self.db.execute("INSERT OR IGNORE INTO fired (key, at) VALUES (?1, ?2)", params![key, at.to_rfc3339()])? == 1)
    }

    pub fn prune_fired(&self, before: DateTime<Utc>) -> Result<()> {
        self.db.execute("DELETE FROM fired WHERE at < ?1", [before.to_rfc3339()])?;
        self.db.execute("DELETE FROM snoozes WHERE until < ?1", [before.to_rfc3339()])?;
        Ok(())
    }

    pub fn snoozes(&self) -> Result<Vec<(String, DateTime<Utc>)>> {
        let mut q = self.db.prepare("SELECT key, until FROM snoozes")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows
            .filter_map(|r| r.ok())
            .filter_map(|(k, u)| DateTime::parse_from_rfc3339(&u).ok().map(|u| (k, u.with_timezone(&Utc))))
            .collect())
    }

    pub fn add_snooze(&self, key: &str, until: DateTime<Utc>) -> Result<()> {
        self.db.execute(
            "INSERT INTO snoozes (key, until) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET until = excluded.until",
            params![key, until.to_rfc3339()],
        )?;
        Ok(())
    }
}
