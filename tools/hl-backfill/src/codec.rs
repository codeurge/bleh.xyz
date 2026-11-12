//! Compression: LZ4 frames in (Hyperliquid's archive format), zstd out
//! (the filtered raw archive format).

use std::io::Read;

/// Errors from decoding or encoding.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The input is not a valid LZ4 frame stream.
    #[error("lz4: {0}")]
    Lz4(std::io::Error),
    /// Decompressed bytes are not UTF-8.
    #[error("decompressed data is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    /// zstd failed.
    #[error("zstd: {0}")]
    Zstd(std::io::Error),
    /// The input exceeds the decompression limit.
    #[error("decompressed size exceeds the limit of {0} bytes")]
    TooLarge(u64),
}

/// Default limit on decompressed size (8 GiB): far above any documented
/// hourly file, low enough to stop a decompression bomb.
pub const DEFAULT_MAX_DECOMPRESSED: u64 = 8 * 1024 * 1024 * 1024;

/// Decompresses LZ4 frame data (possibly several concatenated frames, as
/// the `lz4` tool can write) into text, refusing more than `limit` bytes.
///
/// # Errors
///
/// Fails on invalid frames, non-UTF-8 output, or output above `limit`.
pub fn lz4_to_string(data: &[u8], limit: u64) -> Result<String, CodecError> {
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(data);
    while (cursor.position() as usize) < data.len() {
        let remaining = limit.saturating_add(1).saturating_sub(out.len() as u64);
        let before = cursor.position();
        lz4_flex::frame::FrameDecoder::new(&mut cursor)
            .take(remaining)
            .read_to_end(&mut out)
            .map_err(CodecError::Lz4)?;
        if out.len() as u64 > limit {
            return Err(CodecError::TooLarge(limit));
        }
        if cursor.position() == before {
            break;
        }
    }
    Ok(String::from_utf8(out)?)
}

/// The zstd settings of the filtered raw archive
/// (`docs/specs/archive-layout-v0.md` § 2): level 19, one frame, no
/// dictionary, content size and checksum flags on.
///
/// # Errors
///
/// Fails if zstd fails.
pub fn zstd_archive(data: &[u8]) -> Result<Vec<u8>, CodecError> {
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 19).map_err(CodecError::Zstd)?;
    enc.include_checksum(true).map_err(CodecError::Zstd)?;
    enc.include_contentsize(true).map_err(CodecError::Zstd)?;
    enc.set_pledged_src_size(Some(data.len() as u64))
        .map_err(CodecError::Zstd)?;
    std::io::Write::write_all(&mut enc, data).map_err(CodecError::Zstd)?;
    enc.finish().map_err(CodecError::Zstd)
}

/// Decompresses zstd data (tests and the compare tool).
///
/// # Errors
///
/// Fails on invalid input.
pub fn unzstd(data: &[u8]) -> Result<Vec<u8>, CodecError> {
    zstd::stream::decode_all(data).map_err(CodecError::Zstd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::lz4;

    #[test]
    fn lz4_round_trip_and_concatenated_frames() {
        let mut data = lz4(b"line one\n");
        data.extend(lz4(b"line two\n"));
        assert_eq!(lz4_to_string(&data, 1024).unwrap(), "line one\nline two\n");
    }

    #[test]
    fn lz4_limit_and_garbage() {
        let data = lz4(&[b'a'; 100]);
        assert!(matches!(
            lz4_to_string(&data, 99),
            Err(CodecError::TooLarge(99))
        ));
        assert!(lz4_to_string(&data, 100).is_ok());
        assert!(lz4_to_string(b"not lz4 at all", 100).is_err());
    }

    #[test]
    fn zstd_round_trip_is_deterministic() {
        let a = zstd_archive(b"{\"a\":1}\n").unwrap();
        let b = zstd_archive(b"{\"a\":1}\n").unwrap();
        assert_eq!(a, b);
        assert_eq!(unzstd(&a).unwrap(), b"{\"a\":1}\n");
    }
}
