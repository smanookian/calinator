//! Quick add: "lunch with Sarah tomorrow 1pm" → an event. Understands days
//! (today, tomorrow, fri, next mon, nov 20, 11/20, in 3 days), times (9:30,
//! 3pm, noon, 3pm-4pm, 3-4pm), lengths (for 30m, for 2 hours), "all day" and
//! repeats (every day, every monday, every mon and wed, every 2 weeks, every weekday).

use std::ops::Range;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub date: Option<NaiveDate>,
    pub start: Option<NaiveTime>,
    pub end: Option<NaiveTime>,
    /// Minutes, from "for 30m".
    pub minutes: Option<i64>,
    pub all_day: bool,
    pub rrule: Option<String>,
    /// Byte ranges of the input that were read as date, time or repeat.
    pub parts: Vec<Range<usize>>,
}

struct Tok<'a> {
    w: String,
    raw: &'a str,
    at: usize,
}

fn tokenize(s: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, ch) in s.char_indices().chain(std::iter::once((s.len(), ' '))) {
        match (ch.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(a)) => {
                let raw = &s[a..i];
                let w = raw.trim_end_matches([',', '.']).to_lowercase();
                out.push(Tok { w, raw, at: a });
                start = None;
            }
            _ => {}
        }
    }
    out
}

pub fn weekday(w: &str) -> Option<Weekday> {
    Some(match w {
        "mon" | "monday" | "mondays" => Weekday::Mon,
        "tue" | "tues" | "tuesday" | "tuesdays" => Weekday::Tue,
        "wed" | "wednesday" | "wednesdays" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" | "thursdays" => Weekday::Thu,
        "fri" | "friday" | "fridays" => Weekday::Fri,
        "sat" | "saturday" | "saturdays" => Weekday::Sat,
        "sun" | "sunday" | "sundays" => Weekday::Sun,
        _ => return None,
    })
}

fn month(w: &str) -> Option<u32> {
    const M: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    if w.len() < 3 {
        return None;
    }
    let full = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    M.iter().zip(full).position(|(s, f)| f.starts_with(w) && w.starts_with(s) || w == "sept" && *s == "sep").map(|i| i as u32 + 1)
}

fn day_num(w: &str) -> Option<u32> {
    let n = w.trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th");
    n.parse().ok().filter(|d| (1..=31).contains(d))
}

/// The coming `wd`, today included.
fn coming(today: NaiveDate, wd: Weekday) -> NaiveDate {
    let diff = (wd.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
    today + Duration::days(diff)
}

/// A month/day that hasn't passed (this year, else next).
fn upcoming(today: NaiveDate, m: u32, d: u32) -> Option<NaiveDate> {
    let this = NaiveDate::from_ymd_opt(today.year(), m, d)?;
    if this >= today { Some(this) } else { NaiveDate::from_ymd_opt(today.year() + 1, m, d) }
}

/// A date from the start of `w`: returns tokens used.
fn date(w: &[&str], today: NaiveDate) -> Option<(usize, NaiveDate)> {
    let first = *w.first()?;
    match first {
        "today" | "tonight" => return Some((1, today)),
        "tomorrow" | "tmr" | "tmrw" => return Some((1, today + Duration::days(1))),
        "next" | "this" => {
            let wd = weekday(w.get(1)?)?;
            return Some((2, coming(today + Duration::days(1), wd)));
        }
        "in" => {
            let n: i64 = w.get(1)?.parse().ok()?;
            let days = match *w.get(2)? {
                "day" | "days" => n,
                "week" | "weeks" => n * 7,
                _ => return None,
            };
            return Some((3, today + Duration::days(days)));
        }
        _ => {}
    }
    if let Some(wd) = weekday(first) {
        return Some((1, coming(today, wd)));
    }
    if let Ok(d) = NaiveDate::parse_from_str(first, "%Y-%m-%d") {
        return Some((1, d));
    }
    if let Some((m, d)) = first.split_once('/')
        && let (Ok(m), Ok(d)) = (m.parse(), d.parse())
    {
        return upcoming(today, m, d).map(|x| (1, x));
    }
    if let Some(m) = month(first)
        && let Some(d) = w.get(1).and_then(|x| day_num(x))
    {
        return upcoming(today, m, d).map(|x| (2, x));
    }
    if let Some(d) = day_num(first)
        && let Some(m) = w.get(1).and_then(|x| month(x))
    {
        return upcoming(today, m, d).map(|x| (2, x));
    }
    None
}

/// A clock time in one word: "9", "9:30", "9am", "9:30pm", "21:00", "noon".
/// `loose`: a bare number counts (after "at", or in a range).
fn clock(w: &str, loose: bool) -> Option<(NaiveTime, bool)> {
    match w {
        "noon" | "midday" => return NaiveTime::from_hms_opt(12, 0, 0).map(|t| (t, true)),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0).map(|t| (t, true)),
        _ => {}
    }
    let (body, ampm) = if let Some(b) = w.strip_suffix("am").or_else(|| w.strip_suffix('a')) {
        (b, Some(false))
    } else if let Some(b) = w.strip_suffix("pm").or_else(|| w.strip_suffix('p')) {
        (b, Some(true))
    } else {
        (w, None)
    };
    let (h, m) = match body.split_once(':').or_else(|| body.split_once('.')) {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if ampm.is_some() || loose => (body.parse::<u32>().ok()?, 0),
        _ => return None,
    };
    let h = match ampm {
        Some(pm) if (1..=12).contains(&h) => (h % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        // "at 3" means 3pm; mornings need "am" or 24-hour time.
        None if (1..=6).contains(&h) && !body.contains(':') => h + 12,
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0).map(|t| (t, ampm.is_some()))
}

/// Time or time range from the start of `w`: (tokens used, start, end).
fn times(w: &[&str]) -> Option<(usize, NaiveTime, Option<NaiveTime>)> {
    let (skip, loose) = match w.first() {
        Some(&"at") | Some(&"from") | Some(&"@") => (1, true),
        _ => (0, false),
    };
    let first = *w.get(skip)?;
    // "3pm-4pm", "3-4pm", "9:30-10".
    if let Some((a, b)) = first.split_once(['-', '–'])
        && let Some((end, end_pm)) = clock(b, true)
    {
        let (mut start, start_pm) = clock(a, true)?;
        // "3-4pm": the start takes the end's half of the day.
        if !start_pm && end_pm && start.hour() < 12 && end >= NaiveTime::from_hms_opt(12, 0, 0)? {
            let pm = start + Duration::hours(12);
            if pm < end {
                start = pm;
            }
        }
        return Some((skip + 1, start, Some(end)));
    }
    let (start, start_pm) = clock(first, loose)?;
    // "3pm to 4pm", "3 - 4pm", "from 3 to 4".
    if let (Some(sep), Some(b)) = (w.get(skip + 1), w.get(skip + 2))
        && matches!(*sep, "to" | "-" | "–" | "until" | "till")
        && let Some((end, end_pm)) = clock(b, true)
    {
        let mut s = start;
        if !start_pm && end_pm && end >= NaiveTime::from_hms_opt(12, 0, 0)? && s + Duration::hours(12) < end {
            s += Duration::hours(12);
        }
        return Some((skip + 3, s, Some(end)));
    }
    Some((skip + 1, start, None))
}

/// A date typed in a form field ("tomorrow", "nov 20", "2026-11-20").
pub fn parse_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    let lower = text.trim().to_lowercase();
    let w: Vec<&str> = lower.split_whitespace().map(|x| x.trim_end_matches(',')).collect();
    let (n, d) = date(&w, today)?;
    // "nov 20 2027": a year after the date.
    match w.get(n).and_then(|y| y.parse::<i32>().ok()) {
        Some(y) if n + 1 == w.len() => NaiveDate::from_ymd_opt(y, d.month(), d.day()),
        None if n == w.len() => Some(d),
        _ => None,
    }
}

/// A time typed in a form field ("9:30", "3pm", "15:00").
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    clock(&text.trim().to_lowercase(), true).map(|(t, _)| t)
}

/// A repeat typed in a form field: empty → no repeat; "every week",
/// "every mon and wed", or a raw RRULE ("FREQ=…"). `None` when unreadable.
pub fn parse_repeat(text: &str) -> Option<Option<String>> {
    let t = text.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("never") || t.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    if t.to_uppercase().starts_with("FREQ=") {
        return Some(Some(t.to_uppercase()));
    }
    let lower = t.to_lowercase();
    let w: Vec<&str> = lower.split_whitespace().map(|x| x.trim_end_matches(',')).collect();
    let w: Vec<&str> = if w.first() == Some(&"every") || matches!(w.first(), Some(&"daily" | &"weekly" | &"monthly" | &"yearly")) {
        w
    } else {
        std::iter::once("every").chain(w).collect()
    };
    let (n, mut rule) = repeat(&w)?;
    // ", 5 times" / "until dec 1" after the repeat.
    let rest = &w[n..];
    match rest {
        [] => {}
        [c, "times"] | ["for", c, "times"] => rule += &format!(";COUNT={}", c.parse::<u32>().ok()?),
        ["until", d @ ..] => {
            let day = parse_date(&d.join(" "), NaiveDate::from_ymd_opt(2000, 1, 1)?)?;
            rule += &format!(";UNTIL={}", day.format("%Y%m%d"));
        }
        _ => return None,
    }
    Some(Some(rule))
}

/// Reminders typed as "10m, 1h, 1d" → minutes before.
pub fn parse_reminders(text: &str) -> Option<Vec<i64>> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let s = s.to_lowercase();
            let i = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            let n: i64 = s[..i].trim().parse().ok()?;
            Some(match s[i..].trim() {
                "" | "m" | "min" | "mins" | "minutes" => n,
                "h" | "hr" | "hour" | "hours" => n * 60,
                "d" | "day" | "days" => n * 1440,
                "w" | "week" | "weeks" => n * 10080,
                _ => return None,
            })
        })
        .collect()
}

/// Minutes before as "10m", "1h", "2d".
pub fn reminder_text(m: i64) -> String {
    match m {
        0 => "0m".into(),
        m if m % 10080 == 0 => format!("{}w", m / 10080),
        m if m % 1440 == 0 => format!("{}d", m / 1440),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// "for 30m", "for 2 hours", "for 1.5h".
fn length(w: &[&str]) -> Option<(usize, i64)> {
    if w.first() != Some(&"for") {
        return None;
    }
    let a = *w.get(1)?;
    let unit_min = |u: &str| -> Option<f64> {
        Some(match u {
            "m" | "min" | "mins" | "minute" | "minutes" => 1.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 60.0,
            _ => return None,
        })
    };
    if let Some(i) = a.find(|c: char| c.is_alphabetic()) {
        let n: f64 = a[..i].parse().ok()?;
        return Some((2, (n * unit_min(&a[i..])?) as i64));
    }
    let n: f64 = a.parse().ok()?;
    Some((3, (n * unit_min(w.get(2)?)?) as i64))
}

/// "every …": tokens used and the RRULE.
fn repeat(w: &[&str]) -> Option<(usize, String)> {
    let mut i = match w.first() {
        Some(&"every") => 1,
        Some(&"daily") => return Some((1, "FREQ=DAILY".into())),
        Some(&"weekly") => return Some((1, "FREQ=WEEKLY".into())),
        Some(&"monthly") => return Some((1, "FREQ=MONTHLY".into())),
        Some(&"yearly") | Some(&"annually") => return Some((1, "FREQ=YEARLY".into())),
        _ => return None,
    };
    let mut interval = 1;
    if let Some(n) = w.get(i).and_then(|x| x.parse::<u32>().ok()) {
        interval = n;
        i += 1;
    } else if w.get(i) == Some(&"other") {
        interval = 2;
        i += 1;
    }
    let with = |f: &str| if interval > 1 { format!("FREQ={f};INTERVAL={interval}") } else { format!("FREQ={f}") };
    let unit = *w.get(i)?;
    let freq = match unit {
        "day" | "days" => Some("DAILY"),
        "week" | "weeks" => Some("WEEKLY"),
        "month" | "months" => Some("MONTHLY"),
        "year" | "years" => Some("YEARLY"),
        _ => None,
    };
    // "every week on mon, wed" (how repeats are shown) reads like "every mon, wed".
    let on_days = freq == Some("WEEKLY") && w.get(i + 1) == Some(&"on") && w.get(i + 2).and_then(|x| weekday(x)).is_some();
    if let Some(f) = freq.filter(|_| !on_days) {
        return Some((i + 1, with(f)));
    }
    if on_days {
        i += 2;
    }
    if matches!(unit, "weekday" | "weekdays") {
        return Some((i + 1, "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".into()));
    }
    // "every mon and wed", "every tue, thu".
    let mut days = Vec::new();
    while let Some(x) = w.get(i) {
        if let Some(d) = weekday(x) {
            days.push(crate::edit::weekday_code(coming(NaiveDate::from_ymd_opt(2024, 1, 1)?, d)));
            i += 1;
        } else if matches!(*x, "and" | "&") && w.get(i + 1).and_then(|y| weekday(y)).is_some() {
            i += 1;
        } else {
            break;
        }
    }
    if days.is_empty() {
        return None;
    }
    Some((i, format!("{};BYDAY={}", with("WEEKLY"), days.join(","))))
}

pub fn parse(input: &str, now: NaiveDateTime) -> Parsed {
    let toks = tokenize(input);
    let words: Vec<&str> = toks.iter().map(|t| t.w.as_str()).collect();
    let today = now.date();
    let mut p = Parsed::default();
    let mut used = vec![false; toks.len()];
    let mut i = 0;
    while i < toks.len() {
        let rest = &words[i..];
        let mut take = 0;
        if let Some((n, r)) = repeat(rest).filter(|_| p.rrule.is_none()) {
            p.rrule = Some(r);
            take = n;
        } else if rest.len() >= 2 && rest[0] == "all" && rest[1] == "day" {
            p.all_day = true;
            take = 2;
        } else if rest[0] == "allday" || rest[0] == "all-day" {
            p.all_day = true;
            take = 1;
        } else if let Some((n, m)) = length(rest).filter(|_| p.minutes.is_none()) {
            p.minutes = Some(m);
            take = n;
        } else if let Some((n, d)) = date(if rest[0] == "on" { &rest[1..] } else { rest }, today).filter(|_| p.date.is_none()) {
            p.date = Some(d);
            take = n + usize::from(rest[0] == "on");
        } else if let Some((n, s, e)) = times(rest).filter(|_| p.start.is_none()) {
            p.start = Some(s);
            p.end = e;
            take = n;
        }
        if take == 0 {
            i += 1;
            continue;
        }
        for u in used.iter_mut().skip(i).take(take) {
            *u = true;
        }
        let last = &toks[i + take - 1];
        p.parts.push(toks[i].at..last.at + last.raw.len());
        i += take;
    }
    // A repeat on weekdays starts on the first matching day.
    if p.date.is_none()
        && let Some(r) = &p.rrule
        && let Some(days) = r.split(';').find_map(|x| x.strip_prefix("BYDAY="))
        && let Some(first) = days.split(',').next().and_then(|c| weekday(&code_name(c)))
    {
        p.date = Some(coming(today, first));
    }
    let mut title: Vec<&str> = toks.iter().zip(&used).filter(|(_, u)| !**u).map(|(t, _)| t.raw).collect();
    // Drop a dangling "on"/"at" left before a date or time.
    while title.last().is_some_and(|w| matches!(w.to_lowercase().as_str(), "on" | "at" | "from")) {
        title.pop();
    }
    p.title = title.join(" ");
    p
}

fn code_name(code: &str) -> String {
    match code {
        "MO" => "mon",
        "TU" => "tue",
        "WE" => "wed",
        "TH" => "thu",
        "FR" => "fri",
        "SA" => "sat",
        "SU" => "sun",
        _ => "",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> NaiveDateTime {
        // Sunday.
        NaiveDateTime::parse_from_str("2026-11-01 10:00", "%Y-%m-%d %H:%M").unwrap()
    }

    fn d(s: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
    }

    fn t(s: &str) -> Option<NaiveTime> {
        NaiveTime::parse_from_str(s, "%H:%M").ok()
    }

    #[test]
    fn common_phrases() {
        let p = parse("lunch with Sarah tomorrow 1pm", now());
        assert_eq!((p.title.as_str(), p.date, p.start, p.end), ("lunch with Sarah", d("2026-11-02"), t("13:00"), None));

        let p = parse("dentist fri", now());
        assert_eq!((p.title.as_str(), p.date, p.start), ("dentist", d("2026-11-06"), None));

        let p = parse("standup next mon 9:30", now());
        assert_eq!((p.title.as_str(), p.date, p.start), ("standup", d("2026-11-02"), t("09:30")));

        let p = parse("review 3pm-4pm", now());
        assert_eq!((p.start, p.end), (t("15:00"), t("16:00")));
        let p = parse("review 3-4pm on nov 20", now());
        assert_eq!((p.title.as_str(), p.date, p.start, p.end), ("review", d("2026-11-20"), t("15:00"), t("16:00")));
        let p = parse("call at 11am to 12:30pm", now());
        assert_eq!((p.title.as_str(), p.start, p.end), ("call", t("11:00"), t("12:30")));

        let p = parse("gym every monday 7am", now());
        assert_eq!(
            (p.title.as_str(), p.rrule.as_deref(), p.date, p.start),
            ("gym", Some("FREQ=WEEKLY;BYDAY=MO"), d("2026-11-02"), t("07:00"))
        );
        let p = parse("class every tue and thu 6pm for 90 min", now());
        assert_eq!((p.title.as_str(), p.rrule.as_deref(), p.minutes), ("class", Some("FREQ=WEEKLY;BYDAY=TU,TH"), Some(90)));
        let p = parse("trip dec 24 all day", now());
        assert_eq!((p.title.as_str(), p.date, p.all_day), ("trip", d("2026-12-24"), true));
        let p = parse("rent every month", now());
        assert_eq!(p.rrule.as_deref(), Some("FREQ=MONTHLY"));
        let p = parse("1:1 every 2 weeks at 4", now());
        assert_eq!((p.title.as_str(), p.rrule.as_deref(), p.start), ("1:1", Some("FREQ=WEEKLY;INTERVAL=2"), t("16:00")));
    }

    #[test]
    fn past_month_day_means_next_year_and_numbers_stay_in_title() {
        let p = parse("party jan 5", now());
        assert_eq!(p.date, d("2027-01-05"));
        let p = parse("buy 3 apples", now());
        assert_eq!((p.title.as_str(), p.start), ("buy 3 apples", None));
    }
}
