//! Pair Resolve multicam occurrences independently of their trimmed file edits.
use super::{exact, Diff, Timeline};
use crate::source::multicam::selection::{self, Selection, OBSERVED_GAP};
use graph::{
    source::{Multicam, MulticamUse},
    Placement, Rational,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

type Slot = (&'static str, i64, i64, i64);

#[derive(Default)]
pub(super) struct Comparison<'a> {
    pub left_leaves: HashSet<&'a str>,
    pub right_leaves: HashSet<&'a str>,
    pub left_gaps: HashSet<&'a str>,
    pub right_gaps: HashSet<&'a str>,
    pub leaf_pairs: Vec<(&'a Placement, &'a Placement)>,
}

pub(super) fn compare<'a>(
    left: &Timeline<'a>,
    right: &Timeline<'a>,
    diff: &mut Diff,
) -> Comparison<'a> {
    let mut out = Comparison::default();
    let a = Uses::new(left);
    let b = Uses::new(right);
    if a.by_id.is_empty() && b.by_id.is_empty() {
        return out;
    }
    out.left_leaves.extend(a.claims.keys().copied());
    out.right_leaves.extend(b.claims.keys().copied());
    let mut paired_left = HashSet::new();
    let mut paired_right = HashSet::new();
    let mut pair = |old: &'a MulticamUse, new: &'a MulticamUse, diff: &mut Diff| {
        compare_use(&a, old, &b, new, &mut out, diff);
    };
    for (&id, &old) in &a.by_id {
        if let Some(&new) = b.by_id.get(id).filter(|new| new.kind == old.kind) {
            pair(old, new, diff);
            paired_left.insert(id);
            paired_right.insert(id);
        }
    }
    let slots: BTreeSet<_> = a.slots.keys().chain(b.slots.keys()).copied().collect();
    for key @ (kind, track, num, den) in slots {
        let left = a.slots.get(&key).map(Vec::as_slice).unwrap_or_default();
        let right = b.slots.get(&key).map(Vec::as_slice).unwrap_or_default();
        let old = left
            .iter()
            .copied()
            .find(|u| !paired_left.contains(u.id.as_str()));
        let new = right
            .iter()
            .copied()
            .find(|u| !paired_right.contains(u.id.as_str()));
        if old.is_none() && new.is_none() {
            continue;
        }
        let context = json!({"kind":kind, "track":track,
            "record_start":exact(Rational::new(num.into(), den.into()).unwrap())});
        match (old, new) {
            (Some(old), Some(new)) if left.len() == 1 && right.len() == 1 => {
                pair(old, new, diff);
            }
            _ => diff.gap(
                "multicam",
                &context,
                "saved multicam occurrence could not be paired",
            ),
        }
    }
    for (side, uses) in [("left", &a), ("right", &b)] {
        for usage in uses.by_id.values().filter(|u| u.record_start.is_none()) {
            diff.gap(
                "multicam",
                &json!({"side":side, "id":usage.id}),
                "unknown multicam record start",
            );
        }
    }
    out
}

struct Uses<'a, 't> {
    timeline: &'t Timeline<'a>,
    by_id: BTreeMap<&'a str, &'a MulticamUse>,
    slots: BTreeMap<Slot, Vec<&'a MulticamUse>>,
    claims: HashMap<&'a str, usize>,
    groups: HashMap<&'a str, &'a Multicam>,
    placements: HashMap<&'a str, &'a Placement>,
}

impl<'a, 't> Uses<'a, 't> {
    fn new(timeline: &'t Timeline<'a>) -> Self {
        let mut uses = Self {
            timeline,
            by_id: BTreeMap::new(),
            slots: BTreeMap::new(),
            claims: HashMap::new(),
            groups: HashMap::new(),
            placements: HashMap::new(),
        };
        for usage in timeline
            .graph
            .multicam_uses
            .iter()
            .filter(|u| u.sequence == timeline.seq.id)
        {
            uses.by_id.insert(&usage.id, usage);
            if let Some(start) = usage.record_start {
                uses.slots
                    .entry((usage.kind.as_str(), usage.track, start.num(), start.den()))
                    .or_default()
                    .push(usage);
            }
            for id in &usage.binding.placements {
                *uses.claims.entry(id.as_str()).or_default() += 1;
            }
        }
        if !uses.by_id.is_empty() {
            uses.groups = timeline
                .graph
                .multicams
                .iter()
                .map(|g| (g.id.as_str(), g))
                .collect();
            uses.placements = timeline
                .seq
                .placements
                .iter()
                .map(|p| (p.id.as_str(), p))
                .collect();
        }
        uses
    }

    fn selection(&self, usage: &MulticamUse) -> Option<(Selection<'a>, Option<&'a Placement>)> {
        let [group] = usage.candidates.as_slice() else {
            return None;
        };
        let rate = self.timeline.seq.rate;
        let record = selection::range(usage.record_start?, usage.duration?, rate)?;
        let selected = selection::select(
            usage,
            self.groups.get(group.as_str())?,
            rate,
            self.timeline.receipts.get(group.as_str())?,
            self.timeline.receipts.get(usage.id.as_str())?,
            &record,
        )?;
        let placement = match (&selected.leaf, usage.binding.placements.as_slice()) {
            (None, []) => None,
            (Some(_), [id]) if self.claims.get(id.as_str()) == Some(&1) => {
                let p = *self.placements.get(id.as_str())?;
                let visible = selection::range(p.record_start, p.duration, rate)?;
                if !selected.agrees(usage, p, &visible) {
                    return None;
                }
                Some(p)
            }
            _ => return None,
        };
        Some((selected, placement))
    }
}

fn compare_use<'a>(
    a: &Uses<'a, '_>,
    old: &'a MulticamUse,
    b: &Uses<'a, '_>,
    new: &'a MulticamUse,
    out: &mut Comparison<'a>,
    diff: &mut Diff,
) {
    let context = json!({"kind":old.kind.as_str(), "track":old.track,
        "left_use_id":old.id, "right_use_id":new.id});
    diff.change(
        "multicam.track",
        &context,
        json!(old.track),
        json!(new.track),
    );
    for (field, left, right) in [
        ("multicam.record_start", old.record_start, new.record_start),
        ("multicam.duration", old.duration, new.duration),
        ("multicam.source_in", old.source_in, new.source_in),
    ] {
        super::known(diff, field, &context, left.map(exact), right.map(exact));
    }
    let left = a.selection(old);
    let right = b.selection(new);
    for (side, selection) in [("left", &left), ("right", &right)] {
        if selection.is_none() {
            let mut context = context.clone();
            context["side"] = json!(side);
            diff.gap(
                "multicam.binding",
                &context,
                "saved selection or file edit is unresolved",
            );
        }
    }
    let (Some((left, lp)), Some((right, rp))) = (left, right) else {
        return;
    };
    if old.binding.reason.as_deref() == Some(OBSERVED_GAP) {
        out.left_gaps.insert(&old.id);
    }
    if new.binding.reason.as_deref() == Some(OBSERVED_GAP) {
        out.right_gaps.insert(&new.id);
    }
    diff.change(
        "multicam.angle",
        &context,
        json!(left.angle),
        json!(right.angle),
    );
    diff.change(
        "multicam.audio_selector",
        &context,
        json!(left.audio_selector),
        json!(right.audio_selector),
    );
    match (lp, rp) {
        (Some(lp), Some(rp)) => out.leaf_pairs.push((lp, rp)),
        (None, None) => {}
        (lp, rp) => diff.change(
            "multicam.media_coverage",
            &context,
            visible(lp),
            visible(rp),
        ),
    }
}

fn visible(placement: Option<&Placement>) -> Value {
    placement.map_or(Value::Null, super::edit_summary)
}
