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
//! Every flag can also be set with an environment variable named
//! `BLEH_GATEWAY_<FLAG>` (`--send-buffer-ms` is `BLEH_GATEWAY_SEND_BUFFER_MS`;
//! repeatable flags take a comma-separated list). `BLEH_GATEWAY_MODE`
//! (`demo` or `relay`) supplies the mode when no argument is given, so a
//! service can be configured from the environment alone. Flags win over
//! the environment.
//!
//! In production the gateway runs in the same process as the node reader
//! and block signer (library use: `bus::BlockPublisher` feeding
//! `gateway::Gateway`); see `docs/operations/gateway.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::ffi::OsString;
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

/// Environment variable that supplies the mode when no argument is given.
const MODE_ENV: &str = "BLEH_GATEWAY_MODE";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "bleh.xyz public websocket gateway",
    after_help = "Every flag can also be set as BLEH_GATEWAY_<FLAG> (comma-separated for repeatable flags); BLEH_GATEWAY_MODE=demo|relay supplies the mode when no argument is given."
)]
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
        #[arg(
            long = "signing-key",
            env = "BLEH_GATEWAY_SIGNING_KEY",
            value_delimiter = ','
        )]
        signing_keys: Vec<KeySpec>,
        /// ID of the key that signs (default: the last one).
        #[arg(long, env = "BLEH_GATEWAY_ACTIVE_KEY")]
        active_key: Option<String>,
        /// Use a random key that lives only in memory (demo only).
        #[arg(
            long,
            conflicts_with = "signing_keys",
            env = "BLEH_GATEWAY_EPHEMERAL_KEY"
        )]
        ephemeral_key: bool,
        /// hl-synth seed.
        #[arg(long, default_value_t = hl_synth::DEFAULT_SEED, env = "BLEH_GATEWAY_SEED")]
        seed: u64,
        /// hl-synth blocks per cycle (at least 45).
        #[arg(long, default_value_t = hl_synth::DEFAULT_BLOCKS, env = "BLEH_GATEWAY_BLOCKS")]
        blocks: u64,
        /// Milliseconds between blocks.
        #[arg(long, default_value_t = 100, env = "BLEH_GATEWAY_INTERVAL_MS")]
        interval_ms: u64,
        /// Book checkpoint every N blocks.
        #[arg(long, default_value_t = 10, env = "BLEH_GATEWAY_CHECKPOINT_EVERY")]
        checkpoint_every: u64,
        /// Withhold one book batch every N blocks (exercise re-snapshots).
        #[arg(long, env = "BLEH_GATEWAY_GAP_EVERY")]
        gap_every: Option<u64>,
        /// NATS URL for the JetStream side tap (replayed on start).
        #[arg(long, env = "BLEH_GATEWAY_NATS_URL")]
        nats_url: Option<String>,
        /// JetStream stream name.
        #[arg(long, default_value = "BLEH_V0", env = "BLEH_GATEWAY_NATS_STREAM")]
        nats_stream: String,
        /// Side tap replay window, seconds.
        #[arg(long, default_value_t = 600, env = "BLEH_GATEWAY_REPLAY_WINDOW_SECS")]
        replay_window_secs: u64,
        /// NATS credentials file.
        #[arg(long, env = "BLEH_GATEWAY_NATS_CREDENTIALS")]
        nats_credentials: Option<PathBuf>,
    },
    /// Re-serve an upstream gateway after verifying every commitment.
    Relay {
        #[command(flatten)]
        common: Common,
        /// Upstream websocket URL; repeat for fallbacks.
        #[arg(
            long = "upstream",
            required = true,
            env = "BLEH_GATEWAY_UPSTREAM",
            value_delimiter = ','
        )]
        upstreams: Vec<String>,
        /// Accepted signer address; repeatable. Required.
        #[arg(
            long = "signer",
            required = true,
            env = "BLEH_GATEWAY_SIGNER",
            value_delimiter = ','
        )]
        signers: Vec<EthAddress>,
        /// Seed books from upstream snapshots, which are not signed.
        #[arg(long, env = "BLEH_GATEWAY_TRUST_UPSTREAM_SNAPSHOTS")]
        trust_upstream_snapshots: bool,
        /// Upstream encoding: json or cbor.
        #[arg(long, default_value = "json", env = "BLEH_GATEWAY_UPSTREAM_ENCODING")]
        upstream_encoding: Encoding,
    },
}

#[derive(Debug, Args)]
struct Common {
    /// Listen address.
    #[arg(long, default_value = "127.0.0.1:8080", env = "BLEH_GATEWAY_LISTEN")]
    listen: SocketAddr,
    /// Registry JSON (`{"instruments": [...]}`); unknown exact instruments
    /// are refused.
    #[arg(long, env = "BLEH_GATEWAY_REGISTRY")]
    registry: Option<PathBuf>,
    /// Blocks of proofs to keep per stream.
    #[arg(
        long,
        default_value_t = 100_000,
        env = "BLEH_GATEWAY_PROOF_RETENTION_BLOCKS"
    )]
    proof_retention_blocks: u64,
    /// Connections per client IP.
    #[arg(long, default_value_t = 10, env = "BLEH_GATEWAY_CONNECTIONS_PER_IP")]
    connections_per_ip: u32,
    /// Channels per connection.
    #[arg(
        long,
        default_value_t = 200,
        env = "BLEH_GATEWAY_CHANNELS_PER_CONNECTION"
    )]
    channels_per_connection: u32,
    /// Channels per client IP.
    #[arg(long, default_value_t = 2000, env = "BLEH_GATEWAY_CHANNELS_PER_IP")]
    channels_per_ip: u32,
    /// Client messages per second.
    #[arg(long, default_value_t = 20, env = "BLEH_GATEWAY_MESSAGES_PER_SECOND")]
    messages_per_second: u32,
    /// Client message burst.
    #[arg(long, default_value_t = 50, env = "BLEH_GATEWAY_MESSAGE_BURST")]
    message_burst: u32,
    /// Proof requests per second.
    #[arg(long, default_value_t = 50, env = "BLEH_GATEWAY_PROOFS_PER_SECOND")]
    proofs_per_second: u32,
    /// Send buffer, bytes.
    #[arg(long, default_value_t = 8 * 1024 * 1024, env = "BLEH_GATEWAY_SEND_BUFFER_BYTES")]
    send_buffer_bytes: u64,
    /// Send buffer, milliseconds behind the newest block.
    #[arg(long, default_value_t = 2000, env = "BLEH_GATEWAY_SEND_BUFFER_MS")]
    send_buffer_ms: u64,
    /// Largest client message, bytes.
    #[arg(long, default_value_t = 64 * 1024, env = "BLEH_GATEWAY_MAX_MESSAGE_BYTES")]
    max_message_bytes: u64,
    /// Take client IPs from X-Forwarded-For (only behind a trusted proxy).
    #[arg(long, env = "BLEH_GATEWAY_TRUST_FORWARDED_FOR")]
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

/// The command line, with the mode taken from [`MODE_ENV`] when no
/// argument was given.
fn command_line(args: impl IntoIterator<Item = OsString>, mode: Option<OsString>) -> Vec<OsString> {
    let mut args: Vec<OsString> = args.into_iter().collect();
    if args.len() <= 1
        && let Some(m) = mode.filter(|m| !m.is_empty())
    {
        args.push(m);
    }
    args
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
    let cli = Cli::parse_from(command_line(
        std::env::args_os(),
        std::env::var_os(MODE_ENV),
    ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn every_flag_has_an_environment_variable() {
        let cmd = Cli::command();
        let mut names = std::collections::HashSet::new();
        for sub in cmd.get_subcommands() {
            for arg in sub.get_arguments() {
                let id = arg.get_id().as_str();
                if matches!(id, "help" | "version") {
                    continue;
                }
                let long = arg
                    .get_long()
                    .unwrap_or_else(|| panic!("{id} has no long flag"));
                let env = arg
                    .get_env()
                    .unwrap_or_else(|| panic!("--{long} has no environment variable"))
                    .to_string_lossy()
                    .into_owned();
                let want = format!("BLEH_GATEWAY_{}", long.to_uppercase().replace('-', "_"));
                assert_eq!(env, want, "--{long}");
                names.insert((sub.get_name().to_owned(), env));
            }
        }
        assert!(names.len() > 25, "{names:?}");
    }

    #[test]
    fn mode_comes_from_the_environment_only_without_arguments() {
        let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            command_line(os(&["gateway"]), Some("relay".into())),
            os(&["gateway", "relay"])
        );
        assert_eq!(
            command_line(os(&["gateway", "demo"]), Some("relay".into())),
            os(&["gateway", "demo"])
        );
        assert_eq!(
            command_line(os(&["gateway"]), Some("".into())),
            os(&["gateway"])
        );
        assert_eq!(command_line(os(&["gateway"]), None), os(&["gateway"]));
    }

    #[test]
    fn repeatable_flags_split_on_commas() {
        let addr = "0x0000000000000000000000000000000000000001";
        let cli = Cli::try_parse_from([
            "gateway",
            "relay",
            "--upstream",
            "ws://a/v0/ws,ws://b/v0/ws",
            "--signer",
            addr,
        ])
        .unwrap();
        let Mode::Relay {
            upstreams, signers, ..
        } = cli.mode
        else {
            panic!("not relay")
        };
        assert_eq!(upstreams, vec!["ws://a/v0/ws", "ws://b/v0/ws"]);
        assert_eq!(signers.len(), 1);
    }
}
