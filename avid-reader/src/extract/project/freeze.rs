//! The retained progressive 24 fps traditional Freeze Frame and its generated render.
use super::*;
use std::collections::HashSet;

const FRAMES: i64 = 120;

#[derive(Clone, Copy)]
pub(super) struct Freeze {
    pub(super) owner: MobId,
    input: MobId,
    selected: i64,
}

fn attr<'a>(ctx: &Ctx<'a>, node: &Node, name: &str) -> Option<&'a Value> {
    let Value::Attrs(attrs) = ctx.deref(node.get("attributes"))?.get("entries")? else {
        return None;
    };
    let mut found = attrs.iter().filter(|(key, _)| key == name);
    let (_, value) = found.next()?;
    found.next().is_none().then_some(value)
}

fn picture<'a>(ctx: &Ctx<'a>, node: &'a Node) -> Option<&'a Node> {
    let Value::List(tracks) = node.get("tracks")? else {
        return None;
    };
    let [Value::Node(track)] = tracks.as_slice() else {
        return None;
    };
    let component = ctx.deref(track.get("component"))?;
    (int(track, "index") == Some(1) && int(component, "media_kind_id") == Some(KIND_PICTURE))
        .then_some(component)
}

fn span(node: &Node, length: i64) -> bool {
    int(node, "length") == Some(length) && float(node, "edit_rate") == Some(24.0)
}

fn source_clip(node: &Node, length: i64, start: i64) -> Option<MobId> {
    if node.class != *b"SCLP"
        || !span(node, length)
        || int(node, "track_id") != Some(1)
        || int(node, "media_kind_id") != Some(KIND_PICTURE)
        || int(node, "start_time") != Some(start)
        || !matches!(node.get("precomputed"), None | Some(Value::Null))
        || !matches!(node.get("param_list"), None | Some(Value::Null))
    {
        return None;
    }
    mob_id(node)
}

fn match_ref<'a>(ctx: &Ctx<'a>, effect: &Node) -> Option<&'a Node> {
    ctx.deref(attr(ctx, effect, "_MATCH"))
        .filter(|n| n.class == *b"MCMR")
}

pub(super) fn candidate(ctx: &Ctx<'_>, effect: &Node, ratio: (i64, i64)) -> bool {
    if effect.get("effect_id").and_then(Value::as_str) != Some("EFF_ADV_MOTION_CTL") {
        return false;
    }
    // A recognized freeze whose cache or curve has changed must not silently
    // become an ordinary slow motion edit.
    (ratio.0 > 1
        && ratio.1 == 1
        && int(effect, "length") == Some(ratio.0)
        && ctx.deref(effect.get("precomputed")).is_some()
        && source_capture::timewarp_input(ctx, effect)
            .is_some_and(|input| int(input, "length") == Some(1)))
        || match_ref(ctx, effect)
            .and_then(mob_id)
            .and_then(|id| ctx.mobs.get(&id))
            .is_some_and(|m| {
                int(m, "usage_code") == Some(6)
                    && picture(ctx, m)
                        .and_then(|e| source_capture::timewarp_input(ctx, e))
                        .is_some_and(|input| int(input, "length") == Some(1))
            })
}

fn controls(ctx: &Ctx<'_>, effect: &Node) -> Option<()> {
    let Value::RefList(items) = ctx.deref(effect.get("param_list"))?.get("items")? else {
        return None;
    };
    if items.len() != 3 {
        return None;
    }
    let mut seen = HashSet::new();
    for reference in items {
        let item = ctx.deref(Some(reference))?;
        let Value::Uuid(id) = item.get("uuid")? else {
            return None;
        };
        let id = id.to_string();
        let (value_type, mode, offsets): (i64, i64, &[i64]) = match id.as_str() {
            "8d56827c-847e-11d5-935a-50f857c10000" => (2, 6, &[0]),
            "8d56827d-847e-11d5-935a-50f857c10000" => (2, 3, &[0, FRAMES]),
            "8d568283-847e-11d5-935a-50f857c10000" => (1, 2, &[0]),
            _ => return None,
        };
        let zero = |n: &Node| {
            if value_type == 2 {
                float(n, "value") == Some(0.0)
            } else {
                int(n, "value") == Some(0)
            }
        };
        let curve = ctx.deref(item.get("control_track"))?;
        let Value::List(points) = curve.get("control_points")? else {
            return None;
        };
        if !seen.insert(id)
            || item.class != *b"PRIT"
            || !zero(item)
            || item.get("enable") != Some(&Value::Bool(true))
            || item.get("contribs_to_sig") != Some(&Value::Bool(true))
            || int(item, "value_type") != Some(value_type)
            || curve.class != *b"PRCL"
            || !span(curve, FRAMES)
            || int(curve, "interp_kind") != Some(mode)
            || int(curve, "value_type") != Some(value_type)
            || int(curve, "extrap_kind") != Some(1)
            || int(curve, "fields") != Some(1)
            || points.len() != offsets.len()
        {
            return None;
        }
        for (point, offset) in points.iter().zip(offsets) {
            let Value::Node(point) = point else {
                return None;
            };
            if !zero(point)
                || int(point, "timescale") != Some(1)
                || point.get("offset")
                    != Some(&Value::List(vec![Value::Int(*offset), Value::Int(1)]))
            {
                return None;
            }
        }
    }
    Some(())
}

fn selected(ctx: &Ctx<'_>, effect: &Node) -> Option<(MobId, i64)> {
    if effect.class != *b"SPED"
        || !span(effect, FRAMES)
        || effect.get("effect_id").and_then(Value::as_str) != Some("EFF_ADV_MOTION_CTL")
        || effect.get("speed_ratio") != Some(&Value::List(vec![Value::Int(FRAMES), Value::Int(1)]))
        || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(effect, "phase_offset") != Some(0)
        || float(effect, "offset_adjust") != Some(0.0)
        || effect.get("new_source_calculation") != Some(&Value::Bool(true))
        || !matches!(effect.get("source_param_list"), None | Some(Value::Null))
    {
        return None;
    }
    for (name, expected) in [
        ("_SFX_RENDER", 1),
        ("_SFX_IGNORE_RENDER_SETTING", 1),
        ("_MIXMATCH_MOTIONADAPTER", 3),
        ("_MFX_INPUT_FORMAT", 2),
        ("_MFX_OUTPUT_FORMAT", 2),
        ("_NEW_SHAPE_REFERENCE_FRAME", 1),
    ] {
        if attr(ctx, effect, name)?.as_int() != Some(expected) {
            return None;
        }
    }
    controls(ctx, effect)?;
    let clip = source_capture::timewarp_input(ctx, effect)?;
    let start = int(clip, "start_time")?;
    (start >= 0).then_some((source_clip(clip, 1, start)?, start))
}

pub(super) fn binding(
    ctx: &Ctx<'_>,
    owner: &Node,
    ambiguous: &HashSet<MobId>,
) -> Option<(MobId, Freeze)> {
    if int(owner, "mob_type_id") != Some(MOB_COMPOSITION)
        || int(owner, "usage_code") != Some(6)
        || !span(owner, FRAMES)
    {
        return None;
    }
    let owner = mob_id(owner)?;
    let effect = picture(ctx, ctx.mobs.get(&owner)?)?;
    let (input, selected_frame) = selected(ctx, effect)?;
    let rendered_id = source_clip(ctx.deref(effect.get("precomputed"))?, FRAMES, 0)?;
    let rendered = ctx.mobs.get(&rendered_id)?;
    if int(rendered, "mob_type_id") != Some(MOB_MASTER)
        || int(rendered, "usage_code") != Some(1)
        || !span(rendered, FRAMES)
    {
        return None;
    }
    let file_id = source_clip(picture(ctx, rendered)?, FRAMES, 0)?;
    let file = ctx.mobs.get(&file_id)?;
    if int(file, "mob_type_id") != Some(MOB_SOURCE)
        || int(file, "usage_code") != Some(9)
        || !span(file, FRAMES)
    {
        return None;
    }
    let descriptor = ctx.deref(file.get("descriptor"))?;
    if descriptor.class != *b"CDCI"
        || !span(descriptor, FRAMES)
        || int(descriptor, "frame_layout") != Some(0)
        || int(descriptor, "component_width") != Some(10)
        || int(descriptor, "stored_width") != Some(1920)
        || int(descriptor, "stored_height") != Some(1080)
        || !matches!(descriptor.get("aspect_ratio"), Some(Value::List(parts))
            if parts == &[Value::Int(16), Value::Int(9)])
        || !matches!(descriptor.get("compress_method"), Some(Value::Bytes(bytes)) if bytes == b"RI00")
    {
        return None;
    }
    let upstream = source_clip(picture(ctx, file)?, FRAMES, 0)?;
    let origin = ctx.mobs.get(&upstream)?;
    let ids = [owner, rendered_id, file_id, upstream, input];
    if ids
        .iter()
        .enumerate()
        .any(|(at, id)| ambiguous.contains(id) || ids[..at].contains(id))
        || int(origin, "mob_type_id") != Some(MOB_SOURCE)
        || int(origin, "usage_code") != Some(14)
        || !span(origin, FRAMES)
    {
        return None;
    }
    let origin_effect = picture(ctx, origin)?;
    if origin_effect.class != *b"SPED"
        || !span(origin_effect, FRAMES)
        || origin_effect.get("speed_ratio") != effect.get("speed_ratio")
        || origin_effect.get("effect_id") != effect.get("effect_id")
        || !matches!(origin_effect.get("precomputed"), None | Some(Value::Null))
    {
        return None;
    }
    Some((
        rendered_id,
        Freeze {
            owner,
            input,
            selected: selected_frame,
        },
    ))
}

pub(super) fn sequence_render<'a>(
    ctx: &Ctx<'a>,
    effect: &Node,
    bindings: &HashMap<MobId, Freeze>,
) -> Option<(&'a Node, MobId)> {
    let link = match_ref(ctx, effect)?;
    if int(link, "position") != Some(0) {
        return None;
    }
    let precomputed = ctx.deref(effect.get("precomputed"))?;
    let id = source_clip(precomputed, FRAMES, 0)?;
    let binding = bindings.get(&id)?;
    if mob_id(link) != Some(binding.owner)
        || selected(ctx, effect)? != (binding.input, binding.selected)
    {
        return None;
    }
    Some((precomputed, id))
}
