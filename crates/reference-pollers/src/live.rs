//! Live polling: one task per source, on the source's schedule.
//!
//! Each cycle fetches the reader's live targets in order (plus the pages
//! and documents they point to) with conditional requests, keeps every
//! changed body verbatim ([`crate::raw`]), parses it, drops values dated
//! before the reader's live window, builds records through the
//! [`RevisionStore`] (so unchanged values are skipped and changed ones
//! become revisions), and emits them. Then it flushes the JSON lines, the
//! revision store and the source's state, in that order: delivery is at
//! least once, and a crash between emission and the state write can repeat
//! records (the archive lane dedupes on `(instrument, kind, seq)`).
//!
//! A failed cycle is retried with exponential backoff (one minute,
//! doubling, capped at the schedule's interval); the fetcher already
//! retries transient HTTP failures within a cycle.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mesh_types::{UnixMillis, UnixNanos};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};

use crate::http::{Conditional, Fetcher, Validators};
use crate::metrics::{Health, Metrics};
use crate::obs::{Builder, Stamp};
use crate::output::Emitter;
use crate::raw::{RawStore, sha256_hex, write_atomic_sync};
use crate::revision::RevisionStore;
use crate::sources::{Parsed, Reader};
use crate::time::Date;

/// Durable per-source poll state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollState {
    /// Validators per target key.
    #[serde(default)]
    pub validators: BTreeMap<String, Validators>,
    /// SHA-256 of the last body per target key.
    #[serde(default)]
    pub last_sha: BTreeMap<String, String>,
    /// Fetch-once targets done, with the day they were fetched.
    #[serde(default)]
    pub done_once: BTreeMap<String, String>,
}

impl PollState {
    /// Loads the state, or an empty one.
    ///
    /// # Errors
    ///
    /// Fails on a corrupt file.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Saves the state atomically, dropping fetch-once entries older than
    /// 90 days (listing windows are at most 31 days).
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn save(&mut self, path: &Path, today: Date) -> std::io::Result<()> {
        let cutoff = today.add_days(-90).to_string();
        self.done_once.retain(|_, d| *d >= cutoff);
        let body = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        write_atomic_sync(path, &body)
    }
}

/// What one cycle did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CycleReport {
    /// Bodies fetched that changed.
    pub changed: usize,
    /// Targets that were not modified or unchanged.
    pub unchanged: usize,
    /// Records emitted.
    pub records: usize,
    /// Observations skipped as duplicates.
    pub duplicates: usize,
    /// Targets that failed (fetch or parse).
    pub failures: usize,
}

/// One source's live poller.
#[derive(Debug)]
pub struct SourcePoller {
    reader: Box<dyn Reader>,
    fetcher: Fetcher,
    raw: RawStore,
    state_path: PathBuf,
    state: PollState,
    store: RevisionStore,
    emitter: Arc<Mutex<Emitter>>,
    metrics: Arc<Metrics>,
}

impl SourcePoller {
    /// Opens the poller's state under `state_dir/<source>/`.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or corrupt state.
    pub fn open(
        reader: Box<dyn Reader>,
        fetcher: Fetcher,
        raw: RawStore,
        state_dir: &Path,
        emitter: Arc<Mutex<Emitter>>,
        metrics: Arc<Metrics>,
        today: Date,
    ) -> std::io::Result<Self> {
        let dir = state_dir.join(reader.id().name());
        std::fs::create_dir_all(&dir)?;
        let state_path = dir.join("state.json");
        let state = PollState::load(&state_path)?;
        let store = RevisionStore::open(dir.join("revisions"), reader.revision_days(), today)?;
        Ok(Self {
            reader,
            fetcher,
            raw,
            state_path,
            state,
            store,
            emitter,
            metrics,
        })
    }

    /// The reader.
    #[must_use]
    pub fn reader(&self) -> &dyn Reader {
        self.reader.as_ref()
    }

    fn count_parsed(&self, p: &Parsed) {
        let src = self.reader.id().name();
        for (reason, n) in &p.skipped {
            self.metrics.inc_by(
                "ref_rows_skipped_total",
                &[("source", src), ("reason", reason)],
                *n,
            );
        }
        for field in &p.unknown {
            self.metrics.inc(
                "ref_unknown_fields_total",
                &[("source", src), ("field", field)],
            );
            tracing::warn!(source = src, %field, "undocumented field, column or series; not emitted");
        }
    }

    /// Runs one cycle at `now` (Unix milliseconds).
    ///
    /// # Errors
    ///
    /// Fails on local I/O errors (raw capture, output, state); fetch and
    /// parse failures are counted in the report instead.
    pub async fn cycle(&mut self, now: i64) -> std::io::Result<CycleReport> {
        let id = self.reader.id();
        let src = id.name();
        let today = Date::of_ms(now);
        let window_start = today.add_days(-self.reader.live_window_days());
        let builder = Builder::new(id);
        let mut report = CycleReport::default();
        let mut queue: VecDeque<_> = self.reader.live_targets(today).into();
        let mut newest: Option<Date> = None;
        while let Some(t) = queue.pop_front() {
            if t.once && self.state.done_once.contains_key(&t.key) {
                continue;
            }
            let prev = if t.once {
                Validators::default()
            } else {
                self.state
                    .validators
                    .get(&t.key)
                    .cloned()
                    .unwrap_or_default()
            };
            let (body, validators) = match self.fetcher.get_conditional(&t.url, src, &prev).await {
                Ok(Conditional::NotModified) => {
                    self.metrics.inc(
                        "ref_targets_total",
                        &[("source", src), ("outcome", "not_modified")],
                    );
                    report.unchanged += 1;
                    continue;
                }
                Ok(Conditional::Body { body, validators }) => (body, validators),
                Err(e) if e.is_not_found() => {
                    // A file not published yet (a new year's history file
                    // before its first release): not a failure.
                    tracing::debug!(source = src, url = %t.url, "not found yet");
                    self.metrics.inc(
                        "ref_targets_total",
                        &[("source", src), ("outcome", "not_found")],
                    );
                    report.unchanged += 1;
                    continue;
                }
                Err(e) => {
                    tracing::warn!(source = src, url = %t.url, error = %e, "fetch failed");
                    self.metrics.inc(
                        "ref_targets_total",
                        &[("source", src), ("outcome", "error")],
                    );
                    report.failures += 1;
                    continue;
                }
            };
            let sha = sha256_hex(&body);
            if !t.once && self.state.last_sha.get(&t.key) == Some(&sha) {
                self.state.validators.insert(t.key.clone(), validators);
                self.metrics.inc(
                    "ref_targets_total",
                    &[("source", src), ("outcome", "unchanged")],
                );
                report.unchanged += 1;
                continue;
            }
            let fetched_at = UnixNanos::now();
            self.raw.save(id, &t, &body, &validators, fetched_at)?;
            report.changed += 1;
            self.metrics.inc(
                "ref_targets_total",
                &[("source", src), ("outcome", "changed")],
            );
            match self.reader.parse(&t, &body) {
                Ok(mut parsed) => {
                    self.count_parsed(&parsed);
                    queue.extend(parsed.follow.drain(..));
                    let fetched_ms = fetched_at.to_millis().0;
                    let last_modified_ms = validators
                        .last_modified_ms()
                        .filter(|lm| *lm <= fetched_ms + 300_000);
                    newest = newest.max(parsed.obs.iter().map(crate::obs::Obs::as_of).max());
                    let obs: Vec<_> = parsed
                        .obs
                        .into_iter()
                        .filter(|o| o.as_of() >= window_start)
                        .collect();
                    let (records, dups) = builder
                        .build(
                            obs,
                            Stamp::Live {
                                fetched_at,
                                last_modified_ms,
                            },
                            &mut self.store,
                        )
                        .map_err(std::io::Error::other)?;
                    report.records += records.len();
                    report.duplicates += dups;
                    self.metrics
                        .inc_by("ref_duplicates_total", &[("source", src)], dups as u64);
                    self.emitter.lock().await.emit(id, records).await?;
                }
                Err(e) => {
                    tracing::error!(source = src, url = %t.url, error = %e, "unreadable response; kept in the raw directory");
                    self.metrics
                        .inc("ref_parse_errors_total", &[("source", src)]);
                    report.failures += 1;
                }
            }
            self.state.validators.insert(t.key.clone(), validators);
            self.state.last_sha.insert(t.key.clone(), sha);
            if t.once {
                self.state
                    .done_once
                    .insert(t.key.clone(), today.to_string());
            }
        }
        if let (Some(days), Some(n)) = (self.reader.freshness_days(), newest)
            && n < today.add_days(-days)
        {
            self.metrics
                .inc("ref_stale_reports_total", &[("source", src)]);
            tracing::warn!(source = src, newest = %n, "report file is stale");
        }
        self.emitter.lock().await.flush()?;
        self.store.flush(today)?;
        self.state.save(&self.state_path, today)?;
        Ok(report)
    }

    /// Polls until `shutdown` turns true (or once, with `once`).
    ///
    /// # Errors
    ///
    /// With `once`, fails if the cycle fails or any target failed.
    pub async fn run(
        mut self,
        once: bool,
        health: Arc<Health>,
        mut shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let src = self.reader.id().name();
        let schedule = self.reader.schedule();
        let mut failures = 0u32;
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let now = UnixMillis::now().0;
            let result = self.cycle(now).await;
            let wait = match &result {
                Ok(r) if r.failures == 0 => {
                    failures = 0;
                    health.record_success(src);
                    self.metrics
                        .inc("ref_polls_total", &[("source", src), ("outcome", "ok")]);
                    self.metrics.set(
                        "ref_last_success_timestamp_seconds",
                        &[("source", src)],
                        now as f64 / 1000.0,
                    );
                    tracing::info!(
                        source = src,
                        changed = r.changed,
                        records = r.records,
                        duplicates = r.duplicates,
                        "cycle done"
                    );
                    schedule.interval_at(now)
                }
                Ok(r) => {
                    failures += 1;
                    self.metrics.inc(
                        "ref_polls_total",
                        &[("source", src), ("outcome", "partial")],
                    );
                    tracing::warn!(
                        source = src,
                        failures = r.failures,
                        "cycle had failures; retrying with backoff"
                    );
                    backoff(failures, schedule.interval_at(now))
                }
                Err(e) => {
                    failures += 1;
                    self.metrics
                        .inc("ref_polls_total", &[("source", src), ("outcome", "error")]);
                    tracing::error!(source = src, error = %e, "cycle failed; retrying with backoff");
                    backoff(failures, schedule.interval_at(now))
                }
            };
            if once {
                return match result {
                    Ok(r) if r.failures == 0 => Ok(()),
                    Ok(r) => anyhow::bail!("{src}: {} targets failed", r.failures),
                    Err(e) => Err(e.into()),
                };
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => {}
            }
        }
    }
}

/// Backoff after `failures` consecutive failed cycles: one minute,
/// doubling, at most `cap`.
#[must_use]
pub fn backoff(failures: u32, cap: Duration) -> Duration {
    Duration::from_secs(60)
        .saturating_mul(2u32.saturating_pow(failures.saturating_sub(1).min(16)))
        .min(cap.max(Duration::from_secs(60)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_the_cap() {
        let cap = Duration::from_secs(600);
        assert_eq!(backoff(1, cap), Duration::from_secs(60));
        assert_eq!(backoff(2, cap), Duration::from_secs(120));
        assert_eq!(backoff(10, cap), cap);
    }
}
