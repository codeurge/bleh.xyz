//! Instrument specifications.
//!
//! The registry maps a venue's market symbol (as written in the venue's own
//! data) to a mesh [`InstrumentId`] and the instrument's fixed-point scales.
//! There is no hard-coded market list: a reader builds [`InstrumentSpec`]s
//! from the venue's metadata at start-up and whenever new markets appear.
//!
//! # Hyperliquid
//!
//! [`InstrumentSpec::hyperliquid`] derives the spec from metadata fields:
//!
//! - Symbol: the coin exactly as the node writes it. Core perpetuals are
//!   plain (`BTC`); HIP-3 deployer markets carry the deployer's namespace
//!   (`xyz:CL`); spot markets use `@<index>` or a pair name, where `/` is
//!   written as `-` in the mesh ID; outcome (HIP-4) markets use whatever
//!   symbol the node's metadata gives (format to verify).
//! - Size scale: the market's `szDecimals`.
//! - Price scale: `6 - szDecimals` for perpetuals and `8 - szDecimals` for
//!   spot (Hyperliquid's documented tick rule, which allows at most that
//!   many decimals). Outcome markets take an explicit price scale until
//!   their rule is confirmed.
//!
//! # Registry document (`registry-v0`)
//!
//! The serialized registry is the document of `docs/specs/registry-v0.md`:
//! `instruments` plus optional [`Sections`] (exposures, calendars, roll
//! schedules, reference series, liquidity grades). A registry with only
//! instruments serializes as `{"instruments": [...]}`, as before.

use std::collections::BTreeMap;

mod sections;

pub use sections::{
    Calendar, EarlyClose, EntryId, ExpiryRule, Exposure, ExposureClass, ExposureForm, HolidayRules,
    LiquidityEntry, MappingRelation, MethodPeriod, REGISTRY_VERSION, RollReference, RollRule,
    RollSchedule, Sections, SeriesEntry, Session, VenueMapping, Weekday,
};

use serde::{Deserialize, Serialize};

use crate::flags::Flags;
use crate::ids::{AdmissionId, IdError, InstrumentId, VenueId};

/// Kind of market.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketClass {
    /// Perpetual future (core or HIP-3 deployer market).
    Perp,
    /// Spot pair.
    Spot,
    /// Outcome (event) market, such as Hyperliquid HIP-4.
    Outcome,
    /// Over-the-counter swap product (public dissemination sources).
    Swap,
}

impl MarketClass {
    /// The class part of an [`InstrumentId`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Perp => "perp",
            Self::Spot => "spot",
            Self::Outcome => "outcome",
            Self::Swap => "swap",
        }
    }
}

/// Errors from building or using the registry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// The derived instrument ID is invalid.
    #[error(transparent)]
    Id(#[from] IdError),
    /// `szDecimals` is too large for the class's price rule.
    #[error("szDecimals {sz_decimals} is too large for a {class:?} market")]
    Decimals {
        /// Market class.
        class: MarketClass,
        /// The value given.
        sz_decimals: u8,
    },
    /// An outcome market needs an explicit price scale.
    #[error("outcome markets need an explicit price scale")]
    NeedsPriceScale,
    /// The instrument or venue symbol is already registered.
    #[error("duplicate registration for {0}")]
    Duplicate(String),
    /// The document names a registry version this code does not read.
    #[error("unsupported registry version {0:?} (expected {REGISTRY_VERSION:?})")]
    Version(String),
    /// A registry section entry is malformed or refers to something
    /// missing.
    #[error("invalid registry entry: {0}")]
    Invalid(String),
}

/// Everything a reader needs to normalize one market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentSpec {
    /// Mesh instrument ID.
    pub id: InstrumentId,
    /// Venue.
    pub venue: VenueId,
    /// Market class.
    pub class: MarketClass,
    /// The market's symbol in the venue's own data (Hyperliquid: `coin`).
    pub venue_symbol: String,
    /// Deployer namespace for builder-deployed markets (HIP-3), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Price scale (fractional digits).
    pub price_scale: u8,
    /// Size scale (fractional digits).
    pub size_scale: u8,
    /// Flags applied to every record of this instrument (for example
    /// `oracle_anchored_venue`).
    #[serde(default)]
    pub flags: Flags,
    /// Admission decision covering this market's fields.
    pub admission: AdmissionId,
}

impl InstrumentSpec {
    /// Builds a Hyperliquid spec from node metadata. `price_scale` is
    /// required for outcome markets and ignored otherwise.
    ///
    /// # Errors
    ///
    /// Fails if the symbol cannot form a valid ID, `szDecimals` exceeds the
    /// price rule, or an outcome market has no price scale.
    pub fn hyperliquid(
        class: MarketClass,
        coin: &str,
        sz_decimals: u8,
        price_scale: Option<u8>,
        admission: AdmissionId,
    ) -> Result<Self, RegistryError> {
        let venue = VenueId::new("hyperliquid")?;
        let max_decimals = match class {
            MarketClass::Perp => Some(6u8),
            MarketClass::Spot => Some(8u8),
            MarketClass::Outcome | MarketClass::Swap => None,
        };
        let price_scale = match max_decimals {
            Some(max) => max
                .checked_sub(sz_decimals)
                .ok_or(RegistryError::Decimals { class, sz_decimals })?,
            None => price_scale.ok_or(RegistryError::NeedsPriceScale)?,
        };
        let namespace = match class {
            MarketClass::Perp => coin.split_once(':').map(|(ns, _)| ns.to_owned()),
            _ => None,
        };
        let id = InstrumentId::from_parts(&venue, class.as_str(), &coin.replace('/', "-"))?;
        Ok(Self {
            id,
            venue,
            class,
            venue_symbol: coin.to_owned(),
            namespace,
            price_scale,
            size_scale: sz_decimals,
            flags: Flags::NONE,
            admission,
        })
    }

    /// Returns the spec with `flags` applied to every record.
    #[must_use]
    pub fn with_flags(mut self, flags: Flags) -> Self {
        self.flags = flags;
        self
    }
}

/// A set of instruments, indexed by ID and by venue symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RegistryFile", into = "RegistryFile")]
pub struct Registry {
    by_id: BTreeMap<InstrumentId, InstrumentSpec>,
    by_symbol: BTreeMap<(VenueId, String), InstrumentId>,
    sections: Sections,
}

/// Serialized form: `{"registry": "registry-v0", "instruments": [...],
/// ...sections}`, instruments sorted by ID. `registry` is optional on input
/// and written only when a section is present.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    registry: Option<String>,
    instruments: Vec<InstrumentSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    exposures: Vec<Exposure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    calendars: Vec<Calendar>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rolls: Vec<RollSchedule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    series: Vec<SeriesEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    liquidity: Vec<LiquidityEntry>,
}

impl TryFrom<RegistryFile> for Registry {
    type Error = RegistryError;
    fn try_from(file: RegistryFile) -> Result<Self, Self::Error> {
        if let Some(v) = file.registry
            && v != REGISTRY_VERSION
        {
            return Err(RegistryError::Version(v));
        }
        let mut r = Registry::default();
        for spec in file.instruments {
            r.insert(spec)?;
        }
        r.set_sections(Sections {
            exposures: file.exposures,
            calendars: file.calendars,
            rolls: file.rolls,
            series: file.series,
            liquidity: file.liquidity,
        })?;
        Ok(r)
    }
}

impl From<Registry> for RegistryFile {
    fn from(r: Registry) -> Self {
        RegistryFile {
            registry: (!r.sections.is_empty()).then(|| REGISTRY_VERSION.to_owned()),
            instruments: r.by_id.into_values().collect(),
            exposures: r.sections.exposures,
            calendars: r.sections.calendars,
            rolls: r.sections.rolls,
            series: r.sections.series,
            liquidity: r.sections.liquidity,
        }
    }
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a spec.
    ///
    /// # Errors
    ///
    /// Fails if the ID or the (venue, symbol) pair is already registered.
    pub fn insert(&mut self, spec: InstrumentSpec) -> Result<(), RegistryError> {
        let key = (spec.venue.clone(), spec.venue_symbol.clone());
        if self.by_id.contains_key(&spec.id) || self.by_symbol.contains_key(&key) {
            return Err(RegistryError::Duplicate(spec.id.to_string()));
        }
        self.by_symbol.insert(key, spec.id.clone());
        self.by_id.insert(spec.id.clone(), spec);
        Ok(())
    }

    /// Replaces the spec registered under `spec.id`, or adds it if absent,
    /// and returns the previous spec. The venue symbol may change; the
    /// symbol index follows.
    ///
    /// Changing a registered market's scales changes how its decimals are
    /// read: callers decide when that is allowed (an operator decision for
    /// Hyperliquid, see `docs/operations/node-reader.md`).
    ///
    /// # Errors
    ///
    /// Fails if the new (venue, symbol) pair belongs to another instrument.
    pub fn replace(
        &mut self,
        spec: InstrumentSpec,
    ) -> Result<Option<InstrumentSpec>, RegistryError> {
        let key = (spec.venue.clone(), spec.venue_symbol.clone());
        if let Some(owner) = self.by_symbol.get(&key)
            && *owner != spec.id
        {
            return Err(RegistryError::Duplicate(spec.id.to_string()));
        }
        let old = self.by_id.remove(&spec.id);
        if let Some(o) = &old {
            self.by_symbol
                .remove(&(o.venue.clone(), o.venue_symbol.clone()));
        }
        self.by_symbol.insert(key, spec.id.clone());
        self.by_id.insert(spec.id.clone(), spec);
        Ok(old)
    }

    /// Removes an instrument, returning its spec if it was registered. Its
    /// liquidity entry, if any, goes with it.
    pub fn remove(&mut self, id: &InstrumentId) -> Option<InstrumentSpec> {
        let old = self.by_id.remove(id)?;
        self.by_symbol
            .remove(&(old.venue.clone(), old.venue_symbol.clone()));
        self.sections.liquidity.retain(|l| l.subject != id.as_str());
        Some(old)
    }

    /// The sections besides instruments.
    #[must_use]
    pub fn sections(&self) -> &Sections {
        &self.sections
    }

    /// Replaces the sections besides instruments.
    ///
    /// # Errors
    ///
    /// Fails if an entry is malformed, an ID repeats, or a reference
    /// (calendar, roll, liquidity subject) does not resolve.
    pub fn set_sections(&mut self, sections: Sections) -> Result<(), RegistryError> {
        sections
            .validate(|id| InstrumentId::new(id).is_ok_and(|id| self.by_id.contains_key(&id)))?;
        self.sections = sections;
        Ok(())
    }

    /// Looks up by mesh ID.
    #[must_use]
    pub fn get(&self, id: &InstrumentId) -> Option<&InstrumentSpec> {
        self.by_id.get(id)
    }

    /// Looks up by the venue's own symbol.
    #[must_use]
    pub fn by_venue_symbol(&self, venue: &VenueId, symbol: &str) -> Option<&InstrumentSpec> {
        self.by_symbol
            .get(&(venue.clone(), symbol.to_owned()))
            .and_then(|id| self.by_id.get(id))
    }

    /// All specs, sorted by ID.
    pub fn iter(&self) -> impl Iterator<Item = &InstrumentSpec> {
        self.by_id.values()
    }

    /// Number of instruments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// True if empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Flag;

    fn adm() -> AdmissionId {
        AdmissionId::new("ADM-0001").unwrap()
    }

    #[test]
    fn hyperliquid_specs_from_metadata() {
        let core = InstrumentSpec::hyperliquid(MarketClass::Perp, "BTC", 5, None, adm()).unwrap();
        assert_eq!(core.id.as_str(), "hyperliquid:perp:BTC");
        assert_eq!((core.price_scale, core.size_scale), (1, 5));
        assert_eq!(core.namespace, None);

        let hip3 = InstrumentSpec::hyperliquid(MarketClass::Perp, "xyz:CL", 2, None, adm())
            .unwrap()
            .with_flags(Flags::NONE.with(Flag::OracleAnchoredVenue));
        assert_eq!(hip3.id.as_str(), "hyperliquid:perp:xyz:CL");
        assert_eq!(hip3.namespace.as_deref(), Some("xyz"));
        assert_eq!(hip3.price_scale, 4);

        let spot =
            InstrumentSpec::hyperliquid(MarketClass::Spot, "PURR/USDC", 0, None, adm()).unwrap();
        assert_eq!(spot.id.as_str(), "hyperliquid:spot:PURR-USDC");
        assert_eq!(spot.price_scale, 8);
        assert_eq!(spot.venue_symbol, "PURR/USDC");

        let outcome =
            InstrumentSpec::hyperliquid(MarketClass::Outcome, "#9001", 0, Some(4), adm()).unwrap();
        assert_eq!(outcome.id.as_str(), "hyperliquid:outcome:#9001");
        assert_eq!(
            InstrumentSpec::hyperliquid(MarketClass::Outcome, "#9001", 0, None, adm()),
            Err(RegistryError::NeedsPriceScale)
        );
        assert!(matches!(
            InstrumentSpec::hyperliquid(MarketClass::Perp, "BTC", 7, None, adm()),
            Err(RegistryError::Decimals { .. })
        ));
        assert!(
            InstrumentSpec::hyperliquid(MarketClass::Perp, "bad coin", 1, None, adm()).is_err()
        );
    }

    #[test]
    fn registry_lookup_and_serde() {
        let mut r = Registry::new();
        for (class, coin, d, ps) in [
            (MarketClass::Perp, "BTC", 5, None),
            (MarketClass::Perp, "xyz:CL", 2, None),
            (MarketClass::Outcome, "#9001", 0, Some(4)),
        ] {
            r.insert(InstrumentSpec::hyperliquid(class, coin, d, ps, adm()).unwrap())
                .unwrap();
        }
        let venue = VenueId::new("hyperliquid").unwrap();
        assert_eq!(r.by_venue_symbol(&venue, "xyz:CL").unwrap().size_scale, 2);
        assert!(r.by_venue_symbol(&venue, "CL").is_none());
        let dup = InstrumentSpec::hyperliquid(MarketClass::Perp, "BTC", 5, None, adm()).unwrap();
        assert!(matches!(r.insert(dup), Err(RegistryError::Duplicate(_))));

        let json = serde_json::to_string(&r).unwrap();
        let back: Registry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.len(), 3);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["instruments"][0]["id"], "hyperliquid:outcome:#9001");
    }

    #[test]
    fn registry_replace_and_remove() {
        let mut r = Registry::new();
        let btc = InstrumentSpec::hyperliquid(MarketClass::Perp, "BTC", 5, None, adm()).unwrap();
        let eth = InstrumentSpec::hyperliquid(MarketClass::Perp, "ETH", 4, None, adm()).unwrap();
        r.insert(btc.clone()).unwrap();
        r.insert(eth.clone()).unwrap();
        // Replace with new scales: the old spec comes back.
        let btc4 = InstrumentSpec::hyperliquid(MarketClass::Perp, "BTC", 4, None, adm()).unwrap();
        assert_eq!(r.replace(btc4.clone()).unwrap(), Some(btc.clone()));
        assert_eq!(r.get(&btc.id), Some(&btc4));
        let venue = VenueId::new("hyperliquid").unwrap();
        assert_eq!(r.by_venue_symbol(&venue, "BTC"), Some(&btc4));
        // A new symbol that belongs to another instrument is refused.
        let mut clash = btc4.clone();
        clash.venue_symbol = "ETH".into();
        assert!(matches!(r.replace(clash), Err(RegistryError::Duplicate(_))));
        // Replace of an absent spec inserts it.
        let sol = InstrumentSpec::hyperliquid(MarketClass::Perp, "SOL", 2, None, adm()).unwrap();
        assert_eq!(r.replace(sol.clone()).unwrap(), None);
        assert_eq!(r.len(), 3);
        // Remove clears both indexes.
        assert_eq!(r.remove(&eth.id), Some(eth.clone()));
        assert!(r.by_venue_symbol(&venue, "ETH").is_none());
        assert_eq!(r.remove(&eth.id), None);
        r.insert(eth).unwrap();
    }
}
