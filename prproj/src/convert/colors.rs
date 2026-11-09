//! Port of `converter/src/avid2prproj/colors.py`: Avid clip and marker colours -> Premiere label
//! and marker colours.
//!
//! The nearest-label search uses floating point (cube roots, `atan2`, `hypot`, `sqrt`, `sin`).
//! Python's `**` and `math.*` call the C library (`pow`, `atan2`, `sin`, `sqrt`), so the same
//! calls are made here; `math.hypot` is CPython's own compensated algorithm, ported as
//! [`py_hypot`] because the platform `hypot` differs from it in the last bit for ~15% of inputs.

/// Python float `**` = C `pow`. Both operands pass through `black_box` so LLVM cannot rewrite
/// `pow(x, 2.0)` into `x * x` (the platform `pow` is not always correctly rounded, so that
/// changes bits).
pub fn py_pow(x: f64, y: f64) -> f64 {
    crate::build::fmt::py_pow(std::hint::black_box(x), std::hint::black_box(y))
}

pub const LABEL_NAMES: [&str; 16] = [
    "Violet", "Iris", "Caribbean", "Lavender", "Cerulean", "Forest", "Rose", "Mango", "Purple", "Blue", "Teal", "Magenta",
    "Tan", "Green", "Brown", "Yellow",
];
pub const LABEL_PACKED: [i64; 16] = [
    11405886, 6769408, 480554, 8851829, 5985541, 19005, 3474060, 277129, 11993185, 10104082, 4541953, 5770628, 4545135, 2579725,
    408413, 1664623,
];

pub const MARKER_NAMES: [&str; 8] = ["Green", "Red", "Magenta", "Orange", "Yellow", "White", "Blue", "Cyan"];

/// Avid marker colour name -> Premiere marker colour index (User decisions 2026-10-15).
pub const AVID_MARKER_DEFAULTS: [(&str, i64); 16] = [
    ("Red", 1),
    ("Green", 0),
    ("Blue", 6),
    ("Cyan", 7),
    ("Magenta", 2),
    ("Yellow", 4),
    ("White", 5),
    ("Orange", 3),
    ("Pink", 2),
    ("Forest", 0),
    ("Denim", 6),
    ("Gold", 4),
    ("Violet", 2),
    ("Purple", 6),
    ("Black", 5),
    ("Grey", 5),
];

/// `unpack_bgr`: `(r, g, b)` of a packed `0xBBGGRR` value.
pub fn unpack_bgr(packed: i64) -> (i64, i64, i64) {
    (packed & 255, (packed >> 8) & 255, (packed >> 16) & 255)
}

// --- Python float helpers -----------------------------------------------------------------------

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    (x, (a - x) + b)
}

/// The exponent `e` of C `frexp(x, &e)` for a finite positive `x`.
fn frexp_exp(x: f64) -> i32 {
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32;
    if e == 0 {
        let mantissa = bits & ((1u64 << 52) - 1);
        -1022 - (mantissa.leading_zeros() as i32 - 11)
    } else {
        e - 1022
    }
}

fn vector_norm(v: &mut [f64; 2], max: f64) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if v.iter().any(|x| x.is_nan()) {
        return f64::NAN;
    }
    if max == 0.0 {
        return max;
    }
    let max_e = frexp_exp(max);
    if max_e < -1023 {
        for x in v.iter_mut() {
            *x /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(v, max / f64::MIN_POSITIVE);
    }
    let scale = f64::from_bits(((1023 - max_e) as u64) << 52);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for &x in v.iter() {
        let x = x * scale;
        let (hi, lo) = dl_mul(x, x);
        let (sum, err) = dl_fast_sum(csum, hi);
        csum = sum;
        frac1 += lo;
        frac2 += err;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (hi, lo) = dl_mul(-h, h);
    let (sum, err) = dl_fast_sum(csum, hi);
    csum = sum;
    frac1 += lo;
    frac2 += err;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

/// Python `math.hypot(a, b)` (CPython 3.10+ `vector_norm`), bit for bit.
pub fn py_hypot(a: f64, b: f64) -> f64 {
    let mut v = [a.abs(), b.abs()];
    let max = if v[1] > v[0] { v[1] } else { v[0] };
    vector_norm(&mut v, max)
}

/// Python `x % y` for floats (`y` nonzero): the result takes the sign of `y`.
pub fn py_fmod(x: f64, y: f64) -> f64 {
    let m = x % y;
    if m != 0.0 {
        if (y < 0.0) != (m < 0.0) {
            m + y
        } else {
            m
        }
    } else {
        0.0f64.copysign(y)
    }
}

/// Python `math.degrees`: `x * (180 / pi)`.
pub fn py_degrees(x: f64) -> f64 {
    x * (180.0 / std::f64::consts::PI)
}

/// Python `math.radians`: `x * (pi / 180)`.
pub fn py_radians(x: f64) -> f64 {
    x * (std::f64::consts::PI / 180.0)
}

/// Python `round(x)` for a finite float: nearest, ties to even.
fn py_round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

type Lab = (f64, f64, f64);

fn lin(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        py_pow((c + 0.055) / 1.055, 2.4)
    }
}

fn lab_f(t: f64) -> f64 {
    if t > 0.008856 {
        py_pow(t, 1.0 / 3.0)
    } else {
        7.787 * t + 16.0 / 116.0
    }
}

fn lab(rgb: (i64, i64, i64)) -> Lab {
    let (r, g, b) = (lin(rgb.0 as f64), lin(rgb.1 as f64), lin(rgb.2 as f64));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    (116.0 * lab_f(y) - 16.0, 500.0 * (lab_f(x) - lab_f(y)), 200.0 * (lab_f(y) - lab_f(z)))
}

/// `_distance`: hue first, lightness weakly (Avid's swatches are light, Premiere's labels dark).
pub fn distance(p: (i64, i64, i64), q: (i64, i64, i64)) -> f64 {
    let ((l1, a1, b1), (l2, a2, b2)) = (lab(p), lab(q));
    let (c1, c2) = (py_hypot(a1, b1), py_hypot(a2, b2));
    let (h1, h2) = (py_fmod(py_degrees(b1.atan2(a1)), 360.0), py_fmod(py_degrees(b2.atan2(a2)), 360.0));
    let diff = (h1 - h2).abs();
    let other = 360.0 - diff;
    let dh = if other < diff { other } else { diff };
    let d_hue = 2.0 * (c1 * c2).sqrt() * (py_radians(dh) / 2.0).sin();
    ((py_pow(0.25 * (l1 - l2), 2.0) + 0.5 * py_pow(c1 - c2, 2.0)) + py_pow(d_hue, 2.0)).sqrt()
}

/// `rgb16_hex`: `#RRGGBB` of a 16-bit-per-channel colour (`round(v / 257)` per channel).
pub fn rgb16_hex(rgb16: [i64; 3]) -> String {
    let mut out = String::from("#");
    for v in rgb16 {
        let r = py_round(v as f64 / 257.0);
        // Python `f"{r:02X}"`: a sign, then at least two digits.
        if r < 0 {
            out.push_str(&format!("-{:02X}", r.unsigned_abs()));
        } else {
            out.push_str(&format!("{r:02X}"));
        }
    }
    out
}

/// Python's `dict.get`/`in` over a dict built from `(key, value)` pairs: later duplicates win.
fn lookup<'a>(overrides: &'a [(String, i64)], key: &str) -> Option<&'a i64> {
    overrides.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// `nearest_label`: the Premiere label index for an Avid clip colour. `overrides` maps
/// `"#RRGGBB"` to a label index (the import's colour mapper).
pub fn nearest_label(rgb16: [i64; 3], overrides: &[(String, i64)]) -> i64 {
    let key = rgb16_hex(rgb16);
    if let Some(&index) = lookup(overrides, &key) {
        return index;
    }
    let rgb = (py_round(rgb16[0] as f64 / 257.0), py_round(rgb16[1] as f64 / 257.0), py_round(rgb16[2] as f64 / 257.0));
    let mut best = 0usize;
    let mut best_distance = distance(rgb, unpack_bgr(LABEL_PACKED[0]));
    for (i, packed) in LABEL_PACKED.iter().enumerate().skip(1) {
        let d = distance(rgb, unpack_bgr(*packed));
        if d < best_distance {
            best = i;
            best_distance = d;
        }
    }
    best as i64
}

/// `marker_color`: the Premiere marker colour index for an Avid marker colour name; `None` =
/// unknown name.
pub fn marker_color(avid_name: &str, overrides: &[(String, i64)]) -> Option<i64> {
    if let Some(&index) = lookup(overrides, avid_name) {
        return Some(index);
    }
    AVID_MARKER_DEFAULTS.iter().find(|(name, _)| *name == avid_name).map(|(_, index)| *index)
}
