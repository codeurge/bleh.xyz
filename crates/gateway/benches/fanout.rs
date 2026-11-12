//! Fan-out load sketch: one origin gateway, many websocket clients.
//!
//! ```text
//! cargo bench -p gateway --bench fanout                    # 200 clients, 300 blocks
//! CLIENTS=1000 BLOCKS=500 cargo bench -p gateway --bench fanout
//! CLIENTS=1000 BLOCKS=200 INTERVAL_MS=20 cargo bench -p gateway --bench fanout   # paced
//! ```
//!
//! Without `INTERVAL_MS` blocks are published back to back (a saturation
//! test: latency then measures queueing); with it, at that pace.
//!
//! Every client subscribes to every instrument's trades and book events
//! with commitments (JSON) and reads as fast as it can. The feed publishes
//! signed synthetic blocks back to back. Reported: messages and bytes
//! delivered per second across all clients, and the gateway's send-latency
//! histogram (send time minus record `ts_ingest_ns`).

#![allow(clippy::unwrap_used, unreachable_pub)]

#[path = "../tests/common/mod.rs"]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use gateway::GatewayConfig;
use gateway::demo::DemoConfig;
use mesh_client::Limits;
use tokio_tungstenite::tungstenite::Message;

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    // `cargo test --benches` passes `--bench`-less args; only run when asked.
    if std::env::args().any(|a| a == "--list") {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(run());
}

async fn run() {
    let clients = env("CLIENTS", 200);
    let blocks = env("BLOCKS", 300);
    let interval = Duration::from_millis(env("INTERVAL_MS", 0));
    let mut o = common::origin(
        GatewayConfig {
            limits: Limits {
                connections_per_ip: 1_000_000,
                ..Limits::default()
            },
            channels_per_ip: u32::MAX,
            ..GatewayConfig::default()
        },
        DemoConfig::default(),
    )
    .await;
    let last = o.feed.next_block_number() + blocks - 1;
    let messages = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::new();
    let mut readies = Vec::new();
    for _ in 0..clients {
        let url = common::ws_url(o.addr);
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        readies.push(rx);
        let messages = Arc::clone(&messages);
        let bytes = Arc::clone(&bytes);
        tasks.push(tokio::spawn(async move {
            // The OS accept backlog is small (128 on macOS): retry resets.
            let mut ws = None;
            for attempt in 0..50u64 {
                match tokio_tungstenite::connect_async(url.as_str()).await {
                    Ok((w, _)) => {
                        ws = Some(w);
                        break;
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(10 * (attempt + 1))).await,
                }
            }
            let mut ws = ws.expect("connect");
            ws.next().await.unwrap().unwrap(); // hello
            let sub = r#"{"op":"subscribe","id":"s","channels":[{"instrument":"*","kind":"trade"},{"instrument":"*","kind":"book_event"}],"commitments":true}"#;
            ws.send(Message::Text(sub.into())).await.unwrap();
            let mut tx = Some(tx);
            let needle = format!("\"block_number\":{last},");
            while let Some(Ok(m)) = ws.next().await {
                let Message::Text(t) = m else { continue };
                messages.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(t.len() as u64, Ordering::Relaxed);
                if let Some(r) = tx.take_if(|_| t.contains("\"subscribed\"")) {
                    let _ = r.send(());
                }
                if t.starts_with("{\"type\":\"block\"") && t.contains(&needle) {
                    return;
                }
            }
        }));
    }
    for r in readies {
        r.await.unwrap();
    }
    let start = Instant::now();
    for i in 0..blocks {
        o.feed.publish_next(&mut o.publisher).unwrap();
        if !interval.is_zero() {
            tokio::time::sleep(interval).await;
        } else if i % 8 == 0 {
            tokio::task::yield_now().await;
        }
    }
    let published = start.elapsed();
    for t in tasks {
        tokio::time::timeout(Duration::from_secs(120), t)
            .await
            .unwrap()
            .unwrap();
    }
    let total = start.elapsed();
    let m = o.handle.metrics();
    let msgs = messages.load(Ordering::Relaxed);
    let b = bytes.load(Ordering::Relaxed);
    let secs = total.as_secs_f64();
    println!("fan-out: {clients} clients, {blocks} blocks, interval {interval:?}");
    println!(
        "  published in {:.3}s ({:.0} blocks/s, signing included)",
        published.as_secs_f64(),
        blocks as f64 / published.as_secs_f64()
    );
    println!("  delivered to every client in {secs:.3}s");
    println!(
        "  {msgs} messages, {:.1} MB: {:.0} msg/s, {:.1} MB/s aggregate",
        b as f64 / 1e6,
        msgs as f64 / secs,
        b as f64 / 1e6 / secs
    );
    println!(
        "  gateway send latency (send - ts_ingest): p50 <= {:?}s, p99 <= {:?}s, p999 <= {:?}s over {} sends",
        m.send_latency.quantile_upper_bound(0.5),
        m.send_latency.quantile_upper_bound(0.99),
        m.send_latency.quantile_upper_bound(0.999),
        m.send_latency.count()
    );
    println!(
        "  slow-consumer actions: {} resnapshots, {} disconnects",
        m.slow_consumer_resnapshots.get(),
        m.slow_consumer_disconnects.get()
    );
}
