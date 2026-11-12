//! TradingView UDF datafeed (`/udf/config`, `/udf/symbols`,
//! `/udf/search`, `/udf/history`, `/udf/time`).
//!
//! Point a TradingView Charting Library `UDFCompatibleDatafeed` at
//! `https://<host>/udf`. Bars come from the precomputed bar partitions.
//! UDF wants JSON numbers, so prices and volumes are converted to `f64`
//! here (the only lossy step; `/v0/bars` keeps exact decimal strings).
//!
//! Resolutions: `1S` (1s), `1` (1m), `5`, `15`, `60` (1h), `1D`/`D`.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use mesh_types::{Bar, BarInterval, InstrumentId, RecordKind};
use serde::Deserialize;

use super::{AppState, Q};
use crate::error::ApiResult;
use crate::service::Service;

/// Resolutions offered, in UDF spelling.
pub const RESOLUTIONS: &[&str] = &["1S", "1", "5", "15", "60", "1D"];

/// Routes of this module.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/udf/config", get(config))
        .route("/udf/symbols", get(symbols))
        .route("/udf/search", get(search))
        .route("/udf/history", get(history))
        .route("/udf/time", get(time))
}

/// Maps a UDF resolution to a bar interval.
#[must_use]
pub fn interval(resolution: &str) -> Option<BarInterval> {
    match resolution {
        "1S" | "1s" => Some(BarInterval::S1),
        "1" => Some(BarInterval::M1),
        "5" => Some(BarInterval::M5),
        "15" => Some(BarInterval::M15),
        "60" => Some(BarInterval::H1),
        "1D" | "D" | "1440" => Some(BarInterval::D1),
        _ => None,
    }
}

/// Converts a decimal to `f64` for UDF and CCXT (lossy by nature).
#[must_use]
pub fn to_f64(mantissa: i64, scale: u8) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let m = mantissa as f64;
    m / 10f64.powi(i32::from(scale))
}

async fn config(State(s): State<AppState>) -> Json<serde_json::Value> {
    let venues: Vec<String> = s
        .svc
        .store
        .inventory()
        .await
        .map(|inv| {
            let mut v: Vec<String> = inv
                .keys()
                .filter_map(|k| k.split(':').next().map(str::to_owned))
                .collect();
            v.sort();
            v.dedup();
            v
        })
        .unwrap_or_default();
    Json(serde_json::json!({
        "supported_resolutions": RESOLUTIONS,
        "supports_search": true,
        "supports_group_request": false,
        "supports_marks": false,
        "supports_timescale_marks": false,
        "supports_time": true,
        "exchanges": std::iter::once(serde_json::json!({"value": "", "name": "All", "desc": ""}))
            .chain(venues.iter().map(|v| serde_json::json!({"value": v, "name": v, "desc": v})))
            .collect::<Vec<_>>(),
        "symbols_types": [
            {"name": "All", "value": ""},
            {"name": "perp", "value": "perp"},
            {"name": "spot", "value": "spot"},
            {"name": "outcome", "value": "outcome"},
        ],
    }))
}

#[derive(Debug, Deserialize)]
struct SymbolQuery {
    symbol: String,
}

fn udf_error(msg: &str) -> Response {
    Json(serde_json::json!({"s": "error", "errmsg": msg})).into_response()
}

async fn symbol_info(svc: &Service, symbol: &str) -> ApiResult<serde_json::Value> {
    let id = crate::params::parse_instrument(symbol)?;
    svc.store.require_instrument(&id).await?;
    let (ps, ss) = svc.store.scales(&id).await?;
    let pricescale = 10u64.saturating_pow(u32::from(ps));
    Ok(serde_json::json!({
        "name": id.to_string(),
        "ticker": id.to_string(),
        "full_name": id.to_string(),
        "description": format!("{} {} on {}", id.symbol(), id.class_part(), id.venue_part()),
        "type": id.class_part(),
        "session": "24x7",
        "timezone": "Etc/UTC",
        "exchange": id.venue_part(),
        "listed_exchange": id.venue_part(),
        "format": "price",
        "minmov": 1,
        "pricescale": pricescale,
        "has_intraday": true,
        "has_seconds": true,
        "seconds_multipliers": ["1"],
        "has_daily": true,
        "has_weekly_and_monthly": false,
        "intraday_multipliers": ["1", "5", "15", "60"],
        "supported_resolutions": RESOLUTIONS,
        "volume_precision": ss,
        "data_status": "endofday",
    }))
}

async fn symbols(State(s): State<AppState>, Q(q): Q<SymbolQuery>) -> Response {
    match symbol_info(&s.svc, &q.symbol).await {
        Ok(v) => Json(v).into_response(),
        Err(_) => udf_error("unknown_symbol"),
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    query: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    exchange: Option<String>,
    limit: Option<usize>,
}

async fn search(State(s): State<AppState>, Q(q): Q<SearchQuery>) -> Response {
    let class = q.kind.as_deref().filter(|k| !k.is_empty());
    let venue = q.exchange.as_deref().filter(|e| !e.is_empty());
    match s
        .svc
        .instruments(
            q.query.as_deref().filter(|x| !x.is_empty()),
            venue,
            class,
            Some(q.limit.unwrap_or(30).min(100)),
            None,
        )
        .await
    {
        Ok(page) => Json(
            page.instruments
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "symbol": i.id,
                        "full_name": i.id,
                        "description": format!("{} {} on {}", i.symbol, i.class, i.venue),
                        "exchange": i.venue,
                        "ticker": i.id,
                        "type": i.class,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => udf_error(&e.message),
    }
}

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    symbol: String,
    resolution: String,
    from: i64,
    to: i64,
    countback: Option<usize>,
}

fn bars_json(bars: &[Bar]) -> serde_json::Value {
    let px = |p: mesh_types::Price| to_f64(p.mantissa(), p.scale());
    serde_json::json!({
        "s": "ok",
        "t": bars.iter().map(|b| b.start_ms.0 / 1000).collect::<Vec<_>>(),
        "o": bars.iter().map(|b| px(b.open)).collect::<Vec<_>>(),
        "h": bars.iter().map(|b| px(b.high)).collect::<Vec<_>>(),
        "l": bars.iter().map(|b| px(b.low)).collect::<Vec<_>>(),
        "c": bars.iter().map(|b| px(b.close)).collect::<Vec<_>>(),
        "v": bars.iter().map(|b| to_f64(b.volume.mantissa(), b.volume.scale())).collect::<Vec<_>>(),
    })
}

/// Answers a UDF history request.
///
/// # Errors
///
/// Store errors.
pub async fn history_json(
    svc: &Service,
    id: &InstrumentId,
    interval: BarInterval,
    from_s: i64,
    to_s: i64,
    countback: Option<usize>,
) -> ApiResult<serde_json::Value> {
    let max_rows = svc.config.limits.max_rows;
    let max_ms = svc.max_bar_range_ms(interval);
    let end_ms = to_s.saturating_mul(1000).saturating_add(1);
    let bars = if let Some(n) = countback {
        svc.store
            .bars_before(id, interval, end_ms, n.min(max_rows), max_ms)
            .await?
    } else {
        let start_ms = from_s
            .saturating_mul(1000)
            .max(end_ms.saturating_sub(max_ms));
        svc.store
            .bars(id, interval, start_ms, end_ms, None, max_rows)
            .await?
    };
    if bars.is_empty() {
        let before = svc
            .store
            .bars_before(id, interval, from_s.saturating_mul(1000), 1, max_ms)
            .await?;
        let mut v = serde_json::json!({"s": "no_data"});
        if let Some(b) = before.last() {
            v["nextTime"] = serde_json::json!(b.start_ms.0 / 1000);
        }
        return Ok(v);
    }
    crate::metrics::global().add_rows(bars.len());
    Ok(bars_json(&bars))
}

async fn history(State(s): State<AppState>, Q(q): Q<HistoryQuery>) -> Response {
    let Some(interval) = interval(&q.resolution) else {
        return udf_error("unsupported resolution");
    };
    let id = match crate::params::parse_instrument(&q.symbol) {
        Ok(id) => id,
        Err(e) => return udf_error(&e.message),
    };
    if s.svc.store.require_instrument(&id).await.is_err() {
        return udf_error("unknown_symbol");
    }
    match history_json(&s.svc, &id, interval, q.from, q.to, q.countback).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => udf_error(&e.message),
    }
}

async fn time() -> String {
    (mesh_types::UnixMillis::now().0 / 1000).to_string()
}

/// Kinds the UDF feed reads.
pub const SOURCE_KIND: RecordKind = RecordKind::Bar;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolutions_map_to_intervals() {
        for r in RESOLUTIONS {
            assert!(interval(r).is_some(), "{r}");
        }
        assert_eq!(interval("60"), Some(BarInterval::H1));
        assert!(interval("3").is_none());
        assert!((to_f64(100_250, 3) - 100.25).abs() < 1e-9);
    }
}
