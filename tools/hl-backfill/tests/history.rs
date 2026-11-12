//! End to end, no network: synthetic bucket objects -> `hl-backfill
//! process` -> Parquet partitions -> the history API.
//!
//! The bucket objects are hl-synth's synthetic node output (the
//! `--batch-by-block` fills stream of its `base` scenario), re-framed as
//! the `node_fills_by_block` objects `hl-backfill` reads: one LZ4 frame per
//! hourly file under the raw tier's `hl-mainnet-node-data/` prefix. The
//! ground truth is hl-synth's `expected.json`. Every value is invented by
//! hl-synth; nothing comes from real market data.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use history_api::config::{Config, Limits};
use history_api::service::{RangeQuery, Service};
use mesh_types::{Flag, Record};
use serde_json::{Value, json};

const ARCHIVE: &str = "hyperliquid-backfill";
const HOUR_MS: i64 = 3_600_000;

fn tempdir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("hl-backfill-history-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lz4(data: &[u8]) -> Vec<u8> {
    let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

/// A trade in hl-synth's `expected.json` shape.
fn trade_shape(r: &Record) -> Value {
    let Record::Trade(t) = r else {
        panic!("not a trade: {r:?}")
    };
    json!({
        "seq": [t.prov.seq.block_number, t.prov.seq.event_index],
        "ts_source_ms": t.prov.ts_source_ms.0,
        "trade_id": t.trade_id.0,
        "price": t.price.to_string(),
        "size": t.size.to_string(),
        "aggressor": t.aggressor.as_str(),
    })
}

/// Runs `hl-backfill process` on the raw tier; returns the report.
fn process(raw: &Path, registry: &Path, out: &Path, filtered: &Path) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_hl-backfill"))
        .args(["process", "--datasets", "node_fills_by_block"])
        .args(["--from", "2026-01-01", "--to", "2026-01-01"])
        .arg("--raw")
        .arg(format!("file://{}", raw.display()))
        .arg("--filtered")
        .arg(format!("file://{}", filtered.display()))
        .arg("--out-dir")
        .arg(out)
        .arg("--registry")
        .arg(registry)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Every Parquet file under `root`, by relative path, with its bytes.
fn parquet_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "parquet") {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[tokio::test]
async fn synthetic_buckets_to_history_api() {
    let files = hl_synth::generate(hl_synth::Config::default()).unwrap();
    let expected: Value = serde_json::from_slice(&files["base/expected.json"]).unwrap();
    let root = tempdir("e2e");

    // Synthetic bucket objects in the raw tier, as `copy` lays them out.
    let raw = root.join("raw");
    let prefix = "base/node_fills_by_block/hourly/20260101/";
    let mut hours = 0;
    for (rel, text) in &files {
        if let Some(hour) = rel.strip_prefix(prefix) {
            write(
                &raw.join(format!(
                    "hl-mainnet-node-data/node_fills_by_block/hourly/20260101/{hour}.lz4"
                )),
                &lz4(text),
            );
            hours += 1;
        }
    }
    assert_eq!(hours, 2, "hl-synth crosses one hour boundary");
    let registry = root.join("markets.json");
    write(&registry, &files["markets.json"]);

    // Two independent runs.
    let (out_a, out_b) = (root.join("out-a"), root.join("out-b"));
    let report = process(&raw, &registry, &out_a, &root.join("filtered-a"));
    let report_b = process(&raw, &registry, &out_b, &root.join("filtered-b"));
    assert_eq!(report["hours"].as_array().unwrap().len(), 2);
    for h in report["hours"].as_array().unwrap() {
        assert!(h["failed"].as_array().unwrap().is_empty(), "{h}");
    }

    // Partition content hashes (and bytes) are identical across runs, and
    // each file's declared hash is the hash of its records.
    let (parts_a, parts_b) = (
        parquet_files(&out_a.join(ARCHIVE)),
        parquet_files(&out_b.join(ARCHIVE)),
    );
    assert_eq!(parts_a.len(), 3, "one trade partition per synthetic market");
    assert_eq!(
        parts_a.keys().collect::<Vec<_>>(),
        parts_b.keys().collect::<Vec<_>>()
    );
    let listed: BTreeMap<&str, &str> = report["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["path"].as_str().unwrap(),
                p["content_hash"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(report["partitions"], report_b["partitions"]);
    for (rel, bytes) in &parts_a {
        assert!(rel.ends_with("/part-00000.parquet"), "{rel}");
        let a = archive::partition::read_parquet(bytes.clone().into()).unwrap();
        let b = archive::partition::read_parquet(parts_b[rel].clone().into()).unwrap();
        assert!(a.consistent() && b.consistent());
        assert_eq!(a.content_hash, b.content_hash, "{rel}");
        assert_eq!(bytes, &parts_b[rel], "{rel}: byte-identical");
        assert_eq!(listed[rel.as_str()], a.content_hash.to_string());
    }

    // The history API reads the backfill archive as written.
    let mut cfg = Config::local(&out_a);
    cfg.archives = vec![ARCHIVE.to_owned()];
    cfg.registry_file = Some(registry.clone());
    cfg.limits = Limits {
        rate_per_sec: 0.0,
        ..Limits::default()
    };
    let svc = Service::open(cfg).unwrap();
    let start = hl_synth::START_MS.div_euclid(HOUR_MS) * HOUR_MS;
    let end = start + 2 * HOUR_MS;

    let coins = expected["coins"].as_object().unwrap();
    assert_eq!(coins.len(), 3);
    let mut trade_hours: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for c in coins.values() {
        let instrument = c["instrument"].as_str().unwrap().to_owned();
        let mut got = Vec::new();
        let mut cursor = None;
        loop {
            let page = svc
                .trades(&RangeQuery {
                    instrument: instrument.clone(),
                    start: Some(start.to_string()),
                    end: Some(end.to_string()),
                    limit: Some(17),
                    cursor: cursor.take(),
                    interval: None,
                })
                .await
                .unwrap();
            got.extend(page.records);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        let want = c["trades"].as_array().unwrap();
        assert!(!want.is_empty());
        assert_eq!(
            got.iter().map(trade_shape).collect::<Vec<_>>(),
            *want,
            "{instrument}: trades match the synthetic ground truth"
        );
        for r in &got {
            let p = r.prov();
            assert_eq!(p.instrument.as_str(), instrument);
            assert_eq!(p.admission.as_str(), "ADM-0003", "{instrument}");
            assert!(p.flags.contains(Flag::Backfill), "{instrument}");
            assert!(!p.flags.contains(Flag::GapBefore), "{instrument}");
            trade_hours
                .entry(instrument.clone())
                .or_default()
                .insert(p.ts_source_ms.0.div_euclid(HOUR_MS) * HOUR_MS);
        }
    }

    // Coverage reports the hours with trades as backfilled.
    let cov = svc
        .coverage(None, Some(&start.to_string()), Some(&end.to_string()))
        .await
        .unwrap();
    assert_eq!(cov.instruments.len(), 3);
    for ic in &cov.instruments {
        let with_trades = &trade_hours[&ic.instrument];
        assert_eq!(ic.recorded_hours, 0, "{}", ic.instrument);
        assert_eq!(ic.backfilled_hours, with_trades.len(), "{}", ic.instrument);
        assert_eq!(ic.gap_hours, 0);
        for h in &ic.hours {
            let want = if with_trades.contains(&h.hour_ms) {
                "backfilled"
            } else {
                "missing"
            };
            assert_eq!(h.status, want, "{} {}", ic.instrument, h.hour_ms);
        }
    }
}
