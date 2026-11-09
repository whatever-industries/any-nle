//! Exact rational numbers for times (seconds) and rates (per second).
//!
//! The graph has no clock of its own: a time is a reduced fraction of a second, so 24 fps,
//! 30000/1001 fps, 48 kHz samples and Premiere's 254016000000 ticks per second all stay exact
//! (contract: "rational time values and declared timebases ... without floating-point rounding").
//! Written in JSON as `"num/den"`, or `"num"` when the denominator is 1.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Rational {
    pub const ZERO: Rational = Rational { num: 0, den: 1 };

    /// `num/den` reduced; `None` when `den` is 0 or the reduced value does not fit `i64`.
    pub fn new(num: i128, den: i128) -> Option<Rational> {
        if den == 0 {
            return None;
        }
        // Unsigned magnitudes also represent abs(i128::MIN). Signed remainder,
        // absolute value and sign negation would overflow for those inputs.
        let (n, d) = (num.unsigned_abs(), den.unsigned_abs());
        let g = gcd(n, d);
        let (n, d) = (n / g, d / g);
        let negative = (num < 0) != (den < 0);
        let num = if negative && n == 1u128 << 63 {
            i64::MIN
        } else {
            let n = i64::try_from(n).ok()?;
            if negative { -n } else { n }
        };
        Some(Rational { num, den: i64::try_from(d).ok()? })
    }

    pub fn integer(n: i64) -> Rational {
        Rational { num: n, den: 1 }
    }

    pub fn num(self) -> i64 {
        self.num
    }

    pub fn den(self) -> i64 {
        self.den
    }

    pub fn is_negative(self) -> bool {
        self.num < 0
    }

    /// `count` units of `per_second` units a second (e.g. ticks at 254016000000/s).
    pub fn from_units(count: i64, per_second: i64) -> Option<Rational> {
        Rational::new(count as i128, per_second as i128)
    }

    /// This time as a whole number of units at `per_second`; `None` when it falls between units.
    pub fn to_units(self, per_second: i64) -> Option<i64> {
        let scaled = self.num as i128 * per_second as i128;
        (scaled % self.den as i128 == 0).then(|| i64::try_from(scaled / self.den as i128).ok()).flatten()
    }

    /// `self + other`, exact; `None` if the result does not fit.
    pub fn checked_add(self, other: Rational) -> Option<Rational> {
        let den = (self.den as i128).checked_mul(other.den as i128)?;
        let left = (self.num as i128).checked_mul(other.den as i128)?;
        let right = (other.num as i128).checked_mul(self.den as i128)?;
        Rational::new(left.checked_add(right)?, den)
    }

    /// `self - other`, exact; `None` if the result does not fit.
    pub fn checked_sub(self, other: Rational) -> Option<Rational> {
        // Widen before subtracting: -i64::MIN does not fit i64, even when the
        // final reduced result (for example MIN - MIN) is representable.
        let den = (self.den as i128).checked_mul(other.den as i128)?;
        let left = (self.num as i128).checked_mul(other.den as i128)?;
        let right = (other.num as i128).checked_mul(self.den as i128)?;
        Rational::new(left.checked_sub(right)?, den)
    }

    pub fn parse(text: &str) -> Option<Rational> {
        let (n, d) = text.split_once('/').unwrap_or((text, "1"));
        let int = |s: &str| -> Option<i128> {
            let digits = s.strip_prefix('-').unwrap_or(s);
            let canonical = !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) && (digits == "0" || !digits.starts_with('0'));
            canonical.then(|| s.parse().ok()).flatten()
        };
        let (n, d) = (int(n)?, int(d)?);
        if d <= 0 {
            return None;
        }
        let r = Rational::new(n, d)?;
        // Only the reduced spelling is accepted, so a parsed value writes back as the same text.
        (r.to_string() == text).then_some(r)
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Denominators are positive, so cross-multiplying keeps the order.
        (self.num as i128 * other.den as i128).cmp(&(other.num as i128 * self.den as i128))
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_ticks() {
        let frame = Rational::from_units(254016000000 * 1001 / 30000, 254016000000).unwrap();
        assert_eq!(frame.to_string(), "1001/30000");
        assert_eq!(frame.to_units(254016000000), Some(8475667200));
        assert_eq!(frame.to_units(24), None);
        let big = Rational::from_units(1 << 62, 254016000000).unwrap();
        assert_eq!(big.to_units(254016000000), Some(1 << 62));
    }

    #[test]
    fn parse_only_reduced_text() {
        assert_eq!(Rational::parse("30000/1001"), Rational::new(30000, 1001));
        assert_eq!(Rational::parse("-3/2"), Rational::new(-3, 2));
        for bad in ["2/4", "3/1", "1/0", "1/-2", "01", "+1", "", "/", "1/", "a"] {
            assert_eq!(Rational::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn minimum_magnitudes_reduce_without_signed_overflow() {
        assert_eq!(Rational::new(i128::MIN, i128::MIN), Some(Rational::integer(1)));
        assert_eq!(Rational::new(0, i128::MIN), Some(Rational::ZERO));
        assert_eq!(Rational::new(i128::MIN, 1i128 << 64), Some(Rational::integer(i64::MIN)));
        assert_eq!(Rational::new(i128::MIN, -(1i128 << 64)), None);
        assert_eq!(Rational::new(i128::MIN, -1), None);
        assert_eq!(Rational::new(1, i128::MIN), None);
        assert_eq!(Rational::new(i128::MAX, i128::MAX), Some(Rational::integer(1)));
        assert_eq!(Rational::new(i64::MAX as i128 * 2, 2), Some(Rational::integer(i64::MAX)));
        let inputs = [i128::MIN, i128::MIN+1, i64::MIN as i128, -1, 0, 1, i64::MAX as i128, i128::MAX];
        for n in inputs {
            for d in inputs {
                let actual = std::panic::catch_unwind(|| Rational::new(n,d));
                assert!(actual.is_ok(), "constructor panicked for {n}/{d}");
                if let Some(value) = actual.unwrap() {
                    assert!(value.den() > 0);
                    assert_eq!(Rational::parse(&value.to_string()), Some(value));
                }
            }
        }
    }

    #[test]
    fn checked_integer_arithmetic_matches_independent_integer_operations() {
        let inputs = [i64::MIN, i64::MIN+1, -1, 0, 1, i64::MAX-1, i64::MAX];
        for a in inputs {
            for b in inputs {
                let (a_r,b_r) = (Rational::integer(a),Rational::integer(b));
                assert_eq!(a_r.checked_add(b_r), a.checked_add(b).map(Rational::integer));
                assert_eq!(a_r.checked_sub(b_r), a.checked_sub(b).map(Rational::integer));
            }
        }
        assert_eq!(Rational::integer(i64::MIN).checked_sub(Rational::integer(i64::MIN)), Some(Rational::ZERO));
        for (n,d) in [(i64::MIN,3),(i64::MAX,i64::MAX-1),(-1,i64::MAX)] {
            let a = Rational::new(n as i128,d as i128).unwrap();
            assert_eq!(a.checked_sub(a),Some(Rational::ZERO));
        }
        assert_eq!(Rational::integer(i64::MIN).to_units(-1),None);
    }
}
