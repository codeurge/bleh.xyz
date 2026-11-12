//! HTTP endpoints and the websocket upgrade.
//!
//! | Path | |
//! |---|---|
//! | `GET /v0/ws` | Websocket (gateway-v0) |
//! | `GET /health` | 200 `{"status":"ok"}` when connected upstream and fresh, else 503 |
//! | `GET /freshness` | Per venue, stream and instrument: last block, block time, publish time, lag |
//! | `GET /metrics` | Prometheus text format |
//! | `GET /proof` | Inclusion proof: `venue`, `kind`, `block_number`, `instrument`, `event_index` |
//! | `GET /signers` | Commitment signer addresses (and key IDs when signing locally) |

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, PoisonError};

use axum::Json;
use axum::body::Body;
use axum::extract::{ConnectInfo, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hyper_util::rt::TokioIo;
use mesh_client::Encoding;
use mesh_client::protocol::WS_PATH;
use mesh_types::{InstrumentId, RecordKind, StreamId, UnixMillis, UnixNanos, VenueId};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

use crate::Shared;

pub(crate) fn router(shared: Arc<Shared>) -> axum::Router {
    axum::Router::new()
        .route(WS_PATH, get(ws_upgrade))
        .route("/health", get(health))
        .route("/freshness", get(freshness))
        .route("/metrics", get(metrics))
        .route("/proof", get(proof))
        .route("/signers", get(signers))
        .with_state(shared)
}

fn has_token(headers: &HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers.get_all(name).iter().any(|v| {
        v.to_str()
            .is_ok_and(|s| s.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    })
}

fn client_ip(shared: &Shared, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
    if shared.config.trust_forwarded_for
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse().ok())
    {
        return ip;
    }
    peer.ip()
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    (status, msg).into_response()
}

async fn ws_upgrade(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut req: Request,
) -> Response {
    let headers = req.headers();
    if !has_token(headers, header::CONNECTION, "upgrade")
        || !has_token(headers, header::UPGRADE, "websocket")
    {
        return plain(StatusCode::UPGRADE_REQUIRED, "websocket upgrade required\n");
    }
    if headers
        .get(header::SEC_WEBSOCKET_VERSION)
        .is_none_or(|v| v.as_bytes() != b"13")
    {
        return plain(StatusCode::BAD_REQUEST, "unsupported websocket version\n");
    }
    let Some(key) = headers.get(header::SEC_WEBSOCKET_KEY) else {
        return plain(StatusCode::BAD_REQUEST, "missing Sec-WebSocket-Key\n");
    };
    let accept = derive_accept_key(key.as_bytes());
    let offered: Vec<String> = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    // No subprotocol offered: JSON. Offered but none supported: accept the
    // first offered one so the close code 4001 reaches the client.
    let (encoding, echo) = if offered.is_empty() {
        (Some(Encoding::Json), None)
    } else if let Some((enc, name)) = offered
        .iter()
        .find_map(|s| Encoding::from_subprotocol(s).map(|e| (e, s.clone())))
    {
        (Some(enc), Some(name))
    } else {
        (None, offered.first().cloned())
    };
    let ip = client_ip(&shared, headers, peer);
    let limits = &shared.config.limits;
    let Some(guard) = shared.ips.try_connect(ip, limits.connections_per_ip) else {
        shared.metrics.rejected_ip_cap.inc();
        return plain(
            StatusCode::TOO_MANY_REQUESTS,
            "too many connections from this address\n",
        );
    };
    let max = usize::try_from(limits.max_message_bytes).unwrap_or(usize::MAX);
    let config = WebSocketConfig::default()
        .max_message_size(Some(max))
        .max_frame_size(Some(max))
        .write_buffer_size(128 * 1024)
        .max_write_buffer_size(
            usize::try_from(limits.send_buffer_bytes)
                .unwrap_or(usize::MAX)
                .max(256 * 1024),
        );
    let on_upgrade = hyper::upgrade::on(&mut req);
    let task_shared = Arc::clone(&shared);
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let ws = WebSocketStream::from_raw_socket(
                    TokioIo::new(upgraded),
                    Role::Server,
                    Some(config),
                )
                .await;
                crate::conn::run(task_shared, ws, encoding, guard).await;
            }
            Err(e) => tracing::debug!(error = %e, "websocket upgrade failed"),
        }
    });
    let mut resp = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_ACCEPT, accept);
    if let Some(name) = echo
        && let Ok(v) = HeaderValue::from_str(&name)
    {
        resp = resp.header(header::SEC_WEBSOCKET_PROTOCOL, v);
    }
    resp.body(Body::empty())
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "response error\n"))
}

async fn health(State(shared): State<Arc<Shared>>) -> Response {
    if shared.healthy() {
        (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response()
    } else {
        let upstream = shared.upstream.as_ref().map(|u| u.is_connected());
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status": "unavailable", "upstream_connected": upstream})),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
struct FreshRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    venue: Option<VenueId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instrument: Option<InstrumentId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<RecordKind>,
    last_block: u64,
    block_time_ms: UnixMillis,
    #[serde(skip_serializing_if = "Option::is_none")]
    ts_publish_ns: Option<UnixNanos>,
    /// Now minus block time.
    lag_ms: i64,
    /// Now minus when this gateway saw the block.
    age_ms: i64,
}

fn row(
    venue: Option<VenueId>,
    instrument: Option<InstrumentId>,
    kind: Option<RecordKind>,
    f: &crate::Fresh,
    now: UnixNanos,
) -> FreshRow {
    FreshRow {
        venue,
        instrument,
        kind,
        last_block: f.last_block,
        block_time_ms: f.block_time_ms,
        ts_publish_ns: f.ts_publish_ns,
        lag_ms: if f.block_time_ms.0 > 0 {
            now.to_millis().0 - f.block_time_ms.0
        } else {
            0
        },
        age_ms: (now.0 - f.seen_ns.0) / 1_000_000,
    }
}

async fn freshness(State(shared): State<Arc<Shared>>) -> Response {
    let now = UnixNanos::now();
    let st = shared.state.read().unwrap_or_else(PoisonError::into_inner);
    let mut venues: Vec<FreshRow> = st
        .venues
        .iter()
        .map(|(v, f)| row(Some(v.clone()), None, None, f, now))
        .collect();
    venues.sort_by(|a, b| a.venue.cmp(&b.venue));
    let mut streams: Vec<FreshRow> = st
        .streams
        .iter()
        .map(|((v, k), f)| row(Some(v.clone()), None, Some(*k), f, now))
        .collect();
    streams.sort_by(|a, b| (&a.venue, a.kind).cmp(&(&b.venue, b.kind)));
    let mut instruments: Vec<FreshRow> = st
        .channels
        .iter()
        .map(|((i, k), f)| row(None, Some(i.clone()), Some(*k), f, now))
        .collect();
    instruments.sort_by(|a, b| (&a.instrument, a.kind).cmp(&(&b.instrument, b.kind)));
    drop(st);
    Json(serde_json::json!({
        "now_ns": now,
        "venues": venues,
        "streams": streams,
        "instruments": instruments,
    }))
    .into_response()
}

async fn metrics(State(shared): State<Arc<Shared>>) -> Response {
    let extra = shared.extra_metrics();
    let extra_ref: Vec<(&str, &str, u64)> = extra
        .iter()
        .map(|(n, h, v)| (n.as_str(), h.as_str(), *v))
        .collect();
    let mut body = shared.metrics.render(&extra_ref);
    let proofs = u64::try_from(shared.proofs.len()).unwrap_or(u64::MAX);
    body.push_str(&format!(
        "# HELP gateway_proof_store_blocks Block entries held for proofs\n# TYPE gateway_proof_store_blocks gauge\ngateway_proof_store_blocks {proofs}\n"
    ));
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProofQuery {
    venue: VenueId,
    kind: RecordKind,
    block_number: u64,
    instrument: InstrumentId,
    event_index: u32,
}

async fn proof(
    State(shared): State<Arc<Shared>>,
    q: Result<Query<ProofQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(Query(q)) = q else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": "bad_request",
                "message": "expected venue, kind, block_number, instrument, event_index"
            })),
        )
            .into_response();
    };
    let stream = StreamId {
        venue: q.venue,
        kind: q.kind,
    };
    match shared
        .proofs
        .lookup(&stream, q.block_number, &q.instrument, q.event_index)
    {
        Ok(p) => {
            shared.metrics.proofs_served.inc();
            Json(serde_json::json!({
                "commitment": p.commitment,
                "proof": p.proof,
                "record": p.record,
            }))
            .into_response()
        }
        Err(e) => {
            shared.metrics.proofs_unavailable.inc();
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"code": "proof_unavailable", "message": e.to_string()})),
            )
                .into_response()
        }
    }
}

async fn signers(State(shared): State<Arc<Shared>>) -> Response {
    Json(serde_json::json!({
        "signers": shared.signers(),
        "keys": shared.signers.keys(),
    }))
    .into_response()
}
