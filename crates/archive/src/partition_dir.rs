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

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mesh_types::Record;
use sha2::{Digest, Sha256};

use crate::error::{Error, IoContext, Result};
use crate::layout::Day;
use crate::partition::{self, Existing, PartitionOptions};

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
        let records = read_records(&path)?;
        let n = records.len();
        let po = PartitionOptions {
            existing: Existing::RequireIdentical,
            ..PartitionOptions::default()
        };
        let files = partition::write_partitions(&opts.out, records, &po)?;
        write_marker(&marker, &digest)?;
        tracing::info!(day = %day, records = n, files = files.len(), "partitioned");
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

#[cfg(unix)]
async fn sigterm() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut s) => {
            s.recv().await;
        }
        Err(_) => std::future::pending::<()>().await,
    }
}

#[cfg(not(unix))]
async fn sigterm() {
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

fn read_records(path: &Path) -> Result<Vec<Record>> {
    let reader = BufReader::new(std::fs::File::open(path).at(path)?);
    let mut records = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.at(path)?;
        if line.trim().is_empty() {
            continue;
        }
        let r: Record = serde_json::from_str(&line)
            .map_err(|e| Error::invalid(format!("{}: line {}: {e}", path.display(), i + 1)))?;
        records.push(r);
    }
    Ok(records)
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

    #[test]
    fn missing_records_dir_is_empty() {
        let tmp = tempfile::tempdir().expect("tmp");
        let r = scan(&opts(tmp.path()), today_utc()).expect("scan");
        assert_eq!(r, ScanReport::default());
    }
}
