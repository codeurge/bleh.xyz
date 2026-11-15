//! USDA AMS Market News feeder and stocker cattle (ADM-0012, feeder fields
//! only) from the static report files under `ams.usda.gov/mnreports/`.
//! The MyMarketNews API is not an admitted channel and is never used, and
//! grain reports (rejected) are never read.
//!
//! The national weekly summary (`sj_ls850.txt`) is fixed-layout text.
//! The parser keeps only table rows and drops everything else, so the
//! narrative (which quotes futures moves) never reaches a record:
//!
//! - A **section line** names the class (`Steers`, `Heifers`, `Bulls`)
//!   and the frame and muscle grade (`Medium and Large 1`).
//! - A **sale row** is `head  low-high  avg_wt  price[-price]  avg_price
//!   [note]`, with prices in dollars per hundredweight.
//! - Any row the report labels basis, futures or formula priced is
//!   dropped as excluded.
//!
//! Series `LS850.<CLASS>.<GRADE>.<LOW>_<HIGH>[.<NOTE>].{avg_price,
//! price_low,price_high}` under `usda-mn:ref:`, dated by the report date
//! (`Week Ending:` or `Report Date:`, else the first date in the heading).
//! Each kept sale row's head count (admitted, ADM-0012) is
//! `<...>.head_count`, a `flow` in `head` (ADR 0023); an excluded row is
//! dropped whole, head count included. The receipts block is not a table
//! row and is not read. The files hold only the latest issue, so there is
//! no backfill; the live poller alerts when the file's date is older than
//! ten days (observed on 2026-11-12). PDF issues are not parsed.

use std::time::Duration;

use mesh_types::{QuantityUnit, ReferenceMeasure};

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::Date;

/// Default AMS base.
pub const BASE: &str = "https://www.ams.usda.gov";

const CLASSES: [&str; 3] = ["Steers", "Heifers", "Bulls"];
const EXCLUDED_WORDS: [&str; 3] = ["basis", "futures", "formula"];

fn report_date(text: &str) -> Option<Date> {
    for line in text.lines().take(40) {
        for label in ["Week Ending:", "Report Date:"] {
            if let Some(i) = line.find(label)
                && let Some(d) = line[i + label.len()..]
                    .split_whitespace()
                    .next()
                    .and_then(Date::parse_us)
            {
                return Some(d);
            }
        }
    }
    text.lines()
        .take(15)
        .flat_map(str::split_whitespace)
        .find_map(Date::parse_us)
}

fn is_range(s: &str) -> Option<(&str, &str)> {
    let (a, b) = s.split_once('-')?;
    (!a.is_empty()
        && !b.is_empty()
        && a.bytes()
            .chain(b.bytes())
            .all(|c| c.is_ascii_digit() || c == b'.'))
    .then_some((a, b))
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Parses a feeder cattle summary text file.
///
/// # Errors
///
/// Fails if the file has no report date.
pub fn parse_summary(body: &[u8]) -> Result<Parsed, ParseError> {
    let text = String::from_utf8_lossy(body);
    let as_of = report_date(&text).ok_or_else(|| ParseError("no report date".into()))?;
    let mut out = Parsed::default();
    let mut section: Option<(String, String)> = None;
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.is_empty() {
            continue;
        }
        let is_row = t.len() >= 5 && digits(t[0]) && is_range(t[1]).is_some() && digits(t[2]);
        if !is_row {
            // A section line names a class and has no sale-row shape.
            if let Some(ci) = t.iter().position(|w| CLASSES.contains(w))
                && t.len() <= 12
                && !t.iter().any(|w| digits(w) && w.len() >= 3)
            {
                let grade = t[ci + 1..].join(" ");
                let grade = if grade.is_empty() {
                    "UNGRADED".to_owned()
                } else {
                    num::slug(&grade)
                };
                section = Some((num::slug(t[ci]), grade));
            }
            continue;
        }
        let Some((class, grade)) = section.clone() else {
            out.skip("malformed");
            continue;
        };
        let lower = line.to_ascii_lowercase();
        if EXCLUDED_WORDS.iter().any(|w| lower.contains(w)) {
            out.skip("excluded");
            continue;
        }
        let Some((wlo, whi)) = is_range(t[1]) else {
            out.skip("malformed");
            continue;
        };
        let (plo, phi) = is_range(t[3]).unwrap_or((t[3], t[3]));
        let note = num::slug(&t[5..].join(" "));
        let mut key = format!("LS850.{class}.{grade}.{wlo}_{whi}");
        if !note.is_empty() {
            key.push('.');
            key.push_str(&note);
        }
        let mut any = false;
        for (field, s) in [("avg_price", t[4]), ("price_low", plo), ("price_high", phi)] {
            match num::decimal(s) {
                Ok(Some(v)) => {
                    out.obs.push(Obs::Ref(RefObs::usd_price(
                        format!("{key}.{field}"),
                        v,
                        as_of,
                    )));
                    any = true;
                }
                Ok(None) => {}
                Err(_) => out.skip("malformed"),
            }
        }
        if !any {
            out.skip("missing");
        }
        // t[0] is digits by the row shape.
        if let Ok(Some(head)) = num::decimal(t[0]) {
            out.obs.push(Obs::Ref(RefObs::quantity(
                format!("{key}.head_count"),
                ReferenceMeasure::Flow,
                head,
                QuantityUnit::Head,
                as_of,
            )));
        }
    }
    // Distinct sale rows that share a series on one date get distinct
    // slots, in file order.
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for o in &mut out.obs {
        if let Obs::Ref(r) = o {
            let n = seen.entry(r.series.clone()).or_default();
            r.slot = *n;
            *n += 1;
        }
    }
    Ok(out)
}

/// The feeder cattle reader.
#[derive(Debug, Clone)]
pub struct FeederReader {
    base: String,
}

impl FeederReader {
    /// A reader using `bases` (key `ams`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("ams", BASE),
        }
    }
}

impl Reader for FeederReader {
    fn id(&self) -> SourceId {
        SourceId::UsdaFeeder
    }

    fn live_targets(&self, _today: Date) -> Vec<Target> {
        vec![Target::poll(
            "feeder:sj_ls850",
            format!("{}/mnreports/sj_ls850.txt", self.base),
            "summary",
        )]
    }

    fn backfill_targets(&self, _year: i32) -> Vec<Target> {
        Vec::new()
    }

    fn parse(&self, _target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        parse_summary(body)
    }

    fn schedule(&self) -> Schedule {
        Schedule {
            days: WEEKDAYS,
            from_min: 18 * 60,
            to_min: 23 * 60 + 59,
            inside: Duration::from_secs(3 * 3600),
            outside: Duration::from_secs(12 * 3600),
        }
    }

    fn live_window_days(&self) -> i64 {
        60
    }

    fn freshness_days(&self) -> Option<i64> {
        Some(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(is_range("500-600"), Some(("500", "600")));
        assert_eq!(is_range("245.00-262.50"), Some(("245.00", "262.50")));
        assert_eq!(is_range("-5"), None);
        assert_eq!(
            report_date("Header\n   Week Ending: 11/06/2026\n")
                .unwrap()
                .to_string(),
            "2026-11-06"
        );
    }
}
