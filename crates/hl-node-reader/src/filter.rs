//! Filtered raw: the public reproducibility root (ADR 0006).
//!
//! [`filter_line`] turns one raw node line into its filtered-raw line: the
//! same `--batch-by-block` line format, with
//!
//! - only admitted fields (owner addresses, fees, PnL, position, hashes,
//!   client order IDs, builder data, liquidation objects and order
//!   attributes the mesh does not use are removed; fill events become the
//!   bare fill object instead of `[user, fill]`),
//! - no events of markets outside ADM-0001 (spot),
//! - no fill groups that are not book trades (backstop liquidations,
//!   no-taker pairs such as halt settlements, unknown liquidation methods).
//!
//! Lines are never dropped from an admitted stream, even when no events
//! remain, so block continuity (and gap detection) survives filtering.
//! Lines of any other stream are dropped whole ([`filter_raw_line`]).
//!
//! Malformed fill groups (an unmatched fill, two crossed fills) keep their
//! admitted fields, so that parsing filtered raw yields exactly the records
//! (and `gap_before` flags) that parsing the original raw yields. Unknown
//! fields fail filtering just as they fail parsing.
//!
//! Properties (tested over every fixture and generated scenarios):
//! `parse(filter(raw)) == parse(raw)` and `filter(filter(raw)) == filter(raw)`.

use std::collections::HashSet;

use crate::assumptions::is_admitted_coin;
use crate::fills::{GroupClass, group_fills};
use crate::raw::{FillEvent, FillsLine, OrderStatusesLine, RawBookDiffsLine, Stream};

/// Stream directory names (configurable: assumption 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDirs {
    /// Fills directory.
    pub fills: String,
    /// Order-status directory.
    pub order_statuses: String,
    /// Raw-book-diff directory.
    pub raw_book_diffs: String,
}

impl Default for StreamDirs {
    fn default() -> Self {
        use crate::assumptions::{
            DEFAULT_FILLS_DIR, DEFAULT_ORDER_STATUSES_DIR, DEFAULT_RAW_BOOK_DIFFS_DIR,
        };
        Self {
            fills: DEFAULT_FILLS_DIR.into(),
            order_statuses: DEFAULT_ORDER_STATUSES_DIR.into(),
            raw_book_diffs: DEFAULT_RAW_BOOK_DIFFS_DIR.into(),
        }
    }
}

impl StreamDirs {
    /// Directory name of `stream`.
    #[must_use]
    pub fn get(&self, stream: Stream) -> &str {
        match stream {
            Stream::Fills => &self.fills,
            Stream::OrderStatuses => &self.order_statuses,
            Stream::RawBookDiffs => &self.raw_book_diffs,
        }
    }

    /// The stream a directory name holds, or `None` for any other directory
    /// (excluded streams such as `misc_events`, logs, state snapshots).
    #[must_use]
    pub fn stream_of(&self, dir_name: &str) -> Option<Stream> {
        Stream::ALL.into_iter().find(|&s| self.get(s) == dir_name)
    }
}

/// Error from filtering a line.
#[derive(Debug, thiserror::Error)]
#[error("cannot filter {stream} line: {source}")]
pub struct FilterError {
    /// Stream of the line.
    pub stream: Stream,
    /// Parse or write error.
    pub source: serde_json::Error,
}

/// Filters a parsed fills line in place.
pub fn filter_fills(line: &mut FillsLine) {
    let keep: HashSet<(String, u64)> = group_fills(line.events.iter().map(|e| &e.0))
        .into_iter()
        .filter(|g| is_admitted_coin(g.coin) && !matches!(g.class, GroupClass::Drop(_)))
        .map(|g| (g.coin.to_owned(), g.tid))
        .collect();
    let events = std::mem::take(&mut line.events);
    line.events = events
        .into_iter()
        .filter(|e| keep.contains(&(e.0.coin.clone(), e.0.tid)))
        .map(|e| FillEvent(e.0.admitted()))
        .collect();
}

/// Filters a parsed order-status line in place.
pub fn filter_order_statuses(line: &mut OrderStatusesLine) {
    line.events.retain(|e| is_admitted_coin(&e.order.coin));
}

/// Filters a parsed raw-book-diff line in place.
pub fn filter_raw_book_diffs(line: &mut RawBookDiffsLine) {
    line.events.retain(|e| is_admitted_coin(&e.coin));
}

/// Turns one raw line of an admitted stream into its filtered-raw line
/// (without a trailing newline).
///
/// # Errors
///
/// Fails on malformed lines and unknown fields (schema drift).
pub fn filter_line(stream: Stream, line: &str) -> Result<String, FilterError> {
    let err = |source| FilterError { stream, source };
    match stream {
        Stream::Fills => {
            let mut l: FillsLine = serde_json::from_str(line).map_err(err)?;
            filter_fills(&mut l);
            serde_json::to_string(&l).map_err(err)
        }
        Stream::OrderStatuses => {
            let mut l: OrderStatusesLine = serde_json::from_str(line).map_err(err)?;
            filter_order_statuses(&mut l);
            serde_json::to_string(&l).map_err(err)
        }
        Stream::RawBookDiffs => {
            let mut l: RawBookDiffsLine = serde_json::from_str(line).map_err(err)?;
            filter_raw_book_diffs(&mut l);
            serde_json::to_string(&l).map_err(err)
        }
    }
}

/// Filters a line by the name of the node directory it came from: lines of
/// admitted streams are filtered, lines of every other directory are
/// dropped (`Ok(None)`). Blank lines are dropped too.
///
/// # Errors
///
/// As [`filter_line`].
pub fn filter_raw_line(
    dirs: &StreamDirs,
    dir_name: &str,
    line: &str,
) -> Result<Option<String>, FilterError> {
    match dirs.stream_of(dir_name) {
        Some(_) if line.trim().is_empty() => Ok(None),
        Some(stream) => filter_line(stream, line).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn drops_non_book_pairs_and_spot() {
        let raw = r#"{"block_number":1,"block_time":"2026-01-01T00:00:00.000","events":[["0x01",{"coin":"SYN","crossed":false,"oid":1,"px":"1","side":"A","sz":"1","tid":5,"time":0,"liquidation":{"liquidatedUser":"0x01","markPx":"1","method":"backstop"}}],["0x02",{"coin":"SYN","crossed":false,"oid":2,"px":"1","side":"B","sz":"1","tid":5,"time":0}],["0x03",{"coin":"@4","crossed":true,"oid":3,"px":"1","side":"B","sz":"1","tid":6,"time":0}],["0x04",{"coin":"@4","crossed":false,"oid":4,"px":"1","side":"A","sz":"1","tid":6,"time":0}],["0x05",{"coin":"SYN","crossed":true,"oid":7,"px":"1","side":"B","sz":"1","tid":8,"time":0}]],"local_time":"2026-01-01T00:00:00.000000001"}"#;
        let out = filter_line(Stream::Fills, raw).unwrap();
        let l: FillsLine = serde_json::from_str(&out).unwrap();
        // Only the unmatched (anomalous) SYN fill remains.
        assert_eq!(l.events.len(), 1);
        assert_eq!(l.events[0].0.tid, 8);
        assert!(!out.contains("0x0"));
        assert_eq!(filter_line(Stream::Fills, &out).unwrap(), out);
    }

    #[test]
    fn by_directory() {
        let dirs = StreamDirs::default();
        assert_eq!(
            filter_raw_line(&dirs, "misc_events_by_block", "{\"anything\":1}").unwrap(),
            None
        );
        assert_eq!(
            filter_raw_line(&dirs, "node_fills_by_block", "  ").unwrap(),
            None
        );
        assert!(filter_raw_line(&dirs, "node_fills_by_block", "{").is_err());
        assert_eq!(
            dirs.stream_of("node_raw_book_diffs_by_block"),
            Some(Stream::RawBookDiffs)
        );
    }
}
