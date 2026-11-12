//! Poller tests against a mocked DTCC site (wiremock). Synthetic data only.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use common::{Report, base_trade};
use mesh_types::{Flag, Record, SwapPrint, UnixMillis};
use swap_pollers::checkpoint::CheckpointStore;
use swap_pollers::config::Args;
use swap_pollers::dedupe::{self, SeenStore};
use swap_pollers::http::{Fetcher, HttpConfig};
use swap_pollers::metrics::Metrics;
use swap_pollers::output::{Emitter, JsonlWriter};
use swap_pollers::poller::{DtccConfig, DtccPoller};
use swap_pollers::raw::{RawMeta, RawStore};
use swap_pollers::repository::DtccBucket;
use swap_pollers::server::Health;
use tokio::sync::{mpsc, watch};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn today() -> String {
    dedupe::day_of(UnixMillis::now().0)
}

fn day_ago(n: i64) -> String {
    dedupe::day_of(UnixMillis::now().0 - n * 86_400_000)
}

fn us(day: &str) -> String {
    day.replace('-', "_")
}

fn quick_http() -> HttpConfig {
    HttpConfig {
        min_interval: Duration::ZERO,
        backoff_base: Duration::from_millis(5),
        backoff_max: Duration::from_millis(20),
        max_retries: 3,
        timeout: Duration::from_secs(10),
        ..HttpConfig::default()
    }
}

struct Env {
    dir: tempfile::TempDir,
    server: MockServer,
}

struct Built {
    poller: DtccPoller,
    metrics: Arc<Metrics>,
    rx: mpsc::Receiver<SwapPrint>,
}

impl Env {
    async fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            server: MockServer::start().await,
        }
    }

    fn raw_dir(&self) -> PathBuf {
        self.dir.path().join("raw")
    }
    fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }
    fn out_dir(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    fn build(&self, buckets: &[&str], cumulative_days: u32, dedupe_days: u32) -> Built {
        let metrics = Arc::new(Metrics::new());
        let cfg = DtccConfig {
            api_base: format!("{}/api", self.server.uri()),
            data_base: Some(self.server.uri()),
            buckets: buckets.iter().map(|b| b.parse().unwrap()).collect(),
            cumulative_days,
            ..DtccConfig::default()
        };
        let fetcher = Fetcher::new(quick_http(), metrics.clone()).unwrap();
        let raw = RawStore::new(self.raw_dir(), None, metrics.clone());
        let cp = CheckpointStore::open(self.state_dir().join("checkpoint.json")).unwrap();
        let seen = SeenStore::open(self.state_dir().join("seen"), dedupe_days, &today()).unwrap();
        let (tx, rx) = mpsc::channel(10_000);
        let emitter = Emitter::new(
            seen,
            Some(JsonlWriter::new(self.out_dir())),
            Some(tx),
            metrics.clone(),
        );
        let health = Arc::new(Health::new(Duration::from_secs(600)));
        let poller = DtccPoller::new(cfg, fetcher, raw, cp, emitter, metrics.clone(), health);
        Built {
            poller,
            metrics,
            rx,
        }
    }

    fn jsonl(&self, day: &str) -> Vec<Record> {
        let p = self
            .out_dir()
            .join(format!("date={day}"))
            .join("swap_print.jsonl");
        std::fs::read_to_string(p)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    async fn requests_to(&self, p: &str) -> usize {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| r.url.path() == p)
            .count()
    }
}

fn entry(
    regime_path: &str,
    kind_dir: &str,
    base: &str,
    name: &str,
    slice_id: u64,
    dissem: &str,
) -> serde_json::Value {
    serde_json::json!({
        "dissemDTM": dissem,
        "endTs": dissem,
        "startTs": dissem,
        "fileName": name,
        "fullFilePath": format!("{base}/{regime_path}/{kind_dir}/{name}"),
        "rowCount": 1,
        "sliceId": slice_id,
    })
}

fn rates_report(ids: &[&'static str], day: &str) -> Report {
    let exec: &'static str = Box::leak(format!("{day}T10:00:00Z").into_boxed_str());
    let dissem: &'static str = Box::leak(format!("{day}T10:00:05Z").into_boxed_str());
    let mut r = Report::standard();
    for id in ids {
        let mut row = base_trade(id, "IR", exec, dissem);
        row.push(("Unique Product Identifier", "QZSYN0000100"));
        row.push(("Fixed rate-Leg 1", "0.0375"));
        row.push(("Notional amount-Leg 1", "10,000,000"));
        row.push(("Notional currency-Leg 1", "USD"));
        r = r.row(&row);
    }
    r
}

fn credit_report(ids: &[&'static str], day: &str) -> Report {
    let exec: &'static str = Box::leak(format!("{day}T11:00:00Z").into_boxed_str());
    let mut r = Report::standard();
    for id in ids {
        let mut row = base_trade(id, "CR", exec, exec);
        row.push(("Unique Product Identifier", "QZSYNCDS0100"));
        row.push(("Spread-Leg 1", "0.0062"));
        row.push(("Spread notation-Leg 1", "3"));
        r = r.row(&row);
    }
    r
}

async fn mount_file(server: &MockServer, url_path: &str, body: Vec<u8>) {
    Mock::given(method("GET"))
        .and(path(url_path.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
        .mount(server)
        .await;
}

fn drain(rx: &mut mpsc::Receiver<SwapPrint>) -> Vec<SwapPrint> {
    let mut v = Vec::new();
    while let Ok(p) = rx.try_recv() {
        v.push(p);
    }
    v
}

#[tokio::test]
async fn captures_slices_end_to_end() {
    let env = Env::new().await;
    let base = env.server.uri();
    let d = today();
    let ir1 = format!("CFTC_SLICE_RATES_{}_1.zip", us(&d));
    let ir2 = format!("CFTC_SLICE_RATES_{}_2.zip", us(&d));
    let cr1 = format!("SEC_SLICE_CREDITS_{}_1.zip", us(&d));
    let dissem = format!("{d}T10:00:06Z");
    let listing = serde_json::json!({
        // Newest first, as DTCC lists them.
        "CFTC_IR": [
            entry("cftc", "slices", &base, &ir2, 11, &dissem),
            entry("cftc", "slices", &base, &ir1, 10, &dissem),
        ],
        "SEC_CR": [entry("sec", "slices", &base, &cr1, 12, &dissem)],
        "CA_IR": [entry("ca", "slices", &base, "CA_SLICE_RATES_2026_11_12_1.zip", 13, &dissem)],
        "CFTC_FX": [entry("cftc", "slices", &base, "CFTC_SLICE_FOREX_2026_11_12_1.zip", 14, &dissem)],
    });
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304))
        .with_priority(1)
        .mount(&env.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"v1\"")
                .insert_header("Last-Modified", "Thu, 12 Nov 2026 10:00:07 GMT")
                .set_body_json(listing),
        )
        .mount(&env.server)
        .await;
    let zip1 = rates_report(&["1", "2"], &d).zip("ir1");
    let zip2 = rates_report(&["3"], &d).zip("ir2");
    let zip3 = credit_report(&["4", "5"], &d).zip("cr1");
    mount_file(&env.server, &format!("/cftc/slices/{ir1}"), zip1.clone()).await;
    mount_file(&env.server, &format!("/cftc/slices/{ir2}"), zip2).await;
    mount_file(&env.server, &format!("/sec/slices/{cr1}"), zip3).await;

    let mut b = env.build(&["CFTC_IR", "SEC_CR"], 0, 3);
    let s = b.poller.poll_slices().await.unwrap();
    assert!(!s.not_modified);
    assert_eq!((s.files, s.failed, s.records), (3, 0, 5));

    // Raw capture is verbatim, with a sidecar.
    let raw1 = env
        .raw_dir()
        .join(format!("dtcc-ddr/cftc/slices/{d}/{ir1}"));
    assert_eq!(std::fs::read(&raw1).unwrap(), zip1);
    let meta = RawMeta::load_for(&raw1).unwrap();
    assert_eq!(meta.entry.slice_id, Some(10));
    assert_eq!(meta.bucket, "CFTC_IR");
    assert_eq!(meta.bytes, zip1.len() as u64);
    assert_eq!(meta.sha256.len(), 64);
    assert!(
        env.raw_dir()
            .join(format!("dtcc-ddr/sec/slices/{d}/{cr1}"))
            .exists()
    );
    // Unadmitted (CA) and unconfigured (CFTC_FX) buckets are not fetched.
    assert_eq!(
        env.requests_to("/ca/slices/CA_SLICE_RATES_2026_11_12_1.zip")
            .await,
        0
    );
    assert_eq!(
        env.requests_to("/cftc/slices/CFTC_SLICE_FOREX_2026_11_12_1.zip")
            .await,
        0
    );

    // Records: JSON lines, gateway channel, oldest slice first.
    let lines = env.jsonl(&d);
    assert_eq!(lines.len(), 5);
    let sent = drain(&mut b.rx);
    assert_eq!(sent.len(), 5);
    let ir: Vec<&SwapPrint> = sent
        .iter()
        .filter(|p| p.prov.venue.as_str() == "cftc-part43")
        .collect();
    assert_eq!(ir[0].prov.seq.block_number, 10);
    assert_eq!(ir[2].prov.seq.block_number, 11);
    assert_eq!(ir[0].prov.ts_ingest_ns.0.to_string(), meta.fetched_at_ns);
    for r in &lines {
        mesh_types::allowlist::check_record(r).unwrap();
    }
    assert_eq!(b.metrics.counter_sum("swap_records_total"), 5);
    assert_eq!(b.metrics.counter_sum("swap_rows_total"), 5);

    // Checkpoint written; second poll is a 304.
    assert!(env.state_dir().join("checkpoint.json").exists());
    let s2 = b.poller.poll_slices().await.unwrap();
    assert!(s2.not_modified);
    assert_eq!(env.requests_to(&format!("/cftc/slices/{ir1}")).await, 1);
}

#[tokio::test]
async fn retries_transient_errors_and_failed_files_next_cycle() {
    let env = Env::new().await;
    let base = env.server.uri();
    let d = today();
    let a = format!("CFTC_SLICE_RATES_{}_1.zip", us(&d));
    let bname = format!("CFTC_SLICE_RATES_{}_2.zip", us(&d));
    let dissem = format!("{d}T10:00:06Z");
    let listing = serde_json::json!({
        "CFTC_IR": [entry("cftc", "slices", &base, &a, 1, &dissem), entry("cftc", "slices", &base, &bname, 2, &dissem)],
    });
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"x\"")
                .set_body_json(listing),
        )
        .mount(&env.server)
        .await;
    // A: one 503, then success.
    Mock::given(method("GET"))
        .and(path(format!("/cftc/slices/{a}")))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "0"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&env.server)
        .await;
    mount_file(
        &env.server,
        &format!("/cftc/slices/{a}"),
        rates_report(&["1"], &d).zip("a"),
    )
    .await;
    // B: 404 for now.
    let gone = Mock::given(method("GET"))
        .and(path(format!("/cftc/slices/{bname}")))
        .respond_with(ResponseTemplate::new(404))
        .mount_as_scoped(&env.server)
        .await;

    let mut b = env.build(&["CFTC_IR"], 0, 3);
    let s = b.poller.poll_slices().await.unwrap();
    assert_eq!((s.files, s.failed, s.records), (1, 1, 1));
    assert_eq!(
        b.metrics.counter(
            "swap_fetches_total",
            &[("target", "slice_file"), ("outcome", "retry")]
        ),
        1
    );
    // A failed file keeps the listing validators unset, so the listing is
    // fetched again next cycle without If-None-Match.
    let cp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(env.state_dir().join("checkpoint.json")).unwrap())
            .unwrap();
    assert!(cp["listings"].get("dashboard/Slice.json").is_none());

    drop(gone);
    mount_file(
        &env.server,
        &format!("/cftc/slices/{bname}"),
        rates_report(&["2"], &d).zip("b"),
    )
    .await;
    let s = b.poller.poll_slices().await.unwrap();
    assert_eq!((s.files, s.failed, s.records), (1, 0, 1));
    assert_eq!(env.requests_to(&format!("/cftc/slices/{a}")).await, 2);
    assert_eq!(env.jsonl(&d).len(), 2);
}

#[tokio::test]
async fn cumulative_dedupes_against_slices_and_flags_backfill() {
    let env = Env::new().await;
    let base = env.server.uri();
    let d0 = today();
    let d1 = day_ago(1);
    let d2 = day_ago(2);
    let d9 = day_ago(9);

    // Live slices covered day d2 in an earlier run (outside a 1-day
    // dedupe window).
    {
        let mut cp = CheckpointStore::open(env.state_dir().join("checkpoint.json")).unwrap();
        cp.state
            .slice_days
            .entry("CFTC_IR".into())
            .or_default()
            .insert(d2.clone());
        cp.save().await.unwrap();
    }

    let slice = format!("CFTC_SLICE_RATES_{}_1.zip", us(&d0));
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "CFTC_IR": [entry("cftc", "slices", &base, &slice, 100, &format!("{d0}T10:00:06Z"))],
        })))
        .mount(&env.server)
        .await;
    mount_file(
        &env.server,
        &format!("/cftc/slices/{slice}"),
        rates_report(&["1", "2"], &d0).zip("s"),
    )
    .await;

    let c0 = format!("CFTC_CUMULATIVE_RATES_{}.zip", us(&d0));
    let c1 = format!("CFTC_CUMULATIVE_RATES_{}.zip", us(&d1));
    let c2 = format!("CFTC_CUMULATIVE_RATES_{}.zip", us(&d2));
    let c9 = format!("CFTC_CUMULATIVE_RATES_{}.zip", us(&d9));
    Mock::given(method("GET"))
        .and(path("/dashboard/Cumulative.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "CFTC_IR": [
                entry("cftc", "eod", &base, &c0, 200, &format!("{d0}T23:59:00Z")),
                entry("cftc", "eod", &base, &c1, 199, &format!("{d1}T23:59:00Z")),
                entry("cftc", "eod", &base, &c2, 198, &format!("{d2}T23:59:00Z")),
                entry("cftc", "eod", &base, &c9, 191, &format!("{d9}T23:59:00Z")),
            ],
        })))
        .mount(&env.server)
        .await;
    mount_file(
        &env.server,
        &format!("/cftc/eod/{c0}"),
        rates_report(&["1", "2", "3"], &d0).zip("c0"),
    )
    .await;
    mount_file(
        &env.server,
        &format!("/cftc/eod/{c1}"),
        rates_report(&["11"], &d1).zip("c1"),
    )
    .await;
    mount_file(
        &env.server,
        &format!("/cftc/eod/{c2}"),
        rates_report(&["21"], &d2).zip("c2"),
    )
    .await;
    mount_file(
        &env.server,
        &format!("/cftc/eod/{c9}"),
        rates_report(&["91"], &d9).zip("c9"),
    )
    .await;

    let mut b = env.build(&["CFTC_IR"], 3, 1);
    assert_eq!(b.poller.poll_slices().await.unwrap().records, 2);
    let s = b.poller.poll_cumulative().await.unwrap();
    // c0: only id 3 is new; c1: no live coverage, emitted; c2: covered by
    // live slices and outside the dedupe window, archived only; c9: older
    // than cumulative_days, not fetched.
    assert_eq!((s.files, s.failed), (3, 0));
    assert_eq!(s.records, 2);
    assert_eq!(
        b.metrics
            .counter("swap_duplicates_total", &[("bucket", "CFTC_IR")]),
        2
    );
    let sent = drain(&mut b.rx);
    let ids: Vec<&str> = sent.iter().map(|p| p.dissemination_id.as_str()).collect();
    // End-of-day files are taken oldest slice ID first.
    assert_eq!(ids, vec!["1", "2", "11", "3"]);
    assert!(!sent[0].prov.flags.contains(Flag::Backfill));
    assert!(sent[2].prov.flags.contains(Flag::Backfill));
    assert!(sent[3].prov.flags.contains(Flag::Backfill));
    assert!(
        env.raw_dir()
            .join(format!("dtcc-ddr/cftc/eod/{d2}/{c2}"))
            .exists()
    );
    assert_eq!(
        b.metrics.counter(
            "swap_files_total",
            &[
                ("bucket", "CFTC_IR"),
                ("kind", "cumulative"),
                ("outcome", "archived_only")
            ]
        ),
        1
    );
    assert_eq!(env.requests_to(&format!("/cftc/eod/{c9}")).await, 0);
}

#[tokio::test]
async fn restart_resumes_from_checkpoint_and_raw() {
    let env = Env::new().await;
    let base = env.server.uri();
    let d = today();
    let f = format!("CFTC_SLICE_RATES_{}_1.zip", us(&d));
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "CFTC_IR": [entry("cftc", "slices", &base, &f, 1, &format!("{d}T10:00:06Z"))],
        })))
        .mount(&env.server)
        .await;
    mount_file(
        &env.server,
        &format!("/cftc/slices/{f}"),
        rates_report(&["1"], &d).zip("f"),
    )
    .await;
    {
        let mut b = env.build(&["CFTC_IR"], 0, 3);
        assert_eq!(b.poller.poll_slices().await.unwrap().records, 1);
    }
    // Restart with state intact: nothing new.
    {
        let mut b = env.build(&["CFTC_IR"], 0, 3);
        let s = b.poller.poll_slices().await.unwrap();
        assert_eq!((s.files, s.records), (0, 0));
    }
    // Lost checkpoint (crash before it was written): the raw file is reused,
    // not downloaded again, and dedupe stops a second emission.
    std::fs::remove_file(env.state_dir().join("checkpoint.json")).unwrap();
    {
        let mut b = env.build(&["CFTC_IR"], 0, 3);
        let s = b.poller.poll_slices().await.unwrap();
        assert_eq!((s.files, s.records), (1, 0));
        assert_eq!(
            b.metrics
                .counter("swap_duplicates_total", &[("bucket", "CFTC_IR")]),
            1
        );
    }
    assert_eq!(env.requests_to(&format!("/cftc/slices/{f}")).await, 1);
    assert_eq!(env.jsonl(&d).len(), 1);
}

#[tokio::test]
async fn unparseable_file_is_archived_not_retried() {
    let env = Env::new().await;
    let base = env.server.uri();
    let d = today();
    let f = format!("SEC_SLICE_EQUITIES_{}_1.zip", us(&d));
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "SEC_EQ": [entry("sec", "slices", &base, &f, 1, &format!("{d}T10:00:06Z"))],
        })))
        .mount(&env.server)
        .await;
    let csv = "Totally,Different,Layout\r\n1,2,3\r\n";
    mount_file(
        &env.server,
        &format!("/sec/slices/{f}"),
        common::zip_entries(&[("x.csv", csv.as_bytes())]),
    )
    .await;
    let mut b = env.build(&["SEC_EQ"], 0, 3);
    let s = b.poller.poll_slices().await.unwrap();
    assert_eq!((s.files, s.failed, s.records), (1, 0, 0));
    assert_eq!(
        b.metrics.counter(
            "swap_parse_errors_total",
            &[("bucket", "SEC_EQ"), ("scope", "file")]
        ),
        1
    );
    assert!(
        env.raw_dir()
            .join(format!("dtcc-ddr/sec/slices/{d}/{f}"))
            .exists()
    );
    b.poller.poll_slices().await.unwrap();
    assert_eq!(env.requests_to(&format!("/sec/slices/{f}")).await, 1);
}

#[tokio::test]
async fn rejects_a_bad_bucket_name() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/api/general/bucketname"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not a bucket/../"))
        .mount(&env.server)
        .await;
    let metrics = Arc::new(Metrics::new());
    let cfg = DtccConfig {
        api_base: format!("{}/api", env.server.uri()),
        data_base: None,
        ..DtccConfig::default()
    };
    let seen = SeenStore::open(env.state_dir().join("seen"), 3, &today()).unwrap();
    let mut poller = DtccPoller::new(
        cfg,
        Fetcher::new(quick_http(), metrics.clone()).unwrap(),
        RawStore::new(env.raw_dir(), None, metrics.clone()),
        CheckpointStore::open(env.state_dir().join("checkpoint.json")).unwrap(),
        Emitter::new(seen, None, None, metrics.clone()),
        metrics,
        Arc::new(Health::new(Duration::from_secs(60))),
    );
    assert!(poller.poll_slices().await.is_err());
}

#[tokio::test]
async fn run_once_from_command_line_arguments() {
    let env = Env::new().await;
    let base = env.server.uri();
    let d = today();
    let f = format!("CFTC_SLICE_RATES_{}_1.zip", us(&d));
    Mock::given(method("GET"))
        .and(path("/dashboard/Slice.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "CFTC_IR": [entry("cftc", "slices", &base, &f, 1, &format!("{d}T10:00:06Z"))],
        })))
        .mount(&env.server)
        .await;
    mount_file(
        &env.server,
        &format!("/cftc/slices/{f}"),
        rates_report(&["1", "2"], &d).zip("f"),
    )
    .await;
    let p = |x: &Path| x.to_string_lossy().into_owned();
    let args = Args::parse_from([
        "swap-pollers".to_string(),
        "--once".into(),
        "--dtcc-data-base".into(),
        base.clone(),
        "--buckets".into(),
        "CFTC_IR".into(),
        "--cumulative-days".into(),
        "0".into(),
        "--min-request-interval-ms".into(),
        "0".into(),
        "--raw-dir".into(),
        p(&env.raw_dir()),
        "--state-dir".into(),
        p(&env.state_dir()),
        "--out-dir".into(),
        p(&env.out_dir()),
    ]);
    let (tx, mut rx) = mpsc::channel(100);
    let (_stop, stop_rx) = watch::channel(false);
    swap_pollers::config::run(&args, Some(tx), stop_rx)
        .await
        .unwrap();
    assert_eq!(drain(&mut rx).len(), 2);
    assert_eq!(env.jsonl(&d).len(), 2);
    assert!("CA_IR".parse::<DtccBucket>().is_err());
    assert!(Args::try_parse_from(["swap-pollers", "--buckets", "CA_IR"]).is_err());
}

#[tokio::test]
async fn metrics_and_health_endpoints() {
    let metrics = Arc::new(Metrics::new());
    metrics.inc(
        "swap_records_total",
        &[("bucket", "CFTC_IR"), ("action", "new")],
    );
    let healthy = Arc::new(Health::new(Duration::from_secs(60)));
    healthy.record_success();
    let stale = Arc::new(Health::new(Duration::ZERO));

    let client = reqwest::Client::new();
    for (health, want) in [(healthy, 200u16), (stale, 503u16)] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, mut stop_rx) = watch::channel(false);
        let server = tokio::spawn(swap_pollers::server::serve(
            listener,
            metrics.clone(),
            health,
            async move {
                let _ = stop_rx.changed().await;
            },
        ));
        tokio::time::sleep(Duration::from_millis(5)).await;
        let h = client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .unwrap();
        assert_eq!(h.status().as_u16(), want);
        let body: serde_json::Value = serde_json::from_str(&h.text().await.unwrap()).unwrap();
        assert!(body.get("status").is_some());
        let m = client
            .get(format!("http://{addr}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(m.contains("swap_records_total{action=\"new\",bucket=\"CFTC_IR\"} 1"));
        assert!(m.contains("# TYPE swap_lag_seconds gauge"));
        stop.send(true).unwrap();
        server.await.unwrap().unwrap();
    }
}
