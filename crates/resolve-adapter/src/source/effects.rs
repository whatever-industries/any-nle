//! Resolve clip controls corroborated by native renders.
use super::*;
use crate::audio::{EFFECT, HEAD, KEY_FLAGS, TAIL, VOLUME, VOLUME_DB};
use crate::video::{
    motion::{Curve, LinearMotion, CONTROLS, KEY_BYTES, KEY_FLAGS as VIDEO_KEY_FLAGS},
    PictureControl, Reflection, CROP, FLIP_X, FLIP_Y, SCALE_X, SCALE_Y, X, Y,
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
        [(1, Wire::Bytes(motion))] => (*motion, None),
        [(1, Wire::Bytes(motion)), (1, Wire::Bytes(crop))] => (*motion, Some(*crop)),
        _ => return Ok(None),
    };
    let Some(motion) = section_slots(motion, 4, 0)? else {
        return Ok(None);
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
    if motion[5..first_reflection_slot].iter().any(|slot| !slot.is_empty()) {
        return Ok(None);
    }
    if !matches!(motion[2], [] | [8, 44, 26, 4, 10, 2, 40, 0]) {
        return Ok(None);
    }
    if !motion[2].is_empty() {
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
                let Some(slots) = source_crop_slots(crop)? else {
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
    let (Some(x), Some(y), Some(sx), Some(sy)) = (
        static_slot(&motion, X, 0.0)?,
        static_slot(&motion, Y, 0.0)?,
        static_slot(&motion, SCALE_X, 1.0)?,
        static_slot(&motion, SCALE_Y, 1.0)?,
    ) else {
        return Ok(None);
    };
    if (sx != 1.0 || sy != 1.0) && motion[2].is_empty() {
        return Ok(None);
    }
    let mut edges = [0.0; 4];
    if let Some(crop) = crop {
        let Some(slots) = source_crop_slots(crop)? else {
            return Ok(None);
        };
        for (edge, control) in edges.iter_mut().zip(CROP) {
            let Some(value) = static_slot(&slots, control, 0.0)? else {
                return Ok(None);
            };
            *edge = value;
        }
    }
    let t = Transform {
        position: (x, -y),
        scale: (sx * 100.0, sy * 100.0),
        rotation: 0.0,
        anchor: (0.0, 0.0),
        crop: edges,
    };
    Ok(crate::video::encode_with_reflection(&t, reflection).ok().map(|_| {
        if reflection.active() { PictureControl::ReflectedTransform(t, reflection) }
        else { PictureControl::Transform(t) }
    }))
}

fn source_crop_slots(crop: &[u8]) -> Result<Option<Vec<&[u8]>>> {
    // Softness and Retain Image Position must be absent for source-space crop.
    Ok(section_slots(crop, 6, 6)?
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

pub(super) enum ClipAudio {
    StaticVolume(f64),
    VolumeKeys([(i64, f64); 2]),
    Fades { head: i64, tail: i64 },
}

pub(super) fn clip_audio(
    encoded: &str,
    blobs: &mut Blobs,
    edit: Edit,
) -> Result<Option<ClipAudio>> {
    let bytes = blobs.envelope(encoded)?;
    let Some(slots) = control_slots(&bytes, EFFECT)? else {
        return Ok(None);
    };
    let [volume, head, tail, [], []] = slots.as_slice() else {
        return Ok(None);
    };
    if head.is_empty() && tail.is_empty() {
        if let Some(db) = static_parameter(volume, VOLUME)?.filter(|db| VOLUME_DB.contains(db)) {
            return Ok(Some(ClipAudio::StaticVolume(db)));
        }
        return Ok(volume_keys(volume, edit)?.map(ClipAudio::VolumeKeys));
    }
    // Combined curves and other active Fairlight controls need separate evidence.
    if !volume.is_empty() {
        return Ok(None);
    }
    let (Some(head), Some(tail)) = (
        fade_length(head, HEAD, edit.length)?,
        fade_length(tail, TAIL, edit.length)?,
    ) else {
        return Ok(None);
    };
    Ok((head + tail <= edit.length).then_some(ClipAudio::Fades { head, tail }))
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
        ("in", HEAD, 99, *head, *in_shape),
        ("out", TAIL, 100, *tail, *out_shape),
    ] {
        let Some(length) = fade_length(duration, duration_id, edit.length)? else { continue };
        if !duration.is_empty() {
            data.push((format!("{edge}_frames"), Value::Int(length)));
        }
        if shape.is_empty() {
            continue;
        }
        let params = fields(shape)?;
        let [(1, Wire::Int(id)), (7, Wire::Bytes(point))] = params.as_slice() else { continue };
        if *id != point_id || point.len() != 16 || length == 0 {
            continue;
        }
        let position = f64::from_be_bytes(point[..8].try_into()?);
        let level = f64::from_be_bytes(point[8..].try_into()?);
        if (0.0..=100.0).contains(&position) && level.is_finite() {
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

fn fade_length(data: &[u8], id: u64, limit: i64) -> Result<Option<i64>> {
    if data.is_empty() {
        return Ok(Some(0));
    }
    Ok(static_parameter(data, id)?
        .filter(|n| *n > 0.0 && *n <= limit as f64 && n.fract() == 0.0)
        .map(|n| n as i64))
}

fn volume_keys(data: &[u8], edit: Edit) -> Result<Option<[(i64, f64); 2]>> {
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
        let frame = i64::from(u32::from_le_bytes(bytes[..4].try_into()?));
        let flags = u32::from_le_bytes(bytes[4..8].try_into()?);
        let db = f64::from_le_bytes(bytes[8..].try_into()?);
        if flags != KEY_FLAGS
            || !VOLUME_DB.contains(&db)
            || !(edit.source_in..edit.source_in + edit.length).contains(&frame)
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
    fn static_clip_volume_preserves_signed_db_and_refuses_other_controls() {
        let minus =
            "0000000200000020800a1d087c38004a0f085f1a0b0a091100000000000018c04a004a004a004a00";
        let plus = minus.replacen("18c0", "0840", 1);
        for (native, db) in [(minus, -6.0), (plus.as_str(), 3.0)] {
            let encoded = crate::audio::Envelope::StaticVolume(db).encode().unwrap();
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
            match clip_audio(encoded, &mut Blobs::default(), edit).unwrap() {
                Some(ClipAudio::StaticVolume(db)) => Some(db),
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
    fn direct_audio_fades_are_clip_edge_lengths_and_keys_keep_source_frames() {
        let fades = "000000020000002f800a2c087c38004a004a0f08611a0b0a091100000000000014404a0f08621a0b0a091100000000000022404a004a00";
        let keys = "00000002000000428128b52ffd2045c50100c4020a43087c38004a35085f1a0b0a09110018c05224ffffffe008000c0018c013000c0008404a004a004a004a000510008303191cc8b504";
        use crate::audio::Envelope;
        for (witness, controls) in [
            (fades, Envelope::Fades { head: 5, tail: 9 }),
            (keys, Envelope::VolumeKeys([(8, -6.0), (19, 3.0)])),
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
        };
        for source_in in [2, 6] {
            assert!(matches!(
                read(fades, source_in, 28),
                Some(ClipAudio::Fades { head: 5, tail: 9 })
            ));
            let Some(ClipAudio::VolumeKeys(points)) = read(keys, source_in, 28) else {
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
}
