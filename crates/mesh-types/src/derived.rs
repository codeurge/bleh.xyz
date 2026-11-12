//! Derived records: the shared header and analytics kinds.
//!
//! A derived record is computed by a versioned method from other records.
//! Every derived record carries a [`DerivedHeader`]: the method
//! (`name@version`), the content hash of its inputs in canonical order
//! (see [`crate::hash`]) and the number of inputs, so anyone can recompute
//! it from the archive and compare.
//!
//! [`Bar`](crate::Bar) is the first derived kind. [`Footprint`] and
//! [`VolumeProfile`] are **stubs**: their shapes are fixed here so the
//! encodings, allowlist and schemas are stable, and the book lane
//! implements the methods.

use serde::{Deserialize, Serialize};

use crate::decimal::{Price, Size};
use crate::hash::ContentHash;
use crate::ids::MethodId;
use crate::provenance::Provenance;
use crate::record::{BarInterval, RecordKind, ValidationError, ensure};
use crate::time::UnixMillis;

/// Header common to every derived record.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedHeader {
    /// Method and version, such as `bars@1`.
    pub method: MethodId,
    /// [`ContentHash`] of the input records, in canonical order.
    pub inputs_hash: ContentHash,
    /// Number of input records.
    pub input_count: u64,
}

/// Volume at one price within a footprint bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootprintLevel {
    /// Trade price.
    pub price: Price,
    /// Size traded at the bid (sell aggressor).
    pub bid_volume: Size,
    /// Size traded at the ask (buy aggressor).
    pub ask_volume: Size,
    /// Number of trades at this price.
    pub trade_count: u32,
}

/// Per-price traded volume, split by aggressor, for one bar interval.
/// **Stub**: shape fixed, method not yet implemented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    /// Provenance; `seq` and `ts_source_ms` of the last input trade.
    pub prov: Provenance,
    /// Method and inputs hash.
    pub derived: DerivedHeader,
    /// Interval length.
    pub interval: BarInterval,
    /// Interval start (inclusive), aligned to the interval.
    pub start_ms: UnixMillis,
    /// Levels in ascending price order, one per traded price.
    pub levels: Vec<FootprintLevel>,
}

impl Footprint {
    /// Checks level ordering and alignment.
    ///
    /// # Errors
    ///
    /// Fails if levels are empty or not strictly ascending, or the start is
    /// misaligned.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::Footprint;
        ensure(!self.levels.is_empty(), k, || {
            "footprints are never empty".into()
        })?;
        ensure(
            self.interval.align(self.start_ms) == self.start_ms,
            k,
            || "start is not aligned to the interval".into(),
        )?;
        ensure(
            self.levels.windows(2).all(|w| w[0].price < w[1].price),
            k,
            || "levels must be in strictly ascending price order".into(),
        )
    }
}

/// Volume at one price bucket of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileLevel {
    /// Lower bound of the bucket.
    pub price: Price,
    /// Total size traded in the bucket.
    pub volume: Size,
    /// Size traded with a buy aggressor.
    pub buy_volume: Size,
    /// Size traded with a sell aggressor.
    pub sell_volume: Size,
}

/// Traded volume by price bucket over a time range.
/// **Stub**: shape fixed, method not yet implemented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeProfile {
    /// Provenance; `seq` and `ts_source_ms` of the last input trade.
    pub prov: Provenance,
    /// Method and inputs hash.
    pub derived: DerivedHeader,
    /// Range start (inclusive).
    pub start_ms: UnixMillis,
    /// Range end (exclusive).
    pub end_ms: UnixMillis,
    /// Bucket width, in price units (positive).
    pub bucket: Price,
    /// Buckets in ascending price order; empty buckets are omitted.
    pub levels: Vec<ProfileLevel>,
}

impl VolumeProfile {
    /// Checks range and bucket consistency.
    ///
    /// # Errors
    ///
    /// Fails on an empty or inverted range, a non-positive bucket, or
    /// levels out of order.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::VolumeProfile;
        ensure(self.start_ms < self.end_ms, k, || {
            "range must be non-empty".into()
        })?;
        ensure(self.bucket.mantissa() > 0, k, || {
            "bucket must be positive".into()
        })?;
        ensure(
            self.levels.windows(2).all(|w| w[0].price < w[1].price),
            k,
            || "levels must be in strictly ascending price order".into(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::record::tests::{header, px, sz};
    use crate::{Record, UnixMillis};

    pub(crate) fn footprint() -> Footprint {
        Footprint {
            prov: sample("hyperliquid:perp:BTC", 12, 0),
            derived: header(2),
            interval: BarInterval::M1,
            start_ms: UnixMillis(1_767_225_600_000),
            levels: vec![
                FootprintLevel {
                    price: px("999.5"),
                    bid_volume: sz("1.0"),
                    ask_volume: sz("0.0"),
                    trade_count: 1,
                },
                FootprintLevel {
                    price: px("1000.0"),
                    bid_volume: sz("0.0"),
                    ask_volume: sz("2.0"),
                    trade_count: 1,
                },
            ],
        }
    }

    pub(crate) fn profile() -> VolumeProfile {
        VolumeProfile {
            prov: sample("hyperliquid:perp:BTC", 12, 0),
            derived: header(2),
            start_ms: UnixMillis(1_767_225_600_000),
            end_ms: UnixMillis(1_767_312_000_000),
            bucket: px("5.0"),
            levels: vec![ProfileLevel {
                price: px("995.0"),
                volume: sz("3.0"),
                buy_volume: sz("2.0"),
                sell_volume: sz("1.0"),
            }],
        }
    }

    #[test]
    fn round_trip_and_validate() {
        for r in [Record::from(footprint()), Record::from(profile())] {
            r.validate().unwrap();
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r);
            assert!(r.derived().is_some());
        }
        let mut f = footprint();
        f.levels.reverse();
        assert!(f.validate().is_err());
        let mut p = profile();
        p.end_ms = p.start_ms;
        assert!(p.validate().is_err());
    }
}
