//! Record output: daily JSON lines for the archive lane, a channel for the
//! gateway, and the dedupe and metrics step in front of both.
//!
//! JSON lines are written to
//!
//! ```text
//! <out_dir>/date=<YYYY-MM-DD>/swap_print.jsonl
//! ```
//!
//! by UTC dissemination day, one record per line in the canonical encoding
//! of observation v0 (§ 6.1: the `Record` form with `"kind":"swap_print"`,
//! sorted keys, no whitespace). The archive lane partitions from there.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;

use mesh_types::{Record, SwapAction, SwapEventType, SwapPrint, UnixMillis};
use tokio::sync::mpsc;

use crate::dedupe::{self, Seen, SeenStore};
use crate::metrics::Metrics;

/// Appends canonical JSON lines per UTC day.
#[derive(Debug)]
pub struct JsonlWriter {
    dir: PathBuf,
    open: BTreeMap<String, BufWriter<File>>,
}

impl JsonlWriter {
    /// A writer under `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            open: BTreeMap::new(),
        }
    }

    /// Path of the file for `day`.
    #[must_use]
    pub fn path_for(&self, day: &str) -> PathBuf {
        self.dir
            .join(format!("date={day}"))
            .join("swap_print.jsonl")
    }

    /// Appends one record.
    ///
    /// # Errors
    ///
    /// Fails on I/O or encoding errors.
    pub fn write(&mut self, print: &SwapPrint) -> std::io::Result<()> {
        let day = dedupe::day_of(print.ts_dissemination_ms.0);
        if !self.open.contains_key(&day) {
            let path = self.path_for(&day);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let f = OpenOptions::new().create(true).append(true).open(&path)?;
            // Keep only a few days open.
            while self.open.len() >= 3 {
                if let Some((_, mut w)) = self.open.pop_first() {
                    w.flush()?;
                }
            }
            self.open.insert(day.clone(), BufWriter::new(f));
        }
        let line = mesh_types::hash::canonical_record(&Record::SwapPrint(print.clone()))
            .map_err(std::io::Error::other)?;
        let w = self
            .open
            .get_mut(&day)
            .ok_or_else(|| std::io::Error::other("writer vanished"))?;
        w.write_all(&line)?;
        w.write_all(b"\n")
    }

    /// Flushes and syncs every open file.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self) -> std::io::Result<()> {
        for w in self.open.values_mut() {
            w.flush()?;
            w.get_ref().sync_data()?;
        }
        Ok(())
    }
}

/// Metric label for an event type (`none` when the row had none).
#[must_use]
pub const fn event_type_label(e: Option<SwapEventType>) -> &'static str {
    use SwapEventType as E;
    match e {
        None => "none",
        Some(E::Trade) => "trade",
        Some(E::Novation) => "novation",
        Some(E::Compression) => "compression",
        Some(E::EarlyTermination) => "early_termination",
        Some(E::Clearing) => "clearing",
        Some(E::Exercise) => "exercise",
        Some(E::Allocation) => "allocation",
        Some(E::ClearingAllocation) => "clearing_allocation",
        Some(E::CreditEvent) => "credit_event",
        Some(E::Porting) => "porting",
        Some(E::CorporateEvent) => "corporate_event",
        Some(E::Update) => "update",
    }
}

/// What happened to one parsed print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emitted {
    /// Written and sent.
    Emitted,
    /// Dropped as already emitted.
    Duplicate,
}

/// Dedupe, write, publish and count.
#[derive(Debug)]
pub struct Emitter {
    seen: SeenStore,
    writer: Option<JsonlWriter>,
    sink: Option<mpsc::Sender<SwapPrint>>,
    metrics: Arc<Metrics>,
}

impl Emitter {
    /// An emitter. `writer` writes JSON lines; `sink` is the gateway
    /// channel. Either may be absent.
    #[must_use]
    pub fn new(
        seen: SeenStore,
        writer: Option<JsonlWriter>,
        sink: Option<mpsc::Sender<SwapPrint>>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            seen,
            writer,
            sink,
            metrics,
        }
    }

    /// The dedupe store.
    #[must_use]
    pub fn seen(&self) -> &SeenStore {
        &self.seen
    }

    /// Handles one print for `bucket`.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors (dedupe store or JSON lines). A closed gateway
    /// channel is logged, not fatal: the JSON lines remain the record.
    pub async fn emit(&mut self, bucket: &str, print: SwapPrint) -> std::io::Result<Emitted> {
        let today = dedupe::day_of(UnixMillis::now().0);
        let day = dedupe::day_of(print.ts_dissemination_ms.0);
        let venue = print.prov.venue.as_str();
        let repo = print.repository.as_str();
        let k = dedupe::key(venue, repo, &print.dissemination_id);
        if self.seen.check_and_insert(k, &day, &today)? == Seen::Duplicate {
            self.metrics
                .inc("swap_duplicates_total", &[("bucket", bucket)]);
            return Ok(Emitted::Duplicate);
        }
        // Updates that name an earlier print: count those whose original was
        // not seen. `modify` and `terminate` may come without one.
        if print.action != SwapAction::New
            && let Some(o) = print.original_dissemination_id.as_deref()
            && !self.seen.contains_any(dedupe::key(venue, repo, o))
        {
            self.metrics
                .inc("swap_orphan_updates_total", &[("bucket", bucket)]);
        }
        if let Some(w) = &mut self.writer {
            w.write(&print)?;
        }
        let action = match print.action {
            SwapAction::New => "new",
            SwapAction::Cancel => "cancel",
            SwapAction::Correct => "correct",
            SwapAction::Modify => "modify",
            SwapAction::Terminate => "terminate",
            SwapAction::Revive => "revive",
        };
        self.metrics.inc(
            "swap_records_total",
            &[
                ("bucket", bucket),
                ("action", action),
                ("event_type", event_type_label(print.event_type)),
            ],
        );
        let dissem_secs = print.ts_dissemination_ms.0 as f64 / 1000.0;
        let prev = self
            .metrics
            .gauge(
                "swap_last_dissemination_timestamp_seconds",
                &[("bucket", bucket)],
            )
            .unwrap_or(f64::MIN);
        if dissem_secs > prev {
            self.metrics.set(
                "swap_last_dissemination_timestamp_seconds",
                &[("bucket", bucket)],
                dissem_secs,
            );
            let now = UnixMillis::now().0 as f64 / 1000.0;
            self.metrics.set(
                "swap_lag_seconds",
                &[("bucket", bucket)],
                (now - dissem_secs).max(0.0),
            );
        }
        if let Some(tx) = &self.sink
            && tx.send(print).await.is_err()
        {
            tracing::warn!("gateway channel closed; continuing with JSON lines only");
            self.sink = None;
        }
        Ok(Emitted::Emitted)
    }

    /// Flushes the dedupe store and JSON lines.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self) -> std::io::Result<()> {
        if let Some(w) = &mut self.writer {
            w.flush()?;
        }
        self.seen.flush(&dedupe::day_of(UnixMillis::now().0))
    }
}
