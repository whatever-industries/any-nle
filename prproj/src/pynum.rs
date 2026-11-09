//! Python number semantics the CLIs and the exporter rely on.

/// Python `a / b` for ints: the correctly rounded quotient (not `a as f64 / b as f64`, which
/// rounds twice for operands beyond 2^53). `b` must be nonzero and both operands below 2^64 in
/// magnitude.
pub fn true_div(a: i128, b: i128) -> f64 {
    assert!(b != 0, "division by zero");
    let negative = (a < 0) != (b < 0);
    let (ua, ub) = (a.unsigned_abs(), b.unsigned_abs());
    let magnitude = if ua == 0 {
        0.0
    } else if ua < (1 << 53) && ub < (1 << 53) {
        ua as f64 / ub as f64
    } else {
        debug_assert!(ua < (1 << 64) && ub < (1 << 64));
        // Scale the dividend so the quotient keeps 62+ significant bits, fold the remainder
        // into a sticky low bit, and let the (round-to-nearest-even) u128 -> f64 conversion
        // round once.
        let bits = 128 - ua.leading_zeros();
        let shift = 126 - bits;
        let scaled = ua << shift;
        let (q, r) = (scaled / ub, scaled % ub);
        let q = if r != 0 { q | 1 } else { q };
        (q as f64) * 2f64.powi(-(shift as i32))
    };
    if negative {
        -magnitude
    } else {
        magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_quotients() {
        assert_eq!(true_div(1, 2), 0.5);
        assert_eq!(true_div(0, -5).to_bits(), (-0.0f64).to_bits());
        assert_eq!(true_div(254_016_000_000, 254_016_000_000), 1.0);
        assert_eq!(true_div(-3, 4), -0.75);
        // beyond 2^53: 2^60+1 over 2^60 is 1 + 2^-60, which rounds to 1.0
        assert_eq!(true_div((1 << 60) + 1, 1 << 60), 1.0);
        assert_eq!(true_div(9_007_199_254_740_993, 1), 9_007_199_254_740_992.0);
        assert_eq!(true_div(9_007_199_254_740_993, 3), 3_002_399_751_580_331.0);
    }
}
