//! Two-ended Premiere transitions on the qualified V1 and A1 lanes.
use super::ticks;
use crate::Result;
use graph::{
    source::{SourceBinding, SourceSequence, SourceTransition},
    Placement, Rational, StreamKind,
};
use prproj::{
    model::{Project, TrackData},
    xml::NodeId,
};
use std::collections::{HashMap, HashSet};

pub(super) fn capture(
    project: &Project,
    track: &TrackData,
    kind: StreamKind,
    sequence: &mut SourceSequence,
    frame_ticks: i64,
) -> Result<()> {
    if track.transitions.len() > crate::prepare::MAX_EDITS_PER_LANE {
        return Err("Premiere transition count exceeds the linked lane limit".into());
    }
    let tree = &project.doc.tree;
    let on_lane = |edit: &Placement| match kind {
        StreamKind::Video => edit.video_track == Some(1),
        StreamKind::Audio => edit.audio_tracks.as_slice() == [1],
    };
    let mut starts = HashMap::new();
    let mut ends = HashMap::new();
    for (index, edit) in sequence
        .placements
        .iter()
        .enumerate()
        .filter(|(_, edit)| on_lane(edit))
    {
        insert_endpoint(&mut starts, edit.record_start, index);
        let end = edit
            .record_start
            .checked_add(edit.duration)
            .ok_or("edit range overflow")?;
        insert_endpoint(&mut ends, end, index);
    }

    // Read every clip claim once. A middle edit can own both a head and a tail.
    let mut claims = HashMap::new();
    for &clip in &track.clips {
        let element = project.track_item(clip).element;
        let object = tree
            .get(element, "ObjectID")
            .ok_or("Premiere edit lacks ObjectID")?;
        for (field, head) in [
            ("ClipTrackItem/HeadTransition", true),
            ("ClipTrackItem/TailTransition", false),
        ] {
            let links = tree.findall(element, field)?;
            match links.as_slice() {
                [] => {}
                [link] => {
                    let transition = project.doc.resolve(*link)?;
                    if claims
                        .insert((transition, head), format!("premiere:edit:{object}"))
                        .is_some()
                    {
                        return Err(format!(
                            "Premiere edit {object}: ambiguous transition endpoint claim"
                        )
                        .into());
                    }
                }
                _ => {
                    return Err(format!(
                        "Premiere edit {object}: duplicate saved transition references"
                    )
                    .into())
                }
            }
        }
    }

    let mut seen = HashSet::new();
    let mut pending = Vec::new();
    for &node in &track.transitions {
        let id = tree
            .get(node, "ObjectID")
            .ok_or("Premiere transition lacks ObjectID")?;
        let refuse = |reason| format!("Premiere transition {id}: {reason}");
        if !seen.insert(node) {
            return Err(refuse("duplicate saved transition reference on its lane").into());
        }
        if track.index != 0 {
            return Err(refuse("expected a transition on V1 or A1").into());
        }
        let saved = read(project, node, kind, sequence.rate, frame_ticks, id)?;
        let outgoing = endpoint(&ends, saved.cut, "outgoing", id)?;
        let incoming = endpoint(&starts, saved.cut, "incoming", id)?;
        let out = &sequence.placements[outgoing];
        let input = &sequence.placements[incoming];
        if out.record_start > saved.begin {
            return Err(refuse("missing outgoing edit spanning the blend").into());
        }
        if claims.remove(&(node, false)).as_deref() != Some(out.id.as_str())
            || claims.remove(&(node, true)).as_deref() != Some(input.id.as_str())
        {
            return Err(
                refuse("saved clip transition references disagree with the cut endpoints").into(),
            );
        }
        pending.push((saved, outgoing, incoming));
    }
    if !claims.is_empty() {
        return Err(
            "Premiere clip transition reference is absent from its saved track list".into(),
        );
    }

    pending.sort_by_key(|(saved, _, _)| saved.begin);
    for pair in pending.windows(2) {
        let [first, second] = [&pair[0].0, &pair[1].0];
        if first
            .begin
            .checked_add(first.duration)
            .ok_or("transition range overflow")?
            > second.begin
        {
            return Err(format!(
                "Premiere transitions {} and {} overlap on the same lane",
                first.id, second.id
            )
            .into());
        }
    }
    for (saved, outgoing, incoming) in pending {
        let transition = SourceTransition {
            id: format!("premiere:transition:{}", saved.id),
            kind,
            track: 1,
            cut: saved.cut,
            duration: saved.duration,
            cut_offset: saved.before,
            style: saved.style.into(),
            outgoing: SourceBinding {
                placements: vec![sequence.placements[outgoing].id.clone()],
                reason: None,
            },
            incoming: SourceBinding {
                placements: vec![sequence.placements[incoming].id.clone()],
                reason: None,
            },
        };
        let out = &mut sequence.placements[outgoing];
        out.duration = out
            .duration
            .checked_add(saved.after)
            .ok_or("outgoing extent overflow")?;
        let input = &mut sequence.placements[incoming];
        input.source_in = input
            .source_in
            .checked_sub(saved.before)
            .filter(|time| !time.is_negative())
            .ok_or_else(|| format!("{}: insufficient incoming preroll", transition.id))?;
        input.record_start = saved.begin;
        input.duration = input
            .duration
            .checked_add(saved.before)
            .ok_or("edit range overflow")?;
        sequence.transitions.push(transition);
    }
    Ok(())
}

fn insert_endpoint(map: &mut HashMap<Rational, Option<usize>>, time: Rational, index: usize) {
    map.entry(time)
        .and_modify(|value| *value = None)
        .or_insert(Some(index));
}

fn endpoint(
    map: &HashMap<Rational, Option<usize>>,
    cut: Rational,
    side: &str,
    id: &str,
) -> Result<usize> {
    match map.get(&cut) {
        Some(Some(index)) => Ok(*index),
        Some(None) => Err(format!("Premiere transition {id}: ambiguous {side} edit").into()),
        None => Err(format!("Premiere transition {id}: missing {side} edit").into()),
    }
}

struct SavedTransition<'a> {
    id: &'a str,
    begin: Rational,
    cut: Rational,
    duration: Rational,
    before: Rational,
    after: Rational,
    style: &'static str,
}

fn read<'a>(
    project: &Project,
    node: NodeId,
    kind: StreamKind,
    rate: Rational,
    frame_ticks: i64,
    id: &'a str,
) -> Result<SavedTransition<'a>> {
    use crate::dissolve::{Style, ALIGNMENT, PREMIERE_AUDIO_STYLE, PREMIERE_POWER_STYLE};
    use prproj::model::transition::*;
    let tree = &project.doc.tree;
    let refuse = |reason| format!("Premiere transition {id}: {reason}");
    let start = transition_start(tree, node)?.ticks;
    let end = transition_end(tree, node)?.ticks;
    let offset = transition_cut_point_offset(tree, node)?.ticks;
    let duration = end
        .checked_sub(start)
        .ok_or_else(|| refuse("duration overflow"))?;
    let match_name = transition_match_name(tree, node)?;
    let (profile, style) = match kind {
        StreamKind::Video
            if tree.tag(node) == "VideoTransitionTrackItem"
                && crate::dissolve::supported_rate(rate)
                && match_name == "ADBE Cross Dissolve" =>
        {
            (Style::Video, "premiere.ADBE Cross Dissolve")
        }
        StreamKind::Audio
            if tree.tag(node) == "AudioTransitionTrackItem"
                && rate == Rational::integer(24)
                && match_name == "Constant Gain"
                && constant_gain_fields(tree, node)? =>
        {
            (Style::Gain, PREMIERE_AUDIO_STYLE)
        }
        StreamKind::Audio
            if tree.tag(node) == "AudioTransitionTrackItem"
                && rate == Rational::integer(24)
                && match_name == "Constant Power"
                && constant_power_fields(tree, node)? =>
        {
            (Style::Power, PREMIERE_POWER_STYLE)
        }
        StreamKind::Video => {
            return Err(refuse(
                "expected the qualified left-aligned or centered Cross Dissolve profile",
            )
            .into());
        }
        StreamKind::Audio => {
            return Err(refuse(
                "expected a qualified 24 fps A1 Constant Gain or Constant Power profile",
            )
            .into());
        }
    };
    let alignment = if offset == duration { ALIGNMENT } else { "2" };
    let before_frames = profile.offset(duration / frame_ticks, alignment);
    if start < 0
        || start % frame_ticks != 0
        || duration % frame_ticks != 0
        || before_frames.and_then(|n| n.checked_mul(frame_ticks)) != Some(offset)
        || transition_is_reversed(tree, node)?
        || transition_border_width(tree, node)? != 0
        || !transition_has_incoming_clip(tree, node)?
        || !transition_has_outgoing_clip(tree, node)?
    {
        return Err(refuse(
            "expected a whole-frame transition with qualified alignment and two clip endpoints",
        )
        .into());
    }
    let cut = ticks(
        start
            .checked_add(offset)
            .ok_or_else(|| refuse("cut overflow"))?,
    )?;
    let duration = ticks(duration)?;
    let before = ticks(offset)?;
    let after = duration
        .checked_sub(before)
        .ok_or_else(|| refuse("invalid alignment"))?;
    let begin = ticks(start)?;
    Ok(SavedTransition {
        id,
        begin,
        cut,
        duration,
        before,
        after,
        style,
    })
}

fn constant_gain_fields(tree: &prproj::xml::Tree, transition: NodeId) -> Result<bool> {
    // The saved transition has a stereo render layout even on a mono lane.
    for (field, expected) in [
        ("FadeShapeType", "0"),
        ("FadeShapeValue", "0"),
        (
            "AudioChannelLayout",
            r#"[{"channellabel":100},{"channellabel":101}]"#,
        ),
    ] {
        let refs = tree.findall(transition, field)?;
        let [value] = refs.as_slice() else {
            return Ok(false);
        };
        if !tree.is_childless(*value) || tree.text(*value) != Some(expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn constant_power_fields(tree: &prproj::xml::Tree, transition: NodeId) -> Result<bool> {
    use super::components::field;
    Ok(
        field(tree, transition, "AudioChannelLayout")? == Some(r#"[{"channellabel":0}]"#)
            && tree.findall(transition, "FadeShapeType")?.is_empty()
            && tree.findall(transition, "FadeShapeValue")?.is_empty(),
    )
}
