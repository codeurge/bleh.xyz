//! History backfill into an archive root, through the archive crate's
//! deterministic partition writer (archive-layout-v0 § 3), as
//! `hl-backfill` does.
//!
//! A backfill runs per source, one **unit** per calendar year from the
//! source's earliest admitted date. A unit fetches the reader's targets
//! for the year (files that hold many years are fetched once per run and
//! reused), keeps values dated in that year, and builds records with the
//! `backfill` flag, an estimated publication time and `ts_ingest_ns` equal
//! to it, so the output is a pure function of the files read.
//!
//! Partitions are daily by publication time, and a value can be published
//! after the end of its year (a December report released in January) or
//! before its date (day-ahead prices). So a unit writes only the
//! partitions no later unit can add to (days before the next year's start
//! minus the source's lead), and carries the rest, as canonical JSON
//! lines in the state directory, into the next unit. Each partition is
//! therefore written exactly once, complete.
//!
//! - **Idempotent.** The partition writer accepts an identical existing
//!   file and refuses a different one, so re-running a year, or the whole
//!   backfill from fresh state, reproduces the same files.
//! - **Resumable.** The state directory records the finished years, the
//!   carry and the revision state; a run skips finished years and resumes
//!   with the first unfinished one. The archive root gets `_notices.json`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use archive::partition::{PartitionOptions, WrittenFile, record_day, write_partitions};
use bytes::Bytes;
use mesh_types::Record;
use serde::{Deserialize, Serialize};

use crate::http::Fetcher;
use crate::metrics::Metrics;
use crate::obs::{Builder, Stamp};
use crate::raw::write_atomic_sync;
use crate::revision::RevisionStore;
use crate::source::SourceId;
use crate::sources::{Parsed, Reader, Target};
use crate::time::Date;

/// Default name of the backfill archive (a separate archive with its own
/// manifest chain, archive-layout-v0 § 1).
pub const BACKFILL_ARCHIVE: &str = "reference-backfill";

/// Durable backfill progress for one source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Years finished.
    #[serde(default)]
    pub done: BTreeSet<i32>,
}

/// One unit's outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UnitReport {
    /// Source.
    pub source: String,
    /// Year.
    pub year: i32,
    /// Records built for the year.
    pub records: usize,
    /// Partition files written (new).
    pub written: usize,
    /// Partition files already present and identical.
    pub already_present: usize,
    /// Records carried to the next unit.
    pub carried: usize,
    /// Skipped because already finished.
    pub skipped: bool,
}

/// A function from URL to body (`None` is a 404).
pub type FetchFn = Arc<dyn Fn(&str) -> Option<Bytes> + Send + Sync>;

/// Where a body comes from: the network, or a function for tests.
#[derive(Clone)]
pub enum Fetch {
    /// HTTP through the shared fetcher.
    Http(Fetcher),
    /// A function from URL to body.
    Map(FetchFn),
}

impl std::fmt::Debug for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetch::Http(_) => f.write_str("Fetch::Http"),
            Fetch::Map(_) => f.write_str("Fetch::Map"),
        }
    }
}

impl Fetch {
    async fn get(&self, url: &str, source: &str) -> anyhow::Result<Option<Bytes>> {
        match self {
            Fetch::Http(f) => match f.get(url, source).await {
                Ok((b, _)) => Ok(Some(b)),
                Err(e) if e.is_not_found() => Ok(None),
                Err(e) => Err(e.into()),
            },
            Fetch::Map(m) => Ok(m(url)),
        }
    }
}

/// Backfill settings.
#[derive(Debug, Clone)]
pub struct Options {
    /// Archive root the partitions go under.
    pub archive_root: PathBuf,
    /// State directory (per source subdirectories).
    pub state_dir: PathBuf,
    /// First year (clamped to the source's earliest).
    pub from_year: Option<i32>,
    /// Last year.
    pub to_year: i32,
    /// The current year: carried records are written once the backfill
    /// reaches it.
    pub current_year: i32,
}

struct Paths {
    checkpoint: PathBuf,
    carry: PathBuf,
    revisions: PathBuf,
}

fn paths(state_dir: &Path, source: SourceId) -> Paths {
    let dir = state_dir.join(source.name()).join("backfill");
    Paths {
        checkpoint: dir.join("checkpoint.json"),
        carry: dir.join("carry.jsonl"),
        revisions: dir.join("revisions"),
    }
}

fn load_carry(path: &Path) -> anyhow::Result<Vec<Record>> {
    match std::fs::read(path) {
        Ok(b) => b
            .split(|c| *c == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).map_err(Into::into))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

fn save_carry(path: &Path, records: &[Record]) -> anyhow::Result<()> {
    let mut body = Vec::new();
    for r in records {
        body.extend_from_slice(&mesh_types::hash::canonical_record(r)?);
        body.push(b'\n');
    }
    write_atomic_sync(path, &body)?;
    Ok(())
}

/// Backfills one source over the configured years.
///
/// # Errors
///
/// Fails on a fetch, parse, build or write error; finished years stay
/// finished and the next run resumes after them.
pub async fn run_source(
    reader: &dyn Reader,
    fetch: &Fetch,
    opts: &Options,
    metrics: &Metrics,
) -> anyhow::Result<Vec<UnitReport>> {
    let id = reader.id();
    let src = id.name();
    let Some(earliest) = id.earliest() else {
        tracing::info!(
            source = src,
            "no admitted history channel; nothing to backfill"
        );
        return Ok(Vec::new());
    };
    std::fs::create_dir_all(&opts.archive_root)?;
    merge_notices(&opts.archive_root, id)?;
    let p = paths(&opts.state_dir, id);
    let mut checkpoint: Checkpoint = match std::fs::read(&p.checkpoint) {
        Ok(b) => serde_json::from_slice(&b)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Checkpoint::default(),
        Err(e) => return Err(e.into()),
    };
    let today = Date::new(opts.current_year, 12, 31).unwrap_or(earliest);
    let mut store = RevisionStore::open(&p.revisions, 1_000_000, today)?;
    let mut carry = load_carry(&p.carry)?;
    let builder = Builder::new(id);
    // Recent bodies by URL: files holding many years are fetched once.
    let mut cache: VecDeque<(String, Bytes)> = VecDeque::new();
    let mut reports = Vec::new();
    let from = opts.from_year.unwrap_or(earliest.y).max(earliest.y);
    for year in from..=opts.to_year {
        if checkpoint.done.contains(&year) {
            reports.push(UnitReport {
                source: src.into(),
                year,
                skipped: true,
                ..UnitReport::default()
            });
            continue;
        }
        let mut obs = Vec::new();
        let mut queue: VecDeque<Target> = reader.backfill_targets(year).into();
        while let Some(t) = queue.pop_front() {
            let cached = cache
                .iter()
                .find(|(u, _)| *u == t.url)
                .map(|(_, b)| b.clone());
            let body = match cached {
                Some(b) => Some(b),
                None => {
                    let b = fetch.get(&t.url, src).await?;
                    if let Some(b) = &b {
                        if cache.len() >= 8 {
                            cache.pop_front();
                        }
                        cache.push_back((t.url.clone(), b.clone()));
                    }
                    b
                }
            };
            let Some(body) = body else {
                tracing::info!(source = src, url = %t.url, "not found; skipped");
                continue;
            };
            let mut parsed: Parsed = reader
                .parse(&t, &body)
                .map_err(|e| anyhow::anyhow!("{src} {}: {e}", t.url))?;
            queue.extend(parsed.follow.drain(..));
            for f in &parsed.unknown {
                metrics.inc("ref_unknown_fields_total", &[("source", src), ("field", f)]);
            }
            obs.extend(parsed.obs.into_iter().filter(|o| o.as_of().y == year));
        }
        let (records, _) = builder.build(obs, Stamp::Backfill, &mut store)?;
        let n = records.len();
        carry.extend(records);
        let final_unit = year >= opts.current_year;
        let boundary = Date::new(year + 1, 1, 1)
            .map_or(i64::MAX, |d| d.add_days(-id.max_lead_days()).start_ms());
        let (ready, open): (Vec<Record>, Vec<Record>) = std::mem::take(&mut carry)
            .into_iter()
            .partition(|r| final_unit || record_day(r).start_ms() < boundary);
        let written = write_partitions(&opts.archive_root, ready, &PartitionOptions::default())?;
        carry = open;
        save_carry(&p.carry, &carry)?;
        store.flush(today)?;
        checkpoint.done.insert(year);
        write_atomic_sync(&p.checkpoint, &serde_json::to_vec_pretty(&checkpoint)?)?;
        metrics.inc(
            "ref_backfill_units_total",
            &[("source", src), ("outcome", "done")],
        );
        let report = unit_report(src, year, n, &written, carry.len());
        tracing::info!(
            source = src,
            year,
            records = n,
            written = report.written,
            carried = report.carried,
            "backfill unit done"
        );
        reports.push(report);
    }
    Ok(reports)
}

fn unit_report(
    src: &str,
    year: i32,
    records: usize,
    written: &[WrittenFile],
    carried: usize,
) -> UnitReport {
    UnitReport {
        source: src.into(),
        year,
        records,
        written: written.iter().filter(|w| !w.already_present).count(),
        already_present: written.iter().filter(|w| w.already_present).count(),
        carried,
        skipped: false,
    }
}

/// Adds `id`'s notices to the archive root's `_notices.json`, keeping the
/// notices of other sources already there.
fn merge_notices(root: &Path, id: SourceId) -> anyhow::Result<()> {
    let path = root.join("_notices.json");
    let mut all: BTreeMap<(String, String), mesh_types::SourceNotice> = BTreeMap::new();
    if let Ok(b) = std::fs::read(&path) {
        let existing: Vec<mesh_types::SourceNotice> =
            serde_json::from_slice(&b).unwrap_or_default();
        for n in existing {
            all.insert((n.admission.to_string(), n.source.to_string()), n);
        }
    }
    for n in id.notices() {
        all.insert((n.admission.to_string(), n.source.to_string()), n);
    }
    let v: Vec<_> = all.into_values().collect();
    let mut body = serde_json::to_vec_pretty(&v)?;
    body.push(b'\n');
    write_atomic_sync(&path, &body)?;
    Ok(())
}
