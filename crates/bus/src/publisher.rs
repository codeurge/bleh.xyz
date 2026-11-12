//! The producer side of the bus: ring, signer stage and side tap.
//!
//! A reader (or the demo feed) owns one [`BlockPublisher`] per venue and,
//! for every block, calls [`BlockPublisher::publish_batch`] and
//! [`BlockPublisher::publish_record`] for the block's batches and single
//! records, then [`BlockPublisher::end_block`]. Batches reach the ring
//! immediately (the latency-critical path); their leaves are hashed right
//! after. `end_block` sorts each stream's leaves, signs one commitment per
//! stream, stores the trees for proofs, and publishes the commitments and
//! the block end, in the order gateway-v0 § 5.1 requires.
//!
//! `prov.ts_publish_ns` is stamped here, once, if the record does not
//! already carry one.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use mesh_types::{
    BlockBatch, BlockCommitment, BookSnapshot, Record, RecordKind, StreamId, UnixMillis, UnixNanos,
    VenueId,
};

use crate::event::{BlockEnd, BusEvent, ResyncReason};
use crate::ring::RingSender;
use crate::signer::{BlockEntry, BlockSigner, ProofStore, RecordRef, SignerError, StreamBlock};
use crate::tap::SideTap;

/// Error from publishing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublishError {
    /// The batch or record failed validation.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// The item belongs to another venue or block than the open block.
    #[error("block {open} of {venue} is still open; got an item for block {got}")]
    BlockNotClosed {
        /// Venue of the open block.
        venue: VenueId,
        /// Open block.
        open: u64,
        /// Block of the offending item.
        got: u64,
    },
    /// Blocks must increase.
    #[error("block {got} is not after the last closed block {last}")]
    NotIncreasing {
        /// Last closed block.
        last: u64,
        /// Offending block.
        got: u64,
    },
    /// Signer stage failure.
    #[error(transparent)]
    Signer(#[from] SignerError),
}

/// Counters for a publisher.
#[derive(Debug, Default)]
pub struct PublisherStats {
    /// Batches published.
    pub batches: AtomicU64,
    /// Records published (inside batches and single).
    pub records: AtomicU64,
    /// Blocks closed.
    pub blocks: AtomicU64,
    /// Commitments signed.
    pub commitments: AtomicU64,
    /// Total time spent signing, nanoseconds.
    pub sign_nanos: AtomicU64,
}

/// Publishes blocks onto the ring, signing and tapping them.
pub struct BlockPublisher {
    ring: RingSender<BusEvent>,
    signer: Option<Arc<BlockSigner>>,
    proofs: Option<Arc<ProofStore>>,
    tap: Option<Arc<dyn SideTap>>,
    venue: Option<VenueId>,
    open: Option<u64>,
    last_closed: Option<u64>,
    streams: HashMap<RecordKind, StreamBlock>,
    validate: bool,
    stats: Arc<PublisherStats>,
}

impl fmt::Debug for BlockPublisher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockPublisher")
            .field("venue", &self.venue)
            .field("open", &self.open)
            .field("last_closed", &self.last_closed)
            .field("signing", &self.signer.is_some())
            .finish()
    }
}

impl BlockPublisher {
    /// A publisher onto `ring`, without signing or a tap.
    #[must_use]
    pub fn new(ring: RingSender<BusEvent>) -> Self {
        Self {
            ring,
            signer: None,
            proofs: None,
            tap: None,
            venue: None,
            open: None,
            last_closed: None,
            streams: HashMap::new(),
            validate: true,
            stats: Arc::new(PublisherStats::default()),
        }
    }

    /// Signs every block with `signer` and keeps proofs in `proofs`.
    #[must_use]
    pub fn with_signer(mut self, signer: Arc<BlockSigner>, proofs: Arc<ProofStore>) -> Self {
        self.signer = Some(signer);
        self.proofs = Some(proofs);
        self
    }

    /// Copies every event to a side tap.
    #[must_use]
    pub fn with_tap(mut self, tap: Arc<dyn SideTap>) -> Self {
        self.tap = Some(tap);
        self
    }

    /// Turns input validation off (it is on by default).
    #[must_use]
    pub fn without_validation(mut self) -> Self {
        self.validate = false;
        self
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> Arc<PublisherStats> {
        Arc::clone(&self.stats)
    }

    /// The ring this publisher writes to.
    #[must_use]
    pub fn ring(&self) -> &RingSender<BusEvent> {
        &self.ring
    }

    fn open_block(&mut self, venue: &VenueId, block: u64) -> Result<(), PublishError> {
        match (&self.venue, self.open) {
            (Some(v), Some(open)) if v == venue && open == block => Ok(()),
            (Some(v), Some(open)) => Err(PublishError::BlockNotClosed {
                venue: v.clone(),
                open,
                got: block,
            }),
            _ => {
                if let Some(last) = self.last_closed
                    && self.venue.as_ref() == Some(venue)
                    && block <= last
                {
                    return Err(PublishError::NotIncreasing { last, got: block });
                }
                self.venue = Some(venue.clone());
                self.open = Some(block);
                Ok(())
            }
        }
    }

    fn emit(&self, event: BusEvent) {
        let event = Arc::new(event);
        self.ring.publish_arc(Arc::clone(&event));
        if let Some(tap) = &self.tap {
            tap.publish(&event);
        }
    }

    /// Publishes a batch: stamps the publish time, pushes it to the ring,
    /// then hashes its leaves for the block's commitment.
    ///
    /// # Errors
    ///
    /// Invalid batch, or a batch for another block than the open one.
    pub fn publish_batch(&mut self, mut batch: BlockBatch) -> Result<(), PublishError> {
        if self.validate {
            batch
                .validate()
                .map_err(|e| PublishError::Invalid(e.to_string()))?;
        }
        self.open_block(&batch.venue, batch.block_number)?;
        let now = UnixNanos::now();
        for r in &mut batch.records {
            let p = r.prov_mut();
            if p.ts_publish_ns.is_none() {
                p.stamp_publish(now);
            }
        }
        let n = u64::try_from(batch.records.len()).unwrap_or(u64::MAX);
        let kind = batch.kind;
        let batch = Arc::new(batch);
        self.emit(BusEvent::Batch {
            batch: Arc::clone(&batch),
        });
        self.stats.batches.fetch_add(1, Ordering::Relaxed);
        self.stats.records.fetch_add(n, Ordering::Relaxed);
        if self.signer.is_some() {
            self.streams.entry(kind).or_default().push_batch(&batch)?;
        }
        Ok(())
    }

    /// Publishes a single record (snapshot, bar, derived record).
    ///
    /// # Errors
    ///
    /// Invalid record, or a record for another block than the open one.
    pub fn publish_record(&mut self, mut record: Record) -> Result<(), PublishError> {
        if self.validate {
            record
                .validate()
                .map_err(|e| PublishError::Invalid(e.to_string()))?;
        }
        let (venue, block) = {
            let p = record.prov();
            (p.venue.clone(), p.seq.block_number)
        };
        self.open_block(&venue, block)?;
        let p = record.prov_mut();
        if p.ts_publish_ns.is_none() {
            p.stamp_publish(UnixNanos::now());
        }
        let kind = record.kind();
        let record = Arc::new(record);
        self.emit(BusEvent::Record {
            record: Arc::clone(&record),
        });
        self.stats.records.fetch_add(1, Ordering::Relaxed);
        if self.signer.is_some() {
            self.streams
                .entry(kind)
                .or_default()
                .push(RecordRef::Single(record))?;
        }
        Ok(())
    }

    /// Publishes authoritative book state outside the committed stream.
    ///
    /// # Errors
    ///
    /// An invalid snapshot.
    pub fn resync(
        &mut self,
        snapshot: BookSnapshot,
        reason: ResyncReason,
    ) -> Result<(), PublishError> {
        if self.validate {
            snapshot
                .validate()
                .map_err(|e| PublishError::Invalid(e.to_string()))?;
        }
        self.emit(BusEvent::Resync {
            record: Arc::new(Record::BookSnapshot(snapshot)),
            reason,
        });
        Ok(())
    }

    /// Closes a block: signs one commitment per stream that had records,
    /// stores the trees, then publishes the commitments and the block end.
    /// A block with no records still gets a block end (the heartbeat).
    ///
    /// # Errors
    ///
    /// Another block is open, blocks went backwards, or signing failed.
    pub fn end_block(
        &mut self,
        venue: &VenueId,
        block_number: u64,
        block_time_ms: UnixMillis,
    ) -> Result<Vec<Arc<BlockCommitment>>, PublishError> {
        self.open_block(venue, block_number)?;
        let mut commitments = Vec::new();
        if let Some(signer) = &self.signer {
            let mut kinds: Vec<RecordKind> = self.streams.keys().copied().collect();
            kinds.sort();
            for kind in kinds {
                let Some(sb) = self.streams.remove(&kind) else {
                    continue;
                };
                if sb.is_empty() {
                    continue;
                }
                let started = Instant::now();
                let tree = sb.finish();
                let stream = StreamId {
                    venue: venue.clone(),
                    kind,
                };
                let c = Arc::new(signer.sign(stream, block_number, &tree)?);
                let nanos = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                self.stats.sign_nanos.fetch_add(nanos, Ordering::Relaxed);
                self.stats.commitments.fetch_add(1, Ordering::Relaxed);
                if let Some(store) = &self.proofs {
                    store.insert(BlockEntry::new(Arc::clone(&c), tree));
                }
                commitments.push(c);
            }
        }
        self.streams.clear();
        for c in &commitments {
            self.emit(BusEvent::Commitment {
                commitment: Arc::clone(c),
            });
        }
        self.emit(BusEvent::BlockEnd(BlockEnd {
            venue: venue.clone(),
            block_number,
            block_time_ms,
            ts_publish_ns: UnixNanos::now(),
        }));
        self.stats.blocks.fetch_add(1, Ordering::Relaxed);
        self.open = None;
        self.last_closed = Some(block_number);
        Ok(commitments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::ring;
    use crate::signer::tests::{batch, signer, stream};
    use crate::tap::MemoryTap;
    use mesh_types::InstrumentId;
    use std::time::Duration;

    fn drain(rx: &mut crate::ring::RingReceiver<BusEvent>) -> Vec<Arc<BusEvent>> {
        let mut out = Vec::new();
        while let Ok(Some(e)) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    #[test]
    fn block_order_stamping_signing_and_proofs() {
        let tx = ring(64);
        let mut rx = tx.subscribe();
        let proofs = Arc::new(ProofStore::new(10));
        let tap = Arc::new(MemoryTap::new(Duration::from_secs(60), 100));
        let mut p = BlockPublisher::new(tx)
            .with_signer(Arc::new(signer()), Arc::clone(&proofs))
            .with_tap(tap.clone());
        let venue = VenueId::new("hyperliquid").unwrap();
        p.publish_batch((*batch("hyperliquid:perp:ETH", 7, 2)).clone())
            .unwrap();
        p.publish_batch((*batch("hyperliquid:perp:BTC", 7, 1)).clone())
            .unwrap();
        let cs = p.end_block(&venue, 7, UnixMillis(5)).unwrap();
        assert_eq!(cs.len(), 1);
        let events = drain(&mut rx);
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match &**e {
                BusEvent::Batch { .. } => "batch",
                BusEvent::Commitment { .. } => "commitment",
                BusEvent::BlockEnd(_) => "end",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["batch", "batch", "commitment", "end"]);
        let BusEvent::Batch { batch: b } = &*events[0] else {
            panic!()
        };
        assert!(b.records.iter().all(|r| r.prov().ts_publish_ns.is_some()));
        // The commitment covers BTC then ETH, and ignores ts_publish_ns.
        let mut all: Vec<Record> = Vec::new();
        for e in &events[..2] {
            if let BusEvent::Batch { batch } = &**e {
                all.extend(batch.records.iter().cloned());
            }
        }
        all.sort_by(|a, b| a.canonical_cmp(b));
        cs[0].verify_records(&all).unwrap();
        let eth = InstrumentId::new("hyperliquid:perp:ETH").unwrap();
        let pr = proofs.lookup(&stream(), 7, &eth, 1).unwrap();
        pr.commitment
            .verify_inclusion(&pr.record, &pr.proof)
            .unwrap();
        assert_eq!(tap.stats().queued, 4);
        let s = p.stats();
        assert_eq!(s.batches.load(Ordering::Relaxed), 2);
        assert_eq!(s.records.load(Ordering::Relaxed), 3);
        assert_eq!(s.commitments.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn rejects_out_of_block_and_invalid_items() {
        let tx = ring(16);
        let mut p = BlockPublisher::new(tx);
        let venue = VenueId::new("hyperliquid").unwrap();
        p.publish_batch((*batch("hyperliquid:perp:BTC", 3, 1)).clone())
            .unwrap();
        assert!(matches!(
            p.publish_batch((*batch("hyperliquid:perp:BTC", 4, 1)).clone()),
            Err(PublishError::BlockNotClosed {
                open: 3,
                got: 4,
                ..
            })
        ));
        assert!(p.end_block(&venue, 3, UnixMillis(0)).unwrap().is_empty());
        assert!(matches!(
            p.end_block(&venue, 3, UnixMillis(0)),
            Err(PublishError::NotIncreasing { last: 3, got: 3 })
        ));
        let mut bad = (*batch("hyperliquid:perp:BTC", 5, 2)).clone();
        bad.records.pop();
        bad.records
            .push(batch("hyperliquid:perp:BTC", 5, 3).records[2].clone());
        assert!(matches!(
            p.publish_batch(bad),
            Err(PublishError::Invalid(_))
        ));
        // Empty block: heartbeat only.
        p.end_block(&venue, 6, UnixMillis(0)).unwrap();
    }
}
