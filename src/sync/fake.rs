//! An in-memory calendar server for tests: behaves like a CalDAV server with
//! sync tokens and ETags, and can be switched offline.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{Change, Error, Provider, Pull, RemoteCalendar, RemoteObject, Result};
use crate::model::Event;

#[derive(Debug, Default)]
pub struct Server {
    /// calendar id → name, read-only.
    pub calendars: BTreeMap<String, (String, bool)>,
    /// (calendar, href) → (etag, events).
    pub objects: BTreeMap<(String, String), (String, Vec<Event>)>,
    /// Change log: (version, calendar, href). Tokens are versions.
    log: Vec<(u64, String, String)>,
    version: u64,
    pub offline: bool,
    /// Every `notify` flag the server was given, in order.
    pub notified: Vec<bool>,
    /// Every `raw` an upload started from, in order.
    pub raws: Vec<Option<String>>,
}

impl Server {
    pub fn add_calendar(&mut self, id: &str, name: &str) {
        self.calendars.insert(id.into(), (name.into(), false));
    }

    fn bump(&mut self, cal: &str, href: &str) -> String {
        self.version += 1;
        self.log.push((self.version, cal.into(), href.into()));
        format!("\"{}\"", self.version)
    }

    /// A change made on another device (phone, web).
    pub fn put_from_elsewhere(&mut self, cal: &str, events: Vec<Event>) -> String {
        let href = format!("{cal}/{}.ics", events[0].uid);
        let etag = self.bump(cal, &href);
        self.objects.insert((cal.into(), href.clone()), (etag, events));
        href
    }

    pub fn delete_from_elsewhere(&mut self, cal: &str, href: &str) {
        self.objects.remove(&(cal.into(), href.into()));
        self.bump(cal, href);
    }

    pub fn object(&self, cal: &str, uid: &str) -> Option<&Vec<Event>> {
        self.objects.iter().find(|((c, _), (_, ev))| c == cal && ev.first().is_some_and(|e| e.uid == uid)).map(|(_, (_, ev))| ev)
    }
}

/// A provider talking to a shared [`Server`].
#[derive(Clone, Default)]
pub struct Fake(pub Arc<Mutex<Server>>);

fn clean(events: &[Event]) -> Vec<Event> {
    events.iter().map(|e| Event { id: String::new(), calendar: String::new(), ..e.clone() }).collect()
}

impl Fake {
    fn with<T>(&self, f: impl FnOnce(&mut Server) -> Result<T>) -> Result<T> {
        let mut s = self.0.lock().expect("server");
        if s.offline {
            return Err(Error::Network("offline".into()));
        }
        f(&mut s)
    }
}

/// The fake's "server text" of an object: just its version.
fn raw_of(etag: &str) -> String {
    format!("raw {etag}")
}

impl Provider for Fake {
    fn my_addresses(&mut self) -> Result<Vec<String>> {
        self.with(|_| Ok(vec!["me@example.com".into()]))
    }

    fn calendars(&mut self) -> Result<Vec<RemoteCalendar>> {
        self.with(|s| {
            Ok(s.calendars
                .iter()
                .map(|(id, (name, ro))| RemoteCalendar { remote_id: id.clone(), name: name.clone(), color: None, read_only: *ro })
                .collect())
        })
    }

    fn pull(&mut self, calendar: &str, token: Option<&str>) -> Result<Pull> {
        self.with(|s| {
            let since: Option<u64> = match token {
                Some(t) => Some(t.parse().map_err(|_| Error::TokenExpired)?),
                None => None,
            };
            let mut changes = Vec::new();
            match since {
                None => {
                    for ((c, href), (etag, ev)) in &s.objects {
                        if c == calendar {
                            changes.push(Change::Upsert(RemoteObject {
                                href: href.clone(),
                                etag: etag.clone(),
                                raw: raw_of(etag),
                                events: ev.clone(),
                            }));
                        }
                    }
                }
                Some(v) => {
                    let mut hrefs: Vec<&String> = s.log.iter().filter(|(n, c, _)| *n > v && c == calendar).map(|(_, _, h)| h).collect();
                    hrefs.dedup();
                    for h in hrefs {
                        changes.push(match s.objects.get(&(calendar.to_string(), h.clone())) {
                            Some((etag, ev)) => {
                                Change::Upsert(RemoteObject { href: h.clone(), etag: etag.clone(), raw: raw_of(etag), events: ev.clone() })
                            }
                            None => Change::Deleted(h.clone()),
                        });
                    }
                }
            }
            Ok(Pull { changes, token: Some(s.version.to_string()), full: since.is_none() })
        })
    }

    fn put(
        &mut self,
        calendar: &str,
        href: Option<&str>,
        etag: &str,
        raw: Option<&str>,
        events: &[Event],
        notify: bool,
    ) -> Result<RemoteObject> {
        self.with(|s| {
            s.notified.push(notify);
            s.raws.push(raw.map(String::from));
            let href = match href {
                Some(h) => {
                    let (cur, _) = s.objects.get(&(calendar.to_string(), h.to_string())).ok_or(Error::NotFound)?;
                    if cur != etag {
                        return Err(Error::Conflict);
                    }
                    h.to_string()
                }
                None => format!("{calendar}/{}.ics", events[0].uid),
            };
            let etag = s.bump(calendar, &href);
            let events = clean(events);
            s.objects.insert((calendar.into(), href.clone()), (etag.clone(), events.clone()));
            Ok(RemoteObject { href, raw: raw_of(&etag), etag, events })
        })
    }

    fn delete(&mut self, calendar: &str, href: &str, etag: &str, notify: bool) -> Result<()> {
        self.with(|s| {
            s.notified.push(notify);
            match s.objects.get(&(calendar.to_string(), href.to_string())) {
                None => Ok(()),
                Some((cur, _)) if !etag.is_empty() && cur != etag => Err(Error::Conflict),
                Some(_) => {
                    s.objects.remove(&(calendar.into(), href.into()));
                    s.bump(calendar, href);
                    Ok(())
                }
            }
        })
    }
}
