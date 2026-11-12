//! Daily archive manifests and their anchors.
//!
//! Specified in `docs/specs/manifest-v0.md`. In brief:
//!
//! - A [`Manifest`] lists every file of one archive day with its size,
//!   SHA-256, canonical content hash (normalized partitions), IPFS CID and
//!   BitTorrent v2 piece root, plus the day's torrent infohash, magnet URI
//!   and web-seed base URLs. It links to the previous day's manifest by
//!   hash.
//! - It is signed with secp256k1, Ethereum style ([`crate::signing`]):
//!   `h = keccak256("bleh-manifest-v0\n" || canonical_json(manifest))`,
//!   signed as an EIP-191 personal message. Key rotation: a manifest may
//!   name `next_signer`; the following manifest may be signed by either.
//! - The manifest hash used for chaining and anchoring is
//!   `SHA-256(canonical_json(signed_manifest))`.
//! - A manifest may carry [`SourceNotice`]s: the notices that sources
//!   admitted under the open-terms route require to travel with their
//!   data (ADR 0012, decision 5). The list is omitted when empty, so
//!   manifests without notices keep their encoding and signature.
//! - [`ManifestAnchors`] live beside the signed manifest (anchors are made
//!   after signing): an OpenTimestamps proof and an Ethereum L2
//!   transaction, both committing to the manifest hash.
//!
//! Verification order: signature, then hash chain, then anchors
//! ([`verify_signed`], [`verify_link`], [`ManifestAnchors::check`]).
//! Checking the OpenTimestamps proof and the L2 transaction themselves
//! needs network access and is done by the archive crate's verifier.

use serde::{Deserialize, Serialize};

use crate::hash::{CanonicalError, ContentHash, Sha256Digest, canonical_json};
use crate::ids::{AdmissionId, VenueId};
use crate::merkle::H256;
use crate::signing::{EthAddress, EthSignature, SigError, SigningKey};
use crate::time::UnixNanos;

/// Value of [`Manifest::format`].
pub const MANIFEST_FORMAT: &str = "bleh-manifest-v0";
/// Value of [`ManifestAnchors::format`].
pub const ANCHORS_FORMAT: &str = "bleh-anchors-v0";
/// Domain prefix of the manifest signing preimage.
pub const MANIFEST_SIGNING_DOMAIN: &[u8] = b"bleh-manifest-v0\n";

/// IPFS DAG codec of a file's CID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IpfsCodec {
    /// Single raw block (file no larger than one chunk).
    Raw,
    /// UnixFS DAG (chunked file).
    DagPb,
}

/// How a file is addressed on IPFS. The CID depends on the chunking
/// parameters, so they are recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IpfsRef {
    /// CIDv1, base32 (`bafk...` for raw, `bafy...` for dag-pb).
    pub cid: String,
    /// DAG codec.
    pub codec: IpfsCodec,
    /// Chunker, such as `size-1048576`.
    pub chunker: String,
    /// Whether UnixFS leaves are raw blocks.
    pub raw_leaves: bool,
}

/// One file of an archive day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFile {
    /// Path relative to the day's root, `/`-separated.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the file's bytes. The authoritative integrity check.
    pub sha256: Sha256Digest,
    /// Canonical content hash of the records (normalized partitions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<ContentHash>,
    /// Number of records (normalized partitions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_count: Option<u64>,
    /// IPFS address.
    pub ipfs: IpfsRef,
    /// BitTorrent v2 per-file piece root (SHA-256 Merkle root over 16 KiB
    /// blocks), for checking a single file fetched from the torrent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bt_pieces_root: Option<Sha256Digest>,
}

/// Longest notice text accepted, in bytes.
pub const MAX_NOTICE_BYTES: usize = 4096;

/// A notice a source requires to travel with its data (ADR 0012,
/// decision 5), for example the New York Fed's reference-rates notice.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceNotice {
    /// Admission covering the records the notice applies to.
    pub admission: AdmissionId,
    /// The records' `prov.venue`.
    pub source: VenueId,
    /// The notice text, verbatim as the admission record requires it.
    pub notice: String,
}

/// The day's BitTorrent v2 torrent, containing every listed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TorrentInfo {
    /// v2 infohash (SHA-256 of the bencoded info dictionary).
    pub infohash_v2: Sha256Digest,
    /// Magnet URI (`magnet:?xt=urn:btmh:1220<infohash>&dn=...&ws=...`).
    pub magnet: String,
}

/// The signed body of a daily manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always [`MANIFEST_FORMAT`].
    pub format: String,
    /// Archive name, such as `hyperliquid-raw` or `hyperliquid-normalized`.
    pub archive: String,
    /// UTC day, `YYYY-MM-DD`.
    pub date: String,
    /// Position in this archive's chain; 0 for the first manifest.
    pub sequence: u64,
    /// When the manifest was built.
    pub created_ns: UnixNanos,
    /// Hash of the previous signed manifest; `None` only at sequence 0.
    pub previous_manifest_sha256: Option<Sha256Digest>,
    /// Files, sorted by path.
    pub files: Vec<ManifestFile>,
    /// The day's torrent.
    pub torrent: TorrentInfo,
    /// HTTPS base URLs serving the day's files (BEP 19 web seeds).
    pub web_seeds: Vec<String>,
    /// Address that signs this manifest.
    pub signer: EthAddress,
    /// Address that may sign the next manifest instead (key rotation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_signer: Option<EthAddress>,
    /// Source notices for the archive's records, sorted by admission and
    /// source, unique. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<SourceNotice>,
}

/// A manifest with its signature. This is the file published as
/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifest {
    /// The signed body.
    pub manifest: Manifest,
    /// Signature by `manifest.signer`.
    pub signature: EthSignature,
}

/// Errors from manifest validation and verification.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// A structural rule is broken.
    #[error("invalid manifest: {0}")]
    Invalid(String),
    /// Signature verification failed.
    #[error("signature: {0}")]
    Signature(#[from] SigError),
    /// The chain link to the previous manifest is broken.
    #[error("chain: {0}")]
    Chain(String),
    /// Canonical encoding failed.
    #[error(transparent)]
    Encoding(#[from] CanonicalError),
}

fn invalid(msg: impl Into<String>) -> ManifestError {
    ManifestError::Invalid(msg.into())
}

fn is_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn is_safe_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 512
        && !p.starts_with('/')
        && p.split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && p.bytes().all(|c| c.is_ascii_graphic() && c != b'\\')
}

impl Manifest {
    /// Checks structural rules (see `docs/specs/manifest-v0.md`).
    ///
    /// # Errors
    ///
    /// Returns the first broken rule.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.format != MANIFEST_FORMAT {
            return Err(invalid("unknown format"));
        }
        if !crate::VenueId::new(self.archive.clone()).is_ok_and(|_| self.archive.len() <= 32) {
            return Err(invalid("archive must be a slug"));
        }
        if !is_day(&self.date) {
            return Err(invalid("date must be YYYY-MM-DD"));
        }
        if (self.sequence == 0) != self.previous_manifest_sha256.is_none() {
            return Err(invalid(
                "previous_manifest_sha256 is required except at sequence 0",
            ));
        }
        if self.files.is_empty() {
            return Err(invalid("no files"));
        }
        for w in self.files.windows(2) {
            if w[0].path >= w[1].path {
                return Err(invalid("files must be sorted by path and unique"));
            }
        }
        for f in &self.files {
            if !is_safe_path(&f.path) {
                return Err(invalid(format!("unsafe path {:?}", f.path)));
            }
            let prefix = match f.ipfs.codec {
                IpfsCodec::Raw => "bafk",
                IpfsCodec::DagPb => "bafy",
            };
            if !f.ipfs.cid.starts_with(prefix)
                || !f
                    .ipfs
                    .cid
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c))
            {
                return Err(invalid(format!(
                    "{}: CID must be base32 CIDv1 ({prefix}...)",
                    f.path
                )));
            }
            if f.content_hash.is_some() != f.record_count.is_some() {
                return Err(invalid(format!(
                    "{}: content_hash and record_count go together",
                    f.path
                )));
            }
        }
        let xt = format!("xt=urn:btmh:1220{}", self.torrent.infohash_v2);
        if !self.torrent.magnet.starts_with("magnet:?") || !self.torrent.magnet.contains(&xt) {
            return Err(invalid("magnet must carry the v2 infohash"));
        }
        if self.web_seeds.is_empty()
            || !self
                .web_seeds
                .iter()
                .all(|u| u.starts_with("https://") && u.ends_with('/'))
        {
            return Err(invalid("web_seeds must be https base URLs ending in /"));
        }
        for w in self.notices.windows(2) {
            if (&w[0].admission, &w[0].source) >= (&w[1].admission, &w[1].source) {
                return Err(invalid(
                    "notices must be sorted by admission and source and unique",
                ));
            }
        }
        if let Some(n) = self.notices.iter().find(|n| {
            n.notice.trim().is_empty()
                || n.notice.len() > MAX_NOTICE_BYTES
                || n.notice.chars().any(char::is_control)
        }) {
            return Err(invalid(format!(
                "notice for {} {} must be 1 to {MAX_NOTICE_BYTES} bytes of printable text",
                n.admission, n.source
            )));
        }
        Ok(())
    }

    /// The 32-byte hash that is signed.
    ///
    /// # Errors
    ///
    /// Fails if canonical encoding fails.
    pub fn signing_hash(&self) -> Result<H256, ManifestError> {
        let body = canonical_json(self)?;
        Ok(H256::keccak(&[MANIFEST_SIGNING_DOMAIN, &body]))
    }

    /// Validates and signs. `key` must match `signer`.
    ///
    /// # Errors
    ///
    /// Fails if the manifest is invalid or the key is not the signer.
    pub fn sign(self, key: &SigningKey) -> Result<SignedManifest, ManifestError> {
        self.validate()?;
        if key.address() != self.signer {
            return Err(invalid("signing key does not match signer"));
        }
        let signature = key.sign_hash(&self.signing_hash()?);
        Ok(SignedManifest {
            manifest: self,
            signature,
        })
    }
}

impl SignedManifest {
    /// Canonical bytes of the whole signed manifest.
    ///
    /// # Errors
    ///
    /// Fails if canonical encoding fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        Ok(canonical_json(self)?)
    }

    /// The manifest hash: SHA-256 of [`SignedManifest::canonical_bytes`].
    /// Used by the next manifest's `previous_manifest_sha256` and by
    /// anchors.
    ///
    /// # Errors
    ///
    /// Fails if canonical encoding fails.
    pub fn sha256(&self) -> Result<Sha256Digest, ManifestError> {
        Ok(Sha256Digest::of(&self.canonical_bytes()?))
    }
}

/// Step 1: structure and signature. `trusted` is the set of signer
/// addresses the verifier accepts for this manifest (from the registry, or
/// carried forward through [`verify_link`]).
///
/// # Errors
///
/// Fails if the manifest is invalid, its signer is not trusted, or the
/// signature does not verify.
pub fn verify_signed(signed: &SignedManifest, trusted: &[EthAddress]) -> Result<(), ManifestError> {
    let m = &signed.manifest;
    m.validate()?;
    if !trusted.contains(&m.signer) {
        return Err(ManifestError::Signature(SigError::Mismatch(
            "signer is not trusted",
        )));
    }
    signed
        .signature
        .verify_hash(&m.signing_hash()?, &m.signer)?;
    Ok(())
}

/// Step 2: the chain link from `prev` to `next`, including key rotation.
/// Both must already have passed [`verify_signed`].
///
/// # Errors
///
/// Fails if the archive, sequence, date order, previous hash or signer
/// handover is wrong.
pub fn verify_link(prev: &SignedManifest, next: &SignedManifest) -> Result<(), ManifestError> {
    let (p, n) = (&prev.manifest, &next.manifest);
    let chain = |m: &str| Err(ManifestError::Chain(m.into()));
    if p.archive != n.archive {
        return chain("archive differs");
    }
    if p.sequence.checked_add(1) != Some(n.sequence) {
        return chain("sequence must increase by one");
    }
    if n.date <= p.date {
        return chain("date must increase");
    }
    if n.previous_manifest_sha256 != Some(prev.sha256()?) {
        return chain("previous_manifest_sha256 does not match");
    }
    if n.signer != p.signer && Some(n.signer) != p.next_signer {
        return chain("signer is neither the previous signer nor its announced successor");
    }
    Ok(())
}

/// Whether an anchor is final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorStatus {
    /// Submitted, not yet final (OpenTimestamps calendar promise; L2
    /// transaction not yet final).
    Pending,
    /// Final (proof upgraded to a Bitcoin attestation; L2 transaction
    /// final).
    Confirmed,
}

/// OpenTimestamps anchor over the manifest hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OtsAnchor {
    /// Path of the `.ots` proof beside the manifest.
    pub proof_path: String,
    /// SHA-256 of the `.ots` file.
    pub proof_sha256: Sha256Digest,
    /// Status.
    pub status: AnchorStatus,
    /// Bitcoin block height of the attestation, once confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitcoin_block_height: Option<u64>,
}

/// Ethereum L2 anchor: a transaction that records the manifest hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvmAnchor {
    /// EIP-155 chain ID.
    pub chain_id: u64,
    /// Transaction hash.
    pub tx_hash: H256,
    /// Block that includes the transaction.
    pub block_number: u64,
    /// The 32 bytes committed on chain (must equal the manifest hash).
    pub committed_hash: H256,
    /// Status.
    pub status: AnchorStatus,
}

/// Anchors for one signed manifest, published beside it as
/// `manifest.anchors.json`. Not signed (anchors are made after signing);
/// each anchor proves itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestAnchors {
    /// Always [`ANCHORS_FORMAT`].
    pub format: String,
    /// [`SignedManifest::sha256`] of the anchored manifest.
    pub manifest_sha256: Sha256Digest,
    /// Bitcoin anchor via OpenTimestamps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opentimestamps: Option<OtsAnchor>,
    /// Ethereum L2 anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ethereum_l2: Option<EvmAnchor>,
}

/// Outcome of the local anchor check for one anchor type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorFinding {
    /// Present and claims confirmation; the proof itself must still be
    /// verified externally, and a failure there is an error.
    ConfirmedClaimed,
    /// Present but pending: a warning.
    Pending,
    /// Absent: a warning while within the confirmation window, an error
    /// after it.
    Missing,
}

/// Local anchor check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorReport {
    /// Bitcoin (OpenTimestamps).
    pub bitcoin: AnchorFinding,
    /// Ethereum L2.
    pub ethereum_l2: AnchorFinding,
}

impl AnchorReport {
    /// Whether the report is acceptable. Within the confirmation window
    /// (`within_window`), missing or pending anchors are warnings; after
    /// it, both anchors must be confirmed.
    #[must_use]
    pub fn acceptable(&self, within_window: bool) -> bool {
        within_window
            || (self.bitcoin == AnchorFinding::ConfirmedClaimed
                && self.ethereum_l2 == AnchorFinding::ConfirmedClaimed)
    }
}

impl ManifestAnchors {
    /// Step 3 (local part): checks that the anchors refer to `signed` and
    /// are internally consistent, and reports each anchor's state.
    ///
    /// # Errors
    ///
    /// Fails if the format is unknown, the anchors name a different
    /// manifest, the L2 anchor commits to a different hash, or a confirmed
    /// OpenTimestamps anchor lacks its Bitcoin height.
    pub fn check(&self, signed: &SignedManifest) -> Result<AnchorReport, ManifestError> {
        if self.format != ANCHORS_FORMAT {
            return Err(invalid("unknown anchors format"));
        }
        let hash = signed.sha256()?;
        if self.manifest_sha256 != hash {
            return Err(invalid("anchors refer to a different manifest"));
        }
        let bitcoin = match &self.opentimestamps {
            None => AnchorFinding::Missing,
            Some(a) if a.status == AnchorStatus::Pending => AnchorFinding::Pending,
            Some(a) => {
                if a.bitcoin_block_height.is_none() {
                    return Err(invalid(
                        "confirmed OpenTimestamps anchor needs bitcoin_block_height",
                    ));
                }
                AnchorFinding::ConfirmedClaimed
            }
        };
        let ethereum_l2 = match &self.ethereum_l2 {
            None => AnchorFinding::Missing,
            Some(a) => {
                if a.committed_hash.0 != hash.0 {
                    return Err(invalid("L2 anchor commits to a different hash"));
                }
                match a.status {
                    AnchorStatus::Pending => AnchorFinding::Pending,
                    AnchorStatus::Confirmed => AnchorFinding::ConfirmedClaimed,
                }
            }
        };
        Ok(AnchorReport {
            bitcoin,
            ethereum_l2,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(b: u8) -> Sha256Digest {
        Sha256Digest([b; 32])
    }

    fn file(path: &str) -> ManifestFile {
        ManifestFile {
            path: path.into(),
            size: 1234,
            sha256: Sha256Digest::of(path.as_bytes()),
            content_hash: None,
            record_count: None,
            ipfs: IpfsRef {
                cid: "bafkreiaaaa".into(),
                codec: IpfsCodec::Raw,
                chunker: "size-1048576".into(),
                raw_leaves: true,
            },
            bt_pieces_root: Some(digest(9)),
        }
    }

    fn manifest(key: &SigningKey, seq: u64, date: &str, prev: Option<Sha256Digest>) -> Manifest {
        let infohash = digest(5);
        Manifest {
            format: MANIFEST_FORMAT.into(),
            archive: "hyperliquid-normalized".into(),
            date: date.into(),
            sequence: seq,
            created_ns: UnixNanos(1_767_312_000_000_000_000),
            previous_manifest_sha256: prev,
            files: vec![file("kind=trade/a.parquet"), file("kind=trade/b.parquet")],
            torrent: TorrentInfo {
                infohash_v2: infohash,
                magnet: format!("magnet:?xt=urn:btmh:1220{infohash}&dn=day"),
            },
            web_seeds: vec!["https://mirror.example.org/archive/".into()],
            signer: key.address(),
            next_signer: None,
            notices: Vec::new(),
        }
    }

    #[test]
    fn notices_are_optional_and_checked() {
        let k1 = SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let m = manifest(&k1, 0, "2026-01-01", None);
        let json = String::from_utf8(canonical_json(&m).unwrap()).unwrap();
        assert!(!json.contains("notices"), "empty notices are omitted");
        let notice = |adm: &str, src: &str, text: &str| SourceNotice {
            admission: AdmissionId::new(adm).unwrap(),
            source: VenueId::new(src).unwrap(),
            notice: text.into(),
        };
        let mut m2 = m.clone();
        m2.notices = vec![
            notice("ADM-9001", "example-a", "Example notice A."),
            notice("ADM-9002", "example-b", "Example notice B."),
        ];
        m2.validate().unwrap();
        let signed = m2.clone().sign(&k1).unwrap();
        verify_signed(&signed, &[k1.address()]).unwrap();
        let back: SignedManifest =
            serde_json::from_slice(&canonical_json(&signed).unwrap()).unwrap();
        assert_eq!(back.manifest.notices, m2.notices);
        let mut bad = m2.clone();
        bad.notices.reverse();
        assert!(bad.validate().is_err(), "unsorted");
        let mut bad = m2.clone();
        bad.notices[0].notice = " ".into();
        assert!(bad.validate().is_err(), "empty text");
        let mut bad = m2;
        bad.notices[0].notice = "line\nbreak".into();
        assert!(bad.validate().is_err(), "control character");
    }

    #[test]
    fn sign_verify_and_chain() {
        let k1 = SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let k2 = SigningKey::from_bytes(&[2u8; 32]).unwrap();

        let mut m0 = manifest(&k1, 0, "2026-01-01", None);
        m0.next_signer = Some(k2.address());
        let s0 = m0.sign(&k1).unwrap();
        verify_signed(&s0, &[k1.address()]).unwrap();
        assert!(verify_signed(&s0, &[k2.address()]).is_err());

        // Rotation: the next manifest is signed by the announced key.
        let s1 = manifest(&k2, 1, "2026-01-02", Some(s0.sha256().unwrap()))
            .sign(&k2)
            .unwrap();
        verify_signed(&s1, &[k1.address(), k2.address()]).unwrap();
        verify_link(&s0, &s1).unwrap();

        // An unannounced key breaks the chain.
        let k3 = SigningKey::from_bytes(&[3u8; 32]).unwrap();
        let s1_bad = manifest(&k3, 1, "2026-01-02", Some(s0.sha256().unwrap()))
            .sign(&k3)
            .unwrap();
        assert!(verify_link(&s0, &s1_bad).is_err());

        // A wrong previous hash breaks the chain.
        let s1_hash = manifest(&k2, 1, "2026-01-02", Some(digest(0)))
            .sign(&k2)
            .unwrap();
        assert!(verify_link(&s0, &s1_hash).is_err());

        // Tampering with a signed field breaks the signature.
        let mut tampered = s1.clone();
        tampered.manifest.files[0].size += 1;
        assert!(verify_signed(&tampered, &[k2.address()]).is_err());

        let json = serde_json::to_string(&s1).unwrap();
        assert_eq!(serde_json::from_str::<SignedManifest>(&json).unwrap(), s1);
    }

    #[test]
    fn structural_rules() {
        let k = SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let ok = manifest(&k, 0, "2026-01-01", None);
        ok.validate().unwrap();

        let mut m = ok.clone();
        m.files.reverse();
        assert!(m.validate().is_err());
        let mut m = ok.clone();
        m.files[0].path = "../etc/passwd".into();
        assert!(m.validate().is_err());
        let mut m = ok.clone();
        m.files[0].ipfs.codec = IpfsCodec::DagPb;
        assert!(m.validate().is_err());
        let mut m = ok.clone();
        m.previous_manifest_sha256 = Some(digest(1));
        assert!(m.validate().is_err());
        let mut m = ok.clone();
        m.torrent.magnet = "magnet:?xt=urn:btih:abc".into();
        assert!(m.validate().is_err());
        let mut m = ok.clone();
        m.web_seeds = vec!["http://insecure.example.org/".into()];
        assert!(m.validate().is_err());
        let mut m = ok.clone();
        m.files[0].content_hash = Some(ContentHash(digest(3)));
        assert!(m.validate().is_err());
        m.files[0].record_count = Some(10);
        m.validate().unwrap();
        let other = SigningKey::from_bytes(&[2u8; 32]).unwrap();
        assert!(ok.sign(&other).is_err());
    }

    #[test]
    fn anchors() {
        let k = SigningKey::from_bytes(&[1u8; 32]).unwrap();
        let s = manifest(&k, 0, "2026-01-01", None).sign(&k).unwrap();
        let h = s.sha256().unwrap();
        let mut a = ManifestAnchors {
            format: ANCHORS_FORMAT.into(),
            manifest_sha256: h,
            opentimestamps: None,
            ethereum_l2: None,
        };
        let r = a.check(&s).unwrap();
        assert_eq!(r.bitcoin, AnchorFinding::Missing);
        assert!(r.acceptable(true));
        assert!(!r.acceptable(false));

        a.opentimestamps = Some(OtsAnchor {
            proof_path: "manifest.json.ots".into(),
            proof_sha256: digest(4),
            status: AnchorStatus::Confirmed,
            bitcoin_block_height: Some(900_000),
        });
        a.ethereum_l2 = Some(EvmAnchor {
            chain_id: 8453,
            tx_hash: H256([1; 32]),
            block_number: 1,
            committed_hash: H256(h.0),
            status: AnchorStatus::Confirmed,
        });
        let r = a.check(&s).unwrap();
        assert!(r.acceptable(false));

        let mut wrong = a.clone();
        if let Some(e) = &mut wrong.ethereum_l2 {
            e.committed_hash = H256([0; 32]);
        }
        assert!(wrong.check(&s).is_err());
        let mut wrong = a.clone();
        wrong.manifest_sha256 = digest(0);
        assert!(wrong.check(&s).is_err());
        let mut wrong = a;
        if let Some(o) = &mut wrong.opentimestamps {
            o.bitcoin_block_height = None;
        }
        assert!(wrong.check(&s).is_err());
    }
}
