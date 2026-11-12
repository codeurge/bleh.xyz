//! `hl-node-reader`: tails one or two Hyperliquid nodes (or replays a data
//! directory once) and writes mesh block batches as JSON lines.
//!
//! See `docs/operations/node-reader.md` for configuration, metrics and
//! failure modes.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use hl_node_reader::files::{LineReader, StartPosition};
use hl_node_reader::filter::{StreamDirs, filter_line};
use hl_node_reader::http::InfoClient;
use hl_node_reader::markets::{RegistrationPolicy, RegistryHandle};
use hl_node_reader::merge::{IntegrityAlarm, MergeConfig};
use hl_node_reader::metrics::Metrics;
use hl_node_reader::pipeline::{Pipeline, PipelineConfig, PipelineEvent};
use hl_node_reader::raw::Stream;
use hl_node_reader::replay::Replay;
use hl_node_reader::sequence::BlockOutput;
use hl_node_reader::snapshot::{parse_l4_snapshot, request_l4_snapshot};
use hl_node_reader::tail::TailerConfig;
use mesh_types::{BookSnapshot, Registry, UnixMillis, UnixNanos};
use serde::Serialize;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StartArg {
    Earliest,
    LatestFile,
    End,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum OutputArg {
    /// JSON lines on standard output.
    Stdout,
    /// Nothing (metrics and logs only).
    None,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum LogFormat {
    Text,
    Json,
}

/// Hyperliquid node reader for the bleh.xyz mesh.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Node data directory (`~/hl/data`). Give two (comma-separated or
    /// repeated) for active-active ingest from two nodes.
    #[arg(long = "node-dir", env = "HL_NODE_DIRS", value_delimiter = ',')]
    node_dirs: Vec<PathBuf>,

    /// Read this data directory once (raw or filtered raw) and exit.
    #[arg(long, env = "HL_REPLAY", conflicts_with = "node_dirs")]
    replay: Option<PathBuf>,

    /// With --replay: also write the filtered-raw tree here.
    #[arg(long, env = "HL_WRITE_FILTERED", requires = "replay")]
    write_filtered: Option<PathBuf>,

    /// Instrument registry JSON (mesh-types `Registry`) to start from.
    #[arg(long, env = "HL_REGISTRY")]
    registry: Option<PathBuf>,

    /// Write the registry here whenever auto-registration adds markets.
    #[arg(long, env = "HL_REGISTRY_OUT")]
    registry_out: Option<PathBuf>,

    /// Node info server (`--serve-info`) for metadata and L4 snapshots.
    #[arg(long, env = "HL_INFO_URL")]
    info_url: Option<String>,

    /// Info server request timeout, milliseconds.
    #[arg(long, env = "HL_INFO_TIMEOUT_MS", default_value_t = 10_000)]
    info_timeout_ms: u64,

    /// Periodic registry refresh from metadata, seconds (0: only when an
    /// unknown coin appears).
    #[arg(long, env = "HL_REGISTRY_REFRESH_SECS", default_value_t = 300)]
    registry_refresh_secs: u64,

    /// Checkpoint file (resume position and stream links).
    #[arg(long, env = "HL_CHECKPOINT")]
    checkpoint: Option<PathBuf>,

    /// Minimum seconds between checkpoint writes.
    #[arg(long, env = "HL_CHECKPOINT_INTERVAL_SECS", default_value_t = 5)]
    checkpoint_interval_secs: u64,

    /// Where to start without a checkpoint.
    #[arg(long, env = "HL_START", value_enum, default_value = "latest-file")]
    start: StartArg,

    /// Fills directory name under each node directory.
    #[arg(long, env = "HL_FILLS_DIR", default_value = hl_node_reader::assumptions::DEFAULT_FILLS_DIR)]
    fills_dir: String,

    /// Order-status directory name.
    #[arg(long, env = "HL_ORDER_STATUSES_DIR", default_value = hl_node_reader::assumptions::DEFAULT_ORDER_STATUSES_DIR)]
    order_statuses_dir: String,

    /// Raw-book-diff directory name.
    #[arg(long, env = "HL_RAW_BOOK_DIFFS_DIR", default_value = hl_node_reader::assumptions::DEFAULT_RAW_BOOK_DIFFS_DIR)]
    raw_book_diffs_dir: String,

    /// Disable filesystem notifications (poll only).
    #[arg(long, env = "HL_NO_NOTIFY")]
    no_notify: bool,

    /// Poll interval without notifications, milliseconds.
    #[arg(long, env = "HL_POLL_INTERVAL_MS", default_value_t = 50)]
    poll_interval_ms: u64,

    /// Release blocks without a stream that lags this many blocks.
    #[arg(long, env = "HL_MAX_LAG_BLOCKS", default_value_t = 600)]
    max_lag_blocks: u64,

    /// Active-active: hold blocks after a hole this long for the other node.
    #[arg(long, env = "HL_GAP_WAIT_MS", default_value_t = 1_000)]
    gap_wait_ms: u64,

    /// Active-active: blocks a node may lag before its copies count as missing.
    #[arg(long, env = "HL_CONFIRM_WINDOW_BLOCKS", default_value_t = 100)]
    confirm_window_blocks: u64,

    /// Active-active: seconds of silence before a node is reported stalled.
    #[arg(long, env = "HL_STALL_AFTER_SECS", default_value_t = 10)]
    stall_after_secs: u64,

    /// Request an L4 snapshot this often, seconds (0: never).
    #[arg(long, env = "HL_SNAPSHOT_INTERVAL_SECS", default_value_t = 0)]
    snapshot_interval_secs: u64,

    /// File the node writes snapshots to (must be readable here).
    #[arg(
        long,
        env = "HL_SNAPSHOT_PATH",
        default_value = "/tmp/hl-node-reader-l4.json"
    )]
    snapshot_path: PathBuf,

    /// Metrics and health listener.
    #[arg(long, env = "HL_METRICS_ADDR", default_value = "127.0.0.1:9184")]
    metrics_addr: SocketAddr,

    /// Output.
    #[arg(long, env = "HL_OUTPUT", value_enum, default_value = "stdout")]
    output: OutputArg,

    /// Log format (logs go to standard error).
    #[arg(long, env = "HL_LOG_FORMAT", value_enum, default_value = "text")]
    log_format: LogFormat,
}

impl Cli {
    fn dirs(&self) -> StreamDirs {
        StreamDirs {
            fills: self.fills_dir.clone(),
            order_statuses: self.order_statuses_dir.clone(),
            raw_book_diffs: self.raw_book_diffs_dir.clone(),
        }
    }

    fn info_client(&self) -> Result<Option<InfoClient>> {
        self.info_url
            .as_deref()
            .map(|u| InfoClient::new(u, Duration::from_millis(self.info_timeout_ms)))
            .transpose()
            .context("--info-url")
    }
}

/// One output line.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputLine<'a> {
    Block(&'a BlockOutput),
    Snapshot(&'a BookSnapshot),
    Alarm(&'a IntegrityAlarm),
}

struct Output {
    enabled: bool,
    out: std::io::BufWriter<std::io::Stdout>,
}

impl Output {
    fn write(&mut self, line: &OutputLine<'_>) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        serde_json::to_writer(&mut self.out, line)?;
        self.out.write_all(b"\n")?;
        self.out.flush()?;
        Ok(())
    }
}

fn init_logs(format: LogFormat) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let b = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match format {
        LogFormat::Text => b.init(),
        LogFormat::Json => b.json().init(),
    }
}

fn load_registry(path: Option<&Path>) -> Result<Registry> {
    match path {
        Some(p) => {
            let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", p.display()))
        }
        None => Ok(Registry::new()),
    }
}

fn registry_handle(cli: &Cli, metrics: &Arc<Metrics>) -> Result<Arc<RegistryHandle>> {
    let initial = load_registry(cli.registry.as_deref())?;
    let handle = match cli.info_client()? {
        Some(client) => {
            let h = RegistryHandle::refreshing(
                initial,
                client,
                RegistrationPolicy::new()?,
                cli.registry_out.clone(),
                Duration::from_secs(10),
                metrics.clone(),
            );
            if h.refresh(true).is_none() {
                tracing::warn!("initial metadata refresh failed; starting with the given registry");
            }
            h
        }
        None => RegistryHandle::fixed(initial, metrics.clone()),
    };
    if handle.read().is_empty() {
        bail!("empty registry: give --registry or --info-url");
    }
    Ok(Arc::new(handle))
}

fn write_filtered(src: &Path, dst: &Path, dirs: &StreamDirs) -> Result<()> {
    for s in Stream::ALL {
        let mut current: Option<(String, std::io::BufWriter<std::fs::File>)> = None;
        for line in LineReader::open(&src.join(dirs.get(s)))? {
            let line = line?;
            if current.as_ref().map(|(f, _)| f.as_str()) != Some(line.pos.file.as_str()) {
                if let Some((_, mut w)) = current.take() {
                    w.flush()?;
                }
                let path = dst.join(dirs.get(s)).join(&line.pos.file);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                current = Some((
                    line.pos.file.clone(),
                    std::io::BufWriter::new(std::fs::File::create(path)?),
                ));
            }
            let filtered = filter_line(s, &line.text)
                .with_context(|| format!("{s} {}@{}", line.pos.file, line.pos.offset))?;
            if let Some((_, w)) = current.as_mut() {
                w.write_all(filtered.as_bytes())?;
                w.write_all(b"\n")?;
            }
        }
        if let Some((_, mut w)) = current.take() {
            w.flush()?;
        }
    }
    Ok(())
}

fn run_replay(cli: &Cli, dir: &Path, out: &mut Output) -> Result<()> {
    let metrics = Arc::new(Metrics::new());
    let registry = registry_handle(cli, &metrics)?.snapshot();
    let dirs = cli.dirs();
    if let Some(dst) = &cli.write_filtered {
        write_filtered(dir, dst, &dirs)?;
    }
    let mut errors = 0u64;
    let mut blocks = 0u64;
    for item in Replay::open(dir, &dirs, registry)?.sequenced() {
        match item {
            Ok(b) => {
                blocks += 1;
                out.write(&OutputLine::Block(&b))?;
            }
            Err(e) => {
                errors += 1;
                tracing::error!(error = %e, "replay problem");
            }
        }
    }
    tracing::info!(blocks, errors, "replay done");
    if errors > 0 {
        bail!("{errors} lines could not be replayed");
    }
    Ok(())
}

async fn serve_metrics(addr: SocketAddr, metrics: Arc<Metrics>) -> Result<()> {
    use axum::routing::get;
    let m = metrics.clone();
    let app = axum::Router::new()
        .route(
            "/metrics",
            get(move || {
                let m = m.clone();
                async move { ([("content-type", "text/plain; version=0.0.4")], m.render()) }
            }),
        )
        .route("/health", get(|| async { "ok\n" }));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding metrics listener {addr}"))?;
    tracing::info!(%addr, "metrics listening");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Snapshots wait until the output has passed their height, so they follow
/// the block's batches and take that block's time.
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

    fn drain(
        &mut self,
        last_block: Option<u64>,
        registry: &RegistryHandle,
        out: &mut Output,
    ) -> Result<()> {
        while let Some((text, fetched)) = self.pending.front() {
            let height = match hl_node_reader::snapshot::parse_l4_envelope(text) {
                Ok(e) => e.height,
                Err(e) => {
                    tracing::error!(error = %e, "unreadable L4 snapshot dropped");
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
            match parse_l4_snapshot(text, &registry.read(), ts, *fetched) {
                Ok(p) => {
                    for s in &p.snapshots {
                        out.write(&OutputLine::Snapshot(s))?;
                    }
                }
                Err(e) => tracing::error!(error = %e, "L4 snapshot rejected"),
            }
            self.pending.pop_front();
        }
        Ok(())
    }
}

async fn run_live(cli: Cli, mut out: Output) -> Result<()> {
    if cli.node_dirs.is_empty() || cli.node_dirs.len() > 2 {
        bail!("give one or two --node-dir (or --replay)");
    }
    let metrics = Arc::new(Metrics::new());
    let registry = registry_handle(&cli, &metrics)?;
    let start = match cli.start {
        StartArg::Earliest => StartPosition::Earliest,
        StartArg::LatestFile => StartPosition::LatestFile,
        StartArg::End => StartPosition::End,
    };
    let readers = cli
        .node_dirs
        .iter()
        .zip(["a", "b"])
        .map(|(dir, name)| {
            let mut t = TailerConfig::new(name, dir.clone());
            t.dirs = cli.dirs();
            t.start = start;
            t.use_notify = !cli.no_notify;
            t.poll_interval = Duration::from_millis(cli.poll_interval_ms);
            t.max_lag_blocks = cli.max_lag_blocks;
            t.registry_refresh = (cli.registry_refresh_secs > 0)
                .then(|| Duration::from_secs(cli.registry_refresh_secs));
            t
        })
        .collect();
    let cfg = PipelineConfig {
        readers,
        merge: MergeConfig {
            gap_wait: Duration::from_millis(cli.gap_wait_ms),
            confirm_window_blocks: cli.confirm_window_blocks,
            stall_after: Duration::from_secs(cli.stall_after_secs),
        },
        checkpoint: cli.checkpoint.clone(),
        checkpoint_interval: Duration::from_secs(cli.checkpoint_interval_secs),
        channel_capacity: 1_024,
    };
    tokio::spawn({
        let m = metrics.clone();
        let addr = cli.metrics_addr;
        async move {
            if let Err(e) = serve_metrics(addr, m).await {
                tracing::error!(error = %e, "metrics server stopped");
            }
        }
    });

    let (snap_tx, mut snap_rx) = tokio::sync::mpsc::channel::<(String, UnixNanos)>(4);
    if cli.snapshot_interval_secs > 0 {
        let Some(client) = cli.info_client()? else {
            bail!("--snapshot-interval-secs needs --info-url");
        };
        let path = cli.snapshot_path.clone();
        let every = Duration::from_secs(cli.snapshot_interval_secs);
        tokio::spawn(async move {
            let mut iv = tokio::time::interval(every);
            loop {
                iv.tick().await;
                let (c, p) = (client.clone(), path.clone());
                match tokio::task::spawn_blocking(move || request_l4_snapshot(&c, &p)).await {
                    Ok(Ok(text)) => {
                        if snap_tx.send((text, UnixNanos::now())).await.is_err() {
                            return;
                        }
                    }
                    Ok(Err(e)) => tracing::warn!(error = %e, "L4 snapshot request failed"),
                    Err(e) => tracing::warn!(error = %e, "L4 snapshot task failed"),
                }
            }
        });
    } else {
        drop(snap_tx);
    }

    let mut pipeline = Pipeline::spawn(cfg, registry.clone(), metrics)?;
    let stop = pipeline.stop_flag();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("stopping");
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    let mut snaps = SnapshotQueue {
        pending: VecDeque::new(),
        block_times: BTreeMap::new(),
    };
    let mut last_block = None;
    loop {
        tokio::select! {
            ev = pipeline.events.recv() => match ev {
                Some(PipelineEvent::Block(b)) => {
                    snaps.note_block(b.block_number, b.block_time_ms);
                    last_block = Some(b.block_number);
                    out.write(&OutputLine::Block(&b))?;
                    snaps.drain(last_block, &registry, &mut out)?;
                }
                Some(PipelineEvent::Alarm(a)) => out.write(&OutputLine::Alarm(&a))?,
                None => break,
            },
            Some(s) = snap_rx.recv() => {
                snaps.pending.push_back(s);
                snaps.drain(last_block, &registry, &mut out)?;
            }
        }
    }
    pipeline.join().await;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logs(cli.log_format);
    let mut out = Output {
        enabled: cli.output == OutputArg::Stdout,
        out: std::io::BufWriter::new(std::io::stdout()),
    };
    if let Some(dir) = cli.replay.clone() {
        return run_replay(&cli, &dir, &mut out);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_live(cli, out))
}
