//! Spot composite across independent venue markets (method spec § 8).
//!
//! 1. Each source's observation is its newest trade price or top-of-book
//!    mid at or before the valuation time (the newer of the two; a mid
//!    wins a tie), times the source's multiplier.
//! 2. Fresh sources (age at most `max_age_secs`) are eligible. If two or
//!    more eligible sources are labelled `benchmark` by their newest
//!    liquidity grade, only those are used; otherwise every eligible source
//!    is used and, if a `tape` (or ungraded) source is among them, the
//!    value is flagged `spot_tape`.
//! 3. With no fresh source, the single freshest source no older than
//!    `hard_max_age_secs` is used, flagged `spot_stale`.
//! 4. The composite is the median. With three or more sources, those
//!    further than `max_dispersion_bps` from the median are dropped and
//!    the median recomputed (flag `spot_dispersed`); with two that
//!    disagree by more than that, both are kept and the value is flagged.
//!
//! The median of three or more sources moves by at most the spread of the
//! remaining sources when any one source is removed, which is what makes
//! the composite tolerate the loss of one source (admission policy,
//! "Removal").

use std::collections::BTreeSet;

use mesh_types::{Grade, LiquidityLabel, SyntheticQuality};

use crate::registry::SpotConfig;

/// One source's observation at the valuation time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Index of the source in the exposure's source list.
    pub source: usize,
    /// Price in the exposure's units.
    pub price: f64,
    /// Observation time, ms.
    pub ts_ms: i64,
    /// The source's newest liquidity grade, if any.
    pub grade: Option<Grade>,
    /// The source's newest liquidity label; ungraded sources are tapes.
    pub label: LiquidityLabel,
}

/// The composite.
#[derive(Debug, Clone, PartialEq)]
pub struct Composite {
    /// Composite price.
    pub price: f64,
    /// Newest observation time among the sources used.
    pub ts_ms: i64,
    /// Sources used (indices into the exposure's source list), ascending.
    pub used: Vec<usize>,
    /// Quality flags raised.
    pub quality: BTreeSet<SyntheticQuality>,
    /// Inherited grade (method spec § 10).
    pub grade: Grade,
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

const fn demote(g: Grade) -> Grade {
    match g {
        Grade::A => Grade::B,
        Grade::B => Grade::C,
        Grade::C | Grade::D => Grade::D,
    }
}

/// Grade inherited from the sources used: the second-best grade with two
/// or more sources (no single source decides), the only source's grade
/// demoted one step with one, and `D` when the spot is stale. Ungraded
/// sources count as `D`.
#[must_use]
pub fn inherited_grade(grades: &[Option<Grade>], stale: bool) -> Grade {
    if stale {
        return Grade::D;
    }
    let mut g: Vec<Grade> = grades.iter().map(|g| g.unwrap_or(Grade::D)).collect();
    g.sort();
    match g.as_slice() {
        [] => Grade::D,
        [only] => demote(*only),
        [_, second, ..] => *second,
    }
}

/// The label for a grade: `benchmark` for A and B, else `tape`.
#[must_use]
pub const fn label_for(grade: Option<Grade>) -> LiquidityLabel {
    match grade {
        Some(Grade::A | Grade::B) => LiquidityLabel::Benchmark,
        _ => LiquidityLabel::Tape,
    }
}

/// Builds the composite at `as_of_ms` from each source's observation (at
/// most one per source, none newer than `as_of_ms`). `None` if no source
/// is usable.
#[must_use]
pub fn composite(obs: &[Observation], as_of_ms: i64, cfg: &SpotConfig) -> Option<Composite> {
    let age = |o: &Observation| as_of_ms - o.ts_ms;
    let fresh_ms = i64::from(cfg.max_age_secs) * 1000;
    let hard_ms = i64::from(cfg.hard_max_age_secs) * 1000;
    let usable: Vec<&Observation> = obs
        .iter()
        .filter(|o| o.price.is_finite() && o.price > 0.0 && age(o) >= 0)
        .collect();
    let fresh: Vec<&Observation> = usable
        .iter()
        .copied()
        .filter(|o| age(o) <= fresh_ms)
        .collect();
    let mut quality = BTreeSet::new();
    let (mut used, stale): (Vec<&Observation>, bool) = if fresh.is_empty() {
        let freshest = usable
            .iter()
            .copied()
            .filter(|o| age(o) <= hard_ms)
            .min_by_key(|o| (age(o), o.source))?;
        quality.insert(SyntheticQuality::SpotStale);
        (vec![freshest], true)
    } else {
        let bench: Vec<&Observation> = fresh
            .iter()
            .copied()
            .filter(|o| o.label == LiquidityLabel::Benchmark)
            .collect();
        if bench.len() >= 2 {
            (bench, false)
        } else {
            if fresh.iter().any(|o| o.label == LiquidityLabel::Tape) {
                quality.insert(SyntheticQuality::SpotTape);
            }
            (fresh, false)
        }
    };
    let tol = f64::from(cfg.max_dispersion_bps) / 10_000.0;
    let mut prices: Vec<f64> = used.iter().map(|o| o.price).collect();
    let mut med = median(&mut prices);
    if used.len() >= 3 {
        let before = used.len();
        used.retain(|o| ((o.price - med) / med).abs() <= tol);
        if used.len() < before {
            quality.insert(SyntheticQuality::SpotDispersed);
            let mut prices: Vec<f64> = used.iter().map(|o| o.price).collect();
            med = median(&mut prices);
        }
    } else if used.len() == 2 && ((used[0].price - used[1].price) / med).abs() > tol {
        quality.insert(SyntheticQuality::SpotDispersed);
    }
    if used.is_empty() {
        // Unreachable in practice: the median itself is within tolerance
        // of the median for an odd count. Keep a total fallback.
        return None;
    }
    if used.len() == 1 {
        quality.insert(SyntheticQuality::SpotSingleSource);
    }
    let grades: Vec<Option<Grade>> = used.iter().map(|o| o.grade).collect();
    let mut idx: Vec<usize> = used.iter().map(|o| o.source).collect();
    idx.sort_unstable();
    Some(Composite {
        price: med,
        ts_ms: used.iter().map(|o| o.ts_ms).max().unwrap_or(as_of_ms),
        used: idx,
        quality,
        grade: inherited_grade(&grades, stale),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::SpotTracks;

    fn cfg() -> SpotConfig {
        SpotConfig {
            tracks: SpotTracks::Cash,
            sources: Vec::new(),
            max_age_secs: 60,
            hard_max_age_secs: 3600,
            max_dispersion_bps: 50,
        }
    }

    fn ob(source: usize, price: f64, ts_ms: i64, grade: Option<Grade>) -> Observation {
        Observation {
            source,
            price,
            ts_ms,
            grade,
            label: label_for(grade),
        }
    }

    const T: i64 = 1_000_000_000;

    #[test]
    fn median_of_benchmarks() {
        let obs = [
            ob(0, 100.0, T - 1_000, Some(Grade::A)),
            ob(1, 100.2, T - 2_000, Some(Grade::B)),
            ob(2, 99.9, T - 500, Some(Grade::A)),
            ob(3, 150.0, T - 500, Some(Grade::D)),
        ];
        let c = composite(&obs, T, &cfg()).unwrap();
        assert_eq!(c.price, 100.0);
        assert_eq!(c.used, vec![0, 1, 2]);
        assert!(c.quality.is_empty());
        assert_eq!(c.grade, Grade::A);
        assert_eq!(c.ts_ms, T - 500);
    }

    #[test]
    fn removing_any_one_source_stays_within_tolerance() {
        let obs = [
            ob(0, 100.00, T - 1_000, Some(Grade::A)),
            ob(1, 100.10, T - 1_000, Some(Grade::A)),
            ob(2, 99.95, T - 1_000, Some(Grade::B)),
            ob(3, 100.05, T - 1_000, Some(Grade::B)),
        ];
        let full = composite(&obs, T, &cfg()).unwrap().price;
        for skip in 0..obs.len() {
            let rest: Vec<Observation> = obs.iter().copied().filter(|o| o.source != skip).collect();
            let c = composite(&rest, T, &cfg()).unwrap();
            let bps = ((c.price - full) / full).abs() * 10_000.0;
            assert!(bps <= 50.0, "without {skip}: {bps} bps");
            assert!(bps <= 10.0, "median moves less than the source spread");
        }
    }

    #[test]
    fn outliers_are_dropped() {
        let obs = [
            ob(0, 100.0, T, Some(Grade::A)),
            ob(1, 100.1, T, Some(Grade::A)),
            ob(2, 103.0, T, Some(Grade::A)),
        ];
        let c = composite(&obs, T, &cfg()).unwrap();
        assert!(c.quality.contains(&SyntheticQuality::SpotDispersed));
        assert_eq!(c.used, vec![0, 1]);
        assert!((c.price - 100.05).abs() < 1e-12);
    }

    #[test]
    fn tapes_only_when_benchmarks_are_short() {
        let obs = [ob(0, 100.0, T, Some(Grade::A)), ob(1, 100.2, T, None)];
        let c = composite(&obs, T, &cfg()).unwrap();
        assert!(c.quality.contains(&SyntheticQuality::SpotTape));
        assert_eq!(c.used.len(), 2);
        // Second-best grade: the ungraded source counts as D.
        assert_eq!(c.grade, Grade::D);
    }

    #[test]
    fn staleness_and_single_source() {
        let obs = [
            ob(0, 100.0, T - 120_000, Some(Grade::A)),
            ob(1, 101.0, T - 90_000, Some(Grade::A)),
        ];
        let c = composite(&obs, T, &cfg()).unwrap();
        assert_eq!(c.used, vec![1]);
        assert!(c.quality.contains(&SyntheticQuality::SpotStale));
        assert!(c.quality.contains(&SyntheticQuality::SpotSingleSource));
        assert_eq!(c.grade, Grade::D);
        // Beyond the hard limit: nothing.
        let old = [ob(0, 100.0, T - 4_000_000, Some(Grade::A))];
        assert!(composite(&old, T, &cfg()).is_none());
        // A future observation is never used.
        let future = [ob(0, 100.0, T + 1, Some(Grade::A))];
        assert!(composite(&future, T, &cfg()).is_none());
        // One fresh benchmark: demoted one step.
        let one = [ob(0, 100.0, T, Some(Grade::A))];
        let c = composite(&one, T, &cfg()).unwrap();
        assert_eq!(c.grade, Grade::B);
        assert!(c.quality.contains(&SyntheticQuality::SpotSingleSource));
    }

    #[test]
    fn two_disagreeing_sources_are_flagged() {
        let obs = [
            ob(0, 100.0, T, Some(Grade::A)),
            ob(1, 102.0, T, Some(Grade::B)),
        ];
        let c = composite(&obs, T, &cfg()).unwrap();
        assert!(c.quality.contains(&SyntheticQuality::SpotDispersed));
        assert_eq!(c.price, 101.0);
        assert_eq!(c.grade, Grade::B);
    }
}
