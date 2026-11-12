//! Prometheus metrics, rendered in the text exposition format.
//!
//! A small fixed set of atomic counters, gauges and histograms; no
//! registry crate. Labels are fixed at compile time, so cardinality is
//! bounded.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// A monotonically increasing counter.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Adds one.
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    /// Adds `n`.
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }
    /// Current value.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A gauge.
#[derive(Debug, Default)]
pub struct Gauge(AtomicI64);

impl Gauge {
    /// Sets the value.
    pub fn set(&self, v: i64) {
        self.0.store(v, Ordering::Relaxed);
    }
    /// Adds one.
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    /// Subtracts one.
    pub fn dec(&self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
    /// Current value.
    #[must_use]
    pub fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Latency bucket upper bounds, seconds.
pub const LATENCY_BUCKETS: [f64; 16] = [
    0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
    5.0, 10.0,
];

/// A histogram over [`LATENCY_BUCKETS`], fed in nanoseconds.
#[derive(Debug, Default)]
pub struct Histogram {
    buckets: [AtomicU64; 16],
    count: AtomicU64,
    sum_nanos: AtomicU64,
}

impl Histogram {
    /// Records one observation. Negative values (clock skew) count as zero.
    pub fn observe_nanos(&self, nanos: i64) {
        let n = u64::try_from(nanos).unwrap_or(0);
        #[allow(clippy::cast_precision_loss)]
        let secs = n as f64 / 1e9;
        if let Some(i) = LATENCY_BUCKETS.iter().position(|b| secs <= *b) {
            self.buckets[i].fetch_add(1, Ordering::Relaxed);
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_nanos.fetch_add(n, Ordering::Relaxed);
    }

    /// Number of observations.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Upper bound of the bucket holding quantile `q` (0 to 1), seconds;
    /// `None` without observations or beyond the last bucket.
    #[must_use]
    pub fn quantile_upper_bound(&self, q: f64) -> Option<f64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let want = ((total as f64) * q).ceil() as u64;
        let mut acc = 0;
        for (i, b) in self.buckets.iter().enumerate() {
            acc += b.load(Ordering::Relaxed);
            if acc >= want.max(1) {
                return Some(LATENCY_BUCKETS[i]);
            }
        }
        None
    }

    fn render(&self, out: &mut String, name: &str, help: &str) {
        let _ = writeln!(out, "# HELP {name} {help}");
        let _ = writeln!(out, "# TYPE {name} histogram");
        let mut acc = 0;
        for (i, b) in self.buckets.iter().enumerate() {
            acc += b.load(Ordering::Relaxed);
            let _ = writeln!(out, "{name}_bucket{{le=\"{}\"}} {acc}", LATENCY_BUCKETS[i]);
        }
        let count = self.count();
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {count}");
        #[allow(clippy::cast_precision_loss)]
        let sum = self.sum_nanos.load(Ordering::Relaxed) as f64 / 1e9;
        let _ = writeln!(out, "{name}_sum {sum}");
        let _ = writeln!(out, "{name}_count {count}");
    }
}

/// Every gateway metric.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Open websocket connections.
    pub connections: Gauge,
    /// Accepted connections.
    pub connections_total: Counter,
    /// Upgrades refused: per-IP cap.
    pub rejected_ip_cap: Counter,
    /// Connections closed: unsupported subprotocol.
    pub rejected_subprotocol: Counter,
    /// Messages sent.
    pub messages_sent: Counter,
    /// Bytes sent.
    pub bytes_sent: Counter,
    /// Client messages received.
    pub client_messages: Counter,
    /// Client messages rejected as malformed.
    pub bad_requests: Counter,
    /// Rate-limit hits.
    pub rate_limited: Counter,
    /// Connections closed for repeated rate-limit violations.
    pub rate_limit_disconnects: Counter,
    /// Slow consumers dropped to snapshot.
    pub slow_consumer_resnapshots: Counter,
    /// Slow consumers disconnected.
    pub slow_consumer_disconnects: Counter,
    /// Snapshots sent.
    pub snapshots_sent: Counter,
    /// Forced (unrequested) snapshots sent.
    pub forced_snapshots_sent: Counter,
    /// Proofs served (websocket and HTTP).
    pub proofs_served: Counter,
    /// Proof requests that could not be served.
    pub proofs_unavailable: Counter,
    /// Book contiguity breaks seen from upstream.
    pub upstream_gaps: Counter,
    /// Books replaced after a reconciliation mismatch.
    pub reconciliations: Counter,
    /// Bus events processed by the hub.
    pub hub_events: Counter,
    /// Times the hub fell behind the bus ring.
    pub hub_lagged: Counter,
    /// Blocks closed.
    pub blocks: Counter,
    /// Newest block number.
    pub last_block: Gauge,
    /// gateway send time minus record `ts_ingest_ns`.
    pub send_latency: Histogram,
    /// gateway send time minus record `ts_publish_ns`.
    pub publish_to_send: Histogram,
    /// Relay: blocks whose commitments all verified.
    pub relay_blocks_verified: Counter,
    /// Relay: streams rejected (bad signature, unknown signer, root
    /// mismatch, or no commitment).
    pub relay_streams_rejected: Counter,
    /// Relay: upstream connections established.
    pub relay_connects: Counter,
    /// Relay: 1 while connected upstream.
    pub relay_connected: Gauge,
}

impl Metrics {
    /// Renders every metric, plus extra lines appended by the caller.
    #[must_use]
    pub fn render(&self, extra: &[(&str, &str, u64)]) -> String {
        let mut out = String::with_capacity(8192);
        let gauges: [(&str, &str, i64); 4] = [
            (
                "gateway_connections",
                "Open websocket connections",
                self.connections.get(),
            ),
            (
                "gateway_last_block",
                "Newest block number closed",
                self.last_block.get(),
            ),
            (
                "relay_connected",
                "1 while the relay is connected upstream",
                self.relay_connected.get(),
            ),
            ("gateway_up", "Always 1 while the process serves metrics", 1),
        ];
        for (name, help, v) in gauges {
            let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {v}");
        }
        let counters: [(&str, &str, &Counter); 25] = [
            (
                "gateway_connections_total",
                "Accepted websocket connections",
                &self.connections_total,
            ),
            (
                "gateway_rejected_ip_cap_total",
                "Upgrades refused by the per-IP connection cap",
                &self.rejected_ip_cap,
            ),
            (
                "gateway_rejected_subprotocol_total",
                "Connections closed for an unsupported subprotocol",
                &self.rejected_subprotocol,
            ),
            (
                "gateway_messages_sent_total",
                "Messages sent to clients",
                &self.messages_sent,
            ),
            (
                "gateway_bytes_sent_total",
                "Bytes sent to clients",
                &self.bytes_sent,
            ),
            (
                "gateway_client_messages_total",
                "Messages received from clients",
                &self.client_messages,
            ),
            (
                "gateway_bad_requests_total",
                "Malformed client messages",
                &self.bad_requests,
            ),
            (
                "gateway_rate_limited_total",
                "Client messages refused by a rate limit",
                &self.rate_limited,
            ),
            (
                "gateway_rate_limit_disconnects_total",
                "Connections closed for repeated rate-limit violations",
                &self.rate_limit_disconnects,
            ),
            (
                "gateway_slow_consumer_resnapshots_total",
                "Slow consumers dropped to snapshot",
                &self.slow_consumer_resnapshots,
            ),
            (
                "gateway_slow_consumer_disconnects_total",
                "Slow consumers disconnected",
                &self.slow_consumer_disconnects,
            ),
            (
                "gateway_snapshots_sent_total",
                "Book snapshots sent",
                &self.snapshots_sent,
            ),
            (
                "gateway_forced_snapshots_sent_total",
                "Book snapshots sent without a client request",
                &self.forced_snapshots_sent,
            ),
            (
                "gateway_proofs_served_total",
                "Inclusion proofs served",
                &self.proofs_served,
            ),
            (
                "gateway_proofs_unavailable_total",
                "Inclusion proof requests not served",
                &self.proofs_unavailable,
            ),
            (
                "gateway_upstream_gaps_total",
                "Book contiguity breaks seen from upstream",
                &self.upstream_gaps,
            ),
            (
                "gateway_reconciliations_total",
                "Books replaced after a reconciliation mismatch",
                &self.reconciliations,
            ),
            (
                "gateway_hub_events_total",
                "Bus events processed",
                &self.hub_events,
            ),
            (
                "gateway_hub_lagged_total",
                "Times the hub fell behind the bus ring",
                &self.hub_lagged,
            ),
            ("gateway_blocks_total", "Blocks closed", &self.blocks),
            (
                "relay_blocks_verified_total",
                "Upstream blocks whose commitments verified",
                &self.relay_blocks_verified,
            ),
            (
                "relay_streams_rejected_total",
                "Upstream block streams rejected as unverifiable",
                &self.relay_streams_rejected,
            ),
            (
                "relay_connects_total",
                "Upstream connections established",
                &self.relay_connects,
            ),
            (
                "gateway_send_latency_observations_total",
                "Observations in gateway_send_latency_seconds",
                &Counter(AtomicU64::new(self.send_latency.count())),
            ),
            (
                "gateway_publish_to_send_observations_total",
                "Observations in gateway_publish_to_send_seconds",
                &Counter(AtomicU64::new(self.publish_to_send.count())),
            ),
        ];
        for (name, help, c) in counters {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {}",
                c.get()
            );
        }
        for (name, help, v) in extra {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {v}"
            );
        }
        self.send_latency.render(
            &mut out,
            "gateway_send_latency_seconds",
            "Gateway send time minus the record's ts_ingest_ns (node local write time)",
        );
        self.publish_to_send.render(
            &mut out,
            "gateway_publish_to_send_seconds",
            "Gateway send time minus the record's ts_publish_ns",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_buckets_and_rendering() {
        let m = Metrics::default();
        m.send_latency.observe_nanos(300_000); // 0.3 ms
        m.send_latency.observe_nanos(3_000_000); // 3 ms
        m.send_latency.observe_nanos(-5);
        m.send_latency.observe_nanos(60_000_000_000); // beyond the last bucket
        m.connections.inc();
        m.messages_sent.add(3);
        assert_eq!(m.send_latency.count(), 4);
        assert_eq!(m.send_latency.quantile_upper_bound(0.5), Some(0.0005));
        assert_eq!(m.send_latency.quantile_upper_bound(0.75), Some(0.005));
        assert_eq!(m.send_latency.quantile_upper_bound(1.0), None);
        let text = m.render(&[("bus_ring_published_total", "Items", 9)]);
        assert!(text.contains("gateway_connections 1"));
        assert!(text.contains("gateway_messages_sent_total 3"));
        assert!(text.contains("gateway_send_latency_seconds_bucket{le=\"0.0005\"} 2"));
        assert!(text.contains("gateway_send_latency_seconds_bucket{le=\"+Inf\"} 4"));
        assert!(text.contains("bus_ring_published_total 9"));
        assert!(Histogram::default().quantile_upper_bound(0.5).is_none());
    }
}
