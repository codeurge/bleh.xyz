//! Civil dates, contract months and New York local time.
//!
//! Dates are proleptic Gregorian days since 1970-01-01 (H. Hinnant's
//! algorithms). New York local time follows the United States daylight
//! saving rule in force since 2007: daylight time from 02:00 on the second
//! Sunday of March to 02:00 on the first Sunday of November (UTC-4),
//! standard time otherwise (UTC-5). Dates before 2007 are not supported by
//! the method.

use std::fmt;

/// Milliseconds per day.
pub const DAY_MS: i64 = 86_400_000;

/// Day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    /// Monday.
    Mon,
    /// Tuesday.
    Tue,
    /// Wednesday.
    Wed,
    /// Thursday.
    Thu,
    /// Friday.
    Fri,
    /// Saturday.
    Sat,
    /// Sunday.
    Sun,
}

impl Weekday {
    /// 0 for Monday through 6 for Sunday.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Mon => 0,
            Self::Tue => 1,
            Self::Wed => 2,
            Self::Thu => 3,
            Self::Fri => 4,
            Self::Sat => 5,
            Self::Sun => 6,
        }
    }

    const fn from_index(i: i32) -> Self {
        match i.rem_euclid(7) {
            0 => Self::Mon,
            1 => Self::Tue,
            2 => Self::Wed,
            3 => Self::Thu,
            4 => Self::Fri,
            5 => Self::Sat,
            _ => Self::Sun,
        }
    }

    /// True for Saturday and Sunday.
    #[must_use]
    pub const fn is_weekend(self) -> bool {
        matches!(self, Self::Sat | Self::Sun)
    }
}

/// A calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(i32);

const fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days in a month.
#[must_use]
pub const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Date {
    /// A date from year, month and day; `None` if it does not exist or is
    /// out of range.
    #[must_use]
    pub fn from_ymd(y: i32, m: u32, d: u32) -> Option<Self> {
        if !(1..=12).contains(&m) || d == 0 || d > days_in_month(y, m) {
            return None;
        }
        i32::try_from(days_from_civil(i64::from(y), m, d))
            .ok()
            .map(Self)
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub const fn days(self) -> i32 {
        self.0
    }

    /// The date `n` days since 1970-01-01.
    #[must_use]
    pub const fn from_days(n: i32) -> Self {
        Self(n)
    }

    /// Year, month and day.
    #[must_use]
    pub fn ymd(self) -> (i32, u32, u32) {
        let z = i64::from(self.0) + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        // Day and month are small and positive; the year fits for any
        // date built from an i32 day count.
        (
            i32::try_from(y).unwrap_or(i32::MAX),
            u32::try_from(m).unwrap_or(1),
            u32::try_from(d).unwrap_or(1),
        )
    }

    /// Parses `YYYY-MM-DD`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let y: i32 = s.get(0..4)?.parse().ok()?;
        let m: u32 = s.get(5..7)?.parse().ok()?;
        let d: u32 = s.get(8..10)?.parse().ok()?;
        Self::from_ymd(y, m, d)
    }

    /// Day of the week (1970-01-01 was a Thursday).
    #[must_use]
    pub const fn weekday(self) -> Weekday {
        Weekday::from_index(self.0 + 3)
    }

    /// The date `n` days later (earlier for negative `n`).
    #[must_use]
    pub const fn add_days(self, n: i32) -> Self {
        Self(self.0 + n)
    }

    /// Days from `self` to `other` (negative if `other` is earlier).
    #[must_use]
    pub const fn days_until(self, other: Self) -> i32 {
        other.0 - self.0
    }

    /// The same day `n` months later, clamped to the end of the month.
    #[must_use]
    pub fn add_months(self, n: i32) -> Self {
        let (y, m, d) = self.ymd();
        let ym = YearMonth::new(y, m).plus(n);
        let d = d.min(days_in_month(ym.year, ym.month));
        Self::from_ymd(ym.year, ym.month, d).unwrap_or(self)
    }

    /// The contract month the date falls in.
    #[must_use]
    pub fn year_month(self) -> YearMonth {
        let (y, m, _) = self.ymd();
        YearMonth::new(y, m)
    }

    /// Midnight UTC at the start of the date, in milliseconds.
    #[must_use]
    pub fn utc_midnight_ms(self) -> i64 {
        i64::from(self.0) * DAY_MS
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

/// A contract month in the neutral notation `YYYY-MM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    /// Year.
    pub year: i32,
    /// Month, 1 to 12.
    pub month: u32,
}

impl YearMonth {
    /// A year and month; the month is clamped into 1..=12.
    #[must_use]
    pub fn new(year: i32, month: u32) -> Self {
        Self {
            year,
            month: month.clamp(1, 12),
        }
    }

    /// Parses `YYYY-MM`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 7 || b[4] != b'-' {
            return None;
        }
        let year: i32 = s.get(0..4)?.parse().ok()?;
        let month: u32 = s.get(5..7)?.parse().ok()?;
        (1..=12).contains(&month).then_some(Self { year, month })
    }

    /// `n` months later (earlier for negative `n`).
    #[must_use]
    pub fn plus(self, n: i32) -> Self {
        // month is 1..=12, so the cast is exact.
        let idx = self.year * 12 + i32::try_from(self.month).unwrap_or(1) - 1 + n;
        Self {
            year: idx.div_euclid(12),
            month: u32::try_from(idx.rem_euclid(12) + 1).unwrap_or(1),
        }
    }

    /// The first day of the month.
    #[must_use]
    pub fn first_day(self) -> Date {
        Date::from_ymd(self.year, self.month, 1).unwrap_or(Date(0))
    }

    /// The last day of the month.
    #[must_use]
    pub fn last_day(self) -> Date {
        Date::from_ymd(self.year, self.month, days_in_month(self.year, self.month))
            .unwrap_or(Date(0))
    }

    /// The `n`-th (1-based) `weekday` of the month.
    #[must_use]
    pub fn nth_weekday(self, n: u32, weekday: Weekday) -> Date {
        let first = self.first_day();
        let offset = (weekday.index() - first.weekday().index()).rem_euclid(7);
        let n = i32::try_from(n.max(1)).unwrap_or(1);
        first.add_days(offset + 7 * (n - 1))
    }

    /// The last `weekday` of the month.
    #[must_use]
    pub fn last_weekday(self, weekday: Weekday) -> Date {
        let last = self.last_day();
        let back = (last.weekday().index() - weekday.index()).rem_euclid(7);
        last.add_days(-back)
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// True if New York observes daylight time at local wall time
/// `minutes` (since midnight) on `date`.
fn ny_daylight(date: Date, minutes: u32) -> bool {
    let (y, _, _) = date.ymd();
    let start = YearMonth::new(y, 3).nth_weekday(2, Weekday::Sun);
    let end = YearMonth::new(y, 11).nth_weekday(1, Weekday::Sun);
    let after_start = date > start || (date == start && minutes >= 120);
    let before_end = date < end || (date == end && minutes < 120);
    after_start && before_end
}

/// UTC milliseconds of New York local wall time `hh:mm` on `date`.
#[must_use]
pub fn ny_to_utc_ms(date: Date, hh: u32, mm: u32) -> i64 {
    let minutes = hh * 60 + mm;
    let offset_h: i64 = if ny_daylight(date, minutes) { 4 } else { 5 };
    date.utc_midnight_ms() + (i64::from(minutes) + offset_h * 60) * 60_000
}

/// The New York local date at UTC instant `ms`.
#[must_use]
pub fn ny_date(ms: i64) -> Date {
    // Try daylight time first; confirm against the rule.
    for offset_h in [4i64, 5] {
        let local = ms - offset_h * 3_600_000;
        let days = local.div_euclid(DAY_MS);
        let minutes = u32::try_from(local.rem_euclid(DAY_MS) / 60_000).unwrap_or(0);
        let date = Date(i32::try_from(days).unwrap_or(0));
        if ny_daylight(date, minutes) == (offset_h == 4) {
            return date;
        }
    }
    Date(i32::try_from((ms - 5 * 3_600_000).div_euclid(DAY_MS)).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }

    #[test]
    fn civil_round_trip_and_weekdays() {
        for n in [-1000, -1, 0, 1, 18_000, 20_454, 30_000] {
            let x = Date(n);
            assert_eq!(Date::parse(&x.to_string()), Some(x));
        }
        assert_eq!(d("1970-01-01").weekday(), Weekday::Thu);
        assert_eq!(d("2026-06-19").weekday(), Weekday::Fri);
        assert_eq!(d("2024-02-29").add_months(12), d("2025-02-28"));
        assert_eq!(d("2026-01-31").add_months(1), d("2026-02-28"));
        assert!(Date::parse("2026-02-29").is_none());
    }

    #[test]
    fn month_helpers() {
        let ym = YearMonth::parse("2026-03").unwrap();
        assert_eq!(ym.nth_weekday(3, Weekday::Fri), d("2026-03-20"));
        assert_eq!(ym.nth_weekday(3, Weekday::Wed), d("2026-03-18"));
        assert_eq!(
            YearMonth::new(2026, 5).last_weekday(Weekday::Mon),
            d("2026-05-25")
        );
        assert_eq!(ym.plus(-3).to_string(), "2025-12");
        assert_eq!(ym.plus(10).to_string(), "2027-01");
        assert!(YearMonth::parse("2026-13").is_none());
    }

    #[test]
    fn new_york_time() {
        // 2026: daylight time from 8 March to 1 November.
        assert_eq!(
            ny_to_utc_ms(d("2026-03-06"), 9, 30),
            d("2026-03-06").utc_midnight_ms() + (14 * 60 + 30) * 60_000
        );
        assert_eq!(
            ny_to_utc_ms(d("2026-03-09"), 9, 30),
            d("2026-03-09").utc_midnight_ms() + (13 * 60 + 30) * 60_000
        );
        assert_eq!(
            ny_to_utc_ms(d("2026-11-02"), 9, 30),
            d("2026-11-02").utc_midnight_ms() + (14 * 60 + 30) * 60_000
        );
        // 03:30 UTC on 2 June is 23:30 on 1 June in New York.
        let ms = d("2026-06-02").utc_midnight_ms() + 3 * 3_600_000 + 1_800_000;
        assert_eq!(ny_date(ms), d("2026-06-01"));
        let ms = d("2026-01-02").utc_midnight_ms() + 6 * 3_600_000;
        assert_eq!(ny_date(ms), d("2026-01-02"));
    }
}
