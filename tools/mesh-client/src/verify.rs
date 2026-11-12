//! Commitment and proof verification.
//!
//! - **Signatures** are always checked: the signer recovered from the
//!   commitment must be the commitment's `signer`, and that address must be
//!   trusted: on the configured allowlist (from the registry) if one is
//!   given, otherwise in the gateway's `hello.signers`.
//! - **Roots**: for streams the client receives in full (subscribed to `*`
//!   for that kind), the client recomputes the root from the records of the
//!   block ([`BlockCollector`]).
//! - **Proofs**: for partial subscriptions, the client asks for inclusion
//!   proofs on a sample of records and checks each against the record it
//!   received and the commitment ([`check_proof`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mesh_types::hash::canonical_record;
use mesh_types::{
    BlockBatch, BlockCommitment, EthAddress, MerkleProof, Record, RecordKind, StreamId,
};

/// A verification failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// Signer not trusted.
    #[error("signer {0} is not trusted")]
    UntrustedSigner(EthAddress),
    /// Bad signature.
    #[error("bad signature: {0}")]
    Signature(String),
    /// Recomputed root or count differs.
    #[error("root mismatch for {kind} block {block}: {reason}")]
    Root {
        /// Stream kind.
        kind: RecordKind,
        /// Block.
        block: u64,
        /// Detail.
        reason: String,
    },
    /// The proof does not verify or does not match the received record.
    #[error("proof failed: {0}")]
    Proof(String),
}

/// Trusted signer set.
#[derive(Debug, Clone, Default)]
pub struct SignerSet {
    allowlist: HashSet<EthAddress>,
    announced: HashSet<EthAddress>,
}

impl SignerSet {
    /// A set with an explicit allowlist (empty: trust `hello.signers`).
    #[must_use]
    pub fn new(allowlist: impl IntoIterator<Item = EthAddress>) -> Self {
        Self {
            allowlist: allowlist.into_iter().collect(),
            announced: HashSet::new(),
        }
    }

    /// Records the signers a gateway announced.
    pub fn set_announced(&mut self, signers: &[EthAddress]) {
        self.announced = signers.iter().copied().collect();
    }

    /// True if `a` is trusted.
    #[must_use]
    pub fn trusts(&self, a: &EthAddress) -> bool {
        if self.allowlist.is_empty() {
            self.announced.contains(a)
        } else {
            self.allowlist.contains(a)
        }
    }

    /// Checks the signature and that the signer is trusted.
    ///
    /// # Errors
    ///
    /// [`VerifyError::UntrustedSigner`] or [`VerifyError::Signature`].
    pub fn verify(&self, c: &BlockCommitment) -> Result<(), VerifyError> {
        if !self.trusts(&c.signer) {
            return Err(VerifyError::UntrustedSigner(c.signer));
        }
        c.verify_signature()
            .map_err(|e| VerifyError::Signature(e.to_string()))
    }
}

/// Collects the records of the current block per stream, for streams the
/// client receives in full, and checks them against commitments.
#[derive(Debug, Default)]
pub struct BlockCollector {
    full: HashSet<RecordKind>,
    records: HashMap<(StreamId, u64), Vec<Record>>,
}

impl BlockCollector {
    /// Collects the kinds in `full`.
    #[must_use]
    pub fn new(full: impl IntoIterator<Item = RecordKind>) -> Self {
        Self {
            full: full.into_iter().collect(),
            records: HashMap::new(),
        }
    }

    /// True if `kind` is collected.
    #[must_use]
    pub fn is_full(&self, kind: RecordKind) -> bool {
        self.full.contains(&kind)
    }

    /// Adds a batch.
    pub fn add_batch(&mut self, batch: &Arc<BlockBatch>) {
        if !self.full.contains(&batch.kind) {
            return;
        }
        let key = (
            StreamId {
                venue: batch.venue.clone(),
                kind: batch.kind,
            },
            batch.block_number,
        );
        self.records
            .entry(key)
            .or_default()
            .extend(batch.records.iter().cloned());
    }

    /// Adds a single record.
    pub fn add_record(&mut self, record: &Record) {
        if !self.full.contains(&record.kind()) {
            return;
        }
        let key = (
            StreamId {
                venue: record.prov().venue.clone(),
                kind: record.kind(),
            },
            record.prov().seq.block_number,
        );
        self.records.entry(key).or_default().push(record.clone());
    }

    /// Checks a commitment's root against the collected records. Returns
    /// `None` if the stream is not collected.
    #[must_use]
    pub fn check(&mut self, c: &BlockCommitment) -> Option<Result<(), VerifyError>> {
        if !self.full.contains(&c.stream.kind) {
            return None;
        }
        let mut records = self
            .records
            .remove(&(c.stream.clone(), c.block_number))
            .unwrap_or_default();
        records.sort_by(Record::canonical_cmp);
        Some(c.verify_records(&records).map_err(|e| VerifyError::Root {
            kind: c.stream.kind,
            block: c.block_number,
            reason: e.to_string(),
        }))
    }

    /// Drops collected records of blocks up to `block`.
    pub fn end_block(&mut self, block: u64) {
        self.records.retain(|(_, b), _| *b > block);
    }
}

/// Checks a proof reply: the commitment is trusted and correctly signed,
/// the returned record equals the one the client received (canonically),
/// and the proof places it under the root.
///
/// # Errors
///
/// [`VerifyError`].
pub fn check_proof(
    signers: &SignerSet,
    expected: &Record,
    commitment: &BlockCommitment,
    proof: &MerkleProof,
    record: &Record,
) -> Result<(), VerifyError> {
    signers.verify(commitment)?;
    let a = canonical_record(expected).map_err(|e| VerifyError::Proof(e.to_string()))?;
    let b = canonical_record(record).map_err(|e| VerifyError::Proof(e.to_string()))?;
    if a != b {
        return Err(VerifyError::Proof(
            "returned record differs from the received record".into(),
        ));
    }
    commitment
        .verify_inclusion(record, proof)
        .map_err(|e| VerifyError::Proof(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mesh_types::merkle::record_leaf;
    use mesh_types::{
        AdmissionId, AggressorSide, InstrumentId, Price, Provenance, Seq, SigningKey, Size, Trade,
        TradeId, UnixMillis, UnixNanos, VenueId, merkle_root,
    };

    fn trade(inst: &str, block: u64, i: u32) -> Record {
        Record::Trade(Trade {
            prov: Provenance::new(
                InstrumentId::new(inst).unwrap(),
                VenueId::new("hyperliquid").unwrap(),
                Seq::new(block, i),
                UnixMillis(1),
                UnixNanos(2),
                AdmissionId::new("ADM-0001").unwrap(),
            ),
            price: Price::parse_exact("1.0").unwrap(),
            size: Size::parse_exact("1").unwrap(),
            aggressor: AggressorSide::Unknown,
            trade_id: TradeId(u64::from(i)),
        })
    }

    fn batch(inst: &str, block: u64, n: u32) -> Arc<BlockBatch> {
        Arc::new(BlockBatch {
            instrument: InstrumentId::new(inst).unwrap(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
            block_number: block,
            block_time_ms: UnixMillis(1),
            prev_block_number: None,
            records: (0..n).map(|i| trade(inst, block, i)).collect(),
        })
    }

    fn stream() -> StreamId {
        StreamId {
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
        }
    }

    #[test]
    fn full_stream_root_and_signer_trust() {
        let key = SigningKey::from_bytes(&[5u8; 32]).unwrap();
        let eth = batch("hyperliquid:perp:ETH", 3, 2);
        let btc = batch("hyperliquid:perp:BTC", 3, 1);
        let mut all: Vec<Record> = btc.records.iter().chain(&eth.records).cloned().collect();
        all.sort_by(Record::canonical_cmp);
        let c = BlockCommitment::sign(&key, stream(), 3, &all).unwrap();

        let mut signers = SignerSet::new([]);
        assert!(matches!(
            signers.verify(&c),
            Err(VerifyError::UntrustedSigner(_))
        ));
        signers.set_announced(&[key.address()]);
        signers.verify(&c).unwrap();
        let pinned = SignerSet::new([SigningKey::from_bytes(&[6u8; 32]).unwrap().address()]);
        assert!(pinned.verify(&c).is_err());

        let mut col = BlockCollector::new([RecordKind::Trade]);
        col.add_batch(&eth);
        col.add_batch(&btc);
        assert_eq!(col.check(&c), Some(Ok(())));
        // Missing records: mismatch.
        col.add_batch(&eth);
        assert!(matches!(col.check(&c), Some(Err(VerifyError::Root { .. }))));
        let other = BlockCommitment {
            stream: StreamId {
                kind: RecordKind::Bar,
                ..stream()
            },
            ..c.clone()
        };
        assert_eq!(col.check(&other), None);
        col.add_batch(&batch("hyperliquid:perp:BTC", 4, 1));
        col.end_block(4);
        assert!(col.records.is_empty());
    }

    #[test]
    fn proofs_are_checked_against_the_received_record() {
        let key = SigningKey::from_bytes(&[5u8; 32]).unwrap();
        let b = batch("hyperliquid:perp:BTC", 3, 4);
        let c = BlockCommitment::sign(&key, stream(), 3, &b.records).unwrap();
        let leaves: Vec<_> = b.records.iter().map(|r| record_leaf(r).unwrap()).collect();
        assert_eq!(merkle_root(&leaves), c.merkle_root);
        let proof = MerkleProof::build(&leaves, 2).unwrap();
        let signers = SignerSet::new([key.address()]);
        let mut received = b.records[2].clone();
        received.prov_mut().stamp_publish(UnixNanos(99));
        check_proof(&signers, &received, &c, &proof, &b.records[2]).unwrap();
        assert!(check_proof(&signers, &b.records[1], &c, &proof, &b.records[2]).is_err());
        assert!(check_proof(&signers, &b.records[1], &c, &proof, &b.records[1]).is_err());
    }
}
