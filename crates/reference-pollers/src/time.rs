//! Calendar dates and US local time.
//!
//! Sources publish dates in several spellings (`YYYY-MM-DD`,
//! `MM/DD/YYYY`, `YYMMDD`, ISO date-times) and intra-day times in US
//! prevailing local time. [`Date`] is a proleptic Gregorian date;
//! [`local_to_utc_ms`] converts US Central or Eastern prevailing time to
//! UTC with the federal daylight-saving rules (from 2007: second Sunday of
//! March to first Sunday of November; 1987 to 2006: first Sunday of April
//! to last Sunday of October), so repeated and skipped hours are handled
//! from the source's repeated-hour flag.

use std::fmt;

/// Milliseconds per day.
pub const DAY_MS: i64 = 86_400_000;

/// A calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// Year.
    pub y: i32,
    /// Month, 1 to 12.
    pub m: u32,
    /// Day, 1 to 31.
    pub d: u32,
}

const fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in a month.
#[must_use]
pub const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    /// A date, if it exists.
    #[must_use]
    pub fn new(y: i32, m: u32, d: u32) -> Option<Self> {
        ((1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m) && (1800..=9999).contains(&y))
            .then_some(Self { y, m, d })
    }

    /// Days since 1970-01-01 (negative before).
    #[must_use]
    pub fn days(self) -> i64 {
        // Howard Hinnant's days_from_civil.
        let y = i64::from(self.y) - i64::from(self.m <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let m = i64::from(self.m);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(self.d) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// The date `days` after 1970-01-01.
    #[must_use]
    pub fn from_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        Self {
            y: i32::try_from(y).unwrap_or(i32::MAX),
            m: u32::try_from(m).unwrap_or(1),
            d: u32::try_from(d).unwrap_or(1),
        }
    }

    /// The UTC day of a Unix millisecond time.
    #[must_use]
    pub fn of_ms(ms: i64) -> Self {
        Self::from_days(ms.div_euclid(DAY_MS))
    }

    /// Midnight UTC of the date, Unix milliseconds.
    #[must_use]
    pub fn start_ms(self) -> i64 {
        self.days() * DAY_MS
    }

    /// The date `n` days later (or earlier for negative `n`).
    #[must_use]
    pub fn add_days(self, n: i64) -> Self {
        Self::from_days(self.days() + n)
    }

    /// Day of the week, 0 = Monday to 6 = Sunday.
    #[must_use]
    pub fn weekday(self) -> u32 {
        // 1970-01-01 was a Thursday (3).
        u32::try_from((self.days() + 3).rem_euclid(7)).unwrap_or(0)
    }

    /// True on Saturday and Sunday.
    #[must_use]
    pub fn is_weekend(self) -> bool {
        self.weekday() >= 5
    }

    /// The next Monday-to-Friday date after this one.
    #[must_use]
    pub fn next_weekday(self) -> Self {
        let mut d = self.add_days(1);
        while d.is_weekend() {
            d = d.add_days(1);
        }
        d
    }

    /// Parses `YYYY-MM-DD`, optionally followed by a time (`T...` or a
    /// space), as sources write ISO dates and date-times.
    #[must_use]
    pub fn parse_iso(s: &str) -> Option<Self> {
        let s = s.trim();
        let date = s.get(..10)?;
        if s.len() > 10 && !matches!(s.as_bytes()[10], b'T' | b' ') {
            return None;
        }
        let b = date.as_bytes();
        if b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        Self::new(
            date.get(0..4)?.parse().ok()?,
            date.get(5..7)?.parse().ok()?,
            date.get(8..10)?.parse().ok()?,
        )
    }

    /// Parses `M/D/YYYY` or `MM/DD/YYYY`, optionally followed by a time.
    #[must_use]
    pub fn parse_us(s: &str) -> Option<Self> {
        let s = s.trim();
        let date = s.split_whitespace().next()?;
        let mut it = date.split('/');
        let m = it.next()?.parse().ok()?;
        let d = it.next()?.parse().ok()?;
        let y: i32 = it.next()?.parse().ok()?;
        if it.next().is_some() || y < 1000 {
            return None;
        }
        Self::new(y, m, d)
    }

    /// Parses `YYMMDD` (years 00 to 69 are 20xx, 70 to 99 are 19xx).
    #[must_use]
    pub fn parse_yymmdd(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let yy: i32 = s[0..2].parse().ok()?;
        let y = if yy < 70 { 2000 + yy } else { 1900 + yy };
        Self::new(y, s[2..4].parse().ok()?, s[4..6].parse().ok()?)
    }

    /// Parses any of the spellings above.
    #[must_use]
    pub fn parse_any(s: &str) -> Option<Self> {
        Self::parse_iso(s)
            .or_else(|| Self::parse_us(s))
            .or_else(|| Self::parse_yymmdd(s))
    }

    /// `MM/DD/YYYY`, as the USDA DataMart queries take dates.
    #[must_use]
    pub fn us(self) -> String {
        format!("{:02}/{:02}/{:04}", self.m, self.d, self.y)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

/// A US time zone with federal daylight-saving rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsZone {
    /// Central (ERCOT): UTC-6, UTC-5 in daylight time.
    Central,
    /// Eastern (Treasury, the New York Fed, the CFTC): UTC-5, UTC-4.
    Eastern,
}

impl UsZone {
    const fn standard_offset_min(self) -> i64 {
        match self {
            Self::Central => -360,
            Self::Eastern => -300,
        }
    }
}

/// The `n`th (1-based) `weekday` (0 = Monday) of a month; `n = 5` means the
/// last.
fn nth_weekday(y: i32, m: u32, weekday: u32, n: u32) -> Date {
    let first = Date { y, m, d: 1 };
    let shift = (7 + weekday - first.weekday()) % 7;
    let mut d = 1 + shift + 7 * (n - 1);
    while d > days_in_month(y, m) {
        d -= 7;
    }
    Date { y, m, d }
}

/// Daylight-saving start and end dates of a year (transition at 02:00
/// local time), or `None` before 1987.
fn dst_bounds(y: i32) -> Option<(Date, Date)> {
    if y >= 2007 {
        Some((nth_weekday(y, 3, 6, 2), nth_weekday(y, 11, 6, 1)))
    } else if y >= 1987 {
        Some((nth_weekday(y, 4, 6, 1), nth_weekday(y, 10, 6, 5)))
    } else {
        None
    }
}

/// Converts a local wall-clock time (`date`, minutes after local
/// midnight, 0 to 1439) in `zone` to Unix milliseconds.
///
/// `repeated` marks the second occurrence of the hour repeated when
/// daylight time ends (standard time); without it the first (daylight)
/// occurrence is meant. A time in the hour skipped when daylight time
/// starts is read as daylight time.
#[must_use]
pub fn local_to_utc_ms(date: Date, minutes: i64, zone: UsZone, repeated: bool) -> i64 {
    let dst = match dst_bounds(date.y) {
        None => false,
        Some((start, end)) => {
            if date > start && date < end {
                true
            } else if date == start {
                minutes >= 120
            } else if date == end {
                minutes < 60 || (minutes < 120 && !repeated)
            } else {
                false
            }
        }
    };
    let offset = zone.standard_offset_min() + if dst { 60 } else { 0 };
    date.start_ms() + (minutes - offset) * 60_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-719_468_i64, -1, 0, 1, 10_957, 20_000, 2_932_896] {
            assert_eq!(Date::from_days(days).days(), days);
        }
        assert_eq!(Date::new(1970, 1, 1).unwrap().days(), 0);
        assert_eq!(Date::new(2026, 11, 12).unwrap().weekday(), 3);
        assert_eq!(Date::of_ms(-1).to_string(), "1969-12-31");
        assert!(Date::new(2025, 2, 29).is_none());
        assert_eq!(
            Date::new(2026, 11, 13).unwrap().next_weekday().to_string(),
            "2026-11-16"
        );
    }

    #[test]
    fn spellings() {
        let d = Date::new(2026, 3, 9).unwrap();
        for s in [
            "2026-03-09",
            "2026-03-09T00:00:00",
            "3/9/2026",
            "03/09/2026 12:00:00 AM",
            "260309",
        ] {
            assert_eq!(Date::parse_any(s), Some(d), "{s}");
        }
        assert_eq!(Date::parse_yymmdd("861231").unwrap().y, 1986);
        assert!(Date::parse_any("2026-3-9").is_none());
        assert!(Date::parse_any("13/01/2026").is_none());
        assert!(Date::parse_any("2026-03-09X").is_none());
        assert_eq!(d.us(), "03/09/2026");
    }

    #[test]
    fn dst_transitions() {
        let c = UsZone::Central;
        // 2026: DST from 8 March to 1 November.
        let start = Date::new(2026, 3, 8).unwrap();
        let end = Date::new(2026, 11, 1).unwrap();
        // 01:00 CST on the start day is 07:00 UTC; 03:00 CDT is 08:00 UTC.
        assert_eq!(
            local_to_utc_ms(start, 60, c, false),
            start.start_ms() + 7 * 3_600_000
        );
        assert_eq!(
            local_to_utc_ms(start, 180, c, false),
            start.start_ms() + 8 * 3_600_000
        );
        // 01:00 on the end day: first (CDT) is 06:00 UTC, repeated (CST) 07:00.
        assert_eq!(
            local_to_utc_ms(end, 60, c, false),
            end.start_ms() + 6 * 3_600_000
        );
        assert_eq!(
            local_to_utc_ms(end, 60, c, true),
            end.start_ms() + 7 * 3_600_000
        );
        assert_eq!(
            local_to_utc_ms(end, 120, c, false),
            end.start_ms() + 8 * 3_600_000
        );
        // Winter and summer days.
        let jan = Date::new(2026, 1, 15).unwrap();
        assert_eq!(
            local_to_utc_ms(jan, 0, UsZone::Eastern, false),
            jan.start_ms() + 5 * 3_600_000
        );
        let jul = Date::new(2026, 7, 15).unwrap();
        assert_eq!(
            local_to_utc_ms(jul, 0, UsZone::Eastern, false),
            jul.start_ms() + 4 * 3_600_000
        );
        // Pre-2007 rules: 2005 DST began 3 April.
        let d = Date::new(2005, 3, 20).unwrap();
        assert_eq!(
            local_to_utc_ms(d, 0, c, false),
            d.start_ms() + 6 * 3_600_000
        );
        let d = Date::new(2005, 4, 4).unwrap();
        assert_eq!(
            local_to_utc_ms(d, 0, c, false),
            d.start_ms() + 5 * 3_600_000
        );
    }
}
