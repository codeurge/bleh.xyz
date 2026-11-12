//! Shared fixtures: a synthetic archive written once per test binary in the
//! archive-layout-v0 layout, and helpers to call the router.
//!
//! Everything here is synthetic (see `history_api::synth`).

#![allow(clippy::unwrap_used, dead_code, unreachable_pub, missing_docs)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::OnceLock;

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode};
use history_api::config::{Config, DEFAULT_ARCHIVE, Limits};
use history_api::http::{AppState, router_with_mcp};
use history_api::service::Service;
use history_api::synth::{self, SynthConfig, SynthData};
use mesh_types::{InstrumentId, Record, RecordKind};
use serde_json::Value;
use tower::ServiceExt;

pub struct Fixture {
    /// Full archive.
    pub root: PathBuf,
    /// The same records with only the initial book snapshot, to check book
    /// reconstruction against the generator's own book.
    pub sparse_root: PathBuf,
    pub data: SynthData,
    pub cfg: SynthConfig,
    pub registry: PathBuf,
}

pub fn cfg() -> SynthConfig {
    SynthConfig {
        hours: 4,
        block_ms: 20_000,
        snapshot_every: 30,
        ..SynthConfig::default()
    }
}

pub fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("bleh-history-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cfg = cfg();
        let data = synth::generate(&cfg).unwrap();
        let root = base.join("full");
        let summary = synth::write_archive(&root, DEFAULT_ARCHIVE, &data).unwrap();
        // Sparse copy: drop every snapshot but the first per instrument.
        let mut sparse = SynthData {
            registry: data.registry.clone(),
            partitions: data.partitions.clone(),
        };
        let firsts: std::collections::BTreeMap<InstrumentId, mesh_types::Seq> = [syn(), idx()]
            .into_iter()
            .filter_map(|id| {
                let first = data
                    .records(RecordKind::BookSnapshot, &id)
                    .first()?
                    .prov()
                    .seq;
                Some((id, first))
            })
            .collect();
        for ((kind, id, _), records) in &mut sparse.partitions {
            if *kind == RecordKind::BookSnapshot {
                records.retain(|r| firsts.get(id) == Some(&r.prov().seq));
            }
        }
        sparse.partitions.retain(|_, v| !v.is_empty());
        let sparse_root = base.join("sparse");
        synth::write_archive(&sparse_root, DEFAULT_ARCHIVE, &sparse).unwrap();
        Fixture {
            root,
            sparse_root,
            data,
            cfg,
            registry: summary.registry_file,
        }
    })
}

pub fn config(root: &std::path::Path, registry: Option<PathBuf>) -> Config {
    let mut c = Config::local(root);
    c.registry_file = registry;
    c.limits = Limits {
        rate_per_sec: 0.0,
        ..Limits::default()
    };
    c
}

pub fn service() -> Service {
    let f = fixture();
    Service::open(config(&f.root, Some(f.registry.clone()))).unwrap()
}

pub fn app_with(config: Config) -> Router {
    let svc = Service::open(config).unwrap();
    router_with_mcp(AppState::new(svc))
        .layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 10], 4000))))
}

pub fn app() -> Router {
    let f = fixture();
    app_with(config(&f.root, Some(f.registry.clone())))
}

pub async fn get_raw(app: &Router, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), 256 << 20)
        .await
        .unwrap();
    (status, headers, body.to_vec())
}

pub async fn get_json(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, _, b) = get_raw(app, uri).await;
    let v: Value = serde_json::from_slice(&b)
        .unwrap_or_else(|e| panic!("{uri}: {e}: {}", String::from_utf8_lossy(&b)));
    (s, v)
}

pub fn syn() -> InstrumentId {
    InstrumentId::new("hyperliquid:perp:SYN").unwrap()
}

pub fn idx() -> InstrumentId {
    InstrumentId::new("hyperliquid:perp:syn:IDX").unwrap()
}

/// Records of a kind for an instrument with `start <= ts < end`.
pub fn expected(kind: RecordKind, id: &InstrumentId, start: i64, end: i64) -> Vec<Value> {
    fixture()
        .data
        .records(kind, id)
        .into_iter()
        .filter(|r| {
            let t = r.prov().ts_source_ms.0;
            t >= start && t < end
        })
        .map(|r| serde_json::to_value(r).unwrap())
        .collect()
}

pub fn all(kind: RecordKind, id: &InstrumentId) -> Vec<Record> {
    fixture()
        .data
        .records(kind, id)
        .into_iter()
        .cloned()
        .collect()
}

pub fn start() -> i64 {
    fixture().cfg.start_ms
}

pub const HOUR: i64 = 3_600_000;

/// Follows `next_cursor` until the end and returns every record.
pub async fn collect_pages(app: &Router, base_uri: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..1_000 {
        let uri = match &cursor {
            Some(c) => format!("{base_uri}&cursor={c}"),
            None => base_uri.to_owned(),
        };
        let (s, v) = get_json(app, &uri).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        out.extend(v["records"].as_array().unwrap().iter().cloned());
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_owned()),
            None => return out,
        }
    }
    panic!("too many pages");
}
