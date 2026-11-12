//! Tick, volume and range bars ([`ActivityBar`]).
//!
//! Activity bars close on trading activity instead of the clock:
//!
//! - **Tick** (`tick-bars@1`): after every `trades` trades.
//! - **Volume** (`volume-bars@1`): after the trade that brings the bar's
//!   volume to at least `size`. Trades are never split, so a bar can
//!   exceed the threshold.
//! - **Range** (`range-bars@1`): a trade that would make `high - low`
//!   exceed `width` closes the bar without it and opens the next bar with
//!   it, so every bar's range is at most `width`. No synthetic prices.
//!
//! Every bar series restarts at each `reset` boundary (UTC day by
//! default), so a day can be recomputed from that day's trades alone. A
//! bar still open at a reset, or when the builder finishes, is emitted with
//! `complete = false`.

/// Record types, defined in `mesh-types`.
pub use mesh_types::{ActivityBar, ActivitySpec};

use mesh_types::{BarInterval, ContentHasher, Trade, UnixMillis};

use super::Ohlcv;
use crate::error::InputError;
use crate::fixed::{price, size};
use crate::inputs::{Cursor, TradeIn, encode_trades, header};

#[derive(Debug, Clone)]
struct Open {
    session: UnixMillis,
    acc: Ohlcv,
    hasher: ContentHasher,
}

/// Streams activity bars of one specification.
#[derive(Debug, Clone)]
pub struct ActivityBarBuilder {
    spec: ActivitySpec,
    reset: BarInterval,
    threshold: Option<i64>,
    open: Option<Open>,
    cursor: Cursor,
}

impl ActivityBarBuilder {
    /// A builder for `spec`, restarting at each `reset` boundary.
    ///
    /// # Errors
    ///
    /// Fails on a non-positive threshold.
    pub fn new(spec: ActivitySpec, reset: BarInterval) -> Result<Self, InputError> {
        if !spec.is_valid() {
            return Err(InputError::InvalidConfig(format!(
                "activity bar threshold must be positive: {spec:?}"
            )));
        }
        Ok(Self {
            spec,
            reset,
            threshold: None,
            open: None,
            cursor: Cursor::default(),
        })
    }

    /// The specification.
    #[must_use]
    pub fn spec(&self) -> ActivitySpec {
        self.spec
    }

    /// Adds the next trade; completed bars are appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, mixed scales, a threshold finer than
    /// the instrument's scale, or overflow.
    pub fn push(&mut self, t: TradeIn<'_>, out: &mut Vec<ActivityBar>) -> Result<(), InputError> {
        self.cursor.check(&t.trade.prov)?;
        let threshold = self.threshold(t.trade)?;
        let session = self.reset.align(t.trade.prov.ts_source_ms);
        if self.open.as_ref().is_some_and(|o| o.session != session) {
            self.close(false, out)?;
        }
        if let (ActivitySpec::Range { .. }, Some(o)) = (self.spec, &self.open) {
            let p = t.trade.price.mantissa();
            let hi = o.acc.high.max(p);
            let lo = o.acc.low.min(p);
            if i128::from(hi) - i128::from(lo) > i128::from(threshold) {
                self.close(true, out)?;
            }
        }
        match &mut self.open {
            Some(o) => {
                o.acc.push(t.trade)?;
                o.hasher.update_canonical(t.canonical);
            }
            None => {
                let mut hasher = ContentHasher::new();
                hasher.update_canonical(t.canonical);
                self.open = Some(Open {
                    session,
                    acc: Ohlcv::new(t.trade),
                    hasher,
                });
            }
        }
        let done = self.open.as_ref().is_some_and(|o| match self.spec {
            ActivitySpec::Tick { trades } => o.acc.count >= trades,
            ActivitySpec::Volume { .. } => o.acc.volume >= threshold,
            ActivitySpec::Range { .. } => false,
        });
        if done {
            self.close(true, out)?;
        }
        Ok(())
    }

    /// Emits the open bar, incomplete, if its reset period ended at or
    /// before `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        ts: UnixMillis,
        out: &mut Vec<ActivityBar>,
    ) -> Result<(), InputError> {
        self.cursor.advance(ts);
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.session.0.saturating_add(self.reset.millis()) <= ts.0)
        {
            self.close(false, out)?;
        }
        Ok(())
    }

    /// Emits the open bar, incomplete.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(&mut self, out: &mut Vec<ActivityBar>) -> Result<(), InputError> {
        self.close(false, out)
    }

    /// The threshold as a mantissa at the trade's scale (fixed at the first
    /// trade).
    fn threshold(&mut self, t: &Trade) -> Result<i64, InputError> {
        if let Some(th) = self.threshold {
            return Ok(th);
        }
        let th = match self.spec {
            ActivitySpec::Tick { trades } => i64::from(trades),
            ActivitySpec::Volume { size } => size.rescale(t.size.scale())?.mantissa(),
            ActivitySpec::Range { width } => width.rescale(t.price.scale())?.mantissa(),
        };
        self.threshold = Some(th);
        Ok(th)
    }

    fn close(&mut self, complete: bool, out: &mut Vec<ActivityBar>) -> Result<(), InputError> {
        let Some(o) = self.open.take() else {
            return Ok(());
        };
        let a = &o.acc;
        let (ps, ss) = (a.price_scale, a.size_scale);
        out.push(ActivityBar {
            prov: a.prov(),
            derived: header(self.spec.method(), o.hasher)?,
            spec: self.spec,
            session_start_ms: o.session,
            first_seq: a.first_seq,
            first_ms: a.first_ms,
            open: price(a.open, ps)?,
            high: price(a.high, ps)?,
            low: price(a.low, ps)?,
            close: price(a.close, ps)?,
            volume: size(a.volume, ss)?,
            buy_volume: size(a.buy, ss)?,
            sell_volume: size(a.sell, ss)?,
            trade_count: a.count,
            complete,
        });
        Ok(())
    }
}

/// Activity bars from trades in `seq` order (batch form).
///
/// # Errors
///
/// As [`ActivityBarBuilder::push`].
pub fn activity_bars(
    trades: &[Trade],
    spec: ActivitySpec,
    reset: BarInterval,
) -> Result<Vec<ActivityBar>, InputError> {
    let canon = encode_trades(trades)?;
    let mut b = ActivityBarBuilder::new(spec, reset)?;
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

    fn tape(prices: &[&str], sizes: &[&str]) -> Vec<Trade> {
        prices
            .iter()
            .zip(sizes)
            .enumerate()
            .map(|(i, (p, s))| {
                let i = u32::try_from(i).unwrap();
                trade(u64::from(i) + 1, 0, i64::from(i) * 10, p, s, Buy)
            })
            .collect()
    }

    #[test]
    fn tick_bars() {
        let t = tape(&["1.0"; 5], &["1.00"; 5]);
        let bars = activity_bars(&t, ActivitySpec::Tick { trades: 2 }, BarInterval::D1).unwrap();
        assert_eq!(
            bars.iter().map(|b| b.trade_count).collect::<Vec<_>>(),
            [2, 2, 1]
        );
        assert_eq!(
            bars.iter().map(|b| b.complete).collect::<Vec<_>>(),
            [true, true, false]
        );
        assert_eq!(bars[1].first_seq, t[2].prov.seq);
        assert_eq!(bars[0].derived.input_count, 2);
        assert_eq!(bars[0].derived.method.as_str(), "tick-bars@1");
    }

    #[test]
    fn volume_bars_do_not_split_trades() {
        let t = tape(&["1.0"; 4], &["0.60", "0.60", "2.00", "0.10"]);
        let spec = ActivitySpec::Volume { size: sz("1.0") };
        let bars = activity_bars(&t, spec, BarInterval::D1).unwrap();
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].volume, sz("1.20"));
        assert_eq!(bars[1].volume, sz("2.00"));
        assert!(!bars[2].complete);
        // Threshold finer than the size scale is rejected.
        let fine = ActivitySpec::Volume { size: sz("0.001") };
        assert!(activity_bars(&t, fine, BarInterval::D1).is_err());
    }

    #[test]
    fn range_bars_stay_within_width() {
        let t = tape(
            &["10.0", "10.4", "10.5", "10.6", "10.1", "9.9"],
            &["1.00"; 6],
        );
        let spec = ActivitySpec::Range { width: px("0.5") };
        let bars = activity_bars(&t, spec, BarInterval::D1).unwrap();
        // [10.0 10.4 10.5] | [10.6 10.1] | [9.9]
        assert_eq!(
            bars.iter().map(|b| b.trade_count).collect::<Vec<_>>(),
            [3, 2, 1]
        );
        for b in &bars {
            assert!(b.high.mantissa() - b.low.mantissa() <= 5);
        }
        assert!(bars[0].complete && bars[1].complete && !bars[2].complete);
    }

    #[test]
    fn reset_at_utc_day() {
        let a = trade(1, 0, 86_399_000, "1.0", "1.00", Sell);
        let b = trade(2, 0, 86_400_000, "1.0", "1.00", Sell);
        let bars =
            activity_bars(&[a, b], ActivitySpec::Tick { trades: 5 }, BarInterval::D1).unwrap();
        assert_eq!(bars.len(), 2);
        assert!(!bars[0].complete);
        assert_eq!(bars[1].session_start_ms, ms(86_400_000));
        assert_eq!(bars[0].sell_volume, sz("1.00"));
        assert!(
            ActivityBarBuilder::new(ActivitySpec::Tick { trades: 0 }, BarInterval::D1).is_err()
        );
    }
}
