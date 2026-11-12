//! Provenance carried by every record.
//!
//! | Field | Meaning |
//! |---|---|
//! | `instrument` | Mesh instrument ID |
//! | `venue` | Venue or public-data source |
//! | `ts_source_ms` | Venue time in milliseconds (block time for Hyperliquid) |
//! | `ts_ingest_ns` | When the mesh observed the data, in nanoseconds |
//! | `ts_publish_ns` | When the mesh published the record (live only) |
//! | `seq` | `(block_number, event_index)` |
//! | `admission` | Admission decision covering the record's fields |
//! | `flags` | [`Flags`] |
//!
//! # Determinism
//!
//! Archived records must be a pure function of the raw files. So
//! `ts_ingest_ns` comes from the raw input where the source provides it
//! (Hyperliquid's per-block `local_time`, the node's own receive time), and
//! `ts_publish_ns` is set only on the live path. It is omitted from archived
//! records and excluded from the canonical encoding (see [`crate::hash`]).

use serde::{Deserialize, Serialize};

use crate::flags::Flags;
use crate::ids::{AdmissionId, InstrumentId, VenueId};
use crate::seq::Seq;
use crate::time::{UnixMillis, UnixNanos};

/// Lineage and timing for one record.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Mesh instrument ID.
    pub instrument: InstrumentId,
    /// Venue or public-data source.
    pub venue: VenueId,
    /// Venue (source) time, milliseconds.
    pub ts_source_ms: UnixMillis,
    /// Mesh observation time, nanoseconds.
    pub ts_ingest_ns: UnixNanos,
    /// Mesh publish time, nanoseconds. Live path only; never archived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts_publish_ns: Option<UnixNanos>,
    /// Position in the source.
    pub seq: Seq,
    /// Admission decision covering this record's fields.
    pub admission: AdmissionId,
    /// Record flags.
    #[serde(default)]
    pub flags: Flags,
}

impl Provenance {
    /// Builds provenance with no flags and no publish time.
    #[must_use]
    pub fn new(
        instrument: InstrumentId,
        venue: VenueId,
        seq: Seq,
        ts_source_ms: UnixMillis,
        ts_ingest_ns: UnixNanos,
        admission: AdmissionId,
    ) -> Self {
        Self {
            instrument,
            venue,
            ts_source_ms,
            ts_ingest_ns,
            ts_publish_ns: None,
            seq,
            admission,
            flags: Flags::NONE,
        }
    }

    /// Returns the provenance with `flags` set.
    #[must_use]
    pub fn with_flags(mut self, flags: Flags) -> Self {
        self.flags = flags;
        self
    }

    /// Sets the publish time. Called once, by the publisher, on the live
    /// path.
    pub fn stamp_publish(&mut self, at: UnixNanos) {
        self.ts_publish_ns = Some(at);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample(instrument: &str, block: u64, index: u32) -> Provenance {
        Provenance::new(
            InstrumentId::new(instrument).unwrap(),
            VenueId::new("hyperliquid").unwrap(),
            Seq::new(block, index),
            UnixMillis(1_767_225_600_000),
            UnixNanos(1_767_225_600_004_000_000),
            AdmissionId::new("ADM-0001").unwrap(),
        )
    }

    #[test]
    fn publish_time_is_omitted_until_stamped() {
        let mut p = sample("hyperliquid:perp:BTC", 1, 0);
        let json = serde_json::to_value(&p).unwrap();
        assert!(json.get("ts_publish_ns").is_none());
        p.stamp_publish(UnixNanos(5));
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["ts_publish_ns"], "5");
        let back: Provenance = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn rejects_unknown_fields() {
        let mut json = serde_json::to_value(sample("hyperliquid:perp:BTC", 1, 0)).unwrap();
        json["markPx"] = serde_json::json!("1.0");
        assert!(serde_json::from_value::<Provenance>(json).is_err());
    }
}
