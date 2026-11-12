//! Command line and environment of the `origin` binary.
//!
//! Every flag has an environment variable. Reader settings keep the
//! `hl-node-reader` names (`HL_*`), so the same `.env` drives both; origin
//! settings are `BLEH_*`. Secrets come from files only.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use hl_node_reader::files::StartPosition;
use hl_node_reader::filter::StreamDirs;
use hl_node_reader::merge::MergeConfig;
use mesh_types::BarInterval;

use crate::derive::{BookStart, DeriveConfig};
use crate::service::{CaptureSettings, OriginConfig, ReaderSettings};

/// Where the reader starts without a checkpoint.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StartArg {
    /// The oldest hourly file.
    Earliest,
    /// The newest hourly file, from its start.
    LatestFile,
    /// Only new blocks.
    End,
}

/// How books start.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum BookStartArg {
    /// From the first L4 snapshot (production).
    Snapshot,
    /// Empty (synthetic data, a market's first block).
    Empty,
}

/// Log format.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable.
    Text,
    /// JSON lines.
    Json,
}

/// The bleh.xyz origin: node reader, book builder, block signer, gateway
/// and live raw capture in one process.
#[derive(Debug, Parser)]
#[command(name = "origin", version, about)]
pub struct Cli {
    /// Node data directory; two (comma-separated) for active-active ingest.
    #[arg(
        long = "node-dir",
        env = "HL_NODE_DIRS",
        value_delimiter = ',',
        required = true
    )]
    pub node_dirs: Vec<PathBuf>,
    /// Registry JSON to start from.
    #[arg(long, env = "HL_REGISTRY")]
    pub registry: Option<PathBuf>,
    /// Write the registry here when markets are added.
    #[arg(long, env = "HL_REGISTRY_OUT")]
    pub registry_out: Option<PathBuf>,
    /// Node info server (`--serve-info`) for metadata and L4 snapshots.
    #[arg(long, env = "HL_INFO_URL")]
    pub info_url: Option<String>,
    /// Info request timeout, milliseconds.
    #[arg(long, env = "HL_INFO_TIMEOUT_MS", default_value_t = 10_000)]
    pub info_timeout_ms: u64,
    /// Registry refresh, seconds (0: only on unknown coins).
    #[arg(long, env = "HL_REGISTRY_REFRESH_SECS", default_value_t = 300)]
    pub registry_refresh_secs: u64,
    /// Reader checkpoint file.
    #[arg(long, env = "HL_CHECKPOINT")]
    pub checkpoint: Option<PathBuf>,
    /// Minimum seconds between checkpoint writes.
    #[arg(long, env = "HL_CHECKPOINT_INTERVAL_SECS", default_value_t = 5)]
    pub checkpoint_interval_secs: u64,
    /// Where to start without a checkpoint.
    #[arg(long, env = "HL_START", value_enum, default_value = "latest-file")]
    pub start: StartArg,
    /// Fills directory name.
    #[arg(long, env = "HL_FILLS_DIR", default_value = hl_node_reader::assumptions::DEFAULT_FILLS_DIR)]
    pub fills_dir: String,
    /// Order-status directory name.
    #[arg(long, env = "HL_ORDER_STATUSES_DIR", default_value = hl_node_reader::assumptions::DEFAULT_ORDER_STATUSES_DIR)]
    pub order_statuses_dir: String,
    /// Raw-book-diff directory name.
    #[arg(long, env = "HL_RAW_BOOK_DIFFS_DIR", default_value = hl_node_reader::assumptions::DEFAULT_RAW_BOOK_DIFFS_DIR)]
    pub raw_book_diffs_dir: String,
    /// Poll only (no filesystem notifications).
    #[arg(long, env = "HL_NO_NOTIFY")]
    pub no_notify: bool,
    /// Poll interval, milliseconds.
    #[arg(long, env = "HL_POLL_INTERVAL_MS", default_value_t = 50)]
    pub poll_interval_ms: u64,
    /// Release blocks without a stream lagging this many blocks.
    #[arg(long, env = "HL_MAX_LAG_BLOCKS", default_value_t = 600)]
    pub max_lag_blocks: u64,
    /// Two nodes: hold blocks after a hole this long.
    #[arg(long, env = "HL_GAP_WAIT_MS", default_value_t = 1_000)]
    pub gap_wait_ms: u64,
    /// Two nodes: blocks a node may lag before its copies count as missing.
    #[arg(long, env = "HL_CONFIRM_WINDOW_BLOCKS", default_value_t = 100)]
    pub confirm_window_blocks: u64,
    /// Two nodes: silence before a node is reported stalled, seconds.
    #[arg(long, env = "HL_STALL_AFTER_SECS", default_value_t = 10)]
    pub stall_after_secs: u64,
    /// L4 snapshot interval, seconds (0: never).
    #[arg(long, env = "HL_SNAPSHOT_INTERVAL_SECS", default_value_t = 0)]
    pub snapshot_interval_secs: u64,
    /// File the node writes snapshots to.
    #[arg(
        long,
        env = "HL_SNAPSHOT_PATH",
        default_value = "/tmp/hl-node-reader-l4.json"
    )]
    pub snapshot_path: PathBuf,
    /// Reader and origin metrics listener (`/metrics`, `/health`).
    #[arg(long, env = "HL_METRICS_ADDR", default_value = "127.0.0.1:9184")]
    pub metrics_addr: SocketAddr,
    /// Log format.
    #[arg(long, env = "HL_LOG_FORMAT", value_enum, default_value = "text")]
    pub log_format: LogFormat,

    /// Block signing key file (64 hex digits, optional `0x`).
    #[arg(long, env = "BLEH_SIGNING_KEY_FILE")]
    pub signing_key_file: Option<PathBuf>,
    /// ID of the key in `--signing-key-file` (shown in `/signers`).
    #[arg(long, env = "BLEH_SIGNING_KEY_ID", default_value = "origin")]
    pub signing_key_id: String,
    /// Further keys as `ID=file:PATH` (rotation). Repeatable.
    #[arg(long = "signing-key", env = "BLEH_SIGNING_KEYS", value_delimiter = ',')]
    pub signing_keys: Vec<bus::KeySpec>,
    /// Active key ID (default: the first key).
    #[arg(long, env = "BLEH_ACTIVE_KEY")]
    pub active_key: Option<String>,

    /// Gateway listen address.
    #[arg(long, env = "BLEH_GATEWAY_LISTEN", default_value = "0.0.0.0:8080")]
    pub gateway_listen: SocketAddr,
    /// Take client IPs from `X-Forwarded-For` (behind a trusted proxy only).
    #[arg(long, env = "BLEH_GATEWAY_TRUST_FORWARDED_FOR")]
    pub trust_forwarded_for: bool,
    /// Proof retention, blocks per stream.
    #[arg(
        long,
        env = "BLEH_GATEWAY_PROOF_RETENTION_BLOCKS",
        default_value_t = 100_000
    )]
    pub proof_retention_blocks: u64,
    /// `/health` fails when no block closed for this many seconds.
    #[arg(long, env = "BLEH_GATEWAY_MAX_STALENESS_SECS", default_value_t = 5)]
    pub max_staleness_secs: u64,

    /// NATS URL for the JetStream side tap (replayed into the gateway on
    /// start).
    #[arg(long, env = "BLEH_NATS_URL")]
    pub nats_url: Option<String>,
    /// JetStream stream name.
    #[arg(long, env = "BLEH_NATS_STREAM", default_value = "BLEH_V0")]
    pub nats_stream: String,
    /// JetStream replay window, seconds.
    #[arg(long, env = "BLEH_NATS_REPLAY_WINDOW_SECS", default_value_t = 600)]
    pub nats_replay_window_secs: u64,
    /// NATS credentials file.
    #[arg(long, env = "BLEH_NATS_CREDENTIALS_FILE")]
    pub nats_credentials: Option<PathBuf>,

    /// Live capture staging directory; enables capture of the first node
    /// directory into the private and filtered tiers, and the
    /// `.captured-through` watermark here.
    #[arg(long, env = "BLEH_RAW_DIR")]
    pub raw_dir: Option<PathBuf>,
    /// Do not capture although `--raw-dir` is set.
    #[arg(long, env = "BLEH_CAPTURE_DISABLED")]
    pub no_capture: bool,
    /// Skip the private (unfiltered) tier.
    #[arg(long, env = "BLEH_CAPTURE_NO_PRIVATE")]
    pub capture_no_private: bool,
    /// Upload sealed hours (stores from `BLEH_ARCHIVE_{PRIVATE,PUBLIC}_*`).
    #[arg(long, env = "BLEH_CAPTURE_UPLOAD")]
    pub capture_upload: bool,
    /// Keep uploaded files under `uploaded/`.
    #[arg(long, env = "BLEH_CAPTURE_KEEP_LOCAL")]
    pub capture_keep_local: bool,
    /// Capture metrics textfile (node exporter textfile collector).
    #[arg(long, env = "BLEH_CAPTURE_METRICS_OUT")]
    pub capture_metrics_out: Option<PathBuf>,
    /// Rewrite the capture metrics file this often, seconds.
    #[arg(long, env = "BLEH_CAPTURE_METRICS_EVERY_SECS", default_value_t = 60)]
    pub capture_metrics_every_secs: u64,
    /// Capture poll interval, milliseconds.
    #[arg(long, env = "BLEH_CAPTURE_POLL_MS", default_value_t = 1_000)]
    pub capture_poll_ms: u64,
    /// Seal an hour this long after it ended if a stream stopped, seconds.
    #[arg(long, env = "BLEH_CAPTURE_SEAL_GRACE_SECS", default_value_t = 600)]
    pub capture_seal_grace_secs: u64,

    /// How books start.
    #[arg(long, env = "BLEH_BOOK_START", value_enum, default_value = "snapshot")]
    pub book_start: BookStartArg,
    /// Book checkpoint interval, seconds of block time.
    #[arg(long, env = "BLEH_BOOK_CHECKPOINT_SECS", default_value_t = 600)]
    pub book_checkpoint_secs: u64,
    /// Coverage interval (`1s`, `1m`, `5m`, `15m`, `1h`, `1d`).
    #[arg(long, env = "BLEH_COVERAGE_INTERVAL", default_value = "1m")]
    pub coverage_interval: String,
    /// Blocks of book events kept to roll snapshots forward.
    #[arg(long, env = "BLEH_RECENT_BLOCKS", default_value_t = 600)]
    pub recent_blocks: usize,
}

fn interval(s: &str) -> Result<BarInterval> {
    BarInterval::ALL
        .into_iter()
        .find(|b| b.as_str() == s)
        .with_context(|| format!("unknown interval {s:?}"))
}

impl Cli {
    /// The service settings.
    ///
    /// # Errors
    ///
    /// Fails on inconsistent settings.
    pub fn config(&self) -> Result<OriginConfig> {
        let secs = |n: u64| (n > 0).then(|| Duration::from_secs(n));
        let reader = ReaderSettings {
            node_dirs: self.node_dirs.clone(),
            dirs: StreamDirs {
                fills: self.fills_dir.clone(),
                order_statuses: self.order_statuses_dir.clone(),
                raw_book_diffs: self.raw_book_diffs_dir.clone(),
            },
            registry: self.registry.clone(),
            registry_out: self.registry_out.clone(),
            info_url: self.info_url.clone(),
            info_timeout: Duration::from_millis(self.info_timeout_ms),
            registry_refresh: secs(self.registry_refresh_secs),
            checkpoint: self.checkpoint.clone(),
            checkpoint_interval: Duration::from_secs(self.checkpoint_interval_secs),
            start: match self.start {
                StartArg::Earliest => StartPosition::Earliest,
                StartArg::LatestFile => StartPosition::LatestFile,
                StartArg::End => StartPosition::End,
            },
            use_notify: !self.no_notify,
            poll_interval: Duration::from_millis(self.poll_interval_ms.max(1)),
            max_lag_blocks: self.max_lag_blocks,
            merge: MergeConfig {
                gap_wait: Duration::from_millis(self.gap_wait_ms),
                confirm_window_blocks: self.confirm_window_blocks,
                stall_after: Duration::from_secs(self.stall_after_secs),
            },
            snapshot_interval: secs(self.snapshot_interval_secs),
            snapshot_path: self.snapshot_path.clone(),
        };
        let mut derive = DeriveConfig::hyperliquid()?;
        derive.book_start = match self.book_start {
            BookStartArg::Snapshot => BookStart::Snapshot,
            BookStartArg::Empty => BookStart::Empty,
        };
        derive.processor.checkpoint = if self.book_checkpoint_secs == 0 {
            book::checkpoint::CheckpointPolicy::Never
        } else {
            book::checkpoint::CheckpointPolicy::EveryMillis(
                i64::try_from(self.book_checkpoint_secs.saturating_mul(1_000)).unwrap_or(i64::MAX),
            )
        };
        derive.coverage_interval = interval(&self.coverage_interval)?;
        derive.recent_blocks = self.recent_blocks;

        let mut signing_keys = Vec::new();
        if let Some(f) = &self.signing_key_file {
            signing_keys.push(bus::KeySpec {
                id: self.signing_key_id.clone(),
                source: bus::KeySource::File(f.clone()),
            });
        }
        signing_keys.extend(self.signing_keys.iter().cloned());
        if signing_keys.is_empty() {
            bail!("no signing key: set BLEH_SIGNING_KEY_FILE (or --signing-key ID=file:PATH)");
        }

        let gateway = gateway::GatewayConfig {
            trust_forwarded_for: self.trust_forwarded_for,
            max_staleness: Duration::from_secs(self.max_staleness_secs.max(1)),
            ..Default::default()
        };
        let nats = self
            .nats_url
            .clone()
            .map(|url| bus::jetstream::JetStreamConfig {
                url,
                stream: self.nats_stream.clone(),
                replay_window: Duration::from_secs(self.nats_replay_window_secs),
                credentials: self.nats_credentials.clone(),
                ..Default::default()
            });
        let capture = match (&self.raw_dir, self.no_capture) {
            (Some(dir), false) => Some(CaptureSettings {
                staging: dir.clone(),
                private: !self.capture_no_private,
                upload: self.capture_upload,
                keep_local: self.capture_keep_local,
                metrics_out: self.capture_metrics_out.clone().map(|p| {
                    (
                        p,
                        Duration::from_secs(self.capture_metrics_every_secs.max(1)),
                    )
                }),
                poll_interval: Duration::from_millis(self.capture_poll_ms.max(10)),
                seal_grace: Duration::from_secs(self.capture_seal_grace_secs),
            }),
            _ => None,
        };
        Ok(OriginConfig {
            reader,
            derive,
            signing_keys,
            active_key: self.active_key.clone(),
            gateway_listen: self.gateway_listen,
            gateway,
            proof_retention_blocks: self.proof_retention_blocks,
            metrics_listen: Some(self.metrics_addr),
            nats,
            capture,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags_and_requires_a_key() {
        let cli = Cli::try_parse_from(["origin", "--node-dir", "/hl/data"]).unwrap();
        assert!(cli.config().is_err());
        let cli = Cli::try_parse_from([
            "origin",
            "--node-dir",
            "/a,/b",
            "--signing-key-file",
            "/run/secrets/signing_key",
            "--raw-dir",
            "/var/lib/bleh/raw",
            "--book-start",
            "empty",
            "--coverage-interval",
            "5m",
        ])
        .unwrap();
        let cfg = cli.config().unwrap();
        assert_eq!(cfg.reader.node_dirs.len(), 2);
        assert_eq!(cfg.signing_keys[0].id, "origin");
        assert_eq!(cfg.gateway_listen.port(), 8080);
        assert_eq!(cfg.derive.book_start, BookStart::Empty);
        assert_eq!(cfg.derive.coverage_interval, BarInterval::M5);
        let cap = cfg.capture.unwrap();
        assert!(cap.private && !cap.upload);
        assert!(
            Cli::try_parse_from([
                "origin",
                "--node-dir",
                "/a",
                "--coverage-interval",
                "2m",
                "--signing-key-file",
                "/k"
            ])
            .unwrap()
            .config()
            .is_err()
        );
    }
}
