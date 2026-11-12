//! The venue coverage signal (observation-v0 § 2.11).
//!
//! One [`Coverage`] record per interval of block time, published in the
//! first block after the interval: the blocks observed, block numbers never
//! observed, and blocks flagged `gap_before`. Built from the block
//! heartbeats alone, so the live path and the archive replay agree.

use mesh_types::{
    AdmissionId, BarInterval, Coverage, InstrumentId, Provenance, Seq, UnixMillis, UnixNanos,
    VenueId,
};

#[derive(Debug, Clone)]
struct Open {
    start: UnixMillis,
    first: u64,
    last: u64,
    last_time: UnixMillis,
    blocks: u64,
    missing: u64,
    gaps: u64,
}

/// Builds coverage records from block heartbeats.
#[derive(Debug, Clone)]
pub struct CoverageBuilder {
    venue: VenueId,
    instrument: InstrumentId,
    admission: AdmissionId,
    interval: BarInterval,
    open: Option<Open>,
    prev_block: Option<u64>,
}

impl CoverageBuilder {
    /// A builder for `venue` with records of `interval`.
    #[must_use]
    pub fn new(venue: VenueId, admission: AdmissionId, interval: BarInterval) -> Self {
        Self {
            instrument: InstrumentId::venue_wide(&venue),
            venue,
            admission,
            interval,
            open: None,
            prev_block: None,
        }
    }

    fn record(&self, o: &Open, published_in: u64) -> Coverage {
        Coverage {
            prov: Provenance::new(
                self.instrument.clone(),
                self.venue.clone(),
                Seq::new(published_in, 0),
                o.last_time,
                UnixNanos(o.last_time.0.saturating_mul(1_000_000)),
                self.admission.clone(),
            ),
            interval: self.interval,
            start_ms: o.start,
            first_block: o.first,
            last_block: o.last,
            blocks: o.blocks,
            missing_blocks: o.missing,
            gap_blocks: o.gaps,
        }
    }

    /// Records block `number` at `time`. Returns the coverage of the
    /// previous interval when this block starts a new one (published in
    /// this block). Blocks at or below the last one are ignored.
    pub fn push(&mut self, number: u64, time: UnixMillis, gap_before: bool) -> Option<Coverage> {
        if self.prev_block.is_some_and(|p| number <= p) {
            return None;
        }
        let missing = self
            .prev_block
            .map_or(0, |p| number.saturating_sub(p).saturating_sub(1));
        self.prev_block = Some(number);
        let start = self.interval.align(time);
        let mut closed = None;
        if let Some(o) = &self.open
            && o.start != start
        {
            closed = Some(self.record(o, number));
            self.open = None;
        }
        let o = self.open.get_or_insert(Open {
            start,
            first: number,
            last: number,
            last_time: time,
            blocks: 0,
            missing: 0,
            gaps: 0,
        });
        o.last = number;
        o.last_time = time;
        o.blocks += 1;
        o.missing += missing;
        o.gaps += u64::from(gap_before);
        closed
    }

    /// The open interval's record, published at its last block (end of
    /// input; not used on the live path, where the interval stays open).
    #[must_use]
    pub fn finish(&mut self) -> Option<Coverage> {
        let o = self.open.take()?;
        Some(self.record(&o, o.last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_767_225_600_000;

    fn builder() -> CoverageBuilder {
        CoverageBuilder::new(
            VenueId::new("hyperliquid").unwrap_or_else(|e| panic!("{e}")),
            AdmissionId::new("ADM-0001").unwrap_or_else(|e| panic!("{e}")),
            BarInterval::M1,
        )
    }

    #[test]
    fn closes_intervals_and_counts_holes() {
        let mut b = builder();
        assert!(b.push(10, UnixMillis(T0 + 100), false).is_none());
        assert!(b.push(11, UnixMillis(T0 + 200), true).is_none());
        // Blocks 12 and 13 never arrive.
        assert!(b.push(14, UnixMillis(T0 + 59_900), false).is_none());
        let c = b
            .push(15, UnixMillis(T0 + 60_100), false)
            .unwrap_or_else(|| panic!("closed"));
        c.validate().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((c.first_block, c.last_block, c.blocks), (10, 14, 3));
        assert_eq!((c.missing_blocks, c.gap_blocks), (2, 1));
        assert_eq!(c.prov.seq, Seq::new(15, 0));
        assert_eq!(c.prov.ts_source_ms, UnixMillis(T0 + 59_900));
        assert!(c.prov.instrument.is_venue_wide());
        assert!(!c.is_complete());
        // Repeats are ignored; a hole before the next interval's first
        // block counts there.
        assert!(b.push(15, UnixMillis(T0 + 60_100), false).is_none());
        assert!(b.push(20, UnixMillis(T0 + 61_000), false).is_none());
        let c = b.finish().unwrap_or_else(|| panic!("open"));
        c.validate().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (c.first_block, c.last_block, c.blocks, c.missing_blocks),
            (15, 20, 2, 4)
        );
        assert_eq!(c.prov.seq, Seq::new(20, 0));
    }
}
