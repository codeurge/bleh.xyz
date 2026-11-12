//! The committed hl-synth fixtures (`fixtures/synthetic/hl/`, synthetic,
//! no admission needed): building the books from the expected trades and
//! book events reproduces the expected L4 book at the snapshot height and
//! at the end, and the gap scenario resyncs from the snapshot.

#![allow(clippy::unwrap_used, missing_docs)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use book::processor::{BlockInput, DesyncReason, Output};
use book::{BookState, Processor, ProcessorConfig};
use mesh_types::{
    AdmissionId, AggressorSide, BookAction, BookEvent, BookSide, BookSnapshot, InstrumentId,
    OrderId, Price, Provenance, RestingOrder, Seq, Size, SnapshotDepth, Trade, TradeId, UnixMillis,
    UnixNanos, VenueId,
};
use serde_json::Value;

fn fixture_dir(scenario: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/synthetic/hl")
        .join(scenario)
}

fn block_times(scenario: &str) -> BTreeMap<u64, UnixMillis> {
    let mut out = BTreeMap::new();
    let dir = fixture_dir(scenario).join("node_raw_book_diffs_by_block/hourly/20260101");
    for hour in ["0", "1"] {
        let text = std::fs::read_to_string(dir.join(hour)).unwrap();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(line).unwrap();
            let b = v["block_number"].as_u64().unwrap();
            let t = UnixMillis::parse_rfc3339(v["block_time"].as_str().unwrap()).unwrap();
            out.insert(b, t);
        }
    }
    out
}

fn prov(instrument: &InstrumentId, seq: Seq, ts: UnixMillis) -> Provenance {
    Provenance::new(
        instrument.clone(),
        VenueId::new("hyperliquid").unwrap(),
        seq,
        ts,
        UnixNanos(ts.0 * 1_000_000),
        AdmissionId::new("ADM-0001").unwrap(),
    )
}

fn seq(v: &Value) -> Seq {
    Seq::new(
        v[0].as_u64().unwrap(),
        u32::try_from(v[1].as_u64().unwrap()).unwrap(),
    )
}

fn px(v: &Value) -> Price {
    Price::parse_exact(v.as_str().unwrap()).unwrap()
}

fn sz(v: &Value) -> Size {
    Size::parse_exact(v.as_str().unwrap()).unwrap()
}

struct Coin {
    instrument: InstrumentId,
    trades: Vec<Trade>,
    events: Vec<BookEvent>,
    at_snapshot: BookSnapshot,
    final_depth: SnapshotDepth,
}

fn orders(v: &Value) -> Vec<RestingOrder> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|o| RestingOrder {
            order_id: OrderId(o["order_id"].as_u64().unwrap()),
            price: px(&o["price"]),
            size: sz(&o["size"]),
            ts_order_ms: UnixMillis(o["ts_order_ms"].as_i64().unwrap()),
        })
        .collect()
}

fn depth(v: &Value) -> SnapshotDepth {
    SnapshotDepth::L4 {
        bids: orders(&v["bids"]),
        asks: orders(&v["asks"]),
    }
}

fn load(scenario: &str) -> (Vec<Coin>, BTreeMap<u64, UnixMillis>, Vec<u64>) {
    let times = block_times(scenario);
    let text = std::fs::read_to_string(fixture_dir(scenario).join("expected.json")).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["synthetic"], true);
    let height = v["snapshot_height"].as_u64().unwrap();
    let missing: Vec<u64> = v["missing_blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap())
        .collect();
    let mut coins = Vec::new();
    for (_, c) in v["coins"].as_object().unwrap() {
        let instrument = InstrumentId::new(c["instrument"].as_str().unwrap()).unwrap();
        let trades = c["trades"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| Trade {
                prov: prov(
                    &instrument,
                    seq(&t["seq"]),
                    UnixMillis(t["ts_source_ms"].as_i64().unwrap()),
                ),
                price: px(&t["price"]),
                size: sz(&t["size"]),
                aggressor: match t["aggressor"].as_str().unwrap() {
                    "buy" => AggressorSide::Buy,
                    "sell" => AggressorSide::Sell,
                    _ => AggressorSide::Unknown,
                },
                trade_id: TradeId(t["trade_id"].as_u64().unwrap()),
            })
            .collect();
        let events = c["book_events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let s = seq(&e["seq"]);
                let orig = e.get("orig_size").map(sz);
                BookEvent {
                    prov: prov(&instrument, s, times[&s.block_number]),
                    order_id: OrderId(e["order_id"].as_u64().unwrap()),
                    side: if e["side"] == "bid" {
                        BookSide::Bid
                    } else {
                        BookSide::Ask
                    },
                    price: px(&e["price"]),
                    action: match e["action"].as_str().unwrap() {
                        "add" => BookAction::Add {
                            size: sz(&e["size"]),
                        },
                        "modify" => BookAction::Modify {
                            orig_size: orig,
                            new_size: sz(&e["new_size"]),
                        },
                        "fill" => BookAction::Fill {
                            orig_size: orig,
                            new_size: sz(&e["new_size"]),
                        },
                        "cancel" => BookAction::Cancel {},
                        other => panic!("action {other}"),
                    },
                }
            })
            .collect();
        let mut sp = prov(&instrument, Seq::end_of_block(height), times[&height]);
        sp.seq = Seq::end_of_block(height);
        coins.push(Coin {
            at_snapshot: BookSnapshot {
                prov: sp,
                depth: depth(&c["book_at_snapshot"]),
            },
            final_depth: depth(&c["final_book"]),
            instrument,
            trades,
            events,
        });
    }
    (coins, times, missing)
}

/// Feeds a coin block by block, the snapshot after its height, and
/// returns the processor and its outputs.
fn drive(
    coin: &Coin,
    times: &BTreeMap<u64, UnixMillis>,
    missing: &[u64],
) -> (Processor, Vec<Output>) {
    let mut p =
        Processor::with_empty_book(coin.instrument.clone(), ProcessorConfig::default()).unwrap();
    let mut out = Vec::new();
    let height = coin.at_snapshot.prov.seq.block_number;
    let mut snapshot_done = false;
    let mut gap_pending = false;
    let mut prev = 0;
    for (&b, &t) in times {
        if !snapshot_done && b > height {
            out.extend(p.process_snapshot(&coin.at_snapshot).unwrap());
            snapshot_done = true;
        }
        // A missing block is absent from the raw files too.
        if missing.iter().any(|m| *m < b && *m > prev) {
            gap_pending = true;
        }
        prev = b;
        let trades: Vec<Trade> = coin
            .trades
            .iter()
            .filter(|x| x.prov.seq.block_number == b)
            .cloned()
            .collect();
        let events: Vec<BookEvent> = coin
            .events
            .iter()
            .filter(|x| x.prov.seq.block_number == b)
            .cloned()
            .collect();
        if trades.is_empty() && events.is_empty() {
            continue;
        }
        out.extend(
            p.process_block(BlockInput {
                block_number: b,
                block_time: t,
                trades: &trades,
                events: &events,
                events_gap_before: gap_pending && !events.is_empty(),
            })
            .unwrap(),
        );
        if !events.is_empty() {
            gap_pending = false;
        }
    }
    out.extend(p.finish().unwrap());
    (p, out)
}

#[test]
fn base_books_match_expected() {
    let (coins, times, missing) = load("base");
    assert!(missing.is_empty());
    assert_eq!(coins.len(), 3);
    for coin in &coins {
        let (p, out) = drive(coin, &times, &missing);
        assert!(
            !out.iter().any(|o| matches!(o, Output::Desync(_))),
            "{}: {out:?}",
            coin.instrument
        );
        let reports: Vec<_> = out
            .iter()
            .filter_map(|o| match o {
                Output::Reconciliation(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(reports.len(), 1);
        assert!(
            reports[0].is_clean(),
            "{}: {:?}",
            coin.instrument,
            reports[0]
        );
        let book = p.book().unwrap();
        let fin = book.to_snapshot(coin.at_snapshot.prov.clone()).unwrap();
        assert_eq!(fin.depth, coin.final_depth, "{}", coin.instrument);
        // Bars and checkpoints were produced and validate.
        assert!(out.iter().any(|o| matches!(o, Output::Bar(_))));
        for o in &out {
            if let Some(r) = o.record() {
                r.validate().unwrap();
            }
        }
    }
}

#[test]
fn gap_scenario_resyncs_from_snapshot() {
    let (coins, times, missing) = load("gap");
    assert_eq!(missing, [1_000_015]);
    let mut resynced = 0;
    for coin in &coins {
        let (p, out) = drive(coin, &times, &missing);
        let had_events_after_gap = coin
            .events
            .iter()
            .any(|e| e.prov.seq.block_number > missing[0]);
        if had_events_after_gap {
            assert!(
                out.iter().any(|o| matches!(
                    o,
                    Output::Desync(d) if d.reason == DesyncReason::Gap
                )),
                "{}",
                coin.instrument
            );
        }
        let report = out
            .iter()
            .find_map(|o| match o {
                Output::Reconciliation(r) => Some(r),
                _ => None,
            })
            .unwrap();
        if !report.is_clean() {
            resynced += 1;
        }
        assert_eq!(p.state(), BookState::InSync);
        let fin = p
            .book()
            .unwrap()
            .to_snapshot(coin.at_snapshot.prov.clone())
            .unwrap();
        assert_eq!(fin.depth, coin.final_depth, "{}", coin.instrument);
    }
    assert!(resynced > 0, "the missing block changed no book");
}
