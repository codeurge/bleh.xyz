//! The bleh.xyz origin service.
//!
//! One deployable process that turns Hyperliquid node files into the
//! public, signed live stream and the archive's raw tiers (ADR 0006,
//! ADR 0007):
//!
//! | Module | Contents |
//! |---|---|
//! | [`service`] | Composition: reader pipeline, [`derive::Deriver`], block signer and publisher, embedded gateway, JetStream side tap, live capture |
//! | [`derive`] | Book processors per instrument, block-positioned derived records, snapshot roll-forward |
//! | [`coverage`] | The venue coverage signal |
//! | [`hooks`] | The archive's filters and parser, built from the node reader and the deriver |
//! | [`metrics`] | Origin metrics |
//! | [`config`] | Command line and environment |
//!
//! Binaries: `origin` (the service) and `archive` (the archive command
//! line with [`hooks::env_hooks`] wired in). Operations:
//! `docs/operations/origin.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod config;
pub mod coverage;
pub mod derive;
pub mod hooks;
pub mod metrics;
pub mod service;

pub use derive::{BookStart, DeriveConfig, DerivedBlock, Deriver, Notice};
pub use service::{CaptureSettings, OriginConfig, ReaderSettings, RunningOrigin, start};
