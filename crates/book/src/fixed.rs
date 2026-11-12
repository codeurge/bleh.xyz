//! Fixed-point helpers.
//!
//! Canonical outputs never use floating point. Accumulators work on `i64`
//! mantissas at the instrument's scale (with checked arithmetic) and build
//! [`Price`] and [`Size`] values only when a record is emitted. Ratios are
//! integers in basis points or hundredths of a basis point, always rounded
//! toward zero (floor for non-negative values), so every implementation
//! gets the same digits.

use std::fmt;

use mesh_types::{DecimalError, Flag, Flags, Price, Size};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::InputError;

/// A signed quantity (delta, cumulative volume delta): an `i64` mantissa
/// and a scale, like [`Size`] but allowed to be negative.
///
/// The text form is the canonical decimal form used for [`Price`]: exactly
/// `scale` fractional digits and a leading `-` when negative.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignedSize {
    mantissa: i64,
    scale: u8,
}

impl SignedSize {
    /// Builds a value.
    ///
    /// # Errors
    ///
    /// Fails if the scale exceeds [`mesh_types::MAX_SCALE`].
    pub fn new(mantissa: i64, scale: u8) -> Result<Self, DecimalError> {
        Price::new(mantissa, scale)?;
        Ok(Self { mantissa, scale })
    }

    /// Zero at `scale`.
    ///
    /// # Errors
    ///
    /// Fails if the scale exceeds [`mesh_types::MAX_SCALE`].
    pub fn zero(scale: u8) -> Result<Self, DecimalError> {
        Self::new(0, scale)
    }

    /// `a - b`.
    ///
    /// # Errors
    ///
    /// Fails on a scale mismatch or overflow.
    pub fn difference(a: Size, b: Size) -> Result<Self, DecimalError> {
        if a.scale() != b.scale() {
            return Err(DecimalError::ScaleMismatch(a.scale(), b.scale()));
        }
        let m = a
            .mantissa()
            .checked_sub(b.mantissa())
            .ok_or(DecimalError::Overflow)?;
        Self::new(m, a.scale())
    }

    /// Adds a value of the same scale.
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

    fn as_price(self) -> Option<Price> {
        Price::new(self.mantissa, self.scale).ok()
    }
}

impl fmt::Display for SignedSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_price() {
            Some(p) => p.fmt(f),
            None => write!(f, "{}e-{}", self.mantissa, self.scale),
        }
    }
}

impl fmt::Debug for SignedSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SignedSize({self})")
    }
}

impl Serialize for SignedSize {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SignedSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let p = Price::deserialize(deserializer)?;
        Ok(Self {
            mantissa: p.mantissa(),
            scale: p.scale(),
        })
    }
}

/// The union of two flag sets.
#[must_use]
pub fn union(a: Flags, b: Flags) -> Flags {
    Flag::ALL
        .into_iter()
        .filter(|f| b.contains(*f))
        .fold(a, Flags::with)
}

/// Checked `i64` addition as an [`InputError`].
pub(crate) fn add(a: i64, b: i64) -> Result<i64, InputError> {
    a.checked_add(b).ok_or(InputError::Overflow)
}

/// Checked `u32` increment as an [`InputError`].
pub(crate) fn inc(a: u32) -> Result<u32, InputError> {
    a.checked_add(1).ok_or(InputError::Overflow)
}

/// Checks that `found` equals `expected`, as an [`InputError`].
pub(crate) fn same_scale(field: &'static str, expected: u8, found: u8) -> Result<(), InputError> {
    if expected == found {
        Ok(())
    } else {
        Err(InputError::ScaleMismatch {
            field,
            expected,
            found,
        })
    }
}

/// A [`Price`] from a mantissa and scale.
pub(crate) fn price(m: i64, scale: u8) -> Result<Price, InputError> {
    Ok(Price::new(m, scale)?)
}

/// A [`Size`] from a mantissa and scale.
pub(crate) fn size(m: i64, scale: u8) -> Result<Size, InputError> {
    Ok(Size::new(m, scale)?)
}

/// `n * 10_000 / d` rounded toward zero, or `None` when `d` is zero.
#[must_use]
pub fn ratio_bps(n: u64, d: u64) -> Option<u64> {
    if d == 0 {
        return None;
    }
    let v = u128::from(n) * 10_000 / u128::from(d);
    Some(u64::try_from(v).unwrap_or(u64::MAX))
}

/// `ceil(total * bps / 10_000)` for non-negative values, in `i128`.
#[must_use]
pub fn ceil_bps(total: i128, bps: u32) -> i128 {
    let num = total * i128::from(bps);
    num.div_euclid(10_000) + i128::from(num.rem_euclid(10_000) != 0)
}

/// Spread in hundredths of a basis point of the mid:
/// `floor((ask - bid) * 2 * 1_000_000 / (ask + bid))`, from mantissas at
/// one scale. `None` if `ask + bid` is not positive or `ask < bid`.
#[must_use]
pub fn spread_centi_bps(bid: i64, ask: i64) -> Option<u64> {
    let (b, a) = (i128::from(bid), i128::from(ask));
    let sum = a + b;
    if sum <= 0 || a < b {
        return None;
    }
    let v = (a - b) * 2_000_000 / sum;
    u64::try_from(v).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_size_text_round_trip() {
        let v = SignedSize::new(-1_250, 3).unwrap();
        assert_eq!(v.to_string(), "-1.250");
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "\"-1.250\"");
        assert_eq!(serde_json::from_str::<SignedSize>(&json).unwrap(), v);
        let a = Size::new(5, 1).unwrap();
        let b = Size::new(7, 1).unwrap();
        assert_eq!(SignedSize::difference(a, b).unwrap().mantissa(), -2);
        assert!(SignedSize::new(1, 19).is_err());
        assert!(
            SignedSize::new(i64::MAX, 0)
                .unwrap()
                .checked_add(SignedSize::new(1, 0).unwrap())
                .is_err()
        );
    }

    #[test]
    fn ratios_round_toward_zero() {
        assert_eq!(ratio_bps(1, 3), Some(3_333));
        assert_eq!(ratio_bps(5, 0), None);
        assert_eq!(ceil_bps(10, 7_000), 7);
        assert_eq!(ceil_bps(11, 7_000), 8);
        assert_eq!(ceil_bps(0, 7_000), 0);
        // bid 99.0, ask 101.0: spread 2 on mid 100 = 200 bps = 20_000 cbps.
        assert_eq!(spread_centi_bps(990, 1010), Some(20_000));
        assert_eq!(spread_centi_bps(1010, 990), None);
        assert_eq!(spread_centi_bps(-5, 1), None);
    }

    #[test]
    fn flag_union() {
        let a = Flags::NONE.with(Flag::Backfill);
        let b = Flags::NONE.with(Flag::GapBefore);
        let u = union(a, b);
        assert!(u.contains(Flag::Backfill) && u.contains(Flag::GapBefore));
        assert!(!u.contains(Flag::SizeCapped));
    }
}
