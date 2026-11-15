//! `hl-backfill`: copy, process, compare and replay Hyperliquid's public
//! archive buckets (ADM-0003). See `docs/operations/backfill.md`.
//!
//! Reports are JSON on stdout; logs go to stderr (`RUST_LOG` sets the
//! level). Credentials come only from the environment and are never
//! logged (see `hl_backfill::store`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use futures::StreamExt;

use anyhow::{Context, Result, bail};
use archive::layout::check_archive;
use clap::{Args, Parser, Subcommand};
use hl_backfill::buckets::{Bucket, Dataset, Day};
use hl_backfill::checkpoint::Checkpoint;
use hl_backfill::clock::TableClock;
use hl_backfill::copy::{Copier, CopyConfig};
use hl_backfill::estimate::{Estimate, Prices};
use hl_backfill::filter::{DefaultFilter, RawLineFilter};
use hl_backfill::listing::{Selection, SourceObject, list_dataset};
use hl_backfill::parse::DefaultParser;
use hl_backfill::partition::{BACKFILL_ARCHIVE, JsonlWriter};
use hl_backfill::process::{Pipeline, ProcessConfig};
use hl_backfill::registry::RegistryBuilder;
use hl_backfill::replay::cmds::parse_blocks;
use hl_backfill::replay::matcher::{AssetSpec, Matcher};
use hl_backfill::replay::{L2Checkpoint, levels_of, replay};
use hl_backfill::retry::RetryPolicy;
use hl_backfill::store::{StoreUrl, project_store, source_store};
use mesh_types::{InstrumentId, Record, Registry};
use object_store::prefix::PrefixStore;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(
    name = "hl-backfill",
    version,
    about = "Backfill from Hyperliquid's public archive buckets (ADM-0003)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the selection, report coverage and estimate requester-pays cost.
    Estimate {
        #[command(flatten)]
        sel: SelectionArgs,
        #[command(flatten)]
        regions: RegionArgs,
        /// USD per GiB transferred (0.09 to the internet; 0 from compute in
        /// the bucket's region).
        #[arg(long, default_value_t = 0.09)]
        transfer_usd_per_gib: f64,
        /// USD per 1,000 GET requests.
        #[arg(long, default_value_t = 0.0004)]
        get_usd_per_1000: f64,
        /// USD per 1,000 LIST requests.
        #[arg(long, default_value_t = 0.005)]
        list_usd_per_1000: f64,
    },
    /// Copy the selection verbatim into the private raw tier.
    Copy {
        #[command(flatten)]
        sel: SelectionArgs,
        #[command(flatten)]
        regions: RegionArgs,
        /// Raw tier: s3://bucket[/prefix] (credentials from BLEH_RAW_*) or
        /// file:///dir.
        #[arg(long)]
        raw: String,
        /// Checkpoint file (JSON lines; also the copy log).
        #[arg(long)]
        checkpoint: PathBuf,
        /// Objects copied at once.
        #[arg(long, default_value_t = 16)]
        concurrency: usize,
        /// Attempts per object.
        #[arg(long, default_value_t = 6)]
        max_attempts: u32,
        /// List and plan only.
        #[arg(long)]
        dry_run: bool,
    },
    /// Filter and normalize raw-tier objects into filtered raw and
    /// partitions (the node reader's filter and parser for by-block fills,
    /// Parquet partitions from the archive crate's writer).
    Process {
        #[command(flatten)]
        sel: SelectionArgs,
        /// Raw tier written by `copy`.
        #[arg(long)]
        raw: String,
        /// Filtered raw output: s3://bucket[/prefix] or file:///dir.
        #[arg(long)]
        filtered: String,
        /// Directory holding archives; partitions go to
        /// `<out-dir>/<archive>/kind=.../part-NNNNN.parquet`.
        #[arg(long)]
        out_dir: PathBuf,
        /// Archive name of the partitions (archive-layout-v0 § 1).
        #[arg(long, default_value = BACKFILL_ARCHIVE)]
        archive: String,
        /// Partition format: Parquet (published) or canonical JSON lines
        /// (`records.jsonl` per partition, for debugging).
        #[arg(long, value_enum, default_value_t = OutputFormat::Parquet)]
        format: OutputFormat,
        /// Instrument registry JSON (`{"instruments": [...]}`).
        #[arg(long)]
        registry: PathBuf,
        /// Optional file of block lines (`block_time`, `block_number`) used
        /// to resolve blocks for data without them.
        #[arg(long)]
        clock_lines: Option<PathBuf>,
    },
    /// Derive an instrument registry from the admitted fills in the raw
    /// tier (ADR 0019, 0021, 0022; method hl-backfill-registry/v3). Writes the registry
    /// document `process --registry` reads; the report goes to stdout.
    Registry {
        #[command(flatten)]
        sel: SelectionArgs,
        /// Raw tier written by `copy`: s3://bucket[/prefix] or file:///dir.
        #[arg(long)]
        raw: String,
        /// Where to write the registry document (JSON).
        #[arg(long)]
        out: PathBuf,
        /// Objects read and parsed at once. The result does not depend on it.
        #[arg(long, default_value_t = 8)]
        concurrency: usize,
    },
    /// Compare backfilled records with live-recorded records per hour.
    Compare {
        /// Backfilled records: a JSON-lines or Parquet file, or a directory of them.
        #[arg(long)]
        backfill: PathBuf,
        /// Live records: a JSON-lines or Parquet file, or a directory of them.
        #[arg(long)]
        live: PathBuf,
    },
    /// Replay simplified replica_cmds blocks through the emulated matcher.
    ReplayPrototype {
        /// JSON lines of blocks (`{block_number, block_time_ms, actions}`).
        #[arg(long)]
        blocks: PathBuf,
        /// JSON list of `{asset, instrument, price_scale, size_scale}`.
        #[arg(long)]
        assets: PathBuf,
        /// Optional L2 `book_snapshot` records (as written by `process`)
        /// to check the emulated book against.
        #[arg(long)]
        checkpoints: Option<PathBuf>,
        /// Levels per side in the archived snapshots.
        #[arg(long, default_value_t = 20)]
        archive_depth: usize,
        /// Where to write the replayed records (JSON lines).
        #[arg(long)]
        out: Option<PathBuf>,
        /// First generated order ID (when responses are absent).
        #[arg(long, default_value_t = 1)]
        first_oid: u64,
    },
}

/// Partition output format of `process`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    /// Deterministic Parquet partitions (archive-layout-v0 § 3).
    Parquet,
    /// Canonical JSON lines under the same Hive paths.
    Jsonl,
}

#[derive(Debug, Args)]
struct SelectionArgs {
    /// Datasets, comma-separated (default: all admitted).
    #[arg(long, value_delimiter = ',')]
    datasets: Vec<String>,
    /// First day (YYYY-MM-DD or YYYYMMDD), inclusive.
    #[arg(long)]
    from: Option<String>,
    /// Last day, inclusive.
    #[arg(long)]
    to: Option<String>,
    /// First block height (height-keyed datasets).
    #[arg(long)]
    from_height: Option<u64>,
    /// Last block height.
    #[arg(long)]
    to_height: Option<u64>,
    /// Coins to keep (market_data), comma-separated.
    #[arg(long, value_delimiter = ',')]
    coins: Vec<String>,
    /// Work split across hosts, as INDEX/COUNT (whole days per host).
    #[arg(long)]
    shard: Option<String>,
}

#[derive(Debug, Args)]
struct RegionArgs {
    /// Region of hl-mainnet-node-data (find it with
    /// `curl -sI https://hl-mainnet-node-data.s3.amazonaws.com`).
    #[arg(long, env = "HL_NODE_DATA_REGION")]
    node_data_region: String,
    /// Region of hyperliquid-archive.
    #[arg(long, env = "HL_ARCHIVE_REGION")]
    archive_region: String,
}

impl SelectionArgs {
    fn datasets(&self) -> Result<Vec<Dataset>> {
        if self.datasets.is_empty() {
            return Ok(Dataset::ALL.to_vec());
        }
        self.datasets
            .iter()
            .map(|d| {
                Dataset::from_name(d).with_context(|| format!("{d} is not an admitted dataset"))
            })
            .collect()
    }

    fn selection(&self) -> Result<Selection> {
        let day = |s: &Option<String>| s.as_deref().map(Day::parse).transpose();
        let shard = match &self.shard {
            None => None,
            Some(s) => {
                let (i, n) = s.split_once('/').context("--shard must be INDEX/COUNT")?;
                let (i, n): (u64, u64) = (i.parse()?, n.parse()?);
                if n == 0 || i >= n {
                    bail!("--shard index must be below count");
                }
                Some((i, n))
            }
        };
        Ok(Selection {
            from: day(&self.from)?,
            to: day(&self.to)?,
            from_height: self.from_height,
            to_height: self.to_height,
            coins: self.coins.clone(),
            shard,
        })
    }
}

impl RegionArgs {
    fn sources(&self) -> Result<BTreeMap<Bucket, Arc<dyn ObjectStore>>> {
        let mut m = BTreeMap::new();
        m.insert(
            Bucket::NodeData,
            source_store(Bucket::NodeData, &self.node_data_region)?,
        );
        m.insert(
            Bucket::Archive,
            source_store(Bucket::Archive, &self.archive_region)?,
        );
        Ok(m)
    }
}

/// The raw tier, listed as if it were the buckets.
fn raw_views(
    raw_url: &StoreUrl,
    raw_store: &Arc<dyn ObjectStore>,
) -> BTreeMap<Bucket, Arc<dyn ObjectStore>> {
    let mut views: BTreeMap<Bucket, Arc<dyn ObjectStore>> = BTreeMap::new();
    for b in Bucket::ALL {
        let prefix = [raw_url.prefix(), b.name()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("/");
        views.insert(
            b,
            Arc::new(PrefixStore::new(Arc::clone(raw_store), prefix.as_str())),
        );
    }
    views
}

async fn list_selection(
    stores: &BTreeMap<Bucket, Arc<dyn ObjectStore>>,
    datasets: &[Dataset],
    sel: &Selection,
    mut each: impl FnMut(Dataset, &hl_backfill::listing::Listed),
) -> Result<Vec<SourceObject>> {
    let mut all = Vec::new();
    for d in datasets {
        let store = stores
            .get(&d.bucket())
            .with_context(|| format!("no store for {}", d.bucket()))?;
        let listed = list_dataset(store.as_ref(), *d, sel).await?;
        for (key, why) in &listed.skipped {
            tracing::warn!(%key, reason = %why, "skipped key");
        }
        each(*d, &listed);
        all.extend(listed.objects);
    }
    Ok(all)
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetRow {
    asset: u32,
    instrument: InstrumentId,
    price_scale: u8,
    size_scale: u8,
}

async fn run(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Estimate {
            sel,
            regions,
            transfer_usd_per_gib,
            get_usd_per_1000,
            list_usd_per_1000,
        } => {
            let mut est = Estimate::new(Prices {
                transfer_usd_per_gib,
                get_usd_per_1000,
                list_usd_per_1000,
            });
            list_selection(
                &regions.sources()?,
                &sel.datasets()?,
                &sel.selection()?,
                |d, l| est.add(d, l),
            )
            .await?;
            print_json(&est)?;
            Ok(true)
        }
        Command::Copy {
            sel,
            regions,
            raw,
            checkpoint,
            concurrency,
            max_attempts,
            dry_run,
        } => {
            let url = StoreUrl::parse(&raw)?;
            let sources = regions.sources()?;
            let objects =
                list_selection(&sources, &sel.datasets()?, &sel.selection()?, |_, _| {}).await?;
            let copier = Copier::new(
                sources,
                project_store(&url)?,
                CopyConfig {
                    concurrency,
                    retry: RetryPolicy {
                        max_attempts,
                        ..RetryPolicy::default()
                    },
                    dest_prefix: url.prefix().to_owned(),
                    dry_run,
                    ..CopyConfig::default()
                },
            );
            let mut ckpt = Checkpoint::open(&checkpoint)?;
            let report = copier.copy_all(&objects, &mut ckpt).await?;
            print_json(&report)?;
            Ok(report.failed.is_empty())
        }
        Command::Registry {
            sel,
            raw,
            out,
            concurrency,
        } => {
            let datasets: Vec<Dataset> = sel
                .datasets()?
                .into_iter()
                .filter(|d| {
                    matches!(
                        d,
                        Dataset::NodeFillsByBlock | Dataset::NodeFills | Dataset::NodeTrades
                    )
                })
                .collect();
            if datasets.is_empty() {
                bail!("registry reads node_fills_by_block, node_fills or node_trades");
            }
            let raw_url = StoreUrl::parse(&raw)?;
            let raw_store = project_store(&raw_url)?;
            let views = raw_views(&raw_url, &raw_store);
            let objects = list_selection(&views, &datasets, &sel.selection()?, |_, _| {}).await?;
            let admission = mesh_types::AdmissionId::new(hl_backfill::ADMISSION)?;
            let mut builder = RegistryBuilder::new(admission.clone());
            // Each object is fetched, then decompressed, filtered and
            // observed on a blocking thread into its own builder; the
            // builders merge order-independently (RegistryBuilder::merge).
            let total = objects.len();
            let mut done = 0usize;
            let mut parts = futures::stream::iter(objects.iter().map(|obj| {
                let raw_store = Arc::clone(&raw_store);
                let admission = admission.clone();
                let key = hl_backfill::copy::dest_key(raw_url.prefix(), &obj.bucket, &obj.key);
                async move {
                    let mut one = RegistryBuilder::new(admission.clone());
                    let result = async {
                        let bytes = raw_store.get(&key?).await?.bytes().await?;
                        let (dataset, day) = (obj.dataset, obj.info.day);
                        tokio::task::spawn_blocking(move || {
                            let text = hl_backfill::codec::lz4_to_string(
                                &bytes,
                                hl_backfill::codec::DEFAULT_MAX_DECOMPRESSED,
                            )?;
                            let lines: Vec<&str> =
                                text.lines().filter(|l| !l.trim().is_empty()).collect();
                            let kept = DefaultFilter::default()
                                .filter_lines(dataset, &lines)?
                                .lines;
                            let mut part = RegistryBuilder::new(admission);
                            part.observe(dataset, day, &kept);
                            Ok::<_, anyhow::Error>(part)
                        })
                        .await?
                    }
                    .await;
                    match result {
                        Ok(part) => one.merge(part),
                        Err(e) => {
                            tracing::error!(key = %obj.key, error = %e, "registry: object failed");
                            one.failed(&obj.key, e.to_string());
                        }
                    }
                    one
                }
            }))
            .buffer_unordered(concurrency.max(1));
            while let Some(one) = parts.next().await {
                builder.merge(one);
                done += 1;
                if done.is_multiple_of(500) || done == total {
                    tracing::info!(done, total, "registry: objects read");
                }
            }
            let (registry, report) = builder.build();
            std::fs::write(&out, serde_json::to_vec_pretty(&registry)?)
                .with_context(|| format!("writing {}", out.display()))?;
            let clean = report.clean();
            print_json(&report)?;
            Ok(clean)
        }
        Command::Process {
            sel,
            raw,
            filtered,
            out_dir,
            archive,
            format,
            registry,
            clock_lines,
        } => {
            check_archive(&archive)?;
            let datasets: Vec<Dataset> = sel
                .datasets()?
                .into_iter()
                .filter(|d| d.is_record_source())
                .collect();
            let raw_url = StoreUrl::parse(&raw)?;
            let raw_store = project_store(&raw_url)?;
            let filtered_url = StoreUrl::parse(&filtered)?;
            let filtered_store = project_store(&filtered_url)?;
            let registry: Registry = serde_json::from_slice(
                &std::fs::read(&registry)
                    .with_context(|| format!("reading {}", registry.display()))?,
            )?;
            let views = raw_views(&raw_url, &raw_store);
            let selection = sel.selection()?;
            let mut objects = list_selection(&views, &datasets, &selection, |_, _| {}).await?;
            // This run owns the days of its objects and writes only their
            // partitions. A day's last records can arrive in the next day's
            // first hourly file, so that file is read too when another run
            // owns its day (ParquetWriter::set_owned_days).
            let owned_days: std::collections::BTreeSet<_> =
                objects.iter().filter_map(|o| o.info.day).collect();
            let spill_days: std::collections::BTreeSet<_> = owned_days
                .iter()
                .map(|d| d.succ())
                .filter(|d| !owned_days.contains(d))
                .collect();
            if let (Some(first), Some(last)) = (spill_days.first(), spill_days.last()) {
                let wider = Selection {
                    from: Some(*first),
                    to: Some(*last),
                    shard: None,
                    ..selection.clone()
                };
                let extra = list_selection(&views, &datasets, &wider, |_, _| {}).await?;
                objects.extend(extra.into_iter().filter(|o| {
                    o.info.hour == Some(0) && o.info.day.is_some_and(|d| spill_days.contains(&d))
                }));
            }
            let external = match clock_lines {
                Some(p) => {
                    let text = std::fs::read_to_string(&p)
                        .with_context(|| format!("reading {}", p.display()))?;
                    let mut c = TableClock::new([], TableClock::DEFAULT_MAX_GAP_MS);
                    c.extend_from_block_lines(text.lines());
                    Some(c)
                }
                None => None,
            };
            let cfg = ProcessConfig {
                raw_prefix: raw_url.prefix().to_owned(),
                filtered_root: filtered_url.prefix().to_owned(),
                ..ProcessConfig::default()
            };
            let archive_root = out_dir.join(&archive);
            let mut outside_owned_days = 0u64;
            let (report, partitions) = match format {
                OutputFormat::Parquet => {
                    let mut pipeline = Pipeline::wired(
                        raw_store.as_ref(),
                        filtered_store.as_ref(),
                        cfg,
                        registry,
                        &archive_root,
                    );
                    pipeline.writer.set_owned_days(owned_days.clone());
                    if let Some(c) = &external {
                        pipeline = pipeline.with_external_clock(c);
                    }
                    let report = pipeline.run(&objects).await?;
                    outside_owned_days = pipeline.writer.outside_owned_days;
                    (report, Some(pipeline.writer.files))
                }
                OutputFormat::Jsonl => {
                    let mut pipeline = Pipeline::new(
                        raw_store.as_ref(),
                        filtered_store.as_ref(),
                        cfg,
                        DefaultFilter::default(),
                        DefaultParser::new(registry),
                        JsonlWriter::new(&archive_root),
                    );
                    if let Some(c) = &external {
                        pipeline = pipeline.with_external_clock(c);
                    }
                    (pipeline.run(&objects).await?, None)
                }
            };
            let complete = report.hours.iter().all(|h| h.complete());
            let mut json = serde_json::to_value(&report)?;
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "owned_days".into(),
                    serde_json::to_value(owned_days.iter().map(|d| d.iso()).collect::<Vec<_>>())?,
                );
                obj.insert("outside_owned_days".into(), outside_owned_days.into());
            }
            if let (Some(files), Some(obj)) = (partitions, json.as_object_mut()) {
                obj.insert("partitions".into(), serde_json::to_value(files)?);
            }
            print_json(&json)?;
            Ok(complete)
        }
        Command::Compare { backfill, live } => {
            let b = hl_backfill::compare::load_records(&backfill)?;
            let l = hl_backfill::compare::load_records(&live)?;
            let report = hl_backfill::compare::compare(&b, &l)?;
            print_json(&report)?;
            Ok(report.matches())
        }
        Command::ReplayPrototype {
            blocks,
            assets,
            checkpoints,
            archive_depth,
            out,
            first_oid,
        } => {
            let rows: Vec<AssetRow> = serde_json::from_slice(&std::fs::read(&assets)?)?;
            let table: BTreeMap<u32, AssetSpec> = rows
                .into_iter()
                .map(|r| {
                    (
                        r.asset,
                        AssetSpec {
                            instrument: r.instrument,
                            price_scale: r.price_scale,
                            size_scale: r.size_scale,
                        },
                    )
                })
                .collect();
            let text = std::fs::read_to_string(&blocks)?;
            let blocks = parse_blocks(&text)
                .map_err(|(line, e)| anyhow::anyhow!("blocks line {line}: {e}"))?;
            let mut cps = Vec::new();
            if let Some(path) = checkpoints {
                for r in hl_backfill::compare::load_records(&path)? {
                    let Record::BookSnapshot(s) = r else { continue };
                    let Some((asset, spec)) = table
                        .iter()
                        .find(|(_, a)| a.instrument == s.prov.instrument)
                    else {
                        continue;
                    };
                    cps.push(L2Checkpoint {
                        asset: *asset,
                        block_number: s.prov.seq.block_number,
                        levels: levels_of(&s.depth, spec).map_err(anyhow::Error::msg)?,
                    });
                }
            }
            let mut matcher = Matcher::new(table, first_oid);
            let (records, report) =
                replay(&mut matcher, &blocks, &cps, archive_depth).map_err(anyhow::Error::msg)?;
            if let Some(out) = out {
                let mut buf = Vec::new();
                for r in &records {
                    buf.extend(mesh_types::hash::canonical_record(r)?);
                    buf.push(b'\n');
                }
                std::fs::write(&out, buf).with_context(|| format!("writing {}", out.display()))?;
            }
            print_json(&report)?;
            Ok(report.checkpoints.iter().all(|c| c.matched))
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    match run(Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("hl-backfill: {e:#}");
            ExitCode::from(2)
        }
    }
}
