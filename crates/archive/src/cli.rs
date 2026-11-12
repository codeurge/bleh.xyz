//! The `archive` command line: capture, partition, replay, iceberg,
//! manifest, anchor, publish and verify.
//!
//! The node reader's line filters and raw parser are injected through
//! [`Hooks`]; this crate does not depend on the reader. A build without
//! them can run every command except filtered capture and replay, which
//! fail with a clear message.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

use mesh_types::{EthAddress, Record, Sha256Digest, UnixNanos};

use crate::anchor::{self, evm, ots};
use crate::capture::{self, Capture, Compression, Dictionary, FilterSet, HourWriter, TierConfig};
use crate::error::{Error, IoContext, Result};
use crate::http::{HttpClient, ReqwestClient};
use crate::layout::{self, Day};
use crate::manifest;
use crate::partition::{self, Existing, PartitionOptions};
use crate::publish;
use crate::replay::{self, RawParser, ReplayOptions};
use crate::store::{Store, StoreConfig};
use crate::verify::{self, FileCheck, Status, VerifyOptions};

/// Components injected by the integrator.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Line filters for the filtered tier, by stream.
    pub filters: FilterSet,
    /// Parser from filtered raw files to records.
    pub parser: Option<Arc<dyn RawParser>>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("filters", &self.filters)
            .field("parser", &self.parser.is_some())
            .finish()
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "archive",
    version,
    about = "bleh.xyz archive: raw capture, partitions, replay, manifests, anchors, publishing and verification"
)]
struct Cli {
    /// Log format.
    #[arg(long, value_enum, default_value_t = LogFormat::Text, global = true)]
    log_format: LogFormat,
    /// Write Prometheus counters to this file on exit (textfile collector).
    #[arg(long, global = true)]
    metrics_out: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Raw capture (private and filtered tiers).
    #[command(subcommand)]
    Capture(CaptureCmd),
    /// Write partitions from records (JSON lines, observation-v0).
    Partition(PartitionArgs),
    /// Rebuild a day's partitions from filtered raw.
    Replay(ReplayArgs),
    /// Rebuild the static Iceberg metadata of a normalized archive.
    Iceberg(IcebergArgs),
    /// Daily manifests.
    #[command(subcommand)]
    Manifest(ManifestCmd),
    /// Bitcoin (OpenTimestamps) and Ethereum L2 anchors.
    #[command(subcommand)]
    Anchor(AnchorCmd),
    /// Upload a signed day, pin it on IPFS, refresh torrents and magnets.
    Publish(PublishArgs),
    /// Verify a day from a directory or URL: signature, chain, anchors, files.
    Verify(VerifyArgs),
}

#[derive(Debug, Subcommand)]
enum CaptureCmd {
    /// Capture complete node hour files from node stream directories.
    Run(CaptureRunArgs),
    /// Upload sealed hours from the staging directory.
    Upload(CaptureUploadArgs),
    /// Train a zstd dictionary for a private-tier stream from node files.
    TrainDict(TrainDictArgs),
}

#[derive(Debug, Args)]
struct CaptureRunArgs {
    /// Staging directory.
    #[arg(long)]
    staging: PathBuf,
    /// Stream and its node directory, as `stream=DIR` (for example
    /// `fills=/hl/data/node_fills_by_block`). Repeatable.
    #[arg(long = "stream", value_parser = parse_kv)]
    streams: Vec<(String, String)>,
    /// Skip the private tier.
    #[arg(long)]
    no_private: bool,
    /// Skip the filtered tier.
    #[arg(long)]
    no_filtered: bool,
    /// Private-tier zstd level.
    #[arg(long, default_value_t = 19)]
    private_level: i32,
    /// Private-tier long-distance window (log2 bytes); 0 disables.
    #[arg(long, default_value_t = 27)]
    long_window_log: u32,
    /// Private-tier dictionary for a stream, as `stream=FILE`. Repeatable.
    #[arg(long = "dict", value_parser = parse_kv)]
    dicts: Vec<(String, String)>,
    /// Upload sealed hours afterwards (stores from BLEH_ARCHIVE_PRIVATE_* and
    /// BLEH_ARCHIVE_PUBLIC_*).
    #[arg(long)]
    upload: bool,
    /// Keep uploaded files under `uploaded/`.
    #[arg(long)]
    keep_local: bool,
}

#[derive(Debug, Args)]
struct CaptureUploadArgs {
    /// Staging directory.
    #[arg(long)]
    staging: PathBuf,
    /// Keep uploaded files under `uploaded/`.
    #[arg(long)]
    keep_local: bool,
}

#[derive(Debug, Args)]
struct TrainDictArgs {
    /// Node hour files to sample lines from.
    #[arg(required = true)]
    samples: Vec<PathBuf>,
    /// Output dictionary file.
    #[arg(long)]
    out: PathBuf,
    /// Maximum dictionary size in bytes.
    #[arg(long, default_value_t = 112_640)]
    max_size: usize,
    /// Maximum number of sample lines.
    #[arg(long, default_value_t = 100_000)]
    max_lines: usize,
}

#[derive(Debug, Args)]
struct PartitionArgs {
    /// Records as JSON lines (`-` for stdin).
    #[arg(long)]
    input: PathBuf,
    /// Normalized archive root.
    #[arg(long)]
    out: PathBuf,
    /// Write as a correction (new part numbers) instead of write-once.
    #[arg(long)]
    correction: bool,
}

#[derive(Debug, Args)]
struct ReplayArgs {
    /// Filtered raw archive root.
    #[arg(long)]
    raw: PathBuf,
    /// Day.
    #[arg(long)]
    date: Day,
    /// Normalized archive root to write.
    #[arg(long)]
    out: PathBuf,
    /// Allow hour 00 of the next day to be missing.
    #[arg(long)]
    allow_missing_next_hour: bool,
    /// Compare the result with partitions under this root (content hashes).
    #[arg(long)]
    check_against: Option<PathBuf>,
    /// With --check-against, compare bytes instead of content hashes.
    #[arg(long)]
    bytes: bool,
    /// Write as a correction (new part numbers).
    #[arg(long)]
    correction: bool,
}

#[derive(Debug, Args)]
struct IcebergArgs {
    /// Normalized archive root.
    #[arg(long)]
    root: PathBuf,
    /// Archive name.
    #[arg(long, default_value = layout::NORMALIZED_ARCHIVE)]
    archive: String,
    /// The archive root's URI as readers will see it (for example
    /// `s3://bucket/hyperliquid-normalized`).
    #[arg(long)]
    location: String,
    /// Also upload (store from BLEH_ARCHIVE_PUBLIC_*).
    #[arg(long)]
    upload: bool,
}

#[derive(Debug, Subcommand)]
enum ManifestCmd {
    /// Build the day's manifest and torrent (and sign with --sign).
    Build(BuildArgs),
    /// Sign a built manifest.
    Sign(SignArgs),
    /// Verify a day's manifest, chain and anchors (no file downloads).
    Verify(VerifyArgs),
    /// Generate a new signing key file (mode 600).
    Keygen {
        /// Output file.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Debug, Args)]
struct BuildArgs {
    /// Archive root directory.
    #[arg(long)]
    root: PathBuf,
    /// Archive name.
    #[arg(long)]
    archive: String,
    /// Day.
    #[arg(long)]
    date: Day,
    /// HTTPS web seed base URL ending in `/`. Repeatable.
    #[arg(long = "web-seed", required = true)]
    web_seeds: Vec<String>,
    /// Tracker URL for the .torrent file. Repeatable.
    #[arg(long = "tracker")]
    trackers: Vec<String>,
    /// Signing key file (or BLEH_ARCHIVE_SIGNING_KEY_FILE).
    #[arg(long, env = "BLEH_ARCHIVE_SIGNING_KEY_FILE")]
    key_file: Option<PathBuf>,
    /// Signer address, when the key is used elsewhere.
    #[arg(long)]
    signer: Option<EthAddress>,
    /// Successor key address (rotation).
    #[arg(long)]
    next_signer: Option<EthAddress>,
    /// Build time in Unix nanoseconds (default: now).
    #[arg(long)]
    created_ns: Option<i64>,
    /// Start a new chain at sequence 0 even if earlier manifests exist.
    #[arg(long)]
    genesis: bool,
    /// Sign immediately (needs the key file).
    #[arg(long)]
    sign: bool,
}

#[derive(Debug, Args)]
struct SignArgs {
    /// Archive root directory.
    #[arg(long)]
    root: PathBuf,
    /// Archive name.
    #[arg(long)]
    archive: String,
    /// Day.
    #[arg(long)]
    date: Day,
    /// Signing key file.
    #[arg(long, env = "BLEH_ARCHIVE_SIGNING_KEY_FILE")]
    key_file: PathBuf,
}

#[derive(Debug, Subcommand)]
enum AnchorCmd {
    /// Submit the manifest hash to OpenTimestamps calendars.
    OtsStamp(OtsArgs),
    /// Upgrade the proof from the calendars (and check it against Bitcoin).
    OtsUpgrade(OtsArgs),
    /// Submit (or dry-run) the Ethereum L2 anchor transaction.
    EvmSubmit(EvmArgs),
    /// Poll the L2 transaction and record the anchor once included.
    EvmPoll(EvmArgs),
}

#[derive(Debug, Args)]
struct OtsArgs {
    /// Archive root directory.
    #[arg(long)]
    root: PathBuf,
    /// Day.
    #[arg(long)]
    date: Day,
    /// Calendar URL (default: the public pool). Repeatable.
    #[arg(long = "calendar")]
    calendars: Vec<String>,
    /// Esplora-compatible Bitcoin API to verify attestations (upgrade).
    #[arg(long)]
    bitcoin_api: Option<String>,
}

#[derive(Debug, Args)]
struct EvmArgs {
    /// Archive root directory.
    #[arg(long)]
    root: PathBuf,
    /// Day.
    #[arg(long)]
    date: Day,
    /// EIP-155 chain ID.
    #[arg(long)]
    chain_id: u64,
    /// JSON-RPC URL (required unless --dry-run).
    #[arg(long, env = "BLEH_ARCHIVE_EVM_RPC")]
    rpc: Option<String>,
    /// Anchor signing key file.
    #[arg(long, env = "BLEH_ARCHIVE_EVM_KEY_FILE")]
    key_file: PathBuf,
    /// Anchoring contract (default: self-send with the hash as calldata).
    #[arg(long)]
    to: Option<EthAddress>,
    /// Sign but do not broadcast.
    #[arg(long)]
    dry_run: bool,
    /// Nonce for --dry-run.
    #[arg(long, default_value_t = 0)]
    nonce: u64,
    /// Gas limit (default: estimate).
    #[arg(long)]
    gas_limit: Option<u64>,
    /// Max fee per gas in wei.
    #[arg(long)]
    max_fee: Option<u128>,
    /// Priority fee per gas in wei.
    #[arg(long)]
    priority_fee: Option<u128>,
    /// Use a legacy (EIP-155) transaction.
    #[arg(long)]
    legacy: bool,
    /// Confirmations before the anchor is final.
    #[arg(long, default_value_t = 12)]
    confirmations: u64,
}

#[derive(Debug, Args)]
struct PublishArgs {
    /// Archive root directory.
    #[arg(long)]
    root: PathBuf,
    /// Archive name.
    #[arg(long)]
    archive: String,
    /// Day.
    #[arg(long)]
    date: Day,
    /// Upload to the store from BLEH_ARCHIVE_PUBLIC_* (or --store).
    #[arg(long)]
    upload: bool,
    /// Store URL overriding BLEH_ARCHIVE_PUBLIC_URL.
    #[arg(long)]
    store: Option<String>,
    /// Kubo RPC API URL to add and pin on IPFS (for example http://127.0.0.1:5001).
    #[arg(long, env = "BLEH_ARCHIVE_KUBO_API")]
    kubo: Option<String>,
    /// Rebuild Iceberg metadata for this location after publishing
    /// (normalized archives).
    #[arg(long)]
    iceberg_location: Option<String>,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    /// Local archive root.
    #[arg(long, conflicts_with = "url")]
    dir: Option<PathBuf>,
    /// Archive root URL (a web seed followed by the archive name).
    #[arg(long)]
    url: Option<String>,
    /// Archive name.
    #[arg(long)]
    archive: String,
    /// Day.
    #[arg(long)]
    date: Day,
    /// Trusted signer address. Repeatable.
    #[arg(long = "trusted")]
    trusted: Vec<EthAddress>,
    /// Trusted manifest hash to stop the chain walk at.
    #[arg(long)]
    checkpoint: Option<Sha256Digest>,
    /// Also check IPFS CIDs, pieces roots and partition content hashes.
    #[arg(long)]
    deep: bool,
    /// Do not fetch files.
    #[arg(long)]
    no_files: bool,
    /// Esplora-compatible Bitcoin API for OpenTimestamps verification.
    #[arg(long)]
    bitcoin_api: Option<String>,
    /// Ethereum L2 JSON-RPC URL for anchor verification.
    #[arg(long)]
    evm_rpc: Option<String>,
    /// Confirmations required for the L2 anchor.
    #[arg(long, default_value_t = 12)]
    confirmations: u64,
    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
    /// Treat warnings as failures.
    #[arg(long)]
    strict: bool,
}

fn parse_kv(s: &str) -> std::result::Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .ok_or_else(|| format!("expected NAME=VALUE, got {s:?}"))
}

fn http() -> Result<Arc<dyn HttpClient>> {
    Ok(Arc::new(ReqwestClient::new(Duration::from_secs(60))?))
}

fn public_store(url: Option<&str>) -> Result<Store> {
    let cfg = match url {
        Some(u) => {
            let mut c = StoreConfig::from_env("PUBLIC").unwrap_or_else(|| StoreConfig::new(u));
            c.url = u.to_owned();
            c
        }
        None => StoreConfig::from_env("PUBLIC")
            .ok_or_else(|| Error::invalid("BLEH_ARCHIVE_PUBLIC_URL is not set"))?,
    };
    cfg.open()
}

fn init_logging(format: LogFormat) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let b = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    let _ = match format {
        LogFormat::Text => b.try_init(),
        LogFormat::Json => b.json().try_init(),
    };
}

/// Runs the command line with the given hooks; returns the exit code.
#[must_use]
pub fn run(hooks: Hooks) -> ExitCode {
    run_from(std::env::args_os(), hooks)
}

/// Like [`run`], with explicit arguments (for tests).
pub fn run_from<I, T>(args: I, hooks: Hooks) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    init_logging(cli.log_format);
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("archive: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(dispatch(cli.command, &hooks));
    if let Some(p) = &cli.metrics_out
        && let Err(e) = crate::metrics::global().write_textfile(p)
    {
        tracing::error!(error = %e, "could not write metrics");
    }
    match result {
        Ok(code) => code,
        Err(e) => {
            tracing::error!(error = %e, "archive failed");
            eprintln!("archive: {e}");
            ExitCode::FAILURE
        }
    }
}

fn now() -> UnixNanos {
    UnixNanos::now()
}

async fn dispatch(cmd: Command, hooks: &Hooks) -> Result<ExitCode> {
    match cmd {
        Command::Capture(c) => capture_cmd(c, hooks).await,
        Command::Partition(a) => partition_cmd(&a),
        Command::Replay(a) => replay_cmd(&a, hooks),
        Command::Iceberg(a) => {
            let store = if a.upload {
                Some(public_store(None)?)
            } else {
                None
            };
            let files =
                publish::write_iceberg(&a.root, &a.archive, &a.location, store.as_ref()).await?;
            println!("{} iceberg metadata files", files.len());
            Ok(ExitCode::SUCCESS)
        }
        Command::Manifest(m) => manifest_cmd(m).await,
        Command::Anchor(a) => anchor_cmd(a).await,
        Command::Publish(a) => publish_cmd(a).await,
        Command::Verify(a) => verify_cmd(a, FileCheck::Hashes).await,
    }
}

async fn capture_cmd(c: CaptureCmd, hooks: &Hooks) -> Result<ExitCode> {
    match c {
        CaptureCmd::Run(a) => {
            let private = if a.no_private {
                None
            } else {
                let mut cfg = TierConfig::private(&a.staging);
                cfg.compression = Compression {
                    level: a.private_level,
                    long_window_log: (a.long_window_log > 0).then_some(a.long_window_log),
                    dictionaries: Default::default(),
                };
                for (stream, file) in &a.dicts {
                    let bytes = std::fs::read(file).at(file)?;
                    cfg.compression
                        .dictionaries
                        .insert(stream.clone(), Arc::new(Dictionary::from_bytes(bytes)?));
                }
                Some(HourWriter::open(cfg)?)
            };
            let filtered = if a.no_filtered {
                None
            } else {
                Some(HourWriter::open(TierConfig::filtered(&a.staging))?)
            };
            let mut cap = Capture::new(private, filtered, hooks.filters.clone())?;
            let streams: Vec<(String, PathBuf)> = a
                .streams
                .iter()
                .map(|(s, d)| (s.clone(), PathBuf::from(d)))
                .collect();
            let sealed = capture::capture_node_dirs(&mut cap, &streams)?;
            for s in &sealed {
                println!(
                    "{} {} {} lines {} bytes {}",
                    s.archive, s.path, s.lines, s.size, s.sha256
                );
            }
            if a.upload {
                upload_staging(&a.staging, a.keep_local).await?;
            }
            Ok(ExitCode::SUCCESS)
        }
        CaptureCmd::Upload(a) => upload_staging(&a.staging, a.keep_local).await,
        CaptureCmd::TrainDict(a) => {
            let mut samples = Vec::new();
            'files: for p in &a.samples {
                let f = std::fs::File::open(p).at(p)?;
                for line in std::io::BufReader::new(f).split(b'\n') {
                    samples.push(line.at(p)?);
                    if samples.len() >= a.max_lines {
                        break 'files;
                    }
                }
            }
            let d = Dictionary::train(&samples, a.max_size)?;
            partition::write_once(&a.out, &d.bytes)?;
            println!(
                "dictionary id {} ({} bytes) from {} lines",
                d.id,
                d.bytes.len(),
                samples.len()
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn upload_staging(staging: &Path, keep: bool) -> Result<ExitCode> {
    let mut code = ExitCode::SUCCESS;
    for (tier, archive) in [
        ("PRIVATE", layout::PRIVATE_RAW_ARCHIVE),
        ("PUBLIC", layout::RAW_ARCHIVE),
    ] {
        if !staging.join(archive).join("ready").exists() {
            continue;
        }
        let Some(cfg) = StoreConfig::from_env(tier) else {
            return Err(Error::invalid(format!(
                "BLEH_ARCHIVE_{tier}_URL is not set"
            )));
        };
        let r = crate::store::upload_ready(staging, archive, &cfg.open()?, keep).await?;
        println!(
            "{archive}: {} created, {} identical, {} conflicts",
            r.created.len(),
            r.identical.len(),
            r.conflicts.len()
        );
        if !r.conflicts.is_empty() {
            code = ExitCode::FAILURE;
        }
    }
    Ok(code)
}

fn partition_cmd(a: &PartitionArgs) -> Result<ExitCode> {
    let reader: Box<dyn BufRead> = if a.input.as_os_str() == "-" {
        Box::new(std::io::BufReader::new(std::io::stdin()))
    } else {
        Box::new(std::io::BufReader::new(
            std::fs::File::open(&a.input).at(&a.input)?,
        ))
    };
    let mut records = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.at(&a.input)?;
        if line.trim().is_empty() {
            continue;
        }
        let r: Record = serde_json::from_str(&line)
            .map_err(|e| Error::invalid(format!("line {}: {e}", i + 1)))?;
        records.push(r);
    }
    let opts = PartitionOptions {
        existing: if a.correction {
            Existing::NewParts
        } else {
            Existing::RequireIdentical
        },
        ..PartitionOptions::default()
    };
    for f in partition::write_partitions(&a.out, records, &opts)? {
        println!(
            "{} {} records {} {}",
            f.path,
            f.record_count,
            f.content_hash,
            if f.already_present {
                "present"
            } else {
                "written"
            }
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn replay_cmd(a: &ReplayArgs, hooks: &Hooks) -> Result<ExitCode> {
    let parser = hooks.parser.as_ref().ok_or(Error::NotWired(
        "replay needs the node reader's raw parser; it is not wired into this build",
    ))?;
    let opts = ReplayOptions {
        require_next_hour: !a.allow_missing_next_hour,
        partition: PartitionOptions {
            existing: if a.correction {
                Existing::NewParts
            } else {
                Existing::RequireIdentical
            },
            ..PartitionOptions::default()
        },
    };
    let report = replay::replay_day(&a.raw, a.date, parser.as_ref(), &a.out, &opts)?;
    for (p, h) in &report.inputs {
        println!("input  {p} {h}");
    }
    for f in &report.files {
        println!(
            "output {} {} records content {} sha256 {}",
            f.path, f.record_count, f.content_hash, f.sha256
        );
    }
    println!(
        "day {} records {} content {}",
        report.day, report.records, report.day_content_hash
    );
    if !report.missing_next_hour.is_empty() {
        println!(
            "warning: no next-day hour 00 for {:?}",
            report.missing_next_hour
        );
    }
    if let Some(other) = &a.check_against {
        let differ = replay::compare_with(&report, other, a.bytes)?;
        if !differ.is_empty() {
            for d in &differ {
                println!("differs {d}");
            }
            return Ok(ExitCode::FAILURE);
        }
        println!("matches {}", other.display());
    }
    Ok(ExitCode::SUCCESS)
}

async fn manifest_cmd(m: ManifestCmd) -> Result<ExitCode> {
    match m {
        ManifestCmd::Build(a) => {
            let key = a
                .key_file
                .as_deref()
                .map(manifest::load_signing_key)
                .transpose()?;
            let signer = match (&key, a.signer) {
                (Some(k), None) => k.address(),
                (Some(k), Some(s)) if k.address() == s => s,
                (Some(_), Some(_)) => {
                    return Err(Error::invalid("--signer does not match the key file"));
                }
                (None, Some(s)) => s,
                (None, None) => return Err(Error::invalid("give --key-file or --signer")),
            };
            let previous = if a.genesis {
                None
            } else {
                manifest::previous_for(&a.root, a.date)?
            };
            let opts = manifest::BuildOptions {
                archive: a.archive.clone(),
                day: a.date,
                web_seeds: a.web_seeds,
                trackers: a.trackers,
                created_ns: a.created_ns.map_or_else(now, UnixNanos),
                signer,
                next_signer: a.next_signer,
                previous,
            };
            let built = manifest::build(&a.root, &opts)?;
            let torrent = layout::torrent_file(&a.archive, a.date);
            partition::write_once(&a.root.join(&torrent), &built.torrent.metainfo)?;
            println!(
                "{} {} sequence {} files {} infohash {}",
                a.archive,
                a.date,
                built.manifest.sequence,
                built.manifest.files.len(),
                built.manifest.torrent.infohash_v2
            );
            if a.sign {
                let key = key.ok_or_else(|| Error::invalid("--sign needs --key-file"))?;
                let signed = manifest::sign(built.manifest, &key)?;
                let w = manifest::write_signed(&a.root, &signed, &built.torrent.metainfo)?;
                println!("signed {} manifest {}", w.manifest, w.sha256);
            } else {
                let p = manifest::write_unsigned(&a.root, &built.manifest)?;
                println!("unsigned {}", p.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        ManifestCmd::Sign(a) => {
            let key = manifest::load_signing_key(&a.key_file)?;
            let m = manifest::read_unsigned(&a.root, a.date)?;
            if m.archive != a.archive {
                return Err(Error::invalid("unsigned manifest is for another archive"));
            }
            let torrent_path = a.root.join(layout::torrent_file(&a.archive, a.date));
            let torrent = std::fs::read(&torrent_path).at(&torrent_path)?;
            let signed = manifest::sign(m, &key)?;
            let w = manifest::write_signed(&a.root, &signed, &torrent)?;
            println!("signed {} manifest {}", w.manifest, w.sha256);
            Ok(ExitCode::SUCCESS)
        }
        ManifestCmd::Verify(a) => verify_cmd(a, FileCheck::Skip).await,
        ManifestCmd::Keygen { out } => {
            let addr = manifest::generate_signing_key(&out)?;
            println!("{addr}");
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn anchor_cmd(a: AnchorCmd) -> Result<ExitCode> {
    match a {
        AnchorCmd::OtsStamp(o) => {
            let cal = ots::Calendars::new(http()?, o.calendars);
            let anchor = anchor::ots_stamp(&o.root, o.date, &cal).await?;
            println!(
                "opentimestamps {:?} proof {}",
                anchor.status, anchor.proof_sha256
            );
            Ok(ExitCode::SUCCESS)
        }
        AnchorCmd::OtsUpgrade(o) => {
            let h = http()?;
            let cal = ots::Calendars::new(h.clone(), o.calendars);
            let headers = o.bitcoin_api.map(|u| ots::EsploraHeaders::new(h, u));
            let anchor = anchor::ots_upgrade(
                &o.root,
                o.date,
                &cal,
                headers.as_ref().map(|x| x as &dyn ots::BitcoinHeaders),
            )
            .await?;
            println!(
                "opentimestamps {:?}{}",
                anchor.status,
                anchor
                    .bitcoin_block_height
                    .map(|b| format!(" block {b}"))
                    .unwrap_or_default()
            );
            Ok(ExitCode::SUCCESS)
        }
        AnchorCmd::EvmSubmit(e) | AnchorCmd::EvmPoll(e) if e.dry_run => {
            let key = manifest::load_signing_key(&e.key_file)?;
            let sub = evm::DryRunSubmitter::new(key, evm_config(&e), e.nonce);
            let p = anchor::evm_submit(&e.root, e.date, &sub, now()).await?;
            println!(
                "dry run: chain {} tx {} (not broadcast)\nraw {}",
                p.chain_id, p.tx_hash, p.raw_tx
            );
            Ok(ExitCode::SUCCESS)
        }
        AnchorCmd::EvmSubmit(e) => {
            let sub = rpc_submitter(&e)?;
            let p = anchor::evm_submit(&e.root, e.date, &sub, now()).await?;
            println!("submitted chain {} tx {}", p.chain_id, p.tx_hash);
            Ok(ExitCode::SUCCESS)
        }
        AnchorCmd::EvmPoll(e) => {
            let sub = rpc_submitter(&e)?;
            match anchor::evm_poll(&e.root, e.date, &sub).await? {
                Some(a) => println!(
                    "ethereum_l2 {:?} block {} tx {}",
                    a.status, a.block_number, a.tx_hash
                ),
                None => println!("not yet included"),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn evm_config(e: &EvmArgs) -> evm::EvmConfig {
    let mut cfg = evm::EvmConfig::new(e.chain_id);
    cfg.rpc_url.clone_from(&e.rpc);
    cfg.to = e.to;
    cfg.tx_type = if e.legacy {
        evm::TxType::Legacy
    } else {
        evm::TxType::Eip1559
    };
    cfg.gas_limit = e.gas_limit;
    cfg.max_fee_per_gas = e.max_fee;
    cfg.max_priority_fee_per_gas = e.priority_fee;
    cfg.confirmations = e.confirmations;
    cfg
}

fn rpc_submitter(e: &EvmArgs) -> Result<evm::RpcSubmitter> {
    let url = e
        .rpc
        .clone()
        .ok_or_else(|| Error::invalid("--rpc is required (or --dry-run)"))?;
    let key = manifest::load_signing_key(&e.key_file)?;
    Ok(evm::RpcSubmitter::new(
        key,
        evm_config(e),
        evm::EvmRpc::new(http()?, url),
    ))
}

async fn publish_cmd(a: PublishArgs) -> Result<ExitCode> {
    let store = if a.upload || a.store.is_some() {
        Some(public_store(a.store.as_deref())?)
    } else {
        None
    };
    let kubo = a
        .kubo
        .map(|u| http().map(|h| publish::KuboClient::new(h, u)))
        .transpose()?;
    let report = publish::publish_day(
        &a.root,
        &a.archive,
        a.date,
        store.as_ref(),
        kubo.as_ref().map(|k| k as &dyn publish::IpfsPinner),
    )
    .await?;
    for (p, o) in &report.uploaded {
        println!("upload {p} {o:?}");
    }
    for p in &report.replaced {
        println!("replace {p}");
    }
    for (p, c) in &report.pinned {
        println!("pin {p} {c}");
    }
    if let Some(loc) = a.iceberg_location {
        let files = publish::write_iceberg(&a.root, &a.archive, &loc, store.as_ref()).await?;
        println!("iceberg {} files", files.len());
    }
    Ok(ExitCode::SUCCESS)
}

async fn verify_cmd(a: VerifyArgs, default_files: FileCheck) -> Result<ExitCode> {
    let h = http()?;
    let source: Box<dyn verify::Source> = match (&a.dir, &a.url) {
        (Some(d), None) => Box::new(verify::DirSource::new(d)),
        (None, Some(u)) => Box::new(verify::HttpSource::new(h.clone(), u.clone())),
        _ => return Err(Error::invalid("give exactly one of --dir or --url")),
    };
    let mut opts = VerifyOptions::new(a.trusted, now());
    opts.checkpoint = a.checkpoint;
    opts.files = if a.no_files {
        FileCheck::Skip
    } else if a.deep {
        FileCheck::Deep
    } else {
        default_files
    };
    if let Some(api) = a.bitcoin_api {
        opts.bitcoin = Some(Arc::new(ots::EsploraHeaders::new(h.clone(), api)));
    }
    if let Some(rpc) = a.evm_rpc {
        opts.evm = Some((evm::EvmRpc::new(h, rpc), a.confirmations));
    }
    let report = verify::verify_day(source.as_ref(), &a.archive, a.date, &opts).await?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{report}");
    }
    let failed = match report.status() {
        Status::Fail => true,
        Status::Warn => a.strict,
        Status::Pass => false,
    };
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_every_subcommand() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        for args in [
            vec![
                "archive",
                "capture",
                "run",
                "--staging",
                "/s",
                "--stream",
                "fills=/d",
            ],
            vec!["archive", "capture", "upload", "--staging", "/s"],
            vec!["archive", "capture", "train-dict", "a", "--out", "d"],
            vec!["archive", "partition", "--input", "-", "--out", "/o"],
            vec![
                "archive",
                "replay",
                "--raw",
                "/r",
                "--date",
                "2026-01-01",
                "--out",
                "/o",
            ],
            vec![
                "archive",
                "iceberg",
                "--root",
                "/r",
                "--location",
                "s3://b/x",
            ],
            vec![
                "archive",
                "manifest",
                "build",
                "--root",
                "/r",
                "--archive",
                "a",
                "--date",
                "2026-01-01",
                "--web-seed",
                "https://m/",
                "--signer",
                "0x0000000000000000000000000000000000000001",
            ],
            vec![
                "archive",
                "manifest",
                "sign",
                "--root",
                "/r",
                "--archive",
                "a",
                "--date",
                "2026-01-01",
                "--key-file",
                "k",
            ],
            vec![
                "archive",
                "manifest",
                "verify",
                "--dir",
                "/r",
                "--archive",
                "a",
                "--date",
                "2026-01-01",
            ],
            vec!["archive", "manifest", "keygen", "--out", "k"],
            vec![
                "archive",
                "anchor",
                "ots-stamp",
                "--root",
                "/r",
                "--date",
                "2026-01-01",
            ],
            vec![
                "archive",
                "anchor",
                "ots-upgrade",
                "--root",
                "/r",
                "--date",
                "2026-01-01",
                "--bitcoin-api",
                "https://x",
            ],
            vec![
                "archive",
                "anchor",
                "evm-submit",
                "--root",
                "/r",
                "--date",
                "2026-01-01",
                "--chain-id",
                "10",
                "--key-file",
                "k",
                "--dry-run",
            ],
            vec![
                "archive",
                "anchor",
                "evm-poll",
                "--root",
                "/r",
                "--date",
                "2026-01-01",
                "--chain-id",
                "10",
                "--key-file",
                "k",
                "--rpc",
                "https://x",
            ],
            vec![
                "archive",
                "publish",
                "--root",
                "/r",
                "--archive",
                "a",
                "--date",
                "2026-01-01",
            ],
            vec![
                "archive",
                "--log-format",
                "json",
                "verify",
                "--url",
                "https://m/a",
                "--archive",
                "a",
                "--date",
                "2026-01-01",
                "--deep",
            ],
        ] {
            Cli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
        assert!(
            Cli::try_parse_from([
                "archive",
                "replay",
                "--raw",
                "/r",
                "--date",
                "2026-13-01",
                "--out",
                "/o"
            ])
            .is_err()
        );
    }

    #[test]
    fn unwired_hooks_fail_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let code = run_from(
            [
                "archive",
                "replay",
                "--raw",
                &dir.path().display().to_string(),
                "--date",
                "2026-01-01",
                "--out",
                &dir.path().display().to_string(),
            ],
            Hooks::default(),
        );
        assert_eq!(code, ExitCode::FAILURE);
    }
}
