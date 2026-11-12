//! The hub: the single reader of the bus that keeps the gateway's books
//! and freshness, and republishes everything onto the fan-out ring.
//!
//! Because the hub updates its state *before* it publishes an item, and is
//! the only writer of the fan-out ring, a snapshot item it publishes
//! reflects exactly the items before it on the ring. A connection that
//! sends that snapshot and then forwards the items after it sends a book
//! at block `B` followed only by batches for blocks after `B`
//! (gateway-v0 § 5.2) without any per-connection bookkeeping.
//!
//! Books are seeded only from L4 `book_snapshot` records (the book
//! builder's committed checkpoints) or `Resync` events. A book whose
//! upstream skips a block (`prev_block_number` does not link) or whose
//! events do not fit becomes invalid until the next checkpoint, which then
//! produces a forced snapshot (`upstream_gap` or `reconcile`). A valid book
//! that differs from a checkpoint is replaced (`reconcile`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use bus::{BlockEntry, BusEvent, RecvError, ResyncReason, RingReceiver, RingSender, StreamBlock};
use mesh_client::{L4Book, SnapshotReason};
use mesh_types::{
    BlockBatch, InstrumentId, Provenance, Record, RecordKind, SnapshotDepth, StreamId, UnixMillis,
    UnixNanos, VenueId,
};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::Shared;
use crate::item::{Data, Item, SnapshotItem};

/// Freshness of one channel, stream or venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Fresh {
    /// Newest block.
    pub last_block: u64,
    /// Its block time.
    pub block_time_ms: UnixMillis,
    /// Its publish time upstream.
    pub ts_publish_ns: Option<UnixNanos>,
    /// When this gateway saw it.
    pub seen_ns: UnixNanos,
}

#[derive(Debug, Clone)]
pub(crate) struct BookEntry {
    pub(crate) book: Option<L4Book>,
    pub(crate) valid: bool,
    fresh: bool,
    pub(crate) last_batch_block: Option<u64>,
    pending_reason: SnapshotReason,
    template: Provenance,
}

impl BookEntry {
    fn new(template: Provenance) -> Self {
        Self {
            book: None,
            valid: false,
            fresh: false,
            last_batch_block: None,
            pending_reason: SnapshotReason::Subscribe,
            template,
        }
    }

    pub(crate) fn snapshot_record(&self) -> Option<Arc<Record>> {
        let book = self.book.as_ref().filter(|_| self.valid)?;
        let mut snap = book.snapshot(&self.template, mesh_client::Depth::L4).ok()?;
        snap.prov.flags = self.template.flags;
        Some(Arc::new(Record::BookSnapshot(snap)))
    }
}

/// Gateway state kept by the hub.
#[derive(Debug, Default)]
pub struct HubState {
    pub(crate) books: HashMap<InstrumentId, BookEntry>,
    pub(crate) channels: HashMap<(InstrumentId, RecordKind), Fresh>,
    pub(crate) streams: HashMap<(VenueId, RecordKind), Fresh>,
    pub(crate) venues: HashMap<VenueId, Fresh>,
    pub(crate) last_block_at: Option<Instant>,
}

impl HubState {
    /// Instruments with a valid book matching `pred`.
    pub(crate) fn book_instruments(
        &self,
        pred: impl Fn(&InstrumentId) -> bool,
    ) -> Vec<InstrumentId> {
        self.books.keys().filter(|i| pred(i)).cloned().collect()
    }
}

fn fresh(block: u64, block_time_ms: UnixMillis, ts_publish_ns: Option<UnixNanos>) -> Fresh {
    Fresh {
        last_block: block,
        block_time_ms,
        ts_publish_ns,
        seen_ns: UnixNanos::now(),
    }
}

/// The hub task.
pub(crate) struct Hub {
    shared: Arc<Shared>,
    fanout: RingSender<Item>,
    replayed_through: HashMap<VenueId, u64>,
}

impl Hub {
    pub(crate) fn new(shared: Arc<Shared>, fanout: RingSender<Item>) -> Self {
        Self {
            shared,
            fanout,
            replayed_through: HashMap::new(),
        }
    }

    /// Rebuilds books, freshness and proofs from replayed tap events
    /// (oldest first), without publishing. Live events for blocks already
    /// replayed are then ignored.
    pub(crate) fn replay(&mut self, events: Vec<BusEvent>) {
        let mut pending: HashMap<(StreamId, u64), StreamBlock> = HashMap::new();
        for ev in events {
            match &ev {
                BusEvent::Batch { batch } => {
                    let key = (
                        StreamId {
                            venue: batch.venue.clone(),
                            kind: batch.kind,
                        },
                        batch.block_number,
                    );
                    let _ = pending.entry(key).or_default().push_batch(batch);
                }
                BusEvent::Record { record } => {
                    let p = record.prov();
                    let key = (
                        StreamId {
                            venue: p.venue.clone(),
                            kind: record.kind(),
                        },
                        p.seq.block_number,
                    );
                    let _ = pending
                        .entry(key)
                        .or_default()
                        .push(bus::RecordRef::Single(Arc::clone(record)));
                }
                BusEvent::Commitment { commitment } => {
                    let key = (commitment.stream.clone(), commitment.block_number);
                    if let Some(sb) = pending.remove(&key) {
                        let tree = sb.finish();
                        if tree.root == commitment.merkle_root {
                            self.shared
                                .proofs
                                .insert(BlockEntry::new(Arc::clone(commitment), tree));
                        }
                    }
                }
                BusEvent::BlockEnd(b) => {
                    pending.retain(|(_, block), _| *block > b.block_number);
                    self.replayed_through
                        .insert(b.venue.clone(), b.block_number);
                }
                BusEvent::Resync { .. } => {}
            }
            let restart = matches!(ev, BusEvent::Record { .. } | BusEvent::Resync { .. });
            self.apply(&ev, restart);
        }
    }

    pub(crate) async fn run(
        mut self,
        mut bus: RingReceiver<BusEvent>,
        mut requests: mpsc::Receiver<InstrumentId>,
    ) {
        let mut wanted: HashSet<InstrumentId> = HashSet::new();
        loop {
            let mut closed = false;
            tokio::select! {
                r = bus.recv() => match r {
                    Ok(ev) => self.on_event(&ev),
                    Err(RecvError::Lagged(n)) => self.on_lag(n),
                    Err(RecvError::Closed) => closed = true,
                },
                Some(inst) = requests.recv() => {
                    wanted.insert(inst);
                }
            }
            for _ in 0..4096 {
                match bus.try_recv() {
                    Ok(Some(ev)) => self.on_event(&ev),
                    Ok(None) => break,
                    Err(RecvError::Lagged(n)) => self.on_lag(n),
                    Err(RecvError::Closed) => {
                        closed = true;
                        break;
                    }
                }
            }
            while let Ok(inst) = requests.try_recv() {
                wanted.insert(inst);
            }
            for inst in wanted.drain() {
                self.publish_snapshot(&inst, None);
            }
            self.fanout.wake();
            if closed {
                tracing::info!("bus closed; hub stopping");
                return;
            }
        }
    }

    fn on_lag(&mut self, n: u64) {
        tracing::error!(
            missed = n,
            "hub fell behind the bus; invalidating every book"
        );
        self.shared.metrics.hub_lagged.inc();
        let mut st = self
            .shared
            .state
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        for e in st.books.values_mut() {
            if e.valid {
                e.valid = false;
                e.pending_reason = SnapshotReason::UpstreamGap;
            }
        }
    }

    fn on_event(&mut self, ev: &Arc<BusEvent>) {
        self.shared.metrics.hub_events.inc();
        if !matches!(**ev, BusEvent::Resync { .. })
            && let Some(&through) = self.replayed_through.get(ev.venue())
            && ev.block_number() <= through
        {
            return;
        }
        let snapshots = self.apply(ev, false);
        match &**ev {
            BusEvent::Batch { batch } => {
                self.publish(Item::Data(Data::batch(Arc::clone(batch))));
            }
            BusEvent::Record { record } => {
                self.publish(Item::Data(Data::record(Arc::clone(record))));
            }
            BusEvent::Commitment { commitment } => {
                self.publish(Item::Data(Data::commitment(Arc::clone(commitment))));
            }
            BusEvent::BlockEnd(b) => {
                self.shared.metrics.blocks.inc();
                self.shared
                    .metrics
                    .last_block
                    .set(i64::try_from(b.block_number).unwrap_or(i64::MAX));
                self.publish(Item::Block(b.clone()));
            }
            BusEvent::Resync { .. } => {}
        }
        for (inst, forced) in snapshots {
            self.publish_snapshot(&inst, forced);
        }
    }

    fn publish(&self, item: Item) {
        self.fanout.publish_quiet(Arc::new(item));
    }

    fn publish_snapshot(&self, instrument: &InstrumentId, forced: Option<SnapshotReason>) {
        let item = {
            let st = self
                .shared
                .state
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(entry) = st.books.get(instrument) else {
                return;
            };
            let Some(record) = entry.snapshot_record() else {
                return;
            };
            SnapshotItem::new(record, forced, entry.last_batch_block)
        };
        self.publish(Item::Snapshot(item));
    }

    /// Updates state for one event. Returns the books that were (re)seeded
    /// and need a snapshot item, with the forced reason if clients that
    /// were synced must resend.
    fn apply(
        &mut self,
        ev: &BusEvent,
        replaying: bool,
    ) -> Vec<(InstrumentId, Option<SnapshotReason>)> {
        let mut out = Vec::new();
        let metrics = &self.shared.metrics;
        let mut st = self
            .shared
            .state
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        match ev {
            BusEvent::Batch { batch } => {
                let ts_pub = batch.records.first().and_then(|r| r.prov().ts_publish_ns);
                let f = fresh(batch.block_number, batch.block_time_ms, ts_pub);
                st.channels
                    .insert((batch.instrument.clone(), batch.kind), f);
                st.streams.insert((batch.venue.clone(), batch.kind), f);
                if batch.kind == RecordKind::BookEvent {
                    apply_book_batch(&mut st, batch, metrics);
                }
            }
            BusEvent::Record { record } => {
                let p = record.prov();
                let f = fresh(p.seq.block_number, p.ts_source_ms, p.ts_publish_ns);
                st.channels.insert((p.instrument.clone(), record.kind()), f);
                st.streams.insert((p.venue.clone(), record.kind()), f);
                let reason = if replaying {
                    SnapshotReason::Restart
                } else {
                    SnapshotReason::Reconcile
                };
                if let Some(r) = seed(&mut st, record, reason, false, metrics) {
                    out.push(r);
                }
            }
            BusEvent::Resync { record, reason } => {
                let reason = match reason {
                    ResyncReason::UpstreamGap => SnapshotReason::UpstreamGap,
                    ResyncReason::Reconcile => SnapshotReason::Reconcile,
                    ResyncReason::Restart => SnapshotReason::Restart,
                };
                if let Some(r) = seed(&mut st, record, reason, true, metrics) {
                    out.push(r);
                }
            }
            BusEvent::Commitment { commitment } => {
                let f = fresh(commitment.block_number, UnixMillis(0), None);
                let key = (commitment.stream.venue.clone(), commitment.stream.kind);
                st.streams.entry(key).or_insert(f);
            }
            BusEvent::BlockEnd(b) => {
                st.venues.insert(
                    b.venue.clone(),
                    fresh(b.block_number, b.block_time_ms, Some(b.ts_publish_ns)),
                );
                st.last_block_at = Some(Instant::now());
            }
        }
        out
    }
}

fn apply_book_batch(st: &mut HubState, batch: &BlockBatch, metrics: &crate::metrics::Metrics) {
    let Some(first) = batch.records.first() else {
        return;
    };
    let entry = st
        .books
        .entry(batch.instrument.clone())
        .or_insert_with(|| BookEntry::new(first.prov().clone()));
    entry.template = first.prov().clone();
    let prev_last = entry.last_batch_block;
    entry.last_batch_block = Some(batch.block_number);
    if !entry.valid {
        return;
    }
    let Some(book) = entry.book.as_mut() else {
        return;
    };
    let b = book.last_block().unwrap_or(0);
    if batch.block_number <= b {
        return; // already in the seed snapshot
    }
    let contiguous = if entry.fresh {
        batch.prev_block_number.is_none_or(|p| p <= b)
    } else {
        batch.prev_block_number == prev_last
    };
    if !contiguous {
        tracing::warn!(
            instrument = %batch.instrument,
            block = batch.block_number,
            prev = ?batch.prev_block_number,
            last = ?prev_last,
            "upstream gap; book invalid until the next checkpoint"
        );
        metrics.upstream_gaps.inc();
        entry.valid = false;
        entry.pending_reason = SnapshotReason::UpstreamGap;
        return;
    }
    if let Err(e) = book.apply_batch(batch) {
        tracing::warn!(instrument = %batch.instrument, error = %e, "book event does not fit; book invalid");
        metrics.reconciliations.inc();
        entry.valid = false;
        entry.pending_reason = SnapshotReason::Reconcile;
        return;
    }
    entry.fresh = false;
}

/// Seeds or reconciles a book from an L4 `book_snapshot`.
fn seed(
    st: &mut HubState,
    record: &Arc<Record>,
    reason: SnapshotReason,
    force: bool,
    metrics: &crate::metrics::Metrics,
) -> Option<(InstrumentId, Option<SnapshotReason>)> {
    let Record::BookSnapshot(snap) = &**record else {
        return None;
    };
    if !matches!(snap.depth, SnapshotDepth::L4 { .. }) {
        return None;
    }
    let new_book = L4Book::from_snapshot(snap).ok()?;
    let inst = snap.prov.instrument.clone();
    let entry = st
        .books
        .entry(inst.clone())
        .or_insert_with(|| BookEntry::new(snap.prov.clone()));
    let mut template = snap.prov.clone();
    template.ts_publish_ns = template.ts_publish_ns.or(entry.template.ts_publish_ns);
    let forced = match (&mut entry.book, entry.valid) {
        (None, _) => None,
        (Some(book), true) if !force => {
            if book.last_block().unwrap_or(0) > snap.prov.seq.block_number {
                return None; // stale checkpoint
            }
            if *book == new_book {
                book.advance_to(snap.prov.seq.block_number);
                return None;
            }
            tracing::warn!(instrument = %inst, block = snap.prov.seq.block_number, "book differs from checkpoint; replacing");
            metrics.reconciliations.inc();
            Some(reason)
        }
        (Some(_), true) => Some(reason),
        (Some(_), false) => Some(entry.pending_reason),
    };
    entry.book = Some(new_book);
    entry.valid = true;
    entry.fresh = true;
    entry.template = template;
    Some((inst, forced))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{book_batch, empty_snapshot, gateway_shared};
    use mesh_types::{BookAction, BookSide};

    #[tokio::test]
    async fn seeds_applies_detects_gaps_and_reconciles() {
        let (shared, fanout) = gateway_shared();
        let mut rx = fanout.subscribe();
        let mut hub = Hub::new(Arc::clone(&shared), fanout);
        let inst = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        let ev = |e: BusEvent| Arc::new(e);

        // Batch before any checkpoint: forwarded, book not valid.
        hub.on_event(&ev(BusEvent::batch(book_batch(
            10,
            None,
            &[(1, BookAction::Add { size: sz("1.0") })],
        ))));
        assert!(!shared.state.read().unwrap().books[&inst].valid);

        // Checkpoint at block 10 seeds the book and publishes a snapshot item.
        let mut snap = empty_snapshot(10);
        if let SnapshotDepth::L4 { bids, .. } = &mut snap.depth {
            bids.push(mesh_types::RestingOrder {
                order_id: mesh_types::OrderId(1),
                price: mesh_types::Price::parse_exact("99.000").unwrap(),
                size: sz("1.0"),
                ts_order_ms: UnixMillis(0),
            });
        }
        hub.on_event(&ev(BusEvent::record(Record::BookSnapshot(snap.clone()))));
        let items: Vec<_> = std::iter::from_fn(|| rx.try_recv().unwrap()).collect();
        assert!(matches!(&*items[0], Item::Data(_)));
        assert!(matches!(&*items[1], Item::Data(_)));
        let Item::Snapshot(s) = &*items[2] else {
            panic!("{items:?}")
        };
        assert_eq!((s.forced, s.as_of.block_number), (None, 10));

        // Contiguous batch applies.
        hub.on_event(&ev(BusEvent::batch(book_batch(
            12,
            Some(10),
            &[(1, BookAction::Cancel {})],
        ))));
        {
            let st = shared.state.read().unwrap();
            let e = &st.books[&inst];
            assert!(e.valid);
            assert!(e.book.as_ref().unwrap().is_empty());
        }
        // Gap: prev 13 does not link to 12.
        hub.on_event(&ev(BusEvent::batch(book_batch(
            15,
            Some(13),
            &[(2, BookAction::Add { size: sz("1.0") })],
        ))));
        assert!(!shared.state.read().unwrap().books[&inst].valid);
        assert_eq!(shared.metrics.upstream_gaps.get(), 1);
        // Next checkpoint reseeds with a forced upstream_gap snapshot.
        hub.on_event(&ev(BusEvent::record(Record::BookSnapshot(empty_snapshot(
            16,
        )))));
        let items: Vec<_> = std::iter::from_fn(|| rx.try_recv().unwrap()).collect();
        let Item::Snapshot(s) = &*items[items.len() - 1] else {
            panic!()
        };
        assert_eq!(s.forced, Some(SnapshotReason::UpstreamGap));
        // A matching checkpoint changes nothing; a differing one reconciles.
        hub.on_event(&ev(BusEvent::record(Record::BookSnapshot(empty_snapshot(
            17,
        )))));
        assert!(
            !std::iter::from_fn(|| rx.try_recv().unwrap())
                .any(|i| matches!(&*i, Item::Snapshot(_)))
        );
        hub.on_event(&ev(BusEvent::record(Record::BookSnapshot(
            snap_with_order(18),
        ))));
        let items: Vec<_> = std::iter::from_fn(|| rx.try_recv().unwrap()).collect();
        let Item::Snapshot(s) = &*items[items.len() - 1] else {
            panic!()
        };
        assert_eq!(s.forced, Some(SnapshotReason::Reconcile));
        assert_eq!(shared.metrics.reconciliations.get(), 1);
        // Events that do not fit invalidate with reason reconcile.
        hub.on_event(&ev(BusEvent::batch(book_batch(
            19,
            Some(15),
            &[(42, BookAction::Cancel {})],
        ))));
        assert!(!shared.state.read().unwrap().books[&inst].valid);
        let _ = BookSide::Bid;
    }

    fn sz(s: &str) -> mesh_types::Size {
        mesh_types::Size::parse_exact(s).unwrap()
    }

    fn snap_with_order(block: u64) -> mesh_types::BookSnapshot {
        let mut snap = empty_snapshot(block);
        if let SnapshotDepth::L4 { asks, .. } = &mut snap.depth {
            asks.push(mesh_types::RestingOrder {
                order_id: mesh_types::OrderId(7),
                price: mesh_types::Price::parse_exact("101.000").unwrap(),
                size: sz("2.0"),
                ts_order_ms: UnixMillis(0),
            });
        }
        snap
    }

    #[tokio::test]
    async fn replay_rebuilds_state_and_skips_replayed_blocks() {
        let (shared, fanout) = gateway_shared();
        let mut rx = fanout.subscribe();
        let mut hub = Hub::new(Arc::clone(&shared), fanout);
        let end = |b| {
            BusEvent::BlockEnd(bus::BlockEnd {
                venue: VenueId::new("hyperliquid").unwrap(),
                block_number: b,
                block_time_ms: UnixMillis(0),
                ts_publish_ns: UnixNanos(0),
            })
        };
        hub.replay(vec![
            BusEvent::record(Record::BookSnapshot(empty_snapshot(5))),
            end(5),
            BusEvent::batch(book_batch(
                6,
                Some(4),
                &[(1, BookAction::Add { size: sz("1.0") })],
            )),
            end(6),
        ]);
        let inst = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        assert_eq!(
            shared.state.read().unwrap().books[&inst]
                .book
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        hub.on_event(&Arc::new(end(6)));
        assert!(rx.try_recv().unwrap().is_none());
        hub.on_event(&Arc::new(end(7)));
        assert!(matches!(&*rx.try_recv().unwrap().unwrap(), Item::Block(_)));
    }
}
