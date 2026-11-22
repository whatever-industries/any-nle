//! Resolve clip controls corroborated by native renders.
use super::*;
use crate::audio::{
    Envelope, FadePoint, Fades, Volume, VolumeKeys, EFFECT, HEAD, HEAD_POINT, KEY_FLAGS, TAIL, TAIL_POINT, VOLUME, VOLUME_DB,
};
use crate::video::{
    motion::{Curve, LinearMotion, CONTROLS, KEY_BYTES, KEY_FLAGS as VIDEO_KEY_FLAGS},
    roll::{self, LinearRoll},
    PictureControl, Reflection, ANCHOR, CROP, FLIP_X, FLIP_Y, ROTATION, SCALE_X, SCALE_Y, X, Y,
};
use encoding::{fields, Wire};
use graph::Transform;

pub(super) fn picture_control(
    encoded: &str,
    blobs: &mut Blobs,
    source_in: i64,
    length: i64,
    available: i64,
) -> Result<Option<PictureControl>> {
    let bytes = blobs.envelope(encoded)?;
    let outer = fields(&bytes)?;
    let (motion, crop) = match outer.as_slice() {
        [(1, Wire::Bytes(section))] if section.starts_with(&[8, 6]) => (None, Some(*section)),
        [(1, Wire::Bytes(motion))] => (Some(*motion), None),
        [(1, Wire::Bytes(motion)), (1, Wire::Bytes(crop))] => (Some(*motion), Some(*crop)),
        _ => return Ok(None),
    };
    let motion = if let Some(motion) = motion {
        let Some(slots) = section_slots(motion, 4, 0)? else { return Ok(None) };
        slots
    } else {
        vec![&[][..]; 12]
    };
    if motion.len() != 12 {
        return Ok(None);
    }
    let first_reflection_slot = FLIP_X.0;
    let (Some(horizontal), Some(vertical)) =
        (reflection_slot(&motion, FLIP_X)?, reflection_slot(&motion, FLIP_Y)?)
    else {
        return Ok(None);
    };
    let reflection = Reflection { horizontal, vertical };
    if reflection.active() && crop.is_none() && motion[..first_reflection_slot].iter().all(|slot| slot.is_empty()) {
        return Ok(Some(PictureControl::Reflection(reflection)));
    }
    if motion[ANCHOR.0 + 1..first_reflection_slot].iter().any(|slot| !slot.is_empty()) {
        return Ok(None);
    }
    if !matches!(motion[2], [] | [8, 44, 26, 4, 10, 2, 40, 0]) {
        return Ok(None);
    }
    let has_anchor = !motion[ANCHOR.0].is_empty();
    if has_anchor && !motion[2].is_empty() {
        return Ok(None);
    }
    if !motion[2].is_empty()
        && !motion[ROTATION.0].is_empty()
        && crop.is_none()
        && !reflection.active()
        && motion[SCALE_Y.0].is_empty()
        && motion[Y.0].is_empty()
    {
        let [pan, zoom, angle] = roll::CONTROLS;
        if let (Some(pan), Some(zoom), Some(angle)) = (
            linear_curve(motion[pan.0], pan.1, (0.0, 3))?,
            linear_curve(motion[zoom.0], zoom.1, (1.0, 3))?,
            linear_curve(motion[angle.0], angle.1, (0.0, 3))?,
        ) {
            return Ok(LinearRoll::from_saved([pan, zoom, angle], source_in, length, available)
                .map(PictureControl::LinearRoll));
        }
    }
    if !motion[2].is_empty() && motion[ROTATION.0].is_empty() {
        let mut curves: [Curve; 4] = Default::default();
        let mut all_animated = true;
        for (((slot, id), neutral), curve) in CONTROLS
            .into_iter()
            .zip([0.0, 0.0, 1.0, 1.0])
            .zip(&mut curves)
        {
            let Some(keys) = linear_curve(motion[slot], id, (neutral, 2))? else {
                all_animated = false;
                break;
            };
            *curve = keys;
        }
        if all_animated {
            let crop_curves = if let Some(crop) = crop {
                let Some(slots) = source_crop_slots(crop, 6)? else {
                    return Ok(None);
                };
                let mut edges: [Curve; 4] = Default::default();
                for ((slot, id), edge) in CROP.into_iter().zip(&mut edges) {
                    let Some(keys) = linear_curve(slots[slot], id, (0.0, 3))? else {
                        return Ok(None);
                    };
                    *edge = keys;
                }
                Some(edges)
            } else {
                None
            };
            return Ok(LinearMotion::from_saved(curves, crop_curves, reflection, source_in, length, available)
                .map(PictureControl::LinearMotion));
        }
    }
    let Some(mut t) = static_motion(&motion, false)? else {
        return Ok(None);
    };
    if let Some(crop) = crop {
        let Some(edges) = static_crop(crop, 6)? else {
            return Ok(None);
        };
        t.crop = edges;
    }
    let Some(anchor) = anchor_slot(motion[ANCHOR.0])? else {
        return Ok(None);
    };
    if has_anchor && t.position != (0.0, 0.0) {
        return Ok(None);
    }
    if has_anchor {
        t.position = anchor;
    }
    t.anchor = anchor;
    let centered_crop = crate::video::centered_crop_profile(&t);
    if has_anchor && !crate::video::rotation_axis_profile(&t) {
        return Ok(None);
    }
    if t.rotation != 0.0 && !motion[2].is_empty() && !centered_crop {
        return Ok(None);
    }
    if reflection.active() {
        return Ok(crate::video::encode_with_reflection(&t, reflection).ok()
            .map(|_| PictureControl::ReflectedTransform(t, reflection)));
    }
    Ok((has_anchor || centered_crop || crate::video::validate_static_geometry(&t).is_ok())
        .then_some(PictureControl::Transform(t)))
}

/// A Linear retime with static picture geometry and frame sampling.
pub(super) fn retime_motion(encoded: &str, blobs: &mut Blobs) -> Result<Option<Transform>> {
    let bytes = blobs.envelope(encoded)?;
    let outer = fields(&bytes)?;
    let (motion, crop, sampling) = match outer.as_slice() {
        [(1, Wire::Bytes(crop)), (1, Wire::Bytes(sampling))] if crop.starts_with(&[8, 6]) =>
            (None, Some(*crop), *sampling),
        [(1, Wire::Bytes(motion)), (1, Wire::Bytes(sampling))] =>
            (Some(*motion), None, *sampling),
        [(1, Wire::Bytes(motion)), (1, Wire::Bytes(crop)), (1, Wire::Bytes(sampling))] =>
            (Some(*motion), Some(*crop), *sampling),
        _ => return Ok(None),
    };
    if sampling != crate::retime::NEAREST_SECTION {
        return Ok(None);
    }
    let mut transform = if let Some(motion) = motion {
        let Some(slots) = section_slots(motion, 4, crate::video::RETIME_MOTION_FLAGS)? else {
            return Ok(None);
        };
        if slots.len() != 12 || slots[6..].iter().any(|slot| !slot.is_empty())
            || !matches!(slots[2], [] | [8, 44, 26, 4, 10, 2, 40, 0])
        {
            return Ok(None);
        }
        let Some(transform) = static_motion(&slots, false)? else {
            return Ok(None);
        };
        if crop.is_none() && transform.rotation != 0.0 && !slots[2].is_empty() {
            return Ok(None);
        }
        transform
    } else {
        Transform {
            position: (0.0, 0.0),
            scale: (100.0, 100.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            crop: [0.0; 4],
        }
    };
    if let Some(crop) = crop {
        let Some(edges) = static_crop(crop, crate::video::RETIME_CROP_FLAGS)? else {
            return Ok(None);
        };
        transform.crop = edges;
    }
    Ok(crate::video::retime_motion_supported(&transform).then_some(transform))
}

/// A single static scale and translation stage, independent of input fitting.
pub(super) fn fitted_static_motion(encoded: &str, blobs: &mut Blobs) -> Result<Option<Transform>> {
    let bytes = blobs.envelope(encoded)?;
    let outer = fields(&bytes)?;
    let [(1, Wire::Bytes(motion))] = outer.as_slice() else { return Ok(None) };
    let Some(slots) = section_slots(motion, 4, 0)? else { return Ok(None) };
    if slots.len() != 12 || slots[5..].iter().any(|slot| !slot.is_empty())
        || !matches!(slots[2], [] | [8, 44, 26, 4, 10, 2, 40, 0])
    {
        return Ok(None);
    }
    Ok(static_motion(&slots, true)?.filter(crate::video::fitted_motion))
}

fn static_motion(slots: &[&[u8]], independent_uniform: bool) -> Result<Option<Transform>> {
    let (Some(x), Some(y), Some(sx), Some(sy), Some(rotation)) = (
        static_slot(slots, X, 0.0)?,
        static_slot(slots, Y, 0.0)?,
        static_slot(slots, SCALE_X, 1.0)?,
        static_slot(slots, SCALE_Y, 1.0)?,
        static_slot(slots, ROTATION, 0.0)?,
    ) else {
        return Ok(None);
    };
    if (sx != 1.0 || sy != 1.0) && slots[2].is_empty() && !(independent_uniform && sx == sy) {
        return Ok(None);
    }
    Ok(Some(Transform {
        position: (x, -y),
        scale: (sx * 100.0, sy * 100.0),
        rotation: if rotation == 0.0 { 0.0 } else { -rotation },
        anchor: (0.0, 0.0),
        crop: [0.0; 4],
    }))
}

fn static_crop(crop: &[u8], flags: u64) -> Result<Option<[f64; 4]>> {
    let Some(slots) = source_crop_slots(crop, flags)? else {
        return Ok(None);
    };
    let mut edges = [0.0; 4];
    for (edge, control) in edges.iter_mut().zip(CROP) {
        let Some(value) = static_slot(&slots, control, 0.0)? else {
            return Ok(None);
        };
        *edge = value;
    }
    Ok(Some(edges))
}

fn source_crop_slots(crop: &[u8], flags: u64) -> Result<Option<Vec<&[u8]>>> {
    // Softness and Retain Image Position must be absent for source-space crop.
    Ok(section_slots(crop, 6, flags)?
        .filter(|slots| slots.len() == 6 && slots[4..].iter().all(|slot| slot.is_empty())))
}

fn linear_curve(data: &[u8], id: u64, implicit_base: (f64, usize)) -> Result<Option<Curve>> {
    let fields = fields(data)?;
    let (actual, base, keys) = match fields.as_slice() {
        [(1, Wire::Int(id)), (3, Wire::Bytes(base)), (10, Wire::Bytes(keys))] =>
            (*id, scalar(base)?, *keys),
        // Native saves omit scalar bases on the observed neutral-start curves.
        [(1, Wire::Int(id)), (10, Wire::Bytes(keys))] => (*id, None, *keys),
        _ => return Ok(None),
    };
    let Some((header, bytes)) = keys.split_at_checked(4) else {
        return Ok(None);
    };
    let payload = match bytes.len() {
        32 => -32i32,
        48 => -48i32,
        _ => return Ok(None),
    };
    if actual != id || header != payload.to_be_bytes() {
        return Ok(None);
    }
    let mut curve = Vec::with_capacity(bytes.len() / KEY_BYTES);
    for bytes in bytes.chunks_exact(KEY_BYTES) {
        let frame = u32::from_le_bytes(bytes[..4].try_into()?);
        let flags = u32::from_le_bytes(bytes[4..8].try_into()?);
        let value = f64::from_le_bytes(bytes[8..].try_into()?);
        if flags != VIDEO_KEY_FLAGS || !value.is_finite() {
            return Ok(None);
        }
        curve.push((frame, value));
    }
    // Resolve's UI can quantize the initial key while retaining a double base.
    let base_matches = if fields.len() == 2 {
        let (neutral, count) = implicit_base;
        curve.len() == count && curve[0].1 == neutral
    } else {
        base.is_some_and(|value| (value - curve[0].1).abs() <= 1e-7)
    };
    Ok(base_matches.then_some(curve))
}

fn reflection_slot(slots: &[&[u8]], (slot, id): (usize, u64)) -> Result<Option<bool>> {
    if slots[slot].is_empty() {
        return Ok(Some(false));
    }
    Ok(matches!(
        fields(slots[slot])?.as_slice(),
        [(1, Wire::Int(actual)), (3, Wire::Bytes([0x0a, 2, 0x28, 1]))] if *actual == id
    ).then_some(true))
}

fn static_slot(slots: &[&[u8]], (slot, id): (usize, u64), neutral: f64) -> Result<Option<f64>> {
    if slots[slot].is_empty() {
        Ok(Some(neutral))
    } else {
        static_parameter(slots[slot], id)
    }
}

fn anchor_slot(data: &[u8]) -> Result<Option<(f64, f64)>> {
    if data.is_empty() {
        return Ok(Some((0.0, 0.0)));
    }
    let params = fields(data)?;
    let [(1, Wire::Int(id)), (3, Wire::Bytes(curve))] = params.as_slice() else {
        return Ok(None);
    };
    if *id != ANCHOR.1 {
        return Ok(None);
    }
    let keys = fields(curve)?;
    let [(1, Wire::Bytes(key))] = keys.as_slice() else {
        return Ok(None);
    };
    let point = fields(key)?;
    let [(7, Wire::Bytes(xy))] = point.as_slice() else {
        return Ok(None);
    };
    if xy.len() != 16 {
        return Ok(None);
    }
    let (x, y) = xy.split_at(8);
    let x = f64::from_be_bytes(x.try_into()?);
    let y = f64::from_be_bytes(y.try_into()?);
    Ok((x.is_finite() && y.is_finite()).then_some((x, if y == 0.0 { 0.0 } else { -y })))
}

#[derive(Default)]
pub(super) struct ClipAudio {
    pub(super) envelope: Option<Envelope>,
    pub(super) pan: Option<f64>,
}

pub(super) fn clip_audio(
    encoded: &str,
    blobs: &mut Blobs,
    edit: Edit,
) -> Result<Option<ClipAudio>> {
    let bytes = blobs.envelope(encoded)?;
    let outer = fields(&bytes)?;
    let (first, second) = match outer.as_slice() {
        [(1, Wire::Bytes(first))] => (*first, None),
        [(1, Wire::Bytes(first)), (1, Wire::Bytes(second))] => (*first, Some(*second)),
        _ => return Ok(None),
    };
    let mut audio = ClipAudio::default();
    let pan = if let Some(slots) = section_slots(first, EFFECT, 0)? {
        let Some(envelope) = clip_envelope(&slots, edit)? else {
            return Ok(None);
        };
        audio.envelope = Some(envelope);
        second
    } else if second.is_none() {
        Some(first)
    } else {
        return Ok(None);
    };
    if let Some(pan) = pan {
        let Some(slots) = section_slots(pan, 144, 2)? else {
            return Ok(None);
        };
        let [slot] = slots.as_slice() else {
            return Ok(None);
        };
        audio.pan = static_parameter(slot, 96)?.filter(|value| (-100.0..=100.0).contains(value));
        if audio.pan.is_none() {
            return Ok(None);
        }
    }
    Ok(Some(audio))
}

fn clip_envelope(slots: &[&[u8]], edit: Edit) -> Result<Option<Envelope>> {
    let [volume, head, tail, in_shape, out_shape] = slots else {
        return Ok(None);
    };
    let volume = if volume.is_empty() {
        None
    } else if let Some(db) = static_parameter(volume, VOLUME)?.filter(|db| VOLUME_DB.contains(db)) {
        Some(Volume::Static(db))
    } else if let Some(keys) = volume_keys(volume, edit)? {
        Some(Volume::Keys(keys))
    } else {
        return Ok(None);
    };
    let fades = if [head, tail, in_shape, out_shape].iter().all(|slot| slot.is_empty()) {
        None
    } else {
        let (Some(head), Some(tail)) = (
            fade_length_value(head, HEAD, edit.length)?,
            fade_length_value(tail, TAIL, edit.length)?,
        ) else {
            return Ok(None);
        };
        let head_point = fade_point(in_shape, HEAD_POINT)?
            .and_then(|(position, level)| FadePoint::new(position, level));
        let tail_point = fade_point(out_shape, TAIL_POINT)?
            .and_then(|(position, level)| FadePoint::new(position, level));
        if (!in_shape.is_empty() && head_point.is_none()) || (!out_shape.is_empty() && tail_point.is_none()) {
            return Ok(None);
        }
        let fades = Fades { head, tail, head_point, tail_point };
        if !fades.valid_for(edit.length)
            || (fades.fractional() && matches!(volume, Some(Volume::Keys(_))))
        {
            return Ok(None);
        }
        Some(fades)
    };
    Ok((volume.is_some() || fades.is_some()).then_some(Envelope { volume, fades }))
}

/// Capture documented custom fade control points without assigning a
/// between-point interpolation law or converting them to default fades.
pub(super) fn custom_fades(encoded: &str, blobs: &mut Blobs, edit: Edit) -> Result<Option<Value>> {
    let bytes = blobs.envelope(encoded)?;
    let Some(slots) = control_slots(&bytes, EFFECT)? else { return Ok(None) };
    let [volume, head, tail, in_shape, out_shape] = slots.as_slice() else { return Ok(None) };
    let mut data = vec![
        ("provenance".into(), text("drpkit:af3cedc0 documented fade points")),
        ("clock".into(), text("clip_edges")),
    ];
    let mut seen = false;
    for (edge, duration_id, point_id, duration, shape) in [
        ("in", HEAD, HEAD_POINT, *head, *in_shape),
        ("out", TAIL, TAIL_POINT, *tail, *out_shape),
    ] {
        let Some(length) = fade_length(duration, duration_id, edit.length)? else { continue };
        if !duration.is_empty() {
            data.push((format!("{edge}_frames"), Value::Int(length)));
        }
        if shape.is_empty() {
            continue;
        }
        if let Some((position, level)) = fade_point(shape, point_id)?
            .filter(|(position, level)| length > 0 && (0.0..=100.0).contains(position) && level.is_finite())
        {
            data.push((format!("{edge}_point"), Value::Obj(vec![
                ("position_percent".into(), Value::Float(position)),
                ("relative_tenths_db".into(), Value::Float(level)),
            ])));
            seen = true;
        }
    }
    if let Some(db) = static_parameter(volume, VOLUME)? {
        data.push(("clip_volume_db".into(), Value::Float(db)));
    }
    Ok(seen.then_some(Value::Obj(data)))
}

fn fade_point(shape: &[u8], expected: u64) -> Result<Option<(f64, f64)>> {
    let params = fields(shape)?;
    let [(1, Wire::Int(id)), (3, Wire::Bytes(curve))] = params.as_slice() else {
        return Ok(None);
    };
    if *id != expected {
        return Ok(None);
    }
    let curve = fields(curve)?;
    let [(1, Wire::Bytes(point))] = curve.as_slice() else {
        return Ok(None);
    };
    let point = fields(point)?;
    let [(7, Wire::Bytes(point))] = point.as_slice() else {
        return Ok(None);
    };
    if point.len() != 16 {
        return Ok(None);
    }
    Ok(Some((
        f64::from_be_bytes(point[..8].try_into()?),
        f64::from_be_bytes(point[8..].try_into()?),
    )))
}

fn fade_length(data: &[u8], id: u64, limit: i64) -> Result<Option<i64>> {
    Ok(fade_length_value(data, id, limit)?.filter(|n| n.fract() == 0.0).map(|n| n as i64))
}

fn fade_length_value(data: &[u8], id: u64, limit: i64) -> Result<Option<f64>> {
    if data.is_empty() {
        return Ok(Some(0.0));
    }
    Ok(static_parameter(data, id)?.filter(|n| *n > 0.0 && *n <= limit as f64))
}

fn volume_keys(data: &[u8], edit: Edit) -> Result<Option<VolumeKeys>> {
    let parameters = fields(data)?;
    let [(1, Wire::Int(VOLUME)), (3, Wire::Bytes(first)), (10, Wire::Bytes(keys))] =
        parameters.as_slice()
    else {
        return Ok(None);
    };
    if keys.len() != 36 || keys[..4] != (-32i32).to_be_bytes() {
        return Ok(None);
    }
    let mut pair = [(0, 0.0); 2];
    for (entry, bytes) in pair.iter_mut().zip(keys[4..].chunks_exact(16)) {
        let frame = u32::from_le_bytes(bytes[..4].try_into()?);
        let flags = u32::from_le_bytes(bytes[4..8].try_into()?);
        let db = f64::from_le_bytes(bytes[8..].try_into()?);
        if flags != KEY_FLAGS
            || !VOLUME_DB.contains(&db)
            || !(edit.source_in..edit.source_in + edit.length).contains(&i64::from(frame))
        {
            return Ok(None);
        }
        *entry = (frame, db);
    }
    Ok((pair[0].0 < pair[1].0 && scalar(first)? == Some(pair[0].1)).then_some(pair))
}

fn control_slots(bytes: &[u8], effect_id: u64) -> Result<Option<Vec<&[u8]>>> {
    let outer = fields(bytes)?;
    let [(1, Wire::Bytes(section))] = outer.as_slice() else {
        return Ok(None);
    };
    section_slots(section, effect_id, 0)
}

fn section_slots(bytes: &[u8], effect_id: u64, flags: u64) -> Result<Option<Vec<&[u8]>>> {
    let section = fields(bytes)?;
    let [(1, Wire::Int(section_id)), (7, Wire::Int(saved_flags)), slots @ ..] = section.as_slice()
    else {
        return Ok(None);
    };
    if *section_id != effect_id || *saved_flags != flags {
        return Ok(None);
    }
    Ok(slots
        .iter()
        .map(|(key, wire)| match (key, wire) {
            (9, Wire::Bytes(bytes)) => Some(*bytes),
            _ => None,
        })
        .collect())
}

fn static_parameter(data: &[u8], parameter_id: u64) -> Result<Option<f64>> {
    // The observed static curve has one value and no key time or tangent fields.
    let parameters = fields(data)?;
    let [(1, Wire::Int(id)), (3, Wire::Bytes(curve))] = parameters.as_slice() else {
        return Ok(None);
    };
    if *id != parameter_id {
        return Ok(None);
    }
    scalar(curve)
}

fn scalar(curve: &[u8]) -> Result<Option<f64>> {
    let curve = fields(curve)?;
    let [(1, Wire::Bytes(point))] = curve.as_slice() else {
        return Ok(None);
    };
    let point = fields(point)?;
    let [(2, Wire::Fixed(x))] = point.as_slice() else {
        return Ok(None);
    };
    let Ok(bytes) = <[u8; 8]>::try_from(*x) else {
        return Ok(None);
    };
    let x = f64::from_le_bytes(bytes);
    Ok(x.is_finite().then_some(x))
}

pub(super) fn report_group_controls(
    node: &Node,
    owner: &str,
    graph: &mut SourceGraph,
) -> Result<()> {
    if node
        .maybe("EffectFiltersBA")?
        .is_some_and(|value| !value.is_empty())
    {
        omitted(
            graph,
            owner,
            node.id()?,
            "effect",
            "Resolve Group effect controls are unqualified",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopened_warp_roll_keeps_linear_keys_and_rejects_other_interpolation() {
        let native = concat!(
            "00000002000000a98128b52ffd20f6fd040014070af301080438004a45082a1a0b0a091166e63f5234ffffff",
            "d0000c0066e63f05000c00f03f0c000c009a99e93f4a004a08082c1a040a0228004a45082803ec813331c0a",
            "ebf0271a1e62410b79ebf4a004a45082f002e402e4005000c0039c00c000c0024404a004a004a004a004a004a",
            "001500603008040c068160004c90880b63c28da1c6026501899c0400c9594102880c02018341107003b453004b",
        );
        let decode = |blob| picture_control(blob, &mut Blobs::default(), 0, 13, 240).unwrap();
        let Some(PictureControl::LinearRoll(roll)) = decode(native) else {
            panic!("expected reopened Linear Warp rotation");
        };
        let effect = roll.readback();
        assert_eq!(effect.params.get("rotation"), Some(&Value::Arr(
            [(0, 15.0), (5, -25.0), (12, 10.0)].into_iter().map(|(frame, angle)| {
                Value::Arr(vec![Value::Int(frame), Value::Float(angle)])
            }).collect()
        )));
        assert_eq!(LinearRoll::from_graph(&effect, 0, 13, 240).unwrap(), roll);
        assert!(LinearRoll::from_graph(&effect, 1, 13, 240).is_err());
        assert!(LinearRoll::from_graph(&effect, 0, 13, 12).is_err());

        use prproj::convert::prepared::{PictureEffect, WarpRollInterpolation};
        let PictureEffect::WarpRoll { keys, interpolation: WarpRollInterpolation::Linear } =
            roll.prepare_premiere().unwrap()
        else {
            panic!("expected editable Premiere Motion with Linear keys");
        };
        assert_eq!(keys.map(|key| key.ticks), [0, 52_920_000_000, 127_008_000_000]);
        assert_eq!(keys.map(|key| key.x), [-60.0 / 999.0, 90.0 / 999.0, -30.0 / 999.0]);
        assert_eq!(keys.map(|key| key.width), [70.0, 100.0, 80.0]);
        assert_eq!(keys.map(|key| key.rotation), [-15.0, 25.0, -10.0]);

        let mut body = crate::binary::unpack(native).unwrap();
        let key = [8, 47, 26, 11, 10, 9, 17];
        let offset = body.windows(key.len()).position(|bytes| bytes == key).unwrap();
        let flag = offset + key.len() + 8 + 6 + 4;
        assert_eq!(&body[flag..flag + 4], &VIDEO_KEY_FLAGS.to_le_bytes());
        body[flag] = 1;
        let altered = crate::binary::pack(&body).unwrap();
        assert!(decode(&altered).is_none());
    }

    #[test]
    fn retime_motion_requires_the_saved_nearest_section() {
        let motion = "080438144a0f082a1a0b0a0911000000000000f13f4a0f082b1a0b0a0911000000000000f13f4a08082c1a040a0228004a0f08281a0b0a09119a9999999999993f4a004a004a004a004a004a004a004a00";
        let mut body = Vec::new();
        crate::binary::pb_bytes(1, &hex::decode(motion).unwrap(), &mut body);
        let motion_only = crate::binary::pack(&body).unwrap();
        assert!(retime_motion(&motion_only, &mut Blobs::default()).unwrap().is_none());

        crate::binary::pb_bytes(1, crate::retime::NEAREST_SECTION, &mut body);
        let combined = crate::binary::pack(&body).unwrap();
        let transform = retime_motion(&combined, &mut Blobs::default()).unwrap().unwrap();
        assert_eq!(transform.position, (0.025, 0.0));
        assert_eq!(transform.scale, (106.25, 106.25));

        crate::binary::pb_bytes(1, crate::retime::NEAREST_SECTION, &mut body);
        let ambiguous = crate::binary::pack(&body).unwrap();
        assert!(retime_motion(&ambiguous, &mut Blobs::default()).unwrap().is_none());
    }

    #[test]
    fn retime_motion_preserves_centered_rotation() {
        let motion = "080438144a004a004a004a004a004a0f082f1a0b0a09110000000000002ec04a004a004a004a004a004a00";
        let mut body = Vec::new();
        crate::binary::pb_bytes(1, &hex::decode(motion).unwrap(), &mut body);
        crate::binary::pb_bytes(1, crate::retime::NEAREST_SECTION, &mut body);
        let encoded = crate::binary::pack(&body).unwrap();
        let rotation = retime_motion(&encoded, &mut Blobs::default()).unwrap().unwrap();
        assert_eq!(rotation.rotation, 15.0);
        assert_eq!(rotation.position, (0.0, 0.0));
        assert_eq!(rotation.scale, (100.0, 100.0));
    }

    #[test]
    fn retime_crop_preserves_edges_and_refuses_retain_position() {
        let crop = hex::decode("0806381a4a004a004a0f08381a0b0a0911b81e85eb51b8ae3f4a0f08391a0b0a09119a9999999999b93f4a004a00").unwrap();
        let decode = |crop: &[u8]| {
            let mut body = Vec::new();
            crate::binary::pb_bytes(1, crop, &mut body);
            crate::binary::pb_bytes(1, crate::retime::NEAREST_SECTION, &mut body);
            let encoded = crate::binary::pack(&body).unwrap();
            retime_motion(&encoded, &mut Blobs::default()).unwrap()
        };
        let transform = decode(&crop).unwrap();
        assert_eq!(transform.crop, [0.0, 0.06, 0.0, 0.10]);
        assert_eq!(transform.scale, (100.0, 100.0));

        let mut retain = crop;
        retain.truncate(retain.len() - 2);
        crate::binary::pb_bytes(9, &[1], &mut retain);
        assert!(decode(&retain).is_none());
    }

    #[test]
    fn retimed_crop_scale_and_rotation_require_the_saved_section_order() {
        let motion = hex::decode(concat!(
            "080438144a0f082a1a0b0a0911cdccccccccccec3f4a0f082b1a0b0a0911666666666666ee3f",
            "4a08082c1a040a0228004a004a004a0f082f1a0b0a09110000000000002ec04a004a004a004a004a004a00",
        )).unwrap();
        let crop = hex::decode("0806381a4a004a004a0f08381a0b0a0911b81e85eb51b8ae3f4a0f08391a0b0a09119a9999999999b93f4a004a00").unwrap();
        let decode = |first: &[u8], second: &[u8]| {
            let mut body = Vec::new();
            for section in [first, second, crate::retime::NEAREST_SECTION] {
                crate::binary::pb_bytes(1, section, &mut body);
            }
            let encoded = crate::binary::pack(&body).unwrap();
            retime_motion(&encoded, &mut Blobs::default()).unwrap()
        };
        let transform = decode(&motion, &crop).unwrap();
        assert_eq!(transform.scale, (90.0, 95.0));
        assert_eq!(transform.rotation, 15.0);
        assert_eq!(transform.crop, [0.0, 0.06, 0.0, 0.10]);
        assert!(decode(&crop, &motion).is_none());
    }

    #[test]
    fn native_xy_pivot_round_trips_with_no_pan_or_tilt() {
        let native = "00000002000000468128b52ffd2045e5010014030a43080438004a0f082f1a0b0a091100003ec04a1808301a140a123a1000003fb9a02900419a034a004a004a004a004a000300601408a8cd1901";
        let decode = |encoded| picture_control(encoded, &mut Blobs::default(), 0, 1, 48).unwrap();
        let Some(PictureControl::Transform(transform)) = decode(native) else {
            panic!("expected static Rotation Angle and Anchor Point");
        };
        assert_eq!(transform.rotation, 30.0);
        assert!((transform.anchor.1 + 100.0 / 999.0).abs() < f64::EPSILON);
        assert_eq!(transform.position, transform.anchor);
        assert_eq!(transform.anchor.0, 0.0);
        let encoded = crate::video::encode(&transform).unwrap().unwrap();
        assert_eq!(decode(&encoded), Some(PictureControl::Transform(transform.clone())));

        let mut translated = transform;
        translated.position.0 = 0.01;
        assert!(crate::video::encode(&translated).is_err());
    }

    #[test]
    fn native_centered_crop_with_zoom_and_roll_keeps_source_edges() {
        let native = "000000020000008e8128b52ffd20a1250400d2461a24902b1df7fb44e49c21d0ebdf1903ff3fe627822445976f4c6dc357b59101d809d9dc560678bc6e5ed42ad37238af194a41024494735e398ef0a2666938af1acc012117c570fe718f25ec3d70e13c1e8f06c516a4b0afd3eeee4a09e7b5d30601268010cea35b9eb00a004012b844b325a304e612e06c8813a650876936480060";
        let retain = "00000002000000968128b52ffd20aa65040062471c25902b1df7fbc4a6a6d1f8f4d5ecbfdd9dee950892145d4e30b52d56fc8bc8443b217bed2d030b62aebd45f1f3fc66a64892546d61fd5e22036a3918b47e35503045d24461fd622c846c5458c27a073a86b8899108ebf3f984986bef607fab18638c94c1fadd4881c7015ec1fa84cd0f170a004012b844b325a304e612e06c8813a650876936480060";
        let decode = |encoded| picture_control(encoded, &mut Blobs::default(), 0, 1, 48).unwrap();
        let Some(PictureControl::Transform(transform)) = decode(native) else {
            panic!("expected native crop, zoom and rotation");
        };
        assert_eq!(transform.rotation, 30.0);
        assert_eq!(transform.scale, (60.0, 90.0));
        for (edge, expected) in transform.crop.into_iter().zip([100.0, 80.0, 150.0, 120.0]) {
            assert!((edge - expected / 999.0).abs() < f64::EPSILON);
        }
        let encoded = crate::video::encode(&transform).unwrap().unwrap();
        assert_eq!(decode(&encoded), Some(PictureControl::Transform(transform)));
        assert!(decode(retain).is_none());
    }

    #[test]
    fn native_static_rotation_preserves_angle_and_refuses_unqualified_geometry() {
        let native = "000000020000002e800a2b080438004a004a004a004a004a004a0f082f1a0b0a09110000000000003e404a004a004a004a004a004a00";
        let decode = |encoded| picture_control(encoded, &mut Blobs::default(), 0, 1, 48).unwrap();
        let Some(PictureControl::Transform(transform)) = decode(native) else {
            panic!("expected native Rotation Angle");
        };
        assert_eq!(transform.rotation, -30.0);
        let encoded = crate::video::encode(&transform).unwrap().unwrap();
        assert_eq!(decode(&encoded), Some(PictureControl::Transform(transform.clone())));

        let wrong_id = native.replacen("082f", "0830", 1);
        assert!(decode(&wrong_id).is_none());
        let mut unqualified = transform.clone();
        unqualified.position.0 = 0.02;
        assert!(crate::video::encode(&unqualified).is_err());
        let reflection = Reflection { horizontal: true, vertical: false };
        assert!(crate::video::encode_with_reflection(&transform, reflection).is_err());
    }

    #[test]
    fn reopened_linear_motion_keeps_neutral_start_keys_without_scalar_bases() {
        let native = concat!(
            "00000002000000698128b52ffd20c7fd0200a284111ea035e9cf7fdd55f43fde054f5555d1db30ccb1",
            "ee76c52d18214ea16c3605f7de7befcd7158f66466a6f378d851508950ee92c3b2c0eb82015e0f",
            "516bcd6090c3bea40b2015090080104d89045eec2afa6baa1480185802060f4018",
        );
        let Some(PictureControl::LinearMotion(motion)) =
            picture_control(native, &mut Blobs::default(), 5, 24, 48).unwrap()
        else {
            panic!("expected reopened Motion");
        };
        let effect = motion.readback();
        for (name, neutral, last) in [
            ("pan", 0.0, 0.05),
            ("tilt", 0.0, 0.05),
            ("zoom_x", 1.0, 1.125),
            ("zoom_y", 1.0, 1.125),
        ] {
            let Some(Value::Arr(keys)) = effect.params.get(name) else {
                panic!("missing {name} keys");
            };
            assert_eq!(keys[0], Value::Arr(vec![Value::Int(5), Value::Float(neutral)]));
            let Value::Arr(end) = &keys[1] else {
                panic!("missing {name} end key");
            };
            assert_eq!(end[0], Value::Int(28));
            assert!(matches!(end[1], Value::Float(value) if (value - last).abs() < 1e-9));
        }

        let mut body = crate::binary::unpack(native).unwrap();
        let first_pan_key = [8, 40, 82, 36, 255, 255, 255, 224, 5, 0, 0, 0, 0, 0, 12, 0];
        let at = body.windows(first_pan_key.len()).position(|bytes| bytes == first_pan_key).unwrap();
        body[at + first_pan_key.len()] = 1;
        let changed = crate::binary::pack(&body).unwrap();
        assert!(picture_control(&changed, &mut Blobs::default(), 5, 24, 48)
            .unwrap()
            .is_none());
    }

    #[test]
    fn reopened_linear_crop_preserves_four_edges_and_requires_a_neutral_base() {
        // Native re-export of a retained 36-frame head trim. Resolve omitted
        // the scalar bases for the four crop edges that start at zero.
        let native = concat!(
            "000000020000011c8128b52ffd602e018d0800740c0ab802080438004a45082a1a0b0a09119a9999999999e93f",
            "5234ffffffd02400000000000c0035f93f47f53f4a45082b33f333f35710932b88c9edcdcccccccccce43f",
            "4a08082c1a040a0228004a450828823331c01e38c3bfa6df99721dcf95039a410029a0c93f4a450829b3b33f",
            "9d9841d97197420029a01904c03f4a000af001080638064a3808360000006afcc13d99c69fb93f4a380837",
            "352781b85073b2389c6061dcbc09a6024801cd2080b43f4a38083937c1c2b879a303ec813331c0be3f4a00",
            "4a002000a028a025501890831573e300d04c57511983011423054dd82e2005d9e73802c54071f099120c056",
            "501899c0400c951413428460985385728b80f6f4c090204879b798493984f",
        );
        let Some(PictureControl::LinearMotion(motion)) =
            picture_control(native, &mut Blobs::default(), 36, 36, 120).unwrap()
        else {
            panic!("expected native crop and motion");
        };
        let effect = motion.readback();
        for (name, middle, last) in [
            ("crop_left", 31.0, 100.0),
            ("crop_top", 43.0, 80.0),
            ("crop_right", 72.0, 150.0),
            ("crop_bottom", 38.0, 120.0),
        ] {
            let expected = Value::Arr(
                [(36, 0.0), (53, middle / 999.0), (71, last / 999.0)]
                    .into_iter()
                    .map(|(frame, value)| Value::Arr(vec![Value::Int(frame), Value::Float(value)]))
                    .collect(),
            );
            assert_eq!(effect.params.get(name), Some(&expected), "{name}");
        }
        let mut body = crate::binary::unpack(native).unwrap();
        let left = [8, 54, 82, 52, 255, 255, 255, 208, 36, 0, 0, 0, 0, 0, 12, 0];
        let at = body.windows(left.len()).position(|bytes| bytes == left).unwrap();
        body[at + left.len()] = 1; // Nonzero initial key without a saved base.
        let changed = crate::binary::pack(&body).unwrap();
        assert!(picture_control(&changed, &mut Blobs::default(), 36, 36, 120)
            .unwrap()
            .is_none());
    }

    #[test]
    fn native_three_key_motion_keeps_both_off_edit_keys_with_media_handles() {
        let native = "00000002000000af8128b52ffd603b0025050014070ab802080438004a45082a1a0b0a09119a9999999999e93f5234ffffffd00300000000000c00000000a09999e93f14f93f26f53f4a45082b33f3403333f3cecccccccccceccde43f4a08082c1a040a0228004a450828b9bf00abaaaaaaaa92803f4a450829a9a94e316af8db94a93f4a001500398e805e603bf84c09e60d3504280608e410804289356240e1b633c505c540414af0c31b538200671e4f6ad84f36cb";
        let Some(PictureControl::LinearMotion(motion)) =
            picture_control(native, &mut Blobs::default(), 6, 30, 48).unwrap()
        else {
            panic!("expected three native keys on a trimmed clip");
        };
        let effect = motion.readback();
        let Some(Value::Arr(points)) = effect.params.get("zoom_x") else {
            panic!("expected independent horizontal zoom");
        };
        let times: Vec<_> = points
            .iter()
            .map(|p| match p {
                Value::Arr(point) => point[0].clone(),
                _ => panic!("expected a key pair"),
            })
            .collect();
        assert_eq!(times, [Value::Int(3), Value::Int(20), Value::Int(38)]);
        assert!(picture_control(native, &mut Blobs::default(), 6, 30, 38)
            .unwrap()
            .is_none());
    }

    #[test]
    fn native_linear_motion_uses_source_frame_keys_and_refuses_other_curve_flags() {
        let native = "00000002000000958128b52ffd20fb5d040044060af801080438004a35082a1a0b0a09119a9999999999e93f5224ffffffe00300000000000c00000000a09999e93f26f53f4a35082b1a0b0a091133f3403333f3cdcccccccccce43f4a08082c1a040a0228004a350828b9bf803f4a350829a9a9a93f4a000f00398e4077e033adcc9b390410c8210081122bc480c0ad018db4f28b6b9914069f6c9601";
        let Some(PictureControl::LinearMotion(motion)) =
            picture_control(native, &mut Blobs::default(), 3, 36, 48).unwrap()
        else {
            panic!("expected the native Inspector curves");
        };
        let effect = motion.readback();
        assert_eq!(
            effect.params.get("pan"),
            Some(&Value::Arr(vec![
                Value::Arr(vec![Value::Int(3), Value::Float(-0.1)]),
                Value::Arr(vec![Value::Int(38), Value::Float(0.09999999999999964)]),
            ])),
        );
        assert!(picture_control(native, &mut Blobs::default(), 0, 36, 48)
            .unwrap()
            .is_none());
        let mut body = crate::binary::unpack(native).unwrap();
        let end_key = [38, 0, 0, 0, 0, 0, 12, 0];
        let at = body.windows(end_key.len()).position(|w| w == end_key).unwrap();
        body[at + 6] = 0;
        let changed = crate::binary::pack(&body).unwrap();
        assert!(picture_control(&changed, &mut Blobs::default(), 3, 36, 48)
            .unwrap()
            .is_none());
    }

    #[test]
    fn native_reflection_slots_preserve_axes_and_refuse_unqualified_values() {
        let controls = [
            ("0000000200000027800a24080438004a004a004a004a004a004a004a004a004a004a004a004a0808331a040a022801", false, true),
            ("0000000200000027800a24080438004a004a004a004a004a004a004a004a004a004a004a0808321a040a0228014a00", true, false),
            ("000000020000002f800a2c080438004a004a004a004a004a004a004a004a004a004a004a0808321a040a0228014a0808331a040a022801", true, true),
        ];
        for (encoded, horizontal, vertical) in controls {
            assert_eq!(
                picture_control(encoded, &mut Blobs::default(), 0, 0, 0).unwrap(),
                Some(PictureControl::Reflection(Reflection { horizontal, vertical })),
            );
            let invalid = encoded.replacen("0a022801", "0a022802", 1);
            assert!(picture_control(&invalid, &mut Blobs::default(), 0, 0, 0).unwrap().is_none());
        }
    }

    #[test]
    fn pure_static_pan_does_not_admit_extra_or_animated_parameters() {
        // A single, static fractional X value using the witnessed field shape.
        let bytes = "000000020000002e800a2b080438004a004a004a004a0f08281a0b0a09119a9999999999a93f4a004a004a004a004a004a004a004a00";
        let Some(PictureControl::Transform(transform)) =
            picture_control(bytes, &mut Blobs::default(), 0, 0, 0).unwrap()
        else {
            panic!("expected static transform");
        };
        assert_eq!(transform.position, (0.05, 0.0));
        let changed = bytes.replacen("08281a", "08291a", 1);
        assert!(picture_control(&changed, &mut Blobs::default(), 0, 0, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn static_video_preserves_axes_independent_scale_and_crop_order() {
        let transform = Transform {
            position: (0.025, -0.05),
            scale: (112.5, 87.5),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            crop: [0.0; 4],
        };
        let crop = Transform {
            position: (0.0, 0.0),
            scale: (100.0, 100.0),
            crop: [0.05, 0.04, 0.025, 0.01],
            ..transform.clone()
        };
        for expected in [&transform, &crop] {
            let encoded = crate::video::encode(expected).unwrap().unwrap();
            assert_eq!(
                picture_control(&encoded, &mut Blobs::default(), 0, 0, 0).unwrap(),
                Some(PictureControl::Transform(expected.clone())),
            );
        }
        let ambiguous = Transform {
            crop: crop.crop,
            ..transform
        };
        assert!(crate::video::encode(&ambiguous).is_err());
    }
    #[test]
    fn native_static_crop_can_stand_alone_or_accompany_motion() {
        let crop_only = "000000020000004c8128b52ffd204e15020054030a4c080638064a0f08361a0b0a091166a63f4a0f083755953f4a0f083874e501c93a579e3f4a0f0839427b09ed25b4a73f4a004a0005040220d12cdca5598025";
        let combined = "000000020000008b8128b52ffd20b00d040014060a60080438004a0f082a1a0b0a091100f23f4a0f082bec3f4a08082c1a040a0228004a0f082833a33f4a0f0829a13f4a000a4c080638064a0f083666a63f4a0f083755953f4a0f083874e501c93a579e3f4a0f0839427b09ed25b4a73f4a004a000d004012cd2b5cbb34a57077916a518e131840f30ad72e9f62d1b4e2024b";
        let edges = [84.0 / 1920.0, 32.0 / 1080.0, 40.0 / 1920.0, 50.0 / 1080.0];
        for (saved, position, scale) in [
            (crop_only, (0.0, 0.0), (100.0, 100.0)),
            (combined, (72.0 / 1920.0, -36.0 / 1080.0), (112.5, 87.5)),
        ] {
            let expected = Transform { position, scale, rotation: 0.0, anchor: (0.0, 0.0), crop: edges };
            assert_eq!(picture_control(saved, &mut Blobs::default(), 5, 24, 48).unwrap(),
                Some(PictureControl::Transform(expected)));
        }
    }

    #[test]
    fn static_clip_volume_preserves_signed_db_and_refuses_other_controls() {
        let minus =
            "0000000200000020800a1d087c38004a0f085f1a0b0a091100000000000018c04a004a004a004a00";
        let plus = minus.replacen("18c0", "0840", 1);
        for (native, db) in [(minus, -6.0), (plus.as_str(), 3.0)] {
            let encoded = Envelope::static_volume(db).encode().unwrap();
            assert_eq!(
                crate::binary::unpack(native).unwrap(),
                crate::binary::unpack(&encoded).unwrap(),
            );
        }
        let read = |encoded: &str| {
            let edit = Edit {
                start: 20,
                source_in: 2,
                length: 28,
            };
            match clip_audio(encoded, &mut Blobs::default(), edit).unwrap().and_then(|audio| audio.envelope) {
                Some(Envelope { volume: Some(Volume::Static(db)), fades: None }) => Some(db),
                _ => None,
            }
        };
        assert_eq!(read(minus), Some(-6.0));
        assert_eq!(read(&plus), Some(3.0));
        assert_eq!(read(&minus.replacen("18c0", "28c0", 1)), None);
        assert_eq!(read(&minus.replacen("085f1a", "08611a", 1)), None);

        // A fade in a neighboring slot or automation attached to the scalar
        // requires a separate interpretation, even if its value is neutral.
        let body = crate::binary::unpack(minus).unwrap();
        let extra_slot = crate::binary::replace_bytes(&body, 1, |section| {
            let mut section = section[..section.len() - 2].to_vec();
            crate::binary::pb_bytes(9, &[8, 97], &mut section);
            Ok(section)
        })
        .unwrap();
        assert_eq!(read(&crate::binary::pack(&extra_slot).unwrap()), None);
        let mut with_keys = hex::decode(minus).unwrap();
        with_keys[7] += 2;
        with_keys[10] += 2;
        with_keys[16] += 2;
        let at = with_keys.len() - 8;
        with_keys.splice(at..at, [82, 0]);
        assert_eq!(read(&hex::encode(with_keys)), None);
    }
    #[test]
    fn static_clip_pan_keeps_its_sign_and_coexists_with_an_envelope() {
        let edit = Edit { start: 0, source_in: 3, length: 48 };
        let pan = "0000000200000019800a1608900138024a0f08601a0b0a091100000000000049c0";
        let read = |encoded: &str| clip_audio(encoded, &mut Blobs::default(), edit).unwrap();
        assert_eq!(read(pan).unwrap().pan, Some(-50.0));
        assert_eq!(read(&pan.replace("49c0", "4940")).unwrap().pan, Some(50.0));
        assert!(read(&pan.replace("08601a", "08611a")).is_none());
        let mut body = crate::binary::unpack(&Envelope {
            volume: Some(Volume::Static(-6.0)),
            fades: Some(Fades::linear(6, 0)),
        }.encode().unwrap()).unwrap();
        body.extend(crate::binary::unpack(pan).unwrap());
        let combined = read(&crate::binary::pack(&body).unwrap()).unwrap();
        assert_eq!(combined.pan, Some(-50.0));
        let envelope = combined.envelope.unwrap();
        assert_eq!(envelope.volume, Some(Volume::Static(-6.0)));
        assert_eq!(envelope.fades, Some(Fades::linear(6, 0)));
    }

    #[test]
    fn fractional_fade_replays_exact_ticks_from_the_saved_envelope() {
        let frame_ticks = 10_594_584_000;
        let expected = [64_714_012_088, 86_625_055_551];
        let envelope = Envelope {
            volume: Some(Volume::Static(-6.0)),
            fades: Some(Fades {
                head: expected[0] as f64 / frame_ticks as f64,
                tail: expected[1] as f64 / frame_ticks as f64,
                head_point: None,
                tail_point: None,
            }),
        };
        let encoded = envelope.encode().unwrap();
        let edit = Edit { start: 10, source_in: 3, length: 48 };
        let saved = clip_audio(&encoded, &mut Blobs::default(), edit).unwrap()
            .unwrap().envelope.unwrap();
        assert_eq!(saved, envelope);
        assert_eq!(saved.fades.unwrap().ticks(edit.length, frame_ticks), Some(expected));
        assert_eq!(saved.recovered_loss(), Some(crate::audio::FRACTIONAL_FADE_LOSS));
    }

    #[test]
    fn direct_audio_fades_are_clip_edge_lengths_and_keys_keep_source_frames() {
        let fades = "000000020000002f800a2c087c38004a004a0f08611a0b0a091100000000000014404a0f08621a0b0a091100000000000022404a004a00";
        let keys = "00000002000000428128b52ffd2045c50100c4020a43087c38004a35085f1a0b0a09110018c05224ffffffe008000c0018c013000c0008404a004a004a004a000510008303191cc8b504";
        use crate::audio::Envelope;
        for (witness, controls) in [
            (fades, Envelope { volume: None, fades: Some(Fades::linear(5, 9)) }),
            (keys, Envelope { volume: Some(Volume::Keys([(8, -6.0), (19, 3.0)])), fades: None }),
        ] {
            assert_eq!(
                crate::binary::unpack(witness).unwrap(),
                crate::binary::unpack(&controls.encode().unwrap()).unwrap()
            );
        }
        let read = |encoded: &str, source_in, length| {
            clip_audio(
                encoded,
                &mut Blobs::default(),
                Edit {
                    start: 20,
                    source_in,
                    length,
                },
            )
            .unwrap()
            .and_then(|audio| audio.envelope)
        };
        for source_in in [2, 6] {
            assert!(matches!(
                read(fades, source_in, 28),
                Some(Envelope { volume: None, fades: Some(Fades { head: 5.0, tail: 9.0, .. }) })
            ));
            let Some(Envelope { volume: Some(Volume::Keys(points)), fades: None }) = read(keys, source_in, 28) else {
                panic!("expected exact source-coordinate volume keys");
            };
            assert_eq!(points, [(8, -6.0), (19, 3.0)]);
        }
        assert!(read(fades, 2, 13).is_none());
        assert!(read(keys, 9, 25).is_none());
        let body = crate::binary::unpack(keys).unwrap();
        let bad = crate::binary::replace_bytes(&body, 1, |section| {
            let mut section = section.to_vec();
            let count = section
                .windows(4)
                .position(|bytes| bytes == (-32i32).to_be_bytes())
                .unwrap();
            section[count + 8] ^= 1;
            Ok(section)
        })
        .unwrap();
        assert!(read(&crate::binary::pack(&bad).unwrap(), 2, 28).is_none());
    }

    #[test]
    fn custom_fade_points_use_native_curve_nesting_and_require_a_fade() {
        let body = hex::decode(concat!(
            "0a5c087c38004a004a0f08611a0b0a091100000000000020404a0f08621a0b0a09110000000000002040",
            "4a1808631a140a123a10404f690690690690c039000000000000",
            "4a1808641a140a123a10404296f96f96f970c042800000000000",
        )).unwrap();
        let read = |body: &[u8], length| {
            clip_audio(
                &crate::binary::pack(body).unwrap(),
                &mut Blobs::default(),
                Edit { start: 0, source_in: 4, length },
            ).unwrap().and_then(|audio| audio.envelope)
        };
        let Some(envelope @ Envelope { volume: None, fades: Some(fades) }) = read(&body, 24) else {
            panic!("expected the native fade points");
        };
        assert_eq!(fades.head_point, FadePoint::new(62.82051282051282, -25.0));
        assert_eq!(fades.tail_point, FadePoint::new(37.17948717948718, -37.0));
        assert_eq!(
            crate::binary::unpack(&envelope.encode().unwrap()).unwrap(),
            body,
        );
        assert!(read(&body, 15).is_none());
        let mut outside = body.clone();
        let pos = outside.windows(8).position(|bytes| bytes == 62.82051282051282f64.to_be_bytes()).unwrap();
        outside[pos..pos + 8].copy_from_slice(&100.0f64.to_be_bytes());
        assert!(read(&outside, 24).is_none());
    }
}
