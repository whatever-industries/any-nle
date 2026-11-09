use super::*;
use prproj::convert::prepared::{MotionKey, PictureEffect};

fn pair(value: &Value) -> Option<[f64; 2]> {
    let Value::Arr(values) = value else {
        return None;
    };
    let [a, b] = values.as_slice() else {
        return None;
    };
    let number = |n: &Value| match n {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    };
    Some([number(a)?, number(b)?])
}

pub(super) fn prepare(
    placement: &graph::Placement,
    effect: &graph::Effect,
    full_span: bool,
    same_rate: bool,
    frame_ticks: i64,
    losses: &mut Vec<Loss>,
) -> R<Option<PictureEffect>> {
    let malformed = || {
        error(format!(
            "invalid {} effect for {}",
            effect.kind, placement.id
        ))
    };
    if placement.video_track.is_none() {
        return Err(malformed());
    }
    if effect.kind == "avid.flip" {
        let horizontal = key(&effect.params, "horizontal")
            .and_then(boolean)
            .ok_or_else(malformed)?;
        let vertical = key(&effect.params, "vertical")
            .and_then(boolean)
            .ok_or_else(malformed)?;
        if !horizontal && !vertical {
            return Err(malformed());
        }
        return Ok(Some(PictureEffect::Flip {
            horizontal,
            vertical,
        }));
    }
    if !full_span || !same_rate || placement.speed.is_some() || placement.frame_map.is_some() {
        loss(
            losses,
            &placement.id,
            "picture_effect",
            "Linear Resize animation requires an untrimmed prepared edit at the source media rate",
        );
        return Ok(None);
    }
    let Some(Value::Arr(keys)) = key(&effect.params, "keys") else {
        return Err(malformed());
    };
    let [first, last] = keys.as_slice() else {
        return Err(malformed());
    };
    let source_in = ticks(placement.source_in, "Resize media in")?;
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
            position: key(v, "position").and_then(pair).ok_or_else(malformed)?,
            scale: key(v, "scale").and_then(pair).ok_or_else(malformed)?,
        })
    };
    Ok(Some(PictureEffect::Resize([read(first)?, read(last)?])))
}
