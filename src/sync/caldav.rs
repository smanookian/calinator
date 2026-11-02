//! CalDAV events (RFC 4791): account discovery (RFC 6764), delta sync with
//! sync-collection (RFC 6578) and object writes guarded by ETags. Objects are
//! iCalendar text, read and written by [`crate::ical`].
//!
//! Calendars and objects are named by absolute URLs.

use std::time::Duration;

use roxmltree::{Document, Node};
use ureq::http;

use super::{Change, Error, Provider, Pull, RemoteCalendar, RemoteObject, Result};
use crate::model::Event;
use crate::{ical, tz};

const DAV: &str = "DAV:";
const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
const APPLE: &str = "http://apple.com/ns/ical/";
const XML: &str = "application/xml; charset=utf-8";
/// Hrefs per calendar-multiget request.
const BATCH: usize = 100;
/// Largest reply we read.
const MAX_BODY: u64 = 256 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
/// Pages of a cut-short sync-collection reply we follow.
const MAX_PAGES: usize = 100;

const FIND_PRINCIPAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;

const FIND_HOME: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/><c:calendar-user-address-set/></d:prop></d:propfind>"#;

const FIND_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:prop><d:resourcetype/><d:displayname/><c:supported-calendar-component-set/><d:current-user-privilege-set/><a:calendar-color/></d:prop></d:propfind>"#;

const FIND_SYNC_TOKEN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:sync-token/></d:prop></d:propfind>"#;

const LIST_OBJECTS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getetag/></d:prop></d:propfind>"#;

const FIND_ETAG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>"#;

/// One CalDAV account (Basic auth).
pub struct CalDav {
    agent: ureq::Agent,
    /// The address the user gave.
    base_url: String,
    username: String,
    password: String,
    /// Found on first use.
    account: Option<Account>,
}

/// What discovery found.
struct Account {
    /// Collections that hold the user's calendars.
    homes: Vec<String>,
    /// The user's email addresses.
    addresses: Vec<String>,
}

/// An HTTP reply, read in full.
struct Reply {
    status: u16,
    /// Where the reply came from, after redirects.
    url: String,
    etag: Option<String>,
    body: String,
}

impl Reply {
    fn ok(self) -> Result<Self> {
        if (200..300).contains(&self.status) { Ok(self) } else { Err(status_error(self.status)) }
    }
}

impl CalDav {
    /// `url`: the server, the user's principal, calendar home or one
    /// calendar (iCloud: `https://caldav.icloud.com/`). A missing scheme
    /// means https. Nothing is sent until the first call.
    pub fn new(url: &str, username: &str, password: &str) -> Result<Self> {
        let url = url.trim();
        let base_url = if url.contains("://") { url.to_string() } else { format!("https://{url}") };
        let web = base_url.starts_with("https://") || base_url.starts_with("http://");
        if !web || url.is_empty() || base_url.parse::<http::Uri>().is_err() {
            return Err(Error::Server(0, format!("not a web address: {url}")));
        }
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .allow_non_standard_methods(true)
            .timeout_connect(Some(TIMEOUT))
            .timeout_per_read(Some(TIMEOUT))
            .timeout_per_write(Some(TIMEOUT))
            .build()
            .new_agent();
        Ok(CalDav { agent, base_url, username: username.to_string(), password: password.to_string(), account: None })
    }

    /// The account, found on first use: the principal at the URL or at
    /// `/.well-known/caldav`, then its calendar homes and addresses. Without
    /// a principal the URL itself is listed (it may be a home or a calendar).
    fn account(&mut self) -> Result<&Account> {
        let account = match self.account.take() {
            Some(a) => a,
            None => {
                let start = self.base_url.clone();
                let principal = match self.principal(&start)? {
                    Some(p) => Some(p),
                    None => self.principal(&format!("{}/.well-known/caldav", origin(&start)))?,
                };
                let (mut homes, addresses) = match &principal {
                    Some(p) => self.homes(p)?,
                    None => (Vec::new(), Vec::new()),
                };
                if homes.is_empty() {
                    homes.push(principal.unwrap_or(start));
                }
                Account { homes, addresses }
            }
        };
        Ok(self.account.insert(account))
    }

    fn auth(&self) -> String {
        format!("Basic {}", base64(format!("{}:{}", self.username, self.password).as_bytes()))
    }

    /// One request, no redirects. A request on a kept-alive connection the
    /// server closed meanwhile is sent again: every method we use is
    /// idempotent, and some servers (Radicale) answer HTTP/1.0 and close
    /// connections ureq would reuse.
    fn run(&self, method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<http::Response<ureq::Body>> {
        let mut tries = 0;
        loop {
            let mut req = http::Request::builder().method(method).uri(url).header("Authorization", self.auth());
            for (name, value) in headers {
                req = req.header(*name, *value);
            }
            let res = match body {
                Some(b) => req.body(b).map(|r| self.agent.run(r)),
                None => req.body(()).map(|r| self.agent.run(r)),
            };
            match res.map_err(|e| Error::Server(0, format!("bad address {url}: {e}")))? {
                Err(e) if tries < 2 && dropped(&e) => tries += 1,
                res => return res.map_err(super::http_error),
            }
        }
    }

    /// Sends a request, following redirects with the same method and body.
    /// Credentials only go to the same origin or over https.
    fn send(&self, method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<Reply> {
        let mut url = url.to_string();
        for _ in 0..5 {
            let mut res = self.run(method, &url, headers, body)?;
            let status = res.status().as_u16();
            let header = |name: &str| res.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            if (300..400).contains(&status)
                && status != 304
                && let Some(location) = header("location")
            {
                let next = resolve(&url, &location);
                if next.starts_with("https://") || origin(&next) == origin(&url) {
                    url = next;
                    continue;
                }
            }
            let etag = header("etag");
            let body = res.body_mut().with_config().limit(MAX_BODY).read_to_string().map_err(super::http_error)?;
            return Ok(Reply { status, url, etag, body });
        }
        Err(Error::Server(310, format!("too many redirects at {url}")))
    }

    fn propfind(&self, url: &str, depth: &str, body: &str) -> Result<Reply> {
        self.send("PROPFIND", url, &[("Depth", depth), ("Content-Type", XML)], Some(body))
    }

    /// The principal URL named at `url`, if the server says.
    fn principal(&self, url: &str) -> Result<Option<String>> {
        let reply = self.propfind(url, "0", FIND_PRINCIPAL)?;
        match reply.status {
            207 => Ok(parse_principal(&reply.body).ok().flatten().map(|h| resolve(&reply.url, &h))),
            401 => Err(status_error(401)),
            _ => Ok(None),
        }
    }

    /// Calendar homes and email addresses of a principal.
    fn homes(&self, principal: &str) -> Result<(Vec<String>, Vec<String>)> {
        let reply = self.propfind(principal, "0", FIND_HOME)?;
        match reply.status {
            207 => {
                let (homes, addresses) = parse_principal_props(&reply.body)?;
                Ok((homes.iter().map(|h| resolve(&reply.url, h)).collect(), addresses))
            }
            401 => Err(status_error(401)),
            _ => Ok((Vec::new(), Vec::new())),
        }
    }

    /// The collection's current sync token; `None` when it has none.
    fn sync_token(&self, url: &str) -> Result<Option<String>> {
        let reply = self.propfind(url, "0", FIND_SYNC_TOKEN)?;
        match reply.status {
            207 => parse_sync_token(&reply.body),
            401 | 404 | 410 => Err(status_error(reply.status)),
            _ => Ok(None),
        }
    }

    /// Every object in the calendar.
    fn pull_all(&self, url: &str, me: &[String]) -> Result<Pull> {
        // Token first: a change made during the listing comes again next time.
        let token = self.sync_token(url)?;
        let reply = self.propfind(url, "1", LIST_OBJECTS)?.ok()?;
        let listed =
            parse_objects(&reply.body)?.into_iter().filter(|o| o.status == 200 && !o.collection && !o.href.ends_with('/')).collect();
        let objects = self.fetch(url, &reply.url, listed, me)?;
        Ok(Pull { changes: objects.into_iter().map(Change::Upsert).collect(), token, full: true })
    }

    /// Changes since `token`, following cut-short replies (507 on the collection).
    fn pull_since(&self, url: &str, token: &str, me: &[String]) -> Result<Pull> {
        let mut token = token.to_string();
        let mut seen: Vec<(String, Object)> = Vec::new();
        let mut base = url.to_string();
        let mut next = None;
        for _ in 0..MAX_PAGES {
            let reply = self.send("REPORT", url, &[("Depth", "0"), ("Content-Type", XML)], Some(&sync_collection(&token)))?;
            match reply.status {
                207 => {}
                // An old or unknown token: 403/409 with a valid-sync-token
                // error (some servers answer 400 with it).
                403 | 409 => return Err(Error::TokenExpired),
                _ if reply.body.contains("valid-sync-token") => return Err(Error::TokenExpired),
                // No sync-collection support after all.
                405 | 501 => return self.pull_all(url, me),
                s => return Err(status_error(s)),
            }
            next = parse_sync_token(&reply.body)?;
            let mut more = false;
            for o in parse_objects(&reply.body)? {
                let abs = resolve(&reply.url, &o.href);
                if abs.ends_with('/') {
                    more |= o.status == 507;
                    continue;
                }
                seen.retain(|(h, _)| *h != abs);
                seen.push((abs, o));
            }
            base = reply.url;
            match &next {
                Some(n) if more && *n != token => token = n.clone(),
                _ => break,
            }
        }
        let (gone, changed): (Vec<_>, Vec<_>) = seen.into_iter().partition(|(_, o)| matches!(o.status, 404 | 410));
        let changed = changed.into_iter().filter(|(_, o)| o.status == 200).map(|(_, o)| o).collect();
        let mut changes: Vec<Change> = self.fetch(url, &base, changed, me)?.into_iter().map(Change::Upsert).collect();
        changes.extend(gone.into_iter().map(|(href, _)| Change::Deleted(href)));
        Ok(Pull { changes, token: next, full: false })
    }

    /// Objects from multistatus entries (`base`: the URL their hrefs are
    /// relative to). Entries sent without data are fetched with
    /// calendar-multiget; objects with no VEVENT (tasks) are skipped.
    fn fetch(&self, calendar: &str, base: &str, objects: Vec<Object>, me: &[String]) -> Result<Vec<RemoteObject>> {
        let mut found: Vec<(String, String, String)> = Vec::new();
        let mut missing: Vec<Object> = Vec::new();
        for o in objects {
            match o.data {
                Some(data) => found.push((resolve(base, &o.href), o.etag, data)),
                None => missing.push(o),
            }
        }
        for chunk in missing.chunks(BATCH) {
            let hrefs: Vec<&str> = chunk.iter().map(|o| o.href.as_str()).collect();
            let reply = self.send("REPORT", calendar, &[("Depth", "1"), ("Content-Type", XML)], Some(&multiget(&hrefs)))?.ok()?;
            for o in parse_objects(&reply.body)? {
                if let (200, Some(data)) = (o.status, o.data) {
                    let href = resolve(&reply.url, &o.href);
                    let etag = if o.etag.is_empty() {
                        chunk.iter().find(|m| resolve(base, &m.href) == href).map(|m| m.etag.clone()).unwrap_or_default()
                    } else {
                        o.etag
                    };
                    found.push((href, etag, data));
                }
            }
        }
        let user = tz::system();
        Ok(found
            .into_iter()
            .filter_map(|(href, etag, data)| {
                let events = read_events(&data, me, user);
                (!events.is_empty()).then_some(RemoteObject { href, etag, events, raw: data })
            })
            .collect())
    }

    /// The ETag and text of an object, read back from the server.
    fn get(&self, url: &str) -> Result<(String, String)> {
        let got = self.send("GET", url, &[], None)?.ok()?;
        let etag = match got.etag {
            Some(e) => e,
            None => {
                let reply = self.propfind(url, "0", FIND_ETAG)?.ok()?;
                parse_objects(&reply.body)?.into_iter().map(|o| o.etag).find(|e| !e.is_empty()).unwrap_or_default()
            }
        };
        Ok((etag, got.body))
    }
}

impl Provider for CalDav {
    fn my_addresses(&mut self) -> Result<Vec<String>> {
        Ok(self.account()?.addresses.clone())
    }

    fn calendars(&mut self) -> Result<Vec<RemoteCalendar>> {
        let homes = self.account()?.homes.clone();
        let mut out: Vec<RemoteCalendar> = Vec::new();
        for home in &homes {
            let reply = self.propfind(home, "1", FIND_CALENDARS)?.ok()?;
            for cal in parse_calendars(&reply.body)? {
                let url = resolve(&reply.url, &cal.href);
                if out.iter().any(|c| c.remote_id == url) {
                    continue;
                }
                let name = cal.name.unwrap_or_else(|| last_segment(&url));
                out.push(RemoteCalendar { remote_id: url, name, color: cal.color, read_only: cal.read_only });
            }
        }
        Ok(out)
    }

    fn pull(&mut self, calendar: &str, token: Option<&str>) -> Result<Pull> {
        let me = self.my_addresses()?;
        match token {
            Some(token) => self.pull_since(calendar, token, &me),
            None => self.pull_all(calendar, &me),
        }
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
        let uid = events.first().map(|e| e.uid.as_str()).filter(|u| !u.is_empty());
        let Some(uid) = uid else { return Err(Error::Server(0, "an event without a UID cannot be saved".into())) };
        let me = self.my_addresses()?;
        let user = tz::system();
        let text = ical::write_merged(events, raw, user, notify);
        let (url, condition) = match href {
            None => (format!("{}/{}.ics", calendar.trim_end_matches('/'), file_name(uid)), Some(("If-None-Match", "*"))),
            Some(h) => (h.to_string(), (!etag.is_empty()).then_some(("If-Match", etag))),
        };
        let mut headers = vec![("Content-Type", "text/calendar; charset=utf-8")];
        headers.extend(condition);
        let reply = self.send("PUT", &url, &headers, Some(&text))?.ok()?;
        // A weak or missing ETag means the server changed what we sent: read it back.
        let (etag, stored) = match reply.etag {
            Some(e) if !e.starts_with("W/") => (e, text),
            _ => self.get(&reply.url)?,
        };
        Ok(RemoteObject { href: reply.url, etag, events: read_events(&stored, &me, user), raw: stored })
    }

    fn delete(&mut self, _calendar: &str, href: &str, etag: &str, _notify: bool) -> Result<()> {
        let condition: Vec<(&str, &str)> = if etag.is_empty() { Vec::new() } else { vec![("If-Match", etag)] };
        let reply = self.send("DELETE", href, &condition, None)?;
        match reply.status {
            200..=299 | 404 | 410 => Ok(()),
            s => Err(status_error(s)),
        }
    }
}

/// The events of an object, with empty ids and calendar for the engine.
fn read_events(text: &str, me: &[String], user: chrono_tz::Tz) -> Vec<Event> {
    let mut events = ical::events(text, "", me, user);
    for e in &mut events {
        e.id.clear();
    }
    events
}

/// Error for a failed HTTP status. 403 is "not allowed" (a read-only
/// calendar), not a lost login.
fn status_error(status: u16) -> Error {
    match status {
        403 => Error::Server(403, "not allowed".into()),
        _ => super::http_error(ureq::Error::StatusCode(status)),
    }
}

/// The connection was closed under us (a stale kept-alive connection).
fn dropped(e: &ureq::Error) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, UnexpectedEof};
    matches!(e, ureq::Error::Io(io) if matches!(io.kind(), UnexpectedEof | BrokenPipe | ConnectionReset | ConnectionAborted))
}

/// `scheme://host[:port]` of a URL.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    match url[after..].find('/') {
        Some(i) => &url[..after + i],
        None => url,
    }
}

/// `href` (a URL, an absolute path or a relative path) against `base`.
fn resolve(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    let root = origin(base);
    if href.starts_with('/') {
        return format!("{root}{href}");
    }
    let path = base[root.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
    format!("{root}{dir}{href}")
}

fn last_segment(url: &str) -> String {
    url.trim_end_matches('/').rsplit('/').next().unwrap_or(url).to_string()
}

/// A UID as a file name: unreserved characters stay, the rest is %-encoded.
fn file_name(uid: &str) -> String {
    let mut out = String::with_capacity(uid.len());
    for b in uid.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn sync_collection(token: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
        xml_escape(token)
    )
}

fn multiget(hrefs: &[&str]) -> String {
    let hrefs: String = hrefs.iter().map(|h| format!("<d:href>{}</d:href>", xml_escape(h))).collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop>{hrefs}</c:calendar-multiget>"#
    )
}

fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---- multistatus (RFC 4918) ----

/// A calendar collection from a Depth 1 PROPFIND.
#[derive(Debug, PartialEq)]
struct Calendar {
    href: String,
    name: Option<String>,
    color: Option<String>,
    read_only: bool,
}

/// An object in a PROPFIND or REPORT reply.
#[derive(Debug, PartialEq)]
struct Object {
    /// As the server wrote it.
    href: String,
    status: u16,
    etag: String,
    data: Option<String>,
    collection: bool,
}

/// One `<response>`: href, status and the `<prop>` elements that came back 2xx.
struct Entry<'a, 'i> {
    href: String,
    status: u16,
    props: Vec<Node<'a, 'i>>,
}

impl<'a, 'i> Entry<'a, 'i> {
    fn prop(&self, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
        self.props.iter().find_map(|p| child(*p, ns, name))
    }

    /// The `<href>` values inside a property.
    fn hrefs(&self, ns: &str, name: &str) -> Vec<String> {
        self.prop(ns, name)
            .into_iter()
            .flat_map(|set| set.children().filter(|n| is(n, DAV, "href")))
            .map(|h| text(h).trim().to_string())
            .filter(|h| !h.is_empty())
            .collect()
    }
}

fn xml(text: &str) -> Result<Document<'_>> {
    Document::parse(text).map_err(|e| Error::Parse(e.to_string()))
}

fn is(n: &Node, ns: &str, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
}

fn child<'a, 'i>(n: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| is(c, ns, name))
}

/// All text inside an element.
fn text(n: Node) -> String {
    n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect()
}

/// `HTTP/1.1 404 Not Found` -> 404.
fn status_code(n: Node) -> Option<u16> {
    text(n).split_whitespace().nth(1)?.parse().ok()
}

fn entries<'a, 'i>(doc: &'a Document<'i>) -> Vec<Entry<'a, 'i>> {
    doc.root_element()
        .children()
        .filter(|n| is(n, DAV, "response"))
        .filter_map(|r| {
            let href = text(child(r, DAV, "href")?).trim().to_string();
            let mut props = Vec::new();
            let mut failed = None;
            for stat in r.children().filter(|n| is(n, DAV, "propstat")) {
                match child(stat, DAV, "status").and_then(status_code) {
                    Some(200..=299) | None => props.extend(child(stat, DAV, "prop")),
                    code => failed = failed.or(code),
                }
            }
            let status = child(r, DAV, "status").and_then(status_code).or(if props.is_empty() { failed } else { None }).unwrap_or(200);
            Some(Entry { href, status, props })
        })
        .collect()
}

fn parse_principal(body: &str) -> Result<Option<String>> {
    let doc = xml(body)?;
    Ok(entries(&doc).iter().flat_map(|e| e.hrefs(DAV, "current-user-principal")).next())
}

/// Calendar homes and email addresses (`mailto:` removed) of a principal.
fn parse_principal_props(body: &str) -> Result<(Vec<String>, Vec<String>)> {
    let doc = xml(body)?;
    let entries = entries(&doc);
    let homes = entries.iter().flat_map(|e| e.hrefs(CALDAV, "calendar-home-set")).collect();
    let mut addresses: Vec<String> = Vec::new();
    for a in entries.iter().flat_map(|e| e.hrefs(CALDAV, "calendar-user-address-set")) {
        if let Some(mail) = a.get(..7).filter(|s| s.eq_ignore_ascii_case("mailto:")).map(|_| a[7..].to_string())
            && !addresses.iter().any(|m| m.eq_ignore_ascii_case(&mail))
        {
            addresses.push(mail);
        }
    }
    Ok((homes, addresses))
}

/// `#rrggbb` from Apple's `#RRGGBBAA` (or `#RRGGBB`).
fn color(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix('#')?.get(..6)?;
    hex.chars().all(|c| c.is_ascii_hexdigit()).then(|| format!("#{}", hex.to_ascii_lowercase()))
}

/// Calendars that can hold events. A calendar that does not list its
/// components is taken. Read-only when the privilege set lacks write.
fn parse_calendars(body: &str) -> Result<Vec<Calendar>> {
    let doc = xml(body)?;
    let mut out = Vec::new();
    for e in entries(&doc) {
        let is_calendar = e.prop(DAV, "resourcetype").is_some_and(|t| child(t, CALDAV, "calendar").is_some());
        let has_events = e.prop(CALDAV, "supported-calendar-component-set").is_none_or(|set| {
            set.children().any(|c| is(&c, CALDAV, "comp") && c.attribute("name").is_some_and(|n| n.eq_ignore_ascii_case("VEVENT")))
        });
        if !is_calendar || !has_events {
            continue;
        }
        let read_only = e.prop(DAV, "current-user-privilege-set").is_some_and(|set| {
            !set.descendants().any(|p| {
                p.is_element()
                    && p.tag_name().namespace() == Some(DAV)
                    && matches!(p.tag_name().name(), "write" | "write-content" | "bind" | "all")
            })
        });
        let name = e.prop(DAV, "displayname").map(|n| text(n).trim().to_string()).filter(|n| !n.is_empty());
        let color = e.prop(APPLE, "calendar-color").and_then(|c| color(&text(c)));
        out.push(Calendar { href: e.href, name, color, read_only });
    }
    Ok(out)
}

fn parse_objects(body: &str) -> Result<Vec<Object>> {
    let doc = xml(body)?;
    let objects = entries(&doc)
        .into_iter()
        .map(|e| Object {
            etag: e.prop(DAV, "getetag").map(|n| text(n).trim().to_string()).unwrap_or_default(),
            data: e.prop(CALDAV, "calendar-data").map(text).filter(|d| !d.trim().is_empty()),
            collection: e.prop(DAV, "resourcetype").is_some_and(|t| child(t, DAV, "collection").is_some()),
            status: e.status,
            href: e.href,
        })
        .collect();
    Ok(objects)
}

/// The sync token of a PROPFIND or sync-collection reply.
fn parse_sync_token(body: &str) -> Result<Option<String>> {
    let doc = xml(body)?;
    Ok(doc.descendants().filter(|n| is(n, DAV, "sync-token")).map(|n| text(n).trim().to_string()).find(|t| !t.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRINCIPAL: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:"><response><href>/</href><propstat><prop><current-user-principal><href>/test/</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;

    const ICLOUD_PRINCIPAL_PROPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<multistatus xmlns="DAV:"><response><href>/123/principal/</href><propstat><prop>
<calendar-home-set xmlns="urn:ietf:params:xml:ns:caldav"><href xmlns="DAV:">https://p42-caldav.icloud.com:443/123/calendars/</href></calendar-home-set>
<calendar-user-address-set xmlns="urn:ietf:params:xml:ns:caldav"><href xmlns="DAV:" preferred="1">mailto:anna@icloud.com</href><href xmlns="DAV:">MAILTO:anna@me.com</href><href xmlns="DAV:">/123/principal/</href><href xmlns="DAV:">urn:uuid:123</href><href xmlns="DAV:">mailto:Anna@icloud.com</href></calendar-user-address-set>
</prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;

    const CALENDARS: &str = r##"<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:ICAL="http://apple.com/ns/ical/"><response><href>/test/</href><propstat><prop><resourcetype><principal /><collection /></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>
<response><href>/test/work/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><displayname>Work</displayname><ICAL:calendar-color>#FF8800FF</ICAL:calendar-color><C:supported-calendar-component-set><C:comp name="VEVENT" /><C:comp name="VTODO" /></C:supported-calendar-component-set><current-user-privilege-set><privilege><read /></privilege><privilege><all /></privilege></current-user-privilege-set></prop><status>HTTP/1.1 200 OK</status></propstat></response>
<response><href>/test/tasks/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><displayname>Tasks</displayname><C:supported-calendar-component-set><C:comp name="VTODO" /></C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat></response>
<response><href>/test/shared/</href><propstat><prop><resourcetype><C:calendar /><collection /></resourcetype><current-user-privilege-set><privilege><read /></privilege></current-user-privilege-set></prop><status>HTTP/1.1 200 OK</status></propstat><propstat><prop><displayname /><ICAL:calendar-color /><C:supported-calendar-component-set /></prop><status>HTTP/1.1 404 Not Found</status></propstat></response>
<response><href>/test/inbox/</href><propstat><prop><resourcetype><C:schedule-inbox /><collection /></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"##;

    const SYNC: &str = r#"<?xml version='1.0' encoding='utf-8'?>
<multistatus xmlns="DAV:"><sync-token>http://radicale.org/ns/sync/d13d9a69</sync-token><response><href>/test/work/a.ics</href><status>HTTP/1.1 404 Not Found</status></response><response><href>/test/work/b.ics</href><propstat><prop><getetag>"d9cca11f"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;

    #[test]
    fn reads_principal_homes_and_addresses() {
        assert_eq!(parse_principal(PRINCIPAL).unwrap().as_deref(), Some("/test/"));
        let (homes, addresses) = parse_principal_props(ICLOUD_PRINCIPAL_PROPS).unwrap();
        assert_eq!(homes, vec!["https://p42-caldav.icloud.com:443/123/calendars/"]);
        assert_eq!(addresses, vec!["anna@icloud.com", "anna@me.com"]);
        assert!(matches!(parse_principal("<html>"), Err(Error::Parse(_))));
    }

    #[test]
    fn finds_event_calendars() {
        let cals = parse_calendars(CALENDARS).unwrap();
        assert_eq!(
            cals,
            vec![
                Calendar { href: "/test/work/".into(), name: Some("Work".into()), color: Some("#ff8800".into()), read_only: false },
                Calendar { href: "/test/shared/".into(), name: None, color: None, read_only: true },
            ]
        );
        assert_eq!(last_segment("http://h/test/shared/"), "shared");
        assert_eq!(color("#abcdef"), Some("#abcdef".into()));
        assert_eq!(color("red"), None);
    }

    #[test]
    fn reads_sync_report() {
        assert_eq!(parse_sync_token(SYNC).unwrap().as_deref(), Some("http://radicale.org/ns/sync/d13d9a69"));
        let objects = parse_objects(SYNC).unwrap();
        assert_eq!((objects[0].status, objects[0].href.as_str()), (404, "/test/work/a.ics"));
        assert_eq!((objects[1].status, objects[1].etag.as_str()), (200, "\"d9cca11f\""));
    }

    #[test]
    fn urls_and_names() {
        assert_eq!(origin("https://h.example:8443/a/b"), "https://h.example:8443");
        assert_eq!(resolve("https://h/a/b/", "/x/y.ics"), "https://h/x/y.ics");
        assert_eq!(resolve("https://h/a/b/", "y.ics"), "https://h/a/b/y.ics");
        assert_eq!(resolve("https://h/a/", "https://other/z/"), "https://other/z/");
        assert_eq!(file_name("abc-1_2.x~"), "abc-1_2.x~");
        assert_eq!(file_name("a@b c/d"), "a%40b%20c%2Fd");
        assert_eq!(CalDav::new("cloud.example.com/dav", "u", "p").unwrap().base_url, "https://cloud.example.com/dav");
        assert!(CalDav::new("  ", "u", "p").is_err());
        assert!(CalDav::new("ftp://x", "u", "p").is_err());
    }

    #[test]
    fn errors_and_bodies() {
        assert!(matches!(status_error(401), Error::Auth(_)));
        assert!(matches!(status_error(403), Error::Server(403, _)));
        assert_eq!(status_error(412), Error::Conflict);
        assert_eq!(base64(b"tester:secret123"), "dGVzdGVyOnNlY3JldDEyMw==");
        let body = sync_collection("http://x/sync?a=1&b=2");
        assert!(body.contains("<d:sync-token>http://x/sync?a=1&amp;b=2</d:sync-token>"));
        assert!(Document::parse(&body).is_ok());
        let body = multiget(&["/c/a.ics", "/c/b.ics"]);
        assert!(body.contains("<d:href>/c/a.ics</d:href><d:href>/c/b.ics</d:href>"));
        assert!(Document::parse(&body).is_ok());
    }
}
