//! Durable revision and dedupe state.
//!
//! For each revision key (see [`crate::obs::revision_key`]) the store keeps
//! a 64-bit hash of the key, a 64-bit hash of the last emitted value and
//! the last revision number, sharded by the value's date. Shards live in
//! `<dir>/rev-<YYYY-MM-DD>.bin` (24 bytes per entry, little endian) and
//! are rewritten atomically when they change. Shards older than the
//! retention window are dropped; a value older than the window that shows
//! up again is treated as new (revision 0), which only happens when a
//! source republishes old history, and the archive dedupes on content.
//!
//! An in-memory store (no directory) serves backfill units and tests.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::time::Date;

/// What to do with an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Same value as last emitted: skip.
    Duplicate,
    /// Emit with this revision number.
    Emit(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    value: u64,
    revision: u32,
}

/// Revision state for one source.
#[derive(Debug)]
pub struct RevisionStore {
    dir: Option<PathBuf>,
    retention_days: i64,
    shards: BTreeMap<Date, HashMap<u64, Entry>>,
    dirty: BTreeSet<Date>,
}

fn h64(s: &str) -> u64 {
    let d = Sha256::digest(s.as_bytes());
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_le_bytes(b)
}

fn shard_path(dir: &Path, day: Date) -> PathBuf {
    dir.join(format!("rev-{day}.bin"))
}

impl RevisionStore {
    /// An in-memory store.
    #[must_use]
    pub fn memory() -> Self {
        Self {
            dir: None,
            retention_days: i64::MAX / 4,
            shards: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }

    /// Opens (or creates) the store in `dir`, keeping shards for dates
    /// within `retention_days` of `today`.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a corrupt shard.
    pub fn open(
        dir: impl Into<PathBuf>,
        retention_days: u32,
        today: Date,
    ) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let mut shards = BTreeMap::new();
        let cutoff = today.add_days(-i64::from(retention_days));
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(day) = name
                .strip_prefix("rev-")
                .and_then(|n| n.strip_suffix(".bin"))
                .and_then(Date::parse_iso)
            else {
                continue;
            };
            if day < cutoff {
                std::fs::remove_file(entry.path())?;
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            if bytes.len() % 24 != 0 {
                return Err(std::io::Error::other(format!("{name}: corrupt shard")));
            }
            let mut map = HashMap::with_capacity(bytes.len() / 24);
            for c in bytes.as_chunks::<24>().0 {
                let k = u64::from_le_bytes(c[0..8].try_into().unwrap_or_default());
                let v = u64::from_le_bytes(c[8..16].try_into().unwrap_or_default());
                let r = u32::from_le_bytes(c[16..20].try_into().unwrap_or_default());
                map.insert(
                    k,
                    Entry {
                        value: v,
                        revision: r,
                    },
                );
            }
            shards.insert(day, map);
        }
        Ok(Self {
            dir: Some(dir),
            retention_days: i64::from(retention_days),
            shards,
            dirty: BTreeSet::new(),
        })
    }

    /// Records an observation of `value` under `key` for `day`.
    pub fn observe(&mut self, key: &str, value: &str, day: Date) -> Outcome {
        let k = h64(key);
        let v = h64(value);
        let shard = self.shards.entry(day).or_default();
        let outcome = match shard.get(&k) {
            Some(e) if e.value == v => return Outcome::Duplicate,
            Some(e) => Outcome::Emit(e.revision + 1),
            None => Outcome::Emit(0),
        };
        if let Outcome::Emit(r) = outcome {
            shard.insert(
                k,
                Entry {
                    value: v,
                    revision: r,
                },
            );
            self.dirty.insert(day);
        }
        outcome
    }

    /// The last revision recorded for `key` on `day`, if any.
    #[must_use]
    pub fn revision_of(&self, key: &str, day: Date) -> Option<u32> {
        self.shards.get(&day)?.get(&h64(key)).map(|e| e.revision)
    }

    /// Number of keys held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards.values().map(HashMap::len).sum()
    }

    /// True if no key is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Writes changed shards and drops expired ones.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self, today: Date) -> std::io::Result<()> {
        let cutoff = today.add_days(-self.retention_days.min(1_000_000));
        let expired: Vec<Date> = self.shards.range(..cutoff).map(|(d, _)| *d).collect();
        let Some(dir) = &self.dir else {
            self.dirty.clear();
            return Ok(());
        };
        for day in expired {
            self.shards.remove(&day);
            self.dirty.remove(&day);
            let p = shard_path(dir, day);
            if p.exists() {
                std::fs::remove_file(p)?;
            }
        }
        for day in std::mem::take(&mut self.dirty) {
            let Some(map) = self.shards.get(&day) else {
                continue;
            };
            let mut entries: Vec<(&u64, &Entry)> = map.iter().collect();
            entries.sort_by_key(|(k, _)| **k);
            let mut bytes = Vec::with_capacity(entries.len() * 24);
            for (k, e) in entries {
                bytes.extend_from_slice(&k.to_le_bytes());
                bytes.extend_from_slice(&e.value.to_le_bytes());
                bytes.extend_from_slice(&e.revision.to_le_bytes());
                bytes.extend_from_slice(&[0u8; 4]);
            }
            crate::raw::write_atomic_sync(&shard_path(dir, day), &bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let today = Date::new(2026, 3, 10).unwrap();
        let d = Date::new(2026, 3, 9).unwrap();
        let mut s = RevisionStore::open(dir.path(), 30, today).unwrap();
        assert_eq!(s.observe("k", "1", d), Outcome::Emit(0));
        assert_eq!(s.observe("k", "1", d), Outcome::Duplicate);
        assert_eq!(s.observe("k", "2", d), Outcome::Emit(1));
        assert_eq!(s.observe("k", "2", d), Outcome::Duplicate);
        assert_eq!(s.observe("k", "1", d), Outcome::Emit(2));
        s.flush(today).unwrap();
        let mut s = RevisionStore::open(dir.path(), 30, today).unwrap();
        assert_eq!(s.revision_of("k", d), Some(2));
        assert_eq!(s.observe("k", "1", d), Outcome::Duplicate);
        // Expiry.
        let later = today.add_days(40);
        s.flush(later).unwrap();
        assert!(s.is_empty());
        assert!(!dir.path().join("rev-2026-03-09.bin").exists());
    }
}
