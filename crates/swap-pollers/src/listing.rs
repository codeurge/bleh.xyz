//! DTCC dashboard listings (`dashboard/Slice.json`,
//! `dashboard/Cumulative.json`).
//!
//! Each listing is a JSON object keyed by bucket (`CFTC_IR`, `SEC_EQ`,
//! ...); each value is an array of file entries. Unknown keys and fields
//! are ignored so a listing change cannot stop capture.

use std::collections::BTreeMap;

use mesh_types::UnixMillis;
use serde::{Deserialize, Serialize};

use crate::repository::DtccBucket;
use crate::values;

/// One downloadable report in a listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// File name, such as `CFTC_SLICE_RATES_2026_11_12_2.zip`.
    pub file_name: String,
    /// Absolute URL of the file.
    #[serde(default)]
    pub full_file_path: Option<String>,
    /// DTCC slice ID: increases across all files.
    #[serde(default)]
    pub slice_id: Option<u64>,
    /// Number of data rows.
    #[serde(default)]
    pub row_count: Option<u64>,
    /// Dissemination time of the file (`dissemDTM`).
    #[serde(default, rename = "dissemDTM")]
    pub dissem_dtm: Option<String>,
    /// Start of the slice window.
    #[serde(default)]
    pub start_ts: Option<String>,
    /// End of the slice window.
    #[serde(default)]
    pub end_ts: Option<String>,
}

impl FileEntry {
    /// The file's dissemination time, if parseable.
    #[must_use]
    pub fn dissemination(&self) -> Option<UnixMillis> {
        self.dissem_dtm
            .as_deref()
            .and_then(|s| values::timestamp(s).ok().flatten())
    }

    /// True if the file name is safe to use as a path component and URL
    /// segment: `[A-Za-z0-9_.-]`, no leading dot.
    #[must_use]
    pub fn has_safe_name(&self) -> bool {
        !self.file_name.is_empty()
            && self.file_name.len() <= 128
            && !self.file_name.starts_with('.')
            && self
                .file_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    }

    /// The UTC date in the file name (`..._2026_11_12...`), as
    /// `YYYY-MM-DD`.
    #[must_use]
    pub fn name_date(&self) -> Option<String> {
        name_date(&self.file_name)
    }

    /// The batch number for `seq.block_number`: the slice ID, or a number
    /// derived from the file name.
    #[must_use]
    pub fn batch(&self) -> u64 {
        self.slice_id
            .unwrap_or_else(|| batch_from_name(&self.file_name))
    }
}

/// The UTC date in a DTCC file name, as `YYYY-MM-DD`.
#[must_use]
pub fn name_date(name: &str) -> Option<String> {
    let parts: Vec<&str> = name.trim_end_matches(".zip").split('_').collect();
    parts.windows(3).find_map(|w| {
        let ok = w[0].len() == 4
            && w[1].len() == 2
            && w[2].len() == 2
            && w.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit()));
        ok.then(|| format!("{}-{}-{}", w[0], w[1], w[2]))
    })
}

/// A batch number from a file name when no slice ID is known:
/// `YYYYMMDD * 1_000_000 + slice sequence` (0 for cumulative files).
#[must_use]
pub fn batch_from_name(name: &str) -> u64 {
    let stem = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = stem.split('.').next().unwrap_or(stem);
    let date: u64 = name_date(stem)
        .map(|d| d.replace('-', ""))
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);
    let parts: Vec<&str> = stem.split('_').collect();
    let seq: u64 = if parts.len() >= 7 {
        parts.last().and_then(|s| s.parse().ok()).unwrap_or(0)
    } else {
        0
    };
    date * 1_000_000 + seq.min(999_999)
}

/// A parsed listing: entries for each admitted bucket present.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Entries per bucket, as listed.
    pub buckets: BTreeMap<DtccBucket, Vec<FileEntry>>,
    /// Keys that are not admitted buckets (for example, Canadian ones),
    /// ignored.
    pub ignored_keys: Vec<String>,
    /// Entries that could not be read.
    pub bad_entries: u64,
}

/// Parses a listing body.
///
/// # Errors
///
/// Fails if the body is not a JSON object.
pub fn parse_listing(body: &[u8]) -> Result<Listing, serde_json::Error> {
    let raw: BTreeMap<String, serde_json::Value> = serde_json::from_slice(body)?;
    let mut out = Listing::default();
    for (key, value) in raw {
        let Ok(bucket) = key.parse::<DtccBucket>() else {
            out.ignored_keys.push(key);
            continue;
        };
        let items = match value {
            serde_json::Value::Array(items) => items,
            _ => {
                out.bad_entries += 1;
                continue;
            }
        };
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            match serde_json::from_value::<FileEntry>(item) {
                Ok(e) if e.has_safe_name() => entries.push(e),
                _ => out.bad_entries += 1,
            }
        }
        out.buckets.insert(bucket, entries);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_ignores_unknowns() {
        let body = br#"{
          "CFTC_IR": [{"dissemDTM":"2026-11-12T04:36:08Z","endTs":"2026-11-12T04:36:07Z",
                       "fileName":"CFTC_SLICE_RATES_2026_11_12_2.zip",
                       "fullFilePath":"https://example.invalid/cftc/slices/CFTC_SLICE_RATES_2026_11_12_2.zip",
                       "rowCount":3,"sliceId":12345,"startTs":"2026-11-12T04:35:56Z","newField":1},
                      {"fileName":"../etc/passwd"}],
          "CA_IR": [],
          "SEC_EQ": "oops"
        }"#;
        let l = parse_listing(body).unwrap();
        assert_eq!(l.ignored_keys, vec!["CA_IR".to_string()]);
        assert_eq!(l.bad_entries, 2);
        let e = &l.buckets[&"CFTC_IR".parse().unwrap()][0];
        assert_eq!(e.batch(), 12345);
        assert_eq!(e.name_date().as_deref(), Some("2026-11-12"));
        assert_eq!(e.dissemination(), Some(UnixMillis(1_794_458_168_000)));
    }

    #[test]
    fn batch_numbers_from_names() {
        assert_eq!(
            batch_from_name("CFTC_SLICE_RATES_2026_11_12_2.zip"),
            20_261_112_000_002
        );
        assert_eq!(
            batch_from_name("CFTC_CUMULATIVE_RATES_2026_11_11.zip"),
            20_261_111_000_000
        );
        assert_eq!(batch_from_name("whatever.csv"), 0);
    }
}
