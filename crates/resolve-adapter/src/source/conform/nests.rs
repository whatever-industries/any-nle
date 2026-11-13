//! Project one level of qualified child V1 edits onto the parent frame grid.
use super::{direct_timeline, Bindings, Edit, EditContent, Timeline, MAX_EDITS};
use crate::{
    nests::{int, string, ADAPTER, PROFILE},
    prepare::frames_on,
    source::{ADAPTER as SOURCE, CLOCK_ROUTE},
    Result,
};
use graph::{
    source::{SourceGraph, SourceSequence},
    Rational, Value,
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

const MAX_CHILDREN: usize = 128;
const MAX_CHILD_INTERVAL_VISITS: usize = MAX_EDITS * 16;

pub(super) fn add<'a>(
    graph: &'a SourceGraph,
    bindings: &Bindings<'a>,
    parent: &mut Timeline<'a>,
) -> Result<()> {
    let Some(data) = graph.sidecar(ADAPTER, &parent.sequence.id) else {
        return Ok(());
    };
    let Value::Arr(rows) = data else {
        parent.issue = Some("unknown saved nesting coverage");
        return Ok(());
    };
    if rows.len() > MAX_EDITS {
        return Err("V1 nest count exceeds conform report limit".into());
    }
    let sequences: HashMap<_, _> = graph.sequences.iter().map(|s| (s.id.as_str(), s)).collect();
    let receipts: HashMap<_, _> = graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == SOURCE)
        .map(|s| (s.object.as_str(), &s.data))
        .collect();
    let nested_children: HashSet<_> = graph
        .sidecars
        .iter()
        .filter(|s| s.adapter == ADAPTER)
        .map(|s| s.object.as_str())
        .collect();
    let mut children = HashMap::new();
    let mut prepared_edits = 0;
    let mut visited_intervals = 0;
    let mut id_counts = HashMap::<_, usize>::new();
    for row in rows {
        if let Some(id) = string(row, "id") {
            *id_counts.entry(id).or_default() += 1;
        }
    }
    for row in rows {
        match string(row, "track_kind") {
            Some("audio") => continue,
            Some("video") if int(row, "track").is_some_and(|track| track > 1) => continue,
            Some("video") if int(row, "track") == Some(1) => {}
            _ => {
                parent.issue = Some("unknown saved nesting coverage");
                continue;
            }
        }
        let Some(range) = span(row, parent.extent, parent.sequence.rate) else {
            parent.issue = Some("unknown saved nesting coverage");
            continue;
        };
        parent.coverage_boundaries.extend([range.start, range.end]);
        let qualified = string(row, "id")
            .zip(string(row, "child_sequence"))
            .filter(|(id, child)| {
                id_counts.get(id) == Some(&1)
                    && super::compare::check_id(id).is_ok()
                    && !nested_children.contains(child)
            })
            .zip(int(row, "saved_source_in_frames").filter(|&start| start >= 0));
        let Some(((id, child_id), source_in)) = qualified else {
            parent.unqualified.push(range);
            continue;
        };
        let child = sequences
            .get(child_id)
            .copied()
            .zip(receipts.get(child_id).copied())
            .filter(|(seq, receipt)| qualified_child(row, parent.sequence, seq, receipt));
        let Some((seq, receipt)) = child else {
            parent.unqualified.push(range);
            continue;
        };
        let Some(source_end) = source_in
            .checked_add(range.end - range.start)
            .filter(|end| int(receipt, "record_length").is_some_and(|length| *end <= length))
        else {
            parent.unqualified.push(range);
            continue;
        };
        let child_count = children.len();
        let child = match children.entry(child_id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                if child_count == MAX_CHILDREN {
                    return Err("distinct nested child count exceeds conform report limit".into());
                }
                let timeline = direct_timeline(graph, seq, bindings)?;
                prepared_edits += timeline.edits.len() + timeline.unqualified.len();
                if prepared_edits > MAX_EDITS {
                    return Err("child V1 edit count exceeds conform report limit".into());
                }
                entry.insert(timeline)
            }
        };
        if child.issue.is_some() || child.extent != int(receipt, "record_length") {
            parent.unqualified.push(range);
            continue;
        }
        visited_intervals += child.edits.len() + child.unqualified.len();
        if visited_intervals > MAX_CHILD_INTERVAL_VISITS {
            return Err("nested child interval work exceeds conform report limit".into());
        }
        let clipped = |r: &Range<i64>| r.start.max(source_in)..r.end.min(source_end);
        let project = |r: &Range<i64>| {
            (range.start + (r.start - source_in))..(range.start + (r.end - source_in))
        };
        let mut covered_until = source_in;
        for edit in &child.edits {
            let overlap = clipped(&edit.range);
            if overlap.is_empty() {
                continue;
            }
            if overlap.start > covered_until {
                parent.edits.push(Edit {
                    nest: Some(id),
                    multicam_use: None,
                    range: project(&(covered_until..overlap.start)),
                    content: EditContent::Gap,
                });
            }
            let content = match edit.content {
                EditContent::Gap => EditContent::Gap,
                EditContent::Media {
                    placement,
                    source,
                    elapsed_offset,
                } => EditContent::Media {
                    placement,
                    source,
                    elapsed_offset: elapsed_offset + (overlap.start - edit.range.start),
                },
            };
            parent.edits.push(Edit {
                nest: Some(id),
                multicam_use: edit.multicam_use,
                range: project(&overlap),
                content,
            });
            covered_until = covered_until.max(overlap.end);
            check_budget(parent)?;
        }
        if covered_until < source_end {
            parent.edits.push(Edit {
                nest: Some(id),
                multicam_use: None,
                range: project(&(covered_until..source_end)),
                content: EditContent::Gap,
            });
        }
        // A child transition or unsupported edit takes precedence over
        // the projected file edits and empty intervals at those frames.
        for unknown in child.unqualified.iter().chain(&child.dissolve) {
            let overlap = clipped(unknown);
            if !overlap.is_empty() {
                let interval = project(&overlap);
                parent
                    .coverage_boundaries
                    .extend([interval.start, interval.end]);
                parent.unqualified.push(interval);
                check_budget(parent)?;
            }
        }
    }
    check_budget(parent)
}

fn check_budget(timeline: &Timeline<'_>) -> Result<()> {
    if timeline.edits.len() + timeline.unqualified.len() > MAX_EDITS {
        return Err("expanded V1 edit count exceeds conform report limit".into());
    }
    Ok(())
}

fn span(row: &Value, extent: Option<i64>, rate: Rational) -> Option<Range<i64>> {
    if Rational::parse(string(row, "record_rate")?) != Some(rate) {
        return None;
    }
    let start = int(row, "record_start_frames").filter(|&n| n >= 0)?;
    let length = int(row, "record_duration_frames").filter(|&n| n > 0)?;
    let end = start.checked_add(length)?;
    (end <= extent?).then_some(start..end)
}

fn qualified_child(
    row: &Value,
    parent: &SourceSequence,
    child: &SourceSequence,
    receipt: &Value,
) -> bool {
    let saved_origin = int(row, "child_origin_frames").filter(|&n| n >= 0);
    parent.rate == Rational::integer(24)
        && child.rate == parent.rate
        && parent.frame_size.is_some()
        && child.frame_size == parent.frame_size
        && string(row, "profile") == Some(PROFILE)
        && string(row, "child_rate") == Some("24")
        && string(row, "pool_id")
            .is_some_and(|pool| string(receipt, "media_pool_clip") == Some(pool))
        && saved_origin.is_some()
        && int(receipt, "record_origin") == saved_origin
        && child
            .start
            .and_then(|start| frames_on(start, child.rate).ok())
            == saved_origin
        && child
            .clock
            .as_ref()
            .is_some_and(|clock| clock.provenance.route == CLOCK_ROUTE)
        && match (string(row, "kind"), string(receipt, "kind")) {
            (Some("timeline"), Some("registered_timeline")) => true,
            (Some("compound"), Some("compound")) => {
                receipt.get("container_ref_agrees") == Some(&Value::Bool(true))
            }
            _ => false,
        }
}
