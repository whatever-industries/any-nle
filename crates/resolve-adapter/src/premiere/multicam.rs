//! Premiere's sequence-backed two-angle source with two fixed mono outputs.
use super::{components::field, mixer, owning_bin, source_clock, ticks, SelectionCapture};
use crate::{
    linked::{GroupSpec, MediaSpec},
    prepare::{rate, MAX_EDITS_PER_LANE},
    BindingSpec, Result, StreamSelector, VerifiedBinding,
};
use graph::{
    source::{
        Multicam, MulticamAlternative, MulticamLane, MulticamPiece, MulticamUse, OriginalStream,
        SourceBinding, SourceItem, SourceItemMedia, SourceMedia, SourceSequence, SourceTrack,
    },
    Placement, Rational, Sidecar, StreamKind, Value,
};
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{ItemId, Project, SequenceId, TrackId, TrackItemId},
    xml::NodeId,
};
use std::path::{Path, PathBuf};

pub(crate) const CAMERA: &str = "premiere.multicam_camera";
const FRAME: i64 = 10_594_584_000;
const SAMPLE: i64 = TICKS_PER_SECOND / 48_000;
const MONO: &str = r#"[{"channellabel":0}]"#;

pub(super) fn source(project: &Project, sequence: SequenceId) -> Result<Option<ItemId>> {
    let mut group = None;
    for clip in project.sequence_clips(sequence) {
        if let Some(item) = project.track_item_source(clip) {
            if project.is_multicam_clip(item)? {
                if group.is_some_and(|saved| saved != item) {
                    return Err(
                        "selected Premiere timeline uses more than one multicam source".into(),
                    );
                }
                group = Some(item);
            }
        }
    }
    Ok(group)
}

fn reference(project: &Project, owner: NodeId, path: &str) -> Result<NodeId> {
    let refs = project.doc.tree.findall(owner, path)?;
    let [node] = refs.as_slice() else {
        return Err(format!("missing or ambiguous Premiere multicam reference {path}").into());
    };
    Ok(project.doc.resolve(*node)?)
}

fn number(project: &Project, node: NodeId, path: &str) -> Result<Option<i64>> {
    let tree = &project.doc.tree;
    match tree.findall(node, path)?.as_slice() {
        [] => Ok(None),
        [value] if tree.is_childless(*value) => Ok(Some(
            tree.text(*value)
                .ok_or("empty Premiere multicam value")?
                .parse()?,
        )),
        _ => Err(format!("ambiguous Premiere multicam value {path}").into()),
    }
}

fn tracks(project: &Project, tracks: &[TrackId], limit: usize) -> Result<Vec<TrackId>> {
    let mut out = vec![];
    for &id in tracks {
        let track = project.track(id);
        if !track.transitions.is_empty() {
            return Err(
                "transitions on Premiere multicam tracks require a separate mapping".into(),
            );
        }
        if track.clips.is_empty() {
            continue;
        }
        if track.clips.len() > MAX_EDITS_PER_LANE
            || (track.media_type == prproj::model::MediaType::Video && project.is_muted(id)?)
        {
            return Err("Premiere multicam edit count or track state is unsupported".into());
        }
        out.push(id);
    }
    out.sort_unstable_by_key(|&id| project.track(id).index);
    if out.len() != limit
        || out
            .iter()
            .enumerate()
            .any(|(i, &id)| project.track(id).index != i as i64)
    {
        return Err("unsupported Premiere multicam track topology".into());
    }
    Ok(out)
}

#[derive(Clone, Copy)]
struct Range {
    start: i64,
    source_in: i64,
    duration: i64,
}

fn edit(project: &Project, clip: TrackItemId) -> Result<Range> {
    let native = project.track_item(clip);
    let tree = &project.doc.tree;
    if project.is_disabled(clip)?
        || project.speed(clip)? != 1.0
        || project.is_speed_reversed(clip)?
        || !native.components.is_empty()
        || !native.selection_components.is_empty()
        || !tree
            .findall(native.element, "ClipTrackItem/HeadTransition")?
            .is_empty()
        || !tree
            .findall(native.element, "ClipTrackItem/TailTransition")?
            .is_empty()
    {
        return Err(
            "Premiere multicam clip must be enabled and free of effects, transitions and retiming"
                .into(),
        );
    }
    let start = project.start(clip)?.ticks;
    let end = project.end(clip)?.ticks;
    let source_in = project.in_point(clip)?.ticks;
    let source_out = project.out_point(clip)?.ticks;
    let duration = end
        .checked_sub(start)
        .ok_or("Premiere multicam record span overflows")?;
    if [start, end, source_in, source_out]
        .into_iter()
        .any(|v| v < 0 || v % FRAME != 0)
        || duration <= 0
        || source_out.checked_sub(source_in) != Some(duration)
    {
        return Err(
            "Premiere multicam needs whole-frame, same-rate source and record ranges".into(),
        );
    }
    Ok(Range {
        start,
        source_in,
        duration,
    })
}

fn clip_source(project: &Project, clip: TrackItemId) -> Result<NodeId> {
    reference(project, project.track_item(clip).clip, "Clip/Source")
}

fn stream_group(project: &Project, item: ItemId, lane: usize) -> Result<(usize, i64)> {
    let tree = &project.doc.tree;
    let master = project
        .item(item)
        .master
        .ok_or("missing Premiere multicam master")?;
    let groups = reference(project, master, "AudioClipChannelGroups")?;
    let vectors = tree.findall(groups, "ClipChannelVectors/ClipChannelVectorItem")?;
    let [first, second] = vectors.as_slice() else {
        return Err("Premiere multicam needs two independent mono channel groups".into());
    };
    if [tree.get(*first, "Index"), tree.get(*second, "Index")] != [Some("0"), Some("1")] {
        return Err("Premiere multicam audio group indices are ambiguous".into());
    }
    let vector = project.doc.resolve(vectors[lane])?;
    let channel = reference(project, vector, "ClipChannels/ClipChannelItem")?;
    let source =
        number(project, channel, "SourceClipIndex")?.ok_or("missing audio group source clip")?;
    let index =
        number(project, channel, "mSourceChannelIndex")?.ok_or("missing audio group channel")?;
    if field(tree, vector, "ChannelType")? != Some("0") || index != lane as i64 {
        return Err("unsupported logical-to-physical Premiere mono channel mapping".into());
    }
    Ok((usize::try_from(source)?, index))
}

fn audio_selection(
    project: &Project,
    clip: TrackItemId,
    item: ItemId,
    lane: usize,
) -> Result<usize> {
    let native = project.track_item(clip);
    let tree = &project.doc.tree;
    let (audio_clip, channel) = stream_group(project, item, lane)?;
    if number(project, native.subclip, "OrigChGrp")?.unwrap_or(0) != lane as i64
        || lane.checked_sub(audio_clip)
            != Some(usize::try_from(
                number(project, native.clip, "SecondaryIndex")?.unwrap_or(0),
            )?)
        || super::channel(project, clip)? != channel as usize
        || field(tree, native.clip, "AudioChannelLayout")? != Some(MONO)
    {
        return Err(
            "Premiere multicam mono selectors disagree with the saved master channel groups".into(),
        );
    }
    Ok(audio_clip)
}

fn physical(
    project: &Project,
    clip: TrackItemId,
    item: ItemId,
    kind: StreamKind,
    lane: usize,
) -> Result<(NodeId, PathBuf, i64)> {
    let tree = &project.doc.tree;
    let source = clip_source(project, clip)?;
    let audio = kind == StreamKind::Audio;
    if tree.tag(source)
        != if audio {
            "AudioMediaSource"
        } else {
            "VideoMediaSource"
        }
    {
        return Err("Premiere multicam leaf must link a physical source".into());
    }
    let mut owners = vec![];
    for &master_clip in &project.item(item).clip_elements {
        if tree.tag(master_clip) == if audio { "AudioClip" } else { "VideoClip" } {
            owners.push(reference(project, master_clip, "Clip/Source")?);
        }
    }
    let candidate = if audio {
        audio_selection(project, clip, item, lane)?
    } else {
        0
    };
    if owners.get(candidate) != Some(&source) || (!audio && owners.len() != 1) {
        return Err("Premiere multicam leaf disagrees with the saved camera master".into());
    }
    let media = reference(project, source, "MediaSource/Media")?;
    let path = project
        .item_media_path(item)
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or("missing absolute Premiere camera path")?;
    super::metadata::check_locators(tree, media, path)?;
    if field(tree, media, "UseAlternateStart")? == Some("true") {
        return Err("Premiere multicam camera has an unqualified alternate media start".into());
    }
    let duration =
        number(project, source, "OriginalDuration")?.ok_or("missing saved camera duration")?;
    Ok((media, path.to_path_buf(), duration))
}

fn leaf(project: &Project, track: TrackId) -> Result<TrackItemId> {
    let [clip] = project.track(track).clips.as_slice() else {
        return Err("each Premiere multicam source track needs one continuous camera edit".into());
    };
    Ok(*clip)
}

fn camera(
    project: &Project,
    clip: TrackItemId,
    audio: [TrackItemId; 2],
    ffprobe: &Path,
    capture: &mut SelectionCapture,
) -> Result<(VerifiedBinding, Range)> {
    let item = project
        .track_item_source(clip)
        .ok_or("unresolved Premiere multicam camera item")?;
    let tree = &project.doc.tree;
    let camera_id = tree
        .get(project.item(item).element, "ObjectUID")
        .ok_or("Premiere multicam camera has no project item identity")?;
    let item_id = format!("premiere:item:{camera_id}");
    let (media, saved, declared) = physical(project, clip, item, StreamKind::Video, 0)?;
    let extent = edit(project, clip)?;
    if extent.source_in != 0
        || extent.duration != declared
        || !project.item(item).markers.is_empty()
    {
        return Err(
            "Premiere multicam camera must expose the full original without source markers".into(),
        );
    }
    for (lane, clip) in audio.into_iter().enumerate() {
        if project.track_item_source(clip) != Some(item) {
            return Err("Premiere multicam audio belongs to another camera".into());
        }
        let (_, path, duration) = physical(project, clip, item, StreamKind::Audio, lane)?;
        let trim = edit(project, clip)?;
        if path != saved
            || duration != declared
            || (trim.start, trim.source_in, trim.duration) != (extent.start, 0, declared)
        {
            return Err("Premiere multicam audio and picture extents disagree".into());
        }
    }
    let path = std::fs::canonicalize(&saved)?;
    let dig = hex::encode(crate::media::media_digest(&path)?);
    let streams = || {
        std::array::from_fn(|index| StreamSelector {
            media_id: format!("{item_id}/media/{index}"),
            index,
            channel: (index > 0).then_some(0),
        })
    };
    let binding = crate::verify_group_binding(
        BindingSpec {
            item_id: item_id.clone(),
            path: path.clone(),
            sha256: dig.clone(),
            streams: streams(),
        },
        ffprobe,
    )?;
    if binding.frames.checked_mul(FRAME) != Some(declared) {
        return Err("Premiere saved camera duration disagrees with the verified original".into());
    }
    let graph = &mut capture.graph.graph;
    if graph.items.iter().any(|row| row.id == item_id) {
        return Err("duplicate Premiere multicam camera identity in selected project".into());
    }
    let bin = owning_bin(
        project,
        item,
        &mut capture.graph.folder_ids,
        &mut graph.bins,
    )?;
    let mut links = vec![];
    for stream in &binding.streams {
        let audio = stream.index > 0;
        let clock = source_clock(
            binding.timecode_frames.try_into()?,
            rate(),
            24,
            false,
            &item_id,
            "MXF material timecode",
            "verified Premiere OP1a original",
        );
        let id = stream.media_id.clone();
        links.push(SourceItemMedia {
            media: id.clone(),
            gain_db: 0.0,
            source_offset: Some(Rational::ZERO),
        });
        graph.media.push(SourceMedia {
            id,
            kind: if audio {
                StreamKind::Audio
            } else {
                StreamKind::Video
            },
            locations: vec![crate::prepare::utf8_path(&path)?.into()],
            codec: Some(if audio { "pcm" } else { "AVdn" }.into()),
            frame_size: (!audio).then_some((1920, 1080)),
            rate: (!audio).then_some(rate()),
            sample_rate: audio.then_some(48_000),
            bits: audio.then_some(24),
            duration: ticks(declared)?,
            start: clock.exact_start(),
            clock: Some(clock),
            stream: Some(if audio { stream.index as i64 } else { 1 }),
            online: Some(true),
            original: Some(OriginalStream {
                saved_locators: vec![crate::prepare::utf8_path(&saved)?.into()],
                container: "mxf".into(),
                stream_index: stream.index as i64,
                channel_index: audio.then_some(0),
                channels: audio.then_some(1),
                time_base: if audio {
                    Rational::new(1, 48000).unwrap()
                } else {
                    Rational::new(1001, 24000).unwrap()
                },
                start_pts: 0,
                duration_ts: if audio {
                    declared / SAMPLE
                } else {
                    binding.frames
                },
                sample_representation: audio.then(|| "pcm24".into()),
            }),
        });
    }
    graph.items.push(SourceItem {
        id: item_id.clone(),
        name: project.item_name(item),
        media: links,
        umid: None,
        tape: None,
        rate: Some(rate()),
        color_rgb16: None,
        stream_groups: vec![],
        markers: vec![],
    });
    graph.bins[bin].items.push(item_id.clone());
    graph.sidecars.push(Sidecar {
        adapter: CAMERA.into(),
        object: item_id.clone(),
        data: Value::Obj(vec![
            (
                "media_uid".into(),
                Value::Str(
                    tree.get(media, "ObjectUID")
                        .ok_or("camera Media lacks identity")?
                        .into(),
                ),
            ),
            (
                "file_package".into(),
                Value::Str(hex::encode_upper(binding.source_package)),
            ),
        ]),
    });
    capture.spec.media.push(MediaSpec {
        streams: Vec::from(streams()),
        item_id,
        bin_id: graph.bins[bin].id.clone(),
        path,
        sha256: dig,
    });
    Ok((binding, extent))
}

fn clip_pan(project: &Project, clip: TrackItemId, lane: usize) -> Result<()> {
    let tree = &project.doc.tree;
    let panner = reference(project, project.track_item(clip).element, "Panner")?;
    let parameter = reference(
        project,
        panner,
        "PanProcessor/AudioComponent/Component/Params/Param",
    )?;
    let invalid =
        || "Premiere multicam camera A needs separate hard-panned mono transport".to_owned();
    let (start, current) = mixer::static_mixer_value(tree, parameter, invalid)?;
    if tree.tag(panner) != "MonoToStereoPanProcessor"
        || field(tree, parameter, "Name")? != Some("Pan")
        || field(tree, parameter, "IsInverted")? != Some("true")
        || (lane == 0 && (start.is_none() || current.is_none()))
        || (lane == 1 && start.is_some() != current.is_some())
        || [start, current]
            .into_iter()
            .flatten()
            .any(|value| value.parse::<f64>() != Ok(lane as f64))
    {
        return Err(invalid().into());
    }
    Ok(())
}

fn group_source(project: &Project, item: ItemId) -> Result<(NodeId, NodeId, SequenceId, i64)> {
    let tree = &project.doc.tree;
    let mut sources = [None; 2];
    for &clip in &project.item(item).clip_elements {
        let source = reference(project, clip, "Clip/Source")?;
        let index = match (tree.tag(clip), tree.tag(source)) {
            ("VideoClip", "VideoSequenceSource") => 0,
            ("AudioClip", "AudioSequenceSource") => 1,
            _ => return Err("unsupported Premiere multicam master source".into()),
        };
        if sources[index].replace(source).is_some() {
            return Err("ambiguous Premiere multicam master source".into());
        }
        if index == 0 && number(project, clip, "Clip/SelectedTrackIndex")?.unwrap_or(0) != 0 {
            return Err(
                "Premiere multicam default picture angle is outside the Resolve profile".into(),
            );
        }
    }
    let [Some(video), Some(audio)] = sources else {
        return Err("Premiere multicam must own one video and one audio sequence source".into());
    };
    let backing = reference(project, video, "SequenceSource/Sequence")?;
    if reference(project, audio, "SequenceSource/Sequence")? != backing {
        return Err(
            "Premiere multicam audio and picture reference different backing sequences".into(),
        );
    }
    let sequence = project
        .sequences()
        .into_iter()
        .find(|&id| project.sequence(id).element == backing)
        .ok_or("Premiere multicam backing sequence is unresolved")?;
    let duration =
        number(project, video, "OriginalDuration")?.ok_or("missing Premiere multicam duration")?;
    if duration <= 0
        || duration % FRAME != 0
        || number(project, audio, "OriginalDuration")? != Some(duration)
    {
        return Err("Premiere multicam sources disagree on their whole-frame extent".into());
    }
    Ok((video, audio, sequence, duration))
}

fn check_mix_inputs(project: &Project, sequence: NodeId, tracks: &[TrackId]) -> Result<()> {
    let tree = &project.doc.tree;
    let mut mix = None;
    for group in tree.findall(sequence, "TrackGroups/TrackGroup/Second")? {
        let group = project.doc.resolve(group)?;
        if tree.tag(group) == "AudioTrackGroup" {
            if mix
                .replace(reference(project, group, "MasterTrack")?)
                .is_some()
            {
                return Err("ambiguous Premiere multicam Mix bus".into());
            }
        }
    }
    let master = mix.ok_or("missing Premiere multicam Mix bus")?;
    let inlet = reference(project, master, "Inlet")?;
    let inputs = tree.findall(inlet, "Sources/Source")?;
    if inputs.len() != tracks.len() {
        return Err("Premiere multicam Mix bus inputs do not match its tracks".into());
    }
    for (index, (&input, &track)) in inputs.iter().zip(tracks).enumerate() {
        if tree
            .get(input, "Index")
            .and_then(|value| value.parse().ok())
            != Some(index)
            || project.doc.resolve(input)? != project.track(track).element
        {
            return Err("Premiere multicam Mix bus routing does not match its saved tracks".into());
        }
        let assigned = number(project, project.track(track).element, "AudioTrack/Assign")?;
        if assigned.is_some_and(|value| value != 0) {
            return Err("Premiere multicam audio track routes away from its Mix bus".into());
        }
    }
    Ok(())
}

pub(super) fn capture(
    project: &Project,
    sequence: SequenceId,
    item: ItemId,
    ffprobe: &Path,
    capture: &mut SelectionCapture,
    out: &mut SourceSequence,
) -> Result<GroupSpec> {
    let tree = &project.doc.tree;
    let (video_source, audio_source, backing, length) = group_source(project, item)?;
    if backing == sequence {
        return Err("Premiere multicam cannot refer to its own parent sequence".into());
    }
    let inner = project.sequence(backing);
    let outer = project.sequence(sequence);
    for id in [sequence, backing] {
        if !project.sequence_markers(id).is_empty()
            || project
                .caption_tracks(id)
                .iter()
                .any(|&track| !project.captions(track).is_empty())
        {
            return Err("Premiere multicam markers or captions require a separate mapping".into());
        }
    }
    if inner.timebase != Some(FRAME)
        || inner.frame_size != Some((1920, 1080))
        || inner.audio_channel_count != Some(2)
        || inner.audio_frame_rate != Some(SAMPLE)
        || outer.audio_channel_count != Some(2)
        || outer.audio_frame_rate != Some(SAMPLE)
        || mixer::read_mix_bus(project, inner.element, true)? != 0
        || project
            .sequence_bag_int(backing, "MZ.ZeroPoint")?
            .unwrap_or(0)
            != 0
    {
        return Err("unsupported Premiere multicam backing clocks or Mix bus".into());
    }
    let video = tracks(project, &inner.video_tracks, 2)?;
    let audio = tracks(project, &inner.audio_tracks, 4)?;
    check_mix_inputs(project, inner.element, &audio)?;
    let outer_video = tracks(project, &outer.video_tracks, 1)?[0];
    let outer_audio = tracks(project, &outer.audio_tracks, 2)?;
    for (index, &track) in audio.iter().enumerate() {
        let fader = mixer::read_multicam_strip(project, project.track(track).element)?;
        if fader.tenths_db != 0 || fader.pan != 0 || fader.muted != (index >= 2) {
            return Err(
                "Premiere multicam audio must use the first camera's two mono outputs".into(),
            );
        }
        if index < 2 {
            clip_pan(project, leaf(project, track)?, index)?;
        }
    }
    let (a, a_range) = camera(
        project,
        leaf(project, video[0])?,
        [leaf(project, audio[0])?, leaf(project, audio[1])?],
        ffprobe,
        capture,
    )?;
    let (b, b_range) = camera(
        project,
        leaf(project, video[1])?,
        [leaf(project, audio[2])?, leaf(project, audio[3])?],
        ffprobe,
        capture,
    )?;
    if a.path == b.path || a.source_package == b.source_package {
        return Err("Premiere multicam cameras share one physical original".into());
    }
    let cameras = [(a, a_range), (b, b_range)];
    let group_id = format!(
        "premiere:multicam:{}",
        tree.get(project.item(item).element, "ObjectUID")
            .ok_or("Premiere multicam panel item lacks an identity")?
    );
    let mut lanes = vec![];
    for (kind, lane, parent_track) in [
        (StreamKind::Video, 1, outer_video),
        (StreamKind::Audio, 1, outer_audio[0]),
        (StreamKind::Audio, 2, outer_audio[1]),
    ] {
        if kind == StreamKind::Audio {
            let fader = mixer::read_track(project, project.track(parent_track).element, true)?;
            if fader.tenths_db != 0 || fader.pan != 0 || fader.muted {
                return Err(
                    "Premiere multicam parent audio tracks need neutral faders and pan".into(),
                );
            }
        }
        out.tracks.push(SourceTrack::new(kind, lane, None));
        let mut alternatives = vec![];
        for (camera, span) in &cameras {
            let end = span
                .start
                .checked_add(span.duration)
                .ok_or("Premiere multicam camera range overflows")?;
            if end > length {
                return Err("Premiere multicam camera exceeds the saved group duration".into());
            }
            let gap = |start, duration| -> Result<MulticamPiece> {
                Ok(MulticamPiece {
                    start: ticks(start)?,
                    duration: ticks(duration)?,
                    item: None,
                    stream: None,
                    source_in: None,
                })
            };
            let mut pieces = vec![];
            if span.start > 0 {
                pieces.push(gap(0, span.start)?);
            }
            pieces.push(MulticamPiece {
                start: ticks(span.start)?,
                duration: ticks(span.duration)?,
                item: Some(camera.item_id.clone()),
                stream: Some(lane),
                source_in: Some(Rational::ZERO),
            });
            if end < length {
                pieces.push(gap(end, length - end)?);
            }
            alternatives.push(MulticamAlternative {
                id: format!("{group_id}/{}/{lane}/{}", kind.as_str(), camera.item_id),
                pieces,
                reason: None,
            });
        }
        lanes.push(MulticamLane {
            kind,
            index: lane,
            selected: Some(0),
            alternatives,
            reason: None,
        });
        for &clip in &project.track(parent_track).clips {
            let p = project.track_item(clip);
            let range = edit(project, clip)?;
            let video = kind == StreamKind::Video;
            if project.track_item_source(clip) != Some(item)
                || clip_source(project, clip)? != if video { video_source } else { audio_source }
            {
                return Err(
                    "Premiere multicam parent must use only the selected group on V1/A1/A2".into(),
                );
            }
            let selected = if video {
                number(project, p.clip, "Clip/SelectedTrackIndex")?.unwrap_or(0)
            } else {
                if audio_selection(project, clip, item, (lane - 1) as usize)? != 0
                    || !tree.findall(p.element, "Panner")?.is_empty()
                {
                    return Err(
                        "Premiere multicam parent audio is not a neutral fixed mono selection"
                            .into(),
                    );
                }
                0
            };
            let (camera, span) = cameras
                .get(usize::try_from(selected)?)
                .ok_or("Premiere multicam picture angle is outside the saved group")?;
            let source_in = range
                .source_in
                .checked_sub(span.start)
                .filter(|&value| value >= 0)
                .ok_or("Premiere multicam edit starts in an unavailable camera angle")?;
            if source_in
                .checked_add(range.duration)
                .is_none_or(|end| end > span.duration)
            {
                return Err("Premiere multicam edit exceeds its available camera angle".into());
            }
            let native_id = tree
                .get(p.element, "ObjectID")
                .ok_or("Premiere multicam edit lacks an identity")?;
            let id = format!("premiere:edit:{native_id}");
            let record_start = ticks(range.start)?;
            let duration = ticks(range.duration)?;
            out.placements.push(Placement {
                id: id.clone(),
                item: camera.item_id.clone(),
                record_start,
                duration,
                source_in: ticks(source_in)?,
                video_track: video.then_some(1),
                audio_tracks: if video { vec![] } else { vec![lane] },
                audio_streams: if video { vec![] } else { vec![lane] },
                grouped_audio: vec![],
                gain_db: 0.0,
                volume_db: None,
                volume_keys: vec![],
                track_gains_db: vec![],
                track_pans: vec![],
                speed: None,
                frame_map: None,
                transform: None,
                effects: vec![],
            });
            capture.graph.graph.multicam_uses.push(MulticamUse {
                id: format!("{id}/multicam"),
                sequence: out.id.clone(),
                candidates: vec![group_id.clone()],
                kind,
                lane: Some(lane),
                track: lane,
                selected: Some(selected),
                record_start: Some(record_start),
                duration: Some(duration),
                source_in: Some(ticks(range.source_in)?),
                binding: SourceBinding {
                    placements: vec![id],
                    reason: None,
                },
                reason: None,
            });
        }
    }
    let graph = &mut capture.graph.graph;
    let bin = owning_bin(
        project,
        item,
        &mut capture.graph.folder_ids,
        &mut graph.bins,
    )?;
    graph.multicams.push(Multicam {
        id: group_id.clone(),
        name: project.item_name(item),
        rate: Some(rate()),
        duration: Some(ticks(length)?),
        lanes,
        reason: None,
    });
    let bin_id = graph.bins[bin].id.clone();
    graph.sidecars.push(Sidecar {
        adapter: "premiere.multicam".into(),
        object: group_id,
        data: Value::Obj(vec![
            ("bin_id".into(), Value::Str(bin_id.clone())),
            (
                "backing_sequence".into(),
                Value::Str(
                    project
                        .sequence_id(backing)
                        .ok_or("Premiere multicam backing sequence lacks identity")?,
                ),
            ),
        ]),
    });
    Ok(GroupSpec {
        bin_id,
        cameras: cameras.map(|(binding, _)| binding.item_id.clone()),
    })
}
