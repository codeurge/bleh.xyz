//! Backfill: idempotent, resumable, partitions written once and complete
//! across year boundaries, notices carried to the manifest. Bodies come
//! from synthetic fixtures through a URL map; no network.
#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use common::{fixture, zip_of};
use mesh_types::Record;
use reference_pollers::backfill::{Fetch, Options, run_source};
use reference_pollers::metrics::Metrics;
use reference_pollers::source::SourceId;
use reference_pollers::sources::{Bases, reader};
use reference_pollers::xlsx;

fn fetch_map() -> Fetch {
    Fetch::Map(Arc::new(|url: &str| -> Option<Bytes> {
        let body = if url.contains("/secured/all/") {
            fixture("nyfed/secured_all.json")
        } else if url.contains("/secured/sofrai/") {
            fixture("nyfed/sofrai.json")
        } else if url.contains("/unsecured/effr/") {
            fixture("nyfed/effr.json")
        } else if url.contains("/unsecured/obfr/") {
            fixture("nyfed/obfr.json")
        } else if url.ends_with("/deacot2025.zip") {
            zip_of(&[("annual.txt", &fixture("cot/legacy_fut_2025.txt"))])
        } else if url.ends_with("/deacot2026.zip") {
            zip_of(&[("annual.txt", &fixture("cot/legacy_fut_2026.txt"))])
        } else if url.ends_with("/deahistfo2026.zip") {
            zip_of(&[("annualof.txt", &fixture("cot/legacy_com_2026.txt"))])
        } else if url.ends_with("/com_disagg_txt_2026.zip") {
            zip_of(&[("c_year.txt", &fixture("cot/disagg_com_2026.txt"))])
        } else if url.ends_with("/dea_cit_txt_2026.zip") {
            zip_of(&[("annualci.txt", &fixture("cot/cit_2026.txt"))])
        } else if url.contains("reportTypeId=13060") {
            fixture("ercot/listing_13060.html")
        } else if url.contains("doclookupId=800025") {
            let rows: Vec<Vec<String>> = [
                vec![
                    "Delivery Date",
                    "Hour Ending",
                    "Repeated Hour Flag",
                    "Settlement Point",
                    "Settlement Point Price",
                ],
                vec!["06/02/2025", "01:00", "N", "HB_NORTH", "21.50"],
                vec!["06/02/2025", "02:00", "N", "HB_NORTH", "20.75"],
                vec!["12/31/2025", "24:00", "N", "HB_NORTH", "30.00"],
            ]
            .iter()
            .map(|r| r.iter().map(|c| (*c).to_owned()).collect())
            .collect();
            zip_of(&[(
                "DAMLZHBSPP_2025.xlsx",
                &xlsx::write_minimal(&[rows]).unwrap(),
            )])
        } else {
            return None;
        };
        Some(Bytes::from(body))
    }))
}

fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "parquet") {
                out.insert(
                    p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    out
}

fn opts(root: &Path, state: &Path, from: i32, to: i32, current: i32) -> Options {
    Options {
        archive_root: root.to_path_buf(),
        state_dir: state.to_path_buf(),
        from_year: Some(from),
        to_year: to,
        current_year: current,
    }
}

#[tokio::test]
async fn backfill_is_idempotent_and_resumable() {
    let root = tempfile::tempdir().unwrap();
    let s1 = tempfile::tempdir().unwrap();
    let s2 = tempfile::tempdir().unwrap();
    let r = reader(SourceId::NyFed, &Bases::default());
    let m = Metrics::new();
    let first = run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root.path(), s1.path(), 2025, 2026, 2026),
        &m,
    )
    .await
    .unwrap();
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .all(|u| !u.skipped && u.records > 0 && u.written > 0)
    );
    let files = tree(root.path());
    assert!(!files.is_empty());
    // Partitions hold backfill records with the source's admission.
    let any = files.keys().next().unwrap();
    let part = archive::partition::read_parquet(Bytes::from(files[any].clone())).unwrap();
    for rec in &part.records {
        assert!(rec.prov().flags.contains(mesh_types::Flag::Backfill));
        assert_eq!(rec.prov().admission.as_str(), "ADM-0007");
    }

    // From fresh state: the same files, already present, byte for byte.
    let second = run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root.path(), s2.path(), 2025, 2026, 2026),
        &m,
    )
    .await
    .unwrap();
    assert!(
        second
            .iter()
            .all(|u| u.written == 0 && u.already_present > 0),
        "{second:?}"
    );
    assert_eq!(tree(root.path()), files);

    // With the first state: finished years are skipped.
    let third = run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root.path(), s1.path(), 2025, 2026, 2026),
        &m,
    )
    .await
    .unwrap();
    assert!(third.iter().all(|u| u.skipped));

    // A record in another root from a fresh, partial run is identical.
    let root2 = tempfile::tempdir().unwrap();
    let s3 = tempfile::tempdir().unwrap();
    run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root2.path(), s3.path(), 2025, 2025, 2026),
        &m,
    )
    .await
    .unwrap();
    let partial = tree(root2.path());
    for (k, v) in &partial {
        assert_eq!(files.get(k), Some(v), "{k}");
    }
}

#[tokio::test]
async fn backfill_carries_partitions_across_years() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let r = reader(SourceId::Cot, &Bases::default());
    let m = Metrics::new();
    // 2025 only: the 30 December report is published on 2 January 2026, so
    // its partition stays open.
    let u = run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root.path(), state.path(), 2025, 2025, 2027),
        &m,
    )
    .await
    .unwrap();
    assert_eq!(u[0].carried, 11, "three markets' 2025-12-30 categories");
    let files = tree(root.path());
    assert!(files.keys().all(|k| !k.contains("date=2026-01-02")));
    assert!(files.keys().any(|k| k.contains("date=2025-12-19")));
    // 2026: the carried records land in the 2026-01-02 partition with
    // nothing missing, and combined records are flagged.
    let u = run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root.path(), state.path(), 2025, 2026, 2027),
        &m,
    )
    .await
    .unwrap();
    assert!(u[0].skipped);
    assert_eq!(u[1].carried, 0);
    let files = tree(root.path());
    let jan2: Vec<Record> = files
        .iter()
        .filter(|(k, _)| k.contains("date=2026-01-02"))
        .flat_map(|(_, v)| {
            archive::partition::read_parquet(Bytes::from(v.clone()))
                .unwrap()
                .records
        })
        .collect();
    assert_eq!(jan2.len(), 11);
    assert!(
        jan2.iter()
            .all(|r| matches!(r, Record::Positioning(p) if p.as_of_date == "2025-12-30"))
    );
    let jan9: Vec<Record> = files
        .iter()
        .filter(|(k, _)| k.contains("date=2026-01-09"))
        .flat_map(|(_, v)| {
            archive::partition::read_parquet(Bytes::from(v.clone()))
                .unwrap()
                .records
        })
        .collect();
    assert!(jan9.iter().any(common::flagged), "combined");
    assert!(jan9.iter().any(|r| !common::flagged(r)), "futures only");
}

#[tokio::test]
async fn ercot_workbook_backfill_and_notices_reach_the_manifest() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let m = Metrics::new();
    let ercot = reader(SourceId::Ercot, &Bases::default());
    let u = run_source(
        ercot.as_ref(),
        &fetch_map(),
        &opts(root.path(), state.path(), 2025, 2025, 2027),
        &m,
    )
    .await
    .unwrap();
    assert_eq!(u[0].records, 3);
    // The last hour of 2025 is published on 30 December: written. Nothing
    // of 2025 is left open past the year (lead of one day).
    assert_eq!(u[0].carried, 0);
    let files = tree(root.path());
    assert!(
        files
            .keys()
            .any(|k| k.contains("instrument=ercot~ref~DAM_SPP.HB_NORTH/date=2025-06-01"))
    );
    let ny = reader(SourceId::NyFed, &Bases::default());
    run_source(
        ny.as_ref(),
        &fetch_map(),
        &opts(root.path(), state.path(), 2026, 2026, 2026),
        &m,
    )
    .await
    .unwrap();
    let notices: Vec<mesh_types::SourceNotice> =
        serde_json::from_slice(&std::fs::read(root.path().join("_notices.json")).unwrap()).unwrap();
    let adms: Vec<&str> = notices.iter().map(|n| n.admission.as_str()).collect();
    assert_eq!(adms, vec!["ADM-0007", "ADM-0013"]);
    let key = mesh_types::SigningKey::from_bytes(&[3u8; 32]).unwrap();
    let built = archive::manifest::build(
        root.path(),
        &archive::manifest::BuildOptions {
            archive: "reference-backfill".into(),
            day: "2026-03-17".parse().unwrap(),
            web_seeds: vec!["https://mirror.example.org/".into()],
            trackers: vec![],
            created_ns: mesh_types::UnixNanos(1_773_800_000_000_000_000),
            signer: key.address(),
            next_signer: None,
            previous: None,
        },
    )
    .unwrap();
    assert_eq!(built.manifest.notices, notices);
    assert!(built.manifest.notices[0].notice.contains("newyorkfed.org"));
}

#[tokio::test]
async fn feeder_has_no_backfill_channel() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let r = reader(SourceId::UsdaFeeder, &Bases::default());
    let u = run_source(
        r.as_ref(),
        &fetch_map(),
        &opts(root.path(), state.path(), 2025, 2026, 2026),
        &Metrics::new(),
    )
    .await
    .unwrap();
    assert!(u.is_empty());
}
