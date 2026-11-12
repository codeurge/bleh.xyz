//! Rollups: longer time bars from shorter ones.
//!
//! A rolled-up bar takes open from the first source bar, high and low as
//! the extremes, close from the last, volumes and trade counts as sums,
//! provenance from the last source bar and flags as the union. Its
//! derived header is the same as a bar computed directly from the trades
//! (`bars@1`, inputs hash over the trades of the interval), so a rollup is
//! equal to the direct computation; the property tests check this.
//!
//! [`rollup`] is the batch form (the archive has the trades for the
//! header). [`BarEngine`] streams direct bars and rollups together: its
//! rollups take OHLCV from the 1m bars as they close and hash the trades as
//! they arrive.

use std::collections::BTreeMap;

use mesh_types::{Bar, BarInterval, ContentHasher, Trade, UnixMillis};
use serde::{Deserialize, Serialize};

use super::Ohlcv;
use super::time::TimeBarBuilder;
use crate::error::InputError;
use crate::inputs::{TradeIn, encode_trades, header};
use crate::methods;

fn check_rollup(source: BarInterval, target: BarInterval) -> Result<(), InputError> {
    if target.millis() <= source.millis() || target.millis() % source.millis() != 0 {
        return Err(InputError::InvalidConfig(format!(
            "cannot roll {source} bars up into {target}"
        )));
    }
    Ok(())
}

/// Rolls bars of one interval up into `target` (batch form). `trades` are
/// the trades the source bars were built from, in `seq` order; they give
/// the inputs hash.
///
/// # Errors
///
/// Fails if `target` is not a longer multiple of the source interval, the
/// bars are mixed or out of order, or the trades do not match the bars.
pub fn rollup(bars: &[Bar], target: BarInterval, trades: &[Trade]) -> Result<Vec<Bar>, InputError> {
    let Some(first) = bars.first() else {
        return Ok(Vec::new());
    };
    let source = first.interval;
    check_rollup(source, target)?;
    let mut groups: Vec<(UnixMillis, Ohlcv)> = Vec::new();
    for b in bars {
        if b.interval != source {
            return Err(InputError::Inconsistent("bars of mixed intervals".into()));
        }
        let start = target.align(b.start_ms);
        match groups.last_mut() {
            Some((s, acc)) if *s == start => acc.merge(&Ohlcv::from_bar(b))?,
            Some((s, _)) if *s > start => {
                return Err(InputError::Inconsistent("bars out of order".into()));
            }
            _ => groups.push((start, Ohlcv::from_bar(b))),
        }
    }
    let canon = encode_trades(trades)?;
    let mut hashers: BTreeMap<UnixMillis, ContentHasher> = BTreeMap::new();
    for (t, c) in trades.iter().zip(&canon) {
        hashers
            .entry(target.align(t.prov.ts_source_ms))
            .or_default()
            .update_canonical(c);
    }
    let mut out = Vec::with_capacity(groups.len());
    for (start, acc) in groups {
        let hasher = hashers.remove(&start).unwrap_or_default();
        if hasher.count() != u64::from(acc.count) {
            return Err(InputError::Inconsistent(format!(
                "{} trades in the {target} interval at {}, bars count {}",
                hasher.count(),
                start.0,
                acc.count
            )));
        }
        out.push(acc.to_bar(target, start, header(methods::BARS, hasher)?)?);
    }
    if !hashers.is_empty() {
        return Err(InputError::Inconsistent("trades outside every bar".into()));
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct OpenRollup {
    start: UnixMillis,
    acc: Option<Ohlcv>,
    hasher: ContentHasher,
}

/// Streams one rollup interval: OHLCV from closing 1m bars, inputs hash
/// from the trades.
#[derive(Debug, Clone)]
struct RollupBuilder {
    target: BarInterval,
    open: Option<OpenRollup>,
}

impl RollupBuilder {
    fn on_trade(&mut self, t: TradeIn<'_>, out: &mut Vec<Bar>) -> Result<(), InputError> {
        let start = self.target.align(t.trade.prov.ts_source_ms);
        if self.open.as_ref().is_none_or(|o| o.start != start) {
            self.close(out)?;
            self.open = Some(OpenRollup {
                start,
                acc: None,
                hasher: ContentHasher::new(),
            });
        }
        if let Some(o) = &mut self.open {
            o.hasher.update_canonical(t.canonical);
        }
        Ok(())
    }

    fn on_bar(&mut self, bar: &Bar) -> Result<(), InputError> {
        let start = self.target.align(bar.start_ms);
        let Some(o) = self.open.as_mut().filter(|o| o.start == start) else {
            return Err(InputError::Inconsistent(format!(
                "1m bar at {} outside the open {} interval",
                bar.start_ms.0, self.target
            )));
        };
        match &mut o.acc {
            Some(acc) => acc.merge(&Ohlcv::from_bar(bar)),
            None => {
                o.acc = Some(Ohlcv::from_bar(bar));
                Ok(())
            }
        }
    }

    fn advance_to(&mut self, ts: UnixMillis, out: &mut Vec<Bar>) -> Result<(), InputError> {
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.start.0.saturating_add(self.target.millis()) <= ts.0)
        {
            self.close(out)?;
        }
        Ok(())
    }

    fn close(&mut self, out: &mut Vec<Bar>) -> Result<(), InputError> {
        let Some(o) = self.open.take() else {
            return Ok(());
        };
        let Some(acc) = o.acc else {
            return Err(InputError::Inconsistent(format!(
                "{} interval at {} closed before its 1m bars",
                self.target, o.start.0
            )));
        };
        if o.hasher.count() != u64::from(acc.count) {
            return Err(InputError::Inconsistent(format!(
                "{} interval at {} hashed {} trades, 1m bars count {}",
                self.target,
                o.start.0,
                o.hasher.count(),
                acc.count
            )));
        }
        out.push(acc.to_bar(self.target, o.start, header(methods::BARS, o.hasher)?)?);
        Ok(())
    }
}

/// Which time bars a [`BarEngine`] builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarEngineConfig {
    /// Intervals built directly from trades.
    pub direct: Vec<BarInterval>,
    /// Intervals rolled up from 1m bars (`1m` must be in `direct`).
    pub rollups: Vec<BarInterval>,
}

impl Default for BarEngineConfig {
    /// 1s and 1m direct; 5m, 15m, 1h and 1d rolled up.
    fn default() -> Self {
        Self {
            direct: vec![BarInterval::S1, BarInterval::M1],
            rollups: vec![
                BarInterval::M5,
                BarInterval::M15,
                BarInterval::H1,
                BarInterval::D1,
            ],
        }
    }
}

/// Streams direct time bars and rollups.
#[derive(Debug, Clone)]
pub struct BarEngine {
    direct: Vec<TimeBarBuilder>,
    m1: Option<usize>,
    rollups: Vec<RollupBuilder>,
    scratch: Vec<Bar>,
}

impl BarEngine {
    /// An engine for `config`.
    ///
    /// # Errors
    ///
    /// Fails if rollups are requested without 1m bars, or a rollup
    /// interval is not a longer multiple of 1m, or an interval repeats.
    pub fn new(config: &BarEngineConfig) -> Result<Self, InputError> {
        let mut seen = std::collections::BTreeSet::new();
        for i in config.direct.iter().chain(&config.rollups) {
            if !seen.insert(*i) {
                return Err(InputError::InvalidConfig(format!("interval {i} repeats")));
            }
        }
        let m1 = config.direct.iter().position(|i| *i == BarInterval::M1);
        if !config.rollups.is_empty() && m1.is_none() {
            return Err(InputError::InvalidConfig(
                "rollups need 1m bars in direct".into(),
            ));
        }
        for r in &config.rollups {
            check_rollup(BarInterval::M1, *r)?;
        }
        Ok(Self {
            direct: config
                .direct
                .iter()
                .map(|i| TimeBarBuilder::new(*i))
                .collect(),
            m1,
            rollups: config
                .rollups
                .iter()
                .map(|i| RollupBuilder {
                    target: *i,
                    open: None,
                })
                .collect(),
            scratch: Vec::new(),
        })
    }

    /// Adds the next trade; completed bars are appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails on out-of-order input, mixed scales or overflow.
    pub fn push(&mut self, t: TradeIn<'_>, out: &mut Vec<Bar>) -> Result<(), InputError> {
        for (i, b) in self.direct.iter_mut().enumerate() {
            if Some(i) == self.m1 {
                self.scratch.clear();
                b.push(t, &mut self.scratch)?;
                for bar in &self.scratch {
                    for r in &mut self.rollups {
                        r.on_bar(bar)?;
                    }
                }
                out.append(&mut self.scratch);
            } else {
                b.push(t, out)?;
            }
        }
        for r in &mut self.rollups {
            r.on_trade(t, out)?;
        }
        Ok(())
    }

    /// Emits bars that end at or before `ts`.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn advance_to(&mut self, ts: UnixMillis, out: &mut Vec<Bar>) -> Result<(), InputError> {
        self.drive(out, |b, o| b.advance_to(ts, o))?;
        for r in &mut self.rollups {
            r.advance_to(ts, out)?;
        }
        Ok(())
    }

    /// Emits every open bar.
    ///
    /// # Errors
    ///
    /// Fails if a value cannot be represented.
    pub fn finish(&mut self, out: &mut Vec<Bar>) -> Result<(), InputError> {
        self.drive(out, TimeBarBuilder::finish)?;
        for r in &mut self.rollups {
            r.close(out)?;
        }
        Ok(())
    }

    fn drive(
        &mut self,
        out: &mut Vec<Bar>,
        mut f: impl FnMut(&mut TimeBarBuilder, &mut Vec<Bar>) -> Result<(), InputError>,
    ) -> Result<(), InputError> {
        for (i, b) in self.direct.iter_mut().enumerate() {
            if Some(i) == self.m1 {
                self.scratch.clear();
                f(b, &mut self.scratch)?;
                for bar in &self.scratch {
                    for r in &mut self.rollups {
                        r.on_bar(bar)?;
                    }
                }
                out.append(&mut self.scratch);
            } else {
                f(b, out)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bars::time_bars;
    use crate::inputs::canonical_trade;
    use crate::testutil::*;
    use mesh_types::AggressorSide::*;

    fn tape() -> Vec<Trade> {
        let mut v = Vec::new();
        // Trades spread over two UTC days, with gaps.
        let offsets = [
            0, 30_000, 61_000, 299_999, 300_000, 901_000, 3_599_999, 3_600_000, 86_399_999,
            86_400_000, 86_460_000,
        ];
        for (i, off) in offsets.iter().enumerate() {
            let i = u32::try_from(i).unwrap();
            let p = format!("{}.0", 100 + i % 7);
            v.push(trade(
                u64::from(i) + 1,
                0,
                *off,
                &p,
                "1.00",
                if i % 2 == 0 { Buy } else { Sell },
            ));
        }
        v
    }

    #[test]
    fn rollup_equals_direct() {
        let trades = tape();
        let m1 = time_bars(&trades, BarInterval::M1).unwrap();
        for target in [
            BarInterval::M5,
            BarInterval::M15,
            BarInterval::H1,
            BarInterval::D1,
        ] {
            let direct = time_bars(&trades, target).unwrap();
            let rolled = rollup(&m1, target, &trades).unwrap();
            assert_eq!(rolled, direct, "{target}");
            for b in &rolled {
                b.validate().unwrap();
            }
        }
        assert_eq!(time_bars(&trades, BarInterval::D1).unwrap().len(), 2);
    }

    #[test]
    fn rollup_rejects_mismatches() {
        let trades = tape();
        let m1 = time_bars(&trades, BarInterval::M1).unwrap();
        assert!(rollup(&m1, BarInterval::S1, &trades).is_err());
        assert!(rollup(&m1, BarInterval::M5, &trades[1..]).is_err());
        let m5 = time_bars(&trades, BarInterval::M5).unwrap();
        let mut mixed = m1.clone();
        mixed.push(m5[0].clone());
        assert!(rollup(&mixed, BarInterval::H1, &trades).is_err());
        assert!(rollup(&[], BarInterval::H1, &trades).unwrap().is_empty());
    }

    #[test]
    fn engine_matches_batch() {
        let trades = tape();
        let mut engine = BarEngine::new(&BarEngineConfig::default()).unwrap();
        let mut out = Vec::new();
        for t in &trades {
            let c = canonical_trade(t).unwrap();
            engine
                .push(
                    TradeIn {
                        trade: t,
                        canonical: &c,
                    },
                    &mut out,
                )
                .unwrap();
        }
        engine.finish(&mut out).unwrap();
        for i in BarInterval::ALL {
            let mut got: Vec<Bar> = out.iter().filter(|b| b.interval == i).cloned().collect();
            got.sort_by_key(|b| b.start_ms);
            assert_eq!(got, time_bars(&trades, i).unwrap(), "{i}");
        }
    }

    #[test]
    fn engine_config_checks() {
        assert!(
            BarEngine::new(&BarEngineConfig {
                direct: vec![BarInterval::S1],
                rollups: vec![BarInterval::M5],
            })
            .is_err()
        );
        assert!(
            BarEngine::new(&BarEngineConfig {
                direct: vec![BarInterval::M1, BarInterval::M1],
                rollups: vec![],
            })
            .is_err()
        );
    }
}
