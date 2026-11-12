//! Exact parsing of the numbers sources publish, and series-name helpers.
//!
//! Values are kept exactly as published (USDA's terms forbid altering
//! values presented as USDA data): a published `"245.50"` becomes the
//! fixed-point decimal `245.50`, never a float. Only spelling is
//! normalized (a leading `+`, a bare leading or trailing `.`, thousands
//! separators), and the usual "no value" markers parse as `None`.

use mesh_types::{InstrumentId, Price, VenueId};
use sha2::{Digest, Sha256};

/// Why a cell is not a number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a number: {0:?}")]
pub struct NotANumber(pub String);

/// True for the markers sources use for "no value".
#[must_use]
pub fn is_missing(s: &str) -> bool {
    matches!(
        s.trim(),
        "" | "." | "-" | "--" | "ND" | "NA" | "N/A" | "n/a" | "NC" | "*" | "null" | "NULL"
    )
}

/// Parses a published decimal exactly. `Ok(None)` for a "no value"
/// marker.
///
/// # Errors
///
/// Fails on anything that is not a plain decimal (exponents, letters,
/// more than 18 fractional digits).
pub fn decimal(s: &str) -> Result<Option<Price>, NotANumber> {
    let t = s.trim();
    if is_missing(t) {
        return Ok(None);
    }
    let bad = || NotANumber(s.to_owned());
    let (neg, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let body: String = body.chars().filter(|c| *c != ',').collect();
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body.as_str(), ""),
    };
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let mut canon = String::with_capacity(body.len() + 2);
    if neg && (int != "0" || frac.bytes().any(|b| b != b'0')) {
        canon.push('-');
    }
    canon.push_str(int);
    if !frac.is_empty() {
        canon.push('.');
        canon.push_str(frac);
    }
    Price::parse_exact(&canon).map(Some).map_err(|_| bad())
}

/// Parses a non-negative integer count (thousands separators allowed).
/// `Ok(None)` for a "no value" marker.
///
/// # Errors
///
/// Fails on anything else.
pub fn count(s: &str) -> Result<Option<u64>, NotANumber> {
    let t = s.trim();
    if is_missing(t) {
        return Ok(None);
    }
    let digits: String = t.chars().filter(|c| *c != ',').collect();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NotANumber(s.to_owned()));
    }
    digits
        .parse()
        .map(Some)
        .map_err(|_| NotANumber(s.to_owned()))
}

/// The decimal with trailing fractional zeros removed, for comparing
/// values whose spelling changed (`4.30` and `4.3` are the same value).
#[must_use]
pub fn normalized(p: Price) -> String {
    let s = p.to_string();
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// A series-name segment from free text: ASCII letters and digits kept,
/// every other run of characters written as one `_`, trimmed.
#[must_use]
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if pending && !out.is_empty() {
                out.push('_');
            }
            pending = false;
            out.push(c.to_ascii_uppercase());
        } else {
            pending = true;
        }
    }
    out
}

/// A symbol segment kept as published where the characters are allowed in
/// instrument IDs (`[A-Za-z0-9_.@#+-]`), with `$` written `@` and any other
/// character written `_`.
#[must_use]
pub fn symbol_segment(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '.' | '@' | '#' | '+' | '-' => c,
            '$' => '@',
            _ => '_',
        })
        .collect()
}

/// The instrument `<venue>:<class>:<symbol>`. A symbol that would make the
/// ID longer than 128 bytes is shortened and suffixed with eight hex digits
/// of its SHA-256, so distinct long names stay distinct.
///
/// # Errors
///
/// Fails if the result is not a valid ID (an empty symbol, for example).
pub fn instrument(
    venue: &VenueId,
    class: &str,
    symbol: &str,
) -> Result<InstrumentId, mesh_types::IdError> {
    let room = 128usize.saturating_sub(venue.as_str().len() + class.len() + 2);
    let symbol = if symbol.len() > room {
        let digest = Sha256::digest(symbol.as_bytes());
        let tag: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
        let keep = room.saturating_sub(9);
        let mut cut = symbol[..keep.min(symbol.len())].to_owned();
        while cut.ends_with(['.', ':', '_']) {
            cut.pop();
        }
        format!("{cut}.{tag}")
    } else {
        symbol.to_owned()
    };
    InstrumentId::from_parts(venue, class, &symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> String {
        decimal(s).unwrap().unwrap().to_string()
    }

    #[test]
    fn decimals_are_exact() {
        assert_eq!(d("245.50"), "245.50");
        assert_eq!(d(" 4.3 "), "4.3");
        assert_eq!(d("+1.25"), "1.25");
        assert_eq!(d(".5"), "0.5");
        assert_eq!(d("5."), "5");
        assert_eq!(d("-.25"), "-0.25");
        assert_eq!(d("-0.00"), "0.00");
        assert_eq!(d("1,234.5"), "1234.5");
        assert_eq!(d("007"), "7");
        for m in ["", "ND", "N/A", "-", "."] {
            assert_eq!(decimal(m).unwrap(), None, "{m:?}");
        }
        for b in ["1e5", "abc", "1.2.3", "--1", "1.0000000000000000001"] {
            assert!(decimal(b).is_err(), "{b:?}");
        }
        assert_eq!(normalized(decimal("4.300").unwrap().unwrap()), "4.3");
        assert_eq!(normalized(decimal("4.000").unwrap().unwrap()), "4");
        assert_eq!(count("1,200").unwrap(), Some(1200));
        assert!(count("-5").is_err());
        assert_eq!(count(" ").unwrap(), None);
    }

    #[test]
    fn names() {
        assert_eq!(slug("Medium and Large 1-2"), "MEDIUM_AND_LARGE_1_2");
        assert_eq!(slug("  Prod. Sold Negotiated "), "PROD_SOLD_NEGOTIATED");
        assert_eq!(symbol_segment("RXI$US_N.B.EU"), "RXI@US_N.B.EU");
        assert_eq!(symbol_segment("HB NORTH/1"), "HB_NORTH_1");
        let v = VenueId::new("example").unwrap();
        let long = "X".repeat(200);
        let a = instrument(&v, "ref", &long).unwrap();
        let b = instrument(&v, "ref", &format!("{long}Y")).unwrap();
        assert!(a.as_str().len() <= 128);
        assert_ne!(a, b);
        assert!(instrument(&v, "ref", "").is_err());
    }
}
