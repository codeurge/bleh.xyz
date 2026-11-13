//! S3-compatible object storage (AWS S3, Cloudflare R2, MinIO) and local
//! directories, through the `object_store` crate, with write-once uploads.
//!
//! Write-once: data files and signed manifests are created with
//! `PutMode::Create` (`If-None-Match: *`, supported by S3, R2 and MinIO).
//! If the key exists, the existing object is compared by SHA-256: identical
//! content is accepted (uploads are idempotent), different content is an
//! [`Error::Conflict`] and is never overwritten. Corrections are new keys
//! (a new part number or a new archive version, archive-layout § 6). Only
//! the anchors file, the OpenTimestamps proof, `version-hint.text` and the
//! indexes are mutable.
//!
//! Configuration comes from the environment, per tier (`PUBLIC` or
//! `PRIVATE`):
//!
//! | Variable | Meaning |
//! |---|---|
//! | `BLEH_ARCHIVE_<TIER>_URL` | `s3://bucket/prefix`, `file:///abs/dir` or `memory://` |
//! | `BLEH_ARCHIVE_<TIER>_ENDPOINT` | S3 endpoint (R2: `https://<account>.r2.cloudflarestorage.com`; MinIO: `http://host:9000`) |
//! | `BLEH_ARCHIVE_<TIER>_REGION` | Region (default `auto`, which R2 expects) |
//! | `BLEH_ARCHIVE_<TIER>_ACCESS_KEY_ID`, `BLEH_ARCHIVE_<TIER>_SECRET_ACCESS_KEY` | Credentials (fall back to the standard `AWS_*` variables) |
//! | `BLEH_ARCHIVE_<TIER>_ALLOW_HTTP` | `true` to allow plain HTTP (local MinIO only) |
//! | `BLEH_ARCHIVE_<TIER>_PATH_STYLE` | `true` for path-style requests (MinIO); virtual-hosted otherwise |

use std::path::Path as FsPath;
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    Attribute, Attributes, ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload,
    WriteMultipart,
};
use sha2::{Digest, Sha256};

use mesh_types::Sha256Digest;

use crate::error::{Error, IoContext, Result};
use crate::layout;
use crate::metrics;

/// Files above this size are uploaded in parts (after an existence check).
pub const MULTIPART_THRESHOLD: u64 = 64 << 20;

/// Where a tier's objects live.
#[derive(Clone, PartialEq, Eq)]
pub struct StoreConfig {
    /// `s3://bucket/prefix`, `file:///dir` or `memory://`.
    pub url: String,
    /// S3 endpoint override.
    pub endpoint: Option<String>,
    /// S3 region.
    pub region: Option<String>,
    /// Access key ID (else `AWS_ACCESS_KEY_ID`).
    pub access_key_id: Option<String>,
    /// Secret access key (else `AWS_SECRET_ACCESS_KEY`).
    pub secret_access_key: Option<String>,
    /// Allow plain HTTP.
    pub allow_http: bool,
    /// Path-style requests.
    pub path_style: bool,
}

impl std::fmt::Debug for StoreConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreConfig")
            .field("url", &self.url)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("credentials", &self.access_key_id.as_ref().map(|_| "<set>"))
            .field("allow_http", &self.allow_http)
            .field("path_style", &self.path_style)
            .finish()
    }
}

impl StoreConfig {
    /// A configuration for `url` with everything else default.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            endpoint: None,
            region: None,
            access_key_id: None,
            secret_access_key: None,
            allow_http: false,
            path_style: false,
        }
    }

    /// Reads `BLEH_ARCHIVE_<TIER>_*` (see the module docs). Returns `None`
    /// if the URL variable is unset.
    #[must_use]
    pub fn from_env(tier: &str) -> Option<Self> {
        let var = |k: &str| {
            std::env::var(format!("BLEH_ARCHIVE_{tier}_{k}"))
                .ok()
                .filter(|v| !v.is_empty())
        };
        let flag = |k: &str| var(k).is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        Some(Self {
            url: var("URL")?,
            endpoint: var("ENDPOINT"),
            region: var("REGION"),
            access_key_id: var("ACCESS_KEY_ID"),
            secret_access_key: var("SECRET_ACCESS_KEY"),
            allow_http: flag("ALLOW_HTTP"),
            path_style: flag("PATH_STYLE"),
        })
    }

    /// Opens the store. Returns it with the key prefix from the URL.
    ///
    /// # Errors
    ///
    /// Fails on an unsupported URL or invalid S3 settings.
    pub fn open(&self) -> Result<Store> {
        if let Some(rest) = self.url.strip_prefix("s3://") {
            let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
            if bucket.is_empty() {
                return Err(Error::invalid("s3 URL without bucket"));
            }
            let mut b = AmazonS3Builder::from_env()
                .with_bucket_name(bucket)
                .with_region(self.region.clone().unwrap_or_else(|| "auto".into()))
                .with_allow_http(self.allow_http)
                .with_virtual_hosted_style_request(!self.path_style && self.endpoint.is_none());
            if let Some(e) = &self.endpoint {
                b = b.with_endpoint(e);
            }
            if let Some(k) = &self.access_key_id {
                b = b.with_access_key_id(k);
            }
            if let Some(s) = &self.secret_access_key {
                b = b.with_secret_access_key(s);
            }
            return Ok(Store::new(Arc::new(b.build()?), prefix));
        }
        if let Some(dir) = self.url.strip_prefix("file://") {
            std::fs::create_dir_all(dir).at(dir)?;
            return Ok(Store::new(
                Arc::new(LocalFileSystem::new_with_prefix(dir)?),
                "",
            ));
        }
        if self.url == "memory://" {
            return Ok(Store::new(Arc::new(InMemory::new()), ""));
        }
        Err(Error::invalid(format!(
            "unsupported store URL {:?}",
            self.url
        )))
    }
}

/// An object store with a key prefix.
#[derive(Debug, Clone)]
pub struct Store {
    inner: Arc<dyn ObjectStore>,
    prefix: String,
}

/// Result of a write-once put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// The object was created.
    Created,
    /// An identical object already existed.
    Identical,
}

impl Store {
    /// Wraps a store; `prefix` is prepended to every key.
    pub fn new(inner: Arc<dyn ObjectStore>, prefix: &str) -> Self {
        Self {
            inner,
            prefix: prefix.trim_matches('/').to_owned(),
        }
    }

    /// An in-memory store (tests and dry runs).
    #[must_use]
    pub fn memory() -> Self {
        Self::new(Arc::new(InMemory::new()), "")
    }

    /// The object key for `rel` (`<prefix>/<rel>`).
    ///
    /// # Errors
    ///
    /// Fails on a key that `object_store` rejects.
    pub fn key(&self, rel: &str) -> Result<Path> {
        let full = if self.prefix.is_empty() {
            rel.to_owned()
        } else {
            format!("{}/{rel}", self.prefix)
        };
        Path::parse(&full).map_err(|e| Error::invalid(format!("object key {full:?}: {e}")))
    }

    /// The underlying store.
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn ObjectStore> {
        &self.inner
    }

    /// Reads an object, or `None` if absent.
    ///
    /// # Errors
    ///
    /// Fails on storage errors other than not-found.
    pub async fn get(&self, rel: &str) -> Result<Option<Bytes>> {
        match self.inner.get(&self.key(rel)?).await {
            Ok(r) => Ok(Some(r.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn existing_sha256(&self, key: &Path) -> Result<Sha256Digest> {
        let mut stream = self.inner.get(key).await?.into_stream();
        let mut h = Sha256::new();
        while let Some(chunk) = stream.next().await {
            h.update(chunk?);
        }
        Ok(Sha256Digest(h.finalize().into()))
    }

    async fn compare_existing(
        &self,
        rel: &str,
        key: &Path,
        new: Sha256Digest,
    ) -> Result<PutOutcome> {
        let existing = self.existing_sha256(key).await?;
        if existing == new {
            metrics::inc("archive_store_put_identical_total", &[], 1);
            Ok(PutOutcome::Identical)
        } else {
            metrics::inc("archive_store_put_conflict_total", &[], 1);
            Err(Error::Conflict {
                key: rel.to_owned(),
                existing: existing.to_string(),
                new: new.to_string(),
            })
        }
    }

    /// Creates `rel` with `bytes` unless it exists; see the module docs.
    ///
    /// # Errors
    ///
    /// Fails on storage errors or a conflicting existing object.
    pub async fn put_once(&self, rel: &str, bytes: Bytes) -> Result<PutOutcome> {
        let key = self.key(rel)?;
        let sha = Sha256Digest::of(&bytes);
        let len = bytes.len() as u64;
        let opts = PutOptions {
            mode: PutMode::Create,
            ..PutOptions::default()
        };
        match self
            .inner
            .put_opts(&key, PutPayload::from(bytes), opts)
            .await
        {
            Ok(_) => {
                metrics::inc("archive_store_put_created_total", &[], 1);
                metrics::inc("archive_store_bytes_uploaded_total", &[], len);
                Ok(PutOutcome::Created)
            }
            Err(object_store::Error::AlreadyExists { .. }) => {
                self.compare_existing(rel, &key, sha).await
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Uploads a local file write-once. Large files go up in parts after an
    /// existence check (multipart uploads cannot be conditional).
    ///
    /// # Errors
    ///
    /// Fails on I/O or storage errors or a conflicting existing object.
    pub async fn put_file_once(&self, rel: &str, local: &FsPath) -> Result<PutOutcome> {
        let size = std::fs::metadata(local).at(local)?.len();
        if size <= MULTIPART_THRESHOLD {
            let bytes = Bytes::from(std::fs::read(local).at(local)?);
            return self.put_once(rel, bytes).await;
        }
        let key = self.key(rel)?;
        let sha = crate::digest::sha256_file(local)?;
        match self.inner.head(&key).await {
            Ok(_) => return self.compare_existing(rel, &key, sha).await,
            Err(object_store::Error::NotFound { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        let upload = self.inner.put_multipart(&key).await?;
        let mut w = WriteMultipart::new(upload);
        let mut f = std::fs::File::open(local).at(local)?;
        let mut buf = vec![0u8; 8 << 20];
        loop {
            let n = std::io::Read::read(&mut f, &mut buf).at(local)?;
            if n == 0 {
                break;
            }
            w.wait_for_capacity(4).await?;
            w.write(&buf[..n]);
        }
        w.finish().await?;
        metrics::inc("archive_store_put_created_total", &[], 1);
        metrics::inc("archive_store_bytes_uploaded_total", &[], size);
        Ok(PutOutcome::Created)
    }

    /// Writes a mutable object (anchors, proofs, indexes), replacing it.
    ///
    /// # Errors
    ///
    /// Fails on storage errors.
    pub async fn put_mutable(&self, rel: &str, bytes: Bytes) -> Result<()> {
        self.inner
            .put(&self.key(rel)?, PutPayload::from(bytes))
            .await?;
        Ok(())
    }

    /// Writes a mutable object with a `Content-Type` and `Cache-Control`.
    /// Stores that cannot keep attributes (local directories) get the bytes
    /// alone.
    ///
    /// # Errors
    ///
    /// Fails on storage errors.
    pub async fn put_mutable_with(
        &self,
        rel: &str,
        bytes: Bytes,
        content_type: &str,
        cache_control: &str,
    ) -> Result<()> {
        let key = self.key(rel)?;
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, content_type.to_owned().into());
        attributes.insert(Attribute::CacheControl, cache_control.to_owned().into());
        let opts = PutOptions {
            attributes,
            ..PutOptions::default()
        };
        match self
            .inner
            .put_opts(&key, PutPayload::from(bytes.clone()), opts)
            .await
        {
            Ok(_) => Ok(()),
            Err(object_store::Error::NotImplemented { .. }) => {
                self.inner.put(&key, PutPayload::from(bytes)).await?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// Result of uploading a staging directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UploadReport {
    /// Newly created objects.
    pub created: Vec<String>,
    /// Objects that already existed with identical content.
    pub identical: Vec<String>,
    /// Objects rejected because different content exists (files moved to
    /// `conflicts/`).
    pub conflicts: Vec<String>,
}

/// Uploads every sealed file of `archive` under `staging`
/// (`<staging>/<archive>/ready/**`) to `<archive>/<path>` in `store`, then
/// moves each to `uploaded/` (if `keep_local`) or deletes it. Conflicting
/// files are moved to `conflicts/` and reported, never overwritten.
///
/// # Errors
///
/// Fails on I/O or storage errors (conflicts are reported, not errors).
pub async fn upload_ready(
    staging: &FsPath,
    archive: &str,
    store: &Store,
    keep_local: bool,
) -> Result<UploadReport> {
    let base = staging.join(archive);
    let ready = base.join("ready");
    let mut report = UploadReport::default();
    for rel in layout::list_files(&ready)? {
        if rel.ends_with(".tmp") {
            continue;
        }
        let local = ready.join(&rel);
        let key = format!("{archive}/{rel}");
        let dest = match store.put_file_once(&key, &local).await {
            Ok(PutOutcome::Created) => {
                report.created.push(rel.clone());
                keep_local.then(|| base.join("uploaded").join(&rel))
            }
            Ok(PutOutcome::Identical) => {
                report.identical.push(rel.clone());
                keep_local.then(|| base.join("uploaded").join(&rel))
            }
            Err(Error::Conflict { existing, new, .. }) => {
                tracing::error!(
                    key,
                    existing,
                    new,
                    "write-once conflict; kept in conflicts/"
                );
                report.conflicts.push(rel.clone());
                Some(base.join("conflicts").join(&rel))
            }
            Err(e) => return Err(e),
        };
        match dest {
            Some(d) => {
                if let Some(parent) = d.parent() {
                    std::fs::create_dir_all(parent).at(parent)?;
                }
                std::fs::rename(&local, &d).at(&d)?;
            }
            None => std::fs::remove_file(&local).at(&local)?,
        }
    }
    Ok(report)
}

/// Uploads the listed files of a local archive root write-once (data files,
/// signed manifests), returning the outcome per path.
///
/// # Errors
///
/// Fails on the first I/O, storage or conflict error.
pub async fn upload_files(
    root: &FsPath,
    archive: &str,
    paths: &[String],
    store: &Store,
) -> Result<Vec<(String, PutOutcome)>> {
    let mut out = Vec::new();
    for rel in paths {
        let local = layout::Layout::new(root).file(archive, rel);
        let outcome = store
            .put_file_once(&format!("{archive}/{rel}"), &local)
            .await?;
        out.push((rel.clone(), outcome));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_once_semantics() {
        let s = Store::memory();
        assert_eq!(
            s.put_once("a/b", Bytes::from_static(b"x")).await.unwrap(),
            PutOutcome::Created
        );
        assert_eq!(
            s.put_once("a/b", Bytes::from_static(b"x")).await.unwrap(),
            PutOutcome::Identical
        );
        let err = s.put_once("a/b", Bytes::from_static(b"y")).await;
        assert!(matches!(err, Err(Error::Conflict { .. })), "{err:?}");
        assert_eq!(
            s.get("a/b").await.unwrap().unwrap(),
            Bytes::from_static(b"x")
        );
        s.put_mutable("a/b", Bytes::from_static(b"z"))
            .await
            .unwrap();
        assert_eq!(
            s.get("a/b").await.unwrap().unwrap(),
            Bytes::from_static(b"z")
        );
        assert_eq!(s.get("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn local_directory_store_and_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = StoreConfig::new(format!("file://{}", dir.path().display()));
        let s = cfg.open().unwrap();
        s.put_once("x/y.bin", Bytes::from_static(b"1"))
            .await
            .unwrap();
        assert_eq!(std::fs::read(dir.path().join("x/y.bin")).unwrap(), b"1");
        assert!(matches!(
            s.put_once("x/y.bin", Bytes::from_static(b"2")).await,
            Err(Error::Conflict { .. })
        ));
        let p = Store::new(Arc::new(InMemory::new()), "/pre/fix/");
        assert_eq!(p.key("a").unwrap().as_ref(), "pre/fix/a");
        assert!(StoreConfig::new("ftp://x").open().is_err());
        assert!(StoreConfig::new("s3:///nobucket").open().is_err());
    }

    #[tokio::test]
    async fn s3_config_builds_for_r2_and_minio() {
        let mut cfg = StoreConfig::new("s3://bucket/prefix");
        cfg.endpoint = Some("http://127.0.0.1:9000".into());
        cfg.allow_http = true;
        cfg.path_style = true;
        cfg.access_key_id = Some("test".into());
        cfg.secret_access_key = Some("test".into());
        let s = cfg.open().unwrap();
        assert_eq!(s.key("a").unwrap().as_ref(), "prefix/a");
        assert!(!format!("{cfg:?}").contains("test\""));
    }

    #[tokio::test]
    async fn uploads_ready_files() {
        let staging = tempfile::tempdir().unwrap();
        let ready = staging.path().join("arch/ready/stream=s/date=2026-01-01");
        std::fs::create_dir_all(&ready).unwrap();
        std::fs::write(ready.join("00.jsonl.zst"), b"a").unwrap();
        std::fs::write(ready.join("01.jsonl.zst"), b"b").unwrap();
        let store = Store::memory();
        store
            .put_once(
                "arch/stream=s/date=2026-01-01/01.jsonl.zst",
                Bytes::from_static(b"other"),
            )
            .await
            .unwrap();
        let r = upload_ready(staging.path(), "arch", &store, true)
            .await
            .unwrap();
        assert_eq!(r.created, vec!["stream=s/date=2026-01-01/00.jsonl.zst"]);
        assert_eq!(r.conflicts, vec!["stream=s/date=2026-01-01/01.jsonl.zst"]);
        assert!(
            staging
                .path()
                .join("arch/uploaded/stream=s/date=2026-01-01/00.jsonl.zst")
                .exists()
        );
        assert!(
            staging
                .path()
                .join("arch/conflicts/stream=s/date=2026-01-01/01.jsonl.zst")
                .exists()
        );
        assert!(
            layout::list_files(&staging.path().join("arch/ready"))
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn multipart_path_checks_existence() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        let data = vec![9u8; (MULTIPART_THRESHOLD + 10) as usize];
        std::fs::write(&p, &data).unwrap();
        let s = Store::memory();
        assert_eq!(
            s.put_file_once("big", &p).await.unwrap(),
            PutOutcome::Created
        );
        assert_eq!(
            s.put_file_once("big", &p).await.unwrap(),
            PutOutcome::Identical
        );
        assert_eq!(s.get("big").await.unwrap().unwrap().len(), data.len());
        std::fs::write(&p, b"small now").unwrap();
        assert!(matches!(
            s.put_file_once("big", &p).await,
            Err(Error::Conflict { .. })
        ));
    }
}
