//! Canonical encoding and content hashes.
//!
//! # Canonical encoding of a record
//!
//! 1. Serialize the record to JSON with this crate's serde implementation.
//! 2. Remove `prov.ts_publish_ns` (live-only, not reproducible).
//! 3. Write the value with object keys sorted by byte order, no whitespace,
//!    integers in shortest decimal form, and strings escaped minimally:
//!    `\"`, `\\`, `\b`, `\f`, `\n`, `\r`, `\t`, other control characters as
//!    `\u00xx` (lowercase hex), everything else as raw UTF-8.
//!
//! The model has no floating-point fields, so this coincides with RFC 8785
//! (JCS) for every valid record. A float anywhere is an error.
//!
//! # Content hash of a sequence of records
//!
//! ```text
//! content_hash = SHA-256( "bleh-observation-v0\n"
//!                         || for each record: u64_be(len(c)) || c )
//! ```
//!
//! where `c` is the record's canonical encoding. The order of records is
//! part of the hash; archives use [`Record::canonical_cmp`] order
//! (instrument, kind, seq). The hash depends only on record content, never
//! on Parquet or any other container's bytes, so two writers that produce
//! different files from the same records agree on it.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::record::Record;

/// Domain prefix of [`ContentHash`].
pub const CONTENT_HASH_DOMAIN: &[u8] = b"bleh-observation-v0\n";

/// A SHA-256 digest. Text form: 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Digest(pub [u8; 32]);

impl Sha256Digest {
    /// SHA-256 of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(finalize(Sha256::new_with_prefix(bytes)))
    }
}

fn finalize(h: Sha256) -> [u8; 32] {
    let out = h.finalize();
    let mut a = [0u8; 32];
    a.copy_from_slice(&out);
    a
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::hex::encode(&self.0))
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256Digest({self})")
    }
}

/// Error from parsing a hex digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected 64 lowercase hex digits")]
pub struct DigestParseError;

impl FromStr for Sha256Digest {
    type Err = DigestParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        crate::hex::decode_array(s)
            .map(Self)
            .ok_or(DigestParseError)
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Content hash of a sequence of records (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(pub Sha256Digest);

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error from canonical encoding.
#[derive(Debug, thiserror::Error)]
pub enum CanonicalError {
    /// Serialization failed.
    #[error("serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    /// A non-integer number was found; canonical data has no floats.
    #[error("floating-point number in canonical data")]
    Float,
}

/// Writes `value` in canonical form (sorted keys, compact, minimal escapes).
///
/// # Errors
///
/// Fails on a floating-point number.
pub fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.extend_from_slice(i.to_string().as_bytes());
            } else if let Some(u) = n.as_u64() {
                out.extend_from_slice(u.to_string().as_bytes());
            } else {
                return Err(CanonicalError::Float);
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            // Sort explicitly: serde_json's map may preserve insertion order
            // if any crate in the build enables its `preserve_order` feature.
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push(b'{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_string(k, out);
                out.push(b':');
                write_canonical(v, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{0c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if u32::from(c) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", u32::from(c)).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Canonical JSON bytes of any serializable value (sorted keys, compact).
///
/// # Errors
///
/// Fails if serialization fails or produces a float.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalError> {
    let v = serde_json::to_value(value)?;
    let mut out = Vec::new();
    write_canonical(&v, &mut out)?;
    Ok(out)
}

/// Canonical encoding of a record: canonical JSON without
/// `prov.ts_publish_ns`.
///
/// # Errors
///
/// Fails if serialization fails.
pub fn canonical_record(record: &Record) -> Result<Vec<u8>, CanonicalError> {
    let mut v = serde_json::to_value(record)?;
    if let Some(prov) = v.get_mut("prov").and_then(Value::as_object_mut) {
        prov.remove("ts_publish_ns");
    }
    let mut out = Vec::new();
    write_canonical(&v, &mut out)?;
    Ok(out)
}

/// Incremental [`ContentHash`] over a sequence of records.
#[derive(Clone)]
pub struct ContentHasher {
    inner: Sha256,
    count: u64,
}

impl fmt::Debug for ContentHasher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContentHasher")
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentHasher {
    /// Starts a hash.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Sha256::new_with_prefix(CONTENT_HASH_DOMAIN),
            count: 0,
        }
    }

    /// Adds the next record.
    ///
    /// # Errors
    ///
    /// Fails if the record cannot be encoded.
    pub fn update(&mut self, record: &Record) -> Result<(), CanonicalError> {
        let bytes = canonical_record(record)?;
        self.update_canonical(&bytes);
        Ok(())
    }

    /// Adds the next record from its already-computed canonical encoding.
    pub fn update_canonical(&mut self, canonical: &[u8]) {
        self.inner.update((canonical.len() as u64).to_be_bytes());
        self.inner.update(canonical);
        self.count += 1;
    }

    /// Number of records added so far.
    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    /// Finishes the hash.
    #[must_use]
    pub fn finalize(self) -> ContentHash {
        ContentHash(Sha256Digest(finalize(self.inner)))
    }
}

/// [`ContentHash`] of `records`, in the order given.
///
/// # Errors
///
/// Fails if a record cannot be encoded.
pub fn content_hash<'a, I>(records: I) -> Result<ContentHash, CanonicalError>
where
    I: IntoIterator<Item = &'a Record>,
{
    let mut h = ContentHasher::new();
    for r in records {
        h.update(r)?;
    }
    Ok(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnixNanos;
    use crate::record::tests::trade;

    #[test]
    fn canonical_json_sorts_and_escapes() {
        let v = serde_json::json!({"b": 1, "a": [true, null, "x\"\\\n\u{1}é"], "A": -5});
        let mut out = Vec::new();
        write_canonical(&v, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{"A":-5,"a":[true,null,"x\"\\\n\u0001é"],"b":1}"#
        );
        let mut out = Vec::new();
        assert!(matches!(
            write_canonical(&serde_json::json!({"x": 1.5}), &mut out),
            Err(CanonicalError::Float)
        ));
    }

    #[test]
    fn publish_time_is_excluded() {
        let a = Record::from(trade(1, 0));
        let mut b = a.clone();
        b.prov_mut().stamp_publish(UnixNanos(123));
        assert_eq!(canonical_record(&a).unwrap(), canonical_record(&b).unwrap());
        assert_eq!(content_hash([&a]).unwrap(), content_hash([&b]).unwrap());
    }

    #[test]
    fn hash_depends_on_content_and_order() {
        let a = Record::from(trade(1, 0));
        let b = Record::from(trade(1, 1));
        let ab = content_hash([&a, &b]).unwrap();
        let ba = content_hash([&b, &a]).unwrap();
        assert_ne!(ab, ba);
        assert_ne!(content_hash([&a]).unwrap(), content_hash([&b]).unwrap());
        let mut h = ContentHasher::new();
        h.update(&a).unwrap();
        h.update(&b).unwrap();
        assert_eq!(h.count(), 2);
        assert_eq!(h.finalize(), ab);
    }

    #[test]
    fn canonical_trade_bytes_are_stable() {
        // Golden value: changing it is a breaking change to observation-v0.
        let bytes = canonical_record(&Record::from(trade(5, 1))).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            concat!(
                r#"{"aggressor":"buy","kind":"trade","price":"1000.5","#,
                r#""prov":{"admission":"ADM-0001","flags":[],"instrument":"hyperliquid:perp:BTC","#,
                r#""seq":{"block_number":5,"event_index":1},"ts_ingest_ns":"1767225600004000000","#,
                r#""ts_source_ms":1767225600000,"venue":"hyperliquid"},"size":"0.250","trade_id":77}"#
            )
        );
        let empty = ContentHasher::new().finalize();
        assert_eq!(empty.0, Sha256Digest::of(CONTENT_HASH_DOMAIN));
    }

    #[test]
    fn digest_text_form() {
        let d = Sha256Digest::of(b"abc");
        assert_eq!(
            d.to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(d.to_string().parse::<Sha256Digest>().unwrap(), d);
        assert!("BA7816BF".parse::<Sha256Digest>().is_err());
        let json = serde_json::to_string(&ContentHash(d)).unwrap();
        assert_eq!(
            serde_json::from_str::<ContentHash>(&json).unwrap(),
            ContentHash(d)
        );
    }
}
