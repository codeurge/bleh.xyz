//! Fair value per family (method spec §§ 11–12).
//!
//! Horizons are act/365F years from the valuation time. `lg(t)` is the
//! curve's `z(t)·t` and `c` the carry rate (dividend yield, lease rate or
//! convenience yield, continuously compounded, as a fraction).
//!
//! **Cost of carry** (equity index, metal, energy without anchors): with
//! the spot node at horizon `t_s` (0 for a cash underlying, the front
//! month's horizon for a front-month tracker),
//!
//! ```text
//! F(t) = S · exp( lg(t) - lg(t_s) - c · (t - t_s) )
//! ```
//!
//! which is `S·exp((r - c)·τ)` for a cash spot.
//!
//! **Forward nodes** (any family with swap-print anchors): the spot node
//! and the anchor nodes `(t_i, F_i)` define `ln F` piecewise linear in `t`
//! between nodes; outside them the nearest node is carried with the
//! cost-of-carry drift above.
//!
//! **Short rate**: the implied overnight rate over the reference period,
//! from past fixings and curve forwards (§ 12); the fair value is
//! `100 - 100·R`.

use std::collections::BTreeMap;

use crate::calendar::Calendar;
use crate::curve::{Curve, YEAR_DAYS};
use crate::date::{Date, Weekday, YearMonth};
use crate::detmath::{exp, ln};
use crate::registry::ShortRateKind;

/// A forward node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    /// Horizon, years.
    pub t: f64,
    /// Forward price at the horizon.
    pub price: f64,
    /// True for a swap-print anchor, false for the spot node.
    pub anchor: bool,
}

/// A month's forward and how it was obtained.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forward {
    /// Forward price.
    pub price: f64,
    /// An anchor node took part (interpolation or extrapolation from it).
    pub used_anchor: bool,
    /// Interpolated between two nodes, at least one an anchor.
    pub interpolated: bool,
    /// Beyond the last (or before the first) node, which is an anchor.
    pub extrapolated: bool,
}

/// Cost-of-carry log drift from `t1` to `t2`.
fn drift(curve: &Curve, carry: f64, t1: f64, t2: f64) -> f64 {
    curve.log_growth(t2) - curve.log_growth(t1) - carry * (t2 - t1)
}

/// The forward at horizon `t` from `nodes` (any order). `None` without
/// nodes or with a non-positive node price.
#[must_use]
pub fn forward(t: f64, nodes: &[Node], curve: &Curve, carry: f64) -> Option<Forward> {
    let mut nodes: Vec<Node> = nodes.to_vec();
    if nodes.is_empty()
        || nodes
            .iter()
            .any(|n| n.price.is_nan() || n.price <= 0.0 || !n.t.is_finite())
    {
        return None;
    }
    nodes.sort_by(|a, b| a.t.total_cmp(&b.t).then(a.anchor.cmp(&b.anchor)));
    let first = nodes[0];
    let last = nodes[nodes.len() - 1];
    let out = |price: f64, used: bool, interp: bool, extrap: bool| Forward {
        price,
        used_anchor: used,
        interpolated: interp,
        extrapolated: extrap,
    };
    // Exactly at a node: the node's price, with no rounding through logs.
    if let Some(n) = nodes.iter().find(|n| n.t == t) {
        return Some(out(n.price, n.anchor, false, false));
    }
    if t < first.t {
        let p = first.price * exp(-drift(curve, carry, t, first.t));
        return Some(out(p, first.anchor, false, first.anchor));
    }
    if t > last.t {
        let p = last.price * exp(drift(curve, carry, last.t, t));
        return Some(out(p, last.anchor, false, last.anchor));
    }
    let (a, b) = nodes
        .windows(2)
        .map(|w| (w[0], w[1]))
        .find(|(a, b)| t > a.t && t < b.t)?;
    let x = (t - a.t) / (b.t - a.t);
    let p = a.price * exp(x * ln(b.price / a.price));
    let anchored = a.anchor || b.anchor;
    Some(out(p, anchored, anchored, false))
}

/// Carry implied between two nodes: `(lg(t2) - lg(t1) - ln(F2 / F1)) /
/// (t2 - t1)`; `None` for coincident horizons.
#[must_use]
pub fn implied_carry(curve: &Curve, t1: f64, f1: f64, t2: f64, f2: f64) -> Option<f64> {
    let dt = t2 - t1;
    if dt.abs() < 1e-9 || !(f1 > 0.0 && f2 > 0.0) {
        return None;
    }
    Some((curve.log_growth(t2) - curve.log_growth(t1) - (ln(f2) - ln(f1))) / dt)
}

/// A short-rate month's reference period `[start, end)`.
#[must_use]
pub fn reference_period(kind: ShortRateKind, ym: YearMonth) -> (Date, Date) {
    match kind {
        ShortRateKind::CompoundedQuarter => (
            ym.nth_weekday(3, Weekday::Wed),
            ym.plus(3).nth_weekday(3, Weekday::Wed),
        ),
        ShortRateKind::AverageMonth => (ym.first_day(), ym.plus(1).first_day()),
    }
}

/// The implied rate of a short-rate month.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortRate {
    /// Rate over the reference period, a fraction.
    pub rate: f64,
    /// Fixing dates used.
    pub fixings_used: Vec<Date>,
    /// A past fixing was missing and filled from the curve.
    pub fixings_missing: bool,
}

/// The implied rate over `[start, end)` at New York valuation date
/// `today` (method spec § 12). `fixings` maps a fixing date to its rate
/// (fraction). Each business day `b` carries its rate until the next
/// business day; days before the first business day of the period carry
/// the preceding business day's rate. Business days before `today` use
/// their fixing (or, if missing, the curve, flagged); from `today` on the
/// rate is the curve's simple act/360 forward from `b` to the next
/// business day.
#[must_use]
pub fn implied_short_rate(
    kind: ShortRateKind,
    start: Date,
    end: Date,
    today: Date,
    fixings: &BTreeMap<Date, f64>,
    curve: &Curve,
    cal: &Calendar,
) -> ShortRate {
    let mut used = Vec::new();
    let mut missing = false;
    let mut rate_of = |b: Date| -> f64 {
        if b < today {
            if let Some(&r) = fixings.get(&b) {
                used.push(b);
                return r;
            }
            missing = true;
        }
        let next = cal.next_business_day(b);
        let h1 = f64::from(today.days_until(b).max(0)) / YEAR_DAYS;
        let n = b.days_until(next);
        let h2 = h1 + f64::from(n) / YEAR_DAYS;
        (curve.growth(h1, h2) - 1.0) * 360.0 / f64::from(n)
    };
    let total = start.days_until(end);
    let mut b = cal.on_or_before(start);
    let rate = match kind {
        ShortRateKind::CompoundedQuarter => {
            let mut growth = 1.0;
            while b < end {
                let next = cal.next_business_day(b);
                let from = if b < start { start } else { b };
                let to = if next > end { end } else { next };
                let r = rate_of(b);
                growth *= 1.0 + r * f64::from(from.days_until(to)) / 360.0;
                b = next;
            }
            (growth - 1.0) * 360.0 / f64::from(total.max(1))
        }
        ShortRateKind::AverageMonth => {
            let mut sum = 0.0;
            while b < end {
                let next = cal.next_business_day(b);
                let from = if b < start { start } else { b };
                let to = if next > end { end } else { next };
                sum += rate_of(b) * f64::from(from.days_until(to));
                b = next;
            }
            sum / f64::from(total.max(1))
        }
    };
    used.sort();
    used.dedup();
    ShortRate {
        rate,
        fixings_used: used,
        fixings_missing: missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::CalendarId;

    fn flat(z: f64) -> Curve {
        Curve::build(Some((1.0 / 365.0, z)), &[vec![(10.0, z)]]).unwrap()
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn cost_of_carry_and_convergence() {
        let c = flat(0.05);
        let spot = [Node {
            t: 0.0,
            price: 5000.0,
            anchor: false,
        }];
        let f = forward(0.25, &spot, &c, 0.015).unwrap();
        assert!(close(f.price, 5000.0 * (0.035f64 * 0.25).exp(), 1e-9));
        assert!(!f.used_anchor);
        // At expiry the forward is the spot.
        assert_eq!(forward(0.0, &spot, &c, 0.015).unwrap().price, 5000.0);
        // And converges continuously.
        let near = forward(1e-6, &spot, &c, 0.015).unwrap().price;
        assert!((near - 5000.0).abs() < 1e-3);
    }

    #[test]
    fn front_month_tracker() {
        let c = flat(0.04);
        let spot = [Node {
            t: 0.1,
            price: 70.0,
            anchor: false,
        }];
        assert_eq!(forward(0.1, &spot, &c, 0.0).unwrap().price, 70.0);
        let later = forward(0.2, &spot, &c, 0.0).unwrap().price;
        assert!(close(later, 70.0 * (0.04f64 * 0.1).exp(), 1e-9));
        let earlier = forward(0.05, &spot, &c, 0.0).unwrap().price;
        assert!(close(earlier, 70.0 * (-0.04f64 * 0.05).exp(), 1e-9));
    }

    #[test]
    fn anchors_interpolate_log_linearly_and_extrapolate() {
        let c = flat(0.04);
        let nodes = [
            Node {
                t: 0.0,
                price: 70.0,
                anchor: false,
            },
            Node {
                t: 0.5,
                price: 66.0,
                anchor: true,
            },
            Node {
                t: 1.0,
                price: 64.0,
                anchor: true,
            },
        ];
        // Exact at an anchor.
        let at = forward(0.5, &nodes, &c, 0.0).unwrap();
        assert!(close(at.price, 66.0, 1e-12));
        assert!(at.used_anchor && !at.interpolated);
        // Log-linear between nodes.
        let mid = forward(0.75, &nodes, &c, 0.0).unwrap();
        assert!(close(mid.price, (66.0f64 * 64.0).sqrt(), 1e-9));
        assert!(mid.interpolated && mid.used_anchor && !mid.extrapolated);
        // Between spot and the first anchor.
        let early = forward(0.25, &nodes, &c, 0.0).unwrap();
        assert!(close(early.price, (70.0f64 * 66.0).sqrt(), 1e-9));
        assert!(early.interpolated);
        // Past the last anchor: carried with financing.
        let far = forward(1.5, &nodes, &c, 0.0).unwrap();
        assert!(close(far.price, 64.0 * (0.04f64 * 0.5).exp(), 1e-9));
        assert!(far.extrapolated);
        // Implied carry between spot and the first anchor.
        let q = implied_carry(&c, 0.0, 70.0, 0.5, 66.0).unwrap();
        assert!(close(q, 0.04 - (66.0f64 / 70.0).ln() / 0.5, 1e-12));
        assert!(forward(0.1, &[], &c, 0.0).is_none());
    }

    #[test]
    fn short_rates_from_a_flat_curve() {
        let cal = Calendar::new(CalendarId::UsGovernmentSecurities);
        // Flat continuous 4%: the daily simple act/360 forward over n days
        // is (e^{0.04 n/365} - 1) 360 / n.
        let c = flat(0.04);
        let ym = YearMonth::new(2026, 6);
        let (s, e) = reference_period(ShortRateKind::CompoundedQuarter, ym);
        assert_eq!(s.to_string(), "2026-06-17");
        assert_eq!(e.to_string(), "2026-09-16");
        let today = Date::parse("2026-01-05").unwrap();
        let r = implied_short_rate(
            ShortRateKind::CompoundedQuarter,
            s,
            e,
            today,
            &BTreeMap::new(),
            &c,
            &cal,
        );
        // Compounding daily forwards of a flat continuous curve recovers
        // the continuous rate over the period.
        let days = f64::from(s.days_until(e));
        let want = ((0.04 * days / 365.0f64).exp() - 1.0) * 360.0 / days;
        assert!(close(r.rate, want, 1e-12), "{} vs {want}", r.rate);
        assert!(!r.fixings_missing && r.fixings_used.is_empty());

        let (s, e) = reference_period(ShortRateKind::AverageMonth, ym);
        assert_eq!(
            (s.to_string(), e.to_string()),
            ("2026-06-01".into(), "2026-07-01".into())
        );
        let avg = implied_short_rate(
            ShortRateKind::AverageMonth,
            s,
            e,
            today,
            &BTreeMap::new(),
            &c,
            &cal,
        );
        assert!(avg.rate > 0.0394 && avg.rate < 0.0396, "{}", avg.rate);
    }

    #[test]
    fn realised_fixings_are_used() {
        let cal = Calendar::new(CalendarId::UsGovernmentSecurities);
        let c = flat(0.04);
        let ym = YearMonth::new(2026, 6);
        let (s, e) = reference_period(ShortRateKind::AverageMonth, ym);
        // Mid-month: fixings of 5% for every business day before the 15th
        // except the 10th.
        let today = Date::parse("2026-06-15").unwrap();
        let mut fixings = BTreeMap::new();
        let mut d = Date::parse("2026-05-29").unwrap();
        while d < today {
            if cal.is_business_day(d) && d.to_string() != "2026-06-10" {
                fixings.insert(d, 0.05);
            }
            d = d.add_days(1);
        }
        let r = implied_short_rate(ShortRateKind::AverageMonth, s, e, today, &fixings, &c, &cal);
        assert!(r.fixings_missing);
        assert!(r.fixings_used.contains(&Date::parse("2026-06-01").unwrap()));
        assert!(!r.fixings_used.contains(&Date::parse("2026-05-29").unwrap()));
        // About half the month at 5%, the rest near 3.95% (4% continuous act/365 as simple act/360).
        assert!(r.rate > 0.043 && r.rate < 0.046, "{}", r.rate);
    }
}
