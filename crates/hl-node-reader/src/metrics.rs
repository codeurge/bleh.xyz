//! Counters and gauges, rendered in the Prometheus text format.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::{Mutex, PoisonError};

/// Metric names and their help text. Every metric the crate writes is
/// listed here (and in `docs/operations/node-reader.md`).
pub mod names {
    /// Last block emitted by a reader.
    pub const READER_LAST_BLOCK: &str = "hl_reader_last_block";
    /// Seconds between now and the block time of a reader's last block.
    pub const READER_BLOCK_LAG: &str = "hl_reader_block_lag_seconds";
    /// Seconds between the node's `local_time` and the reader emitting the
    /// block (reader-side latency).
    pub const READER_EMIT_LAG: &str = "hl_reader_emit_lag_seconds";
    /// Blocks emitted by a reader.
    pub const READER_BLOCKS: &str = "hl_reader_blocks_total";
    /// Records emitted by a reader.
    pub const READER_RECORDS: &str = "hl_reader_records_total";
    /// Lines that failed to parse, by stream.
    pub const READER_PARSE_ERRORS: &str = "hl_reader_parse_errors_total";
    /// Lines dropped because their block went backwards or repeated.
    pub const READER_OUT_OF_ORDER: &str = "hl_reader_out_of_order_lines_total";
    /// Blocks released without every stream's line.
    pub const READER_INCOMPLETE: &str = "hl_reader_incomplete_blocks_total";
    /// Blocks rejected by normalization.
    pub const READER_REJECTED: &str = "hl_reader_rejected_blocks_total";
    /// Fill groups dropped as not book trades, by reason.
    pub const READER_DROPPED_PAIRS: &str = "hl_reader_dropped_fill_pairs_total";
    /// Malformed fill groups, by kind.
    pub const READER_ANOMALIES: &str = "hl_reader_fill_anomalies_total";
    /// Blocks with events for coins missing from the registry.
    pub const READER_UNKNOWN_COINS: &str = "hl_reader_unknown_coin_blocks_total";
    /// Events of markets outside the admission (spot), dropped.
    pub const READER_UNADMITTED: &str = "hl_reader_unadmitted_events_total";
    /// Unterminated lines cut off by hourly rotation.
    pub const READER_TRUNCATED: &str = "hl_reader_truncated_lines_total";
    /// Hourly file rotations followed.
    pub const READER_ROTATIONS: &str = "hl_reader_rotations_total";
    /// Instruments in a reader's registry.
    pub const REGISTRY_INSTRUMENTS: &str = "hl_registry_instruments";
    /// Registry refreshes from node metadata, by result.
    pub const REGISTRY_REFRESHES: &str = "hl_registry_refreshes_total";
    /// Blocks confirmed identical by both readers.
    pub const MERGE_CONFIRMED: &str = "hl_merge_confirmed_blocks_total";
    /// Blocks whose content differed between the readers.
    pub const MERGE_MISMATCHES: &str = "hl_merge_mismatches_total";
    /// Blocks one reader had and the other did not, by missing reader.
    pub const MERGE_ONE_SIDED: &str = "hl_merge_one_sided_gap_blocks_total";
    /// Blocks neither reader delivered before the output moved on.
    pub const MERGE_GAP_BLOCKS: &str = "hl_merge_gap_blocks_total";
    /// Copies that arrived after their block was emitted and could no
    /// longer be compared.
    pub const MERGE_LATE: &str = "hl_merge_late_blocks_total";
    /// Integrity alarms raised, by kind.
    pub const MERGE_ALARMS: &str = "hl_merge_alarms_total";
    /// Last block published.
    pub const OUTPUT_LAST_BLOCK: &str = "hl_output_last_block";
    /// Seconds between now and the block time of the last published block.
    pub const OUTPUT_BLOCK_LAG: &str = "hl_output_block_lag_seconds";
    /// Blocks published with a gap before them.
    pub const OUTPUT_GAPS: &str = "hl_output_gaps_total";
    /// Checkpoint writes, by result.
    pub const CHECKPOINTS: &str = "hl_checkpoint_writes_total";

    /// `(name, type, help)` for every metric.
    pub const ALL: &[(&str, &str, &str)] = &[
        (
            READER_LAST_BLOCK,
            "gauge",
            "Last block emitted by the reader.",
        ),
        (
            READER_BLOCK_LAG,
            "gauge",
            "Seconds from block time to now for the reader's last block.",
        ),
        (
            READER_EMIT_LAG,
            "gauge",
            "Seconds from the node's local_time to the reader emitting the block.",
        ),
        (READER_BLOCKS, "counter", "Blocks emitted by the reader."),
        (READER_RECORDS, "counter", "Records emitted by the reader."),
        (
            READER_PARSE_ERRORS,
            "counter",
            "Node lines that failed to parse.",
        ),
        (
            READER_OUT_OF_ORDER,
            "counter",
            "Node lines dropped because their block repeated or went backwards.",
        ),
        (
            READER_INCOMPLETE,
            "counter",
            "Blocks released without a line from every stream.",
        ),
        (
            READER_REJECTED,
            "counter",
            "Blocks rejected by normalization.",
        ),
        (
            READER_DROPPED_PAIRS,
            "counter",
            "Fill groups dropped as not book trades.",
        ),
        (READER_ANOMALIES, "counter", "Malformed fill groups."),
        (
            READER_UNKNOWN_COINS,
            "counter",
            "Blocks with events for coins missing from the registry.",
        ),
        (
            READER_UNADMITTED,
            "counter",
            "Events of markets outside ADM-0001, dropped.",
        ),
        (
            READER_TRUNCATED,
            "counter",
            "Unterminated lines cut off by hourly rotation.",
        ),
        (
            READER_ROTATIONS,
            "counter",
            "Hourly file rotations followed.",
        ),
        (
            REGISTRY_INSTRUMENTS,
            "gauge",
            "Instruments in the reader's registry.",
        ),
        (
            REGISTRY_REFRESHES,
            "counter",
            "Registry refreshes from node metadata.",
        ),
        (
            MERGE_CONFIRMED,
            "counter",
            "Blocks confirmed identical by both readers.",
        ),
        (
            MERGE_MISMATCHES,
            "counter",
            "Blocks whose content differed between the readers.",
        ),
        (
            MERGE_ONE_SIDED,
            "counter",
            "Blocks one reader delivered and the other did not.",
        ),
        (
            MERGE_GAP_BLOCKS,
            "counter",
            "Blocks neither reader delivered before the output moved on.",
        ),
        (
            MERGE_LATE,
            "counter",
            "Copies that arrived too late to compare.",
        ),
        (MERGE_ALARMS, "counter", "Integrity alarms raised."),
        (OUTPUT_LAST_BLOCK, "gauge", "Last block published."),
        (
            OUTPUT_BLOCK_LAG,
            "gauge",
            "Seconds from block time to now for the last published block.",
        ),
        (
            OUTPUT_GAPS,
            "counter",
            "Blocks published with a gap before them.",
        ),
        (CHECKPOINTS, "counter", "Checkpoint writes."),
    ];
}

type Key = (&'static str, Vec<(&'static str, String)>);

/// A thread-safe set of metric values.
#[derive(Debug, Default)]
pub struct Metrics {
    values: Mutex<BTreeMap<Key, f64>>,
}

fn key(name: &'static str, labels: &[(&'static str, &str)]) -> Key {
    (
        name,
        labels.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
    )
}

impl Metrics {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn with<R>(&self, f: impl FnOnce(&mut BTreeMap<Key, f64>) -> R) -> R {
        let mut g = self.values.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut g)
    }

    /// Adds `v` to a counter.
    pub fn add(&self, name: &'static str, labels: &[(&'static str, &str)], v: f64) {
        self.with(|m| *m.entry(key(name, labels)).or_default() += v);
    }

    /// Adds one to a counter.
    pub fn inc(&self, name: &'static str, labels: &[(&'static str, &str)]) {
        self.add(name, labels, 1.0);
    }

    /// Sets a gauge.
    pub fn set(&self, name: &'static str, labels: &[(&'static str, &str)], v: f64) {
        self.with(|m| m.insert(key(name, labels), v));
    }

    /// Current value (zero if never written).
    #[must_use]
    pub fn get(&self, name: &'static str, labels: &[(&'static str, &str)]) -> f64 {
        self.with(|m| m.get(&key(name, labels)).copied().unwrap_or(0.0))
    }

    /// Renders every written metric in the Prometheus text format.
    #[must_use]
    pub fn render(&self) -> String {
        let values = self.with(|m| m.clone());
        let mut out = String::new();
        let mut last: Option<&str> = None;
        for ((name, labels), v) in &values {
            if last != Some(name) {
                if let Some((_, kind, help)) = names::ALL.iter().find(|(n, _, _)| n == name) {
                    let _ = writeln!(out, "# HELP {name} {help}");
                    let _ = writeln!(out, "# TYPE {name} {kind}");
                }
                last = Some(name);
            }
            out.push_str(name);
            if !labels.is_empty() {
                out.push('{');
                for (i, (k, val)) in labels.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    let escaped = val
                        .replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('\n', "\\n");
                    let _ = write!(out, "{k}=\"{escaped}\"");
                }
                out.push('}');
            }
            let _ = writeln!(out, " {v}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_format() {
        let m = Metrics::new();
        m.inc(
            names::READER_PARSE_ERRORS,
            &[("reader", "a"), ("stream", "fills")],
        );
        m.inc(
            names::READER_PARSE_ERRORS,
            &[("reader", "a"), ("stream", "fills")],
        );
        m.set(names::OUTPUT_LAST_BLOCK, &[], 42.0);
        assert!(
            (m.get(
                names::READER_PARSE_ERRORS,
                &[("reader", "a"), ("stream", "fills")]
            ) - 2.0)
                .abs()
                < f64::EPSILON
        );
        let text = m.render();
        assert!(text.contains("# TYPE hl_reader_parse_errors_total counter\n"));
        assert!(text.contains("hl_reader_parse_errors_total{reader=\"a\",stream=\"fills\"} 2\n"));
        assert!(text.contains("hl_output_last_block 42\n"));
        let names: std::collections::BTreeSet<_> = names::ALL.iter().map(|(n, _, _)| n).collect();
        assert_eq!(names.len(), names::ALL.len());
    }
}
