//! Shared setup for the gateway integration tests: an origin gateway fed
//! by the synthetic demo feed through the bus and block signer, optional
//! relays, and reference clients over real localhost websockets.

#![allow(dead_code, unreachable_pub, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bus::{
    BlockPublisher, BlockSigner, BusEvent, KeyRing, ProofStore, RingSender, keys::parse_key,
};
use gateway::demo::{DemoConfig, DemoFeed};
use gateway::{Gateway, GatewayConfig, GatewayHandle, RelayConfig, Signers, UpstreamStatus};
use mesh_client::{Client, ClientConfig, ClientEvent, ClientStats, L4Book, SnapshotReason};
use mesh_types::{EthAddress, InstrumentId};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub const KEY_A: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
pub const KEY_B: &str = "1111111111111111111111111111111111111111111111111111111111111111";

pub fn signer() -> Arc<BlockSigner> {
    Arc::new(BlockSigner::new(KeyRing::single("a", parse_key(KEY_A).unwrap())).unwrap())
}

pub async fn serve(handle: &GatewayHandle) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = handle.clone();
    tokio::spawn(async move { h.serve(listener).await });
    addr
}

pub fn ws_url(addr: SocketAddr) -> String {
    format!("ws://{addr}/v0/ws")
}

pub struct Origin {
    pub handle: GatewayHandle,
    pub addr: SocketAddr,
    pub ring: RingSender<BusEvent>,
    pub publisher: BlockPublisher,
    pub feed: DemoFeed,
    pub signer: Arc<BlockSigner>,
    pub proofs: Arc<ProofStore>,
}

pub async fn origin(config: GatewayConfig, demo: DemoConfig) -> Origin {
    let signer = signer();
    let proofs = Arc::new(ProofStore::new(10_000));
    let ring = bus::ring(1 << 16);
    let publisher =
        BlockPublisher::new(ring.clone()).with_signer(Arc::clone(&signer), Arc::clone(&proofs));
    let feed = DemoFeed::new(demo).unwrap();
    let handle = Gateway::new(
        config,
        ring.subscribe(),
        Signers::Local(Arc::clone(&signer)),
        Arc::clone(&proofs),
    )
    .start();
    let addr = serve(&handle).await;
    Origin {
        handle,
        addr,
        ring,
        publisher,
        feed,
        signer,
        proofs,
    }
}

pub struct Relay {
    pub handle: GatewayHandle,
    pub addr: SocketAddr,
    pub status: Arc<UpstreamStatus>,
}

pub async fn relay(upstream: SocketAddr, allowlist: Vec<EthAddress>) -> Relay {
    let proofs = Arc::new(ProofStore::new(10_000));
    let ring = bus::ring(1 << 16);
    let status = Arc::new(UpstreamStatus::default());
    let gw = Gateway::new(
        GatewayConfig::default(),
        ring.subscribe(),
        Signers::Static(allowlist.clone()),
        Arc::clone(&proofs),
    )
    .with_upstream(Arc::clone(&status));
    let metrics = gw.metrics();
    let handle = gw.start();
    let addr = serve(&handle).await;
    let cfg = RelayConfig::new(vec![ws_url(upstream)], allowlist).unwrap();
    tokio::spawn(gateway::relay::run(
        cfg,
        ring,
        proofs,
        Arc::clone(&status),
        metrics,
    ));
    Relay {
        handle,
        addr,
        status,
    }
}

/// An address that accepts TCP connections and drops them at once: a
/// gateway that is down, and whose port no other test can take.
pub async fn dead_addr() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            drop(s);
        }
    });
    addr
}

/// Waits until `cond` holds, polling.
pub async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// What a client task saw.
#[derive(Debug)]
pub struct Seen {
    pub client: Client,
    pub stats: ClientStats,
    pub snapshot_reasons: Vec<SnapshotReason>,
    pub gaps: usize,
    pub failures: Vec<String>,
    pub connected_urls: Vec<String>,
    pub last_block: u64,
}

impl Seen {
    pub fn book(&self, i: &InstrumentId) -> Option<&L4Book> {
        self.client.book(i)
    }
}

/// Runs a client until it has seen the heartbeat of `until_block`, and
/// signals `ready` once subscribed.
pub fn run_client(
    cfg: ClientConfig,
    until_block: Arc<std::sync::atomic::AtomicU64>,
    ready: tokio::sync::oneshot::Sender<()>,
) -> JoinHandle<Seen> {
    tokio::spawn(async move {
        let mut client = Client::new(cfg);
        let mut ready = Some(ready);
        let mut seen = Seen {
            stats: ClientStats::default(),
            snapshot_reasons: Vec::new(),
            gaps: 0,
            failures: Vec::new(),
            connected_urls: Vec::new(),
            last_block: 0,
            client: Client::new(ClientConfig::default()),
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let Ok(ev) = tokio::time::timeout_at(deadline, client.next_event()).await else {
                panic!(
                    "client did not reach block {} in time: last block {}, urls {:?}, gaps {}, snapshots {:?}, failures {:?}, {:?}",
                    until_block.load(std::sync::atomic::Ordering::SeqCst),
                    seen.last_block,
                    seen.connected_urls,
                    seen.gaps,
                    seen.snapshot_reasons,
                    seen.failures,
                    client.stats()
                );
            };
            let ev = ev.expect("client error");
            match ev {
                ClientEvent::Connected { url, .. } => seen.connected_urls.push(url),
                ClientEvent::Subscribed { .. } => {
                    if let Some(r) = ready.take() {
                        let _ = r.send(());
                    }
                }
                ClientEvent::Snapshot { reason, .. } => seen.snapshot_reasons.push(reason),
                ClientEvent::Gap { .. } => seen.gaps += 1,
                ClientEvent::VerificationFailed { detail } => seen.failures.push(detail),
                ClientEvent::Block { block_number, .. } => {
                    seen.last_block = block_number;
                    let until = until_block.load(std::sync::atomic::Ordering::SeqCst);
                    if block_number >= until {
                        seen.stats = client.stats();
                        seen.client = client;
                        return seen;
                    }
                }
                _ => {}
            }
        }
    })
}

/// Publishes `n` blocks, `interval` apart. Returns the last block number.
pub async fn publish_blocks(o: &mut Origin, n: u64, interval: Duration) -> u64 {
    let mut last = 0;
    for _ in 0..n {
        last = o.feed.publish_next(&mut o.publisher).unwrap().block_number;
        tokio::time::sleep(interval).await;
    }
    last
}
