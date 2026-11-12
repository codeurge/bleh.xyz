//! Historical query API for the bleh.xyz mesh.

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod book;
pub mod config;
pub mod engine;
pub mod error;
pub mod http;
pub mod metrics;
pub mod params;
pub mod rows;
pub mod service;
pub mod source;
pub mod store;
pub mod synth;
