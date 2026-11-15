//! Command line and environment configuration, and the service entry
//! points the binary (or an embedding process) calls.
//!
//! Every global option can also be set through an environment variable
//! named `REFERENCE_POLLERS_<OPTION>` (for example
//! `REFERENCE_POLLERS_RAW_DIR`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use mesh_types::{Record, UnixMillis};
use tokio::sync::{Mutex, mpsc, watch};

use crate::backfill::{self, BACKFILL_ARCHIVE, Fetch};
use crate::http::{Fetcher, HttpConfig};
use crate::live::SourcePoller;
use crate::metrics::{Health, Metrics};
use crate::output::{Emitter, JsonlWriter, write_notices};
use crate::raw::RawStore;
use crate::source::SourceId;
use crate::sources::{Bases, reader};
use crate::time::Date;

/// Pollers and history backfill for admitted public reference sources.
#[derive(Debug, Clone, Parser)]
#[command(name = "reference-pollers", version, about)]
pub struct Args {
    /// Sources, comma separated (default, or empty: every admitted
    /// source).
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_SOURCES",
        value_delimiter = ','
    )]
    pub sources: Vec<String>,

    /// Directory for verbatim raw responses.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_RAW_DIR",
        default_value = "data/reference/raw"
    )]
    pub raw_dir: PathBuf,

    /// Directory for poll, revision and backfill state.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_STATE_DIR",
        default_value = "data/reference/state"
    )]
    pub state_dir: PathBuf,

    /// Directory for daily JSON lines of records.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_OUT_DIR",
        default_value = "data/reference/records"
    )]
    pub out_dir: PathBuf,

    /// Do not write JSON lines (records go to the channel only).
    #[arg(long, global = true, env = "REFERENCE_POLLERS_NO_JSONL")]
    pub no_jsonl: bool,

    /// Address for /metrics and /health.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_LISTEN",
        default_value = "127.0.0.1:9188"
    )]
    pub listen: SocketAddr,

    /// User-Agent header (generic; never a person's name or address).
    #[arg(long, global = true, env = "REFERENCE_POLLERS_USER_AGENT", default_value = concat!("reference-pollers/", env!("CARGO_PKG_VERSION")))]
    pub user_agent: String,

    /// Contact address appended to the User-Agent, as BLS asks of bulk
    /// clients (ADM-0022): a project role address, never a person's.
    /// Empty: none.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_CONTACT",
        default_value = "data@bleh.xyz"
    )]
    pub contact: String,

    /// Minimum milliseconds between request starts.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_MIN_REQUEST_INTERVAL_MS",
        default_value_t = 500
    )]
    pub min_request_interval_ms: u64,

    /// Retries for transient HTTP failures.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_MAX_RETRIES",
        default_value_t = 5
    )]
    pub max_retries: u32,

    /// Per-request timeout, seconds.
    #[arg(
        long,
        global = true,
        env = "REFERENCE_POLLERS_REQUEST_TIMEOUT",
        default_value_t = 300
    )]
    pub request_timeout: u64,

    /// Base URL overrides, `name=url` (names: cftc, treasury, fiscaldata,
    /// nyfed, frb, datamart, ams, ercot, eia, bls), for mirrors and tests.
    #[arg(
        long = "base-url",
        global = true,
        env = "REFERENCE_POLLERS_BASE_URLS",
        value_delimiter = ' '
    )]
    pub base_urls: Vec<String>,

    /// ERCOT report type IDs polled live, comma separated.
    #[arg(long, global = true, env = "REFERENCE_POLLERS_ERCOT_PRODUCTS", default_value = crate::sources::ercot::DEFAULT_PRODUCTS)]
    pub ercot_products: String,

    /// BLS area codes emitted, comma separated, or `all` (every area is
    /// admitted; the default keeps the U.S. city average).
    #[arg(long, global = true, env = "REFERENCE_POLLERS_BLS_AREAS", default_value = crate::sources::bls::DEFAULT_AREAS)]
    pub bls_areas: String,

    /// Log as JSON.
    #[arg(long, global = true, env = "REFERENCE_POLLERS_LOG_JSON")]
    pub log_json: bool,

    /// What to do (default: run).
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Subcommands.
#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Poll live until stopped.
    Run,
    /// One cycle of every source, then exit (non-zero if a target failed).
    Once,
    /// Import history into an archive root, one year per unit.
    Backfill {
        /// Archive root the partitions are written under.
        #[arg(
            long,
            env = "REFERENCE_POLLERS_ARCHIVE_ROOT",
            default_value = "data/reference/archive/reference-backfill"
        )]
        archive_root: PathBuf,
        /// First year (default: each source's earliest).
        #[arg(long)]
        from_year: Option<i32>,
        /// Last year (default: the current year).
        #[arg(long)]
        to_year: Option<i32>,
    },
    /// Parse a raw file offline and print records as JSON lines (uses the
    /// sidecar next to it); no network, no state.
    Parse {
        /// The raw file.
        file: PathBuf,
        /// Mark records as backfill instead of live.
        #[arg(long)]
        backfill: bool,
    },
    /// Print the series catalogue (JSON).
    Catalog,
    /// Print the source notices (JSON) for an archive's `_notices.json`.
    Notices,
}

impl Args {
    /// The selected sources (empty entries ignored; none means all).
    ///
    /// # Errors
    ///
    /// Fails on an unknown source name.
    pub fn selected(&self) -> anyhow::Result<Vec<SourceId>> {
        let mut v = self
            .sources
            .iter()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.parse::<SourceId>().map_err(anyhow::Error::msg))
            .collect::<anyhow::Result<Vec<_>>>()?;
        if v.is_empty() {
            return Ok(SourceId::ALL.to_vec());
        }
        v.sort();
        v.dedup();
        Ok(v)
    }

    /// HTTP settings.
    #[must_use]
    pub fn http_config(&self) -> HttpConfig {
        HttpConfig {
            user_agent: self.full_user_agent(),
            min_interval: Duration::from_millis(self.min_request_interval_ms),
            timeout: Duration::from_secs(self.request_timeout.max(1)),
            max_retries: self.max_retries,
            ..HttpConfig::default()
        }
    }

    /// The User-Agent with the contact address, unless it already names
    /// it or no contact is set.
    #[must_use]
    pub fn full_user_agent(&self) -> String {
        let contact = self.contact.trim();
        if contact.is_empty() || self.user_agent.contains(contact) {
            self.user_agent.clone()
        } else {
            format!("{} ({contact})", self.user_agent.trim())
        }
    }

    /// Base URLs with the ERCOT product list and the BLS areas.
    ///
    /// # Errors
    ///
    /// Fails on a malformed override.
    pub fn bases(&self) -> anyhow::Result<Bases> {
        let mut o = self.base_urls.clone();
        o.push(format!("ercot-products={}", self.ercot_products));
        o.push(format!("bls-areas={}", self.bls_areas));
        Bases::parse(&o).map_err(anyhow::Error::msg)
    }
}

/// Runs the live pollers and the metrics server until `shutdown` turns
/// true (or one cycle with `once`). Records go to the daily JSON lines
/// and, when given, to `sink`.
///
/// # Errors
///
/// Fails on configuration, state or bind errors, or (with `once`) a
/// failed source.
pub async fn run(
    args: &Args,
    once: bool,
    sink: Option<mpsc::Sender<Record>>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let metrics = Arc::new(Metrics::new());
    let health = Arc::new(Health::new());
    let fetcher = Fetcher::new(args.http_config(), metrics.clone())?;
    let raw = RawStore::new(&args.raw_dir);
    let bases = args.bases()?;
    let sources = args.selected()?;
    let writer = (!args.no_jsonl).then(|| JsonlWriter::new(&args.out_dir));
    if !args.no_jsonl {
        std::fs::create_dir_all(&args.out_dir)?;
        write_notices(&args.out_dir, &sources)?;
    }
    let emitter = Arc::new(Mutex::new(Emitter::new(writer, sink, metrics.clone())));
    let today = Date::of_ms(UnixMillis::now().0);
    let mut tasks = Vec::new();
    for id in &sources {
        let r = reader(*id, &bases);
        health.register(id.name(), r.schedule().max_age());
        let poller = SourcePoller::open(
            r,
            fetcher.clone(),
            raw.clone(),
            &args.state_dir,
            emitter.clone(),
            metrics.clone(),
            today,
        )?;
        tasks.push(tokio::spawn(poller.run(
            once,
            health.clone(),
            shutdown.clone(),
        )));
    }
    let server = if once {
        None
    } else {
        let listener = tokio::net::TcpListener::bind(args.listen).await?;
        let mut s = shutdown.clone();
        Some(tokio::spawn(crate::metrics::serve(
            listener,
            metrics.clone(),
            health.clone(),
            async move {
                while !*s.borrow() {
                    if s.changed().await.is_err() {
                        break;
                    }
                }
            },
        )))
    };
    let mut first_err: Option<anyhow::Error> = None;
    for t in tasks {
        match t.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!(error = %format!("{e:#}"), "source failed");
                first_err.get_or_insert(e);
            }
            Err(e) => {
                first_err.get_or_insert(anyhow::anyhow!("source task panicked: {e}"));
            }
        }
    }
    if let Some(s) = server {
        match s.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!(error = %e, "metrics server failed"),
            Err(e) => tracing::error!(error = %e, "metrics server task failed"),
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Runs the backfill for every selected source and prints one JSON report
/// line per unit.
///
/// # Errors
///
/// Fails on the first source that fails.
pub async fn run_backfill(
    args: &Args,
    archive_root: PathBuf,
    from_year: Option<i32>,
    to_year: Option<i32>,
) -> anyhow::Result<Vec<backfill::UnitReport>> {
    let metrics = Arc::new(Metrics::new());
    let fetcher = Fetcher::new(args.http_config(), metrics.clone())?;
    let bases = args.bases()?;
    let current = Date::of_ms(UnixMillis::now().0).y;
    let opts = backfill::Options {
        archive_root,
        state_dir: args.state_dir.clone(),
        from_year,
        to_year: to_year.unwrap_or(current).min(current),
        current_year: current,
    };
    tracing::info!(archive = BACKFILL_ARCHIVE, root = %opts.archive_root.display(), "backfill");
    let mut all = Vec::new();
    for id in args.selected()? {
        let r = reader(id, &bases);
        let reports =
            backfill::run_source(r.as_ref(), &Fetch::Http(fetcher.clone()), &opts, &metrics)
                .await?;
        all.extend(reports);
    }
    Ok(all)
}
