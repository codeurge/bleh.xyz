//! Pollers for CFTC Part 43 and SEC Regulation SBSR public swap
//! dissemination, normalized to `mesh_types::SwapPrint` records.
//!
//! Implemented repository: DTCC Data Repository (U.S.) LLC, through its
//! public price dissemination site, for both regimes (ADM-0002, ADM-0004).
//! Repositories operated by an exchange group are excluded by both
//! admissions. Formats are documented in `docs/specs/swap-repositories.md`.
//!
//! | Module | Contents |
//! |---|---|
//! | [`repository`] | Regimes, repositories and DTCC report buckets |
//! | [`fields`] | Column names and normalization |
//! | [`values`] | Exact parsing of numbers, timestamps and dates |
//! | [`parse`] | Report files to `SwapPrint` |
//! | [`listing`] | DTCC dashboard listings |

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod fields;
pub mod listing;
pub mod parse;
pub mod repository;
pub mod values;
