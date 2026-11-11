//! Preparation for a single, two-angle editable Group with fixed camera-A audio.
use crate::{
    prepare::{
        check_binding, check_losses, check_plain, check_tracks, checked_name, frames,
        qualified_origin, rate, simple_range, Edit, MAX_EDITS_PER_LANE,
    },
    Result, VerifiedBinding, DAY,
};
use graph::{
    source::{MulticamAlternative, SourceGraph},
    StreamKind,
};
use std::{collections::HashSet, sync::Arc};

pub struct PreparedGroup {
    pub(crate) project_name: String,
    pub(crate) sequence_name: String,
    pub(crate) group_name: String,
    pub(crate) origin: i64,
    pub(crate) end: i64,
    pub(crate) length: i64,
    pub(crate) cameras: [Arc<VerifiedBinding>; 2],
    pub(crate) offsets: [i64; 2],
    pub(crate) lanes: [Vec<GroupEdit>; 3],
    notices: Vec<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct GroupEdit {
    pub(crate) edit: Edit,
    pub(crate) camera: usize,
}

impl PreparedGroup {
    pub fn notices(&self) -> &[String] {
        &self.notices
    }
}

fn member(alt: &MulticamAlternative, length: i64) -> Result<(&str, i64, i64, i64)> {
    if alt.reason.is_some() || alt.pieces.len() > 3 {
        return Err("unresolved or multipart Group angle".into());
    }
    let mut cursor = 0;
    let mut leaf = None;
    for piece in &alt.pieces {
        let start = frames(piece.start)?;
        let duration = frames(piece.duration)?;
        if start != cursor
            || duration == 0
            || start.checked_add(duration).is_none_or(|e| e > length)
        {
            return Err("Group angle pieces must cover its extent without overlap".into());
        }
        cursor += duration;
        match (&piece.item, piece.stream, piece.source_in) {
            (Some(item), Some(stream), Some(source)) if frames(source)? == 0 && leaf.is_none() => {
                leaf = Some((item.as_str(), stream, start, duration));
            }
            (None, None, None) => {}
            _ => {
                return Err("Group angle must contain a single full leaf with known zero In".into())
            }
        }
    }
    if cursor != length {
        return Err("Group angle extent is incomplete".into());
    }
    leaf.ok_or_else(|| "Group angle has no media".into())
}

pub fn prepare_group(
    graph: &SourceGraph,
    sequence_id: &str,
    canvas: (u16, u16),
    explicit_origin: Option<i64>,
    cameras: [VerifiedBinding; 2],
) -> Result<PreparedGroup> {
    if cameras.iter().any(|camera| camera.atom.is_some()) {
        return Err("direct OP-Atom is outside the qualified Resolve Group profile".into());
    }
    prepare_bound_group(
        graph,
        sequence_id,
        canvas,
        explicit_origin,
        cameras.map(Arc::new),
    )
}

pub(crate) fn prepare_bound_group(
    graph: &SourceGraph,
    sequence_id: &str,
    canvas: (u16, u16),
    explicit_origin: Option<i64>,
    cameras: [Arc<VerifiedBinding>; 2],
) -> Result<PreparedGroup> {
    graph.validate()?;
    let sequence = graph
        .sequences
        .iter()
        .find(|s| s.id == sequence_id)
        .ok_or("selected sequence identity absent")?;
    if canvas != (1920, 1080)
        || sequence.rate != rate()
        || sequence.frame_size.is_some_and(|s| s != (1920, 1080))
        || !sequence.transitions.is_empty()
        || !sequence.texts.is_empty()
    {
        return Err("unsupported Group canvas, rate, transition or text".into());
    }
    check_tracks(sequence)?;
    let (origin, mut notices) = if sequence.clock.is_none() && sequence.start.is_none() {
        let origin = explicit_origin
            .filter(|n| (0..DAY).contains(n))
            .ok_or("Group sequence start unresolved; supply an explicit destination origin")?;
        (
            origin,
            vec![format!(
                "unresolved Avid sequence start; explicit Resolve origin frame {origin}"
            )],
        )
    } else {
        let origin = qualified_origin(sequence)?;
        if explicit_origin.is_some_and(|value| value != origin) {
            return Err("explicit origin conflicts with qualified source clock".into());
        }
        (origin, Vec::new())
    };
    let uses: Vec<_> = graph
        .multicam_uses
        .iter()
        .filter(|u| u.sequence == sequence_id)
        .take(3 * MAX_EDITS_PER_LANE + 1)
        .collect();
    let first = uses.first().ok_or("selected sequence has no Group uses")?;
    let [group_id] = first.candidates.as_slice() else {
        return Err("Group identity is unresolved".into());
    };
    let group = graph
        .multicams
        .iter()
        .find(|g| &g.id == group_id)
        .ok_or("Group absent")?;
    let length = frames(group.duration.ok_or("Group duration is unresolved")?)?;
    if group.rate != Some(rate())
        || group.reason.is_some()
        || group.lanes.len() != 3
        || length == 0
        || length >= DAY - 86400
    {
        return Err("unsupported Group clock or lane topology".into());
    }
    let expected = [
        (StreamKind::Video, 1),
        (StreamKind::Audio, 1),
        (StreamKind::Audio, 2),
    ];
    let mut offsets = [0; 2];
    for (lane_index, (kind, index)) in expected.into_iter().enumerate() {
        let lane = group
            .lanes
            .iter()
            .find(|l| l.kind == kind && l.index == index)
            .ok_or("Group lane absent")?;
        if lane.reason.is_some() || lane.alternatives.len() != 2 || lane.selected != Some(0) {
            return Err("unsupported Group alternatives or default selection".into());
        }
        for (camera_index, (binding, alt)) in cameras.iter().zip(&lane.alternatives).enumerate() {
            let (item, stream, start, duration) = member(alt, length)?;
            if item != binding.item_id || stream != index || duration != binding.frames {
                return Err(
                    "Group leaf identity, channel or span disagrees with verified camera".into(),
                );
            }
            if lane_index == 0 {
                check_binding(graph, binding)?;
                offsets[camera_index] = start;
            } else if offsets[camera_index] != start {
                return Err("Group picture and mono camera extents disagree".into());
            }
        }
    }
    if cameras[0].item_id == cameras[1].item_id
        || cameras[0].path == cameras[1].path
        || offsets.iter().copied().min() != Some(0)
        || offsets
            .iter()
            .zip(&cameras)
            .map(|(start, b)| start + b.frames)
            .max()
            != Some(length)
    {
        return Err("Group camera identities or outer extent disagree".into());
    }
    check_losses(
        graph,
        sequence,
        &[&cameras[0], &cameras[1]],
        Some(group_id),
        |_| false,
    )?;

    if uses.len() != sequence.placements.len() {
        return Err("Group uses must cover every selected placement exactly once".into());
    }
    let mut used = HashSet::new();
    let mut lanes: [Vec<GroupEdit>; 3] = Default::default();
    for use_ in uses {
        let lane = match (use_.kind, use_.track, use_.lane) {
            (StreamKind::Video, 1, Some(1)) => 0,
            (StreamKind::Audio, 1, Some(1)) => 1,
            (StreamKind::Audio, 2, Some(2)) => 2,
            _ => return Err("unsupported Group edit lane".into()),
        };
        let camera = match (lane, use_.selected) {
            (_, Some(0)) => 0,
            (0, Some(1)) => 1,
            _ => return Err("Resolve Group audio must stay on camera A".into()),
        };
        if use_.reason.is_some()
            || use_.binding.reason.is_some()
            || !matches!(use_.candidates.as_slice(), [id] if id == group_id)
            || lanes[lane].len() == MAX_EDITS_PER_LANE
        {
            return Err("ambiguous Group use or edit count limit exceeded".into());
        }
        let [id] = use_.binding.placements.as_slice() else {
            return Err("Group use does not bind one leaf edit".into());
        };
        let p = sequence
            .placements
            .iter()
            .find(|p| &p.id == id)
            .ok_or("Group leaf edit absent")?;
        check_plain(p)?;
        let route_matches = match lane {
            0 => {
                p.video_track == Some(1) && p.audio_tracks.is_empty() && p.audio_streams.is_empty()
            }
            n => {
                p.video_track.is_none()
                    && p.audio_tracks == [n as i64]
                    && p.audio_streams == [n as i64]
            }
        };
        let source_in = frames(use_.source_in.ok_or("unresolved Group source In")?)?;
        let start = frames(p.record_start)?;
        let span = frames(p.duration)?;
        let leaf_in = source_in
            .checked_sub(offsets[camera])
            .filter(|&n| n >= 0)
            .ok_or("saved Group edit selects an angle outside its available media")?;
        if !used.insert(id)
            || !route_matches
            || p.item != cameras[camera].item_id
            || use_.record_start != Some(p.record_start)
            || use_.duration != Some(p.duration)
            || leaf_in != frames(p.source_in)?
            || leaf_in
                .checked_add(span)
                .is_none_or(|e| e > cameras[camera].frames)
        {
            return Err("saved Group use disagrees with its leaf edit or media range".into());
        }
        lanes[lane].push(GroupEdit {
            edit: Edit {
                start,
                length: span,
                source_in,
                envelope: None,
                binding: 0,
            },
            camera,
        });
    }
    let mut durations = [0; 3];
    for (i, lane) in lanes.iter_mut().enumerate() {
        lane.sort_by_key(|p| p.edit.start);
        durations[i] = simple_range(&mut lane.iter().map(|p| p.edit).collect::<Vec<_>>())?;
    }
    let [duration, a1, a2] = durations;
    if duration == 0 || duration != a1 || duration != a2 {
        return Err("Group lanes must cover the same nonempty record extent".into());
    }
    let end = origin
        .checked_add(duration)
        .filter(|e| *e < DAY)
        .ok_or("Group record end exceeds day")?;
    notices.extend(
        sequence
            .markers
            .iter()
            .map(|m| format!("omitted marker occurrence {}", m.owner_occurrence)),
    );
    for camera in &cameras {
        if let Some(item) = graph.items.iter().find(|i| i.id == camera.item_id) {
            notices.extend(
                item.markers
                    .iter()
                    .map(|m| format!("omitted marker occurrence {}", m.owner_occurrence)),
            );
        }
    }
    if graph.sidecar("avid.group", group_id).is_some() {
        notices.push(format!(
            "Group {group_id}: native marks and selector metadata retained in source graph"
        ));
    }
    Ok(PreparedGroup {
        project_name: checked_name(&graph.project.name)?,
        sequence_name: checked_name(&sequence.name)?,
        group_name: checked_name(&group.name)?,
        origin,
        end,
        length,
        cameras,
        offsets,
        lanes,
        notices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::{source::MulticamPiece, Rational};

    fn time(frame: i128) -> Rational {
        Rational::new(frame * 1001, 24000).unwrap()
    }

    #[test]
    fn member_preserves_an_explicit_gap_and_rejects_multipart_angles() {
        let mut alt = MulticamAlternative {
            id: "camera".into(),
            reason: None,
            pieces: vec![
                MulticamPiece {
                    start: time(0),
                    duration: time(24),
                    item: None,
                    stream: None,
                    source_in: None,
                },
                MulticamPiece {
                    start: time(24),
                    duration: time(48),
                    item: Some("camera-A".into()),
                    stream: Some(1),
                    source_in: Some(time(0)),
                },
            ],
        };
        assert_eq!(member(&alt, 72).unwrap(), ("camera-A", 1, 24, 48));
        let mut later = alt.pieces[1].clone();
        later.start = time(48);
        later.duration = time(24);
        alt.pieces[1].duration = time(24);
        alt.pieces.push(later);
        assert!(member(&alt, 72).is_err());
    }
}
