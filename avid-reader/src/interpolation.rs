//! Numerical routines ported from pyavb `src/avb/interpolation.py` at
//! ff64e06b60077ecabe5db3fef239898b08363898 (MIT; see NOTICE and
//! LICENSE-pyavb). These reproduce pyavb calculations, not Avid rendering.
//!
//! The upstream module contains several exceptional edge paths (negative
//! quadratic discriminant, zero-width intervals, undefined `p`/`t`, and no
//! Bézier root). This module makes them explicit errors. No extrapolation or
//! curve interpretation beyond the upstream routine is implied.

use std::f64::consts::PI;

pub const EPSILON: f64 = 1e-10;
pub const MAX_INTEGRATION_STEPS: usize = 100_000;
pub type Point = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpolationError {
    NonFiniteInput,
    NonFiniteResult,
    DegenerateInterval,
    NegativeDiscriminant,
    NoBezierRoot,
    InvalidStepCount,
    TooManySteps,
    UndefinedUpstreamCase,
}

fn finite(values: &[f64]) -> Result<(), InterpolationError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(InterpolationError::NonFiniteInput)
    }
}

fn output(value: f64) -> Result<f64, InterpolationError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(InterpolationError::NonFiniteResult)
    }
}

pub fn lerp(a: f64, b: f64, t: f64) -> Result<f64, InterpolationError> {
    finite(&[a, b, t])?;
    output(a + (b - a) * t)
}

pub fn cubic_bezier(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> Result<f64, InterpolationError> {
    finite(&[p0, p1, p2, p3, t])?;
    let u = 1.0 - t;
    let w1 = u * u * u;
    let w2 = 3.0 * u * u * t;
    let w3 = 3.0 * u * t * t;
    let w4 = t * t * t;
    output(w1 * p0 + w2 * p1 + w3 * p2 + w4 * p3)
}

pub fn valid_root(v: f64) -> bool {
    (-EPSILON..=1.0 + EPSILON).contains(&v)
}

pub fn cube_root(x: f64) -> Result<f64, InterpolationError> {
    finite(&[x])?;
    if x < 0.0 {
        Ok(-(-x).powf(1.0 / 3.0))
    } else {
        Ok(x.powf(1.0 / 3.0))
    }
}

/// Return roots in pyavb's original order. Roots within EPSILON of [0,1]
/// remain slightly out of range here; callers may clamp when evaluating.
pub fn bezier_cubic_roots(
    pa: f64,
    pb: f64,
    pc: f64,
    pd: f64,
) -> Result<Vec<f64>, InterpolationError> {
    finite(&[pa, pb, pc, pd])?;
    let mut a = 3.0 * pa - 6.0 * pb + 3.0 * pc;
    let mut b = -3.0 * pa + 3.0 * pb;
    let mut c = pa;
    let d = -pa + 3.0 * pb - 3.0 * pc + pd;
    if ![a, b, c, d].iter().all(|v| v.is_finite()) {
        return Err(InterpolationError::NonFiniteResult);
    }
    let mut result = Vec::with_capacity(3);
    if d.abs() < EPSILON {
        if a.abs() < EPSILON {
            if b.abs() < EPSILON {
                return Ok(result);
            }
            let root = -c / b;
            if valid_root(root) {
                result.push(root);
            }
            return Ok(result);
        }
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            return Err(InterpolationError::NegativeDiscriminant);
        }
        if !discriminant.is_finite() {
            return Err(InterpolationError::NonFiniteResult);
        }
        let q = discriminant.sqrt();
        let a2 = 2.0 * a;
        let root = (q - b) / a2;
        if valid_root(root) {
            result.push(root);
        }
        let root = (-b - q) / a2;
        if valid_root(root) {
            result.push(root);
        }
        return Ok(result);
    }
    a /= d;
    b /= d;
    c /= d;
    let p = (3.0 * b - a * a) / 3.0;
    let p3 = p / 3.0;
    let q = (2.0 * a * a * a - 9.0 * a * b + 27.0 * c) / 27.0;
    let q2 = q / 2.0;
    let discriminant = q2 * q2 + p3 * p3 * p3;
    if ![p, p3, q, q2, discriminant].iter().all(|v| v.is_finite()) {
        return Err(InterpolationError::NonFiniteResult);
    }
    if discriminant < 0.0 {
        let mp3 = -p / 3.0;
        let mp33 = mp3 * mp3 * mp3;
        if !mp33.is_finite() {
            return Err(InterpolationError::NonFiniteResult);
        }
        let r = mp33.sqrt();
        if r == 0.0 {
            return Err(InterpolationError::DegenerateInterval);
        }
        let t = -q / (2.0 * r);
        let phi = t.clamp(-1.0, 1.0).acos();
        let t1 = 2.0 * cube_root(r)?;
        for offset in [0.0, 2.0 * PI, 4.0 * PI] {
            let root = t1 * ((phi + offset) / 3.0).cos() - a / 3.0;
            if valid_root(root) {
                result.push(root);
            }
        }
        return Ok(result);
    }
    if discriminant == 0.0 {
        let u1 = if q2 < 0.0 {
            cube_root(-q2)?
        } else {
            -cube_root(q2)?
        };
        let root = 2.0 * u1 - a / 3.0;
        if valid_root(root) {
            result.push(root);
        }
        let root = -u1 - a / 3.0;
        if valid_root(root) {
            result.push(root);
        }
        return Ok(result);
    }
    let sd = discriminant.sqrt();
    let u1 = cube_root(sd - q2)?;
    let v1 = cube_root(sd + q2)?;
    let root = u1 - v1 - a / 3.0;
    if valid_root(root) {
        result.push(root);
    }
    Ok(result)
}

pub fn scale_handle(p0: Point, p1: Point, p2: Point) -> Result<Point, InterpolationError> {
    finite(&[p0.0, p0.1, p1.0, p1.1, p2.0, p2.1])?;
    if p1.0 == p0.0 {
        return Err(InterpolationError::DegenerateInterval);
    }
    let y = (p1.1 - p0.1) * (p2.0 - p0.0) / (p1.0 - p0.0);
    Ok((p2.0, output(p0.1 + y)?))
}

pub fn bezier_interpolate(
    p0: Point,
    mut p1: Point,
    mut p2: Point,
    p3: Point,
    x: f64,
) -> Result<f64, InterpolationError> {
    finite(&[p0.0, p0.1, p1.0, p1.1, p2.0, p2.1, p3.0, p3.1, x])?;
    if p0.0 >= p3.0 {
        return Err(InterpolationError::UndefinedUpstreamCase);
    }
    if p1.0 > p3.0 {
        p1 = scale_handle(p0, p1, p3)?;
    } else if p1.0 < p0.0 {
        p1.0 = p0.0;
    }
    if p2.0 < p0.0 {
        p2 = scale_handle(p3, p2, p0)?;
    } else if p2.0 > p3.0 {
        p2.0 = p3.0;
    }
    let roots = bezier_cubic_roots(p0.0 - x, p1.0 - x, p2.0 - x, p3.0 - x)?;
    let t = roots
        .first()
        .ok_or(InterpolationError::NoBezierRoot)?
        .clamp(0.0, 1.0);
    cubic_bezier(p0.1, p1.1, p2.1, p3.1, t)
}

pub fn bezier_interpolate_old(
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    t: f64,
) -> Result<f64, InterpolationError> {
    finite(&[p0.0, p0.1, p1.0, p1.1, p2.0, p2.1, p3.0, p3.1, t])?;
    let t_len = p3.0 - p0.0;
    if t_len == 0.0 {
        return Err(InterpolationError::DegenerateInterval);
    }
    let mut guess_t = (t - p0.0) / t_len;
    for _ in 0..20 {
        let x = cubic_bezier(p0.0, p1.0, p2.0, p3.0, guess_t)?;
        if x == t {
            break;
        }
        guess_t -= (x - t) / t_len;
        guess_t = guess_t.clamp(0.0, 1.0);
    }
    cubic_bezier(p0.1, p1.1, p2.1, p3.1, guess_t)
}

pub fn sign_no_zero(v: f64) -> Result<i8, InterpolationError> {
    finite(&[v])?;
    Ok(if v >= 0.0 { 1 } else { -1 })
}

pub fn calculate_tangent(
    p0: Point,
    p1: Point,
    p2: Point,
    in_tangent: bool,
) -> Result<Point, InterpolationError> {
    finite(&[p0.0, p0.1, p1.0, p1.1, p2.0, p2.1])?;
    let (px, py) = p0;
    let (x, y) = p1;
    let (nx, ny) = p2;
    if nx == px || x == px || nx == x {
        return Err(InterpolationError::DegenerateInterval);
    }
    let mut tan_x = if in_tangent {
        0.4 * (x - px)
    } else {
        0.4 * (nx - x)
    };
    let slope = (ny - py) / (nx - px);
    let prev_slope = (y - py) / (x - px);
    let next_slope = (ny - y) / (nx - x);
    let mut tan_y = if sign_no_zero(prev_slope)? != sign_no_zero(next_slope)?
        || sign_no_zero(slope)? != sign_no_zero(next_slope)?
        || ny == py
    {
        0.0
    } else {
        let height = (ny - py).abs();
        let h1 = (ny - y).abs();
        let h2 = (y - py).abs();
        let scale = h1.min(h2) / height * 2.0;
        scale * slope * tan_x
    };
    if in_tangent {
        tan_x *= -1.0;
        tan_y *= -1.0;
    }
    Ok((output(tan_x)?, output(tan_y)?))
}

pub fn cubic_interpolate(
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    t: f64,
) -> Result<f64, InterpolationError> {
    let (tan_x0, tan_y0) = calculate_tangent(p0, p1, p2, false)?;
    let (tan_x1, tan_y1) = calculate_tangent(p1, p2, p3, true)?;
    bezier_interpolate(
        p1,
        (p1.0 + tan_x0, p1.1 + tan_y0),
        (p2.0 + tan_x1, p2.1 + tan_y1),
        p2,
        t,
    )
}

pub fn mc_trapezoidal_integrate<F>(
    value_at: F,
    a: f64,
    b: f64,
    n: usize,
) -> Result<f64, InterpolationError>
where
    F: Fn(f64) -> f64,
{
    finite(&[a, b])?;
    if n == 0 {
        return Err(InterpolationError::InvalidStepCount);
    }
    if n > MAX_INTEGRATION_STEPS {
        return Err(InterpolationError::TooManySteps);
    }
    let h = (b - a) / n as f64;
    let mut pv = value_at(a - h);
    finite(&[pv])?;
    let mut result = 0.0;
    for i in 0..n {
        let v = value_at(a + (i as f64 * h));
        finite(&[v])?;
        result += (v + pv) * h * 0.5;
        pv = v;
    }
    output(result)
}

/// Port of pyavb's `integrate_iter`, with a bounded collected result.
/// Upstream raises for `end <= start + 1` because `t` is never assigned.
pub fn integrate_iter<F>(
    value_at: F,
    start: i64,
    end: i64,
) -> Result<Vec<Point>, InterpolationError>
where
    F: Fn(f64) -> f64,
{
    let intervals = end
        .checked_sub(start)
        .ok_or(InterpolationError::TooManySteps)?;
    if intervals <= 1 {
        return Err(InterpolationError::UndefinedUpstreamCase);
    }
    let count = intervals
        .checked_mul(2)
        .and_then(|v| v.checked_sub(1))
        .ok_or(InterpolationError::TooManySteps)?;
    if count > MAX_INTEGRATION_STEPS as i64 {
        return Err(InterpolationError::TooManySteps);
    }
    let mut output = Vec::with_capacity(count as usize);
    let mut pos = 0.0;
    for i in start..end - 1 {
        let mut t = i as f64;
        pos += mc_trapezoidal_integrate(&value_at, t - 0.5, t, 5)?;
        output.push((t, pos));
        t += 0.5;
        pos += mc_trapezoidal_integrate(&value_at, t - 0.5, t, 5)?;
        output.push((t, pos));
    }
    let t = (end - 1) as f64;
    pos += mc_trapezoidal_integrate(&value_at, t - 0.5, t, 5)?;
    output.push((t, pos));
    if output.iter().all(|(t, v)| t.is_finite() && v.is_finite()) {
        Ok(output)
    } else {
        Err(InterpolationError::NonFiniteResult)
    }
}
