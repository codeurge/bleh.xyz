//! Timestamps.
//!
//! Two representations, both counted from the Unix epoch in UTC:
//!
//! - [`UnixMillis`]: venue (source) time. Hyperliquid reports block and
//!   fill times in milliseconds, so `ts_source_ms` keeps that precision
//!   rather than inventing digits.
//! - [`UnixNanos`]: mesh-side times (`ts_ingest_ns`, `ts_publish_ns`).
//!
//! In JSON, [`UnixMillis`] is a number (safe in JavaScript until the year
//! 287396) and [`UnixNanos`] is a decimal **string**, because nanosecond
//! epoch values exceed 2^53 and would lose precision in JavaScript.
//!
//! This module also parses and formats the RFC 3339 subset that venue files
//! use (`YYYY-MM-DDTHH:MM:SS[.fraction][Z]`, UTC only), without depending on
//! a date-time crate.

use std::fmt;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Milliseconds since the Unix epoch, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixMillis(pub i64);

/// Nanoseconds since the Unix epoch, UTC. Serialized as a decimal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixNanos(pub i64);

/// Error from parsing a timestamp.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid timestamp {input:?}: {reason}")]
pub struct TimeError {
    input: String,
    reason: &'static str,
}

impl TimeError {
    fn new(input: &str, reason: &'static str) -> Self {
        Self {
            input: input.chars().take(64).collect(),
            reason,
        }
    }
}

const NANOS_PER_MILLI: i64 = 1_000_000;
const NANOS_PER_SEC: i64 = 1_000_000_000;

impl UnixMillis {
    /// The current wall-clock time.
    #[must_use]
    pub fn now() -> Self {
        Self(i64::try_from(since_epoch().as_millis()).unwrap_or(i64::MAX))
    }

    /// Converts to nanoseconds, or `None` on overflow.
    #[must_use]
    pub fn to_nanos(self) -> Option<UnixNanos> {
        self.0.checked_mul(NANOS_PER_MILLI).map(UnixNanos)
    }

    /// Parses an RFC 3339 UTC timestamp. Sub-millisecond digits must be zero.
    ///
    /// # Errors
    ///
    /// Fails on malformed input, out-of-range values or sub-millisecond
    /// precision.
    pub fn parse_rfc3339(s: &str) -> Result<Self, TimeError> {
        let ns = UnixNanos::parse_rfc3339(s)?;
        if ns.0 % NANOS_PER_MILLI != 0 {
            return Err(TimeError::new(s, "sub-millisecond precision"));
        }
        Ok(Self(ns.0.div_euclid(NANOS_PER_MILLI)))
    }

    /// Formats as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        UnixNanos(self.0.saturating_mul(NANOS_PER_MILLI)).format_rfc3339(3, true)
    }
}

impl UnixNanos {
    /// The current wall-clock time.
    #[must_use]
    pub fn now() -> Self {
        Self(i64::try_from(since_epoch().as_nanos()).unwrap_or(i64::MAX))
    }

    /// Truncates to milliseconds (floor).
    #[must_use]
    pub fn to_millis(self) -> UnixMillis {
        UnixMillis(self.0.div_euclid(NANOS_PER_MILLI))
    }

    /// Parses `YYYY-MM-DDTHH:MM:SS[.f{1,9}][Z]` as UTC. A missing `Z` is
    /// read as UTC too, because Hyperliquid's node writes naive UTC times.
    ///
    /// # Errors
    ///
    /// Fails on malformed input or a time outside the `i64` nanosecond range
    /// (years 1677 to 2262).
    pub fn parse_rfc3339(s: &str) -> Result<Self, TimeError> {
        let body = s.strip_suffix('Z').unwrap_or(s);
        let b = body.as_bytes();
        if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
            return Err(TimeError::new(s, "expected YYYY-MM-DDTHH:MM:SS"));
        }
        if b[13] != b':' || b[16] != b':' {
            return Err(TimeError::new(s, "expected HH:MM:SS"));
        }
        let num = |range: std::ops::Range<usize>| -> Result<i64, TimeError> {
            let part = &b[range];
            if part.iter().all(u8::is_ascii_digit) {
                Ok(part
                    .iter()
                    .fold(0i64, |acc, &d| acc * 10 + i64::from(d - b'0')))
            } else {
                Err(TimeError::new(s, "non-digit in date or time"))
            }
        };
        let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if !(1..=12).contains(&month) {
            return Err(TimeError::new(s, "month out of range"));
        }
        if day < 1 || day > days_in_month(year, month) {
            return Err(TimeError::new(s, "day out of range"));
        }
        if hour > 23 || minute > 59 || second > 59 {
            return Err(TimeError::new(s, "time out of range"));
        }
        let mut nanos_frac = 0i64;
        if b.len() > 19 {
            if b[19] != b'.' {
                return Err(TimeError::new(s, "unexpected trailing characters"));
            }
            let frac = &b[20..];
            if frac.is_empty() || frac.len() > 9 || !frac.iter().all(u8::is_ascii_digit) {
                return Err(TimeError::new(s, "fraction must be 1 to 9 digits"));
            }
            for i in 0..9 {
                nanos_frac = nanos_frac * 10 + frac.get(i).map_or(0, |&d| i64::from(d - b'0'));
            }
        }
        let days = days_from_civil(year, month, day);
        let secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
        secs.checked_mul(NANOS_PER_SEC)
            .and_then(|n| n.checked_add(nanos_frac))
            .map(Self)
            .ok_or_else(|| TimeError::new(s, "outside the i64 nanosecond range"))
    }

    /// Formats as `YYYY-MM-DDTHH:MM:SS[.f]` with `frac_digits` (0 to 9)
    /// fractional digits (truncated), and a trailing `Z` if `zulu`.
    #[must_use]
    pub fn format_rfc3339(self, frac_digits: u8, zulu: bool) -> String {
        let secs = self.0.div_euclid(NANOS_PER_SEC);
        let nanos = self.0.rem_euclid(NANOS_PER_SEC);
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        let mut out = format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
            sod / 3_600,
            (sod / 60) % 60,
            sod % 60
        );
        let digits = u32::from(frac_digits.min(9));
        if digits > 0 {
            let frac = nanos / 10i64.pow(9 - digits);
            out.push_str(&format!(".{frac:0width$}", width = digits as usize));
        }
        if zulu {
            out.push('Z');
        }
        out
    }
}

impl fmt::Display for UnixNanos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for UnixNanos {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl Serialize for UnixNanos {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for UnixNanos {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        let canonical = !s.is_empty()
            && s.bytes()
                .enumerate()
                .all(|(i, c)| c.is_ascii_digit() || (i == 0 && c == b'-'))
            && !(s.len() > 1 && s.starts_with('0'))
            && !s.starts_with("-0");
        if !canonical {
            return Err(serde::de::Error::custom(
                "nanosecond timestamp must be a canonical integer string",
            ));
        }
        s.parse().map(Self).map_err(serde::de::Error::custom)
    }
}

fn since_epoch() -> std::time::Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_naive_and_zulu() {
        let a = UnixNanos::parse_rfc3339("2026-01-01T00:00:00").unwrap();
        assert_eq!(a.0, 1_767_225_600 * NANOS_PER_SEC);
        let b = UnixNanos::parse_rfc3339("2026-01-01T00:00:00.5Z").unwrap();
        assert_eq!(b.0 - a.0, 500_000_000);
        let c = UnixNanos::parse_rfc3339("1970-01-01T00:00:00.000000001").unwrap();
        assert_eq!(c.0, 1);
        let d = UnixNanos::parse_rfc3339("1969-12-31T23:59:59.999999999Z").unwrap();
        assert_eq!(d.0, -1);
    }

    #[test]
    fn rejects_bad_input() {
        for s in [
            "2026-13-01T00:00:00",
            "2026-02-29T00:00:00",
            "2026-01-01 00:00:00",
            "2026-01-01T24:00:00",
            "2026-01-01T00:00:00.",
            "2026-01-01T00:00:00.1234567890",
            "2026-01-01T00:00:00+01:00",
            "2026-1-01T00:00:00",
            "3000-01-01T00:00:00",
        ] {
            assert!(UnixNanos::parse_rfc3339(s).is_err(), "accepted {s}");
        }
        assert!(UnixNanos::parse_rfc3339("2024-02-29T00:00:00").is_ok());
    }

    #[test]
    fn format_round_trips() {
        for ns in [
            0i64,
            1,
            -1,
            1_767_225_600_123_456_789,
            951_782_400_000_000_000,
        ] {
            let t = UnixNanos(ns);
            let s = t.format_rfc3339(9, true);
            assert_eq!(UnixNanos::parse_rfc3339(&s).unwrap(), t, "{s}");
        }
        assert_eq!(
            UnixMillis(1_767_225_600_123).to_rfc3339(),
            "2026-01-01T00:00:00.123Z"
        );
        assert_eq!(
            UnixNanos(1_767_225_600_123_456_789).format_rfc3339(3, false),
            "2026-01-01T00:00:00.123"
        );
    }

    #[test]
    fn millis_parse_requires_ms_precision() {
        assert_eq!(
            UnixMillis::parse_rfc3339("2026-01-01T00:00:00.123").unwrap(),
            UnixMillis(1_767_225_600_123)
        );
        assert!(UnixMillis::parse_rfc3339("2026-01-01T00:00:00.1234").is_err());
    }

    #[test]
    fn nanos_serde_is_a_string() {
        let t = UnixNanos(1_767_225_600_123_456_789);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"1767225600123456789\"");
        assert_eq!(serde_json::from_str::<UnixNanos>(&json).unwrap(), t);
        for bad in [
            "1767225600123456789",
            "\"01\"",
            "\"+1\"",
            "\"\"",
            "\"-0\"",
            "\"1.5\"",
        ] {
            assert!(serde_json::from_str::<UnixNanos>(bad).is_err(), "{bad}");
        }
        assert_eq!(serde_json::to_string(&UnixMillis(5)).unwrap(), "5");
    }
}
