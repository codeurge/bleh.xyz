//! The streaming processor: one instrument, block by block.
//!
//! A [`Processor`] consumes an instrument's trades and book events block by
//! block, in block order, plus book snapshots, and emits derived records
//! ([`Output`]) as soon as they are complete, so the gateway can publish
//! them live. The archive replay uses the same code through
//! [`crate::batch::run`]; outputs are identical for the same input (see the
//! parity tests).
//!
//! # Input
//!
//! - [`Processor::push_batch`] takes [`BlockBatch`]es as published on the
//!   bus (trades and book events of one block, either kind first). A block
//!   is processed when a batch of a later block arrives, or at
//!   [`Processor::flush`], [`Processor::advance_to`],
//!   [`Processor::process_snapshot`] or [`Processor::finish`].
//! - [`Processor::process_block`] takes one whole block directly.
//! - [`Processor::process_snapshot`] takes a [`BookSnapshot`] after the
//!   block it describes: it initialises the book, or reconciles the book
//!   with it and resyncs on a difference.
//! - [`Processor::advance_to`] is a clock: it closes intervals that ended
//!   before a time, so quiet markets still publish their last bar.
//!
//! # Book state
//!
//! The book starts [`BookState::Uninitialized`] (or empty with
//! [`Processor::with_empty_book`]); trade analytics run regardless. A book
//! inconsistency, a gap in the book-event stream or an event flagged
//! `gap_before` puts it in [`BookState::Desynced`]: an [`Output::Desync`]
//! is emitted, book analytics pause (the open heatmap tile and order
//! activity interval are dropped) and events are skipped until a snapshot
//! resyncs it.
//!
//! # Order within a block
//!
//! Heatmap columns and liquidity samples before the block are closed with
//! the book as it was; then trades go to the trade analytics; then events
//! are applied to the book; then checkpoints, queue positions and the
//! block's end are recorded.

use mesh_types::{
    Bar, BarInterval, BlockBatch, BookEvent, BookSnapshot, Flag, Footprint, InstrumentId, Record,
    RecordKind, Seq, SnapshotDepth, Trade, UnixMillis, VolumeProfile,
};
use serde::{Deserialize, Serialize};

use crate::bars::{ActivityBar, ActivityBarBuilder, ActivitySpec, BarEngine, BarEngineConfig};
use crate::book::OrderBook;
use crate::checkpoint::{CheckpointPolicy, Checkpointer};
use crate::error::{BookError, InputError, ProcessError};
use crate::inputs::{EventIn, TradeIn, canonical_event, canonical_snapshot, canonical_trade};
use crate::liquidity::{LiquidityConfig, LiquidityGrade, LiquidityTracker};
use crate::orderflow::{
    DeltaBar, DeltaBuilder, FootprintBuilder, HeatmapBuilder, HeatmapConfig, HeatmapTile,
    OrderActivity, OrderActivityBuilder, OrderActivityConfig, ProfileBuilder, ProfileConfig,
    ProfileSummary, QueueConfig, QueuePositions, QueueTracker, Sweep, SweepConfig, SweepDetector,
};
use crate::reconcile::{ReconcileOptions, ReconcileReport, reconcile};

/// What a [`Processor`] builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessorConfig {
    /// Book checkpoint interval.
    pub checkpoint: CheckpointPolicy,
    /// What reconciliation compares.
    pub reconcile: ReconcileOptions,
    /// Time bars and rollups.
    pub bars: BarEngineConfig,
    /// Tick, volume and range bars (thresholds are per instrument; none by
    /// default).
    pub activity_bars: Vec<ActivitySpec>,
    /// Reset period of activity bars.
    pub activity_reset: BarInterval,
    /// Footprint intervals.
    pub footprints: Vec<BarInterval>,
    /// Delta intervals.
    pub deltas: Vec<BarInterval>,
    /// CVD anchor period.
    pub cvd_anchor: BarInterval,
    /// Volume profiles.
    pub profile: Option<ProfileConfig>,
    /// Order activity.
    pub order_activity: Option<OrderActivityConfig>,
    /// Queue positions.
    pub queue: Option<QueueConfig>,
    /// Sweep detection.
    pub sweeps: Option<SweepConfig>,
    /// Heatmap tiles.
    pub heatmap: Option<HeatmapConfig>,
    /// Liquidity grade.
    pub liquidity: Option<LiquidityConfig>,
}

impl Default for ProcessorConfig {
    /// Everything on with its defaults; no activity bars.
    fn default() -> Self {
        Self {
            checkpoint: CheckpointPolicy::default(),
            reconcile: ReconcileOptions::default(),
            bars: BarEngineConfig::default(),
            activity_bars: Vec::new(),
            activity_reset: BarInterval::D1,
            footprints: vec![BarInterval::M1],
            deltas: vec![BarInterval::M1],
            cvd_anchor: BarInterval::D1,
            profile: Some(ProfileConfig::default()),
            order_activity: Some(OrderActivityConfig::default()),
            queue: Some(QueueConfig::default()),
            sweeps: Some(SweepConfig::default()),
            heatmap: Some(HeatmapConfig::default()),
            liquidity: Some(LiquidityConfig::default()),
        }
    }
}

/// Book state of a processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookState {
    /// No snapshot yet.
    Uninitialized,
    /// The book follows the events.
    InSync,
    /// The book is known to be wrong; waiting for a snapshot.
    Desynced,
}

/// Why the book desynchronised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesyncReason {
    /// An event or the block end was inconsistent with the book.
    Book {
        /// The error, as text.
        error: String,
    },
    /// The book-event stream skipped blocks (batch link broken or an event
    /// flagged `gap_before`).
    Gap,
}

/// The book desynchronised at `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Desync {
    /// Position of the offending event or block.
    pub seq: Seq,
    /// Why.
    pub reason: DesyncReason,
}

/// A record or notice emitted by the processor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stream", content = "record", rename_all = "snake_case")]
pub enum Output {
    /// A time bar (direct or rolled up).
    Bar(Bar),
    /// A tick, volume or range bar.
    ActivityBar(ActivityBar),
    /// A footprint.
    Footprint(Footprint),
    /// Delta and CVD.
    Delta(DeltaBar),
    /// A volume profile.
    VolumeProfile(VolumeProfile),
    /// POC and value area of a volume profile.
    ProfileSummary(ProfileSummary),
    /// An L4 book checkpoint.
    Checkpoint(BookSnapshot),
    /// Order activity.
    OrderActivity(OrderActivity),
    /// Top-of-book queue positions.
    Queue(QueuePositions),
    /// A sweep.
    Sweep(Sweep),
    /// A heatmap tile.
    Heatmap(HeatmapTile),
    /// A liquidity grade.
    Liquidity(LiquidityGrade),
    /// The result of comparing the book with a snapshot.
    Reconciliation(ReconcileReport),
    /// The book desynchronised.
    Desync(Desync),
    /// A snapshot older than the book was ignored.
    StaleSnapshot {
        /// The snapshot's position.
        snapshot_seq: Seq,
        /// The last processed block.
        block_number: u64,
    },
}

impl Output {
    /// The stream name (`bar`, `footprint`, ...).
    #[must_use]
    pub const fn stream(&self) -> &'static str {
        match self {
            Self::Bar(_) => "bar",
            Self::ActivityBar(_) => "activity_bar",
            Self::Footprint(_) => "footprint",
            Self::Delta(_) => "delta",
            Self::VolumeProfile(_) => "volume_profile",
            Self::ProfileSummary(_) => "profile_summary",
            Self::Checkpoint(_) => "checkpoint",
            Self::OrderActivity(_) => "order_activity",
            Self::Queue(_) => "queue",
            Self::Sweep(_) => "sweep",
            Self::Heatmap(_) => "heatmap",
            Self::Liquidity(_) => "liquidity",
            Self::Reconciliation(_) => "reconciliation",
            Self::Desync(_) => "desync",
            Self::StaleSnapshot { .. } => "stale_snapshot",
        }
    }

    /// The output as an observation [`Record`]: every derived record and
    /// checkpoint. Reconciliation reports, desyncs and stale-snapshot
    /// notices are operational and have no record kind.
    #[must_use]
    pub fn record(&self) -> Option<Record> {
        match self {
            Self::Bar(b) => Some(Record::Bar(b.clone())),
            Self::ActivityBar(b) => Some(Record::ActivityBar(b.clone())),
            Self::Footprint(f) => Some(Record::Footprint(f.clone())),
            Self::Delta(d) => Some(Record::DeltaBar(d.clone())),
            Self::VolumeProfile(p) => Some(Record::VolumeProfile(p.clone())),
            Self::ProfileSummary(p) => Some(Record::ProfileSummary(p.clone())),
            Self::Checkpoint(s) => Some(Record::BookSnapshot(s.clone())),
            Self::OrderActivity(a) => Some(Record::OrderActivity(a.clone())),
            Self::Queue(q) => Some(Record::QueuePositions(q.clone())),
            Self::Sweep(s) => Some(Record::Sweep(s.clone())),
            Self::Heatmap(h) => Some(Record::HeatmapTile(h.clone())),
            Self::Liquidity(l) => Some(Record::LiquidityGrade(l.clone())),
            Self::Reconciliation(_) | Self::Desync(_) | Self::StaleSnapshot { .. } => None,
        }
    }

    /// Consumes the output into a [`Record`] (see [`Output::record`]).
    #[must_use]
    pub fn into_record(self) -> Option<Record> {
        match self {
            Self::Bar(b) => Some(Record::Bar(b)),
            Self::ActivityBar(b) => Some(Record::ActivityBar(b)),
            Self::Footprint(f) => Some(Record::Footprint(f)),
            Self::Delta(d) => Some(Record::DeltaBar(d)),
            Self::VolumeProfile(p) => Some(Record::VolumeProfile(p)),
            Self::ProfileSummary(p) => Some(Record::ProfileSummary(p)),
            Self::Checkpoint(s) => Some(Record::BookSnapshot(s)),
            Self::OrderActivity(a) => Some(Record::OrderActivity(a)),
            Self::Queue(q) => Some(Record::QueuePositions(q)),
            Self::Sweep(s) => Some(Record::Sweep(s)),
            Self::Heatmap(h) => Some(Record::HeatmapTile(h)),
            Self::Liquidity(l) => Some(Record::LiquidityGrade(l)),
            Self::Reconciliation(_) | Self::Desync(_) | Self::StaleSnapshot { .. } => None,
        }
    }

    /// A key that orders outputs by stream, sub-stream, time and `seq`.
    /// [`crate::batch::run`] returns outputs sorted by it (stable, so ties
    /// keep generation order).
    #[must_use]
    pub fn sort_key(&self) -> (u8, i64, i64, Seq) {
        match self {
            Self::Bar(b) => (0, b.interval.millis(), b.start_ms.0, b.prov.seq),
            Self::ActivityBar(b) => {
                let sub = match b.spec {
                    ActivitySpec::Tick { trades } => i64::from(trades),
                    ActivitySpec::Volume { size } => size.mantissa(),
                    ActivitySpec::Range { width } => width.mantissa(),
                };
                let rank = match b.spec {
                    ActivitySpec::Tick { .. } => 1,
                    ActivitySpec::Volume { .. } => 2,
                    ActivitySpec::Range { .. } => 3,
                };
                (rank, sub, b.first_ms.0, b.prov.seq)
            }
            Self::Footprint(f) => (4, f.interval.millis(), f.start_ms.0, f.prov.seq),
            Self::Delta(d) => (5, d.interval.millis(), d.start_ms.0, d.prov.seq),
            Self::VolumeProfile(p) => (6, 0, p.start_ms.0, p.prov.seq),
            Self::ProfileSummary(p) => (7, 0, p.start_ms.0, p.prov.seq),
            Self::Checkpoint(s) => (8, 0, s.prov.ts_source_ms.0, s.prov.seq),
            Self::OrderActivity(a) => (9, a.interval.millis(), a.start_ms.0, a.prov.seq),
            Self::Queue(q) => (10, 0, q.prov.ts_source_ms.0, q.prov.seq),
            Self::Sweep(s) => (11, 0, s.prov.ts_source_ms.0, s.first_seq),
            Self::Heatmap(h) => (12, h.price_start.mantissa(), h.time_start_ms.0, h.prov.seq),
            Self::Liquidity(l) => (13, 0, l.as_of_ms.0, l.prov.seq),
            Self::Reconciliation(r) => (14, 0, 0, r.snapshot_seq),
            Self::Desync(d) => (15, 0, 0, d.seq),
            Self::StaleSnapshot { snapshot_seq, .. } => (16, 0, 0, *snapshot_seq),
        }
    }
}

/// One block of an instrument's records.
#[derive(Debug, Clone, Copy)]
pub struct BlockInput<'a> {
    /// Block number.
    pub block_number: u64,
    /// Block time.
    pub block_time: UnixMillis,
    /// The block's trades, in `seq` order.
    pub trades: &'a [Trade],
    /// The block's book events, in `seq` order.
    pub events: &'a [BookEvent],
    /// True if book-event blocks were skipped before this one.
    pub events_gap_before: bool,
}

#[derive(Debug, Clone)]
struct Pending {
    block_number: u64,
    block_time: UnixMillis,
    trades: Vec<Trade>,
    events: Vec<BookEvent>,
    have_trades: bool,
    have_events: bool,
    events_gap: bool,
}

/// Streams derived records for one instrument.
#[derive(Debug, Clone)]
pub struct Processor {
    instrument: InstrumentId,
    config: ProcessorConfig,
    book: Option<OrderBook>,
    state: BookState,
    checkpointer: Checkpointer,
    bars: BarEngine,
    activity: Vec<ActivityBarBuilder>,
    footprints: Vec<FootprintBuilder>,
    deltas: Vec<DeltaBuilder>,
    profile: Option<ProfileBuilder>,
    order_activity: Option<OrderActivityBuilder>,
    queue: Option<QueueTracker>,
    sweeps: Option<SweepDetector>,
    heatmap: Option<HeatmapBuilder>,
    liquidity: Option<LiquidityTracker>,
    last_block: Option<u64>,
    last_trade_batch: Option<u64>,
    last_event_batch: Option<u64>,
    pending: Option<Pending>,
    template: Option<mesh_types::Provenance>,
}

impl Processor {
    /// A processor whose book waits for a snapshot.
    ///
    /// # Errors
    ///
    /// Fails on an invalid configuration.
    pub fn new(instrument: InstrumentId, config: ProcessorConfig) -> Result<Self, InputError> {
        let c = &config;
        Ok(Self {
            bars: BarEngine::new(&c.bars)?,
            activity: c
                .activity_bars
                .iter()
                .map(|s| ActivityBarBuilder::new(*s, c.activity_reset))
                .collect::<Result<_, _>>()?,
            footprints: c
                .footprints
                .iter()
                .map(|i| FootprintBuilder::new(*i))
                .collect(),
            deltas: c
                .deltas
                .iter()
                .map(|i| DeltaBuilder::new(*i, c.cvd_anchor))
                .collect::<Result<_, _>>()?,
            profile: c.profile.map(ProfileBuilder::new).transpose()?,
            order_activity: c.order_activity.map(OrderActivityBuilder::new),
            queue: c.queue.map(QueueTracker::new),
            sweeps: c.sweeps.map(SweepDetector::new).transpose()?,
            heatmap: c.heatmap.map(HeatmapBuilder::new).transpose()?,
            liquidity: c.liquidity.map(LiquidityTracker::new).transpose()?,
            checkpointer: Checkpointer::new(c.checkpoint),
            instrument,
            book: None,
            state: BookState::Uninitialized,
            last_block: None,
            last_trade_batch: None,
            last_event_batch: None,
            pending: None,
            template: None,
            config,
        })
    }

    /// A processor whose book starts empty and in sync (a market observed
    /// from its first order).
    ///
    /// # Errors
    ///
    /// Fails on an invalid configuration.
    pub fn with_empty_book(
        instrument: InstrumentId,
        config: ProcessorConfig,
    ) -> Result<Self, InputError> {
        let mut p = Self::new(instrument.clone(), config)?;
        p.book = Some(OrderBook::new(instrument));
        p.state = BookState::InSync;
        Ok(p)
    }

    /// The instrument.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &ProcessorConfig {
        &self.config
    }

    /// The book (stale while desynchronised).
    #[must_use]
    pub fn book(&self) -> Option<&OrderBook> {
        self.book.as_ref()
    }

    /// The book state.
    #[must_use]
    pub fn state(&self) -> BookState {
        self.state
    }

    /// The last processed block.
    #[must_use]
    pub fn last_block(&self) -> Option<u64> {
        self.last_block
    }

    /// Adds a bus batch. Returns the outputs of any block completed by it.
    ///
    /// # Errors
    ///
    /// Fails on an invalid batch, another instrument, a batch for an
    /// already processed block, or a builder error.
    pub fn push_batch(&mut self, batch: &BlockBatch) -> Result<Vec<Output>, ProcessError> {
        batch.validate()?;
        self.check_instrument(&batch.instrument)?;
        let mut out = Vec::new();
        if let Some(p) = &self.pending {
            if batch.block_number < p.block_number {
                return Err(ProcessError::BlockOutOfOrder {
                    last: p.block_number,
                    got: batch.block_number,
                });
            }
            if batch.block_number > p.block_number {
                out = self.flush()?;
            }
        }
        if self.pending.is_none()
            && let Some(last) = self.last_block
            && batch.block_number <= last
        {
            return Err(ProcessError::BlockOutOfOrder {
                last,
                got: batch.block_number,
            });
        }
        let pending = self.pending.get_or_insert_with(|| Pending {
            block_number: batch.block_number,
            block_time: batch.block_time_ms,
            trades: Vec::new(),
            events: Vec::new(),
            have_trades: false,
            have_events: false,
            events_gap: false,
        });
        match batch.kind {
            RecordKind::Trade => {
                if pending.have_trades {
                    return Err(ProcessError::BlockOutOfOrder {
                        last: batch.block_number,
                        got: batch.block_number,
                    });
                }
                pending.have_trades = true;
                pending.trades = batch
                    .records
                    .iter()
                    .filter_map(|r| match r {
                        Record::Trade(t) => Some(t.clone()),
                        _ => None,
                    })
                    .collect();
                self.last_trade_batch = Some(batch.block_number);
            }
            RecordKind::BookEvent => {
                if pending.have_events {
                    return Err(ProcessError::BlockOutOfOrder {
                        last: batch.block_number,
                        got: batch.block_number,
                    });
                }
                pending.have_events = true;
                pending.events_gap = !batch.follows(self.last_event_batch);
                pending.events = batch
                    .records
                    .iter()
                    .filter_map(|r| match r {
                        Record::BookEvent(e) => Some(e.clone()),
                        _ => None,
                    })
                    .collect();
                self.last_event_batch = Some(batch.block_number);
            }
            other => return Err(ProcessError::UnexpectedKind(other)),
        }
        Ok(out)
    }

    /// Processes the buffered block, if any.
    ///
    /// # Errors
    ///
    /// As [`Processor::process_block`].
    pub fn flush(&mut self) -> Result<Vec<Output>, ProcessError> {
        let Some(p) = self.pending.take() else {
            return Ok(Vec::new());
        };
        self.run_block(BlockInput {
            block_number: p.block_number,
            block_time: p.block_time,
            trades: &p.trades,
            events: &p.events,
            events_gap_before: p.events_gap,
        })
    }

    /// Processes one whole block (after any buffered one).
    ///
    /// # Errors
    ///
    /// Fails on records of another instrument or block, a block not after
    /// the last one, or a builder error.
    pub fn process_block(&mut self, block: BlockInput<'_>) -> Result<Vec<Output>, ProcessError> {
        let mut out = self.flush()?;
        out.extend(self.run_block(block)?);
        Ok(out)
    }

    /// Processes a book snapshot taken after its block (`seq` end of
    /// block): initialises, reconciles or resyncs the book.
    ///
    /// # Errors
    ///
    /// Fails on another instrument, an invalid snapshot, or a builder
    /// error.
    pub fn process_snapshot(
        &mut self,
        snapshot: &BookSnapshot,
    ) -> Result<Vec<Output>, ProcessError> {
        self.check_instrument(&snapshot.prov.instrument)?;
        snapshot
            .validate()
            .map_err(|e| ProcessError::Snapshot(BookError::InvalidSnapshot(e)))?;
        let mut out = self.flush()?;
        let block = snapshot.prov.seq.block_number;
        let stale = match (self.state, &self.book) {
            (BookState::InSync, Some(b)) => b.seq().is_some_and(|s| s.block_number > block),
            _ => self.last_block.is_some_and(|l| l > block),
        };
        if stale {
            out.push(Output::StaleSnapshot {
                snapshot_seq: snapshot.prov.seq,
                block_number: self.last_block.unwrap_or(0),
            });
            return Ok(out);
        }
        let is_l4 = matches!(snapshot.depth, SnapshotDepth::L4 { .. });
        let canonical = canonical_snapshot(snapshot)?;
        match (self.state, self.book.as_mut()) {
            (BookState::InSync, Some(book)) => {
                let report = reconcile(book, snapshot, self.config.reconcile)
                    .map_err(ProcessError::Snapshot)?;
                if !report.is_clean() && is_l4 {
                    book.reset_to(snapshot).map_err(ProcessError::Snapshot)?;
                    self.checkpointer.loaded(block, snapshot.prov.ts_source_ms);
                }
                out.push(Output::Reconciliation(report));
            }
            (_, existing) => {
                if !is_l4 {
                    // An L2 snapshot cannot initialise an L4 book.
                    return Ok(out);
                }
                let fresh = OrderBook::from_snapshot(snapshot).map_err(ProcessError::Snapshot)?;
                if let Some(old) = existing {
                    let report = reconcile(old, snapshot, self.config.reconcile)
                        .map_err(ProcessError::Snapshot)?;
                    out.push(Output::Reconciliation(report));
                }
                self.book = Some(fresh);
                self.state = BookState::InSync;
                self.checkpointer.loaded(block, snapshot.prov.ts_source_ms);
            }
        }
        if let Some(q) = &mut self.queue {
            q.on_input(&canonical);
        }
        if let Some(h) = &mut self.heatmap {
            h.on_input(&canonical);
        }
        if let Some(l) = &mut self.liquidity {
            l.on_input(&canonical);
            l.after_block(&snapshot.prov);
        }
        if self.template.is_none() {
            self.template = Some(snapshot.prov.clone());
        }
        self.last_block = Some(self.last_block.map_or(block, |l| l.max(block)));
        Ok(out)
    }

    /// Time reached `ts`: emits every interval that ended at or before it.
    /// Later blocks must not be earlier than `ts`.
    ///
    /// # Errors
    ///
    /// Fails on a builder error.
    pub fn advance_to(&mut self, ts: UnixMillis) -> Result<Vec<Output>, ProcessError> {
        let mut out = self.flush()?;
        let mut bars = Vec::new();
        self.bars.advance_to(ts, &mut bars)?;
        out.extend(bars.into_iter().map(Output::Bar));
        let mut ab = Vec::new();
        for a in &mut self.activity {
            a.advance_to(ts, &mut ab)?;
        }
        out.extend(ab.into_iter().map(Output::ActivityBar));
        let mut fp = Vec::new();
        for f in &mut self.footprints {
            f.advance_to(ts, &mut fp)?;
        }
        out.extend(fp.into_iter().map(Output::Footprint));
        let mut d = Vec::new();
        for b in &mut self.deltas {
            b.advance_to(ts, &mut d)?;
        }
        out.extend(d.into_iter().map(Output::Delta));
        if let Some(p) = &mut self.profile {
            let mut v = Vec::new();
            p.advance_to(ts, &mut v)?;
            push_profiles(&mut out, v);
        }
        let in_sync = self.state == BookState::InSync;
        if let Some(a) = &mut self.order_activity {
            let mut v = Vec::new();
            a.advance_to(ts, &mut v)?;
            out.extend(v.into_iter().map(Output::OrderActivity));
        }
        if let (Some(h), Some(book), true) = (&mut self.heatmap, &self.book, in_sync) {
            let mut v = Vec::new();
            h.advance_to(book, ts, &mut v)?;
            out.extend(v.into_iter().map(Output::Heatmap));
        }
        if let Some(l) = &mut self.liquidity {
            let book = self.book.as_ref().filter(|_| in_sync);
            let mut v = Vec::new();
            l.advance_to(book, ts, &mut v)?;
            out.extend(v.into_iter().map(Output::Liquidity));
        }
        Ok(out)
    }

    /// Emits every open interval. The processor should not be used after.
    ///
    /// # Errors
    ///
    /// Fails on a builder error.
    pub fn finish(&mut self) -> Result<Vec<Output>, ProcessError> {
        let mut out = self.flush()?;
        let mut bars = Vec::new();
        self.bars.finish(&mut bars)?;
        out.extend(bars.into_iter().map(Output::Bar));
        let mut ab = Vec::new();
        for a in &mut self.activity {
            a.finish(&mut ab)?;
        }
        out.extend(ab.into_iter().map(Output::ActivityBar));
        let mut fp = Vec::new();
        for f in &mut self.footprints {
            f.finish(&mut fp)?;
        }
        out.extend(fp.into_iter().map(Output::Footprint));
        let mut d = Vec::new();
        for b in &mut self.deltas {
            b.finish(&mut d)?;
        }
        out.extend(d.into_iter().map(Output::Delta));
        if let Some(p) = &mut self.profile {
            let mut v = Vec::new();
            p.finish(&mut v)?;
            push_profiles(&mut out, v);
        }
        if let Some(a) = &mut self.order_activity {
            let mut v = Vec::new();
            a.finish(&mut v)?;
            out.extend(v.into_iter().map(Output::OrderActivity));
        }
        if let (Some(h), Some(book), BookState::InSync) =
            (&mut self.heatmap, &self.book, self.state)
        {
            let mut v = Vec::new();
            h.finish(book, &mut v)?;
            out.extend(v.into_iter().map(Output::Heatmap));
        }
        Ok(out)
    }

    fn check_instrument(&self, found: &InstrumentId) -> Result<(), ProcessError> {
        if *found == self.instrument {
            Ok(())
        } else {
            Err(ProcessError::InstrumentMismatch {
                expected: self.instrument.clone(),
                found: found.clone(),
            })
        }
    }

    fn desync(&mut self, seq: Seq, reason: DesyncReason, out: &mut Vec<Output>) {
        self.state = BookState::Desynced;
        self.checkpointer.reset();
        if let Some(q) = &mut self.queue {
            q.reset();
        }
        if let Some(h) = &mut self.heatmap {
            h.reset();
        }
        if let Some(a) = &mut self.order_activity {
            a.discard();
        }
        out.push(Output::Desync(Desync { seq, reason }));
    }

    fn run_block(&mut self, block: BlockInput<'_>) -> Result<Vec<Output>, ProcessError> {
        let b = block.block_number;
        if let Some(last) = self.last_block
            && b <= last
        {
            return Err(ProcessError::BlockOutOfOrder { last, got: b });
        }
        for p in block
            .trades
            .iter()
            .map(|t| &t.prov)
            .chain(block.events.iter().map(|e| &e.prov))
        {
            self.check_instrument(&p.instrument)?;
            if p.seq.block_number != b {
                return Err(ProcessError::Input(InputError::Inconsistent(format!(
                    "record {} in block {b}",
                    p.seq
                ))));
            }
        }
        let trade_canon = block
            .trades
            .iter()
            .map(canonical_trade)
            .collect::<Result<Vec<_>, _>>()?;
        let event_canon = block
            .events
            .iter()
            .map(canonical_event)
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        let t = block.block_time;

        // 1. Close heatmap columns and liquidity samples before the block.
        let in_sync = self.state == BookState::InSync;
        if let (Some(h), Some(book), true) = (&mut self.heatmap, &self.book, in_sync) {
            let mut v = Vec::new();
            h.before_block(book, t, &mut v)?;
            out.extend(v.into_iter().map(Output::Heatmap));
        }
        if let Some(l) = &mut self.liquidity {
            let book = self.book.as_ref().filter(|_| in_sync);
            let mut v = Vec::new();
            l.before_block(book, t, &mut v)?;
            out.extend(v.into_iter().map(Output::Liquidity));
        }

        // 2. Trades.
        let ins: Vec<TradeIn<'_>> = block
            .trades
            .iter()
            .zip(&trade_canon)
            .map(|(trade, c)| TradeIn {
                trade,
                canonical: c,
            })
            .collect();
        self.trades(&ins, &mut out)?;

        // 3. Events.
        let gap = block.events_gap_before
            || block
                .events
                .iter()
                .any(|e| e.prov.flags.contains(Flag::GapBefore));
        if gap
            && self.state == BookState::InSync
            && let Some(first) = block.events.first()
        {
            self.desync(first.prov.seq, DesyncReason::Gap, &mut out);
        }
        if let Some(l) = &mut self.liquidity {
            for c in &event_canon {
                l.on_input(c);
            }
        }
        let mut applied = 0usize;
        if self.state == BookState::InSync {
            for (ev, c) in block.events.iter().zip(&event_canon) {
                let Some(book) = self.book.as_mut() else {
                    break;
                };
                let before = book.order(ev.order_id);
                if let Err(e) = book.apply(ev) {
                    self.desync(
                        ev.prov.seq,
                        DesyncReason::Book {
                            error: e.to_string(),
                        },
                        &mut out,
                    );
                    break;
                }
                applied += 1;
                let input = EventIn {
                    event: ev,
                    canonical: c,
                };
                if let Some(a) = &mut self.order_activity {
                    let mut v = Vec::new();
                    a.observe(input, before.as_ref(), &mut v)?;
                    out.extend(v.into_iter().map(Output::OrderActivity));
                }
                if let Some(q) = &mut self.queue {
                    q.on_input(c);
                }
                if let Some(h) = &mut self.heatmap {
                    h.on_input(c);
                }
            }
        }

        // 4. Block end.
        if let Some(last) = block
            .events
            .last()
            .map(|e| &e.prov)
            .or(block.trades.last().map(|t| &t.prov))
        {
            self.template = Some(last.clone());
        }
        let eob = self.template.as_ref().map(|tpl| {
            let mut p = tpl.clone();
            p.seq = Seq::end_of_block(b);
            p.ts_source_ms = t;
            p.ts_publish_ns = None;
            p.flags = p.flags.without(Flag::GapBefore);
            p
        });
        if self.state == BookState::InSync
            && !block.events.is_empty()
            && let Some(book) = self.book.as_mut()
        {
            book.advance(Seq::end_of_block(b), t);
            if let Err(e) = book.check_crossed() {
                self.desync(
                    Seq::end_of_block(b),
                    DesyncReason::Book {
                        error: e.to_string(),
                    },
                    &mut out,
                );
            }
        }
        if let (BookState::InSync, Some(book), Some(eob)) = (self.state, &self.book, &eob) {
            if applied > 0 && self.checkpointer.after_block(b, t) {
                out.push(Output::Checkpoint(
                    book.to_snapshot(eob.clone())
                        .map_err(|e| InputError::Inconsistent(e.to_string()))?,
                ));
            }
            if let Some(q) = &mut self.queue {
                let mut v = Vec::new();
                q.after_block(book, eob, &mut v)?;
                out.extend(v.into_iter().map(Output::Queue));
            }
            if let Some(h) = &mut self.heatmap {
                h.after_block(eob);
            }
        }
        if let (Some(l), Some(eob)) = (&mut self.liquidity, &eob) {
            l.after_block(eob);
        }
        self.last_block = Some(b);
        Ok(out)
    }

    fn trades(&mut self, ins: &[TradeIn<'_>], out: &mut Vec<Output>) -> Result<(), ProcessError> {
        let mut bars = Vec::new();
        let mut ab = Vec::new();
        let mut fp = Vec::new();
        let mut d = Vec::new();
        let mut prof = Vec::new();
        for t in ins {
            self.bars.push(*t, &mut bars)?;
            for a in &mut self.activity {
                a.push(*t, &mut ab)?;
            }
            for f in &mut self.footprints {
                f.push(*t, &mut fp)?;
            }
            for b in &mut self.deltas {
                b.push(*t, &mut d)?;
            }
            if let Some(p) = &mut self.profile {
                p.push(*t, &mut prof)?;
            }
            if let Some(l) = &mut self.liquidity {
                l.on_trade(t.trade.prov.ts_source_ms, t.canonical);
            }
        }
        out.extend(bars.into_iter().map(Output::Bar));
        out.extend(ab.into_iter().map(Output::ActivityBar));
        out.extend(fp.into_iter().map(Output::Footprint));
        out.extend(d.into_iter().map(Output::Delta));
        push_profiles(out, prof);
        if let Some(s) = &mut self.sweeps {
            let mut v = Vec::new();
            s.on_block(ins, &mut v)?;
            out.extend(v.into_iter().map(Output::Sweep));
        }
        Ok(())
    }
}

fn push_profiles(out: &mut Vec<Output>, v: Vec<(VolumeProfile, Option<ProfileSummary>)>) {
    for (p, s) in v {
        out.push(Output::VolumeProfile(p));
        if let Some(s) = s {
            out.push(Output::ProfileSummary(s));
        }
    }
}
