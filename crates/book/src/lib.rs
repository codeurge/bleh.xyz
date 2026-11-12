//! Order-level books, bars and order-flow analytics for the bleh.xyz mesh.
//!
//! Inputs are observation records from `mesh-types` (trades, book events,
//! book snapshots, or the [`BlockBatch`](mesh_types::BlockBatch)es that
//! carry them); outputs are derived records with a
//! [`DerivedHeader`](mesh_types::DerivedHeader) (`method@version`, inputs
//! hash, input count). Everything is fixed-point and deterministic: the
//! same input gives the same output, byte for byte in canonical form.
//!
//! | Module | Contents |
//! |---|---|
//! | [`book`] | [`OrderBook`]: L4 book, L2 levels, top of book, typed inconsistencies |
//! | [`mod@reconcile`] | Book versus snapshot: diff report and resync |
//! | [`checkpoint`] | Checkpoint policy, "book at time T" |
//! | [`bars`] | Time bars, rollups, tick, volume and range bars |
//! | [`orderflow`] | Footprint, delta and CVD, volume profile and value area, order activity, queue positions, sweeps, heatmap tiles |
//! | [`liquidity`] | Liquidity grade and tape/benchmark label |
//! | [`processor`] | [`Processor`]: streaming, block by block |
//! | [`batch`] | Archive replay through the same processor |
//! | [`inputs`] | Canonical encodings and input hashing |
//! | [`methods`] | Method IDs |
//! | [`fixed`] | Fixed-point helpers, [`SignedSize`](fixed::SignedSize) |
//!
//! Methods are specified in `docs/operations/book.md`. Owned by one build
//! lane; see `docs/decisions/0007-code-layout.md`.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod bars;
pub mod batch;
pub mod book;
pub mod checkpoint;
pub mod error;
pub mod fixed;
pub mod inputs;
pub mod liquidity;
pub mod methods;
pub mod orderflow;
pub mod processor;
pub mod reconcile;
#[cfg(test)]
mod testutil;

pub use book::{OrderBook, OrderView, TopOfBook};
pub use error::{BookError, InputError, ProcessError};
pub use processor::{BlockInput, BookState, Output, Processor, ProcessorConfig};
pub use reconcile::{ReconcileOptions, ReconcileReport, reconcile, resync};
