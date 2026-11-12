//! `mesh-client`: connect to one or more gateways, keep books, verify, and
//! print or record the stream as JSON lines.
//!
//! ```text
//! mesh-client --url ws://127.0.0.1:8080/v0/ws --sub '*|book_event' --sub '*|trade'
//! mesh-client --url wss://a/v0/ws --url wss://b/v0/ws --sub 'hyperliquid:perp:BTC|book_event' \
//!     --signer 0x... --proof-every 100 --record stream.jsonl --duration 60
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use mesh_client::{ChannelSpec, Client, ClientConfig, ClientEvent, Encoding};
use mesh_types::{EthAddress, UnixNanos};

#[derive(Debug, Parser)]
#[command(version, about = "Reference client for the bleh.xyz gateway")]
struct Args {
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

fn is_data(e: &ClientEvent) -> bool {
    matches!(
        e,
        ClientEvent::Snapshot { .. }
            | ClientEvent::Batch { .. }
            | ClientEvent::Record { .. }
            | ClientEvent::Commitment { .. }
            | ClientEvent::Block { .. }
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
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
                    books = client.books().count(),
                    batches = s.batches,
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
    Ok(())
}
