//! Live capture: follows node stream directories while the node writes
//! them, seals each hour once the node has moved past it, uploads sealed
//! hours write-once and maintains the capture watermark.
//!
//! ```text
//! node <stream>/hourly/<YYYYMMDD>/<H> ──▶ Follower ──▶ Capture (private + filtered tiers)
//!                                             │              │ sealed hours
//!                                             │              ▼
//!                                             │        upload_ready (write-once)
//!                                             ▼
//!                               <staging>/.captured-through  (pruner watermark)
//! ```
//!
//! - Only complete lines are captured; a partial last line of the newest
//!   file waits for its newline. The partial last line of an older file
//!   (the node crashed mid-line) is kept verbatim in the private tier.
//! - An hour is sealed when every stream's newest node file is a later
//!   hour, or, for streams that stopped, `seal_grace` after the hour ended
//!   by the wall clock. Node files rotate by hour, so a sealed hour never
//!   grows again.
//! - The watermark `<staging>/.captured-through` holds the last UTC hour
//!   (`YYYY-MM-DDTHH`) that is sealed for every stream and whose files are
//!   all uploaded (`ready/` empty). The node pruner deletes node output only
//!   up to it.
//! - Progress survives restarts: open hours are recovered by the tier
//!   writers (and that many lines of the node file are skipped), and
//!   `<staging>/capture-state.json` records the last sealed hour per
//!   stream so earlier node files are not read again.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::capture::{Capture, RawLine, SealedFile, node_hour_files};
use crate::error::{Error, IoContext, Result};
use crate::layout::Day;
use crate::store::Store;

/// File name of the watermark under the staging directory.
pub const WATERMARK_FILE: &str = ".captured-through";
/// File name of the follower state under the staging directory.
pub const STATE_FILE: &str = "capture-state.json";

/// Live capture settings.
#[derive(Debug, Clone)]
pub struct FollowConfig {
    /// Stream slug and node stream directory
    /// (`fills` → `/hl/data/node_fills_by_block`).
    pub streams: Vec<(String, PathBuf)>,
    /// Staging directory (the tiers' `staging`).
    pub staging: PathBuf,
    /// Time between polls.
    pub poll_interval: Duration,
    /// Seal an hour this long after it ended by the wall clock even if a
    /// stream wrote no later file (a stopped node).
    pub seal_grace: Duration,
    /// Rewrite the archive metrics textfile here at most this often.
    pub metrics_out: Option<(PathBuf, Duration)>,
}

impl FollowConfig {
    /// Defaults for `streams` and `staging`: poll every second, seal stopped
    /// streams 10 minutes after the hour, no metrics file.
    #[must_use]
    pub fn new(streams: Vec<(String, PathBuf)>, staging: PathBuf) -> Self {
        Self {
            streams,
            staging,
            poll_interval: Duration::from_secs(1),
            seal_grace: Duration::from_secs(600),
            metrics_out: None,
        }
    }
}

/// Where sealed hours go.
#[derive(Debug, Clone, Default)]
pub struct UploadTargets {
    /// `(archive name, store)` per tier.
    pub stores: Vec<(String, Store)>,
    /// Keep uploaded files under `uploaded/`.
    pub keep_local: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    /// Last sealed hour per stream, as `YYYY-MM-DDTHH`.
    sealed: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct Cursor {
    day: Day,
    hour: u8,
    path: PathBuf,
    offset: u64,
    /// Complete lines still to skip (resuming an open hour).
    skip: u64,
}

/// An hour as `(day, hour)`.
pub type Hour = (Day, u8);

/// Formats an hour as `YYYY-MM-DDTHH`.
#[must_use]
pub fn hour_text(h: Hour) -> String {
    format!("{}T{:02}", h.0, h.1)
}

fn parse_hour(s: &str) -> Option<Hour> {
    let (d, h) = s.split_once('T')?;
    let hour: u8 = h.parse().ok()?;
    (hour <= 23).then_some(())?;
    Some((d.parse().ok()?, hour))
}

fn prev_hour(h: Hour) -> Hour {
    if h.1 == 0 {
        (Day::from_days(h.0.days() - 1), 23)
    } else {
        (h.0, h.1 - 1)
    }
}

fn next_hour(h: Hour) -> Hour {
    if h.1 == 23 {
        (Day::from_days(h.0.days() + 1), 0)
    } else {
        (h.0, h.1 + 1)
    }
}

fn hour_of_millis(ms: i64) -> Hour {
    let day = Day::from_millis(ms);
    let into = ms.rem_euclid(86_400_000);
    // At most 23 by construction.
    let hour = u8::try_from(into / 3_600_000).unwrap_or(23);
    (day, hour)
}

fn hour_start_ms(h: Hour) -> i64 {
    i64::from(h.0.days()) * 86_400_000 + i64::from(h.1) * 3_600_000
}

/// Follows node stream directories into a [`Capture`].
#[derive(Debug)]
pub struct Follower {
    cfg: FollowConfig,
    capture: Capture,
    cursors: BTreeMap<String, Cursor>,
    state: State,
    /// Every hour strictly before this is sealed for every stream.
    sealed_before: Option<Hour>,
}

impl Follower {
    /// A follower over `capture`, resuming from the tiers' open hours and
    /// the state file.
    ///
    /// # Errors
    ///
    /// Fails if the state file exists but cannot be read.
    pub fn new(cfg: FollowConfig, capture: Capture) -> Result<Self> {
        let path = cfg.staging.join(STATE_FILE);
        let state = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path).at(&path)?)?
        } else {
            State::default()
        };
        Ok(Self {
            cfg,
            capture,
            cursors: BTreeMap::new(),
            state,
            sealed_before: None,
        })
    }

    /// The capture.
    #[must_use]
    pub fn capture(&self) -> &Capture {
        &self.capture
    }

    fn last_sealed(&self, stream: &str) -> Option<Hour> {
        self.state.sealed.get(stream).and_then(|s| parse_hour(s))
    }

    /// Where `stream` resumes: its open hour (skipping the lines already
    /// written), or the first node file after its last sealed hour.
    fn start_cursor(&self, stream: &str, files: &[(Day, u8, PathBuf)]) -> Option<Cursor> {
        let open = [self.capture.private(), self.capture.filtered()]
            .into_iter()
            .flatten()
            .filter_map(|w| w.position(stream))
            .max_by_key(|(d, h, n)| (*d, *h, *n));
        if let Some((day, hour, lines)) = open
            && let Some((_, _, path)) = files.iter().find(|(d, h, _)| (*d, *h) == (day, hour))
        {
            return Some(Cursor {
                day,
                hour,
                path: path.clone(),
                offset: 0,
                skip: lines,
            });
        }
        let floor = self.last_sealed(stream);
        files
            .iter()
            .find(|(d, h, _)| floor.is_none_or(|f| (*d, *h) > f))
            .map(|(day, hour, path)| Cursor {
                day: *day,
                hour: *hour,
                path: path.clone(),
                offset: 0,
                skip: 0,
            })
    }

    /// Reads new lines of one node file from the cursor. Returns sealed
    /// files and whether the file was read to its end including a partial
    /// last line (`complete`).
    fn read_file(&mut self, stream: &str, complete: bool) -> Result<Vec<SealedFile>> {
        let Some(c) = self.cursors.get_mut(stream) else {
            return Ok(Vec::new());
        };
        let mut f = File::open(&c.path).at(&c.path)?;
        f.seek(SeekFrom::Start(c.offset)).at(&c.path)?;
        let mut r = BufReader::new(f);
        let mut buf = Vec::new();
        let (day, hour, path) = (c.day, c.hour, c.path.clone());
        let mut lines: Vec<Vec<u8>> = Vec::new();
        loop {
            buf.clear();
            let n = r.read_until(b'\n', &mut buf).at(&path)?;
            if n == 0 {
                break;
            }
            if buf.last() != Some(&b'\n') && !complete {
                break;
            }
            c.offset += n as u64;
            if c.skip > 0 {
                c.skip -= 1;
                continue;
            }
            lines.push(std::mem::take(&mut buf));
        }
        let mut sealed = Vec::new();
        for line in &lines {
            sealed.extend(self.capture.push(&RawLine {
                stream,
                day,
                hour,
                line,
            })?);
        }
        Ok(sealed)
    }

    /// One pass: reads new lines of every stream, then seals the hours
    /// every stream has moved past. `now_ms` is the wall clock.
    ///
    /// # Errors
    ///
    /// Fails on I/O or capture errors.
    pub fn poll(&mut self, now_ms: i64) -> Result<Vec<SealedFile>> {
        let mut sealed = Vec::new();
        let mut newest: Vec<Option<Hour>> = Vec::new();
        let streams = self.cfg.streams.clone();
        for (stream, dir) in &streams {
            let files = if dir.exists() {
                node_hour_files(dir)?
            } else {
                Vec::new()
            };
            newest.push(files.last().map(|(d, h, _)| (*d, *h)));
            if !self.cursors.contains_key(stream) {
                match self.start_cursor(stream, &files) {
                    Some(c) => {
                        self.cursors.insert(stream.clone(), c);
                    }
                    None => continue,
                }
            }
            while let Some(c) = self.cursors.get(stream) {
                let here = (c.day, c.hour);
                let next = files.iter().find(|(d, h, _)| (*d, *h) > here).cloned();
                // A file with a later one is finished: read it to the end,
                // partial last line included.
                let finished = next.is_some();
                sealed.extend(self.read_file(stream, finished)?);
                match next {
                    Some((day, hour, path)) => {
                        self.cursors.insert(
                            stream.clone(),
                            Cursor {
                                day,
                                hour,
                                path,
                                offset: 0,
                                skip: 0,
                            },
                        );
                    }
                    None => break,
                }
            }
        }
        self.capture.flush()?;
        // Every stream has a later file: hours before the oldest newest
        // file are complete everywhere.
        let by_files = if newest.iter().all(Option::is_some) {
            newest.iter().flatten().min().copied()
        } else {
            None
        };
        // Wall clock: hours that ended more than `seal_grace` ago.
        let grace = i64::try_from(self.cfg.seal_grace.as_millis()).unwrap_or(i64::MAX);
        let by_clock = hour_of_millis(now_ms.saturating_sub(grace));
        let before = by_files.map_or(by_clock, |f| f.max(by_clock));
        if self.sealed_before.is_none_or(|s| before > s) {
            sealed.extend(self.capture.seal_before(before.0, before.1)?);
            self.sealed_before = Some(before);
            let last = prev_hour(before);
            for (stream, _) in &streams {
                if self.last_sealed(stream).is_none_or(|s| s < last) {
                    self.state.sealed.insert(stream.clone(), hour_text(last));
                }
            }
            self.save_state()?;
        }
        Ok(sealed)
    }

    fn save_state(&self) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(&self.state)?;
        bytes.push(b'\n');
        std::fs::create_dir_all(&self.cfg.staging).at(&self.cfg.staging)?;
        crate::partition::write_atomic(&self.cfg.staging.join(STATE_FILE), &bytes)
    }

    /// The last hour sealed for every stream, if any.
    #[must_use]
    pub fn sealed_through(&self) -> Option<Hour> {
        self.sealed_before.map(prev_hour)
    }

    /// Seals every open hour (shutdown of a batch run; never in live use,
    /// where the newest hour is still being written).
    ///
    /// # Errors
    ///
    /// Fails on I/O or compression errors.
    pub fn seal_all(&mut self) -> Result<Vec<SealedFile>> {
        self.capture.seal_all()
    }
}

/// True if `<staging>/<archive>/ready/` holds no sealed file.
fn ready_empty(staging: &Path, archive: &str) -> Result<bool> {
    let ready = staging.join(archive).join("ready");
    Ok(crate::layout::list_files(&ready)?
        .iter()
        .all(|f| f.ends_with(".tmp")))
}

/// Writes the watermark atomically.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn write_watermark(staging: &Path, h: Hour) -> Result<()> {
    let text = format!("{}\n", hour_text(h));
    crate::partition::write_atomic(&staging.join(WATERMARK_FILE), text.as_bytes())
}

/// Reads the watermark, if present and well formed.
#[must_use]
pub fn read_watermark(staging: &Path) -> Option<Hour> {
    let text = std::fs::read_to_string(staging.join(WATERMARK_FILE)).ok()?;
    parse_hour(text.lines().next()?.trim())
}

/// Uploads sealed hours of every tier; returns true if nothing is left to
/// upload and nothing conflicted.
async fn upload(staging: &Path, targets: &UploadTargets) -> Result<bool> {
    let mut clean = true;
    for (archive, store) in &targets.stores {
        let r = crate::store::upload_ready(staging, archive, store, targets.keep_local).await?;
        if !r.created.is_empty() || !r.identical.is_empty() {
            tracing::info!(
                archive,
                created = r.created.len(),
                identical = r.identical.len(),
                "uploaded sealed hours"
            );
        }
        if !r.conflicts.is_empty() {
            clean = false;
        }
        if !ready_empty(staging, archive)? {
            clean = false;
        }
    }
    Ok(clean)
}

/// Runs a follower until `stop` turns true: poll, upload, update the
/// watermark (only for hours sealed and uploaded in every tier), rewrite
/// the metrics file. Without upload targets the watermark follows sealing
/// alone (local staging is the archive). On stop, the newest hour stays
/// open so a restart continues it.
///
/// # Errors
///
/// Fails on capture errors (upload errors are logged and retried).
pub async fn run(
    follower: Follower,
    targets: Option<UploadTargets>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let staging = follower.cfg.staging.clone();
    let poll = follower.cfg.poll_interval;
    let metrics_out = follower.cfg.metrics_out.clone();
    let mut last_metrics: Option<std::time::Instant> = None;
    let mut f = Some(follower);
    loop {
        let mut cur = f.take().ok_or_else(|| Error::invalid("follower lost"))?;
        let now = mesh_types::UnixMillis::now().0;
        let (back, result) = tokio::task::spawn_blocking(move || {
            let r = cur.poll(now);
            (cur, r)
        })
        .await
        .map_err(|e| Error::invalid(format!("capture task failed: {e}")))?;
        result?;
        let through = back.sealed_through();
        f = Some(back);
        let uploaded = match &targets {
            Some(t) => match upload(&staging, t).await {
                Ok(clean) => clean,
                Err(e) => {
                    tracing::warn!(error = %e, "upload failed; retrying next pass");
                    crate::metrics::inc("archive_capture_upload_errors_total", &[], 1);
                    false
                }
            },
            None => true,
        };
        if uploaded
            && let Some(h) = through
            && read_watermark(&staging).is_none_or(|w| w < h)
        {
            write_watermark(&staging, h)?;
            tracing::info!(through = %hour_text(h), "capture watermark advanced");
        }
        if let Some((path, every)) = &metrics_out
            && last_metrics.is_none_or(|t| t.elapsed() >= *every)
        {
            if let Err(e) = crate::metrics::global().write_textfile(path) {
                tracing::warn!(error = %e, "could not write capture metrics");
            }
            last_metrics = Some(std::time::Instant::now());
        }
        tokio::select! {
            () = tokio::time::sleep(poll) => {}
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() {
                    break;
                }
            }
        }
    }
    if let Some((path, _)) = &metrics_out {
        let _ = crate::metrics::global().write_textfile(path);
    }
    Ok(())
}

/// The hour after `h` (exposed for tests and callers computing ranges).
#[must_use]
pub fn following_hour(h: Hour) -> Hour {
    next_hour(h)
}

/// The start of hour `h` in Unix milliseconds.
#[must_use]
pub fn hour_start(h: Hour) -> i64 {
    hour_start_ms(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{FilterSet, HourWriter, TierConfig};
    use std::io::Write;
    use std::sync::Arc;

    fn upper(line: &[u8]) -> Option<Vec<u8>> {
        Some(line.to_ascii_uppercase())
    }

    fn capture(staging: &Path) -> Capture {
        let filters = FilterSet::new()
            .with("fills", Arc::new(upper))
            .unwrap()
            .with("diffs", Arc::new(upper))
            .unwrap();
        Capture::new(
            Some(HourWriter::open(TierConfig::private(staging)).unwrap()),
            Some(HourWriter::open(TierConfig::filtered(staging)).unwrap()),
            filters,
        )
        .unwrap()
    }

    fn append(dir: &Path, stream: &str, day: &str, hour: u8, text: &str) {
        let p = dir
            .join(stream)
            .join("hourly")
            .join(day)
            .join(hour.to_string());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn cfg(node: &Path, staging: &Path) -> FollowConfig {
        FollowConfig::new(
            vec![
                ("fills".into(), node.join("fills")),
                ("diffs".into(), node.join("diffs")),
            ],
            staging.to_path_buf(),
        )
    }

    const T0: i64 = 1_767_225_600_000; // 2026-01-01T00:00Z

    #[test]
    fn follows_seals_and_resumes() {
        let node = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        append(node.path(), "fills", "20260101", 0, "a\nb\n");
        append(node.path(), "diffs", "20260101", 0, "x\npart");
        let mut f =
            Follower::new(cfg(node.path(), staging.path()), capture(staging.path())).unwrap();
        assert!(f.poll(T0 + 60_000).unwrap().is_empty());
        assert_eq!(
            f.capture().private().unwrap().position("diffs"),
            Some(("2026-01-01".parse().unwrap(), 0, 1))
        );
        // The partial line completes; a restart resumes without duplicates.
        append(node.path(), "diffs", "20260101", 0, "ial\n");
        drop(f);
        let mut f =
            Follower::new(cfg(node.path(), staging.path()), capture(staging.path())).unwrap();
        assert!(f.poll(T0 + 120_000).unwrap().is_empty());
        assert_eq!(
            f.capture().private().unwrap().position("diffs"),
            Some(("2026-01-01".parse().unwrap(), 0, 2))
        );
        // Only fills moves to hour 1: nothing is sealed for every stream
        // yet, but fills' hour 0 seals on its own when hour 1 starts.
        append(node.path(), "fills", "20260101", 1, "c\n");
        let sealed = f.poll(T0 + 3_660_000).unwrap();
        assert_eq!(sealed.len(), 2);
        assert!(
            sealed
                .iter()
                .all(|s| s.path == "stream=fills/date=2026-01-01/00.jsonl.zst")
        );
        // Diffs moves too: hour 0 is sealed everywhere.
        append(node.path(), "diffs", "20260101", 1, "y\n");
        let sealed = f.poll(T0 + 3_700_000).unwrap();
        assert_eq!(sealed.len(), 2);
        let through: Hour = ("2026-01-01".parse().unwrap(), 0);
        assert_eq!(f.sealed_through(), Some(through));
        let raw =
            crate::capture::decompress(&std::fs::read(&sealed[0].local).unwrap(), &BTreeMap::new())
                .unwrap();
        assert_eq!(raw, b"x\npartial\n");
        // A restart does not read sealed hours again.
        drop(f);
        let mut f =
            Follower::new(cfg(node.path(), staging.path()), capture(staging.path())).unwrap();
        assert!(f.poll(T0 + 3_800_000).unwrap().is_empty());
        assert_eq!(
            f.capture().filtered().unwrap().position("fills"),
            Some(("2026-01-01".parse().unwrap(), 1, 1))
        );
    }

    #[test]
    fn stopped_streams_seal_after_grace() {
        let node = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        append(node.path(), "fills", "20260101", 0, "a\n");
        append(node.path(), "diffs", "20260101", 0, "x\n");
        let mut c = cfg(node.path(), staging.path());
        c.seal_grace = Duration::from_secs(600);
        let mut f = Follower::new(c, capture(staging.path())).unwrap();
        assert!(f.poll(T0 + 3_600_000 + 599_000).unwrap().is_empty());
        let sealed = f.poll(T0 + 3_600_000 + 601_000).unwrap();
        assert_eq!(sealed.len(), 4);
        assert_eq!(f.sealed_through(), Some(("2026-01-01".parse().unwrap(), 0)));
    }

    #[tokio::test]
    async fn run_uploads_and_writes_the_watermark() {
        let node = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        append(node.path(), "fills", "20260101", 0, "a\n");
        append(node.path(), "diffs", "20260101", 0, "x\n");
        append(node.path(), "fills", "20260101", 1, "b\n");
        append(node.path(), "diffs", "20260101", 1, "y\n");
        let mut c = cfg(node.path(), staging.path());
        c.poll_interval = Duration::from_millis(20);
        let metrics = staging.path().join("capture.prom");
        c.metrics_out = Some((metrics.clone(), Duration::from_millis(1)));
        let f = Follower::new(c, capture(staging.path())).unwrap();
        let store = Store::memory();
        let targets = UploadTargets {
            stores: vec![
                (crate::layout::PRIVATE_RAW_ARCHIVE.to_owned(), store.clone()),
                (crate::layout::RAW_ARCHIVE.to_owned(), store.clone()),
            ],
            keep_local: false,
        };
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run(f, Some(targets), rx));
        for _ in 0..200 {
            if read_watermark(staging.path()).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.send(true).unwrap();
        task.await.unwrap().unwrap();
        assert!(
            read_watermark(staging.path()).is_some_and(|h| h >= ("2026-01-01".parse().unwrap(), 0))
        );
        assert!(
            store
                .get("hyperliquid-raw/stream=fills/date=2026-01-01/00.jsonl.zst")
                .await
                .unwrap()
                .is_some()
        );
        assert!(metrics.exists());
    }

    #[test]
    fn hour_helpers() {
        let h: Hour = ("2026-01-01".parse().unwrap(), 0);
        assert_eq!(hour_text(h), "2026-01-01T00");
        assert_eq!(parse_hour("2026-01-01T00"), Some(h));
        assert_eq!(hour_text(prev_hour(h)), "2025-12-31T23");
        assert_eq!(following_hour(prev_hour(h)), h);
        assert_eq!(hour_start(h), T0);
        assert_eq!(hour_of_millis(T0 + 3_599_999), h);
        assert!(parse_hour("2026-01-01T24").is_none());
    }
}
