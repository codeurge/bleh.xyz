//! Synthetic contract months: a fair-value series for each contract month
//! of an exposure, without exchange data (`synthetic-months@1`).
//!
//! The method is specified in `docs/specs/method-synthetic-months-v1.md`
//! and recorded in ADR 0013. Inputs are mesh records only:
//!
//! - **Spot**: trades, book snapshots, queue positions and liquidity grades
//!   of independent venue markets ([`spot`]).
//! - **Rates**: `reference_value` records (overnight rate, bills, par and
//!   zero yields) and overnight-index `swap_print` records ([`curve`]).
//! - **Calendars**: rulebook last-trading-day and roll rules and US
//!   business-day calendars, computed in code ([`calendar`], [`date`]).
//! - **Carry**: an observed series or a configured estimate, and
//!   commodity `swap_print` forward anchors ([`fair`]).
//!
//! Output records are `synthetic_month` ([`mesh_types::SyntheticMonth`]).
//!
//! | Module | Contents |
//! |---|---|
//! | [`detmath`] | Deterministic `exp` and `ln` |
//! | [`date`] | Dates, contract months, New York time |
//! | [`calendar`] | Holiday calendars, last-trading-day and roll rules, cycles |
//! | [`curve`] | Quote conversion, par bootstrap, interpolation |
//! | [`spot`] | Spot composite and grade inheritance |
//! | [`fair`] | Cost of carry, forward nodes, short-rate months |
//! | [`registry`] | Exposure registry (JSON) |
//! | [`engine`] | Input state and valuation |
//! | [`batch`] | JSON-lines input and grid runs |

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod batch;
pub mod calendar;
pub mod curve;
pub mod date;
pub mod detmath;
pub mod engine;
pub mod error;
pub mod fair;
pub mod registry;
pub mod spot;

pub use engine::{Engine, Valuation};
pub use error::Error;
pub use registry::Registry;

/// The method this crate implements.
pub const METHOD: &str = "synthetic-months@1";
