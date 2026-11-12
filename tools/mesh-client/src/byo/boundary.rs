//! The bring-your-own-feed boundary (ADR 0014), enforced by types.
//!
//! - [`UserSupplied`] wraps every value read from, or derived from, a feed
//!   the user licenses themselves. It has no `Serialize`, no `Deserialize`
//!   and no conversion into any `mesh_types` record, so it cannot reach the
//!   canonical encoder, Merkle leaves, signing, the bus or the archive
//!   writer, which all take `mesh_types` records or `Serialize` values.
//! - [`LocalPath`] is the only way to name an output file. It refuses any
//!   path inside a bleh.xyz archive (an ancestor directory holding
//!   `_manifests/`, `_iceberg/` or a signed bleh manifest, or a Hive
//!   partition directory of the archive layout) and any path inside a
//!   bleh.xyz source checkout.
//! - [`LoopbackAddr`] is the only way to name a local websocket bind
//!   address. It refuses anything but a literal loopback IP.

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Tag carried by every output line built from user-supplied data.
pub const USER_SUPPLIED: &str = "user_supplied";

/// A value read from, or derived from, a feed the user licenses
/// themselves. Processed only by this client, on the user's machine.
///
/// It deliberately implements no serde traits and no conversion into
/// `mesh_types` types. These do not compile:
///
/// ```compile_fail,E0277
/// use mesh_client::byo::UserSupplied;
/// let user = UserSupplied::new(1.5_f64);
/// // No `Serialize`: it cannot reach the canonical encoder (or signing).
/// let _ = mesh_types::hash::canonical_json(&user);
/// ```
///
/// ```compile_fail,E0277
/// use mesh_client::byo::{UserQuote, UserSupplied};
/// fn to_record(user: UserSupplied<UserQuote>) -> mesh_types::Record {
///     // No conversion into a mesh record.
///     user.into()
/// }
/// ```
///
/// ```compile_fail,E0308
/// use mesh_client::byo::{UserQuote, UserSupplied};
/// fn leaf(user: &UserSupplied<UserQuote>) {
///     // Merkle leaves (and so commitments) take mesh records only.
///     let _ = mesh_types::merkle::record_leaf(user);
/// }
/// ```
///
/// ```compile_fail,E0277
/// use mesh_client::byo::{UserQuote, UserSupplied};
/// fn bytes(user: &UserSupplied<UserQuote>) {
///     let _ = serde_json::to_vec(user);
/// }
/// ```
///
/// The same scaffolding compiles without the forbidden step:
///
/// ```
/// use mesh_client::byo::{UserQuote, UserSupplied};
/// let user = UserSupplied::new(UserQuote::trade("IDX-DEC", 1_767_225_600_000_000_000, 101.25));
/// assert_eq!(user.get().symbol, "IDX-DEC");
/// let derived = user.map(|q| q.price_any());
/// assert_eq!(*derived.get(), Some(101.25));
/// ```
#[derive(Clone, PartialEq)]
pub struct UserSupplied<T>(T);

impl<T> UserSupplied<T> {
    /// Wraps a value read from the user's own feed.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Borrows the value for local processing.
    pub const fn get(&self) -> &T {
        &self.0
    }

    /// Derives a new value; it stays user-supplied.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> UserSupplied<U> {
        UserSupplied(f(self.0))
    }
}

impl<T: fmt::Debug> fmt::Debug for UserSupplied<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("UserSupplied").field(&self.0).finish()
    }
}

/// A boundary violation.
#[derive(Debug, thiserror::Error)]
pub enum BoundaryError {
    /// The output path is inside a bleh.xyz archive.
    #[error(
        "refusing to write user-supplied data to {path}: {reason} (ADR 0014: user-supplied data never enters a bleh.xyz archive or repository)"
    )]
    ArchivePath {
        /// The path.
        path: PathBuf,
        /// Which marker was found.
        reason: String,
    },
    /// The output path cannot be resolved.
    #[error("cannot use output path {path}: {reason}")]
    BadPath {
        /// The path.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// The bind address is not loopback.
    #[error(
        "refusing to bind {addr}: the local websocket binds a loopback address only (127.0.0.1 or ::1), so user-supplied data never leaves this machine"
    )]
    NotLoopback {
        /// The address as given.
        addr: String,
    },
}

/// Directory names that mark a bleh.xyz archive root or its metadata.
pub const ARCHIVE_DIR_MARKERS: &[&str] = &["_manifests", "_iceberg"];

/// Hive partition directory prefixes of the archive layout
/// (`docs/specs/archive-layout-v0.md`).
pub const ARCHIVE_PARTITION_PREFIXES: &[&str] =
    &["kind=", "stream=", "venue=", "instrument=", "date="];

/// An output file path that passed the archive and repository checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPath(PathBuf);

impl LocalPath {
    /// Checks `path` and returns it if user-supplied output may be written
    /// there. The parent directory must exist.
    ///
    /// # Errors
    ///
    /// [`BoundaryError::ArchivePath`] if the path, or any ancestor, is a
    /// bleh.xyz archive directory, sits beside archive markers, or is in a
    /// bleh.xyz source checkout; [`BoundaryError::BadPath`] if it is a
    /// directory or its parent does not exist.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, BoundaryError> {
        let path = path.as_ref();
        let bad = |reason: String| BoundaryError::BadPath {
            path: path.to_path_buf(),
            reason,
        };
        if path.is_dir() {
            return Err(bad("is a directory".into()));
        }
        let name = path
            .file_name()
            .ok_or_else(|| bad("has no file name".into()))?;
        let parent = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let parent = parent
            .canonicalize()
            .map_err(|e| bad(format!("parent directory: {e}")))?;
        // A symlink at the final component could point anywhere: resolve it.
        let resolved = match parent.join(name).canonicalize() {
            Ok(p) => p,
            Err(_) => parent.join(name),
        };
        let start = resolved.parent().unwrap_or(&parent).to_path_buf();
        if let Some(reason) = archive_marker(&resolved, &start) {
            return Err(BoundaryError::ArchivePath {
                path: path.to_path_buf(),
                reason,
            });
        }
        Ok(Self(path.to_path_buf()))
    }

    /// The path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// Returns why `file` (whose resolved parent is `dir`) is inside a bleh.xyz
/// archive or source checkout, if it is.
fn archive_marker(file: &Path, dir: &Path) -> Option<String> {
    if let Some(n) = file.file_name().and_then(|n| n.to_str())
        && is_manifest_name(n)
    {
        return Some(format!("{n} is a bleh.xyz archive manifest name"));
    }
    for anc in dir.ancestors() {
        if let Some(n) = anc.file_name().and_then(|n| n.to_str()) {
            if ARCHIVE_DIR_MARKERS.contains(&n) {
                return Some(format!("{} is archive metadata", anc.display()));
            }
            if ARCHIVE_PARTITION_PREFIXES.iter().any(|p| n.starts_with(p)) {
                return Some(format!(
                    "{} is an archive partition directory",
                    anc.display()
                ));
            }
        }
        for m in ARCHIVE_DIR_MARKERS {
            if anc.join(m).is_dir() {
                return Some(format!("{} holds {m}/ (a bleh.xyz archive)", anc.display()));
            }
        }
        let manifest = anc.join("manifest.json");
        if manifest.is_file() && is_bleh_manifest(&manifest) {
            return Some(format!("{} holds a bleh.xyz manifest", anc.display()));
        }
        if anc.join("docs/admission-policy.md").is_file() && anc.join("crates/mesh-types").is_dir()
        {
            return Some(format!("{} is a bleh.xyz source checkout", anc.display()));
        }
    }
    None
}

fn is_manifest_name(n: &str) -> bool {
    matches!(
        n,
        "manifest.json" | "manifest.anchors.json" | "manifest.json.ots"
    )
}

/// A `manifest.json` is a bleh one if it names the bleh manifest format.
fn is_bleh_manifest(p: &Path) -> bool {
    use std::io::Read;
    let mut head = Vec::with_capacity(4096);
    std::fs::File::open(p)
        .and_then(|f| f.take(1 << 20).read_to_end(&mut head))
        .is_ok_and(|_| String::from_utf8_lossy(&head).contains("bleh-manifest-"))
}

/// A loopback socket address for the local websocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackAddr(SocketAddr);

impl LoopbackAddr {
    /// Parses a literal `IP:PORT` and accepts only loopback IPs
    /// (`127.0.0.0/8`, `::1`). Host names are refused: they resolve
    /// outside this program's control.
    ///
    /// # Errors
    ///
    /// [`BoundaryError::NotLoopback`] for anything else.
    pub fn parse(s: &str) -> Result<Self, BoundaryError> {
        let not = || BoundaryError::NotLoopback { addr: s.into() };
        let addr: SocketAddr = s.trim().parse().map_err(|_| not())?;
        if addr.ip().is_loopback() {
            Ok(Self(addr))
        } else {
            Err(not())
        }
    }

    /// The address.
    pub const fn addr(&self) -> SocketAddr {
        self.0
    }
}

impl std::str::FromStr for LoopbackAddr {
    type Err = BoundaryError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for LoopbackAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn plain_paths_are_accepted() {
        let d = tmp();
        let p = d.path().join("merged.jsonl");
        assert_eq!(LocalPath::new(&p).unwrap().as_path(), p);
        std::fs::write(&p, b"x").unwrap();
        assert!(LocalPath::new(&p).is_ok(), "existing files too");
    }

    #[test]
    fn archive_roots_and_their_subdirectories_are_refused() {
        let d = tmp();
        let root = d.path().join("hyperliquid-normalized");
        std::fs::create_dir_all(root.join("_manifests/2026-01-01")).unwrap();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        for p in [
            root.join("out.jsonl"),
            root.join("sub/deeper/out.jsonl"),
            root.join("_manifests/2026-01-01/out.jsonl"),
        ] {
            let e = LocalPath::new(&p).unwrap_err();
            assert!(matches!(e, BoundaryError::ArchivePath { .. }), "{p:?}: {e}");
        }
        // Beside the archive, not inside it: fine.
        assert!(LocalPath::new(d.path().join("out.jsonl")).is_ok());
    }

    #[test]
    fn iceberg_and_partition_directories_are_refused() {
        let d = tmp();
        let ice = d.path().join("a/_iceberg/trade");
        std::fs::create_dir_all(&ice).unwrap();
        assert!(LocalPath::new(ice.join("x.jsonl")).is_err());
        let part = d.path().join("b/kind=trade/venue=v");
        std::fs::create_dir_all(&part).unwrap();
        let e = LocalPath::new(part.join("x.jsonl")).unwrap_err();
        assert!(e.to_string().contains("partition"), "{e}");
    }

    #[test]
    fn signed_manifests_mark_an_archive_but_other_manifests_do_not() {
        let d = tmp();
        let a = d.path().join("arch");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::write(
            a.join("manifest.json"),
            br#"{"manifest":{"format":"bleh-manifest-v0"}}"#,
        )
        .unwrap();
        assert!(LocalPath::new(a.join("x.jsonl")).is_err());
        let w = d.path().join("webapp");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::write(w.join("manifest.json"), br#"{"name":"an app"}"#).unwrap();
        assert!(LocalPath::new(w.join("x.jsonl")).is_ok());
        // The manifest file names themselves are refused anywhere.
        assert!(LocalPath::new(w.join("manifest.json")).is_err());
    }

    #[test]
    fn source_checkouts_are_refused() {
        let d = tmp();
        std::fs::create_dir_all(d.path().join("docs")).unwrap();
        std::fs::create_dir_all(d.path().join("crates/mesh-types")).unwrap();
        std::fs::create_dir_all(d.path().join("fixtures/synthetic")).unwrap();
        std::fs::write(d.path().join("docs/admission-policy.md"), b"#").unwrap();
        let e = LocalPath::new(d.path().join("fixtures/synthetic/x.csv")).unwrap_err();
        assert!(e.to_string().contains("source checkout"), "{e}");
        // This crate's own checkout is one too.
        let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("merged.jsonl");
        assert!(LocalPath::new(here).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_into_an_archive_are_refused() {
        let d = tmp();
        let root = d.path().join("arch");
        std::fs::create_dir_all(root.join("_manifests")).unwrap();
        let link = d.path().join("innocent");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(LocalPath::new(link.join("x.jsonl")).is_err());
        let file_link = d.path().join("out.jsonl");
        std::os::unix::fs::symlink(root.join("y.jsonl"), &file_link).unwrap();
        std::fs::write(root.join("y.jsonl"), b"").unwrap();
        assert!(LocalPath::new(&file_link).is_err());
    }

    #[test]
    fn directories_and_missing_parents_are_bad_paths() {
        let d = tmp();
        assert!(matches!(
            LocalPath::new(d.path()),
            Err(BoundaryError::BadPath { .. })
        ));
        assert!(matches!(
            LocalPath::new(d.path().join("nope/x.jsonl")),
            Err(BoundaryError::BadPath { .. })
        ));
    }

    #[test]
    fn only_loopback_bind_addresses_are_accepted() {
        for ok in [
            "127.0.0.1:9100",
            "127.0.0.1:0",
            "[::1]:9100",
            "127.1.2.3:80",
        ] {
            assert!(LoopbackAddr::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "0.0.0.0:9100",
            "[::]:9100",
            "192.168.1.10:9100",
            "10.0.0.1:9100",
            "localhost:9100",
            "example.com:9100",
            "[::ffff:127.0.0.1]:9100",
            "127.0.0.1",
            "",
        ] {
            let e = LoopbackAddr::parse(bad).unwrap_err();
            assert!(matches!(e, BoundaryError::NotLoopback { .. }), "{bad}");
        }
    }
}
