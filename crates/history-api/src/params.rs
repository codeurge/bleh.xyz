//! Request parameter parsing shared by every surface: times, ranges,
//! instruments, intervals and opaque pagination cursors.

use mesh_types::{BarInterval, InstrumentId, UnixMillis};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult, ErrorCode};

/// Milliseconds per day.
pub const DAY_MS: i64 = 86_400_000;
/// Milliseconds per hour.
pub const HOUR_MS: i64 = 3_600_000;

/// Parses a time: integer milliseconds since the epoch, or RFC 3339
/// (`2026-01-01T00:00:00Z`), or a bare date (`2026-01-01`, UTC midnight).
///
/// # Errors
///
/// `bad_request` on anything else.
pub fn parse_time(name: &str, s: &str) -> ApiResult<UnixMillis> {
    let s = s.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s
            .parse::<i64>()
            .map(UnixMillis)
            .map_err(|_| ApiError::bad_request(format!("{name}: out of range")));
    }
    if s.len() == 10 && s.as_bytes().get(4) == Some(&b'-') {
        return UnixMillis::parse_rfc3339(&format!("{s}T00:00:00Z"))
            .map_err(|e| ApiError::bad_request(format!("{name}: {e}")));
    }
    UnixMillis::parse_rfc3339(s).map_err(|e| {
        ApiError::bad_request(format!(
            "{name}: expected milliseconds since the epoch or RFC 3339 ({e})"
        ))
    })
}

/// A half-open time range `[start, end)` in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    /// Inclusive start.
    pub start_ms: i64,
    /// Exclusive end.
    pub end_ms: i64,
}

impl TimeRange {
    /// Builds and checks a range against a maximum width.
    ///
    /// # Errors
    ///
    /// `bad_request` if empty, `range_too_large` if wider than `max_ms`.
    pub fn new(start: UnixMillis, end: UnixMillis, max_ms: i64) -> ApiResult<Self> {
        if end.0 <= start.0 {
            return Err(ApiError::bad_request("end must be after start"));
        }
        if end.0.saturating_sub(start.0) > max_ms {
            return Err(ApiError::new(
                ErrorCode::RangeTooLarge,
                format!(
                    "range is wider than the maximum of {} hours; split the query",
                    max_ms / HOUR_MS
                ),
            ));
        }
        Ok(Self {
            start_ms: start.0,
            end_ms: end.0,
        })
    }

    /// Parses optional `start`/`end` strings. Missing `end` means
    /// `start + default_ms`; missing `start` means `end - default_ms`;
    /// neither means the `default_ms` before `now`.
    ///
    /// # Errors
    ///
    /// As [`parse_time`] and [`TimeRange::new`].
    pub fn parse(
        start: Option<&str>,
        end: Option<&str>,
        default_ms: i64,
        max_ms: i64,
    ) -> ApiResult<Self> {
        let start = start.map(|s| parse_time("start", s)).transpose()?;
        let end = end.map(|s| parse_time("end", s)).transpose()?;
        let (s, e) = match (start, end) {
            (Some(s), Some(e)) => (s, e),
            (Some(s), None) => (s, UnixMillis(s.0.saturating_add(default_ms))),
            (None, Some(e)) => (UnixMillis(e.0.saturating_sub(default_ms)), e),
            (None, None) => {
                let now = UnixMillis::now();
                (UnixMillis(now.0.saturating_sub(default_ms)), now)
            }
        };
        Self::new(s, e, max_ms)
    }

    /// UTC days (as days since the epoch) that intersect the range.
    #[must_use]
    pub fn days(&self) -> Vec<i64> {
        let first = self.start_ms.div_euclid(DAY_MS);
        let last = (self.end_ms - 1).div_euclid(DAY_MS);
        (first..=last).collect()
    }
}

/// Formats days since the epoch as `YYYY-MM-DD`.
#[must_use]
pub fn day_string(day: i64) -> String {
    let rfc = UnixMillis(day.saturating_mul(DAY_MS)).to_rfc3339();
    rfc.get(..10).unwrap_or_default().to_owned()
}

/// Parses an instrument ID.
///
/// # Errors
///
/// `bad_request` when the ID is malformed.
pub fn parse_instrument(s: &str) -> ApiResult<InstrumentId> {
    InstrumentId::new(s.trim()).map_err(|e| ApiError::bad_request(format!("instrument: {e}")))
}

/// Parses a bar interval: `1s`, `1m`, `5m`, `15m`, `1h`, `1d`.
///
/// Tick, volume and range bars are not part of `observation-v0`, so they
/// are rejected with an explanation rather than silently returning nothing.
///
/// # Errors
///
/// `unsupported_interval`.
pub fn parse_interval(s: &str) -> ApiResult<BarInterval> {
    let s = s.trim();
    if let Some(i) = BarInterval::ALL.iter().find(|i| i.as_str() == s) {
        return Ok(*i);
    }
    let lower = s.to_ascii_lowercase();
    let msg = if lower.contains("tick") || lower.ends_with('t') {
        "tick bars are not part of observation-v0 and are not published yet".to_owned()
    } else if lower.contains("vol") || lower.ends_with('v') {
        "volume bars are not part of observation-v0 and are not published yet".to_owned()
    } else if lower.contains("range") || lower.ends_with('r') {
        "range bars are not part of observation-v0 and are not published yet".to_owned()
    } else {
        format!("interval {s:?} is not one of 1s, 1m, 5m, 15m, 1h, 1d")
    };
    Err(ApiError::new(ErrorCode::UnsupportedInterval, msg))
}

/// The position after the last row of a page, carried in an opaque cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Format version.
    pub v: u8,
    /// Query fingerprint (kind, instrument, range, parameters). A cursor
    /// is valid only for the query that produced it.
    pub q: String,
    /// Sort key of the last row returned.
    pub k: Vec<i64>,
}

impl Cursor {
    /// Encodes as unpadded base64url JSON.
    #[must_use]
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        b64url_encode(&json)
    }

    /// Decodes and checks the fingerprint.
    ///
    /// # Errors
    ///
    /// `invalid_cursor` when malformed or produced by a different query.
    pub fn decode(s: &str, fingerprint: &str, key_len: usize) -> ApiResult<Self> {
        let bad = || ApiError::new(ErrorCode::InvalidCursor, "cursor is malformed");
        let bytes = b64url_decode(s).ok_or_else(bad)?;
        let c: Cursor = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        if c.v != 1 || c.k.len() != key_len {
            return Err(bad());
        }
        if c.q != fingerprint {
            return Err(ApiError::new(
                ErrorCode::InvalidCursor,
                "cursor belongs to a different query; repeat the original parameters",
            ));
        }
        Ok(c)
    }

    /// A cursor for `fingerprint` after `key`.
    #[must_use]
    pub fn after(fingerprint: &str, key: Vec<i64>) -> String {
        Self {
            v: 1,
            q: fingerprint.to_owned(),
            k: key,
        }
        .encode()
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url.
#[must_use]
pub fn b64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for i in 0..chars {
            let idx = ((n >> (18 - 6 * i)) & 0x3f) as usize;
            out.push(char::from(B64[idx]));
        }
    }
    out
}

/// Decodes unpadded base64url; `None` on invalid input.
#[must_use]
pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut vals = Vec::with_capacity(s.len());
    for c in s.bytes() {
        let v = B64.iter().position(|&x| x == c)?;
        vals.push(u32::try_from(v).ok()?);
    }
    if vals.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        let mut n = 0u32;
        for (i, v) in chunk.iter().enumerate() {
            n |= v << (18 - 6 * i);
        }
        let bytes = chunk.len() - 1;
        for i in 0..bytes {
            out.push(((n >> (16 - 8 * i)) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(
            parse_time("t", "1767225600000").unwrap(),
            UnixMillis(1_767_225_600_000)
        );
        assert_eq!(
            parse_time("t", "2026-01-01").unwrap(),
            UnixMillis(1_767_225_600_000)
        );
        assert_eq!(
            parse_time("t", "2026-01-01T00:00:01.5Z").unwrap(),
            UnixMillis(1_767_225_601_500)
        );
        assert!(parse_time("t", "yesterday").is_err());
    }

    #[test]
    fn ranges() {
        let r =
            TimeRange::parse(Some("2026-01-01"), Some("2026-01-03"), DAY_MS, 31 * DAY_MS).unwrap();
        assert_eq!(r.days().len(), 2);
        assert_eq!(day_string(r.days()[1]), "2026-01-02");
        let e = TimeRange::parse(Some("2026-01-01"), Some("2026-03-01"), DAY_MS, 31 * DAY_MS)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::RangeTooLarge);
        assert!(TimeRange::parse(Some("2026-01-02"), Some("2026-01-01"), 1, 10).is_err());
        let d = TimeRange::parse(Some("2026-01-01"), None, HOUR_MS, DAY_MS).unwrap();
        assert_eq!(d.end_ms - d.start_ms, HOUR_MS);
        assert_eq!(d.days().len(), 1);
    }

    #[test]
    fn intervals() {
        assert_eq!(parse_interval("15m").unwrap(), BarInterval::M15);
        let e = parse_interval("100t").unwrap_err();
        assert_eq!(e.code, ErrorCode::UnsupportedInterval);
        assert!(e.message.contains("tick"));
        assert!(parse_interval("2h").is_err());
    }

    #[test]
    fn base64url_round_trips() {
        for n in 0..40 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64url_decode(&b64url_encode(&data)).unwrap(), data);
        }
        assert_eq!(b64url_encode(b"hi?"), "aGk_");
        assert!(b64url_decode("a").is_none());
        assert!(b64url_decode("a=bc").is_none());
    }

    #[test]
    fn cursors_bind_to_their_query() {
        let c = Cursor::after("trade|x|1|2", vec![5, 7]);
        assert_eq!(Cursor::decode(&c, "trade|x|1|2", 2).unwrap().k, vec![5, 7]);
        assert_eq!(
            Cursor::decode(&c, "trade|y|1|2", 2).unwrap_err().code,
            ErrorCode::InvalidCursor
        );
        assert!(Cursor::decode("!!", "q", 2).is_err());
        assert!(Cursor::decode(&c, "trade|x|1|2", 1).is_err());
    }
}
