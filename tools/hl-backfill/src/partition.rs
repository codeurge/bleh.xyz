//! Writing records to partitions.
//!
//! [`PartitionWriter`] is the seam for the archive crate's Parquet writer
//! (`docs/specs/archive-layout-v0.md` § 3). The pipeline calls it once per
//! partition and hour, with the records already in canonical order, so a
//! writer that appends keeps each partition in canonical order as long as
//! hours are processed in order.
//!
//! Local implementations: [`MemoryWriter`] (tests) and [`JsonlWriter`],
//! which writes canonical JSON lines under the same Hive paths. JSON lines
//! are a development and comparison format, not the published one.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use mesh_types::{InstrumentId, Record, RecordKind, UnixMillis, VenueId};

use crate::buckets::Day;

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
}
