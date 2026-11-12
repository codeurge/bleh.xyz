//! Synthetic archive for the demo and the tests.
//!
//! **Everything this module produces is invented** by a seeded simulation:
//! two fictional markets (`hyperliquid:perp:SYN` and
//! `hyperliquid:perp:syn:IDX`, the same fictional names and scales as
//! `fixtures/synthetic/hl`), an order-level book per market, trades against
//! it, periodic L4 snapshots, precomputed bars for every interval,
//! one-minute footprints and hourly volume profiles. No value comes from a
//! venue or any real market.
//!
//! The output follows `docs/specs/archive-layout-v0.md`: Hive-partitioned
//! Parquet with the `mesh_types::arrow` schemas, canonical row order, zstd,
//! 131,072-row groups, page statistics, `created_by = bleh-archive` and the
//! four `bleh.*` key-value entries. It also writes a registry file and a
//! signed daily manifest per day so `/v0/manifests` has something to show;
//! the demo manifests' torrent fields are placeholders (no torrent is
//! built) and their web seed is `https://demo.invalid/`.
//!
//! One hour of `syn:IDX` is deliberately missing, and its first records
//! after the hole carry the `gap_before` flag, so coverage maps show a gap.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::types::{
    TimestampMillisecondType, TimestampNanosecondType, UInt8Type, UInt32Type, UInt64Type,
};
use arrow_array::{
    ArrayRef, FixedSizeBinaryArray, Int64Array, ListArray, PrimitiveArray, RecordBatch,
    StringArray, StructArray,
};
use arrow_schema::{ArrowError, DataType, FieldRef, Fields};
use mesh_types::manifest::{
    IpfsCodec, IpfsRef, MANIFEST_FORMAT, Manifest, ManifestFile, SignedManifest, TorrentInfo,
};
use mesh_types::{
    AdmissionId, AggressorSide, Bar, BarInterval, BookAction, BookEvent, BookSide, BookSnapshot,
    DerivedHeader, Flag, Flags, Footprint, FootprintLevel, InstrumentId, InstrumentSpec,
    MarketClass, MethodId, OrderId, Price, ProfileLevel, Provenance, Record, RecordKind, Registry,
    RestingOrder, Seq, SigningKey, Size, SnapshotDepth, Trade, TradeId, UnixMillis, UnixNanos,
    VenueId, VolumeProfile,
};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_writer::ArrowWriterOptions;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};

use crate::params::{DAY_MS, HOUR_MS, day_string};
use crate::source::Layout;

/// Errors from generating or writing the synthetic archive.
#[derive(Debug, thiserror::Error)]
pub enum SynthError {
    /// Filesystem error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Arrow error.
    #[error("arrow: {0}")]
    Arrow(#[from] ArrowError),
    /// Parquet error.
    #[error("parquet: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    /// A mesh-types invariant failed (a bug in the generator).
    #[error("model: {0}")]
    Model(String),
}

fn model<E: std::fmt::Display>(e: E) -> SynthError {
    SynthError::Model(e.to_string())
}

/// Generator settings.
#[derive(Debug, Clone)]
pub struct SynthConfig {
    /// RNG seed.
    pub seed: u64,
    /// Time of the first block. Default 2026-01-01T22:00:00Z, so the default
    /// four hours cross a UTC day boundary.
    pub start_ms: i64,
    /// Hours to simulate.
    pub hours: u32,
    /// Time between blocks.
    pub block_ms: i64,
    /// Height of the first block.
    pub first_block: u64,
    /// An L4 snapshot after every this many blocks.
    pub snapshot_every: u64,
    /// Hour index (from 0) with no data for `syn:IDX`, if any.
    pub gap_hour: Option<u32>,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            seed: 7,
            start_ms: 1_767_304_800_000,
            hours: 4,
            block_ms: 2_000,
            first_block: 5_000_000,
            snapshot_every: 300,
            gap_hour: Some(1),
        }
    }
}

/// SplitMix64: output depends only on the seed and this code.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = u64::try_from(hi - lo + 1).unwrap_or(1).max(1);
        lo + i64::try_from(self.next() % span).unwrap_or(0)
    }

    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

#[derive(Debug, Clone)]
struct Resting {
    oid: u64,
    size: i64,
    ts_ms: i64,
}

/// Order-level book of the simulation.
#[derive(Debug, Default)]
struct SimBook {
    bids: BTreeMap<i64, VecDeque<Resting>>,
    asks: BTreeMap<i64, VecDeque<Resting>>,
    index: HashMap<u64, (BookSide, i64)>,
}

impl SimBook {
    fn side(&mut self, side: BookSide) -> &mut BTreeMap<i64, VecDeque<Resting>> {
        match side {
            BookSide::Bid => &mut self.bids,
            BookSide::Ask => &mut self.asks,
        }
    }

    fn best_bid(&self) -> Option<i64> {
        self.bids.keys().next_back().copied()
    }

    fn best_ask(&self) -> Option<i64> {
        self.asks.keys().next().copied()
    }

    fn add(&mut self, side: BookSide, price: i64, r: Resting) {
        self.index.insert(r.oid, (side, price));
        self.side(side).entry(price).or_default().push_back(r);
    }

    fn get_mut(&mut self, oid: u64) -> Option<&mut Resting> {
        let (side, price) = *self.index.get(&oid)?;
        self.side(side)
            .get_mut(&price)?
            .iter_mut()
            .find(|r| r.oid == oid)
    }

    fn remove(&mut self, oid: u64) {
        if let Some((side, price)) = self.index.remove(&oid) {
            let levels = self.side(side);
            if let Some(q) = levels.get_mut(&price) {
                q.retain(|r| r.oid != oid);
                if q.is_empty() {
                    levels.remove(&price);
                }
            }
        }
    }

    fn snapshot(&self, price_scale: u8, size_scale: u8) -> Result<SnapshotDepth, SynthError> {
        let conv = |price: i64, r: &Resting| -> Result<RestingOrder, SynthError> {
            Ok(RestingOrder {
                order_id: OrderId(r.oid),
                price: Price::new(price, price_scale).map_err(model)?,
                size: Size::new(r.size, size_scale).map_err(model)?,
                ts_order_ms: UnixMillis(r.ts_ms),
            })
        };
        let mut bids = Vec::new();
        for (p, q) in self.bids.iter().rev() {
            for r in q {
                bids.push(conv(*p, r)?);
            }
        }
        let mut asks = Vec::new();
        for (p, q) in &self.asks {
            for r in q {
                asks.push(conv(*p, r)?);
            }
        }
        Ok(SnapshotDepth::L4 { bids, asks })
    }

    fn random_order(&self, rng: &mut Rng) -> Option<u64> {
        if self.index.is_empty() {
            return None;
        }
        let mut ids: Vec<u64> = self.index.keys().copied().collect();
        ids.sort_unstable();
        let i = usize::try_from(rng.next() % u64::try_from(ids.len()).unwrap_or(1)).unwrap_or(0);
        ids.get(i).copied()
    }
}

/// A fictional market and its simulation parameters.
#[derive(Debug, Clone)]
struct Market {
    spec: InstrumentSpec,
    mid: i64,
    tick: i64,
    min_size: i64,
    max_size: i64,
    gap_hour: Option<u32>,
}

/// The fictional markets: the same names and scales as
/// `fixtures/synthetic/hl/markets.json`.
///
/// # Errors
///
/// Never in practice; mesh-types validation errors are reported.
pub fn registry() -> Result<Registry, SynthError> {
    let mut r = Registry::new();
    for m in markets(None)? {
        r.insert(m.spec).map_err(model)?;
    }
    Ok(r)
}

fn markets(gap_hour: Option<u32>) -> Result<Vec<Market>, SynthError> {
    let adm = AdmissionId::new("ADM-0001").map_err(model)?;
    let syn = InstrumentSpec::hyperliquid(MarketClass::Perp, "SYN", 3, None, adm.clone())
        .map_err(model)?;
    let idx = InstrumentSpec::hyperliquid(MarketClass::Perp, "syn:IDX", 2, None, adm)
        .map_err(model)?
        .with_flags(Flags::NONE.with(Flag::OracleAnchoredVenue));
    Ok(vec![
        Market {
            spec: syn,
            mid: 100_000,
            tick: 10,
            min_size: 1_000,
            max_size: 40_000,
            gap_hour: None,
        },
        Market {
            spec: idx,
            mid: 2_500_000,
            tick: 50,
            min_size: 100,
            max_size: 4_000,
            gap_hour,
        },
    ])
}

/// The generated records, grouped as they are partitioned.
#[derive(Debug, Default)]
pub struct SynthData {
    /// The registry of the fictional markets.
    pub registry: Registry,
    /// Records per (kind, instrument, UTC day), in canonical order.
    pub partitions: BTreeMap<(RecordKind, InstrumentId, i64), Vec<Record>>,
}

impl SynthData {
    /// All records of one kind and instrument, across days, in order.
    #[must_use]
    pub fn records(&self, kind: RecordKind, instrument: &InstrumentId) -> Vec<&Record> {
        self.partitions
            .iter()
            .filter(|((k, i, _), _)| *k == kind && i == instrument)
            .flat_map(|(_, v)| v.iter())
            .collect()
    }
}

fn prov(spec: &InstrumentSpec, seq: Seq, ts_ms: i64, ingest_ns: i64, flags: Flags) -> Provenance {
    Provenance::new(
        spec.id.clone(),
        spec.venue.clone(),
        seq,
        UnixMillis(ts_ms),
        UnixNanos(ingest_ns),
        spec.admission.clone(),
    )
    .with_flags(flags)
}

fn index_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX - 1)
}

/// Runs the simulation.
///
/// # Errors
///
/// Only on a generator bug (a record fails mesh-types validation).
#[allow(clippy::too_many_lines)]
pub fn generate(cfg: &SynthConfig) -> Result<SynthData, SynthError> {
    let mut data = SynthData {
        registry: registry()?,
        ..SynthData::default()
    };
    let blocks_per_hour = u64::try_from(HOUR_MS / cfg.block_ms.max(1))
        .unwrap_or(1)
        .max(1);
    let total_blocks = blocks_per_hour * u64::from(cfg.hours);
    for (mi, mut m) in markets(cfg.gap_hour)?.into_iter().enumerate() {
        let mut rng = Rng(cfg.seed ^ (0xA5A5_0000 + u64::try_from(mi).unwrap_or(0)));
        let ps = m.spec.price_scale;
        let ss = m.spec.size_scale;
        let base_flags = m.spec.flags;
        let mut book = SimBook::default();
        let mut next_oid: u64 = 10_000_000 * (u64::try_from(mi).unwrap_or(0) + 1);
        let mut next_tid: u64 = 900_000_000 * (u64::try_from(mi).unwrap_or(0) + 1);
        let mut trades: Vec<Trade> = Vec::new();
        let mut events: Vec<BookEvent> = Vec::new();
        let mut snapshots: Vec<BookSnapshot> = Vec::new();

        // Initial book, as of the end of the block before the first.
        let pre_block = cfg.first_block - 1;
        let pre_ts = cfg.start_ms - cfg.block_ms;
        for lvl in 1..=10 {
            for side in [BookSide::Bid, BookSide::Ask] {
                let px = match side {
                    BookSide::Bid => m.mid - lvl * m.tick,
                    BookSide::Ask => m.mid + lvl * m.tick,
                };
                next_oid += 1;
                book.add(
                    side,
                    px,
                    Resting {
                        oid: next_oid,
                        size: rng.range(m.min_size, m.max_size),
                        ts_ms: pre_ts - 60_000,
                    },
                );
            }
        }
        snapshots.push(BookSnapshot {
            prov: prov(
                &m.spec,
                Seq::end_of_block(pre_block),
                pre_ts,
                pre_ts * 1_000_000 + 3_000_000,
                base_flags,
            ),
            depth: book.snapshot(ps, ss)?,
        });

        let mut after_gap = false;
        for b in 0..total_blocks {
            let block = cfg.first_block + b;
            let ts = cfg.start_ms + i64::try_from(b).unwrap_or(0) * cfg.block_ms;
            let hour = u32::try_from(b / blocks_per_hour).unwrap_or(u32::MAX);
            if m.gap_hour == Some(hour) {
                after_gap = true;
                continue;
            }
            let ingest = ts * 1_000_000 + 4_000_000 + rng.range(0, 900_000);
            let mut flags = base_flags;
            if after_gap {
                flags = flags.with(Flag::GapBefore);
            }
            let mut block_trades = Vec::new();
            let mut block_events = Vec::new();
            let push_event = |order_id: u64, side: BookSide, price: i64, action: BookAction| {
                (order_id, side, price, action)
            };
            let mut raw_events = Vec::new();

            // Trades against the best level.
            for _ in 0..rng.range(0, 2) {
                let aggressor = if rng.chance(50) {
                    AggressorSide::Buy
                } else {
                    AggressorSide::Sell
                };
                let (side, best) = match aggressor {
                    AggressorSide::Buy => (BookSide::Ask, book.best_ask()),
                    _ => (BookSide::Bid, book.best_bid()),
                };
                let Some(best) = best else { continue };
                let Some(front) = book.side(side).get(&best).and_then(|q| q.front()).cloned()
                else {
                    continue;
                };
                let qty = if rng.chance(40) {
                    front.size
                } else {
                    rng.range(1, front.size)
                };
                let new_size = front.size - qty;
                next_tid += 1;
                block_trades.push((best, qty, aggressor, next_tid));
                raw_events.push(push_event(
                    front.oid,
                    side,
                    best,
                    BookAction::Fill {
                        orig_size: Some(Size::new(front.size, ss).map_err(model)?),
                        new_size: Size::new(new_size, ss).map_err(model)?,
                    },
                ));
                if new_size == 0 {
                    book.remove(front.oid);
                } else if let Some(r) = book.get_mut(front.oid) {
                    r.size = new_size;
                }
            }
            // Cancels.
            for _ in 0..rng.range(0, 1) {
                if book.index.len() < 12 {
                    break;
                }
                if let Some(oid) = book.random_order(&mut rng)
                    && let Some(&(side, price)) = book.index.get(&oid)
                {
                    book.remove(oid);
                    raw_events.push(push_event(oid, side, price, BookAction::Cancel {}));
                }
            }
            // Modifies.
            if rng.chance(30)
                && let Some(oid) = book.random_order(&mut rng)
                && let Some(&(side, price)) = book.index.get(&oid)
                && let Some(r) = book.get_mut(oid)
            {
                let orig = r.size;
                let mut new_size = rng.range(m.min_size, m.max_size);
                if new_size == orig {
                    new_size += 1;
                }
                r.size = new_size;
                raw_events.push(push_event(
                    oid,
                    side,
                    price,
                    BookAction::Modify {
                        orig_size: Some(Size::new(orig, ss).map_err(model)?),
                        new_size: Size::new(new_size, ss).map_err(model)?,
                    },
                ));
            }
            // Adds.
            for _ in 0..rng.range(1, 3) {
                let side = if rng.chance(50) {
                    BookSide::Bid
                } else {
                    BookSide::Ask
                };
                let k = rng.range(1, 12) * m.tick;
                let price = match side {
                    BookSide::Bid => {
                        let p = m.mid - k;
                        book.best_ask().map_or(p, |a| p.min(a - m.tick))
                    }
                    BookSide::Ask => {
                        let p = m.mid + k;
                        book.best_bid().map_or(p, |bb| p.max(bb + m.tick))
                    }
                };
                next_oid += 1;
                let size = rng.range(m.min_size, m.max_size);
                book.add(
                    side,
                    price,
                    Resting {
                        oid: next_oid,
                        size,
                        ts_ms: ts,
                    },
                );
                raw_events.push(push_event(
                    next_oid,
                    side,
                    price,
                    BookAction::Add {
                        size: Size::new(size, ss).map_err(model)?,
                    },
                ));
            }

            for (i, (best, qty, aggressor, tid)) in block_trades.into_iter().enumerate() {
                let t = Trade {
                    prov: prov(&m.spec, Seq::new(block, index_u32(i)), ts, ingest, flags),
                    price: Price::new(best, ps).map_err(model)?,
                    size: Size::new(qty, ss).map_err(model)?,
                    aggressor,
                    trade_id: TradeId(tid),
                };
                t.validate().map_err(model)?;
                trades.push(t);
            }
            for (i, (oid, side, price, action)) in raw_events.into_iter().enumerate() {
                let e = BookEvent {
                    prov: prov(&m.spec, Seq::new(block, index_u32(i)), ts, ingest, flags),
                    order_id: OrderId(oid),
                    side,
                    price: Price::new(price, ps).map_err(model)?,
                    action,
                };
                e.validate().map_err(model)?;
                block_events.push(e);
            }
            events.extend(block_events);

            if after_gap || (b + 1) % cfg.snapshot_every.max(1) == 0 {
                let s = BookSnapshot {
                    prov: prov(&m.spec, Seq::end_of_block(block), ts, ingest, base_flags),
                    depth: book.snapshot(ps, ss)?,
                };
                s.validate().map_err(model)?;
                snapshots.push(s);
            }
            after_gap = false;

            // Drift the mid, staying inside the spread.
            m.mid += rng.range(-1, 1) * m.tick;
            if let (Some(bb), Some(ba)) = (book.best_bid(), book.best_ask())
                && (m.mid <= bb || m.mid >= ba)
            {
                m.mid = bb + (ba - bb) / 2;
            }
        }

        let day_of = |ms: i64| ms.div_euclid(DAY_MS);
        let id = m.spec.id.clone();
        let mut put = |kind: RecordKind, day: i64, r: Record| {
            data.partitions
                .entry((kind, id.clone(), day))
                .or_default()
                .push(r);
        };
        for t in &trades {
            put(
                RecordKind::Trade,
                day_of(t.prov.ts_source_ms.0),
                Record::Trade(t.clone()),
            );
        }
        for e in events {
            put(
                RecordKind::BookEvent,
                day_of(e.prov.ts_source_ms.0),
                Record::BookEvent(e),
            );
        }
        for s in snapshots {
            put(
                RecordKind::BookSnapshot,
                day_of(s.prov.ts_source_ms.0),
                Record::BookSnapshot(s),
            );
        }
        for bar in bars(&trades)? {
            put(RecordKind::Bar, day_of(bar.start_ms.0), Record::Bar(bar));
        }
        for f in footprints(&trades)? {
            put(
                RecordKind::Footprint,
                day_of(f.start_ms.0),
                Record::Footprint(f),
            );
        }
        for p in profiles(&trades, m.tick * 10)? {
            put(
                RecordKind::VolumeProfile,
                day_of(p.start_ms.0),
                Record::VolumeProfile(p),
            );
        }
    }
    for records in data.partitions.values_mut() {
        records.sort_by(|a, b| {
            (a.prov().instrument.as_str(), a.prov().seq)
                .cmp(&(b.prov().instrument.as_str(), b.prov().seq))
        });
    }
    Ok(data)
}

fn header(method: &str, inputs: &[Record]) -> Result<DerivedHeader, SynthError> {
    Ok(DerivedHeader {
        method: MethodId::new(method).map_err(model)?,
        inputs_hash: mesh_types::content_hash(inputs.iter()).map_err(model)?,
        input_count: u64::try_from(inputs.len()).unwrap_or(u64::MAX),
    })
}

fn group_by<K: Ord>(trades: &[Trade], key: impl Fn(&Trade) -> K) -> BTreeMap<K, Vec<&Trade>> {
    let mut m: BTreeMap<K, Vec<&Trade>> = BTreeMap::new();
    for t in trades {
        m.entry(key(t)).or_default().push(t);
    }
    m
}

fn sum(sizes: impl Iterator<Item = i64>, scale: u8) -> Result<Size, SynthError> {
    Size::new(sizes.sum(), scale).map_err(model)
}

/// Bars for every interval, method `bars@1` (observation-v0 § 2.5).
fn bars(trades: &[Trade]) -> Result<Vec<Bar>, SynthError> {
    let mut out = Vec::new();
    for interval in BarInterval::ALL {
        for (start, group) in group_by(trades, |t| interval.align(t.prov.ts_source_ms).0) {
            let (Some(first), Some(last)) = (group.first(), group.last()) else {
                continue;
            };
            let ss = first.size.scale();
            let inputs: Vec<Record> = group.iter().map(|t| Record::Trade((*t).clone())).collect();
            let vol = |side: Option<AggressorSide>| {
                group
                    .iter()
                    .filter(move |t| side.is_none_or(|s| t.aggressor == s))
                    .map(|t| t.size.mantissa())
            };
            let bar = Bar {
                prov: last.prov.clone(),
                derived: header("bars@1", &inputs)?,
                interval,
                start_ms: UnixMillis(start),
                open: first.price,
                high: group.iter().map(|t| t.price).max().unwrap_or(first.price),
                low: group.iter().map(|t| t.price).min().unwrap_or(first.price),
                close: last.price,
                volume: sum(vol(None), ss)?,
                buy_volume: sum(vol(Some(AggressorSide::Buy)), ss)?,
                sell_volume: sum(vol(Some(AggressorSide::Sell)), ss)?,
                trade_count: u32::try_from(group.len()).unwrap_or(u32::MAX),
            };
            bar.validate().map_err(model)?;
            out.push(bar);
        }
    }
    Ok(out)
}

/// One-minute footprints (synthetic method `footprint@1`).
fn footprints(trades: &[Trade]) -> Result<Vec<Footprint>, SynthError> {
    let mut out = Vec::new();
    for (start, group) in group_by(trades, |t| BarInterval::M1.align(t.prov.ts_source_ms).0) {
        let Some(last) = group.last() else { continue };
        let ss = last.size.scale();
        let mut levels: BTreeMap<i64, (i64, i64, u32, u8)> = BTreeMap::new();
        for t in &group {
            let e = levels
                .entry(t.price.mantissa())
                .or_insert((0, 0, 0, t.price.scale()));
            match t.aggressor {
                AggressorSide::Sell => e.0 += t.size.mantissa(),
                AggressorSide::Buy => e.1 += t.size.mantissa(),
                AggressorSide::Unknown => {}
            }
            e.2 += 1;
        }
        let inputs: Vec<Record> = group.iter().map(|t| Record::Trade((*t).clone())).collect();
        let f = Footprint {
            prov: last.prov.clone(),
            derived: header("footprint@1", &inputs)?,
            interval: BarInterval::M1,
            start_ms: UnixMillis(start),
            levels: levels
                .into_iter()
                .map(|(p, (bid, ask, n, ps))| {
                    Ok(FootprintLevel {
                        price: Price::new(p, ps).map_err(model)?,
                        bid_volume: Size::new(bid, ss).map_err(model)?,
                        ask_volume: Size::new(ask, ss).map_err(model)?,
                        trade_count: n,
                    })
                })
                .collect::<Result<_, SynthError>>()?,
        };
        f.validate().map_err(model)?;
        out.push(f);
    }
    Ok(out)
}

/// Hourly volume profiles (synthetic method `volume-profile@1`).
fn profiles(trades: &[Trade], bucket: i64) -> Result<Vec<VolumeProfile>, SynthError> {
    let mut out = Vec::new();
    for (start, group) in group_by(trades, |t| BarInterval::H1.align(t.prov.ts_source_ms).0) {
        let Some(last) = group.last() else { continue };
        let ss = last.size.scale();
        let ps = last.price.scale();
        let mut levels: BTreeMap<i64, (i64, i64, i64)> = BTreeMap::new();
        for t in &group {
            let p = t.price.mantissa().div_euclid(bucket) * bucket;
            let e = levels.entry(p).or_default();
            e.0 += t.size.mantissa();
            match t.aggressor {
                AggressorSide::Buy => e.1 += t.size.mantissa(),
                AggressorSide::Sell => e.2 += t.size.mantissa(),
                AggressorSide::Unknown => {}
            }
        }
        let inputs: Vec<Record> = group.iter().map(|t| Record::Trade((*t).clone())).collect();
        let v = VolumeProfile {
            prov: last.prov.clone(),
            derived: header("volume-profile@1", &inputs)?,
            start_ms: UnixMillis(start),
            end_ms: UnixMillis(start + HOUR_MS),
            bucket: Price::new(bucket, ps).map_err(model)?,
            levels: levels
                .into_iter()
                .map(|(p, (v, b, s))| {
                    Ok(ProfileLevel {
                        price: Price::new(p, ps).map_err(model)?,
                        volume: Size::new(v, ss).map_err(model)?,
                        buy_volume: Size::new(b, ss).map_err(model)?,
                        sell_volume: Size::new(s, ss).map_err(model)?,
                    })
                })
                .collect::<Result<_, SynthError>>()?,
        };
        v.validate().map_err(model)?;
        out.push(v);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Parquet writing
// ---------------------------------------------------------------------------

fn list_item(schema: &arrow_schema::Schema, name: &str) -> Result<(FieldRef, Fields), SynthError> {
    let field = schema.field_with_name(name)?;
    if let DataType::List(item) = field.data_type()
        && let DataType::Struct(fields) = item.data_type()
    {
        return Ok((item.clone(), fields.clone()));
    }
    Err(SynthError::Model(format!(
        "{name} is not a list of structs"
    )))
}

fn list_column(
    item: FieldRef,
    fields: Fields,
    children: Vec<ArrayRef>,
    offsets: Vec<i32>,
) -> Result<ArrayRef, SynthError> {
    let values = StructArray::try_new(fields, children, None)?;
    let list = ListArray::try_new(
        item,
        arrow_buffer::OffsetBuffer::new(offsets.into()),
        Arc::new(values),
        None,
    )?;
    Ok(Arc::new(list))
}

fn ts_ms(v: Vec<i64>) -> ArrayRef {
    Arc::new(PrimitiveArray::<TimestampMillisecondType>::from(v).with_timezone("UTC"))
}

fn ts_ms_opt(v: Vec<Option<i64>>) -> ArrayRef {
    Arc::new(PrimitiveArray::<TimestampMillisecondType>::from(v).with_timezone("UTC"))
}

fn utf8<S: AsRef<str>>(v: impl IntoIterator<Item = S>) -> ArrayRef {
    Arc::new(StringArray::from_iter_values(v))
}

fn i64s(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

/// Converts records of one kind to a `RecordBatch` with
/// `mesh_types::arrow::schema(kind)`. Scales are the partition's.
///
/// # Errors
///
/// Fails if a record is of another kind or arrays do not fit the schema.
#[allow(clippy::too_many_lines)]
pub fn to_batch(
    kind: RecordKind,
    records: &[Record],
    price_scale: u8,
    size_scale: u8,
) -> Result<RecordBatch, SynthError> {
    let schema = Arc::new(mesh_types::arrow::schema(kind));
    let provs: Vec<&Provenance> = records.iter().map(Record::prov).collect();
    let mut cols: Vec<ArrayRef> = vec![
        utf8(provs.iter().map(|p| p.instrument.as_str())),
        utf8(provs.iter().map(|p| p.venue.as_str())),
        ts_ms(provs.iter().map(|p| p.ts_source_ms.0).collect()),
        Arc::new(
            PrimitiveArray::<TimestampNanosecondType>::from(
                provs.iter().map(|p| p.ts_ingest_ns.0).collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        Arc::new(PrimitiveArray::<UInt64Type>::from(
            provs.iter().map(|p| p.seq.block_number).collect::<Vec<_>>(),
        )),
        Arc::new(PrimitiveArray::<UInt32Type>::from(
            provs.iter().map(|p| p.seq.event_index).collect::<Vec<_>>(),
        )),
        utf8(provs.iter().map(|p| p.admission.as_str())),
        Arc::new(PrimitiveArray::<UInt32Type>::from(
            provs.iter().map(|p| p.flags.bits()).collect::<Vec<_>>(),
        )),
    ];
    let wrong = || SynthError::Model(format!("record is not a {kind}"));
    if kind.is_derived() {
        let headers: Vec<&DerivedHeader> = records
            .iter()
            .map(|r| match r {
                Record::Bar(b) => Ok(&b.derived),
                Record::Footprint(f) => Ok(&f.derived),
                Record::VolumeProfile(v) => Ok(&v.derived),
                _ => Err(wrong()),
            })
            .collect::<Result<_, _>>()?;
        cols.push(utf8(headers.iter().map(|h| h.method.as_str())));
        cols.push(Arc::new(FixedSizeBinaryArray::try_from_iter(
            headers.iter().map(|h| h.inputs_hash.0.0),
        )?));
        cols.push(Arc::new(PrimitiveArray::<UInt64Type>::from(
            headers.iter().map(|h| h.input_count).collect::<Vec<_>>(),
        )));
    }
    match kind {
        RecordKind::Trade => {
            let t: Vec<&Trade> = records
                .iter()
                .map(|r| match r {
                    Record::Trade(t) => Ok(t),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()?;
            cols.push(i64s(t.iter().map(|x| x.price.mantissa()).collect()));
            cols.push(i64s(t.iter().map(|x| x.size.mantissa()).collect()));
            cols.push(utf8(t.iter().map(|x| x.aggressor.as_str())));
            cols.push(Arc::new(PrimitiveArray::<UInt64Type>::from(
                t.iter().map(|x| x.trade_id.0).collect::<Vec<_>>(),
            )));
        }
        RecordKind::BookEvent => {
            let e: Vec<&BookEvent> = records
                .iter()
                .map(|r| match r {
                    Record::BookEvent(e) => Ok(e),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()?;
            cols.push(Arc::new(PrimitiveArray::<UInt64Type>::from(
                e.iter().map(|x| x.order_id.0).collect::<Vec<_>>(),
            )));
            cols.push(utf8(e.iter().map(|x| x.side.as_str())));
            cols.push(i64s(e.iter().map(|x| x.price.mantissa()).collect()));
            cols.push(utf8(e.iter().map(|x| x.action.as_str())));
            cols.push(Arc::new(Int64Array::from(
                e.iter()
                    .map(|x| x.action.size_after().map(Size::mantissa))
                    .collect::<Vec<_>>(),
            )));
            cols.push(Arc::new(Int64Array::from(
                e.iter()
                    .map(|x| match x.action {
                        BookAction::Modify { orig_size, .. }
                        | BookAction::Fill { orig_size, .. } => orig_size.map(Size::mantissa),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )));
        }
        RecordKind::BookSnapshot => {
            let s: Vec<&BookSnapshot> = records
                .iter()
                .map(|r| match r {
                    Record::BookSnapshot(s) => Ok(s),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()?;
            cols.push(utf8(s.iter().map(|x| match x.depth {
                SnapshotDepth::L4 { .. } => "l4",
                SnapshotDepth::L2 { .. } => "l2",
            })));
            for side in ["bids", "asks"] {
                let (item, fields) = list_item(&schema, side)?;
                let mut offsets = vec![0i32];
                let (mut oid, mut px, mut sz, mut ts, mut cnt) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for snap in &s {
                    match &snap.depth {
                        SnapshotDepth::L4 { bids, asks } => {
                            for o in if side == "bids" { bids } else { asks } {
                                oid.push(Some(o.order_id.0));
                                px.push(o.price.mantissa());
                                sz.push(o.size.mantissa());
                                ts.push(Some(o.ts_order_ms.0));
                                cnt.push(None);
                            }
                        }
                        SnapshotDepth::L2 { bids, asks } => {
                            for l in if side == "bids" { bids } else { asks } {
                                oid.push(None);
                                px.push(l.price.mantissa());
                                sz.push(l.size.mantissa());
                                ts.push(None);
                                cnt.push(Some(l.order_count));
                            }
                        }
                    }
                    offsets.push(i32::try_from(px.len()).map_err(model)?);
                }
                cols.push(list_column(
                    item,
                    fields,
                    vec![
                        Arc::new(PrimitiveArray::<UInt64Type>::from(oid)),
                        i64s(px),
                        i64s(sz),
                        ts_ms_opt(ts),
                        Arc::new(PrimitiveArray::<UInt32Type>::from(cnt)),
                    ],
                    offsets,
                )?);
            }
        }
        RecordKind::Bar => {
            let b: Vec<&Bar> = records
                .iter()
                .map(|r| match r {
                    Record::Bar(b) => Ok(b),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()?;
            cols.push(utf8(b.iter().map(|x| x.interval.as_str())));
            cols.push(ts_ms(b.iter().map(|x| x.start_ms.0).collect()));
            for f in [
                |x: &Bar| x.open.mantissa(),
                |x: &Bar| x.high.mantissa(),
                |x: &Bar| x.low.mantissa(),
                |x: &Bar| x.close.mantissa(),
                |x: &Bar| x.volume.mantissa(),
                |x: &Bar| x.buy_volume.mantissa(),
                |x: &Bar| x.sell_volume.mantissa(),
            ] {
                cols.push(i64s(b.iter().map(|x| f(x)).collect()));
            }
            cols.push(Arc::new(PrimitiveArray::<UInt32Type>::from(
                b.iter().map(|x| x.trade_count).collect::<Vec<_>>(),
            )));
        }
        RecordKind::Footprint => {
            let f: Vec<&Footprint> = records
                .iter()
                .map(|r| match r {
                    Record::Footprint(f) => Ok(f),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()?;
            cols.push(utf8(f.iter().map(|x| x.interval.as_str())));
            cols.push(ts_ms(f.iter().map(|x| x.start_ms.0).collect()));
            let (item, fields) = list_item(&schema, "levels")?;
            let mut offsets = vec![0i32];
            let (mut px, mut bid, mut ask, mut n) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for x in &f {
                for l in &x.levels {
                    px.push(l.price.mantissa());
                    bid.push(l.bid_volume.mantissa());
                    ask.push(l.ask_volume.mantissa());
                    n.push(l.trade_count);
                }
                offsets.push(i32::try_from(px.len()).map_err(model)?);
            }
            cols.push(list_column(
                item,
                fields,
                vec![
                    i64s(px),
                    i64s(bid),
                    i64s(ask),
                    Arc::new(PrimitiveArray::<UInt32Type>::from(n)),
                ],
                offsets,
            )?);
        }
        RecordKind::VolumeProfile => {
            let v: Vec<&VolumeProfile> = records
                .iter()
                .map(|r| match r {
                    Record::VolumeProfile(v) => Ok(v),
                    _ => Err(wrong()),
                })
                .collect::<Result<_, _>>()?;
            cols.push(ts_ms(v.iter().map(|x| x.start_ms.0).collect()));
            cols.push(ts_ms(v.iter().map(|x| x.end_ms.0).collect()));
            cols.push(i64s(v.iter().map(|x| x.bucket.mantissa()).collect()));
            let (item, fields) = list_item(&schema, "levels")?;
            let mut offsets = vec![0i32];
            let (mut px, mut vol, mut buy, mut sell) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for x in &v {
                for l in &x.levels {
                    px.push(l.price.mantissa());
                    vol.push(l.volume.mantissa());
                    buy.push(l.buy_volume.mantissa());
                    sell.push(l.sell_volume.mantissa());
                }
                offsets.push(i32::try_from(px.len()).map_err(model)?);
            }
            cols.push(list_column(
                item,
                fields,
                vec![i64s(px), i64s(vol), i64s(buy), i64s(sell)],
                offsets,
            )?);
        }
        other => {
            return Err(SynthError::Model(format!(
                "{other} records are not generated by this module"
            )));
        }
    }
    if kind != RecordKind::SwapPrint {
        let n = records.len();
        cols.push(Arc::new(PrimitiveArray::<UInt8Type>::from(vec![
            price_scale;
            n
        ])));
        cols.push(Arc::new(PrimitiveArray::<UInt8Type>::from(vec![
            size_scale;
            n
        ])));
    }
    Ok(RecordBatch::try_new(schema, cols)?)
}

/// Writes one Parquet partition file with the archive-layout-v0 settings.
///
/// # Errors
///
/// I/O, Arrow or Parquet errors.
pub fn write_partition_file(
    path: &Path,
    kind: RecordKind,
    records: &[Record],
    price_scale: u8,
    size_scale: u8,
) -> Result<(), SynthError> {
    let batch = to_batch(kind, records, price_scale, size_scale)?;
    let content_hash = mesh_types::content_hash(records.iter()).map_err(model)?;
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(9)?))
        .set_max_row_group_row_count(Some(131_072))
        .set_data_page_size_limit(1 << 20)
        .set_dictionary_enabled(true)
        .set_statistics_enabled(EnabledStatistics::Page)
        .set_bloom_filter_enabled(false)
        .set_created_by("bleh-archive".to_owned())
        .set_key_value_metadata(Some(vec![
            KeyValue::new(
                "bleh.schema".to_owned(),
                mesh_types::OBSERVATION_SCHEMA.to_owned(),
            ),
            KeyValue::new("bleh.kind".to_owned(), kind.as_str().to_owned()),
            KeyValue::new("bleh.content_hash".to_owned(), content_hash.to_string()),
            KeyValue::new("bleh.record_count".to_owned(), records.len().to_string()),
        ]))
        .build();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path)?;
    let options = ArrowWriterOptions::new()
        .with_properties(props)
        .with_skip_arrow_metadata(true);
    let mut writer = ArrowWriter::try_new_with_options(file, batch.schema(), options)?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(())
}

/// What [`write_archive`] wrote.
#[derive(Debug, Clone, Default)]
pub struct WriteSummary {
    /// Parquet files written (paths relative to the archive root).
    pub files: Vec<String>,
    /// Records written.
    pub records: usize,
    /// Days with a manifest.
    pub days: Vec<String>,
    /// The registry file.
    pub registry_file: PathBuf,
    /// The address that signed the demo manifests.
    pub signer: String,
}

/// RFC 4648 base32, lowercase, no padding (as used in CIDv1).
fn base32_lower(data: &[u8]) -> String {
    const A: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::new();
    let mut buf: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(A[((buf >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(A[((buf << (5 - bits)) & 31) as usize]));
    }
    out
}

/// CIDv1 for a file that fits one 1 MiB chunk: codec raw (0x55), sha2-256.
fn raw_cid(bytes: &[u8]) -> String {
    let digest = mesh_types::Sha256Digest::of(bytes);
    let mut cid = vec![0x01, 0x55, 0x12, 0x20];
    cid.extend_from_slice(&digest.0);
    format!("b{}", base32_lower(&cid))
}

/// The deterministic key that signs demo manifests. It is derived from a
/// public string, so it protects nothing: demo manifests are for exercising
/// the API only.
///
/// # Errors
///
/// Never in practice.
pub fn demo_signing_key() -> Result<SigningKey, SynthError> {
    let seed = mesh_types::Sha256Digest::of(b"bleh history-api synthetic demo manifests");
    SigningKey::from_bytes(&seed.0).map_err(model)
}

/// Writes the synthetic archive under `root/<archive>`, plus
/// `root/registry.json`.
///
/// # Errors
///
/// I/O, Arrow or Parquet errors.
pub fn write_archive(
    root: &Path,
    archive: &str,
    data: &SynthData,
) -> Result<WriteSummary, SynthError> {
    let base = root.join(archive);
    let mut summary = WriteSummary::default();
    let mut by_day: BTreeMap<i64, Vec<ManifestFile>> = BTreeMap::new();
    for ((kind, instrument, day), records) in &data.partitions {
        let spec = data
            .registry
            .get(instrument)
            .ok_or_else(|| SynthError::Model(format!("{instrument} not in registry")))?;
        let rel = format!(
            "{}/part-00000.parquet",
            Layout::partition_dir(*kind, instrument, *day)
        );
        let path = base.join(&rel);
        write_partition_file(&path, *kind, records, spec.price_scale, spec.size_scale)?;
        let bytes = fs::read(&path)?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        by_day.entry(*day).or_default().push(ManifestFile {
            path: rel.clone(),
            size,
            sha256: mesh_types::Sha256Digest::of(&bytes),
            content_hash: Some(mesh_types::content_hash(records.iter()).map_err(model)?),
            record_count: Some(u64::try_from(records.len()).unwrap_or(u64::MAX)),
            ipfs: IpfsRef {
                cid: raw_cid(&bytes),
                codec: IpfsCodec::Raw,
                chunker: "size-1048576".to_owned(),
                raw_leaves: true,
            },
            bt_pieces_root: None,
        });
        summary.records += records.len();
        summary.files.push(rel);
    }
    let key = demo_signing_key()?;
    summary.signer = key.address().to_string();
    let mut prev: Option<mesh_types::Sha256Digest> = None;
    for (seq, (day, mut files)) in by_day.into_iter().enumerate() {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let date = day_string(day);
        // Placeholder: no torrent is built for the demo.
        let mut joined = Vec::new();
        for f in &files {
            joined.extend_from_slice(&f.sha256.0);
        }
        let infohash = mesh_types::Sha256Digest::of(&joined);
        let manifest = Manifest {
            format: MANIFEST_FORMAT.to_owned(),
            archive: archive.to_owned(),
            date: date.clone(),
            sequence: u64::try_from(seq).unwrap_or(0),
            created_ns: UnixNanos((day + 1) * DAY_MS * 1_000_000),
            previous_manifest_sha256: prev,
            files,
            torrent: TorrentInfo {
                infohash_v2: infohash,
                magnet: format!(
                    "magnet:?xt=urn:btmh:1220{infohash}&dn={archive}-{date}&ws=https%3A%2F%2Fdemo.invalid%2F"
                ),
            },
            web_seeds: vec!["https://demo.invalid/".to_owned()],
            signer: key.address(),
            next_signer: None,
            notices: Vec::new(),
        };
        let signed: SignedManifest = manifest.sign(&key).map_err(model)?;
        prev = Some(signed.sha256().map_err(model)?);
        let dir = base.join("_manifests").join(&date);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&signed).map_err(model)?,
        )?;
        summary.days.push(date);
    }
    let reg_path = root.join("registry.json");
    fs::write(
        &reg_path,
        serde_json::to_vec_pretty(&data.registry).map_err(model)?,
    )?;
    summary.registry_file = reg_path;
    Ok(summary)
}

/// The venue of the fictional markets.
///
/// # Errors
///
/// Never in practice.
pub fn venue() -> Result<VenueId, SynthError> {
    VenueId::new("hyperliquid").map_err(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> SynthConfig {
        SynthConfig {
            hours: 3,
            block_ms: 20_000,
            snapshot_every: 30,
            ..SynthConfig::default()
        }
    }

    #[test]
    fn generation_is_deterministic_and_valid() {
        let a = generate(&small()).unwrap();
        let b = generate(&small()).unwrap();
        assert_eq!(a.partitions.len(), b.partitions.len());
        for (k, v) in &a.partitions {
            assert_eq!(Some(v), b.partitions.get(k));
            for r in v {
                mesh_types::allowlist::check_record(r).unwrap();
            }
            for w in v.windows(2) {
                assert!(w[0].prov().seq <= w[1].prov().seq);
            }
        }
        let syn = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        assert!(!a.records(RecordKind::Trade, &syn).is_empty());
        assert!(!a.records(RecordKind::BookSnapshot, &syn).is_empty());
        // The gap: no syn:IDX record in hour 1, and gap_before right after.
        let idx = InstrumentId::new("hyperliquid:perp:syn:IDX").unwrap();
        let cfg = small();
        let gap_start = cfg.start_ms + HOUR_MS;
        let events = a.records(RecordKind::BookEvent, &idx);
        assert!(events.iter().all(|r| {
            let t = r.prov().ts_source_ms.0;
            t < gap_start || t >= gap_start + HOUR_MS
        }));
        let first_after = events
            .iter()
            .find(|r| r.prov().ts_source_ms.0 >= gap_start + HOUR_MS)
            .unwrap();
        assert!(first_after.prov().flags.contains(Flag::GapBefore));
    }

    #[test]
    fn cid_and_base32() {
        assert_eq!(base32_lower(b"f"), "my");
        assert_eq!(base32_lower(b"foobar"), "mzxw6ytboi");
        let cid = raw_cid(b"");
        assert!(cid.starts_with("bafk"));
        // CIDv1 raw of the empty input, as `ipfs add --cid-version=1 --raw-leaves`.
        assert_eq!(
            cid,
            "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
        );
    }

    #[test]
    fn writes_partitions_and_manifests() {
        let dir = std::env::temp_dir().join(format!("bleh-synth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let data = generate(&small()).unwrap();
        let s = write_archive(&dir, "hyperliquid-normalized", &data).unwrap();
        assert!(!s.files.is_empty());
        assert_eq!(s.days.len(), 2);
        let m: SignedManifest = serde_json::from_slice(
            &fs::read(
                dir.join("hyperliquid-normalized/_manifests")
                    .join(&s.days[1])
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        mesh_types::manifest::verify_signed(&m, &[demo_signing_key().unwrap().address()]).unwrap();
        let reg: Registry = serde_json::from_slice(&fs::read(&s.registry_file).unwrap()).unwrap();
        assert_eq!(reg.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
