//! Raw archive, partition writer, replay and manifests for the bleh.xyz mesh.
//!
//! Owned by the archive lane; see `docs/decisions/0007-code-layout.md`.
//! The layout is specified in `docs/specs/archive-layout-v0.md` and the
//! manifests in `docs/specs/manifest-v0.md`; operations are documented in
//! `docs/operations/archive.md`.
//!
//! # Modules
//!
//! | Module | Contents |
//! |---|---|
//! | [`capture`] | Private and filtered raw tiers, hourly rotation, zstd, dictionaries |
//! | [`follow`] | Live capture: following node files, sealing, upload, the `.captured-through` watermark |
//! | [`store`] | S3-compatible and local object storage with write-once uploads |
//! | [`partition`] | Deterministic Parquet partitions, records to Arrow and back, Iceberg metadata |
//! | [`partition_dir`] | Daily record files (`date=YYYY-MM-DD/<file>`) to partitions, once per settled day |
//! | [`replay`] | Filtered raw to partitions through the injected parser |
//! | [`manifest`] | Building, signing, writing and indexing daily manifests |
//! | [`ipfs`], [`torrent`], [`digest`] | UnixFS CIDs, BitTorrent v2, one-pass file digests |
//! | [`anchor`] | OpenTimestamps and Ethereum L2 anchors |
//! | [`publish`] | Uploads, IPFS pinning (Kubo), Iceberg publishing |
//! | [`verify`] | Mirror verification: signature, chain, anchors, files |
//! | [`cli`] | The `archive` command line and its [`cli::Hooks`] |
//! | [`http`], [`metrics`], [`layout`] | HTTP abstraction (mockable), counters, paths |
//!
//! # Integration points
//!
//! The node reader is not a dependency. The integrator supplies:
//!
//! - a [`capture::LineFilter`] per stream (any
//!   `Fn(&[u8]) -> Option<Vec<u8>> + Send + Sync`) in a
//!   [`capture::FilterSet`], applied to every raw line for the filtered tier;
//! - a [`replay::RawParser`] (any
//!   `Fn(&[replay::RawFile]) -> Result<Vec<mesh_types::Record>, BoxError> + Send + Sync`)
//!   turning filtered raw files into records;
//!
//! and passes both to [`cli::run`] through [`cli::Hooks`], or uses
//! [`capture::Capture`] and [`replay::replay_day`] directly.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod anchor;
pub mod capture;
pub mod cli;
pub mod digest;
pub mod error;
pub mod follow;
pub mod hex;
pub mod http;
pub mod ipfs;
pub mod layout;
pub mod manifest;
pub mod metrics;
pub mod partition;
pub mod partition_dir;
pub mod publish;
pub mod replay;
pub mod store;
pub mod torrent;
pub mod verify;

#[cfg(test)]
mod testutil;

pub use error::{BoxError, Error, Result};
pub use layout::Day;
