//! Runtime field allowlist.
//!
//! The type system already prevents excluded fields: no record type has a
//! field for them, and deserialization rejects unknown fields. This module
//! adds an explicit, reviewable list of every JSON field path each record
//! kind may emit, written independently of the structs, plus a denylist of
//! venue field names that must never appear anywhere.
//!
//! Use it as a guard at output boundaries and in tests:
//!
//! - Adding a field to a record struct without adding it here makes
//!   [`check_record`] fail, so the change needs a deliberate allowlist edit
//!   (and an admission check) in review.
//! - The gateway and archive lanes call [`check_record`] in tests over
//!   everything they emit for the synthetic fixtures, which is the Phase 1
//!   "a test fails the build if any non-allowlisted field reaches the
//!   output" criterion.
//!
//! Paths are dot-separated keys; `[]` marks array elements
//! (`depth.bids[].price`). Optional fields are listed; their absence is
//! fine.

use serde_json::Value;

use crate::batch::BlockBatch;
use crate::record::{Record, RecordKind};

/// Venue field names that must never appear in mesh output, at any depth.
///
/// These are Hyperliquid node fields that ADM-0001 excludes (owner
/// addresses; oracle, mark, external and funding prices; liquidation
/// details; fees, PnL and position data; miscellaneous events) or that the
/// mesh has no use for (transaction hashes, client order IDs, builder data).
pub const EXCLUDED_SOURCE_FIELDS: &[&str] = &[
    // Owner and account identity.
    "user",
    "users",
    "owner",
    "address",
    "side_info",
    "liquidatedUser",
    "liquidated_user",
    "validator",
    // Oracle, mark, external and funding prices.
    "markPx",
    "mark_px",
    "oraclePx",
    "oracle_px",
    "externalPx",
    "external_px",
    "midPx",
    "impactPxs",
    "premium",
    "funding",
    "fundingRate",
    "funding_rate",
    "openInterest",
    // Liquidation and position details.
    "liquidation",
    "liquidatedNtlPos",
    "liquidatedPositions",
    "accountValue",
    "leverageType",
    "startPosition",
    "start_pos",
    "closedPnl",
    "szi",
    "dir",
    // Fees and builders.
    "fee",
    "feeToken",
    "builder",
    "builderFee",
    "deployerFee",
    "subBuilders",
    // Transaction plumbing.
    "hash",
    "cloid",
    "twapId",
    "twap_id",
    // Miscellaneous events.
    "inner",
    "delta",
    "usdc",
];

const PROV: &[&str] = &[
    "kind",
    "prov",
    "prov.instrument",
    "prov.venue",
    "prov.ts_source_ms",
    "prov.ts_ingest_ns",
    "prov.ts_publish_ns",
    "prov.seq",
    "prov.seq.block_number",
    "prov.seq.event_index",
    "prov.admission",
    "prov.flags",
    "prov.flags[]",
];

const DERIVED: &[&str] = &[
    "derived",
    "derived.method",
    "derived.inputs_hash",
    "derived.input_count",
];

const TRADE: &[&str] = &["price", "size", "aggressor", "trade_id"];

const BOOK_EVENT: &[&str] = &[
    "order_id",
    "side",
    "price",
    "action",
    "action.type",
    "action.size",
    "action.orig_size",
    "action.new_size",
];

const BOOK_SNAPSHOT: &[&str] = &[
    "depth",
    "depth.type",
    "depth.bids",
    "depth.bids[]",
    "depth.bids[].order_id",
    "depth.bids[].price",
    "depth.bids[].size",
    "depth.bids[].ts_order_ms",
    "depth.bids[].order_count",
    "depth.asks",
    "depth.asks[]",
    "depth.asks[].order_id",
    "depth.asks[].price",
    "depth.asks[].size",
    "depth.asks[].ts_order_ms",
    "depth.asks[].order_count",
];

const BAR: &[&str] = &[
    "interval",
    "start_ms",
    "open",
    "high",
    "low",
    "close",
    "volume",
    "buy_volume",
    "sell_volume",
    "trade_count",
];

const FOOTPRINT: &[&str] = &[
    "interval",
    "start_ms",
    "levels",
    "levels[]",
    "levels[].price",
    "levels[].bid_volume",
    "levels[].ask_volume",
    "levels[].trade_count",
];

const VOLUME_PROFILE: &[&str] = &[
    "start_ms",
    "end_ms",
    "bucket",
    "levels",
    "levels[]",
    "levels[].price",
    "levels[].volume",
    "levels[].buy_volume",
    "levels[].sell_volume",
];

const SWAP_PRINT: &[&str] = &[
    "repository",
    "dissemination_id",
    "original_dissemination_id",
    "action",
    "asset_class",
    "upi",
    "product",
    "underlier",
    "tenor",
    "expiry",
    "price",
    "price.type",
    "price.value",
    "notional",
    "notional.amount",
    "notional.currency",
    "ts_execution_ms",
    "ts_dissemination_ms",
];

/// A field that is not allowed in a record's output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AllowlistError {
    /// The path is not on the kind's allowlist.
    #[error("{kind}: field {path:?} is not allowlisted")]
    NotAllowed {
        /// Record kind.
        kind: RecordKind,
        /// Offending path.
        path: String,
    },
    /// The path contains an excluded venue field name.
    #[error("{kind}: field {path:?} is an excluded venue field")]
    Excluded {
        /// Record kind.
        kind: RecordKind,
        /// Offending path.
        path: String,
    },
    /// The value is not a JSON object or could not be serialized.
    #[error("{0}")]
    Shape(String),
}

/// The kind-specific allowlist (excluding the common provenance paths).
#[must_use]
pub const fn kind_paths(kind: RecordKind) -> &'static [&'static str] {
    match kind {
        RecordKind::Trade => TRADE,
        RecordKind::BookEvent => BOOK_EVENT,
        RecordKind::BookSnapshot => BOOK_SNAPSHOT,
        RecordKind::Bar => BAR,
        RecordKind::Footprint => FOOTPRINT,
        RecordKind::VolumeProfile => VOLUME_PROFILE,
        RecordKind::SwapPrint => SWAP_PRINT,
    }
}

/// Every allowed path for `kind`, including provenance and (for derived
/// kinds) the derived header.
#[must_use]
pub fn allowed_paths(kind: RecordKind) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = PROV.to_vec();
    if kind.is_derived() {
        v.extend_from_slice(DERIVED);
    }
    v.extend_from_slice(kind_paths(kind));
    v
}

/// Checks a serialized record of `kind` against the allowlist.
///
/// # Errors
///
/// Returns the first path that is excluded or not allowlisted.
pub fn check_value(kind: RecordKind, value: &Value) -> Result<(), AllowlistError> {
    let Value::Object(map) = value else {
        return Err(AllowlistError::Shape("record must be a JSON object".into()));
    };
    let allowed = allowed_paths(kind);
    for (k, v) in map {
        walk(kind, &allowed, k.clone(), k, v)?;
    }
    Ok(())
}

fn walk(
    kind: RecordKind,
    allowed: &[&str],
    path: String,
    key: &str,
    value: &Value,
) -> Result<(), AllowlistError> {
    if EXCLUDED_SOURCE_FIELDS.contains(&key) {
        return Err(AllowlistError::Excluded { kind, path });
    }
    if !allowed.contains(&path.as_str()) {
        return Err(AllowlistError::NotAllowed { kind, path });
    }
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                walk(kind, allowed, format!("{path}.{k}"), k, v)?;
            }
        }
        Value::Array(items) => {
            let elem = format!("{path}[]");
            for v in items {
                walk(kind, allowed, elem.clone(), key, v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Serializes `record` and checks it against the allowlist.
///
/// # Errors
///
/// Returns the first offending path.
pub fn check_record(record: &Record) -> Result<(), AllowlistError> {
    let v = serde_json::to_value(record).map_err(|e| AllowlistError::Shape(e.to_string()))?;
    check_value(record.kind(), &v)
}

/// Checks every record of a batch.
///
/// # Errors
///
/// Returns the first offending path.
pub fn check_batch(batch: &BlockBatch) -> Result<(), AllowlistError> {
    batch.records.iter().try_for_each(check_record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derived::tests::{footprint, profile};
    use crate::record::tests::{bar, book_event, snapshot, sz, trade};
    use crate::swap::tests::print;
    use crate::{BookAction, SnapshotDepth, UnixNanos};

    fn all_records() -> Vec<Record> {
        let mut t = trade(1, 0);
        t.prov.stamp_publish(UnixNanos(1));
        let mut l2 = snapshot();
        l2.depth = l2.depth.to_l2().unwrap();
        assert!(matches!(l2.depth, SnapshotDepth::L2 { .. }));
        vec![
            t.into(),
            book_event(1, 0, BookAction::Add { size: sz("1.0") }).into(),
            book_event(
                1,
                1,
                BookAction::Modify {
                    orig_size: Some(sz("1.0")),
                    new_size: sz("0.5"),
                },
            )
            .into(),
            book_event(
                1,
                2,
                BookAction::Fill {
                    orig_size: Some(sz("0.5")),
                    new_size: sz("0.0"),
                },
            )
            .into(),
            book_event(1, 3, BookAction::Cancel {}).into(),
            snapshot().into(),
            l2.into(),
            bar().into(),
            footprint().into(),
            profile().into(),
            print().into(),
            {
                let mut p = print();
                p.action = crate::SwapAction::Correct;
                p.original_dissemination_id = Some("SYN-0".into());
                p.upi = Some("SYNUPI0000".into());
                p.into()
            },
        ]
    }

    #[test]
    fn every_kind_passes() {
        let records = all_records();
        for kind in RecordKind::ALL {
            assert!(
                records.iter().any(|r| r.kind() == kind),
                "no sample for {kind}"
            );
        }
        for r in &records {
            check_record(r).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn every_allowlisted_path_is_used() {
        // Guards against stale allowlist entries.
        let records = all_records();
        let mut common = std::collections::BTreeSet::new();
        for kind in RecordKind::ALL {
            let mut seen = std::collections::BTreeSet::new();
            for r in records.iter().filter(|r| r.kind() == kind) {
                collect(&serde_json::to_value(r).unwrap(), String::new(), &mut seen);
            }
            for p in kind_paths(kind) {
                assert!(
                    seen.contains(*p),
                    "{kind}: allowlisted path {p} never emitted by samples"
                );
            }
            common.extend(seen);
        }
        for p in PROV.iter().chain(DERIVED) {
            assert!(
                common.contains(*p),
                "common path {p} never emitted by samples"
            );
        }
    }

    fn collect(v: &Value, path: String, out: &mut std::collections::BTreeSet<String>) {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    out.insert(p.clone());
                    collect(v, p, out);
                }
            }
            Value::Array(items) => {
                let p = format!("{path}[]");
                for v in items {
                    out.insert(p.clone());
                    collect(v, p.clone(), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn extra_and_excluded_fields_fail() {
        let mut v = serde_json::to_value(Record::from(trade(1, 0))).unwrap();
        v["comment"] = Value::from("x");
        assert!(matches!(
            check_value(RecordKind::Trade, &v),
            Err(AllowlistError::NotAllowed { .. })
        ));

        let mut v = serde_json::to_value(Record::from(trade(1, 0))).unwrap();
        v["prov"]["markPx"] = Value::from("1.0");
        assert_eq!(
            check_value(RecordKind::Trade, &v),
            Err(AllowlistError::Excluded {
                kind: RecordKind::Trade,
                path: "prov.markPx".into()
            })
        );

        let mut v = serde_json::to_value(Record::from(snapshot())).unwrap();
        v["depth"]["bids"][0]["user"] = Value::from("0x0");
        assert!(matches!(
            check_value(RecordKind::BookSnapshot, &v),
            Err(AllowlistError::Excluded { .. })
        ));

        // A trade field is not allowed on a book event.
        let mut v =
            serde_json::to_value(Record::from(book_event(1, 0, BookAction::Cancel {}))).unwrap();
        v["aggressor"] = Value::from("buy");
        assert!(check_value(RecordKind::BookEvent, &v).is_err());
    }

    #[test]
    fn json_schema_matches_allowlist_and_samples() {
        use std::collections::BTreeSet;
        let schema: Value = serde_json::from_str(include_str!(
            "../../../docs/specs/observation-v0.schema.json"
        ))
        .unwrap();
        let keys =
            |v: &Value| -> BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
        for kind in RecordKind::ALL {
            let def = &schema["$defs"][kind.as_str()];
            let props = keys(&def["properties"]);
            let top: BTreeSet<String> = allowed_paths(kind)
                .into_iter()
                .filter(|p| !p.contains('.') && !p.contains("[]"))
                .map(str::to_owned)
                .collect();
            assert_eq!(
                props, top,
                "{kind}: schema properties differ from allowlist"
            );
            let required: BTreeSet<String> = def["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            for r in all_records().iter().filter(|r| r.kind() == kind) {
                let v = serde_json::to_value(r).unwrap();
                let present = keys(&v);
                assert!(
                    required.is_subset(&present),
                    "{kind}: missing required field"
                );
                assert!(present.is_subset(&props), "{kind}: field not in schema");
                assert_eq!(v["kind"], def["properties"]["kind"]["const"]);
            }
        }
        let prov = keys(&schema["$defs"]["prov"]["properties"]);
        let want: BTreeSet<String> = PROV
            .iter()
            .filter_map(|p| p.strip_prefix("prov."))
            .filter(|p| !p.contains('.') && !p.contains("[]"))
            .map(str::to_owned)
            .collect();
        assert_eq!(prov, want);
        let derived = keys(&schema["$defs"]["derived"]["properties"]);
        let want: BTreeSet<String> = DERIVED
            .iter()
            .filter_map(|p| p.strip_prefix("derived."))
            .map(str::to_owned)
            .collect();
        assert_eq!(derived, want);
    }

    #[test]
    fn allowlist_and_denylist_do_not_overlap() {
        for kind in RecordKind::ALL {
            for p in allowed_paths(kind) {
                for seg in p.split('.') {
                    let seg = seg.trim_end_matches("[]");
                    assert!(!EXCLUDED_SOURCE_FIELDS.contains(&seg), "{kind}: {p}");
                }
            }
        }
    }
}
