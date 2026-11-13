//! Linear keys on Inspector position, independent zoom, and source crop axes.
use super::{pack, parameter, pb_bytes, section, Reflection, CROP, SCALE_X, SCALE_Y, X, Y};
use crate::{prepare::frames_on, Result};
use graph::{picture_motion, Effect, Loss, Rational, Transform, Value};

pub(crate) const KEY_FLAGS: u32 = 0x000c_0000;
pub(crate) const CONTROLS: [(usize, u64); 4] = [X, Y, SCALE_X, SCALE_Y];
const NAMES: [&str; 4] = ["pan", "tilt", "zoom_x", "zoom_y"];
const CROP_NAMES: [&str; 4] = ["crop_left", "crop_top", "crop_right", "crop_bottom"];
pub(crate) const KEY_BYTES: usize = 16;
pub(crate) type Curve = Vec<(u32, f64)>;
const BAD_MOTION: &str = "unsupported Resolve Linear motion keys or geometry";
const BAD_HANDLE: &str = "Resolve Linear motion key is outside verified source media";
const BAD_CROP: &str = "unsupported Resolve Linear crop keys or clock";
pub(crate) const NATIVE_LINEAR_KIND: &str = "resolve.motion_linear";

#[derive(Debug, PartialEq)]
pub(crate) struct LinearMotion {
    curves: [Curve; 4],
    crop: Option<[Curve; 4]>,
    reflection: Reflection,
}

pub(crate) fn is_avid_linear(kind: &str) -> bool {
    matches!(
        kind,
        "avid.resize_linear"
            | "avid.warp_linear"
            | "avid.resize_trim_linear"
            | "avid.warp_trim_linear"
            | "avid.resize_trim_crop_linear"
            | "avid.warp_trim_crop_linear"
    )
}

pub(crate) fn is_neutral_resize_background(loss: &Loss) -> bool {
    // The qualified Avid extraction admits these background controls at zero.
    // Older static interpretation also emits them as transform diagnostics.
    loss.aspect == "transform"
        && matches!(
            loss.reason.as_str(),
            "Resize control 4781568d = 0"
                | "Resize control 4881568d = 0"
                | "Resize control 4981568d = 0"
        )
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(n) => Some(*n),
        Value::Int(n) => Some(*n as f64),
        _ => None,
    }
}

fn pair(value: &Value, key: &str) -> Option<(f64, f64)> {
    let Value::Arr(values) = value.get(key)? else {
        return None;
    };
    let [a, b] = values.as_slice() else {
        return None;
    };
    Some((number(a)?, number(b)?))
}

impl LinearMotion {
    pub(crate) fn prepare_premiere(
        effect: &Effect,
        source_in: i64,
        length: i64,
        available: i64,
        frame_ticks: i64,
    ) -> Result<(Transform, prproj::convert::prepared::PictureEffect)> {
        use prproj::convert::prepared::{MotionKey, PictureEffect, TrimmedMotion, TrimmedMotionKey};

        let Value::Obj(fields) = &effect.params else {
            return Err(BAD_MOTION.into());
        };
        if effect.kind != NATIVE_LINEAR_KIND || fields.len() != NAMES.len() {
            return Err(BAD_MOTION.into());
        }
        let mut curves: [Curve; 4] = Default::default();
        for (axis, curve) in NAMES.into_iter().zip(&mut curves) {
            let Some(Value::Arr(keys)) = effect.params.get(axis) else {
                return Err(BAD_MOTION.into());
            };
            if !matches!(keys.len(), 2 | 3) {
                return Err(BAD_MOTION.into());
            }
            for key in keys {
                let Value::Arr(pair) = key else {
                    return Err(BAD_MOTION.into());
                };
                let [Value::Int(frame), value] = pair.as_slice() else {
                    return Err(BAD_MOTION.into());
                };
                curve.push((u32::try_from(*frame)?, number(value).ok_or(BAD_MOTION)?));
            }
        }
        if curves.iter().flatten().any(|&(frame, _)| i64::from(frame) >= available) {
            return Err(BAD_HANDLE.into());
        }
        let native = Self::from_saved(curves, None, Reflection::default(), source_in, length, available)
            .ok_or(BAD_MOTION)?;
        let [pan, tilt, width, height] = &native.curves;
        let aligned_width = width.iter().map(|key| key.0).eq(pan.iter().map(|key| key.0));
        let ticks = |frame: u32| i64::from(frame).checked_mul(frame_ticks).ok_or(BAD_HANDLE);
        let position = |i: usize| [pan[i].1, -tilt[i].1];
        let scale = |i: usize| [width[i].1 * 100.0, height[i].1 * 100.0];
        let picture = if pan.len() == 2 {
            if !aligned_width {
                return Err(BAD_MOTION.into());
            }
            let key = |i: usize| -> Result<MotionKey> {
                Ok(MotionKey {
                    ticks: ticks(pan[i].0)?,
                    position: position(i),
                    scale: scale(i),
                    crop: None,
                })
            };
            PictureEffect::Resize([key(0)?, key(1)?])
        } else {
            let (start, end) = key_range(source_in, length).ok_or(BAD_MOTION)?;
            if !((pan[0].0 == start && width[0].0 == start)
                || (pan[2].0 == end && width[2].0 == end))
            {
                return Err("Resolve Linear motion needs a shared visible edge on both key grids".into());
            }
            let motion = |i: usize| -> Result<TrimmedMotionKey> {
                Ok(TrimmedMotionKey {
                    ticks: ticks(pan[i].0)?,
                    position: position(i),
                    height: height[i].1 * 100.0,
                })
            };
            let width_key = |i: usize| -> Result<(i64, f64)> {
                Ok((ticks(width[i].0)?, width[i].1 * 100.0))
            };
            PictureEffect::TrimmedResize(TrimmedMotion {
                motion: [motion(0)?, motion(1)?, motion(2)?],
                width: [width_key(0)?, width_key(1)?, width_key(2)?],
                fixed_width: !aligned_width,
                crop: None,
            })
        };
        // The prepared Motion component uses the first key of each axis as its base.
        let base = Transform {
            position: position(0).into(),
            scale: scale(0).into(),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            crop: [0.0; 4],
        };
        Ok((base, picture))
    }

    pub(crate) fn from_graph(
        effect: &Effect,
        rate: Rational,
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Result<Self> {
        let [first, last] = picture_motion::read_linear(effect).ok_or(BAD_MOTION)?;
        if first.position != (0.0, 0.0)
            || first.scale_percent != 100.0
            || !(-0.05..=0.05).contains(&last.position.0)
            || !(-0.05..=0.05).contains(&last.position.1)
            || !(87.5..=112.5).contains(&last.scale_percent)
        {
            return Err(BAD_MOTION.into());
        }
        let start = u32::try_from(frames_on(first.at, rate)?)?;
        let end = u32::try_from(frames_on(last.at, rate)?)?;
        let zoom = vec![(start, 1.0), (end, last.scale_percent / 100.0)];
        let curves = [
            vec![(start, 0.0), (end, last.position.0)],
            vec![(start, 0.0), (end, -last.position.1)],
            zoom.clone(),
            zoom,
        ];
        Self::from_saved(
            curves,
            None,
            Reflection::default(),
            source_in,
            length,
            available,
        )
        .ok_or_else(|| BAD_MOTION.into())
    }

    pub(crate) fn from_avid(
        kind: &str,
        params: &Value,
        transform: &Transform,
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Result<Self> {
        let (start, end) = key_range(source_in, length).ok_or(BAD_MOTION)?;
        if transform.rotation != 0.0 || transform.anchor != (0.0, 0.0) || transform.crop != [0.0; 4]
        {
            return Err(BAD_MOTION.into());
        }
        let (curves, crop) = match kind {
            "avid.resize_linear" | "avid.warp_linear" => {
                (two_keys(params, transform, start, end)?, None)
            }
            "avid.resize_trim_linear" | "avid.warp_trim_linear" => (
                trim_keys(
                    kind,
                    params,
                    transform,
                    source_in,
                    length - 1,
                    available,
                    false,
                )?,
                None,
            ),
            "avid.resize_trim_crop_linear" | "avid.warp_trim_crop_linear" => {
                let curves = trim_keys(
                    kind,
                    params,
                    transform,
                    source_in,
                    length - 1,
                    available,
                    true,
                )?;
                let crop = crop_keys(params, &curves[0], source_in).ok_or(BAD_CROP)?;
                (curves, Some(crop))
            }
            _ => return Err(BAD_MOTION.into()),
        };
        Self::from_saved(
            curves,
            crop,
            Reflection::default(),
            source_in,
            length,
            available,
        )
        .ok_or_else(|| BAD_MOTION.into())
    }

    pub(crate) fn from_saved(
        curves: [Curve; 4],
        crop: Option<[Curve; 4]>,
        reflection: Reflection,
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Option<Self> {
        let (start, end) = key_range(source_in, length)?;
        let count = curves[0].len();
        if !matches!(count, 2 | 3) {
            return None;
        }
        let width_limit = if count == 3 { 1.6 } else { 1.35 };
        let limits = [
            (if reflection.horizontal {
                -0.201
            } else {
                -0.151
            })..=0.201,
            (if reflection.vertical { -0.126 } else { -0.076 })..=0.126,
            0.65..=width_limit,
            0.65..=1.35,
        ];
        let same_time_grid = |axis: usize| {
            curves[axis]
                .iter()
                .map(|p| p.0)
                .eq(curves[0].iter().map(|p| p.0))
        };
        if !same_time_grid(1) || !same_time_grid(3) {
            return None;
        }
        if let Some(edges) = &crop {
            // Native qualification covers one Elastic grid spanning the visible edit.
            if count != 3
                || !same_time_grid(2)
                || curves[0][0].0 != start
                || curves[0][2].0 != end
                || edges.iter().any(|edge| {
                    !edge
                        .iter()
                        .map(|key| key.0)
                        .eq(curves[0].iter().map(|key| key.0))
                        || edge.iter().any(|(_, value)| !(0.0..=0.151).contains(value))
                })
            {
                return None;
            }
        }
        curves
            .iter()
            .zip(limits)
            .all(|(curve, limit)| {
                if curve.len() != count {
                    return false;
                }
                let (first, last) = (curve[0].0, curve[count - 1].0);
                (if count == 2 {
                    first == start && last == end
                } else {
                    first <= start && last >= end
                }) && i64::from(last) < available
                    && curve.windows(2).all(|pair| pair[0].0 < pair[1].0)
                    && curve.iter().all(|(_, value)| limit.contains(value))
            })
            .then_some(Self {
                curves,
                crop,
                reflection,
            })
    }

    pub(crate) fn project_chain(
        mut self,
        before: graph::picture_chain::Reflection,
        after: graph::picture_chain::Reflection,
        source_in: i64,
        length: i64,
        available: i64,
    ) -> Result<Self> {
        // Resolve crops before Flip, then applies Position and Zoom. Swap the
        // incoming crop edges in source space and mirror the final position.
        self.crop = self.crop.map(|edges| before.crop(edges));
        for (curve, reflected) in self
            .curves
            .iter_mut()
            .zip([after.horizontal, after.vertical])
        {
            if reflected {
                for (_, value) in curve {
                    *value = -*value;
                }
            }
        }
        Self::from_saved(
            self.curves,
            self.crop,
            before.then(after).into(),
            source_in,
            length,
            available,
        )
        .ok_or_else(|| "ordered picture chain is outside qualified Resolve motion bounds".into())
    }

    pub(crate) fn encode(&self) -> Result<String> {
        let mut motion: [Vec<u8>; 12] = Default::default();
        motion[2] = parameter(44, &[0x28, 0]); // Independent Zoom X and Zoom Y.
        for ((slot, id), curve) in CONTROLS.into_iter().zip(&self.curves) {
            motion[slot] = linear_parameter(id, curve)?;
        }
        self.reflection.write_slots(&mut motion);
        let mut body = Vec::new();
        section(4, 0, &motion, &mut body);
        if let Some(edges) = &self.crop {
            let mut crop: [Vec<u8>; 6] = Default::default();
            for ((slot, id), curve) in CROP.into_iter().zip(edges) {
                crop[slot] = linear_parameter(id, curve)?;
            }
            // Resolve applies source-space crop before Motion with Retain Image Position off.
            section(6, 6, &crop, &mut body);
        }
        pack(&body)
    }

    pub(crate) fn readback(&self) -> Effect {
        let mut params: Vec<_> = NAMES
            .into_iter()
            .zip(&self.curves)
            .chain(CROP_NAMES.into_iter().zip(self.crop.iter().flatten()))
            .map(|(name, curve)| {
                let keys = curve
                    .iter()
                    .map(|&(frame, value)| {
                        Value::Arr(vec![Value::Int(i64::from(frame)), Value::Float(value)])
                    })
                    .collect();
                (name.into(), Value::Arr(keys))
            })
            .collect();
        if self.reflection.active() {
            params.push((
                "horizontal_flip".into(),
                Value::Bool(self.reflection.horizontal),
            ));
            params.push((
                "vertical_flip".into(),
                Value::Bool(self.reflection.vertical),
            ));
        }
        Effect {
            kind: NATIVE_LINEAR_KIND.into(),
            params: Value::Obj(params),
        }
    }
}

fn linear_parameter(id: u64, curve: &Curve) -> Result<Vec<u8>> {
    let mut scalar = vec![0x11];
    scalar.extend_from_slice(&curve[0].1.to_le_bytes());
    let mut param = parameter(id, &scalar);
    let bytes = curve.len() * KEY_BYTES;
    let mut keys = Vec::with_capacity(4 + bytes);
    keys.extend_from_slice(&(-i32::try_from(bytes)?).to_be_bytes());
    for (frame, value) in curve {
        keys.extend_from_slice(&frame.to_le_bytes());
        keys.extend_from_slice(&KEY_FLAGS.to_le_bytes());
        keys.extend_from_slice(&value.to_le_bytes());
    }
    pb_bytes(10, &keys, &mut param);
    Ok(param)
}

fn crop_keys(params: &Value, motion: &Curve, source_in: i64) -> Option<[Curve; 4]> {
    let Value::Arr(keys) = params.get("crop")? else {
        return None;
    };
    if keys.len() != 3 || keys.len() != motion.len() {
        return None;
    }
    let mut curves: [Curve; 4] = std::array::from_fn(|_| Vec::with_capacity(3));
    for (key, &(source_frame, _)) in keys.iter().zip(motion) {
        let Value::Obj(fields) = key else { return None };
        let (Some(Value::Int(frame)), Some(Value::Arr(edges))) =
            (key.get("frame"), key.get("edges"))
        else {
            return None;
        };
        if fields.len() != 2
            || edges.len() != 4
            || source_in.checked_add(*frame)? != i64::from(source_frame)
        {
            return None;
        }
        for (curve, edge) in curves.iter_mut().zip(edges) {
            curve.push((source_frame, number(edge)?));
        }
    }
    Some(curves)
}

fn two_keys(params: &Value, transform: &Transform, start: u32, end: u32) -> Result<[Curve; 4]> {
    let Value::Obj(fields) = params else {
        return Err(BAD_MOTION.into());
    };
    let [(name, Value::Arr(keys))] = fields.as_slice() else {
        return Err(BAD_MOTION.into());
    };
    let [first, last] = keys.as_slice() else {
        return Err(BAD_MOTION.into());
    };
    if name != "keys" {
        return Err(BAD_MOTION.into());
    }
    let parse = |v: &Value, offset: i64| {
        let Value::Obj(fields) = v else { return None };
        if fields.len() != 3 || v.get("frame") != Some(&Value::Int(offset)) {
            return None;
        }
        Some((pair(v, "position")?, pair(v, "scale")?))
    };
    let (first_pos, first_scale) = parse(first, 0).ok_or(BAD_MOTION)?;
    let (last_pos, last_scale) = parse(last, i64::from(end - start)).ok_or(BAD_MOTION)?;
    if transform.position != first_pos || transform.scale != first_scale {
        return Err(BAD_MOTION.into());
    }
    // Graph Y is down for both Avid identities. Resolve Tilt is up.
    Ok([
        vec![(start, first_pos.0), (end, last_pos.0)],
        vec![(start, -first_pos.1), (end, -last_pos.1)],
        vec![(start, first_scale.0 / 100.0), (end, last_scale.0 / 100.0)],
        vec![(start, first_scale.1 / 100.0), (end, last_scale.1 / 100.0)],
    ])
}

fn trim_keys(
    kind: &str,
    params: &Value,
    transform: &Transform,
    source_in: i64,
    last: i64,
    available: i64,
    cropped: bool,
) -> Result<[Curve; 4]> {
    let Value::Obj(fields) = params else {
        return Err(BAD_MOTION.into());
    };
    let (Some(Value::Arr(motion)), Some(Value::Arr(width)), Some(Value::Bool(fixed))) = (
        params.get("motion"),
        params.get("width"),
        params.get("fixed_width"),
    ) else {
        return Err(BAD_MOTION.into());
    };
    if fields.len() != 3 + usize::from(cropped) || motion.len() != 3 || width.len() != 3 {
        return Err(BAD_MOTION.into());
    }
    if cropped && *fixed {
        return Err(BAD_CROP.into());
    }
    let source_frame = |at| {
        source_in
            .checked_add(at)
            .filter(|frame| *frame < available)
            .and_then(|frame| u32::try_from(frame).ok())
            .ok_or(BAD_HANDLE)
    };
    let mut times = [0; 3];
    let mut width_times = [0; 3];
    let mut curves: [Curve; 4] = std::array::from_fn(|_| Vec::with_capacity(3));
    for (i, (pose, width)) in motion.iter().zip(width).enumerate() {
        let Value::Obj(pose_fields) = pose else {
            return Err(BAD_MOTION.into());
        };
        let (Some(Value::Int(at)), Some(height)) = (pose.get("frame"), pose.get("height")) else {
            return Err(BAD_MOTION.into());
        };
        let Value::Arr(width) = width else {
            return Err(BAD_MOTION.into());
        };
        let [Value::Int(width_at), width] = width.as_slice() else {
            return Err(BAD_MOTION.into());
        };
        if pose_fields.len() != 3 {
            return Err(BAD_MOTION.into());
        }
        let (x, y) = pair(pose, "position").ok_or(BAD_MOTION)?;
        let width = number(width).ok_or(BAD_MOTION)?;
        let height = number(height).ok_or(BAD_MOTION)?;
        if i == 0 && (transform.position != (x, y) || transform.scale != (width, height)) {
            return Err(BAD_MOTION.into());
        }
        times[i] = *at;
        width_times[i] = *width_at;
        let at = source_frame(*at)?;
        curves[0].push((at, x));
        curves[1].push((at, -y));
        curves[2].push((source_frame(*width_at)?, width / 100.0));
        curves[3].push((at, height / 100.0));
    }
    let [a, b, c] = width_times;
    let same_visible_edge = (times[0] == 0 && a == 0) || (times[2] == last && c == last);
    if (!*fixed && width_times != times)
        || match kind {
            "avid.resize_trim_linear" | "avid.resize_trim_crop_linear" => !same_visible_edge,
            "avid.warp_trim_linear" | "avid.warp_trim_crop_linear" => {
                times[0] != 0
                    || times[2] != last
                    || !(0 < times[1] && times[1] < last)
                    || !(0 < b && b < last)
                    || (*fixed && !((a < 0 && c == last) || (a == 0 && c > last)))
            }
            _ => true,
        }
    {
        return Err(BAD_MOTION.into());
    }
    Ok(curves)
}

fn key_range(source_in: i64, length: i64) -> Option<(u32, u32)> {
    if length < 2 {
        return None;
    }
    Some((
        u32::try_from(source_in).ok()?,
        u32::try_from(source_in.checked_add(length - 1)?).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::picture_motion::{linear_effect, MotionKey};
    use prproj::convert::prepared::PictureEffect;

    #[test]
    fn premiere_linear_keys_preserve_the_saved_source_grid_and_independent_scale() {
        let native = LinearMotion::from_saved(
            [
                vec![(24, -0.15), (71, 0.20)],
                vec![(24, -0.075), (71, 0.125)],
                vec![(24, 0.8), (71, 1.35)],
                vec![(24, 1.2), (71, 0.65)],
            ],
            None,
            Reflection::default(),
            24, 48, 120,
        ).unwrap();
        let unit = prproj::convert::model::TICKS_PER_SECOND / 24_000 * 1001;
        let effect = native.readback();
        let (_, PictureEffect::Resize([first, last])) =
            LinearMotion::prepare_premiere(&effect, 24, 48, 120, unit).unwrap()
        else {
            panic!("expected two-key motion");
        };
        assert_eq!(first.ticks, 24 * unit);
        assert_eq!(last.ticks, 71 * unit);
        assert_eq!(first.position, [-0.15, 0.075]);
        assert_eq!(last.position, [0.20, -0.125]);
        assert_eq!(first.scale, [80.0, 120.0]);
        assert_eq!(last.scale, [135.0, 65.0]);
        assert!(LinearMotion::prepare_premiere(&effect, 25, 47, 120, unit).is_err());
    }

    #[test]
    fn premiere_trimmed_linear_keys_keep_both_source_grids() {
        let unit = prproj::convert::model::TICKS_PER_SECOND / 24_000 * 1001;
        let effect = LinearMotion::from_saved(
            [
                vec![(26, -0.15), (39, 0.05), (55, 0.20)],
                vec![(26, -0.075), (39, 0.025), (55, 0.125)],
                vec![(9, 0.8), (31, 1.05), (55, 1.35)],
                vec![(26, 1.2), (39, 0.9), (55, 0.65)],
            ],
            None,
            Reflection::default(),
            32, 24, 56,
        ).unwrap().readback();
        let (base, PictureEffect::TrimmedResize(trim)) =
            LinearMotion::prepare_premiere(&effect, 32, 24, 56, unit).unwrap()
        else {
            panic!("expected three-key trimmed motion");
        };
        assert!(trim.fixed_width);
        assert_eq!(base.position, (-0.15, 0.075));
        assert_eq!(base.scale, (80.0, 120.0));
        assert_eq!(trim.motion.map(|k| k.ticks), [26, 39, 55].map(|f| f * unit));
        assert_eq!(trim.width, [(9 * unit, 80.0), (31 * unit, 105.0), (55 * unit, 135.0)]);
        assert_eq!(trim.motion[1].position, [0.05, -0.025]);
        assert_eq!(trim.motion[1].height, 90.0);
        assert!(LinearMotion::prepare_premiere(&effect, 32, 24, 55, unit).is_err());
        assert!(LinearMotion::prepare_premiere(&effect, 32, 23, 56, unit).is_err());
    }

    #[test]
    fn resolve_linear_keys_use_the_exact_fractional_source_frame_grid() {
        let rate = Rational::new(24_000, 1001).unwrap();
        let mut keys = [
            MotionKey {
                at: Rational::new(1001, 4800).unwrap(),
                position: (0.0, 0.0),
                scale_percent: 100.0,
            },
            MotionKey {
                at: Rational::new(7007, 6000).unwrap(),
                position: (0.05, -0.025),
                scale_percent: 112.5,
            },
        ];
        let motion = LinearMotion::from_graph(&linear_effect(keys), rate, 5, 24, 29).unwrap();
        assert_eq!(motion.curves[0], [(5, 0.0), (28, 0.05)]);
        assert_eq!(motion.curves[1], [(5, 0.0), (28, 0.025)]);
        assert_eq!(motion.curves[2], [(5, 1.0), (28, 1.125)]);
        assert_eq!(motion.curves[2], motion.curves[3]);

        keys[1].at = Rational::new(7, 6).unwrap(); // Frame 28 at 24, between frames at 24000/1001.
        assert!(LinearMotion::from_graph(&linear_effect(keys), rate, 5, 24, 29).is_err());
    }
}
