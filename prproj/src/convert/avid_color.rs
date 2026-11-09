//! Port of `converter/src/avid2prproj/avid_color.py`: how Avid's Color Correction renders, as far
//! as it has been measured (PROVISIONAL formulas, see the Python docstring), for conversion.
//!
//! Every float operation follows the Python expression's evaluation order, and `**` / `math.*`
//! call the same C library functions, so the results equal Python's bit for bit (checked by
//! `tests/convert_markers_color.rs` against `cube_lut` and curve dumps).

use super::colors::py_radians;
use super::colors::py_pow;
use crate::error::{value, Result};

pub const LEGAL_LOW: f64 = 16.0;
pub const LEGAL_RANGE: f64 = 219.0;
pub const CONTRAST_PIVOT: f64 = 0.507;
pub const CONTRAST_DEG_PER_UNIT: f64 = 0.45;
pub const BRIGHTNESS_CODES_PER_UNIT: f64 = 35.0 / 30.0;
/// Lumetri stores up to 32 points per curve.
pub const SAMPLES: usize = 32;

pub const KR: f64 = 0.2126;
pub const KG: f64 = 0.7152;
pub const KB: f64 = 0.0722;

/// Python `dict[str, float]` (insertion order kept, keys unique): HSL tone values.
pub type Tone = [(String, f64)];
/// Python `dict[str, list[tuple[float, float]]]`: curve name -> points.
pub type Curves = [(String, Vec<(f64, f64)>)];
/// A function on one normalized channel value.
pub type Transfer = Box<dyn Fn(f64) -> f64>;
/// A function on normalized `(r, g, b)`.
pub type RgbTransform = Box<dyn Fn(f64, f64, f64) -> (f64, f64, f64)>;

fn tone_get(tone: &Tone, key: &str) -> Option<f64> {
    tone.iter().rev().find(|(k, _)| k == key).map(|(_, v)| *v)
}

fn curve_get<'a>(curves: &'a Curves, key: &str) -> Option<&'a [(f64, f64)]> {
    curves.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
}

/// Python `min(a, b)`.
fn pmin(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// Python `max(a, b)`.
fn pmax(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

fn clip(v: f64) -> f64 {
    pmin(pmax(v, 0.0), 1.0)
}

/// `tone_transfer`: Avid HSL Controls tone values -> a function on 0..1 (full-range normalized).
pub fn tone_transfer(tone: &Tone) -> Transfer {
    let setup = tone_get(tone, "setup");
    let brightness = tone_get(tone, "brightness");
    let slope = tone_get(tone, "contrast").map(|c| {
        let angle = py_radians(45.0 + CONTRAST_DEG_PER_UNIT * pmax(pmin(c, 99.0), -99.0));
        angle.tan()
    });
    let gain = tone_get(tone, "gain");
    let gamma = tone_get(tone, "gamma");
    Box::new(move |mut x: f64| {
        if let Some(s) = setup {
            x = clip(x + s / LEGAL_RANGE);
        }
        if let Some(b) = brightness {
            x = clip(x + b * BRIGHTNESS_CODES_PER_UNIT / LEGAL_RANGE);
        }
        if let Some(k) = slope {
            x = clip(CONTRAST_PIVOT + (x - CONTRAST_PIVOT) * k);
        }
        if let Some(g) = gain {
            x = clip(x * g / 100.0);
        }
        if let Some(g) = gamma {
            x = if g > 0.0 { py_pow(clip(x), 1.0 / g) } else { x };
        }
        x
    })
}

fn index_error<T>() -> Result<T> {
    value("IndexError: list index out of range")
}

fn div(a: f64, b: f64) -> Result<f64> {
    if b == 0.0 {
        return value("ZeroDivisionError: float division by zero");
    }
    Ok(a / b)
}

/// `_hermite_slopes`: Catmull-Rom-style secant slopes (interior `(y[i+1]-y[i-1])/(x[i+1]-x[i-1])`,
/// ends the neighbouring secant).
fn hermite_slopes(xs: &[f64], ys: &[f64]) -> Result<Vec<f64>> {
    let n = xs.len();
    if n < 2 {
        return index_error();
    }
    if n == 2 {
        let s = div(ys[1] - ys[0], xs[1] - xs[0])?;
        return Ok(vec![s, s]);
    }
    let mut m = Vec::with_capacity(n);
    m.push(div(ys[1] - ys[0], xs[1] - xs[0])?);
    for i in 1..n - 1 {
        m.push(div(ys[i + 1] - ys[i - 1], xs[i + 1] - xs[i - 1])?);
    }
    m.push(div(ys[n - 1] - ys[n - 2], xs[n - 1] - xs[n - 2])?);
    Ok(m)
}

/// `curve_function`: an Avid curve (points in 8-bit UI units, as typed) -> its curve `C(u)`
/// on 0..255. Fewer than two points or a repeated end x is an error, as in Python.
pub fn curve_function(points_ui8: &[(f64, f64)]) -> Result<Transfer> {
    let mut pts: Vec<(f64, f64)> = points_ui8.to_vec();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if pts.is_empty() {
        return index_error();
    }
    let xs: Vec<f64> = pts.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
    let m = hermite_slopes(&xs, &ys)?;
    Ok(Box::new(move |u: f64| {
        if u <= xs[0] {
            return ys[0];
        }
        if u >= xs[xs.len() - 1] {
            return ys[ys.len() - 1];
        }
        let i = (0..xs.len() - 1).filter(|&j| xs[j] <= u).max().unwrap_or(0);
        let h = xs[i + 1] - xs[i];
        let t = (u - xs[i]) / h;
        let t3 = py_pow(t, 3.0);
        let t2 = py_pow(t, 2.0);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * ys[i] + h10 * h * m[i] + h01 * ys[i + 1] + h11 * h * m[i + 1]
    }))
}

/// `curve_transfer`: an Avid curve as it acts on full-range normalized values (legal-domain
/// evaluation).
pub fn curve_transfer(points_ui8: &[(f64, f64)]) -> Result<Transfer> {
    let c = curve_function(points_ui8)?;
    Ok(Box::new(move |x: f64| clip((c(LEGAL_LOW + LEGAL_RANGE * x) - LEGAL_LOW) / LEGAL_RANGE)))
}

/// `sample`: 32 evenly spaced `(x, f(x))` points on 0..1.
pub fn sample(f: &dyn Fn(f64) -> f64) -> Vec<(f64, f64)> {
    (0..SAMPLES)
        .map(|k| {
            let x = k as f64 / (SAMPLES - 1) as f64;
            (x, f(x))
        })
        .collect()
}

/// `lumetri_curves`: Lumetri RGB Curves (0..1 points) reproducing the modelled Avid result, per
/// channel: channel curve after master curve after the HSL tone controls. Lumetri's white
/// (master) curve is left alone. Returns the `red`/`green`/`blue` entries (empty when nothing
/// changes), in that order.
pub fn lumetri_curves(tone: &Tone, curves_ui8: &Curves) -> Result<Vec<(String, Vec<(f64, f64)>)>> {
    let tone_f = if tone.is_empty() { None } else { Some(tone_transfer(tone)) };
    let master_c = match curve_get(curves_ui8, "master") {
        Some(p) => Some(curve_transfer(p)?),
        None => None,
    };
    let mut out = Vec::new();
    for name in ["red", "green", "blue"] {
        let channel_c = match curve_get(curves_ui8, name) {
            Some(p) => Some(curve_transfer(p)?),
            None => None,
        };
        if tone_f.is_none() && master_c.is_none() && channel_c.is_none() {
            continue;
        }
        let steps: Vec<&Transfer> = [tone_f.as_ref(), master_c.as_ref(), channel_c.as_ref()].into_iter().flatten().collect();
        let run = |mut x: f64| {
            for f in &steps {
                x = f(x);
            }
            x
        };
        out.push((name.to_string(), sample(&run)));
    }
    Ok(out)
}

/// `hue_saturation`: hue rotation of Rec.709 chroma, then saturation about Rec.709 luma.
pub fn hue_saturation(hue_deg: f64, saturation: f64) -> RgbTransform {
    let th = -py_radians(hue_deg);
    let (c, s_) = (th.cos(), th.sin());
    let sat = saturation / 100.0;
    Box::new(move |r, g, b| {
        let y = KR * r + KG * g + KB * b;
        let (cb, cr) = ((b - y) / (2.0 * (1.0 - KB)), (r - y) / (2.0 * (1.0 - KR)));
        let (u, v) = (cb * c - cr * s_, cb * s_ + cr * c);
        let (rr, bb) = (y + 2.0 * (1.0 - KR) * v, y + 2.0 * (1.0 - KB) * u);
        let gg = (y - KR * rr - KB * bb) / KG;
        (clip(y + sat * (rr - y)), clip(y + sat * (gg - y)), clip(y + sat * (bb - y)))
    })
}

/// `rgb_transform`: the modelled Avid Color Correction as a function on full-range normalized RGB.
pub fn rgb_transform(tone: &Tone, saturation: Option<f64>, curves_ui8: &Curves) -> Result<RgbTransform> {
    let hue = tone_get(tone, "hue").unwrap_or(0.0);
    let rest: Vec<(String, f64)> = tone.iter().filter(|(k, _)| k != "hue").cloned().collect();
    // `hue or saturation not in (None, 100.0)`
    let hs = if hue != 0.0 || matches!(saturation, Some(s) if s != 100.0) {
        Some(hue_saturation(hue, saturation.unwrap_or(100.0)))
    } else {
        None
    };
    let tone_f = if rest.is_empty() { None } else { Some(tone_transfer(&rest)) };
    let master_c = match curve_get(curves_ui8, "master") {
        Some(p) => Some(curve_transfer(p)?),
        None => None,
    };
    let mut chans: Vec<Option<Transfer>> = Vec::with_capacity(3);
    for name in ["red", "green", "blue"] {
        chans.push(match curve_get(curves_ui8, name) {
            Some(p) => Some(curve_transfer(p)?),
            None => None,
        });
    }
    Ok(Box::new(move |mut r, mut g, mut b| {
        if let Some(hs) = &hs {
            (r, g, b) = hs(r, g, b);
        }
        let mut out = [0.0; 3];
        for (slot, (mut x, ch)) in out.iter_mut().zip([r, g, b].into_iter().zip(chans.iter())) {
            for f in [tone_f.as_ref(), master_c.as_ref(), ch.as_ref()].into_iter().flatten() {
                x = f(x);
            }
            *slot = x;
        }
        (out[0], out[1], out[2])
    }))
}

/// `is_identity`: no tone, saturation at most 100, no curves.
pub fn is_identity(tone: &Tone, saturation: Option<f64>, curves_ui8: &Curves) -> bool {
    tone.is_empty() && (saturation.is_none() || matches!(saturation, Some(s) if s == 100.0)) && curves_ui8.is_empty()
}

/// `cube_lut`: `size^3` output triplets, red fastest (the `.cube` and Premiere ITX3 order).
pub fn cube_lut(f: &dyn Fn(f64, f64, f64) -> (f64, f64, f64), size: i64) -> Result<Vec<(f64, f64, f64)>> {
    if size == 1 {
        return value("ZeroDivisionError: float division by zero");
    }
    let step = 1.0 / (size - 1) as f64;
    let mut out = Vec::new();
    for b in 0..size.max(0) {
        for g in 0..size {
            for r in 0..size {
                out.push(f(r as f64 * step, g as f64 * step, b as f64 * step));
            }
        }
    }
    Ok(out)
}
