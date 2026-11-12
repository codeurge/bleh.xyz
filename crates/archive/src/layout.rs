//! Archive layout: archive names, days and the paths of every file
//! (`docs/specs/archive-layout-v0.md`, `docs/specs/manifest-v0.md`).
//!
//! All paths here are relative to an archive root and `/`-separated, the
//! form manifests list them in. [`Layout`] joins them onto a local
//! directory.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use mesh_types::{InstrumentId, RecordKind, VenueId};

use crate::error::{Error, Result};

/// Default name of the filtered raw archive (the public reproducibility root).
pub const RAW_ARCHIVE: &str = "hyperliquid-raw";
/// Default name of the normalized partition archive.
pub const NORMALIZED_ARCHIVE: &str = "hyperliquid-normalized";
/// Default name of the unfiltered raw archive (private tier, never published).
pub const PRIVATE_RAW_ARCHIVE: &str = "hyperliquid-raw-private";

/// Directory, under an archive root, holding one subdirectory per day of
/// manifests.
pub const MANIFESTS_DIR: &str = "_manifests";
/// Signed manifest file name.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Anchors file name.
pub const ANCHORS_FILE: &str = "manifest.anchors.json";
/// OpenTimestamps proof file name.
pub const OTS_FILE: &str = "manifest.json.ots";
/// Unsigned manifest body written by `manifest build` before signing.
pub const UNSIGNED_MANIFEST_FILE: &str = "manifest.unsigned.json";
/// Pending Ethereum L2 anchor transaction state.
pub const L2_TX_FILE: &str = "manifest.l2-tx.json";
/// Index of every manifest of an archive (lets HTTP mirrors walk the chain).
pub const MANIFEST_INDEX_FILE: &str = "_manifests/index.json";
/// Magnet list of every published day.
pub const MAGNETS_FILE: &str = "_manifests/magnets.txt";
/// Pointer to the newest signed manifest of an archive (mutable, short
/// cache; probed by monitoring).
pub const LATEST_FILE: &str = "_manifests/latest.json";
/// Directory of Iceberg metadata under the normalized archive.
pub const ICEBERG_DIR: &str = "_iceberg";
/// Directory of zstd dictionaries under the private raw archive.
pub const DICTIONARIES_DIR: &str = "_dictionaries";

/// Rows per Parquet part file (2^24).
pub const PART_ROWS: usize = 1 << 24;

/// A UTC calendar day, `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day {
    /// Days since 1970-01-01.
    days: i32,
}

const MS_PER_DAY: i64 = 86_400_000;

// Howard Hinnant's civil-from-days and days-from-civil algorithms.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl Day {
    /// The day with this many days since 1970-01-01.
    #[must_use]
    pub const fn from_days(days: i32) -> Self {
        Self { days }
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub const fn days(self) -> i32 {
        self.days
    }

    /// The UTC day containing `ms` milliseconds since the epoch.
    #[must_use]
    pub fn from_millis(ms: i64) -> Self {
        let d = ms.div_euclid(MS_PER_DAY);
        Self {
            days: i32::try_from(d).unwrap_or(if d < 0 { i32::MIN } else { i32::MAX }),
        }
    }

    /// Milliseconds since the epoch at 00:00 UTC of this day.
    #[must_use]
    pub fn start_ms(self) -> i64 {
        i64::from(self.days) * MS_PER_DAY
    }

    /// The next day.
    #[must_use]
    pub const fn next(self) -> Self {
        Self {
            days: self.days.saturating_add(1),
        }
    }

    /// The previous day.
    #[must_use]
    pub const fn prev(self) -> Self {
        Self {
            days: self.days.saturating_sub(1),
        }
    }

    /// The day as `YYYYMMDD` (the node's hourly directory name).
    #[must_use]
    pub fn compact(self) -> String {
        self.to_string().replace('-', "")
    }

    /// Parses `YYYYMMDD`.
    ///
    /// # Errors
    ///
    /// Fails on anything else.
    pub fn parse_compact(s: &str) -> Result<Self> {
        if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::invalid(format!("not a YYYYMMDD day: {s:?}")));
        }
        format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]).parse()
    }
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = civil_from_days(i64::from(self.days));
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

impl FromStr for Day {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::invalid(format!("not a YYYY-MM-DD day: {s:?}"));
        let b = s.as_bytes();
        if b.len() != 10
            || b[4] != b'-'
            || b[7] != b'-'
            || !b
                .iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        {
            return Err(bad());
        }
        let y: i64 = s[0..4].parse().map_err(|_| bad())?;
        let m: u32 = s[5..7].parse().map_err(|_| bad())?;
        let d: u32 = s[8..10].parse().map_err(|_| bad())?;
        if !(1..=12).contains(&m) || d == 0 || d > 31 {
            return Err(bad());
        }
        let days = days_from_civil(y, m, d);
        let day = Self {
            days: i32::try_from(days).map_err(|_| bad())?,
        };
        // Reject impossible dates such as 2026-02-30 by round-tripping.
        if day.to_string() != s {
            return Err(bad());
        }
        Ok(day)
    }
}

impl serde::Serialize for Day {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Day {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Validates an archive name or stream slug: `[a-z0-9][a-z0-9_-]{0,31}`.
///
/// # Errors
///
/// Fails if `s` is not a slug.
pub fn check_slug(what: &str, s: &str) -> Result<()> {
    let b = s.as_bytes();
    let ok = !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!("{what} must be a slug: {s:?}")))
    }
}

/// Validates an archive name as manifests require (`[a-z0-9][a-z0-9-]{0,31}`).
///
/// # Errors
///
/// Fails if `s` is not a valid archive name.
pub fn check_archive(s: &str) -> Result<()> {
    check_slug("archive", s)?;
    if s.contains('_') {
        return Err(Error::invalid(format!("archive name has '_': {s:?}")));
    }
    Ok(())
}

/// Path of a raw file: `stream=<stream>/date=<day>/<HH>.jsonl.zst`.
#[must_use]
pub fn raw_path(stream: &str, day: Day, hour: u8) -> String {
    format!("stream={stream}/date={day}/{hour:02}.jsonl.zst")
}

/// A parsed raw file path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPath {
    /// Stream slug.
    pub stream: String,
    /// UTC day of the node file.
    pub day: Day,
    /// UTC hour of the node file.
    pub hour: u8,
}

/// Parses a path produced by [`raw_path`].
#[must_use]
pub fn parse_raw_path(path: &str) -> Option<RawPath> {
    let mut it = path.split('/');
    let stream = it.next()?.strip_prefix("stream=")?;
    let day = it.next()?.strip_prefix("date=")?.parse().ok()?;
    let file = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let hh = file.strip_suffix(".jsonl.zst")?;
    if hh.len() != 2 {
        return None;
    }
    let hour: u8 = hh.parse().ok()?;
    if hour > 23 || check_slug("stream", stream).is_err() {
        return None;
    }
    Some(RawPath {
        stream: stream.to_owned(),
        day,
        hour,
    })
}

/// Identifies one partition directory: kind, venue, instrument and day.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionKey {
    /// Record kind.
    pub kind: RecordKind,
    /// Venue.
    pub venue: VenueId,
    /// Instrument.
    pub instrument: InstrumentId,
    /// UTC day.
    pub day: Day,
}

impl PartitionKey {
    /// Directory of the partition, relative to the archive root.
    #[must_use]
    pub fn dir(&self) -> String {
        format!(
            "kind={}/venue={}/instrument={}/date={}",
            self.kind.as_str(),
            self.venue.as_str(),
            self.instrument.path_segment(),
            self.day
        )
    }

    /// Path of part `part`.
    #[must_use]
    pub fn part_path(&self, part: u32) -> String {
        format!("{}/part-{part:05}.parquet", self.dir())
    }
}

/// Parses a partition file path into its key and part number.
#[must_use]
pub fn parse_partition_path(path: &str) -> Option<(PartitionKey, u32)> {
    let mut it = path.split('/');
    let kind_s = it.next()?.strip_prefix("kind=")?;
    let kind = RecordKind::ALL.into_iter().find(|k| k.as_str() == kind_s)?;
    let venue = VenueId::new(it.next()?.strip_prefix("venue=")?).ok()?;
    let instrument =
        InstrumentId::from_path_segment(it.next()?.strip_prefix("instrument=")?).ok()?;
    let day = it.next()?.strip_prefix("date=")?.parse().ok()?;
    let file = it.next()?;
    if it.next().is_some() {
        return None;
    }
    let n = file.strip_prefix("part-")?.strip_suffix(".parquet")?;
    if n.len() != 5 {
        return None;
    }
    Some((
        PartitionKey {
            kind,
            venue,
            instrument,
            day,
        },
        n.parse().ok()?,
    ))
}

/// The day of a data file: the value of its `date=` segment, if any.
#[must_use]
pub fn day_of_path(path: &str) -> Option<Day> {
    path.split('/')
        .find_map(|seg| seg.strip_prefix("date="))
        .and_then(|d| d.parse().ok())
}

/// Directory of a day's manifests, relative to the archive root.
#[must_use]
pub fn manifest_dir(day: Day) -> String {
    format!("{MANIFESTS_DIR}/{day}")
}

/// Path of a file in a day's manifest directory.
#[must_use]
pub fn manifest_file(day: Day, name: &str) -> String {
    format!("{}/{name}", manifest_dir(day))
}

/// Path of the day's `.torrent` file: `_manifests/<day>/<archive>-<day>.torrent`.
#[must_use]
pub fn torrent_file(archive: &str, day: Day) -> String {
    manifest_file(day, &format!("{archive}-{day}.torrent"))
}

/// Whether `path` (relative to an archive root) is a data file that belongs
/// in a manifest: it has a `date=` segment and is not under a reserved
/// `_`-prefixed directory or a hidden or temporary name.
#[must_use]
pub fn is_data_path(path: &str) -> bool {
    path.split('/').all(|seg| {
        !seg.is_empty() && !seg.starts_with('_') && !seg.starts_with('.') && !seg.ends_with(".tmp")
    }) && day_of_path(path).is_some()
}

/// A local directory holding one or more archive roots.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// A layout rooted at `root`; archive `a` lives at `root/a`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory holding the archives.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root directory of `archive`.
    #[must_use]
    pub fn archive(&self, archive: &str) -> PathBuf {
        self.root.join(archive)
    }

    /// The local path of `rel` inside `archive`.
    #[must_use]
    pub fn file(&self, archive: &str, rel: &str) -> PathBuf {
        let mut p = self.archive(archive);
        for seg in rel.split('/') {
            p.push(seg);
        }
        p
    }
}

/// Lists every regular file under `dir`, as `/`-separated paths relative
/// to `dir`, sorted by byte order. A missing directory yields no files.
///
/// # Errors
///
/// Fails on I/O errors or non-UTF-8 names.
pub fn list_files(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, rel)) = stack.pop() {
        let entries = std::fs::read_dir(&path).map_err(|e| Error::io(&path, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io(&path, e))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|n| Error::invalid(format!("non-UTF-8 file name {n:?}")))?;
            let child_rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let ft = entry.file_type().map_err(|e| Error::io(entry.path(), e))?;
            if ft.is_dir() {
                stack.push((entry.path(), child_rel));
            } else if ft.is_file() {
                out.push(child_rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip() {
        let d: Day = "2026-01-01".parse().unwrap();
        assert_eq!(d.start_ms(), 1_767_225_600_000);
        assert_eq!(Day::from_millis(1_767_229_197_000), d);
        assert_eq!(Day::from_millis(1_767_225_599_999), d.prev());
        assert_eq!(d.next().to_string(), "2026-01-02");
        assert_eq!(d.compact(), "20260101");
        assert_eq!(Day::parse_compact("20260101").unwrap(), d);
        assert_eq!("1970-01-01".parse::<Day>().unwrap().days(), 0);
        assert_eq!(
            "2024-02-29".parse::<Day>().unwrap().next().to_string(),
            "2024-03-01"
        );
        for bad in ["2026-02-30", "2026-13-01", "2026-1-01", "26-01-01x", ""] {
            assert!(bad.parse::<Day>().is_err(), "{bad}");
        }
        for days in [-1000, 0, 1, 20_000, 60_000] {
            let d = Day::from_days(days);
            assert_eq!(d.to_string().parse::<Day>().unwrap(), d);
        }
    }

    #[test]
    fn paths() {
        let d: Day = "2026-01-01".parse().unwrap();
        let p = raw_path("fills", d, 7);
        assert_eq!(p, "stream=fills/date=2026-01-01/07.jsonl.zst");
        let parsed = parse_raw_path(&p).unwrap();
        assert_eq!(
            (parsed.stream.as_str(), parsed.day, parsed.hour),
            ("fills", d, 7)
        );
        assert!(parse_raw_path("stream=fills/date=2026-01-01/24.jsonl.zst").is_none());
        assert!(parse_raw_path("stream=Fills/date=2026-01-01/01.jsonl.zst").is_none());

        let key = PartitionKey {
            kind: RecordKind::BookEvent,
            venue: VenueId::new("hyperliquid").unwrap(),
            instrument: InstrumentId::new("hyperliquid:perp:xyz:CL").unwrap(),
            day: d,
        };
        let path = key.part_path(3);
        assert_eq!(
            path,
            "kind=book_event/venue=hyperliquid/instrument=hyperliquid~perp~xyz~CL/date=2026-01-01/part-00003.parquet"
        );
        assert_eq!(parse_partition_path(&path), Some((key, 3)));
        assert_eq!(day_of_path(&path), Some(d));
        assert!(is_data_path(&path));
        assert!(!is_data_path("_manifests/2026-01-01/manifest.json"));
        assert!(!is_data_path("kind=trade/x.parquet"));
        assert_eq!(
            torrent_file("hyperliquid-raw", d),
            "_manifests/2026-01-01/hyperliquid-raw-2026-01-01.torrent"
        );
    }

    #[test]
    fn slugs() {
        check_slug("stream", "order_statuses").unwrap();
        check_archive("hyperliquid-raw").unwrap();
        assert!(check_archive("hyper_liquid").is_err());
        assert!(check_slug("stream", "-x").is_err());
        assert!(check_slug("stream", "").is_err());
    }
}
