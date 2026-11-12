//! BitTorrent v2 (BEP 52) torrents with web seeds (BEP 19).
//!
//! A day's torrent is v2-only, named after the archive, with a 1 MiB piece
//! length, and contains exactly the manifest's files at their paths
//! (`docs/specs/manifest-v0.md` § 2.2). With these fixed parameters anyone
//! rebuilds the same `info` dictionary and so the same infohash.
//!
//! Per BEP 52:
//!
//! - Each file is hashed separately: SHA-256 of every 16 KiB block (the last
//!   block may be shorter), padded with all-zero leaf hashes to a power of
//!   two, combined pairwise with SHA-256 into the file's `pieces root`.
//!   Empty files have no `pieces root`.
//! - `piece layers` maps each `pieces root` of a file larger than the piece
//!   length to the concatenated hashes of the tree layer where one hash
//!   covers one piece.
//! - The infohash is SHA-256 of the bencoded `info` dictionary.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use mesh_types::Sha256Digest;

use crate::error::{Error, Result};
use crate::hex;

/// Merkle block size (16 KiB).
pub const BLOCK_SIZE: usize = 16 * 1024;
/// Piece length used by manifest v0 (1 MiB).
pub const PIECE_LENGTH: u64 = 1 << 20;

/// A bencoded value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bencode {
    /// Integer.
    Int(i64),
    /// Byte string.
    Bytes(Vec<u8>),
    /// List.
    List(Vec<Bencode>),
    /// Dictionary with keys sorted by raw bytes.
    Dict(BTreeMap<Vec<u8>, Bencode>),
}

impl Bencode {
    /// A byte string from text.
    pub fn str(s: impl AsRef<[u8]>) -> Self {
        Self::Bytes(s.as_ref().to_vec())
    }

    /// Appends the encoding to `out`.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Self::Int(i) => {
                out.push(b'i');
                out.extend_from_slice(i.to_string().as_bytes());
                out.push(b'e');
            }
            Self::Bytes(b) => {
                out.extend_from_slice(b.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(b);
            }
            Self::List(items) => {
                out.push(b'l');
                for i in items {
                    i.encode_into(out);
                }
                out.push(b'e');
            }
            Self::Dict(map) => {
                out.push(b'd');
                for (k, v) in map {
                    out.extend_from_slice(k.len().to_string().as_bytes());
                    out.push(b':');
                    out.extend_from_slice(k);
                    v.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }

    /// The encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    /// Decodes exactly one value occupying all of `data`.
    ///
    /// # Errors
    ///
    /// Fails on malformed or non-canonical input (unsorted or duplicate
    /// keys, leading zeros).
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut pos = 0;
        let v = decode_value(data, &mut pos, 0)?;
        if pos != data.len() {
            return Err(Error::invalid("bencode: trailing bytes"));
        }
        Ok(v)
    }

    /// Looks up a dictionary key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Bencode> {
        match self {
            Self::Dict(m) => m.get(key.as_bytes()),
            _ => None,
        }
    }

    /// The byte string, if this is one.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The integer, if this is one.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
}

fn bad(msg: &str) -> Error {
    Error::invalid(format!("bencode: {msg}"))
}

fn decode_len(data: &[u8], pos: &mut usize, end: u8) -> Result<usize> {
    let start = *pos;
    while *pos < data.len() && data[*pos] != end {
        *pos += 1;
    }
    let digits = std::str::from_utf8(&data[start..*pos]).map_err(|_| bad("length"))?;
    if *pos >= data.len() || digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
        return Err(bad("length"));
    }
    *pos += 1;
    digits.parse().map_err(|_| bad("length"))
}

fn decode_value(data: &[u8], pos: &mut usize, depth: usize) -> Result<Bencode> {
    if depth > 64 {
        return Err(bad("nesting too deep"));
    }
    match data.get(*pos) {
        Some(b'i') => {
            *pos += 1;
            let start = *pos;
            while *pos < data.len() && data[*pos] != b'e' {
                *pos += 1;
            }
            let s = std::str::from_utf8(&data[start..*pos]).map_err(|_| bad("integer"))?;
            if *pos >= data.len()
                || s.is_empty()
                || s == "-0"
                || (s.len() > 1 && s.starts_with('0'))
                || s.starts_with("-0")
            {
                return Err(bad("integer"));
            }
            *pos += 1;
            Ok(Bencode::Int(s.parse().map_err(|_| bad("integer"))?))
        }
        Some(b'l') => {
            *pos += 1;
            let mut items = Vec::new();
            while data.get(*pos) != Some(&b'e') {
                if *pos >= data.len() {
                    return Err(bad("unterminated list"));
                }
                items.push(decode_value(data, pos, depth + 1)?);
            }
            *pos += 1;
            Ok(Bencode::List(items))
        }
        Some(b'd') => {
            *pos += 1;
            let mut map = BTreeMap::new();
            let mut last: Option<Vec<u8>> = None;
            while data.get(*pos) != Some(&b'e') {
                if *pos >= data.len() {
                    return Err(bad("unterminated dictionary"));
                }
                let Bencode::Bytes(k) = decode_value(data, pos, depth + 1)? else {
                    return Err(bad("dictionary key is not a string"));
                };
                if last.as_ref().is_some_and(|l| l >= &k) {
                    return Err(bad("dictionary keys unsorted or duplicated"));
                }
                let v = decode_value(data, pos, depth + 1)?;
                last = Some(k.clone());
                map.insert(k, v);
            }
            *pos += 1;
            Ok(Bencode::Dict(map))
        }
        Some(b'0'..=b'9') => {
            let len = decode_len(data, pos, b':')?;
            let end = pos
                .checked_add(len)
                .filter(|e| *e <= data.len())
                .ok_or_else(|| bad("string"))?;
            let b = data[*pos..end].to_vec();
            *pos = end;
            Ok(Bencode::Bytes(b))
        }
        _ => Err(bad("unexpected byte")),
    }
}

/// Root of a Merkle tree over `leaves`, padded with `pad` to a power of two
/// of at least `min_leaves` leaves.
fn merkle_root(leaves: &[[u8; 32]], pad: [u8; 32], min_leaves: usize) -> [u8; 32] {
    let n = leaves.len().max(min_leaves).max(1).next_power_of_two();
    let mut layer: Vec<[u8; 32]> = leaves.to_vec();
    layer.resize(n, pad);
    while layer.len() > 1 {
        layer = layer
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| hash_pair(&p[0], &p[1]))
            .collect();
    }
    layer[0]
}

fn hash_pair(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.finalize().into()
}

/// The result of hashing one file for a v2 torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHashes {
    /// File length in bytes.
    pub length: u64,
    /// `pieces root`; `None` for an empty file.
    pub pieces_root: Option<[u8; 32]>,
    /// Piece-layer hashes; empty unless the file is larger than one piece.
    pub piece_layer: Vec<[u8; 32]>,
}

/// Incremental BEP 52 file hasher.
#[derive(Debug, Clone)]
pub struct PiecesHasher {
    piece_length: u64,
    buf: Vec<u8>,
    leaves: Vec<[u8; 32]>,
    length: u64,
}

impl Default for PiecesHasher {
    fn default() -> Self {
        Self::new(PIECE_LENGTH)
    }
}

impl PiecesHasher {
    /// A hasher for the given piece length (a power of two, at least 16 KiB;
    /// other values are rounded up to one).
    #[must_use]
    pub fn new(piece_length: u64) -> Self {
        Self {
            piece_length: piece_length.max(BLOCK_SIZE as u64).next_power_of_two(),
            buf: Vec::with_capacity(BLOCK_SIZE),
            leaves: Vec::new(),
            length: 0,
        }
    }

    /// Adds file bytes.
    pub fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        if !self.buf.is_empty() {
            let take = (BLOCK_SIZE - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == BLOCK_SIZE {
                self.leaves.push(Sha256::digest(&self.buf).into());
                self.buf.clear();
            }
        }
        while data.len() >= BLOCK_SIZE {
            let (block, rest) = data.split_at(BLOCK_SIZE);
            self.leaves.push(Sha256::digest(block).into());
            data = rest;
        }
        self.buf.extend_from_slice(data);
    }

    /// Finishes the file.
    #[must_use]
    pub fn finish(mut self) -> FileHashes {
        if !self.buf.is_empty() {
            self.leaves.push(Sha256::digest(&self.buf).into());
        }
        if self.length == 0 {
            return FileHashes {
                length: 0,
                pieces_root: None,
                piece_layer: Vec::new(),
            };
        }
        let root = merkle_root(&self.leaves, [0u8; 32], 1);
        let blocks_per_piece = (self.piece_length / BLOCK_SIZE as u64) as usize;
        let piece_layer = if self.length > self.piece_length {
            let pieces = self.length.div_ceil(self.piece_length) as usize;
            // Hash of a piece made only of padding leaves.
            let zero_piece = merkle_root(&[], [0u8; 32], blocks_per_piece);
            (0..pieces)
                .map(|i| {
                    let start = i * blocks_per_piece;
                    let end = (start + blocks_per_piece).min(self.leaves.len());
                    if start >= end {
                        zero_piece
                    } else {
                        merkle_root(&self.leaves[start..end], [0u8; 32], blocks_per_piece)
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        FileHashes {
            length: self.length,
            pieces_root: Some(root),
            piece_layer,
        }
    }
}

/// Hashes a whole file in memory.
#[must_use]
pub fn hash_file(data: &[u8]) -> FileHashes {
    let mut h = PiecesHasher::default();
    h.update(data);
    h.finish()
}

/// One file of a torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    /// Path inside the torrent, `/`-separated.
    pub path: String,
    /// Hashes from [`PiecesHasher`].
    pub hashes: FileHashes,
}

/// A built v2 torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Torrent {
    /// Torrent name (the archive).
    pub name: String,
    /// v2 infohash.
    pub infohash: Sha256Digest,
    /// Bencoded `info` dictionary.
    pub info: Vec<u8>,
    /// Complete `.torrent` file (info, piece layers, web seeds, trackers).
    pub metainfo: Vec<u8>,
}

/// Builds the `info` dictionary for `files` (paths unique and sorted or not;
/// the file tree is a sorted dictionary either way).
///
/// # Errors
///
/// Fails on an empty, duplicate or conflicting path.
pub fn build_info(name: &str, piece_length: u64, files: &[TorrentFile]) -> Result<Bencode> {
    let mut tree: BTreeMap<Vec<u8>, Bencode> = BTreeMap::new();
    for f in files {
        let segs: Vec<&str> = f.path.split('/').collect();
        if segs.iter().any(|s| s.is_empty()) {
            return Err(Error::invalid(format!("torrent path {:?}", f.path)));
        }
        let mut leaf = BTreeMap::new();
        leaf.insert(
            b"length".to_vec(),
            Bencode::Int(
                i64::try_from(f.hashes.length).map_err(|_| Error::invalid("file too large"))?,
            ),
        );
        if let Some(root) = f.hashes.pieces_root {
            leaf.insert(b"pieces root".to_vec(), Bencode::Bytes(root.to_vec()));
        }
        let mut file_entry = BTreeMap::new();
        file_entry.insert(Vec::new(), Bencode::Dict(leaf));
        insert_path(&mut tree, &segs, Bencode::Dict(file_entry), &f.path)?;
    }
    let mut info = BTreeMap::new();
    info.insert(b"file tree".to_vec(), Bencode::Dict(tree));
    info.insert(b"meta version".to_vec(), Bencode::Int(2));
    info.insert(b"name".to_vec(), Bencode::str(name));
    info.insert(
        b"piece length".to_vec(),
        Bencode::Int(i64::try_from(piece_length).map_err(|_| Error::invalid("piece length"))?),
    );
    Ok(Bencode::Dict(info))
}

fn insert_path(
    tree: &mut BTreeMap<Vec<u8>, Bencode>,
    segs: &[&str],
    file: Bencode,
    full: &str,
) -> Result<()> {
    let conflict = || Error::invalid(format!("torrent path conflict at {full:?}"));
    let (first, rest) = segs.split_first().ok_or_else(conflict)?;
    let key = first.as_bytes().to_vec();
    if rest.is_empty() {
        if tree.contains_key(&key) {
            return Err(conflict());
        }
        tree.insert(key, file);
        return Ok(());
    }
    let entry = tree
        .entry(key)
        .or_insert_with(|| Bencode::Dict(BTreeMap::new()));
    match entry {
        Bencode::Dict(sub) if !sub.contains_key(&Vec::new()) => insert_path(sub, rest, file, full),
        _ => Err(conflict()),
    }
}

/// Builds a complete torrent: `info`, `piece layers`, `url-list` (web
/// seeds) and, if given, `announce` / `announce-list` (trackers). No
/// creation date or other variable fields are written, so the metainfo is
/// reproducible too.
///
/// # Errors
///
/// Fails on invalid paths.
pub fn build_torrent(
    name: &str,
    files: &[TorrentFile],
    web_seeds: &[String],
    trackers: &[String],
) -> Result<Torrent> {
    let info = build_info(name, PIECE_LENGTH, files)?;
    let info_bytes = info.encode();
    let infohash = Sha256Digest(Sha256::digest(&info_bytes).into());
    let mut layers = BTreeMap::new();
    for f in files {
        if let (Some(root), false) = (f.hashes.pieces_root, f.hashes.piece_layer.is_empty()) {
            layers.insert(root.to_vec(), Bencode::Bytes(f.hashes.piece_layer.concat()));
        }
    }
    let mut meta = BTreeMap::new();
    meta.insert(b"info".to_vec(), info);
    meta.insert(b"piece layers".to_vec(), Bencode::Dict(layers));
    if !web_seeds.is_empty() {
        meta.insert(
            b"url-list".to_vec(),
            Bencode::List(web_seeds.iter().map(Bencode::str).collect()),
        );
    }
    if let Some(first) = trackers.first() {
        meta.insert(b"announce".to_vec(), Bencode::str(first));
        meta.insert(
            b"announce-list".to_vec(),
            Bencode::List(
                trackers
                    .iter()
                    .map(|t| Bencode::List(vec![Bencode::str(t)]))
                    .collect(),
            ),
        );
    }
    Ok(Torrent {
        name: name.to_owned(),
        infohash,
        info: info_bytes,
        metainfo: Bencode::Dict(meta).encode(),
    })
}

/// Percent-encodes a magnet parameter value (RFC 3986 unreserved kept).
#[must_use]
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The magnet URI of a day's torrent:
/// `magnet:?xt=urn:btmh:1220<infohash>&dn=<archive>-<day>&ws=<seed>...`.
#[must_use]
pub fn magnet(infohash: &Sha256Digest, display_name: &str, web_seeds: &[String]) -> String {
    let mut m = format!(
        "magnet:?xt=urn:btmh:1220{}&dn={}",
        hex::encode(&infohash.0),
        percent_encode(display_name)
    );
    for ws in web_seeds {
        m.push_str("&ws=");
        m.push_str(&percent_encode(ws));
    }
    m
}

/// A file listed in a torrent: path, length and pieces root.
pub type ListedFile = (String, u64, Option<[u8; 32]>);

/// Checks a `.torrent` file's piece layers against its file tree and
/// returns `(path, length, pieces root)` for every file, plus the infohash.
///
/// # Errors
///
/// Fails if the metainfo is malformed or inconsistent.
pub fn inspect_metainfo(metainfo: &[u8]) -> Result<(Sha256Digest, Vec<ListedFile>)> {
    let meta = Bencode::decode(metainfo)?;
    let info = meta.get("info").ok_or_else(|| bad("no info"))?;
    let infohash = Sha256Digest(Sha256::digest(info.encode()).into());
    if info.get("meta version").and_then(Bencode::as_int) != Some(2) {
        return Err(bad("not a v2 torrent"));
    }
    let piece_length = info
        .get("piece length")
        .and_then(Bencode::as_int)
        .and_then(|p| u64::try_from(p).ok())
        .ok_or_else(|| bad("piece length"))?;
    let mut files = Vec::new();
    walk_tree(
        info.get("file tree").ok_or_else(|| bad("no file tree"))?,
        "",
        &mut files,
    )?;
    let layers = meta.get("piece layers");
    for (path, len, root) in &files {
        if *len > piece_length {
            let root = root.ok_or_else(|| bad("missing pieces root"))?;
            let layer = layers
                .and_then(|l| match l {
                    Bencode::Dict(m) => m.get(root.as_slice()),
                    _ => None,
                })
                .and_then(Bencode::as_bytes)
                .ok_or_else(|| bad("missing piece layer"))?;
            let hashes: Vec<[u8; 32]> = layer
                .chunks(32)
                .map(|c| <[u8; 32]>::try_from(c).map_err(|_| bad("piece layer length")))
                .collect::<Result<_>>()?;
            if hashes.len() as u64 != len.div_ceil(piece_length) {
                return Err(bad("piece layer count"));
            }
            let blocks_per_piece = (piece_length / BLOCK_SIZE as u64) as usize;
            let zero_piece = merkle_root(&[], [0u8; 32], blocks_per_piece);
            if merkle_root(&hashes, zero_piece, 1) != root {
                return Err(Error::invalid(format!(
                    "{path}: piece layer does not match pieces root"
                )));
            }
        }
    }
    Ok((infohash, files))
}

fn walk_tree(node: &Bencode, prefix: &str, out: &mut Vec<ListedFile>) -> Result<()> {
    let Bencode::Dict(map) = node else {
        return Err(bad("file tree node"));
    };
    for (k, v) in map {
        if k.is_empty() {
            let len = v
                .get("length")
                .and_then(Bencode::as_int)
                .and_then(|l| u64::try_from(l).ok())
                .ok_or_else(|| bad("length"))?;
            let root = match v.get("pieces root").and_then(Bencode::as_bytes) {
                Some(r) => Some(<[u8; 32]>::try_from(r).map_err(|_| bad("pieces root"))?),
                None => None,
            };
            out.push((prefix.to_owned(), len, root));
        } else {
            let name = std::str::from_utf8(k).map_err(|_| bad("path"))?;
            let p = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            walk_tree(v, &p, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(b: &[u8]) -> [u8; 32] {
        Sha256::digest(b).into()
    }

    #[test]
    fn bencode_round_trip_and_canonical() {
        let v = Bencode::decode(b"d3:bari-5e3:fool4:spami0eee").unwrap();
        assert_eq!(v.encode(), b"d3:bari-5e3:fool4:spami0eee");
        for bad in [
            &b"i01e"[..],
            b"i-0e",
            b"d3:foo1:a3:bar1:be",
            b"d1:b1:x1:a1:ye",
            b"3:ab",
            b"le extra",
        ] {
            assert!(
                Bencode::decode(bad).is_err(),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn bep52_small_file_root_is_block_hash() {
        // A file of at most one block: the pieces root is SHA-256 of the
        // data itself (a one-leaf tree).
        let h = hash_file(b"hello");
        assert_eq!(h.pieces_root, Some(sha(b"hello")));
        assert!(h.piece_layer.is_empty());
        assert_eq!(hash_file(b"").pieces_root, None);
    }

    #[test]
    fn bep52_padding_with_zero_leaves() {
        // 3 blocks (2 full + 1 byte): leaves padded to 4 with zero hashes.
        let mut data = vec![0xabu8; 2 * BLOCK_SIZE];
        data.push(1);
        let l0 = sha(&data[..BLOCK_SIZE]);
        let l1 = sha(&data[BLOCK_SIZE..2 * BLOCK_SIZE]);
        let l2 = sha(&[1]);
        let z = [0u8; 32];
        let want = hash_pair(&hash_pair(&l0, &l1), &hash_pair(&l2, &z));
        let got = hash_file(&data);
        assert_eq!(got.pieces_root, Some(want));
        assert!(got.piece_layer.is_empty());
    }

    #[test]
    fn bep52_piece_layers() {
        // With a 32 KiB piece length, a 5-block file has 3 pieces: the piece
        // layer is the tree layer one above the leaves, and the root pads
        // the 3 piece hashes with the hash of an all-padding piece.
        let data: Vec<u8> = (0..5 * BLOCK_SIZE).map(|i| (i % 251) as u8).collect();
        let leaves: Vec<[u8; 32]> = data.chunks(BLOCK_SIZE).map(sha).collect();
        let z = [0u8; 32];
        let p0 = hash_pair(&leaves[0], &leaves[1]);
        let p1 = hash_pair(&leaves[2], &leaves[3]);
        let p2 = hash_pair(&leaves[4], &z);
        let zp = hash_pair(&z, &z);
        let root = hash_pair(&hash_pair(&p0, &p1), &hash_pair(&p2, &zp));
        let mut h = PiecesHasher::new(2 * BLOCK_SIZE as u64);
        for piece in data.chunks(7000) {
            h.update(piece);
        }
        let got = h.finish();
        assert_eq!(got.pieces_root, Some(root));
        assert_eq!(got.piece_layer, vec![p0, p1, p2]);
    }

    #[test]
    fn info_dictionary_bytes() {
        // Exact bencoding of a v2 info dictionary per BEP 52, written by hand.
        let files = vec![
            TorrentFile {
                path: "b/x.bin".into(),
                hashes: hash_file(b"hello"),
            },
            TorrentFile {
                path: "a.txt".into(),
                hashes: hash_file(b""),
            },
        ];
        let info = build_info("arch", 1 << 20, &files).unwrap().encode();
        let mut want = Vec::new();
        want.extend_from_slice(
            b"d9:file treed5:a.txtd0:d6:lengthi0eee1:bd5:x.bind0:d6:lengthi5e11:pieces root32:",
        );
        want.extend_from_slice(&sha(b"hello"));
        want.extend_from_slice(b"eeee12:meta versioni2e4:name4:arch12:piece lengthi1048576ee");
        assert_eq!(info, want);

        let t = build_torrent("arch", &files, &["https://m.example/".into()], &[]).unwrap();
        assert_eq!(t.infohash, Sha256Digest(sha(&want)));
        let (ih, listed) = inspect_metainfo(&t.metainfo).unwrap();
        assert_eq!(ih, t.infohash);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1], ("b/x.bin".to_owned(), 5, Some(sha(b"hello"))));
        assert!(build_info("arch", 1 << 20, &[files[0].clone(), files[0].clone()]).is_err());
    }

    #[test]
    fn metainfo_carries_piece_layers() {
        let big = vec![3u8; (PIECE_LENGTH as usize) * 2 + 10];
        let f = TorrentFile {
            path: "d/big".into(),
            hashes: hash_file(&big),
        };
        assert_eq!(f.hashes.piece_layer.len(), 3);
        let t = build_torrent(
            "arch",
            std::slice::from_ref(&f),
            &[],
            &["udp://t.example:1337".into()],
        )
        .unwrap();
        let (_, listed) = inspect_metainfo(&t.metainfo).unwrap();
        assert_eq!(listed[0].2, f.hashes.pieces_root);
        let meta = Bencode::decode(&t.metainfo).unwrap();
        assert!(meta.get("announce").is_some());
        assert!(meta.get("creation date").is_none());
    }

    #[test]
    fn magnet_format() {
        let ih = Sha256Digest([0xab; 32]);
        let m = magnet(
            &ih,
            "hyperliquid-raw-2026-01-01",
            &["https://a.example/x/".into()],
        );
        assert_eq!(
            m,
            format!(
                "magnet:?xt=urn:btmh:1220{}&dn=hyperliquid-raw-2026-01-01&ws=https%3A%2F%2Fa.example%2Fx%2F",
                "ab".repeat(32)
            )
        );
    }
}
