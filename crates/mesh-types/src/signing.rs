//! secp256k1 signatures, Ethereum style, and per-block commitments.
//!
//! The mesh signs with secp256k1 so that Ethereum contracts can verify
//! signatures with `ecrecover`. Both block commitments and daily manifests
//! use this scheme.
//!
//! - A signer is identified by its Ethereum address ([`EthAddress`]): the
//!   last 20 bytes of `keccak256` of the uncompressed public key without its
//!   `0x04` prefix.
//! - A signature ([`EthSignature`]) is 65 bytes `r || s || v`, with low `s`
//!   (EIP-2) and `v` in `{27, 28}`.
//! - What is signed is always a 32-byte hash `h`, wrapped as an EIP-191
//!   personal message: `keccak256("\x19Ethereum Signed Message:\n32" || h)`.
//!   In Solidity: `ECDSA.recover(MessageHashUtils.toEthSignedMessageHash(h), sig)`.
//!
//! # Block commitments
//!
//! For each block and stream ([`StreamId`]: venue and record kind) the
//! publisher computes the Merkle root of the block's records of that kind
//! across all instruments (ordered by instrument, then `seq`; see
//! [`crate::merkle`]) and signs:
//!
//! ```text
//! h = keccak256( "bleh-block-commitment-v0"
//!                || uint8(len(venue)) || venue
//!                || uint8(len(kind))  || kind
//!                || uint64_be(block_number)
//!                || uint32_be(leaf_count)
//!                || merkle_root )
//! ```
//!
//! which is `keccak256(abi.encodePacked(...))` with those types in
//! Solidity. A client checks a record by recomputing its leaf, verifying a
//! [`MerkleProof`] against the commitment's root, and verifying the
//! commitment's signature against a published signer address.

use std::fmt;
use std::str::FromStr;

use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::VenueId;
use crate::merkle::{H256, HexParseError, MerkleProof, merkle_root, parse_0x, record_leaf};
use crate::record::{Record, RecordKind};

/// Domain tag of the block-commitment preimage.
pub const BLOCK_COMMITMENT_DOMAIN: &[u8] = b"bleh-block-commitment-v0";

/// Errors from signing and verification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SigError {
    /// The private key is not a valid secp256k1 scalar.
    #[error("invalid secp256k1 private key")]
    InvalidKey,
    /// The signature is malformed, has high `s`, or a bad `v`.
    #[error("malformed signature")]
    Malformed,
    /// No public key could be recovered.
    #[error("public key recovery failed")]
    Recovery,
    /// The signature is valid but from a different signer.
    #[error("signed by {recovered}, expected {expected}")]
    WrongSigner {
        /// Address the commitment claims.
        expected: EthAddress,
        /// Address recovered from the signature.
        recovered: EthAddress,
    },
    /// The commitment's record count or root does not match.
    #[error("commitment does not match: {0}")]
    Mismatch(&'static str),
    /// A record could not be canonically encoded.
    #[error("canonical encoding failed: {0}")]
    Encoding(String),
}

macro_rules! hex_bytes {
    ($(#[$doc:meta])* $name:ident, $n:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub [u8; $n]);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "0x{}", crate::hex::encode(&self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({self})", stringify!($name))
            }
        }

        impl FromStr for $name {
            type Err = HexParseError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                parse_0x(s).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_bytes!(
    /// An Ethereum address. Text form: `0x` and 40 **lowercase** hex digits
    /// (no EIP-55 checksum casing), so each address has one text form.
    EthAddress,
    20
);

hex_bytes!(
    /// A 65-byte `r || s || v` signature. Text form: `0x` and 130 lowercase
    /// hex digits.
    EthSignature,
    65
);

/// EIP-191 personal-message hash of an arbitrary message:
/// `keccak256("\x19Ethereum Signed Message:\n" || len || msg)`.
#[must_use]
pub fn eth_personal_message_hash(msg: &[u8]) -> H256 {
    let prefix = format!("\x19Ethereum Signed Message:\n{}", msg.len());
    H256::keccak(&[prefix.as_bytes(), msg])
}

/// EIP-191 personal-message hash of a 32-byte hash (the form every mesh
/// signature uses).
#[must_use]
pub fn eth_signed_hash(h: &H256) -> H256 {
    eth_personal_message_hash(&h.0)
}

fn address_of(vk: &VerifyingKey) -> EthAddress {
    let point = vk.to_sec1_point(false);
    let bytes = point.as_bytes();
    // Uncompressed SEC1: 0x04 || x || y.
    let h = H256::keccak(&[&bytes[1..]]);
    let mut a = [0u8; 20];
    a.copy_from_slice(&h.0[12..]);
    EthAddress(a)
}

/// A secp256k1 private key.
#[derive(Clone)]
pub struct SigningKey(k256::ecdsa::SigningKey);

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({})", self.address())
    }
}

impl SigningKey {
    /// Loads a key from its 32-byte big-endian scalar.
    ///
    /// # Errors
    ///
    /// Fails if the bytes are zero or not below the curve order.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, SigError> {
        k256::ecdsa::SigningKey::from_slice(bytes)
            .map(Self)
            .map_err(|_| SigError::InvalidKey)
    }

    /// The key's Ethereum address.
    #[must_use]
    pub fn address(&self) -> EthAddress {
        address_of(self.0.verifying_key())
    }

    /// Signs a raw 32-byte digest (RFC 6979 deterministic nonce, low `s`).
    #[must_use]
    pub fn sign_prehash(&self, digest: &H256) -> EthSignature {
        let (sig, recid) = self.0.sign_prehash_recoverable(&digest.0);
        let (sig, recid) = match sig.normalize_s() {
            n if n == sig => (sig, recid),
            n => (n, RecoveryId::new(!recid.is_y_odd(), recid.is_x_reduced())),
        };
        let mut out = [0u8; 65];
        out[..64].copy_from_slice(&sig.to_bytes());
        out[64] = 27 + u8::from(recid);
        EthSignature(out)
    }

    /// Signs a 32-byte hash as an EIP-191 personal message.
    #[must_use]
    pub fn sign_hash(&self, h: &H256) -> EthSignature {
        self.sign_prehash(&eth_signed_hash(h))
    }
}

impl EthSignature {
    /// Recovers the signer of a raw 32-byte digest.
    ///
    /// # Errors
    ///
    /// Fails on a malformed or high-`s` signature, or if recovery fails.
    pub fn recover_prehash(&self, digest: &H256) -> Result<EthAddress, SigError> {
        let v = self.0[64];
        if v != 27 && v != 28 {
            return Err(SigError::Malformed);
        }
        let sig = Signature::from_slice(&self.0[..64]).map_err(|_| SigError::Malformed)?;
        if sig.normalize_s() != sig {
            return Err(SigError::Malformed);
        }
        let recid = RecoveryId::try_from(v - 27).map_err(|_| SigError::Malformed)?;
        let vk = VerifyingKey::recover_from_prehash(&digest.0, &sig, recid)
            .map_err(|_| SigError::Recovery)?;
        Ok(address_of(&vk))
    }

    /// Recovers the signer of a 32-byte hash signed as an EIP-191 personal
    /// message.
    ///
    /// # Errors
    ///
    /// See [`EthSignature::recover_prehash`].
    pub fn recover_hash(&self, h: &H256) -> Result<EthAddress, SigError> {
        self.recover_prehash(&eth_signed_hash(h))
    }

    /// Checks that `expected` signed `h` as an EIP-191 personal message.
    ///
    /// # Errors
    ///
    /// Fails on a malformed signature or a different signer.
    pub fn verify_hash(&self, h: &H256, expected: &EthAddress) -> Result<(), SigError> {
        let recovered = self.recover_hash(h)?;
        if recovered == *expected {
            Ok(())
        } else {
            Err(SigError::WrongSigner {
                expected: *expected,
                recovered,
            })
        }
    }
}

/// A committed stream: one venue and one record kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamId {
    /// Venue.
    pub venue: VenueId,
    /// Record kind.
    pub kind: RecordKind,
}

/// A signed Merkle root over one block's records of one stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockCommitment {
    /// The stream.
    pub stream: StreamId,
    /// Block number.
    pub block_number: u64,
    /// Number of records (leaves).
    pub leaf_count: u32,
    /// Merkle root of the records' leaf hashes.
    pub merkle_root: H256,
    /// Address of the signer.
    pub signer: EthAddress,
    /// Signature over [`BlockCommitment::commitment_hash`].
    pub signature: EthSignature,
}

impl BlockCommitment {
    /// The hash that is signed (see the module docs).
    #[must_use]
    pub fn commitment_hash(
        stream: &StreamId,
        block_number: u64,
        leaf_count: u32,
        root: &H256,
    ) -> H256 {
        let venue = stream.venue.as_str().as_bytes();
        let kind = stream.kind.as_str().as_bytes();
        // Both are short ASCII by construction (VenueId <= 32 bytes).
        let venue_len = [u8::try_from(venue.len()).unwrap_or(u8::MAX)];
        let kind_len = [u8::try_from(kind.len()).unwrap_or(u8::MAX)];
        H256::keccak(&[
            BLOCK_COMMITMENT_DOMAIN,
            &venue_len,
            venue,
            &kind_len,
            kind,
            &block_number.to_be_bytes(),
            &leaf_count.to_be_bytes(),
            &root.0,
        ])
    }

    /// Computes leaves for `records` (which must be the block's records of
    /// this stream, in instrument-then-seq order) and signs the root.
    ///
    /// # Errors
    ///
    /// Fails if a record cannot be encoded or there are more than
    /// `u32::MAX` records.
    pub fn sign(
        key: &SigningKey,
        stream: StreamId,
        block_number: u64,
        records: &[Record],
    ) -> Result<Self, SigError> {
        let leaves = records
            .iter()
            .map(record_leaf)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SigError::Encoding(e.to_string()))?;
        let leaf_count =
            u32::try_from(leaves.len()).map_err(|_| SigError::Mismatch("too many records"))?;
        let root = merkle_root(&leaves);
        let h = Self::commitment_hash(&stream, block_number, leaf_count, &root);
        Ok(Self {
            stream,
            block_number,
            leaf_count,
            merkle_root: root,
            signer: key.address(),
            signature: key.sign_hash(&h),
        })
    }

    /// Verifies the signature against the `signer` field. Callers must also
    /// check that `signer` is a published, unrevoked key.
    ///
    /// # Errors
    ///
    /// Fails on a malformed signature or a different signer.
    pub fn verify_signature(&self) -> Result<(), SigError> {
        let h = Self::commitment_hash(
            &self.stream,
            self.block_number,
            self.leaf_count,
            &self.merkle_root,
        );
        self.signature.verify_hash(&h, &self.signer)
    }

    /// Verifies that `record` is included in this commitment.
    ///
    /// # Errors
    ///
    /// Fails if the record's kind, venue or block differ from the
    /// commitment's, the proof's leaf count differs, or the proof fails.
    pub fn verify_inclusion(&self, record: &Record, proof: &MerkleProof) -> Result<(), SigError> {
        let prov = record.prov();
        if record.kind() != self.stream.kind
            || prov.venue != self.stream.venue
            || prov.seq.block_number != self.block_number
        {
            return Err(SigError::Mismatch("record is not in this stream and block"));
        }
        if proof.leaf_count != self.leaf_count {
            return Err(SigError::Mismatch("proof leaf count"));
        }
        let leaf = record_leaf(record).map_err(|e| SigError::Encoding(e.to_string()))?;
        if proof.verify(&leaf, &self.merkle_root) {
            Ok(())
        } else {
            Err(SigError::Mismatch("inclusion proof"))
        }
    }

    /// Verifies the commitment against the complete list of records (for
    /// clients that received the whole stream for the block).
    ///
    /// # Errors
    ///
    /// Fails if the count or root differ, or the signature is invalid.
    pub fn verify_records(&self, records: &[Record]) -> Result<(), SigError> {
        let leaves = records
            .iter()
            .map(record_leaf)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SigError::Encoding(e.to_string()))?;
        if u32::try_from(leaves.len()).ok() != Some(self.leaf_count) {
            return Err(SigError::Mismatch("record count"));
        }
        if merkle_root(&leaves) != self.merkle_root {
            return Err(SigError::Mismatch("merkle root"));
        }
        self.verify_signature()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::trade;

    // Well-known example key from the web3.js documentation (test only).
    const DOC_KEY: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";

    fn doc_key() -> SigningKey {
        SigningKey::from_bytes(&crate::hex::decode_array(DOC_KEY).unwrap()).unwrap()
    }

    #[test]
    fn address_matches_known_vector() {
        assert_eq!(
            doc_key().address().to_string(),
            "0x2c7536e3605d9c16a7a3d7b1898e529396a65c23"
        );
    }

    #[test]
    fn personal_sign_matches_known_vector() {
        let h = eth_personal_message_hash(b"Some data");
        assert_eq!(
            h.to_string(),
            "0x1da44b586eb0729ff70a73c326926f6ed5a25f5b056e7f47fbc6e58d86871655"
        );
        let sig = doc_key().sign_prehash(&h);
        assert_eq!(
            sig.to_string(),
            "0xb91467e570a6466aa9e9876cbcd013baba02900b8979d43fe208a4a4f339f5fd6007e74cd82e037b800186422fc2da167c747ef045e5d18a5f5d4300f8e1a0291c"
        );
        assert_eq!(sig.recover_prehash(&h).unwrap(), doc_key().address());
    }

    #[test]
    fn sign_and_recover_hash() {
        let key = SigningKey::from_bytes(&[7u8; 32]).unwrap();
        for i in 0..20u8 {
            let h = H256::keccak(&[&[i]]);
            let sig = key.sign_hash(&h);
            assert!(sig.0[64] == 27 || sig.0[64] == 28);
            sig.verify_hash(&h, &key.address()).unwrap();
            let other = H256::keccak(&[b"other"]);
            assert!(sig.verify_hash(&other, &key.address()).is_err());
        }
        assert_eq!(
            SigningKey::from_bytes(&[0u8; 32]).err(),
            Some(SigError::InvalidKey)
        );
        assert_eq!(
            SigningKey::from_bytes(&[0xff; 32]).err(),
            Some(SigError::InvalidKey)
        );
    }

    #[test]
    fn rejects_bad_v_and_high_s() {
        let key = SigningKey::from_bytes(&[9u8; 32]).unwrap();
        let h = H256::keccak(&[b"x"]);
        let mut sig = key.sign_hash(&h);
        sig.0[64] = 1;
        assert_eq!(sig.recover_hash(&h), Err(SigError::Malformed));
        // Flip s to n - s (high s): must be rejected even though it is a
        // mathematically valid signature.
        let mut sig = key.sign_hash(&h);
        let s = Signature::from_slice(&sig.0[..64]).unwrap();
        let (r, s_bytes) = s.split_bytes();
        let s_scalar =
            <k256::Scalar as k256::elliptic_curve::PrimeField>::from_repr(s_bytes).unwrap();
        let high = Signature::from_scalars(r, (-s_scalar).to_bytes()).unwrap();
        sig.0[..64].copy_from_slice(&high.to_bytes());
        sig.0[64] ^= 1;
        assert_eq!(sig.recover_hash(&h), Err(SigError::Malformed));
    }

    #[test]
    fn block_commitment_round_trip() {
        let key = SigningKey::from_bytes(&[3u8; 32]).unwrap();
        let records: Vec<Record> = (0..5).map(|i| trade(42, i).into()).collect();
        let stream = StreamId {
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
        };
        let c = BlockCommitment::sign(&key, stream, 42, &records).unwrap();
        c.verify_signature().unwrap();
        c.verify_records(&records).unwrap();

        let leaves: Vec<H256> = records.iter().map(|r| record_leaf(r).unwrap()).collect();
        for (i, r) in records.iter().enumerate() {
            let proof = MerkleProof::build(&leaves, i).unwrap();
            c.verify_inclusion(r, &proof).unwrap();
            let wrong = MerkleProof::build(&leaves, (i + 1) % 5).unwrap();
            assert!(c.verify_inclusion(r, &wrong).is_err());
        }
        assert!(c.verify_records(&records[..4]).is_err());
        let mut tampered = records.clone();
        if let Record::Trade(t) = &mut tampered[2] {
            t.trade_id = crate::TradeId(1);
        }
        assert!(c.verify_records(&tampered).is_err());

        let mut forged = c.clone();
        forged.block_number = 43;
        assert!(matches!(
            forged.verify_signature(),
            Err(SigError::WrongSigner { .. })
        ));

        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<BlockCommitment>(&json).unwrap(), c);
    }

    #[test]
    fn commitment_hash_layout() {
        let stream = StreamId {
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::BookEvent,
        };
        let root = H256([0xab; 32]);
        let mut pre = Vec::new();
        pre.extend_from_slice(b"bleh-block-commitment-v0");
        pre.push(11);
        pre.extend_from_slice(b"hyperliquid");
        pre.push(10);
        pre.extend_from_slice(b"book_event");
        pre.extend_from_slice(&7u64.to_be_bytes());
        pre.extend_from_slice(&3u32.to_be_bytes());
        pre.extend_from_slice(&[0xab; 32]);
        assert_eq!(
            BlockCommitment::commitment_hash(&stream, 7, 3, &root),
            H256::keccak(&[&pre])
        );
    }
}
