//! Durable dedupe of emitted prints by repository dissemination ID.
//!
//! The same message can arrive twice: once in a live slice and again in the
//! end-of-day cumulative report, or again after a crash between emitting and
//! checkpointing. Each emitted message's key (a 64-bit digest of regime,
//! repository and dissemination ID) is kept in memory per dissemination day
//! and appended to `seen-<YYYY-MM-DD>.bin` (8 bytes per key, little
//! endian) on [`SeenStore::flush`]. Callers flush their record output first,
//! so a key reaches disk only after its record: a crash can repeat a record
//! (at-least-once) but never lose one. Days older than the window are
//! dropped from memory and disk.

use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Digest of a print's identity.
#[must_use]
pub fn key(venue: &str, repository: &str, dissemination_id: &str) -> u64 {
    let mut h = Sha256::new();
    h.update(venue.as_bytes());
    h.update([0]);
    h.update(repository.as_bytes());
    h.update([0]);
    h.update(dissemination_id.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    u64::from_le_bytes(b)
}

#[derive(Debug)]
struct Day {
    keys: HashSet<u64>,
    pending: Vec<u64>,
    file: File,
}

/// Result of checking a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// First time: recorded now.
    New,
    /// Already emitted.
    Duplicate,
    /// The day is older than the window; not tracked.
    OutOfWindow,
}

/// Per-day sets of emitted keys, persisted.
#[derive(Debug)]
pub struct SeenStore {
    dir: PathBuf,
    window_days: u32,
    days: BTreeMap<String, Day>,
}

fn day_number(day: &str) -> Option<i64> {
    let b = day.as_bytes();
    if b.len() != 10 {
        return None;
    }
    let y: i64 = day[..4].parse().ok()?;
    let m: i64 = day[5..7].parse().ok()?;
    let d: i64 = day[8..].parse().ok()?;
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The UTC day of a millisecond timestamp.
#[must_use]
pub fn day_of(ms: i64) -> String {
    mesh_types::UnixMillis(ms).to_rfc3339()[..10].to_string()
}

impl SeenStore {
    /// Opens the store in `dir`, loading days within `window_days` of
    /// `today` and deleting older day files.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn open(dir: impl Into<PathBuf>, window_days: u32, today: &str) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let mut store = Self {
            dir,
            window_days,
            days: BTreeMap::new(),
        };
        let today_n = day_number(today).unwrap_or(0);
        for entry in std::fs::read_dir(&store.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(day) = name
                .strip_prefix("seen-")
                .and_then(|n| n.strip_suffix(".bin"))
            else {
                continue;
            };
            match day_number(day) {
                Some(n) if today_n - n < i64::from(window_days) && n <= today_n + 1 => {
                    store.load_day(day)?;
                }
                _ => {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Ok(store)
    }

    fn day_path(&self, day: &str) -> PathBuf {
        self.dir.join(format!("seen-{day}.bin"))
    }

    fn load_day(&mut self, day: &str) -> std::io::Result<&mut Day> {
        if !self.days.contains_key(day) {
            let path = self.day_path(day);
            let mut keys = HashSet::new();
            if let Ok(mut f) = File::open(&path) {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                // A torn final write leaves a partial key; ignore it.
                for chunk in buf.as_chunks::<8>().0 {
                    keys.insert(u64::from_le_bytes(*chunk));
                }
                let whole = buf.len() - buf.len() % 8;
                if whole != buf.len() {
                    let f = OpenOptions::new().write(true).open(&path)?;
                    f.set_len(whole as u64)?;
                }
            }
            let file = OpenOptions::new().create(true).append(true).open(&path)?;
            self.days.insert(
                day.to_string(),
                Day {
                    keys,
                    pending: Vec::new(),
                    file,
                },
            );
        }
        self.days
            .get_mut(day)
            .ok_or_else(|| std::io::Error::other("day vanished"))
    }

    fn in_window(&self, day: &str, today: &str) -> bool {
        match (day_number(day), day_number(today)) {
            (Some(d), Some(t)) => t - d < i64::from(self.window_days) && d <= t + 1,
            _ => false,
        }
    }

    /// Checks `key` for dissemination day `day` and records it if new.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn check_and_insert(&mut self, key: u64, day: &str, today: &str) -> std::io::Result<Seen> {
        if !self.in_window(day, today) {
            return Ok(Seen::OutOfWindow);
        }
        let d = self.load_day(day)?;
        if !d.keys.insert(key) {
            return Ok(Seen::Duplicate);
        }
        d.pending.push(key);
        Ok(Seen::New)
    }

    /// True if `key` was emitted on any day in memory.
    #[must_use]
    pub fn contains_any(&self, key: u64) -> bool {
        self.days.values().any(|d| d.keys.contains(&key))
    }

    /// True if `day` is inside the window relative to `today`.
    #[must_use]
    pub fn covers(&self, day: &str, today: &str) -> bool {
        self.in_window(day, today)
    }

    /// Flushes appended keys to disk and drops days outside the window.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn flush(&mut self, today: &str) -> std::io::Result<()> {
        for d in self.days.values_mut() {
            if d.pending.is_empty() {
                continue;
            }
            let buf: Vec<u8> = d.pending.iter().flat_map(|k| k.to_le_bytes()).collect();
            d.file.write_all(&buf)?;
            d.file.sync_data()?;
            d.pending.clear();
        }
        let stale: Vec<String> = self
            .days
            .keys()
            .filter(|day| !self.in_window(day, today))
            .cloned()
            .collect();
        for day in stale {
            self.days.remove(&day);
            let _ = std::fs::remove_file(self.day_path(&day));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let k = key("cftc-part43", "dtcc-ddr", "123");
        assert_ne!(k, key("sec-sbsr", "dtcc-ddr", "123"));
        {
            let mut s = SeenStore::open(dir.path(), 3, "2026-11-12").unwrap();
            assert_eq!(
                s.check_and_insert(k, "2026-11-12", "2026-11-12").unwrap(),
                Seen::New
            );
            assert_eq!(
                s.check_and_insert(k, "2026-11-12", "2026-11-12").unwrap(),
                Seen::Duplicate
            );
            assert_eq!(
                s.check_and_insert(k, "2026-11-01", "2026-11-12").unwrap(),
                Seen::OutOfWindow
            );
            s.flush("2026-11-12").unwrap();
        }
        let mut s = SeenStore::open(dir.path(), 3, "2026-11-13").unwrap();
        assert!(s.contains_any(k));
        assert_eq!(
            s.check_and_insert(k, "2026-11-12", "2026-11-13").unwrap(),
            Seen::Duplicate
        );
        drop(s);
        let s = SeenStore::open(dir.path(), 3, "2026-11-20").unwrap();
        assert!(!s.contains_any(k));
        assert!(!dir.path().join("seen-2026-11-12.bin").exists());
    }

    #[test]
    fn days() {
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("2026-11-12"), Some(20_769));
        assert_eq!(day_of(1_794_441_600_000), "2026-11-12");
    }
}
