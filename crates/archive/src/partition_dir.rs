//! Partition a directory of daily record files, once per closed day.
//!
//! Pollers such as swap-pollers and reference-pollers write records as
//! `<records-dir>/date=YYYY-MM-DD/<file>`. This module writes each day's
//! file into the normalized archive once the day has settled (it is at least
//! `settle_days` whole days before today, UTC), and remembers what it wrote
//! with a marker holding the file's SHA-256. Partitions are write-once
//! ([`Existing::RequireIdentical`]): a day whose file changes after it was
//! partitioned is reported (log and `archive_partition_dir_changed_total`)
//! and left alone, because fixing it is a deliberate correction.
//!
//! Day files can be gigabytes (a day of public swap prints), so a day is
//! partitioned with bounded memory ([`partition_file`]): one pass parses and
//! checks every record and spills each line to a bucket file chosen by its
//! partition key, then each bucket (every record of the partitions it holds)
//! is read and written in turn. The files written are the same as
//! [`partition::write_partitions`] writes for the whole day at once.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mesh_types::{Record, Seq};
use sha2::{Digest, Sha256};

use crate::error::{Error, IoContext, Result};
use crate::layout::{Day, PartitionKey};
use crate::partition::{self, Existing, PartitionOptions};

/// Day-file bytes per spill bucket. A bucket's records are held in memory
/// while its partitions are written, so this (or the largest single
/// partition, if larger) sets peak memory.
pub const BUCKET_BYTES: u64 = 64 << 20;
/// Upper bound on spill buckets (open files) per day file.
const MAX_BUCKETS: u64 = 1024;

/// Options for [`scan`].
#[derive(Debug, Clone)]
pub struct PartitionDir {
    /// Directory holding `date=YYYY-MM-DD/<file>` subdirectories.
    pub records_dir: PathBuf,
    /// Record file names inside each day directory (for example
    /// `swap_print.jsonl`, or `reference_value.jsonl` and `positioning.jsonl`).
    pub files: Vec<String>,
    /// Normalized archive root.
    pub out: PathBuf,
    /// Directory for markers (`<state_dir>/<file>/<day>.sha256`).
    pub state_dir: PathBuf,
    /// Whole days a day must be in the past before it is partitioned.
    pub settle_days: u32,
}

/// What one scan did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    /// Day files written this scan.
    pub written: Vec<(Day, String)>,
    /// Day files already partitioned and unchanged.
    pub unchanged: usize,
    /// Day files that changed after partitioning (left alone).
    pub changed: Vec<(Day, String)>,
    /// Day files not settled yet.
    pub pending: usize,
}

/// Scan once, partitioning every settled day that has no marker.
pub fn scan(opts: &PartitionDir, today: Day) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let mut days = Vec::new();
    for file in &opts.files {
        let state = opts.state_dir.join(file);
        std::fs::create_dir_all(&state).at(&state)?;
        days.extend(
            list_days(&opts.records_dir, file)?
                .into_iter()
                .map(|(d, p)| (d, file.clone(), p)),
        );
    }
    days.sort();
    for (day, file, path) in days {
        if !settled(day, today, opts.settle_days) {
            report.pending += 1;
            continue;
        }
        let digest = file_sha256(&path)?;
        let marker = opts.state_dir.join(&file).join(format!("{day}.sha256"));
        match std::fs::read_to_string(&marker) {
            Ok(prev) if prev.trim() == digest => {
                report.unchanged += 1;
                continue;
            }
            Ok(_) => {
                tracing::warn!(day = %day, file = %path.display(),
                    "record file changed after it was partitioned; left alone (write a correction to change it)");
                crate::metrics::inc("archive_partition_dir_changed_total", &[], 1);
                report.changed.push((day, file));
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).at(&marker),
        }
        let spill = opts.state_dir.join(&file).join(format!("{day}.spill"));
        let (n, files) = partition_file(&path, &opts.out, &spill, BUCKET_BYTES)?;
        write_marker(&marker, &digest)?;
        tracing::info!(day = %day, records = n, files, "partitioned");
        crate::metrics::inc("archive_partition_dir_days_total", &[], 1);
        crate::metrics::inc("archive_partition_dir_records_total", &[], n as u64);
        report.written.push((day, file));
    }
    Ok(report)
}

/// Scan every `interval` until the process is stopped.
pub async fn follow(opts: &PartitionDir, interval: Duration) -> Result<()> {
    loop {
        match scan(opts, today_utc()) {
            Ok(r) => tracing::info!(
                written = r.written.len(),
                unchanged = r.unchanged,
                changed = r.changed.len(),
                pending = r.pending,
                "scan done"
            ),
            Err(e) => {
                tracing::error!(error = %e, "scan failed; retrying next interval");
                crate::metrics::inc("archive_partition_dir_errors_total", &[], 1);
            }
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            _ = tokio::signal::ctrl_c() => return Ok(()),
            () = sigterm() => return Ok(()),
        }
    }
}

/// Resolves on SIGTERM.
#[cfg(unix)]
pub(crate) async fn sigterm() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut s) => {
            s.recv().await;
        }
        Err(_) => std::future::pending::<()>().await,
    }
}

/// Never resolves (no SIGTERM here).
#[cfg(not(unix))]
pub(crate) async fn sigterm() {
    std::future::pending::<()>().await
}

/// Today's UTC date.
pub fn today_utc() -> Day {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Day::from_millis(ms)
}

fn settled(day: Day, today: Day, settle_days: u32) -> bool {
    let age_ms = today.start_ms() - day.start_ms();
    age_ms >= i64::from(settle_days) * 86_400_000
}

fn list_days(dir: &Path, file: &str) -> Result<Vec<(Day, PathBuf)>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).at(dir),
    };
    for entry in entries {
        let entry = entry.at(dir)?;
        let name = entry.file_name();
        let Some(date) = name.to_str().and_then(|n| n.strip_prefix("date=")) else {
            continue;
        };
        let Ok(day) = date.parse::<Day>() else {
            continue;
        };
        let path = entry.path().join(file);
        if path.is_file() {
            out.push((day, path));
        }
    }
    Ok(out)
}

/// Writes every partition of the JSON-lines record file `path` under the
/// archive root `out` (write-once), holding at most one spill bucket's
/// records in memory. `spill` is a scratch directory, removed afterwards;
/// it needs free space about the size of the file. Returns the number of
/// records and of partition files.
///
/// The result matches [`partition::write_partitions`] over the whole file
/// with [`Existing::RequireIdentical`]: the same files with the same bytes.
/// Every record is validated and every partition checked for duplicate
/// `seq` before anything is written, with the same errors.
///
/// # Errors
///
/// Fails on unreadable or invalid lines, invalid or duplicate records, I/O
/// errors and write-once conflicts.
pub fn partition_file(
    path: &Path,
    out: &Path,
    spill: &Path,
    bucket_bytes: u64,
) -> Result<(usize, usize)> {
    let len = std::fs::metadata(path).at(path)?.len();
    let buckets = (len / bucket_bytes.max(1) + 1).min(MAX_BUCKETS);
    let spill = SpillDir::create(spill)?;

    // Pass 1: parse, validate and spill each line to its partition's
    // bucket; collect each partition's seqs for the duplicate check.
    let mut writers: Vec<Option<BufWriter<std::fs::File>>> = (0..buckets).map(|_| None).collect();
    let mut seqs: BTreeMap<PartitionKey, Vec<Seq>> = BTreeMap::new();
    let mut records = 0usize;
    for_each_record(path, |r, line| {
        partition::validate_record(&r)?;
        let key = partition::partition_key(&r);
        let b = bucket_of(&key, buckets);
        let w = match &mut writers[b] {
            Some(w) => w,
            slot => {
                let p = spill.bucket(b);
                let f = std::fs::File::create(&p).at(&p)?;
                slot.insert(BufWriter::with_capacity(1 << 16, f))
            }
        };
        w.write_all(line.as_bytes()).at(spill.bucket(b))?;
        w.write_all(b"\n").at(spill.bucket(b))?;
        let seq = r.prov().seq;
        match seqs.get_mut(&key) {
            Some(v) => v.push(seq),
            None => {
                seqs.insert(key, vec![seq]);
            }
        }
        records += 1;
        Ok(())
    })?;
    let mut present = Vec::new();
    for (b, w) in writers.into_iter().enumerate() {
        if let Some(w) = w {
            w.into_inner()
                .map_err(std::io::IntoInnerError::into_error)
                .at(spill.bucket(b))?;
            present.push(b);
        }
    }
    // Duplicate seqs, in partition order, as `partition::plan` reports them.
    for (key, v) in &mut seqs {
        v.sort_unstable();
        if let Some(w) = v.windows(2).find(|w| w[0] == w[1]) {
            return Err(partition::duplicate_seq(key, w[0]));
        }
    }
    drop(seqs);

    // Pass 2: one bucket at a time. Partitions never span buckets, and each
    // partition's files depend only on its own records.
    let po = PartitionOptions {
        validate: false, // done in pass 1
        verify_readback: true,
        existing: Existing::RequireIdentical,
    };
    let mut files = 0usize;
    for b in present {
        let p = spill.bucket(b);
        let mut recs = Vec::new();
        for_each_record(&p, |r, _| {
            recs.push(r);
            Ok(())
        })?;
        files += partition::write_partitions(out, recs, &po)?.len();
        std::fs::remove_file(&p).at(&p)?;
    }
    Ok((records, files))
}

/// Calls `f` with each record of a JSON-lines file and its line (without
/// the line ending). Blank lines are skipped; lines end in `\n` or `\r\n`.
fn for_each_record(path: &Path, mut f: impl FnMut(Record, &str) -> Result<()>) -> Result<()> {
    let mut reader = BufReader::with_capacity(1 << 16, std::fs::File::open(path).at(path)?);
    let mut buf = Vec::new();
    let mut line_no = 0u64;
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).at(path)? == 0 {
            return Ok(());
        }
        line_no += 1;
        let bad = |e: &dyn std::fmt::Display| {
            Error::invalid(format!("{}: line {line_no}: {e}", path.display()))
        };
        let mut bytes = buf.as_slice();
        if let Some(b) = bytes.strip_suffix(b"\n") {
            bytes = b.strip_suffix(b"\r").unwrap_or(b);
        }
        let line = std::str::from_utf8(bytes).map_err(|e| bad(&e))?;
        if line.trim().is_empty() {
            continue;
        }
        let r: Record = serde_json::from_str(line).map_err(|e| bad(&e))?;
        f(r, line)?;
    }
}

/// The spill bucket of a partition. Any fixed function of the key works:
/// buckets only bound memory and never change what is written.
fn bucket_of(key: &PartitionKey, buckets: u64) -> usize {
    let mut h = std::hash::DefaultHasher::new();
    key.hash(&mut h);
    usize::try_from(h.finish() % buckets).unwrap_or(0)
}

/// A scratch directory for spill buckets, emptied on creation (a crashed
/// run may have left one) and removed on drop.
struct SpillDir(PathBuf);

impl SpillDir {
    fn create(dir: &Path) -> Result<Self> {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).at(dir),
        }
        std::fs::create_dir_all(dir).at(dir)?;
        Ok(Self(dir.to_path_buf()))
    }

    fn bucket(&self, b: usize) -> PathBuf {
        self.0.join(format!("bucket-{b:04}.jsonl"))
    }
}

impl Drop for SpillDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            tracing::warn!(dir = %self.0.display(), error = %e, "could not remove spill directory");
        }
    }
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).at(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).at(path)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn write_marker(marker: &Path, digest: &str) -> Result<()> {
    let tmp = marker.with_extension("sha256.tmp");
    std::fs::write(&tmp, format!("{digest}\n")).at(&tmp)?;
    std::fs::rename(&tmp, marker).at(marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn write_day(root: &Path, day: &str, lines: &[String]) {
        let d = root.join(format!("date={day}"));
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join("swap_print.jsonl"), lines.join("\n")).expect("write");
    }

    fn sample_lines(day_ms: i64) -> Vec<String> {
        (0..3u32)
            .map(|i| {
                let r = testutil::trade("hyperliquid:perp:BTC", 1, i, day_ms + i64::from(i) * 1000);
                serde_json::to_string(&r).expect("json")
            })
            .collect()
    }

    fn opts(tmp: &Path) -> PartitionDir {
        PartitionDir {
            records_dir: tmp.join("records"),
            files: vec!["swap_print.jsonl".into()],
            out: tmp.join("normalized"),
            state_dir: tmp.join("state"),
            settle_days: 2,
        }
    }

    #[test]
    fn partitions_settled_days_once_and_reports_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let o = opts(tmp.path());
        let d1: Day = "2026-01-01".parse().expect("day");
        let d2: Day = "2026-01-02".parse().expect("day");
        let d3: Day = "2026-01-03".parse().expect("day");
        write_day(&o.records_dir, "2026-01-01", &sample_lines(d1.start_ms()));
        write_day(&o.records_dir, "2026-01-02", &sample_lines(d2.start_ms()));
        write_day(&o.records_dir, "2026-01-03", &sample_lines(d3.start_ms()));
        let today: Day = "2026-01-04".parse().expect("day");

        let r = scan(&o, today).expect("scan");
        let f = || "swap_print.jsonl".to_string();
        assert_eq!(r.written, vec![(d1, f()), (d2, f())]);
        assert_eq!(r.pending, 1);

        let again = scan(&o, today).expect("rescan");
        assert!(again.written.is_empty());
        assert_eq!(again.unchanged, 2);

        // A changed file after partitioning is reported, not rewritten.
        let mut lines = sample_lines(d1.start_ms());
        lines.truncate(1);
        write_day(&o.records_dir, "2026-01-01", &lines);
        let changed = scan(&o, today).expect("scan after change");
        assert_eq!(changed.changed, vec![(d1, f())]);

        // The next day settles.
        let later = scan(&o, "2026-01-05".parse().expect("day")).expect("later");
        assert_eq!(later.written, vec![(d3, f())]);
    }

    /// The in-memory path `scan` used before streaming: the whole file as
    /// one `Vec<Record>`.
    fn read_records(path: &Path) -> Vec<Record> {
        std::fs::read_to_string(path)
            .expect("read")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("json"))
            .collect()
    }

    /// A synthetic day file with many partitions (every kind, several
    /// instruments, records on the next day too), lines in no particular
    /// order, blank lines and a CRLF line ending.
    fn multi_key_day_file(path: &Path) {
        let mut recs = testutil::every_kind();
        for (n, inst) in ["SYN", "AAA", "BBB", "CCC", "DDD"].iter().enumerate() {
            let inst = format!("hyperliquid:perp:{inst}:X");
            for i in 0..40u32 {
                // Interleave blocks so seq order differs from file order,
                // and put a few records on the next day.
                let block = 100 + u64::from((i * 7) % 5);
                let ts = testutil::DAY_MS + i64::from(i) * 60_000 + n as i64;
                let ts = if i % 13 == 0 { ts + 86_400_000 } else { ts };
                recs.push(testutil::trade(&inst, block, i, ts));
            }
        }
        // Deterministic shuffle.
        let len = recs.len();
        let order: Vec<usize> = (0..len).map(|i| (i * 37 + 11) % len).collect();
        let mut text = String::new();
        for (j, i) in order.into_iter().enumerate() {
            text.push_str(&serde_json::to_string(&recs[i]).expect("json"));
            text.push_str(match j % 17 {
                3 => "\r\n",
                5 => "\n\n",
                9 => "\n  \n",
                _ => "\n",
            });
        }
        std::fs::write(path, text).expect("write");
    }

    fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
        crate::layout::list_files(root)
            .expect("list")
            .into_iter()
            .map(|p| {
                let bytes = std::fs::read(root.join(&p)).expect("read");
                (p, bytes)
            })
            .collect()
    }

    #[test]
    fn streamed_partitions_match_in_memory() {
        let tmp = tempfile::tempdir().expect("tmp");
        let day_file = tmp.path().join("swap_print.jsonl");
        multi_key_day_file(&day_file);

        let records = read_records(&day_file);
        let n = records.len();
        let mem_out = tmp.path().join("in-memory");
        let po = PartitionOptions {
            existing: Existing::RequireIdentical,
            ..PartitionOptions::default()
        };
        let mem_files = partition::write_partitions(&mem_out, records, &po).expect("in memory");
        let expected = tree(&mem_out);
        assert!(expected.len() > 10, "{} files", expected.len());

        // Many small buckets, and a single bucket.
        for (name, bucket_bytes) in [("tiny", 256), ("one", u64::MAX)] {
            let out = tmp.path().join(name);
            let spill = tmp.path().join(format!("{name}.spill"));
            let (records, files) =
                partition_file(&day_file, &out, &spill, bucket_bytes).expect("streamed");
            assert_eq!((records, files), (n, mem_files.len()), "{name}");
            assert_eq!(tree(&out), expected, "{name}");
            assert!(!spill.exists(), "{name}: spill directory left behind");
        }

        // Write-once: a second streamed run over an existing archive is
        // accepted and changes nothing.
        let spill = tmp.path().join("again.spill");
        partition_file(&day_file, &mem_out, &spill, 256).expect("identical rerun");
        assert_eq!(tree(&mem_out), expected);
    }

    #[test]
    fn streamed_errors_match_in_memory_and_write_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ms = testutil::DAY_MS;
        let mut bad = testutil::trade("hyperliquid:perp:BBB:X", 1, 0, ms);
        if let Record::Trade(t) = &mut bad {
            t.size = testutil::sz("0");
        }
        let cases = [
            // Duplicate seq in the second partition (in key order).
            vec![
                testutil::trade("hyperliquid:perp:BBB:X", 1, 0, ms),
                testutil::trade("hyperliquid:perp:AAA:X", 1, 0, ms),
                testutil::trade("hyperliquid:perp:BBB:X", 1, 0, ms + 1),
                testutil::trade("hyperliquid:perp:AAA:X", 1, 1, ms),
            ],
            // An invalid record after valid ones.
            vec![testutil::trade("hyperliquid:perp:AAA:X", 1, 0, ms), bad],
        ];
        for (i, recs) in cases.into_iter().enumerate() {
            let day_file = tmp.path().join(format!("case-{i}.jsonl"));
            let lines: Vec<String> = recs
                .iter()
                .map(|r| serde_json::to_string(r).expect("json"))
                .collect();
            std::fs::write(&day_file, lines.join("\n")).expect("write");
            let mem_err = partition::write_partitions(
                &tmp.path().join("mem"),
                recs,
                &PartitionOptions::default(),
            )
            .expect_err("in memory");
            let out = tmp.path().join(format!("out-{i}"));
            let spill = tmp.path().join(format!("spill-{i}"));
            let err = partition_file(&day_file, &out, &spill, 1).expect_err("streamed");
            assert_eq!(err.to_string(), mem_err.to_string(), "case {i}");
            assert!(tree(&out).is_empty(), "case {i}: wrote files");
            assert!(!spill.exists(), "case {i}: spill directory left behind");
        }
    }

    #[test]
    fn missing_records_dir_is_empty() {
        let tmp = tempfile::tempdir().expect("tmp");
        let r = scan(&opts(tmp.path()), today_utc()).expect("scan");
        assert_eq!(r, ScanReport::default());
    }
}
