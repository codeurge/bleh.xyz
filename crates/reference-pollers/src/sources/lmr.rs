//! USDA AMS Livestock Mandatory Reporting (ADM-0010) from the LMPR
//! DataMart API, and the DataMart response reader shared with the dairy
//! reports.
//!
//! Prices are admitted by purchase type, exactly as ADM-0010 lists them:
//!
//! | Report | Rows read | Price fields | Flag |
//! |---|---|---|---|
//! | 2511 LM_HG201, section `Barrows/Gilts` | `purchase_type` = `Prod. Sold Negotiated` | `base_price`, `avg_net_price`, `lowest_net_price`, `highest_net_price` | none |
//! | 2511, same section | `Prod. Sold Negotiated Formula`, `Prod. Sold Swine or Pork Market Formula`, `Prod. Sold Combined` | the same | `exchange_referenced` |
//! | 2466 LM_CT100, section `Detail` (negotiated cattle) | every row | `price_range_low`, `price_range_high`, `weighted_avg_price` | none |
//!
//! The prices of `Prod. Sold Other Market Formula`, `Prod. Sold Other
//! Purchase Arrangement`, `Packer Owned` and the all-purchase-type
//! aggregates are dropped as excluded, as are other sections of those
//! reports (Carcass Measurements mixes purchase types in its price
//! averages). Unknown purchase types are reported and dropped whole.
//!
//! Volumes are admitted for every purchase type (ADM-0010) and are emitted
//! as quantities (ADR 0023), without a flag (they are not prices):
//!
//! | Report | Rows read | Field | Record |
//! |---|---|---|---|
//! | 2511, section `Barrows/Gilts` | every known purchase type, excluded-price types and aggregates included | `head_count` | `flow` in `head` |
//! | 2466, section `Detail` | every row | `head_count` | `flow` in `head` |
//!
//! Weights and carcass measurements are admitted but not yet emitted.
//! Other LMR reports are not read until their layouts are documented in
//! the access specification. Values are carried exactly as published.
//!
//! Series: `LM_HG201.<PURCHASE_TYPE>.<field>` and
//! `LM_CT100.<CLASS>.<BASIS>.<GRADE>.<LOW>_<HIGH>.<field>` under
//! `usda-lmr:ref:`, dated by `report_date`; `published_date`, when given,
//! is the publication time (US Central time).

use std::time::Duration;

use mesh_types::{QuantityUnit, ReferenceMeasure};
use serde_json::{Map, Value};

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::{Date, UsZone, local_to_utc_ms};

/// Default DataMart base.
pub const BASE: &str = "https://mpr.datamart.ams.usda.gov";

/// One section of a DataMart response.
#[derive(Debug, Clone, Default)]
pub struct Section {
    /// `reportSection`.
    pub name: String,
    /// `results` rows.
    pub rows: Vec<Map<String, Value>>,
}

/// Reads a DataMart response: an array of sections, one section object,
/// or the string `"No Results Found."` (no sections).
///
/// # Errors
///
/// Fails on anything else.
pub fn sections(body: &[u8]) -> Result<Vec<Section>, ParseError> {
    let v: Value = serde_json::from_slice(body).map_err(ParseError::new)?;
    let one = |s: &Value| -> Option<Section> {
        let o = s.as_object()?;
        Some(Section {
            name: o
                .get("reportSection")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            rows: o
                .get("results")?
                .as_array()?
                .iter()
                .filter_map(|r| r.as_object().cloned())
                .collect(),
        })
    };
    match &v {
        Value::String(s)
            if s.trim_end_matches('.')
                .eq_ignore_ascii_case("No Results Found") =>
        {
            Ok(Vec::new())
        }
        Value::Array(a) => Ok(a.iter().filter_map(one).collect()),
        Value::Object(_) => one(&v)
            .map(|s| vec![s])
            .ok_or_else(|| ParseError("section without results".into())),
        _ => Err(ParseError("unexpected DataMart response".into())),
    }
}

/// A cell as text (DataMart values are strings; numbers are accepted).
#[must_use]
pub fn text(row: &Map<String, Value>, key: &str) -> Option<String> {
    match row.get(key)? {
        Value::String(s) => Some(s.trim().to_owned()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The publication time of a row (`published_date`, US Central time),
/// Unix milliseconds.
#[must_use]
pub fn published_ms(row: &Map<String, Value>) -> Option<i64> {
    let s = text(row, "published_date")?;
    let date = Date::parse_us(&s)?;
    let time = s.split_whitespace().nth(1).unwrap_or("00:00:00");
    let mut it = time.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m) = (it.next().flatten()?, it.next().flatten().unwrap_or(0));
    let pm = s.to_ascii_uppercase().ends_with("PM");
    let h = match (h, pm) {
        (12, false) if s.to_ascii_uppercase().ends_with("AM") => 0,
        (h, true) if h < 12 => h + 12,
        (h, _) => h,
    };
    Some(local_to_utc_ms(date, h * 60 + m, UsZone::Central, false))
}

/// The DataMart query path for `slug` over `[from, to]` on `var`.
#[must_use]
pub fn range_url(base: &str, slug: u32, var: &str, from: Date, to: Date) -> String {
    format!(
        "{base}/services/v1.1/reports/{slug}?q={var}={}:{}&allSections=true",
        from.us(),
        to.us()
    )
}

/// Splits a year into date ranges of at most 180 days (the API's cap).
#[must_use]
pub fn year_chunks(year: i32, first: Date) -> Vec<(Date, Date)> {
    let (Some(start), Some(end)) = (Date::new(year, 1, 1), Date::new(year, 12, 31)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut from = start.max(first);
    while from <= end {
        let to = from.add_days(119).min(end);
        out.push((from, to));
        from = to.add_days(1);
    }
    out
}

/// Swine purchase types, their series code and how ADM-0010 treats their
/// prices: `Some(flag)` admitted (with `exchange_referenced` when true),
/// `None` excluded. Volumes are admitted for every type.
pub const SWINE_PURCHASE_TYPES: [(&str, &str, Option<bool>); 9] = [
    ("Prod. Sold Negotiated", "NEGOTIATED", Some(false)),
    (
        "Prod. Sold Negotiated Formula",
        "NEGOTIATED_FORMULA",
        Some(true),
    ),
    (
        "Prod. Sold Swine or Pork Market Formula",
        "SWINE_PORK_MARKET_FORMULA",
        Some(true),
    ),
    ("Prod. Sold Combined", "COMBINED", Some(true)),
    (
        "Prod. Sold Other Market Formula",
        "OTHER_MARKET_FORMULA",
        None,
    ),
    (
        "Prod. Sold Other Purchase Arrangement",
        "OTHER_PURCHASE_ARRANGEMENT",
        None,
    ),
    ("Prod. Sold (All Purchase Types)", "PRODUCER_SOLD_ALL", None),
    ("Pack. Sold (All Purchase Types)", "PACKER_SOLD_ALL", None),
    ("Packer Owned", "PACKER_OWNED", None),
];

const SWINE_PRICES: [&str; 4] = [
    "base_price",
    "avg_net_price",
    "lowest_net_price",
    "highest_net_price",
];
const CATTLE_PRICES: [&str; 3] = ["price_range_low", "price_range_high", "weighted_avg_price"];

fn push_price(
    out: &mut Parsed,
    row: &Map<String, Value>,
    field: &str,
    series: String,
    as_of: Date,
    flag: bool,
) {
    let Some(t) = text(row, field) else { return };
    match num::decimal(&t) {
        Ok(Some(v)) => {
            let mut r = RefObs::usd_price(series, v, as_of);
            r.exchange_referenced = flag;
            r.published_ms = published_ms(row);
            out.obs.push(Obs::Ref(r));
        }
        Ok(None) => out.skip("missing"),
        Err(_) => out.skip("malformed"),
    }
}

/// Emits a row's `head_count` as a flow in head under `series`, if the row
/// has one (ADR 0023; admitted for every type, never flagged).
fn push_head_count(out: &mut Parsed, row: &Map<String, Value>, series: String, as_of: Date) {
    let Some(t) = text(row, "head_count") else {
        return;
    };
    match num::decimal(&t) {
        Ok(Some(v)) => {
            let mut r =
                RefObs::quantity(series, ReferenceMeasure::Flow, v, QuantityUnit::Head, as_of);
            r.published_ms = published_ms(row);
            out.obs.push(Obs::Ref(r));
        }
        Ok(None) => out.skip("missing"),
        Err(_) => out.skip("malformed"),
    }
}

/// Parses LM_HG201 (2511).
///
/// # Errors
///
/// Fails on an unreadable response.
pub fn parse_swine(body: &[u8]) -> Result<Parsed, ParseError> {
    let mut out = Parsed::default();
    for s in sections(body)? {
        if s.name != "Barrows/Gilts" {
            out.skipped
                .entry("out_of_scope")
                .and_modify(|n| *n += s.rows.len() as u64)
                .or_insert(s.rows.len() as u64);
            continue;
        }
        for row in &s.rows {
            let Some(as_of) = text(row, "report_date").and_then(|d| Date::parse_any(&d)) else {
                out.skip("malformed");
                continue;
            };
            let pt = text(row, "purchase_type").unwrap_or_default();
            match SWINE_PURCHASE_TYPES.iter().find(|(name, _, _)| *name == pt) {
                Some((_, code, Some(flag))) => {
                    for f in SWINE_PRICES {
                        push_price(
                            &mut out,
                            row,
                            f,
                            format!("LM_HG201.{code}.{f}"),
                            as_of,
                            *flag,
                        );
                    }
                    push_head_count(&mut out, row, format!("LM_HG201.{code}.head_count"), as_of);
                }
                Some((_, code, None)) => {
                    // The prices are excluded; the volume is admitted.
                    out.skip("excluded");
                    push_head_count(&mut out, row, format!("LM_HG201.{code}.head_count"), as_of);
                }
                None => {
                    out.unknown.insert(format!("purchase_type:{pt}"));
                    out.skip("excluded");
                }
            }
        }
    }
    Ok(out)
}

/// Parses LM_CT100 (2466).
///
/// # Errors
///
/// Fails on an unreadable response.
pub fn parse_cattle(body: &[u8]) -> Result<Parsed, ParseError> {
    let mut out = Parsed::default();
    for s in sections(body)? {
        if s.name != "Detail" {
            *out.skipped.entry("out_of_scope").or_default() += s.rows.len() as u64;
            continue;
        }
        for row in &s.rows {
            let Some(as_of) = text(row, "report_date").and_then(|d| Date::parse_any(&d)) else {
                out.skip("malformed");
                continue;
            };
            let part = |k: &str| num::slug(&text(row, k).unwrap_or_default());
            let key = format!(
                "LM_CT100.{}.{}.{}.{}_{}",
                part("class_description"),
                part("selling_basis_description"),
                part("grade_description"),
                part("weight_range_low"),
                part("weight_range_high")
            );
            for f in CATTLE_PRICES {
                push_price(&mut out, row, f, format!("{key}.{f}"), as_of, false);
            }
            push_head_count(&mut out, row, format!("{key}.head_count"), as_of);
        }
    }
    Ok(out)
}

/// The LMR reader.
#[derive(Debug, Clone)]
pub struct LmrReader {
    base: String,
}

/// Reports read, with their first date.
pub const REPORTS: [(u32, &str); 2] = [(2511, "swine"), (2466, "cattle")];

impl LmrReader {
    /// A reader using `bases` (key `datamart`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("datamart", BASE),
        }
    }
}

impl Reader for LmrReader {
    fn id(&self) -> SourceId {
        SourceId::UsdaLmr
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        REPORTS
            .iter()
            .map(|(slug, kind)| {
                Target::poll(
                    format!("lmr:{slug}:live"),
                    range_url(&self.base, *slug, "report_date", today.add_days(-7), today),
                    *kind,
                )
            })
            .collect()
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        let first = SourceId::UsdaLmr.earliest().unwrap_or(Date {
            y: 2001,
            m: 4,
            d: 2,
        });
        let mut out = Vec::new();
        for (slug, kind) in REPORTS {
            for (from, to) in year_chunks(year, first) {
                out.push(Target::poll(
                    format!("lmr:{slug}:{from}"),
                    range_url(&self.base, slug, "report_date", from, to),
                    kind,
                ));
            }
        }
        out
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        match target.kind.as_str() {
            "swine" => parse_swine(body),
            "cattle" => parse_cattle(body),
            k => Err(ParseError(format!("bad target kind {k}"))),
        }
    }

    fn schedule(&self) -> Schedule {
        // Daily reports publish through the US business day.
        Schedule {
            days: WEEKDAYS,
            from_min: 13 * 60,
            to_min: 23 * 60 + 59,
            inside: Duration::from_secs(1800),
            outside: Duration::from_secs(6 * 3600),
        }
    }

    fn live_window_days(&self) -> i64 {
        14
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert!(sections(br#""No Results Found.""#).unwrap().is_empty());
        assert!(sections(b"[1]").unwrap().is_empty());
        assert!(sections(b"5").is_err());
        let chunks = year_chunks(2024, Date::new(2001, 4, 2).unwrap());
        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|(a, b)| b.days() - a.days() < 180));
        assert_eq!(chunks.last().unwrap().1.to_string(), "2024-12-31");
        let first = year_chunks(2001, Date::new(2001, 4, 2).unwrap());
        assert_eq!(first[0].0.to_string(), "2001-04-02");
        let mut row = Map::new();
        row.insert("published_date".into(), Value::from("03/10/2026 08:30:00"));
        // 08:30 CDT is 13:30 UTC.
        let d = Date::new(2026, 3, 10).unwrap();
        assert_eq!(
            published_ms(&row),
            Some(d.start_ms() + 13 * 3_600_000 + 30 * 60_000)
        );
    }
}
