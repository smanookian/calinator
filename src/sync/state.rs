//! What the sync engine remembers in the store: accounts, the server copy of
//! each object, the upload queue, sync tokens and conflicts.

use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::accounts::Account;
use crate::model::{Event, new_id};
use crate::store::{Result, Store};

fn bad(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// One object (UID) in a synced calendar.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncRow {
    pub uid: String,
    pub calendar: String,
    /// Where it lives on the server; `None` until first uploaded.
    pub href: Option<String>,
    pub etag: String,
    /// The events as the server last had them.
    pub base: Option<Vec<Event>>,
    pub rev: i64,
    pub synced_rev: i64,
    /// Deleted here; delete on the server.
    pub deleted: bool,
    pub notify: bool,
    /// The object as the server last sent it; uploads start from it.
    pub raw: Option<String>,
}

impl SyncRow {
    /// Changed here since the last upload.
    pub fn dirty(&self) -> bool {
        self.rev > self.synced_rev
    }
}

/// An edit the server's version replaced. Kept so nothing is lost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub calendar: String,
    pub uid: String,
    pub title: String,
    pub at: DateTime<Utc>,
    /// What happened, in simple words.
    pub note: String,
    /// The version from this computer that was replaced (empty: it was deleted here).
    pub mine: Vec<Event>,
    /// The server's version that is now shown (empty: deleted on the server).
    pub theirs: Vec<Event>,
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<SyncRow> {
    let base: Option<String> = r.get(4)?;
    Ok(SyncRow {
        uid: r.get(0)?,
        calendar: r.get(1)?,
        href: r.get(2)?,
        etag: r.get(3)?,
        base: base.map(|b| serde_json::from_str(&b).map_err(bad)).transpose()?,
        rev: r.get(5)?,
        synced_rev: r.get(6)?,
        deleted: r.get(7)?,
        notify: r.get(8)?,
        raw: r.get(9)?,
    })
}

const COLS: &str = "uid, calendar, href, etag, base, rev, synced_rev, deleted, notify, raw";

impl Store {
    // ---------------- accounts

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut q = self.db.prepare("SELECT json FROM accounts ORDER BY rowid")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect()
    }

    pub fn save_account(&self, a: &Account) -> Result<()> {
        self.db.execute(
            "INSERT INTO accounts (id, json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            params![a.id, serde_json::to_string(a).expect("serializable")],
        )?;
        Ok(())
    }

    /// Saves sync results for an account that still exists. False when the
    /// account was removed meanwhile (then nothing is written).
    pub fn update_account(&self, a: &Account) -> Result<bool> {
        let n = self
            .db
            .execute("UPDATE accounts SET json = ?2 WHERE id = ?1", params![a.id, serde_json::to_string(a).expect("serializable")])?;
        Ok(n == 1)
    }

    /// Adds a new account. `Ok(Some(name))` instead when the same login
    /// (kind, server, user) is already here.
    pub fn add_account(&self, a: &Account) -> Result<Option<String>> {
        let same = |x: &Account| x.kind == a.kind && x.url == a.url && x.username.eq_ignore_ascii_case(&a.username);
        if let Some(x) = self.accounts()?.into_iter().find(same) {
            return Ok(Some(x.name));
        }
        self.save_account(a)?;
        Ok(None)
    }

    /// Removes an account, its calendars, their events and sync state.
    pub fn delete_account(&mut self, id: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        let cals: Vec<String> = {
            let mut q = tx.prepare("SELECT id FROM calendars WHERE json_extract(json, '$.account') = ?1")?;
            q.query_map([id], |r| r.get(0))?.collect::<Result<_>>()?
        };
        for c in &cals {
            tx.execute("DELETE FROM events WHERE calendar = ?1", [c])?;
            tx.execute("DELETE FROM sync_objects WHERE calendar = ?1", [c])?;
            tx.execute("DELETE FROM meta WHERE key = ?1", [format!("sync_token:{c}")])?;
            tx.execute("DELETE FROM calendars WHERE id = ?1", [c])?;
        }
        tx.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        tx.commit()
    }

    // ---------------- sync rows

    pub fn sync_rows(&self, calendar: &str) -> Result<Vec<SyncRow>> {
        let mut q = self.db.prepare(&format!("SELECT {COLS} FROM sync_objects WHERE calendar = ?1"))?;
        q.query_map([calendar], row)?.collect()
    }

    pub fn sync_row(&self, uid: &str, calendar: &str) -> Result<Option<SyncRow>> {
        self.db.query_row(&format!("SELECT {COLS} FROM sync_objects WHERE uid = ?1 AND calendar = ?2"), [uid, calendar], row).optional()
    }

    /// How many objects wait for upload, per calendar id.
    pub fn pending(&self) -> Result<Vec<(String, i64)>> {
        let mut q = self.db.prepare("SELECT calendar, COUNT(*) FROM sync_objects WHERE rev > synced_rev GROUP BY calendar")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    }

    /// Records the server copy after a pull or upload. Marks the row clean up
    /// to `rev` (a local edit made meanwhile stays queued).
    #[allow(clippy::too_many_arguments)]
    pub fn set_synced(&self, uid: &str, calendar: &str, href: &str, etag: &str, raw: &str, base: &[Event], rev: i64) -> Result<()> {
        let base = serde_json::to_string(base).expect("serializable");
        self.db.execute(
            "INSERT INTO sync_objects (uid, calendar, href, etag, raw, base, rev, synced_rev, deleted, notify) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 0, 0)
             ON CONFLICT(uid, calendar) DO UPDATE SET href = excluded.href, etag = excluded.etag, raw = excluded.raw, base = excluded.base,
               synced_rev = MAX(synced_rev, excluded.synced_rev), rev = MAX(rev, excluded.rev),
               notify = CASE WHEN rev > excluded.synced_rev THEN notify ELSE 0 END",
            params![uid, calendar, href, etag, raw, base, rev],
        )?;
        Ok(())
    }

    /// Forgets where an object was on the server, so the next upload creates
    /// it again (it was deleted there but changed here).
    pub fn clear_href(&self, uid: &str, calendar: &str) -> Result<()> {
        self.db.execute(
            "UPDATE sync_objects SET href = NULL, etag = '', raw = NULL, base = NULL WHERE uid = ?1 AND calendar = ?2",
            [uid, calendar],
        )?;
        Ok(())
    }

    pub fn remove_sync_row(&self, uid: &str, calendar: &str) -> Result<()> {
        self.db.execute("DELETE FROM sync_objects WHERE uid = ?1 AND calendar = ?2", [uid, calendar])?;
        Ok(())
    }

    /// Replaces the local rows of an object with the server's version, without
    /// queuing an upload. Keeps local row ids for the same instances, so the
    /// selection on screen stays put. Empty `events` deletes the object.
    pub fn replace_from_server(&mut self, calendar: &str, uid: &str, events: &[Event]) -> Result<()> {
        let tx = self.db.transaction()?;
        let old: Vec<Event> = {
            let mut q = tx.prepare("SELECT json FROM events WHERE uid = ?1 AND calendar = ?2")?;
            let rows = q.query_map([uid, calendar], |r| r.get::<_, String>(0))?;
            rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect::<Result<_>>()?
        };
        tx.execute("DELETE FROM events WHERE uid = ?1 AND calendar = ?2", [uid, calendar])?;
        for e in events {
            let mut e = e.clone();
            e.calendar = calendar.to_string();
            e.uid = uid.to_string();
            e.id = old.iter().find(|o| o.recurrence_id == e.recurrence_id).map(|o| o.id.clone()).unwrap_or_else(new_id);
            tx.execute(
                "INSERT INTO events (id, uid, calendar, json) VALUES (?1, ?2, ?3, ?4)",
                params![e.id, e.uid, e.calendar, serde_json::to_string(&e).expect("serializable")],
            )?;
        }
        tx.commit()
    }

    /// The local rows of an object in a calendar.
    pub fn object(&self, calendar: &str, uid: &str) -> Result<Vec<Event>> {
        let mut q = self.db.prepare("SELECT json FROM events WHERE uid = ?1 AND calendar = ?2")?;
        let rows = q.query_map([uid, calendar], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect()
    }

    // ---------------- tokens

    pub fn sync_token(&self, calendar: &str) -> Result<Option<String>> {
        self.meta(&format!("sync_token:{calendar}"))
    }

    pub fn set_sync_token(&self, calendar: &str, token: Option<&str>) -> Result<()> {
        match token {
            Some(t) => self.set_meta(&format!("sync_token:{calendar}"), t),
            None => {
                self.db.execute("DELETE FROM meta WHERE key = ?1", [format!("sync_token:{calendar}")])?;
                Ok(())
            }
        }
    }

    // ---------------- conflicts

    pub fn conflicts(&self) -> Result<Vec<Conflict>> {
        let mut q = self.db.prepare("SELECT json FROM conflicts ORDER BY rowid DESC")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.and_then(|s| serde_json::from_str(&s).map_err(bad))).collect()
    }

    pub fn add_conflict(&self, c: &Conflict) -> Result<()> {
        self.db
            .execute("INSERT INTO conflicts (id, json) VALUES (?1, ?2)", params![c.id, serde_json::to_string(c).expect("serializable")])?;
        Ok(())
    }

    pub fn delete_conflict(&self, id: &str) -> Result<()> {
        self.db.execute("DELETE FROM conflicts WHERE id = ?1", [id])?;
        Ok(())
    }
}
