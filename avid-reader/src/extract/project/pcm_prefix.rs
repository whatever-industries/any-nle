//! A saved 24 fps stereo master with a managed PCM prefix and a separate MPEG tail.
use super::*;
use crate::source::{Rate, SourcePcmPrefix};
use std::collections::HashSet;

pub(in crate::extract) struct Binding<'a> {
    clips: [&'a Node; 2],
    pub(super) fact: SourcePcmPrefix,
}

impl<'a> Binding<'a> {
    pub(in crate::extract) fn clip(&self, kind: i64, label: i64) -> Option<&'a Node> {
        match (kind, label) {
            (KIND_SOUND, 1) => Some(self.clips[0]),
            (KIND_SOUND, 2) => Some(self.clips[1]),
            _ => None,
        }
    }
}

/// Retain a refusal for the observed two-piece topology when its selectors
/// or bounds disagree, even if the master itself cannot be extracted.
pub(super) fn has_mpeg_tail(ctx: &Ctx<'_>, mob: &Node) -> bool {
    int(mob, "mob_type_id") == Some(MOB_MASTER)
        && int(mob, "usage_code") == Some(0)
        && float(mob, "edit_rate").and_then(Rate::from_avid) == Rate::new(24, 1)
        && ctx
            .deref(mob.get("attributes"))
            .and_then(|a| a.get("entries"))
            .is_some_and(|attrs| {
                matches!(attrs, Value::Attrs(entries) if entries.iter().any(|(k, v)|
                k == "_SPANNEDUSINGFILLER" && v.as_int() == Some(1)))
            })
        && ctx.tracks(mob).iter().any(|t| {
            let Some(c) = ctx.deref(t.get("component")) else {
                return false;
            };
            if c.class != *b"SEQU" || int(c, "media_kind_id") != Some(KIND_SOUND) {
                return false;
            }
            let Some(Value::RefList(children)) = c.get("components") else {
                return false;
            };
            let [_, prefix, tail, _] = children.as_slice() else {
                return false;
            };
            let Some(prefix) = ctx.deref(Some(prefix)) else {
                return false;
            };
            if prefix.class != *b"SCLP" || !int(prefix, "length").is_some_and(|n| n > 0) {
                return false;
            }
            ctx.deref(Some(tail))
                .filter(|tail| tail.class == *b"SCLP" && int(tail, "length") == Some(1))
                .and_then(mob_id)
                .and_then(|id| ctx.mobs.get(&id))
                .and_then(|file| ctx.deref(file.get("descriptor")))
                .is_some_and(|d| d.class == *b"MPGA")
        })
}

pub(super) fn inspect<'a>(
    ctx: &Ctx<'a>,
    mob: &'a Node,
    ambiguous: &HashSet<MobId>,
) -> Option<Binding<'a>> {
    let fps = Rate::new(24, 1);
    let clock = |node: &Node| float(node, "edit_rate").and_then(Rate::from_avid);
    let length = int(mob, "length").filter(|&n| n >= 2)?;
    let frames = length - 1;
    let samples = length.checked_mul(2000)?;
    if ambiguous.contains(&mob_id(mob)?) || clock(mob) != fps || int(mob, "usage_code") != Some(0) {
        return None;
    }
    let attrs = ctx.deref(mob.get("attributes"))?;
    let Value::Attrs(attrs) = attrs.get("entries")? else {
        return None;
    };
    for (name, expected) in [
        ("_SPANNEDUSINGFILLER", Value::Int(1)),
        ("_SPANNED_CLIP_FILLER_REMOVED", Value::Int(1)),
        ("_CHANNEL_GROUP_LIST", Value::Str("ST:A1A2,".into())),
    ] {
        let mut matches = attrs.iter().filter(|(key, _)| key == name);
        if matches.next().is_none_or(|(_, value)| *value != expected) || matches.next().is_some() {
            return None;
        }
    }
    let mut audio = [None, None];
    for track in ctx.tracks(mob) {
        let component = ctx.deref(track.get("component"))?;
        if int(component, "media_kind_id") != Some(KIND_SOUND) {
            continue;
        }
        let label = int(track, "index")?;
        let slot = match label {
            1 => &mut audio[0],
            2 => &mut audio[1],
            _ => return None,
        };
        let Value::RefList(children) = component.get("components")? else {
            return None;
        };
        let [head, prefix, tail, end] = children.as_slice() else {
            return None;
        };
        let [head, prefix, tail, end] = [head, prefix, tail, end].map(|n| ctx.deref(Some(n)));
        let (Some(head), Some(prefix), Some(tail), Some(end)) = (head, prefix, tail, end) else {
            return None;
        };
        if slot.is_some()
            || component.class != *b"SEQU"
            || clock(component) != fps
            || !matches!(int(track, "start_pos"), None | Some(0))
            || ![head, prefix, tail, end]
                .iter()
                .all(|n| int(n, "media_kind_id") == Some(KIND_SOUND) && clock(n) == fps)
            || [head, end]
                .iter()
                .any(|n| n.class != *b"FILL" || int(n, "length") != Some(0))
            || ![(prefix, frames), (tail, 1)].iter().all(|(n, length)| {
                n.class == *b"SCLP"
                    && int(n, "length") == Some(*length)
                    && int(n, "track_id") == Some(label)
                    && int(n, "start_time") == Some(0)
            })
        {
            return None;
        }
        let pcm = mob_id(prefix)?;
        let mpeg = mob_id(tail)?;
        if pcm == mpeg || ambiguous.contains(&pcm) || ambiguous.contains(&mpeg) {
            return None;
        }
        let pcm_mob = *ctx.mobs.get(&pcm)?;
        let mpeg_mob = *ctx.mobs.get(&mpeg)?;
        if int(pcm_mob, "mob_type_id") != Some(MOB_SOURCE)
            || int(mpeg_mob, "mob_type_id") != Some(MOB_SOURCE)
            || clock(pcm_mob) != fps
            || int(pcm_mob, "length") != Some(length)
            || int(mpeg_mob, "length") != Some(1)
        {
            return None;
        }
        let d = ctx.deref(pcm_mob.get("descriptor"))?;
        let tail_desc = ctx.deref(mpeg_mob.get("descriptor"))?;
        if d.class != *b"PCMA"
            || tail_desc.class != *b"MPGA"
            || float(d, "edit_rate") != Some(48_000.0)
            || float(d, "sample_rate") != Some(48_000.0)
            || int(d, "channels") != Some(1)
            || int(d, "quantization_bits") != Some(16)
            || int(d, "length") != Some(samples)
            || int(tail_desc, "channels") != Some(2)
        {
            return None;
        }
        let file_tracks = ctx.tracks(pcm_mob);
        let [file_track] = file_tracks.as_slice() else {
            return None;
        };
        let file_clip = ctx.deref(file_track.get("component"))?;
        if int(file_track, "index") != Some(label)
            || file_clip.class != *b"SCLP"
            || !matches!(int(file_track, "start_pos"), None | Some(0))
            || int(file_clip, "media_kind_id") != Some(KIND_SOUND)
            || clock(file_clip) != fps
            || int(file_clip, "length") != Some(length)
            || int(file_clip, "start_time") != Some(0)
            || int(file_clip, "track_id") != Some(label)
        {
            return None;
        }
        *slot = Some((prefix, pcm, mpeg, mob_id(file_clip)?));
    }
    let [Some((left, pcm_a, tail_a, origin_a)), Some((right, pcm_b, tail_b, origin_b))] = audio
    else {
        return None;
    };
    if pcm_a == pcm_b || tail_a != tail_b || origin_a != origin_b {
        return None;
    }
    Some(Binding {
        clips: [left, right],
        fact: SourcePcmPrefix {
            frames,
            tail_package: tail_a,
        },
    })
}
