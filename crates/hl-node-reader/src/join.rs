//! Joining the three streams per block (spec § 9).
//!
//! The streams are separate files written by one process and read
//! independently, so they can be at different blocks at any moment. The
//! joiner buffers parsed lines per stream and releases block `N` only once
//! every stream has reached `N` (has a line for it or a later one), so the
//! fill-based classification of book diffs sees the whole block. A stream
//! that stalls for more than `max_lag_blocks` behind the leading stream is
//! not waited for: its blocks are released incomplete.

use std::collections::BTreeMap;

use crate::checkpoint::{Position, ReaderPositions};
use crate::normalize::JoinedBlock;
use crate::raw::{ParsedLine, Stream};

/// What [`Joiner::push`] did with a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// Queued.
    Accepted,
    /// At or below the last released block, or before the start floor:
    /// already handled (a re-read after a restart). Dropped.
    Stale,
    /// Not above the stream's previous line: the stream went backwards or
    /// repeated a block. Dropped.
    OutOfOrder,
}

/// Buffers lines and releases complete blocks in order.
#[derive(Debug)]
pub struct Joiner {
    queues: [BTreeMap<u64, (ParsedLine, Option<Position>)>; 3],
    high: [Option<u64>; 3],
    first: [Option<u64>; 3],
    /// Blocks below the floor are dropped. `None` until known.
    floor: Option<u64>,
    released: Option<u64>,
    max_lag_blocks: u64,
}

impl Joiner {
    /// A joiner. `resume_after` skips blocks at or below it (resume from a
    /// checkpoint). Without it, the joiner starts at the first block that
    /// every stream has reached, so a fresh start never emits a block that
    /// is missing a stream only because reading began mid-file.
    #[must_use]
    pub fn new(resume_after: Option<u64>, max_lag_blocks: u64) -> Self {
        Self {
            queues: Default::default(),
            high: [None; 3],
            first: [None; 3],
            floor: resume_after.map(|b| b.saturating_add(1)),
            released: resume_after,
            max_lag_blocks,
        }
    }

    /// Last block released.
    #[must_use]
    pub fn released(&self) -> Option<u64> {
        self.released
    }

    /// Highest block seen per stream.
    #[must_use]
    pub fn high(&self) -> [Option<u64>; 3] {
        self.high
    }

    /// Queues a line. `pos` is the line's start position (live tailing).
    pub fn push(&mut self, line: ParsedLine, pos: Option<Position>) -> PushOutcome {
        let s = line.stream().index();
        let n = line.block_number();
        if self.released.is_some_and(|r| n <= r) || self.floor.is_some_and(|f| n < f) {
            return PushOutcome::Stale;
        }
        if self.high[s].is_some_and(|h| n <= h) {
            return PushOutcome::OutOfOrder;
        }
        self.high[s] = Some(n);
        self.first[s].get_or_insert(n);
        self.queues[s].insert(n, (line, pos));
        PushOutcome::Accepted
    }

    fn align(&mut self) {
        if self.floor.is_some() {
            return;
        }
        let floor = if self.first.iter().all(Option::is_some) {
            self.first.iter().flatten().max().copied()
        } else {
            // A stream with no line at all: give up waiting once the others
            // are more than the lag limit ahead.
            let lo = self.first.iter().flatten().min().copied();
            let hi = self.high.iter().flatten().max().copied();
            match (lo, hi) {
                (Some(lo), Some(hi)) if hi - lo > self.max_lag_blocks => {
                    self.first.iter().flatten().max().copied()
                }
                _ => None,
            }
        };
        if let Some(f) = floor {
            self.floor = Some(f);
            for q in &mut self.queues {
                *q = q.split_off(&f);
            }
        }
    }

    fn candidate(&self) -> Option<u64> {
        self.queues
            .iter()
            .filter_map(|q| q.keys().next().copied())
            .min()
    }

    fn take(&mut self, n: u64) -> JoinedBlock {
        let mut take = |s: Stream| self.queues[s.index()].remove(&n).map(|(l, _)| l);
        let (f, o, d) = (
            take(Stream::Fills),
            take(Stream::OrderStatuses),
            take(Stream::RawBookDiffs),
        );
        self.released = Some(n);
        JoinedBlock {
            block_number: n,
            fills: match f {
                Some(ParsedLine::Fills(l)) => Some(l),
                _ => None,
            },
            statuses: match o {
                Some(ParsedLine::OrderStatuses(l)) => Some(l),
                _ => None,
            },
            diffs: match d {
                Some(ParsedLine::RawBookDiffs(l)) => Some(l),
                _ => None,
            },
        }
    }

    /// Releases the next block if every stream has reached it (or the
    /// laggards are beyond the lag limit).
    pub fn pop_ready(&mut self) -> Option<JoinedBlock> {
        self.align();
        self.floor?;
        let n = self.candidate()?;
        let lead = self.high.iter().flatten().max().copied().unwrap_or(n);
        let all_reached = self.high.iter().all(|h| h.is_some_and(|h| h >= n));
        if all_reached || lead - n > self.max_lag_blocks {
            Some(self.take(n))
        } else {
            None
        }
    }

    /// Releases the next block regardless of readiness (end of a replay).
    pub fn flush(&mut self) -> Option<JoinedBlock> {
        if self.floor.is_none() {
            let f = self.first.iter().flatten().max().copied()?;
            self.floor = Some(f);
            for q in &mut self.queues {
                *q = q.split_off(&f);
            }
        }
        let n = self.candidate()?;
        Some(self.take(n))
    }

    /// Where each stream resumes after the last released block: the start
    /// of its first queued line, or `cursors[stream]` (the reader's current
    /// position) if nothing is queued.
    #[must_use]
    pub fn resume_positions(&self, cursors: &[Option<Position>; 3]) -> Option<ReaderPositions> {
        let last_block = self.released?;
        let mut streams = BTreeMap::new();
        for s in Stream::ALL {
            let queued = self.queues[s.index()]
                .values()
                .next()
                .and_then(|(_, p)| p.clone());
            if let Some(p) = queued.or_else(|| cursors[s.index()].clone()) {
                streams.insert(s, p);
            }
        }
        Some(ReaderPositions {
            last_block,
            streams,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::raw::BlockLine;

    fn line(s: Stream, n: u64) -> ParsedLine {
        let text = serde_json::to_string(&BlockLine::<()> {
            block_number: n,
            block_time: "2026-01-01T00:00:00.000".into(),
            events: vec![],
            local_time: "2026-01-01T00:00:00.000".into(),
        })
        .unwrap();
        ParsedLine::parse(s, &text).unwrap()
    }

    fn pos(off: u64) -> Option<Position> {
        Some(Position {
            file: "hourly/20260101/0".into(),
            offset: off,
        })
    }

    #[test]
    fn waits_for_all_streams() {
        let mut j = Joiner::new(None, 100);
        assert_eq!(
            j.push(line(Stream::Fills, 5), pos(0)),
            PushOutcome::Accepted
        );
        assert_eq!(
            j.push(line(Stream::Fills, 6), pos(10)),
            PushOutcome::Accepted
        );
        assert_eq!(
            j.push(line(Stream::Fills, 6), pos(10)),
            PushOutcome::OutOfOrder
        );
        assert!(j.pop_ready().is_none());
        j.push(line(Stream::OrderStatuses, 6), pos(0));
        j.push(line(Stream::RawBookDiffs, 6), pos(0));
        // Fresh start aligns on block 6: block 5 lacks two streams only
        // because reading began there.
        let b = j.pop_ready().unwrap();
        assert_eq!(b.block_number, 6);
        assert!(b.is_complete());
        assert!(j.pop_ready().is_none());
        assert_eq!(j.push(line(Stream::Fills, 4), pos(0)), PushOutcome::Stale);
    }

    #[test]
    fn missing_block_in_one_stream_is_released_incomplete() {
        let mut j = Joiner::new(Some(9), 100);
        for s in Stream::ALL {
            j.push(line(s, 10), pos(0));
        }
        j.push(line(Stream::Fills, 11), pos(100));
        j.push(line(Stream::OrderStatuses, 12), pos(100));
        j.push(line(Stream::RawBookDiffs, 11), pos(100));
        assert_eq!(j.pop_ready().unwrap().block_number, 10);
        let b = j.pop_ready().unwrap();
        assert_eq!(b.block_number, 11);
        assert!(b.statuses.is_none());
        assert!(j.pop_ready().is_none());
        let cursors = [pos(200), pos(300), pos(400)];
        let r = j.resume_positions(&cursors).unwrap();
        assert_eq!(r.last_block, 11);
        assert_eq!(r.streams[&Stream::Fills].offset, 200);
        // Statuses still has block 12 queued: resume at its start.
        assert_eq!(r.streams[&Stream::OrderStatuses].offset, 100);
    }

    #[test]
    fn stalled_stream_is_not_waited_for_forever() {
        let mut j = Joiner::new(Some(0), 3);
        for n in 1..=6 {
            j.push(line(Stream::Fills, n), None);
            j.push(line(Stream::OrderStatuses, n), None);
        }
        let b = j.pop_ready().unwrap();
        assert_eq!(b.block_number, 1);
        assert!(b.diffs.is_none());
        assert_eq!(j.pop_ready().unwrap().block_number, 2);
        assert!(j.pop_ready().is_none());
        assert_eq!(j.flush().unwrap().block_number, 3);
    }
}
