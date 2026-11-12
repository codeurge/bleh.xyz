//! Derived records: one book [`Processor`] per instrument, driven block by
//! block, plus the venue [`Coverage`](mesh_types::Coverage) signal.
//!
//! The live origin and the archive replay call the same [`Deriver`] with
//! the same block outputs, so derived records are identical on both paths
//! (observation-v0 § 2.8):
//!
//! 1. every `trade` and `book_event` batch of block `B` goes to its
//!    instrument's processor;
//! 2. every processor is flushed and advanced to `B`'s block time, so quiet
//!    markets close their intervals too;
//! 3. derived records are positioned at `B`: `seq = (B, i)` per instrument
//!    and kind, in emission order. Book checkpoints and queue positions keep
//!    the end-of-block marker of `B`;
//! 4. the coverage record of the previous interval, if `B` starts a new
//!    one, is added.
//!
//! L4 snapshots (from the node's `fileSnapshot`) are usually older than the
//! last processed block by the time they are read. The deriver keeps the
//! book events of the last `recent_blocks` blocks and rolls a snapshot
//! forward to the last processed block before handing it to the
//! processor, which initialises, reconciles or resets the book with it.

use std::collections::{BTreeMap, HashMap, VecDeque};

use book::{BookState, OrderBook, Output, Processor, ProcessorConfig};
use hl_node_reader::BlockOutput;
use mesh_types::{
    AdmissionId, BarInterval, BookEvent, BookSnapshot, Flag, InstrumentId, Provenance, Record,
    RecordKind, Seq, UnixMillis, UnixNanos, VenueId,
};

use crate::coverage::CoverageBuilder;

/// How a processor's book starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookStart {
    /// Empty and in sync: the input starts at the market's first order (a
    /// new market, the synthetic fixtures, the first recorded day).
    Empty,
    /// Unknown until the first L4 snapshot (production: the reader starts
    /// in the middle of every market's life).
    Snapshot,
}

/// Deriver settings.
#[derive(Debug, Clone)]
pub struct DeriveConfig {
    /// Venue of every input.
    pub venue: VenueId,
    /// Admission of coverage records.
    pub admission: AdmissionId,
    /// What every processor builds.
    pub processor: ProcessorConfig,
    /// How books start.
    pub book_start: BookStart,
    /// Coverage interval.
    pub coverage_interval: BarInterval,
    /// Blocks of book events kept to roll snapshots forward.
    pub recent_blocks: usize,
}

impl DeriveConfig {
    /// Hyperliquid under ADM-0001 with every analytic on, books from
    /// snapshots, one-minute coverage and one minute of recent blocks.
    ///
    /// # Errors
    ///
    /// Never in practice (fixed identifiers).
    pub fn hyperliquid() -> Result<Self, mesh_types::IdError> {
        Ok(Self {
            venue: VenueId::new("hyperliquid")?,
            admission: AdmissionId::new("ADM-0001")?,
            processor: ProcessorConfig::default(),
            book_start: BookStart::Snapshot,
            coverage_interval: BarInterval::M1,
            recent_blocks: 600,
        })
    }
}

/// Something operators should know about, not a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A snapshot was compared with the book.
    Reconciled {
        /// Instrument.
        instrument: InstrumentId,
        /// True if the book matched.
        clean: bool,
        /// Orders missing from the book, extra in it, and changed.
        differences: (usize, usize, usize),
    },
    /// The book desynchronised.
    Desync {
        /// Instrument.
        instrument: InstrumentId,
        /// Position.
        seq: Seq,
        /// Why.
        reason: String,
    },
    /// A snapshot could not be used: older than the processed blocks and
    /// than the recent-events buffer, or inconsistent with the events.
    SnapshotUnusable {
        /// Instrument.
        instrument: InstrumentId,
        /// The snapshot's block.
        block: u64,
        /// Why.
        reason: String,
    },
    /// A processor rejected its input.
    ProcessError {
        /// Instrument.
        instrument: InstrumentId,
        /// The error.
        error: String,
    },
}

/// What one block (or one snapshot) produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DerivedBlock {
    /// Records to publish (and sign) in the block.
    pub records: Vec<Record>,
    /// Authoritative books at the end of the block for instruments whose
    /// book was just initialised or reset, for gateways to reseed from
    /// (outside the signed stream).
    pub resyncs: Vec<BookSnapshot>,
    /// Notices.
    pub notices: Vec<Notice>,
}

#[derive(Debug, Clone)]
struct Recent {
    number: u64,
    time: UnixMillis,
    events: BTreeMap<InstrumentId, Vec<BookEvent>>,
}

#[derive(Debug, Clone)]
struct Proc {
    p: Processor,
    last_state: BookState,
    template: Option<Provenance>,
}

/// Builds derived records block by block.
#[derive(Debug, Clone)]
pub struct Deriver {
    cfg: DeriveConfig,
    procs: BTreeMap<InstrumentId, Proc>,
    coverage: CoverageBuilder,
    recent: VecDeque<Recent>,
    pending: Vec<BookSnapshot>,
    last: Option<(u64, UnixMillis)>,
}

impl Deriver {
    /// A deriver with no instruments yet.
    #[must_use]
    pub fn new(cfg: DeriveConfig) -> Self {
        Self {
            coverage: CoverageBuilder::new(
                cfg.venue.clone(),
                cfg.admission.clone(),
                cfg.coverage_interval,
            ),
            cfg,
            procs: BTreeMap::new(),
            recent: VecDeque::new(),
            pending: Vec::new(),
            last: None,
        }
    }

    /// The last processed block.
    #[must_use]
    pub fn last_block(&self) -> Option<u64> {
        self.last.map(|(b, _)| b)
    }

    /// The book of `instrument`, if its processor has one in sync.
    #[must_use]
    pub fn book(&self, instrument: &InstrumentId) -> Option<&OrderBook> {
        self.procs
            .get(instrument)
            .filter(|p| p.p.state() == BookState::InSync)
            .and_then(|p| p.p.book())
    }

    fn proc_for(
        &mut self,
        instrument: &InstrumentId,
        start: BookStart,
    ) -> Result<&mut Proc, String> {
        if !self.procs.contains_key(instrument) {
            let config = self.cfg.processor.clone();
            let p = match start {
                BookStart::Empty => Processor::with_empty_book(instrument.clone(), config),
                BookStart::Snapshot => Processor::new(instrument.clone(), config),
            }
            .map_err(|e| e.to_string())?;
            self.procs.insert(
                instrument.clone(),
                Proc {
                    p,
                    last_state: BookState::Uninitialized,
                    template: None,
                },
            );
        }
        self.procs
            .get_mut(instrument)
            .ok_or_else(|| "processor missing".to_owned())
    }

    /// Processes one published block. Never fails: problems are notices.
    pub fn block(&mut self, out: &BlockOutput) -> DerivedBlock {
        let b = out.block_number;
        let t = out.block_time_ms;
        let mut result = DerivedBlock::default();
        let mut outs: Vec<(InstrumentId, Output)> = Vec::new();
        let mut recent = Recent {
            number: b,
            time: t,
            events: BTreeMap::new(),
        };
        let start = self.cfg.book_start;
        for batch in &out.batches {
            if !matches!(batch.kind, RecordKind::Trade | RecordKind::BookEvent) {
                continue;
            }
            if batch.kind == RecordKind::BookEvent && self.cfg.recent_blocks > 0 {
                recent.events.insert(
                    batch.instrument.clone(),
                    batch
                        .records
                        .iter()
                        .filter_map(|r| match r {
                            Record::BookEvent(e) => Some(e.clone()),
                            _ => None,
                        })
                        .collect(),
                );
            }
            let proc = match self.proc_for(&batch.instrument, start) {
                Ok(p) => p,
                Err(error) => {
                    result.notices.push(Notice::ProcessError {
                        instrument: batch.instrument.clone(),
                        error,
                    });
                    continue;
                }
            };
            if let Some(first) = batch.records.first() {
                let mut prov = first.prov().clone();
                prov.ts_publish_ns = None;
                proc.template = Some(prov);
            }
            match proc.p.push_batch(batch) {
                Ok(o) => outs.extend(o.into_iter().map(|o| (batch.instrument.clone(), o))),
                Err(e) => result.notices.push(Notice::ProcessError {
                    instrument: batch.instrument.clone(),
                    error: e.to_string(),
                }),
            }
        }
        for (inst, proc) in &mut self.procs {
            for step in [proc.p.flush(), proc.p.advance_to(t)] {
                match step {
                    Ok(o) => outs.extend(o.into_iter().map(|o| (inst.clone(), o))),
                    Err(e) => result.notices.push(Notice::ProcessError {
                        instrument: inst.clone(),
                        error: e.to_string(),
                    }),
                }
            }
        }
        self.last = Some((b, t));
        if self.cfg.recent_blocks > 0 {
            self.recent.push_back(recent);
            while self.recent.len() > self.cfg.recent_blocks {
                self.recent.pop_front();
            }
        }
        self.place(b, outs, &mut result);
        // Snapshots that waited for this block.
        let due: Vec<BookSnapshot> = {
            let (due, keep) = std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|s| s.prov.seq.block_number <= b);
            self.pending = keep;
            due
        };
        for s in due {
            self.apply_snapshot(&s, &mut result);
        }
        self.collect_resyncs(&mut result);
        if let Some(c) = self.coverage.push(b, t, out.gap_before) {
            result.records.push(Record::Coverage(c));
        }
        result
    }

    /// Hands L4 snapshots to the processors: now, rolled forward to the
    /// last processed block, or once their block is processed.
    pub fn snapshots(&mut self, snapshots: Vec<BookSnapshot>) -> DerivedBlock {
        let mut result = DerivedBlock::default();
        for s in snapshots {
            if self
                .last_block()
                .is_none_or(|l| l < s.prov.seq.block_number)
            {
                self.pending.push(s);
            } else {
                self.apply_snapshot(&s, &mut result);
            }
        }
        self.collect_resyncs(&mut result);
        result
    }

    /// Rolls `s` forward to the last processed block and processes it.
    fn apply_snapshot(&mut self, s: &BookSnapshot, result: &mut DerivedBlock) {
        let inst = s.prov.instrument.clone();
        let h = s.prov.seq.block_number;
        let unusable = |reason: String| Notice::SnapshotUnusable {
            instrument: inst.clone(),
            block: h,
            reason,
        };
        let Some((last, last_time)) = self.last else {
            return;
        };
        let rolled = if last == h {
            s.clone()
        } else {
            if self
                .recent
                .front()
                .is_none_or(|r| r.number > h.saturating_add(1))
            {
                result.notices.push(unusable(format!(
                    "older than the {} buffered blocks",
                    self.recent.len()
                )));
                return;
            }
            let mut book = match OrderBook::from_snapshot(s) {
                Ok(b) => b,
                Err(e) => {
                    result.notices.push(unusable(e.to_string()));
                    return;
                }
            };
            for r in self.recent.iter().filter(|r| r.number > h) {
                if let Some(evs) = r.events.get(&inst)
                    && let Err((i, e)) = book.apply_block(r.number, r.time, evs)
                {
                    result.notices.push(unusable(format!(
                        "event {i} of block {} does not fit: {e}",
                        r.number
                    )));
                    return;
                }
            }
            let mut prov = s.prov.clone();
            prov.seq = Seq::end_of_block(last);
            prov.ts_source_ms = last_time;
            match book.to_snapshot(prov) {
                Ok(snap) => snap,
                Err(e) => {
                    result.notices.push(unusable(e.to_string()));
                    return;
                }
            }
        };
        let proc = match self.proc_for(&inst, BookStart::Snapshot) {
            Ok(p) => p,
            Err(error) => {
                result.notices.push(Notice::ProcessError {
                    instrument: inst.clone(),
                    error,
                });
                return;
            }
        };
        if proc.template.is_none() {
            proc.template = Some(rolled.prov.clone());
        }
        match proc.p.process_snapshot(&rolled) {
            Ok(outs) => {
                let mut reset = false;
                let mut placed = Vec::new();
                for o in outs {
                    match o {
                        Output::Reconciliation(r) => {
                            let clean = r.is_clean();
                            reset |= !clean;
                            result.notices.push(Notice::Reconciled {
                                instrument: inst.clone(),
                                clean,
                                differences: (r.missing.len(), r.extra.len(), r.changed.len()),
                            });
                        }
                        Output::StaleSnapshot { block_number, .. } => {
                            result
                                .notices
                                .push(unusable(format!("stale against block {block_number}")));
                        }
                        other => placed.push((inst.clone(), other)),
                    }
                }
                if reset {
                    // The book was replaced: gateways reseed from it.
                    if let Some(p) = self.procs.get_mut(&inst) {
                        p.last_state = BookState::Desynced;
                    }
                }
                if !placed.is_empty() {
                    self.place(last, placed, result);
                }
            }
            Err(e) => result.notices.push(Notice::ProcessError {
                instrument: inst,
                error: e.to_string(),
            }),
        }
    }

    /// Resync snapshots for books that came into sync since the last call.
    fn collect_resyncs(&mut self, result: &mut DerivedBlock) {
        let Some((b, t)) = self.last else {
            return;
        };
        for (inst, proc) in &mut self.procs {
            let st = proc.p.state();
            if st == BookState::InSync && proc.last_state != BookState::InSync {
                let prov = match &proc.template {
                    Some(t0) => {
                        let mut p = t0.clone();
                        p.seq = Seq::end_of_block(b);
                        p.ts_source_ms = t;
                        p.ts_ingest_ns = UnixNanos(t.0.saturating_mul(1_000_000));
                        p.flags = p.flags.without(Flag::GapBefore);
                        p
                    }
                    None => Provenance::new(
                        inst.clone(),
                        self.cfg.venue.clone(),
                        Seq::end_of_block(b),
                        t,
                        UnixNanos(t.0.saturating_mul(1_000_000)),
                        self.cfg.admission.clone(),
                    ),
                };
                if let Some(book) = proc.p.book()
                    && let Ok(snap) = book.to_snapshot(prov)
                {
                    result.resyncs.push(snap);
                }
            }
            proc.last_state = st;
        }
    }

    /// Positions the outputs of block `b` and turns them into records and
    /// notices.
    fn place(&self, b: u64, outs: Vec<(InstrumentId, Output)>, result: &mut DerivedBlock) {
        let mut counters: HashMap<(InstrumentId, RecordKind), u32> = HashMap::new();
        for (inst, o) in outs {
            match o {
                Output::Reconciliation(r) => result.notices.push(Notice::Reconciled {
                    instrument: inst,
                    clean: r.is_clean(),
                    differences: (r.missing.len(), r.extra.len(), r.changed.len()),
                }),
                Output::Desync(d) => result.notices.push(Notice::Desync {
                    instrument: inst,
                    seq: d.seq,
                    reason: match d.reason {
                        book::processor::DesyncReason::Book { error } => error,
                        book::processor::DesyncReason::Gap => "gap".into(),
                    },
                }),
                Output::StaleSnapshot { snapshot_seq, .. } => {
                    result.notices.push(Notice::SnapshotUnusable {
                        instrument: inst,
                        block: snapshot_seq.block_number,
                        reason: "stale".into(),
                    });
                }
                other => {
                    let Some(mut r) = other.into_record() else {
                        continue;
                    };
                    match r.kind() {
                        RecordKind::BookSnapshot | RecordKind::QueuePositions => {
                            if r.prov().seq != Seq::end_of_block(b) {
                                result.notices.push(Notice::ProcessError {
                                    instrument: inst,
                                    error: format!(
                                        "{} at {:?} emitted in block {b}; dropped",
                                        r.kind(),
                                        r.prov().seq
                                    ),
                                });
                                continue;
                            }
                        }
                        kind => {
                            let i = counters.entry((inst, kind)).or_insert(0);
                            r.prov_mut().seq = Seq::new(b, *i);
                            *i = i.saturating_add(1);
                        }
                    }
                    result.records.push(r);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hl_node_reader::replay::{Replay, sources_from_texts};
    use mesh_types::{Registry, SnapshotDepth};

    pub(crate) fn synth() -> (hl_synth::FileSet, Registry) {
        let files =
            hl_synth::generate(hl_synth::Config::default()).unwrap_or_else(|e| panic!("{e:?}"));
        let reg: Registry =
            serde_json::from_slice(&files["markets.json"]).unwrap_or_else(|e| panic!("{e}"));
        (files, reg)
    }

    pub(crate) fn texts(files: &hl_synth::FileSet, scenario: &str) -> [String; 3] {
        [
            "node_fills_by_block",
            "node_order_statuses_by_block",
            "node_raw_book_diffs_by_block",
        ]
        .map(|dir| {
            files
                .iter()
                .filter(|(k, _)| k.starts_with(&format!("{scenario}/{dir}/")))
                .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
                .collect::<Vec<_>>()
                .join("")
        })
    }

    fn outputs(scenario: &str) -> (Vec<BlockOutput>, Registry, hl_synth::FileSet) {
        let (files, reg) = synth();
        let blocks = Replay::new(sources_from_texts(texts(&files, scenario)), reg.clone())
            .sequenced()
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|e| panic!("{e}"));
        (blocks, reg, files)
    }

    fn cfg(start: BookStart) -> DeriveConfig {
        let mut c = DeriveConfig::hyperliquid().unwrap_or_else(|e| panic!("{e}"));
        c.book_start = start;
        c.processor.checkpoint = book::checkpoint::CheckpointPolicy::EveryBlocks(10);
        c
    }

    fn triples(depth: &SnapshotDepth) -> Vec<(u64, String, String)> {
        let SnapshotDepth::L4 { bids, asks } = depth else {
            return vec![];
        };
        bids.iter()
            .chain(asks)
            .map(|o| (o.order_id.0, o.price.to_string(), o.size.to_string()))
            .collect()
    }

    fn expected(
        files: &hl_synth::FileSet,
        scenario: &str,
        coin: &str,
    ) -> Vec<(u64, String, String)> {
        let v: serde_json::Value =
            serde_json::from_slice(&files[&format!("{scenario}/expected.json")])
                .unwrap_or_else(|e| panic!("{e}"));
        let book = &v["coins"][coin]["final_book"];
        ["bids", "asks"]
            .iter()
            .flat_map(|side| book[*side].as_array().cloned().unwrap_or_default())
            .map(|o| {
                (
                    o["order_id"].as_u64().unwrap_or_default(),
                    o["price"].as_str().unwrap_or_default().to_owned(),
                    o["size"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn empty_books_match_and_records_are_positioned() {
        let (blocks, reg, files) = outputs("base");
        let mut d = Deriver::new(cfg(BookStart::Empty));
        let mut records = Vec::new();
        let mut resyncs = 0;
        for b in &blocks {
            let out = d.block(b);
            assert!(out.notices.is_empty(), "{:?}", out.notices);
            for r in &out.records {
                r.validate().unwrap_or_else(|e| panic!("{e}: {r:?}"));
                mesh_types::allowlist::check_record(r).unwrap_or_else(|e| panic!("{e}"));
                assert_eq!(r.prov().seq.block_number, b.block_number);
            }
            resyncs += out.resyncs.len();
            records.extend(out.records);
        }
        assert_eq!(resyncs, 3);
        for spec in reg.iter() {
            let book = d
                .book(&spec.id)
                .unwrap_or_else(|| panic!("{} book", spec.id));
            let snap = book
                .to_snapshot(Provenance::new(
                    spec.id.clone(),
                    spec.venue.clone(),
                    Seq::end_of_block(1),
                    UnixMillis(0),
                    UnixNanos(0),
                    spec.admission.clone(),
                ))
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                triples(&snap.depth),
                expected(&files, "base", &spec.venue_symbol)
            );
        }
        // Every derived stream obeys seq rules: (B, 0..n) per instrument
        // and kind.
        let mut seen: HashMap<(InstrumentId, RecordKind, u64), Vec<u32>> = HashMap::new();
        for r in &records {
            if r.prov().seq.is_end_of_block() {
                continue;
            }
            seen.entry((
                r.prov().instrument.clone(),
                r.kind(),
                r.prov().seq.block_number,
            ))
            .or_default()
            .push(r.prov().seq.event_index);
        }
        for v in seen.values() {
            assert_eq!(
                *v,
                (0..u32::try_from(v.len()).unwrap_or(0)).collect::<Vec<_>>()
            );
        }
        for kind in [
            RecordKind::Bar,
            RecordKind::BookSnapshot,
            RecordKind::QueuePositions,
            RecordKind::Coverage,
            RecordKind::Footprint,
            RecordKind::DeltaBar,
        ] {
            assert!(records.iter().any(|r| r.kind() == kind), "no {kind}");
        }
        // Deterministic.
        let mut again = Deriver::new(cfg(BookStart::Empty));
        let replayed: Vec<Record> = blocks.iter().flat_map(|b| again.block(b).records).collect();
        assert_eq!(replayed, records);
    }

    #[test]
    fn gap_resyncs_from_a_rolled_forward_snapshot() {
        let (blocks, reg, files) = outputs("gap");
        let snap_text = files
            .iter()
            .find(|(k, _)| k.starts_with("gap/l4_snapshots/"))
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
            .unwrap_or_else(|| panic!("snapshot"));
        let height = hl_node_reader::snapshot::parse_l4_envelope(&snap_text)
            .unwrap_or_else(|e| panic!("{e}"))
            .height;
        let mut d = Deriver::new(cfg(BookStart::Empty));
        let mut desyncs = 0;
        let mut reseeded = 0;
        for b in &blocks {
            let out = d.block(b);
            desyncs += out
                .notices
                .iter()
                .filter(|n| matches!(n, Notice::Desync { .. }))
                .count();
            // The snapshot is read a few blocks after its height, as live.
            if b.block_number == height + 3 {
                let parsed = hl_node_reader::snapshot::parse_l4_snapshot(
                    &snap_text,
                    &reg,
                    b.block_time_ms,
                    UnixNanos(0),
                )
                .unwrap_or_else(|e| panic!("{e}"));
                let out = d.snapshots(parsed.snapshots);
                assert!(
                    out.notices
                        .iter()
                        .all(|n| !matches!(n, Notice::SnapshotUnusable { .. })),
                    "{:?}",
                    out.notices
                );
                reseeded += out.resyncs.len();
            }
        }
        assert!(desyncs > 0);
        assert!(reseeded > 0);
        for spec in reg.iter() {
            let book = d
                .book(&spec.id)
                .unwrap_or_else(|| panic!("{} book", spec.id));
            let snap = book
                .to_snapshot(Provenance::new(
                    spec.id.clone(),
                    spec.venue.clone(),
                    Seq::end_of_block(1),
                    UnixMillis(0),
                    UnixNanos(0),
                    spec.admission.clone(),
                ))
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                triples(&snap.depth),
                expected(&files, "gap", &spec.venue_symbol)
            );
        }
    }

    #[test]
    fn snapshot_mode_waits_for_a_snapshot() {
        let (blocks, reg, files) = outputs("base");
        let snap_text = files
            .iter()
            .find(|(k, _)| k.starts_with("base/l4_snapshots/"))
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
            .unwrap_or_else(|| panic!("snapshot"));
        let height = hl_node_reader::snapshot::parse_l4_envelope(&snap_text)
            .unwrap_or_else(|e| panic!("{e}"))
            .height;
        let mut d = Deriver::new(cfg(BookStart::Snapshot));
        let mut checkpoints_before = 0;
        for b in &blocks {
            if b.block_number == height {
                // Delivered early: it waits for its block.
                let parsed = hl_node_reader::snapshot::parse_l4_snapshot(
                    &snap_text,
                    &reg,
                    b.block_time_ms,
                    UnixNanos(0),
                )
                .unwrap_or_else(|e| panic!("{e}"));
                assert!(d.snapshots(parsed.snapshots).resyncs.is_empty());
            }
            let out = d.block(b);
            if b.block_number < height {
                checkpoints_before += out
                    .records
                    .iter()
                    .filter(|r| r.kind() == RecordKind::BookSnapshot)
                    .count();
                assert!(out.resyncs.is_empty());
            }
            if b.block_number == height {
                assert_eq!(out.resyncs.len(), 3);
            }
        }
        assert_eq!(checkpoints_before, 0);
        for spec in reg.iter() {
            assert!(d.book(&spec.id).is_some());
        }
    }
}
