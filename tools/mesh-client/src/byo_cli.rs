//! `mesh-client byo`: bring your own feed (ADR 0014).
//!
//! ```text
//! mesh-client byo check-map --map pairs.toml
//! mesh-client byo merge --map pairs.toml --url wss://a/v0/ws --feed csv --feed-path mine.csv
//! mesh-client byo merge --map pairs.toml --recording stream.jsonl --feed dbn --feed-path mine.dbn.zst \
//!     --out merged.jsonl --stats tracking.jsonl
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Subcommand};
use mesh_client::ClientConfig;
use mesh_client::byo::feed::{Columns, TextFeed, TextFormat, TsFormat};
use mesh_client::byo::mapping::{Mapping, channel_string};
use mesh_client::byo::run::{BlehSource, Clock, MergeConfig, Outputs, UserSource};
use mesh_client::byo::sink::{JsonlSink, LocalWs};
use mesh_client::byo::{LocalPath, LoopbackAddr};
use mesh_client::probe;
use mesh_types::EthAddress;

#[derive(Debug, Subcommand)]
pub(crate) enum ByoCommand {
    /// Merge your feed with bleh.xyz prices: side-by-side JSON lines, an
    /// optional loopback websocket, and local tracking statistics.
    Merge(Box<MergeArgs>),
    /// Validate a mapping file and print the gateway channels it needs.
    CheckMap {
        /// Mapping file (TOML).
        #[arg(long)]
        map: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum FeedKind {
    /// Delimited text with configurable columns.
    Csv,
    /// One JSON object per line with configurable keys.
    Jsonl,
    /// A DBN file (`.dbn` or `.dbn.zst`).
    Dbn,
    /// The vendor's live client (build with `--features databento-live`).
    Databento,
}

#[derive(Debug, Args)]
pub(crate) struct MergeArgs {
    /// Mapping file (TOML): your symbols to bleh.xyz markets.
    #[arg(long)]
    pub(crate) map: PathBuf,
    /// bleh.xyz gateway websocket URL; repeat for fallbacks.
    #[arg(
        long = "url",
        required_unless_present = "recording",
        conflicts_with = "recording"
    )]
    pub(crate) urls: Vec<String>,
    /// Instead of a gateway: a recording made with `mesh-client --record`.
    #[arg(long)]
    pub(crate) recording: Option<PathBuf>,
    /// Trusted commitment signer; repeatable.
    #[arg(long = "signer")]
    pub(crate) signers: Vec<EthAddress>,
    /// Your feed's connector.
    #[arg(long, value_enum)]
    pub(crate) feed: FeedKind,
    /// Your feed's file ("-" for standard input); csv, jsonl and dbn.
    #[arg(long)]
    pub(crate) feed_path: Option<PathBuf>,
    /// CSV/JSONL: timestamp column (header name, key, or 0-based index).
    #[arg(long, default_value = "ts")]
    pub(crate) ts_col: String,
    /// CSV/JSONL: timestamp format: ns, us, ms, s or rfc3339 (UTC).
    #[arg(long, default_value = "ns")]
    pub(crate) ts_format: TsFormat,
    /// CSV/JSONL: symbol column.
    #[arg(long, default_value = "symbol")]
    pub(crate) symbol_col: String,
    /// CSV/JSONL: one symbol for every row (no symbol column).
    #[arg(long)]
    pub(crate) symbol: Option<String>,
    /// CSV/JSONL: trade or last price column (default "price" unless
    /// --bid-col and --ask-col are given).
    #[arg(long)]
    pub(crate) price_col: Option<String>,
    /// CSV/JSONL: best bid column.
    #[arg(long, requires = "ask_col")]
    pub(crate) bid_col: Option<String>,
    /// CSV/JSONL: best ask column.
    #[arg(long, requires = "bid_col")]
    pub(crate) ask_col: Option<String>,
    /// CSV: delimiter.
    #[arg(long, default_value_t = ',')]
    pub(crate) delimiter: char,
    /// CSV: no header row (columns are 0-based indices).
    #[arg(long)]
    pub(crate) no_header: bool,
    /// DBN: timestamp to use: event or recv.
    #[arg(long, default_value = "event")]
    pub(crate) dbn_ts: String,
    /// Live: dataset code, as the vendor names it.
    #[arg(long)]
    pub(crate) dataset: Option<String>,
    /// Live: schema (trades, mbp-1, bbo-1s, ...).
    #[arg(long, default_value = "trades")]
    pub(crate) schema: String,
    /// Live: environment variable holding your own API key.
    #[arg(long, default_value = "DATABENTO_API_KEY")]
    pub(crate) key_env: String,
    /// Merged stream: a file you choose, or "-" for standard output.
    #[arg(long, default_value = "-")]
    pub(crate) out: PathBuf,
    /// Do not write the merged stream (statistics only).
    #[arg(long)]
    pub(crate) no_merged: bool,
    /// Tracking statistics file (default: standard error).
    #[arg(long)]
    pub(crate) stats: Option<PathBuf>,
    /// Write statistics this often while live (5s, 2m); always at the end.
    #[arg(long, value_parser = probe::parse_duration)]
    pub(crate) stats_every: Option<Duration>,
    /// Also serve the merged stream on a local websocket (loopback only,
    /// e.g. 127.0.0.1:9100).
    #[arg(long)]
    pub(crate) ws: Option<String>,
    /// A browser origin allowed to connect besides loopback pages;
    /// repeatable.
    #[arg(long = "ws-allow-origin")]
    pub(crate) ws_allow_origins: Vec<String>,
    /// Alignment grid interval.
    #[arg(long, default_value = "1s", value_parser = probe::parse_duration)]
    pub(crate) interval: Duration,
    /// A side's price older than this at a grid time is stale.
    #[arg(long, default_value = "30s", value_parser = probe::parse_duration)]
    pub(crate) max_stale: Duration,
    /// Largest lead or lag tested.
    #[arg(long, default_value = "10s", value_parser = probe::parse_duration)]
    pub(crate) max_lag: Duration,
    /// Samples kept for lead/lag.
    #[arg(long, default_value_t = 3600)]
    pub(crate) window: usize,
    /// event (the feeds' timestamps) or receive (local arrival, live only).
    #[arg(long, default_value = "event")]
    pub(crate) clock: Clock,
    /// Stop after this long (live).
    #[arg(long, value_parser = probe::parse_duration)]
    pub(crate) duration: Option<Duration>,
}

/// Runs a `byo` subcommand.
pub(crate) async fn run(command: ByoCommand) -> anyhow::Result<ExitCode> {
    match command {
        ByoCommand::CheckMap { map } => {
            let m = Mapping::from_file(&map)?;
            let v = serde_json::json!({
                "ok": true,
                "pairs": m.pairs.len(),
                "channels": m.channels().iter().map(channel_string).collect::<Vec<_>>(),
                "inactive": m.inactive().map(|p| serde_json::json!({
                    "pair": p.name,
                    "target": p.bleh.label(),
                    "reason": "not served by gateway v0 yet",
                })).collect::<Vec<_>>(),
            });
            println!("{v}");
            Ok(ExitCode::SUCCESS)
        }
        ByoCommand::Merge(args) => merge(*args).await,
    }
}

fn open_input(path: &Path) -> anyhow::Result<Box<dyn std::io::BufRead + Send>> {
    Ok(if path.as_os_str() == "-" {
        Box::new(std::io::BufReader::new(std::io::stdin()))
    } else {
        Box::new(std::io::BufReader::new(
            std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
        ))
    })
}

fn columns(args: &MergeArgs) -> anyhow::Result<Columns> {
    let both_quotes = args.bid_col.is_some() && args.ask_col.is_some();
    Ok(Columns {
        ts: args.ts_col.clone(),
        symbol: args.symbol.is_none().then(|| args.symbol_col.clone()),
        fixed_symbol: args.symbol.clone(),
        price: args
            .price_col
            .clone()
            .or_else(|| (!both_quotes).then(|| "price".to_string())),
        bid: args.bid_col.clone(),
        ask: args.ask_col.clone(),
        ts_format: args.ts_format,
        delimiter: u8::try_from(args.delimiter)
            .ok()
            .filter(u8::is_ascii)
            .context("--delimiter must be one ASCII character")?,
        has_header: !args.no_header,
    })
}

fn user_source(args: &MergeArgs, mapping: &Mapping) -> anyhow::Result<UserSource> {
    let path = || {
        args.feed_path
            .as_deref()
            .context("--feed-path is required for csv, jsonl and dbn feeds")
    };
    Ok(match args.feed {
        FeedKind::Csv | FeedKind::Jsonl => {
            let format = if args.feed == FeedKind::Csv {
                TextFormat::Csv
            } else {
                TextFormat::Jsonl
            };
            UserSource::Reader(Box::new(TextFeed::new(
                format,
                open_input(path()?)?,
                columns(args)?,
            )?))
        }
        #[cfg(feature = "dbn-files")]
        FeedKind::Dbn => {
            use mesh_client::byo::dbn_feed::{DbnFileFeed, DbnTs};
            let which: DbnTs = args.dbn_ts.parse().map_err(anyhow::Error::msg)?;
            UserSource::Reader(Box::new(DbnFileFeed::open(path()?, which)?))
        }
        #[cfg(not(feature = "dbn-files"))]
        FeedKind::Dbn => anyhow::bail!("this build has no DBN support (feature dbn-files)"),
        #[cfg(feature = "databento-live")]
        FeedKind::Databento => {
            use mesh_client::byo::dbn_feed::{DbnTs, live::LiveConfig};
            let which: DbnTs = args.dbn_ts.parse().map_err(anyhow::Error::msg)?;
            UserSource::DatabentoLive(LiveConfig {
                key_env: args.key_env.clone(),
                dataset: args
                    .dataset
                    .clone()
                    .context("--dataset is required for the live feed")?,
                schema: args.schema.clone(),
                symbols: mapping.user_symbols(),
                ts: which,
            })
        }
        #[cfg(not(feature = "databento-live"))]
        FeedKind::Databento => {
            let _ = mapping;
            anyhow::bail!(
                "this build has no live client: rebuild with `cargo build -p mesh-client --features databento-live`"
            )
        }
    })
}

/// A sink for "-" (standard output) or a file that passed the archive and
/// repository checks.
fn output_sink(path: &Path) -> anyhow::Result<JsonlSink> {
    if path.as_os_str() == "-" {
        return Ok(JsonlSink::stdout());
    }
    let local = LocalPath::new(path)?;
    JsonlSink::create(&local).with_context(|| format!("create {}", path.display()))
}

async fn merge(args: MergeArgs) -> anyhow::Result<ExitCode> {
    let mapping = Mapping::from_file(&args.map)?;
    for p in mapping.inactive() {
        eprintln!(
            "note: pair {:?} ({}) waits for a series gateway v0 does not serve yet; skipped",
            p.name,
            p.bleh.label()
        );
    }
    if mapping.active().count() == 0 {
        anyhow::bail!("the mapping has no pair served today (market pairs)");
    }
    // Check every output before reading or connecting to anything.
    let merged = if args.no_merged {
        None
    } else {
        Some(output_sink(&args.out)?)
    };
    let stats = args.stats.as_deref().map(output_sink).transpose()?;
    let ws_addr = args.ws.as_deref().map(LoopbackAddr::parse).transpose()?;
    let user = user_source(&args, &mapping)?;
    let bleh = match &args.recording {
        Some(p) => BlehSource::Recording(open_input(p)?),
        None => BlehSource::Gateway(ClientConfig {
            urls: args.urls.clone(),
            channels: mapping.channels(),
            signers: args.signers.clone(),
            ..ClientConfig::default()
        }),
    };
    let ws = match ws_addr {
        Some(a) => {
            let ws = LocalWs::bind(a, args.ws_allow_origins.clone()).await?;
            eprintln!(
                "serving the merged stream on ws://{}/ (this machine only)",
                ws.local_addr()
            );
            Some(ws)
        }
        None => None,
    };
    let cfg = MergeConfig {
        mapping,
        interval: args.interval,
        max_stale: args.max_stale,
        max_lag: args.max_lag,
        window: args.window,
        clock: args.clock,
        stats_every: args.stats_every,
        duration: args.duration,
    };
    mesh_client::byo::run::run(cfg, user, bleh, Outputs { merged, stats, ws }).await?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, clap::Parser)]
    struct T {
        #[command(subcommand)]
        c: ByoCommand,
    }

    fn parse(args: &[&str]) -> Result<ByoCommand, clap::Error> {
        <T as clap::Parser>::try_parse_from(std::iter::once("byo").chain(args.iter().copied()))
            .map(|t| t.c)
    }

    fn merge_args(extra: &[&str]) -> MergeArgs {
        let mut a = vec!["merge", "--map", "m.toml", "--url", "ws://a/v0/ws"];
        a.extend_from_slice(extra);
        match parse(&a).unwrap() {
            ByoCommand::Merge(m) => *m,
            ByoCommand::CheckMap { .. } => panic!("not merge"),
        }
    }

    #[test]
    fn merge_parses() {
        let m = merge_args(&[
            "--feed",
            "csv",
            "--feed-path",
            "-",
            "--bid-col",
            "b",
            "--ask-col",
            "a",
            "--ws",
            "127.0.0.1:9100",
            "--interval",
            "500ms",
        ]);
        assert_eq!(
            (m.feed, m.interval, m.clock),
            (FeedKind::Csv, Duration::from_millis(500), Clock::Event)
        );
        let c = columns(&m).unwrap();
        assert_eq!(c.price, None, "bid and ask without a price column");
        let m = merge_args(&["--feed", "jsonl", "--symbol", "MINE", "--delimiter", ";"]);
        let c = columns(&m).unwrap();
        assert_eq!(
            (
                c.symbol,
                c.fixed_symbol.as_deref(),
                c.price.as_deref(),
                c.delimiter
            ),
            (None, Some("MINE"), Some("price"), b';')
        );
        assert!(
            parse(&["merge", "--map", "m", "--feed", "csv"]).is_err(),
            "a source is required"
        );
        assert!(
            parse(&[
                "merge",
                "--map",
                "m",
                "--feed",
                "dbn",
                "--url",
                "ws://a",
                "--recording",
                "r"
            ])
            .is_err(),
            "not both"
        );
        assert!(
            parse(&[
                "merge",
                "--map",
                "m",
                "--feed",
                "csv",
                "--url",
                "ws://a",
                "--bid-col",
                "b"
            ])
            .is_err(),
            "bid needs ask"
        );
        assert!(matches!(
            parse(&["check-map", "--map", "m.toml"]).unwrap(),
            ByoCommand::CheckMap { .. }
        ));
    }

    #[test]
    fn outputs_are_checked_before_anything_runs() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("arch/_manifests")).unwrap();
        let e = output_sink(&d.path().join("arch/out.jsonl")).unwrap_err();
        assert!(e.to_string().contains("ADR 0014"), "{e}");
        assert!(output_sink(&d.path().join("ok.jsonl")).is_ok());
        assert!(output_sink(Path::new("-")).is_ok());
    }

    #[tokio::test]
    async fn merge_refuses_a_public_bind_address() {
        let d = tempfile::tempdir().unwrap();
        let map = d.path().join("m.toml");
        std::fs::write(
            &map,
            "[[pair]]\nname = \"p\"\nuser_symbol = \"S\"\n[pair.bleh]\nmarket = \"hyperliquid:perp:BTC\"\n",
        )
        .unwrap();
        let mut m = merge_args(&[
            "--feed",
            "csv",
            "--feed-path",
            "/nonexistent",
            "--ws",
            "0.0.0.0:9100",
        ]);
        m.map = map;
        m.no_merged = true;
        let e = merge(m).await.unwrap_err();
        assert!(e.to_string().contains("loopback"), "{e}");
    }

    #[cfg(not(feature = "databento-live"))]
    #[test]
    fn the_live_feed_needs_its_feature() {
        let m = merge_args(&["--feed", "databento"]);
        let map = Mapping::parse(
            "[[pair]]\nname = \"p\"\nuser_symbol = \"S\"\n[pair.bleh]\nmarket = \"hyperliquid:perp:BTC\"\n",
        )
        .unwrap();
        let e = user_source(&m, &map).unwrap_err();
        assert!(e.to_string().contains("databento-live"), "{e}");
    }
}
