//! `/metrics` (Prometheus text) and `/health`.
//!
//! `/health` returns `200` while a listing poll has succeeded within the
//! staleness limit (or the process is still inside its startup grace
//! period), and `503` otherwise, with a small JSON body either way.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;

use crate::metrics::Metrics;

/// Liveness of the pollers.
#[derive(Debug)]
pub struct Health {
    started: Instant,
    last_ok: Mutex<Option<Instant>>,
    max_age: Duration,
}

impl Health {
    /// Healthy while a poll succeeded within `max_age`.
    #[must_use]
    pub fn new(max_age: Duration) -> Self {
        Self {
            started: Instant::now(),
            last_ok: Mutex::new(None),
            max_age,
        }
    }

    /// Records a successful poll.
    pub fn record_success(&self) {
        if let Ok(mut g) = self.last_ok.lock() {
            *g = Some(Instant::now());
        }
    }

    /// Current status and seconds since the last success.
    #[must_use]
    pub fn status(&self) -> (bool, Option<u64>) {
        let last = self.last_ok.lock().ok().and_then(|g| *g);
        let age = last.map(|t| t.elapsed());
        let ok = match age {
            Some(a) => a <= self.max_age,
            None => self.started.elapsed() <= self.max_age,
        };
        (ok, age.map(|a| a.as_secs()))
    }
}

#[derive(Debug, Clone)]
struct AppState {
    metrics: Arc<Metrics>,
    health: Arc<Health>,
}

async fn metrics(State(s): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        s.metrics.render(),
    )
}

async fn health(State(s): State<AppState>) -> impl IntoResponse {
    let (ok, age) = s.health.status();
    let body = serde_json::json!({
        "status": if ok { "ok" } else { "stale" },
        "seconds_since_last_successful_poll": age,
    })
    .to_string();
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, [(header::CONTENT_TYPE, "application/json")], body)
}

/// The HTTP router.
pub fn router(metrics_reg: Arc<Metrics>, health_state: Arc<Health>) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .route("/health", get(health))
        .with_state(AppState {
            metrics: metrics_reg,
            health: health_state,
        })
}

/// Serves the router on a bound listener until `shutdown` resolves.
///
/// # Errors
///
/// Fails if serving fails.
pub async fn serve(
    listener: tokio::net::TcpListener,
    metrics_reg: Arc<Metrics>,
    health_state: Arc<Health>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    if let Ok(addr) = listener.local_addr() {
        tracing::info!(%addr, "serving /metrics and /health");
    }
    axum::serve(listener, router(metrics_reg, health_state))
        .with_graceful_shutdown(shutdown)
        .await
}
