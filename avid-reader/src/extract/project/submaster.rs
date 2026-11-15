//! Validate the plain, picture-only Submaster made by Collapse.
use super::*;
use crate::source::{Rate, SubmasterPictureInput};

pub(super) const EFFECT: &str = "EFF_SUBMASTER";

/// Retain a saved source reference and its track-local range. The reference
/// can point to another composition and has not been resolved to a file.
pub(super) fn picture_source(
    ctx: &Ctx<'_>,
    effect: &Node,
    component: &Node,
) -> Option<SubmasterPictureInput> {
    let rate = Rate::from_avid(float(effect, "edit_rate")?)?;
    let same_picture = |node: &Node| {
        int(node, "media_kind_id") == Some(KIND_PICTURE)
            && float(node, "edit_rate").and_then(Rate::from_avid) == Some(rate)
    };
    if !same_picture(component) {
        return None;
    }
    let mut position = 0i64;
    let mut clip = None;
    let mut take = |segment: &Node| -> Option<()> {
        let length = int(segment, "length").filter(|n| *n >= 0)?;
        if !same_picture(segment) {
            return None;
        }
        match &segment.class {
            b"FILL" => {}
            b"SCLP" if clip.is_none() && length > 0 => {
                let mob = mob_id(segment)?;
                if mob.material.data1 == 0 && mob.label == [0; 12] {
                    return None;
                }
                let track = int(segment, "track_id").filter(|t| *t > 0)?;
                let source_start = int(segment, "start_time").filter(|n| *n >= 0)?;
                source_start.checked_add(length)?;
                clip = Some(SubmasterPictureInput {
                    mob,
                    track,
                    source_start,
                    local_start: position,
                    length,
                    local_length: 0,
                    rate,
                });
            }
            _ => return None,
        }
        position = position.checked_add(length)?;
        Some(())
    };
    match &component.class {
        b"SCLP" => take(component)?,
        b"SEQU" => {
            let Value::RefList(segments) = component.get("components")? else { return None };
            if segments.len() > 256 {
                return None;
            }
            for reference in segments {
                take(ctx.deref(Some(reference))?)?;
            }
            if int(component, "length").is_some_and(|length| length != position) {
                return None;
            }
        }
        _ => return None,
    }
    if int(effect, "length") != Some(position) {
        return None;
    }
    let mut clip = clip?;
    clip.local_length = position;
    Some(clip)
}

pub(super) fn neutral_controls(ctx: &Ctx<'_>, effect: &Node, length: i64) -> bool {
    if length <= 0 || int(effect, "media_kind_id") != Some(KIND_PICTURE)
        || int(effect, "mc_mode") != Some(0) || int(effect, "num_scalars") != Some(0)
        || int(effect, "info_smooth") != Some(0) || int(effect, "info_is_reversed") != Some(0)
        || ["left_bob", "right_bob", "param_list", "precomputed", "trackman"].iter()
            .any(|name| !matches!(effect.get(name), None | Some(Value::Null | Value::Ref(0))))
    {
        return false;
    }
    let Some(attrs) = ctx.deref(effect.get("attributes")) else { return false };
    let Some(Value::Attrs(attrs)) = attrs.get("entries") else { return false };
    if attrs.len() > 12 || attrs.iter().enumerate().any(|(i, (name, value))|
        attrs[..i].iter().any(|(other, _)| other == name) || match name.as_str() {
        "_EFFECT_PLUGIN_TYPE" => value.as_str() != Some("Unknown"),
        "_EFFECT_PLUGIN_NAME" => value.as_str() != Some("Submaster"),
        "_EFFECT_PLUGIN_CLASS" => value.as_str() != Some("Image"),
        "_EFFECT_PLUGIN_MANUFACTURER_ID" | "_EFFECT_PLUGIN_MANUFACTURER_NAME"
        | "_EFFECT_PLUGIN_PRODUCT_ID" | "_EFFECT_PLUGIN_PRODUCT_NAME"
        | "_EFFECT_PLUGIN_VERSION" | "_EFFECT_PLUGIN_SKU" => value.as_str() != Some(""),
        "_BORDER_FIXED" | "_IGNORE_TRACKING" => value.as_int() != Some(0),
        // Markers are captured separately. A recognized marker carrier does
        // not add a picture control to the otherwise neutral Submaster.
        "_TMP_CRM" => !ctx.deref(Some(value)).is_some_and(|carrier| match &carrier.class {
            b"TMBC" => true,
            b"TMCS" => matches!(carrier.get("items"), Some(Value::RefList(items))
                if items.iter().all(|item|
                    ctx.deref(Some(item)).is_some_and(|marker| marker.class == *b"TMBC"))),
            _ => false,
        }),
        _ => true,
    }) {
        return false;
    }
    let Some(keys) = ctx.deref(effect.get("keyframes")) else { return false };
    let Some(Value::List(keys)) = keys.get("parameters").filter(|_| {
        int(keys, "orig_length") == Some(-1) && int(keys, "window_offset") == Some(0)
            && int(keys, "keyframe_size") == Some(168)
    }) else { return false };
    if keys.len() != 2 || keys.iter().zip([0, 1 << 30]).any(|(value, at)| {
        let Value::Node(key) = value else { return true };
        int(key, "percent_time") != Some(at)
            || [("level", 1 << 29), ("scale_x", 65536), ("scale_y", 65536),
                ("pos_x", 0), ("pos_y", 0), ("floor_x", 0), ("ceil_x", 1000),
                ("floor_y", 0), ("ceil_y", 1000), ("crop_left", 0), ("crop_right", 0),
                ("crop_top", 0), ("crop_bottom", 0), ("border_width", 0), ("border_soft", 0),
                ("splill_gain2", 0), ("splill_gain", 0), ("splill_soft2", 0), ("splill_soft", 0),
                ("enable_key_flags", 0)].iter().any(|&(field, n)| int(key, field) != Some(n))
            || !matches!(key.get("box"), Some(Value::List(values)) if values.as_slice()
                == [Value::Int(-32_768_000), Value::Int(32_768_000), Value::Int(-32_768_000), Value::Int(32_768_000)])
            || ["box_xscale", "box_yscale", "box_xpos", "box_ypos"].iter()
                .any(|field| key.get(field) != Some(&Value::Bool(false)))
            || !matches!(key.get("colors"), Some(Value::List(values)) if values.is_empty())
            || !matches!(key.get("user_param"), Some(Value::Bytes(bytes)) if bytes.is_empty())
    }) {
        return false;
    }
    true
}

pub(super) fn plain_input(ctx: &Ctx<'_>, effect: &Node, length: i64) -> bool {
    if !neutral_controls(ctx, effect, length) {
        return false;
    }
    let Some(Value::List(tracks)) = effect.get("tracks") else { return false };
    let [Value::Node(track)] = tracks.as_slice() else { return false };
    let Some(sequence) = ctx.deref(track.get("component")) else { return false };
    let rate = |node| float(node, "edit_rate").and_then(crate::source::Rate::from_avid);
    let same_picture = |node| int(node, "media_kind_id") == Some(KIND_PICTURE)
        && rate(node).is_some() && rate(node) == rate(effect);
    if int(track, "index") != Some(1) || sequence.class != *b"SEQU" || !same_picture(sequence)
        || ["effect_id", "left_bob", "right_bob", "attributes", "session_attrs", "precomputed", "param_list"]
            .iter().any(|name| !matches!(sequence.get(name), None | Some(Value::Null | Value::Ref(0))))
    {
        return false;
    }
    let Some(Value::RefList(components)) = sequence.get("components") else { return false };
    let [lead, clip, tail] = components.as_slice() else { return false };
    let empty = |part| ctx.deref(Some(part)).is_some_and(|node| {
        node.class == *b"FILL" && int(node, "length") == Some(0) && same_picture(node)
    });
    empty(lead) && empty(tail) && ctx.deref(Some(clip)).is_some_and(|node| {
        node.class == *b"SCLP" && same_picture(node) && int(node, "length") == Some(length)
            && int(node, "track_id") == Some(1) && int(node, "start_time").is_some_and(|at| at >= 0)
    })
}
