//! CalDAV against a live Radicale at 127.0.0.1:5233 (user `test`/`test`).
//! Each test makes its own calendar. Without the server the tests pass
//! without doing anything.

use std::net::TcpStream;
use std::time::Duration;

use calinator::model::{Event, Occurrence, new_id};
use calinator::recur::expand;
use calinator::sync::caldav::CalDav;
use calinator::sync::{Change, Error, Provider, RemoteObject};
use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use ureq::http;

const ROOT: &str = "http://127.0.0.1:5233/";
/// `test:test`.
const AUTH: &str = "Basic dGVzdDp0ZXN0";
const DUBAI: Tz = chrono_tz::Asia::Dubai;

const MKCALENDAR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:set><d:prop><d:displayname>Test calendar</d:displayname><a:calendar-color>#336699FF</a:calendar-color><c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"#;

/// A calendar made for one test, removed when dropped.
struct Calendar {
    url: String,
    agent: ureq::Agent,
}

impl Calendar {
    /// `None` (and a note) when the test server is not running.
    fn new(test: &str) -> Option<Calendar> {
        let addr = "127.0.0.1:5233".parse().unwrap();
        if TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err() {
            eprintln!("skipped: no CalDAV test server at {ROOT}");
            return None;
        }
        // Radicale answers HTTP/1.0 and closes each connection: do not keep any.
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            .max_idle_connections(0)
            .build()
            .new_agent();
        let cal = Calendar { url: format!("{ROOT}test/{test}-{}/", new_id()), agent };
        // The first request makes the user's home.
        cal.request("PROPFIND", &format!("{ROOT}test/"), "");
        assert_eq!(cal.request("MKCALENDAR", &cal.url, MKCALENDAR), 201);
        Some(cal)
    }

    fn request(&self, method: &str, url: &str, body: &str) -> u16 {
        let kind = if method == "PUT" { "text/calendar; charset=utf-8" } else { "application/xml; charset=utf-8" };
        let req = http::Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", AUTH)
            .header("Depth", "0")
            .header("Content-Type", kind)
            .body(body)
            .unwrap();
        self.agent.run(req).unwrap().status().as_u16()
    }
}

impl Drop for Calendar {
    fn drop(&mut self) {
        self.request("DELETE", &self.url, "");
    }
}

fn dav() -> CalDav {
    CalDav::new(ROOT, "test", "test").unwrap()
}

fn t(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
}

fn utc(s: &str) -> DateTime<Utc> {
    t(s).and_utc()
}

fn meeting(title: &str) -> Event {
    Event {
        uid: format!("{}@calinator.test", new_id()),
        title: title.into(),
        start: t("2026-11-02 09:00"),
        end: t("2026-11-02 10:00"),
        tz: "America/New_York".into(),
        reminders: vec![10],
        ..Default::default()
    }
}

fn upserts(changes: &[Change]) -> Vec<&RemoteObject> {
    changes
        .iter()
        .filter_map(|c| match c {
            Change::Upsert(o) => Some(o),
            Change::Deleted(_) => None,
        })
        .collect()
}

#[test]
fn discovery_finds_the_calendar_from_any_start() {
    let Some(cal) = Calendar::new("discover") else { return };
    for start in [ROOT.to_string(), format!("{ROOT}test/"), cal.url.clone(), "127.0.0.1:5233".into()] {
        let start = if start.starts_with("127.") { format!("http://{start}") } else { start };
        let mut dav = CalDav::new(&start, "test", "test").unwrap();
        let cals = dav.calendars().unwrap();
        let found = cals.iter().find(|c| c.remote_id == cal.url).unwrap_or_else(|| panic!("{start}: {cals:?}"));
        assert_eq!(found.name, "Test calendar");
        assert_eq!(found.color.as_deref(), Some("#336699"));
        assert!(!found.read_only);
        // Radicale lists no email addresses.
        assert!(dav.my_addresses().unwrap().is_empty());
    }
}

#[test]
fn new_event_comes_back_in_a_full_pull() {
    let Some(cal) = Calendar::new("full") else { return };
    let mut dav = dav();
    let ev = meeting("Planning, round 2");
    let put = dav.put(&cal.url, None, "", None, std::slice::from_ref(&ev), false).unwrap();
    assert_eq!(put.href, format!("{}{}.ics", cal.url, ev.uid.replace('@', "%40")));
    assert!(!put.etag.is_empty());
    assert_eq!(put.events.len(), 1);
    assert_eq!((put.events[0].title.as_str(), put.events[0].id.as_str()), ("Planning, round 2", ""));

    let pull = dav.pull(&cal.url, None).unwrap();
    assert!(pull.full);
    assert!(pull.token.is_some());
    let got = upserts(&pull.changes);
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].href.as_str(), got[0].etag.as_str()), (put.href.as_str(), put.etag.as_str()));
    let back = &got[0].events[0];
    assert_eq!((back.uid.as_str(), back.title.as_str(), back.tz.as_str()), (ev.uid.as_str(), ev.title.as_str(), ev.tz.as_str()));
    assert_eq!((back.start, back.end, back.reminders.clone()), (ev.start, ev.end, ev.reminders.clone()));
    assert!(put.raw.contains("SUMMARY:Planning\\, round 2"));
    assert!(got[0].raw.contains("SUMMARY:Planning\\, round 2"));
}

/// An event as a phone would store it, with properties Calinator does not show.
const PHONE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//iPhone 18.0//EN\r\nX-WR-CALNAME:Phone\r\nBEGIN:VEVENT\r\n\
UID:UID-HERE\r\nDTSTAMP:20261101T120000Z\r\nDTSTART:20261105T150000Z\r\nDTEND:20261105T160000Z\r\nSUMMARY:Dentist\r\n\
CATEGORIES:Health,Personal\r\nTRANSP:TRANSPARENT\r\nCLASS:PRIVATE\r\nX-APPLE-TRAVEL-ADVISORY-BEHAVIOR:AUTOMATIC\r\n\
URL;VALUE=URI:https://dentist.example/visit\r\nORGANIZER;CN=Dr Tooth:mailto:tooth@example.com\r\n\
ATTENDEE;CN=Me;ROLE=REQ-PARTICIPANT;RSVP=TRUE;PARTSTAT=NEEDS-ACTION:mailto:me@example.com\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT30M\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn editing_keeps_what_a_phone_stored() {
    let Some(cal) = Calendar::new("phone") else { return };
    let uid = new_id();
    assert_eq!(cal.request("PUT", &format!("{}{uid}.ics", cal.url), &PHONE.replace("UID-HERE", &uid)), 201);

    let mut dav = dav();
    let pull = dav.pull(&cal.url, None).unwrap();
    let got = upserts(&pull.changes)[0].clone();
    let mut events = got.events.clone();
    assert_eq!((events[0].title.as_str(), events[0].reminders.as_slice()), ("Dentist", &[30][..]));
    events[0].title = "Dentist (moved)".into();
    events[0].start = t("2026-11-05 16:00");
    events[0].end = t("2026-11-05 17:00");
    let put = dav.put(&cal.url, Some(&got.href), &got.etag, Some(&got.raw), &events, true).unwrap();
    assert_eq!(put.events[0].title, "Dentist (moved)");

    // What the server now has, as another client would read it.
    let again = dav.pull(&cal.url, None).unwrap();
    let stored = &upserts(&again.changes)[0].raw;
    let tops = calinator::ical::parse(stored);
    let vcal = &tops[0];
    assert_eq!(vcal.get("X-WR-CALNAME").unwrap().value, "Phone");
    let ev = vcal.children.iter().find(|c| c.name == "VEVENT").unwrap();
    assert_eq!(ev.get("SUMMARY").unwrap().value, "Dentist (moved)");
    assert_eq!(ev.get("DTSTART").unwrap().value, "20261105T160000Z");
    assert_eq!(ev.get("CATEGORIES").unwrap().value, "Health,Personal");
    assert_eq!(ev.get("TRANSP").unwrap().value, "TRANSPARENT");
    assert_eq!(ev.get("CLASS").unwrap().value, "PRIVATE");
    assert_eq!(ev.get("X-APPLE-TRAVEL-ADVISORY-BEHAVIOR").unwrap().value, "AUTOMATIC");
    assert_eq!(ev.get("URL").unwrap().value, "https://dentist.example/visit");
    assert_eq!(ev.get("ORGANIZER").unwrap().param("CN"), Some("Dr Tooth"));
    let me = ev.get("ATTENDEE").unwrap();
    assert_eq!((me.param("ROLE"), me.param("RSVP"), me.param("CN")), (Some("REQ-PARTICIPANT"), Some("TRUE"), Some("Me")));
    assert_eq!(ev.children.iter().filter(|c| c.name == "VALARM").count(), 1);
}

#[test]
fn stale_etag_is_a_conflict() {
    let Some(cal) = Calendar::new("conflict") else { return };
    let mut dav = dav();
    let mut ev = meeting("First");
    let first = dav.put(&cal.url, None, "", None, std::slice::from_ref(&ev), true).unwrap();
    ev.title = "Second".into();
    let second = dav.put(&cal.url, Some(&first.href), &first.etag, Some(&first.raw), std::slice::from_ref(&ev), true).unwrap();
    assert_eq!(second.href, first.href);
    assert_ne!(second.etag, first.etag);

    ev.title = "Third".into();
    assert_eq!(dav.put(&cal.url, Some(&first.href), &first.etag, Some(&first.raw), std::slice::from_ref(&ev), true), Err(Error::Conflict));
    // Creating it again under the same name is a conflict too.
    assert_eq!(dav.put(&cal.url, None, "", None, std::slice::from_ref(&ev), true), Err(Error::Conflict));
    assert_eq!(dav.delete(&cal.url, &first.href, &first.etag, true), Err(Error::Conflict));
    dav.delete(&cal.url, &first.href, &second.etag, true).unwrap();
    // Already gone is fine.
    dav.delete(&cal.url, &first.href, &second.etag, true).unwrap();
    dav.delete(&cal.url, &first.href, "", true).unwrap();
}

#[test]
fn delta_after_update_and_delete() {
    let Some(cal) = Calendar::new("delta") else { return };
    let mut dav = dav();
    let mut a = meeting("A");
    let b = meeting("B");
    let put_a = dav.put(&cal.url, None, "", None, std::slice::from_ref(&a), true).unwrap();
    let put_b = dav.put(&cal.url, None, "", None, std::slice::from_ref(&b), true).unwrap();
    let full = dav.pull(&cal.url, None).unwrap();
    assert_eq!(upserts(&full.changes).len(), 2);
    let token = full.token.unwrap();

    let none = dav.pull(&cal.url, Some(&token)).unwrap();
    assert!(!none.full);
    assert!(none.changes.is_empty(), "{:?}", none.changes);

    a.title = "A, moved".into();
    a.start = t("2026-11-03 09:00");
    a.end = t("2026-11-03 10:00");
    let put_a2 = dav.put(&cal.url, Some(&put_a.href), &put_a.etag, Some(&put_a.raw), std::slice::from_ref(&a), true).unwrap();
    let after_update = dav.pull(&cal.url, Some(&token)).unwrap();
    let got = upserts(&after_update.changes);
    assert_eq!(after_update.changes.len(), 1);
    assert_eq!((got[0].href.as_str(), got[0].etag.as_str()), (put_a2.href.as_str(), put_a2.etag.as_str()));
    assert_eq!((got[0].events[0].title.as_str(), got[0].events[0].start), ("A, moved", t("2026-11-03 09:00")));
    let token = after_update.token.unwrap();

    dav.delete(&cal.url, &put_b.href, &put_b.etag, true).unwrap();
    let after_delete = dav.pull(&cal.url, Some(&token)).unwrap();
    assert_eq!(after_delete.changes, vec![Change::Deleted(put_b.href.clone())]);
    assert!(!after_delete.full);
}

/// Start, end, part of a series, original start.
type Instance = (DateTime<Utc>, DateTime<Utc>, bool, Option<NaiveDateTime>);

/// Occurrences without the row ids (the server copy has none).
fn instances(events: &[Event]) -> Vec<Instance> {
    let occ: Vec<Occurrence> = expand(events, utc("2026-10-01 00:00"), utc("2027-01-01 00:00"), DUBAI);
    occ.into_iter().map(|o| (o.start, o.end, o.recurring, o.original)).collect()
}

#[test]
fn series_with_override_and_exdate_round_trips() {
    let Some(cal) = Calendar::new("series") else { return };
    let mut dav = dav();
    let uid = format!("{}@calinator.test", new_id());
    let series = Event {
        id: "s".into(),
        uid: uid.clone(),
        title: "Standup".into(),
        start: t("2026-10-19 09:00"),
        end: t("2026-10-19 09:30"),
        tz: "America/New_York".into(),
        rrule: Some("FREQ=WEEKLY;BYDAY=MO;COUNT=6".into()),
        rdates: vec![t("2026-11-04 15:00")],
        exdates: vec![t("2026-10-26 09:00")],
        ..Default::default()
    };
    let moved = Event {
        id: "o".into(),
        uid: uid.clone(),
        title: "Standup (moved)".into(),
        start: t("2026-11-09 11:00"),
        end: t("2026-11-09 11:30"),
        tz: "America/New_York".into(),
        recurrence_id: Some(t("2026-11-09 09:00")),
        ..Default::default()
    };
    let sent = vec![series, moved];
    dav.put(&cal.url, None, "", None, &sent, false).unwrap();

    let pull = dav.pull(&cal.url, None).unwrap();
    let got = upserts(&pull.changes);
    assert_eq!(got.len(), 1);
    let back = &got[0].events;
    assert_eq!(back.len(), 2);
    assert!(back[0].is_series() && back[1].recurrence_id == Some(t("2026-11-09 09:00")));
    assert_eq!(back[0].exdates, vec![t("2026-10-26 09:00")]);
    assert_eq!(back[0].rdates, vec![t("2026-11-04 15:00")]);

    let want = instances(&sent);
    assert_eq!(instances(back), want);
    let starts: Vec<DateTime<Utc>> = want.iter().map(|w| w.0).collect();
    assert_eq!(
        starts,
        vec![
            utc("2026-10-19 13:00"),
            // Oct 26 is skipped; New York leaves DST on Nov 1.
            utc("2026-11-02 14:00"),
            utc("2026-11-04 20:00"),
            utc("2026-11-09 16:00"),
            utc("2026-11-16 14:00"),
            utc("2026-11-23 14:00"),
        ]
    );
}

#[test]
fn unknown_token_has_expired() {
    let Some(cal) = Calendar::new("token") else { return };
    assert_eq!(dav().pull(&cal.url, Some("http://radicale.org/ns/sync/bogus")), Err(Error::TokenExpired));
}

#[test]
fn wrong_password_is_a_login_error() {
    let Some(_cal) = Calendar::new("auth") else { return };
    let mut dav = CalDav::new(ROOT, "test", "wrong").unwrap();
    assert!(matches!(dav.calendars(), Err(Error::Auth(_))), "{:?}", dav.calendars());
}
