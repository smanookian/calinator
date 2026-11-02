//! Tricky repeating events, time zones and DST, and the three edit scopes.

use calinator::edit::{self, Scope, Target};
use calinator::model::{Event, Occurrence, Status};
use calinator::recur::expand;
use calinator::store::Store;
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

const NY: Tz = chrono_tz::America::New_York;
const DUBAI: Tz = chrono_tz::Asia::Dubai;
const BERLIN: Tz = chrono_tz::Europe::Berlin;

fn t(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
}

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn utc(s: &str) -> DateTime<Utc> {
    Utc.from_utc_datetime(&t(s))
}

fn ev(id: &str, start: &str, end: &str, tz: &str, rule: Option<&str>) -> Event {
    Event {
        id: id.into(),
        uid: format!("uid-{id}"),
        calendar: "personal".into(),
        title: id.into(),
        start: t(start),
        end: t(end),
        tz: tz.into(),
        rrule: rule.map(Into::into),
        ..Default::default()
    }
}

/// Starts as wall time in `zone`.
fn local_starts(o: &[Occurrence], zone: Tz) -> Vec<String> {
    o.iter().map(|o| o.start.with_timezone(&zone).format("%Y-%m-%d %H:%M").to_string()).collect()
}

fn range(events: &[Event], from: &str, to: &str) -> Vec<Occurrence> {
    expand(events, utc(from), utc(to), NY)
}

#[test]
fn weekly_keeps_wall_time_across_dst() {
    let e = ev("w", "2026-10-26 09:00", "2026-10-26 10:00", "America/New_York", Some("FREQ=WEEKLY"));
    let o = range(&[e], "2026-10-26 00:00", "2026-11-10 00:00");
    assert_eq!(local_starts(&o, NY), ["2026-10-26 09:00", "2026-11-02 09:00", "2026-11-09 09:00"]);
    // The UTC hour moves by one after New York leaves summer time on Nov 1.
    assert_eq!(o[0].start, utc("2026-10-26 13:00"));
    assert_eq!(o[1].start, utc("2026-11-02 14:00"));
}

#[test]
fn new_york_event_shows_right_in_dubai() {
    let e = ev("ny", "2026-11-05 09:00", "2026-11-05 10:00", "America/New_York", None);
    let o = expand(&[e], utc("2026-11-04 00:00"), utc("2026-11-07 00:00"), DUBAI);
    assert_eq!(local_starts(&o, DUBAI), ["2026-11-05 18:00"]);
}

#[test]
fn europe_and_us_change_on_different_weeks() {
    // Berlin leaves summer time Oct 25, New York Nov 1: 15:00 Berlin is
    // 09:00, then 10:00 for one week, then 09:00 again in New York.
    let e = ev("b", "2026-10-19 15:00", "2026-10-19 16:00", "Europe/Berlin", Some("FREQ=WEEKLY;COUNT=3"));
    let o = range(&[e], "2026-10-01 00:00", "2026-12-01 00:00");
    assert_eq!(local_starts(&o, NY), ["2026-10-19 09:00", "2026-10-26 10:00", "2026-11-02 09:00"]);
    assert_eq!(local_starts(&o, BERLIN), ["2026-10-19 15:00", "2026-10-26 15:00", "2026-11-02 15:00"]);
}

#[test]
fn daily_at_0230_jumps_past_spring_gap() {
    let e = ev("g", "2026-03-07 02:30", "2026-03-07 03:00", "America/New_York", Some("FREQ=DAILY;COUNT=3"));
    let o = range(&[e], "2026-03-01 00:00", "2026-03-20 00:00");
    // Mar 8 02:30 does not exist; it moves to 03:30 like other calendar apps.
    assert_eq!(local_starts(&o, NY), ["2026-03-07 02:30", "2026-03-08 03:30", "2026-03-09 02:30"]);
}

#[test]
fn exdate_rdate_and_until_date_only() {
    let mut e = ev("x", "2026-11-02 09:00", "2026-11-02 09:30", "America/New_York", Some("FREQ=DAILY;UNTIL=20261105"));
    e.exdates = vec![t("2026-11-03 09:00")];
    e.rdates = vec![t("2026-11-08 12:00")];
    let o = range(&[e], "2026-11-01 00:00", "2026-11-30 00:00");
    assert_eq!(local_starts(&o, NY), ["2026-11-02 09:00", "2026-11-04 09:00", "2026-11-05 09:00", "2026-11-08 12:00"]);
}

#[test]
fn monthly_31st_skips_short_months_and_last_friday() {
    let e = ev("m", "2026-01-31 08:00", "2026-01-31 09:00", "America/New_York", Some("FREQ=MONTHLY;COUNT=4"));
    let o = range(&[e], "2026-01-01 00:00", "2027-01-01 00:00");
    assert_eq!(local_starts(&o, NY), ["2026-01-31 08:00", "2026-03-31 08:00", "2026-05-31 08:00", "2026-07-31 08:00"]);

    let f = ev("f", "2026-10-30 17:00", "2026-10-30 18:00", "America/New_York", Some("FREQ=MONTHLY;BYDAY=-1FR;COUNT=3"));
    let o = range(&[f], "2026-10-01 00:00", "2027-02-01 00:00");
    assert_eq!(local_starts(&o, NY), ["2026-10-30 17:00", "2026-11-27 17:00", "2026-12-25 17:00"]);
}

#[test]
fn leap_day_yearly() {
    let mut e = ev("l", "2028-02-29 00:00", "2028-03-01 00:00", "", Some("FREQ=YEARLY;COUNT=2"));
    e.all_day = true;
    let o = expand(&[e], utc("2028-01-01 00:00"), utc("2037-01-01 00:00"), NY);
    let days: Vec<_> = o.iter().map(|o| o.days.unwrap().0).collect();
    assert_eq!(days, [d("2028-02-29"), d("2032-02-29")]);
}

#[test]
fn all_day_multi_day_covers_each_day_in_any_zone() {
    let mut e = ev("trip", "2026-11-06 00:00", "2026-11-09 00:00", "", None);
    e.all_day = true;
    for zone in [NY, DUBAI] {
        let o = expand(std::slice::from_ref(&e), utc("2026-11-01 00:00"), utc("2026-11-15 00:00"), zone);
        assert_eq!(o.len(), 1);
        let on: Vec<bool> = ["2026-11-05", "2026-11-06", "2026-11-08", "2026-11-09"].iter().map(|x| o[0].on_day(d(x), zone)).collect();
        assert_eq!(on, [false, true, true, false]);
    }
}

#[test]
fn override_replaces_and_cancelled_override_hides() {
    let m = ev("s", "2026-11-02 09:00", "2026-11-02 10:00", "America/New_York", Some("FREQ=DAILY;COUNT=3"));
    let mut moved = ev("o1", "2026-11-03 15:00", "2026-11-03 16:00", "America/New_York", None);
    moved.uid = m.uid.clone();
    moved.recurrence_id = Some(t("2026-11-03 09:00"));
    let mut gone = ev("o2", "2026-11-04 09:00", "2026-11-04 10:00", "America/New_York", None);
    gone.uid = m.uid.clone();
    gone.recurrence_id = Some(t("2026-11-04 09:00"));
    gone.status = Status::Cancelled;
    let o = range(&[m, moved, gone], "2026-11-01 00:00", "2026-11-10 00:00");
    assert_eq!(local_starts(&o, NY), ["2026-11-02 09:00", "2026-11-03 15:00"]);
    assert!(o.iter().all(|o| o.recurring));
}

// ---- Edit scopes ----

fn series() -> Vec<Event> {
    vec![ev("s", "2026-11-02 09:00", "2026-11-02 10:00", "America/New_York", Some("FREQ=WEEKLY;BYDAY=MO;COUNT=6"))]
}

fn apply(store: &mut Store, ch: &edit::Changes) -> Vec<Event> {
    store.apply(ch).unwrap();
    store.events().unwrap()
}

fn store_with(events: &[Event]) -> Store {
    let s = Store::open_in_memory().unwrap();
    for e in events {
        s.save_event(e).unwrap();
    }
    s
}

fn pick(o: &[Occurrence], i: usize) -> Target {
    Target { event_id: o[i].event_id.clone(), original: o[i].original }
}

fn titles(o: &[Occurrence], events: &[Event]) -> Vec<String> {
    o.iter().map(|o| events.iter().find(|e| e.id == o.event_id).unwrap().title.clone()).collect()
}

const FROM: &str = "2026-11-01 00:00";
const TO: &str = "2027-01-01 00:00";

#[test]
fn edit_this_only_changes_one() {
    let mut s = store_with(&series());
    let all = s.events().unwrap();
    let o = range(&all, FROM, TO);
    let mut e = all[0].clone();
    e.title = "moved".into();
    e.start = t("2026-11-10 11:00");
    e.end = t("2026-11-10 12:00");
    let after = apply(&mut s, &edit::edit(&all, &pick(&o, 1), &e, Scope::This, NY, Utc::now()));
    let o = range(&after, FROM, TO);
    assert_eq!(o.len(), 6);
    assert_eq!(local_starts(&o, NY)[1], "2026-11-10 11:00");
    assert_eq!(titles(&o, &after), ["s", "moved", "s", "s", "s", "s"]);
    // Editing the override again keeps one override.
    let mut e2 = after.iter().find(|e| e.title == "moved").unwrap().clone();
    e2.title = "moved again".into();
    let after = apply(&mut s, &edit::edit(&after, &pick(&o, 1), &e2, Scope::This, NY, Utc::now()));
    assert_eq!(after.len(), 2);
}

#[test]
fn edit_following_splits_and_keeps_count() {
    let mut s = store_with(&series());
    let all = s.events().unwrap();
    let o = range(&all, FROM, TO);
    // A one-off change to instance 5 must follow the split.
    let mut x = all[0].clone();
    x.title = "special".into();
    x.start = t("2026-11-30 13:00");
    x.end = t("2026-11-30 14:00");
    let all = apply(&mut s, &edit::edit(&all, &pick(&o, 4), &x, Scope::This, NY, Utc::now()));
    let o = range(&all, FROM, TO);

    let mut e = all.iter().find(|e| e.is_series()).unwrap().clone();
    e.title = "new time".into();
    e.start = t("2026-11-17 14:00"); // instance 3, moved from Mon to Tue 14:00
    e.end = t("2026-11-17 15:00");
    let after = apply(&mut s, &edit::edit(&all, &pick(&o, 2), &e, Scope::Following, NY, Utc::now()));
    let o = range(&after, FROM, TO);
    // The one-off keeps the time the user gave it.
    assert_eq!(
        local_starts(&o, NY),
        ["2026-11-02 09:00", "2026-11-09 09:00", "2026-11-17 14:00", "2026-11-24 14:00", "2026-11-30 13:00", "2026-12-08 14:00"]
    );
    assert_eq!(titles(&o, &after), ["s", "s", "new time", "new time", "special", "new time"]);
}

#[test]
fn edit_all_moves_weekday_and_overrides() {
    let mut s = store_with(&series());
    let all = s.events().unwrap();
    let o = range(&all, FROM, TO);
    let mut e = all[0].clone();
    e.title = "wed".into();
    e.start = t("2026-11-18 09:00"); // instance 3 moved Mon → Wed
    e.end = t("2026-11-18 10:00");
    let after = apply(&mut s, &edit::edit(&all, &pick(&o, 2), &e, Scope::All, NY, Utc::now()));
    let o = range(&after, FROM, TO);
    assert_eq!(local_starts(&o, NY)[..3], ["2026-11-04 09:00", "2026-11-11 09:00", "2026-11-18 09:00"]);
    assert_eq!(o.len(), 6);
}

#[test]
fn delete_scopes() {
    let base = series();
    let o = range(&base, FROM, TO);
    let now = Utc::now();

    let mut s = store_with(&base);
    let after = apply(&mut s, &edit::delete(&base, &pick(&o, 1), Scope::This, NY, now));
    assert_eq!(range(&after, FROM, TO).len(), 5);

    let mut s = store_with(&base);
    let after = apply(&mut s, &edit::delete(&base, &pick(&o, 3), Scope::Following, NY, now));
    assert_eq!(local_starts(&range(&after, FROM, TO), NY), ["2026-11-02 09:00", "2026-11-09 09:00", "2026-11-16 09:00"]);

    let mut s = store_with(&base);
    let after = apply(&mut s, &edit::delete(&base, &pick(&o, 3), Scope::All, NY, now));
    assert!(after.is_empty());
}

#[test]
fn delete_following_all_day_and_floating_until() {
    let mut e = ev("a", "2026-11-02 00:00", "2026-11-03 00:00", "", Some("FREQ=DAILY"));
    e.all_day = true;
    let base = vec![e];
    let o = expand(&base, utc(FROM), utc("2026-11-10 00:00"), DUBAI);
    let mut s = store_with(&base);
    let after = apply(&mut s, &edit::delete(&base, &pick(&o, 3), Scope::Following, DUBAI, Utc::now()));
    let o = expand(&after, utc(FROM), utc("2026-12-01 00:00"), DUBAI);
    let days: Vec<_> = o.iter().map(|o| o.days.unwrap().0).collect();
    assert_eq!(days, [d("2026-11-02"), d("2026-11-03"), d("2026-11-04")]);
}
