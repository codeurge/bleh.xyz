//! `mesh-client probe` logic against a real gateway fed by the synthetic
//! demo feed.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::*;
use gateway::GatewayConfig;
use gateway::demo::{DemoConfig, DemoFeed};
use mesh_client::Encoding;
use mesh_client::probe::{self, ProbeConfig, ProbeResult};

fn cfg(o: &Origin, channel: &str, timeout: Duration) -> ProbeConfig {
    ProbeConfig {
        url: ws_url(o.addr),
        channel: channel.parse().unwrap(),
        signers: o.signer.signers(),
        timeout,
        encoding: Encoding::Json,
    }
}

/// Runs the probe while the feed publishes blocks.
async fn probe_live(o: &mut Origin, c: ProbeConfig) -> ProbeResult {
    let task = tokio::spawn(probe::run(c));
    for _ in 0..200 {
        if task.is_finished() {
            break;
        }
        o.feed.publish_next(&mut o.publisher).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    task.await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_reports_heartbeats_snapshots_and_failures() {
    let registry = DemoFeed::new(DemoConfig::default())
        .unwrap()
        .registry()
        .clone();
    let mut o = origin(
        GatewayConfig {
            registry: Some(registry),
            ..GatewayConfig::default()
        },
        DemoConfig::default(),
    )
    .await;
    let syn = "hyperliquid:perp:SYN";

    let c = cfg(&o, &format!("{syn}|trade"), Duration::from_secs(5));
    let r = probe_live(&mut o, c).await;
    assert!(r.ok, "{r:?}");
    assert!(r.first_block_seconds.unwrap() > 0.0);
    assert!(r.block_time_ms.unwrap() > 0);
    assert!(r.snapshot_seconds.is_none() && r.error.is_none());

    for depth in ["l4", "l2"] {
        let c = cfg(
            &o,
            &format!("{syn}|book_event|{depth}"),
            Duration::from_secs(5),
        );
        let r = probe_live(&mut o, c).await;
        assert!(r.ok, "{depth}: {r:?}");
        assert!(r.snapshot_seconds.unwrap() > 0.0, "{depth}: {r:?}");
    }

    let c = cfg(&o, "hyperliquid:perp:NOPE|trade", Duration::from_secs(5));
    let r = probe_live(&mut o, c).await;
    assert!(!r.ok);
    assert!(
        r.error.as_deref().unwrap().contains("unknown_instrument"),
        "{r:?}"
    );

    let mut c = cfg(&o, &format!("{syn}|trade"), Duration::from_secs(5));
    c.signers = vec![
        "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap(),
    ];
    let r = probe_live(&mut o, c).await;
    assert!(!r.ok);
    assert!(r.error.as_deref().unwrap().contains("signer"), "{r:?}");

    // No blocks: times out without a heartbeat.
    let r = probe::run(cfg(&o, &format!("{syn}|trade"), Duration::from_millis(300))).await;
    assert!(!r.ok);
    assert_eq!(r.first_block_seconds, None);
    assert!(
        r.error
            .as_deref()
            .unwrap()
            .starts_with("no block heartbeat"),
        "{r:?}"
    );
}
