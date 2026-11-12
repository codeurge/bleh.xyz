//! Synthetic command streams with known outcomes.

use std::collections::BTreeMap;

use mesh_types::{AggressorSide, BookSide, InstrumentId, Record};

use super::cmds::{
    Action, CancelWire, CloidCancelWire, ModifyWire, OrderRef, OrderType, OrderWire, ReplayBlock,
    SignedAction, Tif,
};
use super::matcher::{AssetSpec, BookChange, MatchEvent, Matcher};
use super::*;

const BTC: u32 = 0;

fn assets() -> BTreeMap<u32, AssetSpec> {
    let mut m = BTreeMap::new();
    m.insert(
        BTC,
        AssetSpec {
            instrument: InstrumentId::new("hyperliquid:perp:BTC").unwrap(),
            price_scale: 1,
            size_scale: 2,
        },
    );
    m
}

fn matcher() -> Matcher {
    Matcher::new(assets(), 100)
}

fn order(b: bool, p: &str, s: &str, tif: Tif) -> OrderWire {
    OrderWire {
        a: BTC,
        b,
        p: p.into(),
        s: s.into(),
        r: false,
        t: OrderType::Limit { tif },
        c: None,
    }
}

fn place(account: u64, o: OrderWire) -> SignedAction {
    SignedAction {
        account,
        action: Action::Order {
            orders: vec![o],
            grouping: None,
            oids: vec![],
        },
    }
}

fn block(n: u64, actions: Vec<SignedAction>) -> ReplayBlock {
    ReplayBlock {
        block_number: n,
        block_time_ms: 1_753_606_800_000 + n as i64 * 100,
        actions,
    }
}

fn book(oid: u64, side: BookSide, px: i64, change: BookChange) -> MatchEvent {
    MatchEvent::Book {
        asset: BTC,
        oid,
        side,
        px,
        change,
    }
}

fn trades(ev: &[MatchEvent]) -> Vec<(i64, i64, AggressorSide, u64, u64)> {
    ev.iter()
        .filter_map(|e| match *e {
            MatchEvent::Trade {
                px,
                sz,
                aggressor,
                maker_oid,
                taker_oid,
                ..
            } => Some((px, sz, aggressor, maker_oid, taker_oid)),
            MatchEvent::Book { .. } => None,
        })
        .collect()
}

#[test]
fn resting_orders_build_a_book() {
    let mut m = matcher();
    let ev = m.apply_block(&block(
        1,
        vec![
            place(1, order(true, "100.0", "1", Tif::Gtc)),
            place(2, order(true, "99.5", "2", Tif::Gtc)),
            place(3, order(false, "101", "0.5", Tif::Gtc)),
        ],
    ));
    assert_eq!(
        ev,
        [
            book(100, BookSide::Bid, 1000, BookChange::Add { size: 100 }),
            book(101, BookSide::Bid, 995, BookChange::Add { size: 200 }),
            book(102, BookSide::Ask, 1010, BookChange::Add { size: 50 }),
        ]
    );
    assert_eq!(
        m.l2(BTC),
        (vec![(1000, 100, 1), (995, 200, 1)], vec![(1010, 50, 1)])
    );
}

#[test]
fn price_then_time_priority() {
    let mut m = matcher();
    m.apply_block(&block(
        1,
        vec![
            place(1, order(false, "101", "1", Tif::Gtc)), // oid 100
            place(2, order(false, "100", "1", Tif::Gtc)), // oid 101
            place(3, order(false, "100", "1", Tif::Gtc)), // oid 102, behind 101
        ],
    ));
    let ev = m.apply_block(&block(
        2,
        vec![place(4, order(true, "101", "2.5", Tif::Gtc))],
    ));
    assert_eq!(
        trades(&ev),
        [
            (1000, 100, AggressorSide::Buy, 101, 103),
            (1000, 100, AggressorSide::Buy, 102, 103),
            (1010, 50, AggressorSide::Buy, 100, 103),
        ]
    );
    assert_eq!(
        ev[1],
        book(
            101,
            BookSide::Ask,
            1000,
            BookChange::Fill { orig: 100, new: 0 }
        )
    );
    assert_eq!(
        ev[5],
        book(
            100,
            BookSide::Ask,
            1010,
            BookChange::Fill { orig: 100, new: 50 }
        )
    );
    // Fully filled: the taker never rests.
    assert_eq!(ev.len(), 6);
    assert_eq!(m.l2(BTC), (vec![], vec![(1010, 50, 1)]));
}

#[test]
fn ioc_remainder_is_dropped_and_gtc_remainder_rests() {
    let mut m = matcher();
    m.apply_block(&block(
        1,
        vec![place(1, order(false, "100", "1", Tif::Gtc))],
    ));
    let ev = m.apply_block(&block(2, vec![place(2, order(true, "100", "3", Tif::Ioc))]));
    assert_eq!(trades(&ev).len(), 1);
    assert_eq!(ev.len(), 2);
    assert_eq!(m.stats.ioc_remainders, 1);
    assert_eq!(m.l2(BTC), (vec![], vec![]));

    m.apply_block(&block(
        3,
        vec![place(1, order(false, "100", "1", Tif::Gtc))],
    ));
    let ev = m.apply_block(&block(4, vec![place(2, order(true, "100", "3", Tif::Gtc))]));
    assert_eq!(
        ev.last(),
        Some(&book(
            103,
            BookSide::Bid,
            1000,
            BookChange::Add { size: 200 }
        ))
    );
}

#[test]
fn post_only_never_takes() {
    let mut m = matcher();
    m.apply_block(&block(
        1,
        vec![place(1, order(false, "100", "1", Tif::Gtc))],
    ));
    let ev = m.apply_block(&block(
        2,
        vec![
            place(2, order(true, "100", "1", Tif::Alo)),
            place(2, order(true, "99.9", "1", Tif::Alo)),
        ],
    ));
    assert_eq!(m.stats.alo_rejected, 1);
    assert_eq!(
        ev,
        [book(101, BookSide::Bid, 999, BookChange::Add { size: 100 })]
    );
}

#[test]
fn cancels_by_oid_and_cloid() {
    let mut m = matcher();
    let mut with_cloid = order(true, "99", "1", Tif::Gtc);
    with_cloid.c = Some("0xabc".into());
    m.apply_block(&block(
        1,
        vec![
            place(1, order(true, "100", "1", Tif::Gtc)),
            place(1, with_cloid),
        ],
    ));
    let ev = m.apply_block(&block(
        2,
        vec![
            SignedAction {
                account: 1,
                action: Action::Cancel {
                    cancels: vec![CancelWire { a: BTC, o: 100 }, CancelWire { a: BTC, o: 999 }],
                },
            },
            // Another account cannot cancel by this client order ID.
            SignedAction {
                account: 2,
                action: Action::CancelByCloid {
                    cancels: vec![CloidCancelWire {
                        asset: BTC,
                        cloid: "0xabc".into(),
                    }],
                },
            },
            SignedAction {
                account: 1,
                action: Action::CancelByCloid {
                    cancels: vec![CloidCancelWire {
                        asset: BTC,
                        cloid: "0xabc".into(),
                    }],
                },
            },
        ],
    ));
    assert_eq!(
        ev,
        [
            book(100, BookSide::Bid, 1000, BookChange::Cancel),
            book(101, BookSide::Bid, 990, BookChange::Cancel),
        ]
    );
    assert_eq!(m.stats.cancel_missing, 2);
    assert_eq!(m.l2(BTC), (vec![], vec![]));
}

#[test]
fn modify_replaces_and_loses_priority() {
    let mut m = matcher();
    m.apply_block(&block(
        1,
        vec![
            place(1, order(false, "100", "1", Tif::Gtc)), // 100
            place(2, order(false, "100", "1", Tif::Gtc)), // 101
            place(3, order(true, "98", "1", Tif::Gtc)),   // 102
        ],
    ));
    let ev = m.apply_block(&block(
        2,
        vec![SignedAction {
            account: 1,
            action: Action::Modify {
                oid: OrderRef::Oid(100),
                order: order(false, "100", "2", Tif::Gtc),
                new_oid: Some(500),
            },
        }],
    ));
    assert_eq!(
        ev,
        [
            book(100, BookSide::Ask, 1000, BookChange::Cancel),
            book(500, BookSide::Ask, 1000, BookChange::Add { size: 200 }),
        ]
    );
    // Order 101 is now first in the queue.
    let ev = m.apply_block(&block(3, vec![place(4, order(true, "100", "1", Tif::Ioc))]));
    assert_eq!(trades(&ev)[0].3, 101);
    // A replacement that would cross is rejected; the old order stays.
    let ev = m.apply_block(&block(
        4,
        vec![SignedAction {
            account: 3,
            action: Action::BatchModify {
                modifies: vec![ModifyWire {
                    oid: OrderRef::Oid(102),
                    order: order(true, "100", "1", Tif::Gtc),
                    new_oid: None,
                }],
            },
        }],
    ));
    assert!(ev.is_empty());
    assert_eq!(m.stats.modify_rejected, 1);
    assert_eq!(m.l2(BTC).0, vec![(980, 100, 1)]);
    // Unknown order.
    m.apply_block(&block(
        5,
        vec![SignedAction {
            account: 3,
            action: Action::Modify {
                oid: OrderRef::Cloid("0xnone".into()),
                order: order(true, "97", "1", Tif::Gtc),
                new_oid: None,
            },
        }],
    ));
    assert_eq!(m.stats.modify_missing, 1);
}

#[test]
fn self_trade_expires_the_maker() {
    let mut m = matcher();
    m.apply_block(&block(
        1,
        vec![
            place(7, order(false, "100", "1", Tif::Gtc)), // 100, same account as taker
            place(8, order(false, "100", "1", Tif::Gtc)), // 101
        ],
    ));
    let ev = m.apply_block(&block(2, vec![place(7, order(true, "100", "1", Tif::Gtc))]));
    assert_eq!(ev[0], book(100, BookSide::Ask, 1000, BookChange::Cancel));
    assert_eq!(trades(&ev), [(1000, 100, AggressorSide::Buy, 101, 102)]);
    assert_eq!(m.stats.self_trade_cancels, 1);
}

#[test]
fn response_oids_are_used_and_unsupported_inputs_counted() {
    let mut m = matcher();
    let mut trig = order(false, "90", "1", Tif::Gtc);
    trig.t = OrderType::Trigger {
        is_market: true,
        trigger_px: "91".into(),
        tpsl: "sl".into(),
    };
    let mut reduce = order(true, "50", "1", Tif::Gtc);
    reduce.r = true;
    let ev = m.apply_block(&block(
        1,
        vec![
            SignedAction {
                account: 1,
                action: Action::Order {
                    orders: vec![order(true, "60", "1", Tif::Gtc), trig, reduce],
                    grouping: Some("normalTpsl".into()),
                    oids: vec![9000, 9001, 9002],
                },
            },
            place(1, order(true, "1.25", "1", Tif::Gtc)), // too precise for scale 1
            place(1, order(true, "1", "1", Tif::Gtc)),    // generated oid after the largest given
            SignedAction {
                account: 1,
                action: Action::ScheduleCancel { time: Some(1) },
            },
        ],
    ));
    let oids: Vec<u64> = ev
        .iter()
        .filter_map(|e| match e {
            MatchEvent::Book { oid, .. } => Some(*oid),
            MatchEvent::Trade { .. } => None,
        })
        .collect();
    assert_eq!(oids, [9000, 9002, 9003]);
    let s = &m.stats;
    assert_eq!(
        (
            s.triggers_not_emulated,
            s.reduce_only_unchecked,
            s.rejected,
            s.not_emulated
        ),
        (1, 1, 1, 1)
    );
}

/// Deterministic pseudo-random stream: the book is never crossed, sizes
/// stay positive, and traded size equals the size removed by fills.
#[test]
fn invariants_hold_on_a_generated_stream() {
    let mut m = matcher();
    let mut seed: u64 = 0x5eed;
    let mut rnd = |n: u64| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let (mut traded, mut filled) = (0i64, 0i64);
    for b in 0..400u64 {
        let mut actions = Vec::new();
        for _ in 0..5 {
            let acct = rnd(6);
            let a = match rnd(10) {
                0..=5 => {
                    let tif = [Tif::Gtc, Tif::Gtc, Tif::Ioc, Tif::Alo][rnd(4) as usize];
                    let px = format!("{}.{}", 95 + rnd(10), rnd(10));
                    let sz = format!("0.{:02}", 1 + rnd(99));
                    Action::Order {
                        orders: vec![order(rnd(2) == 0, &px, &sz, tif)],
                        grouping: None,
                        oids: vec![],
                    }
                }
                6..=7 => Action::Cancel {
                    cancels: vec![CancelWire {
                        a: BTC,
                        o: 100 + rnd(b * 5 + 5),
                    }],
                },
                _ => Action::Modify {
                    oid: OrderRef::Oid(100 + rnd(b * 5 + 5)),
                    order: order(rnd(2) == 0, &format!("{}", 95 + rnd(10)), "0.5", Tif::Gtc),
                    new_oid: None,
                },
            };
            actions.push(SignedAction {
                account: acct,
                action: a,
            });
        }
        let blk = block(b + 1, actions);
        let ev = m.apply_block(&blk);
        for e in &ev {
            match *e {
                MatchEvent::Trade { sz, .. } => traded += sz,
                MatchEvent::Book {
                    change: BookChange::Fill { orig, new },
                    ..
                } => filled += orig - new,
                MatchEvent::Book { .. } => {}
            }
        }
        let (bids, asks) = m.l2(BTC);
        if let (Some(b), Some(a)) = (bids.first(), asks.first()) {
            assert!(b.0 < a.0, "crossed book after block {}", blk.block_number);
        }
        assert!(bids.iter().chain(&asks).all(|l| l.1 > 0 && l.2 > 0));
        for r in to_records(&blk, &ev, m.assets(), mesh_types::Flags::NONE).unwrap() {
            r.validate().unwrap();
            mesh_types::allowlist::check_record(&r).unwrap();
        }
        l4_snapshot(&m, BTC, &blk).unwrap().validate().unwrap();
    }
    assert_eq!(traded, filled);
    assert!(m.stats.trades > 50, "{:?}", m.stats);
}

#[test]
fn records_carry_admission_flags_and_indices() {
    let mut m = matcher();
    let b1 = block(
        1,
        vec![
            place(1, order(false, "100", "1", Tif::Gtc)),
            place(1, order(false, "101", "1", Tif::Gtc)),
        ],
    );
    let b2 = block(2, vec![place(2, order(true, "101", "2", Tif::Gtc))]);
    let (records, report) = replay(&mut m, &[b1, b2], &[], 20).unwrap();
    assert_eq!(report.records, 6);
    let idx: Vec<(&str, u64, u32)> = records
        .iter()
        .map(|r| {
            (
                r.kind().as_str(),
                r.prov().seq.block_number,
                r.prov().seq.event_index,
            )
        })
        .collect();
    assert_eq!(
        idx,
        [
            ("book_event", 1, 0),
            ("book_event", 1, 1),
            ("trade", 2, 0),
            ("book_event", 2, 0),
            ("trade", 2, 1),
            ("book_event", 2, 1),
        ]
    );
    for r in &records {
        assert_eq!(r.prov().admission.as_str(), "ADM-0003");
        assert!(r.prov().flags.contains(mesh_types::Flag::Backfill));
        assert!(r.prov().flags.contains(mesh_types::Flag::Replayed));
    }
    let Record::Trade(t) = &records[2] else {
        panic!()
    };
    assert_eq!(t.price.to_string(), "100.0");
    assert_eq!(t.size.to_string(), "1.00");
}

#[test]
fn checkpoints_against_archived_l2() {
    let mut m = matcher();
    let b1 = block(
        1,
        vec![
            place(1, order(true, "99", "1", Tif::Gtc)),
            place(2, order(true, "99", "0.5", Tif::Gtc)),
            place(3, order(false, "101", "2", Tif::Gtc)),
        ],
    );
    let good = L2Checkpoint {
        asset: BTC,
        block_number: 1,
        levels: (vec![(990, 150, 2)], vec![(1010, 200, 1)]),
    };
    let bad = L2Checkpoint {
        asset: BTC,
        block_number: 1,
        levels: (vec![(990, 150, 2)], vec![(1010, 100, 1)]),
    };
    let (_, report) = replay(&mut m, &[b1], &[good, bad], 20).unwrap();
    assert!(report.checkpoints[0].matched);
    assert!(!report.checkpoints[1].matched);
    assert_eq!(
        report.checkpoints[1].first_mismatch,
        Some(("ask".into(), 0))
    );

    // Depth: a truncated archive side compares only its levels; a short
    // archive side requires the emulated side to be equally short.
    let emu = (vec![(3, 1, 1), (2, 1, 1), (1, 1, 1)], vec![]);
    assert!(check_l2(BTC, 1, &emu, &(vec![(3, 1, 1), (2, 1, 1)], vec![]), 2).matched);
    assert!(!check_l2(BTC, 1, &emu, &(vec![(3, 1, 1), (2, 1, 1)], vec![]), 20).matched);

    // Archived snapshot records convert at the asset's scales.
    let depth = mesh_types::SnapshotDepth::L2 {
        bids: vec![mesh_types::Level {
            price: mesh_types::Price::parse_exact("99.00").unwrap(),
            size: mesh_types::Size::parse_exact("1.5").unwrap(),
            order_count: 2,
        }],
        asks: vec![],
    };
    assert_eq!(
        levels_of(&depth, &assets()[&BTC]).unwrap(),
        (vec![(990, 150, 2)], vec![])
    );
}
