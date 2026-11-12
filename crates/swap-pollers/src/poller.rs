//! DTCC Data Repository (U.S.) poller: CFTC Part 43 and SEC SBSR public
//! price dissemination.
//!
//! Each cycle:
//!
//! 1. Resolve the public data bucket (`GET <api>/general/bucketname`)
//!    unless a data base URL is configured, and again after a `403`/`404`.
//! 2. Fetch `dashboard/Slice.json` conditionally (`If-None-Match`,
//!    `If-Modified-Since`). On `304` nothing changed.
//! 3. For every admitted, configured bucket, fetch each listed file not yet
//!    in the checkpoint, oldest slice ID first: stream it verbatim to the raw
//!    directory (and mirror), then parse, dedupe and emit, then checkpoint.
//! 4. Less often, do the same for `dashboard/Cumulative.json` for the last
//!    `cumulative_days` days. End-of-day files are always archived; their
//!    records are emitted (flagged `backfill`) when the day is inside the
//!    dedupe window or no live slice covered it.
//!
//! The listing validators are stored only after every new file in it was
//! captured, so a failed file is retried on the next cycle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mesh_types::{SwapPrint, UnixMillis, UnixNanos};
use tokio::sync::{mpsc, watch};

use crate::checkpoint::{CapturedFile, CheckpointStore};
use crate::dedupe;
use crate::http::{Conditional, FetchError, Fetcher};
use crate::listing::{FileEntry, parse_listing};
use crate::metrics::Metrics;
use crate::output::{Emitted, Emitter};
use crate::parse::{
    FileContext, ParseError, ParseStats, ReportKind, SinkClosed, parse_report_file,
};
use crate::raw::{RawMeta, RawStore};
use crate::repository::{DtccBucket, Repository};
use crate::server::Health;

/// DTCC poller settings.
#[derive(Debug, Clone)]
pub struct DtccConfig {
    /// PPD API base, such as `https://pddata.dtcc.com/ppd/api`.
    pub api_base: String,
    /// Public data base URL. When unset it is resolved from the API.
    pub data_base: Option<String>,
    /// Buckets to capture.
    pub buckets: Vec<DtccBucket>,
    /// Slice listing poll interval.
    pub slice_interval: Duration,
    /// Cumulative listing poll interval.
    pub cumulative_interval: Duration,
    /// Days of end-of-day files to capture (0 disables).
    pub cumulative_days: u32,
    /// Days of slice files to remember in the checkpoint.
    pub slice_retention_days: u32,
}

impl Default for DtccConfig {
    fn default() -> Self {
        Self {
            api_base: "https://pddata.dtcc.com/ppd/api".into(),
            data_base: None,
            buckets: DtccBucket::ADMITTED.to_vec(),
            slice_interval: Duration::from_secs(60),
            cumulative_interval: Duration::from_secs(3600),
            cumulative_days: 7,
            slice_retention_days: 3,
        }
    }
}

const SLICE_LISTING: &str = "dashboard/Slice.json";
const CUMULATIVE_LISTING: &str = "dashboard/Cumulative.json";

/// The DTCC poller.
#[derive(Debug)]
pub struct DtccPoller {
    cfg: DtccConfig,
    fetcher: Fetcher,
    raw: RawStore,
    checkpoint: CheckpointStore,
    emitter: Emitter,
    metrics: Arc<Metrics>,
    health: Arc<Health>,
    data_base: Option<String>,
}

/// Summary of one listing poll.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PollSummary {
    /// The listing was unchanged (`304`).
    pub not_modified: bool,
    /// Files captured.
    pub files: u64,
    /// Files that failed (retried next cycle).
    pub failed: u64,
    /// Records emitted.
    pub records: u64,
}

fn today() -> String {
    dedupe::day_of(UnixMillis::now().0)
}

fn days_ago(n: u32) -> String {
    dedupe::day_of(UnixMillis::now().0 - i64::from(n) * 86_400_000)
}

impl DtccPoller {
    /// Builds a poller.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: DtccConfig,
        fetcher: Fetcher,
        raw: RawStore,
        checkpoint: CheckpointStore,
        emitter: Emitter,
        metrics: Arc<Metrics>,
        health: Arc<Health>,
    ) -> Self {
        let data_base = cfg
            .data_base
            .clone()
            .map(|b| b.trim_end_matches('/').to_string());
        Self {
            cfg,
            fetcher,
            raw,
            checkpoint,
            emitter,
            metrics,
            health,
            data_base,
        }
    }

    async fn resolve_data_base(&mut self, force: bool) -> anyhow::Result<String> {
        if let (Some(b), false) = (&self.data_base, force) {
            return Ok(b.clone());
        }
        if let (Some(b), true) = (&self.cfg.data_base, force) {
            return Ok(b.trim_end_matches('/').to_string());
        }
        let url = format!(
            "{}/general/bucketname",
            self.cfg.api_base.trim_end_matches('/')
        );
        let body = self.fetcher.get(&url, "bucketname").await?;
        let name = String::from_utf8_lossy(&body).trim().to_string();
        let valid = !name.is_empty()
            && name.len() <= 63
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
        if !valid {
            anyhow::bail!("bucket name endpoint returned an unexpected value");
        }
        let base = format!("https://{name}.s3.amazonaws.com");
        tracing::info!(%base, "resolved DTCC public data bucket");
        self.data_base = Some(base.clone());
        Ok(base)
    }

    /// URL for a listed file: the listed URL when it is under the data
    /// base, else built from the known layout.
    fn file_url(base: &str, bucket: DtccBucket, kind: ReportKind, entry: &FileEntry) -> String {
        if let Some(full) = &entry.full_file_path
            && full.starts_with(&format!("{base}/"))
            && full.ends_with(&format!("/{}", entry.file_name))
        {
            return full.clone();
        }
        let dir = match kind {
            ReportKind::Slice => "slices",
            ReportKind::Cumulative => "eod",
        };
        format!(
            "{base}/{}/{dir}/{}",
            bucket.regime.dtcc_path(),
            entry.file_name
        )
    }

    /// Polls the slice listing once.
    ///
    /// # Errors
    ///
    /// Fails if the listing cannot be fetched or parsed. Individual file
    /// failures are counted in the summary.
    pub async fn poll_slices(&mut self) -> anyhow::Result<PollSummary> {
        self.poll_listing(SLICE_LISTING, ReportKind::Slice).await
    }

    /// Polls the cumulative listing once.
    ///
    /// # Errors
    ///
    /// As [`DtccPoller::poll_slices`].
    pub async fn poll_cumulative(&mut self) -> anyhow::Result<PollSummary> {
        self.poll_listing(CUMULATIVE_LISTING, ReportKind::Cumulative)
            .await
    }

    async fn poll_listing(
        &mut self,
        listing: &str,
        kind: ReportKind,
    ) -> anyhow::Result<PollSummary> {
        let mut base = self.resolve_data_base(false).await?;
        let prev = self
            .checkpoint
            .state
            .listings
            .get(listing)
            .cloned()
            .unwrap_or_default();
        let target = match kind {
            ReportKind::Slice => "slice_listing",
            ReportKind::Cumulative => "cumulative_listing",
        };
        let mut result = self
            .fetcher
            .get_conditional(&format!("{base}/{listing}"), target, &prev)
            .await;
        if let Err(e) = &result
            && matches!(e.status().map(|s| s.as_u16()), Some(403 | 404))
            && self.cfg.data_base.is_none()
        {
            tracing::warn!(error = %e, "listing not found; re-resolving the data bucket");
            base = self.resolve_data_base(true).await?;
            result = self
                .fetcher
                .get_conditional(&format!("{base}/{listing}"), target, &prev)
                .await;
        }
        let mut summary = PollSummary::default();
        let (body, validators) = match result? {
            Conditional::NotModified => {
                summary.not_modified = true;
                self.mark_poll_ok(listing);
                return Ok(summary);
            }
            Conditional::Body { body, validators } => (body, validators),
        };
        let parsed = parse_listing(&body)?;
        if parsed.bad_entries > 0 {
            tracing::warn!(
                listing,
                bad = parsed.bad_entries,
                "listing entries could not be read"
            );
        }
        let oldest = days_ago(self.cfg.cumulative_days.saturating_sub(1));
        for bucket in self.cfg.buckets.clone() {
            let Some(entries) = parsed.buckets.get(&bucket) else {
                continue;
            };
            let mut todo: Vec<FileEntry> = entries
                .iter()
                .filter(|e| !self.checkpoint.has(&bucket.key(), &e.file_name))
                .filter(|e| {
                    kind == ReportKind::Slice
                        || e.name_date().is_some_and(|d| d.as_str() >= oldest.as_str())
                })
                .cloned()
                .collect();
            todo.sort_by_key(FileEntry::batch);
            for entry in todo {
                match self.capture(bucket, kind, &entry, &base).await {
                    Ok(records) => {
                        summary.files += 1;
                        summary.records += records;
                    }
                    Err(e) => {
                        summary.failed += 1;
                        self.metrics.inc(
                            "swap_files_total",
                            &[
                                ("bucket", &bucket.key()),
                                ("kind", kind_label(kind)),
                                ("outcome", "error"),
                            ],
                        );
                        tracing::error!(file = %entry.file_name, error = %e, "capture failed; will retry");
                    }
                }
            }
        }
        if summary.failed == 0 {
            self.checkpoint
                .state
                .listings
                .insert(listing.to_string(), validators);
        }
        if kind == ReportKind::Slice {
            self.checkpoint
                .prune_slices(&days_ago(self.cfg.slice_retention_days.saturating_sub(1)));
        }
        self.checkpoint.save().await?;
        self.mark_poll_ok(listing);
        Ok(summary)
    }

    fn mark_poll_ok(&self, listing: &str) {
        let now = UnixMillis::now().0 as f64 / 1000.0;
        self.metrics.set(
            "swap_last_successful_poll_timestamp_seconds",
            &[("listing", listing)],
            now,
        );
        self.health.record_success();
    }

    /// Captures one file: download verbatim (unless already on disk from an
    /// interrupted run), mirror, parse, emit, checkpoint. Returns the
    /// records emitted.
    async fn capture(
        &mut self,
        bucket: DtccBucket,
        kind: ReportKind,
        entry: &FileEntry,
        base: &str,
    ) -> anyhow::Result<u64> {
        let repo = Repository::DtccDdr;
        let path = self.raw.path_for(repo, bucket, kind, entry);
        let meta = match RawMeta::load_for(&path).filter(|_| path.exists()) {
            Some(meta) => meta,
            None => {
                let url = Self::file_url(base, bucket, kind, entry);
                let target = match kind {
                    ReportKind::Slice => "slice_file",
                    ReportKind::Cumulative => "cumulative_file",
                };
                let d = self
                    .fetcher
                    .download_to(&url, target, &path)
                    .await
                    .map_err(|e| {
                        if let FetchError::Io(_) = e {
                            self.metrics
                                .inc("swap_raw_persist_errors_total", &[("store", "local")]);
                        }
                        e
                    })?;
                let meta = RawMeta {
                    version: 1,
                    repository: repo.id_str().to_string(),
                    bucket: bucket.key(),
                    kind,
                    url,
                    fetched_at_ns: UnixNanos::now().0.to_string(),
                    bytes: d.bytes,
                    sha256: d.sha256,
                    etag: d.validators.etag,
                    last_modified: d.validators.last_modified,
                    entry: entry.clone(),
                };
                self.raw.write_meta(&path, &meta).await.inspect_err(|_| {
                    self.metrics
                        .inc("swap_raw_persist_errors_total", &[("store", "local")]);
                })?;
                meta
            }
        };
        self.raw.mirror(&path).await;

        let day = entry.name_date();
        let key = bucket.key();
        let had_slices = day
            .as_deref()
            .is_some_and(|d| self.checkpoint.had_slices(&key, d));
        let emit = match kind {
            ReportKind::Slice => true,
            ReportKind::Cumulative => {
                !had_slices
                    || day
                        .as_deref()
                        .is_some_and(|d| self.emitter.seen().covers(d, &today()))
            }
        };
        let ctx = FileContext {
            bucket,
            repository: repo,
            kind,
            batch: entry.batch(),
            fetched_at: meta.fetched_at_ns.parse().ok().map(UnixNanos),
            listing_dissemination: entry.dissemination(),
            backfill: kind == ReportKind::Cumulative,
        };
        let mut outcome = if emit { "captured" } else { "archived_only" };
        let (stats, emitted) = if emit {
            match self.parse_and_emit(&path, &ctx).await {
                Ok(r) => r,
                Err(e) if is_format_error(&e) => {
                    // The raw file is kept; a format change will not fix
                    // itself on retry. Count it and move on.
                    self.metrics.inc(
                        "swap_parse_errors_total",
                        &[("bucket", &bucket.key()), ("scope", "file")],
                    );
                    tracing::error!(file = %entry.file_name, error = %e, "report file unreadable; raw kept");
                    outcome = "unparseable";
                    (ParseStats::default(), 0)
                }
                Err(e) => return Err(e),
            }
        } else {
            (ParseStats::default(), 0)
        };
        self.metrics.inc(
            "swap_files_total",
            &[
                ("bucket", &key),
                ("kind", kind_label(kind)),
                ("outcome", outcome),
            ],
        );
        self.checkpoint.mark(
            &key,
            &entry.file_name,
            CapturedFile {
                kind,
                batch: ctx.batch,
                sha256: meta.sha256.clone(),
                rows: stats.rows,
                records: emitted,
                emitted: emit,
            },
            if kind == ReportKind::Slice { day } else { None },
        );
        self.checkpoint.save().await?;
        Ok(emitted)
    }

    async fn parse_and_emit(
        &mut self,
        path: &Path,
        ctx: &FileContext,
    ) -> anyhow::Result<(ParseStats, u64)> {
        let (stats, emitted) = parse_and_emit(path, ctx, &mut self.emitter).await?;
        record_stats(&self.metrics, &ctx.bucket.key(), &stats);
        Ok((stats, emitted))
    }

    /// Runs until `shutdown` turns true. With `once`, runs one slice cycle
    /// (and one cumulative cycle if enabled) and returns.
    ///
    /// # Errors
    ///
    /// In `once` mode, fails if a cycle fails. Otherwise errors are logged
    /// and the next cycle retries.
    pub async fn run(
        &mut self,
        once: bool,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        if once {
            let s = self.poll_slices().await?;
            tracing::info!(?s, "slice cycle done");
            if self.cfg.cumulative_days > 0 {
                let c = self.poll_cumulative().await?;
                tracing::info!(?c, "cumulative cycle done");
            }
            if s.failed > 0 {
                anyhow::bail!("{} slice files failed", s.failed);
            }
            return Ok(());
        }
        let mut slice_tick = tokio::time::interval(self.cfg.slice_interval);
        slice_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut cum_tick = tokio::time::interval(self.cfg.cumulative_interval);
        cum_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        tracing::info!("shutting down DTCC poller");
                        return Ok(());
                    }
                }
                _ = slice_tick.tick() => {
                    // Small jitter so many pollers do not align.
                    tokio::time::sleep(Duration::from_millis(fastrand::u64(0..=2_000))).await;
                    match self.poll_slices().await {
                        Ok(s) => tracing::debug!(?s, "slice cycle done"),
                        Err(e) => tracing::error!(error = %e, "slice cycle failed"),
                    }
                }
                _ = cum_tick.tick(), if self.cfg.cumulative_days > 0 => {
                    match self.poll_cumulative().await {
                        Ok(s) => tracing::debug!(?s, "cumulative cycle done"),
                        Err(e) => tracing::error!(error = %e, "cumulative cycle failed"),
                    }
                }
            }
        }
    }
}

fn is_format_error(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<ParseError>(),
        Some(ParseError::Csv(_) | ParseError::Zip(_) | ParseError::MissingColumn(_))
    )
}

fn kind_label(kind: ReportKind) -> &'static str {
    match kind {
        ReportKind::Slice => "slice",
        ReportKind::Cumulative => "cumulative",
    }
}

/// Parses `path` on a blocking thread and feeds each record through the
/// emitter, then flushes it. Returns the parse counters and the number of
/// records emitted (parsed minus duplicates).
///
/// # Errors
///
/// Fails on file-level parse errors or output I/O errors.
pub async fn parse_and_emit(
    path: &Path,
    ctx: &FileContext,
    emitter: &mut Emitter,
) -> anyhow::Result<(ParseStats, u64)> {
    let (tx, mut rx) = mpsc::channel::<SwapPrint>(4096);
    let path_buf: PathBuf = path.to_path_buf();
    let ctx2 = ctx.clone();
    let handle = tokio::task::spawn_blocking(move || {
        let mut sink = |p: SwapPrint| tx.blocking_send(p).map_err(|_| SinkClosed);
        parse_report_file(&path_buf, &ctx2, &mut sink)
    });
    let key = ctx.bucket.key();
    let mut emitted = 0u64;
    while let Some(p) = rx.recv().await {
        if emitter.emit(&key, p).await? == Emitted::Emitted {
            emitted += 1;
        }
    }
    let stats = handle.await??;
    emitter.flush()?;
    Ok((stats, emitted))
}

/// Adds a file's parse counters to the metrics.
pub fn record_stats(metrics: &Metrics, bucket: &str, stats: &ParseStats) {
    metrics.inc_by("swap_rows_total", &[("bucket", bucket)], stats.rows);
    for (reason, n) in &stats.skipped {
        metrics.inc_by(
            "swap_records_skipped_total",
            &[("bucket", bucket), ("reason", reason)],
            *n,
        );
    }
    if stats.row_errors > 0 {
        metrics.inc_by(
            "swap_parse_errors_total",
            &[("bucket", bucket), ("scope", "row")],
            stats.row_errors,
        );
    }
    for (field, n) in &stats.field_errors {
        metrics.inc_by(
            "swap_field_errors_total",
            &[("bucket", bucket), ("field", field)],
            *n,
        );
    }
    for col in &stats.unknown_columns {
        metrics.inc(
            "swap_unknown_columns_total",
            &[("bucket", bucket), ("column", col)],
        );
    }
    for (src, n) in &stats.dissemination_fallbacks {
        metrics.inc_by("swap_dissemination_fallbacks_total", &[("source", src)], *n);
    }
}
