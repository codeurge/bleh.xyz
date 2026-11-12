//! Test helpers: temporary directories, LZ4 framing, and an object store
//! wrapper that records every path touched and injects failures.
//!
//! All data written by tests is synthetic.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    ObjectStoreExt, PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

/// A fresh, empty temporary directory.
pub(crate) fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "hl-backfill-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// LZ4 frame compression, as written by the `lz4` command line tool.
pub(crate) fn lz4(data: &[u8]) -> Vec<u8> {
    let mut enc = lz4_flex::frame::FrameEncoder::new(Vec::new());
    enc.write_all(data).unwrap();
    enc.finish().unwrap()
}

/// Puts `data` at `key`.
pub(crate) async fn put(store: &dyn ObjectStore, key: &str, data: Vec<u8>) {
    store
        .put(&Path::parse(key).unwrap(), PutPayload::from(data))
        .await
        .unwrap();
}

/// Wraps a store: records every path read, listed or written, and fails
/// the first `n` reads of chosen paths with a transient error.
#[derive(Debug, Clone)]
pub(crate) struct RecordingStore {
    inner: Arc<dyn ObjectStore>,
    log: Arc<Mutex<Vec<String>>>,
    fail_reads: Arc<Mutex<HashMap<String, u32>>>,
}

impl RecordingStore {
    pub(crate) fn new() -> Self {
        Self::wrap(Arc::new(InMemory::new()))
    }

    pub(crate) fn wrap(inner: Arc<dyn ObjectStore>) -> Self {
        Self {
            inner,
            log: Arc::default(),
            fail_reads: Arc::default(),
        }
    }

    /// Every path touched, as `op:path`.
    pub(crate) fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    pub(crate) fn clear_log(&self) {
        self.log.lock().unwrap().clear();
    }

    pub(crate) fn fail_next_reads(&self, key: &str, n: u32) {
        self.fail_reads.lock().unwrap().insert(key.to_owned(), n);
    }

    fn note(&self, op: &str, path: Option<&Path>) {
        let p = path.map(ToString::to_string).unwrap_or_default();
        self.log.lock().unwrap().push(format!("{op}:{p}"));
    }
}

impl fmt::Display for RecordingStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RecordingStore({})", self.inner)
    }
}

#[async_trait]
impl ObjectStore for RecordingStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.note("put", Some(location));
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.note("put_multipart", Some(location));
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.note(if options.head { "head" } else { "get" }, Some(location));
        if !options.head {
            let mut fails = self.fail_reads.lock().unwrap();
            if let Some(n) = fails.get_mut(location.as_ref())
                && *n > 0
            {
                *n -= 1;
                return Err(object_store::Error::Generic {
                    store: "recording",
                    source: "injected transient failure".into(),
                });
            }
        }
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.note("delete", None);
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.note("list", prefix);
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.note("list_dir", prefix);
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.note("copy", Some(from));
        self.inner.copy_opts(from, to, options).await
    }
}
