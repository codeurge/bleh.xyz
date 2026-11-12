//! Pollers and history backfill for the admitted public reference sources
//! (open reference prices plan, task 4), emitting `reference_value` and
//! `positioning` records (observation v0 §§ 2.12, 2.14).
//!
//! | Source | Admission | Venue | Records |
//! |---|---|---|---|
//! | CFTC Commitments of Traders | ADM-0005 | `cftc-cot` | `positioning` |
//! | Treasury rates and auctions | ADM-0006 | `us-treasury` | rates and yields |
//! | New York Fed reference rates | ADM-0007 | `nyfed` | rates, SOFR averages and index |
//! | Federal Reserve Board H.15 | ADM-0008 | `frb-h15` | rates and yields |
//! | Federal Reserve Board H.10 | ADM-0009 | `frb-h10` | exchange rates |
//! | USDA Livestock Mandatory Reporting | ADM-0010 | `usda-lmr` | prices |
//! | USDA dairy | ADM-0011 | `usda-dairy` | prices (`exchange_referenced`) |
//! | USDA feeder cattle report files | ADM-0012 | `usda-mn` | prices |
//! | ERCOT website downloads | ADM-0013 | `ercot` | prices per interval |
//!
//! Each reader is written clean-room from `docs/specs/reference-sources.md`
//! and reads only the admitted channels and fields, through explicit
//! allowlists. Operations: `docs/operations/reference-pollers.md`.
//!
//! | Module | Contents |
//! |---|---|
//! | [`source`] | Source identities, admissions, notices, history start |
//! | [`sources`] | One reader per source, schedules, targets |
//! | [`obs`] | Parsed observations to records: revisions, sequence, flags |
//! | [`revision`] | Durable revision and dedupe state |
//! | [`http`] | Polite fetching: pacing, conditional requests, backoff |
//! | [`raw`] | Verbatim raw capture with sidecars |
//! | [`output`] | JSON lines, gateway channel, notices file |
//! | [`live`] | The live poll loop |
//! | [`backfill`] | History backfill through the archive partition writer |
//! | [`catalog`] | Series catalogue (registry entries with notices) |
//! | [`metrics`] | Prometheus metrics, `/metrics` and `/health` |
//! | [`config`] | Command line and entry points |
//! | [`time`], [`num`], [`xml`], [`xlsx`] | Dates and US local time, exact numbers, XML and workbook reading |

#![cfg_attr(test, allow(clippy::unwrap_used))]
#![warn(missing_docs)]

pub mod backfill;
pub mod catalog;
pub mod config;
pub mod http;
pub mod live;
pub mod metrics;
pub mod num;
pub mod obs;
pub mod output;
pub mod raw;
pub mod revision;
pub mod source;
pub mod sources;
pub mod time;
pub mod xlsx;
pub mod xml;

pub use source::SourceId;
