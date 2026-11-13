//! Resolve Edit Inspector controls on a matched 1920x1080 canvas.
pub(crate) mod motion;

use crate::{
    binary::{pack, pb_bytes, pb_int},
    Result,
};
use graph::{picture_motion, Effect, Placement, Rational, Transform};
use graph::picture_chain::{Chain, Reflection as Axes};

pub(crate) struct PreparedVideo {
    pub(crate) encoded: String,
    pub(crate) readback: PictureControl,
}

#[derive(Debug, PartialEq)]
pub(crate) enum PictureControl {
    Transform(Transform),
    ReflectedTransform(Transform, Reflection),
    Reflection(Reflection),
    LinearMotion(motion::LinearMotion),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reflection {
    pub(crate) horizontal: bool,
    pub(crate) vertical: bool,
}

impl Reflection {
    fn from_effect(effect: &Effect) -> Result<Self> {
        if !Axes::is_effect_kind(&effect.kind) {
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
) -> Result<Option<PreparedVideo>> {
    match (&placement.transform, placement.effects.as_slice()) {
        (None, []) => Ok(None),
        (Some(transform), []) => prepare_static(transform.clone()),
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
        (None, [effect]) if effect.kind == graph::picture_chain::KIND => {
            prepare_chain(effect, source_in, length, available)
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

fn prepare_static(mut transform: Transform) -> Result<Option<PreparedVideo>> {
    Ok(encode(&transform)?.map(|encoded| {
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

fn prepare_chain(effect: &Effect, source_in: i64, length: i64, available: i64) -> Result<Option<PreparedVideo>> {
    let chain = Chain::read(effect)?;
    let reflection = Reflection::from(chain.before.then(chain.after));
    let Some(motion) = chain.motion else {
        return reflection.active().then(|| {
            Ok(PreparedVideo { encoded: reflection.encode()?, readback: PictureControl::Reflection(reflection) })
        }).transpose();
    };
    if motion.is_static() {
        let mut t = motion.transform;
        t.position = chain.after.point(t.position.into()).into();
        t.crop = chain.before.crop(t.crop);
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
        None => placement.transform.is_none() && placement.effects.is_empty(),
    }
}

pub(crate) const X: (usize, u64) = (3, 40);
pub(crate) const Y: (usize, u64) = (4, 41);
pub(crate) const SCALE_X: (usize, u64) = (0, 42);
pub(crate) const SCALE_Y: (usize, u64) = (1, 43);
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

pub(crate) fn encode(t: &Transform) -> Result<Option<String>> {
    encode_with_reflection(t, Reflection::default())
}

pub(crate) fn validate_static_geometry(t: &Transform) -> Result<()> {
    let (x, y) = t.position;
    let (sx, sy) = t.scale;
    if t.rotation != 0.0
        || t.anchor != (0.0, 0.0)
        || !(-0.05..=0.05).contains(&x)
        || !(-0.05..=0.05).contains(&y)
        || !(87.5..=112.5).contains(&sx)
        || !(87.5..=112.5).contains(&sy)
        || t.crop.iter().any(|v| !(0.0..=0.05).contains(v))
    {
        return Err("unsupported Resolve static picture geometry".into());
    }
    Ok(())
}

pub(crate) fn encode_with_reflection(t: &Transform, reflection: Reflection) -> Result<Option<String>> {
    validate_static_geometry(t)?;
    let (x, y) = t.position;
    let (sx, sy) = t.scale;
    let has_crop = t.crop != [0.0; 4];
    let has_transform = t.position != (0.0, 0.0) || t.scale != (100.0, 100.0);
    if has_crop && has_transform {
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
    ] {
        if value != neutral {
            motion[slot] = scalar(id, value);
        }
    }
    if t.scale != (100.0, 100.0) {
        motion[2] = parameter(44, &[0x28, 0]);
    }
    reflection.write_slots(&mut motion);
    section(4, 0, &motion, &mut body);
    if has_crop {
        let mut crop: [Vec<u8>; 6] = Default::default();
        for ((slot, id), value) in CROP.into_iter().zip(t.crop) {
            if value != 0.0 {
                crop[slot] = scalar(id, value);
            }
        }
        section(6, 6, &crop, &mut body);
    }
    Ok(Some(pack(&body)?))
}
