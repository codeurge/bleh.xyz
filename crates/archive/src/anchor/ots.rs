//! OpenTimestamps proofs: the `.ots` format, calendar submission and
//! upgrade over HTTPS, and verification against Bitcoin block headers.
//!
//! Format (python-opentimestamps, the reference implementation): the
//! detached file is a magic header, version 1, the file-hash operation
//! (`0x08`, SHA-256) and the 32-byte digest, then a timestamp tree. Each
//! tree node holds attestations and operations; every child but the last is
//! prefixed by `0xff`; an attestation is `0x00`, an 8-byte tag and a
//! length-prefixed payload; an operation is its tag byte (binary operations
//! with a length-prefixed argument) followed by the child node. Integers are
//! unsigned LEB128.
//!
//! The timestamped digest is the manifest hash, which is also the SHA-256 of
//! `manifest.json` (written as canonical JSON), so the stock `ots verify
//! manifest.json.ots` works too.

use std::sync::Arc;

use futures::future::BoxFuture;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::hex;
use crate::http::{HttpClient, join_url};

/// Magic bytes at the start of a detached timestamp file.
pub const HEADER_MAGIC: &[u8] =
    b"\x00OpenTimestamps\x00\x00Proof\x00\xbf\x89\xe2\xe8\x84\xe8\x92\x94";
const MAJOR_VERSION: u64 = 1;
const MAX_MSG_LEN: usize = 4096;
const MAX_URI_LEN: usize = 1000;
const MAX_PAYLOAD_LEN: usize = 8192;
const RECURSION_LIMIT: usize = 256;

const TAG_PENDING: [u8; 8] = [0x83, 0xdf, 0xe3, 0x0d, 0x2e, 0xf9, 0x0c, 0x8e];
const TAG_BITCOIN: [u8; 8] = [0x05, 0x88, 0x96, 0x0d, 0x73, 0xd7, 0x19, 0x01];

/// Public calendars used by default (`ots stamp` defaults).
pub const DEFAULT_CALENDARS: &[&str] = &[
    "https://a.pool.opentimestamps.org",
    "https://b.pool.opentimestamps.org",
    "https://a.pool.eternitywall.com",
    "https://ots.btc.catallaxy.com",
];

/// Calendars an upgrade may contact (pending attestations name the
/// calendar that will hold the upgrade; contacting arbitrary URLs from a
/// downloaded proof is refused).
pub const DEFAULT_UPGRADE_WHITELIST: &[&str] = &[
    "https://alice.btc.calendar.opentimestamps.org",
    "https://bob.btc.calendar.opentimestamps.org",
    "https://finney.calendar.eternitywall.com",
    "https://ots.btc.catallaxy.com",
    "https://a.pool.opentimestamps.org",
    "https://b.pool.opentimestamps.org",
    "https://a.pool.eternitywall.com",
];

/// A default Esplora-compatible Bitcoin API.
pub const DEFAULT_BITCOIN_API: &str = "https://blockstream.info/api";

fn bad(msg: impl Into<String>) -> Error {
    Error::Ots(msg.into())
}

/// A timestamp operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    /// SHA-1 (tag 0x02). Deprecated; parsed and evaluated for old proofs.
    Sha1,
    /// RIPEMD-160 (0x03).
    Ripemd160,
    /// SHA-256 (0x08).
    Sha256,
    /// Keccak-256 (0x67).
    Keccak256,
    /// Append the argument (0xf0).
    Append(Vec<u8>),
    /// Prepend the argument (0xf1).
    Prepend(Vec<u8>),
    /// Reverse the bytes (0xf2).
    Reverse,
    /// Lowercase hex encoding (0xf3).
    Hexlify,
}

impl Op {
    fn tag(&self) -> u8 {
        match self {
            Op::Sha1 => 0x02,
            Op::Ripemd160 => 0x03,
            Op::Sha256 => 0x08,
            Op::Keccak256 => 0x67,
            Op::Append(_) => 0xf0,
            Op::Prepend(_) => 0xf1,
            Op::Reverse => 0xf2,
            Op::Hexlify => 0xf3,
        }
    }

    fn sort_key(&self) -> (u8, &[u8]) {
        match self {
            Op::Append(a) | Op::Prepend(a) => (self.tag(), a),
            _ => (self.tag(), &[]),
        }
    }

    /// Applies the operation.
    ///
    /// # Errors
    ///
    /// Fails if the result would exceed the maximum message length.
    pub fn apply(&self, msg: &[u8]) -> Result<Vec<u8>> {
        let out = match self {
            Op::Sha1 => {
                use sha1::Digest as _;
                sha1::Sha1::digest(msg).to_vec()
            }
            Op::Ripemd160 => {
                use ripemd::Digest as _;
                ripemd::Ripemd160::digest(msg).to_vec()
            }
            Op::Sha256 => Sha256::digest(msg).to_vec(),
            Op::Keccak256 => mesh_types::H256::keccak(&[msg]).0.to_vec(),
            Op::Append(a) => [msg, a.as_slice()].concat(),
            Op::Prepend(a) => [a.as_slice(), msg].concat(),
            Op::Reverse => msg.iter().rev().copied().collect(),
            Op::Hexlify => hex::encode(msg).into_bytes(),
        };
        if out.len() > MAX_MSG_LEN {
            return Err(bad("operation result exceeds 4096 bytes"));
        }
        Ok(out)
    }
}

/// An attestation at a leaf of the timestamp tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Attestation {
    /// A calendar's promise to include the commitment; upgrade later.
    Pending(String),
    /// The commitment is the Merkle root of the Bitcoin block at this height.
    Bitcoin(u64),
    /// Any other attestation, kept verbatim.
    Unknown {
        /// Tag.
        tag: [u8; 8],
        /// Payload.
        payload: Vec<u8>,
    },
}

impl Attestation {
    fn tag(&self) -> [u8; 8] {
        match self {
            Attestation::Pending(_) => TAG_PENDING,
            Attestation::Bitcoin(_) => TAG_BITCOIN,
            Attestation::Unknown { tag, .. } => *tag,
        }
    }

    fn payload(&self) -> Vec<u8> {
        let mut p = Vec::new();
        match self {
            Attestation::Pending(uri) => put_varbytes(&mut p, uri.as_bytes()),
            Attestation::Bitcoin(h) => put_varuint(&mut p, *h),
            Attestation::Unknown { payload, .. } => p.extend_from_slice(payload),
        }
        p
    }

    fn sort_key(&self) -> ([u8; 8], Vec<u8>) {
        match self {
            Attestation::Pending(u) => (TAG_PENDING, u.as_bytes().to_vec()),
            Attestation::Bitcoin(h) => (TAG_BITCOIN, h.to_be_bytes().to_vec()),
            Attestation::Unknown { tag, payload } => (*tag, payload.clone()),
        }
    }
}

/// A node of the timestamp tree: its message, attestations and children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timestamp {
    /// The message at this node.
    pub msg: Vec<u8>,
    /// Attestations on `msg`.
    pub attestations: Vec<Attestation>,
    /// Operations applied to `msg`, each with the resulting node.
    pub ops: Vec<(Op, Timestamp)>,
}

impl Timestamp {
    /// An empty node for `msg`.
    #[must_use]
    pub fn new(msg: Vec<u8>) -> Self {
        Self {
            msg,
            attestations: Vec::new(),
            ops: Vec::new(),
        }
    }

    /// Merges `other` (a timestamp for the same message) into `self`.
    ///
    /// # Errors
    ///
    /// Fails if the messages differ.
    pub fn merge(&mut self, other: Timestamp) -> Result<()> {
        if self.msg != other.msg {
            return Err(bad("cannot merge timestamps for different messages"));
        }
        for a in other.attestations {
            if !self.attestations.contains(&a) {
                self.attestations.push(a);
            }
        }
        for (op, child) in other.ops {
            if let Some((_, mine)) = self.ops.iter_mut().find(|(o, _)| *o == op) {
                mine.merge(child)?;
            } else {
                self.ops.push((op, child));
            }
        }
        Ok(())
    }

    /// Every attestation with the message it attests, depth first.
    #[must_use]
    pub fn all_attestations(&self) -> Vec<(&[u8], &Attestation)> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<(&'a [u8], &'a Attestation)>) {
        for a in &self.attestations {
            out.push((&self.msg, a));
        }
        for (_, c) in &self.ops {
            c.collect(out);
        }
    }

    /// Merges `upgrade` into the first node whose message is `commitment`.
    /// Returns whether such a node was found.
    ///
    /// # Errors
    ///
    /// Fails if `upgrade` is for another message.
    pub fn merge_at(&mut self, commitment: &[u8], upgrade: &Timestamp) -> Result<bool> {
        if self.msg == commitment {
            self.merge(upgrade.clone())?;
            return Ok(true);
        }
        for (_, c) in &mut self.ops {
            if c.merge_at(commitment, upgrade)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Serializes the tree (without the file header).
    pub fn serialize(&self, out: &mut Vec<u8>) {
        let mut atts: Vec<&Attestation> = self.attestations.iter().collect();
        atts.sort_by_key(|a| a.sort_key());
        let mut ops: Vec<&(Op, Timestamp)> = self.ops.iter().collect();
        ops.sort_by(|a, b| a.0.sort_key().cmp(&b.0.sort_key()));
        let items = atts.len() + ops.len();
        let mut written = 0;
        for a in atts {
            written += 1;
            if written < items {
                out.push(0xff);
            }
            out.push(0x00);
            out.extend_from_slice(&a.tag());
            put_varbytes(out, &a.payload());
        }
        for (op, child) in ops {
            written += 1;
            if written < items {
                out.push(0xff);
            }
            out.push(op.tag());
            if let Op::Append(arg) | Op::Prepend(arg) = op {
                put_varbytes(out, arg);
            }
            child.serialize(out);
        }
    }

    /// Parses a tree for `msg` from `data` (all of it).
    ///
    /// # Errors
    ///
    /// Fails on malformed input or trailing bytes.
    pub fn parse(data: &[u8], msg: Vec<u8>) -> Result<Self> {
        let mut r = Reader { data, pos: 0 };
        let t = parse_node(&mut r, msg, RECURSION_LIMIT)?;
        if r.pos != data.len() {
            return Err(bad("trailing bytes after timestamp"));
        }
        Ok(t)
    }
}

fn put_varuint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_varbytes(out: &mut Vec<u8>, b: &[u8]) {
    put_varuint(out, b.len() as u64);
    out.extend_from_slice(b);
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or_else(|| bad("truncated proof"))?;
        self.pos += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| bad("truncated proof"))?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn varuint(&mut self) -> Result<u64> {
        let mut v: u64 = 0;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift >= 64 {
                return Err(bad("varuint overflow"));
            }
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    fn varbytes(&mut self, max: usize) -> Result<Vec<u8>> {
        let n = usize::try_from(self.varuint()?).map_err(|_| bad("length"))?;
        if n > max {
            return Err(bad(format!("length {n} exceeds {max}")));
        }
        Ok(self.bytes(n)?.to_vec())
    }
}

fn parse_attestation(r: &mut Reader<'_>) -> Result<Attestation> {
    let tag: [u8; 8] = r.bytes(8)?.try_into().map_err(|_| bad("tag"))?;
    let payload = r.varbytes(MAX_PAYLOAD_LEN)?;
    let mut p = Reader {
        data: &payload,
        pos: 0,
    };
    let att = match tag {
        TAG_PENDING => {
            let uri = p.varbytes(MAX_URI_LEN)?;
            let uri = String::from_utf8(uri).map_err(|_| bad("pending URI is not UTF-8"))?;
            if !uri
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-._/:".contains(&c))
            {
                return Err(bad("pending URI has invalid characters"));
            }
            Attestation::Pending(uri)
        }
        TAG_BITCOIN => Attestation::Bitcoin(p.varuint()?),
        _ => {
            p.pos = payload.len();
            Attestation::Unknown {
                tag,
                payload: payload.clone(),
            }
        }
    };
    if p.pos != payload.len() {
        return Err(bad("trailing bytes in attestation payload"));
    }
    Ok(att)
}

fn parse_op(r: &mut Reader<'_>, tag: u8) -> Result<Op> {
    Ok(match tag {
        0x02 => Op::Sha1,
        0x03 => Op::Ripemd160,
        0x08 => Op::Sha256,
        0x67 => Op::Keccak256,
        0xf0 => Op::Append(r.varbytes(MAX_MSG_LEN)?),
        0xf1 => Op::Prepend(r.varbytes(MAX_MSG_LEN)?),
        0xf2 => Op::Reverse,
        0xf3 => Op::Hexlify,
        t => return Err(bad(format!("unknown operation tag 0x{t:02x}"))),
    })
}

fn parse_node(r: &mut Reader<'_>, msg: Vec<u8>, depth: usize) -> Result<Timestamp> {
    if depth == 0 {
        return Err(bad("timestamp nesting too deep"));
    }
    let mut t = Timestamp::new(msg);
    loop {
        let mut tag = r.byte()?;
        let more = tag == 0xff;
        if more {
            tag = r.byte()?;
        }
        if tag == 0x00 {
            t.attestations.push(parse_attestation(r)?);
        } else {
            let op = parse_op(r, tag)?;
            let child_msg = op.apply(&t.msg)?;
            let child = parse_node(r, child_msg, depth - 1)?;
            t.ops.push((op, child));
        }
        if !more {
            return Ok(t);
        }
    }
}

/// A detached timestamp (`.ots` file) over a SHA-256 digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedTimestamp {
    /// The timestamp tree; its root message is the file digest.
    pub timestamp: Timestamp,
}

impl DetachedTimestamp {
    /// A proof with no attestations yet for `digest`.
    #[must_use]
    pub fn new(digest: [u8; 32]) -> Self {
        Self {
            timestamp: Timestamp::new(digest.to_vec()),
        }
    }

    /// The file digest.
    #[must_use]
    pub fn digest(&self) -> &[u8] {
        &self.timestamp.msg
    }

    /// Serializes to `.ots` bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = HEADER_MAGIC.to_vec();
        put_varuint(&mut out, MAJOR_VERSION);
        out.push(0x08);
        out.extend_from_slice(&self.timestamp.msg);
        self.timestamp.serialize(&mut out);
        out
    }

    /// Parses `.ots` bytes (SHA-256 file hash only).
    ///
    /// # Errors
    ///
    /// Fails on a bad header, an unsupported version or file-hash
    /// operation, or a malformed tree.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let rest = data
            .strip_prefix(HEADER_MAGIC)
            .ok_or_else(|| bad("not an OpenTimestamps proof (bad magic)"))?;
        let mut r = Reader { data: rest, pos: 0 };
        if r.varuint()? != MAJOR_VERSION {
            return Err(bad("unsupported proof version"));
        }
        if r.byte()? != 0x08 {
            return Err(bad("only SHA-256 file digests are supported"));
        }
        let digest = r.bytes(32)?.to_vec();
        let ts = Timestamp::parse(&rest[r.pos..], digest)?;
        Ok(Self { timestamp: ts })
    }

    /// Pending attestations as (calendar URI, commitment).
    #[must_use]
    pub fn pending(&self) -> Vec<(String, Vec<u8>)> {
        self.timestamp
            .all_attestations()
            .into_iter()
            .filter_map(|(m, a)| match a {
                Attestation::Pending(u) => Some((u.clone(), m.to_vec())),
                _ => None,
            })
            .collect()
    }

    /// Bitcoin attestations as (height, attested 32-byte message).
    #[must_use]
    pub fn bitcoin(&self) -> Vec<(u64, Vec<u8>)> {
        self.timestamp
            .all_attestations()
            .into_iter()
            .filter_map(|(m, a)| match a {
                Attestation::Bitcoin(h) => Some((*h, m.to_vec())),
                _ => None,
            })
            .collect()
    }

    /// Structural verification: the digest is `expected`, there is at least
    /// one attestation, and every Bitcoin attestation is on a 32-byte
    /// message. (Parsing already checked the encoding and recomputed every
    /// message.)
    ///
    /// # Errors
    ///
    /// Fails on the first broken rule.
    pub fn check(&self, expected: &[u8; 32]) -> Result<()> {
        if self.digest() != expected {
            return Err(bad("proof is for a different digest"));
        }
        let atts = self.timestamp.all_attestations();
        if atts.is_empty() {
            return Err(bad("proof has no attestations"));
        }
        for (m, a) in atts {
            if matches!(a, Attestation::Bitcoin(_)) && m.len() != 32 {
                return Err(bad("Bitcoin attestation on a message that is not 32 bytes"));
            }
        }
        Ok(())
    }
}

/// Source of Bitcoin block headers.
pub trait BitcoinHeaders: Send + Sync + std::fmt::Debug {
    /// The 80-byte header of the block at `height` on the best chain.
    fn header(&self, height: u64) -> BoxFuture<'_, Result<[u8; 80]>>;
}

/// [`BitcoinHeaders`] over an Esplora-compatible HTTP API
/// (`/block-height/<h>`, `/block/<hash>/header`).
#[derive(Debug, Clone)]
pub struct EsploraHeaders {
    http: Arc<dyn HttpClient>,
    base: String,
}

impl EsploraHeaders {
    /// A client for the API at `base` (such as [`DEFAULT_BITCOIN_API`]).
    pub fn new(http: Arc<dyn HttpClient>, base: impl Into<String>) -> Self {
        Self {
            http,
            base: base.into(),
        }
    }
}

/// Double SHA-256 of a block header, in display (reversed) byte order.
#[must_use]
pub fn block_hash_hex(header: &[u8; 80]) -> String {
    let h = Sha256::digest(Sha256::digest(header));
    let mut b: Vec<u8> = h.to_vec();
    b.reverse();
    hex::encode(&b)
}

impl BitcoinHeaders for EsploraHeaders {
    fn header(&self, height: u64) -> BoxFuture<'_, Result<[u8; 80]>> {
        Box::pin(async move {
            let url = join_url(&self.base, &format!("block-height/{height}"));
            let hash = self
                .http
                .get(&url, &[])
                .await?
                .into_ok("bitcoin block hash")?;
            let hash = String::from_utf8_lossy(&hash).trim().to_ascii_lowercase();
            if hash.len() != 64 || hex::decode(&hash).is_none() {
                return Err(bad("bitcoin API returned a malformed block hash"));
            }
            let url = join_url(&self.base, &format!("block/{hash}/header"));
            let body = self
                .http
                .get(&url, &[])
                .await?
                .into_ok("bitcoin block header")?;
            let header: [u8; 80] = hex::decode(String::from_utf8_lossy(&body).trim())
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| bad("bitcoin API returned a malformed header"))?;
            if block_hash_hex(&header) != hash {
                return Err(bad("bitcoin header does not hash to the block hash"));
            }
            Ok(header)
        })
    }
}

/// A verified Bitcoin attestation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitcoinConfirmation {
    /// Block height.
    pub height: u64,
    /// Block time (Unix seconds) from the header.
    pub block_time: u32,
}

/// Verifies every Bitcoin attestation in `proof` against block headers.
/// Returns the confirmations (empty if the proof has none).
///
/// # Errors
///
/// Fails if a header cannot be fetched or an attested message is not the
/// block's Merkle root.
pub async fn verify_bitcoin(
    proof: &DetachedTimestamp,
    headers: &dyn BitcoinHeaders,
) -> Result<Vec<BitcoinConfirmation>> {
    let mut out = Vec::new();
    for (height, msg) in proof.bitcoin() {
        let header = headers.header(height).await?;
        if header[36..68] != msg[..] {
            return Err(bad(format!(
                "attestation at height {height} does not match the block's Merkle root"
            )));
        }
        let block_time = u32::from_le_bytes([header[68], header[69], header[70], header[71]]);
        out.push(BitcoinConfirmation { height, block_time });
    }
    Ok(out)
}

/// Client for OpenTimestamps calendars.
#[derive(Debug, Clone)]
pub struct Calendars {
    http: Arc<dyn HttpClient>,
    /// Calendars to submit to.
    pub urls: Vec<String>,
    /// Calendars an upgrade may contact.
    pub whitelist: Vec<String>,
    /// Minimum successful submissions.
    pub min_responses: usize,
}

const ACCEPT: (&str, &str) = ("Accept", "application/vnd.opentimestamps.v1");

impl Calendars {
    /// A client for `urls` (the defaults if empty). Upgrades may contact the
    /// default whitelist and `urls`; at least two submissions must succeed
    /// (one if only one calendar is configured).
    pub fn new(http: Arc<dyn HttpClient>, urls: Vec<String>) -> Self {
        let urls: Vec<String> = if urls.is_empty() {
            DEFAULT_CALENDARS.iter().map(|s| (*s).to_owned()).collect()
        } else {
            urls
        };
        let mut whitelist: Vec<String> = DEFAULT_UPGRADE_WHITELIST
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        whitelist.extend(urls.iter().cloned());
        let min_responses = urls.len().min(2);
        Self {
            http,
            urls,
            whitelist,
            min_responses,
        }
    }

    fn allowed(&self, uri: &str) -> bool {
        let uri = uri.trim_end_matches('/');
        self.whitelist.iter().any(|w| {
            let w = w.trim_end_matches('/');
            uri == w || uri.starts_with(&format!("{w}/"))
        })
    }

    /// Submits `digest` to every calendar and merges the responses.
    ///
    /// # Errors
    ///
    /// Fails if fewer than `min_responses` calendars answer with a valid
    /// timestamp.
    pub async fn stamp(&self, digest: [u8; 32]) -> Result<DetachedTimestamp> {
        let mut proof = DetachedTimestamp::new(digest);
        let mut ok = 0;
        for url in &self.urls {
            let endpoint = join_url(url, "digest");
            let result = async {
                let body = self
                    .http
                    .post(&endpoint, &[ACCEPT], bytes::Bytes::copy_from_slice(&digest))
                    .await?
                    .into_ok("calendar submit")?;
                Timestamp::parse(&body, digest.to_vec())
            }
            .await;
            match result {
                Ok(ts) => {
                    proof.timestamp.merge(ts)?;
                    ok += 1;
                    crate::metrics::inc("archive_ots_submissions_total", &[("result", "ok")], 1);
                }
                Err(e) => {
                    tracing::warn!(calendar = %url, error = %e, "calendar submission failed");
                    crate::metrics::inc("archive_ots_submissions_total", &[("result", "error")], 1);
                }
            }
        }
        if ok < self.min_responses.max(1) {
            return Err(bad(format!(
                "only {ok} of {} calendars accepted the digest (need {})",
                self.urls.len(),
                self.min_responses.max(1)
            )));
        }
        Ok(proof)
    }

    /// Asks each pending attestation's calendar for an upgrade and merges
    /// what it returns. Returns whether the proof changed.
    ///
    /// # Errors
    ///
    /// Fails if an upgrade response is malformed. Unreachable calendars and
    /// "not yet" answers leave the proof unchanged.
    pub async fn upgrade(&self, proof: &mut DetachedTimestamp) -> Result<bool> {
        let mut changed = false;
        for (uri, commitment) in proof.pending() {
            if !self.allowed(&uri) {
                tracing::warn!(calendar = %uri, "pending attestation names a calendar outside the whitelist; skipped");
                continue;
            }
            let url = join_url(&uri, &format!("timestamp/{}", hex::encode(&commitment)));
            let resp = match self.http.get(&url, &[ACCEPT]).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(calendar = %uri, error = %e, "upgrade request failed");
                    continue;
                }
            };
            if !resp.ok() {
                tracing::info!(calendar = %uri, status = resp.status, "not upgraded yet");
                continue;
            }
            let upgrade = Timestamp::parse(&resp.body, commitment.clone())?;
            let before = proof.to_bytes();
            proof.timestamp.merge_at(&commitment, &upgrade)?;
            changed |= proof.to_bytes() != before;
        }
        Ok(changed)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::http::MockHttp;

    /// Builds a calendar-style response for `digest`: append a nonce,
    /// SHA-256, then a pending attestation for `uri`.
    pub(crate) fn calendar_response(digest: &[u8], uri: &str) -> (Vec<u8>, Vec<u8>) {
        let nonce = b"nonce-0123456789".to_vec();
        let commitment = Sha256::digest([digest, &nonce].concat()).to_vec();
        let mut out = vec![0xf0];
        put_varbytes(&mut out, &nonce);
        out.push(0x08);
        out.push(0x00);
        out.extend_from_slice(&TAG_PENDING);
        let mut payload = Vec::new();
        put_varbytes(&mut payload, uri.as_bytes());
        put_varbytes(&mut out, &payload);
        (out, commitment)
    }

    /// An upgrade response for `commitment`: prepend, SHA-256, reverse, then
    /// a Bitcoin attestation at `height`. Returns the bytes and the attested
    /// 32-byte message.
    pub(crate) fn upgrade_response(commitment: &[u8], height: u64) -> (Vec<u8>, Vec<u8>) {
        let pre = vec![0xaa; 32];
        let mut attested = Sha256::digest([pre.as_slice(), commitment].concat()).to_vec();
        attested.reverse();
        let mut out = vec![0xf1];
        put_varbytes(&mut out, &pre);
        out.push(0x08);
        out.push(0xf2);
        out.push(0x00);
        out.extend_from_slice(&TAG_BITCOIN);
        let mut payload = Vec::new();
        put_varuint(&mut payload, height);
        put_varbytes(&mut out, &payload);
        (out, attested)
    }

    /// A fake 80-byte header whose Merkle root is `root`.
    pub(crate) fn header_with_root(root: &[u8]) -> [u8; 80] {
        let mut h = [0u8; 80];
        h[0] = 1;
        h[36..68].copy_from_slice(root);
        h[68..72].copy_from_slice(&1_767_300_000u32.to_le_bytes());
        h
    }

    #[test]
    fn varuint_vectors() {
        for (v, want) in [
            (0u64, vec![0x00]),
            (1, vec![0x01]),
            (127, vec![0x7f]),
            (128, vec![0x80, 0x01]),
            (300, vec![0xac, 0x02]),
        ] {
            let mut out = Vec::new();
            put_varuint(&mut out, v);
            assert_eq!(out, want);
            assert_eq!(Reader { data: &out, pos: 0 }.varuint().unwrap(), v);
        }
    }

    #[test]
    fn hand_built_proof_round_trips() {
        // A proof built byte by byte from the format description: header,
        // version 1, SHA-256 file hash op, digest, then two children of the
        // root (0xff-prefixed first): a pending attestation and an
        // append+sha256 path to a Bitcoin attestation at height 2.
        let digest = [0x11u8; 32];
        let mut bytes = HEADER_MAGIC.to_vec();
        bytes.extend_from_slice(&[0x01, 0x08]);
        bytes.extend_from_slice(&digest);
        bytes.extend_from_slice(&[0xff, 0x00]);
        bytes.extend_from_slice(&TAG_PENDING);
        let uri = b"https://alice.btc.calendar.opentimestamps.org";
        bytes.push(uri.len() as u8 + 1);
        bytes.push(uri.len() as u8);
        bytes.extend_from_slice(uri);
        bytes.extend_from_slice(&[0xf0, 0x01, 0xab, 0x08, 0x00]);
        bytes.extend_from_slice(&TAG_BITCOIN);
        bytes.extend_from_slice(&[0x01, 0x02]);

        let p = DetachedTimestamp::parse(&bytes).unwrap();
        assert_eq!(p.to_bytes(), bytes);
        p.check(&digest).unwrap();
        assert!(p.check(&[0u8; 32]).is_err());
        assert_eq!(
            p.pending(),
            vec![(String::from_utf8(uri.to_vec()).unwrap(), digest.to_vec())]
        );
        let leaf = Sha256::digest([digest.as_slice(), &[0xab]].concat()).to_vec();
        assert_eq!(p.bitcoin(), vec![(2, leaf)]);

        // Malformed inputs.
        assert!(DetachedTimestamp::parse(&bytes[..bytes.len() - 1]).is_err());
        assert!(DetachedTimestamp::parse(&[bytes.as_slice(), &[0]].concat()).is_err());
        assert!(DetachedTimestamp::parse(b"not a proof").is_err());
        let mut bad_op = bytes.clone();
        let i = bad_op.len() - 12;
        bad_op[i] = 0x99;
        assert!(DetachedTimestamp::parse(&bad_op).is_err());
    }

    #[test]
    fn ops_apply() {
        assert_eq!(Op::Reverse.apply(b"abc").unwrap(), b"cba");
        assert_eq!(Op::Hexlify.apply(&[0xab]).unwrap(), b"ab");
        assert_eq!(Op::Append(b"x".to_vec()).apply(b"a").unwrap(), b"ax");
        assert_eq!(Op::Prepend(b"x".to_vec()).apply(b"a").unwrap(), b"xa");
        assert_eq!(
            hex::encode(&Op::Ripemd160.apply(b"").unwrap()),
            "9c1185a5c5e9fc54612808977ee8f548b2258d31"
        );
        assert_eq!(
            hex::encode(&Op::Sha1.apply(b"abc").unwrap()),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex::encode(&Op::Keccak256.apply(b"").unwrap()),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
        assert!(Op::Append(vec![0; 4096]).apply(b"a").is_err());
    }

    #[tokio::test]
    async fn stamp_upgrade_and_verify_against_bitcoin() {
        let http = MockHttp::new();
        let digest = [0x42u8; 32];
        let (resp_a, commit_a) =
            calendar_response(&digest, "https://alice.btc.calendar.opentimestamps.org");
        let (resp_b, _) = calendar_response(&digest, "https://bob.btc.calendar.opentimestamps.org");
        http.route(
            "POST",
            "https://a.pool.opentimestamps.org/digest",
            200,
            resp_a,
        );
        http.route(
            "POST",
            "https://b.pool.opentimestamps.org/digest",
            200,
            resp_b,
        );
        let cal = Calendars::new(
            Arc::new(http.clone()),
            vec![
                "https://a.pool.opentimestamps.org".into(),
                "https://b.pool.opentimestamps.org".into(),
            ],
        );
        let mut proof = cal.stamp(digest).await.unwrap();
        // Both responses share the nonce op, so they merge into one branch.
        assert_eq!(proof.pending().len(), 2);
        proof.check(&digest).unwrap();
        assert_eq!(http.requests()[0].body.as_ref(), &digest);

        // Not ready yet: unchanged.
        assert!(!cal.upgrade(&mut proof).await.unwrap());
        let (up, attested) = upgrade_response(&commit_a, 900_000);
        http.route(
            "GET",
            &format!(
                "https://alice.btc.calendar.opentimestamps.org/timestamp/{}",
                hex::encode(&commit_a)
            ),
            200,
            up,
        );
        assert!(cal.upgrade(&mut proof).await.unwrap());
        assert_eq!(proof.bitcoin(), vec![(900_000, attested.clone())]);
        let reparsed = DetachedTimestamp::parse(&proof.to_bytes()).unwrap();
        assert_eq!(reparsed.to_bytes(), proof.to_bytes());

        // Bitcoin header lookup over a mocked Esplora API.
        let header = header_with_root(&attested);
        let hash = block_hash_hex(&header);
        http.route(
            "GET",
            "https://btc.example/api/block-height/900000",
            200,
            hash.clone(),
        );
        http.route(
            "GET",
            &format!("https://btc.example/api/block/{hash}/header"),
            200,
            hex::encode(&header),
        );
        let headers = EsploraHeaders::new(Arc::new(http.clone()), "https://btc.example/api");
        let conf = verify_bitcoin(&proof, &headers).await.unwrap();
        assert_eq!(
            conf,
            vec![BitcoinConfirmation {
                height: 900_000,
                block_time: 1_767_300_000
            }]
        );

        // A header with another Merkle root fails.
        let wrong = header_with_root(&[0u8; 32]);
        let wrong_hash = block_hash_hex(&wrong);
        http.route(
            "GET",
            "https://btc.example/api/block-height/900000",
            200,
            wrong_hash.clone(),
        );
        http.route(
            "GET",
            &format!("https://btc.example/api/block/{wrong_hash}/header"),
            200,
            hex::encode(&wrong),
        );
        assert!(verify_bitcoin(&proof, &headers).await.is_err());
        // A header that does not hash to the claimed block hash fails.
        http.route(
            "GET",
            &format!("https://btc.example/api/block/{wrong_hash}/header"),
            200,
            hex::encode(&header),
        );
        assert!(verify_bitcoin(&proof, &headers).await.is_err());
    }

    #[tokio::test]
    async fn stamp_needs_enough_calendars_and_upgrade_respects_whitelist() {
        let http = MockHttp::new();
        let digest = [1u8; 32];
        let (resp, commit) = calendar_response(&digest, "https://evil.example");
        http.route(
            "POST",
            "https://a.pool.opentimestamps.org/digest",
            200,
            resp,
        );
        let cal = Calendars::new(
            Arc::new(http.clone()),
            vec![
                "https://a.pool.opentimestamps.org".into(),
                "https://down.example".into(),
            ],
        );
        assert!(cal.stamp(digest).await.is_err());
        let one = Calendars::new(
            Arc::new(http.clone()),
            vec!["https://a.pool.opentimestamps.org".into()],
        );
        let mut proof = one.stamp(digest).await.unwrap();
        let (up, _) = upgrade_response(&commit, 1);
        http.route(
            "GET",
            &format!("https://evil.example/timestamp/{}", hex::encode(&commit)),
            200,
            up,
        );
        assert!(!one.upgrade(&mut proof).await.unwrap());
        assert!(
            http.requests()
                .iter()
                .all(|r| !r.url.starts_with("https://evil.example"))
        );
    }
}
