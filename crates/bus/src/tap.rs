//! The side tap: a durable copy of the bus for replay, off the hot path.
//!
//! The publisher hands every [`BusEvent`] to a [`SideTap`] after it is on
//! the ring. `publish` must never block: implementations queue and drop
//! (and count) when full. A restarting gateway calls [`SideTap::replay`] to
//! rebuild its books, freshness and proof store from the retained window.
//! Nothing archives from the tap (ADR 0006).
//!
//! Subjects (NATS JetStream, one per instrument and kind; [`subject`]):
//!
//! | Event | Subject |
//! |---|---|
//! | batch, record | `<prefix>.rec.<kind>.<instrument>` |
//! | commitment | `<prefix>.commit.<venue>.<kind>` |
//! | block end | `<prefix>.block.<venue>` |
//! | resync | `<prefix>.resync.<instrument>` |
//!
//! `<instrument>` is the instrument ID with `:` written `~` (as in archive
//! paths) and `.` written `%2E`, so it is a single subject token; `*`, `>`
//! and whitespace cannot occur in instrument IDs.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use mesh_types::InstrumentId;

use crate::event::BusEvent;

/// Default subject prefix.
pub const DEFAULT_PREFIX: &str = "bleh.v0";

/// Error from a side tap.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TapError {
    /// Connection or server error.
    #[error("side tap unavailable: {0}")]
    Unavailable(String),
    /// A stored message could not be decoded.
    #[error("bad message on {subject}: {reason}")]
    Decode {
        /// Subject.
        subject: String,
        /// Decoder error.
        reason: String,
    },
}

/// Counters for a side tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TapStats {
    /// Events accepted for publication.
    pub queued: u64,
    /// Events confirmed stored.
    pub stored: u64,
    /// Events dropped because the queue was full or the store failed.
    pub dropped: u64,
}

/// A durable, replayable copy of the bus.
pub trait SideTap: Send + Sync + fmt::Debug {
    /// Queues an event. Never blocks.
    fn publish(&self, event: &Arc<BusEvent>);
    /// Every retained event, oldest first.
    fn replay(&self) -> BoxFuture<'_, Result<Vec<BusEvent>, TapError>>;
    /// Counters.
    fn stats(&self) -> TapStats;
}

/// One subject token for an instrument.
#[must_use]
pub fn instrument_token(instrument: &InstrumentId) -> String {
    instrument.path_segment().replace('.', "%2E")
}

/// The subject an event is published on.
#[must_use]
pub fn subject(prefix: &str, event: &BusEvent) -> String {
    match event {
        BusEvent::Batch { batch } => format!(
            "{prefix}.rec.{}.{}",
            batch.kind,
            instrument_token(&batch.instrument)
        ),
        BusEvent::Record { record } => format!(
            "{prefix}.rec.{}.{}",
            record.kind(),
            instrument_token(&record.prov().instrument)
        ),
        BusEvent::Commitment { commitment } => format!(
            "{prefix}.commit.{}.{}",
            commitment.stream.venue, commitment.stream.kind
        ),
        BusEvent::BlockEnd(b) => format!("{prefix}.block.{}", b.venue),
        BusEvent::Resync { record, .. } => format!(
            "{prefix}.resync.{}",
            instrument_token(&record.prov().instrument)
        ),
    }
}

/// A deduplication ID for an event (JetStream `Nats-Msg-Id`), so a
/// publisher that restarts and republishes does not store duplicates.
#[must_use]
pub fn message_id(event: &BusEvent) -> String {
    match event {
        BusEvent::Batch { batch } => format!(
            "b|{}|{}|{}",
            batch.instrument, batch.kind, batch.block_number
        ),
        BusEvent::Record { record } => {
            let p = record.prov();
            let extra = match &**record {
                mesh_types::Record::Bar(b) => b.interval.as_str(),
                _ => "",
            };
            format!(
                "r|{}|{}|{}|{}{}",
                p.instrument,
                record.kind(),
                p.seq.block_number,
                p.seq.event_index,
                extra
            )
        }
        BusEvent::Commitment { commitment } => format!(
            "c|{}|{}|{}",
            commitment.stream.venue, commitment.stream.kind, commitment.block_number
        ),
        BusEvent::BlockEnd(b) => format!("e|{}|{}", b.venue, b.block_number),
        BusEvent::Resync { record, reason } => format!(
            "s|{}|{}|{}",
            record.prov().instrument,
            record.prov().seq.block_number,
            reason.as_str()
        ),
    }
}

/// An in-memory tap with a bounded replay window (by age and count). For
/// tests and single-process deployments that only need restart-free
/// replay.
pub struct MemoryTap {
    window: Duration,
    max_events: usize,
    events: Mutex<VecDeque<(Instant, Arc<BusEvent>)>>,
    queued: AtomicU64,
    dropped: AtomicU64,
}

impl fmt::Debug for MemoryTap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryTap")
            .field("window", &self.window)
            .field("max_events", &self.max_events)
            .finish()
    }
}

impl MemoryTap {
    /// A tap keeping events younger than `window`, at most `max_events`.
    #[must_use]
    pub fn new(window: Duration, max_events: usize) -> Self {
        Self {
            window,
            max_events: max_events.max(1),
            events: Mutex::new(VecDeque::new()),
            queued: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    fn prune(&self, events: &mut VecDeque<(Instant, Arc<BusEvent>)>) {
        while events.len() > self.max_events {
            events.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        while let Some((t, _)) = events.front() {
            if t.elapsed() <= self.window {
                break;
            }
            events.pop_front();
        }
    }
}

impl SideTap for MemoryTap {
    fn publish(&self, event: &Arc<BusEvent>) {
        let mut events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        events.push_back((Instant::now(), Arc::clone(event)));
        self.queued.fetch_add(1, Ordering::Relaxed);
        self.prune(&mut events);
    }

    fn replay(&self) -> BoxFuture<'_, Result<Vec<BusEvent>, TapError>> {
        let out = {
            let mut events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
            self.prune(&mut events);
            events.iter().map(|(_, e)| (**e).clone()).collect()
        };
        Box::pin(async move { Ok(out) })
    }

    fn stats(&self) -> TapStats {
        let queued = self.queued.load(Ordering::Relaxed);
        let dropped = self.dropped.load(Ordering::Relaxed);
        TapStats {
            queued,
            stored: queued.saturating_sub(dropped),
            dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{BlockEnd, ResyncReason};
    use crate::signer::StreamBlock;
    use crate::signer::tests::{batch, signer, stream};
    use mesh_types::{UnixMillis, UnixNanos, VenueId};

    #[test]
    fn subjects_are_single_tokens_per_instrument_and_kind() {
        let b = BusEvent::Batch {
            batch: batch("hyperliquid:spot:PURR.X", 3, 1),
        };
        assert_eq!(
            subject(DEFAULT_PREFIX, &b),
            "bleh.v0.rec.trade.hyperliquid~spot~PURR%2EX"
        );
        assert_eq!(message_id(&b), "b|hyperliquid:spot:PURR.X|trade|3");
        let mut sb = StreamBlock::new();
        sb.push_batch(&batch("hyperliquid:perp:BTC", 3, 1)).unwrap();
        let c = BusEvent::Commitment {
            commitment: Arc::new(signer().sign(stream(), 3, &sb.finish()).unwrap()),
        };
        assert_eq!(subject("x", &c), "x.commit.hyperliquid.trade");
        assert_eq!(message_id(&c), "c|hyperliquid|trade|3");
        let e = BusEvent::BlockEnd(BlockEnd {
            venue: VenueId::new("hyperliquid").unwrap(),
            block_number: 3,
            block_time_ms: UnixMillis(0),
            ts_publish_ns: UnixNanos(0),
        });
        assert_eq!(subject("x", &e), "x.block.hyperliquid");
        let r = BusEvent::Record {
            record: Arc::new(batch("hyperliquid:perp:xyz:CL", 3, 1).records[0].clone()),
        };
        assert_eq!(subject("x", &r), "x.rec.trade.hyperliquid~perp~xyz~CL");
        assert_eq!(message_id(&r), "r|hyperliquid:perp:xyz:CL|trade|3|0");
        let s = BusEvent::Resync {
            record: Arc::new(batch("hyperliquid:perp:BTC", 3, 1).records[0].clone()),
            reason: ResyncReason::Reconcile,
        };
        assert_eq!(subject("x", &s), "x.resync.hyperliquid~perp~BTC");
        assert!(message_id(&s).ends_with("reconcile"));
    }

    #[tokio::test]
    async fn memory_tap_bounds_by_count_and_age() {
        let tap = MemoryTap::new(Duration::from_secs(60), 3);
        for block in 0..5 {
            tap.publish(&Arc::new(BusEvent::Batch {
                batch: batch("hyperliquid:perp:BTC", block, 1),
            }));
        }
        let got = tap.replay().await.unwrap();
        assert_eq!(
            got.iter().map(BusEvent::block_number).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(tap.stats().dropped, 2);
        let short = MemoryTap::new(Duration::ZERO, 10);
        short.publish(&Arc::new(BusEvent::Batch {
            batch: batch("hyperliquid:perp:BTC", 1, 1),
        }));
        std::thread::sleep(Duration::from_millis(2));
        assert!(short.replay().await.unwrap().is_empty());
    }
}
