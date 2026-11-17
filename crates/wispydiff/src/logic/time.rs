//! Human-readable ages ("5 min ago") and exact times.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local, TimeZone};

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// How long ago `then` was, at `now` (both Unix seconds).
pub fn ago(then: i64, now: i64) -> String {
    let seconds = (now - then).max(0);
    let (n, unit) = match seconds {
        0..45 => return "a few seconds ago".into(),
        45..90 => return "a minute ago".into(),
        90..3_600 => (seconds / 60, "min"),
        3_600..86_400 => (seconds / 3_600, "h"),
        _ => (seconds / 86_400, if seconds < 2 * 86_400 { "day" } else { "days" }),
    };
    format!("{n} {unit} ago")
}

/// `then` (Unix seconds) as an exact time in the user's time zone.
pub fn exact_local(then: i64) -> String {
    exact(then, &Local)
}

fn exact<Tz: TimeZone>(then: i64, zone: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(utc) = DateTime::from_timestamp(then, 0) else { return String::new() };
    utc.with_timezone(zone).format("%a, %-d %b %Y, %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::{ago, exact};

    #[test]
    fn exact_times_are_in_the_given_zone() {
        let berlin_winter = chrono::FixedOffset::east_opt(3_600).unwrap();
        // 2026-11-17 13:32:05 UTC
        assert_eq!(exact(1_794_922_325, &berlin_winter), "Tue, 17 Nov 2026, 14:32:05");
        assert_eq!(exact(1_794_922_325, &chrono::Utc), "Tue, 17 Nov 2026, 13:32:05");
    }

    #[test]
    fn rounds_to_the_largest_unit() {
        assert_eq!(ago(100, 100), "a few seconds ago");
        assert_eq!(ago(100, 90), "a few seconds ago", "clock skew is not the future");
        assert_eq!(ago(0, 44), "a few seconds ago");
        assert_eq!(ago(0, 60), "a minute ago");
        assert_eq!(ago(0, 150), "2 min ago");
        assert_eq!(ago(0, 59 * 60), "59 min ago");
        assert_eq!(ago(0, 3_600), "1 h ago");
        assert_eq!(ago(0, 23 * 3_600 + 3_599), "23 h ago");
        assert_eq!(ago(0, 86_400), "1 day ago");
        assert_eq!(ago(0, 3 * 86_400), "3 days ago");
    }
}
