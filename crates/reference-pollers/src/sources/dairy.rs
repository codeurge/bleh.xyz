//! USDA AMS dairy (ADM-0011): the National Dairy Products Sales Report
//! (2993) and the Announcement of Class and Component Prices (2991), from
//! the LMPR DataMart. Every price carries `exchange_referenced`.
//!
//! | Report | Fields | Series (`usda-dairy:ref:...`) |
//! |---|---|---|
//! | 2993 NDPSR, every section (and the `Final` sections) | `Butter_Price`, `cheese_40_Price`, `cheese_500_Price`, `cheese_500_Wtd_Price`, `whey_Price`, `nonfat_milk_Price` | `NDPSR.<field>`, dated by `week_ending_date` |
//! | 2991, section `Detail` | the 18 class, component and monthly-average price fields of ADM-0011 | `FMMO.<field>`, dated the first of `report_year`/`report_month` |
//!
//! | 2993 NDPSR, same sections | `Butter_Sales`, `cheese_40_Sales`, `cheese_500_Sales`, `whey_Sales`, `nonfat_milk_Sales` | `NDPSR.<field>`: `flow` in `pounds` (the week's sales), no flag |
//! | 2993 NDPSR, barrel cheese | `cheese_500_Wtd_Mst`, `cheese_500_Mst` | `NDPSR.<field>`: `ratio` in `percent` (moisture content), no flag |
//!
//! NDPSR is revised for up to four weeks: live polls re-read five weeks,
//! and `Final` sections are read after the others, so a changed value
//! becomes the next revision. Sales volumes and moisture are admitted
//! (ADM-0011) and emitted as quantities (ADR 0023); they are not prices,
//! so they carry no `exchange_referenced`. Any field quoting an exchange's
//! spot cash or futures price (`CME` or `futures` in its name) is dropped
//! as excluded; other undocumented price fields are reported and dropped.
//! Announced prices of reports 2989, 3345, 3351, 3354, 3355 and 3357 are
//! admitted but their field lists are not yet documented, so they are not
//! read.

use std::time::Duration;

use mesh_types::{QuantityUnit, ReferenceMeasure};
use serde_json::{Map, Value};

use super::lmr::{published_ms, range_url, sections, text, year_chunks};
use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::Date;

/// Admitted NDPSR price fields.
pub const NDPSR_PRICES: [&str; 6] = [
    "Butter_Price",
    "cheese_40_Price",
    "cheese_500_Price",
    "cheese_500_Wtd_Price",
    "whey_Price",
    "nonfat_milk_Price",
];
/// Admitted NDPSR volume fields, with their measure and unit: weekly sales
/// in pounds, and the moisture content of barrel cheese in percent.
pub const NDPSR_VOLUMES: [(&str, ReferenceMeasure, QuantityUnit); 7] = [
    ("Butter_Sales", ReferenceMeasure::Flow, QuantityUnit::Pounds),
    (
        "cheese_40_Sales",
        ReferenceMeasure::Flow,
        QuantityUnit::Pounds,
    ),
    (
        "cheese_500_Sales",
        ReferenceMeasure::Flow,
        QuantityUnit::Pounds,
    ),
    (
        "cheese_500_Wtd_Mst",
        ReferenceMeasure::Ratio,
        QuantityUnit::Percent,
    ),
    ("whey_Sales", ReferenceMeasure::Flow, QuantityUnit::Pounds),
    (
        "nonfat_milk_Sales",
        ReferenceMeasure::Flow,
        QuantityUnit::Pounds,
    ),
    (
        "cheese_500_Mst",
        ReferenceMeasure::Ratio,
        QuantityUnit::Percent,
    ),
];
/// Admitted announced class and component price fields.
pub const CLASS_PRICES: [&str; 18] = [
    "class_2_Price",
    "class_2_butterfat_Price",
    "advanced_skim_milk_class_2_Price",
    "class_3_Price",
    "class_3_skim_milk_Price",
    "class_4_Price",
    "class_4_skim_milk_Price",
    "butterfat_Price",
    "nonfat_solids_Price",
    "protein_Price",
    "other_solids_Price",
    "somatic_cell_adjustment_Rate",
    "butter_monthly_avg_Price",
    "nfdm_monthly_avg_Price",
    "cheese_monthly_avg_Price",
    "cheese_monthly_40p_Price",
    "cheese_monthly_500p_Price",
    "whey_monthly_avg_Price",
];

fn exchange_quote(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("cme") || k.contains("futures")
}

fn price_like(key: &str) -> bool {
    key.ends_with("_Price") || key.ends_with("_price") || key.ends_with("_Rate")
}

fn read_fields(
    out: &mut Parsed,
    row: &Map<String, Value>,
    admitted: &[&str],
    volumes: &[(&str, ReferenceMeasure, QuantityUnit)],
    prefix: &str,
    as_of: Date,
    dest: &mut Vec<Obs>,
) {
    for key in row.keys() {
        if exchange_quote(key) {
            out.skip("excluded");
            continue;
        }
        if admitted.contains(&key.as_str()) {
            let Some(t) = text(row, key) else { continue };
            match num::decimal(&t) {
                Ok(Some(v)) => {
                    let mut r = RefObs::usd_price(format!("{prefix}.{key}"), v, as_of);
                    r.exchange_referenced = true;
                    r.published_ms = published_ms(row);
                    dest.push(Obs::Ref(r));
                }
                Ok(None) => out.skip("missing"),
                Err(_) => out.skip("malformed"),
            }
        } else if let Some(&(_, measure, unit)) = volumes.iter().find(|v| v.0 == key) {
            let Some(t) = text(row, key) else { continue };
            match num::decimal(&t) {
                Ok(Some(v)) => {
                    let mut r =
                        RefObs::quantity(format!("{prefix}.{key}"), measure, v, unit, as_of);
                    r.published_ms = published_ms(row);
                    dest.push(Obs::Ref(r));
                }
                Ok(None) => out.skip("missing"),
                Err(_) => out.skip("malformed"),
            }
        } else if price_like(key) {
            out.unknown.insert(key.clone());
        }
    }
}

/// Parses NDPSR (2993).
///
/// # Errors
///
/// Fails on an unreadable response.
pub fn parse_ndpsr(body: &[u8]) -> Result<Parsed, ParseError> {
    let mut out = Parsed::default();
    let mut finals = Vec::new();
    for s in sections(body)? {
        let is_final = s.name.trim_start().starts_with("Final");
        for row in &s.rows {
            let Some(as_of) = text(row, "week_ending_date").and_then(|d| Date::parse_any(&d))
            else {
                out.skip("malformed");
                continue;
            };
            let mut dest = Vec::new();
            read_fields(
                &mut out,
                row,
                &NDPSR_PRICES,
                &NDPSR_VOLUMES,
                "NDPSR",
                as_of,
                &mut dest,
            );
            if is_final {
                finals.extend(dest);
            } else {
                out.obs.extend(dest);
            }
        }
    }
    out.obs.extend(finals);
    Ok(out)
}

fn month_number(s: &str) -> Option<u32> {
    if let Ok(n) = s.trim().parse::<u32>() {
        return (1..=12).contains(&n).then_some(n);
    }
    const NAMES: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let l = s.trim().to_ascii_lowercase();
    NAMES
        .iter()
        .position(|n| l.starts_with(n))
        .and_then(|i| u32::try_from(i + 1).ok())
}

/// Parses the announced class and component prices (2991).
///
/// # Errors
///
/// Fails on an unreadable response.
pub fn parse_class(body: &[u8]) -> Result<Parsed, ParseError> {
    let mut out = Parsed::default();
    for s in sections(body)? {
        if s.name != "Detail" {
            *out.skipped.entry("out_of_scope").or_default() += s.rows.len() as u64;
            continue;
        }
        for row in &s.rows {
            let year = text(row, "report_year").and_then(|y| y.parse::<i32>().ok());
            let month = text(row, "report_month").and_then(|m| month_number(&m));
            let Some(as_of) = year.zip(month).and_then(|(y, m)| Date::new(y, m, 1)) else {
                out.skip("malformed");
                continue;
            };
            let mut dest = Vec::new();
            read_fields(&mut out, row, &CLASS_PRICES, &[], "FMMO", as_of, &mut dest);
            out.obs.extend(dest);
        }
    }
    Ok(out)
}

/// The dairy reader.
#[derive(Debug, Clone)]
pub struct DairyReader {
    base: String,
}

impl DairyReader {
    /// A reader using `bases` (key `datamart`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("datamart", super::lmr::BASE),
        }
    }

    fn class_target(&self, year: i32, tag: &str) -> Target {
        Target::poll(
            format!("dairy:2991:{year}:{tag}"),
            format!(
                "{}/services/v1.1/reports/2991?q=report_year={year}&allSections=true",
                self.base
            ),
            "class",
        )
    }
}

impl Reader for DairyReader {
    fn id(&self) -> SourceId {
        SourceId::UsdaDairy
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        let mut t = vec![
            Target::poll(
                "dairy:2993:live",
                range_url(
                    &self.base,
                    2993,
                    "week_ending_date",
                    today.add_days(-35),
                    today,
                ),
                "ndpsr",
            ),
            self.class_target(today.y, "live"),
        ];
        if today.m == 1 {
            t.push(self.class_target(today.y - 1, "live"));
        }
        t
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        let first = SourceId::UsdaDairy.earliest().unwrap_or(Date {
            y: 2000,
            m: 1,
            d: 1,
        });
        let mut out: Vec<Target> = year_chunks(year, first)
            .into_iter()
            .map(|(from, to)| {
                Target::poll(
                    format!("dairy:2993:{from}"),
                    range_url(&self.base, 2993, "week_ending_date", from, to),
                    "ndpsr",
                )
            })
            .collect();
        out.push(self.class_target(year, "history"));
        out
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        match target.kind.as_str() {
            "ndpsr" => parse_ndpsr(body),
            "class" => parse_class(body),
            k => Err(ParseError(format!("bad target kind {k}"))),
        }
    }

    fn schedule(&self) -> Schedule {
        // NDPSR is dated Wednesday; class prices are announced monthly.
        Schedule {
            days: WEEKDAYS,
            from_min: 15 * 60,
            to_min: 23 * 60,
            inside: Duration::from_secs(3600),
            outside: Duration::from_secs(12 * 3600),
        }
    }

    fn live_window_days(&self) -> i64 {
        // Class prices are monthly; keep the year.
        400
    }

    fn revision_days(&self) -> u32 {
        400
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_and_exclusions() {
        assert_eq!(month_number("3"), Some(3));
        assert_eq!(month_number("March"), Some(3));
        assert_eq!(month_number("13"), None);
        assert!(exchange_quote("CME_Block_Price"));
        assert!(exchange_quote("butter_futures_close"));
        assert!(!exchange_quote("Butter_Price"));
    }
}
