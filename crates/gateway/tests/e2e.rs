//! End to end: synthetic records → bus → block signer → gateway →
//! reference client over real localhost websockets, with a verifying relay
//! chained in the middle.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bus::keys::parse_key;
use bus::{MemoryTap, SideTap};
use common::*;
use gateway::demo::DemoConfig;
use gateway::{Gateway, GatewayConfig, Signers};
use mesh_client::{ChannelSpec, ClientConfig, Encoding, SnapshotReason};
use mesh_types::InstrumentId;

fn full_channels() -> Vec<ChannelSpec> {
    ["*|book_event", "*|trade", "*|book_snapshot"]
        .iter()
        .map(|c| c.parse().unwrap())
        .collect()
}

fn client_cfg(url: String) -> ClientConfig {
    ClientConfig {
        urls: vec![url],
        channels: full_channels(),
        commitments: true,
        stall_timeout: Duration::from_secs(5),
        ..ClientConfig::default()
    }
}

fn assert_clean(name: &str, seen: &Seen) {
    assert!(seen.failures.is_empty(), "{name}: {:?}", seen.failures);
    let s = seen.stats;
    assert_eq!(
        s.signatures_failed + s.roots_failed + s.proofs_failed,
        0,
        "{name}: {s:?}"
    );
    assert!(
        s.signatures_ok > 0,
        "{name}: no verified commitments: {s:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn books_match_and_every_commitment_verifies_directly_and_through_a_relay() {
    let mut o = origin(GatewayConfig::default(), DemoConfig::default()).await;
    let allow = o.signer.signers();
    let r = relay(o.addr, allow.clone()).await;
    wait_for("relay upstream", || r.status.is_connected()).await;

    let blocks = 130;
    let last = o.feed.next_block_number() + blocks - 1;
    let until = Arc::new(AtomicU64::new(last));
    let syn = InstrumentId::new("hyperliquid:perp:SYN").unwrap();

    let mut clients = Vec::new();
    let mut readies = Vec::new();
    let configs = vec![
        (
            "origin-json",
            ClientConfig {
                signers: allow.clone(),
                proof_every: 25,
                ..client_cfg(ws_url(o.addr))
            },
        ),
        (
            "relay-cbor",
            ClientConfig {
                encoding: Encoding::Cbor,
                signers: allow.clone(),
                ..client_cfg(ws_url(r.addr))
            },
        ),
        (
            "relay-partial-proofs",
            ClientConfig {
                channels: vec![
                    format!("{syn}|book_event").parse().unwrap(),
                    format!("{syn}|trade").parse().unwrap(),
                ],
                proof_every: 3,
                ..client_cfg(ws_url(r.addr))
            },
        ),
    ];
    for (name, cfg) in configs {
        let (tx, rx) = tokio::sync::oneshot::channel();
        clients.push((name, run_client(cfg, Arc::clone(&until), tx)));
        readies.push(rx);
    }
    for r in readies {
        r.await.unwrap();
    }

    publish_blocks(&mut o, blocks, Duration::from_millis(8)).await;

    for (name, h) in clients {
        let seen = h.await.unwrap();
        assert_clean(name, &seen);
        assert_eq!(seen.gaps, 0, "{name}");
        let instruments: Vec<InstrumentId> = if name == "relay-partial-proofs" {
            vec![syn.clone()]
        } else {
            o.feed.instruments().cloned().collect()
        };
        for inst in &instruments {
            let mine = seen
                .book(inst)
                .unwrap_or_else(|| panic!("{name}: no book for {inst}"));
            assert_eq!(Some(mine), o.feed.book(inst), "{name}: book {inst}");
            assert!(mine.last_block().unwrap_or(0) <= last);
        }
        match name {
            "relay-partial-proofs" => {
                assert_eq!(seen.stats.roots_ok, 0);
                assert!(seen.stats.proofs_ok > 0, "{name}: {:?}", seen.stats);
            }
            "origin-json" => {
                assert!(seen.stats.roots_ok > 100, "{name}: {:?}", seen.stats);
                assert!(seen.stats.proofs_ok > 0, "{name}: {:?}", seen.stats);
            }
            _ => assert!(seen.stats.roots_ok > 100, "{name}: {:?}", seen.stats),
        }
        assert!(
            seen.snapshot_reasons
                .iter()
                .all(|r| *r == SnapshotReason::Subscribe)
        );
    }
    let m = o.handle.metrics();
    assert!(m.send_latency.count() > 0);
    let rm = r.handle.metrics();
    assert!(rm.relay_blocks_verified.get() > 100);
    assert_eq!(rm.relay_streams_rejected.get(), 0);
    eprintln!(
        "send latency (ts_ingest -> send): p50 <= {:?}s, p99 <= {:?}s over {} sends; relay p99 <= {:?}s",
        m.send_latency.quantile_upper_bound(0.5),
        m.send_latency.quantile_upper_bound(0.99),
        m.send_latency.count(),
        rm.send_latency.quantile_upper_bound(0.99),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upstream_gaps_force_resnapshots_and_books_recover() {
    let mut o = origin(
        GatewayConfig::default(),
        DemoConfig {
            checkpoint_every: 5,
            gap_every: Some(7),
            ..DemoConfig::default()
        },
    )
    .await;
    let allow = o.signer.signers();
    let r = relay(o.addr, allow.clone()).await;
    wait_for("relay upstream", || r.status.is_connected()).await;
    // Ends right after a checkpoint that follows the last gap.
    let blocks = 106;
    let last = o.feed.next_block_number() + blocks - 1;
    let until = Arc::new(AtomicU64::new(last));
    let (ta, ra) = tokio::sync::oneshot::channel();
    let a = run_client(
        ClientConfig {
            signers: allow.clone(),
            ..client_cfg(ws_url(o.addr))
        },
        Arc::clone(&until),
        ta,
    );
    let (tb, rb) = tokio::sync::oneshot::channel();
    let b = run_client(client_cfg(ws_url(r.addr)), Arc::clone(&until), tb);
    ra.await.unwrap();
    rb.await.unwrap();
    publish_blocks(&mut o, blocks, Duration::from_millis(8)).await;
    for (name, h) in [("origin", a), ("relay", b)] {
        let seen = h.await.unwrap();
        assert_clean(name, &seen);
        assert!(seen.gaps > 0, "{name}: expected gaps");
        assert!(
            seen.snapshot_reasons
                .iter()
                .any(|r| matches!(r, SnapshotReason::UpstreamGap | SnapshotReason::Reconcile)),
            "{name}: {:?}",
            seen.snapshot_reasons
        );
        for inst in o.feed.instruments() {
            assert_eq!(seen.book(inst), o.feed.book(inst), "{name}: book {inst}");
        }
    }
    assert!(o.handle.metrics().upstream_gaps.get() > 0);
    assert!(o.handle.metrics().forced_snapshots_sent.get() > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_rotates_urls_and_survives_key_rotation_and_gateway_loss() {
    let mut o = origin(GatewayConfig::default(), DemoConfig::default()).await;
    // A second gateway on the same bus and signer.
    let second = Gateway::new(
        GatewayConfig::default(),
        o.ring.subscribe(),
        Signers::Local(Arc::clone(&o.signer)),
        Arc::clone(&o.proofs),
    )
    .start();
    let second_addr = serve(&second).await;
    // Publish the next key before anyone connects, rotate to it later.
    o.signer.add_key("b", parse_key(KEY_B).unwrap()).unwrap();
    let dead = dead_addr().await;
    let blocks = 80;
    let last = o.feed.next_block_number() + blocks - 1;
    let until = Arc::new(AtomicU64::new(last));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let c = run_client(
        ClientConfig {
            urls: vec![ws_url(dead), ws_url(o.addr), ws_url(second_addr)],
            min_backoff: Duration::from_millis(10),
            ..client_cfg(String::new())
        },
        until,
        tx,
    );
    rx.await.unwrap();
    publish_blocks(&mut o, 30, Duration::from_millis(8)).await;
    o.signer.rotate("b").unwrap();
    publish_blocks(&mut o, 10, Duration::from_millis(8)).await;
    o.handle.shutdown();
    publish_blocks(&mut o, 40, Duration::from_millis(15)).await;
    let seen = c.await.unwrap();
    assert_clean("failover", &seen);
    assert_eq!(
        seen.connected_urls,
        vec![ws_url(o.addr), ws_url(second_addr)]
    );
    for inst in o.feed.instruments() {
        assert_eq!(seen.book(inst), o.feed.book(inst), "book {inst}");
    }
    assert_eq!(second.signers()[0], parse_key(KEY_B).unwrap().address());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarted_gateway_rebuilds_from_the_side_tap() {
    let tap = Arc::new(MemoryTap::new(Duration::from_secs(60), 100_000));
    let mut o = origin(GatewayConfig::default(), DemoConfig::default()).await;
    o.publisher = bus::BlockPublisher::new(o.ring.clone())
        .with_signer(Arc::clone(&o.signer), Arc::clone(&o.proofs))
        .with_tap(tap.clone());
    publish_blocks(&mut o, 15, Duration::ZERO).await;
    // A fresh gateway (new proof store) restarted from the tap.
    let proofs = Arc::new(bus::ProofStore::new(1000));
    let replayed = tap.replay().await.unwrap();
    assert!(!replayed.is_empty());
    let restarted = Gateway::new(
        GatewayConfig::default(),
        o.ring.subscribe(),
        Signers::Local(Arc::clone(&o.signer)),
        Arc::clone(&proofs),
    )
    .with_replay(replayed)
    .start();
    let syn = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
    assert_eq!(restarted.book(&syn).as_ref(), o.feed.book(&syn));
    assert!(!proofs.is_empty());
    let addr = serve(&restarted).await;
    let last = o.feed.next_block_number() + 4;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let c = run_client(client_cfg(ws_url(addr)), Arc::new(AtomicU64::new(last)), tx);
    rx.await.unwrap();
    publish_blocks(&mut o, 5, Duration::from_millis(10)).await;
    let seen = c.await.unwrap();
    assert_clean("restarted", &seen);
    for inst in o.feed.instruments() {
        assert_eq!(seen.book(inst), o.feed.book(inst), "book {inst}");
    }
    let _ = Ordering::SeqCst;
}
