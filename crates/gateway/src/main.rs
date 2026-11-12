//! `gateway`: the public websocket gateway, in demo or relay mode.
//!
//! ```text
//! # Synthetic data, signed with a key from a file:
//! gateway demo --listen 127.0.0.1:8080 --signing-key k1=file:/run/secrets/block-key
//!
//! # Relay: re-serve an upstream gateway, verifying every commitment:
//! gateway relay --listen 0.0.0.0:8080 --upstream wss://a.example/v0/ws \
//!     --upstream wss://b.example/v0/ws --signer 0x...
//! ```
//!
//! In production the gateway runs in the same process as the node reader
//! and block signer (library use: `bus::BlockPublisher` feeding
//! `gateway::Gateway`); see `docs/operations/gateway.md`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use bus::{BlockPublisher, BlockSigner, KeyRing, KeySpec, ProofStore, SideTap};
use clap::{Args, Parser, Subcommand};
use gateway::demo::{DemoConfig, DemoFeed};
use gateway::{Gateway, GatewayConfig, RelayConfig, Signers, UpstreamStatus};
use mesh_client::{Encoding, Limits};
use mesh_types::{EthAddress, Registry};

#[derive(Debug, Parser)]
#[command(version, about = "bleh.xyz public websocket gateway")]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Debug, Subcommand)]
enum Mode {
    /// Serve the synthetic hl-synth feed on a loop, signed locally.
    Demo {
        #[command(flatten)]
        common: Common,
        /// Signing key `ID=file:PATH` or `ID=env:VAR`; repeat for rotation.
        #[arg(long = "signing-key")]
        signing_keys: Vec<KeySpec>,
        /// ID of the key that signs (default: the last one).
        #[arg(long)]
        active_key: Option<String>,
        /// Use a random key that lives only in memory (demo only).
        #[arg(long, conflicts_with = "signing_keys")]
        ephemeral_key: bool,
        /// hl-synth seed.
        #[arg(long, default_value_t = hl_synth::DEFAULT_SEED)]
        seed: u64,
        /// hl-synth blocks per cycle (at least 45).
        #[arg(long, default_value_t = hl_synth::DEFAULT_BLOCKS)]
        blocks: u64,
        /// Milliseconds between blocks.
        #[arg(long, default_value_t = 100)]
        interval_ms: u64,
        /// Book checkpoint every N blocks.
        #[arg(long, default_value_t = 10)]
        checkpoint_every: u64,
        /// Withhold one book batch every N blocks (exercise re-snapshots).
        #[arg(long)]
        gap_every: Option<u64>,
        /// NATS URL for the JetStream side tap (replayed on start).
        #[arg(long)]
        nats_url: Option<String>,
        /// JetStream stream name.
        #[arg(long, default_value = "BLEH_V0")]
        nats_stream: String,
        /// Side tap replay window, seconds.
        #[arg(long, default_value_t = 600)]
        replay_window_secs: u64,
        /// NATS credentials file.
        #[arg(long)]
        nats_credentials: Option<PathBuf>,
    },
    /// Re-serve an upstream gateway after verifying every commitment.
    Relay {
        #[command(flatten)]
        common: Common,
        /// Upstream websocket URL; repeat for fallbacks.
        #[arg(long = "upstream", required = true)]
        upstreams: Vec<String>,
        /// Accepted signer address; repeatable. Required.
        #[arg(long = "signer", required = true)]
        signers: Vec<EthAddress>,
        /// Seed books from upstream snapshots, which are not signed.
        #[arg(long)]
        trust_upstream_snapshots: bool,
        /// Upstream encoding: json or cbor.
        #[arg(long, default_value = "json")]
        upstream_encoding: Encoding,
    },
}

#[derive(Debug, Args)]
struct Common {
    /// Listen address.
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    /// Registry JSON (`{"instruments": [...]}`); unknown exact instruments
    /// are refused.
    #[arg(long)]
    registry: Option<PathBuf>,
    /// Blocks of proofs to keep per stream.
    #[arg(long, default_value_t = 100_000)]
    proof_retention_blocks: u64,
    /// Connections per client IP.
    #[arg(long, default_value_t = 10)]
    connections_per_ip: u32,
    /// Channels per connection.
    #[arg(long, default_value_t = 200)]
    channels_per_connection: u32,
    /// Channels per client IP.
    #[arg(long, default_value_t = 2000)]
    channels_per_ip: u32,
    /// Client messages per second.
    #[arg(long, default_value_t = 20)]
    messages_per_second: u32,
    /// Client message burst.
    #[arg(long, default_value_t = 50)]
    message_burst: u32,
    /// Proof requests per second.
    #[arg(long, default_value_t = 50)]
    proofs_per_second: u32,
    /// Send buffer, bytes.
    #[arg(long, default_value_t = 8 * 1024 * 1024)]
    send_buffer_bytes: u64,
    /// Send buffer, milliseconds behind the newest block.
    #[arg(long, default_value_t = 2000)]
    send_buffer_ms: u64,
    /// Largest client message, bytes.
    #[arg(long, default_value_t = 64 * 1024)]
    max_message_bytes: u64,
    /// Take client IPs from X-Forwarded-For (only behind a trusted proxy).
    #[arg(long)]
    trust_forwarded_for: bool,
}

impl Common {
    fn config(&self) -> anyhow::Result<GatewayConfig> {
        let registry = match &self.registry {
            Some(p) => Some(
                serde_json::from_slice::<Registry>(
                    &std::fs::read(p).with_context(|| format!("read {}", p.display()))?,
                )
                .with_context(|| format!("parse {}", p.display()))?,
            ),
            None => None,
        };
        Ok(GatewayConfig {
            limits: Limits {
                connections_per_ip: self.connections_per_ip,
                channels_per_connection: self.channels_per_connection,
                messages_per_second: self.messages_per_second,
                message_burst: self.message_burst,
                resnapshot_interval_ms: 5_000,
                proofs_per_second: self.proofs_per_second,
                send_buffer_bytes: self.send_buffer_bytes,
                send_buffer_ms: self.send_buffer_ms,
                max_message_bytes: self.max_message_bytes,
            },
            channels_per_ip: self.channels_per_ip,
            trust_forwarded_for: self.trust_forwarded_for,
            registry,
            ..GatewayConfig::default()
        })
    }
}

fn ephemeral_key() -> anyhow::Result<mesh_types::SigningKey> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").context("open /dev/urandom")?;
    for _ in 0..8 {
        let mut b = zeroize::Zeroizing::new([0u8; 32]);
        f.read_exact(&mut b[..]).context("read /dev/urandom")?;
        if let Ok(k) = mesh_types::SigningKey::from_bytes(&b) {
            return Ok(k);
        }
    }
    bail!("could not draw a valid key")
}

async fn shutdown_signal(handle: gateway::GatewayHandle) {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
    handle.shutdown();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    match cli.mode {
        Mode::Demo {
            common,
            signing_keys,
            active_key,
            ephemeral_key: ephemeral,
            seed,
            blocks,
            interval_ms,
            checkpoint_every,
            gap_every,
            nats_url,
            nats_stream,
            replay_window_secs,
            nats_credentials,
        } => {
            let ring_keys = if ephemeral || signing_keys.is_empty() {
                if !ephemeral {
                    bail!("demo mode needs --signing-key ID=file:PATH (or --ephemeral-key)");
                }
                tracing::warn!("using an ephemeral in-memory signing key (demo only)");
                KeyRing::single("ephemeral", ephemeral_key()?)
            } else {
                KeyRing::load(&signing_keys, active_key.as_deref())?
            };
            let signer = Arc::new(BlockSigner::new(ring_keys)?);
            tracing::info!(signers = ?signer.signers(), "block signer ready");
            let proofs = Arc::new(ProofStore::new(common.proof_retention_blocks));
            let ring = bus::ring(bus::DEFAULT_RING_CAPACITY);
            let mut publisher = BlockPublisher::new(ring.clone())
                .with_signer(Arc::clone(&signer), Arc::clone(&proofs));
            let mut replay = Vec::new();
            if let Some(url) = nats_url {
                let tap = bus::jetstream::JetStreamTap::connect(bus::jetstream::JetStreamConfig {
                    url,
                    stream: nats_stream,
                    replay_window: Duration::from_secs(replay_window_secs),
                    credentials: nats_credentials,
                    ..Default::default()
                })
                .await?;
                replay = tap.replay().await?;
                publisher = publisher.with_tap(Arc::new(tap));
            }
            let mut feed = DemoFeed::new(DemoConfig {
                seed,
                blocks,
                block_interval: Duration::from_millis(interval_ms),
                checkpoint_every,
                gap_every,
            })?;
            let mut cfg = common.config()?;
            if cfg.registry.is_none() {
                cfg.registry = Some(feed.registry().clone());
            }
            // Continue numbering after anything replayed.
            if let Some(last) = replay.iter().map(bus::BusEvent::block_number).max() {
                while feed.next_block_number() <= last {
                    let _ = feed.next_block();
                }
            }
            let ring_for_stats = ring.clone();
            let pstats = publisher.stats();
            let gw = Gateway::new(cfg, ring.subscribe(), Signers::Local(signer), proofs)
                .with_replay(replay)
                .with_extra_metrics(Box::new(move || {
                    use std::sync::atomic::Ordering::Relaxed;
                    let r = ring_for_stats.stats();
                    vec![
                        (
                            "bus_ring_published_total".into(),
                            "Items published on the bus ring".into(),
                            r.published,
                        ),
                        (
                            "bus_ring_lag_events_total".into(),
                            "Times a bus receiver lagged".into(),
                            r.lag_events,
                        ),
                        (
                            "signer_commitments_total".into(),
                            "Commitments signed".into(),
                            pstats.commitments.load(Relaxed),
                        ),
                        (
                            "signer_sign_nanoseconds_total".into(),
                            "Time spent building roots and signing".into(),
                            pstats.sign_nanos.load(Relaxed),
                        ),
                        (
                            "bus_records_published_total".into(),
                            "Records published".into(),
                            pstats.records.load(Relaxed),
                        ),
                    ]
                }));
            drop(ring);
            let handle = gw.start();
            let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
            let demo = tokio::spawn(gateway::demo::run(feed, publisher, stop_rx));
            let listener = tokio::net::TcpListener::bind(common.listen).await?;
            tracing::info!(addr = %common.listen, "gateway (demo) listening on /v0/ws");
            tokio::spawn(shutdown_signal(handle.clone()));
            handle.serve(listener).await?;
            let _ = stop_tx.send(true);
            demo.await??;
        }
        Mode::Relay {
            common,
            upstreams,
            signers,
            trust_upstream_snapshots,
            upstream_encoding,
        } => {
            let proofs = Arc::new(ProofStore::new(common.proof_retention_blocks));
            let ring = bus::ring(bus::DEFAULT_RING_CAPACITY);
            let status = Arc::new(UpstreamStatus::default());
            let gw = Gateway::new(
                common.config()?,
                ring.subscribe(),
                Signers::Static(signers.clone()),
                Arc::clone(&proofs),
            )
            .with_upstream(Arc::clone(&status));
            let metrics = gw.metrics();
            let handle = gw.start();
            let mut rcfg = RelayConfig::new(upstreams, signers);
            rcfg.trust_upstream_snapshots = trust_upstream_snapshots;
            rcfg.encoding = upstream_encoding;
            tokio::spawn(gateway::relay::run(rcfg, ring, proofs, status, metrics));
            let listener = tokio::net::TcpListener::bind(common.listen).await?;
            tracing::info!(addr = %common.listen, "gateway (relay) listening on /v0/ws");
            tokio::spawn(shutdown_signal(handle.clone()));
            handle.serve(listener).await?;
        }
    }
    Ok(())
}
