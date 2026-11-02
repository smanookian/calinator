//! Sync engine against an in-memory server: offline queue, conflicts, deletes
//! on either side, repeating-event edits, moves between calendars, guest
//! notifications.

use calinator::accounts::{Account, Kind};
use calinator::edit::{self, Changes, Scope, Target};
use calinator::model::{Event, new_id};
use calinator::store::Store;
use calinator::sync::engine::{Report, sync_account};
use calinator::sync::fake::Fake;
use chrono::{NaiveDateTime, Utc};
use chrono_tz::Tz;

const NY: Tz = chrono_tz::America::New_York;

fn t(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
}

fn ev(uid: &str, title: &str) -> Event {
    Event {
        id: new_id(),
        uid: uid.into(),
        title: title.into(),
        start: t("2026-11-02 09:00"),
        end: t("2026-11-02 10:00"),
        tz: "America/New_York".into(),
        updated: Utc::now(),
        ..Default::default()
    }
}

struct World {
    store: Store,
    account: Account,
    fake: Fake,
}

impl World {
    fn new() -> Self {
        let fake = Fake::default();
        fake.0.lock().unwrap().add_calendar("home", "Home");
        let account = Account { id: "acc".into(), kind: Kind::CalDav, name: "Test".into(), ..Default::default() };
        let store = Store::open_in_memory().unwrap();
        store.save_account(&account).unwrap();
        let mut w = World { store, account, fake };
        w.sync();
        w
    }

    fn sync(&mut self) -> Report {
        let mut p = self.fake.clone();
        sync_account(&mut self.store, &mut self.account, &mut p)
    }

    fn cal(&self, name: &str) -> String {
        self.store.calendars().unwrap().into_iter().find(|c| c.name == name).unwrap().id
    }

    fn local(&self, uid: &str) -> Vec<Event> {
        self.store.group(uid).unwrap()
    }

    fn server(&self, uid: &str) -> Option<Vec<Event>> {
        self.fake.0.lock().unwrap().object("home", uid).cloned()
    }

    fn add_local(&mut self, e: Event) {
        let mut e = e;
        e.calendar = self.cal("Home");
        self.store.apply(&Changes::new(vec![e], vec![])).unwrap();
    }

    fn rename_local(&mut self, uid: &str, title: &str) {
        let mut e = self.local(uid).remove(0);
        e.title = title.into();
        self.store.apply(&Changes::new(vec![e], vec![])).unwrap();
    }

    fn rename_remote(&mut self, uid: &str, title: &str) {
        let mut s = self.fake.0.lock().unwrap();
        let mut events = s.object("home", uid).unwrap().clone();
        events[0].title = title.into();
        s.put_from_elsewhere("home", events);
    }

    fn pending(&self) -> i64 {
        self.store.pending().unwrap().iter().map(|(_, n)| n).sum()
    }
}

#[test]
fn first_sync_creates_calendars_and_pulls_events() {
    let mut w = World::new();
    w.fake.0.lock().unwrap().put_from_elsewhere("home", vec![ev("phone-1", "Made on the phone")]);
    let r = w.sync();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(w.local("phone-1")[0].title, "Made on the phone");
    assert_eq!(w.local("phone-1")[0].calendar, w.cal("Home"));
    // Pulling does not queue anything for upload.
    assert_eq!(w.pending(), 0);
}

#[test]
fn local_create_edit_delete_reach_the_server() {
    let mut w = World::new();
    w.add_local(ev("u1", "Lunch"));
    assert_eq!(w.pending(), 1);
    w.sync();
    assert_eq!(w.server("u1").unwrap()[0].title, "Lunch");
    w.rename_local("u1", "Lunch with Sarah");
    w.sync();
    assert_eq!(w.server("u1").unwrap()[0].title, "Lunch with Sarah");
    let id = w.local("u1")[0].id.clone();
    w.store.apply(&Changes::new(vec![], vec![id])).unwrap();
    w.sync();
    assert!(w.server("u1").is_none());
    assert_eq!(w.pending(), 0);
}

#[test]
fn offline_edits_wait_and_nothing_is_lost() {
    let mut w = World::new();
    w.add_local(ev("u1", "Dentist"));
    w.sync();
    w.fake.0.lock().unwrap().offline = true;
    w.rename_local("u1", "Dentist (moved)");
    w.add_local(ev("u2", "New while offline"));
    let r = w.sync();
    assert!(!r.errors.is_empty());
    assert_eq!(w.pending(), 2);
    w.fake.0.lock().unwrap().offline = false;
    let r = w.sync();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(w.server("u1").unwrap()[0].title, "Dentist (moved)");
    assert_eq!(w.server("u2").unwrap()[0].title, "New while offline");
    assert_eq!(w.pending(), 0);
}

#[test]
fn changed_on_both_sides_server_wins_and_ours_is_kept() {
    let mut w = World::new();
    w.add_local(ev("u1", "Meeting"));
    w.sync();
    w.rename_local("u1", "Meeting (mine)");
    w.rename_remote("u1", "Meeting (phone)");
    let r = w.sync();
    assert_eq!(r.conflicts, 1);
    assert_eq!(w.local("u1")[0].title, "Meeting (phone)");
    assert_eq!(w.server("u1").unwrap()[0].title, "Meeting (phone)");
    let c = &w.store.conflicts().unwrap()[0];
    assert_eq!(c.mine[0].title, "Meeting (mine)");
    assert_eq!(c.theirs[0].title, "Meeting (phone)");
    assert_eq!(w.pending(), 0);
}

#[test]
fn deleted_on_server_but_edited_here_is_put_back() {
    let mut w = World::new();
    w.add_local(ev("u1", "Keep me"));
    w.sync();
    let href = w.fake.0.lock().unwrap().objects.keys().next().unwrap().1.clone();
    w.fake.0.lock().unwrap().delete_from_elsewhere("home", &href);
    w.rename_local("u1", "Keep me, edited");
    let r = w.sync();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    w.sync();
    assert_eq!(w.server("u1").unwrap()[0].title, "Keep me, edited");
}

#[test]
fn deleted_on_server_and_untouched_here_goes_away() {
    let mut w = World::new();
    w.add_local(ev("u1", "Cancelled"));
    w.sync();
    let href = w.fake.0.lock().unwrap().objects.keys().next().unwrap().1.clone();
    w.fake.0.lock().unwrap().delete_from_elsewhere("home", &href);
    w.sync();
    assert!(w.local("u1").is_empty());
}

#[test]
fn deleted_here_but_changed_on_server_comes_back() {
    let mut w = World::new();
    w.add_local(ev("u1", "Party"));
    w.sync();
    let id = w.local("u1")[0].id.clone();
    w.store.apply(&Changes::new(vec![], vec![id])).unwrap();
    w.rename_remote("u1", "Party (new time)");
    let r = w.sync();
    assert_eq!(r.conflicts, 1);
    assert_eq!(w.local("u1")[0].title, "Party (new time)");
    assert!(w.server("u1").is_some());
}

#[test]
fn repeating_edit_this_event_uploads_series_with_override() {
    let mut w = World::new();
    let mut s = ev("series", "Standup");
    s.rrule = Some("FREQ=DAILY;COUNT=5".into());
    w.add_local(s);
    w.sync();
    let group = w.local("series");
    let mut changed = group[0].clone();
    changed.title = "Standup (late)".into();
    changed.start = t("2026-11-04 11:00");
    changed.end = t("2026-11-04 11:30");
    let target = Target { event_id: group[0].id.clone(), original: Some(t("2026-11-04 09:00")) };
    let ch = edit::edit(&group, &target, &changed, Scope::This, NY, Utc::now());
    w.store.apply(&ch).unwrap();
    w.sync();
    let on_server = w.server("series").unwrap();
    assert_eq!(on_server.len(), 2);
    let o = on_server.iter().find(|e| e.recurrence_id.is_some()).unwrap();
    assert_eq!((o.title.as_str(), o.recurrence_id), ("Standup (late)", Some(t("2026-11-04 09:00"))));
}

#[test]
fn this_and_following_creates_a_second_object() {
    let mut w = World::new();
    let mut s = ev("series", "Gym");
    s.rrule = Some("FREQ=DAILY;COUNT=6".into());
    w.add_local(s);
    w.sync();
    let group = w.local("series");
    let mut changed = group[0].clone();
    changed.title = "Gym (evening)".into();
    changed.start = t("2026-11-05 18:00");
    changed.end = t("2026-11-05 19:00");
    let target = Target { event_id: group[0].id.clone(), original: Some(t("2026-11-05 09:00")) };
    w.store.apply(&edit::edit(&group, &target, &changed, Scope::Following, NY, Utc::now())).unwrap();
    w.sync();
    let s = w.fake.0.lock().unwrap();
    assert_eq!(s.objects.len(), 2);
    assert!(s.object("home", "series").unwrap()[0].rrule.as_deref().unwrap().contains("UNTIL="));
}

#[test]
fn moving_to_another_calendar_moves_on_the_server() {
    let mut w = World::new();
    w.fake.0.lock().unwrap().add_calendar("work", "Work");
    w.sync();
    w.add_local(ev("u1", "Review"));
    w.sync();
    let mut e = w.local("u1").remove(0);
    e.calendar = w.cal("Work");
    w.store.apply(&Changes::new(vec![e], vec![])).unwrap();
    w.sync();
    let s = w.fake.0.lock().unwrap();
    assert!(s.object("home", "u1").is_none());
    assert!(s.object("work", "u1").is_some());
}

#[test]
fn guests_are_told_only_when_the_user_said_so() {
    let mut w = World::new();
    w.add_local(ev("u1", "Quiet"));
    w.sync();
    let mut e = w.local("u1").remove(0);
    e.title = "Loud".into();
    let mut ch = Changes::new(vec![e], vec![]);
    ch.notify = true;
    w.store.apply(&ch).unwrap();
    w.sync();
    assert_eq!(w.fake.0.lock().unwrap().notified, vec![false, true]);
}

#[test]
fn an_edit_during_sync_stays_queued() {
    let mut w = World::new();
    w.add_local(ev("u1", "A"));
    // The upload records rev 1; a later local edit (rev 2) is still pending.
    let cal = w.cal("Home");
    let rev_at_upload = w.store.sync_row("u1", &cal).unwrap().unwrap().rev;
    w.rename_local("u1", "B");
    w.store.set_synced("u1", &cal, "home/u1.ics", "\"x\"", "", &[], rev_at_upload).unwrap();
    assert!(w.store.sync_row("u1", &cal).unwrap().unwrap().dirty());
}

#[test]
fn uploads_start_from_the_server_text() {
    let mut w = World::new();
    w.add_local(ev("u1", "A"));
    w.sync();
    w.rename_remote("u1", "B");
    w.sync();
    w.rename_local("u1", "C");
    w.sync();
    let s = w.fake.0.lock().unwrap();
    // First upload: nothing to start from. The edit: the text from the last pull.
    assert_eq!(s.raws.first(), Some(&None));
    assert_eq!(s.raws.last().unwrap().as_deref(), Some("raw \"2\""));
}

#[test]
fn an_account_removed_during_sync_stays_removed() {
    let mut w = World::new();
    let mut stale = w.account.clone();
    w.store.delete_account("acc").unwrap();
    // A sync that started before the removal finishes afterwards.
    let mut p = w.fake.clone();
    sync_account(&mut w.store, &mut stale, &mut p);
    assert!(w.store.accounts().unwrap().is_empty());
    assert!(w.store.calendars().unwrap().iter().all(|c| c.account != "acc"));
}
