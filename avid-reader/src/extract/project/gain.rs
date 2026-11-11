//! A saved mono AudioSuite Gain render on a 24 fps edit.
use super::*;
use std::collections::HashSet;

const EDIT_RATE: i64 = 24;
const SAMPLE_RATE: i64 = 48_000;

pub(super) fn candidate(effect: &Node) -> bool {
    matches!(effect.get("plugins"), Some(Value::List(plugins)) if plugins.iter().any(
        |p| matches!(p, Value::Node(n) if n.get("name").and_then(Value::as_str) == Some("Gain")
            || n.get("plugin_id") == Some(&Value::UInt(1195460942)))))
}

fn settings(effect: &Node, frames: i64) -> Option<&Value> {
    if effect.class != *b"ASPI"
        || !span(effect, frames)
        || int(effect, "media_kind_id") != Some(KIND_SOUND)
        || effect.get("effect_id").and_then(Value::as_str) != Some("EFF_AUDIO_AS_PLUG_IN")
        || int(effect, "rendering_mode") != Some(1)
        || int(effect, "padding_secs") != Some(0)
        || int(effect, "info_is_reversed") != Some(0)
        || effect.get("tracks_to_affect") != Some(&Value::UInt(0))
        || !matches!(effect.get("param_list"), None | Some(Value::Null))
    {
        return None;
    }
    let value = effect.get("plugins")?;
    let Value::List(plugins) = value else {
        return None;
    };
    let [Value::Node(plugin)] = plugins.as_slice() else {
        return None;
    };
    if plugin.get("name").and_then(Value::as_str) != Some("Gain") {
        return None;
    }
    let Value::List(chunks) = plugin.get("chunks")? else {
        return None;
    };
    let [Value::Node(chunk)] = chunks.as_slice() else {
        return None;
    };
    for (field, id) in [
        ("manufacturer_id", 1147758441),
        ("product_id", 1314018336),
        ("plugin_id", 1195460942),
    ] {
        if plugin.get(field) != Some(&Value::UInt(id)) || chunk.get(field) != Some(&Value::UInt(id))
        {
            return None;
        }
    }
    (int(chunk, "version") == Some(2)
        && chunk.get("chunk_id") == Some(&Value::UInt(1097624608))
        && matches!(chunk.get("data"), Some(Value::Bytes(bytes)) if bytes.len() == 12))
    .then_some(value)
}

fn span(node: &Node, frames: i64) -> bool {
    int(node, "length") == Some(frames) && float(node, "edit_rate") == Some(EDIT_RATE as f64)
}

fn mono<'a>(ctx: &Ctx<'a>, node: &Node) -> Option<&'a Node> {
    let Value::List(tracks) = node.get("tracks")? else {
        return None;
    };
    let [Value::Node(track)] = tracks.as_slice() else {
        return None;
    };
    let c = ctx.deref(track.get("component"))?;
    (int(track, "index") == Some(1) && int(c, "media_kind_id") == Some(KIND_SOUND)).then_some(c)
}

fn clip(node: &Node, frames: i64) -> Option<(MobId, i64)> {
    if node.class != *b"SCLP"
        || !span(node, frames)
        || int(node, "media_kind_id") != Some(KIND_SOUND)
        || int(node, "track_id") != Some(1)
    {
        return None;
    }
    Some((mob_id(node)?, int(node, "start_time")?))
}

fn dry<'a>(ctx: &Ctx<'a>, effect: &Node, frames: i64) -> Option<(MobId, i64)> {
    let seq = mono(ctx, effect)?;
    if seq.class != *b"SEQU" || float(seq, "edit_rate") != Some(EDIT_RATE as f64) {
        return None;
    }
    let Value::RefList(parts) = seq.get("components")? else {
        return None;
    };
    let [first, input, last] = parts.as_slice() else {
        return None;
    };
    for f in [first, last] {
        let f = ctx.deref(Some(f))?;
        if f.class != *b"FILL" || int(f, "length") != Some(0) {
            return None;
        }
    }
    clip(ctx.deref(Some(input))?, frames)
}

pub(super) fn binding<'a>(
    ctx: &Ctx<'a>,
    effect: &Node,
    ambiguous: &HashSet<MobId>,
) -> Option<(&'a Node, MobId, MobId)> {
    let frames = int(effect, "length")?;
    // Native references qualify exactly one and two seconds at 24 fps.
    if frames != EDIT_RATE && frames != 2 * EDIT_RATE {
        return None;
    }
    let samples = frames * (SAMPLE_RATE / EDIT_RATE);
    let controls = settings(effect, frames)?;
    let input = dry(ctx, effect, frames)?;
    let master_input = ctx.mobs.get(&input.0)?;
    if input.1 < 0
        || int(master_input, "mob_type_id") != Some(MOB_MASTER)
        || float(master_input, "edit_rate") != Some(EDIT_RATE as f64)
        || input.1.checked_add(frames)? > int(master_input, "length")?
    {
        return None;
    }
    // The sequence effect uses the dry master. The precompute source's copy
    // addresses its file directly at the same position.
    let master_source = mono(ctx, master_input)?;
    if master_source.class != *b"SCLP"
        || float(master_source, "edit_rate") != Some(EDIT_RATE as f64)
        || int(master_source, "start_time") != Some(0)
        || int(master_source, "track_id") != Some(1)
        || int(master_source, "length") != int(master_input, "length")
    {
        return None;
    }
    let dry_file = mob_id(master_source)?;
    let dry_source = ctx.mobs.get(&dry_file)?;
    if int(dry_source, "mob_type_id") != Some(MOB_SOURCE)
        || int(dry_source, "usage_code") != Some(0)
    {
        return None;
    }
    let render = ctx.deref(effect.get("precomputed"))?;
    let (master_id, 0) = clip(render, frames)? else {
        return None;
    };
    let master = ctx.mobs.get(&master_id)?;
    if !span(master, frames)
        || int(master, "mob_type_id") != Some(MOB_MASTER)
        || int(master, "usage_code") != Some(1)
    {
        return None;
    }
    let (file_id, 0) = clip(mono(ctx, master)?, frames)? else {
        return None;
    };
    let file = ctx.mobs.get(&file_id)?;
    if !span(file, frames)
        || int(file, "mob_type_id") != Some(MOB_SOURCE)
        || int(file, "usage_code") != Some(9)
    {
        return None;
    }
    let d = ctx.deref(file.get("descriptor"))?;
    if d.class != *b"PCMA"
        || float(d, "edit_rate") != Some(SAMPLE_RATE as f64)
        || float(d, "sample_rate") != Some(SAMPLE_RATE as f64)
        || int(d, "length") != Some(samples + 2)
        || int(d, "channels") != Some(1)
        || int(d, "quantization_bits") != Some(24)
        || int(d, "block_align") != Some(3)
        || d.get("intermediate") != Some(&Value::Bool(true))
    {
        return None;
    }
    let locator = ctx.deref(d.get("locator"))?;
    if locator.class != *b"MSML" || mob_id(locator) != Some(file_id) {
        return None;
    }
    let (origin_id, 0) = clip(mono(ctx, file)?, frames)? else {
        return None;
    };
    let origin = ctx.mobs.get(&origin_id)?;
    if !span(origin, frames)
        || int(origin, "mob_type_id") != Some(MOB_SOURCE)
        || int(origin, "usage_code") != Some(14)
    {
        return None;
    }
    let original = mono(ctx, origin)?;
    let ids = [input.0, dry_file, master_id, file_id, origin_id];
    if ids
        .iter()
        .enumerate()
        .any(|(i, id)| ambiguous.contains(id) || ids[..i].contains(id))
        || !matches!(original.get("precomputed"), None | Some(Value::Null))
        || settings(original, frames)? != controls
        || dry(ctx, original, frames)? != (dry_file, input.1)
    {
        return None;
    }
    Some((render, master_id, origin_id))
}
