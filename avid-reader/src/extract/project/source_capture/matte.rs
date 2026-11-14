//! Saved-control witnesses for AVB-0006's imported straight-alpha stills.
//! Matching these signatures does not prove the physical PNG or its raster,
//! the lower track's coverage, or how another editor will composite the result.
use super::*;
use crate::color::file_order_hex;

const SHARED_CONTROL: &str = "dca5f78a7b361949afc75ca00a5bd558";
const ADAPTER_PAYLOAD: &str = "d85c7fbdfd154e42a34d11642fbbb867";

pub(super) fn imported_png(
    ctx: &Ctx<'_>,
    effect: &Node,
    inputs: &MatteInputs,
    import: &MatteImport,
) -> bool {
    let (Some(fill), Some(matte)) = (inputs.fill, inputs.matte) else {
        return false;
    };
    if inputs.background != MatteBackground::LowerTrack
        || (fill.track, matte.track) != (1, 2)
        || [fill, matte]
            .iter()
            .any(|i| i.start != 0 || i.length != 1 || Some(i.rate) != Rate::new(24, 1))
        || import.ignore_alpha != Some(false)
        || import.invert_alpha.is_none()
        || import.locators.is_empty()
        || import.locators.iter().any(|p| {
            !std::path::Path::new(p)
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("png"))
        })
    {
        return false;
    }
    let Some(settings) = ctx
        .mobs
        .get(&fill.mob)
        .and_then(|m| import_settings(ctx, m))
    else {
        return false;
    };
    if ![
        ("_IMPORTPICTSEQ", 0),
        ("_IMPORTASPECT", 3),
        ("_COLORLEVEL", 1),
    ]
    .iter()
    .all(|(k, v)| unique_attribute(settings, k).and_then(Value::as_int) == Some(*v))
    {
        return false;
    }
    let track_leaf = |id| matte_track(ctx, effect, id).and_then(|n| picture_leaf(ctx, n));
    let Some(background) = track_leaf(1) else {
        return false;
    };
    let Some(foreground) = track_leaf(2) else {
        return false;
    };
    let Some(alpha) = track_leaf(3) else {
        return false;
    };
    if background.class != *b"FILL"
        || int(background, "length") != Some(1)
        || !saved_image_bounds(ctx, alpha)
        || !(saved_image_bounds(ctx, foreground) || empty_color_adapter(ctx, foreground).is_some())
        || !effect_controls(ctx, effect, "EFF2_2DMATTE", 0, 360, 1 << 30)
    {
        return false;
    }
    let attrs = borrowed_attrs(ctx, effect);
    attrs.len() == 4
        && unique_attribute(attrs, "_USE_DSK").and_then(Value::as_int) == Some(1)
        && ctx
            .deref(unique_attribute(attrs, "_MATCH"))
            .is_some_and(|n| {
                n.class == *b"MCMR" && int(n, "position") == Some(0) && mob_id(n).is_some()
            })
        && parameters(
            ctx,
            effect,
            &[
                ("1133b7131c163140bddab3df672fc6de", 1, Some(1), true),
                ("2f9c977d4fe93941b154638729b884db", 1, Some(0), true),
                (SHARED_CONTROL, 1, Some(1), false),
            ],
        )
}

/// Match the observed empty payload, not every adapter with this effect ID.
pub(super) fn empty_color_adapter<'a>(ctx: &Ctx<'a>, effect: &'a Node) -> Option<&'a Node> {
    if !effect_controls(ctx, effect, "EFF2_LUTSFX", 15, 15, 1 << 29)
        || borrowed_attrs(ctx, effect).len() != 2
        || !parameters(
            ctx,
            effect,
            &[
                (SHARED_CONTROL, 1, Some(1), false),
                (ADAPTER_PAYLOAD, 4, None, true),
            ],
        )
    {
        return None;
    }
    let Value::List(tracks) = effect.get("tracks")? else {
        return None;
    };
    let [Value::Node(_)] = tracks.as_slice() else {
        return None;
    };
    let leaf = picture_leaf(ctx, matte_track(ctx, effect, 1)?)?;
    (leaf.class == *b"SCLP"
        && int(leaf, "length") == int(effect, "length")
        && float(leaf, "edit_rate") == Some(24.0)
        && saved_image_bounds(ctx, leaf))
    .then_some(leaf)
}

fn saved_image_bounds(ctx: &Ctx<'_>, clip: &Node) -> bool {
    if clip.class != *b"SCLP" || !null_fields(clip, &["param_list", "effect_id"]) {
        return false;
    }
    let attrs = borrowed_attrs(ctx, clip);
    let Some(xml) = unique_attribute(attrs, "_IMAGE_BOUNDS_OVERRIDE").and_then(Value::as_str)
    else {
        return false;
    };
    // The validated render used these equal framing, valid, essence and
    // source rectangles. Keep changed framing out of this profile.
    const RECTANGLES: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?><Bounds>",
        "  <Framing>-800/1 -450/1 1600/1 900/1</Framing>",
        "  <Valid>-800/1 -450/1 1600/1 900/1</Valid>",
        "  <Essence>-800/1 -450/1 1600/1 900/1</Essence>",
        "  <Source>-800/1 -450/1 1600/1 900/1</Source></Bounds>",
    );
    xml == RECTANGLES
        && attrs.iter().all(|(name, value)| match name.as_str() {
            "_IMAGE_BOUNDS_OVERRIDE" | "_TMP_CRM" => true,
            "_BOUNDS_REFORMATTING_OPTION" => {
                value.as_int() == Some(1) && unique_attribute(attrs, name).is_some()
            }
            _ => false,
        })
}

fn picture_leaf<'a>(ctx: &Ctx<'a>, node: &'a Node) -> Option<&'a Node> {
    let plain = |n: &Node| {
        int(n, "media_kind_id") == Some(KIND_PICTURE)
            && float(n, "edit_rate") == Some(24.0)
            && null_fields(
                n,
                &["precomputed", "session_attrs", "left_bob", "right_bob"],
            )
    };
    if !plain(node) {
        return None;
    }
    if node.class != *b"SEQU" {
        return Some(node);
    }
    if !null_fields(node, &["param_list", "effect_id", "attributes"]) {
        return None;
    }
    sole_picture_segment(ctx, node).filter(|leaf| plain(leaf))
}

fn null_fields(node: &Node, fields: &[&str]) -> bool {
    fields.iter().all(|k| node.get(k) == Some(&Value::Null))
}

fn effect_controls(
    ctx: &Ctx<'_>,
    effect: &Node,
    id: &str,
    left: i64,
    right: i64,
    level: i64,
) -> bool {
    let attrs = borrowed_attrs(ctx, effect);
    effect.class == *b"TKFX"
        && effect.get("effect_id").and_then(Value::as_str) == Some(id)
        && int(effect, "media_kind_id") == Some(KIND_PICTURE)
        && float(effect, "edit_rate") == Some(24.0)
        && int(effect, "length") == Some(1)
        && null_fields(
            effect,
            &[
                "left_bob",
                "right_bob",
                "precomputed",
                "session_attrs",
                "trackman",
            ],
        )
        && [
            ("mc_mode", 0),
            ("num_scalars", 0),
            ("info_version", 2),
            ("info_current", 0),
            ("info_smooth", 0),
            ("info_color_item", 1),
            ("info_quality", 1),
            ("info_is_reversed", 0),
            ("left_length", left),
            ("right_length", right),
        ]
        .iter()
        .all(|(k, v)| int(effect, k) == Some(*v))
        && [
            ("info_aspect_on", true),
            ("info_force_software", false),
            ("info_never_hardware", false),
        ]
        .iter()
        .all(|(k, v)| effect.get(k) == Some(&Value::Bool(*v)))
        && ["_BORDER_FIXED", "_IGNORE_TRACKING"]
            .iter()
            .all(|k| unique_attribute(attrs, k).and_then(Value::as_int) == Some(0))
        && ctx
            .deref(effect.get("keyframes"))
            .is_some_and(|fx| neutral_keys(fx, level))
}

fn neutral_keys(fx: &Node, level: i64) -> bool {
    if fx.class != *b"FXPS"
        || int(fx, "orig_length") != Some(-1)
        || int(fx, "window_offset") != Some(0)
        || int(fx, "keyframe_size") != Some(168)
    {
        return false;
    }
    let Some(Value::List(keys)) = fx.get("parameters") else {
        return false;
    };
    let [Value::Node(first), Value::Node(last)] = keys.as_slice() else {
        return false;
    };
    [(first, 0), (last, 1 << 30)].iter().all(|(k, time)| {
        [("percent_time", *time), ("level", level), ("pos_x", 0), ("floor_x", 0),
            ("ceil_x", 1000), ("pos_y", 0), ("floor_y", 0), ("ceil_y", 1000),
            ("scale_x", 65536), ("scale_y", 65536), ("crop_left", 0), ("crop_right", 0),
            ("crop_top", 0), ("crop_bottom", 0), ("border_width", 0), ("border_soft", 0),
            ("splill_gain2", 0), ("splill_gain", 0), ("splill_soft2", 0), ("splill_soft", 0),
            ("enable_key_flags", 0)].iter().all(|(field, value)| int(k, field) == Some(*value))
            && ["box_xscale", "box_yscale", "box_xpos", "box_ypos"]
                .iter().all(|field| k.get(field) == Some(&Value::Bool(false)))
            && matches!(k.get("colors"), Some(Value::List(c)) if c.is_empty())
            && matches!(k.get("user_param"), Some(Value::Bytes(c)) if c.is_empty())
            && matches!(k.get("box"), Some(Value::List(b)) if b.as_slice() == [
                Value::Int(-32768000), Value::Int(32768000), Value::Int(-32768000), Value::Int(32768000),
            ])
    })
}

fn parameters(ctx: &Ctx<'_>, effect: &Node, expected: &[(&str, i64, Option<i64>, bool)]) -> bool {
    let Some(list) = ctx
        .deref(effect.get("param_list"))
        .filter(|n| n.class == *b"PRLS")
    else {
        return false;
    };
    let Some(Value::RefList(items)) = list.get("items") else {
        return false;
    };
    if items.len() != expected.len() {
        return false;
    }
    expected.iter().all(|(id, ty, value, signature)| {
        let mut matched = items.iter().filter_map(|r| ctx.deref(Some(r))).filter(|n| {
            n.class == *b"PRIT"
                && matches!(n.get("uuid"), Some(Value::Uuid(u)) if file_order_hex(u) == *id)
        });
        let Some(item) = matched.next() else { return false };
        matched.next().is_none()
            && int(item, "value_type") == Some(*ty)
            && item.get("name").and_then(Value::as_str) == Some("")
            && item.get("enable") == Some(&Value::Bool(true))
            && item.get("control_track") == Some(&Value::Null)
            && item.get("contribs_to_sig") == Some(&Value::Bool(*signature))
            && match value {
                Some(v) => int(item, "value") == Some(*v),
                None => ctx.deref(item.get("value")).is_some_and(|o| {
                    o.class == *b"AVUP" && int(o, "byte_order") == Some(18761)
                        && matches!(o.get("uuid"), Some(Value::Uuid(u))
                            if file_order_hex(u) == "cc999a218b2c244286fec05794055e1d")
                        && matches!(o.get("data"), Some(Value::Bytes(bytes)) if bytes.as_slice() == [0])
                }),
            }
    })
}
