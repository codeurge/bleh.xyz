//! An instrument registry derived from the admitted fills themselves
//! (ADR 0019, method [`METHOD`]).
//!
//! `process` needs a registry (`--registry`) and skips events whose coin is
//! not in it. Live registration reads market metadata from the project's
//! own node; a backfill run without a node has no admitted metadata source,
//! so this module derives the registry from the data being backfilled:
//!
//! 1. Every object of the selection is read from the raw tier, decompressed
//!    and filtered with the same filter `process` uses, so only admitted,
//!    book-matched events count (spot events are already removed).
//! 2. Every fill or trade object in the filtered lines (a JSON object with
//!    string `coin`, `px` and `sz`) is observed: the coin, the number of
//!    fractional digits of its price and size (trailing zeros ignored), and
//!    the day.
//! 3. Each coin is classified from its symbol
//!    ([`hl_node_reader::assumptions::classify_coin`]): plain names are core
//!    perpetuals, `dex:NAME` are HIP-3 perpetuals (namespace and the
//!    `oracle_anchored_venue` flag), `#N` are outcome markets, and spot
//!    (`@N`, pairs) is dropped.
//! 4. The size scale is the largest number of size decimals seen for the
//!    coin over the selection. The price scale follows the venue's rule
//!    ([`InstrumentSpec::hyperliquid`]: `6 - size scale` for perpetuals;
//!    `8 - size scale` for outcome markets, the spot rule, since method v2
//!    and ADR 0021; v1 used a fixed 4 and left every outcome coin out).
//!    Since v3, price and size strings with floating-point noise (more
//!    than 15 significant digits, such as `"2946657.1000000001"`) are
//!    rounded to 15 first, as the reader does
//!    ([`hl_node_reader::assumptions::venue_decimal`], ADR 0022).
//! 5. Every observed price must fit the price scale. A coin where one does
//!    not (for example after a change of `szDecimals` within the period) is
//!    reported as a violation and **not** registered, so its events are
//!    counted as `unknown_coin` by `process` instead of being written with
//!    a wrong scale.
//!
//! Delisted coins are covered because they traded. When a node runs, its
//! metadata is authoritative: the registered spec is kept and conflicts are
//! reported (registry-v0 § 3).
//!
//! Observed decimals can be fewer than the venue's `szDecimals` (sizes are
//! written without trailing zeros), never more. Either way every size and
//! price of the period is represented exactly at the derived scales.

use std::collections::{BTreeMap, BTreeSet};

use hl_node_reader::assumptions::{CoinClass, classify_coin, outcome_price_scale, venue_decimal};
use hl_node_reader::markets::default_flags;
use mesh_types::{AdmissionId, InstrumentSpec, MarketClass, Registry};
use serde::Serialize;
use serde_json::Value;

use crate::buckets::{Dataset, Day};

/// Method name and version, recorded in every report (rule 5: a change to
/// the method gets a new version).
pub const METHOD: &str = "hl-backfill-registry/v3";

/// Largest perpetual size scale the venue's price rule allows.
const PERP_MAX_DECIMALS: u8 = 6;

/// Observations for one coin.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CoinStats {
    trades: u64,
    max_sz_decimals: u8,
    max_px_decimals: u8,
    first_day: Option<Day>,
    last_day: Option<Day>,
    datasets: BTreeSet<String>,
}

/// One registered coin in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoinReport {
    /// Venue symbol.
    pub coin: String,
    /// Mesh instrument ID.
    pub instrument: String,
    /// Market class.
    pub class: String,
    /// Fill or trade objects observed.
    pub trades: u64,
    /// First day the coin traded in the selection (`YYYY-MM-DD`).
    pub first_day: Option<String>,
    /// Last day the coin traded in the selection.
    pub last_day: Option<String>,
    /// Registered size scale (largest size decimals seen).
    pub size_scale: u8,
    /// Registered price scale.
    pub price_scale: u8,
    /// Largest price decimals seen (at most `price_scale`).
    pub max_price_decimals: u8,
    /// Datasets the coin was seen in.
    pub datasets: Vec<String>,
}

/// A coin that could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// Venue symbol.
    pub coin: String,
    /// Why.
    pub reason: String,
}

/// What a registry run found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RegistryReport {
    /// Method and version ([`METHOD`]).
    pub method: String,
    /// Objects read.
    pub objects: u64,
    /// Objects that failed to read, decompress or filter, with the error.
    /// Their coins are not observed, so a run with failures is incomplete.
    pub failed: Vec<(String, String)>,
    /// Fill or trade objects observed.
    pub trades_observed: u64,
    /// Price or size strings that are not plain decimals (not observed).
    pub malformed_values: u64,
    /// Price or size strings with floating-point noise (more than 15
    /// significant digits), observed after rounding to 15 (ADR 0022).
    pub float_noise_rounded: u64,
    /// Spot coins seen and dropped (outside ADM-0001's scope). Normally
    /// empty: the filter removes spot events first.
    pub spot_dropped: Vec<String>,
    /// Registered coins, by symbol.
    pub coins: Vec<CoinReport>,
    /// Coins not registered, with the reason.
    pub violations: Vec<Violation>,
}

impl RegistryReport {
    /// True if every object was read and every observed coin registered.
    #[must_use]
    pub fn clean(&self) -> bool {
        self.failed.is_empty() && self.violations.is_empty()
    }
}

/// Number of fractional digits of a plain decimal string, ignoring trailing
/// zeros (`"100.50"` has 1, `"7"` and `"7.0"` have 0). `None` if the string
/// is not a plain decimal (an optional `-`, digits, an optional `.` and
/// digits).
#[must_use]
pub fn decimals(s: &str) -> Option<u8> {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body, ""),
    };
    if int.is_empty() || !int.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) || (body.contains('.') && frac.is_empty()) {
        return None;
    }
    u8::try_from(frac.trim_end_matches('0').len()).ok()
}

/// Derives a registry from observed fills (see the module docs).
#[derive(Debug, Clone)]
pub struct RegistryBuilder {
    admission: AdmissionId,
    coins: BTreeMap<String, CoinStats>,
    report: RegistryReport,
}

impl RegistryBuilder {
    /// A builder whose specs carry `admission` (the backfill's, ADM-0003).
    #[must_use]
    pub fn new(admission: AdmissionId) -> Self {
        Self {
            admission,
            coins: BTreeMap::new(),
            report: RegistryReport {
                method: METHOD.into(),
                ..RegistryReport::default()
            },
        }
    }

    /// Records an object that could not be read or filtered.
    pub fn failed(&mut self, key: impl Into<String>, error: impl Into<String>) {
        self.report.objects += 1;
        self.report.failed.push((key.into(), error.into()));
    }

    /// Observes the filtered lines of one object of `dataset` on `day`.
    pub fn observe<S: AsRef<str>>(&mut self, dataset: Dataset, day: Option<Day>, lines: &[S]) {
        self.report.objects += 1;
        for line in lines {
            let Ok(v) = serde_json::from_str::<Value>(line.as_ref()) else {
                self.report.malformed_values += 1;
                continue;
            };
            self.walk(&v, dataset, day);
        }
    }

    /// Adds another builder's observations (for example one object's,
    /// built in parallel). Every statistic is a count, a maximum, a minimum
    /// or a set union, so the result does not depend on merge order.
    pub fn merge(&mut self, other: Self) {
        self.report.objects += other.report.objects;
        self.report.trades_observed += other.report.trades_observed;
        self.report.malformed_values += other.report.malformed_values;
        self.report.float_noise_rounded += other.report.float_noise_rounded;
        self.report.failed.extend(other.report.failed);
        for (coin, o) in other.coins {
            let c = self.coins.entry(coin).or_default();
            c.trades += o.trades;
            c.max_px_decimals = c.max_px_decimals.max(o.max_px_decimals);
            c.max_sz_decimals = c.max_sz_decimals.max(o.max_sz_decimals);
            c.first_day = match (c.first_day, o.first_day) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            c.last_day = match (c.last_day, o.last_day) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            c.datasets.extend(o.datasets);
        }
    }

    fn walk(&mut self, v: &Value, dataset: Dataset, day: Option<Day>) {
        match v {
            Value::Object(m) => {
                if let (
                    Some(Value::String(coin)),
                    Some(Value::String(px)),
                    Some(Value::String(sz)),
                ) = (m.get("coin"), m.get("px"), m.get("sz"))
                {
                    self.trade(coin, px, sz, dataset, day);
                    return;
                }
                for x in m.values() {
                    self.walk(x, dataset, day);
                }
            }
            Value::Array(a) => {
                for x in a {
                    self.walk(x, dataset, day);
                }
            }
            _ => {}
        }
    }

    fn trade(&mut self, coin: &str, px: &str, sz: &str, dataset: Dataset, day: Option<Day>) {
        let (px, sz) = (venue_decimal(px), venue_decimal(sz));
        for v in [&px, &sz] {
            if matches!(v, std::borrow::Cow::Owned(_)) {
                self.report.float_noise_rounded += 1;
            }
        }
        let (Some(pd), Some(sd)) = (decimals(&px), decimals(&sz)) else {
            self.report.malformed_values += 1;
            return;
        };
        self.report.trades_observed += 1;
        let c = self.coins.entry(coin.to_owned()).or_default();
        c.trades += 1;
        c.max_px_decimals = c.max_px_decimals.max(pd);
        c.max_sz_decimals = c.max_sz_decimals.max(sd);
        if let Some(d) = day {
            c.first_day = Some(c.first_day.map_or(d, |f| f.min(d)));
            c.last_day = Some(c.last_day.map_or(d, |l| l.max(d)));
        }
        c.datasets.insert(dataset.to_string());
    }

    /// The registry and the report. Coins with violations are left out.
    #[must_use]
    pub fn build(&self) -> (Registry, RegistryReport) {
        let mut report = self.report.clone();
        report.failed.sort();
        let mut registry = Registry::new();
        for (coin, c) in &self.coins {
            let spec = match classify_coin(coin) {
                CoinClass::Spot => {
                    report.spot_dropped.push(coin.clone());
                    continue;
                }
                CoinClass::Outcome => {
                    let ps = outcome_price_scale(c.max_sz_decimals);
                    if c.max_px_decimals > ps {
                        report.violations.push(Violation {
                            coin: coin.clone(),
                            reason: format!(
                                "a price has {} decimals; with {} size decimals outcome \
                                 prices may have at most {ps}",
                                c.max_px_decimals, c.max_sz_decimals
                            ),
                        });
                        continue;
                    }
                    InstrumentSpec::hyperliquid(
                        MarketClass::Outcome,
                        coin,
                        c.max_sz_decimals,
                        Some(ps),
                        self.admission.clone(),
                    )
                }
                CoinClass::CorePerp | CoinClass::Hip3Perp => {
                    let sz = c.max_sz_decimals;
                    if sz > PERP_MAX_DECIMALS || c.max_px_decimals > PERP_MAX_DECIMALS - sz {
                        report.violations.push(Violation {
                            coin: coin.clone(),
                            reason: format!(
                                "sizes need {sz} decimals, so prices may have at most {}, \
                                 but a price has {} (szDecimals changed within the period?)",
                                PERP_MAX_DECIMALS.saturating_sub(sz),
                                c.max_px_decimals
                            ),
                        });
                        continue;
                    }
                    InstrumentSpec::hyperliquid(
                        MarketClass::Perp,
                        coin,
                        sz,
                        None,
                        self.admission.clone(),
                    )
                }
            };
            let spec = match spec {
                Ok(s) => s.with_flags(default_flags(coin)),
                Err(e) => {
                    report.violations.push(Violation {
                        coin: coin.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            report.coins.push(CoinReport {
                coin: coin.clone(),
                instrument: spec.id.to_string(),
                class: spec.class.as_str().to_owned(),
                trades: c.trades,
                first_day: c.first_day.map(Day::iso),
                last_day: c.last_day.map(Day::iso),
                size_scale: spec.size_scale,
                price_scale: spec.price_scale,
                max_price_decimals: c.max_px_decimals,
                datasets: c.datasets.iter().cloned().collect(),
            });
            if let Err(e) = registry.insert(spec) {
                report.coins.pop();
                report.violations.push(Violation {
                    coin: coin.clone(),
                    reason: e.to_string(),
                });
            }
        }
        (registry, report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::tests::{block_line, fill};
    use crate::filter::{LocalFilter, RawLineFilter};
    use mesh_types::{Flag, VenueId};

    fn adm() -> AdmissionId {
        AdmissionId::new("ADM-0003").unwrap()
    }

    fn day(s: &str) -> Option<Day> {
        Some(Day::parse(s).unwrap())
    }

    /// Filters synthetic raw lines the way `registry` and `process` do.
    fn filtered(lines: &[String]) -> Vec<String> {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        LocalFilter
            .filter_lines(Dataset::NodeFillsByBlock, &refs)
            .unwrap()
            .lines
    }

    fn pair(coin: &str, px: &str, sz: &str, tid: u64) -> Vec<Value> {
        vec![
            fill(coin, px, sz, "B", true, tid, tid * 10, 1000),
            fill(coin, px, sz, "A", false, tid, tid * 10 + 1, 1000),
        ]
    }

    #[test]
    fn decimals_of_plain_strings() {
        assert_eq!(decimals("100"), Some(0));
        assert_eq!(decimals("100.0"), Some(0));
        assert_eq!(decimals("100.50"), Some(1));
        assert_eq!(decimals("0.00012"), Some(5));
        assert_eq!(decimals("-1.25"), Some(2));
        for bad in ["", ".5", "1.", "1e5", "abc", "1.2.3", "+1"] {
            assert_eq!(decimals(bad), None, "{bad}");
        }
    }

    #[test]
    fn derives_scales_classes_and_flags_from_fills() {
        let mut events = pair("BTC", "100000", "0.00012", 1);
        events.extend(pair("BTC", "100001.5", "0.1", 2));
        events.extend(pair("xyz:CL", "61.25", "0.5", 3));
        events.extend(pair("#90", "0.5123", "3", 4));
        // Five decimals, as observed on the venue (ADR 0021).
        events.extend(pair("#91", "0.48771", "2", 6));
        let raw = vec![block_line(10, "2025-07-27T00:00:01.000", events)];
        let mut b = RegistryBuilder::new(adm());
        b.observe(
            Dataset::NodeFillsByBlock,
            day("2025-07-27"),
            &filtered(&raw),
        );
        let raw2 = vec![block_line(
            11,
            "2025-07-28T00:00:01.000",
            pair("BTC", "99000", "1", 5),
        )];
        b.observe(
            Dataset::NodeFillsByBlock,
            day("2025-07-28"),
            &filtered(&raw2),
        );
        let (reg, rep) = b.build();
        assert!(rep.clean(), "{rep:?}");
        assert_eq!(rep.method, METHOD);
        assert_eq!(rep.objects, 2);
        // Each matched pair is two fill objects.
        assert_eq!(rep.trades_observed, 12);

        let hl = VenueId::new("hyperliquid").unwrap();
        let btc = reg.by_venue_symbol(&hl, "BTC").unwrap();
        assert_eq!((btc.size_scale, btc.price_scale), (5, 1));
        assert_eq!(btc.class, MarketClass::Perp);
        assert_eq!(btc.admission.as_str(), "ADM-0003");
        let cl = reg.by_venue_symbol(&hl, "xyz:CL").unwrap();
        assert_eq!(cl.namespace.as_deref(), Some("xyz"));
        assert!(cl.flags.contains(Flag::OracleAnchoredVenue));
        let outcome = reg.by_venue_symbol(&hl, "#90").unwrap();
        assert_eq!(outcome.class, MarketClass::Outcome);
        assert_eq!(outcome.price_scale, outcome_price_scale(0));
        assert_eq!(outcome.price_scale, 8);
        let five = reg.by_venue_symbol(&hl, "#91").unwrap();
        assert_eq!((five.size_scale, five.price_scale), (0, 8));

        let btc_rep = rep.coins.iter().find(|c| c.coin == "BTC").unwrap();
        assert_eq!(btc_rep.first_day.as_deref(), Some("2025-07-27"));
        assert_eq!(btc_rep.last_day.as_deref(), Some("2025-07-28"));
        assert_eq!(btc_rep.trades, 6);

        // The document is what `process --registry` reads.
        let json = serde_json::to_vec(&reg).unwrap();
        let back: Registry = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, reg);
    }

    #[test]
    fn float_noise_does_not_inflate_the_size_scale() {
        let mut events = pair("SUI", "1.99203", "2946657.1000000001", 1);
        events.extend(pair("SUI", "2.1", "10.5", 2));
        let raw = vec![block_line(10, "2025-10-10T21:17:40.819", events)];
        let mut b = RegistryBuilder::new(adm());
        b.observe(
            Dataset::NodeFillsByBlock,
            day("2025-10-10"),
            &filtered(&raw),
        );
        let (reg, rep) = b.build();
        assert!(rep.clean(), "{rep:?}");
        assert_eq!(rep.method, METHOD);
        // Two fill objects carry the noisy size.
        assert_eq!(rep.float_noise_rounded, 2);
        let hl = VenueId::new("hyperliquid").unwrap();
        let sui = reg.by_venue_symbol(&hl, "SUI").unwrap();
        assert_eq!((sui.size_scale, sui.price_scale), (1, 5));
    }

    #[test]
    fn merging_per_object_builders_equals_one_serial_builder() {
        let objects = [
            (
                "2025-07-28",
                block_line(11, "2025-07-28T00:00:01.000", pair("BTC", "99000", "1", 5)),
            ),
            (
                "2025-07-27",
                block_line(
                    10,
                    "2025-07-27T00:00:01.000",
                    [
                        pair("BTC", "100000", "0.00012", 1),
                        pair("#90", "0.5123", "3", 2),
                    ]
                    .concat(),
                ),
            ),
            (
                "2025-07-29",
                block_line(
                    12,
                    "2025-07-29T00:00:01.000",
                    pair("ETH", "3000.1", "0.01", 3),
                ),
            ),
        ];
        let mut serial = RegistryBuilder::new(adm());
        for (d, line) in &objects {
            serial.observe(
                Dataset::NodeFillsByBlock,
                day(d),
                &filtered(std::slice::from_ref(line)),
            );
        }
        serial.failed("z", "late");
        serial.failed("a", "early");

        // Reverse order, one builder per object, as the parallel run does.
        let mut merged = RegistryBuilder::new(adm());
        let mut late = RegistryBuilder::new(adm());
        late.failed("a", "early");
        merged.merge(late);
        for (d, line) in objects.iter().rev() {
            let mut one = RegistryBuilder::new(adm());
            one.observe(
                Dataset::NodeFillsByBlock,
                day(d),
                &filtered(std::slice::from_ref(line)),
            );
            merged.merge(one);
        }
        let mut z = RegistryBuilder::new(adm());
        z.failed("z", "late");
        merged.merge(z);

        assert_eq!(serial.build(), merged.build());
    }

    #[test]
    fn a_price_that_does_not_fit_is_a_violation_not_a_spec() {
        // Sizes need 4 decimals, so prices may have 2; one has 3.
        let mut events = pair("ETH", "3000.12", "0.0001", 1);
        events.extend(pair("ETH", "3000.125", "1", 2));
        events.extend(pair("SOL", "150.25", "2", 3));
        let raw = vec![block_line(10, "2025-07-27T00:00:01.000", events)];
        let mut b = RegistryBuilder::new(adm());
        b.observe(
            Dataset::NodeFillsByBlock,
            day("2025-07-27"),
            &filtered(&raw),
        );
        let (reg, rep) = b.build();
        assert!(!rep.clean());
        assert_eq!(rep.violations.len(), 1);
        assert_eq!(rep.violations[0].coin, "ETH");
        let hl = VenueId::new("hyperliquid").unwrap();
        assert!(reg.by_venue_symbol(&hl, "ETH").is_none());
        assert!(reg.by_venue_symbol(&hl, "SOL").is_some());
    }

    #[test]
    fn excluded_fills_and_spot_never_shape_the_registry() {
        // A liquidation priced by mark (no taker) is dropped by the filter,
        // so its extra price decimals cannot cause a violation; an unpaired
        // spot fill is dropped too.
        let mut events = pair("BTC", "100000", "0.1", 1);
        events.push(fill("BTC", "99999.1234", "1", "B", false, 2, 21, 1000));
        events.push(fill("BTC", "99999.1234", "1", "A", false, 2, 22, 1000));
        events.extend(pair("@107", "1.234567", "1", 3));
        let raw = vec![block_line(10, "2025-07-27T00:00:01.000", events)];
        let mut b = RegistryBuilder::new(adm());
        b.observe(
            Dataset::NodeFillsByBlock,
            day("2025-07-27"),
            &filtered(&raw),
        );
        let (reg, rep) = b.build();
        assert!(rep.violations.is_empty(), "{rep:?}");
        assert_eq!(reg.len(), 1);
        assert!(rep.coins.iter().all(|c| c.coin == "BTC"));
    }

    #[test]
    fn failed_objects_make_the_run_incomplete() {
        let mut b = RegistryBuilder::new(adm());
        b.failed("node_fills/hourly/20250601/0.lz4", "bad lz4");
        let (_, rep) = b.build();
        assert!(!rep.clean());
        assert_eq!(rep.objects, 1);
    }
}
