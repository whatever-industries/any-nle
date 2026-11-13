//! Compare saved V1 nesting occurrences in the selected parent timelines.
use super::{check_id, Diff, Timeline};
use crate::nests::{int, string, ADAPTER, LOSS, PROFILE};
use graph::{Loss, Rational, Value};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet, HashSet};

struct Nest<'a> {
    id: &'a str,
    kind: &'a str,
    child: &'a str,
    pool: &'a str,
    start: i64,
    duration: i64,
    source_in: i64,
    origin: i64,
}

impl<'a> Nest<'a> {
    fn read(row: &'a Value, parent_length: i64) -> Option<Self> {
        if string(row, "profile") != Some(PROFILE)
            || string(row, "track_kind") != Some("video")
            || int(row, "track") != Some(1)
            || string(row, "record_rate") != Some("24")
            || string(row, "child_rate") != Some("24")
        {
            return None;
        }
        let id = string(row, "id")?;
        let child = string(row, "child_sequence")?;
        let pool = string(row, "pool_id")?;
        check_id(id).ok()?;
        check_id(child).ok()?;
        check_id(pool).ok()?;
        let kind = string(row, "kind")?;
        if !matches!(kind, "timeline" | "compound") {
            return None;
        }
        let start = int(row, "record_start_frames").filter(|&n| n >= 0)?;
        let duration = int(row, "record_duration_frames").filter(|&n| n > 0)?;
        if start.checked_add(duration)? > parent_length {
            return None;
        }
        let source_in = int(row, "saved_source_in_frames").filter(|&n| n >= 0)?;
        let origin = int(row, "child_origin_frames").filter(|&n| n >= 0)?;
        Some(Self {
            id,
            kind,
            child,
            pool,
            start,
            duration,
            source_in,
            origin,
        })
    }

    fn summary(&self) -> Json {
        json!({"id":self.id, "kind":self.kind, "child_sequence":self.child,
            "record_start_frames":self.start, "record_duration_frames":self.duration,
            "source_in_frames":self.source_in, "child_origin_frames":self.origin})
    }
}

#[derive(Default)]
struct Saved<'a> {
    by_id: BTreeMap<&'a str, Nest<'a>>,
    by_start: BTreeMap<i64, Vec<&'a str>>,
    incomplete: bool,
}

impl<'a> Saved<'a> {
    fn read(timeline: &Timeline<'a>) -> Self {
        let mut saved = Self::default();
        let Some(data) = timeline.graph.sidecar(ADAPTER, &timeline.seq.id) else {
            return saved;
        };
        let Value::Arr(rows) = data else {
            saved.incomplete = true;
            return saved;
        };
        if timeline.seq.rate != Rational::integer(24) {
            saved.incomplete = true;
            return saved;
        }
        let Some(parent_length) = timeline
            .receipt(&timeline.seq.id, "record_length")
            .and_then(|n| match n {
                Value::Int(n) => Some(*n),
                _ => None,
            })
        else {
            saved.incomplete = true;
            return saved;
        };
        let mut seen = HashSet::new();
        for row in rows {
            if let Some(id) = string(row, "id") {
                if !seen.insert(id) {
                    saved.by_id.remove(id);
                    saved.incomplete = true;
                    continue;
                }
            }
            let Some(nest) = Nest::read(row, parent_length) else {
                saved.incomplete = true;
                continue;
            };
            saved.by_id.insert(nest.id, nest);
        }
        for nest in saved.by_id.values() {
            saved.by_start.entry(nest.start).or_default().push(nest.id);
        }
        saved
    }
}

pub(super) fn compare<'a>(
    left: &Timeline<'a>,
    right: &Timeline<'a>,
    diff: &mut Diff,
) -> (HashSet<&'a str>, HashSet<&'a str>) {
    let a = Saved::read(left);
    let b = Saved::read(right);
    let left_ids: HashSet<_> = a.by_id.keys().copied().collect();
    let right_ids: HashSet<_> = b.by_id.keys().copied().collect();
    if a.by_id.is_empty() && b.by_id.is_empty() && !a.incomplete && !b.incomplete {
        return (left_ids, right_ids);
    }
    for (side, saved) in [("left", &a), ("right", &b)] {
        if saved.incomplete {
            diff.gap(
                "nest",
                &json!({"side":side}),
                "unqualified or ambiguous saved nesting occurrence",
            );
        }
    }
    let a_complete = !a.incomplete && !super::may_omit_edits(left, &left_ids);
    let b_complete = !b.incomplete && !super::may_omit_edits(right, &right_ids);
    let a_children: HashSet<_> = left.graph.sequences.iter().map(|s| s.id.as_str()).collect();
    let b_children: HashSet<_> = right
        .graph
        .sequences
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    let pair = |a: &Nest<'_>, b: &Nest<'_>, diff: &mut Diff| {
        let context = json!({"left_nest_id":a.id, "right_nest_id":b.id});
        diff.change("nest.kind", &context, json!(a.kind), json!(b.kind));
        diff.change(
            "nest.record_start_frames",
            &context,
            json!(a.start),
            json!(b.start),
        );
        diff.change(
            "nest.record_duration_frames",
            &context,
            json!(a.duration),
            json!(b.duration),
        );
        diff.change(
            "nest.source_in_frames",
            &context,
            json!(a.source_in),
            json!(b.source_in),
        );
        diff.change(
            "nest.child_origin_frames",
            &context,
            json!(a.origin),
            json!(b.origin),
        );
        if !a_children.contains(a.child)
            || !b_children.contains(b.child)
            || !child_owner_agrees(left, a)
            || !child_owner_agrees(right, b)
        {
            diff.gap(
                "nest.child_sequence",
                &context,
                "saved child sequence or its pool binding is unresolved",
            );
        } else if a.child != b.child {
            if a_children.contains(b.child) || b_children.contains(a.child) {
                diff.change(
                    "nest.child_sequence",
                    &context,
                    json!(a.child),
                    json!(b.child),
                );
            } else {
                let context = json!({"left_nest_id":a.id, "right_nest_id":b.id,
                    "left_child_sequence":a.child, "right_child_sequence":b.child});
                diff.gap(
                    "nest.child_sequence",
                    &context,
                    "child IDs differ across archives and cannot be paired by identity",
                );
            }
        }
    };
    for (id, left_nest) in &a.by_id {
        if let Some(right_nest) = b.by_id.get(id) {
            pair(left_nest, right_nest, diff);
        }
    }
    let slots: BTreeSet<_> = a
        .by_start
        .keys()
        .chain(b.by_start.keys())
        .copied()
        .collect();
    let direct_starts = |timeline: &Timeline<'_>| -> HashSet<i64> {
        timeline
            .seq
            .placements
            .iter()
            .filter_map(|p| {
                (p.video_track == Some(1))
                    .then_some(p.record_start)
                    .and_then(|start| start.to_units(24))
            })
            .collect()
    };
    let a_direct = direct_starts(left);
    let b_direct = direct_starts(right);
    for start in slots {
        let left_slot = a
            .by_start
            .get(&start)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let right_slot = b
            .by_start
            .get(&start)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let left_open = left_slot.iter().any(|id| !right_ids.contains(id));
        let right_open = right_slot.iter().any(|id| !left_ids.contains(id));
        if !left_open && !right_open {
            continue;
        }
        let context = json!({"track":1,"record_start_frames":start});
        if left_slot.len() > 1
            || right_slot.len() > 1
            || (!left_slot.is_empty() && a_direct.contains(&start))
            || (!right_slot.is_empty() && b_direct.contains(&start))
        {
            diff.gap(
                "nest",
                &context,
                "multiple edits occupy the same nesting slot",
            );
            continue;
        }
        match (
            left_slot.first().filter(|_| left_open),
            right_slot.first().filter(|_| right_open),
        ) {
            (Some(a_id), Some(b_id)) if a_complete && b_complete => {
                pair(&a.by_id[a_id], &b.by_id[b_id], diff)
            }
            (Some(id), None) if b_complete => {
                diff.change("nest", &context, a.by_id[id].summary(), Json::Null)
            }
            (None, Some(id)) if a_complete => {
                diff.change("nest", &context, Json::Null, b.by_id[id].summary())
            }
            _ => diff.gap(
                "nest",
                &context,
                "a saved nested use could not be paired with certainty",
            ),
        }
    }
    (left_ids, right_ids)
}

fn child_owner_agrees(timeline: &Timeline<'_>, nest: &Nest<'_>) -> bool {
    let Some(owner) = timeline.receipts.get(nest.child) else {
        return false;
    };
    matches!(
        (nest.kind, string(owner, "kind")),
        ("timeline", Some("registered_timeline")) | ("compound", Some("compound"))
    ) && string(owner, "media_pool_clip") == Some(nest.pool)
        && int(owner, "record_origin") == Some(nest.origin)
        && int(owner, "record_length").is_some_and(|length| {
            nest.source_in
                .checked_add(nest.duration)
                .is_some_and(|end| end <= length)
        })
        && (nest.kind != "compound"
            || owner.get("container_ref_agrees") == Some(&Value::Bool(true)))
}

pub(super) fn compared_loss(timeline: &Timeline<'_>, loss: &Loss, ids: &HashSet<&str>) -> bool {
    loss.object == timeline.seq.id
        && loss.aspect == "placement"
        && loss
            .reason
            .split_once(": ")
            .is_some_and(|(id, reason)| reason == LOSS && ids.contains(id))
}
