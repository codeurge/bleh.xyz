//! Record output, the same paths swap-pollers uses: daily JSON lines for
//! the archive lane and an optional channel for an embedding gateway.
//!
//! JSON lines are written to
//!
//! ```text
//! <out_dir>/date=<YYYY-MM-DD>/reference_value.jsonl
//! <out_dir>/date=<YYYY-MM-DD>/positioning.jsonl
//! <out_dir>/_notices.json
//! ```
//!
//! by UTC day of `prov.ts_source_ms`, one record per line in the canonical
//! encoding of observation v0 (§ 6.1). `_notices.json` lists the source
//! notices (ADR 0012) for the archive builder to put in every manifest
//! (manifest-v0 § 2.3); copy it to the root of the archive the records
//! are partitioned into.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mesh_types::{Record, SourceNotice};
use tokio::sync::mpsc;

use crate::metrics::Metrics;
use crate::source::SourceId;
use crate::time::Date;

/// Appends canonical JSON lines per UTC day and kind.
#[derive(Debug)]
pub struct JsonlWriter {
    dir: PathBuf,
    open: BTreeMap<(String, &'static str), BufWriter<File>>,
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

    /// Path of the file for `day` and `kind`.
    #[must_use]
    pub fn path_for(&self, day: &str, kind: &str) -> PathBuf {
        self.dir
            .join(format!("date={day}"))
            .join(format!("{kind}.jsonl"))
    }

    /// Appends one record.
    ///
    /// # Errors
    ///
    /// Fails on I/O or encoding errors.
    pub fn write(&mut self, record: &Record) -> std::io::Result<()> {
        let day = Date::of_ms(record.prov().ts_source_ms.0).to_string();
        let kind = record.kind().as_str();
        let key = (day.clone(), kind);
        if !self.open.contains_key(&key) {
            let path = self.path_for(&day, kind);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let f = OpenOptions::new().create(true).append(true).open(&path)?;
            while self.open.len() >= 8 {
                if let Some((_, mut w)) = self.open.pop_first() {
                    w.flush()?;
                }
            }
            self.open.insert(key.clone(), BufWriter::new(f));
        }
        let line = mesh_types::hash::canonical_record(record).map_err(std::io::Error::other)?;
        let w = self
            .open
            .get_mut(&key)
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

/// Writes `_notices.json` for `sources` under `dir`.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn write_notices(dir: &Path, sources: &[SourceId]) -> std::io::Result<Vec<SourceNotice>> {
    let notices = crate::source::all_notices(sources);
    let mut body = serde_json::to_vec_pretty(&notices).map_err(std::io::Error::other)?;
    body.push(b'\n');
    crate::raw::write_atomic_sync(&dir.join("_notices.json"), &body)?;
    Ok(notices)
}

/// Writes records and publishes them, counting.
#[derive(Debug)]
pub struct Emitter {
    writer: Option<JsonlWriter>,
    sink: Option<mpsc::Sender<Record>>,
    metrics: Arc<Metrics>,
}

impl Emitter {
    /// An emitter; either output may be absent.
    #[must_use]
    pub fn new(
        writer: Option<JsonlWriter>,
        sink: Option<mpsc::Sender<Record>>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            writer,
            sink,
            metrics,
        }
    }

    /// Writes and publishes `records` from `source`.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors. A closed gateway channel is logged, not fatal:
    /// the JSON lines remain the record.
    pub async fn emit(&mut self, source: SourceId, records: Vec<Record>) -> std::io::Result<()> {
        for r in records {
            if let Some(w) = &mut self.writer {
                w.write(&r)?;
            }
            let revision = match &r {
                Record::ReferenceValue(v) => v.revision,
                Record::Positioning(p) => p.revision,
                _ => 0,
            };
            if revision > 0 {
                self.metrics
                    .inc("ref_revisions_total", &[("source", source.name())]);
            }
            self.metrics.inc(
                "ref_records_total",
                &[("source", source.name()), ("kind", r.kind().as_str())],
            );
            let as_of = match &r {
                Record::ReferenceValue(v) => Date::parse_iso(&v.as_of_date),
                Record::Positioning(p) => Date::parse_iso(&p.as_of_date),
                _ => None,
            };
            if let Some(d) = as_of {
                self.metrics.set_max(
                    "ref_latest_value_date_seconds",
                    &[("source", source.name())],
                    (d.start_ms() / 1000) as f64,
                );
            }
            if let Some(tx) = &self.sink
                && tx.send(r).await.is_err()
            {
                tracing::warn!("gateway channel closed; continuing with JSON lines only");
                self.sink = None;
            }
        }
        Ok(())
    }

    /// Flushes the JSON lines.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self) -> std::io::Result<()> {
        if let Some(w) = &mut self.writer {
            w.flush()?;
        }
        Ok(())
    }
}
