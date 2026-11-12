//! Shared data model for the bleh.xyz market-data mesh (observation v0).
//!
//! Every crate in the workspace speaks these types. They are owned by the
//! integrator: build lanes use them but do not edit them (see
//! `docs/decisions/0007-code-layout.md`). The model is specified in
//! `docs/specs/observation-v0.md`, with a JSON Schema alongside.
//!
//! # Design rules
//!
//! - **No floats in canonical data.** Prices and sizes are fixed-point
//!   ([`Price`], [`Size`]): an `i64` mantissa and a per-instrument scale.
//! - **Provenance on every record.** Each record carries a [`Provenance`]:
//!   instrument, venue, source/ingest/publish timestamps, [`Seq`], admission
//!   ID and [`Flags`].
//! - **Excluded fields do not exist.** There are no fields for oracle, mark,
//!   external or funding prices, liquidation details, owner addresses or
//!   miscellaneous venue events. Deserialization rejects unknown fields, and
//!   [`allowlist`] checks every serialized record against an explicit list
//!   of permitted field paths.
//! - **Deterministic encodings.** [`hash`] defines a canonical JSON encoding
//!   and a content hash over a sequence of records that does not depend on
//!   Parquet bytes. [`merkle`] and [`signing`] commit to the same canonical
//!   encodings per block and stream, signed with secp256k1 so Ethereum
//!   contracts can verify them.
//!
//! # Modules
//!
//! | Module | Contents |
//! |---|---|
//! | [`decimal`] | [`Price`], [`Size`], exact decimal parsing and formatting |
//! | [`time`] | [`UnixMillis`], [`UnixNanos`], RFC 3339 parsing and formatting |
//! | [`seq`] | [`Seq`]: `(block_number, event_index)`, totally ordered |
//! | [`ids`] | [`InstrumentId`], [`VenueId`], [`AdmissionId`], [`MethodId`], order and trade IDs |
//! | [`flags`] | [`Flags`] bitset |
//! | [`provenance`] | [`Provenance`] |
//! | [`record`] | Record kinds and the [`Record`] enum |
//! | [`derived`] | Derived-record header and analytics kinds (footprint, volume profile) |
//! | [`swap`] | Part 43 / SBSR swap prints |
//! | [`batch`] | [`BlockBatch`], the unit published on the bus |
//! | [`registry`] | Instrument specifications built from venue metadata |
//! | [`allowlist`] | Runtime field allowlist |
//! | [`hash`] | Canonical encoding and content hash |
//! | [`merkle`] | Per-block Merkle roots and inclusion proofs |
//! | [`signing`] | secp256k1 (Ethereum-style) keys, signatures, block commitments |
//! | [`manifest`] | Daily archive manifest and anchors |
//! | [`arrow`] | Arrow schemas for each record kind |

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod allowlist;
pub mod arrow;
pub mod batch;
pub mod decimal;
pub mod derived;
pub mod flags;
pub mod hash;
mod hex;
pub mod ids;
pub mod manifest;
pub mod merkle;
pub mod provenance;
pub mod record;
pub mod registry;
pub mod seq;
pub mod signing;
pub mod swap;
pub mod time;

pub use batch::{BatchError, BlockBatch};
pub use decimal::{DecimalError, MAX_SCALE, Price, Size};
pub use derived::{DerivedHeader, Footprint, FootprintLevel, ProfileLevel, VolumeProfile};
pub use flags::{Flag, Flags};
pub use hash::{ContentHash, ContentHasher, Sha256Digest, canonical_json, content_hash};
pub use ids::{AdmissionId, IdError, InstrumentId, MethodId, OrderId, TradeId, VenueId};
pub use merkle::{H256, MerkleProof, merkle_root};
pub use provenance::Provenance;
pub use record::{
    AggressorSide, Bar, BarInterval, BookAction, BookEvent, BookSide, BookSnapshot, Level, Record,
    RecordKind, RestingOrder, SnapshotDepth, Trade, ValidationError,
};
pub use registry::{InstrumentSpec, MarketClass, Registry};
pub use seq::Seq;
pub use signing::{BlockCommitment, EthAddress, EthSignature, SigningKey, StreamId};
pub use swap::{AssetClass, Notional, SwapAction, SwapPrice, SwapPrint};
pub use time::{UnixMillis, UnixNanos};

/// Version tag of the observation data model implemented by this crate.
pub const OBSERVATION_SCHEMA: &str = "observation-v0";
