//! Typed queries over the archive: resolves partition files for a kind,
//! instrument and day range, then reads them with DuckDB.
//!
//! Precomputed kinds (bars, footprints, volume profiles) are read as
//! stored, never recomputed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use duckdb::types::Value;
use mesh_types::manifest::SignedManifest;
use mesh_types::{InstrumentId, RecordKind, Registry};

use crate::config::{ArchiveRoot, Config};
use crate::engine::Engine;
use crate::error::{ApiError, ApiResult, ErrorCode};
use crate::params::{DAY_MS, HOUR_MS, day_string};
use crate::rows;
use crate::source::{
    Layout, list_local_parquet, local_manifest_days, manifest_partition_files,
    parse_partition_path, sql_file_list, sql_literal, walk_local_archive,
};

const NEGATIVE_TTL: Duration = Duration::from_secs(60);
const INVENTORY_TTL: Duration = Duration::from_secs(60);
const MANIFEST_CACHE_CAP: usize = 8_192;

/// Manifests by (archive, date), with the time they were fetched.
type ManifestCache = HashMap<(String, String), (Instant, Option<Arc<SignedManifest>>)>;

/// Position after which a page starts: `(block_number, event_index)`.
pub type SeqKey = (u64, u32);

/// One instrument's presence in the archive.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Presence {
    /// Kinds with at least one partition.
    pub kinds: BTreeSet<String>,
    /// First and last partition dates.
    pub first_date: Option<String>,
    /// Last partition date.
    pub last_date: Option<String>,
    /// Archives holding it.
    pub archives: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct Inventory {
    at: Instant,
    by_instrument: BTreeMap<String, Presence>,
}

/// The store.
#[derive(Debug, Clone)]
pub struct Store {
    /// Engine.
    pub engine: Engine,
    /// Layout.
    pub layout: Layout,
    registry: Option<Arc<Registry>>,
    manifests: Arc<Mutex<ManifestCache>>,
    inventory: Arc<Mutex<Option<Inventory>>>,
}

impl Store {
    /// Builds a store; reads the registry file if configured.
    ///
    /// # Errors
    ///
    /// Fails if the registry file cannot be read or parsed.
    pub fn new(config: &Config, engine: Engine) -> anyhow::Result<Self> {
        let registry = match &config.registry_file {
            Some(p) => {
                let bytes = std::fs::read(p)
                    .map_err(|e| anyhow::anyhow!("registry {}: {e}", p.display()))?;
                let r: Registry = serde_json::from_slice(&bytes)
                    .map_err(|e| anyhow::anyhow!("registry {}: {e}", p.display()))?;
                Some(Arc::new(r))
            }
            None => None,
        };
        let root = match &config.root {
            ArchiveRoot::Local(p) => {
                ArchiveRoot::Local(std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            }
            other => other.clone(),
        };
        Ok(Self {
            engine,
            layout: Layout {
                root,
                archives: config.archives.clone(),
            },
            registry,
            manifests: Arc::default(),
            inventory: Arc::default(),
        })
    }

    /// The configured registry, if any.
    #[must_use]
    pub fn registry(&self) -> Option<&Registry> {
        self.registry.as_deref()
    }

    // -----------------------------------------------------------------
    // Manifests and file resolution
    // -----------------------------------------------------------------

    /// The signed manifest of one archive day, if published.
    ///
    /// # Errors
    ///
    /// `archive_unavailable` when a remote read fails for a reason other
    /// than absence.
    pub async fn manifest(
        &self,
        archive: &str,
        date: &str,
    ) -> ApiResult<Option<Arc<SignedManifest>>> {
        let key = (archive.to_owned(), date.to_owned());
        if let Ok(cache) = self.manifests.lock()
            && let Some((at, v)) = cache.get(&key)
            && (v.is_some() || at.elapsed() < NEGATIVE_TTL)
        {
            return Ok(v.clone());
        }
        let loc = self.layout.manifest_location(archive, date);
        let bytes: Option<Vec<u8>> = match &self.layout.root {
            ArchiveRoot::Local(_) => match tokio::fs::read(&loc).await {
                Ok(b) => Some(b),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(ApiError::internal(e)),
            },
            ArchiveRoot::ObjectStore(_) | ArchiveRoot::Https(_) => {
                let sql = format!("SELECT content FROM read_text({})", sql_literal(&loc));
                match self
                    .engine
                    .query(sql, vec![], |r| r.get::<_, String>(0))
                    .await
                {
                    Ok(mut rows) => rows.pop().map(String::into_bytes),
                    Err(e) if e.code == ErrorCode::ArchiveUnavailable => None,
                    Err(e) => return Err(e),
                }
            }
        };
        let parsed = match bytes {
            Some(b) => match serde_json::from_slice::<SignedManifest>(&b) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    tracing::warn!(archive, date, error = %e, "manifest does not parse");
                    None
                }
            },
            None => None,
        };
        if let Ok(mut cache) = self.manifests.lock() {
            if cache.len() >= MANIFEST_CACHE_CAP {
                cache.clear();
            }
            cache.insert(key, (Instant::now(), parsed.clone()));
        }
        Ok(parsed)
    }

    /// Days with a manifest in `[first_day, last_day]` for an archive.
    ///
    /// # Errors
    ///
    /// As [`Store::manifest`].
    pub async fn manifest_days(
        &self,
        archive: &str,
        first_day: i64,
        last_day: i64,
    ) -> ApiResult<Vec<String>> {
        match &self.layout.root {
            ArchiveRoot::Local(p) => {
                let (lo, hi) = (day_string(first_day), day_string(last_day));
                Ok(local_manifest_days(p, archive)
                    .into_iter()
                    .filter(|d| *d >= lo && *d <= hi)
                    .collect())
            }
            _ => {
                let mut out = Vec::new();
                for day in first_day..=last_day {
                    let d = day_string(day);
                    if self.manifest(archive, &d).await?.is_some() {
                        out.push(d);
                    }
                }
                Ok(out)
            }
        }
    }

    /// Parquet files of `kind` for `instrument` on the given UTC days,
    /// across all configured archives.
    ///
    /// # Errors
    ///
    /// `archive_unavailable` on remote failures.
    pub async fn files(
        &self,
        kind: RecordKind,
        instrument: &InstrumentId,
        days: &[i64],
    ) -> ApiResult<Vec<String>> {
        let mut out = Vec::new();
        match &self.layout.root {
            ArchiveRoot::Local(_) => {
                for archive in &self.layout.archives {
                    for day in days {
                        if let Some(dir) = self
                            .layout
                            .local_partition_dir(archive, kind, instrument, *day)
                        {
                            out.extend(list_local_parquet(&dir));
                        }
                    }
                }
            }
            ArchiveRoot::ObjectStore(_) => {
                let globs: Vec<String> = self
                    .layout
                    .archives
                    .iter()
                    .flat_map(|a| {
                        days.iter()
                            .map(move |d| self.layout.partition_glob(a, kind, instrument, *d))
                    })
                    .map(|g| format!("SELECT file FROM glob({})", sql_literal(&g)))
                    .collect();
                if globs.is_empty() {
                    return Ok(out);
                }
                let sql = format!("{} ORDER BY file", globs.join(" UNION ALL "));
                out = self
                    .engine
                    .query(sql, vec![], |r| r.get::<_, String>(0))
                    .await?;
            }
            ArchiveRoot::Https(_) => {
                for archive in &self.layout.archives {
                    let base = self.layout.archive_base(archive);
                    for day in days {
                        if let Some(m) = self.manifest(archive, &day_string(*day)).await? {
                            let dir = Layout::partition_dir(kind, instrument, *day);
                            out.extend(manifest_partition_files(&m, &base, &dir));
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// What the archive holds, per instrument (cached for a minute).
    ///
    /// Local roots are walked; object stores are globbed; HTTPS roots use
    /// the manifests of the last seven days.
    ///
    /// # Errors
    ///
    /// `archive_unavailable` on remote failures.
    pub async fn inventory(&self) -> ApiResult<BTreeMap<String, Presence>> {
        if let Ok(inv) = self.inventory.lock()
            && let Some(i) = inv.as_ref()
            && i.at.elapsed() < INVENTORY_TTL
        {
            return Ok(i.by_instrument.clone());
        }
        let mut keys = Vec::new();
        match &self.layout.root {
            ArchiveRoot::Local(p) => {
                for archive in &self.layout.archives {
                    keys.extend(walk_local_archive(p, archive).into_keys());
                }
            }
            ArchiveRoot::ObjectStore(_) => {
                for archive in &self.layout.archives {
                    let base = self.layout.archive_base(archive);
                    let pattern = format!("{base}/kind=*/venue=*/instrument=*/date=*/*.parquet");
                    let sql = format!("SELECT file FROM glob({})", sql_literal(&pattern));
                    let files = self
                        .engine
                        .query(sql, vec![], |r| r.get::<_, String>(0))
                        .await?;
                    for f in files {
                        if let Some(rel) = f.strip_prefix(&format!("{base}/"))
                            && let Some(k) =
                                parse_partition_path(archive, &format!("{archive}/{rel}"))
                        {
                            keys.push(k);
                        }
                    }
                }
            }
            ArchiveRoot::Https(_) => {
                let today = crate::params::DAY_MS;
                let now_day = mesh_types::UnixMillis::now().0.div_euclid(today);
                for archive in &self.layout.archives {
                    for day in (now_day - 7)..=now_day {
                        if let Some(m) = self.manifest(archive, &day_string(day)).await? {
                            for f in &m.manifest.files {
                                if let Some(k) =
                                    parse_partition_path(archive, &format!("{archive}/{}", f.path))
                                {
                                    keys.push(k);
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut by: BTreeMap<String, Presence> = BTreeMap::new();
        for k in keys {
            let p = by.entry(k.instrument.clone()).or_default();
            p.kinds.insert(k.kind.clone());
            p.archives.insert(k.archive.clone());
            if p.first_date.as_ref().is_none_or(|d| k.date < *d) {
                p.first_date = Some(k.date.clone());
            }
            if p.last_date.as_ref().is_none_or(|d| k.date > *d) {
                p.last_date = Some(k.date.clone());
            }
        }
        if let Ok(mut inv) = self.inventory.lock() {
            *inv = Some(Inventory {
                at: Instant::now(),
                by_instrument: by.clone(),
            });
        }
        Ok(by)
    }

    /// Checks that an instrument is known (registry or archive).
    ///
    /// # Errors
    ///
    /// `unknown_instrument`.
    pub async fn require_instrument(&self, id: &InstrumentId) -> ApiResult<()> {
        if self.registry().is_some_and(|r| r.get(id).is_some()) {
            return Ok(());
        }
        if self.inventory().await?.contains_key(id.as_str()) {
            return Ok(());
        }
        Err(ApiError::new(
            ErrorCode::UnknownInstrument,
            format!("{id} is not in the registry or the archive"),
        ))
    }

    /// Price and size scales, admission and flags of an instrument, from
    /// the registry or from its first partition row.
    ///
    /// # Errors
    ///
    /// `unknown_instrument` when neither has it.
    pub async fn scales(&self, id: &InstrumentId) -> ApiResult<(u8, u8)> {
        if let Some(spec) = self.registry().and_then(|r| r.get(id)) {
            return Ok((spec.price_scale, spec.size_scale));
        }
        let inv = self.inventory().await?;
        let presence = inv.get(id.as_str()).ok_or_else(|| {
            ApiError::new(
                ErrorCode::UnknownInstrument,
                format!("{id} is not in the archive"),
            )
        })?;
        for kind in [RecordKind::Trade, RecordKind::BookEvent, RecordKind::Bar] {
            if !presence.kinds.contains(kind.as_str()) {
                continue;
            }
            let Some(last) = &presence.last_date else {
                continue;
            };
            let day = crate::params::parse_time("date", last)?
                .0
                .div_euclid(DAY_MS);
            let files = self.files(kind, id, &[day]).await?;
            if files.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT price_scale, size_scale FROM read_parquet({}) LIMIT 1",
                sql_file_list(&files[..1])
            );
            let rows = self
                .engine
                .query(sql, vec![], |r| {
                    Ok((r.get::<_, u8>(0)?, r.get::<_, u8>(1)?))
                })
                .await?;
            if let Some(s) = rows.first() {
                return Ok(*s);
            }
        }
        Err(ApiError::new(
            ErrorCode::UnknownInstrument,
            format!("{id} has no partitions with scales"),
        ))
    }

    // -----------------------------------------------------------------
    // Record queries
    // -----------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    async fn select<R, F>(
        &self,
        kind: RecordKind,
        files: &[String],
        where_clause: &str,
        order: &str,
        limit: usize,
        params: Vec<Value>,
        map: F,
    ) -> ApiResult<Vec<R>>
    where
        R: Send + 'static,
        F: FnMut(&duckdb::Row<'_>) -> duckdb::Result<R> + Send + 'static,
    {
        if files.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT {} FROM read_parquet({}, hive_partitioning = false, union_by_name = true) \
             WHERE {where_clause} ORDER BY {order} LIMIT {limit}",
            rows::select_list(kind),
            sql_file_list(files),
        );
        self.engine.query(sql, params, map).await
    }

    fn seq_filter(after: Option<SeqKey>, params: &mut Vec<Value>) -> &'static str {
        match after {
            Some((b, i)) => {
                params.push(Value::UBigInt(b));
                params.push(Value::UBigInt(b));
                params.push(Value::UInt(i));
                " AND (block_number > ? OR (block_number = ? AND event_index > ?))"
            }
            None => "",
        }
    }

    fn time_params(start_ms: i64, end_ms: i64) -> Vec<Value> {
        vec![Value::BigInt(start_ms), Value::BigInt(end_ms)]
    }

    /// Trades with `start <= ts_source_ms < end` after `after`, in
    /// canonical order.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn trades(
        &self,
        id: &InstrumentId,
        start_ms: i64,
        end_ms: i64,
        after: Option<SeqKey>,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::Trade>> {
        let files = self
            .files(RecordKind::Trade, id, &days(start_ms, end_ms))
            .await?;
        let mut params = Self::time_params(start_ms, end_ms);
        let seq = Self::seq_filter(after, &mut params);
        let raw = self
            .select(
                RecordKind::Trade,
                &files,
                &format!("ts_source_ms >= epoch_ms(?::BIGINT) AND ts_source_ms < epoch_ms(?::BIGINT){seq}"),
                "block_number, event_index",
                limit,
                params,
                rows::map_trade,
            )
            .await?;
        rows::convert(raw, rows::RawTrade::into_record)
    }

    /// The newest `limit` trades with `ts_source_ms < end_ms`, looking back
    /// `lookback_ms`, returned in canonical (ascending) order.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn recent_trades(
        &self,
        id: &InstrumentId,
        end_ms: i64,
        lookback_ms: i64,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::Trade>> {
        let start_ms = end_ms.saturating_sub(lookback_ms);
        let files = self
            .files(RecordKind::Trade, id, &days(start_ms, end_ms))
            .await?;
        let raw = self
            .select(
                RecordKind::Trade,
                &files,
                "ts_source_ms >= epoch_ms(?::BIGINT) AND ts_source_ms < epoch_ms(?::BIGINT)",
                "block_number DESC, event_index DESC",
                limit,
                Self::time_params(start_ms, end_ms),
                rows::map_trade,
            )
            .await?;
        let mut v = rows::convert(raw, rows::RawTrade::into_record)?;
        v.reverse();
        Ok(v)
    }

    /// Book events with `start <= ts_source_ms < end` after `after`.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn book_events(
        &self,
        id: &InstrumentId,
        start_ms: i64,
        end_ms: i64,
        after: Option<SeqKey>,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::BookEvent>> {
        let files = self
            .files(RecordKind::BookEvent, id, &days(start_ms, end_ms))
            .await?;
        let mut params = Self::time_params(start_ms, end_ms);
        let seq = Self::seq_filter(after, &mut params);
        let raw = self
            .select(
                RecordKind::BookEvent,
                &files,
                &format!("ts_source_ms >= epoch_ms(?::BIGINT) AND ts_source_ms < epoch_ms(?::BIGINT){seq}"),
                "block_number, event_index",
                limit,
                params,
                rows::map_book_event,
            )
            .await?;
        rows::convert(raw, rows::RawBookEvent::into_record)
    }

    /// Book events strictly after block `after_block` with
    /// `ts_source_ms <= until_ms`, after `after` (for paging), on `days`.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn book_events_after_block(
        &self,
        id: &InstrumentId,
        files: &[String],
        after_block: u64,
        until_ms: i64,
        after: Option<SeqKey>,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::BookEvent>> {
        let mut params = vec![Value::UBigInt(after_block), Value::BigInt(until_ms)];
        let seq = Self::seq_filter(after, &mut params);
        let _ = id;
        let raw = self
            .select(
                RecordKind::BookEvent,
                files,
                &format!("block_number > ? AND ts_source_ms <= epoch_ms(?::BIGINT){seq}"),
                "block_number, event_index",
                limit,
                params,
                rows::map_book_event,
            )
            .await?;
        rows::convert(raw, rows::RawBookEvent::into_record)
    }

    /// The latest snapshot with `ts_source_ms <= at_ms`, looking back over
    /// `lookback_ms`.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn snapshot_at(
        &self,
        id: &InstrumentId,
        at_ms: i64,
        lookback_ms: i64,
    ) -> ApiResult<Option<mesh_types::BookSnapshot>> {
        let start = at_ms.saturating_sub(lookback_ms);
        let mut ds = days(start, at_ms + 1);
        ds.reverse();
        // Newest day first: stop at the first day with a checkpoint.
        for day in ds {
            let files = self.files(RecordKind::BookSnapshot, id, &[day]).await?;
            let raw = self
                .select(
                    RecordKind::BookSnapshot,
                    &files,
                    "ts_source_ms <= epoch_ms(?::BIGINT) AND ts_source_ms >= epoch_ms(?::BIGINT)",
                    "block_number DESC",
                    1,
                    vec![Value::BigInt(at_ms), Value::BigInt(start)],
                    rows::map_snapshot,
                )
                .await?;
            if let Some(r) = raw.into_iter().next() {
                return r.into_record().map(Some);
            }
        }
        Ok(None)
    }

    /// Bars of one interval with `start <= start_ms < end`, after
    /// `after_start` (exclusive), ascending.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn bars(
        &self,
        id: &InstrumentId,
        interval: mesh_types::BarInterval,
        start_ms: i64,
        end_ms: i64,
        after_start: Option<i64>,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::Bar>> {
        let files = self
            .files(RecordKind::Bar, id, &days(start_ms, end_ms))
            .await?;
        let lo = after_start.map_or(start_ms, |a| a.saturating_add(1).max(start_ms));
        let raw = self
            .select(
                RecordKind::Bar,
                &files,
                "interval = ? AND start_ms >= epoch_ms(?::BIGINT) AND start_ms < epoch_ms(?::BIGINT)",
                "start_ms, block_number, event_index",
                limit,
                vec![
                    Value::Text(interval.as_str().to_owned()),
                    Value::BigInt(lo),
                    Value::BigInt(end_ms),
                ],
                rows::map_bar,
            )
            .await?;
        rows::convert(raw, rows::RawBar::into_record)
    }

    /// The `count` bars before `before_ms` (descending start), looking back
    /// at most `lookback_ms`. Returned ascending.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn bars_before(
        &self,
        id: &InstrumentId,
        interval: mesh_types::BarInterval,
        before_ms: i64,
        count: usize,
        lookback_ms: i64,
    ) -> ApiResult<Vec<mesh_types::Bar>> {
        let start = before_ms.saturating_sub(lookback_ms);
        let files = self
            .files(RecordKind::Bar, id, &days(start, before_ms))
            .await?;
        let raw = self
            .select(
                RecordKind::Bar,
                &files,
                "interval = ? AND start_ms < epoch_ms(?::BIGINT) AND start_ms >= epoch_ms(?::BIGINT)",
                "start_ms DESC",
                count,
                vec![
                    Value::Text(interval.as_str().to_owned()),
                    Value::BigInt(before_ms),
                    Value::BigInt(start),
                ],
                rows::map_bar,
            )
            .await?;
        let mut bars = rows::convert(raw, rows::RawBar::into_record)?;
        bars.reverse();
        Ok(bars)
    }

    /// Footprints of one interval with `start <= start_ms < end`.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn footprints(
        &self,
        id: &InstrumentId,
        interval: mesh_types::BarInterval,
        start_ms: i64,
        end_ms: i64,
        after_start: Option<i64>,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::Footprint>> {
        let files = self
            .files(RecordKind::Footprint, id, &days(start_ms, end_ms))
            .await?;
        let lo = after_start.map_or(start_ms, |a| a.saturating_add(1).max(start_ms));
        let raw = self
            .select(
                RecordKind::Footprint,
                &files,
                "interval = ? AND start_ms >= epoch_ms(?::BIGINT) AND start_ms < epoch_ms(?::BIGINT)",
                "start_ms, block_number, event_index",
                limit,
                vec![
                    Value::Text(interval.as_str().to_owned()),
                    Value::BigInt(lo),
                    Value::BigInt(end_ms),
                ],
                rows::map_footprint,
            )
            .await?;
        rows::convert(raw, rows::RawFootprint::into_record)
    }

    /// Volume profiles overlapping `[start, end)`, after `(start_ms,
    /// end_ms)` of the previous page.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn profiles(
        &self,
        id: &InstrumentId,
        start_ms: i64,
        end_ms: i64,
        after: Option<(i64, i64)>,
        limit: usize,
    ) -> ApiResult<Vec<mesh_types::VolumeProfile>> {
        // A profile is partitioned by its start, which may be up to a day
        // before the range.
        let files = self
            .files(
                RecordKind::VolumeProfile,
                id,
                &days(start_ms - DAY_MS, end_ms),
            )
            .await?;
        let mut params = vec![Value::BigInt(end_ms), Value::BigInt(start_ms)];
        let page = match after {
            Some((s, e)) => {
                params.extend([Value::BigInt(s), Value::BigInt(s), Value::BigInt(e)]);
                " AND (start_ms > epoch_ms(?::BIGINT) OR (start_ms = epoch_ms(?::BIGINT) AND end_ms > epoch_ms(?::BIGINT)))"
            }
            None => "",
        };
        let raw = self
            .select(
                RecordKind::VolumeProfile,
                &files,
                &format!("start_ms < epoch_ms(?::BIGINT) AND end_ms > epoch_ms(?::BIGINT){page}"),
                "start_ms, end_ms",
                limit,
                params,
                rows::map_profile,
            )
            .await?;
        rows::convert(raw, rows::RawProfile::into_record)
    }

    /// One record by kind and position, with the file it came from.
    ///
    /// # Errors
    ///
    /// Engine errors; `unsupported` for swap prints.
    pub async fn record_at(
        &self,
        kind: RecordKind,
        id: &InstrumentId,
        day: i64,
        block: u64,
        index: u32,
    ) -> ApiResult<Option<(mesh_types::Record, String)>> {
        use mesh_types::Record;
        let files = self.files(kind, id, &[day]).await?;
        for file in files {
            let params = vec![Value::UBigInt(block), Value::UInt(index)];
            let one = std::slice::from_ref(&file);
            let w = "block_number = ? AND event_index = ?";
            let o = "block_number";
            let rec = match kind {
                RecordKind::Trade => self
                    .select(kind, one, w, o, 1, params, rows::map_trade)
                    .await?
                    .into_iter()
                    .next()
                    .map(|r| r.into_record().map(Record::Trade))
                    .transpose()?,
                RecordKind::BookEvent => self
                    .select(kind, one, w, o, 1, params, rows::map_book_event)
                    .await?
                    .into_iter()
                    .next()
                    .map(|r| r.into_record().map(Record::BookEvent))
                    .transpose()?,
                RecordKind::BookSnapshot => self
                    .select(kind, one, w, o, 1, params, rows::map_snapshot)
                    .await?
                    .into_iter()
                    .next()
                    .map(|r| r.into_record().map(Record::BookSnapshot))
                    .transpose()?,
                RecordKind::Bar => self
                    .select(kind, one, w, o, 1, params, rows::map_bar)
                    .await?
                    .into_iter()
                    .next()
                    .map(|r| r.into_record().map(Record::Bar))
                    .transpose()?,
                RecordKind::Footprint => self
                    .select(kind, one, w, o, 1, params, rows::map_footprint)
                    .await?
                    .into_iter()
                    .next()
                    .map(|r| r.into_record().map(Record::Footprint))
                    .transpose()?,
                RecordKind::VolumeProfile => self
                    .select(kind, one, w, o, 1, params, rows::map_profile)
                    .await?
                    .into_iter()
                    .next()
                    .map(|r| r.into_record().map(Record::VolumeProfile))
                    .transpose()?,
                RecordKind::SwapPrint => {
                    return Err(ApiError::new(
                        ErrorCode::Unsupported,
                        "swap prints are not served by this API yet",
                    ));
                }
            };
            if let Some(r) = rec {
                return Ok(Some((r, file)));
            }
        }
        Ok(None)
    }

    /// Per-hour record counts and OR-ed flags of trades and book events for
    /// one instrument: `(hour_start_ms, count, flags_or, all_backfill)`.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn hourly_activity(
        &self,
        id: &InstrumentId,
        start_ms: i64,
        end_ms: i64,
    ) -> ApiResult<Vec<(i64, u64, u32, bool)>> {
        let ds = days(start_ms, end_ms);
        let mut files = self.files(RecordKind::Trade, id, &ds).await?;
        files.extend(self.files(RecordKind::BookEvent, id, &ds).await?);
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let backfill = mesh_types::Flag::Backfill.bit();
        let sql = format!(
            "SELECT (epoch_ms(ts_source_ms) // {HOUR_MS}) * {HOUR_MS} AS h, count(*)::UBIGINT, \
             bit_or(flags), bool_and((flags & {backfill}) <> 0) \
             FROM read_parquet({}, hive_partitioning = false, union_by_name = true) \
             WHERE ts_source_ms >= epoch_ms(?::BIGINT) AND ts_source_ms < epoch_ms(?::BIGINT) \
             GROUP BY h ORDER BY h",
            sql_file_list(&files)
        );
        self.engine
            .query(sql, Self::time_params(start_ms, end_ms), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .await
    }

    /// Latest `ts_source_ms` of `kind` for an instrument, from its newest
    /// partition day.
    ///
    /// # Errors
    ///
    /// Engine errors.
    pub async fn latest_time(&self, kind: RecordKind, id: &InstrumentId) -> ApiResult<Option<i64>> {
        let inv = self.inventory().await?;
        let Some(last) = inv.get(id.as_str()).and_then(|p| p.last_date.clone()) else {
            return Ok(None);
        };
        let day = crate::params::parse_time("date", &last)?
            .0
            .div_euclid(DAY_MS);
        for d in [day, day - 1] {
            let files = self.files(kind, id, &[d]).await?;
            if files.is_empty() {
                continue;
            }
            let sql = format!(
                "SELECT max(epoch_ms(ts_source_ms)) FROM read_parquet({})",
                sql_file_list(&files)
            );
            let rows = self
                .engine
                .query(sql, vec![], |r| r.get::<_, Option<i64>>(0))
                .await?;
            if let Some(Some(t)) = rows.first() {
                return Ok(Some(*t));
            }
        }
        Ok(None)
    }
}

/// UTC days intersecting `[start, end)`.
#[must_use]
pub fn days(start_ms: i64, end_ms: i64) -> Vec<i64> {
    if end_ms <= start_ms {
        return Vec::new();
    }
    (start_ms.div_euclid(DAY_MS)..=(end_ms - 1).div_euclid(DAY_MS)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_lists() {
        assert_eq!(days(0, DAY_MS), vec![0]);
        assert_eq!(days(DAY_MS - 1, DAY_MS + 1), vec![0, 1]);
        assert!(days(5, 5).is_empty());
    }
}
