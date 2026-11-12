//! Static Apache Iceberg (format v2) metadata over the normalized
//! partitions, with no catalog service (`docs/specs/archive-layout-v0.md`
//! § 3.3).
//!
//! One table per record kind under `_iceberg/<kind>/metadata/`. Each
//! published day with files of that kind is one `append` snapshot whose
//! manifest lists the day's Parquet files in place. The metadata is a pure
//! function of the day manifests and the table location, so it is rebuilt
//! from scratch each time and every file except `version-hint.text` is
//! write-once:
//!
//! - snapshot IDs are the first 8 bytes of the day manifest's hash (top bit
//!   cleared); timestamps are the manifest's `created_ns`;
//! - the table UUID is derived from the archive and kind;
//! - Avro sync markers are derived from the file's path.
//!
//! Partitioning: identity on `venue` and `instrument`, and `day` of the
//! partition time column (`ts_source_ms`, or `start_ms` for interval
//! kinds), named `date`. The Hive `date=` directory is not a column in the
//! files, so the day transform of the time column stands in for it with the
//! same value.
//!
//! Paths inside Iceberg metadata are absolute, so the metadata is generated
//! for one `location` (the archive root's URI, such as
//! `s3://bucket/hyperliquid-normalized`); every mirror can generate its own.
//! Data files also carry Parquet field IDs, and the table sets
//! `schema.name-mapping.default` as a fallback.

use std::collections::BTreeMap;

use arrow_schema::{DataType, Field, Fields};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

use mesh_types::RecordKind;
use mesh_types::manifest::SignedManifest;

use super::avro::{self, Value};
use super::convert::{field_id, writer_schema};
use crate::error::{Error, Result};
use crate::layout::{self, Day};

/// First partition field ID (Iceberg convention).
const PARTITION_FIELD_BASE: i32 = 1000;

/// One data file in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFile {
    /// Path relative to the archive root.
    pub path: String,
    /// File size in bytes.
    pub size: u64,
    /// Number of records.
    pub record_count: u64,
    /// Partition: venue.
    pub venue: String,
    /// Partition: instrument ID (with `:`).
    pub instrument: String,
    /// Partition: UTC day.
    pub day: Day,
}

/// One snapshot: the data files added by one day's manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Snapshot ID.
    pub id: i64,
    /// Commit time, milliseconds since the epoch.
    pub timestamp_ms: i64,
    /// Files added.
    pub files: Vec<DataFile>,
}

/// Snapshot ID derived from a manifest hash.
#[must_use]
pub fn snapshot_id(manifest_sha256: &[u8; 32]) -> i64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&manifest_sha256[..8]);
    (u64::from_be_bytes(b) & (i64::MAX as u64)) as i64
}

/// Splits each manifest's normalized partition files into per-kind
/// snapshots, in manifest order.
///
/// # Errors
///
/// Fails if a manifest cannot be hashed or a listed partition lacks its
/// record count.
pub fn snapshots_from_manifests(
    manifests: &[SignedManifest],
) -> Result<BTreeMap<RecordKind, Vec<Snapshot>>> {
    let mut out: BTreeMap<RecordKind, Vec<Snapshot>> = BTreeMap::new();
    let mut sorted: Vec<&SignedManifest> = manifests.iter().collect();
    sorted.sort_by_key(|m| m.manifest.sequence);
    for m in sorted {
        let id = snapshot_id(&m.sha256()?.0);
        let ts = m.manifest.created_ns.0.div_euclid(1_000_000);
        let mut by_kind: BTreeMap<RecordKind, Vec<DataFile>> = BTreeMap::new();
        for f in &m.manifest.files {
            let Some((key, _)) = layout::parse_partition_path(&f.path) else {
                continue;
            };
            let record_count = f.record_count.ok_or_else(|| {
                Error::invalid(format!("{}: partition without record_count", f.path))
            })?;
            by_kind.entry(key.kind).or_default().push(DataFile {
                path: f.path.clone(),
                size: f.size,
                record_count,
                venue: key.venue.as_str().to_owned(),
                instrument: key.instrument.as_str().to_owned(),
                day: key.day,
            });
        }
        for (kind, files) in by_kind {
            out.entry(kind).or_default().push(Snapshot {
                id,
                timestamp_ms: ts,
                files,
            });
        }
    }
    Ok(out)
}

fn iceberg_type(dt: &DataType, nullable_elem: bool) -> Result<Json> {
    Ok(match dt {
        DataType::Utf8 => json!("string"),
        DataType::Boolean => json!("boolean"),
        DataType::Timestamp(_, _) => json!("timestamptz"),
        DataType::Int64 | DataType::UInt64 | DataType::UInt32 => json!("long"),
        DataType::UInt8 | DataType::Int32 => json!("int"),
        DataType::FixedSizeBinary(n) => json!(format!("fixed[{n}]")),
        DataType::Date32 => json!("date"),
        DataType::Struct(fields) => struct_type(fields)?,
        DataType::List(item) => json!({
            "type": "list",
            "element-id": field_id(item).ok_or_else(|| Error::invalid("list element without id"))?,
            "element": iceberg_type(item.data_type(), item.is_nullable())?,
            "element-required": !item.is_nullable() && !nullable_elem,
        }),
        other => return Err(Error::invalid(format!("no Iceberg type for {other}"))),
    })
}

fn iceberg_field(f: &Field) -> Result<Json> {
    Ok(json!({
        "id": field_id(f).ok_or_else(|| Error::invalid(format!("{} has no field id", f.name())))?,
        "name": f.name(),
        "required": !f.is_nullable(),
        "type": iceberg_type(f.data_type(), false)?,
    }))
}

fn struct_type(fields: &Fields) -> Result<Json> {
    Ok(json!({
        "type": "struct",
        "fields": fields.iter().map(|f| iceberg_field(f)).collect::<Result<Vec<_>>>()?,
    }))
}

fn name_mapping(fields: &Fields) -> Json {
    Json::Array(
        fields
            .iter()
            .filter_map(|f| {
                let id = field_id(f)?;
                let mut m = json!({"field-id": id, "names": [f.name()]});
                let nested = match f.data_type() {
                    DataType::Struct(sub) => Some(name_mapping(sub)),
                    DataType::List(item) => {
                        let mut e =
                            json!({"field-id": field_id(item)?, "names": ["element", item.name()]});
                        if let DataType::Struct(sub) = item.data_type() {
                            e["fields"] = name_mapping(sub);
                        }
                        Some(json!([e]))
                    }
                    _ => None,
                };
                if let Some(n) = nested {
                    m["fields"] = n;
                }
                Some(m)
            })
            .collect(),
    )
}

fn max_field_id(fields: &Fields) -> i32 {
    fields
        .iter()
        .map(|f| {
            let own = field_id(f).unwrap_or(0);
            let nested = match f.data_type() {
                DataType::Struct(sub) => max_field_id(sub),
                DataType::List(item) => {
                    let e = field_id(item).unwrap_or(0);
                    match item.data_type() {
                        DataType::Struct(sub) => e.max(max_field_id(sub)),
                        _ => e,
                    }
                }
                _ => 0,
            };
            own.max(nested)
        })
        .max()
        .unwrap_or(0)
}

/// The column the `date` partition is derived from.
#[must_use]
pub fn time_column(kind: RecordKind) -> &'static str {
    match kind {
        RecordKind::Bar | RecordKind::Footprint | RecordKind::VolumeProfile => "start_ms",
        _ => "ts_source_ms",
    }
}

fn table_uuid(archive: &str, kind: RecordKind) -> String {
    let mut h: [u8; 32] = Sha256::digest(format!("bleh-iceberg-v0\n{archive}\n{kind}")).into();
    h[6] = (h[6] & 0x0f) | 0x80; // version 8 (custom)
    h[8] = (h[8] & 0x3f) | 0x80; // RFC 4122 variant
    let x = crate::hex::encode(&h[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &x[0..8],
        &x[8..12],
        &x[12..16],
        &x[16..20],
        &x[20..32]
    )
}

const MANIFEST_ENTRY_SCHEMA: &str = r#"{"type":"record","name":"manifest_entry","fields":[{"name":"status","type":"int","field-id":0},{"name":"snapshot_id","type":["null","long"],"default":null,"field-id":1},{"name":"sequence_number","type":["null","long"],"default":null,"field-id":3},{"name":"file_sequence_number","type":["null","long"],"default":null,"field-id":4},{"name":"data_file","type":{"type":"record","name":"r2","fields":[{"name":"content","type":"int","doc":"Contents of the file: 0=data, 1=position deletes, 2=equality deletes","field-id":134},{"name":"file_path","type":"string","doc":"Location URI with FS scheme","field-id":100},{"name":"file_format","type":"string","doc":"File format name: avro, orc, or parquet","field-id":101},{"name":"partition","type":{"type":"record","name":"r102","fields":[{"name":"venue","type":["null","string"],"default":null,"field-id":1000},{"name":"instrument","type":["null","string"],"default":null,"field-id":1001},{"name":"date","type":["null",{"type":"int","logicalType":"date"}],"default":null,"field-id":1002}]},"doc":"Partition data tuple, schema based on the partition spec","field-id":102},{"name":"record_count","type":"long","doc":"Number of records in the file","field-id":103},{"name":"file_size_in_bytes","type":"long","doc":"Total file size in bytes","field-id":104},{"name":"column_sizes","type":["null",{"type":"array","items":{"type":"record","name":"k117_v118","fields":[{"name":"key","type":"int","field-id":117},{"name":"value","type":"long","field-id":118}]},"logicalType":"map"}],"doc":"Map of column id to total size on disk","default":null,"field-id":108},{"name":"value_counts","type":["null",{"type":"array","items":{"type":"record","name":"k119_v120","fields":[{"name":"key","type":"int","field-id":119},{"name":"value","type":"long","field-id":120}]},"logicalType":"map"}],"doc":"Map of column id to total count, including null and NaN","default":null,"field-id":109},{"name":"null_value_counts","type":["null",{"type":"array","items":{"type":"record","name":"k121_v122","fields":[{"name":"key","type":"int","field-id":121},{"name":"value","type":"long","field-id":122}]},"logicalType":"map"}],"doc":"Map of column id to null value count","default":null,"field-id":110},{"name":"nan_value_counts","type":["null",{"type":"array","items":{"type":"record","name":"k138_v139","fields":[{"name":"key","type":"int","field-id":138},{"name":"value","type":"long","field-id":139}]},"logicalType":"map"}],"doc":"Map of column id to number of NaN values in the column","default":null,"field-id":137},{"name":"lower_bounds","type":["null",{"type":"array","items":{"type":"record","name":"k126_v127","fields":[{"name":"key","type":"int","field-id":126},{"name":"value","type":"bytes","field-id":127}]},"logicalType":"map"}],"doc":"Map of column id to lower bound","default":null,"field-id":125},{"name":"upper_bounds","type":["null",{"type":"array","items":{"type":"record","name":"k129_v130","fields":[{"name":"key","type":"int","field-id":129},{"name":"value","type":"bytes","field-id":130}]},"logicalType":"map"}],"doc":"Map of column id to upper bound","default":null,"field-id":128},{"name":"key_metadata","type":["null","bytes"],"doc":"Encryption key metadata blob","default":null,"field-id":131},{"name":"split_offsets","type":["null",{"type":"array","items":"long","element-id":133}],"doc":"Splittable offsets","default":null,"field-id":132},{"name":"equality_ids","type":["null",{"type":"array","items":"int","element-id":136}],"doc":"Equality comparison field IDs","default":null,"field-id":135},{"name":"sort_order_id","type":["null","int"],"doc":"Sort order ID","default":null,"field-id":140}]},"field-id":2}]}"#;

const MANIFEST_LIST_SCHEMA: &str = r#"{"type":"record","name":"manifest_file","fields":[{"name":"manifest_path","type":"string","doc":"Location URI with FS scheme","field-id":500},{"name":"manifest_length","type":"long","doc":"Total file size in bytes","field-id":501},{"name":"partition_spec_id","type":"int","doc":"Spec ID used to write","field-id":502},{"name":"content","type":"int","doc":"Contents of the manifest: 0=data, 1=deletes","field-id":517},{"name":"sequence_number","type":"long","doc":"Sequence number when the manifest was added","field-id":515},{"name":"min_sequence_number","type":"long","doc":"Lowest sequence number in the manifest","field-id":516},{"name":"added_snapshot_id","type":"long","doc":"Snapshot ID that added the manifest","field-id":503},{"name":"added_files_count","type":"int","doc":"Added entry count","field-id":504},{"name":"existing_files_count","type":"int","doc":"Existing entry count","field-id":505},{"name":"deleted_files_count","type":"int","doc":"Deleted entry count","field-id":506},{"name":"added_rows_count","type":"long","doc":"Added rows count","field-id":512},{"name":"existing_rows_count","type":"long","doc":"Existing rows count","field-id":513},{"name":"deleted_rows_count","type":"long","doc":"Deleted rows count","field-id":514},{"name":"partitions","type":["null",{"type":"array","items":{"type":"record","name":"r508","fields":[{"name":"contains_null","type":"boolean","doc":"True if any file has a null partition value","field-id":509},{"name":"contains_nan","type":["null","boolean"],"doc":"True if any file has a nan partition value","default":null,"field-id":518},{"name":"lower_bound","type":["null","bytes"],"doc":"Partition lower bound for all files","default":null,"field-id":510},{"name":"upper_bound","type":["null","bytes"],"doc":"Partition upper bound for all files","default":null,"field-id":511}]},"element-id":508}],"doc":"Summary for each partition","default":null,"field-id":507},{"name":"key_metadata","type":["null","bytes"],"doc":"Encryption key metadata blob","default":null,"field-id":519}]}"#;

/// Generated metadata: files to write, relative to the archive root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableFiles {
    /// Write-once files (manifests, manifest lists, metadata versions).
    pub immutable: Vec<(String, Vec<u8>)>,
    /// `version-hint.text`, replaced on each new version.
    pub version_hint: Option<(String, Vec<u8>)>,
}

fn i32_of(v: u64, what: &str) -> Result<i32> {
    i32::try_from(v).map_err(|_| Error::invalid(format!("{what} too large for Iceberg")))
}

fn i64_of(v: u64, what: &str) -> Result<i64> {
    i64::try_from(v).map_err(|_| Error::invalid(format!("{what} too large for Iceberg")))
}

/// Builds the whole metadata history of one kind's table.
///
/// `archive` names the archive (for the table UUID), `location` is the
/// archive root's URI without a trailing slash.
///
/// # Errors
///
/// Fails on an empty snapshot list or values out of Iceberg's range.
pub fn build_table(
    archive: &str,
    location: &str,
    kind: RecordKind,
    snapshots: &[Snapshot],
) -> Result<TableFiles> {
    if snapshots.is_empty() {
        return Ok(TableFiles::default());
    }
    let location = location.trim_end_matches('/');
    let schema = writer_schema(kind);
    let table_rel = format!("{}/{}", layout::ICEBERG_DIR, kind.as_str());
    let meta_rel = format!("{table_rel}/metadata");
    let abs = |rel: &str| format!("{location}/{rel}");

    let source = |name: &str| -> Result<i32> {
        schema
            .field_with_name(name)
            .ok()
            .and_then(field_id)
            .ok_or_else(|| Error::invalid(format!("no field id for {name}")))
    };
    let spec_fields = json!([
        {"name": "venue", "transform": "identity", "source-id": source("venue")?, "field-id": PARTITION_FIELD_BASE},
        {"name": "instrument", "transform": "identity", "source-id": source("instrument")?, "field-id": PARTITION_FIELD_BASE + 1},
        {"name": "date", "transform": "day", "source-id": source(time_column(kind))?, "field-id": PARTITION_FIELD_BASE + 2},
    ]);
    let mut schema_json = struct_type(schema.fields())?;
    schema_json["schema-id"] = json!(0);
    let schema_str = serde_json::to_string(&schema_json)?;
    let spec_str = serde_json::to_string(&spec_fields)?;
    let mapping = serde_json::to_string(&name_mapping(schema.fields()))?;

    let mut out = TableFiles::default();
    let mut manifest_list_entries: Vec<Value> = Vec::new();
    let mut snapshot_entries: Vec<Json> = Vec::new();
    let mut snapshot_log: Vec<Json> = Vec::new();
    let mut metadata_log: Vec<Json> = Vec::new();
    let (mut total_files, mut total_records, mut total_size) = (0u64, 0u64, 0u64);
    let mut parent: Option<i64> = None;
    let mut last_ts = i64::MIN;

    for (i, snap) in snapshots.iter().enumerate() {
        let seq = i64::try_from(i + 1).map_err(|_| Error::invalid("too many snapshots"))?;
        let mut files = snap.files.clone();
        files.sort_by(|a, b| a.path.cmp(&b.path));

        // Data manifest for this snapshot.
        let entries = files
            .iter()
            .map(|f| {
                Ok(Value::Record(vec![
                    Value::Int(1), // ADDED
                    Value::some(Value::Long(snap.id)),
                    Value::some(Value::Long(seq)),
                    Value::some(Value::Long(seq)),
                    Value::Record(vec![
                        Value::Int(0),
                        Value::Str(abs(&f.path)),
                        Value::Str("PARQUET".into()),
                        Value::Record(vec![
                            Value::some(Value::Str(f.venue.clone())),
                            Value::some(Value::Str(f.instrument.clone())),
                            Value::some(Value::Int(f.day.days())),
                        ]),
                        Value::Long(i64_of(f.record_count, "record_count")?),
                        Value::Long(i64_of(f.size, "file size")?),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                        Value::none(),
                    ]),
                ]))
            })
            .collect::<Result<Vec<_>>>()?;
        let manifest_rel = format!("{meta_rel}/{}-m0.avro", snap.id);
        let manifest_bytes = avro::container(
            MANIFEST_ENTRY_SCHEMA,
            &[
                ("schema", schema_str.clone()),
                ("schema-id", "0".into()),
                ("partition-spec", spec_str.clone()),
                ("partition-spec-id", "0".into()),
                ("format-version", "2".into()),
                ("content", "data".into()),
            ],
            &entries,
            manifest_rel.as_bytes(),
        );

        let added_rows: u64 = files.iter().map(|f| f.record_count).sum();
        let added_size: u64 = files.iter().map(|f| f.size).sum();
        total_files += files.len() as u64;
        total_records += added_rows;
        total_size += added_size;

        let bounds = |vals: Vec<Vec<u8>>| -> Value {
            let lo = vals.iter().min().cloned().unwrap_or_default();
            let hi = vals.iter().max().cloned().unwrap_or_default();
            Value::Record(vec![
                Value::Bool(false),
                Value::none(),
                Value::some(Value::Bytes(lo)),
                Value::some(Value::Bytes(hi)),
            ])
        };
        // Day bounds compare numerically; encode min and max by value.
        let day_min = files.iter().map(|f| f.day.days()).min().unwrap_or(0);
        let day_max = files.iter().map(|f| f.day.days()).max().unwrap_or(0);
        manifest_list_entries.push(Value::Record(vec![
            Value::Str(abs(&manifest_rel)),
            Value::Long(manifest_bytes.len() as i64),
            Value::Int(0),
            Value::Int(0),
            Value::Long(seq),
            Value::Long(seq),
            Value::Long(snap.id),
            Value::Int(i32_of(files.len() as u64, "file count")?),
            Value::Int(0),
            Value::Int(0),
            Value::Long(i64_of(added_rows, "row count")?),
            Value::Long(0),
            Value::Long(0),
            Value::some(Value::Array(vec![
                bounds(files.iter().map(|f| f.venue.as_bytes().to_vec()).collect()),
                bounds(
                    files
                        .iter()
                        .map(|f| f.instrument.as_bytes().to_vec())
                        .collect(),
                ),
                Value::Record(vec![
                    Value::Bool(false),
                    Value::none(),
                    Value::some(Value::Bytes(day_min.to_le_bytes().to_vec())),
                    Value::some(Value::Bytes(day_max.to_le_bytes().to_vec())),
                ]),
            ])),
            Value::none(),
        ]));
        out.immutable.push((manifest_rel, manifest_bytes));

        // Manifest list: every manifest so far, newest first.
        let list_rel = format!("{meta_rel}/snap-{}-1.avro", snap.id);
        let mut list: Vec<Value> = manifest_list_entries.clone();
        list.reverse();
        let list_bytes = avro::container(
            MANIFEST_LIST_SCHEMA,
            &[
                ("snapshot-id", snap.id.to_string()),
                (
                    "parent-snapshot-id",
                    parent.map_or_else(|| "null".to_owned(), |p| p.to_string()),
                ),
                ("sequence-number", seq.to_string()),
                ("format-version", "2".into()),
            ],
            &list,
            list_rel.as_bytes(),
        );
        out.immutable.push((list_rel.clone(), list_bytes));

        // Timestamps must not go backwards.
        let ts = snap.timestamp_ms.max(last_ts);
        last_ts = ts;
        let mut snapshot = json!({
            "snapshot-id": snap.id,
            "sequence-number": seq,
            "timestamp-ms": ts,
            "manifest-list": abs(&list_rel),
            "summary": {
                "operation": "append",
                "added-data-files": files.len().to_string(),
                "added-records": added_rows.to_string(),
                "added-files-size": added_size.to_string(),
                "total-data-files": total_files.to_string(),
                "total-records": total_records.to_string(),
                "total-files-size": total_size.to_string(),
                "total-delete-files": "0",
                "total-position-deletes": "0",
                "total-equality-deletes": "0",
            },
            "schema-id": 0,
        });
        if let Some(p) = parent {
            snapshot["parent-snapshot-id"] = json!(p);
        }
        snapshot_entries.push(snapshot);
        snapshot_log.push(json!({"timestamp-ms": ts, "snapshot-id": snap.id}));

        let version = i + 1;
        let metadata = json!({
            "format-version": 2,
            "table-uuid": table_uuid(archive, kind),
            "location": abs(&table_rel),
            "last-sequence-number": seq,
            "last-updated-ms": ts,
            "last-column-id": max_field_id(schema.fields()),
            "current-schema-id": 0,
            "schemas": [schema_json.clone()],
            "default-spec-id": 0,
            "partition-specs": [{"spec-id": 0, "fields": spec_fields.clone()}],
            "last-partition-id": PARTITION_FIELD_BASE + 2,
            "default-sort-order-id": 0,
            "sort-orders": [{"order-id": 0, "fields": []}],
            "properties": {
                "schema.name-mapping.default": mapping.clone(),
                "write.format.default": "parquet",
                "bleh.archive": archive,
                "bleh.kind": kind.as_str(),
            },
            "current-snapshot-id": snap.id,
            "refs": {"main": {"snapshot-id": snap.id, "type": "branch"}},
            "snapshots": snapshot_entries.clone(),
            "snapshot-log": snapshot_log.clone(),
            "metadata-log": metadata_log.clone(),
        });
        let rel = format!("{meta_rel}/v{version}.metadata.json");
        let mut bytes = serde_json::to_vec_pretty(&metadata)?;
        bytes.push(b'\n');
        metadata_log.push(json!({"timestamp-ms": ts, "metadata-file": abs(&rel)}));
        out.immutable.push((rel, bytes));
        out.version_hint = Some((
            format!("{meta_rel}/version-hint.text"),
            version.to_string().into_bytes(),
        ));
        parent = Some(snap.id);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(id: i64, ts: i64, day: &str, n: u64) -> Snapshot {
        Snapshot {
            id,
            timestamp_ms: ts,
            files: (0..n)
                .map(|i| DataFile {
                    path: format!(
                        "kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~SYN/date={day}/part-0000{i}.parquet"
                    ),
                    size: 1000 + i,
                    record_count: 10 + i,
                    venue: "hyperliquid".into(),
                    instrument: "hyperliquid:perp:SYN".into(),
                    day: day.parse().unwrap(),
                })
                .collect(),
        }
    }

    #[test]
    fn table_history_is_deterministic_and_well_formed() {
        let snaps = vec![
            snap(11, 1000, "2026-01-01", 2),
            snap(22, 2000, "2026-01-02", 1),
        ];
        let a = build_table(
            "hyperliquid-normalized",
            "s3://b/hyperliquid-normalized/",
            RecordKind::Trade,
            &snaps,
        )
        .unwrap();
        let b = build_table(
            "hyperliquid-normalized",
            "s3://b/hyperliquid-normalized",
            RecordKind::Trade,
            &snaps,
        )
        .unwrap();
        assert_eq!(a, b);
        let names: Vec<&str> = a.immutable.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "_iceberg/trade/metadata/11-m0.avro",
                "_iceberg/trade/metadata/snap-11-1.avro",
                "_iceberg/trade/metadata/v1.metadata.json",
                "_iceberg/trade/metadata/22-m0.avro",
                "_iceberg/trade/metadata/snap-22-1.avro",
                "_iceberg/trade/metadata/v2.metadata.json",
            ]
        );
        assert_eq!(
            a.version_hint,
            Some((
                "_iceberg/trade/metadata/version-hint.text".into(),
                b"2".to_vec()
            ))
        );
        let v2: Json = serde_json::from_slice(&a.immutable[5].1).unwrap();
        assert_eq!(v2["format-version"], 2);
        assert_eq!(v2["current-snapshot-id"], 22);
        assert_eq!(v2["snapshots"].as_array().unwrap().len(), 2);
        assert_eq!(v2["snapshots"][1]["parent-snapshot-id"], 11);
        assert_eq!(v2["snapshots"][1]["summary"]["total-records"], "31");
        assert_eq!(
            v2["snapshots"][1]["manifest-list"],
            "s3://b/hyperliquid-normalized/_iceberg/trade/metadata/snap-22-1.avro"
        );
        assert_eq!(
            v2["metadata-log"][0]["metadata-file"],
            "s3://b/hyperliquid-normalized/_iceberg/trade/metadata/v1.metadata.json"
        );
        assert_eq!(v2["partition-specs"][0]["fields"][2]["transform"], "day");
        // Every schema field has a unique ID up to last-column-id.
        let mut ids = Vec::new();
        fn collect(t: &Json, ids: &mut Vec<i64>) {
            if let Some(fields) = t.get("fields").and_then(Json::as_array) {
                for f in fields {
                    ids.push(f["id"].as_i64().unwrap());
                    collect(&f["type"], ids);
                }
            }
            if let Some(e) = t.get("element-id").and_then(Json::as_i64) {
                ids.push(e);
                collect(&t["element"], ids);
            }
        }
        collect(&v2["schemas"][0], &mut ids);
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert_eq!(v2["last-column-id"].as_i64().unwrap(), *ids.last().unwrap());
        let uuid = v2["table-uuid"].as_str().unwrap();
        assert_eq!(uuid.len(), 36);
        // Manifests embed absolute data file paths.
        let manifest = &a.immutable[0].1;
        let needle = b"s3://b/hyperliquid-normalized/kind=trade/";
        assert!(manifest.windows(needle.len()).any(|w| w == needle));
        assert!(
            build_table("a", "s3://x", RecordKind::Trade, &[])
                .unwrap()
                .immutable
                .is_empty()
        );
    }

    #[test]
    fn nested_kinds_have_list_types() {
        let snaps = vec![snap(1, 1, "2026-01-01", 1)];
        let t = build_table("a", "file:///tmp/a", RecordKind::BookSnapshot, &snaps).unwrap();
        let meta: Json = serde_json::from_slice(&t.immutable[2].1).unwrap();
        let fields = meta["schemas"][0]["fields"].as_array().unwrap();
        let bids = fields.iter().find(|f| f["name"] == "bids").unwrap();
        assert_eq!(bids["type"]["type"], "list");
        assert_eq!(bids["type"]["element"]["type"], "struct");
        let mapping: Json = serde_json::from_str(
            meta["properties"]["schema.name-mapping.default"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(
            mapping
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["names"][0] == "bids" && m["fields"][0]["fields"].is_array())
        );
        let bar = build_table("a", "s3://x", RecordKind::Bar, &snaps).unwrap();
        let meta: Json = serde_json::from_slice(&bar.immutable[2].1).unwrap();
        let start_id = meta["schemas"][0]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "start_ms")
            .unwrap()["id"]
            .clone();
        assert_eq!(
            meta["partition-specs"][0]["fields"][2]["source-id"],
            start_id
        );
    }

    #[test]
    fn snapshot_ids_are_positive() {
        assert_eq!(snapshot_id(&[0xff; 32]), i64::MAX);
        assert_eq!(snapshot_id(&[0; 32]), 0);
    }
}
