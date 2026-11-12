//! Arrow schemas for archived records.
//!
//! One flat schema per record kind, used for the normalized Parquet
//! partitions (see `docs/specs/archive-layout-v0.md`). Conventions:
//!
//! - Provenance is flattened into leading columns: `instrument`, `venue`,
//!   `ts_source_ms`, `ts_ingest_ns`, `block_number`, `event_index`,
//!   `admission`, `flags`. `ts_publish_ns` is live-only and not archived.
//! - Fixed-point values are `Int64` mantissas. Their scales are separate
//!   `UInt8` columns (`price_scale`, `size_scale`), constant within a
//!   partition and nearly free after Parquet encoding. Readers compute the
//!   decimal value as `mantissa / 10^scale`.
//! - Timestamps are Arrow timestamps in UTC: milliseconds for source times,
//!   nanoseconds for mesh times.
//! - Enumerations are `Utf8` with the JSON spellings (`buy`, `bid`,
//!   `add`, ...).
//! - Derived kinds add `method`, `inputs_hash` (32-byte fixed binary) and
//!   `input_count`.
//!
//! Every schema carries metadata `bleh.schema = observation-v0` and
//! `bleh.kind = <kind>`.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, Schema, TimeUnit};

use crate::OBSERVATION_SCHEMA;
use crate::record::RecordKind;

/// Metadata key naming the observation schema version.
pub const META_SCHEMA: &str = "bleh.schema";
/// Metadata key naming the record kind.
pub const META_KIND: &str = "bleh.kind";

fn utc(unit: TimeUnit) -> DataType {
    DataType::Timestamp(unit, Some("UTC".into()))
}

fn req(name: &str, dt: DataType) -> Field {
    Field::new(name, dt, false)
}

fn opt(name: &str, dt: DataType) -> Field {
    Field::new(name, dt, true)
}

fn provenance() -> Vec<Field> {
    vec![
        req("instrument", DataType::Utf8),
        req("venue", DataType::Utf8),
        req("ts_source_ms", utc(TimeUnit::Millisecond)),
        req("ts_ingest_ns", utc(TimeUnit::Nanosecond)),
        req("block_number", DataType::UInt64),
        req("event_index", DataType::UInt32),
        req("admission", DataType::Utf8),
        req("flags", DataType::UInt32),
    ]
}

fn derived() -> Vec<Field> {
    vec![
        req("method", DataType::Utf8),
        req("inputs_hash", DataType::FixedSizeBinary(32)),
        req("input_count", DataType::UInt64),
    ]
}

fn scales() -> Vec<Field> {
    vec![
        req("price_scale", DataType::UInt8),
        req("size_scale", DataType::UInt8),
    ]
}

fn list_of(name: &str, fields: Vec<Field>) -> Field {
    let item = Field::new("item", DataType::Struct(Fields::from(fields)), false);
    req(name, DataType::List(Arc::new(item)))
}

fn book_side_entry() -> Vec<Field> {
    vec![
        opt("order_id", DataType::UInt64),
        req("price", DataType::Int64),
        req("size", DataType::Int64),
        opt("ts_order_ms", utc(TimeUnit::Millisecond)),
        opt("order_count", DataType::UInt32),
    ]
}

fn kind_fields(kind: RecordKind) -> Vec<Field> {
    match kind {
        RecordKind::Trade => vec![
            req("price", DataType::Int64),
            req("size", DataType::Int64),
            req("aggressor", DataType::Utf8),
            req("trade_id", DataType::UInt64),
        ],
        RecordKind::BookEvent => vec![
            req("order_id", DataType::UInt64),
            req("side", DataType::Utf8),
            req("price", DataType::Int64),
            req("action", DataType::Utf8),
            // Size after the action; null for a cancel.
            opt("size", DataType::Int64),
            opt("orig_size", DataType::Int64),
        ],
        RecordKind::BookSnapshot => vec![
            req("depth", DataType::Utf8),
            list_of("bids", book_side_entry()),
            list_of("asks", book_side_entry()),
        ],
        RecordKind::Bar => vec![
            req("interval", DataType::Utf8),
            req("start_ms", utc(TimeUnit::Millisecond)),
            req("open", DataType::Int64),
            req("high", DataType::Int64),
            req("low", DataType::Int64),
            req("close", DataType::Int64),
            req("volume", DataType::Int64),
            req("buy_volume", DataType::Int64),
            req("sell_volume", DataType::Int64),
            req("trade_count", DataType::UInt32),
        ],
        RecordKind::Footprint => vec![
            req("interval", DataType::Utf8),
            req("start_ms", utc(TimeUnit::Millisecond)),
            list_of(
                "levels",
                vec![
                    req("price", DataType::Int64),
                    req("bid_volume", DataType::Int64),
                    req("ask_volume", DataType::Int64),
                    req("trade_count", DataType::UInt32),
                ],
            ),
        ],
        RecordKind::VolumeProfile => vec![
            req("start_ms", utc(TimeUnit::Millisecond)),
            req("end_ms", utc(TimeUnit::Millisecond)),
            req("bucket", DataType::Int64),
            list_of(
                "levels",
                vec![
                    req("price", DataType::Int64),
                    req("volume", DataType::Int64),
                    req("buy_volume", DataType::Int64),
                    req("sell_volume", DataType::Int64),
                ],
            ),
        ],
        RecordKind::SwapPrint => vec![
            req("repository", DataType::Utf8),
            req("dissemination_id", DataType::Utf8),
            opt("original_dissemination_id", DataType::Utf8),
            req("action", DataType::Utf8),
            req("asset_class", DataType::Utf8),
            opt("upi", DataType::Utf8),
            opt("product", DataType::Utf8),
            opt("underlier", DataType::Utf8),
            opt("tenor", DataType::Utf8),
            opt("expiry", DataType::Date32),
            opt("price_type", DataType::Utf8),
            opt("price", DataType::Int64),
            opt("price_scale", DataType::UInt8),
            opt("notional", DataType::Int64),
            opt("notional_scale", DataType::UInt8),
            opt("notional_currency", DataType::Utf8),
            req("ts_execution_ms", utc(TimeUnit::Millisecond)),
            req("ts_dissemination_ms", utc(TimeUnit::Millisecond)),
        ],
    }
}

/// The Arrow schema for archived records of `kind`.
#[must_use]
pub fn schema(kind: RecordKind) -> Schema {
    let mut fields = provenance();
    if kind.is_derived() {
        fields.extend(derived());
    }
    fields.extend(kind_fields(kind));
    // Swap prints carry per-row scales; every other kind has one price and
    // one size scale per instrument.
    if kind != RecordKind::SwapPrint {
        fields.extend(scales());
    }
    let metadata = HashMap::from([
        (META_SCHEMA.to_owned(), OBSERVATION_SCHEMA.to_owned()),
        (META_KIND.to_owned(), kind.as_str().to_owned()),
    ]);
    Schema::new_with_metadata(fields, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allowlist::EXCLUDED_SOURCE_FIELDS;

    fn names(dt: &DataType, prefix: &str, out: &mut Vec<String>) {
        match dt {
            DataType::List(f) => names(f.data_type(), prefix, out),
            DataType::Struct(fields) => {
                for f in fields {
                    out.push(format!("{prefix}.{}", f.name()));
                    names(f.data_type(), &format!("{prefix}.{}", f.name()), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn schemas_are_well_formed() {
        for kind in RecordKind::ALL {
            let s = schema(kind);
            assert_eq!(s.metadata()[META_SCHEMA], "observation-v0");
            assert_eq!(s.metadata()[META_KIND], kind.as_str());
            let mut all: Vec<String> = Vec::new();
            for f in s.fields() {
                all.push(f.name().clone());
                names(f.data_type(), f.name(), &mut all);
            }
            let top: std::collections::HashSet<_> = s.fields().iter().map(|f| f.name()).collect();
            assert_eq!(top.len(), s.fields().len(), "{kind}: duplicate column");
            for n in &all {
                for seg in n.split('.') {
                    assert!(!EXCLUDED_SOURCE_FIELDS.contains(&seg), "{kind}: {n}");
                }
            }
            assert_eq!(s.field(0).name(), "instrument");
            assert!(s.field_with_name("ts_publish_ns").is_err());
            assert_eq!(s.field_with_name("inputs_hash").is_ok(), kind.is_derived());
        }
    }
}
