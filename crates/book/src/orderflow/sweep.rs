//! Sweep detection (`sweep@1`): aggressive orders that consume several
//! price levels within one block.
//!
//! The mesh has no taker order ID (owner and order linkage are excluded
//! fields), so a sweep is inferred from the trade sequence. Within one
//! block, a **run** is a maximal sequence of consecutive trades (in `seq`
//! order) with the same known aggressor side whose prices never move
//! against the aggressor: non-decreasing for a buyer, non-increasing for a
//! seller. A trade with an unknown aggressor ends a run. A run that trades
//! at `min_levels` or more distinct prices is a [`Sweep`].
//!
//! Venues write a taker's fills consecutively, in the order the book was
//! consumed, so a sweeping order appears as one run. Two takers in a row on
//! the same side whose prices happen to continue monotonically merge into
//! one run; this is a known limit of the heuristic. The inputs hash covers
//! the run's trades in `seq` order.

use mesh_types::{
    AggressorSide, ContentHasher, DerivedHeader, Price, Provenance, Seq, Size, Trade,
};
use serde::{Deserialize, Serialize};

use crate::error::InputError;
use crate::fixed::{add, inc, price, same_scale, size, union};
use crate::inputs::{Cursor, TradeIn, encode_trades, header};
use crate::methods;

/// Sweep settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SweepConfig {
    /// Minimum distinct prices in a run.
    pub min_levels: u32,
}

impl Default for SweepConfig {
    /// Two or more levels.
    fn default() -> Self {
        Self { min_levels: 2 }
    }
}

/// An aggressive run across several price levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sweep {
    /// Provenance of the run's last trade, with the union of flags.
    pub prov: Provenance,
    /// `sweep@1`; inputs are the run's trades.
    pub derived: DerivedHeader,
    /// Aggressor side (`buy` or `sell`).
    pub side: AggressorSide,
    /// `seq` of the first trade.
    pub first_seq: Seq,
    /// Price of the first trade.
    pub first_price: Price,
    /// Price of the last trade (the deepest level reached).
    pub last_price: Price,
    /// Distinct prices traded.
    pub levels: u32,
    /// Total size.
    pub size: Size,
    /// Number of trades.
    pub trade_count: u32,
}

#[derive(Debug, Clone)]
struct Run {
    side: AggressorSide,
    first_seq: Seq,
    first: i64,
    last: i64,
    levels: u32,
    size: i64,
    count: u32,
    price_scale: u8,
    size_scale: u8,
    prov: Provenance,
    hasher: ContentHasher,
}

/// Finds sweeps in each block's trades.
#[derive(Debug, Clone)]
pub struct SweepDetector {
    config: SweepConfig,
    cursor: Cursor,
}

impl SweepDetector {
    /// A detector for `config`.
    ///
    /// # Errors
    ///
    /// Fails if `min_levels` is below 2.
    pub fn new(config: SweepConfig) -> Result<Self, InputError> {
        if config.min_levels < 2 {
            return Err(InputError::InvalidConfig(
                "a sweep needs at least two levels".into(),
            ));
        }
        Ok(Self {
            config,
            cursor: Cursor::default(),
        })
    }

    /// Scans one block's trades (all from the same block, in `seq` order).
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, trades from several blocks, mixed
    /// scales or overflow.
    pub fn on_block(
        &mut self,
        trades: &[TradeIn<'_>],
        out: &mut Vec<Sweep>,
    ) -> Result<(), InputError> {
        let Some(block) = trades.first().map(|t| t.trade.prov.seq.block_number) else {
            return Ok(());
        };
        let mut run: Option<Run> = None;
        for t in trades {
            let tr = t.trade;
            self.cursor.check(&tr.prov)?;
            if tr.prov.seq.block_number != block {
                return Err(InputError::Inconsistent(
                    "sweep detection got trades from several blocks".into(),
                ));
            }
            let p = tr.price.mantissa();
            let extends = run.as_ref().is_some_and(|r| {
                r.side == tr.aggressor
                    && match r.side {
                        AggressorSide::Buy => p >= r.last,
                        AggressorSide::Sell => p <= r.last,
                        AggressorSide::Unknown => false,
                    }
            });
            if extends {
                if let Some(r) = &mut run {
                    same_scale("price", r.price_scale, tr.price.scale())?;
                    same_scale("size", r.size_scale, tr.size.scale())?;
                    if p != r.last {
                        r.levels = inc(r.levels)?;
                    }
                    r.last = p;
                    r.size = add(r.size, tr.size.mantissa())?;
                    r.count = inc(r.count)?;
                    let flags = union(r.prov.flags, tr.prov.flags);
                    r.prov = tr.prov.clone();
                    r.prov.flags = flags;
                    r.hasher.update_canonical(t.canonical);
                }
                continue;
            }
            if let Some(r) = run.take() {
                self.emit(r, out)?;
            }
            if tr.aggressor != AggressorSide::Unknown {
                let mut hasher = ContentHasher::new();
                hasher.update_canonical(t.canonical);
                run = Some(Run {
                    side: tr.aggressor,
                    first_seq: tr.prov.seq,
                    first: p,
                    last: p,
                    levels: 1,
                    size: tr.size.mantissa(),
                    count: 1,
                    price_scale: tr.price.scale(),
                    size_scale: tr.size.scale(),
                    prov: tr.prov.clone(),
                    hasher,
                });
            }
        }
        if let Some(r) = run {
            self.emit(r, out)?;
        }
        Ok(())
    }

    fn emit(&self, r: Run, out: &mut Vec<Sweep>) -> Result<(), InputError> {
        if r.levels < self.config.min_levels {
            return Ok(());
        }
        let mut prov = r.prov;
        prov.ts_publish_ns = None;
        out.push(Sweep {
            prov,
            derived: header(methods::SWEEP, r.hasher)?,
            side: r.side,
            first_seq: r.first_seq,
            first_price: price(r.first, r.price_scale)?,
            last_price: price(r.last, r.price_scale)?,
            levels: r.levels,
            size: size(r.size, r.size_scale)?,
            trade_count: r.count,
        });
        Ok(())
    }
}

/// Sweeps from trades in `seq` order (batch form).
///
/// # Errors
///
/// As [`SweepDetector::on_block`].
pub fn sweeps(trades: &[Trade], config: SweepConfig) -> Result<Vec<Sweep>, InputError> {
    let canon = encode_trades(trades)?;
    let mut d = SweepDetector::new(config)?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < trades.len() {
        let block = trades[i].prov.seq.block_number;
        let n = trades[i..].partition_point(|t| t.prov.seq.block_number == block);
        let ins: Vec<TradeIn<'_>> = trades[i..i + n]
            .iter()
            .zip(&canon[i..i + n])
            .map(|(trade, c)| TradeIn {
                trade,
                canonical: c,
            })
            .collect();
        d.on_block(&ins, &mut out)?;
        i += n;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use mesh_types::AggressorSide::*;

    #[test]
    fn detects_monotone_runs_within_a_block() {
        let t = vec![
            trade(1, 0, 0, "100.0", "1.00", Buy),
            trade(1, 1, 0, "100.0", "0.50", Buy),
            trade(1, 2, 0, "100.5", "1.00", Buy),
            trade(1, 3, 0, "101.0", "2.00", Buy),
            // Moves against the buyer: a new run (single level, ignored).
            trade(1, 4, 0, "100.5", "1.00", Buy),
            trade(1, 5, 0, "99.0", "1.00", Sell),
            trade(1, 6, 0, "98.5", "1.00", Sell),
            // Unknown ends the run.
            trade(1, 7, 0, "98.0", "1.00", Unknown),
            trade(1, 8, 0, "97.0", "1.00", Sell),
            // Next block never continues a run.
            trade(2, 0, 1_000, "96.0", "1.00", Sell),
        ];
        let s = sweeps(&t, SweepConfig::default()).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].side, Buy);
        assert_eq!(s[0].levels, 3);
        assert_eq!(s[0].trade_count, 4);
        assert_eq!(s[0].size, sz("4.50"));
        assert_eq!(
            (s[0].first_price, s[0].last_price),
            (px("100.0"), px("101.0"))
        );
        assert_eq!(s[0].first_seq, t[0].prov.seq);
        assert_eq!(s[0].prov.seq, t[3].prov.seq);
        assert_eq!(s[1].side, Sell);
        assert_eq!(s[1].levels, 2);
        let three = sweeps(&t, SweepConfig { min_levels: 3 }).unwrap();
        assert_eq!(three.len(), 1);
        assert!(SweepDetector::new(SweepConfig { min_levels: 1 }).is_err());
    }
}
