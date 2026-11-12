//! Exact parsing of disseminated cell values.
//!
//! No floating point is used anywhere. Numbers are rewritten to canonical
//! decimal text (thousands separators removed, exponents expanded) and
//! handed to `mesh_types` fixed-point parsing, which takes the scale from
//! the digits.

use mesh_types::{Price, Size, UnixMillis, UnixNanos};

/// Errors from parsing a cell.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValueError {
    /// Not a number in any accepted notation.
    #[error("malformed number {0:?}")]
    Number(String),
    /// More fractional digits than the fixed-point type can hold, or out of
    /// range.
    #[error("number out of range {0:?}")]
    Range(String),
    /// Not a timestamp in any accepted form.
    #[error("malformed timestamp {0:?}")]
    Timestamp(String),
    /// Not a date in any accepted form.
    #[error("malformed date {0:?}")]
    Date(String),
}

/// A disseminated number: canonical decimal text, and whether the
/// repository marked it as capped (a trailing `+`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Number {
    /// Canonical decimal text (`-?digits(.digits)?`).
    pub text: String,
    /// The repository capped the value (`250,000,000+`).
    pub capped: bool,
}

fn take_cell(s: &str) -> Option<&str> {
    let t = s.trim().trim_matches('"').trim();
    if t.is_empty() {
        return None;
    }
    let upper = t.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "NA" | "N/A" | "NULL" | "NONE" | "UNKNOWN" | "-"
    ) {
        return None;
    }
    Some(t)
}

/// True for the technical specification's "unknown" sentinel: every digit is
/// `9`, with at least five digits (for example `99999.9999999999999`).
fn is_all_nines(text: &str) -> bool {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_digit).collect();
    digits.len() >= 5 && digits.iter().all(|&d| d == b'9')
}

/// Parses a disseminated number. Returns `Ok(None)` for an empty cell, a
/// placeholder (`NA`) or the all-nines "unknown" sentinel.
///
/// Accepts thousands separators (`1,250,000`), a leading `+`, a trailing
/// `+` (capped), and exponents (`1.5E-4`), expanded exactly.
///
/// # Errors
///
/// Fails on anything else.
pub fn number(cell: &str) -> Result<Option<Number>, ValueError> {
    let Some(t) = take_cell(cell) else {
        return Ok(None);
    };
    let bad = || ValueError::Number(t.chars().take(64).collect());
    let (body, capped) = match t.strip_suffix('+') {
        Some(rest) => (rest.trim_end(), true),
        None => (t, false),
    };
    let body = body.strip_prefix('+').unwrap_or(body);
    let (negative, body) = match body.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, body),
    };
    let body: String = body.chars().filter(|&c| c != ',' && c != '_').collect();
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(i) => {
            let exp: i32 = body[i + 1..].parse().map_err(|_| bad())?;
            (&body[..i], exp)
        }
        None => (body.as_str(), 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(bad());
    }
    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    if !(-40..=40).contains(&exponent) {
        return Err(ValueError::Range(t.chars().take(64).collect()));
    }
    // Shift the decimal point by `exponent` over the digit string.
    let digits = format!("{int_part}{frac_part}");
    let point = i64::try_from(int_part.len()).map_err(|_| bad())? + i64::from(exponent);
    let (int_digits, frac_digits) = if point <= 0 {
        let zeros = "0".repeat(usize::try_from(-point).map_err(|_| bad())?);
        ("0".to_string(), format!("{zeros}{digits}"))
    } else {
        let p = usize::try_from(point).map_err(|_| bad())?;
        if p >= digits.len() {
            (
                format!("{digits}{}", "0".repeat(p - digits.len())),
                String::new(),
            )
        } else {
            (digits[..p].to_string(), digits[p..].to_string())
        }
    };
    let int_digits = int_digits.trim_start_matches('0');
    let int_digits = if int_digits.is_empty() {
        "0"
    } else {
        int_digits
    };
    let mut text = String::new();
    let is_zero = int_digits == "0" && frac_digits.bytes().all(|b| b == b'0');
    if negative && !is_zero {
        text.push('-');
    }
    text.push_str(int_digits);
    if !frac_digits.is_empty() {
        text.push('.');
        text.push_str(&frac_digits);
    }
    if is_all_nines(&text) {
        return Ok(None);
    }
    Ok(Some(Number { text, capped }))
}

/// Drops trailing fractional zeros so values fit a fixed-point scale.
fn trim_fraction(text: &str) -> &str {
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text
    }
}

/// Converts canonical text to a [`Price`], taking the scale from the digits.
///
/// # Errors
///
/// Fails if the value needs more than 18 fractional digits or overflows.
pub fn to_price(text: &str) -> Result<Price, ValueError> {
    Price::parse_exact(trim_fraction(text)).map_err(|_| ValueError::Range(text.to_string()))
}

/// Converts canonical text to a [`Size`] (non-negative).
///
/// # Errors
///
/// Fails on a negative value, too many fractional digits or overflow.
pub fn to_size(text: &str) -> Result<Size, ValueError> {
    Size::parse_exact(trim_fraction(text)).map_err(|_| ValueError::Range(text.to_string()))
}

/// Multiplies a price by `10^k` exactly (used for decimal to percent, `k =
/// 2`, and decimal to basis points, `k = 4`).
///
/// # Errors
///
/// Fails on overflow.
pub fn shift(p: Price, k: u8) -> Result<Price, ValueError> {
    let range = || ValueError::Range(p.to_string());
    if p.scale() >= k {
        Price::new(p.mantissa(), p.scale() - k).map_err(|_| range())
    } else {
        let f = 10i64.pow(u32::from(k - p.scale()));
        let m = p.mantissa().checked_mul(f).ok_or_else(range)?;
        Price::new(m, 0).map_err(|_| range())
    }
}

/// Parses a disseminated boolean (`TRUE`, `True`, `Y`, `1`, ...).
#[must_use]
pub fn boolean(cell: &str) -> Option<bool> {
    let t = take_cell(cell)?;
    match t.to_ascii_uppercase().as_str() {
        "TRUE" | "T" | "Y" | "YES" | "1" => Some(true),
        "FALSE" | "F" | "N" | "NO" | "0" => Some(false),
        _ => None,
    }
}

/// Trimmed text, or `None` for an empty cell or placeholder.
#[must_use]
pub fn text(cell: &str) -> Option<String> {
    take_cell(cell).map(str::to_string)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Parses a date as `(year, month, day)`. Accepts `YYYY-MM-DD`,
/// `YYYY/MM/DD`, `YYYYMMDD`, `MM/DD/YYYY` and the date part of a
/// timestamp.
fn ymd(t: &str) -> Option<(i64, i64, i64)> {
    let num = |s: &str| -> Option<i64> {
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    let head: &str = t.split(['T', 't', ' ']).next().unwrap_or(t);
    let parts: Vec<&str> = head.split(['-', '/']).collect();
    let (y, m, d) = match parts.as_slice() {
        [y, m, d] if y.len() == 4 => (num(y)?, num(m)?, num(d)?),
        [m, d, y] if y.len() == 4 => (num(y)?, num(m)?, num(d)?),
        [all] if all.len() == 8 => (num(&all[..4])?, num(&all[4..6])?, num(&all[6..])?),
        _ => return None,
    };
    ((1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m) && (1000..=9999).contains(&y))
        .then_some((y, m, d))
}

/// Parses a date to `YYYY-MM-DD`. Returns `Ok(None)` for an empty cell.
///
/// # Errors
///
/// Fails on an unrecognized date.
pub fn date(cell: &str) -> Result<Option<String>, ValueError> {
    let Some(t) = take_cell(cell) else {
        return Ok(None);
    };
    let (y, m, d) = ymd(t).ok_or_else(|| ValueError::Date(t.chars().take(64).collect()))?;
    Ok(Some(format!("{y:04}-{m:02}-{d:02}")))
}

/// Parses a timestamp to milliseconds (floor). Returns `Ok(None)` for an
/// empty cell.
///
/// Accepts RFC 3339 with `T` or a space, `Z`, a numeric offset
/// (`+00:00`, `-0500`) or no zone (read as UTC), fractions of up to nine
/// digits, minutes-only times (`2026-11-12T14:05Z`), US dates
/// (`11/12/2026 14:05:00`), a bare date (midnight UTC) and integer epoch
/// seconds or milliseconds.
///
/// # Errors
///
/// Fails on anything else.
pub fn timestamp(cell: &str) -> Result<Option<UnixMillis>, ValueError> {
    let Some(t) = take_cell(cell) else {
        return Ok(None);
    };
    let bad = || ValueError::Timestamp(t.chars().take(64).collect());
    // Integer epoch: 10 digits seconds, 13 digits milliseconds.
    if t.bytes().all(|b| b.is_ascii_digit()) && (t.len() == 10 || t.len() == 13) {
        let v: i64 = t.parse().map_err(|_| bad())?;
        return Ok(Some(UnixMillis(if t.len() == 10 { v * 1000 } else { v })));
    }
    let (date_part, rest) = match t.find(['T', 't', ' ']) {
        Some(i) => (&t[..i], t[i + 1..].trim()),
        None => (t, ""),
    };
    let (y, m, d) = ymd(date_part).ok_or_else(bad)?;
    // Split off the zone.
    let (time, offset_secs) = split_zone(rest).ok_or_else(bad)?;
    let time = if time.is_empty() { "00:00:00" } else { time };
    let (hms, frac) = match time.split_once(['.', ',']) {
        Some((h, f)) => (h, Some(f)),
        None => (time, None),
    };
    let hms_parts: Vec<&str> = hms.split(':').collect();
    let (hh, mm, ss) = match hms_parts.as_slice() {
        [h, m] => (*h, *m, "00"),
        [h, m, s] => (*h, *m, *s),
        _ => return Err(bad()),
    };
    fn two(s: &str) -> Option<&str> {
        (s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit())).then_some(s)
    }
    let canonical = format!(
        "{y:04}-{m:02}-{d:02}T{}:{}:{}{}Z",
        two(hh).ok_or_else(bad)?,
        two(mm).ok_or_else(bad)?,
        two(ss).ok_or_else(bad)?,
        match frac {
            Some(f) if !f.is_empty() && f.len() <= 9 && f.bytes().all(|b| b.is_ascii_digit()) =>
                format!(".{f}"),
            Some(_) => return Err(bad()),
            None => String::new(),
        }
    );
    let ns = UnixNanos::parse_rfc3339(&canonical).map_err(|_| bad())?;
    let utc =
        ns.0.checked_sub(offset_secs.checked_mul(1_000_000_000).ok_or_else(bad)?)
            .ok_or_else(bad)?;
    Ok(Some(UnixNanos(utc).to_millis()))
}

/// Splits `HH:MM:SS[.f][zone]` into the time and the zone offset in
/// seconds east of UTC.
fn split_zone(rest: &str) -> Option<(&str, i64)> {
    let rest = rest.trim();
    if let Some(t) = rest.strip_suffix(['Z', 'z']) {
        return Some((t.trim(), 0));
    }
    if let Some(t) = rest
        .strip_suffix("UTC")
        .or_else(|| rest.strip_suffix("GMT"))
    {
        return Some((t.trim(), 0));
    }
    // A sign after the time part starts an offset.
    if let Some(i) = rest.rfind(['+', '-'])
        && i > 0
    {
        let (time, zone) = rest.split_at(i);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let z: String = zone[1..].chars().filter(|&c| c != ':').collect();
        if z.len() == 4 && z.bytes().all(|b| b.is_ascii_digit()) {
            let h: i64 = z[..2].parse().ok()?;
            let m: i64 = z[2..].parse().ok()?;
            if h > 23 || m > 59 {
                return None;
            }
            return Some((time.trim(), sign * (h * 3600 + m * 60)));
        }
        if z.len() == 2 && z.bytes().all(|b| b.is_ascii_digit()) {
            let h: i64 = z.parse().ok()?;
            return Some((time.trim(), sign * h * 3600));
        }
        return None;
    }
    Some((rest, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Option<(String, bool)> {
        number(s).unwrap().map(|x| (x.text, x.capped))
    }

    #[test]
    fn numbers() {
        assert_eq!(n("1,250,000"), Some(("1250000".into(), false)));
        assert_eq!(n("250,000,000+"), Some(("250000000".into(), true)));
        assert_eq!(n(" 0.0375 "), Some(("0.0375".into(), false)));
        assert_eq!(n("-0.0012"), Some(("-0.0012".into(), false)));
        assert_eq!(n("+12.5"), Some(("12.5".into(), false)));
        assert_eq!(n(".5"), Some(("0.5".into(), false)));
        assert_eq!(n("1.5E-4"), Some(("0.00015".into(), false)));
        assert_eq!(n("2.5e3"), Some(("2500".into(), false)));
        assert_eq!(n("-0"), Some(("0".into(), false)));
        assert_eq!(n(""), None);
        assert_eq!(n("NA"), None);
        assert_eq!(n("99999.9999999999999"), None);
        assert_eq!(n("9.9999999999"), None);
        assert_eq!(n("99"), Some(("99".into(), false)));
        assert!(number("12abc").is_err());
        assert!(number("1.2.3").is_err());
        assert!(number("1e400").is_err());
    }

    #[test]
    fn fixed_point() {
        let p = to_price("0.037500").unwrap();
        assert_eq!(p.to_string(), "0.0375");
        assert_eq!(shift(p, 2).unwrap().to_string(), "3.75");
        assert_eq!(
            shift(to_price("0.0062").unwrap(), 4).unwrap().to_string(),
            "62"
        );
        assert_eq!(shift(to_price("3").unwrap(), 2).unwrap().to_string(), "300");
        assert_eq!(to_size("1000000").unwrap().to_string(), "1000000");
        assert!(to_size("-1").is_err());
        assert!(to_price("0.0000000000000000001").is_err());
    }

    fn ts(s: &str) -> i64 {
        timestamp(s).unwrap().unwrap().0
    }

    #[test]
    fn timestamps() {
        let base = 1_794_441_600_000; // 2026-11-12T00:00:00Z
        assert_eq!(ts("2026-11-12T00:00:00Z"), base);
        assert_eq!(ts("2026-11-12 00:00:00"), base);
        assert_eq!(ts("2026-11-12T00:00:00.123Z"), base + 123);
        assert_eq!(ts("2026-11-12T00:00:00.123456789Z"), base + 123);
        assert_eq!(ts("2026-11-12T01:00:00+01:00"), base);
        assert_eq!(ts("2026-11-11T19:00:00-0500"), base);
        assert_eq!(ts("2026-11-12T00:00Z"), base);
        assert_eq!(ts("11/12/2026 00:00:00"), base);
        assert_eq!(ts("2026-11-12"), base);
        assert_eq!(ts("1794441600"), base);
        assert_eq!(ts("1794441600000"), base);
        assert_eq!(timestamp("").unwrap(), None);
        assert!(timestamp("yesterday").is_err());
        assert!(timestamp("2026-13-01T00:00:00Z").is_err());
        assert!(timestamp("2026-11-12T25:00:00Z").is_err());
    }

    #[test]
    fn dates_and_booleans() {
        assert_eq!(date("2031-11-14").unwrap().unwrap(), "2031-11-14");
        assert_eq!(date("2031/11/14").unwrap().unwrap(), "2031-11-14");
        assert_eq!(date("20311114").unwrap().unwrap(), "2031-11-14");
        assert_eq!(date("11/14/2031").unwrap().unwrap(), "2031-11-14");
        assert_eq!(date("2031-11-14T00:00:00Z").unwrap().unwrap(), "2031-11-14");
        assert!(date("2031-02-30").is_err());
        assert_eq!(boolean("TRUE"), Some(true));
        assert_eq!(boolean("false"), Some(false));
        assert_eq!(boolean(""), None);
    }
}
