//! `swap-pollers`: capture CFTC Part 43 and SEC SBSR public swap
//! dissemination as raw files and `SwapPrint` records.
//!
//! ```text
//! swap-pollers                      # poll continuously
//! swap-pollers --once               # one cycle, then exit
//! swap-pollers --parse FILE.zip     # offline: print records as JSON lines
//! ```
//!
//! See `docs/operations/swap-pollers.md`.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use mesh_types::{Record, UnixNanos};
use swap_pollers::config::{Args, run};
use swap_pollers::listing::batch_from_name;
use swap_pollers::parse::{FileContext, ReportKind, SinkClosed, parse_report_file};
use swap_pollers::raw::RawMeta;
use swap_pollers::repository::{DtccBucket, Repository};
use tokio::sync::watch;

fn init_tracing(json: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

/// Offline parse: records to stdout as canonical JSON lines, a summary to
/// stderr.
fn parse_offline(args: &Args) -> anyhow::Result<()> {
    let Some(path) = &args.parse else {
        anyhow::bail!("--parse needs a file");
    };
    let meta = RawMeta::load_for(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bucket = args
        .bucket
        .or_else(|| {
            meta.as_ref()
                .and_then(|m| m.bucket.parse::<DtccBucket>().ok())
        })
        .or_else(|| DtccBucket::from_file_name(&name))
        .ok_or_else(|| anyhow::anyhow!("cannot tell the bucket from {name:?}; pass --bucket"))?;
    let kind = meta.as_ref().map_or_else(
        || {
            if name.contains("_CUMULATIVE_") {
                ReportKind::Cumulative
            } else {
                ReportKind::Slice
            }
        },
        |m| m.kind,
    );
    let ctx = FileContext {
        bucket,
        repository: Repository::DtccDdr,
        kind,
        batch: meta
            .as_ref()
            .map_or_else(|| batch_from_name(&name), |m| m.entry.batch()),
        fetched_at: meta
            .as_ref()
            .and_then(|m| m.fetched_at_ns.parse().ok())
            .map(UnixNanos),
        listing_dissemination: meta.as_ref().and_then(|m| m.entry.dissemination()),
        backfill: kind == ReportKind::Cumulative,
    };
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut sink = |p| -> Result<(), SinkClosed> {
        let line =
            mesh_types::hash::canonical_record(&Record::SwapPrint(p)).map_err(|_| SinkClosed)?;
        out.write_all(&line).map_err(|_| SinkClosed)?;
        out.write_all(b"\n").map_err(|_| SinkClosed)
    };
    let stats = parse_report_file(path, &ctx, &mut sink)?;
    out.flush()?;
    eprintln!(
        "{}: rows={} records={} skipped={:?} row_errors={} field_errors={:?} unknown_columns={:?} dissemination_fallbacks={:?}",
        name,
        stats.rows,
        stats.records,
        stats.skipped,
        stats.row_errors,
        stats.field_errors,
        stats.unknown_columns,
        stats.dissemination_fallbacks
    );
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(args.log_json);
    if args.parse.is_some() {
        return match parse_offline(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("swap-pollers: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            match signal(SignalKind::terminate()) {
                Ok(mut term) => {
                    tokio::select! {
                        _ = ctrl_c => {}
                        _ = term.recv() => {}
                    }
                }
                Err(_) => {
                    let _ = ctrl_c.await;
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ctrl_c.await;
        }
        let _ = tx.send(true);
    });
    match run(&args, None, rx).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "swap-pollers failed");
            ExitCode::FAILURE
        }
    }
}
