//! IPFS content identifiers, computed exactly as
//! `ipfs add --cid-version=1 --raw-leaves --chunker=size-<N>` does with the
//! default balanced DAG layout.
//!
//! # Construction (UnixFS v1, as implemented by Kubo's `go-unixfs`)
//!
//! - The file is cut into fixed-size chunks (`size-1048576` in manifest v0).
//! - Every chunk is a **raw** block (multicodec `raw`, 0x55). A file of at
//!   most one chunk (including the empty file) is just that raw block, so its
//!   CID is `bafkrei...`.
//! - Otherwise chunks become leaves of a balanced tree with at most 174
//!   links per node. The tree is filled left to right: a node at depth `d`
//!   holds up to 174 children, each a full subtree of depth `d - 1` except
//!   possibly the last. Depth is the smallest `d` with `174^d >= chunks`.
//!   Internal nodes are never collapsed, even with one child.
//! - Internal nodes are `dag-pb` (0x70) `PBNode`s: each link is
//!   `PBLink{Hash: child CID, Name: "", Tsize: cumulative size}`, written
//!   before `Data`, which is a UnixFS `Data{Type: File, filesize,
//!   blocksizes[]}` message. The cumulative size of a raw leaf is its length;
//!   of an internal node, its encoded length plus its children's sizes.
//! - CIDs are CIDv1 with a sha2-256 multihash, written in multibase base32
//!   (lowercase, no padding, prefix `b`).

use sha2::{Digest, Sha256};

use mesh_types::manifest::{IpfsCodec, IpfsRef};

/// Chunk size used by manifest v0 (`size-1048576`).
pub const DEFAULT_CHUNK_SIZE: usize = 1 << 20;
/// Kubo's default maximum number of links per UnixFS node.
pub const DEFAULT_MAX_LINKS: usize = 174;

/// Multicodec of raw blocks.
pub const CODEC_RAW: u64 = 0x55;
/// Multicodec of dag-pb blocks.
pub const CODEC_DAG_PB: u64 = 0x70;
const MULTIHASH_SHA2_256: u64 = 0x12;

/// Appends `v` as an unsigned LEB128 varint (multiformats and protobuf).
pub fn put_uvarint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// RFC 4648 base32, lowercase, without padding.
#[must_use]
pub fn base32_lower(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for &b in data {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// A CIDv1 with a sha2-256 multihash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cid {
    /// Multicodec of the block ([`CODEC_RAW`] or [`CODEC_DAG_PB`]).
    pub codec: u64,
    /// SHA-256 of the block.
    pub digest: [u8; 32],
}

impl Cid {
    /// The CID of `block` encoded with `codec`.
    #[must_use]
    pub fn of(codec: u64, block: &[u8]) -> Self {
        Self {
            codec,
            digest: Sha256::digest(block).into(),
        }
    }

    /// Binary form: `varint(1) varint(codec) 0x12 0x20 digest`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(36);
        put_uvarint(&mut out, 1);
        put_uvarint(&mut out, self.codec);
        put_uvarint(&mut out, MULTIHASH_SHA2_256);
        put_uvarint(&mut out, 32);
        out.extend_from_slice(&self.digest);
        out
    }

    /// The manifest's codec name for this CID.
    #[must_use]
    pub fn manifest_codec(&self) -> IpfsCodec {
        if self.codec == CODEC_RAW {
            IpfsCodec::Raw
        } else {
            IpfsCodec::DagPb
        }
    }
}

impl std::fmt::Display for Cid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "b{}", base32_lower(&self.to_bytes()))
    }
}

/// A child of a UnixFS node.
#[derive(Debug, Clone, Copy)]
struct Link {
    cid: Cid,
    /// Cumulative block size (PBLink.Tsize).
    tsize: u64,
    /// File bytes under this child (UnixFS blocksize).
    filesize: u64,
}

fn put_bytes_field(out: &mut Vec<u8>, tag: u8, bytes: &[u8]) {
    out.push(tag);
    put_uvarint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// Encodes an internal UnixFS file node over `children`.
fn encode_node(children: &[Link]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in children {
        let mut link = Vec::with_capacity(48);
        put_bytes_field(&mut link, 0x0a, &c.cid.to_bytes()); // Hash
        put_bytes_field(&mut link, 0x12, b""); // Name
        link.push(0x18); // Tsize
        put_uvarint(&mut link, c.tsize);
        put_bytes_field(&mut out, 0x12, &link); // PBNode.Links
    }
    let mut data = vec![0x08, 0x02]; // Type = File
    data.push(0x18); // filesize
    put_uvarint(&mut data, children.iter().map(|c| c.filesize).sum());
    for c in children {
        data.push(0x20); // blocksizes (repeated, not packed)
        put_uvarint(&mut data, c.filesize);
    }
    put_bytes_field(&mut out, 0x0a, &data); // PBNode.Data
    out
}

/// Builds the node of depth `depth` from the front of `leaves`, consuming
/// what it uses.
fn build(depth: u32, leaves: &mut std::slice::Iter<'_, Link>, max_links: usize) -> Link {
    let mut children = Vec::new();
    while children.len() < max_links {
        if leaves.as_slice().is_empty() {
            break;
        }
        let child = if depth == 1 {
            match leaves.next() {
                Some(l) => *l,
                None => break,
            }
        } else {
            build(depth - 1, leaves, max_links)
        };
        children.push(child);
    }
    let block = encode_node(&children);
    Link {
        cid: Cid::of(CODEC_DAG_PB, &block),
        tsize: block.len() as u64 + children.iter().map(|c| c.tsize).sum::<u64>(),
        filesize: children.iter().map(|c| c.filesize).sum(),
    }
}

/// Incremental UnixFS CID computation.
#[derive(Debug, Clone)]
pub struct UnixFsHasher {
    chunk_size: usize,
    max_links: usize,
    buf: Vec<u8>,
    leaves: Vec<Link>,
}

impl Default for UnixFsHasher {
    fn default() -> Self {
        Self::new(DEFAULT_CHUNK_SIZE, DEFAULT_MAX_LINKS)
    }
}

impl UnixFsHasher {
    /// A hasher with the given chunk size and maximum links per node.
    /// Values of zero are replaced by 1 and 2 respectively.
    #[must_use]
    pub fn new(chunk_size: usize, max_links: usize) -> Self {
        Self {
            chunk_size: chunk_size.max(1),
            max_links: max_links.max(2),
            buf: Vec::new(),
            leaves: Vec::new(),
        }
    }

    fn push_leaf(&mut self, chunk: &[u8]) {
        self.leaves.push(Link {
            cid: Cid::of(CODEC_RAW, chunk),
            tsize: chunk.len() as u64,
            filesize: chunk.len() as u64,
        });
    }

    /// Adds file bytes.
    pub fn update(&mut self, mut data: &[u8]) {
        if !self.buf.is_empty() {
            let need = self.chunk_size - self.buf.len();
            let take = need.min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == self.chunk_size {
                let chunk = std::mem::take(&mut self.buf);
                self.push_leaf(&chunk);
            }
        }
        while data.len() >= self.chunk_size {
            let (chunk, rest) = data.split_at(self.chunk_size);
            self.push_leaf(chunk);
            data = rest;
        }
        self.buf.extend_from_slice(data);
    }

    /// Finishes and returns the root CID.
    #[must_use]
    pub fn finish(mut self) -> Cid {
        if !self.buf.is_empty() || self.leaves.is_empty() {
            let chunk = std::mem::take(&mut self.buf);
            self.push_leaf(&chunk);
        }
        if self.leaves.len() == 1 {
            return self.leaves[0].cid;
        }
        let mut depth = 1u32;
        let mut capacity = self.max_links;
        while capacity < self.leaves.len() {
            depth += 1;
            capacity = capacity.saturating_mul(self.max_links);
        }
        build(depth, &mut self.leaves.iter(), self.max_links).cid
    }

    /// The chunker name recorded in manifests, such as `size-1048576`.
    #[must_use]
    pub fn chunker(&self) -> String {
        format!("size-{}", self.chunk_size)
    }

    /// Finishes and returns the manifest's IPFS reference.
    #[must_use]
    pub fn finish_ref(self) -> IpfsRef {
        let chunker = self.chunker();
        let cid = self.finish();
        IpfsRef {
            cid: cid.to_string(),
            codec: cid.manifest_codec(),
            chunker,
            raw_leaves: true,
        }
    }
}

/// Parses a manifest chunker name (`size-<N>`).
#[must_use]
pub fn parse_chunker(chunker: &str) -> Option<usize> {
    chunker
        .strip_prefix("size-")?
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

/// CID of `data` with the default parameters.
#[must_use]
pub fn cid_of(data: &[u8]) -> Cid {
    let mut h = UnixFsHasher::default();
    h.update(data);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    #[test]
    fn base32_rfc4648_vectors() {
        // RFC 4648 section 10, lowercased and unpadded.
        for (input, want) in [
            ("", ""),
            ("f", "my"),
            ("fo", "mzxq"),
            ("foo", "mzxw6"),
            ("foob", "mzxw6yq"),
            ("fooba", "mzxw6ytb"),
            ("foobar", "mzxw6ytboi"),
        ] {
            assert_eq!(base32_lower(input.as_bytes()), want);
        }
    }

    #[test]
    fn well_known_raw_cids() {
        // The empty file and "hello world" with --raw-leaves --cid-version=1.
        assert_eq!(
            cid_of(b"").to_string(),
            "bafkreihdwdcefgh4dqkjv67uzcmw7ojee6xedzdetojuzjevtenxquvyku"
        );
        assert_eq!(
            cid_of(b"hello world").to_string(),
            "bafkreifzjut3te2nhyekklss27nh3k72ysco7y32koao5eei66wof36n5e"
        );
        // Exactly one chunk is still a single raw block.
        let one = vec![7u8; DEFAULT_CHUNK_SIZE];
        let c = cid_of(&one);
        assert_eq!(c.codec, CODEC_RAW);
        assert_eq!(c.digest, <[u8; 32]>::from(Sha256::digest(&one)));
    }

    /// Hand-encodes a file node per the UnixFS and dag-pb specs, byte by
    /// byte, independently of `encode_node`.
    fn spec_node(children: &[(Vec<u8>, u64, u64)]) -> Vec<u8> {
        // Two-byte-at-most LEB128, written out by hand for the test.
        fn uv(v: u64) -> Vec<u8> {
            assert!(v < 1 << 14);
            if v < 0x80 {
                vec![v as u8]
            } else {
                vec![(v as u8 & 0x7f) | 0x80, (v >> 7) as u8]
            }
        }
        let mut out = Vec::new();
        for (cid, tsize, _) in children {
            // PBLink: 0a <len> cid, 12 00, 18 <tsize>
            let mut link = vec![0x0a, cid.len() as u8];
            link.extend_from_slice(cid);
            link.extend_from_slice(&[0x12, 0x00, 0x18]);
            link.extend_from_slice(&uv(*tsize));
            out.push(0x12);
            out.extend_from_slice(&uv(link.len() as u64));
            out.extend_from_slice(&link);
        }
        let total: u64 = children.iter().map(|c| c.2).sum();
        let mut data = vec![0x08, 0x02, 0x18];
        data.extend_from_slice(&uv(total));
        for (_, _, fs) in children {
            data.push(0x20);
            data.extend_from_slice(&uv(*fs));
        }
        out.push(0x0a);
        out.extend_from_slice(&uv(data.len() as u64));
        out.extend_from_slice(&data);
        out
    }

    fn raw_cid_bytes(chunk: &[u8]) -> Vec<u8> {
        let mut v = vec![0x01, 0x55, 0x12, 0x20];
        v.extend_from_slice(&Sha256::digest(chunk));
        v
    }

    fn pb_cid_bytes(block: &[u8]) -> Vec<u8> {
        let mut v = vec![0x01, 0x70, 0x12, 0x20];
        v.extend_from_slice(&Sha256::digest(block));
        v
    }

    #[test]
    fn two_level_tree_matches_hand_encoding() {
        // "abcdefghij" with 4-byte chunks: leaves "abcd", "efgh", "ij" under
        // one root.
        let leaves: Vec<&[u8]> = vec![b"abcd", b"efgh", b"ij"];
        let children: Vec<_> = leaves
            .iter()
            .map(|l| (raw_cid_bytes(l), l.len() as u64, l.len() as u64))
            .collect();
        let root = spec_node(&children);
        // Fixed expected encoding of the root's UnixFS Data and the start of
        // the first link.
        assert_eq!(hex::encode(&root[..4]), "122a0a24");
        assert!(root.ends_with(&[
            0x0a, 0x0a, 0x08, 0x02, 0x18, 0x0a, 0x20, 0x04, 0x20, 0x04, 0x20, 0x02
        ]));
        let want = format!("b{}", base32_lower(&pb_cid_bytes(&root)));

        let mut h = UnixFsHasher::new(4, DEFAULT_MAX_LINKS);
        // Feed in odd pieces to exercise buffering.
        h.update(b"abc");
        h.update(b"defghi");
        h.update(b"j");
        let got = h.finish();
        assert_eq!(got.to_string(), want);
        assert!(want.starts_with("bafybei"));
    }

    #[test]
    fn three_level_tree_matches_hand_encoding() {
        // 7 one-byte chunks with at most 3 links per node: depth 2. The
        // root holds [n(a,b,c), n(d,e,f), n(g)]; the last internal node is
        // kept even with a single child.
        let data = b"abcdefg";
        let leaf = |i: usize| (raw_cid_bytes(&data[i..=i]), 1u64, 1u64);
        let mut mids = Vec::new();
        for group in [&[0usize, 1, 2][..], &[3, 4, 5], &[6]] {
            let node = spec_node(&group.iter().map(|&i| leaf(i)).collect::<Vec<_>>());
            let tsize = node.len() as u64 + group.len() as u64;
            mids.push((pb_cid_bytes(&node), tsize, group.len() as u64));
        }
        let root = spec_node(&mids);
        let want = format!("b{}", base32_lower(&pb_cid_bytes(&root)));

        let mut h = UnixFsHasher::new(1, 3);
        h.update(data);
        assert_eq!(h.finish().to_string(), want);

        // Exactly filling depth 1 (3 chunks) stays at depth 1.
        let mut h = UnixFsHasher::new(1, 3);
        h.update(b"abc");
        let root = spec_node(&(0..3).map(leaf).collect::<Vec<_>>());
        assert_eq!(h.finish().digest, <[u8; 32]>::from(Sha256::digest(&root)));
    }

    #[test]
    fn chunker_names() {
        assert_eq!(parse_chunker("size-1048576"), Some(DEFAULT_CHUNK_SIZE));
        assert_eq!(parse_chunker("size-0"), None);
        assert_eq!(parse_chunker("rabin"), None);
        let r = UnixFsHasher::default().finish_ref();
        assert_eq!(r.chunker, "size-1048576");
        assert_eq!(r.codec, IpfsCodec::Raw);
        assert!(r.raw_leaves);
    }

    #[test]
    fn multi_chunk_default_is_dag_pb() {
        let data = vec![1u8; DEFAULT_CHUNK_SIZE + 1];
        let c = cid_of(&data);
        assert_eq!(c.codec, CODEC_DAG_PB);
        assert!(c.to_string().starts_with("bafybei"));
        // Streaming in arbitrary pieces gives the same CID.
        let mut h = UnixFsHasher::default();
        for piece in data.chunks(333_333) {
            h.update(piece);
        }
        assert_eq!(h.finish(), c);
    }
}
