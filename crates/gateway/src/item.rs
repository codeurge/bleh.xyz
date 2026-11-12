//! Fan-out items: what the hub puts on the gateway ring for connections.
//!
//! Messages that are identical for every connection (batches, records,
//! commitments) are encoded once per encoding, lazily, and shared.

use std::sync::{Arc, OnceLock};

use bus::BlockEnd;
use bytes::Bytes;
use mesh_client::codec::{self, CodecError};
use mesh_client::{Depth, Encoding, Levels, ServerMessage, SnapshotReason};
use mesh_types::{
    BarInterval, BlockBatch, BlockCommitment, InstrumentId, Record, RecordKind, Seq, UnixNanos,
    VenueId,
};
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};

/// An encoded frame, cheap to clone.
#[derive(Debug, Clone)]
pub(crate) enum Frame {
    Text(Utf8Bytes),
    Binary(Bytes),
}

impl Frame {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Text(t) => t.len(),
            Self::Binary(b) => b.len(),
        }
    }

    pub(crate) fn into_message(self) -> Message {
        match self {
            Self::Text(t) => Message::Text(t),
            Self::Binary(b) => Message::Binary(b),
        }
    }
}

/// Encodes a server message as a frame.
pub(crate) fn encode_frame(msg: &ServerMessage, enc: Encoding) -> Result<Frame, CodecError> {
    let bytes = codec::encode(msg, enc)?;
    Ok(match enc {
        Encoding::Json => Frame::Text(
            // serde_json always produces UTF-8.
            Utf8Bytes::try_from(bytes).map_err(|_| CodecError::WrongFrame)?,
        ),
        Encoding::Cbor => Frame::Binary(Bytes::from(bytes)),
    })
}

/// Who a data item is for.
#[derive(Debug, Clone)]
pub(crate) enum Route {
    /// A batch of one instrument and kind.
    Batch(InstrumentId, RecordKind),
    /// Level changes of one instrument's book, for `l2` subscribers.
    Levels(InstrumentId),
    /// A single record; bars carry their interval.
    Record(InstrumentId, RecordKind, Option<BarInterval>),
    /// A commitment of one venue and kind.
    Commitment(VenueId, RecordKind),
}

/// A shared data message.
#[derive(Debug)]
pub(crate) struct Data {
    pub(crate) route: Route,
    pub(crate) msg: ServerMessage,
    pub(crate) block_number: u64,
    /// Node local write time of the records, for the latency histogram.
    pub(crate) ts_ingest_ns: Option<UnixNanos>,
    pub(crate) ts_publish_ns: Option<UnixNanos>,
    json: OnceLock<Result<Frame, String>>,
    cbor: OnceLock<Result<Frame, String>>,
}

impl Data {
    pub(crate) fn batch(batch: Arc<BlockBatch>) -> Self {
        let first = batch.records.first().map(Record::prov);
        Self {
            route: Route::Batch(batch.instrument.clone(), batch.kind),
            block_number: batch.block_number,
            ts_ingest_ns: first.map(|p| p.ts_ingest_ns),
            ts_publish_ns: first.and_then(|p| p.ts_publish_ns),
            msg: ServerMessage::Batch { batch },
            json: OnceLock::new(),
            cbor: OnceLock::new(),
        }
    }

    /// Level changes derived from `batch` (the hub's book after applying
    /// it).
    pub(crate) fn levels(levels: Levels, batch: &BlockBatch) -> Self {
        let first = batch.records.first().map(Record::prov);
        Self {
            route: Route::Levels(levels.instrument.clone()),
            block_number: levels.block_number,
            ts_ingest_ns: first.map(|p| p.ts_ingest_ns),
            ts_publish_ns: levels.ts_publish_ns,
            msg: ServerMessage::Levels(Arc::new(levels)),
            json: OnceLock::new(),
            cbor: OnceLock::new(),
        }
    }

    pub(crate) fn record(record: Arc<Record>) -> Self {
        let p = record.prov();
        let interval = match &*record {
            Record::Bar(b) => Some(b.interval),
            _ => None,
        };
        Self {
            route: Route::Record(p.instrument.clone(), record.kind(), interval),
            block_number: p.seq.block_number,
            ts_ingest_ns: Some(p.ts_ingest_ns),
            ts_publish_ns: p.ts_publish_ns,
            msg: ServerMessage::Record { record },
            json: OnceLock::new(),
            cbor: OnceLock::new(),
        }
    }

    pub(crate) fn commitment(c: Arc<BlockCommitment>) -> Self {
        Self {
            route: Route::Commitment(c.stream.venue.clone(), c.stream.kind),
            block_number: c.block_number,
            ts_ingest_ns: None,
            ts_publish_ns: None,
            msg: ServerMessage::Commitment { commitment: c },
            json: OnceLock::new(),
            cbor: OnceLock::new(),
        }
    }

    /// The encoded frame, computed once per encoding.
    pub(crate) fn frame(&self, enc: Encoding) -> Result<Frame, String> {
        let cell = match enc {
            Encoding::Json => &self.json,
            Encoding::Cbor => &self.cbor,
        };
        cell.get_or_init(|| encode_frame(&self.msg, enc).map_err(|e| e.to_string()))
            .clone()
    }
}

/// A book snapshot published by the hub at its current position, so every
/// item after it on the ring is newer than the snapshot.
#[derive(Debug)]
pub(crate) struct SnapshotItem {
    pub(crate) instrument: InstrumentId,
    pub(crate) as_of: Seq,
    /// The L4 snapshot record.
    pub(crate) l4: Arc<Record>,
    l2: OnceLock<Option<Arc<Record>>>,
    /// Set when the hub replaced its book (upstream gap, reconcile,
    /// restart): connections already synced must resend.
    pub(crate) forced: Option<SnapshotReason>,
    /// Last block with a batch for the instrument's book events.
    pub(crate) last_batch_block: Option<u64>,
}

impl SnapshotItem {
    pub(crate) fn new(
        l4: Arc<Record>,
        forced: Option<SnapshotReason>,
        last_batch_block: Option<u64>,
    ) -> Self {
        let p = l4.prov();
        Self {
            instrument: p.instrument.clone(),
            as_of: p.seq,
            l4,
            l2: OnceLock::new(),
            forced,
            last_batch_block,
        }
    }

    /// The snapshot record at the requested depth.
    pub(crate) fn record(&self, depth: Depth) -> Option<Arc<Record>> {
        match depth {
            Depth::L4 => Some(Arc::clone(&self.l4)),
            Depth::L2 => self
                .l2
                .get_or_init(|| match &*self.l4 {
                    Record::BookSnapshot(s) => s.depth.to_l2().ok().map(|depth| {
                        Arc::new(Record::BookSnapshot(mesh_types::BookSnapshot {
                            prov: s.prov.clone(),
                            depth,
                        }))
                    }),
                    _ => None,
                })
                .clone(),
        }
    }
}

/// One fan-out item.
#[derive(Debug)]
pub(crate) enum Item {
    /// A shared message.
    Data(Data),
    /// A block closed: each connection sends its own heartbeat.
    Block(BlockEnd),
    /// A book snapshot.
    Snapshot(SnapshotItem),
}
