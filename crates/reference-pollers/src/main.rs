//! `reference-pollers`: capture the admitted public reference sources as
//! raw files and `reference_value` / `positioning` records, live and from
//! history.
//!
//! ```text
//! reference-pollers [run]                       # poll continuously
//! reference-pollers once                        # one cycle, then exit
//! reference-pollers backfill --archive-root DIR # history into partitions
//! reference-pollers parse FILE                  # offline: records as JSON lines
//! reference-pollers catalog | notices           # registry entries, notices
//! ```
//!
//! See `docs/operations/reference-pollers.md`.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use reference_pollers::config::{Args, Command, run, run_backfill};
use reference_pollers::obs::{Builder, Stamp};
use reference_pollers::raw::RawStore;
use reference_pollers::revision::RevisionStore;
use reference_pollers::sources::{Target, reader};
use reference_pollers::{SourceId, catalog, source};
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

fn parse_offline(args: &Args, file: &std::path::Path, backfill: bool) -> anyhow::Result<()> {
    let (body, meta) = RawStore::load(file)?;
    let id: SourceId = meta.source.parse().map_err(anyhow::Error::msg)?;
    let r = reader(id, &args.bases()?);
    let target = Target {
        key: meta.target.clone(),
        url: meta.url.clone(),
        kind: meta.kind.clone(),
        once: false,
    };
    let parsed = r.parse(&target, &body).map_err(anyhow::Error::msg)?;
    let stamp = if backfill {
        Stamp::Backfill
    } else {
        Stamp::Live {
            fetched_at: meta
                .fetched_at()
                .ok_or_else(|| anyhow::anyhow!("sidecar has no fetch time"))?,
            last_modified_ms: meta.validators.last_modified_ms(),
        }
    };
    let (records, dups) =
        Builder::new(id).build(parsed.obs, stamp, &mut RevisionStore::memory())?;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for rec in &records {
        out.write_all(&mesh_types::hash::canonical_record(rec)?)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    eprintln!(
        "{}: records={} duplicates={dups} follow={} skipped={:?} unknown={:?}",
        file.display(),
        records.len(),
        parsed.follow.len(),
        parsed.skipped,
        parsed.unknown
    );
    Ok(())
}

fn print_json<T: serde::Serialize>(v: &T) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, v)?;
    out.write_all(b"\n")?;
    Ok(())
}

async fn real_main(args: Args) -> anyhow::Result<()> {
    match args.command.clone() {
        Some(Command::Parse { file, backfill }) => parse_offline(&args, &file, backfill),
        Some(Command::Catalog) => print_json(&catalog::entries()),
        Some(Command::Notices) => print_json(&source::all_notices(&args.selected()?)),
        Some(Command::Backfill {
            archive_root,
            from_year,
            to_year,
        }) => {
            let reports = run_backfill(&args, archive_root, from_year, to_year).await?;
            let mut out = std::io::stdout().lock();
            for r in reports {
                serde_json::to_writer(&mut out, &r)?;
                out.write_all(b"\n")?;
            }
            Ok(())
        }
        Some(Command::Once) => run(&args, true, None, watch::channel(false).1).await,
        Some(Command::Run) | None => {
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
            run(&args, false, None, rx).await
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    init_tracing(args.log_json);
    match real_main(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "reference-pollers failed");
            eprintln!("reference-pollers: {e:#}");
            ExitCode::FAILURE
        }
    }
}
