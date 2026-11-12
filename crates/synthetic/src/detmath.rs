//! Deterministic `exp` and `ln`.
//!
//! `f64::exp` and `f64::ln` call the platform's math library, whose last
//! bit may differ between systems. The method must give the same digits
//! everywhere (method spec § 3), so these use only IEEE 754 addition,
//! subtraction, multiplication and division (which are correctly rounded
//! everywhere), with fixed range reduction and fixed series lengths. Rust
//! never fuses multiply-adds implicitly, so the results are bit-identical
//! on every conforming platform. Accuracy is within a few units in the last
//! place, far finer than any published scale.

/// ln 2 split into a high part with trailing zero bits (exact in products
/// with small integers) and a low correction.
const LN2_HI: f64 = 6.931_471_803_691_238e-1;
const LN2_LO: f64 = 1.908_214_929_270_587_7e-10;
const LN2: f64 = std::f64::consts::LN_2;

/// `2^k` for `k` in the normal exponent range.
fn pow2(k: i32) -> f64 {
    let k = k.clamp(-1022, 1023);
    // The biased exponent is in 1..=2046, so the cast cannot lose bits.
    let biased = u64::try_from(k + 1023).unwrap_or(1023);
    f64::from_bits(biased << 52)
}

/// Deterministic `e^x`. Saturates to `0` below -745 and to infinity above
/// 709.
#[must_use]
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -745.0 {
        return 0.0;
    }
    // x = k ln2 + r, |r| <= ln2 / 2.
    let k = (x / LN2).round();
    let r = (x - k * LN2_HI) - k * LN2_LO;
    // Taylor series of e^r in Horner form, 1 + r(1 + r/2(1 + r/3(...))):
    // 24 terms reach far below one ulp for |r| < 0.35.
    let mut sum = 1.0;
    for n in (1..=24).rev() {
        sum = 1.0 + sum * r / f64::from(n);
    }
    // k is within [-1075, 1023] here.
    #[allow(clippy::cast_possible_truncation)]
    let k = k as i32;
    if k < -1022 {
        // Two steps keep the intermediate normal.
        sum * pow2(-1022) * pow2(k + 1022)
    } else {
        sum * pow2(k)
    }
}

/// Deterministic natural logarithm. `NaN` for negative input, negative
/// infinity for zero.
#[must_use]
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x.is_infinite() {
        return f64::INFINITY;
    }
    // Normalise subnormals.
    let (x, extra) = if x < f64::MIN_POSITIVE {
        (x * pow2(54), -54)
    } else {
        (x, 0)
    };
    let bits = x.to_bits();
    // The exponent field is 11 bits, so the cast is exact.
    let exp_field = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(1023);
    let mut e = exp_field - 1023 + extra;
    // Mantissa in [1, 2).
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if m > std::f64::consts::SQRT_2 {
        m /= 2.0;
        e += 1;
    }
    // ln m = 2 atanh(s), s = (m - 1) / (m + 1), |s| < 0.172.
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut term = s;
    let mut sum = 0.0;
    let mut n = 1.0;
    for _ in 0..30 {
        sum += term / n;
        term *= s2;
        n += 2.0;
    }
    let ef = f64::from(e);
    (ef * LN2_HI + 2.0 * sum) + ef * LN2_LO
}

/// Deterministic `base^power` for `base > 0`.
#[must_use]
pub fn powf(base: f64, power: f64) -> f64 {
    exp(power * ln(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-300)
    }

    #[test]
    fn matches_the_platform_closely() {
        let mut x = -30.0;
        while x < 30.0 {
            assert!(close(exp(x), x.exp(), 1e-15), "exp {x}");
            x += 0.0137;
        }
        let mut y = 1e-12;
        while y < 1e12 {
            assert!(
                (ln(y) - y.ln()).abs() <= 1e-15 * y.ln().abs().max(1.0),
                "ln {y}"
            );
            y *= 1.37;
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert!(close(powf(1.05, 2.5), 1.05f64.powf(2.5), 1e-15));
    }

    #[test]
    fn edges() {
        assert!(exp(f64::NAN).is_nan());
        assert_eq!(exp(1000.0), f64::INFINITY);
        assert_eq!(exp(-1000.0), 0.0);
        assert!(ln(-1.0).is_nan());
        assert_eq!(ln(0.0), f64::NEG_INFINITY);
        assert!(close(ln(5e-310), 5e-310f64.ln(), 1e-15));
        assert!(close(exp(-740.0), (-740.0f64).exp(), 1e-6));
    }

    #[test]
    fn fixed_values_are_pinned() {
        // Bit patterns are part of the method: a change here changes
        // published digits.
        assert_eq!(exp(0.5).to_bits(), 0x3ffa_6129_8e1e_069c);
        assert_eq!(ln(2.0).to_bits(), LN2.to_bits());
    }
}
