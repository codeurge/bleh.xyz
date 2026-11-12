//! Prometheus metrics, rendered in the text exposition format, and
//! `/metrics` plus `/health` over HTTP.
//!
//! Label values come from bounded sets (source names, outcomes, reasons,
//! documented column names). `/health` returns `200` while every enabled
//! source has polled successfully within its staleness limit (or the
//! process is inside its start-up grace period) and `503` otherwise.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;

type Labels = Vec<(&'static str, String)>;

#[derive(Debug, Default)]
struct Inner {
    counters: BTreeMap<(&'static str, Labels), u64>,
    gauges: BTreeMap<(&'static str, Labels), f64>,
}

/// Metric registry.
#[derive(Debug, Default)]
pub struct Metrics {
    inner: Mutex<Inner>,
}

const HELP: &[(&str, &str, &str)] = &[
    (
        "ref_fetches_total",
        "counter",
        "HTTP fetches by source and outcome",
    ),
    (
        "ref_fetch_bytes_total",
        "counter",
        "Bytes downloaded by source",
    ),
    (
        "ref_polls_total",
        "counter",
        "Poll cycles by source and outcome",
    ),
    (
        "ref_targets_total",
        "counter",
        "Fetched targets by source and outcome (changed, not_modified, unchanged, error)",
    ),
    (
        "ref_records_total",
        "counter",
        "Records emitted by source and kind",
    ),
    (
        "ref_revisions_total",
        "counter",
        "Records emitted as revisions (revision > 0) by source",
    ),
    (
        "ref_duplicates_total",
        "counter",
        "Observations dropped as unchanged, by source",
    ),
    (
        "ref_rows_skipped_total",
        "counter",
        "Source rows not emitted, by source and reason (excluded, missing, malformed)",
    ),
    (
        "ref_parse_errors_total",
        "counter",
        "Unreadable files or responses by source",
    ),
    (
        "ref_unknown_fields_total",
        "counter",
        "Undocumented columns, elements or series seen, by source and field",
    ),
    (
        "ref_stale_reports_total",
        "counter",
        "Report files whose date is older than the freshness limit, by source",
    ),
    (
        "ref_last_success_timestamp_seconds",
        "gauge",
        "Time of the last successful poll cycle, by source",
    ),
    (
        "ref_latest_value_date_seconds",
        "gauge",
        "Midnight UTC of the newest value date emitted, by source",
    ),
    (
        "ref_backfill_units_total",
        "counter",
        "Backfill units by source and outcome",
    ),
];

impl Metrics {
    /// A new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn key(name: &'static str, labels: &[(&'static str, &str)]) -> (&'static str, Labels) {
        let mut l: Labels = labels.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
        l.sort();
        (name, l)
    }

    /// Adds `v` to a counter.
    pub fn inc_by(&self, name: &'static str, labels: &[(&'static str, &str)], v: u64) {
        if let Ok(mut g) = self.inner.lock() {
            *g.counters.entry(Self::key(name, labels)).or_default() += v;
        }
    }

    /// Adds one to a counter.
    pub fn inc(&self, name: &'static str, labels: &[(&'static str, &str)]) {
        self.inc_by(name, labels, 1);
    }

    /// Sets a gauge.
    pub fn set(&self, name: &'static str, labels: &[(&'static str, &str)], v: f64) {
        if let Ok(mut g) = self.inner.lock() {
            g.gauges.insert(Self::key(name, labels), v);
        }
    }

    /// Raises a gauge to `v` if `v` is larger.
    pub fn set_max(&self, name: &'static str, labels: &[(&'static str, &str)], v: f64) {
        if let Ok(mut g) = self.inner.lock() {
            let e = g.gauges.entry(Self::key(name, labels)).or_insert(v);
            if v > *e {
                *e = v;
            }
        }
    }

    /// Reads a counter.
    #[must_use]
    pub fn counter(&self, name: &'static str, labels: &[(&'static str, &str)]) -> u64 {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.counters.get(&Self::key(name, labels)).copied())
            .unwrap_or(0)
    }

    /// Sum of a counter over all label sets.
    #[must_use]
    pub fn counter_sum(&self, name: &'static str) -> u64 {
        self.inner
            .lock()
            .map(|g| {
                g.counters
                    .iter()
                    .filter(|((n, _), _)| *n == name)
                    .map(|(_, v)| *v)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Renders every metric in the Prometheus text format.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let Ok(g) = self.inner.lock() else {
            return out;
        };
        for (name, kind, help) in HELP {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} {kind}");
            for ((n, labels), v) in &g.counters {
                if n == name {
                    let _ = writeln!(out, "{n}{} {v}", fmt_labels(labels));
                }
            }
            for ((n, labels), v) in &g.gauges {
                if n == name {
                    let _ = writeln!(out, "{n}{} {v}", fmt_labels(labels));
                }
            }
        }
        out
    }
}

fn fmt_labels(labels: &Labels) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let body: Vec<String> = labels
        .iter()
        .map(|(k, v)| {
            let v = v
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            format!("{k}=\"{v}\"")
        })
        .collect();
    format!("{{{}}}", body.join(","))
}

/// Liveness of the pollers: one entry per enabled source.
#[derive(Debug)]
pub struct Health {
    started: Instant,
    sources: Mutex<BTreeMap<String, (Duration, Option<Instant>)>>,
}

impl Health {
    /// Healthy while each registered source succeeded within its limit.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            sources: Mutex::new(BTreeMap::new()),
        }
    }

    /// Registers a source with its staleness limit.
    pub fn register(&self, source: &str, max_age: Duration) {
        if let Ok(mut g) = self.sources.lock() {
            g.insert(source.to_owned(), (max_age, None));
        }
    }

    /// Records a successful poll of `source`.
    pub fn record_success(&self, source: &str) {
        if let Ok(mut g) = self.sources.lock()
            && let Some(e) = g.get_mut(source)
        {
            e.1 = Some(Instant::now());
        }
    }

    /// Overall status and the stale sources.
    #[must_use]
    pub fn status(&self) -> (bool, Vec<String>) {
        let Ok(g) = self.sources.lock() else {
            return (false, Vec::new());
        };
        let stale: Vec<String> = g
            .iter()
            .filter(|(_, (max_age, last))| match last {
                Some(t) => t.elapsed() > *max_age,
                None => self.started.elapsed() > *max_age,
            })
            .map(|(k, _)| k.clone())
            .collect();
        (stale.is_empty(), stale)
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct AppState {
    metrics: Arc<Metrics>,
    health: Arc<Health>,
}

async fn metrics_handler(State(s): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        s.metrics.render(),
    )
}

async fn health_handler(State(s): State<AppState>) -> impl IntoResponse {
    let (ok, stale) = s.health.status();
    let body = serde_json::json!({
        "status": if ok { "ok" } else { "stale" },
        "stale_sources": stale,
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
pub fn router(metrics: Arc<Metrics>, health: Arc<Health>) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .with_state(AppState { metrics, health })
}

/// Serves the router on a bound listener until `shutdown` resolves.
///
/// # Errors
///
/// Fails if serving fails.
pub async fn serve(
    listener: tokio::net::TcpListener,
    metrics: Arc<Metrics>,
    health: Arc<Health>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    if let Ok(addr) = listener.local_addr() {
        tracing::info!(%addr, "serving /metrics and /health");
    }
    axum::serve(listener, router(metrics, health))
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_health() {
        let m = Metrics::new();
        m.inc(
            "ref_records_total",
            &[("source", "nyfed"), ("kind", "reference_value")],
        );
        m.inc_by(
            "ref_records_total",
            &[("source", "nyfed"), ("kind", "reference_value")],
            2,
        );
        m.set_max("ref_latest_value_date_seconds", &[("source", "nyfed")], 5.0);
        m.set_max("ref_latest_value_date_seconds", &[("source", "nyfed")], 3.0);
        let text = m.render();
        assert!(text.contains("ref_records_total{kind=\"reference_value\",source=\"nyfed\"} 3"));
        assert!(text.contains("ref_latest_value_date_seconds{source=\"nyfed\"} 5"));
        let h = Health::new();
        h.register("nyfed", Duration::from_secs(3600));
        assert!(h.status().0);
        h.register("ercot", Duration::ZERO);
        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(h.status().1, vec!["ercot".to_owned()]);
        h.record_success("ercot");
        // A zero limit is stale again immediately after.
        std::thread::sleep(Duration::from_millis(2));
        assert!(!h.status().0);
    }
}
