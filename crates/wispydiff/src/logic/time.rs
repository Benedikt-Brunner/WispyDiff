//! Human-readable ages ("5 min ago").

use std::time::{SystemTime, UNIX_EPOCH};

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

#[cfg(test)]
mod tests {
    use super::ago;

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
