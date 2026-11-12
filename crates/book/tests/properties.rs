//! Property tests: book invariants and bar rollup equivalence.

#![allow(clippy::unwrap_used, missing_docs)]

mod common;

use std::collections::BTreeMap;

use book::bars::{ActivitySpec, BarEngine, BarEngineConfig, activity_bars, rollup, time_bars};
use book::inputs::{TradeIn, canonical_trade};
use book::orderflow::{ProfileConfig, footprints, value_area, volume_profiles};
use book::reconcile::{ReconcileOptions, reconcile};
use book::{BookError, OrderBook};
use common::*;
use mesh_types::{
    AggressorSide, BarInterval, BookAction, BookEvent, BookSide, OrderId, RestingOrder, Seq,
    SnapshotDepth, Trade, TradeId, UnixMillis,
};
use proptest::prelude::*;

// ----- book invariants -----

#[derive(Debug, Clone)]
struct Op {
    kind: u8,
    bid: bool,
    off: i64,
    size: i64,
    pick: usize,
    new_block: bool,
}

fn op() -> impl Strategy<Value = Op> {
    (
        0u8..100,
        any::<bool>(),
        1i64..30,
        1i64..5_000,
        any::<usize>(),
        any::<bool>(),
    )
        .prop_map(|(kind, bid, off, size, pick, new_block)| Op {
            kind,
            bid,
            off,
            size,
            pick,
            new_block,
        })
}

/// Reference model: orders and per-price FIFO queues.
#[derive(Default)]
struct Model {
    orders: BTreeMap<u64, (BookSide, i64, i64, i64)>,
    queues: BTreeMap<(bool, i64), Vec<u64>>,
}

impl Model {
    fn best(&self, side: BookSide) -> Option<i64> {
        let bid = side == BookSide::Bid;
        let ps = self
            .queues
            .keys()
            .filter(|(b, _)| *b == bid)
            .map(|(_, p)| *p);
        if bid { ps.max() } else { ps.min() }
    }

    fn remove(&mut self, oid: u64) {
        if let Some((side, p, _, _)) = self.orders.remove(&oid) {
            let key = (side == BookSide::Bid, p);
            if let Some(q) = self.queues.get_mut(&key) {
                q.retain(|x| *x != oid);
                if q.is_empty() {
                    self.queues.remove(&key);
                }
            }
        }
    }

    fn depth(&self) -> SnapshotDepth {
        let side = |bid: bool| -> Vec<RestingOrder> {
            let mut keys: Vec<_> = self.queues.keys().filter(|(b, _)| *b == bid).collect();
            if bid {
                keys.reverse();
            }
            keys.into_iter()
                .flat_map(|k| self.queues[k].iter())
                .map(|oid| {
                    let (_, p, s, t) = self.orders[oid];
                    RestingOrder {
                        order_id: OrderId(*oid),
                        price: price(p),
                        size: size(s),
                        ts_order_ms: UnixMillis(t),
                    }
                })
                .collect()
        };
        SnapshotDepth::L4 {
            bids: side(true),
            asks: side(false),
        }
    }
}

fn ev(block: u64, idx: u32, oid: u64, side: BookSide, p: i64, action: BookAction) -> BookEvent {
    BookEvent {
        prov: prov(block, idx, BASE_MS + i64::try_from(block).unwrap() * 1_000),
        order_id: OrderId(oid),
        side,
        price: price(p),
        action,
    }
}

fn check_invariants(book: &OrderBook, model: &Model, block: u64) {
    assert_eq!(book.len(), model.orders.len());
    let mut eob = prov(block, 0, BASE_MS);
    eob.seq = Seq::end_of_block(block);
    let snap = book.to_snapshot(eob).unwrap();
    assert_eq!(snap.depth, model.depth());
    // Levels equal the aggregation of the orders.
    let SnapshotDepth::L2 { bids, asks } = snap.depth.to_l2().unwrap() else {
        panic!()
    };
    assert_eq!(bids, book.levels(BookSide::Bid, usize::MAX).unwrap());
    assert_eq!(asks, book.levels(BookSide::Ask, usize::MAX).unwrap());
    assert_eq!(book.top().bid, bids.first().copied());
    assert_eq!(book.top().ask, asks.first().copied());
    book.check_crossed().unwrap();
    snap.validate().unwrap();
    // Round trip and self-reconciliation.
    let back = OrderBook::from_snapshot(&snap).unwrap();
    assert_eq!(back.to_snapshot(snap.prov.clone()).unwrap(), snap);
    assert!(
        reconcile(book, &snap, ReconcileOptions::default())
            .unwrap()
            .is_clean()
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn book_matches_model_and_rejects_bad_events(ops in prop::collection::vec(op(), 1..300)) {
        let mut book = OrderBook::new(inst());
        let mut model = Model::default();
        let mut block = 1u64;
        let mut idx = 0u32;
        let mut next_oid = 1u64;
        for o in ops {
            if o.new_block {
                book.advance(Seq::end_of_block(block), UnixMillis(BASE_MS));
                check_invariants(&book, &model, block);
                block += 1;
                idx = 0;
            }
            let side = if o.bid { BookSide::Bid } else { BookSide::Ask };
            let pick = (!model.orders.is_empty())
                .then(|| *model.orders.keys().nth(o.pick % model.orders.len()).unwrap());
            let e = match (o.kind, pick) {
                (0..=44, _) | (_, None) => {
                    // Add without crossing.
                    let p = match side {
                        BookSide::Bid => {
                            let p = 10_000 - o.off;
                            model.best(BookSide::Ask).map_or(p, |a| p.min(a - 1))
                        }
                        BookSide::Ask => {
                            let p = 10_000 + o.off;
                            model.best(BookSide::Bid).map_or(p, |b| p.max(b + 1))
                        }
                    };
                    let oid = next_oid;
                    next_oid += 1;
                    let e = ev(block, idx, oid, side, p, BookAction::Add { size: size(o.size) });
                    model.orders.insert(oid, (side, p, o.size, BASE_MS + i64::try_from(block).unwrap() * 1_000));
                    model.queues.entry((side == BookSide::Bid, p)).or_default().push(oid);
                    Some(e)
                }
                (45..=59, Some(oid)) => {
                    let (s, p, _, _) = model.orders[&oid];
                    model.remove(oid);
                    Some(ev(block, idx, oid, s, p, BookAction::Cancel {}))
                }
                (60..=69, Some(oid)) => {
                    let (s, p, cur, _) = model.orders[&oid];
                    let new = 1 + o.size % 9_999;
                    model.orders.get_mut(&oid).unwrap().2 = new;
                    Some(ev(block, idx, oid, s, p, BookAction::Modify { orig_size: Some(size(cur)), new_size: size(new) }))
                }
                (70..=84, Some(oid)) => {
                    let (s, p, cur, _) = model.orders[&oid];
                    let new = if cur > 1 { o.size % cur } else { 0 };
                    let orig = (o.size % 2 == 0).then(|| size(cur));
                    if new == 0 {
                        model.remove(oid);
                    } else {
                        model.orders.get_mut(&oid).unwrap().2 = new;
                    }
                    Some(ev(block, idx, oid, s, p, BookAction::Fill { orig_size: orig, new_size: size(new) }))
                }
                (_, Some(oid)) => {
                    // Deliberately inconsistent events: typed error, no change.
                    let (s, p, cur, _) = model.orders[&oid];
                    let other = if s == BookSide::Bid { BookSide::Ask } else { BookSide::Bid };
                    let bad = match o.kind % 6 {
                        0 => ev(block, idx, u64::MAX - 1, s, p, BookAction::Cancel {}),
                        1 => ev(block, idx, oid, other, p, BookAction::Cancel {}),
                        2 => ev(block, idx, oid, s, p + 1, BookAction::Cancel {}),
                        3 => ev(block, idx, oid, s, p, BookAction::Fill { orig_size: None, new_size: size(cur) }),
                        4 => ev(block, idx, oid, s, p, BookAction::Modify { orig_size: Some(size(cur + 1)), new_size: size(1) }),
                        _ => ev(block, idx, oid, s, p, BookAction::Add { size: size(1) }),
                    };
                    let before = book.clone();
                    let err = book.apply(&bad).unwrap_err();
                    let typed = matches!(
                        err,
                        BookError::UnknownOrder { .. }
                            | BookError::SideMismatch { .. }
                            | BookError::PriceMismatch { .. }
                            | BookError::NonReducingFill { .. }
                            | BookError::OrigSizeMismatch { .. }
                            | BookError::DuplicateOrder { .. }
                    );
                    prop_assert!(typed, "unexpected error {:?}", err);
                    prop_assert_eq!(&book, &before);
                    None
                }
            };
            if let Some(e) = e {
                book.apply(&e).unwrap();
                idx += 1;
            }
        }
        book.advance(Seq::end_of_block(block), UnixMillis(BASE_MS));
        check_invariants(&book, &model, block);
    }
}

// ----- bars -----

fn trades_strategy() -> impl Strategy<Value = Vec<Trade>> {
    prop::collection::vec((0i64..400_000, 1i64..2_000, 1i64..1_000, 0u8..3), 1..200).prop_map(
        |raw| {
            // Start 30 minutes before a UTC midnight so daily bars roll.
            let mut t = BASE_MS - 1_800_000;
            raw.into_iter()
                .enumerate()
                .map(|(i, (dt, p, s, a))| {
                    t += dt;
                    let i = u32::try_from(i).unwrap();
                    Trade {
                        prov: prov(u64::from(i) + 1, 0, t),
                        price: price(p),
                        size: size(s),
                        aggressor: match a {
                            0 => AggressorSide::Buy,
                            1 => AggressorSide::Sell,
                            _ => AggressorSide::Unknown,
                        },
                        trade_id: TradeId(u64::from(i)),
                    }
                })
                .collect()
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn rollups_equal_direct_computation(trades in trades_strategy()) {
        let m1 = time_bars(&trades, BarInterval::M1).unwrap();
        for target in [BarInterval::M5, BarInterval::M15, BarInterval::H1, BarInterval::D1] {
            let direct = time_bars(&trades, target).unwrap();
            prop_assert_eq!(&rollup(&m1, target, &trades).unwrap(), &direct);
        }
        // Nested rollups too.
        let m5 = time_bars(&trades, BarInterval::M5).unwrap();
        prop_assert_eq!(rollup(&m5, BarInterval::M15, &trades).unwrap(), time_bars(&trades, BarInterval::M15).unwrap());
        let h1 = time_bars(&trades, BarInterval::H1).unwrap();
        prop_assert_eq!(rollup(&h1, BarInterval::D1, &trades).unwrap(), time_bars(&trades, BarInterval::D1).unwrap());
        // The streaming engine agrees.
        let mut engine = BarEngine::new(&BarEngineConfig::default()).unwrap();
        let mut out = Vec::new();
        for t in &trades {
            let c = canonical_trade(t).unwrap();
            engine.push(TradeIn { trade: t, canonical: &c }, &mut out).unwrap();
        }
        engine.finish(&mut out).unwrap();
        for i in BarInterval::ALL {
            let mut got: Vec<_> = out.iter().filter(|b| b.interval == i).cloned().collect();
            got.sort_by_key(|b| b.start_ms);
            prop_assert_eq!(got, time_bars(&trades, i).unwrap());
        }
    }

    #[test]
    fn bars_conserve_volume_and_validate(trades in trades_strategy()) {
        let total: i64 = trades.iter().map(|t| t.size.mantissa()).sum();
        let buys: i64 = trades.iter().filter(|t| t.aggressor == AggressorSide::Buy).map(|t| t.size.mantissa()).sum();
        for i in BarInterval::ALL {
            let bars = time_bars(&trades, i).unwrap();
            for b in &bars {
                b.validate().unwrap();
            }
            prop_assert_eq!(bars.iter().map(|b| b.volume.mantissa()).sum::<i64>(), total);
            prop_assert_eq!(bars.iter().map(|b| b.buy_volume.mantissa()).sum::<i64>(), buys);
            prop_assert_eq!(bars.iter().map(|b| u64::from(b.trade_count)).sum::<u64>(), trades.len() as u64);
            prop_assert!(bars.windows(2).all(|w| w[0].start_ms < w[1].start_ms));
        }
        let fp = footprints(&trades, BarInterval::M1).unwrap();
        prop_assert_eq!(fp.iter().flat_map(|f| f.levels.iter()).map(|l| l.ask_volume.mantissa()).sum::<i64>(), buys);
        for f in &fp {
            f.validate().unwrap();
        }
    }

    #[test]
    fn activity_bars_partition_the_tape(trades in trades_strategy(), n in 1u32..20, w in 1i64..300, v in 1i64..5_000) {
        for spec in [
            ActivitySpec::Tick { trades: n },
            ActivitySpec::Range { width: price(w) },
            ActivitySpec::Volume { size: size(v) },
        ] {
            let bars = activity_bars(&trades, spec, BarInterval::D1).unwrap();
            prop_assert_eq!(bars.iter().map(|b| u64::from(b.trade_count)).sum::<u64>(), trades.len() as u64);
            let mut i = 0usize;
            for b in &bars {
                prop_assert_eq!(b.first_seq, trades[i].prov.seq);
                i += b.trade_count as usize;
                prop_assert_eq!(b.prov.seq, trades[i - 1].prov.seq);
                match spec {
                    ActivitySpec::Tick { trades: n } => prop_assert!(b.trade_count == n || !b.complete),
                    ActivitySpec::Range { width } => prop_assert!(b.high.mantissa() - b.low.mantissa() <= width.mantissa()),
                    ActivitySpec::Volume { size } => prop_assert!(b.volume >= size || !b.complete),
                }
            }
        }
    }

    #[test]
    fn value_area_contains_poc_and_target(trades in trades_strategy(), bps in 0u32..=10_000) {
        let profiles = volume_profiles(&trades, ProfileConfig { value_area_bps: bps, bucket: Some(price(50)), ..ProfileConfig::default() }).unwrap();
        for (p, s) in &profiles {
            p.validate().unwrap();
            let va = value_area(&p.levels, bps).unwrap().unwrap();
            let s = s.as_ref().unwrap();
            prop_assert_eq!(s.poc, va.poc);
            prop_assert!(va.low <= va.poc && va.poc <= va.high);
            let total = i128::from(va.total.mantissa());
            let target = (total * i128::from(bps) + 9_999) / 10_000;
            prop_assert!(i128::from(va.volume.mantissa()) >= target);
            prop_assert!(va.volume <= va.total);
            let max = p.levels.iter().map(|l| l.volume).max().unwrap();
            prop_assert_eq!(p.levels.iter().find(|l| l.volume == max).unwrap().price, va.poc);
        }
    }
}

// ----- canonical encoding -----

fn any_prov() -> impl Strategy<Value = mesh_types::Provenance> {
    (
        any::<u64>(),
        0u32..u32::MAX,
        -9_000_000_000_000i64..9_000_000_000_000,
        any::<i64>(),
        0u32..64,
        any::<bool>(),
    )
        .prop_map(|(block, idx, ts, ingest, flags, eob)| {
            let mut p = prov(block, idx, ts);
            p.ts_ingest_ns = mesh_types::UnixNanos(ingest);
            p.flags = mesh_types::Flags::from_bits(flags).unwrap();
            if eob {
                p.seq = Seq::end_of_block(block);
                p.stamp_publish(mesh_types::UnixNanos(1));
            }
            p
        })
}

fn any_price() -> impl Strategy<Value = mesh_types::Price> {
    (any::<i64>(), 0u8..=18).prop_map(|(m, s)| mesh_types::Price::new(m, s).unwrap())
}

fn any_size() -> impl Strategy<Value = mesh_types::Size> {
    (0i64..=i64::MAX, 0u8..=18).prop_map(|(m, s)| mesh_types::Size::new(m, s).unwrap())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn fast_canonical_encoding_equals_mesh_types(
        p in any_prov(),
        price in any_price(),
        a in any_size(),
        b in prop::option::of(any_size()),
        kind in 0u8..4,
        id in any::<u64>(),
        aggr in 0u8..3,
        orders in prop::collection::vec((any::<u64>(), any_price(), any_size(), any::<i64>()), 0..5),
    ) {
        use mesh_types::hash::canonical_record;
        use mesh_types::{Level, Record, RestingOrder};
        let t = Trade {
            prov: p.clone(),
            price,
            size: a,
            aggressor: [AggressorSide::Buy, AggressorSide::Sell, AggressorSide::Unknown][aggr as usize],
            trade_id: TradeId(id),
        };
        prop_assert_eq!(book::inputs::canonical_trade(&t).unwrap(), canonical_record(&Record::Trade(t)).unwrap());
        let action = match kind {
            0 => BookAction::Add { size: a },
            1 => BookAction::Modify { orig_size: b, new_size: a },
            2 => BookAction::Fill { orig_size: b, new_size: a },
            _ => BookAction::Cancel {},
        };
        let e = BookEvent { prov: p.clone(), order_id: OrderId(id), side: if aggr == 0 { BookSide::Bid } else { BookSide::Ask }, price, action };
        prop_assert_eq!(book::inputs::canonical_event(&e).unwrap(), canonical_record(&Record::BookEvent(e)).unwrap());
        let ro: Vec<RestingOrder> = orders.iter().map(|(i, p, s, t)| RestingOrder { order_id: OrderId(*i), price: *p, size: *s, ts_order_ms: UnixMillis(*t) }).collect();
        let lv: Vec<Level> = orders.iter().map(|(i, p, s, _)| Level { price: *p, size: *s, order_count: (*i % 1000) as u32 }).collect();
        for depth in [
            SnapshotDepth::L4 { bids: ro.clone(), asks: ro.iter().rev().copied().collect() },
            SnapshotDepth::L2 { bids: lv.clone(), asks: lv.iter().rev().copied().collect() },
        ] {
            let s = mesh_types::BookSnapshot { prov: p.clone(), depth };
            prop_assert_eq!(book::inputs::canonical_snapshot(&s).unwrap(), canonical_record(&Record::BookSnapshot(s)).unwrap());
        }
    }
}
