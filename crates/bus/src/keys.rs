//! Block-signing keys: loading and rotation.
//!
//! A key is a secp256k1 private scalar written as 64 hexadecimal digits
//! (optionally prefixed with `0x`, surrounding whitespace ignored), read
//! from a file or an environment variable. Key material never appears in
//! `Debug` output, errors or logs: only the key ID and its Ethereum address
//! do. Buffers that held key text are zeroed after parsing.
//!
//! A [`KeyRing`] holds several keys by ID with one active key. Rotation is
//! `add` (or `load`) the new key, publish its address, then
//! [`KeyRing::rotate`] to it; the old key stays listed as a signer until
//! [`KeyRing::retire`] removes it, so verifiers accept both during the
//! overlap.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use mesh_types::{EthAddress, SigningKey};
use zeroize::Zeroizing;

/// Error from loading or managing keys. Never contains key material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The key file could not be read.
    #[error("cannot read key file {path}: {reason}")]
    Io {
        /// File path.
        path: PathBuf,
        /// I/O error kind.
        reason: String,
    },
    /// The environment variable is unset or not Unicode.
    #[error("environment variable {0} is not set")]
    EnvMissing(String),
    /// The text is not 64 hex digits.
    #[error("key text must be 64 hexadecimal digits, optionally prefixed with 0x")]
    Format,
    /// The scalar is zero or not below the curve order.
    #[error("not a valid secp256k1 private key")]
    Invalid,
    /// A key source specification could not be parsed.
    #[error("bad key specification {0:?}: expected ID=file:PATH or ID=env:VAR")]
    Spec(String),
    /// Two keys share an ID.
    #[error("duplicate key id {0}")]
    DuplicateId(String),
    /// No key has this ID.
    #[error("unknown key id {0}")]
    UnknownId(String),
    /// The active key cannot be retired.
    #[error("key {0} is active; rotate to another key first")]
    RetireActive(String),
    /// The key ring is empty.
    #[error("no signing keys")]
    Empty,
}

/// Where a key comes from.
#[derive(Clone, PartialEq, Eq)]
pub enum KeySource {
    /// A file containing the hex key.
    File(PathBuf),
    /// An environment variable containing the hex key.
    Env(String),
}

impl fmt::Debug for KeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(p) => write!(f, "file:{}", p.display()),
            Self::Env(v) => write!(f, "env:{v}"),
        }
    }
}

impl FromStr for KeySource {
    type Err = KeyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(p) = s.strip_prefix("file:") {
            if p.is_empty() {
                return Err(KeyError::Spec(s.to_owned()));
            }
            Ok(Self::File(PathBuf::from(p)))
        } else if let Some(v) = s.strip_prefix("env:") {
            if v.is_empty() {
                return Err(KeyError::Spec(s.to_owned()));
            }
            Ok(Self::Env(v.to_owned()))
        } else {
            Err(KeyError::Spec(s.to_owned()))
        }
    }
}

/// A key ID and its source: `ID=file:PATH` or `ID=env:VAR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    /// Key ID (operator-chosen label, for example `2026-11`).
    pub id: String,
    /// Source.
    pub source: KeySource,
}

impl FromStr for KeySpec {
    type Err = KeyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (id, src) = s
            .split_once('=')
            .ok_or_else(|| KeyError::Spec(s.to_owned()))?;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(KeyError::Spec(s.to_owned()));
        }
        Ok(Self {
            id: id.to_owned(),
            source: src.parse()?,
        })
    }
}

/// Parses hex key text. The input is not retained.
///
/// # Errors
///
/// [`KeyError::Format`] or [`KeyError::Invalid`].
pub fn parse_key(text: &str) -> Result<SigningKey, KeyError> {
    let t = text.trim();
    let t = t.strip_prefix("0x").unwrap_or(t);
    if t.len() != 64 {
        return Err(KeyError::Format);
    }
    let mut bytes = Zeroizing::new([0u8; 32]);
    for (i, pair) in t.as_bytes().chunks(2).enumerate() {
        let hi = hex_val(pair[0]).ok_or(KeyError::Format)?;
        let lo = hex_val(pair[1]).ok_or(KeyError::Format)?;
        bytes[i] = (hi << 4) | lo;
    }
    SigningKey::from_bytes(&bytes).map_err(|_| KeyError::Invalid)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Loads a key from its source.
///
/// # Errors
///
/// I/O, missing variable, or format errors.
pub fn load_key(source: &KeySource) -> Result<SigningKey, KeyError> {
    match source {
        KeySource::File(path) => {
            warn_if_readable_by_others(path);
            let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|e| KeyError::Io {
                path: path.clone(),
                reason: e.kind().to_string(),
            })?);
            parse_key(&text)
        }
        KeySource::Env(var) => {
            let text =
                Zeroizing::new(std::env::var(var).map_err(|_| KeyError::EnvMissing(var.clone()))?);
            parse_key(&text)
        }
    }
}

#[cfg(unix)]
fn warn_if_readable_by_others(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().mode() & 0o077 != 0
    {
        tracing::warn!(
            path = %path.display(),
            "signing key file is accessible by group or others; restrict it to the owner (chmod 600)"
        );
    }
}

#[cfg(not(unix))]
fn warn_if_readable_by_others(_path: &Path) {}

/// Public description of one key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeyInfo {
    /// Key ID.
    pub id: String,
    /// Ethereum address.
    pub address: EthAddress,
    /// True for the key that signs new commitments.
    pub active: bool,
}

/// Keys by ID with one active key.
#[derive(Clone, Default)]
pub struct KeyRing {
    keys: Vec<(String, SigningKey)>,
    active: usize,
}

impl fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.info()).finish()
    }
}

impl KeyRing {
    /// An empty ring.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A ring with one key, active.
    #[must_use]
    pub fn single(id: impl Into<String>, key: SigningKey) -> Self {
        Self {
            keys: vec![(id.into(), key)],
            active: 0,
        }
    }

    /// Loads every spec; `active` names the signing key (default: the last
    /// spec).
    ///
    /// # Errors
    ///
    /// Any load error, a duplicate ID, an unknown active ID, or no specs.
    pub fn load(specs: &[KeySpec], active: Option<&str>) -> Result<Self, KeyError> {
        let mut ring = Self::new();
        for spec in specs {
            ring.add(spec.id.clone(), load_key(&spec.source)?)?;
        }
        if ring.keys.is_empty() {
            return Err(KeyError::Empty);
        }
        match active {
            Some(id) => ring.rotate(id)?,
            None => ring.active = ring.keys.len() - 1,
        }
        Ok(ring)
    }

    /// Adds a key (inactive unless it is the first).
    ///
    /// # Errors
    ///
    /// [`KeyError::DuplicateId`].
    pub fn add(&mut self, id: impl Into<String>, key: SigningKey) -> Result<(), KeyError> {
        let id = id.into();
        if self.keys.iter().any(|(k, _)| *k == id) {
            return Err(KeyError::DuplicateId(id));
        }
        self.keys.push((id, key));
        Ok(())
    }

    /// Makes `id` the signing key.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownId`].
    pub fn rotate(&mut self, id: &str) -> Result<(), KeyError> {
        let i = self
            .keys
            .iter()
            .position(|(k, _)| k == id)
            .ok_or_else(|| KeyError::UnknownId(id.to_owned()))?;
        self.active = i;
        Ok(())
    }

    /// Removes an inactive key.
    ///
    /// # Errors
    ///
    /// [`KeyError::UnknownId`] or [`KeyError::RetireActive`].
    pub fn retire(&mut self, id: &str) -> Result<(), KeyError> {
        let i = self
            .keys
            .iter()
            .position(|(k, _)| k == id)
            .ok_or_else(|| KeyError::UnknownId(id.to_owned()))?;
        if i == self.active {
            return Err(KeyError::RetireActive(id.to_owned()));
        }
        self.keys.remove(i);
        if i < self.active {
            self.active -= 1;
        }
        Ok(())
    }

    /// The active key and its ID.
    ///
    /// # Errors
    ///
    /// [`KeyError::Empty`].
    pub fn active(&self) -> Result<(&str, &SigningKey), KeyError> {
        self.keys
            .get(self.active)
            .map(|(id, k)| (id.as_str(), k))
            .ok_or(KeyError::Empty)
    }

    /// Addresses of every key in the ring, active first.
    #[must_use]
    pub fn signers(&self) -> Vec<EthAddress> {
        let mut out: Vec<EthAddress> = Vec::with_capacity(self.keys.len());
        if let Some((_, k)) = self.keys.get(self.active) {
            out.push(k.address());
        }
        for (i, (_, k)) in self.keys.iter().enumerate() {
            if i != self.active {
                out.push(k.address());
            }
        }
        out
    }

    /// Public description of every key.
    #[must_use]
    pub fn info(&self) -> Vec<KeyInfo> {
        self.keys
            .iter()
            .enumerate()
            .map(|(i, (id, k))| KeyInfo {
                id: id.clone(),
                address: k.address(),
                active: i == self.active,
            })
            .collect()
    }

    /// Number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True if the ring has no keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "0101010101010101010101010101010101010101010101010101010101010101";
    const KEY_B: &str = "0202020202020202020202020202020202020202020202020202020202020202";

    #[test]
    fn parses_hex_with_and_without_prefix() {
        let a = parse_key(KEY_A).unwrap();
        let b = parse_key(&format!("  0x{}\n", KEY_A.to_uppercase())).unwrap();
        assert_eq!(a.address(), b.address());
        assert_eq!(parse_key("abc").err(), Some(KeyError::Format));
        assert_eq!(parse_key(&"zz".repeat(32)).err(), Some(KeyError::Format));
        assert_eq!(parse_key(&"00".repeat(32)).err(), Some(KeyError::Invalid));
    }

    #[test]
    fn debug_and_errors_never_show_key_material() {
        let ring = KeyRing::single("k1", parse_key(KEY_A).unwrap());
        let dbg = format!("{ring:?}");
        assert!(!dbg.contains(KEY_A) && dbg.contains("k1"));
        let e = parse_key(&format!("{KEY_A}00")).unwrap_err().to_string();
        assert!(!e.contains(KEY_A));
    }

    #[test]
    fn specs_parse() {
        let s: KeySpec = "2026-11=file:/run/secrets/k".parse().unwrap();
        assert_eq!(s.id, "2026-11");
        assert_eq!(s.source, KeySource::File("/run/secrets/k".into()));
        let s: KeySpec = "k2=env:BLEH_KEY".parse().unwrap();
        assert_eq!(s.source, KeySource::Env("BLEH_KEY".into()));
        for bad in ["nokey", "=file:/x", "a b=env:X", "k=env:", "k=http://x"] {
            assert!(bad.parse::<KeySpec>().is_err(), "{bad}");
        }
    }

    #[test]
    fn loads_from_file_and_reports_missing_env() {
        let dir = std::env::temp_dir().join(format!("bleh-bus-keytest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("key.hex");
        std::fs::write(&path, format!("{KEY_B}\n")).unwrap();
        let ring = KeyRing::load(
            &[KeySpec {
                id: "b".into(),
                source: KeySource::File(path.clone()),
            }],
            None,
        )
        .unwrap();
        assert_eq!(ring.active().unwrap().0, "b");
        std::fs::remove_dir_all(&dir).unwrap();
        let missing = load_key(&KeySource::Env("BLEH_TEST_SURELY_UNSET_VAR".into()));
        assert!(matches!(missing, Err(KeyError::EnvMissing(_))));
        let missing = load_key(&KeySource::File(path));
        assert!(matches!(missing, Err(KeyError::Io { .. })));
    }

    #[test]
    fn rotation_and_retirement() {
        let a = parse_key(KEY_A).unwrap();
        let b = parse_key(KEY_B).unwrap();
        let mut ring = KeyRing::single("a", a.clone());
        ring.add("b", b.clone()).unwrap();
        assert_eq!(
            ring.add("b", b.clone()),
            Err(KeyError::DuplicateId("b".into()))
        );
        assert_eq!(ring.active().unwrap().0, "a");
        assert_eq!(ring.signers(), vec![a.address(), b.address()]);
        ring.rotate("b").unwrap();
        assert_eq!(ring.signers(), vec![b.address(), a.address()]);
        assert_eq!(ring.retire("b"), Err(KeyError::RetireActive("b".into())));
        ring.retire("a").unwrap();
        assert_eq!(ring.active().unwrap().0, "b");
        assert_eq!(ring.len(), 1);
        assert!(ring.rotate("a").is_err());
        let info = ring.info();
        assert!(info[0].active && info[0].address == b.address());
        assert_eq!(KeyRing::load(&[], None).err(), Some(KeyError::Empty));
    }
}
