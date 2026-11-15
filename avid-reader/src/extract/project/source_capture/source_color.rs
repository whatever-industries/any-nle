use super::*;
use crate::color::file_order_hex;

pub(super) fn capture_adapter(
    ctx: &Ctx<'_>,
    effect: &Node,
    length: i64,
    ambiguous: &HashSet<MobId>,
    budget: &SharedBudget,
) -> Option<SourceColorAdapter> {
    if effect.class != *b"TKFX"
        || int(effect, "length") != Some(length)
        || float(effect, "edit_rate") != Some(24.0)
    {
        return None;
    }
    let transform = saved_payload(ctx, effect)?;
    let Value::List(tracks) = effect.get("tracks")? else { return None };
    let [Value::Node(_)] = tracks.as_slice() else { return None };
    let input = sole_picture_input(ctx, matte_track(ctx, effect, 1)?)?;
    if int(input, "length") != Some(length) || float(input, "edit_rate") != Some(24.0)
    {
        return None;
    }
    let master_id = mob_id(input)?;
    let master_track = int(input, "track_id").filter(|track| *track > 0)?;
    let source_in = int(input, "start_time")?;
    let picture_file = match direct_picture_file(ctx, master_id, master_track, ambiguous) {
        Some((id, file)) if input_transform_text(ctx, file) == Some(Ok(transform)) => Observed::Known(id),
        Some(_) => Observed::Unresolved(copy_text(
            budget, "edit-side color transformation does not match the saved picture source",
        )),
        None => Observed::Unresolved(copy_text(
            budget, "color transformation has no unambiguous direct picture file",
        )),
    };
    Some(SourceColorAdapter {
        master: master_id,
        master_track,
        source_in,
        picture_file,
        transform: copy_text(budget, transform),
        neutral_wrapper: neutral_wrapper(ctx, effect),
    })
}

fn neutral_wrapper(ctx: &Ctx<'_>, effect: &Node) -> bool {
    let attrs = borrowed_attrs(ctx, effect);
    let border = unique_attribute(attrs, "_BORDER_FIXED").and_then(Value::as_int);
    attrs.len() == 1 && border == Some(0)
        && ["left_bob", "right_bob", "precomputed", "session_attrs", "trackman"]
            .iter().all(|k| effect.get(k) == Some(&Value::Null))
        && [
            ("mc_mode", 0), ("num_scalars", 0), ("info_version", 2),
            ("info_current", 0), ("info_smooth", 0), ("info_color_item", 1),
            ("info_quality", 1), ("info_is_reversed", 0),
            ("left_length", 15), ("right_length", 15),
        ].iter().all(|(key, expected)| int(effect, key) == Some(*expected))
        && [
            ("info_aspect_on", true), ("info_force_software", false),
            ("info_never_hardware", false),
        ].iter().all(|(key, expected)| effect.get(key) == Some(&Value::Bool(*expected)))
        && ctx.deref(effect.get("keyframes"))
            .is_some_and(|fx| matte::neutral_keys(fx, 1 << 29))
}

fn sole_picture_input<'a>(ctx: &Ctx<'a>, component: &'a Node) -> Option<&'a Node> {
    let clip = if component.class == *b"SEQU" {
        sole_picture_segment(ctx, component)?
    } else {
        component
    };
    (clip.class == *b"SCLP"
        && int(clip, "media_kind_id") == Some(KIND_PICTURE)
        && int(clip, "length").is_some_and(|length| length > 0))
        .then_some(clip)
}

fn direct_picture_file<'a>(
    ctx: &Ctx<'a>, master_id: MobId, master_track: i64, ambiguous: &HashSet<MobId>,
) -> Option<(MobId, &'a Node)> {
    let master = ctx.mobs.get(&master_id)?;
    if ambiguous.contains(&master_id) || int(master, "mob_type_id") != Some(MOB_MASTER) {
        return None;
    }
    let master_tracks = ctx.tracks(master);
    let mut matches = master_tracks.iter().filter(|track| {
        int(track, "index") == Some(master_track)
            && ctx.deref(track.get("component")).and_then(|node| int(node, "media_kind_id"))
                == Some(KIND_PICTURE)
    });
    let backing = sole_picture_input(ctx, ctx.deref(matches.next()?.get("component"))?)?;
    let file_id = mob_id(backing)?;
    let file = ctx.mobs.get(&file_id)?;
    if matches.next().is_some() || ambiguous.contains(&file_id)
        || int(file, "mob_type_id") != Some(MOB_SOURCE) {
        return None;
    }
    Some((file_id, file))
}

fn saved_payload<'a>(ctx: &Ctx<'a>, effect: &Node) -> Option<&'a str> {
    const ENABLE: &str = "dca5f78a7b361949afc75ca00a5bd558";
    const PAYLOAD: &str = "d85c7fbdfd154e42a34d11642fbbb867";
    const TRANSFORM: &str = "cc999a218b2c244286fec05794055e1d";
    let list = ctx.deref(effect.get("param_list")).filter(|n| n.class == *b"PRLS")?;
    let Value::RefList(items) = list.get("items")? else { return None };
    if items.len() != 2 {
        return None;
    }
    let parameter = |id, ty, signature| {
        items.iter().filter_map(|r| ctx.deref(Some(r))).find(|n| {
            n.class == *b"PRIT"
                && matches!(n.get("uuid"), Some(Value::Uuid(u)) if file_order_hex(u) == id)
                && int(n, "value_type") == Some(ty)
                && n.get("enable") == Some(&Value::Bool(true))
                && n.get("control_track") == Some(&Value::Null)
                && n.get("contribs_to_sig") == Some(&Value::Bool(signature))
        })
    };
    if int(parameter(ENABLE, 1, false)?, "value") != Some(1) {
        return None;
    }
    let value = ctx.deref(parameter(PAYLOAD, 4, true)?.get("value"))?;
    if value.class != *b"AVUP" || int(value, "byte_order") != Some(18761)
        || !matches!(value.get("uuid"), Some(Value::Uuid(u)) if file_order_hex(u) == TRANSFORM)
    {
        return None;
    }
    let Value::Bytes(bytes) = value.get("data")? else { return None };
    let text = bytes.strip_suffix(&[0])?;
    if text.is_empty() || text.len() > MAX_INPUT_TRANSFORM_BYTES || text.contains(&0) {
        return None;
    }
    std::str::from_utf8(text).ok()
}
