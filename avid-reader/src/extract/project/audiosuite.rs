//! Link the saved output of a single mono AudioSuite plug-in.
use super::*;
use crate::source::SourceAudioSuiteRender;
use std::collections::HashSet;

const PADDING_SAMPLES: i64 = 2;
const MAX_CHUNK_BYTES: usize = 16_384;

pub(super) fn binding<'a>(
    ctx: &Ctx<'a>,
    effect: &Node,
    frames: i64,
    ambiguous: &HashSet<MobId>,
) -> Option<(&'a Node, SourceAudioSuiteRender)> {
    let mode = int(effect, "rendering_mode").filter(|mode| matches!(mode, 1 | 2))?;
    let (plugin, chunk, attrs) = settings(ctx, effect, frames)?;
    let cache = gain::render_chain(ctx, effect, frames, PADDING_SAMPLES)?;
    let copy = cache.effect;
    let copy_mode = int(copy, "rendering_mode")?;
    let trimmed = cache.start != 0 || cache.frames != frames;
    if !(copy_mode == mode || trimmed && mode == 2 && copy_mode == 1)
        || !matches!(copy.get("precomputed"), None | Some(Value::Null))
        || settings(ctx, copy, cache.frames)? != (plugin, chunk, attrs)
        || saved_fields(effect).ne(saved_fields(copy))
    {
        return None;
    }

    let input = gain::full_span_mono_clip(ctx, effect, frames, |n| equalizer::plain_input(ctx, n))?;
    let (source_id, label, start) = equalizer::sound_clip(input)?;
    let source = ctx.mobs.get(&source_id)?;
    if float(source, "edit_rate") != Some(24.0)
        || start.checked_add(frames)? > int(source, "length")?
    {
        return None;
    }
    let copied = gain::single_mono_component(ctx, copy, |n| equalizer::plain_input(ctx, n))?;
    let sequence_input = volume::neutral_aaf_source(ctx, copied);
    let (input_subclip, input_master, input_track, input_file, file_track, input_start) =
        match (int(source, "mob_type_id")?, int(source, "usage_code")?) {
            (MOB_MASTER, 0) if sequence_input.is_some() => {
                let (file, track, start) =
                    equalizer::neutral_aaf_master_file(ctx, source, label, start, frames)?;
                (None, source_id, label, file, track, start)
            }
            (MOB_MASTER, 0) => {
                let (file, track, start, None) =
                    equalizer::master_file(ctx, source, label, start, frames)?
                else {
                    return None;
                };
                (None, source_id, label, file, track, start)
            }
            (MOB_COMPOSITION, 2) if sequence_input.is_none() => {
                let (master, lane, file, track, input_start) =
                    equalizer::subclip_file(ctx, source, label, start, frames)?;
                (
                    Some((source_id, label, start)),
                    master,
                    lane,
                    file,
                    track,
                    input_start,
                )
            }
            _ => return None,
        };
    let copied = if copied.class == *b"PVOL" {
        if !gain::span(copied, cache.frames) {
            return None;
        }
        if let Some(clip) = sequence_input {
            clip
        } else {
            match volume::subclip_source(ctx, copied) {
                Some((clip, None)) => clip,
                None if input_subclip.is_some() => volume::stacked_neutral_source(ctx, copied)?,
                _ => return None,
            }
        }
    } else {
        copied
    };
    if !gain::span(copied, cache.frames) || !equalizer::plain_input(ctx, copied) {
        return None;
    }
    let (copied_file, copied_track, copied_start) = equalizer::sound_clip(copied)?;
    if (
        copied_file,
        copied_track,
        copied_start.checked_add(cache.start)?,
    ) != (input_file, file_track, input_start)
    {
        return None;
    }
    let file = ctx.mobs.get(&input_file)?;
    let ids = [
        input_master,
        input_file,
        cache.master,
        cache.file,
        cache.origin,
    ];
    if int(file, "mob_type_id") != Some(MOB_SOURCE)
        || int(file, "usage_code") != Some(0)
        || ids
            .iter()
            .enumerate()
            .any(|(i, id)| ambiguous.contains(id) || ids[..i].contains(id))
        || input_subclip.is_some_and(|(id, _, _)| ambiguous.contains(&id) || ids.contains(&id))
    {
        return None;
    }
    let Value::Bytes(bytes) = chunk.get("data")? else {
        return None;
    };
    Some((
        cache.clip,
        SourceAudioSuiteRender {
            file: cache.file,
            origin: cache.origin,
            start: cache.start,
            frames: cache.frames,
            padding_samples: PADDING_SAMPLES,
            input_subclip,
            input_master,
            input_track,
            input_file,
            file_track,
            input_start,
            name: plugin.get("name")?.as_str()?.into(),
            manufacturer_id: fourcc(plugin, "manufacturer_id")?,
            product_id: fourcc(plugin, "product_id")?,
            plugin_id: fourcc(plugin, "plugin_id")?,
            chunk_id: fourcc(chunk, "chunk_id")?,
            chunk_version: int(chunk, "version")?,
            chunk: bytes.clone(),
        },
    ))
}

fn saved_fields(node: &Node) -> impl Iterator<Item = &(&'static str, Value)> {
    node.fields.iter().filter(|(field, _)| {
        !matches!(
            *field,
            "tracks" | "precomputed" | "attributes" | "plugins" | "length" | "rendering_mode"
        )
    })
}

fn fourcc(node: &Node, key: &str) -> Option<u32> {
    let Value::UInt(value) = node.get(key)? else {
        return None;
    };
    u32::try_from(*value).ok().filter(|n| *n != 0)
}

fn settings<'a>(
    ctx: &Ctx<'a>,
    effect: &'a Node,
    frames: i64,
) -> Option<(&'a Node, &'a Node, &'a Value)> {
    if effect.class != *b"ASPI"
        || !gain::span(effect, frames)
        || effect.get("effect_id")?.as_str() != Some("EFF_AUDIO_AS_PLUG_IN")
        || int(effect, "media_kind_id") != Some(KIND_SOUND)
        || int(effect, "padding_secs") != Some(0)
        || int(effect, "info_is_reversed") != Some(0)
        || int(effect, "mc_mode") != Some(0)
        || int(effect, "num_scalars") != Some(0)
        || [int(effect, "mark_in"), int(effect, "mark_out")] != [Some(-1); 2]
        || effect.get("tracks_to_affect") != Some(&Value::UInt(0))
        || [
            "left_bob",
            "right_bob",
            "session_attrs",
            "param_list",
            "keyframes",
            "trackman",
        ]
        .iter()
        .any(|field| !matches!(effect.get(field), None | Some(Value::Null)))
    {
        return None;
    }
    let Value::List(plugins) = effect.get("plugins")? else {
        return None;
    };
    let [Value::Node(plugin)] = plugins.as_slice() else {
        return None;
    };
    let name = plugin.get("name")?.as_str()?;
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return None;
    }
    let Value::List(chunks) = plugin.get("chunks")? else {
        return None;
    };
    let [Value::Node(chunk)] = chunks.as_slice() else {
        return None;
    };
    if !matches!(chunk.get("data"), Some(Value::Bytes(bytes)) if !bytes.is_empty() && bytes.len() <= MAX_CHUNK_BYTES)
        || ["manufacturer_id", "product_id"]
            .iter()
            .any(|key| fourcc(plugin, key).is_none() || plugin.get(key) != chunk.get(key))
    {
        return None;
    }
    let attrs = ctx.deref(effect.get("attributes"))?.get("entries")?;
    let Value::Attrs(values) = attrs else {
        return None;
    };
    if values.len() > 40
        || values.iter().any(|(_, value)| match value {
            Value::Bytes(bytes) => bytes.len() > MAX_CHUNK_BYTES,
            Value::Str(text) => text.len() > 1_024,
            _ => value.as_int().is_none(),
        })
    {
        return None;
    }
    let attribute = |key: &str| {
        let mut matches = values.iter().filter(|(name, _)| name == key);
        let value = &matches.next()?.1;
        matches.next().is_none().then_some(value)
    };
    if attribute("_EFFECT_PLUGIN_TYPE")?.as_str() != Some("AAXClip")
        || attribute("_EFFECT_PLUGIN_NAME")?.as_str() != Some(name)
        || attribute("_AS_EFFECT_0_MAX_INPUT_PORTS")?.as_int() != Some(1)
        || attribute("_AS_EFFECT_0_MAX_OUTPUT_PORTS")?.as_int() != Some(1)
    {
        return None;
    }
    Some((plugin, chunk, attrs))
}
