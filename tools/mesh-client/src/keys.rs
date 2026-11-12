//! Signing-key files: read a secp256k1 secret and derive its address.
//!
//! A key file holds the 32-byte secret as 64 hex characters, optionally
//! prefixed with `0x` and followed by whitespace (a trailing newline). The
//! secret never leaves this module: callers get the [`SigningKey`] or its
//! [`EthAddress`], and errors never quote the file's contents.

use std::path::{Path, PathBuf};

use mesh_types::{EthAddress, SigningKey};
use zeroize::Zeroizing;

/// A key file that cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum KeyFileError {
    /// The file cannot be read.
    #[error("cannot read key file {path}: {source}")]
    Read {
        /// Path.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// Not 64 hex characters.
    #[error(
        "key file {0} must hold 64 hex characters (a 32-byte secret), optionally prefixed with 0x"
    )]
    Format(PathBuf),
    /// Not a valid secp256k1 secret (zero, or not below the curve order).
    #[error("key file {0} does not hold a valid secp256k1 secret key")]
    Invalid(PathBuf),
}

/// A key read from a file, with a warning when the file's permissions are
/// too open.
#[derive(Debug)]
pub struct KeyFile {
    /// The key.
    pub key: SigningKey,
    /// Set when the file is readable or writable by group or others.
    pub permission_warning: Option<String>,
}

impl KeyFile {
    /// The key's Ethereum address.
    #[must_use]
    pub fn address(&self) -> EthAddress {
        self.key.address()
    }
}

/// Parses a secret as 64 hex characters, optional `0x`, trailing
/// whitespace allowed. `None` on a format error, `Some(Err(()))` for an
/// invalid secret.
fn parse_secret(text: &str) -> Option<Result<SigningKey, ()>> {
    let t = text.trim_end();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    if t.len() != 64 {
        return None;
    }
    let mut bytes = Zeroizing::new([0u8; 32]);
    for (i, pair) in t.as_bytes().chunks(2).enumerate() {
        let hi = hex_val(*pair.first()?)?;
        let lo = hex_val(*pair.get(1)?)?;
        *bytes.get_mut(i)? = (hi << 4) | lo;
    }
    Some(SigningKey::from_bytes(&bytes).map_err(|_| ()))
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Reads a key file.
///
/// # Errors
///
/// [`KeyFileError`]; messages never include the file's contents.
pub fn read_key_file(path: &Path) -> Result<KeyFile, KeyFileError> {
    let raw = Zeroizing::new(std::fs::read(path).map_err(|source| KeyFileError::Read {
        path: path.to_owned(),
        source,
    })?);
    let text = std::str::from_utf8(&raw).map_err(|_| KeyFileError::Format(path.to_owned()))?;
    let key = match parse_secret(text) {
        None => return Err(KeyFileError::Format(path.to_owned())),
        Some(Err(())) => return Err(KeyFileError::Invalid(path.to_owned())),
        Some(Ok(k)) => k,
    };
    Ok(KeyFile {
        key,
        permission_warning: permission_warning(path),
    })
}

#[cfg(unix)]
fn permission_warning(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| {
        format!(
            "key file {} is accessible by group or others (mode {mode:03o}); restrict it with chmod 600",
            path.display()
        )
    })
}

#[cfg(not(unix))]
fn permission_warning(_path: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
    const ADDRESS: &str = "0x2c7536e3605d9c16a7a3d7b1898e529396a65c23";

    fn temp_file(name: &str, contents: &str, mode: u32) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mesh-client-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = mode;
        p
    }

    #[test]
    fn reads_keys_in_every_accepted_form() {
        for (name, text) in [
            ("plain", KEY.to_owned()),
            ("prefixed", format!("0x{KEY}\n")),
            ("upper", format!("0X{}  \r\n", KEY.to_uppercase())),
        ] {
            let p = temp_file(name, &text, 0o600);
            let k = read_key_file(&p).unwrap();
            assert_eq!(k.address().to_string(), ADDRESS, "{name}");
            assert!(k.permission_warning.is_none(), "{name}");
        }
    }

    #[test]
    fn rejects_bad_files_without_quoting_them() {
        for (name, text) in [
            ("short", KEY[..62].to_owned()),
            ("long", format!("{KEY}00")),
            ("nonhex", format!("{}zz", &KEY[..62])),
            ("leading-space", format!(" {KEY}")),
            ("empty", String::new()),
        ] {
            let p = temp_file(name, &text, 0o600);
            let e = read_key_file(&p).unwrap_err();
            assert!(matches!(e, KeyFileError::Format(_)), "{name}: {e}");
            assert!(!e.to_string().contains(&KEY[..16]), "{name}: {e}");
        }
        let p = temp_file("zero", &"0".repeat(64), 0o600);
        assert!(matches!(
            read_key_file(&p).unwrap_err(),
            KeyFileError::Invalid(_)
        ));
        let missing = std::env::temp_dir().join("mesh-client-keys-missing/none");
        assert!(matches!(
            read_key_file(&missing).unwrap_err(),
            KeyFileError::Read { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn warns_about_open_permissions() {
        let p = temp_file("open", KEY, 0o644);
        let k = read_key_file(&p).unwrap();
        let w = k.permission_warning.unwrap();
        assert!(w.contains("644"), "{w}");
        assert!(!w.contains(&KEY[..16]));
    }
}
