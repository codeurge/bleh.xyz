//! Writing records to partitions.
//!
//! [`PartitionWriter`] is the seam. The pipeline calls
//! [`PartitionWriter::write_partition`] once per partition and hour, with
//! the records already in canonical order, then
//! [`PartitionWriter::finish_hour`] after each hour and
//! [`PartitionWriter::flush`] at the end of the run.
//!
//! Implementations:
//!
//! - [`ParquetWriter`] (the binary's default): the archive crate's
//!   deterministic Parquet partition writer
//!   ([`archive::partition::write_partitions`], archive-layout-v0 § 3),
//!   under the backfill archive's own root ([`BACKFILL_ARCHIVE`] by
//!   default; archive-layout-v0 § 1). Parquet files cannot be appended to
//!   and partitions are daily, so it buffers each partition until its day
//!   is complete (one hour after the day ends, for records whose source
//!   time falls just before the hourly file they arrive in) and writes it
//!   once.
//! - [`JsonlWriter`]: canonical JSON lines under the same Hive paths, a
//!   debugging and comparison format (`process --format jsonl`), not the
//!   published one.
//! - [`MemoryWriter`], for tests.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;

use archive::partition::{PartitionOptions, WrittenFile, write_partitions};
use mesh_types::{InstrumentId, Record, RecordKind, UnixMillis, VenueId};
use serde::Serialize;

use crate::buckets::Day;

/// Name of the backfill archive (archive-layout-v0 § 1: a backfill is its
/// own archive, with its own manifest chain and admission ID).
pub const BACKFILL_ARCHIVE: &str = "hyperliquid-backfill";

/// The partition a record belongs to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionKey {
    /// Record kind.
    pub kind: RecordKind,
    /// Venue.
    pub venue: VenueId,
    /// Instrument.
    pub instrument: InstrumentId,
    /// UTC day of `ts_source_ms`.
    pub date: Day,
}

impl PartitionKey {
    /// The partition of a record.
    #[must_use]
    pub fn of(record: &Record) -> Self {
        let p = record.prov();
        Self {
            kind: record.kind(),
            venue: p.venue.clone(),
            instrument: p.instrument.clone(),
            date: Day::of(p.ts_source_ms),
        }
    }

    /// The Hive directory under an archive root:
    /// `<root>/kind=<kind>/venue=<venue>/instrument=<id with ~>/date=<YYYY-MM-DD>`.
    #[must_use]
    pub fn hive_dir(&self, root: &str) -> String {
        format!(
            "{root}/kind={}/venue={}/instrument={}/date={}",
            self.kind.as_str(),
            self.venue,
            self.instrument.path_segment(),
            self.date.iso()
        )
    }
}

/// Errors from writing partitions.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// I/O.
    #[error("writing partition {path}: {source}")]
    Io {
        /// Path being written.
        path: String,
        /// The error.
        source: std::io::Error,
    },
    /// Encoding.
    #[error("encoding a record: {0}")]
    Encode(String),
    /// The archive crate's writer failed (invalid or duplicate records,
    /// a write-once conflict, a failed read-back check, I/O).
    #[error("archive partition writer: {0}")]
    Archive(String),
    /// Records arrived for a partition that was already written.
    #[error("partition {0} was already written; process whole days in order")]
    Closed(String),
}

/// Writes records to partitions.
pub trait PartitionWriter: Send {
    /// Appends `records` (all in `key`, in canonical order) to the
    /// partition.
    ///
    /// # Errors
    ///
    /// Fails if the records cannot be written.
    fn write_partition(&mut self, key: &PartitionKey, records: &[Record])
    -> Result<(), WriteError>;

    /// Called after the hour starting at `hour_start` has been handed over
    /// in full. Hours arrive in order.
    ///
    /// # Errors
    ///
    /// Fails if buffered partitions cannot be written.
    fn finish_hour(&mut self, hour_start: UnixMillis) -> Result<(), WriteError> {
        let _ = hour_start;
        Ok(())
    }

    /// Called once at the end of a run: writes everything still buffered.
    ///
    /// # Errors
    ///
    /// Fails if buffered partitions cannot be written.
    fn flush(&mut self) -> Result<(), WriteError> {
        Ok(())
    }
}

/// One partition file written by [`ParquetWriter`], for the run report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartitionFile {
    /// Path relative to the archive root.
    pub path: String,
    /// Records in the file.
    pub record_count: u64,
    /// Canonical content hash of the records (`bleh.content_hash`).
    pub content_hash: String,
    /// SHA-256 of the file bytes.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
    /// True if an identical file was already there (a rerun).
    pub already_present: bool,
}

impl From<&WrittenFile> for PartitionFile {
    fn from(w: &WrittenFile) -> Self {
        Self {
            path: w.path.clone(),
            record_count: w.record_count,
            content_hash: w.content_hash.to_string(),
            sha256: w.sha256.to_string(),
            size: w.size,
            already_present: w.already_present,
        }
    }
}

/// Writes daily Parquet partitions with the archive crate's writer under
/// an archive root (see the module docs).
///
/// Files are write-once: rerunning a day with identical input finds
/// byte-identical files and reports them `already_present`; different
/// content for an existing file is an error. Book snapshots that resolve
/// to the same block in two consecutive hourly files keep the later one
/// (as within one file); any other duplicate `seq` is an error.
#[derive(Debug)]
pub struct ParquetWriter {
    root: PathBuf,
    opts: PartitionOptions,
    pending: BTreeMap<PartitionKey, Vec<Record>>,
    written_keys: BTreeSet<PartitionKey>,
    /// Files written so far, in write order.
    pub files: Vec<PartitionFile>,
}

impl ParquetWriter {
    /// A writer for the archive rooted at `root` (for example
    /// `<out-dir>/hyperliquid-backfill`), with the archive crate's default
    /// options: validation, read-back check, write-once.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            opts: PartitionOptions::default(),
            pending: BTreeMap::new(),
            written_keys: BTreeSet::new(),
            files: Vec::new(),
        }
    }

    /// The archive root.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn write_keys(&mut self, keys: Vec<PartitionKey>) -> Result<(), WriteError> {
        let mut records = Vec::new();
        for key in keys {
            let Some(mut rs) = self.pending.remove(&key) else {
                continue;
            };
            if key.kind == RecordKind::BookSnapshot {
                // Stable sort keeps arrival order among equal seqs; keep
                // the last of each run.
                rs.sort_by_key(|r| r.prov().seq);
                let mut kept: Vec<Record> = Vec::with_capacity(rs.len());
                for r in rs {
                    match kept.last_mut() {
                        Some(last) if last.prov().seq == r.prov().seq => *last = r,
                        _ => kept.push(r),
                    }
                }
                rs = kept;
            }
            records.extend(rs);
            self.written_keys.insert(key);
        }
        if records.is_empty() {
            return Ok(());
        }
        let written = write_partitions(&self.root, records, &self.opts)
            .map_err(|e| WriteError::Archive(e.to_string()))?;
        self.files.extend(written.iter().map(PartitionFile::from));
        Ok(())
    }
}

impl PartitionWriter for ParquetWriter {
    fn write_partition(
        &mut self,
        key: &PartitionKey,
        records: &[Record],
    ) -> Result<(), WriteError> {
        if self.written_keys.contains(key) {
            return Err(WriteError::Closed(key.hive_dir("")));
        }
        self.pending
            .entry(key.clone())
            .or_default()
            .extend_from_slice(records);
        Ok(())
    }

    fn finish_hour(&mut self, hour_start: UnixMillis) -> Result<(), WriteError> {
        // A day is complete once the first hour after it has been handed
        // over: write partitions of days that ended at or before the start
        // of the hour just finished.
        let done: Vec<PartitionKey> = self
            .pending
            .keys()
            .filter(|k| k.date.succ().start_ms() <= hour_start)
            .cloned()
            .collect();
        self.write_keys(done)
    }

    fn flush(&mut self) -> Result<(), WriteError> {
        let all: Vec<PartitionKey> = self.pending.keys().cloned().collect();
        self.write_keys(all)
    }
}

/// Keeps partitions in memory.
#[derive(Debug, Default)]
pub struct MemoryWriter {
    /// Records per partition.
    pub parts: BTreeMap<PartitionKey, Vec<Record>>,
}

impl PartitionWriter for MemoryWriter {
    fn write_partition(
        &mut self,
        key: &PartitionKey,
        records: &[Record],
    ) -> Result<(), WriteError> {
        self.parts
            .entry(key.clone())
            .or_default()
            .extend_from_slice(records);
        Ok(())
    }
}

/// Writes `<root>/<hive dir>/records.jsonl`, one canonical record per
/// line, appending.
#[derive(Debug, Clone)]
pub struct JsonlWriter {
    root: PathBuf,
}

impl JsonlWriter {
    /// A writer under `root` (created as needed).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The file a partition is written to.
    #[must_use]
    pub fn file(&self, key: &PartitionKey) -> PathBuf {
        let dir = key.hive_dir("");
        self.root
            .join(dir.trim_start_matches('/'))
            .join("records.jsonl")
    }
}

impl PartitionWriter for JsonlWriter {
    fn write_partition(
        &mut self,
        key: &PartitionKey,
        records: &[Record],
    ) -> Result<(), WriteError> {
        let path = self.file(key);
        let io = |source| WriteError::Io {
            path: path.display().to_string(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut buf = Vec::new();
        for r in records {
            let line = mesh_types::hash::canonical_record(r)
                .map_err(|e| WriteError::Encode(e.to_string()))?;
            buf.extend_from_slice(&line);
            buf.push(b'\n');
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io)?;
        f.write_all(&buf).map_err(io)
    }
}

/// UTC hour start of a timestamp.
#[must_use]
pub fn hour_of(ts: UnixMillis) -> UnixMillis {
    UnixMillis(ts.0.div_euclid(3_600_000) * 3_600_000)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mesh_types::{
        AdmissionId, AggressorSide, Price, Provenance, Seq, Size, Trade, TradeId, UnixNanos,
    };

    pub(crate) fn trade(instrument: &str, block: u64, idx: u32, ts: i64) -> Record {
        Record::from(Trade {
            prov: Provenance::new(
                InstrumentId::new(instrument).unwrap(),
                VenueId::new("hyperliquid").unwrap(),
                Seq::new(block, idx),
                UnixMillis(ts),
                UnixNanos(ts * 1_000_000),
                AdmissionId::new("ADM-0003").unwrap(),
            ),
            price: Price::new(100, 1).unwrap(),
            size: Size::new(1, 0).unwrap(),
            aggressor: AggressorSide::Buy,
            trade_id: TradeId(1),
        })
    }

    #[test]
    fn hive_paths_follow_the_archive_layout() {
        let r = trade("hyperliquid:perp:xyz:CL", 1, 0, 1_753_574_401_000);
        let k = PartitionKey::of(&r);
        assert_eq!(
            k.hive_dir("hyperliquid-backfill"),
            "hyperliquid-backfill/kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~xyz~CL/date=2025-07-27"
        );
    }

    #[test]
    fn jsonl_writer_appends_canonical_lines() {
        let dir = crate::testutil::tempdir();
        let mut w = JsonlWriter::new(&dir);
        let a = trade("hyperliquid:perp:BTC", 1, 0, 1_753_574_401_000);
        let b = trade("hyperliquid:perp:BTC", 2, 0, 1_753_574_402_000);
        let k = PartitionKey::of(&a);
        w.write_partition(&k, std::slice::from_ref(&a)).unwrap();
        w.write_partition(&k, std::slice::from_ref(&b)).unwrap();
        let text = std::fs::read_to_string(w.file(&k)).unwrap();
        let back: Vec<Record> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(back, [a, b]);
        assert_eq!(hour_of(UnixMillis(3_600_001)), UnixMillis(3_600_000));
    }

    #[test]
    fn parquet_writer_writes_each_day_once_it_is_complete() {
        // 2025-07-27T00:00:01Z and the next day's first hour.
        let (t, day_end) = (1_753_574_401_000, 1_753_660_800_000);
        let a = trade("hyperliquid:perp:BTC", 2, 0, t + 1000);
        let b = trade("hyperliquid:perp:BTC", 1, 0, t);
        let k = PartitionKey::of(&a);
        let mut runs = Vec::new();
        for _ in 0..2 {
            let dir = crate::testutil::tempdir().join(BACKFILL_ARCHIVE);
            let mut w = ParquetWriter::new(&dir);
            w.write_partition(&k, std::slice::from_ref(&a)).unwrap();
            w.finish_hour(UnixMillis(day_end - 3_600_000)).unwrap();
            assert!(w.files.is_empty(), "the slack hour has not passed");
            w.write_partition(&k, std::slice::from_ref(&b)).unwrap();
            w.finish_hour(UnixMillis(day_end)).unwrap();
            assert_eq!(w.files.len(), 1);
            assert!(matches!(
                w.write_partition(&k, std::slice::from_ref(&a)),
                Err(WriteError::Closed(_))
            ));
            w.flush().unwrap();
            let f = &w.files[0];
            assert_eq!(
                f.path,
                "kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~BTC/date=2025-07-27/part-00000.parquet"
            );
            let bytes = std::fs::read(dir.join(&f.path)).unwrap();
            let back = archive::partition::read_parquet(bytes.into()).unwrap();
            assert!(back.consistent());
            assert_eq!(back.records, [b.clone(), a.clone()], "sorted by seq");
            assert_eq!(back.content_hash.to_string(), f.content_hash);

            // A rerun with the same records finds the identical file.
            let mut again = ParquetWriter::new(&dir);
            again.write_partition(&k, &[b.clone(), a.clone()]).unwrap();
            again.flush().unwrap();
            assert!(again.files[0].already_present);
            runs.push(f.clone());
        }
        assert_eq!(runs[0], runs[1], "byte-identical across runs");
    }
}
