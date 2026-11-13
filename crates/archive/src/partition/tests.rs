use bytes::Bytes;
use parquet::file::reader::{FileReader, SerializedFileReader};

use mesh_types::{BookAction, Record, RecordKind};

use super::*;
use crate::testutil::{self, DAY_MS, every_kind, trade};

fn by_kind(kind: RecordKind) -> Vec<Record> {
    let mut v: Vec<Record> = every_kind()
        .into_iter()
        .filter(|r| r.kind() == kind)
        .collect();
    v.sort_by(Record::canonical_cmp);
    v
}

#[test]
fn arrow_round_trip_every_kind() {
    for kind in RecordKind::ALL {
        let recs = by_kind(kind);
        assert!(!recs.is_empty(), "{kind}");
        let batch = convert::encode(kind, &recs).unwrap();
        assert_eq!(batch.num_rows(), recs.len());
        assert_eq!(convert::decode(kind, &batch).unwrap(), recs, "{kind}");
        // Empty batches work too.
        let empty = convert::encode(kind, &[]).unwrap();
        assert_eq!(empty.num_rows(), 0);
    }
}

#[test]
fn parquet_round_trip_and_pinned_metadata() {
    for kind in RecordKind::ALL {
        let recs = by_kind(kind);
        let (bytes, hash) = write_parquet(kind, &recs).unwrap();
        let back = read_parquet(Bytes::from(bytes.clone())).unwrap();
        assert_eq!(back.kind, kind);
        assert_eq!(back.records, recs, "{kind}");
        assert_eq!(back.content_hash, hash);
        assert!(back.consistent());
        assert_eq!(hash, mesh_types::content_hash(&recs).unwrap());

        let reader = SerializedFileReader::new(Bytes::from(bytes.clone())).unwrap();
        let meta = reader.metadata().file_metadata();
        assert_eq!(meta.created_by(), Some(CREATED_BY));
        let keys: Vec<&str> = meta
            .key_value_metadata()
            .unwrap()
            .iter()
            .map(|k| k.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "bleh.schema",
                "bleh.kind",
                "bleh.content_hash",
                "bleh.record_count"
            ]
        );
        // Field IDs are written for Iceberg.
        let schema = meta.schema_descr();
        for col in schema.columns() {
            assert!(
                col.self_type().get_basic_info().has_id(),
                "{kind}: {}",
                col.path()
            );
        }
        // Byte-identical on a second write.
        assert_eq!(write_parquet(kind, &recs).unwrap().0, bytes, "{kind}");
    }
}

#[test]
fn row_groups_are_fixed_size() {
    let n = ROW_GROUP_ROWS + 3;
    let recs: Vec<Record> = (0..n)
        .map(|i| {
            let i = u32::try_from(i).unwrap();
            trade(
                "hyperliquid:perp:SYN",
                u64::from(i / 4),
                i % 4,
                DAY_MS + i64::from(i),
            )
        })
        .collect();
    let (bytes, _) = write_parquet(RecordKind::Trade, &recs).unwrap();
    let reader = SerializedFileReader::new(Bytes::from(bytes)).unwrap();
    let rgs: Vec<i64> = reader
        .metadata()
        .row_groups()
        .iter()
        .map(|g| g.num_rows())
        .collect();
    assert_eq!(rgs, vec![ROW_GROUP_ROWS as i64, 3]);
    // Page index (column and offset indexes) is written for every column.
    for rg in reader.metadata().row_groups() {
        for col in rg.columns() {
            assert!(col.column_index_offset().is_some(), "{}", col.column_path());
            assert!(col.offset_index_offset().is_some(), "{}", col.column_path());
        }
    }
}

#[test]
fn scale_mismatch_within_a_row_is_rejected() {
    let r = testutil::book_event(
        1,
        0,
        BookAction::Fill {
            orig_size: Some(testutil::sz("1.00")),
            new_size: testutil::sz("0.500"),
        },
    );
    assert!(convert::encode(RecordKind::BookEvent, &[r]).is_err());
    let t = trade("hyperliquid:perp:SYN", 1, 0, DAY_MS);
    assert!(convert::encode(RecordKind::BookEvent, &[t]).is_err());
}

#[test]
fn plan_groups_sorts_and_rejects_duplicates() {
    let a = trade("hyperliquid:perp:SYN", 2, 0, DAY_MS);
    let b = trade("hyperliquid:perp:SYN", 1, 5, DAY_MS);
    let c = trade("hyperliquid:perp:SYN", 1, 0, DAY_MS + 86_400_000);
    let groups = plan(vec![a.clone(), b.clone(), c]).unwrap();
    assert_eq!(groups.len(), 2);
    let first = groups.values().next().unwrap();
    assert_eq!(first, &vec![b, a.clone()]);
    assert!(plan(vec![a.clone(), a]).is_err());
}

#[test]
fn write_partitions_is_write_once() {
    let dir = tempfile::tempdir().unwrap();
    let files = write_partitions(dir.path(), every_kind(), &PartitionOptions::default()).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(
        &"kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~syn~IDX/date=2026-01-01/part-00000.parquet"
    ));
    assert!(paths.contains(
        &"kind=swap_print/venue=cftc-part43/instrument=cftc-part43~swap~USD-SOFR/date=2026-01-01/part-00000.parquet"
    ));
    assert_eq!(
        files
            .iter()
            .map(|f| f.key.kind)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        RecordKind::ALL.len()
    );
    for f in &files {
        let bytes = std::fs::read(dir.path().join(&f.path)).unwrap();
        assert_eq!(Sha256Digest::of(&bytes), f.sha256);
        assert!(!f.already_present);
    }
    // Rewriting identical content is accepted.
    let again = write_partitions(dir.path(), every_kind(), &PartitionOptions::default()).unwrap();
    assert!(again.iter().all(|f| f.already_present));
    assert_eq!(
        again.iter().map(|f| &f.sha256).collect::<Vec<_>>(),
        files.iter().map(|f| &f.sha256).collect::<Vec<_>>()
    );
    // Different content for the same partition is a conflict...
    let changed = vec![trade("hyperliquid:perp:syn:IDX", 6, 1, DAY_MS + 200)];
    let err = write_partitions(dir.path(), changed.clone(), &PartitionOptions::default());
    assert!(matches!(err, Err(Error::Conflict { .. })), "{err:?}");
    // ...unless written as a correction under a new part number.
    let corr = write_partitions(
        dir.path(),
        changed,
        &PartitionOptions {
            existing: Existing::NewParts,
            ..PartitionOptions::default()
        },
    )
    .unwrap();
    assert_eq!(corr[0].part, 1);
    assert!(corr[0].path.ends_with("part-00001.parquet"));
    let day = "2026-01-01".parse().unwrap();
    assert_eq!(
        day_partitions(dir.path(), day).unwrap().len(),
        files.len() + 1
    );
}

#[test]
fn invalid_records_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut bad = trade("hyperliquid:perp:SYN", 1, 0, DAY_MS);
    if let Record::Trade(t) = &mut bad {
        t.size = testutil::sz("0");
    }
    assert!(write_partitions(dir.path(), vec![bad], &PartitionOptions::default()).is_err());
}

#[test]
fn record_days() {
    let recs = every_kind();
    for r in &recs {
        assert_eq!(record_day(r).to_string(), "2026-01-01");
    }
}

/// `docs/specs/observation-v0.arrow-schema.json`, relative to this crate.
const ARROW_SCHEMA_DOC: &str = "../../docs/specs/observation-v0.arrow-schema.json";

/// Compact JSON with keys in reading order (`kind`, `name`, `type`,
/// `nullable`, then the rest by name).
fn compact(v: &serde_json::Value) -> String {
    const FIRST: [&str; 4] = ["kind", "name", "type", "nullable"];
    match v {
        serde_json::Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by_key(|k| {
                (
                    FIRST.iter().position(|f| f == k).unwrap_or(FIRST.len()),
                    (*k).clone(),
                )
            });
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}: {}", serde_json::Value::from(k.clone()), compact(&o[k])))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        serde_json::Value::Array(a) => {
            format!("[{}]", a.iter().map(compact).collect::<Vec<_>>().join(", "))
        }
        other => other.to_string(),
    }
}

/// The published Arrow schema document, generated from
/// [`convert::writer_schema`]: one kind per block, one column per line.
fn arrow_schema_doc() -> String {
    let comment = "Generated from crates/archive (partition::convert::writer_schema); do not edit. \
                   Regenerate with BLEH_UPDATE_SPECS=1 cargo test -p archive arrow_schema_doc. \
                   Format: observation-v0.md section 9.";
    let mut out = String::from("{\n");
    out += &format!("  \"$comment\": {},\n", serde_json::Value::from(comment));
    out += &format!(
        "  \"schema\": {},\n  \"kinds\": [\n",
        serde_json::Value::from(mesh_types::OBSERVATION_SCHEMA)
    );
    let kinds: Vec<String> = RecordKind::ALL
        .into_iter()
        .map(|kind| {
            let d = mesh_types::arrow::describe(&convert::writer_schema(kind));
            let fields: Vec<String> = d["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| format!("        {}", compact(f)))
                .collect();
            format!(
                "    {{\n      \"kind\": {},\n      \"metadata\": {},\n      \"fields\": [\n{}\n      ]\n    }}",
                serde_json::Value::from(kind.as_str()),
                compact(&d["metadata"]),
                fields.join(",\n")
            )
        })
        .collect();
    out += &kinds.join(",\n");
    out += "\n  ]\n}\n";
    // Still one JSON document.
    serde_json::from_str::<serde_json::Value>(&out).unwrap();
    out
}

/// The committed Arrow schema document is what the archive writer uses.
/// `BLEH_UPDATE_SPECS=1` rewrites it.
#[test]
fn arrow_schema_doc_matches_writer_schema() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ARROW_SCHEMA_DOC);
    let want = arrow_schema_doc();
    if std::env::var_os("BLEH_UPDATE_SPECS").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap();
    assert!(
        have == want,
        "{} is stale; regenerate with BLEH_UPDATE_SPECS=1 cargo test -p archive arrow_schema_doc",
        path.display()
    );
}

/// What a Parquet file keeps of a field: name, type, nullability and
/// field ID. `bleh.encoding` is not stored (the writer skips the embedded
/// Arrow schema) and UTC reads back as `+00:00`.
fn physical(field: &serde_json::Value) -> serde_json::Value {
    let ty = match &field["type"] {
        serde_json::Value::String(s) => s.replace("+00:00", "UTC").into(),
        serde_json::Value::Object(o) if o.contains_key("list") => {
            serde_json::json!({ "list": physical(&o["list"]) })
        }
        serde_json::Value::Object(o) if o.contains_key("struct") => serde_json::json!({
            "struct": o["struct"].as_array().unwrap().iter().map(physical).collect::<Vec<_>>()
        }),
        other => other.clone(),
    };
    serde_json::json!({
        "name": field["name"],
        "type": ty,
        "nullable": field["nullable"],
        "field_id": field["metadata"]["PARQUET:field_id"],
    })
}

/// Partition files written by the archive writer, read back from Parquet
/// alone, have the columns of the published Arrow schema document.
#[test]
fn parquet_files_match_arrow_schema_doc() {
    let doc: serde_json::Value = serde_json::from_str(&arrow_schema_doc()).unwrap();
    for (kind, want) in RecordKind::ALL
        .into_iter()
        .zip(doc["kinds"].as_array().unwrap())
    {
        assert_eq!(want["kind"], kind.as_str());
        let (bytes, _) = write_parquet(kind, &by_kind(kind)).unwrap();
        let builder = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(bytes)).unwrap();
        let have = mesh_types::arrow::describe(builder.schema());
        assert_eq!(
            have["metadata"]["bleh.schema"],
            want["metadata"]["bleh.schema"]
        );
        assert_eq!(have["metadata"]["bleh.kind"], want["metadata"]["bleh.kind"]);
        let fields = |d: &serde_json::Value| -> Vec<serde_json::Value> {
            d["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(physical)
                .collect()
        };
        assert_eq!(fields(&have), fields(want), "{kind}");
    }
}
