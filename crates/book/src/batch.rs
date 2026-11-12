//! Batch form for the archive replay.
//!
//! [`run`] replays a range of an instrument's archived records through the
//! same [`Processor`] the live path uses, block by block, and returns every
//! output sorted by [`Output::sort_key`]. Given the same records, block
//! times, start and end, it returns exactly what a live processor emits
//! (after sorting); the parity tests check this.
//!
//! The trade analytics also have slice functions that do not need the
//! book: [`crate::bars::time_bars`], [`fn@crate::bars::rollup`],
//! [`crate::bars::activity_bars`], [`crate::orderflow::footprints`],
//! [`crate::orderflow::deltas`], [`crate::orderflow::volume_profiles`] and
//! [`crate::orderflow::sweeps`].

use std::collections::{BTreeMap, BTreeSet};

use mesh_types::{BookEvent, BookSnapshot, InstrumentId, Trade, UnixMillis};

use crate::error::ProcessError;
use crate::processor::{BlockInput, Output, Processor, ProcessorConfig};

/// How the book starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookStart {
    /// Wait for the first L4 snapshot in the input.
    FromSnapshot,
    /// Start empty and in sync (the range begins with the market).
    Empty,
}

/// Records of one instrument for a replay range.
#[derive(Debug, Clone, Copy)]
pub struct ReplayInput<'a> {
    /// Trades in `seq` order.
    pub trades: &'a [Trade],
    /// Book events in `seq` order.
    pub events: &'a [BookEvent],
    /// Book snapshots in `seq` order; each is processed after its block.
    pub snapshots: &'a [BookSnapshot],
    /// Block times from the raw files. A block missing here takes the time
    /// of its first book event, or else of its last trade.
    pub block_times: &'a BTreeMap<u64, UnixMillis>,
    /// Blocks before which the book-event stream has a gap (missing
    /// blocks in the raw files).
    pub event_gaps: &'a BTreeSet<u64>,
    /// How the book starts.
    pub start: BookStart,
    /// End of the range: intervals ending at or before it are closed as a
    /// live processor's clock would close them, before the final flush.
    pub end: Option<UnixMillis>,
}

/// Replays `input` and returns every output, sorted by
/// [`Output::sort_key`].
///
/// # Errors
///
/// As the [`Processor`] methods.
pub fn run(
    instrument: InstrumentId,
    config: ProcessorConfig,
    input: ReplayInput<'_>,
) -> Result<Vec<Output>, ProcessError> {
    let mut p = match input.start {
        BookStart::Empty => Processor::with_empty_book(instrument, config)?,
        BookStart::FromSnapshot => Processor::new(instrument, config)?,
    };
    let mut blocks: BTreeSet<u64> = BTreeSet::new();
    blocks.extend(input.trades.iter().map(|t| t.prov.seq.block_number));
    blocks.extend(input.events.iter().map(|e| e.prov.seq.block_number));
    blocks.extend(input.snapshots.iter().map(|s| s.prov.seq.block_number));
    let mut out = Vec::new();
    let (mut ti, mut ei, mut si) = (0, 0, 0);
    for b in blocks {
        let tn = input.trades[ti..].partition_point(|t| t.prov.seq.block_number <= b);
        let en = input.events[ei..].partition_point(|e| e.prov.seq.block_number <= b);
        let trades = &input.trades[ti..ti + tn];
        let events = &input.events[ei..ei + en];
        ti += tn;
        ei += en;
        if !trades.is_empty() || !events.is_empty() {
            let block_time = input
                .block_times
                .get(&b)
                .copied()
                .or(events.first().map(|e| e.prov.ts_source_ms))
                .or(trades.last().map(|t| t.prov.ts_source_ms))
                .unwrap_or(UnixMillis(0));
            out.extend(p.process_block(BlockInput {
                block_number: b,
                block_time,
                trades,
                events,
                events_gap_before: !events.is_empty() && input.event_gaps.contains(&b),
            })?);
        }
        while let Some(s) = input.snapshots.get(si) {
            if s.prov.seq.block_number > b {
                break;
            }
            out.extend(p.process_snapshot(s)?);
            si += 1;
        }
    }
    if let Some(end) = input.end {
        out.extend(p.advance_to(end)?);
    }
    out.extend(p.finish()?);
    out.sort_by_key(Output::sort_key);
    Ok(out)
}
