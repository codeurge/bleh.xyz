//! Automatic registration from node metadata writes registry-v0 documents
//! (`docs/specs/registry-v0.schema.json`). Synthetic metadata only: the
//! market names are the fictional ones of `fixtures/synthetic/hl`.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use hl_node_reader::markets::{
    DexMeta, NodeMetadata, OutcomeMarket, PerpAsset, RegistrationPolicy, merge_specs,
    specs_from_metadata,
};
use mesh_types::Registry;
use serde_json::Value;

/// Validates `v` against `docs/specs/registry-v0.schema.json`.
fn validate(v: &Value) -> Result<(), String> {
    let url = "https://bleh.xyz/specs/registry-v0.schema.json";
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/registry-v0.schema.json");
    let schema: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    compiler.add_resource(url, schema).unwrap();
    let index = compiler.compile(url, &mut schemas).unwrap();
    schemas.validate(v, index).map_err(|e| format!("{e:#}"))
}

fn perp(name: &str, sz_decimals: u8, is_delisted: bool) -> PerpAsset {
    PerpAsset {
        name: name.to_owned(),
        sz_decimals,
        is_delisted,
    }
}

#[test]
fn registration_output_matches_the_schema() {
    let meta = NodeMetadata {
        perps: vec![perp("SYN", 3, false), perp("OLD", 1, true)],
        dexes: vec![DexMeta {
            name: "syn".to_owned(),
            perps: vec![perp("IDX", 2, false), perp("syn:OIL", 1, false)],
        }],
        outcomes: vec![OutcomeMarket {
            outcome: 900,
            sz_decimals: None,
        }],
    };
    let (specs, issues) = specs_from_metadata(&meta, &RegistrationPolicy::new().unwrap());
    assert!(issues.is_empty(), "{issues:?}");
    let mut registry = Registry::new();
    let update = merge_specs(&mut registry, specs).unwrap();
    assert_eq!(update.added.len(), 6);

    let v = serde_json::to_value(&registry).unwrap();
    if let Err(e) = validate(&v) {
        panic!("{e}");
    }
    // Registration never invents exposures, names or grades.
    assert!(v.get("exposures").is_none() && v.get("liquidity").is_none());
}
