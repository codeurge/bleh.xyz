//! The OpenAPI 3.1 document served at `/v0/openapi.json`, written by hand
//! so it documents behaviour (limits, cursors, lossy points) and not only
//! shapes. A test checks that every route in the router appears here.

use serde_json::{Value, json};

fn param(name: &str, required: bool, desc: &str, schema: &Value) -> Value {
    json!({"name": name, "in": "query", "required": required, "description": desc, "schema": schema})
}

fn s() -> Value {
    json!({"type": "string"})
}

fn int() -> Value {
    json!({"type": "integer", "minimum": 0})
}

fn instrument(required: bool) -> Value {
    param(
        "instrument",
        required,
        "Instrument ID, for example `hyperliquid:perp:BTC`.",
        &s(),
    )
}

fn range_params() -> Vec<Value> {
    vec![
        param(
            "start",
            false,
            "Inclusive start: ms since the epoch, RFC 3339 or `YYYY-MM-DD` (UTC).",
            &s(),
        ),
        param("end", false, "Exclusive end, same formats.", &s()),
    ]
}

fn page_params() -> Vec<Value> {
    vec![
        param(
            "limit",
            false,
            "Page size (default 1000, maximum per `/v0/limits`).",
            &int(),
        ),
        param(
            "cursor",
            false,
            "Opaque `next_cursor` of the previous page; valid only with the same other parameters.",
            &s(),
        ),
    ]
}

fn interval_param() -> Value {
    param(
        "interval",
        false,
        "Bar length: 1s, 1m (default), 5m, 15m, 1h, 1d. Tick, volume and range bars are not part of observation-v0 and return `unsupported_interval`.",
        &json!({"type": "string", "enum": ["1s", "1m", "5m", "15m", "1h", "1d"]}),
    )
}

fn get(summary: &str, desc: &str, params: Vec<Value>, schema_ref: &str) -> Value {
    json!({
        "get": {
            "summary": summary,
            "description": desc,
            "parameters": params,
            "responses": {
                "200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": schema_ref}}}},
                "default": {"description": "Error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}},
            },
        }
    })
}

fn cat(parts: Vec<Vec<Value>>) -> Vec<Value> {
    parts.into_iter().flatten().collect()
}

/// The document.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn document() -> Value {
    let page = "#/components/schemas/Page";
    let any = "#/components/schemas/Any";
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "bleh.xyz history API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Small JSON API over the static, signed Parquet archive (archive-layout-v0). \
                The archive itself is the primary interface: query it directly with DuckDB, Polars or ClickHouse. \
                Records are observation-v0 JSON: decimals are strings with exactly `scale` fractional digits, `*_ns` are strings. \
                Every query has a timeout, a memory cap and a row limit; pages carry an opaque `next_cursor`. \
                Errors are `{\"error\": {\"code\", \"message\"}}`.",
            "license": {"name": "Apache-2.0 (code); data CC0-1.0", "identifier": "Apache-2.0"},
        },
        "paths": {
            "/health": get("Health", "200 when the query engine answers.", vec![], any),
            "/metrics": {"get": {"summary": "Prometheus metrics", "responses": {"200": {"description": "text/plain exposition format"}}}},
            "/v0/openapi.json": get("This document", "OpenAPI 3.1.", vec![], any),
            "/v0/limits": get("Limits in force", "Timeouts, memory cap, page sizes, range caps, rate limits.", vec![], any),
            "/v0/instruments": get(
                "Instruments",
                "From the registry file and the archive partitions, with scales, flags, admission and dates present.",
                cat(vec![
                    vec![
                        param("q", false, "Substring of ID or symbol.", &s()),
                        param("venue", false, "Venue ID.", &s()),
                        param("class", false, "perp, spot, outcome, swap.", &s()),
                    ],
                    page_params(),
                ]),
                any,
            ),
            "/v0/trades": get("Trades", "Venue trades in block order (default range: one hour from start).", cat(vec![vec![instrument(true)], range_params(), page_params()]), page),
            "/v0/book-events": get("Book events", "Order-level book events in block order.", cat(vec![vec![instrument(true)], range_params(), page_params()]), page),
            "/v0/bars": get(
                "Bars",
                "Precomputed bars (method bars@1), read as stored. The range cap is the larger of the general cap and one page of bars.",
                cat(vec![vec![instrument(true), interval_param()], range_params(), page_params()]),
                page,
            ),
            "/v0/footprint": get("Footprints", "Precomputed footprints (per-price volume by aggressor).", cat(vec![vec![instrument(true), interval_param()], range_params(), page_params()]), page),
            "/v0/volume-profile": get("Volume profiles", "Precomputed volume profiles overlapping the range.", cat(vec![vec![instrument(true)], range_params(), page_params()]), page),
            "/v0/book": get(
                "Book at time T",
                "Nearest checkpoint snapshot at or before T plus every book event after it up to T (observation-v0 § 2.4). Returns a book_snapshot record at the end of the last block at or before T.",
                vec![
                    instrument(true),
                    param("at", false, "Time T (default: latest data).", &s()),
                    param("depth", false, "l4 (orders, default) or l2 (levels).", &json!({"type": "string", "enum": ["l4", "l2"]})),
                    param("levels", false, "Price levels per side.", &int()),
                ],
                any,
            ),
            "/v0/coverage": get(
                "Coverage",
                "Recorded, backfilled and missing hours per market. Missing means no trade or book event; gap_before marks source-reported gaps. Never filled.",
                cat(vec![vec![instrument(false)], range_params()]),
                any,
            ),
            "/v0/manifests": get(
                "Manifests",
                "Signed daily manifests with torrent (magnet, infohash) and IPFS (CID) links.",
                cat(vec![
                    range_params(),
                    vec![
                        param("archive", false, "Archive name.", &s()),
                        param("files", false, "Include the file list.", &json!({"type": "boolean"})),
                    ],
                ]),
                any,
            ),
            "/v0/replay": {"get": {
                "summary": "Websocket replay (gateway-v0)",
                "description": "Upgrade to a websocket. Server sends hello; client sends a gateway-v0 subscribe (channels: trade, book_event with depth, bar with interval); server replays snapshot, batch, record and block messages at `speed`× archive time (0 = as fast as possible), then closes with 1000.",
                "parameters": [
                    param("start", true, "Replay start.", &s()),
                    param("end", false, "Replay end (default start + 1 h).", &s()),
                    param("speed", false, "Multiplier; 0 = unthrottled.", &json!({"type": "number", "minimum": 0})),
                ],
                "responses": {"101": {"description": "Switching protocols"}},
            }},
            "/databento/v0/timeseries.get_range": {
                "get": {
                    "summary": "Databento-shaped historical range (DBN)",
                    "description": "Same parameters as Databento's timeseries.get_range: dataset (BLEH.HL), symbols (bleh IDs), schema (trades, mbo, mbp-1, mbp-10, ohlcv-1s/1m/1h/1d), start, end, stype_in=raw_symbol, limit, encoding (dbn, json, csv), compression (none, zstd), pretty_px, pretty_ts. Extension: size_units=lots|whole. Also accepts POST with a form body.",
                    "parameters": [
                        param("dataset", true, "BLEH.HL", &s()),
                        param("symbols", false, "Comma-separated instrument IDs or ALL_SYMBOLS.", &s()),
                        param("schema", true, "DBN schema.", &s()),
                        param("start", true, "Start.", &s()),
                        param("end", false, "End.", &s()),
                        param("limit", false, "Maximum records.", &int()),
                        param("encoding", false, "dbn, json, csv.", &s()),
                        param("compression", false, "none, zstd.", &s()),
                        param("size_units", false, "lots (lossless, default) or whole (rounded).", &s()),
                    ],
                    "responses": {"200": {"description": "DBN stream (application/octet-stream), JSON lines or CSV"}},
                },
            },
            "/databento/v0/metadata.list_datasets": get("Datasets", "Datasets offered.", vec![], any),
            "/databento/v0/metadata.list_schemas": get("Schemas", "Schemas offered.", vec![param("dataset", false, "Dataset.", &s())], any),
            "/databento/v0/metadata.list_publishers": get("Publishers", "bleh publisher IDs (private range, not Databento-assigned).", vec![], any),
            "/udf/config": get("UDF config", "TradingView UDF datafeed configuration.", vec![], any),
            "/udf/symbols": get("UDF symbol", "Symbol info.", vec![param("symbol", true, "Instrument ID.", &s())], any),
            "/udf/search": get("UDF search", "Symbol search.", vec![param("query", false, "Text.", &s()), param("type", false, "Class.", &s()), param("exchange", false, "Venue.", &s()), param("limit", false, "Max.", &int())], any),
            "/udf/history": get(
                "UDF history",
                "Bars as UDF arrays (numbers are f64).",
                vec![
                    param("symbol", true, "Instrument ID.", &s()),
                    param("resolution", true, "1S, 1, 5, 15, 60, 1D.", &s()),
                    param("from", true, "Unix seconds.", &int()),
                    param("to", true, "Unix seconds (inclusive).", &int()),
                    param("countback", false, "Bars ending at `to`.", &int()),
                ],
                any,
            ),
            "/udf/time": {"get": {"summary": "UDF server time", "responses": {"200": {"description": "Unix seconds as text"}}}},
            "/ccxt": get("CCXT describe", "Capabilities and timeframes.", vec![], any),
            "/ccxt/markets": get("CCXT fetchMarkets", "Market structures (symbol = instrument ID).", vec![], any),
            "/ccxt/trades": get("CCXT fetchTrades", "Trade structures; most recent without `since`.", vec![param("symbol", true, "Instrument ID.", &s()), param("since", false, "ms.", &int()), param("limit", false, "Max.", &int())], any),
            "/ccxt/ohlcv": get("CCXT fetchOHLCV", "[[t, o, h, l, c, v]].", vec![param("symbol", true, "Instrument ID.", &s()), param("timeframe", false, "1s, 1m, 5m, 15m, 1h, 1d.", &s()), param("since", false, "ms.", &int()), param("limit", false, "Max.", &int())], any),
            "/ccxt/orderbook": get("CCXT fetchOrderBook", "Order book at `timestamp` (default latest).", vec![param("symbol", true, "Instrument ID.", &s()), param("limit", false, "Levels.", &int()), param("timestamp", false, "ms.", &int())], any),
            "/ccxt/time": get("CCXT fetchTime", "Server time in ms.", vec![], any),
            "/mcp": {"post": {"summary": "MCP streamable HTTP transport", "description": "Model Context Protocol (JSON-RPC). Tools: list_instruments, get_trades, get_bars, get_book_at, get_volume_profile, get_coverage, explain_provenance.", "responses": {"200": {"description": "JSON-RPC response or SSE stream"}}}},
        },
        "components": {
            "schemas": {
                "Any": {"type": "object"},
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {"error": {"type": "object", "required": ["code", "message"], "properties": {
                        "code": {"type": "string", "enum": [
                            "bad_request", "not_found", "unknown_instrument", "unsupported_interval", "unsupported",
                            "range_too_large", "invalid_cursor", "rate_limited", "overloaded", "query_timeout",
                            "too_large", "archive_unavailable", "internal"
                        ]},
                        "message": {"type": "string"},
                    }}},
                },
                "Page": {
                    "type": "object",
                    "required": ["instrument", "kind", "start_ms", "end_ms", "count", "records", "next_cursor"],
                    "properties": {
                        "instrument": {"type": "string"},
                        "kind": {"type": "string"},
                        "start_ms": {"type": "integer"},
                        "end_ms": {"type": "integer"},
                        "count": {"type": "integer"},
                        "records": {"type": "array", "items": {"type": "object", "description": "observation-v0 record (docs/specs/observation-v0.schema.json)"}},
                        "next_cursor": {"type": ["string", "null"]},
                    },
                },
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_every_known_route() {
        let doc = document();
        let paths = doc["paths"].as_object().unwrap();
        for route in super::super::KNOWN_ROUTES {
            assert!(paths.contains_key(*route), "{route} missing from OpenAPI");
        }
        assert_eq!(doc["openapi"], "3.1.0");
    }
}
