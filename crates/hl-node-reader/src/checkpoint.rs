//! Resume points and the checkpoint file.
//!
//! A checkpoint records, per reader, where each stream resumes (hourly file
//! and byte offset of the first line not yet covered by an emitted block),
//! and the sequencer state (`prev_block_number` links and pending gaps), so
//! a restart continues the published streams without breaking their links.
//! Delivery is at least once: blocks after the checkpoint may be re-read
//! and are dropped downstream by block number.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::raw::Stream;
use crate::sequence::SequencerState;

/// A byte position in one stream's hourly files.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// Hourly file, relative to the stream directory (`hourly/{date}/{hour}`).
    pub file: String,
    /// Byte offset of the next line to read.
    pub offset: u64,
}

/// Where one reader resumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderPositions {
    /// Last block covered by these positions; blocks at or below it are
    /// skipped on resume.
    pub last_block: u64,
    /// Per-stream positions. A missing stream starts at its first file.
    pub streams: BTreeMap<Stream, Position>,
}

/// Current checkpoint format version.
pub const CHECKPOINT_VERSION: u32 = 1;

/// The checkpoint file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    /// Format version ([`CHECKPOINT_VERSION`]).
    pub version: u32,
    /// Per reader, in configuration order.
    pub readers: Vec<Option<ReaderPositions>>,
    /// Sequencer state of the published output.
    pub sequencer: SequencerState,
}

/// Errors reading or writing a checkpoint.
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// I/O failure.
    #[error("checkpoint {path}: {source}")]
    Io {
        /// File.
        path: String,
        /// Error.
        source: std::io::Error,
    },
    /// The file is not a valid checkpoint.
    #[error("checkpoint {path}: {source}")]
    Format {
        /// File.
        path: String,
        /// Error.
        source: serde_json::Error,
    },
    /// The file has another format version.
    #[error("checkpoint {path}: version {found}, expected {CHECKPOINT_VERSION}")]
    Version {
        /// File.
        path: String,
        /// Version found.
        found: u32,
    },
}

impl Checkpoint {
    /// Loads a checkpoint, or `None` if the file does not exist.
    ///
    /// # Errors
    ///
    /// Fails if the file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Option<Self>, CheckpointError> {
        let p = path.display().to_string();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(CheckpointError::Io { path: p, source }),
        };
        let cp: Checkpoint =
            serde_json::from_slice(&bytes).map_err(|source| CheckpointError::Format {
                path: p.clone(),
                source,
            })?;
        if cp.version != CHECKPOINT_VERSION {
            return Err(CheckpointError::Version {
                path: p,
                found: cp.version,
            });
        }
        Ok(Some(cp))
    }

    /// Writes the checkpoint atomically (temporary file, fsync, rename).
    ///
    /// # Errors
    ///
    /// Fails on any I/O error.
    pub fn save(&self, path: &Path) -> Result<(), CheckpointError> {
        let p = path.display().to_string();
        let io = |source| CheckpointError::Io {
            path: p.clone(),
            source,
        };
        let json = serde_json::to_vec_pretty(self).map_err(|source| CheckpointError::Format {
            path: p.clone(),
            source,
        })?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = std::path::PathBuf::from(tmp);
        {
            let mut f = std::fs::File::create(&tmp).map_err(io)?;
            f.write_all(&json).map_err(io)?;
            f.write_all(b"\n").map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cp.json");
        assert!(Checkpoint::load(&path).unwrap().is_none());
        let mut streams = BTreeMap::new();
        streams.insert(
            Stream::Fills,
            Position {
                file: "hourly/20260101/0".into(),
                offset: 42,
            },
        );
        let cp = Checkpoint {
            version: CHECKPOINT_VERSION,
            readers: vec![
                Some(ReaderPositions {
                    last_block: 9,
                    streams,
                }),
                None,
            ],
            sequencer: SequencerState::default(),
        };
        cp.save(&path).unwrap();
        assert_eq!(Checkpoint::load(&path).unwrap(), Some(cp.clone()));
        let mut v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v["version"] = serde_json::json!(99);
        std::fs::write(&path, v.to_string()).unwrap();
        assert!(matches!(
            Checkpoint::load(&path),
            Err(CheckpointError::Version { found: 99, .. })
        ));
    }
}
