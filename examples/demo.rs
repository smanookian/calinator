//! Fills a calendar file with sample events for screenshots and UI checks.
//! `XDG_DATA_HOME=/tmp/cal-demo cargo run --example demo` then run the app
//! with the same XDG_DATA_HOME.

use calinator::model::{Attendee, Calendar, Event, Partstat, new_id};
use calinator::store::Store;
use chrono::{Datelike, Duration, NaiveDate, Utc};

fn cal(id: &str, name: &str, color: &str, default: bool) -> Calendar {
    Calendar {
        id: id.into(),
        name: name.into(),
        color: color.into(),
        enabled: true,
        default,
        account: "local".into(),
        read_only: false,
        remote: None,
    }
}

fn main() {
    let store = Store::open_default().expect("store");
    let zone = calinator::tz::system();
    let today = calinator::tz::local(Utc::now(), zone).date();
    let mon = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    for c in [
        cal("personal", "Personal", "#89b4fa", true),
        cal("work", "Work", "#fab387", false),
        cal("family", "Family", "#a6e3a1", false),
        cal("holidays", "Holidays", "#f38ba8", false),
    ] {
        store.save_calendar(&c).expect("calendar");
    }
    let tzname = zone.name().to_string();
    let at = |d: NaiveDate, h: u32, m: u32| d.and_hms_opt(h, m, 0).expect("time");
    let ev = |cal: &str, title: &str, d: NaiveDate, h: u32, m: u32, mins: i64| Event {
        id: new_id(),
        uid: new_id(),
        calendar: cal.into(),
        title: title.into(),
        start: at(d, h, m),
        end: at(d, h, m) + Duration::minutes(mins),
        tz: tzname.clone(),
        reminders: vec![10],
        updated: Utc::now(),
        ..Default::default()
    };
    let mut events = vec![];
    let mut standup = ev("work", "Standup", mon, 9, 30, 15);
    standup.rrule = Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into());
    standup.video = Some("https://meet.google.com/abc-defg-hij".into());
    events.push(standup);
    let mut design = ev("work", "Design review", mon + Duration::days(1), 11, 0, 90);
    design.location = "Room 4B".into();
    design.organizer = Some("maya@example.com".into());
    design.attendees = vec![
        Attendee { email: "maya@example.com".into(), name: "Maya".into(), status: Partstat::Accepted, me: false },
        Attendee { email: "me@example.com".into(), name: "Me".into(), status: Partstat::Accepted, me: true },
        Attendee { email: "li@example.com".into(), name: "Li".into(), status: Partstat::Tentative, me: false },
    ];
    design.description = "Go through the new onboarding screens. Bring notes from user tests.".into();
    events.push(design);
    events.push(ev("work", "1:1 with Maya", mon + Duration::days(1), 12, 0, 30));
    let mut lunch = ev("personal", "Lunch with Sarah", mon + Duration::days(2), 12, 30, 60);
    lunch.location = "Café Lumen".into();
    events.push(lunch);
    let mut ny = ev("work", "Call with New York team", mon + Duration::days(2), 10, 0, 60);
    ny.tz = "America/New_York".into();
    ny.video = Some("https://zoom.us/j/123456789".into());
    events.push(ny);
    let mut invite = ev("work", "Quarterly planning", mon + Duration::days(3), 14, 0, 120);
    invite.organizer = Some("ceo@example.com".into());
    invite.attendees = vec![
        Attendee { email: "ceo@example.com".into(), name: "Dana".into(), status: Partstat::Accepted, me: false },
        Attendee { email: "me@example.com".into(), name: "Me".into(), status: Partstat::NeedsAction, me: true },
    ];
    events.push(invite);
    events.push(ev("work", "Write report", mon + Duration::days(3), 14, 30, 60));
    let mut gym = ev("personal", "Gym", mon, 18, 0, 60);
    gym.rrule = Some("FREQ=WEEKLY;BYDAY=MO,WE,FR".into());
    events.push(gym);
    events.push(ev("family", "Kids' swim class", mon + Duration::days(5), 10, 0, 45));
    events.push(ev("family", "Dinner at Mom's", mon + Duration::days(6), 18, 30, 120));
    let mut trip = ev("family", "Weekend trip", mon + Duration::days(4), 0, 0, 0);
    trip.all_day = true;
    trip.start = at(mon + Duration::days(4), 0, 0);
    trip.end = at(mon + Duration::days(7), 0, 0);
    events.push(trip);
    let mut bday = ev("holidays", "Sam's birthday", mon + Duration::days(2), 0, 0, 0);
    bday.all_day = true;
    bday.start = at(mon + Duration::days(2), 0, 0);
    bday.end = at(mon + Duration::days(3), 0, 0);
    bday.rrule = Some("FREQ=YEARLY".into());
    events.push(bday);
    events.push(ev("personal", "Dentist", mon + Duration::days(9), 8, 15, 45));
    events.push(ev("work", "Team offsite", mon + Duration::days(15), 9, 0, 480));
    for e in &events {
        store.save_event(e).expect("event");
    }
    println!("Added {} events to {}", events.len(), calinator::store::data_dir().display());
}
