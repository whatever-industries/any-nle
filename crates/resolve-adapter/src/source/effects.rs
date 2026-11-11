//! Resolve clip controls corroborated by native renders.
use super::*;
use crate::audio::{EFFECT, HEAD, KEY_FLAGS, TAIL, VOLUME, VOLUME_DB};
use encoding::{fields, Wire};
use graph::Transform;

pub(super) fn static_pan(encoded: &str, blobs: &mut Blobs) -> Result<Option<Transform>> {
    let value = static_control(encoded, blobs, (4, 40), 12, 3)?;
    Ok(value.filter(|x| x.abs() <= 0.05).map(|x| Transform {
        position: (x, 0.0),
        scale: (100.0, 100.0),
        rotation: 0.0,
        anchor: (0.0, 0.0),
        crop: [0.0; 4],
    }))
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

fn static_control(
    encoded: &str,
    blobs: &mut Blobs,
    (effect_id, parameter_id): (u64, u64),
    slot_count: usize,
    value_slot: usize,
) -> Result<Option<f64>> {
    let bytes = blobs.envelope(encoded)?;
    let Some(slots) = control_slots(&bytes, effect_id)? else {
        return Ok(None);
    };
    if slots.len() != slot_count
        || slots
            .iter()
            .enumerate()
            .any(|(slot, bytes)| slot != value_slot && !bytes.is_empty())
    {
        return Ok(None);
    }
    static_parameter(slots[value_slot], parameter_id)
}

fn control_slots(bytes: &[u8], effect_id: u64) -> Result<Option<Vec<&[u8]>>> {
    let outer = fields(bytes)?;
    let [(1, Wire::Bytes(section))] = outer.as_slice() else {
        return Ok(None);
    };
    let section = fields(section)?;
    let [(1, Wire::Int(section_id)), (7, Wire::Int(0)), slots @ ..] = section.as_slice() else {
        return Ok(None);
    };
    if *section_id != effect_id {
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
    fn pure_static_pan_does_not_admit_extra_or_animated_parameters() {
        // A single, static fractional X value using the witnessed field shape.
        let bytes = "000000020000002e800a2b080438004a004a004a004a0f08281a0b0a09119a9999999999a93f4a004a004a004a004a004a004a004a00";
        let position = static_pan(bytes, &mut Blobs::default())
            .unwrap()
            .unwrap()
            .position;
        assert_eq!(position, (0.05, 0.0));
        let changed = bytes.replacen("08281a", "08291a", 1);
        assert!(static_pan(&changed, &mut Blobs::default())
            .unwrap()
            .is_none());
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
