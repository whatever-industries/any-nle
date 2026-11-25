//! Prepare reusable Avid subclips from verified direct and original-file links.
use super::*;
use prproj::convert::prepared::{av_subclip_edit_fits, PreparedAvSubclip, PreparedAvSubclipUse, PreparedPictureSubclip};

const RATE: &str = "24000/1001";
const EXACT_24: &str = "24";
const RATE_2997: &str = "30000/1001";

pub(super) fn field_int(object: &Value, field: &str) -> Option<i64> {
    key(object, field).and_then(integer)
}

pub(super) fn mob_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

// The bin receipt uses full uppercase hex. The edit's route uses the same
// 32 bytes in SMPTE URN form.
pub(super) fn urn(mob: &str) -> String {
    let groups = (0..8).map(|i| mob[i * 8..i * 8 + 8].to_ascii_lowercase()).collect::<Vec<_>>();
    format!("urn:smpte:umid:{}", groups.join("."))
}

fn track_matches(track: &Value, rate: &str, kind: i64, label: i64, backing: i64, target: &str) -> bool {
    track_matches_at(track, rate, (kind, label), (0, backing), target)
}

fn track_matches_at(track: &Value, rate: &str, (kind, label): (i64, i64), (offset, length): (i64, i64), target: &str) -> bool {
    key(track, "component_class").and_then(str_value) == Some("SCLP")
        && key(track, "rate").and_then(str_value) == Some(rate)
        && field_int(track, "kind") == Some(kind)
        && field_int(track, "label") == Some(label)
        && field_int(track, "target_track") == Some(label)
        && field_int(track, "target_offset") == Some(offset)
        && field_int(track, "length") == Some(length)
        && key(track, "target_mob").and_then(str_value) == Some(target)
}

enum Subclip {
    Picture(PreparedPictureSubclip),
    Av(PreparedAvSubclip, Option<LinkedBacking>),
    AutoSync(Sequence, bool),
}

#[derive(Clone, Copy)]
struct LinkedBacking {
    offset: i64,
    length: i64,
}

#[derive(Clone, Copy)]
struct AvRoute {
    index: usize,
    linked: Option<LinkedBacking>,
}

pub(super) fn prepare(
    graph: &SourceGraph,
    media: &HashMap<&str, &SourceMedia>,
    sidecars: &Sidecars<'_>,
    input: &mut PreparedProjectInput,
    ownership: &HashMap<String, (usize, usize)>,
    typed: bool,
    clock: ClockProfile,
    losses: &mut Vec<Loss>,
) -> R<Vec<(usize, Sequence)>> {
    if !graph.bins.iter().any(|bin| side(sidecars, "avid.subclips", &bin.id).is_some()) {
        return Ok(Vec::new());
    }
    let mut parents = HashMap::new();
    let mut items_by_id = HashMap::new();
    for item in &graph.items {
        items_by_id.insert(item.id.as_str(), item);
        if let Some(mob) = item.umid.as_deref() {
            parents.entry(mob).and_modify(|entry| *entry = None).or_insert(Some(item));
        }
    }
    let clips: HashMap<_, _> = input.clips.iter().map(|clip| (clip.id(), clip)).collect();
    let mut autosyncs = Vec::new();
    let mut av_routes = HashMap::new();
    let mut seen = HashSet::new();
    let mut native_ids = HashSet::new();
    let mut duplicate_native_ids = HashSet::new();
    for (bin, owner) in graph.bins.iter().enumerate() {
        let Some(receipt) = side(sidecars, "avid.subclips", &owner.id) else { continue };
        let Value::Arr(rows) = receipt else { return Err(error("invalid Avid subclip receipt")) };
        let mut bin_parents: HashMap<_, Option<&SourceItem>> = HashMap::new();
        for item_id in &owner.items {
            let Some(item) = items_by_id.get(item_id.as_str()).copied() else { continue };
            if let Some(mob) = item.umid.as_deref() {
                bin_parents
                    .entry(mob)
                    .and_modify(|entry| {
                        if entry.is_some_and(|existing| existing.id != item.id) {
                            *entry = None;
                        }
                    })
                    .or_insert(Some(item));
            }
        }
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
                if clock == ClockProfile::LegacyImport
                    && matches!(key(row, "rate").and_then(str_value), Some(EXACT_24 | RATE))
                {
                    if let Some(Value::Arr(tracks)) = key(row, "tracks") {
                        let target = |kind| tracks.iter().find(|t| field_int(t, "kind") == Some(kind))
                            .and_then(|t| key(t, "target_mob")).and_then(str_value);
                        if target(1).zip(target(2)).is_some_and(|(picture, sound)| picture != sound) {
                            if autosyncs.len() >= 10_000usize.saturating_sub(input.sequences.len()) {
                                return Err("AutoSync source sequence limit reached");
                            }
                            let (sequence, has_clock) = super::autosync::prepare(
                                row, name, tracks, &parents, media, &clips, sidecars,
                            )?;
                            return Ok((native_id, Subclip::AutoSync(sequence, has_clock)));
                        }
                    }
                }
                let (Some(Value::Arr(tracks)), Some(rate @ (RATE | EXACT_24 | RATE_2997))) = (
                    key(row, "tracks"), key(row, "rate").and_then(str_value),
                ) else { return Err("reusable subclips require a saved supported picture rate (23.976, 24, or 29.97 DF)") };
                let track = tracks.iter().find(|t| field_int(t, "kind") == Some(1))
                    .ok_or("reusable subclips require picture")?;
                let target = key(track, "target_mob").and_then(str_value)
                    .ok_or("backing master identity is missing")?;
                if target == native_id {
                    return Err("backing master refers to the subclip itself");
                }
                // A bin can retain its own copy of a native master and its markers.
                // Prefer that occurrence when other selected bins also carry the mob.
                let parent = bin_parents.get(target).or_else(|| parents.get(target)).copied()
                    .ok_or("backing master is unavailable in the selected bins")?
                    .ok_or("more than one selected master has the backing identity")?;
                let parent_clip = clip_id(&parent.id);
                let prepared = clips.get(parent_clip.as_str()).copied();
                if let Some(PreparedClip::Original(original)) = prepared {
                    let (subclip, linked) = original_range(row, tracks, parent, original, media, bin, target, rate)?;
                    return Ok((native_id, Subclip::Av(subclip, Some(linked))));
                }
                let Some(PreparedClip::Legacy(master)) = prepared else {
                    return Err("backing master is not a prepared managed-media clip");
                };
                let video = master.video.as_ref()
                    .ok_or("backing master has no prepared picture")?;
                if rate == EXACT_24 {
                    if clock != ClockProfile::LegacyImport
                        || video.rate != model::Fraction::new(24, 1)
                        || video.start != 0
                        || !matches!(direct_opatom_clocks(parent, media, sidecars, true, true, false), Some((0, 0, _, _)))
                    {
                        return Err("24 fps A/V subclips require verified zero-origin DNxHD LB with two mono PCM16 siblings");
                    }
                } else if rate == RATE_2997 {
                    if video.rate != model::Fraction::new(30_000, 1001)
                        || master.time_display != Some(102)
                        || input.direct_dnxhr_origins.get(&parent_clip) != Some(&video.start)
                    {
                        return Err("29.97 A/V subclips require verified drop-frame DNxHD LB with two mono PCM24 siblings");
                    }
                } else if video.rate != model::Fraction::new(24_000, 1001)
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
                if !track_matches(track, rate, 1, 1, backing, target) {
                    return Err("reusable subclips require a direct, same-rate V1 reference with zero backing offset");
                }
                if backing.checked_mul(unit).is_none_or(|length| length > video.duration) {
                    return Err("saved backing length disagrees with its track or exceeds the linked picture");
                }
                if rate == RATE && tracks.len() == 1 && master.audio.iter().all(|a| a.bits == 24) {
                    return Ok((native_id, Subclip::Picture(PreparedPictureSubclip {
                        bin, parent_clip, name: name.into(), start: start * unit, end: end * unit,
                    })));
                }
                if tracks.len() != 3 || ![1, 2].iter().all(|&label| {
                    tracks.iter().any(|t| track_matches(t, rate, 2, label, backing, target))
                }) {
                    return Err("A/V subclips require direct V1/A1/A2 references to one master with zero backing offsets");
                }
                let uniform_pcm = matches!(master.audio.as_slice(), [a, b]
                    if matches!(a.bits, 16 | 24) && a.bits == b.bits);
                if !parent.media.iter().all(|reference|
                    media.get(reference.media.as_str()).is_some_and(|m| m.online == Some(true)))
                    || !uniform_pcm
                    || backing * unit != video.duration
                {
                    return Err("A/V subclips require an online managed DNx master with two independent mono lanes of the same PCM16 or PCM24 depth and matching backing length");
                }
                let subclip = PreparedAvSubclip {
                    bin, parent_clip, name: name.into(), start: start * unit, end: end * unit,
                };
                subclip.source_marker(master)?;
                Ok((native_id, Subclip::Av(subclip, None)))
            };
            match admit() {
                Ok((_, Subclip::Picture(subclip))) => input.picture_subclips.push(subclip),
                Ok((mob, Subclip::Av(subclip, linked))) => {
                    av_routes.insert(urn(mob), Some(AvRoute { index: input.av_subclips.len(), linked }));
                    input.av_subclips.push(subclip);
                }
                Ok((mob, Subclip::AutoSync(sequence, has_clock))) => autosyncs.push((urn(mob), bin, sequence, has_clock, id)),
                Err(reason) => loss(losses, id, "subclip_bin_item", format!("{name:?}: {reason}")),
            }
        }
    }
    for id in &duplicate_native_ids {
        if let Some(route) = av_routes.get_mut(id) {
            *route = None;
        }
    }
    input.av_subclip_uses = bind_av_uses(graph, sidecars, ownership, &clips, &av_routes, input, losses);
    let bound_edits: HashSet<_> = input.av_subclip_uses.iter()
        .map(|u| (u.sequence, u.event)).collect();
    for (si, (sequence, source)) in input.sequences.iter_mut().zip(&graph.sequences).enumerate() {
        for marker in &mut sequence.markers {
            let Some(event) = marker.event.and_then(|i| usize::try_from(i).ok()) else { continue };
            if marker.owner != "edit" || marker.inherited || !bound_edits.contains(&(si, event)) {
                continue;
            }
            loss(losses, &source.id, "marker_ownership", format!(
                "marker {:?} on {} at {} ticks moved from the A/V subclip edit to the sequence",
                marker.name, marker.track.as_deref().unwrap_or("unknown track"), marker.start,
            ));
            marker.owner = "sequence".into();
            marker.event = None;
            marker.source = None;
        }
    }
    Ok(autosyncs.into_iter().filter_map(|(mob, bin, sequence, has_clock, id)| {
        if duplicate_native_ids.contains(&mob) {
            loss(losses, id, "subclip_bin_item", "AutoSync native identity is ambiguous");
            return None;
        }
        if !has_clock {
            loss(losses, id, "subclip_timecode", "AutoSync source sequence uses a zero display start because the picture clock is unqualified");
        }
        Some((bin, sequence))
    }).collect())
}

fn original_range(
    row: &Value, tracks: &[Value], parent: &SourceItem, original: &OriginalClip,
    media: &HashMap<&str, &SourceMedia>, bin: usize, target: &str, rate: &str,
) -> Result<(PreparedAvSubclip, LinkedBacking), &'static str> {
    let unsupported = "linked A/V subclips require verified same-rate ProRes HQ and two ordered PCM24 mono selections";
    let Some(video) = &original.video else { return Err(unsupported) };
    let frame = model::ticks_per_unit(video.rate);
    let backing = field_int(row, "backing_length").ok_or("backing length is unresolved")?;
    let bound = |name| key(row, name).and_then(|value| field_int(value, "value"));
    let (start, end) = bound("start").zip(bound("end"))
        .ok_or("saved In and Out are unresolved")?;
    let offset = tracks.iter().find(|t| field_int(t, "kind") == Some(1))
        .and_then(|t| field_int(t, "target_offset")).filter(|&n| n >= 0)
        .ok_or("linked subclip picture has no verified backing offset")?;
    if !(0 <= start && start < end && end <= backing) || tracks.len() != 3
        || ![(1, 1), (2, 1), (2, 2)].into_iter().all(|lane| {
            tracks.iter().any(|t| track_matches_at(t, rate, lane, (offset, backing), target))
        })
    {
        return Err("linked A/V subclip requires direct V1/A1/A2 references with a common backing offset and range");
    }
    let file_ticks = |frames| offset.checked_add(frames).and_then(|value| value.checked_mul(frame))
        .ok_or("linked subclip backing range overflows");
    let subclip = PreparedAvSubclip {
        bin, parent_clip: original.metadata.id.clone(),
        name: key(row, "name").and_then(str_value).unwrap_or_default().into(),
        start: file_ticks(start)?, end: file_ticks(end)?,
    };
    let saved_clock_matches = parent.media.iter().filter_map(|r| media.get(r.media.as_str()))
        .filter(|m| m.kind == StreamKind::Video).any(|m| {
            m.clock.as_ref().is_some_and(|clock| !clock.drop && m.rate == Some(clock.rate)
                && clock.exact_start().is_some_and(|start| {
                    m.start == Some(start) && ticks(start, "linked subclip source clock").ok() == Some(video.source_start)
                }))
        });
    if rate != RATE || parent.media.len() != 3 || !saved_clock_matches
        || !parent.media.iter().all(|r| {
            r.source_offset == Some(Rational::ZERO)
                && media.get(r.media.as_str()).is_some_and(|m| m.online == Some(true))
        })
        || file_ticks(backing)? > video.duration
        || !subclip.fits_original(original)
    {
        return Err(unsupported);
    }
    Ok((subclip, LinkedBacking { offset, length: backing }))
}

fn bind_av_uses(
    graph: &SourceGraph,
    sidecars: &Sidecars<'_>,
    ownership: &HashMap<String, (usize, usize)>,
    clips: &HashMap<&str, &PreparedClip>,
    routes: &HashMap<String, Option<AvRoute>>,
    input: &PreparedProjectInput,
    losses: &mut Vec<Loss>,
) -> Vec<PreparedAvSubclipUse> {
    if routes.is_empty() {
        return Vec::new();
    }
    let multicam: HashSet<_> = input.multicam_uses.iter().map(|u| (u.sequence, u.event)).collect();
    let mut marked: HashMap<_, Vec<_>> = HashMap::new();
    for (si, sequence) in input.sequences.iter().enumerate() {
        for marker in sequence.markers.iter().filter(|m| m.owner == "edit" && !m.inherited) {
            if let Some(event) = marker.event.and_then(|event| usize::try_from(event).ok()) {
                marked.entry((si, event)).or_default().push(marker.source);
            }
        }
    }
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
            let AvRoute { index: subclip, linked } = (*subclip)?;
            let [first, second] = path.as_slice() else { return None };
            let &(sequence, event) = ownership.get(&p.id)?;
            let seq = &input.sequences[sequence];
            let edit = &seq.events[event];
            let definition = &input.av_subclips[subclip];
            let rate = match (seq.rate.num, seq.rate.den) {
                (24, 1) => EXACT_24,
                (24_000, 1001) => RATE,
                (30_000, 1001) => RATE_2997,
                _ => return None,
            };
            if multicam.contains(&(sequence, event))
                || clip_id(&p.item) != definition.parent_clip
                || key(second, "mob").and_then(str_value) != Some(parent_urns.get(p.item.as_str())?.as_str())
                || [first, second].iter().any(|h| key(h, "rate").and_then(str_value) != Some(rate))
            {
                return None;
            }
            let unit = model::ticks_per_unit(seq.rate);
            let (audio_track, label) = match (p.video_track, p.audio_tracks.as_slice(), p.audio_streams.as_slice()) {
                (Some(v), [], []) if edit.video_track == Some(v) && field_int(receipt, "native_kind") == Some(1) => (None, 1),
                (None, [a], [label]) if field_int(receipt, "native_kind") == Some(2) => (Some(*a), *label),
                _ => return None,
            };
            if field_int(first, "track") != Some(label) || field_int(second, "track") != Some(label) {
                return None;
            }
            let slot = match audio_track {
                Some(a) => Some(edit.audio_tracks.iter().position(|&track| track == a)?),
                None => None,
            };
            let (offset, backing_length) = match (clips.get(definition.parent_clip.as_str()).copied()?, linked) {
                (PreparedClip::Legacy(parent), None) => {
                    if !av_subclip_edit_fits(seq, edit, parent, definition)
                        || slot.is_some_and(|index| model::event_audio_files(parent, edit).ok()
                            .and_then(|files| files.get(index)?.avid_track) != Some(label))
                    {
                        return None;
                    }
                    // A shared source marker or unresolved edit marker keeps the regular source path.
                    if let (Some(edit_markers), Some(source)) = (
                        marked.get(&(sequence, event)), definition.source_marker(parent).ok()?,
                    ) {
                        if edit_markers.iter().any(|start| start.is_none() || *start == Some(source.start)) {
                            return None;
                        }
                    }
                    (0, parent.video.as_ref()?.duration / unit)
                }
                (PreparedClip::Original(parent), Some(backing)) => {
                    let PreparedClip::Original(placed) = clips.get(edit.clip.as_str()).copied()? else { return None };
                    if !definition.fits_original_edit(seq, edit, parent, placed)
                        || slot.is_some_and(|index| !matches!(label, 1 | 2)
                            || edit.audio_channels.get(index).copied() != Some(label))
                    {
                        return None;
                    }
                    (backing.offset, backing.length)
                }
                _ => return None,
            };
            let saved = |v, field| key(v, field).and_then(|v| field_int(v, "value"));
            // The first hop is subclip-relative. The second must describe the
            // same backing component that established the reusable bin range.
            let local_in = saved(first, "source_offset").filter(|&in_point| in_point >= 0)?;
            if local_in.checked_add(offset) != Some(edit.source_in / unit)
                || saved(first, "length") != Some(edit.duration / unit)
                || saved(second, "source_offset") != Some(offset)
                || saved(second, "length") != Some(backing_length)
            {
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
    uses.retain(|(u, id)| {
        if ambiguous.contains(&(u.sequence, u.event, u.audio_track)) {
            loss(losses, id, "subclip_timeline_source", UNBOUND);
            return false;
        }
        true
    });
    // Moving a marker off the edit is useful only when all its lanes can keep
    // referencing the same reusable subclip. A partial binding keeps the normal marker path.
    let mut marked_bindings = HashMap::new();
    for (u, _) in &uses {
        let owner = (u.sequence, u.event);
        if marked.contains_key(&owner) {
            let (subclip, count, consistent) = marked_bindings.entry(owner).or_insert((u.subclip, 0, true));
            *count += 1;
            *consistent &= *subclip == u.subclip;
        }
    }
    uses.into_iter().filter_map(|(u, id)| {
        let owner = (u.sequence, u.event);
        if let Some(&(_, count, consistent)) = marked_bindings.get(&owner) {
            let event = &input.sequences[u.sequence].events[u.event];
            if !consistent || count != usize::from(event.video_track.is_some()) + event.audio_tracks.len() {
                loss(losses, id, "subclip_timeline_source", UNBOUND);
                return None;
            }
        }
        Some(u)
    }).collect()
}
