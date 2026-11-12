//! Synthetic contract months (observation-v0 § 2.13).
//!
//! A [`SyntheticMonth`] is the fair value of one contract month of an
//! exposure, computed by the versioned method `synthetic-months@N`
//! (`docs/specs/method-synthetic-months-v1.md`) from independent-venue
//! prices, public reference rates, rulebook calendars, carry inputs and,
//! where available, swap-print forward anchors. No exchange price is an
//! input. It is a model value: it states what the inputs imply for the
//! month, not what any exchange traded.
//!
//! - `prov.venue` is the pseudo-venue [`SYNTHETIC_VENUE`] and
//!   `prov.instrument` is `synthetic:month:<exposure>:<month>`, with the
//!   month in the neutral notation `YYYY-MM`.
//! - `prov.ts_source_ms` is the time of the newest input and
//!   `prov.ts_ingest_ns` the newest input's ingest time, so the record is
//!   a pure function of its inputs; `as_of_ms` is the valuation time.
//! - `prov.admission` is the lowest-numbered admission among the inputs;
//!   `admissions` lists every input's admission.
//! - `prov.seq` follows the derived-record rule (observation-v0 § 2.8).

use serde::{Deserialize, Serialize};

use crate::analytics::{Grade, LiquidityLabel};
use crate::decimal::{Price, Size};
use crate::derived::DerivedHeader;
use crate::ids::AdmissionId;
use crate::provenance::Provenance;
use crate::record::{RecordKind, ValidationError, ensure};
use crate::reference::{is_calendar_date, is_year_month};
use crate::time::UnixMillis;

/// Pseudo-venue of synthetic-month records: not a trading venue.
pub const SYNTHETIC_VENUE: &str = "synthetic";
/// Market class of synthetic-month instruments.
pub const MONTH_CLASS: &str = "month";
/// Method name (without version) of synthetic-month records.
pub const SYNTHETIC_METHOD: &str = "synthetic-months";

/// Exposure family, which selects the fair-value formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntheticFamily {
    /// Equity index, cost of carry with a dividend yield.
    EquityIndex,
    /// Crude oil or natural gas, swap-print anchors or financing only.
    Energy,
    /// Precious metal, cost of carry with a lease rate.
    Metal,
    /// Short-term interest rate: expected overnight rate over a reference
    /// period.
    ShortRate,
}

/// Where the carry in a fair value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CarrySource {
    /// An observed public series.
    Observed,
    /// A configured estimate (flag `carry_estimated`).
    Estimated,
    /// Implied by swap-print forward anchors.
    SwapAnchor,
    /// No carry: financing only (flag `carry_unobserved`), or not
    /// applicable (short-rate months).
    None,
}

/// Quality flags of a synthetic month, in canonical (declaration) order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntheticQuality {
    /// No spot source was fresh; the freshest stale one was used.
    SpotStale,
    /// Only one spot source was used.
    SpotSingleSource,
    /// At least one spot source was dropped as an outlier.
    SpotDispersed,
    /// A source labelled `tape` was used because too few benchmark
    /// sources were available.
    SpotTape,
    /// The newest rate input is older than the staleness limit.
    RatesStale,
    /// The month lies beyond the longest curve point (flat extrapolation).
    CurveExtrapolated,
    /// Past overnight fixings in a reference period were missing and
    /// filled from the curve.
    FixingsMissing,
    /// The carry is a configured estimate.
    CarryEstimated,
    /// No carry was available: financing only.
    CarryUnobserved,
    /// The month lies between two forward nodes, at least one a swap
    /// anchor (interpolated).
    AnchorInterpolated,
    /// The month lies beyond the last forward node (extrapolated with
    /// financing).
    AnchorExtrapolated,
    /// The newest anchor print is older than the staleness limit.
    AnchorStale,
    /// The month is past its roll date (rank 0) and still trading until
    /// expiry.
    Rolling,
    /// Less than one day to expiry.
    Expiring,
}

impl SyntheticQuality {
    /// Every flag, in canonical order.
    pub const ALL: [SyntheticQuality; 14] = [
        Self::SpotStale,
        Self::SpotSingleSource,
        Self::SpotDispersed,
        Self::SpotTape,
        Self::RatesStale,
        Self::CurveExtrapolated,
        Self::FixingsMissing,
        Self::CarryEstimated,
        Self::CarryUnobserved,
        Self::AnchorInterpolated,
        Self::AnchorExtrapolated,
        Self::AnchorStale,
        Self::Rolling,
        Self::Expiring,
    ];
}

/// Fair value of one contract month at one valuation time
/// (`synthetic-months@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticMonth {
    /// Provenance (see the module docs).
    pub prov: Provenance,
    /// Method and inputs hash; inputs are every record the value used.
    pub derived: DerivedHeader,
    /// Exposure ID from the registry, such as `us-large-cap-index`.
    pub exposure: String,
    /// Family.
    pub family: SyntheticFamily,
    /// Contract month, `YYYY-MM`.
    pub month: String,
    /// Position in the strip after the roll rule: 1 is the front month,
    /// 2 the next; 0 is a month past its roll date that still trades
    /// until expiry.
    pub rank: u32,
    /// Valuation time.
    pub as_of_ms: UnixMillis,
    /// Last trading day under the family's rulebook calendar.
    pub last_trade_date: String,
    /// The instant the month stops trading (last trading day at the
    /// family's settlement time).
    pub expiry_ms: UnixMillis,
    /// Act/365F years from `as_of_ms` to `expiry_ms`, eight decimals.
    pub year_fraction: Size,
    /// Fair value, at the exposure's price scale. Short-rate months are
    /// quoted as 100 minus the implied rate.
    pub fair_value: Price,
    /// Spot composite used, if any.
    pub spot: Option<Price>,
    /// Time of the newest spot observation used.
    pub spot_ms: Option<UnixMillis>,
    /// Spot sources used in the composite.
    pub spot_sources: u32,
    /// Continuously compounded zero rate to expiry, percent, six decimals.
    pub financing_rate: Price,
    /// Time of the newest rate input used.
    pub rates_ms: UnixMillis,
    /// Carry rate used (dividend yield, lease or convenience yield),
    /// continuously compounded percent, six decimals.
    pub carry_rate: Option<Price>,
    /// Where the carry came from.
    pub carry_source: CarrySource,
    /// Swap-print anchors used.
    pub anchors: u32,
    /// Time of the newest anchor print used.
    pub anchor_ms: Option<UnixMillis>,
    /// Short-rate months: the implied average or compounded rate over the
    /// reference period, percent, six decimals.
    pub implied_rate: Option<Price>,
    /// Quality flags, in canonical order, without duplicates.
    pub quality: Vec<SyntheticQuality>,
    /// Every input's admission, ascending, without duplicates.
    pub admissions: Vec<AdmissionId>,
    /// Liquidity grade inherited from the spot sources; `None` when the
    /// month has no traded input to grade (short-rate months).
    pub grade: Option<Grade>,
    /// `benchmark` only for grades A and B.
    pub label: LiquidityLabel,
}

/// True for an exposure ID: `[a-z0-9][a-z0-9-]{0,47}`.
#[must_use]
pub fn is_exposure_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl SyntheticMonth {
    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Fails on a method other than `synthetic-months@N`, an instrument
    /// that is not `<venue>:month:<exposure>:<month>`, malformed IDs or
    /// dates, rank 0 without the `rolling` flag (or the reverse), a
    /// valuation after expiry, spot fields that disagree,
    /// carry fields that disagree, an implied rate on a non-short-rate
    /// month (or none on a short-rate month), unordered quality flags or
    /// admissions, or a `benchmark` label without grade A or B.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::SyntheticMonth;
        ensure(
            self.derived
                .method
                .as_str()
                .split_once('@')
                .is_some_and(|(n, _)| n == SYNTHETIC_METHOD),
            k,
            || format!("method must be {SYNTHETIC_METHOD}@N"),
        )?;
        ensure(is_exposure_id(&self.exposure), k, || "bad exposure".into())?;
        ensure(is_year_month(&self.month), k, || {
            "month must be YYYY-MM".into()
        })?;
        ensure(
            self.prov.instrument.class_part() == MONTH_CLASS
                && self.prov.instrument.symbol() == format!("{}:{}", self.exposure, self.month),
            k,
            || "instrument must be <venue>:month:<exposure>:<month>".into(),
        )?;
        ensure(is_calendar_date(&self.last_trade_date), k, || {
            "last_trade_date must be a YYYY-MM-DD date".into()
        })?;
        ensure(
            (self.rank == 0) == self.quality.contains(&SyntheticQuality::Rolling),
            k,
            || "rank 0 exactly when rolling".into(),
        )?;
        ensure(self.as_of_ms <= self.expiry_ms, k, || {
            "no valuation after expiry".into()
        })?;
        ensure(
            self.spot.is_some() == self.spot_ms.is_some()
                && self.spot.is_some() == (self.spot_sources > 0),
            k,
            || "spot, spot_ms and spot_sources must agree".into(),
        )?;
        ensure(self.anchor_ms.is_some() == (self.anchors > 0), k, || {
            "anchor_ms and anchors must agree".into()
        })?;
        ensure(
            self.carry_rate.is_none() == (self.carry_source == CarrySource::None),
            k,
            || "carry_rate is set exactly when carry_source is not none".into(),
        )?;
        ensure(
            self.implied_rate.is_some() == (self.family == SyntheticFamily::ShortRate),
            k,
            || "implied_rate is set exactly for short-rate months".into(),
        )?;
        ensure(self.quality.windows(2).all(|w| w[0] < w[1]), k, || {
            "quality flags must be in canonical order without duplicates".into()
        })?;
        ensure(
            !self.admissions.is_empty()
                && self.admissions.windows(2).all(|w| w[0] < w[1])
                && self.admissions[0] == self.prov.admission,
            k,
            || "admissions must be ascending, unique and start with prov.admission".into(),
        )?;
        ensure(
            self.label == LiquidityLabel::Tape || matches!(self.grade, Some(Grade::A | Grade::B)),
            k,
            || "benchmark needs grade A or B".into(),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::record::tests::{px, sz};
    use crate::{ContentHash, InstrumentId, MethodId, Record, Seq, Sha256Digest, VenueId};

    const T0: i64 = 1_767_225_600_000;

    /// A synthetic energy month with every optional field set.
    pub(crate) fn month() -> SyntheticMonth {
        let mut prov = sample("synthetic:month:example-energy:2026-03", 1_767_225_600, 0);
        prov.venue = VenueId::new(SYNTHETIC_VENUE).unwrap();
        prov.seq = Seq::new(1_767_225_600, 0);
        SyntheticMonth {
            prov,
            derived: DerivedHeader {
                method: MethodId::new("synthetic-months@1").unwrap(),
                inputs_hash: ContentHash(Sha256Digest([5; 32])),
                input_count: 7,
            },
            exposure: "example-energy".into(),
            family: SyntheticFamily::Energy,
            month: "2026-03".into(),
            rank: 1,
            as_of_ms: crate::UnixMillis(T0),
            last_trade_date: "2026-03-20".into(),
            expiry_ms: crate::UnixMillis(T0 + 6_800_000_000),
            year_fraction: sz("0.21562658"),
            fair_value: px("5032.75"),
            spot: Some(px("5000.00")),
            spot_ms: Some(crate::UnixMillis(T0 - 400)),
            spot_sources: 3,
            financing_rate: px("4.250000"),
            rates_ms: crate::UnixMillis(T0 - 3_600_000),
            carry_rate: Some(px("1.300000")),
            carry_source: CarrySource::SwapAnchor,
            anchors: 2,
            anchor_ms: Some(crate::UnixMillis(T0 - 7_200_000)),
            implied_rate: None,
            quality: vec![
                SyntheticQuality::SpotDispersed,
                SyntheticQuality::AnchorInterpolated,
            ],
            admissions: vec![
                AdmissionId::new("ADM-0001").unwrap(),
                AdmissionId::new("ADM-9001").unwrap(),
            ],
            grade: Some(Grade::B),
            label: LiquidityLabel::Benchmark,
        }
    }

    /// A synthetic short-rate month (no spot, an implied rate).
    pub(crate) fn short_rate() -> SyntheticMonth {
        let mut m = month();
        m.prov.instrument = InstrumentId::new("synthetic:month:example-rate:2026-03").unwrap();
        m.prov.admission = AdmissionId::new("ADM-9001").unwrap();
        m.exposure = "example-rate".into();
        m.family = SyntheticFamily::ShortRate;
        m.fair_value = px("95.8750");
        m.spot = None;
        m.spot_ms = None;
        m.spot_sources = 0;
        m.carry_rate = None;
        m.carry_source = CarrySource::None;
        m.anchors = 0;
        m.anchor_ms = None;
        m.implied_rate = Some(px("4.125000"));
        m.quality = vec![SyntheticQuality::FixingsMissing];
        m.admissions = vec![AdmissionId::new("ADM-9001").unwrap()];
        m.grade = None;
        m.label = LiquidityLabel::Tape;
        m
    }

    pub(crate) fn samples() -> Vec<Record> {
        vec![month().into(), short_rate().into()]
    }

    #[test]
    fn samples_validate_and_round_trip() {
        for r in samples() {
            r.validate().unwrap_or_else(|e| panic!("{e}"));
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r, "{json}");
            assert!(r.derived().is_some());
            assert!(r.kind().is_derived());
        }
    }

    #[test]
    fn invalid_months_fail() {
        let mut m = month();
        m.rank = 0;
        assert!(m.validate().is_err());
        let mut m = month();
        m.month = "2026-3".into();
        assert!(m.validate().is_err());
        let mut m = month();
        m.prov.instrument = InstrumentId::new("synthetic:month:example-energy:2026-06").unwrap();
        assert!(m.validate().is_err());
        let mut m = month();
        m.spot = None;
        assert!(m.validate().is_err());
        let mut m = month();
        m.carry_source = CarrySource::None;
        assert!(m.validate().is_err());
        let mut m = month();
        m.quality.reverse();
        assert!(m.validate().is_err());
        let mut m = month();
        m.admissions.reverse();
        assert!(m.validate().is_err());
        let mut m = month();
        m.grade = Some(Grade::C);
        assert!(m.validate().is_err());
        let mut m = month();
        m.as_of_ms = crate::UnixMillis(m.expiry_ms.0 + 1);
        assert!(m.validate().is_err());
        let mut m = month();
        m.implied_rate = Some(px("1.0"));
        assert!(m.validate().is_err());
        let mut m = month();
        m.derived.method = MethodId::new("bars@1").unwrap();
        assert!(m.validate().is_err());
        let mut m = short_rate();
        m.implied_rate = None;
        assert!(m.validate().is_err());
    }

    #[test]
    fn quality_order_is_declaration_order() {
        let mut sorted = SyntheticQuality::ALL;
        sorted.sort();
        assert_eq!(sorted, SyntheticQuality::ALL);
        let json = serde_json::to_string(&SyntheticQuality::CarryUnobserved).unwrap();
        assert_eq!(json, r#""carry_unobserved""#);
    }
}
