//! From [`ReaderBlock`]s to published [`BlockBatch`]es.
//!
//! The sequencer owns everything that depends on stream history rather
//! than on one block: each batch's `prev_block_number` (the previous block
//! with a batch for the same instrument and kind) and the `gap_before`
//! flag on the first record after a missing or incomplete block (spec § 9).
//! Its state is part of the checkpoint so a restart keeps the links.

use std::collections::BTreeMap;

use mesh_types::{BlockBatch, Flag, InstrumentId, RecordKind, UnixMillis};
use serde::{Deserialize, Serialize};

use crate::assumptions::is_block_gap;
use crate::normalize::ReaderBlock;

/// Link state of one published stream.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLink {
    /// Last block with a batch, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_block: Option<u64>,
    /// The next record must carry `gap_before`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gap_pending: bool,
}

/// Serializable sequencer state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerState {
    /// Last block sequenced.
    pub last_block: Option<u64>,
    /// Trade streams.
    #[serde(default)]
    pub trade: BTreeMap<InstrumentId, StreamLink>,
    /// Book-event streams.
    #[serde(default)]
    pub book_event: BTreeMap<InstrumentId, StreamLink>,
}

/// Everything published for one block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockOutput {
    /// Block height.
    pub block_number: u64,
    /// Block time.
    pub block_time_ms: UnixMillis,
    /// True if data was skipped immediately before or inside this block.
    pub gap_before: bool,
    /// One batch per instrument and kind with records, sorted by instrument
    /// then kind.
    pub batches: Vec<BlockBatch>,
}

/// Assigns `prev_block_number` links and block-gap flags.
#[derive(Debug, Clone, Default)]
pub struct Sequencer {
    state: SequencerState,
}

impl Sequencer {
    /// A sequencer resuming from `state`.
    #[must_use]
    pub fn new(state: SequencerState) -> Self {
        Self { state }
    }

    /// The current state, for checkpoints.
    #[must_use]
    pub fn state(&self) -> &SequencerState {
        &self.state
    }

    /// Last block sequenced.
    #[must_use]
    pub fn last_block(&self) -> Option<u64> {
        self.state.last_block
    }

    fn links(&mut self, kind: RecordKind) -> Option<&mut BTreeMap<InstrumentId, StreamLink>> {
        match kind {
            RecordKind::Trade => Some(&mut self.state.trade),
            RecordKind::BookEvent => Some(&mut self.state.book_event),
            _ => None,
        }
    }

    /// Sequences one block. Returns `None` for a block at or below the last
    /// one sequenced (a re-delivery after a restart).
    pub fn sequence(&mut self, block: ReaderBlock) -> Option<BlockOutput> {
        let n = block.block_number;
        if self.state.last_block.is_some_and(|l| n <= l) {
            return None;
        }
        let jumped = self.state.last_block.is_some_and(|l| is_block_gap(l, n));
        let gap = jumped || block.incomplete;
        if gap {
            for link in self
                .state
                .trade
                .values_mut()
                .chain(self.state.book_event.values_mut())
            {
                link.gap_pending = true;
            }
        }
        let mut batches = Vec::with_capacity(block.streams.len());
        for ((instrument, kind), mut records) in block.streams {
            let Some(first) = records.first_mut() else {
                continue;
            };
            let venue = first.prov().venue.clone();
            let Some(links) = self.links(kind) else {
                continue;
            };
            let link = links.entry(instrument.clone()).or_default();
            if link.gap_pending {
                first.prov_mut().flags.insert(Flag::GapBefore);
                link.gap_pending = false;
            }
            batches.push(BlockBatch {
                instrument,
                venue,
                kind,
                block_number: n,
                block_time_ms: block.block_time_ms,
                prev_block_number: link.last_block,
                records,
            });
            link.last_block = Some(n);
        }
        for (instrument, kind) in block.carry_gaps {
            if let Some(links) = self.links(kind) {
                links.entry(instrument).or_default().gap_pending = true;
            }
        }
        self.state.last_block = Some(n);
        Some(BlockOutput {
            block_number: n,
            block_time_ms: block.block_time_ms,
            gap_before: gap,
            batches,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use std::collections::BTreeSet;

    use super::*;
    use crate::normalize::BlockStats;
    use mesh_types::{
        AdmissionId, AggressorSide, Price, Provenance, Record, Seq, Size, Trade, TradeId,
        UnixNanos, VenueId,
    };

    fn id(s: &str) -> InstrumentId {
        InstrumentId::new(format!("hyperliquid:perp:{s}")).unwrap()
    }

    fn trade(inst: &str, block: u64, idx: u32) -> Record {
        Trade {
            prov: Provenance::new(
                id(inst),
                VenueId::new("hyperliquid").unwrap(),
                Seq::new(block, idx),
                UnixMillis(1),
                UnixNanos(2),
                AdmissionId::new("ADM-0001").unwrap(),
            ),
            price: Price::new(10, 1).unwrap(),
            size: Size::new(1, 0).unwrap(),
            aggressor: AggressorSide::Buy,
            trade_id: TradeId(1),
        }
        .into()
    }

    fn block(n: u64, insts: &[&str]) -> ReaderBlock {
        let mut streams = BTreeMap::new();
        for i in insts {
            streams.insert(
                (id(i), RecordKind::Trade),
                vec![trade(i, n, 0), trade(i, n, 1)],
            );
        }
        ReaderBlock {
            block_number: n,
            block_time_ms: UnixMillis(5),
            streams,
            incomplete: false,
            rejected: None,
            carry_gaps: BTreeSet::new(),
            stats: BlockStats::default(),
            resume: None,
        }
    }

    fn gap_flags(out: &BlockOutput) -> Vec<(String, bool)> {
        out.batches
            .iter()
            .map(|b| {
                (
                    b.instrument.symbol().to_owned(),
                    b.records[0].prov().flags.contains(Flag::GapBefore),
                )
            })
            .collect()
    }

    #[test]
    fn links_and_gaps() {
        let mut s = Sequencer::default();
        let o = s.sequence(block(10, &["AAA", "BBB"])).unwrap();
        assert!(o.batches.iter().all(|b| b.prev_block_number.is_none()));
        for b in &o.batches {
            b.validate().unwrap();
        }
        let o = s.sequence(block(11, &["AAA"])).unwrap();
        assert_eq!(o.batches[0].prev_block_number, Some(10));
        assert!(!o.gap_before);
        assert!(s.sequence(block(11, &["AAA"])).is_none());

        // Block 12 missing: the next record of every known stream is flagged,
        // once.
        let o = s.sequence(block(13, &["AAA"])).unwrap();
        assert!(o.gap_before);
        assert_eq!(gap_flags(&o), vec![("AAA".into(), true)]);
        assert!(
            !o.batches[0].records[1]
                .prov()
                .flags
                .contains(Flag::GapBefore)
        );
        let o = s.sequence(block(14, &["AAA", "BBB", "CCC"])).unwrap();
        assert_eq!(
            gap_flags(&o),
            vec![
                ("AAA".into(), false),
                ("BBB".into(), true),
                ("CCC".into(), false)
            ]
        );
        assert_eq!(o.batches[1].prev_block_number, Some(10));
        assert_eq!(o.batches[2].prev_block_number, None);

        // State round-trips through JSON (checkpoint).
        let json = serde_json::to_string(s.state()).unwrap();
        let mut s2 = Sequencer::new(serde_json::from_str(&json).unwrap());
        let mut b = block(15, &["CCC"]);
        b.carry_gaps.insert((id("CCC"), RecordKind::Trade));
        let o = s2.sequence(b).unwrap();
        assert_eq!(o.batches[0].prev_block_number, Some(14));
        assert_eq!(
            gap_flags(&s2.sequence(block(16, &["CCC"])).unwrap()),
            vec![("CCC".into(), true)]
        );

        // An incomplete block flags its own records.
        let mut b = block(17, &["AAA"]);
        b.incomplete = true;
        assert_eq!(
            gap_flags(&s2.sequence(b).unwrap()),
            vec![("AAA".into(), true)]
        );
    }
}
