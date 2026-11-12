//! End-to-end runs of the `hl-backfill` binary on synthetic local data.
//! No network: the commands that need AWS (`estimate`, `copy`) are
//! covered by library tests against in-memory stores.

#![allow(clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hl-backfill-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lz4(data: &str) -> Vec<u8> {
    let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
    enc.write_all(data.as_bytes()).unwrap();
    enc.finish().unwrap()
}

fn write(path: &Path, data: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, data).unwrap();
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hl-backfill"))
}

/// Invented values only.
fn fill(coin: &str, side: &str, crossed: bool, tid: u64, time: u64) -> serde_json::Value {
    serde_json::json!(["0x0000000000000000000000000000000000000001", {
        "coin": coin, "px": "150.25", "sz": "2", "side": side, "time": time, "startPosition": "0",
        "dir": "Open Long", "closedPnl": "0", "hash": "0x00", "oid": tid * 10 + u64::from(crossed),
        "crossed": crossed, "fee": "0", "tid": tid, "feeToken": "USDC"
    }])
}

#[test]
fn process_compare_and_replay_on_local_files() {
    let root = tempdir("e2e");
    let raw = root.join("raw");
    let t0 = 1_753_606_800_000u64; // 2025-07-27T09:00:00Z
    let fills = [
        serde_json::json!({"local_time": "2025-07-27T09:00:00.100000000", "block_time": "2025-07-27T09:00:00.100",
            "block_number": 1000, "events": [fill("SOL", "B", true, 1, t0 + 100), fill("SOL", "A", false, 1, t0 + 100)]}),
        serde_json::json!({"local_time": "2025-07-27T09:00:00.600000000", "block_time": "2025-07-27T09:00:00.600",
            "block_number": 1001, "events": []}),
    ]
    .iter()
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    write(
        &raw.join("hl-mainnet-node-data/node_fills_by_block/hourly/20250727/9.lz4"),
        &lz4(&fills),
    );
    let snap = serde_json::json!({"time": "2025-07-27T09:00:00.700000000", "ver_num": 1, "raw": {"channel": "l2Book",
        "data": {"coin": "SOL", "time": t0 + 700, "levels": [[{"px": "150.10", "sz": "5", "n": 2}], [{"px": "150.30", "sz": "3", "n": 1}]]}}})
    .to_string();
    write(
        &raw.join("hyperliquid-archive/market_data/20250727/9/l2Book/SOL.lz4"),
        &lz4(&snap),
    );
    let registry = root.join("registry.json");
    write(
        &registry,
        br#"{"instruments":[{"id":"hyperliquid:perp:SOL","venue":"hyperliquid","class":"perp","venue_symbol":"SOL","price_scale":4,"size_scale":2,"flags":[],"admission":"ADM-0003"}]}"#,
    );
    let parts = root.join("parts");

    let out = bin()
        .args(["process", "--datasets", "node_fills_by_block,market_data"])
        .arg("--raw")
        .arg(format!("file://{}", raw.display()))
        .arg("--filtered")
        .arg(format!("file://{}", root.join("filtered").display()))
        .arg("--out-dir")
        .arg(&parts)
        .arg("--registry")
        .arg(&registry)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["hours"][0]["records"]["trade"], 1);
    assert_eq!(report["hours"][0]["records"]["book_snapshot"], 1);
    assert!(
        root.join("filtered/stream=node_fills_by_block/date=2025-07-27/09.jsonl.zst")
            .is_file()
    );
    assert!(
        parts
            .join("kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~SOL/date=2025-07-27/records.jsonl")
            .is_file()
    );

    let out = bin()
        .arg("compare")
        .arg("--backfill")
        .arg(&parts)
        .arg("--live")
        .arg(&parts)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Replay a book that matches the archived snapshot after block 1001.
    let assets = root.join("assets.json");
    write(
        &assets,
        br#"[{"asset":5,"instrument":"hyperliquid:perp:SOL","price_scale":4,"size_scale":2}]"#,
    );
    let blocks = root.join("blocks.jsonl");
    let order = |acct: u64, b: bool, p: &str, s: &str| {
        serde_json::json!({"account": acct, "action": {"type": "order", "orders": [
            {"a": 5, "b": b, "p": p, "s": s, "r": false, "t": {"limit": {"tif": "Gtc"}}}]}})
    };
    let lines = [
        serde_json::json!({"block_number": 1000, "block_time_ms": t0 + 100, "actions": [
            order(1, true, "150.10", "3"), order(2, true, "150.10", "2")]}),
        serde_json::json!({"block_number": 1001, "block_time_ms": t0 + 600, "actions": [
            order(3, false, "150.30", "3")]}),
    ]
    .iter()
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    write(&blocks, lines.as_bytes());
    let replayed = root.join("replayed.jsonl");
    let out = bin()
        .arg("replay-prototype")
        .arg("--blocks")
        .arg(&blocks)
        .arg("--assets")
        .arg(&assets)
        .arg("--checkpoints")
        .arg(&parts)
        .arg("--out")
        .arg(&replayed)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["checkpoints"][0]["matched"], true);
    assert_eq!(
        std::fs::read_to_string(&replayed).unwrap().lines().count(),
        3
    );
}

#[test]
fn rejects_excluded_datasets_on_the_command_line() {
    let out = bin()
        .args([
            "compare",
            "--backfill",
            "/nonexistent",
            "--live",
            "/nonexistent",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let root = tempdir("excluded");
    let out = bin()
        .args(["process", "--datasets", "asset_ctxs", "--raw"])
        .arg(format!("file://{}", root.display()))
        .arg("--filtered")
        .arg(format!("file://{}", root.join("f").display()))
        .arg("--out-dir")
        .arg(root.join("p"))
        .arg("--registry")
        .arg(root.join("missing.json"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
