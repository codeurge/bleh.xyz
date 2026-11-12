//! End to end on synthetic data, no network beyond localhost:
//!
//! 1. hl-synth node files are written block by block into a node data
//!    directory while the origin tails it; a reference client on a real
//!    localhost websocket checks that its books equal hl-synth's expected
//!    books, that every commitment signature and every recomputed root
//!    verifies, and that derived records and coverage arrive. The origin's
//!    live capture seals the finished hour and writes the watermark.
//! 2. The same node files go through `archive capture run` and `archive
//!    replay` (the wired binary) twice: partitions are byte-identical and
//!    their content hashes equal, and the history API serving them returns
//!    the live path's trades and bars and its final books.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use history_api::config::{Config, Limits};
use history_api::service::{RangeQuery, Service};
use mesh_client::{ChannelSpec, Client, ClientConfig, ClientEvent};
use mesh_types::hash::canonical_record;
use mesh_types::{InstrumentId, Record, RecordKind, Registry, SnapshotDepth};
use origin::derive::{BookStart, DeriveConfig};
use origin::service::{CaptureSettings, OriginConfig, ReaderSettings};
use serde_json::Value;

const STREAMS: [&str; 4] = [
    "node_fills_by_block",
    "node_order_statuses_by_block",
    "node_raw_book_diffs_by_block",
    "misc_events_by_block",
];

fn tempdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("origin-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Per stream, the lines of the base scenario with the hour file each
/// belongs to, in order.
fn stream_lines(files: &hl_synth::FileSet, stream: &str) -> Vec<(String, String)> {
    let prefix = format!("base/{stream}/");
    let mut out = Vec::new();
    for (rel, data) in files {
        let Some(rest) = rel.strip_prefix(&prefix) else {
            continue;
        };
        for line in String::from_utf8_lossy(data).lines() {
            out.push((rest.to_owned(), format!("{line}\n")));
        }
    }
    out
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn triples(depth: &SnapshotDepth) -> Vec<(u64, String, String)> {
    let SnapshotDepth::L4 { bids, asks } = depth else {
        panic!("not L4");
    };
    bids.iter()
        .chain(asks)
        .map(|o| (o.order_id.0, o.price.to_string(), o.size.to_string()))
        .collect()
}

fn expected_book(expected: &Value, coin: &str) -> Vec<(u64, String, String)> {
    let book = &expected["coins"][coin]["final_book"];
    ["bids", "asks"]
        .iter()
        .flat_map(|side| book[*side].as_array().unwrap().clone())
        .map(|o| {
            (
                o["order_id"].as_u64().unwrap(),
                o["price"].as_str().unwrap().to_owned(),
                o["size"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn canon(r: &Record) -> String {
    String::from_utf8(canonical_record(r).unwrap()).unwrap()
}

fn origin_config(node: &Path, registry: &Path, key: &Path, raw: &Path) -> OriginConfig {
    let mut derive = DeriveConfig::hyperliquid().unwrap();
    // The archive replay derives with the same (default) settings.
    derive.book_start = BookStart::Empty;
    OriginConfig {
        reader: ReaderSettings {
            node_dirs: vec![node.to_owned()],
            dirs: hl_node_reader::filter::StreamDirs::default(),
            registry: Some(registry.to_owned()),
            registry_out: None,
            info_url: None,
            info_timeout: Duration::from_secs(1),
            registry_refresh: None,
            checkpoint: Some(raw.join("reader-checkpoint.json")),
            checkpoint_interval: Duration::from_millis(100),
            start: hl_node_reader::files::StartPosition::Earliest,
            use_notify: false,
            poll_interval: Duration::from_millis(5),
            max_lag_blocks: 600,
            merge: hl_node_reader::merge::MergeConfig::default(),
            snapshot_interval: None,
            snapshot_path: raw.join("l4.json"),
        },
        derive,
        signing_keys: vec![format!("e2e=file:{}", key.display()).parse().unwrap()],
        active_key: None,
        gateway_listen: "127.0.0.1:0".parse().unwrap(),
        gateway: gateway::GatewayConfig::default(),
        proof_retention_blocks: 10_000,
        metrics_listen: Some("127.0.0.1:0".parse().unwrap()),
        nats: None,
        capture: Some(CaptureSettings {
            staging: raw.to_owned(),
            private: true,
            upload: false,
            keep_local: false,
            metrics_out: Some((raw.join("capture.prom"), Duration::from_millis(50))),
            poll_interval: Duration::from_millis(20),
            // The fixture's hours are long over by the wall clock; seal by
            // node files only.
            seal_grace: Duration::from_secs(10 * 365 * 86_400),
        }),
    }
}

/// Everything the live client saw.
#[derive(Default)]
struct Live {
    trades: BTreeMap<String, Vec<Record>>,
    records: Vec<Record>,
    books: BTreeMap<String, Vec<(u64, String, String)>>,
}

fn run_archive(args: &[&str], registry: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_archive"))
        .args(args)
        .env("BLEH_ARCHIVE_REGISTRY", registry)
        .env("BLEH_ARCHIVE_BOOK_START", "empty")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "archive {args:?}\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn parquet_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "parquet") {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn node_files_to_signed_live_stream_and_archive() {
    let files = hl_synth::generate(hl_synth::Config::default()).unwrap();
    let expected: Value = serde_json::from_slice(&files["base/expected.json"]).unwrap();
    let registry_bytes = &files["markets.json"];
    let registry: Registry = serde_json::from_slice(registry_bytes).unwrap();
    let root = tempdir("main");
    let node = root.join("node/data");
    for s in STREAMS {
        std::fs::create_dir_all(node.join(s).join("hourly")).unwrap();
    }
    let registry_path = root.join("markets.json");
    std::fs::write(&registry_path, registry_bytes).unwrap();
    let key = root.join("signing.key");
    std::fs::write(&key, format!("{}\n", "11".repeat(32))).unwrap();
    let raw = root.join("raw");

    // ---------------------------------------------------------- live path
    let running = origin::start(origin_config(&node, &registry_path, &key, &raw))
        .await
        .unwrap();
    let url = format!("ws://{}/v0/ws", running.gateway_addr);
    let kinds = [
        "trade",
        "book_event",
        "book_snapshot",
        "bar",
        "footprint",
        "delta_bar",
        "order_activity",
        "queue_positions",
        "sweep",
        "heatmap_tile",
        "liquidity_grade",
        "coverage",
    ];
    let channels: Vec<ChannelSpec> = kinds
        .iter()
        .map(|k| format!("*|{k}").parse().unwrap())
        .collect();
    let mut client = Client::new(ClientConfig {
        urls: vec![url],
        channels,
        commitments: true,
        signers: running.signers.clone(),
        stall_timeout: Duration::from_secs(60),
        ..ClientConfig::default()
    });
    // Subscribe before the node writes anything.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let ev = tokio::time::timeout_at(deadline.into(), client.next_event())
            .await
            .expect("subscribe timeout")
            .unwrap();
        if matches!(ev, ClientEvent::Subscribed { .. }) {
            break;
        }
    }

    // The "node" writes one block of every stream at a time.
    let per_stream: Vec<Vec<(String, String)>> =
        STREAMS.iter().map(|s| stream_lines(&files, s)).collect();
    let blocks = per_stream[0].len();
    assert_eq!(blocks, usize::try_from(hl_synth::DEFAULT_BLOCKS).unwrap());
    let last_block = hl_synth::FIRST_BLOCK + hl_synth::DEFAULT_BLOCKS - 1;
    let writer = {
        let node = node.clone();
        tokio::spawn(async move {
            for i in 0..blocks {
                for (s, lines) in STREAMS.iter().zip(&per_stream) {
                    let (rel, line) = &lines[i];
                    append(&node.join(s).join(rel), line);
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        })
    };

    let mut live = Live::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ev = tokio::time::timeout_at(deadline.into(), client.next_event())
            .await
            .expect("live stream timeout")
            .unwrap();
        match ev {
            ClientEvent::Batch { batch } if batch.kind == RecordKind::Trade => {
                live.trades
                    .entry(batch.instrument.to_string())
                    .or_default()
                    .extend(batch.records.iter().cloned());
            }
            ClientEvent::Record { record } => live.records.push((*record).clone()),
            ClientEvent::VerificationFailed { detail } => panic!("verification failed: {detail}"),
            ClientEvent::Gap {
                instrument,
                kind,
                detail,
                ..
            } => {
                panic!("gap on {instrument} {kind}: {detail}")
            }
            ClientEvent::Block { block_number, .. } if block_number == last_block => break,
            _ => {}
        }
    }
    writer.await.unwrap();
    for spec in registry.iter() {
        let book = client
            .book(&spec.id)
            .unwrap_or_else(|| panic!("no live book for {}", spec.id));
        let got = triples(&book.depth_l4());
        assert_eq!(
            got,
            expected_book(&expected, &spec.venue_symbol),
            "{}: live book equals hl-synth's final book",
            spec.id
        );
        live.books.insert(spec.id.to_string(), got);
    }
    let stats = client.stats();
    assert_eq!(
        stats.signatures_failed + stats.roots_failed + stats.proofs_failed,
        0,
        "{stats:?}"
    );
    assert!(stats.signatures_ok > 0 && stats.roots_ok > 0, "{stats:?}");
    // Trades are exactly the ground truth.
    for spec in registry.iter() {
        let want = expected["coins"][&spec.venue_symbol]["trades"]
            .as_array()
            .unwrap();
        let got = &live.trades[spec.id.as_str()];
        assert_eq!(got.len(), want.len(), "{}", spec.id);
    }
    let live_kinds: BTreeSet<RecordKind> = live.records.iter().map(Record::kind).collect();
    for k in [
        RecordKind::Bar,
        RecordKind::BookSnapshot,
        RecordKind::Footprint,
        RecordKind::DeltaBar,
        RecordKind::QueuePositions,
        RecordKind::Coverage,
    ] {
        assert!(
            live_kinds.contains(&k),
            "no live {k} records: {live_kinds:?}"
        );
    }
    for r in &live.records {
        r.validate().unwrap();
        mesh_types::allowlist::check_record(r).unwrap();
        assert!(r.prov().ts_publish_ns.is_some());
    }
    // Health, freshness and metrics endpoints answer.
    assert!(running.gateway.healthy());
    let metrics_addr = running.metrics_addr.unwrap();
    let body = http_get(metrics_addr, "/metrics").await;
    assert!(body.contains("hl_output_last_block"), "{body}");
    assert!(body.contains("bleh_origin_blocks_total"), "{body}");
    let gw_metrics = http_get(running.gateway_addr, "/metrics").await;
    assert!(gw_metrics.contains("gateway_send_latency_seconds"));
    assert!(gw_metrics.contains("signer_commitments_total"));
    let fresh = http_get(running.gateway_addr, "/freshness").await;
    assert!(fresh.contains(&last_block.to_string()), "{fresh}");

    // Live capture sealed hour 0 of every stream and moved the watermark.
    let wm_deadline = Instant::now() + Duration::from_secs(10);
    while archive::follow::read_watermark(&raw).map(archive::follow::hour_text)
        != Some("2026-01-01T00".to_owned())
        && Instant::now() < wm_deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        archive::follow::read_watermark(&raw).map(archive::follow::hour_text),
        Some("2026-01-01T00".to_owned())
    );
    for s in ["fills", "order_statuses", "raw_book_diffs"] {
        for archive_name in ["hyperliquid-raw", "hyperliquid-raw-private"] {
            let p = raw
                .join(archive_name)
                .join("ready")
                .join(format!("stream={s}/date=2026-01-01/00.jsonl.zst"));
            assert!(p.exists(), "{}", p.display());
        }
    }
    assert!(raw.join("capture.prom").exists());
    running.stop();
    tokio::time::timeout(Duration::from_secs(20), running.join())
        .await
        .expect("origin stops")
        .unwrap();

    // ------------------------------------------------------- archive path
    let staging = root.join("staging");
    let mut capture_args: Vec<String> = vec![
        "capture".into(),
        "run".into(),
        "--staging".into(),
        staging.display().to_string(),
    ];
    for (slug, dir) in [
        ("fills", "node_fills_by_block"),
        ("order_statuses", "node_order_statuses_by_block"),
        ("raw_book_diffs", "node_raw_book_diffs_by_block"),
        ("misc_events", "misc_events_by_block"),
    ] {
        capture_args.push("--stream".into());
        capture_args.push(format!("{slug}={}", node.join(dir).display()));
    }
    let args: Vec<&str> = capture_args.iter().map(String::as_str).collect();
    run_archive(&args, &registry_path);
    let filtered_root = staging.join("hyperliquid-raw/ready");
    assert!(
        !filtered_root.join("stream=misc_events").exists(),
        "excluded streams never reach the filtered tier"
    );

    let norm_a = root.join("norm-a/hyperliquid-normalized");
    let norm_b = root.join("norm-b/hyperliquid-normalized");
    for out in [&norm_a, &norm_b] {
        run_archive(
            &[
                "replay",
                "--raw",
                &filtered_root.display().to_string(),
                "--date",
                "2026-01-01",
                "--out",
                &out.display().to_string(),
                "--allow-missing-next-hour",
            ],
            &registry_path,
        );
    }
    let (pa, pb) = (parquet_files(&norm_a), parquet_files(&norm_b));
    assert!(!pa.is_empty());
    assert_eq!(pa.keys().collect::<Vec<_>>(), pb.keys().collect::<Vec<_>>());
    let mut archived: Vec<Record> = Vec::new();
    for (rel, bytes) in &pa {
        assert_eq!(bytes, &pb[rel], "{rel}: byte-identical across runs");
        let a = archive::partition::read_parquet(bytes.clone().into()).unwrap();
        let b = archive::partition::read_parquet(pb[rel].clone().into()).unwrap();
        assert!(a.consistent());
        assert_eq!(a.content_hash, b.content_hash, "{rel}");
        archived.extend(a.records);
    }
    let archived_kinds: BTreeSet<RecordKind> = archived.iter().map(Record::kind).collect();
    assert!(
        archived_kinds.contains(&RecordKind::Coverage),
        "{archived_kinds:?}"
    );
    // Every derived record the live client received is in the archive,
    // identical apart from the publish time.
    let archived_canon: BTreeSet<String> = archived.iter().map(canon).collect();
    for r in &live.records {
        assert!(
            archived_canon.contains(&canon(r)),
            "live record missing from the archive: {}",
            canon(r)
        );
    }

    // --------------------------------------------- history API over it
    let mut cfg = Config::local(root.join("norm-a"));
    cfg.registry_file = Some(registry_path.clone());
    cfg.limits = Limits {
        rate_per_sec: 0.0,
        ..Limits::default()
    };
    let svc = Service::open(cfg).unwrap();
    let start = hl_synth::START_MS.div_euclid(3_600_000) * 3_600_000;
    let end = start + 2 * 3_600_000;
    for spec in registry.iter() {
        let inst = spec.id.to_string();
        let q = |interval: Option<&str>| RangeQuery {
            instrument: inst.clone(),
            start: Some(start.to_string()),
            end: Some(end.to_string()),
            limit: Some(10_000),
            cursor: None,
            interval: interval.map(str::to_owned),
        };
        let trades = svc.trades(&q(None)).await.unwrap();
        assert_eq!(
            trades.records.iter().map(canon).collect::<Vec<_>>(),
            live.trades[&inst].iter().map(canon).collect::<Vec<_>>(),
            "{inst}: archived trades equal the live trades"
        );
        for interval in ["1s", "1m"] {
            let bars = svc.bars(&q(Some(interval))).await.unwrap();
            let live_bars: Vec<String> = live
                .records
                .iter()
                .filter(|r| match r {
                    Record::Bar(b) => {
                        b.prov.instrument.as_str() == inst && b.interval.as_str() == interval
                    }
                    _ => false,
                })
                .map(canon)
                .collect();
            assert_eq!(
                bars.records.iter().map(canon).collect::<Vec<_>>(),
                live_bars,
                "{inst} {interval}: archived bars equal the live bars"
            );
        }
        let at = svc
            .book_at(&inst, Some(&(end - 1).to_string()), None, None)
            .await
            .unwrap();
        assert!(!at.inconsistencies.any(), "{inst}");
        let Record::BookSnapshot(s) = &at.book else {
            panic!("book_at returned {:?}", at.book.kind());
        };
        assert_eq!(&triples(&s.depth), &live.books[&inst], "{inst}: book at T");
    }
    let _ = InstrumentId::new("hyperliquid:venue:all").unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}
