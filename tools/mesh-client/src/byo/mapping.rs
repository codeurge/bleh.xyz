//! The user-written mapping file: which symbol of the user's feed is
//! compared with which bleh.xyz series.
//!
//! ```toml
//! [[pair]]
//! name = "btc-perp-vs-mine"       # unique; names the pair in every output
//! user_symbol = "MY-BTC"          # symbol as the user's feed writes it
//! user_price = "any"              # any (trade, else mid) | trade | mid
//! multiplier = 1.0                # user price x multiplier is compared
//!
//! [pair.bleh]
//! market = "hyperliquid:perp:BTC" # a bleh.xyz instrument
//! price = "mid"                   # trade | mid | bar_close
//! # interval = "1m"               # bar_close only
//!
//! [[pair]]
//! name = "index-december"
//! user_symbol = "IDX-DEC"
//! [pair.bleh]
//! synthetic_month = "<id>"        # reserved: a synthetic contract month
//! # exposure = "<id>"             # reserved: an exposure's front series
//! ```
//!
//! `market` pairs are served by gateway v0 today. `synthetic_month` and
//! `exposure` references parse and validate, so mapping files can be written
//! now, but stay inactive until the synthetic contract-month engine
//! publishes those series (open reference prices plan, task 2).

use std::collections::{BTreeMap, HashSet};

use mesh_types::{BarInterval, InstrumentId, RecordKind};
use serde::Deserialize;

use crate::protocol::{ChannelSpec, Depth, InstrumentPattern};

/// Which user price a pair compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserPrice {
    /// The trade price when the record has one, else the mid.
    #[default]
    Any,
    /// Trade prices only.
    Trade,
    /// Mid of bid and ask only.
    Mid,
}

/// Which bleh.xyz price a market pair compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlehPrice {
    /// Trade prices (`trade` channel).
    Trade,
    /// Mid of the L2 book (`book_event` channel at `l2` depth).
    Mid,
    /// Bar close (`bar` channel at one interval).
    BarClose(BarInterval),
}

/// What a pair compares the user's symbol with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlehTarget {
    /// A bleh.xyz market served by the gateway.
    Market {
        /// Instrument.
        instrument: InstrumentId,
        /// Price.
        price: BlehPrice,
    },
    /// A synthetic contract month (reserved until the engine publishes).
    SyntheticMonth(String),
    /// An exposure's front series (reserved until the engine publishes).
    Exposure(String),
}

impl BlehTarget {
    /// The gateway channel serving this target, if it is served today.
    pub fn channel(&self) -> Option<ChannelSpec> {
        let Self::Market { instrument, price } = self else {
            return None;
        };
        let inst = InstrumentPattern::Exact(instrument.clone());
        Some(match price {
            BlehPrice::Trade => ChannelSpec::new(inst, RecordKind::Trade),
            BlehPrice::Mid => {
                let mut c = ChannelSpec::new(inst, RecordKind::BookEvent);
                c.depth = Some(Depth::L2);
                c
            }
            BlehPrice::BarClose(i) => {
                let mut c = ChannelSpec::new(inst, RecordKind::Bar);
                c.interval = Some(*i);
                c
            }
        })
    }

    /// Short label for outputs.
    pub fn label(&self) -> String {
        match self {
            Self::Market { instrument, .. } => instrument.to_string(),
            Self::SyntheticMonth(id) => format!("synthetic_month:{id}"),
            Self::Exposure(id) => format!("exposure:{id}"),
        }
    }
}

/// `INSTRUMENT|KIND[|l2|INTERVAL]`, the form `--sub` takes.
pub fn channel_string(c: &ChannelSpec) -> String {
    let mut s = format!("{}|{}", c.instrument, c.kind);
    match (c.depth, c.interval) {
        (Some(Depth::L2), _) => s.push_str("|l2"),
        (Some(Depth::L4), _) => s.push_str("|l4"),
        (None, Some(i)) => {
            s.push('|');
            s.push_str(i.as_str());
        }
        (None, None) => {}
    }
    s
}

/// One validated pair.
#[derive(Debug, Clone, PartialEq)]
pub struct Pair {
    /// Unique name.
    pub name: String,
    /// Symbol in the user's feed.
    pub user_symbol: String,
    /// Which user price.
    pub user_price: UserPrice,
    /// Applied to the user price before comparison.
    pub multiplier: f64,
    /// The bleh.xyz side.
    pub bleh: BlehTarget,
}

/// A validated mapping file.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    /// Pairs, in file order.
    pub pairs: Vec<Pair>,
}

/// A mapping file problem.
#[derive(Debug, thiserror::Error)]
#[error("mapping file: {0}")]
pub struct MappingError(String);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    pair: Vec<RawPair>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPair {
    name: String,
    user_symbol: String,
    #[serde(default)]
    user_price: UserPrice,
    multiplier: Option<f64>,
    bleh: RawBleh,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBleh {
    market: Option<String>,
    price: Option<String>,
    interval: Option<String>,
    synthetic_month: Option<String>,
    exposure: Option<String>,
}

impl Mapping {
    /// Parses and validates a TOML mapping file.
    ///
    /// # Errors
    ///
    /// Syntax errors, unknown keys, duplicate names, a bad instrument,
    /// price or interval, or a pair with zero or several targets.
    pub fn parse(text: &str) -> Result<Self, MappingError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| MappingError(e.to_string()))?;
        if raw.pair.is_empty() {
            return Err(MappingError("no [[pair]] entries".into()));
        }
        let mut names = HashSet::new();
        let mut pairs = Vec::with_capacity(raw.pair.len());
        for p in raw.pair {
            let err = |m: String| MappingError(format!("pair {:?}: {m}", p.name));
            if p.name.trim().is_empty() || p.user_symbol.trim().is_empty() {
                return Err(MappingError("pairs need a name and a user_symbol".into()));
            }
            if !names.insert(p.name.clone()) {
                return Err(err("duplicate name".into()));
            }
            let multiplier = p.multiplier.unwrap_or(1.0);
            if !(multiplier.is_finite() && multiplier > 0.0) {
                return Err(err("multiplier must be a positive number".into()));
            }
            let b = &p.bleh;
            let targets = [
                b.market.is_some(),
                b.synthetic_month.is_some(),
                b.exposure.is_some(),
            ]
            .into_iter()
            .filter(|x| *x)
            .count();
            if targets != 1 {
                return Err(err(
                    "[pair.bleh] needs exactly one of market, synthetic_month, exposure".into(),
                ));
            }
            let bleh = if let Some(m) = &b.market {
                let instrument = InstrumentId::new(m.as_str()).map_err(|e| err(e.to_string()))?;
                let interval = b
                    .interval
                    .as_deref()
                    .map(|i| {
                        serde_json::from_value::<BarInterval>(serde_json::Value::String(i.into()))
                            .map_err(|_| err(format!("unknown interval {i:?}")))
                    })
                    .transpose()?;
                let price = match (b.price.as_deref().unwrap_or("trade"), interval) {
                    ("trade", None) => BlehPrice::Trade,
                    ("mid", None) => BlehPrice::Mid,
                    ("bar_close", Some(i)) => BlehPrice::BarClose(i),
                    ("bar_close", None) => {
                        return Err(err("price bar_close needs an interval".into()));
                    }
                    ("trade" | "mid", Some(_)) => {
                        return Err(err("interval applies to bar_close only".into()));
                    }
                    (o, _) => {
                        return Err(err(format!(
                            "unknown price {o:?} (trade, mid or bar_close)"
                        )));
                    }
                };
                BlehTarget::Market { instrument, price }
            } else {
                if b.price.is_some() || b.interval.is_some() {
                    return Err(err("price and interval apply to market pairs only".into()));
                }
                match (&b.synthetic_month, &b.exposure) {
                    (Some(id), None) if !id.trim().is_empty() => {
                        BlehTarget::SyntheticMonth(id.trim().into())
                    }
                    (None, Some(id)) if !id.trim().is_empty() => {
                        BlehTarget::Exposure(id.trim().into())
                    }
                    _ => return Err(err("empty synthetic_month or exposure".into())),
                }
            };
            pairs.push(Pair {
                name: p.name,
                user_symbol: p.user_symbol.trim().into(),
                user_price: p.user_price,
                multiplier,
                bleh,
            });
        }
        Ok(Self { pairs })
    }

    /// Reads and parses a mapping file.
    ///
    /// # Errors
    ///
    /// I/O or [`Mapping::parse`] errors.
    pub fn from_file(path: &std::path::Path) -> Result<Self, MappingError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| MappingError(format!("{}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Pairs served today.
    pub fn active(&self) -> impl Iterator<Item = &Pair> {
        self.pairs.iter().filter(|p| p.bleh.channel().is_some())
    }

    /// Pairs waiting for series the gateway does not serve yet.
    pub fn inactive(&self) -> impl Iterator<Item = &Pair> {
        self.pairs.iter().filter(|p| p.bleh.channel().is_none())
    }

    /// The distinct gateway channels the active pairs need.
    pub fn channels(&self) -> Vec<ChannelSpec> {
        let mut by_key = BTreeMap::new();
        for c in self.active().filter_map(|p| p.bleh.channel()) {
            by_key.entry(channel_string(&c)).or_insert(c);
        }
        by_key.into_values().collect()
    }

    /// The distinct user symbols of active pairs (for live subscriptions).
    pub fn user_symbols(&self) -> Vec<String> {
        let mut s: Vec<String> = self.active().map(|p| p.user_symbol.clone()).collect();
        s.sort();
        s.dedup();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
[[pair]]
name = "a"
user_symbol = "MY-BTC"
user_price = "mid"
multiplier = 0.5
[pair.bleh]
market = "hyperliquid:perp:BTC"
price = "mid"

[[pair]]
name = "b"
user_symbol = "MY-BTC"
[pair.bleh]
market = "hyperliquid:perp:BTC"

[[pair]]
name = "c"
user_symbol = "MY-ETH"
[pair.bleh]
market = "hyperliquid:perp:ETH"
price = "bar_close"
interval = "1m"

[[pair]]
name = "d"
user_symbol = "IDX-DEC"
[pair.bleh]
synthetic_month = "equity-index-us-large:2026-12"
"#;

    #[test]
    fn parses_pairs_targets_and_channels() {
        let m = Mapping::parse(GOOD).unwrap();
        assert_eq!(m.pairs.len(), 4);
        assert_eq!(m.pairs[0].user_price, UserPrice::Mid);
        assert!((m.pairs[0].multiplier - 0.5).abs() < f64::EPSILON);
        assert_eq!(m.pairs[1].user_price, UserPrice::Any);
        assert!(matches!(
            m.pairs[1].bleh,
            BlehTarget::Market {
                price: BlehPrice::Trade,
                ..
            }
        ));
        assert_eq!(
            m.pairs[3].bleh,
            BlehTarget::SyntheticMonth("equity-index-us-large:2026-12".into())
        );
        let ch: Vec<String> = m.channels().iter().map(channel_string).collect();
        assert_eq!(
            ch,
            [
                "hyperliquid:perp:BTC|book_event|l2",
                "hyperliquid:perp:BTC|trade",
                "hyperliquid:perp:ETH|bar|1m"
            ]
        );
        assert_eq!(m.active().count(), 3);
        assert_eq!(
            m.inactive().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["d"]
        );
        assert_eq!(m.user_symbols(), ["MY-BTC", "MY-ETH"]);
    }

    #[test]
    fn rejects_bad_files() {
        let pair = |bleh: &str| {
            format!("[[pair]]\nname = \"x\"\nuser_symbol = \"S\"\n[pair.bleh]\n{bleh}\n")
        };
        for (text, want) in [
            (String::new(), "no [[pair]]"),
            (pair(""), "exactly one"),
            (
                pair("market = \"hyperliquid:perp:BTC\"\nexposure = \"e\""),
                "exactly one",
            ),
            (pair("market = \"not an id\""), "pair \"x\""),
            (
                pair("market = \"hyperliquid:perp:BTC\"\nprice = \"vwap\""),
                "unknown price",
            ),
            (
                pair("market = \"hyperliquid:perp:BTC\"\nprice = \"bar_close\""),
                "needs an interval",
            ),
            (
                pair("market = \"hyperliquid:perp:BTC\"\nprice = \"bar_close\"\ninterval = \"7m\""),
                "unknown interval",
            ),
            (
                pair("market = \"hyperliquid:perp:BTC\"\ninterval = \"1m\""),
                "bar_close only",
            ),
            (
                pair("exposure = \"e\"\nprice = \"mid\""),
                "market pairs only",
            ),
            (pair("exposure = \" \""), "empty"),
            (
                pair("market = \"hyperliquid:perp:BTC\"\noracle = 1"),
                "unknown field",
            ),
            (
                format!("{}{}", pair("exposure = \"e\""), pair("exposure = \"f\"")),
                "duplicate",
            ),
            (
                pair("exposure = \"e\"").replace("user_symbol", "multiplier = -1\nuser_symbol"),
                "positive",
            ),
        ] {
            let e = Mapping::parse(&text).unwrap_err().to_string();
            assert!(e.contains(want), "{text:?}: {e}");
        }
    }
}
