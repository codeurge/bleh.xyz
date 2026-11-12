//! One-pass file digests: size, SHA-256, IPFS CID and BitTorrent v2 hashes.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use mesh_types::Sha256Digest;
use mesh_types::manifest::IpfsRef;

use crate::error::{IoContext, Result};
use crate::ipfs::UnixFsHasher;
use crate::torrent::{FileHashes, PiecesHasher};

/// Everything a manifest records about a file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the bytes.
    pub sha256: Sha256Digest,
    /// IPFS reference with the parameters used.
    pub ipfs: IpfsRef,
    /// BitTorrent v2 hashes.
    pub torrent: FileHashes,
}

impl FileDigest {
    /// The BitTorrent v2 `pieces root`, if the file is not empty.
    #[must_use]
    pub fn pieces_root(&self) -> Option<Sha256Digest> {
        self.torrent.pieces_root.map(Sha256Digest)
    }
}

/// Incremental computation of a [`FileDigest`].
#[derive(Debug, Clone)]
pub struct FileDigester {
    sha: Sha256,
    ipfs: UnixFsHasher,
    bt: PiecesHasher,
    size: u64,
}

impl Default for FileDigester {
    fn default() -> Self {
        Self::new(UnixFsHasher::default())
    }
}

impl FileDigester {
    /// A digester using `ipfs` for the CID (to honour recorded chunker
    /// parameters).
    #[must_use]
    pub fn new(ipfs: UnixFsHasher) -> Self {
        Self {
            sha: Sha256::new(),
            ipfs,
            bt: PiecesHasher::default(),
            size: 0,
        }
    }

    /// Adds bytes.
    pub fn update(&mut self, data: &[u8]) {
        self.sha.update(data);
        self.ipfs.update(data);
        self.bt.update(data);
        self.size += data.len() as u64;
    }

    /// Finishes.
    #[must_use]
    pub fn finish(self) -> FileDigest {
        FileDigest {
            size: self.size,
            sha256: Sha256Digest(self.sha.finalize().into()),
            ipfs: self.ipfs.finish_ref(),
            torrent: self.bt.finish(),
        }
    }
}

/// Digests bytes in memory.
#[must_use]
pub fn digest_bytes(data: &[u8]) -> FileDigest {
    let mut d = FileDigester::default();
    d.update(data);
    d.finish()
}

/// Digests a file, streaming.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn digest_file(path: &Path) -> Result<FileDigest> {
    digest_file_with(path, FileDigester::default())
}

/// Digests a file with a configured digester.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn digest_file_with(path: &Path, mut digester: FileDigester) -> Result<FileDigest> {
    let mut f = std::fs::File::open(path).at(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).at(path)?;
        if n == 0 {
            break;
        }
        digester.update(&buf[..n]);
    }
    Ok(digester.finish())
}

/// SHA-256 of a file, streaming.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn sha256_file(path: &Path) -> Result<Sha256Digest> {
    let mut f = std::fs::File::open(path).at(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).at(path)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(Sha256Digest(h.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_and_memory_agree() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 7) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let a = digest_file(&p).unwrap();
        let b = digest_bytes(&data);
        assert_eq!(a, b);
        assert_eq!(a.size, 3_000_000);
        assert_eq!(a.sha256, Sha256Digest::of(&data));
        assert_eq!(sha256_file(&p).unwrap(), a.sha256);
        assert_eq!(a.torrent.piece_layer.len(), 3);
        assert!(a.ipfs.cid.starts_with("bafy"));
    }
}
