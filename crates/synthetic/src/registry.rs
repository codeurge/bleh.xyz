//! Exposure registry: which exposures exist, their calendars and rules,
//! their spot sources, carry inputs, swap anchors and the rate curve
//! (method spec § 4).
//!
//! Loaded from a JSON file. The repository ships only synthetic example
//! entries (`fixtures/synthetic/synthetic-months/registry.json`); real
//! venue market IDs are added by the operator. Exposure IDs and names are
//! neutral and descriptive. The `tracks` text may name the economic
//! reference ("tracks the exposure of the front ... futures month") as a
//! fact, per the admission policy's naming rules.

use std::collections::BTreeSet;
use std::path::Path;

use mesh_types::synthetic::is_exposure_id;
use mesh_types::{InstrumentId, MethodId, Price, SyntheticFamily, VenueId};
use serde::{Deserialize, Serialize};

use crate::calendar::{Calendar, CalendarId, ExpiryRule, RollRule};
use crate::date::Date;
use crate::error::Error;

/// The registry file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    /// File format version (1).
    pub version: u32,
    /// The method this registry is for, `synthetic-months@1`.
    pub method: MethodId,
    /// Calendar exceptions.
    #[serde(default)]
    pub calendars: Vec<CalendarExceptions>,
    /// The rate curve shared by every exposure.
    pub curve: CurveConfig,
    /// Exposures.
    pub exposures: Vec<Exposure>,
}

/// Exceptions to a computed calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalendarExceptions {
    /// The calendar.
    pub calendar: CalendarId,
    /// Extra closures, `YYYY-MM-DD`.
    #[serde(default)]
    pub closures: Vec<String>,
    /// Scheduled holidays that are open, `YYYY-MM-DD`.
    #[serde(default)]
    pub openings: Vec<String>,
}

/// The rate curve's inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurveConfig {
    /// Overnight-rate series (`reference_value`, tenor `ON`): the front
    /// node and the daily fixings of short-rate months.
    pub overnight: InstrumentId,
    /// Point sets in priority order.
    pub sets: Vec<CurveSet>,
    /// Rates are stale when the newest curve date is more than this many
    /// fixing-calendar business days before the valuation date.
    #[serde(default = "default_rates_max_age")]
    pub max_age_business_days: u32,
}

const fn default_rates_max_age() -> u32 {
    3
}

/// One set of curve points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurveSet {
    /// Reference-value series (bills, par yields, zero yields, term
    /// rates); every tenor of the newest `as_of_date` of each series.
    Series {
        /// Name for logs.
        name: String,
        /// Series instruments.
        series: Vec<InstrumentId>,
    },
    /// Overnight-index swap prints (`swap_print`): median fixed rate per
    /// tenor over a trailing window.
    SwapPrints {
        /// Name for logs.
        name: String,
        /// Public-data source of the prints (`prov.venue`).
        venue: VenueId,
        /// Exact `underlier` text to match.
        underlier: String,
        /// Trailing window, hours.
        window_hours: u32,
    },
}

/// Where a spot source's price points in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpotTracks {
    /// The cash underlying (horizon 0).
    Cash,
    /// The front contract month (horizon: the front month's expiry).
    FrontMonth,
}

/// One spot source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotSource {
    /// Mesh instrument (trades, book snapshots, queue positions and
    /// liquidity grades of this instrument are read).
    pub instrument: InstrumentId,
    /// Factor from the source's quote to the exposure's units.
    #[serde(default = "one")]
    pub multiplier: Price,
}

fn one() -> Price {
    Price::new(1, 0).unwrap_or_else(|_| unreachable_price())
}

fn unreachable_price() -> Price {
    // Price::new(1, 0) never fails; keep a total fallback anyway.
    Price::parse_exact("1").unwrap_or_else(|_| unreachable_price())
}

/// Spot composite settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotConfig {
    /// What the sources' prices represent.
    pub tracks: SpotTracks,
    /// Sources.
    pub sources: Vec<SpotSource>,
    /// A source observation older than this is stale, seconds.
    #[serde(default = "default_spot_age")]
    pub max_age_secs: u32,
    /// With no fresh source, the freshest source up to this age is used
    /// (flag `spot_stale`), seconds.
    #[serde(default = "default_spot_hard_age")]
    pub hard_max_age_secs: u32,
    /// Sources further than this from the median are outliers, basis
    /// points.
    #[serde(default = "default_dispersion")]
    pub max_dispersion_bps: u32,
}

const fn default_spot_age() -> u32 {
    120
}
const fn default_spot_hard_age() -> u32 {
    86_400
}
const fn default_dispersion() -> u32 {
    100
}

/// Carry input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarryConfig {
    /// Observed series (`reference_value`, percent, continuously
    /// compounded), preferred when present.
    #[serde(default)]
    pub series: Option<InstrumentId>,
    /// Estimate, percent, continuously compounded (flag
    /// `carry_estimated`).
    #[serde(default)]
    pub estimate: Option<Price>,
    /// Where the estimate comes from (provenance text).
    #[serde(default)]
    pub note: Option<String>,
}

/// Swap-print forward anchors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorConfig {
    /// Public-data source of the prints (`prov.venue`).
    pub venue: VenueId,
    /// Exact `underlier` text to match.
    pub underlier: String,
    /// Trailing window, hours.
    pub window_hours: u32,
    /// The newest anchor print is stale after this many hours.
    pub max_age_hours: u32,
    /// Factor from the print price to the exposure's units.
    #[serde(default = "one")]
    pub multiplier: Price,
}

/// Short-rate reference period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortRateKind {
    /// Compounded over the quarter from the third Wednesday of the
    /// contract month to the third Wednesday three months later.
    CompoundedQuarter,
    /// Arithmetic average over the calendar days of the contract month.
    AverageMonth,
}

/// One exposure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    /// Neutral exposure ID, such as `us-large-cap-index`.
    pub id: String,
    /// Neutral display name.
    pub name: String,
    /// The economic reference, stated as a fact.
    pub tracks: String,
    /// Family.
    pub family: SyntheticFamily,
    /// Business-day calendar of the rulebook.
    pub calendar: CalendarId,
    /// Contract month numbers in the cycle, such as `[3, 6, 9, 12]`.
    pub cycle: Vec<u32>,
    /// Months valued at once.
    pub months_listed: u32,
    /// Last trading day rule.
    pub expiry: ExpiryRule,
    /// Settlement time on the last trading day, New York, `HH:MM`.
    pub settlement_time: String,
    /// Roll rule.
    pub roll: RollRule,
    /// Fractional digits of `fair_value` and `spot`.
    pub price_scale: u8,
    /// Spot composite (required except for short-rate exposures).
    #[serde(default)]
    pub spot: Option<SpotConfig>,
    /// Carry.
    #[serde(default)]
    pub carry: Option<CarryConfig>,
    /// Swap anchors.
    #[serde(default)]
    pub anchors: Option<AnchorConfig>,
    /// Short-rate reference period (short-rate exposures only).
    #[serde(default)]
    pub short_rate: Option<ShortRateKind>,
}

impl Exposure {
    /// Settlement time as `(hour, minute)`.
    ///
    /// # Errors
    ///
    /// Fails on a malformed time or one before 03:00 (inside a daylight
    /// saving transition).
    pub fn settlement_hm(&self) -> Result<(u32, u32), Error> {
        let bad = || Error::Registry(format!("{}: bad settlement_time", self.id));
        let (h, m) = self.settlement_time.split_once(':').ok_or_else(bad)?;
        if h.len() != 2 || m.len() != 2 {
            return Err(bad());
        }
        let h: u32 = h.parse().map_err(|_| bad())?;
        let m: u32 = m.parse().map_err(|_| bad())?;
        if !(3..24).contains(&h) || m >= 60 {
            return Err(bad());
        }
        Ok((h, m))
    }
}

impl Registry {
    /// Reads and checks a registry file.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read or parsed, or does not check.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Registry(format!("{}: {e}", path.display())))?;
        Self::from_json(&text)
    }

    /// Parses and checks registry JSON.
    ///
    /// # Errors
    ///
    /// Fails if the text does not parse or the registry does not check.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        let r: Self = serde_json::from_str(text).map_err(|e| Error::Registry(e.to_string()))?;
        r.check()?;
        Ok(r)
    }

    /// The calendar `id` with its configured exceptions.
    #[must_use]
    pub fn calendar(&self, id: CalendarId) -> Calendar {
        let parse =
            |v: &[String]| -> Vec<Date> { v.iter().filter_map(|s| Date::parse(s)).collect() };
        let (closures, openings) = self.calendars.iter().filter(|c| c.calendar == id).fold(
            (Vec::new(), Vec::new()),
            |(mut c, mut o), x| {
                c.extend(parse(&x.closures));
                o.extend(parse(&x.openings));
                (c, o)
            },
        );
        Calendar::with_exceptions(id, closures, openings)
    }

    /// Checks the registry's consistency.
    ///
    /// # Errors
    ///
    /// Fails on a wrong version or method, duplicate or malformed IDs, an
    /// empty or invalid cycle, a family without its required section, or
    /// malformed dates and times.
    pub fn check(&self) -> Result<(), Error> {
        let err = |s: String| Err(Error::Registry(s));
        if self.version != 1 {
            return err(format!("unsupported registry version {}", self.version));
        }
        if self.method.as_str() != crate::METHOD {
            return err(format!(
                "registry is for {}, this engine implements {}",
                self.method,
                crate::METHOD
            ));
        }
        for c in &self.calendars {
            for d in c.closures.iter().chain(&c.openings) {
                if Date::parse(d).is_none() {
                    return err(format!("bad calendar date {d:?}"));
                }
            }
        }
        if self.curve.sets.is_empty() {
            return err("the curve needs at least one point set".into());
        }
        let mut ids = BTreeSet::new();
        for e in &self.exposures {
            if !is_exposure_id(&e.id) {
                return err(format!("bad exposure id {:?}", e.id));
            }
            if !ids.insert(e.id.as_str()) {
                return err(format!("duplicate exposure {}", e.id));
            }
            if e.cycle.is_empty() || e.cycle.iter().any(|m| !(1..=12).contains(m)) {
                return err(format!("{}: cycle must hold month numbers 1-12", e.id));
            }
            if e.months_listed == 0 || e.months_listed > 120 {
                return err(format!("{}: months_listed must be 1-120", e.id));
            }
            if e.price_scale > 12 {
                return err(format!("{}: price_scale above 12", e.id));
            }
            e.settlement_hm()?;
            let short = e.family == SyntheticFamily::ShortRate;
            if short != e.short_rate.is_some() {
                return err(format!(
                    "{}: short_rate is required for, and only for, short-rate exposures",
                    e.id
                ));
            }
            if !short && e.spot.as_ref().is_none_or(|s| s.sources.is_empty()) {
                return err(format!("{}: spot sources are required", e.id));
            }
            if short && (e.spot.is_some() || e.carry.is_some() || e.anchors.is_some()) {
                return err(format!(
                    "{}: short-rate exposures take no spot, carry or anchors",
                    e.id
                ));
            }
            if let Some(s) = &e.spot
                && s.hard_max_age_secs < s.max_age_secs
            {
                return err(format!("{}: hard_max_age_secs below max_age_secs", e.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = r#"{
      "version": 1,
      "method": "synthetic-months@1",
      "curve": {
        "overnight": "example-rates:ref:OVERNIGHT",
        "sets": [{"type": "series", "name": "t", "series": ["example-rates:ref:CURVE"]}]
      },
      "exposures": [{
        "id": "example-index", "name": "Example index months",
        "tracks": "Synthetic example.", "family": "equity_index", "calendar": "us_trading",
        "cycle": [3, 6, 9, 12], "months_listed": 2,
        "expiry": {"rule": "nth_weekday", "n": 3, "weekday": "fri", "month_offset": 0, "business_days_before": 0},
        "settlement_time": "09:30",
        "roll": {"business_days_before": 8, "reference": "last_trade"},
        "price_scale": 2,
        "spot": {"tracks": "cash", "sources": [{"instrument": "example-a:perp:IDX"}]}
      }]
    }"#;

    #[test]
    fn loads_and_checks() {
        let r = Registry::from_json(MIN).unwrap();
        assert_eq!(r.exposures[0].settlement_hm().unwrap(), (9, 30));
        let s = r.exposures[0].spot.as_ref().unwrap();
        assert_eq!(s.max_age_secs, 120);
        assert_eq!(s.sources[0].multiplier.to_string(), "1");
        assert_eq!(r.curve.max_age_business_days, 3);
    }

    #[test]
    fn rejects_bad_registries() {
        for (from, to) in [
            (r#""version": 1"#, r#""version": 2"#),
            ("synthetic-months@1", "synthetic-months@9"),
            (r#""cycle": [3, 6, 9, 12]"#, r#""cycle": [13]"#),
            (r#""09:30""#, r#""9:30""#),
            (r#""09:30""#, r#""02:30""#),
            (r#""family": "equity_index""#, r#""family": "short_rate""#),
            (r#""id": "example-index""#, r#""id": "Example""#),
            (r#""months_listed": 2"#, r#""months_listed": 0"#),
            (r#""price_scale": 2"#, r#""price_scale": 2, "extra": 1"#),
        ] {
            let text = MIN.replace(from, to);
            assert!(Registry::from_json(&text).is_err(), "{to}");
        }
    }

    #[test]
    fn calendar_exceptions_apply() {
        let text = MIN.replace(
            r#""curve""#,
            r#""calendars": [{"calendar": "us_trading", "closures": ["2026-01-09"]}], "curve""#,
        );
        let r = Registry::from_json(&text).unwrap();
        let c = r.calendar(CalendarId::UsTrading);
        assert!(c.is_holiday(Date::parse("2026-01-09").unwrap()));
        assert!(
            !r.calendar(CalendarId::UsGovernmentSecurities)
                .is_holiday(Date::parse("2026-01-09").unwrap())
        );
    }
}
