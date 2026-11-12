//! Live tailing of a node's by-block output (ADR 0006).
//!
//! A [`Tailer`] follows the three stream directories of one node, woken by
//! filesystem notifications (`notify`: inotify on Linux) with a polling
//! fallback, handles hourly rotation, joins the streams per block,
//! normalizes and sends one [`ReaderBlock`] per block on a tokio channel.
//! Each block carries the positions to resume from after it, which the
//! pipeline writes to the checkpoint once the block is published.
//!
//! The tailer runs on its own OS thread: file reads, JSON parsing and the
//! occasional metadata request are blocking work that should not sit on
//! the async runtime.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use mesh_types::{UnixMillis, UnixNanos};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::assumptions::parse_local_time;
use crate::checkpoint::{Position, ReaderPositions};
use crate::files::{StartPosition, StreamCursor};
use crate::filter::StreamDirs;
use crate::join::{Joiner, PushOutcome};
use crate::markets::RegistryHandle;
use crate::metrics::{Metrics, names};
use crate::normalize::{JoinedBlock, ReaderBlock, normalize};
use crate::raw::{ParsedLine, Stream};

/// Tailer settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailerConfig {
    /// Reader label for logs and metrics (`a`, `b`).
    pub name: String,
    /// The node's data directory (`~/hl/data`).
    pub data_dir: PathBuf,
    /// Stream directory names under `data_dir`.
    pub dirs: StreamDirs,
    /// Where to start without a checkpoint.
    pub start: StartPosition,
    /// Use filesystem notifications (falls back to polling if they fail).
    pub use_notify: bool,
    /// Poll interval without notifications.
    pub poll_interval: Duration,
    /// Safety poll interval while notifications work (catches missed
    /// events and rotations).
    pub notify_poll_interval: Duration,
    /// Release a block without a stalled stream once the leading stream is
    /// this many blocks ahead.
    pub max_lag_blocks: u64,
    /// Refresh the registry from metadata this often (`None`: only on
    /// unknown coins).
    pub registry_refresh: Option<Duration>,
}

impl TailerConfig {
    /// Defaults for a node data directory.
    #[must_use]
    pub fn new(name: impl Into<String>, data_dir: PathBuf) -> Self {
        Self {
            name: name.into(),
            data_dir,
            dirs: StreamDirs::default(),
            start: StartPosition::default(),
            use_notify: true,
            poll_interval: Duration::from_millis(50),
            notify_poll_interval: Duration::from_millis(500),
            max_lag_blocks: 600,
            registry_refresh: Some(Duration::from_secs(300)),
        }
    }
}

/// Errors that stop a tailer.
#[derive(Debug, thiserror::Error)]
pub enum TailError {
    /// The thread could not be started.
    #[error("cannot start tailer thread: {0}")]
    Spawn(std::io::Error),
}

/// Follows one node.
#[derive(Debug)]
pub struct Tailer {
    cfg: TailerConfig,
    registry: Arc<RegistryHandle>,
    metrics: Arc<Metrics>,
    cursors: Vec<StreamCursor>,
    joiner: Joiner,
}

impl Tailer {
    /// A tailer resuming at `resume` (from the checkpoint) if given.
    #[must_use]
    pub fn new(
        cfg: TailerConfig,
        registry: Arc<RegistryHandle>,
        metrics: Arc<Metrics>,
        resume: Option<ReaderPositions>,
    ) -> Self {
        let cursors = Stream::ALL
            .iter()
            .map(|&s| {
                StreamCursor::new(
                    cfg.data_dir.join(cfg.dirs.get(s)),
                    cfg.start,
                    resume.as_ref().and_then(|r| r.streams.get(&s).cloned()),
                )
            })
            .collect();
        let joiner = Joiner::new(resume.map(|r| r.last_block), cfg.max_lag_blocks);
        Self {
            cfg,
            registry,
            metrics,
            cursors,
            joiner,
        }
    }

    fn label(&self) -> [(&'static str, &str); 1] {
        [("reader", self.cfg.name.as_str())]
    }

    /// Reads every stream once and queues the lines.
    fn read_all(&mut self) {
        for (i, s) in Stream::ALL.into_iter().enumerate() {
            let (lines, report) = match self.cursors[i].poll() {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(reader = %self.cfg.name, stream = %s, error = %e, "read failed");
                    self.metrics.inc(
                        names::READER_PARSE_ERRORS,
                        &[("reader", &self.cfg.name), ("stream", s.as_str())],
                    );
                    continue;
                }
            };
            if report.rotations > 0 {
                self.metrics.add(
                    names::READER_ROTATIONS,
                    &self.label(),
                    f64::from(report.rotations),
                );
            }
            if report.truncated_bytes > 0 {
                tracing::warn!(reader = %self.cfg.name, stream = %s, bytes = report.truncated_bytes,
                    "unterminated line cut off by hourly rotation");
                self.metrics.inc(names::READER_TRUNCATED, &self.label());
            }
            for line in lines {
                match ParsedLine::parse(s, &line.text) {
                    Ok(p) => {
                        let block = p.block_number();
                        if self.joiner.push(p, Some(line.pos.clone())) == PushOutcome::OutOfOrder {
                            tracing::warn!(reader = %self.cfg.name, stream = %s, block,
                                file = %line.pos.file, "line out of order; dropped");
                            self.metrics.inc(names::READER_OUT_OF_ORDER, &self.label());
                        }
                    }
                    Err(e) => {
                        tracing::error!(reader = %self.cfg.name, stream = %s, file = %line.pos.file,
                            offset = line.pos.offset, error = %e, "cannot parse line (schema drift?)");
                        self.metrics.inc(
                            names::READER_PARSE_ERRORS,
                            &[("reader", &self.cfg.name), ("stream", s.as_str())],
                        );
                    }
                }
            }
        }
    }

    fn cursor_positions(&self) -> [Option<Position>; 3] {
        [
            self.cursors[0].position(),
            self.cursors[1].position(),
            self.cursors[2].position(),
        ]
    }

    fn process(&mut self, joined: &JoinedBlock) -> ReaderBlock {
        self.registry.ensure(joined.coins());
        let mut block = {
            let reg = self.registry.read();
            normalize(joined, &reg)
        };
        block.resume = self.joiner.resume_positions(&self.cursor_positions());
        self.observe(joined, &block);
        block
    }

    fn observe(&self, joined: &JoinedBlock, block: &ReaderBlock) {
        let l = self.label();
        let m = &self.metrics;
        m.inc(names::READER_BLOCKS, &l);
        m.add(names::READER_RECORDS, &l, block.record_count() as f64);
        m.set(names::READER_LAST_BLOCK, &l, block.block_number as f64);
        let now = UnixNanos::now();
        let lag_ms = now.to_millis().0.saturating_sub(block.block_time_ms.0);
        m.set(names::READER_BLOCK_LAG, &l, lag_ms as f64 / 1e3);
        let local = joined
            .fills
            .as_ref()
            .map(|f| f.local_time.as_str())
            .and_then(|t| parse_local_time(t).ok());
        if let Some(local) = local {
            m.set(
                names::READER_EMIT_LAG,
                &l,
                now.0.saturating_sub(local.0) as f64 / 1e9,
            );
        }
        if block.incomplete {
            m.inc(names::READER_INCOMPLETE, &l);
        }
        if let Some(reason) = &block.rejected {
            tracing::error!(reader = %self.cfg.name, block = block.block_number, %reason, "block rejected");
            m.inc(names::READER_REJECTED, &l);
        }
        for (r, n) in &block.stats.dropped_pairs {
            m.add(
                names::READER_DROPPED_PAIRS,
                &[("reader", &self.cfg.name), ("reason", r.as_str())],
                *n as f64,
            );
        }
        for (a, n) in &block.stats.anomalies {
            tracing::warn!(reader = %self.cfg.name, block = block.block_number, anomaly = a.as_str(),
                "malformed fill group");
            m.add(
                names::READER_ANOMALIES,
                &[("reader", &self.cfg.name), ("kind", a.as_str())],
                *n as f64,
            );
        }
        if !block.stats.unknown_coins.is_empty() {
            tracing::warn!(reader = %self.cfg.name, block = block.block_number,
                coins = ?block.stats.unknown_coins, "events for unregistered coins dropped");
            m.inc(names::READER_UNKNOWN_COINS, &l);
        }
        if block.stats.unadmitted_events > 0 {
            m.add(
                names::READER_UNADMITTED,
                &l,
                block.stats.unadmitted_events as f64,
            );
        }
    }

    fn watch(&self, watcher: &mut Option<RecommendedWatcher>, watched: &mut [bool; 3]) {
        let Some(w) = watcher.as_mut() else {
            return;
        };
        for (i, c) in self.cursors.iter().enumerate() {
            if watched[i] || !c.dir().is_dir() {
                continue;
            }
            match w.watch(c.dir(), RecursiveMode::Recursive) {
                Ok(()) => watched[i] = true,
                Err(e) => {
                    tracing::warn!(reader = %self.cfg.name, dir = %c.dir().display(), error = %e,
                        "cannot watch directory; polling it");
                }
            }
        }
    }

    /// Runs until `stop` is set or the receiver is dropped.
    pub fn run(mut self, tx: mpsc::Sender<ReaderBlock>, stop: Arc<AtomicBool>) {
        let (wake_tx, wake_rx) = std_mpsc::channel::<()>();
        let mut watcher: Option<RecommendedWatcher> = if self.cfg.use_notify {
            match notify::recommended_watcher(move |_res: notify::Result<notify::Event>| {
                let _ = wake_tx.send(());
            }) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(reader = %self.cfg.name, error = %e,
                        "filesystem notifications unavailable; polling");
                    None
                }
            }
        } else {
            None
        };
        let mut watched = [false; 3];
        let mut last_refresh = std::time::Instant::now();
        tracing::info!(reader = %self.cfg.name, dir = %self.cfg.data_dir.display(),
            notify = watcher.is_some(), "tailer started");
        while !stop.load(Ordering::Relaxed) {
            self.watch(&mut watcher, &mut watched);
            self.read_all();
            while let Some(joined) = self.joiner.pop_ready() {
                let block = self.process(&joined);
                if tx.blocking_send(block).is_err() {
                    return;
                }
            }
            if let Some(every) = self.cfg.registry_refresh
                && last_refresh.elapsed() >= every
            {
                last_refresh = std::time::Instant::now();
                self.registry.refresh(false);
            }
            let wait = if watcher.is_some() && watched.iter().all(|w| *w) {
                self.cfg.notify_poll_interval
            } else {
                self.cfg.poll_interval
            };
            if wake_rx.recv_timeout(wait).is_ok() {
                while wake_rx.try_recv().is_ok() {}
            }
        }
        tracing::info!(reader = %self.cfg.name, last_block = ?self.joiner.released(), "tailer stopped");
    }

    /// Starts the tailer on its own thread.
    ///
    /// # Errors
    ///
    /// Fails if the thread cannot be spawned.
    pub fn spawn(
        self,
        tx: mpsc::Sender<ReaderBlock>,
        stop: Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>, TailError> {
        let name = format!("hl-tail-{}", self.cfg.name);
        std::thread::Builder::new()
            .name(name)
            .spawn(move || self.run(tx, stop))
            .map_err(TailError::Spawn)
    }
}

/// Block time lag helper for callers: seconds from `t` to now.
#[must_use]
pub fn lag_seconds(t: UnixMillis) -> f64 {
    UnixMillis::now().0.saturating_sub(t.0) as f64 / 1e3
}
