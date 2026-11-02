//! Online accounts and their secrets. Passwords and sign-in tokens live in
//! the system keyring (Secret Service), never in the calendar file.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use keyring_core::api::CredentialStoreApi;
use keyring_core::{Entry, Error as KeyringError};
use serde::{Deserialize, Serialize};
use zbus_secret_service_keyring_store::Store as SecretStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Any CalDAV server: Fastmail, Nextcloud, Radicale…
    CalDav,
    /// iCloud: CalDAV with an app-specific password.
    ICloud,
    Google,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::CalDav => "CalDAV",
            Kind::ICloud => "iCloud",
            Kind::Google => "Google",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    pub id: String,
    pub kind: Kind,
    /// Shown in the app, e.g. "Fastmail" or the email address.
    pub name: String,
    /// CalDAV server address.
    pub url: String,
    pub username: String,
    /// The user's own addresses (to find their answer in invites).
    pub addresses: Vec<String>,
    pub last_sync: Option<DateTime<Utc>>,
    /// Last sync problem, in simple words; `None` when the last sync worked.
    pub error: Option<String>,
}

impl Default for Account {
    fn default() -> Self {
        Account {
            id: String::new(),
            kind: Kind::CalDav,
            name: String::new(),
            url: String::new(),
            username: String::new(),
            addresses: vec![],
            last_sync: None,
            error: None,
        }
    }
}

pub const ICLOUD_URL: &str = "https://caldav.icloud.com/";
/// Apple's page for making an app-specific password.
pub const ICLOUD_PASSWORD_PAGE: &str = "https://account.apple.com/account/manage";

// ---------------------------------------------------------------- keyring

/// Keyring service name for every secret we store.
const SERVICE: &str = "calinator";

const NO_KEYRING: &str = "No keyring found. Start gnome-keyring or KeePassXC.";

/// The Secret Service connection: made on first use, dropped when it breaks.
static STORE: Mutex<Option<Arc<SecretStore>>> = Mutex::new(None);

/// Saves a secret (password or refresh token) for an account.
pub fn set_secret(account_id: &str, value: &str) -> Result<(), String> {
    match entry(account_id)?.set_password(value) {
        Err(KeyringError::Ambiguous(found)) => found.iter().try_for_each(|e| e.set_password(value)).map_err(keyring_error),
        other => other.map_err(keyring_error),
    }
}

/// The saved secret for an account; `None` when there is none.
pub fn get_secret(account_id: &str) -> Result<Option<String>, String> {
    match entry(account_id)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(KeyringError::Ambiguous(found)) => found.first().map(Entry::get_password).transpose().map_err(keyring_error),
        Err(e) => Err(keyring_error(e)),
    }
}

/// Removes the secret for an account. Ok when there is none.
pub fn delete_secret(account_id: &str) -> Result<(), String> {
    match entry(account_id)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(KeyringError::Ambiguous(found)) => found.iter().try_for_each(Entry::delete_credential).map_err(keyring_error),
        Err(e) => Err(keyring_error(e)),
    }
}

fn entry(account_id: &str) -> Result<Entry, String> {
    let store = {
        let mut slot = STORE.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some(store) => store.clone(),
            None => {
                let store = SecretStore::new().map_err(|_| NO_KEYRING.to_string())?;
                *slot = Some(store.clone());
                store
            }
        }
    };
    store.build(SERVICE, account_id, None).map_err(keyring_error)
}

/// Simple words for keyring failures. A failed connection is dropped so the
/// next call connects again (the keyring may have been restarted).
fn keyring_error(e: KeyringError) -> String {
    match e {
        KeyringError::NoStorageAccess(_) => "The keyring is locked. Unlock it and try again.".to_string(),
        KeyringError::PlatformFailure(e) => {
            STORE.lock().unwrap_or_else(|p| p.into_inner()).take();
            format!("Keyring error: {e}")
        }
        other => format!("Keyring error: {other}"),
    }
}
