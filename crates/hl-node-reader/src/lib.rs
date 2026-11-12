//! Hyperliquid node output reader for the bleh.xyz mesh.
//!
//! Reads the per-block files a non-validating Hyperliquid node writes with
//! `--write-fills --write-order-statuses --write-raw-book-diffs
//! --batch-by-block` and turns them into `mesh-types` records, exactly as
//! `docs/specs/hyperliquid-node-format.md` specifies, for every market in
//! the node's metadata that ADM-0001 admits. Clean-room: written from the
//! documented schemas only.
//!
//! # Layers
//!
//! | Layer | Module | Pure |
//! |---|---|---|
//! | Every **to verify** assumption, one name each | [`assumptions`] | yes |
//! | Typed node lines, strict on unknown fields | [`raw`] | yes |
//! | Fill pairing rule | [`fills`] | yes |
//! | Filtered raw (public reproducibility root) | [`filter`] | yes |
//! | Records from one block | [`mod@normalize`] | yes |
//! | Joining the three streams per block | [`join`] | yes |
//! | `prev_block_number` links and gap flags | [`sequence`] | yes |
//! | L4 snapshots | [`snapshot`] | parse: yes |
//! | Market registration from metadata | [`markets`] | specs: yes |
//! | Hourly files, one-pass reading, tail cursors | [`files`] | no |
//! | One-pass replay of a directory | [`replay`] | no |
//! | Live tailing (notify + polling) | [`tail`] | no |
//! | Active-active merge and integrity alarms | [`merge`] | state machine: yes |
//! | Tailers, merge, sequencer, checkpoint | [`pipeline`] | no |
//! | Info server client | [`http`] | no |
//! | Prometheus metrics | [`metrics`] | no |
//!
//! # What other components call
//!
//! - Archive: [`filter::filter_line`] / [`filter::filter_raw_line`] on every
//!   raw line and [`snapshot::filter_l4_snapshot`] on snapshot files to
//!   write filtered raw; [`replay::Replay`] (with
//!   [`replay::Replay::sequenced`]) to rebuild records from filtered raw.
//! - Book builder: [`snapshot::request_l4_snapshot`] and
//!   [`snapshot::parse_l4_snapshot`] for checkpoints; the `book_event`
//!   batches of [`sequence::BlockOutput`].
//! - Gateway and bus: [`pipeline::Pipeline::spawn`], whose
//!   [`pipeline::PipelineEvent::Block`] carries one
//!   [`mesh_types::BlockBatch`] per instrument, kind and block.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod assumptions;
pub mod checkpoint;
pub mod files;
pub mod fills;
pub mod filter;
pub mod http;
pub mod join;
pub mod markets;
pub mod merge;
pub mod metrics;
pub mod normalize;
pub mod pipeline;
pub mod raw;
pub mod replay;
pub mod sequence;
pub mod snapshot;
pub mod tail;

pub use filter::{StreamDirs, filter_line, filter_raw_line};
pub use normalize::{ReaderBlock, normalize};
pub use raw::Stream;
pub use replay::Replay;
pub use sequence::{BlockOutput, Sequencer};
