//! Protocol limits, errors, HTTP endpoints and the slow-consumer policy,
//! over real localhost connections.

#![allow(clippy::unwrap_used)]

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::*;
use futures::{SinkExt, StreamExt};
use gateway::GatewayConfig;
use gateway::demo::DemoConfig;
use mesh_client::Limits;
use mesh_types::{BlockCommitment, MerkleProof, Record};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn raw(addr: SocketAddr, protocol: Option<&str>) -> Result<Ws, String> {
    let mut req = ws_url(addr).into_client_request().unwrap();
    if let Some(p) = protocol {
        req.headers_mut()
            .insert("Sec-WebSocket-Protocol", HeaderValue::from_str(p).unwrap());
    }
    tokio_tungstenite::connect_async(req)
        .await
        .map(|(ws, _)| ws)
        .map_err(|e| e.to_string())
}

/// Next JSON message, or the close code.
async fn next(ws: &mut Ws) -> Result<Value, Option<u16>> {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
        {
            Some(Ok(Message::Text(t))) => return Ok(serde_json::from_str(&t).unwrap()),
            Some(Ok(Message::Close(f))) => return Err(f.map(|f| u16::from(f.code))),
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return Err(None),
        }
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn expect_type(ws: &mut Ws, ty: &str) -> Value {
    loop {
        let v = next(ws)
            .await
            .unwrap_or_else(|c| panic!("closed {c:?} waiting for {ty}"));
        if v["type"] == ty {
            return v;
        }
    }
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8(buf).unwrap();
    let status = text[9..12].parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_errors_and_caps() {
    let mut o = origin(
        GatewayConfig {
            limits: Limits {
                channels_per_connection: 3,
                connections_per_ip: 3,
                ..Limits::default()
            },
            registry: Some(
                gateway::demo::DemoFeed::new(DemoConfig::default())
                    .unwrap()
                    .registry()
                    .clone(),
            ),
            ..GatewayConfig::default()
        },
        DemoConfig::default(),
    )
    .await;
    let mut ws = raw(o.addr, None).await.unwrap();
    let hello = next(&mut ws).await.unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["protocol"], "bleh-gateway-v0");
    assert_eq!(hello["limits"]["channels_per_connection"], 3);
    assert_eq!(hello["signers"][0], o.signer.signers()[0].to_string());

    send(&mut ws, json!({"op": "ping", "id": "p1"})).await;
    let pong = expect_type(&mut ws, "pong").await;
    assert_eq!(pong["id"], "p1");

    ws.send(Message::Text("{not json".into())).await.unwrap();
    assert_eq!(expect_type(&mut ws, "error").await["code"], "bad_request");
    send(&mut ws, json!({"op": "ping", "id": "x", "surprise": 1})).await;
    assert_eq!(expect_type(&mut ws, "error").await["code"], "bad_request");

    send(&mut ws, json!({"op":"subscribe","id":"s1","channels":[{"instrument":"hyperliquid:perp:NOPE","kind":"trade"}]})).await;
    let e = expect_type(&mut ws, "error").await;
    assert_eq!(
        (e["code"].as_str(), e["id"].as_str()),
        (Some("unknown_instrument"), Some("s1"))
    );

    send(&mut ws, json!({"op":"subscribe","id":"s2","channels":[{"instrument":"*","kind":"trade","depth":"l2"}]})).await;
    assert_eq!(
        expect_type(&mut ws, "error").await["code"],
        "unsupported_kind"
    );

    send(
        &mut ws,
        json!({"op":"subscribe","id":"s3","channels":[
        {"instrument":"*","kind":"trade"},{"instrument":"*","kind":"book_event"},
        {"instrument":"*","kind":"bar"},{"instrument":"*","kind":"book_snapshot"}]}),
    )
    .await;
    let e = expect_type(&mut ws, "error").await;
    assert_eq!(e["code"], "limit_exceeded");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("channels per connection")
    );

    send(
        &mut ws,
        json!({"op":"resnapshot","id":"r1","instrument":"hyperliquid:perp:SYN","reason":"test"}),
    )
    .await;
    let e = expect_type(&mut ws, "error").await;
    assert!(
        e["message"].as_str().unwrap().contains("not subscribed"),
        "{e}"
    );
    send(&mut ws, json!({"op":"subscribe","id":"s4","channels":[{"instrument":"*","kind":"book_event"}],"commitments":true})).await;
    assert_eq!(expect_type(&mut ws, "subscribed").await["id"], "s4");

    // Data flows: snapshot (after the first checkpoint), batch, commitment, heartbeat.
    publish_blocks(&mut o, 3, Duration::from_millis(5)).await;
    let snap = expect_type(&mut ws, "snapshot").await;
    assert_eq!(snap["reason"], "subscribe");
    let block = expect_type(&mut ws, "block").await;
    assert_eq!(block["venue"], "hyperliquid");
    assert!(
        block["last"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| k.ends_with("|book_event"))
    );

    // Proof over websocket for a published record.
    let inst = "hyperliquid:perp:SYN";
    let b = o.feed.next_block_number() - 1;
    send(
        &mut ws,
        json!({"op":"proof","id":"pf","stream":{"venue":"hyperliquid","kind":"book_snapshot"},
        "block_number": b - 2, "instrument": inst, "event_index": u32::MAX}),
    )
    .await;
    let p = expect_type(&mut ws, "proof").await;
    let c: BlockCommitment = serde_json::from_value(p["commitment"].clone()).unwrap();
    let proof: MerkleProof = serde_json::from_value(p["proof"].clone()).unwrap();
    let rec: Record = serde_json::from_value(p["record"].clone()).unwrap();
    c.verify_signature().unwrap();
    c.verify_inclusion(&rec, &proof).unwrap();
    send(
        &mut ws,
        json!({"op":"proof","id":"old","stream":{"venue":"hyperliquid","kind":"trade"},
        "block_number": 1, "instrument": inst, "event_index": 0}),
    )
    .await;
    let e = expect_type(&mut ws, "error").await;
    assert_eq!(
        (e["code"].as_str(), e["id"].as_str()),
        (Some("proof_unavailable"), Some("old"))
    );

    // Per-IP connection cap: 3 allowed, the 4th refused with HTTP 429.
    let _b = raw(o.addr, None).await.unwrap();
    let _c = raw(o.addr, None).await.unwrap();
    let err = raw(o.addr, None).await.unwrap_err();
    assert!(err.contains("429"), "{err}");
    assert_eq!(o.handle.metrics().rejected_ip_cap.get(), 1);

    // Unsupported subprotocol: 4001.
    drop(_b);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut bad = raw(o.addr, Some("bleh.v9.json")).await.unwrap();
    assert_eq!(next(&mut bad).await.unwrap_err(), Some(4001));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limits_and_frame_size_close_the_connection() {
    let o = origin(
        GatewayConfig {
            limits: Limits {
                messages_per_second: 1,
                message_burst: 2,
                max_message_bytes: 512,
                ..Limits::default()
            },
            violations_before_close: 3,
            ..GatewayConfig::default()
        },
        DemoConfig::default(),
    )
    .await;
    let mut ws = raw(o.addr, None).await.unwrap();
    next(&mut ws).await.unwrap(); // hello
    for i in 0..10 {
        let _ = ws
            .send(Message::Text(
                json!({"op":"ping","id": i.to_string()}).to_string().into(),
            ))
            .await;
    }
    let mut limited = 0;
    let code = loop {
        match next(&mut ws).await {
            Ok(v) if v["type"] == "error" => {
                assert_eq!(v["code"], "rate_limited");
                limited += 1;
            }
            Ok(_) => {}
            Err(code) => break code,
        }
    };
    assert_eq!(code, Some(4029));
    assert_eq!(limited, 2);
    assert_eq!(o.handle.metrics().rate_limit_disconnects.get(), 1);

    let mut ws = raw(o.addr, None).await.unwrap();
    next(&mut ws).await.unwrap();
    let big = format!("{{\"op\":\"ping\",\"id\":\"{}\"}}", "x".repeat(2048));
    let _ = ws.send(Message::Text(big.into())).await;
    assert_eq!(next(&mut ws).await.unwrap_err(), Some(1009));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_endpoints() {
    let mut o = origin(GatewayConfig::default(), DemoConfig::default()).await;
    let (status, _) = http_get(o.addr, "/health").await;
    assert_eq!(status, 503);
    // A client connection so the latency histogram has observations.
    let mut ws = raw(o.addr, None).await.unwrap();
    next(&mut ws).await.unwrap();
    send(
        &mut ws,
        json!({"op":"subscribe","id":"s","channels":[{"instrument":"*","kind":"trade"}]}),
    )
    .await;
    expect_type(&mut ws, "subscribed").await;
    publish_blocks(&mut o, 12, Duration::from_millis(2)).await;
    expect_type(&mut ws, "batch").await;

    let (status, body) = http_get(o.addr, "/health").await;
    assert_eq!((status, body.as_str()), (200, r#"{"status":"ok"}"#));

    let (status, body) = http_get(o.addr, "/freshness").await;
    assert_eq!(status, 200);
    let f: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(f["venues"][0]["venue"], "hyperliquid");
    let insts = f["instruments"].as_array().unwrap();
    assert!(
        insts
            .iter()
            .any(|r| r["instrument"] == "hyperliquid:perp:SYN" && r["kind"] == "book_event")
    );
    assert!(insts.iter().all(|r| r["lag_ms"].as_i64().unwrap() >= 0));

    let (status, body) = http_get(o.addr, "/metrics").await;
    assert_eq!(status, 200);
    assert!(body.contains("gateway_send_latency_seconds_bucket{le=\"0.05\"}"));
    assert!(body.contains("gateway_connections 1"));
    assert!(body.contains("gateway_blocks_total 12"));

    let (status, body) = http_get(o.addr, "/signers").await;
    assert_eq!(status, 200);
    let s: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(s["signers"][0], o.signer.signers()[0].to_string());
    assert_eq!(s["keys"][0]["id"], "a");
    assert_eq!(s["keys"][0]["active"], true);

    let b = o.feed.next_block_number() - 1;
    let (status, body) = http_get(
        o.addr,
        &format!("/proof?venue=hyperliquid&kind=book_snapshot&block_number={}&instrument=hyperliquid:perp:SYN&event_index={}", b - 1, u32::MAX),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let p: Value = serde_json::from_str(&body).unwrap();
    let c: BlockCommitment = serde_json::from_value(p["commitment"].clone()).unwrap();
    let proof: MerkleProof = serde_json::from_value(p["proof"].clone()).unwrap();
    let rec: Record = serde_json::from_value(p["record"].clone()).unwrap();
    c.verify_inclusion(&rec, &proof).unwrap();
    let (status, _) = http_get(o.addr, "/proof?venue=hyperliquid").await;
    assert_eq!(status, 400);
    let (status, body) = http_get(
        o.addr,
        "/proof?venue=hyperliquid&kind=trade&block_number=1&instrument=hyperliquid:perp:SYN&event_index=0",
    )
    .await;
    assert_eq!(status, 404);
    assert!(body.contains("proof_unavailable"));
    let (status, _) = http_get(o.addr, "/v0/ws").await;
    assert_eq!(status, 426);

    // A relay whose upstream is down is unhealthy.
    let dead = dead_addr().await;
    let r = relay(dead, o.signer.signers()).await;
    let (status, body) = http_get(r.addr, "/health").await;
    assert_eq!(status, 503);
    assert!(body.contains("\"upstream_connected\":false"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_consumer_drops_to_snapshot_then_disconnects() {
    let mut o = origin(
        GatewayConfig {
            limits: Limits {
                send_buffer_ms: 40,
                ..Limits::default()
            },
            ..GatewayConfig::default()
        },
        DemoConfig {
            checkpoint_every: 1,
            ..DemoConfig::default()
        },
    )
    .await;
    let mut ws = raw(o.addr, None).await.unwrap();
    next(&mut ws).await.unwrap();
    send(
        &mut ws,
        json!({"op":"subscribe","id":"s","channels":[
        {"instrument":"*","kind":"book_event"},{"instrument":"*","kind":"trade"},
        {"instrument":"*","kind":"book_snapshot"}],"commitments":true}),
    )
    .await;
    expect_type(&mut ws, "subscribed").await;
    // Stop reading while the feed runs flat out.
    let feeder = tokio::spawn(async move {
        for _ in 0..3000 {
            o.feed.publish_next(&mut o.publisher).unwrap();
            if o.handle.metrics().slow_consumer_disconnects.get() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        o
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let o = feeder.await.unwrap();
    let m = o.handle.metrics();
    assert!(
        m.slow_consumer_resnapshots.get() + m.slow_consumer_disconnects.get() > 0,
        "no slow-consumer action"
    );
    // Draining now ends in a slow_consumer snapshot, a 4008 close, or a drop.
    let mut saw = false;
    loop {
        match next(&mut ws).await {
            Ok(v) if v["type"] == "snapshot" && v["reason"] == "slow_consumer" => saw = true,
            Ok(_) => {}
            Err(code) => {
                assert!(code.is_none() || code == Some(4008), "{code:?}");
                break;
            }
        }
    }
    assert!(saw || m.slow_consumer_disconnects.get() > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l2_subscribers_get_levels_instead_of_book_batches() {
    let mut o = origin(GatewayConfig::default(), DemoConfig::default()).await;
    let mut ws = raw(o.addr, None).await.unwrap();
    next(&mut ws).await.unwrap();
    send(
        &mut ws,
        json!({"op":"subscribe","id":"s","channels":[
        {"instrument":"*","kind":"book_event","depth":"l2"}]}),
    )
    .await;
    expect_type(&mut ws, "subscribed").await;
    publish_blocks(&mut o, 30, Duration::from_millis(2)).await;
    let snap = expect_type(&mut ws, "snapshot").await;
    assert_eq!(snap["record"]["depth"]["type"], "l2", "{snap}");
    let mut levels = 0;
    let mut blocks = 0;
    while blocks < 15 {
        let v = next(&mut ws).await.unwrap();
        match v["type"].as_str().unwrap() {
            "levels" => {
                levels += 1;
                assert!(v["block_number"].as_u64().unwrap() > 0);
                assert!(v.get("prev_block_number").is_some());
                for l in v["bids"].as_array().unwrap() {
                    let removed = l["size"] == "0";
                    assert_eq!(removed, l["order_count"] == 0, "{l}");
                }
            }
            "batch" => panic!("l2 subscriber got a batch: {v}"),
            "block" => blocks += 1,
            _ => {}
        }
    }
    assert!(levels > 0);
}
