//! Fixed-point decimal numbers for canonical data.
//!
//! Canonical data never uses floating point. A value is an `i64` mantissa
//! and a scale (number of fractional digits): `mantissa * 10^-scale`. The
//! scale is fixed per instrument and comes from the registry (see
//! [`crate::registry`]).
//!
//! # Text form
//!
//! The canonical text form has exactly `scale` fractional digits, a leading
//! `-` for negative values and no exponent, `+` sign or grouping:
//! `Price::new(123_400, 3)` formats as `123.400`, and scale 0 has no decimal
//! point. Parsing that text recovers the same mantissa and scale, so JSON
//! round-trips are exact.
//!
//! [`Price::parse_with_scale`] parses venue text (which often drops trailing
//! zeros, such as `"123.4"`) into a known instrument scale. It rejects
//! input with more significant fractional digits than the scale instead of
//! rounding.
//!
//! # Equality and ordering
//!
//! Equality is structural: `1.0` (scale 1) and `1.00` (scale 2) are
//! different values, because canonical data for one instrument always uses
//! one scale. Ordering compares numeric value first and scale second, which
//! is a total order consistent with equality.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Largest supported scale. `10^18` is the largest power of ten in an `i64`.
pub const MAX_SCALE: u8 = 18;

/// Errors from constructing or parsing a fixed-point value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecimalError {
    /// The input string is empty.
    #[error("empty decimal string")]
    Empty,
    /// The input is not of the form `-?[0-9]+(\.[0-9]+)?`.
    #[error("malformed decimal string")]
    Malformed,
    /// The input has significant fractional digits beyond the scale.
    #[error("decimal has more fractional digits than scale {scale}")]
    TooPrecise {
        /// The scale that was requested.
        scale: u8,
    },
    /// The value does not fit a 64-bit mantissa at the requested scale.
    #[error("decimal out of range for a 64-bit mantissa")]
    Overflow,
    /// The scale exceeds [`MAX_SCALE`].
    #[error("scale {0} exceeds the maximum of {MAX_SCALE}")]
    ScaleTooLarge(u8),
    /// A negative value was given where only non-negative values are valid.
    #[error("negative value where a non-negative one is required")]
    Negative,
    /// Two values with different scales were combined.
    #[error("scales differ: {0} and {1}")]
    ScaleMismatch(u8, u8),
}

/// Parsed decimal: mantissa and scale.
fn parse_decimal(s: &str, target_scale: Option<u8>) -> Result<(i64, u8), DecimalError> {
    if s.is_empty() {
        return Err(DecimalError::Empty);
    }
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int_part, frac_part) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let all_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int_part) || frac_part.is_some_and(|f| !all_digits(f)) {
        return Err(DecimalError::Malformed);
    }
    let frac = frac_part.unwrap_or("");
    let scale = match target_scale {
        Some(scale) => {
            if scale > MAX_SCALE {
                return Err(DecimalError::ScaleTooLarge(scale));
            }
            // Digits beyond the scale must all be zero.
            if frac.len() > usize::from(scale)
                && frac.as_bytes()[usize::from(scale)..]
                    .iter()
                    .any(|&b| b != b'0')
            {
                return Err(DecimalError::TooPrecise { scale });
            }
            scale
        }
        None => {
            let n = u8::try_from(frac.len()).unwrap_or(u8::MAX);
            if n > MAX_SCALE {
                return Err(DecimalError::ScaleTooLarge(n));
            }
            n
        }
    };

    let mut acc: i128 = 0;
    let push = |acc: i128, digit: u8| -> Result<i128, DecimalError> {
        let v = acc * 10 + i128::from(digit - b'0');
        // Keep within i64 magnitude (+1 for i64::MIN) as we go.
        if v > i128::from(i64::MAX) + 1 {
            Err(DecimalError::Overflow)
        } else {
            Ok(v)
        }
    };
    for &b in int_part.as_bytes() {
        acc = push(acc, b)?;
    }
    let frac_bytes = frac.as_bytes();
    for i in 0..usize::from(scale) {
        acc = push(acc, frac_bytes.get(i).copied().unwrap_or(b'0'))?;
    }
    let signed = if negative { -acc } else { acc };
    let mantissa = i64::try_from(signed).map_err(|_| DecimalError::Overflow)?;
    Ok((mantissa, scale))
}

/// Writes `mantissa * 10^-scale` in canonical text form.
fn format_decimal(mantissa: i64, scale: u8, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let negative = mantissa < 0;
    let abs = mantissa.unsigned_abs();
    if scale == 0 {
        return write!(f, "{}{abs}", if negative { "-" } else { "" });
    }
    let pow = 10u64.pow(u32::from(scale));
    let int = abs / pow;
    let frac = abs % pow;
    write!(
        f,
        "{}{int}.{frac:0width$}",
        if negative { "-" } else { "" },
        width = usize::from(scale)
    )
}

/// Numeric comparison of two fixed-point values with possibly different
/// scales. Exact: both are widened to `i128` at the larger scale.
fn cmp_value(a: (i64, u8), b: (i64, u8)) -> Ordering {
    let scale = a.1.max(b.1);
    let widen = |(m, s): (i64, u8)| i128::from(m) * 10i128.pow(u32::from(scale - s));
    widen(a).cmp(&widen(b)).then(a.1.cmp(&b.1))
}

macro_rules! fixed_point {
    ($(#[$doc:meta])* $name:ident, allow_negative = $neg:expr) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name {
            mantissa: i64,
            scale: u8,
        }

        impl $name {
            /// Builds a value from a mantissa and scale.
            ///
            /// # Errors
            ///
            /// Fails if the scale exceeds [`MAX_SCALE`] or the value is not
            /// permitted for this type.
            pub fn new(mantissa: i64, scale: u8) -> Result<Self, DecimalError> {
                if scale > MAX_SCALE {
                    return Err(DecimalError::ScaleTooLarge(scale));
                }
                if !$neg && mantissa < 0 {
                    return Err(DecimalError::Negative);
                }
                Ok(Self { mantissa, scale })
            }

            /// Zero at the given scale.
            ///
            /// # Errors
            ///
            /// Fails if the scale exceeds [`MAX_SCALE`].
            pub fn zero(scale: u8) -> Result<Self, DecimalError> {
                Self::new(0, scale)
            }

            /// Parses canonical text, taking the scale from the number of
            /// fractional digits (`"1.50"` has scale 2).
            ///
            /// # Errors
            ///
            /// Fails on malformed input, overflow, or a disallowed value.
            pub fn parse_exact(s: &str) -> Result<Self, DecimalError> {
                let (m, sc) = parse_decimal(s, None)?;
                Self::new(m, sc)
            }

            /// Parses venue text into a known scale. Missing fractional
            /// digits are zero-filled; extra fractional digits must be zero.
            ///
            /// # Errors
            ///
            /// Fails on malformed input, digits beyond the scale that are not
            /// zero, overflow, or a disallowed value.
            pub fn parse_with_scale(s: &str, scale: u8) -> Result<Self, DecimalError> {
                let (m, sc) = parse_decimal(s, Some(scale))?;
                Self::new(m, sc)
            }

            /// The integer mantissa.
            #[must_use]
            pub const fn mantissa(self) -> i64 {
                self.mantissa
            }

            /// The number of fractional digits.
            #[must_use]
            pub const fn scale(self) -> u8 {
                self.scale
            }

            /// True if the value is zero.
            #[must_use]
            pub const fn is_zero(self) -> bool {
                self.mantissa == 0
            }

            /// True if the value is greater than zero.
            #[must_use]
            pub const fn is_positive(self) -> bool {
                self.mantissa > 0
            }

            /// Re-expresses the value at another scale, exactly.
            ///
            /// # Errors
            ///
            /// Fails if reducing the scale would drop non-zero digits, or if
            /// the result overflows.
            pub fn rescale(self, scale: u8) -> Result<Self, DecimalError> {
                if scale > MAX_SCALE {
                    return Err(DecimalError::ScaleTooLarge(scale));
                }
                let mantissa = if scale >= self.scale {
                    let f = 10i64.pow(u32::from(scale - self.scale));
                    self.mantissa.checked_mul(f).ok_or(DecimalError::Overflow)?
                } else {
                    let f = 10i64.pow(u32::from(self.scale - scale));
                    if self.mantissa % f != 0 {
                        return Err(DecimalError::TooPrecise { scale });
                    }
                    self.mantissa / f
                };
                Self::new(mantissa, scale)
            }

            /// Adds two values of the same scale.
            ///
            /// # Errors
            ///
            /// Fails on a scale mismatch or overflow.
            pub fn checked_add(self, other: Self) -> Result<Self, DecimalError> {
                if self.scale != other.scale {
                    return Err(DecimalError::ScaleMismatch(self.scale, other.scale));
                }
                let m = self
                    .mantissa
                    .checked_add(other.mantissa)
                    .ok_or(DecimalError::Overflow)?;
                Self::new(m, self.scale)
            }

            /// Subtracts a value of the same scale.
            ///
            /// # Errors
            ///
            /// Fails on a scale mismatch, overflow, or a disallowed result
            /// (a negative [`Size`]).
            pub fn checked_sub(self, other: Self) -> Result<Self, DecimalError> {
                if self.scale != other.scale {
                    return Err(DecimalError::ScaleMismatch(self.scale, other.scale));
                }
                let m = self
                    .mantissa
                    .checked_sub(other.mantissa)
                    .ok_or(DecimalError::Overflow)?;
                Self::new(m, self.scale)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                format_decimal(self.mantissa, self.scale, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}(", stringify!($name))?;
                format_decimal(self.mantissa, self.scale, f)?;
                write!(f, ")")
            }
        }

        impl FromStr for $name {
            type Err = DecimalError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse_exact(s)
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for $name {
            fn cmp(&self, other: &Self) -> Ordering {
                cmp_value((self.mantissa, self.scale), (other.mantissa, other.scale))
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                Self::parse_exact(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

fixed_point!(
    /// A price: signed fixed-point decimal with a per-instrument scale.
    ///
    /// Negative prices are allowed (spreads, some rates, historical
    /// commodity episodes).
    Price,
    allow_negative = true
);

fixed_point!(
    /// A size, volume or notional: non-negative fixed-point decimal with a
    /// per-instrument scale.
    Size,
    allow_negative = false
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exact_takes_scale_from_digits() {
        let p = Price::parse_exact("123.400").unwrap();
        assert_eq!((p.mantissa(), p.scale()), (123_400, 3));
        let p = Price::parse_exact("-0.05").unwrap();
        assert_eq!((p.mantissa(), p.scale()), (-5, 2));
        let p = Price::parse_exact("42").unwrap();
        assert_eq!((p.mantissa(), p.scale()), (42, 0));
    }

    #[test]
    fn parse_with_scale_pads_and_rejects_excess() {
        let p = Price::parse_with_scale("123.4", 3).unwrap();
        assert_eq!((p.mantissa(), p.scale()), (123_400, 3));
        let p = Price::parse_with_scale("7", 2).unwrap();
        assert_eq!(p.mantissa(), 700);
        // Trailing zeros beyond the scale are fine.
        let p = Price::parse_with_scale("1.2000", 1).unwrap();
        assert_eq!(p.mantissa(), 12);
        assert_eq!(
            Price::parse_with_scale("1.25", 1),
            Err(DecimalError::TooPrecise { scale: 1 })
        );
    }

    #[test]
    fn rejects_malformed() {
        for s in [
            "", "-", ".5", "5.", "+1", "1e5", "1.2.3", " 1", "1 ", "0x10", "١",
        ] {
            assert!(Price::parse_exact(s).is_err(), "accepted {s:?}");
        }
        assert_eq!(Price::parse_exact(""), Err(DecimalError::Empty));
    }

    #[test]
    fn overflow_and_bounds() {
        assert_eq!(
            Price::parse_exact("9223372036854775807")
                .unwrap()
                .mantissa(),
            i64::MAX
        );
        assert_eq!(
            Price::parse_exact("-9223372036854775808")
                .unwrap()
                .mantissa(),
            i64::MIN
        );
        assert_eq!(
            Price::parse_exact("9223372036854775808"),
            Err(DecimalError::Overflow)
        );
        assert_eq!(
            Price::parse_with_scale("92233720368547758.08", 3),
            Err(DecimalError::Overflow)
        );
        assert!(Price::parse_exact("0.0000000000000000001").is_err());
        assert_eq!(Price::new(1, 19), Err(DecimalError::ScaleTooLarge(19)));
    }

    #[test]
    fn size_is_non_negative() {
        assert_eq!(Size::parse_exact("-1"), Err(DecimalError::Negative));
        assert_eq!(Size::new(-1, 0), Err(DecimalError::Negative));
        let a = Size::new(5, 1).unwrap();
        let b = Size::new(7, 1).unwrap();
        assert_eq!(a.checked_sub(b), Err(DecimalError::Negative));
        assert_eq!(b.checked_sub(a).unwrap().mantissa(), 2);
    }

    #[test]
    fn display_round_trips() {
        for (m, s) in [
            (0, 0),
            (0, 4),
            (1, 4),
            (-1, 4),
            (123_400, 3),
            (i64::MAX, 0),
            (i64::MIN, 0),
            (i64::MIN, 18),
            (i64::MAX, 18),
            (-5, 2),
        ] {
            let p = Price::new(m, s).unwrap();
            let text = p.to_string();
            assert_eq!(Price::parse_exact(&text).unwrap(), p, "{text}");
        }
        assert_eq!(Price::new(-5, 2).unwrap().to_string(), "-0.05");
        assert_eq!(Price::new(123_400, 3).unwrap().to_string(), "123.400");
        assert_eq!(Price::new(42, 0).unwrap().to_string(), "42");
        assert_eq!(
            Price::new(i64::MIN, 18).unwrap().to_string(),
            "-9.223372036854775808"
        );
    }

    #[test]
    fn ordering_is_numeric_then_scale() {
        let a = Price::new(10, 1).unwrap(); // 1.0
        let b = Price::new(100, 2).unwrap(); // 1.00
        let c = Price::new(101, 2).unwrap(); // 1.01
        assert_ne!(a, b);
        assert!(a < b && b < c);
        let mut v = [c, b, a, Price::new(-1, 0).unwrap()];
        v.sort();
        assert_eq!(v[0].mantissa(), -1);
        assert_eq!(v[3], c);
    }

    #[test]
    fn rescale_is_exact() {
        let p = Price::new(1230, 3).unwrap();
        assert_eq!(p.rescale(2).unwrap().mantissa(), 123);
        assert_eq!(p.rescale(5).unwrap().mantissa(), 123_000);
        assert!(Price::new(1231, 3).unwrap().rescale(2).is_err());
        assert_eq!(
            Price::new(i64::MAX, 0).unwrap().rescale(1),
            Err(DecimalError::Overflow)
        );
    }

    #[test]
    fn arithmetic_requires_same_scale() {
        let a = Size::new(10, 1).unwrap();
        let b = Size::new(10, 2).unwrap();
        assert_eq!(a.checked_add(b), Err(DecimalError::ScaleMismatch(1, 2)));
        assert_eq!(a.checked_add(a).unwrap().mantissa(), 20);
        assert_eq!(
            Size::new(i64::MAX, 0)
                .unwrap()
                .checked_add(Size::new(1, 0).unwrap()),
            Err(DecimalError::Overflow)
        );
    }

    #[test]
    fn serde_is_a_string() {
        let p = Price::new(-12_345, 2).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"-123.45\"");
        assert_eq!(serde_json::from_str::<Price>(&json).unwrap(), p);
        assert!(serde_json::from_str::<Price>("123.45").is_err());
        assert!(serde_json::from_str::<Size>("\"-1\"").is_err());
    }
}
