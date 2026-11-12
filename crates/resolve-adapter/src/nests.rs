//! Same-rate, single-level Resolve picture nests.
use crate::{linked::TimelineSpec, Result};
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Rational, Value,
};
use std::collections::{HashMap, HashSet};

pub(crate) const ADAPTER: &str = "resolve.nested_uses";
pub(crate) const PROFILE: &str = "resolve.picture_nest_24";
pub(crate) const LOSS: &str =
    "Resolve nested sequence retained in graph; destination mapping is unqualified";
pub(crate) const EMPTY_STEREO: &str = "unqualified Resolve track state or layout";
pub(crate) const ITEM_FIELDS: &str =
    "0000000200000017800a02200112100000000000000006ffffffffffffffff";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Timeline,
    Compound,
}

impl Kind {
    pub(crate) fn saved(self) -> &'static str {
        match self {
            Self::Timeline => "timeline",
            Self::Compound => "compound",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Use {
    pub(crate) id: String,
    pub(crate) kind: Kind,
    pub(crate) child: usize,
    pub(crate) origin: i64,
    pub(crate) child_length: i64,
    pub(crate) start: i64,
    pub(crate) length: i64,
    pub(crate) source_in: i64,
}

pub(crate) fn string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    match value.get(field)? {
        Value::Str(v) => Some(v),
        _ => None,
    }
}

pub(crate) fn int(value: &Value, field: &str) -> Option<i64> {
    match value.get(field)? {
        Value::Int(n) => Some(*n),
        _ => None,
    }
}

pub(crate) fn compact_map(frames: i64) -> [u8; 9] {
    let mut map = [2; 9];
    map[1..].copy_from_slice(&((frames - 1) as f64 / 24.0).to_be_bytes());
    map
}

pub(crate) fn identity_map(encoded: &str, frames: i64) -> bool {
    if encoded.len() != 18 || frames <= 0 {
        return false;
    }
    let mut bytes = [0; 9];
    if hex::decode_to_slice(encoded, &mut bytes).is_err() || bytes[0] != 2 {
        return false;
    }
    let seconds = f64::from_be_bytes(bytes[1..].try_into().unwrap());
    (seconds * 24.0 - (frames - 1) as f64).abs() < 1e-8
}

pub(crate) fn plan(graph: &SourceGraph, spec: &[TimelineSpec]) -> Result<Vec<Vec<Use>>> {
    let selected: HashMap<_, _> = spec
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    let sequences: HashMap<_, _> = graph.sequences.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut all = vec![Vec::new(); spec.len()];
    let mut referenced = HashSet::new();
    let mut owned_pool = HashMap::new();
    let mut ids = HashSet::new();
    for (parent_index, parent_spec) in spec.iter().enumerate() {
        let Some(data) = graph.sidecar(ADAPTER, &parent_spec.id) else {
            continue;
        };
        let parent = sequences
            .get(parent_spec.id.as_str())
            .ok_or("nested parent sequence is absent")?;
        let Value::Arr(rows) = data else {
            return Err("invalid Resolve nest receipt".into());
        };
        if rows.is_empty()
            || rows.len() > 4096
            || !clock(parent)
            || graph
                .sidecar(crate::source::ADAPTER, &parent.id)
                .and_then(|owner| string(owner, "kind"))
                != Some("registered_timeline")
            || parent_spec.group.is_some()
        {
            return Err(format!(
                "{}: unsupported Resolve nesting parent or use count",
                parent.id
            )
            .into());
        }
        for row in rows {
            let id = string(row, "id").ok_or("Resolve nest occurrence lacks an ID")?;
            let refuse = |why| format!("Resolve nest {id}: {why}");
            let child_id =
                string(row, "child_sequence").ok_or_else(|| refuse("missing child sequence ID"))?;
            let child = *selected.get(child_id).ok_or_else(|| {
                refuse("select its exact child sequence ID in the project manifest")
            })?;
            let seq = sequences
                .get(child_id)
                .ok_or_else(|| refuse("child sequence is absent"))?;
            let owner = graph
                .sidecar(crate::source::ADAPTER, child_id)
                .ok_or_else(|| refuse("child owner receipt is absent"))?;
            let kind = match (string(row, "kind"), string(owner, "kind")) {
                (Some("compound"), Some("compound")) => Kind::Compound,
                (Some("timeline"), Some("registered_timeline")) => Kind::Timeline,
                _ => return Err(refuse("child kind disagrees with its saved owner").into()),
            };
            let pool = string(owner, "media_pool_clip")
                .filter(|s| !s.is_empty())
                .ok_or_else(|| refuse("child pool identity is absent"))?;
            let origin = int(row, "child_origin_frames")
                .filter(|n| *n >= 0)
                .ok_or_else(|| refuse("child origin is absent"))?;
            let child_length = int(owner, "record_length")
                .filter(|n| *n > 0)
                .ok_or_else(|| refuse("child length is absent"))?;
            let start = int(row, "record_start_frames")
                .filter(|n| *n >= 0)
                .ok_or_else(|| refuse("record start is absent"))?;
            let length = int(row, "record_duration_frames")
                .filter(|n| *n > 0)
                .ok_or_else(|| refuse("record duration is absent"))?;
            let source_in = int(row, "saved_source_in_frames")
                .filter(|n| *n >= 0)
                .ok_or_else(|| refuse("child trim is absent"))?;
            if id.is_empty()
                || !ids.insert(id)
                || !clock(seq)
                || spec[child].group.is_some()
                || graph.sidecar(ADAPTER, child_id).is_some()
                || string(row, "profile") != Some(PROFILE)
                || string(row, "track_kind") != Some("video")
                || int(row, "track") != Some(1)
                || string(row, "child_rate") != Some("24")
                || string(row, "record_rate") != Some("24")
                || string(row, "pool_id") != Some(pool)
                || owned_pool
                    .insert(pool, child)
                    .is_some_and(|prior| prior != child)
                || int(owner, "record_origin") != Some(origin)
                || seq
                    .clock
                    .as_ref()
                    .and_then(|c| i64::try_from(c.effective_count).ok())
                    != Some(origin)
                || (kind == Kind::Compound
                    && owner.get("container_ref_agrees") != Some(&Value::Bool(true)))
                || source_in
                    .checked_add(length)
                    .is_none_or(|end| end > child_length)
                || start.checked_add(length).is_none()
            {
                return Err(refuse(
                    "unqualified picture nest, inconsistent child identity, clock or trim",
                )
                .into());
            }
            referenced.insert(child);
            all[parent_index].push(Use {
                id: id.into(),
                kind,
                child,
                origin,
                child_length,
                start,
                length,
                source_in,
            });
        }
        all[parent_index].sort_by_key(|r| r.start);
    }
    for (index, row) in spec.iter().enumerate() {
        if compound(graph, &row.id) && !referenced.contains(&index) {
            return Err(format!(
                "{}: a compound must be referenced by a selected parent",
                row.id
            )
            .into());
        }
    }
    Ok(all)
}

fn clock(sequence: &SourceSequence) -> bool {
    sequence.rate == Rational::integer(24)
        && sequence
            .clock
            .as_ref()
            .is_some_and(|c| c.provenance.route == crate::source::CLOCK_ROUTE)
}

pub(crate) fn compound(graph: &SourceGraph, sequence: &str) -> bool {
    graph
        .sidecar(crate::source::ADAPTER, sequence)
        .and_then(|r| string(r, "kind"))
        == Some("compound")
}

pub(crate) fn recover(loss: &Loss, owner: &str, uses: &[Use]) -> bool {
    loss.object == owner
        && loss.aspect == "placement"
        && loss
            .reason
            .split_once(": ")
            .is_some_and(|(id, reason)| reason == LOSS && uses.iter().any(|usage| usage.id == id))
}

pub(crate) fn recover_empty_stereo(graph: &SourceGraph, loss: &Loss, owner: &str) -> bool {
    if loss.object != owner || loss.aspect != "track" || !compound(graph, owner) {
        return false;
    }
    let Some((id, reason)) = loss.reason.split_once(": ") else {
        return false;
    };
    reason == EMPTY_STEREO
        && matches!(
            graph.sidecar("resolve.omitted_tracks",owner),
            Some(Value::Arr(rows)) if matches!(rows.as_slice(), [row] if string(row,"id") == Some(id)
                && string(row,"kind") == Some("audio") && int(row,"track") == Some(1)
                && row.get("empty_default_stereo") == Some(&Value::Bool(true)))
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_map_uses_child_extent_instead_of_trimmed_parent_duration() {
        let full_child = hex::encode(compact_map(24));
        assert_eq!(full_child, "023feeaaaaaaaaaaab");
        assert!(identity_map(&full_child, 24));
        assert!(!identity_map(&full_child, 10));
        // Native floating arithmetic for a 20-frame child can differ by one ULP.
        assert!(identity_map("023fe9555555555556", 20));
        for bad in ["02", "027ff8000000000000", "013feeaaaaaaaaaaab"] {
            assert!(!identity_map(bad, 24));
        }
    }
}
