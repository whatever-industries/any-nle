//! Qualified interpretations of saved Premiere intrinsic clip components.
use super::components::field;
use crate::{
    audio::VOLUME_DB,
    video::{centered_crop_profile, roll::LinearRoll, validate_static_geometry},
    Result,
};
use graph::{picture_motion::MotionKey, Rational, Sidecar, StreamKind, Transform, Value, VolumeKey};
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{
        component::{component_inner, decode_value, ParamValue},
        Project, TrackItemId,
    },
    xml::{NodeId, Tree},
};

const MOTION: &str = "AE.ADBE Motion";
const MONO_VOLUME: &str = "Internal Volume Mono";
pub(super) const DEFAULT_TICKS: i64 = -91_445_760_000_000_000;

pub(super) enum ClipControl {
    StaticMotion(Transform),
    LinearMotion([MotionKey; 2]),
    LinearRoll(LinearRoll),
    MonoVolume(f64),
    MonoVolumeKeys([VolumeKey; 2]),
}

pub(super) fn interpret(
    project: &Project,
    item: TrackItemId,
    kind: StreamKind,
    data: &Value,
    source_range: (i64, i64),
    frame_ticks: i64,
    linear_retime: bool,
) -> Result<Option<ClipControl>> {
    let (source_in, source_out) = source_range;
    let item = project.track_item(item);
    let ([id], [], Value::Arr(components)) = (
        item.components.as_slice(),
        item.selection_components.as_slice(),
        data,
    ) else {
        return Ok(None);
    };
    let [receipt] = components.as_slice() else {
        return Ok(None);
    };
    let component = project.component(*id);
    let tree = &project.doc.tree;
    let Some(owner) = component_inner(tree, component.element) else {
        return Ok(None);
    };
    if !component.sub_components.is_empty() {
        return Ok(None);
    }
    for node in [component.element, owner] {
        for tag in ["Bypass", "IsBypassed", "Enabled", "IsEnabled"] {
            if field(tree, node, tag)?.is_some() {
                return Ok(None);
            }
        }
    }
    let (Some(Value::Str(name)), Some(Value::Arr(params))) =
        (receipt.get("match_name"), receipt.get("parameters"))
    else {
        return Ok(None);
    };
    match (kind, name.as_str()) {
        (StreamKind::Video, MOTION)
            if field(tree, item.element, "FrameRect")? == Some("0,0,1920,1080")
                && field(tree, item.element, "PixelAspectRatio")? == Some("1,1") =>
        {
            let source_in = source_in / frame_ticks;
            let source_out = source_out / frame_ticks;
            if frame_ticks == TICKS_PER_SECOND / 24 {
                if let Some(roll) = linear_roll(params, source_in, source_out, frame_ticks) {
                    return Ok(neutral_motion(tree, &component.properties, &[5])?
                        .then_some(ClipControl::LinearRoll(roll)));
                }
            }
            if linear_retime || frame_ticks == TICKS_PER_SECOND / 24 {
                if let Some(transform) = static_warp(params) {
                    return Ok(neutral_motion(tree, &component.properties, &[5, 8, 9, 10, 11])?
                        .then_some(ClipControl::StaticMotion(transform)));
                }
            }
            Ok(if neutral_motion(tree, &component.properties, &[])? {
                motion(params, source_in, source_out, frame_ticks)
            } else {
                None
            })
        }
        (StreamKind::Audio, MONO_VOLUME) if neutral_mute(project, owner)? => {
            Ok(mono_volume(params).map(ClipControl::MonoVolume).or_else(|| {
                mono_volume_keys(params, source_in, source_out, frame_ticks)
                    .map(ClipControl::MonoVolumeKeys)
            }))
        }
        _ => Ok(None),
    }
}

fn neutral_motion(tree: &Tree, params: &[NodeId], interpreted: &[usize]) -> Result<bool> {
    if params.len() != 11 {
        return Ok(false);
    }
    let mut seen = [false; 11];
    for &param in params {
        let Some(id) = field(tree, param, "ParameterID")?
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|id| (1..=11).contains(id))
        else {
            return Ok(false);
        };
        if std::mem::replace(&mut seen[id - 1], true) {
            return Ok(false);
        }
        if id <= 4 || interpreted.contains(&id) {
            continue;
        }
        if field(tree, param, "Keyframes")?.is_some()
            || !matches!(field(tree, param, "IsTimeVarying")?, None | Some("false"))
        {
            return Ok(false);
        }
        let Some(start) = field(tree, param, "StartKeyframe")? else {
            return Ok(false);
        };
        let parts: Vec<_> = start.split(',').take(15).collect();
        let neutral = |v| match decode_value(v) {
            Ok(ParamValue::List(point)) if id == 6 => point == [0.5, 0.5],
            Ok(ParamValue::Float(value)) if id != 6 => value == 0.0,
            _ => false,
        };
        let width = if id == 6 { 14 } else { 8 };
        if parts.len() != width
            || parts[0].parse::<i64>().ok() != Some(DEFAULT_TICKS)
            || parts[2..4] != ["0", "0"]
            || !neutral(parts[1])
            || field(tree, param, "CurrentValue")?.is_some_and(|v| !neutral(v))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn neutral_mute(project: &Project, owner: NodeId) -> Result<bool> {
    let tree = &project.doc.tree;
    let refs = tree.findall(owner, "Params/Param")?;
    let [mute, level] = refs.as_slice() else {
        return Ok(false);
    };
    if tree.get(*mute, "Index") != Some("0") || tree.get(*level, "Index") != Some("1") {
        return Ok(false);
    }
    let mute = project.doc.resolve(*mute)?;
    if tree.tag(mute) != "AudioComponentParam"
        || tree.get(mute, "ClassID") != Some("32657501-3aa4-445f-a49b-d09ecb9fa1ae")
        || !matches!(field(tree, mute, "IsTimeVarying")?, None | Some("false"))
    {
        return Ok(false);
    }
    for tag in ["Keyframes", "StartKeyframe", "CurrentValue"] {
        if field(tree, mute, tag)?.is_some() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn parameter<'a>(params: &'a [Value], name: &str) -> Option<&'a Value> {
    params
        .iter()
        .find(|param| matches!(param.get("parameter"), Some(Value::Str(n)) if n == name))
}

fn start_value(param: &Value) -> Option<&Value> {
    let start = param.get("start_keyframe")?;
    (start.get("ticks") == Some(&Value::Int(DEFAULT_TICKS)) && is_linear(start))
        .then_some(start.get("value")?)
}

fn is_linear(key: &Value) -> bool {
    key.get("interpolation_code_2") == Some(&Value::Int(0))
        && key.get("interpolation_code_3") == Some(&Value::Int(0))
}

fn number(value: &Value) -> Option<f64> {
    let Value::Float(value) = value else {
        return None;
    };
    value.is_finite().then_some(*value)
}

fn point(value: &Value) -> Option<(f64, f64)> {
    let Value::Arr(values) = value else {
        return None;
    };
    let [x, y] = values.as_slice() else {
        return None;
    };
    let (x, y) = (number(x)?, number(y)?);
    ((0.45..=0.55).contains(&x) && (0.45..=0.55).contains(&y)).then_some((x, y))
}

fn keys(param: &Value) -> Option<&[Value]> {
    let Value::Arr(values) = param.get("keyframes")? else {
        return None;
    };
    Some(values)
}

fn static_value(param: &Value) -> Option<&Value> {
    if !keys(param)?.is_empty()
        || !matches!(
            param.get("is_time_varying")?,
            Value::Null | Value::Bool(false)
        )
    {
        return None;
    }
    start_value(param)
}

fn static_motion_value(param: &Value) -> Option<&Value> {
    let value = static_value(param)?;
    let current = param.get("current_value")?;
    (current == &Value::Null || current == value).then_some(value)
}

fn mono_volume(params: &[Value]) -> Option<f64> {
    let [parameter] = params else { return None };
    let level = parameter.get("parameter")?;
    if !matches!(level, Value::Str(name) if name == "level") {
        return None;
    }
    let raw = number(static_value(parameter)?)?;
    if let current @ Value::Float(_) = parameter.get("current_value")? {
        if (number(current)? - raw).abs() > 1e-10 {
            return None;
        }
    }
    let db = clip_level_db(raw)?;
    let rounded = (db * 10.0).round() / 10.0;
    ((db - rounded).abs() < 1e-5 && VOLUME_DB.contains(&rounded)).then_some(rounded)
}

fn clip_level_db(raw: f64) -> Option<f64> {
    // Premiere's saved Level gives unity at -15 dB in its numeric domain.
    (raw > 0.0).then(|| 20.0 * raw.log10() + 15.0)
}

fn mono_volume_keys(
    params: &[Value],
    source_in: i64,
    source_out: i64,
    frame_ticks: i64,
) -> Option<[VolumeKey; 2]> {
    let [param] = params else { return None };
    if frame_ticks != TICKS_PER_SECOND / 24
        || !matches!(param.get("parameter"), Some(Value::Str(name)) if name == "level")
        || !matches!(param.get("is_time_varying")?, Value::Null | Value::Bool(true))
    {
        return None;
    }
    let [first, last] = keys(param)? else { return None };
    let parse = |key: &Value| {
        let Value::Int(ticks) = key.get("ticks")? else { return None };
        let Value::Arr(aux) = key.get("auxiliary_fields")? else { return None };
        if !is_linear(key) || aux.len() != 4 || ticks % frame_ticks != 0
            || !(source_in..source_out).contains(ticks)
        {
            return None;
        }
        Some(VolumeKey {
            at: Rational::new((*ticks).into(), TICKS_PER_SECOND.into())?,
            db: clip_level_db(number(key.get("value")?)?)?,
        })
    };
    let (first, last) = (parse(first)?, parse(last)?);
    (first.at < last.at).then_some([first, last])
}

pub(super) fn volume_key_receipt(owner: &str, keys: [VolumeKey; 2]) -> Sidecar {
    Sidecar {
        adapter: "premiere.clip_volume".into(),
        object: owner.into(),
        data: Value::Obj(vec![
            ("volume_key_clock".into(), Value::Str("item_source".into())),
            ("interpolation".into(), Value::Str("premiere_linear_level".into())),
            ("keys".into(), Value::Arr(keys.into_iter().map(|key| Value::Obj(vec![
                ("at".into(), Value::Str(key.at.to_string())),
                ("db".into(), Value::Float(key.db)),
            ])).collect())),
        ]),
    }
}

fn motion(
    params: &[Value],
    source_in: i64,
    source_out: i64,
    frame_ticks: i64,
) -> Option<ClipControl> {
    let position = parameter(params, "position")?;
    let scale = parameter(params, "scale")?;
    let width = number(static_motion_value(parameter(params, "scale_width")?)?)?;
    let Value::Bool(uniform) = static_motion_value(parameter(params, "uniform_scale")?)? else {
        return None;
    };
    let zoom_ok = |n: f64| (87.5..=112.5).contains(&n);
    if keys(position)?.is_empty() && keys(scale)?.is_empty() {
        let (x, y) = point(static_motion_value(position)?)?;
        let height = number(static_motion_value(scale)?)?;
        let width = if *uniform { height } else { width };
        if !zoom_ok(height) || !zoom_ok(width) {
            return None;
        }
        return Some(ClipControl::StaticMotion(Transform {
            position: (centered(x), centered(y)),
            scale: (width, height),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            crop: [0.0; 4],
        }));
    }
    if !uniform
        || width != 100.0
        || point(start_value(position)?)? != (0.5, 0.5)
        || number(start_value(scale)?)? != 100.0
        || position.get("current_value")? != &Value::Null
        || scale.get("current_value")? != &Value::Null
    {
        return None;
    }
    let [position_a, position_b] = linear_keys(position, source_in, source_out, frame_ticks)?;
    let [scale_a, scale_b] = linear_keys(scale, source_in, source_out, frame_ticks)?;
    let (xa, ya) = point(position_a)?;
    let (xb, yb) = point(position_b)?;
    let (za, zb) = (number(scale_a)?, number(scale_b)?);
    if (xa, ya, za) != (0.5, 0.5, 100.0) || !zoom_ok(zb) {
        return None;
    }
    let last = source_out.checked_sub(1)?;
    if source_in >= last || u32::try_from(last).is_err() {
        return None;
    }
    let time = |frame: i64| Rational::from_units(frame.checked_mul(frame_ticks)?, TICKS_PER_SECOND);
    Some(ClipControl::LinearMotion([
        MotionKey {
            at: time(source_in)?,
            position: (0.0, 0.0),
            scale_percent: 100.0,
        },
        MotionKey {
            at: time(last)?,
            position: (centered(xb), centered(yb)),
            scale_percent: zb,
        },
    ]))
}

fn centered(value: f64) -> f64 {
    // Input has already passed the coordinate bounds. Clamp only roundoff at the endpoints.
    (value - 0.5).clamp(-0.05, 0.05)
}

fn static_warp(params: &[Value]) -> Option<Transform> {
    let saved = |name| static_motion_value(parameter(params, name)?);
    if point(saved("position")?)? != (0.5, 0.5) {
        return None;
    }
    let height = number(saved("scale")?)?;
    let width = number(saved("scale_width")?)?;
    let Value::Bool(uniform) = saved("uniform_scale")? else {
        return None;
    };
    let transform = Transform {
        position: (0.0, 0.0),
        scale: (if *uniform { height } else { width }, height),
        rotation: number(saved("rotation")?)?,
        anchor: (0.0, 0.0),
        crop: [
            number(saved("crop_left")?)? / 100.0,
            number(saved("crop_top")?)? / 100.0,
            number(saved("crop_right")?)? / 100.0,
            number(saved("crop_bottom")?)? / 100.0,
        ],
    };
    (centered_crop_profile(&transform)
        || (transform.rotation != 0.0 && validate_static_geometry(&transform).is_ok()))
        .then_some(transform)
}

fn linear_roll(
    params: &[Value],
    source_in: i64,
    source_out: i64,
    frame_ticks: i64,
) -> Option<LinearRoll> {
    if number(static_motion_value(parameter(params, "scale")?)?)? != 100.0
        || static_motion_value(parameter(params, "uniform_scale")?)? != &Value::Bool(false)
    {
        return None;
    }
    let [position, width, rotation] = ["position", "scale_width", "rotation"]
        .map(|name| parameter(params, name).and_then(three_linear_keys));
    let (position, width, rotation) = (position?, width?, rotation?);
    let mut curves = std::array::from_fn(|_| Vec::with_capacity(3));
    let range = (-60.0 / 999.0)..=(90.0 / 999.0);
    for ((pos, width), angle) in position.into_iter().zip(width).zip(rotation) {
        let (frame, point) = source_key(pos, frame_ticks)?;
        let (width_frame, width) = source_key(width, frame_ticks)?;
        let (angle_frame, angle) = source_key(angle, frame_ticks)?;
        let Value::Arr(point) = point else {
            return None;
        };
        let [x, y] = point.as_slice() else {
            return None;
        };
        let x = number(x)? - 0.5;
        // Recenter the saved normalized point and admit only endpoint roundoff.
        let bounded_x = x.clamp(*range.start(), *range.end());
        if frame != width_frame || frame != angle_frame || number(y)? != 0.5
            || (x - bounded_x).abs() > f64::EPSILON
        {
            return None;
        }
        curves[0].push((frame, bounded_x));
        curves[1].push((frame, number(width)? / 100.0));
        curves[2].push((frame, -number(angle)?));
    }
    LinearRoll::from_saved(curves, source_in, source_out.checked_sub(source_in)?, source_out)
}

fn three_linear_keys(param: &Value) -> Option<[&Value; 3]> {
    let [first, middle, last] = keys(param)? else { return None };
    (param.get("is_time_varying")? == &Value::Bool(true)
        && param.get("current_value")? == &Value::Null
        && start_value(param)? == first.get("value")?)
        .then_some([first, middle, last])
}

fn source_key(key: &Value, frame_ticks: i64) -> Option<(u32, &Value)> {
    let Value::Int(ticks) = key.get("ticks")? else {
        return None;
    };
    if !is_linear_motion_key(key)? || ticks % frame_ticks != 0 {
        return None;
    }
    Some((u32::try_from(ticks / frame_ticks).ok()?, key.get("value")?))
}

fn is_linear_motion_key(key: &Value) -> Option<bool> {
    let Value::Arr(aux) = key.get("auxiliary_fields")? else {
        return None;
    };
    Some(is_linear(key)
        && (aux.len() == 4 || (aux.len() == 10
            && matches!(&aux[4..6], [Value::Str(a), Value::Str(b)] if a == "5" && b == "4"))))
}

fn linear_keys(
    param: &Value,
    source_in: i64,
    source_out: i64,
    frame_ticks: i64,
) -> Option<[&Value; 2]> {
    if param.get("is_time_varying")? != &Value::Bool(true) {
        return None;
    }
    let [first, last] = keys(param)? else {
        return None;
    };
    for (key, frame) in [(first, source_in), (last, source_out.checked_sub(1)?)] {
        if !is_linear_motion_key(key)?
            || key.get("ticks")? != &Value::Int(frame.checked_mul(frame_ticks)?)
        {
            return None;
        }
    }
    Some([first.get("value")?, last.get("value")?])
}
