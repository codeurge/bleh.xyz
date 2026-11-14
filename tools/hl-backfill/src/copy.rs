//! Verbatim copy of admitted objects into the private raw tier.
//!
//! Each object is read from its requester-pays bucket and written,
//! byte for byte, to `<dest_prefix>/<bucket>/<key>` in the destination
//! store (any S3-compatible store, such as R2). Work runs with bounded
//! concurrency; completed objects are appended to the [`Checkpoint`], so a
//! rerun resumes where it stopped.
//!
//! The source buckets are requester-pays, so each object is read from the
//! source once per run whenever possible. Before reading anything, a run
//! writes, reads back and deletes a probe object in the destination
//! ([`Copier::preflight`]); a destination that cannot be written stops the
//! run before a paid request is made. Each object is then read once (with
//! retries only for source-side failures) into memory or, above
//! [`CopyConfig::multipart_threshold`], into a spool file in
//! [`CopyConfig::spool_dir`], and only the upload and its size check are
//! retried against the destination. Errors that cannot change on a retry
//! (permissions, bad configuration, most 4xx statuses) fail after one
//! attempt.
//!
//! The denylist is enforced again here, before any request for an object
//! is made, whatever the caller passes in.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, WriteMultipart};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::buckets::{Bucket, check_key};
use crate::checkpoint::{Checkpoint, CheckpointEntry, CheckpointError, Status};
use crate::listing::SourceObject;
use crate::retry::{RetryPolicy, object_store_transient, retry};

/// Copier settings.
#[derive(Debug, Clone)]
pub struct CopyConfig {
    /// Objects copied at once.
    pub concurrency: usize,
    /// Per-object retry policy.
    pub retry: RetryPolicy,
    /// Destination key prefix (for example `hyperliquid-archive-raw`); the
    /// source bucket name and key follow it.
    pub dest_prefix: String,
    /// Objects up to this size are held in memory between the source read
    /// and the upload; larger ones are spooled to a file in `spool_dir` and
    /// uploaded with a multipart upload.
    pub multipart_threshold: u64,
    /// Directory for spool files of large objects (the scratch working
    /// directory in production). Each file is removed once its object is
    /// uploaded or has failed.
    pub spool_dir: PathBuf,
    /// List and plan only; read and write nothing.
    pub dry_run: bool,
}

impl Default for CopyConfig {
    fn default() -> Self {
        Self {
            concurrency: 16,
            retry: RetryPolicy::default(),
            dest_prefix: "hyperliquid-archive-raw".into(),
            multipart_threshold: 64 * 1024 * 1024,
            spool_dir: PathBuf::from("."),
            dry_run: false,
        }
    }
}

/// Upload part size when uploading a spool file.
const SPOOL_CHUNK: usize = 8 * 1024 * 1024;

/// Key of the destination probe, under the destination prefix.
const PROBE_NAME: &str = "_bleh-write-probe";

/// Errors that stop a copy run (per-object failures are reported in
/// [`CopyReport::failed`] instead).
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    /// The checkpoint could not be written.
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
    /// No store was configured for a bucket.
    #[error("no source store configured for bucket {0}")]
    NoSource(String),
    /// The destination failed the write probe; nothing was read from the
    /// source.
    #[error(
        "destination is not writable ({probe}): {error}; nothing was read from the source buckets"
    )]
    Destination {
        /// Probe object key.
        probe: String,
        /// Last error.
        error: String,
    },
}

/// Why one object failed.
#[derive(Debug, thiserror::Error)]
enum ObjectError {
    #[error("refused by policy: {0}")]
    Policy(#[from] crate::buckets::PolicyError),
    #[error("{0}")]
    Store(#[from] object_store::Error),
    #[error("size mismatch: listed {listed}, read {read}")]
    Size { listed: u64, read: u64 },
    #[error("destination size {dest} differs from source size {src}")]
    Verify { src: u64, dest: u64 },
    #[error("spool file {path}: {source}")]
    Spool {
        path: String,
        source: std::io::Error,
    },
}

impl ObjectError {
    fn transient(&self) -> bool {
        match self {
            Self::Store(e) => object_store_transient(e),
            Self::Size { .. } | Self::Verify { .. } => true,
            // A local disk problem (full, permissions) does not go away on
            // a retry.
            Self::Policy(_) | Self::Spool { .. } => false,
        }
    }

    fn spool(path: &std::path::Path, source: std::io::Error) -> Self {
        Self::Spool {
            path: path.display().to_string(),
            source,
        }
    }
}

/// An object read from the source, waiting to be uploaded.
enum Staged {
    /// At or below the multipart threshold: held in memory.
    Memory(Bytes),
    /// Above it: spooled to this file.
    File(PathBuf),
}

/// One failed object.
#[derive(Debug, Clone, Serialize)]
pub struct FailedObject {
    /// Bucket.
    pub bucket: String,
    /// Key.
    pub key: String,
    /// Last error.
    pub error: String,
    /// Attempts made.
    pub attempts: u32,
}

/// Summary of a copy run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CopyReport {
    /// Objects selected.
    pub selected: u64,
    /// Objects copied in this run.
    pub copied: u64,
    /// Bytes copied in this run.
    pub bytes_copied: u64,
    /// Objects skipped because the checkpoint already has them.
    pub already_done: u64,
    /// Objects copied again because their size or ETag changed.
    pub recopied_changed: u64,
    /// Objects that needed more than one attempt.
    pub retried: u64,
    /// Objects refused by the denylist (never read).
    pub refused: u64,
    /// Objects that failed after all retries.
    pub failed: Vec<FailedObject>,
    /// Per-shard copied object counts (`YYYY-MM-DD/HH`, `YYYY-MM-DD` or
    /// height range).
    pub shards: BTreeMap<String, u64>,
}

/// Copies objects from the source buckets into the destination store.
#[derive(Debug)]
pub struct Copier {
    sources: BTreeMap<Bucket, Arc<dyn ObjectStore>>,
    dest: Arc<dyn ObjectStore>,
    cfg: CopyConfig,
}

/// The destination key for a source object: `<prefix>/<bucket>/<key>`,
/// preserving the key's segments exactly.
///
/// # Errors
///
/// Fails if the key is not a valid object path.
pub fn dest_key(prefix: &str, bucket: &str, key: &str) -> Result<Path, object_store::path::Error> {
    let prefix = prefix.trim_matches('/');
    let joined = if prefix.is_empty() {
        format!("{bucket}/{key}")
    } else {
        format!("{prefix}/{bucket}/{key}")
    };
    Path::parse(joined)
}

struct Copied {
    entry: CheckpointEntry,
    attempts: u32,
}

impl Copier {
    /// A copier with one store per source bucket.
    #[must_use]
    pub fn new(
        sources: BTreeMap<Bucket, Arc<dyn ObjectStore>>,
        dest: Arc<dyn ObjectStore>,
        cfg: CopyConfig,
    ) -> Self {
        Self { sources, dest, cfg }
    }

    /// The spool file for a destination key (named by the key's SHA-256,
    /// so concurrent objects never share one).
    fn spool_path(&self, dest: &Path) -> PathBuf {
        self.cfg
            .spool_dir
            .join(".copy-spool")
            .join(format!("{}.part", hex(&Sha256::digest(dest.as_ref()))))
    }

    /// Reads one object from the source and stages it: in memory up to the
    /// multipart threshold, otherwise in its spool file. Returns the staged
    /// object and the SHA-256 of its bytes.
    async fn read_source(
        &self,
        src: &dyn ObjectStore,
        obj: &SourceObject,
        spool: &std::path::Path,
    ) -> Result<(Staged, String), ObjectError> {
        let location = Path::parse(&obj.key).map_err(object_store::Error::from)?;
        let got = src.get(&location).await?;
        let read_size = got.meta.size;
        if read_size != obj.size {
            return Err(ObjectError::Size {
                listed: obj.size,
                read: read_size,
            });
        }
        let mut hasher = Sha256::new();
        if read_size <= self.cfg.multipart_threshold {
            let bytes = got.bytes().await?;
            if bytes.len() as u64 != read_size {
                return Err(ObjectError::Size {
                    listed: obj.size,
                    read: bytes.len() as u64,
                });
            }
            hasher.update(&bytes);
            return Ok((Staged::Memory(bytes), hex(&hasher.finalize())));
        }
        if let Some(dir) = spool.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| ObjectError::spool(dir, e))?;
        }
        let mut file = tokio::fs::File::create(spool)
            .await
            .map_err(|e| ObjectError::spool(spool, e))?;
        let mut stream = got.into_stream();
        let mut written = 0u64;
        while let Some(chunk) = stream.try_next().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| ObjectError::spool(spool, e))?;
            written += chunk.len() as u64;
        }
        file.sync_all()
            .await
            .map_err(|e| ObjectError::spool(spool, e))?;
        if written != read_size {
            return Err(ObjectError::Size {
                listed: obj.size,
                read: written,
            });
        }
        Ok((Staged::File(spool.to_owned()), hex(&hasher.finalize())))
    }

    /// Uploads a staged object and checks the destination size. Retrying
    /// this never touches the source.
    async fn write_dest(&self, dest: &Path, staged: &Staged, size: u64) -> Result<(), ObjectError> {
        match staged {
            Staged::Memory(bytes) => {
                self.dest.put(dest, PutPayload::from(bytes.clone())).await?;
            }
            Staged::File(path) => {
                let mut file = tokio::fs::File::open(path)
                    .await
                    .map_err(|e| ObjectError::spool(path, e))?;
                let upload = self.dest.put_multipart(dest).await?;
                let mut writer = WriteMultipart::new_with_chunk_size(upload, SPOOL_CHUNK);
                let mut buf = vec![0u8; SPOOL_CHUNK];
                loop {
                    let n = match file.read(&mut buf).await {
                        Ok(n) => n,
                        Err(e) => {
                            let _ = writer.abort().await;
                            return Err(ObjectError::spool(path, e));
                        }
                    };
                    if n == 0 {
                        break;
                    }
                    if let Err(e) = writer.wait_for_capacity(8).await {
                        let _ = writer.abort().await;
                        return Err(e.into());
                    }
                    writer.put(Bytes::copy_from_slice(&buf[..n]));
                }
                writer.finish().await?;
            }
        }
        let written = self.dest.head(dest).await?.size;
        if written != size {
            return Err(ObjectError::Verify {
                src: size,
                dest: written,
            });
        }
        Ok(())
    }

    async fn copy_object(&self, obj: &SourceObject) -> Result<Copied, FailedObject> {
        let fail = |error: String, attempts| FailedObject {
            bucket: obj.bucket.clone(),
            key: obj.key.clone(),
            error,
            attempts,
        };
        // Hard enforcement: nothing excluded is ever requested.
        if let Err(e) = check_key(&obj.bucket, &obj.key) {
            return Err(fail(ObjectError::from(e).to_string(), 0));
        }
        let bucket =
            Bucket::from_name(&obj.bucket).ok_or_else(|| fail("unknown bucket".into(), 0))?;
        let src = self
            .sources
            .get(&bucket)
            .ok_or_else(|| fail(CopyError::NoSource(obj.bucket.clone()).to_string(), 0))?;
        let dest = dest_key(&self.cfg.dest_prefix, &obj.bucket, &obj.key)
            .map_err(|e| fail(e.to_string(), 0))?;
        let spool = self.spool_path(&dest);
        // Read once (the source is requester-pays), retrying only source
        // failures; then retry the upload from the staged copy. Attempts
        // count every request round: one read plus one upload is 1.
        let read = retry(self.cfg.retry, ObjectError::transient, || {
            self.read_source(src.as_ref(), obj, &spool)
        })
        .await;
        let read = match read {
            Ok(r) => r,
            Err((e, attempts)) => {
                let _ = std::fs::remove_file(&spool);
                return Err(fail(e.to_string(), attempts));
            }
        };
        let (staged, sha256) = read.value;
        let wrote = retry(self.cfg.retry, ObjectError::transient, || {
            self.write_dest(&dest, &staged, obj.size)
        })
        .await;
        if let Staged::File(path) = &staged {
            let _ = std::fs::remove_file(path);
        }
        let wrote =
            wrote.map_err(|(e, attempts)| fail(e.to_string(), read.attempts + attempts - 1))?;
        Ok(Copied {
            entry: CheckpointEntry {
                bucket: obj.bucket.clone(),
                key: obj.key.clone(),
                size: obj.size,
                etag: obj.etag.clone(),
                dest: dest.to_string(),
                sha256,
            },
            attempts: read.attempts + wrote.attempts - 1,
        })
    }

    /// Checks that the destination accepts writes before anything is read
    /// from the requester-pays source: writes a small probe object under
    /// the destination prefix, reads its size back and deletes it.
    ///
    /// # Errors
    ///
    /// [`CopyError::Destination`] if any step fails (transient errors are
    /// retried first).
    pub async fn preflight(&self) -> Result<(), CopyError> {
        let prefix = self.cfg.dest_prefix.trim_matches('/');
        let probe = if prefix.is_empty() {
            PROBE_NAME.to_owned()
        } else {
            format!("{prefix}/{PROBE_NAME}")
        };
        let fail = |error: String| CopyError::Destination {
            probe: probe.clone(),
            error,
        };
        let path = Path::parse(&probe).map_err(|e| fail(e.to_string()))?;
        let body = Bytes::from_static(b"bleh.xyz hl-backfill write probe\n");
        retry(self.cfg.retry, ObjectError::transient, || async {
            self.write_dest(&path, &Staged::Memory(body.clone()), body.len() as u64)
                .await
        })
        .await
        .map_err(|(e, _)| fail(e.to_string()))?;
        retry(self.cfg.retry, ObjectError::transient, || async {
            self.dest.delete(&path).await.map_err(ObjectError::from)
        })
        .await
        .map_err(|(e, _)| fail(e.to_string()))?;
        Ok(())
    }

    /// Copies every object not already in the checkpoint.
    ///
    /// # Errors
    ///
    /// Fails only if the checkpoint cannot be written; per-object failures
    /// are listed in the report.
    pub async fn copy_all(
        &self,
        objects: &[SourceObject],
        checkpoint: &mut Checkpoint,
    ) -> Result<CopyReport, CopyError> {
        let mut report = CopyReport {
            selected: objects.len() as u64,
            ..CopyReport::default()
        };
        let mut todo = Vec::new();
        for obj in objects {
            if check_key(&obj.bucket, &obj.key).is_err() {
                report.refused += 1;
                tracing::warn!(bucket = %obj.bucket, key = %obj.key, "refused by ADM-0003 denylist");
                continue;
            }
            match checkpoint.status(&obj.bucket, &obj.key, obj.size, obj.etag.as_deref()) {
                Status::Done => report.already_done += 1,
                Status::Changed => {
                    report.recopied_changed += 1;
                    todo.push(obj);
                }
                Status::New => todo.push(obj),
            }
        }
        if self.cfg.dry_run {
            return Ok(report);
        }
        if !todo.is_empty() {
            self.preflight().await?;
        }
        let mut results = futures::stream::iter(todo)
            .map(|obj| async move { (obj, self.copy_object(obj).await) })
            .buffer_unordered(self.cfg.concurrency.max(1));
        while let Some((obj, result)) = results.next().await {
            match result {
                Ok(done) => {
                    if done.attempts > 1 {
                        report.retried += 1;
                    }
                    report.copied += 1;
                    report.bytes_copied += obj.size;
                    *report.shards.entry(obj.info.shard_label()).or_default() += 1;
                    tracing::info!(key = %obj.key, size = obj.size, "copied");
                    checkpoint.record(done.entry)?;
                }
                Err(f) => {
                    tracing::error!(key = %f.key, error = %f.error, attempts = f.attempts, "copy failed");
                    report.failed.push(f);
                }
            }
        }
        checkpoint.sync()?;
        report.failed.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(report)
    }
}

/// Lowercase hex.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buckets::{Dataset, denied_prefix};
    use crate::listing::tests::{synthetic_archive_bucket, synthetic_node_bucket};
    use crate::listing::{Selection, list_dataset};
    use crate::testutil::RecordingStore;

    struct Fixture {
        node: RecordingStore,
        archive: RecordingStore,
        dest: RecordingStore,
        copier: Copier,
    }

    async fn fixture(cfg: CopyConfig) -> Fixture {
        let node = synthetic_node_bucket().await;
        let archive = synthetic_archive_bucket().await;
        let dest = RecordingStore::new();
        let mut sources: BTreeMap<Bucket, Arc<dyn ObjectStore>> = BTreeMap::new();
        sources.insert(Bucket::NodeData, Arc::new(node.clone()));
        sources.insert(Bucket::Archive, Arc::new(archive.clone()));
        let copier = Copier::new(sources, Arc::new(dest.clone()), cfg);
        Fixture {
            node,
            archive,
            dest,
            copier,
        }
    }

    async fn list_all(f: &Fixture) -> Vec<SourceObject> {
        let mut out = Vec::new();
        for d in Dataset::ALL {
            let store: &dyn ObjectStore = match d.bucket() {
                Bucket::NodeData => &f.node,
                Bucket::Archive => &f.archive,
            };
            out.extend(
                list_dataset(store, d, &Selection::default())
                    .await
                    .unwrap()
                    .objects,
            );
        }
        out
    }

    fn cfg() -> CopyConfig {
        CopyConfig {
            concurrency: 4,
            retry: RetryPolicy::immediate(4),
            dest_prefix: "raw".into(),
            multipart_threshold: 45,
            spool_dir: crate::testutil::tempdir(),
            dry_run: false,
        }
    }

    fn spool_is_empty(cfg: &CopyConfig) -> bool {
        let dir = cfg.spool_dir.join(".copy-spool");
        !dir.exists() || std::fs::read_dir(dir).unwrap().next().is_none()
    }

    const FILLS: &str = "node_fills_by_block/hourly/20250727/0.lz4";

    #[tokio::test]
    async fn copies_verbatim_and_resumes() {
        let f = fixture(cfg()).await;
        let objects = list_all(&f).await;
        assert_eq!(objects.len(), 14);
        let mut ckpt = Checkpoint::in_memory();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!(r.copied, 14, "{r:?}");
        assert!(r.failed.is_empty());
        for o in &objects {
            let src = f
                .node_or_archive(o)
                .get(&Path::parse(&o.key).unwrap())
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            let dst = f
                .dest
                .get(&dest_key("raw", &o.bucket, &o.key).unwrap())
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            assert_eq!(src, dst, "{}", o.key);
            let e = ckpt.get(&o.bucket, &o.key).unwrap();
            assert_eq!(e.sha256, hex(&Sha256::digest(&src)));
        }
        // Large objects went through the spool, which is now empty, and the
        // write probe was removed.
        assert!(objects.iter().any(|o| o.size > 45));
        assert!(spool_is_empty(&f.copier.cfg));
        assert!(
            f.dest
                .head(&Path::parse("raw/_bleh-write-probe").unwrap())
                .await
                .is_err()
        );
        // Second run copies nothing.
        f.dest.clear_log();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!((r.copied, r.already_done), (0, 14));
        assert!(f.dest.log().is_empty());
    }

    #[tokio::test]
    async fn never_touches_excluded_prefixes() {
        let f = fixture(cfg()).await;
        let mut objects = list_all(&f).await;
        // Even if a caller injects excluded keys, they are refused unread.
        for (bucket, key) in [
            ("hyperliquid-archive", "asset_ctxs/20230916.csv.lz4"),
            (
                "hl-mainnet-node-data",
                "misc_events_by_block/hourly/20250727/0.lz4",
            ),
        ] {
            objects.push(SourceObject {
                dataset: Dataset::NodeFills,
                bucket: bucket.into(),
                key: key.into(),
                size: 10,
                etag: None,
                info: Default::default(),
            });
        }
        let mut ckpt = Checkpoint::in_memory();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!(r.refused, 2);
        assert_eq!(r.copied, 14);
        for s in [&f.node, &f.archive, &f.dest] {
            for entry in s.log() {
                assert!(denied_prefix(&entry).is_none(), "touched {entry}");
            }
        }
    }

    #[tokio::test]
    async fn retries_transient_failures_and_reports_permanent_ones() {
        let f = fixture(cfg()).await;
        let objects = list_all(&f).await;
        f.node
            .fail_next_reads("node_fills_by_block/hourly/20250727/0.lz4", 2);
        f.node
            .fail_next_reads("node_trades/hourly/20250401/7.lz4", 5);
        let mut ckpt = Checkpoint::in_memory();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!(r.copied, 13);
        assert_eq!(r.retried, 1);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].attempts, 4);
        // The failed object is picked up by the next run.
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!((r.copied, r.already_done), (1, 13));
    }

    #[tokio::test]
    async fn destination_failures_never_repeat_the_source_read() {
        let f = fixture(cfg()).await;
        let objects = list_all(&f).await;
        let big = objects
            .iter()
            .find(|o| o.size > 45 && o.bucket == Bucket::NodeData.name())
            .unwrap();
        let small = objects
            .iter()
            .find(|o| o.size <= 45 && o.bucket == Bucket::NodeData.name())
            .unwrap();
        for o in [big, small] {
            let d = dest_key("raw", &o.bucket, &o.key).unwrap();
            f.dest.fail_next_writes(d.as_ref(), 2);
        }
        f.node.clear_log();
        let mut ckpt = Checkpoint::in_memory();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert!(r.failed.is_empty(), "{r:?}");
        assert_eq!(r.copied, 14);
        assert_eq!(r.retried, 2);
        for o in [big, small] {
            // Read from the source once, written three times.
            assert_eq!(f.node.count("get", &o.key), 1, "{}", o.key);
            let d = dest_key("raw", &o.bucket, &o.key).unwrap();
            let op = if o.size > 45 { "put_multipart" } else { "put" };
            assert_eq!(f.dest.count(op, d.as_ref()), 3, "{}", o.key);
        }
        assert!(spool_is_empty(&f.copier.cfg));
    }

    #[tokio::test]
    async fn unwritable_destination_stops_before_any_source_read() {
        let f = fixture(cfg()).await;
        let objects = list_all(&f).await;
        f.dest.deny_writes("raw/_bleh-write-probe");
        f.node.clear_log();
        f.archive.clear_log();
        let mut ckpt = Checkpoint::in_memory();
        let err = f.copier.copy_all(&objects, &mut ckpt).await.unwrap_err();
        assert!(matches!(err, CopyError::Destination { .. }), "{err}");
        assert!(err.to_string().contains("nothing was read"));
        // Permanent: one probe attempt, and no source request at all.
        assert_eq!(f.dest.count("put", "raw/_bleh-write-probe"), 1);
        assert!(f.node.log().is_empty(), "{:?}", f.node.log());
        assert!(f.archive.log().is_empty(), "{:?}", f.archive.log());
    }

    #[tokio::test]
    async fn permanent_errors_are_attempted_once() {
        let f = fixture(cfg()).await;
        let objects = list_all(&f).await;
        let o = objects.iter().find(|o| o.key == FILLS).unwrap();
        let d = dest_key("raw", &o.bucket, &o.key).unwrap();
        f.dest.deny_writes(d.as_ref());
        f.node.clear_log();
        let mut ckpt = Checkpoint::in_memory();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!(r.copied, 13);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].key, FILLS);
        assert_eq!(r.failed[0].attempts, 1, "{:?}", r.failed[0]);
        assert_eq!(f.node.count("get", FILLS), 1);
        assert!(spool_is_empty(&f.copier.cfg));
    }

    #[tokio::test]
    async fn recopies_changed_objects_and_supports_dry_run() {
        let f = fixture(CopyConfig {
            dry_run: true,
            ..cfg()
        })
        .await;
        let mut objects = list_all(&f).await;
        let mut ckpt = Checkpoint::in_memory();
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!((r.selected, r.copied), (14, 0));
        assert!(f.dest.log().is_empty());

        let f = fixture(cfg()).await;
        f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        objects[0].etag = Some("changed".into());
        let r = f.copier.copy_all(&objects, &mut ckpt).await.unwrap();
        assert_eq!((r.recopied_changed, r.copied), (1, 1));
    }

    impl Fixture {
        fn node_or_archive(&self, o: &SourceObject) -> &RecordingStore {
            if o.bucket == Bucket::Archive.name() {
                &self.archive
            } else {
                &self.node
            }
        }
    }
}
