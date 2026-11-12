//! The origin service: node files to a public signed live stream, and raw
//! capture for the archive, in one process (ADR 0006).
//!
//! ```text
//! node dir(s) ─▶ reader pipeline (tail, merge, sequence) ─▶ Deriver (books, bars, analytics, coverage)
//!                                     │                              │
//!                                     ▼                              ▼
//!                              BlockPublisher ◀────────── records, resyncs
//!                    (BlockSigner, ProofStore, JetStream side tap)
//!                                     │
//!                                     ▼
//!                        embedded gateway  /v0/ws /health /freshness /metrics /proof /signers
//!
//! node dir ─▶ archive::follow (private + filtered tiers, upload, .captured-through)
//! ```
//!
//! Per block: the block's batches are published, then its derived records,
//! then any book resyncs, then `end_block` signs one commitment per stream.

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use archive::capture::{Capture, HourWriter, TierConfig};
use archive::follow::{FollowConfig, Follower, UploadTargets};
use archive::store::StoreConfig;
use bus::{BlockPublisher, BlockSigner, KeyRing, KeySpec, ProofStore, ResyncReason, SideTap};
use gateway::{Gateway, GatewayConfig, GatewayHandle, Signers};
use hl_node_reader::BlockOutput;
use hl_node_reader::files::StartPosition;
use hl_node_reader::filter::StreamDirs;
use hl_node_reader::http::InfoClient;
use hl_node_reader::markets::{RegistrationPolicy, RegistryHandle};
use hl_node_reader::merge::MergeConfig;
use hl_node_reader::metrics::Metrics;
use hl_node_reader::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
use hl_node_reader::snapshot::{parse_l4_envelope, parse_l4_snapshot, request_l4_snapshot};
use hl_node_reader::tail::TailerConfig;
use mesh_types::{EthAddress, Registry, UnixMillis, UnixNanos, VenueId};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use crate::derive::{DeriveConfig, Deriver, Notice};
use crate::metrics::names;

/// Node reader settings (the same meaning as `hl-node-reader`'s flags).
#[derive(Debug, Clone)]
pub struct ReaderSettings {
    /// One or two node data directories.
    pub node_dirs: Vec<PathBuf>,
    /// Stream directory names.
    pub dirs: StreamDirs,
    /// Registry to start from.
    pub registry: Option<PathBuf>,
    /// Write the registry here when markets are added.
    pub registry_out: Option<PathBuf>,
    /// Node info server (metadata, L4 snapshots).
    pub info_url: Option<String>,
    /// Info request timeout.
    pub info_timeout: Duration,
    /// Periodic registry refresh.
    pub registry_refresh: Option<Duration>,
    /// Reader checkpoint file.
    pub checkpoint: Option<PathBuf>,
    /// Minimum time between checkpoint writes.
    pub checkpoint_interval: Duration,
    /// Where to start without a checkpoint.
    pub start: StartPosition,
    /// Use filesystem notifications.
    pub use_notify: bool,
    /// Poll interval without notifications.
    pub poll_interval: Duration,
    /// Release blocks without a stream lagging this far.
    pub max_lag_blocks: u64,
    /// Two-node merge settings.
    pub merge: MergeConfig,
    /// L4 snapshot interval (`None`: never).
    pub snapshot_interval: Option<Duration>,
    /// File the node writes snapshots to.
    pub snapshot_path: PathBuf,
}

/// Live capture settings.
#[derive(Debug, Clone)]
pub struct CaptureSettings {
    /// Staging directory (`BLEH_RAW_DIR`); the watermark is written here.
    pub staging: PathBuf,
    /// Capture the private (unfiltered) tier.
    pub private: bool,
    /// Upload sealed hours (stores from `BLEH_ARCHIVE_{PRIVATE,PUBLIC}_*`).
    pub upload: bool,
    /// Keep uploaded files locally.
    pub keep_local: bool,
    /// Archive metrics textfile and its rewrite interval.
    pub metrics_out: Option<(PathBuf, Duration)>,
    /// Poll interval.
    pub poll_interval: Duration,
    /// Seal stopped streams this long after the hour.
    pub seal_grace: Duration,
}

/// Origin settings.
#[derive(Debug, Clone)]
pub struct OriginConfig {
    /// Node reader.
    pub reader: ReaderSettings,
    /// Derived records.
    pub derive: DeriveConfig,
    /// Signing keys (`ID=file:PATH`) and the active key ID.
    pub signing_keys: Vec<KeySpec>,
    /// Active key ID (default: the first).
    pub active_key: Option<String>,
    /// Gateway listen address.
    pub gateway_listen: SocketAddr,
    /// Gateway settings.
    pub gateway: GatewayConfig,
    /// Proof retention, blocks per stream.
    pub proof_retention_blocks: u64,
    /// Reader and origin metrics listen address (`/metrics`, `/health`).
    pub metrics_listen: Option<SocketAddr>,
    /// JetStream side tap.
    pub nats: Option<bus::jetstream::JetStreamConfig>,
    /// Live capture.
    pub capture: Option<CaptureSettings>,
}

/// A running origin.
#[derive(Debug)]
pub struct RunningOrigin {
    /// Where the gateway listens.
    pub gateway_addr: SocketAddr,
    /// Where reader and origin metrics are served.
    pub metrics_addr: Option<SocketAddr>,
    /// Signer addresses (active first).
    pub signers: Vec<EthAddress>,
    /// The gateway.
    pub gateway: GatewayHandle,
    /// Reader metrics.
    pub reader_metrics: Arc<Metrics>,
    /// Origin metrics.
    pub origin_metrics: Arc<Metrics>,
    stop: watch::Sender<bool>,
    tasks: tokio::task::JoinSet<Result<()>>,
}

impl RunningOrigin {
    /// Asks every part to stop: the reader drains and checkpoints, the
    /// gateway closes connections with 1001, capture keeps its open hours.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
        self.gateway.shutdown();
    }

    /// Waits until one part ends (normally only after a stop, or on a fatal
    /// error), returning its result.
    pub async fn next_exit(&mut self) -> Option<Result<()>> {
        let r = self.tasks.join_next().await?;
        Some(r.context("task panicked").and_then(|r| r))
    }

    /// Waits for every part to finish (after [`RunningOrigin::stop`]).
    ///
    /// # Errors
    ///
    /// The first part that failed.
    pub async fn join(mut self) -> Result<()> {
        let mut first = Ok(());
        while let Some(r) = self.next_exit().await {
            if first.is_ok() {
                first = r;
            }
        }
        first
    }
}

fn load_registry(path: Option<&std::path::Path>) -> Result<Registry> {
    match path {
        Some(p) => {
            let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", p.display()))
        }
        None => Ok(Registry::new()),
    }
}

fn registry_handle(r: &ReaderSettings, metrics: &Arc<Metrics>) -> Result<Arc<RegistryHandle>> {
    let initial = load_registry(r.registry.as_deref())?;
    let handle = match &r.info_url {
        Some(url) => {
            let client = InfoClient::new(url, r.info_timeout).context("--info-url")?;
            let h = RegistryHandle::refreshing(
                initial,
                client,
                RegistrationPolicy::new()?,
                r.registry_out.clone(),
                Duration::from_secs(10),
                Arc::clone(metrics),
            );
            if h.refresh(true).is_none() {
                tracing::warn!("initial metadata refresh failed; starting with the given registry");
            }
            h
        }
        None => RegistryHandle::fixed(initial, Arc::clone(metrics)),
    };
    if handle.read().is_empty() {
        bail!("empty registry: give HL_REGISTRY or HL_INFO_URL");
    }
    Ok(Arc::new(handle))
}

fn pipeline_config(r: &ReaderSettings) -> Result<PipelineConfig> {
    if r.node_dirs.is_empty() || r.node_dirs.len() > 2 {
        bail!("give one or two node directories (HL_NODE_DIRS)");
    }
    let readers = r
        .node_dirs
        .iter()
        .zip(["a", "b"])
        .map(|(dir, name)| {
            let mut t = TailerConfig::new(name, dir.clone());
            t.dirs = r.dirs.clone();
            t.start = r.start;
            t.use_notify = r.use_notify;
            t.poll_interval = r.poll_interval;
            t.max_lag_blocks = r.max_lag_blocks;
            t.registry_refresh = r.registry_refresh;
            t
        })
        .collect();
    Ok(PipelineConfig {
        readers,
        merge: r.merge,
        checkpoint: r.checkpoint.clone(),
        checkpoint_interval: r.checkpoint_interval,
        channel_capacity: 1_024,
    })
}

async fn serve_metrics(
    listener: TcpListener,
    reader: Arc<Metrics>,
    origin: Arc<Metrics>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    use axum::routing::get;
    let app = axum::Router::new()
        .route(
            "/metrics",
            get(move || {
                let (r, o) = (Arc::clone(&reader), Arc::clone(&origin));
                async move {
                    (
                        [("content-type", "text/plain; version=0.0.4")],
                        crate::metrics::render(&r, &o),
                    )
                }
            }),
        )
        .route("/health", get(|| async { "ok\n" }));
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = stop.changed().await;
        })
        .await?;
    Ok(())
}

fn capture_follower(
    c: &CaptureSettings,
    dirs: &StreamDirs,
    node_dir: &std::path::Path,
) -> Result<(Follower, Option<UploadTargets>)> {
    let filtered = HourWriter::open(TierConfig::filtered(&c.staging))?;
    let private = if c.private {
        Some(HourWriter::open(TierConfig::private(&c.staging))?)
    } else {
        None
    };
    let capture = Capture::new(private, Some(filtered), crate::hooks::filters()?)?;
    let streams = hl_node_reader::Stream::ALL
        .into_iter()
        .map(|s| (s.as_str().to_owned(), node_dir.join(dirs.get(s))))
        .collect();
    let mut cfg = FollowConfig::new(streams, c.staging.clone());
    cfg.poll_interval = c.poll_interval;
    cfg.seal_grace = c.seal_grace;
    cfg.metrics_out = c.metrics_out.clone();
    let targets = if c.upload {
        let mut stores = Vec::new();
        for (on, tier, archive) in [
            (c.private, "PRIVATE", archive::layout::PRIVATE_RAW_ARCHIVE),
            (true, "PUBLIC", archive::layout::RAW_ARCHIVE),
        ] {
            if !on {
                continue;
            }
            let sc = StoreConfig::from_env(tier)
                .with_context(|| format!("BLEH_ARCHIVE_{tier}_URL is not set (capture upload)"))?;
            stores.push((archive.to_owned(), sc.open()?));
        }
        Some(UploadTargets {
            stores,
            keep_local: c.keep_local,
        })
    } else {
        None
    };
    Ok((Follower::new(cfg, capture)?, targets))
}

/// Starts every part of the origin. Must be called inside a multi-thread
/// Tokio runtime.
///
/// # Errors
///
/// Fails on bad settings, unreadable keys or registry, or listeners that
/// cannot bind.
pub async fn start(cfg: OriginConfig) -> Result<RunningOrigin> {
    let reader_metrics = Arc::new(Metrics::new());
    let origin_metrics = Arc::new(Metrics::new());
    let (stop_tx, stop_rx) = watch::channel(false);
    let mut tasks: tokio::task::JoinSet<Result<()>> = tokio::task::JoinSet::new();

    // Keys, signer, proofs, ring, publisher, side tap.
    if cfg.signing_keys.is_empty() {
        bail!("no signing key: set BLEH_SIGNING_KEY_FILE");
    }
    let keys = KeyRing::load(&cfg.signing_keys, cfg.active_key.as_deref())?;
    let signer = Arc::new(BlockSigner::new(keys)?);
    let signers = signer.signers();
    tracing::info!(signers = ?signers, "block signer ready");
    let proofs = Arc::new(ProofStore::new(cfg.proof_retention_blocks));
    let ring = bus::ring(bus::DEFAULT_RING_CAPACITY);
    let mut publisher =
        BlockPublisher::new(ring.clone()).with_signer(Arc::clone(&signer), Arc::clone(&proofs));
    let mut replay = Vec::new();
    if let Some(nats) = cfg.nats.clone() {
        let tap = bus::jetstream::JetStreamTap::connect(nats).await?;
        replay = tap.replay().await?;
        tracing::info!(events = replay.len(), "side tap replay window loaded");
        publisher = publisher.with_tap(Arc::new(tap));
    }

    // Gateway.
    let ring_stats = ring.clone();
    let pstats = publisher.stats();
    let gw = Gateway::new(
        cfg.gateway.clone(),
        ring.subscribe(),
        Signers::Local(Arc::clone(&signer)),
        Arc::clone(&proofs),
    )
    .with_replay(replay)
    .with_extra_metrics(Box::new(move || {
        use std::sync::atomic::Ordering::Relaxed;
        let r = ring_stats.stats();
        vec![
            (
                "bus_ring_published_total".into(),
                "Items published on the bus ring".into(),
                r.published,
            ),
            (
                "bus_ring_lag_events_total".into(),
                "Times a bus receiver lagged".into(),
                r.lag_events,
            ),
            (
                "signer_commitments_total".into(),
                "Commitments signed".into(),
                pstats.commitments.load(Relaxed),
            ),
            (
                "signer_sign_nanoseconds_total".into(),
                "Time spent building roots and signing".into(),
                pstats.sign_nanos.load(Relaxed),
            ),
            (
                "bus_records_published_total".into(),
                "Records published".into(),
                pstats.records.load(Relaxed),
            ),
        ]
    }));
    drop(ring);
    let gateway = gw.start();
    let listener = TcpListener::bind(cfg.gateway_listen)
        .await
        .with_context(|| format!("binding the gateway on {}", cfg.gateway_listen))?;
    let gateway_addr = listener.local_addr()?;
    tracing::info!(addr = %gateway_addr, "gateway listening on /v0/ws");
    {
        let g = gateway.clone();
        tasks.spawn(async move {
            g.serve(listener).await?;
            Ok(())
        });
    }

    // Metrics listener.
    let metrics_addr = match cfg.metrics_listen {
        Some(addr) => {
            let l = TcpListener::bind(addr)
                .await
                .with_context(|| format!("binding the metrics listener on {addr}"))?;
            let a = l.local_addr()?;
            tracing::info!(addr = %a, "metrics listening");
            tasks.spawn(serve_metrics(
                l,
                Arc::clone(&reader_metrics),
                Arc::clone(&origin_metrics),
                stop_rx.clone(),
            ));
            Some(a)
        }
        None => None,
    };

    // Live capture (first node directory).
    if let Some(c) = &cfg.capture {
        let node_dir = cfg
            .reader
            .node_dirs
            .first()
            .context("capture needs a node directory")?;
        let (follower, targets) = capture_follower(c, &cfg.reader.dirs, node_dir)?;
        let stop = stop_rx.clone();
        tasks.spawn(async move {
            archive::follow::run(follower, targets, stop).await?;
            Ok(())
        });
        tracing::info!(staging = %c.staging.display(), "live capture running");
    }

    // Reader pipeline and the publishing loop.
    let registry = registry_handle(&cfg.reader, &reader_metrics)?;
    let pipeline = Pipeline::spawn(
        pipeline_config(&cfg.reader)?,
        Arc::clone(&registry),
        Arc::clone(&reader_metrics),
    )?;
    let snapshots = spawn_snapshot_requests(&cfg.reader, &origin_metrics, stop_rx.clone())?;
    tasks.spawn(publish_loop(
        pipeline,
        publisher,
        Deriver::new(cfg.derive.clone()),
        cfg.derive.venue.clone(),
        registry,
        snapshots,
        Arc::clone(&origin_metrics),
        stop_rx,
    ));

    Ok(RunningOrigin {
        gateway_addr,
        metrics_addr,
        signers,
        gateway,
        reader_metrics,
        origin_metrics,
        stop: stop_tx,
        tasks,
    })
}

/// Requests an L4 snapshot every interval; texts arrive on the channel.
fn spawn_snapshot_requests(
    r: &ReaderSettings,
    metrics: &Arc<Metrics>,
    mut stop: watch::Receiver<bool>,
) -> Result<mpsc::Receiver<(String, UnixNanos)>> {
    let (tx, rx) = mpsc::channel(4);
    let Some(every) = r.snapshot_interval else {
        return Ok(rx);
    };
    let Some(url) = &r.info_url else {
        bail!("HL_SNAPSHOT_INTERVAL_SECS needs HL_INFO_URL");
    };
    let client = InfoClient::new(url, r.info_timeout).context("--info-url")?;
    let path = r.snapshot_path.clone();
    let metrics = Arc::clone(metrics);
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(every);
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = iv.tick() => {}
                _ = stop.changed() => return,
            }
            let (c, p) = (client.clone(), path.clone());
            match tokio::task::spawn_blocking(move || request_l4_snapshot(&c, &p)).await {
                Ok(Ok(text)) => {
                    metrics.inc(names::SNAPSHOTS, &[]);
                    if tx.send((text, UnixNanos::now())).await.is_err() {
                        return;
                    }
                }
                Ok(Err(e)) => {
                    metrics.inc(names::SNAPSHOT_FAILURES, &[]);
                    tracing::warn!(error = %e, "L4 snapshot request failed");
                }
                Err(e) => {
                    metrics.inc(names::SNAPSHOT_FAILURES, &[]);
                    tracing::warn!(error = %e, "L4 snapshot task failed");
                }
            }
        }
    });
    Ok(rx)
}

/// Snapshots wait until the output has passed their height, so they take
/// that block's time.
#[derive(Debug, Default)]
struct SnapshotQueue {
    pending: VecDeque<(String, UnixNanos)>,
    block_times: BTreeMap<u64, UnixMillis>,
}

impl SnapshotQueue {
    const KEEP_BLOCKS: usize = 20_000;

    fn note_block(&mut self, n: u64, t: UnixMillis) {
        self.block_times.insert(n, t);
        while self.block_times.len() > Self::KEEP_BLOCKS {
            self.block_times.pop_first();
        }
    }

    /// Parsed snapshots whose height the output has reached.
    fn ready(
        &mut self,
        last_block: Option<u64>,
        registry: &Registry,
        metrics: &Metrics,
    ) -> Vec<mesh_types::BookSnapshot> {
        let mut out = Vec::new();
        while let Some((text, fetched)) = self.pending.front() {
            let height = match parse_l4_envelope(text) {
                Ok(e) => e.height,
                Err(e) => {
                    tracing::error!(error = %e, "unreadable L4 snapshot dropped");
                    metrics.inc(names::SNAPSHOT_UNUSABLE, &[]);
                    self.pending.pop_front();
                    continue;
                }
            };
            if last_block.is_none_or(|b| b < height) {
                break;
            }
            let ts = self
                .block_times
                .range(..=height)
                .next_back()
                .map_or(fetched.to_millis(), |(_, t)| *t);
            match parse_l4_snapshot(text, registry, ts, *fetched) {
                Ok(p) => out.extend(p.snapshots),
                Err(e) => {
                    metrics.inc(names::SNAPSHOT_UNUSABLE, &[]);
                    tracing::error!(error = %e, "L4 snapshot rejected");
                }
            }
            self.pending.pop_front();
        }
        out
    }
}

fn count_notices(notices: &[Notice], metrics: &Metrics) {
    for n in notices {
        match n {
            Notice::Reconciled {
                instrument,
                clean,
                differences,
            } => {
                metrics.inc(names::RECONCILE, &[]);
                if !clean {
                    metrics.inc(names::RECONCILE_MISMATCH, &[]);
                    tracing::warn!(%instrument, ?differences, "book differed from the snapshot; reset");
                }
            }
            Notice::Desync {
                instrument,
                seq,
                reason,
            } => {
                let label = if reason == "gap" { "gap" } else { "book" };
                metrics.inc(names::DESYNC, &[("reason", label)]);
                tracing::warn!(%instrument, ?seq, reason, "book desynchronised");
            }
            Notice::SnapshotUnusable {
                instrument,
                block,
                reason,
            } => {
                metrics.inc(names::SNAPSHOT_UNUSABLE, &[]);
                tracing::warn!(%instrument, block, reason, "snapshot not used");
            }
            Notice::ProcessError { instrument, error } => {
                metrics.inc(names::PROCESS_ERRORS, &[]);
                tracing::error!(%instrument, error, "book processor error");
            }
        }
    }
}

/// Publishes one block: batches, derived records, resyncs, commitments.
fn publish_block(
    b: &BlockOutput,
    publisher: &mut BlockPublisher,
    deriver: &mut Deriver,
    venue: &VenueId,
    metrics: &Metrics,
) {
    for batch in &b.batches {
        if let Err(e) = publisher.publish_batch(batch.clone()) {
            metrics.inc(names::PUBLISH_ERRORS, &[]);
            tracing::error!(error = %e, instrument = %batch.instrument, "batch rejected");
        }
    }
    let derived = deriver.block(b);
    count_notices(&derived.notices, metrics);
    for r in derived.records {
        let kind = r.kind();
        match publisher.publish_record(r) {
            Ok(()) => metrics.inc(names::DERIVED, &[("kind", kind.as_str())]),
            Err(e) => {
                metrics.inc(names::PUBLISH_ERRORS, &[]);
                tracing::error!(error = %e, %kind, "record rejected");
            }
        }
    }
    for s in derived.resyncs {
        if let Err(e) = publisher.resync(s, ResyncReason::Reconcile) {
            metrics.inc(names::PUBLISH_ERRORS, &[]);
            tracing::error!(error = %e, "resync rejected");
        }
    }
    match publisher.end_block(venue, b.block_number, b.block_time_ms) {
        Ok(_) => {
            metrics.inc(names::BLOCKS, &[]);
            metrics.set(names::LAST_BLOCK, &[], b.block_number as f64);
        }
        Err(e) => {
            metrics.inc(names::SIGNING_FAILURES, &[]);
            tracing::error!(error = %e, block = b.block_number, "block not signed");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn publish_loop(
    mut pipeline: Pipeline,
    mut publisher: BlockPublisher,
    mut deriver: Deriver,
    venue: VenueId,
    registry: Arc<RegistryHandle>,
    mut snapshot_rx: mpsc::Receiver<(String, UnixNanos)>,
    metrics: Arc<Metrics>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let stop_flag = pipeline.stop_flag();
    let mut queue = SnapshotQueue::default();
    let mut snapshots_open = true;
    let mut stopping = false;
    loop {
        tokio::select! {
            ev = pipeline.events.recv() => match ev {
                Some(PipelineEvent::Block(b)) => {
                    queue.note_block(b.block_number, b.block_time_ms);
                    publish_block(&b, &mut publisher, &mut deriver, &venue, &metrics);
                    let ready = queue.ready(deriver.last_block(), &registry.read(), &metrics);
                    if !ready.is_empty() {
                        let out = deriver.snapshots(ready);
                        count_notices(&out.notices, &metrics);
                        for s in out.resyncs {
                            if let Err(e) = publisher.resync(s, ResyncReason::Reconcile) {
                                metrics.inc(names::PUBLISH_ERRORS, &[]);
                                tracing::error!(error = %e, "resync rejected");
                            }
                        }
                    }
                }
                Some(PipelineEvent::Alarm(a)) => {
                    tracing::warn!(alarm = ?a, "integrity alarm");
                }
                None => break,
            },
            s = snapshot_rx.recv(), if snapshots_open => match s {
                Some(s) => queue.pending.push_back(s),
                None => snapshots_open = false,
            },
            r = stop.changed(), if !stopping => {
                if r.is_err() || *stop.borrow() {
                    tracing::info!("stopping the reader");
                    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    stopping = true;
                }
            }
        }
    }
    pipeline.join().await;
    Ok(())
}
