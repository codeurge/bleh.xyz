//! Live (streaming) and batch (archive replay) outputs are identical, and
//! the processor's trade analytics equal the slice functions.

#![allow(clippy::unwrap_used, missing_docs)]

mod common;

use std::collections::BTreeSet;

use book::bars::{ActivitySpec, activity_bars, rollup, time_bars};
use book::batch::{BookStart, ReplayInput, run};
use book::checkpoint::{CheckpointPolicy, book_at, nearest_checkpoint};
use book::orderflow::{
    HeatmapConfig, ProfileConfig, SweepConfig, deltas, footprints, sweeps, volume_profiles,
};
use book::processor::{DesyncReason, Output};
use book::{BookState, OrderBook, Processor, ProcessorConfig};
use common::*;
use mesh_types::{BarInterval, BookAction, BookSnapshot, OrderId, UnixMillis};

fn config() -> ProcessorConfig {
    ProcessorConfig {
        checkpoint: CheckpointPolicy::EveryBlocks(500),
        activity_bars: vec![
            ActivitySpec::Tick { trades: 50 },
            ActivitySpec::Volume { size: size(20_000) },
            ActivitySpec::Range { width: price(10) },
        ],
        heatmap: Some(HeatmapConfig {
            price_step: Some(price(10)),
            ..HeatmapConfig::default()
        }),
        ..ProcessorConfig::default()
    }
}

struct Scenario {
    blocks: Vec<SimBlock>,
    snapshots: Vec<BookSnapshot>,
    end: UnixMillis,
}

/// Starts 40 minutes before a UTC midnight and runs 6000 blocks (about 85
/// minutes), so every bar interval and the daily reset roll over.
fn scenario(seed: u64) -> Scenario {
    let mut sim = Sim::new(seed);
    sim.time = BASE_MS + 86_400_000 - 2_400_000;
    let mut blocks = Vec::new();
    let mut snapshots = Vec::new();
    for i in 0..6_000 {
        let b = sim.step();
        if !b.trades.is_empty() || !b.events.is_empty() {
            blocks.push(b);
        }
        if i % 1_000 == 999 {
            snapshots.push(sim.snapshot());
        }
    }
    let end = UnixMillis(sim.time + 120_000);
    Scenario {
        blocks,
        snapshots,
        end,
    }
}

fn live(s: &Scenario) -> Vec<Output> {
    let mut p = Processor::with_empty_book(inst(), config()).unwrap();
    let mut out = Vec::new();
    let (mut pt, mut pe) = (None, None);
    let mut snaps = s.snapshots.iter().peekable();
    for (i, b) in s.blocks.iter().enumerate() {
        while let Some(snap) = snaps.peek() {
            if snap.prov.seq.block_number >= b.block_number {
                break;
            }
            out.extend(p.process_snapshot(snap).unwrap());
            snaps.next();
        }
        let mut bs = batches(b, &mut pt, &mut pe);
        if i % 2 == 1 {
            bs.reverse();
        }
        for batch in &bs {
            out.extend(p.push_batch(batch).unwrap());
        }
        if i % 37 == 0 {
            out.extend(p.advance_to(b.block_time).unwrap());
        }
    }
    for snap in snaps {
        out.extend(p.process_snapshot(snap).unwrap());
    }
    out.extend(p.advance_to(s.end).unwrap());
    out.extend(p.finish().unwrap());
    out.sort_by_key(Output::sort_key);
    out
}

fn batch(s: &Scenario) -> Vec<Output> {
    let trades = all_trades(&s.blocks);
    let events = all_events(&s.blocks);
    let times = block_times(&s.blocks);
    run(
        inst(),
        config(),
        ReplayInput {
            trades: &trades,
            events: &events,
            snapshots: &s.snapshots,
            block_times: &times,
            event_gaps: &BTreeSet::new(),
            start: BookStart::Empty,
            end: Some(s.end),
        },
    )
    .unwrap()
}

fn count(out: &[Output], stream: &str) -> usize {
    out.iter().filter(|o| o.stream() == stream).count()
}

#[test]
fn live_equals_batch() {
    let s = scenario(7);
    let a = live(&s);
    let b = batch(&s);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x, y);
    }
    // Every stream is exercised.
    for stream in [
        "bar",
        "activity_bar",
        "footprint",
        "delta",
        "volume_profile",
        "profile_summary",
        "checkpoint",
        "order_activity",
        "queue",
        "sweep",
        "heatmap",
        "liquidity",
        "reconciliation",
    ] {
        assert!(count(&a, stream) > 0, "no {stream} output");
    }
    assert_eq!(count(&a, "desync"), 0);
    for i in BarInterval::ALL {
        assert!(
            a.iter()
                .any(|o| matches!(o, Output::Bar(bar) if bar.interval == i)),
            "no {i} bars"
        );
    }
    // The simulator's snapshots match the rebuilt book.
    for o in &a {
        if let Output::Reconciliation(r) = o {
            assert!(r.is_clean(), "{r:?}");
        }
    }
    // Records the data model defines validate and pass the allowlist.
    for o in &a {
        if let Some(r) = o.record() {
            r.validate().unwrap();
            mesh_types::allowlist::check_record(&r).unwrap();
        }
    }
    // Two daily profiles (the run crosses UTC midnight).
    assert_eq!(count(&a, "volume_profile"), 2);
}

#[test]
fn outputs_are_deterministic_across_runs() {
    let s = scenario(11);
    let a = batch(&s);
    let b = batch(&s);
    let bytes = |v: &[Output]| -> Vec<Vec<u8>> {
        v.iter()
            .map(|o| mesh_types::canonical_json(o).unwrap())
            .collect()
    };
    assert_eq!(bytes(&a), bytes(&b));
}

#[test]
fn processor_trade_analytics_equal_slice_functions() {
    let s = scenario(3);
    let out = batch(&s);
    let trades = all_trades(&s.blocks);
    for i in BarInterval::ALL {
        let got: Vec<_> = out
            .iter()
            .filter_map(|o| match o {
                Output::Bar(b) if b.interval == i => Some(b.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(got, time_bars(&trades, i).unwrap(), "{i}");
    }
    let m1 = time_bars(&trades, BarInterval::M1).unwrap();
    for i in [
        BarInterval::M5,
        BarInterval::M15,
        BarInterval::H1,
        BarInterval::D1,
    ] {
        assert_eq!(
            rollup(&m1, i, &trades).unwrap(),
            time_bars(&trades, i).unwrap()
        );
    }
    let pick = |f: fn(&Output) -> Option<serde_json::Value>| -> Vec<serde_json::Value> {
        out.iter().filter_map(f).collect()
    };
    assert_eq!(
        pick(|o| match o {
            Output::Footprint(f) => Some(serde_json::to_value(f).unwrap()),
            _ => None,
        }),
        footprints(&trades, BarInterval::M1)
            .unwrap()
            .iter()
            .map(|f| serde_json::to_value(f).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        pick(|o| match o {
            Output::Delta(d) => Some(serde_json::to_value(d).unwrap()),
            _ => None,
        }),
        deltas(&trades, BarInterval::M1, BarInterval::D1)
            .unwrap()
            .iter()
            .map(|d| serde_json::to_value(d).unwrap())
            .collect::<Vec<_>>()
    );
    let profiles = volume_profiles(&trades, ProfileConfig::default()).unwrap();
    assert_eq!(
        pick(|o| match o {
            Output::VolumeProfile(p) => Some(serde_json::to_value(p).unwrap()),
            _ => None,
        }),
        profiles
            .iter()
            .map(|(p, _)| serde_json::to_value(p).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        pick(|o| match o {
            Output::Sweep(s) => Some(serde_json::to_value(s).unwrap()),
            _ => None,
        }),
        sweeps(&trades, SweepConfig::default())
            .unwrap()
            .iter()
            .map(|s| serde_json::to_value(s).unwrap())
            .collect::<Vec<_>>()
    );
    for spec in config().activity_bars {
        let got: Vec<_> = out
            .iter()
            .filter_map(|o| match o {
                Output::ActivityBar(b) if b.spec == spec => Some(b.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(got, activity_bars(&trades, spec, BarInterval::D1).unwrap());
    }
}

#[test]
fn checkpoints_answer_book_at_time() {
    let s = scenario(5);
    let out = batch(&s);
    let checkpoints: Vec<BookSnapshot> = out
        .iter()
        .filter_map(|o| match o {
            Output::Checkpoint(c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    assert!(checkpoints.len() >= 5);
    let events = all_events(&s.blocks);
    for b in s.blocks.iter().step_by(397) {
        let t = b.block_time;
        let Some(cp) = nearest_checkpoint(&checkpoints, t) else {
            continue;
        };
        let from_cp = book_at(cp, &events, t).unwrap();
        // Ground truth: replay every event from an empty book.
        let mut full = OrderBook::new(inst());
        for blk in s.blocks.iter().take_while(|x| x.block_time <= t) {
            if !blk.events.is_empty() {
                full.apply_block(blk.block_number, blk.block_time, &blk.events)
                    .unwrap();
            }
        }
        assert_eq!(
            from_cp.to_snapshot(cp.prov.clone()).unwrap().depth,
            full.to_snapshot(cp.prov.clone()).unwrap().depth
        );
    }
}

#[test]
fn inconsistency_desyncs_and_snapshot_resyncs() {
    let (sim, mut blocks) = simulate(21, 1_500);
    // Corrupt a cancel in the middle: unknown order id.
    let k = blocks.len() / 2;
    let (bi, ei) = blocks
        .iter()
        .enumerate()
        .skip(k)
        .find_map(|(i, b)| {
            b.events
                .iter()
                .position(|e| matches!(e.action, BookAction::Cancel {}))
                .map(|j| (i, j))
        })
        .unwrap();
    blocks[bi].events[ei].order_id = OrderId(u64::MAX);
    let mut p = Processor::with_empty_book(inst(), config()).unwrap();
    let mut out = Vec::new();
    let (mut pt, mut pe) = (None, None);
    for b in &blocks {
        for batch in batches(b, &mut pt, &mut pe) {
            out.extend(p.push_batch(&batch).unwrap());
        }
    }
    out.extend(p.flush().unwrap());
    let desyncs: Vec<_> = out
        .iter()
        .filter_map(|o| match o {
            Output::Desync(d) => Some(d.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(desyncs.len(), 1);
    assert_eq!(desyncs[0].seq, blocks[bi].events[ei].prov.seq);
    assert!(matches!(desyncs[0].reason, DesyncReason::Book { .. }));
    assert_eq!(p.state(), BookState::Desynced);

    // A fresh snapshot resyncs, with a non-empty diff report.
    let snap = sim.snapshot();
    let r = p.process_snapshot(&snap).unwrap();
    let Some(Output::Reconciliation(report)) = r.first() else {
        panic!("{r:?}")
    };
    assert!(!report.is_clean());
    assert_eq!(p.state(), BookState::InSync);
    assert_eq!(
        p.book().unwrap().to_snapshot(snap.prov.clone()).unwrap(),
        snap
    );
}

#[test]
fn event_gap_desyncs_until_snapshot() {
    let (sim, blocks) = simulate(33, 800);
    let mut p = Processor::with_empty_book(inst(), config()).unwrap();
    let mut out = Vec::new();
    let (mut pt, mut pe) = (None, None);
    let skip = blocks
        .iter()
        .position(|b| !b.events.is_empty() && b.block_number > blocks[100].block_number)
        .unwrap();
    for (i, b) in blocks.iter().enumerate() {
        for batch in batches(b, &mut pt, &mut pe) {
            if i == skip && batch.kind == mesh_types::RecordKind::BookEvent {
                continue; // lost on the way
            }
            out.extend(p.push_batch(&batch).unwrap());
        }
    }
    out.extend(p.flush().unwrap());
    assert!(out.iter().any(|o| matches!(
        o,
        Output::Desync(d) if d.reason == DesyncReason::Gap
    )));
    // No checkpoints or queue records after the gap.
    let gap_block = blocks[skip].block_number;
    assert!(!out.iter().any(|o| matches!(
        o,
        Output::Checkpoint(c) if c.prov.seq.block_number > gap_block
    )));
    p.process_snapshot(&sim.snapshot()).unwrap();
    assert_eq!(p.state(), BookState::InSync);
}

#[test]
fn snapshot_initialises_a_waiting_book_and_stale_ones_are_ignored() {
    let mut sim = Sim::new(44);
    let mut early = Vec::new();
    for _ in 0..300 {
        early.push(sim.step());
    }
    let snap = sim.snapshot();
    let mut later = Vec::new();
    for _ in 0..300 {
        later.push(sim.step());
    }
    let mut p = Processor::new(inst(), config()).unwrap();
    let (mut pt, mut pe) = (None, None);
    let mut out = Vec::new();
    for b in early
        .iter()
        .filter(|b| !b.trades.is_empty() || !b.events.is_empty())
    {
        for batch in batches(b, &mut pt, &mut pe) {
            out.extend(p.push_batch(&batch).unwrap());
        }
    }
    assert_eq!(p.state(), BookState::Uninitialized);
    out.extend(p.process_snapshot(&snap).unwrap());
    assert_eq!(p.state(), BookState::InSync);
    for b in later
        .iter()
        .filter(|b| !b.trades.is_empty() || !b.events.is_empty())
    {
        for batch in batches(b, &mut pt, &mut pe) {
            out.extend(p.push_batch(&batch).unwrap());
        }
    }
    out.extend(p.flush().unwrap());
    assert!(!out.iter().any(|o| matches!(o, Output::Desync(_))));
    let fin = sim.snapshot();
    assert_eq!(
        p.book().unwrap().to_snapshot(fin.prov.clone()).unwrap(),
        fin
    );
    // The old snapshot is now stale.
    let r = p.process_snapshot(&snap).unwrap();
    assert!(matches!(r.as_slice(), [Output::StaleSnapshot { .. }]));
}
