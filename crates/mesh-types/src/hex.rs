//! Minimal lowercase hex encoding, to avoid a dependency for 30 lines.

/// Encodes bytes as lowercase hex.
pub(crate) fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

/// Decodes lowercase hex into exactly `N` bytes. Uppercase is rejected so
/// that every value has one text form.
pub(crate) fn decode_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != N * 2 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (i, pair) in b.as_chunks::<2>().0.iter().enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let bytes = [0x00, 0x7f, 0x80, 0xff, 0x12];
        let s = encode(&bytes);
        assert_eq!(s, "007f80ff12");
        assert_eq!(decode_array::<5>(&s), Some(bytes));
        assert_eq!(decode_array::<5>("007F80FF12"), None);
        assert_eq!(decode_array::<4>(&s), None);
        assert_eq!(decode_array::<1>("0g"), None);
    }
}
