//! Evaluation of decoded PRCL numeric control points, following pyavb's
//! `ParamClip.value_at` (MIT; see NOTICE). This matches that library's model,
//! not a verified Avid rendering or speed-map implementation.

use crate::interpolation::{self, InterpolationError};
use crate::objects::{Node, Value};
use std::cell::RefCell;

#[derive(Debug, Clone, PartialEq)]
pub enum ParamEvalError {
    NotParamClip,
    MissingField(&'static str),
    InvalidPoint,
    EmptyPoints,
    ZeroDenominator,
    ZeroTimescale,
    ReferenceValue,
    UnsupportedInterpolation(i64),
    InvalidTime,
    InvalidRange,
    MissingCenter,
    Math(InterpolationError),
}

impl From<InterpolationError> for ParamEvalError {
    fn from(value: InterpolationError) -> Self {
        Self::Math(value)
    }
}

#[derive(Debug, Clone)]
struct Point {
    time: f64,
    value: f64,
    in_tangent: (f64, f64),
    out_tangent: (f64, f64),
    base_frame: f64,
}

fn numeric(value: &Value) -> Result<f64, ParamEvalError> {
    match value {
        Value::Int(n) => Ok(*n as f64),
        Value::UInt(n) => Ok(*n as f64),
        Value::Float(n) if n.is_finite() => Ok(*n),
        Value::Ref(_) => Err(ParamEvalError::ReferenceValue),
        _ => Err(ParamEvalError::InvalidPoint),
    }
}

fn field<'a>(node: &'a Node, name: &'static str) -> Result<&'a Value, ParamEvalError> {
    node.get(name).ok_or(ParamEvalError::MissingField(name))
}

fn points(node: &Node) -> Result<Vec<Point>, ParamEvalError> {
    let Value::List(raw) = field(node, "control_points")? else {
        return Err(ParamEvalError::InvalidPoint);
    };
    if raw.is_empty() {
        return Err(ParamEvalError::EmptyPoints);
    }
    let mut result = Vec::with_capacity(raw.len());
    for raw_point in raw {
        let Value::Node(point) = raw_point else {
            return Err(ParamEvalError::InvalidPoint);
        };
        let Value::List(offset) = field(point, "offset")? else {
            return Err(ParamEvalError::InvalidPoint);
        };
        if offset.len() != 2 {
            return Err(ParamEvalError::InvalidPoint);
        }
        let num = numeric(&offset[0])?;
        let den = numeric(&offset[1])?;
        if den == 0.0 {
            return Err(ParamEvalError::ZeroDenominator);
        }
        let scale = numeric(field(point, "timescale")?)?;
        if scale == 0.0 {
            return Err(ParamEvalError::ZeroTimescale);
        }
        let time = num / den * scale;
        let value = numeric(field(point, "value")?)?;
        if !time.is_finite() || !value.is_finite() {
            return Err(ParamEvalError::InvalidTime);
        }
        let mut tangents = [(0.0, 0.0); 2];
        let mut base_frame = 0.0;
        if let Value::List(properties) = field(point, "pp")? {
            for property in properties {
                let Value::Node(prop) = property else {
                    return Err(ParamEvalError::InvalidPoint);
                };
                let code = numeric(field(prop, "code")?)? as i64;
                let value = numeric(field(prop, "value")?)?;
                match code {
                    5 => tangents[0].0 = value,
                    6 => tangents[0].1 = value,
                    7 => tangents[1].0 = value,
                    8 => tangents[1].1 = value,
                    14 => base_frame = value,
                    _ => {}
                }
            }
        } else {
            return Err(ParamEvalError::InvalidPoint);
        }
        result.push(Point {
            time,
            value,
            in_tangent: tangents[0],
            out_tangent: tangents[1],
            base_frame,
        });
    }
    Ok(result)
}

/// Evaluate one numeric PRCL at `time`, using the interpolation kind stored in the bin.
pub fn value_at(node: &Node, time: f64) -> Result<f64, ParamEvalError> {
    if &node.class != b"PRCL" {
        return Err(ParamEvalError::NotParamClip);
    }
    if !time.is_finite() {
        return Err(ParamEvalError::InvalidTime);
    }
    let kind = numeric(field(node, "interp_kind")?)? as i64;
    let points = points(node)?;
    eval_points(kind, &points, time)
}

fn eval_points(kind: i64, points: &[Point], time: f64) -> Result<f64, ParamEvalError> {
    if !time.is_finite() {
        return Err(ParamEvalError::InvalidTime);
    }
    // Match pyavb's binary search, including its choice among duplicate times.
    let (mut start, mut end) = (0isize, points.len() as isize - 1);
    let index = loop {
        if end < start {
            break end.max(0) as usize;
        }
        let mid = (start + end) / 2;
        if points[mid as usize].time < time {
            start = mid + 1;
        } else if points[mid as usize].time > time {
            end = mid - 1;
        } else {
            break mid as usize;
        }
    };
    let p1 = &points[index];
    if time < p1.time || index + 1 >= points.len() || kind == 2 {
        return Ok(p1.value);
    }
    let p2 = &points[index + 1];
    let a = (p1.time, p1.value);
    let b = (p2.time, p2.value);
    match kind {
        3 => {
            let len = p2.time - p1.time;
            if len == 0.0 {
                return Err(ParamEvalError::Math(InterpolationError::DegenerateInterval));
            }
            Ok(interpolation::lerp(
                p1.value,
                p2.value,
                (time - p1.time) / len,
            )?)
        }
        5 => Ok(interpolation::bezier_interpolate(
            a,
            (p1.time + p1.out_tangent.0, p1.value + p1.out_tangent.1),
            (p2.time + p2.in_tangent.0, p2.value + p2.in_tangent.1),
            b,
            time,
        )?),
        6 => {
            let prev = if index > 0 {
                let p = &points[index - 1];
                (p.time, p.value)
            } else {
                (p1.time - (p2.time - p1.time) * 0.5, p1.value)
            };
            let next = if index + 2 < points.len() {
                let p = &points[index + 2];
                (p.time, p.value)
            } else {
                (p2.time + (p2.time - p1.time) * 0.5, p2.value)
            };
            Ok(interpolation::cubic_interpolate(prev, a, b, next, time)?)
        }
        _ => Err(ParamEvalError::UnsupportedInterpolation(kind)),
    }
}

/// Integrate a PRCL speed curve to pyavb's offset-map samples.
///
/// The source implementation itself questions the meaning of `base_frame`,
/// and these samples have not been compared with Avid's rendered Timewarp.
pub fn integrate(
    node: &Node,
    start: i64,
    end: Option<i64>,
) -> Result<Vec<(f64, f64)>, ParamEvalError> {
    if &node.class != b"PRCL" {
        return Err(ParamEvalError::NotParamClip);
    }
    let kind = numeric(field(node, "interp_kind")?)? as i64;
    let points = points(node)?;
    let center = points[0].time as i64;
    let end = end.unwrap_or(points.last().ok_or(ParamEvalError::EmptyPoints)?.time as i64);
    if start > end {
        return Err(ParamEvalError::InvalidRange);
    }
    let inter_start = start.min(center);
    let inter_end = center.max(end.checked_add(1).ok_or(ParamEvalError::InvalidRange)?);
    // The numerical integration routine accepts a non-fallible callback. Capture
    // a precise parameter error and return NaN only to stop its current sample.
    let error = RefCell::new(None);
    let result = interpolation::integrate_iter(
        |t| match eval_points(kind, &points, t) {
            Ok(value) => value,
            Err(e) => {
                *error.borrow_mut() = Some(e);
                f64::NAN
            }
        },
        inter_start,
        inter_end,
    );
    if let Some(e) = error.into_inner() {
        return Err(e);
    }
    let samples = result?;
    let center_offset = samples
        .iter()
        .find(|(t, _)| *t == center as f64)
        .map(|(_, v)| *v)
        .ok_or(ParamEvalError::MissingCenter)?
        - points[0].base_frame;
    Ok(samples
        .into_iter()
        .filter(|(t, _)| *t >= start as f64 && *t <= end as f64)
        .map(|(t, v)| (t, v - center_offset))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(time: i64, value: f64) -> Value {
        Value::Node(Box::new(Node {
            class: [0; 4],
            type_name: "ParamControlPoint",
            fields: vec![
                ("offset", Value::List(vec![Value::Int(time), Value::Int(1)])),
                ("timescale", Value::Int(1)),
                ("value", Value::Float(value)),
                ("pp", Value::List(Vec::new())),
            ],
        }))
    }

    fn clip(kind: i64) -> Node {
        Node {
            class: *b"PRCL",
            type_name: "ParamClip",
            fields: vec![
                ("interp_kind", Value::Int(kind)),
                (
                    "control_points",
                    Value::List(vec![point(0, 10.0), point(10, 30.0)]),
                ),
            ],
        }
    }

    #[test]
    fn constant_and_linear_clamp_like_pyavb() {
        let constant = clip(2);
        assert_eq!(value_at(&constant, 5.0), Ok(10.0));
        let linear = clip(3);
        assert_eq!(value_at(&linear, -1.0), Ok(10.0));
        assert_eq!(value_at(&linear, 5.0), Ok(20.0));
        assert_eq!(value_at(&linear, 12.0), Ok(30.0));
    }

    #[test]
    fn rejects_bad_time_and_reference_values() {
        let mut linear = clip(3);
        assert_eq!(
            value_at(&linear, f64::NAN),
            Err(ParamEvalError::InvalidTime)
        );
        let Value::List(points) = &mut linear.fields[1].1 else {
            panic!()
        };
        let Value::Node(point) = &mut points[0] else {
            panic!()
        };
        point.fields[2].1 = Value::Ref(2);
        assert_eq!(value_at(&linear, 5.0), Err(ParamEvalError::ReferenceValue));
    }

    #[test]
    fn pinned_upstream_paramclip_matches_python_midpoint() {
        let Ok(root) = std::env::var("PYAVB_TEST_FILES") else {
            return;
        };
        let path = std::path::Path::new(&root).join("test_file_01.avb");
        let file = crate::file::AvbFile::open(&path).expect("pinned upstream bin");
        let param = file.node(15).expect("PRCL object 15");
        assert_eq!(&param.class, b"PRCL");
        // pyavb ff64e06, use_ext=False: point times 0 and 604;
        // value_at(302) == 0.5, interp_kind 3 (linear).
        assert_eq!(value_at(param, 302.0), Ok(0.5));
        // Same pinned pyavb object: ParamClip.integrate(0, 3).
        let offsets = integrate(param, 0, Some(3)).unwrap();
        let expected = [
            (0.0, 0.0),
            (0.5, 0.00013245033112582781),
            (1.0, 0.0006705298013245033),
            (1.5, 0.0016225165562913906),
            (2.0, 0.00298841059602649),
            (2.5, 0.004768211920529801),
            (3.0, 0.006961920529801324),
        ];
        assert_eq!(offsets.len(), expected.len());
        for (actual, expected) in offsets.iter().zip(expected) {
            assert_eq!(actual.0, expected.0);
            assert!(
                (actual.1 - expected.1).abs() < 1e-12,
                "{actual:?} != {expected:?}"
            );
        }
    }
}
