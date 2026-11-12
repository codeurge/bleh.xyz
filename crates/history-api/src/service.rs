//! The operations every surface shares: the JSON API, the compatibility
//! layers and the MCP tools all call these, so limits, validation and
//! pagination behave the same everywhere.

use std::collections::BTreeMap;
use std::sync::Arc;

use mesh_types::{
    BarInterval, BookSnapshot, Flag, InstrumentId, Record, RecordKind, SnapshotDepth, UnixMillis,
};
use serde::{Deserialize, Serialize};

use crate::book::{Inconsistencies, L4Book};
use crate::config::Config;
use crate::engine::Engine;
use crate::error::{ApiError, ApiResult, ErrorCode};
use crate::params::{
    Cursor, DAY_MS, HOUR_MS, TimeRange, day_string, parse_instrument, parse_interval, parse_time,
};
use crate::store::{Presence, Store};

/// Repository URL used in provenance explanations.
pub const REPOSITORY: &str = "https://github.com/codeurge/bleh.xyz";

/// Common query parameters for record ranges.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RangeQuery {
    /// Instrument ID (`hyperliquid:perp:BTC`).
    pub instrument: String,
    /// Start (inclusive): ms since the epoch, RFC 3339 or `YYYY-MM-DD`.
    pub start: Option<String>,
    /// End (exclusive).
    pub end: Option<String>,
    /// Page size.
    pub limit: Option<usize>,
    /// Opaque cursor from a previous page.
    pub cursor: Option<String>,
    /// Bar or footprint interval.
    pub interval: Option<String>,
}

/// A page of records.
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    /// Instrument.
    pub instrument: String,
    /// Record kind.
    pub kind: RecordKind,
    /// Range start (inclusive), ms.
    pub start_ms: i64,
    /// Range end (exclusive), ms.
    pub end_ms: i64,
    /// Records in this page.
    pub count: usize,
    /// Records, in observation-v0 JSON.
    pub records: Vec<Record>,
    /// Cursor for the next page, or `null` when this is the last.
    pub next_cursor: Option<String>,
}

/// One instrument in `/v0/instruments`.
#[derive(Debug, Clone, Serialize)]
pub struct InstrumentInfo {
    /// Instrument ID.
    pub id: String,
    /// Venue.
    pub venue: String,
    /// Market class (`perp`, `spot`, ...).
    pub class: String,
    /// Venue's own symbol.
    pub symbol: String,
    /// Fractional digits of prices.
    pub price_scale: Option<u8>,
    /// Fractional digits of sizes.
    pub size_scale: Option<u8>,
    /// Default flags.
    pub flags: Vec<&'static str>,
    /// Admission covering the instrument's records.
    pub admission: Option<String>,
    /// Where the entry came from: `registry`, `archive` or both.
    pub sources: Vec<&'static str>,
    /// Presence in the archive.
    pub archive: Option<Presence>,
}

/// A page of instruments.
#[derive(Debug, Clone, Serialize)]
pub struct InstrumentPage {
    /// Instruments in this page.
    pub instruments: Vec<InstrumentInfo>,
    /// Total matching.
    pub total: usize,
    /// Cursor for the next page.
    pub next_cursor: Option<String>,
}

/// Book at time T.
#[derive(Debug, Clone, Serialize)]
pub struct BookAt {
    /// Instrument.
    pub instrument: String,
    /// Requested time, ms.
    pub at_ms: i64,
    /// The checkpoint the book was built from.
    pub checkpoint: Checkpoint,
    /// Book events applied on top of the checkpoint.
    pub events_applied: u64,
    /// Events that did not fit the book (should be zero).
    pub inconsistencies: Inconsistencies,
    /// Best bid price, if any.
    pub best_bid: Option<String>,
    /// Best ask price, if any.
    pub best_ask: Option<String>,
    /// The book as a `book_snapshot` record at the end of the last block
    /// at or before T.
    pub book: Record,
}

/// The checkpoint snapshot behind a reconstructed book.
#[derive(Debug, Clone, Serialize)]
pub struct Checkpoint {
    /// Its sequence (an end-of-block marker).
    pub seq: mesh_types::Seq,
    /// Its block time.
    pub ts_source_ms: i64,
    /// `l4` or `l2`.
    pub depth: &'static str,
    /// Flags on the checkpoint.
    pub flags: Vec<&'static str>,
}

/// Coverage of one hour.
#[derive(Debug, Clone, Serialize)]
pub struct HourCoverage {
    /// Hour start, ms.
    pub hour_ms: i64,
    /// `recorded`, `backfilled` or `missing`.
    pub status: &'static str,
    /// Trades and book events in the hour.
    pub records: u64,
    /// A record in this hour carries `gap_before`: the source itself
    /// reported skipped data.
    pub gap_before: bool,
}

/// Coverage of one instrument.
#[derive(Debug, Clone, Serialize)]
pub struct InstrumentCoverage {
    /// Instrument.
    pub instrument: String,
    /// Hours with records recorded live.
    pub recorded_hours: usize,
    /// Hours whose records are all backfilled.
    pub backfilled_hours: usize,
    /// Hours with no records.
    pub missing_hours: usize,
    /// Hours with a source-reported gap.
    pub gap_hours: usize,
    /// Per-hour detail.
    pub hours: Vec<HourCoverage>,
}

/// Coverage response.
#[derive(Debug, Clone, Serialize)]
pub struct Coverage {
    /// Range start (hour-aligned), ms.
    pub start_ms: i64,
    /// Range end (hour-aligned, exclusive), ms.
    pub end_ms: i64,
    /// Note on interpretation.
    pub note: &'static str,
    /// Per instrument.
    pub instruments: Vec<InstrumentCoverage>,
}

/// One manifest in `/v0/manifests`.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestInfo {
    /// Archive.
    pub archive: String,
    /// Day.
    pub date: String,
    /// Chain position.
    pub sequence: u64,
    /// `SHA-256(canonical_json(signed_manifest))`, the chain and anchor hash.
    pub manifest_sha256: Option<String>,
    /// Previous day's manifest hash.
    pub previous_manifest_sha256: Option<String>,
    /// Signer address.
    pub signer: String,
    /// Manifest location (path relative to the archive root, and URL when
    /// a public base is known).
    pub manifest_path: String,
    /// Manifest URL.
    pub manifest_url: Option<String>,
    /// Anchors file URL.
    pub anchors_url: Option<String>,
    /// OpenTimestamps proof URL.
    pub ots_url: Option<String>,
    /// BitTorrent v2 infohash.
    pub infohash_v2: String,
    /// Magnet link.
    pub magnet: String,
    /// Web seeds.
    pub web_seeds: Vec<String>,
    /// Number of files.
    pub file_count: usize,
    /// Total bytes.
    pub total_bytes: u64,
    /// Files, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<ManifestFileInfo>>,
}

/// One file of a manifest.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestFileInfo {
    /// Path relative to the archive root.
    pub path: String,
    /// HTTPS URL on the first web seed.
    pub url: String,
    /// Bytes.
    pub size: u64,
    /// SHA-256 of the bytes.
    pub sha256: String,
    /// Canonical content hash of the records.
    pub content_hash: Option<String>,
    /// Records in the file.
    pub record_count: Option<u64>,
    /// IPFS CID.
    pub cid: String,
    /// `ipfs://` URI.
    pub ipfs_uri: String,
}

/// The service: configuration, engine and store.
#[derive(Debug, Clone)]
pub struct Service {
    /// Configuration.
    pub config: Arc<Config>,
    /// Store.
    pub store: Store,
}

fn flag_names(f: mesh_types::Flags) -> Vec<&'static str> {
    f.iter().map(Flag::name).collect()
}

impl Service {
    /// Opens the engine and store.
    ///
    /// # Errors
    ///
    /// Engine or registry errors.
    pub fn open(config: Config) -> anyhow::Result<Self> {
        let engine = Engine::open(&config)?;
        let store = Store::new(&config, engine)?;
        Ok(Self {
            config: Arc::new(config),
            store,
        })
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Engine {
        &self.store.engine
    }

    fn range(&self, q: &RangeQuery, default_ms: i64) -> ApiResult<TimeRange> {
        let max = i64::try_from(self.config.limits.max_range.as_millis()).unwrap_or(i64::MAX);
        TimeRange::parse(q.start.as_deref(), q.end.as_deref(), default_ms, max)
    }

    /// The widest range for precomputed bars of an interval: the general
    /// maximum, or one page of bars, whichever is wider.
    #[must_use]
    pub fn max_bar_range_ms(&self, interval: BarInterval) -> i64 {
        let l = &self.config.limits;
        let general = i64::try_from(l.max_range.as_millis()).unwrap_or(i64::MAX);
        let page = interval
            .millis()
            .saturating_mul(i64::try_from(l.max_rows).unwrap_or(i64::MAX));
        general.max(page)
    }

    fn bar_range(&self, q: &RangeQuery, interval: BarInterval) -> ApiResult<TimeRange> {
        let default = interval.millis().saturating_mul(500).max(DAY_MS);
        TimeRange::parse(
            q.start.as_deref(),
            q.end.as_deref(),
            default,
            self.max_bar_range_ms(interval),
        )
    }

    async fn instrument(&self, s: &str) -> ApiResult<InstrumentId> {
        if s.trim().is_empty() {
            return Err(ApiError::bad_request("instrument is required"));
        }
        let id = parse_instrument(s)?;
        self.store.require_instrument(&id).await?;
        Ok(id)
    }

    /// Lists instruments, optionally filtered by a case-insensitive
    /// substring of the ID or symbol, or by venue or class.
    ///
    /// # Errors
    ///
    /// Store errors; `invalid_cursor`.
    pub async fn instruments(
        &self,
        query: Option<&str>,
        venue: Option<&str>,
        class: Option<&str>,
        limit: Option<usize>,
        cursor: Option<&str>,
    ) -> ApiResult<InstrumentPage> {
        let inv = self.store.inventory().await?;
        let mut all: BTreeMap<String, InstrumentInfo> = BTreeMap::new();
        if let Some(reg) = self.store.registry() {
            for spec in reg.iter() {
                all.insert(
                    spec.id.to_string(),
                    InstrumentInfo {
                        id: spec.id.to_string(),
                        venue: spec.venue.to_string(),
                        class: spec.class.as_str().to_owned(),
                        symbol: spec.venue_symbol.clone(),
                        price_scale: Some(spec.price_scale),
                        size_scale: Some(spec.size_scale),
                        flags: flag_names(spec.flags),
                        admission: Some(spec.admission.to_string()),
                        sources: vec!["registry"],
                        archive: inv.get(spec.id.as_str()).cloned(),
                    },
                );
            }
        }
        for (id, presence) in &inv {
            if let Some(e) = all.get_mut(id) {
                e.sources.push("archive");
                continue;
            }
            let Ok(iid) = InstrumentId::new(id.clone()) else {
                continue;
            };
            all.insert(
                id.clone(),
                InstrumentInfo {
                    id: id.clone(),
                    venue: iid.venue_part().to_owned(),
                    class: iid.class_part().to_owned(),
                    symbol: iid.symbol().to_owned(),
                    price_scale: None,
                    size_scale: None,
                    flags: Vec::new(),
                    admission: None,
                    sources: vec!["archive"],
                    archive: Some(presence.clone()),
                },
            );
        }
        let needle = query.map(str::to_ascii_lowercase);
        let mut matching: Vec<InstrumentInfo> = all
            .into_values()
            .filter(|i| {
                needle.as_ref().is_none_or(|n| {
                    i.id.to_ascii_lowercase().contains(n)
                        || i.symbol.to_ascii_lowercase().contains(n)
                }) && venue.is_none_or(|v| i.venue == v)
                    && class.is_none_or(|c| i.class == c)
            })
            .collect();
        let total = matching.len();
        let fp = format!(
            "instruments|{}|{}|{}",
            query.unwrap_or(""),
            venue.unwrap_or(""),
            class.unwrap_or("")
        );
        let offset = match cursor {
            Some(c) => usize::try_from(Cursor::decode(c, &fp, 1)?.k[0].max(0)).unwrap_or(0),
            None => 0,
        };
        let page = self.config.limits.page(limit);
        let mut slice: Vec<InstrumentInfo> =
            matching.drain(offset.min(total)..).take(page).collect();
        // Fill scales for archive-only instruments on this page.
        for i in &mut slice {
            if i.price_scale.is_none()
                && let Ok(id) = InstrumentId::new(i.id.clone())
                && let Ok((ps, ss)) = self.store.scales(&id).await
            {
                i.price_scale = Some(ps);
                i.size_scale = Some(ss);
            }
        }
        let next = offset + slice.len();
        Ok(InstrumentPage {
            next_cursor: (next < total)
                .then(|| Cursor::after(&fp, vec![i64::try_from(next).unwrap_or(i64::MAX)])),
            instruments: slice,
            total,
        })
    }

    /// Trades over a range, paginated.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn trades(&self, q: &RangeQuery) -> ApiResult<Page> {
        let id = self.instrument(&q.instrument).await?;
        let r = self.range(q, HOUR_MS)?;
        let fp = format!("trade|{id}|{}|{}", r.start_ms, r.end_ms);
        let after = decode_seq_cursor(q.cursor.as_deref(), &fp)?;
        let limit = self.config.limits.page(q.limit);
        let mut v = self
            .store
            .trades(&id, r.start_ms, r.end_ms, after, limit + 1)
            .await?;
        let more = v.len() > limit;
        v.truncate(limit);
        let next = more
            .then(|| v.last().map(|t| seq_cursor(&fp, t.prov.seq)))
            .flatten();
        Ok(page(
            &id,
            RecordKind::Trade,
            r,
            v.into_iter().map(Record::Trade).collect(),
            next,
        ))
    }

    /// Book events over a range, paginated.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn book_events(&self, q: &RangeQuery) -> ApiResult<Page> {
        let id = self.instrument(&q.instrument).await?;
        let r = self.range(q, HOUR_MS)?;
        let fp = format!("book_event|{id}|{}|{}", r.start_ms, r.end_ms);
        let after = decode_seq_cursor(q.cursor.as_deref(), &fp)?;
        let limit = self.config.limits.page(q.limit);
        let mut v = self
            .store
            .book_events(&id, r.start_ms, r.end_ms, after, limit + 1)
            .await?;
        let more = v.len() > limit;
        v.truncate(limit);
        let next = more
            .then(|| v.last().map(|t| seq_cursor(&fp, t.prov.seq)))
            .flatten();
        Ok(page(
            &id,
            RecordKind::BookEvent,
            r,
            v.into_iter().map(Record::BookEvent).collect(),
            next,
        ))
    }

    /// Precomputed bars over a range, paginated.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn bars(&self, q: &RangeQuery) -> ApiResult<Page> {
        let id = self.instrument(&q.instrument).await?;
        let interval = parse_interval(q.interval.as_deref().unwrap_or("1m"))?;
        let r = self.bar_range(q, interval)?;
        let fp = format!("bar|{id}|{interval}|{}|{}", r.start_ms, r.end_ms);
        let after = match q.cursor.as_deref() {
            Some(c) => Some(Cursor::decode(c, &fp, 1)?.k[0]),
            None => None,
        };
        let limit = self.config.limits.page(q.limit);
        let mut v = self
            .store
            .bars(&id, interval, r.start_ms, r.end_ms, after, limit + 1)
            .await?;
        let more = v.len() > limit;
        v.truncate(limit);
        let next = more
            .then(|| v.last().map(|b| Cursor::after(&fp, vec![b.start_ms.0])))
            .flatten();
        Ok(page(
            &id,
            RecordKind::Bar,
            r,
            v.into_iter().map(Record::Bar).collect(),
            next,
        ))
    }

    /// Precomputed footprints over a range, paginated.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn footprints(&self, q: &RangeQuery) -> ApiResult<Page> {
        let id = self.instrument(&q.instrument).await?;
        let interval = parse_interval(q.interval.as_deref().unwrap_or("1m"))?;
        let r = self.range(q, HOUR_MS)?;
        let fp = format!("footprint|{id}|{interval}|{}|{}", r.start_ms, r.end_ms);
        let after = match q.cursor.as_deref() {
            Some(c) => Some(Cursor::decode(c, &fp, 1)?.k[0]),
            None => None,
        };
        let limit = self.config.limits.page(q.limit);
        let mut v = self
            .store
            .footprints(&id, interval, r.start_ms, r.end_ms, after, limit + 1)
            .await?;
        let more = v.len() > limit;
        v.truncate(limit);
        let next = more
            .then(|| v.last().map(|b| Cursor::after(&fp, vec![b.start_ms.0])))
            .flatten();
        Ok(page(
            &id,
            RecordKind::Footprint,
            r,
            v.into_iter().map(Record::Footprint).collect(),
            next,
        ))
    }

    /// Precomputed volume profiles overlapping a range, paginated.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn volume_profiles(&self, q: &RangeQuery) -> ApiResult<Page> {
        let id = self.instrument(&q.instrument).await?;
        let r = self.range(q, DAY_MS)?;
        let fp = format!("volume_profile|{id}|{}|{}", r.start_ms, r.end_ms);
        let after = match q.cursor.as_deref() {
            Some(c) => {
                let k = Cursor::decode(c, &fp, 2)?.k;
                Some((k[0], k[1]))
            }
            None => None,
        };
        let limit = self.config.limits.page(q.limit);
        let mut v = self
            .store
            .profiles(&id, r.start_ms, r.end_ms, after, limit + 1)
            .await?;
        let more = v.len() > limit;
        v.truncate(limit);
        let next = more
            .then(|| {
                v.last()
                    .map(|p| Cursor::after(&fp, vec![p.start_ms.0, p.end_ms.0]))
            })
            .flatten();
        Ok(page(
            &id,
            RecordKind::VolumeProfile,
            r,
            v.into_iter().map(Record::VolumeProfile).collect(),
            next,
        ))
    }

    /// Reconstructs the order-level book at `at_ms` into an [`L4Book`],
    /// returning it with its checkpoint.
    ///
    /// # Errors
    ///
    /// `not_found` without a checkpoint in the lookback window,
    /// `too_large` when more than the configured number of events would
    /// have to be applied.
    pub async fn build_book(
        &self,
        id: &InstrumentId,
        at_ms: i64,
    ) -> ApiResult<(L4Book, BookSnapshot)> {
        let lookback =
            i64::try_from(self.config.limits.max_checkpoint_lookback.as_millis()).unwrap_or(DAY_MS);
        let snap = self
            .store
            .snapshot_at(id, at_ms, lookback)
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotFound,
                    format!(
                        "no book checkpoint for {id} within {} hours before {}",
                        lookback / HOUR_MS,
                        UnixMillis(at_ms).to_rfc3339()
                    ),
                )
            })?;
        if matches!(snap.depth, SnapshotDepth::L2 { .. }) {
            // Aggregated checkpoints (for example backfilled hourly L2) are
            // returned as they are: order-level events cannot apply.
            return Ok((L4Book::default(), snap));
        }
        let mut book = L4Book::from_snapshot(&snap)?;
        let block = snap.prov.seq.block_number;
        let ds = crate::store::days(snap.prov.ts_source_ms.0, at_ms + 1);
        let files = self.store.files(RecordKind::BookEvent, id, &ds).await?;
        let max = self.config.limits.max_book_events;
        let chunk = 50_000usize;
        let mut after = None;
        loop {
            let events = self
                .store
                .book_events_after_block(id, &files, block, at_ms, after, chunk)
                .await?;
            for e in &events {
                book.apply(e);
            }
            if usize::try_from(book.events_applied).unwrap_or(usize::MAX) > max {
                return Err(ApiError::new(
                    ErrorCode::TooLarge,
                    format!("more than {max} book events after the nearest checkpoint"),
                ));
            }
            match events.last() {
                Some(last) if events.len() == chunk => {
                    after = Some((last.prov.seq.block_number, last.prov.seq.event_index));
                }
                _ => break,
            }
        }
        Ok((book, snap))
    }

    /// Book at time T (`depth` `l4` or `l2`, top `levels` price levels).
    ///
    /// # Errors
    ///
    /// As [`Service::build_book`].
    pub async fn book_at(
        &self,
        instrument: &str,
        at: Option<&str>,
        depth: Option<&str>,
        levels: Option<usize>,
    ) -> ApiResult<BookAt> {
        let id = self.instrument(instrument).await?;
        let at_ms = match at {
            Some(s) => parse_time("at", s)?.0,
            None => self
                .store
                .latest_time(RecordKind::BookEvent, &id)
                .await?
                .or(self
                    .store
                    .latest_time(RecordKind::BookSnapshot, &id)
                    .await?)
                .ok_or_else(|| {
                    ApiError::new(ErrorCode::NotFound, format!("no book data for {id}"))
                })?,
        };
        let l2 = match depth.unwrap_or("l4") {
            "l4" => false,
            "l2" => true,
            other => {
                return Err(ApiError::bad_request(format!(
                    "depth must be l4 or l2, not {other:?}"
                )));
            }
        };
        let (book, snap) = self.build_book(&id, at_ms).await?;
        let checkpoint = Checkpoint {
            seq: snap.prov.seq,
            ts_source_ms: snap.prov.ts_source_ms.0,
            depth: match snap.depth {
                SnapshotDepth::L4 { .. } => "l4",
                SnapshotDepth::L2 { .. } => "l2",
            },
            flags: flag_names(snap.prov.flags),
        };
        let record = if checkpoint.depth == "l2" {
            let mut s = snap.clone();
            if let (Some(n), SnapshotDepth::L2 { bids, asks }) = (levels, &mut s.depth) {
                bids.truncate(n);
                asks.truncate(n);
            }
            s
        } else {
            book.to_snapshot(&snap.prov, l2, levels)?
        };
        let (bb, ba) = record.depth.top();
        crate::metrics::global().add_rows(1);
        Ok(BookAt {
            instrument: id.to_string(),
            at_ms,
            checkpoint,
            events_applied: book.events_applied,
            inconsistencies: book.inconsistencies,
            best_bid: bb.map(|p| p.to_string()),
            best_ask: ba.map(|p| p.to_string()),
            book: Record::BookSnapshot(record),
        })
    }

    /// Recorded, backfilled and missing hours per market.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn coverage(
        &self,
        instrument: Option<&str>,
        start: Option<&str>,
        end: Option<&str>,
    ) -> ApiResult<Coverage> {
        let max =
            i64::try_from(self.config.limits.max_coverage_range.as_millis()).unwrap_or(i64::MAX);
        let r = TimeRange::parse(start, end, DAY_MS, max)?;
        let start_ms = r.start_ms.div_euclid(HOUR_MS) * HOUR_MS;
        let end_ms = (r.end_ms + HOUR_MS - 1).div_euclid(HOUR_MS) * HOUR_MS;
        let ids: Vec<InstrumentId> = match instrument {
            Some(s) => vec![self.instrument(s).await?],
            None => {
                let mut v: Vec<InstrumentId> = self
                    .store
                    .inventory()
                    .await?
                    .into_keys()
                    .filter_map(|k| InstrumentId::new(k).ok())
                    .collect();
                if let Some(reg) = self.store.registry() {
                    for s in reg.iter() {
                        if !v.contains(&s.id) {
                            v.push(s.id.clone());
                        }
                    }
                }
                v.sort();
                if v.len() > 200 {
                    return Err(ApiError::new(
                        ErrorCode::RangeTooLarge,
                        "more than 200 instruments; pass instrument=",
                    ));
                }
                v
            }
        };
        let mut out = Vec::new();
        for id in ids {
            let activity = self.store.hourly_activity(&id, start_ms, end_ms).await?;
            let by_hour: BTreeMap<i64, (u64, u32, bool)> = activity
                .into_iter()
                .map(|(h, n, f, all_bf)| (h, (n, f, all_bf)))
                .collect();
            let gap_bit = Flag::GapBefore.bit();
            let mut hours = Vec::new();
            let mut h = start_ms;
            while h < end_ms {
                let entry = by_hour.get(&h);
                let (status, records, gap) = match entry {
                    None => ("missing", 0, false),
                    Some((n, f, all_bf)) => (
                        if *all_bf { "backfilled" } else { "recorded" },
                        *n,
                        f & gap_bit != 0,
                    ),
                };
                hours.push(HourCoverage {
                    hour_ms: h,
                    status,
                    records,
                    gap_before: gap,
                });
                h += HOUR_MS;
            }
            let count = |s: &str| hours.iter().filter(|x| x.status == s).count();
            out.push(InstrumentCoverage {
                instrument: id.to_string(),
                recorded_hours: count("recorded"),
                backfilled_hours: count("backfilled"),
                missing_hours: count("missing"),
                gap_hours: hours.iter().filter(|x| x.gap_before).count(),
                hours,
            });
        }
        Ok(Coverage {
            start_ms,
            end_ms,
            note: "An hour is missing when the archive has no trade or book event for the market in it. \
                   A quiet market and a recording gap look alike unless gap_before marks the gap. \
                   Gaps are never filled.",
            instruments: out,
        })
    }

    fn public_url(&self, archive: &str, path: &str) -> Option<String> {
        let base = self
            .config
            .public_base_url
            .clone()
            .or(match &self.config.root {
                crate::config::ArchiveRoot::Https(u) => Some(u.clone()),
                _ => None,
            })?;
        Some(format!("{}/{archive}/{path}", base.trim_end_matches('/')))
    }

    /// Manifests (with torrent and IPFS links) for days in a range.
    ///
    /// # Errors
    ///
    /// Validation and store errors.
    pub async fn manifests(
        &self,
        start: Option<&str>,
        end: Option<&str>,
        archive: Option<&str>,
        with_files: bool,
    ) -> ApiResult<Vec<ManifestInfo>> {
        let r = TimeRange::parse(start, end, 7 * DAY_MS, 366 * DAY_MS)?;
        let days = r.days();
        let (Some(first), Some(last)) = (days.first(), days.last()) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for a in &self.config.archives {
            if archive.is_some_and(|x| x != a) {
                continue;
            }
            for date in self.store.manifest_days(a, *first, *last).await? {
                let Some(signed) = self.store.manifest(a, &date).await? else {
                    continue;
                };
                let m = &signed.manifest;
                let mpath = format!("_manifests/{date}/manifest.json");
                let seed = m.web_seeds.first().cloned().unwrap_or_default();
                out.push(ManifestInfo {
                    archive: a.clone(),
                    date: date.clone(),
                    sequence: m.sequence,
                    manifest_sha256: signed.sha256().ok().map(|h| h.to_string()),
                    previous_manifest_sha256: m.previous_manifest_sha256.map(|h| h.to_string()),
                    signer: m.signer.to_string(),
                    manifest_url: self.public_url(a, &mpath),
                    anchors_url: self
                        .public_url(a, &format!("_manifests/{date}/manifest.anchors.json")),
                    ots_url: self.public_url(a, &format!("_manifests/{date}/manifest.json.ots")),
                    manifest_path: mpath,
                    infohash_v2: m.torrent.infohash_v2.to_string(),
                    magnet: m.torrent.magnet.clone(),
                    web_seeds: m.web_seeds.clone(),
                    file_count: m.files.len(),
                    total_bytes: m.files.iter().map(|f| f.size).sum(),
                    files: with_files.then(|| {
                        m.files
                            .iter()
                            .map(|f| ManifestFileInfo {
                                path: f.path.clone(),
                                url: format!("{seed}{a}/{}", f.path),
                                size: f.size,
                                sha256: f.sha256.to_string(),
                                content_hash: f.content_hash.map(|h| h.to_string()),
                                record_count: f.record_count,
                                cid: f.ipfs.cid.clone(),
                                ipfs_uri: format!("ipfs://{}", f.ipfs.cid),
                            })
                            .collect()
                    }),
                });
            }
        }
        Ok(out)
    }

    /// Finds one record and explains its provenance and how to verify it.
    ///
    /// # Errors
    ///
    /// `not_found` when the record does not exist.
    pub async fn explain_provenance(
        &self,
        instrument: &str,
        kind: &str,
        block_number: u64,
        event_index: Option<u32>,
        time: &str,
    ) -> ApiResult<serde_json::Value> {
        let id = self.instrument(instrument).await?;
        let kind = RecordKind::ALL
            .iter()
            .find(|k| k.as_str() == kind)
            .copied()
            .ok_or_else(|| ApiError::bad_request(format!("unknown kind {kind:?}")))?;
        let t = parse_time("time", time)?;
        let day = t.0.div_euclid(DAY_MS);
        let index = event_index.unwrap_or(if kind == RecordKind::BookSnapshot {
            u32::MAX
        } else {
            0
        });
        let (record, file) = self
            .store
            .record_at(kind, &id, day, block_number, index)
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotFound,
                    format!(
                        "no {kind} for {id} at block {block_number} index {index} on {}",
                        day_string(day)
                    ),
                )
            })?;
        let prov = record.prov().clone();
        let mut manifest_entry = serde_json::Value::Null;
        let mut rel_path = None;
        for a in &self.config.archives {
            let base = self.store.layout.archive_base(a);
            if let Some(rel) = file.strip_prefix(&format!("{base}/")) {
                rel_path = Some((a.clone(), rel.to_owned()));
            }
        }
        if let Some((a, rel)) = &rel_path
            && let Some(m) = self.store.manifest(a, &day_string(day)).await?
            && let Some(f) = m.manifest.files.iter().find(|f| f.path == *rel)
        {
            manifest_entry = serde_json::json!({
                "archive": a,
                "date": m.manifest.date,
                "manifest_sha256": m.sha256().ok().map(|h| h.to_string()),
                "signer": m.manifest.signer.to_string(),
                "file": {
                    "path": f.path,
                    "sha256": f.sha256.to_string(),
                    "content_hash": f.content_hash.map(|h| h.to_string()),
                    "record_count": f.record_count,
                    "cid": f.ipfs.cid,
                },
                "magnet": m.manifest.torrent.magnet,
            });
        }
        let flags: Vec<serde_json::Value> = prov
            .flags
            .iter()
            .map(|f| serde_json::json!({"name": f.name(), "meaning": flag_meaning(f)}))
            .collect();
        let derived = match &record {
            Record::Bar(b) => Some(&b.derived),
            Record::Footprint(f) => Some(&f.derived),
            Record::VolumeProfile(v) => Some(&v.derived),
            _ => None,
        };
        let canonical = mesh_types::hash::canonical_record(&record)
            .map(|c| String::from_utf8_lossy(&c).into_owned())
            .map_err(ApiError::internal)?;
        Ok(serde_json::json!({
            "record": record,
            "canonical_encoding": canonical,
            "provenance": {
                "instrument": prov.instrument.to_string(),
                "venue": prov.venue.to_string(),
                "admission": {
                    "id": prov.admission.to_string(),
                    "meaning": "The admission record that allows these fields to be redistributed.",
                    "records": format!("{REPOSITORY}/tree/main/docs/admissions"),
                },
                "seq": {
                    "block_number": prov.seq.block_number,
                    "event_index": prov.seq.event_index,
                    "meaning": if prov.seq.is_end_of_block() {
                        "End-of-block marker: the state after every event of this block."
                    } else {
                        "Block height, then zero-based position within the block for this instrument and kind."
                    },
                },
                "ts_source_ms": {
                    "value": prov.ts_source_ms.0,
                    "utc": prov.ts_source_ms.to_rfc3339(),
                    "meaning": "Venue time (block time for Hyperliquid).",
                },
                "ts_ingest_ns": {
                    "value": prov.ts_ingest_ns.to_string(),
                    "meaning": "When the recording node observed the block (its local_time).",
                },
                "flags": flags,
            },
            "derived": derived.map(|d| serde_json::json!({
                "method": d.method.to_string(),
                "inputs_hash": d.inputs_hash.to_string(),
                "input_count": d.input_count,
                "how_to_verify": "Read the input trades for the interval from the archive, in canonical order, \
                                  and compute the observation-v0 content hash; it must equal inputs_hash.",
            })),
            "file": rel_path.as_ref().map(|(a, p)| format!("{a}/{p}")),
            "manifest": manifest_entry,
            "how_to_verify": [
                "Download the file over HTTPS, BitTorrent (magnet) or IPFS (cid) and check its SHA-256 against the manifest.",
                "Check the manifest signature (EIP-191 over keccak256(\"bleh-manifest-v0\\n\" || canonical_json(manifest))) against the registry's signer keys, and the hash chain to the previous day.",
                "Recompute the content hash of the file's records (observation-v0 § 6.2) and compare it with the manifest's content_hash.",
                "For live records, the per-block Merkle commitment covers canonical_encoding (observation-v0 § 7).",
            ],
            "docs": {
                "observation": format!("{REPOSITORY}/blob/main/docs/specs/observation-v0.md"),
                "archive_layout": format!("{REPOSITORY}/blob/main/docs/specs/archive-layout-v0.md"),
                "manifest": format!("{REPOSITORY}/blob/main/docs/specs/manifest-v0.md"),
            },
        }))
    }
}

fn flag_meaning(f: Flag) -> &'static str {
    match f {
        Flag::OracleAnchoredVenue => {
            "The venue anchors funding or price bands to an oracle; the record is still a venue trade or order."
        }
        Flag::Backfill => "Imported from a historical archive, not recorded live.",
        Flag::GapBefore => "The source skipped data immediately before this record.",
        Flag::LowResolution => "Lower resolution than the live stream.",
        Flag::SizeCapped => "The source capped the size or notional.",
        Flag::BlockTradeDelayed => "The source delayed the record as a block trade.",
        Flag::Replayed => {
            "Reconstructed by replaying venue actions through an emulated matcher, not observed directly."
        }
        Flag::ExchangeReferenced => {
            "A public body's statistic whose underlying transactions may be priced against an exchange price, or that uses an exchange-supplied factor; never an exchange price."
        }
    }
}

fn page(
    id: &InstrumentId,
    kind: RecordKind,
    r: TimeRange,
    records: Vec<Record>,
    next: Option<String>,
) -> Page {
    crate::metrics::global().add_rows(records.len());
    Page {
        instrument: id.to_string(),
        kind,
        start_ms: r.start_ms,
        end_ms: r.end_ms,
        count: records.len(),
        records,
        next_cursor: next,
    }
}

fn seq_cursor(fp: &str, seq: mesh_types::Seq) -> String {
    Cursor::after(
        fp,
        vec![
            i64::try_from(seq.block_number).unwrap_or(i64::MAX),
            i64::from(seq.event_index),
        ],
    )
}

fn decode_seq_cursor(c: Option<&str>, fp: &str) -> ApiResult<Option<(u64, u32)>> {
    let Some(c) = c else { return Ok(None) };
    let k = Cursor::decode(c, fp, 2)?.k;
    let bad = || ApiError::new(ErrorCode::InvalidCursor, "cursor is malformed");
    Ok(Some((
        u64::try_from(k[0]).map_err(|_| bad())?,
        u32::try_from(k[1]).map_err(|_| bad())?,
    )))
}

/// Parses a bar interval or defaults to one minute (shared by adapters).
///
/// # Errors
///
/// As [`parse_interval`].
pub fn interval_or_default(s: Option<&str>) -> ApiResult<BarInterval> {
    parse_interval(s.unwrap_or("1m"))
}
