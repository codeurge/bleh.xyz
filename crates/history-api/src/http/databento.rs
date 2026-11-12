//! A Databento-shaped historical endpoint that answers in DBN, encoded with
//! the Apache-2.0 [`dbn`] crate (ADR 0004).
//!
//! ```text
//! GET|POST /databento/v0/timeseries.get_range
//!     dataset=BLEH.HL  symbols=hyperliquid:perp:BTC  schema=trades
//!     start=...  end=...  [stype_in=raw_symbol] [limit=N]
//!     [encoding=dbn|json|csv] [compression=none|zstd]
//!     [pretty_px=true] [pretty_ts=true] [size_units=lots|whole]
//! ```
//!
//! Mapping (also in `docs/operations/history-api.md`):
//!
//! - **Dataset and publisher.** One dataset per venue (`BLEH.HL` for
//!   Hyperliquid) with a bleh publisher ID from a private range (45001+).
//!   These are not Databento-assigned IDs and imply no affiliation.
//! - **Symbols.** `stype_in=raw_symbol` takes bleh instrument IDs;
//!   `stype_out` is `instrument_id`, a stable 32-bit ID derived from the
//!   first four bytes of SHA-256 of the instrument ID, listed in the
//!   metadata's symbol mappings.
//! - **Prices** become DBN fixed-point (1e-9). Exact for scales up to 9.
//! - **Sizes** are integers in DBN. `size_units=lots` (default) writes the
//!   bleh size mantissa, that is the size in units of `10^-size_scale`
//!   (lossless; scales are in the `X-Bleh-Size-Scales` header).
//!   `size_units=whole` rounds to whole units (lossy for fractional
//!   sizes). Sizes above `u32::MAX` saturate and are counted in
//!   `X-Bleh-Size-Saturated`.
//! - **Times.** `ts_event` is the venue time (ms, scaled to ns);
//!   `ts_recv` is the node's ingest time; `sequence` is the block number
//!   modulo 2^32; `F_LAST` marks the last record of a block.
//! - **Schemas.** `trades`; `mbo` (starts with a clear and an `F_SNAPSHOT`
//!   book reconstructed at `start`; fills are `F` then `M` or `C`);
//!   `mbp-1` and `mbp-10` (book events that touch the top levels, with the
//!   levels after the event; trades are not interleaved because the
//!   archive does not order trades against book events within a block);
//!   `ohlcv-1s`, `ohlcv-1m`, `ohlcv-1h`, `ohlcv-1d` from precomputed bars.
//!   Provenance (admission, flags, signatures) is not carried in DBN; use
//!   `/v0/...` or the manifests for it.

use std::collections::BTreeMap;
use std::ffi::c_char;
use std::num::NonZeroU64;

use axum::Router;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, State};
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use dbn::encode::{DynEncoder, EncodeRecord};
use dbn::{
    BidAskPair, Compression, Encoding, FlagSet, MappingInterval, MboMsg, Mbp1Msg, Mbp10Msg,
    Metadata, OhlcvMsg, RecordHeader, SType, Schema, SymbolMapping, TradeMsg, rtype,
};
use mesh_types::{AggressorSide, BarInterval, BookAction, BookSide, InstrumentId, RecordKind};
use serde::Deserialize;

use super::{AppState, Q};
use crate::book::L4Book;
use crate::error::{ApiError, ApiResult, ErrorCode};
use crate::params::{DAY_MS, HOUR_MS, TimeRange, parse_instrument, parse_time};
use crate::service::Service;

/// A bleh dataset in Databento's naming.
#[derive(Debug, Clone, Copy)]
pub struct Dataset {
    /// Venue ID.
    pub venue: &'static str,
    /// Dataset code.
    pub code: &'static str,
    /// Publisher ID (private range, not assigned by Databento).
    pub publisher_id: u16,
    /// Description.
    pub description: &'static str,
}

/// Datasets offered.
pub const DATASETS: &[Dataset] = &[Dataset {
    venue: "hyperliquid",
    code: "BLEH.HL",
    publisher_id: 45_001,
    description: "bleh.xyz mesh: Hyperliquid order-book trades, order-level book events and bars (ADM-0001)",
}];

const SCHEMAS: &[&str] = &[
    "trades", "mbo", "mbp-1", "mbp-10", "ohlcv-1s", "ohlcv-1m", "ohlcv-1h", "ohlcv-1d",
];

const PAGE: usize = 50_000;

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/databento/v0/timeseries.get_range",
            get(get_range_query).post(get_range_form),
        )
        .route("/databento/v0/metadata.list_datasets", get(list_datasets))
        .route("/databento/v0/metadata.list_schemas", get(list_schemas))
        .route(
            "/databento/v0/metadata.list_publishers",
            get(list_publishers),
        )
}

/// Parameters of `timeseries.get_range`.
#[derive(Debug, Clone, Deserialize)]
pub struct GetRange {
    /// Dataset code (`BLEH.HL`) or venue ID.
    pub dataset: String,
    /// Comma-separated bleh instrument IDs, or `ALL_SYMBOLS`.
    pub symbols: Option<String>,
    /// DBN schema.
    pub schema: String,
    /// Start (inclusive).
    pub start: String,
    /// End (exclusive); default start plus one hour.
    pub end: Option<String>,
    /// `raw_symbol` (default).
    pub stype_in: Option<String>,
    /// `instrument_id` (default and only value).
    pub stype_out: Option<String>,
    /// Maximum records.
    pub limit: Option<u64>,
    /// `dbn` (default), `json` or `csv`.
    pub encoding: Option<String>,
    /// `none` (default) or `zstd`.
    pub compression: Option<String>,
    /// JSON/CSV: decimal prices.
    pub pretty_px: Option<bool>,
    /// JSON/CSV: ISO 8601 times.
    pub pretty_ts: Option<bool>,
    /// Accepted for compatibility; symbols are in the metadata mappings.
    pub map_symbols: Option<bool>,
    /// bleh extension: `lots` (default) or `whole`.
    pub size_units: Option<String>,
}

async fn get_range_query(State(s): State<AppState>, Q(q): Q<GetRange>) -> Response {
    respond(get_range(&s.svc, q).await)
}

async fn get_range_form(
    State(s): State<AppState>,
    form: Result<Form<GetRange>, FormRejection>,
) -> Response {
    match form {
        Ok(Form(q)) => respond(get_range(&s.svc, q).await),
        Err(e) => ApiError::bad_request(e.body_text()).into_response(),
    }
}

fn respond(r: ApiResult<Encoded>) -> Response {
    match r {
        Ok(enc) => {
            let mut resp = ([(header::CONTENT_TYPE, enc.content_type)], enc.body).into_response();
            for (k, v) in enc.headers {
                if let Ok(v) = HeaderValue::from_str(&v) {
                    resp.headers_mut().insert(HeaderName::from_static(k), v);
                }
            }
            resp
        }
        Err(e) => e.into_response(),
    }
}

async fn list_datasets() -> axum::Json<Vec<&'static str>> {
    axum::Json(DATASETS.iter().map(|d| d.code).collect())
}

#[derive(Debug, Deserialize)]
struct DatasetQuery {
    dataset: Option<String>,
}

async fn list_schemas(Q(q): Q<DatasetQuery>) -> Response {
    if let Some(d) = q.dataset.as_deref()
        && dataset(d).is_none()
    {
        return ApiError::new(ErrorCode::NotFound, format!("unknown dataset {d}")).into_response();
    }
    axum::Json(SCHEMAS).into_response()
}

async fn list_publishers() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::Value::Array(
        DATASETS
            .iter()
            .map(|d| {
                serde_json::json!({
                    "publisher_id": d.publisher_id,
                    "dataset": d.code,
                    "venue": d.venue,
                    "description": d.description,
                })
            })
            .collect(),
    ))
}

/// Finds a dataset by code or venue.
#[must_use]
pub fn dataset(s: &str) -> Option<&'static Dataset> {
    DATASETS
        .iter()
        .find(|d| d.code.eq_ignore_ascii_case(s) || d.venue == s)
}

/// The 32-bit DBN instrument ID of a bleh instrument.
#[must_use]
pub fn dbn_instrument_id(id: &InstrumentId) -> u32 {
    let d = mesh_types::Sha256Digest::of(id.as_str().as_bytes());
    let v = u32::from_be_bytes([d.0[0], d.0[1], d.0[2], d.0[3]]);
    v.max(1)
}

/// Converts a bleh price mantissa to DBN fixed-point (1e-9).
///
/// # Errors
///
/// `too_large` on overflow.
pub fn dbn_price(mantissa: i64, scale: u8) -> ApiResult<i64> {
    let overflow = || ApiError::new(ErrorCode::TooLarge, "price does not fit DBN fixed-point");
    if scale <= 9 {
        mantissa
            .checked_mul(10i64.pow(9 - u32::from(scale)))
            .ok_or_else(overflow)
    } else {
        let div = 10i64.pow(u32::from(scale) - 9);
        let half = div / 2;
        Ok((mantissa + if mantissa >= 0 { half } else { -half }) / div)
    }
}

/// Size conversion to DBN integers.
#[derive(Debug, Clone, Copy, Default)]
pub struct SizeConv {
    /// Round to whole units instead of writing lots.
    pub whole: bool,
    /// Sizes that saturated at `u32::MAX`.
    pub saturated: u64,
}

impl SizeConv {
    /// Converts a size mantissa at `scale`.
    pub fn conv64(&mut self, mantissa: i64, scale: u8) -> u64 {
        let m = mantissa.max(0);
        let v = if self.whole && scale > 0 {
            let div = 10i64.pow(u32::from(scale));
            (m + div / 2) / div
        } else {
            m
        };
        u64::try_from(v).unwrap_or(0)
    }

    /// As [`SizeConv::conv64`], saturating at `u32::MAX`.
    pub fn conv32(&mut self, mantissa: i64, scale: u8) -> u32 {
        let v = self.conv64(mantissa, scale);
        u32::try_from(v).unwrap_or_else(|_| {
            self.saturated += 1;
            u32::MAX
        })
    }
}

fn cc(b: u8) -> c_char {
    // ASCII action and side codes fit either signedness of c_char.
    c_char::try_from(b).unwrap_or(0)
}

fn ns(ms: i64) -> u64 {
    u64::try_from(ms.saturating_mul(1_000_000)).unwrap_or(0)
}

fn seq32(block: u64) -> u32 {
    u32::try_from(block & u64::from(u32::MAX)).unwrap_or(0)
}

enum Rec {
    Trade(TradeMsg),
    Mbo(MboMsg),
    Mbp1(Mbp1Msg),
    Mbp10(Box<Mbp10Msg>),
    Ohlcv(OhlcvMsg),
}

/// An encoded response.
#[derive(Debug)]
pub struct Encoded {
    /// Body bytes.
    pub body: Vec<u8>,
    /// Content type.
    pub content_type: &'static str,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
}

struct Sym {
    id: InstrumentId,
    dbn_id: u32,
    ps: u8,
    ss: u8,
}

/// Runs a `timeseries.get_range` request.
///
/// # Errors
///
/// Validation, store and encoding errors.
#[allow(clippy::too_many_lines)]
pub async fn get_range(svc: &Service, q: GetRange) -> ApiResult<Encoded> {
    let ds = dataset(&q.dataset).ok_or_else(|| {
        ApiError::new(
            ErrorCode::NotFound,
            format!(
                "unknown dataset {:?}; offered: {}",
                q.dataset,
                DATASETS
                    .iter()
                    .map(|d| d.code)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })?;
    let schema: Schema = q.schema.parse().map_err(|_| {
        ApiError::new(
            ErrorCode::Unsupported,
            format!("unknown schema {:?}", q.schema),
        )
    })?;
    if !SCHEMAS.contains(&schema.as_str()) {
        return Err(ApiError::new(
            ErrorCode::Unsupported,
            format!(
                "schema {} is not offered; offered: {}",
                schema.as_str(),
                SCHEMAS.join(", ")
            ),
        ));
    }
    if let Some(s) = q.stype_in.as_deref()
        && s != "raw_symbol"
    {
        return Err(ApiError::new(
            ErrorCode::Unsupported,
            "stype_in must be raw_symbol (bleh instrument IDs)",
        ));
    }
    if let Some(s) = q.stype_out.as_deref()
        && s != "instrument_id"
    {
        return Err(ApiError::new(
            ErrorCode::Unsupported,
            "stype_out must be instrument_id",
        ));
    }
    let encoding: Encoding = q
        .encoding
        .as_deref()
        .unwrap_or("dbn")
        .parse()
        .map_err(|_| ApiError::bad_request("encoding must be dbn, json or csv"))?;
    let compression: Compression = q
        .compression
        .as_deref()
        .unwrap_or("none")
        .parse()
        .map_err(|_| ApiError::bad_request("compression must be none or zstd"))?;
    let whole = match q.size_units.as_deref().unwrap_or("lots") {
        "lots" => false,
        "whole" => true,
        other => {
            return Err(ApiError::bad_request(format!(
                "size_units must be lots or whole, not {other:?}"
            )));
        }
    };
    let start = parse_time("start", &q.start)?;
    let end = match q.end.as_deref() {
        Some(e) => parse_time("end", e)?,
        None => mesh_types::UnixMillis(start.0.saturating_add(HOUR_MS)),
    };
    let interval = match schema {
        Schema::Ohlcv1S => Some(BarInterval::S1),
        Schema::Ohlcv1M => Some(BarInterval::M1),
        Schema::Ohlcv1H => Some(BarInterval::H1),
        Schema::Ohlcv1D => Some(BarInterval::D1),
        _ => None,
    };
    let max_range = match interval {
        Some(i) => svc.max_bar_range_ms(i),
        None => i64::try_from(svc.config.limits.max_range.as_millis()).unwrap_or(31 * DAY_MS),
    };
    let range = TimeRange::new(start, end, max_range)?;

    // Symbols.
    let raw = q.symbols.as_deref().unwrap_or("").trim();
    let mut ids: Vec<InstrumentId> = if raw.is_empty() || raw == "ALL_SYMBOLS" {
        svc.store
            .inventory()
            .await?
            .into_keys()
            .filter_map(|k| InstrumentId::new(k).ok())
            .filter(|i| i.venue_part() == ds.venue)
            .collect()
    } else {
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(parse_instrument)
            .collect::<ApiResult<_>>()?
    };
    ids.dedup();
    if ids.is_empty() {
        return Err(ApiError::bad_request("no symbols"));
    }
    if ids.len() > 100 {
        return Err(ApiError::bad_request("at most 100 symbols per request"));
    }
    let mut syms = Vec::new();
    let mut seen: BTreeMap<u32, String> = BTreeMap::new();
    for id in ids {
        if id.venue_part() != ds.venue {
            return Err(ApiError::bad_request(format!(
                "{id} is not in dataset {} (venue {})",
                ds.code, ds.venue
            )));
        }
        svc.store.require_instrument(&id).await?;
        let (ps, ss) = svc.store.scales(&id).await?;
        let dbn_id = dbn_instrument_id(&id);
        if let Some(other) = seen.insert(dbn_id, id.to_string()) {
            return Err(ApiError::new(
                ErrorCode::Unsupported,
                format!(
                    "instrument_id collision between {other} and {id}; request them separately"
                ),
            ));
        }
        syms.push(Sym { id, dbn_id, ps, ss });
    }

    let cap = svc.config.limits.max_dbn_records;
    let user_limit = q
        .limit
        .and_then(|l| usize::try_from(l).ok())
        .filter(|l| *l > 0);
    let budget = user_limit.unwrap_or(cap).min(cap);
    let pub_id = ds.publisher_id;
    let mut sizes = SizeConv {
        whole,
        saturated: 0,
    };
    // (ts_event, symbol index, block, index, sub) -> record
    let mut out: Out = Vec::new();
    let over = |n: usize| n > budget && user_limit.is_none();

    for (si, sym) in syms.iter().enumerate() {
        match schema {
            Schema::Trades => {
                let mut after = None;
                loop {
                    let page = svc
                        .store
                        .trades(&sym.id, range.start_ms, range.end_ms, after, PAGE)
                        .await?;
                    let n = page.len();
                    for (i, t) in page.iter().enumerate() {
                        let last_in_block = page
                            .get(i + 1)
                            .is_none_or(|x| x.prov.seq.block_number != t.prov.seq.block_number);
                        let mut flags = FlagSet::empty();
                        if last_in_block {
                            flags.set_last();
                        }
                        let ts = ns(t.prov.ts_source_ms.0);
                        let side = match t.aggressor {
                            AggressorSide::Buy => b'B',
                            AggressorSide::Sell => b'A',
                            AggressorSide::Unknown => b'N',
                        };
                        out.push((
                            (ts, si, t.prov.seq.block_number, t.prov.seq.event_index, 0),
                            Rec::Trade(TradeMsg {
                                hd: RecordHeader::new::<TradeMsg>(
                                    rtype::MBP_0,
                                    pub_id,
                                    sym.dbn_id,
                                    ts,
                                ),
                                price: dbn_price(t.price.mantissa(), t.price.scale())?,
                                size: sizes.conv32(t.size.mantissa(), t.size.scale()),
                                action: cc(b'T'),
                                side: cc(side),
                                flags,
                                depth: 0,
                                ts_recv: u64::try_from(t.prov.ts_ingest_ns.0).unwrap_or(0),
                                ts_in_delta: 0,
                                sequence: seq32(t.prov.seq.block_number),
                            }),
                        ));
                    }
                    if over(out.len()) {
                        return Err(too_many(cap));
                    }
                    match page.last() {
                        Some(l) if n == PAGE => {
                            after = Some((l.prov.seq.block_number, l.prov.seq.event_index));
                        }
                        _ => break,
                    }
                }
            }
            Schema::Mbo | Schema::Mbp1 | Schema::Mbp10 => {
                let (mut book, _) = svc.build_book(&sym.id, range.start_ms - 1).await?;
                if schema == Schema::Mbo {
                    snapshot_records(&book, sym, si, pub_id, &mut sizes, &mut out)?;
                }
                let depth_n = match schema {
                    Schema::Mbp1 => 1,
                    Schema::Mbp10 => 10,
                    _ => 0,
                };
                let mut after = None;
                loop {
                    let page = svc
                        .store
                        .book_events(&sym.id, range.start_ms, range.end_ms, after, PAGE)
                        .await?;
                    let n = page.len();
                    for (i, e) in page.iter().enumerate() {
                        let last_in_block = page
                            .get(i + 1)
                            .is_none_or(|x| x.prov.seq.block_number != e.prov.seq.block_number);
                        if schema == Schema::Mbo {
                            mbo_records(
                                &mut book,
                                e,
                                sym,
                                si,
                                pub_id,
                                last_in_block,
                                &mut sizes,
                                &mut out,
                            )?;
                        } else {
                            mbp_record(
                                &mut book,
                                e,
                                sym,
                                si,
                                pub_id,
                                depth_n,
                                last_in_block,
                                &mut sizes,
                                &mut out,
                            )?;
                        }
                    }
                    if over(out.len()) {
                        return Err(too_many(cap));
                    }
                    match page.last() {
                        Some(l) if n == PAGE => {
                            after = Some((l.prov.seq.block_number, l.prov.seq.event_index));
                        }
                        _ => break,
                    }
                }
            }
            _ => {
                let Some(interval) = interval else {
                    return Err(ApiError::new(ErrorCode::Unsupported, "schema not offered"));
                };
                let rt = match interval {
                    BarInterval::S1 => rtype::OHLCV_1S,
                    BarInterval::M1 => rtype::OHLCV_1M,
                    BarInterval::H1 => rtype::OHLCV_1H,
                    _ => rtype::OHLCV_1D,
                };
                let mut after = None;
                loop {
                    let page = svc
                        .store
                        .bars(&sym.id, interval, range.start_ms, range.end_ms, after, PAGE)
                        .await?;
                    let n = page.len();
                    for b in &page {
                        let ts = ns(b.start_ms.0);
                        let p = |x: mesh_types::Price| dbn_price(x.mantissa(), x.scale());
                        out.push((
                            (ts, si, b.prov.seq.block_number, b.prov.seq.event_index, 0),
                            Rec::Ohlcv(OhlcvMsg {
                                hd: RecordHeader::new::<OhlcvMsg>(rt, pub_id, sym.dbn_id, ts),
                                open: p(b.open)?,
                                high: p(b.high)?,
                                low: p(b.low)?,
                                close: p(b.close)?,
                                volume: sizes.conv64(b.volume.mantissa(), b.volume.scale()),
                            }),
                        ));
                    }
                    if over(out.len()) {
                        return Err(too_many(cap));
                    }
                    match page.last() {
                        Some(l) if n == PAGE => after = Some(l.start_ms.0),
                        _ => break,
                    }
                }
            }
        }
    }
    out.sort_by_key(|(k, _)| *k);
    out.truncate(budget);

    let start_ns = ns(range.start_ms);
    let end_ns = ns(range.end_ms);
    let date = |ms: i64| {
        let day = i32::try_from(ms.div_euclid(DAY_MS)).unwrap_or(0);
        time::Date::from_julian_day(2_440_588 + day).map_err(ApiError::internal)
    };
    let (d0, d1) = (date(range.start_ms)?, date(range.end_ms + DAY_MS - 1)?);
    let mappings: Vec<SymbolMapping> = syms
        .iter()
        .map(|s| SymbolMapping {
            raw_symbol: s.id.to_string(),
            intervals: vec![MappingInterval {
                start_date: d0,
                end_date: d1,
                symbol: s.dbn_id.to_string(),
            }],
        })
        .collect();
    let metadata = Metadata::builder()
        .dataset(ds.code)
        .schema(Some(schema))
        .start(start_ns)
        .end(NonZeroU64::new(end_ns))
        .limit(user_limit.and_then(|l| NonZeroU64::new(u64::try_from(l).unwrap_or(0))))
        .stype_in(Some(SType::RawSymbol))
        .stype_out(SType::InstrumentId)
        .symbols(syms.iter().map(|s| s.id.to_string()).collect())
        .mappings(mappings)
        .build();
    let mut body = Vec::new();
    {
        let mut enc = DynEncoder::new(
            &mut body,
            encoding,
            compression,
            &metadata,
            false,
            q.pretty_px.unwrap_or(false),
            q.pretty_ts.unwrap_or(false),
        )
        .map_err(ApiError::internal)?;
        for (_, r) in &out {
            let res = match r {
                Rec::Trade(m) => enc.encode_record(m),
                Rec::Mbo(m) => enc.encode_record(m),
                Rec::Mbp1(m) => enc.encode_record(m),
                Rec::Mbp10(m) => enc.encode_record(m.as_ref()),
                Rec::Ohlcv(m) => enc.encode_record(m),
            };
            res.map_err(ApiError::internal)?;
        }
        enc.flush().map_err(ApiError::internal)?;
    }
    let content_type = match (encoding, compression) {
        (_, Compression::Zstd) | (Encoding::Dbn, _) => "application/octet-stream",
        (Encoding::Json, _) => "application/json",
        (Encoding::Csv, _) => "text/csv",
    };
    let scales: BTreeMap<String, u8> = syms.iter().map(|s| (s.id.to_string(), s.ss)).collect();
    let _ = q.map_symbols;
    crate::metrics::global().add_rows(out.len());
    Ok(Encoded {
        body,
        content_type,
        headers: vec![
            (
                "x-bleh-size-units",
                if whole { "whole" } else { "lots" }.to_owned(),
            ),
            (
                "x-bleh-size-scales",
                serde_json::to_string(&scales).unwrap_or_default(),
            ),
            ("x-bleh-size-saturated", sizes.saturated.to_string()),
            ("x-bleh-record-count", out.len().to_string()),
        ],
    })
}

fn too_many(cap: usize) -> ApiError {
    ApiError::new(
        ErrorCode::TooLarge,
        format!("more than {cap} records; narrow the range or pass limit"),
    )
}

/// Sort key: `(ts_event, symbol index, block, event index, sub-record)`.
type SortKey = (u64, usize, u64, u32, u32);
type Out = Vec<(SortKey, Rec)>;

fn snapshot_records(
    book: &L4Book,
    sym: &Sym,
    si: usize,
    pub_id: u16,
    sizes: &mut SizeConv,
    out: &mut Out,
) -> ApiResult<()> {
    let ts = ns(book.last_ts_ms);
    let block = book.last_seq.block_number;
    let mut flags = FlagSet::empty();
    flags.set_snapshot();
    let orders = book.orders();
    let mut clear_flags = flags;
    if orders.is_empty() {
        clear_flags.set_last();
    }
    out.push((
        (ts, si, block, 0, 0),
        Rec::Mbo(MboMsg {
            hd: RecordHeader::new::<MboMsg>(rtype::MBO, pub_id, sym.dbn_id, ts),
            order_id: 0,
            price: dbn::UNDEF_PRICE,
            size: 0,
            flags: clear_flags,
            channel_id: 0,
            action: cc(b'R'),
            side: cc(b'N'),
            ts_recv: ts,
            ts_in_delta: 0,
            sequence: seq32(block),
        }),
    ));
    for (i, (side, oid, price, size, _)) in orders.iter().enumerate() {
        let mut f = flags;
        if i + 1 == orders.len() {
            f.set_last();
        }
        out.push((
            (ts, si, block, 0, u32::try_from(i + 1).unwrap_or(u32::MAX)),
            Rec::Mbo(MboMsg {
                hd: RecordHeader::new::<MboMsg>(rtype::MBO, pub_id, sym.dbn_id, ts),
                order_id: *oid,
                price: dbn_price(*price, sym.ps)?,
                size: sizes.conv32(*size, sym.ss),
                flags: f,
                channel_id: 0,
                action: cc(b'A'),
                side: cc(side_code(*side)),
                ts_recv: ts,
                ts_in_delta: 0,
                sequence: seq32(block),
            }),
        ));
    }
    Ok(())
}

const fn side_code(s: BookSide) -> u8 {
    match s {
        BookSide::Bid => b'B',
        BookSide::Ask => b'A',
    }
}

#[allow(clippy::too_many_arguments)]
fn mbo_records(
    book: &mut L4Book,
    e: &mesh_types::BookEvent,
    sym: &Sym,
    si: usize,
    pub_id: u16,
    last_in_block: bool,
    sizes: &mut SizeConv,
    out: &mut Out,
) -> ApiResult<()> {
    let applied = book.apply(e);
    let ts = ns(e.prov.ts_source_ms.0);
    let recv = u64::try_from(e.prov.ts_ingest_ns.0).unwrap_or(0);
    let price = dbn_price(e.price.mantissa(), e.price.scale())?;
    let ss = sym.ss;
    let mut msgs: Vec<(u8, i64)> = Vec::new();
    match e.action {
        BookAction::Add { size } => msgs.push((b'A', size.mantissa())),
        BookAction::Modify { new_size, .. } => msgs.push((b'M', new_size.mantissa())),
        BookAction::Fill {
            orig_size,
            new_size,
        } => {
            let before = applied.before.or(orig_size.map(mesh_types::Size::mantissa));
            let qty = before.map_or(0, |b| b - new_size.mantissa());
            msgs.push((b'F', qty));
            if new_size.mantissa() == 0 {
                msgs.push((b'C', before.unwrap_or(0)));
            } else {
                msgs.push((b'M', new_size.mantissa()));
            }
        }
        BookAction::Cancel {} => msgs.push((b'C', applied.before.unwrap_or(0))),
    }
    let n = msgs.len();
    for (i, (action, size)) in msgs.into_iter().enumerate() {
        let mut flags = FlagSet::empty();
        if last_in_block && i + 1 == n {
            flags.set_last();
        }
        out.push((
            (
                ts,
                si,
                e.prov.seq.block_number,
                e.prov.seq.event_index,
                u32::try_from(i).unwrap_or(0),
            ),
            Rec::Mbo(MboMsg {
                hd: RecordHeader::new::<MboMsg>(rtype::MBO, pub_id, sym.dbn_id, ts),
                order_id: e.order_id.0,
                price,
                size: sizes.conv32(size, ss),
                flags,
                channel_id: 0,
                action: cc(action),
                side: cc(side_code(e.side)),
                ts_recv: recv,
                ts_in_delta: 0,
                sequence: seq32(e.prov.seq.block_number),
            }),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mbp_record(
    book: &mut L4Book,
    e: &mesh_types::BookEvent,
    sym: &Sym,
    si: usize,
    pub_id: u16,
    depth_n: usize,
    last_in_block: bool,
    sizes: &mut SizeConv,
    out: &mut Out,
) -> ApiResult<()> {
    let px = e.price.mantissa();
    let pos = |b: &L4Book| b.levels(e.side, depth_n).iter().position(|l| l.0 == px);
    let before_pos = pos(book);
    let applied = book.apply(e);
    let after_pos = pos(book);
    let Some(depth) = after_pos.or(before_pos) else {
        return Ok(());
    };
    let (action, size) = match e.action {
        BookAction::Add { size } => (b'A', size.mantissa()),
        BookAction::Modify { new_size, .. } => (b'M', new_size.mantissa()),
        BookAction::Fill {
            orig_size,
            new_size,
        } => {
            let before = applied.before.or(orig_size.map(mesh_types::Size::mantissa));
            (b'F', before.map_or(0, |b| b - new_size.mantissa()))
        }
        BookAction::Cancel {} => (b'C', applied.before.unwrap_or(0)),
    };
    let bids = book.levels(BookSide::Bid, depth_n);
    let asks = book.levels(BookSide::Ask, depth_n);
    let mut levels: Vec<BidAskPair> = (0..depth_n).map(|_| BidAskPair::default()).collect();
    for (i, l) in levels.iter_mut().enumerate() {
        l.bid_px = dbn::UNDEF_PRICE;
        l.ask_px = dbn::UNDEF_PRICE;
        if let Some((p, s, c)) = bids.get(i) {
            l.bid_px = dbn_price(*p, sym.ps)?;
            l.bid_sz = sizes.conv32(*s, sym.ss);
            l.bid_ct = *c;
        }
        if let Some((p, s, c)) = asks.get(i) {
            l.ask_px = dbn_price(*p, sym.ps)?;
            l.ask_sz = sizes.conv32(*s, sym.ss);
            l.ask_ct = *c;
        }
    }
    let mut flags = FlagSet::empty();
    if last_in_block {
        flags.set_last();
    }
    let ts = ns(e.prov.ts_source_ms.0);
    let recv = u64::try_from(e.prov.ts_ingest_ns.0).unwrap_or(0);
    let key = (ts, si, e.prov.seq.block_number, e.prov.seq.event_index, 0);
    let price = dbn_price(px, e.price.scale())?;
    let size = sizes.conv32(size, sym.ss);
    let depth = u8::try_from(depth).unwrap_or(u8::MAX);
    if depth_n == 1 {
        out.push((
            key,
            Rec::Mbp1(Mbp1Msg {
                hd: RecordHeader::new::<Mbp1Msg>(rtype::MBP_1, pub_id, sym.dbn_id, ts),
                price,
                size,
                action: cc(action),
                side: cc(side_code(e.side)),
                flags,
                depth,
                ts_recv: recv,
                ts_in_delta: 0,
                sequence: seq32(e.prov.seq.block_number),
                levels: [levels.into_iter().next().unwrap_or_default()],
            }),
        ));
    } else {
        let mut it = levels.into_iter();
        let arr: [BidAskPair; 10] = std::array::from_fn(|_| it.next().unwrap_or_default());
        out.push((
            key,
            Rec::Mbp10(Box::new(Mbp10Msg {
                hd: RecordHeader::new::<Mbp10Msg>(rtype::MBP_10, pub_id, sym.dbn_id, ts),
                price,
                size,
                action: cc(action),
                side: cc(side_code(e.side)),
                flags,
                depth,
                ts_recv: recv,
                ts_in_delta: 0,
                sequence: seq32(e.prov.seq.block_number),
                levels: arr,
            })),
        ));
    }
    Ok(())
}

/// Record kind each schema reads (for documentation and tests).
#[must_use]
pub fn source_kind(schema: &str) -> Option<RecordKind> {
    match schema {
        "trades" => Some(RecordKind::Trade),
        "mbo" | "mbp-1" | "mbp-10" => Some(RecordKind::BookEvent),
        s if s.starts_with("ohlcv-") => Some(RecordKind::Bar),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_and_sizes() {
        assert_eq!(dbn_price(100_250, 3).unwrap(), 100_250_000_000);
        assert_eq!(dbn_price(1, 9).unwrap(), 1);
        assert_eq!(dbn_price(15, 10).unwrap(), 2);
        assert!(dbn_price(i64::MAX, 0).is_err());
        let mut s = SizeConv::default();
        assert_eq!(s.conv32(12_345, 3), 12_345);
        let mut w = SizeConv {
            whole: true,
            saturated: 0,
        };
        assert_eq!(w.conv32(12_500, 3), 13);
        assert_eq!(w.conv32(400, 3), 0);
        assert_eq!(s.conv32(i64::MAX, 0), u32::MAX);
        assert_eq!(s.saturated, 1);
    }

    #[test]
    fn ids_and_datasets() {
        let a = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        assert_eq!(dbn_instrument_id(&a), dbn_instrument_id(&a));
        assert_ne!(
            dbn_instrument_id(&a),
            dbn_instrument_id(&InstrumentId::new("hyperliquid:perp:syn:IDX").unwrap())
        );
        assert_eq!(dataset("BLEH.HL").unwrap().venue, "hyperliquid");
        assert_eq!(dataset("hyperliquid").unwrap().code, "BLEH.HL");
        assert!(dataset("other").is_none());
        assert_eq!(source_kind("ohlcv-1m"), Some(RecordKind::Bar));
        for s in SCHEMAS {
            assert!(s.parse::<Schema>().is_ok(), "{s}");
        }
    }
}
