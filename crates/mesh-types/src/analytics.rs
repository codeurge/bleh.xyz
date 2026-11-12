//! Analytics record kinds and the coverage signal.
//!
//! The book builder (`crates/book`) computes these from trades and book
//! events with the versioned methods in `docs/operations/book.md`; their
//! shapes live here so the gateway, the archive and every client share one
//! definition, canonical encoding, allowlist entry and Arrow schema.
//!
//! | Kind | Type | Method |
//! |---|---|---|
//! | `activity_bar` | [`ActivityBar`] | `tick-bars@1`, `volume-bars@1`, `range-bars@1` |
//! | `delta_bar` | [`DeltaBar`] | `delta@1` |
//! | `profile_summary` | [`ProfileSummary`] | `value-area@1` |
//! | `order_activity` | [`OrderActivity`] | `order-activity@1` |
//! | `queue_positions` | [`QueuePositions`] | `queue-position@1` |
//! | `sweep` | [`Sweep`] | `sweep@1` |
//! | `heatmap_tile` | [`HeatmapTile`] | `heatmap@1` |
//! | `liquidity_grade` | [`LiquidityGrade`] | `liquidity-grade@1` |
//! | `coverage` | [`Coverage`] | not derived: what the recorder observed |
//!
//! Ratios are integers (basis points, hundredths) and signed quantities use
//! [`SignedSize`], so no record carries a float.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::decimal::{DecimalError, Price, Size};
use crate::derived::DerivedHeader;
use crate::ids::OrderId;
use crate::provenance::Provenance;
use crate::record::{AggressorSide, BarInterval, RecordKind, ValidationError, ensure};
use crate::seq::Seq;
use crate::time::UnixMillis;

// ------------------------------------------------------------------ SignedSize

/// A signed fixed-point quantity (delta, cumulative volume delta): an
/// `i64` mantissa and a scale, like [`Size`] but allowed to be negative.
///
/// The text form is the canonical decimal form used for [`Price`]: exactly
/// `scale` fractional digits and a leading `-` when negative.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignedSize {
    mantissa: i64,
    scale: u8,
}

impl SignedSize {
    /// Builds a value.
    ///
    /// # Errors
    ///
    /// Fails if the scale exceeds [`crate::MAX_SCALE`].
    pub fn new(mantissa: i64, scale: u8) -> Result<Self, DecimalError> {
        Price::new(mantissa, scale)?;
        Ok(Self { mantissa, scale })
    }

    /// Zero at `scale`.
    ///
    /// # Errors
    ///
    /// Fails if the scale exceeds [`crate::MAX_SCALE`].
    pub fn zero(scale: u8) -> Result<Self, DecimalError> {
        Self::new(0, scale)
    }

    /// `a - b`.
    ///
    /// # Errors
    ///
    /// Fails on a scale mismatch or overflow.
    pub fn difference(a: Size, b: Size) -> Result<Self, DecimalError> {
        if a.scale() != b.scale() {
            return Err(DecimalError::ScaleMismatch(a.scale(), b.scale()));
        }
        let m = a
            .mantissa()
            .checked_sub(b.mantissa())
            .ok_or(DecimalError::Overflow)?;
        Self::new(m, a.scale())
    }

    /// Adds a value of the same scale.
    ///
    /// # Errors
    ///
    /// Fails on a scale mismatch or overflow.
    pub fn checked_add(self, other: Self) -> Result<Self, DecimalError> {
        if self.scale != other.scale {
            return Err(DecimalError::ScaleMismatch(self.scale, other.scale));
        }
        let m = self
            .mantissa
            .checked_add(other.mantissa)
            .ok_or(DecimalError::Overflow)?;
        Self::new(m, self.scale)
    }

    /// The integer mantissa.
    #[must_use]
    pub const fn mantissa(self) -> i64 {
        self.mantissa
    }

    /// The number of fractional digits.
    #[must_use]
    pub const fn scale(self) -> u8 {
        self.scale
    }

    fn as_price(self) -> Option<Price> {
        Price::new(self.mantissa, self.scale).ok()
    }
}

impl fmt::Display for SignedSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_price() {
            Some(p) => p.fmt(f),
            None => write!(f, "{}e-{}", self.mantissa, self.scale),
        }
    }
}

impl fmt::Debug for SignedSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SignedSize({self})")
    }
}

impl Serialize for SignedSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SignedSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let p = Price::deserialize(deserializer)?;
        Ok(Self {
            mantissa: p.mantissa(),
            scale: p.scale(),
        })
    }
}

// ---------------------------------------------------------------- activity bars

/// What closes an activity bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivitySpec {
    /// A bar every `trades` trades.
    Tick {
        /// Trades per bar (at least 1).
        trades: u32,
    },
    /// A bar when volume reaches `size`.
    Volume {
        /// Volume threshold (positive).
        size: Size,
    },
    /// A bar whose range stays within `width`.
    Range {
        /// Maximum `high - low` (positive).
        width: Price,
    },
}

impl ActivitySpec {
    /// The method that builds bars of this kind.
    #[must_use]
    pub const fn method(self) -> &'static str {
        match self {
            Self::Tick { .. } => "tick-bars@1",
            Self::Volume { .. } => "volume-bars@1",
            Self::Range { .. } => "range-bars@1",
        }
    }

    /// True if the threshold is positive.
    #[must_use]
    pub fn is_valid(self) -> bool {
        match self {
            Self::Tick { trades } => trades > 0,
            Self::Volume { size } => size.is_positive(),
            Self::Range { width } => width.mantissa() > 0,
        }
    }
}

/// A tick, volume or range bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityBar {
    /// Provenance of the last trade, with the union of the trades' flags.
    pub prov: Provenance,
    /// Method (`tick-bars@1`, `volume-bars@1` or `range-bars@1`) and the
    /// hash of the bar's trades.
    pub derived: DerivedHeader,
    /// What closes the bar.
    pub spec: ActivitySpec,
    /// Start of the reset period (UTC day by default) the bar belongs to.
    pub session_start_ms: UnixMillis,
    /// `seq` of the first trade.
    pub first_seq: Seq,
    /// Time of the first trade.
    pub first_ms: UnixMillis,
    /// First trade price.
    pub open: Price,
    /// Highest trade price.
    pub high: Price,
    /// Lowest trade price.
    pub low: Price,
    /// Last trade price.
    pub close: Price,
    /// Total traded size.
    pub volume: Size,
    /// Size with a buy aggressor.
    pub buy_volume: Size,
    /// Size with a sell aggressor.
    pub sell_volume: Size,
    /// Number of trades.
    pub trade_count: u32,
    /// True if the threshold closed the bar; false if a reset or the end of
    /// input did.
    pub complete: bool,
}

impl ActivityBar {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on an empty bar, a non-positive threshold, a method that does
    /// not match the spec, or inconsistent OHLC.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::ActivityBar;
        ensure(self.trade_count > 0, k, || "bars are never empty".into())?;
        ensure(self.spec.is_valid(), k, || {
            "threshold must be positive".into()
        })?;
        ensure(
            self.derived.method.as_str() == self.spec.method(),
            k,
            || "method does not match the spec".into(),
        )?;
        ensure(
            self.low <= self.open.min(self.close) && self.high >= self.open.max(self.close),
            k,
            || "OHLC is inconsistent".into(),
        )?;
        ensure(self.first_seq <= self.prov.seq, k, || {
            "first trade is after the last".into()
        })?;
        ensure(
            self.derived.input_count == u64::from(self.trade_count),
            k,
            || "input_count must equal trade_count".into(),
        )
    }
}

// -------------------------------------------------------------------- delta

/// Delta and cumulative volume delta for one interval (`delta@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaBar {
    /// Provenance of the last trade in the interval, with the union of the
    /// interval's trade flags.
    pub prov: Provenance,
    /// `delta@1`; inputs are the anchor period's trades through this
    /// interval.
    pub derived: DerivedHeader,
    /// Interval length.
    pub interval: BarInterval,
    /// Interval start (inclusive), aligned.
    pub start_ms: UnixMillis,
    /// Size traded with a buy aggressor.
    pub buy_volume: Size,
    /// Size traded with a sell aggressor.
    pub sell_volume: Size,
    /// `buy_volume - sell_volume`. (Named `volume_delta` because `delta`
    /// is an excluded venue field name.)
    pub volume_delta: SignedSize,
    /// Start of the anchor period.
    pub anchor_start_ms: UnixMillis,
    /// Sum of `volume_delta` from the anchor start through this interval.
    pub cvd: SignedSize,
}

impl DeltaBar {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on a misaligned start, an anchor after the start, or a delta
    /// that is not `buy - sell`.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::DeltaBar;
        ensure(
            self.interval.align(self.start_ms) == self.start_ms,
            k,
            || "start is not aligned to the interval".into(),
        )?;
        ensure(self.anchor_start_ms <= self.start_ms, k, || {
            "anchor starts after the interval".into()
        })?;
        let d = SignedSize::difference(self.buy_volume, self.sell_volume)
            .map_err(|e| ValidationError::new(k, e.to_string()))?;
        ensure(d == self.volume_delta, k, || {
            "volume_delta must be buy - sell".into()
        })?;
        ensure(self.cvd.scale() == self.volume_delta.scale(), k, || {
            "cvd and delta scales differ".into()
        })
    }
}

// ----------------------------------------------------------- profile summary

/// POC and value area of one volume-profile period (`value-area@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSummary {
    /// The profile's provenance.
    pub prov: Provenance,
    /// `value-area@1`, with the profile's inputs.
    pub derived: DerivedHeader,
    /// Period start (inclusive).
    pub start_ms: UnixMillis,
    /// Period end (exclusive).
    pub end_ms: UnixMillis,
    /// Bucket width.
    pub bucket: Price,
    /// Value-area share in basis points (7000 = 70%).
    pub value_area_bps: u32,
    /// Lower bound of the point-of-control bucket.
    pub poc: Price,
    /// Lower bound of the lowest value-area bucket.
    pub value_area_low: Price,
    /// Lower bound of the highest value-area bucket.
    pub value_area_high: Price,
    /// Volume inside the value area.
    pub value_area_volume: Size,
    /// Total volume.
    pub total_volume: Size,
}

impl ProfileSummary {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on an empty range, a share above 100%, a POC outside the value
    /// area or a value-area volume above the total.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::ProfileSummary;
        ensure(self.end_ms > self.start_ms, k, || "empty range".into())?;
        ensure(self.value_area_bps <= 10_000, k, || {
            "value-area share above 100%".into()
        })?;
        ensure(
            self.value_area_low <= self.poc && self.poc <= self.value_area_high,
            k,
            || "POC outside the value area".into(),
        )?;
        ensure(self.value_area_volume <= self.total_volume, k, || {
            "value-area volume above the total".into()
        })
    }
}

// ------------------------------------------------------------ order activity

/// Lifetime statistics of orders removed in an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifetimeStats {
    /// Orders removed (filled or cancelled).
    pub removed: u64,
    /// Removed by a full fill.
    pub filled: u64,
    /// Removed by a cancel.
    pub cancelled: u64,
    /// Cancelled younger than `fleeting_ms`.
    pub fleeting_cancels: u64,
    /// Shortest lifetime, ms.
    pub min_ms: Option<i64>,
    /// Nearest-rank median, ms.
    pub p50_ms: Option<i64>,
    /// Nearest-rank 90th percentile, ms.
    pub p90_ms: Option<i64>,
    /// Longest lifetime, ms.
    pub max_ms: Option<i64>,
}

/// Order activity in one interval (`order-activity@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderActivity {
    /// Provenance of the interval's last event, with the union of flags.
    pub prov: Provenance,
    /// `order-activity@1`; inputs are the interval's book events.
    pub derived: DerivedHeader,
    /// Interval length.
    pub interval: BarInterval,
    /// Interval start (inclusive), aligned.
    pub start_ms: UnixMillis,
    /// `add` events.
    pub adds: u64,
    /// `modify` events.
    pub modifies: u64,
    /// `cancel` events.
    pub cancels: u64,
    /// `fill` events leaving size.
    pub partial_fills: u64,
    /// `fill` events removing the order.
    pub full_fills: u64,
    /// Size added.
    pub added_size: Size,
    /// Size cancelled.
    pub cancelled_size: Size,
    /// Size filled.
    pub filled_size: Size,
    /// `floor(cancels * 10_000 / adds)`.
    pub cancel_to_add_bps: Option<u64>,
    /// `floor((cancels + modifies) * 10_000 / fills)`.
    pub cancel_replace_to_fill_bps: Option<u64>,
    /// Lifetimes of removed orders.
    pub lifetimes: LifetimeStats,
}

impl OrderActivity {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on a misaligned start or lifetime counts that do not add up.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::OrderActivity;
        ensure(
            self.interval.align(self.start_ms) == self.start_ms,
            k,
            || "start is not aligned to the interval".into(),
        )?;
        let l = &self.lifetimes;
        ensure(
            l.filled.checked_add(l.cancelled) == Some(l.removed),
            k,
            || "removed must equal filled plus cancelled".into(),
        )
    }
}

// ----------------------------------------------------------- queue positions

/// One order in a top-of-book queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueEntry {
    /// Order ID.
    pub order_id: OrderId,
    /// Resting size.
    pub size: Size,
    /// Total size of the orders ahead of it at this price.
    pub size_ahead: Size,
    /// Placement time.
    pub ts_order_ms: UnixMillis,
}

/// The queue at one best price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueLevel {
    /// Price.
    pub price: Price,
    /// Total resting size.
    pub total_size: Size,
    /// Number of orders.
    pub order_count: u32,
    /// The first orders, in queue order.
    pub orders: Vec<QueueEntry>,
    /// True if orders were left out (more than `max_orders`).
    pub truncated: bool,
}

/// Top-of-book queues after a block (`queue-position@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuePositions {
    /// End of the block (`seq` is an end-of-block marker).
    pub prov: Provenance,
    /// `queue-position@1`; inputs are the book events since the previous
    /// record.
    pub derived: DerivedHeader,
    /// Best bid queue.
    pub bid: Option<QueueLevel>,
    /// Best ask queue.
    pub ask: Option<QueueLevel>,
}

impl QueuePositions {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails if `seq` is not an end-of-block marker, the book is crossed or
    /// a level lists more orders than it has.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::QueuePositions;
        ensure(self.prov.seq.is_end_of_block(), k, || {
            "seq must be an end-of-block marker".into()
        })?;
        for l in [&self.bid, &self.ask].into_iter().flatten() {
            ensure(l.orders.len() <= l.order_count as usize, k, || {
                "more orders listed than the level has".into()
            })?;
        }
        if let (Some(b), Some(a)) = (&self.bid, &self.ask) {
            ensure(b.price < a.price, k, || "book is crossed or locked".into())?;
        }
        Ok(())
    }
}

// --------------------------------------------------------------------- sweep

/// An aggressive run across several price levels (`sweep@1`).
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

impl Sweep {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on an unknown side, fewer than two levels, or more levels than
    /// trades.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::Sweep;
        ensure(self.side != AggressorSide::Unknown, k, || {
            "a sweep has a known side".into()
        })?;
        ensure(
            self.levels >= 2 && self.levels <= self.trade_count,
            k,
            || "levels must be at least 2 and at most the trade count".into(),
        )?;
        ensure(self.first_seq <= self.prov.seq, k, || {
            "first trade is after the last".into()
        })
    }
}

// ------------------------------------------------------------------- heatmap

/// One non-empty heatmap cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatCell {
    /// Column within the tile.
    pub col: u32,
    /// Row within the tile.
    pub row: u32,
    /// Resting bid size in the cell.
    pub bid_size: Size,
    /// Resting ask size in the cell.
    pub ask_size: Size,
}

/// A tile of resting size (`heatmap@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatmapTile {
    /// End of the last block applied before the tile was emitted.
    pub prov: Provenance,
    /// `heatmap@1`; inputs are the book events of the tile's blocks.
    pub derived: DerivedHeader,
    /// Start of column 0.
    pub time_start_ms: UnixMillis,
    /// Column width, ms.
    pub time_step_ms: i64,
    /// Columns in the tile.
    pub cols: u32,
    /// Lower bound of row 0.
    pub price_start: Price,
    /// Row height.
    pub price_step: Price,
    /// Rows in the tile.
    pub rows: u32,
    /// Non-empty cells, by column then row.
    pub cells: Vec<HeatCell>,
}

impl HeatmapTile {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on non-positive steps, cells outside the tile or cells out of
    /// order.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::HeatmapTile;
        ensure(
            self.time_step_ms > 0 && self.price_step.mantissa() > 0,
            k,
            || "steps must be positive".into(),
        )?;
        ensure(
            self.cells
                .iter()
                .all(|c| c.col < self.cols && c.row < self.rows),
            k,
            || "cell outside the tile".into(),
        )?;
        ensure(
            self.cells
                .windows(2)
                .all(|w| (w[0].col, w[0].row) < (w[1].col, w[1].row)),
            k,
            || "cells must be ordered by column then row".into(),
        )
    }
}

// --------------------------------------------------------------- liquidity

/// A liquidity grade, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Grade {
    /// Deep, tight and active.
    A,
    /// Usable as a benchmark.
    B,
    /// Thin.
    C,
    /// Below every threshold.
    D,
}

/// Whether a market may serve as a price reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiquidityLabel {
    /// Liquid enough to serve as a reference price.
    Benchmark,
    /// A record of trades only; not a reference price.
    Tape,
}

/// A market's liquidity grade at one time (`liquidity-grade@1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiquidityGrade {
    /// End of the last block before `as_of_ms`.
    pub prov: Provenance,
    /// `liquidity-grade@1`; inputs are the trades and book events since
    /// the previous grade.
    pub derived: DerivedHeader,
    /// The grading time (a multiple of `emit_ms`).
    pub as_of_ms: UnixMillis,
    /// Window length used, ms.
    pub window_ms: i64,
    /// Book samples in the window.
    pub samples: u32,
    /// Median spread, basis points with two decimals.
    pub spread_bps: Option<Size>,
    /// Depth band, basis points.
    pub depth_bps: u32,
    /// Median bid depth, quote units.
    pub depth_bid: Size,
    /// Median ask depth, quote units.
    pub depth_ask: Size,
    /// Trades in the window.
    pub trades: u64,
    /// Trades per minute, two decimals.
    pub trades_per_minute: Size,
    /// The grade.
    pub grade: Grade,
    /// Benchmark or tape.
    pub label: LiquidityLabel,
}

impl LiquidityGrade {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on a non-positive window.
    pub fn validate(&self) -> Result<(), ValidationError> {
        ensure(self.window_ms > 0, RecordKind::LiquidityGrade, || {
            "window must be positive".into()
        })
    }
}

// ------------------------------------------------------------------ coverage

/// What the recorder observed of a venue over one interval, so a quiet
/// market (blocks observed, no records) and a recording gap (blocks
/// missing or flagged) can be told apart, live and in the archive.
///
/// Coverage is venue-wide: `prov.instrument` is
/// [`InstrumentId::venue_wide`](crate::InstrumentId::venue_wide)
/// (`<venue>:venue:all`), because the recorder sees whole blocks. A market
/// that is registered and has no records in an interval with
/// `missing_blocks == 0` and `gap_blocks == 0` was quiet, not unrecorded.
///
/// `prov.seq` is `(B, 0)` where `B` is the block in which the interval was
/// closed and the record published (the first block after the interval,
/// or `last_block` when the input ended); `prov.ts_source_ms` is the last
/// observed block's time and `prov.ts_ingest_ns` the same instant in
/// nanoseconds, so the record is a pure function of the raw files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    /// Provenance (see the type docs).
    pub prov: Provenance,
    /// Interval length.
    pub interval: BarInterval,
    /// Interval start (inclusive), aligned; intervals are by block time.
    pub start_ms: UnixMillis,
    /// First block observed in the interval.
    pub first_block: u64,
    /// Last block observed in the interval.
    pub last_block: u64,
    /// Blocks observed (published by the reader) in the interval.
    pub blocks: u64,
    /// Block numbers between the previous observed block and `last_block`
    /// that were never observed (including any hole just before
    /// `first_block`).
    pub missing_blocks: u64,
    /// Observed blocks flagged `gap_before` (data skipped before or inside
    /// the block, for example an unreadable line or a rejected block).
    pub gap_blocks: u64,
}

impl Coverage {
    /// Checks consistency.
    ///
    /// # Errors
    ///
    /// Fails on a misaligned start, an empty interval, block counts that do
    /// not fit the block range, or a `seq` before `last_block` or with a
    /// non-zero index.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::Coverage;
        ensure(
            self.interval.align(self.start_ms) == self.start_ms,
            k,
            || "start is not aligned to the interval".into(),
        )?;
        ensure(
            self.blocks > 0 && self.first_block <= self.last_block,
            k,
            || "coverage has at least one block".into(),
        )?;
        let span = self.last_block - self.first_block + 1;
        ensure(self.blocks <= span, k, || {
            "more blocks than the block range holds".into()
        })?;
        ensure(self.gap_blocks <= self.blocks, k, || {
            "more gap blocks than blocks".into()
        })?;
        ensure(
            self.prov.seq.block_number >= self.last_block && self.prov.seq.event_index == 0,
            k,
            || "seq must be (B, 0) with B at or after last_block".into(),
        )?;
        let ts = self.prov.ts_source_ms.0;
        ensure(
            ts >= self.start_ms.0 && ts < self.start_ms.0 + self.interval.millis(),
            k,
            || "last block time is outside the interval".into(),
        )
    }

    /// True if every block of the interval was observed without a gap.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.missing_blocks == 0 && self.gap_blocks == 0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::record::tests::{px, sz};
    use crate::{ContentHash, InstrumentId, MethodId, Record, Sha256Digest, VenueId};

    fn header(method: &str, count: u64) -> DerivedHeader {
        DerivedHeader {
            method: MethodId::new(method).unwrap(),
            inputs_hash: ContentHash(Sha256Digest([9; 32])),
            input_count: count,
        }
    }

    const T0: i64 = 1_767_225_600_000;

    fn activity(spec: ActivitySpec) -> ActivityBar {
        ActivityBar {
            prov: sample("hyperliquid:perp:BTC", 12, 3),
            derived: header(spec.method(), 2),
            spec,
            session_start_ms: UnixMillis(T0),
            first_seq: Seq::new(11, 0),
            first_ms: UnixMillis(T0 - 500),
            open: px("1000.0"),
            high: px("1001.0"),
            low: px("999.0"),
            close: px("1000.5"),
            volume: sz("2.0"),
            buy_volume: sz("1.5"),
            sell_volume: sz("0.5"),
            trade_count: 2,
            complete: true,
        }
    }

    fn queue_level(price: &str, first_id: u64) -> QueueLevel {
        QueueLevel {
            price: px(price),
            total_size: sz("3.0"),
            order_count: 3,
            orders: vec![
                QueueEntry {
                    order_id: OrderId(first_id),
                    size: sz("1.0"),
                    size_ahead: sz("0.0"),
                    ts_order_ms: UnixMillis(T0 - 10),
                },
                QueueEntry {
                    order_id: OrderId(first_id + 1),
                    size: sz("2.0"),
                    size_ahead: sz("1.0"),
                    ts_order_ms: UnixMillis(T0 - 5),
                },
            ],
            truncated: true,
        }
    }

    /// One valid record of every analytics kind and coverage, using every
    /// optional field.
    pub(crate) fn samples() -> Vec<Record> {
        let mut eob = sample("hyperliquid:perp:BTC", 12, 0);
        eob.seq = Seq::end_of_block(12);
        let mut cov_prov = sample("hyperliquid:perp:BTC", 120, 0);
        cov_prov.instrument = InstrumentId::venue_wide(&VenueId::new("hyperliquid").unwrap());
        vec![
            activity(ActivitySpec::Tick { trades: 2 }).into(),
            activity(ActivitySpec::Volume { size: sz("2.0") }).into(),
            activity(ActivitySpec::Range { width: px("2.0") }).into(),
            DeltaBar {
                prov: sample("hyperliquid:perp:BTC", 12, 3),
                derived: header("delta@1", 4),
                interval: BarInterval::M1,
                start_ms: UnixMillis(T0),
                buy_volume: sz("0.5"),
                sell_volume: sz("1.5"),
                volume_delta: SignedSize::new(-10, 1).unwrap(),
                anchor_start_ms: UnixMillis(T0),
                cvd: SignedSize::new(-25, 1).unwrap(),
            }
            .into(),
            ProfileSummary {
                prov: sample("hyperliquid:perp:BTC", 12, 3),
                derived: header("value-area@1", 4),
                start_ms: UnixMillis(T0),
                end_ms: UnixMillis(T0 + 86_400_000),
                bucket: px("0.5"),
                value_area_bps: 7_000,
                poc: px("1000.0"),
                value_area_low: px("999.5"),
                value_area_high: px("1000.5"),
                value_area_volume: sz("3.0"),
                total_volume: sz("4.0"),
            }
            .into(),
            OrderActivity {
                prov: sample("hyperliquid:perp:BTC", 12, 7),
                derived: header("order-activity@1", 9),
                interval: BarInterval::M1,
                start_ms: UnixMillis(T0),
                adds: 4,
                modifies: 1,
                cancels: 2,
                partial_fills: 1,
                full_fills: 1,
                added_size: sz("4.0"),
                cancelled_size: sz("2.0"),
                filled_size: sz("1.5"),
                cancel_to_add_bps: Some(5_000),
                cancel_replace_to_fill_bps: Some(15_000),
                lifetimes: LifetimeStats {
                    removed: 3,
                    filled: 1,
                    cancelled: 2,
                    fleeting_cancels: 1,
                    min_ms: Some(200),
                    p50_ms: Some(900),
                    p90_ms: Some(4_000),
                    max_ms: Some(4_000),
                },
            }
            .into(),
            QueuePositions {
                prov: eob.clone(),
                derived: header("queue-position@1", 3),
                bid: Some(queue_level("999.5", 1)),
                ask: Some(queue_level("1000.5", 10)),
            }
            .into(),
            Sweep {
                prov: sample("hyperliquid:perp:BTC", 12, 3),
                derived: header("sweep@1", 3),
                side: AggressorSide::Buy,
                first_seq: Seq::new(12, 1),
                first_price: px("1000.0"),
                last_price: px("1001.0"),
                levels: 2,
                size: sz("2.5"),
                trade_count: 3,
            }
            .into(),
            HeatmapTile {
                prov: eob.clone(),
                derived: header("heatmap@1", 5),
                time_start_ms: UnixMillis(T0),
                time_step_ms: 1_000,
                cols: 60,
                price_start: px("990.0"),
                price_step: px("0.5"),
                rows: 64,
                cells: vec![
                    HeatCell {
                        col: 0,
                        row: 3,
                        bid_size: sz("1.0"),
                        ask_size: sz("0.0"),
                    },
                    HeatCell {
                        col: 1,
                        row: 0,
                        bid_size: sz("0.0"),
                        ask_size: sz("2.0"),
                    },
                ],
            }
            .into(),
            LiquidityGrade {
                prov: eob,
                derived: header("liquidity-grade@1", 6),
                as_of_ms: UnixMillis(T0),
                window_ms: 300_000,
                samples: 300,
                spread_bps: Some(sz("4.99")),
                depth_bps: 50,
                depth_bid: sz("650000"),
                depth_ask: sz("700000"),
                trades: 120,
                trades_per_minute: sz("24.00"),
                grade: Grade::A,
                label: LiquidityLabel::Benchmark,
            }
            .into(),
            Coverage {
                prov: {
                    let mut p = cov_prov;
                    p.ts_source_ms = UnixMillis(T0 + 59_900);
                    p
                },
                interval: BarInterval::M1,
                start_ms: UnixMillis(T0),
                first_block: 100,
                last_block: 120,
                blocks: 19,
                missing_blocks: 2,
                gap_blocks: 1,
            }
            .into(),
        ]
    }

    #[test]
    fn samples_validate_and_round_trip() {
        for r in samples() {
            r.validate().unwrap_or_else(|e| panic!("{e}"));
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), r, "{json}");
            assert_eq!(r.derived().is_some(), r.kind().is_derived());
            assert!(r.kind().is_analytics());
        }
    }

    #[test]
    fn invalid_samples_fail() {
        let mut bad = samples();
        let Record::DeltaBar(d) = &mut bad[3] else {
            panic!("order")
        };
        d.volume_delta = SignedSize::new(5, 1).unwrap();
        assert!(d.validate().is_err());
        let Record::Coverage(c) = &mut bad[10] else {
            panic!("order")
        };
        c.blocks = 50;
        assert!(c.validate().is_err());
        c.blocks = 19;
        c.prov.seq = Seq::new(119, 0);
        assert!(c.validate().is_err());
        c.prov.seq = Seq::new(121, 0);
        assert!(c.validate().is_ok());
        c.prov.seq = Seq::new(121, 1);
        assert!(c.validate().is_err());
        let Record::Sweep(s) = &mut bad[7] else {
            panic!("order")
        };
        s.side = AggressorSide::Unknown;
        assert!(s.validate().is_err());
    }

    #[test]
    fn signed_size_text_form() {
        let s = SignedSize::new(-1250, 3).unwrap();
        assert_eq!(s.to_string(), "-1.250");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""-1.250""#);
        assert_eq!(serde_json::from_str::<SignedSize>(&json).unwrap(), s);
        let a = Size::parse_exact("1.0").unwrap();
        let b = Size::parse_exact("2.5").unwrap();
        assert_eq!(SignedSize::difference(a, b).unwrap().to_string(), "-1.5");
        assert!(SignedSize::new(1, 19).is_err());
    }

    #[test]
    fn activity_spec_methods() {
        assert_eq!(ActivitySpec::Tick { trades: 3 }.method(), "tick-bars@1");
        assert!(!ActivitySpec::Tick { trades: 0 }.is_valid());
        let json = serde_json::to_string(&ActivitySpec::Tick { trades: 3 }).unwrap();
        assert_eq!(json, r#"{"type":"tick","trades":3}"#);
    }
}
