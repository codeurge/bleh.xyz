//! What travels on the bus.
//!
//! Per block the publisher emits, in this order (gateway-v0 § 5.1):
//!
//! 1. [`BusEvent::Batch`] for every instrument and batched kind with records
//!    in the block, pushed as soon as the reader hands them over;
//! 2. [`BusEvent::Record`] for single records produced by the block (book
//!    snapshots, bars and other derived records);
//! 3. [`BusEvent::Commitment`] for every stream (venue and kind) with
//!    records in the block;
//! 4. one [`BusEvent::BlockEnd`], which closes the block.
//!
//! [`BusEvent::Resync`] is out of band: authoritative book state that is not
//! part of the committed stream (for example after a reader resynchronized
//! from the node's own snapshot). Consumers reseed their books from it.

use std::sync::Arc;

use mesh_types::{
    BlockBatch, BlockCommitment, InstrumentId, Record, RecordKind, UnixMillis, UnixNanos, VenueId,
};
use serde::{Deserialize, Serialize};

/// End of one block of one venue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockEnd {
    /// Venue whose block this is (the block sequence is per venue).
    pub venue: VenueId,
    /// Block number.
    pub block_number: u64,
    /// Block time.
    pub block_time_ms: UnixMillis,
    /// When the publisher closed the block.
    pub ts_publish_ns: UnixNanos,
}

/// Why consumers must reseed a book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    /// The upstream skipped data.
    UpstreamGap,
    /// The book failed reconciliation against a fresh snapshot.
    Reconcile,
    /// The publisher restarted.
    Restart,
}

impl ResyncReason {
    /// Text form, as used in the gateway's `snapshot.reason`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamGap => "upstream_gap",
            Self::Reconcile => "reconcile",
            Self::Restart => "restart",
        }
    }
}

/// One bus item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BusEvent {
    /// All records of one instrument and batched kind in one block.
    Batch {
        /// The batch.
        batch: Arc<BlockBatch>,
    },
    /// A single record (snapshot, bar, derived record).
    Record {
        /// The record.
        record: Arc<Record>,
    },
    /// A signed Merkle root for one block and stream.
    Commitment {
        /// The commitment.
        commitment: Arc<BlockCommitment>,
    },
    /// Closes a block.
    BlockEnd(BlockEnd),
    /// Authoritative book state outside the committed stream.
    Resync {
        /// A `book_snapshot` record.
        record: Arc<Record>,
        /// Why.
        reason: ResyncReason,
    },
}

impl BusEvent {
    /// Wraps a batch.
    #[must_use]
    pub fn batch(batch: BlockBatch) -> Self {
        Self::Batch {
            batch: Arc::new(batch),
        }
    }

    /// Wraps a record.
    #[must_use]
    pub fn record(record: Record) -> Self {
        Self::Record {
            record: Arc::new(record),
        }
    }

    /// The venue this event belongs to.
    #[must_use]
    pub fn venue(&self) -> &VenueId {
        match self {
            Self::Batch { batch } => &batch.venue,
            Self::Record { record } | Self::Resync { record, .. } => &record.prov().venue,
            Self::Commitment { commitment } => &commitment.stream.venue,
            Self::BlockEnd(b) => &b.venue,
        }
    }

    /// The block this event belongs to.
    #[must_use]
    pub fn block_number(&self) -> u64 {
        match self {
            Self::Batch { batch } => batch.block_number,
            Self::Record { record } | Self::Resync { record, .. } => record.prov().seq.block_number,
            Self::Commitment { commitment } => commitment.block_number,
            Self::BlockEnd(b) => b.block_number,
        }
    }

    /// The instrument and kind, for batches, records and resyncs.
    #[must_use]
    pub fn channel(&self) -> Option<(&InstrumentId, RecordKind)> {
        match self {
            Self::Batch { batch } => Some((&batch.instrument, batch.kind)),
            Self::Record { record } | Self::Resync { record, .. } => {
                Some((&record.prov().instrument, record.kind()))
            }
            Self::Commitment { .. } | Self::BlockEnd(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_end_json_round_trip() {
        let e = BusEvent::BlockEnd(BlockEnd {
            venue: VenueId::new("hyperliquid").unwrap(),
            block_number: 7,
            block_time_ms: UnixMillis(1),
            ts_publish_ns: UnixNanos(2),
        });
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(
            json,
            r#"{"type":"block_end","venue":"hyperliquid","block_number":7,"block_time_ms":1,"ts_publish_ns":"2"}"#
        );
        assert_eq!(serde_json::from_str::<BusEvent>(&json).unwrap(), e);
        assert_eq!(e.block_number(), 7);
        assert_eq!(e.venue().as_str(), "hyperliquid");
        assert!(e.channel().is_none());
        assert_eq!(ResyncReason::UpstreamGap.as_str(), "upstream_gap");
    }
}
