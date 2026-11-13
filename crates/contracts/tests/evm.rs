//! The Solidity verifier against the Rust signer, in an in-process EVM.
//!
//! Commitments and proofs come from the bus signer stage (`StreamBlock`,
//! `BlockSigner`, `ProofStore`), the same code the origin runs, with a fixed
//! test key. Every case is checked against the Rust verifier as well: the
//! contract must accept exactly what Rust accepts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use bus::signer::RecordRef;
use bus::{BlockEntry, BlockSigner, KeyRing, ProofResponse, ProofStore, StreamBlock};
use contracts::{CREATION_CODE, SOLC_VERSION, SOURCE, SOURCE_SHA256, decode_hex, selector};
use mesh_types::hash::canonical_record;
use mesh_types::merkle::{leaf_hash, node_hash, record_leaf};
use mesh_types::{
    AdmissionId, AggressorSide, BlockCommitment, EthAddress, EthSignature, H256, InstrumentId,
    MerkleProof, Price, Provenance, Record, RecordKind, Seq, SigningKey, Size, StreamId, Trade,
    TradeId, UnixMillis, UnixNanos, VenueId,
};
use revm::context::TxEnv;
use revm::context_interface::result::{ExecutionResult, Output};
use revm::database::{CacheDB, EmptyDB};
use revm::handler::{MainnetContext, MainnetEvm};
use revm::primitives::{Address, Bytes};
use revm::{Context, ExecuteCommitEvm, ExecuteEvm, MainBuilder, MainContext};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Test keys. Fixed scalars, for tests only; never used outside this file.

fn test_key() -> SigningKey {
    SigningKey::from_bytes(&[0x42; 32]).unwrap()
}

fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[0x24; 32]).unwrap()
}

// ---------------------------------------------------------------------------
// A minimal ABI encoder for the calls the tests make.

enum Tok {
    Word([u8; 32]),
    Bytes(Vec<u8>),
    Array(Vec<Tok>),
    Tuple(Vec<Tok>),
}

impl Tok {
    fn is_dynamic(&self) -> bool {
        match self {
            Self::Word(_) => false,
            Self::Bytes(_) | Self::Array(_) => true,
            Self::Tuple(ts) => ts.iter().any(Self::is_dynamic),
        }
    }

    fn head_size(&self) -> usize {
        match self {
            Self::Tuple(ts) if !self.is_dynamic() => ts.iter().map(Self::head_size).sum(),
            _ => 32,
        }
    }

    fn encode(&self) -> Vec<u8> {
        match self {
            Self::Word(w) => w.to_vec(),
            Self::Bytes(b) => {
                let mut out = uint(b.len() as u64).to_vec();
                out.extend_from_slice(b);
                out.resize(32 + b.len().div_ceil(32) * 32, 0);
                out
            }
            Self::Array(ts) => {
                let mut out = uint(ts.len() as u64).to_vec();
                out.extend(encode_seq(ts));
                out
            }
            Self::Tuple(ts) => encode_seq(ts),
        }
    }
}

fn encode_seq(ts: &[Tok]) -> Vec<u8> {
    let head_len: usize = ts.iter().map(Tok::head_size).sum();
    let mut head = Vec::new();
    let mut tail = Vec::new();
    for t in ts {
        if t.is_dynamic() {
            head.extend_from_slice(&uint((head_len + tail.len()) as u64));
            tail.extend(t.encode());
        } else {
            head.extend(t.encode());
        }
    }
    head.extend(tail);
    head
}

fn uint(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

fn addr(a: &EthAddress) -> Tok {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(&a.0);
    Tok::Word(w)
}

fn word(h: &H256) -> Tok {
    Tok::Word(h.0)
}

fn calldata(signature: &str, args: &[Tok]) -> Vec<u8> {
    let sel = selector(signature).unwrap_or_else(|| panic!("no selector for {signature}"));
    let mut out = sel.to_vec();
    out.extend(encode_seq(args));
    out
}

const COMMITMENT: &str = "(string,string,uint64,uint32,bytes32,address,bytes)";
const PROOF: &str = "(uint32,uint32,bytes32[])";

fn commitment_tok(c: &BlockCommitment) -> Tok {
    Tok::Tuple(vec![
        Tok::Bytes(c.stream.venue.as_str().as_bytes().to_vec()),
        Tok::Bytes(c.stream.kind.as_str().as_bytes().to_vec()),
        Tok::Word(uint(c.block_number)),
        Tok::Word(uint(u64::from(c.leaf_count))),
        word(&c.merkle_root),
        addr(&c.signer),
        Tok::Bytes(c.signature.0.to_vec()),
    ])
}

fn proof_tok(p: &MerkleProof) -> Tok {
    Tok::Tuple(vec![
        Tok::Word(uint(u64::from(p.leaf_index))),
        Tok::Word(uint(u64::from(p.leaf_count))),
        Tok::Array(p.path.iter().map(word).collect()),
    ])
}

// ---------------------------------------------------------------------------
// The in-process chain.

type Evm = MainnetEvm<MainnetContext<CacheDB<EmptyDB>>>;

const DEPLOYER: Address = Address::new([0xde; 20]);

struct Chain {
    evm: Evm,
    contract: Address,
}

enum CallResult {
    Ok { output: Vec<u8>, gas: u64 },
    Revert(Vec<u8>),
}

impl Chain {
    /// Deploys the committed creation bytecode.
    fn deploy() -> Self {
        let mut evm = Context::mainnet()
            .with_db(CacheDB::new(EmptyDB::default()))
            .build_mainnet();
        let code = decode_hex(CREATION_CODE).unwrap();
        let tx = TxEnv::builder()
            .caller(DEPLOYER)
            .create()
            .data(Bytes::from(code))
            .nonce(0)
            .build_fill();
        let contract = match evm.transact_commit(tx).expect("deploy transaction") {
            ExecutionResult::Success {
                output: Output::Create(_, Some(a)),
                ..
            } => a,
            other => panic!("deployment failed: {other:?}"),
        };
        Self { evm, contract }
    }

    fn call(&mut self, data: Vec<u8>) -> CallResult {
        let tx = TxEnv::builder()
            .caller(DEPLOYER)
            .call(self.contract)
            .data(Bytes::from(data))
            .nonce(1)
            .build_fill();
        match self.evm.transact(tx).expect("call transaction").result {
            ExecutionResult::Success { output, gas, .. } => CallResult::Ok {
                output: output.into_data().to_vec(),
                gas: gas.total_gas_spent(),
            },
            ExecutionResult::Revert { output, .. } => CallResult::Revert(output.to_vec()),
            other => panic!("call halted: {other:?}"),
        }
    }

    fn call_ok(&mut self, signature: &str, args: &[Tok]) -> (Vec<u8>, u64) {
        match self.call(calldata(signature, args)) {
            CallResult::Ok { output, gas } => (output, gas),
            CallResult::Revert(r) => panic!("{signature} reverted: 0x{}", hex(&r)),
        }
    }

    fn bool(&mut self, signature: &str, args: &[Tok]) -> bool {
        let (out, _) = self.call_ok(signature, args);
        assert_eq!(out.len(), 32, "{signature} output");
        assert!(out[..31].iter().all(|&b| b == 0) && out[31] <= 1);
        out[31] == 1
    }

    fn bytes32(&mut self, signature: &str, args: &[Tok]) -> H256 {
        let (out, _) = self.call_ok(signature, args);
        H256(out.try_into().expect("32-byte output"))
    }

    fn address(&mut self, signature: &str, args: &[Tok]) -> EthAddress {
        let out = self.bytes32(signature, args).0;
        assert!(out[..12].iter().all(|&b| b == 0));
        EthAddress(out[12..].try_into().unwrap())
    }

    // Contract functions.

    fn leaf_hash(&mut self, canonical: &[u8]) -> H256 {
        self.bytes32("leafHash(bytes)", &[Tok::Bytes(canonical.to_vec())])
    }

    fn node_hash(&mut self, l: &H256, r: &H256) -> H256 {
        self.bytes32("nodeHash(bytes32,bytes32)", &[word(l), word(r)])
    }

    fn commitment_hash(&mut self, stream: &StreamId, block: u64, count: u32, root: &H256) -> H256 {
        self.bytes32(
            "commitmentHash(string,string,uint64,uint32,bytes32)",
            &commitment_hash_args(
                stream.venue.as_str().as_bytes(),
                stream.kind.as_str().as_bytes(),
                block,
                count,
                root,
            ),
        )
    }

    fn recover(&mut self, h: &H256, sig: &[u8]) -> EthAddress {
        self.address(
            "recoverSigner(bytes32,bytes)",
            &[word(h), Tok::Bytes(sig.to_vec())],
        )
    }

    fn verify_commitment(&mut self, c: &BlockCommitment) -> bool {
        self.bool(
            &format!("verifyCommitment({COMMITMENT})"),
            &[commitment_tok(c)],
        )
    }

    fn verify_inclusion(&mut self, leaf: &H256, p: &MerkleProof, root: &H256) -> bool {
        self.bool(
            &format!("verifyInclusion(bytes32,{PROOF},bytes32)"),
            &[word(leaf), proof_tok(p), word(root)],
        )
    }

    fn verify_record_call(c: &BlockCommitment, record: &[u8], p: &MerkleProof) -> Vec<u8> {
        calldata(
            &format!("verifyRecord({COMMITMENT},bytes,{PROOF})"),
            &[commitment_tok(c), Tok::Bytes(record.to_vec()), proof_tok(p)],
        )
    }

    fn verify_record(&mut self, c: &BlockCommitment, record: &[u8], p: &MerkleProof) -> bool {
        self.bool(
            &format!("verifyRecord({COMMITMENT},bytes,{PROOF})"),
            &[commitment_tok(c), Tok::Bytes(record.to_vec()), proof_tok(p)],
        )
    }
}

fn commitment_hash_args(
    venue: &[u8],
    kind: &[u8],
    block: u64,
    count: u32,
    root: &H256,
) -> [Tok; 5] {
    [
        Tok::Bytes(venue.to_vec()),
        Tok::Bytes(kind.to_vec()),
        Tok::Word(uint(block)),
        Tok::Word(uint(u64::from(count))),
        word(root),
    ]
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Synthetic records and the bus signer.

const VENUES: [&str; 2] = ["hyperliquid", "test-venue-0"];
const INSTRUMENTS: [&str; 3] = [
    "hyperliquid:perp:ETH",
    "hyperliquid:perp:BTC",
    "hyperliquid:perp:SOL",
];

fn trade(venue: &str, instrument: &str, block: u64, index: u32) -> Record {
    Trade {
        prov: Provenance::new(
            InstrumentId::new(instrument).unwrap(),
            VenueId::new(venue).unwrap(),
            Seq::new(block, index),
            UnixMillis(1_767_225_600_000),
            UnixNanos(1_767_225_600_004_000_000),
            AdmissionId::new("ADM-0001").unwrap(),
        ),
        price: Price::parse_exact(&format!("{}.{}", 100 + index, index % 10)).unwrap(),
        size: Size::parse_exact("0.25").unwrap(),
        aggressor: if index.is_multiple_of(2) {
            AggressorSide::Buy
        } else {
            AggressorSide::Sell
        },
        trade_id: TradeId(u64::from(index) + 1),
    }
    .into()
}

fn signer() -> BlockSigner {
    BlockSigner::new(KeyRing::single("test", test_key())).unwrap()
}

/// One signed block of `n` trades, spread over instruments and pushed out of
/// canonical order, as the origin's signer stage sees them.
struct SignedBlock {
    commitment: Arc<BlockCommitment>,
    /// Proof responses, in canonical leaf order.
    responses: Vec<ProofResponse>,
}

fn sign_block(signer: &BlockSigner, venue: &str, block: u64, n: u32) -> SignedBlock {
    let stream = StreamId {
        venue: VenueId::new(venue).unwrap(),
        kind: RecordKind::Trade,
    };
    let mut keys = Vec::new();
    let mut sb = StreamBlock::new();
    for i in (0..n).rev() {
        let instrument = INSTRUMENTS[(i as usize * 7) % INSTRUMENTS.len()];
        sb.push(RecordRef::Single(Arc::new(trade(
            venue, instrument, block, i,
        ))))
        .unwrap();
        keys.push((InstrumentId::new(instrument).unwrap(), i));
    }
    let tree = sb.finish();
    let commitment = Arc::new(signer.sign(stream.clone(), block, &tree).unwrap());
    let store = ProofStore::new(4);
    store.insert(BlockEntry::new(Arc::clone(&commitment), tree));
    let mut responses: Vec<ProofResponse> = keys
        .iter()
        .map(|(inst, i)| store.lookup(&stream, block, inst, *i).unwrap())
        .collect();
    responses.sort_by_key(|r| r.proof.leaf_index);
    SignedBlock {
        commitment,
        responses,
    }
}

/// The Rust verifier's answer for a commitment, canonical record bytes and
/// proof (the checks of `BlockCommitment::verify_signature` and
/// `verify_inclusion`, on bytes).
fn rust_accepts(c: &BlockCommitment, canonical: &[u8], p: &MerkleProof) -> bool {
    p.leaf_count == c.leaf_count
        && p.verify(&leaf_hash(canonical), &c.merkle_root)
        && c.verify_signature().is_ok()
}

/// Asserts the contract and the Rust verifier agree, and both give `expect`.
fn check(
    chain: &mut Chain,
    c: &BlockCommitment,
    canonical: &[u8],
    p: &MerkleProof,
    expect: bool,
    what: &str,
) {
    let rust = rust_accepts(c, canonical, p);
    let evm = chain.verify_record(c, canonical, p);
    assert_eq!(rust, expect, "Rust verifier: {what}");
    assert_eq!(evm, expect, "contract: {what}");
}

fn flip(h: &H256) -> H256 {
    let mut x = *h;
    x.0[31] ^= 1;
    x
}

// ---------------------------------------------------------------------------
// Tests.

#[test]
fn bytecode_matches_committed_source() {
    let digest = Sha256::digest(SOURCE.as_bytes());
    assert_eq!(
        hex(&digest),
        SOURCE_SHA256,
        "sol/BlehBlockVerifier.sol changed: run `node compile.mjs` in crates/contracts"
    );
    let version = SOLC_VERSION.split('+').next().unwrap();
    assert!(
        SOURCE.contains(&format!("pragma solidity {version};")),
        "pragma must pin solc {version}"
    );
}

#[test]
fn selectors_match_the_compiler() {
    for (sig, id) in contracts::METHOD_IDS {
        let h = H256::keccak(&[sig.as_bytes()]);
        assert_eq!(hex(&h.0[..4]), *id, "{sig}");
    }
}

#[test]
fn hashes_match_rust() {
    let mut chain = Chain::deploy();
    for venue in VENUES {
        for n in [1u32, 2, 5] {
            let block = sign_block(&signer(), venue, 7, n);
            for r in &block.responses {
                let bytes = canonical_record(&r.record).unwrap();
                assert_eq!(chain.leaf_hash(&bytes), record_leaf(&r.record).unwrap());
            }
        }
    }
    assert_eq!(chain.leaf_hash(b""), leaf_hash(b""));
    let (a, b) = (leaf_hash(b"a"), leaf_hash(b"b"));
    assert_eq!(chain.node_hash(&a, &b), node_hash(&a, &b));
    assert_eq!(chain.node_hash(&b, &a), node_hash(&b, &a));

    let kinds = [RecordKind::Trade, RecordKind::BookEvent];
    for venue in VENUES {
        for kind in kinds {
            let stream = StreamId {
                venue: VenueId::new(venue).unwrap(),
                kind,
            };
            for (block, count) in [(0u64, 0u32), (7, 3), (0x0123_4567_89ab_cdef, u32::MAX)] {
                let root = H256::keccak(&[&block.to_be_bytes()]);
                assert_eq!(
                    chain.commitment_hash(&stream, block, count, &root),
                    BlockCommitment::commitment_hash(&stream, block, count, &root),
                    "{venue} {kind:?} {block} {count}"
                );
            }
        }
    }

    // A venue or kind longer than one length byte can describe reverts.
    let long = vec![b'a'; 256];
    let data = calldata(
        "commitmentHash(string,string,uint64,uint32,bytes32)",
        &commitment_hash_args(&long, b"trade", 1, 1, &H256::default()),
    );
    match chain.call(data) {
        CallResult::Revert(r) => {
            assert_eq!(r, H256::keccak(&[b"FieldTooLong()"]).0[..4].to_vec());
        }
        CallResult::Ok { .. } => panic!("expected FieldTooLong"),
    }
}

#[test]
fn recovers_rust_signatures() {
    let mut chain = Chain::deploy();
    for key in [test_key(), other_key()] {
        for i in 0..24u8 {
            let h = H256::keccak(&[b"recover", &[i]]);
            let sig = key.sign_hash(&h);
            assert_eq!(sig.recover_hash(&h).unwrap(), key.address());
            assert_eq!(chain.recover(&h, &sig.0), key.address(), "i={i}");
            assert_ne!(chain.recover(&flip(&h), &sig.0), key.address());
        }
    }
}

#[test]
fn rejects_malformed_signatures_like_rust() {
    let mut chain = Chain::deploy();
    let key = test_key();
    let zero = EthAddress([0; 20]);
    // secp256k1 group order.
    let n: [u8; 32] =
        decode_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141")
            .unwrap()
            .try_into()
            .unwrap();
    let mut tried_v = [false, false];
    for i in 0..16u8 {
        let h = H256::keccak(&[b"malformed", &[i]]);
        let sig = key.sign_hash(&h);
        tried_v[usize::from(sig.0[64] - 27)] = true;

        // v outside {27, 28}.
        for v in [0u8, 1, 26, 29] {
            let mut bad = sig;
            bad.0[64] = v;
            assert!(bad.recover_hash(&h).is_err());
            assert_eq!(chain.recover(&h, &bad.0), zero, "v={v}");
        }

        // High s: (r, n - s, v ^ 1) is the same point, mathematically valid,
        // and must be rejected (EIP-2), as Rust does.
        let mut high = sig;
        let mut borrow = 0i32;
        for j in (0..32).rev() {
            let d = i32::from(n[j]) - i32::from(sig.0[32 + j]) - borrow;
            let (d, b) = if d < 0 { (d + 0x100, 1) } else { (d, 0) };
            high.0[32 + j] = u8::try_from(d).unwrap();
            borrow = b;
        }
        assert_eq!(borrow, 0);
        high.0[64] = 27 + (1 - (sig.0[64] - 27));
        assert!(high.recover_hash(&h).is_err());
        assert_eq!(chain.recover(&h, &high.0), zero, "high s, i={i}");

        // r or s zero.
        let mut r0 = sig;
        r0.0[..32].fill(0);
        assert!(r0.recover_hash(&h).is_err());
        assert_eq!(chain.recover(&h, &r0.0), zero);
        let mut s0 = sig;
        s0.0[32..64].fill(0);
        assert!(s0.recover_hash(&h).is_err());
        assert_eq!(chain.recover(&h, &s0.0), zero);

        // Wrong length.
        assert_eq!(chain.recover(&h, &sig.0[..64]), zero);
        let mut long = sig.0.to_vec();
        long.push(0);
        assert_eq!(chain.recover(&h, &long), zero);
    }
    assert_eq!(tried_v, [true, true], "both recovery ids exercised");
}

#[test]
fn accepts_every_origin_commitment_and_proof() {
    let mut chain = Chain::deploy();
    let signer = signer();
    let sizes = [1u32, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 31, 33];
    for (k, &n) in sizes.iter().enumerate() {
        let venue = VENUES[k % VENUES.len()];
        let block_number = 1_000 + u64::from(n) * 0x1_0000_0001;
        let block = sign_block(&signer, venue, block_number, n);
        let c = &*block.commitment;
        assert_eq!(c.signer, test_key().address());
        assert!(chain.verify_commitment(c), "n={n}");
        for r in &block.responses {
            // The Rust client's own check on the typed record.
            c.verify_inclusion(&r.record, &r.proof).unwrap();
            assert_eq!(&*r.commitment, c);
            let bytes = canonical_record(&r.record).unwrap();
            let leaf = leaf_hash(&bytes);
            assert!(chain.verify_inclusion(&leaf, &r.proof, &c.merkle_root));
            check(
                &mut chain,
                c,
                &bytes,
                &r.proof,
                true,
                &format!("n={n} i={}", r.proof.leaf_index),
            );
        }
    }
}

#[test]
fn rejects_what_rust_rejects() {
    let mut chain = Chain::deploy();
    let signer = signer();
    let block = sign_block(&signer, "hyperliquid", 42, 11);
    let c = &*block.commitment;
    let other_signer = BlockSigner::new(KeyRing::single("other", other_key())).unwrap();

    for r in &block.responses {
        let i = r.proof.leaf_index;
        let bytes = canonical_record(&r.record).unwrap();
        let p = &r.proof;
        let what = |s: &str| format!("leaf {i}: {s}");

        // The genuine triple.
        check(&mut chain, c, &bytes, p, true, &what("genuine"));

        // Record bytes changed or replaced by another record of the block.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        check(&mut chain, c, &tampered, p, false, &what("tampered record"));
        let next = &block.responses[(i as usize + 1) % block.responses.len()];
        let other = canonical_record(&next.record).unwrap();
        check(&mut chain, c, &other, p, false, &what("other record"));

        // Proof changed.
        let mut q = p.clone();
        q.leaf_index = (i + 1) % c.leaf_count;
        check(&mut chain, c, &bytes, &q, false, &what("moved index"));
        let mut q = p.clone();
        q.leaf_index = c.leaf_count;
        check(
            &mut chain,
            c,
            &bytes,
            &q,
            false,
            &what("index out of range"),
        );
        let mut q = p.clone();
        q.leaf_count += 1;
        check(&mut chain, c, &bytes, &q, false, &what("proof leaf count"));
        let mut q = p.clone();
        q.path[0] = flip(&q.path[0]);
        check(&mut chain, c, &bytes, &q, false, &what("path element"));
        let mut q = p.clone();
        q.path.pop();
        check(&mut chain, c, &bytes, &q, false, &what("short path"));
        let mut q = p.clone();
        q.path.push(leaf_hash(b"extra"));
        check(&mut chain, c, &bytes, &q, false, &what("long path"));

        // Commitment fields changed after signing.
        let mut d = c.clone();
        d.block_number += 1;
        check(&mut chain, &d, &bytes, p, false, &what("block number"));
        let mut d = c.clone();
        d.stream.venue = VenueId::new("test-venue-0").unwrap();
        check(&mut chain, &d, &bytes, p, false, &what("venue"));
        let mut d = c.clone();
        d.stream.kind = RecordKind::BookEvent;
        check(&mut chain, &d, &bytes, p, false, &what("kind"));
        let mut d = c.clone();
        d.leaf_count += 1;
        let mut q = p.clone();
        q.leaf_count += 1;
        check(&mut chain, &d, &bytes, &q, false, &what("leaf count"));
        let mut d = c.clone();
        d.merkle_root = flip(&d.merkle_root);
        check(&mut chain, &d, &bytes, p, false, &what("root"));

        // Signer and signature.
        let mut d = c.clone();
        d.signer = other_key().address();
        check(&mut chain, &d, &bytes, p, false, &what("claimed signer"));
        let mut d = c.clone();
        d.signer = EthAddress([0; 20]);
        check(&mut chain, &d, &bytes, p, false, &what("zero signer"));
        let mut d = c.clone();
        d.signature = EthSignature([0; 65]);
        check(&mut chain, &d, &bytes, p, false, &what("zero signature"));
        let mut d = c.clone();
        d.signature.0[10] ^= 1;
        check(&mut chain, &d, &bytes, p, false, &what("signature bit"));

        // A different key signing the same tree, claiming the real signer.
        let tree_sig = other_signer
            .sign(c.stream.clone(), c.block_number, &{
                let mut sb = StreamBlock::new();
                for x in &block.responses {
                    sb.push(RecordRef::Single(Arc::new(x.record.clone())))
                        .unwrap();
                }
                sb.finish()
            })
            .unwrap();
        assert_eq!(tree_sig.merkle_root, c.merkle_root);
        let mut d = c.clone();
        d.signature = tree_sig.signature;
        check(&mut chain, &d, &bytes, p, false, &what("other key"));
        // ...and that commitment verifies under its own signer.
        check(
            &mut chain,
            &tree_sig,
            &bytes,
            p,
            true,
            &what("other key, own signer"),
        );
    }
}

#[test]
fn rotated_keys_verify_under_their_own_address() {
    let mut chain = Chain::deploy();
    let signer = signer();
    signer.add_key("next", other_key()).unwrap();
    let before = sign_block(&signer, "hyperliquid", 100, 4);
    signer.rotate("next").unwrap();
    let after = sign_block(&signer, "hyperliquid", 101, 4);
    assert_eq!(before.commitment.signer, test_key().address());
    assert_eq!(after.commitment.signer, other_key().address());
    for b in [&before, &after] {
        assert!(chain.verify_commitment(&b.commitment));
        let r = &b.responses[2];
        let bytes = canonical_record(&r.record).unwrap();
        check(
            &mut chain,
            &b.commitment,
            &bytes,
            &r.proof,
            true,
            "rotation",
        );
    }
}

#[test]
fn gas_for_verify_record() {
    // Reported, and bounded so a regression shows: one ecrecover plus one
    // hash per tree level.
    let mut chain = Chain::deploy();
    let signer = signer();
    for n in [1u32, 16, 1024] {
        let block = sign_block(&signer, "hyperliquid", 9, n);
        let r = &block.responses[block.responses.len() / 2];
        let bytes = canonical_record(&r.record).unwrap();
        let data = Chain::verify_record_call(&block.commitment, &bytes, &r.proof);
        let CallResult::Ok { output, gas } = chain.call(data) else {
            panic!("verifyRecord reverted");
        };
        assert_eq!(output[31], 1);
        eprintln!(
            "verifyRecord: {n} leaves, path {}, record {} bytes: {gas} gas (transaction total)",
            r.proof.path.len(),
            bytes.len()
        );
        assert!(gas < 60_000, "{n} leaves: {gas} gas");
    }
}
