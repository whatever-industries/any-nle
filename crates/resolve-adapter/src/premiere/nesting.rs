//! Saved Premiere 27 picture and stereo sequence edits.
use super::{components::field, MAX_EDITS};
use crate::{nests, Result};
use graph::{Sidecar, Value};
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{MediaType, Project, SequenceId, TrackItemId},
    xml::NodeId,
};
use std::collections::{HashMap, HashSet};

const FRAME: i64 = TICKS_PER_SECOND / 24;

pub(super) struct Plan {
    pub(super) sequences: Vec<SequenceId>,
    lengths: HashMap<SequenceId, i64>,
    uses: HashMap<SequenceId, Vec<Value>>,
    empty_audio_parents: HashSet<SequenceId>,
    stereo_parents: HashSet<SequenceId>,
    stereo_children: HashSet<SequenceId>,
}

impl Plan {
    pub(super) fn source_facts(&mut self, sequence: SequenceId, owner: &str) -> Option<Sidecar> {
        let rows = self.uses.remove(&sequence)?;
        Some(Sidecar {
            adapter: nests::PREMIERE_ADAPTER.into(),
            object: owner.into(),
            data: Value::Arr(rows),
        })
    }

    pub(super) fn length(&self, sequence: SequenceId) -> Option<i64> {
        self.lengths.get(&sequence).copied()
    }

    pub(super) fn omits_empty_audio(&self, sequence: SequenceId) -> bool {
        self.empty_audio_parents.contains(&sequence)
    }

    pub(super) fn stereo_parent(&self, sequence: SequenceId) -> bool {
        self.stereo_parents.contains(&sequence)
    }

    pub(super) fn stereo_child(&self, sequence: SequenceId) -> bool {
        self.stereo_children.contains(&sequence)
    }
}

fn video_items(project: &Project, sequence: SequenceId) -> Result<&[TrackItemId]> {
    let data = project.sequence(sequence);
    let mut video = data
        .video_tracks
        .iter()
        .copied()
        .filter(|&id| !project.track(id).clips.is_empty());
    let first_id = video.next().ok_or("Premiere nesting lacks active V1")?;
    let first = project.track(first_id);
    if data.timebase != Some(FRAME)
        || data.frame_size != Some((1920, 1080))
        || project.video_display_format(sequence)? != Some(100)
        || first.index != 0
        || video.next().is_some()
        || project.is_muted(first_id)?
        || data
            .video_tracks
            .iter()
            .chain(&data.audio_tracks)
            .any(|id| !project.track(*id).transitions.is_empty())
        || project
            .caption_tracks(sequence)
            .iter()
            .any(|id| !project.captions(*id).is_empty())
        || !project.sequence_markers(sequence).is_empty()
        || first.clips.len() > MAX_EDITS
    {
        return Err("Premiere nesting requires plain 1080p24 timelines with one active V1".into());
    }
    Ok(&first.clips)
}

fn frames(ticks: i64) -> Result<i64> {
    if ticks < 0 || ticks % FRAME != 0 {
        return Err("Premiere nest timing is off the 24 fps frame grid".into());
    }
    Ok(ticks / FRAME)
}

fn extent(project: &Project, sequence: SequenceId) -> Result<i64> {
    let mut end = 0;
    for &clip in video_items(project, sequence)? {
        end = end.max(frames(project.end(clip)?.ticks)?);
    }
    Ok(end)
}

fn reference(project: &Project, node: NodeId, path: &str) -> Result<NodeId> {
    let [value] = project
        .doc
        .tree
        .findall(node, path)?
        .try_into()
        .map_err(|_| format!("missing or ambiguous Premiere nest reference {path}"))?;
    Ok(project.doc.resolve(value)?)
}

fn check_neutral_audio_track(project: &Project, track: NodeId) -> Result<()> {
    let fader = super::mixer::read_track(project, track, true)?;
    if fader.tenths_db != 0 || fader.muted || fader.pan != 0 {
        return Err("Premiere nest audio requires neutral mixer tracks".into());
    }
    Ok(())
}

enum AudioCompanions {
    None,
    Empty,
    Stereo,
}

fn has_stereo_child_audio(project: &Project, child: SequenceId, parent_edit: &str) -> Result<bool> {
    let mut active = project.sequence(child).audio_tracks.iter().map(|&id| project.track(id))
        .filter(|track| !track.clips.is_empty());
    let Some(track) = active.next() else { return Ok(false) };
    let invalid = || format!("{parent_edit}: Premiere nest child contains audio edits outside the neutral Standard stereo WAV profile");
    if active.next().is_some() || track.index != 0
        || !super::mixer::is_standard_stereo(project, track.element)?
    {
        return Err(invalid().into());
    }
    let [clip] = track.clips.as_slice() else { return Err(invalid().into()) };
    let source = project.track_item_source(*clip).ok_or_else(invalid)?;
    let path = project.item_media_path(source).ok_or_else(invalid)?;
    if !std::path::Path::new(path).extension().and_then(|part| part.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
    {
        return Err(invalid().into());
    }
    Ok(true)
}

fn check_audio(
    project: &Project,
    parent: SequenceId,
    by_uid: &HashMap<String, SequenceId>,
    rows: &mut [Value],
    stereo_children: &HashSet<SequenceId>,
) -> Result<AudioCompanions> {
    let audio_tracks = &project.sequence(parent).audio_tracks;
    let stereo_child = rows.iter().any(|row| {
        nests::string(row, "child_sequence").and_then(|id| by_uid.get(id))
            .is_some_and(|id| stereo_children.contains(id))
    });
    if audio_tracks
        .iter()
        .all(|&id| project.track(id).clips.is_empty())
    {
        if stereo_child {
            return Err("Premiere audible nest requires a matching stereo audio companion".into());
        }
        return Ok(AudioCompanions::None);
    }
    let tree = &project.doc.tree;
    let mut by_range = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        if let (Some(child), Some(start), Some(duration), Some(source_in)) = (
            nests::string(row, "child_sequence"),
            nests::int(row, "record_start_frames"),
            nests::int(row, "record_duration_frames"),
            nests::int(row, "saved_source_in_frames"),
        ) {
            if by_range
                .insert((child.to_owned(), start, duration, source_in), index)
                .is_some()
            {
                return Err("ambiguous Premiere picture nests at the same range".into());
            }
        }
    }
    let mut found = false;
    let mut stereo = false;
    let mut checked_children = HashSet::new();
    for &track_id in audio_tracks {
        let track = project.track(track_id);
        check_neutral_audio_track(project, track.element)?;
        if track.clips.is_empty() {
            continue;
        }
        if track.index != 0 || project.is_muted(track_id)? || found {
            return Err(
                "Premiere picture nests only admit empty audio companions on neutral A1".into(),
            );
        }
        if stereo_child && !super::mixer::is_standard_stereo(project, track.element)? {
            return Err("Premiere audible nests require Standard stereo A1".into());
        }
        found = true;
        for &clip in &track.clips {
            let native = project.track_item(clip);
            let object = tree
                .get(native.element, "ObjectID")
                .ok_or("Premiere nest audio lacks ObjectID")?;
            let id = format!("premiere:edit:{object}");
            let invalid = || {
                format!("{id}: Premiere nest audio must be a stereo companion with the same picture trim")
            };
            let item = project.track_item_source(clip).ok_or_else(invalid)?;
            let child_uid = project
                .item(item)
                .sequence_uid
                .as_ref()
                .ok_or_else(invalid)?;
            let child = *by_uid.get(child_uid).ok_or_else(invalid)?;
            let audible = stereo_children.contains(&child);
            if checked_children.insert(child) {
                for &id in &project.sequence(child).audio_tracks {
                    check_neutral_audio_track(project, project.track(id).element)?;
                }
            }
            let start = frames(project.start(clip)?.ticks)?;
            let duration = frames(project.end(clip)?.ticks)? - start;
            let source_in = frames(project.in_point(clip)?.ticks)?;
            let source_out = frames(project.out_point(clip)?.ticks)?;
            let index = by_range
                .remove(&(child_uid.clone(), start, duration, source_in))
                .ok_or_else(invalid)?;
            let source = reference(project, native.clip, "Clip/Source")?;
            if tree.tag(source) != "AudioSequenceSource"
                || reference(project, source, "SequenceSource/Sequence")?
                    != project.sequence(child).element
                || project.sequence_item(child) != Some(item)
                || project.is_disabled(clip)?
                || project.speed(clip)? != 1.0
                || project.is_speed_reversed(clip)?
                || !native.components.is_empty()
                || !native.selection_components.is_empty()
                || source_out - source_in != duration
            {
                return Err(invalid().into());
            }
            let saved_length = field(tree, source, "OriginalDuration")?
                .ok_or_else(invalid)?
                .parse()?;
            if nests::int(&rows[index], "child_length_frames") != Some(frames(saved_length)?)
                || field(tree, native.clip, "AudioChannelLayout")?
                    != Some(r#"[{"channellabel":100},{"channellabel":101}]"#)
            {
                return Err(invalid().into());
            }
            let [left, right] = tree
                .findall(native.clip, "SecondaryContents/SecondaryContentItem")?
                .try_into()
                .map_err(|_| invalid())?;
            for (selector, channel) in [(left, "0"), (right, "1")] {
                let content = project.doc.resolve(selector)?;
                if tree.get(selector, "Index") != Some(channel)
                    || tree.tag(content) != "SecondaryContent"
                    || field(tree, content, "ChannelIndex")? != Some(channel)
                    || reference(project, content, "Content")? != source
                {
                    if audible {
                        return Err(format!("{id}: Premiere nest stereo channel order or source owner is contradictory").into());
                    }
                    return Err(invalid().into());
                }
            }
            let Value::Obj(row) = &mut rows[index] else {
                return Err(invalid().into());
            };
            let field = if audible {
                stereo = true;
                "stereo_audio_companion"
            } else {
                "empty_audio_companion"
            };
            row.push((field.into(), Value::Str(id)));
        }
    }
    if stereo_child && rows.iter().any(|row| row.get("stereo_audio_companion").is_none()) {
        return Err("Premiere audible nesting requires a matching stereo audio companion for every nest".into());
    }
    Ok(if stereo { AudioCompanions::Stereo } else { AudioCompanions::Empty })
}

pub(super) fn discover(project: &Project, selected: Vec<SequenceId>) -> Result<Plan> {
    let mut plan = Plan {
        sequences: selected,
        lengths: HashMap::new(),
        uses: HashMap::new(),
        empty_audio_parents: HashSet::new(),
        stereo_parents: HashSet::new(),
        stereo_children: HashSet::new(),
    };
    let by_uid: HashMap<_, _> = project
        .sequences()
        .into_iter()
        .filter_map(|id| project.sequence_id(id).map(|uid| (uid, id)))
        .collect();
    let mut seen: HashSet<_> = plan.sequences.iter().copied().collect();
    for parent in plan.sequences.clone() {
        if super::multicam::source(project, parent)?.is_some() {
            continue;
        }
        let mut rows = Vec::new();
        let mut has_nested_audio = false;
        for clip in project.sequence_clips(parent) {
            let Some(item) = project.track_item_source(clip) else {
                continue;
            };
            let Some(uid) = project.item(item).sequence_uid.as_ref() else {
                continue;
            };
            if project.track_item_media_type(clip) == MediaType::Audio {
                has_nested_audio = true;
                continue;
            }
            if rows.is_empty() {
                plan.lengths.insert(parent, extent(project, parent)?);
            }
            if rows.len() == 4096 {
                return Err("Premiere nest use count exceeds the Resolve limit".into());
            }
            let native = project.track_item(clip);
            let tree = &project.doc.tree;
            let object = tree
                .get(native.element, "ObjectID")
                .ok_or("Premiere nest lacks ObjectID")?;
            let id = format!("premiere:edit:{object}");
            let child = *by_uid
                .get(uid)
                .ok_or_else(|| format!("{id}: Premiere nest child is absent"))?;
            if child == parent || project.sequence_item(child) != Some(item) {
                return Err(format!(
                    "{id}: Premiere nest child identity is cyclic or contradictory"
                )
                .into());
            }
            if project.is_disabled(clip)?
                || project.speed(clip)? != 1.0
                || project.is_speed_reversed(clip)?
                || !native.components.is_empty()
                || !native.selection_components.is_empty()
            {
                return Err(format!(
                    "{id}: Premiere nest must be enabled and free of effects or retiming"
                )
                .into());
            }
            let source = reference(project, native.clip, "Clip/Source")?;
            let target = reference(project, source, "SequenceSource/Sequence")?;
            if tree.tag(source) != "VideoSequenceSource"
                || target != project.sequence(child).element
            {
                return Err(format!(
                    "{id}: Premiere nest edit source disagrees with its child item"
                )
                .into());
            }
            let child_length = if let Some(&length) = plan.lengths.get(&child) {
                if plan.uses.contains_key(&child) {
                    return Err(format!("{id}: Premiere nesting supports one level").into());
                }
                length
            } else {
                let items = video_items(project, child)?;
                if has_stereo_child_audio(project, child, &id)? {
                    plan.stereo_children.insert(child);
                }
                if items.iter().any(|&id| {
                    project
                        .track_item_source(id)
                        .is_some_and(|item| project.item(item).sequence_uid.is_some())
                }) {
                    return Err(format!("{id}: Premiere nesting supports one level").into());
                }
                if project.start(items[0])?.ticks != 0 {
                    return Err(format!(
                        "{id}: Premiere nest child must begin at its saved origin"
                    )
                    .into());
                }
                let length = extent(project, child)?;
                plan.lengths.insert(child, length);
                length
            };
            let saved_length = field(tree, source, "OriginalDuration")?
                .ok_or("Premiere nest source duration is absent")?
                .parse()?;
            let origin = frames(
                project
                    .sequence_bag_int(child, "MZ.ZeroPoint")?
                    .ok_or("Premiere nest child lacks a saved zero point")?,
            )?;
            let start = frames(project.start(clip)?.ticks)?;
            let length = frames(project.end(clip)?.ticks)? - start;
            let source_in = frames(project.in_point(clip)?.ticks)?;
            let source_out = frames(project.out_point(clip)?.ticks)?;
            if frames(saved_length)? != child_length
                || length <= 0
                || source_out - source_in != length
                || source_out > child_length
            {
                return Err(
                    format!("{id}: Premiere nest trim or source extent is contradictory").into(),
                );
            }
            if seen.insert(child) {
                if plan.sequences.len() == 64 {
                    return Err(
                        "Premiere selection and its nest children exceed the 64 timeline limit"
                            .into(),
                    );
                }
                plan.sequences.push(child);
            }
            rows.push(Value::Obj(vec![
                ("id".into(), Value::Str(id)),
                ("child_sequence".into(), Value::Str(uid.clone())),
                ("child_origin_frames".into(), Value::Int(origin)),
                ("child_length_frames".into(), Value::Int(child_length)),
                ("record_start_frames".into(), Value::Int(start)),
                ("record_duration_frames".into(), Value::Int(length)),
                ("saved_source_in_frames".into(), Value::Int(source_in)),
            ]));
        }
        if !rows.is_empty() {
            match check_audio(project, parent, &by_uid, &mut rows, &plan.stereo_children)? {
                AudioCompanions::Empty => { plan.empty_audio_parents.insert(parent); }
                AudioCompanions::Stereo => { plan.stereo_parents.insert(parent); }
                AudioCompanions::None => {}
            }
            plan.uses.insert(parent, rows);
        } else if has_nested_audio {
            return Err("Premiere nested audio needs a matching picture nest".into());
        }
    }
    Ok(plan)
}
