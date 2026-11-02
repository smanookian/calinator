//! iCalendar (RFC 5545) reading and writing: VEVENTs with zones, all-day
//! dates, RRULE / RDATE / EXDATE, overrides (RECURRENCE-ID), guests and alarms.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

use crate::model::{Attendee, Event, Partstat, Status, new_id};
use crate::{recur, tz};

/// One content line: name, parameters, value.
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
    /// The whole line as read (unfolded), to write it back unchanged.
    pub raw: String,
}

impl Prop {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    /// Parameters as written (`ROLE=CHAIR`, `MEMBER="mailto:a@x"`), quotes kept.
    pub fn raw_params(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut quoted = false;
        let mut start = None;
        for (i, ch) in self.raw.char_indices() {
            match ch {
                '"' => quoted = !quoted,
                ';' | ':' if !quoted => {
                    if let Some(s) = start {
                        out.push(&self.raw[s..i]);
                    }
                    if ch == ':' {
                        break;
                    }
                    start = Some(i + 1);
                }
                _ => {}
            }
        }
        out
    }
}

/// Joins folded lines (a line starting with space or tab continues the last).
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t'))
            && let Some(last) = out.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn parse_line(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside quotes.
    let mut quoted = false;
    let mut split = None;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let i = split?;
    let (head, value) = (&line[..i], &line[i + 1..]);
    let mut parts = Vec::new();
    let mut cur = String::new();
    quoted = false;
    for ch in head.chars() {
        match ch {
            '"' => quoted = !quoted,
            ';' if !quoted => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.remove(0).to_uppercase();
    let params = parts.into_iter().filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_uppercase(), v.to_string()))).collect();
    Some(Prop { name, params, value: value.to_string(), raw: line.to_string() })
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A component: name, its properties and its sub-components.
#[derive(Debug, Clone, Default)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Prop> + 'a {
        self.props.iter().filter(move |p| p.name == name)
    }
}

/// Parses text into its top-level components (normally one VCALENDAR).
pub fn parse(text: &str) -> Vec<Component> {
    let mut stack: Vec<Component> = Vec::new();
    let mut top = Vec::new();
    for line in unfold(text) {
        let Some(p) = parse_line(&line) else { continue };
        match p.name.as_str() {
            "BEGIN" => stack.push(Component { name: p.value.to_uppercase(), ..Default::default() }),
            "END" => {
                if let Some(c) = stack.pop() {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(c),
                        None => top.push(c),
                    }
                }
            }
            _ => {
                if let Some(c) = stack.last_mut() {
                    c.props.push(p);
                }
            }
        }
    }
    top
}

/// A date or date-time value as (wall time in `zone`, is date, zone name).
fn when(p: &Prop, value: &str, user: Tz) -> Option<(NaiveDateTime, bool, String)> {
    let v = value.trim();
    if p.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE")) || (v.len() == 8 && !v.contains('T')) {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((d.and_hms_opt(0, 0, 0)?, true, String::new()));
    }
    if let Some(u) = v.strip_suffix('Z') {
        let t = NaiveDateTime::parse_from_str(u, "%Y%m%dT%H%M%S").ok()?;
        return Some((t, false, "UTC".into()));
    }
    let t = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let zone = p.param("TZID").map(|z| zone_name(z, user)).unwrap_or_default();
    Some((t, false, zone))
}

/// IANA name for a TZID: IANA as is, common Windows names mapped, else the user's zone.
fn zone_name(tzid: &str, user: Tz) -> String {
    let id = tzid.trim_matches('"').trim_start_matches('/');
    if id.parse::<Tz>().is_ok() {
        return id.to_string();
    }
    const WINDOWS: [(&str, &str); 14] = [
        ("Eastern Standard Time", "America/New_York"),
        ("Central Standard Time", "America/Chicago"),
        ("Mountain Standard Time", "America/Denver"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("GMT Standard Time", "Europe/London"),
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("Romance Standard Time", "Europe/Paris"),
        ("Central Europe Standard Time", "Europe/Budapest"),
        ("Arabian Standard Time", "Asia/Dubai"),
        ("India Standard Time", "Asia/Kolkata"),
        ("China Standard Time", "Asia/Shanghai"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
        ("UTC", "UTC"),
    ];
    WINDOWS.iter().find(|(w, _)| w.eq_ignore_ascii_case(id)).map(|(_, i)| i.to_string()).unwrap_or_else(|| user.name().to_string())
}

/// Moves a wall time from one zone to another.
fn convert(t: NaiveDateTime, from: &str, to: &str, user: Tz) -> NaiveDateTime {
    if from == to {
        return t;
    }
    let at = tz::resolve(t, tz::parse(from, user));
    tz::local(at, tz::parse(to, user))
}

/// `-PT10M` → 10 minutes before; `-P1D` → 1440. Positive (after start) → None.
pub fn duration_minutes(s: &str) -> Option<i64> {
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for ch in rest.chars() {
        match ch {
            'T' => in_time = true,
            d if d.is_ascii_digit() => num.push(d),
            u => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (u, in_time) {
                    ('W', _) => n * 10080,
                    ('D', _) => n * 1440,
                    ('H', true) => n * 60,
                    ('M', true) => n,
                    ('S', true) => n / 60,
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { total } else { -total })
}

fn partstat(s: Option<&str>) -> Partstat {
    match s.map(str::to_uppercase).as_deref() {
        Some("ACCEPTED") => Partstat::Accepted,
        Some("DECLINED") => Partstat::Declined,
        Some("TENTATIVE") => Partstat::Tentative,
        _ => Partstat::NeedsAction,
    }
}

fn mail(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix("mailto:").or_else(|| v.strip_prefix("MAILTO:")).unwrap_or(v).to_string()
}

/// Every VEVENT in the text as an event in `calendar`. `me`: the user's
/// email addresses (to find their own answer).
pub fn events(text: &str, calendar: &str, me: &[String], user: Tz) -> Vec<Event> {
    read_vevents(&parse(text), calendar, me, user).into_iter().map(|(e, _)| e).collect()
}

/// Every VEVENT with the event read from it.
fn read_vevents<'a>(tops: &'a [Component], calendar: &str, me: &[String], user: Tz) -> Vec<(Event, &'a Component)> {
    let mut out = Vec::new();
    for top in tops {
        let comps: Vec<&Component> =
            if top.name == "VEVENT" { vec![top] } else { top.children.iter().filter(|c| c.name == "VEVENT").collect() };
        for c in comps {
            if let Some(e) = event(c, calendar, me, user) {
                out.push((e, c));
            }
        }
    }
    // An override names the instance it replaces in its own zone; series
    // instances are matched in the series' zone.
    let zones: Vec<(String, String)> =
        out.iter().map(|(e, _)| e).filter(|e| e.is_series() && !e.all_day).map(|e| (e.uid.clone(), e.tz.clone())).collect();
    for (e, _) in out.iter_mut().filter(|(e, _)| e.recurrence_id.is_some() && !e.all_day) {
        if let Some((_, z)) = zones.iter().find(|(u, _)| *u == e.uid) {
            e.recurrence_id = e.recurrence_id.map(|r| convert(r, &e.tz, z, user));
        }
    }
    out
}

/// One VEVENT as an event.
pub fn event(c: &Component, calendar: &str, me: &[String], user: Tz) -> Option<Event> {
    let ds = c.get("DTSTART")?;
    let (start, all_day, zone) = when(ds, &ds.value, user)?;
    let end = if let Some(de) = c.get("DTEND") {
        let (t, _, z) = when(de, &de.value, user)?;
        if all_day { t } else { convert(t, &z, &zone, user) }
    } else if let Some(d) = c.get("DURATION").and_then(|d| duration_minutes(&d.value)) {
        start + Duration::minutes(-d)
    } else if all_day {
        start + Duration::days(1)
    } else {
        start
    };
    let list = |name: &str| -> Vec<NaiveDateTime> {
        c.all(name)
            .flat_map(|p| {
                p.value
                    .split(',')
                    .filter_map(|v| when(p, v, user))
                    .map(|(t, _, z)| if all_day { t } else { convert(t, &z, &zone, user) })
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let text = |name: &str| c.get(name).map(|p| unescape(&p.value)).unwrap_or_default();
    let mut e = Event {
        id: new_id(),
        uid: c.get("UID").map(|p| p.value.clone()).unwrap_or_else(new_id),
        calendar: calendar.into(),
        title: text("SUMMARY"),
        location: text("LOCATION"),
        description: text("DESCRIPTION"),
        all_day,
        start,
        end: end.max(start),
        tz: zone.clone(),
        rrule: c.get("RRULE").map(|p| p.value.clone()),
        rdates: list("RDATE"),
        exdates: list("EXDATE"),
        recurrence_id: c
            .get("RECURRENCE-ID")
            .and_then(|p| when(p, &p.value, user))
            .map(|(t, _, z)| if all_day { t } else { convert(t, &z, &zone, user) }),
        organizer: c.get("ORGANIZER").map(|p| mail(&p.value)),
        status: match c.get("STATUS").map(|p| p.value.to_uppercase()).as_deref() {
            Some("CANCELLED") => Status::Cancelled,
            Some("TENTATIVE") => Status::Tentative,
            _ => Status::Confirmed,
        },
        sequence: c.get("SEQUENCE").and_then(|p| p.value.trim().parse().ok()).unwrap_or(0),
        updated: c
            .get("LAST-MODIFIED")
            .or_else(|| c.get("DTSTAMP"))
            .and_then(|p| NaiveDateTime::parse_from_str(p.value.trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok())
            .map(|t| DateTime::<Utc>::from_naive_utc_and_offset(t, Utc))
            .unwrap_or_else(Utc::now),
        ..Default::default()
    };
    e.attendees = c
        .all("ATTENDEE")
        .map(|p| {
            let email = mail(&p.value);
            Attendee {
                me: me.iter().any(|m| m.eq_ignore_ascii_case(&email)),
                name: p.param("CN").map(|n| n.trim_matches('"').to_string()).unwrap_or_default(),
                status: partstat(p.param("PARTSTAT")),
                email,
            }
        })
        .collect();
    // Conference properties always hold a call link; URL only when it points at a call service.
    let https = |p: &&Prop| p.value.starts_with("https://");
    e.video = c
        .all("X-GOOGLE-CONFERENCE")
        .chain(c.all("CONFERENCE"))
        .find(https)
        .or_else(|| c.all("URL").filter(https).find(|p| VIDEO_HOSTS.iter().any(|h| p.value.contains(h))))
        .map(|p| p.value.clone());
    e.reminders = c
        .children
        .iter()
        .filter(|a| a.name == "VALARM")
        .filter_map(|a| a.get("TRIGGER"))
        .filter(|t| !t.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END")))
        .filter_map(|t| duration_minutes(&t.value))
        .filter(|m| *m >= 0)
        .collect();
    e.reminders.sort_unstable();
    e.reminders.dedup();
    Some(e)
}

/// The calendar's name from `X-WR-CALNAME`, if any.
pub fn calendar_name(text: &str) -> Option<String> {
    parse(text).iter().find_map(|c| c.get("X-WR-CALNAME").map(|p| unescape(&p.value))).filter(|s| !s.trim().is_empty())
}

/// Hosts of video call services, to tell a call link from any web link.
const VIDEO_HOSTS: [&str; 5] = ["meet.google.com", "zoom.us", "teams.microsoft.com", "whereby.com", "meet.jit.si"];

// ---- writing ----

/// How a time is written: a date, UTC (`Z`), floating, or in a zone (`TZID`).
#[derive(Clone, Copy, PartialEq)]
enum Form {
    Date,
    Utc,
    Floating,
    Zone(Tz),
}

impl Form {
    /// The form of an event's times. Unknown zone names become `user`.
    fn of(e: &Event, user: Tz) -> Form {
        if e.all_day {
            return Form::Date;
        }
        if e.tz.is_empty() {
            return Form::Floating;
        }
        match tz::parse(&e.tz, user) {
            Tz::UTC => Form::Utc,
            z => Form::Zone(z),
        }
    }

    /// A date-time property (`DTSTART`, `EXDATE`…) holding `times`.
    fn prop(self, name: &str, times: &[NaiveDateTime]) -> String {
        let (params, format) = match self {
            Form::Date => (";VALUE=DATE".to_string(), "%Y%m%d"),
            Form::Utc => (String::new(), "%Y%m%dT%H%M%SZ"),
            Form::Floating => (String::new(), "%Y%m%dT%H%M%S"),
            Form::Zone(z) => (format!(";TZID={}", z.name()), "%Y%m%dT%H%M%S"),
        };
        let values: Vec<String> = times.iter().map(|t| t.format(format).to_string()).collect();
        format!("{name}{params}:{}", values.join(","))
    }
}

/// Events sharing one UID as an iCalendar object: the series (or single
/// event) first, then its changed instances. `user` stands in for unknown
/// zone names. With `notify` false, guests carry `SCHEDULE-AGENT=CLIENT`
/// so servers do not email them.
pub fn write(events: &[Event], user: Tz, notify: bool) -> String {
    write_merged(events, None, user, notify)
}

/// Like [`write`], keeping what Calinator does not edit from `base` (the
/// object as the server has it): unknown properties and parameters of each
/// VEVENT (matched by instance), calendar-level properties and zone
/// definitions we do not write ourselves. Alarms are replaced by ours;
/// base VEVENTs for instances not in `events` are dropped.
pub fn write_merged(events: &[Event], base: Option<&str>, user: Tz, notify: bool) -> String {
    let tops = base.map(parse).unwrap_or_default();
    let calendar = tops.iter().find(|c| c.name == "VCALENDAR");
    let olds = read_vevents(&tops, "", &[], user);

    let mut rows: Vec<&Event> = events.iter().collect();
    rows.sort_by_key(|e| e.recurrence_id);
    let mut out = String::new();
    for line in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Calinator//EN", "CALSCALE:GREGORIAN"] {
        fold(line, &mut out);
    }
    if let Some(c) = calendar {
        for p in c.props.iter().filter(|p| !matches!(p.name.as_str(), "VERSION" | "PRODID" | "CALSCALE" | "METHOD")) {
            fold(&p.raw, &mut out);
        }
    }
    let mut zones: Vec<Tz> = Vec::new();
    for e in &rows {
        if let Form::Zone(z) = Form::of(e, user)
            && !zones.contains(&z)
        {
            zones.push(z);
        }
    }
    if !zones.is_empty() {
        let (from, to) = years(&rows, user);
        for z in &zones {
            vtimezone(*z, from, to, &mut out);
        }
    }
    if let Some(c) = calendar {
        let ours = |k: &Component| k.name == "VTIMEZONE" && k.get("TZID").is_some_and(|t| zones.iter().any(|z| z.name() == t.value));
        for k in c.children.iter().filter(|k| k.name != "VEVENT" && !ours(k)) {
            emit(k, &mut out);
        }
    }
    let now = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    for e in &rows {
        let series = rows.iter().find(|s| s.recurrence_id.is_none() && s.uid == e.uid).copied();
        let old = olds.iter().find(|(o, _)| o.uid == e.uid && o.recurrence_id == e.recurrence_id);
        vevent(e, series, old.map(|(o, c)| (o, *c)), &now, user, notify, &mut out);
    }
    fold("END:VCALENDAR", &mut out);
    out
}

/// A call link in a `URL` property (other URLs are the event's web page).
fn video_url(v: &str) -> bool {
    v.starts_with("https://") && VIDEO_HOSTS.iter().any(|h| v.contains(h))
}

/// Properties Calinator reads, and so writes itself.
fn modeled(p: &Prop) -> bool {
    match p.name.as_str() {
        "UID"
        | "DTSTAMP"
        | "LAST-MODIFIED"
        | "SEQUENCE"
        | "DTSTART"
        | "DTEND"
        | "DURATION"
        | "RECURRENCE-ID"
        | "RRULE"
        | "RDATE"
        | "EXDATE"
        | "SUMMARY"
        | "LOCATION"
        | "DESCRIPTION"
        | "STATUS"
        | "ORGANIZER"
        | "ATTENDEE"
        | "X-GOOGLE-CONFERENCE" => true,
        "CONFERENCE" => p.value.starts_with("https://"),
        "URL" => video_url(&p.value),
        _ => false,
    }
}

/// Parameters of a base property as written, without the named ones, each with its `;`.
fn kept_params(p: Option<&Prop>, drop: &[&str]) -> String {
    let Some(p) = p else { return String::new() };
    p.raw_params()
        .into_iter()
        .filter(|s| !drop.iter().any(|d| s.split('=').next().is_some_and(|k| k.trim().eq_ignore_ascii_case(d))))
        .map(|s| format!(";{s}"))
        .collect()
}

/// One VEVENT. `old`: the same instance in the server's copy, as read and as written.
fn vevent(e: &Event, series: Option<&Event>, old: Option<(&Event, &Component)>, now: &str, user: Tz, notify: bool, out: &mut String) {
    let form = Form::of(e, user);
    let base = old.map(|(_, c)| c);
    // An edited description makes the server's HTML copy wrong.
    let stale_html = old.is_some_and(|(o, _)| o.description != e.description);
    let kept: Vec<&Prop> =
        base.map(|c| c.props.iter().filter(|p| !modeled(p) && !(stale_html && p.name == "X-ALT-DESC")).collect()).unwrap_or_default();
    let mut line = |s: &str| fold(s, out);
    line("BEGIN:VEVENT");
    line(&format!("UID:{}", e.uid));
    line(&format!("DTSTAMP:{now}"));
    if e.updated.timestamp() > 0 {
        line(&format!("LAST-MODIFIED:{}", e.updated.format("%Y%m%dT%H%M%SZ")));
    }
    line(&format!("SEQUENCE:{}", e.sequence));
    line(&form.prop("DTSTART", &[e.start]));
    line(&form.prop("DTEND", &[e.end.max(e.start)]));
    match e.recurrence_id {
        // An override names the instance in the series' own form and zone.
        Some(r) => line(&series.map_or(form, |s| Form::of(s, user)).prop("RECURRENCE-ID", &[r])),
        None => {
            if let Some(r) = &e.rrule {
                line(&format!("RRULE:{r}"));
            }
            if !e.rdates.is_empty() {
                line(&form.prop("RDATE", &e.rdates));
            }
            if !e.exdates.is_empty() {
                line(&form.prop("EXDATE", &e.exdates));
            }
        }
    }
    line(&format!("SUMMARY:{}", escape(&e.title)));
    if !e.location.is_empty() {
        line(&format!("LOCATION:{}", escape(&e.location)));
    }
    if !e.description.is_empty() {
        line(&format!("DESCRIPTION:{}", escape(&e.description)));
    }
    let status = match e.status {
        Status::Confirmed => "CONFIRMED",
        Status::Tentative => "TENTATIVE",
        Status::Cancelled => "CANCELLED",
    };
    line(&format!("STATUS:{status}"));
    // Scheduling parameters are ours to set; the server sets SCHEDULE-STATUS.
    const SCHEDULE: [&str; 2] = ["SCHEDULE-AGENT", "SCHEDULE-STATUS"];
    let agent = if notify { "" } else { ";SCHEDULE-AGENT=CLIENT" };
    let same = |p: &&Prop, email: &str| mail(&p.value).eq_ignore_ascii_case(email);
    if let Some(o) = &e.organizer {
        let was = base.and_then(|c| c.all("ORGANIZER").find(|p| same(p, o)));
        line(&format!("ORGANIZER{}{agent}:{}", kept_params(was, &SCHEDULE), address(o)));
    }
    for a in &e.attendees {
        let was = base.and_then(|c| c.all("ATTENDEE").find(|p| same(p, &a.email)));
        let name = if a.name.is_empty() { was.and_then(|p| p.param("CN")).unwrap_or_default() } else { a.name.as_str() };
        let cn = if name.is_empty() { String::new() } else { format!(";CN={}", param(name)) };
        let partstat = match a.status {
            Partstat::NeedsAction => "NEEDS-ACTION",
            Partstat::Accepted => "ACCEPTED",
            Partstat::Tentative => "TENTATIVE",
            Partstat::Declined => "DECLINED",
        };
        let rest = kept_params(was, &["CN", "PARTSTAT", SCHEDULE[0], SCHEDULE[1]]);
        line(&format!("ATTENDEE{cn}{rest};PARTSTAT={partstat}{agent}:{}", address(&a.email)));
    }
    if let Some(v) = &e.video {
        if v.contains("meet.google.com") {
            line(&format!("X-GOOGLE-CONFERENCE:{v}"));
        } else if !kept.iter().any(|p| p.name == "URL") {
            // URL may appear once; the event's own web page wins.
            line(&format!("URL:{v}"));
        }
        line(&format!("CONFERENCE;VALUE=URI;FEATURE=VIDEO:{v}"));
    }
    for p in &kept {
        line(&p.raw);
    }
    let note = escape(if e.title.is_empty() { "Reminder" } else { &e.title });
    for m in &e.reminders {
        line("BEGIN:VALARM");
        line("ACTION:DISPLAY");
        line(&if *m >= 0 { format!("TRIGGER:-PT{m}M") } else { format!("TRIGGER:PT{}M", -m) });
        line(&format!("DESCRIPTION:{note}"));
        line("END:VALARM");
    }
    for k in base.iter().flat_map(|c| &c.children).filter(|k| k.name != "VALARM") {
        emit(k, out);
    }
    fold("END:VEVENT", out);
}

/// A parsed component written back as read.
fn emit(c: &Component, out: &mut String) {
    fold(&format!("BEGIN:{}", c.name), out);
    for p in &c.props {
        fold(&p.raw, out);
    }
    for k in &c.children {
        emit(k, out);
    }
    fold(&format!("END:{}", c.name), out);
}

/// First and last year the events touch, for their zone definitions. Open
/// series are followed five years past today.
fn years(rows: &[&Event], user: Tz) -> (i32, i32) {
    let horizon = Utc::now() + Duration::days(5 * 366);
    let (mut from, mut to) = (i32::MAX, i32::MIN);
    for e in rows {
        let last = if e.is_series() { recur::instance_starts(e, user, horizon).last().map(|t| *t + (e.end - e.start)) } else { None };
        let times = [e.start, e.end]
            .into_iter()
            .chain(e.rdates.iter().copied())
            .chain(e.exdates.iter().copied())
            .chain(e.recurrence_id)
            .chain(last);
        for t in times {
            from = from.min(t.year());
            to = to.max(t.year());
        }
    }
    (from, to.max(from))
}

/// A VTIMEZONE for `zone` from chrono-tz: one observance per offset change
/// from the year before `from` through `to`, plus the offset in force
/// before the first change (the only one for zones without changes).
fn vtimezone(zone: Tz, from: i32, to: i32, out: &mut String) {
    let jan1 = |y: i32| NaiveDate::from_ymd_opt(y, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).map_or(0, |t| t.and_utc().timestamp());
    let at = |t: i64| zone.offset_from_utc_datetime(&DateTime::from_timestamp(t, 0).unwrap_or_default().naive_utc());
    let total = |t: i64| at(t).fix().local_minus_utc();
    let (start, end) = (jan1(from - 1), jan1(to + 1));

    // Instants (UTC seconds) where the offset changes: steps of 12 hours,
    // then a binary search down to the second.
    let mut changes = Vec::new();
    let (mut t, mut prev) = (start, total(start));
    while t < end {
        let next = (t + 12 * 3600).min(end);
        let off = total(next);
        if off != prev {
            let (mut a, mut b) = (t, next);
            while b - a > 1 {
                let m = a + (b - a) / 2;
                if total(m) == prev { a = m } else { b = m }
            }
            changes.push(b);
            prev = off;
        }
        t = next;
    }

    fold("BEGIN:VTIMEZONE", out);
    fold(&format!("TZID:{}", zone.name()), out);
    let mut observance = |onset: &str, t: i64, before: i32| {
        let offset = at(t);
        let kind = if offset.dst_offset().num_seconds() == 0 { "STANDARD" } else { "DAYLIGHT" };
        fold(&format!("BEGIN:{kind}"), out);
        fold(&format!("DTSTART:{onset}"), out);
        fold(&format!("TZOFFSETFROM:{}", utc_offset(before)), out);
        fold(&format!("TZOFFSETTO:{}", utc_offset(offset.fix().local_minus_utc())), out);
        if let Some(name) = offset.abbreviation() {
            fold(&format!("TZNAME:{name}"), out);
        }
        fold(&format!("END:{kind}"), out);
    };
    if changes.first().is_none_or(|c| *c >= jan1(from)) {
        observance("19700101T000000", start, total(start));
    }
    for c in &changes {
        // The onset is the wall time just before the change.
        let before = total(c - 1);
        let onset = DateTime::from_timestamp(c + i64::from(before), 0).unwrap_or_default().format("%Y%m%dT%H%M%S").to_string();
        observance(&onset, *c, before);
    }
    fold("END:VTIMEZONE", out);
}

/// `-0500`, or `+053730` when seconds are needed.
fn utc_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, a % 3600 / 60, a % 60);
    if s == 0 { format!("{sign}{h:02}{m:02}") } else { format!("{sign}{h:02}{m:02}{s:02}") }
}

/// An email as a `mailto:` address (values that already have a scheme stay).
fn address(v: &str) -> String {
    if v.contains(':') { v.to_string() } else { format!("mailto:{v}") }
}

/// A parameter value: quoted when it holds `:`, `;` or `,`. Quotes and
/// control characters (not allowed) are dropped.
fn param(v: &str) -> String {
    let clean: String = v.chars().filter(|c| *c != '"' && !c.is_control()).collect();
    if clean.contains([':', ';', ',']) { format!("\"{clean}\"") } else { clean }
}

/// Plain text to an RFC 5545 TEXT value.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Writes a content line folded at 75 octets (never inside a character), ending in CRLF.
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Team\r\nBEGIN:VEVENT\r\nUID:abc@x\r\nSUMMARY:Weekly sync\\, all hands\r\n\
DTSTART;TZID=America/New_York:20261102T090000\r\nDTEND;TZID=America/New_York:20261102T100000\r\nRRULE:FREQ=WEEKLY;COUNT=4\r\n\
EXDATE;TZID=America/New_York:20261109T090000\r\nORGANIZER;CN=Dana:mailto:dana@x.com\r\nATTENDEE;CN=\"Me, Myself\";PARTSTAT=NEEDS-ACTION:mailto:me@x.com\r\n\
DESCRIPTION:Line one\\nline two that is long and gets fol\r\n ded here\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:abc@x\r\nRECURRENCE-ID:20261116T140000Z\r\nDTSTART:20261116T160000Z\r\nDTEND:20261116T170000Z\r\nSUMMARY:Moved\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:day@x\r\nDTSTART;VALUE=DATE:20261124\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_series_override_and_all_day() {
        let user: Tz = "Asia/Dubai".parse().unwrap();
        let ev = events(ICS, "c", &["me@x.com".into()], user);
        assert_eq!(ev.len(), 3);
        let s = &ev[0];
        assert_eq!(s.title, "Weekly sync, all hands");
        assert_eq!(s.tz, "America/New_York");
        assert_eq!(s.rrule.as_deref(), Some("FREQ=WEEKLY;COUNT=4"));
        assert_eq!(s.exdates.len(), 1);
        assert_eq!(s.description, "Line one\nline two that is long and gets folded here");
        assert_eq!(s.reminders, vec![15]);
        assert_eq!(s.my_status(), Some(Partstat::NeedsAction));
        assert_eq!(s.attendees[0].name, "Me, Myself");
        // The override's RECURRENCE-ID (14:00 UTC) names the 9:00 New York instance.
        let o = &ev[1];
        assert_eq!(o.recurrence_id.map(|t| t.to_string()).as_deref(), Some("2026-11-16 09:00:00"));
        let h = &ev[2];
        assert!(h.all_day && (h.end - h.start).num_days() == 1);
        assert_eq!(calendar_name(ICS).as_deref(), Some("Team"));
    }

    #[test]
    fn durations() {
        assert_eq!(duration_minutes("-PT10M"), Some(10));
        assert_eq!(duration_minutes("-P1DT2H"), Some(1560));
        assert_eq!(duration_minutes("PT5M"), Some(-5));
        assert_eq!(duration_minutes("-P1W"), Some(10080));
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        dt(s).and_utc()
    }

    /// Writes and reads back; ids and calendar are put back for comparing.
    fn round_trip(input: &[Event], user: Tz) -> (String, Vec<Event>) {
        let text = write(input, user, true);
        let mut back = events(&text, "c", &["me@x.com".into()], user);
        for b in &mut back {
            b.id = input.iter().find(|i| i.recurrence_id == b.recurrence_id).map(|i| i.id.clone()).unwrap_or_default();
        }
        (text, back)
    }

    fn lines(text: &str) -> Vec<&str> {
        text.split("\r\n").filter(|l| !l.is_empty()).collect()
    }

    #[test]
    fn writes_and_reads_back_a_series_with_override() {
        let user: Tz = "Asia/Dubai".parse().unwrap();
        let series = Event {
            id: "s".into(),
            uid: "series@x".into(),
            calendar: "c".into(),
            title: "Café sync; plans, ideas".into(),
            location: "Room 1, floor 2".into(),
            description:
                "Line one\nLine two; with, punctuation \\ and a long tail: ünïcödé ✓ — that goes past seventy-five octets for sure".into(),
            start: dt("2026-10-19 09:00"),
            end: dt("2026-10-19 10:00"),
            tz: "America/New_York".into(),
            rrule: Some("FREQ=WEEKLY;COUNT=6".into()),
            rdates: vec![dt("2026-11-04 15:00")],
            exdates: vec![dt("2026-11-02 09:00")],
            organizer: Some("dana@x.com".into()),
            attendees: vec![
                Attendee { email: "me@x.com".into(), name: "Me".into(), status: Partstat::Accepted, me: true },
                Attendee { email: "bob@x.com".into(), name: "Bob, Jr.; B".into(), status: Partstat::NeedsAction, me: false },
            ],
            reminders: vec![10, 60],
            status: Status::Tentative,
            video: Some("https://example.com/call/123".into()),
            updated: utc("2026-10-01 12:00"),
            sequence: 3,
            ..Default::default()
        };
        // Moved two hours later, written in another zone.
        let moved = Event {
            id: "o".into(),
            uid: "series@x".into(),
            calendar: "c".into(),
            title: "Moved".into(),
            start: dt("2026-11-09 17:00"),
            end: dt("2026-11-09 18:00"),
            tz: "Europe/Berlin".into(),
            recurrence_id: Some(dt("2026-11-09 09:00")),
            updated: utc("2026-10-02 08:30"),
            ..Default::default()
        };
        // Overrides are written after the series, whatever the input order.
        let (text, back) = round_trip(&[moved.clone(), series.clone()], user);
        assert_eq!(back, vec![series.clone(), moved]);
        assert!(text.contains("RECURRENCE-ID;TZID=America/New_York:20261109T090000\r\n"));
        assert!(text.contains("EXDATE;TZID=America/New_York:20261102T090000\r\n"));
        assert!(text.contains("TRIGGER:-PT60M\r\n"));
        assert!(lines(&text).iter().all(|l| l.len() <= 75), "{text}");
        assert!(text.contains("\r\n "), "long lines are folded");
        assert!(!text.contains("SCHEDULE-AGENT"));
        // Every TZID used has a zone definition.
        let cal = &parse(&text)[0];
        let tzids: Vec<&str> =
            cal.children.iter().filter(|c| c.name == "VTIMEZONE").filter_map(|c| c.get("TZID")).map(|p| p.value.as_str()).collect();
        assert_eq!(tzids, vec!["America/New_York", "Europe/Berlin"]);

        let quiet = write(std::slice::from_ref(&series), user, false);
        assert!(quiet.contains("ORGANIZER;SCHEDULE-AGENT=CLIENT:mailto:dana@x.com"));
        let guests = events(&quiet, "c", &["me@x.com".into()], user);
        assert!(guests[0].attendees.iter().all(|a| a.email.ends_with("@x.com")));
        assert_eq!(guests[0].attendees[1].name, "Bob, Jr.; B");
        assert!(
            unfold(&quiet).iter().any(|l| l == "ATTENDEE;CN=\"Bob, Jr.; B\";PARTSTAT=NEEDS-ACTION;SCHEDULE-AGENT=CLIENT:mailto:bob@x.com")
        );
        assert_eq!(param("say \"hi\""), "say hi");
    }

    #[test]
    fn writes_all_day_floating_and_utc() {
        let user: Tz = "Asia/Dubai".parse().unwrap();
        let all_day = Event {
            uid: "day@x".into(),
            calendar: "c".into(),
            title: "Holiday".into(),
            all_day: true,
            start: dt("2026-12-24 00:00"),
            end: dt("2026-12-26 00:00"),
            tz: String::new(),
            rrule: Some("FREQ=YEARLY".into()),
            exdates: vec![dt("2027-12-24 00:00")],
            updated: utc("2026-10-01 12:00"),
            ..Default::default()
        };
        let (text, back) = round_trip(std::slice::from_ref(&all_day), user);
        assert_eq!(back, vec![all_day]);
        assert!(text.contains("DTSTART;VALUE=DATE:20261224\r\nDTEND;VALUE=DATE:20261226\r\n"));
        assert!(text.contains("EXDATE;VALUE=DATE:20271224\r\n"));
        assert!(!text.contains("VTIMEZONE"));

        let floating = Event {
            uid: "float@x".into(),
            calendar: "c".into(),
            title: "Wake up".into(),
            start: dt("2026-11-01 07:00"),
            end: dt("2026-11-01 07:30"),
            tz: String::new(),
            updated: utc("2026-10-01 12:00"),
            ..Default::default()
        };
        let (text, back) = round_trip(std::slice::from_ref(&floating), user);
        assert_eq!(back, vec![floating]);
        assert!(text.contains("DTSTART:20261101T070000\r\n"));

        let in_utc = Event {
            uid: "utc@x".into(),
            calendar: "c".into(),
            title: "Deploy".into(),
            start: dt("2026-11-01 07:00"),
            end: dt("2026-11-01 08:00"),
            tz: "UTC".into(),
            reminders: vec![5],
            video: Some("https://meet.google.com/abc-defg-hij".into()),
            updated: utc("2026-10-01 12:00"),
            ..Default::default()
        };
        let (text, back) = round_trip(std::slice::from_ref(&in_utc), user);
        assert_eq!(back, vec![in_utc]);
        assert!(text.contains("DTSTART:20261101T070000Z\r\n"));
        assert!(text.contains("X-GOOGLE-CONFERENCE:https://meet.google.com/abc-defg-hij\r\n"));
    }

    #[test]
    fn folds_without_splitting_characters() {
        let mut out = String::new();
        fold(&format!("SUMMARY:{}", "é".repeat(60)), &mut out);
        let parts = lines(&out);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|l| l.len() <= 75));
        assert_eq!(unfold(&out)[0], format!("SUMMARY:{}", "é".repeat(60)));
    }

    /// The zone definition for `tzid` in a written object.
    fn zone_def(tzid: &str, start: &str) -> Component {
        let e = Event { uid: "z".into(), start: dt(start), end: dt(start), tz: tzid.into(), ..Default::default() };
        let text = write(&[e], Tz::UTC, true);
        parse(&text)[0].children.iter().find(|c| c.name == "VTIMEZONE" && c.get("TZID").is_some_and(|p| p.value == tzid)).unwrap().clone()
    }

    #[test]
    fn zone_definitions_have_real_offsets() {
        let ny = zone_def("America/New_York", "2026-06-01 09:00");
        let find = |kind: &str, start: &str| {
            ny.children.iter().find(|c| c.name == kind && c.get("DTSTART").is_some_and(|p| p.value == start)).unwrap().clone()
        };
        let fall = find("STANDARD", "20261101T020000");
        assert_eq!(fall.get("TZOFFSETFROM").unwrap().value, "-0400");
        assert_eq!(fall.get("TZOFFSETTO").unwrap().value, "-0500");
        assert_eq!(fall.get("TZNAME").unwrap().value, "EST");
        let spring = find("DAYLIGHT", "20260308T020000");
        assert_eq!(spring.get("TZOFFSETFROM").unwrap().value, "-0500");
        assert_eq!(spring.get("TZOFFSETTO").unwrap().value, "-0400");

        let dubai = zone_def("Asia/Dubai", "2026-06-01 09:00");
        assert_eq!(dubai.children.len(), 1);
        let only = &dubai.children[0];
        assert_eq!(only.name, "STANDARD");
        assert_eq!(only.get("TZOFFSETFROM").unwrap().value, "+0400");
        assert_eq!(only.get("TZOFFSETTO").unwrap().value, "+0400");
    }

    const PHONE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Phone//EN\r\nMETHOD:PUBLISH\r\nX-WR-CALNAME:Home\r\n\
BEGIN:VTIMEZONE\r\nTZID:Custom Zone\r\nBEGIN:STANDARD\r\nDTSTART:19700101T000000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:m@x\r\nDTSTAMP:20261001T000000Z\r\nDTSTART;TZID=America/New_York:20261102T090000\r\nDTEND;TZID=America/New_York:20261102T100000\r\n\
RRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Sync\r\nCATEGORIES:Work,Team\r\nX-FOO;X-BAR=1:keep me\r\nATTACH;FMTTYPE=application/pdf:https://x.example/agenda.pdf\r\n\
TRANSP:TRANSPARENT\r\nURL;VALUE=URI:https://x.example/sync\r\nORGANIZER;CN=\"Dana, D\":mailto:dana@x.com\r\n\
ATTENDEE;CN=Bob;ROLE=OPT-PARTICIPANT;RSVP=TRUE;PARTSTAT=ACCEPTED;SCHEDULE-STATUS=2.0:mailto:bob@x.com\r\n\
ATTENDEE;CN=Gone;ROLE=REQ-PARTICIPANT:mailto:gone@x.com\r\nDESCRIPTION:hi\r\nX-ALT-DESC;FMTTYPE=text/html:<b>hi</b>\r\n\
BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:m@x\r\nRECURRENCE-ID;TZID=America/New_York:20261109T090000\r\nDTSTART;TZID=America/New_York:20261109T100000\r\n\
DTEND;TZID=America/New_York:20261109T110000\r\nSUMMARY:Sync later\r\nX-KEEP:override\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:m@x\r\nRECURRENCE-ID;TZID=America/New_York:20261116T090000\r\nDTSTART;TZID=America/New_York:20261116T120000\r\n\
DTEND;TZID=America/New_York:20261116T130000\r\nSUMMARY:Dropped\r\nX-DROP:yes\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn merge_keeps_what_we_do_not_edit() {
        let user: Tz = "Asia/Dubai".parse().unwrap();
        let mut evs = events(PHONE, "", &[], user);
        assert_eq!(evs.len(), 3);
        // Edit the title, drop a guest and the last changed instance.
        evs.retain(|e| e.title != "Dropped");
        evs[0].title = "Sync (new)".into();
        evs[0].attendees.retain(|a| a.email != "gone@x.com");
        assert_eq!(evs[0].reminders, vec![5]);

        let text = write_merged(&evs, Some(PHONE), user, true);
        let cal = &parse(&text)[0];
        assert_eq!(cal.get("X-WR-CALNAME").unwrap().value, "Home");
        assert_eq!(cal.get("PRODID").unwrap().value, "-//Calinator//EN");
        assert!(cal.get("METHOD").is_none());
        let tzids: Vec<&str> =
            cal.children.iter().filter(|c| c.name == "VTIMEZONE").map(|c| c.get("TZID").unwrap().value.as_str()).collect();
        assert_eq!(tzids, vec!["America/New_York", "Custom Zone"]);

        let vevents: Vec<&Component> = cal.children.iter().filter(|c| c.name == "VEVENT").collect();
        assert_eq!(vevents.len(), 2);
        let master = vevents[0];
        assert_eq!(master.get("SUMMARY").unwrap().value, "Sync (new)");
        assert_eq!(master.get("CATEGORIES").unwrap().value, "Work,Team");
        assert_eq!(master.get("X-FOO").unwrap().raw, "X-FOO;X-BAR=1:keep me");
        assert_eq!(master.get("ATTACH").unwrap().param("FMTTYPE"), Some("application/pdf"));
        assert_eq!(master.get("TRANSP").unwrap().value, "TRANSPARENT");
        assert_eq!(master.get("URL").unwrap().value, "https://x.example/sync");
        assert_eq!(master.get("X-ALT-DESC").unwrap().value, "<b>hi</b>");
        assert_eq!(master.get("ORGANIZER").unwrap().raw, "ORGANIZER;CN=\"Dana, D\":mailto:dana@x.com");
        let guests: Vec<&Prop> = master.all("ATTENDEE").collect();
        assert_eq!(guests.len(), 1);
        assert_eq!(guests[0].raw, "ATTENDEE;CN=Bob;ROLE=OPT-PARTICIPANT;RSVP=TRUE;PARTSTAT=ACCEPTED:mailto:bob@x.com");
        // Our alarms replace theirs.
        let alarms: Vec<&Component> = master.children.iter().filter(|c| c.name == "VALARM").collect();
        assert_eq!(alarms.len(), 1);
        assert_eq!(alarms[0].get("ACTION").unwrap().value, "DISPLAY");
        let moved = vevents[1];
        assert_eq!(moved.get("X-KEEP").unwrap().value, "override");
        assert!(!text.contains("X-DROP") && !text.contains("Dropped"));

        // Reading it back gives the edited events.
        let mut back = events(&text, "", &[], user);
        for (b, e) in back.iter_mut().zip(&evs) {
            b.id = e.id.clone();
            b.updated = e.updated;
        }
        assert_eq!(back, evs);

        // A new description makes the HTML copy stale; guests are told not to be emailed.
        evs[0].description = "new".into();
        let text = write_merged(&evs, Some(PHONE), user, false);
        assert!(!text.contains("X-ALT-DESC"));
        assert!(unfold(&text).iter().any(|l| l == "ORGANIZER;CN=\"Dana, D\";SCHEDULE-AGENT=CLIENT:mailto:dana@x.com"));
    }
}
