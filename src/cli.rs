//! Headless commands for scripts, bars and AI agents. `--json` everywhere.

use std::path::Path;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::{Value, json};

use crate::edit::{self, Changes, Scope, Target};
use crate::model::{Event, Occurrence, new_id};
use crate::settings::Settings;
use crate::store::Store;
use crate::{quickadd, recur, tz};

pub const HELP: &str = "Calinator: a terminal calendar.

Usage:
  calinator                         open the app
  calinator agenda [--days N] [--json]
  calinator events list [--from DAY] [--to DAY] [--json]
  calinator events create TEXT [--calendar NAME] [--json]
      e.g. calinator events create \"lunch with Sarah tomorrow 1pm\"
  calinator events delete ID [--start TIME] [--scope this|following|all]
  calinator search TEXT [--json]
  calinator calendars [--json]
  calinator import FILE.ics         import events from a file
  calinator import webcal://…       subscribe to a calendar link
  calinator sync [--json]           sync every account now
  calinator accounts [--json]       list accounts
  calinator accounts add caldav URL USER     (password on stdin)
  calinator accounts add icloud APPLE_ID     (app password on stdin)
  calinator accounts remove ID
  calinator --bar                   next event for the Omarchy bar / Waybar
  calinator doctor                  check that everything works
  calinator daemon [--install | --uninstall]
                                    reminders with the app closed

Options:
  --db FILE     use another calendar file
  --version     show the version

Days: today, tomorrow, fri, nov 20, 2026-11-20.";

pub type Result<T, E = String> = std::result::Result<T, E>;

struct Ctx {
    store: Store,
    settings: Settings,
    zone: Tz,
}

impl Ctx {
    fn open(db: &Path) -> Result<Self> {
        let store = Store::open(db).map_err(|e| format!("cannot open {}: {e}", db.display()))?;
        store.ensure_calendar().map_err(|e| e.to_string())?;
        let settings = Settings::load(&store);
        let zone = settings.zone();
        Ok(Ctx { store, settings, zone })
    }

    fn shown_events(&self) -> Result<Vec<Event>> {
        let on: Vec<String> = self.store.calendars().map_err(|e| e.to_string())?.into_iter().filter(|c| c.enabled).map(|c| c.id).collect();
        Ok(self.store.events().map_err(|e| e.to_string())?.into_iter().filter(|e| on.contains(&e.calendar)).collect())
    }

    fn today(&self) -> NaiveDate {
        tz::local(Utc::now(), self.zone).date()
    }
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// Words that are not options or option values.
fn words(args: &[String], with_value: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if with_value.contains(&a.as_str()) {
            skip = true;
        } else if !a.starts_with("--") {
            out.push(a.clone());
        }
    }
    out
}

fn day(ctx: &Ctx, s: &str) -> Result<NaiveDate> {
    quickadd::parse_date(s, ctx.today()).ok_or_else(|| format!("can't read the day “{s}”. Try today, fri, nov 20 or 2026-11-20"))
}

/// An occurrence as JSON, times with the user's zone offset.
fn occ_json(ctx: &Ctx, o: &Occurrence, events: &[Event]) -> Value {
    let e = events.iter().find(|e| e.id == o.event_id);
    let cal = ctx.store.calendars().ok().and_then(|cs| cs.into_iter().find(|c| c.id == o.calendar));
    let rule = e.and_then(|e| events.iter().find(|x| x.uid == e.uid && x.is_series())).and_then(|m| m.rrule.clone());
    let t = |x: DateTime<Utc>| x.with_timezone(&ctx.zone).to_rfc3339();
    let (start, end) = match o.days {
        Some((a, b)) => (json!(a.to_string()), json!((b - Duration::days(1)).to_string())),
        None => (json!(t(o.start)), json!(t(o.end))),
    };
    json!({
        "id": o.event_id,
        "uid": o.uid,
        "calendar": cal.as_ref().map(|c| c.name.clone()),
        "calendar_id": o.calendar,
        "title": e.map(|e| e.title.clone()),
        "start": start,
        "end": end,
        "all_day": o.all_day,
        "timezone": e.filter(|_| !o.all_day).map(|e| recur::event_zone(e, ctx.zone).name().to_string()),
        "instance_start": o.original.map(|_| t(o.start)),
        "recurring": o.recurring,
        "repeat": rule.as_deref().map(recur::describe),
        "rrule": rule,
        "location": e.map(|e| e.location.clone()).filter(|s| !s.is_empty()),
        "description": e.map(|e| e.description.clone()).filter(|s| !s.is_empty()),
        "video": e.and_then(|e| e.video_link()),
        "attendees": e.map(|e| e.attendees.iter().map(|a| json!({"email": a.email, "name": a.name, "status": a.status.label(), "me": a.me})).collect::<Vec<_>>()),
        "my_status": e.and_then(|e| e.my_status()).map(|s| s.label()),
        "reminders_minutes": e.map(|e| e.reminders.clone()),
    })
}

fn line(ctx: &Ctx, o: &Occurrence, events: &[Event]) -> String {
    let title = events.iter().find(|e| e.id == o.event_id).map(|e| e.title.clone()).unwrap_or_default();
    let h24 = ctx.settings.clock_24h;
    let when = match o.days {
        Some(_) => "all day".to_string(),
        None => format!(
            "{} – {}",
            crate::fmt::time_label(tz::local(o.start, ctx.zone).time(), h24),
            crate::fmt::time_label(tz::local(o.end, ctx.zone).time(), h24)
        ),
    };
    format!("  {when:<18} {title}  [{}]", o.event_id)
}

fn print_list(ctx: &Ctx, occs: &[Occurrence], events: &[Event], json_out: bool) {
    // By day, all-day first within a day (expand sorts all-day first overall).
    let day = |o: &Occurrence| o.days.map(|(a, _)| a).unwrap_or_else(|| tz::local(o.start, ctx.zone).date());
    let mut sorted: Vec<Occurrence> = occs.to_vec();
    sorted.sort_by_key(|o| (day(o), !o.all_day, o.start));
    let occs = &sorted[..];
    if json_out {
        let v: Vec<Value> = occs.iter().map(|o| occ_json(ctx, o, events)).collect();
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return;
    }
    if occs.is_empty() {
        println!("No events.");
    }
    let mut last = None;
    for o in occs {
        let d = o.days.map(|(a, _)| a).unwrap_or_else(|| tz::local(o.start, ctx.zone).date());
        if last != Some(d) {
            println!("{}", d.format("%A, %B %-d"));
            last = Some(d);
        }
        println!("{}", line(ctx, o, events));
    }
}

fn range(ctx: &Ctx, from: NaiveDate, to_excl: NaiveDate) -> Result<(Vec<Occurrence>, Vec<Event>)> {
    let events = ctx.shown_events()?;
    let a = tz::day_bounds(from, ctx.zone).0;
    let b = tz::day_bounds(to_excl, ctx.zone).0;
    Ok((recur::expand(&events, a, b, ctx.zone), events))
}

fn agenda(ctx: &Ctx, args: &[String]) -> Result<()> {
    let days: i64 = opt(args, "--days").map(|d| d.parse().map_err(|_| "--days needs a number")).transpose()?.unwrap_or(7);
    let today = ctx.today();
    let (mut occs, events) = range(ctx, today, today + Duration::days(days.max(1)))?;
    // Today: leave out what already ended.
    let now = Utc::now();
    occs.retain(|o| o.all_day || o.end > now);
    print_list(ctx, &occs, &events, flag(args, "--json"));
    Ok(())
}

fn events_list(ctx: &Ctx, args: &[String]) -> Result<()> {
    let from = opt(args, "--from").map(|s| day(ctx, s)).transpose()?.unwrap_or(ctx.today());
    let to = opt(args, "--to").map(|s| day(ctx, s)).transpose()?.unwrap_or(from + Duration::days(6));
    if to < from {
        return Err("--to is before --from".into());
    }
    let (occs, events) = range(ctx, from, to + Duration::days(1))?;
    print_list(ctx, &occs, &events, flag(args, "--json"));
    Ok(())
}

fn events_create(ctx: &mut Ctx, args: &[String]) -> Result<()> {
    let text = words(args, &["--calendar"]).join(" ");
    if text.trim().is_empty() {
        return Err("say what to add, e.g. calinator events create \"lunch tomorrow 1pm\"".into());
    }
    let cals = ctx.store.calendars().map_err(|e| e.to_string())?;
    let cal = match opt(args, "--calendar") {
        Some(name) => {
            cals.iter().find(|c| c.name.eq_ignore_ascii_case(name) || c.id == name).ok_or_else(|| format!("no calendar “{name}”"))?
        }
        None => {
            cals.iter().find(|c| c.default && !c.read_only).or_else(|| cals.iter().find(|c| !c.read_only)).ok_or("no calendar to add to")?
        }
    };
    if cal.read_only {
        return Err(format!("“{}” is read-only", cal.name));
    }
    let now = tz::local(Utc::now(), ctx.zone);
    let p = quickadd::parse(&text, now);
    if p.title.trim().is_empty() {
        return Err("the event needs a title".into());
    }
    let d = p.date.unwrap_or(now.date());
    let mut e = Event {
        id: new_id(),
        uid: new_id(),
        calendar: cal.id.clone(),
        title: p.title.clone(),
        tz: ctx.zone.name().into(),
        rrule: p.rrule.clone(),
        reminders: if ctx.settings.default_reminder >= 0 { vec![ctx.settings.default_reminder] } else { vec![] },
        updated: Utc::now(),
        ..Default::default()
    };
    match (p.start, p.all_day) {
        (Some(st), false) => {
            e.start = d.and_time(st);
            e.end = match (p.end, p.minutes) {
                (Some(en), _) => {
                    let mut x = d.and_time(en);
                    if x <= e.start {
                        x += Duration::days(1);
                    }
                    x
                }
                (None, Some(m)) => e.start + Duration::minutes(m),
                (None, None) => e.start + Duration::minutes(ctx.settings.default_duration),
            };
        }
        _ => {
            e.all_day = true;
            e.start = d.and_hms_opt(0, 0, 0).expect("midnight");
            e.end = e.start + Duration::days(1);
        }
    }
    ctx.store.apply(&Changes::new(vec![e.clone()], vec![])).map_err(|x| x.to_string())?;
    let events = vec![e.clone()];
    let occ = recur::expand(
        &events,
        tz::resolve(e.start, ctx.zone) - Duration::days(2),
        tz::resolve(e.start, ctx.zone) + Duration::days(2),
        ctx.zone,
    );
    match occ.first() {
        Some(o) if flag(args, "--json") => println!("{}", serde_json::to_string_pretty(&occ_json(ctx, o, &events)).unwrap_or_default()),
        Some(o) => println!("Added:\n{}", line(ctx, o, &events)),
        None => println!("Added “{}” [{}]", e.title, e.id),
    }
    Ok(())
}

fn events_delete(ctx: &mut Ctx, args: &[String]) -> Result<()> {
    let id = words(args, &["--start", "--scope"]).into_iter().next().ok_or("which event? give its id (see events list)")?;
    let e = ctx.store.event(&id).map_err(|x| x.to_string())?.ok_or_else(|| format!("no event with id {id}"))?;
    let group = ctx.store.group(&e.uid).map_err(|x| x.to_string())?;
    let repeating = group.iter().any(|x| x.is_series());
    let scope = match opt(args, "--scope") {
        None | Some("all") => Scope::All,
        Some("this") => Scope::This,
        Some("following") => Scope::Following,
        Some(s) => return Err(format!("--scope is this, following or all (not “{s}”)")),
    };
    let original = match opt(args, "--start") {
        Some(s) => {
            let at =
                DateTime::parse_from_rfc3339(s).map_err(|_| "--start is the instance_start from events list --json")?.with_timezone(&Utc);
            Some(tz::local(at, recur::event_zone(&e, ctx.zone)))
        }
        None if repeating && scope != Scope::All => {
            return Err("a repeating event needs --start (instance_start from events list --json)".into());
        }
        None => e.recurrence_id.or(repeating.then_some(e.start)),
    };
    let target = Target { event_id: id, original };
    let ch = edit::delete(&group, &target, scope, ctx.zone, Utc::now());
    ctx.store.apply(&ch).map_err(|x| x.to_string())?;
    println!("Deleted “{}” ({})", e.title, scope.label().to_lowercase());
    Ok(())
}

fn search(ctx: &Ctx, args: &[String]) -> Result<()> {
    let q = words(args, &[]).join(" ").to_lowercase();
    if q.trim().is_empty() {
        return Err("search for what?".into());
    }
    let events = ctx.shown_events()?;
    let now = Utc::now();
    let mut hits = Vec::new();
    for e in events
        .iter()
        .filter(|e| e.recurrence_id.is_none() && [&e.title, &e.location, &e.description].iter().any(|t| t.to_lowercase().contains(&q)))
    {
        let group: Vec<Event> = events.iter().filter(|x| x.uid == e.uid).cloned().collect();
        let next = recur::expand(&group, now - Duration::days(1), now + Duration::days(730), ctx.zone).into_iter().find(|o| o.end >= now);
        hits.extend(next.or_else(|| recur::expand(&group, now - Duration::days(3650), now, ctx.zone).pop()));
    }
    hits.sort_by_key(|o| o.start);
    print_list(ctx, &hits, &events, flag(args, "--json"));
    Ok(())
}

fn calendars(ctx: &Ctx, args: &[String]) -> Result<()> {
    let cals = ctx.store.calendars().map_err(|e| e.to_string())?;
    if flag(args, "--json") {
        let v: Vec<Value> = cals
            .iter()
            .map(|c| json!({"id": c.id, "name": c.name, "color": c.color, "shown": c.enabled, "default": c.default, "read_only": c.read_only, "account": c.account}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        for c in cals {
            let mut notes = vec![];
            if c.default {
                notes.push("default");
            }
            if !c.enabled {
                notes.push("hidden");
            }
            if c.read_only {
                notes.push("read-only");
            }
            println!(
                "{} {}{}",
                if c.enabled { "●" } else { "○" },
                c.name,
                if notes.is_empty() { String::new() } else { format!(" ({})", notes.join(", ")) }
            );
        }
    }
    Ok(())
}

/// Waybar / Omarchy bar JSON: next event with a countdown.
pub fn bar(db: &Path) -> Result<()> {
    let ctx = Ctx::open(db)?;
    if !ctx.settings.show_in_bar {
        println!("{}", json!({"text": "", "tooltip": "", "class": "off"}));
        return Ok(());
    }
    let now = Utc::now();
    let events = ctx.shown_events()?;
    let occs = recur::expand(&events, now - Duration::hours(12), now + Duration::days(2), ctx.zone);
    let timed: Vec<&Occurrence> = occs.iter().filter(|o| !o.all_day && o.end > now).collect();
    let title = |o: &Occurrence| events.iter().find(|e| e.id == o.event_id).map(|e| e.title.clone()).unwrap_or_default();
    let h24 = ctx.settings.clock_24h;
    let (text, class) = match timed.iter().min_by_key(|o| o.start) {
        Some(o) if o.start <= now => (format!("󰃭 {} · ends {}", title(o), crate::fmt::countdown((o.end - now).num_minutes())), "now"),
        Some(o) => {
            let mins = (o.start - now).num_minutes();
            let class = if mins <= 15 { "soon" } else { "later" };
            let when = if mins < 1440 { crate::fmt::countdown(mins) } else { format!("{}", tz::local(o.start, ctx.zone).format("%a")) };
            (format!("󰃭 {} {when}", title(o)), class)
        }
        None => (String::new(), "none"),
    };
    let today = ctx.today();
    let mut tip: Vec<String> = occs
        .iter()
        .filter(|o| o.on_day(today, ctx.zone) && (o.all_day || o.end > now))
        .map(|o| match o.days {
            Some(_) => format!("all day  {}", title(o)),
            None => format!("{}  {}", crate::fmt::time_label(tz::local(o.start, ctx.zone).time(), h24), title(o)),
        })
        .collect();
    if tip.is_empty() {
        tip.push("Nothing else today".into());
    }
    println!("{}", json!({"text": text, "tooltip": tip.join("\n"), "class": class, "alt": class}));
    Ok(())
}

/// Imports a `.ics` file or subscribes to a link. Returns what happened.
pub fn import(db: &Path, args: &[String]) -> Result<String> {
    let what = words(args, &[]).into_iter().next().ok_or("import what? a .ics file or a webcal:// link")?;
    let mut ctx = Ctx::open(db)?;
    if crate::webcal::is_link(&what) {
        crate::webcal::subscribe(&mut ctx.store, &what, ctx.zone)
    } else {
        let p = what.strip_prefix("file://").unwrap_or(&what);
        crate::webcal::import_file(&mut ctx.store, Path::new(p), ctx.zone)
    }
}

fn events(db: &Path, args: &[String]) -> Result<()> {
    let (sub, rest) = args.split_first().ok_or("events list, events create or events delete")?;
    let mut ctx = Ctx::open(db)?;
    match sub.as_str() {
        "list" => events_list(&ctx, rest),
        "create" | "add" => events_create(&mut ctx, rest),
        "delete" => events_delete(&mut ctx, rest),
        s => Err(format!("unknown: events {s}. Try events list, create or delete")),
    }
}

/// `calinator sync`: sync every account now and say what happened.
fn sync(db: &Path, args: &[String]) -> Result<()> {
    use crate::sync::runner::{Outcome, summary, sync_all};
    match sync_all(db, None)? {
        Outcome::NoAccounts => println!("No online accounts. Add one in the app (@) or with: calinator accounts add"),
        Outcome::Busy => println!("Another Calinator is syncing right now; try again in a moment."),
        Outcome::Done(results) => {
            if flag(args, "--json") {
                let v: Vec<Value> = results
                    .iter()
                    .map(|r| json!({"account": r.name, "up": r.report.up, "down": r.report.down, "conflicts": r.report.conflicts, "errors": r.report.errors, "notes": r.report.notes}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                for r in &results {
                    println!("{}", summary(r));
                    for n in &r.report.notes {
                        println!("  {n}");
                    }
                }
            }
            if results.iter().any(|r| !r.report.errors.is_empty()) {
                return Err("some accounts did not sync (see above)".into());
            }
        }
    }
    Ok(())
}

/// `calinator accounts [list|add|remove]`.
fn accounts(db: &Path, args: &[String]) -> Result<()> {
    use crate::accounts::{self as acc, Account, ICLOUD_URL, Kind};
    let ctx = Ctx::open(db)?;
    match args.first().map(String::as_str) {
        None | Some("list") | Some("--json") => {
            let list = ctx.store.accounts().map_err(|e| e.to_string())?;
            if flag(args, "--json") {
                let v: Vec<Value> = list
                    .iter()
                    .map(|a| json!({"id": a.id, "kind": a.kind.label(), "name": a.name, "url": a.url, "user": a.username, "last_sync": a.last_sync, "error": a.error}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else if list.is_empty() {
                println!("No online accounts.");
            } else {
                for a in list {
                    let state = a.error.clone().map(|e| format!("⚠ {e}")).unwrap_or_else(|| "●".into());
                    println!("{state} {} ({}) [{}]", a.name, a.kind.label(), a.id);
                }
            }
            Ok(())
        }
        Some("add") => {
            // calinator accounts add caldav URL USER   |   accounts add icloud APPLE_ID
            let w = words(&args[1..], &[]);
            let (kind, url, user) = match w.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["caldav", url, user] => (Kind::CalDav, url.to_string(), user.to_string()),
                ["icloud", user] => (Kind::ICloud, ICLOUD_URL.to_string(), user.to_string()),
                _ => {
                    return Err(
                        "use: accounts add caldav URL USER  or  accounts add icloud APPLE_ID (the password is read from stdin)".into()
                    );
                }
            };
            let mut password = String::new();
            std::io::stdin().read_line(&mut password).map_err(|e| e.to_string())?;
            let password = password.trim().replace(if kind == Kind::ICloud { ' ' } else { '\u{0}' }, "");
            let mut p = crate::sync::caldav::CalDav::new(&url, &user, &password).map_err(|e| e.to_string())?;
            use crate::sync::Provider;
            let n = p.calendars().map_err(|e| e.to_string())?.len();
            let name = if kind == Kind::ICloud {
                format!("iCloud ({user})")
            } else {
                url.split("://").nth(1).unwrap_or(&url).split('/').next().unwrap_or("CalDAV").to_string()
            };
            let a = Account {
                id: crate::model::new_id(),
                kind,
                name: name.clone(),
                url,
                username: user,
                addresses: p.my_addresses().unwrap_or_default(),
                ..Default::default()
            };
            acc::set_secret(&a.id, &password)?;
            if let Some(existing) = ctx.store.add_account(&a).map_err(|e| e.to_string())? {
                acc::delete_secret(&a.id)?;
                return Err(format!("{existing} is already added"));
            }
            println!("Added {name}: {n} calendar{}. Run calinator sync (or open the app).", if n == 1 { "" } else { "s" });
            Ok(())
        }
        Some("remove") => {
            let id = args.get(1).ok_or("which account? give its id (see accounts list)")?;
            let mut store = ctx.store;
            let a: Account = store.accounts().map_err(|e| e.to_string())?.into_iter().find(|a| &a.id == id).ok_or("no such account")?;
            store.delete_account(id).map_err(|e| e.to_string())?;
            acc::delete_secret(id)?;
            println!("Removed {} from this computer (nothing was deleted on the server).", a.name);
            Ok(())
        }
        Some(s) => Err(format!("unknown: accounts {s}. Try list, add or remove")),
    }
}

/// Runs a headless command. `None` when `cmd` is not one.
pub fn run(db: &Path, cmd: &str, args: &[String]) -> Option<Result<()>> {
    Some(match cmd {
        "agenda" => Ctx::open(db).and_then(|c| agenda(&c, args)),
        "events" => events(db, args),
        "search" => Ctx::open(db).and_then(|c| search(&c, args)),
        "calendars" => Ctx::open(db).and_then(|c| calendars(&c, args)),
        "import" => import(db, args).map(|m| println!("{m}")),
        "sync" => sync(db, args),
        "accounts" => accounts(db, args),
        _ => return None,
    })
}
