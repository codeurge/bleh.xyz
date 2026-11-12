//! Verbatim raw capture.
//!
//! Every response body that changed since the last poll is kept byte for
//! byte, before parsing, under
//!
//! ```text
//! <raw_dir>/<source>/<YYYY-MM-DD>/<target>.<HHMMSS>.<sha256 prefix>
//! <raw_dir>/<source>/<YYYY-MM-DD>/<target>.<HHMMSS>.<sha256 prefix>.meta.json
//! ```
//!
//! The sidecar records the URL, fetch time, size, SHA-256 and validators,
//! so `reference-pollers parse` reproduces the same records offline
//! (`ts_ingest_ns` and the publication time come from the sidecar).

use std::path::{Path, PathBuf};

use mesh_types::UnixNanos;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::http::Validators;
use crate::num::slug;
use crate::source::SourceId;
use crate::time::Date;

/// Sidecar metadata for one raw file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMeta {
    /// Sidecar format version (1).
    pub version: u32,
    /// Source name.
    pub source: String,
    /// Target key (names the series group the body is for).
    pub target: String,
    /// Parser of the reader that reads the body (the target's kind).
    pub kind: String,
    /// URL fetched.
    pub url: String,
    /// Fetch completion time, Unix nanoseconds, decimal string.
    pub fetched_at_ns: String,
    /// Bytes.
    pub bytes: u64,
    /// SHA-256, lowercase hex.
    pub sha256: String,
    /// Response validators.
    #[serde(default)]
    pub validators: Validators,
}

impl RawMeta {
    /// The fetch time.
    #[must_use]
    pub fn fetched_at(&self) -> Option<UnixNanos> {
        self.fetched_at_ns.parse().ok().map(UnixNanos)
    }
}

/// Lowercase hex SHA-256.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Path of the sidecar for `file`.
#[must_use]
pub fn sidecar_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_os_string();
    s.push(".meta.json");
    PathBuf::from(s)
}

/// Writes `body` to `path` via a temporary file, fsync and rename.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn write_atomic_sync(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent()
        && let Ok(d) = std::fs::File::open(parent)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The local raw directory.
#[derive(Debug, Clone)]
pub struct RawStore {
    dir: PathBuf,
}

impl RawStore {
    /// A store rooted at `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The root directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Saves a body and its sidecar; returns the body's path.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub fn save(
        &self,
        source: SourceId,
        target: &crate::sources::Target,
        body: &[u8],
        validators: &Validators,
        fetched_at: UnixNanos,
    ) -> std::io::Result<PathBuf> {
        let ms = fetched_at.to_millis().0;
        let day = Date::of_ms(ms);
        let secs = ms.rem_euclid(86_400_000) / 1000;
        let sha = sha256_hex(body);
        let mut name = slug(&target.key).to_ascii_lowercase();
        name.truncate(120);
        let file = format!(
            "{name}.{:02}{:02}{:02}.{}",
            secs / 3600,
            (secs / 60) % 60,
            secs % 60,
            &sha[..12]
        );
        let path = self
            .dir
            .join(source.name())
            .join(day.to_string())
            .join(file);
        write_atomic_sync(&path, body)?;
        let meta = RawMeta {
            version: 1,
            source: source.name().to_owned(),
            target: target.key.clone(),
            kind: target.kind.clone(),
            url: target.url.clone(),
            fetched_at_ns: fetched_at.0.to_string(),
            bytes: body.len() as u64,
            sha256: sha,
            validators: validators.clone(),
        };
        let json = serde_json::to_vec_pretty(&meta).map_err(std::io::Error::other)?;
        write_atomic_sync(&sidecar_path(&path), &json)?;
        Ok(path)
    }

    /// Reads a raw file and its sidecar.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors or a missing or malformed sidecar.
    pub fn load(path: &Path) -> std::io::Result<(Vec<u8>, RawMeta)> {
        let body = std::fs::read(path)?;
        let meta: RawMeta = serde_json::from_slice(&std::fs::read(sidecar_path(path))?)
            .map_err(std::io::Error::other)?;
        Ok((body, meta))
    }
}
