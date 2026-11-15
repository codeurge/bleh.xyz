//! Web snapshots: small JSON files a static site can fetch
//! ([web-snapshot-v0](../../../docs/specs/web-snapshot-v0.md), ADR 0016).
//!
//! The job reads the daily record files that pollers write
//! (`<records-dir>/date=YYYY-MM-DD/{reference_value,positioning,swap_print}.jsonl`)
//! incrementally, keeps a bounded summary in memory, and writes:
//!
//! ```text
//! latest.json                      index: sources, notices, files
//! reference/<source>.json          latest value per series; daily history
//! positioning/<source>/<report>.json  latest and previous report per market
//! swaps/<source>.json              daily print counts and recent prints
//! ```
//!
//! Every file except `latest.json` changes only when its data changes, so an
//! unchanged file is neither rewritten nor uploaded. `latest.json` is
//! written last, so a reader never sees it point at a file that is not
//! there yet. Values are carried as the records give them (decimal strings,
//! source field names); nothing here derives a new value except swap print
//! counts.
//!
//! Files are appended to by the pollers. A file that shrinks makes the job
//! forget everything and read every file again on the next pass.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use mesh_types::{Record, SourceNotice};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::error::{IoContext, Result};
use crate::store::Store;

/// The `format` field of every snapshot file.
pub const FORMAT: &str = "bleh-web-snapshot/v0";

/// Record files read from each day directory.
pub const RECORD_FILES: [&str; 3] = [
    "reference_value.jsonl",
    "positioning.jsonl",
    "swap_print.jsonl",
];

/// `Cache-Control` for uploaded snapshot files. The edge rule for `/web/`
/// overrides it on the project's own domain.
pub const CACHE_CONTROL: &str = "public, max-age=60";

/// Options for a [`Snapshotter`].
#[derive(Debug, Clone)]
pub struct SnapshotOptions {
    /// Directories holding `date=YYYY-MM-DD/<file>` and, optionally,
    /// `_notices.json`.
    pub records_dirs: Vec<PathBuf>,
    /// Daily history points kept per reference series.
    pub points: usize,
    /// Recent swap prints kept per source and asset class.
    pub recent_swaps: usize,
    /// Days of swap print counts kept per source.
    pub swap_days: usize,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            records_dirs: Vec::new(),
            points: 260,
            recent_swaps: 50,
            swap_days: 14,
        }
    }
}

/// What one [`Snapshotter::ingest`] pass read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestReport {
    /// Records applied.
    pub records: u64,
    /// Lines that did not parse as records (skipped and logged).
    pub bad_lines: u64,
    /// True when a shrunken file made the pass start from scratch.
    pub reset: bool,
}

/// One snapshot file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    /// Path relative to the snapshot root.
    pub path: String,
    /// File contents.
    pub bytes: Vec<u8>,
}

impl SnapshotFile {
    fn sha256(&self) -> String {
        hex_digest(&self.bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Point {
    as_of_date: String,
    period_start_ms: Option<i64>,
    period_end_ms: Option<i64>,
    value: String,
    revision: u32,
}

impl Point {
    fn key(&self) -> (&str, i64) {
        (&self.as_of_date, self.period_start_ms.unwrap_or(i64::MIN))
    }

    /// True if `self` should replace `old` as the series' latest value:
    /// a later date or interval, or a revision of the same one.
    fn supersedes(&self, old: &Self) -> bool {
        match self.key().cmp(&old.key()) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => self.revision >= old.revision,
            std::cmp::Ordering::Less => false,
        }
    }
}

#[derive(Debug, Default)]
struct Series {
    meta: Map<String, Value>,
    intraday: bool,
    latest: Option<Point>,
    /// Daily series only: `as_of_date` -> (revision, value).
    points: BTreeMap<String, (u32, String)>,
}

#[derive(Debug, Clone)]
struct PosRow {
    long: u64,
    short: u64,
    spreading: Option<u64>,
    open_interest: u64,
    revision: u32,
}

#[derive(Debug, Default)]
struct PosMarket {
    flags: Value,
    /// `as_of_date` -> category -> row; the two latest dates are kept.
    dates: BTreeMap<String, BTreeMap<String, PosRow>>,
}

#[derive(Debug, Default)]
struct SwapSource {
    /// (UTC day, asset class) -> prints.
    counts: BTreeMap<(String, String), u64>,
    /// Asset class -> recent prints (full records), oldest first.
    recent: BTreeMap<String, VecDeque<(i64, Value)>>,
}

#[derive(Debug, Default)]
struct SourceState {
    admission: String,
    kinds: BTreeSet<&'static str>,
    records: u64,
    last_record_ms: i64,
    latest_as_of: Option<String>,
}

/// Markets of one report family, by (contract market code, combined).
type Markets = BTreeMap<(String, bool), PosMarket>;

/// Incremental builder of web snapshots.
#[derive(Debug)]
pub struct Snapshotter {
    opts: SnapshotOptions,
    offsets: HashMap<PathBuf, u64>,
    sources: BTreeMap<String, SourceState>,
    /// source -> series key -> series.
    reference: BTreeMap<String, BTreeMap<String, Series>>,
    /// source -> report -> (code, combined) -> market.
    positioning: BTreeMap<String, BTreeMap<String, Markets>>,
    swaps: BTreeMap<String, SwapSource>,
}

impl Snapshotter {
    /// A snapshotter that has read nothing yet.
    #[must_use]
    pub fn new(opts: SnapshotOptions) -> Self {
        Self {
            opts,
            offsets: HashMap::new(),
            sources: BTreeMap::new(),
            reference: BTreeMap::new(),
            positioning: BTreeMap::new(),
            swaps: BTreeMap::new(),
        }
    }

    fn forget(&mut self) {
        self.offsets.clear();
        self.sources.clear();
        self.reference.clear();
        self.positioning.clear();
        self.swaps.clear();
    }

    fn record_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for dir in &self.opts.records_dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).at(dir),
            };
            let mut days = Vec::new();
            for e in entries {
                let e = e.at(dir)?;
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("date=") && e.path().is_dir() {
                    days.push(e.path());
                }
            }
            days.sort();
            for day in days {
                for f in RECORD_FILES {
                    let p = day.join(f);
                    if p.is_file() {
                        files.push(p);
                    }
                }
            }
        }
        Ok(files)
    }

    /// Reads every complete line appended since the last pass.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors. Lines that are not records are counted and
    /// skipped.
    pub fn ingest(&mut self) -> Result<IngestReport> {
        let files = self.record_files()?;
        let shrunk = files.iter().any(|p| {
            let seen = self.offsets.get(p).copied().unwrap_or(0);
            std::fs::metadata(p).is_ok_and(|m| m.len() < seen)
        });
        let mut report = IngestReport::default();
        if shrunk {
            tracing::warn!("a record file shrank; reading every file again");
            self.forget();
            report.reset = true;
        }
        for path in files {
            self.ingest_file(&path, &mut report)?;
        }
        Ok(report)
    }

    fn ingest_file(&mut self, path: &Path, report: &mut IngestReport) -> Result<()> {
        let mut offset = self.offsets.get(path).copied().unwrap_or(0);
        let mut f = std::fs::File::open(path).at(path)?;
        if f.metadata().at(path)?.len() == offset {
            return Ok(());
        }
        f.seek(SeekFrom::Start(offset)).at(path)?;
        let mut reader = BufReader::with_capacity(1 << 20, f);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader.read_until(b'\n', &mut line).at(path)?;
            if n == 0 || line.last() != Some(&b'\n') {
                // End of file, or a line still being written.
                break;
            }
            offset += n as u64;
            let text = &line[..n - 1];
            if text.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            match serde_json::from_slice::<Record>(text) {
                Ok(r) => {
                    self.apply(&r);
                    report.records += 1;
                }
                Err(e) => {
                    report.bad_lines += 1;
                    if report.bad_lines <= 5 {
                        tracing::warn!(path = %path.display(), error = %e, "skipping a line that is not a record");
                    }
                }
            }
        }
        self.offsets.insert(path.to_path_buf(), offset);
        Ok(())
    }

    fn apply(&mut self, r: &Record) {
        let (kind, as_of) = match r {
            Record::ReferenceValue(v) => ("reference_value", Some(v.as_of_date.clone())),
            Record::Positioning(p) => ("positioning", Some(p.as_of_date.clone())),
            Record::SwapPrint(_) => ("swap_print", None),
            _ => return,
        };
        let prov = r.prov();
        let source = prov.venue.as_str().to_owned();
        let ts = prov.ts_source_ms.0;
        let s = self.sources.entry(source.clone()).or_default();
        prov.admission.as_str().clone_into(&mut s.admission);
        s.kinds.insert(kind);
        s.records += 1;
        s.last_record_ms = s.last_record_ms.max(ts);
        let as_of = as_of.unwrap_or_else(|| day_of_ms(ts));
        if s.latest_as_of.as_ref().is_none_or(|d| as_of > *d) {
            s.latest_as_of = Some(as_of);
        }
        match r {
            Record::ReferenceValue(v) => self.apply_reference(&source, v),
            Record::Positioning(p) => self.apply_positioning(&source, p),
            Record::SwapPrint(sp) => self.apply_swap(&source, ts, sp, r),
            _ => {}
        }
    }

    fn apply_reference(&mut self, source: &str, v: &mesh_types::ReferenceValue) {
        let measure = enum_name(&v.measure);
        let key = format!(
            "{}|{}|{}|{}",
            v.prov.instrument.as_str(),
            measure,
            v.tenor.as_deref().unwrap_or(""),
            v.maturity_date.as_deref().unwrap_or("")
        );
        let point = Point {
            as_of_date: v.as_of_date.clone(),
            period_start_ms: v.period_start_ms.map(|t| t.0),
            period_end_ms: v.period_end_ms.map(|t| t.0),
            value: v.value.to_string(),
            revision: v.revision,
        };
        let series = self
            .reference
            .entry(source.to_owned())
            .or_default()
            .entry(key)
            .or_default();
        if point.period_start_ms.is_some() {
            series.intraday = true;
            series.points.clear();
        }
        let newest = series
            .latest
            .as_ref()
            .is_none_or(|old| point.supersedes(old));
        if newest {
            let mut meta = Map::new();
            meta.insert("instrument".into(), json!(v.prov.instrument.as_str()));
            meta.insert("measure".into(), json!(measure));
            insert_some(&mut meta, "tenor", v.tenor.as_ref().map(|t| json!(t)));
            insert_some(
                &mut meta,
                "maturity_date",
                v.maturity_date.as_ref().map(|d| json!(d)),
            );
            insert_some(
                &mut meta,
                "day_count",
                v.day_count.as_ref().map(enum_name).map(Value::String),
            );
            insert_some(
                &mut meta,
                "compounding",
                v.compounding.as_ref().map(enum_name).map(Value::String),
            );
            insert_some(&mut meta, "currency", v.currency.as_ref().map(|c| json!(c)));
            insert_some(&mut meta, "unit", v.unit.map(|u| json!(u.as_str())));
            meta.insert(
                "flags".into(),
                serde_json::to_value(v.prov.flags).unwrap_or(Value::Null),
            );
            series.meta = meta;
        }
        if !series.intraday {
            let slot = series.points.entry(point.as_of_date.clone());
            match slot {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert((point.revision, point.value.clone()));
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    if point.revision >= e.get().0 {
                        e.insert((point.revision, point.value.clone()));
                    }
                }
            }
            while series.points.len() > self.opts.points {
                series.points.pop_first();
            }
        }
        if newest {
            series.latest = Some(point);
        }
    }

    fn apply_positioning(&mut self, source: &str, p: &mesh_types::Positioning) {
        let market = self
            .positioning
            .entry(source.to_owned())
            .or_default()
            .entry(enum_name(&p.report))
            .or_default()
            .entry((p.contract_market_code.clone(), p.combined))
            .or_default();
        if let Some(oldest) = market.dates.keys().next()
            && market.dates.len() >= 2
            && p.as_of_date < *oldest
        {
            return;
        }
        let row = PosRow {
            long: p.long,
            short: p.short,
            spreading: p.spreading,
            open_interest: p.open_interest,
            revision: p.revision,
        };
        let day = market.dates.entry(p.as_of_date.clone()).or_default();
        let category = enum_name(&p.category);
        if day
            .get(&category)
            .is_none_or(|old| row.revision >= old.revision)
        {
            day.insert(category, row);
        }
        while market.dates.len() > 2 {
            market.dates.pop_first();
        }
        if market.dates.keys().next_back() == Some(&p.as_of_date) {
            market.flags = serde_json::to_value(p.prov.flags).unwrap_or(Value::Null);
        }
    }

    fn apply_swap(&mut self, source: &str, ts: i64, sp: &mesh_types::SwapPrint, r: &Record) {
        let class = enum_name(&sp.asset_class);
        let src = self.swaps.entry(source.to_owned()).or_default();
        *src.counts
            .entry((day_of_ms(ts), class.clone()))
            .or_default() += 1;
        let days: BTreeSet<String> = src.counts.keys().map(|(d, _)| d.clone()).collect();
        if days.len() > self.opts.swap_days {
            let keep: BTreeSet<String> = days.into_iter().rev().take(self.opts.swap_days).collect();
            src.counts.retain(|(d, _), _| keep.contains(d));
        }
        let recent = src.recent.entry(class).or_default();
        let value = serde_json::to_value(r).unwrap_or(Value::Null);
        let pos = recent.partition_point(|(t, _)| *t <= ts);
        recent.insert(pos, (ts, value));
        while recent.len() > self.opts.recent_swaps {
            recent.pop_front();
        }
    }

    /// Renders every snapshot file, `latest.json` last.
    ///
    /// # Errors
    ///
    /// Fails if the notices files are unreadable or JSON encoding fails.
    pub fn render(&self, generated_ms: i64) -> Result<Vec<SnapshotFile>> {
        let notices = self.notices()?;
        let mut files = Vec::new();
        let mut per_source: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut series_count: BTreeMap<&str, usize> = BTreeMap::new();

        for (source, series) in &self.reference {
            let path = format!("reference/{source}.json");
            let list: Vec<Value> = series.values().filter_map(render_series).collect();
            series_count.insert(source, list.len());
            let mut body = self.header(source, &notices);
            body.insert("series".into(), Value::Array(list));
            files.push(file(&path, &Value::Object(body))?);
            per_source.entry(source).or_default().push(path);
        }
        for (source, reports) in &self.positioning {
            for (report, markets) in reports {
                let path = format!("positioning/{source}/{report}.json");
                let list: Vec<Value> = markets
                    .iter()
                    .filter_map(|((code, combined), m)| render_market(code, *combined, m))
                    .collect();
                *series_count.entry(source).or_default() += list.len();
                let mut body = self.header(source, &notices);
                body.insert("report".into(), json!(report));
                body.insert("markets".into(), Value::Array(list));
                files.push(file(&path, &Value::Object(body))?);
                per_source.entry(source).or_default().push(path);
            }
        }
        for (source, s) in &self.swaps {
            let path = format!("swaps/{source}.json");
            let counts: Vec<Value> = s
                .counts
                .iter()
                .map(|((day, class), n)| json!({"date": day, "asset_class": class, "prints": n}))
                .collect();
            let recent: Map<String, Value> = s
                .recent
                .iter()
                .map(|(class, prints)| {
                    let list = prints.iter().rev().map(|(_, v)| v.clone()).collect();
                    (class.clone(), Value::Array(list))
                })
                .collect();
            let mut body = self.header(source, &notices);
            body.insert("daily_counts".into(), Value::Array(counts));
            body.insert("recent".into(), Value::Object(recent));
            files.push(file(&path, &Value::Object(body))?);
            per_source.entry(source).or_default().push(path);
        }

        let sources: Vec<Value> = self
            .sources
            .iter()
            .map(|(name, s)| {
                let mut o = Map::new();
                o.insert("source".into(), json!(name));
                o.insert("admission".into(), json!(s.admission));
                insert_some(
                    &mut o,
                    "notice",
                    notices.get(name.as_str()).map(|n| json!(n)),
                );
                o.insert("kinds".into(), json!(s.kinds));
                o.insert("records".into(), json!(s.records));
                o.insert(
                    "series".into(),
                    json!(series_count.get(name.as_str()).copied().unwrap_or(0)),
                );
                insert_some(
                    &mut o,
                    "latest_as_of",
                    s.latest_as_of.as_ref().map(|d| json!(d)),
                );
                o.insert("last_record_ms".into(), json!(s.last_record_ms));
                o.insert(
                    "files".into(),
                    json!(per_source.get(name.as_str()).cloned().unwrap_or_default()),
                );
                Value::Object(o)
            })
            .collect();
        let index: Vec<Value> = files
            .iter()
            .map(|f| json!({"path": f.path, "bytes": f.bytes.len(), "sha256": f.sha256()}))
            .collect();
        let latest = json!({
            "format": FORMAT,
            "generated_ms": generated_ms,
            "sources": sources,
            "files": index,
        });
        files.push(file("latest.json", &latest)?);
        Ok(files)
    }

    fn header(&self, source: &str, notices: &BTreeMap<String, String>) -> Map<String, Value> {
        let s = self.sources.get(source);
        let mut o = Map::new();
        o.insert("format".into(), json!(FORMAT));
        o.insert("source".into(), json!(source));
        o.insert("admission".into(), json!(s.map(|s| s.admission.as_str())));
        insert_some(&mut o, "notice", notices.get(source).map(|n| json!(n)));
        o.insert(
            "last_record_ms".into(),
            json!(s.map_or(0, |s| s.last_record_ms)),
        );
        o
    }

    fn notices(&self) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for dir in &self.opts.records_dirs {
            let p = dir.join("_notices.json");
            let bytes = match std::fs::read(&p) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e).at(&p),
            };
            let list: Vec<SourceNotice> = serde_json::from_slice(&bytes)
                .map_err(|e| crate::error::Error::invalid(format!("{}: {e}", p.display())))?;
            for n in list {
                out.insert(n.source.as_str().to_owned(), n.notice);
            }
        }
        Ok(out)
    }
}

fn render_series(s: &Series) -> Option<Value> {
    let latest = s.latest.as_ref()?;
    let mut o = s.meta.clone();
    o.insert("intraday".into(), json!(s.intraday));
    let mut l = Map::new();
    l.insert("as_of_date".into(), json!(latest.as_of_date));
    insert_some(
        &mut l,
        "period_start_ms",
        latest.period_start_ms.map(|t| json!(t)),
    );
    insert_some(
        &mut l,
        "period_end_ms",
        latest.period_end_ms.map(|t| json!(t)),
    );
    l.insert("value".into(), json!(latest.value));
    l.insert("revision".into(), json!(latest.revision));
    o.insert("latest".into(), Value::Object(l));
    if !s.intraday {
        let points: Vec<Value> = s.points.iter().map(|(d, (_, v))| json!([d, v])).collect();
        o.insert("points".into(), Value::Array(points));
    }
    Some(Value::Object(o))
}

fn render_report(date: &str, rows: &BTreeMap<String, PosRow>) -> Value {
    let open_interest = rows.values().map(|r| r.open_interest).max().unwrap_or(0);
    let categories: Map<String, Value> = rows
        .iter()
        .map(|(c, r)| {
            let mut o = Map::new();
            o.insert("long".into(), json!(r.long));
            o.insert("short".into(), json!(r.short));
            insert_some(&mut o, "spreading", r.spreading.map(|s| json!(s)));
            (c.clone(), Value::Object(o))
        })
        .collect();
    json!({"as_of_date": date, "open_interest": open_interest, "categories": categories})
}

fn render_market(code: &str, combined: bool, m: &PosMarket) -> Option<Value> {
    let mut it = m.dates.iter().rev();
    let (date, rows) = it.next()?;
    let mut o = Map::new();
    o.insert("contract_market_code".into(), json!(code));
    o.insert("combined".into(), json!(combined));
    o.insert("flags".into(), m.flags.clone());
    if let Value::Object(latest) = render_report(date, rows) {
        o.extend(latest);
    }
    if let Some((d, r)) = it.next() {
        o.insert("previous".into(), render_report(d, r));
    }
    Some(Value::Object(o))
}

fn file(path: &str, v: &Value) -> Result<SnapshotFile> {
    let mut bytes = serde_json::to_vec(v)
        .map_err(|e| crate::error::Error::invalid(format!("encoding {path}: {e}")))?;
    bytes.push(b'\n');
    Ok(SnapshotFile {
        path: path.to_owned(),
        bytes,
    })
}

fn insert_some(o: &mut Map<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        o.insert(k.to_owned(), v);
    }
}

/// The serde name of a unit enum variant (`"rate"`, `"managed_money"`).
fn enum_name<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string(),
        Err(_) => String::new(),
    }
}

fn day_of_ms(ms: i64) -> String {
    crate::layout::Day::from_millis(ms).to_string()
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Writes changed files under `dir` (atomically, `latest.json` last) and
/// remembers their digests in `written`. Returns how many were written.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn write_local(
    dir: &Path,
    files: &[SnapshotFile],
    written: &mut HashMap<String, String>,
) -> Result<usize> {
    let mut n = 0;
    for f in files {
        let sha = f.sha256();
        if written.get(&f.path) == Some(&sha) {
            continue;
        }
        let path = dir.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &f.bytes).at(&tmp)?;
        std::fs::rename(&tmp, &path).at(&path)?;
        written.insert(f.path.clone(), sha);
        n += 1;
    }
    Ok(n)
}

/// Uploads changed files to `store` (mutable objects, `latest.json` last)
/// and remembers their digests in `uploaded`. Returns how many were sent.
///
/// # Errors
///
/// Fails on storage errors; files sent before the error stay remembered.
pub async fn upload(
    store: &Store,
    files: &[SnapshotFile],
    uploaded: &mut HashMap<String, String>,
) -> Result<usize> {
    let mut n = 0;
    for f in files {
        let sha = f.sha256();
        if uploaded.get(&f.path) == Some(&sha) {
            continue;
        }
        store
            .put_mutable_with(
                &f.path,
                Bytes::from(f.bytes.clone()),
                "application/json",
                CACHE_CONTROL,
            )
            .await?;
        uploaded.insert(f.path.clone(), sha);
        n += 1;
    }
    crate::metrics::inc("archive_web_snapshot_uploaded_files_total", &[], n as u64);
    Ok(n)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// State kept between passes of [`run_once`].
#[derive(Debug)]
pub struct Runner {
    /// The incremental builder.
    pub snap: Snapshotter,
    /// Local output directory.
    pub out_dir: PathBuf,
    /// Upload target, if any.
    pub store: Option<Store>,
    written: HashMap<String, String>,
    uploaded: HashMap<String, String>,
}

impl Runner {
    /// A runner writing to `out_dir` and, if given, uploading to `store`.
    #[must_use]
    pub fn new(opts: SnapshotOptions, out_dir: PathBuf, store: Option<Store>) -> Self {
        Self {
            snap: Snapshotter::new(opts),
            out_dir,
            store,
            written: HashMap::new(),
            uploaded: HashMap::new(),
        }
    }

    /// Reads new records, renders, writes changed files and uploads them.
    /// Returns (records read, files written, files uploaded).
    ///
    /// # Errors
    ///
    /// Fails on I/O or storage errors.
    pub async fn run_once(&mut self) -> Result<(u64, usize, usize)> {
        let ingest = self.snap.ingest()?;
        if ingest.bad_lines > 0 {
            crate::metrics::inc(
                "archive_web_snapshot_bad_lines_total",
                &[],
                ingest.bad_lines,
            );
        }
        let files = self.snap.render(now_ms())?;
        let written = write_local(&self.out_dir, &files, &mut self.written)?;
        let sent = match &self.store {
            Some(s) => upload(s, &files, &mut self.uploaded).await?,
            None => 0,
        };
        Ok((ingest.records, written, sent))
    }
}

/// Runs passes every `interval` until SIGTERM or Ctrl-C.
///
/// # Errors
///
/// Never returns an error from a pass (it is logged and retried); fails
/// only if signal handling cannot be set up.
pub async fn follow(runner: &mut Runner, interval: Duration) -> Result<()> {
    loop {
        match runner.run_once().await {
            Ok((records, written, uploaded)) => {
                tracing::info!(records, written, uploaded, "snapshot pass done");
            }
            Err(e) => {
                tracing::error!(error = %e, "snapshot pass failed; retrying next interval");
                crate::metrics::inc("archive_web_snapshot_errors_total", &[], 1);
            }
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            _ = tokio::signal::ctrl_c() => return Ok(()),
            () = crate::partition_dir::sigterm() => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic records only: invented sources (example-*), admissions in the
    // ADM-9xxx test range and made-up values.

    fn reference(day: &str, ts: i64, value: &str, revision: u32) -> String {
        format!(
            r#"{{"kind":"reference_value","prov":{{"instrument":"example-rates:ref:OVERNIGHT","venue":"example-rates","ts_source_ms":{ts},"ts_ingest_ns":"{ts}000000","seq":{{"block_number":1,"event_index":0}},"admission":"ADM-9002","flags":[]}},"measure":"rate","value":"{value}","tenor":"ON","maturity_date":null,"day_count":"act_360","compounding":"simple","currency":null,"as_of_date":"{day}","revision":{revision}}}"#
        )
    }

    fn interval(day: &str, start: i64, value: &str) -> String {
        format!(
            r#"{{"kind":"reference_value","prov":{{"instrument":"example-grid:ref:NODE1","venue":"example-grid","ts_source_ms":{start},"ts_ingest_ns":"{start}000000","seq":{{"block_number":1,"event_index":0}},"admission":"ADM-9004","flags":[]}},"measure":"price","value":"{value}","tenor":null,"maturity_date":null,"day_count":null,"compounding":null,"currency":"USD","as_of_date":"{day}","revision":0,"period_start_ms":{start},"period_end_ms":{end}}}"#,
            end = start + 900_000
        )
    }

    fn positioning(day: &str, category: &str, long: u64) -> String {
        format!(
            r#"{{"kind":"positioning","prov":{{"instrument":"example-cot:positioning:000001","venue":"example-cot","ts_source_ms":1767700000000,"ts_ingest_ns":"1767700000000000000","seq":{{"block_number":1,"event_index":0}},"admission":"ADM-9001","flags":[]}},"report":"disaggregated","combined":false,"contract_market_code":"000001","as_of_date":"{day}","category":"{category}","long":{long},"short":800,"spreading":150,"open_interest":10000,"revision":0}}"#
        )
    }

    fn swap(id: u32, ts: i64) -> String {
        format!(
            r#"{{"kind":"swap_print","prov":{{"instrument":"example-swaps:swap:EXAMPLE-OIS.1M","venue":"example-swaps","ts_source_ms":{ts},"ts_ingest_ns":"{ts}000000","seq":{{"block_number":1,"event_index":{id}}},"admission":"ADM-9003","flags":[]}},"repository":"sdr-example","dissemination_id":"SYN-{id}","action":"new","asset_class":"interest_rate","product":"Synthetic:Example","underlier":"EXAMPLE-OIS","tenor":"1M","price":{{"type":"rate","value":"4.0800"}},"notional":{{"amount":"10000000","currency":"USD"}},"event_type":"trade","ts_execution_ms":{ts},"ts_dissemination_ms":{ts}}}"#
        )
    }

    fn append(dir: &Path, day: &str, file: &str, lines: &[String]) {
        use std::io::Write;
        let d = dir.join(format!("date={day}"));
        std::fs::create_dir_all(&d).unwrap();
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(d.join(file))
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    fn get(files: &[SnapshotFile], path: &str) -> Value {
        let f = files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("{path} missing"));
        serde_json::from_slice(&f.bytes).unwrap()
    }

    fn snap(dir: &Path) -> Snapshotter {
        Snapshotter::new(SnapshotOptions {
            records_dirs: vec![dir.to_path_buf()],
            points: 2,
            recent_swaps: 2,
            swap_days: 14,
        })
    }

    #[test]
    fn reference_latest_revisions_and_bounded_history() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        append(
            d,
            "2026-01-05",
            "reference_value.jsonl",
            &[
                reference("2026-01-02", 1_767_600_000_000, "4.10", 0),
                reference("2026-01-05", 1_767_610_000_000, "4.20", 0),
            ],
        );
        append(
            d,
            "2026-01-06",
            "reference_value.jsonl",
            &[
                reference("2026-01-05", 1_767_700_000_000, "4.25", 1),
                reference("2026-01-01", 1_767_700_000_001, "4.00", 0),
                reference("2026-01-06", 1_767_700_000_002, "4.30", 0),
            ],
        );
        let mut s = snap(d);
        let r = s.ingest().unwrap();
        assert_eq!(r.records, 5);
        let files = s.render(1).unwrap();
        assert_eq!(files.last().unwrap().path, "latest.json");
        let v = get(&files, "reference/example-rates.json");
        let series = &v["series"][0];
        assert_eq!(series["instrument"], "example-rates:ref:OVERNIGHT");
        assert_eq!(series["latest"]["as_of_date"], "2026-01-06");
        assert_eq!(series["latest"]["value"], "4.30");
        // Two points kept; the revision of 2026-01-05 wins.
        assert_eq!(
            series["points"],
            json!([["2026-01-05", "4.25"], ["2026-01-06", "4.30"]])
        );
        assert!(series.get("maturity_date").is_none(), "nulls are omitted");
        assert_eq!(v["admission"], "ADM-9002");
    }

    #[test]
    fn quantity_series_carry_their_unit() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let stock = r#"{"kind":"reference_value","prov":{"instrument":"example-energy:ref:STOCKS.W","venue":"example-energy","ts_source_ms":1767700000000,"ts_ingest_ns":"1767700000000000000","seq":{"block_number":1,"event_index":0},"admission":"ADM-9005","flags":[]},"measure":"stock","value":"421234","tenor":null,"maturity_date":null,"day_count":null,"compounding":null,"currency":null,"as_of_date":"2026-01-02","revision":0,"unit":"thousand_barrels"}"#;
        append(
            d,
            "2026-01-06",
            "reference_value.jsonl",
            &[stock.to_owned()],
        );
        append(
            d,
            "2026-01-06",
            "reference_value.jsonl",
            &[reference("2026-01-06", 1_767_700_000_002, "4.30", 0)],
        );
        let mut s = snap(d);
        s.ingest().unwrap();
        let files = s.render(1).unwrap();
        let series = &get(&files, "reference/example-energy.json")["series"][0];
        assert_eq!(series["measure"], "stock");
        assert_eq!(series["unit"], "thousand_barrels");
        assert!(series.get("currency").is_none());
        assert_eq!(series["latest"]["value"], "421234");
        let rate = &get(&files, "reference/example-rates.json")["series"][0];
        assert!(rate.get("unit").is_none(), "no unit on a rate");
    }

    #[test]
    fn intraday_series_keep_only_the_latest_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let base = 1_767_657_600_000;
        append(
            d,
            "2026-01-06",
            "reference_value.jsonl",
            &[
                interval("2026-01-06", base + 900_000, "31.5"),
                interval("2026-01-06", base, "30.0"),
            ],
        );
        let mut s = snap(d);
        s.ingest().unwrap();
        let v = get(&s.render(1).unwrap(), "reference/example-grid.json");
        let series = &v["series"][0];
        assert_eq!(series["intraday"], true);
        assert!(series.get("points").is_none());
        assert_eq!(series["latest"]["value"], "31.5");
        assert_eq!(series["latest"]["period_start_ms"], base + 900_000);
    }

    #[test]
    fn incremental_reads_skip_partial_lines_and_reset_on_shrink() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        append(
            d,
            "2026-01-06",
            "reference_value.jsonl",
            &[reference("2026-01-06", 1, "4.30", 0)],
        );
        let path = d.join("date=2026-01-06/reference_value.jsonl");
        let partial = reference("2026-01-07", 2, "4.40", 0);
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(&partial.as_bytes()[..40]).unwrap();
        }
        let mut s = snap(d);
        assert_eq!(s.ingest().unwrap().records, 1);
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "{}", &partial[40..]).unwrap();
        }
        assert_eq!(s.ingest().unwrap().records, 1);
        assert_eq!(s.ingest().unwrap().records, 0);
        let v = get(&s.render(1).unwrap(), "reference/example-rates.json");
        assert_eq!(v["series"][0]["latest"]["value"], "4.40");

        std::fs::write(
            &path,
            format!("{}\n", reference("2026-01-06", 1, "4.30", 0)),
        )
        .unwrap();
        let r = s.ingest().unwrap();
        assert!(r.reset);
        assert_eq!(r.records, 1);
    }

    #[test]
    fn positioning_keeps_latest_and_previous_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        append(
            d,
            "2026-01-09",
            "positioning.jsonl",
            &[
                positioning("2025-12-30", "managed_money", 900),
                positioning("2026-01-06", "managed_money", 1200),
                positioning("2026-01-06", "swap_dealer", 300),
                positioning("2026-01-13", "managed_money", 1300),
            ],
        );
        let mut s = snap(d);
        s.ingest().unwrap();
        let files = s.render(1).unwrap();
        let v = get(&files, "positioning/example-cot/disaggregated.json");
        let m = &v["markets"][0];
        assert_eq!(m["contract_market_code"], "000001");
        assert_eq!(m["as_of_date"], "2026-01-13");
        assert_eq!(m["categories"]["managed_money"]["long"], 1300);
        assert_eq!(m["previous"]["as_of_date"], "2026-01-06");
        assert_eq!(m["previous"]["categories"]["swap_dealer"]["long"], 300);
        let latest = get(&files, "latest.json");
        assert_eq!(latest["sources"][0]["latest_as_of"], "2026-01-13");
        assert_eq!(latest["sources"][0]["series"], 1);
    }

    #[test]
    fn swaps_count_per_day_and_keep_recent_prints() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let day = 1_767_657_600_000;
        append(
            d,
            "2026-01-06",
            "swap_print.jsonl",
            &[swap(1, day + 1), swap(3, day + 3), swap(2, day + 2)],
        );
        let mut s = snap(d);
        s.ingest().unwrap();
        let v = get(&s.render(1).unwrap(), "swaps/example-swaps.json");
        assert_eq!(
            v["daily_counts"],
            json!([{"date": "2026-01-06", "asset_class": "interest_rate", "prints": 3}])
        );
        let recent = v["recent"]["interest_rate"].as_array().unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0]["dissemination_id"], "SYN-3", "newest first");
        assert_eq!(recent[1]["dissemination_id"], "SYN-2");
    }

    #[test]
    fn notices_reach_the_index_and_source_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        append(
            d,
            "2026-01-06",
            "reference_value.jsonl",
            &[reference("2026-01-06", 1, "4.30", 0)],
        );
        std::fs::write(
            d.join("_notices.json"),
            r#"[{"admission":"ADM-9002","source":"example-rates","notice":"Source: Example Rates Office."}]"#,
        )
        .unwrap();
        let mut s = snap(d);
        s.ingest().unwrap();
        let files = s.render(7).unwrap();
        let latest = get(&files, "latest.json");
        assert_eq!(latest["format"], FORMAT);
        assert_eq!(latest["generated_ms"], 7);
        assert_eq!(
            latest["sources"][0]["notice"],
            "Source: Example Rates Office."
        );
        assert_eq!(
            latest["sources"][0]["files"],
            json!(["reference/example-rates.json"])
        );
        assert_eq!(latest["files"][0]["path"], "reference/example-rates.json");
        let v = get(&files, "reference/example-rates.json");
        assert_eq!(v["notice"], "Source: Example Rates Office.");
    }

    #[tokio::test]
    async fn unchanged_files_are_not_rewritten_or_uploaded() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("records");
        append(
            &d,
            "2026-01-06",
            "reference_value.jsonl",
            &[reference("2026-01-06", 1, "4.30", 0)],
        );
        let store = Store::memory();
        let mut runner = Runner::new(
            SnapshotOptions {
                records_dirs: vec![d.clone()],
                ..SnapshotOptions::default()
            },
            tmp.path().join("out"),
            Some(store.clone()),
        );
        assert_eq!(runner.run_once().await.unwrap(), (1, 2, 2));
        // Only latest.json (new generated_ms) goes out again.
        let (_, written, sent) = runner.run_once().await.unwrap();
        assert!(written <= 1 && sent <= 1, "{written} {sent}");
        append(
            &d,
            "2026-01-07",
            "reference_value.jsonl",
            &[reference("2026-01-07", 2, "4.40", 0)],
        );
        let (records, _, sent) = runner.run_once().await.unwrap();
        assert_eq!(records, 1);
        assert!(sent >= 1);
        let body = store
            .get("reference/example-rates.json")
            .await
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["series"][0]["latest"]["value"], "4.40");
        assert!(tmp.path().join("out/latest.json").is_file());
    }
}
