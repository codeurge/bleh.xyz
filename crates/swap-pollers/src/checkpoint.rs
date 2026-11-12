//! Durable poller state, written atomically after every captured file.
//!
//! It records which report files have been captured (so a restart neither
//! refetches nor re-emits them), the listing validators for conditional
//! requests, and which days the live slice poller covered (so end-of-day
//! files for those days are archived without re-emitting their records
//! outside the dedupe window).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::http::Validators;
use crate::parse::ReportKind;
use crate::raw::write_atomic;

/// One captured file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedFile {
    /// Report kind.
    pub kind: ReportKind,
    /// Batch number used for its records.
    pub batch: u64,
    /// SHA-256 of the raw bytes.
    pub sha256: String,
    /// Rows read.
    pub rows: u64,
    /// Records emitted.
    pub records: u64,
    /// Whether records were emitted (false when only archived).
    pub emitted: bool,
}

/// Poller state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Format version.
    #[serde(default)]
    pub version: u32,
    /// Validators per listing name.
    #[serde(default)]
    pub listings: BTreeMap<String, Validators>,
    /// Captured files per bucket key, by file name.
    #[serde(default)]
    pub files: BTreeMap<String, BTreeMap<String, CapturedFile>>,
    /// Days (UTC, `YYYY-MM-DD`) with at least one live slice captured, per
    /// bucket key.
    #[serde(default)]
    pub slice_days: BTreeMap<String, BTreeSet<String>>,
}

/// Checkpoint stored at a path.
#[derive(Debug)]
pub struct CheckpointStore {
    path: PathBuf,
    /// The current state.
    pub state: Checkpoint,
}

impl CheckpointStore {
    /// Loads the checkpoint at `path`, or starts empty if there is none.
    ///
    /// # Errors
    ///
    /// Fails if the file exists but cannot be read or parsed; a corrupt
    /// checkpoint must not silently restart capture.
    pub fn open(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let state = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| anyhow::anyhow!("checkpoint {} is corrupt: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Checkpoint {
                version: 1,
                ..Checkpoint::default()
            },
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, state })
    }

    /// The checkpoint path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True if `file` of `bucket` was captured.
    #[must_use]
    pub fn has(&self, bucket: &str, file: &str) -> bool {
        self.state
            .files
            .get(bucket)
            .is_some_and(|m| m.contains_key(file))
    }

    /// Records a captured file.
    pub fn mark(
        &mut self,
        bucket: &str,
        file: &str,
        captured: CapturedFile,
        slice_day: Option<String>,
    ) {
        if let Some(day) = slice_day {
            self.state
                .slice_days
                .entry(bucket.to_string())
                .or_default()
                .insert(day);
        }
        self.state
            .files
            .entry(bucket.to_string())
            .or_default()
            .insert(file.to_string(), captured);
    }

    /// True if live slices were captured for `bucket` on `day`.
    #[must_use]
    pub fn had_slices(&self, bucket: &str, day: &str) -> bool {
        self.state
            .slice_days
            .get(bucket)
            .is_some_and(|d| d.contains(day))
    }

    /// Forgets slice files dated before `oldest_day` (they are no longer
    /// listed). End-of-day files and slice-day coverage are kept.
    pub fn prune_slices(&mut self, oldest_day: &str) {
        for files in self.state.files.values_mut() {
            files.retain(|name, f| {
                f.kind != ReportKind::Slice
                    || crate::listing::name_date(name).is_none_or(|d| d.as_str() >= oldest_day)
            });
        }
    }

    /// Writes the state atomically.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub async fn save(&self) -> std::io::Result<()> {
        let body = serde_json::to_vec_pretty(&self.state).map_err(std::io::Error::other)?;
        write_atomic(&self.path, &body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip_and_prune() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/checkpoint.json");
        let mut cp = CheckpointStore::open(&path).unwrap();
        let f = |kind| CapturedFile {
            kind,
            batch: 1,
            sha256: "00".into(),
            rows: 1,
            records: 1,
            emitted: true,
        };
        cp.mark(
            "CFTC_IR",
            "CFTC_SLICE_RATES_2026_11_10_1.zip",
            f(ReportKind::Slice),
            Some("2026-11-10".into()),
        );
        cp.mark(
            "CFTC_IR",
            "CFTC_SLICE_RATES_2026_11_12_1.zip",
            f(ReportKind::Slice),
            Some("2026-11-12".into()),
        );
        cp.mark(
            "CFTC_IR",
            "CFTC_CUMULATIVE_RATES_2026_11_10.zip",
            f(ReportKind::Cumulative),
            None,
        );
        cp.save().await.unwrap();
        let mut cp = CheckpointStore::open(&path).unwrap();
        assert!(cp.has("CFTC_IR", "CFTC_SLICE_RATES_2026_11_10_1.zip"));
        cp.prune_slices("2026-11-11");
        assert!(!cp.has("CFTC_IR", "CFTC_SLICE_RATES_2026_11_10_1.zip"));
        assert!(cp.has("CFTC_IR", "CFTC_SLICE_RATES_2026_11_12_1.zip"));
        assert!(cp.has("CFTC_IR", "CFTC_CUMULATIVE_RATES_2026_11_10.zip"));
        assert!(cp.had_slices("CFTC_IR", "2026-11-10"));
        std::fs::write(&path, b"{not json").unwrap();
        assert!(CheckpointStore::open(&path).is_err());
    }
}
