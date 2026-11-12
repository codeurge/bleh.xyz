//! JSON API over a synthetic archive.

#![allow(clippy::unwrap_used, missing_docs)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use history_api::config::Limits;
use mesh_types::{Record, RecordKind, SnapshotDepth};
use tower::ServiceExt;

#[tokio::test]
async fn health_metrics_limits_openapi() {
    let app = app();
    let (s, v) = get_json(&app, "/health").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "ok");
    assert!(v["duckdb"].as_str().unwrap().starts_with('v'));
    let (s, v) = get_json(&app, "/v0/limits").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["limits"]["max_rows"], 10_000);
    let (s, v) = get_json(&app, "/v0/openapi.json").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["paths"]["/v0/book"].is_object());
    let _ = get_json(&app, "/v0/instruments").await;
    let (s, h, body) = get_raw(&app, "/metrics").await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        h["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    let text = String::from_utf8(body).unwrap();
    assert!(text.contains("bleh_history_requests_total{route=\"/v0/instruments\",status=\"200\"}"));
    assert!(text.contains("bleh_history_query_seconds_count"));
}

#[tokio::test]
async fn instruments_from_registry_and_archive() {
    let app = app();
    let (s, v) = get_json(&app, "/v0/instruments").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["total"], 2);
    let syn = v["instruments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "hyperliquid:perp:SYN")
        .unwrap();
    assert_eq!(syn["price_scale"], 3);
    assert_eq!(syn["size_scale"], 3);
    assert_eq!(syn["admission"], "ADM-0001");
    assert_eq!(syn["sources"], serde_json::json!(["registry", "archive"]));
    assert_eq!(syn["archive"]["first_date"], "2026-01-01");
    assert_eq!(syn["archive"]["last_date"], "2026-01-02");
    assert!(
        syn["archive"]["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k == "bar")
    );
    let (_, v) = get_json(&app, "/v0/instruments?q=idx").await;
    assert_eq!(v["total"], 1);
    assert_eq!(
        v["instruments"][0]["flags"],
        serde_json::json!(["oracle_anchored_venue"])
    );
    // Pagination.
    let (_, p1) = get_json(&app, "/v0/instruments?limit=1").await;
    let c = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get_json(&app, &format!("/v0/instruments?limit=1&cursor={c}")).await;
    assert_ne!(p1["instruments"][0]["id"], p2["instruments"][0]["id"]);
    assert!(p2["next_cursor"].is_null());
}

#[tokio::test]
async fn instruments_without_registry_are_discovered() {
    let f = fixture();
    let app = app_with(config(&f.root, None));
    let (s, v) = get_json(&app, "/v0/instruments").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["total"], 2);
    let first = &v["instruments"][0];
    assert_eq!(first["sources"], serde_json::json!(["archive"]));
    assert!(
        first["price_scale"].is_u64(),
        "scales read from partitions: {first}"
    );
}

#[tokio::test]
async fn trades_page_through_exactly() {
    let app = app();
    let (s0, e0) = (start(), start() + 3 * HOUR);
    let base = format!("/v0/trades?instrument=hyperliquid:perp:SYN&start={s0}&end={e0}&limit=97");
    let got = collect_pages(&app, &base).await;
    let want = expected(RecordKind::Trade, &syn(), s0, e0);
    assert!(want.len() > 200, "fixture too small: {}", want.len());
    assert_eq!(got, want);
    // RFC 3339 and day forms work too, and the range spans two UTC days.
    let (s, v) = get_json(
        &app,
        "/v0/trades?instrument=hyperliquid:perp:SYN&start=2026-01-01T23:59:00Z&end=2026-01-02T00:01:00Z",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let recs = v["records"].as_array().unwrap();
    assert!(!recs.is_empty());
    assert!(
        recs.iter()
            .any(|r| r["prov"]["ts_source_ms"].as_i64().unwrap() >= 1_767_312_000_000)
    );
    assert!(
        recs.iter()
            .any(|r| r["prov"]["ts_source_ms"].as_i64().unwrap() < 1_767_312_000_000)
    );
    // Decimal strings with exact scale.
    let p = recs[0]["price"].as_str().unwrap();
    assert_eq!(p.split('.').nth(1).unwrap().len(), 3);
}

#[tokio::test]
async fn cursors_are_bound_to_their_query() {
    let app = app();
    let s0 = start();
    let (_, v) = get_json(
        &app,
        &format!("/v0/trades?instrument=hyperliquid:perp:SYN&start={s0}&limit=5"),
    )
    .await;
    let c = v["next_cursor"].as_str().unwrap();
    let (s, e) = get_json(
        &app,
        &format!("/v0/trades?instrument=hyperliquid:perp:syn:IDX&start={s0}&limit=5&cursor={c}"),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"]["code"], "invalid_cursor");
    let (s, e) = get_json(
        &app,
        &format!("/v0/trades?instrument=hyperliquid:perp:SYN&start={s0}&limit=5&cursor=garbage!"),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"]["code"], "invalid_cursor");
}

#[tokio::test]
async fn book_events_page_through_exactly() {
    let app = app();
    let (s0, e0) = (start() + HOUR / 2, start() + HOUR);
    let base =
        format!("/v0/book-events?instrument=hyperliquid:perp:syn:IDX&start={s0}&end={e0}&limit=50");
    let got = collect_pages(&app, &base).await;
    assert_eq!(got, expected(RecordKind::BookEvent, &idx(), s0, e0));
}

#[tokio::test]
async fn bars_are_read_as_stored() {
    let app = app();
    for interval in ["1s", "1m", "5m", "15m", "1h", "1d"] {
        let (s0, e0) = (start() - 86_400_000, start() + 5 * HOUR);
        let got = collect_pages(
            &app,
            &format!(
                "/v0/bars?instrument=hyperliquid:perp:SYN&interval={interval}&start={s0}&end={e0}&limit=400"
            ),
        )
        .await;
        let want: Vec<serde_json::Value> = all(RecordKind::Bar, &syn())
            .into_iter()
            .filter_map(|r| match r {
                Record::Bar(b) if b.interval.as_str() == interval => {
                    Some(serde_json::to_value(Record::Bar(b)).unwrap())
                }
                _ => None,
            })
            .collect();
        assert!(!want.is_empty(), "{interval}");
        assert_eq!(got, want, "{interval}");
    }
    let (s, e) = get_json(
        &app,
        "/v0/bars?instrument=hyperliquid:perp:SYN&interval=500t&start=2026-01-01",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"]["code"], "unsupported_interval");
    assert!(e["error"]["message"].as_str().unwrap().contains("tick"));
    // Daily bars allow a wide range (one page of bars), minute bars do not.
    let (s, _) = get_json(
        &app,
        "/v0/bars?instrument=hyperliquid:perp:SYN&interval=1d&start=2025-01-01&end=2026-02-01",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, e) = get_json(
        &app,
        "/v0/bars?instrument=hyperliquid:perp:SYN&interval=1s&start=2025-01-01&end=2026-02-01",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"]["code"], "range_too_large");
}

#[tokio::test]
async fn footprints_and_volume_profiles() {
    let app = app();
    let (s0, e0) = (start(), start() + 4 * HOUR);
    let got = collect_pages(
        &app,
        &format!(
            "/v0/footprint?instrument=hyperliquid:perp:SYN&interval=1m&start={s0}&end={e0}&limit=60"
        ),
    )
    .await;
    let want: Vec<_> = all(RecordKind::Footprint, &syn())
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    assert_eq!(got, want);
    let got = collect_pages(
        &app,
        &format!("/v0/volume-profile?instrument=hyperliquid:perp:SYN&start={s0}&end={e0}&limit=2"),
    )
    .await;
    let want: Vec<_> = all(RecordKind::VolumeProfile, &syn())
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    assert_eq!(want.len(), 4);
    assert_eq!(got, want);
    // Overlap: a range inside one profile hour returns that profile.
    let (_, v) = get_json(
        &app,
        &format!(
            "/v0/volume-profile?instrument=hyperliquid:perp:SYN&start={}&end={}",
            s0 + HOUR + 60_000,
            s0 + HOUR + 120_000
        ),
    )
    .await;
    assert_eq!(v["count"], 1);
}

#[tokio::test]
async fn book_at_a_checkpoint_matches_it() {
    let app = app();
    let snaps = all(RecordKind::BookSnapshot, &syn());
    let Record::BookSnapshot(s) = &snaps[3] else {
        panic!()
    };
    let t = s.prov.ts_source_ms.0;
    let (st, v) = get_json(
        &app,
        &format!("/v0/book?instrument=hyperliquid:perp:SYN&at={t}"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["events_applied"], 0);
    assert_eq!(
        v["checkpoint"]["seq"]["block_number"],
        s.prov.seq.block_number
    );
    let got: Record = serde_json::from_value(v["book"].clone()).unwrap();
    let Record::BookSnapshot(g) = got else {
        panic!()
    };
    assert_eq!(g.depth, s.depth);
    g.validate().unwrap();
}

#[tokio::test]
async fn book_reconstruction_matches_the_generator() {
    // The sparse archive only has the initial snapshot; every later book
    // must be rebuilt from events and equal the generator's snapshots.
    let f = fixture();
    let mut cfg = config(&f.sparse_root, Some(f.registry.clone()));
    cfg.limits = Limits {
        max_checkpoint_lookback: std::time::Duration::from_secs(86_400),
        rate_per_sec: 0.0,
        ..Limits::default()
    };
    let app = app_with(cfg);
    for id in [syn(), idx()] {
        let snaps = all(RecordKind::BookSnapshot, &id);
        assert!(snaps.len() > 5);
        for snap in snaps.iter().skip(1).step_by(4) {
            let Record::BookSnapshot(s) = snap else {
                panic!()
            };
            let t = s.prov.ts_source_ms.0;
            let (st, v) = get_json(&app, &format!("/v0/book?instrument={id}&at={t}")).await;
            assert_eq!(st, StatusCode::OK, "{v}");
            assert!(v["events_applied"].as_u64().unwrap() > 0);
            assert_eq!(v["inconsistencies"]["unknown_order"], 0);
            let Record::BookSnapshot(g) = serde_json::from_value(v["book"].clone()).unwrap() else {
                panic!()
            };
            assert_eq!(g.depth, s.depth, "{id} at {t}");
            assert_eq!(g.prov.seq, s.prov.seq);
            // L2 with a level cap.
            let (_, v) = get_json(
                &app,
                &format!("/v0/book?instrument={id}&at={t}&depth=l2&levels=3"),
            )
            .await;
            let Record::BookSnapshot(l2) = serde_json::from_value(v["book"].clone()).unwrap()
            else {
                panic!()
            };
            let SnapshotDepth::L2 { bids, asks } = &l2.depth else {
                panic!()
            };
            assert!(bids.len() <= 3 && asks.len() <= 3);
            let SnapshotDepth::L2 { bids: wb, .. } = s.depth.to_l2().unwrap() else {
                panic!()
            };
            assert_eq!(bids[..], wb[..bids.len()]);
            assert_eq!(v["best_bid"], serde_json::json!(wb[0].price.to_string()));
        }
    }
}

#[tokio::test]
async fn book_without_checkpoint_is_not_found() {
    let app = app();
    let (s, v) = get_json(
        &app,
        "/v0/book?instrument=hyperliquid:perp:SYN&at=2025-06-01T00:00:00Z",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
    let (s, v) = get_json(&app, "/v0/book?instrument=hyperliquid:perp:SYN&depth=l3").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
}

#[tokio::test]
async fn book_defaults_to_latest() {
    let app = app();
    let (s, v) = get_json(
        &app,
        "/v0/book?instrument=hyperliquid:perp:SYN&depth=l2&levels=2",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let last_event = all(RecordKind::BookEvent, &syn())
        .last()
        .unwrap()
        .prov()
        .ts_source_ms
        .0;
    assert_eq!(v["at_ms"], last_event);
}

#[tokio::test]
async fn coverage_shows_the_gap() {
    let app = app();
    let s0 = start();
    let (s, v) = get_json(
        &app,
        &format!("/v0/coverage?start={s0}&end={}", s0 + 4 * HOUR),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let by: std::collections::HashMap<String, serde_json::Value> = v["instruments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["instrument"].as_str().unwrap().to_owned(), i.clone()))
        .collect();
    let syn = &by["hyperliquid:perp:SYN"];
    assert_eq!(syn["recorded_hours"], 4);
    assert_eq!(syn["missing_hours"], 0);
    let idx = &by["hyperliquid:perp:syn:IDX"];
    assert_eq!(idx["recorded_hours"], 3);
    assert_eq!(idx["missing_hours"], 1);
    assert_eq!(idx["hours"][1]["status"], "missing");
    assert_eq!(idx["hours"][2]["gap_before"], true);
    assert_eq!(idx["gap_hours"], 1);
    // Range hours outside the archive are missing.
    let (_, v) = get_json(
        &app,
        &format!(
            "/v0/coverage?instrument=hyperliquid:perp:SYN&start={}&end={}",
            s0 - 2 * HOUR,
            s0
        ),
    )
    .await;
    assert_eq!(v["instruments"][0]["missing_hours"], 2);
}

#[tokio::test]
async fn manifests_link_torrents_and_cids() {
    let app = app();
    let (s, v) = get_json(
        &app,
        "/v0/manifests?start=2026-01-01&end=2026-01-03&files=true",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let list = v["manifests"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["date"], "2026-01-01");
    assert_eq!(list[1]["sequence"], 1);
    assert_eq!(
        list[1]["previous_manifest_sha256"],
        list[0]["manifest_sha256"]
    );
    assert!(
        list[0]["magnet"]
            .as_str()
            .unwrap()
            .starts_with("magnet:?xt=urn:btmh:1220")
    );
    let files = list[0]["files"].as_array().unwrap();
    assert!(
        files
            .iter()
            .all(|f| f["cid"].as_str().unwrap().starts_with("bafk"))
    );
    assert!(
        files[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://demo.invalid/hyperliquid-normalized/kind=")
    );
    let (_, v) = get_json(&app, "/v0/manifests?start=2026-01-01&end=2026-01-03").await;
    assert!(v["manifests"][0].get("files").is_none());
}

#[tokio::test]
async fn errors_are_consistent() {
    let app = app();
    let cases = [
        (
            "/v0/trades?instrument=hyperliquid:perp:NOPE&start=2026-01-01",
            StatusCode::NOT_FOUND,
            "unknown_instrument",
        ),
        (
            "/v0/trades?instrument=bad id&start=2026-01-01",
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            "/v0/trades?instrument=hyperliquid:perp:SYN&start=yesterday",
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            "/v0/trades?instrument=hyperliquid:perp:SYN&start=2026-01-01&end=2026-03-01",
            StatusCode::BAD_REQUEST,
            "range_too_large",
        ),
        (
            "/v0/trades?instrument=hyperliquid:perp:SYN&start=2026-01-02&end=2026-01-01",
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            "/v0/trades?instrument=hyperliquid:perp:SYN&limit=abc",
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        ("/v0/trades", StatusCode::BAD_REQUEST, "bad_request"),
        ("/v0/nope", StatusCode::NOT_FOUND, "not_found"),
    ];
    for (uri, status, code) in cases {
        let (s, v) = get_json(&app, &uri.replace(' ', "%20")).await;
        assert_eq!(s, status, "{uri}: {v}");
        assert_eq!(v["error"]["code"], code, "{uri}");
        assert!(v["error"]["message"].is_string());
    }
}

#[tokio::test]
async fn empty_ranges_return_empty_pages() {
    let app = app();
    let (s, v) = get_json(
        &app,
        "/v0/trades?instrument=hyperliquid:perp:SYN&start=2025-01-01&end=2025-01-02",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["count"], 0);
    assert!(v["next_cursor"].is_null());
}

#[tokio::test]
async fn rate_limits_per_ip() {
    let f = fixture();
    let mut cfg = config(&f.root, Some(f.registry.clone()));
    cfg.limits.rate_per_sec = 0.5;
    cfg.limits.rate_burst = 2.0;
    let app = app_with(cfg);
    assert_eq!(get_raw(&app, "/v0/limits").await.0, StatusCode::OK);
    assert_eq!(get_raw(&app, "/v0/limits").await.0, StatusCode::OK);
    let (s, h, b) = get_raw(&app, "/v0/limits").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key("retry-after"));
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["error"]["code"], "rate_limited");
    // Health stays reachable.
    assert_eq!(get_raw(&app, "/health").await.0, StatusCode::OK);
}

#[tokio::test]
async fn cors_preflight() {
    let app = app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/v0/trades")
                .header("origin", "https://example.org")
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success());
    assert_eq!(resp.headers()["access-control-allow-origin"], "*");
}

#[tokio::test]
async fn query_timeouts_are_reported() {
    let f = fixture();
    let mut cfg = config(&f.root, Some(f.registry.clone()));
    cfg.limits.query_timeout = std::time::Duration::from_nanos(1);
    let app = app_with(cfg);
    let (s, v) = get_json(
        &app,
        &format!(
            "/v0/trades?instrument=hyperliquid:perp:SYN&start={}&end={}",
            start(),
            start() + 4 * HOUR
        ),
    )
    .await;
    assert_eq!(s, StatusCode::GATEWAY_TIMEOUT, "{v}");
    assert_eq!(v["error"]["code"], "query_timeout");
}

#[tokio::test]
async fn explain_provenance_links_the_manifest() {
    let svc = service();
    let t = all(RecordKind::Trade, &syn())[10].clone();
    let p = t.prov();
    let v = svc
        .explain_provenance(
            "hyperliquid:perp:SYN",
            "trade",
            p.seq.block_number,
            Some(p.seq.event_index),
            &p.ts_source_ms.0.to_string(),
        )
        .await
        .unwrap();
    assert_eq!(v["record"], serde_json::to_value(&t).unwrap());
    assert_eq!(v["provenance"]["admission"]["id"], "ADM-0001");
    assert!(v["file"].as_str().unwrap().ends_with("part-00000.parquet"));
    assert!(
        v["manifest"]["file"]["cid"]
            .as_str()
            .unwrap()
            .starts_with("bafk")
    );
    assert_eq!(
        v["canonical_encoding"],
        String::from_utf8(mesh_types::hash::canonical_record(&t).unwrap()).unwrap()
    );
    let bar = all(RecordKind::Bar, &idx())[0].clone();
    let Record::Bar(b) = &bar else { panic!() };
    let v = svc
        .explain_provenance(
            "hyperliquid:perp:syn:IDX",
            "bar",
            b.prov.seq.block_number,
            Some(b.prov.seq.event_index),
            &b.start_ms.0.to_string(),
        )
        .await
        .unwrap();
    assert_eq!(v["derived"]["method"], "bars@1");
    assert_eq!(v["provenance"]["flags"][0]["name"], "oracle_anchored_venue");
    let e = svc
        .explain_provenance("hyperliquid:perp:SYN", "trade", 1, Some(0), "2026-01-01")
        .await
        .unwrap_err();
    assert_eq!(e.code, history_api::error::ErrorCode::NotFound);
}
