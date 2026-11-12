//! Order-level books, bars and order-flow analytics for the bleh.xyz mesh.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod book;
pub mod checkpoint;
pub mod error;
pub mod fixed;
pub mod inputs;
pub mod methods;
pub mod reconcile;
#[cfg(test)]
mod testutil;

pub use book::{OrderBook, OrderView, TopOfBook};
pub use error::{BookError, InputError, ProcessError};
pub use reconcile::{ReconcileOptions, ReconcileReport, reconcile, resync};
