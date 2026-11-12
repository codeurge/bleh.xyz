//! Finding partition files in the archive layout.
//!
//! Paths follow `docs/specs/archive-layout-v0.md` § 3.1:
//!
//! ```text
//! <root>/<archive>/kind=<kind>/venue=<venue>/instrument=<id with : as ~>/date=<YYYY-MM-DD>/part-<NNNNN>.parquet
//! ```
//!
//! Three ways to find files, as in the spec's § 4:
//!
//! - **Local directory**: walk the tree (exact, cheap).
//! - **Object store** (`s3://`, `r2://`, `gcs://`): expand globs with
//!   DuckDB's `glob()` table function, which returns nothing (rather than
//!   failing) when a day has no files.
//! - **HTTPS**: plain HTTPS cannot list, so read the day's
//!   `_manifests/<date>/manifest.json` and take its file list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mesh_types::RecordKind;
use mesh_types::manifest::SignedManifest;

use crate::config::ArchiveRoot;
use crate::params::day_string;

/// One partition directory's identity, parsed from a path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PartitionKey {
    /// Archive name.
    pub archive: String,
    /// Record kind (`trade`, ...).
    pub kind: String,
    /// Venue.
    pub venue: String,
    /// Instrument ID (real form, with `:`).
    pub instrument: String,
    /// `YYYY-MM-DD`.
    pub date: String,
}

/// Parses `kind=.../venue=.../instrument=.../date=...` segments out of a
/// relative path (`<archive>/kind=...`). Returns `None` for other paths.
#[must_use]
pub fn parse_partition_path(archive: &str, rel: &str) -> Option<PartitionKey> {
    let mut kind = None;
    let mut venue = None;
    let mut instrument = None;
    let mut date = None;
    for seg in rel.split('/') {
        if let Some(v) = seg.strip_prefix("kind=") {
            kind = Some(v.to_owned());
        } else if let Some(v) = seg.strip_prefix("venue=") {
            venue = Some(v.to_owned());
        } else if let Some(v) = seg.strip_prefix("instrument=") {
            instrument = Some(v.replace('~', ":"));
        } else if let Some(v) = seg.strip_prefix("date=") {
            date = Some(v.to_owned());
        }
    }
    Some(PartitionKey {
        archive: archive.to_owned(),
        kind: kind?,
        venue: venue?,
        instrument: instrument?,
        date: date?,
    })
}

/// Quotes a string as a SQL literal.
#[must_use]
pub fn sql_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Builds a SQL list literal of file paths for `read_parquet`.
#[must_use]
pub fn sql_file_list(files: &[String]) -> String {
    let items: Vec<String> = files.iter().map(|f| sql_literal(f)).collect();
    format!("[{}]", items.join(", "))
}

/// Location helpers for one archive root.
#[derive(Debug, Clone)]
pub struct Layout {
    /// Root.
    pub root: ArchiveRoot,
    /// Archive names, unioned.
    pub archives: Vec<String>,
}

impl Layout {
    /// Base location of an archive (path or URL, no trailing slash).
    #[must_use]
    pub fn archive_base(&self, archive: &str) -> String {
        match &self.root {
            ArchiveRoot::Local(p) => p.join(archive).display().to_string(),
            ArchiveRoot::ObjectStore(u) | ArchiveRoot::Https(u) => format!("{u}/{archive}"),
        }
    }

    /// The relative directory of a partition.
    #[must_use]
    pub fn partition_dir(
        kind: RecordKind,
        instrument: &mesh_types::InstrumentId,
        day: i64,
    ) -> String {
        format!(
            "kind={}/venue={}/instrument={}/date={}",
            kind.as_str(),
            instrument.venue_part(),
            instrument.path_segment(),
            day_string(day)
        )
    }

    /// Local directory of a partition, for local roots.
    #[must_use]
    pub fn local_partition_dir(
        &self,
        archive: &str,
        kind: RecordKind,
        instrument: &mesh_types::InstrumentId,
        day: i64,
    ) -> Option<PathBuf> {
        match &self.root {
            ArchiveRoot::Local(p) => Some(
                p.join(archive)
                    .join(Self::partition_dir(kind, instrument, day)),
            ),
            _ => None,
        }
    }

    /// Glob for one partition directory's Parquet files (object stores).
    #[must_use]
    pub fn partition_glob(
        &self,
        archive: &str,
        kind: RecordKind,
        instrument: &mesh_types::InstrumentId,
        day: i64,
    ) -> String {
        format!(
            "{}/{}/*.parquet",
            self.archive_base(archive),
            Self::partition_dir(kind, instrument, day)
        )
    }

    /// Manifest location for a day.
    #[must_use]
    pub fn manifest_location(&self, archive: &str, date: &str) -> String {
        format!(
            "{}/_manifests/{date}/manifest.json",
            self.archive_base(archive)
        )
    }
}

/// Lists Parquet files in a local partition directory, sorted by name.
#[must_use]
pub fn list_local_parquet(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.ends_with(".parquet")
                .then(|| e.path().display().to_string())
        })
        .collect();
    files.sort();
    files
}

/// Walks a local archive and returns every partition with its files.
#[must_use]
pub fn walk_local_archive(root: &Path, archive: &str) -> BTreeMap<PartitionKey, Vec<String>> {
    let mut out = BTreeMap::new();
    let base = root.join(archive);
    let level = |p: &Path, prefix: &str| -> Vec<(String, PathBuf)> {
        let Ok(rd) = std::fs::read_dir(p) else {
            return Vec::new();
        };
        let mut v: Vec<(String, PathBuf)> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let val = name.strip_prefix(prefix)?.to_owned();
                Some((val, e.path()))
            })
            .collect();
        v.sort();
        v
    };
    for (kind, kp) in level(&base, "kind=") {
        for (venue, vp) in level(&kp, "venue=") {
            for (inst, ip) in level(&vp, "instrument=") {
                for (date, dp) in level(&ip, "date=") {
                    let files = list_local_parquet(&dp);
                    if files.is_empty() {
                        continue;
                    }
                    out.insert(
                        PartitionKey {
                            archive: archive.to_owned(),
                            kind: kind.clone(),
                            venue: venue.clone(),
                            instrument: inst.replace('~', ":"),
                            date: date.clone(),
                        },
                        files,
                    );
                }
            }
        }
    }
    out
}

/// Lists the days that have a manifest in a local archive.
#[must_use]
pub fn local_manifest_days(root: &Path, archive: &str) -> BTreeSet<String> {
    let dir = root.join(archive).join("_manifests");
    let Ok(rd) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    rd.filter_map(Result::ok)
        .filter(|e| e.path().join("manifest.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

/// Files of a manifest that belong to one partition directory, as URLs.
#[must_use]
pub fn manifest_partition_files(
    manifest: &SignedManifest,
    base: &str,
    partition_dir: &str,
) -> Vec<String> {
    let prefix = format!("{partition_dir}/");
    manifest
        .manifest
        .files
        .iter()
        .filter(|f| f.path.starts_with(&prefix) && f.path.ends_with(".parquet"))
        .map(|f| format!("{base}/{}", f.path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_types::InstrumentId;

    #[test]
    fn partition_paths() {
        let id = InstrumentId::new("hyperliquid:perp:syn:IDX").unwrap();
        let d = Layout::partition_dir(RecordKind::Trade, &id, 20_454);
        assert_eq!(
            d,
            "kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~syn~IDX/date=2026-01-01"
        );
        let k = parse_partition_path("a", &format!("a/{d}/part-00000.parquet")).unwrap();
        assert_eq!(k.instrument, "hyperliquid:perp:syn:IDX");
        assert_eq!(k.kind, "trade");
        assert_eq!(k.date, "2026-01-01");
        assert!(parse_partition_path("a", "a/_manifests/2026-01-01/manifest.json").is_none());
    }

    #[test]
    fn literals_escape_quotes() {
        assert_eq!(sql_literal("a'b"), "'a''b'");
        assert_eq!(sql_file_list(&["x".into(), "y".into()]), "['x', 'y']");
    }

    #[test]
    fn layout_locations() {
        let l = Layout {
            root: ArchiveRoot::Https("https://cdn.example".into()),
            archives: vec!["hyperliquid-normalized".into()],
        };
        assert_eq!(
            l.manifest_location("hyperliquid-normalized", "2026-01-01"),
            "https://cdn.example/hyperliquid-normalized/_manifests/2026-01-01/manifest.json"
        );
        let id = InstrumentId::new("hyperliquid:perp:SYN").unwrap();
        assert!(
            l.partition_glob("hyperliquid-normalized", RecordKind::Bar, &id, 0)
                .ends_with("date=1970-01-01/*.parquet")
        );
    }
}
