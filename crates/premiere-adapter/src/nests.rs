//! Preserve the editable boundary of a proven inline Avid Submaster.
use super::*;
use prproj::convert::prepared::{plain_picture_edit, PreparedPictureNest, PICTURE_NEST_24_RATE, PICTURE_NEST_RATES};
use prproj::convert::prepared::{PreparedLayeredSubmaster, PreparedPictureLayer};

pub const LAYERED_LOSS: &str = "layered_submaster";

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    key(value, field).and_then(str_value)
}

fn is_layered_profile(issue: &Value) -> bool {
    key(issue, "effect_id").and_then(str_value) == Some("EFF_SUBMASTER")
        && key(issue, "picture_profile").and_then(str_value) == Some("plain_layered_submaster")
}

pub fn has_layered_candidates(graph: &SourceGraph) -> bool {
    graph.sidecars.iter().any(|s| s.adapter == "avid.source"
        && matches!(key(&s.data, "unresolved"), Some(Value::Arr(issues)) if issues.iter().any(is_layered_profile)))
}

pub(super) struct LayeredCandidate<'a> {
    sequence_id: &'a str,
    loss_reason: String,
    parent_start: i64,
    pub draft: PreparedLayeredSubmaster,
}

// The unresolved list can describe effects whose spans have no portable
// placement. Check those spans before filling an apparent gap on the track.
fn picture_issue_conflicts(issues: &[Value], rate: &str) -> Option<HashSet<usize>> {
    use subclips::field_int as num;
    let mut spans = Vec::new();
    let mut unknown_tracks = HashSet::new();
    let mut blocked = HashSet::new();
    let mut owners = HashMap::new();
    for (index, issue) in issues.iter().enumerate() {
        if num(issue, "kind")? != 1 {
            continue;
        }
        let track = num(issue, "track").filter(|n| *n > 0)?;
        if let Some(owner) = text(issue, "owner") {
            if let Some(previous) = owners.insert(owner, index) {
                blocked.extend([previous, index]);
            }
        }
        let span = (|| {
            if text(issue, "record_rate")? != rate {
                return None;
            }
            let start = num(key(issue, "record_start")?, "value").filter(|n| *n >= 0)?;
            let length = num(issue, "record_length").filter(|n| *n > 0)?;
            Some((track, start, start.checked_add(length)?, index))
        })();
        if let Some(span) = span {
            spans.push(span);
        } else {
            unknown_tracks.insert(track);
        }
    }
    spans.sort_unstable();
    let mut previous_track = None;
    let mut previous_end = 0;
    for (position, &(track, start, end, index)) in spans.iter().enumerate() {
        if previous_track != Some(track) {
            previous_track = Some(track);
            previous_end = 0;
        }
        if unknown_tracks.contains(&track) || start < previous_end
            || spans.get(position + 1).is_some_and(|&(next_track, next_start, _, _)| {
                next_track == track && next_start < end
            })
        {
            blocked.insert(index);
        }
        previous_end = previous_end.max(end);
    }
    Some(blocked)
}

fn record_span_conflicts(
    nest: &PreparedLayeredSubmaster, track: i64, start: Option<Rational>, duration: Option<Rational>,
) -> bool {
    track == nest.track && start.zip(duration)
        .and_then(|(start, duration)| Some((start.to_units(TICKS_PER_SECOND)?, duration.to_units(TICKS_PER_SECOND)?)))
        .is_none_or(|(start, duration)| nest.overlaps(track, start, duration.max(1)))
}

fn neighboring_picture_conflicts(seq: &SourceSequence, nest: &PreparedLayeredSubmaster) -> bool {
    seq.placements.iter().chain(&seq.disabled_placements).any(|edit| {
        edit.video_track.is_some_and(|track| {
            record_span_conflicts(nest, track, Some(edit.record_start), Some(edit.duration))
        })
    }) || seq.transitions.iter().any(|transition| {
        transition.kind == StreamKind::Video
            && record_span_conflicts(nest, transition.track, transition.cut.checked_sub(transition.cut_offset),
                Some(transition.duration))
    }) || seq.texts.iter().any(|text| {
        text.kind == graph::source::TextKind::Graphic
            && record_span_conflicts(nest, text.track, text.record_start, text.duration)
    })
}

// An exact outer marker with no portable edit binding falls back to the parent
// sequence. Its range does not own or occupy the picture on a neighboring nest.
fn marker_falls_back_to_parent(
    seq: &SourceSequence, marker: &SourceMarker, nest: &PreparedLayeredSubmaster,
) -> bool {
    if marker.owner != MarkerOwner::Edit
        || marker.track != Some(TrackRef { kind: StreamKind::Video, index: nest.track })
        || marker.owner_rate != Some(seq.rate)
        || !marker.binding.placements.is_empty()
        || marker.source.is_some()
    {
        return false;
    }
    marker.start.zip(marker.owner_local).and_then(|(start, local)| {
        let start = start.to_units(TICKS_PER_SECOND)?;
        let local = local.to_units(TICKS_PER_SECOND)?;
        let duration = marker.duration.to_units(TICKS_PER_SECOND)?;
        let end = nest.start.checked_add(nest.duration)?;
        start.checked_add(duration)?;
        Some(start >= nest.start && start < end && start - nest.start == local)
    }) == Some(true)
}

pub(super) fn layered_candidates<'a>(
    graph: &'a SourceGraph, sidecars: &Sidecars<'a>, media: &HashMap<&str, &SourceMedia>,
) -> Vec<LayeredCandidate<'a>> {
    if !has_layered_candidates(graph) {
        return Vec::new();
    }
    let mut masters = HashMap::new();
    let mut source_items = HashMap::new();
    for item in &graph.items {
        if let Some(mob) = item.umid.as_deref().filter(|m| subclips::mob_id(m)) {
            masters.entry(subclips::urn(mob)).and_modify(|slot| *slot = None).or_insert(Some(item));
            source_items.insert(item.id.as_str(), (mob, item));
        }
    }
    use subclips::field_int as num;
    let layer = |value: &Value, length: i64, rate, rate_text: &str| -> Option<PreparedPictureLayer> {
        let unit = model::ticks_per_unit(rate);
        let exact_24 = rate == PICTURE_NEST_24_RATE;
        let source = key(value, "picture_source")?;
        if num(source, "track")? != 1 || text(source, "rate")? != rate_text
            || num(source, "local_length")? != length
        {
            return None;
        }
        let mob = text(source, "mob")?;
        let item = match key(source, "item") {
            // Older graph/2 receipts can only resolve an unambiguous global master.
            None => (*masters.get(mob)?)?,
            Some(Value::Str(id)) => {
                let (saved_mob, item) = source_items.get(id.as_str())?;
                if subclips::urn(saved_mob) != mob {
                    return None;
                }
                *item
            }
            Some(_) => return None,
        };
        if item.rate.map(fraction) != Some(rate) || !item.markers.is_empty()
            || item.media.iter().any(|m| media[m.media.as_str()].online != Some(true))
            || side(sidecars, "avid.source", &item.id).and_then(|raw| key(raw, "input_color_transforms")).is_some()
            || !direct_opatom_clocks(item, media, sidecars, true, exact_24, true)
                .is_some_and(|(start, origin, _, _)| !exact_24 || (start == 0 && origin == 0))
        {
            return None;
        }
        let local = num(source, "local_start")?;
        let source_in = num(source, "source_start")?;
        let frames = num(source, "length")?;
        let video = item.media.iter().map(|p| media[p.media.as_str()]).find(|m| m.kind == StreamKind::Video)?;
        if local < 0 || source_in < 0 || frames <= 0 || local.checked_add(frames)? > length
            || source_in.checked_add(frames)?.checked_mul(unit)? > video.duration.to_units(TICKS_PER_SECOND)?
        {
            return None;
        }
        Some(PreparedPictureLayer {
            clip: clip_id(&item.id),
            start: local.checked_mul(unit)?,
            source_in: source_in.checked_mul(unit)?,
            duration: frames.checked_mul(unit)?,
        })
    };
    let sequences = graph.sequences.iter().enumerate().filter_map(|(sequence, seq)| {
        let rate = fraction(seq.rate);
        if !PICTURE_NEST_RATES.contains(&rate) {
            return None;
        }
        let rate_text = if rate == PICTURE_NEST_24_RATE { "24" } else { "24000/1001" };
        let receipt = side(sidecars, "avid.source", &seq.id)?;
        let parent_start = direct_avid_tc1(seq, 24, false, 24 * 60 * 60 * 24)?
            .to_units(TICKS_PER_SECOND).filter(|n| *n > 0)?;
        if rate == PICTURE_NEST_24_RATE
            && text(receipt, "compatibility_start").and_then(Rational::parse)
                .and_then(|start| start.to_units(TICKS_PER_SECOND)) != Some(parent_start)
        {
            return None;
        }
        let Value::Arr(issues) = key(receipt, "unresolved")? else { return None };
        let blocked = picture_issue_conflicts(issues, rate_text)?;
        Some((sequence, seq, parent_start, rate, rate_text, issues, blocked))
    });
    let mut candidates = Vec::new();
    for (sequence, seq, parent_start, rate, rate_text, issues, blocked) in sequences {
        let unit = model::ticks_per_unit(rate);
        let drafts: Vec<_> = issues.iter().enumerate().filter_map(|(index, issue)| {
            if blocked.contains(&index) {
                return None;
            }
            if !is_layered_profile(issue) || num(issue, "kind")? != 1
                || text(issue, "record_rate")? != rate_text || num(issue, "input_count")? != 3
            {
                return None;
            }
            let track = num(issue, "track").filter(|n| (1..=64).contains(n))?;
            let frame = num(key(issue, "record_start")?, "value").filter(|n| *n >= 0)?;
            let length = num(issue, "record_length").filter(|n| *n > 0)?;
            let owner = text(issue, "owner")?;
            let reason = text(issue, "reason")?;
            if !owner.strip_prefix(&seq.id).is_some_and(|path| path.starts_with("/component/")) {
                return None;
            }
            let Value::Arr(inputs) = key(issue, "submaster_inputs")? else { return None };
            let [background, lower, upper] = inputs.as_slice() else { return None };
            if [num(background, "track"), num(lower, "track"), num(upper, "track")] != [Some(1), Some(2), Some(3)]
                || text(background, "segment_class") != Some("FILL")
                || key(background, "picture_source").is_some()
            {
                return None;
            }
            let layers = [layer(lower, length, rate, rate_text)?, layer(upper, length, rate, rate_text)?];
            let start = frame.checked_mul(unit)?;
            let duration = length.checked_mul(unit)?;
            if layers[0].start != 0 || layers[0].duration != duration
                || parent_start.checked_add(start.checked_add(duration)?).is_none()
            {
                return None;
            }
            let draft = PreparedLayeredSubmaster { sequence, track, start, duration, layers };
            if neighboring_picture_conflicts(seq, &draft) {
                return None;
            }
            Some((owner, LayeredCandidate {
                sequence_id: &seq.id,
                loss_reason: format!("V{track} at {owner}: {reason}"),
                parent_start,
                draft,
            }))
        }).collect();
        let owners: HashMap<_, _> = drafts.iter().map(|(owner, candidate)| (*owner, &candidate.draft)).collect();
        let uncertain_markers: Vec<_> = seq.markers.iter().filter(|marker| {
            marker.owner == MarkerOwner::Edit && !owners.get(marker.owner_occurrence.as_str())
                .is_some_and(|nest| marker_falls_back_to_parent(seq, marker, nest))
        }).collect();
        candidates.extend(drafts.into_iter().filter_map(|(owner, candidate)| {
            let conflict = uncertain_markers.iter().any(|marker| {
                marker.owner_occurrence == owner
                    || marker.owner_occurrence.strip_prefix(owner).is_some_and(|path| path.starts_with('/'))
                    || marker.track.is_some_and(|lane| {
                        lane.kind == StreamKind::Video && record_span_conflicts(
                            &candidate.draft, lane.index, marker.start, Some(marker.duration),
                        )
                    })
            });
            (!conflict).then_some(candidate)
        }));
    }
    candidates
}

pub(super) fn add_layered<'a>(
    candidates: Vec<LayeredCandidate<'a>>, input: &mut PreparedProjectInput,
) -> R<HashMap<&'a str, HashSet<String>>> {
    let mut restored = HashMap::<_, HashSet<_>>::new();
    for LayeredCandidate { sequence_id, loss_reason, parent_start, draft } in candidates {
        let seq = &input.sequences[draft.sequence];
        if seq.start != parent_start || (seq.width, seq.height) != (1920, 1080)
            || (seq.rate != PICTURE_NEST_24_RATE
                && draft.layers.iter().any(|layer| !input.direct_dnxhr_origins.contains_key(&layer.clip)))
            || draft.has_picture_conflict(input)
        {
            return Err(error(format!("layered Submaster in {:?} conflicts with prepared picture or clock", seq.name)));
        }
        input.layered_submasters.push(draft);
        restored.entry(sequence_id).or_default().insert(loss_reason);
    }
    Ok(restored)
}

pub(super) fn prepare(
    graph: &SourceGraph, media: &HashMap<&str, &SourceMedia>, sidecars: &Sidecars<'_>, input: &mut PreparedProjectInput,
    ownership: &HashMap<String, (usize, usize)>, record_only_markers: &HashSet<(usize, usize)>,
    typed: bool, clock: ClockProfile, losses: &mut Vec<Loss>,
) {
    let online: HashSet<_> = graph.media.iter().filter(|m| m.online == Some(true))
        .map(|m| m.id.as_str()).collect();
    let linked: HashMap<_, _> = graph.items.iter().filter(|item| {
        item.media.iter().all(|reference| online.contains(reference.media.as_str()))
    }).map(|item| (item.id.as_str(), item)).collect();
    let busy: HashSet<_> = input.picture_effects.iter().map(|e| (e.sequence, e.event))
        .chain(input.video_dissolves.iter().flat_map(|e| [(e.sequence, e.incoming), (e.sequence, e.outgoing)]))
        .chain(input.multicam_uses.iter().map(|e| (e.sequence, e.event)))
        .chain(input.av_subclip_uses.iter().map(|e| (e.sequence, e.event)))
        .chain(input.edit_labels.iter().map(|e| (e.sequence, e.event)))
        .collect();
    for (sequence_index, sequence) in graph.sequences.iter().enumerate() {
        let rate = fraction(sequence.rate);
        let parent_start = if rate == PICTURE_NEST_24_RATE {
            direct_avid_tc1(sequence, 24, false, 24 * 60 * 60 * 24)
        } else {
            qualified_sequence_start(sequence)
        }.filter(|start| *start > Rational::ZERO)
            .and_then(|start| ticks(start, "Submaster parent origin").ok());
        // Every marker claiming the nest must have a resolved owner. It can
        // already be on the parent if its record range survived preparation.
        let mut source_markers = HashMap::new();
        for (marker_index, marker) in sequence.markers.iter().enumerate() {
            let qualified = marker.owner == MarkerOwner::Edit
                && marker.binding.reason.is_none() && marker.binding.placements.len() == 1;
            for id in &marker.binding.placements {
                let matches_track = ownership.get(id).is_some_and(|&(si, ei)| {
                    si == sequence_index && input.sequences[si].events[ei].video_track.is_some_and(|index| {
                        marker.track == Some(TrackRef { kind: StreamKind::Video, index })
                    })
                });
                let count = source_markers.entry(id.as_str()).or_insert(Some(0usize));
                let already_on_parent = record_only_markers.contains(&(sequence_index, marker_index));
                *count = count.filter(|_| qualified && matches_track)
                    .map(|n| n + usize::from(!already_on_parent));
            }
        }
        let mut edit_markers: HashMap<usize, Vec<usize>> = HashMap::new();
        for (index, marker) in input.sequences[sequence_index].markers.iter().enumerate() {
            if marker.owner == "edit" {
                if let Some(event) = marker.event.and_then(|i| usize::try_from(i).ok()) {
                    edit_markers.entry(event).or_default().push(index);
                }
            }
        }
        for placement in &sequence.placements {
            let Some(facts) = side(sidecars, "avid.source", &placement.id) else { continue };
            let Some(Value::Arr(via)) = key(facts, "via") else { continue };
            if !via.iter().any(|entry| str_value(entry) == Some("effect EFF_SUBMASTER not converted")) {
                continue;
            }
            let admitted = ownership.get(&placement.id).filter(|&&(si, ei)| {
                let target = &input.sequences[si];
                let edit = &target.events[ei];
                let markers = edit_markers.get(&ei).map(Vec::as_slice).unwrap_or_default();
                let Some(backing) = linked.get(placement.item.as_str()) else { return false };
                typed && clock == ClockProfile::LegacyImport && input.options.bin_mode == "separate"
                    && PICTURE_NEST_RATES.contains(&rate) && parent_start == Some(target.start)
                    && (target.width, target.height) == (1920, 1080)
                    && key(facts, "plain_submaster").and_then(boolean) == Some(true) && via.len() == 1
                    && plain_picture_edit(edit) && !busy.contains(&(si, ei))
                    && (input.direct_dnxhr_origins.contains_key(&edit.clip)
                        || (target.rate == PICTURE_NEST_24_RATE
                            && direct_opatom_clocks(backing, media, sidecars, true, true, false)
                                .is_some_and(|(start, origin, _, _)| start == 0 && origin == 0)))
                    && si == sequence_index
                    && source_markers.get(placement.id.as_str()).copied().unwrap_or(Some(0)) == Some(markers.len())
                    && ticks(placement.source_in, "Submaster source").ok() == Some(edit.source_in)
                    && ticks(placement.record_start, "Submaster record").ok() == Some(edit.record_start)
                    && ticks(placement.duration, "Submaster length").ok() == Some(edit.duration)
            });
            if let Some(&(si, event)) = admitted {
                input.picture_nests.push(PreparedPictureNest { sequence: si, event });
                let track = input.sequences[si].events[event].video_track.expect("plain picture edit");
                for &i in edit_markers.get(&event).into_iter().flatten() {
                    let marker = &mut input.sequences[si].markers[i];
                    loss(losses, &placement.id, "marker_ownership", format!(
                        "marker {:?} on V{track} at {} ticks moved from the Submaster edit to the parent sequence",
                        marker.name, marker.start,
                    ));
                    marker.owner = "sequence".into();
                    marker.event = None;
                    marker.source = None;
                    marker.inherited = false;
                }
            } else {
                loss(losses, &placement.id, "picture_nest",
                    "Avid Submaster boundary flattened; editable output requires one plain same-rate linked picture input and an exact clock");
            }
        }
    }
}
