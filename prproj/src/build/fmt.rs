//! Number formatters (port of the writer-side formatting in `models/component.py` lines
//! ~85-125 and `models/track_item.py` lines ~116-127, plus Python's `repr(float)`, `%g` and
//! `%f` which Rust does not have).
//!
//! Everything works on the exact decimal expansion Rust's `{:e}` / `{:.N}` produce (both are
//! correctly rounded from the binary value, ties to even, like CPython's `dtoa`), so the output
//! matches Python digit for digit; `tests/build_multicam_marker.rs` checks thousands of values
//! against tables Python wrote.

use crate::error::{value, Result};

/// Split Rust `{:e}` output (`d.ddde-5`) into `(digits, exponent)`; `digits` has no point.
fn sci_parts(s: &str) -> (String, i32) {
    let (mantissa, exp) = s.split_once('e').expect("exponent form");
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits, exp.parse().expect("exponent"))
}

fn non_finite(v: f64) -> &'static str {
    if v.is_nan() {
        "nan"
    } else if v > 0.0 {
        "inf"
    } else {
        "-inf"
    }
}

/// The digits and decimal exponent of the shortest string that round-trips to `v` (> 0), as
/// CPython's `dtoa` mode 0 picks them. Rust's shortest-digits output has the same length but
/// breaks exact ties upward where `dtoa` breaks them to even (`30.3639068603515625` is
/// `...562` in Python, `...563` in Rust), so the correctly rounded string of that length is
/// preferred whenever it round-trips.
fn shortest_digits(v: f64) -> (String, i32) {
    let (digits, exp) = sci_parts(&format!("{:e}", v));
    let exact = format!("{:.*e}", digits.len() - 1, v);
    if exact.parse::<f64>() == Ok(v) {
        return sci_parts(&exact);
    }
    (digits, exp)
}

/// Python `float.__repr__` / `str(float)`: the shortest string that round-trips (`1e-05`,
/// `1e+16`, `0.1`, `100.0`, `inf`, `nan`).
pub fn py_repr(v: f64) -> String {
    if !v.is_finite() {
        return non_finite(v).to_string();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let (digits, exp) = shortest_digits(v.abs());
    let decpt = exp + 1;
    let n = digits.len() as i32;
    let body = if decpt <= -4 || decpt > 16 {
        let mut s = digits[..1].to_string();
        if n > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        s.push_str(&format!("e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs()));
        s
    } else if decpt <= 0 {
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt >= n {
        format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
    } else {
        format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    };
    format!("{sign}{body}")
}

/// Python `"%.{precision}g" % v` (also `f"{v:.{precision}g}"`).
pub fn fmt_g(v: f64, precision: usize) -> String {
    if !v.is_finite() {
        return non_finite(v).to_string();
    }
    let p = precision.max(1);
    if v == 0.0 {
        return if v.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    let sci = format!("{:.*e}", p - 1, v);
    let (mantissa, exp) = sci.split_once('e').expect("exponent form");
    let exp: i32 = exp.parse().expect("exponent");
    if exp < -4 || exp >= p as i32 {
        let mut m = mantissa.to_string();
        if m.contains('.') {
            m = m.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else {
        let mut s = format!("{:.*}", (p as i32 - 1 - exp) as usize, v);
        if s.contains('.') {
            s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        }
        s
    }
}

/// Python `"%.{decimals}f" % v`.
pub fn fmt_f(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return non_finite(v).to_string();
    }
    format!("{:.*}", decimals, v)
}

/// Python `str(int(v))` for a finite float (truncates toward zero; exact for any magnitude).
pub fn int_str(v: f64) -> Result<String> {
    if v.is_nan() {
        return value("cannot convert float NaN to integer");
    }
    if v.is_infinite() {
        return value("cannot convert float infinity to integer");
    }
    let s = format!("{:.0}", v.trunc());
    Ok(if s == "-0" { "0".to_string() } else { s })
}

/// Python `v == int(v)` (finite floats only; non-finite values raise in Python).
fn is_integral(v: f64) -> Result<bool> {
    int_str(v)?;
    Ok(v == v.trunc())
}

/// `component._float32`: round a double to float32 and back. Python's `struct.pack("f")`
/// raises `OverflowError` for values beyond float32 range; so does this.
pub fn float32(v: f64) -> Result<f64> {
    let f = v as f32;
    if f.is_infinite() && v.is_finite() {
        return value("float too large to pack with f format");
    }
    Ok(f as f64)
}

/// The "never exponent form" fallback shared by `_format_scalar` and `_format_current_value`.
fn plain_decimal(v: f64, significant: i32) -> String {
    let exponent = v.abs().log10().floor() as i32;
    let decimals = (significant - 1 - exponent).max(0) as usize;
    let plain = format!("{:.*}", decimals, v);
    let plain = plain.trim_end_matches('0').to_string();
    if plain.ends_with('.') {
        plain + "0"
    } else {
        plain
    }
}

/// `component._format_scalar`: float32 value, integers with a trailing dot (`100.`), others at
/// 12 significant figures with `%g`'s zero stripping, never in exponent form.
pub fn format_scalar(v: f64) -> Result<String> {
    let f = float32(v)?;
    if is_integral(f)? {
        return Ok(format!("{}.", int_str(f)?));
    }
    let text = fmt_g(f, 12);
    if !text.contains('e') && !text.contains('E') {
        return Ok(text);
    }
    Ok(plain_decimal(f, 12))
}

/// `component._format_current_value`: the float32 value at 17 significant figures (`0` for zero).
pub fn format_current_value(v: f64) -> Result<String> {
    let f = float32(v)?;
    if f == 0.0 {
        return Ok("0".to_string());
    }
    let text = fmt_g(f, 17);
    if !text.contains('e') && !text.contains('E') {
        return Ok(text);
    }
    Ok(plain_decimal(f, 17))
}

/// `component._format_tangent`: integers plain (`50`, `0`), fractions at 17 significant figures.
pub fn format_tangent(v: f64) -> Result<String> {
    if is_integral(v)? {
        int_str(v)
    } else {
        Ok(fmt_g(v, 17))
    }
}

/// `track_item._format_remap_value`: source seconds at 24 fixed decimals.
pub fn format_remap_value(v: f64) -> String {
    fmt_f(v, 24)
}

/// `track_item._format_remap_bound`: integral bounds as bare integers, others via `repr`.
pub fn format_remap_bound(v: f64) -> Result<String> {
    if is_integral(v)? {
        int_str(v)
    } else {
        Ok(py_repr(v))
    }
}

/// `avid2prproj/lumetri.py` line 72: `f"{int(value)}."` when integral, else `repr(float(value))`.
pub fn format_lumetri_value(v: f64) -> Result<String> {
    if v.is_finite() && v == v.trunc() {
        Ok(format!("{}.", int_str(v)?))
    } else {
        Ok(py_repr(v))
    }
}

/// `repr(10 ** (db / 20.0))` as `opatom._set_master_gain` writes it.
pub fn db_to_gain_repr(db: f64) -> String {
    py_repr(py_pow(10.0, db / 20.0))
}

extern "C" {
    fn pow(x: f64, y: f64) -> f64;
}

/// Python's `x ** y` for floats: CPython calls the C library `pow`. `f64::powf` with a constant
/// base 10 is rewritten by LLVM in release builds into `exp10`, which can differ in the last bit
/// (`10 ** (-8/20)`: pow 0.3981071705534972, exp10 0.39810717055349726).
pub fn py_pow(x: f64, y: f64) -> f64 {
    // `black_box` keeps LLVM from recognising `pow(10, y)` and turning even this libcall into
    // `exp10`. SAFETY: `pow` from the platform C library is pure and thread-safe.
    unsafe { pow(std::hint::black_box(x), y) }
}

/// `repr(float(speed))` as `writer.set_playback_speed` writes it.
pub fn speed_repr(speed: f64) -> String {
    py_repr(speed)
}
