//! Metrics of the origin itself, rendered next to the node reader's on the
//! reader metrics listener (`HL_METRICS_ADDR`). The gateway, bus and signer
//! metrics are on the gateway's `/metrics`; live-capture counters go to the
//! capture textfile (`BLEH_CAPTURE_METRICS_OUT`), as the archive's do.

use std::fmt::Write as _;
use std::sync::Arc;

use hl_node_reader::metrics::Metrics;

/// Names, types and help of the origin's metrics.
pub mod names {
    /// Snapshots that differed from the book (the book was reset).
    pub const RECONCILE_MISMATCH: &str = "bleh_book_reconcile_mismatch_total";
    /// Snapshots compared with the book.
    pub const RECONCILE: &str = "bleh_book_reconciliations_total";
    /// Books that desynchronised, by reason (`gap`, `book`).
    pub const DESYNC: &str = "bleh_book_desyncs_total";
    /// Snapshots that could not be used.
    pub const SNAPSHOT_UNUSABLE: &str = "bleh_book_snapshots_unusable_total";
    /// Processor input errors.
    pub const PROCESS_ERRORS: &str = "bleh_book_process_errors_total";
    /// Blocks whose commitments could not be signed or published.
    pub const SIGNING_FAILURES: &str = "bleh_signing_failures_total";
    /// Batches or records the publisher rejected.
    pub const PUBLISH_ERRORS: &str = "bleh_publish_errors_total";
    /// Derived records published, by kind.
    pub const DERIVED: &str = "bleh_derived_records_total";
    /// Blocks published by the origin.
    pub const BLOCKS: &str = "bleh_origin_blocks_total";
    /// Last block published by the origin.
    pub const LAST_BLOCK: &str = "bleh_origin_last_block";
    /// Failed `fileSnapshot` requests.
    pub const SNAPSHOT_FAILURES: &str = "hl_reader_snapshot_failures_total";
    /// Successful `fileSnapshot` requests.
    pub const SNAPSHOTS: &str = "hl_reader_snapshots_total";

    /// `(name, type, help)`.
    pub const ALL: &[(&str, &str, &str)] = &[
        (
            RECONCILE_MISMATCH,
            "counter",
            "Snapshots that differed from the book; the book was reset to the snapshot.",
        ),
        (RECONCILE, "counter", "Snapshots compared with the book."),
        (DESYNC, "counter", "Books that desynchronised, by reason."),
        (
            SNAPSHOT_UNUSABLE,
            "counter",
            "L4 snapshots that could not be used (too old, stale or inconsistent).",
        ),
        (PROCESS_ERRORS, "counter", "Book processor input errors."),
        (
            SIGNING_FAILURES,
            "counter",
            "Blocks whose commitments could not be signed or published.",
        ),
        (
            PUBLISH_ERRORS,
            "counter",
            "Batches or records rejected by the publisher.",
        ),
        (DERIVED, "counter", "Derived records published, by kind."),
        (BLOCKS, "counter", "Blocks published by the origin."),
        (LAST_BLOCK, "gauge", "Last block published by the origin."),
        (
            SNAPSHOT_FAILURES,
            "counter",
            "Failed L4 fileSnapshot requests.",
        ),
        (SNAPSHOTS, "counter", "Successful L4 fileSnapshot requests."),
    ];
}

/// Renders the reader's metrics followed by the origin's, with `HELP` and
/// `TYPE` lines for the origin's names.
#[must_use]
pub fn render(reader: &Arc<Metrics>, origin: &Arc<Metrics>) -> String {
    let mut out = reader.render();
    let text = origin.render();
    let mut last: Option<String> = None;
    for line in text.lines() {
        let name = line.split(['{', ' ']).next().unwrap_or_default().to_owned();
        if last.as_deref() != Some(name.as_str()) {
            if let Some((_, kind, help)) = names::ALL.iter().find(|(n, _, _)| *n == name) {
                let _ = writeln!(out, "# HELP {name} {help}");
                let _ = writeln!(out, "# TYPE {name} {kind}");
            }
            last = Some(name);
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_help_for_origin_metrics() {
        let reader = Arc::new(Metrics::new());
        let origin = Arc::new(Metrics::new());
        origin.inc(names::RECONCILE_MISMATCH, &[]);
        origin.inc(names::DERIVED, &[("kind", "bar")]);
        let text = render(&reader, &origin);
        assert!(text.contains("# TYPE bleh_book_reconcile_mismatch_total counter"));
        assert!(text.contains("bleh_book_reconcile_mismatch_total 1"));
        assert!(text.contains("bleh_derived_records_total{kind=\"bar\"} 1"));
    }
}
