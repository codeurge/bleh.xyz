//! HTTP surface: router, middleware and shared extractors.
//!
//! | Prefix | Module |
//! |---|---|
//! | `/v0/...`, `/health`, `/metrics` | [`v0`] JSON API, [`replay`] websocket replay |
//! | `/databento/v0/...` | [`databento`] Databento-shaped historical API (DBN) |
//! | `/udf/...` | [`udf`] TradingView UDF datafeed |
//! | `/ccxt/...` | [`ccxt`] CCXT-style REST shim |
//! | `/mcp` | [`crate::mcp`] MCP streamable HTTP transport |

pub mod ccxt;
pub mod databento;
pub mod openapi;
pub mod replay;
pub mod udf;
pub mod v0;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Router;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use tower_http::cors::{Any, CorsLayer};

use crate::error::{ApiError, ErrorCode};
use crate::metrics;
use crate::service::Service;

/// Shared state of every handler.
#[derive(Debug, Clone)]
pub struct AppState {
    /// The service.
    pub svc: Service,
    /// Per-IP token buckets.
    pub limiter: Arc<RateLimiter>,
    /// Active replays per IP.
    pub replays: Arc<Mutex<HashMap<IpAddr, usize>>>,
}

impl AppState {
    /// State for a service.
    #[must_use]
    pub fn new(svc: Service) -> Self {
        let l = &svc.config.limits;
        let limiter = Arc::new(RateLimiter::new(l.rate_per_sec, l.rate_burst));
        Self {
            svc,
            limiter,
            replays: Arc::default(),
        }
    }
}

/// Token-bucket rate limiter keyed by client IP.
#[derive(Debug)]
pub struct RateLimiter {
    rate: f64,
    burst: f64,
    buckets: Mutex<HashMap<IpAddr, (f64, Instant)>>,
}

impl RateLimiter {
    /// A limiter refilling `rate` tokens per second up to `burst`.
    #[must_use]
    pub fn new(rate: f64, burst: f64) -> Self {
        Self {
            rate: rate.max(0.0),
            burst: burst.max(1.0),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Takes one token for `ip`; on refusal returns the seconds until one
    /// is available.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        if self.rate <= 0.0 {
            return Ok(());
        }
        let now = Instant::now();
        let Ok(mut b) = self.buckets.lock() else {
            return Ok(());
        };
        if b.len() > 100_000 {
            let (rate, burst) = (self.rate, self.burst);
            b.retain(|_, (t, at)| *t + now.duration_since(*at).as_secs_f64() * rate < burst);
        }
        let entry = b.entry(ip).or_insert((self.burst, now));
        let elapsed = now.duration_since(entry.1).as_secs_f64();
        entry.0 = (entry.0 + elapsed * self.rate).min(self.burst);
        entry.1 = now;
        if entry.0 >= 1.0 {
            entry.0 -= 1.0;
            Ok(())
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let wait = ((1.0 - entry.0) / self.rate).ceil().max(1.0) as u64;
            Err(wait)
        }
    }
}

/// Query-string extractor whose rejections are JSON [`ApiError`]s.
#[derive(Debug, Clone)]
pub struct Q<T>(pub T);

impl<T, S> FromRequestParts<S> for Q<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::from_request_parts(parts, state)
            .await
            .map(|q| Self(q.0))
            .map_err(|e| ApiError::bad_request(e.body_text()))
    }
}

/// The client address used for rate limits and logs.
#[must_use]
pub fn client_ip(
    req_headers: &axum::http::HeaderMap,
    connect: Option<SocketAddr>,
    trust_xff: bool,
) -> IpAddr {
    if trust_xff
        && let Some(v) = req_headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        && let Some(first) = v.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    connect.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |a| a.ip())
}

const KNOWN_ROUTES: &[&str] = &[
    "/health",
    "/metrics",
    "/v0/instruments",
    "/v0/trades",
    "/v0/bars",
    "/v0/book",
    "/v0/book-events",
    "/v0/footprint",
    "/v0/volume-profile",
    "/v0/coverage",
    "/v0/manifests",
    "/v0/limits",
    "/v0/openapi.json",
    "/v0/replay",
    "/databento/v0/timeseries.get_range",
    "/databento/v0/metadata.list_datasets",
    "/databento/v0/metadata.list_schemas",
    "/databento/v0/metadata.list_publishers",
    "/udf/config",
    "/udf/symbols",
    "/udf/search",
    "/udf/history",
    "/udf/time",
    "/ccxt",
    "/ccxt/markets",
    "/ccxt/trades",
    "/ccxt/ohlcv",
    "/ccxt/orderbook",
    "/ccxt/time",
    "/mcp",
];

/// Rate limiting, request logging and metrics for every request.
async fn observe(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let started = Instant::now();
    let path = req.uri().path().to_owned();
    let method = req.method().clone();
    let connect = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let ip = client_ip(req.headers(), connect, state.svc.config.trust_forwarded_for);
    let route = KNOWN_ROUTES
        .iter()
        .find(|r| path == **r || (path.starts_with("/mcp") && **r == "/mcp"))
        .copied()
        .unwrap_or("other");
    let exempt = matches!(route, "/health" | "/metrics") || method == Method::OPTIONS;
    let resp = if !exempt && let Err(wait) = state.limiter.check(ip) {
        metrics::global().inc_rate_limited();
        let mut e = ApiError::new(
            ErrorCode::RateLimited,
            "too many requests from this address; slow down",
        );
        e.retry_after = Some(wait);
        e.into_response()
    } else {
        next.run(req).await
    };
    let status = resp.status().as_u16();
    let elapsed = started.elapsed();
    metrics::global().observe_request(route, status, elapsed);
    tracing::info!(
        target: "history_api::access",
        %method,
        path,
        status,
        ip = %ip,
        ms = elapsed.as_secs_f64() * 1e3,
        "request"
    );
    resp
}

async fn not_found() -> ApiError {
    ApiError::new(
        ErrorCode::NotFound,
        "no such endpoint; see /v0/openapi.json",
    )
}

/// Builds the full router (without the MCP transport; see
/// [`router_with_mcp`]).
pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
        .expose_headers([
            header::RETRY_AFTER,
            header::HeaderName::from_static("x-bleh-size-units"),
            header::HeaderName::from_static("x-bleh-size-scales"),
        ]);
    Router::new()
        .merge(v0::routes())
        .merge(replay::routes())
        .merge(databento::routes())
        .merge(udf::routes())
        .merge(ccxt::routes())
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(state.clone(), observe))
        .layer(cors)
        .with_state(state)
}

/// The router with the MCP streamable HTTP transport mounted at `/mcp`.
pub fn router_with_mcp(state: AppState) -> Router {
    let mcp = crate::mcp::streamable_http_service(state.svc.clone());
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any)
        .expose_headers(Any);
    let mcp_router = Router::new()
        .nest_service("/mcp", mcp)
        .layer(axum::middleware::from_fn_with_state(state.clone(), observe))
        .layer(cors)
        .with_state(state.clone());
    router(state).merge(mcp_router)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_refuses_after_burst() {
        let l = RateLimiter::new(1.0, 3.0);
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(l.check(ip).is_ok());
        assert!(l.check(ip).is_ok());
        assert!(l.check(ip).is_ok());
        assert_eq!(l.check(ip), Err(1));
        let other: IpAddr = "192.0.2.2".parse().unwrap();
        assert!(l.check(other).is_ok());
        assert!(RateLimiter::new(0.0, 1.0).check(ip).is_ok());
    }

    #[test]
    fn client_ip_honours_forwarded_for_only_when_trusted() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-forwarded-for", "198.51.100.7, 10.0.0.1".parse().unwrap());
        let peer: SocketAddr = "10.0.0.1:5000".parse().unwrap();
        assert_eq!(client_ip(&h, Some(peer), true).to_string(), "198.51.100.7");
        assert_eq!(client_ip(&h, Some(peer), false).to_string(), "10.0.0.1");
        assert_eq!(client_ip(&h, None, false).to_string(), "0.0.0.0");
    }
}
