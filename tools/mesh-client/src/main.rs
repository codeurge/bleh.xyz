//! `mesh-client`: connect to one or more gateways, keep books, verify, and
//! print or record the stream as JSON lines; probe a gateway; derive a
//! signing key's address.
//!
//! ```text
//! mesh-client --url ws://127.0.0.1:8080/v0/ws --sub '*|book_event' --sub '*|trade'
//! mesh-client --url wss://a/v0/ws --url wss://b/v0/ws --sub 'hyperliquid:perp:BTC|book_event' \
//!     --signer 0x... --proof-every 100 --record stream.jsonl --duration 60
//! mesh-client probe --url wss://a/v0/ws --instrument hyperliquid:perp:BTC --kind book_event --timeout 5s
//! mesh-client keys address --key-file /run/secrets/block-key
//! ```
//!
//! Streaming is the default (no subcommand) and is also `mesh-client stream`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use mesh_client::probe::{self, ProbeConfig};
use mesh_client::{
    ChannelSpec, Client, ClientConfig, ClientEvent, Depth, Encoding, InstrumentPattern,
};
use mesh_types::{BarInterval, EthAddress, RecordKind, UnixNanos};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Reference client for the bleh.xyz gateway",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Streaming options (the default command).
    #[command(flatten)]
    stream: StreamArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect, subscribe, keep books, verify (the default without a
    /// subcommand).
    Stream(StreamArgs),
    /// Check that a gateway serves a channel: print one JSON line
    /// `{"ok":bool,"first_block_seconds":f,"block_time_ms":n}` and exit 0
    /// when ok, 1 otherwise.
    Probe(ProbeArgs),
    /// Signing-key utilities.
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
}

#[derive(Debug, Subcommand)]
enum KeysCommand {
    /// Print the Ethereum address of a secp256k1 secret key file (64 hex
    /// characters, optional 0x). The key itself is never printed.
    Address {
        /// Key file.
        #[arg(long)]
        key_file: PathBuf,
    },
}

fn parse_kind(s: &str) -> Result<RecordKind, String> {
    serde_json::from_value(serde_json::Value::String(s.into()))
        .map_err(|_| format!("unknown kind {s:?}"))
}

fn parse_depth(s: &str) -> Result<Depth, String> {
    match s {
        "l4" => Ok(Depth::L4),
        "l2" => Ok(Depth::L2),
        other => Err(format!("unknown depth {other:?} (l4 or l2)")),
    }
}

fn parse_interval(s: &str) -> Result<BarInterval, String> {
    serde_json::from_value(serde_json::Value::String(s.into()))
        .map_err(|_| format!("unknown bar interval {s:?}"))
}

#[derive(Debug, Args)]
struct ProbeArgs {
    /// Gateway websocket URL.
    #[arg(long)]
    url: String,
    /// Instrument ID (or pattern: `*`, `prefix*`).
    #[arg(long)]
    instrument: InstrumentPattern,
    /// Record kind, e.g. trade or book_event.
    #[arg(long, value_parser = parse_kind)]
    kind: RecordKind,
    /// book_event only: l4 or l2.
    #[arg(long, value_parser = parse_depth)]
    depth: Option<Depth>,
    /// bar only: bar interval, e.g. 1m.
    #[arg(long, value_parser = parse_interval)]
    interval: Option<BarInterval>,
    /// Signer the gateway must announce in hello; repeatable.
    #[arg(long = "signer")]
    signers: Vec<EthAddress>,
    /// Give up after this long (5s, 500ms, 2m), connection included.
    #[arg(long, default_value = "5s", value_parser = probe::parse_duration)]
    timeout: Duration,
    /// Frame encoding: json or cbor.
    #[arg(long, default_value = "json")]
    encoding: Encoding,
}

#[derive(Debug, Args)]
struct StreamArgs {
    /// Gateway websocket URL; repeat for fallbacks (rotated on failure).
    #[arg(long = "url", required = true)]
    urls: Vec<String>,
    /// Channel `INSTRUMENT|KIND[|l4|l2|INTERVAL]`; repeatable.
    #[arg(long = "sub", required = true)]
    subs: Vec<ChannelSpec>,
    /// Frame encoding: json or cbor.
    #[arg(long, default_value = "json")]
    encoding: Encoding,
    /// Do not ask for commitments.
    #[arg(long)]
    no_commitments: bool,
    /// Trusted signer address; repeatable. Without any, the gateway's
    /// announced signers are trusted.
    #[arg(long = "signer")]
    signers: Vec<EthAddress>,
    /// Ask for an inclusion proof every N records (0: never).
    #[arg(long, default_value_t = 0)]
    proof_every: u64,
    /// Write events as JSON lines to this file ("-" for stdout).
    #[arg(long)]
    record: Option<PathBuf>,
    /// Which events to record: all, or data (snapshots, batches, records,
    /// commitments, blocks).
    #[arg(long, default_value = "all")]
    record_events: String,
    /// Stop after this many seconds.
    #[arg(long)]
    duration: Option<u64>,
    /// Stall timeout in milliseconds (no block heartbeat).
    #[arg(long, default_value_t = 2000)]
    stall_ms: u64,
    /// Give up after this many consecutive connection failures.
    #[arg(long)]
    max_failures: Option<u32>,
    /// Print a summary line to stderr every N blocks (0: never).
    #[arg(long, default_value_t = 50)]
    summary_every: u64,
}

fn print_json(v: &impl serde::Serialize) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, v)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn is_data(e: &ClientEvent) -> bool {
    matches!(
        e,
        ClientEvent::Snapshot { .. }
            | ClientEvent::Batch { .. }
            | ClientEvent::Levels { .. }
            | ClientEvent::Record { .. }
            | ClientEvent::Commitment { .. }
            | ClientEvent::Block { .. }
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    let default_level = match cli.command {
        Some(Command::Probe(_) | Command::Keys { .. }) => "warn",
        _ => "info",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level)),
        )
        .with_writer(std::io::stderr)
        .init();
    match cli.command {
        None => stream(cli.stream).await,
        Some(Command::Stream(args)) => stream(args).await,
        Some(Command::Probe(args)) => {
            let res = probe::run(ProbeConfig {
                url: args.url,
                channel: ChannelSpec {
                    instrument: args.instrument,
                    kind: args.kind,
                    depth: args.depth,
                    interval: args.interval,
                },
                signers: args.signers,
                timeout: args.timeout,
                encoding: args.encoding,
            })
            .await;
            print_json(&res)?;
            Ok(if res.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Some(Command::Keys {
            command: KeysCommand::Address { key_file },
        }) => match mesh_client::keys::read_key_file(&key_file) {
            Ok(k) => {
                if let Some(w) = &k.permission_warning {
                    eprintln!("warning: {w}");
                }
                println!("{}", k.address());
                Ok(ExitCode::SUCCESS)
            }
            Err(e) => {
                eprintln!("error: {e}");
                Ok(ExitCode::FAILURE)
            }
        },
    }
}

async fn stream(args: StreamArgs) -> anyhow::Result<ExitCode> {
    let mut out: Option<Box<dyn Write>> = match &args.record {
        None => None,
        Some(p) if p.as_os_str() == "-" => Some(Box::new(std::io::stdout())),
        Some(p) => Some(Box::new(std::io::BufWriter::new(
            std::fs::File::create(p).with_context(|| format!("create {}", p.display()))?,
        ))),
    };
    let data_only = args.record_events == "data";
    let mut client = Client::new(ClientConfig {
        urls: args.urls.clone(),
        encoding: args.encoding,
        channels: args.subs.clone(),
        commitments: !args.no_commitments,
        signers: args.signers.clone(),
        proof_every: args.proof_every,
        stall_timeout: Duration::from_millis(args.stall_ms),
        max_connect_failures: args.max_failures,
        ..ClientConfig::default()
    });
    let deadline = args
        .duration
        .map(|s| Instant::now() + Duration::from_secs(s));
    let mut latencies_us: Vec<i64> = Vec::new();
    loop {
        let next = match deadline {
            Some(d) => match tokio::time::timeout_at(d.into(), client.next_event()).await {
                Ok(e) => e,
                Err(_) => break,
            },
            None => client.next_event().await,
        };
        let event = next?;
        match &event {
            ClientEvent::Connected { url, hello } => {
                tracing::info!(%url, signers = ?hello.signers, "connected");
            }
            ClientEvent::ConnectFailed { url, error } => {
                tracing::warn!(%url, %error, "connect failed");
            }
            ClientEvent::Disconnected { url, reason } => {
                tracing::warn!(%url, %reason, "disconnected");
            }
            ClientEvent::Gap {
                instrument,
                kind,
                detail,
                ..
            } => {
                tracing::warn!(%instrument, %kind, %detail, "gap");
            }
            ClientEvent::VerificationFailed { detail } => {
                tracing::error!(%detail, "verification failed");
            }
            ClientEvent::ServerError { code, message, .. } => {
                tracing::warn!(?code, %message, "server error");
            }
            ClientEvent::Batch { batch } => {
                let now = UnixNanos::now().0;
                if let Some(p) = batch.records.first().and_then(|r| r.prov().ts_publish_ns) {
                    latencies_us.push((now - p.0) / 1000);
                }
            }
            ClientEvent::Levels { levels } => {
                if let Some(p) = levels.ts_publish_ns {
                    latencies_us.push((UnixNanos::now().0 - p.0) / 1000);
                }
            }
            ClientEvent::Block { block_number, .. }
                if args.summary_every > 0 && block_number.is_multiple_of(args.summary_every) =>
            {
                let s = client.stats();
                latencies_us.sort_unstable();
                let pct = |q: usize| {
                    latencies_us
                        .get(latencies_us.len().saturating_sub(1) * q / 100)
                        .copied()
                        .unwrap_or(0)
                };
                tracing::info!(
                    block = block_number,
                    books = client.books().count() + client.l2_books().count(),
                    batches = s.batches,
                    levels = s.levels,
                    gaps = s.gaps,
                    signatures_ok = s.signatures_ok,
                    roots_ok = s.roots_ok,
                    proofs_ok = s.proofs_ok,
                    failures = s.signatures_failed + s.roots_failed + s.proofs_failed,
                    publish_to_receive_p50_us = pct(50),
                    publish_to_receive_p99_us = pct(99),
                    "summary"
                );
                latencies_us.clear();
            }
            _ => {}
        }
        if let Some(w) = out.as_mut()
            && (!data_only || is_data(&event))
        {
            serde_json::to_writer(&mut *w, &event)?;
            w.write_all(b"\n")?;
        }
    }
    if let Some(w) = out.as_mut() {
        w.flush()?;
    }
    client.close().await;
    let s = client.stats();
    eprintln!("{}", serde_json::to_string(&s)?);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("mesh-client").chain(args.iter().copied()))
    }

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn streaming_without_a_subcommand_still_works() {
        let cli = parse(&["--url", "ws://a/v0/ws", "--sub", "*|trade"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.stream.urls, vec!["ws://a/v0/ws"]);
        assert!(
            parse(&["--sub", "*|trade"]).is_err(),
            "--url stays required"
        );
        let cli = parse(&["stream", "--url", "ws://a/v0/ws", "--sub", "*|trade"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Stream(_))));
    }

    #[test]
    fn probe_and_keys_parse() {
        let cli = parse(&[
            "probe",
            "--url",
            "ws://a/v0/ws",
            "--instrument",
            "hyperliquid:perp:BTC",
            "--kind",
            "book_event",
            "--depth",
            "l2",
            "--timeout",
            "750ms",
        ])
        .unwrap();
        let Some(Command::Probe(p)) = cli.command else {
            panic!("not probe")
        };
        assert_eq!(
            (p.kind, p.depth, p.timeout),
            (
                RecordKind::BookEvent,
                Some(Depth::L2),
                Duration::from_millis(750)
            )
        );
        let cli = parse(&[
            "probe",
            "--url",
            "ws://a",
            "--instrument",
            "*",
            "--kind",
            "bar",
            "--interval",
            "1m",
        ])
        .unwrap();
        let Some(Command::Probe(p)) = cli.command else {
            panic!("not probe")
        };
        assert_eq!(
            (p.interval, p.timeout),
            (Some(BarInterval::M1), Duration::from_secs(5))
        );
        assert!(
            parse(&[
                "probe",
                "--url",
                "ws://a",
                "--instrument",
                "*",
                "--kind",
                "nope"
            ])
            .is_err()
        );
        let cli = parse(&["keys", "address", "--key-file", "/k"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Keys {
                command: KeysCommand::Address { .. }
            })
        ));
    }
}
