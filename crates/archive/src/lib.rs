//! Raw archive, partition writer, replay and manifests for the bleh.xyz mesh.
//!
//! Owned by the archive lane; see `docs/decisions/0007-code-layout.md`.
//! The layout is specified in `docs/specs/archive-layout-v0.md` and the
//! manifests in `docs/specs/manifest-v0.md`; operations are documented in
//! `docs/operations/archive.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod anchor;
pub mod capture;
pub mod digest;
pub mod error;
pub mod hex;
pub mod http;
pub mod ipfs;
pub mod layout;
pub mod manifest;
pub mod metrics;
pub mod partition;
pub mod store;
pub mod torrent;

#[cfg(test)]
#[allow(dead_code)]
mod testutil;

pub use error::{BoxError, Error, Result};
pub use layout::Day;
