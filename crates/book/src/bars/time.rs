//! Time bars (`bars@1`).
//!
//! A trade at `ts_source_ms = t` belongs to the bar starting at
//! `interval.align(t)`. A bar is emitted once a trade arrives in a later
//! interval, time is advanced past its end ([`TimeBarBuilder::advance_to`])
//! or the builder is finished. Intervals without trades emit nothing. The
//! inputs hash covers the bar's trades in `seq` order.

use mesh_types::{Bar, BarInterval, ContentHasher, Trade, UnixMillis};

use super::Ohlcv;
use crate::error::InputError;
use crate::inputs::{Cursor, TradeIn, encode_trades, header};
use crate::methods;

#[derive(Debug, Clone)]
struct Open {
    start: UnixMillis,
    acc: Ohlcv,
    hasher: ContentHasher,
}

/// Streams time bars of one interval from trades.
#[derive(Debug, Clone)]
pub struct TimeBarBuilder {
    interval: BarInterval,
    open: Option<Open>,
    cursor: Cursor,
}

impl TimeBarBuilder {
    /// A builder for `interval`.
    #[must_use]
    pub fn new(interval: BarInterval) -> Self {
        Self {
            interval,
            open: None,
            cursor: Cursor::default(),
        }
    }

    /// The interval.
    #[must_use]
    pub fn interval(&self) -> BarInterval {
        self.interval
    }

    /// Adds the next trade; completed bars are appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, mixed scales or overflow.
    pub fn push(&mut self, t: TradeIn<'_>, out: &mut Vec<Bar>) -> Result<(), InputError> {
        self.cursor.check(&t.trade.prov)?;
        let start = self.interval.align(t.trade.prov.ts_source_ms);
        if let Some(open) = &mut self.open
            && open.start == start
        {
            open.acc.push(t.trade)?;
            open.hasher.update_canonical(t.canonical);
            return Ok(());
        }
        self.close(out)?;
        let mut hasher = ContentHasher::new();
        hasher.update_canonical(t.canonical);
        self.open = Some(Open {
            start,
            acc: Ohlcv::new(t.trade),
            hasher,
        });
        Ok(())
    }

    /// Emits the open bar if it ends at or before `ts`; later trades must
    /// not be earlier than `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(&mut self, ts: UnixMillis, out: &mut Vec<Bar>) -> Result<(), InputError> {
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

    /// Emits the open bar, if any.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(&mut self, out: &mut Vec<Bar>) -> Result<(), InputError> {
        self.close(out)
    }

    fn close(&mut self, out: &mut Vec<Bar>) -> Result<(), InputError> {
        if let Some(open) = self.open.take() {
            let derived = header(methods::BARS, open.hasher)?;
            out.push(open.acc.to_bar(self.interval, open.start, derived)?);
        }
        Ok(())
    }
}

/// Time bars of `interval` from trades in `seq` order (batch form).
///
/// # Errors
///
/// Fails on out-of-order input, mixed scales or overflow.
pub fn time_bars(trades: &[Trade], interval: BarInterval) -> Result<Vec<Bar>, InputError> {
    let canon = encode_trades(trades)?;
    let mut b = TimeBarBuilder::new(interval);
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
    use mesh_types::{AggressorSide::*, Flag, Record, content_hash};

    #[test]
    fn ohlcv_and_aggressor_volumes() {
        let trades = vec![
            trade(1, 0, 100, "100.0", "1.00", Buy),
            trade(1, 1, 100, "101.0", "0.50", Sell),
            trade(2, 0, 900, "99.5", "2.00", Unknown),
            trade(3, 0, 1_500, "100.5", "1.00", Buy),
            // 2 s with no trades: no bar.
            trade(4, 0, 3_000, "102.0", "1.00", Sell),
        ];
        let bars = time_bars(&trades, BarInterval::S1).unwrap();
        assert_eq!(bars.len(), 3);
        let b = &bars[0];
        assert_eq!(b.start_ms, ms(0));
        assert_eq!(
            (b.open, b.high, b.low, b.close),
            (px("100.0"), px("101.0"), px("99.5"), px("99.5"))
        );
        assert_eq!(b.volume, sz("3.50"));
        assert_eq!(b.buy_volume, sz("1.00"));
        assert_eq!(b.sell_volume, sz("0.50"));
        assert_eq!(b.trade_count, 3);
        assert_eq!(b.prov.seq, trades[2].prov.seq);
        let records: Vec<Record> = trades[..3].iter().cloned().map(Record::from).collect();
        assert_eq!(b.derived.inputs_hash, content_hash(&records).unwrap());
        assert_eq!(b.derived.method.as_str(), "bars@1");
        for b in &bars {
            b.validate().unwrap();
        }
        assert_eq!(bars[2].start_ms, ms(3_000));
    }

    #[test]
    fn flags_are_united_and_publish_time_dropped() {
        let mut a = trade(1, 0, 0, "1.0", "1.00", Buy);
        a.prov.flags = a.prov.flags.with(Flag::GapBefore);
        a.prov.stamp_publish(mesh_types::UnixNanos(9));
        let b = trade(1, 1, 10, "1.0", "1.00", Buy);
        let bars = time_bars(&[a, b], BarInterval::M1).unwrap();
        assert!(bars[0].prov.flags.contains(Flag::GapBefore));
        assert!(bars[0].prov.ts_publish_ns.is_none());
    }

    #[test]
    fn rejects_bad_order_and_scales() {
        let t1 = trade(2, 0, 0, "1.0", "1.00", Buy);
        let t0 = trade(1, 0, 10, "1.0", "1.00", Buy);
        assert!(matches!(
            time_bars(&[t1.clone(), t0], BarInterval::S1),
            Err(InputError::OutOfOrder { .. })
        ));
        let back = trade(3, 0, -10, "1.0", "1.00", Buy);
        assert!(matches!(
            time_bars(&[t1.clone(), back], BarInterval::S1),
            Err(InputError::TimeWentBackwards { .. })
        ));
        let other = trade(3, 0, 10, "1.00", "1.00", Buy);
        assert!(matches!(
            time_bars(&[t1, other], BarInterval::S1),
            Err(InputError::ScaleMismatch { .. })
        ));
    }

    #[test]
    fn advance_closes_and_sets_watermark() {
        let t = trade(1, 0, 100, "1.0", "1.00", Buy);
        let c = crate::inputs::canonical_trade(&t).unwrap();
        let mut b = TimeBarBuilder::new(BarInterval::S1);
        let mut out = Vec::new();
        b.push(
            TradeIn {
                trade: &t,
                canonical: &c,
            },
            &mut out,
        )
        .unwrap();
        b.advance_to(ms(999), &mut out).unwrap();
        assert!(out.is_empty());
        b.advance_to(ms(1_000), &mut out).unwrap();
        assert_eq!(out.len(), 1);
        let late = trade(2, 0, 999, "1.0", "1.00", Buy);
        let c2 = crate::inputs::canonical_trade(&late).unwrap();
        assert!(
            b.push(
                TradeIn {
                    trade: &late,
                    canonical: &c2
                },
                &mut out
            )
            .is_err()
        );
    }
}
