//! Relay mode: re-serve an upstream gateway's signed stream, verified.
//!
//! The relay subscribes to every instrument of every kind on an upstream
//! gateway (rotating through several URLs), with commitments. It holds each
//! block's records until the block heartbeat, then, per stream (venue and
//! kind), checks that the commitment's signer is on the configured
//! allowlist, that the signature is valid, and that the Merkle root and
//! leaf count match the records received. Only verified streams are
//! published to the relay's own bus, unchanged (records keep their upstream
//! `ts_publish_ns`); the trees go to the relay's proof store so it serves
//! proofs too. Unverifiable data (no commitment, unknown signer, bad
//! signature, root mismatch, or a partial first block after connecting) is
//! dropped and counted; downstream books see the resulting gap and resync
//! from the next verified checkpoint.
//!
//! Upstream `snapshot` messages are not covered by commitments, so they are
//! ignored unless `trust_upstream_snapshots` is set; books are seeded from
//! the committed `book_snapshot` checkpoints instead.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use bus::{
    BlockEnd, BlockEntry, BusEvent, ProofStore, RecordRef, ResyncReason, RingSender, StreamBlock,
    verify_commitment,
};
use mesh_client::{
    ChannelSpec, Client, ClientConfig, ClientEvent, Encoding, InstrumentPattern, SnapshotReason,
};
use mesh_types::{BlockBatch, BlockCommitment, EthAddress, Record, RecordKind, StreamId};

use crate::metrics::Metrics;

/// Relay settings.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Upstream gateway websocket URLs.
    pub upstreams: Vec<String>,
    /// Signer addresses whose commitments are accepted.
    pub allowlist: Vec<EthAddress>,
    /// Seed books from upstream `snapshot` messages (not signed).
    pub trust_upstream_snapshots: bool,
    /// Encoding on the upstream connection.
    pub encoding: Encoding,
    /// Reconnect when no upstream heartbeat arrives for this long.
    pub stall_timeout: Duration,
}

impl RelayConfig {
    /// Defaults for `upstreams` and `allowlist`.
    #[must_use]
    pub fn new(upstreams: Vec<String>, allowlist: Vec<EthAddress>) -> Self {
        Self {
            upstreams,
            allowlist,
            trust_upstream_snapshots: false,
            encoding: Encoding::Json,
            stall_timeout: Duration::from_secs(2),
        }
    }
}

/// Upstream connection status, for `/health`.
#[derive(Debug, Default)]
pub struct UpstreamStatus {
    connected: AtomicBool,
    url: Mutex<Option<String>>,
}

impl UpstreamStatus {
    /// True while connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// The current upstream URL.
    #[must_use]
    pub fn url(&self) -> Option<String> {
        self.url
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set(&self, url: Option<String>) {
        self.connected.store(url.is_some(), Ordering::Relaxed);
        *self.url.lock().unwrap_or_else(PoisonError::into_inner) = url;
    }
}

#[derive(Debug)]
enum Held {
    Batch(Arc<BlockBatch>),
    Record(Arc<Record>),
}

impl Held {
    fn stream(&self) -> StreamId {
        match self {
            Self::Batch(b) => StreamId {
                venue: b.venue.clone(),
                kind: b.kind,
            },
            Self::Record(r) => StreamId {
                venue: r.prov().venue.clone(),
                kind: r.kind(),
            },
        }
    }
}

/// Every kind, every instrument.
#[must_use]
pub fn all_channels() -> Vec<ChannelSpec> {
    RecordKind::ALL
        .iter()
        .map(|k| ChannelSpec::new(InstrumentPattern::All, *k))
        .collect()
}

/// Verifies one block's held data and publishes what verified. Data is
/// held per venue until that venue's heartbeat.
pub(crate) struct BlockVerifier {
    allowlist: Vec<EthAddress>,
    held: Vec<Held>,
    commitments: Vec<Arc<BlockCommitment>>,
}

/// Outcome of closing a block.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub(crate) accepted: usize,
    pub(crate) rejected: usize,
}

impl BlockVerifier {
    pub(crate) fn new(allowlist: Vec<EthAddress>) -> Self {
        Self {
            allowlist,
            held: Vec::new(),
            commitments: Vec::new(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.held.clear();
        self.commitments.clear();
    }

    fn hold(&mut self, h: Held) {
        self.held.push(h);
    }

    /// Closes `venue`'s block: verifies and publishes that venue's held
    /// data; other venues' data stays held until their heartbeat.
    pub(crate) fn close(
        &mut self,
        end: BlockEnd,
        ring: &RingSender<BusEvent>,
        proofs: &ProofStore,
    ) -> Verdict {
        let block_number = end.block_number;
        let (held, other): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|h| h.stream().venue == end.venue);
        self.held = other;
        let (commitments, other): (Vec<_>, Vec<_>) = std::mem::take(&mut self.commitments)
            .into_iter()
            .partition(|c| c.stream.venue == end.venue);
        self.commitments = other;
        let mut streams: HashMap<StreamId, StreamBlock> = HashMap::new();
        for h in &held {
            let r = match h {
                Held::Batch(b) => streams.entry(h.stream()).or_default().push_batch(b),
                Held::Record(r) => streams
                    .entry(h.stream())
                    .or_default()
                    .push(RecordRef::Single(Arc::clone(r))),
            };
            if let Err(e) = r {
                tracing::warn!(error = %e, "relay: cannot hash upstream record");
            }
        }
        let mut accepted: HashSet<StreamId> = HashSet::new();
        let mut verdict = Verdict::default();
        let mut good_commitments = Vec::new();
        for c in commitments {
            if c.block_number != block_number {
                tracing::warn!(
                    block = c.block_number,
                    heartbeat = block_number,
                    "relay: commitment for another block"
                );
                verdict.rejected += 1;
                continue;
            }
            let Some(sb) = streams.remove(&c.stream) else {
                tracing::warn!(stream = ?c.stream, block = block_number, "relay: commitment without records; cannot verify root");
                verdict.rejected += 1;
                continue;
            };
            match verify_commitment(&c, sb, &self.allowlist) {
                Ok(tree) => {
                    proofs.insert(BlockEntry::new(Arc::clone(&c), tree));
                    accepted.insert(c.stream.clone());
                    good_commitments.push(c);
                    verdict.accepted += 1;
                }
                Err(e) => {
                    tracing::warn!(stream = ?c.stream, block = block_number, error = %e, "relay: rejected stream");
                    verdict.rejected += 1;
                }
            }
        }
        for s in streams.keys() {
            tracing::warn!(stream = ?s, block = block_number, "relay: records without a commitment; rejected");
            verdict.rejected += 1;
        }
        for h in held {
            if !accepted.contains(&h.stream()) {
                continue;
            }
            let ev = match h {
                Held::Batch(batch) => BusEvent::Batch { batch },
                Held::Record(record) => BusEvent::Record { record },
            };
            ring.publish_quiet(Arc::new(ev));
        }
        for commitment in good_commitments {
            ring.publish_quiet(Arc::new(BusEvent::Commitment { commitment }));
        }
        ring.publish_quiet(Arc::new(BusEvent::BlockEnd(end)));
        ring.wake();
        verdict
    }
}

/// Runs the relay. It returns only if no upstream URL is configured:
/// upstream failures are retried with backoff across the configured URLs.
pub async fn run(
    cfg: RelayConfig,
    ring: RingSender<BusEvent>,
    proofs: Arc<ProofStore>,
    status: Arc<UpstreamStatus>,
    metrics: Arc<Metrics>,
) {
    let mut client = Client::new(ClientConfig {
        urls: cfg.upstreams.clone(),
        encoding: cfg.encoding,
        channels: all_channels(),
        commitments: true,
        signers: cfg.allowlist.clone(),
        verify_signatures: false,
        verify_roots: false,
        proof_every: 0,
        track_books: false,
        stall_timeout: cfg.stall_timeout,
        ..ClientConfig::default()
    });
    let mut verifier = BlockVerifier::new(cfg.allowlist.clone());
    let mut started = false;
    loop {
        let event = match client.next_event().await {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(error = %e, "relay: upstream client stopped");
                status.set(None);
                return;
            }
        };
        match event {
            ClientEvent::Connected { url, hello } => {
                tracing::info!(%url, upstream_signers = ?hello.signers, "relay: connected upstream");
                let unknown: Vec<_> = hello
                    .signers
                    .iter()
                    .filter(|s| !cfg.allowlist.contains(s))
                    .collect();
                if !unknown.is_empty() {
                    tracing::warn!(
                        ?unknown,
                        "relay: upstream announces signers not on the allowlist"
                    );
                }
                status.set(Some(url));
                metrics.relay_connects.inc();
                metrics.relay_connected.set(1);
                started = false;
                verifier.clear();
            }
            ClientEvent::Disconnected { url, reason } => {
                tracing::warn!(%url, %reason, "relay: upstream disconnected");
                status.set(None);
                metrics.relay_connected.set(0);
                started = false;
                verifier.clear();
            }
            ClientEvent::ConnectFailed { url, error } => {
                tracing::warn!(%url, %error, "relay: upstream connect failed");
            }
            ClientEvent::Batch { batch } if started => verifier.hold(Held::Batch(batch)),
            ClientEvent::Record { record } if started => verifier.hold(Held::Record(record)),
            ClientEvent::Commitment { commitment, .. } if started => {
                verifier.commitments.push(commitment);
            }
            ClientEvent::Snapshot { record, reason, .. } if cfg.trust_upstream_snapshots => {
                let reason = match reason {
                    SnapshotReason::Reconcile => ResyncReason::Reconcile,
                    SnapshotReason::Restart => ResyncReason::Restart,
                    _ => ResyncReason::UpstreamGap,
                };
                ring.publish(BusEvent::Resync { record, reason });
            }
            ClientEvent::Block {
                venue,
                block_number,
                block_time_ms,
                ts_publish_ns,
                ..
            } => {
                if !started {
                    // Data before the first heartbeat may be a partial block.
                    started = true;
                    verifier.clear();
                    continue;
                }
                let end = BlockEnd {
                    venue,
                    block_number,
                    block_time_ms,
                    ts_publish_ns,
                };
                let v = verifier.close(end, &ring, &proofs);
                metrics
                    .relay_streams_rejected
                    .add(u64::try_from(v.rejected).unwrap_or(u64::MAX));
                if v.rejected == 0 {
                    metrics.relay_blocks_verified.inc();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bus::{BlockSigner, KeyRing, keys::parse_key};
    use mesh_types::{
        AdmissionId, AggressorSide, InstrumentId, Price, Provenance, Seq, Size, Trade, TradeId,
        UnixMillis, UnixNanos, VenueId,
    };

    fn batch(block: u64) -> Arc<BlockBatch> {
        batch_of("hyperliquid", block)
    }

    fn batch_of(venue: &str, block: u64) -> Arc<BlockBatch> {
        let inst = InstrumentId::new(format!("{venue}:perp:SYN")).unwrap();
        Arc::new(BlockBatch {
            instrument: inst.clone(),
            venue: VenueId::new(venue).unwrap(),
            kind: RecordKind::Trade,
            block_number: block,
            block_time_ms: UnixMillis(1),
            prev_block_number: None,
            records: (0..3)
                .map(|i| {
                    Record::Trade(Trade {
                        prov: Provenance::new(
                            inst.clone(),
                            VenueId::new(venue).unwrap(),
                            Seq::new(block, i),
                            UnixMillis(1),
                            UnixNanos(2),
                            AdmissionId::new("ADM-0001").unwrap(),
                        ),
                        price: Price::parse_exact("1.0").unwrap(),
                        size: Size::parse_exact("1").unwrap(),
                        aggressor: AggressorSide::Buy,
                        trade_id: TradeId(u64::from(i)),
                    })
                })
                .collect(),
        })
    }

    fn sign(key: &str, b: &Arc<BlockBatch>) -> Arc<BlockCommitment> {
        let signer = BlockSigner::new(KeyRing::single("k", parse_key(key).unwrap())).unwrap();
        let mut sb = StreamBlock::new();
        sb.push_batch(b).unwrap();
        Arc::new(
            signer
                .sign(
                    StreamId {
                        venue: b.venue.clone(),
                        kind: b.kind,
                    },
                    b.block_number,
                    &sb.finish(),
                )
                .unwrap(),
        )
    }

    fn end(block: u64) -> BlockEnd {
        end_of("hyperliquid", block)
    }

    fn end_of(venue: &str, block: u64) -> BlockEnd {
        BlockEnd {
            venue: VenueId::new(venue).unwrap(),
            block_number: block,
            block_time_ms: UnixMillis(1),
            ts_publish_ns: UnixNanos(1),
        }
    }

    fn drain(rx: &mut bus::RingReceiver<BusEvent>) -> Vec<&'static str> {
        std::iter::from_fn(|| rx.try_recv().unwrap())
            .map(|e| match &*e {
                BusEvent::Batch { .. } => "batch",
                BusEvent::Commitment { .. } => "commitment",
                BusEvent::BlockEnd(_) => "end",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn verifies_before_forwarding_and_rejects_the_rest() {
        let good_key = "11".repeat(32);
        let bad_key = "22".repeat(32);
        let allow = vec![parse_key(&good_key).unwrap().address()];
        let ring = bus::ring(64);
        let mut rx = ring.subscribe();
        let proofs = ProofStore::new(8);
        let mut v = BlockVerifier::new(allow);

        // Verified block: forwarded with its commitment, proof stored.
        let b = batch(5);
        v.hold(Held::Batch(Arc::clone(&b)));
        v.commitments.push(sign(&good_key, &b));
        let verdict = v.close(end(5), &ring, &proofs);
        assert_eq!(
            verdict,
            Verdict {
                accepted: 1,
                rejected: 0
            }
        );
        assert_eq!(drain(&mut rx), vec!["batch", "commitment", "end"]);
        let inst = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        let stream = StreamId {
            venue: b.venue.clone(),
            kind: RecordKind::Trade,
        };
        assert!(proofs.lookup(&stream, 5, &inst, 1).is_ok());

        // Unknown signer: dropped, heartbeat still forwarded.
        let b = batch(6);
        v.hold(Held::Batch(Arc::clone(&b)));
        v.commitments.push(sign(&bad_key, &b));
        assert_eq!(v.close(end(6), &ring, &proofs).rejected, 1);
        assert_eq!(drain(&mut rx), vec!["end"]);

        // Tampered record: root mismatch.
        let b = batch(7);
        let c = sign(&good_key, &b);
        let mut t = (*b).clone();
        if let Record::Trade(tr) = &mut t.records[0] {
            tr.trade_id = TradeId(99);
        }
        v.hold(Held::Batch(Arc::new(t)));
        v.commitments.push(c);
        assert_eq!(v.close(end(7), &ring, &proofs).rejected, 1);
        assert_eq!(drain(&mut rx), vec!["end"]);

        // No commitment at all.
        v.hold(Held::Batch(batch(8)));
        assert_eq!(v.close(end(8), &ring, &proofs).rejected, 1);
        assert_eq!(drain(&mut rx), vec!["end"]);
        assert_eq!(all_channels().len(), RecordKind::ALL.len());
    }

    #[test]
    fn a_heartbeat_closes_only_its_own_venue() {
        let key = "11".repeat(32);
        let allow = vec![parse_key(&key).unwrap().address()];
        let ring = bus::ring(64);
        let mut rx = ring.subscribe();
        let proofs = ProofStore::new(8);
        let mut v = BlockVerifier::new(allow);
        let hl = batch(5);
        let other = batch_of("othervenue", 900);
        v.hold(Held::Batch(Arc::clone(&other)));
        v.commitments.push(sign(&key, &other));
        v.hold(Held::Batch(Arc::clone(&hl)));
        v.commitments.push(sign(&key, &hl));
        assert_eq!(v.close(end(5), &ring, &proofs).accepted, 1);
        assert_eq!(drain(&mut rx), vec!["batch", "commitment", "end"]);
        // The other venue's block is still held, intact, until its heartbeat.
        let verdict = v.close(end_of("othervenue", 900), &ring, &proofs);
        assert_eq!(
            verdict,
            Verdict {
                accepted: 1,
                rejected: 0
            }
        );
        assert_eq!(drain(&mut rx), vec!["batch", "commitment", "end"]);
    }
}
