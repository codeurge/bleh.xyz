//! In-process bus, block signer and side tap for the bleh.xyz mesh.
//!
//! The hot path is in-process (ADR 0006): a reader hands block batches and
//! derived records to a [`BlockPublisher`], which pushes them onto a
//! bounded broadcast [`ring`](ring::ring) that the gateway reads with its
//! own cursor. The publisher is also the **block signer stage**: per block
//! and per stream it builds the Merkle root over canonical record encodings
//! and signs a `BlockCommitment` with the active key of a [`KeyRing`],
//! keeping the trees in a [`ProofStore`] to answer inclusion-proof
//! requests. Every event is also copied to a [`SideTap`] (NATS JetStream in
//! production, [`MemoryTap`] in tests) for durability and replay after a
//! gateway restart; nothing archives from the tap.
//!
//! | Module | Contents |
//! |---|---|
//! | [`ring`] | [`RingSender`], [`RingReceiver`]: broadcast ring with per-subscriber cursors and lag detection |
//! | [`event`] | [`BusEvent`], [`BlockEnd`], [`ResyncReason`] |
//! | [`keys`] | [`KeyRing`], [`KeySpec`], key loading from files or environment variables |
//! | [`signer`] | [`BlockSigner`], [`StreamBlock`], [`ProofStore`], [`verify_commitment`] |
//! | [`publisher`] | [`BlockPublisher`] |
//! | [`tap`] | [`SideTap`], [`MemoryTap`], subjects |
//! | `jetstream` | `JetStreamTap` (feature `jetstream`, on by default) |
//!
//! Operations: `docs/operations/bus.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod event;
#[cfg(feature = "jetstream")]
pub mod jetstream;
pub mod keys;
pub mod publisher;
pub mod ring;
pub mod signer;
pub mod tap;

pub use event::{BlockEnd, BusEvent, ResyncReason};
pub use keys::{KeyError, KeyInfo, KeyRing, KeySource, KeySpec};
pub use publisher::{BlockPublisher, PublishError, PublisherStats};
pub use ring::{RecvError, RingReceiver, RingSender, RingStats, ring};
pub use signer::{
    BlockEntry, BlockSigner, ProofError, ProofResponse, ProofStore, RecordRef, SignerError,
    SortedLeaves, StreamBlock, verify_commitment,
};
pub use tap::{MemoryTap, SideTap, TapError, TapStats};

/// Default ring capacity: about a minute of Hyperliquid blocks with every
/// market active.
pub const DEFAULT_RING_CAPACITY: usize = 1 << 16;
