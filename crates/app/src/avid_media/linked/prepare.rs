//! Validate the graph and resolve each edit to its saved physical stream.
use super::{
    markers,
    source_profile::{MediaIndex, Profile},
    transition, EditBinding, EditRate, Plan, PreparedEdit, Result, Span, RASTER, TIMECODE_BASE,
};
use graph::{
    source::{SourceGraph, SourceSequence},
    Rational, StreamKind,
};

pub(super) fn frames(rate: EditRate, time: Rational) -> Option<u32> {
    rate.frames(time)?.try_into().ok()
}

pub(super) fn timeline_clock(rate: EditRate, time: Option<Rational>) -> Option<u32> {
    frames(rate, time?).filter(|count| *count < 86_400 * TIMECODE_BASE as u32)
}

pub(super) fn matches_clock(
    clock: Option<&graph::source::SourceClock>,
    start: u32,
    rate: EditRate,
) -> bool {
    clock.is_none_or(|clock| {
        clock.effective_count == u64::from(start)
            && clock.rate == rate.exact()
            && clock.display_base == 24
            && !clock.drop
    })
}

pub(super) fn neutral_edit(edit: &graph::Placement, picture: bool) -> bool {
    let separate = if picture {
        edit.audio_tracks.is_empty() && edit.audio_streams.is_empty()
    } else {
        edit.video_track.is_none()
    };
    separate
        && edit.grouped_audio.is_empty()
        && edit.gain_db == 0.0
        && matches!(edit.volume_db, None | Some(0.0))
        && edit.volume_keys.is_empty()
        && edit
            .track_gains_db
            .iter()
            .all(|gain| matches!(gain, None | Some(0.0)))
        && edit.track_pans.iter().all(Option::is_none)
        && edit.speed.is_none()
        && edit.frame_map.is_none()
        && edit.transform.is_none()
        && edit.effects.is_empty()
}

pub(super) fn prepare_span(
    occurrence: &graph::Placement,
    source_offset: Option<Rational>,
    extent: u32,
    rate: EditRate,
) -> Result<Span> {
    let source = source_offset.and_then(|offset| occurrence.source_in.checked_add(offset));
    let span = Span {
        record: frames(rate, occurrence.record_start)
            .ok_or_else(|| format!("{}: record start is not on the frame grid", occurrence.id))?,
        source: source.and_then(|time| frames(rate, time)).ok_or_else(|| {
            format!(
                "{}: source trim has an unknown offset or falls off the frame grid",
                occurrence.id
            )
        })?,
        length: frames(rate, occurrence.duration)
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                format!(
                    "{}: edit needs a positive whole-frame duration",
                    occurrence.id
                )
            })?,
    };
    if span
        .source
        .checked_add(span.length)
        .is_none_or(|end| end > extent)
        || span.source > i32::MAX as u32
    {
        return Err(format!(
            "{}: source range exceeds the original extent",
            occurrence.id
        )
        .into());
    }
    if span.record.checked_add(span.length).is_none() {
        return Err(format!("{}: record range overflows", occurrence.id).into());
    }
    Ok(span)
}

pub(super) const LANES: [(StreamKind, i64, &str); 3] = [
    (StreamKind::Video, 1, "V1"),
    (StreamKind::Audio, 1, "A1"),
    (StreamKind::Audio, 2, "A2"),
];

pub(super) fn neutral_sequence(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    start: u32,
    rate: EditRate,
    allow_empty_tracks: bool,
) -> bool {
    sequence.frame_size == Some(RASTER)
        && !sequence.name.trim().is_empty()
        && matches_clock(sequence.clock.as_ref(), start, rate)
        && sequence.texts.is_empty()
        && sequence.disabled_placements.is_empty()
        && graph.losses.is_empty()
        && graph.multicams.is_empty()
        && graph.multicam_uses.is_empty()
        && sequence
            .tracks
            .iter()
            .filter(|t| !allow_empty_tracks || !t.empty)
            .count()
            == 3
}

pub(super) fn lane_edits(
    sequence: &SourceSequence,
    lane_index: usize,
) -> Result<Vec<&graph::Placement>> {
    let (kind, lane, label) = LANES[lane_index];
    if !sequence.tracks.iter().any(|track| {
        track.kind == kind
            && track.index == lane
            && !track.empty
            && matches!(track.level_db, None | Some(0.0))
            && track.name.as_deref().is_none_or(|name| name == label)
    }) {
        return Err(format!("{label}: missing neutral track").into());
    }
    Ok(sequence
        .placements
        .iter()
        .filter(|edit| {
            if kind == StreamKind::Video {
                edit.video_track == Some(lane)
            } else {
                edit.audio_tracks == [lane]
            }
        })
        .collect())
}

pub(super) fn prepare_transitions(
    sequence: &SourceSequence,
    lane_index: usize,
    rate: EditRate,
    edits: &mut [(&str, PreparedEdit)],
) -> Result<Vec<transition::Transition>> {
    let (kind, lane, label) = LANES[lane_index];
    edits.sort_by_key(|(_, edit)| edit.span.record);
    let spans: Vec<_> = edits.iter().map(|(id, edit)| (*id, edit.span)).collect();
    transition::prepare(
        sequence
            .transitions
            .iter()
            .filter(|t| t.kind == kind && t.track == lane),
        &spans,
        rate,
        kind == StreamKind::Video,
        label,
    )
}

pub(super) fn plan<'a>(
    graph: &'a SourceGraph,
    sequence_id: &str,
    captured: bool,
) -> Result<Plan<'a>> {
    let [sequence] = graph.sequences.as_slice() else {
        return Err("linked bin pilot requires one sequence".into());
    };
    if sequence.id != sequence_id {
        return Err("linked bin sequence ID was not found".into());
    }
    if super::mixed::applies(graph) {
        return super::mixed::plan(graph, sequence);
    }
    let profile = Profile::identify(graph);
    profile.check_owners(graph, captured)?;
    let overrides = profile.check_receipts(graph, captured)?;
    let rate = EditRate::from_graph(sequence.rate)?;
    let start =
        timeline_clock(rate, sequence.start).ok_or("sequence needs a known whole-frame start")?;
    if !neutral_sequence(
        graph,
        sequence,
        start,
        rate,
        captured || profile == Profile::ManagedAtom,
    ) || !profile.accepts_counts(graph.items.len(), graph.media.len())
    {
        return Err(profile.layout_error().into());
    }
    profile.check_rate_and_overrides(rate, overrides, graph.items.len())?;
    let media_index = MediaIndex::new(graph);
    let mut lanes = Vec::with_capacity(3);
    let mut item_ids = Vec::with_capacity(3);
    let mut picture_markers = Vec::new();
    let mut transitions = [Vec::new(), Vec::new(), Vec::new()];
    for (lane_index, (kind, _, label)) in LANES.into_iter().enumerate() {
        let picture = kind == StreamKind::Video;
        let edits = lane_edits(sequence, lane_index)?;
        let edit = edits
            .first()
            .copied()
            .ok_or_else(|| format!("{label}: missing edit"))?;
        let item = graph
            .items
            .iter()
            .find(|item| item.id == edit.item)
            .ok_or("missing edit item")?;
        let qualified = profile.qualify_route(
            &media_index,
            item,
            edit,
            kind,
            lane_index,
            rate,
            captured,
            item_ids.first().copied(),
        )?;
        if picture {
            picture_markers = qualified.markers;
        }
        let binding = EditBinding {
            group: 0,
            route: lane_index,
            track: qualified.route.source_lane,
        };
        let mut spans = Vec::with_capacity(edits.len());
        for occurrence in edits {
            if !neutral_edit(occurrence, picture) {
                return Err(
                    format!("{}: expected a neutral edit on {label}", occurrence.id).into(),
                );
            }
            if occurrence.item != edit.item || occurrence.audio_streams != edit.audio_streams {
                return Err(format!(
                    "{}: {label} needs a consistent source item and channel",
                    occurrence.id
                )
                .into());
            }
            let span = prepare_span(
                occurrence,
                qualified.binding.source_offset,
                qualified.route.extent,
                rate,
            )?;
            spans.push((occurrence.id.as_str(), PreparedEdit { span, binding }));
        }
        transitions[lane_index] = prepare_transitions(sequence, lane_index, rate, &mut spans)?;
        item_ids.push(qualified.item.id.as_str());
        lanes.push((
            qualified.route,
            spans.into_iter().map(|(_, edit)| edit).collect::<Vec<_>>(),
        ));
    }
    let [(video, v), (left, a), (right, b)] =
        lanes.try_into().map_err(|_| "missing linked lane")?;
    let source = profile.finish(
        &media_index,
        graph.items.iter(),
        [video, left, right],
        &item_ids,
        overrides,
        picture_markers,
    )?;
    timeline_plan(sequence, rate, start, vec![source], [v, a, b], transitions)
}

pub(super) fn timeline_plan<'a>(
    sequence: &'a SourceSequence,
    rate: EditRate,
    start: u32,
    sources: Vec<super::SourceSet<'a>>,
    edits: [Vec<PreparedEdit>; 3],
    transitions: [Vec<transition::Transition>; 3],
) -> Result<Plan<'a>> {
    if transitions.iter().map(Vec::len).sum::<usize>() != sequence.transitions.len() {
        return Err("every linked transition must belong to V1, A1 or A2".into());
    }
    if edits.iter().map(Vec::len).sum::<usize>() != sequence.placements.len() {
        return Err("every linked edit must belong to V1, A1 or A2".into());
    }
    let length = edits
        .iter()
        .filter_map(|lane| lane.last())
        .map(|edit| edit.span.record + edit.span.length)
        .max()
        .ok_or("missing linked edits")?;
    let sequence_markers = markers::prepare(&sequence.markers, Some(Rational::ZERO), length, rate)?;
    Ok(Plan {
        name: &sequence.name,
        rate,
        start,
        length,
        sources,
        edits,
        transitions,
        sequence_markers,
    })
}
