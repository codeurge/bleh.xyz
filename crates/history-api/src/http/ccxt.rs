//! A CCXT-style REST shim under `/ccxt/`, with the semantics of CCXT's
//! unified `fetchMarkets`, `fetchTrades`, `fetchOHLCV`, `fetchOrderBook`
//! and `fetchTime`, so a thin CCXT exchange class (or any client written
//! against CCXT structures) can read the archive.
//!
//! | Endpoint | CCXT method | Returns |
//! |---|---|---|
//! | `GET /ccxt` | `describe()` | capabilities and timeframes |
//! | `GET /ccxt/markets` | `fetchMarkets()` | market structures |
//! | `GET /ccxt/trades?symbol&since&limit` | `fetchTrades` | trade structures |
//! | `GET /ccxt/ohlcv?symbol&timeframe&since&limit` | `fetchOHLCV` | `[[t, o, h, l, c, v], ...]` |
//! | `GET /ccxt/orderbook?symbol&limit&timestamp` | `fetchOrderBook` | order book structure |
//! | `GET /ccxt/time` | `fetchTime` | ms |
//!
//! Symbols are bleh instrument IDs (the shim does not invent unified
//! `BASE/QUOTE:SETTLE` names the registry does not carry). Numbers are
//! `f64` as CCXT expects; each trade's `info` keeps the exact
//! observation-v0 record. Without `since`, trades and candles are the most
//! recent in the archive.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use mesh_types::{AggressorSide, BarInterval, Record, RecordKind, SnapshotDepth, UnixMillis};
use serde::Deserialize;

use super::udf::to_f64;
use super::{AppState, Q};
use crate::error::{ApiError, ApiResult, ErrorCode};
use crate::params::{HOUR_MS, parse_instrument};

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/ccxt", get(describe))
        .route("/ccxt/markets", get(markets))
        .route("/ccxt/trades", get(trades))
        .route("/ccxt/ohlcv", get(ohlcv))
        .route("/ccxt/orderbook", get(orderbook))
        .route("/ccxt/time", get(time))
}

const TIMEFRAMES: &[&str] = &["1s", "1m", "5m", "15m", "1h", "1d"];

async fn describe() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": "bleh",
        "name": "bleh.xyz history",
        "version": "v0",
        "pro": false,
        "has": {
            "fetchMarkets": true,
            "fetchTrades": true,
            "fetchOHLCV": true,
            "fetchOrderBook": true,
            "fetchTime": true,
            "fetchTicker": false,
            "fetchBalance": false,
            "createOrder": false,
        },
        "timeframes": TIMEFRAMES.iter().map(|t| ((*t).to_owned(), serde_json::Value::from(*t))).collect::<serde_json::Map<_, _>>(),
        "note": "Read-only historical data. Symbols are bleh instrument IDs.",
    }))
}

async fn markets(State(s): State<AppState>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = s
            .svc
            .instruments(
                None,
                None,
                None,
                Some(s.svc.config.limits.max_rows),
                cursor.as_deref(),
            )
            .await?;
        for i in &page.instruments {
            let class = i.class.as_str();
            let amount_step = i.size_scale.map(|ss| to_f64(1, ss));
            let price_step = i.price_scale.map(|ps| to_f64(1, ps));
            out.push(serde_json::json!({
                "id": i.id,
                "symbol": i.id,
                "base": i.symbol,
                "quote": serde_json::Value::Null,
                "settle": serde_json::Value::Null,
                "type": if class == "perp" { "swap" } else { class },
                "spot": class == "spot",
                "swap": class == "perp",
                "future": false,
                "option": false,
                "contract": class == "perp",
                "linear": serde_json::Value::Null,
                "active": true,
                "precision": {"amount": amount_step, "price": price_step},
                "limits": {"amount": {"min": amount_step}, "price": {"min": price_step}},
                "info": i,
            }));
        }
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    Ok(Json(out))
}

#[derive(Debug, Deserialize)]
struct TradesQuery {
    symbol: String,
    since: Option<i64>,
    limit: Option<usize>,
}

fn trade_json(r: &mesh_types::Trade) -> serde_json::Value {
    let price = to_f64(r.price.mantissa(), r.price.scale());
    let amount = to_f64(r.size.mantissa(), r.size.scale());
    serde_json::json!({
        "id": r.trade_id.0.to_string(),
        "timestamp": r.prov.ts_source_ms.0,
        "datetime": r.prov.ts_source_ms.to_rfc3339(),
        "symbol": r.prov.instrument.to_string(),
        "order": serde_json::Value::Null,
        "type": serde_json::Value::Null,
        "side": match r.aggressor {
            AggressorSide::Buy => serde_json::json!("buy"),
            AggressorSide::Sell => serde_json::json!("sell"),
            AggressorSide::Unknown => serde_json::Value::Null,
        },
        "takerOrMaker": "taker",
        "price": price,
        "amount": amount,
        "cost": price * amount,
        "fee": serde_json::Value::Null,
        "fees": [],
        "info": Record::Trade(r.clone()),
    })
}

async fn trades(
    State(s): State<AppState>,
    Q(q): Q<TradesQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let id = parse_instrument(&q.symbol)?;
    s.svc.store.require_instrument(&id).await?;
    let limit = s.svc.config.limits.page(Some(q.limit.unwrap_or(500)));
    let max_ms = i64::try_from(s.svc.config.limits.max_range.as_millis()).unwrap_or(i64::MAX);
    let trades = match q.since {
        Some(since) => {
            s.svc
                .store
                .trades(&id, since, since.saturating_add(max_ms), None, limit)
                .await?
        }
        None => {
            let Some(latest) = s.svc.store.latest_time(RecordKind::Trade, &id).await? else {
                return Ok(Json(Vec::new()));
            };
            s.svc
                .store
                .recent_trades(&id, latest + 1, 24 * HOUR_MS, limit)
                .await?
        }
    };
    crate::metrics::global().add_rows(trades.len());
    Ok(Json(trades.iter().map(trade_json).collect()))
}

#[derive(Debug, Deserialize)]
struct OhlcvQuery {
    symbol: String,
    timeframe: Option<String>,
    since: Option<i64>,
    limit: Option<usize>,
}

async fn ohlcv(State(s): State<AppState>, Q(q): Q<OhlcvQuery>) -> ApiResult<Json<Vec<[f64; 6]>>> {
    let id = parse_instrument(&q.symbol)?;
    s.svc.store.require_instrument(&id).await?;
    let interval: BarInterval =
        crate::params::parse_interval(q.timeframe.as_deref().unwrap_or("1m"))?;
    let limit = s.svc.config.limits.page(Some(q.limit.unwrap_or(500)));
    let max_ms = s.svc.max_bar_range_ms(interval);
    let bars = match q.since {
        Some(since) => {
            s.svc
                .store
                .bars(
                    &id,
                    interval,
                    since,
                    since.saturating_add(max_ms),
                    None,
                    limit,
                )
                .await?
        }
        None => {
            let Some(latest) = s.svc.store.latest_time(RecordKind::Bar, &id).await? else {
                return Ok(Json(Vec::new()));
            };
            s.svc
                .store
                .bars_before(&id, interval, latest + 1, limit, max_ms)
                .await?
        }
    };
    crate::metrics::global().add_rows(bars.len());
    #[allow(clippy::cast_precision_loss)]
    let rows = bars
        .iter()
        .map(|b| {
            let p = |x: mesh_types::Price| to_f64(x.mantissa(), x.scale());
            [
                b.start_ms.0 as f64,
                p(b.open),
                p(b.high),
                p(b.low),
                p(b.close),
                to_f64(b.volume.mantissa(), b.volume.scale()),
            ]
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Debug, Deserialize)]
struct BookQuery {
    symbol: String,
    limit: Option<usize>,
    timestamp: Option<i64>,
}

async fn orderbook(
    State(s): State<AppState>,
    Q(q): Q<BookQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let at = q.timestamp.map(|t| t.to_string());
    let b = s
        .svc
        .book_at(&q.symbol, at.as_deref(), Some("l2"), q.limit)
        .await?;
    let Record::BookSnapshot(snap) = &b.book else {
        return Err(ApiError::new(ErrorCode::Internal, "unexpected record"));
    };
    let SnapshotDepth::L2 { bids, asks } = &snap.depth else {
        return Err(ApiError::new(ErrorCode::Internal, "expected levels"));
    };
    let side = |levels: &Vec<mesh_types::Level>| -> Vec<[f64; 2]> {
        levels
            .iter()
            .map(|l| {
                [
                    to_f64(l.price.mantissa(), l.price.scale()),
                    to_f64(l.size.mantissa(), l.size.scale()),
                ]
            })
            .collect()
    };
    let ts = snap.prov.ts_source_ms;
    Ok(Json(serde_json::json!({
        "symbol": b.instrument,
        "bids": side(bids),
        "asks": side(asks),
        "timestamp": ts.0,
        "datetime": ts.to_rfc3339(),
        "nonce": snap.prov.seq.block_number,
        "info": {"checkpoint": b.checkpoint, "events_applied": b.events_applied},
    })))
}

async fn time() -> Json<i64> {
    Json(UnixMillis::now().0)
}
