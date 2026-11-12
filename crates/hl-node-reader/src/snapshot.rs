//! L4 book snapshots from the node's info server (spec §§ 7.2-7.4).

use std::path::Path;

use mesh_types::{
    BookSide, BookSnapshot, Price, Provenance, Registry, RestingOrder, Seq, Size, SnapshotDepth,
    UnixMillis, UnixNanos,
};
use serde::{Deserialize, Serialize};

use crate::assumptions::{L4_HEIGHT_KEY, L4_SNAPSHOT_KEY, is_admitted_coin, order_side};
use crate::http::{HttpError, InfoClient};
use crate::normalize::venue;
use crate::raw::Order;

/// One coin's book in the snapshot: `[coin, [bids, asks]]`.
pub type CoinBook = (String, (Vec<Order>, Vec<Order>));

/// The snapshot file with `includeHeightInOutput` (assumption 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct L4Envelope {
    /// Height of the block the books reflect.
    pub height: u64,
    /// Books per coin.
    pub snapshot: Vec<CoinBook>,
}

/// Errors from snapshot parsing.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The file is not the expected shape.
    #[error("L4 snapshot: {0}")]
    Format(#[from] serde_json::Error),
    /// A field cannot be mapped.
    #[error("L4 snapshot {coin}: {reason}")]
    Invalid {
        /// Market.
        coin: String,
        /// What is wrong.
        reason: String,
    },
    /// The info server request failed.
    #[error("L4 snapshot request: {0}")]
    Http(#[from] HttpError),
    /// The snapshot file could not be read.
    #[error("L4 snapshot file {path}: {source}")]
    Io {
        /// File.
        path: String,
        /// Error.
        source: std::io::Error,
    },
}

/// Parses the envelope. Assumption 8 lives here: the expected keys are
/// [`L4_HEIGHT_KEY`] and [`L4_SNAPSHOT_KEY`].
///
/// # Errors
///
/// Fails on any other shape, or unknown order fields.
pub fn parse_l4_envelope(text: &str) -> Result<L4Envelope, SnapshotError> {
    debug_assert_eq!(L4_HEIGHT_KEY, "height");
    debug_assert_eq!(L4_SNAPSHOT_KEY, "snapshot");
    Ok(serde_json::from_str(text)?)
}

/// A parsed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L4Parsed {
    /// Snapshot height.
    pub height: u64,
    /// One L4 `book_snapshot` per registered coin, `seq = end_of_block(height)`.
    pub snapshots: Vec<BookSnapshot>,
    /// Admitted coins missing from the registry (skipped).
    pub unknown_coins: Vec<String>,
    /// Trigger orders dropped.
    pub dropped_triggers: usize,
}

/// Maps an L4 snapshot to mesh `book_snapshot` records: trigger orders and
/// users dropped, one record per registered coin (spec § 7.4). The caller
/// supplies the timestamps: `ts_source_ms` should be the block time of the
/// snapshot height, `ts_ingest_ns` when the file was read.
///
/// # Errors
///
/// Fails on an unexpected shape, a decimal that does not fit the
/// instrument's scale, or a book that fails validation.
pub fn parse_l4_snapshot(
    text: &str,
    registry: &Registry,
    ts_source_ms: UnixMillis,
    ts_ingest_ns: UnixNanos,
) -> Result<L4Parsed, SnapshotError> {
    let env = parse_l4_envelope(text)?;
    let venue = venue();
    let mut out = L4Parsed {
        height: env.height,
        snapshots: Vec::new(),
        unknown_coins: Vec::new(),
        dropped_triggers: 0,
    };
    for (coin, (bids, asks)) in &env.snapshot {
        if !is_admitted_coin(coin) {
            continue;
        }
        let Some(spec) = registry.by_venue_symbol(&venue, coin) else {
            out.unknown_coins.push(coin.clone());
            continue;
        };
        let invalid = |reason: String| SnapshotError::Invalid {
            coin: coin.clone(),
            reason,
        };
        let mut side =
            |orders: &[Order], want: BookSide| -> Result<Vec<RestingOrder>, SnapshotError> {
                let mut v = Vec::with_capacity(orders.len());
                for o in orders {
                    if o.is_trigger() {
                        out.dropped_triggers += 1;
                        continue;
                    }
                    if order_side(&o.side) != Some(want) {
                        return Err(invalid(format!("order {} has side {:?}", o.oid, o.side)));
                    }
                    v.push(RestingOrder {
                        order_id: mesh_types::OrderId(o.oid),
                        price: Price::parse_with_scale(&o.limit_px, spec.price_scale)
                            .map_err(|e| invalid(format!("limitPx {:?}: {e}", o.limit_px)))?,
                        size: Size::parse_with_scale(&o.sz, spec.size_scale)
                            .map_err(|e| invalid(format!("sz {:?}: {e}", o.sz)))?,
                        ts_order_ms: UnixMillis(o.timestamp),
                    });
                }
                Ok(v)
            };
        let bids = side(bids, BookSide::Bid)?;
        let asks = side(asks, BookSide::Ask)?;
        let prov = Provenance::new(
            spec.id.clone(),
            spec.venue.clone(),
            Seq::end_of_block(env.height),
            ts_source_ms,
            ts_ingest_ns,
            spec.admission.clone(),
        )
        .with_flags(spec.flags);
        let snap = BookSnapshot {
            prov,
            depth: SnapshotDepth::L4 { bids, asks },
        };
        snap.validate().map_err(|e| invalid(e.to_string()))?;
        out.snapshots.push(snap);
    }
    Ok(out)
}

/// The filtered-raw form of an L4 snapshot file: same envelope, admitted
/// order fields only, no trigger orders, no markets outside ADM-0001.
///
/// # Errors
///
/// Fails on an unexpected shape or unknown fields.
pub fn filter_l4_snapshot(text: &str) -> Result<String, SnapshotError> {
    let mut env = parse_l4_envelope(text)?;
    env.snapshot.retain(|(coin, _)| is_admitted_coin(coin));
    for (_, (bids, asks)) in &mut env.snapshot {
        bids.retain(|o| !o.is_trigger());
        asks.retain(|o| !o.is_trigger());
    }
    Ok(serde_json::to_string(&env)?)
}

/// Asks the node to write an L4 snapshot to `out_path` (which the node
/// process must be able to write and this process to read) and returns
/// the file's contents.
///
/// # Errors
///
/// Fails if the request fails or the file cannot be read.
pub fn request_l4_snapshot(client: &InfoClient, out_path: &Path) -> Result<String, SnapshotError> {
    let path = out_path.display().to_string();
    client.post_json(&crate::assumptions::l4_snapshot_request(&path))?;
    std::fs::read_to_string(out_path).map_err(|source| SnapshotError::Io { path, source })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use mesh_types::{AdmissionId, InstrumentSpec, MarketClass};

    const SNAP: &str = r#"{"height":12,"snapshot":[["SYN",[[{"coin":"SYN","side":"B","limitPx":"9.9","sz":"1","oid":1,"timestamp":5,"isTrigger":false,"user":"0x01","cloid":null,"tif":"Gtc","origSz":"1"},{"coin":"SYN","side":"B","limitPx":"1","sz":"1","oid":2,"timestamp":5,"isTrigger":true,"triggerPx":"1","user":"0x01"}],[{"coin":"SYN","side":"A","limitPx":"10.1","sz":"2","oid":3,"timestamp":6}]]],["@9",[[],[]]]]}"#;

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.insert(
            InstrumentSpec::hyperliquid(
                MarketClass::Perp,
                "SYN",
                1,
                None,
                AdmissionId::new("ADM-0001").unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        r
    }

    #[test]
    fn parses_and_filters() {
        let p = parse_l4_snapshot(SNAP, &registry(), UnixMillis(1), UnixNanos(2)).unwrap();
        assert_eq!(p.height, 12);
        assert_eq!(p.dropped_triggers, 1);
        assert_eq!(p.snapshots.len(), 1);
        let s = &p.snapshots[0];
        assert_eq!(s.prov.seq, Seq::end_of_block(12));
        let SnapshotDepth::L4 { bids, asks } = &s.depth else {
            panic!()
        };
        assert_eq!(bids.len(), 1);
        assert_eq!(asks[0].price.to_string(), "10.10000");
        mesh_types::allowlist::check_record(&s.clone().into()).unwrap();

        let f = filter_l4_snapshot(SNAP).unwrap();
        for banned in ["user", "cloid", "tif", "isTrigger", "@9", "0x01"] {
            assert!(!f.contains(banned), "{banned} in {f}");
        }
        let again = parse_l4_snapshot(&f, &registry(), UnixMillis(1), UnixNanos(2)).unwrap();
        assert_eq!(again.snapshots, p.snapshots);
        assert_eq!(filter_l4_snapshot(&f).unwrap(), f);
        assert!(parse_l4_envelope(r#"{"height":1,"snapshot":[],"extra":1}"#).is_err());
    }
}
