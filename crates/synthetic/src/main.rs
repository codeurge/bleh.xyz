//! `synthetic`: reads input records (JSON lines) and writes
//! `synthetic_month` records (JSON lines) on a valuation grid.
//!
//! ```text
//! synthetic --registry registry.json --input inputs.jsonl [--input more.jsonl]
//!           [--step-secs 60] [--from-ms T] [--to-ms T] [--output out.jsonl]
//! ```

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use synthetic::batch::{Grid, read_records, run};
use synthetic::{Engine, Registry};

/// Synthetic contract-month fair values (synthetic-months@1).
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Exposure registry (JSON).
    #[arg(long)]
    registry: PathBuf,
    /// Input records, JSON lines (repeatable).
    #[arg(long, required = true)]
    input: Vec<PathBuf>,
    /// Seconds between valuations.
    #[arg(long, default_value_t = 60)]
    step_secs: u32,
    /// First valuation time, Unix ms (default: first input, rounded up).
    #[arg(long)]
    from_ms: Option<i64>,
    /// Last valuation time, Unix ms (default: last input).
    #[arg(long)]
    to_ms: Option<i64>,
    /// Output file (default: standard output).
    #[arg(long)]
    output: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let registry = Registry::load(&args.registry)?;
    let mut inputs = Vec::new();
    for p in &args.input {
        let f = File::open(p).with_context(|| p.display().to_string())?;
        inputs.extend(read_records(BufReader::new(f)).with_context(|| p.display().to_string())?);
    }
    let mut engine = Engine::new(registry);
    let out = run(
        &mut engine,
        inputs,
        Grid {
            step_ms: i64::from(args.step_secs) * 1000,
            from_ms: args.from_ms,
            to_ms: args.to_ms,
        },
    )?;
    let mut w: Box<dyn Write> = match &args.output {
        Some(p) => Box::new(BufWriter::new(
            File::create(p).with_context(|| p.display().to_string())?,
        )),
        None => Box::new(BufWriter::new(std::io::stdout().lock())),
    };
    for r in out.records {
        serde_json::to_writer(&mut w, &mesh_types::Record::from(r))?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    for n in out.notices {
        eprintln!("notice: {n}");
    }
    Ok(())
}
