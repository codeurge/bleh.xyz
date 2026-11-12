//! Websocket replay in the gateway-v0 message format.

#![allow(clippy::unwrap_used, missing_docs)]

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use common::*;
use futures::{SinkExt, StreamExt};
use history_api::book::L4Book;
use history_api::config::Config;
use history_api::http::{AppState, router_with_mcp};
use history_api::service::Service;
use mesh_types::{BlockBatch, BookSnapshot, Record, RecordKind};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

async fn spawn(config: Config) -> SocketAddr {
    let svc = Service::open(config).unwrap();
    let app = router_with_mcp(AppState::new(svc));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    addr
}

async fn server() -> SocketAddr {
    let f = fixture();
    spawn(config(&f.root, Some(f.registry.clone()))).await
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_json(ws: &mut Ws) -> Option<Value> {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), ws.next())
            .await
            .ok()??
        {
            Ok(Message::Text(t)) => return Some(serde_json::from_str(t.as_str()).unwrap()),
            Ok(Message::Close(frame)) => {
                let code = frame.map(|f| u16::from(f.code)).unwrap_or(0);
                return Some(json!({"type": "__close", "code": code}));
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

#[tokio::test]
async fn replays_a_range_in_gateway_order() {
    let addr = server().await;
    let (s0, e0) = (start() + 20 * 60_000, start() + 50 * 60_000);
    let (mut ws, resp) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/v0/replay?start={s0}&end={e0}&speed=0"
    ))
    .await
    .unwrap();
    assert_eq!(
        resp.headers()
            .get("sec-websocket-protocol")
            .map(|v| v.to_str().unwrap()),
        None,
        "no subprotocol requested, none selected"
    );
    let hello = next_json(&mut ws).await.unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["protocol"], "bleh-gateway-v0");
    ws.send(Message::Text(
        json!({"op": "subscribe", "id": "s1", "channels": [
            {"instrument": "hyperliquid:perp:SYN", "kind": "trade"},
            {"instrument": "hyperliquid:perp:SYN", "kind": "book_event", "depth": "l4"},
            {"instrument": "hyperliquid:perp:syn:IDX", "kind": "bar", "interval": "1m"},
        ]})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let sub = next_json(&mut ws).await.unwrap();
    assert_eq!(sub["type"], "subscribed");
    assert_eq!(sub["channels"].as_array().unwrap().len(), 3);
    let snap = next_json(&mut ws).await.unwrap();
    assert_eq!(snap["type"], "snapshot", "{snap}");
    assert_eq!(snap["reason"], "subscribe");
    let Record::BookSnapshot(snapshot): Record =
        serde_json::from_value(snap["record"].clone()).unwrap()
    else {
        panic!()
    };
    let snapshot: BookSnapshot = snapshot;
    assert!(snapshot.prov.ts_publish_ns.is_some());
    let mut book = L4Book::from_snapshot(&snapshot).unwrap();
    let mut trades = Vec::new();
    let mut last_block: BTreeMap<String, u64> = BTreeMap::new();
    last_block.insert(
        "book".into(),
        snap["as_of"]["block_number"].as_u64().unwrap(),
    );
    let mut first_trade_batch = true;
    let mut heartbeats = 0;
    let mut prev_hb_block = 0u64;
    let mut current_block_time = 0i64;
    let mut bars = Vec::new();
    let mut flushed = false;
    let close_code = loop {
        let m = next_json(&mut ws).await.unwrap();
        match m["type"].as_str().unwrap() {
            "batch" => {
                assert!(!flushed, "a bar was emitted before its interval closed");
                let b: BlockBatch = serde_json::from_value(m["batch"].clone()).unwrap();
                b.validate().unwrap();
                assert!(b.block_number > prev_hb_block, "blocks ascend");
                current_block_time = b.block_time_ms.0;
                match b.kind {
                    RecordKind::Trade => {
                        if first_trade_batch {
                            assert_eq!(b.prev_block_number, None);
                            first_trade_batch = false;
                        } else {
                            assert_eq!(b.prev_block_number, last_block.get("trade").copied());
                        }
                        last_block.insert("trade".into(), b.block_number);
                        trades.extend(b.records);
                    }
                    RecordKind::BookEvent => {
                        let last = last_block["book"];
                        assert!(b.prev_block_number.unwrap() <= last);
                        last_block.insert("book".into(), b.block_number);
                        for r in &b.records {
                            let Record::BookEvent(e) = r else { panic!() };
                            assert!(e.prov.ts_publish_ns.is_some());
                            book.apply(e);
                        }
                    }
                    other => panic!("unexpected batch kind {other}"),
                }
            }
            "record" => {
                let Record::Bar(b): Record = serde_json::from_value(m["record"].clone()).unwrap()
                else {
                    panic!()
                };
                let close = b.start_ms.0 + 60_000;
                // Either the current block is at or after the close, or this
                // is the final flush after the last block.
                if current_block_time < close {
                    flushed = true;
                }
                bars.push(b);
            }
            "block" => {
                heartbeats += 1;
                let n = m["block_number"].as_u64().unwrap();
                assert!(n > prev_hb_block);
                prev_hb_block = n;
                assert!(
                    m["last"]
                        .as_object()
                        .unwrap()
                        .contains_key("hyperliquid:perp:SYN|book_event")
                );
            }
            "__close" => break m["code"].as_u64().unwrap(),
            other => panic!("unexpected message {other}: {m}"),
        }
    };
    assert_eq!(close_code, 1000);
    assert!(heartbeats > 10);
    // Trades are exactly the archive's.
    let got: Vec<Value> = trades
        .into_iter()
        .map(|mut r| {
            if let Record::Trade(t) = &mut r {
                t.prov.ts_publish_ns = None;
            }
            serde_json::to_value(r).unwrap()
        })
        .collect();
    assert_eq!(got, expected(RecordKind::Trade, &syn(), s0, e0));
    // The book a client builds from snapshot + deltas equals the API's book.
    let svc = service();
    let want = svc
        .book_at(
            "hyperliquid:perp:SYN",
            Some(&(e0 - 1).to_string()),
            Some("l4"),
            None,
        )
        .await
        .unwrap();
    let Record::BookSnapshot(w) = want.book else {
        panic!()
    };
    assert_eq!(book.depth(false, None).unwrap(), w.depth);
    assert_eq!(book.inconsistencies.unknown_order, 0);
    // Bars whose interval closes inside the range, each after it closed.
    let want_bars: Vec<i64> = all(RecordKind::Bar, &idx())
        .into_iter()
        .filter_map(|r| match r {
            Record::Bar(b) if b.interval == mesh_types::BarInterval::M1 => Some(b.start_ms.0),
            _ => None,
        })
        .filter(|s| s + 60_000 >= s0 && s + 60_000 <= e0)
        .collect();
    assert_eq!(
        bars.iter().map(|b| b.start_ms.0).collect::<Vec<_>>(),
        want_bars
    );
}

#[tokio::test]
async fn speed_paces_the_stream() {
    let addr = server().await;
    let s0 = start() + 2 * HOUR;
    let run = |speed: f64| async move {
        let (mut ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://{addr}/v0/replay?start={s0}&end={}&speed={speed}",
            s0 + 60_000
        ))
        .await
        .unwrap();
        let _ = next_json(&mut ws).await;
        ws.send(Message::Text(
            json!({"op": "subscribe", "id": "x", "channels": [{"instrument": "hyperliquid:perp:SYN", "kind": "book_event"}]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let t = Instant::now();
        loop {
            let m = next_json(&mut ws).await.unwrap();
            if m["type"] == "__close" {
                break;
            }
        }
        t.elapsed()
    };
    let fast = run(0.0).await;
    let paced = run(60.0).await;
    assert!(paced >= Duration::from_millis(600), "{paced:?}");
    assert!(fast < paced, "{fast:?} vs {paced:?}");
}

#[tokio::test]
async fn control_messages_and_errors() {
    let addr = server().await;
    let s0 = start();
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/v0/replay?start={s0}&end={}&speed=1",
        s0 + 10 * 60_000
    ))
    .await
    .unwrap();
    let _ = next_json(&mut ws).await;
    ws.send(Message::Text(
        json!({"op": "ping", "id": "p0"}).to_string().into(),
    ))
    .await
    .unwrap();
    assert_eq!(next_json(&mut ws).await.unwrap()["type"], "pong");
    ws.send(Message::Text(
        json!({"op": "subscribe", "id": "s", "channels": [
            {"instrument": "hyperliquid:perp:NOPE", "kind": "trade"},
            {"instrument": "hyperliquid:perp:SYN", "kind": "swap_print"},
            {"instrument": "hyperliquid:perp:SYN", "kind": "trade"},
        ]})
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let e1 = next_json(&mut ws).await.unwrap();
    assert_eq!(e1["code"], "unknown_instrument");
    let e2 = next_json(&mut ws).await.unwrap();
    assert_eq!(e2["code"], "unsupported_kind");
    let sub = next_json(&mut ws).await.unwrap();
    assert_eq!(sub["type"], "subscribed");
    assert_eq!(sub["channels"].as_array().unwrap().len(), 1);
    // Mid-stream: ping, an unknown field, and a second subscribe.
    ws.send(Message::Text(
        json!({"op": "ping", "id": "p1"}).to_string().into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(
        json!({"op": "ping", "id": "p2", "extra": 1})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(
        json!({"op": "subscribe", "id": "s2", "channels": []})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let mut seen = Vec::new();
    for _ in 0..200 {
        let m = next_json(&mut ws).await.unwrap();
        match m["type"].as_str().unwrap() {
            "pong" => seen.push("pong".to_owned()),
            "error" => seen.push(m["code"].as_str().unwrap().to_owned()),
            _ => {}
        }
        if seen.len() == 3 {
            break;
        }
    }
    assert!(seen.contains(&"pong".to_owned()));
    assert_eq!(
        seen.iter().filter(|s| *s == "bad_request").count(),
        2,
        "{seen:?}"
    );
    ws.close(None).await.unwrap();
}

#[tokio::test]
async fn rejects_bad_requests_and_caps_replays_per_ip() {
    let f = fixture();
    let mut cfg = config(&f.root, Some(f.registry.clone()));
    cfg.limits.max_replays_per_ip = 1;
    let addr = spawn(cfg).await;
    let err = tokio_tungstenite::connect_async(format!("ws://{addr}/v0/replay?start=nope"))
        .await
        .unwrap_err();
    match err {
        tokio_tungstenite::tungstenite::Error::Http(r) => assert_eq!(r.status(), 400),
        other => panic!("{other:?}"),
    }
    let err = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/v0/replay?start=2026-01-01&end=2026-03-01"
    ))
    .await
    .unwrap_err();
    assert!(matches!(err, tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 400));
    let s0 = start();
    let (mut first, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v0/replay?start={s0}"))
            .await
            .unwrap();
    let _ = next_json(&mut first).await;
    let err = tokio_tungstenite::connect_async(format!("ws://{addr}/v0/replay?start={s0}"))
        .await
        .unwrap_err();
    assert!(matches!(err, tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 429));
    first.close(None).await.unwrap();
    drop(first);
    // The slot frees once the first replay ends.
    let mut ok = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if tokio_tungstenite::connect_async(format!("ws://{addr}/v0/replay?start={s0}"))
            .await
            .is_ok()
        {
            ok = true;
            break;
        }
    }
    assert!(ok);
}
