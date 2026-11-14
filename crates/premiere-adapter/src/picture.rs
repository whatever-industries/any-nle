use super::*;
use graph::picture_chain::{Chain, Reflection};
use prproj::convert::prepared::{CornerPin, MotionKey, PictureEffect, TrimmedMotion, TrimmedMotionKey};

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

fn numbers<const N: usize>(value: &Value) -> Option<[f64; N]> {
    let Value::Arr(values) = value else {
        return None;
    };
    let mut result = [0.0; N];
    if values.len() != result.len() {
        return None;
    }
    for (output, input) in result.iter_mut().zip(values) {
        *output = number(input)?;
    }
    Some(result)
}

pub(super) fn corner_pin(params: &Value) -> Option<CornerPin> {
    let Value::Obj(fields) = params else { return None };
    let Value::Arr(points) = params.get("target_corners_tl_tr_br_bl")? else { return None };
    let [tl, tr, br, bl] = points.as_slice() else { return None };
    let source_crop = params.get("source_crop_ltrb");
    let pin = CornerPin {
        corners: [numbers(tl)?, numbers(tr)?, numbers(br)?, numbers(bl)?],
        source_crop: match source_crop { Some(value) => numbers(value)?, None => [0.0; 4] },
    };
    (fields.len() == 1 + usize::from(source_crop.is_some()) && pin.is_valid()).then_some(pin)
}

pub(super) fn prepare_chain(
    placement: &graph::Placement,
    effect: &graph::Effect,
    full_span: bool,
    same_rate: bool,
    frame_ticks: i64,
    video_duration: Option<Rational>,
    matched_hd_raster: bool,
    losses: &mut Vec<Loss>,
) -> R<(Option<graph::Transform>, Option<PictureEffect>, [bool; 2])> {
    let refused = (None, None, [false; 2]);
    if let Some(Value::Arr(stages)) = effect.params.get("stages") {
        if stages.iter().any(is_pin_stage) {
            let Some(native) = prepare_pin_chain(stages, &effect.params)
                .map_err(|()| error(format!("malformed ordered Corner Pin at {}", placement.id)))?
            else {
                loss(losses, &placement.id, "picture_effect",
                    "Ordered Corner Pin requires one static pin and one horizontal reflection");
                return Ok(refused);
            };
            if !full_span || !same_rate || !matched_hd_raster
                || frame_ticks != model::ticks_per_unit(Fraction::new(24, 1))
                || placement.video_track.is_none() || placement.speed.is_some() || placement.frame_map.is_some()
            {
                loss(losses, &placement.id, "picture_effect",
                    "Ordered Corner Pin requires a same-rate 1080p24 picture edit over the saved span");
                return Ok(refused);
            }
            return Ok((None, Some(native), [false; 2]));
        }
    }
    let chain = match Chain::read(effect) {
        Ok(chain) => chain,
        Err(graph::picture_chain::Error::Unsupported(reason)) => {
            loss(losses, &placement.id, "picture_effect", reason);
            return Ok(refused);
        }
        Err(err) => return Err(error(format!("{err} at {}", placement.id))),
    };
    if !full_span || !same_rate || !matched_hd_raster
        || placement.video_track.is_none() || placement.speed.is_some() || placement.frame_map.is_some()
    {
        loss(losses, &placement.id, "picture_effect", "Ordered picture chains require a same-rate 1920x1080 edit over the saved span");
        return Ok(refused);
    }
    let flip = chain.before.then(chain.after);
    let Some(motion) = chain.motion else {
        return Ok((None, flip.active().then_some(PictureEffect::Flip {
            horizontal: flip.horizontal, vertical: flip.vertical,
        }), [false; 2]));
    };
    // Standard Flip runs before intrinsic Motion. Reflect Motion's output
    // position and crop when Avid's chain has a reflection after Motion.
    let is_static = motion.is_static();
    let mut transform = motion.transform;
    transform.position = chain.after.point(transform.position.into()).into();
    transform.crop = chain.after.crop(transform.crop);
    let native = if is_static {
        PictureEffect::StaticMotion
    } else {
        let Some(effect) = prepare(placement, motion.kind, motion.params, full_span, same_rate,
            frame_ticks, video_duration, losses)? else { return Ok(refused) };
        reflect_motion(effect, chain.after)
    };
    Ok((Some(transform), Some(native), [flip.horizontal, flip.vertical]))
}

fn is_pin_stage(stage: &Value) -> bool {
    matches!(stage.get("kind"), Some(Value::Str(kind)) if kind == "picture.corner_pin")
}

fn prepare_pin_chain(stages: &[Value], params: &Value) -> Result<Option<PictureEffect>, ()> {
    if !matches!(params, Value::Obj(fields) if fields.len() == 1)
        || !(2..=16).contains(&stages.len())
    {
        return Err(());
    }
    let [first, second] = stages else { return Ok(None); };
    let (pin, reflection, flop_before_pin) = if is_pin_stage(first) {
        (first, second, false)
    } else {
        (second, first, true)
    };
    if !matches!(pin, Value::Obj(fields) if fields.len() == 2) {
        return Err(());
    }
    let pin = corner_pin(pin.get("params").ok_or(())?).ok_or(())?;
    let Some(Value::Str(kind)) = reflection.get("kind") else { return Err(()); };
    if !Reflection::is_effect_kind(kind) {
        return Ok(None);
    }
    if !matches!(reflection, Value::Obj(fields) if fields.len() == 2) {
        return Err(());
    }
    let axes = Reflection::from_params(reflection.get("params").ok_or(())?).ok_or(())?;
    Ok((axes.horizontal && !axes.vertical).then_some(PictureEffect::CornerPinWithFlop {
        pin, flop_before_pin,
    }))
}

#[cfg(test)]
mod pin_chain_tests {
    use super::*;

    #[test]
    fn static_pin_chain_keeps_horizontal_order_and_rejects_other_stacks() {
        let pin = Value::Obj(vec![
            ("kind".into(), Value::Str("picture.corner_pin".into())),
            ("params".into(), Value::Obj(vec![("target_corners_tl_tr_br_bl".into(),
                Value::Arr([[0.1, 0.2], [0.8, 0.1], [0.9, 0.8], [0.2, 0.9]]
                    .map(|p| Value::Arr(p.map(Value::Float).into())).into()))])),
        ]);
        let reflect = |horizontal, vertical| {
            let e = Reflection { horizontal, vertical }.effect();
            Value::Obj(vec![("kind".into(), Value::Str(e.kind)), ("params".into(), e.params)])
        };
        let prepare = |stages: Vec<Value>| {
            let params = Value::Obj(vec![("stages".into(), Value::Arr(stages.clone()))]);
            prepare_pin_chain(&stages, &params)
        };
        for (stages, before) in [
            (vec![reflect(true, false), pin.clone()], true),
            (vec![pin.clone(), reflect(true, false)], false),
        ] {
            assert!(matches!(prepare(stages), Ok(Some(PictureEffect::CornerPinWithFlop {
                flop_before_pin, ..
            })) if flop_before_pin == before));
        }
        assert_eq!(prepare(vec![reflect(false, true), pin.clone()]), Ok(None));
        assert_eq!(prepare(vec![pin.clone(), pin.clone()]), Ok(None));
        assert_eq!(prepare(vec![reflect(true, false), pin.clone(), reflect(true, false)]), Ok(None));
        let malformed = Value::Obj(vec![
            ("kind".into(), Value::Str("picture.corner_pin".into())),
            ("params".into(), Value::Obj(vec![])),
        ]);
        assert!(prepare(vec![malformed, reflect(true, false)]).is_err());
    }
}

fn reflect_motion(effect: PictureEffect, reflection: Reflection) -> PictureEffect {
    let trimmed = |mut trim: TrimmedMotion| {
        for key in &mut trim.motion {
            key.position = reflection.point(key.position);
        }
        if let Some(crop) = &mut trim.crop {
            for (_, edges) in crop {
                *edges = reflection.crop(*edges);
            }
        }
        trim
    };
    match effect {
        PictureEffect::Resize(mut keys) => {
            for key in &mut keys {
                key.position = reflection.point(key.position);
                key.crop = key.crop.map(|edges| reflection.crop(edges));
            }
            PictureEffect::Resize(keys)
        }
        PictureEffect::TrimmedWarp(trim) => PictureEffect::TrimmedWarp(trimmed(trim)),
        PictureEffect::TrimmedResize(trim) => PictureEffect::TrimmedResize(trimmed(trim)),
        other => other,
    }
}

pub(super) fn prepare(
    placement: &graph::Placement,
    kind: &str,
    params: &Value,
    full_span: bool,
    same_rate: bool,
    frame_ticks: i64,
    video_duration: Option<Rational>,
    losses: &mut Vec<Loss>,
) -> R<Option<PictureEffect>> {
    let malformed = || {
        error(format!(
            "invalid {} effect for {}",
            kind, placement.id
        ))
    };
    if placement.video_track.is_none() {
        return Err(malformed());
    }
    if !full_span || !same_rate || placement.speed.is_some() || placement.frame_map.is_some() {
        loss(
            losses,
            &placement.id,
            "picture_effect",
            "Linear picture animation requires the saved edit span at the source media rate",
        );
        return Ok(None);
    }
    use prproj::convert::prepared::WarpRollInterpolation;
    let roll_interpolation = match kind {
        "avid.warp_roll_linear" => Some(WarpRollInterpolation::Linear),
        "avid.warp_roll_shelf" => Some(WarpRollInterpolation::Hold),
        "avid.warp_roll_bezier" | "avid.warp_roll_spline" => Some(WarpRollInterpolation::FlatBezier),
        _ => None,
    };
    if let Some(interpolation) = roll_interpolation {
        let Some(Value::Arr(keys)) = key(params, "keys") else { return Err(malformed()); };
        let [a, b, c] = keys.as_slice() else { return Err(malformed()); };
        if !matches!(params, Value::Obj(fields) if fields.len() == 1) || placement.transform.is_some() {
            return Err(malformed());
        }
        let source = ticks(placement.source_in, "Warp Rotation media in")?;
        let read = |key_values: &Value| -> R<prproj::convert::prepared::WarpRollKey> {
            if !matches!(key_values, Value::Obj(fields) if fields.len() == 4) {
                return Err(malformed());
            }
            let get = |name| key(key_values, name).and_then(number).ok_or_else(malformed);
            let frame = key(key_values, "frame").and_then(integer).ok_or_else(malformed)?;
            let ticks = frame.checked_mul(frame_ticks).and_then(|t| source.checked_add(t)).ok_or_else(malformed)?;
            Ok(prproj::convert::prepared::WarpRollKey {
                ticks, x: get("x")?, width: get("width")?, rotation: get("rotation")?,
            })
        };
        let keys = [read(a)?, read(b)?, read(c)?];
        if video_duration.and_then(|t| ticks(t, "Warp Rotation media extent").ok())
            .is_none_or(|end| keys.iter().any(|k| !(0..end).contains(&k.ticks)))
        {
            loss(losses, &placement.id, "picture_effect", "Animated Warp Rotation requires verified source-media coverage");
            return Ok(None);
        }
        return Ok(Some(PictureEffect::WarpRoll { keys, interpolation }));
    }
    let trimmed_crop = matches!(kind, "avid.warp_trim_crop_linear" | "avid.resize_trim_crop_linear");
    if trimmed_crop || matches!(kind, "avid.warp_trim_linear" | "avid.resize_trim_linear") {
        let Some(Value::Arr(motion)) = key(params, "motion") else { return Err(malformed()); };
        let Some(Value::Arr(width)) = key(params, "width") else { return Err(malformed()); };
        let ([a, b, c], [d, e, f]) = (motion.as_slice(), width.as_slice()) else { return Err(malformed()); };
        let fixed_width = key(params, "fixed_width").and_then(boolean).ok_or_else(malformed)?;
        let source_in = ticks(placement.source_in, "Motion media in")?;
        let key_ticks = |frame: i64| frame.checked_mul(frame_ticks)
            .and_then(|offset| source_in.checked_add(offset)).ok_or_else(malformed);
        let read_motion = |v: &Value| -> R<TrimmedMotionKey> {
            Ok(TrimmedMotionKey {
                ticks: key_ticks(key(v, "frame").and_then(integer).ok_or_else(malformed)?)?,
                position: key(v, "position").and_then(numbers).ok_or_else(malformed)?,
                height: key(v, "height").and_then(number).ok_or_else(malformed)?,
            })
        };
        let read_width = |v: &Value| -> R<(i64, f64)> {
            let Value::Arr(values) = v else { return Err(malformed()); };
            let [frame, value] = values.as_slice() else { return Err(malformed()); };
            Ok((key_ticks(integer(frame).ok_or_else(malformed)?)?, number(value).ok_or_else(malformed)?))
        };
        let crop = if trimmed_crop {
            let Some(Value::Arr(keys)) = key(params, "crop") else { return Err(malformed()); };
            let [first, middle, last] = keys.as_slice() else { return Err(malformed()); };
            let read_crop = |v: &Value| -> R<(i64, [f64; 4])> {
                Ok((
                    key_ticks(key(v, "frame").and_then(integer).ok_or_else(malformed)?)?,
                    key(v, "edges").and_then(numbers).ok_or_else(malformed)?,
                ))
            };
            Some([read_crop(first)?, read_crop(middle)?, read_crop(last)?])
        } else {
            None
        };
        let trim = TrimmedMotion {
            motion: [read_motion(a)?, read_motion(b)?, read_motion(c)?],
            width: [read_width(d)?, read_width(e)?, read_width(f)?],
            fixed_width,
            crop,
        };
        if video_duration.and_then(|duration| ticks(duration, "Motion source duration").ok())
            .is_none_or(|end| !trim.keys_fit_media(end))
        {
            loss(losses, &placement.id, "picture_effect", "Trimmed motion keys need qualified source-media handles");
            return Ok(None);
        }
        return Ok(Some(if matches!(kind, "avid.resize_trim_linear" | "avid.resize_trim_crop_linear") {
            PictureEffect::TrimmedResize(trim)
        } else {
            PictureEffect::TrimmedWarp(trim)
        }));
    }
    let Some(Value::Arr(keys)) = key(params, "keys") else {
        return Err(malformed());
    };
    let [first, last] = keys.as_slice() else {
        return Err(malformed());
    };
    let source_in = ticks(placement.source_in, "Motion media in")?;
    let keyed_crop = matches!(kind, "avid.resize_crop_linear" | "avid.warp_crop_linear");
    let read = |v: &Value| -> R<MotionKey> {
        let frame = key(v, "frame")
            .and_then(integer)
            .filter(|&n| n >= 0)
            .ok_or_else(malformed)?;
        let at = frame
            .checked_mul(frame_ticks)
            .and_then(|n| n.checked_add(source_in))
            .ok_or_else(malformed)?;
        Ok(MotionKey {
            ticks: at,
            position: key(v, "position").and_then(numbers).ok_or_else(malformed)?,
            scale: key(v, "scale").and_then(numbers).ok_or_else(malformed)?,
            crop: if keyed_crop {
                Some(key(v, "crop").and_then(numbers).ok_or_else(malformed)?)
            } else {
                None
            },
        })
    };
    Ok(Some(PictureEffect::Resize([read(first)?, read(last)?])))
}
