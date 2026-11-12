//! The committed synthetic fixture: inputs match the generator, and the
//! engine reproduces the committed output exactly.
//!
//! Regenerate both files after an intended change with
//! `SYNTHETIC_REGENERATE=1 cargo test -p synthetic --test fixtures`.

#![allow(clippy::unwrap_used)]

mod common;

use common::{DAY, HOUR, T_END, T_START, fixture_dir, fixture_inputs, registry, to_jsonl};
use mesh_types::Record;
use synthetic::Engine;
use synthetic::batch::{Grid, read_records, run};

fn regenerate() -> bool {
    std::env::var_os("SYNTHETIC_REGENERATE").is_some()
}

fn check(name: &str, want: &str) {
    let path = fixture_dir().join(name);
    if regenerate() {
        std::fs::write(&path, want).unwrap();
        return;
    }
    let got = std::fs::read_to_string(&path).unwrap();
    assert!(
        got == want,
        "{name} is stale: run SYNTHETIC_REGENERATE=1 cargo test -p synthetic --test fixtures"
    );
}

/// The golden grid: daily at 20:00 UTC.
fn golden_grid() -> Grid {
    Grid {
        step_ms: DAY,
        from_ms: Some(T_START + 20 * HOUR),
        to_ms: Some(T_END),
    }
}

#[test]
fn inputs_match_the_generator() {
    check("inputs.jsonl", &to_jsonl(&fixture_inputs()));
}

#[test]
fn engine_reproduces_the_committed_output() {
    let text = std::fs::read_to_string(fixture_dir().join("inputs.jsonl")).unwrap();
    let inputs = read_records(text.as_bytes()).unwrap();
    let mut engine = Engine::new(registry());
    let out = run(&mut engine, inputs, golden_grid()).unwrap();
    let records: Vec<Record> = out.records.into_iter().map(Record::from).collect();
    for r in &records {
        r.validate().unwrap();
        mesh_types::allowlist::check_record(r).unwrap();
    }
    assert!(!records.is_empty());
    check("expected.jsonl", &to_jsonl(&records));
}
