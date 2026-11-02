//! Sync every account, one process at a time (app, daemon or CLI).

use std::fs::File;
use std::path::{Path, PathBuf};

use super::Provider;
use super::caldav::CalDav;
use super::engine::{Report, sync_account};
use super::google::Google;
use crate::accounts::{self, Account, Kind};
use crate::oauth::{self, OAuthTokens};
use crate::store::Store;

#[derive(Debug, Clone)]
pub struct AccountResult {
    pub account_id: String,
    pub name: String,
    pub report: Report,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Done(Vec<AccountResult>),
    /// Another process (daemon or app) is syncing right now.
    Busy,
    /// Nothing to sync: no online accounts.
    NoAccounts,
}

fn lock_path(db: &Path) -> PathBuf {
    db.with_extension("sync.lock")
}

/// Syncs every account (or just `only`). Holds a file lock while running.
pub fn sync_all(db: &Path, only: Option<&str>) -> Result<Outcome, String> {
    let lock = File::create(lock_path(db)).map_err(|e| format!("lock file: {e}"))?;
    if lock.try_lock().is_err() {
        return Ok(Outcome::Busy);
    }
    let mut store = Store::open(db).map_err(|e| format!("database: {e}"))?;
    let accounts: Vec<Account> =
        store.accounts().map_err(|e| format!("database: {e}"))?.into_iter().filter(|a| only.is_none_or(|o| o == a.id)).collect();
    if accounts.is_empty() {
        return Ok(Outcome::NoAccounts);
    }
    let mut out = Vec::new();
    for mut a in accounts {
        let report = match provider_for(&a) {
            Ok(mut p) => sync_account(&mut store, &mut a, p.as_mut()),
            Err(e) => {
                a.error = Some(e.clone());
                let _ = store.update_account(&a);
                Report { errors: vec![e], ..Default::default() }
            }
        };
        out.push(AccountResult { account_id: a.id.clone(), name: a.name.clone(), report });
    }
    drop(lock);
    Ok(Outcome::Done(out))
}

/// The service client for an account, with its secret from the keyring.
pub fn provider_for(a: &Account) -> Result<Box<dyn Provider>, String> {
    match a.kind {
        Kind::CalDav | Kind::ICloud => {
            let password = accounts::get_secret(&a.id)?.ok_or("The password is missing from the keyring. Add the account again.")?;
            Ok(Box::new(CalDav::new(&a.url, &a.username, &password).map_err(|e| e.to_string())?))
        }
        Kind::Google => {
            let app = oauth::google_app().ok_or("Google sign-in is not set up in this build.")?;
            accounts::get_secret(&a.id)?.ok_or("The Google login is missing from the keyring. Sign in again.")?;
            let email = if a.username.is_empty() { a.addresses.first().cloned().unwrap_or_default() } else { a.username.clone() };
            Ok(Box::new(Google::new(OAuthTokens::new(app, a.id.clone(), None), email)))
        }
    }
}

/// Short status line for one run, e.g. "Fastmail: 3 up, 2 down".
pub fn summary(r: &AccountResult) -> String {
    let rep = &r.report;
    if let Some(e) = rep.errors.first() {
        return format!("{}: {e}", r.name);
    }
    let mut parts = Vec::new();
    if rep.up > 0 {
        parts.push(format!("{} up", rep.up));
    }
    if rep.down > 0 {
        parts.push(format!("{} down", rep.down));
    }
    if rep.conflicts > 0 {
        parts.push(format!("{} conflict{}", rep.conflicts, if rep.conflicts == 1 { "" } else { "s" }));
    }
    if parts.is_empty() {
        parts.push("up to date".into());
    }
    format!("{}: {}", r.name, parts.join(", "))
}
