//! Same-rate, single-level Resolve timeline and compound nests.
use crate::{linked::TimelineSpec, Result};
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Rational, Value,
};
use std::collections::{HashMap, HashSet};

pub(crate) const ADAPTER: &str = "resolve.nested_uses";
pub(crate) const PREMIERE_ADAPTER: &str = "premiere.nested_uses";
pub(crate) const PREMIERE_LENGTH: &str = "nest_record_length_frames";
pub(crate) const EMPTY_NEST_AUDIO_LOSS: &str = "nest_audio_editability";
pub(crate) const STEREO_NEST_EDGE_LOSS: &str = "nest_audio_edge_approximation";
pub(crate) const PROFILE: &str = "resolve.picture_nest_24";
pub(crate) const STEREO_PROFILE: &str = "resolve.stereo_nest_24";
const MAX_NEST_ROWS: usize = 4096;
pub(crate) const LOSS: &str =
    "Resolve nested sequence retained in graph; destination mapping is unqualified";
pub(crate) const PREMIERE_LOSS: &str =
    "Premiere nested sequence retained in graph; destination mapping is unqualified";
pub(crate) const ITEM_FIELDS: &str =
    "0000000200000017800a02200112100000000000000006ffffffffffffffff";
const PAIR_PREFIX: &[u8] = b"\x0a\x65\x0a\x61\x0a\x5f\0\0\0\x01\0\0\0\x01\0\0\0\x02\0\x30\0\0\0\x0a\0\0\0\0\x48";
const PAIR_VIDEO_END: &[u8] = b"\x20\x01\x12\x10\0\0\0\0\0\0\0\x06\xff\xff\xff\xff\xff\xff\xff\xff";
const PAIR_AUDIO_END: &[u8] = b"\x20\x01\x78\x04";

fn pair_end(audio: bool) -> &'static [u8] {
    if audio { PAIR_AUDIO_END } else { PAIR_VIDEO_END }
}

pub(crate) fn paired_id(fields: &[u8], audio: bool) -> Option<String> {
    let id = fields.strip_prefix(PAIR_PREFIX)?.strip_suffix(pair_end(audio))?;
    if id.len() != 72 {
        return None;
    }
    let id = String::from_utf16(&id.chunks_exact(2)
        .map(|b| u16::from_be_bytes([b[0], b[1]])).collect::<Vec<_>>()).ok()?;
    (uuid::Uuid::parse_str(&id).ok()?.to_string() == id).then_some(id)
}

pub(crate) fn pair_fields(id: &str, audio: bool) -> Vec<u8> {
    let mut fields = PAIR_PREFIX.to_vec();
    fields.extend(id.encode_utf16().flat_map(u16::to_be_bytes));
    fields.extend(pair_end(audio));
    fields
}

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
    pub(crate) empty_audio_companion: Option<String>,
    pub(crate) stereo_audio: Option<String>,
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
        if let Some(data) = graph.sidecar(PREMIERE_ADAPTER, &parent_spec.id) {
            let parent = sequences.get(parent_spec.id.as_str())
                .ok_or("nested Premiere parent sequence is absent")?;
            if graph.sidecar(ADAPTER, &parent.id).is_some()
                || parent_spec.group.is_some()
                || premiere_extent(graph, parent).is_none()
            {
                return Err("unqualified Premiere nesting parent".into());
            }
            let Value::Arr(rows) = data else {
                return Err("invalid Premiere nest receipt".into());
            };
            if rows.is_empty() || rows.len() > MAX_NEST_ROWS {
                return Err("unsupported Premiere nest use count".into());
            }
            let stereo = rows.iter().any(|row| row.get("stereo_audio_companion").is_some());
            if stereo && (rows.len() > MAX_NEST_ROWS / 2
                || rows.iter().any(|row| row.get("stereo_audio_companion").is_none()))
            {
                return Err("Premiere audible nesting requires a stereo companion for every nest".into());
            }
            for row in rows {
                let id = string(row, "id").ok_or("Premiere nest occurrence lacks an ID")?;
                let refuse = || format!("{id}: invalid Premiere nest child, origin or trim");
                let child_id = string(row, "child_sequence").ok_or_else(refuse)?;
                let child = *selected.get(child_id).ok_or_else(refuse)?;
                let seq = sequences.get(child_id).ok_or_else(refuse)?;
                let (origin, child_length) = premiere_extent(graph, seq).ok_or_else(refuse)?;
                let start = int(row, "record_start_frames").filter(|n| *n >= 0).ok_or_else(refuse)?;
                let length = int(row, "record_duration_frames").filter(|n| *n > 0).ok_or_else(refuse)?;
                let source_in = int(row, "saved_source_in_frames").filter(|n| *n >= 0).ok_or_else(refuse)?;
                let empty_audio_companion = match row.get("empty_audio_companion") {
                    None => None,
                    Some(Value::Str(id)) if !id.is_empty() && ids.insert(id.as_str()) => Some(id.clone()),
                    _ => return Err(refuse().into()),
                };
                let stereo_audio = match row.get("stereo_audio_companion") {
                    None => None,
                    Some(Value::Str(audio)) if empty_audio_companion.is_none()
                        && !audio.is_empty() && ids.insert(audio.as_str()) => Some(audio.clone()),
                    _ => return Err(refuse().into()),
                };
                if id.is_empty()
                    || !ids.insert(id)
                    || spec[child].group.is_some()
                    || graph.sidecar(PREMIERE_ADAPTER, child_id).is_some()
                    || graph.sidecar(ADAPTER, child_id).is_some()
                    || int(row, "child_length_frames") != Some(child_length)
                    || int(row, "child_origin_frames") != Some(origin)
                    || source_in.checked_add(length).is_none_or(|end| end > child_length)
                    || start.checked_add(length).is_none()
                {
                    return Err(refuse().into());
                }
                all[parent_index].push(Use {
                    id: id.into(), kind: Kind::Timeline, child, origin, child_length, start, length, source_in,
                    empty_audio_companion, stereo_audio,
                });
            }
            all[parent_index].sort_by_key(|usage| usage.start);
            continue;
        }
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
            || rows.len() > MAX_NEST_ROWS
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
        let pairs = stereo_pairs(rows)?;
        for row in rows {
            let id = string(row, "id").ok_or("Resolve nest occurrence lacks an ID")?;
            if !pairs.is_empty() && string(row, "track_kind") == Some("audio") {
                continue;
            }
            let stereo = pairs.get(id).copied();
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
                || string(row, "profile") != Some(if stereo.is_some() { STEREO_PROFILE } else { PROFILE })
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
                || stereo.is_some_and(|audio| kind != Kind::Timeline || !ids.insert(audio))
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
                empty_audio_companion: None,
                stereo_audio: stereo.map(str::to_owned),
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

pub(crate) fn stereo_pairs(rows: &[Value]) -> Result<HashMap<&str, &str>> {
    if !rows.iter().any(|r| string(r, "profile") == Some(STEREO_PROFILE)) {
        return Ok(HashMap::new());
    }
    let refuse = || "Resolve stereo nest requires matching, reciprocally linked V1/A1 pairs";
    let mut by_id = HashMap::new();
    for row in rows {
        let id = string(row, "id").filter(|id| !id.is_empty()).ok_or_else(refuse)?;
        if string(row, "profile") != Some(STEREO_PROFILE)
            || int(row, "track") != Some(1)
            || by_id.insert(id, row).is_some()
        {
            return Err(refuse().into());
        }
    }
    let mut pairs = HashMap::new();
    for (&id, &video) in &by_id {
        if string(video, "track_kind") != Some("video") {
            continue;
        }
        let audio_id = string(video, "paired_occurrence").ok_or_else(refuse)?;
        let audio = by_id.get(audio_id).ok_or_else(refuse)?;
        if string(audio, "track_kind") != Some("audio")
            || string(audio, "paired_occurrence") != Some(id)
            || !stereo_selector(audio)
            || ["kind", "child_sequence", "pool_id", "child_rate", "record_rate", "child_origin_frames",
                "record_start_frames", "record_duration_frames", "saved_source_in_frames"]
                .into_iter().any(|field| video.get(field) != audio.get(field))
        {
            return Err(refuse().into());
        }
        pairs.insert(id, audio_id);
    }
    if pairs.len() * 2 != rows.len() {
        return Err(refuse().into());
    }
    Ok(pairs)
}

pub(crate) fn stereo_selector(row: &Value) -> bool {
    int(row, "saved_media_track_index") == Some(0)
        && string(row, "saved_virtual_audio_track") == Some(crate::stereo::SELECTOR)
        && matches!(row.get("saved_child_mix_channels"), Some(Value::Arr(channels))
            if channels == &[Value::Int(1), Value::Int(2)])
        && row.get("saved_clip_volume_db").is_none()
}

fn clock(sequence: &SourceSequence) -> bool {
    sequence.rate == Rational::integer(24)
        && sequence
            .clock
            .as_ref()
            .is_some_and(|c| c.provenance.route == crate::source::CLOCK_ROUTE)
}

fn premiere_extent(graph: &SourceGraph, sequence: &SourceSequence) -> Option<(i64, i64)> {
    let clock = sequence.clock.as_ref()?;
    if sequence.rate != Rational::integer(24)
        || sequence.frame_size != Some((1920, 1080))
        || clock.provenance.route != "Premiere sequence ticks"
    {
        return None;
    }
    let source = graph.sidecar("premiere.source", &sequence.id)?;
    let length = int(source, PREMIERE_LENGTH).filter(|n| *n > 0)?;
    Some((crate::linked::check_origin(sequence).ok()?, length))
}

pub(crate) fn matches_extent(graph: &SourceGraph, sequence: &SourceSequence, origin: i64, length: i64) -> bool {
    if let Some(receipt) = graph.sidecar(crate::source::ADAPTER, &sequence.id) {
        int(receipt, "record_length") == Some(length)
            && int(receipt, "record_origin") == Some(origin)
    } else {
        premiere_extent(graph, sequence) == Some((origin, length))
    }
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
            .is_some_and(|(id, reason)| {
                matches!(reason, LOSS | PREMIERE_LOSS)
                    && uses.iter().any(|usage| usage.id == id || usage.stereo_audio.as_deref() == Some(id))
            })
}

pub(crate) fn recover_empty_stereo(graph: &SourceGraph, loss: &Loss, owner: &str) -> bool {
    compound(graph, owner) && crate::source::default_empty_stereo(graph, owner)
        .is_some_and(|track| crate::source::is_empty_stereo_loss(loss, owner, track))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_nest_fields_identify_the_opposite_edit() {
        let opposite = "01234567-89ab-4cde-8123-456789abcdef";
        for (audio, size) in [(false, 121), (true, 105)] {
            let mut fields = pair_fields(opposite, audio);
            assert_eq!(fields.len(), size);
            assert_eq!(paired_id(&fields, audio).as_deref(), Some(opposite));
            assert!(paired_id(&fields, !audio).is_none());
            fields[PAIR_PREFIX.len()] = 1;
            assert!(paired_id(&fields, audio).is_none());
        }
    }

    #[test]
    fn repeated_stereo_pairs_match_by_occurrence_and_trim() {
        let row = |id: &str, opposite: &str, kind: &str, trim| {
            let mut fields = vec![
                ("id", Value::Str(id.into())),
                ("profile", Value::Str(STEREO_PROFILE.into())),
                ("track", Value::Int(1)),
                ("track_kind", Value::Str(kind.into())),
                ("paired_occurrence", Value::Str(opposite.into())),
                ("saved_source_in_frames", Value::Int(trim)),
            ];
            if kind == "audio" {
                fields.extend([
                    ("saved_media_track_index", Value::Int(0)),
                    ("saved_virtual_audio_track", Value::Str(crate::stereo::SELECTOR.into())),
                    ("saved_child_mix_channels", Value::Arr(vec![Value::Int(1), Value::Int(2)])),
                ]);
            }
            Value::Obj(fields.into_iter().map(|(key, value)| (key.into(), value)).collect())
        };
        let mut rows = [
            row("v1", "a1", "video", 3),
            row("v2", "a2", "video", 15),
            row("a2", "v2", "audio", 15),
            row("a1", "v1", "audio", 3),
        ];
        assert_eq!(stereo_pairs(&rows).unwrap(), HashMap::from([("v1", "a1"), ("v2", "a2")]));
        rows[2] = row("a2", "v2", "audio", 16);
        assert!(stereo_pairs(&rows).is_err());
        rows[2] = row("a2", "v1", "audio", 15);
        assert!(stereo_pairs(&rows).is_err());
        assert!(stereo_pairs(&rows[..3]).is_err());
    }

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
