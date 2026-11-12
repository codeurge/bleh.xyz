//! Record kinds.
//!
//! | Kind | Type | Source |
//! |---|---|---|
//! | `trade` | [`Trade`] | Venue fills, one record per trade |
//! | `book_event` | [`BookEvent`] | Order-level book changes |
//! | `book_snapshot` | [`BookSnapshot`] | L4 order list or L2 levels at a block boundary |
//! | `bar` | [`Bar`] | Derived from trades |
//! | `footprint` | [`crate::Footprint`] | Derived from trades (stub) |
//! | `volume_profile` | [`crate::VolumeProfile`] | Derived from trades (stub) |
//! | `swap_print` | [`crate::SwapPrint`] | Part 43 / SBSR public dissemination |
//!
//! The [`Record`] enum is the tagged union of all kinds. In JSON it is an
//! object with a `kind` field, a `prov` object ([`Provenance`]) and the
//! kind's own fields. Every struct rejects unknown fields.
//!
//! There are deliberately no fields for oracle, mark, external or funding
//! prices, liquidation details, owner addresses, transaction hashes, fees
//! or miscellaneous venue events: a reader has nowhere to put them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::decimal::{Price, Size};
use crate::derived::{DerivedHeader, Footprint, VolumeProfile};
use crate::ids::{OrderId, TradeId};
use crate::provenance::Provenance;
use crate::swap::SwapPrint;
use crate::time::UnixMillis;

/// Error from validating a record's internal consistency.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} record: {reason}")]
pub struct ValidationError {
    /// Kind of the offending record.
    pub kind: RecordKind,
    /// What is wrong.
    pub reason: String,
}

impl ValidationError {
    pub(crate) fn new(kind: RecordKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }
}

pub(crate) fn ensure(
    cond: bool,
    kind: RecordKind,
    reason: impl FnOnce() -> String,
) -> Result<(), ValidationError> {
    if cond {
        Ok(())
    } else {
        Err(ValidationError::new(kind, reason()))
    }
}

/// Aggressor (taker) side of a trade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggressorSide {
    /// The taker bought (lifted an offer).
    Buy,
    /// The taker sold (hit a bid).
    Sell,
    /// The source does not identify the taker.
    Unknown,
}

impl AggressorSide {
    /// Text form used in JSON and Arrow.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
            Self::Unknown => "unknown",
        }
    }
}

/// Side of the book an order rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookSide {
    /// Buy orders.
    Bid,
    /// Sell orders.
    Ask,
}

impl BookSide {
    /// Text form used in JSON and Arrow.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bid => "bid",
            Self::Ask => "ask",
        }
    }
}

/// One trade (execution) on a venue.
///
/// Hyperliquid writes one fill per side; the reader emits one `Trade` per
/// pair, with the aggressor taken from the fill marked `crossed = true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trade {
    /// Provenance. `ts_source_ms` is the venue's trade time.
    pub prov: Provenance,
    /// Execution price.
    pub price: Price,
    /// Executed size (positive).
    pub size: Size,
    /// Taker side.
    pub aggressor: AggressorSide,
    /// Venue trade ID. Unique together with `ts_source_ms` and instrument.
    pub trade_id: TradeId,
}

impl Trade {
    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Fails if the size is not positive.
    pub fn validate(&self) -> Result<(), ValidationError> {
        ensure(self.size.is_positive(), RecordKind::Trade, || {
            "size must be positive".into()
        })
    }
}

/// What happened to a resting order.
///
/// `orig_size` is the size before the change when the source reports it
/// (Hyperliquid raw book diffs do for updates). A removal does not carry the
/// prior size; the book holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BookAction {
    /// A new resting order.
    Add {
        /// Resting size (positive).
        size: Size,
    },
    /// A size change not caused by a fill.
    Modify {
        /// Size before the change, if reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orig_size: Option<Size>,
        /// Size after the change (positive).
        new_size: Size,
    },
    /// A fill against the resting order. `new_size` zero means fully
    /// filled and removed.
    Fill {
        /// Size before the fill, if reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        orig_size: Option<Size>,
        /// Size remaining after the fill.
        new_size: Size,
    },
    /// Removal without a fill (cancel, expiry, venue-side cancel). A struct
    /// variant so that unknown fields are rejected like everywhere else.
    Cancel {},
}

impl BookAction {
    /// Text form of the action type, used in Arrow.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Add { .. } => "add",
            Self::Modify { .. } => "modify",
            Self::Fill { .. } => "fill",
            Self::Cancel {} => "cancel",
        }
    }

    /// The resting size after the action (zero for a removal).
    #[must_use]
    pub fn size_after(self) -> Option<Size> {
        match self {
            Self::Add { size } => Some(size),
            Self::Modify { new_size, .. } | Self::Fill { new_size, .. } => Some(new_size),
            Self::Cancel {} => None,
        }
    }
}

/// One order-level book change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookEvent {
    /// Provenance. `ts_source_ms` is the block time.
    pub prov: Provenance,
    /// Venue order ID.
    pub order_id: OrderId,
    /// Book side.
    pub side: BookSide,
    /// Limit price of the order.
    pub price: Price,
    /// The change.
    pub action: BookAction,
}

impl BookEvent {
    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Fails on non-positive resting sizes, a fill that does not reduce the
    /// size, a modify to zero, or mixed size scales.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::BookEvent;
        match self.action {
            BookAction::Add { size } => {
                ensure(size.is_positive(), k, || "add size must be positive".into())
            }
            BookAction::Modify {
                orig_size,
                new_size,
            } => {
                ensure(new_size.is_positive(), k, || {
                    "modify to zero must be a cancel".into()
                })?;
                check_orig(orig_size, new_size, k, false)
            }
            BookAction::Fill {
                orig_size,
                new_size,
            } => check_orig(orig_size, new_size, k, true),
            BookAction::Cancel {} => Ok(()),
        }
    }
}

fn check_orig(
    orig: Option<Size>,
    new: Size,
    k: RecordKind,
    must_decrease: bool,
) -> Result<(), ValidationError> {
    if let Some(orig) = orig {
        ensure(orig.scale() == new.scale(), k, || {
            "orig_size and new_size scales differ".into()
        })?;
        if must_decrease {
            ensure(new < orig, k, || {
                "a fill must reduce the resting size".into()
            })?;
        }
    }
    Ok(())
}

/// One resting order in an L4 snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestingOrder {
    /// Venue order ID.
    pub order_id: OrderId,
    /// Limit price.
    pub price: Price,
    /// Resting size (positive).
    pub size: Size,
    /// Order placement time, milliseconds.
    pub ts_order_ms: UnixMillis,
}

/// One aggregated price level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    /// Level price.
    pub price: Price,
    /// Total resting size (positive).
    pub size: Size,
    /// Number of orders at the level.
    pub order_count: u32,
}

/// Book contents in a snapshot.
///
/// Bids are ordered best (highest) first, asks best (lowest) first. In L4,
/// orders at the same price are in queue (time) priority order. Trigger
/// orders are not resting and never appear.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotDepth {
    /// Every resting order.
    L4 {
        /// Bid orders.
        bids: Vec<RestingOrder>,
        /// Ask orders.
        asks: Vec<RestingOrder>,
    },
    /// Aggregated levels.
    L2 {
        /// Bid levels.
        bids: Vec<Level>,
        /// Ask levels.
        asks: Vec<Level>,
    },
}

impl SnapshotDepth {
    /// Aggregates to L2. An L2 snapshot is returned unchanged.
    ///
    /// # Errors
    ///
    /// Fails if a level's total size overflows or scales are mixed.
    pub fn to_l2(&self) -> Result<SnapshotDepth, crate::DecimalError> {
        match self {
            SnapshotDepth::L2 { .. } => Ok(self.clone()),
            SnapshotDepth::L4 { bids, asks } => Ok(SnapshotDepth::L2 {
                bids: aggregate(bids)?,
                asks: aggregate(asks)?,
            }),
        }
    }

    /// Best bid and best ask prices, if present.
    #[must_use]
    pub fn top(&self) -> (Option<Price>, Option<Price>) {
        match self {
            SnapshotDepth::L4 { bids, asks } => {
                (bids.first().map(|o| o.price), asks.first().map(|o| o.price))
            }
            SnapshotDepth::L2 { bids, asks } => {
                (bids.first().map(|l| l.price), asks.first().map(|l| l.price))
            }
        }
    }
}

fn aggregate(orders: &[RestingOrder]) -> Result<Vec<Level>, crate::DecimalError> {
    let mut levels: Vec<Level> = Vec::new();
    for o in orders {
        match levels.last_mut() {
            Some(l) if l.price == o.price => {
                l.size = l.size.checked_add(o.size)?;
                l.order_count = l.order_count.saturating_add(1);
            }
            _ => levels.push(Level {
                price: o.price,
                size: o.size,
                order_count: 1,
            }),
        }
    }
    Ok(levels)
}

/// The book of one instrument after all events of a block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookSnapshot {
    /// Provenance. `seq` is [`Seq::end_of_block`](crate::Seq::end_of_block)
    /// of the block the snapshot reflects; deltas with a greater `seq`
    /// apply on top of it.
    pub prov: Provenance,
    /// Contents.
    pub depth: SnapshotDepth,
}

impl BookSnapshot {
    /// Checks ordering and consistency.
    ///
    /// # Errors
    ///
    /// Fails if `seq` is not an end-of-block marker, sides are mis-ordered,
    /// sizes are not positive, L2 prices repeat, an order ID repeats, or the
    /// book is crossed.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::BookSnapshot;
        ensure(self.prov.seq.is_end_of_block(), k, || {
            "snapshot seq must be an end-of-block marker".into()
        })?;
        match &self.depth {
            SnapshotDepth::L4 { bids, asks } => {
                let mut ids = std::collections::HashSet::new();
                for (side, orders) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
                    for w in orders.windows(2) {
                        ensure(ordered(side, w[0].price, w[1].price, true), k, || {
                            format!("{} orders out of price order", side.as_str())
                        })?;
                    }
                    for o in orders {
                        ensure(o.size.is_positive(), k, || {
                            "resting size must be positive".into()
                        })?;
                        ensure(ids.insert(o.order_id), k, || {
                            format!("order id {} appears twice", o.order_id.0)
                        })?;
                    }
                }
            }
            SnapshotDepth::L2 { bids, asks } => {
                for (side, levels) in [(BookSide::Bid, bids), (BookSide::Ask, asks)] {
                    for w in levels.windows(2) {
                        ensure(ordered(side, w[0].price, w[1].price, false), k, || {
                            format!("{} levels out of strict price order", side.as_str())
                        })?;
                    }
                    for l in levels {
                        ensure(l.size.is_positive() && l.order_count > 0, k, || {
                            "level size and order count must be positive".into()
                        })?;
                    }
                }
            }
        }
        if let (Some(bid), Some(ask)) = self.depth.top() {
            ensure(bid < ask, k, || "book is crossed or locked".into())?;
        }
        Ok(())
    }
}

/// True if `b` may follow `a` on `side`.
fn ordered(side: BookSide, a: Price, b: Price, allow_equal: bool) -> bool {
    match (side, allow_equal) {
        (BookSide::Bid, true) => a >= b,
        (BookSide::Bid, false) => a > b,
        (BookSide::Ask, true) => a <= b,
        (BookSide::Ask, false) => a < b,
    }
}

/// Bar interval. Bars start at multiples of the interval since the Unix
/// epoch (UTC), so a daily bar starts at UTC midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BarInterval {
    /// One second.
    #[serde(rename = "1s")]
    S1,
    /// One minute.
    #[serde(rename = "1m")]
    M1,
    /// Five minutes.
    #[serde(rename = "5m")]
    M5,
    /// Fifteen minutes.
    #[serde(rename = "15m")]
    M15,
    /// One hour.
    #[serde(rename = "1h")]
    H1,
    /// One day (UTC).
    #[serde(rename = "1d")]
    D1,
}

impl BarInterval {
    /// Every interval, shortest first.
    pub const ALL: [BarInterval; 6] = [
        BarInterval::S1,
        BarInterval::M1,
        BarInterval::M5,
        BarInterval::M15,
        BarInterval::H1,
        BarInterval::D1,
    ];

    /// Length in milliseconds.
    #[must_use]
    pub const fn millis(self) -> i64 {
        match self {
            Self::S1 => 1_000,
            Self::M1 => 60_000,
            Self::M5 => 300_000,
            Self::M15 => 900_000,
            Self::H1 => 3_600_000,
            Self::D1 => 86_400_000,
        }
    }

    /// Text form (`1s`, `1m`, ...).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S1 => "1s",
            Self::M1 => "1m",
            Self::M5 => "5m",
            Self::M15 => "15m",
            Self::H1 => "1h",
            Self::D1 => "1d",
        }
    }

    /// Start of the interval containing `ts`.
    #[must_use]
    pub const fn align(self, ts: UnixMillis) -> UnixMillis {
        UnixMillis(ts.0.div_euclid(self.millis()) * self.millis())
    }
}

impl fmt::Display for BarInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An OHLCV bar built from trades. No bar is emitted for an interval with
/// no trades.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bar {
    /// Provenance. `seq` and `ts_source_ms` are those of the last trade in
    /// the bar.
    pub prov: Provenance,
    /// Method and inputs hash (the trades, in canonical order).
    pub derived: DerivedHeader,
    /// Interval length.
    pub interval: BarInterval,
    /// Interval start (inclusive), aligned to the interval.
    pub start_ms: UnixMillis,
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
    /// Size traded with a buy aggressor.
    pub buy_volume: Size,
    /// Size traded with a sell aggressor.
    pub sell_volume: Size,
    /// Number of trades (at least one).
    pub trade_count: u32,
}

impl Bar {
    /// Checks OHLC and volume consistency.
    ///
    /// # Errors
    ///
    /// Fails on an empty bar, a misaligned start, a last trade outside the
    /// interval, inconsistent OHLC, mixed scales, or buy plus sell volume
    /// exceeding the total.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let k = RecordKind::Bar;
        ensure(self.trade_count > 0, k, || "bars are never empty".into())?;
        ensure(
            self.interval.align(self.start_ms) == self.start_ms,
            k,
            || "start is not aligned to the interval".into(),
        )?;
        let ts = self.prov.ts_source_ms.0;
        ensure(
            ts >= self.start_ms.0 && ts < self.start_ms.0 + self.interval.millis(),
            k,
            || "last trade time is outside the interval".into(),
        )?;
        let prices = [self.open, self.high, self.low, self.close];
        ensure(
            prices.iter().all(|p| p.scale() == self.open.scale()),
            k,
            || "price scales differ".into(),
        )?;
        ensure(
            self.low <= self.open.min(self.close) && self.high >= self.open.max(self.close),
            k,
            || "OHLC is inconsistent".into(),
        )?;
        let sides = self
            .buy_volume
            .checked_add(self.sell_volume)
            .map_err(|e| ValidationError::new(k, e.to_string()))?;
        ensure(
            sides <= self.volume && sides.scale() == self.volume.scale(),
            k,
            || "buy plus sell volume exceeds total volume".into(),
        )?;
        ensure(self.volume.is_positive(), k, || {
            "volume must be positive".into()
        })?;
        ensure(
            self.derived.input_count == u64::from(self.trade_count),
            k,
            || "input_count must equal trade_count".into(),
        )
    }
}

/// Record kind discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// [`Trade`].
    Trade,
    /// [`BookEvent`].
    BookEvent,
    /// [`BookSnapshot`].
    BookSnapshot,
    /// [`Bar`].
    Bar,
    /// [`Footprint`].
    Footprint,
    /// [`VolumeProfile`].
    VolumeProfile,
    /// [`SwapPrint`].
    SwapPrint,
}

impl RecordKind {
    /// Every kind.
    pub const ALL: [RecordKind; 7] = [
        RecordKind::Trade,
        RecordKind::BookEvent,
        RecordKind::BookSnapshot,
        RecordKind::Bar,
        RecordKind::Footprint,
        RecordKind::VolumeProfile,
        RecordKind::SwapPrint,
    ];

    /// The `kind` tag in JSON, and the `kind=` partition value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trade => "trade",
            Self::BookEvent => "book_event",
            Self::BookSnapshot => "book_snapshot",
            Self::Bar => "bar",
            Self::Footprint => "footprint",
            Self::VolumeProfile => "volume_profile",
            Self::SwapPrint => "swap_print",
        }
    }

    /// True for kinds computed by a versioned method from other records.
    #[must_use]
    pub const fn is_derived(self) -> bool {
        matches!(self, Self::Bar | Self::Footprint | Self::VolumeProfile)
    }
}

impl fmt::Display for RecordKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Any record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// A trade.
    Trade(Trade),
    /// A book event.
    BookEvent(BookEvent),
    /// A book snapshot.
    BookSnapshot(BookSnapshot),
    /// A bar.
    Bar(Bar),
    /// A footprint (stub).
    Footprint(Footprint),
    /// A volume profile (stub).
    VolumeProfile(VolumeProfile),
    /// A swap print.
    SwapPrint(SwapPrint),
}

impl Record {
    /// The record's kind.
    #[must_use]
    pub const fn kind(&self) -> RecordKind {
        match self {
            Record::Trade(_) => RecordKind::Trade,
            Record::BookEvent(_) => RecordKind::BookEvent,
            Record::BookSnapshot(_) => RecordKind::BookSnapshot,
            Record::Bar(_) => RecordKind::Bar,
            Record::Footprint(_) => RecordKind::Footprint,
            Record::VolumeProfile(_) => RecordKind::VolumeProfile,
            Record::SwapPrint(_) => RecordKind::SwapPrint,
        }
    }

    /// The record's provenance.
    #[must_use]
    pub const fn prov(&self) -> &Provenance {
        match self {
            Record::Trade(r) => &r.prov,
            Record::BookEvent(r) => &r.prov,
            Record::BookSnapshot(r) => &r.prov,
            Record::Bar(r) => &r.prov,
            Record::Footprint(r) => &r.prov,
            Record::VolumeProfile(r) => &r.prov,
            Record::SwapPrint(r) => &r.prov,
        }
    }

    /// The record's provenance, mutably (for stamping the publish time).
    pub fn prov_mut(&mut self) -> &mut Provenance {
        match self {
            Record::Trade(r) => &mut r.prov,
            Record::BookEvent(r) => &mut r.prov,
            Record::BookSnapshot(r) => &mut r.prov,
            Record::Bar(r) => &mut r.prov,
            Record::Footprint(r) => &mut r.prov,
            Record::VolumeProfile(r) => &mut r.prov,
            Record::SwapPrint(r) => &mut r.prov,
        }
    }

    /// The derived-record header, for derived kinds.
    #[must_use]
    pub const fn derived(&self) -> Option<&DerivedHeader> {
        match self {
            Record::Bar(r) => Some(&r.derived),
            Record::Footprint(r) => Some(&r.derived),
            Record::VolumeProfile(r) => Some(&r.derived),
            _ => None,
        }
    }

    /// Checks internal consistency.
    ///
    /// # Errors
    ///
    /// Returns the kind's validation error.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Record::Trade(r) => r.validate(),
            Record::BookEvent(r) => r.validate(),
            Record::BookSnapshot(r) => r.validate(),
            Record::Bar(r) => r.validate(),
            Record::Footprint(r) => r.validate(),
            Record::VolumeProfile(r) => r.validate(),
            Record::SwapPrint(r) => r.validate(),
        }
    }

    /// Canonical archive order: instrument, then kind, then `seq`.
    #[must_use]
    pub fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        let (a, b) = (self.prov(), other.prov());
        a.instrument
            .cmp(&b.instrument)
            .then(self.kind().cmp(&other.kind()))
            .then(a.seq.cmp(&b.seq))
    }
}

macro_rules! from_variant {
    ($($t:ident),*) => {$(
        impl From<$t> for Record {
            fn from(r: $t) -> Self {
                Record::$t(r)
            }
        }
    )*};
}
from_variant!(
    Trade,
    BookEvent,
    BookSnapshot,
    Bar,
    Footprint,
    VolumeProfile,
    SwapPrint
);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::provenance::tests::sample;
    use crate::{ContentHash, MethodId, Seq, Sha256Digest};

    pub(crate) fn px(s: &str) -> Price {
        Price::parse_exact(s).unwrap()
    }
    pub(crate) fn sz(s: &str) -> Size {
        Size::parse_exact(s).unwrap()
    }

    pub(crate) fn trade(block: u64, index: u32) -> Trade {
        Trade {
            prov: sample("hyperliquid:perp:BTC", block, index),
            price: px("1000.5"),
            size: sz("0.250"),
            aggressor: AggressorSide::Buy,
            trade_id: TradeId(77),
        }
    }

    pub(crate) fn book_event(block: u64, index: u32, action: BookAction) -> BookEvent {
        BookEvent {
            prov: sample("hyperliquid:perp:BTC", block, index),
            order_id: OrderId(10),
            side: BookSide::Bid,
            price: px("999.5"),
            action,
        }
    }

    pub(crate) fn order(id: u64, price: &str, size: &str) -> RestingOrder {
        RestingOrder {
            order_id: OrderId(id),
            price: px(price),
            size: sz(size),
            ts_order_ms: UnixMillis(1_767_225_599_000),
        }
    }

    pub(crate) fn snapshot() -> BookSnapshot {
        let mut prov = sample("hyperliquid:perp:BTC", 9, 0);
        prov.seq = Seq::end_of_block(9);
        BookSnapshot {
            prov,
            depth: SnapshotDepth::L4 {
                bids: vec![
                    order(1, "999.5", "1.000"),
                    order(2, "999.5", "0.500"),
                    order(3, "999.0", "2.000"),
                ],
                asks: vec![order(4, "1000.5", "0.100")],
            },
        }
    }

    pub(crate) fn header(count: u64) -> DerivedHeader {
        DerivedHeader {
            method: MethodId::new("bars@1").unwrap(),
            inputs_hash: ContentHash(Sha256Digest([7; 32])),
            input_count: count,
        }
    }

    pub(crate) fn bar() -> Bar {
        let mut prov = sample("hyperliquid:perp:BTC", 12, 0);
        prov.ts_source_ms = UnixMillis(1_767_225_600_900);
        Bar {
            prov,
            derived: header(3),
            interval: BarInterval::S1,
            start_ms: UnixMillis(1_767_225_600_000),
            open: px("1000.0"),
            high: px("1001.0"),
            low: px("999.0"),
            close: px("1000.5"),
            volume: sz("3.000"),
            buy_volume: sz("2.000"),
            sell_volume: sz("1.000"),
            trade_count: 3,
        }
    }

    #[test]
    fn json_round_trip_all_core_kinds() {
        let records: Vec<Record> = vec![
            trade(1, 0).into(),
            book_event(1, 0, BookAction::Add { size: sz("1.000") }).into(),
            book_event(
                1,
                1,
                BookAction::Modify {
                    orig_size: Some(sz("1.000")),
                    new_size: sz("0.500"),
                },
            )
            .into(),
            book_event(
                1,
                2,
                BookAction::Fill {
                    orig_size: None,
                    new_size: sz("0.000"),
                },
            )
            .into(),
            book_event(1, 3, BookAction::Cancel {}).into(),
            snapshot().into(),
            bar().into(),
        ];
        for r in records {
            r.validate().unwrap();
            let json = serde_json::to_string(&r).unwrap();
            let back: Record = serde_json::from_str(&json).unwrap();
            assert_eq!(back, r, "{json}");
        }
    }

    #[test]
    fn trade_json_shape() {
        let v = serde_json::to_value(Record::from(trade(5, 1))).unwrap();
        assert_eq!(v["kind"], "trade");
        assert_eq!(v["price"], "1000.5");
        assert_eq!(v["size"], "0.250");
        assert_eq!(v["aggressor"], "buy");
        assert_eq!(v["trade_id"], 77);
        assert_eq!(v["prov"]["seq"]["block_number"], 5);
        assert_eq!(v["prov"]["ts_ingest_ns"], "1767225600004000000");
    }

    #[test]
    fn unknown_fields_are_rejected_everywhere() {
        let mut v = serde_json::to_value(Record::from(trade(5, 1))).unwrap();
        v["markPx"] = serde_json::json!("1.0");
        assert!(serde_json::from_value::<Record>(v).is_err());

        let mut v =
            serde_json::to_value(Record::from(book_event(1, 0, BookAction::Cancel {}))).unwrap();
        v["action"]["user"] = serde_json::json!("0x00");
        assert!(serde_json::from_value::<Record>(v).is_err());

        let mut v = serde_json::to_value(Record::from(snapshot())).unwrap();
        v["depth"]["bids"][0]["owner"] = serde_json::json!("0x00");
        assert!(serde_json::from_value::<Record>(v).is_err());

        let v = serde_json::json!({"kind": "liquidation"});
        assert!(serde_json::from_value::<Record>(v).is_err());
    }

    #[test]
    fn book_event_validation() {
        assert!(
            book_event(1, 0, BookAction::Add { size: sz("0") })
                .validate()
                .is_err()
        );
        assert!(
            book_event(
                1,
                0,
                BookAction::Modify {
                    orig_size: None,
                    new_size: sz("0")
                }
            )
            .validate()
            .is_err()
        );
        assert!(
            book_event(
                1,
                0,
                BookAction::Fill {
                    orig_size: Some(sz("1.0")),
                    new_size: sz("1.5")
                }
            )
            .validate()
            .is_err()
        );
        assert!(
            book_event(
                1,
                0,
                BookAction::Fill {
                    orig_size: Some(sz("1.0")),
                    new_size: sz("0.50")
                }
            )
            .validate()
            .is_err()
        );
        assert!(
            book_event(
                1,
                0,
                BookAction::Fill {
                    orig_size: Some(sz("1.0")),
                    new_size: sz("0.5")
                }
            )
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn snapshot_validation_and_l2() {
        let s = snapshot();
        s.validate().unwrap();
        let SnapshotDepth::L2 { bids, asks } = s.depth.to_l2().unwrap() else {
            panic!("expected L2");
        };
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].size, sz("1.500"));
        assert_eq!(bids[0].order_count, 2);
        assert_eq!(asks[0].order_count, 1);

        let mut bad = snapshot();
        bad.prov.seq = Seq::new(9, 0);
        assert!(bad.validate().is_err());

        let mut bad = snapshot();
        if let SnapshotDepth::L4 { bids, .. } = &mut bad.depth {
            bids.swap(0, 2);
        }
        assert!(bad.validate().is_err());

        let mut crossed = snapshot();
        if let SnapshotDepth::L4 { asks, .. } = &mut crossed.depth {
            asks[0].price = px("999.5");
        }
        assert!(crossed.validate().is_err());

        let mut dup = snapshot();
        if let SnapshotDepth::L4 { asks, .. } = &mut dup.depth {
            asks[0].order_id = OrderId(1);
        }
        assert!(dup.validate().is_err());
    }

    #[test]
    fn bar_validation() {
        bar().validate().unwrap();
        let mut b = bar();
        b.trade_count = 0;
        assert!(b.validate().is_err());
        let mut b = bar();
        b.high = px("999.5");
        assert!(b.validate().is_err());
        let mut b = bar();
        b.buy_volume = sz("2.500");
        assert!(b.validate().is_err());
        let mut b = bar();
        b.start_ms = UnixMillis(1_767_225_600_001);
        assert!(b.validate().is_err());
        let mut b = bar();
        b.prov.ts_source_ms = UnixMillis(1_767_225_601_000);
        assert!(b.validate().is_err());
    }

    #[test]
    fn bar_interval_alignment() {
        assert_eq!(
            BarInterval::M1.align(UnixMillis(125_000)),
            UnixMillis(120_000)
        );
        assert_eq!(BarInterval::S1.align(UnixMillis(-1)), UnixMillis(-1_000));
        assert_eq!(
            BarInterval::D1.align(UnixMillis(86_400_001)),
            UnixMillis(86_400_000)
        );
        assert_eq!(serde_json::to_string(&BarInterval::M15).unwrap(), "\"15m\"");
    }

    #[test]
    fn canonical_order() {
        let mut v: Vec<Record> = vec![
            trade(2, 0).into(),
            book_event(1, 1, BookAction::Cancel {}).into(),
            trade(1, 3).into(),
            book_event(1, 0, BookAction::Cancel {}).into(),
        ];
        v.sort_by(Record::canonical_cmp);
        let keys: Vec<_> = v.iter().map(|r| (r.kind(), r.prov().seq)).collect();
        assert_eq!(
            keys,
            vec![
                (RecordKind::Trade, Seq::new(1, 3)),
                (RecordKind::Trade, Seq::new(2, 0)),
                (RecordKind::BookEvent, Seq::new(1, 0)),
                (RecordKind::BookEvent, Seq::new(1, 1)),
            ]
        );
    }
}
