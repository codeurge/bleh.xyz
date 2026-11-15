//! New York Fed reference rates (ADM-0007) from the Markets Data API.
//!
//! JSON responses hold `refRates`, an array of records. Admitted keys
//! only: `effectiveDate`, `type`, `percentRate`, the four percentiles,
//! `volumeInBillions`, `revisionIndicator`, `targetRateFrom`,
//! `targetRateTo`, and the SOFR averages and index. Any other key is
//! counted as unknown and never emitted.
//!
//! | Series (`nyfed:ref:...`) | From | Measure |
//! |---|---|---|
//! | `SOFR`, `BGCR`, `TGCR`, `EFFR`, `OBFR` | `percentRate` | rate, `ON`, simple act/360 |
//! | `<RATE>.P1`, `.P25`, `.P75`, `.P99` | percentiles | rate, `ON`, simple act/360 |
//! | `EFFR.TARGET_FROM`, `EFFR.TARGET_TO` | FOMC target range | rate, `ON`, simple act/360 |
//! | `SOFR.AVG30`, `.AVG90`, `.AVG180` | SOFR averages | rate, `30D`/`90D`/`180D`, simple act/360 |
//! | `SOFR.INDEX` | SOFR Index | index level |
//! | `<RATE>.VOLUME` | `volumeInBillions` | flow in `billion_usd` (ADR 0023): the day's underlying transaction volume, as published |
//!
//! Revisions (same-day, after about 2:30 p.m. ET, and the quarterly
//! revisions of the summary statistics) arrive as changed values for a
//! date already seen and become new revisions; live polls re-read the
//! last ten days.

use std::time::Duration;

use mesh_types::{Compounding, DayCount, QuantityUnit, ReferenceMeasure};
use serde_json::Value;

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::Date;

/// Default API base.
pub const BASE: &str = "https://markets.newyorkfed.org";

const RATES: [&str; 5] = ["SOFR", "BGCR", "TGCR", "EFFR", "OBFR"];
const ALLOWED_KEYS: [&str; 15] = [
    "effectiveDate",
    "type",
    "percentRate",
    "percentPercentile1",
    "percentPercentile25",
    "percentPercentile75",
    "percentPercentile99",
    "volumeInBillions",
    "revisionIndicator",
    "targetRateFrom",
    "targetRateTo",
    "average30day",
    "average90day",
    "average180day",
    "index",
];

/// The New York Fed reader.
#[derive(Debug, Clone)]
pub struct NyFedReader {
    base: String,
}

impl NyFedReader {
    /// A reader using `bases` (key `nyfed`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("nyfed", BASE),
        }
    }

    fn targets(&self, from: Date, to: Date, tag: &str) -> Vec<Target> {
        let mut out = Vec::new();
        let range = format!("startDate={from}&endDate={to}");
        let mut add = |path: &str, first: Date, name: &str| {
            if to >= first {
                out.push(Target::poll(
                    format!("nyfed:{name}:{tag}"),
                    format!("{}/api/rates/{path}/search.json?{range}", self.base),
                    "rates",
                ));
            }
        };
        add("secured/all", ymd(2018, 4, 2), "secured");
        add("secured/sofrai", ymd(2020, 3, 2), "sofrai");
        add("unsecured/effr", ymd(2000, 7, 3), "effr");
        add("unsecured/obfr", ymd(2016, 3, 1), "obfr");
        out
    }
}

fn ymd(y: i32, m: u32, d: u32) -> Date {
    Date::new(y, m, d).unwrap_or(Date { y, m: 1, d: 1 })
}

fn decimal_of(v: &Value) -> Result<Option<mesh_types::Price>, num::NotANumber> {
    match v {
        Value::Null => Ok(None),
        Value::Number(n) => num::decimal(&n.to_string()),
        Value::String(s) => num::decimal(s),
        other => Err(num::NotANumber(other.to_string())),
    }
}

fn on_rate(series: String, value: mesh_types::Price, date: Date) -> Obs {
    Obs::Ref(RefObs::rate(
        series,
        ReferenceMeasure::Rate,
        value,
        "ON",
        Compounding::Simple,
        Some(DayCount::Act360),
        date,
    ))
}

/// Parses a `refRates` response.
///
/// # Errors
///
/// Fails if the body is not JSON with a `refRates` array.
pub fn parse_rates(body: &[u8]) -> Result<Parsed, ParseError> {
    let v: Value = serde_json::from_slice(body).map_err(ParseError::new)?;
    let rows = v
        .get("refRates")
        .and_then(Value::as_array)
        .ok_or_else(|| ParseError("no refRates array".into()))?;
    let mut out = Parsed::default();
    for row in rows {
        let Some(obj) = row.as_object() else {
            out.skip("malformed");
            continue;
        };
        for k in obj.keys() {
            if !ALLOWED_KEYS.contains(&k.as_str()) {
                out.unknown.insert(k.clone());
            }
        }
        let Some(date) = obj
            .get("effectiveDate")
            .and_then(Value::as_str)
            .and_then(Date::parse_iso)
        else {
            out.skip("malformed");
            continue;
        };
        let kind = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_uppercase();
        let emit = |out: &mut Parsed, key: &str, f: &dyn Fn(mesh_types::Price) -> Obs| match obj
            .get(key)
            .map(decimal_of)
        {
            None | Some(Ok(None)) => {}
            Some(Ok(Some(p))) => out.obs.push(f(p)),
            Some(Err(_)) => out.skip("malformed"),
        };
        if kind == "SOFRAI" || obj.contains_key("average30day") {
            for (key, series, tenor) in [
                ("average30day", "SOFR.AVG30", "30D"),
                ("average90day", "SOFR.AVG90", "90D"),
                ("average180day", "SOFR.AVG180", "180D"),
            ] {
                emit(&mut out, key, &|p| {
                    Obs::Ref(RefObs::rate(
                        series.to_owned(),
                        ReferenceMeasure::Rate,
                        p,
                        tenor,
                        Compounding::Simple,
                        Some(DayCount::Act360),
                        date,
                    ))
                });
            }
            emit(&mut out, "index", &|p| {
                let mut r = RefObs::usd_price("SOFR.INDEX".into(), p, date);
                r.measure = ReferenceMeasure::IndexLevel;
                r.currency = None;
                Obs::Ref(r)
            });
            continue;
        }
        if !RATES.contains(&kind.as_str()) {
            out.unknown.insert(format!("type:{kind}"));
            out.skip("out_of_scope");
            continue;
        }
        emit(&mut out, "percentRate", &|p| on_rate(kind.clone(), p, date));
        for (key, suffix) in [
            ("percentPercentile1", "P1"),
            ("percentPercentile25", "P25"),
            ("percentPercentile75", "P75"),
            ("percentPercentile99", "P99"),
        ] {
            emit(&mut out, key, &|p| {
                on_rate(format!("{kind}.{suffix}"), p, date)
            });
        }
        if kind == "EFFR" {
            emit(&mut out, "targetRateFrom", &|p| {
                on_rate("EFFR.TARGET_FROM".into(), p, date)
            });
            emit(&mut out, "targetRateTo", &|p| {
                on_rate("EFFR.TARGET_TO".into(), p, date)
            });
        }
        emit(&mut out, "volumeInBillions", &|p| {
            Obs::Ref(RefObs::quantity(
                format!("{kind}.VOLUME"),
                ReferenceMeasure::Flow,
                p,
                QuantityUnit::BillionUsd,
                date,
            ))
        });
    }
    Ok(out)
}

impl Reader for NyFedReader {
    fn id(&self) -> SourceId {
        SourceId::NyFed
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        self.targets(today.add_days(-10), today, "live")
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        self.targets(ymd(year, 1, 1), ymd(year, 12, 31), &year.to_string())
    }

    fn parse(&self, _target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        parse_rates(body)
    }

    fn schedule(&self) -> Schedule {
        // Releases at about 8:00, 9:00 and (revisions) 14:30 ET.
        Schedule {
            days: WEEKDAYS,
            from_min: 12 * 60,
            to_min: 20 * 60,
            inside: Duration::from_secs(600),
            outside: Duration::from_secs(7200),
        }
    }

    fn revision_days(&self) -> u32 {
        // Summary statistics are revised quarterly.
        120
    }
}
