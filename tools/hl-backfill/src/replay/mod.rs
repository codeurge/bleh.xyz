//! Order-level replay prototype (ADM-0003: `replica_cmds`,
//! `explorer_blocks`).
//!
//! Before the project's nodes wrote raw book diffs, the only order-level
//! history is the L1 transaction log. Replaying its book actions through
//! an emulated matcher ([`matcher`]) rebuilds L3/L4 books, book events and
//! trades, and each archived L2 snapshot (`market_data`) is a checkpoint:
//! the emulated book's top levels must equal the archived ones
//! ([`check_l2`]). Days whose every checkpoint matches can be published;
//! the design and its fidelity limits are in `docs/operations/backfill.md`.
//!
//! Only ADM-0001 fields leave the replay: book events (order ID, side,
//! price, size, action) and trades (price, size, aggressor, trade ID).
//! Accounts stay inside the matcher.

pub mod cmds;
pub mod matcher;

use std::collections::BTreeMap;

use mesh_types::{
    AdmissionId, BookAction, BookEvent, Flag, Flags, Level, OrderId, Price, Provenance, Record,
    RestingOrder, Seq, Size, SnapshotDepth, Trade, TradeId, UnixMillis, VenueId,
};
use serde::Serialize;

use self::cmds::ReplayBlock;
use self::matcher::{AssetSpec, BookChange, Levels, MatchEvent, Matcher, ReplayStats};

/// Outcome of comparing the emulated book with an archived L2 snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Checkpoint {
    /// Asset index.
    pub asset: u32,
    /// Block the snapshot was resolved to.
    pub block_number: u64,
    /// Levels compared per side.
    pub levels_compared: usize,
    /// True if every compared level matched.
    pub matched: bool,
    /// First mismatch: `bid` or `ask`, and the level index.
    pub first_mismatch: Option<(String, usize)>,
}

/// Compares emulated levels with archived levels (both best first). The
/// archive truncates each side (20 levels in the API), so only its depth
/// is compared; if the archive shows fewer levels than its usual depth the
/// emulated side must have exactly as many.
#[must_use]
pub fn check_l2(
    asset: u32,
    block_number: u64,
    emulated: &(Levels, Levels),
    archived: &(Levels, Levels),
    archive_depth: usize,
) -> Checkpoint {
    let mut first_mismatch = None;
    let mut compared = 0;
    for (name, emu, arc) in [
        ("bid", &emulated.0, &archived.0),
        ("ask", &emulated.1, &archived.1),
    ] {
        let n = arc.len();
        compared = compared.max(n);
        let mismatch = (0..n)
            .find(|&i| emu.get(i) != arc.get(i))
            .or_else(|| (n < archive_depth && emu.len() != n).then_some(n));
        if first_mismatch.is_none() {
            first_mismatch = mismatch.map(|i| (name.to_owned(), i));
        }
    }
    Checkpoint {
        asset,
        block_number,
        levels_compared: compared,
        matched: first_mismatch.is_none(),
        first_mismatch,
    }
}

/// Converts an archived L2 snapshot depth to mantissa levels at the
/// asset's scales.
///
/// # Errors
///
/// Fails if the depth is L4 or a value does not fit the scales.
pub fn levels_of(depth: &SnapshotDepth, spec: &AssetSpec) -> Result<(Levels, Levels), String> {
    let SnapshotDepth::L2 { bids, asks } = depth else {
        return Err("expected an L2 snapshot".into());
    };
    let conv = |ls: &[Level]| -> Result<Levels, String> {
        ls.iter()
            .map(|l| {
                let p = l
                    .price
                    .rescale(spec.price_scale)
                    .map_err(|e| e.to_string())?;
                let s = l.size.rescale(spec.size_scale).map_err(|e| e.to_string())?;
                Ok((p.mantissa(), s.mantissa(), l.order_count))
            })
            .collect()
    };
    Ok((conv(bids)?, conv(asks)?))
}

/// Turns one block's match events into records: `book_event` and `trade`,
/// with `event_index` per instrument and kind in emission order, admission
/// `ADM-0003` and flag `backfill`. `ts_source_ms` and `ts_ingest_ns` are
/// the block time (replay output is a pure function of the input).
///
/// # Errors
///
/// Fails if an event names an unknown asset or a value is out of range.
pub fn to_records(
    block: &ReplayBlock,
    events: &[MatchEvent],
    assets: &BTreeMap<u32, AssetSpec>,
    extra_flags: Flags,
) -> Result<Vec<Record>, String> {
    let venue = VenueId::new("hyperliquid").map_err(|e| e.to_string())?;
    let admission = AdmissionId::new(crate::ADMISSION).map_err(|e| e.to_string())?;
    let ts = UnixMillis(block.block_time_ms);
    let ingest = ts.to_nanos().ok_or("block time out of range")?;
    let flags = extra_flags.with(Flag::Backfill);
    let mut idx: BTreeMap<(u32, bool), u32> = BTreeMap::new();
    let mut out = Vec::with_capacity(events.len());
    for ev in events {
        let (asset, is_trade) = match ev {
            MatchEvent::Book { asset, .. } => (*asset, false),
            MatchEvent::Trade { asset, .. } => (*asset, true),
        };
        let spec = assets
            .get(&asset)
            .ok_or_else(|| format!("unknown asset {asset}"))?;
        let i = idx.entry((asset, is_trade)).or_insert(0);
        let seq = Seq::new(block.block_number, *i);
        *i += 1;
        let prov = Provenance::new(
            spec.instrument.clone(),
            venue.clone(),
            seq,
            ts,
            ingest,
            admission.clone(),
        )
        .with_flags(flags);
        let price = |m: i64| Price::new(m, spec.price_scale).map_err(|e| e.to_string());
        let size = |m: i64| Size::new(m, spec.size_scale).map_err(|e| e.to_string());
        out.push(match *ev {
            MatchEvent::Trade {
                px,
                sz,
                aggressor,
                tid,
                ..
            } => Record::from(Trade {
                prov,
                price: price(px)?,
                size: size(sz)?,
                aggressor,
                trade_id: TradeId(tid),
            }),
            MatchEvent::Book {
                oid,
                side,
                px,
                change,
                ..
            } => Record::from(BookEvent {
                prov,
                order_id: OrderId(oid),
                side,
                price: price(px)?,
                action: match change {
                    BookChange::Add { size: s } => BookAction::Add { size: size(s)? },
                    BookChange::Fill { orig, new } => BookAction::Fill {
                        orig_size: Some(size(orig)?),
                        new_size: size(new)?,
                    },
                    BookChange::Cancel => BookAction::Cancel {},
                },
            }),
        });
    }
    Ok(out)
}

/// An L4 snapshot record of the emulated book after `block`.
///
/// # Errors
///
/// Fails if the asset is unknown or a value is out of range.
pub fn l4_snapshot(matcher: &Matcher, asset: u32, block: &ReplayBlock) -> Result<Record, String> {
    let spec = matcher
        .assets()
        .get(&asset)
        .ok_or_else(|| format!("unknown asset {asset}"))?;
    let venue = VenueId::new("hyperliquid").map_err(|e| e.to_string())?;
    let admission = AdmissionId::new(crate::ADMISSION).map_err(|e| e.to_string())?;
    let ts = UnixMillis(block.block_time_ms);
    let prov = Provenance::new(
        spec.instrument.clone(),
        venue,
        Seq::end_of_block(block.block_number),
        ts,
        ts.to_nanos().ok_or("block time out of range")?,
        admission,
    )
    .with_flags(Flags::NONE.with(Flag::Backfill));
    let conv = |orders: Vec<(u64, i64, i64, i64)>| -> Result<Vec<RestingOrder>, String> {
        orders
            .into_iter()
            .map(|(oid, px, sz, t)| {
                Ok(RestingOrder {
                    order_id: OrderId(oid),
                    price: Price::new(px, spec.price_scale).map_err(|e| e.to_string())?,
                    size: Size::new(sz, spec.size_scale).map_err(|e| e.to_string())?,
                    ts_order_ms: UnixMillis(t),
                })
            })
            .collect()
    };
    let (bids, asks) = matcher.l4(asset);
    Ok(Record::from(mesh_types::BookSnapshot {
        prov,
        depth: SnapshotDepth::L4 {
            bids: conv(bids)?,
            asks: conv(asks)?,
        },
    }))
}

/// Result of replaying a stream of blocks.
#[derive(Debug, Default, Serialize)]
pub struct ReplayReport {
    /// What was and was not emulated.
    pub stats: ReplayStats,
    /// Checkpoints against archived L2 snapshots.
    pub checkpoints: Vec<Checkpoint>,
    /// Records produced.
    pub records: u64,
}

/// An archived L2 checkpoint: compare after `block_number` for `asset`.
#[derive(Debug, Clone)]
pub struct L2Checkpoint {
    /// Asset index.
    pub asset: u32,
    /// Block after which the snapshot applies.
    pub block_number: u64,
    /// Archived levels (mantissas).
    pub levels: (Levels, Levels),
}

/// Replays `blocks`, comparing against `checkpoints` after their blocks,
/// and returns the records and a report.
///
/// # Errors
///
/// Fails if records cannot be built.
pub fn replay(
    matcher: &mut Matcher,
    blocks: &[ReplayBlock],
    checkpoints: &[L2Checkpoint],
    archive_depth: usize,
) -> Result<(Vec<Record>, ReplayReport), String> {
    let mut by_block: BTreeMap<u64, Vec<&L2Checkpoint>> = BTreeMap::new();
    for c in checkpoints {
        by_block.entry(c.block_number).or_default().push(c);
    }
    let mut report = ReplayReport::default();
    let mut records = Vec::new();
    for b in blocks {
        let events = matcher.apply_block(b);
        records.extend(to_records(b, &events, matcher.assets(), Flags::NONE)?);
        for c in by_block.get(&b.block_number).into_iter().flatten() {
            let emu = matcher.l2(c.asset);
            report.checkpoints.push(check_l2(
                c.asset,
                b.block_number,
                &emu,
                &c.levels,
                archive_depth,
            ));
        }
    }
    report.stats = matcher.stats.clone();
    report.records = records.len() as u64;
    Ok((records, report))
}

#[cfg(test)]
mod tests;
