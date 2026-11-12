//! Model Context Protocol server, built on the official Rust SDK
//! ([`rmcp`]), with stdio and streamable HTTP transports.
//!
//! Tools: `list_instruments`, `get_trades`, `get_bars`, `get_book_at`,
//! `get_volume_profile`, `get_coverage`, `explain_provenance`. Each calls
//! the same [`Service`] as the JSON API, so limits and validation are the
//! same; pages are capped at [`MCP_MAX_ROWS`] to keep tool results small
//! enough for a model's context.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult};
use crate::service::{RangeQuery, Service};

/// Largest page a tool returns.
pub const MCP_MAX_ROWS: usize = 500;
const MCP_DEFAULT_ROWS: usize = 100;

const INSTRUCTIONS: &str = "Read-only access to the bleh.xyz market-data archive: an open, \
signed, freely redistributable history of venue trades, order-level book events, book \
snapshots and precomputed bars. Prices and sizes are exact decimal strings. Times accept \
milliseconds since the epoch, RFC 3339 or YYYY-MM-DD (UTC). Start with list_instruments, \
then get_coverage to see which hours exist, then query. Every record carries provenance \
(admission, block sequence, flags); explain_provenance shows how to verify one against the \
signed daily manifests.";

/// The MCP server.
#[derive(Debug, Clone)]
pub struct HistoryMcp {
    svc: Service,
}

impl HistoryMcp {
    /// A server over a service.
    #[must_use]
    pub const fn new(svc: Service) -> Self {
        Self { svc }
    }
}

fn schema(v: Value) -> Arc<JsonObject> {
    match v {
        Value::Object(m) => Arc::new(m),
        _ => Arc::new(JsonObject::new()),
    }
}

fn time_props() -> Value {
    json!({
        "start": {"type": "string", "description": "Inclusive start: ms since the epoch, RFC 3339 or YYYY-MM-DD (UTC)."},
        "end": {"type": "string", "description": "Exclusive end, same formats."},
    })
}

fn props(extra: Value, with_time: bool, with_page: bool) -> Value {
    let mut m = serde_json::Map::new();
    if let Value::Object(e) = extra {
        m.extend(e);
    }
    if with_time && let Value::Object(t) = time_props() {
        m.extend(t);
    }
    if with_page {
        m.insert(
            "limit".into(),
            json!({"type": "integer", "minimum": 1, "maximum": MCP_MAX_ROWS, "description": "Page size (default 100)."}),
        );
        m.insert(
            "cursor".into(),
            json!({"type": "string", "description": "next_cursor from the previous page."}),
        );
    }
    Value::Object(m)
}

fn instrument_prop() -> Value {
    json!({"instrument": {"type": "string", "description": "Instrument ID, for example hyperliquid:perp:BTC (see list_instruments)."}})
}

/// The tool definitions.
#[must_use]
pub fn tools() -> Vec<Tool> {
    let obj = |p: Value, required: &[&str]| {
        schema(
            json!({"type": "object", "properties": p, "required": required, "additionalProperties": false}),
        )
    };
    vec![
        Tool::new(
            "list_instruments",
            "List instruments in the registry and the archive, with price and size scales, admission, flags and the dates present.",
            obj(
                props(
                    json!({
                        "query": {"type": "string", "description": "Case-insensitive substring of the ID or symbol."},
                        "venue": {"type": "string"},
                        "class": {"type": "string", "description": "perp, spot, outcome or swap."},
                    }),
                    false,
                    true,
                ),
                &[],
            ),
        ),
        Tool::new(
            "get_trades",
            "Venue trades for one instrument over a time range (default: the hour after start), in block order.",
            obj(props(instrument_prop(), true, true), &["instrument"]),
        ),
        Tool::new(
            "get_bars",
            "Precomputed OHLCV bars (method bars@1) for one instrument and interval over a time range.",
            obj(
                props(
                    {
                        let mut p = instrument_prop();
                        p["interval"] = json!({"type": "string", "enum": ["1s", "1m", "5m", "15m", "1h", "1d"], "description": "Default 1m."});
                        p
                    },
                    true,
                    true,
                ),
                &["instrument"],
            ),
        ),
        Tool::new(
            "get_book_at",
            "The order book at time T: nearest checkpoint snapshot plus book events up to T.",
            obj(
                {
                    let mut p = instrument_prop();
                    p["at"] =
                        json!({"type": "string", "description": "Time T (default: latest data)."});
                    p["depth"] = json!({"type": "string", "enum": ["l2", "l4"], "description": "l2 levels (default) or l4 orders."});
                    p["levels"] = json!({"type": "integer", "minimum": 1, "maximum": 100, "description": "Price levels per side (default 10)."});
                    p
                },
                &["instrument"],
            ),
        ),
        Tool::new(
            "get_volume_profile",
            "Precomputed volume profiles (volume by price bucket, split by aggressor) overlapping a time range.",
            obj(props(instrument_prop(), true, true), &["instrument"]),
        ),
        Tool::new(
            "get_coverage",
            "Recorded, backfilled and missing hours per market, as runs of equal status. Gaps are never filled.",
            obj(
                props(
                    json!({"instrument": {"type": "string", "description": "One instrument (default: all)."}}),
                    true,
                    false,
                ),
                &[],
            ),
        ),
        Tool::new(
            "explain_provenance",
            "Fetch one record and explain its provenance: admission, venue, block sequence, flags, derivation method, the archive file and manifest entry, and how to verify it.",
            obj(
                {
                    let mut p = instrument_prop();
                    p["kind"] = json!({"type": "string", "enum": ["trade", "book_event", "book_snapshot", "bar", "footprint", "volume_profile"]});
                    p["block_number"] = json!({"type": "integer", "minimum": 0});
                    p["event_index"] = json!({"type": "integer", "minimum": 0, "description": "Default 0 (snapshots: the end-of-block marker)."});
                    p["time"] = json!({"type": "string", "description": "Any time on the record's UTC day (its ts_source_ms, or start_ms for bars)."});
                    p
                },
                &["instrument", "kind", "block_number", "time"],
            ),
        ),
    ]
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Args {
    instrument: Option<String>,
    query: Option<String>,
    venue: Option<String>,
    class: Option<String>,
    start: Option<String>,
    end: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    interval: Option<String>,
    at: Option<String>,
    depth: Option<String>,
    levels: Option<usize>,
    kind: Option<String>,
    block_number: Option<u64>,
    event_index: Option<u32>,
    time: Option<String>,
}

impl Args {
    fn range(&self) -> RangeQuery {
        RangeQuery {
            instrument: self.instrument.clone().unwrap_or_default(),
            start: self.start.clone(),
            end: self.end.clone(),
            limit: Some(self.limit.unwrap_or(MCP_DEFAULT_ROWS).min(MCP_MAX_ROWS)),
            cursor: self.cursor.clone(),
            interval: self.interval.clone(),
        }
    }

    fn need<'a>(v: &'a Option<String>, name: &str) -> ApiResult<&'a str> {
        v.as_deref()
            .ok_or_else(|| ApiError::bad_request(format!("{name} is required")))
    }
}

fn to_value<T: serde::Serialize>(v: &T) -> ApiResult<Value> {
    serde_json::to_value(v).map_err(ApiError::internal)
}

/// Runs one tool by name with JSON arguments.
///
/// # Errors
///
/// `bad_request` for unknown tools or arguments; service errors.
pub async fn call(svc: &Service, name: &str, args: Option<JsonObject>) -> ApiResult<Value> {
    let a: Args = serde_json::from_value(Value::Object(args.unwrap_or_default()))
        .map_err(|e| ApiError::bad_request(format!("arguments: {e}")))?;
    match name {
        "list_instruments" => to_value(
            &svc.instruments(
                a.query.as_deref(),
                a.venue.as_deref(),
                a.class.as_deref(),
                Some(a.limit.unwrap_or(MCP_DEFAULT_ROWS).min(MCP_MAX_ROWS)),
                a.cursor.as_deref(),
            )
            .await?,
        ),
        "get_trades" => to_value(&svc.trades(&a.range()).await?),
        "get_bars" => to_value(&svc.bars(&a.range()).await?),
        "get_volume_profile" => to_value(&svc.volume_profiles(&a.range()).await?),
        "get_book_at" => to_value(
            &svc.book_at(
                Args::need(&a.instrument, "instrument")?,
                a.at.as_deref(),
                Some(a.depth.as_deref().unwrap_or("l2")),
                Some(a.levels.unwrap_or(10).min(100)),
            )
            .await?,
        ),
        "get_coverage" => {
            let c = svc
                .coverage(
                    a.instrument.as_deref(),
                    a.start.as_deref(),
                    a.end.as_deref(),
                )
                .await?;
            let instruments: Vec<Value> = c
                .instruments
                .iter()
                .map(|i| {
                    let mut runs: Vec<Value> = Vec::new();
                    let mut cur: Option<(i64, i64, &str, bool)> = None;
                    for h in &i.hours {
                        match cur {
                            Some((s, _, st, g)) if st == h.status && g == h.gap_before && !g => {
                                cur = Some((s, h.hour_ms + crate::params::HOUR_MS, st, g));
                            }
                            _ => {
                                if let Some((s, e, st, g)) = cur {
                                    runs.push(json!({"from_ms": s, "to_ms": e, "status": st, "gap_before": g}));
                                }
                                cur = Some((h.hour_ms, h.hour_ms + crate::params::HOUR_MS, h.status, h.gap_before));
                            }
                        }
                    }
                    if let Some((s, e, st, g)) = cur {
                        runs.push(json!({"from_ms": s, "to_ms": e, "status": st, "gap_before": g}));
                    }
                    json!({
                        "instrument": i.instrument,
                        "recorded_hours": i.recorded_hours,
                        "backfilled_hours": i.backfilled_hours,
                        "missing_hours": i.missing_hours,
                        "gap_hours": i.gap_hours,
                        "runs": runs,
                    })
                })
                .collect();
            Ok(
                json!({"start_ms": c.start_ms, "end_ms": c.end_ms, "note": c.note, "instruments": instruments}),
            )
        }
        "explain_provenance" => {
            svc.explain_provenance(
                Args::need(&a.instrument, "instrument")?,
                Args::need(&a.kind, "kind")?,
                a.block_number
                    .ok_or_else(|| ApiError::bad_request("block_number is required"))?,
                a.event_index,
                Args::need(&a.time, "time")?,
            )
            .await
        }
        other => Err(ApiError::bad_request(format!("unknown tool {other:?}"))),
    }
}

impl ServerHandler for HistoryMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "bleh-history",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|t| t.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if !tools().iter().any(|t| t.name == request.name) {
            return Err(McpError::invalid_params(
                format!("unknown tool {}", request.name),
                None,
            ));
        }
        let result = match call(&self.svc, &request.name, request.arguments).await {
            Ok(v) => CallToolResult::structured(v),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!(
                "{}: {}",
                e.code.as_str(),
                e.message
            ))]),
        };
        Ok(result.into())
    }
}

/// Serves MCP over stdin/stdout until the client disconnects.
///
/// # Errors
///
/// Transport errors.
pub async fn serve_stdio(svc: Service) -> anyhow::Result<()> {
    use rmcp::ServiceExt;
    let running = HistoryMcp::new(svc)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("mcp: {e}"))?;
    running.waiting().await?;
    Ok(())
}

/// The streamable HTTP transport as a tower service, for mounting at
/// `/mcp`.
#[must_use]
pub fn streamable_http_service(
    svc: Service,
) -> StreamableHttpService<HistoryMcp, LocalSessionManager> {
    let hosts = svc.config.mcp_allowed_hosts.clone();
    let mut config = StreamableHttpServerConfig::default();
    config = if hosts.is_empty() {
        config.disable_allowed_hosts()
    } else {
        config.with_allowed_hosts(hosts)
    };
    StreamableHttpService::new(
        move || Ok(HistoryMcp::new(svc.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_are_complete() {
        let names: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(
            names,
            [
                "list_instruments",
                "get_trades",
                "get_bars",
                "get_book_at",
                "get_volume_profile",
                "get_coverage",
                "explain_provenance"
            ]
        );
        for t in tools() {
            assert_eq!(t.input_schema["type"], "object");
            assert!(t.description.is_some());
        }
    }
}
