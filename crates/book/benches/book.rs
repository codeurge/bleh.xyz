//! Benchmarks on a large synthetic book (100,000 resting orders).

#![allow(clippy::unwrap_used, missing_docs, unreachable_pub)]

#[path = "../tests/common/mod.rs"]
mod common;

use std::collections::BTreeSet;
use std::hint::black_box;

use book::batch::{BookStart, ReplayInput, run};
use book::reconcile::{ReconcileOptions, reconcile};
use book::{OrderBook, ProcessorConfig};
use common::*;
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use mesh_types::{BookSide, Seq};

const ORDERS_PER_SIDE: usize = 50_000;

/// A book with 100,000 orders and the next 2,000 blocks of activity.
fn setup() -> (OrderBook, Vec<SimBlock>, Sim) {
    let mut sim = Sim::new(99);
    let seed = sim.seed_book(ORDERS_PER_SIDE);
    let mut book = OrderBook::new(inst());
    book.apply_block(seed.block_number, seed.block_time, &seed.events)
        .unwrap();
    sim.max_actions = 20;
    let blocks: Vec<SimBlock> = (0..2_000).map(|_| sim.step()).collect();
    (book, blocks, sim)
}

fn bench(c: &mut Criterion) {
    let (book, blocks, sim) = setup();
    let events: usize = blocks.iter().map(|b| b.events.len()).sum();
    let trades: usize = blocks.iter().map(|b| b.trades.len()).sum();

    let mut g = c.benchmark_group("book_100k");
    g.sample_size(20);

    g.throughput(Throughput::Elements(events as u64));
    g.bench_function("apply_events", |b| {
        b.iter_batched(
            || book.clone(),
            |mut bk| {
                for blk in &blocks {
                    if !blk.events.is_empty() {
                        bk.apply_block(blk.block_number, blk.block_time, &blk.events)
                            .unwrap();
                    }
                }
                bk
            },
            BatchSize::LargeInput,
        );
    });

    g.throughput(Throughput::Elements(1));
    g.bench_function("top_of_book", |b| b.iter(|| black_box(book.top())));
    g.bench_function("l2_top_100_levels", |b| {
        b.iter(|| black_box(book.levels(BookSide::Bid, 100).unwrap()))
    });

    let mut eob = prov(sim.block, 0, sim.time);
    eob.seq = Seq::end_of_block(sim.block);
    let snap = book.to_snapshot(eob.clone()).unwrap();
    g.bench_function("to_l4_snapshot", |b| {
        b.iter(|| black_box(book.to_snapshot(eob.clone()).unwrap()))
    });
    g.bench_function("from_l4_snapshot", |b| {
        b.iter(|| black_box(OrderBook::from_snapshot(&snap).unwrap()))
    });
    g.bench_function("reconcile_clean", |b| {
        b.iter(|| black_box(reconcile(&book, &snap, ReconcileOptions::default()).unwrap()))
    });
    g.finish();

    // Full processor (every analytic on), archive replay of the 2,000
    // blocks on top of the 100k book.
    let mut g = c.benchmark_group("processor");
    g.sample_size(10);
    g.throughput(Throughput::Elements((events + trades) as u64));
    let seed_snapshot = {
        let mut p = prov(blocks[0].block_number - 1, 0, blocks[0].block_time.0 - 1);
        p.seq = Seq::end_of_block(blocks[0].block_number - 1);
        book.to_snapshot(p).unwrap()
    };
    let snapshots = [seed_snapshot];
    let blocks: Vec<SimBlock> = blocks
        .into_iter()
        .filter(|b| !b.trades.is_empty() || !b.events.is_empty())
        .collect();
    let all_t = all_trades(&blocks);
    let all_e = all_events(&blocks);
    let times = block_times(&blocks);
    let gaps = BTreeSet::new();
    g.bench_function("replay_all_analytics", |b| {
        b.iter(|| {
            black_box(
                run(
                    inst(),
                    ProcessorConfig::default(),
                    ReplayInput {
                        trades: &all_t,
                        events: &all_e,
                        snapshots: &snapshots,
                        block_times: &times,
                        event_gaps: &gaps,
                        start: BookStart::FromSnapshot,
                        end: None,
                    },
                )
                .unwrap(),
            )
        })
    });
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
