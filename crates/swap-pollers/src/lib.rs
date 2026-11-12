//! Pollers for CFTC Part 43 and SEC Regulation SBSR public swap
//! dissemination, normalized to `mesh_types::SwapPrint` records.
//!
//! Implemented repository: DTCC Data Repository (U.S.) LLC, through its
//! public price dissemination site, for both regimes (ADM-0002, ADM-0004).
//! Repositories operated by an exchange group are excluded by both
//! admissions. Formats are documented in `docs/specs/swap-repositories.md`
//! and operations in `docs/operations/swap-pollers.md`.
//!
//! | Module | Contents |
//! |---|---|
//! | [`repository`] | Regimes, repositories and DTCC report buckets |
//! | [`fields`] | Column names and normalization |
//! | [`values`] | Exact parsing of numbers, timestamps and dates |
//! | [`parse`] | Report files to `SwapPrint` |
//! | [`listing`] | DTCC dashboard listings |
//! | [`http`] | Polite fetching: pacing, conditional requests, backoff |
//! | [`raw`] | Verbatim raw capture and object-store mirror |
//! | [`checkpoint`] | Durable poller state |
//! | [`dedupe`] | Durable dedupe by dissemination ID |
//! | [`output`] | JSON lines, gateway channel, emission |
//! | [`poller`] | The DTCC poller |
//! | [`metrics`], [`server`] | Prometheus metrics and `/health` |
//! | [`config`] | Command line and environment |

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod checkpoint;
pub mod config;
pub mod dedupe;
pub mod fields;
pub mod http;
pub mod listing;
pub mod metrics;
pub mod output;
pub mod parse;
pub mod poller;
pub mod raw;
pub mod repository;
pub mod server;
pub mod values;
