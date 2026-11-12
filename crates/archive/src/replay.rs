//! Replay: the only path from filtered raw files to normalized partitions,
//! for live days, rebuilds and backfills alike (ADR 0006).
//!
//! For day `D`, the inputs are every filtered raw file
//! `stream=<s>/date=<D>/<HH>.jsonl.zst` plus, for each such stream, hour
//! `00` of `D + 1`: a block whose source time is just before midnight can be
//! written to the next node hour file. The injected [`RawParser`] (the node
//! reader's) turns the decompressed files into records; records whose
//! partition day is not `D` are dropped (they belong to the neighbouring
//! day's replay). The partition writer then writes `D`'s partitions.
//!
//! The output is a pure function of the input files (listed with their
//! SHA-256 in the report) and the code version: the canonical content hash
//! is the gate, and byte identity holds for a pinned build.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mesh_types::{ContentHash, ContentHasher, Record, Sha256Digest};

use crate::capture;
use crate::error::{BoxError, Error, IoContext, Result};
use crate::layout::{self, Day};
use crate::partition::{self, PartitionOptions, WrittenFile};

/// One decompressed filtered raw file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFile {
    /// Stream slug.
    pub stream: String,
    /// Day of the node file.
    pub day: Day,
    /// Hour of the node file.
    pub hour: u8,
    /// Path relative to the raw archive root.
    pub path: String,
    /// Decompressed JSON lines.
    pub data: Vec<u8>,
}

/// Turns filtered raw files into records. The node reader provides the
/// implementation; the archive only orchestrates. Files arrive sorted by
/// (day, hour, stream).
pub trait RawParser: Send + Sync {
    /// Parses the files of one replay.
    ///
    /// # Errors
    ///
    /// Any parse failure aborts the replay.
    fn parse(&self, files: &[RawFile]) -> Result<Vec<Record>, BoxError>;
}

impl<F> RawParser for F
where
    F: Fn(&[RawFile]) -> Result<Vec<Record>, BoxError> + Send + Sync,
{
    fn parse(&self, files: &[RawFile]) -> Result<Vec<Record>, BoxError> {
        self(files)
    }
}

/// Replay settings.
#[derive(Debug, Clone, Copy)]
pub struct ReplayOptions {
    /// Fail if a stream with files on `D` has no hour `00` on `D + 1`
    /// (true for scheduled daily runs, so a day is replayed only once its
    /// inputs are complete).
    pub require_next_hour: bool,
    /// Partition writer settings.
    pub partition: PartitionOptions,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            require_next_hour: true,
            partition: PartitionOptions::default(),
        }
    }
}

/// What a replay read and wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    /// The day replayed.
    pub day: Day,
    /// Input files with their SHA-256 (compressed bytes).
    pub inputs: InputList,
    /// Streams with files on the day but no hour `00` the next day.
    pub missing_next_hour: Vec<String>,
    /// Records the parser produced for other days (dropped).
    pub out_of_day: u64,
    /// Records written.
    pub records: u64,
    /// Content hash of all the day's records in canonical order
    /// (instrument, kind, seq): one fingerprint for the whole day.
    pub day_content_hash: ContentHash,
    /// Partition files written.
    pub files: Vec<WrittenFile>,
}

/// Input files and their hashes, as `(path, SHA-256 of the compressed file)`.
pub type InputList = Vec<(String, Sha256Digest)>;

/// The loaded inputs of a replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    /// Decompressed files, sorted by (day, hour, stream).
    pub files: Vec<RawFile>,
    /// Paths and hashes.
    pub list: InputList,
    /// Streams without a next-day hour 00.
    pub missing_next_hour: Vec<String>,
}

/// Parsed records of a replay, before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The day's records in canonical order.
    pub records: Vec<Record>,
    /// Paths and hashes of the inputs.
    pub inputs: InputList,
    /// Streams without a next-day hour 00.
    pub missing_next_hour: Vec<String>,
    /// Records dropped because they belong to another day.
    pub out_of_day: u64,
}

/// Loads the inputs of `day` from the filtered raw archive root.
///
/// # Errors
///
/// Fails on I/O or decompression errors, or a missing next hour when
/// required.
pub fn load_inputs(raw_root: &Path, day: Day, require_next_hour: bool) -> Result<Inputs> {
    let all = layout::list_files(raw_root)?;
    let mut chosen: BTreeMap<(Day, u8, String), String> = BTreeMap::new();
    let mut streams = BTreeSet::new();
    for p in &all {
        if let Some(rp) = layout::parse_raw_path(p)
            && rp.day == day
        {
            streams.insert(rp.stream.clone());
            chosen.insert((rp.day, rp.hour, rp.stream), p.clone());
        }
    }
    let mut missing = Vec::new();
    for s in &streams {
        let next = layout::raw_path(s, day.next(), 0);
        if all.binary_search(&next).is_ok() {
            chosen.insert((day.next(), 0, s.clone()), next);
        } else {
            missing.push(s.clone());
        }
    }
    if require_next_hour && !missing.is_empty() {
        return Err(Error::invalid(format!(
            "replay of {day} needs hour 00 of {} for streams {missing:?}; wait for it or allow it to be missing",
            day.next()
        )));
    }
    let no_dicts = BTreeMap::new();
    let mut files = Vec::with_capacity(chosen.len());
    let mut inputs = Vec::with_capacity(chosen.len());
    for ((d, h, stream), rel) in chosen {
        let local = raw_root.join(&rel);
        let compressed = std::fs::read(&local).at(&local)?;
        inputs.push((rel.clone(), Sha256Digest::of(&compressed)));
        files.push(RawFile {
            stream,
            day: d,
            hour: h,
            path: rel,
            data: capture::decompress(&compressed, &no_dicts)?,
        });
    }
    Ok(Inputs {
        files,
        list: inputs,
        missing_next_hour: missing,
    })
}

/// Parses `day`'s inputs into the day's records (canonical order), without
/// writing.
///
/// # Errors
///
/// Fails on input or parser errors.
pub fn replay_records(
    raw_root: &Path,
    day: Day,
    parser: &dyn RawParser,
    require_next_hour: bool,
) -> Result<Parsed> {
    let inputs = load_inputs(raw_root, day, require_next_hour)?;
    let parsed = parser
        .parse(&inputs.files)
        .map_err(|source| Error::Component {
            context: "raw parser",
            source,
        })?;
    let before = parsed.len() as u64;
    let mut records: Vec<Record> = parsed
        .into_iter()
        .filter(|r| partition::record_day(r) == day)
        .collect();
    let out_of_day = before - records.len() as u64;
    records.sort_by(Record::canonical_cmp);
    Ok(Parsed {
        records,
        inputs: inputs.list,
        missing_next_hour: inputs.missing_next_hour,
        out_of_day,
    })
}

/// Replays `day` from the filtered raw archive root `raw_root` into the
/// normalized archive root `out_root`.
///
/// # Errors
///
/// Fails on input, parser or partition errors (including write-once
/// conflicts with existing partitions and the content-hash gate).
pub fn replay_day(
    raw_root: &Path,
    day: Day,
    parser: &dyn RawParser,
    out_root: &Path,
    opts: &ReplayOptions,
) -> Result<ReplayReport> {
    let Parsed {
        records,
        inputs,
        missing_next_hour: missing,
        out_of_day,
    } = replay_records(raw_root, day, parser, opts.require_next_hour)?;
    let mut h = ContentHasher::new();
    for r in &records {
        h.update(r)?;
    }
    let count = h.count();
    let day_content_hash = h.finalize();
    let files = partition::write_partitions(out_root, records, &opts.partition)?;
    crate::metrics::inc("archive_replay_records_total", &[], count);
    tracing::info!(%day, records = count, files = files.len(), out_of_day, %day_content_hash, "replayed day");
    Ok(ReplayReport {
        day,
        inputs,
        missing_next_hour: missing,
        out_of_day,
        records: count,
        day_content_hash,
        files,
    })
}

/// Compares a replay's output with existing partitions: every file the
/// replay produced must have the same content hash (and, if `bytes`, the
/// same SHA-256) as the file at the same path under `existing_root`.
/// Returns the paths that differ or are missing.
///
/// # Errors
///
/// Fails on I/O or Parquet errors.
pub fn compare_with(
    report: &ReplayReport,
    existing_root: &Path,
    bytes: bool,
) -> Result<Vec<String>> {
    let mut differ = Vec::new();
    for f in &report.files {
        let p = existing_root.join(&f.path);
        if !p.exists() {
            differ.push(f.path.clone());
            continue;
        }
        let data = std::fs::read(&p).at(&p)?;
        if bytes {
            if Sha256Digest::of(&data) != f.sha256 {
                differ.push(f.path.clone());
            }
            continue;
        }
        let read = partition::read_parquet(bytes::Bytes::from(data))?;
        if read.content_hash != f.content_hash || !read.consistent() {
            differ.push(f.path.clone());
        }
    }
    Ok(differ)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{DAY_MS, trade};

    /// Lines of `{"b":block,"i":index,"t":ms}`; one trade each.
    fn parser(files: &[RawFile]) -> Result<Vec<Record>, BoxError> {
        let mut out = Vec::new();
        for f in files {
            for line in std::str::from_utf8(&f.data)?.lines() {
                let v: serde_json::Value = serde_json::from_str(line)?;
                out.push(trade(
                    "hyperliquid:perp:SYN",
                    v["b"].as_u64().ok_or("b")?,
                    u32::try_from(v["i"].as_u64().ok_or("i")?)?,
                    v["t"].as_i64().ok_or("t")?,
                ));
            }
        }
        Ok(out)
    }

    fn put(root: &Path, rel: &str, lines: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, capture::compress_filtered(lines.as_bytes()).unwrap()).unwrap();
    }

    #[test]
    fn replays_with_next_hour_and_drops_other_days() {
        let raw = tempfile::tempdir().unwrap();
        let day: Day = "2026-01-01".parse().unwrap();
        // Hour 00 of D holds a record from D-1 (dropped); hour 00 of D+1 holds
        // one from D (kept) and one from D+1 (dropped).
        put(
            raw.path(),
            "stream=fills/date=2026-01-01/00.jsonl.zst",
            &format!(
                "{{\"b\":1,\"i\":0,\"t\":{}}}\n{{\"b\":2,\"i\":0,\"t\":{}}}\n",
                DAY_MS - 1,
                DAY_MS + 5
            ),
        );
        put(
            raw.path(),
            "stream=fills/date=2026-01-01/23.jsonl.zst",
            &format!("{{\"b\":3,\"i\":0,\"t\":{}}}\n", DAY_MS + 86_399_000),
        );
        let next = DAY_MS + 86_400_000;
        put(
            raw.path(),
            "stream=fills/date=2026-01-02/00.jsonl.zst",
            &format!(
                "{{\"b\":4,\"i\":0,\"t\":{}}}\n{{\"b\":5,\"i\":0,\"t\":{}}}\n",
                next - 1,
                next
            ),
        );
        let out1 = tempfile::tempdir().unwrap();
        let out2 = tempfile::tempdir().unwrap();
        let r1 = replay_day(
            raw.path(),
            day,
            &parser,
            out1.path(),
            &ReplayOptions::default(),
        )
        .unwrap();
        let r2 = replay_day(
            raw.path(),
            day,
            &parser,
            out2.path(),
            &ReplayOptions::default(),
        )
        .unwrap();
        assert_eq!(r1.records, 3);
        assert_eq!(r1.out_of_day, 2);
        assert_eq!(r1.inputs.len(), 3);
        assert!(r1.missing_next_hour.is_empty());
        assert_eq!(r1.files, r2.files);
        assert_eq!(r1.day_content_hash, r2.day_content_hash);
        assert!(compare_with(&r1, out2.path(), true).unwrap().is_empty());
        assert!(compare_with(&r1, out2.path(), false).unwrap().is_empty());

        // Without the next hour: refused unless allowed.
        std::fs::remove_file(raw.path().join("stream=fills/date=2026-01-02/00.jsonl.zst")).unwrap();
        let out3 = tempfile::tempdir().unwrap();
        assert!(
            replay_day(
                raw.path(),
                day,
                &parser,
                out3.path(),
                &ReplayOptions::default()
            )
            .is_err()
        );
        let r3 = replay_day(
            raw.path(),
            day,
            &parser,
            out3.path(),
            &ReplayOptions {
                require_next_hour: false,
                ..ReplayOptions::default()
            },
        )
        .unwrap();
        assert_eq!(r3.missing_next_hour, vec!["fills".to_owned()]);
        assert_eq!(r3.records, 2);
        // The different output is detected.
        assert_eq!(compare_with(&r3, out1.path(), false).unwrap().len(), 1);
    }

    #[test]
    fn parser_errors_are_reported() {
        let raw = tempfile::tempdir().unwrap();
        put(
            raw.path(),
            "stream=fills/date=2026-01-01/00.jsonl.zst",
            "not json\n",
        );
        put(raw.path(), "stream=fills/date=2026-01-02/00.jsonl.zst", "");
        let out = tempfile::tempdir().unwrap();
        let err = replay_day(
            raw.path(),
            "2026-01-01".parse().unwrap(),
            &parser,
            out.path(),
            &ReplayOptions::default(),
        );
        assert!(matches!(
            err,
            Err(Error::Component {
                context: "raw parser",
                ..
            })
        ));
    }
}
