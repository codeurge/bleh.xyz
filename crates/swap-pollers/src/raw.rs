//! Verbatim raw capture.
//!
//! Every fetched report file is kept byte for byte, before parsing, under
//!
//! ```text
//! <raw_dir>/<repository>/<regime>/<slices|eod>/<YYYY-MM-DD>/<file name>
//! <raw_dir>/<repository>/<regime>/<slices|eod>/<YYYY-MM-DD>/<file name>.meta.json
//! ```
//!
//! The sidecar records where and when the file was fetched, its SHA-256 and
//! the listing entry, so replaying the raw directory reproduces the same
//! records (`ts_ingest_ns` and `seq.block_number` come from the sidecar).
//! Optionally the same keys are mirrored to S3-compatible storage.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::{Deserialize, Serialize};

use crate::listing::FileEntry;
use crate::metrics::Metrics;
use crate::parse::ReportKind;
use crate::repository::{DtccBucket, Repository};

/// Sidecar metadata for one raw file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawMeta {
    /// Sidecar format version.
    pub version: u32,
    /// Repository ID.
    pub repository: String,
    /// Bucket key (`CFTC_IR`).
    pub bucket: String,
    /// Report kind.
    pub kind: ReportKind,
    /// URL fetched.
    pub url: String,
    /// Fetch completion time, Unix nanoseconds, as a decimal string.
    pub fetched_at_ns: String,
    /// Bytes.
    pub bytes: u64,
    /// SHA-256, lowercase hex.
    pub sha256: String,
    /// Response `ETag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Response `Last-Modified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// The listing entry that pointed at the file.
    pub entry: FileEntry,
}

impl RawMeta {
    /// Reads the sidecar next to `file`, if any.
    #[must_use]
    pub fn load_for(file: &Path) -> Option<Self> {
        let bytes = std::fs::read(sidecar_path(file)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

/// Path of the sidecar for `file`.
#[must_use]
pub fn sidecar_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_os_string();
    s.push(".meta.json");
    PathBuf::from(s)
}

/// Local raw directory plus an optional object-store mirror.
#[derive(Debug, Clone)]
pub struct RawStore {
    dir: PathBuf,
    mirror: Option<Mirror>,
    metrics: Arc<Metrics>,
}

/// Object-store mirror of the raw directory.
#[derive(Clone)]
pub struct Mirror {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl std::fmt::Debug for Mirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mirror")
            .field("store", &self.store.to_string())
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl Mirror {
    /// A mirror writing under `prefix` in `store`.
    #[must_use]
    pub fn new(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self {
            store,
            prefix: prefix.into().trim_matches('/').to_string(),
        }
    }
}

impl RawStore {
    /// A store rooted at `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>, mirror: Option<Mirror>, metrics: Arc<Metrics>) -> Self {
        Self {
            dir: dir.into(),
            mirror,
            metrics,
        }
    }

    /// Relative key of a report file.
    #[must_use]
    pub fn relative(
        repo: Repository,
        bucket: DtccBucket,
        kind: ReportKind,
        entry: &FileEntry,
    ) -> String {
        let date = entry.name_date().unwrap_or_else(|| "undated".to_string());
        let kind_dir = match kind {
            ReportKind::Slice => "slices",
            ReportKind::Cumulative => "eod",
        };
        format!(
            "{}/{}/{kind_dir}/{date}/{}",
            repo.id_str(),
            bucket.regime.dtcc_path(),
            entry.file_name
        )
    }

    /// Local path of a report file.
    #[must_use]
    pub fn path_for(
        &self,
        repo: Repository,
        bucket: DtccBucket,
        kind: ReportKind,
        entry: &FileEntry,
    ) -> PathBuf {
        self.dir.join(Self::relative(repo, bucket, kind, entry))
    }

    /// Writes the sidecar for `file` atomically.
    ///
    /// # Errors
    ///
    /// Fails on I/O errors.
    pub async fn write_meta(&self, file: &Path, meta: &RawMeta) -> std::io::Result<()> {
        let body = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
        write_atomic(&sidecar_path(file), &body).await
    }

    /// Copies a captured file and its sidecar to the mirror, if configured.
    /// Failures are logged and counted; the local copy remains.
    pub async fn mirror(&self, file: &Path) {
        let Some(m) = &self.mirror else {
            return;
        };
        let Ok(rel) = file.strip_prefix(&self.dir) else {
            return;
        };
        for local in [file.to_path_buf(), sidecar_path(file)] {
            let rel_str = match local.strip_prefix(&self.dir) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => rel.to_string_lossy().replace('\\', "/"),
            };
            let key = if m.prefix.is_empty() {
                rel_str
            } else {
                format!("{}/{rel_str}", m.prefix)
            };
            let result = match tokio::fs::read(&local).await {
                Ok(bytes) => m
                    .store
                    .put(&ObjectPath::from(key.as_str()), bytes.into())
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            if let Err(e) = result {
                self.metrics
                    .inc("swap_raw_persist_errors_total", &[("store", "object")]);
                tracing::error!(%key, error = %e, "raw mirror upload failed; local copy kept");
            }
        }
    }
}

/// Writes `body` to `path` via a temporary file, fsync and rename.
///
/// # Errors
///
/// Fails on I/O errors.
pub async fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = tokio::fs::File::create(&tmp).await?;
    f.write_all(body).await?;
    f.flush().await?;
    f.sync_all().await?;
    drop(f);
    tokio::fs::rename(&tmp, path).await?;
    crate::http::sync_dir(path).await;
    Ok(())
}
