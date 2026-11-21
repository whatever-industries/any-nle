//! Resolve's two picture angles and camera-A reference audio as a Premiere multicam.
use crate::{
    group::{member, prepare_native_group},
    media::{media_digest, verify_group_binding, BindingSpec},
    prepare::{frames, rate},
    source::{mixer::FAIRLIGHT_ADAPTER, ADAPTER, REFERENCE_AUDIO_SWITCH},
    Result, StreamSelector, VerifiedBinding,
};
use graph::{
    source::{SourceGraph, SourceItem, SourceSequence},
    Loss, StreamKind, Value,
};
use prproj::{
    convert::{
        model::{AudioFile, Bin, Clip, Event, Model, Options, Sequence, VideoFile},
        multicam::{
            PreparedCamera, PreparedGroupAudio, PreparedMulticam, PreparedMulticamUse, FRAME, RATE,
        },
        prepared::PreparedProjectInput,
        rewrap::MediaRelink,
        writer::{prepare_typed_project_for_premiere_27, TypedMedia},
    },
    RandomUuids,
};
use std::{collections::HashSet, path::Path, sync::Arc};

// Complete neutral mixer model on the retained Resolve Save As/reopen control.
const NEUTRAL_MIXER: &str = "6f9f0411d14f45daccac9b617134ef768f23bea282f33e221527797c49309874";

pub(super) fn write(
    graph: &SourceGraph,
    sequence: &SourceSequence,
    ffprobe: &Path,
    output: &Path,
    accept_losses: bool,
) -> Result<Vec<Loss>> {
    let fail = |reason: &str| format!("{}: {reason}", sequence.id);
    let uses: HashSet<_> = graph
        .multicam_uses
        .iter()
        .filter(|usage| usage.sequence == sequence.id && usage.kind == StreamKind::Audio)
        .map(|usage| usage.id.as_str())
        .collect();
    if let Some(loss) = graph.losses.iter().find(|loss| {
        (loss.aspect.as_str(), loss.reason.as_str()) == REFERENCE_AUDIO_SWITCH
            && uses.contains(loss.object.as_str())
    }) {
        return Err(format!("{}: {}", loss.object, loss.reason).into());
    }
    if sequence.frame_size != Some((1920, 1080))
        || sequence.rate != rate()
        || !sequence
            .clock
            .as_ref()
            .is_some_and(|clock| clock.provenance.route == "Resolve archive clock")
        || [
            crate::dissolve::ADAPTER,
            crate::track_state::ADAPTER,
            crate::generator::ADAPTER,
            "resolve.nested_uses",
        ]
        .into_iter()
        .any(|adapter| graph.sidecar(adapter, &sequence.id).is_some())
        || graph.sidecar(ADAPTER, &sequence.id).is_some_and(|receipt| {
            [super::MAIN_BUS_LEVEL, super::MUTED_AUDIO_TRACKS]
                .into_iter()
                .any(|field| receipt.get(field).is_some())
        })
        || !matches!(
            graph.sidecar(FAIRLIGHT_ADAPTER, &sequence.id).and_then(|receipt| receipt.get("mixer_sha256")),
            Some(Value::Str(hash)) if hash == NEUTRAL_MIXER
        )
    {
        return Err(fail("unsupported Resolve multicam clock, layout or Fairlight state").into());
    }
    let usage = graph
        .multicam_uses
        .iter()
        .find(|usage| usage.sequence == sequence.id)
        .ok_or_else(|| fail("missing Resolve multicam use"))?;
    let [group_id] = usage.candidates.as_slice() else {
        return Err(fail("ambiguous Resolve multicam identity").into());
    };
    let group = graph
        .multicams
        .iter()
        .find(|group| &group.id == group_id)
        .ok_or_else(|| fail("missing Resolve multicam definition"))?;
    let picture = group
        .lanes
        .iter()
        .find(|lane| lane.kind == StreamKind::Video && lane.index == 1)
        .ok_or_else(|| fail("missing Resolve multicam picture lane"))?;
    let [a, b] = picture.alternatives.as_slice() else {
        return Err(fail("expected exactly two Resolve multicam picture angles").into());
    };
    let length = frames(
        group
            .duration
            .ok_or_else(|| fail("unknown Resolve multicam duration"))?,
    )?;
    let camera_ids = [member(a, length)?.0, member(b, length)?.0];
    let camera_item = |id| {
        graph
            .items
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| fail("missing Resolve multicam camera"))
    };
    let items = [camera_item(camera_ids[0])?, camera_item(camera_ids[1])?];
    if !sequence.markers.is_empty()
        || items.iter().any(|item| !item.markers.is_empty())
        || sequence.placements.iter().any(|edit| {
            [
                super::EDIT_MARKER_ADAPTER,
                crate::clip_label::ADAPTER,
                crate::audio::UNQUALIFIED_FADE_ADAPTER,
            ]
            .into_iter()
            .any(|adapter| graph.sidecar(adapter, &edit.id).is_some())
        })
        || graph
            .sidecar(ADAPTER, &sequence.id)
            .and_then(|receipt| receipt.get(crate::timeline_marks::FIELD))
            .is_some()
    {
        return Err(fail(
            "markers, clip labels or clip envelopes on Resolve multicam are unqualified",
        )
        .into());
    }
    let default = Loss {
        object: group.id.clone(),
        aspect: "multicam_picture_default".into(),
        reason: "Resolve archive leaves the multicam picture default unknown. Fresh insertions use Premiere's own angle selection. Existing edit choices are preserved.".into(),
    };
    if !accept_losses {
        return Err(format!(
            "{}: {} Use --accept-losses to proceed.",
            default.object, default.reason
        )
        .into());
    }
    let cameras = [
        Arc::new(camera_binding(graph, items[0], ffprobe)?),
        Arc::new(camera_binding(graph, items[1], ffprobe)?),
    ];
    let prepared = prepare_native_group(graph, &sequence.id, cameras)
        .map_err(|error| format!("{}: {error}", group.id))?;
    let mut bindings = Vec::new();
    let clips = prepared
        .cameras
        .iter()
        .zip(items)
        .map(|(binding, item)| camera_clip(binding, item, &mut bindings))
        .collect();
    let mut destination = Sequence {
        name: prepared.sequence_name,
        rate: RATE,
        width: 1920,
        height: 1080,
        events: Vec::new(),
        markers: Vec::new(),
        track_levels: Vec::new(),
        audio_crossfades: Vec::new(),
        start: prepared.origin * FRAME,
    };
    let mut uses = Vec::new();
    for (lane, edits) in prepared.lanes.iter().enumerate() {
        for edit in edits {
            let label = lane as i64;
            let is_video = lane == 0;
            let group_in = edit.edit.source_in;
            uses.push(PreparedMulticamUse {
                sequence: 0,
                event: destination.events.len(),
                group: 0,
                source_in: group_in * FRAME,
                video: is_video.then_some(edit.camera),
                warp_picture_input: false,
                audio: if is_video {
                    vec![]
                } else {
                    vec![PreparedGroupAudio::Group(lane - 1)]
                },
            });
            destination.events.push(Event {
                clip: camera_ids[edit.camera].to_string(),
                record_start: edit.edit.start * FRAME,
                source_in: (group_in - prepared.offsets[edit.camera]) * FRAME,
                duration: edit.edit.length * FRAME,
                video_track: is_video.then_some(1),
                audio_tracks: if is_video { vec![] } else { vec![label] },
                audio_channels: if is_video { vec![] } else { vec![label] },
                audio: Default::default(),
                stereo: Vec::new(),
                audio_gains: Vec::new(),
                audio_pans: Vec::new(),
                speed_ratio: None,
                source_duration: None,
                color_correction: None,
                transform: None,
                matte: None,
                time_remap: None,
            });
        }
    }
    let source_bin = graph.bins.iter().find(|bin| {
        camera_ids
            .iter()
            .all(|&id| bin.items.iter().any(|item| item == id))
    });
    let model = Model {
        project_name: prepared.project_name,
        options: Options::default(),
        clips,
        bins: vec![Bin {
            name: source_bin.map_or_else(|| "Multicam Sources".into(), |bin| bin.name.clone()),
            clips: camera_ids.map(str::to_owned).to_vec(),
        }],
        sequences: vec![destination],
    };
    let mut input = PreparedProjectInput::from_model(&model)?;
    input.multicams.push(PreparedMulticam {
        id: group.id.clone(),
        name: prepared.group_name,
        bin: 0,
        duration: prepared.length * FRAME,
        cameras: std::array::from_fn(|index| PreparedCamera {
            clip: camera_ids[index].to_string(),
            start: prepared.offsets[index] * FRAME,
            source_in: 0,
            duration: prepared.cameras[index].frames * FRAME,
            audio_labels: vec![1, 2],
        }),
        audio_cameras: vec![0, 0],
    });
    input.multicam_uses = uses;
    let staged = prepare_typed_project_for_premiere_27(
        &input, output, &mut RandomUuids::new(),
        TypedMedia::Bound { bindings: &bindings, native_av: true }, None,
    )?;
    for camera in &prepared.cameras {
        camera.recheck()?;
    }
    staged.publish()?;
    Ok(vec![default])
}

fn camera_binding(graph: &SourceGraph, item: &SourceItem, tool: &Path) -> Result<VerifiedBinding> {
    let fail = |reason: &str| format!("{}: {reason}", item.id);
    if !matches!(graph.sidecar(ADAPTER, &item.id).and_then(|r| r.get("kind")),
        Some(Value::Str(kind)) if kind == "media_pool_clip")
    {
        return Err(fail("expected a saved Resolve camera master").into());
    }
    let media: Vec<_> = item
        .media
        .iter()
        .filter_map(|link| graph.media.iter().find(|m| m.id == link.media))
        .collect();
    let select = |index: usize, kind, logical| {
        let mut streams = media
            .iter()
            .filter(|m| m.kind == kind && m.stream == Some(logical));
        let selected = streams
            .next()
            .ok_or_else(|| fail("missing saved Resolve camera selector"))?;
        if streams.next().is_some() {
            return Err(fail("ambiguous saved Resolve camera selector"));
        }
        Ok((
            StreamSelector {
                media_id: selected.id.clone(),
                index,
                channel: (kind == StreamKind::Audio).then_some(0),
            },
            selected.locations.as_slice(),
        ))
    };
    let (video, [path]) = select(0, StreamKind::Video, 1)? else {
        return Err(fail("camera requires one saved picture locator").into());
    };
    let (a1, a1_paths) = select(1, StreamKind::Audio, 1)?;
    let (a2, a2_paths) = select(2, StreamKind::Audio, 2)?;
    if !Path::new(path).is_absolute() || a1_paths != [path.as_str()] || a2_paths != [path.as_str()]
    {
        return Err(
            fail("camera picture and mono tracks must share one absolute original path").into(),
        );
    }
    let digest = media_digest(Path::new(path))
        .map_err(|error| fail(&format!("could not read camera original {path}: {error}")))?;
    verify_group_binding(
        BindingSpec {
            item_id: item.id.clone(),
            path: path.into(),
            sha256: hex::encode(digest),
            streams: [video, a1, a2],
        },
        tool,
    )
    .map_err(|error| fail(&format!("camera original verification failed: {error}")).into())
}

fn camera_clip(
    binding: &VerifiedBinding,
    item: &SourceItem,
    mappings: &mut Vec<MediaRelink>,
) -> Clip {
    let duration = binding.frames * FRAME;
    let start = binding.timecode_frames * FRAME;
    let mut view = |name, audio_stream| {
        let source_path = format!("{}.anynle-{name}.mxf", binding.path.display());
        mappings.push(MediaRelink {
            source_path: source_path.clone(),
            target_path: binding.path.to_string_lossy().into_owned(),
            audio_stream,
            separate_mono: false,
            pixel_aspect: None,
        });
        source_path
    };
    Clip {
        id: item.id.clone(),
        name: item.name.clone(),
        material_umid: None,
        tape_name: item.tape.clone(),
        video: Some(VideoFile {
            path: view("V1", None),
            codec: "AVdn".into(),
            width: 1920,
            height: 1080,
            rate: RATE,
            duration,
            start,
        }),
        audio: (1..=2)
            .map(|label| AudioFile {
                path: view(if label == 1 { "A1" } else { "A2" }, Some(label as u32 - 1)),
                sample_rate: 48000,
                duration,
                bits: 24,
                avid_track: Some(label),
                start,
                gain_db: 0.0,
            })
            .collect(),
        avid: None,
        time_display: None,
        stereo_pairs: Vec::new(),
        multichannel_groups: Vec::new(),
        rate: Some(RATE),
        markers: Vec::new(),
    }
}
