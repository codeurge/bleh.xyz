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
//! | Hourly files, one-pass reading, tail cursors | [`files`] | no |
//! | One-pass replay of a directory | [`replay`] | no |
//! | Resume points and the checkpoint file | [`checkpoint`] | no |

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod assumptions;
pub mod checkpoint;
pub mod files;
pub mod fills;
pub mod filter;
pub mod join;
pub mod normalize;
pub mod raw;
pub mod replay;
pub mod sequence;

pub use filter::{StreamDirs, filter_line, filter_raw_line};
pub use normalize::{ReaderBlock, normalize};
pub use raw::Stream;
pub use replay::Replay;
pub use sequence::{BlockOutput, Sequencer};
