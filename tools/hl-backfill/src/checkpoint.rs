//! Resumable copy checkpoint: a JSON-lines manifest of copied objects.
//!
//! Each line records one object copied verbatim: source bucket and key,
//! size and ETag as listed, the destination key, and the SHA-256 of the
//! bytes copied. A rerun skips every object whose bucket, key, size and
//! ETag match a line; an object whose size or ETag changed since it was
//! copied is copied again and reported. The file doubles as the copy log.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One copied object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointEntry {
    /// Source bucket.
    pub bucket: String,
    /// Source key.
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// Source ETag, as listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Destination key in the raw tier.
    pub dest: String,
    /// SHA-256 of the copied bytes, lowercase hex.
    pub sha256: String,
}

/// What the checkpoint says about an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Never copied.
    New,
    /// Copied with the same size and ETag.
    Done,
    /// Copied before, but the source's size or ETag has changed.
    Changed,
}

/// Errors reading or writing a checkpoint.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// File I/O.
    #[error("checkpoint {path}: {source}")]
    Io {
        /// The checkpoint file.
        path: PathBuf,
        /// The error.
        source: std::io::Error,
    },
    /// A malformed line other than a truncated last line.
    #[error("checkpoint {path} line {line}: {source}")]
    Parse {
        /// The checkpoint file.
        path: PathBuf,
        /// 1-based line number.
        line: usize,
        /// The error.
        source: serde_json::Error,
    },
}

/// A checkpoint, loaded in memory and appended to as objects complete.
#[derive(Debug)]
pub struct Checkpoint {
    entries: HashMap<(String, String), CheckpointEntry>,
    path: Option<PathBuf>,
    out: Option<BufWriter<File>>,
}

impl Checkpoint {
    /// An in-memory checkpoint with no file (tests and dry runs).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            entries: HashMap::new(),
            path: None,
            out: None,
        }
    }

    /// Opens (or creates) the checkpoint at `path`. A truncated final line,
    /// left by an interrupted run, is ignored.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a malformed line before the last.
    pub fn open(path: &Path) -> Result<Self, CheckpointError> {
        let io = |source| CheckpointError::Io {
            path: path.to_owned(),
            source,
        };
        let mut entries = HashMap::new();
        if path.exists() {
            let lines: Vec<String> = BufReader::new(File::open(path).map_err(io)?)
                .lines()
                .collect::<Result<_, _>>()
                .map_err(io)?;
            let last = lines.len();
            for (i, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<CheckpointEntry>(line) {
                    Ok(e) => {
                        entries.insert((e.bucket.clone(), e.key.clone()), e);
                    }
                    Err(_) if i + 1 == last => {
                        tracing::warn!(path = %path.display(), "ignoring truncated last checkpoint line");
                    }
                    Err(source) => {
                        return Err(CheckpointError::Parse {
                            path: path.to_owned(),
                            line: i + 1,
                            source,
                        });
                    }
                }
            }
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        // Drop a partial last line left by a run that died mid-write.
        if path.exists() {
            let content = std::fs::read(path).map_err(io)?;
            if !content.is_empty() && content.last() != Some(&b'\n') {
                let keep = content
                    .iter()
                    .rposition(|b| *b == b'\n')
                    .map_or(0, |i| i + 1);
                OpenOptions::new()
                    .write(true)
                    .open(path)
                    .and_then(|f| f.set_len(keep as u64))
                    .map_err(io)?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(io)?;
        Ok(Self {
            entries,
            path: Some(path.to_owned()),
            out: Some(BufWriter::new(file)),
        })
    }

    /// Status of an object as listed now.
    #[must_use]
    pub fn status(&self, bucket: &str, key: &str, size: u64, etag: Option<&str>) -> Status {
        match self.entries.get(&(bucket.to_owned(), key.to_owned())) {
            None => Status::New,
            Some(e) if e.size == size && e.etag.as_deref() == etag => Status::Done,
            Some(_) => Status::Changed,
        }
    }

    /// The entry for an object, if any.
    #[must_use]
    pub fn get(&self, bucket: &str, key: &str) -> Option<&CheckpointEntry> {
        self.entries.get(&(bucket.to_owned(), key.to_owned()))
    }

    /// Number of objects recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if nothing is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records a copied object and appends it to the file (flushed per
    /// line, so a crash loses at most the line being written).
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn record(&mut self, entry: CheckpointEntry) -> Result<(), CheckpointError> {
        if let (Some(out), Some(path)) = (self.out.as_mut(), self.path.as_ref()) {
            let io = |source| CheckpointError::Io {
                path: path.clone(),
                source,
            };
            let mut line = serde_json::to_vec(&entry).map_err(|e| io(std::io::Error::other(e)))?;
            line.push(b'\n');
            out.write_all(&line).map_err(io)?;
            out.flush().map_err(io)?;
        }
        self.entries
            .insert((entry.bucket.clone(), entry.key.clone()), entry);
        Ok(())
    }

    /// Flushes and syncs the file.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn sync(&mut self) -> Result<(), CheckpointError> {
        if let (Some(out), Some(path)) = (self.out.as_mut(), self.path.as_ref()) {
            let io = |source| CheckpointError::Io {
                path: path.clone(),
                source,
            };
            out.flush().map_err(io)?;
            out.get_ref().sync_all().map_err(io)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tempdir;

    fn entry(key: &str, size: u64, etag: &str) -> CheckpointEntry {
        CheckpointEntry {
            bucket: "hl-mainnet-node-data".into(),
            key: key.into(),
            size,
            etag: Some(etag.into()),
            dest: format!("raw/{key}"),
            sha256: "00".repeat(32),
        }
    }

    #[test]
    fn status_tracks_size_and_etag() {
        let mut c = Checkpoint::in_memory();
        c.record(entry("a", 10, "e1")).unwrap();
        assert_eq!(
            c.status("hl-mainnet-node-data", "a", 10, Some("e1")),
            Status::Done
        );
        assert_eq!(
            c.status("hl-mainnet-node-data", "a", 11, Some("e1")),
            Status::Changed
        );
        assert_eq!(
            c.status("hl-mainnet-node-data", "a", 10, Some("e2")),
            Status::Changed
        );
        assert_eq!(
            c.status("hl-mainnet-node-data", "b", 10, Some("e1")),
            Status::New
        );
    }

    #[test]
    fn persists_and_tolerates_truncated_tail() {
        let dir = tempdir();
        let path = dir.join("ckpt.jsonl");
        {
            let mut c = Checkpoint::open(&path).unwrap();
            c.record(entry("a", 1, "x")).unwrap();
            c.record(entry("b", 2, "y")).unwrap();
            c.sync().unwrap();
        }
        // Simulate a crash mid-line.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"bucket\":\"hl-mai").unwrap();
        drop(f);
        let mut c = Checkpoint::open(&path).unwrap();
        assert_eq!(c.len(), 2);
        c.record(entry("c", 3, "z")).unwrap();
        drop(c);
        let c = Checkpoint::open(&path).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c.get("hl-mainnet-node-data", "c").unwrap().size, 3);
    }

    #[test]
    fn rejects_corruption_before_the_last_line() {
        let dir = tempdir();
        let path = dir.join("ckpt.jsonl");
        std::fs::write(&path, "garbage\n{}\n").unwrap();
        assert!(matches!(
            Checkpoint::open(&path),
            Err(CheckpointError::Parse { line: 1, .. })
        ));
    }
}
