//! Per-block Merkle trees over canonical record encodings.
//!
//! The tree is the RFC 9162 (Certificate Transparency v2) Merkle tree with
//! Keccak-256 as the hash, so an Ethereum contract can verify inclusion
//! cheaply:
//!
//! ```text
//! leaf(c)        = keccak256(0x00 || c)          c = canonical record bytes
//! node(l, r)     = keccak256(0x01 || l || r)
//! root([])       = keccak256("")
//! root([x])      = leaf(x)
//! root(D[0..n])  = node(root(D[0..k]), root(D[k..n]))   k = largest power of two < n
//! ```
//!
//! The domain bytes `0x00` / `0x01` prevent a leaf being passed off as an
//! inner node. An odd node is promoted, never duplicated, so two different
//! leaf lists cannot share a root. Inclusion proofs ([`MerkleProof`]) are
//! RFC 9162 audit paths and verify with the RFC 9162 §2.1.3.2 algorithm.
//!
//! Leaves are the records of one block and one stream (see
//! [`crate::signing::StreamId`]), ordered by instrument and then `seq`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha3::{Digest, Keccak256};

use crate::hash::{CanonicalError, canonical_record};
use crate::record::Record;

/// A 32-byte Keccak-256 digest. Text form: `0x` and 64 lowercase hex
/// digits, as Ethereum tooling expects.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct H256(pub [u8; 32]);

impl H256 {
    /// Keccak-256 of the concatenation of `parts`.
    #[must_use]
    pub fn keccak(parts: &[&[u8]]) -> Self {
        let mut h = Keccak256::new();
        for p in parts {
            h.update(p);
        }
        let out = h.finalize();
        let mut a = [0u8; 32];
        a.copy_from_slice(&out);
        Self(a)
    }
}

impl fmt::Display for H256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", crate::hex::encode(&self.0))
    }
}

impl fmt::Debug for H256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "H256({self})")
    }
}

/// Error from parsing a `0x`-prefixed hex value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected 0x and {0} lowercase hex digits")]
pub struct HexParseError(pub usize);

pub(crate) fn parse_0x<const N: usize>(s: &str) -> Result<[u8; N], HexParseError> {
    s.strip_prefix("0x")
        .and_then(crate::hex::decode_array)
        .ok_or(HexParseError(N * 2))
}

impl FromStr for H256 {
    type Err = HexParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_0x(s).map(Self)
    }
}

impl Serialize for H256 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for H256 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Leaf hash of canonical record bytes.
#[must_use]
pub fn leaf_hash(canonical: &[u8]) -> H256 {
    H256::keccak(&[&[0x00], canonical])
}

/// Inner-node hash.
#[must_use]
pub fn node_hash(left: &H256, right: &H256) -> H256 {
    H256::keccak(&[&[0x01], &left.0, &right.0])
}

/// Leaf hash of a record (its canonical encoding, see [`crate::hash`]).
///
/// # Errors
///
/// Fails if the record cannot be encoded.
pub fn record_leaf(record: &Record) -> Result<H256, CanonicalError> {
    Ok(leaf_hash(&canonical_record(record)?))
}

/// Largest power of two strictly less than `n` (`n >= 2`).
fn split_point(n: usize) -> usize {
    let mut k = 1;
    while k * 2 < n {
        k *= 2;
    }
    k
}

/// Root over leaf hashes.
#[must_use]
pub fn merkle_root(leaves: &[H256]) -> H256 {
    match leaves.len() {
        0 => H256::keccak(&[]),
        1 => leaves[0],
        n => {
            let k = split_point(n);
            node_hash(&merkle_root(&leaves[..k]), &merkle_root(&leaves[k..]))
        }
    }
}

/// An inclusion proof for one leaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MerkleProof {
    /// Zero-based position of the leaf.
    pub leaf_index: u32,
    /// Number of leaves in the tree.
    pub leaf_count: u32,
    /// Audit path, from the leaf's sibling up to the root's child.
    pub path: Vec<H256>,
}

impl MerkleProof {
    /// Builds the proof for `leaves[index]`, or `None` if out of range or
    /// the tree has more than `u32::MAX` leaves.
    #[must_use]
    pub fn build(leaves: &[H256], index: usize) -> Option<Self> {
        if index >= leaves.len() {
            return None;
        }
        let mut path = Vec::new();
        audit_path(index, leaves, &mut path);
        Some(Self {
            leaf_index: u32::try_from(index).ok()?,
            leaf_count: u32::try_from(leaves.len()).ok()?,
            path,
        })
    }

    /// Verifies that `leaf` is at `leaf_index` in a tree of `leaf_count`
    /// leaves with root `root` (RFC 9162 §2.1.3.2).
    #[must_use]
    pub fn verify(&self, leaf: &H256, root: &H256) -> bool {
        if self.leaf_index >= self.leaf_count {
            return false;
        }
        let mut fn_ = u64::from(self.leaf_index);
        let mut sn = u64::from(self.leaf_count) - 1;
        let mut r = *leaf;
        for p in &self.path {
            if sn == 0 {
                return false;
            }
            if fn_ & 1 == 1 || fn_ == sn {
                r = node_hash(p, &r);
                if fn_ & 1 == 0 {
                    while fn_ & 1 == 0 && fn_ != 0 {
                        fn_ >>= 1;
                        sn >>= 1;
                    }
                }
            } else {
                r = node_hash(&r, p);
            }
            fn_ >>= 1;
            sn >>= 1;
        }
        sn == 0 && r == *root
    }
}

fn audit_path(m: usize, leaves: &[H256], out: &mut Vec<H256>) {
    let n = leaves.len();
    if n <= 1 {
        return;
    }
    let k = split_point(n);
    if m < k {
        audit_path(m, &leaves[..k], out);
        out.push(merkle_root(&leaves[k..]));
    } else {
        audit_path(m - k, &leaves[k..], out);
        out.push(merkle_root(&leaves[..k]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(n: usize) -> Vec<H256> {
        (0..n)
            .map(|i| leaf_hash(format!("record-{i}").as_bytes()))
            .collect()
    }

    #[test]
    fn small_trees_match_the_definition() {
        let l = leaves(3);
        assert_eq!(merkle_root(&l[..1]), l[0]);
        assert_eq!(merkle_root(&l[..2]), node_hash(&l[0], &l[1]));
        assert_eq!(merkle_root(&l), node_hash(&node_hash(&l[0], &l[1]), &l[2]));
        assert_eq!(merkle_root(&[]), H256::keccak(&[]));
        // keccak256("") is the well-known constant.
        assert_eq!(
            H256::keccak(&[]).to_string(),
            "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn every_proof_verifies_for_many_sizes() {
        for n in 1..=33 {
            let l = leaves(n);
            let root = merkle_root(&l);
            for i in 0..n {
                let proof = MerkleProof::build(&l, i).unwrap();
                assert!(proof.verify(&l[i], &root), "n={n} i={i}");
                // Wrong leaf, wrong root, wrong index all fail.
                let other = leaf_hash(b"other");
                assert!(!proof.verify(&other, &root));
                assert!(!proof.verify(&l[i], &other));
                if n > 1 {
                    let mut moved = proof.clone();
                    moved.leaf_index = u32::try_from((i + 1) % n).unwrap();
                    assert!(!moved.verify(&l[i], &root), "n={n} i={i}");
                }
            }
        }
    }

    #[test]
    fn tampered_proofs_fail() {
        let l = leaves(10);
        let root = merkle_root(&l);
        let proof = MerkleProof::build(&l, 6).unwrap();
        let mut short = proof.clone();
        short.path.pop();
        assert!(!short.verify(&l[6], &root));
        let mut long = proof.clone();
        long.path.push(l[0]);
        assert!(!long.verify(&l[6], &root));
        let mut bad_count = proof.clone();
        bad_count.leaf_count = 6;
        assert!(!bad_count.verify(&l[6], &root));
        assert!(MerkleProof::build(&l, 10).is_none());
    }

    #[test]
    fn duplicated_last_leaf_changes_the_root() {
        let l = leaves(3);
        let mut dup = l.clone();
        dup.push(l[2]);
        assert_ne!(merkle_root(&l), merkle_root(&dup));
    }

    #[test]
    fn h256_text_form() {
        let h = leaf_hash(b"x");
        let s = h.to_string();
        assert!(s.starts_with("0x") && s.len() == 66);
        assert_eq!(s.parse::<H256>().unwrap(), h);
        assert!(s[2..].parse::<H256>().is_err());
        let json = serde_json::to_string(&MerkleProof::build(&leaves(4), 1).unwrap()).unwrap();
        let back: MerkleProof = serde_json::from_str(&json).unwrap();
        assert_eq!(back.path.len(), 2);
    }
}
