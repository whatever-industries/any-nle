//! The saved half-frame position map of the retained 1080p24 ramp profile (AVB-0010).
//! Read the visible samples directly. The Speed keys help identify the profile.
use super::*;
use super::param_keys::key_parts;

const SPEED: &str = "8d56827c-847e-11d5-935a-50f857c10000";
const POSITION: &str = "8d56827d-847e-11d5-935a-50f857c10000";
const EDITABLE_POSITION: &str = "8d56827a-847e-11d5-935a-50f857c10000";
const AUXILIARY: &str = "8d568283-847e-11d5-935a-50f857c10000";
const FRAMES: i64 = 48;
pub(super) const MAX_SAVED_PICTURE_FRAMES: i64 = 4096;
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
    single_clip(ctx, component, effect)
}

fn single_clip<'a>(ctx: &Ctx<'a>, component: &'a Node, effect: &Node) -> Option<&'a Node> {
    let clip = single_component(ctx, component, effect)?;
    (clip.class == *b"SCLP" && null(clip, "precomputed") && null(clip, "param_list"))
        .then_some(clip)
}

pub(super) fn single_component<'a>(ctx: &Ctx<'a>, component: &'a Node, effect: &Node) -> Option<&'a Node> {
    if component.class == *b"SEQU" {
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
        clip
    } else {
        Some(component)
    }
}

fn null(node: &Node, field: &str) -> bool {
    matches!(node.get(field), None | Some(Value::Null))
}

/// Retain the saved controls even when their application to the source route
/// or to the renderer is unresolved. Sample values are relative to the
/// motion owner and are not a file-relative frame map.
pub(super) fn observe(
    ctx: &Ctx<'_>, effect: &Node, budget: &SharedBudget,
) -> Option<Box<SourceTimewarpControls>> {
    const MAX_CURVES: usize = 16;
    if !charge(budget, size_of::<SourceTimewarpControls>()) { return None; }
    let attrs = borrowed_attrs(ctx, effect);
    let attr = |name| unique_attribute(attrs, name).and_then(Value::as_int);
    let input = |node: &Node| Some(TimewarpInput {
        mob: mob_id(node)?,
        track: int(node, "track_id"),
        start: int(node, "start_time"),
        length: int(node, "length"),
        rate: float(node, "edit_rate").and_then(Rate::from_avid),
    });
    let mut curves = Vec::new();
    if let Some(Value::RefList(items)) = ctx.deref(effect.get("param_list")).and_then(|n| n.get("items")) {
        if items.len() <= MAX_CURVES && reserve(budget, &mut curves, items.len()) {
            for item in items.iter().filter_map(|item| ctx.deref(Some(item))) {
                let Some(Value::Uuid(parameter)) = item.get("uuid") else { continue; };
                let track = ctx.deref(item.get("control_track"));
                let saved = track.and_then(points);
                let (interpolation, extrapolation, fields) = track.map_or((None, None, None), |n|
                    (int(n, "interp_kind"), int(n, "extrap_kind"), int(n, "fields")));
                curves.push(SourceTimewarpCurve {
                    parameter: *parameter,
                    enabled: item.get("enable").and_then(flag),
                    interpolation, extrapolation, fields,
                    point_count: saved.map_or(0, <[_]>::len),
                    points: saved.and_then(|points| observe_points(points, interpolation, budget)),
                });
            }
        }
    }
    let span_ratio = match effect.get("speed_ratio") {
        Some(Value::List(values)) => match values.as_slice() {
            [Value::Int(n), Value::Int(d)] => Some((*n, *d)),
            _ => None,
        },
        _ => None,
    };
    Some(Box::new(SourceTimewarpControls {
        span_ratio,
        phase_offset: int(effect, "phase_offset"),
        offset_adjust: float(effect, "offset_adjust").filter(|v| v.is_finite()),
        render_mode: attr("_SFX_RENDER"),
        ignore_render_setting: attr("_SFX_IGNORE_RENDER_SETTING"),
        graph_mode: attr("_MFX_GRAPH_ENABLED"),
        input_format: attr("_MFX_INPUT_FORMAT"),
        output_format: attr("_MFX_OUTPUT_FORMAT"),
        new_source_calculation: effect.get("new_source_calculation").and_then(flag),
        input: direct_input(ctx, effect).and_then(input),
        precompute: saved_render_clip(ctx, effect).and_then(input),
        precompute_present: !null(effect, "precomputed"),
        resize_render_owner: None,
        curves,
    }))
}

fn flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(value) => Some(*value),
        _ => None,
    }
}

fn observe_points(
    points: &[Value], interpolation: Option<i64>, budget: &SharedBudget,
) -> Option<SourceTimewarpPoints> {
    const MAX_SAMPLES: usize = 2 * MAX_SAVED_PICTURE_FRAMES as usize + 1;
    if interpolation == Some(3) && (3..=MAX_SAMPLES).contains(&points.len()) {
        let (_, num, den, _, _) = key_parts(points.first()?)?;
        let first = num.checked_mul(2)?.checked_div(den)?;
        let samples = points.iter().enumerate().map(|(index, raw)| {
            let (node, num, den, timescale, value) = key_parts(raw)?;
            let position = first.checked_add(index as i64)?;
            (timescale == 1 && no_properties(node) &&
                i128::from(num) * 2 == i128::from(den) * i128::from(position)).then_some(value)
        });
        if samples.clone().all(|point| point.is_some()) {
            let mut values = Vec::new();
            if !reserve(budget, &mut values, points.len()) { return None; }
            values.extend(samples.flatten());
            return Some(SourceTimewarpPoints::HalfFrameSamples { first, values });
        }
    }
    param_keys::observe(points, budget).map(SourceTimewarpPoints::Keys)
}

fn saved_render_clip<'a>(ctx: &Ctx<'a>, effect: &Node) -> Option<&'a Node> {
    let render = ctx.deref(effect.get("precomputed"))?;
    if !null(render, "precomputed") || !null(render, "param_list") {
        return None;
    }
    single_clip(ctx, render, effect)
}

fn resize_over_timewarp(ctx: &Ctx<'_>, node: &Node, input: &Node, length: i64) -> bool {
    int(node, "length") == Some(length) && int(node, "info_is_reversed") == Some(0)
        && int(node, "media_kind_id") == Some(KIND_PICTURE)
        && single_component(ctx, input, node).is_some_and(|child| {
            child.class == *b"SPED"
                && child.get("effect_id").and_then(Value::as_str) == Some("EFF_ADV_MOTION_CTL")
                && float(child, "edit_rate") == float(node, "edit_rate")
                && int(child, "media_kind_id") == Some(KIND_PICTURE)
                && int(child, "length") == Some(length)
        })
}

/// The occurrence itself names this precompute. Keep the saved record-relative
/// trim when it lies entirely inside a single same-rate render package.
pub(super) fn precompute(
    ctx: &Ctx<'_>, effect: &Node, length: i64, rate: Rate, ambiguous: &HashSet<MobId>,
) -> Option<[(MobId, i64, i64); 3]> {
    let at_rate = |n: &Node| float(n, "edit_rate").and_then(Rate::from_avid) == Some(rate);
    if !at_rate(effect) { return None; }
    let picture = |n: &Node| {
        let Value::List(tracks) = n.get("tracks")? else { return None };
        let [Value::Node(track)] = tracks.as_slice() else { return None };
        let node = ctx.deref(track.get("component"))?;
        (int(track, "index") == Some(1) && int(node, "media_kind_id") == Some(KIND_PICTURE))
            .then_some(node)
    };
    let clip = |n: &Node, span: i64| {
        (n.class == *b"SCLP" && at_rate(n) && int(n, "media_kind_id") == Some(KIND_PICTURE)
            && int(n, "track_id") == Some(1) && int(n, "length") == Some(span)
            && null(n, "precomputed") && null(n, "param_list"))
            .then(|| Some((mob_id(n)?, int(n, "start_time")?)))?
    };
    let valid_effect = |node: &Node, span| {
        match (&node.class, node.get("effect_id").and_then(Value::as_str)) {
            (b"SPED", Some("EFF_ADV_MOTION_CTL")) => true,
            (b"TKFX", Some("EFF2_BLEND_RESIZE")) => Some(rate) == Rate::new(24, 1)
                && picture(node).is_some_and(|input| resize_over_timewarp(ctx, node, input, span)),
            _ => false,
        }
    };
    if !valid_effect(effect, length) { return None; }
    let (master_id, start) = clip(saved_render_clip(ctx, effect)?, length)?;
    let master = ctx.mobs.get(&master_id)?;
    let span = int(master, "length")?;
    if int(master, "mob_type_id") != Some(MOB_MASTER)
        || int(master, "usage_code") != Some(1) || !at_rate(master)
        || start < 0 || start.checked_add(length)? > span
    { return None; }
    let (file_id, 0) = clip(picture(master)?, span)? else { return None };
    let file = ctx.mobs.get(&file_id)?;
    if int(file, "mob_type_id") != Some(MOB_SOURCE) || int(file, "usage_code") != Some(9)
        || !at_rate(file) || int(file, "length") != Some(span)
    { return None; }
    let descriptor = ctx.deref(file.get("descriptor"))?;
    if descriptor.class != *b"CDCI" || !at_rate(descriptor)
        || int(descriptor, "length") != Some(span) || int(descriptor, "frame_layout") != Some(0)
    { return None; }
    let (origin_id, 0) = clip(picture(file)?, span)? else { return None };
    let origin = ctx.mobs.get(&origin_id)?;
    let saved = picture(origin)?;
    if int(origin, "mob_type_id") != Some(MOB_SOURCE) || int(origin, "usage_code") != Some(14)
        || !at_rate(origin) || int(origin, "length") != Some(span)
        || !valid_effect(saved, span) || !at_rate(saved) || int(saved, "length") != Some(span)
        || saved.get("effect_id") != effect.get("effect_id") || !null(saved, "precomputed")
    { return None; }
    let ids = [master_id, file_id, origin_id];
    if ids.iter().enumerate().any(|(n, id)| ambiguous.contains(id) || ids[..n].contains(id)) {
        return None;
    }
    Some([(master_id, start, length), (file_id, 0, span), (origin_id, 0, span)])
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

// Recover a reduced slope that round-trips to the saved double. The native
// span ratio describes source coverage, so it cannot supply the slope.
fn positive_speed_ratio(speed: f64) -> Option<(i64, i64)> {
    if !speed.is_finite() || speed <= 0.0 {
        return None;
    }
    let mut remainder = speed;
    let (mut previous, mut current) = ((0_i128, 1_i128), (1_i128, 0_i128));
    for _ in 0..64 {
        let whole = remainder.floor();
        if !whole.is_finite() || whole >= i64::MAX as f64 {
            return None;
        }
        let next = (whole as i128 * current.0 + previous.0,
                    whole as i128 * current.1 + previous.1);
        let ratio = (i64::try_from(next.0).ok()?, i64::try_from(next.1).ok()?);
        if ratio.0 > 0 && ratio.0 as f64 / ratio.1 as f64 == speed {
            return Some(ratio);
        }
        (previous, current) = (current, next);
        remainder = (remainder - whole).recip();
    }
    None
}

#[test]
fn nested_speed_recovers_reduced_slope_from_saved_double() {
    for (num, den) in [(1, 4), (4, 5), (5, 4), (5, 2), (1, 3), (1, 11), (1247, 1000)] {
        assert_eq!(positive_speed_ratio(num as f64 / den as f64), Some((num, den)));
    }
    for speed in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        assert_eq!(positive_speed_ratio(speed), None);
    }
}

/// These are saved controls in the untrimmed motion owner's clock. The frame
/// span ratio alone does not establish the rendered sampling.
pub(super) fn observe_subclip_motion(
    ctx: &Ctx<'_>, effect: &Node, trim_start: i64, trim_length: i64, ambiguous: &HashSet<MobId>,
    rate: Rate, allow_other_speeds: bool,
) -> Option<SourceSubclipMotion> {
    let advanced = match effect.get("effect_id")?.as_str()? {
        "EFF_TIMEWARP.MOTION_CTL" => false,
        "EFF_ADV_MOTION_CTL" => true,
        _ => return None,
    };
    let length = int(effect, "length").filter(|&length| length > 0)?;
    let Value::List(span) = effect.get("speed_ratio")? else { return None };
    let [Value::Int(num), Value::Int(den)] = span.as_slice() else { return None };
    let input = direct_input(ctx, effect)?;
    let curve_rate = float(effect, "edit_rate")?;
    if *num <= 0 || *den <= 0 || effect.class != *b"SPED"
        || Rate::from_avid(curve_rate) != Some(rate)
        || float(input, "edit_rate").and_then(Rate::from_avid) != Some(rate)
        || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
        || !null(effect, "source_param_list") || !null(effect, "precomputed")
    {
        return None;
    }
    let params = ctx.deref(effect.get("param_list")).filter(|n| n.class == *b"PRLS")?;
    let Value::RefList(items) = params.get("items")? else { return None };
    if !(2..=4).contains(&items.len()) {
        return None;
    }
    let mut speed = None;
    let mut position = None;
    let mut auxiliary = None;
    let mut editable = None;
    let mut distinct_aux = true;
    for item in items {
        let item = ctx.deref(Some(item))?;
        let slot = match uuid(item)?.as_str() {
            SPEED => &mut speed,
            POSITION => &mut position,
            AUXILIARY => {
                distinct_aux &= auxiliary.replace(item).is_none();
                continue;
            }
            EDITABLE_POSITION => {
                distinct_aux &= editable.replace(item).is_none();
                continue;
            }
            _ => return None,
        };
        if slot.replace(curve(ctx, item, 2, length, curve_rate, 1)?).is_some() {
            return None;
        }
    }
    let speed = speed.filter(|node| int(node, "interp_kind") == Some(6))?;
    let position = position.filter(|node| int(node, "interp_kind") == Some(3))?;
    let [speed] = points(speed)? else { return None };
    let [first, last] = points(position)? else { return None };
    let Value::Node(speed) = speed else { return None };
    let Value::List(key_frame) = speed.get("offset")? else { return None };
    let [Value::Int(key_num), Value::Int(key_den)] = key_frame.as_slice() else { return None };
    if *key_den <= 0 || int(speed, "timescale") != Some(1) {
        return None;
    }
    let speed_props = speed.get("pp");
    let speed = float(speed, "value").filter(|v| v.is_finite() && *v > 0.0)?;
    let first = point(first, 0).filter(|node| no_properties(node))?;
    let last = point(last, length.checked_mul(2)?).filter(|node| no_properties(node))?;
    let start = float(first, "value")?;
    let end = float(last, "value")?;
    if start != 0.0 || !end.is_finite() || end != speed * length as f64 {
        return None;
    }
    let attrs = borrowed_attrs(ctx, effect);
    let render_mode = unique_attribute(attrs, "_SFX_RENDER")?.as_int()?;
    let graph_mode = if attrs.iter().any(|(key, _)| key == "_MFX_GRAPH_ENABLED") {
        Some(unique_attribute(attrs, "_MFX_GRAPH_ENABLED")?.as_int()?)
    } else {
        None
    };
    let Value::Bool(new_source_calculation) = effect.get("new_source_calculation")? else { return None };
    let phase_offset = int(effect, "phase_offset")?;
    let offset_adjust = float(effect, "offset_adjust").filter(|v| v.is_finite())?;
    let ignore_render_setting = unique_attribute(attrs, "_SFX_IGNORE_RENDER_SETTING")?.as_int()?;
    let input_mob = mob_id(input)?;
    let input_start = int(input, "start_time").filter(|&start| start >= 0)?;
    let input_length = int(input, "length").filter(|&length| length > 0)?;
    let sampled_input_frames = (|| {
        // Native same-rate Speed graphs select floor(frame * speed) in the
        // motion owner's clock. Retain the fraction lost by the first sample.
        let (rate_num, rate_den) = positive_speed_ratio(speed)?;
        let double_speed = (rate_num, rate_den) == (2, 1);
        if !double_speed && !allow_other_speeds {
            return None;
        }
        if advanced || (*key_num, *key_den) != (0, 1)
            || phase_offset != 0 || offset_adjust != 0.0 || !new_source_calculation
            || render_mode != 2 || ignore_render_setting != 0 || graph_mode != Some(1)
            || int(effect, "mc_mode") != Some(0) || int(effect, "num_scalars") != Some(0)
            || !distinct_aux || ambiguous.contains(&input_mob)
            || !["_MFX_INPUT_FORMAT", "_MFX_OUTPUT_FORMAT"].iter().all(|name|
                unique_attribute(attrs, name).and_then(Value::as_int) == Some(2))
            || unique_attribute(attrs, "_NEW_SHAPE_REFERENCE_FRAME").and_then(Value::as_int) != Some(1)
            || attrs.iter().any(|(name, value)| match name.as_str() {
                "_MIXMATCH_MOTIONADAPTER" => value.as_int() != Some(0),
                "_MFX_VIEW_SPD_TOP" | "_MFX_VIEW_SPD_BOTTOM" | "_TMP_CRM" |
                "_MFX_INPUT_FORMAT" | "_MFX_OUTPUT_FORMAT" | "_SFX_RENDER" |
                "_SFX_IGNORE_RENDER_SETTING" | "_NEW_SHAPE_REFERENCE_FRAME" |
                "_MFX_GRAPH_ENABLED" => false,
                _ => true,
            })
        {
            return None;
        }
        let Some(Value::List(props)) = speed_props else { return None };
        for (code, expected) in [(14, 0.0), (6, 0.0), (8, 0.0)] {
            let mut matches = props.iter().filter_map(|prop| match prop {
                Value::Node(prop) if int(prop, "code") == Some(code) => Some(prop),
                _ => None,
            });
            let prop = matches.next()?;
            if int(prop, "type") != Some(2) || float(prop, "value") != Some(expected) || matches.next().is_some() {
                return None;
            }
        }
        let aux = curve(ctx, auxiliary?, 1, length, curve_rate, 1)?;
        let [aux_point] = points(aux)? else { return None };
        let aux_point = point(aux_point, 0)?;
        let editable = curve(ctx, editable?, 2, length, curve_rate, 2)?;
        if int(aux, "interp_kind") != Some(2) || !no_properties(aux_point)
            || int(aux_point, "value") != Some(0) || int(editable, "interp_kind") != Some(6)
        {
            return None;
        }
        let trim_end = trim_start.checked_add(trim_length)?;
        if trim_start < 0 || trim_length <= 0 || trim_end > length {
            return None;
        }
        let (rate_num_wide, rate_den_wide) = (i128::from(rate_num), i128::from(rate_den));
        if !double_speed && (i128::from(input_length) <= i128::from(length - 1) * rate_num_wide / rate_den_wide
            || i128::from(*num) * i128::from(input_length) != i128::from(*den) * i128::from(length))
        {
            return None;
        }
        let first = i128::from(trim_start) * rate_num_wide;
        let last = i64::try_from(i128::from(trim_end - 1) * rate_num_wide / rate_den_wide).ok()?;
        let master = ctx.mobs.get(&input_mob)?;
        let last = (last < input_length).then_some(input_start.checked_add(last)?)?;
        if float(master, "edit_rate").and_then(Rate::from_avid) != Some(rate) || last >= int(master, "length")? {
            return None;
        }
        Some(SourceSubclipSampling {
            first: input_start.checked_add(i64::try_from(first / rate_den_wide).ok()?)?,
            last,
            ratio: (rate_num, rate_den),
            phase: i64::try_from(first % rate_den_wide).ok()?,
        })
    })();
    Some(SourceSubclipMotion {
        input_mob,
        input_track: int(input, "track_id").filter(|&track| track > 0)?,
        input_start,
        input_length,
        length,
        advanced,
        span_ratio: (*num, *den),
        speed,
        speed_key_frame: (*key_num, *key_den),
        position: [start, end],
        phase_offset,
        offset_adjust,
        render_mode,
        ignore_render_setting,
        new_source_calculation: *new_source_calculation,
        graph_mode,
        sampled_input_frames,
        sampled_picture_master: None,
        sampled_picture_file: None,
    })
}

/// The three 50-frame constant controls with native picture references (AVB-0010).
/// The stored span ratio and the signed Speed value describe different things.
pub(super) fn constant_speed(
    ctx: &Ctx<'_>, effect: &Node, input: &Node, stored: (i64, i64),
) -> Option<(i64, i64)> {
    const LENGTH: i64 = 50;
    let (num, den, source_length, first_position, end_position) = match stored {
        (2, 1) => (1, 2, 25, 0, 25),
        (50, 99) => (2, 1, 99, 0, 100),
        (-1, 1) => (-1, 1, 50, 49, -1),
        _ => return None,
    };
    if effect.get("effect_id").and_then(Value::as_str) != Some("EFF_ADV_MOTION_CTL")
        || float(effect, "edit_rate") != Some(24.0)
        || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(effect, "length") != Some(LENGTH)
        || int(effect, "mc_mode") != Some(0)
        || int(effect, "num_scalars") != Some(0)
        || int(effect, "phase_offset") != Some(0)
        || float(effect, "offset_adjust") != Some(0.0)
        || effect.get("new_source_calculation") != Some(&Value::Bool(true))
        || !null(effect, "source_param_list")
        || int(input, "length") != Some(source_length)
        || float(input, "edit_rate") != Some(24.0)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
        || !std::ptr::eq(direct_input(ctx, effect)?, input)
        || !master_picture_matches(ctx, input)
    {
        return None;
    }
    let attrs = borrowed_attrs(ctx, effect);
    for (key, expected) in [
        ("_MFX_INPUT_FORMAT", 2), ("_MFX_OUTPUT_FORMAT", 2),
        ("_SFX_RENDER", 0), ("_SFX_IGNORE_RENDER_SETTING", 0),
        ("_NEW_SHAPE_REFERENCE_FRAME", 1),
    ] {
        if unique_attribute(attrs, key).and_then(Value::as_int) != Some(expected) {
            return None;
        }
    }
    match (num, attrs.iter().filter(|(key, _)| key == "_MFX_GRAPH_ENABLED").count()) {
        (-1, 0) => (),
        (1 | 2, 1) if unique_attribute(attrs, "_MFX_GRAPH_ENABLED").and_then(Value::as_int) == Some(1) => (),
        _ => return None,
    }
    if attrs.iter().any(|(k, _)| !matches!(k.as_str(),
        "_MFX_INPUT_FORMAT" | "_MFX_OUTPUT_FORMAT" | "_SFX_RENDER" |
        "_SFX_IGNORE_RENDER_SETTING" | "_NEW_SHAPE_REFERENCE_FRAME" |
        "_MFX_GRAPH_ENABLED" | "_MFX_VIEW_SPD_TOP" | "_MFX_VIEW_SPD_BOTTOM" |
        "_TMP_CRM"))
    {
        return None;
    }
    let list = ctx.deref(effect.get("param_list")).filter(|n| n.class == *b"PRLS")?;
    let Value::RefList(items) = list.get("items")? else { return None };
    if items.len() != if num < 0 { 3 } else { 4 } {
        return None;
    }
    let mut controls = [None; 4];
    for reference in items {
        let item = ctx.deref(Some(reference))?;
        let (slot, ty) = match uuid(item)?.as_str() {
            SPEED => (0, 2),
            POSITION => (1, 2),
            AUXILIARY => (2, 1),
            EDITABLE_POSITION if num > 0 => (3, 2),
            _ => return None,
        };
        let extrap = if slot == 3 { 2 } else { 1 };
        if controls[slot].replace(curve(ctx, item, ty, LENGTH, 24.0, extrap)?).is_some() {
            return None;
        }
    }
    if let Some(editable) = controls[3] {
        let keys = points(editable)?;
        let edge_x = 4.800000000000001;
        let expected = [
            (0, 0.0, [-edge_x, -2.45], [edge_x, 2.45]),
            (12, 6.125, [-edge_x, -1.5891891891891894], [10.0, 3.310810810810811]),
            (37, 42.875, [-10.0, -3.310810810810811], [edge_x, 1.5891891891891894]),
            (49, 49.0, [-edge_x, -2.45], [edge_x, 2.45]),
        ];
        if int(editable, "interp_kind") != Some(6) || keys.len() != 4
            || !expected.into_iter().zip(keys).all(|((frame, value, incoming, outgoing), key)| {
                    preset_key(key, frame).is_some_and(|k| {
                        k.value == value && k.incoming == incoming && k.outgoing == outgoing
                    })
                })
        {
            return None;
        }
    }
    let speed = controls[0]?;
    let [only_speed] = points(speed)? else { return None };
    let only_speed = point(only_speed, 0)?;
    let position = controls[1]?;
    let [first, end] = points(position)? else { return None };
    let auxiliary = controls[2]?;
    let [aux] = points(auxiliary)? else { return None };
    let aux = point(aux, 0)?;
    let Value::List(props) = only_speed.get("pp")? else { return None };
    let has_speed_properties = [(14, first_position as f64), (5, -5.0), (6, 0.0),
        (7, 5.0), (8, 0.0)].into_iter().all(|(code, expected)| {
        props.iter().any(|p| matches!(p, Value::Node(n) if int(n, "code") == Some(code)
            && int(n, "type") == Some(2) && float(n, "value") == Some(expected)))
    });
    (int(speed, "interp_kind") == Some(6)
        && float(only_speed, "value") == Some(num as f64 / den as f64)
        && props.len() == 6 && has_speed_properties
        && props.iter().any(|p| matches!(p, Value::Node(n) if int(n, "code") == Some(9)
            && int(n, "type") == Some(1) && int(n, "value") == Some(1)))
        && int(position, "interp_kind") == Some(3)
        && [(first, 0, first_position), (end, 2 * LENGTH, end_position)]
            .into_iter().all(|(p, t, value)| point(p, t)
                .is_some_and(|p| no_properties(p) && float(p, "value") == Some(value as f64)))
        && int(auxiliary, "interp_kind") == Some(2)
        && no_properties(aux) && int(aux, "value") == Some(0))
    .then_some((num, den))
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

/// A different duration can reuse the saved forward map when the explicit
/// progressive render settings agree with the calibrated two-key profile.
/// Keep inherited settings and offset/phase edits outside this route.
pub(super) fn visible_linear_ramp(
    ctx: &Ctx<'_>, effect: &Node, input: &Node, ratio: (i64, i64),
    budget: &SharedBudget,
) -> Option<Vec<i64>> {
    let length = int(effect, "length")?;
    let source_length = int(input, "length")?;
    if length == FRAMES || !(3..=MAX_SAVED_PICTURE_FRAMES).contains(&length)
        || ratio.0 <= 0 || ratio.1 <= 0 || source_length <= 0
        || i128::from(length) * i128::from(ratio.1) != i128::from(source_length) * i128::from(ratio.0)
        || float(effect, "edit_rate") != Some(24.0)
        || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(effect, "mc_mode") != Some(0) || int(effect, "num_scalars") != Some(0)
        || int(effect, "phase_offset") != Some(0) || float(effect, "offset_adjust") != Some(0.0)
        || effect.get("new_source_calculation") != Some(&Value::Bool(true))
        || !null(effect, "source_param_list") || !null(effect, "precomputed")
        || float(input, "edit_rate") != Some(24.0)
        || int(input, "media_kind_id") != Some(KIND_PICTURE)
    {
        return None;
    }
    saved_settings(ctx, effect)?;
    let Value::RefList(items) = ctx.deref(effect.get("param_list"))?.get("items")? else {
        return None;
    };
    if items.len() != 3 { return None; }
    let mut controls = [None; 3];
    for entry in items {
        let item = ctx.deref(Some(entry))?;
        let (index, value_type, interpolation) = match uuid(item)?.as_str() {
            SPEED => (0, 2, 3),
            POSITION => (1, 2, 3),
            AUXILIARY => (2, 1, 2),
            _ => return None,
        };
        let track = curve(ctx, item, value_type, length, 24.0, 1)?;
        if int(track, "interp_kind") != Some(interpolation)
            || controls[index].replace(points(track)?).is_some()
        {
            return None;
        }
    }
    let [start, end] = controls[0]? else { return None; };
    let speed = |raw, frame, first| {
        let node = point(raw, frame * 2)?;
        let Value::List(properties) = node.get("pp")? else { return None; };
        let mut seen = HashSet::new();
        for p in properties {
            let Value::Node(p) = p else { return None; };
            let code = int(p, "code")?;
            if !seen.insert(code) { return None; }
            let known = match code {
                5 | 7 => int(p, "type") == Some(2)
                    && float(p, "value").is_some_and(|v| v.is_finite() &&
                        v.abs() <= length as f64 && if code == 5 { v < 0.0 } else { v > 0.0 }),
                6 | 8 => int(p, "type") == Some(2) && float(p, "value") == Some(0.0),
                9 => int(p, "type") == Some(1) && int(p, "value") == Some(1),
                14 if first => int(p, "type") == Some(2) && float(p, "value") == Some(0.0),
                _ => false,
            };
            if !known { return None; }
        }
        if seen.len() != if first { 6 } else { 5 } { return None; }
        float(node, "value").filter(|v| (0.5..=2.0).contains(v))
    };
    let first_speed = speed(start, 0, true)?;
    let last_speed = speed(end, length - 1, false)?;
    if first_speed == last_speed { return None; }
    let [aux] = controls[2]? else { return None; };
    let aux = point(aux, 0)?;
    if !no_properties(aux) || int(aux, "value") != Some(0) { return None; }
    let samples = controls[1]?;
    if samples.len() != (2 * length + 1) as usize { return None; }
    let mut frames = Vec::new();
    if !reserve(budget, &mut frames, length as usize) { return None; }
    let (mut previous, mut previous_step) = (0.0, first_speed / 2.0);
    let direction = (last_speed - first_speed).signum();
    for (index, raw) in samples.iter().enumerate() {
        let node = point(raw, index as i64)?;
        let value = float(node, "value")?;
        if !no_properties(node) || !value.is_finite() || (index == 0 && value != 0.0) {
            return None;
        }
        if index > 0 {
            let step = value - previous;
            if !(0.25 - FLOAT_NOISE..=1.0 + FLOAT_NOISE).contains(&step)
                || direction * (step - previous_step) < -FLOAT_NOISE
                || (index >= samples.len() - 2 && (step - last_speed / 2.0).abs() > FLOAT_NOISE)
            {
                return None;
            }
            previous_step = step;
        }
        if index % 2 == 0 && index / 2 < length as usize {
            if index > 0 && (value - value.round()).abs() <= FLOAT_NOISE { return None; }
            let frame = value.floor() as i64;
            if !(0..source_length).contains(&frame) { return None; }
            frames.push(frame);
        }
        previous = value;
    }
    (frames.last() == Some(&(source_length - 1))).then_some(frames)
}
