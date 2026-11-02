//! Time zones: the user's zone, and wall-clock times that survive DST.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// The computer's zone: `$TZ`, else `/etc/localtime`, else UTC.
pub fn system() -> Tz {
    if let Ok(v) = std::env::var("TZ")
        && let Ok(t) = v.trim_start_matches(':').parse()
    {
        return t;
    }
    if let Ok(p) = std::fs::read_link("/etc/localtime") {
        let s = p.to_string_lossy();
        if let Some(i) = s.find("zoneinfo/")
            && let Ok(t) = s[i + 9..].parse()
        {
            return t;
        }
    }
    if let Ok(s) = std::fs::read_to_string("/etc/timezone")
        && let Ok(t) = s.trim().parse()
    {
        return t;
    }
    Tz::UTC
}

/// A zone by IANA name; unknown names fall back to `fallback`.
pub fn parse(name: &str, fallback: Tz) -> Tz {
    name.parse().unwrap_or(fallback)
}

/// A wall-clock time in `tz` as a real instant. In the spring-forward gap the
/// time moves forward by the gap (02:30 → 03:30); in the fall-back overlap the
/// first one wins, like Google and Apple calendars.
pub fn resolve(local: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    // Most gaps are one hour; a few zones use 30 minutes or two hours.
    for shift in [0, 60, 30, 120] {
        if let Some(t) = tz.from_local_datetime(&(local + Duration::minutes(shift))).earliest() {
            return t.with_timezone(&Utc);
        }
    }
    Utc.from_utc_datetime(&local)
}

/// Start and end of a day in `tz` (23 or 25 hours on DST days).
pub fn day_bounds(day: NaiveDate, tz: Tz) -> (DateTime<Utc>, DateTime<Utc>) {
    let midnight = |d: NaiveDate| resolve(d.and_hms_opt(0, 0, 0).expect("midnight"), tz);
    (midnight(day), midnight(day + Duration::days(1)))
}

/// An instant as wall time in `tz`.
pub fn local(t: DateTime<Utc>, tz: Tz) -> NaiveDateTime {
    t.with_timezone(&tz).naive_local()
}

/// Short zone name at that instant (`EST`, `+04`).
pub fn abbrev(t: DateTime<Utc>, tz: Tz) -> String {
    t.with_timezone(&tz).format("%Z").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn gap_moves_forward_and_overlap_takes_first() {
        let ny: Tz = "America/New_York".parse().unwrap();
        assert_eq!(resolve(dt("2026-03-08 02:30"), ny).to_rfc3339(), "2026-03-08T07:30:00+00:00");
        assert_eq!(resolve(dt("2026-11-01 01:30"), ny).to_rfc3339(), "2026-11-01T05:30:00+00:00");
    }

    #[test]
    fn dst_days_have_odd_lengths() {
        let ny: Tz = "America/New_York".parse().unwrap();
        let (a, b) = day_bounds(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(), ny);
        assert_eq!((b - a).num_hours(), 25);
        let (a, b) = day_bounds(NaiveDate::from_ymd_opt(2026, 3, 8).unwrap(), ny);
        assert_eq!((b - a).num_hours(), 23);
    }
}
