//! The filter and normalize pipeline: raw tier to filtered raw and
//! partitions.
//!
//! For each UTC hour, in order:
//!
//! 1. read each raw object of the hour from the raw tier (fills first,
//!    then `market_data`), decompress it (LZ4);
//! 2. filter its lines with the injected [`RawLineFilter`] and write the
//!    filtered raw file, zstd-compressed, under
//!    `<filtered_root>/stream=<dataset>/date=<YYYY-MM-DD>/<HH>.jsonl.zst`
//!    (`market_data`: `.../<HH>/<coin>.jsonl.zst`);
//! 3. parse the filtered lines with the injected [`RecordParser`]; block
//!    numbers for data without them come from a clock built from the
//!    batched fill lines seen so far (plus an optional external clock);
//! 4. stamp every record with admission `ADM-0003` and flag `backfill`,
//!    validate it and check it against the mesh field allowlist;
//! 5. group the hour's records by partition, sort them canonically and
//!    hand them to the injected [`PartitionWriter`], then tell it the hour
//!    is done (and flush it at the end of the run);
//! 6. report counts and a content hash per (kind, instrument) for the
//!    hour, which [`compare`](crate::compare) checks against live data.
//!
//! The defaults ([`Pipeline::wired`], used by the binary) are the node
//! reader's filter and parser for by-block fills objects, this crate's
//! local ones for the other bucket formats ([`DefaultFilter`],
//! [`DefaultParser`]) and the archive crate's Parquet writer
//! ([`ParquetWriter`]).
//!
//! An object that fails any step contributes nothing, and the hour is
//! reported as incomplete; the run continues with the next object.

use std::collections::BTreeMap;

use mesh_types::{AdmissionId, Flag, Record, UnixMillis};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use serde::Serialize;

use crate::buckets::{Dataset, Day};
use crate::clock::{BlockClock, ChainClock, TableClock};
use crate::codec::{DEFAULT_MAX_DECOMPRESSED, lz4_to_string, zstd_archive};
use crate::copy::dest_key;
use crate::filter::{DefaultFilter, RawLineFilter};
use crate::listing::SourceObject;
use crate::parse::{DefaultParser, RecordParser};
use crate::partition::{ParquetWriter, PartitionKey, PartitionWriter, hour_of};

/// Pipeline settings.
#[derive(Debug, Clone)]
pub struct ProcessConfig {
    /// Prefix of the raw tier copy (as given to the copier).
    pub raw_prefix: String,
    /// Root of the filtered raw output (for example
    /// `hyperliquid-backfill-raw`).
    pub filtered_root: String,
    /// Decompression limit per object.
    pub max_decompressed: u64,
}

impl Default for ProcessConfig {
    fn default() -> Self {
        Self {
            raw_prefix: "hyperliquid-archive-raw".into(),
            filtered_root: "hyperliquid-backfill-raw".into(),
            max_decompressed: DEFAULT_MAX_DECOMPRESSED,
        }
    }
}

/// Counts and hashes for one hour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HourReport {
    /// `YYYY-MM-DD/HH`.
    pub hour: String,
    /// Objects processed successfully.
    pub objects: u64,
    /// Raw lines read.
    pub lines_in: u64,
    /// Filtered lines written.
    pub lines_out: u64,
    /// Events kept by the filter.
    pub events_kept: u64,
    /// Events dropped by the filter.
    pub events_dropped: u64,
    /// Records written, by kind.
    pub records: BTreeMap<String, u64>,
    /// Events with no resolvable block.
    pub unresolved: u64,
    /// Events for coins missing from the registry.
    pub unknown_coin: u64,
    /// Invalid snapshots skipped.
    pub invalid: u64,
    /// Snapshots superseded within one block.
    pub superseded: u64,
    /// By-block lines skipped as repeated empty blocks (ADR 0024).
    pub repeated_empty_blocks: u64,
    /// L2 snapshots read.
    pub snapshots_read: u64,
    /// Median interval between consecutive snapshots in one file (ms).
    pub snapshot_interval_median_ms: Option<i64>,
    /// Content hash of the hour's records per `kind/instrument`, in
    /// canonical order.
    pub content_hashes: BTreeMap<String, String>,
    /// Objects that failed, with the error.
    pub failed: Vec<(String, String)>,
    /// Filtered raw files written.
    pub filtered_files: Vec<String>,
}

impl HourReport {
    /// True if every object of the hour was processed.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Report of a processing run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProcessReport {
    /// One entry per hour, in order.
    pub hours: Vec<HourReport>,
    /// Objects not processed (replay-only datasets or no hour in the key).
    pub skipped: Vec<(String, String)>,
}

/// Errors that stop the run.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The partition writer failed. Partitions may be partly written, so
    /// the run stops.
    #[error(transparent)]
    Write(#[from] crate::partition::WriteError),
    /// Hashing failed.
    #[error("content hash: {0}")]
    Hash(String),
}

/// The pipeline, generic over the injected stages.
#[derive(Debug)]
pub struct Pipeline<'a, F, P, W> {
    raw: &'a dyn ObjectStore,
    filtered: &'a dyn ObjectStore,
    cfg: ProcessConfig,
    filter: F,
    parser: P,
    /// The partition writer (public so callers can recover it).
    pub writer: W,
    external_clock: Option<&'a dyn BlockClock>,
    clock: TableClock,
}

fn order_in_hour(d: Dataset) -> u8 {
    match d {
        Dataset::NodeFillsByBlock => 0,
        Dataset::NodeFills => 1,
        Dataset::NodeTrades => 2,
        Dataset::MarketData => 3,
        Dataset::ReplicaCmds | Dataset::ExplorerBlocks => 4,
    }
}

/// Median of a list (the lower middle for even lengths).
fn median(v: &mut [i64]) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[(v.len() - 1) / 2])
}

/// Content hash per `kind/instrument` of records in canonical order.
///
/// # Errors
///
/// Fails if a record cannot be encoded.
pub fn hashes_by_stream(records: &[&Record]) -> Result<BTreeMap<String, (u64, String)>, String> {
    let mut groups: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    for r in records {
        groups
            .entry(format!("{}/{}", r.kind().as_str(), r.prov().instrument))
            .or_default()
            .push(r);
    }
    groups
        .into_iter()
        .map(|(k, mut rs)| {
            rs.sort_by(|a, b| a.canonical_cmp(b));
            let n = rs.len() as u64;
            mesh_types::content_hash(rs)
                .map(|h| (k, (n, h.to_string())))
                .map_err(|e| e.to_string())
        })
        .collect()
}

impl<'a> Pipeline<'a, DefaultFilter, DefaultParser, ParquetWriter> {
    /// The default pipeline: the node reader's filter and parser for
    /// by-block fills objects, the local ones for the other formats, and
    /// Parquet partitions under the archive root `archive_root` (for
    /// example `<out-dir>/hyperliquid-backfill`).
    pub fn wired(
        raw: &'a dyn ObjectStore,
        filtered: &'a dyn ObjectStore,
        cfg: ProcessConfig,
        registry: mesh_types::Registry,
        archive_root: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self::new(
            raw,
            filtered,
            cfg,
            DefaultFilter::default(),
            DefaultParser::new(registry),
            ParquetWriter::new(archive_root),
        )
    }
}

impl<'a, F: RawLineFilter, P: RecordParser, W: PartitionWriter> Pipeline<'a, F, P, W> {
    /// A pipeline reading the raw tier from `raw` and writing filtered raw
    /// to `filtered`.
    pub fn new(
        raw: &'a dyn ObjectStore,
        filtered: &'a dyn ObjectStore,
        cfg: ProcessConfig,
        filter: F,
        parser: P,
        writer: W,
    ) -> Self {
        Self {
            raw,
            filtered,
            cfg,
            filter,
            parser,
            writer,
            external_clock: None,
            clock: TableClock::new([], TableClock::DEFAULT_MAX_GAP_MS),
        }
    }

    /// Adds a clock consulted after the one built from batched fills.
    #[must_use]
    pub fn with_external_clock(mut self, clock: &'a dyn BlockClock) -> Self {
        self.external_clock = Some(clock);
        self
    }

    fn filtered_path(&self, obj: &SourceObject, day: Day, hour: u8) -> String {
        let base = format!(
            "{}/stream={}/date={}",
            self.cfg.filtered_root.trim_matches('/'),
            obj.dataset,
            day.iso()
        );
        match &obj.info.coin {
            Some(coin) => format!("{base}/{hour:02}/{coin}.jsonl.zst"),
            None => format!("{base}/{hour:02}.jsonl.zst"),
        }
    }

    /// Processes one object; returns its records.
    async fn object(
        &mut self,
        obj: &SourceObject,
        day: Day,
        hour: u8,
        rep: &mut HourReport,
    ) -> Result<(Vec<Record>, Vec<i64>), String> {
        let key =
            dest_key(&self.cfg.raw_prefix, &obj.bucket, &obj.key).map_err(|e| e.to_string())?;
        let bytes = self
            .raw
            .get(&key)
            .await
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())?;
        let text = lz4_to_string(&bytes, self.cfg.max_decompressed).map_err(|e| e.to_string())?;
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let filtered = self
            .filter
            .filter_lines(obj.dataset, &lines)
            .map_err(|e| e.to_string())?;

        let mut body = filtered.lines.join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        let path = self.filtered_path(obj, day, hour);
        let compressed = zstd_archive(body.as_bytes()).map_err(|e| e.to_string())?;
        let out = Path::parse(&path).map_err(|e| e.to_string())?;
        self.filtered
            .put(&out, PutPayload::from(compressed))
            .await
            .map_err(|e| e.to_string())?;

        if matches!(obj.dataset, Dataset::NodeFillsByBlock | Dataset::NodeFills) {
            self.clock
                .extend_from_block_lines(filtered.lines.iter().map(String::as_str));
        }
        let clock = {
            let mut chain: Vec<&dyn BlockClock> = vec![&self.clock];
            if let Some(c) = self.external_clock {
                chain.push(c);
            }
            ChainClock(chain)
        };
        let parsed = self
            .parser
            .parse(obj.dataset, &filtered.lines, &clock)
            .map_err(|e| e.to_string())?;

        let admission = AdmissionId::new(crate::ADMISSION).map_err(|e| e.to_string())?;
        let mut records = parsed.records;
        for r in &mut records {
            let p = r.prov_mut();
            p.admission = admission.clone();
            p.flags.insert(Flag::Backfill);
            p.ts_publish_ns = None;
            r.validate().map_err(|e| e.to_string())?;
            mesh_types::allowlist::check_record(r).map_err(|e| e.to_string())?;
        }

        rep.objects += 1;
        rep.lines_in += filtered.stats.lines_in;
        rep.lines_out += filtered.stats.lines_out;
        rep.events_kept += filtered.stats.events_kept;
        rep.events_dropped += filtered.stats.events_dropped;
        rep.unresolved += parsed.stats.unresolved;
        rep.unknown_coin += parsed.stats.unknown_coin;
        rep.invalid += parsed.stats.invalid;
        rep.superseded += parsed.stats.superseded;
        rep.repeated_empty_blocks += parsed.stats.repeated_empty_blocks;
        if obj.dataset == Dataset::MarketData {
            rep.snapshots_read += filtered.stats.events_kept;
        }
        rep.filtered_files.push(path);
        Ok((records, parsed.stats.snapshot_intervals_ms))
    }

    /// Processes the objects (record-source datasets only), hour by hour.
    ///
    /// # Errors
    ///
    /// Fails if the partition writer fails; per-object failures are in the
    /// report.
    pub async fn run(&mut self, objects: &[SourceObject]) -> Result<ProcessReport, ProcessError> {
        let mut report = ProcessReport::default();
        let mut hours: BTreeMap<(Day, u8), Vec<&SourceObject>> = BTreeMap::new();
        for o in objects {
            match (o.dataset.is_record_source(), o.info.day, o.info.hour) {
                (true, Some(d), Some(h)) => hours.entry((d, h)).or_default().push(o),
                (false, ..) => report.skipped.push((
                    o.key.clone(),
                    "replay input only; not processed into records".into(),
                )),
                _ => report
                    .skipped
                    .push((o.key.clone(), "key has no day and hour".into())),
            }
        }
        for ((day, hour), mut objs) in hours {
            objs.sort_by(|a, b| {
                (order_in_hour(a.dataset), &a.key).cmp(&(order_in_hour(b.dataset), &b.key))
            });
            let mut rep = HourReport {
                hour: format!("{day}/{hour:02}"),
                ..HourReport::default()
            };
            let mut intervals = Vec::new();
            let mut records = Vec::new();
            for obj in objs {
                match self.object(obj, day, hour, &mut rep).await {
                    Ok((rs, iv)) => {
                        intervals.extend(iv);
                        records.extend(rs);
                    }
                    Err(e) => {
                        tracing::error!(key = %obj.key, error = %e, "object failed");
                        rep.failed.push((obj.key.clone(), e));
                    }
                }
            }
            rep.snapshot_interval_median_ms = median(&mut intervals);
            self.finish_hour(records, &mut rep)?;
            // Keep two hours of block times for snapshots near boundaries.
            let start = UnixMillis(day.start_ms().0 + i64::from(hour) * 3_600_000);
            self.clock.retain_from(UnixMillis(start.0 - 3_600_000));
            report.hours.push(rep);
            self.writer.finish_hour(start)?;
        }
        self.writer.flush()?;
        Ok(report)
    }

    fn finish_hour(
        &mut self,
        records: Vec<Record>,
        rep: &mut HourReport,
    ) -> Result<(), ProcessError> {
        let refs: Vec<&Record> = records.iter().collect();
        for (k, (_, h)) in hashes_by_stream(&refs).map_err(ProcessError::Hash)? {
            rep.content_hashes.insert(k, h);
        }
        let mut parts: BTreeMap<PartitionKey, Vec<Record>> = BTreeMap::new();
        for r in records {
            *rep.records.entry(r.kind().as_str().to_owned()).or_default() += 1;
            parts.entry(PartitionKey::of(&r)).or_default().push(r);
        }
        for (key, mut rs) in parts {
            rs.sort_by(|a, b| a.canonical_cmp(b));
            self.writer.write_partition(&key, &rs)?;
        }
        Ok(())
    }
}

/// Hour key `YYYY-MM-DD/HH` of a record's source time.
#[must_use]
pub fn hour_label(r: &Record) -> String {
    let h = hour_of(r.prov().ts_source_ms);
    let day = Day::of(h);
    let hh = (h.0 - day.start_ms().0) / 3_600_000;
    format!("{day}/{hh:02}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::buckets::parse_key;
    use crate::codec::unzstd;
    use crate::filter::LocalFilter;
    use crate::filter::tests::{block_line, fill};
    use crate::parse::LocalParser;
    use crate::parse::tests::{registry, snapshot_line};
    use crate::partition::MemoryWriter;
    use crate::testutil::{RecordingStore, lz4, put};
    use mesh_types::{RecordKind, Seq};

    pub(crate) fn obj(dataset: Dataset, key: &str) -> SourceObject {
        SourceObject {
            dataset,
            bucket: dataset.bucket().name().into(),
            key: key.into(),
            size: 0,
            etag: None,
            info: parse_key(dataset, key).unwrap(),
        }
    }

    /// A synthetic raw tier with one hour of fills and one L2 file.
    pub(crate) async fn synthetic_raw_tier() -> (RecordingStore, Vec<SourceObject>) {
        let raw = RecordingStore::new();
        // 2025-07-27T09:00:00Z = 1_753_606_800_000 ms.
        let t0 = 1_753_606_800_000u64;
        let fills = [
            block_line(
                1000,
                "2025-07-27T09:00:00.100",
                vec![
                    fill("BTC", "100000", "0.5", "B", true, 1, 11, t0 + 100),
                    fill("BTC", "100000", "0.5", "A", false, 1, 12, t0 + 100),
                ],
            ),
            block_line(1001, "2025-07-27T09:00:00.600", vec![]),
            block_line(
                1002,
                "2025-07-27T09:00:01.100",
                vec![
                    fill("SOL", "150.25", "2", "A", true, 2, 21, t0 + 1100),
                    fill("SOL", "150.25", "2", "B", false, 2, 22, t0 + 1100),
                ],
            ),
        ]
        .join("\n");
        put(
            &raw,
            "raw/hl-mainnet-node-data/node_fills_by_block/hourly/20250727/9.lz4",
            lz4(fills.as_bytes()),
        )
        .await;
        let l2 = [
            snapshot_line("SOL", t0 + 200, "150.10", "150.30"),
            snapshot_line("SOL", t0 + 700, "150.20", "150.30"),
        ]
        .join("\n");
        put(
            &raw,
            "raw/hyperliquid-archive/market_data/20250727/9/l2Book/SOL.lz4",
            lz4(l2.as_bytes()),
        )
        .await;
        put(
            &raw,
            "raw/hl-mainnet-node-data/node_trades/hourly/20250727/9.lz4",
            lz4(b"not json"),
        )
        .await;
        let objects = vec![
            obj(Dataset::MarketData, "market_data/20250727/9/l2Book/SOL.lz4"),
            obj(
                Dataset::NodeFillsByBlock,
                "node_fills_by_block/hourly/20250727/9.lz4",
            ),
            obj(Dataset::NodeTrades, "node_trades/hourly/20250727/9.lz4"),
            obj(Dataset::ReplicaCmds, "replica_cmds/20250727/1000.lz4"),
        ];
        (raw, objects)
    }

    pub(crate) fn cfg() -> ProcessConfig {
        ProcessConfig {
            raw_prefix: "raw".into(),
            filtered_root: "filtered".into(),
            max_decompressed: 1 << 20,
        }
    }

    #[tokio::test]
    async fn processes_an_hour_end_to_end() {
        let (raw, objects) = synthetic_raw_tier().await;
        let filtered = RecordingStore::new();
        let mut p = Pipeline::new(
            &raw,
            &filtered,
            cfg(),
            LocalFilter,
            LocalParser::new(registry()),
            MemoryWriter::default(),
        );
        let report = p.run(&objects).await.unwrap();
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.hours.len(), 1);
        let h = &report.hours[0];
        assert_eq!(h.hour, "2025-07-27/09");
        assert_eq!(h.objects, 2);
        assert_eq!(
            h.failed.len(),
            1,
            "the malformed node_trades file fails alone"
        );
        assert!(!h.complete());
        assert_eq!(h.records["trade"], 2);
        assert_eq!(h.records["book_snapshot"], 2);
        assert_eq!(h.snapshots_read, 2);
        assert_eq!(h.snapshot_interval_median_ms, Some(500));
        assert_eq!(h.content_hashes.len(), 3);

        // Every record carries ADM-0003 and the backfill flag.
        let parts = &p.writer.parts;
        for rs in parts.values() {
            for r in rs {
                assert_eq!(r.prov().admission.as_str(), "ADM-0003");
                assert!(r.prov().flags.contains(Flag::Backfill));
            }
        }
        // Snapshots resolved to the blocks before them.
        let snaps: Vec<Seq> = parts
            .iter()
            .filter(|(k, _)| k.kind == RecordKind::BookSnapshot)
            .flat_map(|(_, rs)| rs.iter().map(|r| r.prov().seq))
            .collect();
        assert_eq!(snaps, [Seq::end_of_block(1000), Seq::end_of_block(1001)]);

        // Filtered raw: zstd JSON lines without excluded fields.
        let path = Path::parse("filtered/stream=node_fills_by_block/date=2025-07-27/09.jsonl.zst")
            .unwrap();
        let text = String::from_utf8(
            unzstd(&filtered.get(&path).await.unwrap().bytes().await.unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(text.lines().count(), 3);
        for f in mesh_types::allowlist::EXCLUDED_SOURCE_FIELDS {
            assert!(!text.contains(&format!("\"{f}\"")), "{f}");
        }
        assert!(
            filtered
                .get(
                    &Path::parse("filtered/stream=market_data/date=2025-07-27/09/SOL.jsonl.zst")
                        .unwrap()
                )
                .await
                .is_ok()
        );
    }

    #[test]
    fn hour_labels_and_median() {
        let r = crate::partition::tests::trade(
            "hyperliquid:perp:BTC",
            1,
            0,
            1_753_606_800_000 + 3_599_999,
        );
        assert_eq!(hour_label(&r), "2025-07-27/09");
        assert_eq!(median(&mut [5, 1, 3, 2]), Some(2));
        assert_eq!(median(&mut []), None);
    }
}
