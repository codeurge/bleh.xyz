//! Backfill from Hyperliquid's public archive buckets (ADM-0003).
//!
//! Three stages, each usable on its own:
//!
//! 1. **Copy** ([`copy`]): list the admitted prefixes of the requester-pays
//!    buckets ([`listing`]) and copy them verbatim into the private raw
//!    tier, resumable through a [`checkpoint`]. The ADM-0003 denylist
//!    ([`buckets::DENIED_PREFIXES`]) is enforced before any request.
//!    [`estimate`] prices a copy and reports coverage from the listing.
//! 2. **Process** ([`process`]): decompress raw objects, apply a field
//!    filter ([`filter::RawLineFilter`]), write filtered raw, parse to
//!    records ([`parse::RecordParser`]) flagged `backfill` under ADM-0003,
//!    and hand them to a [`partition::PartitionWriter`]. The traits are
//!    implemented locally for tests; the integrator wires the node reader's
//!    filter and parser and the archive's partition writer. [`compare`]
//!    checks backfilled hours against live-recorded overlap days.
//! 3. **Replay** ([`replay`]): a prototype emulated matching engine that
//!    turns simplified `replica_cmds` actions into book events and trades,
//!    checkpointed against archived L2 snapshots.
//!
//! Layouts and formats: `docs/specs/hyperliquid-archive-buckets.md`.
//! Operations: `docs/operations/backfill.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod buckets;
pub mod checkpoint;
pub mod codec;
pub mod copy;
pub mod estimate;
pub mod listing;
pub mod retry;
pub mod store;

#[cfg(test)]
pub(crate) mod testutil;

/// The admission covering every record this crate produces.
pub const ADMISSION: &str = "ADM-0003";
