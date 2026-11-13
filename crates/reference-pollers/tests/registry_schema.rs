//! The `catalog` output is the registry's `series` section
//! (`docs/specs/registry-v0.md` § 8): it validates against the schema and
//! parses as `mesh_types::Registry`.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use mesh_types::Registry;
use reference_pollers::catalog;
use serde_json::{Value, json};

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

#[test]
fn catalog_is_a_registry_series_section() {
    let entries = catalog::entries();
    let doc = json!({
        "registry": "registry-v0",
        "instruments": [],
        "series": entries,
    });
    if let Err(e) = validate(&doc) {
        panic!("{e}");
    }

    let registry: Registry = serde_json::from_value(doc).unwrap();
    assert_eq!(registry.sections().series.len(), entries.len());
    let sofr = registry
        .sections()
        .series
        .iter()
        .find(|s| s.instrument == "nyfed:ref:SOFR")
        .unwrap();
    assert!(!sofr.notice.is_empty());
}
