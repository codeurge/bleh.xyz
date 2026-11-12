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
//! - Analytics kinds, coverage, reference values and synthetic months
//!   ([`RecordKind::is_analytics`]) are flat
//!   too, with one column per top-level field. Each column's
//!   [`META_ENCODING`] field metadata says how its value is stored:
//!   `price` or `size` (an `Int64` mantissa at the row's `price_scale` or
//!   `size_scale`), `decimal` (exact decimal text, for values with a
//!   method-defined scale such as basis points with two decimals) or
//!   `json` (the canonical JSON of a nested value, such as a queue level
//!   or heatmap cells). Columns without it hold plain scalars. Kinds with
//!   no `price` or `size` column have no scale columns.
//! - Swap prints carry per-row scales for each decimal.
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
/// Field metadata key giving how an analytics column stores its value
/// (`price`, `size`, `decimal` or `json`; see the module docs).
pub const META_ENCODING: &str = "bleh.encoding";

/// How an analytics column stores its value ([`META_ENCODING`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnEncoding {
    /// A plain scalar (string, integer, boolean, timestamp).
    Plain,
    /// `Int64` mantissa at the row's `price_scale`.
    Price,
    /// `Int64` mantissa at the row's `size_scale` (may be negative for
    /// signed quantities).
    Size,
    /// Exact decimal text.
    Decimal,
    /// Canonical JSON text of a nested value.
    Json,
}

impl ColumnEncoding {
    /// The metadata value, if any.
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Price => Some("price"),
            Self::Size => Some("size"),
            Self::Decimal => Some("decimal"),
            Self::Json => Some("json"),
        }
    }

    /// The encoding of a field built by [`schema`].
    #[must_use]
    pub fn of(field: &Field) -> Self {
        match field.metadata().get(META_ENCODING).map(String::as_str) {
            Some("price") => Self::Price,
            Some("size") => Self::Size,
            Some("decimal") => Self::Decimal,
            Some("json") => Self::Json,
            _ => Self::Plain,
        }
    }
}

fn enc(f: Field, e: ColumnEncoding) -> Field {
    match e.as_str() {
        Some(v) => f.with_metadata(HashMap::from([(META_ENCODING.to_owned(), v.to_owned())])),
        None => f,
    }
}

fn price_col(name: &str) -> Field {
    enc(req(name, DataType::Int64), ColumnEncoding::Price)
}

fn size_col(name: &str) -> Field {
    enc(req(name, DataType::Int64), ColumnEncoding::Size)
}

fn decimal_col(name: &str, nullable: bool) -> Field {
    enc(
        Field::new(name, DataType::Utf8, nullable),
        ColumnEncoding::Decimal,
    )
}

fn json_col(name: &str, nullable: bool) -> Field {
    enc(
        Field::new(name, DataType::Utf8, nullable),
        ColumnEncoding::Json,
    )
}

fn opt_price_col(name: &str) -> Field {
    enc(opt(name, DataType::Int64), ColumnEncoding::Price)
}

fn ms_col(name: &str) -> Field {
    req(name, utc(TimeUnit::Millisecond))
}

fn opt_ms_col(name: &str) -> Field {
    opt(name, utc(TimeUnit::Millisecond))
}

/// Kind-specific columns of the analytics kinds and coverage.
fn analytics_fields(kind: RecordKind) -> Vec<Field> {
    match kind {
        RecordKind::ActivityBar => vec![
            json_col("spec", false),
            ms_col("session_start_ms"),
            json_col("first_seq", false),
            ms_col("first_ms"),
            price_col("open"),
            price_col("high"),
            price_col("low"),
            price_col("close"),
            size_col("volume"),
            size_col("buy_volume"),
            size_col("sell_volume"),
            req("trade_count", DataType::UInt32),
            req("complete", DataType::Boolean),
        ],
        RecordKind::DeltaBar => vec![
            req("interval", DataType::Utf8),
            ms_col("start_ms"),
            size_col("buy_volume"),
            size_col("sell_volume"),
            size_col("volume_delta"),
            ms_col("anchor_start_ms"),
            size_col("cvd"),
        ],
        RecordKind::ProfileSummary => vec![
            ms_col("start_ms"),
            ms_col("end_ms"),
            price_col("bucket"),
            req("value_area_bps", DataType::UInt32),
            price_col("poc"),
            price_col("value_area_low"),
            price_col("value_area_high"),
            size_col("value_area_volume"),
            size_col("total_volume"),
        ],
        RecordKind::OrderActivity => vec![
            req("interval", DataType::Utf8),
            ms_col("start_ms"),
            req("adds", DataType::UInt64),
            req("modifies", DataType::UInt64),
            req("cancels", DataType::UInt64),
            req("partial_fills", DataType::UInt64),
            req("full_fills", DataType::UInt64),
            size_col("added_size"),
            size_col("cancelled_size"),
            size_col("filled_size"),
            opt("cancel_to_add_bps", DataType::UInt64),
            opt("cancel_replace_to_fill_bps", DataType::UInt64),
            json_col("lifetimes", false),
        ],
        RecordKind::QueuePositions => vec![json_col("bid", true), json_col("ask", true)],
        RecordKind::Sweep => vec![
            req("side", DataType::Utf8),
            json_col("first_seq", false),
            price_col("first_price"),
            price_col("last_price"),
            req("levels", DataType::UInt32),
            size_col("size"),
            req("trade_count", DataType::UInt32),
        ],
        RecordKind::HeatmapTile => vec![
            ms_col("time_start_ms"),
            req("time_step_ms", DataType::Int64),
            req("cols", DataType::UInt32),
            price_col("price_start"),
            price_col("price_step"),
            req("rows", DataType::UInt32),
            json_col("cells", false),
        ],
        RecordKind::LiquidityGrade => vec![
            ms_col("as_of_ms"),
            req("window_ms", DataType::Int64),
            req("samples", DataType::UInt32),
            decimal_col("spread_bps", true),
            req("depth_bps", DataType::UInt32),
            decimal_col("depth_bid", false),
            decimal_col("depth_ask", false),
            req("trades", DataType::UInt64),
            decimal_col("trades_per_minute", false),
            req("grade", DataType::Utf8),
            req("label", DataType::Utf8),
        ],
        RecordKind::Coverage => vec![
            req("interval", DataType::Utf8),
            ms_col("start_ms"),
            req("first_block", DataType::UInt64),
            req("last_block", DataType::UInt64),
            req("blocks", DataType::UInt64),
            req("missing_blocks", DataType::UInt64),
            req("gap_blocks", DataType::UInt64),
        ],
        RecordKind::ReferenceValue => vec![
            req("measure", DataType::Utf8),
            decimal_col("value", false),
            opt("tenor", DataType::Utf8),
            opt("maturity_date", DataType::Utf8),
            opt("day_count", DataType::Utf8),
            opt("compounding", DataType::Utf8),
            opt("currency", DataType::Utf8),
            req("as_of_date", DataType::Utf8),
            req("revision", DataType::UInt32),
        ],
        RecordKind::SyntheticMonth => vec![
            req("exposure", DataType::Utf8),
            req("family", DataType::Utf8),
            req("month", DataType::Utf8),
            req("rank", DataType::UInt32),
            ms_col("as_of_ms"),
            req("last_trade_date", DataType::Utf8),
            ms_col("expiry_ms"),
            decimal_col("year_fraction", false),
            price_col("fair_value"),
            opt_price_col("spot"),
            opt_ms_col("spot_ms"),
            req("spot_sources", DataType::UInt32),
            decimal_col("financing_rate", false),
            ms_col("rates_ms"),
            decimal_col("carry_rate", true),
            req("carry_source", DataType::Utf8),
            req("anchors", DataType::UInt32),
            opt_ms_col("anchor_ms"),
            decimal_col("implied_rate", true),
            json_col("quality", false),
            json_col("admissions", false),
            opt("grade", DataType::Utf8),
            req("label", DataType::Utf8),
        ],
        _ => Vec::new(),
    }
}

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
            opt("quantity", DataType::Int64),
            opt("quantity_scale", DataType::UInt8),
            opt("quantity_unit", DataType::Utf8),
            opt("event_type", DataType::Utf8),
            opt("strike_type", DataType::Utf8),
            opt("strike", DataType::Int64),
            opt("strike_scale", DataType::UInt8),
            opt("option_premium", DataType::Int64),
            opt("option_premium_scale", DataType::UInt8),
            opt("option_premium_currency", DataType::Utf8),
            opt("option_type", DataType::Utf8),
            opt("option_style", DataType::Utf8),
            opt("first_exercise_date", DataType::Date32),
            req("ts_execution_ms", utc(TimeUnit::Millisecond)),
            req("ts_dissemination_ms", utc(TimeUnit::Millisecond)),
        ],
        RecordKind::ActivityBar
        | RecordKind::DeltaBar
        | RecordKind::ProfileSummary
        | RecordKind::OrderActivity
        | RecordKind::QueuePositions
        | RecordKind::Sweep
        | RecordKind::HeatmapTile
        | RecordKind::LiquidityGrade
        | RecordKind::Coverage
        | RecordKind::ReferenceValue
        | RecordKind::SyntheticMonth => analytics_fields(kind),
    }
}

/// True if `kind`'s schema has `price_scale` and `size_scale` columns.
#[must_use]
pub fn has_scales(kind: RecordKind) -> bool {
    if kind == RecordKind::SwapPrint {
        return false;
    }
    if !kind.is_analytics() {
        return true;
    }
    analytics_fields(kind).iter().any(|f| {
        matches!(
            ColumnEncoding::of(f),
            ColumnEncoding::Price | ColumnEncoding::Size
        )
    })
}

/// The Arrow schema for archived records of `kind`.
#[must_use]
pub fn schema(kind: RecordKind) -> Schema {
    let mut fields = provenance();
    if kind.is_derived() {
        fields.extend(derived());
    }
    fields.extend(kind_fields(kind));
    // Swap prints carry per-row scales; every other kind with prices or
    // sizes has one price and one size scale per instrument.
    if has_scales(kind) {
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
            if kind != RecordKind::SwapPrint {
                assert_eq!(s.field_with_name("price_scale").is_ok(), has_scales(kind));
            }
        }
    }

    #[test]
    fn analytics_columns_cover_every_field() {
        use crate::allowlist::kind_paths;
        for kind in RecordKind::ALL.into_iter().filter(|k| k.is_analytics()) {
            let s = schema(kind);
            for p in kind_paths(kind)
                .iter()
                .filter(|p| !p.contains('.') && !p.contains("[]"))
            {
                assert!(s.field_with_name(p).is_ok(), "{kind}: no column for {p}");
            }
            for f in analytics_fields(kind) {
                assert!(
                    kind_paths(kind).contains(&f.name().as_str()),
                    "{kind}: column {} is not a field",
                    f.name()
                );
            }
        }
        assert!(!has_scales(RecordKind::Coverage));
        assert!(!has_scales(RecordKind::LiquidityGrade));
        assert!(has_scales(RecordKind::DeltaBar));
        let f = schema(RecordKind::DeltaBar);
        assert_eq!(
            ColumnEncoding::of(f.field_with_name("cvd").unwrap()),
            ColumnEncoding::Size
        );
    }
}
