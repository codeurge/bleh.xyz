//! BLS consumer price indexes and average prices (ADM-0022), read from
//! the bulk time-series files under `download.bls.gov/pub/time.series/`
//! (the Public Data API is for checks only and is not read).
//!
//! | Survey | Directory | Series ID | Records (`bls:ref:<series_id>`) |
//! |---|---|---|---|
//! | CPI-U | `cu` | `CU` seasonal (`S`, `U`), periodicity, area (4), item | `index_level`, not and seasonally adjusted, every item and area |
//! | CPI-W | `cw` | as CPI-U | `index_level` |
//! | C-CPI-U | `su` | as CPI-U (`U` only) | `index_level`; initial and interim values revised to final |
//! | Average prices | `ap` | `AP`, seasonal (`U`), area (4), item | `price` in US dollars per the unit in the item name |
//!
//! Only these four surveys are admitted: producer and import/export price
//! indexes (and any other BLS survey) are not, and a series of another
//! survey in a file is dropped as `excluded`. Values are monthly (`M01` to
//! `M12`) and dated the first of the month; annual averages (`M13`) and
//! semiannual periods (`S01` to `S03`) are skipped as `other_frequency`.
//! A value BLS did not publish (a dash, with footnote `X` for the 2025
//! lapse in appropriations) is a gap: skipped as `missing`, never filled.
//! Footnote codes have no place in the record and are not emitted.
//!
//! Data files are text with a header row, tab-separated and padded with
//! spaces (the layout files `cu.txt`, `cw.txt`, `su.txt` and `ap.txt`
//! describe the columns `series_id`, `year`, `period`, `value`,
//! `footnote_codes`); columns are found by header name. The
//! `.data.0.Current` file holds every current series from the late 1990s;
//! earlier years are in the partition files.
//!
//! Revisions: seasonally adjusted series are revised for five years each
//! February, and C-CPI-U values are revised from initial to final over
//! 10 to 12 months. Live polls keep five years and two months of revision
//! state, so each changed value becomes the next revision.
//!
//! The area selection (`--bls-areas`, default `0000`, the U.S. city
//! average; `all` for every area) narrows what is emitted; every area is
//! admitted.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::time::Duration;

use mesh_types::ReferenceMeasure;

use super::{Bases, ParseError, Parsed, Reader, Schedule, Target, WEEKDAYS};
use crate::num;
use crate::obs::{Obs, RefObs};
use crate::source::SourceId;
use crate::time::Date;

/// Default base of the time-series files.
pub const BASE: &str = "https://download.bls.gov/pub/time.series";

/// Default area selection: the U.S. city average.
pub const DEFAULT_AREAS: &str = "0000";

/// An admitted survey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Survey {
    /// CPI for All Urban Consumers.
    Cu,
    /// CPI for Urban Wage Earners and Clerical Workers.
    Cw,
    /// Chained CPI for All Urban Consumers.
    Su,
    /// Average prices.
    Ap,
}

/// What a partition file covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cover {
    /// The U.S. city average only.
    Us,
    /// Areas other than the U.S. city average only.
    Areas,
    /// Both.
    Mixed,
}

const CPI_PARTITIONS: &[(&str, Cover)] = &[
    ("data.1.AllItems", Cover::Mixed),
    ("data.2.Summaries", Cover::Mixed),
    ("data.3.AsizeNorthEast", Cover::Areas),
    ("data.4.AsizeNorthCentral", Cover::Areas),
    ("data.5.AsizeSouth", Cover::Areas),
    ("data.6.AsizeWest", Cover::Areas),
    ("data.7.OtherNorthEast", Cover::Areas),
    ("data.8.OtherNorthCentral", Cover::Areas),
    ("data.9.OtherSouth", Cover::Areas),
    ("data.10.OtherWest", Cover::Areas),
    ("data.11.USFoodBeverage", Cover::Us),
    ("data.12.USHousing", Cover::Us),
    ("data.13.USApparel", Cover::Us),
    ("data.14.USTransportation", Cover::Us),
    ("data.15.USMedical", Cover::Us),
    ("data.16.USRecreation", Cover::Us),
    ("data.17.USEducationAndCommunication", Cover::Us),
    ("data.18.USOtherGoodsAndServices", Cover::Us),
    ("data.19.PopulationSize", Cover::Areas),
    ("data.20.USCommoditiesServicesSpecial", Cover::Us),
];

const AP_PARTITIONS: &[(&str, Cover)] = &[
    ("data.1.HouseholdFuels", Cover::Mixed),
    ("data.2.Gasoline", Cover::Mixed),
    ("data.3.Food", Cover::Mixed),
];

/// The file holding current values.
pub const CURRENT: &str = "data.0.Current";

impl Survey {
    /// Every admitted survey.
    pub const ALL: [Survey; 4] = [Survey::Cu, Survey::Cw, Survey::Su, Survey::Ap];

    /// Directory and file prefix (`cu`).
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Survey::Cu => "cu",
            Survey::Cw => "cw",
            Survey::Su => "su",
            Survey::Ap => "ap",
        }
    }

    /// Series ID prefix (`CU`).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Survey::Cu => "CU",
            Survey::Cw => "CW",
            Survey::Su => "SU",
            Survey::Ap => "AP",
        }
    }

    fn from_dir(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.dir() == s)
    }

    /// What the values measure.
    #[must_use]
    pub const fn measure(self) -> ReferenceMeasure {
        match self {
            Survey::Ap => ReferenceMeasure::Price,
            _ => ReferenceMeasure::IndexLevel,
        }
    }

    /// First year of `.data.0.Current` (seen 2026-11-14): earlier years
    /// are read from the partition files. C-CPI-U starts in December 1999
    /// and its current file holds everything.
    #[must_use]
    pub const fn current_from(self) -> i32 {
        match self {
            Survey::Cu | Survey::Cw => 1997,
            Survey::Ap => 1995,
            Survey::Su => 1999,
        }
    }

    fn partitions(self) -> &'static [(&'static str, Cover)] {
        match self {
            Survey::Cu | Survey::Cw => CPI_PARTITIONS,
            Survey::Ap => AP_PARTITIONS,
            Survey::Su => &[],
        }
    }
}

/// The areas emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Areas {
    /// Every area.
    All,
    /// These area codes.
    Only(BTreeSet<String>),
}

impl Areas {
    /// Parses a comma list of area codes, or `all` (or `*`). An empty list
    /// means the default, the U.S. city average.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let codes: BTreeSet<String> = s
            .split(',')
            .map(|c| c.trim().to_ascii_uppercase())
            .filter(|c| !c.is_empty())
            .collect();
        if codes.iter().any(|c| c == "ALL" || c == "*") {
            Areas::All
        } else if codes.is_empty() {
            Areas::Only(std::iter::once(DEFAULT_AREAS.to_owned()).collect())
        } else {
            Areas::Only(codes)
        }
    }

    fn contains(&self, area: &str) -> bool {
        match self {
            Areas::All => true,
            Areas::Only(s) => s.contains(area),
        }
    }

    fn wants(&self, cover: Cover) -> bool {
        match (self, cover) {
            (_, Cover::Mixed) | (Areas::All, _) => true,
            (Areas::Only(s), Cover::Us) => s.contains(DEFAULT_AREAS),
            (Areas::Only(s), Cover::Areas) => s.iter().any(|a| a != DEFAULT_AREAS),
        }
    }
}

/// The area code of a series ID of `survey`, if the ID is well formed:
/// `CU` + seasonal (`S`/`U`) + periodicity (`R`/`S`) + area (4) + item
/// (1 to 8) for the indexes, `AP` + `U` + area (4) + item (1 to 8) for
/// average prices.
#[must_use]
pub fn series_area(survey: Survey, id: &str) -> Option<&str> {
    let rest = id.strip_prefix(survey.prefix())?;
    if !rest.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let b = rest.as_bytes();
    let rest = match survey {
        Survey::Ap => rest.strip_prefix('U')?,
        Survey::Cu | Survey::Cw | Survey::Su => {
            let seasonal = match survey {
                Survey::Su => matches!(b.first(), Some(b'U')),
                _ => matches!(b.first(), Some(b'S' | b'U')),
            };
            if !seasonal || !matches!(b.get(1), Some(b'R' | b'S')) {
                return None;
            }
            rest.get(2..)?
        }
    };
    let (area, item) = (rest.get(..4)?, rest.get(4..)?);
    (!item.is_empty() && item.len() <= 8).then_some(area)
}

/// What a data-file parse keeps.
#[derive(Debug, Clone)]
pub struct Scope {
    /// First year kept.
    pub from_year: i32,
    /// Last year kept.
    pub to_year: i32,
    /// Areas kept.
    pub areas: Areas,
}

impl Scope {
    /// Every year and area.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            from_year: i32::MIN,
            to_year: i32::MAX,
            areas: Areas::All,
        }
    }
}

const REQUIRED: [&str; 4] = ["series_id", "year", "period", "value"];

/// Parses one data file of `survey`.
///
/// # Errors
///
/// Fails if the body has no header row or the header lacks a required
/// column.
pub fn parse_data(survey: Survey, body: &[u8], scope: &Scope) -> Result<Parsed, ParseError> {
    let text = String::from_utf8_lossy(body);
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    let header = lines
        .by_ref()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| ParseError(format!("{}: empty data file", survey.dir())))?;
    let mut out = Parsed::default();
    let index: HashMap<String, usize> = split(header)
        .enumerate()
        .map(|(i, h)| (h.to_ascii_lowercase(), i))
        .collect();
    for h in index.keys() {
        if !REQUIRED.contains(&h.as_str()) && h != "footnote_codes" {
            out.unknown.insert(format!("column:{h}"));
        }
    }
    let col = |name: &str| {
        index
            .get(name)
            .copied()
            .ok_or_else(|| ParseError(format!("{}: header lacks {name}", survey.dir())))
    };
    let (c_id, c_year, c_period, c_value) = (
        col("series_id")?,
        col("year")?,
        col("period")?,
        col("value")?,
    );
    let measure = survey.measure();
    let mut cells: Vec<&str> = Vec::with_capacity(5);
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        cells.clear();
        cells.extend(split(line));
        let cell = |i: usize| cells.get(i).copied().unwrap_or("");
        let Ok(year) = cell(c_year).parse::<i32>() else {
            out.skip("malformed");
            continue;
        };
        if year < scope.from_year || year > scope.to_year {
            continue;
        }
        let id = cell(c_id);
        if !id.starts_with(survey.prefix()) {
            out.skip("excluded");
            continue;
        }
        let Some(area) = series_area(survey, id) else {
            out.skip("malformed");
            continue;
        };
        if !scope.areas.contains(area) {
            out.skip("out_of_scope");
            continue;
        }
        let period = cell(c_period);
        let month = match period.strip_prefix('M').and_then(|m| m.parse::<u32>().ok()) {
            Some(m @ 1..=12) => m,
            Some(13) => {
                out.skip("other_frequency");
                continue;
            }
            _ if period.starts_with('S') => {
                out.skip("other_frequency");
                continue;
            }
            _ => {
                out.skip("malformed");
                continue;
            }
        };
        let Some(as_of) = Date::new(year, month, 1) else {
            out.skip("malformed");
            continue;
        };
        match num::decimal(cell(c_value)) {
            Ok(Some(v)) => {
                let mut r = RefObs::usd_price(id.to_owned(), v, as_of);
                r.measure = measure;
                if measure != ReferenceMeasure::Price {
                    r.currency = None;
                }
                out.obs.push(Obs::Ref(r));
            }
            Ok(None) => out.skip("missing"),
            Err(_) => out.skip("malformed"),
        }
    }
    Ok(out)
}

/// Cells of a line: tab-separated as published (padded with spaces), or
/// separated by runs of spaces as the layout files describe.
fn split(line: &str) -> Box<dyn Iterator<Item = &str> + '_> {
    if line.contains('\t') {
        Box::new(line.split('\t').map(str::trim))
    } else {
        Box::new(line.split_whitespace())
    }
}

/// The BLS reader.
#[derive(Debug)]
pub struct BlsReader {
    base: String,
    areas: Areas,
}

impl BlsReader {
    /// A reader using `bases` (keys `bls` and `bls-areas`).
    #[must_use]
    pub fn new(bases: &Bases) -> Self {
        Self {
            base: bases.get("bls", BASE),
            areas: Areas::parse(&bases.get("bls-areas", DEFAULT_AREAS)),
        }
    }

    fn target(&self, survey: Survey, file: &str, tag: &str, from: i32, to: i32) -> Target {
        let dir = survey.dir();
        Target::poll(
            format!("bls:{dir}:{file}:{tag}"),
            format!("{}/{dir}/{dir}.{file}", self.base),
            format!("data:{dir}:{from}:{to}"),
        )
    }
}

impl Reader for BlsReader {
    fn id(&self) -> SourceId {
        SourceId::Bls
    }

    fn live_targets(&self, today: Date) -> Vec<Target> {
        // Live polls keep 1,900 days; years before that are skipped while
        // parsing.
        Survey::ALL
            .into_iter()
            .map(|s| self.target(s, CURRENT, "live", today.y - 6, 9999))
            .collect()
    }

    fn backfill_targets(&self, year: i32) -> Vec<Target> {
        let mut out = Vec::new();
        for s in Survey::ALL {
            if year >= s.current_from() {
                out.push(self.target(s, CURRENT, "history", year, year));
            } else {
                for (file, cover) in s.partitions() {
                    if self.areas.wants(*cover) {
                        out.push(self.target(s, file, "history", year, year));
                    }
                }
            }
        }
        out
    }

    fn parse(&self, target: &Target, body: &[u8]) -> Result<Parsed, ParseError> {
        let bad = || ParseError(format!("bad target kind {}", target.kind));
        let mut parts = target.kind.split(':');
        if parts.next() != Some("data") {
            return Err(bad());
        }
        let survey = parts.next().and_then(Survey::from_dir).ok_or_else(bad)?;
        let from_year = parts.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
        let to_year = parts.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
        parse_data(
            survey,
            body,
            &Scope {
                from_year,
                to_year,
                areas: self.areas.clone(),
            },
        )
    }

    fn schedule(&self) -> Schedule {
        // CPI is released at 8:30 a.m. ET on a weekday around the 10th to
        // 15th of the month (12:30 or 13:30 UTC), and the files carry the
        // release time as Last-Modified.
        Schedule {
            days: WEEKDAYS,
            from_min: 12 * 60 + 15,
            to_min: 16 * 60,
            inside: Duration::from_secs(1800),
            outside: Duration::from_secs(12 * 3600),
        }
    }

    fn revision_days(&self) -> u32 {
        1900
    }

    fn live_window_days(&self) -> i64 {
        // Seasonally adjusted series are revised five years back each
        // February.
        1900
    }

    fn backfill_cache(&self) -> usize {
        // Years before the current file are spread over up to 23
        // partition files per survey; keep them all for the run.
        32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_ids_and_areas() {
        assert_eq!(series_area(Survey::Cu, "CUUR0000SA0"), Some("0000"));
        assert_eq!(series_area(Survey::Cu, "CUSR0000SA0L1E"), Some("0000"));
        assert_eq!(series_area(Survey::Cu, "CUURS49ASA0"), Some("S49A"));
        assert_eq!(series_area(Survey::Cw, "CWUS0000SA0"), Some("0000"));
        assert_eq!(series_area(Survey::Su, "SUUR0000SA0E"), Some("0000"));
        assert_eq!(series_area(Survey::Su, "SUSR0000SA0"), None);
        assert_eq!(series_area(Survey::Ap, "APU0000701111"), Some("0000"));
        assert_eq!(series_area(Survey::Ap, "APUS49A74714"), Some("S49A"));
        assert_eq!(series_area(Survey::Cu, "CUXR0000SA0"), None);
        assert_eq!(series_area(Survey::Cu, "CUUR0000"), None);
        assert_eq!(series_area(Survey::Cu, "CWUR0000SA0"), None);
        assert_eq!(series_area(Survey::Cu, "CUUR0000SA0-1"), None);
    }

    #[test]
    fn area_selection_picks_partitions() {
        let us = Areas::parse("");
        assert_eq!(us, Areas::parse("0000"));
        assert!(us.wants(Cover::Us) && us.wants(Cover::Mixed) && !us.wants(Cover::Areas));
        let all = Areas::parse("all");
        assert!(all.wants(Cover::Areas) && all.contains("S49A"));
        let west = Areas::parse("0400, s49a");
        assert!(!west.wants(Cover::Us) && west.wants(Cover::Areas));
        assert!(west.contains("S49A") && !west.contains("0000"));
    }
}
