//! Shared helpers. Fixtures are synthetic (fixtures/synthetic/reference/,
//! every value invented) and no test touches the network.
#![allow(dead_code, clippy::unwrap_used, unreachable_pub)]

use std::io::Write;
use std::path::PathBuf;

use mesh_types::{Record, UnixNanos};
use reference_pollers::obs::{Builder, Stamp};
use reference_pollers::revision::RevisionStore;
use reference_pollers::source::SourceId;
use reference_pollers::sources::{Parsed, Target};

pub fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/reference")
}

pub fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

pub fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        for (name, body) in members {
            z.start_file(*name, opts).unwrap();
            z.write_all(body).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

pub fn target(kind: &str) -> Target {
    Target::poll("test", "https://example.invalid/test", kind)
}

/// A fetch time in March 2026 (synthetic).
pub const FETCHED: UnixNanos = UnixNanos(1_773_700_000_000_000_000);

pub fn live() -> Stamp {
    Stamp::Live {
        fetched_at: FETCHED,
        last_modified_ms: None,
    }
}

/// Builds records with a fresh in-memory revision store and checks each
/// against validation and the field allowlist.
pub fn records(source: SourceId, parsed: Parsed, stamp: Stamp) -> Vec<Record> {
    records_with(source, parsed, stamp, &mut RevisionStore::memory())
}

pub fn records_with(
    source: SourceId,
    parsed: Parsed,
    stamp: Stamp,
    store: &mut RevisionStore,
) -> Vec<Record> {
    let (recs, _) = Builder::new(source)
        .build(parsed.obs, stamp, store)
        .unwrap();
    for r in &recs {
        r.validate().unwrap_or_else(|e| panic!("{e}"));
        mesh_types::allowlist::check_record(r).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.prov().admission.as_str(), source.admission_str());
        assert_eq!(r.prov().venue.as_str(), source.venue_str());
    }
    recs
}

pub fn symbols(recs: &[Record]) -> Vec<String> {
    let mut v: Vec<String> = recs
        .iter()
        .map(|r| r.prov().instrument.symbol().to_owned())
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn flagged(r: &Record) -> bool {
    r.prov()
        .flags
        .contains(mesh_types::Flag::ExchangeReferenced)
}

pub fn value_of(r: &Record) -> String {
    match r {
        Record::ReferenceValue(v) => v.value.to_string(),
        _ => String::new(),
    }
}
