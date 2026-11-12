//! NATS JetStream side tap.
//!
//! Every bus event is published to its subject (see [`crate::tap`]) on one
//! JetStream stream whose age limit is the replay window. A gateway that
//! restarts replays the stream with an ordered consumer to rebuild its
//! books, freshness and proofs. Publication runs on a background task fed by
//! a bounded queue, so the hot path never waits for NATS; when the queue is
//! full the event is dropped and counted (the tap is for durability and
//! replay, never a source of truth).
//!
//! Integration tests against a real `nats-server` are `#[ignore]`d; see
//! `docs/operations/bus.md` for how to run them.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_nats::jetstream::{self, consumer, stream};
use bytes::Bytes;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use tokio::sync::mpsc;

use crate::event::BusEvent;
use crate::tap::{DEFAULT_PREFIX, SideTap, TapError, TapStats, message_id, subject};

/// JetStream tap settings.
#[derive(Debug, Clone)]
pub struct JetStreamConfig {
    /// Server URL(s), comma-separated, e.g. `nats://127.0.0.1:4222`.
    pub url: String,
    /// Stream name.
    pub stream: String,
    /// Subject prefix.
    pub prefix: String,
    /// Replay window: the stream's maximum message age.
    pub replay_window: Duration,
    /// Size bound of the stream, bytes (oldest discarded first).
    pub max_bytes: i64,
    /// Publication queue length.
    pub queue: usize,
    /// Unacknowledged publishes in flight.
    pub max_in_flight: usize,
    /// Optional NATS credentials file.
    pub credentials: Option<PathBuf>,
    /// File storage (true) or memory storage.
    pub file_storage: bool,
}

impl Default for JetStreamConfig {
    fn default() -> Self {
        Self {
            url: "nats://127.0.0.1:4222".into(),
            stream: "BLEH_V0".into(),
            prefix: DEFAULT_PREFIX.into(),
            replay_window: Duration::from_secs(600),
            max_bytes: 4 * 1024 * 1024 * 1024,
            queue: 65_536,
            max_in_flight: 4096,
            credentials: None,
            file_storage: true,
        }
    }
}

#[derive(Debug, Default)]
struct Counters {
    queued: AtomicU64,
    stored: AtomicU64,
    dropped: AtomicU64,
}

/// A side tap on NATS JetStream.
pub struct JetStreamTap {
    cfg: JetStreamConfig,
    js: jetstream::Context,
    tx: mpsc::Sender<Arc<BusEvent>>,
    counters: Arc<Counters>,
}

impl fmt::Debug for JetStreamTap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JetStreamTap")
            .field("stream", &self.cfg.stream)
            .field("prefix", &self.cfg.prefix)
            .finish()
    }
}

fn unavailable(e: impl fmt::Display) -> TapError {
    TapError::Unavailable(e.to_string())
}

impl JetStreamTap {
    /// Connects, creates or updates the stream, and starts the publisher
    /// task. Must be called inside a Tokio runtime.
    ///
    /// # Errors
    ///
    /// [`TapError::Unavailable`] if the server cannot be reached or the
    /// stream cannot be created.
    pub async fn connect(cfg: JetStreamConfig) -> Result<Self, TapError> {
        let mut opts = async_nats::ConnectOptions::new().name("bleh-bus-tap");
        if let Some(path) = &cfg.credentials {
            opts = opts.credentials_file(path).await.map_err(unavailable)?;
        }
        let client = opts.connect(cfg.url.as_str()).await.map_err(unavailable)?;
        let js = jetstream::new(client);
        js.get_or_create_stream(stream::Config {
            name: cfg.stream.clone(),
            subjects: vec![format!("{}.>", cfg.prefix)],
            max_age: cfg.replay_window,
            max_bytes: cfg.max_bytes,
            discard: stream::DiscardPolicy::Old,
            storage: if cfg.file_storage {
                stream::StorageType::File
            } else {
                stream::StorageType::Memory
            },
            duplicate_window: cfg.replay_window.min(Duration::from_secs(120)),
            ..Default::default()
        })
        .await
        .map_err(unavailable)?;
        let (tx, rx) = mpsc::channel(cfg.queue.max(1));
        let counters = Arc::new(Counters::default());
        tokio::spawn(publish_loop(
            js.clone(),
            cfg.prefix.clone(),
            cfg.max_in_flight.max(1),
            rx,
            Arc::clone(&counters),
        ));
        Ok(Self {
            cfg,
            js,
            tx,
            counters,
        })
    }

    async fn replay_inner(&self) -> Result<Vec<BusEvent>, TapError> {
        let mut stream = self
            .js
            .get_stream(&self.cfg.stream)
            .await
            .map_err(unavailable)?;
        let info = stream.info().await.map_err(unavailable)?;
        let last = info.state.last_sequence;
        if info.state.messages == 0 {
            return Ok(Vec::new());
        }
        let consumer = stream
            .create_consumer(consumer::pull::OrderedConfig {
                deliver_policy: consumer::DeliverPolicy::All,
                filter_subject: format!("{}.>", self.cfg.prefix),
                ..Default::default()
            })
            .await
            .map_err(unavailable)?;
        let mut messages = consumer.messages().await.map_err(unavailable)?;
        let mut out = Vec::new();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(5), messages.next())
                .await
                .map_err(|_| TapError::Unavailable("replay timed out".into()))?;
            let Some(m) = next else { break };
            let m = m.map_err(unavailable)?;
            let event: BusEvent =
                serde_json::from_slice(&m.payload).map_err(|e| TapError::Decode {
                    subject: m.subject.to_string(),
                    reason: e.to_string(),
                })?;
            out.push(event);
            let info = m.info().map_err(unavailable)?;
            if info.pending == 0 || info.stream_sequence >= last {
                break;
            }
        }
        Ok(out)
    }
}

async fn publish_loop(
    js: jetstream::Context,
    prefix: String,
    max_in_flight: usize,
    mut rx: mpsc::Receiver<Arc<BusEvent>>,
    counters: Arc<Counters>,
) {
    let mut in_flight = FuturesUnordered::new();
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                let payload = match serde_json::to_vec(&*ev) {
                    Ok(p) => Bytes::from(p),
                    Err(e) => {
                        tracing::error!(error = %e, "side tap: cannot encode event");
                        counters.dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let mut headers = async_nats::HeaderMap::new();
                headers.insert(async_nats::header::NATS_MESSAGE_ID, message_id(&ev).as_str());
                match js.publish_with_headers(subject(&prefix, &ev), headers, payload).await {
                    Ok(ack) => in_flight.push(ack.into_future()),
                    Err(e) => {
                        tracing::warn!(error = %e, "side tap: publish failed");
                        counters.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                while in_flight.len() >= max_in_flight {
                    if let Some(r) = in_flight.next().await {
                        record_ack(&counters, r);
                    }
                }
            }
            Some(r) = in_flight.next(), if !in_flight.is_empty() => record_ack(&counters, r),
        }
    }
    while let Some(r) = in_flight.next().await {
        record_ack(&counters, r);
    }
}

fn record_ack<T, E: fmt::Display>(counters: &Counters, r: Result<T, E>) {
    match r {
        Ok(_) => {
            counters.stored.fetch_add(1, Ordering::Relaxed);
        }
        Err(e) => {
            tracing::warn!(error = %e, "side tap: publish not acknowledged");
            counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

use std::future::IntoFuture;

impl SideTap for JetStreamTap {
    fn publish(&self, event: &Arc<BusEvent>) {
        match self.tx.try_send(Arc::clone(event)) {
            Ok(()) => {
                self.counters.queued.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn replay(&self) -> BoxFuture<'_, Result<Vec<BusEvent>, TapError>> {
        Box::pin(self.replay_inner())
    }

    fn stats(&self) -> TapStats {
        TapStats {
            queued: self.counters.queued.load(Ordering::Relaxed),
            stored: self.counters.stored.load(Ordering::Relaxed),
            dropped: self.counters.dropped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    //! These tests need a local `nats-server -js` on 127.0.0.1:4222:
    //!
    //! ```text
    //! nats-server -js &            # or: docker run -p 4222:4222 nats -js
    //! cargo test -p bus -- --ignored jetstream
    //! ```
    use super::*;
    use crate::event::BlockEnd;
    use crate::signer::tests::batch;
    use mesh_types::{UnixMillis, UnixNanos, VenueId};

    #[tokio::test]
    #[ignore = "needs nats-server -js on 127.0.0.1:4222"]
    async fn jetstream_publish_and_replay() {
        let name = format!("BLEH_TEST_{}", std::process::id());
        let cfg = JetStreamConfig {
            stream: name.clone(),
            prefix: format!("bleh.test{}", std::process::id()),
            file_storage: false,
            ..Default::default()
        };
        let tap = JetStreamTap::connect(cfg).await.unwrap();
        for block in 1..=3 {
            tap.publish(&Arc::new(BusEvent::Batch {
                batch: batch("hyperliquid:perp:BTC", block, 2),
            }));
            tap.publish(&Arc::new(BusEvent::BlockEnd(BlockEnd {
                venue: VenueId::new("hyperliquid").unwrap(),
                block_number: block,
                block_time_ms: UnixMillis(0),
                ts_publish_ns: UnixNanos(0),
            })));
        }
        // Duplicate: deduplicated by message ID.
        tap.publish(&Arc::new(BusEvent::Batch {
            batch: batch("hyperliquid:perp:BTC", 3, 2),
        }));
        for _ in 0..50 {
            if tap.stats().stored + tap.stats().dropped >= 7 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let events = tap.replay().await.unwrap();
        assert_eq!(events.len(), 6, "{events:?}");
        assert_eq!(events[0].block_number(), 1);
        tap.js.delete_stream(&name).await.unwrap();
    }
}
