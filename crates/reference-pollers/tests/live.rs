//! Live polling against a local mock server (no network): raw capture,
//! JSON lines, dedupe of unchanged bodies, revisions, fetch-once
//! documents, backoff on errors and offline replay.
#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;

use common::{fixture, zip_of};
use mesh_types::Record;
use reference_pollers::http::{Fetcher, HttpConfig};
use reference_pollers::live::SourcePoller;
use reference_pollers::metrics::Metrics;
use reference_pollers::obs::{Builder, Stamp};
use reference_pollers::output::{Emitter, JsonlWriter};
use reference_pollers::raw::RawStore;
use reference_pollers::revision::RevisionStore;
use reference_pollers::source::SourceId;
use reference_pollers::sources::{Bases, Target, reader};
use reference_pollers::time::Date;
use tokio::sync::{Mutex, mpsc};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 2026-03-16 23:00 UTC.
const NOW: i64 = 1_773_702_000_000;

struct Rig {
    _dirs: (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir),
    raw: std::path::PathBuf,
    out: std::path::PathBuf,
    poller: SourcePoller,
    metrics: Arc<Metrics>,
    rx: mpsc::Receiver<Record>,
}

fn rig(id: SourceId, server: &MockServer, key: &str) -> Rig {
    let (raw, state, out) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let metrics = Arc::new(Metrics::new());
    let fetcher = Fetcher::new(
        HttpConfig {
            min_interval: std::time::Duration::from_millis(1),
            max_retries: 1,
            backoff_base: std::time::Duration::from_millis(5),
            ..HttpConfig::default()
        },
        metrics.clone(),
    )
    .unwrap();
    let (tx, rx) = mpsc::channel(10_000);
    let emitter = Arc::new(Mutex::new(Emitter::new(
        Some(JsonlWriter::new(out.path())),
        Some(tx),
        metrics.clone(),
    )));
    let bases = Bases::parse(&[format!("{key}={}", server.uri())]).unwrap();
    let poller = SourcePoller::open(
        reader(id, &bases),
        fetcher,
        RawStore::new(raw.path()),
        state.path(),
        emitter,
        metrics.clone(),
        Date::of_ms(NOW),
    )
    .unwrap();
    Rig {
        raw: raw.path().to_path_buf(),
        out: out.path().to_path_buf(),
        _dirs: (raw, state, out),
        poller,
        metrics,
        rx,
    }
}

async fn mount(server: &MockServer, p: &str, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(server)
        .await;
}

fn jsonl_records(dir: &std::path::Path) -> Vec<Record> {
    let mut out = Vec::new();
    for day in std::fs::read_dir(dir).unwrap() {
        let day = day.unwrap().path();
        if !day.is_dir() {
            continue;
        }
        for f in std::fs::read_dir(&day).unwrap() {
            let text = std::fs::read_to_string(f.unwrap().path()).unwrap();
            out.extend(
                text.lines()
                    .map(|l| serde_json::from_str::<Record>(l).unwrap()),
            );
        }
    }
    out
}

fn raw_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if !p.to_string_lossy().ends_with(".meta.json") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

async fn mount_nyfed(server: &MockServer, secured: &str) {
    mount(
        server,
        "/api/rates/secured/all/search.json",
        fixture(secured),
    )
    .await;
    mount(
        server,
        "/api/rates/secured/sofrai/search.json",
        fixture("nyfed/sofrai.json"),
    )
    .await;
    mount(
        server,
        "/api/rates/unsecured/effr/search.json",
        fixture("nyfed/effr.json"),
    )
    .await;
    mount(
        server,
        "/api/rates/unsecured/obfr/search.json",
        fixture("nyfed/obfr.json"),
    )
    .await;
}

#[tokio::test]
async fn nyfed_live_cycle_dedupes_and_revises() {
    let server = MockServer::start().await;
    mount_nyfed(&server, "nyfed/secured_all.json").await;
    let mut rig = rig(SourceId::NyFed, &server, "nyfed");
    let r1 = rig.poller.cycle(NOW).await.unwrap();
    assert_eq!(r1.failures, 0);
    assert_eq!(r1.changed, 4);
    assert!(r1.records > 0);
    let written = jsonl_records(&rig.out);
    assert_eq!(written.len(), r1.records);
    // Only values within the live window (40 days before 2026-03-16).
    assert!(written.iter().all(|r| match r {
        Record::ReferenceValue(v) => v.as_of_date.as_str() >= "2026-02-04",
        _ => false,
    }));
    // Every record also went to the gateway channel.
    let mut sent = 0;
    while rig.rx.try_recv().is_ok() {
        sent += 1;
    }
    assert_eq!(sent, r1.records);
    // Raw capture: one body and sidecar per changed target.
    assert_eq!(raw_files(&rig.raw).len(), 4);

    // Same bodies: unchanged, nothing emitted.
    let r2 = rig.poller.cycle(NOW + 600_000).await.unwrap();
    assert_eq!((r2.changed, r2.records, r2.unchanged), (0, 0, 4));

    // The revision is emitted once, as revision 1.
    server.reset().await;
    mount_nyfed(&server, "nyfed/secured_all_revised.json").await;
    let r3 = rig.poller.cycle(NOW + 1_200_000).await.unwrap();
    assert_eq!((r3.changed, r3.records), (1, 1));
    let all = jsonl_records(&rig.out);
    let rev: Vec<_> = all
        .iter()
        .filter(|r| matches!(r, Record::ReferenceValue(v) if v.revision == 1))
        .collect();
    assert_eq!(rev.len(), 1);
    assert_eq!(
        rig.metrics
            .counter("ref_revisions_total", &[("source", "nyfed")]),
        1
    );

    // Offline replay of a captured file reproduces the emitted records.
    let file = raw_files(&rig.raw)
        .into_iter()
        .find(|p| p.to_string_lossy().contains("effr"))
        .unwrap();
    let (body, meta) = RawStore::load(&file).unwrap();
    let t = Target {
        key: meta.target.clone(),
        url: meta.url.clone(),
        kind: meta.kind.clone(),
        once: false,
    };
    let parsed = reader(SourceId::NyFed, &Bases::default())
        .parse(&t, &body)
        .unwrap();
    let window = Date::of_ms(NOW).add_days(-40);
    let obs = parsed
        .obs
        .into_iter()
        .filter(|o| o.as_of() >= window)
        .collect();
    let (replayed, _) = Builder::new(SourceId::NyFed)
        .build(
            obs,
            Stamp::Live {
                fetched_at: meta.fetched_at().unwrap(),
                last_modified_ms: None,
            },
            &mut RevisionStore::memory(),
        )
        .unwrap();
    let emitted_effr: Vec<&Record> = all
        .iter()
        .filter(|r| r.prov().instrument.symbol().starts_with("EFFR"))
        .collect();
    assert_eq!(replayed.len(), emitted_effr.len());
    for (a, b) in replayed.iter().zip(emitted_effr) {
        assert_eq!(a, b);
    }
}

#[tokio::test]
async fn ercot_documents_are_fetched_once() {
    let server = MockServer::start().await;
    let listing = String::from_utf8(fixture("ercot/listing_12331.html")).unwrap();
    let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = served.clone();
    // The listing page changes on every request (as a timestamp would),
    // so it is parsed again each cycle.
    Mock::given(method("GET"))
        .and(path("/misapp/GetReports.do"))
        .and(query_param("reportTypeId", "12331"))
        .respond_with(move |_: &wiremock::Request| {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_string(format!("{listing}<!-- {n} -->"))
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/misapp/GetReports.do"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><table></table></html>"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/misdownload/servlets/mirDownload"))
        .and(query_param("doclookupId", "900001"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(zip_of(&[("a.csv", &fixture("ercot/dam_spp.csv"))])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut rig = rig(SourceId::Ercot, &server, "ercot");
    let r1 = rig.poller.cycle(NOW).await.unwrap();
    assert_eq!(r1.records, 48);
    assert_eq!(r1.failures, 0);
    // The listing changes (a new timestamp in the page) but the document
    // is not fetched again: wiremock verifies exactly one download.
    let r2 = rig.poller.cycle(NOW + 300_000).await.unwrap();
    assert_eq!(r2.records, 0);
    assert_eq!(served.load(std::sync::atomic::Ordering::SeqCst), 2);
    let recs = jsonl_records(&rig.out);
    assert!(
        recs.iter()
            .all(|r| matches!(r, Record::ReferenceValue(v) if v.period_start_ms.is_some()))
    );
}

#[tokio::test]
async fn failures_are_counted_and_partial_cycles_continue() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/api/rates/secured/all/search.json",
        fixture("nyfed/secured_all.json"),
    )
    .await;
    mount(
        &server,
        "/api/rates/secured/sofrai/search.json",
        b"not json".to_vec(),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/rates/unsecured/effr/search.json"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/rates/unsecured/obfr/search.json"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let mut rig = rig(SourceId::NyFed, &server, "nyfed");
    let r = rig.poller.cycle(NOW).await.unwrap();
    assert_eq!(
        r.failures, 2,
        "unparseable, retried 503 (a 404 is not yet published)"
    );
    assert!(r.records > 0, "the good target still emits");
    assert_eq!(
        rig.metrics
            .counter("ref_parse_errors_total", &[("source", "nyfed")]),
        1
    );
    assert!(
        rig.metrics.counter(
            "ref_fetches_total",
            &[("source", "nyfed"), ("outcome", "retry")]
        ) >= 1
    );
}

#[tokio::test]
async fn stale_feeder_file_is_flagged() {
    let server = MockServer::start().await;
    mount(
        &server,
        "/mnreports/sj_ls850.txt",
        fixture("feeder/sj_ls850_stale.txt"),
    )
    .await;
    let mut rig = rig(SourceId::UsdaFeeder, &server, "ams");
    // 2026-03-16: the file is dated 2026-01-02, older than ten days and
    // outside the live window, so it is reported stale and not emitted.
    let r = rig.poller.cycle(NOW).await.unwrap();
    assert_eq!(r.records, 0);
    assert_eq!(
        rig.metrics
            .counter("ref_stale_reports_total", &[("source", "usda-feeder")]),
        1
    );
    server.reset().await;
    mount(
        &server,
        "/mnreports/sj_ls850.txt",
        fixture("feeder/sj_ls850.txt"),
    )
    .await;
    let later = Date::new(2026, 3, 12).unwrap().start_ms() + 20 * 3_600_000;
    let r = rig.poller.cycle(later).await.unwrap();
    assert_eq!(r.records, 16, "12 prices and 4 head counts");
    let much_later = Date::new(2026, 4, 1).unwrap().start_ms();
    server.reset().await;
    mount(
        &server,
        "/mnreports/sj_ls850.txt",
        fixture("feeder/sj_ls850.txt")
            .into_iter()
            .chain(b"\n".iter().copied())
            .collect(),
    )
    .await;
    let r = rig.poller.cycle(much_later).await.unwrap();
    assert_eq!(r.records, 0, "same values: nothing new");
    assert_eq!(
        rig.metrics
            .counter("ref_stale_reports_total", &[("source", "usda-feeder")]),
        2
    );
}
