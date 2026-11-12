//! The live pipeline: one or two tailers, the active-active merge, the
//! sequencer and the checkpoint, producing [`PipelineEvent`]s on a tokio
//! channel.
//!
//! ```text
//! node A ─▶ Tailer A ─┐                       ┌─▶ BlockOutput (BlockBatches)
//!                     ├─▶ Merger ─▶ Sequencer ─┤
//! node B ─▶ Tailer B ─┘   (alarms)            └─▶ checkpoint file
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::checkpoint::{CHECKPOINT_VERSION, Checkpoint, CheckpointError, ReaderPositions};
use crate::markets::RegistryHandle;
use crate::merge::{IntegrityAlarm, MergeConfig, MergeEvent, Merger, run_merge, run_single};
use crate::metrics::{Metrics, names};
use crate::sequence::{BlockOutput, Sequencer, SequencerState};
use crate::tail::{TailError, Tailer, TailerConfig, lag_seconds};

/// Pipeline settings.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// One reader per node: one (single) or two (active-active).
    pub readers: Vec<TailerConfig>,
    /// Merge settings (two readers).
    pub merge: MergeConfig,
    /// Checkpoint file, if any.
    pub checkpoint: Option<PathBuf>,
    /// Minimum time between checkpoint writes.
    pub checkpoint_interval: Duration,
    /// Capacity of the internal channels (blocks).
    pub channel_capacity: usize,
}

/// What the pipeline produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineEvent {
    /// A published block.
    Block(BlockOutput),
    /// An integrity alarm.
    Alarm(IntegrityAlarm),
}

/// Errors starting the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// Zero or more than two readers.
    #[error("need one or two readers, got {0}")]
    Readers(usize),
    /// The checkpoint cannot be loaded.
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
    /// A tailer thread cannot start.
    #[error(transparent)]
    Tail(#[from] TailError),
}

/// A running pipeline.
#[derive(Debug)]
pub struct Pipeline {
    /// Published blocks and alarms, in order.
    pub events: mpsc::Receiver<PipelineEvent>,
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Pipeline {
    /// Starts the pipeline. Must be called inside a tokio runtime.
    ///
    /// # Errors
    ///
    /// Fails on a bad reader count, an unreadable checkpoint or a thread
    /// that cannot start.
    pub fn spawn(
        cfg: PipelineConfig,
        registry: Arc<RegistryHandle>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, PipelineError> {
        let n = cfg.readers.len();
        if !(1..=2).contains(&n) {
            return Err(PipelineError::Readers(n));
        }
        let cp = match &cfg.checkpoint {
            Some(p) => Checkpoint::load(p)?,
            None => None,
        };
        let (seq_state, resumes) = match cp {
            Some(cp) if cp.readers.len() == n => (cp.sequencer, cp.readers),
            Some(cp) => {
                tracing::warn!(
                    "checkpoint has {} readers, configuration has {n}; reader positions ignored",
                    cp.readers.len()
                );
                (cp.sequencer, vec![None; n])
            }
            None => (SequencerState::default(), vec![None; n]),
        };
        // Copies at or below the last published block are dropped by the
        // merger and the sequencer.
        let resume_after = seq_state.last_block;

        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut tasks = Vec::new();
        let mut reader_rx = Vec::new();
        for (tcfg, resume) in cfg.readers.into_iter().zip(resumes) {
            let (tx, rx) = mpsc::channel(cfg.channel_capacity);
            let tailer = Tailer::new(tcfg, registry.clone(), metrics.clone(), resume);
            threads.push(tailer.spawn(tx, stop.clone())?);
            reader_rx.push(rx);
        }
        let (merge_tx, merge_rx) = mpsc::channel(cfg.channel_capacity);
        let mut rx_iter = reader_rx.into_iter();
        match (rx_iter.next(), rx_iter.next()) {
            (Some(a), Some(b)) => {
                let merger = Merger::new(cfg.merge, metrics.clone(), resume_after, Instant::now());
                tasks.push(tokio::spawn(run_merge(
                    a,
                    b,
                    merge_tx,
                    merger,
                    Duration::from_millis(100),
                )));
            }
            (Some(a), None) => tasks.push(tokio::spawn(run_single(a, merge_tx))),
            _ => return Err(PipelineError::Readers(0)),
        }
        let (out_tx, out_rx) = mpsc::channel(cfg.channel_capacity);
        tasks.push(tokio::spawn(publish(
            merge_rx,
            out_tx,
            Sequencer::new(seq_state),
            Publisher {
                checkpoint: cfg.checkpoint,
                interval: cfg.checkpoint_interval,
                readers: n,
                metrics,
            },
        )));
        Ok(Self {
            events: out_rx,
            stop,
            threads,
            tasks,
        })
    }

    /// Asks the tailers to stop. The event channel then drains and closes,
    /// after a final checkpoint.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// A handle that stops the pipeline when set.
    #[must_use]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Waits for every thread and task to finish (after [`Pipeline::stop`]
    /// and draining [`Pipeline::events`]).
    pub async fn join(self) {
        drop(self.events);
        for t in self.tasks {
            let _ = t.await;
        }
        for t in self.threads {
            let _ = tokio::task::spawn_blocking(move || t.join()).await;
        }
    }
}

struct Publisher {
    checkpoint: Option<PathBuf>,
    interval: Duration,
    readers: usize,
    metrics: Arc<Metrics>,
}

impl Publisher {
    fn save(&self, seq: &Sequencer, positions: &[Option<ReaderPositions>]) {
        let Some(path) = &self.checkpoint else {
            return;
        };
        let cp = Checkpoint {
            version: CHECKPOINT_VERSION,
            readers: positions.iter().take(self.readers).cloned().collect(),
            sequencer: seq.state().clone(),
        };
        match cp.save(path) {
            Ok(()) => self.metrics.inc(names::CHECKPOINTS, &[("result", "ok")]),
            Err(e) => {
                tracing::error!(error = %e, "checkpoint write failed");
                self.metrics.inc(names::CHECKPOINTS, &[("result", "error")]);
            }
        }
    }
}

async fn publish(
    mut rx: mpsc::Receiver<MergeEvent>,
    tx: mpsc::Sender<PipelineEvent>,
    mut seq: Sequencer,
    p: Publisher,
) {
    let mut positions: [Option<ReaderPositions>; 2] = [None, None];
    let mut last_save = Instant::now();
    let mut dirty = false;
    while let Some(ev) = rx.recv().await {
        let out = match ev {
            MergeEvent::Alarm(a) => PipelineEvent::Alarm(a),
            MergeEvent::Block(mb) => {
                for (slot, p) in positions.iter_mut().zip(mb.positions) {
                    if p.is_some() {
                        *slot = p;
                    }
                }
                let Some(out) = seq.sequence(mb.block) else {
                    continue;
                };
                p.metrics
                    .set(names::OUTPUT_LAST_BLOCK, &[], out.block_number as f64);
                p.metrics
                    .set(names::OUTPUT_BLOCK_LAG, &[], lag_seconds(out.block_time_ms));
                if out.gap_before {
                    p.metrics.inc(names::OUTPUT_GAPS, &[]);
                }
                dirty = true;
                PipelineEvent::Block(out)
            }
        };
        if tx.send(out).await.is_err() {
            break;
        }
        if dirty && last_save.elapsed() >= p.interval {
            p.save(&seq, &positions);
            last_save = Instant::now();
            dirty = false;
        }
    }
    if dirty {
        p.save(&seq, &positions);
    }
}
