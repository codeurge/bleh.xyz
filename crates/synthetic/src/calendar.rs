//! Business-day calendars, contract cycles, last-trading-day and roll
//! rules.
//!
//! These are published rulebook facts and public holiday rules, computed
//! in code; they are not market data (method spec § 5).
//!
//! # Calendars
//!
//! [`CalendarId::UsTrading`] (US exchange trading days, used for equity
//! index, energy and metal months): weekends and New Year's Day, Martin
//! Luther King Jr. Day (third Monday of January), Washington's Birthday
//! (third Monday of February), Good Friday, Memorial Day (last Monday of
//! May), Juneteenth (19 June, from 2022), Independence Day (4 July), Labor
//! Day (first Monday of September), Thanksgiving (fourth Thursday of
//! November) and Christmas (25 December). A fixed-date holiday on a Sunday
//! moves to Monday; on a Saturday it moves to Friday, except New Year's Day,
//! which is then not observed.
//!
//! [`CalendarId::UsGovernmentSecurities`] (overnight-rate fixing days,
//! used for short-rate months and curve tenors): the same weekday holidays
//! plus Columbus Day (second Monday of October) and Veterans Day
//! (11 November). A fixed-date holiday on a Sunday moves to Monday; on a
//! Saturday it is not moved (the Federal Reserve rule). Good Friday is a
//! closure; years in which it is not are configured as openings.
//!
//! Unscheduled closures (for example a national day of mourning) and
//! exceptions are configured per calendar ([`Calendar::with_exceptions`]).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::date::{Date, Weekday, YearMonth};

/// A holiday calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarId {
    /// US exchange trading days.
    UsTrading,
    /// US government securities (overnight-rate fixing) days.
    UsGovernmentSecurities,
}

/// Easter Sunday (Gregorian, anonymous algorithm).
#[must_use]
pub fn easter(year: i32) -> Date {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    Date::from_ymd(
        year,
        u32::try_from(month).unwrap_or(4),
        u32::try_from(day).unwrap_or(1),
    )
    .unwrap_or_else(|| Date::from_days(0))
}

/// Observed date of a fixed-date holiday.
fn observed(date: Date, saturday_to_friday: bool) -> Option<Date> {
    match date.weekday() {
        Weekday::Sun => Some(date.add_days(1)),
        Weekday::Sat if saturday_to_friday => Some(date.add_days(-1)),
        Weekday::Sat => None,
        _ => Some(date),
    }
}

/// The scheduled holidays of `cal` in `year` (observed dates).
#[must_use]
pub fn holidays(cal: CalendarId, year: i32) -> Vec<Date> {
    let ym = |m| YearMonth::new(year, m);
    let fixed = |m, d| Date::from_ymd(year, m, d);
    let sat_to_fri = cal == CalendarId::UsTrading;
    let mut v = Vec::new();
    // New Year's Day is never moved back into the previous year.
    if let Some(d) = fixed(1, 1).and_then(|d| observed(d, false)) {
        v.push(d);
    }
    v.push(ym(1).nth_weekday(3, Weekday::Mon));
    v.push(ym(2).nth_weekday(3, Weekday::Mon));
    v.push(easter(year).add_days(-2));
    v.push(ym(5).last_weekday(Weekday::Mon));
    if year >= 2022 {
        v.extend(fixed(6, 19).and_then(|d| observed(d, sat_to_fri)));
    }
    v.extend(fixed(7, 4).and_then(|d| observed(d, sat_to_fri)));
    v.push(ym(9).nth_weekday(1, Weekday::Mon));
    if cal == CalendarId::UsGovernmentSecurities {
        v.push(ym(10).nth_weekday(2, Weekday::Mon));
        v.extend(fixed(11, 11).and_then(|d| observed(d, false)));
    }
    v.push(ym(11).nth_weekday(4, Weekday::Thu));
    v.extend(fixed(12, 25).and_then(|d| observed(d, sat_to_fri)));
    v.sort();
    v
}

/// A calendar with its configured exceptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    id: CalendarId,
    closures: BTreeSet<Date>,
    openings: BTreeSet<Date>,
}

impl Calendar {
    /// The calendar with no exceptions.
    #[must_use]
    pub fn new(id: CalendarId) -> Self {
        Self {
            id,
            closures: BTreeSet::new(),
            openings: BTreeSet::new(),
        }
    }

    /// The calendar with extra closures and with scheduled holidays that
    /// are open in a given year.
    #[must_use]
    pub fn with_exceptions(
        id: CalendarId,
        closures: impl IntoIterator<Item = Date>,
        openings: impl IntoIterator<Item = Date>,
    ) -> Self {
        Self {
            id,
            closures: closures.into_iter().collect(),
            openings: openings.into_iter().collect(),
        }
    }

    /// The calendar's ID.
    #[must_use]
    pub const fn id(&self) -> CalendarId {
        self.id
    }

    /// True if `d` is a weekday holiday or a configured closure.
    #[must_use]
    pub fn is_holiday(&self, d: Date) -> bool {
        if self.openings.contains(&d) {
            return false;
        }
        self.closures.contains(&d) || holidays(self.id, d.ymd().0).contains(&d)
    }

    /// True if `d` is a business day.
    #[must_use]
    pub fn is_business_day(&self, d: Date) -> bool {
        !d.weekday().is_weekend() && !self.is_holiday(d)
    }

    /// The first business day strictly after `d`.
    #[must_use]
    pub fn next_business_day(&self, d: Date) -> Date {
        let mut x = d.add_days(1);
        while !self.is_business_day(x) {
            x = x.add_days(1);
        }
        x
    }

    /// The last business day strictly before `d`.
    #[must_use]
    pub fn prev_business_day(&self, d: Date) -> Date {
        let mut x = d.add_days(-1);
        while !self.is_business_day(x) {
            x = x.add_days(-1);
        }
        x
    }

    /// `d` if it is a business day, else the preceding one.
    #[must_use]
    pub fn on_or_before(&self, d: Date) -> Date {
        if self.is_business_day(d) {
            d
        } else {
            self.prev_business_day(d)
        }
    }

    /// Steps `n` business days back from `d` (`n = 0` returns `d`).
    #[must_use]
    pub fn business_days_before(&self, d: Date, n: u32) -> Date {
        (0..n).fold(d, |x, _| self.prev_business_day(x))
    }

    /// Business days in `(from, to]`; negative if `to < from`.
    #[must_use]
    pub fn business_days_between(&self, from: Date, to: Date) -> i32 {
        let (lo, hi, sign) = if from <= to {
            (from, to, 1)
        } else {
            (to, from, -1)
        };
        let mut n = 0;
        let mut x = lo;
        while x < hi {
            x = x.add_days(1);
            if self.is_business_day(x) {
                n += 1;
            }
        }
        sign * n
    }
}

/// Rule for a contract month's last trading day (method spec § 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpiryRule {
    /// The `n`-th `weekday` of the month `month_offset` months after the
    /// contract month, then `business_days_before` business days earlier.
    /// With `business_days_before = 0`, a non-business day moves to the
    /// preceding business day.
    NthWeekday {
        /// Which occurrence, from 1.
        n: u32,
        /// Day of the week.
        weekday: Weekday,
        /// Months after the contract month.
        month_offset: i32,
        /// Business days to step back.
        business_days_before: u32,
    },
    /// Calendar day `day` of the month `month_offset` months after the
    /// contract month (or the preceding business day if it is not one),
    /// then `business_days_before` business days earlier.
    BeforeDay {
        /// Calendar day of the month.
        day: u32,
        /// Months after the contract month (negative: before).
        month_offset: i32,
        /// Business days to step back.
        business_days_before: u32,
    },
    /// The `n`-th last business day of the month `month_offset` months
    /// after the contract month (`n = 1` is the last business day).
    NthLastBusinessDay {
        /// Which occurrence from the end, from 1.
        n: u32,
        /// Months after the contract month (negative: before).
        month_offset: i32,
    },
}

impl ExpiryRule {
    /// The last trading day of contract month `ym` under `cal`.
    #[must_use]
    pub fn last_trade_date(&self, cal: &Calendar, ym: YearMonth) -> Date {
        match *self {
            Self::NthWeekday {
                n,
                weekday,
                month_offset,
                business_days_before,
            } => {
                let d = ym.plus(month_offset).nth_weekday(n, weekday);
                if business_days_before == 0 {
                    cal.on_or_before(d)
                } else {
                    cal.business_days_before(d, business_days_before)
                }
            }
            Self::BeforeDay {
                day,
                month_offset,
                business_days_before,
            } => {
                let m = ym.plus(month_offset);
                let day = day.clamp(1, crate::date::days_in_month(m.year, m.month));
                let d = Date::from_ymd(m.year, m.month, day).unwrap_or_else(|| m.first_day());
                cal.business_days_before(cal.on_or_before(d), business_days_before)
            }
            Self::NthLastBusinessDay { n, month_offset } => {
                let m = ym.plus(month_offset);
                let last = cal.on_or_before(m.last_day());
                cal.business_days_before(last, n.saturating_sub(1))
            }
        }
    }
}

/// What a roll is counted back from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollReference {
    /// The last trading day.
    LastTrade,
    /// The first calendar day of the contract month (physically delivered
    /// contracts, ahead of the delivery period).
    MonthStart,
}

/// Fixed-days roll rule: the front month changes `business_days_before`
/// business days before the reference day (method spec § 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollRule {
    /// Business days before the reference day.
    pub business_days_before: u32,
    /// The reference day.
    pub reference: RollReference,
}

impl RollRule {
    /// The roll date of contract month `ym`: from this New York date on,
    /// the month is no longer the front month.
    #[must_use]
    pub fn roll_date(&self, cal: &Calendar, ym: YearMonth, last_trade: Date) -> Date {
        let reference = match self.reference {
            RollReference::LastTrade => last_trade,
            RollReference::MonthStart => cal.on_or_before(ym.first_day()),
        };
        // The roll never falls after the last trading day.
        cal.business_days_before(reference, self.business_days_before)
            .min(last_trade)
    }
}

/// Contract months listed under a cycle: those in `cycle` (month numbers),
/// starting from `from`, in order.
#[must_use]
pub fn cycle_months(cycle: &[u32], from: YearMonth, count: usize) -> Vec<YearMonth> {
    let mut out = Vec::with_capacity(count);
    let mut ym = from;
    // Every cycle month recurs within 12 months; bound the scan.
    let limit = count.saturating_mul(12).saturating_add(12);
    for _ in 0..limit {
        if out.len() >= count {
            break;
        }
        if cycle.contains(&ym.month) {
            out.push(ym);
        }
        ym = ym.plus(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        Date::parse(s).unwrap()
    }
    fn ym(s: &str) -> YearMonth {
        YearMonth::parse(s).unwrap()
    }
    fn trading() -> Calendar {
        Calendar::new(CalendarId::UsTrading)
    }
    fn govt() -> Calendar {
        Calendar::new(CalendarId::UsGovernmentSecurities)
    }

    #[test]
    fn easter_dates() {
        assert_eq!(easter(2024), d("2024-03-31"));
        assert_eq!(easter(2025), d("2025-04-20"));
        assert_eq!(easter(2026), d("2026-04-05"));
        assert_eq!(easter(2027), d("2027-03-28"));
        assert_eq!(easter(2038), d("2038-04-25"));
    }

    #[test]
    fn trading_holidays_and_observance() {
        let c = trading();
        // Juneteenth on a Friday (2026) and on a Saturday (2027: Friday).
        assert!(c.is_holiday(d("2026-06-19")));
        assert!(c.is_holiday(d("2027-06-18")));
        // Not before 2022.
        assert!(!c.is_holiday(d("2021-06-18")));
        // New Year's Day on a Saturday is not moved into December.
        assert!(c.is_business_day(d("2021-12-31")));
        // New Year's Day on a Sunday moves to Monday.
        assert!(c.is_holiday(d("2023-01-02")));
        // Independence Day on a Saturday (2026) moves to Friday, on a
        // Sunday (2027) to Monday.
        assert!(c.is_holiday(d("2026-07-03")));
        assert!(c.is_holiday(d("2027-07-05")));
        // Good Friday, Thanksgiving, Christmas.
        assert!(c.is_holiday(d("2026-04-03")));
        assert!(c.is_holiday(d("2026-11-26")));
        assert!(c.is_holiday(d("2026-12-25")));
        // Columbus and Veterans Day are trading days.
        assert!(c.is_business_day(d("2026-10-12")));
        assert!(c.is_business_day(d("2026-11-11")));
        assert_eq!(holidays(CalendarId::UsTrading, 2026).len(), 10);
    }

    #[test]
    fn government_securities_holidays() {
        let c = govt();
        assert!(c.is_holiday(d("2026-10-12")));
        assert!(c.is_holiday(d("2026-11-11")));
        // Saturday holidays are not moved (Independence Day 2026).
        assert!(c.is_business_day(d("2026-07-03")));
        // Sunday holidays move to Monday (Veterans Day 2029).
        assert!(c.is_holiday(d("2029-11-12")));
        assert!(c.is_holiday(d("2026-04-03")));
    }

    #[test]
    fn exceptions() {
        let c =
            Calendar::with_exceptions(CalendarId::UsTrading, [d("2025-01-09")], [d("2026-04-03")]);
        assert!(c.is_holiday(d("2025-01-09")));
        assert!(c.is_business_day(d("2026-04-03")));
    }

    #[test]
    fn business_day_steps() {
        let c = trading();
        assert_eq!(c.next_business_day(d("2026-07-02")), d("2026-07-06"));
        assert_eq!(c.prev_business_day(d("2026-07-06")), d("2026-07-02"));
        assert_eq!(c.business_days_between(d("2026-07-02"), d("2026-07-07")), 2);
        assert_eq!(
            c.business_days_between(d("2026-07-07"), d("2026-07-02")),
            -2
        );
    }

    #[test]
    fn last_trading_days() {
        let quarterly = ExpiryRule::NthWeekday {
            n: 3,
            weekday: Weekday::Fri,
            month_offset: 0,
            business_days_before: 0,
        };
        assert_eq!(
            quarterly.last_trade_date(&trading(), ym("2026-03")),
            d("2026-03-20")
        );
        // Third Friday is Juneteenth: the Thursday before.
        assert_eq!(
            quarterly.last_trade_date(&trading(), ym("2026-06")),
            d("2026-06-18")
        );

        // Crude: 3 business days before the 25th of the prior month; the
        // 25th of December 2025 is Christmas, so count from the 24th.
        let crude = ExpiryRule::BeforeDay {
            day: 25,
            month_offset: -1,
            business_days_before: 3,
        };
        assert_eq!(
            crude.last_trade_date(&trading(), ym("2026-01")),
            d("2025-12-19")
        );
        // 25 February 2026 is a Wednesday.
        assert_eq!(
            crude.last_trade_date(&trading(), ym("2026-03")),
            d("2026-02-20")
        );
        // 25 April 2026 is a Saturday: from Friday the 24th.
        assert_eq!(
            crude.last_trade_date(&trading(), ym("2026-05")),
            d("2026-04-21")
        );

        // Gas: third last business day of the prior month.
        let gas = ExpiryRule::NthLastBusinessDay {
            n: 3,
            month_offset: -1,
        };
        assert_eq!(
            gas.last_trade_date(&trading(), ym("2026-01")),
            d("2025-12-29")
        );
        // Metals: third last business day of the month.
        let metal = ExpiryRule::NthLastBusinessDay {
            n: 3,
            month_offset: 0,
        };
        assert_eq!(
            metal.last_trade_date(&trading(), ym("2026-02")),
            d("2026-02-25")
        );

        // Three-month rate: business day before the third Wednesday of
        // the month three months on.
        let rate3 = ExpiryRule::NthWeekday {
            n: 3,
            weekday: Weekday::Wed,
            month_offset: 3,
            business_days_before: 1,
        };
        assert_eq!(
            rate3.last_trade_date(&govt(), ym("2026-03")),
            d("2026-06-16")
        );
        // One-month rate: last business day of the month (31 May 2026 is
        // a Sunday).
        let rate1 = ExpiryRule::NthLastBusinessDay {
            n: 1,
            month_offset: 0,
        };
        assert_eq!(
            rate1.last_trade_date(&govt(), ym("2026-05")),
            d("2026-05-29")
        );
    }

    #[test]
    fn roll_dates_and_cycles() {
        let roll = RollRule {
            business_days_before: 8,
            reference: RollReference::LastTrade,
        };
        assert_eq!(
            roll.roll_date(&trading(), ym("2026-03"), d("2026-03-20")),
            d("2026-03-10")
        );
        let roll = RollRule {
            business_days_before: 2,
            reference: RollReference::MonthStart,
        };
        // 1 February 2026 is a Sunday: from Friday the 30th.
        assert_eq!(
            roll.roll_date(&trading(), ym("2026-02"), d("2026-02-25")),
            d("2026-01-28")
        );
        let q = cycle_months(&[3, 6, 9, 12], ym("2026-04"), 3);
        let s: Vec<String> = q.iter().map(ToString::to_string).collect();
        assert_eq!(s, ["2026-06", "2026-09", "2026-12"]);
        assert!(cycle_months(&[], ym("2026-04"), 3).is_empty());
    }
}
