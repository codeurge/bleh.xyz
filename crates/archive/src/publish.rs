//! Publishing a signed day: object storage (HTTPS origin and web seed),
//! IPFS pinning through the Kubo HTTP API, torrent files and magnet lists,
//! and the static Iceberg metadata.
//!
//! Before anything leaves the machine, every listed file is checked against
//! the signed manifest (size and SHA-256). Uploads are write-once except the
//! anchors, the OpenTimestamps proof, `version-hint.text` and the indexes.
//! Every CID Kubo returns must equal the manifest's CID.

use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;
use futures::future::BoxFuture;

use mesh_types::manifest::SignedManifest;

use crate::error::{Error, IoContext, Result};
use crate::http::{HttpClient, Response, join_url};
use crate::layout::{self, Day};
use crate::manifest;
use crate::partition::{self, iceberg};
use crate::store::{PutOutcome, Store};

/// Adds and pins content on IPFS.
pub trait IpfsPinner: Send + Sync + std::fmt::Debug {
    /// Adds `data` with CIDv1, raw leaves and `chunker`, pins it, and
    /// returns the CID.
    fn add<'a>(
        &'a self,
        name: &'a str,
        data: Bytes,
        chunker: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
    /// Pins an existing CID.
    fn pin<'a>(&'a self, cid: &'a str) -> BoxFuture<'a, Result<()>>;
}

/// [`IpfsPinner`] over the Kubo RPC API (`/api/v0/add`, `/api/v0/pin/add`),
/// such as `http://127.0.0.1:5001`.
///
/// A call that fails in transport (connection refused or reset, timeout, a
/// response body cut short, as when Kubo stalls at its memory limit), or
/// that a proxy answers 502, 503 or 504, is retried with doubling delays, a
/// few times, and each retry is logged. Adding and pinning are idempotent:
/// the same bytes give the same CID. Every other status is final, so a
/// Kubo error such as a full repo fails at once, and a returned CID that
/// differs from the manifest is never retried.
#[derive(Debug, Clone)]
pub struct KuboClient {
    http: Arc<dyn HttpClient>,
    api: String,
    attempts: u32,
    first_delay: std::time::Duration,
}

impl KuboClient {
    /// Tries per call, the first included.
    pub const DEFAULT_ATTEMPTS: u32 = 4;
    /// Delay before the first retry; it doubles after each one (1, 2, 4 s).
    pub const DEFAULT_FIRST_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
    /// Gateway statuses retried like transport errors (a proxy in front of
    /// Kubo failing to reach it, not an answer from Kubo).
    pub const GATEWAY_RETRY: [u16; 3] = [502, 503, 504];

    /// A client for the API at `api`.
    pub fn new(http: Arc<dyn HttpClient>, api: impl Into<String>) -> Self {
        Self {
            http,
            api: api.into(),
            attempts: Self::DEFAULT_ATTEMPTS,
            first_delay: Self::DEFAULT_FIRST_DELAY,
        }
    }

    /// Overrides the transport retries: `attempts` tries per call (at least
    /// one), the first retry after `first_delay`.
    #[must_use]
    pub fn with_retries(mut self, attempts: u32, first_delay: std::time::Duration) -> Self {
        self.attempts = attempts.max(1);
        self.first_delay = first_delay;
        self
    }

    /// POSTs `body` to `url`, retrying transport and gateway failures.
    async fn post(&self, url: &str, headers: &[(&str, &str)], body: Bytes) -> Result<Response> {
        let mut delay = self.first_delay;
        let mut attempt = 1;
        loop {
            let res = self.http.post(url, headers, body.clone()).await;
            let why = match &res {
                Ok(r) if !Self::GATEWAY_RETRY.contains(&r.status) => None,
                Ok(r) => Some(format!("status {}", r.status)),
                Err(e) => Some(e.to_string()),
            };
            match why {
                Some(why) if attempt < self.attempts => {
                    tracing::warn!(
                        %url, attempt, of = self.attempts, error = %why,
                        retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                        "kubo call failed in transport; retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2);
                    attempt += 1;
                }
                _ => return res,
            }
        }
    }
}

const BOUNDARY: &str = "bleh-archive-boundary-7d3f1c";

/// One `file` part named after the last segment of `name`. Kubo treats a
/// filename with `/` as a path inside a directory and returns the
/// directory's CID, not the file's, so only the base name is sent.
fn multipart(name: &str, data: &[u8]) -> Bytes {
    let base = name.rsplit('/').next().unwrap_or(name);
    let safe: String = base
        .chars()
        .map(|c| if c == '"' || c.is_control() { '_' } else { c })
        .collect();
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    Bytes::from(body)
}

impl IpfsPinner for KuboClient {
    fn add<'a>(
        &'a self,
        name: &'a str,
        data: Bytes,
        chunker: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let url = join_url(
                &self.api,
                &format!(
                    "api/v0/add?cid-version=1&raw-leaves=true&chunker={chunker}&hash=sha2-256&pin=true&quieter=true"
                ),
            );
            let ct = format!("multipart/form-data; boundary={BOUNDARY}");
            let body = self
                .post(
                    &url,
                    &[("Content-Type", ct.as_str())],
                    multipart(name, &data),
                )
                .await?
                .into_ok("kubo add")?;
            // Kubo streams one JSON object per line; the last names the root.
            let text = String::from_utf8_lossy(&body);
            let last = text
                .lines()
                .rfind(|l| !l.trim().is_empty())
                .ok_or_else(|| Error::Http("kubo add: empty response".into()))?;
            let v: serde_json::Value = serde_json::from_str(last)?;
            v.get("Hash")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| Error::Http(format!("kubo add: no Hash in {last}")))
        })
    }

    fn pin<'a>(&'a self, cid: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let url = join_url(&self.api, &format!("api/v0/pin/add?arg={cid}"));
            self.post(&url, &[], Bytes::new())
                .await?
                .into_ok("kubo pin")?;
            Ok(())
        })
    }
}

fn local(root: &Path, rel: &str) -> std::path::PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// Checks every listed file under the archive root against the manifest
/// (size and SHA-256).
///
/// # Errors
///
/// Fails on the first missing or mismatching file.
pub fn check_local(root: &Path, signed: &SignedManifest) -> Result<()> {
    for f in &signed.manifest.files {
        let p = local(root, &f.path);
        let size = std::fs::metadata(&p).at(&p)?.len();
        if size != f.size || crate::digest::sha256_file(&p)? != f.sha256 {
            return Err(Error::invalid(format!(
                "{}: does not match the signed manifest",
                f.path
            )));
        }
    }
    Ok(())
}

/// What [`publish_day`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishReport {
    /// Write-once uploads and their outcome.
    pub uploaded: Vec<(String, PutOutcome)>,
    /// Mutable objects replaced.
    pub replaced: Vec<String>,
    /// Pinned files and their CIDs.
    pub pinned: Vec<(String, String)>,
    /// CID of `manifest.json`, if pinned.
    pub manifest_cid: Option<String>,
}

/// Publishes a signed day of the archive at `root` (the archive's own root
/// directory, named `archive`). Uploads to `store` under `<archive>/` and
/// pins on IPFS when given.
///
/// # Errors
///
/// Fails if the day is not signed, a local file does not match the
/// manifest, an upload conflicts, or Kubo returns a different CID.
pub async fn publish_day(
    root: &Path,
    archive: &str,
    day: Day,
    store: Option<&Store>,
    pinner: Option<&dyn IpfsPinner>,
) -> Result<PublishReport> {
    let signed = manifest::read_signed(root, day)?
        .ok_or_else(|| Error::invalid(format!("no signed manifest for {day}; sign it first")))?;
    if signed.manifest.archive != archive {
        return Err(Error::invalid("manifest belongs to another archive"));
    }
    check_local(root, &signed)?;
    let mut report = PublishReport::default();
    let manifest_rel = layout::manifest_file(day, layout::MANIFEST_FILE);
    let torrent_rel = layout::torrent_file(archive, day);
    if let Some(store) = store {
        let mut once: Vec<String> = signed
            .manifest
            .files
            .iter()
            .map(|f| f.path.clone())
            .collect();
        once.push(manifest_rel.clone());
        if local(root, &torrent_rel).exists() {
            once.push(torrent_rel.clone());
        }
        for rel in &once {
            let outcome = store
                .put_file_once(&format!("{archive}/{rel}"), &local(root, rel))
                .await?;
            report.uploaded.push((rel.clone(), outcome));
        }
        for rel in [
            layout::manifest_file(day, layout::ANCHORS_FILE),
            layout::manifest_file(day, layout::OTS_FILE),
            layout::MANIFEST_INDEX_FILE.to_owned(),
            layout::MAGNETS_FILE.to_owned(),
            layout::LATEST_FILE.to_owned(),
        ] {
            let p = local(root, &rel);
            if p.exists() {
                store
                    .put_mutable(
                        &format!("{archive}/{rel}"),
                        Bytes::from(std::fs::read(&p).at(&p)?),
                    )
                    .await?;
                report.replaced.push(rel);
            }
        }
    }
    if let Some(pinner) = pinner {
        for f in &signed.manifest.files {
            let data = Bytes::from(std::fs::read(local(root, &f.path)).at(local(root, &f.path))?);
            let cid = pinner.add(&f.path, data, &f.ipfs.chunker).await?;
            if cid != f.ipfs.cid {
                return Err(Error::invalid(format!(
                    "{}: IPFS returned CID {cid}, manifest says {}",
                    f.path, f.ipfs.cid
                )));
            }
            report.pinned.push((f.path.clone(), cid));
        }
        let mj = std::fs::read(local(root, &manifest_rel)).at(local(root, &manifest_rel))?;
        let expected = crate::ipfs::cid_of(&mj).to_string();
        let cid = pinner
            .add(
                &manifest_rel,
                Bytes::from(mj),
                &format!("size-{}", crate::ipfs::DEFAULT_CHUNK_SIZE),
            )
            .await?;
        if cid != expected {
            return Err(Error::invalid(format!(
                "manifest.json: IPFS returned {cid}, expected {expected}"
            )));
        }
        report.pinned.push((manifest_rel.clone(), cid.clone()));
        report.manifest_cid = Some(cid);
        for rel in [
            layout::manifest_file(day, layout::ANCHORS_FILE),
            layout::manifest_file(day, layout::OTS_FILE),
        ] {
            let p = local(root, &rel);
            if p.exists() {
                let cid = pinner
                    .add(
                        &rel,
                        Bytes::from(std::fs::read(&p).at(&p)?),
                        &format!("size-{}", crate::ipfs::DEFAULT_CHUNK_SIZE),
                    )
                    .await?;
                report.pinned.push((rel, cid));
            }
        }
    }
    crate::metrics::inc("archive_publish_days_total", &[("archive", archive)], 1);
    Ok(report)
}

/// Rebuilds the Iceberg metadata of a normalized archive from every signed
/// manifest under `root`, for tables located at `location` (the archive
/// root's URI). Writes the files under `root/_iceberg/` and returns their
/// paths; uploads them too when `store` is given.
///
/// # Errors
///
/// Fails on I/O, parse or upload errors.
pub async fn write_iceberg(
    root: &Path,
    archive: &str,
    location: &str,
    store: Option<&Store>,
) -> Result<Vec<String>> {
    let mut manifests = Vec::new();
    for d in manifest::manifest_days(root)? {
        if let Some(m) = manifest::read_signed(root, d)? {
            manifests.push(m);
        }
    }
    let mut written = Vec::new();
    for (kind, snaps) in iceberg::snapshots_from_manifests(&manifests)? {
        let files = iceberg::build_table(archive, location, kind, &snaps)?;
        for (rel, bytes) in &files.immutable {
            partition::write_once(&local(root, rel), bytes)?;
            if let Some(s) = store {
                s.put_once(&format!("{archive}/{rel}"), Bytes::from(bytes.clone()))
                    .await?;
            }
            written.push(rel.clone());
        }
        if let Some((rel, bytes)) = &files.version_hint {
            partition::write_atomic(&local(root, rel), bytes)?;
            if let Some(s) = store {
                s.put_mutable(&format!("{archive}/{rel}"), Bytes::from(bytes.clone()))
                    .await?;
            }
            written.push(rel.clone());
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockHttp, Response};
    use crate::partition::{PartitionOptions, write_partitions};
    use crate::testutil::{every_kind, key};
    use mesh_types::UnixNanos;

    #[test]
    fn kubo_part_names_the_file_not_its_path() {
        // A path in the filename makes Kubo wrap the file in directories
        // and return the root directory's CID.
        let body = multipart("kind=trade/date=2026-01-01/part-00000.parquet", b"x");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("filename=\"part-00000.parquet\""), "{text}");
        assert!(!text.contains("kind=trade"));
    }

    fn signed_day(root: &Path) -> (Day, SignedManifest) {
        write_partitions(root, every_kind(), &PartitionOptions::default()).unwrap();
        let k = key(1);
        let opts = manifest::BuildOptions {
            archive: "hyperliquid-normalized".into(),
            day: "2026-01-01".parse().unwrap(),
            web_seeds: vec!["https://m.example/".into()],
            trackers: vec![],
            created_ns: UnixNanos(1_767_312_000_000_000_000),
            signer: k.address(),
            next_signer: None,
            previous: None,
        };
        let built = manifest::build(root, &opts).unwrap();
        let s = manifest::sign(built.manifest, &k).unwrap();
        manifest::write_signed(root, &s, &built.torrent.metainfo).unwrap();
        (opts.day, s)
    }

    /// A fake Kubo that computes CIDs with this crate (as a real node would
    /// with the same parameters), optionally corrupting one.
    fn fake_kubo(http: &MockHttp, corrupt: bool) {
        http.handle("http://kubo:5001/api/v0/add", move |req| {
            let body = &req.body;
            let start = body.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let end = body.len() - format!("\r\n--{BOUNDARY}--\r\n").len();
            let mut cid = crate::ipfs::cid_of(&body[start..end]).to_string();
            if corrupt {
                cid = crate::ipfs::cid_of(b"other").to_string();
            }
            Response {
                status: 200,
                body: format!("{{\"Name\":\"x\",\"Hash\":\"{cid}\",\"Size\":\"1\"}}\n").into(),
            }
        });
    }

    #[tokio::test]
    async fn publishes_to_store_and_ipfs() {
        let dir = tempfile::tempdir().unwrap();
        let (day, signed) = signed_day(dir.path());
        let store = Store::memory();
        let http = MockHttp::new();
        fake_kubo(&http, false);
        let kubo = KuboClient::new(Arc::new(http.clone()), "http://kubo:5001");
        let r = publish_day(
            dir.path(),
            "hyperliquid-normalized",
            day,
            Some(&store),
            Some(&kubo),
        )
        .await
        .unwrap();
        assert_eq!(r.uploaded.len(), signed.manifest.files.len() + 2);
        assert!(r.uploaded.iter().all(|(_, o)| *o == PutOutcome::Created));
        assert!(r.replaced.contains(&layout::MANIFEST_INDEX_FILE.to_owned()));
        assert_eq!(r.pinned.len(), signed.manifest.files.len() + 1);
        let mj = store
            .get("hyperliquid-normalized/_manifests/2026-01-01/manifest.json")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(manifest::parse_signed(&mj).unwrap(), signed);
        // Requests carry the pinned IPFS parameters.
        assert!(http.requests().iter().all(|q| {
            q.url
                .contains("cid-version=1&raw-leaves=true&chunker=size-1048576")
        }));
        // Publishing again is idempotent.
        let again = publish_day(
            dir.path(),
            "hyperliquid-normalized",
            day,
            Some(&store),
            None,
        )
        .await
        .unwrap();
        assert!(
            again
                .uploaded
                .iter()
                .all(|(_, o)| *o == PutOutcome::Identical)
        );
    }

    #[tokio::test]
    async fn refuses_mismatches() {
        let dir = tempfile::tempdir().unwrap();
        let (day, signed) = signed_day(dir.path());
        let http = MockHttp::new();
        fake_kubo(&http, true);
        let kubo = KuboClient::new(Arc::new(http), "http://kubo:5001");
        assert!(
            publish_day(dir.path(), "hyperliquid-normalized", day, None, Some(&kubo))
                .await
                .is_err()
        );
        assert!(
            publish_day(dir.path(), "other-archive", day, None, None)
                .await
                .is_err()
        );
        // A tampered local file is caught before upload.
        let f = &signed.manifest.files[0];
        std::fs::write(dir.path().join(&f.path), b"tampered").unwrap();
        let store = Store::memory();
        assert!(
            publish_day(
                dir.path(),
                "hyperliquid-normalized",
                day,
                Some(&store),
                None
            )
            .await
            .is_err()
        );
        assert!(
            store
                .get(&format!("hyperliquid-normalized/{}", f.path))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn iceberg_metadata_from_manifests() {
        let dir = tempfile::tempdir().unwrap();
        signed_day(dir.path());
        let store = Store::memory();
        let written = write_iceberg(
            dir.path(),
            "hyperliquid-normalized",
            "s3://bucket/hyperliquid-normalized",
            Some(&store),
        )
        .await
        .unwrap();
        assert!(written.contains(&"_iceberg/trade/metadata/v1.metadata.json".to_owned()));
        assert!(written.contains(&"_iceberg/swap_print/metadata/version-hint.text".to_owned()));
        // Rebuilding is idempotent (write-once files are identical).
        let again = write_iceberg(
            dir.path(),
            "hyperliquid-normalized",
            "s3://bucket/hyperliquid-normalized",
            Some(&store),
        )
        .await
        .unwrap();
        assert_eq!(written, again);
        let meta: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("_iceberg/trade/metadata/v1.metadata.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["snapshots"][0]["summary"]["added-data-files"], "2");
    }

    /// Fails the first `fail` POSTs (transport error, or status `fail_with`),
    /// then answers from `inner`.
    #[derive(Debug)]
    struct FlakyKubo {
        inner: MockHttp,
        fail: std::sync::atomic::AtomicU32,
        fail_with: Option<u16>,
        posts: std::sync::atomic::AtomicU32,
    }

    impl FlakyKubo {
        fn new(fail: u32, fail_with: Option<u16>) -> Arc<Self> {
            let inner = MockHttp::new();
            fake_kubo(&inner, false);
            inner.route(
                "POST",
                "http://kubo:5001/api/v0/pin/add?arg=bafyx",
                200,
                "{}",
            );
            Arc::new(Self {
                inner,
                fail: fail.into(),
                fail_with,
                posts: 0.into(),
            })
        }
        fn posts(&self) -> u32 {
            self.posts.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl HttpClient for FlakyKubo {
        fn get<'a>(
            &'a self,
            url: &'a str,
            headers: &'a [(&'a str, &'a str)],
        ) -> BoxFuture<'a, Result<Response>> {
            self.inner.get(url, headers)
        }
        fn post<'a>(
            &'a self,
            url: &'a str,
            headers: &'a [(&'a str, &'a str)],
            body: Bytes,
        ) -> BoxFuture<'a, Result<Response>> {
            use std::sync::atomic::Ordering::SeqCst;
            self.posts.fetch_add(1, SeqCst);
            if self.fail.load(SeqCst) > 0 {
                self.fail.fetch_sub(1, SeqCst);
                let r = match self.fail_with {
                    None => Err(Error::Http("error decoding response body".into())),
                    Some(status) => Ok(Response {
                        status,
                        body: Bytes::from_static(b"kubo error"),
                    }),
                };
                return Box::pin(async move { r });
            }
            self.inner.post(url, headers, body)
        }
    }

    fn flaky_kubo(fail: u32, fail_with: Option<u16>) -> (Arc<FlakyKubo>, KuboClient) {
        let http = FlakyKubo::new(fail, fail_with);
        let kubo = KuboClient::new(http.clone(), "http://kubo:5001")
            .with_retries(4, std::time::Duration::ZERO);
        (http, kubo)
    }

    #[tokio::test]
    async fn kubo_add_retries_transport_errors() {
        let want = crate::ipfs::cid_of(b"data").to_string();
        // Three cut-off responses, then the CID: the fourth try succeeds.
        let (http, kubo) = flaky_kubo(3, None);
        let cid = kubo.add("a/f.parquet", Bytes::from_static(b"data"), "size-1048576");
        assert_eq!(cid.await.unwrap(), want);
        assert_eq!(http.posts(), 4);

        // Four failures exhaust the attempts and return the transport error.
        let (http, kubo) = flaky_kubo(4, None);
        let err = kubo
            .add("a/f.parquet", Bytes::from_static(b"data"), "size-1048576")
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("error decoding response body"),
            "{err}"
        );
        assert_eq!(http.posts(), 4);

        // Pinning retries the same way.
        let (http, kubo) = flaky_kubo(2, None);
        kubo.pin("bafyx").await.unwrap();
        assert_eq!(http.posts(), 3);
    }

    #[tokio::test]
    async fn kubo_retries_gateway_statuses_only() {
        for status in KuboClient::GATEWAY_RETRY {
            let (http, kubo) = flaky_kubo(1, Some(status));
            kubo.pin("bafyx").await.unwrap();
            assert_eq!(http.posts(), 2, "{status}");
        }
        // A Kubo error (a full repo answers 500) is final: one request.
        for status in [500, 400, 404] {
            let (http, kubo) = flaky_kubo(1, Some(status));
            assert!(kubo.pin("bafyx").await.is_err(), "{status}");
            assert_eq!(http.posts(), 1, "{status}");
        }
    }
}
