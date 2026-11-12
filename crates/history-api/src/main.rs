//! `history-api`: the historical query service.
//!
//! ```text
//! history-api serve --archive /data/archive            # local archive
//! history-api serve --archive s3://bucket/prefix       # object store (httpfs)
//! history-api serve --archive https://cdn.example      # HTTPS via manifests
//! history-api demo                                     # synthetic archive + server
//! history-api synth --dir ./synthetic                  # synthetic archive only
//! ```
//!
//! Every flag also reads an environment variable (`HISTORY_API_*`); see
//! `docs/operations/history-api.md`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use history_api::config::{ArchiveRoot, Config, DEFAULT_ARCHIVE, Limits, RemoteSettings};
use history_api::http::{AppState, router};
use history_api::service::Service;
use history_api::synth;

#[derive(Debug, Parser)]
#[command(
    name = "history-api",
    version,
    about = "Historical query API for the bleh.xyz mesh"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the HTTP API (JSON and websocket replay).
    Serve {
        #[command(flatten)]
        archive: ArchiveArgs,
        #[command(flatten)]
        server: ServerArgs,
    },
    /// Generate a synthetic archive and serve it.
    Demo {
        /// Where to write the synthetic archive.
        #[arg(long, env = "HISTORY_API_DEMO_DIR")]
        dir: Option<PathBuf>,
        /// Hours to simulate.
        #[arg(long, default_value_t = 4)]
        hours: u32,
        /// RNG seed.
        #[arg(long, default_value_t = 7)]
        seed: u64,
        #[command(flatten)]
        server: ServerArgs,
        #[command(flatten)]
        limits: LimitArgs,
    },
    /// Generate a synthetic archive only.
    Synth {
        /// Output directory (receives `hyperliquid-normalized/` and `registry.json`).
        #[arg(long)]
        dir: PathBuf,
        /// Hours to simulate.
        #[arg(long, default_value_t = 4)]
        hours: u32,
        /// RNG seed.
        #[arg(long, default_value_t = 7)]
        seed: u64,
    },
}

#[derive(Debug, Args)]
struct ArchiveArgs {
    /// Archive root: a local directory, `s3://`/`r2://`/`gcs://` URL, or HTTPS base URL.
    #[arg(long, env = "HISTORY_API_ARCHIVE")]
    archive: String,
    /// Archive names under the root, unioned (repeat or comma-separate).
    #[arg(long = "archive-name", env = "HISTORY_API_ARCHIVE_NAMES", value_delimiter = ',', default_value = DEFAULT_ARCHIVE)]
    archive_names: Vec<String>,
    /// Registry file (mesh_types::Registry JSON); without it instruments are discovered from partitions.
    #[arg(long, env = "HISTORY_API_REGISTRY")]
    registry: Option<PathBuf>,
    /// Public base URL for links in /v0/manifests.
    #[arg(long, env = "HISTORY_API_PUBLIC_BASE_URL")]
    public_base_url: Option<String>,
    /// Directory of pre-downloaded DuckDB extensions (httpfs) for offline remote access.
    #[arg(long, env = "HISTORY_API_EXTENSION_DIR")]
    extension_dir: Option<PathBuf>,
    /// S3 endpoint (R2, MinIO), without scheme.
    #[arg(long, env = "HISTORY_API_S3_ENDPOINT")]
    s3_endpoint: Option<String>,
    /// S3 region.
    #[arg(long, env = "HISTORY_API_S3_REGION")]
    s3_region: Option<String>,
    /// Path-style S3 URLs.
    #[arg(long, env = "HISTORY_API_S3_URL_STYLE_PATH")]
    s3_url_style_path: bool,
    #[command(flatten)]
    limits: LimitArgs,
}

#[derive(Debug, Args, Clone)]
struct LimitArgs {
    /// Per-query timeout in milliseconds.
    #[arg(long, env = "HISTORY_API_QUERY_TIMEOUT_MS", default_value_t = 10_000)]
    query_timeout_ms: u64,
    /// DuckDB memory cap in MiB.
    #[arg(long, env = "HISTORY_API_MEMORY_LIMIT_MB", default_value_t = 1024)]
    memory_limit_mb: u64,
    /// DuckDB threads.
    #[arg(long, env = "HISTORY_API_THREADS", default_value_t = 4)]
    threads: u32,
    /// Queries running at once.
    #[arg(long, env = "HISTORY_API_MAX_CONCURRENT_QUERIES", default_value_t = 8)]
    max_concurrent_queries: usize,
    /// Default page size.
    #[arg(long, env = "HISTORY_API_DEFAULT_ROWS", default_value_t = 1_000)]
    default_rows: usize,
    /// Largest page size.
    #[arg(long, env = "HISTORY_API_MAX_ROWS", default_value_t = 10_000)]
    max_rows: usize,
    /// Widest query range in days.
    #[arg(long, env = "HISTORY_API_MAX_RANGE_DAYS", default_value_t = 31)]
    max_range_days: u64,
    /// Checkpoint lookback for book at time T, in hours.
    #[arg(
        long,
        env = "HISTORY_API_CHECKPOINT_LOOKBACK_HOURS",
        default_value_t = 24
    )]
    checkpoint_lookback_hours: u64,
    /// Most book events applied on a checkpoint.
    #[arg(long, env = "HISTORY_API_MAX_BOOK_EVENTS", default_value_t = 2_000_000)]
    max_book_events: usize,
    /// Widest replay in days.
    #[arg(long, env = "HISTORY_API_MAX_REPLAY_DAYS", default_value_t = 7)]
    max_replay_days: u64,
    /// Concurrent replays per IP.
    #[arg(long, env = "HISTORY_API_MAX_REPLAYS_PER_IP", default_value_t = 2)]
    max_replays_per_ip: usize,
    /// Most records per Databento-shaped request.
    #[arg(long, env = "HISTORY_API_MAX_DBN_RECORDS", default_value_t = 1_000_000)]
    max_dbn_records: usize,
    /// Requests per second per IP (0 disables).
    #[arg(long, env = "HISTORY_API_RATE_PER_SEC", default_value_t = 20.0)]
    rate_per_sec: f64,
    /// Request burst per IP.
    #[arg(long, env = "HISTORY_API_RATE_BURST", default_value_t = 50.0)]
    rate_burst: f64,
}

impl LimitArgs {
    fn limits(&self) -> Limits {
        const DAY: u64 = 86_400;
        Limits {
            query_timeout: Duration::from_millis(self.query_timeout_ms),
            memory_limit_bytes: self.memory_limit_mb.saturating_mul(1 << 20),
            threads: self.threads,
            max_concurrent_queries: self.max_concurrent_queries,
            default_rows: self.default_rows,
            max_rows: self.max_rows,
            max_range: Duration::from_secs(self.max_range_days * DAY),
            max_coverage_range: Duration::from_secs(self.max_range_days * DAY),
            max_checkpoint_lookback: Duration::from_secs(self.checkpoint_lookback_hours * 3_600),
            max_book_events: self.max_book_events,
            max_replay_range: Duration::from_secs(self.max_replay_days * DAY),
            max_replays_per_ip: self.max_replays_per_ip,
            max_dbn_records: self.max_dbn_records,
            rate_per_sec: self.rate_per_sec,
            rate_burst: self.rate_burst,
            ..Limits::default()
        }
    }
}

#[derive(Debug, Args, Clone)]
struct ServerArgs {
    /// Listen address.
    #[arg(long, env = "HISTORY_API_BIND", default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
    /// Rate-limit by the first X-Forwarded-For address (only behind a proxy that sets it).
    #[arg(long, env = "HISTORY_API_TRUST_FORWARDED_FOR")]
    trust_forwarded_for: bool,
    /// Host header values the MCP endpoint accepts (repeat; empty list disables the check).
    #[arg(long = "mcp-allowed-host", env = "HISTORY_API_MCP_ALLOWED_HOSTS", value_delimiter = ',', default_values_t = ["localhost".to_owned(), "127.0.0.1".to_owned(), "::1".to_owned()])]
    mcp_allowed_hosts: Vec<String>,
    /// Log format.
    #[arg(long, env = "HISTORY_API_LOG_FORMAT", default_value = "text", value_parser = ["text", "json"])]
    log_format: String,
}

impl ArchiveArgs {
    fn config(&self) -> Config {
        Config {
            root: ArchiveRoot::parse(&self.archive),
            archives: self.archive_names.clone(),
            registry_file: self.registry.clone(),
            public_base_url: self.public_base_url.clone(),
            remote: RemoteSettings {
                extension_dir: self.extension_dir.clone(),
                s3_endpoint: self.s3_endpoint.clone(),
                s3_region: self.s3_region.clone(),
                s3_url_style_path: self.s3_url_style_path,
            },
            limits: self.limits.limits(),
            ..Config::local(".")
        }
    }
}

fn init_logging(json: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
    tracing::info!("shutting down");
}

async fn serve(mut config: Config, server: &ServerArgs) -> anyhow::Result<()> {
    config.trust_forwarded_for = server.trust_forwarded_for;
    config.mcp_allowed_hosts = server.mcp_allowed_hosts.clone();
    let root = config.root.display();
    let svc = Service::open(config).context("opening the archive")?;
    let app = router(AppState::new(svc));
    let listener = tokio::net::TcpListener::bind(server.bind)
        .await
        .with_context(|| format!("binding {}", server.bind))?;
    tracing::info!(addr = %server.bind, archive = %root, "history-api listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

fn generate(dir: &std::path::Path, hours: u32, seed: u64) -> anyhow::Result<synth::WriteSummary> {
    let cfg = synth::SynthConfig {
        hours,
        seed,
        ..synth::SynthConfig::default()
    };
    let data = synth::generate(&cfg)?;
    let summary = synth::write_archive(dir, DEFAULT_ARCHIVE, &data)?;
    Ok(summary)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { archive, server } => {
            init_logging(server.log_format == "json");
            serve(archive.config(), &server).await
        }
        Command::Demo {
            dir,
            hours,
            seed,
            server,
            limits,
        } => {
            init_logging(server.log_format == "json");
            let dir = dir.unwrap_or_else(|| std::env::temp_dir().join("bleh-history-demo"));
            if dir.join(DEFAULT_ARCHIVE).exists() {
                std::fs::remove_dir_all(dir.join(DEFAULT_ARCHIVE))
                    .with_context(|| format!("clearing {}", dir.display()))?;
            }
            let summary = generate(&dir, hours, seed)?;
            let start = synth::SynthConfig::default().start_ms;
            let t = mesh_types::UnixMillis(start).to_rfc3339();
            eprintln!(
                "Synthetic demo archive (invented data, fictional markets): {}\n  {} files, {} records, days {:?}\n\nTry:\n  curl 'http://{b}/v0/instruments'\n  curl 'http://{b}/v0/trades?instrument=hyperliquid:perp:SYN&start={t}&limit=5'\n  curl 'http://{b}/v0/bars?instrument=hyperliquid:perp:SYN&interval=5m&start={t}'\n  curl 'http://{b}/v0/book?instrument=hyperliquid:perp:SYN&at={t}&depth=l2&levels=5'\n  curl 'http://{b}/v0/coverage?instrument=hyperliquid:perp:syn:IDX&start={t}&end=2026-01-02T02:00:00Z'\n",
                dir.display(),
                summary.files.len(),
                summary.records,
                summary.days,
                b = server.bind,
            );
            let mut config = Config::local(&dir);
            config.registry_file = Some(summary.registry_file);
            config.limits = limits.limits();
            serve(config, &server).await
        }
        Command::Synth { dir, hours, seed } => {
            init_logging(false);
            let summary = generate(&dir, hours, seed)?;
            eprintln!(
                "wrote {} files ({} records) and {}",
                summary.files.len(),
                summary.records,
                summary.registry_file.display()
            );
            Ok(())
        }
    }
}
