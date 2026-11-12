//! One reader per admitted source, written clean-room from
//! `docs/specs/reference-sources.md`.
//!
//! A reader knows its source's URLs ([`Target`]s for live polling and for
//! each backfill year), parses each response into [`Obs`], and reads only
//! the admitted channels and fields: every parser works from an explicit
//! allowlist (series stems, purchase types, column names) and counts what
//! it drops, so an excluded field never reaches a record.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::obs::Obs;
use crate::source::SourceId;
use crate::time::Date;

pub mod cot;
pub mod dairy;
pub mod ercot;
pub mod feeder;
pub mod frb;
pub mod lmr;
pub mod nyfed;
pub mod treasury;

/// One URL to fetch and how to read it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Target {
    /// Stable key: names the raw file, holds validators and dedupe state.
    pub key: String,
    /// URL.
    pub url: String,
    /// Which parser of the reader reads the body.
    pub kind: String,
    /// Fetch only once (immutable documents such as grid-operator report
    /// files); remembered after success and never fetched again.
    pub once: bool,
}

impl Target {
    /// A target polled repeatedly (conditional requests).
    #[must_use]
    pub fn poll(key: impl Into<String>, url: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            url: url.into(),
            kind: kind.into(),
            once: false,
        }
    }
}

/// What a parser found in one body.
#[derive(Debug, Default)]
pub struct Parsed {
    /// Observations.
    pub obs: Vec<Obs>,
    /// Further targets to fetch now (pages, listed documents).
    pub follow: Vec<Target>,
    /// Rows not emitted, by reason (`excluded`, `missing`, `malformed`,
    /// `out_of_scope`).
    pub skipped: BTreeMap<&'static str, u64>,
    /// Undocumented fields, columns or series seen (alerted, not emitted).
    pub unknown: BTreeSet<String>,
    /// The report is older than the freshness limit.
    pub stale: bool,
}

impl Parsed {
    /// Counts one skipped row.
    pub fn skip(&mut self, reason: &'static str) {
        *self.skipped.entry(reason).or_default() += 1;
    }
}

/// A body that cannot be read at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

impl ParseError {
    /// A parse error from anything printable.
    pub fn new(e: impl std::fmt::Display) -> Self {
        Self(e.to_string())
    }
}

impl From<crate::xml::XmlError> for ParseError {
    fn from(e: crate::xml::XmlError) -> Self {
        Self(e.to_string())
    }
}

/// When to poll: often inside the source's release window, rarely outside
/// it (the specification asks for polls after the documented release
/// time, not continuously).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    /// Weekdays of the window, bit 0 = Monday.
    pub days: u8,
    /// Window start, minutes after midnight UTC.
    pub from_min: u32,
    /// Window end, minutes after midnight UTC (may be below `from_min`,
    /// wrapping past midnight into the next day).
    pub to_min: u32,
    /// Interval inside the window.
    pub inside: Duration,
    /// Interval outside the window.
    pub outside: Duration,
}

/// Monday to Friday.
pub const WEEKDAYS: u8 = 0b0001_1111;
/// Every day.
pub const EVERY_DAY: u8 = 0b0111_1111;

impl Schedule {
    /// True if `ms` (Unix milliseconds) falls inside the window.
    #[must_use]
    pub fn in_window(&self, ms: i64) -> bool {
        let day = Date::of_ms(ms);
        let min = u32::try_from(ms.rem_euclid(86_400_000) / 60_000).unwrap_or(0);
        let on = |d: Date| self.days & (1 << d.weekday()) != 0;
        if self.from_min <= self.to_min {
            on(day) && min >= self.from_min && min < self.to_min
        } else {
            (on(day) && min >= self.from_min) || (on(day.add_days(-1)) && min < self.to_min)
        }
    }

    /// The interval after a poll at `ms`.
    #[must_use]
    pub fn interval_at(&self, ms: i64) -> Duration {
        if self.in_window(ms) {
            self.inside
        } else {
            self.outside
        }
    }

    /// Staleness limit for health: a source is stale after missing about
    /// three outside-window polls.
    #[must_use]
    pub fn max_age(&self) -> Duration {
        self.outside.saturating_mul(3) + Duration::from_secs(600)
    }
}

/// A source reader.
pub trait Reader: Send + Sync + std::fmt::Debug {
    /// The source.
    fn id(&self) -> SourceId;

    /// Targets to poll live on `today`, in order (a target whose parse
    /// another needs comes first).
    fn live_targets(&self, today: Date) -> Vec<Target>;

    /// Targets holding the history of `year`. Observations whose date is
    /// outside `year` are dropped by the backfill.
    fn backfill_targets(&self, year: i32) -> Vec<Target>;

    /// Parses one fetched body.
    ///
    /// # Errors
    ///
    /// Fails if the body cannot be read at all (a malformed row is skipped
    /// and counted instead).
    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError>;

    /// The live poll schedule.
    fn schedule(&self) -> Schedule;

    /// Days of revision state kept live: how far back the source revises.
    fn revision_days(&self) -> u32 {
        45
    }

    /// Live polls emit only values dated within this many days of today
    /// (whole-history files are left to the backfill).
    fn live_window_days(&self) -> i64 {
        40
    }

    /// Days after which the newest value of a poll counts as stale (for
    /// files that hold only the latest issue), if any.
    fn freshness_days(&self) -> Option<i64> {
        None
    }
}

/// Base URLs, overridable for tests and mirrors.
#[derive(Debug, Clone, Default)]
pub struct Bases {
    overrides: BTreeMap<String, String>,
}

impl Bases {
    /// Base URLs with `overrides` (`name=url`), where `name` is a source
    /// name or a host key a reader documents.
    ///
    /// # Errors
    ///
    /// Fails on an entry without `=`.
    pub fn parse(overrides: &[String]) -> Result<Self, String> {
        let mut map = BTreeMap::new();
        for o in overrides.iter().filter(|o| !o.trim().is_empty()) {
            let (k, v) = o
                .split_once('=')
                .ok_or_else(|| format!("base URL override {o:?} is not name=url"))?;
            map.insert(
                k.trim().to_owned(),
                v.trim().trim_end_matches('/').to_owned(),
            );
        }
        Ok(Self { overrides: map })
    }

    /// The base for `key`, or `default`.
    #[must_use]
    pub fn get(&self, key: &str, default: &str) -> String {
        self.overrides
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    }
}

/// Builds the reader for `id`.
#[must_use]
pub fn reader(id: SourceId, bases: &Bases) -> Box<dyn Reader> {
    match id {
        SourceId::Cot => Box::new(cot::CotReader::new(bases)),
        SourceId::TreasuryRates => Box::new(treasury::RatesReader::new(bases)),
        SourceId::TreasuryAuctions => Box::new(treasury::AuctionsReader::new(bases)),
        SourceId::NyFed => Box::new(nyfed::NyFedReader::new(bases)),
        SourceId::FrbH15 => Box::new(frb::FrbReader::h15(bases)),
        SourceId::FrbH10 => Box::new(frb::FrbReader::h10(bases)),
        SourceId::UsdaLmr => Box::new(lmr::LmrReader::new(bases)),
        SourceId::UsdaDairy => Box::new(dairy::DairyReader::new(bases)),
        SourceId::UsdaFeeder => Box::new(feeder::FeederReader::new(bases)),
        SourceId::Ercot => Box::new(ercot::ErcotReader::new(bases)),
    }
}

/// Header names normalized for matching: lowercase ASCII letters and
/// digits, every other run of characters as one `_`, trimmed. Sources
/// spell the same column as `Open Interest (All)`, `Open_Interest_All`
/// and `open_interest_all`; all three normalize alike.
#[must_use]
pub fn norm_header(s: &str) -> String {
    crate::num::slug(s).to_ascii_lowercase()
}

/// Reads a zip archive's members as `(name, bytes)`, in archive order.
///
/// # Errors
///
/// Fails on a malformed archive.
pub fn unzip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ParseError> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(ParseError::new)?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(ParseError::new)?;
        if f.is_dir() {
            continue;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(ParseError::new)?;
        let name = f.name().map_err(ParseError::new)?.into_owned();
        out.push((name, buf));
    }
    Ok(out)
}

/// The body itself, or the first member if it is a zip archive.
///
/// # Errors
///
/// Fails on a malformed or empty archive.
pub fn maybe_unzip(bytes: &[u8]) -> Result<Vec<u8>, ParseError> {
    if bytes.starts_with(b"PK\x03\x04") {
        unzip(bytes)?
            .into_iter()
            .next()
            .map(|(_, b)| b)
            .ok_or_else(|| ParseError("empty zip archive".into()))
    } else {
        Ok(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_windows() {
        // Weekdays 12:00 to 20:00 UTC.
        let s = Schedule {
            days: WEEKDAYS,
            from_min: 12 * 60,
            to_min: 20 * 60,
            inside: Duration::from_secs(600),
            outside: Duration::from_secs(7200),
        };
        let thu = Date::new(2026, 11, 12).unwrap().start_ms();
        assert!(s.in_window(thu + 13 * 3_600_000));
        assert!(!s.in_window(thu + 21 * 3_600_000));
        let sat = Date::new(2026, 11, 14).unwrap().start_ms();
        assert!(!s.in_window(sat + 13 * 3_600_000));
        // Wrapping window 21:00 to 03:00.
        let w = Schedule {
            from_min: 21 * 60,
            to_min: 3 * 60,
            ..s
        };
        assert!(w.in_window(thu + 22 * 3_600_000));
        assert!(w.in_window(thu + 86_400_000 + 3_600_000));
        assert!(!w.in_window(thu + 4 * 3_600_000 + 86_400_000));
        assert_eq!(
            norm_header(" Total Reportable Positions-Long (All)"),
            "total_reportable_positions_long_all"
        );
        assert_eq!(
            norm_header("Swap__Positions_Short_All"),
            "swap_positions_short_all"
        );
        assert!(Bases::parse(&["nyfed".into()]).is_err());
        let b = Bases::parse(&["nyfed=http://x/".into()]).unwrap();
        assert_eq!(b.get("nyfed", "https://y"), "http://x");
    }
}
