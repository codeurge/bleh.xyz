//! The binary's offline `--parse` mode. Synthetic data only.

#![allow(clippy::unwrap_used)]

mod common;

use std::process::Command;

use common::{Report, base_trade};
use mesh_types::Record;

#[test]
fn parse_mode_prints_canonical_json_lines() {
    let mut row = base_trade("1", "CO", "2026-11-12T16:45:00Z", "2026-11-12T16:45:03Z");
    row.extend([
        ("Product name", "Commodity:Energy:NatGas:Swap:Cash"),
        ("Price", "3.215"),
        ("Price notation", "1"),
    ]);
    let report = Report::standard().row(&row).row(&{
        let mut r = base_trade("2", "CO", "2026-11-12T16:46:00Z", "2026-11-12T16:46:03Z");
        r.push(("Action type", "MODI"));
        r
    });
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("CFTC_SLICE_COMMODITIES_2026_11_12_3.zip");
    std::fs::write(&file, report.zip("x")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_swap-pollers"))
        .arg("--parse")
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1);
    let rec: Record = serde_json::from_str(lines[0]).unwrap();
    let Record::SwapPrint(p) = rec else {
        panic!("not a swap print");
    };
    // No sidecar: the batch comes from the file name.
    assert_eq!(p.prov.seq.block_number, 20_261_112_000_003);
    assert_eq!(p.prov.venue.as_str(), "cftc-part43");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("records=1"), "{stderr}");
    assert!(stderr.contains("lifecycle_action"), "{stderr}");

    // A name that does not say the bucket needs --bucket.
    let anon = dir.path().join("report.zip");
    std::fs::copy(&file, &anon).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_swap-pollers"))
        .arg("--parse")
        .arg(&anon)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let out = Command::new(env!("CARGO_BIN_EXE_swap-pollers"))
        .args(["--bucket", "CFTC_CO", "--parse"])
        .arg(&anon)
        .output()
        .unwrap();
    assert!(out.status.success());
}
