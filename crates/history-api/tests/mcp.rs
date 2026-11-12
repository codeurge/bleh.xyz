//! MCP: tool behaviour, the stdio-style transport over an in-memory pipe,
//! and the streamable HTTP transport at `/mcp`.

#![allow(clippy::unwrap_used, missing_docs)]

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use futures::StreamExt;
use history_api::mcp::{HistoryMcp, call};
use mesh_types::{Record, RecordKind};
use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt as _;

fn args(v: Value) -> Option<serde_json::Map<String, Value>> {
    match v {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

#[tokio::test]
async fn tools_answer_like_the_api() {
    let svc = service();
    let v = call(&svc, "list_instruments", None).await.unwrap();
    assert_eq!(v["total"], 2);

    let s0 = start();
    let v = call(
        &svc,
        "get_trades",
        args(json!({"instrument": "hyperliquid:perp:SYN", "start": s0.to_string(), "limit": 3})),
    )
    .await
    .unwrap();
    assert_eq!(v["count"], 3);
    assert_eq!(
        v["records"][0],
        serde_json::to_value(&all(RecordKind::Trade, &syn())[0]).unwrap()
    );
    // MCP pages are capped.
    let v = call(
        &svc,
        "get_trades",
        args(json!({"instrument": "hyperliquid:perp:SYN", "start": s0.to_string(), "end": (s0 + 4 * HOUR).to_string(), "limit": 100_000})),
    )
    .await
    .unwrap();
    assert_eq!(v["count"], 500);

    let v = call(
        &svc,
        "get_bars",
        args(json!({"instrument": "hyperliquid:perp:SYN", "interval": "1h", "start": "2026-01-01", "end": "2026-01-03"})),
    )
    .await
    .unwrap();
    assert_eq!(v["count"], 4);

    let v = call(
        &svc,
        "get_book_at",
        args(json!({"instrument": "hyperliquid:perp:syn:IDX", "at": (s0 + 30 * 60_000).to_string(), "levels": 3})),
    )
    .await
    .unwrap();
    let Record::BookSnapshot(b) = serde_json::from_value(v["book"].clone()).unwrap() else {
        panic!()
    };
    match b.depth {
        mesh_types::SnapshotDepth::L2 { bids, .. } => assert_eq!(bids.len(), 3),
        mesh_types::SnapshotDepth::L4 { .. } => panic!("default depth is l2"),
    }

    let v = call(
        &svc,
        "get_volume_profile",
        args(json!({"instrument": "hyperliquid:perp:SYN", "start": s0.to_string(), "end": (s0 + HOUR).to_string()})),
    )
    .await
    .unwrap();
    assert_eq!(v["count"], 1);

    let v = call(
        &svc,
        "get_coverage",
        args(json!({"instrument": "hyperliquid:perp:syn:IDX", "start": s0.to_string(), "end": (s0 + 4 * HOUR).to_string()})),
    )
    .await
    .unwrap();
    let runs = v["instruments"][0]["runs"].as_array().unwrap();
    assert_eq!(runs[0]["status"], "recorded");
    assert_eq!(runs[1]["status"], "missing");
    assert_eq!(
        runs[1]["to_ms"].as_i64().unwrap() - runs[1]["from_ms"].as_i64().unwrap(),
        HOUR
    );

    let t = all(RecordKind::Trade, &idx())[3].clone();
    let p = t.prov();
    let v = call(
        &svc,
        "explain_provenance",
        args(json!({
            "instrument": "hyperliquid:perp:syn:IDX",
            "kind": "trade",
            "block_number": p.seq.block_number,
            "event_index": p.seq.event_index,
            "time": p.ts_source_ms.to_rfc3339(),
        })),
    )
    .await
    .unwrap();
    assert_eq!(v["record"], serde_json::to_value(&t).unwrap());

    let e = call(
        &svc,
        "get_trades",
        args(json!({"instrument": "hyperliquid:perp:SYN", "bogus": 1})),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code, history_api::error::ErrorCode::BadRequest);
    assert!(call(&svc, "nope", None).await.is_err());
}

async fn rpc_line<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut tokio::io::Lines<R>) -> Value {
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test]
async fn stdio_transport_speaks_json_rpc() {
    let (client, server) = tokio::io::duplex(1 << 20);
    let (sr, sw) = tokio::io::split(server);
    let svc = service();
    tokio::spawn(async move {
        if let Ok(running) = HistoryMcp::new(svc).serve((sr, sw)).await {
            let _ = running.waiting().await;
        }
    });
    let (cr, mut cw) = tokio::io::split(client);
    let mut lines = BufReader::new(cr).lines();
    let send = |v: Value| format!("{v}\n");
    cw.write_all(
        send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }}),
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let init = rpc_line(&mut lines).await;
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "bleh-history");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    cw.write_all(send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).as_bytes())
        .await
        .unwrap();
    cw.write_all(send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).as_bytes())
        .await
        .unwrap();
    let list = rpc_line(&mut lines).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"get_book_at") && names.contains(&"explain_provenance"));
    cw.write_all(
        send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "get_bars",
            "arguments": {"instrument": "hyperliquid:perp:SYN", "interval": "1h", "start": "2026-01-01", "end": "2026-01-03"},
        }}))
        .as_bytes(),
    )
    .await
    .unwrap();
    let res = rpc_line(&mut lines).await;
    assert_eq!(res["id"], 3);
    assert_eq!(res["result"]["structuredContent"]["count"], 4, "{res}");
    // A tool error is a result with isError, not a protocol error.
    cw.write_all(
        send(
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
                "name": "get_trades",
                "arguments": {"instrument": "hyperliquid:perp:NOPE"},
            }}),
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let res = rpc_line(&mut lines).await;
    assert_eq!(res["result"]["isError"], true, "{res}");
    assert!(
        res["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("unknown_instrument")
    );
}

async fn post_mcp(
    app: &axum::Router,
    host: &str,
    session: Option<&str>,
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Option<Value>) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(s) = session {
        req = req
            .header("mcp-session-id", s)
            .header("mcp-protocol-version", "2025-06-18");
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let mut stream = resp.into_body().into_data_stream();
    let mut buf = String::new();
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if let Ok(v) = serde_json::from_str::<Value>(buf.trim()) {
                return Some(v);
            }
            for line in buf.lines() {
                if let Some(data) = line.strip_prefix("data:")
                    && let Ok(v) = serde_json::from_str::<Value>(data.trim())
                    && v.get("id").is_some()
                {
                    return Some(v);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    (status, headers, found)
}

#[tokio::test]
async fn streamable_http_transport() {
    let app = app();
    let (s, h, init) = post_mcp(
        &app,
        "localhost",
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"},
        }}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let init = init.unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "bleh-history");
    let session = h["mcp-session-id"].to_str().unwrap().to_owned();
    let (s, _, _) = post_mcp(
        &app,
        "localhost",
        Some(&session),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert!(s.is_success(), "{s}");
    let (s, _, res) = post_mcp(
        &app,
        "localhost",
        Some(&session),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "list_instruments", "arguments": {"query": "SYN"}
        }}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let res = res.unwrap();
    assert_eq!(res["result"]["structuredContent"]["total"], 2, "{res}");

    // DNS-rebinding protection: unknown Host headers are refused.
    let (s, _, _) = post_mcp(
        &app,
        "attacker.example",
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}
        }}),
    )
    .await;
    assert!(s.is_client_error(), "{s}");
}
