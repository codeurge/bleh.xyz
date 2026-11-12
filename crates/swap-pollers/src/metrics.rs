//! Prometheus metrics, rendered in the text exposition format.
//!
//! A small registry with no external dependency: counters and gauges keyed
//! by name and a sorted label set. Label values come from a bounded set
//! (buckets, report kinds, reasons, documented column names).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Mutex;

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
        "swap_fetches_total",
        "counter",
        "HTTP fetches by target and outcome",
    ),
    (
        "swap_fetch_bytes_total",
        "counter",
        "Bytes downloaded by target",
    ),
    (
        "swap_files_total",
        "counter",
        "Report files captured by bucket, kind and outcome",
    ),
    ("swap_rows_total", "counter", "Data rows read by bucket"),
    (
        "swap_records_total",
        "counter",
        "SwapPrint records emitted by bucket and action",
    ),
    (
        "swap_records_skipped_total",
        "counter",
        "Rows with no record, by bucket and reason",
    ),
    (
        "swap_parse_errors_total",
        "counter",
        "Rejected rows and unreadable files by bucket and scope",
    ),
    (
        "swap_field_errors_total",
        "counter",
        "Optional cells dropped as malformed, by bucket and field",
    ),
    (
        "swap_unknown_columns_total",
        "counter",
        "Files containing an undocumented column, by bucket and column",
    ),
    (
        "swap_duplicates_total",
        "counter",
        "Records dropped as already emitted, by bucket",
    ),
    (
        "swap_orphan_updates_total",
        "counter",
        "Corrections and cancels whose original was not seen, by bucket",
    ),
    (
        "swap_dissemination_fallbacks_total",
        "counter",
        "Records whose dissemination time came from a fallback, by source",
    ),
    (
        "swap_raw_persist_errors_total",
        "counter",
        "Raw capture write failures by store",
    ),
    (
        "swap_last_dissemination_timestamp_seconds",
        "gauge",
        "Latest dissemination time seen, by bucket",
    ),
    (
        "swap_lag_seconds",
        "gauge",
        "Capture time minus latest dissemination time, by bucket",
    ),
    (
        "swap_last_successful_poll_timestamp_seconds",
        "gauge",
        "Time of the last successful listing poll, by listing",
    ),
];

impl Metrics {
    /// A new, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn key(name: &'static str, labels: &[(&'static str, &str)]) -> (&'static str, Labels) {
        let mut l: Labels = labels.iter().map(|(k, v)| (*k, (*v).to_string())).collect();
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

    /// Reads a counter (for tests and health).
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

    /// Reads a gauge.
    #[must_use]
    pub fn gauge(&self, name: &'static str, labels: &[(&'static str, &str)]) -> Option<f64> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.gauges.get(&Self::key(name, labels)).copied())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders() {
        let m = Metrics::new();
        m.inc("swap_rows_total", &[("bucket", "CFTC_IR")]);
        m.inc_by("swap_rows_total", &[("bucket", "CFTC_IR")], 2);
        m.set("swap_lag_seconds", &[("bucket", "SEC_EQ")], 1.5);
        let text = m.render();
        assert!(text.contains("swap_rows_total{bucket=\"CFTC_IR\"} 3"));
        assert!(text.contains("swap_lag_seconds{bucket=\"SEC_EQ\"} 1.5"));
        assert!(text.contains("# TYPE swap_records_total counter"));
        assert_eq!(m.counter("swap_rows_total", &[("bucket", "CFTC_IR")]), 3);
        assert_eq!(m.counter_sum("swap_rows_total"), 3);
    }
}
