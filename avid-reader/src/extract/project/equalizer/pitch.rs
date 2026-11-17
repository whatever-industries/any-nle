//! Saved Pitch II beneath legacy EQ, with a separate render for each effect.
use super::*;
use crate::source::SourcePitchCache;

struct PitchSettings<'a> {
    plugin: &'a Value,
    attributes: &'a Value,
    chunk: &'a [u8; 148],
}

fn pitch_settings<'a>(ctx: &Ctx<'a>, effect: &'a Node, mode: i64, frames: i64) -> Option<PitchSettings<'a>> {
    if effect.class != *b"ASPI" || !gain::span(effect, frames)
        || int(effect, "media_kind_id") != Some(KIND_SOUND)
        || effect.get("effect_id")?.as_str() != Some("EFF_AUDIO_AS_PLUG_IN")
        || int(effect, "rendering_mode") != Some(mode)
        || int(effect, "padding_secs") != Some(0)
        || int(effect, "info_is_reversed") != Some(0)
        || effect.get("tracks_to_affect") != Some(&Value::UInt(0))
        || ["left_bob", "right_bob", "session_attrs", "param_list", "keyframes", "trackman"]
            .iter().any(|field| !matches!(effect.get(field), None | Some(Value::Null | Value::Ref(0))))
    {
        return None;
    }
    let plugin = effect.get("plugins")?;
    let Value::List(plugins) = plugin else { return None };
    let [Value::Node(pitch)] = plugins.as_slice() else { return None };
    let Value::List(chunks) = pitch.get("chunks")? else { return None };
    let [Value::Node(chunk)] = chunks.as_slice() else { return None };
    if pitch.get("name")?.as_str() != Some("Pitch II") || int(chunk, "version") != Some(1)
        || chunk.get("chunk_id") != Some(&Value::UInt(1349077321))
    {
        return None;
    }
    for (field, id) in [("manufacturer_id", 1147758441), ("product_id", 1349088104), ("plugin_id", 1349738867)] {
        if pitch.get(field) != Some(&Value::UInt(id)) || chunk.get(field) != Some(&Value::UInt(id)) {
            return None;
        }
    }
    let Value::Bytes(bytes) = chunk.get("data")? else { return None };
    let attributes = ctx.deref(effect.get("attributes"))?.get("entries")?;
    let Value::Attrs(values) = attributes else { return None };
    if values.len() > 16 || values.iter().any(|(_, v)| match v {
        Value::Bytes(v) => v.len() > 256,
        Value::Str(v) => v.len() > 128,
        _ => v.as_int().is_none(),
    }) {
        return None;
    }
    Some(PitchSettings { plugin, attributes, chunk: bytes.as_slice().try_into().ok()? })
}

fn pitch_input<'a>(ctx: &Ctx<'a>, eq: &'a Node) -> Option<&'a Node> {
    let seq = gain::mono(ctx, eq)?;
    if seq.class != *b"SEQU" || float(seq, "edit_rate") != Some(24.0) || !plain_input(ctx, seq) {
        return None;
    }
    let Value::RefList(parts) = seq.get("components")? else { return None };
    let [first, pitch, last] = parts.as_slice() else { return None };
    for part in [first, last] {
        let filler = ctx.deref(Some(part))?;
        if filler.class != *b"FILL" || !gain::span(filler, 0)
            || int(filler, "media_kind_id") != Some(KIND_SOUND) || !plain_input(ctx, filler)
        {
            return None;
        }
    }
    ctx.deref(Some(pitch))
}

fn copied_source(ctx: &Ctx<'_>, pitch: &Node, frames: i64) -> Option<(MobId, i64, i64)> {
    let (wrapper, 0) = track_component(ctx, gain::mono(ctx, pitch)?, 0, frames)? else { return None };
    let clip = volume::stacked_neutral_source(ctx, wrapper)?;
    (gain::span(wrapper, frames) && gain::span(clip, frames) && plain_input(ctx, clip))
        .then_some(())?;
    sound_clip(clip)
}

pub(in crate::extract::project) fn rendered_pitch_stack<'a>(
    ctx: &Ctx<'a>, eq: &'a Node, frames: i64, ambiguous: &HashSet<MobId>,
) -> Option<(&'a Node, SourceEqualizer)> {
    if frames != 48 { return None; }
    let mut equalizer = settings(eq, frames)?;
    if !equalizer.enabled { return None; }
    let outer = gain::render_chain(ctx, eq, frames, 0)?;
    if outer.start != 0 || outer.frames != frames || settings(outer.effect, frames)? != equalizer
        || !matches!(outer.effect.get("precomputed"), None | Some(Value::Null))
    {
        return None;
    }
    let live = pitch_input(ctx, eq)?;
    let pitch = pitch_settings(ctx, live, 2, frames)?;
    let outer_copy = pitch_input(ctx, outer.effect)?;
    let inner = gain::render_chain(ctx, live, frames, 2)?;
    if inner.start != 0 || inner.frames != frames { return None; }
    let inner_mode = int(inner.effect, "rendering_mode")?;
    if !matches!(inner_mode, 1 | 2) { return None; }
    for (copy, mode) in [(outer_copy, 2), (inner.effect, inner_mode)] {
        let saved = pitch_settings(ctx, copy, mode, frames)?;
        if !plain_input(ctx, copy) || saved.plugin != pitch.plugin || saved.attributes != pitch.attributes
            || ["mark_in", "mark_out", "preset_path", "num_scalars", "mc_mode"]
                .iter().any(|field| copy.get(field) != live.get(field))
        {
            return None;
        }
    }
    let input = gain::full_span_mono_source(ctx, live, frames, |node| plain_input(ctx, node))?;
    let (subclip, lane, start) = sound_clip(input)?;
    let sub = ctx.mobs.get(&subclip)?;
    if int(sub, "mob_type_id") != Some(MOB_COMPOSITION) || int(sub, "usage_code") != Some(2) {
        return None;
    }
    let (master, master_track, source, source_track, source_start) = subclip_file(ctx, sub, lane, start, frames)?;
    let file = ctx.mobs.get(&source)?;
    let descriptor = ctx.deref(file.get("descriptor"))?;
    if int(descriptor, "quantization_bits") != Some(24) || int(descriptor, "block_align") != Some(3) {
        return None;
    }
    let ids = [outer.master, outer.file, outer.origin, inner.master, inner.file, inner.origin, subclip, master, source];
    if ids.iter().enumerate().any(|(i, id)| ambiguous.contains(id) || ids[..i].contains(id))
        || int(file, "mob_type_id") != Some(MOB_SOURCE) || int(file, "usage_code") != Some(0)
        || copied_source(ctx, outer_copy, frames)? != (source, source_track, source_start)
        || copied_source(ctx, inner.effect, frames)? != (source, source_track, source_start)
    {
        return None;
    }
    equalizer.render = Some(SourceEqualizerRender {
        file: outer.file, origin: outer.origin, start: outer.start, frames: outer.frames,
        dry_master: master, dry_master_track: master_track, dry_file: source, dry_track: source_track,
        dry_start: source_start, dry_subclip: Some(subclip), dry_wrapped_master: false, input_gain: None,
    });
    equalizer.pitch = Some(Box::new(SourcePitchCache {
        master: inner.master, file: inner.file, origin: inner.origin,
        samples: frames * 2_000 + 2, pitch_chunk: *pitch.chunk,
    }));
    Some((outer.clip, equalizer))
}
