//! Footprints (`footprint@1`): per-price traded volume by aggressor, per
//! time interval.
//!
//! For each interval with trades, one [`Footprint`] with one level per
//! traded price, in ascending price order: `ask_volume` sums trades with a
//! buy aggressor (lifting the ask), `bid_volume` trades with a sell
//! aggressor (hitting the bid), and `trade_count` counts every trade at the
//! price, including those with an unknown aggressor (whose size is in
//! neither volume). The inputs hash covers the interval's trades in `seq`
//! order.

use std::collections::BTreeMap;

use mesh_types::{
    AggressorSide, BarInterval, ContentHasher, Flags, Footprint, FootprintLevel, Provenance, Trade,
    UnixMillis,
};

use crate::error::InputError;
use crate::fixed::{add, inc, price, same_scale, size, union};
use crate::inputs::{Cursor, TradeIn, encode_trades, header};
use crate::methods;

#[derive(Debug, Clone, Default)]
struct Cell {
    bid: i64,
    ask: i64,
    count: u32,
}

#[derive(Debug, Clone)]
struct Open {
    start: UnixMillis,
    levels: BTreeMap<i64, Cell>,
    hasher: ContentHasher,
    last: Provenance,
    flags: Flags,
    price_scale: u8,
    size_scale: u8,
}

/// Streams footprints of one interval.
#[derive(Debug, Clone)]
pub struct FootprintBuilder {
    interval: BarInterval,
    open: Option<Open>,
    cursor: Cursor,
}

impl FootprintBuilder {
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

    /// Adds the next trade; completed footprints are appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, mixed scales or overflow.
    pub fn push(&mut self, t: TradeIn<'_>, out: &mut Vec<Footprint>) -> Result<(), InputError> {
        let tr = t.trade;
        self.cursor.check(&tr.prov)?;
        let start = self.interval.align(tr.prov.ts_source_ms);
        if self.open.as_ref().is_none_or(|o| o.start != start) {
            self.close(out)?;
            self.open = Some(Open {
                start,
                levels: BTreeMap::new(),
                hasher: ContentHasher::new(),
                last: tr.prov.clone(),
                flags: Flags::NONE,
                price_scale: tr.price.scale(),
                size_scale: tr.size.scale(),
            });
        }
        let Some(o) = &mut self.open else {
            return Ok(());
        };
        same_scale("price", o.price_scale, tr.price.scale())?;
        same_scale("size", o.size_scale, tr.size.scale())?;
        let cell = o.levels.entry(tr.price.mantissa()).or_default();
        match tr.aggressor {
            AggressorSide::Buy => cell.ask = add(cell.ask, tr.size.mantissa())?,
            AggressorSide::Sell => cell.bid = add(cell.bid, tr.size.mantissa())?,
            AggressorSide::Unknown => {}
        }
        cell.count = inc(cell.count)?;
        o.hasher.update_canonical(t.canonical);
        o.last = tr.prov.clone();
        o.flags = union(o.flags, tr.prov.flags);
        Ok(())
    }

    /// Emits the open footprint if it ends at or before `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        ts: UnixMillis,
        out: &mut Vec<Footprint>,
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

    /// Emits the open footprint, if any.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(&mut self, out: &mut Vec<Footprint>) -> Result<(), InputError> {
        self.close(out)
    }

    fn close(&mut self, out: &mut Vec<Footprint>) -> Result<(), InputError> {
        let Some(o) = self.open.take() else {
            return Ok(());
        };
        let levels = o
            .levels
            .iter()
            .map(|(p, c)| {
                Ok(FootprintLevel {
                    price: price(*p, o.price_scale)?,
                    bid_volume: size(c.bid, o.size_scale)?,
                    ask_volume: size(c.ask, o.size_scale)?,
                    trade_count: c.count,
                })
            })
            .collect::<Result<Vec<_>, InputError>>()?;
        let mut prov = o.last;
        prov.flags = o.flags;
        prov.ts_publish_ns = None;
        out.push(Footprint {
            prov,
            derived: header(methods::FOOTPRINT, o.hasher)?,
            interval: self.interval,
            start_ms: o.start,
            levels,
        });
        Ok(())
    }
}

/// Footprints of `interval` from trades in `seq` order (batch form).
///
/// # Errors
///
/// As [`FootprintBuilder::push`].
pub fn footprints(trades: &[Trade], interval: BarInterval) -> Result<Vec<Footprint>, InputError> {
    let canon = encode_trades(trades)?;
    let mut b = FootprintBuilder::new(interval);
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

    #[test]
    fn per_price_volumes() {
        let t = vec![
            trade(1, 0, 0, "100.0", "1.00", Buy),
            trade(1, 1, 0, "99.5", "2.00", Sell),
            trade(2, 0, 500, "100.0", "0.50", Sell),
            trade(2, 1, 500, "100.0", "0.25", Unknown),
            trade(3, 0, 60_000, "101.0", "1.00", Buy),
        ];
        let f = footprints(&t, BarInterval::M1).unwrap();
        assert_eq!(f.len(), 2);
        f[0].validate().unwrap();
        let l = &f[0].levels;
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].price, px("99.5"));
        assert_eq!(l[0].bid_volume, sz("2.00"));
        assert_eq!(l[1].ask_volume, sz("1.00"));
        assert_eq!(l[1].bid_volume, sz("0.50"));
        assert_eq!(l[1].trade_count, 3);
        assert_eq!(f[0].derived.input_count, 4);
        assert_eq!(f[0].prov.seq, t[3].prov.seq);
        assert_eq!(f[1].start_ms, ms(60_000));
    }
}
