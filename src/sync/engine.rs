//! Sync one account: calendars, then for each calendar pull, then push.
//!
//! Rules that keep data safe:
//! - Pull first, then push, so uploads start from the newest server copy.
//! - An object changed both here and on the server: the server's version
//!   wins, and ours is kept as a [`Conflict`] the user can bring back.
//! - Deleted on the server but changed here: ours is kept and uploaded again.
//! - Deleted here but changed on the server: the server's version comes back,
//!   with a note.
//! - Local edits made while a sync runs stay queued (`rev` check).
//! - Network or login problems stop the run; everything waits in the queue.

use std::collections::HashSet;

use chrono::Utc;

use super::state::{Conflict, SyncRow};
use super::{Change, Error, Provider, RemoteCalendar, RemoteObject};
use crate::accounts::Account;
use crate::model::{Calendar, Event, new_id};
use crate::store::Store;

/// What one run did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    /// Objects received from the server.
    pub down: usize,
    /// Objects uploaded or deleted on the server.
    pub up: usize,
    /// Edits replaced by the server's version (kept as conflicts).
    pub conflicts: usize,
    pub notes: Vec<String>,
    pub errors: Vec<String>,
}

type R<T> = Result<T, String>;

fn db<T>(r: rusqlite::Result<T>) -> R<T> {
    r.map_err(|e| format!("database: {e}"))
}

/// Sync errors that end the whole run (vs. one object).
fn fatal(e: Error) -> String {
    match e {
        Error::Auth(_) => format!("{e}. Check the password in Accounts."),
        other => other.to_string(),
    }
}

const COLORS: [&str; 8] = ["#89b4fa", "#a6e3a1", "#f38ba8", "#fab387", "#cba6f7", "#94e2d5", "#f9e2af", "#74c7ec"];

/// Runs a full sync of `account`. Server errors end up in the report and in
/// `account.error`; never panics.
pub fn sync_account(store: &mut Store, account: &mut Account, p: &mut dyn Provider) -> Report {
    let mut rep = Report::default();
    if let Err(e) = run(store, account, p, &mut rep) {
        rep.errors.push(e);
    }
    account.error = rep.errors.first().cloned();
    if account.error.is_none() {
        account.last_sync = Some(Utc::now());
    }
    match store.update_account(account) {
        Ok(true) => {}
        // Removed while this sync ran: drop the calendars it may have made again.
        Ok(false) => {
            if let Err(e) = store.delete_account(&account.id) {
                rep.errors.push(format!("database: {e}"));
            }
        }
        Err(e) => rep.errors.push(format!("database: {e}")),
    }
    rep
}

fn run(store: &mut Store, account: &mut Account, p: &mut dyn Provider, rep: &mut Report) -> R<()> {
    if let Ok(a) = p.my_addresses()
        && !a.is_empty()
    {
        account.addresses = a;
    }
    let remote = p.calendars().map_err(fatal)?;
    let cals = match_calendars(store, account, &remote, rep)?;
    for cal in cals {
        let Some(rid) = cal.remote.clone() else { continue };
        // A second round when an upload met a newer server copy.
        for _ in 0..2 {
            pull(store, p, &cal, &rid, rep).map_err(fatal)?;
            if cal.read_only || !push(store, p, &cal, &rid, rep).map_err(fatal)? {
                break;
            }
        }
    }
    Ok(())
}

/// Makes the account's local calendars match the server's list. Returns them.
fn match_calendars(store: &mut Store, account: &Account, remote: &[RemoteCalendar], rep: &mut Report) -> R<Vec<Calendar>> {
    let all = db(store.calendars())?;
    let mine: Vec<&Calendar> = all.iter().filter(|c| c.account == account.id).collect();
    let mut out = Vec::new();
    for (i, rc) in remote.iter().enumerate() {
        let cal = match mine.iter().find(|c| c.remote.as_deref() == Some(&rc.remote_id)) {
            Some(c) => {
                let mut c = (*c).clone();
                if c.name != rc.name || c.read_only != rc.read_only {
                    c.name = rc.name.clone();
                    c.read_only = rc.read_only;
                    db(store.save_calendar(&c))?;
                }
                c
            }
            None => {
                let c = Calendar {
                    id: new_id(),
                    name: rc.name.clone(),
                    color: rc.color.clone().unwrap_or_else(|| COLORS[(all.len() + i) % COLORS.len()].into()),
                    enabled: true,
                    default: false,
                    account: account.id.clone(),
                    read_only: rc.read_only,
                    remote: Some(rc.remote_id.clone()),
                };
                db(store.save_calendar(&c))?;
                c
            }
        };
        out.push(cal);
    }
    // Calendars removed on the server: keep unsent edits as conflicts.
    for c in mine.iter().filter(|c| !remote.iter().any(|r| Some(&r.remote_id) == c.remote.as_ref())) {
        for row in db(store.sync_rows(&c.id))?.into_iter().filter(|r| r.dirty() && !r.deleted) {
            let ours = db(store.object(&c.id, &row.uid))?;
            keep_conflict(store, &c.id, &row.uid, ours, vec![], &format!("The calendar “{}” was removed on the server.", c.name), rep)?;
        }
        db(store.delete_calendar(&c.id))?;
        db(store.set_sync_token(&c.id, None))?;
        rep.notes.push(format!("Calendar “{}” was removed on the server", c.name));
    }
    Ok(out)
}

fn title_of(events: &[Event]) -> String {
    events.iter().find(|e| e.recurrence_id.is_none()).or(events.first()).map(|e| e.title.clone()).unwrap_or_default()
}

fn keep_conflict(store: &Store, calendar: &str, uid: &str, mine: Vec<Event>, theirs: Vec<Event>, note: &str, rep: &mut Report) -> R<()> {
    let title = if mine.is_empty() { title_of(&theirs) } else { title_of(&mine) };
    db(store.add_conflict(&Conflict {
        id: new_id(),
        calendar: calendar.into(),
        uid: uid.into(),
        title,
        at: Utc::now(),
        note: note.into(),
        mine,
        theirs,
    }))?;
    rep.conflicts += 1;
    Ok(())
}

// ---------------- pull

fn pull(store: &mut Store, p: &mut dyn Provider, cal: &Calendar, rid: &str, rep: &mut Report) -> Result<(), Error> {
    let e = |x: String| Error::Parse(x);
    let token = store.sync_token(&cal.id).map_err(|x| e(x.to_string()))?;
    let pulled = match p.pull(rid, token.as_deref()) {
        Err(Error::TokenExpired) => p.pull(rid, None)?,
        other => other?,
    };
    let rows = store.sync_rows(&cal.id).map_err(|x| e(x.to_string()))?;
    let mut seen: HashSet<String> = HashSet::new();
    for change in pulled.changes {
        match change {
            Change::Upsert(obj) => {
                seen.insert(obj.href.clone());
                let Some(uid) = obj.events.first().map(|x| x.uid.clone()) else { continue };
                let row = rows.iter().find(|r| r.uid == uid);
                received(store, cal, &uid, row, obj, rep).map_err(e)?;
            }
            Change::Deleted(href) => {
                if let Some(r) = rows.iter().find(|r| r.href.as_deref() == Some(&href)) {
                    gone(store, cal, r, rep).map_err(e)?;
                }
            }
        }
    }
    if pulled.full {
        for r in rows.iter().filter(|r| r.href.as_ref().is_some_and(|h| !seen.contains(h))) {
            gone(store, cal, r, rep).map_err(e)?;
        }
    }
    store.set_sync_token(&cal.id, pulled.token.as_deref()).map_err(|x| e(x.to_string()))?;
    Ok(())
}

/// The server has this version of an object.
fn received(store: &mut Store, cal: &Calendar, uid: &str, row: Option<&SyncRow>, obj: RemoteObject, rep: &mut Report) -> R<()> {
    let unchanged = row.is_some_and(|r| r.href.as_deref() == Some(&obj.href) && !obj.etag.is_empty() && r.etag == obj.etag);
    if unchanged {
        // Same as last time (a full listing); local edits, if any, go up next.
        return Ok(());
    }
    if let Some(r) = row.filter(|r| r.dirty()) {
        let ours = if r.deleted { vec![] } else { db(store.object(&cal.id, uid))? };
        let note = if r.deleted {
            "Deleted here, but changed on the server. The server's version is back."
        } else {
            "Changed here and on the server at the same time. The server's version is shown; yours is kept."
        };
        keep_conflict(store, &cal.id, uid, ours, obj.events.clone(), note, rep)?;
    }
    db(store.replace_from_server(&cal.id, uid, &obj.events))?;
    db(store.set_synced(uid, &cal.id, &obj.href, &obj.etag, &obj.raw, &obj.events, row.map(|r| r.rev).unwrap_or(0)))?;
    rep.down += 1;
    Ok(())
}

/// The server deleted an object.
fn gone(store: &mut Store, cal: &Calendar, r: &SyncRow, rep: &mut Report) -> R<()> {
    if r.deleted {
        db(store.remove_sync_row(&r.uid, &cal.id))?;
    } else if r.dirty() {
        // Changed here: keep it and upload it again.
        db(store.clear_href(&r.uid, &cal.id))?;
        let title = title_of(&db(store.object(&cal.id, &r.uid))?);
        rep.notes.push(format!("“{title}” was deleted on the server but changed here; it was put back"));
    } else {
        db(store.replace_from_server(&cal.id, &r.uid, &[]))?;
        db(store.remove_sync_row(&r.uid, &cal.id))?;
        rep.down += 1;
    }
    Ok(())
}

// ---------------- push

/// Uploads queued changes. True when an upload met a newer server copy and
/// the calendar should be pulled again.
fn push(store: &mut Store, p: &mut dyn Provider, cal: &Calendar, rid: &str, rep: &mut Report) -> Result<bool, Error> {
    let e = |x: rusqlite::Error| Error::Parse(format!("database: {x}"));
    let rows: Vec<SyncRow> = store.sync_rows(&cal.id).map_err(e)?.into_iter().filter(SyncRow::dirty).collect();
    let mut again = false;
    for r in rows {
        if r.deleted {
            let Some(href) = &r.href else {
                // Never uploaded: nothing to delete there.
                store.remove_sync_row(&r.uid, &cal.id).map_err(e)?;
                continue;
            };
            match p.delete(rid, href, &r.etag, r.notify) {
                Ok(()) | Err(Error::NotFound) => {
                    store.remove_sync_row(&r.uid, &cal.id).map_err(e)?;
                    rep.up += 1;
                }
                Err(Error::Conflict) => {
                    // Changed there meanwhile: the next pull brings it back.
                    stale(store, &cal.id, &r, vec![], "Deleted here, but changed on the server. The server's version is back.", rep)
                        .map_err(Error::Parse)?;
                    again = true;
                }
                Err(x) => return Err(x),
            }
            continue;
        }
        let events = store.object(&cal.id, &r.uid).map_err(e)?;
        if events.is_empty() {
            store.remove_sync_row(&r.uid, &cal.id).map_err(e)?;
            continue;
        }
        match p.put(rid, r.href.as_deref(), &r.etag, r.raw.as_deref(), &events, r.notify) {
            Ok(obj) => {
                store.set_synced(&r.uid, &cal.id, &obj.href, &obj.etag, &obj.raw, &obj.events, r.rev).map_err(e)?;
                rep.up += 1;
            }
            Err(Error::Conflict) => {
                stale(
                    store,
                    &cal.id,
                    &r,
                    events,
                    "Changed here and on the server at the same time. The server's version is shown; yours is kept.",
                    rep,
                )
                .map_err(Error::Parse)?;
                again = true;
            }
            Err(Error::NotFound) => {
                // Gone there: create it again next round.
                store.clear_href(&r.uid, &cal.id).map_err(e)?;
                again = true;
            }
            Err(x @ (Error::Server(..) | Error::Parse(_))) => {
                // This object only: keep it queued and go on with the rest.
                rep.errors.push(format!("“{}”: {x}", title_of(&events)));
            }
            Err(x) => return Err(x),
        }
    }
    Ok(again)
}

/// An upload met a newer server copy: keep ours as a conflict and let the
/// next pull bring the server's version.
fn stale(store: &mut Store, calendar: &str, r: &SyncRow, mine: Vec<Event>, note: &str, rep: &mut Report) -> R<()> {
    keep_conflict(store, calendar, &r.uid, mine, vec![], note, rep)?;
    db(store
        .db
        .execute("UPDATE sync_objects SET synced_rev = rev, deleted = 0, etag = '' WHERE uid = ?1 AND calendar = ?2", [&r.uid, calendar]))?;
    // A full listing next time, so the server's copy surely arrives.
    db(store.set_sync_token(calendar, None))?;
    Ok(())
}
