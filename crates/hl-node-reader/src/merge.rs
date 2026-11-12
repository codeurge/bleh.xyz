//! Active-active merge of two readers (ADR 0006).
//!
//! Two readers tail two nodes. Each delivers [`ReaderBlock`]s in block
//! order. The merger publishes every block once, from whichever reader
//! delivers it first, and compares the second copy with the first on
//! `(block, event_index)` content. It raises an [`IntegrityAlarm`] when
//!
//! - the copies differ (`Mismatch`): any record, ignoring only the fields
//!   that legitimately differ between nodes (`ts_ingest_ns`, the node's own
//!   receive time, and `gap_before`, which depends on each reader's
//!   history);
//! - one reader has a block the other skipped, or does not deliver within
//!   `confirm_window_blocks` of the leader (`OneSidedGap`);
//! - neither reader delivers a block before the output must move on
//!   (`MergedGap`);
//! - a reader delivers nothing for `stall_after` (`ReaderStalled`, then
//!   `ReaderRecovered`).
//!
//! When the leading reader skips a block, its later blocks are held for up
//! to `gap_wait` so the other reader can fill the hole; this costs latency
//! only when a gap actually occurs. [`Merger`] is a pure state machine
//! (time is passed in); [`run_merge`] drives it on tokio channels.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mesh_types::{ContentHasher, Flag, Sha256Digest, UnixNanos};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::checkpoint::ReaderPositions;
use crate::metrics::{Metrics, names};
use crate::normalize::ReaderBlock;

/// One of the two readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderId {
    /// The first configured reader.
    A,
    /// The second configured reader.
    B,
}

impl ReaderId {
    /// Index (0 or 1).
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            ReaderId::A => 0,
            ReaderId::B => 1,
        }
    }

    /// The other reader.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            ReaderId::A => ReaderId::B,
            ReaderId::B => ReaderId::A,
        }
    }

    /// Label (`a` or `b`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ReaderId::A => "a",
            ReaderId::B => "b",
        }
    }
}

/// Merge settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeConfig {
    /// How long to hold blocks after a hole for the other reader to fill it.
    pub gap_wait: Duration,
    /// How many blocks behind the leader a reader may be before its missing
    /// copies count as one-sided gaps.
    pub confirm_window_blocks: u64,
    /// Silence after which a reader is reported stalled.
    pub stall_after: Duration,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            gap_wait: Duration::from_millis(1_000),
            confirm_window_blocks: 100,
            stall_after: Duration::from_secs(10),
        }
    }
}

/// An integrity alarm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "alarm", rename_all = "snake_case")]
pub enum IntegrityAlarm {
    /// The two readers' copies of a block differ.
    Mismatch {
        /// Block.
        block_number: u64,
        /// The differing streams (`instrument/kind`, or `block_time`).
        streams: Vec<String>,
    },
    /// One reader delivered blocks the other did not.
    OneSidedGap {
        /// The reader that is missing them.
        missing: ReaderId,
        /// First missing block.
        first_block: u64,
        /// Last missing block.
        last_block: u64,
    },
    /// Neither reader delivered these blocks; the output skips them.
    MergedGap {
        /// First missing block.
        first_block: u64,
        /// Last missing block.
        last_block: u64,
    },
    /// A reader has delivered nothing for the stall timeout.
    ReaderStalled {
        /// The reader.
        reader: ReaderId,
    },
    /// A stalled reader delivers again.
    ReaderRecovered {
        /// The reader.
        reader: ReaderId,
    },
}

impl IntegrityAlarm {
    /// Metric label.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            IntegrityAlarm::Mismatch { .. } => "mismatch",
            IntegrityAlarm::OneSidedGap { .. } => "one_sided_gap",
            IntegrityAlarm::MergedGap { .. } => "merged_gap",
            IntegrityAlarm::ReaderStalled { .. } => "reader_stalled",
            IntegrityAlarm::ReaderRecovered { .. } => "reader_recovered",
        }
    }
}

/// Content digest of a block per stream, ignoring node-local fields.
pub type BlockDigest = BTreeMap<String, Sha256Digest>;

/// Digests a block for comparison between readers: per stream, the content
/// hash of the canonical records with `ts_ingest_ns` zeroed, no publish
/// time and no `gap_before` flag; plus the block time and any rejection.
#[must_use]
pub fn block_digest(block: &ReaderBlock) -> BlockDigest {
    let mut out = BlockDigest::new();
    out.insert(
        "block_time".into(),
        Sha256Digest::of(block.block_time_ms.0.to_string().as_bytes()),
    );
    if let Some(r) = &block.rejected {
        out.insert("rejected".into(), Sha256Digest::of(r.as_bytes()));
    }
    for ((instrument, kind), records) in &block.streams {
        let mut h = ContentHasher::new();
        for r in records {
            let mut r = r.clone();
            let p = r.prov_mut();
            p.ts_ingest_ns = UnixNanos(0);
            p.ts_publish_ns = None;
            p.flags = p.flags.without(Flag::GapBefore);
            match mesh_types::hash::canonical_record(&r) {
                Ok(c) => h.update_canonical(&c),
                Err(e) => h.update_canonical(format!("unencodable: {e}").as_bytes()),
            }
        }
        out.insert(format!("{instrument}/{kind}"), h.finalize().0);
    }
    out
}

fn differing(a: &BlockDigest, b: &BlockDigest) -> Vec<String> {
    let mut keys: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    keys.extend(b.keys().filter(|k| !a.contains_key(*k)).cloned());
    keys.sort();
    keys
}

/// A published block with the resume positions of both readers at the time
/// (for the checkpoint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedBlock {
    /// The block.
    pub block: ReaderBlock,
    /// Per reader, where it resumes; covers only blocks already handled.
    pub positions: [Option<ReaderPositions>; 2],
}

/// The result of one merger step.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeStep {
    /// Blocks to publish, in order.
    pub blocks: Vec<MergedBlock>,
    /// Alarms raised.
    pub alarms: Vec<IntegrityAlarm>,
}

#[derive(Debug)]
struct Pending {
    from: ReaderId,
    digest: BlockDigest,
    incomplete: bool,
}

#[derive(Debug)]
struct Held {
    from: ReaderId,
    block: ReaderBlock,
    digest: BlockDigest,
    since: Instant,
    confirmed: bool,
}

/// The merge state machine.
#[derive(Debug)]
pub struct Merger {
    cfg: MergeConfig,
    metrics: Arc<Metrics>,
    emitted: Option<u64>,
    held: BTreeMap<u64, Held>,
    confirm: BTreeMap<u64, Pending>,
    high: [Option<u64>; 2],
    last_seen: [Instant; 2],
    stalled: [bool; 2],
    positions: [Option<ReaderPositions>; 2],
}

impl Merger {
    /// A merger. `resume_after` is the last block already published (from
    /// the checkpoint); copies at or below it are dropped.
    #[must_use]
    pub fn new(
        cfg: MergeConfig,
        metrics: Arc<Metrics>,
        resume_after: Option<u64>,
        now: Instant,
    ) -> Self {
        Self {
            cfg,
            metrics,
            emitted: resume_after,
            held: BTreeMap::new(),
            confirm: BTreeMap::new(),
            high: [None; 2],
            last_seen: [now; 2],
            stalled: [false; 2],
            positions: [None, None],
        }
    }

    /// Last block published.
    #[must_use]
    pub fn emitted(&self) -> Option<u64> {
        self.emitted
    }

    fn alarm(&self, step: &mut MergeStep, a: IntegrityAlarm) {
        self.metrics.inc(names::MERGE_ALARMS, &[("kind", a.kind())]);
        match &a {
            IntegrityAlarm::Mismatch { .. } => {
                tracing::error!(alarm = ?a, "integrity alarm: readers disagree");
            }
            _ => tracing::warn!(alarm = ?a, "integrity alarm"),
        }
        step.alarms.push(a);
    }

    fn one_sided(&self, step: &mut MergeStep, missing: ReaderId, blocks: &[u64]) {
        if blocks.is_empty() {
            return;
        }
        self.metrics.add(
            names::MERGE_ONE_SIDED,
            &[("missing", missing.as_str())],
            blocks.len() as f64,
        );
        if self.stalled[missing.index()] {
            return; // Covered by the stall alarm.
        }
        let mut start = blocks[0];
        let mut prev = blocks[0];
        for &b in &blocks[1..] {
            if b != prev + 1 {
                self.alarm(
                    step,
                    IntegrityAlarm::OneSidedGap {
                        missing,
                        first_block: start,
                        last_block: prev,
                    },
                );
                start = b;
            }
            prev = b;
        }
        self.alarm(
            step,
            IntegrityAlarm::OneSidedGap {
                missing,
                first_block: start,
                last_block: prev,
            },
        );
    }

    fn compare(
        &self,
        step: &mut MergeStep,
        n: u64,
        first: (&BlockDigest, bool),
        second: (&BlockDigest, bool),
    ) {
        if first.1 || second.1 {
            // An incomplete copy cannot be compared; the gap is already
            // flagged on the published stream.
            return;
        }
        let diff = differing(first.0, second.0);
        if diff.is_empty() {
            self.metrics.inc(names::MERGE_CONFIRMED, &[]);
        } else {
            self.metrics.inc(names::MERGE_MISMATCHES, &[]);
            self.alarm(
                step,
                IntegrityAlarm::Mismatch {
                    block_number: n,
                    streams: diff,
                },
            );
        }
    }

    fn touch(&mut self, r: ReaderId, now: Instant, step: &mut MergeStep) {
        self.last_seen[r.index()] = now;
        if self.stalled[r.index()] {
            self.stalled[r.index()] = false;
            self.alarm(step, IntegrityAlarm::ReaderRecovered { reader: r });
        }
    }

    fn record_position(&mut self, r: ReaderId, block: &ReaderBlock) {
        if let Some(p) = &block.resume {
            self.positions[r.index()] = Some(p.clone());
        }
    }

    fn emit(&mut self, step: &mut MergeStep, r: ReaderId, h: (ReaderBlock, BlockDigest, bool)) {
        let (block, digest, confirmed) = h;
        let n = block.block_number;
        self.record_position(r, &block);
        self.emitted = Some(n);
        if !confirmed {
            let other = r.other();
            if self.high[other.index()].is_some_and(|h| h >= n) {
                // The other reader is already past this block without
                // having delivered it.
                self.one_sided(step, other, &[n]);
            } else {
                self.confirm.insert(
                    n,
                    Pending {
                        from: r,
                        digest,
                        incomplete: block.incomplete,
                    },
                );
            }
        }
        step.blocks.push(MergedBlock {
            block,
            positions: self.positions.clone(),
        });
    }

    fn try_release(&mut self, now: Instant, step: &mut MergeStep) {
        while let Some((&k, h)) = self.held.first_key_value() {
            let Some(e) = self.emitted else {
                break;
            };
            if k <= e {
                self.held.remove(&k);
                self.metrics.inc(names::MERGE_LATE, &[]);
                continue;
            }
            if k > e + 1 {
                let other = h.from.other().index();
                let other_passed = self.high[other].is_some_and(|hh| hh > e);
                let waited = now.saturating_duration_since(h.since) >= self.cfg.gap_wait;
                if !(other_passed || waited || self.stalled[other]) {
                    break;
                }
                let (first, last) = (e + 1, k - 1);
                self.metrics
                    .add(names::MERGE_GAP_BLOCKS, &[], (last - first + 1) as f64);
                self.alarm(
                    step,
                    IntegrityAlarm::MergedGap {
                        first_block: first,
                        last_block: last,
                    },
                );
            }
            if let Some(h) = self.held.remove(&k) {
                self.emit(step, h.from, (h.block, h.digest, h.confirmed));
            }
        }
    }

    /// Handles a block delivered by reader `r`.
    pub fn receive(&mut self, r: ReaderId, block: ReaderBlock, now: Instant) -> MergeStep {
        let mut step = MergeStep::default();
        self.touch(r, now, &mut step);
        let n = block.block_number;
        let digest = block_digest(&block);

        if self.high[r.index()].is_none_or(|h| n > h) {
            // Blocks the other reader published below `n` and above this
            // reader's previous block will never come from this reader:
            // it delivers in order.
            let lo = self.high[r.index()].map_or(0, |h| h.saturating_add(1));
            let skipped: Vec<u64> = self
                .confirm
                .range(lo..n)
                .filter(|(_, p)| p.from != r)
                .map(|(&k, _)| k)
                .collect();
            for k in &skipped {
                self.confirm.remove(k);
            }
            self.one_sided(&mut step, r, &skipped);
            self.high[r.index()] = Some(n);
        }

        if self.emitted.is_some_and(|e| n <= e) {
            self.record_position(r, &block);
            match self.confirm.remove(&n) {
                Some(p) if p.from != r => self.compare(
                    &mut step,
                    n,
                    (&p.digest, p.incomplete),
                    (&digest, block.incomplete),
                ),
                Some(p) => {
                    self.confirm.insert(n, p);
                }
                None => self.metrics.inc(names::MERGE_LATE, &[]),
            }
            return step;
        }

        if let Some(h) = self.held.get(&n) {
            if h.from != r && !h.confirmed {
                let (first_digest, first_incomplete) = (h.digest.clone(), h.block.incomplete);
                self.compare(
                    &mut step,
                    n,
                    (&first_digest, first_incomplete),
                    (&digest, block.incomplete),
                );
                if let Some(h) = self.held.get_mut(&n) {
                    h.confirmed = true;
                    if h.block.incomplete && !block.incomplete {
                        // Publish the complete copy instead.
                        h.from = r;
                        h.block = block;
                        h.digest = digest;
                    }
                }
            }
            self.try_release(now, &mut step);
            return step;
        }

        if self.emitted.is_none_or(|e| n == e + 1) {
            self.emit(&mut step, r, (block, digest, false));
        } else {
            self.held.insert(
                n,
                Held {
                    from: r,
                    block,
                    digest,
                    since: now,
                    confirmed: false,
                },
            );
        }
        self.try_release(now, &mut step);
        step
    }

    /// Time-based work: stall detection, releasing held blocks after
    /// `gap_wait`, and expiring unconfirmed blocks beyond the window.
    pub fn tick(&mut self, now: Instant) -> MergeStep {
        let mut step = MergeStep::default();
        for r in [ReaderId::A, ReaderId::B] {
            let i = r.index();
            if !self.stalled[i]
                && now.saturating_duration_since(self.last_seen[i]) >= self.cfg.stall_after
            {
                self.stalled[i] = true;
                self.alarm(&mut step, IntegrityAlarm::ReaderStalled { reader: r });
            }
        }
        self.try_release(now, &mut step);
        let leader = self.high.iter().flatten().max().copied().unwrap_or(0);
        let limit = leader.saturating_sub(self.cfg.confirm_window_blocks);
        let expired: Vec<u64> = self.confirm.range(..limit).map(|(&k, _)| k).collect();
        let mut missing: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
        for k in expired {
            if let Some(p) = self.confirm.remove(&k) {
                missing[p.from.other().index()].push(k);
            }
        }
        self.one_sided(&mut step, ReaderId::A, &missing[0]);
        self.one_sided(&mut step, ReaderId::B, &missing[1]);
        step
    }

    /// Releases every held block (both inputs ended).
    pub fn finish(&mut self) -> MergeStep {
        let mut step = MergeStep::default();
        let far = Instant::now() + self.cfg.gap_wait + Duration::from_secs(1);
        for i in 0..2 {
            self.stalled[i] = true;
        }
        self.try_release(far, &mut step);
        step
    }
}

/// Merger output event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeEvent {
    /// A block to publish.
    Block(Box<MergedBlock>),
    /// An alarm.
    Alarm(IntegrityAlarm),
}

async fn send_step(out: &mpsc::Sender<MergeEvent>, step: MergeStep) -> bool {
    for a in step.alarms {
        if out.send(MergeEvent::Alarm(a)).await.is_err() {
            return false;
        }
    }
    for b in step.blocks {
        if out.send(MergeEvent::Block(Box::new(b))).await.is_err() {
            return false;
        }
    }
    true
}

/// Drives a [`Merger`] over two reader channels until both close.
pub async fn run_merge(
    mut a: mpsc::Receiver<ReaderBlock>,
    mut b: mpsc::Receiver<ReaderBlock>,
    out: mpsc::Sender<MergeEvent>,
    mut merger: Merger,
    tick: Duration,
) {
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let (mut a_open, mut b_open) = (true, true);
    while a_open || b_open {
        let step = tokio::select! {
            m = a.recv(), if a_open => match m {
                Some(block) => merger.receive(ReaderId::A, block, Instant::now()),
                None => { a_open = false; MergeStep::default() }
            },
            m = b.recv(), if b_open => match m {
                Some(block) => merger.receive(ReaderId::B, block, Instant::now()),
                None => { b_open = false; MergeStep::default() }
            },
            _ = interval.tick() => merger.tick(Instant::now()),
        };
        if !send_step(&out, step).await {
            return;
        }
    }
    let _ = send_step(&out, merger.finish()).await;
}

/// Passes one reader's blocks through as merged blocks (single-node mode).
pub async fn run_single(mut a: mpsc::Receiver<ReaderBlock>, out: mpsc::Sender<MergeEvent>) {
    while let Some(block) = a.recv().await {
        let positions = [block.resume.clone(), None];
        if out
            .send(MergeEvent::Block(Box::new(MergedBlock {
                block,
                positions,
            })))
            .await
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::collections::BTreeSet;

    use super::*;
    use crate::normalize::BlockStats;
    use mesh_types::{
        AdmissionId, AggressorSide, InstrumentId, Price, Provenance, Record, RecordKind, Seq, Size,
        Trade, TradeId, UnixMillis, VenueId,
    };

    fn block(n: u64, price: i64, ingest: i64) -> ReaderBlock {
        let id = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        let t: Record = Trade {
            prov: Provenance::new(
                id.clone(),
                VenueId::new("hyperliquid").unwrap(),
                Seq::new(n, 0),
                UnixMillis(1_000 + i64::try_from(n).unwrap()),
                UnixNanos(ingest),
                AdmissionId::new("ADM-0001").unwrap(),
            ),
            price: Price::new(price, 1).unwrap(),
            size: Size::new(1, 0).unwrap(),
            aggressor: AggressorSide::Buy,
            trade_id: TradeId(n),
        }
        .into();
        let mut streams = BTreeMap::new();
        streams.insert((id, RecordKind::Trade), vec![t]);
        ReaderBlock {
            block_number: n,
            block_time_ms: UnixMillis(1_000 + i64::try_from(n).unwrap()),
            streams,
            incomplete: false,
            rejected: None,
            carry_gaps: BTreeSet::new(),
            stats: BlockStats::default(),
            resume: None,
        }
    }

    fn cfg() -> MergeConfig {
        MergeConfig {
            gap_wait: Duration::from_millis(100),
            confirm_window_blocks: 5,
            stall_after: Duration::from_secs(60),
        }
    }

    fn emitted(steps: &[MergeStep]) -> Vec<u64> {
        steps
            .iter()
            .flat_map(|s| s.blocks.iter().map(|b| b.block.block_number))
            .collect()
    }

    #[test]
    fn dedupes_and_confirms() {
        let m = Arc::new(Metrics::new());
        let t0 = Instant::now();
        let mut g = Merger::new(cfg(), m.clone(), None, t0);
        let mut steps = Vec::new();
        for n in 1..=5 {
            // Different ingest times on the two nodes are not a mismatch.
            steps.push(g.receive(ReaderId::A, block(n, 10, 1), t0));
            steps.push(g.receive(ReaderId::B, block(n, 10, 2), t0));
        }
        assert_eq!(emitted(&steps), vec![1, 2, 3, 4, 5]);
        assert!(steps.iter().all(|s| s.alarms.is_empty()));
        assert!((m.get(names::MERGE_CONFIRMED, &[]) - 5.0).abs() < f64::EPSILON);
        // B leads for a while: still emitted once.
        let s = g.receive(ReaderId::B, block(6, 10, 2), t0);
        assert_eq!(emitted(&[s]), vec![6]);
        let s = g.receive(ReaderId::A, block(6, 10, 1), t0);
        assert!(s.blocks.is_empty() && s.alarms.is_empty());
    }

    #[test]
    fn mismatch_raises_alarm() {
        let m = Arc::new(Metrics::new());
        let t0 = Instant::now();
        let mut g = Merger::new(cfg(), m.clone(), None, t0);
        g.receive(ReaderId::A, block(1, 10, 1), t0);
        let s = g.receive(ReaderId::B, block(1, 11, 1), t0);
        assert_eq!(
            s.alarms,
            vec![IntegrityAlarm::Mismatch {
                block_number: 1,
                streams: vec!["hyperliquid:perp:SYN/trade".into()],
            }]
        );
        assert!((m.get(names::MERGE_MISMATCHES, &[]) - 1.0).abs() < f64::EPSILON);
        assert!((m.get(names::MERGE_ALARMS, &[("kind", "mismatch")]) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn other_reader_fills_a_hole() {
        let m = Arc::new(Metrics::new());
        let t0 = Instant::now();
        let mut g = Merger::new(cfg(), m.clone(), None, t0);
        let mut steps = vec![
            g.receive(ReaderId::A, block(1, 10, 1), t0),
            g.receive(ReaderId::A, block(3, 10, 1), t0), // A skipped 2: held
        ];
        assert_eq!(emitted(&steps), vec![1]);
        steps.push(g.receive(ReaderId::B, block(1, 10, 2), t0));
        steps.push(g.receive(ReaderId::B, block(2, 10, 2), t0));
        assert_eq!(emitted(&steps), vec![1, 2, 3]);
        // A is already past 2: one-sided gap for A, no merged gap.
        let alarms: Vec<_> = steps.iter().flat_map(|s| s.alarms.clone()).collect();
        assert_eq!(
            alarms,
            vec![IntegrityAlarm::OneSidedGap {
                missing: ReaderId::A,
                first_block: 2,
                last_block: 2
            }]
        );
        // B's copy of 3 confirms the held block.
        let s = g.receive(ReaderId::B, block(3, 10, 2), t0);
        assert!(s.alarms.is_empty());
        assert!((m.get(names::MERGE_CONFIRMED, &[]) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn hole_in_both_is_released_after_wait() {
        let m = Arc::new(Metrics::new());
        let t0 = Instant::now();
        let mut g = Merger::new(cfg(), m.clone(), None, t0);
        g.receive(ReaderId::A, block(1, 10, 1), t0);
        let s = g.receive(ReaderId::A, block(3, 10, 1), t0);
        assert!(s.blocks.is_empty());
        assert!(g.tick(t0 + Duration::from_millis(50)).blocks.is_empty());
        let s = g.tick(t0 + Duration::from_millis(150));
        assert_eq!(emitted(&[s]), vec![3]);
        assert!((m.get(names::MERGE_GAP_BLOCKS, &[]) - 1.0).abs() < f64::EPSILON);
        // The other reader passing the hole releases at once.
        let mut g = Merger::new(cfg(), Arc::new(Metrics::new()), None, t0);
        g.receive(ReaderId::A, block(1, 10, 1), t0);
        g.receive(ReaderId::B, block(1, 10, 1), t0);
        g.receive(ReaderId::A, block(3, 10, 1), t0);
        let s = g.receive(ReaderId::B, block(4, 10, 1), t0);
        assert_eq!(emitted(&[s]), vec![3, 4]);
    }

    #[test]
    fn lagging_reader_expires_into_one_sided_gaps_and_stalls() {
        let m = Arc::new(Metrics::new());
        let t0 = Instant::now();
        let mut g = Merger::new(cfg(), m.clone(), None, t0);
        for n in 1..=10 {
            g.receive(ReaderId::A, block(n, 10, 1), t0);
        }
        let s = g.tick(t0);
        assert_eq!(
            s.alarms,
            vec![IntegrityAlarm::OneSidedGap {
                missing: ReaderId::B,
                first_block: 1,
                last_block: 4
            }]
        );
        let s = g.tick(t0 + Duration::from_secs(61));
        assert_eq!(
            s.alarms,
            vec![
                IntegrityAlarm::ReaderStalled {
                    reader: ReaderId::A
                },
                IntegrityAlarm::ReaderStalled {
                    reader: ReaderId::B
                }
            ]
        );
        let s = g.receive(ReaderId::B, block(11, 10, 1), t0 + Duration::from_secs(62));
        assert!(s.alarms.contains(&IntegrityAlarm::ReaderRecovered {
            reader: ReaderId::B
        }));
        assert!(s.alarms.iter().any(|a| matches!(
            a,
            IntegrityAlarm::OneSidedGap {
                missing: ReaderId::B,
                first_block: 5,
                last_block: 10
            }
        )));
        assert_eq!(emitted(&[s]), vec![11]);
    }

    #[test]
    fn incomplete_copy_is_replaced_while_held() {
        let t0 = Instant::now();
        let mut g = Merger::new(cfg(), Arc::new(Metrics::new()), Some(1), t0);
        let mut partial = block(3, 10, 1);
        partial.incomplete = true;
        partial.streams.clear();
        // A skipped 2 and has only part of 3: held.
        assert!(g.receive(ReaderId::A, partial, t0).blocks.is_empty());
        // B skipped 2 as well and has all of 3: its copy is published, after
        // a merged-gap alarm for 2.
        let s = g.receive(ReaderId::B, block(3, 10, 2), t0);
        assert_eq!(
            s.alarms,
            vec![IntegrityAlarm::MergedGap {
                first_block: 2,
                last_block: 2
            }]
        );
        assert_eq!(s.blocks.len(), 1);
        assert!(!s.blocks[0].block.incomplete);
        assert_eq!(s.blocks[0].block.record_count(), 1);
        assert!(g.finish().blocks.is_empty());
    }

    #[tokio::test]
    async fn async_driver_merges_two_channels() {
        let (ta, ra) = mpsc::channel(16);
        let (tb, rb) = mpsc::channel(16);
        let (to, mut ro) = mpsc::channel(64);
        let g = Merger::new(cfg(), Arc::new(Metrics::new()), None, Instant::now());
        let h = tokio::spawn(run_merge(ra, rb, to, g, Duration::from_millis(10)));
        for n in 1..=3 {
            ta.send(block(n, 10, 1)).await.unwrap();
            tb.send(block(n, if n == 2 { 12 } else { 10 }, 2))
                .await
                .unwrap();
        }
        drop((ta, tb));
        h.await.unwrap();
        let mut blocks = Vec::new();
        let mut alarms = Vec::new();
        while let Some(e) = ro.recv().await {
            match e {
                MergeEvent::Block(b) => blocks.push(b.block.block_number),
                MergeEvent::Alarm(a) => alarms.push(a),
            }
        }
        assert_eq!(blocks, vec![1, 2, 3]);
        assert!(alarms.iter().any(|a| matches!(
            a,
            IntegrityAlarm::Mismatch {
                block_number: 2,
                ..
            }
        )));
    }
}
