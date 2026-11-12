//! Building, signing, storing and indexing daily manifests
//! (`docs/specs/manifest-v0.md`).
//!
//! A day's manifest lists every data file of the archive whose `date=`
//! segment is that day, with size, SHA-256, IPFS CID (UnixFS, raw leaves,
//! 1 MiB chunks), BitTorrent v2 pieces root and, for normalized partitions,
//! the canonical content hash and record count read back from the Parquet
//! file (and checked against its metadata: the determinism gate). The
//! day's v2 torrent (name = archive, 1 MiB pieces, web seeds) gives the
//! infohash and magnet. The previous day's manifest hash chains the days.
//!
//! `manifest.json` is written as the canonical JSON of the signed manifest,
//! so its SHA-256 is the manifest hash.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use mesh_types::manifest::{MANIFEST_FORMAT, Manifest, ManifestFile, SignedManifest, TorrentInfo};
use mesh_types::{EthAddress, Sha256Digest, SigningKey, UnixNanos};

use crate::digest::{self, FileDigest};
use crate::error::{Error, IoContext, Result};
use crate::layout::{self, Day};
use crate::partition;
use crate::torrent::{self, Torrent, TorrentFile};

/// Settings for [`build`].
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Archive name.
    pub archive: String,
    /// Day.
    pub day: Day,
    /// HTTPS base URLs (each ending in `/`) serving `<seed><archive>/<path>`.
    pub web_seeds: Vec<String>,
    /// Optional tracker URLs for the `.torrent` file (not in the infohash).
    pub trackers: Vec<String>,
    /// Build time.
    pub created_ns: UnixNanos,
    /// Address of the signing key.
    pub signer: EthAddress,
    /// Successor key, when rotating.
    pub next_signer: Option<EthAddress>,
    /// The previous manifest of this archive; `None` starts a chain at
    /// sequence 0.
    pub previous: Option<SignedManifest>,
}

/// A built (unsigned) manifest with its torrent and per-file digests.
#[derive(Debug, Clone)]
pub struct Built {
    /// The manifest body.
    pub manifest: Manifest,
    /// The day's torrent.
    pub torrent: Torrent,
    /// Path and digest of every file, in manifest order.
    pub digests: Vec<(String, FileDigest)>,
}

/// The data files of `day` under the archive root `root`, sorted by path.
///
/// # Errors
///
/// Fails on I/O errors or a path a manifest cannot list.
pub fn day_files(root: &Path, day: Day) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in layout::list_files(root)? {
        if layout::is_data_path(&p) && layout::day_of_path(&p) == Some(day) {
            if !p.bytes().all(|c| c.is_ascii_graphic() && c != b'\\') || p.len() > 512 {
                return Err(Error::invalid(format!(
                    "{p:?} cannot be listed in a manifest"
                )));
            }
            out.push(p);
        }
    }
    Ok(out)
}

/// Content hash and count for a normalized partition file, checked against
/// its declared metadata.
///
/// # Errors
///
/// Fails if the file cannot be read or its records do not hash to the
/// declared content hash.
pub fn partition_content(path: &Path) -> Result<(mesh_types::ContentHash, u64)> {
    let bytes = bytes::Bytes::from(std::fs::read(path).at(path)?);
    let read = partition::read_parquet(bytes)?;
    if !read.consistent() {
        return Err(Error::invalid(format!(
            "{}: records hash to {} ({} records) but the file declares {} ({})",
            path.display(),
            read.content_hash,
            read.records.len(),
            read.declared_hash,
            read.declared_count
        )));
    }
    Ok((read.content_hash, read.declared_count))
}

/// Builds the manifest of `opts.day` from the files under the archive root
/// `root`.
///
/// # Errors
///
/// Fails if the day has no files, a partition fails the content-hash gate,
/// or the result breaks a manifest rule.
pub fn build(root: &Path, opts: &BuildOptions) -> Result<Built> {
    layout::check_archive(&opts.archive)?;
    let paths = day_files(root, opts.day)?;
    if paths.is_empty() {
        return Err(Error::invalid(format!(
            "{} has no files for {}; a day with no files has no manifest",
            opts.archive, opts.day
        )));
    }
    let mut files = Vec::with_capacity(paths.len());
    let mut digests = Vec::with_capacity(paths.len());
    let mut tfiles = Vec::with_capacity(paths.len());
    for rel in &paths {
        let local = local(root, rel);
        let d = digest::digest_file(&local)?;
        let (content_hash, record_count) = if layout::parse_partition_path(rel).is_some() {
            let (h, n) = partition_content(&local)?;
            (Some(h), Some(n))
        } else {
            (None, None)
        };
        files.push(ManifestFile {
            path: rel.clone(),
            size: d.size,
            sha256: d.sha256,
            content_hash,
            record_count,
            ipfs: d.ipfs.clone(),
            bt_pieces_root: d.pieces_root(),
        });
        tfiles.push(TorrentFile {
            path: rel.clone(),
            hashes: d.torrent.clone(),
        });
        digests.push((rel.clone(), d));
    }
    let torrent = torrent::build_torrent(&opts.archive, &tfiles, &opts.web_seeds, &opts.trackers)?;
    let magnet = torrent::magnet(
        &torrent.infohash,
        &format!("{}-{}", opts.archive, opts.day),
        &opts.web_seeds,
    );
    let (sequence, previous_manifest_sha256) = match &opts.previous {
        None => (0, None),
        Some(p) => {
            if p.manifest.archive != opts.archive {
                return Err(Error::invalid("previous manifest is from another archive"));
            }
            if p.manifest.date.as_str() >= opts.day.to_string().as_str() {
                return Err(Error::invalid(
                    "previous manifest is not from an earlier day",
                ));
            }
            (p.manifest.sequence + 1, Some(p.sha256()?))
        }
    };
    let manifest = Manifest {
        format: MANIFEST_FORMAT.to_owned(),
        archive: opts.archive.clone(),
        date: opts.day.to_string(),
        sequence,
        created_ns: opts.created_ns,
        previous_manifest_sha256,
        files,
        torrent: TorrentInfo {
            infohash_v2: torrent.infohash,
            magnet,
        },
        web_seeds: opts.web_seeds.clone(),
        signer: opts.signer,
        next_signer: opts.next_signer,
    };
    manifest.validate()?;
    Ok(Built {
        manifest,
        torrent,
        digests,
    })
}

/// Parses a signed manifest (`manifest.json`).
///
/// # Errors
///
/// Fails on malformed JSON or unknown fields.
pub fn parse_signed(bytes: &[u8]) -> Result<SignedManifest> {
    Ok(serde_json::from_slice(bytes)?)
}

/// Reads a day's signed manifest from a local archive root, if present.
///
/// # Errors
///
/// Fails on I/O or parse errors.
pub fn read_signed(root: &Path, day: Day) -> Result<Option<SignedManifest>> {
    let p = local(root, &layout::manifest_file(day, layout::MANIFEST_FILE));
    if !p.exists() {
        return Ok(None);
    }
    parse_signed(&std::fs::read(&p).at(&p)?).map(Some)
}

fn local(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/') {
        p.push(seg);
    }
    p
}

/// Days with a signed manifest under a local archive root, ascending.
///
/// # Errors
///
/// Fails on I/O errors.
pub fn manifest_days(root: &Path) -> Result<Vec<Day>> {
    let dir = root.join(layout::MANIFESTS_DIR);
    let mut days = Vec::new();
    if dir.exists() {
        for e in std::fs::read_dir(&dir).at(&dir)? {
            let e = e.at(&dir)?;
            if let Some(day) = e.file_name().to_str().and_then(|n| n.parse::<Day>().ok())
                && e.path().join(layout::MANIFEST_FILE).exists()
            {
                days.push(day);
            }
        }
    }
    days.sort();
    Ok(days)
}

/// The latest signed manifest strictly before `day`, if any.
///
/// # Errors
///
/// Fails on I/O or parse errors.
pub fn previous_for(root: &Path, day: Day) -> Result<Option<SignedManifest>> {
    match manifest_days(root)?.into_iter().rfind(|d| *d < day) {
        Some(d) => read_signed(root, d),
        None => Ok(None),
    }
}

/// Writes the unsigned body for later signing (`manifest.unsigned.json`).
///
/// # Errors
///
/// Fails on I/O errors.
pub fn write_unsigned(root: &Path, m: &Manifest) -> Result<PathBuf> {
    let day: Day = m.date.parse()?;
    let p = local(
        root,
        &layout::manifest_file(day, layout::UNSIGNED_MANIFEST_FILE),
    );
    let mut bytes = serde_json::to_vec_pretty(m)?;
    bytes.push(b'\n');
    partition::write_atomic(&p, &bytes)?;
    Ok(p)
}

/// Reads an unsigned body written by [`write_unsigned`].
///
/// # Errors
///
/// Fails on I/O or parse errors.
pub fn read_unsigned(root: &Path, day: Day) -> Result<Manifest> {
    let p = local(
        root,
        &layout::manifest_file(day, layout::UNSIGNED_MANIFEST_FILE),
    );
    Ok(serde_json::from_slice(&std::fs::read(&p).at(&p)?)?)
}

/// Signs `m` with `key` (which must match `m.signer`).
///
/// # Errors
///
/// Fails if the manifest is invalid or the key is not its signer.
pub fn sign(m: Manifest, key: &SigningKey) -> Result<SignedManifest> {
    Ok(m.sign(key)?)
}

/// One entry of the manifest index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexEntry {
    /// Day.
    pub date: Day,
    /// Chain position.
    pub sequence: u64,
    /// Manifest hash.
    pub sha256: Sha256Digest,
}

/// `_manifests/index.json`: every manifest of an archive, so mirrors
/// without directory listings (plain HTTPS, IPFS) can walk the chain. It is
/// not trusted: every manifest it points at is verified by hash and
/// signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestIndex {
    /// Always `bleh-manifest-index-v0`.
    pub format: String,
    /// Archive name.
    pub archive: String,
    /// Entries, ascending by sequence.
    pub manifests: Vec<IndexEntry>,
}

/// Format tag of [`ManifestIndex`].
pub const INDEX_FORMAT: &str = "bleh-manifest-index-v0";

impl ManifestIndex {
    /// An empty index.
    #[must_use]
    pub fn new(archive: &str) -> Self {
        Self {
            format: INDEX_FORMAT.to_owned(),
            archive: archive.to_owned(),
            manifests: Vec::new(),
        }
    }

    /// Adds or replaces the entry for `signed`'s sequence.
    ///
    /// # Errors
    ///
    /// Fails if the manifest cannot be hashed or belongs to another archive.
    pub fn insert(&mut self, signed: &SignedManifest) -> Result<()> {
        if signed.manifest.archive != self.archive {
            return Err(Error::invalid("index is for another archive"));
        }
        let entry = IndexEntry {
            date: signed.manifest.date.parse()?,
            sequence: signed.manifest.sequence,
            sha256: signed.sha256()?,
        };
        self.manifests.retain(|e| e.sequence != entry.sequence);
        self.manifests.push(entry);
        self.manifests.sort_by_key(|e| e.sequence);
        Ok(())
    }

    /// The entry at `sequence`.
    #[must_use]
    pub fn at(&self, sequence: u64) -> Option<&IndexEntry> {
        self.manifests.iter().find(|e| e.sequence == sequence)
    }
}

/// Rebuilds the index from every manifest under a local archive root.
///
/// # Errors
///
/// Fails on I/O or parse errors.
pub fn rebuild_index(root: &Path, archive: &str) -> Result<ManifestIndex> {
    let mut idx = ManifestIndex::new(archive);
    for d in manifest_days(root)? {
        if let Some(m) = read_signed(root, d)? {
            idx.insert(&m)?;
        }
    }
    Ok(idx)
}

/// Files written for a signed day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenDay {
    /// `_manifests/<day>/manifest.json`.
    pub manifest: String,
    /// `_manifests/<day>/<archive>-<day>.torrent`.
    pub torrent: String,
    /// The manifest hash.
    pub sha256: Sha256Digest,
}

/// Writes the signed manifest (write-once, canonical JSON), the day's
/// `.torrent` (write-once), and refreshes `_manifests/index.json` and
/// `_manifests/magnets.txt` under the archive root.
///
/// # Errors
///
/// Fails on I/O errors or if a different manifest or torrent already
/// exists for the day.
pub fn write_signed(
    root: &Path,
    signed: &SignedManifest,
    torrent_metainfo: &[u8],
) -> Result<WrittenDay> {
    let m = &signed.manifest;
    let day: Day = m.date.parse()?;
    let manifest = layout::manifest_file(day, layout::MANIFEST_FILE);
    partition::write_once(&local(root, &manifest), &signed.canonical_bytes()?)?;
    let (infohash, _) = torrent::inspect_metainfo(torrent_metainfo)?;
    if infohash != m.torrent.infohash_v2 {
        return Err(Error::invalid(
            "torrent does not match the manifest's infohash",
        ));
    }
    let torrent = layout::torrent_file(&m.archive, day);
    partition::write_once(&local(root, &torrent), torrent_metainfo)?;
    refresh_indexes(root, &m.archive)?;
    Ok(WrittenDay {
        manifest,
        torrent,
        sha256: signed.sha256()?,
    })
}

/// Rewrites `_manifests/index.json` and `_manifests/magnets.txt` from the
/// manifests present.
///
/// # Errors
///
/// Fails on I/O or parse errors.
pub fn refresh_indexes(root: &Path, archive: &str) -> Result<()> {
    let idx = rebuild_index(root, archive)?;
    let mut bytes = serde_json::to_vec_pretty(&idx)?;
    bytes.push(b'\n');
    partition::write_atomic(&local(root, layout::MANIFEST_INDEX_FILE), &bytes)?;
    let mut magnets = String::new();
    for e in &idx.manifests {
        if let Some(m) = read_signed(root, e.date)? {
            magnets.push_str(&format!(
                "{} {}\n",
                m.manifest.date, m.manifest.torrent.magnet
            ));
        }
    }
    partition::write_atomic(&local(root, layout::MAGNETS_FILE), magnets.as_bytes())?;
    Ok(())
}

/// Loads a secp256k1 signing key from a file holding 64 hex digits
/// (optionally `0x`-prefixed, surrounding whitespace ignored). Warns if the
/// file is readable by group or others.
///
/// # Errors
///
/// Fails if the file is missing or malformed. The key is never printed.
pub fn load_signing_key(path: &Path) -> Result<SigningKey> {
    let text = std::fs::read_to_string(path).at(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            tracing::warn!(path = %path.display(), "signing key file is readable by group or others; chmod 600 it");
        }
    }
    let bytes = crate::hex::decode_array::<32>(text.trim())
        .ok_or_else(|| Error::invalid(format!("{}: expected 64 hex digits", path.display())))?;
    SigningKey::from_bytes(&bytes).map_err(|e| Error::invalid(format!("{}: {e}", path.display())))
}

/// Generates a new signing key from the operating system's randomness and
/// writes it to `path` (mode 600, never overwriting). Returns its address.
///
/// # Errors
///
/// Fails if `path` exists or randomness is unavailable.
pub fn generate_signing_key(path: &Path) -> Result<EthAddress> {
    use std::io::{Read, Write};
    if path.exists() {
        return Err(Error::invalid(format!(
            "{} exists; not overwriting",
            path.display()
        )));
    }
    let mut bytes = [0u8; 32];
    let mut key = None;
    for _ in 0..8 {
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .at("/dev/urandom")?;
        if let Ok(k) = SigningKey::from_bytes(&bytes) {
            key = Some(k);
            break;
        }
    }
    let key = key.ok_or_else(|| Error::invalid("could not generate a valid key"))?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).at(path)?;
    f.write_all(format!("{}\n", crate::hex::encode(&bytes)).as_bytes())
        .at(path)?;
    bytes.fill(0);
    Ok(key.address())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{PartitionOptions, write_partitions};
    use crate::testutil::{every_kind, key};
    use mesh_types::manifest::{IpfsCodec, verify_link, verify_signed};

    pub(crate) fn opts(
        day: &str,
        k: &SigningKey,
        previous: Option<SignedManifest>,
    ) -> BuildOptions {
        BuildOptions {
            archive: "hyperliquid-normalized".into(),
            day: day.parse().unwrap(),
            web_seeds: vec!["https://mirror.example.org/".into()],
            trackers: vec![],
            created_ns: UnixNanos(1_767_312_000_000_000_000),
            signer: k.address(),
            next_signer: None,
            previous,
        }
    }

    #[test]
    fn builds_signs_and_chains() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let written = write_partitions(root, every_kind(), &PartitionOptions::default()).unwrap();
        // A stray file outside the day and a reserved directory are ignored.
        std::fs::create_dir_all(root.join("_iceberg/trade")).unwrap();
        std::fs::write(root.join("_iceberg/trade/x.avro"), b"x").unwrap();
        let k = key(1);
        let built = build(root, &opts("2026-01-01", &k, None)).unwrap();
        let m = &built.manifest;
        assert_eq!(m.sequence, 0);
        assert_eq!(m.files.len(), written.len());
        for f in &m.files {
            assert!(f.content_hash.is_some() && f.record_count.is_some());
            assert_eq!(f.ipfs.codec, IpfsCodec::Raw);
            assert!(f.bt_pieces_root.is_some());
        }
        assert!(
            m.torrent
                .magnet
                .contains("&dn=hyperliquid-normalized-2026-01-01")
        );
        // Deterministic.
        let again = build(root, &opts("2026-01-01", &k, None)).unwrap();
        assert_eq!(again.manifest, built.manifest);
        assert_eq!(again.torrent.metainfo, built.torrent.metainfo);

        let signed = sign(built.manifest.clone(), &k).unwrap();
        verify_signed(&signed, &[k.address()]).unwrap();
        let written = write_signed(root, &signed, &built.torrent.metainfo).unwrap();
        let bytes = std::fs::read(root.join(&written.manifest)).unwrap();
        assert_eq!(Sha256Digest::of(&bytes), signed.sha256().unwrap());
        assert_eq!(
            read_signed(root, "2026-01-01".parse().unwrap())
                .unwrap()
                .unwrap(),
            signed
        );
        // Same manifest again is fine; a different one is a conflict.
        write_signed(root, &signed, &built.torrent.metainfo).unwrap();
        let mut other = built.manifest.clone();
        other.created_ns = UnixNanos(1);
        let other = sign(other, &k).unwrap();
        assert!(matches!(
            write_signed(root, &other, &built.torrent.metainfo),
            Err(Error::Conflict { .. })
        ));

        // Next day chains to it.
        let mut recs = every_kind();
        for r in &mut recs {
            r.prov_mut().ts_source_ms.0 += 86_400_000;
            match r {
                mesh_types::Record::Bar(b) => b.start_ms.0 += 86_400_000,
                mesh_types::Record::Footprint(f) => f.start_ms.0 += 86_400_000,
                mesh_types::Record::VolumeProfile(v) => {
                    v.start_ms.0 += 86_400_000;
                    v.end_ms.0 += 86_400_000;
                }
                mesh_types::Record::SwapPrint(s) => s.ts_dissemination_ms.0 += 86_400_000,
                r if r.kind().is_analytics() => {
                    let mut v = serde_json::to_value(&*r).unwrap();
                    for (k, x) in v.as_object_mut().unwrap() {
                        if k.ends_with("_ms") && k != "time_step_ms" && k != "window_ms" {
                            *x = serde_json::Value::from(x.as_i64().unwrap() + 86_400_000);
                        }
                    }
                    *r = serde_json::from_value(v).unwrap();
                }
                _ => {}
            }
        }
        write_partitions(root, recs, &PartitionOptions::default()).unwrap();
        let day2: Day = "2026-01-02".parse().unwrap();
        let prev = previous_for(root, day2).unwrap().unwrap();
        assert_eq!(prev, signed);
        let built2 = build(root, &opts("2026-01-02", &k, Some(prev))).unwrap();
        assert_eq!(built2.manifest.sequence, 1);
        let signed2 = sign(built2.manifest, &k).unwrap();
        verify_link(&signed, &signed2).unwrap();
        write_signed(root, &signed2, &built2.torrent.metainfo).unwrap();
        let idx: ManifestIndex =
            serde_json::from_slice(&std::fs::read(root.join(layout::MANIFEST_INDEX_FILE)).unwrap())
                .unwrap();
        assert_eq!(idx.manifests.len(), 2);
        assert_eq!(idx.at(1).unwrap().sha256, signed2.sha256().unwrap());
        let magnets = std::fs::read_to_string(root.join(layout::MAGNETS_FILE)).unwrap();
        assert_eq!(magnets.lines().count(), 2);
        // Building day 1 again with day 2 as "previous" is refused.
        assert!(build(root, &opts("2026-01-01", &k, Some(signed2))).is_err());
    }

    #[test]
    fn empty_day_and_tampered_partition_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let k = key(1);
        assert!(build(dir.path(), &opts("2026-01-01", &k, None)).is_err());
        let files =
            write_partitions(dir.path(), every_kind(), &PartitionOptions::default()).unwrap();
        // Rewrite one file with a wrong declared hash: the gate catches it.
        let f = &files[0];
        let read = partition::read_parquet(bytes::Bytes::from(
            std::fs::read(dir.path().join(&f.path)).unwrap(),
        ))
        .unwrap();
        let mut recs = read.records.clone();
        let (bytes, _) = partition::write_parquet(read.kind, &recs).unwrap();
        assert_eq!(Sha256Digest::of(&bytes), f.sha256);
        recs.pop();
        if !recs.is_empty() {
            // Same metadata hash would only be produced by a buggy writer;
            // simulate by patching the declared hash into a file with fewer rows.
            let (mut short, _) = partition::write_parquet(read.kind, &recs).unwrap();
            let declared = read.declared_hash.to_string();
            let actual = partition::content_hash_of(&recs).unwrap().0.to_string();
            let pos = short
                .windows(64)
                .position(|w| w == actual.as_bytes())
                .unwrap();
            short[pos..pos + 64].copy_from_slice(declared.as_bytes());
            std::fs::write(dir.path().join(&f.path), &short).unwrap();
            assert!(build(dir.path(), &opts("2026-01-01", &k, None)).is_err());
        }
    }

    #[test]
    fn signing_key_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("key");
        let addr = generate_signing_key(&p).unwrap();
        assert!(generate_signing_key(&p).is_err());
        assert_eq!(load_signing_key(&p).unwrap().address(), addr);
        std::fs::write(dir.path().join("bad"), "zz").unwrap();
        assert!(load_signing_key(&dir.path().join("bad")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
