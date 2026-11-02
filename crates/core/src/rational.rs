//! Exact fractions, so time arithmetic never drifts.

use std::cmp::Ordering;
use std::fmt;
use std::num::NonZeroU32;

/// Plan times are rounded to this many steps per second (microseconds).
const MICROS_PER_SECOND: i64 = 1_000_000;

/// 2^63: the smallest magnitude that no longer fits in an `i64`.
const I64_LIMIT: f64 = 9_223_372_036_854_775_808.0;

/// An exact fraction `numer / denom`, always in lowest terms with `denom > 0`.
///
/// Plan times are read as decimal seconds and rounded to the nearest microsecond
/// (see [`Rational::from_secs_f64`]), so `0.15` means exactly 3/20 of a second rather
/// than the nearest binary float. All arithmetic is checked and returns `None`
/// instead of overflowing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    numer: i64,
    denom: i64,
}

impl Rational {
    /// Zero.
    pub const ZERO: Self = Self { numer: 0, denom: 1 };

    /// Builds `numer / denom` in lowest terms.
    ///
    /// Returns `None` when `denom` is zero or the reduced fraction does not fit in `i64`.
    pub fn new(numer: i64, denom: i64) -> Option<Self> {
        Self::from_i128(i128::from(numer), i128::from(denom))
    }

    /// The whole number `n`.
    pub const fn from_integer(n: i64) -> Self {
        Self { numer: n, denom: 1 }
    }

    /// Converts decimal seconds to an exact fraction, rounded to the nearest microsecond.
    ///
    /// Returns `None` for NaN, infinities, and values too large to count in microseconds.
    pub fn from_secs_f64(secs: f64) -> Option<Self> {
        let micros = (secs * 1e6).round();
        if micros.is_nan() || micros.abs() >= I64_LIMIT {
            return None;
        }
        // In range and already a whole number, so the cast is exact.
        Self::new(micros as i64, MICROS_PER_SECOND)
    }

    /// `numer / denom` for a positive denominator, which cannot fail.
    pub(crate) fn from_ratio(numer: i64, denom: NonZeroU32) -> Self {
        let denom = i64::from(denom.get());
        // The gcd divides `denom`, so it is in 1..=u32::MAX and the divisions are exact.
        let divisor = i64::try_from(gcd(
            u128::from(numer.unsigned_abs()),
            denom.unsigned_abs().into(),
        ))
        .unwrap_or(1);
        Self {
            numer: numer / divisor,
            denom: denom / divisor,
        }
    }

    /// The numerator, in lowest terms; carries the sign.
    pub const fn numer(self) -> i64 {
        self.numer
    }

    /// The denominator, in lowest terms; always positive.
    pub const fn denom(self) -> i64 {
        self.denom
    }

    /// `self + rhs`, or `None` on overflow.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let (a, b, c, d) = self.widen(rhs);
        Self::from_i128(
            a.checked_mul(d)?.checked_add(c.checked_mul(b)?)?,
            b.checked_mul(d)?,
        )
    }

    /// `self - rhs`, or `None` on overflow.
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        let (a, b, c, d) = self.widen(rhs);
        Self::from_i128(
            a.checked_mul(d)?.checked_sub(c.checked_mul(b)?)?,
            b.checked_mul(d)?,
        )
    }

    /// `self * rhs`, or `None` on overflow.
    pub fn checked_mul(self, rhs: Self) -> Option<Self> {
        let (a, b, c, d) = self.widen(rhs);
        Self::from_i128(a.checked_mul(c)?, b.checked_mul(d)?)
    }

    /// `self / rhs`, or `None` when `rhs` is zero or on overflow.
    pub fn checked_div(self, rhs: Self) -> Option<Self> {
        let (a, b, c, d) = self.widen(rhs);
        Self::from_i128(a.checked_mul(d)?, b.checked_mul(c)?)
    }

    /// The nearest `f64`, for display. Not exact.
    pub fn to_f64(self) -> f64 {
        self.numer as f64 / self.denom as f64
    }

    fn widen(self, rhs: Self) -> (i128, i128, i128, i128) {
        (
            i128::from(self.numer),
            i128::from(self.denom),
            i128::from(rhs.numer),
            i128::from(rhs.denom),
        )
    }

    fn from_i128(numer: i128, denom: i128) -> Option<Self> {
        if denom == 0 {
            return None;
        }
        let divisor = i128::try_from(gcd(numer.unsigned_abs(), denom.unsigned_abs())).ok()?;
        let (numer, denom) = (numer.checked_div(divisor)?, denom.checked_div(divisor)?);
        let (numer, denom) = if denom < 0 {
            (numer.checked_neg()?, denom.checked_neg()?)
        } else {
            (numer, denom)
        };
        Some(Self {
            numer: i64::try_from(numer).ok()?,
            denom: i64::try_from(denom).ok()?,
        })
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        // Denominators are positive, so cross-multiplying keeps the order,
        // and an i64 × i64 product always fits in i128.
        let lhs = i128::from(self.numer) * i128::from(other.denom);
        let rhs = i128::from(other.numer) * i128::from(self.denom);
        lhs.cmp(&rhs)
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.denom == 1 {
            write!(f, "{}", self.numer)
        } else {
            write!(f, "{}/{}", self.numer, self.denom)
        }
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(numer: i64, denom: i64) -> Rational {
        Rational::new(numer, denom).unwrap()
    }

    #[test]
    fn new_reduces_and_normalises_sign() {
        assert_eq!(r(6, 4), r(3, 2));
        assert_eq!((r(6, -4).numer(), r(6, -4).denom()), (-3, 2));
        assert_eq!((r(0, -7).numer(), r(0, -7).denom()), (0, 1));
    }

    #[test]
    fn new_rejects_zero_denominator_and_overflow() {
        assert_eq!(Rational::new(1, 0), None);
        // Negating i64::MIN does not fit in i64.
        assert_eq!(Rational::new(i64::MIN, -1), None);
    }

    #[test]
    fn from_secs_f64_is_exact_to_the_microsecond() {
        assert_eq!(Rational::from_secs_f64(0.15), Some(r(3, 20)));
        assert_eq!(Rational::from_secs_f64(3.12), Some(r(78, 25)));
        assert_eq!(Rational::from_secs_f64(-2.5), Some(r(-5, 2)));
        assert_eq!(Rational::from_secs_f64(0.000_000_4), Some(Rational::ZERO));
    }

    #[test]
    fn from_secs_f64_rejects_non_finite_and_huge_values() {
        assert_eq!(Rational::from_secs_f64(f64::NAN), None);
        assert_eq!(Rational::from_secs_f64(f64::INFINITY), None);
        assert_eq!(Rational::from_secs_f64(f64::NEG_INFINITY), None);
        assert_eq!(Rational::from_secs_f64(1e300), None);
        assert_eq!(Rational::from_secs_f64(-1e14), None);
    }

    #[test]
    fn from_ratio_reduces() {
        let thirty = NonZeroU32::new(30).unwrap();
        assert_eq!(Rational::from_ratio(68, thirty), r(34, 15));
        assert_eq!(Rational::from_ratio(-60, thirty), r(-2, 1));
        assert_eq!(Rational::from_ratio(i64::MIN, thirty), r(i64::MIN / 2, 15));
    }

    #[test]
    fn checked_arithmetic_is_exact() {
        assert_eq!(r(1, 3).checked_add(r(1, 6)), Some(r(1, 2)));
        assert_eq!(r(1, 3).checked_sub(r(1, 2)), Some(r(-1, 6)));
        assert_eq!(r(2, 3).checked_mul(r(9, 4)), Some(r(3, 2)));
        assert_eq!(r(2, 3).checked_div(r(-4, 9)), Some(r(-3, 2)));
    }

    #[test]
    fn checked_arithmetic_reports_overflow_and_division_by_zero() {
        let max = Rational::from_integer(i64::MAX);
        assert_eq!(max.checked_add(Rational::from_integer(1)), None);
        assert_eq!(max.checked_mul(Rational::from_integer(2)), None);
        assert_eq!(Rational::from_integer(i64::MIN).checked_sub(r(1, 1)), None);
        assert_eq!(r(1, 2).checked_div(Rational::ZERO), None);
    }

    #[test]
    fn ordering_is_exact() {
        assert!(r(1, 3) < r(334, 1000));
        assert!(r(-1, 2) < r(-1, 3));
        assert!(Rational::from_integer(i64::MAX) > r(i64::MAX - 1, 1));
        assert_eq!(r(2, 4).cmp(&r(1, 2)), Ordering::Equal);
    }

    #[test]
    fn display() {
        assert_eq!(r(4, 2).to_string(), "2");
        assert_eq!(r(-202, 30).to_string(), "-101/15");
    }
}
