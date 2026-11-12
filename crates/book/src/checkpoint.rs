//! Book checkpoints and "book at time T".
//!
//! A checkpoint is an L4 [`BookSnapshot`] the processor emits after a block,
//! so that the archive can answer "book at time T" as the nearest
//! checkpoint at or before T plus the book events after it.
//!
//! # When a checkpoint is emitted
//!
//! The [`CheckpointPolicy`] divides blocks into buckets, by block number
//! (`block_number / every_blocks`) or by block time
//! (`floor(block_time_ms / every_ms)`), aligned to absolute boundaries so
//! that every replay of the same input emits the same checkpoints. After
//! applying a block, a checkpoint is emitted if the block's bucket differs
//! from the bucket of the last checkpoint (or of the snapshot the book was
//! loaded from). A checkpoint carries `seq = end_of_block(block)` and the
//! block's time. Blocks without book events for the instrument do not
//! reach the book and emit nothing; the previous checkpoint plus no events
//! still gives the right book.

use mesh_types::{BookEvent, BookSnapshot, Seq, UnixMillis};
use serde::{Deserialize, Serialize};

use crate::book::OrderBook;
use crate::error::BookError;

/// How often to emit book checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointPolicy {
    /// Never.
    Never,
    /// When `block_number / n` changes.
    EveryBlocks(u64),
    /// When `floor(block_time_ms / n)` changes.
    EveryMillis(i64),
}

impl Default for CheckpointPolicy {
    /// Every 10 minutes of block time (the interval proposed in
    /// `docs/architecture.md` § 3).
    fn default() -> Self {
        Self::EveryMillis(600_000)
    }
}

impl CheckpointPolicy {
    /// The bucket of a block, or `None` for [`CheckpointPolicy::Never`] or
    /// a non-positive interval.
    #[must_use]
    pub fn bucket(self, block_number: u64, block_time: UnixMillis) -> Option<i128> {
        match self {
            Self::Never => None,
            Self::EveryBlocks(n) if n > 0 => Some(i128::from(block_number / n)),
            Self::EveryMillis(n) if n > 0 => Some(i128::from(block_time.0.div_euclid(n))),
            _ => None,
        }
    }
}

/// Decides, block by block, whether to emit a checkpoint.
#[derive(Debug, Clone)]
pub struct Checkpointer {
    policy: CheckpointPolicy,
    last: Option<i128>,
}

impl Checkpointer {
    /// A checkpointer with no checkpoint yet.
    #[must_use]
    pub fn new(policy: CheckpointPolicy) -> Self {
        Self { policy, last: None }
    }

    /// The policy.
    #[must_use]
    pub fn policy(&self) -> CheckpointPolicy {
        self.policy
    }

    /// Records that the book was (re)loaded from a snapshot of this block:
    /// it counts as a checkpoint.
    pub fn loaded(&mut self, block_number: u64, block_time: UnixMillis) {
        self.last = self.policy.bucket(block_number, block_time);
    }

    /// Forgets the last checkpoint (after the book desynchronises).
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// After a block was applied: true if a checkpoint is due. Records it
    /// as emitted.
    pub fn after_block(&mut self, block_number: u64, block_time: UnixMillis) -> bool {
        let Some(bucket) = self.policy.bucket(block_number, block_time) else {
            return false;
        };
        if self.last == Some(bucket) {
            return false;
        }
        self.last = Some(bucket);
        true
    }
}

/// The checkpoint to start from for time `t`: the last one whose
/// `ts_source_ms` is at or before `t`. `checkpoints` must be in `seq`
/// order.
#[must_use]
pub fn nearest_checkpoint(checkpoints: &[BookSnapshot], t: UnixMillis) -> Option<&BookSnapshot> {
    let n = checkpoints.partition_point(|c| c.prov.ts_source_ms <= t);
    n.checked_sub(1).and_then(|i| checkpoints.get(i))
}

/// The book after every event with `seq` after the checkpoint and
/// `ts_source_ms` at or before `t`. Events must be in `seq` order; events
/// at or before the checkpoint are skipped. Blocks are applied whole (all
/// events of a block share its time) and checked for a crossed book.
///
/// # Errors
///
/// Fails on an invalid checkpoint or an inconsistent event.
pub fn book_at(
    checkpoint: &BookSnapshot,
    events: &[BookEvent],
    t: UnixMillis,
) -> Result<OrderBook, BookError> {
    replay(checkpoint, events, |e| e.prov.ts_source_ms <= t)
}

/// The book after every event with `seq` after the checkpoint and at or
/// before `until`.
///
/// # Errors
///
/// Fails on an invalid checkpoint or an inconsistent event.
pub fn book_at_seq(
    checkpoint: &BookSnapshot,
    events: &[BookEvent],
    until: Seq,
) -> Result<OrderBook, BookError> {
    replay(checkpoint, events, |e| e.prov.seq <= until)
}

fn replay(
    checkpoint: &BookSnapshot,
    events: &[BookEvent],
    keep: impl Fn(&BookEvent) -> bool,
) -> Result<OrderBook, BookError> {
    let mut book = OrderBook::from_snapshot(checkpoint)?;
    let start = checkpoint.prov.seq;
    let mut i = events.partition_point(|e| e.prov.seq <= start);
    while i < events.len() {
        let block = events[i].prov.seq.block_number;
        let end = i + events[i..].partition_point(|e| e.prov.seq.block_number == block);
        let slice = &events[i..end];
        let take = slice.partition_point(&keep);
        if take == 0 {
            break;
        }
        if take < slice.len() {
            // Part of a block: the position is the last applied event.
            for e in &slice[..take] {
                book.apply(e)?;
            }
            break;
        }
        let time = slice[0].prov.ts_source_ms;
        book.apply_block(block, time, slice).map_err(|(_, e)| e)?;
        i = end;
    }
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use mesh_types::BookSide;

    #[test]
    fn policy_buckets_are_absolute() {
        let p = CheckpointPolicy::EveryBlocks(10);
        let mut c = Checkpointer::new(p);
        assert!(c.after_block(5, ms(0)));
        assert!(!c.after_block(9, ms(0)));
        assert!(c.after_block(10, ms(0)));
        c.loaded(25, ms(0));
        assert!(!c.after_block(29, ms(0)));
        assert!(c.after_block(30, ms(0)));
        c.reset();
        assert!(c.after_block(30, ms(0)));

        let mut t = Checkpointer::new(CheckpointPolicy::EveryMillis(60_000));
        assert!(t.after_block(1, ms(59_999)));
        assert!(t.after_block(2, ms(60_000)));
        assert!(!t.after_block(3, ms(119_999)));
        assert!(!Checkpointer::new(CheckpointPolicy::Never).after_block(1, ms(0)));
        assert!(!Checkpointer::new(CheckpointPolicy::EveryBlocks(0)).after_block(1, ms(0)));
    }

    #[test]
    fn book_at_time_from_checkpoint() {
        let mut live = OrderBook::new(inst());
        let b1 = [
            add(1, 0, 1, BookSide::Bid, "100.0", "1.00"),
            add(1, 1, 2, BookSide::Ask, "101.0", "1.00"),
        ];
        live.apply_block(1, UnixMillis(block_ms(1)), &b1).unwrap();
        let cp = live.to_snapshot(prov_eob(1, 1_000)).unwrap();
        let later = [
            add(2, 0, 3, BookSide::Bid, "99.0", "1.00"),
            cancel(3, 0, 1, BookSide::Bid, "100.0"),
            add(3, 1, 4, BookSide::Ask, "102.0", "1.00"),
        ];
        let mut all = b1.to_vec();
        all.extend(later.iter().cloned());

        let at2 = book_at(&cp, &all, UnixMillis(block_ms(2))).unwrap();
        assert_eq!(at2.len(), 3);
        assert_eq!(at2.seq(), Some(Seq::end_of_block(2)));
        let at3 = book_at(&cp, &all, UnixMillis(block_ms(3) + 5)).unwrap();
        assert_eq!(at3.len(), 3);
        assert!(at3.order(mesh_types::OrderId(1)).is_none());
        let mid3 = book_at_seq(&cp, &all, Seq::new(3, 0)).unwrap();
        assert_eq!(mid3.len(), 2);
        assert_eq!(mid3.seq(), Some(Seq::new(3, 0)));
        let before = book_at(&cp, &all, UnixMillis(block_ms(1))).unwrap();
        assert_eq!(before.len(), 2);

        let cps = [cp];
        assert_eq!(
            nearest_checkpoint(&cps, UnixMillis(block_ms(5))),
            Some(&cps[0])
        );
        assert_eq!(nearest_checkpoint(&cps, UnixMillis(block_ms(0))), None);
    }
}
