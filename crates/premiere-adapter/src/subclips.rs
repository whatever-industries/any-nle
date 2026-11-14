//! Prepare reusable subclips on the verified direct MXF route.
use super::*;
use prproj::convert::prepared::{av_subclip_edit_fits, PreparedAvSubclip, PreparedAvSubclipUse, PreparedPictureSubclip};

const RATE: &str = "24000/1001";

fn field_int(object: &Value, field: &str) -> Option<i64> {
    key(object, field).and_then(integer)
}

fn mob_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// The bin receipt uses full uppercase hex. The edit's route uses the same
// 32 bytes in SMPTE URN form.
fn urn(mob: &str) -> String {
    let groups = (0..8).map(|i| mob[i * 8..i * 8 + 8].to_ascii_lowercase()).collect::<Vec<_>>();
    format!("urn:smpte:umid:{}", groups.join("."))
}

fn track_matches(track: &Value, kind: i64, label: i64, backing: i64, target: &str) -> bool {
    key(track, "component_class").and_then(str_value) == Some("SCLP")
        && key(track, "rate").and_then(str_value) == Some(RATE)
        && field_int(track, "kind") == Some(kind)
        && field_int(track, "label") == Some(label)
        && field_int(track, "target_track") == Some(label)
        && field_int(track, "target_offset") == Some(0)
        && field_int(track, "length") == Some(backing)
        && key(track, "target_mob").and_then(str_value) == Some(target)
}

enum Subclip {
    Picture(PreparedPictureSubclip),
    Av(PreparedAvSubclip),
}

pub(super) fn prepare(
    graph: &SourceGraph,
    sidecars: &Sidecars<'_>,
    input: &mut PreparedProjectInput,
    ownership: &HashMap<String, (usize, usize)>,
    typed: bool,
    losses: &mut Vec<Loss>,
) -> R<()> {
    if !graph.bins.iter().any(|bin| side(sidecars, "avid.subclips", &bin.id).is_some()) {
        return Ok(());
    }
    let mut parents = HashMap::new();
    for item in &graph.items {
        if let Some(mob) = item.umid.as_deref() {
            parents.entry(mob).and_modify(|entry| *entry = None).or_insert(Some(item.id.as_str()));
        }
    }
    let masters: HashMap<_, _> = input.clips.iter().filter_map(|clip| match clip {
        PreparedClip::Legacy(clip) => Some((clip.id.as_str(), clip)),
        _ => None,
    }).collect();
    let online_items: HashSet<_> = {
        let online_media: HashSet<_> = graph.media.iter().filter(|m| m.online == Some(true))
            .map(|m| m.id.as_str()).collect();
        graph.items.iter().filter(|i| i.media.iter().all(|m| online_media.contains(m.media.as_str())))
            .map(|i| i.id.as_str()).collect()
    };
    let mut av_routes = HashMap::new();
    let mut seen = HashSet::new();
    let mut native_ids = HashSet::new();
    let mut duplicate_native_ids = HashSet::new();
    for (bin, owner) in graph.bins.iter().enumerate() {
        let Some(receipt) = side(sidecars, "avid.subclips", &owner.id) else { continue };
        let Value::Arr(rows) = receipt else { return Err(error("invalid Avid subclip receipt")) };
        for row in rows {
            let id = key(row, "id").and_then(str_value).filter(|id| !id.is_empty())
                .ok_or_else(|| error("Avid subclip receipt lacks occurrence identity"))?;
            if !seen.insert(id) || seen.len() > model::MAX_ITEMS {
                return Err(error("duplicate or excessive Avid subclip occurrences"));
            }
            if let Some(native_id) = key(row, "mob").and_then(str_value).filter(|value| mob_id(value)) {
                let id = urn(native_id);
                if !native_ids.insert(id.clone()) {
                    duplicate_native_ids.insert(id);
                }
            }
            let name = key(row, "name").and_then(str_value).unwrap_or("");
            let admit = || -> Result<(&str, Subclip), &'static str> {
                if !typed || input.options.bin_mode != "separate" {
                    return Err("use direct linking in Separate files mode for reusable subclips");
                }
                let native_id = key(row, "mob").and_then(str_value)
                    .ok_or("saved subclip identity is missing")?;
                if name.is_empty() || name.len() > 4096 || !mob_id(native_id) {
                    return Err("saved subclip name or identity is invalid");
                }
                let (Some(Value::Arr(tracks)), Some(RATE)) = (
                    key(row, "tracks"), key(row, "rate").and_then(str_value),
                ) else { return Err("reusable subclips require a saved 23.976 picture track") };
                let track = tracks.iter().find(|t| field_int(t, "kind") == Some(1))
                    .ok_or("reusable subclips require picture")?;
                let target = key(track, "target_mob").and_then(str_value)
                    .ok_or("backing master identity is missing")?;
                if target == native_id {
                    return Err("backing master refers to the subclip itself");
                }
                let parent = parents.get(target).copied()
                    .ok_or("backing master is unavailable in the selected bins")?
                    .ok_or("more than one selected master has the backing identity")?;
                let parent_clip = clip_id(parent);
                let master = masters.get(parent_clip.as_str())
                    .ok_or("backing master is not a prepared managed-media clip")?;
                let video = master.video.as_ref()
                    .ok_or("backing master has no prepared picture")?;
                if video.rate != model::Fraction::new(24_000, 1001)
                    || !input.direct_dnxhr_origins.contains_key(&parent_clip)
                {
                    return Err("backing picture requires a verified 23.976 DNxHR LB link");
                }
                let start = key(row, "start").and_then(|value| field_int(value, "value"))
                    .ok_or("saved subclip In is unresolved")?;
                let end = key(row, "end").and_then(|value| field_int(value, "value"))
                    .ok_or("saved subclip Out is unresolved")?;
                let backing = field_int(row, "backing_length").ok_or("backing length is unresolved")?;
                let unit = model::ticks_per_unit(video.rate);
                if !(0 <= start && start < end && end <= backing) {
                    return Err("saved subclip In and Out fall outside the usable backing range");
                }
                if !track_matches(track, 1, 1, backing, target) {
                    return Err("reusable subclips require a direct, same-rate V1 reference with zero backing offset");
                }
                if backing.checked_mul(unit).is_none_or(|length| length > video.duration) {
                    return Err("saved backing length disagrees with its track or exceeds the linked picture");
                }
                if tracks.len() == 1 && master.audio.iter().all(|a| a.bits == 24) {
                    return Ok((native_id, Subclip::Picture(PreparedPictureSubclip {
                        bin, parent_clip, name: name.into(), start: start * unit, end: end * unit,
                    })));
                }
                if tracks.len() != 3 || ![1, 2].iter().all(|&label| {
                    tracks.iter().any(|t| track_matches(t, 2, label, backing, target))
                }) {
                    return Err("A/V subclips require direct V1/A1/A2 references to one master with zero backing offsets");
                }
                let uniform_pcm = matches!(master.audio.as_slice(), [a, b]
                    if matches!(a.bits, 16 | 24) && a.bits == b.bits);
                if !online_items.contains(parent) || !uniform_pcm
                    || backing * unit != video.duration
                    || !master.markers.is_empty()
                {
                    return Err("A/V subclips require an online 23.976 managed DNx master with two independent mono lanes of the same PCM16 or PCM24 depth, matching backing length and no source markers");
                }
                Ok((native_id, Subclip::Av(PreparedAvSubclip {
                    bin, parent_clip, name: name.into(), start: start * unit, end: end * unit,
                })))
            };
            match admit() {
                Ok((_, Subclip::Picture(subclip))) => input.picture_subclips.push(subclip),
                Ok((mob, Subclip::Av(subclip))) => {
                    av_routes.insert(urn(mob), Some(input.av_subclips.len()));
                    input.av_subclips.push(subclip);
                }
                Err(reason) => loss(losses, id, "subclip_bin_item", format!("{name:?}: {reason}")),
            }
        }
    }
    for id in duplicate_native_ids {
        if let Some(route) = av_routes.get_mut(&id) {
            *route = None;
        }
    }
    input.av_subclip_uses = bind_av_uses(graph, sidecars, ownership, &masters, &av_routes, input, losses);
    Ok(())
}

fn bind_av_uses(
    graph: &SourceGraph,
    sidecars: &Sidecars<'_>,
    ownership: &HashMap<String, (usize, usize)>,
    masters: &HashMap<&str, &Clip>,
    routes: &HashMap<String, Option<usize>>,
    input: &PreparedProjectInput,
    losses: &mut Vec<Loss>,
) -> Vec<PreparedAvSubclipUse> {
    if routes.is_empty() {
        return Vec::new();
    }
    let multicam: HashSet<_> = input.multicam_uses.iter().map(|u| (u.sequence, u.event)).collect();
    let marked: HashSet<_> = input.sequences.iter().enumerate().flat_map(|(si, s)| {
        s.markers.iter().filter(|m| m.owner == "edit" && !m.inherited)
            .filter_map(move |m| Some((si, usize::try_from(m.event?).ok()?)))
    }).collect();
    let mut parent_urns = HashMap::new();
    for item in &graph.items {
        if let Some(mob) = item.umid.as_deref().filter(|s| mob_id(s)) {
            parent_urns.insert(item.id.as_str(), urn(mob));
        }
    }
    let mut uses = Vec::new();
    let mut seen = HashSet::new();
    let mut ambiguous = HashSet::new();
    const UNBOUND: &str = "A/V subclip edit could not be bound uniquely within the qualified profile";
    for p in graph.sequences.iter().flat_map(|s| &s.placements) {
        let Some(receipt) = side(sidecars, "avid.source", &p.id) else { continue };
        let Some(Value::Arr(path)) = key(receipt, "route") else { continue };
        let Some(head) = path.first() else { continue };
        let Some(mob) = key(head, "mob").and_then(str_value) else { continue };
        let Some(subclip) = routes.get(mob) else { continue };
        let bind = || -> Option<PreparedAvSubclipUse> {
            let subclip = (*subclip)?;
            let [first, second] = path.as_slice() else { return None };
            let &(sequence, event) = ownership.get(&p.id)?;
            let seq = &input.sequences[sequence];
            let edit = &seq.events[event];
            let definition = &input.av_subclips[subclip];
            let parent = masters[definition.parent_clip.as_str()];
            if multicam.contains(&(sequence, event)) || marked.contains(&(sequence, event))
                || !av_subclip_edit_fits(seq, edit, parent, definition)
                || key(second, "mob").and_then(str_value) != Some(parent_urns.get(p.item.as_str())?.as_str())
                || [first, second].iter().any(|h| key(h, "rate").and_then(str_value) != Some(RATE))
            {
                return None;
            }
            let unit = model::ticks_per_unit(seq.rate);
            let saved = |v, field| key(v, field).and_then(|v| field_int(v, "value"));
            if saved(first, "source_offset") != Some(edit.source_in / unit)
                || saved(first, "length") != Some(edit.duration / unit)
                || saved(second, "source_offset") != Some(0)
                || saved(second, "length") != Some(parent.video.as_ref()?.duration / unit)
            {
                return None;
            }
            let (audio_track, label) = match (p.video_track, p.audio_tracks.as_slice(), p.audio_streams.as_slice()) {
                (Some(v), [], []) if edit.video_track == Some(v) && field_int(receipt, "native_kind") == Some(1) => (None, 1),
                (None, [a], [label]) if field_int(receipt, "native_kind") == Some(2) => {
                    let index = edit.audio_tracks.iter().position(|t| t == a)?;
                    let files = model::event_audio_files(parent, edit).ok()?;
                    if files.get(index)?.avid_track != Some(*label) { return None; }
                    (Some(*a), *label)
                }
                _ => return None,
            };
            if field_int(first, "track") != Some(label) || field_int(second, "track") != Some(label) {
                return None;
            }
            Some(PreparedAvSubclipUse { sequence, event, audio_track, subclip })
        };
        match bind() {
            Some(u) if seen.insert((u.sequence, u.event, u.audio_track)) => uses.push((u, &p.id)),
            Some(u) => {
                ambiguous.insert((u.sequence, u.event, u.audio_track));
                loss(losses, &p.id, "subclip_timeline_source", UNBOUND);
            }
            None => loss(losses, &p.id, "subclip_timeline_source", UNBOUND),
        }
    }
    uses.into_iter().filter_map(|(u, id)| {
        if !ambiguous.contains(&(u.sequence, u.event, u.audio_track)) {
            Some(u)
        } else {
            loss(losses, id, "subclip_timeline_source", UNBOUND);
            None
        }
    }).collect()
}
