//! Mirror verification (`docs/specs/manifest-v0.md` § 7): given a local
//! directory or an HTTPS base URL holding an archive, verify a day in the
//! order the spec requires — **signature, hash chain, anchors, files** —
//! and produce a report anyone can read.
//!
//! - Signature: structure rules and secp256k1 recovery, against trusted
//!   signer addresses (or a trusted checkpoint manifest hash).
//! - Chain: walks back through `previous_manifest_sha256` (using
//!   `_manifests/index.json`, or a directory listing locally) to sequence 0
//!   or the checkpoint, then checks each link forward, carrying key rotation.
//! - Anchors: `manifest.anchors.json` must refer to this manifest; the
//!   OpenTimestamps proof must hash to `proof_sha256` and commit to the
//!   manifest hash, and its Bitcoin attestations are checked against block
//!   headers when an API is configured; the L2 transaction is checked over
//!   JSON-RPC when an RPC URL is configured. Missing or pending anchors are
//!   warnings inside the confirmation window (48 h Bitcoin, 24 h L2) and
//!   errors after it; a confirmed anchor that fails is always an error.
//! - Files: size and SHA-256 (authoritative); with `deep`, also the IPFS
//!   CID, the BitTorrent pieces root and, for partitions, the canonical
//!   content hash and record count recomputed from the Parquet records.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use futures::future::BoxFuture;
use serde::Serialize;

use mesh_types::manifest::{
    AnchorStatus, ManifestAnchors, SignedManifest, verify_link, verify_signed,
};
use mesh_types::{EthAddress, Sha256Digest, UnixNanos};

use crate::anchor::{evm, ots};
use crate::digest::FileDigester;
use crate::error::{Error, Result};
use crate::http::{HttpClient, encode_path, join_url};
use crate::ipfs::{UnixFsHasher, parse_chunker};
use crate::layout::{self, Day};
use crate::manifest::{self, ManifestIndex};
use crate::partition;

/// Where an archive is read from. Paths are relative to the archive root.
pub trait Source: Send + Sync + fmt::Debug {
    /// Reads a file, or `None` if absent.
    fn read<'a>(&'a self, rel: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>>;
    /// Days with a manifest, if the source can list them.
    fn manifest_days(&self) -> BoxFuture<'_, Result<Option<Vec<Day>>>>;
    /// A description for reports.
    fn describe(&self) -> String;
}

/// A local archive root.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    /// The archive rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Source for DirSource {
    fn read<'a>(&'a self, rel: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>> {
        Box::pin(async move {
            if rel.split('/').any(|s| s == ".." || s.is_empty()) {
                return Err(Error::invalid(format!("unsafe path {rel:?}")));
            }
            let mut p = self.root.clone();
            for seg in rel.split('/') {
                p.push(seg);
            }
            match std::fs::read(&p) {
                Ok(b) => Ok(Some(Bytes::from(b))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(Error::io(p, e)),
            }
        })
    }

    fn manifest_days(&self) -> BoxFuture<'_, Result<Option<Vec<Day>>>> {
        Box::pin(async move { manifest::manifest_days(&self.root).map(Some) })
    }

    fn describe(&self) -> String {
        self.root.display().to_string()
    }
}

/// An archive served over HTTP(S): `<base>/<path>`, where `base` is the
/// archive root URL (a web seed followed by the archive name).
///
/// A request that fails in transport (connection refused or reset, timeout,
/// a body cut short), or that a CDN edge answers with a gateway error (502,
/// 503, 504, or Cloudflare's 520 to 524), is retried with doubling delays,
/// a few times, and each retry is logged. Every other status is final (500,
/// 403, 404 and 410 included). The size, hash and content checks run on
/// what a read returns, so a mismatch is never retried.
#[derive(Debug, Clone)]
pub struct HttpSource {
    http: Arc<dyn HttpClient>,
    base: String,
    attempts: u32,
    first_delay: std::time::Duration,
}

impl HttpSource {
    /// Tries per request, the first included.
    pub const DEFAULT_ATTEMPTS: u32 = 4;
    /// Gateway statuses retried like transport errors: a CDN edge (such as
    /// Cloudflare in front of data.bleh.xyz) failing to reach the origin,
    /// not an answer from it.
    pub const EDGE_RETRY: [u16; 8] = [502, 503, 504, 520, 521, 522, 523, 524];
    /// Delay before the first retry; it doubles after each one (1, 2, 4 s).
    pub const DEFAULT_FIRST_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

    /// The archive at `base`.
    pub fn new(http: Arc<dyn HttpClient>, base: impl Into<String>) -> Self {
        Self {
            http,
            base: base.into(),
            attempts: Self::DEFAULT_ATTEMPTS,
            first_delay: Self::DEFAULT_FIRST_DELAY,
        }
    }

    /// Overrides the transport retries: `attempts` tries per request (at
    /// least one), the first retry after `first_delay`.
    #[must_use]
    pub fn with_retries(mut self, attempts: u32, first_delay: std::time::Duration) -> Self {
        self.attempts = attempts.max(1);
        self.first_delay = first_delay;
        self
    }
}

impl Source for HttpSource {
    fn read<'a>(&'a self, rel: &'a str) -> BoxFuture<'a, Result<Option<Bytes>>> {
        Box::pin(async move {
            let url = join_url(&self.base, &encode_path(rel));
            let mut delay = self.first_delay;
            let mut attempt = 1;
            let r = loop {
                let res = self.http.get(&url, &[]).await;
                let why = match &res {
                    Ok(r) if !Self::EDGE_RETRY.contains(&r.status) => None,
                    Ok(r) => Some(format!("status {}", r.status)),
                    Err(e) => Some(e.to_string()),
                };
                match why {
                    Some(why) if attempt < self.attempts => {
                        tracing::warn!(
                            %url, attempt, of = self.attempts, error = %why,
                            retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                            "request failed in transport or at the edge; retrying"
                        );
                        tokio::time::sleep(delay).await;
                        delay = delay.saturating_mul(2);
                        attempt += 1;
                    }
                    _ => break res?,
                }
            };
            match r.status {
                404 | 410 => Ok(None),
                _ => r.into_ok(rel).map(Some),
            }
        })
    }

    fn manifest_days(&self) -> BoxFuture<'_, Result<Option<Vec<Day>>>> {
        Box::pin(async { Ok(None) })
    }

    fn describe(&self) -> String {
        self.base.clone()
    }
}

/// How thoroughly files are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCheck {
    /// Do not fetch files (manifest, chain and anchors only).
    Skip,
    /// Size and SHA-256.
    Hashes,
    /// Also IPFS CID, BitTorrent pieces root and partition content hashes.
    Deep,
}

/// Verification settings.
#[derive(Debug, Clone)]
pub struct VerifyOptions {
    /// Trusted signer addresses (from the registry).
    pub trusted: Vec<EthAddress>,
    /// A manifest hash the verifier already trusts; the chain walk stops
    /// there.
    pub checkpoint: Option<Sha256Digest>,
    /// Current time, for the confirmation windows.
    pub now: UnixNanos,
    /// File checks.
    pub files: FileCheck,
    /// Bitcoin header source for OpenTimestamps verification.
    pub bitcoin: Option<Arc<dyn ots::BitcoinHeaders>>,
    /// L2 RPC for anchor verification, with the confirmations required.
    pub evm: Option<(evm::EvmRpc, u64)>,
    /// Longest chain walked back.
    pub max_chain: usize,
}

impl VerifyOptions {
    /// Options trusting `trusted`, checking hashes, without network checks.
    #[must_use]
    pub fn new(trusted: Vec<EthAddress>, now: UnixNanos) -> Self {
        Self {
            trusted,
            checkpoint: None,
            now,
            files: FileCheck::Hashes,
            bitcoin: None,
            evm: None,
            max_chain: 100_000,
        }
    }
}

/// Verification step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Structure and signature.
    Signature,
    /// Hash chain.
    Chain,
    /// Anchors.
    Anchors,
    /// Files.
    Files,
}

/// Outcome of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Passed.
    Pass,
    /// Acceptable for now, needs attention.
    Warn,
    /// Failed.
    Fail,
}

/// One check in a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Step.
    pub step: Step,
    /// What was checked.
    pub subject: String,
    /// Outcome.
    pub status: Status,
    /// Details.
    pub detail: String,
}

/// A verification report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// Source description.
    pub source: String,
    /// Archive.
    pub archive: String,
    /// Day.
    pub date: String,
    /// Manifest hash, if the manifest was read.
    pub manifest_sha256: Option<Sha256Digest>,
    /// Checks in order.
    pub checks: Vec<Check>,
}

impl Report {
    fn push(
        &mut self,
        step: Step,
        subject: impl Into<String>,
        status: Status,
        detail: impl Into<String>,
    ) {
        self.checks.push(Check {
            step,
            subject: subject.into(),
            status,
            detail: detail.into(),
        });
    }

    /// The worst status.
    #[must_use]
    pub fn status(&self) -> Status {
        self.checks
            .iter()
            .map(|c| c.status)
            .max()
            .unwrap_or(Status::Pass)
    }

    /// Whether no check failed.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status() != Status::Fail
    }

    /// Count of checks with `status`.
    #[must_use]
    pub fn count(&self, status: Status) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "archive  {}", self.archive)?;
        writeln!(f, "date     {}", self.date)?;
        writeln!(f, "source   {}", self.source)?;
        if let Some(h) = &self.manifest_sha256 {
            writeln!(f, "manifest {h}")?;
        }
        for c in &self.checks {
            let s = match c.status {
                Status::Pass => "PASS",
                Status::Warn => "WARN",
                Status::Fail => "FAIL",
            };
            let step = match c.step {
                Step::Signature => "signature",
                Step::Chain => "chain",
                Step::Anchors => "anchors",
                Step::Files => "files",
            };
            writeln!(f, "{s}  {step:<9}  {}: {}", c.subject, c.detail)?;
        }
        let verdict = match self.status() {
            Status::Pass => "VERIFIED",
            Status::Warn => "VERIFIED WITH WARNINGS",
            Status::Fail => "FAILED",
        };
        write!(
            f,
            "{verdict}: {} passed, {} warnings, {} failed",
            self.count(Status::Pass),
            self.count(Status::Warn),
            self.count(Status::Fail)
        )
    }
}

const OTS_WINDOW_NS: i64 = 48 * 3600 * 1_000_000_000;
const L2_WINDOW_NS: i64 = 24 * 3600 * 1_000_000_000;
/// A missing L2 anchor: optional until an L2 is chosen (ADR 0026).
const L2_ABSENT: &str = "no L2 anchor (optional until an L2 is chosen)";

async fn read_manifest(source: &dyn Source, day: Day) -> Result<Option<SignedManifest>> {
    match source
        .read(&layout::manifest_file(day, layout::MANIFEST_FILE))
        .await?
    {
        Some(b) => manifest::parse_signed(&b).map(Some),
        None => Ok(None),
    }
}

/// Finds the day of the manifest with `sequence`, from the index or a
/// listing.
async fn day_of_sequence(
    source: &dyn Source,
    index: Option<&ManifestIndex>,
    listing: Option<&[Day]>,
    sequence: u64,
    before: Day,
) -> Result<Option<SignedManifest>> {
    if let Some(e) = index.and_then(|i| i.at(sequence))
        && let Some(m) = read_manifest(source, e.date).await?
        && m.manifest.sequence == sequence
    {
        return Ok(Some(m));
    }
    if let Some(days) = listing {
        for d in days.iter().rev().filter(|d| **d < before) {
            if let Some(m) = read_manifest(source, *d).await? {
                if m.manifest.sequence == sequence {
                    return Ok(Some(m));
                }
                if m.manifest.sequence < sequence {
                    break;
                }
            }
        }
    }
    Ok(None)
}

/// Verifies one day of `archive` from `source`.
///
/// # Errors
///
/// Returns an error only for problems reading the source; verification
/// failures are reported as failed checks.
pub async fn verify_day(
    source: &dyn Source,
    archive: &str,
    day: Day,
    opts: &VerifyOptions,
) -> Result<Report> {
    let mut report = Report {
        source: source.describe(),
        archive: archive.to_owned(),
        date: day.to_string(),
        manifest_sha256: None,
        checks: Vec::new(),
    };
    let signed = match read_manifest(source, day).await {
        Ok(Some(m)) => m,
        Ok(None) => {
            report.push(Step::Signature, "manifest.json", Status::Fail, "not found");
            return Ok(report);
        }
        Err(e) => {
            report.push(
                Step::Signature,
                "manifest.json",
                Status::Fail,
                e.to_string(),
            );
            return Ok(report);
        }
    };
    let hash = signed.sha256()?;
    report.manifest_sha256 = Some(hash);
    if signed.manifest.archive != archive || signed.manifest.date != day.to_string() {
        report.push(
            Step::Signature,
            "manifest.json",
            Status::Fail,
            format!(
                "is for {} {}",
                signed.manifest.archive, signed.manifest.date
            ),
        );
        return Ok(report);
    }

    // Steps 1 and 2: walk back to a trusted root, then verify forward.
    let index: Option<ManifestIndex> = match source.read(layout::MANIFEST_INDEX_FILE).await? {
        Some(b) => serde_json::from_slice(&b).ok(),
        None => None,
    };
    let listing = source.manifest_days().await?;
    let mut chain = vec![signed.clone()];
    while let Some(last) = chain.last() {
        let last_hash = last.sha256()?;
        if Some(last_hash) == opts.checkpoint || last.manifest.sequence == 0 {
            break;
        }
        if chain.len() > opts.max_chain {
            report.push(
                Step::Chain,
                "walk",
                Status::Fail,
                "chain longer than the walk limit and no checkpoint reached",
            );
            return Ok(report);
        }
        let before: Day = last.manifest.date.parse()?;
        match day_of_sequence(
            source,
            index.as_ref(),
            listing.as_deref(),
            last.manifest.sequence - 1,
            before,
        )
        .await?
        {
            Some(prev) => chain.push(prev),
            None => {
                report.push(
                    Step::Chain,
                    format!("sequence {}", last.manifest.sequence - 1),
                    Status::Fail,
                    "previous manifest not found (no index entry or listing)",
                );
                return Ok(report);
            }
        }
    }
    chain.reverse();
    let root = &chain[0];
    let root_hash = root.sha256()?;
    let mut trusted = opts.trusted.clone();
    if Some(root_hash) == opts.checkpoint {
        trusted.push(root.manifest.signer);
    }
    if trusted.is_empty() {
        report.push(
            Step::Signature,
            "trust",
            Status::Fail,
            "no trusted signer or checkpoint given",
        );
        return Ok(report);
    }
    let mut prev: Option<&SignedManifest> = None;
    for m in &chain {
        let subject = format!("{} (sequence {})", m.manifest.date, m.manifest.sequence);
        let allowed: Vec<EthAddress> = match prev {
            None => trusted.clone(),
            Some(p) => std::iter::once(p.manifest.signer)
                .chain(p.manifest.next_signer)
                .collect(),
        };
        match verify_signed(m, &allowed) {
            Ok(()) => report.push(
                Step::Signature,
                &subject,
                Status::Pass,
                format!("signed by {}", m.manifest.signer),
            ),
            Err(e) => {
                report.push(Step::Signature, &subject, Status::Fail, e.to_string());
                return Ok(report);
            }
        }
        if let Some(p) = prev {
            match verify_link(p, m) {
                Ok(()) => report.push(
                    Step::Chain,
                    &subject,
                    Status::Pass,
                    "links to the previous manifest",
                ),
                Err(e) => {
                    report.push(Step::Chain, &subject, Status::Fail, e.to_string());
                    return Ok(report);
                }
            }
        }
        prev = Some(m);
    }
    let root_desc = if Some(root_hash) == opts.checkpoint {
        format!("checkpoint {root_hash}")
    } else {
        "sequence 0".to_owned()
    };
    report.push(
        Step::Chain,
        "root",
        Status::Pass,
        format!("{} manifests back to {root_desc}", chain.len()),
    );

    // Step 3: anchors.
    verify_anchors(source, day, &signed, &hash, opts, &mut report).await?;

    // Step 4: files.
    if opts.files != FileCheck::Skip {
        for f in &signed.manifest.files {
            let Some(data) = source.read(&f.path).await? else {
                report.push(Step::Files, &f.path, Status::Fail, "missing");
                continue;
            };
            if data.len() as u64 != f.size {
                report.push(
                    Step::Files,
                    &f.path,
                    Status::Fail,
                    format!("size {} != {}", data.len(), f.size),
                );
                continue;
            }
            if Sha256Digest::of(&data) != f.sha256 {
                report.push(Step::Files, &f.path, Status::Fail, "sha256 mismatch");
                continue;
            }
            if opts.files == FileCheck::Deep {
                let chunk = parse_chunker(&f.ipfs.chunker);
                let Some(chunk) = chunk.filter(|_| f.ipfs.raw_leaves) else {
                    report.push(
                        Step::Files,
                        &f.path,
                        Status::Warn,
                        format!("unsupported IPFS parameters {}", f.ipfs.chunker),
                    );
                    continue;
                };
                let mut d =
                    FileDigester::new(UnixFsHasher::new(chunk, crate::ipfs::DEFAULT_MAX_LINKS));
                d.update(&data);
                let d = d.finish();
                if d.ipfs.cid != f.ipfs.cid {
                    report.push(
                        Step::Files,
                        &f.path,
                        Status::Fail,
                        format!("IPFS CID {} != {}", d.ipfs.cid, f.ipfs.cid),
                    );
                    continue;
                }
                if d.pieces_root() != f.bt_pieces_root {
                    report.push(
                        Step::Files,
                        &f.path,
                        Status::Fail,
                        "BitTorrent pieces root mismatch",
                    );
                    continue;
                }
                if let (Some(ch), Some(n)) = (f.content_hash, f.record_count) {
                    match partition::read_parquet(data.clone()) {
                        Ok(r)
                            if r.content_hash == ch
                                && r.records.len() as u64 == n
                                && r.consistent() => {}
                        Ok(r) => {
                            report.push(
                                Step::Files,
                                &f.path,
                                Status::Fail,
                                format!(
                                    "records hash to {} ({}), manifest says {ch} ({n})",
                                    r.content_hash,
                                    r.records.len()
                                ),
                            );
                            continue;
                        }
                        Err(e) => {
                            report.push(
                                Step::Files,
                                &f.path,
                                Status::Fail,
                                format!("unreadable partition: {e}"),
                            );
                            continue;
                        }
                    }
                }
                report.push(
                    Step::Files,
                    &f.path,
                    Status::Pass,
                    "size, sha256, CID, pieces root and content verified",
                );
            } else {
                report.push(
                    Step::Files,
                    &f.path,
                    Status::Pass,
                    "size and sha256 verified",
                );
            }
        }
    }
    Ok(report)
}

async fn verify_anchors(
    source: &dyn Source,
    day: Day,
    signed: &SignedManifest,
    hash: &Sha256Digest,
    opts: &VerifyOptions,
    report: &mut Report,
) -> Result<()> {
    let age = opts.now.0.saturating_sub(signed.manifest.created_ns.0);
    let window = |w: i64| if age < w { Status::Warn } else { Status::Fail };
    let anchors: Option<ManifestAnchors> = match source
        .read(&layout::manifest_file(day, layout::ANCHORS_FILE))
        .await?
    {
        Some(b) => match serde_json::from_slice(&b) {
            Ok(a) => Some(a),
            Err(e) => {
                report.push(
                    Step::Anchors,
                    "manifest.anchors.json",
                    Status::Fail,
                    format!("malformed: {e}"),
                );
                return Ok(());
            }
        },
        None => None,
    };
    let Some(anchors) = anchors else {
        report.push(
            Step::Anchors,
            "bitcoin",
            window(OTS_WINDOW_NS),
            "no anchors file",
        );
        report.push(Step::Anchors, "ethereum_l2", Status::Warn, L2_ABSENT);
        return Ok(());
    };
    if let Err(e) = anchors.check(signed) {
        report.push(
            Step::Anchors,
            "manifest.anchors.json",
            Status::Fail,
            e.to_string(),
        );
        return Ok(());
    }

    // Bitcoin via OpenTimestamps.
    match &anchors.opentimestamps {
        None => report.push(
            Step::Anchors,
            "bitcoin",
            window(OTS_WINDOW_NS),
            "no OpenTimestamps anchor",
        ),
        Some(a) => {
            let confirmed = a.status == AnchorStatus::Confirmed;
            let fail_status = if confirmed {
                Status::Fail
            } else {
                window(OTS_WINDOW_NS)
            };
            let proof_rel = format!("{}/{}", layout::manifest_dir(day), a.proof_path);
            let proof_bytes = if a.proof_path.contains('/') || a.proof_path.contains("..") {
                None
            } else {
                source.read(&proof_rel).await?
            };
            match proof_bytes {
                None => report.push(Step::Anchors, "bitcoin", fail_status, "proof file missing"),
                Some(b) if Sha256Digest::of(&b) != a.proof_sha256 => {
                    report.push(
                        Step::Anchors,
                        "bitcoin",
                        fail_status,
                        "proof does not match proof_sha256",
                    );
                }
                Some(b) => match ots::DetachedTimestamp::parse(&b)
                    .and_then(|p| p.check(&hash.0).map(|()| p))
                {
                    Err(e) => report.push(Step::Anchors, "bitcoin", fail_status, e.to_string()),
                    Ok(proof) => {
                        let heights: Vec<u64> = proof.bitcoin().iter().map(|(h, _)| *h).collect();
                        if confirmed
                            && !a.bitcoin_block_height.is_some_and(|h| heights.contains(&h))
                        {
                            report.push(
                                Step::Anchors,
                                "bitcoin",
                                Status::Fail,
                                "proof has no attestation at the recorded height",
                            );
                        } else if heights.is_empty() {
                            report.push(
                                Step::Anchors,
                                "bitcoin",
                                window(OTS_WINDOW_NS),
                                "pending (calendar promise only)",
                            );
                        } else {
                            match &opts.bitcoin {
                                Some(h) => match ots::verify_bitcoin(&proof, h.as_ref()).await {
                                    Ok(c) => report.push(
                                        Step::Anchors,
                                        "bitcoin",
                                        if confirmed { Status::Pass } else { Status::Warn },
                                        format!(
                                            "attested in block {} (verified against the header){}",
                                            c.iter().map(|c| c.height.to_string()).collect::<Vec<_>>().join(", "),
                                            if confirmed { "" } else { "; anchors file still says pending" }
                                        ),
                                    ),
                                    Err(e) => report.push(Step::Anchors, "bitcoin", Status::Fail, e.to_string()),
                                },
                                None => report.push(
                                    Step::Anchors,
                                    "bitcoin",
                                    Status::Warn,
                                    format!("attestation at block {heights:?} not checked: no Bitcoin API configured"),
                                ),
                            }
                        }
                    }
                },
            }
        }
    }

    // Ethereum L2: optional until an L2 is chosen (ADR 0026). A missing
    // anchor is a warning at any age; one that is present follows the
    // confirmation window.
    match &anchors.ethereum_l2 {
        None => report.push(Step::Anchors, "ethereum_l2", Status::Warn, L2_ABSENT),
        Some(a) => match &opts.evm {
            None => report.push(
                Step::Anchors,
                "ethereum_l2",
                if a.status == AnchorStatus::Confirmed {
                    Status::Warn
                } else {
                    window(L2_WINDOW_NS)
                },
                format!(
                    "chain {} tx {} not checked: no RPC configured",
                    a.chain_id, a.tx_hash
                ),
            ),
            Some((rpc, confs)) => match evm::verify_anchor(rpc, a, &hash.0, *confs).await {
                Ok(AnchorStatus::Confirmed) => report.push(
                    Step::Anchors,
                    "ethereum_l2",
                    Status::Pass,
                    format!(
                        "chain {} tx {} in block {}",
                        a.chain_id, a.tx_hash, a.block_number
                    ),
                ),
                Ok(AnchorStatus::Pending) if a.status == AnchorStatus::Confirmed => report.push(
                    Step::Anchors,
                    "ethereum_l2",
                    Status::Fail,
                    "marked confirmed but has too few confirmations",
                ),
                Ok(AnchorStatus::Pending) => {
                    report.push(
                        Step::Anchors,
                        "ethereum_l2",
                        window(L2_WINDOW_NS),
                        "included, not yet final",
                    );
                }
                Err(e) => report.push(Step::Anchors, "ethereum_l2", Status::Fail, e.to_string()),
            },
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockHttp, Response};
    use crate::partition::{PartitionOptions, write_partitions};
    use crate::testutil::{every_kind, key};
    use mesh_types::manifest::{ANCHORS_FORMAT, EvmAnchor, OtsAnchor};

    const T0: i64 = 1_767_312_000_000_000_000;

    fn shifted(days: i64) -> Vec<mesh_types::Record> {
        let mut recs = every_kind();
        let d = days * 86_400_000;
        for r in &mut recs {
            r.prov_mut().ts_source_ms.0 += d;
            match r {
                mesh_types::Record::Bar(b) => b.start_ms.0 += d,
                mesh_types::Record::Footprint(f) => f.start_ms.0 += d,
                mesh_types::Record::VolumeProfile(v) => {
                    v.start_ms.0 += d;
                    v.end_ms.0 += d;
                }
                mesh_types::Record::SwapPrint(s) => s.ts_dissemination_ms.0 += d,
                r if r.kind().is_analytics() => {
                    // Shift every top-level time; durations stay.
                    let mut v = serde_json::to_value(&*r).unwrap();
                    for (k, x) in v.as_object_mut().unwrap() {
                        if k.ends_with("_ms") && k != "time_step_ms" && k != "window_ms" {
                            *x = serde_json::Value::from(x.as_i64().unwrap() + d);
                        }
                    }
                    *r = serde_json::from_value(v).unwrap();
                }
                _ => {}
            }
        }
        recs
    }

    /// Three signed days; the third is signed by a rotated key.
    fn chain(root: &std::path::Path) -> Vec<SignedManifest> {
        let (k1, k2) = (key(1), key(2));
        let mut out: Vec<SignedManifest> = Vec::new();
        for i in 0..3 {
            write_partitions(root, shifted(i), &PartitionOptions::default()).unwrap();
            let k = if i == 2 { &k2 } else { &k1 };
            let opts = manifest::BuildOptions {
                archive: "hyperliquid-normalized".into(),
                day: Day::from_days(20_454 + i32::try_from(i).unwrap()),
                web_seeds: vec!["https://m.example/".into()],
                trackers: vec![],
                created_ns: UnixNanos(T0 + i * 86_400_000_000_000),
                signer: k.address(),
                next_signer: (i == 1).then(|| k2.address()),
                previous: out.last().cloned(),
            };
            let built = manifest::build(root, &opts).unwrap();
            let s = manifest::sign(built.manifest, k).unwrap();
            manifest::write_signed(root, &s, &built.torrent.metainfo).unwrap();
            out.push(s);
        }
        out
    }

    fn opts(now_offset_h: i64) -> VerifyOptions {
        let mut o = VerifyOptions::new(
            vec![key(1).address()],
            UnixNanos(T0 + 2 * 86_400_000_000_000 + now_offset_h * 3_600_000_000_000),
        );
        o.files = FileCheck::Deep;
        o
    }

    #[tokio::test]
    async fn verifies_chain_rotation_and_files_locally() {
        let dir = tempfile::tempdir().unwrap();
        let ms = chain(dir.path());
        let day: Day = ms[2].manifest.date.parse().unwrap();
        let src = DirSource::new(dir.path());
        let r = verify_day(&src, "hyperliquid-normalized", day, &opts(1))
            .await
            .unwrap();
        assert!(r.ok(), "{r}");
        assert_eq!(r.status(), Status::Warn, "{r}"); // anchors missing, within window
        assert_eq!(r.count(Status::Fail), 0);
        assert_eq!(
            r.checks
                .iter()
                .filter(|c| c.step == Step::Signature && c.status == Status::Pass)
                .count(),
            3
        );
        assert!(r.to_string().contains("VERIFIED WITH WARNINGS"));
        // After the window, a missing Bitcoin anchor fails; a missing L2
        // anchor stays a warning at any age (ADR 0026).
        let late = verify_day(&src, "hyperliquid-normalized", day, &opts(72))
            .await
            .unwrap();
        assert!(!late.ok());
        let status_of = |r: &Report, subject: &str| {
            r.checks
                .iter()
                .find(|c| c.step == Step::Anchors && c.subject == subject)
                .map(|c| c.status)
        };
        assert_eq!(status_of(&late, "bitcoin"), Some(Status::Fail), "{late}");
        assert_eq!(
            status_of(&late, "ethereum_l2"),
            Some(Status::Warn),
            "{late}"
        );
        // Untrusted signer fails.
        let mut o = opts(1);
        o.trusted = vec![key(9).address()];
        assert!(
            !verify_day(&src, "hyperliquid-normalized", day, &o)
                .await
                .unwrap()
                .ok()
        );
        // A checkpoint at day 2 is enough without any trusted key.
        let mut o = opts(1);
        o.trusted.clear();
        o.checkpoint = Some(ms[1].sha256().unwrap());
        let r = verify_day(&src, "hyperliquid-normalized", day, &o)
            .await
            .unwrap();
        assert!(r.ok(), "{r}");
        assert!(r.to_string().contains("checkpoint"));
        // No trust at all fails.
        let mut o = opts(1);
        o.trusted.clear();
        assert!(
            !verify_day(&src, "hyperliquid-normalized", day, &o)
                .await
                .unwrap()
                .ok()
        );
    }

    #[tokio::test]
    async fn detects_tampered_files_and_missing_links() {
        let dir = tempfile::tempdir().unwrap();
        let ms = chain(dir.path());
        let day: Day = ms[2].manifest.date.parse().unwrap();
        let f = &ms[2].manifest.files[1];
        let p = dir.path().join(&f.path);
        let mut data = std::fs::read(&p).unwrap();
        let last = data.len() - 9;
        data[last] ^= 1;
        std::fs::write(&p, &data).unwrap();
        let src = DirSource::new(dir.path());
        let r = verify_day(&src, "hyperliquid-normalized", day, &opts(1))
            .await
            .unwrap();
        assert!(!r.ok());
        assert!(
            r.checks
                .iter()
                .any(|c| c.subject == f.path && c.status == Status::Fail)
        );
        // Remove day 2's manifest and the index: the chain cannot be walked.
        std::fs::remove_file(
            dir.path()
                .join(format!("_manifests/{}/manifest.json", ms[1].manifest.date)),
        )
        .unwrap();
        std::fs::remove_file(dir.path().join(layout::MANIFEST_INDEX_FILE)).unwrap();
        let r = verify_day(&src, "hyperliquid-normalized", day, &opts(1))
            .await
            .unwrap();
        assert!(
            r.checks
                .iter()
                .any(|c| c.step == Step::Chain && c.status == Status::Fail),
            "{r}"
        );
        // Missing manifest.
        let r = verify_day(
            &src,
            "hyperliquid-normalized",
            "2030-01-01".parse().unwrap(),
            &opts(1),
        )
        .await
        .unwrap();
        assert!(!r.ok());
    }

    #[tokio::test]
    async fn verifies_over_http_with_anchors() {
        let dir = tempfile::tempdir().unwrap();
        let ms = chain(dir.path());
        let s = &ms[2];
        let day: Day = s.manifest.date.parse().unwrap();
        let hash = s.sha256().unwrap();

        // Confirmed anchors: an OTS proof with a Bitcoin attestation and an L2 tx.
        let (resp, commit) =
            ots::tests::calendar_response(&hash.0, "https://alice.btc.calendar.opentimestamps.org");
        let mut proof = ots::DetachedTimestamp::new(hash.0);
        proof
            .timestamp
            .merge(ots::Timestamp::parse(&resp, hash.0.to_vec()).unwrap())
            .unwrap();
        let (up, attested) = ots::tests::upgrade_response(&commit, 900_002);
        proof
            .timestamp
            .merge_at(
                &commit,
                &ots::Timestamp::parse(&up, commit.clone()).unwrap(),
            )
            .unwrap();
        let proof_bytes = proof.to_bytes();
        let tx_hash = mesh_types::H256([0x77; 32]);
        let anchors = ManifestAnchors {
            format: ANCHORS_FORMAT.into(),
            manifest_sha256: hash,
            opentimestamps: Some(OtsAnchor {
                proof_path: layout::OTS_FILE.into(),
                proof_sha256: Sha256Digest::of(&proof_bytes),
                status: AnchorStatus::Confirmed,
                bitcoin_block_height: Some(900_002),
            }),
            ethereum_l2: Some(EvmAnchor {
                chain_id: 10,
                tx_hash,
                block_number: 40,
                committed_hash: mesh_types::H256(hash.0),
                status: AnchorStatus::Confirmed,
            }),
        };
        std::fs::write(
            dir.path()
                .join(format!("_manifests/{day}/manifest.json.ots")),
            &proof_bytes,
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join(format!("_manifests/{day}/manifest.anchors.json")),
            serde_json::to_vec(&anchors).unwrap(),
        )
        .unwrap();

        // Serve the directory over a mocked HTTPS mirror.
        let http = MockHttp::new();
        let root = dir.path().to_path_buf();
        http.handle(
            "https://mirror.example/hyperliquid-normalized/",
            move |req| {
                assert!(!req.url.contains('#'), "unencoded URL {}", req.url);
                let rel = req
                    .url
                    .trim_start_matches("https://mirror.example/hyperliquid-normalized/")
                    .replace("%23", "#");
                match std::fs::read(root.join(rel)) {
                    Ok(b) => Response {
                        status: 200,
                        body: b.into(),
                    },
                    Err(_) => Response {
                        status: 404,
                        body: Bytes::new(),
                    },
                }
            },
        );
        let header = ots::tests::header_with_root(&attested);
        let bh = ots::block_hash_hex(&header);
        http.route(
            "GET",
            "https://btc.example/block-height/900002",
            200,
            bh.clone(),
        );
        http.route(
            "GET",
            &format!("https://btc.example/block/{bh}/header"),
            200,
            crate::hex::encode(&header),
        );
        evm::tests::mock_node(
            &http,
            "https://rpc.example",
            10,
            tx_hash,
            evm::anchor_calldata(&hash.0),
            40,
            100,
        );

        let src = HttpSource::new(
            Arc::new(http.clone()),
            "https://mirror.example/hyperliquid-normalized",
        );
        let mut o = opts(100);
        o.bitcoin = Some(Arc::new(ots::EsploraHeaders::new(
            Arc::new(http.clone()),
            "https://btc.example",
        )));
        o.evm = Some((
            evm::EvmRpc::new(Arc::new(http.clone()), "https://rpc.example"),
            12,
        ));
        let r = verify_day(&src, "hyperliquid-normalized", day, &o)
            .await
            .unwrap();
        assert_eq!(r.status(), Status::Pass, "{r}");
        assert!(r.to_string().contains("VERIFIED:"));

        // Without network sources the confirmed anchors are warnings.
        let mut o2 = opts(100);
        o2.files = FileCheck::Skip;
        let r = verify_day(&src, "hyperliquid-normalized", day, &o2)
            .await
            .unwrap();
        assert_eq!(r.status(), Status::Warn, "{r}");
        // No L2 anchor at all, long after the window: a warning, not a
        // failure, while Bitcoin still passes (ADR 0026).
        let no_l2 = ManifestAnchors {
            ethereum_l2: None,
            ..anchors.clone()
        };
        let anchors_path = dir
            .path()
            .join(format!("_manifests/{day}/manifest.anchors.json"));
        std::fs::write(&anchors_path, serde_json::to_vec(&no_l2).unwrap()).unwrap();
        let r = verify_day(&src, "hyperliquid-normalized", day, &o)
            .await
            .unwrap();
        assert!(r.ok(), "{r}");
        assert_eq!(r.status(), Status::Warn, "{r}");
        assert_eq!(r.count(Status::Fail), 0, "{r}");
        std::fs::write(&anchors_path, serde_json::to_vec(&anchors).unwrap()).unwrap();
        // A tampered proof fails even though it is "confirmed".
        std::fs::write(
            dir.path()
                .join(format!("_manifests/{day}/manifest.json.ots")),
            b"junk",
        )
        .unwrap();
        let r = verify_day(&src, "hyperliquid-normalized", day, &o)
            .await
            .unwrap();
        assert!(!r.ok());
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"status\":\"fail\""));
    }

    /// Fails the first `fail` GETs in transport, then answers from `inner`.
    #[derive(Debug)]
    struct FlakyHttp {
        inner: MockHttp,
        fail: std::sync::atomic::AtomicU32,
        /// What a failure is: `None` a transport error, `Some(s)` status `s`.
        fail_with: Option<u16>,
        gets: std::sync::atomic::AtomicU32,
    }

    impl FlakyHttp {
        fn new(inner: MockHttp, fail: u32) -> Self {
            Self {
                inner,
                fail: fail.into(),
                fail_with: None,
                gets: 0.into(),
            }
        }
        fn gets(&self) -> u32 {
            self.gets.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl HttpClient for FlakyHttp {
        fn get<'a>(
            &'a self,
            url: &'a str,
            headers: &'a [(&'a str, &'a str)],
        ) -> BoxFuture<'a, Result<Response>> {
            use std::sync::atomic::Ordering::SeqCst;
            self.gets.fetch_add(1, SeqCst);
            let fail_now = self.fail.load(SeqCst) > 0;
            if fail_now {
                self.fail.fetch_sub(1, SeqCst);
                let r = match self.fail_with {
                    None => Err(Error::Http("error sending request".into())),
                    Some(status) => Ok(Response {
                        status,
                        body: Bytes::from_static(b"edge error"),
                    }),
                };
                return Box::pin(async move { r });
            }
            self.inner.get(url, headers)
        }
        fn post<'a>(
            &'a self,
            url: &'a str,
            headers: &'a [(&'a str, &'a str)],
            body: Bytes,
        ) -> BoxFuture<'a, Result<Response>> {
            self.inner.post(url, headers, body)
        }
    }

    fn flaky_source(fail: u32, status: u16) -> (Arc<FlakyHttp>, HttpSource) {
        let m = MockHttp::new();
        m.route("GET", "https://seed.example/a/f.parquet", status, "data");
        let http = Arc::new(FlakyHttp::new(m, fail));
        let src = HttpSource::new(http.clone(), "https://seed.example/a")
            .with_retries(4, std::time::Duration::ZERO);
        (http, src)
    }

    #[tokio::test]
    async fn http_source_retries_transport_errors() {
        // Three transport failures, then the answer: the fourth try succeeds.
        let (http, src) = flaky_source(3, 200);
        let got = src.read("f.parquet").await.unwrap();
        assert_eq!(got.as_deref(), Some(&b"data"[..]));
        assert_eq!(http.gets(), 4);

        // Four failures exhaust the attempts and return the transport error.
        let (http, src) = flaky_source(4, 200);
        let err = src.read("f.parquet").await.unwrap_err();
        assert!(err.to_string().contains("error sending request"), "{err}");
        assert_eq!(http.gets(), 4);
    }

    #[tokio::test]
    async fn http_source_retries_edge_gateway_errors() {
        for edge in HttpSource::EDGE_RETRY {
            // Three gateway errors, then the answer.
            let m = MockHttp::new();
            m.route("GET", "https://seed.example/a/f.parquet", 200, "data");
            let mut flaky = FlakyHttp::new(m, 3);
            flaky.fail_with = Some(edge);
            let http = Arc::new(flaky);
            let src = HttpSource::new(http.clone(), "https://seed.example/a")
                .with_retries(4, std::time::Duration::ZERO);
            let got = src.read("f.parquet").await.unwrap();
            assert_eq!(got.as_deref(), Some(&b"data"[..]), "{edge}");
            assert_eq!(http.gets(), 4, "{edge}");

            // A gateway error on every try is reported after four.
            let (http, src) = flaky_source(0, edge);
            let err = src.read("f.parquet").await.unwrap_err();
            assert!(err.to_string().contains(&format!("status {edge}")), "{err}");
            assert_eq!(http.gets(), 4, "{edge}");
        }
    }

    #[tokio::test]
    async fn http_source_never_retries_an_origin_status() {
        // Any other answer is final: one request each.
        for (status, absent) in [(500, false), (403, false), (404, true), (410, true)] {
            let (http, src) = flaky_source(0, status);
            let r = src.read("f.parquet").await;
            if absent {
                assert!(matches!(r, Ok(None)), "{status}");
            } else {
                assert!(r.is_err(), "{status}");
            }
            assert_eq!(http.gets(), 1, "status {status}");
        }
    }

    #[tokio::test]
    async fn a_hash_mismatch_is_not_retried() {
        // A tampered file is fetched once and fails; the retry layer only
        // sees transport errors.
        let dir = tempfile::tempdir().unwrap();
        let ms = chain(dir.path());
        let day: Day = ms[2].manifest.date.parse().unwrap();
        let m = MockHttp::new();
        let base = "https://seed.example/hyperliquid-normalized";
        let root = dir.path().to_path_buf();
        let tampered = ms[2].manifest.files[0].path.clone();
        let t2 = tampered.clone();
        m.handle(base, move |r| {
            let rel = r.url[base.len() + 1..].to_owned();
            if rel == encode_path(&t2) {
                return Response {
                    status: 200,
                    body: Bytes::from_static(b"not the signed bytes"),
                };
            }
            match std::fs::read(root.join(&rel)) {
                Ok(b) => Response {
                    status: 200,
                    body: Bytes::from(b),
                },
                Err(_) => Response {
                    status: 404,
                    body: Bytes::new(),
                },
            }
        });
        let http = Arc::new(FlakyHttp::new(m.clone(), 0));
        let src = HttpSource::new(http, base).with_retries(4, std::time::Duration::ZERO);
        let mut o = opts(1);
        o.files = FileCheck::Hashes;
        let r = verify_day(&src, "hyperliquid-normalized", day, &o)
            .await
            .unwrap();
        assert!(!r.ok());
        let target = join_url(base, &encode_path(&tampered));
        let fetched = m.requests().iter().filter(|q| q.url == target).count();
        assert_eq!(fetched, 1);
    }
}
