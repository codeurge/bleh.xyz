//! Liquidity grade per market (`liquidity-grade@1`).
//!
//! # Samples
//!
//! At every multiple of `sample_ms` of block time, the book at that
//! boundary (after the last block before it) gives one sample:
//!
//! - **spread** in hundredths of a basis point of the mid:
//!   `floor((ask - bid) * 2_000_000 / (ask + bid))`; no spread without both
//!   sides or with a non-positive `ask + bid`;
//! - **depth** per side: the notional `sum(price * size)` of levels within
//!   `depth_bps` of the mid (bids with `price * 20_000 >= (bid + ask) *
//!   (10_000 - depth_bps)`, asks with `price * 20_000 <= (bid + ask) *
//!   (10_000 + depth_bps)`), in quote units at the price scale, rounded
//!   down.
//!
//! Trades are counted per sample step.
//!
//! # Grade
//!
//! At every multiple of `emit_ms`, over the trailing window
//! `(T - window_ms, T]` (shortened to the time since the tracker started
//! when that is less):
//!
//! - spread = nearest-rank lower median of the samples' spreads (a sample
//!   without a spread ranks worst);
//! - depth per side = lower median of the samples' depths; the graded
//!   depth is the smaller side;
//! - trades per minute = `floor(trades * 100 * 60_000 / window)` in
//!   hundredths.
//!
//! The grade is the best of A, B, C whose thresholds all three metrics
//! meet (spread at most, depth and trades per minute at least), else D.
//! Grades at or better than `benchmark_min` are labelled `benchmark`,
//! others `tape`. Default thresholds are in [`LiquidityConfig::default`]
//! and `docs/operations/book.md`.
//!
//! The inputs hash covers the trades and book events since the previous
//! grade, in block-major order.

/// Record types, defined in `mesh-types`.
pub use mesh_types::{Grade, LiquidityGrade, LiquidityLabel};

use std::collections::VecDeque;

use mesh_types::{BookSide, ContentHasher, Provenance, UnixMillis};
use serde::{Deserialize, Serialize};

use crate::book::OrderBook;
use crate::error::InputError;
use crate::fixed::{size, spread_centi_bps};
use crate::inputs::header;
use crate::methods;

/// Thresholds a market must meet for one grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GradeThreshold {
    /// Maximum spread, hundredths of a basis point.
    pub max_spread_centi_bps: u64,
    /// Minimum depth on the thinner side, whole quote units.
    pub min_depth_quote: u64,
    /// Minimum trades per minute, hundredths.
    pub min_trades_per_minute_centi: u64,
}

/// Liquidity grade settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidityConfig {
    /// Sample step, ms.
    pub sample_ms: i64,
    /// Emission step, ms (a multiple of `sample_ms`).
    pub emit_ms: i64,
    /// Trailing window, ms.
    pub window_ms: i64,
    /// Depth band around the mid, basis points.
    pub depth_bps: u32,
    /// Thresholds for A, B and C.
    pub thresholds: [GradeThreshold; 3],
    /// The worst grade still labelled `benchmark`.
    pub benchmark_min: Grade,
}

impl Default for LiquidityConfig {
    /// Samples every minute, a grade every five minutes over one hour,
    /// depth within 25 bps, and:
    ///
    /// | Grade | Spread at most | Depth at least | Trades/min at least |
    /// |---|---|---|---|
    /// | A | 5 bps | 500,000 | 20 |
    /// | B | 20 bps | 50,000 | 2 |
    /// | C | 100 bps | 5,000 | 0.2 |
    ///
    /// A and B are `benchmark`; C and D are `tape`.
    fn default() -> Self {
        Self {
            sample_ms: 60_000,
            emit_ms: 300_000,
            window_ms: 3_600_000,
            depth_bps: 25,
            thresholds: [
                GradeThreshold {
                    max_spread_centi_bps: 500,
                    min_depth_quote: 500_000,
                    min_trades_per_minute_centi: 2_000,
                },
                GradeThreshold {
                    max_spread_centi_bps: 2_000,
                    min_depth_quote: 50_000,
                    min_trades_per_minute_centi: 200,
                },
                GradeThreshold {
                    max_spread_centi_bps: 10_000,
                    min_depth_quote: 5_000,
                    min_trades_per_minute_centi: 20,
                },
            ],
            benchmark_min: Grade::B,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    at: i64,
    spread: Option<u64>,
    bid: i128,
    ask: i128,
}

/// Streams liquidity grades.
#[derive(Debug, Clone)]
pub struct LiquidityTracker {
    config: LiquidityConfig,
    samples: VecDeque<Sample>,
    trades: VecDeque<(i64, u64)>,
    cur_step: Option<i64>,
    started: Option<i64>,
    price_scale: Option<u8>,
    hasher: ContentHasher,
    last_prov: Option<Provenance>,
}

impl LiquidityTracker {
    /// A tracker for `config`.
    ///
    /// # Errors
    ///
    /// Fails on non-positive steps, an emission step that is not a multiple
    /// of the sample step, or thresholds out of order.
    pub fn new(config: LiquidityConfig) -> Result<Self, InputError> {
        let t = &config.thresholds;
        let ordered = t.windows(2).all(|w| {
            w[0].max_spread_centi_bps <= w[1].max_spread_centi_bps
                && w[0].min_depth_quote >= w[1].min_depth_quote
                && w[0].min_trades_per_minute_centi >= w[1].min_trades_per_minute_centi
        });
        if config.sample_ms <= 0
            || config.emit_ms <= 0
            || config.window_ms <= 0
            || config.emit_ms % config.sample_ms != 0
            || config.depth_bps >= 10_000
            || !ordered
        {
            return Err(InputError::InvalidConfig(format!(
                "invalid liquidity settings: {config:?}"
            )));
        }
        Ok(Self {
            config,
            samples: VecDeque::new(),
            trades: VecDeque::new(),
            cur_step: None,
            started: None,
            price_scale: None,
            hasher: ContentHasher::new(),
            last_prov: None,
        })
    }

    /// Before a block at `block_time`: samples the boundaries before it
    /// from the current book (`None` while the book is not usable) and
    /// emits grades that are due.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn before_block(
        &mut self,
        book: Option<&OrderBook>,
        block_time: UnixMillis,
        out: &mut Vec<LiquidityGrade>,
    ) -> Result<(), InputError> {
        self.advance(book, block_time, out)
    }

    /// Counts a trade of the current block and adds its canonical encoding.
    pub fn on_trade(&mut self, ts: UnixMillis, canonical: &[u8]) {
        let step = ts.0.div_euclid(self.config.sample_ms);
        match self.trades.back_mut() {
            Some((s, n)) if *s == step => *n += 1,
            _ => self.trades.push_back((step, 1)),
        }
        self.hasher.update_canonical(canonical);
    }

    /// Adds a book event's (or snapshot's) canonical encoding.
    pub fn on_input(&mut self, canonical: &[u8]) {
        self.hasher.update_canonical(canonical);
    }

    /// After a block. `prov` is its end-of-block provenance.
    pub fn after_block(&mut self, prov: &Provenance) {
        self.last_prov = Some(prov.clone());
    }

    /// Time reached `ts` with no block.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(
        &mut self,
        book: Option<&OrderBook>,
        ts: UnixMillis,
        out: &mut Vec<LiquidityGrade>,
    ) -> Result<(), InputError> {
        if self.cur_step.is_some() {
            self.advance(book, ts, out)?;
        }
        Ok(())
    }

    fn advance(
        &mut self,
        book: Option<&OrderBook>,
        t: UnixMillis,
        out: &mut Vec<LiquidityGrade>,
    ) -> Result<(), InputError> {
        let step = t.0.div_euclid(self.config.sample_ms);
        let Some(cur) = self.cur_step else {
            self.cur_step = Some(step);
            self.started = Some(t.0);
            return Ok(());
        };
        if step <= cur {
            return Ok(());
        }
        let sample = match book {
            Some(b) => self.sample(b)?,
            None => None,
        };
        // Boundaries cur+1 ..= step. After a long gap only the last
        // window's worth of boundaries can change a grade's samples, but
        // every emission boundary is still graded.
        let mut b = cur + 1;
        while b <= step {
            let at = b.saturating_mul(self.config.sample_ms);
            if let Some(mut s) = sample {
                s.at = at;
                self.samples.push_back(s);
            }
            if at.rem_euclid(self.config.emit_ms) == 0 {
                self.emit(at, out)?;
            }
            b += 1;
        }
        self.cur_step = Some(step);
        Ok(())
    }

    fn sample(&mut self, book: &OrderBook) -> Result<Option<Sample>, InputError> {
        let (Some(ps), Some(ss)) = (book.price_scale(), book.size_scale()) else {
            return Ok(None);
        };
        self.price_scale.get_or_insert(ps);
        let top = book.top();
        let (Some(bid), Some(ask)) = (top.bid, top.ask) else {
            return Ok(Some(Sample {
                at: 0,
                spread: None,
                bid: 0,
                ask: 0,
            }));
        };
        let (b, a) = (bid.price.mantissa(), ask.price.mantissa());
        let spread = spread_centi_bps(b, a);
        let sum = i128::from(b) + i128::from(a);
        let bps = i128::from(self.config.depth_bps);
        let down = 10i128.pow(u32::from(ss));
        let depth = |side: BookSide| -> i128 {
            let mut total: i128 = 0;
            for l in book.level_refs(side) {
                let p = i128::from(l.price_mantissa());
                let inside = match side {
                    BookSide::Bid => p * 20_000 >= sum * (10_000 - bps),
                    BookSide::Ask => p * 20_000 <= sum * (10_000 + bps),
                };
                if !inside {
                    break;
                }
                total = total.saturating_add(p * i128::from(l.size_mantissa()));
            }
            // Price scale + size scale digits down to the price scale.
            total / down
        };
        Ok(Some(Sample {
            at: 0,
            spread,
            bid: depth(BookSide::Bid),
            ask: depth(BookSide::Ask),
        }))
    }

    fn emit(&mut self, at: i64, out: &mut Vec<LiquidityGrade>) -> Result<(), InputError> {
        let Some(prov) = self.last_prov.clone() else {
            return Ok(());
        };
        let start = self.started.unwrap_or(at);
        let window = self.config.window_ms.min(at - start);
        if window <= 0 {
            return Ok(());
        }
        let from = at - window;
        while self.samples.front().is_some_and(|s| s.at <= from) {
            self.samples.pop_front();
        }
        let first_step = from.div_euclid(self.config.sample_ms);
        let end_step = at.div_euclid(self.config.sample_ms);
        while self.trades.front().is_some_and(|(s, _)| *s < first_step) {
            self.trades.pop_front();
        }
        let trades: u64 = self
            .trades
            .iter()
            .filter(|(s, _)| *s < end_step)
            .map(|(_, n)| *n)
            .sum();
        let samples: Vec<Sample> = self
            .samples
            .iter()
            .copied()
            .filter(|s| s.at <= at)
            .collect();
        let n = samples.len();
        let lower_median = |mut v: Vec<i128>| -> i128 {
            v.sort_unstable();
            v.get(n.saturating_sub(1) / 2).copied().unwrap_or(0)
        };
        let spread = {
            let mut v: Vec<Option<u64>> = samples.iter().map(|s| s.spread).collect();
            v.sort_unstable_by_key(|s| s.unwrap_or(u64::MAX));
            v.get(n.saturating_sub(1) / 2).copied().flatten()
        };
        let bid = lower_median(samples.iter().map(|s| s.bid).collect());
        let ask = lower_median(samples.iter().map(|s| s.ask).collect());
        let tpm = u128::from(trades) * 100 * 60_000 / u128::try_from(window).unwrap_or(1);
        let tpm = u64::try_from(tpm).unwrap_or(u64::MAX);
        let ps = self.price_scale.unwrap_or(0);
        let whole = 10i128.pow(u32::from(ps));
        let depth_quote = u64::try_from(bid.min(ask) / whole).unwrap_or(0);
        let grade = match spread {
            None => Grade::D,
            Some(sp) => {
                let mut g = Grade::D;
                for (grade, th) in [Grade::A, Grade::B, Grade::C]
                    .into_iter()
                    .zip(&self.config.thresholds)
                {
                    if sp <= th.max_spread_centi_bps
                        && depth_quote >= th.min_depth_quote
                        && tpm >= th.min_trades_per_minute_centi
                    {
                        g = grade;
                        break;
                    }
                }
                g
            }
        };
        let to_i64 = |v: i128| i64::try_from(v).map_err(|_| InputError::Overflow);
        let hasher = std::mem::take(&mut self.hasher);
        out.push(LiquidityGrade {
            prov,
            derived: header(methods::LIQUIDITY_GRADE, hasher)?,
            as_of_ms: UnixMillis(at),
            window_ms: window,
            samples: u32::try_from(n).unwrap_or(u32::MAX),
            spread_bps: match spread {
                Some(s) => Some(size(
                    i64::try_from(s).map_err(|_| InputError::Overflow)?,
                    2,
                )?),
                None => None,
            },
            depth_bps: self.config.depth_bps,
            depth_bid: size(to_i64(bid)?, ps)?,
            depth_ask: size(to_i64(ask)?, ps)?,
            trades,
            trades_per_minute: size(i64::try_from(tpm).map_err(|_| InputError::Overflow)?, 2)?,
            grade,
            label: if grade <= self.config.benchmark_min {
                LiquidityLabel::Benchmark
            } else {
                LiquidityLabel::Tape
            },
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use mesh_types::BookSide::*;

    fn deep_book() -> OrderBook {
        let mut b = OrderBook::new(inst());
        // Mid 100.0, spread 0.2 = 20 bps; 25 bps band = 99.75 .. 100.25.
        let evs = [
            add(1, 0, 1, Bid, "99.9", "2000.00"),
            add(1, 1, 2, Bid, "99.7", "9000.00"),
            add(1, 2, 3, Ask, "100.1", "3000.00"),
            add(1, 3, 4, Ask, "100.3", "9000.00"),
        ];
        b.apply_block(1, UnixMillis(block_ms(1)), &evs).unwrap();
        b
    }

    #[test]
    fn grades_from_samples_and_trades() {
        let book = deep_book();
        let cfg = LiquidityConfig {
            emit_ms: 300_000,
            ..LiquidityConfig::default()
        };
        let mut l = LiquidityTracker::new(cfg).unwrap();
        let mut out = Vec::new();
        let t0 = UnixMillis(BASE_MS);
        l.before_block(Some(&book), t0, &mut out).unwrap();
        l.after_block(&prov_eob(1, 0));
        // 10 trades per minute for five minutes.
        for m in 0..5 {
            for k in 0..10 {
                l.on_trade(UnixMillis(BASE_MS + m * 60_000 + k * 1_000), b"x");
            }
        }
        l.advance_to(Some(&book), UnixMillis(BASE_MS + 300_000), &mut out)
            .unwrap();
        assert_eq!(out.len(), 1);
        let g = &out[0];
        assert_eq!(g.as_of_ms, UnixMillis(BASE_MS + 300_000));
        assert_eq!(g.window_ms, 300_000);
        assert_eq!(g.samples, 5);
        assert_eq!(g.spread_bps, Some(sz("20.00")));
        // Bid depth: 99.9 * 2000 = 199,800 (99.7 is outside the band).
        assert_eq!(g.depth_bid, sz("199800.0"));
        assert_eq!(g.depth_ask, sz("300300.0"));
        assert_eq!(g.trades, 50);
        assert_eq!(g.trades_per_minute, sz("10.00"));
        // Spread 20 bps fails A (5) and passes B (20); depth 199,800 >= 50k;
        // 10/min >= 2.
        assert_eq!(g.grade, Grade::B);
        assert_eq!(g.label, LiquidityLabel::Benchmark);
        assert_eq!(g.derived.input_count, 50);
    }

    #[test]
    fn one_sided_or_quiet_is_tape() {
        let mut b = OrderBook::new(inst());
        b.apply(&add(1, 0, 1, Bid, "99.9", "1.00")).unwrap();
        let mut l = LiquidityTracker::new(LiquidityConfig::default()).unwrap();
        let mut out = Vec::new();
        l.before_block(Some(&b), UnixMillis(BASE_MS), &mut out)
            .unwrap();
        l.after_block(&prov_eob(1, 0));
        l.advance_to(Some(&b), UnixMillis(BASE_MS + 600_000), &mut out)
            .unwrap();
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .all(|g| g.grade == Grade::D && g.label == LiquidityLabel::Tape)
        );
        assert_eq!(out[0].spread_bps, None);
    }

    #[test]
    fn config_checks() {
        let c = LiquidityConfig {
            emit_ms: 90_000,
            ..LiquidityConfig::default()
        };
        assert!(LiquidityTracker::new(c).is_err());
        let mut c = LiquidityConfig::default();
        c.thresholds.swap(0, 2);
        assert!(LiquidityTracker::new(c).is_err());
    }
}
