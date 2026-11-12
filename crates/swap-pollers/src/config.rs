//! Command line and environment configuration, and the service entry point
//! the binary (or an embedding gateway) calls.
//!
//! Every option can also be set through an environment variable named
//! `SWAP_POLLERS_<OPTION>` (for example `SWAP_POLLERS_RAW_DIR`). S3
//! credentials come from the standard `AWS_*` environment variables.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use mesh_types::{SwapPrint, UnixMillis};
use tokio::sync::{mpsc, watch};

use crate::checkpoint::CheckpointStore;
use crate::dedupe::{self, SeenStore};
use crate::http::{Fetcher, HttpConfig};
use crate::metrics::Metrics;
use crate::output::{Emitter, JsonlWriter};
use crate::poller::{DtccConfig, DtccPoller};
use crate::raw::{Mirror, RawStore};
use crate::repository::DtccBucket;
use crate::server::Health;

/// Swap dissemination pollers (CFTC Part 43, SEC SBSR).
#[derive(Debug, Clone, Parser)]
#[command(name = "swap-pollers", version, about)]
pub struct Args {
    /// Directory for verbatim raw report files.
    #[arg(long, env = "SWAP_POLLERS_RAW_DIR", default_value = "data/swap/raw")]
    pub raw_dir: PathBuf,

    /// Directory for the checkpoint and dedupe state.
    #[arg(
        long,
        env = "SWAP_POLLERS_STATE_DIR",
        default_value = "data/swap/state"
    )]
    pub state_dir: PathBuf,

    /// Directory for daily JSON lines of SwapPrint records.
    #[arg(
        long,
        env = "SWAP_POLLERS_OUT_DIR",
        default_value = "data/swap/records"
    )]
    pub out_dir: PathBuf,

    /// Do not write JSON lines (records go to the gateway channel only).
    #[arg(long, env = "SWAP_POLLERS_NO_JSONL")]
    pub no_jsonl: bool,

    /// Address for /metrics and /health.
    #[arg(long, env = "SWAP_POLLERS_LISTEN", default_value = "127.0.0.1:9187")]
    pub listen: SocketAddr,

    /// Buckets to capture, comma separated (CFTC_IR, CFTC_CR, CFTC_EQ,
    /// CFTC_FX, CFTC_CO, SEC_IR, SEC_CR, SEC_EQ). Default: all.
    #[arg(long, env = "SWAP_POLLERS_BUCKETS", value_delimiter = ',')]
    pub buckets: Vec<DtccBucket>,

    /// Seconds between slice listing polls.
    #[arg(long, env = "SWAP_POLLERS_SLICE_INTERVAL", default_value_t = 60)]
    pub slice_interval: u64,

    /// Seconds between end-of-day listing polls.
    #[arg(long, env = "SWAP_POLLERS_CUMULATIVE_INTERVAL", default_value_t = 3600)]
    pub cumulative_interval: u64,

    /// Days of end-of-day files to capture (0 disables; DTCC keeps 366).
    #[arg(long, env = "SWAP_POLLERS_CUMULATIVE_DAYS", default_value_t = 7)]
    pub cumulative_days: u32,

    /// Days of emitted dissemination IDs kept for dedupe.
    #[arg(long, env = "SWAP_POLLERS_DEDUPE_DAYS", default_value_t = 3)]
    pub dedupe_days: u32,

    /// DTCC PPD API base.
    #[arg(
        long,
        env = "SWAP_POLLERS_DTCC_API_BASE",
        default_value = "https://pddata.dtcc.com/ppd/api"
    )]
    pub dtcc_api_base: String,

    /// DTCC public data base URL; resolved from the API when unset.
    #[arg(long, env = "SWAP_POLLERS_DTCC_DATA_BASE")]
    pub dtcc_data_base: Option<String>,

    /// User-Agent header.
    #[arg(long, env = "SWAP_POLLERS_USER_AGENT", default_value = concat!("swap-pollers/", env!("CARGO_PKG_VERSION")))]
    pub user_agent: String,

    /// Minimum milliseconds between request starts.
    #[arg(
        long,
        env = "SWAP_POLLERS_MIN_REQUEST_INTERVAL_MS",
        default_value_t = 250
    )]
    pub min_request_interval_ms: u64,

    /// Retries for transient HTTP failures.
    #[arg(long, env = "SWAP_POLLERS_MAX_RETRIES", default_value_t = 5)]
    pub max_retries: u32,

    /// Per-request timeout, seconds (large end-of-day files need minutes).
    #[arg(long, env = "SWAP_POLLERS_REQUEST_TIMEOUT", default_value_t = 600)]
    pub request_timeout: u64,

    /// S3 bucket to mirror raw files to (optional).
    #[arg(long, env = "SWAP_POLLERS_S3_BUCKET")]
    pub s3_bucket: Option<String>,

    /// Key prefix in the S3 bucket.
    #[arg(long, env = "SWAP_POLLERS_S3_PREFIX", default_value = "swap-raw")]
    pub s3_prefix: String,

    /// S3-compatible endpoint (for non-AWS storage).
    #[arg(long, env = "SWAP_POLLERS_S3_ENDPOINT")]
    pub s3_endpoint: Option<String>,

    /// S3 region.
    #[arg(long, env = "SWAP_POLLERS_S3_REGION")]
    pub s3_region: Option<String>,

    /// Run one poll cycle and exit.
    #[arg(long)]
    pub once: bool,

    /// Parse a local report file (zip or CSV) and print records as JSON
    /// lines to stdout; no network, no state.
    #[arg(long, value_name = "FILE")]
    pub parse: Option<PathBuf>,

    /// Bucket of the `--parse` file when its name does not say.
    #[arg(long, requires = "parse")]
    pub bucket: Option<DtccBucket>,

    /// Log as JSON.
    #[arg(long, env = "SWAP_POLLERS_LOG_JSON")]
    pub log_json: bool,
}

impl Args {
    /// HTTP settings.
    #[must_use]
    pub fn http_config(&self) -> HttpConfig {
        HttpConfig {
            user_agent: self.user_agent.clone(),
            min_interval: Duration::from_millis(self.min_request_interval_ms),
            timeout: Duration::from_secs(self.request_timeout.max(1)),
            max_retries: self.max_retries,
            ..HttpConfig::default()
        }
    }

    /// DTCC poller settings.
    #[must_use]
    pub fn dtcc_config(&self) -> DtccConfig {
        DtccConfig {
            api_base: self.dtcc_api_base.clone(),
            data_base: self.dtcc_data_base.clone(),
            buckets: if self.buckets.is_empty() {
                DtccBucket::ADMITTED.to_vec()
            } else {
                self.buckets.clone()
            },
            slice_interval: Duration::from_secs(self.slice_interval.max(5)),
            cumulative_interval: Duration::from_secs(self.cumulative_interval.max(60)),
            cumulative_days: self.cumulative_days.min(366),
            slice_retention_days: 3,
        }
    }

    /// The object-store mirror, if configured.
    ///
    /// # Errors
    ///
    /// Fails if the S3 client cannot be built.
    pub fn mirror(&self) -> anyhow::Result<Option<Mirror>> {
        let Some(bucket) = &self.s3_bucket else {
            return Ok(None);
        };
        let mut b = object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);
        if let Some(e) = &self.s3_endpoint {
            b = b.with_endpoint(e).with_allow_http(e.starts_with("http://"));
        }
        if let Some(r) = &self.s3_region {
            b = b.with_region(r);
        }
        let store = b.build()?;
        Ok(Some(Mirror::new(Arc::new(store), self.s3_prefix.clone())))
    }
}

/// Builds and runs the pollers and the metrics server until `shutdown`
/// turns true (or after one cycle with `--once`). Records go to the daily
/// JSON lines and, when given, to `sink` for the gateway.
///
/// # Errors
///
/// Fails on configuration, state or bind errors, or a failed `--once`
/// cycle.
pub async fn run(
    args: &Args,
    sink: Option<mpsc::Sender<SwapPrint>>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let metrics = Arc::new(Metrics::new());
    let dtcc = args.dtcc_config();
    let health = Arc::new(Health::new(
        dtcc.slice_interval * 5 + Duration::from_secs(300),
    ));

    let fetcher = Fetcher::new(args.http_config(), metrics.clone())?;
    let raw = RawStore::new(&args.raw_dir, args.mirror()?, metrics.clone());
    let checkpoint = CheckpointStore::open(args.state_dir.join("checkpoint.json"))?;
    let today = dedupe::day_of(UnixMillis::now().0);
    let seen = SeenStore::open(args.state_dir.join("seen"), args.dedupe_days.max(1), &today)?;
    let writer = (!args.no_jsonl).then(|| JsonlWriter::new(&args.out_dir));
    let emitter = Emitter::new(seen, writer, sink, metrics.clone());
    let mut poller = DtccPoller::new(
        dtcc,
        fetcher,
        raw,
        checkpoint,
        emitter,
        metrics.clone(),
        health.clone(),
    );

    if args.once {
        return poller.run(true, shutdown).await;
    }
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let mut server_shutdown = shutdown.clone();
    let server = tokio::spawn(crate::server::serve(
        listener,
        metrics,
        health,
        async move {
            while !*server_shutdown.borrow() {
                if server_shutdown.changed().await.is_err() {
                    break;
                }
            }
        },
    ));
    let result = poller.run(false, shutdown).await;
    match server.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!(error = %e, "metrics server failed"),
        Err(e) => tracing::error!(error = %e, "metrics server task failed"),
    }
    result
}
