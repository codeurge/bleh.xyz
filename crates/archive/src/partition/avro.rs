//! A minimal, deterministic Avro object-container writer, enough for
//! Iceberg manifests and manifest lists. Written by hand so the sync marker
//! and metadata order are fixed (Avro libraries pick random sync markers).
//!
//! Encoding (Avro 1.11 specification): `int`/`long` as zig-zag varints,
//! `string`/`bytes` as a length then the bytes, unions as the branch index
//! then the value, records as their fields in order, arrays and maps as one
//! block (count, items) then a zero count. The container is the magic
//! `Obj\x01`, a metadata map, a 16-byte sync marker, then data blocks of
//! (object count, byte length, objects, sync marker). Codec `null`.

use sha2::{Digest, Sha256};

/// An Avro datum.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// `null`.
    Null,
    /// `boolean`.
    Bool(bool),
    /// `int`.
    Int(i32),
    /// `long`.
    Long(i64),
    /// `string`.
    Str(String),
    /// `bytes`.
    Bytes(Vec<u8>),
    /// A union branch: index and value.
    Union(i64, Box<Value>),
    /// A record's fields, in schema order.
    Record(Vec<Value>),
    /// An array.
    Array(Vec<Value>),
}

impl Value {
    /// `["null", T]` with a value.
    pub(crate) fn some(v: Value) -> Self {
        Self::Union(1, Box::new(v))
    }

    /// `["null", T]` without a value.
    pub(crate) fn none() -> Self {
        Self::Union(0, Box::new(Value::Null))
    }
}

fn put_long(out: &mut Vec<u8>, v: i64) {
    let mut z = ((v << 1) ^ (v >> 63)) as u64;
    while z >= 0x80 {
        out.push((z as u8 & 0x7f) | 0x80);
        z >>= 7;
    }
    out.push(z as u8);
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_long(out, b.len() as i64);
    out.extend_from_slice(b);
}

/// Appends the binary encoding of `v`.
pub(crate) fn encode(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => {}
        Value::Bool(b) => out.push(u8::from(*b)),
        Value::Int(i) => put_long(out, i64::from(*i)),
        Value::Long(l) => put_long(out, *l),
        Value::Str(s) => put_bytes(out, s.as_bytes()),
        Value::Bytes(b) => put_bytes(out, b),
        Value::Union(i, inner) => {
            put_long(out, *i);
            encode(inner, out);
        }
        Value::Record(fields) => {
            for f in fields {
                encode(f, out);
            }
        }
        Value::Array(items) => {
            if !items.is_empty() {
                put_long(out, items.len() as i64);
                for i in items {
                    encode(i, out);
                }
            }
            put_long(out, 0);
        }
    }
}

/// Builds an object container file. `metadata` is written in the given
/// order after `avro.schema` and `avro.codec`. The sync marker is derived
/// from `sync_seed`, so equal inputs give equal bytes.
pub(crate) fn container(
    schema: &str,
    metadata: &[(&str, String)],
    records: &[Value],
    sync_seed: &[u8],
) -> Vec<u8> {
    let digest = Sha256::digest(sync_seed);
    let mut sync = [0u8; 16];
    sync.copy_from_slice(&digest[..16]);

    let mut out = b"Obj\x01".to_vec();
    let mut meta: Vec<(&str, &[u8])> =
        vec![("avro.schema", schema.as_bytes()), ("avro.codec", b"null")];
    meta.extend(metadata.iter().map(|(k, v)| (*k, v.as_bytes())));
    put_long(&mut out, meta.len() as i64);
    for (k, v) in meta {
        put_bytes(&mut out, k.as_bytes());
        put_bytes(&mut out, v);
    }
    put_long(&mut out, 0);
    out.extend_from_slice(&sync);
    if !records.is_empty() {
        let mut block = Vec::new();
        for r in records {
            encode(r, &mut block);
        }
        put_long(&mut out, records.len() as i64);
        put_long(&mut out, block.len() as i64);
        out.extend_from_slice(&block);
        out.extend_from_slice(&sync);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_and_primitives() {
        // Avro specification examples: 0 -> 00, -1 -> 01, 1 -> 02, -2 -> 03,
        // 2 -> 04, -64 -> 7f, 64 -> 80 01.
        for (v, want) in [
            (0i64, vec![0x00]),
            (-1, vec![0x01]),
            (1, vec![0x02]),
            (-2, vec![0x03]),
            (2, vec![0x04]),
            (-64, vec![0x7f]),
            (64, vec![0x80, 0x01]),
        ] {
            let mut out = Vec::new();
            put_long(&mut out, v);
            assert_eq!(out, want, "{v}");
        }
        // The specification's "foo" string example: 06 66 6f 6f.
        let mut out = Vec::new();
        encode(&Value::Str("foo".into()), &mut out);
        assert_eq!(out, vec![0x06, b'f', b'o', b'o']);
        // Array {3, 27} of longs: 04 06 36 00.
        let mut out = Vec::new();
        encode(
            &Value::Array(vec![Value::Long(3), Value::Long(27)]),
            &mut out,
        );
        assert_eq!(out, vec![0x04, 0x06, 0x36, 0x00]);
        // Union ["null","string"] with "a": 02 02 61.
        let mut out = Vec::new();
        encode(&Value::some(Value::Str("a".into())), &mut out);
        assert_eq!(out, vec![0x02, 0x02, b'a']);
    }

    #[test]
    fn container_layout() {
        let schema = r#"{"type":"record","name":"r","fields":[{"name":"x","type":"long"}]}"#;
        let a = container(
            schema,
            &[("k", "v".into())],
            &[Value::Record(vec![Value::Long(1)])],
            b"seed",
        );
        let b = container(
            schema,
            &[("k", "v".into())],
            &[Value::Record(vec![Value::Long(1)])],
            b"seed",
        );
        assert_eq!(a, b);
        assert!(a.starts_with(b"Obj\x01"));
        let sync = &Sha256::digest(b"seed")[..16];
        assert!(a.ends_with(sync));
        // Block: count 1 (02), size 1 (02), datum 1 (02), then sync.
        let n = a.len();
        assert_eq!(&a[n - 19..n - 16], &[0x02, 0x02, 0x02]);
    }
}
