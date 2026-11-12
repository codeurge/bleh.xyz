//! Prometheus metrics, in the text exposition format, with no extra
//! dependency: request counts by route and status, request and query
//! latency histograms, rate-limit rejections, rows served and active
//! replays.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const BUCKETS: [f64; 12] = [
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

#[derive(Debug, Default)]
struct Histogram {
    counts: [u64; BUCKETS.len()],
    sum: f64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, secs: f64) {
        for (i, b) in BUCKETS.iter().enumerate() {
            if secs <= *b {
                self.counts[i] += 1;
            }
        }
        self.sum += secs;
        self.count += 1;
    }

    fn render(&self, out: &mut String, name: &str, labels: &str) {
        let sep = if labels.is_empty() { "" } else { "," };
        for (i, b) in BUCKETS.iter().enumerate() {
            let _ = writeln!(
                out,
                "{name}_bucket{{{labels}{sep}le=\"{b}\"}} {}",
                self.counts[i]
            );
        }
        let _ = writeln!(
            out,
            "{name}_bucket{{{labels}{sep}le=\"+Inf\"}} {}",
            self.count
        );
        let braces = if labels.is_empty() {
            String::new()
        } else {
            format!("{{{labels}}}")
        };
        let _ = writeln!(out, "{name}_sum{braces} {}", self.sum);
        let _ = writeln!(out, "{name}_count{braces} {}", self.count);
    }
}

/// Process-wide metrics registry.
#[derive(Debug, Default)]
pub struct Metrics {
    requests: Mutex<BTreeMap<(String, u16), u64>>,
    latency: Mutex<BTreeMap<String, Histogram>>,
    queries: Mutex<Histogram>,
    rate_limited: AtomicU64,
    rows: AtomicU64,
    replays_active: AtomicU64,
    replay_messages: AtomicU64,
}

/// The global registry.
pub fn global() -> &'static Metrics {
    static M: OnceLock<Metrics> = OnceLock::new();
    M.get_or_init(Metrics::default)
}

impl Metrics {
    /// Records one HTTP request on a route template.
    pub fn observe_request(&self, route: &str, status: u16, elapsed: Duration) {
        if let Ok(mut m) = self.requests.lock() {
            *m.entry((route.to_owned(), status)).or_default() += 1;
        }
        if let Ok(mut m) = self.latency.lock() {
            m.entry(route.to_owned())
                .or_default()
                .observe(elapsed.as_secs_f64());
        }
    }

    /// Records one DuckDB query.
    pub fn observe_query(&self, elapsed: Duration) {
        if let Ok(mut h) = self.queries.lock() {
            h.observe(elapsed.as_secs_f64());
        }
    }

    /// Counts a rate-limited request.
    pub fn inc_rate_limited(&self) {
        self.rate_limited.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts rows served.
    pub fn add_rows(&self, n: usize) {
        self.rows
            .fetch_add(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// A replay started (`+1`) or ended (`-1`).
    pub fn replay_delta(&self, started: bool) {
        if started {
            self.replays_active.fetch_add(1, Ordering::Relaxed);
        } else {
            self.replays_active.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Counts replay messages sent.
    pub fn add_replay_messages(&self, n: u64) {
        self.replay_messages.fetch_add(n, Ordering::Relaxed);
    }

    /// Renders the text exposition format.
    #[must_use]
    pub fn render(&self, busy_slots: usize) -> String {
        let mut out = String::new();
        out.push_str("# HELP bleh_history_requests_total HTTP requests by route and status.\n");
        out.push_str("# TYPE bleh_history_requests_total counter\n");
        if let Ok(m) = self.requests.lock() {
            for ((route, status), n) in m.iter() {
                let _ = writeln!(
                    out,
                    "bleh_history_requests_total{{route=\"{}\",status=\"{status}\"}} {n}",
                    escape(route)
                );
            }
        }
        out.push_str("# HELP bleh_history_request_seconds HTTP request latency.\n");
        out.push_str("# TYPE bleh_history_request_seconds histogram\n");
        if let Ok(m) = self.latency.lock() {
            for (route, h) in m.iter() {
                h.render(
                    &mut out,
                    "bleh_history_request_seconds",
                    &format!("route=\"{}\"", escape(route)),
                );
            }
        }
        out.push_str("# HELP bleh_history_query_seconds DuckDB query latency.\n");
        out.push_str("# TYPE bleh_history_query_seconds histogram\n");
        if let Ok(h) = self.queries.lock() {
            h.render(&mut out, "bleh_history_query_seconds", "");
        }
        let gauges = [
            (
                "bleh_history_rate_limited_total",
                "counter",
                "Requests rejected by the per-IP rate limit.",
                self.rate_limited.load(Ordering::Relaxed),
            ),
            (
                "bleh_history_rows_total",
                "counter",
                "Records served by the JSON API.",
                self.rows.load(Ordering::Relaxed),
            ),
            (
                "bleh_history_replays_active",
                "gauge",
                "Websocket replays in progress.",
                self.replays_active.load(Ordering::Relaxed),
            ),
            (
                "bleh_history_replay_messages_total",
                "counter",
                "Messages sent by websocket replays.",
                self.replay_messages.load(Ordering::Relaxed),
            ),
            (
                "bleh_history_query_slots_busy",
                "gauge",
                "Query slots in use.",
                u64::try_from(busy_slots).unwrap_or(u64::MAX),
            ),
        ];
        for (name, kind, help, v) in gauges {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {v}"
            );
        }
        out
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_exposition_format() {
        let m = Metrics::default();
        m.observe_request("/v0/trades", 200, Duration::from_millis(3));
        m.observe_request("/v0/trades", 200, Duration::from_millis(30));
        m.observe_query(Duration::from_millis(2));
        m.inc_rate_limited();
        m.add_rows(5);
        let text = m.render(1);
        assert!(
            text.contains("bleh_history_requests_total{route=\"/v0/trades\",status=\"200\"} 2")
        );
        assert!(
            text.contains(
                "bleh_history_request_seconds_bucket{route=\"/v0/trades\",le=\"0.005\"} 1"
            )
        );
        assert!(text.contains("bleh_history_request_seconds_count{route=\"/v0/trades\"} 2"));
        assert!(text.contains("bleh_history_query_seconds_count 1"));
        assert!(text.contains("bleh_history_rate_limited_total 1"));
        assert!(text.contains("bleh_history_rows_total 5"));
        assert!(text.contains("bleh_history_query_slots_busy 1"));
    }
}
