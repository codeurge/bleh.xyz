//! Verbatim copy of admitted objects into the private raw tier.
//!
//! Each object is read from its requester-pays bucket and written,
//! byte for byte, to `<dest_prefix>/<bucket>/<key>` in the destination
//! store (any S3-compatible store, such as R2). Work runs with bounded
//! concurrency; each object is retried with backoff; completed objects are
//! appended to the [`Checkpoint`], so a rerun resumes where it stopped.
//!
//! The denylist is enforced again here, before any request for an object
//! is made, whatever the caller passes in.

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::{StreamExt, TryStreamExt};
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, WriteMultipart};
use serde::Serialize;
use sha2::{Digest, Sha256};

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
    /// Objects larger than this are streamed with a multipart upload
    /// instead of being buffered.
    pub multipart_threshold: u64,
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
            dry_run: false,
        }
    }
}

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
}

impl ObjectError {
    fn transient(&self) -> bool {
        match self {
            Self::Store(e) => object_store_transient(e),
            Self::Size { .. } | Self::Verify { .. } => true,
            Self::Policy(_) => false,
        }
    }
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

    async fn copy_once(
        &self,
        src: &dyn ObjectStore,
        obj: &SourceObject,
        dest: &Path,
    ) -> Result<String, ObjectError> {
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
            hasher.update(&bytes);
            self.dest.put(dest, PutPayload::from(bytes)).await?;
        } else {
            let upload = self.dest.put_multipart(dest).await?;
            let mut writer = WriteMultipart::new(upload);
            let mut stream = got.into_stream();
            loop {
                match stream.try_next().await {
                    Ok(Some(chunk)) => {
                        hasher.update(&chunk);
                        writer.wait_for_capacity(8).await?;
                        writer.put(chunk);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = writer.abort().await;
                        return Err(e.into());
                    }
                }
            }
            writer.finish().await?;
        }
        let written = self.dest.head(dest).await?.size;
        if written != read_size {
            return Err(ObjectError::Verify {
                src: read_size,
                dest: written,
            });
        }
        Ok(hex(&hasher.finalize()))
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
        let out = retry(self.cfg.retry, ObjectError::transient, || {
            self.copy_once(src.as_ref(), obj, &dest)
        })
        .await
        .map_err(|(e, attempts)| fail(e.to_string(), attempts))?;
        Ok(Copied {
            entry: CheckpointEntry {
                bucket: obj.bucket.clone(),
                key: obj.key.clone(),
                size: obj.size,
                etag: obj.etag.clone(),
                dest: dest.to_string(),
                sha256: out.value,
            },
            attempts: out.attempts,
        })
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
            dry_run: false,
        }
    }

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
