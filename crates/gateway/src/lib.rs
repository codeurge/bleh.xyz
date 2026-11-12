//! Public websocket gateway and relay for the bleh.xyz mesh.
//!
//! Implements `docs/protocol/gateway-v0.md`. Data comes from an in-process
//! bus ring ([`bus`]): from a reader and block signer in the same process,
//! from the [`demo`] feed, or from an upstream gateway through the
//! verifying [`relay`].
//!
//! ```text
//! bus ring ──▶ hub (books, freshness) ──▶ fan-out ring ──▶ connection tasks ──▶ clients
//! ```
//!
//! | Module | Contents |
//! |---|---|
//! | [`config`] | [`GatewayConfig`] |
//! | [`metrics`] | Prometheus metrics, including the send-latency histogram |
//! | [`relay`] | Upstream gateway client that verifies every commitment before re-serving |
//! | [`demo`] | Synthetic feed from `tools/hl-synth` on a loop |
//! | `hub`, `conn`, `server` | Internals: state, connections, HTTP and websocket endpoints |
//!
//! Operations: `docs/operations/gateway.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod config;
mod conn;
pub mod demo;
mod hub;
mod item;
mod limits;
pub mod metrics;
pub mod relay;
mod server;

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use bus::{BlockSigner, BusEvent, KeyInfo, ProofStore, RingReceiver};
use mesh_types::{EthAddress, InstrumentId};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

pub use config::GatewayConfig;
pub use hub::{Fresh, HubState};
pub use metrics::Metrics;
pub use relay::{RelayConfig, UpstreamStatus};

use crate::item::Item;
use crate::limits::IpTracker;

/// Where the gateway's commitment signers come from.
#[derive(Debug, Clone)]
pub enum Signers {
    /// This process signs: the key ring's addresses.
    Local(Arc<BlockSigner>),
    /// A relay re-serving upstream commitments: the allowlist it verifies
    /// against.
    Static(Vec<EthAddress>),
}

impl Signers {
    fn addresses(&self) -> Vec<EthAddress> {
        match self {
            Self::Local(s) => s.signers(),
            Self::Static(v) => v.clone(),
        }
    }

    fn keys(&self) -> Vec<KeyInfo> {
        match self {
            Self::Local(s) => s.keys(),
            Self::Static(_) => Vec::new(),
        }
    }
}

/// Extra metric lines from the embedding process: `(name, help, value)`.
pub type ExtraMetrics = Box<dyn Fn() -> Vec<(String, String, u64)> + Send + Sync>;

/// State shared by the hub, connections and HTTP handlers.
pub(crate) struct Shared {
    pub(crate) config: GatewayConfig,
    pub(crate) state: RwLock<HubState>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) proofs: Arc<ProofStore>,
    signers: Signers,
    fanout: Mutex<RingReceiver<Item>>,
    snapshot_tx: mpsc::Sender<InstrumentId>,
    pub(crate) ips: Arc<IpTracker>,
    pub(crate) upstream: Option<Arc<UpstreamStatus>>,
    extra_metrics: Option<ExtraMetrics>,
    pub(crate) shutdown: watch::Receiver<bool>,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

impl Shared {
    pub(crate) fn signers(&self) -> Vec<EthAddress> {
        self.signers.addresses()
    }

    pub(crate) fn subscribe_fanout(&self) -> RingReceiver<Item> {
        self.fanout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .resubscribe()
    }

    pub(crate) fn request_snapshot(&self, instrument: InstrumentId) {
        if self.snapshot_tx.try_send(instrument).is_err() {
            tracing::warn!("snapshot request queue full");
        }
    }

    pub(crate) fn extra_metrics(&self) -> Vec<(String, String, u64)> {
        self.extra_metrics.as_ref().map(|f| f()).unwrap_or_default()
    }

    /// Healthy: connected upstream (relay) and a block closed recently.
    pub(crate) fn healthy(&self) -> bool {
        if let Some(u) = &self.upstream
            && !u.is_connected()
        {
            return false;
        }
        let st = self.state.read().unwrap_or_else(PoisonError::into_inner);
        st.last_block_at
            .is_some_and(|t| t.elapsed() <= self.config.max_staleness)
    }
}

/// A gateway being configured. [`Gateway::start`] spawns its hub.
pub struct Gateway {
    config: GatewayConfig,
    bus: RingReceiver<BusEvent>,
    signers: Signers,
    proofs: Arc<ProofStore>,
    metrics: Arc<Metrics>,
    upstream: Option<Arc<UpstreamStatus>>,
    replay: Vec<BusEvent>,
    extra_metrics: Option<ExtraMetrics>,
}

impl fmt::Debug for Gateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gateway")
            .field("config", &self.config)
            .field("signers", &self.signers)
            .finish_non_exhaustive()
    }
}

impl Gateway {
    /// A gateway reading `bus`, announcing `signers`, serving proofs from
    /// `proofs`.
    #[must_use]
    pub fn new(
        config: GatewayConfig,
        bus: RingReceiver<BusEvent>,
        signers: Signers,
        proofs: Arc<ProofStore>,
    ) -> Self {
        Self {
            config,
            bus,
            signers,
            proofs,
            metrics: Arc::new(Metrics::default()),
            upstream: None,
            replay: Vec::new(),
            extra_metrics: None,
        }
    }

    /// The metrics, for components started alongside (the relay).
    #[must_use]
    pub fn metrics(&self) -> Arc<Metrics> {
        Arc::clone(&self.metrics)
    }

    /// Relay mode: health also requires the upstream connection.
    #[must_use]
    pub fn with_upstream(mut self, status: Arc<UpstreamStatus>) -> Self {
        self.upstream = Some(status);
        self
    }

    /// Events replayed from the side tap, applied before going live.
    #[must_use]
    pub fn with_replay(mut self, events: Vec<BusEvent>) -> Self {
        self.replay = events;
        self
    }

    /// Extra metric lines rendered on `/metrics`.
    #[must_use]
    pub fn with_extra_metrics(mut self, f: ExtraMetrics) -> Self {
        self.extra_metrics = Some(f);
        self
    }

    /// Starts the hub. Must be called inside a Tokio runtime.
    #[must_use]
    pub fn start(self) -> GatewayHandle {
        let fanout_tx = bus::ring::<Item>(self.config.ring_capacity);
        let (snapshot_tx, snapshot_rx) = mpsc::channel(self.config.snapshot_queue.max(1));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            fanout: Mutex::new(fanout_tx.subscribe()),
            config: self.config,
            state: RwLock::new(HubState::default()),
            metrics: self.metrics,
            proofs: self.proofs,
            signers: self.signers,
            snapshot_tx,
            ips: Arc::new(IpTracker::default()),
            upstream: self.upstream,
            extra_metrics: self.extra_metrics,
            shutdown: shutdown_rx,
        });
        let mut hub = hub::Hub::new(Arc::clone(&shared), fanout_tx);
        if !self.replay.is_empty() {
            let n = self.replay.len();
            hub.replay(self.replay);
            tracing::info!(events = n, "replayed side tap window");
        }
        tokio::spawn(hub.run(self.bus, snapshot_rx));
        GatewayHandle {
            shared,
            shutdown: Arc::new(shutdown_tx),
        }
    }
}

/// A running gateway.
#[derive(Debug, Clone)]
pub struct GatewayHandle {
    shared: Arc<Shared>,
    shutdown: Arc<watch::Sender<bool>>,
}

impl GatewayHandle {
    /// The HTTP and websocket router.
    pub fn router(&self) -> axum::Router {
        server::router(Arc::clone(&self.shared))
    }

    /// Serves on `listener` until [`GatewayHandle::shutdown`].
    ///
    /// # Errors
    ///
    /// I/O errors from the listener.
    pub async fn serve(&self, listener: TcpListener) -> std::io::Result<()> {
        let mut stop = self.shared.shutdown.clone();
        axum::serve(
            listener,
            self.router()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = stop.changed().await;
        })
        .await
    }

    /// Stops serving; open connections close with 1001.
    pub fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Metrics.
    #[must_use]
    pub fn metrics(&self) -> &Metrics {
        &self.shared.metrics
    }

    /// True when `/health` would answer 200.
    #[must_use]
    pub fn healthy(&self) -> bool {
        self.shared.healthy()
    }

    /// Current signer addresses.
    #[must_use]
    pub fn signers(&self) -> Vec<EthAddress> {
        self.shared.signers()
    }

    /// The hub's L4 book of an instrument, if valid (for tests and tools).
    #[must_use]
    pub fn book(&self, instrument: &InstrumentId) -> Option<mesh_client::L4Book> {
        let st = self
            .shared
            .state
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        st.books
            .get(instrument)
            .filter(|e| e.valid)
            .and_then(|e| e.book.clone())
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use mesh_types::{
        AdmissionId, BlockBatch, BookAction, BookEvent, BookSide, BookSnapshot, OrderId, Price,
        Provenance, Record, RecordKind, Seq, SnapshotDepth, UnixMillis, UnixNanos, VenueId,
    };

    pub(crate) fn gateway_shared() -> (Arc<Shared>, bus::RingSender<Item>) {
        let fanout = bus::ring::<Item>(1024);
        let (snapshot_tx, _rx) = mpsc::channel(16);
        let (_tx, shutdown) = watch::channel(false);
        let shared = Arc::new(Shared {
            fanout: Mutex::new(fanout.subscribe()),
            config: GatewayConfig::default(),
            state: RwLock::new(HubState::default()),
            metrics: Arc::new(Metrics::default()),
            proofs: Arc::new(ProofStore::new(16)),
            signers: Signers::Static(Vec::new()),
            snapshot_tx,
            ips: Arc::new(IpTracker::default()),
            upstream: None,
            extra_metrics: None,
            shutdown,
        });
        (shared, fanout)
    }

    fn prov(block: u64, i: u32) -> Provenance {
        Provenance::new(
            InstrumentId::new("hyperliquid:perp:SYN").unwrap(),
            VenueId::new("hyperliquid").unwrap(),
            Seq::new(block, i),
            UnixMillis(1_767_229_197_000),
            UnixNanos(1_767_229_197_010_000_000),
            AdmissionId::new("ADM-0001").unwrap(),
        )
    }

    pub(crate) fn book_batch(
        block: u64,
        prev: Option<u64>,
        events: &[(u64, BookAction)],
    ) -> BlockBatch {
        BlockBatch {
            instrument: InstrumentId::new("hyperliquid:perp:SYN").unwrap(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::BookEvent,
            block_number: block,
            block_time_ms: UnixMillis(1_767_229_197_000),
            prev_block_number: prev,
            records: events
                .iter()
                .enumerate()
                .map(|(i, (id, action))| {
                    Record::BookEvent(BookEvent {
                        prov: prov(block, u32::try_from(i).unwrap()),
                        order_id: OrderId(*id),
                        side: BookSide::Bid,
                        price: Price::parse_exact("99.000").unwrap(),
                        action: *action,
                    })
                })
                .collect(),
        }
    }

    pub(crate) fn empty_snapshot(block: u64) -> BookSnapshot {
        let mut p = prov(block, 0);
        p.seq = Seq::end_of_block(block);
        BookSnapshot {
            prov: p,
            depth: SnapshotDepth::L4 {
                bids: vec![],
                asks: vec![],
            },
        }
    }
}
