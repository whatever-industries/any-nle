//! The saved half-frame position map of the retained 1080p24 ramp profile (AVB-0010).
//! Read the visible samples directly. The Speed keys help identify the profile.
use super::*;

const SPEED: &str = "8d56827c-847e-11d5-935a-50f857c10000";
const POSITION: &str = "8d56827d-847e-11d5-935a-50f857c10000";
const EDITABLE_POSITION: &str = "8d56827a-847e-11d5-935a-50f857c10000";
const AUXILIARY: &str = "8d568283-847e-11d5-935a-50f857c10000";
const FRAMES: i64 = 48;
pub(super) const SPARSE_FRAMES: usize = 1666;
const FLOAT_NOISE: f64 = 1e-9;

fn uuid(node: &Node) -> Option<String> {
    match node.get("uuid")? {
        Value::Uuid(id) => Some(id.to_string()),
        _ => None,
    }
}

fn points(curve: &Node) -> Option<&[Value]> {
    match curve.get("control_points")? {
        Value::List(points) => Some(points),
        _ => None,
    }
}

pub(in crate::extract::project) fn is_sampled_effect(ctx: &Ctx<'_>, effect: &Node) -> bool {
    effect.get("effect_id").and_then(Value::as_str) == Some("EFF_ADV_MOTION_CTL")
        && ctx.deref(effect.get("param_list")).is_some_and(|list| {
            let Some(Value::RefList(items)) = list.get("items") else {
                return false;
            };
            items
                .iter()
                .filter_map(|item| ctx.deref(Some(item)))
                .any(|item| {
                    let id = uuid(item);
                    let minimum = match id.as_deref() {
                        Some(SPEED) => 2,
                        Some(POSITION) => 3,
                        _ => return false,
                    };
                    ctx.deref(item.get("control_track"))
                        .and_then(points)
                        .is_some_and(|points| points.len() >= minimum)
                })
        })
}

/// The default named presets select a sparse editable Position curve and save a
/// unity span ratio. The selected curve needs its own cadence qualification.
pub(in crate::extract::project) fn uses_editable_position(ctx: &Ctx<'_>, effect: &Node) -> bool {
    if effect.get("effect_id").and_then(Value::as_str) != Some("EFF_ADV_MOTION_CTL") {
        return false;
    }
    let attrs = ctx
        .deref(effect.get("attributes"))
        .and_then(|n| n.get("entries"));
    if !matches!(attrs, Some(Value::Attrs(attrs)) if attrs.iter().any(|(key, value)|
        key == "_MFX_GRAPH_ENABLED" && value.as_int() == Some(2)))
    {
        return false;
    }
    let params = ctx
        .deref(effect.get("param_list"))
        .and_then(|n| n.get("items"));
    let Some(Value::RefList(items)) = params else {
        return false;
    };
    items
        .iter()
        .filter_map(|item| ctx.deref(Some(item)))
        .any(|item| {
            uuid(item).as_deref() == Some(EDITABLE_POSITION)
                && item.get("enable") == Some(&Value::Bool(true))
        })
}

pub(in crate::extract::project) fn direct_input<'a>(
    ctx: &Ctx<'a>,
    effect: &Node,
) -> Option<&'a Node> {
    let Value::List(tracks) = effect.get("tracks")? else {
        return None;
    };
    let [Value::Node(track)] = tracks.as_slice() else {
        return None;
    };
    let component = ctx.deref(track.get("component"))?;
    let input = if component.class == *b"SEQU" {
        let Value::RefList(parts) = component.get("components")? else {
            return None;
        };
        if !(1..=3).contains(&parts.len())
            || float(component, "edit_rate") != float(effect, "edit_rate")
            || int(component, "media_kind_id") != Some(KIND_PICTURE)
        {
            return None;
        }
        let mut clip = None;
        for part in parts {
            let part = ctx.deref(Some(part))?;
            if part.class == *b"FILL" && int(part, "length") == Some(0) {
                continue;
            }
            if clip.replace(part).is_some() {
                return None;
            }
        }
        clip?
    } else {
        component
    };
    (input.class == *b"SCLP" && null(input, "precomputed") && null(input, "param_list"))
        .then_some(input)
}

fn null(node: &Node, field: &str) -> bool {
    matches!(node.get(field), None | Some(Value::Null))
}

pub(super) fn master_picture_matches(ctx: &Ctx<'_>, input: &Node) -> bool {
    let Some(master) = mob_id(input).and_then(|id| ctx.mobs.get(&id)) else {
        return false;
    };
    if int(master, "mob_type_id") != Some(MOB_MASTER) {
        return false;
    }
    let mut picture = ctx.tracks(master).into_iter().filter(|track| {
        ctx.deref(track.get("component"))
            .is_some_and(|n| int(n, "media_kind_id") == Some(KIND_PICTURE))
    });
    let label = picture.next().and_then(|track| int(track, "index"));
    label.is_some() && label == int(input, "track_id") && picture.next().is_none()
}

fn saved_settings(ctx: &Ctx<'_>, effect: &Node) -> Option<()> {
    let attrs = ctx.deref(effect.get("attributes"))?;
    let Value::Attrs(entries) = attrs.get("entries")? else {
        return None;
    };
    for (name, expected) in [
        ("_MFX_INPUT_FORMAT", 2),
        ("_MFX_OUTPUT_FORMAT", 2),
        ("_SFX_RENDER", 2),
        ("_SFX_IGNORE_RENDER_SETTING", 1),
        ("_NEW_SHAPE_REFERENCE_FRAME", 1),
    ] {
        let mut matches = entries.iter().filter(|(key, _)| key == name);
        if matches.next()?.1.as_int() != Some(expected) || matches.next().is_some() {
            return None;
        }
    }
    entries
        .iter()
        .all(|(k, _)| {
            matches!(
                k.as_str(),
                "_MFX_INPUT_FORMAT"
                    | "_MFX_OUTPUT_FORMAT"
                    | "_SFX_RENDER"
                    | "_SFX_IGNORE_RENDER_SETTING"
                    | "_NEW_SHAPE_REFERENCE_FRAME"
                    | "_MFX_VIEW_SPD_TOP"
                    | "_MFX_VIEW_SPD_BOTTOM"
                    | "_FX_CURRENT_KF"
                    | "_TMP_CRM"
            )
        })
        .then_some(())
}

fn curve<'a>(
    ctx: &Ctx<'a>,
    item: &'a Node,
    value_type: i64,
    length: i64,
    rate: f64,
    extrap: i64,
) -> Option<&'a Node> {
    let inline_zero = if value_type == 2 {
        float(item, "value") == Some(0.0)
    } else {
        int(item, "value") == Some(0)
    };
    let curve = ctx.deref(item.get("control_track"))?;
    (item.class == *b"PRIT"
        && item.get("enable") == Some(&Value::Bool(true))
        && item.get("contribs_to_sig") == Some(&Value::Bool(true))
        && int(item, "value_type") == Some(value_type)
        && inline_zero
        && curve.class == *b"PRCL"
        && float(curve, "edit_rate") == Some(rate)
        && int(curve, "media_kind_id") == Some(10)
        && int(curve, "length") == Some(length)
        && int(curve, "value_type") == Some(value_type)
        && int(curve, "extrap_kind") == Some(extrap)
        && int(curve, "fields") == Some(1)
        && null(curve, "param_list")
        && null(curve, "precomputed"))
    .then_some(curve)
}

fn point(value: &Value, twice_frame: i64) -> Option<&Node> {
    point_with_scale(value, twice_frame, 1)
}

fn point_with_scale(value: &Value, twice_frame: i64, scale: i64) -> Option<&Node> {
    let Value::Node(node) = value else {
        return None;
    };
    let Some(Value::List(offset)) = node.get("offset") else {
        return None;
    };
    let [Value::Int(num), Value::Int(den)] = offset.as_slice() else {
        return None;
    };
    (*den > 0
        && i128::from(*num) * 2 == i128::from(*den) * i128::from(twice_frame)
        && int(node, "timescale") == Some(scale))
    .then_some(node)
}

fn no_properties(node: &Node) -> bool {
    matches!(node.get("pp"), Some(Value::List(props)) if props.is_empty())
}

fn speed_point(value: &Value, end: bool) -> Option<f64> {
    let node = point(value, if end { (FRAMES - 1) * 2 } else { 0 })?;
    let Some(Value::List(props)) = node.get("pp") else {
        return None;
    };
    let handles = [(5, -18.8), (6, 0.0), (7, 18.8), (8, 0.0)];
    let expected = if end { 5 } else { 6 };
    if props.len() != expected {
        return None;
    }
    let mut seen = 0u16;
    for prop in props {
        let Value::Node(prop) = prop else {
            return None;
        };
        let code = int(prop, "code")?;
        let bit = 1u16.checked_shl(u32::try_from(code).ok()?)?;
        if seen & bit != 0 {
            return None;
        }
        seen |= bit;
        match code {
            9 if int(prop, "type") == Some(1) && int(prop, "value") == Some(1) => {}
            14 if !end && int(prop, "type") == Some(2) && float(prop, "value") == Some(0.0) => {}
            _ if int(prop, "type") == Some(2)
                && handles
                    .iter()
                    .any(|&(c, value)| c == code && float(prop, "value") == Some(value)) => {}
            _ => return None,
        }
    }
    float(node, "value")
}

#[derive(Clone, Copy)]
struct PositionKey {
    value: f64,
    incoming: [f64; 2],
    outgoing: [f64; 2],
}

fn preset_key(raw: &Value, frame: i64) -> Option<PositionKey> {
    let point = point_with_scale(raw, 2 * frame, 4)?;
    let Some(Value::List(props)) = point.get("pp") else {
        return None;
    };
    if props.len() != 5 {
        return None;
    }
    let mut handles = [None; 4];
    let mut flag = false;
    for prop in props {
        let Value::Node(prop) = prop else {
            return None;
        };
        match int(prop, "code")? {
            code @ 5..=8 if int(prop, "type") == Some(2) => {
                if handles[(code - 5) as usize]
                    .replace(float(prop, "value")?)
                    .is_some()
                {
                    return None;
                }
            }
            9 if !flag && int(prop, "type") == Some(1) && int(prop, "value") == Some(1) => {
                flag = true;
            }
            _ => return None,
        }
    }
    let [ix, iy, ox, oy] = handles;
    Some(PositionKey {
        value: float(point, "value")?,
        incoming: [ix?, iy?],
        outgoing: [ox?, oy?],
    })
}

fn cubic(points: [f64; 4], t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * points[0]
        + 3.0 * u * u * t * points[1]
        + 3.0 * u * t * t * points[2]
        + t * t * t * points[3]
}

/// The two native 23.976 presets use four Position keys and saved handles.
/// The qualified range includes the last key, so no extrapolation is needed.
pub(super) fn visible_sparse_frames(
    ctx: &Ctx<'_>,
    effect: &Node,
    input: &Node,
    ratio: (i64, i64),
) -> Option<[i64; SPARSE_FRAMES]> {
    let length = SPARSE_FRAMES as i64;
    if ratio != (1, 1)
        || int(effect, "length") != Some(length)
        || float(effect, "edit_rate") != Some(23.976)
        || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(effect, "mc_mode") != Some(0)
        || int(effect, "num_scalars") != Some(0)
        || int(effect, "phase_offset") != Some(0)
        || float(effect, "offset_adjust") != Some(0.0)
        || effect.get("new_source_calculation") != Some(&Value::Bool(true))
        || !null(effect, "source_param_list")
        || !null(effect, "precomputed")
        || float(input, "edit_rate") != Some(23.976)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
        || int(input, "length") != Some(length)
        || int(input, "start_time") != Some(0)
    {
        return None;
    }
    let attrs = ctx.deref(effect.get("attributes"))?;
    let Value::Attrs(entries) = attrs.get("entries")? else {
        return None;
    };
    let expected = [
        ("_MFX_INPUT_FORMAT", 2),
        ("_MFX_OUTPUT_FORMAT", 2),
        ("_MFX_GRAPH_ENABLED", 2),
        ("_SFX_IGNORE_RENDER_SETTING", 0),
        ("_SFX_RENDER", 0),
        ("_NEW_SHAPE_REFERENCE_FRAME", 1),
    ];
    if entries.len() != expected.len()
        || !expected.iter().all(|(name, value)| {
            entries
                .iter()
                .any(|(key, found)| key == name && found.as_int() == Some(*value))
        })
    {
        return None;
    }
    let list = ctx.deref(effect.get("param_list"))?;
    let Value::RefList(items) = list.get("items")? else {
        return None;
    };
    let [first, second] = items.as_slice() else {
        return None;
    };
    let first = ctx.deref(Some(first))?;
    let second = ctx.deref(Some(second))?;
    let (position, aux) = match (uuid(first)?.as_str(), uuid(second)?.as_str()) {
        (EDITABLE_POSITION, AUXILIARY) => (first, second),
        (AUXILIARY, EDITABLE_POSITION) => (second, first),
        _ => return None,
    };
    let position = curve(ctx, position, 2, length, 23.976, 2)?;
    let auxiliary = curve(ctx, aux, 1, length, 23.976, 1)?;
    let [aux] = points(auxiliary)? else {
        return None;
    };
    let aux = point(aux, 0)?;
    if int(position, "interp_kind") != Some(6)
        || int(auxiliary, "interp_kind") != Some(2)
        || int(aux, "value") != Some(0)
        || !no_properties(aux)
    {
        return None;
    }
    let [a, b, c, d] = points(position)? else {
        return None;
    };
    let times = [0, 416, 1249, 1665];
    let keys = [
        preset_key(a, times[0])?,
        preset_key(b, times[1])?,
        preset_key(c, times[2])?,
        preset_key(d, times[3])?,
    ];
    let (edge, join) = match keys.map(|p| p.value) {
        [0.0, 208.125, 1456.875, 1665.0] => (83.25, 55.45556445156125),
        [0.0, 624.375, 1040.625, 1665.0] => (249.75, 110.91112890312252),
        _ => return None,
    };
    let mid = join * 833.0 / 416.0;
    let handles = [
        ([-166.4, -edge], [166.4, edge]),
        ([-166.4, -join], [333.2, mid]),
        ([-333.2, -mid], [166.4, join]),
        ([-166.4, -edge], [166.4, edge]),
    ];
    for (key, (incoming, outgoing)) in keys.iter().zip(handles) {
        if !key
            .incoming
            .into_iter()
            .zip(incoming)
            .chain(key.outgoing.into_iter().zip(outgoing))
            .all(|(actual, expected)| (actual - expected).abs() <= FLOAT_NOISE)
        {
            return None;
        }
    }
    let mut frames = [0; SPARSE_FRAMES];
    for i in 0..3 {
        let start = times[i];
        let end = times[i + 1];
        let a = keys[i];
        let b = keys[i + 1];
        let x = [
            start as f64,
            start as f64 + a.outgoing[0],
            end as f64 + b.incoming[0],
            end as f64,
        ];
        let y = [
            a.value,
            a.value + a.outgoing[1],
            b.value + b.incoming[1],
            b.value,
        ];
        frames[start as usize] = a.value.floor() as i64;
        for frame in start + 1..end {
            let (mut low, mut high) = (0.0, 1.0);
            for _ in 0..48 {
                let mid = (low + high) / 2.0;
                if cubic(x, mid) < frame as f64 {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            frames[frame as usize] = cubic(y, (low + high) / 2.0).floor() as i64;
        }
    }
    frames[SPARSE_FRAMES - 1] = length - 1;
    (frames.windows(2).all(|p| p[0] <= p[1])).then_some(frames)
}

/// None inside Some means the qualified flat control needs no new map.
pub(super) fn visible_frames(
    ctx: &Ctx<'_>,
    effect: &Node,
    input: &Node,
    ratio: (i64, i64),
) -> Option<Option<[i64; FRAMES as usize]>> {
    if int(effect, "length") != Some(FRAMES)
        || float(effect, "edit_rate") != Some(24.0)
        || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(effect, "mc_mode") != Some(0)
        || int(effect, "num_scalars") != Some(0)
        || int(effect, "phase_offset") != Some(0)
        || float(effect, "offset_adjust") != Some(0.0)
        || effect.get("new_source_calculation") != Some(&Value::Bool(true))
        || !null(effect, "source_param_list")
        || !null(effect, "precomputed")
        || float(input, "edit_rate") != Some(24.0)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
    {
        return None;
    }
    saved_settings(ctx, effect)?;
    let list = ctx.deref(effect.get("param_list"))?;
    let Value::RefList(items) = list.get("items")? else {
        return None;
    };
    if items.len() != 3 {
        return None;
    }
    let mut speed = None;
    let mut position = None;
    let mut auxiliary = None;
    for reference in items {
        let item = ctx.deref(Some(reference))?;
        let (slot, ty) = match uuid(item)?.as_str() {
            SPEED => (&mut speed, 2),
            POSITION => (&mut position, 2),
            AUXILIARY => (&mut auxiliary, 1),
            _ => return None,
        };
        if slot.is_some() {
            return None;
        }
        *slot = Some(curve(ctx, item, ty, FRAMES, 24.0, 1)?);
    }
    let auxiliary = auxiliary?;
    let [aux] = points(auxiliary)? else {
        return None;
    };
    let aux = point(aux, 0)?;
    if int(auxiliary, "interp_kind") != Some(2)
        || int(aux, "value") != Some(0)
        || !no_properties(aux)
    {
        return None;
    }
    let speed = speed?;
    let [first, last] = points(speed)? else {
        return None;
    };
    let start = speed_point(first, false)?;
    let end = speed_point(last, true)?;
    let mode = int(speed, "interp_kind")?;
    let length = int(input, "length")?;
    let flat = match (start, end, mode, ratio, length) {
        (1.0, 1.0, 6, (1, 1), 48) => true,
        (0.5, 2.0, 3 | 6, (48, 59), 59) | (2.0, 0.5, 3, (48, 59), 59) => false,
        _ => return None,
    };
    let position = position?;
    let samples = points(position)?;
    if int(position, "interp_kind") != Some(3) || samples.len() != (2 * FRAMES + 1) as usize {
        return None;
    }
    let mut visible = [0; FRAMES as usize];
    let mut previous = 0.0;
    let mut previous_step = 0.0;
    for (i, raw) in samples.iter().enumerate() {
        let sample = point(raw, i as i64)?;
        let value = float(sample, "value")?;
        if !no_properties(sample) || !value.is_finite() || value < 0.0 {
            return None;
        }
        if flat {
            if (value - i as f64 / 2.0).abs() > FLOAT_NOISE {
                return None;
            }
            continue;
        }
        let step = value - previous;
        if i == 0 {
            if value != 0.0 {
                return None;
            }
        } else if i == 1 {
            // Check the saved mode agrees with the shape near its starting key.
            let departure = (step - start / 2.0) * (end - start).signum();
            if !(if mode == 3 {
                (0.002..0.003).contains(&departure)
            } else {
                (0.0..0.0001).contains(&departure)
            }) {
                return None;
            }
        } else if (end - start) * (step - previous_step) < -FLOAT_NOISE
            || !(0.25 - FLOAT_NOISE..=1.0 + FLOAT_NOISE).contains(&step)
            || (i >= samples.len() - 2 && (step - end / 2.0).abs() > FLOAT_NOISE)
        {
            return None;
        }
        previous = value;
        previous_step = step;
        if i % 2 == 0 && i / 2 < visible.len() {
            // Nonzero near-integer samples need their own rounding evidence.
            if i > 0 && (value - value.round()).abs() <= FLOAT_NOISE {
                return None;
            }
            let frame = value.floor() as i64;
            if frame >= length {
                return None;
            }
            visible[i / 2] = frame;
        }
    }
    if flat {
        return Some(None);
    }
    (visible.last() == Some(&(length - 1))).then_some(Some(visible))
}
