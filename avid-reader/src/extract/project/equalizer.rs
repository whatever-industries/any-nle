//! Saved legacy EQ settings and the input eligible for dry playback.
use super::*;
use crate::source::{SourceEqualizer, SourceEqualizerBand, SourceEqualizerRender, SourceSubclipGain};
use std::collections::HashSet;

mod pitch;
pub(super) use pitch::rendered_pitch_stack;

pub(super) fn input<'a>(
    ctx: &Ctx<'a>, effect: &Node, frames: i64,
) -> Option<(&'a Node, SourceEqualizer)> {
    if !matches!(effect.get("precomputed"), None | Some(Value::Null)) {
        return None;
    }
    let settings = settings(effect, frames)?;
    let input = gain::full_span_mono_source(ctx, effect, frames, |n| plain_input(ctx, n))?;
    Some((input, settings))
}

fn settings(effect: &Node, frames: i64) -> Option<SourceEqualizer> {
    if frames <= 0
        || effect.class != *b"EQMB"
        || !gain::span(effect, frames)
        || int(effect, "media_kind_id") != Some(KIND_SOUND)
        || effect.get("effect_id").and_then(Value::as_str) != Some("EFF_AUDIO_EQMB")
        || int(effect, "info_is_reversed") != Some(0)
        || !["param_list", "keyframes", "left_bob", "right_bob", "trackman"]
            .iter()
            .all(|field| matches!(effect.get(field), None | Some(Value::Null)))
    {
        return None;
    }
    let Value::Bool(enabled) = effect.get("effect_enable")? else { return None };
    let filter_name = effect.get("filter_name")?.as_str()?;
    let Value::List(bands) = effect.get("bands")? else { return None };
    if !matches!(bands.len(), 3 | 4) || filter_name.len() > 64 {
        return None;
    }
    let bands = bands.iter().map(|band| {
        let Value::Node(band) = band else { return None };
        let Value::Bool(enabled) = band.get("enable")? else { return None };
        Some(SourceEqualizerBand {
            filter_type: int(band, "type")?,
            frequency: int(band, "freq")?,
            gain: int(band, "gain")?,
            q: int(band, "q")?,
            enabled: *enabled,
        })
    }).collect::<Option<Vec<_>>>()?;
    Some(SourceEqualizer { enabled: *enabled, filter_name: filter_name.into(), bands, render: None, pitch: None })
}

pub(super) fn plain_input(ctx: &Ctx<'_>, node: &Node) -> bool {
    ["left_bob", "right_bob", "precomputed", "param_list"]
        .iter()
        .all(|field| matches!(node.get(field), None | Some(Value::Null)))
        && match node.get("attributes") {
            None | Some(Value::Null) => true,
            attribute => ctx.deref(attribute).is_some_and(|attrs| {
                matches!(attrs.get("entries"), Some(Value::Attrs(values))
                    if values.iter().all(|(key, _)| key != "_TMP_CRM"))
            }),
        }
}

fn mono_lane<'a>(ctx: &Ctx<'a>, mob: &'a Node, label: i64) -> Option<&'a Node> {
    let mut sound = None;
    for track in ctx.tracks(mob).into_iter().filter(|track| int(track, "index") == Some(label)) {
        let component = ctx.deref(track.get("component"))?;
        if int(component, "media_kind_id")? == KIND_SOUND
            && (sound.replace(component).is_some()
                || int(track, "lock_number").is_some_and(|lock| lock != 0 && lock != 1 && lock != label)
                || !matches!(track.get("filler_proxy"), None | Some(Value::Null)))
        {
            return None;
        }
    }
    sound
}

pub(super) fn sound_clip(node: &Node) -> Option<(MobId, i64, i64)> {
    (node.class == *b"SCLP"
        && float(node, "edit_rate") == Some(24.0)
        && int(node, "media_kind_id") == Some(KIND_SOUND)).then_some(())?;
    Some((
        mob_id(node)?,
        int(node, "track_id").filter(|&n| n > 0)?,
        int(node, "start_time").filter(|&n| n >= 0)?,
    ))
}

/// Select the single same-clock sound component, excluding leading filler.
fn track_component<'a>(
    ctx: &Ctx<'a>, mut component: &'a Node, mut start: i64, frames: i64,
) -> Option<(&'a Node, i64)> {
    if float(component, "edit_rate") != Some(24.0)
        || int(component, "media_kind_id") != Some(KIND_SOUND)
        || !plain_input(ctx, component)
    {
        return None;
    }
    if component.class == *b"SEQU" {
        let Value::RefList(parts) = component.get("components")? else { return None };
        let mut position = 0i64;
        let mut sound = None;
        for part in parts {
            let child = ctx.deref(Some(part))?;
            let length = int(child, "length").filter(|&n| n >= 0)?;
            if float(child, "edit_rate") != Some(24.0)
                || int(child, "media_kind_id") != Some(KIND_SOUND)
                || !plain_input(ctx, child)
            {
                return None;
            }
            if child.class != *b"FILL" && sound.replace((position, child)).is_some() {
                return None;
            }
            position = position.checked_add(length)?;
        }
        let (origin, child) = sound?;
        start = start.checked_sub(origin)?;
        component = child;
    }
    if start < 0 || start.checked_add(frames)? > int(component, "length")? {
        return None;
    }
    Some((component, start))
}

fn track_input<'a>(
    ctx: &Ctx<'a>, component: &'a Node, start: i64, frames: i64, require_pan_volume: bool,
) -> Option<(&'a Node, i64)> {
    let (mut component, start) = track_component(ctx, component, start, frames)?;
    if component.class == *b"PVOL" {
        let (input, None) = volume::subclip_source(ctx, component)? else { return None };
        component = input;
    } else if require_pan_volume {
        return None;
    }
    (plain_input(ctx, component) && sound_clip(component).is_some()).then_some((component, start))
}

fn gained_track_input<'a>(
    ctx: &Ctx<'a>, component: &'a Node, start: i64, frames: i64,
) -> Option<(&'a Node, i64, SourceSubclipGain)> {
    let (wrapper, start) = track_component(ctx, component, start, frames)?;
    let (input, gain) = volume::equalizer_gain_source(ctx, wrapper)?;
    plain_input(ctx, input).then_some((input, start, gain))
}

pub(super) fn subclip_file(
    ctx: &Ctx<'_>, subclip: &Node, label: i64, start: i64, frames: i64,
) -> Option<(MobId, i64, MobId, i64, i64)> {
    let (backing, local) = track_input(ctx, mono_lane(ctx, subclip, label)?, start, frames, false)?;
    let (master_id, master_track, offset) = sound_clip(backing)?;
    let start = local.checked_add(offset)?;
    let master = ctx.mobs.get(&master_id)?;
    if int(master, "usage_code") != Some(0) {
        return None;
    }
    let (file, file_track, start, None) = master_file(ctx, master, master_track, start, frames)? else {
        return None;
    };
    Some((master_id, master_track, file, file_track, start))
}

pub(super) fn master_file(
    ctx: &Ctx<'_>, master: &Node, master_track: i64, start: i64, frames: i64,
) -> Option<(MobId, i64, i64, Option<SourceSubclipGain>)> {
    if int(master, "mob_type_id") != Some(MOB_MASTER) || float(master, "edit_rate") != Some(24.0)
        || start.checked_add(frames)? > int(master, "length")?
    {
        return None;
    }
    let usage = int(master, "usage_code")?;
    let (source, local, input_gain) = match usage {
        0 => {
            let track = mono_lane(ctx, master, master_track)?;
            if let Some((source, local)) = track_input(ctx, track, start, frames, false) {
                (source, local, None)
            } else {
                if master_track != 1 || gain::mono(ctx, master)?.class != *b"PVOL"
                    || !gain::span(track, int(master, "length")?)
                {
                    return None;
                }
                let (source, local, gain) = gained_track_input(ctx, track, start, frames)?;
                (source, local, Some(gain))
            }
        }
        // This saved master has a single full-span Pan Volume input. The
        // render's effect copy must independently corroborate the route.
        7 if master_track == 1 => {
            let wrapper = gain::mono(ctx, master)?;
            if wrapper.class != *b"PVOL" || !gain::span(wrapper, int(master, "length")?) {
                return None;
            }
            let (source, local) = track_input(ctx, wrapper, start, frames, true)?;
            (source, local, None)
        }
        _ => return None,
    };
    let (file, track, start) =
        pcm_file_range(ctx, source, local, frames, usage == 7 || input_gain.is_some())?;
    Some((file, track, start, input_gain))
}

pub(super) fn neutral_aaf_master_file(
    ctx: &Ctx<'_>, master: &Node, label: i64, start: i64, frames: i64,
) -> Option<(MobId, i64, i64)> {
    let wrapper = mono_lane(ctx, master, label)?;
    let (source, start) = track_component(ctx, wrapper, start, frames)?;
    if !gain::span(wrapper, int(master, "length")?) {
        return None;
    }
    let source = volume::neutral_aaf_source(ctx, source)?;
    if !plain_input(ctx, source) {
        return None;
    }
    pcm_file_range(ctx, source, start, frames, false)
}

fn pcm_file_range(
    ctx: &Ctx<'_>, source: &Node, local: i64, frames: i64, strict_pcm24: bool,
) -> Option<(MobId, i64, i64)> {
    let (file_id, file_track, offset) = sound_clip(source)?;
    if strict_pcm24 && (file_track != 1 || offset != 0) {
        return None;
    }
    let start = local.checked_add(offset)?;
    let file = ctx.mobs.get(&file_id)?;
    let descriptor = ctx.deref(file.get("descriptor"))?;
    if descriptor.class != *b"PCMA" || int(descriptor, "channels") != Some(1)
        || strict_pcm24 && (int(descriptor, "quantization_bits") != Some(24)
            || int(descriptor, "block_align") != Some(3))
        || float(descriptor, "sample_rate") != Some(48_000.0)
        || float(descriptor, "edit_rate") != Some(48_000.0)
        || float(file, "edit_rate") != Some(24.0)
        || !gain::span(mono_lane(ctx, file, file_track)?, int(file, "length")?)
        || start.checked_add(frames)? > int(file, "length")?
        || start.checked_add(frames)?.checked_mul(2_000)? > int(descriptor, "length")?
    {
        return None;
    }
    Some((file_id, file_track, start))
}

pub(super) fn rendered<'a>(
    ctx: &Ctx<'a>, effect: &Node, frames: i64, ambiguous: &HashSet<MobId>,
) -> Option<(&'a Node, SourceEqualizer)> {
    let mut saved = settings(effect, frames)?;
    if !saved.enabled {
        return None;
    }
    let input = gain::full_span_mono_clip(ctx, effect, frames, |n| plain_input(ctx, n))?;
    let (dry, label, start) = sound_clip(input)?;
    let source = ctx.mobs.get(&dry)?;
    if float(source, "edit_rate") != Some(24.0) || start.checked_add(frames)? > int(source, "length")? {
        return None;
    }
    let wrapped_master = int(source, "mob_type_id") == Some(MOB_MASTER)
        && int(source, "usage_code") == Some(7);
    let (master_id, dry_master_track, dry_file, dry_track, dry_start, subclip, input_gain) = if int(source, "mob_type_id") == Some(MOB_MASTER) {
        let (file, track, start, gain) = master_file(ctx, source, label, start, frames)?;
        (dry, label, file, track, start, None, gain)
    } else if int(source, "mob_type_id") == Some(MOB_COMPOSITION) && int(source, "usage_code") == Some(2) {
        let (master, master_track, file, track, start) = subclip_file(ctx, source, label, start, frames)?;
        (master, master_track, file, track, start, Some(dry), None)
    } else {
        return None;
    };
    let dry_source = ctx.mobs.get(&dry_file)?;
    if int(dry_source, "mob_type_id") != Some(MOB_SOURCE)
        || int(dry_source, "usage_code") != Some(0)
    {
        return None;
    }
    let render = gain::render_chain(ctx, effect, frames, 0)?;
    let copy = render.effect;
    let ids = [master_id, dry_file, render.master, render.file, render.origin];
    if ids.iter().enumerate().any(|(i, id)| ambiguous.contains(id) || ids[..i].contains(id))
        || subclip.is_some_and(|sub| ambiguous.contains(&sub) || ids.contains(&sub))
        || !matches!(copy.get("precomputed"), None | Some(Value::Null))
        || settings(copy, render.frames)? != saved
    {
        return None;
    }
    let copied_track = gain::mono(ctx, copy)?;
    let (copy_input, local) = if let Some(expected) = input_gain {
        let (input, local, actual) = gained_track_input(ctx, copied_track, render.start, frames)?;
        if actual != expected {
            return None;
        }
        (input, local)
    } else {
        track_input(ctx, copied_track, render.start, frames, wrapped_master)?
    };
    let (copy_file, copy_track, copy_start) = sound_clip(copy_input)?;
    if (copy_file, copy_track, copy_start.checked_add(local)?) != (dry_file, dry_track, dry_start) {
        return None;
    }
    saved.render = Some(SourceEqualizerRender {
        file: render.file, origin: render.origin, start: render.start, frames: render.frames,
        dry_master: master_id, dry_master_track, dry_file, dry_track,
        dry_start, dry_subclip: subclip, dry_wrapped_master: wrapped_master, input_gain,
    });
    Some((render.clip, saved))
}
