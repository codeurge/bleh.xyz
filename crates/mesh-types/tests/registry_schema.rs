//! `docs/specs/registry-v0.schema.json` against the registry JSON the code
//! reads and writes. Synthetic data only.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use mesh_types::registry::{RegistryError, Sections};
use mesh_types::{AdmissionId, Flag, InstrumentSpec, MarketClass, RecordKind, Registry};
use serde_json::{Value, json};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(rel: &str) -> Value {
    serde_json::from_slice(&std::fs::read(repo().join(rel)).unwrap()).unwrap()
}

/// The registry schema, compiled.
struct Validator {
    schemas: boon::Schemas,
    index: boon::SchemaIndex,
}

impl Validator {
    fn is_valid(&self, v: &Value) -> bool {
        self.schemas.validate(v, self.index).is_ok()
    }
}

fn validator() -> Validator {
    let url = "https://bleh.xyz/specs/registry-v0.schema.json";
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    compiler
        .add_resource(url, read_json("docs/specs/registry-v0.schema.json"))
        .unwrap();
    let index = compiler.compile(url, &mut schemas).unwrap();
    Validator { schemas, index }
}

fn assert_valid(v: &Value) {
    let val = validator();
    if let Err(e) = val.schemas.validate(v, val.index) {
        panic!("schema errors: {e:#}");
    }
}

fn adm() -> AdmissionId {
    AdmissionId::new("ADM-0001").unwrap()
}

#[test]
fn example_registry_validates_parses_and_round_trips() {
    let v = read_json("fixtures/synthetic/registry/registry-v0.json");
    assert_valid(&v);
    let r: Registry = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(r.len(), 3);
    assert_eq!(r.sections().exposures.len(), 3);
    assert_eq!(r.sections().calendars.len(), 3);
    assert_eq!(r.sections().rolls.len(), 2);
    let back = serde_json::to_value(&r).unwrap();
    assert_eq!(back, v, "serialization differs from the example");
    assert_valid(&back);
}

#[test]
fn hyperliquid_fixture_registry_validates() {
    // Written by tools/hl-synth from `InstrumentSpec::hyperliquid`.
    let v = read_json("fixtures/synthetic/hl/markets.json");
    assert_valid(&v);
    let r: Registry = serde_json::from_value(v.clone()).unwrap();
    // An instruments-only registry keeps its old form: no version tag.
    assert_eq!(serde_json::to_value(&r).unwrap(), v);
}

#[test]
fn registry_built_from_metadata_validates() {
    let mut r = Registry::new();
    for (class, coin, d, ps) in [
        (MarketClass::Perp, "SYN", 3, None),
        (MarketClass::Perp, "syn:IDX", 2, None),
        (MarketClass::Spot, "AAA/USDC", 1, None),
        (MarketClass::Spot, "@107", 0, None),
        (MarketClass::Outcome, "#9001", 0, Some(4)),
    ] {
        r.insert(InstrumentSpec::hyperliquid(class, coin, d, ps, adm()).unwrap())
            .unwrap();
    }
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("registry").is_none());
    assert_valid(&v);
}

#[test]
fn schema_enums_match_the_code() {
    let schema = read_json("docs/specs/registry-v0.schema.json");
    let names = |p: &str| -> BTreeSet<String> {
        schema
            .pointer(p)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    let kinds: BTreeSet<String> = RecordKind::ALL
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect();
    assert_eq!(names("/$defs/record_kind/enum"), kinds);
    let flags: BTreeSet<String> = Flag::ALL.iter().map(|f| f.name().to_owned()).collect();
    assert_eq!(names("/$defs/flags/items/enum"), flags);
    let classes: BTreeSet<String> = [
        MarketClass::Perp,
        MarketClass::Spot,
        MarketClass::Outcome,
        MarketClass::Swap,
    ]
    .iter()
    .map(|c| c.as_str().to_owned())
    .collect();
    assert_eq!(names("/$defs/instrument/properties/class/enum"), classes);
}

fn example() -> Value {
    read_json("fixtures/synthetic/registry/registry-v0.json")
}

fn both_reject(v: &Value) {
    assert!(!validator().is_valid(v), "schema accepted {v}");
    assert!(
        serde_json::from_value::<Registry>(v.clone()).is_err(),
        "code accepted {v}"
    );
}

#[test]
fn schema_and_code_reject_the_same_shapes() {
    // Unknown top-level field.
    let mut v = example();
    v["extra"] = json!([]);
    both_reject(&v);
    // Another version.
    let mut v = example();
    v["registry"] = json!("registry-v1");
    both_reject(&v);
    assert!(
        serde_json::from_value::<Registry>(v)
            .unwrap_err()
            .to_string()
            .contains("unsupported registry version")
    );
    // A front exposure without a roll.
    let mut v = example();
    v["exposures"][1].as_object_mut().unwrap().remove("roll");
    both_reject(&v);
    // An entry ID with ':' (reserved for instrument IDs).
    let mut v = example();
    v["calendars"][0]["id"] = json!("us:trading");
    both_reject(&v);
    // A zero multiplier.
    let mut v = example();
    v["exposures"][2]["venue_mappings"][0]["multiplier"] = json!("0.00");
    both_reject(&v);
    // A session time past midnight.
    let mut v = example();
    v["calendars"][1]["sessions"][0]["open"] = json!("24:00");
    both_reject(&v);
    // A month outside 1 to 12.
    let mut v = example();
    v["rolls"][0]["cycle"] = json!([3, 6, 9, 13]);
    both_reject(&v);
    // Unknown field in an exposure.
    let mut v = example();
    v["exposures"][0]["ticker"] = json!("X");
    both_reject(&v);
}

#[test]
fn code_checks_references_the_schema_cannot() {
    let parse = |v: Value| serde_json::from_value::<Registry>(v);
    // Unknown calendar.
    let mut v = example();
    v["exposures"][0]["calendar"] = json!("nowhere");
    assert!(validator().is_valid(&v));
    assert!(
        parse(v)
            .unwrap_err()
            .to_string()
            .contains("unknown calendar")
    );
    // Unknown roll.
    let mut v = example();
    v["exposures"][1]["roll"] = json!("nowhere");
    assert!(parse(v).unwrap_err().to_string().contains("unknown roll"));
    // Liquidity for an unregistered instrument.
    let mut v = example();
    v["liquidity"][0]["subject"] = json!("hyperliquid:perp:NOPE");
    assert!(validator().is_valid(&v));
    assert!(parse(v).is_err());
    // Duplicate exposure ID.
    let mut v = example();
    v["exposures"][1]["id"] = json!("us-large-cap-index");
    assert!(parse(v).is_err());
    // Removing an instrument drops its liquidity entry.
    let mut r: Registry = parse(example()).unwrap();
    let id = mesh_types::InstrumentId::new("hyperliquid:perp:syn:IDX").unwrap();
    r.remove(&id).unwrap();
    assert!(
        r.sections()
            .liquidity
            .iter()
            .all(|l| l.subject != id.as_str())
    );
    // Sections that fail validation are not applied.
    let mut bad = r.sections().clone();
    bad.rolls.clear();
    assert!(matches!(
        r.set_sections(bad),
        Err(RegistryError::Invalid(_))
    ));
    assert_eq!(r.sections().rolls.len(), 2);
    r.set_sections(Sections::default()).unwrap();
    assert!(serde_json::to_value(&r).unwrap().get("registry").is_none());
}
