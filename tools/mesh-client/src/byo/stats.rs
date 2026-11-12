//! Local tracking statistics: how bleh.xyz prices compare with the user's
//! own feed. Computed on aligned samples (one per interval), on the user's
//! machine, and never sent anywhere.
//!
//! - **Basis**: `bleh - user x multiplier`, in price units and in basis
//!   points of the user price.
//! - **Tracking error**: the standard deviation of the difference of log
//!   returns over one interval, in basis points.
//! - **Lead/lag**: the shift `k` (in intervals, up to `max_lag`) that
//!   maximizes the correlation of bleh.xyz returns at `t` with user returns
//!   at `t + k`. Positive `bleh_lead_ms`: bleh.xyz moves first; negative:
//!   the user's feed moves first.

use std::collections::VecDeque;

/// Running mean and variance (Welford).
#[derive(Debug, Clone, Copy, Default)]
struct Welford {
    n: u64,
    mean: f64,
    m2: f64,
}

impl Welford {
    fn push(&mut self, x: f64) {
        self.n += 1;
        #[allow(clippy::cast_precision_loss)]
        let n = self.n as f64;
        let d = x - self.mean;
        self.mean += d / n;
        self.m2 += d * (x - self.mean);
    }

    fn mean(&self) -> Option<f64> {
        (self.n > 0).then_some(self.mean)
    }

    fn std(&self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        (self.n > 1).then(|| (self.m2 / (self.n - 1) as f64).sqrt())
    }
}

/// Pearson correlation over pairs.
fn corr(pairs: impl Iterator<Item = (f64, f64)>) -> Option<(f64, u64)> {
    let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0u64, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (x, y) in pairs {
        n += 1;
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    if n < 3 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f64;
    let cov = sxy - sx * sy / nf;
    let vx = sxx - sx * sx / nf;
    let vy = syy - sy * sy / nf;
    if vx <= 0.0 || vy <= 0.0 {
        return None;
    }
    Some((cov / (vx.sqrt() * vy.sqrt()), n))
}

/// Settings for the statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsConfig {
    /// Sample interval, nanoseconds.
    pub interval_ns: i64,
    /// Largest lead or lag tested, in intervals.
    pub max_lag_steps: usize,
    /// Return pairs kept for lead/lag (the most recent).
    pub window: usize,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            interval_ns: 1_000_000_000,
            max_lag_steps: 10,
            window: 3600,
        }
    }
}

/// Input counters reported with the statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputCounts {
    /// User observations routed to the pair.
    pub user: u64,
    /// bleh.xyz observations routed to the pair.
    pub bleh: u64,
    /// Observations older than an already-sampled interval (dropped).
    pub late: u64,
    /// Intervals skipped because one side had no fresh price.
    pub stale: u64,
    /// Observations dropped to bound memory.
    pub dropped: u64,
}

/// The best lead or lag found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeadLag {
    /// Positive: bleh.xyz moves first by this much; negative: the user's
    /// feed moves first.
    pub bleh_lead_ms: i64,
    /// Return correlation at that shift.
    pub corr: f64,
    /// Return correlation with no shift.
    pub corr_at_zero: Option<f64>,
    /// Return pairs used at that shift.
    pub pairs: u64,
    /// Largest shift tested, milliseconds.
    pub max_lag_ms: i64,
}

/// A statistics snapshot for one pair.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReport {
    /// Pair name.
    pub pair: String,
    /// Aligned samples.
    pub samples: u64,
    /// Sample interval, milliseconds.
    pub interval_ms: i64,
    /// First and last sample time, Unix nanoseconds.
    pub span_ns: Option<(i64, i64)>,
    /// Basis (`bleh - user x multiplier`): mean, standard deviation,
    /// minimum, maximum, last.
    pub basis_mean: Option<f64>,
    /// See [`TrackingReport::basis_mean`].
    pub basis_std: Option<f64>,
    /// See [`TrackingReport::basis_mean`].
    pub basis_min: Option<f64>,
    /// See [`TrackingReport::basis_mean`].
    pub basis_max: Option<f64>,
    /// See [`TrackingReport::basis_mean`].
    pub basis_last: Option<f64>,
    /// Basis in basis points of the user price: mean and standard
    /// deviation.
    pub basis_bps_mean: Option<f64>,
    /// See [`TrackingReport::basis_bps_mean`].
    pub basis_bps_std: Option<f64>,
    /// One-interval return pairs.
    pub returns: u64,
    /// Standard deviation of the return difference, basis points per
    /// interval.
    pub tracking_error_bps: Option<f64>,
    /// Correlation of one-interval returns.
    pub return_corr: Option<f64>,
    /// Lead/lag over the most recent window.
    pub lead_lag: Option<LeadLag>,
    /// Input counters.
    pub inputs: InputCounts,
}

/// Running statistics for one pair.
#[derive(Debug, Clone)]
pub struct TrackingStats {
    cfg: StatsConfig,
    pair: String,
    samples: u64,
    span: Option<(i64, i64)>,
    basis: Welford,
    basis_bps: Welford,
    basis_min: f64,
    basis_max: f64,
    basis_last: Option<f64>,
    ret_diff: Welford,
    ret_corr_acc: CorrAcc,
    prev: Option<(i64, f64, f64)>,
    /// One slot per interval, most recent last: `(bleh return, user
    /// return)`, `None` where an interval has no return.
    series: VecDeque<Option<(f64, f64)>>,
    last_tick: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default)]
struct CorrAcc {
    n: u64,
    sx: f64,
    sy: f64,
    sxx: f64,
    syy: f64,
    sxy: f64,
}

impl CorrAcc {
    fn push(&mut self, x: f64, y: f64) {
        self.n += 1;
        self.sx += x;
        self.sy += y;
        self.sxx += x * x;
        self.syy += y * y;
        self.sxy += x * y;
    }

    fn corr(&self) -> Option<f64> {
        if self.n < 3 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = self.n as f64;
        let vx = self.sxx - self.sx * self.sx / n;
        let vy = self.syy - self.sy * self.sy / n;
        (vx > 0.0 && vy > 0.0).then(|| (self.sxy - self.sx * self.sy / n) / (vx.sqrt() * vy.sqrt()))
    }
}

impl TrackingStats {
    /// New, empty statistics.
    pub fn new(pair: impl Into<String>, cfg: StatsConfig) -> Self {
        Self {
            cfg,
            pair: pair.into(),
            samples: 0,
            span: None,
            basis: Welford::default(),
            basis_bps: Welford::default(),
            basis_min: f64::INFINITY,
            basis_max: f64::NEG_INFINITY,
            basis_last: None,
            ret_diff: Welford::default(),
            ret_corr_acc: CorrAcc::default(),
            prev: None,
            series: VecDeque::new(),
            last_tick: None,
        }
    }

    /// Adds one aligned sample: the interval time, the user price after the
    /// multiplier, and the bleh.xyz price. Ticks must increase.
    pub fn push(&mut self, tick_ns: i64, user: f64, bleh: f64) {
        if self.last_tick.is_some_and(|t| tick_ns <= t) {
            return;
        }
        self.samples += 1;
        self.span = Some(self.span.map_or((tick_ns, tick_ns), |(a, _)| (a, tick_ns)));
        let b = bleh - user;
        self.basis.push(b);
        self.basis_min = self.basis_min.min(b);
        self.basis_max = self.basis_max.max(b);
        self.basis_last = Some(b);
        if user != 0.0 {
            self.basis_bps.push(b / user.abs() * 1e4);
        }
        let step = self.cfg.interval_ns.max(1);
        let ret = match self.prev {
            Some((t0, u0, b0))
                if tick_ns - t0 == step && u0 > 0.0 && b0 > 0.0 && user > 0.0 && bleh > 0.0 =>
            {
                Some(((bleh / b0).ln(), (user / u0).ln()))
            }
            _ => None,
        };
        if let Some((rb, ru)) = ret {
            self.ret_diff.push(rb - ru);
            self.ret_corr_acc.push(rb, ru);
        }
        // Keep one slot per interval for lead/lag.
        if let Some(t0) = self.last_tick {
            let gap = usize::try_from((tick_ns - t0) / step).unwrap_or(usize::MAX);
            if gap > self.cfg.window {
                self.series.clear();
            } else {
                for _ in 1..gap {
                    self.series.push_back(None);
                }
            }
        }
        self.series.push_back(ret);
        while self.series.len() > self.cfg.window.max(1) {
            self.series.pop_front();
        }
        self.prev = Some((tick_ns, user, bleh));
        self.last_tick = Some(tick_ns);
    }

    /// Lead/lag over the current window.
    pub fn lead_lag(&self) -> Option<LeadLag> {
        let s: Vec<Option<(f64, f64)>> = self.series.iter().copied().collect();
        let max = self.cfg.max_lag_steps;
        let at = |k: isize| {
            corr((0..s.len()).filter_map(|i| {
                let j = isize::try_from(i).ok()? + k;
                let (rb, _) = s.get(i).copied().flatten()?;
                let (_, ru) = s.get(usize::try_from(j).ok()?).copied().flatten()?;
                Some((rb, ru))
            }))
        };
        let max_i = isize::try_from(max).unwrap_or(0);
        let mut best: Option<(isize, f64, u64)> = None;
        // Search outward from zero so that ties keep the smallest shift.
        let mut ks = vec![0isize];
        for k in 1..=max_i {
            ks.push(k);
            ks.push(-k);
        }
        for k in ks {
            if let Some((c, n)) = at(k)
                && best.is_none_or(|(_, bc, _)| c > bc + 1e-12)
            {
                best = Some((k, c, n));
            }
        }
        let interval_ms = self.cfg.interval_ns / 1_000_000;
        best.map(|(k, c, n)| LeadLag {
            bleh_lead_ms: i64::try_from(k).unwrap_or(0) * interval_ms,
            corr: c,
            corr_at_zero: at(0).map(|(c, _)| c),
            pairs: n,
            max_lag_ms: i64::try_from(max).unwrap_or(0) * interval_ms,
        })
    }

    /// A snapshot of the statistics.
    pub fn report(&self, inputs: InputCounts) -> TrackingReport {
        let some = self.samples > 0;
        TrackingReport {
            pair: self.pair.clone(),
            samples: self.samples,
            interval_ms: self.cfg.interval_ns / 1_000_000,
            span_ns: self.span,
            basis_mean: self.basis.mean(),
            basis_std: self.basis.std(),
            basis_min: some.then_some(self.basis_min),
            basis_max: some.then_some(self.basis_max),
            basis_last: self.basis_last,
            basis_bps_mean: self.basis_bps.mean(),
            basis_bps_std: self.basis_bps.std(),
            returns: self.ret_diff.n,
            tracking_error_bps: self.ret_diff.std().map(|s| s * 1e4),
            return_corr: self.ret_corr_acc.corr(),
            lead_lag: self.lead_lag(),
            inputs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    /// A deterministic pseudo-random walk (synthetic; no market data).
    fn walk(n: usize, seed: u64) -> Vec<f64> {
        let mut x = seed;
        let mut p = 100.0;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                #[allow(clippy::cast_precision_loss)]
                let u = (x % 2001) as f64 / 1000.0 - 1.0;
                p *= 1.0 + u * 0.001;
                p
            })
            .collect()
    }

    fn cfg() -> StatsConfig {
        StatsConfig {
            interval_ns: S,
            max_lag_steps: 5,
            window: 1000,
        }
    }

    #[test]
    fn constant_basis_has_no_tracking_error() {
        let p = walk(500, 7);
        let mut st = TrackingStats::new("p", cfg());
        for (i, x) in p.iter().enumerate() {
            let t = i64::try_from(i).unwrap() * S;
            st.push(t, *x, x * 1.001);
        }
        let r = st.report(InputCounts::default());
        assert_eq!(r.samples, 500);
        assert_eq!(r.returns, 499);
        assert!((r.basis_bps_mean.unwrap() - 10.0).abs() < 1e-6, "{r:?}");
        assert!(r.basis_bps_std.unwrap() < 1e-6);
        assert!(r.tracking_error_bps.unwrap() < 1e-6);
        assert!((r.return_corr.unwrap() - 1.0).abs() < 1e-9);
        let ll = r.lead_lag.unwrap();
        assert_eq!(ll.bleh_lead_ms, 0);
        assert_eq!(ll.max_lag_ms, 5000);
    }

    #[test]
    fn basis_moments_are_exact() {
        let mut st = TrackingStats::new("p", cfg());
        for (i, (u, b)) in [(100.0, 101.0), (100.0, 103.0), (100.0, 99.0)]
            .iter()
            .enumerate()
        {
            st.push(i64::try_from(i).unwrap() * S, *u, *b);
        }
        let r = st.report(InputCounts::default());
        assert_eq!(r.basis_mean, Some(1.0));
        assert_eq!(r.basis_std, Some(2.0));
        assert_eq!(
            (r.basis_min, r.basis_max, r.basis_last),
            (Some(-1.0), Some(3.0), Some(-1.0))
        );
        assert_eq!(r.span_ns, Some((0, 2 * S)));
    }

    #[test]
    fn lead_and_lag_are_found_with_the_right_sign() {
        let p = walk(800, 11);
        for shift in [3usize, 0] {
            // bleh at t equals user at t + shift: bleh moves first.
            let mut st = TrackingStats::new("lead", cfg());
            for i in 0..700 {
                st.push(i64::try_from(i).unwrap() * S, p[i], p[i + shift]);
            }
            let ll = st.report(InputCounts::default()).lead_lag.unwrap();
            assert_eq!(
                ll.bleh_lead_ms,
                i64::try_from(shift).unwrap() * 1000,
                "{ll:?}"
            );
            assert!(ll.corr > 0.99);
        }
        // user at t equals bleh at t + 2: the user's feed moves first.
        let mut st = TrackingStats::new("lag", cfg());
        for i in 0..700 {
            st.push(i64::try_from(i).unwrap() * S, p[i + 2], p[i]);
        }
        let r = st.report(InputCounts::default());
        assert_eq!(r.lead_lag.unwrap().bleh_lead_ms, -2000);
        assert!(r.tracking_error_bps.unwrap() > 0.0);
    }

    #[test]
    fn gaps_break_returns_and_shifts() {
        let mut st = TrackingStats::new("p", cfg());
        st.push(0, 100.0, 100.0);
        st.push(S, 101.0, 101.0);
        st.push(5 * S, 102.0, 102.0); // gap: no return across it
        st.push(5 * S, 103.0, 103.0); // not increasing: ignored
        let r = st.report(InputCounts::default());
        assert_eq!((r.samples, r.returns), (3, 1));
        assert_eq!(st.series.len(), 6, "one slot per interval, 0s to 5s");
        assert_eq!(st.series.iter().filter(|r| r.is_some()).count(), 1);
        st.push(5000 * S, 1.0, 1.0);
        assert_eq!(st.series.len(), 1, "a gap longer than the window clears it");
    }

    #[test]
    fn non_positive_prices_skip_returns_but_keep_basis() {
        let mut st = TrackingStats::new("spread", cfg());
        st.push(0, -1.0, -0.5);
        st.push(S, -1.0, 0.5);
        let r = st.report(InputCounts::default());
        assert_eq!((r.samples, r.returns), (2, 0));
        assert_eq!(r.basis_mean, Some(1.0));
        assert!(r.lead_lag.is_none());
    }
}
