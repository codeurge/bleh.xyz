//! Backfill against live recording on overlap days.
//!
//! ADM-0003 asks for archived data to be compared with live recording
//! wherever both exist. Records from the two paths can never be
//! byte-identical: backfilled records carry `ADM-0003` and the `backfill`
//! (and for snapshots `low_resolution`) flags, and `ts_ingest_ns` is the
//! receive time of a different node. The comparison therefore hashes a
//! **comparable projection** of each record:
//!
//! - `admission` set to `ADM-0000` on both sides;
//! - flags `backfill` and `low_resolution` cleared;
//! - `ts_ingest_ns` set to 0 and `ts_publish_ns` removed.
//!
//! Records are grouped by UTC hour of `ts_source_ms`, kind and instrument,
//! sorted canonically and hashed with the mesh content hash. Each group is
//! reported as equal, different (with counts and the first differing
//! position), or present on one side only.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead;
use std::path::Path;

use mesh_types::{AdmissionId, Flag, Record, UnixNanos};
use serde::Serialize;

use crate::process::hour_label;

/// One group's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GroupResult {
    /// Same comparable content.
    Equal {
        /// Records on each side.
        count: u64,
    },
    /// Different content.
    Different {
        /// Backfilled records.
        backfill_count: u64,
        /// Live records.
        live_count: u64,
        /// Index (canonical order) of the first record that differs.
        first_difference: u64,
        /// Comparable content hashes.
        backfill_hash: String,
        /// Comparable content hash of the live side.
        live_hash: String,
    },
    /// Only backfilled records.
    BackfillOnly {
        /// Records.
        count: u64,
    },
    /// Only live records.
    LiveOnly {
        /// Records.
        count: u64,
    },
}

/// The comparison.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CompareReport {
    /// By `YYYY-MM-DD/HH kind/instrument`.
    pub groups: BTreeMap<String, GroupResult>,
    /// Hours present on both sides.
    pub overlap_hours: u64,
    /// Groups equal within the overlap hours.
    pub equal: u64,
    /// Groups different within the overlap hours.
    pub different: u64,
    /// Groups present on one side within the overlap hours.
    pub one_sided: u64,
}

impl CompareReport {
    /// True if every group in the overlap hours is equal.
    #[must_use]
    pub fn matches(&self) -> bool {
        self.different == 0 && self.one_sided == 0
    }
}

/// Errors from loading or comparing.
#[derive(Debug, thiserror::Error)]
pub enum CompareError {
    /// I/O.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The error.
        source: std::io::Error,
    },
    /// A line is not a record.
    #[error("{path} line {line}: {reason}")]
    Parse {
        /// The file.
        path: String,
        /// 1-based line.
        line: usize,
        /// Parser message.
        reason: String,
    },
    /// Encoding failed.
    #[error("encoding: {0}")]
    Encode(String),
}

/// The comparable projection of a record (see the module docs).
///
/// # Panics
///
/// Never: `ADM-0000` is a valid admission ID.
#[must_use]
pub fn comparable(r: &Record) -> Record {
    let mut r = r.clone();
    let p = r.prov_mut();
    p.admission = AdmissionId::new("ADM-0000").unwrap_or_else(|_| unreachable!());
    p.flags = p.flags.without(Flag::Backfill).without(Flag::LowResolution);
    p.ts_ingest_ns = UnixNanos(0);
    p.ts_publish_ns = None;
    r
}

type Groups = BTreeMap<String, Vec<Record>>;

fn group(records: &[Record]) -> (Groups, BTreeSet<String>) {
    let mut groups: Groups = BTreeMap::new();
    let mut hours = BTreeSet::new();
    for r in records {
        let hour = hour_label(r);
        hours.insert(hour.clone());
        let key = format!("{hour} {}/{}", r.kind().as_str(), r.prov().instrument);
        groups.entry(key).or_default().push(comparable(r));
    }
    for rs in groups.values_mut() {
        rs.sort_by(|a, b| a.canonical_cmp(b));
    }
    (groups, hours)
}

fn hash(rs: &[Record]) -> Result<String, CompareError> {
    mesh_types::content_hash(rs)
        .map(|h| h.to_string())
        .map_err(|e| CompareError::Encode(e.to_string()))
}

/// Compares backfilled and live records.
///
/// # Errors
///
/// Fails if a record cannot be encoded.
pub fn compare(backfill: &[Record], live: &[Record]) -> Result<CompareReport, CompareError> {
    let (bg, bh) = group(backfill);
    let (lg, lh) = group(live);
    let overlap: BTreeSet<&String> = bh.intersection(&lh).collect();
    let mut report = CompareReport {
        overlap_hours: overlap.len() as u64,
        ..CompareReport::default()
    };
    let keys: BTreeSet<&String> = bg.keys().chain(lg.keys()).collect();
    for key in keys {
        let in_overlap = key
            .split(' ')
            .next()
            .is_some_and(|h| overlap.contains(&h.to_owned()));
        let result = match (bg.get(key), lg.get(key)) {
            (Some(b), Some(l)) => {
                let (hb, hl) = (hash(b)?, hash(l)?);
                if hb == hl {
                    GroupResult::Equal {
                        count: b.len() as u64,
                    }
                } else {
                    let first = b
                        .iter()
                        .zip(l.iter())
                        .position(|(x, y)| x != y)
                        .unwrap_or(b.len().min(l.len()));
                    GroupResult::Different {
                        backfill_count: b.len() as u64,
                        live_count: l.len() as u64,
                        first_difference: first as u64,
                        backfill_hash: hb,
                        live_hash: hl,
                    }
                }
            }
            (Some(b), None) => GroupResult::BackfillOnly {
                count: b.len() as u64,
            },
            (None, Some(l)) => GroupResult::LiveOnly {
                count: l.len() as u64,
            },
            (None, None) => continue,
        };
        if in_overlap {
            match result {
                GroupResult::Equal { .. } => report.equal += 1,
                GroupResult::Different { .. } => report.different += 1,
                _ => report.one_sided += 1,
            }
        }
        report.groups.insert(key.clone(), result);
    }
    Ok(report)
}

/// Loads records from a JSON-lines file, or from every `*.jsonl` file
/// under a directory (recursively, in path order).
///
/// # Errors
///
/// Fails on I/O errors or a line that is not a record.
pub fn load_records(path: &Path) -> Result<Vec<Record>, CompareError> {
    let io = |p: &Path, source| CompareError::Io {
        path: p.display().to_string(),
        source,
    };
    let mut files = Vec::new();
    if path.is_dir() {
        let mut stack = vec![path.to_owned()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).map_err(|e| io(&dir, e))? {
                let p = e.map_err(|e| io(&dir, e))?.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "jsonl") {
                    files.push(p);
                }
            }
        }
        files.sort();
    } else {
        files.push(path.to_owned());
    }
    let mut out = Vec::new();
    for f in files {
        let file = std::fs::File::open(&f).map_err(|e| io(&f, e))?;
        for (i, line) in std::io::BufReader::new(file).lines().enumerate() {
            let line = line.map_err(|e| io(&f, e))?;
            if line.trim().is_empty() {
                continue;
            }
            out.push(
                serde_json::from_str(&line).map_err(|e| CompareError::Parse {
                    path: f.display().to_string(),
                    line: i + 1,
                    reason: e.to_string(),
                })?,
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::tests::trade;
    use mesh_types::{Flags, Price};

    fn as_live(r: &Record) -> Record {
        let mut r = r.clone();
        let p = r.prov_mut();
        p.admission = AdmissionId::new("ADM-0001").unwrap();
        p.flags = Flags::NONE;
        p.ts_ingest_ns = UnixNanos(p.ts_ingest_ns.0 + 12_345);
        r
    }

    fn backfilled(r: Record) -> Record {
        let mut r = r;
        r.prov_mut().flags.insert(Flag::Backfill);
        r
    }

    const H9: i64 = 1_753_606_800_000;

    #[test]
    fn equal_hours_match_despite_provenance_differences() {
        let b: Vec<Record> = (0..3)
            .map(|i| {
                backfilled(trade(
                    "hyperliquid:perp:BTC",
                    10 + i,
                    0,
                    H9 + i as i64 * 1000,
                ))
            })
            .collect();
        let l: Vec<Record> = b.iter().rev().map(as_live).collect();
        let r = compare(&b, &l).unwrap();
        assert!(r.matches(), "{r:?}");
        assert_eq!((r.overlap_hours, r.equal), (1, 1));
    }

    #[test]
    fn reports_differences_and_one_sided_groups() {
        let b = vec![
            backfilled(trade("hyperliquid:perp:BTC", 10, 0, H9)),
            backfilled(trade("hyperliquid:perp:BTC", 11, 0, H9 + 1000)),
            backfilled(trade("hyperliquid:perp:SOL", 11, 0, H9 + 1000)),
            // An hour live never recorded: outside the overlap.
            backfilled(trade("hyperliquid:perp:BTC", 99, 0, H9 - 3_600_000)),
        ];
        let mut l: Vec<Record> = b[..2].iter().map(as_live).collect();
        if let Record::Trade(t) = &mut l[1] {
            t.price = Price::new(101, 1).unwrap();
        }
        let r = compare(&b, &l).unwrap();
        assert!(!r.matches());
        assert_eq!((r.different, r.one_sided, r.equal), (1, 1, 0));
        let g = &r.groups["2025-07-27/09 trade/hyperliquid:perp:BTC"];
        assert!(
            matches!(
                g,
                GroupResult::Different {
                    first_difference: 1,
                    ..
                }
            ),
            "{g:?}"
        );
        assert!(matches!(
            r.groups["2025-07-27/08 trade/hyperliquid:perp:BTC"],
            GroupResult::BackfillOnly { count: 1 }
        ));
    }

    #[test]
    fn loads_jsonl_dirs() {
        use crate::partition::{JsonlWriter, PartitionKey, PartitionWriter};
        let dir = crate::testutil::tempdir();
        let mut w = JsonlWriter::new(&dir);
        let a = trade("hyperliquid:perp:BTC", 1, 0, H9);
        let b = trade("hyperliquid:perp:SOL", 1, 0, H9);
        w.write_partition(&PartitionKey::of(&a), std::slice::from_ref(&a))
            .unwrap();
        w.write_partition(&PartitionKey::of(&b), std::slice::from_ref(&b))
            .unwrap();
        let back = load_records(&dir).unwrap();
        assert_eq!(back.len(), 2);
    }
}
