//! Block signer stage and inclusion-proof store.
//!
//! Per block and per stream ([`StreamId`]: venue and record kind) the signer
//! builds the RFC 9162 Merkle tree over the canonical encodings of all the
//! stream's records in the block, across instruments, ordered by
//! instrument then `seq` (observation-v0 § 7), and signs the
//! [`BlockCommitment`] with the active key of a [`KeyRing`].
//!
//! Leaves are hashed as records arrive ([`StreamBlock::push`]), off the
//! path that delivers the records, so closing a block only sorts, builds
//! the root and signs. The finished tree is kept in a [`ProofStore`] for a
//! bounded number of blocks to serve inclusion proofs.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use mesh_types::merkle::record_leaf;
use mesh_types::signing::SigError;
use mesh_types::{
    BlockBatch, BlockCommitment, EthAddress, H256, InstrumentId, MerkleProof, Record, Seq,
    StreamId, merkle_root,
};

use crate::keys::{KeyError, KeyInfo, KeyRing};

/// A record held by reference: inside a shared batch, or on its own.
#[derive(Debug, Clone)]
pub enum RecordRef {
    /// Record `index` of a batch.
    Batch(Arc<BlockBatch>, usize),
    /// A single record.
    Single(Arc<Record>),
}

impl RecordRef {
    /// The record, if the reference is valid.
    #[must_use]
    pub fn get(&self) -> Option<&Record> {
        match self {
            Self::Batch(b, i) => b.records.get(*i),
            Self::Single(r) => Some(r),
        }
    }
}

#[derive(Debug, Clone)]
struct Pending {
    instrument: InstrumentId,
    seq: Seq,
    leaf: H256,
    record: RecordRef,
}

/// Error from the signer stage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignerError {
    /// A record could not be canonically encoded.
    #[error("canonical encoding failed: {0}")]
    Encoding(String),
    /// A record reference points outside its batch.
    #[error("dangling record reference")]
    Dangling,
    /// More than `u32::MAX` records in one block and stream.
    #[error("too many records in one block and stream")]
    TooMany,
    /// Key problem.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// Commitment verification failed.
    #[error("commitment verification failed: {0}")]
    Verify(String),
    /// The commitment's signer is not on the allowlist.
    #[error("signer {0} is not allowed")]
    UnknownSigner(EthAddress),
}

/// Records of one stream in one block, with their leaf hashes.
#[derive(Debug, Clone, Default)]
pub struct StreamBlock {
    items: Vec<Pending>,
}

/// A finished tree: leaves in canonical order and the records behind them.
#[derive(Debug, Clone)]
pub struct SortedLeaves {
    /// Leaf hashes, ordered by instrument then `seq`.
    pub leaves: Vec<H256>,
    /// The records, in the same order.
    pub records: Vec<RecordRef>,
    /// Merkle root of `leaves`.
    pub root: H256,
}

impl StreamBlock {
    /// An empty accumulator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hashes and adds one record.
    ///
    /// # Errors
    ///
    /// Fails if the reference dangles or the record cannot be encoded.
    pub fn push(&mut self, record: RecordRef) -> Result<(), SignerError> {
        let r = record.get().ok_or(SignerError::Dangling)?;
        let leaf = record_leaf(r).map_err(|e| SignerError::Encoding(e.to_string()))?;
        self.items.push(Pending {
            instrument: r.prov().instrument.clone(),
            seq: r.prov().seq,
            leaf,
            record,
        });
        Ok(())
    }

    /// Adds every record of a batch.
    ///
    /// # Errors
    ///
    /// As [`StreamBlock::push`].
    pub fn push_batch(&mut self, batch: &Arc<BlockBatch>) -> Result<(), SignerError> {
        for i in 0..batch.records.len() {
            self.push(RecordRef::Batch(Arc::clone(batch), i))?;
        }
        Ok(())
    }

    /// Number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True if empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Sorts into canonical leaf order and builds the root. The sort is
    /// stable, so records with equal instrument and `seq` (for example bars
    /// of different intervals) keep their publication order.
    #[must_use]
    pub fn finish(mut self) -> SortedLeaves {
        self.items
            .sort_by(|a, b| a.instrument.cmp(&b.instrument).then(a.seq.cmp(&b.seq)));
        let leaves: Vec<H256> = self.items.iter().map(|p| p.leaf).collect();
        let root = merkle_root(&leaves);
        SortedLeaves {
            records: self.items.into_iter().map(|p| p.record).collect(),
            leaves,
            root,
        }
    }
}

/// Signs block commitments with the active key of a key ring. Rotation is
/// safe while signing: the ring sits behind a lock.
pub struct BlockSigner {
    ring: RwLock<KeyRing>,
}

impl fmt::Debug for BlockSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockSigner")
            .field("keys", &self.keys())
            .finish()
    }
}

impl BlockSigner {
    /// A signer over `ring`.
    ///
    /// # Errors
    ///
    /// [`KeyError::Empty`] if the ring has no keys.
    pub fn new(ring: KeyRing) -> Result<Self, KeyError> {
        ring.active()?;
        Ok(Self {
            ring: RwLock::new(ring),
        })
    }

    /// Signs a finished tree.
    ///
    /// # Errors
    ///
    /// [`SignerError::TooMany`] or a key error.
    pub fn sign(
        &self,
        stream: StreamId,
        block_number: u64,
        tree: &SortedLeaves,
    ) -> Result<BlockCommitment, SignerError> {
        let leaf_count = u32::try_from(tree.leaves.len()).map_err(|_| SignerError::TooMany)?;
        let h = BlockCommitment::commitment_hash(&stream, block_number, leaf_count, &tree.root);
        let ring = self.ring.read().unwrap_or_else(PoisonError::into_inner);
        let (_, key) = ring.active()?;
        Ok(BlockCommitment {
            stream,
            block_number,
            leaf_count,
            merkle_root: tree.root,
            signer: key.address(),
            signature: key.sign_hash(&h),
        })
    }

    /// Switches the signing key.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownId`].
    pub fn rotate(&self, id: &str) -> Result<(), KeyError> {
        self.ring
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .rotate(id)
    }

    /// Adds a key to the ring.
    ///
    /// # Errors
    ///
    /// [`KeyError::DuplicateId`].
    pub fn add_key(&self, id: &str, key: mesh_types::SigningKey) -> Result<(), KeyError> {
        self.ring
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .add(id, key)
    }

    /// Removes an inactive key.
    ///
    /// # Errors
    ///
    /// As [`KeyRing::retire`].
    pub fn retire(&self, id: &str) -> Result<(), KeyError> {
        self.ring
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retire(id)
    }

    /// Current signer addresses, active first.
    #[must_use]
    pub fn signers(&self) -> Vec<EthAddress> {
        self.ring
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .signers()
    }

    /// Public description of every key.
    #[must_use]
    pub fn keys(&self) -> Vec<KeyInfo> {
        self.ring
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .info()
    }
}

/// Verifies an upstream commitment against the complete set of records of
/// its stream and block, and an allowlist of signers. Returns the finished
/// tree on success.
///
/// # Errors
///
/// [`SignerError::UnknownSigner`] or [`SignerError::Verify`].
pub fn verify_commitment(
    commitment: &BlockCommitment,
    records: StreamBlock,
    allowlist: &[EthAddress],
) -> Result<SortedLeaves, SignerError> {
    if !allowlist.contains(&commitment.signer) {
        return Err(SignerError::UnknownSigner(commitment.signer));
    }
    commitment
        .verify_signature()
        .map_err(|e: SigError| SignerError::Verify(e.to_string()))?;
    let tree = records.finish();
    if u32::try_from(tree.leaves.len()).ok() != Some(commitment.leaf_count) {
        return Err(SignerError::Verify(format!(
            "leaf count {} != {}",
            tree.leaves.len(),
            commitment.leaf_count
        )));
    }
    if tree.root != commitment.merkle_root {
        return Err(SignerError::Verify("merkle root mismatch".into()));
    }
    Ok(tree)
}

/// One block and stream in the proof store.
#[derive(Debug)]
pub struct BlockEntry {
    commitment: Arc<BlockCommitment>,
    tree: SortedLeaves,
    index: HashMap<(InstrumentId, u32), usize>,
}

impl BlockEntry {
    /// Indexes a finished tree under its commitment.
    #[must_use]
    pub fn new(commitment: Arc<BlockCommitment>, tree: SortedLeaves) -> Self {
        let index = tree
            .records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                r.get()
                    .map(|r| ((r.prov().instrument.clone(), r.prov().seq.event_index), i))
            })
            .collect();
        Self {
            commitment,
            tree,
            index,
        }
    }

    /// The commitment.
    #[must_use]
    pub fn commitment(&self) -> &Arc<BlockCommitment> {
        &self.commitment
    }
}

/// Answer to a proof request.
#[derive(Debug, Clone)]
pub struct ProofResponse {
    /// The commitment covering the record.
    pub commitment: Arc<BlockCommitment>,
    /// RFC 9162 audit path.
    pub proof: MerkleProof,
    /// The record.
    pub record: Record,
}

/// Why a proof is not available.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    /// The block is older than the retention window (or the stream has
    /// never been seen).
    #[error("block {block_number} is outside the proof retention window")]
    OutOfRetention {
        /// Requested block.
        block_number: u64,
    },
    /// The block is retained but has no such record.
    #[error("no record {instrument} #{event_index} in block {block_number}")]
    NotFound {
        /// Requested block.
        block_number: u64,
        /// Requested instrument.
        instrument: InstrumentId,
        /// Requested index.
        event_index: u32,
    },
}

/// Recent finished trees, per stream, for a bounded number of blocks.
pub struct ProofStore {
    retention_blocks: u64,
    streams: RwLock<HashMap<StreamId, BTreeMap<u64, Arc<BlockEntry>>>>,
}

impl fmt::Debug for ProofStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofStore")
            .field("retention_blocks", &self.retention_blocks)
            .field("blocks", &self.len())
            .finish()
    }
}

impl ProofStore {
    /// A store keeping the newest `retention_blocks` block numbers per
    /// stream (at least 1).
    #[must_use]
    pub fn new(retention_blocks: u64) -> Self {
        Self {
            retention_blocks: retention_blocks.max(1),
            streams: RwLock::new(HashMap::new()),
        }
    }

    /// Adds a block and prunes blocks that fell out of the window.
    pub fn insert(&self, entry: BlockEntry) {
        let stream = entry.commitment.stream.clone();
        let block = entry.commitment.block_number;
        let mut streams = self.streams.write().unwrap_or_else(PoisonError::into_inner);
        let blocks = streams.entry(stream).or_default();
        blocks.insert(block, Arc::new(entry));
        let newest = blocks.keys().next_back().copied().unwrap_or(block);
        let cutoff = newest.saturating_sub(self.retention_blocks - 1);
        while let Some((&oldest, _)) = blocks.first_key_value() {
            if oldest >= cutoff {
                break;
            }
            blocks.remove(&oldest);
        }
    }

    /// The stored commitment for a block and stream.
    #[must_use]
    pub fn commitment(&self, stream: &StreamId, block_number: u64) -> Option<Arc<BlockCommitment>> {
        let streams = self.streams.read().unwrap_or_else(PoisonError::into_inner);
        streams
            .get(stream)
            .and_then(|b| b.get(&block_number))
            .map(|e| Arc::clone(&e.commitment))
    }

    /// Builds the inclusion proof for one record.
    ///
    /// # Errors
    ///
    /// [`ProofError`].
    pub fn lookup(
        &self,
        stream: &StreamId,
        block_number: u64,
        instrument: &InstrumentId,
        event_index: u32,
    ) -> Result<ProofResponse, ProofError> {
        let entry = {
            let streams = self.streams.read().unwrap_or_else(PoisonError::into_inner);
            streams
                .get(stream)
                .and_then(|b| b.get(&block_number))
                .cloned()
                .ok_or(ProofError::OutOfRetention { block_number })?
        };
        let not_found = || ProofError::NotFound {
            block_number,
            instrument: instrument.clone(),
            event_index,
        };
        let &i = entry
            .index
            .get(&(instrument.clone(), event_index))
            .ok_or_else(not_found)?;
        let proof = MerkleProof::build(&entry.tree.leaves, i).ok_or_else(not_found)?;
        let record = entry.tree.records[i].get().ok_or_else(not_found)?.clone();
        Ok(ProofResponse {
            commitment: Arc::clone(&entry.commitment),
            proof,
            record,
        })
    }

    /// Number of block entries held, over all streams.
    #[must_use]
    pub fn len(&self) -> usize {
        self.streams
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(BTreeMap::len)
            .sum()
    }

    /// True if nothing is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Oldest and newest retained block of a stream.
    #[must_use]
    pub fn range(&self, stream: &StreamId) -> Option<(u64, u64)> {
        let streams = self.streams.read().unwrap_or_else(PoisonError::into_inner);
        let b = streams.get(stream)?;
        Some((*b.keys().next()?, *b.keys().next_back()?))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::keys::parse_key;
    use mesh_types::{
        AdmissionId, AggressorSide, Price, Provenance, RecordKind, Size, Trade, TradeId,
        UnixMillis, UnixNanos, VenueId,
    };

    pub(crate) fn trade(instrument: &str, block: u64, index: u32) -> Record {
        Trade {
            prov: Provenance::new(
                InstrumentId::new(instrument).unwrap(),
                VenueId::new("hyperliquid").unwrap(),
                Seq::new(block, index),
                UnixMillis(1_767_225_600_000),
                UnixNanos(1_767_225_600_004_000_000),
                AdmissionId::new("ADM-0001").unwrap(),
            ),
            price: Price::parse_exact("100.5").unwrap(),
            size: Size::parse_exact("0.25").unwrap(),
            aggressor: AggressorSide::Buy,
            trade_id: TradeId(u64::from(index) + 1),
        }
        .into()
    }

    pub(crate) fn batch(instrument: &str, block: u64, n: u32) -> Arc<BlockBatch> {
        Arc::new(BlockBatch {
            instrument: InstrumentId::new(instrument).unwrap(),
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
            block_number: block,
            block_time_ms: UnixMillis(1_767_225_600_000),
            prev_block_number: None,
            records: (0..n).map(|i| trade(instrument, block, i)).collect(),
        })
    }

    pub(crate) fn stream() -> StreamId {
        StreamId {
            venue: VenueId::new("hyperliquid").unwrap(),
            kind: RecordKind::Trade,
        }
    }

    pub(crate) fn signer() -> BlockSigner {
        BlockSigner::new(KeyRing::single(
            "test",
            parse_key(&"11".repeat(32)).unwrap(),
        ))
        .unwrap()
    }

    #[test]
    fn signs_in_canonical_order_matching_mesh_types() {
        // Published out of instrument order: ETH first, then BTC.
        let eth = batch("hyperliquid:perp:ETH", 5, 2);
        let btc = batch("hyperliquid:perp:BTC", 5, 3);
        let mut sb = StreamBlock::new();
        sb.push_batch(&eth).unwrap();
        sb.push_batch(&btc).unwrap();
        assert_eq!(sb.len(), 5);
        let tree = sb.finish();
        let s = signer();
        let c = s.sign(stream(), 5, &tree).unwrap();
        let canonical: Vec<Record> = btc.records.iter().chain(&eth.records).cloned().collect();
        c.verify_records(&canonical).unwrap();
        assert_eq!(c.signer, s.signers()[0]);
    }

    #[test]
    fn proofs_verify_and_retention_prunes() {
        let store = ProofStore::new(3);
        let s = signer();
        for block in 1..=5 {
            let b = batch("hyperliquid:perp:BTC", block, 4);
            let mut sb = StreamBlock::new();
            sb.push_batch(&b).unwrap();
            let tree = sb.finish();
            let c = Arc::new(s.sign(stream(), block, &tree).unwrap());
            store.insert(BlockEntry::new(c, tree));
        }
        assert_eq!(store.range(&stream()), Some((3, 5)));
        assert_eq!(store.len(), 3);
        let btc = InstrumentId::new("hyperliquid:perp:BTC").unwrap();
        let p = store.lookup(&stream(), 4, &btc, 2).unwrap();
        p.commitment.verify_signature().unwrap();
        p.commitment.verify_inclusion(&p.record, &p.proof).unwrap();
        assert_eq!(p.record.prov().seq, Seq::new(4, 2));
        assert_eq!(
            store.lookup(&stream(), 1, &btc, 0).err(),
            Some(ProofError::OutOfRetention { block_number: 1 })
        );
        assert!(matches!(
            store.lookup(&stream(), 4, &btc, 9),
            Err(ProofError::NotFound { .. })
        ));
        assert!(store.commitment(&stream(), 5).is_some());
    }

    #[test]
    fn verify_commitment_checks_signer_root_and_count() {
        let s = signer();
        let b = batch("hyperliquid:perp:BTC", 9, 3);
        let mut sb = StreamBlock::new();
        sb.push_batch(&b).unwrap();
        let c = s.sign(stream(), 9, &sb.clone().finish()).unwrap();
        let allow = s.signers();
        verify_commitment(&c, sb.clone(), &allow).unwrap();
        assert!(matches!(
            verify_commitment(&c, sb.clone(), &[]),
            Err(SignerError::UnknownSigner(_))
        ));
        let mut short = StreamBlock::new();
        short.push(RecordRef::Batch(Arc::clone(&b), 0)).unwrap();
        assert!(matches!(
            verify_commitment(&c, short, &allow),
            Err(SignerError::Verify(_))
        ));
        let mut tampered = (*b).clone();
        if let Record::Trade(t) = &mut tampered.records[1] {
            t.trade_id = TradeId(999);
        }
        let mut sb2 = StreamBlock::new();
        sb2.push_batch(&Arc::new(tampered)).unwrap();
        assert!(matches!(
            verify_commitment(&c, sb2, &allow),
            Err(SignerError::Verify(_))
        ));
        let mut forged = c.clone();
        forged.block_number = 10;
        assert!(verify_commitment(&forged, sb, &allow).is_err());
    }

    #[test]
    fn rotation_changes_signer_without_breaking_verification() {
        let s = signer();
        s.add_key("next", parse_key(&"22".repeat(32)).unwrap())
            .unwrap();
        let b = batch("hyperliquid:perp:BTC", 1, 1);
        let mut sb = StreamBlock::new();
        sb.push_batch(&b).unwrap();
        let tree = sb.finish();
        let c1 = s.sign(stream(), 1, &tree).unwrap();
        s.rotate("next").unwrap();
        let c2 = s.sign(stream(), 1, &tree).unwrap();
        assert_ne!(c1.signer, c2.signer);
        c1.verify_signature().unwrap();
        c2.verify_signature().unwrap();
        assert_eq!(s.signers().len(), 2);
        s.retire("test").unwrap();
        assert_eq!(s.signers(), vec![c2.signer]);
        assert!(format!("{s:?}").contains("next"));
    }
}
