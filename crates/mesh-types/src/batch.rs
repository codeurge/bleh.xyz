//! Block batches: the unit published on the bus.
//!
//! A [`BlockBatch`] holds every record of one instrument and one event kind
//! (`trade` or `book_event`) from one block, in source order. The reader
//! publishes one batch per (instrument, kind, block) that has records;
//! blocks without records for an instrument publish nothing.
//!
//! Gap detection uses the `prev_block_number` link: each batch names the
//! previous block that had a batch for the same instrument and kind. A
//! consumer that last saw block `b` and receives a batch whose
//! `prev_block_number` is not `b` has missed data. Within a batch,
//! `event_index` runs `0..n` without holes ([`BlockBatch::validate`]).
//!
//! Book snapshots, bars and other derived records are published as single
//! [`Record`]s on their own subjects, not in batches.

use serde::{Deserialize, Serialize};

use crate::ids::{InstrumentId, VenueId};
use crate::record::{Record, RecordKind};
use crate::time::UnixMillis;

/// Error from [`BlockBatch::validate`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    /// Only `trade` and `book_event` records travel in batches.
    #[error("kind {0} is not batched")]
    KindNotBatched(RecordKind),
    /// A batch must contain at least one record.
    #[error("empty batch")]
    Empty,
    /// A record's kind differs from the batch's.
    #[error("record {index} has kind {found}, batch is {expected}")]
    WrongKind {
        /// Position in the batch.
        index: usize,
        /// Batch kind.
        expected: RecordKind,
        /// Record kind.
        found: RecordKind,
    },
    /// A record's instrument, venue or block differs from the batch's.
    #[error("record {0} belongs to a different instrument, venue or block")]
    Foreign(usize),
    /// `event_index` does not equal the record's position.
    #[error("record {index} has event_index {found}")]
    IndexHole {
        /// Position in the batch.
        index: usize,
        /// The record's `event_index`.
        found: u32,
    },
    /// `prev_block_number` is not before `block_number`.
    #[error("prev_block_number must be less than block_number")]
    BadPrevLink,
    /// A record failed its own validation.
    #[error("record {index}: {source}")]
    Invalid {
        /// Position in the batch.
        index: usize,
        /// The validation error.
        source: crate::ValidationError,
    },
}

/// All records of one instrument and kind from one block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockBatch {
    /// Instrument of every record.
    pub instrument: InstrumentId,
    /// Venue of every record.
    pub venue: VenueId,
    /// `trade` or `book_event`.
    pub kind: RecordKind,
    /// Block of every record.
    pub block_number: u64,
    /// Block time.
    pub block_time_ms: UnixMillis,
    /// Previous block with a batch for this instrument and kind, if known.
    /// `None` only for the first batch a reader publishes for the stream.
    pub prev_block_number: Option<u64>,
    /// The records, in `event_index` order.
    pub records: Vec<Record>,
}

impl BlockBatch {
    /// Checks the batch invariants listed in the module docs, and each
    /// record's own validation.
    ///
    /// # Errors
    ///
    /// Returns the first violated invariant.
    pub fn validate(&self) -> Result<(), BatchError> {
        if !matches!(self.kind, RecordKind::Trade | RecordKind::BookEvent) {
            return Err(BatchError::KindNotBatched(self.kind));
        }
        if self.records.is_empty() {
            return Err(BatchError::Empty);
        }
        if self
            .prev_block_number
            .is_some_and(|p| p >= self.block_number)
        {
            return Err(BatchError::BadPrevLink);
        }
        for (index, r) in self.records.iter().enumerate() {
            if r.kind() != self.kind {
                return Err(BatchError::WrongKind {
                    index,
                    expected: self.kind,
                    found: r.kind(),
                });
            }
            let p = r.prov();
            if p.instrument != self.instrument
                || p.venue != self.venue
                || p.seq.block_number != self.block_number
            {
                return Err(BatchError::Foreign(index));
            }
            if usize::try_from(p.seq.event_index).ok() != Some(index) {
                return Err(BatchError::IndexHole {
                    index,
                    found: p.seq.event_index,
                });
            }
            r.validate()
                .map_err(|source| BatchError::Invalid { index, source })?;
        }
        Ok(())
    }

    /// True if this batch directly follows a consumer's last seen block for
    /// the stream (`None` if the consumer has seen nothing yet).
    #[must_use]
    pub fn follows(&self, last_seen_block: Option<u64>) -> bool {
        match (last_seen_block, self.prev_block_number) {
            (None, _) => true,
            (Some(last), Some(prev)) => last == prev,
            (Some(_), None) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::{book_event, trade};
    use crate::{BookAction, Seq};

    fn batch(n: u32) -> BlockBatch {
        BlockBatch {
            instrument: InstrumentId::new("hyperliquid:perp:BTC").unwrap(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
            block_number: 10,
            block_time_ms: UnixMillis(1_767_225_600_000),
            prev_block_number: Some(8),
            records: (0..n).map(|i| trade(10, i).into()).collect(),
        }
    }

    #[test]
    fn valid_batch_round_trips() {
        let b = batch(3);
        b.validate().unwrap();
        let json = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<BlockBatch>(&json).unwrap(), b);
    }

    #[test]
    fn invariants() {
        assert_eq!(batch(0).validate(), Err(BatchError::Empty));

        let mut b = batch(3);
        b.records.remove(1);
        assert_eq!(
            b.validate(),
            Err(BatchError::IndexHole { index: 1, found: 2 })
        );

        let mut b = batch(2);
        b.records[1] = book_event(10, 1, BookAction::Cancel {}).into();
        assert!(matches!(
            b.validate(),
            Err(BatchError::WrongKind { index: 1, .. })
        ));

        let mut b = batch(2);
        b.records[1].prov_mut().seq = Seq::new(11, 1);
        assert_eq!(b.validate(), Err(BatchError::Foreign(1)));

        let mut b = batch(1);
        b.prev_block_number = Some(10);
        assert_eq!(b.validate(), Err(BatchError::BadPrevLink));

        let mut b = batch(1);
        b.kind = RecordKind::Bar;
        assert_eq!(
            b.validate(),
            Err(BatchError::KindNotBatched(RecordKind::Bar))
        );
    }

    #[test]
    fn gap_detection() {
        let b = batch(1);
        assert!(b.follows(None));
        assert!(b.follows(Some(8)));
        assert!(!b.follows(Some(7)));
        let mut first = batch(1);
        first.prev_block_number = None;
        assert!(!first.follows(Some(8)));
    }
}
