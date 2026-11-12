//! Rate curve: quote conversion, par bootstrapping and interpolation
//! (method spec § 7).
//!
//! The curve is a set of nodes `(t, z)`: horizon `t` in act/365F years and
//! continuously compounded zero rate `z` (a fraction, not percent). It is
//! a *sticky-tenor* curve: a point quoted for a tenor gives the rate for
//! that horizon from any valuation time, and staleness is flagged rather
//! than corrected. Between nodes, `z·t` (minus the log discount factor) is
//! linear in `t`, so instantaneous forwards are piecewise flat. Before the
//! first node and after the last, `z` is held flat; past the last node the
//! value is flagged `curve_extrapolated`.

use mesh_types::{Compounding, DayCount};

use crate::calendar::Calendar;
use crate::date::Date;
use crate::detmath::{exp, ln};

/// Days per year of the act/365F year fraction.
pub const YEAR_DAYS: f64 = 365.0;

/// A rate quote over a single period, as published.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quote {
    /// Simple interest at `rate` (fraction) under `day_count`.
    Simple {
        /// Rate, as a fraction.
        rate: f64,
        /// Accrual basis.
        day_count: DayCount,
    },
    /// Bank discount rate under `day_count`.
    Discount {
        /// Rate, as a fraction.
        rate: f64,
        /// Accrual basis.
        day_count: DayCount,
    },
    /// Zero-coupon yield compounded `per_year` times a year (0 for
    /// continuous).
    Zero {
        /// Yield, as a fraction.
        rate: f64,
        /// Compounding frequency; 0 is continuous.
        per_year: u32,
    },
}

/// Accrual fraction of `days` calendar days under a day count. Act/act
/// uses 365 and 30/360 uses 360 (exact for the single-period tenors the
/// method reads; method spec § 7.1).
#[must_use]
pub fn accrual(days: i32, day_count: DayCount) -> f64 {
    let d = f64::from(days);
    match day_count {
        DayCount::Act360 | DayCount::Thirty360 => d / 360.0,
        DayCount::Act365Fixed | DayCount::ActAct => d / 365.0,
    }
}

/// Continuously compounded zero rate of a single-period quote over `days`
/// calendar days; `None` if the quote is not usable (non-positive period
/// or growth).
#[must_use]
pub fn zero_from_quote(days: i32, quote: Quote) -> Option<f64> {
    if days <= 0 {
        return None;
    }
    let t = f64::from(days) / YEAR_DAYS;
    let z = match quote {
        Quote::Simple { rate, day_count } => ln(1.0 + rate * accrual(days, day_count)) / t,
        Quote::Discount { rate, day_count } => {
            let x = 1.0 - rate * accrual(days, day_count);
            if x <= 0.0 {
                return None;
            }
            -ln(x) / t
        }
        Quote::Zero { rate, per_year: 0 } => rate,
        Quote::Zero { rate, per_year } => {
            let f = f64::from(per_year);
            f * ln(1.0 + rate / f)
        }
    };
    z.is_finite().then_some(z)
}

/// The quote for a compounding convention, or `None` for par yields and
/// conventions that need a missing day count.
#[must_use]
pub fn quote_for(
    rate: f64,
    compounding: Compounding,
    day_count: Option<DayCount>,
) -> Option<Quote> {
    Some(match compounding {
        Compounding::Simple => Quote::Simple {
            rate,
            day_count: day_count?,
        },
        Compounding::Discount => Quote::Discount {
            rate,
            day_count: day_count?,
        },
        Compounding::Annual => Quote::Zero { rate, per_year: 1 },
        Compounding::SemiAnnual => Quote::Zero { rate, per_year: 2 },
        Compounding::Continuous => Quote::Zero { rate, per_year: 0 },
    })
}

/// Coupons per year of a par-yield convention.
#[must_use]
pub const fn coupons_per_year(compounding: Compounding) -> Option<u32> {
    match compounding {
        Compounding::Annual => Some(1),
        Compounding::SemiAnnual => Some(2),
        _ => None,
    }
}

/// Calendar days from `from` to the end of `tenor` (`ON` is the next
/// business day under `cal`; `TN` is not used). `None` for unknown tenors.
#[must_use]
pub fn tenor_days(tenor: &str, from: Date, cal: &Calendar) -> Option<i32> {
    if tenor == "ON" {
        return Some(from.days_until(cal.next_business_day(from)));
    }
    let (n, unit) = tenor.split_at(tenor.len().checked_sub(1)?);
    let n: i32 = n.parse().ok()?;
    let end = match unit {
        "D" => from.add_days(n),
        "W" => from.add_days(7 * n),
        "M" => from.add_months(n),
        "Y" => from.add_months(12 * n),
        _ => return None,
    };
    Some(from.days_until(end))
}

/// Bootstraps par yields with `per_year` coupons a year into zero rates at
/// the par points' horizons (method spec § 7.2).
///
/// `points` are `(t, c)`: horizon in years and par yield (fraction),
/// sorted by `t`. Coupon dates are `k / per_year`; the par yield at a
/// coupon date is linear in `t` between points and flat before the first.
/// A point at or below one coupon period is a single-period zero yield.
#[must_use]
pub fn bootstrap_par(points: &[(f64, f64)], per_year: u32) -> Vec<(f64, f64)> {
    let f = f64::from(per_year.max(1));
    let Some(&(t_max, _)) = points.last() else {
        return Vec::new();
    };
    let par_at = |t: f64| -> f64 {
        let mut prev = points[0];
        if t <= prev.0 {
            return prev.1;
        }
        for &p in &points[1..] {
            if t <= p.0 {
                let w = (t - prev.0) / (p.0 - prev.0);
                return prev.1 + w * (p.1 - prev.1);
            }
            prev = p;
        }
        prev.1
    };
    // Discount factors at coupon dates 1/f, 2/f, ...; ceil(t_max * f) of
    // them. t_max is a horizon in years, so the count is small.
    let n = (t_max * f - 1e-9).ceil().max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = n as usize;
    let mut dfs: Vec<f64> = Vec::with_capacity(n);
    let mut annuity = 0.0;
    for k in 1..=n {
        // k is at most a few hundred, exact in f64.
        #[allow(clippy::cast_precision_loss)]
        let t = k as f64 / f;
        let c = par_at(t) / f;
        let df = (1.0 - c * annuity) / (1.0 + c);
        annuity += df;
        dfs.push(df);
    }
    let log_df_at = |t: f64| -> f64 {
        let pos = t * f;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let k = pos.floor() as usize;
        let lo = if k == 0 {
            0.0
        } else {
            ln(dfs[(k - 1).min(n - 1)])
        };
        if k >= n {
            return lo;
        }
        let hi = ln(dfs[k]);
        #[allow(clippy::cast_precision_loss)]
        let w = pos - k as f64;
        lo + w * (hi - lo)
    };
    points
        .iter()
        .map(|&(t, c)| {
            let z = if t <= 1.0 / f + 1e-12 {
                f * ln(1.0 + c / f)
            } else {
                -log_df_at(t) / t
            };
            (t, z)
        })
        .filter(|(_, z)| z.is_finite())
        .collect()
}

/// A built curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    nodes: Vec<(f64, f64)>,
}

impl Curve {
    /// Builds a curve from point sets in priority order (method spec
    /// § 7.3). `front` (the overnight point) is always a node. Each later
    /// set contributes only points beyond the longest horizon of the sets
    /// before it, so sets never interleave. Returns `None` with no points.
    #[must_use]
    pub fn build(front: Option<(f64, f64)>, sets: &[Vec<(f64, f64)>]) -> Option<Self> {
        Self::build_tagged(front, sets).map(|(c, _)| c)
    }

    /// [`Curve::build`], also returning which sets contributed at least one
    /// node (by index).
    #[must_use]
    pub fn build_tagged(
        front: Option<(f64, f64)>,
        sets: &[Vec<(f64, f64)>],
    ) -> Option<(Self, Vec<bool>)> {
        // (t, z, tag): tag is the set index, or sets.len() for the front.
        let mut nodes: Vec<(f64, f64, usize)> = Vec::new();
        let mut covered = 0.0f64;
        for (i, set) in sets.iter().enumerate() {
            let mut longest = covered;
            for &(t, z) in set {
                if t > covered && t.is_finite() && z.is_finite() {
                    nodes.push((t, z, i));
                    longest = longest.max(t);
                }
            }
            covered = longest;
        }
        if let Some((t, z)) = front
            && t.is_finite()
            && z.is_finite()
            && t > 0.0
        {
            nodes.push((t, z, sets.len()));
        }
        // Sort by horizon; on equal horizons keep the first pushed
        // (higher priority set before the overnight point).
        nodes.sort_by(|a, b| a.0.total_cmp(&b.0));
        nodes.dedup_by(|b, a| a.0 == b.0);
        if nodes.is_empty() {
            return None;
        }
        let mut used = vec![false; sets.len()];
        for n in &nodes {
            if let Some(u) = used.get_mut(n.2) {
                *u = true;
            }
        }
        let nodes = nodes.into_iter().map(|(t, z, _)| (t, z)).collect();
        Some((Self { nodes }, used))
    }

    /// The nodes `(t, z)`, by horizon.
    #[must_use]
    pub fn nodes(&self) -> &[(f64, f64)] {
        &self.nodes
    }

    /// The longest horizon.
    #[must_use]
    pub fn max_horizon(&self) -> f64 {
        self.nodes.last().map_or(0.0, |n| n.0)
    }

    /// `z(t)·t`: minus the log discount factor to horizon `t`.
    #[must_use]
    pub fn log_growth(&self, t: f64) -> f64 {
        if t <= 0.0 {
            return 0.0;
        }
        let first = self.nodes[0];
        if t <= first.0 {
            return first.1 * t;
        }
        let mut prev = first;
        for &n in &self.nodes[1..] {
            if t <= n.0 {
                let w = (t - prev.0) / (n.0 - prev.0);
                return prev.1 * prev.0 + w * (n.1 * n.0 - prev.1 * prev.0);
            }
            prev = n;
        }
        prev.1 * t
    }

    /// Zero rate to horizon `t`, and whether `t` is past the last node.
    #[must_use]
    pub fn zero(&self, t: f64) -> (f64, bool) {
        let extrapolated = t > self.max_horizon();
        if t <= 0.0 {
            return (self.nodes[0].1, false);
        }
        (self.log_growth(t) / t, extrapolated)
    }

    /// Growth factor from horizon `t1` to `t2`: `exp(z2 t2 - z1 t1)`.
    #[must_use]
    pub fn growth(&self, t1: f64, t2: f64) -> f64 {
        exp(self.log_growth(t2) - self.log_growth(t1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::CalendarId;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn single_period_quotes() {
        // 4% simple act/360 for 90 days.
        let z = zero_from_quote(
            90,
            Quote::Simple {
                rate: 0.04,
                day_count: DayCount::Act360,
            },
        )
        .unwrap();
        assert!(close(
            z,
            (1.0f64 + 0.04 * 0.25).ln() / (90.0 / 365.0),
            1e-15
        ));
        let zd = zero_from_quote(
            90,
            Quote::Discount {
                rate: 0.04,
                day_count: DayCount::Act360,
            },
        )
        .unwrap();
        assert!(zd > z);
        let za = zero_from_quote(
            365,
            Quote::Zero {
                rate: 0.04,
                per_year: 1,
            },
        )
        .unwrap();
        assert!(close(za, 1.04f64.ln(), 1e-15));
        assert_eq!(
            zero_from_quote(
                10,
                Quote::Zero {
                    rate: 0.04,
                    per_year: 0
                }
            ),
            Some(0.04)
        );
        assert_eq!(
            zero_from_quote(
                0,
                Quote::Zero {
                    rate: 0.04,
                    per_year: 0
                }
            ),
            None
        );
        assert!(
            zero_from_quote(
                400,
                Quote::Discount {
                    rate: 0.95,
                    day_count: DayCount::Act360
                }
            )
            .is_none()
        );
    }

    #[test]
    fn tenors() {
        let cal = Calendar::new(CalendarId::UsGovernmentSecurities);
        let fri = Date::parse("2026-07-02").unwrap();
        // Friday 3 July 2026 is open under this calendar (Saturday
        // holidays are not moved).
        assert_eq!(tenor_days("ON", fri, &cal), Some(1));
        let thu = Date::parse("2026-11-26").unwrap();
        assert_eq!(tenor_days("ON", thu, &cal), Some(1));
        let wed = Date::parse("2026-11-25").unwrap();
        assert_eq!(tenor_days("ON", wed, &cal), Some(2));
        assert_eq!(
            tenor_days("3M", Date::parse("2026-01-31").unwrap(), &cal),
            Some(89)
        );
        // 2028 is a leap year.
        assert_eq!(tenor_days("2Y", wed, &cal), Some(731));
        assert_eq!(tenor_days("4W", wed, &cal), Some(28));
        assert_eq!(tenor_days("TN", wed, &cal), None);
    }

    #[test]
    fn flat_par_curve_bootstraps_flat() {
        // A flat 5% semi-annual par curve has a flat zero curve at
        // 2 ln(1.025).
        let pts = [(1.0, 0.05), (2.0, 0.05), (5.0, 0.05), (10.0, 0.05)];
        let want = 2.0 * 1.025f64.ln();
        for (t, z) in bootstrap_par(&pts, 2) {
            assert!(close(z, want, 1e-12), "{t}: {z}");
        }
        let pts = [(2.0, 0.04), (3.0, 0.04)];
        for (_, z) in bootstrap_par(&pts, 1) {
            assert!(close(z, 1.04f64.ln(), 1e-12));
        }
    }

    #[test]
    fn upward_par_curve_gives_zeros_above_par() {
        let pts = [(0.5, 0.03), (1.0, 0.032), (2.0, 0.035), (5.0, 0.04)];
        let z = bootstrap_par(&pts, 2);
        assert_eq!(z.len(), 4);
        // Zero yields exceed (continuous) par yields on a rising curve.
        assert!(z[3].1 > 2.0 * (1.0 + 0.04 / 2.0f64).ln());
        assert!(z.windows(2).all(|w| w[0].1 < w[1].1));
    }

    #[test]
    fn interpolation_priority_and_extrapolation() {
        let ois = vec![(0.25, 0.040), (1.0, 0.038)];
        let tsy = vec![(0.5, 0.050), (2.0, 0.036), (5.0, 0.035)];
        let c = Curve::build(Some((1.0 / 365.0, 0.043)), &[ois, tsy]).unwrap();
        let ts: Vec<f64> = c.nodes().iter().map(|n| n.0).collect();
        // The treasury 0.5y point lies inside the higher-priority set.
        assert_eq!(ts, vec![1.0 / 365.0, 0.25, 1.0, 2.0, 5.0]);
        // Exact at nodes; linear in z·t between them.
        assert!(close(c.zero(1.0).0, 0.038, 1e-15));
        let mid = c.log_growth(1.5);
        assert!(close(mid, (0.038 + 2.0 * 0.036) / 2.0, 1e-15));
        assert_eq!(c.zero(6.0), (0.035, true));
        assert!(!c.zero(0.001).1);
        assert!(close(c.zero(0.0001).0, 0.043, 1e-15));
        assert!(close(c.growth(0.0, 1.0), 0.038f64.exp(), 1e-14));
        assert!(Curve::build(None, &[]).is_none());
        let (_, used) = Curve::build_tagged(None, &[vec![(1.0, 0.04)], vec![(0.5, 0.05)]]).unwrap();
        assert_eq!(used, vec![true, false]);
    }
}
