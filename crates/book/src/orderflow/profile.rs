//! Volume profiles (`volume-profile@1`) and value areas (`value-area@1`).
//!
//! # Profile
//!
//! Over a period (UTC day by default, aligned to the epoch), each trade's
//! size is added to the bucket `floor(price / bucket) * bucket` (bucket
//! width at the instrument's price scale; one tick by default), split into
//! buy and sell aggressor volume. Buckets without trades are omitted. One
//! [`VolumeProfile`] per period with trades; inputs are the period's trades
//! in `seq` order.
//!
//! # Value area
//!
//! [`value_area`] computes, from a profile's buckets:
//!
//! - **POC** (point of control): the bucket with the most volume; on a
//!   tie, the lowest price.
//! - **Value area**: starting from the POC, repeatedly add the next traded
//!   bucket above or below the current range, whichever has more volume
//!   (both on a tie; the only one left at an edge), until the included
//!   volume is at least `ceil(total * value_area_bps / 10_000)`.
//!   `value_area_low` and `value_area_high` are the lower bounds of the
//!   lowest and highest included buckets. Empty buckets between traded
//!   ones are skipped, not compared.
//!
//! A [`ProfileSummary`] carries the result with the same inputs as its
//! profile.

/// Record types, defined in `mesh-types`.
pub use mesh_types::ProfileSummary;

use std::collections::BTreeMap;

use mesh_types::{
    AggressorSide, BarInterval, ContentHasher, Flags, Price, ProfileLevel, Provenance, Size, Trade,
    UnixMillis, VolumeProfile,
};
use serde::{Deserialize, Serialize};

use crate::error::InputError;
use crate::fixed::{add, ceil_bps, price, same_scale, size, union};
use crate::inputs::{Cursor, TradeIn, encode_trades, header};
use crate::methods;

/// Default value-area share: 70%.
pub const DEFAULT_VALUE_AREA_BPS: u32 = 7_000;

/// Point of control and value area of a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueArea {
    /// Lower bound of the bucket with the most volume.
    pub poc: Price,
    /// Lower bound of the lowest bucket in the value area.
    pub low: Price,
    /// Lower bound of the highest bucket in the value area.
    pub high: Price,
    /// Volume in the value area.
    pub volume: Size,
    /// Total volume of the profile.
    pub total: Size,
}

/// The value area of `levels` (ascending, as in a [`VolumeProfile`]), or
/// `None` for an empty profile or zero volume.
///
/// # Errors
///
/// Fails on mixed scales, overflow or a share above 100%.
pub fn value_area(
    levels: &[ProfileLevel],
    value_area_bps: u32,
) -> Result<Option<ValueArea>, InputError> {
    if value_area_bps > 10_000 {
        return Err(InputError::InvalidConfig(format!(
            "value area share {value_area_bps} bps exceeds 100%"
        )));
    }
    let Some(first) = levels.first() else {
        return Ok(None);
    };
    let ss = first.volume.scale();
    let mut total: i128 = 0;
    for l in levels {
        same_scale("size", ss, l.volume.scale())?;
        total += i128::from(l.volume.mantissa());
    }
    if total == 0 {
        return Ok(None);
    }
    let vol = |i: usize| i128::from(levels[i].volume.mantissa());
    // POC: max volume, lowest price on a tie (levels ascend).
    let mut poc = 0;
    for i in 1..levels.len() {
        if vol(i) > vol(poc) {
            poc = i;
        }
    }
    let target = ceil_bps(total, value_area_bps);
    let (mut lo, mut hi) = (poc, poc);
    let mut acc = vol(poc);
    while acc < target {
        let below = lo.checked_sub(1);
        let above = (hi + 1 < levels.len()).then_some(hi + 1);
        match (below, above) {
            (Some(b), Some(a)) => {
                let (vb, va) = (vol(b), vol(a));
                if va >= vb {
                    hi = a;
                    acc += va;
                }
                if vb >= va {
                    lo = b;
                    acc += vb;
                }
            }
            (Some(b), None) => {
                lo = b;
                acc += vol(b);
            }
            (None, Some(a)) => {
                hi = a;
                acc += vol(a);
            }
            (None, None) => break,
        }
    }
    let to_size = |v: i128| -> Result<Size, InputError> {
        size(i64::try_from(v).map_err(|_| InputError::Overflow)?, ss)
    };
    Ok(Some(ValueArea {
        poc: levels[poc].price,
        low: levels[lo].price,
        high: levels[hi].price,
        volume: to_size(acc)?,
        total: to_size(total)?,
    }))
}

/// Volume profile settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Period of each profile.
    pub period: BarInterval,
    /// Bucket width; `None` means one tick (mantissa 1 at the price scale).
    pub bucket: Option<Price>,
    /// Value-area share in basis points.
    pub value_area_bps: u32,
}

impl Default for ProfileConfig {
    /// Daily profiles, one-tick buckets, 70% value area.
    fn default() -> Self {
        Self {
            period: BarInterval::D1,
            bucket: None,
            value_area_bps: DEFAULT_VALUE_AREA_BPS,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Bucket {
    volume: i64,
    buy: i64,
    sell: i64,
}

#[derive(Debug, Clone)]
struct Open {
    start: UnixMillis,
    buckets: BTreeMap<i64, Bucket>,
    hasher: ContentHasher,
    last: Provenance,
    flags: Flags,
}

/// Streams volume profiles and their summaries.
#[derive(Debug, Clone)]
pub struct ProfileBuilder {
    config: ProfileConfig,
    bucket: Option<i64>,
    scales: Option<(u8, u8)>,
    open: Option<Open>,
    cursor: Cursor,
}

impl ProfileBuilder {
    /// A builder for `config`.
    ///
    /// # Errors
    ///
    /// Fails on a non-positive bucket or a share above 100%.
    pub fn new(config: ProfileConfig) -> Result<Self, InputError> {
        if config.bucket.is_some_and(|b| b.mantissa() <= 0) || config.value_area_bps > 10_000 {
            return Err(InputError::InvalidConfig(format!(
                "invalid volume profile settings: {config:?}"
            )));
        }
        Ok(Self {
            config,
            bucket: None,
            scales: None,
            open: None,
            cursor: Cursor::default(),
        })
    }

    /// Adds the next trade; completed profiles are appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, mixed scales, a bucket finer than the
    /// price scale, or overflow.
    pub fn push(
        &mut self,
        t: TradeIn<'_>,
        out: &mut Vec<(VolumeProfile, Option<ProfileSummary>)>,
    ) -> Result<(), InputError> {
        let tr = t.trade;
        self.cursor.check(&tr.prov)?;
        let (ps, ss) = *self
            .scales
            .get_or_insert((tr.price.scale(), tr.size.scale()));
        same_scale("price", ps, tr.price.scale())?;
        same_scale("size", ss, tr.size.scale())?;
        let bucket = match self.bucket {
            Some(b) => b,
            None => {
                let b = match self.config.bucket {
                    Some(b) => b.rescale(ps)?.mantissa(),
                    None => 1,
                };
                self.bucket = Some(b);
                b
            }
        };
        let start = self.config.period.align(tr.prov.ts_source_ms);
        if self.open.as_ref().is_none_or(|o| o.start != start) {
            self.close(out)?;
            self.open = Some(Open {
                start,
                buckets: BTreeMap::new(),
                hasher: ContentHasher::new(),
                last: tr.prov.clone(),
                flags: Flags::NONE,
            });
        }
        let Some(o) = &mut self.open else {
            return Ok(());
        };
        let key = tr
            .price
            .mantissa()
            .div_euclid(bucket)
            .checked_mul(bucket)
            .ok_or(InputError::Overflow)?;
        let b = o.buckets.entry(key).or_default();
        let s = tr.size.mantissa();
        b.volume = add(b.volume, s)?;
        match tr.aggressor {
            AggressorSide::Buy => b.buy = add(b.buy, s)?,
            AggressorSide::Sell => b.sell = add(b.sell, s)?,
            AggressorSide::Unknown => {}
        }
        o.hasher.update_canonical(t.canonical);
        o.last = tr.prov.clone();
        o.flags = union(o.flags, tr.prov.flags);
        Ok(())
    }

    /// Emits the open profile if its period ends at or before `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        ts: UnixMillis,
        out: &mut Vec<(VolumeProfile, Option<ProfileSummary>)>,
    ) -> Result<(), InputError> {
        self.cursor.advance(ts);
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.start.0.saturating_add(self.config.period.millis()) <= ts.0)
        {
            self.close(out)?;
        }
        Ok(())
    }

    /// Emits the open profile, if any.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(
        &mut self,
        out: &mut Vec<(VolumeProfile, Option<ProfileSummary>)>,
    ) -> Result<(), InputError> {
        self.close(out)
    }

    fn close(
        &mut self,
        out: &mut Vec<(VolumeProfile, Option<ProfileSummary>)>,
    ) -> Result<(), InputError> {
        let Some(o) = self.open.take() else {
            return Ok(());
        };
        let (ps, ss) = self.scales.unwrap_or((0, 0));
        let levels = o
            .buckets
            .iter()
            .map(|(p, b)| {
                Ok(ProfileLevel {
                    price: price(*p, ps)?,
                    volume: size(b.volume, ss)?,
                    buy_volume: size(b.buy, ss)?,
                    sell_volume: size(b.sell, ss)?,
                })
            })
            .collect::<Result<Vec<_>, InputError>>()?;
        let mut prov = o.last;
        prov.flags = o.flags;
        prov.ts_publish_ns = None;
        let end = UnixMillis(o.start.0.saturating_add(self.config.period.millis()));
        let bucket = price(self.bucket.unwrap_or(1), ps)?;
        let summary = match value_area(&levels, self.config.value_area_bps)? {
            Some(va) => Some(ProfileSummary {
                prov: prov.clone(),
                derived: header(methods::VALUE_AREA, o.hasher.clone())?,
                start_ms: o.start,
                end_ms: end,
                bucket,
                value_area_bps: self.config.value_area_bps,
                poc: va.poc,
                value_area_low: va.low,
                value_area_high: va.high,
                value_area_volume: va.volume,
                total_volume: va.total,
            }),
            None => None,
        };
        let profile = VolumeProfile {
            prov,
            derived: header(methods::VOLUME_PROFILE, o.hasher)?,
            start_ms: o.start,
            end_ms: end,
            bucket,
            levels,
        };
        out.push((profile, summary));
        Ok(())
    }
}

/// Volume profiles and summaries from trades in `seq` order (batch form).
///
/// # Errors
///
/// As [`ProfileBuilder::push`].
pub fn volume_profiles(
    trades: &[Trade],
    config: ProfileConfig,
) -> Result<Vec<(VolumeProfile, Option<ProfileSummary>)>, InputError> {
    let canon = encode_trades(trades)?;
    let mut b = ProfileBuilder::new(config)?;
    let mut out = Vec::new();
    for (trade, canonical) in trades.iter().zip(&canon) {
        b.push(TradeIn { trade, canonical }, &mut out)?;
    }
    b.finish(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use mesh_types::AggressorSide::*;

    fn lvl(p: &str, v: &str) -> ProfileLevel {
        ProfileLevel {
            price: px(p),
            volume: sz(v),
            buy_volume: sz("0"),
            sell_volume: sz("0"),
        }
    }

    #[test]
    fn value_area_expands_toward_larger_side() {
        // Volumes 10 20 50 15 5 => total 100, target 70.
        let levels = [
            lvl("1", "10"),
            lvl("2", "20"),
            lvl("3", "50"),
            lvl("4", "15"),
            lvl("5", "5"),
        ];
        let va = value_area(&levels, 7_000).unwrap().unwrap();
        assert_eq!(va.poc, px("3"));
        // 50 + 20 (below beats 15) = 70.
        assert_eq!((va.low, va.high), (px("2"), px("3")));
        assert_eq!(va.volume, sz("70"));
        assert_eq!(va.total, sz("100"));
        let all = value_area(&levels, 10_000).unwrap().unwrap();
        assert_eq!((all.low, all.high), (px("1"), px("5")));
        assert!(value_area(&levels, 10_001).is_err());
        assert_eq!(value_area(&[], 7_000).unwrap(), None);
    }

    #[test]
    fn ties_take_both_and_lowest_poc() {
        let levels = [lvl("1", "5"), lvl("2", "9"), lvl("3", "5"), lvl("4", "9")];
        let va = value_area(&levels, 7_000).unwrap().unwrap();
        assert_eq!(va.poc, px("2"));
        // target ceil(28 * 0.7) = 20: 9, then 5 and 5 on a tie = 19, then 9.
        assert_eq!((va.low, va.high), (px("1"), px("4")));
    }

    #[test]
    fn profile_buckets_and_summary() {
        let t = vec![
            trade(1, 0, 0, "100.0", "1.00", Buy),
            trade(1, 1, 0, "100.4", "2.00", Sell),
            trade(2, 0, 10, "100.5", "1.00", Buy),
            trade(2, 1, 10, "99.9", "0.50", Unknown),
            trade(3, 0, 86_400_000, "101.0", "1.00", Buy),
        ];
        let cfg = ProfileConfig {
            bucket: Some(px("0.5")),
            ..ProfileConfig::default()
        };
        let p = volume_profiles(&t, cfg).unwrap();
        assert_eq!(p.len(), 2);
        let (prof, summary) = &p[0];
        prof.validate().unwrap();
        let prices: Vec<_> = prof.levels.iter().map(|l| l.price.to_string()).collect();
        assert_eq!(prices, ["99.5", "100.0", "100.5"]);
        assert_eq!(prof.levels[1].volume, sz("3.00"));
        assert_eq!(prof.levels[1].buy_volume, sz("1.00"));
        assert_eq!(prof.levels[1].sell_volume, sz("2.00"));
        assert_eq!(prof.end_ms, ms(86_400_000));
        assert_eq!(prof.bucket, px("0.5"));
        let s = summary.as_ref().unwrap();
        assert_eq!(s.poc, px("100.0"));
        assert_eq!(s.derived.inputs_hash, prof.derived.inputs_hash);
        assert_eq!(s.derived.method.as_str(), "value-area@1");
        // A bucket finer than the price scale is rejected.
        let fine = ProfileConfig {
            bucket: Some(px("0.05")),
            ..ProfileConfig::default()
        };
        assert!(volume_profiles(&t, fine).is_err());
    }
}
