//! Resolve Edit Inspector controls on a matched 1920x1080 canvas.
pub(crate) mod motion;
pub(crate) mod roll;

use crate::{
    binary::{pack, pb_bytes, pb_int},
    Result,
};
use graph::{picture_motion, Effect, Placement, Rational, Transform};
use graph::picture_chain::Reflection as Axes;
use mapping::picture_chain::{is_reflection_kind, Chain};

pub(crate) struct PreparedVideo {
    pub(crate) encoded: String,
    pub(crate) readback: PictureControl,
}

pub(crate) const RETIME_MOTION_FLAGS: u64 = 20;
pub(crate) const RETIME_CROP_FLAGS: u64 = 26;

pub(crate) fn retime_motion_supported(t: &Transform) -> bool {
    if t.crop == [0.0; 4] {
        return validate_static_geometry(t).is_ok();
    }
    centered_crop_profile(t)
}

#[derive(Debug, PartialEq)]
pub(crate) enum PictureControl {
    Transform(Transform),
    ReflectedTransform(Transform, Reflection),
    Reflection(Reflection),
    LinearMotion(motion::LinearMotion),
    LinearRoll(roll::LinearRoll),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reflection {
    pub(crate) horizontal: bool,
    pub(crate) vertical: bool,
}

impl Reflection {
    fn from_effect(effect: &Effect) -> Result<Self> {
        if !is_reflection_kind(&effect.kind) {
            return Err("unsupported Resolve picture effect".into());
        }
        Axes::from_params(&effect.params)
            .map(Self::from)
            .ok_or_else(|| "invalid Resolve reflection flags".into())
    }

    pub(crate) fn readback(self) -> Effect {
        Axes { horizontal: self.horizontal, vertical: self.vertical }.effect()
    }

    pub(crate) fn write_slots(self, motion: &mut [Vec<u8>; 12]) {
        for ((slot, id), enabled) in [(FLIP_X, self.horizontal), (FLIP_Y, self.vertical)] {
            if enabled {
                motion[slot] = parameter(id, &[0x28, 1]);
            }
        }
    }

    fn encode(self) -> Result<String> {
        let mut motion: [Vec<u8>; 12] = Default::default();
        self.write_slots(&mut motion);
        let mut body = Vec::new();
        section(4, 0, &motion, &mut body);
        pack(&body)
    }
}

impl From<graph::picture_chain::Reflection> for Reflection {
    fn from(axes: graph::picture_chain::Reflection) -> Self {
        Self { horizontal: axes.horizontal, vertical: axes.vertical }
    }
}

impl Reflection {
    pub(crate) fn active(self) -> bool {
        self.horizontal || self.vertical
    }
}

pub(crate) fn prepare(
    placement: &Placement,
    rate: Rational,
    source_in: i64,
    length: i64,
    available: i64,
    linear_retime: bool,
) -> Result<Option<PreparedVideo>> {
    match (&placement.transform, placement.effects.as_slice()) {
        (None, []) => Ok(None),
        (Some(transform), []) => prepare_static(transform.clone(), rate, linear_retime),
        (Some(transform), [effect]) if motion::is_avid_linear(&effect.kind) => {
            let motion = motion::LinearMotion::from_avid(&effect.kind, &effect.params, transform, source_in, length, available)?;
            Ok(Some(PreparedVideo {
                encoded: motion.encode()?,
                readback: PictureControl::LinearMotion(motion),
            }))
        }
        (None, [effect]) if effect.kind == picture_motion::LINEAR_KIND => {
            let motion = motion::LinearMotion::from_graph(effect, rate, source_in, length, available)?;
            Ok(Some(PreparedVideo {
                encoded: motion.encode()?,
                readback: PictureControl::LinearMotion(motion),
            }))
        }
        (None, [effect]) if roll::is_linear(&effect.kind) => {
            if rate != Rational::integer(24) {
                return Err("Resolve Linear Warp rotation requires a 24 fps timeline".into());
            }
            let roll = roll::LinearRoll::from_graph(effect, source_in, length, available)?;
            Ok(Some(PreparedVideo {
                encoded: roll.encode()?,
                readback: PictureControl::LinearRoll(roll),
            }))
        }
        (None, [effect]) if effect.kind == graph::picture_chain::KIND => {
            prepare_chain(effect, rate, source_in, length, available)
        }
        (None, [effect]) => {
            let reflection = Reflection::from_effect(effect)?;
            Ok(Some(PreparedVideo {
                encoded: reflection.encode()?,
                readback: PictureControl::Reflection(reflection),
            }))
        }
        _ => Err("Resolve picture effect stack order is unqualified".into()),
    }
}

fn prepare_static(mut transform: Transform, rate: Rational, linear_retime: bool) -> Result<Option<PreparedVideo>> {
    // Linear retime preparation already validates the geometry and clocks.
    if !linear_retime {
        check_static_rate(&transform, rate)?;
    }
    Ok(encode_motion(&transform, Reflection::default(), linear_retime)?.map(|encoded| {
        transform.scale = (
            (transform.scale.0 / 100.0) * 100.0,
            (transform.scale.1 / 100.0) * 100.0,
        );
        PreparedVideo {
            encoded,
            readback: PictureControl::Transform(transform),
        }
    }))
}

fn prepare_chain(effect: &Effect, rate: Rational, source_in: i64, length: i64, available: i64) -> Result<Option<PreparedVideo>> {
    let chain = Chain::read(effect)?;
    let reflection = Reflection::from(chain.before.then(chain.after));
    let Some(motion) = chain.motion else {
        return reflection.active().then(|| {
            Ok(PreparedVideo { encoded: reflection.encode()?, readback: PictureControl::Reflection(reflection) })
        }).transpose();
    };
    if motion.is_static() {
        let mut t = motion.transform;
        if t.rotation != 0.0 && (chain.before.active() || chain.after.active()) {
            return Err("Resolve static rotation with reflection requires effect-order qualification".into());
        }
        t.position = chain.after.point(t.position.into()).into();
        t.crop = chain.before.crop(t.crop);
        if centered_crop_profile(&t) {
            return Err("Resolve centered crop in an ordered effect chain requires qualification".into());
        }
        check_static_rate(&t, rate)?;
        return Ok(encode_with_reflection(&t, reflection)?.map(|encoded| {
            t.scale = ((t.scale.0 / 100.0) * 100.0, (t.scale.1 / 100.0) * 100.0);
            let readback = if reflection.active() {
                if t.position == (0.0, 0.0) && t.scale == (100.0, 100.0) && t.crop == [0.0; 4] {
                    PictureControl::Reflection(reflection)
                }
                else { PictureControl::ReflectedTransform(t, reflection) }
            } else {
                PictureControl::Transform(t)
            };
            PreparedVideo { encoded, readback }
        }));
    }
    if !motion::is_avid_linear(motion.kind) {
        return Err("ordered picture chain uses an unqualified Resolve Motion stage".into());
    }
    let motion = motion::LinearMotion::from_avid(motion.kind, motion.params, &motion.transform, source_in, length, available)?
        .project_chain(chain.before, chain.after, source_in, length, available)?;
    Ok(Some(PreparedVideo { encoded: motion.encode()?, readback: PictureControl::LinearMotion(motion) }))
}

fn check_static_rate(transform: &Transform, rate: Rational) -> Result<()> {
    if rate != Rational::integer(24)
        && (transform.rotation != 0.0 || transform.anchor != (0.0, 0.0) || centered_crop_profile(transform))
    {
        return Err("Resolve static rotation, pivot and centered crop profiles require a 24 fps timeline".into());
    }
    Ok(())
}

pub(crate) fn matches_readback(placement: &Placement, video: Option<&PreparedVideo>) -> bool {
    match video.map(|video| &video.readback) {
        Some(PictureControl::Transform(transform)) => {
            placement.transform.as_ref() == Some(transform) && placement.effects.is_empty()
        }
        Some(PictureControl::ReflectedTransform(transform, reflection)) => {
            placement.transform.as_ref() == Some(transform) && placement.effects.as_slice() == [reflection.readback()]
        }
        Some(PictureControl::Reflection(reflection)) => {
            placement.transform.is_none() && placement.effects.as_slice() == [reflection.readback()]
        }
        Some(PictureControl::LinearMotion(motion)) => {
            placement.transform.is_none() && placement.effects.as_slice() == [motion.readback()]
        }
        Some(PictureControl::LinearRoll(roll)) => {
            placement.transform.is_none() && placement.effects.as_slice() == [roll.readback()]
        }
        None => placement.transform.is_none() && placement.effects.is_empty(),
    }
}

pub(crate) const X: (usize, u64) = (3, 40);
pub(crate) const Y: (usize, u64) = (4, 41);
pub(crate) const SCALE_X: (usize, u64) = (0, 42);
pub(crate) const SCALE_Y: (usize, u64) = (1, 43);
pub(crate) const ROTATION: (usize, u64) = (5, 47);
pub(crate) const ANCHOR: (usize, u64) = (6, 48);
pub(crate) const FLIP_X: (usize, u64) = (10, 50);
pub(crate) const FLIP_Y: (usize, u64) = (11, 51);
pub(crate) const CROP: [(usize, u64); 4] = [(0, 54), (2, 56), (1, 55), (3, 57)];

fn scalar(id: u64, value: f64) -> Vec<u8> {
    let mut point = vec![0x11];
    point.extend_from_slice(&value.to_le_bytes());
    parameter(id, &point)
}

fn parameter(id: u64, point: &[u8]) -> Vec<u8> {
    let mut curve = Vec::new();
    pb_bytes(1, point, &mut curve);
    let mut encoded = Vec::new();
    pb_int(1, id, &mut encoded);
    pb_bytes(3, &curve, &mut encoded);
    encoded
}

fn section<const N: usize>(effect: u64, flags: u64, slots: &[Vec<u8>; N], out: &mut Vec<u8>) {
    let mut bytes = Vec::new();
    pb_int(1, effect, &mut bytes);
    pb_int(7, flags, &mut bytes);
    for param in slots {
        pb_bytes(9, param, &mut bytes);
    }
    pb_bytes(1, &bytes, out);
}

pub(crate) fn validate_static_geometry(t: &Transform) -> Result<()> {
    let (x, y) = t.position;
    let (sx, sy) = t.scale;
    if t.anchor != (0.0, 0.0)
        || !(-0.05..=0.05).contains(&x)
        || !(-0.05..=0.05).contains(&y)
        || !(87.5..=112.5).contains(&sx)
        || !(87.5..=112.5).contains(&sy)
        || t.crop.iter().any(|v| !(0.0..=0.05).contains(v))
    {
        return Err("unsupported Resolve static picture geometry".into());
    }
    if t.rotation != 0.0
        && (!(-30.0..=30.0).contains(&t.rotation)
            || t.position != (0.0, 0.0)
            || t.scale != (100.0, 100.0)
            || t.crop != [0.0; 4])
    {
        return Err("Resolve rotation outside the centered static picture profiles".into());
    }
    Ok(())
}

pub(crate) fn fitted_motion(t: &Transform) -> bool {
    t.scale.0 == t.scale.1 && t.scale.0 > 0.0
        && t.rotation == 0.0 && t.anchor == (0.0, 0.0) && t.crop == [0.0; 4]
}

#[cfg(test)]
pub(crate) fn encode(t: &Transform) -> Result<Option<String>> {
    encode_with_reflection(t, Reflection::default())
}

pub(crate) fn encode_with_reflection(t: &Transform, reflection: Reflection) -> Result<Option<String>> {
    encode_motion(t, reflection, false)
}

fn encode_motion(t: &Transform, reflection: Reflection, linear_retime: bool) -> Result<Option<String>> {
    let centered_crop = centered_crop_profile(t);
    let axis = rotation_axis_profile(t);
    if !centered_crop && !axis {
        validate_static_geometry(t)?;
    } else if reflection.active() {
        return Err("Resolve crop or pivot with reflection requires effect-order qualification".into());
    }
    if t.rotation != 0.0 && reflection.active() {
        return Err("Resolve static rotation with reflection requires effect-order qualification".into());
    }
    // In the off-center profile graph position equals anchor, so native
    // Pan and Tilt stay at zero. Resolve rotates about Anchor Point itself.
    let (x, y) = if axis { (0.0, 0.0) } else { t.position };
    let (sx, sy) = t.scale;
    let has_crop = t.crop != [0.0; 4];
    let has_transform = t.position != (0.0, 0.0) || t.scale != (100.0, 100.0) || t.rotation != 0.0;
    if has_crop && has_transform && !centered_crop {
        return Err("combined Resolve transform and crop order is unqualified".into());
    }
    if !has_crop && !has_transform && !reflection.active() {
        return Ok(None);
    }
    let mut body = Vec::new();
    let mut motion: [Vec<u8>; 12] = Default::default();
    for ((slot, id), value, neutral) in [
        (X, x, 0.0),
        (Y, -y, 0.0),
        (SCALE_X, sx / 100.0, 1.0),
        (SCALE_Y, sy / 100.0, 1.0),
        (ROTATION, -t.rotation, 0.0),
    ] {
        if value != neutral {
            motion[slot] = scalar(id, value);
        }
    }
    if t.scale != (100.0, 100.0) {
        motion[2] = parameter(44, &[0x28, 0]);
    }
    if axis {
        // Resolve saves both anchor coordinates as one big-endian point.
        let mut xy = Vec::with_capacity(16);
        xy.extend_from_slice(&t.anchor.0.to_be_bytes());
        let y = if t.anchor.1 == 0.0 { 0.0 } else { -t.anchor.1 };
        xy.extend_from_slice(&y.to_be_bytes());
        let mut point = Vec::new();
        pb_bytes(7, &xy, &mut point);
        motion[ANCHOR.0] = parameter(ANCHOR.1, &point);
    }
    reflection.write_slots(&mut motion);
    // Native retimed crop-only edits save the crop section without an empty Motion section.
    if !linear_retime || has_transform {
        let motion_flags = if linear_retime { RETIME_MOTION_FLAGS } else { 0 };
        section(4, motion_flags, &motion, &mut body);
    }
    if has_crop {
        let mut crop: [Vec<u8>; 6] = Default::default();
        for ((slot, id), value) in CROP.into_iter().zip(t.crop) {
            if value != 0.0 {
                crop[slot] = scalar(id, value);
            }
        }
        let crop_flags = if linear_retime { RETIME_CROP_FLAGS } else { 6 };
        section(6, crop_flags, &crop, &mut body);
    }
    if linear_retime {
        pb_bytes(1, crate::retime::NEAREST_SECTION, &mut body);
    }
    Ok(Some(pack(&body)?))
}

/// A single off-center axis, with no translation, scale, or crop.
pub(crate) fn rotation_axis_profile(t: &Transform) -> bool {
    let (x, y) = t.anchor;
    t.anchor != (0.0, 0.0)
        && t.position == t.anchor
        && (x == 0.0 || y == 0.0)
        && (0.0..=(100.0_f64 / 999.0).next_up()).contains(&x)
        && (-(100.0_f64 / 999.0).next_up()..=0.0).contains(&y)
        && t.scale == (100.0, 100.0)
        && (0.0..=30.0).contains(&t.rotation)
        && t.crop == [0.0; 4]
}

/// Crop before a centered zoom and clockwise roll, measured on matching 1080p rasters.
pub(crate) fn centered_crop_profile(t: &Transform) -> bool {
    (t.crop.iter().any(|&v| v > 0.05)
        || (t.crop != [0.0; 4] && (t.scale != (100.0, 100.0) || t.rotation != 0.0)))
        && t.position == (0.0, 0.0)
        && t.anchor == (0.0, 0.0)
        && (60.0..=100.0).contains(&t.scale.0)
        && (90.0..=100.0).contains(&t.scale.1)
        && (0.0..=30.0).contains(&t.rotation)
        && t.crop.into_iter().zip([100.0, 80.0, 150.0, 120.0])
            .all(|(edge, max)| {
                // Resolve can round one ULP on save. Premiere stores crop
                // percentages at binary32 precision with 12 decimal places.
                let fraction = max / 999.0_f64;
                let premiere = (f64::from((fraction * 100.0) as f32) + 0.5e-12) / 100.0;
                let upper = fraction.next_up().max(premiere);
                (0.0..=upper).contains(&edge)
            })
}
