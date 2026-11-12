//! Reading partition rows back into `mesh_types` records.
//!
//! Each kind has a fixed SELECT list (timestamps converted to epoch
//! integers in SQL, nested lists converted to JSON) and a raw row struct
//! filled by the DuckDB row mapper. Raw rows are then converted to records
//! outside the mapper, so a corrupt partition surfaces as an `internal`
//! error with a clear log line instead of a panic.

use duckdb::Row;
use mesh_types::{
    AdmissionId, AggressorSide, Bar, BarInterval, BookAction, BookEvent, BookSide, BookSnapshot,
    ContentHash, DerivedHeader, Flags, Footprint, FootprintLevel, InstrumentId, Level, MethodId,
    OrderId, Price, ProfileLevel, Provenance, RecordKind, RestingOrder, Seq, Sha256Digest, Size,
    SnapshotDepth, Trade, TradeId, UnixMillis, UnixNanos, VenueId, VolumeProfile,
};
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};

/// Provenance columns, in this order, at the start of every SELECT list.
pub const PROV_COLS: &str = "instrument, venue, epoch_ms(ts_source_ms), epoch_ns(ts_ingest_ns), \
     block_number, event_index, admission, flags";
const PROV_N: usize = 8;

/// The SELECT list for a kind.
#[must_use]
pub fn select_list(kind: RecordKind) -> String {
    let rest = match kind {
        RecordKind::Trade => "price, size, aggressor, trade_id, price_scale, size_scale",
        RecordKind::BookEvent => {
            "order_id, side, price, action, size, orig_size, price_scale, size_scale"
        }
        RecordKind::BookSnapshot => {
            "depth, \
             to_json(list_transform(bids, lambda x: {'o': x.order_id, 'p': x.price, 's': x.size, 't': epoch_ms(x.ts_order_ms), 'c': x.order_count}))::VARCHAR, \
             to_json(list_transform(asks, lambda x: {'o': x.order_id, 'p': x.price, 's': x.size, 't': epoch_ms(x.ts_order_ms), 'c': x.order_count}))::VARCHAR, \
             price_scale, size_scale"
        }
        RecordKind::Bar => {
            "method, inputs_hash, input_count, interval, epoch_ms(start_ms), open, high, low, close, \
             volume, buy_volume, sell_volume, trade_count, price_scale, size_scale"
        }
        RecordKind::Footprint => {
            "method, inputs_hash, input_count, interval, epoch_ms(start_ms), \
             to_json(levels)::VARCHAR, price_scale, size_scale"
        }
        RecordKind::VolumeProfile => {
            "method, inputs_hash, input_count, epoch_ms(start_ms), epoch_ms(end_ms), bucket, \
             to_json(levels)::VARCHAR, price_scale, size_scale"
        }
        // Not served by the row mappers; listed for exhaustiveness.
        _ => "repository",
    };
    format!("{PROV_COLS}, {rest}")
}

/// Raw provenance columns.
#[derive(Debug, Clone)]
pub struct RawProv {
    instrument: String,
    venue: String,
    ts_ms: i64,
    ingest_ns: i64,
    block: u64,
    index: u32,
    admission: String,
    flags: u32,
}

fn raw_prov(r: &Row<'_>) -> duckdb::Result<RawProv> {
    Ok(RawProv {
        instrument: r.get(0)?,
        venue: r.get(1)?,
        ts_ms: r.get(2)?,
        ingest_ns: r.get(3)?,
        block: r.get(4)?,
        index: r.get(5)?,
        admission: r.get(6)?,
        flags: r.get(7)?,
    })
}

fn corrupt(what: impl std::fmt::Display) -> ApiError {
    ApiError::internal(format!("corrupt partition row: {what}"))
}

impl RawProv {
    fn into_prov(self) -> ApiResult<Provenance> {
        Ok(Provenance::new(
            InstrumentId::new(self.instrument).map_err(corrupt)?,
            VenueId::new(self.venue).map_err(corrupt)?,
            Seq::new(self.block, self.index),
            UnixMillis(self.ts_ms),
            UnixNanos(self.ingest_ns),
            AdmissionId::new(self.admission).map_err(corrupt)?,
        )
        .with_flags(Flags::from_bits(self.flags).map_err(|e| corrupt(format!("{e:?}")))?))
    }
}

#[derive(Debug, Clone)]
struct RawDerived {
    method: String,
    inputs_hash: Vec<u8>,
    input_count: u64,
}

fn raw_derived(r: &Row<'_>) -> duckdb::Result<RawDerived> {
    Ok(RawDerived {
        method: r.get(PROV_N)?,
        inputs_hash: r.get(PROV_N + 1)?,
        input_count: r.get(PROV_N + 2)?,
    })
}

impl RawDerived {
    fn into_header(self) -> ApiResult<DerivedHeader> {
        let hash: [u8; 32] = self
            .inputs_hash
            .try_into()
            .map_err(|_| corrupt("inputs_hash is not 32 bytes"))?;
        Ok(DerivedHeader {
            method: MethodId::new(self.method).map_err(corrupt)?,
            inputs_hash: ContentHash(Sha256Digest(hash)),
            input_count: self.input_count,
        })
    }
}

fn px(m: i64, s: u8) -> ApiResult<Price> {
    Price::new(m, s).map_err(corrupt)
}

fn sz(m: i64, s: u8) -> ApiResult<Size> {
    Size::new(m, s).map_err(corrupt)
}

fn interval(s: &str) -> ApiResult<BarInterval> {
    BarInterval::ALL
        .iter()
        .find(|i| i.as_str() == s)
        .copied()
        .ok_or_else(|| corrupt(format!("interval {s}")))
}

/// A raw trade row.
#[derive(Debug, Clone)]
pub struct RawTrade {
    prov: RawProv,
    price: i64,
    size: i64,
    aggressor: String,
    trade_id: u64,
    ps: u8,
    ss: u8,
}

/// Row mapper for [`select_list`]`(Trade)`.
///
/// # Errors
///
/// DuckDB conversion errors.
pub fn map_trade(r: &Row<'_>) -> duckdb::Result<RawTrade> {
    let n = PROV_N;
    Ok(RawTrade {
        prov: raw_prov(r)?,
        price: r.get(n)?,
        size: r.get(n + 1)?,
        aggressor: r.get(n + 2)?,
        trade_id: r.get(n + 3)?,
        ps: r.get(n + 4)?,
        ss: r.get(n + 5)?,
    })
}

impl RawTrade {
    /// Converts to a [`Trade`].
    ///
    /// # Errors
    ///
    /// `internal` on corrupt values.
    pub fn into_record(self) -> ApiResult<Trade> {
        let aggressor = match self.aggressor.as_str() {
            "buy" => AggressorSide::Buy,
            "sell" => AggressorSide::Sell,
            "unknown" => AggressorSide::Unknown,
            other => return Err(corrupt(format!("aggressor {other}"))),
        };
        Ok(Trade {
            prov: self.prov.into_prov()?,
            price: px(self.price, self.ps)?,
            size: sz(self.size, self.ss)?,
            aggressor,
            trade_id: TradeId(self.trade_id),
        })
    }
}

/// A raw book event row.
#[derive(Debug, Clone)]
pub struct RawBookEvent {
    prov: RawProv,
    order_id: u64,
    side: String,
    price: i64,
    action: String,
    size: Option<i64>,
    orig_size: Option<i64>,
    ps: u8,
    ss: u8,
}

/// Row mapper for [`select_list`]`(BookEvent)`.
///
/// # Errors
///
/// DuckDB conversion errors.
pub fn map_book_event(r: &Row<'_>) -> duckdb::Result<RawBookEvent> {
    let n = PROV_N;
    Ok(RawBookEvent {
        prov: raw_prov(r)?,
        order_id: r.get(n)?,
        side: r.get(n + 1)?,
        price: r.get(n + 2)?,
        action: r.get(n + 3)?,
        size: r.get(n + 4)?,
        orig_size: r.get(n + 5)?,
        ps: r.get(n + 6)?,
        ss: r.get(n + 7)?,
    })
}

fn side(s: &str) -> ApiResult<BookSide> {
    match s {
        "bid" => Ok(BookSide::Bid),
        "ask" => Ok(BookSide::Ask),
        other => Err(corrupt(format!("side {other}"))),
    }
}

impl RawBookEvent {
    /// Converts to a [`BookEvent`].
    ///
    /// # Errors
    ///
    /// `internal` on corrupt values.
    pub fn into_record(self) -> ApiResult<BookEvent> {
        let ss = self.ss;
        let need = |v: Option<i64>| v.ok_or_else(|| corrupt("missing size"));
        let orig = self.orig_size.map(|m| sz(m, ss)).transpose()?;
        let action = match self.action.as_str() {
            "add" => BookAction::Add {
                size: sz(need(self.size)?, ss)?,
            },
            "modify" => BookAction::Modify {
                orig_size: orig,
                new_size: sz(need(self.size)?, ss)?,
            },
            "fill" => BookAction::Fill {
                orig_size: orig,
                new_size: sz(need(self.size)?, ss)?,
            },
            "cancel" => BookAction::Cancel {},
            other => return Err(corrupt(format!("action {other}"))),
        };
        Ok(BookEvent {
            prov: self.prov.into_prov()?,
            order_id: OrderId(self.order_id),
            side: side(&self.side)?,
            price: px(self.price, self.ps)?,
            action,
        })
    }
}

/// A raw snapshot row.
#[derive(Debug, Clone)]
pub struct RawSnapshot {
    prov: RawProv,
    depth: String,
    bids: String,
    asks: String,
    ps: u8,
    ss: u8,
}

/// Row mapper for [`select_list`]`(BookSnapshot)`.
///
/// # Errors
///
/// DuckDB conversion errors.
pub fn map_snapshot(r: &Row<'_>) -> duckdb::Result<RawSnapshot> {
    let n = PROV_N;
    Ok(RawSnapshot {
        prov: raw_prov(r)?,
        depth: r.get(n)?,
        bids: r.get(n + 1)?,
        asks: r.get(n + 2)?,
        ps: r.get(n + 3)?,
        ss: r.get(n + 4)?,
    })
}

#[derive(Debug, Deserialize)]
struct JsonEntry {
    o: Option<u64>,
    p: i64,
    s: i64,
    t: Option<i64>,
    c: Option<u32>,
}

impl RawSnapshot {
    /// Converts to a [`BookSnapshot`].
    ///
    /// # Errors
    ///
    /// `internal` on corrupt values.
    pub fn into_record(self) -> ApiResult<BookSnapshot> {
        let parse =
            |s: &str| -> ApiResult<Vec<JsonEntry>> { serde_json::from_str(s).map_err(corrupt) };
        let (bids, asks) = (parse(&self.bids)?, parse(&self.asks)?);
        let (ps, ss) = (self.ps, self.ss);
        let depth = match self.depth.as_str() {
            "l4" => {
                let conv = |v: Vec<JsonEntry>| -> ApiResult<Vec<RestingOrder>> {
                    v.into_iter()
                        .map(|e| {
                            Ok(RestingOrder {
                                order_id: OrderId(e.o.ok_or_else(|| corrupt("l4 order_id"))?),
                                price: px(e.p, ps)?,
                                size: sz(e.s, ss)?,
                                ts_order_ms: UnixMillis(
                                    e.t.ok_or_else(|| corrupt("l4 ts_order_ms"))?,
                                ),
                            })
                        })
                        .collect()
                };
                SnapshotDepth::L4 {
                    bids: conv(bids)?,
                    asks: conv(asks)?,
                }
            }
            "l2" => {
                let conv = |v: Vec<JsonEntry>| -> ApiResult<Vec<Level>> {
                    v.into_iter()
                        .map(|e| {
                            Ok(Level {
                                price: px(e.p, ps)?,
                                size: sz(e.s, ss)?,
                                order_count: e.c.ok_or_else(|| corrupt("l2 order_count"))?,
                            })
                        })
                        .collect()
                };
                SnapshotDepth::L2 {
                    bids: conv(bids)?,
                    asks: conv(asks)?,
                }
            }
            other => return Err(corrupt(format!("depth {other}"))),
        };
        Ok(BookSnapshot {
            prov: self.prov.into_prov()?,
            depth,
        })
    }
}

/// A raw bar row.
#[derive(Debug, Clone)]
pub struct RawBar {
    prov: RawProv,
    derived: RawDerived,
    interval: String,
    start_ms: i64,
    ohlc: [i64; 4],
    vols: [i64; 3],
    trade_count: u32,
    ps: u8,
    ss: u8,
}

/// Row mapper for [`select_list`]`(Bar)`.
///
/// # Errors
///
/// DuckDB conversion errors.
pub fn map_bar(r: &Row<'_>) -> duckdb::Result<RawBar> {
    let n = PROV_N + 3;
    Ok(RawBar {
        prov: raw_prov(r)?,
        derived: raw_derived(r)?,
        interval: r.get(n)?,
        start_ms: r.get(n + 1)?,
        ohlc: [r.get(n + 2)?, r.get(n + 3)?, r.get(n + 4)?, r.get(n + 5)?],
        vols: [r.get(n + 6)?, r.get(n + 7)?, r.get(n + 8)?],
        trade_count: r.get(n + 9)?,
        ps: r.get(n + 10)?,
        ss: r.get(n + 11)?,
    })
}

impl RawBar {
    /// Converts to a [`Bar`].
    ///
    /// # Errors
    ///
    /// `internal` on corrupt values.
    pub fn into_record(self) -> ApiResult<Bar> {
        let (ps, ss) = (self.ps, self.ss);
        Ok(Bar {
            prov: self.prov.into_prov()?,
            derived: self.derived.into_header()?,
            interval: interval(&self.interval)?,
            start_ms: UnixMillis(self.start_ms),
            open: px(self.ohlc[0], ps)?,
            high: px(self.ohlc[1], ps)?,
            low: px(self.ohlc[2], ps)?,
            close: px(self.ohlc[3], ps)?,
            volume: sz(self.vols[0], ss)?,
            buy_volume: sz(self.vols[1], ss)?,
            sell_volume: sz(self.vols[2], ss)?,
            trade_count: self.trade_count,
        })
    }
}

/// A raw footprint row.
#[derive(Debug, Clone)]
pub struct RawFootprint {
    prov: RawProv,
    derived: RawDerived,
    interval: String,
    start_ms: i64,
    levels: String,
    ps: u8,
    ss: u8,
}

/// Row mapper for [`select_list`]`(Footprint)`.
///
/// # Errors
///
/// DuckDB conversion errors.
pub fn map_footprint(r: &Row<'_>) -> duckdb::Result<RawFootprint> {
    let n = PROV_N + 3;
    Ok(RawFootprint {
        prov: raw_prov(r)?,
        derived: raw_derived(r)?,
        interval: r.get(n)?,
        start_ms: r.get(n + 1)?,
        levels: r.get(n + 2)?,
        ps: r.get(n + 3)?,
        ss: r.get(n + 4)?,
    })
}

#[derive(Debug, Deserialize)]
struct JsonFootprintLevel {
    price: i64,
    bid_volume: i64,
    ask_volume: i64,
    trade_count: u32,
}

impl RawFootprint {
    /// Converts to a [`Footprint`].
    ///
    /// # Errors
    ///
    /// `internal` on corrupt values.
    pub fn into_record(self) -> ApiResult<Footprint> {
        let (ps, ss) = (self.ps, self.ss);
        let levels: Vec<JsonFootprintLevel> =
            serde_json::from_str(&self.levels).map_err(corrupt)?;
        Ok(Footprint {
            prov: self.prov.into_prov()?,
            derived: self.derived.into_header()?,
            interval: interval(&self.interval)?,
            start_ms: UnixMillis(self.start_ms),
            levels: levels
                .into_iter()
                .map(|l| {
                    Ok(FootprintLevel {
                        price: px(l.price, ps)?,
                        bid_volume: sz(l.bid_volume, ss)?,
                        ask_volume: sz(l.ask_volume, ss)?,
                        trade_count: l.trade_count,
                    })
                })
                .collect::<ApiResult<_>>()?,
        })
    }
}

/// A raw volume profile row.
#[derive(Debug, Clone)]
pub struct RawProfile {
    prov: RawProv,
    derived: RawDerived,
    start_ms: i64,
    end_ms: i64,
    bucket: i64,
    levels: String,
    ps: u8,
    ss: u8,
}

/// Row mapper for [`select_list`]`(VolumeProfile)`.
///
/// # Errors
///
/// DuckDB conversion errors.
pub fn map_profile(r: &Row<'_>) -> duckdb::Result<RawProfile> {
    let n = PROV_N + 3;
    Ok(RawProfile {
        prov: raw_prov(r)?,
        derived: raw_derived(r)?,
        start_ms: r.get(n)?,
        end_ms: r.get(n + 1)?,
        bucket: r.get(n + 2)?,
        levels: r.get(n + 3)?,
        ps: r.get(n + 4)?,
        ss: r.get(n + 5)?,
    })
}

#[derive(Debug, Deserialize)]
struct JsonProfileLevel {
    price: i64,
    volume: i64,
    buy_volume: i64,
    sell_volume: i64,
}

impl RawProfile {
    /// Converts to a [`VolumeProfile`].
    ///
    /// # Errors
    ///
    /// `internal` on corrupt values.
    pub fn into_record(self) -> ApiResult<VolumeProfile> {
        let (ps, ss) = (self.ps, self.ss);
        let levels: Vec<JsonProfileLevel> = serde_json::from_str(&self.levels).map_err(corrupt)?;
        Ok(VolumeProfile {
            prov: self.prov.into_prov()?,
            derived: self.derived.into_header()?,
            start_ms: UnixMillis(self.start_ms),
            end_ms: UnixMillis(self.end_ms),
            bucket: px(self.bucket, ps)?,
            levels: levels
                .into_iter()
                .map(|l| {
                    Ok(ProfileLevel {
                        price: px(l.price, ps)?,
                        volume: sz(l.volume, ss)?,
                        buy_volume: sz(l.buy_volume, ss)?,
                        sell_volume: sz(l.sell_volume, ss)?,
                    })
                })
                .collect::<ApiResult<_>>()?,
        })
    }
}

/// Converts a vector of raw rows with a fallible conversion.
///
/// # Errors
///
/// The first conversion error.
pub fn convert<R, T>(rows: Vec<R>, f: impl Fn(R) -> ApiResult<T>) -> ApiResult<Vec<T>> {
    rows.into_iter().map(f).collect()
}
