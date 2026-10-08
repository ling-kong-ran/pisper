//! The two `Date` operations the JSONL session layer uses, hand-rolled over
//! Hinnant's civil-date algorithms (the in-repo precedent is
//! `src/ai/retry.rs::days_from_civil`):
//! - [`parse_iso8601_utc`] — upstream `Date.parse(timestamp)` restricted to
//!   the ISO-8601 UTC forms `Date.prototype.toISOString` emits (optionally
//!   with fractional seconds and/or no time part). Other inputs return
//!   `None` (upstream `NaN`).
//! - [`format_iso8601_utc`] — upstream `new Date(ms).toISOString()`
//!   (`YYYY-MM-DDTHH:MM:SS.sssZ`).
//!
//! Disclosed substitution: no `chrono`/`time` dependency; only UTC forms are
//! supported because every fixture and producer in the layer writes
//! `toISOString` output.

/// Days since 1970-01-01 (Hinnant `days_from_civil`), `month` 1..=12.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (y, m) = if month <= 2 {
        (year - 1, i64::from(month) + 9)
    } else {
        (year, i64::from(month) - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`] (Hinnant `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Upstream `Date.parse` on the ISO-8601 UTC subset: `YYYY-MM-DD` with an
/// optional `THH:MM[:SS[.fraction]]` and mandatory `Z` when a time is
/// present. Fractional seconds beyond milliseconds are truncated (like
/// `Date.parse` precision). Leap seconds `:60` clamp to `:59`-aligned math
/// is not attempted; `:60` returns `None`.
pub fn parse_iso8601_utc(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date_part, time_part) = match text.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (text, None),
    };
    let mut date = date_part.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: u32 = date.next()?.parse().ok()?;
    let day: u32 = date.next()?.parse().ok()?;
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut total_seconds = days_from_civil(year, month, day) * 86_400;
    if let Some(time_part) = time_part {
        if !time_part.ends_with('Z') {
            return None;
        }
        let clock = time_part.trim_end_matches('Z');
        let mut clock = clock.split(':');
        let hour: i64 = clock.next()?.parse().ok()?;
        let minute: i64 = clock.next()?.parse().ok()?;
        let (second, fraction): (i64, i64) = match clock.next() {
            None => (0, 0),
            Some(second_part) => match second_part.split_once('.') {
                None => (second_part.parse().ok()?, 0),
                Some((second_part, fraction_part)) => {
                    let second: i64 = second_part.parse().ok()?;
                    let fraction: i64 = parse_fraction_millis(fraction_part)?;
                    (second, fraction)
                }
            },
        };
        if clock.next().is_some()
            || !(0..24).contains(&hour)
            || !(0..60).contains(&minute)
            // Upstream `Date.parse` accepts :00-:59 (the doc's :60-leap-second
            // rejection stays); the exclusive range silently rejected :59.
            || !(0..=59).contains(&second)
        {
            return None;
        }
        total_seconds += hour * 3_600 + minute * 60 + second;
        return Some(total_seconds * 1_000 + fraction);
    }
    Some(total_seconds * 1_000)
}

/// Fractional seconds "fff..." truncated/padded to milliseconds.
fn parse_fraction_millis(digits: &str) -> Option<i64> {
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let mut millis = String::from(&digits[..digits.len().min(3)]);
    while millis.len() < 3 {
        millis.push('0');
    }
    millis.parse().ok()
}

/// Upstream `new Date(ms).toISOString()`: `YYYY-MM-DDTHH:MM:SS.sssZ` in UTC.
pub fn format_iso8601_utc(milliseconds: i64) -> String {
    let days = milliseconds.div_euclid(86_400_000);
    let millis_of_day = milliseconds.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = millis_of_day / 3_600_000;
    let minute = (millis_of_day % 3_600_000) / 60_000;
    let second = (millis_of_day % 60_000) / 1_000;
    let millis = millis_of_day % 1_000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_to_isostring_shapes() {
        // 2023-11-14T22:13:20Z (NOW = 1_700_000_000_000).
        assert_eq!(
            parse_iso8601_utc("2023-11-14T22:13:20.000Z"),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            parse_iso8601_utc("2023-11-14T22:13:20Z"),
            Some(1_700_000_000_000)
        );
        assert_eq!(
            format_iso8601_utc(1_700_000_000_000),
            "2023-11-14T22:13:20.000Z"
        );
        assert_eq!(parse_iso8601_utc("1970-01-01"), Some(0));
        assert_eq!(parse_iso8601_utc("not-a-date"), None);
        assert_eq!(parse_iso8601_utc("2023-13-01T00:00:00Z"), None);
        // Fractional truncation: .5 => 500 ms.
        assert_eq!(
            parse_iso8601_utc("2023-11-14T22:13:20.5Z"),
            Some(1_700_000_000_500)
        );
        // Pre-epoch and far-future.
        assert_eq!(format_iso8601_utc(-1), "1969-12-31T23:59:59.999Z");
        assert_eq!(
            parse_iso8601_utc(&format_iso8601_utc(-86_400_000)),
            Some(-86_400_000)
        );
    }
}
