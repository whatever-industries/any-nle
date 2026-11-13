//! Verify the leaf edit selected by a saved Resolve V1 multicam occurrence.
use super::{frame_range, Edit, EditContent, Timeline, MAX_EDITS};
use crate::{
    source::{multicam::selection, ADAPTER},
    Result,
};
use graph::{source::SourceGraph, StreamKind};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

pub(super) fn add<'a>(graph: &'a SourceGraph, timeline: &mut Timeline<'a>) -> Result<()> {
    let uses = graph.multicam_uses.iter().filter(|usage| {
        usage.sequence == timeline.sequence.id
            && usage.kind == StreamKind::Video
            && usage.track == 1
    });
    let count = uses.clone().count();
    if count == 0 {
        return Ok(());
    }
    if count > MAX_EDITS {
        return Err("V1 multicam use count exceeds conform report limit".into());
    }
    let groups: HashMap<_, _> = graph.multicams.iter().map(|g| (g.id.as_str(), g)).collect();
    let receipts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|r| r.adapter == ADAPTER)
        .map(|r| (r.object.as_str(), &r.data))
        .collect();
    let edits: HashMap<_, _> = timeline
        .edits
        .iter()
        .enumerate()
        .filter_map(|(i, edit)| match edit.content {
            EditContent::Media { placement, .. } => Some((placement.id.as_str(), i)),
            EditContent::Gap => None,
        })
        .collect();
    let mut binding_counts = HashMap::<_, usize>::new();
    for usage in uses.clone() {
        for id in &usage.binding.placements {
            *binding_counts.entry(id.as_str()).or_default() += 1;
        }
    }
    let mut rejected_bindings = HashSet::new();
    for usage in uses {
        let Some(range) = usage
            .record_start
            .zip(usage.duration)
            .and_then(|(start, duration)| {
                frame_range(start, duration, timeline.sequence.rate).ok()
            })
        else {
            timeline.issue = Some("unqualified multicam timing");
            continue;
        };
        timeline
            .coverage_boundaries
            .extend([range.start, range.end]);
        if timeline.extent.is_none_or(|end| range.end > end) {
            timeline.issue = Some("multicam edit exceeds saved timeline extent");
            continue;
        }
        let group = match usage.candidates.as_slice() {
            [id] => groups.get(id.as_str()).copied(),
            _ => None,
        };
        let Some(selected) = group.and_then(|group| {
            selection::select(
                usage,
                group,
                timeline.sequence.rate,
                receipts.get(group.id.as_str())?,
                receipts.get(usage.id.as_str())?,
                &range,
            )
        }) else {
            rejected_bindings.extend(usage.binding.placements.iter().map(String::as_str));
            timeline.unqualified.push(range);
            continue;
        };
        let bound = match (&selected.leaf, usage.binding.placements.as_slice()) {
            (Some((visible, _, _)), [id]) if binding_counts.get(id.as_str()) == Some(&1) => edits
                .get(id.as_str())
                .copied()
                .filter(|&i| {
                    let edit = &timeline.edits[i];
                    matches!(edit.content, EditContent::Media { placement, .. }
                        if selected.agrees(usage, placement, &edit.range))
                })
                .map(|i| Some((i, visible.clone()))),
            (None, []) => Some(None),
            _ => None,
        };
        let Some(bound) = bound else {
            rejected_bindings.extend(usage.binding.placements.iter().map(String::as_str));
            timeline.unqualified.push(range);
            continue;
        };
        if let Some((i, _)) = &bound {
            timeline.edits[*i].multicam_use = Some(&usage.id);
        }
        let mut gap = |range: Range<i64>| {
            if !range.is_empty() {
                timeline.edits.push(Edit {
                    nest: None,
                    multicam_use: Some(&usage.id),
                    range,
                    content: EditContent::Gap,
                });
            }
        };
        if let Some((_, visible)) = bound {
            gap(range.start..visible.start);
            gap(visible.end..range.end);
        } else {
            gap(range);
        }
    }
    // A contradictory flattened placement can lie partly outside its saved
    // multicam occurrence. Its extra frames also lack a proven mapping.
    for edit in &timeline.edits {
        if matches!(edit.content, EditContent::Media { placement, .. }
            if rejected_bindings.contains(placement.id.as_str()))
        {
            timeline.unqualified.push(edit.range.clone());
        }
    }
    if timeline.edits.len() + timeline.unqualified.len() > MAX_EDITS {
        return Err("expanded V1 multicam edit count exceeds conform report limit".into());
    }
    Ok(())
}
