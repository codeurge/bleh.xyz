//! Time-to-block clocks.
//!
//! Some archive data carries no block number: L2 snapshots, and
//! unbatched fills and trades in the older prefixes. Their records still
//! need `seq.block_number`, so the parser asks a [`BlockClock`] for the
//! last block at or before the event time. The clock never extrapolates:
//! a time outside the span it knows is unresolved, and the record is
//! counted and skipped, not guessed.

use mesh_types::UnixMillis;

/// Maps a time to the last block at or before it.
pub trait BlockClock: Send + Sync + std::fmt::Debug {
    /// The highest block whose block time is at or before `ts`, if `ts`
    /// lies within the span this clock knows.
    fn block_at(&self, ts: UnixMillis) -> Option<u64>;
}

/// A clock from `(block_time, block_number)` points, such as the block
/// envelopes of `node_fills_by_block` or `explorer_blocks`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableClock {
    points: Vec<(i64, u64)>,
    /// Largest gap (ms) between `ts` and the last known block before it
    /// that still resolves. Blocks are sub-second, so a large gap means
    /// missing data.
    max_gap_ms: i64,
}

impl TableClock {
    /// Default largest gap: 5 seconds.
    pub const DEFAULT_MAX_GAP_MS: i64 = 5_000;

    /// A clock from points in any order.
    #[must_use]
    pub fn new(points: impl IntoIterator<Item = (UnixMillis, u64)>, max_gap_ms: i64) -> Self {
        let mut c = Self {
            points: Vec::new(),
            max_gap_ms,
        };
        c.extend(points);
        c
    }

    /// Adds points.
    pub fn extend(&mut self, points: impl IntoIterator<Item = (UnixMillis, u64)>) {
        self.points
            .extend(points.into_iter().map(|(t, b)| (t.0, b)));
        self.points.sort_unstable();
        self.points.dedup();
    }

    /// Number of points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// True if the clock knows no blocks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Forgets points before `ts` (keeps memory bounded on long runs).
    pub fn retain_from(&mut self, ts: UnixMillis) {
        self.points.retain(|(t, _)| *t >= ts.0);
    }

    /// Reads `block_time` and `block_number` from batched block lines
    /// (raw or filtered). Lines without both are ignored.
    pub fn extend_from_block_lines<'a>(&mut self, lines: impl IntoIterator<Item = &'a str>) {
        let pts = lines.into_iter().filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).ok()?;
            let block = v.get("block_number")?.as_u64()?;
            let t = mesh_types::UnixNanos::parse_rfc3339(v.get("block_time")?.as_str()?).ok()?;
            Some((t.to_millis(), block))
        });
        self.extend(pts.collect::<Vec<_>>());
    }
}

impl BlockClock for TableClock {
    fn block_at(&self, ts: UnixMillis) -> Option<u64> {
        let (first, last) = (self.points.first()?, self.points.last()?);
        if ts.0 < first.0 || ts.0 > last.0 + self.max_gap_ms {
            return None;
        }
        // Last point with time <= ts; among equal times, the highest block.
        let idx = self.points.partition_point(|(t, _)| *t <= ts.0);
        let (t, _) = self.points[idx.checked_sub(1)?];
        if ts.0 - t > self.max_gap_ms {
            return None;
        }
        self.points[..idx]
            .iter()
            .rev()
            .take_while(|(pt, _)| *pt == t)
            .map(|(_, b)| *b)
            .max()
    }
}

/// Tries clocks in order.
#[derive(Default)]
pub struct ChainClock<'a>(pub Vec<&'a dyn BlockClock>);

impl std::fmt::Debug for ChainClock<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ChainClock({} clocks)", self.0.len())
    }
}

impl BlockClock for ChainClock<'_> {
    fn block_at(&self, ts: UnixMillis) -> Option<u64> {
        self.0.iter().find_map(|c| c.block_at(ts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_within_span_only() {
        let c = TableClock::new(
            [
                (UnixMillis(1000), 10),
                (UnixMillis(1000), 11),
                (UnixMillis(1500), 12),
            ],
            300,
        );
        assert_eq!(c.block_at(UnixMillis(999)), None);
        assert_eq!(c.block_at(UnixMillis(1000)), Some(11));
        assert_eq!(c.block_at(UnixMillis(1200)), Some(11));
        assert_eq!(c.block_at(UnixMillis(1301)), None, "gap too large");
        assert_eq!(c.block_at(UnixMillis(1500)), Some(12));
        assert_eq!(c.block_at(UnixMillis(1800)), Some(12));
        assert_eq!(c.block_at(UnixMillis(1801)), None);
    }

    #[test]
    fn reads_block_lines_and_chains() {
        let mut c = TableClock::new([], 1000);
        c.extend_from_block_lines([
            r#"{"block_number":5,"block_time":"1970-01-01T00:00:01.000","events":[]}"#,
            "garbage",
        ]);
        assert_eq!(c.len(), 1);
        let empty = TableClock::default();
        let chain = ChainClock(vec![&empty, &c]);
        assert_eq!(chain.block_at(UnixMillis(1500)), Some(5));
    }
}
