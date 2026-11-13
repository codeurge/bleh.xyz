// SPDX-License-Identifier: Apache-2.0
pragma solidity 0.8.38;

/// @title BlehCommitments
/// @notice On-chain verification of bleh.xyz block commitments: the per-block,
/// per-stream Merkle roots and secp256k1 signatures that the origin publishes.
///
/// The scheme is defined by `crates/mesh-types/src/signing.rs` and
/// `crates/mesh-types/src/merkle.rs` and documented in
/// `docs/protocol/gateway-v0.md` and `docs/operations/bus.md`. This library
/// accepts exactly what the origin signs:
///
///   leaf(c)     = keccak256(0x00 || c)            c = canonical record bytes
///   node(l, r)  = keccak256(0x01 || l || r)
///   tree        = RFC 9162 Merkle tree (odd nodes promoted, never duplicated)
///   h           = keccak256("bleh-block-commitment-v0"
///                           || uint8(len(venue)) || venue
///                           || uint8(len(kind))  || kind
///                           || uint64_be(blockNumber)
///                           || uint32_be(leafCount)
///                           || merkleRoot)
///   signature   = 65 bytes r || s || v over the EIP-191 personal message
///                 keccak256("\x19Ethereum Signed Message:\n32" || h),
///                 with low s (EIP-2) and v in {27, 28}
///
/// Trust is the caller's decision: a valid commitment proves that
/// `commitment.signer` signed it, and the caller must check that this address
/// is a published, unrevoked bleh.xyz signer (the registry).
///
/// Verification functions return false on any invalid input; they never
/// revert.
library BlehCommitments {
    /// Domain tag of the block-commitment preimage (`BLOCK_COMMITMENT_DOMAIN`).
    bytes internal constant DOMAIN = "bleh-block-commitment-v0";

    /// secp256k1 group order divided by two, rounded down: the largest low `s`.
    uint256 internal constant HALF_N =
        0x7fffffffffffffffffffffffffffffff5d576e7357a4501ddfe92f46681b20a0;

    /// A signed Merkle root over one block's records of one stream, as served
    /// by the gateway (`BlockCommitment`).
    struct Commitment {
        /// Venue identifier text, for example "hyperliquid".
        string venue;
        /// Record kind text, for example "trade".
        string kind;
        uint64 blockNumber;
        uint32 leafCount;
        bytes32 merkleRoot;
        /// The address that claims to have signed.
        address signer;
        /// 65 bytes: r || s || v.
        bytes signature;
    }

    /// An RFC 9162 inclusion proof (`MerkleProof`).
    struct Proof {
        uint32 leafIndex;
        uint32 leafCount;
        /// Audit path, from the leaf's sibling up to the root's child.
        bytes32[] path;
    }

    /// The venue or kind is longer than its one-byte length prefix allows.
    error FieldTooLong();

    /// Leaf hash of canonical record bytes.
    function leafHash(bytes calldata canonical) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked(bytes1(0x00), canonical));
    }

    /// Inner-node hash.
    function nodeHash(bytes32 left, bytes32 right) internal pure returns (bytes32 h) {
        assembly ("memory-safe") {
            // 65 bytes at the free-memory pointer, which is not advanced.
            let m := mload(0x40)
            mstore8(m, 0x01)
            mstore(add(m, 1), left)
            mstore(add(m, 33), right)
            h := keccak256(m, 65)
        }
    }

    /// The hash the origin signs for a commitment. Reverts with
    /// `FieldTooLong` if venue or kind exceed 255 bytes (no such commitment
    /// can exist).
    function commitmentHash(
        string calldata venue,
        string calldata kind,
        uint64 blockNumber,
        uint32 leafCount,
        bytes32 merkleRoot
    ) internal pure returns (bytes32) {
        uint256 venueLen = bytes(venue).length;
        uint256 kindLen = bytes(kind).length;
        if (venueLen > type(uint8).max || kindLen > type(uint8).max) revert FieldTooLong();
        return keccak256(
            abi.encodePacked(
                DOMAIN, uint8(venueLen), venue, uint8(kindLen), kind, blockNumber, leafCount, merkleRoot
            )
        );
    }

    /// EIP-191 personal-message hash of a 32-byte hash.
    function toEthSignedMessageHash(bytes32 h) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("\x19Ethereum Signed Message:\n32", h));
    }

    /// The signer of `h` (signed as an EIP-191 personal message), or the zero
    /// address if the signature is malformed: wrong length, `v` not 27 or 28,
    /// high `s`, or `r`/`s` out of range.
    function recoverSigner(bytes32 h, bytes calldata signature) internal pure returns (address) {
        if (signature.length != 65) return address(0);
        bytes32 r = bytes32(signature[0:32]);
        bytes32 s = bytes32(signature[32:64]);
        uint8 v = uint8(signature[64]);
        if (v != 27 && v != 28) return address(0);
        if (uint256(s) > HALF_N) return address(0);
        // The precompile rejects r or s equal to zero or not below the group
        // order by returning the zero address.
        return ecrecover(toEthSignedMessageHash(h), v, r, s);
    }

    /// True if `c.signer` signed the commitment.
    function verifyCommitment(Commitment calldata c) internal pure returns (bool) {
        if (c.signer == address(0)) return false;
        if (bytes(c.venue).length > type(uint8).max || bytes(c.kind).length > type(uint8).max) return false;
        bytes32 h = commitmentHash(c.venue, c.kind, c.blockNumber, c.leafCount, c.merkleRoot);
        return recoverSigner(h, c.signature) == c.signer;
    }

    /// True if `leaf` is at `leafIndex` in a tree of `leafCount` leaves with
    /// root `root` (RFC 9162 section 2.1.3.2).
    function verifyInclusion(bytes32 leaf, uint256 leafIndex, uint256 leafCount, bytes32[] calldata path, bytes32 root)
        internal
        pure
        returns (bool)
    {
        if (leafIndex >= leafCount) return false;
        uint256 fn_ = leafIndex;
        uint256 sn = leafCount - 1;
        bytes32 r = leaf;
        for (uint256 i = 0; i < path.length; ++i) {
            if (sn == 0) return false;
            if (fn_ & 1 == 1 || fn_ == sn) {
                r = nodeHash(path[i], r);
                while (fn_ & 1 == 0 && fn_ != 0) {
                    fn_ >>= 1;
                    sn >>= 1;
                }
            } else {
                r = nodeHash(r, path[i]);
            }
            fn_ >>= 1;
            sn >>= 1;
        }
        return sn == 0 && r == root;
    }

    /// True if the record with canonical bytes `record` is included, by
    /// `proof`, in a commitment that `c.signer` signed. The record's own
    /// venue, kind and block are not decoded here: a caller that decodes the
    /// record must check them against `c`.
    function verifyRecord(Commitment calldata c, bytes calldata record, Proof calldata proof)
        internal
        pure
        returns (bool)
    {
        if (proof.leafCount != c.leafCount) return false;
        if (!verifyInclusion(leafHash(record), proof.leafIndex, proof.leafCount, proof.path, c.merkleRoot)) {
            return false;
        }
        return verifyCommitment(c);
    }
}

/// @title BlehBlockVerifier
/// @notice Stateless test contract exposing `BlehCommitments`. It holds no
/// keys and no signer set; see the library for the trust model.
contract BlehBlockVerifier {
    function leafHash(bytes calldata canonical) external pure returns (bytes32) {
        return BlehCommitments.leafHash(canonical);
    }

    function nodeHash(bytes32 left, bytes32 right) external pure returns (bytes32) {
        return BlehCommitments.nodeHash(left, right);
    }

    function commitmentHash(
        string calldata venue,
        string calldata kind,
        uint64 blockNumber,
        uint32 leafCount,
        bytes32 merkleRoot
    ) external pure returns (bytes32) {
        return BlehCommitments.commitmentHash(venue, kind, blockNumber, leafCount, merkleRoot);
    }

    function recoverSigner(bytes32 h, bytes calldata signature) external pure returns (address) {
        return BlehCommitments.recoverSigner(h, signature);
    }

    function verifyCommitment(BlehCommitments.Commitment calldata c) external pure returns (bool) {
        return BlehCommitments.verifyCommitment(c);
    }

    function verifyInclusion(bytes32 leaf, BlehCommitments.Proof calldata proof, bytes32 root)
        external
        pure
        returns (bool)
    {
        return BlehCommitments.verifyInclusion(leaf, proof.leafIndex, proof.leafCount, proof.path, root);
    }

    function verifyRecord(
        BlehCommitments.Commitment calldata c,
        bytes calldata record,
        BlehCommitments.Proof calldata proof
    ) external pure returns (bool) {
        return BlehCommitments.verifyRecord(c, record, proof);
    }
}
