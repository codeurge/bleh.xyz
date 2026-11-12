//! Aligns the user's feed with bleh.xyz prices on a fixed time grid.
//!
//! Per pair, each side keeps its recent observations. A grid time `t` (a
//! multiple of the interval) is sampled once both sides have moved past it
//! (an observation or a heartbeat later than `t`), using each side's last
//! price at or before `t` (an as-of join). A side whose last price is older
//! than `max_stale` makes the interval stale: it is skipped and counted.
//! Observations older than an interval already sampled are late: dropped
//! and counted. Grid times run in event time (the feeds' own timestamps) or,
//! with the receive clock, in local arrival time.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use mesh_types::InstrumentId;

use super::bleh::{BlehObs, ObsSource};
use super::boundary::UserSupplied;
use super::feed::UserQuote;
use super::mapping::{BlehPrice, BlehTarget, Mapping, Pair, UserPrice};
use super::stats::InputCounts;

/// Most observations kept per side per pair; older ones are dropped.
pub const MAX_HISTORY: usize = 200_000;

/// The user's side of a sample.
#[derive(Debug, Clone, PartialEq)]
pub struct UserPoint {
    /// Symbol in the user's feed.
    pub symbol: Arc<str>,
    /// Price as the feed gave it.
    pub price: f64,
    /// Price after the pair's multiplier (what is compared).
    pub adjusted: f64,
    /// Time of the observation, Unix nanoseconds.
    pub ts_ns: i64,
}

/// The bleh.xyz side of a sample (CC0 data).
#[derive(Debug, Clone, PartialEq)]
pub struct BlehPoint {
    /// Series label (instrument ID).
    pub label: Arc<str>,
    /// Price.
    pub price: f64,
    /// Time of the observation, Unix nanoseconds.
    pub ts_ns: i64,
}

/// One aligned sample. It contains user-supplied data, so it has no serde
/// implementation; only this module's local sinks render it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// Index of the pair in the mapping.
    pub pair: usize,
    /// Pair name.
    pub pair_name: Arc<str>,
    /// Grid time, Unix nanoseconds.
    pub ts_ns: i64,
    /// The user's side.
    pub user: UserSupplied<UserPoint>,
    /// The bleh.xyz side.
    pub bleh: BlehPoint,
    /// The pair's multiplier.
    pub multiplier: f64,
}

impl Sample {
    /// `bleh - user x multiplier`.
    pub fn basis(&self) -> f64 {
        self.bleh.price - self.user.get().adjusted
    }
}

/// Alignment settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignConfig {
    /// Grid interval, nanoseconds.
    pub interval_ns: i64,
    /// A side's price older than this at a grid time makes it stale.
    pub max_stale_ns: i64,
}

#[derive(Debug, Default)]
struct Side {
    hist: VecDeque<(i64, f64)>,
    last_ts: Option<i64>,
    clock: Option<i64>,
}

impl Side {
    fn watermark(&self) -> Option<i64> {
        match (self.last_ts, self.clock) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// Inserts in time order; returns false if the history was full and
    /// the oldest point was dropped.
    fn insert(&mut self, ts: i64, px: f64) -> bool {
        match self.hist.back() {
            Some(&(t, _)) if ts < t => {
                let i = self.hist.partition_point(|p| p.0 <= ts);
                self.hist.insert(i, (ts, px));
            }
            _ => self.hist.push_back((ts, px)),
        }
        self.last_ts = Some(self.last_ts.map_or(ts, |l| l.max(ts)));
        if self.hist.len() > MAX_HISTORY {
            self.hist.pop_front();
            return false;
        }
        true
    }

    /// Last point at or before `t`.
    fn as_of(&self, t: i64) -> Option<(i64, f64)> {
        let i = self.hist.partition_point(|p| p.0 <= t);
        i.checked_sub(1).and_then(|i| self.hist.get(i).copied())
    }

    /// First point after `t`.
    fn after(&self, t: i64) -> Option<i64> {
        let i = self.hist.partition_point(|p| p.0 <= t);
        self.hist.get(i).map(|p| p.0)
    }

    /// Drops points that can no longer answer an as-of query at or after
    /// `t`.
    fn prune(&mut self, t: i64) {
        let i = self.hist.partition_point(|p| p.0 <= t);
        for _ in 1..i {
            self.hist.pop_front();
        }
    }
}

#[derive(Debug)]
struct PairState {
    name: Arc<str>,
    symbol: Arc<str>,
    label: Arc<str>,
    user_price: UserPrice,
    multiplier: f64,
    user: Side,
    bleh: Side,
    next_tick: Option<i64>,
    last_tick: Option<i64>,
    counts: InputCounts,
}

/// The aligner for every active pair of a mapping.
#[derive(Debug)]
pub struct Aligner {
    cfg: AlignConfig,
    pairs: Vec<PairState>,
    by_symbol: HashMap<String, Vec<usize>>,
    by_series: HashMap<(InstrumentId, ObsSource), Vec<usize>>,
}

fn align_up(t: i64, step: i64) -> i64 {
    let r = t.rem_euclid(step);
    if r == 0 { t } else { t - r + step }
}

impl Aligner {
    /// An aligner for the active pairs of `mapping`. Indices in samples are
    /// positions in `mapping.pairs`; inactive pairs never produce samples.
    pub fn new(mapping: &Mapping, cfg: AlignConfig) -> Self {
        let cfg = AlignConfig {
            interval_ns: cfg.interval_ns.max(1),
            max_stale_ns: cfg.max_stale_ns.max(0),
        };
        let mut by_symbol: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_series: HashMap<(InstrumentId, ObsSource), Vec<usize>> = HashMap::new();
        let pairs = mapping
            .pairs
            .iter()
            .enumerate()
            .map(|(i, p): (usize, &Pair)| {
                if let BlehTarget::Market { instrument, price } = &p.bleh {
                    let src = match price {
                        BlehPrice::Trade => ObsSource::Trade,
                        BlehPrice::Mid => ObsSource::Mid,
                        BlehPrice::BarClose(iv) => ObsSource::Bar(*iv),
                    };
                    by_series
                        .entry((instrument.clone(), src))
                        .or_default()
                        .push(i);
                    by_symbol.entry(p.user_symbol.clone()).or_default().push(i);
                }
                PairState {
                    name: p.name.as_str().into(),
                    symbol: p.user_symbol.as_str().into(),
                    label: p.bleh.label().into(),
                    user_price: p.user_price,
                    multiplier: p.multiplier,
                    user: Side::default(),
                    bleh: Side::default(),
                    next_tick: None,
                    last_tick: None,
                    counts: InputCounts::default(),
                }
            })
            .collect();
        Self {
            cfg,
            pairs,
            by_symbol,
            by_series,
        }
    }

    /// Input counters of a pair.
    pub fn counts(&self, pair: usize) -> InputCounts {
        self.pairs.get(pair).map(|p| p.counts).unwrap_or_default()
    }

    /// True if the user's feed symbol belongs to an active pair.
    pub fn wants_symbol(&self, symbol: &str) -> bool {
        self.by_symbol.contains_key(symbol)
    }

    /// Adds a user observation; returns the samples it completed.
    pub fn push_user(&mut self, q: &UserSupplied<UserQuote>) -> Vec<Sample> {
        let q = q.get();
        let Some(ix) = self.by_symbol.get(&q.symbol).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in ix {
            let Some(p) = self.pairs.get_mut(i) else {
                continue;
            };
            let px = match p.user_price {
                UserPrice::Any => q.price_any(),
                UserPrice::Trade => q.price,
                UserPrice::Mid => q.mid(),
            };
            let Some(px) = px else { continue };
            p.counts.user += 1;
            if p.last_tick.is_some_and(|t| q.ts_ns <= t) {
                p.counts.late += 1;
                continue;
            }
            if !p.user.insert(q.ts_ns, px) {
                p.counts.dropped += 1;
            }
            self.sample(i, false, &mut out);
        }
        out
    }

    /// Adds a bleh.xyz observation; returns the samples it completed.
    pub fn push_bleh(&mut self, o: &BlehObs) -> Vec<Sample> {
        let Some(ix) = self
            .by_series
            .get(&(o.instrument.clone(), o.source))
            .cloned()
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in ix {
            let Some(p) = self.pairs.get_mut(i) else {
                continue;
            };
            p.counts.bleh += 1;
            if p.last_tick.is_some_and(|t| o.ts_ns <= t) {
                p.counts.late += 1;
                continue;
            }
            if !p.bleh.insert(o.ts_ns, o.price) {
                p.counts.dropped += 1;
            }
            self.sample(i, false, &mut out);
        }
        out
    }

    /// Advances the bleh.xyz side's clock (a block heartbeat): everything
    /// up to `ts_ns` was delivered.
    pub fn bleh_clock(&mut self, ts_ns: i64) -> Vec<Sample> {
        self.clock(ts_ns, false, true)
    }

    /// Advances both sides' clocks (receive-clock timer).
    pub fn both_clocks(&mut self, ts_ns: i64) -> Vec<Sample> {
        self.clock(ts_ns, true, true)
    }

    fn clock(&mut self, ts_ns: i64, user: bool, bleh: bool) -> Vec<Sample> {
        let mut out = Vec::new();
        for i in 0..self.pairs.len() {
            if let Some(p) = self.pairs.get_mut(i) {
                if user {
                    p.user.clock = Some(p.user.clock.map_or(ts_ns, |c| c.max(ts_ns)));
                }
                if bleh {
                    p.bleh.clock = Some(p.bleh.clock.map_or(ts_ns, |c| c.max(ts_ns)));
                }
            }
            self.sample(i, false, &mut out);
        }
        out
    }

    /// At the end of input: samples every grid time both sides reached.
    pub fn flush(&mut self) -> Vec<Sample> {
        let mut out = Vec::new();
        for i in 0..self.pairs.len() {
            self.sample(i, true, &mut out);
        }
        out
    }

    /// Samples pair `i` up to its watermark (inclusive when `inclusive`).
    fn sample(&mut self, i: usize, inclusive: bool, out: &mut Vec<Sample>) {
        let cfg = self.cfg;
        let Some(p) = self.pairs.get_mut(i) else {
            return;
        };
        let (Some(wu), Some(wb)) = (p.user.watermark(), p.bleh.watermark()) else {
            return;
        };
        let wm = wu.min(wb);
        if p.next_tick.is_none() {
            let (Some(u0), Some(b0)) = (p.user.hist.front(), p.bleh.hist.front()) else {
                return;
            };
            p.next_tick = Some(align_up(u0.0.max(b0.0), cfg.interval_ns));
        }
        while let Some(t) = p.next_tick {
            if t > wm || (t == wm && !inclusive) {
                break;
            }
            let u = p.user.as_of(t);
            let b = p.bleh.as_of(t);
            let fresh = |x: Option<(i64, f64)>| x.filter(|(ts, _)| t - ts <= cfg.max_stale_ns);
            let mut next = t + cfg.interval_ns;
            match (fresh(u), fresh(b)) {
                (Some((uts, upx)), Some((bts, bpx))) => out.push(Sample {
                    pair: i,
                    pair_name: p.name.clone(),
                    ts_ns: t,
                    user: UserSupplied::new(UserPoint {
                        symbol: p.symbol.clone(),
                        price: upx,
                        adjusted: upx * p.multiplier,
                        ts_ns: uts,
                    }),
                    bleh: BlehPoint {
                        label: p.label.clone(),
                        price: bpx,
                        ts_ns: bts,
                    },
                    multiplier: p.multiplier,
                }),
                (fu, fb) => {
                    p.counts.stale += 1;
                    // Jump to the first grid time at which every stale side
                    // can be fresh again, so long gaps cost nothing.
                    let mut target = next;
                    for (fresh, side) in [(fu, &p.user), (fb, &p.bleh)] {
                        if fresh.is_none()
                            && let Some(a) = side.after(t)
                        {
                            target = target.max(align_up(a, cfg.interval_ns));
                        }
                    }
                    next = target;
                }
            }
            p.last_tick = Some(t);
            p.next_tick = Some(next);
            p.user.prune(t);
            p.bleh.prune(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn mapping() -> Mapping {
        Mapping::parse(
            r#"
[[pair]]
name = "t"
user_symbol = "U"
multiplier = 2.0
[pair.bleh]
market = "hyperliquid:perp:BTC"

[[pair]]
name = "m"
user_symbol = "U"
user_price = "mid"
[pair.bleh]
market = "hyperliquid:perp:BTC"
price = "mid"

[[pair]]
name = "later"
user_symbol = "U"
[pair.bleh]
exposure = "reserved"
"#,
        )
        .unwrap()
    }

    fn aligner() -> Aligner {
        Aligner::new(
            &mapping(),
            AlignConfig {
                interval_ns: S,
                max_stale_ns: 5 * S,
            },
        )
    }

    fn user(ts: i64, px: f64) -> UserSupplied<UserQuote> {
        UserSupplied::new(UserQuote::trade("U", ts, px))
    }

    fn bleh(ts: i64, px: f64, source: ObsSource) -> BlehObs {
        BlehObs {
            instrument: "hyperliquid:perp:BTC".parse().unwrap(),
            source,
            ts_ns: ts,
            price: px,
        }
    }

    #[test]
    fn as_of_samples_on_the_grid_with_the_multiplier() {
        let mut a = aligner();
        let mut got = Vec::new();
        got.extend(a.push_user(&user(S / 2, 50.0)));
        got.extend(a.push_bleh(&bleh(S + S / 2, 101.0, ObsSource::Trade)));
        assert!(got.is_empty(), "first grid time is 2s; nothing passed it");
        got.extend(a.push_user(&user(2 * S + 1, 51.0)));
        assert!(got.is_empty(), "bleh has not passed 2s");
        got.extend(a.push_bleh(&bleh(3 * S + 1, 103.0, ObsSource::Trade)));
        assert_eq!(got.len(), 1);
        let s = &got[0];
        assert_eq!((s.pair_name.as_ref(), s.ts_ns), ("t", 2 * S));
        assert_eq!(s.user.get().price, 50.0);
        assert_eq!(s.user.get().adjusted, 100.0);
        assert_eq!(s.bleh.price, 101.0);
        assert_eq!(s.basis(), 1.0);
        // Both sides reached only 2s + 1ns: 3s cannot be sampled, even at
        // the end of input.
        assert!(a.flush().is_empty());
        a.push_user(&user(3 * S, 52.0));
        let s = a.flush();
        assert_eq!(s.len(), 1);
        assert_eq!(
            (s[0].ts_ns, s[0].user.get().price, s[0].bleh.price),
            (3 * S, 52.0, 101.0)
        );
    }

    #[test]
    fn heartbeats_move_the_bleh_side_and_staleness_is_counted() {
        let mut a = aligner();
        a.push_bleh(&bleh(0, 100.0, ObsSource::Trade));
        a.push_user(&user(0, 50.0));
        let s = a.bleh_clock(20 * S);
        assert!(s.is_empty(), "user watermark is still 0");
        let s = a.push_user(&user(20 * S, 50.0));
        // Grid 0s..=5s are fresh (6 samples); 6s is stale on both sides
        // and the aligner skips straight to 20s, which waits for a later
        // watermark.
        assert_eq!(
            s.len(),
            6,
            "{:?}",
            s.iter().map(|s| s.ts_ns).collect::<Vec<_>>()
        );
        assert!(a.counts(0).stale >= 1);
        assert_eq!(a.counts(0).user, 2);
    }

    #[test]
    fn late_points_are_dropped_and_counted() {
        let mut a = aligner();
        a.push_user(&user(0, 50.0));
        a.push_bleh(&bleh(0, 100.0, ObsSource::Trade));
        a.push_user(&user(3 * S, 50.0));
        a.push_bleh(&bleh(3 * S, 100.0, ObsSource::Trade));
        assert!(a.push_bleh(&bleh(S, 1.0, ObsSource::Trade)).is_empty());
        assert_eq!(a.counts(0).late, 1);
    }

    #[test]
    fn routing_by_symbol_series_and_user_price() {
        let mut a = aligner();
        // A trade only reaches the "any" pair, a quote reaches both.
        a.push_user(&user(0, 50.0));
        a.push_user(&UserSupplied::new(UserQuote::quote("U", 0, 99.0, 101.0)));
        a.push_user(&UserSupplied::new(UserQuote::trade("OTHER", 0, 1.0)));
        a.push_bleh(&bleh(0, 100.0, ObsSource::Mid));
        a.push_bleh(&bleh(0, 1.0, ObsSource::Bar(mesh_types::BarInterval::M1)));
        assert_eq!(a.counts(0).user, 2);
        assert_eq!(a.counts(1).user, 1);
        assert_eq!((a.counts(0).bleh, a.counts(1).bleh), (0, 1));
        assert_eq!(
            a.counts(2),
            InputCounts::default(),
            "inactive pairs get nothing"
        );
        assert!(a.wants_symbol("U") && !a.wants_symbol("OTHER"));
        let s = a.flush();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].pair, s[0].user.get().price), (1, 100.0));
    }

    #[test]
    fn receive_clock_samples_last_values_every_interval() {
        let mut a = aligner();
        a.push_user(&user(10 * S, 50.0));
        a.push_bleh(&bleh(10 * S, 100.0, ObsSource::Trade));
        let s = a.both_clocks(13 * S);
        let ts: Vec<i64> = s.iter().map(|s| s.ts_ns).collect();
        assert_eq!(ts, [10 * S, 11 * S, 12 * S]);
    }
}
