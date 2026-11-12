//! Read-only comparisons of selected Resolve timeline facts and saved bytes.
mod blob;

use super::{archive_is_drt, drt, read, report, ADAPTER};
use crate::{audio, dissolve, Result};
use graph::{
    source::{SourceClock, SourceGraph, SourceItem, SourceMedia, SourceSequence},
    Loss, MarkerOwner, Placement, Rational, StreamKind, Value,
};
use serde_json::{json, Value as Json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::Write,
    path::Path,
};

pub fn timelines(
    left: &Path,
    left_id: &str,
    right: &Path,
    right_id: &str,
    writer: &mut impl Write,
) -> Result<()> {
    let left = read_graph(left)?;
    let right = read_graph(right)?;
    let left = Timeline::new(&left, left_id)?;
    let right = Timeline::new(&right, right_id)?;
    let mut diff = Diff::default();
    compare_timelines(&left, &right, &mut diff);
    report::write(writer, &diff.finish(left_id, right_id)?)
}

pub fn blobs(
    left: &Path,
    left_id: &str,
    right: &Path,
    right_id: &str,
    field: &str,
    writer: &mut impl Write,
) -> Result<()> {
    archive_is_drt(left)?;
    archive_is_drt(right)?;
    report::write(
        writer,
        &blob::compare(left, left_id, right, right_id, field)?,
    )
}

fn check_id(id: &str) -> Result<()> {
    if id.len() != 36 || uuid::Uuid::parse_str(id).is_err() {
        return Err("expected an exact Resolve object UUID".into());
    }
    Ok(())
}

fn read_graph(path: &Path) -> Result<SourceGraph> {
    if archive_is_drt(path)? {
        drt::inspect(path)?.into_graph()
    } else {
        read(path)
    }
}

#[derive(Default)]
struct Diff {
    differences: Vec<Json>,
    gaps: Vec<Json>,
    row_bytes: usize,
    over_budget: bool,
}

impl Diff {
    fn change(&mut self, field: &str, context: &Json, left: Json, right: Json) {
        if left != right && self.charge(&[context, &left, &right]) {
            self.differences.push(json!({
                "field":field, "context":context, "left":left, "right":right,
            }));
        }
    }

    fn gap(&mut self, field: &str, context: &Json, reason: &str) {
        if self.charge(&[context, &json!(reason)]) {
            self.gaps
                .push(json!({"field":field, "context":context, "reason":reason}));
        }
    }

    fn charge(&mut self, values: &[&Json]) -> bool {
        if !self.over_budget {
            let bytes = values.iter().map(|v| v.to_string().len()).sum::<usize>();
            // Cap the accumulated report before cloning repeated values into rows.
            self.row_bytes = self.row_bytes.saturating_add(128).saturating_add(bytes);
            self.over_budget = self.row_bytes > (16 << 20);
        }
        !self.over_budget
    }

    fn finish(self, left_id: &str, right_id: &str) -> Result<Json> {
        if self.over_budget {
            return Err("Resolve comparison exceeds report budget".into());
        }
        let status = if !self.differences.is_empty() {
            "different"
        } else if !self.gaps.is_empty() {
            "inconclusive"
        } else {
            "match"
        };
        Ok(json!({
            "schema":"anynle.resolve-timeline-diff/1", "status":status,
            "left_timeline_id":left_id, "right_timeline_id":right_id,
            "differences":self.differences, "gaps":self.gaps,
        }))
    }
}

struct Timeline<'a> {
    graph: &'a SourceGraph,
    seq: &'a SourceSequence,
    items: HashMap<&'a str, &'a SourceItem>,
    media: HashMap<&'a str, &'a SourceMedia>,
    receipts: HashMap<&'a str, &'a Value>,
    losses: Vec<&'a Loss>,
    unknown_effects: HashSet<&'a str>,
    may_omit_edits: bool,
}

impl<'a> Timeline<'a> {
    fn new(graph: &'a SourceGraph, id: &str) -> Result<Self> {
        check_id(id)?;
        let seq = graph
            .sequences
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("Resolve timeline {id} was not found"))?;
        let item_ids: HashSet<_> = seq.placements.iter().map(|p| p.item.as_str()).collect();
        let items: HashMap<_, _> = graph
            .items
            .iter()
            .filter(|i| item_ids.contains(i.id.as_str()))
            .map(|i| (i.id.as_str(), i))
            .collect();
        let media_ids: HashSet<_> = items
            .values()
            .flat_map(|item| item.media.iter().map(|m| m.media.as_str()))
            .collect();
        let uses: HashSet<_> = graph
            .multicam_uses
            .iter()
            .filter(|u| u.sequence == id)
            .map(|u| u.id.as_str())
            .collect();
        let placements: HashSet<_> = seq.placements.iter().map(|p| p.id.as_str()).collect();
        let losses: Vec<_> = graph
            .losses
            .iter()
            .filter(|loss| {
                let owner = loss.object.as_str();
                owner == id
                    || item_ids.contains(owner)
                    || media_ids.contains(owner)
                    || placements.contains(owner)
                    || uses.contains(owner)
            })
            .collect();
        let unknown_effects = losses
            .iter()
            .filter(|loss| loss.aspect == "effect")
            .map(|loss| {
                if loss.object == id {
                    loss.reason
                        .split_once(": ")
                        .filter(|(occurrence, _)| placements.contains(occurrence))
                        .map_or(seq.id.as_str(), |(occurrence, _)| occurrence)
                } else {
                    loss.object.as_str()
                }
            })
            .collect();
        let may_omit_edits = losses.iter().any(|loss| {
            (loss.object == id || uses.contains(loss.object.as_str()))
                && matches!(
                    loss.aspect.as_str(),
                    "track"
                        | "tracks"
                        | "placement"
                        | "media"
                        | "timing"
                        | "audio_routing"
                        | "multicam_binding"
                )
        }) || seq.placements.iter().any(|p| lane(p).is_none());
        Ok(Self {
            graph,
            seq,
            items,
            media: graph
                .media
                .iter()
                .filter(|m| media_ids.contains(m.id.as_str()))
                .map(|m| (m.id.as_str(), m))
                .collect(),
            receipts: graph
                .sidecars
                .iter()
                .filter(|s| s.adapter == ADAPTER)
                .map(|s| (s.object.as_str(), &s.data))
                .collect(),
            losses,
            unknown_effects,
            may_omit_edits,
        })
    }

    fn receipt(&self, id: &str, key: &str) -> Option<&'a Value> {
        self.receipts.get(id)?.get(key)
    }

    fn binding(&self, p: &Placement) -> Option<Json> {
        let (kind, _) = lane(p)?;
        let item = self.items.get(p.item.as_str())?;
        let audio = if kind == "audio" {
            let [stream] = p.audio_streams.as_slice() else {
                return None;
            };
            Some(*stream)
        } else {
            None
        };
        let mut candidates = item.media.iter().filter_map(|r| {
            let m = self.media.get(r.media.as_str())?;
            match (m.kind, audio) {
                (StreamKind::Video, None) => Some((m, r)),
                (StreamKind::Audio, Some(selector)) if m.stream == Some(selector) => Some((m, r)),
                _ => None,
            }
        });
        let (m, r) = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        Some(json!({
            "locators":(!m.locations.is_empty()).then_some(&m.locations), "codec":m.codec,
            "frame_size":m.frame_size,
            "rate":m.rate.map(exact), "sample_rate":m.sample_rate, "bits":m.bits,
            "start":m.start.map(exact), "duration":exact(m.duration), "clock":m.clock.as_ref().map(clock),
            "source_offset":r.source_offset.map(exact),
            "physical_audio":audio.and_then(|label| self.physical_audio(m, label)),
        }))
    }

    fn physical_audio(&self, media: &SourceMedia, label: i64) -> Option<Json> {
        let (track, channel) =
            if let Some(Value::Int(track)) = self.receipt(&media.id, "saved_cache_track") {
                // The PCM reader verifies one physical channel for cache tracks
                // without an explicit channel receipt.
                let channel = match self.receipt(&media.id, "saved_channel") {
                    Some(Value::Int(channel)) => *channel,
                    None => 0,
                    _ => return None,
                };
                (*track, channel)
            } else if media.codec.as_deref() == Some("aac") && matches!(label, 1 | 2) {
                // The AVC reader admits one embedded AAC track with two channels.
                (0, label - 1)
            } else {
                return None;
            };
        Some(json!({"track_index":track, "channel_index":channel}))
    }
}

fn compare_timelines(left: &Timeline<'_>, right: &Timeline<'_>, diff: &mut Diff) {
    let a = left.seq;
    let b = right.seq;
    let scope = json!({"left_timeline_id":a.id, "right_timeline_id":b.id});
    diff.change("rate", &scope, exact(a.rate), exact(b.rate));
    known(
        diff,
        "clock",
        &scope,
        a.clock.as_ref().map(clock),
        b.clock.as_ref().map(clock),
    );
    known(
        diff,
        "canvas",
        &scope,
        a.frame_size.map(|v| json!(v)),
        b.frame_size.map(|v| json!(v)),
    );
    known(
        diff,
        "record_duration_frames",
        &scope,
        left.receipt(&a.id, "record_length").map(graph_value),
        right.receipt(&b.id, "record_length").map(graph_value),
    );
    diff.change("tracks", &scope, tracks(a), tracks(b));

    let pairs = pair_edits(left, right, diff);
    for &(a, b) in &pairs {
        compare_edits(left, a, right, b, diff);
    }
    let a_dissolve = dissolve::prepare(left.graph, a);
    let b_dissolve = dissolve::prepare(right.graph, b);
    report_losses(
        left,
        "left",
        a_dissolve.as_ref().ok().and_then(Option::as_ref),
        diff,
    );
    report_losses(
        right,
        "right",
        b_dissolve.as_ref().ok().and_then(Option::as_ref),
        diff,
    );
    compare_transition(left, a_dissolve, right, b_dissolve, &pairs, &scope, diff);
    known(
        diff,
        "timeline_markers",
        &scope,
        markers(left),
        markers(right),
    );
}

fn report_losses(
    t: &Timeline<'_>,
    side: &str,
    transition: Option<&dissolve::Dissolve>,
    diff: &mut Diff,
) {
    for loss in &t.losses {
        let signature = (loss.aspect.as_str(), loss.reason.as_str());
        if matches!(signature, audio::KEY_LOSS | audio::FADE_LOSS)
            || transition.is_some_and(|value| value.recover(loss, &t.seq.id))
        {
            continue;
        }
        diff.gap(
            &loss.aspect,
            &json!({"side":side, "owner_id":loss.object}),
            &loss.reason,
        );
    }
}

fn exact(value: Rational) -> Json {
    json!(value.to_string())
}

fn clock(clock: &SourceClock) -> Json {
    json!({"effective_count":clock.effective_count, "rate":exact(clock.rate),
        "display_base":clock.display_base, "drop":clock.drop})
}

fn known(diff: &mut Diff, field: &str, context: &Json, left: Option<Json>, right: Option<Json>) {
    match (left, right) {
        (Some(left), Some(right)) => diff.change(field, context, left, right),
        (left, right) => diff.gap(
            field,
            context,
            match (left.is_none(), right.is_none()) {
                (true, true) => "unresolved on both sides",
                (true, false) => "unresolved on the left",
                (false, true) => "unresolved on the right",
                (false, false) => unreachable!(),
            },
        ),
    }
}

fn tracks(seq: &SourceSequence) -> Json {
    let mut tracks: Vec<_> = seq
        .tracks
        .iter()
        .map(|t| (t.kind.as_str(), t.index))
        .collect();
    tracks.sort_unstable();
    json!(tracks)
}

fn lane(p: &Placement) -> Option<(&'static str, i64)> {
    match (p.video_track, p.audio_tracks.as_slice()) {
        (Some(v), []) => Some(("video", v)),
        (None, [a]) => Some(("audio", *a)),
        _ => None,
    }
}

type Slot = (&'static str, i64, i64, i64);

fn slots(seq: &SourceSequence) -> BTreeMap<Slot, Vec<&Placement>> {
    let mut slots = BTreeMap::<_, Vec<_>>::new();
    for p in &seq.placements {
        if let Some((kind, track)) = lane(p) {
            slots
                .entry((kind, track, p.record_start.num(), p.record_start.den()))
                .or_default()
                .push(p);
        }
    }
    slots
}

fn pair_edits<'a>(
    left: &'a Timeline<'a>,
    right: &'a Timeline<'a>,
    diff: &mut Diff,
) -> Vec<(&'a Placement, &'a Placement)> {
    let mut pairs = Vec::new();
    let mut paired_left = HashSet::new();
    let mut paired_right = HashSet::new();
    // SourceGraph validation guarantees uniqueness of placement IDs within each archive.
    let right_ids: HashMap<_, _> = right
        .seq
        .placements
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    for a in &left.seq.placements {
        if let (Some((kind, _)), Some(b)) = (lane(a), right_ids.get(a.id.as_str())) {
            if lane(b).is_some_and(|(other, _)| kind == other) {
                pairs.push((a, *b));
                paired_left.insert(a.id.as_str());
                paired_right.insert(b.id.as_str());
            }
        }
    }
    let left_slots = slots(left.seq);
    let right_slots = slots(right.seq);
    let keys: BTreeSet<_> = left_slots
        .keys()
        .chain(right_slots.keys())
        .copied()
        .collect();
    for key @ (kind, track, num, den) in keys {
        let a = left_slots.get(&key).map(Vec::as_slice).unwrap_or_default();
        let b = right_slots.get(&key).map(Vec::as_slice).unwrap_or_default();
        let open_a: Vec<_> = a
            .iter()
            .copied()
            .filter(|p| !paired_left.contains(p.id.as_str()))
            .collect();
        let open_b: Vec<_> = b
            .iter()
            .copied()
            .filter(|p| !paired_right.contains(p.id.as_str()))
            .collect();
        if open_a.is_empty() && open_b.is_empty() {
            continue;
        }
        let context = json!({"kind":kind,"track":track,"record_start":exact(Rational::new(num.into(),den.into()).unwrap())});
        if a.len() > 1 || b.len() > 1 {
            diff.gap("edit", &context, "multiple edits occupy the same slot");
            continue;
        }
        match (open_a.as_slice(), open_b.as_slice()) {
            ([a], [b]) => pairs.push((a, b)),
            ([a], []) if !right.may_omit_edits => {
                diff.change("edit", &context, edit_summary(a), Json::Null)
            }
            ([], [b]) if !left.may_omit_edits => {
                diff.change("edit", &context, Json::Null, edit_summary(b))
            }
            _ => diff.gap(
                "edit",
                &context,
                "the other side may contain an unresolved edit",
            ),
        }
    }
    if left
        .seq
        .placements
        .iter()
        .chain(&right.seq.placements)
        .any(|p| lane(p).is_none())
    {
        diff.gap("edit", &Json::Null, "unsupported edit lane shape");
    }
    pairs
}

fn edit_summary(p: &Placement) -> Json {
    json!({"id":p.id, "record_start":exact(p.record_start), "source_in":exact(p.source_in),
        "duration":exact(p.duration)})
}

fn compare_edits(
    left: &Timeline<'_>,
    a: &Placement,
    right: &Timeline<'_>,
    b: &Placement,
    diff: &mut Diff,
) {
    let context = json!({"left_edit_id":a.id, "right_edit_id":b.id});
    diff.change("edit.track", &context, json!(lane(a)), json!(lane(b)));
    diff.change(
        "edit.record_start",
        &context,
        exact(a.record_start),
        exact(b.record_start),
    );
    diff.change(
        "edit.duration",
        &context,
        exact(a.duration),
        exact(b.duration),
    );
    diff.change(
        "edit.source_in",
        &context,
        exact(a.source_in),
        exact(b.source_in),
    );
    match (left.binding(a), right.binding(b)) {
        (Some(old), Some(new)) => {
            for key in ["locators", "source_offset", "start", "physical_audio"] {
                if key == "physical_audio" && a.video_track.is_some() {
                    continue;
                }
                let resolved = |value: &Json| value.get(key).filter(|v| !v.is_null()).cloned();
                known(
                    diff,
                    &format!("media.{key}"),
                    &context,
                    resolved(&old),
                    resolved(&new),
                );
            }
            for key in [
                "duration",
                "clock",
                "rate",
                "frame_size",
                "sample_rate",
                "bits",
                "codec",
            ] {
                if old[key] != new[key] {
                    diff.change(
                        &format!("media.{key}"),
                        &context,
                        old[key].clone(),
                        new[key].clone(),
                    );
                }
            }
        }
        (a, b) => known(diff, "media.binding", &context, a, b),
    }
    let opaque = |t: &Timeline<'_>, p: &Placement| {
        t.unknown_effects.contains(p.id.as_str()) || t.unknown_effects.contains(t.seq.id.as_str())
    };
    if !opaque(left, a) && !opaque(right, b) {
        diff.change("edit.picture_controls", &context, picture(a), picture(b));
        diff.change(
            "edit.clip_volume",
            &context,
            volume(left, a),
            volume(right, b),
        );
    }
}

fn picture(p: &Placement) -> Json {
    json!({
        "transform":p.transform.as_ref().map(|t| json!({
            "position":t.position, "scale":t.scale, "rotation":t.rotation,
            "anchor":t.anchor, "crop":t.crop,
        })),
        "effects":p.effects.iter().map(|e| json!({"kind":e.kind, "parameters":graph_value(&e.params)})).collect::<Vec<_>>(),
    })
}

fn volume(t: &Timeline<'_>, p: &Placement) -> Json {
    let fields = [
        "volume_key_clock",
        "volume_interpolation",
        "volume_extrapolation",
        "fade_in_frames",
        "fade_out_frames",
        "fade_clock",
        "fade_shape",
    ];
    json!({"gain_db":p.gain_db, "level_db":p.volume_db,
        "keys":p.volume_keys.iter().map(|k| json!({"at":exact(k.at),"db":k.db})).collect::<Vec<_>>(),
        "saved":fields.into_iter().filter_map(|key| t.receipt(&p.id,key).map(|value| (key,graph_value(value))))
            .collect::<BTreeMap<_,_>>(),
    })
}

fn markers(t: &Timeline<'_>) -> Option<Json> {
    let mut rows = Vec::new();
    for m in &t.seq.markers {
        if m.owner != MarkerOwner::Sequence
            || m.binding.reason.is_some()
            || !m.binding.placements.is_empty()
        {
            return None;
        }
        rows.push(
            json!({"name":m.name, "note":m.comment, "user":m.user, "start":exact(m.start?),
            "duration":exact(m.duration), "color":m.color.as_ref().map(|v| (&v.palette,&v.name)),
            "owner":m.owner.as_str()}),
        );
    }
    rows.sort_by_cached_key(Json::to_string);
    Some(
        json!({"markers":rows, "native":t.receipt(&t.seq.id, "markers")
        .map(graph_value).unwrap_or_else(|| json!([]))}),
    )
}

fn compare_transition(
    left: &Timeline<'_>,
    a: Result<Option<dissolve::Dissolve>>,
    right: &Timeline<'_>,
    b: Result<Option<dissolve::Dissolve>>,
    pairs: &[(&Placement, &Placement)],
    scope: &Json,
    diff: &mut Diff,
) {
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        _ => {
            diff.gap(
                "transition",
                scope,
                "a saved transition is outside the qualified Cross Dissolve profile",
            );
            return;
        }
    };
    let summary = |value: Option<dissolve::Dissolve>| {
        value.map_or(Json::Null, |d| {
            json!({"kind":dissolve::TYPE, "track":1, "cut_frames":d.cut,
            "duration_frames":dissolve::DURATION, "alignment":dissolve::ALIGNMENT})
        })
    };
    let both = a.is_some() && b.is_some();
    diff.change("transition", scope, summary(a), summary(b));
    if !both {
        return;
    }
    for key in ["outgoing", "incoming"] {
        if let (Some(a), Some(b)) = (endpoint(left, key), endpoint(right, key)) {
            if pairs.iter().any(|(l, r)| l.id == a && r.id == b) {
                continue;
            }
            if let Some((_, expected)) = pairs.iter().find(|(l, _)| l.id == a) {
                diff.change(
                    &format!("transition.{key}"),
                    scope,
                    json!(expected.id),
                    json!(b),
                );
                continue;
            }
        }
        diff.gap(
            &format!("transition.{key}"),
            scope,
            "transition endpoint has no corresponding edit",
        );
    }
}

fn endpoint<'a>(t: &Timeline<'a>, key: &str) -> Option<&'a str> {
    let Value::Arr(rows) = t.graph.sidecar(dissolve::ADAPTER, &t.seq.id)? else {
        return None;
    };
    match rows.first()?.get(key)? {
        Value::Str(id) => Some(id),
        _ => None,
    }
}

fn graph_value(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(v) => json!(v),
        Value::Int(v) => json!(v),
        Value::Float(v) => json!(v),
        Value::Str(v) => json!(v),
        Value::Arr(v) => Json::Array(v.iter().map(graph_value).collect()),
        Value::Obj(v) => Json::Object(v.iter().map(|(k, v)| (k.clone(), graph_value(v))).collect()),
    }
}
