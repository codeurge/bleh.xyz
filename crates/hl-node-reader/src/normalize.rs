//! From one joined block (fills, order statuses and raw book diffs of the
//! same height) to mesh records (spec §§ 4.1, 6.1).
//!
//! Pure: the output depends only on the input lines and the registry. The
//! result is a [`ReaderBlock`]: records grouped per instrument and kind with
//! `seq = (block_number, event_index)` assigned, but without
//! `prev_block_number` links or block-gap flags, which depend on the stream
//! history and are added by [`crate::sequence::Sequencer`] (after the
//! active-active merge, when there are two readers).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use mesh_types::{
    BookAction, BookEvent, Flag, InstrumentId, InstrumentSpec, Price, Provenance, Record,
    RecordKind, Registry, Seq, Size, Trade, UnixMillis, UnixNanos, VenueId,
};

use crate::assumptions::{
    FILLED_STATUS, diff_side, fill_side_aggressor, is_admitted_coin, parse_block_time,
    parse_local_time, venue_decimal,
};
use crate::checkpoint::ReaderPositions;
use crate::fills::{Anomaly, DropReason, GroupClass, group_fills};
use crate::raw::{FillsLine, OrderStatusesLine, RawBookDiff, RawBookDiffsLine};

/// The venue of every record this crate emits.
pub const VENUE: &str = "hyperliquid";

/// [`VENUE`] as a [`VenueId`].
#[must_use]
pub fn venue() -> VenueId {
    VenueId::new(VENUE).unwrap_or_else(|_| unreachable!("the constant venue id is valid"))
}

/// One record stream: an instrument and a kind.
pub type StreamKey = (InstrumentId, RecordKind);

/// The three lines of one block. A line is `None` when its stream skipped
/// the block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinedBlock {
    /// Block height.
    pub block_number: u64,
    /// Fills line.
    pub fills: Option<FillsLine>,
    /// Order-status line.
    pub statuses: Option<OrderStatusesLine>,
    /// Raw-book-diff line.
    pub diffs: Option<RawBookDiffsLine>,
}

impl JoinedBlock {
    /// True if every stream has a line for the block.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.fills.is_some() && self.statuses.is_some() && self.diffs.is_some()
    }

    /// Every distinct coin in the block's events.
    #[must_use]
    pub fn coins(&self) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        if let Some(l) = &self.fills {
            out.extend(l.events.iter().map(|e| e.0.coin.as_str()));
        }
        if let Some(l) = &self.statuses {
            out.extend(l.events.iter().map(|e| e.order.coin.as_str()));
        }
        if let Some(l) = &self.diffs {
            out.extend(l.events.iter().map(|e| e.coin.as_str()));
        }
        out
    }
}

/// Counters from normalizing one block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockStats {
    /// Fill groups dropped as not book trades, by reason.
    pub dropped_pairs: BTreeMap<DropReason, u64>,
    /// Malformed fill groups, by kind.
    pub anomalies: BTreeMap<Anomaly, u64>,
    /// Admitted coins missing from the registry (their events are dropped).
    pub unknown_coins: BTreeSet<String>,
    /// Events of markets outside ADM-0001 (spot), dropped.
    pub unadmitted_events: u64,
}

/// One block's records from one reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderBlock {
    /// Block height.
    pub block_number: u64,
    /// Block time.
    pub block_time_ms: UnixMillis,
    /// Records per stream, `event_index` `0..n` in each.
    pub streams: BTreeMap<StreamKey, Vec<Record>>,
    /// True if a stream had no line for this block, or the block was
    /// rejected: records may be missing, so the sequencer flags a gap.
    pub incomplete: bool,
    /// Why the whole block was rejected, if it was. A rejected block has no
    /// records.
    pub rejected: Option<String>,
    /// Streams whose next record (in a later block) must carry
    /// `gap_before` because a malformed fill group ended this block.
    pub carry_gaps: BTreeSet<StreamKey>,
    /// Counters.
    pub stats: BlockStats,
    /// Where the reader resumes after this block (live tailing only).
    pub resume: Option<ReaderPositions>,
}

impl ReaderBlock {
    /// Total number of records.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.streams.values().map(Vec::len).sum()
    }
}

/// Why a block was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NormalizeError {
    /// A time field has an unexpected format.
    #[error("{field}: {reason}")]
    Time {
        /// Field name.
        field: &'static str,
        /// Parse error.
        reason: String,
    },
    /// The three streams disagree on the block time.
    #[error("streams disagree on block_time for block {0}")]
    BlockTimeMismatch(u64),
    /// A decimal does not fit the instrument's scale.
    #[error("{coin}: {field} {value:?}: {reason}")]
    Decimal {
        /// Market.
        coin: String,
        /// Field name.
        field: &'static str,
        /// The text.
        value: String,
        /// Parse error.
        reason: String,
    },
    /// The two fills of a trade disagree on price or size.
    #[error("{coin} tid {tid}: fills disagree on {field}")]
    PairDisagrees {
        /// Market.
        coin: String,
        /// Trade ID.
        tid: u64,
        /// `price` or `size`.
        field: &'static str,
    },
    /// A side value is not one of the documented ones.
    #[error("{coin}: unknown side {side:?}")]
    Side {
        /// Market.
        coin: String,
        /// The value.
        side: String,
    },
    /// A record failed its own validation.
    #[error("{0}")]
    Invalid(String),
    /// More records in one stream and block than `event_index` can hold.
    #[error("too many events in one block")]
    TooManyEvents,
}

fn parse_price(
    coin: &str,
    field: &'static str,
    s: &str,
    scale: u8,
) -> Result<Price, NormalizeError> {
    Price::parse_with_scale(&venue_decimal(s), scale).map_err(|e| NormalizeError::Decimal {
        coin: coin.to_owned(),
        field,
        value: s.to_owned(),
        reason: e.to_string(),
    })
}

fn parse_size(coin: &str, field: &'static str, s: &str, scale: u8) -> Result<Size, NormalizeError> {
    Size::parse_with_scale(&venue_decimal(s), scale).map_err(|e| NormalizeError::Decimal {
        coin: coin.to_owned(),
        field,
        value: s.to_owned(),
        reason: e.to_string(),
    })
}

fn block_time(block: &JoinedBlock) -> Result<UnixMillis, NormalizeError> {
    let texts = [
        block.fills.as_ref().map(|l| l.block_time.as_str()),
        block.statuses.as_ref().map(|l| l.block_time.as_str()),
        block.diffs.as_ref().map(|l| l.block_time.as_str()),
    ];
    let mut out: Option<UnixMillis> = None;
    for t in texts.into_iter().flatten() {
        let v = parse_block_time(t).map_err(|reason| NormalizeError::Time {
            field: "block_time",
            reason,
        })?;
        match out {
            Some(prev) if prev != v => {
                return Err(NormalizeError::BlockTimeMismatch(block.block_number));
            }
            _ => out = Some(v),
        }
    }
    out.ok_or(NormalizeError::BlockTimeMismatch(block.block_number))
}

fn local_time(s: &str) -> Result<UnixNanos, NormalizeError> {
    parse_local_time(s).map_err(|reason| NormalizeError::Time {
        field: "local_time",
        reason,
    })
}

fn next_index(records: &[Record]) -> Result<u32, NormalizeError> {
    u32::try_from(records.len())
        .ok()
        .filter(|&i| i != u32::MAX)
        .ok_or(NormalizeError::TooManyEvents)
}

/// Classifies one raw book diff (spec § 6.1), refined so that fills and
/// modifies of the same order in the same block stay distinguishable.
///
/// `maker_fills` holds the sizes of the not-yet-matched fills in this block
/// where the order was the resting (maker, `crossed = false`) side of a
/// book trade, in file order. `filled_status` is true if the block has an
/// order status `filled` for the order.
///
/// | Diff | Condition | Action |
/// |---|---|---|
/// | `new {sz}` | | `add {size}` |
/// | `update {a, b}`, `b < a` | next maker fill has size `a - b` (consumed) | `fill {a, b}` |
/// | `update {a, 0}` | otherwise | `cancel` |
/// | `update {a, b}` | otherwise | `modify {a, b}` |
/// | `remove` | a maker fill is pending (consumed) | `fill {new_size: 0}` |
/// | `remove` | status `filled` | `fill {new_size: 0}` |
/// | `remove` | otherwise | `cancel` |
///
/// The spec's rule ("a fill with this `oid` in the same block") is the
/// special case with at most one diff per order per block. Matching the
/// fill size keeps a modify after a partial fill a `modify`, and a cancel
/// after a partial fill a `cancel` (assumption 6). Before classifying, the
/// block's final fill of an order that ends `filled` with a `remove` is
/// reserved for that `remove` (see `reserve_final_fills`), so a modify
/// followed by the fill that removes the order stays a `modify`. Equal size
/// changes of a modify and a later partial fill of the same order in one
/// block remain ambiguous; the earlier diff takes the fill.
///
/// # Errors
///
/// Fails if a size does not fit the scale or a new order has no size.
pub fn classify_diff(
    coin: &str,
    diff: &RawBookDiff,
    size_scale: u8,
    maker_fills: Option<&mut VecDeque<Size>>,
    filled_status: bool,
) -> Result<BookAction, NormalizeError> {
    let zero = || {
        Size::zero(size_scale).map_err(|e| NormalizeError::Decimal {
            coin: coin.to_owned(),
            field: "size",
            value: String::new(),
            reason: e.to_string(),
        })
    };
    match diff {
        RawBookDiff::New { sz } => Ok(BookAction::Add {
            size: parse_size(coin, "new.sz", sz, size_scale)?,
        }),
        RawBookDiff::Update { orig_sz, new_sz } => {
            let a = parse_size(coin, "update.origSz", orig_sz, size_scale)?;
            let b = parse_size(coin, "update.newSz", new_sz, size_scale)?;
            if b < a {
                let delta = a
                    .checked_sub(b)
                    .map_err(|e| NormalizeError::Invalid(e.to_string()))?;
                if let Some(q) = maker_fills
                    && q.front() == Some(&delta)
                {
                    q.pop_front();
                    return Ok(BookAction::Fill {
                        orig_size: Some(a),
                        new_size: b,
                    });
                }
                if b.is_zero() {
                    return Ok(BookAction::Cancel {});
                }
            }
            Ok(BookAction::Modify {
                orig_size: Some(a),
                new_size: b,
            })
        }
        RawBookDiff::Remove => {
            let pending = maker_fills.and_then(VecDeque::pop_front).is_some();
            if pending || filled_status {
                Ok(BookAction::Fill {
                    orig_size: None,
                    new_size: zero()?,
                })
            } else {
                Ok(BookAction::Cancel {})
            }
        }
    }
}

/// An order that ends the block filled (status `filled`) and whose last
/// diff is `remove` was removed by its final fill, so that fill is taken
/// out of the queue that `update` diffs match against: a modify earlier in
/// the block with the same size change is then not mistaken for the fill.
/// The `remove` still classifies as a fill through the `filled` status.
fn reserve_final_fills<'a>(
    diffs: &'a [crate::raw::BookDiffEvent],
    filled: &HashSet<(&str, u64)>,
    maker_fills: &mut HashMap<(&'a str, u64), VecDeque<Size>>,
) {
    let mut last: HashMap<(&str, u64), bool> = HashMap::new();
    for ev in diffs {
        last.insert(
            (ev.coin.as_str(), ev.oid),
            matches!(ev.raw_book_diff, RawBookDiff::Remove),
        );
    }
    for (key, is_remove) in last {
        if is_remove
            && filled.contains(&key)
            && let Some(q) = maker_fills.get_mut(&key)
        {
            q.pop_back();
        }
    }
}

/// Normalizes one joined block. Never fails: a block that cannot be
/// normalized is returned with `rejected` set and no records.
#[must_use]
pub fn normalize(block: &JoinedBlock, registry: &Registry) -> ReaderBlock {
    let mut stats = BlockStats::default();
    let mut carry_gaps = BTreeSet::new();
    let time = block_time(block);
    let result = time
        .clone()
        .and_then(|t| build(block, registry, t, &mut stats, &mut carry_gaps));
    let block_time_ms = time.unwrap_or(UnixMillis(0));
    match result {
        Ok(streams) => ReaderBlock {
            block_number: block.block_number,
            block_time_ms,
            streams,
            incomplete: !block.is_complete(),
            rejected: None,
            carry_gaps,
            stats,
            resume: None,
        },
        Err(e) => ReaderBlock {
            block_number: block.block_number,
            block_time_ms,
            streams: BTreeMap::new(),
            incomplete: true,
            rejected: Some(e.to_string()),
            carry_gaps: BTreeSet::new(),
            stats,
            resume: None,
        },
    }
}

fn lookup<'r>(
    registry: &'r Registry,
    venue: &VenueId,
    coin: &str,
    stats: &mut BlockStats,
) -> Option<&'r InstrumentSpec> {
    if !is_admitted_coin(coin) {
        stats.unadmitted_events += 1;
        return None;
    }
    let spec = registry.by_venue_symbol(venue, coin);
    if spec.is_none() {
        stats.unknown_coins.insert(coin.to_owned());
    }
    spec
}

fn build(
    block: &JoinedBlock,
    registry: &Registry,
    block_time_ms: UnixMillis,
    stats: &mut BlockStats,
    carry_gaps: &mut BTreeSet<StreamKey>,
) -> Result<BTreeMap<StreamKey, Vec<Record>>, NormalizeError> {
    let venue = venue();
    let n = block.block_number;
    let mut streams: BTreeMap<StreamKey, Vec<Record>> = BTreeMap::new();

    // Without the fills line neither trades nor book-event classification
    // are possible: emit nothing (the block is incomplete).
    let Some(fills_line) = &block.fills else {
        return Ok(streams);
    };
    let ingest = local_time(&fills_line.local_time)?;
    let mut maker_fills: HashMap<(&str, u64), VecDeque<Size>> = HashMap::new();
    let mut anomaly_pending: HashSet<InstrumentId> = HashSet::new();

    for g in group_fills(fills_line.events.iter().map(|e| &e.0)) {
        if !is_admitted_coin(g.coin) {
            stats.unadmitted_events += g.fills.len() as u64;
            continue;
        }
        let Some(spec) = registry.by_venue_symbol(&venue, g.coin) else {
            stats.unknown_coins.insert(g.coin.to_owned());
            continue;
        };
        match g.class {
            GroupClass::Drop(r) => *stats.dropped_pairs.entry(r).or_default() += 1,
            GroupClass::Anomaly(a) => {
                *stats.anomalies.entry(a).or_default() += 1;
                anomaly_pending.insert(spec.id.clone());
            }
            GroupClass::Trade { taker, maker } => {
                let (t, m) = (g.fills[taker], g.fills[maker]);
                let price = parse_price(g.coin, "px", &t.px, spec.price_scale)?;
                let size = parse_size(g.coin, "sz", &t.sz, spec.size_scale)?;
                if parse_price(g.coin, "px", &m.px, spec.price_scale)? != price {
                    return Err(NormalizeError::PairDisagrees {
                        coin: g.coin.to_owned(),
                        tid: g.tid,
                        field: "price",
                    });
                }
                if parse_size(g.coin, "sz", &m.sz, spec.size_scale)? != size {
                    return Err(NormalizeError::PairDisagrees {
                        coin: g.coin.to_owned(),
                        tid: g.tid,
                        field: "size",
                    });
                }
                let side_err = |s: &str| NormalizeError::Side {
                    coin: g.coin.to_owned(),
                    side: s.to_owned(),
                };
                let aggressor = fill_side_aggressor(&t.side).ok_or_else(|| side_err(&t.side))?;
                fill_side_aggressor(&m.side).ok_or_else(|| side_err(&m.side))?;

                let records = streams
                    .entry((spec.id.clone(), RecordKind::Trade))
                    .or_default();
                let idx = next_index(records)?;
                let mut flags = spec.flags;
                if anomaly_pending.remove(&spec.id) {
                    flags.insert(Flag::GapBefore);
                }
                let prov = Provenance::new(
                    spec.id.clone(),
                    spec.venue.clone(),
                    Seq::new(n, idx),
                    UnixMillis(t.time),
                    ingest,
                    spec.admission.clone(),
                )
                .with_flags(flags);
                let trade = Trade {
                    prov,
                    price,
                    size,
                    aggressor,
                    trade_id: mesh_types::TradeId(g.tid),
                };
                trade
                    .validate()
                    .map_err(|e| NormalizeError::Invalid(e.to_string()))?;
                records.push(trade.into());
                maker_fills
                    .entry((m.coin.as_str(), m.oid))
                    .or_default()
                    .push_back(size);
            }
        }
    }
    carry_gaps.extend(
        anomaly_pending
            .into_iter()
            .map(|id| (id, RecordKind::Trade)),
    );

    let Some(diffs_line) = &block.diffs else {
        return Ok(streams);
    };
    let ingest = local_time(&diffs_line.local_time)?;
    let filled: HashSet<(&str, u64)> = block
        .statuses
        .iter()
        .flat_map(|l| l.events.iter())
        .filter(|e| e.status == FILLED_STATUS)
        .map(|e| (e.order.coin.as_str(), e.order.oid))
        .collect();
    reserve_final_fills(&diffs_line.events, &filled, &mut maker_fills);

    for ev in &diffs_line.events {
        let Some(spec) = lookup(registry, &venue, &ev.coin, stats) else {
            continue;
        };
        let side = diff_side(&ev.side).ok_or_else(|| NormalizeError::Side {
            coin: ev.coin.clone(),
            side: ev.side.clone(),
        })?;
        let price = parse_price(&ev.coin, "px", &ev.px, spec.price_scale)?;
        let key = (ev.coin.as_str(), ev.oid);
        let action = classify_diff(
            &ev.coin,
            &ev.raw_book_diff,
            spec.size_scale,
            maker_fills.get_mut(&key),
            filled.contains(&key),
        )?;
        let records = streams
            .entry((spec.id.clone(), RecordKind::BookEvent))
            .or_default();
        let idx = next_index(records)?;
        let prov = Provenance::new(
            spec.id.clone(),
            spec.venue.clone(),
            Seq::new(n, idx),
            block_time_ms,
            ingest,
            spec.admission.clone(),
        )
        .with_flags(spec.flags);
        let event = BookEvent {
            prov,
            order_id: mesh_types::OrderId(ev.oid),
            side,
            price,
            action,
        };
        event
            .validate()
            .map_err(|e| NormalizeError::Invalid(e.to_string()))?;
        records.push(event.into());
    }
    Ok(streams)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::raw::{BlockLine, BookDiffEvent, Fill, FillEvent, Order, OrderStatusEvent};
    use mesh_types::{AdmissionId, AggressorSide, InstrumentSpec, MarketClass};

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.insert(
            InstrumentSpec::hyperliquid(
                MarketClass::Perp,
                "SYN",
                2,
                None,
                AdmissionId::new("ADM-0001").unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        r
    }

    fn line<E>(n: u64, events: Vec<E>) -> BlockLine<E> {
        BlockLine {
            block_number: n,
            block_time: "2026-01-01T00:00:00.500".into(),
            events,
            local_time: "2026-01-01T00:00:00.510000000".into(),
        }
    }

    fn sz(s: &str) -> Size {
        Size::parse_exact(s).unwrap()
    }

    fn joined(
        fills: Vec<Fill>,
        statuses: Vec<OrderStatusEvent>,
        diffs: Vec<BookDiffEvent>,
    ) -> JoinedBlock {
        JoinedBlock {
            block_number: 3,
            fills: Some(line(3, fills.into_iter().map(FillEvent).collect())),
            statuses: Some(line(3, statuses)),
            diffs: Some(line(3, diffs)),
        }
    }

    #[test]
    fn classify_matches_fill_sizes() {
        let upd = |a: &str, b: &str| RawBookDiff::Update {
            orig_sz: a.into(),
            new_sz: b.into(),
        };
        let mut q: VecDeque<Size> = [sz("1.00")].into();
        // A modify whose delta differs from the pending fill stays a modify.
        assert_eq!(
            classify_diff("SYN", &upd("5", "4.5"), 2, Some(&mut q), false).unwrap(),
            BookAction::Modify {
                orig_size: Some(sz("5.00")),
                new_size: sz("4.50")
            }
        );
        assert_eq!(
            classify_diff("SYN", &upd("4.5", "3.5"), 2, Some(&mut q), false).unwrap(),
            BookAction::Fill {
                orig_size: Some(sz("4.50")),
                new_size: sz("3.50")
            }
        );
        assert!(q.is_empty());
        // A cancel after the fill was consumed is a cancel.
        assert_eq!(
            classify_diff("SYN", &RawBookDiff::Remove, 2, Some(&mut q), false).unwrap(),
            BookAction::Cancel {}
        );
        assert_eq!(
            classify_diff("SYN", &RawBookDiff::Remove, 2, None, true).unwrap(),
            BookAction::Fill {
                orig_size: None,
                new_size: sz("0.00")
            }
        );
        assert_eq!(
            classify_diff("SYN", &upd("2", "0"), 2, None, false).unwrap(),
            BookAction::Cancel {}
        );
        assert!(
            classify_diff(
                "SYN",
                &RawBookDiff::New { sz: "0.001".into() },
                2,
                None,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn trades_and_book_events() {
        let taker = Fill::new("SYN", true, 20, "10.5", "A", "1.5", 77, 1_767_225_600_400);
        let maker = Fill::new("SYN", false, 10, "10.5", "B", "1.5", 77, 1_767_225_600_400);
        let spot = Fill::new("@1", true, 30, "1", "B", "1", 78, 0);
        let b = joined(
            vec![maker, taker, spot],
            vec![OrderStatusEvent::new(
                Order::new("SYN", "B", "10.5", "0", 10, 1),
                "filled",
                "2026-01-01T00:00:00.500",
            )],
            vec![
                BookDiffEvent::new("SYN", 10, "10.5", "Bid", RawBookDiff::Remove),
                BookDiffEvent::new(
                    "SYN",
                    11,
                    "10.4",
                    "Bid",
                    RawBookDiff::New { sz: "3".into() },
                ),
                BookDiffEvent::new("OTHER", 12, "1", "Bid", RawBookDiff::New { sz: "3".into() }),
            ],
        );
        let out = normalize(&b, &registry());
        assert_eq!(out.rejected, None);
        assert!(!out.incomplete);
        assert_eq!(out.stats.unadmitted_events, 1);
        assert!(out.stats.unknown_coins.contains("OTHER"));
        let id = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        let trades = &out.streams[&(id.clone(), RecordKind::Trade)];
        let Record::Trade(t) = &trades[0] else {
            panic!()
        };
        assert_eq!(t.aggressor, AggressorSide::Sell);
        assert_eq!(t.price.to_string(), "10.5000");
        assert_eq!(t.size.to_string(), "1.50");
        assert_eq!(t.prov.seq, Seq::new(3, 0));
        assert_eq!(t.prov.ts_ingest_ns, UnixNanos(1_767_225_600_510_000_000));
        let events = &out.streams[&(id, RecordKind::BookEvent)];
        assert_eq!(events.len(), 2);
        let Record::BookEvent(e) = &events[0] else {
            panic!()
        };
        assert!(matches!(e.action, BookAction::Fill { .. }));
        assert_eq!(e.prov.ts_source_ms, UnixMillis(1_767_225_600_500));
    }

    #[test]
    fn modify_then_removing_fill_in_one_block() {
        let taker = Fill::new("SYN", true, 20, "10.5", "A", "0.24", 77, 0);
        let maker = Fill::new("SYN", false, 10, "10.5", "B", "0.24", 77, 0);
        let upd = RawBookDiff::Update {
            orig_sz: "0.48".into(),
            new_sz: "0.24".into(),
        };
        let b = joined(
            vec![taker, maker],
            vec![OrderStatusEvent::new(
                Order::new("SYN", "B", "10.5", "0", 10, 1),
                "filled",
                "2026-01-01T00:00:00.500",
            )],
            vec![
                BookDiffEvent::new("SYN", 10, "10.5", "Bid", upd),
                BookDiffEvent::new("SYN", 10, "10.5", "Bid", RawBookDiff::Remove),
            ],
        );
        let out = normalize(&b, &registry());
        let id = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        let actions: Vec<&str> = out.streams[&(id, RecordKind::BookEvent)]
            .iter()
            .map(|r| match r {
                Record::BookEvent(e) => e.action.as_str(),
                _ => "other",
            })
            .collect();
        assert_eq!(actions, ["modify", "fill"]);
    }

    #[test]
    fn rejects_disagreeing_pairs_and_flags_anomalies() {
        let a = Fill::new("SYN", true, 20, "10.5", "A", "1.5", 77, 0);
        let b = Fill::new("SYN", false, 10, "10.6", "B", "1.5", 77, 0);
        let out = normalize(&joined(vec![a, b], vec![], vec![]), &registry());
        assert!(out.rejected.unwrap().contains("price"));
        assert!(out.incomplete);
        assert!(out.streams.is_empty());

        let lone = Fill::new("SYN", true, 20, "10.5", "A", "1.5", 1, 0);
        let t1 = Fill::new("SYN", true, 21, "10.5", "A", "1", 2, 0);
        let m1 = Fill::new("SYN", false, 22, "10.5", "B", "1", 2, 0);
        let out = normalize(&joined(vec![lone, t1, m1], vec![], vec![]), &registry());
        let id = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        let Record::Trade(t) = &out.streams[&(id.clone(), RecordKind::Trade)][0] else {
            panic!()
        };
        assert!(t.prov.flags.contains(Flag::GapBefore));
        assert!(out.carry_gaps.is_empty());
        assert_eq!(out.stats.anomalies[&Anomaly::Unmatched], 1);

        let lone = Fill::new("SYN", true, 20, "10.5", "A", "1.5", 1, 0);
        let out = normalize(&joined(vec![lone], vec![], vec![]), &registry());
        assert!(out.carry_gaps.contains(&(id, RecordKind::Trade)));
    }

    #[test]
    fn missing_lines_mark_incomplete() {
        let mut b = joined(vec![], vec![], vec![]);
        b.statuses = None;
        let out = normalize(&b, &registry());
        assert!(out.incomplete);
        assert_eq!(out.rejected, None);
        let mut b = joined(vec![], vec![], vec![]);
        b.diffs.as_mut().unwrap().block_time = "2026-01-01T00:00:00.600".into();
        assert!(normalize(&b, &registry()).rejected.is_some());
    }
}
