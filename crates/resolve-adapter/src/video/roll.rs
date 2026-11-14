//! Three aligned Linear curves on Inspector Pan, Zoom X, and Rotation Angle.
use super::{
    motion::{key_range, linear_parameter, Curve},
    pack, parameter, section, ROTATION, SCALE_X, X,
};
use crate::Result;
use graph::{Effect, Value};

const AVID_KIND: &str = "avid.warp_roll_linear";
pub(crate) const RESOLVE_KIND: &str = "resolve.warp_roll_linear";
const BAD_KEYS: &str = "unsupported Resolve Linear Warp rotation keys or source extent";
pub(crate) const CONTROLS: [(usize, u64); 3] = [X, SCALE_X, ROTATION];
const NAMES: [&str; 3] = ["pan", "zoom_x", "rotation"];

pub(crate) fn is_linear(kind: &str) -> bool {
    matches!(kind, AVID_KIND | RESOLVE_KIND)
}

#[derive(Debug, PartialEq)]
pub(crate) struct LinearRoll([Curve; 3]);

impl LinearRoll {
    pub(crate) fn from_graph(
        effect: &Effect,
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Result<Self> {
        let Value::Obj(fields) = &effect.params else {
            return Err(BAD_KEYS.into());
        };
        let mut curves: [Curve; 3] = Default::default();
        match effect.kind.as_str() {
            AVID_KIND => {
                let [(name, Value::Arr(keys))] = fields.as_slice() else {
                    return Err(BAD_KEYS.into());
                };
                if name != "keys" || keys.len() != 3 {
                    return Err(BAD_KEYS.into());
                }
                for key in keys {
                    let Value::Obj(fields) = key else {
                        return Err(BAD_KEYS.into());
                    };
                    if fields.len() != 4 {
                        return Err(BAD_KEYS.into());
                    }
                    let Some(Value::Int(frame)) = key.get("frame") else {
                        return Err(BAD_KEYS.into());
                    };
                    let frame = source_in.checked_add(*frame).ok_or(BAD_KEYS)?;
                    let frame = u32::try_from(frame)?;
                    let value = |name| key.get(name).and_then(number).ok_or(BAD_KEYS);
                    for (curve, value) in curves.iter_mut().zip([
                        value("x")?,
                        value("width")? / 100.0,
                        -value("rotation")?,
                    ]) {
                        curve.push((frame, value));
                    }
                }
            }
            RESOLVE_KIND if fields.len() == 3 => {
                for (curve, name) in curves.iter_mut().zip(NAMES) {
                    let Some(Value::Arr(keys)) = effect.params.get(name) else {
                        return Err(BAD_KEYS.into());
                    };
                    if keys.len() != 3 {
                        return Err(BAD_KEYS.into());
                    }
                    for key in keys {
                        let Value::Arr(fields) = key else {
                            return Err(BAD_KEYS.into());
                        };
                        let [Value::Int(frame), value] = fields.as_slice() else {
                            return Err(BAD_KEYS.into());
                        };
                        curve.push((u32::try_from(*frame)?, number(value).ok_or(BAD_KEYS)?));
                    }
                }
            }
            _ => return Err(BAD_KEYS.into()),
        }
        Self::from_saved(curves, source_in, length, available).ok_or_else(|| BAD_KEYS.into())
    }

    pub(crate) fn from_saved(
        curves: [Curve; 3],
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Option<Self> {
        let (start, end) = key_range(source_in, length)?;
        let [(first, _), (middle, _), (last, _)] = curves[0].as_slice() else {
            return None;
        };
        if !(3..=256).contains(&length)
            || (*first, *last) != (start, end)
            || !(start < *middle && *middle < end)
            || i64::from(end) >= available
        {
            return None;
        }
        let limits = [(-60.0 / 999.0)..=(90.0 / 999.0), 0.7..=1.0, -25.0..=15.0];
        curves
            .iter()
            .zip(limits)
            .all(|(curve, limits)| {
                curve.iter().map(|key| key.0).eq([start, *middle, end])
                    && curve.iter().all(|(_, value)| limits.contains(value))
            })
            .then_some(Self(curves))
    }

    pub(crate) fn encode(&self) -> Result<String> {
        let mut motion: [Vec<u8>; 12] = Default::default();
        motion[2] = parameter(44, &[0x28, 0]);
        for ((slot, id), curve) in CONTROLS.into_iter().zip(&self.0) {
            motion[slot] = linear_parameter(id, curve)?;
        }
        let mut body = Vec::new();
        section(4, 0, &motion, &mut body);
        pack(&body)
    }

    pub(crate) fn prepare_premiere(&self) -> Result<prproj::convert::prepared::PictureEffect> {
        use prproj::convert::{
            model::TICKS_PER_SECOND,
            prepared::{PictureEffect, WarpRollInterpolation, WarpRollKey},
        };

        let [pan, zoom, angle] = &self.0;
        let key = |index: usize| -> Result<WarpRollKey> {
            Ok(WarpRollKey {
                ticks: i64::from(pan[index].0)
                    .checked_mul(TICKS_PER_SECOND / 24)
                    .ok_or(BAD_KEYS)?,
                x: pan[index].1,
                width: zoom[index].1 * 100.0,
                rotation: -angle[index].1,
            })
        };
        Ok(PictureEffect::WarpRoll {
            keys: [key(0)?, key(1)?, key(2)?],
            interpolation: WarpRollInterpolation::Linear,
        })
    }

    pub(crate) fn readback(&self) -> Effect {
        Effect {
            kind: RESOLVE_KIND.into(),
            params: Value::Obj(
                NAMES
                    .into_iter()
                    .zip(&self.0)
                    .map(|(name, curve)| {
                        let keys = curve
                            .iter()
                            .map(|&(frame, value)| {
                                Value::Arr(vec![Value::Int(i64::from(frame)), Value::Float(value)])
                            })
                            .collect();
                        (name.into(), Value::Arr(keys))
                    })
                    .collect(),
            ),
        }
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(n) => Some(*n),
        Value::Int(n) => Some(*n as f64),
        _ => None,
    }
}
