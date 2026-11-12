//! Public reference values: rates, yields, prices and index levels that a
//! public-data source publishes (observation-v0 § 2.12).
//!
//! A [`ReferenceValue`] is not derived: it is one published value, read by
//! a poller from a source admitted for that field (for example a
//! government yield curve or an overnight reference rate). The synthetic
//! contract-month engine (`crates/synthetic`) reads these to build its rate
//! curve.
//!
//! - `prov.venue` is the public-data source and `prov.instrument` is
//!   `<source>:ref:<series>`, where `<series>` is the source's own series
//!   name. A curve publishes one record per tenor under one instrument,
//!   told apart by `tenor` or `maturity_date`.
//! - `prov.seq` is `(batch, index)`, as for swap prints: the poll or file
//!   that delivered the value, and its position in it.
//! - `prov.ts_source_ms` is the publication time; `as_of_date` is the
//!   business day the value is for (an overnight rate for day `D` is often
//!   published on `D + 1`).
//! - A later publication for the same series, tenor, period and
//!   `as_of_date` with a higher `revision` supersedes an earlier one.
//! - Values for a delivery or settlement interval within the day (for
//!   example a grid operator's hourly or 15-minute price) carry
//!   `period_start_ms` and `period_end_ms`. Both are omitted (not `null`)
//!   when absent, so records without a period keep their canonical
//!   encoding.

use serde::{Deserialize, Serialize};

use crate::decimal::Price;
use crate::provenance::Provenance;
use crate::record::{RecordKind, ValidationError, ensure};
use crate::time::UnixMillis;

/// Market class of reference-value instruments (`<source>:ref:<series>`).
pub const REFERENCE_CLASS: &str = "ref";

/// What a reference value measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMeasure {
    /// A money-market or overnight rate, percent per annum.
    Rate,
    /// A par yield (coupon rate of a bond priced at par), percent.
    ParYield,
    /// A zero-coupon yield, percent.
    ZeroYield,
    /// A price in currency units.
    Price,
    /// A dimensionless index level (for example a compounded-rate index).
    IndexLevel,
}

impl ReferenceMeasure {
    /// True for rates and yields, which need a compounding convention.
    #[must_use]
    pub const fn is_rate(self) -> bool {
        matches!(self, Self::Rate | Self::ParYield | Self::ZeroYield)
    }
}

/// Day-count convention of a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DayCount {
    /// Actual days / 360.
    #[serde(rename = "act_360")]
    Act360,
    /// Actual days / 365.
    #[serde(rename = "act_365_fixed")]
    Act365Fixed,
    /// Actual / actual.
    #[serde(rename = "act_act")]
    ActAct,
    /// 30 / 360.
    #[serde(rename = "thirty_360")]
    Thirty360,
}

/// How a rate or yield compounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compounding {
    /// Simple (money-market) interest over the period.
    Simple,
    /// Bank discount rate.
    Discount,
    /// Compounded once a year.
    Annual,
    /// Compounded twice a year (bond-equivalent).
    SemiAnnual,
    /// Continuously compounded.
    Continuous,
}

/// One published reference value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceValue {
    /// Provenance (see the module docs).
    pub prov: Provenance,
    /// What the value measures.
    pub measure: ReferenceMeasure,
    /// The value: percent for rates and yields, currency units for prices.
    pub value: Price,
    /// Tenor: `ON` (overnight), `TN` (tomorrow-next) or a count and unit
    /// (`D`, `W`, `M`, `Y`), such as `3M`.
    pub tenor: Option<String>,
    /// Maturity of the instrument the value refers to, `YYYY-MM-DD`.
    pub maturity_date: Option<String>,
    /// Day count (rates and yields with simple or discount compounding).
    pub day_count: Option<DayCount>,
    /// Compounding (required for rates and yields).
    pub compounding: Option<Compounding>,
    /// ISO 4217 currency (prices only).
    pub currency: Option<String>,
    /// The business day the value is for, `YYYY-MM-DD`.
    pub as_of_date: String,
    /// Revision number: 0 for the first publication.
    pub revision: u32,
    /// Start of the delivery or settlement interval the value is for,
    /// inclusive (intra-day values only). Omitted when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_start_ms: Option<UnixMillis>,
    /// End of that interval, exclusive. Present exactly when
    /// `period_start_ms` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_end_ms: Option<UnixMillis>,
}

/// True for a real proleptic Gregorian date written `YYYY-MM-DD`.
pub(crate) fn is_calendar_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        b[r].iter().try_fold(0u32, |acc, &c| {
            c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
        })
    };
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else {
        return false;
    };
    (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)
}

/// True for a year and month written `YYYY-MM`.
pub(crate) fn is_year_month(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7
        && b[4] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_digit())
        && matches!((b[5], b[6]), (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2'))
}

const fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        2 if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// True for a tenor: `ON`, `TN`, or 1 to 3 digits (no leading zero) and a
/// unit `D`, `W`, `M` or `Y`.
#[must_use]
pub fn is_tenor(s: &str) -> bool {
    if s == "ON" || s == "TN" {
        return true;
    }
    let b = s.as_bytes();
    let Some((&unit, digits)) = b.split_last() else {
        return false;
    };
    matches!(unit, b'D' | b'W' | b'M' | b'Y')
        && (1..=3).contains(&digits.len())
        && digits[0] != b'0'
        && digits.iter().all(u8::is_ascii_digit)
}

impl ReferenceValue {
    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Fails if the instrument class is not `ref`, a date or tenor is
    /// malformed, a rate has no compounding or no tenor or maturity, a
    /// simple or discount rate has no day count, rate conventions are set
    /// on a price or index level, or a currency is set on anything but a
    /// price or is not three uppercase letters, or a period has only one
    /// end or does not end after it starts.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::ReferenceValue;
        match (self.period_start_ms, self.period_end_ms) {
            (None, None) => {}
            (Some(s), Some(e)) => ensure(s.0 < e.0, k, || {
                "period_end_ms must be after period_start_ms".into()
            })?,
            _ => {
                return Err(ValidationError::new(
                    k,
                    "period_start_ms and period_end_ms go together",
                ));
            }
        }
        ensure(
            self.prov.instrument.class_part() == REFERENCE_CLASS,
            k,
            || "instrument class must be ref".into(),
        )?;
        ensure(is_calendar_date(&self.as_of_date), k, || {
            "as_of_date must be a YYYY-MM-DD date".into()
        })?;
        if let Some(d) = &self.maturity_date {
            ensure(is_calendar_date(d), k, || {
                "maturity_date must be a YYYY-MM-DD date".into()
            })?;
        }
        if let Some(t) = &self.tenor {
            ensure(is_tenor(t), k, || format!("bad tenor {t:?}"))?;
        }
        if self.measure.is_rate() {
            ensure(
                self.tenor.is_some() || self.maturity_date.is_some(),
                k,
                || "a rate needs a tenor or a maturity date".into(),
            )?;
            let Some(c) = self.compounding else {
                return Err(ValidationError::new(k, "a rate needs compounding"));
            };
            ensure(
                !matches!(c, Compounding::Simple | Compounding::Discount)
                    || self.day_count.is_some(),
                k,
                || "simple and discount rates need a day count".into(),
            )?;
        } else {
            ensure(
                self.day_count.is_none() && self.compounding.is_none(),
                k,
                || "day count and compounding apply to rates only".into(),
            )?;
        }
        match &self.currency {
            Some(c) => ensure(
                self.measure == ReferenceMeasure::Price
                    && c.len() == 3
                    && c.bytes().all(|b| b.is_ascii_uppercase()),
                k,
                || "currency is an ISO 4217 code, on prices only".into(),
            ),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::record::tests::px;
    use crate::{AdmissionId, Record, VenueId};

    /// A synthetic overnight rate (invented source and values).
    pub(crate) fn overnight() -> ReferenceValue {
        let mut prov = sample("example-rates:ref:OVERNIGHT", 40, 0);
        prov.venue = VenueId::new("example-rates").unwrap();
        prov.admission = AdmissionId::new("ADM-9001").unwrap();
        ReferenceValue {
            prov,
            measure: ReferenceMeasure::Rate,
            value: px("4.31"),
            tenor: Some("ON".into()),
            maturity_date: None,
            day_count: Some(DayCount::Act360),
            compounding: Some(Compounding::Simple),
            currency: None,
            as_of_date: "2025-12-31".into(),
            revision: 0,
            period_start_ms: None,
            period_end_ms: None,
        }
    }

    /// A synthetic price with a currency and a maturity (every optional
    /// field set somewhere across the samples).
    pub(crate) fn price() -> ReferenceValue {
        let mut r = overnight();
        r.prov.instrument = crate::InstrumentId::new("example-rates:ref:BILL").unwrap();
        r.measure = ReferenceMeasure::Price;
        r.value = px("98.765");
        r.tenor = None;
        r.maturity_date = Some("2026-06-30".into());
        r.day_count = None;
        r.compounding = None;
        r.currency = Some("USD".into());
        r.revision = 1;
        r
    }

    /// A synthetic price for one hourly delivery interval.
    pub(crate) fn interval_price() -> ReferenceValue {
        let mut r = price();
        r.prov.instrument = crate::InstrumentId::new("example-rates:ref:HOURLY.NODE").unwrap();
        r.maturity_date = None;
        r.value = px("-12.50");
        r.revision = 0;
        r.period_start_ms = Some(UnixMillis(1_767_222_000_000));
        r.period_end_ms = Some(UnixMillis(1_767_225_600_000));
        r
    }

    pub(crate) fn samples() -> Vec<Record> {
        vec![overnight().into(), price().into(), interval_price().into()]
    }

    #[test]
    fn samples_validate_and_round_trip() {
        for r in samples() {
            r.validate().unwrap_or_else(|e| panic!("{e}"));
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r, "{json}");
            assert!(r.derived().is_none());
            assert!(!r.kind().is_derived());
        }
        let json = serde_json::to_value(Record::from(overnight())).unwrap();
        assert_eq!(json["day_count"], "act_360");
        assert_eq!(json["kind"], "reference_value");
        // No period: the fields are omitted, so the canonical encoding of
        // records without a period is unchanged.
        assert!(json.get("period_start_ms").is_none());
        let json = serde_json::to_value(Record::from(interval_price())).unwrap();
        assert_eq!(json["period_start_ms"], 1_767_222_000_000_i64);
    }

    #[test]
    fn canonical_encoding_without_period_is_unchanged() {
        // Pinned before the period fields existed.
        let c = crate::hash::canonical_record(&Record::from(overnight())).unwrap();
        assert_eq!(
            String::from_utf8(c).unwrap(),
            concat!(
                r#"{"as_of_date":"2025-12-31","compounding":"simple","currency":null,"#,
                r#""day_count":"act_360","kind":"reference_value","maturity_date":null,"#,
                r#""measure":"rate","prov":{"admission":"ADM-9001","flags":[],"#,
                r#""instrument":"example-rates:ref:OVERNIGHT","seq":{"block_number":40,"#,
                r#""event_index":0},"ts_ingest_ns":"1767225600004000000","#,
                r#""ts_source_ms":1767225600000,"venue":"example-rates"},"revision":0,"#,
                r#""tenor":"ON","value":"4.31"}"#
            )
        );
    }

    #[test]
    fn periods_are_checked() {
        let mut r = interval_price();
        r.period_end_ms = None;
        assert!(r.validate().is_err());
        let mut r = interval_price();
        r.period_end_ms = r.period_start_ms;
        assert!(r.validate().is_err());
        interval_price().validate().unwrap();
    }

    #[test]
    fn invalid_values_fail() {
        let mut r = overnight();
        r.compounding = None;
        assert!(r.validate().is_err());
        let mut r = overnight();
        r.day_count = None;
        assert!(r.validate().is_err());
        let mut r = overnight();
        r.tenor = Some("03M".into());
        assert!(r.validate().is_err());
        let mut r = overnight();
        r.tenor = None;
        assert!(r.validate().is_err());
        let mut r = overnight();
        r.as_of_date = "2025-02-29".into();
        assert!(r.validate().is_err());
        let mut r = overnight();
        r.currency = Some("USD".into());
        assert!(r.validate().is_err());
        let mut r = price();
        r.compounding = Some(Compounding::Annual);
        assert!(r.validate().is_err());
        let mut r = overnight();
        r.prov.instrument = crate::InstrumentId::new("example-rates:swap:X").unwrap();
        assert!(r.validate().is_err());
    }

    #[test]
    fn tenor_and_date_grammar() {
        for t in ["ON", "TN", "1D", "4W", "13W", "3M", "120M", "30Y"] {
            assert!(is_tenor(t), "{t}");
        }
        for t in ["", "M", "0M", "1000D", "3m", "1Q", "O/N"] {
            assert!(!is_tenor(t), "{t}");
        }
        assert!(is_calendar_date("2024-02-29"));
        assert!(!is_calendar_date("2100-02-29"));
        assert!(!is_calendar_date("2024-13-01"));
        assert!(is_year_month("2027-03"));
        assert!(!is_year_month("2027-13"));
        assert!(!is_year_month("2027-00"));
    }
}
