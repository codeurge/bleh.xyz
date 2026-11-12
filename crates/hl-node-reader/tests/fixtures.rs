//! End-to-end checks against the synthetic fixtures in
//! `fixtures/synthetic/hl/` and against further scenarios generated with
//! `hl-synth`: records match `expected.json`, every batch is valid and
//! allowlisted, and filtered raw parses to exactly the same records.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hl_node_reader::files::parse_rel;
use hl_node_reader::filter::{StreamDirs, filter_line, filter_raw_line};
use hl_node_reader::raw::Stream;
use hl_node_reader::replay::{Replay, sources_from_texts};
use hl_node_reader::sequence::BlockOutput;
use hl_node_reader::snapshot::{filter_l4_snapshot, parse_l4_snapshot};
use mesh_types::allowlist::{EXCLUDED_SOURCE_FIELDS, check_batch};
use mesh_types::{
    BookAction, Flag, Record, RecordKind, Registry, SnapshotDepth, UnixMillis, UnixNanos,
};
use proptest::prelude::*;
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/hl")
}

fn fixture_registry() -> Registry {
    serde_json::from_slice(&std::fs::read(root().join("markets.json")).unwrap()).unwrap()
}

/// Per-stream texts of a scenario from a generated file set.
fn texts_from_files(files: &BTreeMap<String, Vec<u8>>, scenario: &str) -> [String; 3] {
    let dirs = StreamDirs::default();
    Stream::ALL.map(|s| {
        let prefix = format!("{scenario}/{}/", dirs.get(s));
        let mut parts: Vec<((u32, u8), String)> = files
            .iter()
            .filter_map(|(k, v)| {
                let rel = k.strip_prefix(&prefix)?;
                Some((parse_rel(rel)?, String::from_utf8(v.clone()).unwrap()))
            })
            .collect();
        parts.sort();
        parts.into_iter().map(|(_, t)| t).collect()
    })
}

fn texts_from_dir(dir: &Path) -> [String; 3] {
    let dirs = StreamDirs::default();
    Stream::ALL.map(|s| {
        hl_node_reader::files::list_hourly(&dir.join(dirs.get(s)))
            .unwrap()
            .into_iter()
            .map(|f| std::fs::read_to_string(f.path).unwrap())
            .collect()
    })
}

fn filter_texts(texts: &[String; 3]) -> [String; 3] {
    let mut i = 0;
    texts.clone().map(|t| {
        let s = Stream::ALL[i];
        i += 1;
        t.lines()
            .map(|l| filter_line(s, l).unwrap() + "\n")
            .collect()
    })
}

fn replay(texts: [String; 3], registry: &Registry) -> Vec<BlockOutput> {
    Replay::new(sources_from_texts(texts), registry.clone())
        .sequenced()
        .map(Result::unwrap)
        .collect()
}

/// A record in the `expected.json` shape.
fn expected_shape(r: &Record) -> Value {
    match r {
        Record::Trade(t) => json!({
            "seq": [t.prov.seq.block_number, t.prov.seq.event_index],
            "ts_source_ms": t.prov.ts_source_ms.0,
            "trade_id": t.trade_id.0,
            "price": t.price.to_string(),
            "size": t.size.to_string(),
            "aggressor": t.aggressor.as_str(),
        }),
        Record::BookEvent(e) => {
            let mut v = json!({
                "seq": [e.prov.seq.block_number, e.prov.seq.event_index],
                "order_id": e.order_id.0,
                "side": e.side.as_str(),
                "price": e.price.to_string(),
                "action": e.action.as_str(),
            });
            match e.action {
                BookAction::Add { size } => v["size"] = json!(size.to_string()),
                BookAction::Modify {
                    orig_size,
                    new_size,
                }
                | BookAction::Fill {
                    orig_size,
                    new_size,
                } => {
                    if let Some(o) = orig_size {
                        v["orig_size"] = json!(o.to_string());
                    }
                    v["new_size"] = json!(new_size.to_string());
                }
                BookAction::Cancel {} => {}
            }
            v
        }
        _ => panic!("unexpected kind"),
    }
}

/// Checks outputs against an `expected.json` value; returns the blocks
/// whose outputs were flagged `gap_before`.
fn check_against_expected(
    outputs: &[BlockOutput],
    expected: &Value,
    registry: &Registry,
) -> Vec<u64> {
    let mut by_coin: BTreeMap<String, (Vec<Value>, Vec<Value>)> = BTreeMap::new();
    let mut last_block: BTreeMap<(String, RecordKind), u64> = BTreeMap::new();
    let mut gaps = Vec::new();
    for o in outputs {
        if o.gap_before {
            gaps.push(o.block_number);
        }
        for b in &o.batches {
            b.validate().unwrap();
            check_batch(b).unwrap();
            let key = (b.instrument.to_string(), b.kind);
            assert_eq!(
                b.prev_block_number,
                last_block.get(&key).copied(),
                "{key:?}"
            );
            last_block.insert(key, b.block_number);
            let spec = registry.get(&b.instrument).unwrap();
            let entry = by_coin.entry(spec.venue_symbol.clone()).or_default();
            for r in &b.records {
                assert_eq!(r.prov().admission.as_str(), "ADM-0001");
                assert_eq!(
                    r.prov().flags.contains(Flag::OracleAnchoredVenue),
                    spec.flags.contains(Flag::OracleAnchoredVenue)
                );
                match b.kind {
                    RecordKind::Trade => entry.0.push(expected_shape(r)),
                    _ => entry.1.push(expected_shape(r)),
                }
            }
        }
    }
    let coins = expected["coins"].as_object().unwrap();
    assert_eq!(coins.len(), by_coin.len());
    for (coin, c) in coins {
        let (trades, events) = &by_coin[coin];
        assert_eq!(
            trades.len(),
            c["trades"].as_array().unwrap().len(),
            "{coin} trades"
        );
        for (got, want) in trades.iter().zip(c["trades"].as_array().unwrap()) {
            assert_eq!(got, want, "{coin} trade");
        }
        assert_eq!(
            events.len(),
            c["book_events"].as_array().unwrap().len(),
            "{coin} events"
        );
        for (got, want) in events.iter().zip(c["book_events"].as_array().unwrap()) {
            assert_eq!(got, want, "{coin} book event");
        }
    }
    gaps
}

fn records(outputs: &[BlockOutput]) -> Vec<(u64, Vec<mesh_types::BlockBatch>)> {
    outputs
        .iter()
        .map(|o| (o.block_number, o.batches.clone()))
        .collect()
}

fn assert_no_excluded_keys(text: &str) {
    for line in text.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        walk(&v, &mut |k| {
            assert!(
                !EXCLUDED_SOURCE_FIELDS.contains(&k),
                "excluded key {k} in filtered raw"
            );
            assert!(
                !["cloid", "tif", "orderType", "children", "triggerPx"].contains(&k),
                "unused key {k} in filtered raw"
            );
        });
    }
}

fn walk(v: &Value, f: &mut impl FnMut(&str)) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                f(k);
                walk(v, f);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| walk(v, f)),
        _ => {}
    }
}

#[test]
fn base_fixture_matches_expected() {
    let registry = fixture_registry();
    let dir = root().join("base");
    let expected: Value =
        serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
    let outputs: Vec<BlockOutput> = Replay::open(&dir, &StreamDirs::default(), registry.clone())
        .unwrap()
        .sequenced()
        .map(Result::unwrap)
        .collect();
    assert_eq!(outputs.first().unwrap().block_number, 1_000_000);
    assert_eq!(outputs.last().unwrap().block_number, 1_000_059);
    assert_eq!(outputs.len(), 60);
    let gaps = check_against_expected(&outputs, &expected, &registry);
    assert!(gaps.is_empty());
    for o in &outputs {
        for b in &o.batches {
            for r in &b.records {
                assert!(!r.prov().flags.contains(Flag::GapBefore));
            }
        }
    }
}

#[test]
fn base_fixture_drop_counts() {
    let registry = fixture_registry();
    let mut dropped: BTreeMap<&str, u64> = BTreeMap::new();
    for b in Replay::open(&root().join("base"), &StreamDirs::default(), registry).unwrap() {
        let b = b.unwrap();
        assert!(b.rejected.is_none());
        assert!(!b.incomplete);
        assert!(b.stats.anomalies.is_empty());
        for (r, n) in b.stats.dropped_pairs {
            *dropped.entry(r.as_str()).or_default() += n;
        }
    }
    // One backstop liquidation (SYN) and one halt settlement (syn:IDX).
    assert_eq!(dropped.get("backstop_liquidation"), Some(&1));
    assert_eq!(dropped.get("no_taker"), Some(&1));
}

#[test]
fn gap_fixture_flags_the_missing_block() {
    let registry = fixture_registry();
    let dir = root().join("gap");
    let expected: Value =
        serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
    let outputs = replay(texts_from_dir(&dir), &registry);
    let missing = expected["missing_blocks"][0].as_u64().unwrap();
    assert!(outputs.iter().all(|o| o.block_number != missing));
    let gaps = check_against_expected(&outputs, &expected, &registry);
    assert_eq!(gaps, vec![missing + 1]);
    // The first record of every stream after the gap carries gap_before,
    // exactly once.
    let mut flagged: BTreeMap<(String, RecordKind), u64> = BTreeMap::new();
    for o in &outputs {
        for b in &o.batches {
            for (i, r) in b.records.iter().enumerate() {
                if r.prov().flags.contains(Flag::GapBefore) {
                    assert_eq!(i, 0);
                    assert!(b.block_number > missing);
                    let prev = flagged.insert((b.instrument.to_string(), b.kind), b.block_number);
                    assert!(prev.is_none());
                }
            }
        }
    }
    assert_eq!(flagged.len(), 6, "{flagged:?}");
    for ((inst, kind), block) in &flagged {
        // It is the stream's first batch after the gap.
        let first_after = outputs
            .iter()
            .filter(|o| o.block_number > missing)
            .flat_map(|o| o.batches.iter())
            .find(|b| b.instrument.as_str() == inst && b.kind == *kind)
            .unwrap();
        assert_eq!(first_after.block_number, *block);
    }
}

#[test]
fn filtered_raw_parses_to_the_same_records() {
    let registry = fixture_registry();
    for scenario in ["base", "gap"] {
        let raw = texts_from_dir(&root().join(scenario));
        let filtered = filter_texts(&raw);
        for t in &filtered {
            assert_no_excluded_keys(t);
        }
        // Idempotent.
        assert_eq!(filter_texts(&filtered), filtered);
        // Same line count: no block lines dropped.
        for (r, f) in raw.iter().zip(&filtered) {
            assert_eq!(r.lines().count(), f.lines().count());
        }
        let a = replay(raw, &registry);
        let b = replay(filtered, &registry);
        assert_eq!(records(&a), records(&b), "{scenario}");
    }
}

#[test]
fn excluded_streams_are_dropped_by_directory() {
    let dirs = StreamDirs::default();
    let misc = root().join("base/misc_events_by_block/hourly/20260101/0");
    for line in std::fs::read_to_string(misc).unwrap().lines() {
        assert_eq!(
            filter_raw_line(&dirs, "misc_events_by_block", line).unwrap(),
            None
        );
    }
}

#[test]
fn l4_snapshot_matches_expected_book() {
    let registry = fixture_registry();
    for scenario in ["base", "gap"] {
        let dir = root().join(scenario);
        let expected: Value =
            serde_json::from_slice(&std::fs::read(dir.join("expected.json")).unwrap()).unwrap();
        let text = std::fs::read_to_string(dir.join("l4_snapshots/1000040.json")).unwrap();
        let parsed = parse_l4_snapshot(&text, &registry, UnixMillis(1), UnixNanos(2)).unwrap();
        assert_eq!(parsed.height, expected["snapshot_height"].as_u64().unwrap());
        assert_eq!(parsed.dropped_triggers, 1);
        assert_eq!(parsed.snapshots.len(), 3);
        for s in &parsed.snapshots {
            mesh_types::allowlist::check_record(&s.clone().into()).unwrap();
            let coin = &registry.get(&s.prov.instrument).unwrap().venue_symbol;
            let SnapshotDepth::L4 { bids, asks } = &s.depth else {
                panic!()
            };
            let side = |orders: &[mesh_types::RestingOrder]| -> Vec<Value> {
                orders
                    .iter()
                    .map(|o| {
                        json!({
                            "order_id": o.order_id.0,
                            "price": o.price.to_string(),
                            "size": o.size.to_string(),
                            "ts_order_ms": o.ts_order_ms.0,
                        })
                    })
                    .collect()
            };
            let want = &expected["coins"][coin]["book_at_snapshot"];
            assert_eq!(
                json!({"bids": side(bids), "asks": side(asks)}),
                *want,
                "{coin}"
            );
        }
        let filtered = filter_l4_snapshot(&text).unwrap();
        assert_no_excluded_keys(&filtered);
        let again = parse_l4_snapshot(&filtered, &registry, UnixMillis(1), UnixNanos(2)).unwrap();
        assert_eq!(again.snapshots, parsed.snapshots);
    }
}

#[test]
fn unknown_registry_drops_and_counts() {
    // With an empty registry nothing is emitted, and unknown coins are
    // reported instead of guessed.
    let blocks: Vec<_> = Replay::open(
        &root().join("base"),
        &StreamDirs::default(),
        Registry::new(),
    )
    .unwrap()
    .map(Result::unwrap)
    .collect();
    assert!(blocks.iter().all(|b| b.streams.is_empty()));
    assert!(
        blocks
            .iter()
            .any(|b| b.stats.unknown_coins.contains("syn:IDX"))
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Every generated scenario: records match the generator's ground
    /// truth, and filtered raw parses to the same records.
    #[test]
    fn generated_scenarios(seed in 0u64..1_000_000, blocks in 45u64..90) {
        let files = hl_synth::generate(hl_synth::Config { seed, blocks }).unwrap();
        let registry: Registry = serde_json::from_slice(&files["markets.json"]).unwrap();
        for scenario in ["base", "gap"] {
            let expected: Value =
                serde_json::from_slice(&files[&format!("{scenario}/expected.json")]).unwrap();
            let raw = texts_from_files(&files, scenario);
            let outputs = replay(raw.clone(), &registry);
            let gaps = check_against_expected(&outputs, &expected, &registry);
            prop_assert_eq!(gaps.len(), usize::from(scenario == "gap"));
            let filtered = filter_texts(&raw);
            prop_assert_eq!(records(&replay(filtered.clone(), &registry)), records(&outputs));
            prop_assert_eq!(filter_texts(&filtered), filtered);
        }
    }
}
