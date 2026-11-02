//! Google Calendar (API v3).
//!
//! An object is a series: its master event plus its exceptions (changed or
//! cancelled instances, which carry `recurringEventId` and `originalStartTime`),
//! or one single event. `href` is the master's event id; the UID is Google's
//! `iCalUID` (event ids are not UIDs). Instances of a series we cannot see
//! (an invite to one instance) form an object of overrides only, under the
//! series id.
//!
//! - A cancelled instance is an override row with `status: Cancelled`. A
//!   cancelled master or single event is a deleted object.
//! - `RRULE` / `RDATE` / `EXDATE` lines (with `TZID`, `Z` or `VALUE=DATE`) map
//!   to `rrule` / `rdates` / `exdates` in the event's zone; `EXRULE` lines stay
//!   in the raw copy and are written back as they were.
//! - Deleting one instance here adds an `EXDATE` to the series. A changed
//!   instance on Google that is no longer part of the series here (excluded,
//!   or past a new end) is cancelled there.
//! - "This and following" (see `edit.rs`) ends the series with `UNTIL` and
//!   makes a new series with a new UID, so Google gets one patched series and
//!   one new one, the same as in its own app.
//! - Reminders: `useDefault` shows the calendar's default reminders; saving the
//!   same list keeps `useDefault`. Other lists are written as overrides (popup,
//!   or the methods Google already had for that minute). At most five.
//! - Video links come from conference data or `hangoutLink` and are not
//!   written back (Meet links cannot be made by hand).
//! - Working location entries are not calendar events and are skipped.
//!
//! `raw` is `{"master": <event JSON or null>, "exceptions": [<event JSON>…]}`.
//! `etag` is the master's etag, then each exception's, joined by commas, so it
//! changes when any part of the object changes.
//!
//! Writes are PATCH requests holding only the fields that differ from `raw`,
//! so data Calinator does not show is kept, and a guest's answer is an
//! attendees-only patch (allowed for guests). Guests get email only when
//! `notify` is set (`sendUpdates=all`, else `none`).

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use ureq::http::Response;
use ureq::{Agent, Body};

use super::{Change, Error, Provider, Pull, RemoteCalendar, RemoteObject, Result, http_error};
use crate::model::{Attendee, Event, Partstat, Status};
use crate::oauth::{OAuthTokens, url_encode};
use crate::{recur, tz};

const BASE: &str = "https://www.googleapis.com/calendar/v3";
/// Largest page Google allows for events.list.
const PAGE: &str = "2500";
/// Google keeps at most this many reminder overrides.
const MAX_REMINDERS: usize = 5;

/// What a calendar adds to its events: its zone (for times without one) and
/// the user's default reminders on it.
#[derive(Debug, Clone, Default, PartialEq)]
struct CalInfo {
    zone: String,
    defaults: Vec<i64>,
}

impl CalInfo {
    /// From a calendarList entry or an events.list reply.
    fn from_json(v: &Value) -> CalInfo {
        CalInfo { zone: v["timeZone"].as_str().unwrap_or("UTC").to_string(), defaults: minutes(&v["defaultReminders"]) }
    }
}

/// One Google account.
pub struct Google {
    agent: Agent,
    tokens: OAuthTokens,
    email: String,
    base: String,
    cals: HashMap<String, CalInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Method {
    Get,
    Post,
    Patch,
    Delete,
}

/// Every page of one events.list call.
struct Listing {
    items: Vec<Value>,
    info: CalInfo,
    sync_token: Option<String>,
}

impl Google {
    /// `email`: the account's address (from the login).
    pub fn new(tokens: OAuthTokens, email: String) -> Self {
        let config = Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).http_status_as_error(false).build();
        Google { agent: Agent::new_with_config(config), tokens, email, base: BASE.to_string(), cals: HashMap::new() }
    }

    /// One API call, any status. On 401 the token is refreshed once and the call retried.
    fn request(
        &mut self,
        method: Method,
        url: &str,
        query: &[(&str, &str)],
        body: Option<&str>,
        if_match: Option<&str>,
    ) -> Result<(u16, String)> {
        let token = self.tokens.token()?;
        let mut resp = call(&self.agent, method, url, query, body, if_match, &token)?;
        if resp.status().as_u16() == 401 {
            let token = self.tokens.refresh()?;
            resp = call(&self.agent, method, url, query, body, if_match, &token)?;
        }
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(http_error)?;
        Ok((status, text))
    }

    /// One API call; the reply body of a 2xx answer.
    fn send(&mut self, method: Method, url: &str, query: &[(&str, &str)], body: Option<&str>, if_match: Option<&str>) -> Result<String> {
        let (status, text) = self.request(method, url, query, body, if_match)?;
        if (200..300).contains(&status) { Ok(text) } else { Err(status_error(status, &text)) }
    }

    fn events_url(&self, calendar: &str) -> String {
        format!("{}/calendars/{}/events", self.base, url_encode(calendar))
    }

    fn event_url(&self, calendar: &str, id: &str) -> String {
        format!("{}/{}", self.events_url(calendar), url_encode(id))
    }

    /// events.list, all pages, deleted entries and exceptions included,
    /// series not expanded. 410: the sync token expired.
    fn list(&mut self, calendar: &str, extra: &[(&str, &str)]) -> Result<Listing> {
        let url = self.events_url(calendar);
        let mut items = Vec::new();
        let mut info = None;
        let mut page: Option<String> = None;
        loop {
            let (status, text) = {
                let mut query = vec![("maxResults", PAGE), ("showDeleted", "true"), ("singleEvents", "false")];
                query.extend_from_slice(extra);
                if let Some(p) = &page {
                    query.push(("pageToken", p.as_str()));
                }
                self.request(Method::Get, &url, &query, None, None)?
            };
            if status == 410 {
                return Err(Error::TokenExpired);
            }
            if !(200..300).contains(&status) {
                return Err(status_error(status, &text));
            }
            let mut v: Value = serde_json::from_str(&text).map_err(parse_error)?;
            let info = info.get_or_insert_with(|| CalInfo::from_json(&v));
            if let Value::Array(a) = v["items"].take() {
                items.extend(a);
            }
            match v["nextPageToken"].as_str() {
                Some(p) => page = Some(p.to_string()),
                None => {
                    let info = info.clone();
                    self.cals.insert(calendar.to_string(), info.clone());
                    return Ok(Listing { items, info, sync_token: v["nextSyncToken"].as_str().map(str::to_string) });
                }
            }
        }
    }

    /// Every object with this UID, as the server has it now.
    fn fetch(&mut self, calendar: &str, uid: &str) -> Result<Grouped> {
        let listing = self.list(calendar, &[("iCalUID", uid)])?;
        Ok(group(&listing.items, &listing.info))
    }

    /// The UID of an event, `None` when it is gone.
    fn uid_of(&mut self, calendar: &str, id: &str) -> Result<Option<String>> {
        let url = self.event_url(calendar, id);
        match self.send(Method::Get, &url, &[], None, None) {
            Ok(text) => {
                let v: Value = serde_json::from_str(&text).map_err(parse_error)?;
                Ok(if cancelled(&v) { None } else { v["iCalUID"].as_str().map(str::to_string) })
            }
            Err(Error::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Zone and default reminders of a calendar.
    fn cal_info(&mut self, calendar: &str) -> Result<CalInfo> {
        if let Some(info) = self.cals.get(calendar) {
            return Ok(info.clone());
        }
        let url = format!("{}/users/me/calendarList/{}", self.base, url_encode(calendar));
        let v: Value = serde_json::from_str(&self.send(Method::Get, &url, &[], None, None)?).map_err(parse_error)?;
        let info = CalInfo::from_json(&v);
        self.cals.insert(calendar.to_string(), info.clone());
        Ok(info)
    }

    /// The object as stored after a write.
    fn stored(&mut self, calendar: &str, uid: &str, master_id: &str) -> Result<RemoteObject> {
        let found = self.fetch(calendar, uid)?.objects.into_iter().find(|o| o.href == master_id);
        if let Some(o) = found {
            return Ok(o);
        }
        // The UID search may lag behind a write: read the event itself.
        let url = self.event_url(calendar, master_id);
        let v: Value = serde_json::from_str(&self.send(Method::Get, &url, &[], None, None)?).map_err(parse_error)?;
        let info = self.cal_info(calendar)?;
        group([&v], &info).objects.into_iter().next().ok_or(Error::NotFound)
    }

    /// Sets an instance's status to cancelled (hides it). Ok when already gone.
    fn cancel_instance(&mut self, calendar: &str, id: &str, query: &[(&str, &str)]) -> Result<()> {
        let url = self.event_url(calendar, id);
        let body = json!({ "status": "cancelled" }).to_string();
        match self.send(Method::Patch, &url, query, Some(&body), None) {
            Ok(_) | Err(Error::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn call(
    agent: &Agent,
    method: Method,
    url: &str,
    query: &[(&str, &str)],
    body: Option<&str>,
    if_match: Option<&str>,
    token: &str,
) -> Result<Response<Body>> {
    let auth = format!("Bearer {token}");
    let query = query.iter().copied();
    let res = match method {
        Method::Get | Method::Delete => {
            let req = if method == Method::Get { agent.get(url) } else { agent.delete(url) };
            let req = req.header("Authorization", auth.as_str()).header("Accept", "application/json").query_pairs(query);
            match if_match {
                Some(tag) => req.header("If-Match", tag).call(),
                None => req.call(),
            }
        }
        Method::Post | Method::Patch => {
            let req = if method == Method::Post { agent.post(url) } else { agent.patch(url) };
            let mut req = req.header("Authorization", auth.as_str()).header("Accept", "application/json").query_pairs(query);
            if let Some(tag) = if_match {
                req = req.header("If-Match", tag);
            }
            req.content_type("application/json").send(body.unwrap_or("{}"))
        }
    };
    res.map_err(http_error)
}

impl Provider for Google {
    fn my_addresses(&mut self) -> Result<Vec<String>> {
        Ok(if self.email.is_empty() { vec![] } else { vec![self.email.clone()] })
    }

    fn calendars(&mut self) -> Result<Vec<RemoteCalendar>> {
        let url = format!("{}/users/me/calendarList", self.base);
        let mut cals = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let next = {
                let mut query = vec![("maxResults", "250"), ("minAccessRole", "reader")];
                if let Some(p) = &page {
                    query.push(("pageToken", p.as_str()));
                }
                let text = self.send(Method::Get, &url, &query, None, None)?;
                calendar_page(&text, &mut cals, &mut self.cals)?
            };
            page = next;
            if page.is_none() {
                return Ok(cals);
            }
        }
    }

    /// A full pull lists every event; a delta lists what changed since the
    /// sync token. A series touched by a delta is read again whole (master
    /// and every exception), because objects are replaced whole.
    fn pull(&mut self, calendar: &str, token: Option<&str>) -> Result<Pull> {
        let Some(token) = token else {
            let listing = self.list(calendar, &[])?;
            let g = group(&listing.items, &listing.info);
            return Ok(Pull { changes: g.objects.into_iter().map(Change::Upsert).collect(), token: listing.sync_token, full: true });
        };
        let listing = self.list(calendar, &[("syncToken", token)])?;
        let d = delta(&listing.items, &listing.info);
        let mut changes = d.changes;
        for (master, uid) in d.series {
            let uid = match uid {
                Some(u) => Some(u),
                None => self.uid_of(calendar, &master)?,
            };
            let Some(uid) = uid else {
                changes.push(Change::Deleted(master));
                continue;
            };
            let g = self.fetch(calendar, &uid)?;
            if !g.objects.iter().any(|o| o.href == master) && !g.deleted.contains(&master) {
                changes.push(Change::Deleted(master.clone()));
            }
            changes.extend(g.deleted.into_iter().map(Change::Deleted));
            changes.extend(g.objects.into_iter().map(Change::Upsert));
        }
        Ok(Pull { changes, token: listing.sync_token, full: false })
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
        let Some(uid) = events.first().map(|e| e.uid.clone()).filter(|u| !u.is_empty()) else {
            return Err(Error::Server(0, "an event without a UID cannot be saved".into()));
        };
        let query = [("sendUpdates", send_updates(notify))];
        let info = self.cal_info(calendar)?;
        let mut raw = Raw::parse(raw);
        let master = events.iter().find(|e| e.recurrence_id.is_none());
        let overrides: Vec<&Event> = events.iter().filter(|e| e.recurrence_id.is_some()).collect();

        // The master (or single event): create, or patch what changed.
        let mut guard = Some(etag.split(',').next().unwrap_or_default()).filter(|t| !t.is_empty());
        let master_id = match href {
            Some(h) => h.to_string(),
            None => {
                let Some(m) = master else {
                    return Err(Error::Server(0, "a changed instance cannot be saved without its series".into()));
                };
                let mut body = merged(&json!({}), m, &info, true);
                body["iCalUID"] = uid.clone().into();
                let url = self.events_url(calendar);
                match self.send(Method::Post, &url, &query, Some(&body.to_string()), None) {
                    Ok(text) => {
                        let v: Value = serde_json::from_str(&text).map_err(parse_error)?;
                        let id = v["id"].as_str().ok_or_else(|| Error::Parse("event without id".into()))?.to_string();
                        raw = Raw { master: v, exceptions: vec![] };
                        id
                    }
                    Err(Error::Server(409, _)) => {
                        // Made by an earlier try whose reply was lost: update it.
                        let found = self.fetch(calendar, &uid)?.objects.into_iter().next().ok_or(Error::NotFound)?;
                        raw = Raw::parse(Some(&found.raw));
                        guard = None;
                        found.href
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        if master.is_some() && !raw.master.is_object() {
            // No saved copy to compare with: start from the server's.
            let url = self.event_url(calendar, &master_id);
            raw.master = serde_json::from_str(&self.send(Method::Get, &url, &[], None, None)?).map_err(parse_error)?;
        }
        let mut master_patched = false;
        if let Some(m) = master
            && raw.master.is_object()
        {
            let base = event_from_json(&raw.master, &info, None);
            let body = patch_body(m, Some(&base), &raw.master, &info, true);
            if !body.is_empty() {
                let url = self.event_url(calendar, &master_id);
                self.send(Method::Patch, &url, &query, Some(&Value::Object(body).to_string()), guard)?;
                master_patched = true;
            }
        }

        // Instances. The series' zone names them.
        let series_zone = master.map(|m| m.tz.clone()).or_else(|| raw.master["start"]["timeZone"].as_str().map(str::to_string));
        let zone_name = series_zone.unwrap_or_else(|| info.zone.clone());
        let zone = tz::parse(&zone_name, Tz::UTC);
        let server: Vec<(Event, Value)> = raw.exceptions.iter().map(|x| (event_from_json(x, &info, Some(&zone_name)), x.clone())).collect();

        // Changed instances on the server that are no longer part of the
        // series here (deleted one, or past a new end): cancel them.
        for (ev, x) in &server {
            let Some(original) = ev.recurrence_id else { continue };
            if ev.status == Status::Cancelled || overrides.iter().any(|o| o.recurrence_id == Some(original)) {
                continue;
            }
            if master.is_some_and(|m| is_instance(m, original)) {
                continue;
            }
            if let Some(id) = x["id"].as_str() {
                self.cancel_instance(calendar, id, &query)?;
            }
        }

        for o in &overrides {
            let original = o.recurrence_id.expect("filtered");
            let found = server.iter().find(|(e, _)| e.recurrence_id == Some(original));
            let all_day = master.map_or(o.all_day, |m| m.all_day);
            let id = match found.and_then(|(_, x)| x["id"].as_str()) {
                Some(id) => id.to_string(),
                None => instance_id(&master_id, original, all_day, zone),
            };
            if o.status == Status::Cancelled {
                if found.is_none_or(|(e, _)| e.status != Status::Cancelled) {
                    self.cancel_instance(calendar, &id, &query)?;
                }
                continue;
            }
            let body = match (found, master) {
                (Some((base, x)), _) => patch_body(o, Some(base), x, &info, false),
                (None, Some(m)) => patch_body(o, Some(&instance_of(m, original)), &raw.master, &info, false),
                (None, None) => patch_body(o, None, &raw.master, &info, false),
            };
            if body.is_empty() {
                continue;
            }
            let tag = if master_patched { None } else { found.and_then(|(_, x)| x["etag"].as_str()) };
            let url = self.event_url(calendar, &id);
            match self.send(Method::Patch, &url, &query, Some(&Value::Object(body).to_string()), tag) {
                Ok(_) => {}
                Err(Error::NotFound) => {
                    return Err(Error::Server(404, format!("the instance on {} is not part of the series on Google", o.start.date())));
                }
                Err(e) => return Err(e),
            }
        }
        self.stored(calendar, &uid, &master_id)
    }

    fn delete(&mut self, calendar: &str, href: &str, etag: &str, notify: bool) -> Result<()> {
        let tag = etag.split(',').next().filter(|t| !t.is_empty());
        let url = self.event_url(calendar, href);
        match self.send(Method::Delete, &url, &[("sendUpdates", send_updates(notify))], None, tag) {
            Ok(_) | Err(Error::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// `sendUpdates`: email guests only when asked.
fn send_updates(notify: bool) -> &'static str {
    if notify { "all" } else { "none" }
}

/// The raw copy of an object (see the module docs).
#[derive(Debug, Default)]
struct Raw {
    master: Value,
    exceptions: Vec<Value>,
}

impl Raw {
    fn parse(raw: Option<&str>) -> Raw {
        let Some(v) = raw.and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return Raw::default() };
        Raw { master: v["master"].clone(), exceptions: v["exceptions"].as_array().cloned().unwrap_or_default() }
    }
}

// ---------------------------------------------------------------- reading

/// Adds the calendars of one calendarList page; returns the next page token.
fn calendar_page(text: &str, cals: &mut Vec<RemoteCalendar>, infos: &mut HashMap<String, CalInfo>) -> Result<Option<String>> {
    let page: Value = serde_json::from_str(text).map_err(parse_error)?;
    for item in page["items"].as_array().into_iter().flatten() {
        let role = item["accessRole"].as_str().unwrap_or_default();
        if item["deleted"].as_bool() == Some(true) || matches!(role, "freeBusyReader" | "none") {
            continue;
        }
        let id = item["id"].as_str().ok_or_else(|| Error::Parse("calendar without id".into()))?;
        let name = item["summaryOverride"].as_str().filter(|s| !s.is_empty()).or(item["summary"].as_str()).unwrap_or(id);
        infos.insert(id.to_string(), CalInfo::from_json(item));
        cals.push(RemoteCalendar {
            remote_id: id.to_string(),
            name: name.to_string(),
            color: item["backgroundColor"].as_str().and_then(color),
            read_only: !matches!(role, "owner" | "writer"),
        });
    }
    Ok(page["nextPageToken"].as_str().map(str::to_string))
}

fn color(s: &str) -> Option<String> {
    let hex = s.strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// Objects made from a list of events.
#[derive(Debug, Default)]
struct Grouped {
    objects: Vec<RemoteObject>,
    /// Masters or single events that are cancelled (deleted).
    deleted: Vec<String>,
}

/// Groups events into objects: a master with its exceptions, or a single event.
fn group<'a>(items: impl IntoIterator<Item = &'a Value>, info: &CalInfo) -> Grouped {
    let mut order: Vec<&str> = Vec::new();
    let mut parts: HashMap<&str, (Option<&Value>, Vec<&Value>)> = HashMap::new();
    for v in items.into_iter().filter(|v| shown(v)) {
        let Some(id) = v["id"].as_str() else { continue };
        let key = v["recurringEventId"].as_str().unwrap_or(id);
        let part = parts.entry(key).or_insert_with(|| {
            order.push(key);
            (None, Vec::new())
        });
        if key == id { part.0 = Some(v) } else { part.1.push(v) }
    }
    let mut out = Grouped::default();
    for key in order {
        let Some((master, mut exceptions)) = parts.remove(key) else { continue };
        if master.is_some_and(cancelled) || (master.is_none() && exceptions.iter().all(|x| cancelled(x))) {
            out.deleted.push(key.to_string());
            continue;
        }
        exceptions.sort_by_key(|x| x["id"].as_str().unwrap_or_default().to_string());
        out.objects.push(object(key, master, &exceptions, info));
    }
    out
}

/// One object from its master (if we can see it) and exceptions.
fn object(href: &str, master: Option<&Value>, exceptions: &[&Value], info: &CalInfo) -> RemoteObject {
    let m = master.map(|v| event_from_json(v, info, None));
    let zone = m.as_ref().map(|e| e.tz.clone());
    let mut events: Vec<Event> = m.iter().cloned().collect();
    for x in exceptions {
        let mut e = event_from_json(x, info, zone.as_deref());
        if let Some(m) = &m {
            e.uid = m.uid.clone();
        }
        events.push(e);
    }
    let etag = std::iter::once(master.map_or("", |v| v["etag"].as_str().unwrap_or_default()))
        .chain(exceptions.iter().map(|x| x["etag"].as_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(",");
    let raw = json!({ "master": master.cloned().unwrap_or(Value::Null), "exceptions": exceptions }).to_string();
    RemoteObject { href: href.to_string(), etag, events, raw }
}

/// What a delta asks for: ready changes, and series to read again whole
/// (master id, UID when known).
#[derive(Debug, Default)]
struct Delta {
    changes: Vec<Change>,
    series: Vec<(String, Option<String>)>,
}

fn delta(items: &[Value], info: &CalInfo) -> Delta {
    let mut series: Vec<(String, Option<String>)> = Vec::new();
    let mut deleted: Vec<String> = Vec::new();
    let mut singles: Vec<&Value> = Vec::new();
    for v in items.iter().filter(|v| shown(v)) {
        let Some(id) = v["id"].as_str() else { continue };
        let uid = v["iCalUID"].as_str().map(str::to_string);
        let master = match v["recurringEventId"].as_str() {
            Some(m) => m,
            None if cancelled(v) => {
                deleted.push(id.to_string());
                continue;
            }
            None if v["recurrence"].as_array().is_some_and(|r| !r.is_empty()) => id,
            None => {
                singles.push(v);
                continue;
            }
        };
        match series.iter_mut().find(|(m, _)| m == master) {
            Some(s) => s.1 = s.1.take().or(uid),
            None => series.push((master.to_string(), uid)),
        }
    }
    // A deleted master takes its instances with it.
    series.retain(|(m, _)| !deleted.contains(m));
    singles.retain(|v| !series.iter().any(|(m, _)| v["id"].as_str() == Some(m)));
    let g = group(singles, info);
    let changes = g.objects.into_iter().map(Change::Upsert).chain(deleted.into_iter().map(Change::Deleted)).collect();
    Delta { changes, series }
}

/// Working location entries are not events.
fn shown(v: &Value) -> bool {
    v["eventType"].as_str() != Some("workingLocation")
}

fn cancelled(v: &Value) -> bool {
    v["status"].as_str() == Some("cancelled")
}

/// A start, end or original start.
#[derive(Debug, Clone, Copy, PartialEq)]
enum When {
    Day(NaiveDate),
    At(DateTime<Utc>),
}

fn when(v: &Value) -> Option<When> {
    if let Some(d) = v["date"].as_str() {
        return NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().map(When::Day);
    }
    v["dateTime"].as_str().and_then(parse_utc).map(When::At)
}

fn midnight(d: NaiveDate) -> NaiveDateTime {
    d.and_hms_opt(0, 0, 0).expect("midnight")
}

/// A Google event as ours. `series_zone`: the master's zone, for an
/// instance's original start.
fn event_from_json(v: &Value, info: &CalInfo, series_zone: Option<&str>) -> Event {
    let start = when(&v["start"]);
    let original = when(&v["originalStartTime"]);
    let named = v["start"]["timeZone"].as_str().or(series_zone).unwrap_or(&info.zone);
    let zone: Tz = named.parse().or_else(|_| info.zone.parse()).unwrap_or(Tz::UTC);
    let series: Tz = series_zone.and_then(|z| z.parse().ok()).unwrap_or(zone);
    let mut e = Event {
        uid: v["iCalUID"].as_str().or(v["id"].as_str()).unwrap_or_default().to_string(),
        title: text(&v["summary"]),
        location: text(&v["location"]),
        description: text(&v["description"]),
        tz: zone.name().to_string(),
        recurrence_id: original.map(|w| match w {
            When::Day(d) => midnight(d),
            When::At(t) => tz::local(t, series),
        }),
        organizer: v["organizer"]["email"].as_str().map(str::to_string),
        attendees: v["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|a| a["resource"].as_bool() != Some(true))
            .filter_map(|a| {
                Some(Attendee {
                    email: a["email"].as_str()?.to_string(),
                    name: text(&a["displayName"]),
                    status: partstat(a["responseStatus"].as_str().unwrap_or_default()),
                    me: a["self"].as_bool() == Some(true),
                })
            })
            .collect(),
        reminders: if v["reminders"]["useDefault"].as_bool() == Some(true) {
            info.defaults.clone()
        } else {
            minutes(&v["reminders"]["overrides"])
        },
        status: match v["status"].as_str() {
            Some("cancelled") => Status::Cancelled,
            Some("tentative") => Status::Tentative,
            _ => Status::Confirmed,
        },
        video: video(v),
        updated: v["updated"].as_str().and_then(parse_utc).unwrap_or_default(),
        sequence: v["sequence"].as_u64().and_then(|s| u32::try_from(s).ok()).unwrap_or(0),
        ..Event::default()
    };
    // Cancelled instances may come without times: use the original start.
    match start.or(original) {
        Some(When::Day(d)) => {
            e.all_day = true;
            e.start = midnight(d);
            e.end = match when(&v["end"]) {
                Some(When::Day(end)) if end > d => midnight(end),
                _ => midnight(d + chrono::Duration::days(1)),
            };
        }
        Some(When::At(t)) => {
            e.start = tz::local(t, zone);
            e.end = match when(&v["end"]) {
                Some(When::At(end)) if end >= t => tz::local(end, zone),
                _ => e.start,
            };
        }
        None => {}
    }
    for line in v["recurrence"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        read_recurrence(line, zone, e.all_day, &mut e);
    }
    e
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn partstat(s: &str) -> Partstat {
    match s {
        "accepted" => Partstat::Accepted,
        "tentative" => Partstat::Tentative,
        "declined" => Partstat::Declined,
        _ => Partstat::NeedsAction,
    }
}

fn partstat_name(p: Partstat) -> &'static str {
    match p {
        Partstat::NeedsAction => "needsAction",
        Partstat::Accepted => "accepted",
        Partstat::Tentative => "tentative",
        Partstat::Declined => "declined",
    }
}

/// Sorted, distinct reminder minutes.
fn minutes(list: &Value) -> Vec<i64> {
    let mut m: Vec<i64> = list.as_array().into_iter().flatten().filter_map(|r| r["minutes"].as_i64()).collect();
    m.sort_unstable();
    m.dedup();
    m
}

/// The video link: conference data first (Meet, Zoom add-on…), then `hangoutLink`.
fn video(v: &Value) -> Option<String> {
    v["conferenceData"]["entryPoints"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["entryPointType"].as_str() == Some("video"))
        .and_then(|p| p["uri"].as_str())
        .or(v["hangoutLink"].as_str())
        .filter(|u| !u.is_empty())
        .map(str::to_string)
}

/// One `recurrence` line into `e` (`zone`: the event's zone).
fn read_recurrence(line: &str, zone: Tz, all_day: bool, e: &mut Event) {
    let Some((head, value)) = line.split_once(':') else { return };
    let mut params = head.split(';');
    let name = params.next().unwrap_or_default().to_ascii_uppercase();
    let tzid: Option<Tz> = params.find_map(|p| p.strip_prefix("TZID=")).and_then(|z| z.trim_matches('"').parse().ok());
    let times: Vec<NaiveDateTime> = value.split(',').filter_map(|s| date_value(s, tzid, zone, all_day)).collect();
    match name.as_str() {
        "RRULE" => e.rrule = Some(value.to_string()),
        "RDATE" => e.rdates.extend(times),
        "EXDATE" => e.exdates.extend(times),
        _ => {}
    }
}

/// An RDATE / EXDATE value as wall time in `zone` (date-only: midnight).
fn date_value(s: &str, tzid: Option<Tz>, zone: Tz, all_day: bool) -> Option<NaiveDateTime> {
    // PERIOD values: the start counts.
    let s = s.trim().split('/').next()?;
    let t = if let Ok(d) = NaiveDate::parse_from_str(s, "%Y%m%d") {
        midnight(d)
    } else if let Some(utc) = s.strip_suffix('Z') {
        tz::local(NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?.and_utc(), zone)
    } else {
        let t = NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S").ok()?;
        match tzid {
            Some(from) if from != zone => tz::local(tz::resolve(t, from), zone),
            _ => t,
        }
    };
    Some(if all_day { midnight(t.date()) } else { t })
}

/// `recurrence` lines for a series. `EXRULE` lines are kept from `raw`.
fn recurrence_lines(ev: &Event, raw: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(r) = &ev.rrule {
        lines.push(format!("RRULE:{r}"));
    }
    lines.extend(
        raw["recurrence"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|l| l.to_ascii_uppercase().starts_with("EXRULE"))
            .map(str::to_string),
    );
    for (name, list) in [("RDATE", &ev.rdates), ("EXDATE", &ev.exdates)] {
        let mut list = list.clone();
        list.sort_unstable();
        list.dedup();
        if list.is_empty() {
            continue;
        }
        let line = if ev.all_day {
            let days: Vec<String> = list.iter().map(|t| t.format("%Y%m%d").to_string()).collect();
            format!("{name};VALUE=DATE:{}", days.join(","))
        } else {
            let times: Vec<String> = list.iter().map(|t| t.format("%Y%m%dT%H%M%S").to_string()).collect();
            format!("{name};TZID={}:{}", tz::parse(&ev.tz, Tz::UTC).name(), times.join(","))
        };
        lines.push(line);
    }
    lines
}

fn parse_utc(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

// ---------------------------------------------------------------- writing

/// Google's id of one instance: `<series id>_<original start in UTC>`, or
/// `_<date>` for all-day series.
fn instance_id(master: &str, original: NaiveDateTime, all_day: bool, zone: Tz) -> String {
    if all_day {
        format!("{master}_{}", original.format("%Y%m%d"))
    } else {
        format!("{master}_{}", tz::resolve(original, zone).format("%Y%m%dT%H%M%SZ"))
    }
}

/// A series' instance as the server makes it when unchanged.
fn instance_of(m: &Event, original: NaiveDateTime) -> Event {
    let mut e = m.clone();
    e.rrule = None;
    e.rdates.clear();
    e.exdates.clear();
    e.recurrence_id = Some(original);
    e.start = original;
    e.end = original + (m.end - m.start);
    e
}

/// `original` is still an instance of the series `m`.
fn is_instance(m: &Event, original: NaiveDateTime) -> bool {
    if !m.is_series() {
        return false;
    }
    let at = if m.all_day { original.and_utc() } else { tz::resolve(original, tz::parse(&m.tz, Tz::UTC)) };
    recur::instance_starts(m, Tz::UTC, at + chrono::Duration::seconds(1)).contains(&original)
}

/// The fields Calinator owns, as Google JSON. `raw`: the server's copy of
/// this event (attendee and reminder details are kept from it). Null values
/// clear a field.
fn fields(ev: &Event, raw: &Value, info: &CalInfo, master: bool) -> Map<String, Value> {
    let mut f = Map::new();
    f.insert("summary".into(), ev.title.clone().into());
    f.insert("location".into(), ev.location.clone().into());
    f.insert("description".into(), ev.description.clone().into());
    let (start, end) = times(ev);
    f.insert("start".into(), start);
    f.insert("end".into(), end);
    if master {
        let lines = if ev.is_series() { recurrence_lines(ev, raw).into() } else { Value::Null };
        f.insert("recurrence".into(), lines);
    }
    let status = match ev.status {
        Status::Confirmed => "confirmed",
        Status::Tentative => "tentative",
        Status::Cancelled => "cancelled",
    };
    f.insert("status".into(), status.into());
    f.insert("attendees".into(), attendees(ev, raw));
    f.insert("reminders".into(), reminders(ev, raw, info));
    f
}

fn times(ev: &Event) -> (Value, Value) {
    if ev.all_day {
        let day = |t: NaiveDateTime| json!({ "date": t.format("%Y-%m-%d").to_string(), "dateTime": null, "timeZone": null });
        let end = if ev.end.date() > ev.start.date() { ev.end } else { ev.start + chrono::Duration::days(1) };
        return (day(ev.start), day(end));
    }
    let zone = tz::parse(&ev.tz, Tz::UTC);
    let at = |t: NaiveDateTime| json!({ "dateTime": tz::resolve(t, zone).with_timezone(&zone).to_rfc3339(), "timeZone": zone.name(), "date": null });
    (at(ev.start), at(ev.end.max(ev.start)))
}

/// Guests, keeping what Google knows about each (comments, optional,
/// rooms…) from `raw`.
fn attendees(ev: &Event, raw: &Value) -> Value {
    let old: Vec<&Value> = raw["attendees"].as_array().into_iter().flatten().collect();
    let same = |o: &Value, email: &str| o["email"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(email));
    let mut out: Vec<Value> = ev
        .attendees
        .iter()
        .map(|a| {
            let mut j = old.iter().find(|o| same(o, &a.email)).map_or_else(|| json!({}), |o| (*o).clone());
            j["email"] = a.email.clone().into();
            if !a.name.is_empty() {
                j["displayName"] = a.name.clone().into();
            }
            j["responseStatus"] = partstat_name(a.status).into();
            j
        })
        .collect();
    // Rooms and resources are not shown, so they stay as they are.
    out.extend(
        old.into_iter().filter(|o| o["resource"].as_bool() == Some(true) && !ev.attendees.iter().any(|a| same(o, &a.email))).cloned(),
    );
    Value::Array(out)
}

/// Reminders: the calendar's defaults stay `useDefault`; other lists become
/// overrides, keeping the methods Google had for each minute.
fn reminders(ev: &Event, raw: &Value, info: &CalInfo) -> Value {
    let mut mine: Vec<i64> = ev.reminders.iter().copied().filter(|m| (0..=40_320).contains(m)).collect();
    mine.sort_unstable();
    mine.dedup();
    let was_default = raw["reminders"]["useDefault"].as_bool().unwrap_or(raw.get("reminders").is_none());
    if was_default && mine == info.defaults {
        return json!({ "useDefault": true, "overrides": null });
    }
    let old: Vec<&Value> = raw["reminders"]["overrides"].as_array().into_iter().flatten().collect();
    let overrides: Vec<Value> = mine
        .iter()
        .flat_map(|m| {
            let had: Vec<Value> = old.iter().filter(|o| o["minutes"].as_i64() == Some(*m)).map(|o| (*o).clone()).collect();
            if had.is_empty() { vec![json!({ "method": "popup", "minutes": m })] } else { had }
        })
        .take(MAX_REMINDERS)
        .collect();
    json!({ "useDefault": false, "overrides": overrides })
}

/// `raw` with our fields written over it (JSON merge patch): everything
/// Calinator does not know stays.
fn merged(raw: &Value, ev: &Event, info: &CalInfo, master: bool) -> Value {
    let mut out = raw.clone();
    merge_patch(&mut out, &Value::Object(fields(ev, raw, info, master)));
    out
}

/// RFC 7386: objects merge, null removes, anything else replaces.
fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(p) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let Value::Object(t) = target else { return };
    for (k, v) in p {
        if v.is_null() {
            t.remove(k);
        } else {
            merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
        }
    }
}

/// The PATCH body: our fields that differ from `base` (the event as the
/// server has it; `None`: send all). Start and end go together.
fn patch_body(ev: &Event, base: Option<&Event>, raw: &Value, info: &CalInfo, master: bool) -> Map<String, Value> {
    let ours = fields(ev, raw, info, master);
    let theirs = base.map(|b| fields(b, raw, info, master)).unwrap_or_default();
    let mut body: Map<String, Value> =
        ours.iter().filter(|(k, v)| theirs.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    if ["start", "end", "recurrence"].iter().any(|k| body.contains_key(*k)) {
        for k in ["start", "end"] {
            body.insert(k.into(), ours[k].clone());
        }
    }
    body
}

// ---------------------------------------------------------------- errors

/// Maps an error reply. Google answers 403 for rate limits and for changes a
/// guest may not make; only missing permissions mean signing in again.
fn status_error(status: u16, body: &str) -> Error {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    let message = v["error"]["message"].as_str().map_or_else(|| body.chars().take(200).collect(), str::to_string);
    let reason = v["error"]["errors"][0]["reason"].as_str().unwrap_or_default();
    match status {
        401 => Error::Auth(message),
        403 if matches!(reason, "insufficientPermissions" | "authError") => Error::Auth(message),
        404 | 410 => Error::NotFound,
        412 => Error::Conflict,
        _ => Error::Server(status, message),
    }
}

fn parse_error(e: serde_json::Error) -> Error {
    Error::Parse(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ny() -> CalInfo {
        CalInfo { zone: "America/New_York".into(), defaults: vec![10] }
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        parse_utc(s).unwrap()
    }

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    /// A full events.list reply: one weekly series with an excluded date, a
    /// moved instance and a cancelled one; a single event; a deleted event;
    /// a working location entry.
    const LISTING: &str = r#"{
 "kind": "calendar#events",
 "timeZone": "America/New_York",
 "defaultReminders": [{"method": "popup", "minutes": 10}],
 "nextSyncToken": "CPDAlvWDx70CEPDAlvWDx70CGAU=",
 "items": [
  {
   "kind": "calendar#event", "id": "abc", "etag": "\"3001\"", "status": "confirmed",
   "summary": "Standup", "iCalUID": "abc@google.com", "sequence": 2,
   "updated": "2026-10-15T12:00:00.000Z",
   "start": {"dateTime": "2026-10-20T09:00:00-04:00", "timeZone": "America/New_York"},
   "end": {"dateTime": "2026-10-20T09:30:00-04:00", "timeZone": "America/New_York"},
   "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=TU", "EXDATE;TZID=America/New_York:20261027T090000"],
   "reminders": {"useDefault": true}
  },
  {
   "kind": "calendar#event", "id": "abc_20261103T140000Z", "etag": "\"3002\"", "status": "confirmed",
   "summary": "Standup (late)", "iCalUID": "abc@google.com", "recurringEventId": "abc",
   "originalStartTime": {"dateTime": "2026-11-03T09:00:00-05:00", "timeZone": "America/New_York"},
   "start": {"dateTime": "2026-11-03T11:00:00-05:00", "timeZone": "America/New_York"},
   "end": {"dateTime": "2026-11-03T11:30:00-05:00", "timeZone": "America/New_York"},
   "reminders": {"useDefault": true}
  },
  {
   "kind": "calendar#event", "id": "abc_20261110T140000Z", "etag": "\"3003\"", "status": "cancelled",
   "recurringEventId": "abc", "originalStartTime": {"dateTime": "2026-11-10T14:00:00Z"}
  },
  {
   "kind": "calendar#event", "id": "single1", "etag": "\"4001\"", "status": "confirmed",
   "summary": "Dentist", "iCalUID": "single1@google.com",
   "start": {"dateTime": "2026-11-05T15:00:00Z"}, "end": {"dateTime": "2026-11-05T16:00:00Z"},
   "reminders": {"useDefault": false}
  },
  {"kind": "calendar#event", "id": "gone1", "etag": "\"5001\"", "status": "cancelled"},
  {
   "kind": "calendar#event", "id": "wl1", "eventType": "workingLocation", "status": "confirmed",
   "start": {"date": "2026-11-04"}, "end": {"date": "2026-11-05"}
  }
 ]
}"#;

    fn listing() -> (Vec<Value>, CalInfo) {
        let v = json(LISTING);
        (v["items"].as_array().unwrap().clone(), CalInfo::from_json(&v))
    }

    #[test]
    fn timed_event_uses_its_zone_or_the_calendars() {
        let v = json(
            r#"{"id": "e1", "iCalUID": "uid-1", "etag": "\"1\"", "status": "tentative", "summary": "Lunch",
                "location": "Cafe", "description": "Bring notes", "sequence": 3,
                "updated": "2026-10-30T09:15:02.000Z", "organizer": {"email": "boss@example.com"},
                "start": {"dateTime": "2026-11-05T12:00:00+01:00", "timeZone": "Europe/Berlin"},
                "end": {"dateTime": "2026-11-05T13:30:00+01:00", "timeZone": "Europe/Berlin"},
                "reminders": {"useDefault": false, "overrides": [{"method": "email", "minutes": 30}, {"method": "popup", "minutes": 5}]}}"#,
        );
        let e = event_from_json(&v, &ny(), None);
        assert_eq!(
            (e.uid.as_str(), e.title.as_str(), e.location.as_str(), e.description.as_str()),
            ("uid-1", "Lunch", "Cafe", "Bring notes")
        );
        assert_eq!((e.start, e.end, e.tz.as_str()), (dt("2026-11-05 12:00"), dt("2026-11-05 13:30"), "Europe/Berlin"));
        assert!(!e.all_day);
        assert_eq!((e.status, e.sequence, e.organizer.as_deref()), (Status::Tentative, 3, Some("boss@example.com")));
        assert_eq!(e.updated, utc("2026-10-30T09:15:02Z"));
        assert_eq!(e.reminders, vec![5, 30]);

        let (items, info) = listing();
        let dentist = event_from_json(&items[3], &info, None);
        assert_eq!((dentist.start, dentist.tz.as_str()), (dt("2026-11-05 10:00"), "America/New_York"));
        assert!(dentist.reminders.is_empty());
    }

    #[test]
    fn all_day_event_spans_days_with_exclusive_end() {
        let v = json(r#"{"id": "e2", "iCalUID": "u2", "summary": "Trip", "start": {"date": "2026-11-02"}, "end": {"date": "2026-11-05"}}"#);
        let e = event_from_json(&v, &ny(), None);
        assert!(e.all_day);
        assert_eq!((e.start, e.end), (dt("2026-11-02 00:00"), dt("2026-11-05 00:00")));
        let (start, end) = times(&e);
        assert_eq!((start["date"].as_str(), end["date"].as_str()), (Some("2026-11-02"), Some("2026-11-05")));
    }

    #[test]
    fn series_and_exceptions_are_one_object_and_expand_across_dst() {
        let (items, info) = listing();
        let g = group(&items, &info);
        assert_eq!(g.deleted, vec!["gone1".to_string()]);
        assert_eq!(g.objects.len(), 2);
        let series = &g.objects[0];
        assert_eq!(series.href, "abc");
        assert_eq!(series.etag, r#""3001","3002","3003""#);
        assert_eq!(series.events.len(), 3);
        assert!(series.events.iter().all(|e| e.uid == "abc@google.com"));
        let m = &series.events[0];
        assert_eq!(m.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=TU"));
        assert_eq!(m.exdates, vec![dt("2026-10-27 09:00")]);
        assert_eq!(m.reminders, vec![10]);
        let moved = &series.events[1];
        assert_eq!(
            (moved.recurrence_id, moved.start, moved.title.as_str()),
            (Some(dt("2026-11-03 09:00")), dt("2026-11-03 11:00"), "Standup (late)")
        );
        let gone = &series.events[2];
        assert_eq!((gone.recurrence_id, gone.status), (Some(dt("2026-11-10 09:00")), Status::Cancelled));
        let raw = json(&series.raw);
        assert_eq!(raw["master"]["id"], "abc");
        assert_eq!(raw["exceptions"].as_array().unwrap().len(), 2);
        assert_eq!(g.objects[1].href, "single1");
        assert_eq!(g.objects[1].etag, "\"4001\"");

        let zone: Tz = "America/New_York".parse().unwrap();
        let shown = recur::expand(&series.events, utc("2026-10-19T00:00:00Z"), utc("2026-11-25T00:00:00Z"), zone);
        let starts: Vec<String> = shown.iter().map(|o| o.start.to_rfc3339()).collect();
        assert_eq!(
            starts,
            ["2026-10-20T13:00:00+00:00", "2026-11-03T16:00:00+00:00", "2026-11-17T14:00:00+00:00", "2026-11-24T14:00:00+00:00"]
        );
    }

    #[test]
    fn instances_without_a_visible_series_stay_together() {
        let items = [
            json(
                r#"{"id": "s_20261103T140000Z", "iCalUID": "s@x", "recurringEventId": "s", "status": "confirmed",
                    "originalStartTime": {"dateTime": "2026-11-03T14:00:00Z"},
                    "start": {"dateTime": "2026-11-03T14:00:00Z", "timeZone": "UTC"}, "end": {"dateTime": "2026-11-03T15:00:00Z", "timeZone": "UTC"}}"#,
            ),
            json(r#"{"id": "t_20261103", "recurringEventId": "t", "status": "cancelled", "originalStartTime": {"date": "2026-11-03"}}"#),
        ];
        let g = group(&items, &ny());
        assert_eq!(g.objects.len(), 1);
        assert_eq!((g.objects[0].href.as_str(), g.objects[0].etag.as_str()), ("s", ","));
        assert_eq!(g.objects[0].events[0].recurrence_id, Some(dt("2026-11-03 14:00")));
        assert_eq!(g.deleted, vec!["t".to_string()]);
    }

    #[test]
    fn delta_reads_touched_series_again() {
        let (items, info) = listing();
        let d = delta(&items, &info);
        assert_eq!(d.series, vec![("abc".to_string(), Some("abc@google.com".to_string()))]);
        assert_eq!(d.changes.len(), 2);
        assert!(matches!(&d.changes[0], Change::Upsert(o) if o.href == "single1"));
        assert_eq!(d.changes[1], Change::Deleted("gone1".into()));
        // Only a cancelled instance changed, without its UID; the series was deleted too.
        let only = [json(r#"{"id": "abc_20261110T140000Z", "status": "cancelled", "recurringEventId": "abc"}"#)];
        assert_eq!(delta(&only, &info).series, vec![("abc".to_string(), None)]);
        let both = [only[0].clone(), json(r#"{"id": "abc", "status": "cancelled"}"#)];
        let d = delta(&both, &info);
        assert!(d.series.is_empty());
        assert_eq!(d.changes, vec![Change::Deleted("abc".into())]);
    }

    #[test]
    fn attendees_self_and_video() {
        let v = json(
            r#"{"id": "e3", "iCalUID": "u3", "start": {"dateTime": "2026-11-04T10:00:00Z"}, "end": {"dateTime": "2026-11-04T11:00:00Z"},
                "hangoutLink": "https://meet.google.com/old-link",
                "conferenceData": {"entryPoints": [
                    {"entryPointType": "phone", "uri": "tel:+1-555-0100"},
                    {"entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij"}]},
                "attendees": [
                    {"email": "boss@example.com", "displayName": "Boss", "organizer": true, "responseStatus": "accepted"},
                    {"email": "me@example.com", "self": true, "responseStatus": "needsAction"},
                    {"email": "pal@example.com", "responseStatus": "tentative"},
                    {"email": "c_room@resource.calendar.google.com", "resource": true, "responseStatus": "accepted"}]}"#,
        );
        let e = event_from_json(&v, &ny(), None);
        assert_eq!(e.video.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        let people: Vec<(&str, &str, Partstat, bool)> =
            e.attendees.iter().map(|a| (a.email.as_str(), a.name.as_str(), a.status, a.me)).collect();
        assert_eq!(
            people,
            [
                ("boss@example.com", "Boss", Partstat::Accepted, false),
                ("me@example.com", "", Partstat::NeedsAction, true),
                ("pal@example.com", "", Partstat::Tentative, false)
            ]
        );
        assert_eq!(e.my_status(), Some(Partstat::NeedsAction));
        let only_meet = json(r#"{"id": "e4", "hangoutLink": "https://meet.google.com/xyz"}"#);
        assert_eq!(event_from_json(&only_meet, &ny(), None).video.as_deref(), Some("https://meet.google.com/xyz"));
    }

    const RAW_EVENT: &str = r#"{
     "id": "e5", "etag": "\"77\"", "iCalUID": "u5", "status": "confirmed", "summary": "Review",
     "colorId": "5", "extendedProperties": {"private": {"x": "1"}}, "guestsCanModify": true,
     "start": {"dateTime": "2026-11-06T10:00:00-05:00", "timeZone": "America/New_York"},
     "end": {"dateTime": "2026-11-06T11:00:00-05:00", "timeZone": "America/New_York"},
     "attendees": [
       {"email": "boss@example.com", "organizer": true, "responseStatus": "accepted", "comment": "see you"},
       {"email": "me@example.com", "self": true, "optional": true, "responseStatus": "needsAction"},
       {"email": "room@resource.calendar.google.com", "resource": true, "responseStatus": "accepted"}],
     "reminders": {"useDefault": false, "overrides": [{"method": "email", "minutes": 60}]},
     "conferenceData": {"entryPoints": [{"entryPointType": "video", "uri": "https://meet.google.com/aaa"}]}
    }"#;

    #[test]
    fn writing_keeps_unknown_fields_from_raw() {
        let raw = json(RAW_EVENT);
        let mut e = event_from_json(&raw, &ny(), None);
        e.title = "Design review".into();
        e.start = dt("2026-11-06 14:00");
        e.end = dt("2026-11-06 15:00");
        e.reminders = vec![60, 15];
        let out = merged(&raw, &e, &ny(), true);
        assert_eq!(out["summary"], "Design review");
        assert_eq!(out["colorId"], "5");
        assert_eq!(out["extendedProperties"]["private"]["x"], "1");
        assert_eq!(out["guestsCanModify"], true);
        assert_eq!(out["conferenceData"], raw["conferenceData"]);
        assert_eq!(out["start"], json(r#"{"dateTime": "2026-11-06T14:00:00-05:00", "timeZone": "America/New_York"}"#));
        assert!(out.get("recurrence").is_none());
        assert_eq!(out["attendees"][0]["comment"], "see you");
        assert_eq!(out["attendees"][1]["optional"], true);
        assert_eq!(out["attendees"][2]["resource"], true);
        assert_eq!(
            out["reminders"],
            json(r#"{"useDefault": false, "overrides": [{"method": "popup", "minutes": 15}, {"method": "email", "minutes": 60}]}"#)
        );
        // Reading it back gives the same event.
        let back = event_from_json(&out, &ny(), None);
        assert_eq!(
            (back.title, back.start, back.end, back.attendees, back.reminders),
            (e.title, e.start, e.end, e.attendees, vec![15, 60])
        );
    }

    #[test]
    fn answer_alone_is_an_attendees_patch() {
        let raw = json(RAW_EVENT);
        let base = event_from_json(&raw, &ny(), None);
        assert!(patch_body(&base, Some(&base), &raw, &ny(), true).is_empty());
        let mut e = base.clone();
        e.attendees.iter_mut().find(|a| a.me).unwrap().status = Partstat::Accepted;
        let body = patch_body(&e, Some(&base), &raw, &ny(), true);
        assert_eq!(body.keys().collect::<Vec<_>>(), ["attendees"]);
        let me = &body["attendees"][1];
        assert_eq!(
            (me["email"].as_str(), me["responseStatus"].as_str(), me["optional"].as_bool()),
            (Some("me@example.com"), Some("accepted"), Some(true))
        );
        assert_eq!(body["attendees"].as_array().unwrap().len(), 3);
        // A new time sends start and end together.
        let mut moved = base.clone();
        moved.end = dt("2026-11-06 12:00");
        let body = patch_body(&moved, Some(&base), &raw, &ny(), true);
        assert_eq!(body.keys().collect::<Vec<_>>(), ["end", "start"]);
    }

    #[test]
    fn default_reminders_stay_default() {
        let raw = json(r#"{"reminders": {"useDefault": true}}"#);
        let mut e = Event { reminders: vec![10], ..Event::default() };
        assert_eq!(reminders(&e, &raw, &ny()), json(r#"{"useDefault": true, "overrides": null}"#));
        e.reminders = vec![];
        assert_eq!(reminders(&e, &raw, &ny()), json(r#"{"useDefault": false, "overrides": []}"#));
    }

    #[test]
    fn recurrence_lines_read_and_write() {
        let zone: Tz = "America/New_York".parse().unwrap();
        let mut e = Event { tz: "America/New_York".into(), ..Event::default() };
        for line in [
            "RRULE:FREQ=DAILY;COUNT=10",
            "EXDATE:20261103T140000Z",
            "EXDATE;TZID=Europe/Berlin:20261104T150000,20261105T150000",
            "RDATE:20261120T090000",
            "EXRULE:FREQ=WEEKLY;BYDAY=SA",
        ] {
            read_recurrence(line, zone, false, &mut e);
        }
        assert_eq!(e.rrule.as_deref(), Some("FREQ=DAILY;COUNT=10"));
        assert_eq!(e.exdates, vec![dt("2026-11-03 09:00"), dt("2026-11-04 09:00"), dt("2026-11-05 09:00")]);
        assert_eq!(e.rdates, vec![dt("2026-11-20 09:00")]);
        let raw = json(r#"{"recurrence": ["RRULE:FREQ=DAILY", "EXRULE:FREQ=WEEKLY;BYDAY=SA"]}"#);
        assert_eq!(
            recurrence_lines(&e, &raw),
            [
                "RRULE:FREQ=DAILY;COUNT=10",
                "EXRULE:FREQ=WEEKLY;BYDAY=SA",
                "RDATE;TZID=America/New_York:20261120T090000",
                "EXDATE;TZID=America/New_York:20261103T090000,20261104T090000,20261105T090000"
            ]
        );
        let mut day = Event { all_day: true, rrule: Some("FREQ=WEEKLY".into()), ..Event::default() };
        read_recurrence("EXDATE;VALUE=DATE:20261110,20261117", zone, true, &mut day);
        assert_eq!(day.exdates, vec![dt("2026-11-10 00:00"), dt("2026-11-17 00:00")]);
        assert_eq!(recurrence_lines(&day, &Value::Null), ["RRULE:FREQ=WEEKLY", "EXDATE;VALUE=DATE:20261110,20261117"]);
    }

    #[test]
    fn instance_ids_use_utc_or_the_date() {
        let zone: Tz = "America/New_York".parse().unwrap();
        assert_eq!(instance_id("abc", dt("2026-11-03 09:00"), false, zone), "abc_20261103T140000Z");
        assert_eq!(instance_id("abc", dt("2026-10-27 09:00"), false, zone), "abc_20261027T130000Z");
        assert_eq!(instance_id("abc", dt("2026-11-03 00:00"), true, zone), "abc_20261103");
    }

    #[test]
    fn instances_left_in_the_series_are_known() {
        let (items, info) = listing();
        let m = event_from_json(&items[0], &info, None);
        assert!(is_instance(&m, dt("2026-11-17 09:00")));
        assert!(!is_instance(&m, dt("2026-10-27 09:00")));
        assert!(!is_instance(&m, dt("2026-11-17 10:00")));
        let unchanged = instance_of(&m, dt("2026-11-17 09:00"));
        assert_eq!((unchanged.start, unchanged.end, unchanged.rrule), (dt("2026-11-17 09:00"), dt("2026-11-17 09:30"), None));
    }

    #[test]
    fn guests_are_emailed_only_when_asked() {
        assert_eq!(send_updates(true), "all");
        assert_eq!(send_updates(false), "none");
    }

    #[test]
    fn calendar_list_skips_free_busy_and_marks_read_only() {
        let page = r##"{"items": [
            {"id": "me@example.com", "summary": "me@example.com", "summaryOverride": "Personal", "accessRole": "owner",
             "backgroundColor": "#9FE1E7", "timeZone": "America/New_York", "defaultReminders": [{"method": "popup", "minutes": 30}]},
            {"id": "en.usa#holiday@group.v.calendar.google.com", "summary": "Holidays", "accessRole": "reader", "timeZone": "UTC"},
            {"id": "pal@example.com", "summary": "Pal", "accessRole": "freeBusyReader"}],
            "nextPageToken": "p2"}"##;
        let (mut cals, mut infos) = (Vec::new(), HashMap::new());
        assert_eq!(calendar_page(page, &mut cals, &mut infos).unwrap(), Some("p2".into()));
        assert_eq!(
            cals,
            [
                RemoteCalendar {
                    remote_id: "me@example.com".into(),
                    name: "Personal".into(),
                    color: Some("#9fe1e7".into()),
                    read_only: false
                },
                RemoteCalendar {
                    remote_id: "en.usa#holiday@group.v.calendar.google.com".into(),
                    name: "Holidays".into(),
                    color: None,
                    read_only: true
                }
            ]
        );
        assert_eq!(infos["me@example.com"], CalInfo { zone: "America/New_York".into(), defaults: vec![30] });
        assert_eq!(url_encode("en.usa#holiday@group.v.calendar.google.com"), "en.usa%23holiday%40group.v.calendar.google.com");
    }

    #[test]
    fn errors_map_to_sync_errors() {
        let guest = r#"{"error": {"code": 403, "message": "Forbidden", "errors": [{"reason": "forbiddenForNonOrganizer"}]}}"#;
        assert_eq!(status_error(403, guest), Error::Server(403, "Forbidden".into()));
        let scope = r#"{"error": {"code": 403, "message": "Insufficient Permission", "errors": [{"reason": "insufficientPermissions"}]}}"#;
        assert_eq!(status_error(403, scope), Error::Auth("Insufficient Permission".into()));
        assert_eq!(status_error(412, ""), Error::Conflict);
        assert_eq!(status_error(410, ""), Error::NotFound);
    }
}
