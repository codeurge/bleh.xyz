//! Delta and cumulative volume delta (`delta@1`).
//!
//! For each time interval with trades: `delta = buy_volume - sell_volume`
//! (trades with an unknown aggressor count in neither), and `cvd` is the
//! sum of `delta` over every trade from the start of the anchor period
//! (UTC day by default) up to the end of the interval. CVD restarts at
//! each anchor boundary, so a day can be recomputed from that day's trades.
//!
//! Because `cvd` depends on every earlier trade in the anchor period, the
//! inputs hash of a [`DeltaBar`] covers all trades from the anchor start
//! through the end of the interval, in `seq` order, and `input_count` is
//! their number.

/// Record types, defined in `mesh-types`.
pub use mesh_types::DeltaBar;

use mesh_types::{AggressorSide, BarInterval, ContentHasher, Flags, Provenance, Trade, UnixMillis};

use crate::error::InputError;
use crate::fixed::{SignedSize, add, same_scale, size, union};
use crate::inputs::{Cursor, TradeIn, encode_trades, header_of_clone};
use crate::methods;

#[derive(Debug, Clone)]
struct Open {
    start: UnixMillis,
    buy: i64,
    sell: i64,
    last: Provenance,
    flags: Flags,
}

/// Streams delta bars of one interval with CVD over an anchor period.
#[derive(Debug, Clone)]
pub struct DeltaBuilder {
    interval: BarInterval,
    anchor: BarInterval,
    open: Option<Open>,
    anchor_start: Option<UnixMillis>,
    anchor_hasher: ContentHasher,
    cvd: i64,
    size_scale: Option<u8>,
    cursor: Cursor,
}

impl DeltaBuilder {
    /// A builder for `interval` with CVD restarting at each `anchor`
    /// boundary.
    ///
    /// # Errors
    ///
    /// Fails if `anchor` is not a multiple of `interval`.
    pub fn new(interval: BarInterval, anchor: BarInterval) -> Result<Self, InputError> {
        if anchor.millis() % interval.millis() != 0 {
            return Err(InputError::InvalidConfig(format!(
                "CVD anchor {anchor} is not a multiple of {interval}"
            )));
        }
        Ok(Self {
            interval,
            anchor,
            open: None,
            anchor_start: None,
            anchor_hasher: ContentHasher::new(),
            cvd: 0,
            size_scale: None,
            cursor: Cursor::default(),
        })
    }

    /// The interval.
    #[must_use]
    pub fn interval(&self) -> BarInterval {
        self.interval
    }

    /// Adds the next trade; completed delta bars are appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, mixed scales or overflow.
    pub fn push(&mut self, t: TradeIn<'_>, out: &mut Vec<DeltaBar>) -> Result<(), InputError> {
        let tr = t.trade;
        self.cursor.check(&tr.prov)?;
        let ss = *self.size_scale.get_or_insert(tr.size.scale());
        same_scale("size", ss, tr.size.scale())?;
        let ts = tr.prov.ts_source_ms;
        let start = self.interval.align(ts);
        if self.open.as_ref().is_none_or(|o| o.start != start) {
            self.close(out)?;
            let anchor = self.anchor.align(ts);
            if self.anchor_start != Some(anchor) {
                self.anchor_start = Some(anchor);
                self.anchor_hasher = ContentHasher::new();
                self.cvd = 0;
            }
            self.open = Some(Open {
                start,
                buy: 0,
                sell: 0,
                last: tr.prov.clone(),
                flags: Flags::NONE,
            });
        }
        let Some(o) = &mut self.open else {
            return Ok(());
        };
        match tr.aggressor {
            AggressorSide::Buy => o.buy = add(o.buy, tr.size.mantissa())?,
            AggressorSide::Sell => o.sell = add(o.sell, tr.size.mantissa())?,
            AggressorSide::Unknown => {}
        }
        o.last = tr.prov.clone();
        o.flags = union(o.flags, tr.prov.flags);
        self.anchor_hasher.update_canonical(t.canonical);
        Ok(())
    }

    /// Emits the open delta bar if it ends at or before `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        ts: UnixMillis,
        out: &mut Vec<DeltaBar>,
    ) -> Result<(), InputError> {
        self.cursor.advance(ts);
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.start.0.saturating_add(self.interval.millis()) <= ts.0)
        {
            self.close(out)?;
        }
        Ok(())
    }

    /// Emits the open delta bar, if any.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(&mut self, out: &mut Vec<DeltaBar>) -> Result<(), InputError> {
        self.close(out)
    }

    fn close(&mut self, out: &mut Vec<DeltaBar>) -> Result<(), InputError> {
        let Some(o) = self.open.take() else {
            return Ok(());
        };
        let ss = self.size_scale.unwrap_or(0);
        let delta = o.buy.checked_sub(o.sell).ok_or(InputError::Overflow)?;
        self.cvd = add(self.cvd, delta)?;
        let mut prov = o.last;
        prov.flags = o.flags;
        prov.ts_publish_ns = None;
        out.push(DeltaBar {
            prov,
            derived: header_of_clone(methods::DELTA, &self.anchor_hasher)?,
            interval: self.interval,
            start_ms: o.start,
            buy_volume: size(o.buy, ss)?,
            sell_volume: size(o.sell, ss)?,
            volume_delta: SignedSize::new(delta, ss)?,
            anchor_start_ms: self.anchor_start.unwrap_or(o.start),
            cvd: SignedSize::new(self.cvd, ss)?,
        });
        Ok(())
    }
}

/// Delta bars from trades in `seq` order (batch form).
///
/// # Errors
///
/// As [`DeltaBuilder::push`].
pub fn deltas(
    trades: &[Trade],
    interval: BarInterval,
    anchor: BarInterval,
) -> Result<Vec<DeltaBar>, InputError> {
    let canon = encode_trades(trades)?;
    let mut b = DeltaBuilder::new(interval, anchor)?;
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
    use mesh_types::{Record, content_hash};

    #[test]
    fn delta_and_cvd_reset_daily() {
        let t = vec![
            trade(1, 0, 0, "1.0", "3.00", Buy),
            trade(1, 1, 10, "1.0", "1.00", Sell),
            trade(2, 0, 60_000, "1.0", "5.00", Sell),
            trade(2, 1, 60_001, "1.0", "9.00", Unknown),
            trade(3, 0, 86_400_000, "1.0", "1.00", Buy),
        ];
        let d = deltas(&t, BarInterval::M1, BarInterval::D1).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].volume_delta.to_string(), "2.00");
        assert_eq!(d[0].cvd.to_string(), "2.00");
        assert_eq!(d[1].volume_delta.to_string(), "-5.00");
        assert_eq!(d[1].cvd.to_string(), "-3.00");
        assert_eq!(d[2].cvd.to_string(), "1.00");
        assert_eq!(d[2].anchor_start_ms, ms(86_400_000));
        // Inputs: the day's trades through the interval.
        assert_eq!(d[1].derived.input_count, 4);
        let recs: Vec<Record> = t[..4].iter().cloned().map(Record::from).collect();
        assert_eq!(d[1].derived.inputs_hash, content_hash(&recs).unwrap());
        assert_eq!(d[2].derived.input_count, 1);
        assert!(DeltaBuilder::new(BarInterval::H1, BarInterval::M15).is_err());
    }
}
