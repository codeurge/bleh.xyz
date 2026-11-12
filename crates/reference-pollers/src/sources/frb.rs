//! Federal Reserve Board H.15 (ADM-0008) and H.10 (ADM-0009), from the
//! whole-release SDMX-ML zip linked on each release page.
//!
//! Each `Series` element carries `SERIES_NAME`; its `Obs` children carry
//! `TIME_PERIOD` and `OBS_VALUE` (`ND` is no data). Series are admitted by
//! stem, from an explicit allowlist; every other series is dropped
//! (counted as `excluded` when the admission names it, as unknown
//! otherwise, so a new series is reviewed before it is emitted). Only
//! business-daily series (`.B`) are emitted; weekly and monthly averages
//! can be recomputed.
//!
//! ## H.15 (`frb-h15:ref:<SERIES_NAME>`)
//!
//! | Stem | Measure |
//! |---|---|
//! | `RIFSPFF_N` (EFFR; carries the NY Fed notice) | rate, `ON`, simple act/360 |
//! | `RIFSPPNAAD<n>_N`, `RIFSPPFAAD<n>_N` (AA commercial paper, `n` days) | rate, `<n>D`, simple act/360 |
//! | `RIFSRP_F02_N`, `H0.RIFSRD_F02_N` (discount window) | rate, `ON`, simple act/360 |
//! | `RIFSGFS<u><n>_N` (bills, secondary market) | rate, discount act/360 |
//! | `H1.RIFSGFPAM03_N`, `H0.RIFSGFPAM06_N`, `H0.RIFSGFPAY01_N` (bill auction high rates) | rate, discount act/360 |
//! | `RIFLGFC<u><n>_N`, `RIFLGFC<u><n>_XII_N`, `H0.RIFLGFCY20_N` (constant maturities) | par yield, semi-annual |
//!
//! Admitted series without a tenor (bank prime, the facility rates, the
//! long-term averages and composites) are counted as skipped: a
//! `reference_value` rate needs a tenor or a maturity.
//!
//! ## H.10 (`frb-h10:ref:<SERIES_NAME>`, `$` written `@`)
//!
//! `RXI$US_N.B.{AL,EU,NZ,UK}` (US dollars per unit, currency `USD`) and
//! `RXI_N.B.<code>` for the other 19 table currencies (units per US
//! dollar, currency the foreign ISO code), as prices. The dollar indexes
//! are excluded.

use std::time::Duration;

use mesh_types::{Compounding, DayCount, ReferenceMeasure};

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS, unzip};
use crate::num;
use crate::obs::{Obs, RefObs, tenor_slot};
use crate::source::SourceId;
use crate::time::Date;
use crate::xml::{self, Node};

/// Default Board base.
pub const BASE: &str = "https://www.federalreserve.gov";

/// Which release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// H.15 Selected Interest Rates.
    H15,
    /// H.10 Foreign Exchange Rates.
    H10,
}

/// The Board reader (one per release).
#[derive(Debug, Clone)]
pub struct FrbReader {
    release: Release,
    base: String,
}

impl FrbReader {
    /// The H.15 reader (base key `frb`).
    #[must_use]
    pub fn h15(bases: &Bases) -> Self {
        Self {
            release: Release::H15,
            base: bases.get("frb", BASE),
        }
    }

    /// The H.10 reader (base key `frb`).
    #[must_use]
    pub fn h10(bases: &Bases) -> Self {
        Self {
            release: Release::H10,
            base: bases.get("frb", BASE),
        }
    }

    fn target(&self, tag: &str) -> Target {
        let (rel, file) = match self.release {
            Release::H15 => ("h15", "FRB_h15_xml.zip"),
            Release::H10 => ("h10", "FRB_h10_xml.zip"),
        };
        Target::poll(
            format!("frb:{rel}:{tag}"),
            format!("{}/releases/{rel}/data/{file}", self.base),
            rel,
        )
    }
}

/// H.15 stems the admission excludes (vendor, benchmark administrator or
/// undocumented sources).
const H15_EXCLUDED: [&str; 11] = [
    "RILSPDEP",
    "RIFLDIY",
    "RIMLPAAAR_N",
    "RIMLPBAAR_N",
    "RIMLPSL",
    "RIMLPMR",
    "H0.RIFSPDCNSM",
    "H0.RIFSPPCM",
    "H0.RIFSPPFM",
    "H0.RIFSPABM",
    "H1.RIFSPABM",
];

/// Admitted H.15 stems with no tenor, counted as skipped.
const H15_NO_TENOR: [&str; 6] = [
    "RIFSPBLP_N",
    "RIFSPPCU_N",
    "RIFSPPCUS_N",
    "RIFLGFL_XII_N",
    "H0.RIFLGFL_N",
    "H1.RIFLGFY10P_N",
];

/// H.10 table currencies by series code.
pub const H10_USD_PER_UNIT: [(&str, &str); 4] =
    [("AL", "AUD"), ("EU", "EUR"), ("NZ", "NZD"), ("UK", "GBP")];
/// H.10 currencies quoted in units per US dollar.
pub const H10_UNITS_PER_USD: [(&str, &str); 19] = [
    ("BZ", "BRL"),
    ("CA", "CAD"),
    ("CH", "CNY"),
    ("DN", "DKK"),
    ("HK", "HKD"),
    ("IN", "INR"),
    ("JA", "JPY"),
    ("MA", "MYR"),
    ("MX", "MXN"),
    ("NO", "NOK"),
    ("SI", "SGD"),
    ("SF", "ZAR"),
    ("KO", "KRW"),
    ("SL", "LKR"),
    ("SD", "SEK"),
    ("SZ", "CHF"),
    ("TA", "TWD"),
    ("TH", "THB"),
    ("VE", "VEB"),
];

/// Splits a series name into stem, frequency and optional code:
/// `RIFLGFCY10_N.B` is `("RIFLGFCY10_N", "B", None)`, `H0.RIFSRD_F02_N.B`
/// keeps its `H0.` prefix, `RXI_N.B.JA` has code `JA`.
#[must_use]
pub fn split_series(name: &str) -> Option<(String, String, Option<String>)> {
    let parts: Vec<&str> = name.split('.').collect();
    let (stem, rest) = if matches!(parts.first(), Some(&"H0" | &"H1" | &"V0")) && parts.len() >= 3 {
        (format!("{}.{}", parts[0], parts[1]), &parts[2..])
    } else {
        ((*parts.first()?).to_owned(), &parts[1..])
    };
    let freq = (*rest.first()?).to_owned();
    Some((stem, freq, rest.get(1).map(|c| (*c).to_owned())))
}

/// `<u><n>` after `prefix` and before `suffix` as a tenor (`Y10` is
/// `10Y`, `M03` is `3M`, `W04` is `4W`).
fn unit_tenor(stem: &str, prefix: &str, suffix: &str) -> Option<String> {
    let mid = stem.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let (u, n) = mid.split_at(1);
    let n: u32 = n.parse().ok()?;
    matches!(u, "D" | "W" | "M" | "Y")
        .then(|| format!("{n}{u}"))
        .filter(|_| (1..=999).contains(&n))
}

/// Conventions of an admitted H.15 stem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H15Kind {
    /// A rate or yield with these conventions.
    Rate {
        /// Measure.
        measure: ReferenceMeasure,
        /// Tenor.
        tenor: String,
        /// Compounding.
        compounding: Compounding,
        /// Day count.
        day_count: Option<DayCount>,
    },
    /// Admitted, but without a tenor.
    NoTenor,
    /// Excluded by the admission.
    Excluded,
    /// Not on the allowlist (reviewed before it is ever emitted).
    Unknown,
}

/// Classifies an H.15 stem.
#[must_use]
pub fn h15_kind(stem: &str) -> H15Kind {
    let simple = |tenor: String| H15Kind::Rate {
        measure: ReferenceMeasure::Rate,
        tenor,
        compounding: Compounding::Simple,
        day_count: Some(DayCount::Act360),
    };
    let discount = |tenor: String| H15Kind::Rate {
        measure: ReferenceMeasure::Rate,
        tenor,
        compounding: Compounding::Discount,
        day_count: Some(DayCount::Act360),
    };
    let par = |tenor: String| H15Kind::Rate {
        measure: ReferenceMeasure::ParYield,
        tenor,
        compounding: Compounding::SemiAnnual,
        day_count: None,
    };
    if H15_EXCLUDED.iter().any(|p| stem.starts_with(p)) {
        return H15Kind::Excluded;
    }
    if H15_NO_TENOR.contains(&stem) {
        return H15Kind::NoTenor;
    }
    match stem {
        "RIFSPFF_N" | "RIFSRP_F02_N" | "H0.RIFSRD_F02_N" => return simple("ON".into()),
        "H1.RIFSGFPAM03_N" => return discount("3M".into()),
        "H0.RIFSGFPAM06_N" => return discount("6M".into()),
        "H0.RIFSGFPAY01_N" => return discount("1Y".into()),
        "H0.RIFLGFCY20_N" => return par("20Y".into()),
        _ => {}
    }
    for p in ["RIFSPPNAAD", "RIFSPPFAAD"] {
        if let Some(n) = stem.strip_prefix(p).and_then(|r| r.strip_suffix("_N")) {
            return match n.parse::<u32>() {
                Ok(d) if (1..=999).contains(&d) => simple(format!("{d}D")),
                _ => H15Kind::Unknown,
            };
        }
    }
    if let Some(t) = unit_tenor(stem, "RIFSGFS", "_N") {
        return discount(t);
    }
    if let Some(t) =
        unit_tenor(stem, "RIFLGFC", "_XII_N").or_else(|| unit_tenor(stem, "RIFLGFC", "_N"))
    {
        return par(t);
    }
    H15Kind::Unknown
}

/// Reads every series of an SDMX data file: `(series name, attributes,
/// observations as (period, value))`, streaming.
///
/// # Errors
///
/// Fails on malformed XML or an error from `f`.
pub fn sdmx_series(
    xml_bytes: &[u8],
    mut f: impl FnMut(&str, &[(String, String)], &[(String, String)]) -> Result<(), ParseError>,
) -> Result<(), ParseError> {
    let mut name: Option<String> = None;
    let mut attrs: Vec<(String, String)> = Vec::new();
    let mut obs: Vec<(String, String)> = Vec::new();
    xml::walk::<ParseError>(xml_bytes, |n| {
        match n {
            Node::Start {
                name: "Series",
                attrs: a,
            } => {
                name = xml::attr(a, "SERIES_NAME").map(str::to_owned);
                attrs = a.to_vec();
                obs.clear();
            }
            Node::Start {
                name: "Obs",
                attrs: a,
            } if name.is_some() => {
                if let (Some(p), Some(v)) = (xml::attr(a, "TIME_PERIOD"), xml::attr(a, "OBS_VALUE"))
                {
                    obs.push((p.to_owned(), v.to_owned()));
                }
            }
            Node::End { name: "Series" } => {
                if let Some(s) = name.take() {
                    f(&s, &attrs, &obs)?;
                }
                obs.clear();
            }
            _ => {}
        }
        Ok(())
    })
}

fn data_member(body: &[u8]) -> Result<Vec<u8>, ParseError> {
    if !body.starts_with(b"PK\x03\x04") {
        return Ok(body.to_vec());
    }
    let members = unzip(body)?;
    let pick = members
        .iter()
        .position(|(n, _)| n.to_ascii_lowercase().ends_with("_data.xml"))
        .or_else(|| {
            members
                .iter()
                .enumerate()
                .filter(|(_, (n, _))| n.to_ascii_lowercase().ends_with(".xml"))
                .max_by_key(|(_, (_, b))| b.len())
                .map(|(i, _)| i)
        })
        .ok_or_else(|| ParseError("no data XML in the release zip".into()))?;
    Ok(members
        .into_iter()
        .nth(pick)
        .map(|(_, b)| b)
        .unwrap_or_default())
}

/// Parses an H.15 or H.10 release file (zip or bare XML).
///
/// # Errors
///
/// Fails on a malformed archive or XML.
pub fn parse_release(release: Release, body: &[u8]) -> Result<Parsed, ParseError> {
    let xml_bytes = data_member(body)?;
    let mut out = Parsed::default();
    sdmx_series(&xml_bytes, |name, _attrs, obs| {
        let Some((stem, freq, code)) = split_series(name) else {
            out.unknown.insert(name.to_owned());
            return Ok(());
        };
        let make: Box<dyn Fn(mesh_types::Price, Date) -> RefObs> = match release {
            Release::H15 => match h15_kind(&stem) {
                H15Kind::Rate {
                    measure,
                    tenor,
                    compounding,
                    day_count,
                } => {
                    if freq != "B" {
                        out.skip("other_frequency");
                        return Ok(());
                    }
                    let series = num::symbol_segment(name);
                    Box::new(move |v, d| {
                        let mut r = RefObs::rate(
                            series.clone(),
                            measure,
                            v,
                            &tenor,
                            compounding,
                            day_count,
                            d,
                        );
                        r.slot = tenor_slot(&tenor);
                        r
                    })
                }
                H15Kind::NoTenor => {
                    out.skip("unsupported_no_tenor");
                    return Ok(());
                }
                H15Kind::Excluded => {
                    out.skip("excluded");
                    return Ok(());
                }
                H15Kind::Unknown => {
                    out.unknown.insert(stem);
                    return Ok(());
                }
            },
            Release::H10 => {
                let code = code.unwrap_or_default();
                let currency = match stem.as_str() {
                    "RXI$US_N" => H10_USD_PER_UNIT
                        .iter()
                        .find(|(c, _)| *c == code)
                        .map(|_| "USD".to_owned()),
                    "RXI_N" => H10_UNITS_PER_USD
                        .iter()
                        .find(|(c, _)| *c == code)
                        .map(|(_, iso)| (*iso).to_owned()),
                    s if s.starts_with("JRXWTF")
                        || s.starts_with("V0.")
                        || s.starts_with("JRX") =>
                    {
                        out.skip("excluded");
                        return Ok(());
                    }
                    _ => None,
                };
                let Some(currency) = currency else {
                    out.unknown.insert(name.to_owned());
                    return Ok(());
                };
                if freq != "B" {
                    out.skip("other_frequency");
                    return Ok(());
                }
                let series = num::symbol_segment(name);
                Box::new(move |v, d| {
                    let mut r = RefObs::usd_price(series.clone(), v, d);
                    r.currency = Some(currency.clone());
                    r
                })
            }
        };
        for (period, value) in obs {
            let Some(date) = Date::parse_iso(period) else {
                out.skip("malformed");
                continue;
            };
            match num::decimal(value) {
                Ok(Some(v)) => out.obs.push(Obs::Ref(make(v, date))),
                Ok(None) => out.skip("missing"),
                Err(_) => out.skip("malformed"),
            }
        }
        Ok(())
    })?;
    Ok(out)
}

impl Reader for FrbReader {
    fn id(&self) -> SourceId {
        match self.release {
            Release::H15 => SourceId::FrbH15,
            Release::H10 => SourceId::FrbH10,
        }
    }

    fn live_targets(&self, _today: Date) -> Vec<Target> {
        vec![self.target("live")]
    }

    fn backfill_targets(&self, _year: i32) -> Vec<Target> {
        // One file holds the whole history; the backfill reads it once.
        vec![self.target("history")]
    }

    fn parse(&self, _target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        parse_release(self.release, body)
    }

    fn schedule(&self) -> Schedule {
        match self.release {
            // H.15 posts each business day at about 4:15 p.m. ET.
            Release::H15 => Schedule {
                days: WEEKDAYS,
                from_min: 20 * 60 + 30,
                to_min: 23 * 60 + 59,
                inside: Duration::from_secs(1800),
                outside: Duration::from_secs(6 * 3600),
            },
            // H.10 posts on Mondays for the previous week.
            Release::H10 => Schedule {
                days: 0b0000_0011,
                from_min: 20 * 60 + 30,
                to_min: 23 * 60 + 59,
                inside: Duration::from_secs(1800),
                outside: Duration::from_secs(12 * 3600),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems() {
        assert_eq!(
            split_series("RIFLGFCY10_N.B"),
            Some(("RIFLGFCY10_N".into(), "B".into(), None))
        );
        assert_eq!(
            split_series("H0.RIFSRD_F02_N.B"),
            Some(("H0.RIFSRD_F02_N".into(), "B".into(), None))
        );
        assert_eq!(
            split_series("RXI$US_N.B.EU"),
            Some(("RXI$US_N".into(), "B".into(), Some("EU".into())))
        );
        assert!(
            matches!(h15_kind("RIFLGFCY10_N"), H15Kind::Rate { ref tenor, .. } if tenor == "10Y")
        );
        assert!(
            matches!(h15_kind("RIFLGFCY05_XII_N"), H15Kind::Rate { ref tenor, .. } if tenor == "5Y")
        );
        assert!(matches!(
            h15_kind("RIFSGFSW04_N"),
            H15Kind::Rate {
                compounding: Compounding::Discount,
                ..
            }
        ));
        assert!(
            matches!(h15_kind("RIFSPPNAAD30_N"), H15Kind::Rate { ref tenor, .. } if tenor == "30D")
        );
        assert_eq!(h15_kind("RIFLDIY10_N"), H15Kind::Excluded);
        assert_eq!(h15_kind("RILSPDEPM03_N"), H15Kind::Excluded);
        assert_eq!(h15_kind("RIMLPAAAR_N"), H15Kind::Excluded);
        assert_eq!(h15_kind("H0.RIFSPPCM01_N"), H15Kind::Excluded);
        assert_eq!(h15_kind("RIFSPBLP_N"), H15Kind::NoTenor);
        assert_eq!(h15_kind("RIFNEWSERIES_N"), H15Kind::Unknown);
    }
}
