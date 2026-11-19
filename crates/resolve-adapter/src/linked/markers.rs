use super::{MovieVideo, Profile};
use crate::{
    premiere::{CROSSES_SPLIT, CROSSES_SPLIT_REASON, EDIT_MARKER_PROVENANCE},
    prepare::{
        frames_on, marker_storage, prepare_edit_markers, prepare_marker, prepare_markers,
        PreparedMarker, MARKER_STORAGE_BUDGET, MAX_MARKERS_PER_OWNER,
    },
    Result,
};
use graph::{
    source::{SourceGraph, SourceMarker, SourceSequence},
    Loss, MarkerOwner, Placement, Rational, StreamKind, Value,
};
use std::collections::{HashMap, HashSet};

pub(super) const LOSS_ASPECT: &str = "marker_ownership";

pub(super) struct TimelineMarkerPreparation<'a> {
    graph: &'a SourceGraph,
    sequence: &'a SourceSequence,
    restored_occurrences: HashSet<&'a str>,
    eligible_edits: Vec<(&'a Placement, &'a Value)>,
}

impl<'a> TimelineMarkerPreparation<'a> {
    pub(super) fn new(graph: &'a SourceGraph, sequence: &'a SourceSequence) -> Self {
        Self {
            graph,
            sequence,
            restored_occurrences: HashSet::new(),
            eligible_edits: Vec::new(),
        }
    }

    pub(super) fn prepare_edit(
        &mut self,
        edit: &'a Placement,
        profile: &Profile,
        editable_v1: bool,
    ) -> Result<Vec<PreparedMarker>> {
        let sequence = self.sequence;
        let provenance = self
            .graph
            .sidecar(EDIT_MARKER_PROVENANCE, &edit.id)
            .filter(|row| {
                editable_v1
                    && crate::source::supported_marker_rate(sequence.rate)
                    && row.get("restored") == Some(&Value::Bool(true))
                    && matches!(profile, Profile::Mov {
                    video: MovieVideo::ProResHq, raster: (1920, 1080), aac: false, pcm: None,
                    avid_source_timecode: None | Some(0), rate, ..
                } if *rate == sequence.rate)
            });
        let Some(provenance) = provenance else {
            return Ok(Vec::new());
        };
        let (markers, omitted) =
            prepare_edit_markers(sequence, edit, None, provenance.get("markers"))?;
        self.eligible_edits.push((edit, provenance));
        for marker in &sequence.markers {
            if marker.owner == MarkerOwner::Edit
                && marker.binding.placements.as_slice() == [edit.id.as_str()]
                && !omitted.contains(&marker.owner_occurrence)
            {
                self.restored_occurrences
                    .insert(marker.owner_occurrence.as_str());
            }
        }
        Ok(markers)
    }

    pub(super) fn finish(
        self,
        duration: i64,
        losses: &mut Vec<Loss>,
    ) -> Result<(Vec<PreparedMarker>, Vec<String>)> {
        let (mut markers, mut omitted) = prepare_markers(self.graph, self.sequence, duration)?;
        omitted.retain(|occurrence| !self.restored_occurrences.contains(occurrence.as_str()));
        append_fallbacks(
            self.sequence,
            duration,
            self.eligible_edits,
            &mut markers,
            &mut omitted,
            losses,
        );
        Ok((markers, omitted))
    }
}

/// Preserve the timing of ranges that span contiguous pieces of one marked
/// edit. Existing timeline markers retain priority at their saved frames.
fn append_fallbacks(
    sequence: &SourceSequence,
    duration: i64,
    mut edits: Vec<(&Placement, &Value)>,
    markers: &mut Vec<PreparedMarker>,
    omitted: &mut Vec<String>,
    losses: &mut Vec<Loss>,
) {
    if edits.len() < 2 {
        return;
    }
    edits.sort_unstable_by_key(|(edit, _)| edit.record_start);
    let indices: HashMap<_, _> = edits
        .iter()
        .enumerate()
        .map(|(index, (edit, _))| (edit.id.as_str(), index))
        .collect();
    let mut counts = HashMap::new();
    for marker in &sequence.markers {
        if marker.owner == MarkerOwner::Sequence {
            if let Some(frame) = marker
                .start
                .and_then(|start| frames_on(start, sequence.rate).ok())
            {
                *counts.entry(frame).or_insert(0) += 1;
            }
        }
    }
    let mut candidates = Vec::new();
    for marker in &sequence.markers {
        let [owner] = marker.binding.placements.as_slice() else {
            continue;
        };
        if marker.owner != MarkerOwner::Edit
            || marker.binding.reason.as_deref() != Some(CROSSES_SPLIT_REASON)
            || !marker
                .track
                .is_some_and(|track| track.kind == StreamKind::Video && track.index == 1)
        {
            continue;
        }
        let Some(&index) = indices.get(owner.as_str()) else {
            continue;
        };
        let Some(Value::Arr(rows)) = edits[index].1.get("markers") else {
            continue;
        };
        let Some(row) = rows.iter().find(|row| {
            matches!(row.get("source_marker"), Some(Value::Str(id)) if id == &marker.owner_occurrence)
                && row.get(CROSSES_SPLIT) == Some(&Value::Bool(true))
        }) else { continue };
        let Some(start) = project_range(marker, &edits[index..]) else {
            continue;
        };
        let Ok(frame) = frames_on(start, sequence.rate) else {
            continue;
        };
        *counts.entry(frame).or_insert(0) += 1;
        let mut candidate = marker.clone();
        candidate.owner = MarkerOwner::Sequence;
        candidate.start = Some(start);
        candidate.owner_local = Some(start);
        candidate.track = None;
        candidate.source = None;
        candidate.binding.placements.clear();
        candidate.binding.reason = None;
        let custom = match row.get("custom_data") {
            Some(Value::Str(value)) => value.as_str(),
            _ => "",
        };
        candidates.push((candidate, custom));
    }
    let mut used: usize = markers
        .iter()
        .map(|marker| marker_storage(&marker.name, &marker.note, &marker.custom_data))
        .sum();
    let mut recovered = HashSet::new();
    for (candidate, custom) in candidates {
        let bytes = marker_storage(&candidate.name, &candidate.comment, custom);
        if markers.len() == MAX_MARKERS_PER_OWNER
            || bytes > MARKER_STORAGE_BUDGET.saturating_sub(used)
        {
            continue;
        }
        let Some(mut marker) = prepare_marker(&candidate, sequence.rate, duration, &counts) else {
            continue;
        };
        marker.custom_data = custom.into();
        markers.push(marker);
        used += bytes;
        recovered.insert(candidate.owner_occurrence.clone());
        losses.push(Loss {
            object: candidate.owner_occurrence,
            aspect: LOSS_ASPECT.into(),
            reason:
                "Edit marker range crosses a split and will be written as a Resolve timeline marker"
                    .into(),
        });
    }
    if !recovered.is_empty() {
        markers.sort_unstable_by_key(|marker| marker.frame);
        omitted.retain(|occurrence| !recovered.contains(occurrence));
    }
}

fn project_range(marker: &SourceMarker, edits: &[(&Placement, &Value)]) -> Option<Rational> {
    let (&(first, receipt), rest) = edits.split_first()?;
    let Some(copy @ Value::Str(_)) = receipt.get("dedicated_item") else {
        return None;
    };
    let start = marker.source?;
    let end = start.checked_add(marker.duration)?;
    let mut source_end = first.source_in.checked_add(first.duration)?;
    if start < first.source_in || start >= source_end || end <= source_end {
        return None;
    }
    let mut record_end = first.record_start.checked_add(first.duration)?;
    for &(next, receipt) in rest {
        if next.record_start != record_end
            || next.source_in != source_end
            || next.item != first.item
            || receipt.get("dedicated_item") != Some(copy)
        {
            break;
        }
        source_end = next.source_in.checked_add(next.duration)?;
        record_end = next.record_start.checked_add(next.duration)?;
        if end <= source_end {
            return first
                .record_start
                .checked_add(start.checked_sub(first.source_in)?);
        }
    }
    None
}
