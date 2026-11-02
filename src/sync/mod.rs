//! Two-way sync. A [`Provider`] talks to one service (CalDAV, iCloud,
//! Google, later Outlook); the engine merges, queues and pushes. Providers
//! never touch the store.
//!
//! The unit of sync is an *object*: one event, or one repeating series with
//! its changed instances (all rows sharing a UID).

pub mod caldav;
pub mod engine;
pub mod fake;
pub mod google;
pub mod runner;
pub mod state;

use crate::model::Event;

/// A calendar on the server.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteCalendar {
    /// CalDAV: collection URL. Google: calendar id.
    pub remote_id: String,
    pub name: String,
    /// `#rrggbb`, when the server has one.
    pub color: Option<String>,
    pub read_only: bool,
}

/// One object as the server has it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteObject {
    /// CalDAV: object URL. Google: event id.
    pub href: String,
    /// Version tag for safe updates. May be empty when the server has none.
    pub etag: String,
    /// The series (or single event) and its changed instances. `id` and
    /// `calendar` are empty: the engine fills them.
    pub events: Vec<Event>,
    /// The object as the server sent it (iCalendar text, JSON…). Uploads
    /// start from it, so data Calinator does not show is not lost.
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Upsert(RemoteObject),
    /// `href` of an object deleted on the server.
    Deleted(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pull {
    pub changes: Vec<Change>,
    /// Pass back next time for a delta. `None`: the service has no deltas.
    pub token: Option<String>,
    /// True when `changes` holds every object: any object we know that is
    /// not in it was deleted on the server.
    pub full: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Login needed again (401, revoked token, wrong password).
    Auth(String),
    /// No network, DNS, timeout.
    Network(String),
    /// Server said no (status, message).
    Server(u16, String),
    /// We could not read what the server sent.
    Parse(String),
    /// The object changed on the server since we last saw it (412).
    Conflict,
    /// The object is gone on the server (404 / 410).
    NotFound,
    /// The delta token expired: pull again with no token.
    TokenExpired,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Auth(m) => write!(f, "login needed: {m}"),
            Error::Network(m) => write!(f, "offline: {m}"),
            Error::Server(code, m) => write!(f, "server error {code}: {m}"),
            Error::Parse(m) => write!(f, "could not read server reply: {m}"),
            Error::Conflict => f.write_str("changed on the server"),
            Error::NotFound => f.write_str("gone from the server"),
            Error::TokenExpired => f.write_str("sync token expired"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// One service account. All calls block; the engine runs them off the UI thread.
pub trait Provider: Send {
    /// The user's own addresses on this account (to find their answer in invites).
    fn my_addresses(&mut self) -> Result<Vec<String>>;
    /// Calendars that hold events.
    fn calendars(&mut self) -> Result<Vec<RemoteCalendar>>;
    /// Changes in a calendar since `token` (every object when `None`).
    fn pull(&mut self, calendar: &str, token: Option<&str>) -> Result<Pull>;
    /// Creates (`href` None) or updates an object. Updates send `etag` and
    /// fail with `Err(Conflict)` when it is stale. `notify`: the service may
    /// email guests about this change.
    /// `raw`: the object's [`RemoteObject::raw`] from the server, when known;
    /// properties Calinator does not edit are kept from it.
    #[allow(clippy::too_many_arguments)]
    fn put(
        &mut self,
        calendar: &str,
        href: Option<&str>,
        etag: &str,
        raw: Option<&str>,
        events: &[Event],
        notify: bool,
    ) -> Result<RemoteObject>;
    /// Deletes an object; Ok when already gone. `Err(Conflict)` when `etag` is stale.
    fn delete(&mut self, calendar: &str, href: &str, etag: &str, notify: bool) -> Result<()>;
}

/// Maps a ureq error to a sync error.
pub fn http_error(e: ureq::Error) -> Error {
    match e {
        ureq::Error::StatusCode(401 | 403) => Error::Auth("the server did not accept the login".into()),
        ureq::Error::StatusCode(404 | 410) => Error::NotFound,
        ureq::Error::StatusCode(412) => Error::Conflict,
        ureq::Error::StatusCode(c) => Error::Server(c, String::new()),
        ureq::Error::Io(e) => Error::Network(e.to_string()),
        ureq::Error::HostNotFound => Error::Network("server not found".into()),
        ureq::Error::ConnectionFailed => Error::Network("could not connect".into()),
        ureq::Error::Timeout(_) => Error::Network("timed out".into()),
        other => Error::Network(other.to_string()),
    }
}
