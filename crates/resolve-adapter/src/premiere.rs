//! Same-rate Premiere straight cuts into the shared Resolve turnover path.
use crate::{
    linked::{self, MediaSpec, Profile, ProjectSpec, TimelineSpec},
    media::media_digest,
    turnover::Turnover,
    Result, StreamSelector,
};
use graph::{
    source::{
        OriginalStream, SourceClock, SourceClockProvenance, SourceGraph, SourceItem,
        SourceItemMedia, SourceMedia, SourceSequence, SourceText, TextKind,
    },
    Bin, Loss, Placement, Project as GraphProject, Rational, Sidecar, StreamKind, Track, Value,
};
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{ItemId, ItemType, Project, TrackId, TrackItemId},
    xml::{NodeId, PremiereDocument},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_EDITS: usize = 12_288;

#[derive(Serialize)]
pub struct PremiereSequenceInfo {
    pub object_uid: Option<String>,
    pub name: String,
    pub rate: Option<String>,
    pub frame_size: Option<(i64, i64)>,
    pub zero_point_ticks: Option<String>,
    pub video: PremiereTrackCounts,
    pub audio: PremiereTrackCounts,
}

#[derive(Default, Serialize)]
pub struct PremiereTrackCounts {
    pub tracks_with_clips: usize,
    pub clips: usize,
    pub transitions: usize,
}

/// Read saved sequence metadata without opening or probing linked media.
pub fn list_premiere_sequences(path: &Path) -> Result<Vec<PremiereSequenceInfo>> {
    let (project, _) = read_project(&premiere_path(path)?)?;
    let track_counts = |tracks: &[TrackId]| {
        let mut counts = PremiereTrackCounts::default();
        for &track in tracks {
            let track = project.track(track);
            counts.tracks_with_clips += usize::from(!track.clips.is_empty());
            counts.clips += track.clips.len();
            counts.transitions += track.transitions.len();
        }
        counts
    };
    Ok(project
        .sequences()
        .into_iter()
        .map(|sequence| {
            let data = project.sequence(sequence);
            let rate = data
                .timebase
                .filter(|&ticks| ticks > 0)
                .and_then(|ticks| Rational::new(TICKS_PER_SECOND.into(), ticks.into()))
                .map(|rate| rate.to_string());
            PremiereSequenceInfo {
                object_uid: project.sequence_id(sequence),
                name: project.sequence_name(sequence),
                rate,
                frame_size: data.frame_size,
                zero_point_ticks: project.sequence_bag_text(sequence, "MZ.ZeroPoint"),
                video: track_counts(&data.video_tracks),
                audio: track_counts(&data.audio_tracks),
            }
        })
        .collect())
}

pub fn prepare_premiere_turnover(path: &Path, selector: &str, ffprobe: &Path) -> Result<Turnover> {
    let path = premiere_path(path)?;
    let (project, digest) = read_project(&path)?;
    let project = &project;
    let sequences = project.sequences();
    let sequence = if let Some(id) = sequences
        .iter()
        .copied()
        .find(|id| project.sequence_id(*id).as_deref() == Some(selector))
    {
        id
    } else {
        let mut matches = sequences
            .iter()
            .copied()
            .filter(|id| project.sequence_name(*id) == selector);
        let found = matches
            .next()
            .ok_or("selected Premiere sequence is absent")?;
        if matches.next().is_some() {
            return Err("ambiguous Premiere sequence name; select by ObjectUID".into());
        }
        found
    };
    let id = project
        .sequence_id(sequence)
        .ok_or("Premiere sequence lacks ObjectUID")?;
    let data = project.sequence(sequence);
    check_audio_bus(project, data.element)?;
    let frame_ticks = data
        .timebase
        .filter(|&n| n > 0)
        .ok_or("missing Premiere sequence frame timebase")?;
    let rate = Rational::new(TICKS_PER_SECOND.into(), frame_ticks.into())
        .ok_or("invalid sequence rate")?;
    if !matches!((rate.num(), rate.den()), (24, 1) | (24000, 1001))
        || data.frame_size != Some((1920, 1080))
    {
        return Err("Premiere turnover requires same-rate 1080p24 or 1080p23.976".into());
    }
    let zero = project
        .sequence_bag_int(sequence, "MZ.ZeroPoint")?
        .ok_or("explicit Premiere sequence start required")?;
    let origin = u64::try_from(zero / frame_ticks)?;
    if zero < 0 || zero % frame_ticks != 0 || origin >= 2_073_600 {
        return Err("Premiere sequence origin is off-frame or outside one day".into());
    }
    let clock = SourceClock {
        effective_count: origin,
        rate,
        display_base: 24,
        drop: false,
        provenance: SourceClockProvenance {
            origin_count: origin,
            source_offset: 0,
            source_rate: rate,
            source_package: id.clone(),
            timecode_track: "MZ.ZeroPoint".into(),
            route: "Premiere sequence ticks".into(),
        },
    };
    let sequence_panel_item = project
        .sequence_item(sequence)
        .ok_or("Premiere sequence panel owner is missing")?;
    let root = project
        .root_item()
        .ok_or("Premiere project root is absent")?;
    let mut graph = SourceGraph {
        project: GraphProject {
            name: path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("invalid project name")?
                .into(),
        },
        media: vec![],
        items: vec![],
        bins: vec![Bin {
            id: "premiere:root".into(),
            name: "Master".into(),
            parent: None,
            items: vec![],
        }],
        sequences: vec![],
        losses: vec![],
        sidecars: vec![],
        multicams: vec![],
        multicam_uses: vec![],
    };
    let mut folder_ids = HashMap::from([(root, 0)]);
    let sequence_bin = owning_bin(
        project,
        sequence_panel_item,
        &mut folder_ids,
        &mut graph.bins,
    )?;
    let mut out = SourceSequence {
        id: id.clone(),
        name: project.sequence_name(sequence),
        rate,
        frame_size: data.frame_size,
        start: clock.exact_start(),
        clock: Some(clock),
        tracks: vec![],
        placements: vec![],
        transitions: vec![],
        markers: vec![],
        texts: vec![],
    };
    let mut seen_items = HashMap::new();
    let mut spec = ProjectSpec {
        root_bin_id: Some("premiere:root".into()),
        timelines: vec![TimelineSpec {
            id: id.clone(),
            bin_id: graph.bins[sequence_bin].id.clone(),
            canvas: (1920, 1080),
            group: None,
        }],
        media: vec![],
        atom_media: vec![],
    };
    for (kind, tracks, expected) in [
        (StreamKind::Video, &data.video_tracks, 1),
        (StreamKind::Audio, &data.audio_tracks, 2),
    ] {
        let mut track_indexes = HashSet::new();
        for &track_id in tracks {
            let track = project.track(track_id);
            if !track.transitions.is_empty() {
                return Err(
                    "Premiere transitions require a qualified Resolve timing mapping".into(),
                );
            }
            if track.clips.is_empty() {
                continue;
            }
            let index = track
                .index
                .checked_add(1)
                .ok_or("Premiere track index overflow")?;
            if !(1..=expected).contains(&index) || !track_indexes.insert(index) {
                return Err("expected active V1 and independent A1/A2 Premiere tracks".into());
            }
            if project.is_muted(track_id)? {
                return Err("muted Premiere track requires a verified Resolve mix mapping".into());
            }
            if kind == StreamKind::Audio {
                check_neutral_mixer(project, track.element)?;
            }
            out.tracks.push(Track {
                kind,
                index,
                level_db: None,
            });
            for &clip in &track.clips {
                if out.placements.len() == MAX_EDITS {
                    return Err("Premiere sequence exceeds edit limit".into());
                }
                if project.is_disabled(clip)? {
                    return Err(
                        "disabled Premiere edit requires a verified Resolve enable mapping".into(),
                    );
                }
                let source = project
                    .track_item_source(clip)
                    .ok_or("unresolved Premiere source item")?;
                let (media, saved_path) = clip_media(project, clip, source, kind)?;
                let channel = if kind == StreamKind::Audio {
                    Some(channel(project, clip)?)
                } else {
                    None
                };
                let item = if let Some(&index) = seen_items.get(&source) {
                    index
                } else {
                    if seen_items.len() == 256 {
                        return Err("Premiere sequence exceeds linked media limit".into());
                    }
                    let bin = owning_bin(project, source, &mut folder_ids, &mut graph.bins)?;
                    let index = graph.items.len();
                    let binding = bind(
                        project,
                        source,
                        media,
                        &saved_path,
                        kind,
                        rate,
                        ffprobe,
                        bin,
                        &mut graph,
                    )?;
                    spec.media.push(binding);
                    seen_items.insert(source, index);
                    index
                };
                let binding = &spec.media[item];
                if !binding.streams.iter().any(|s| s.channel == channel) {
                    return Err(
                        "Premiere clip selects a nonexistent channel on its saved media".into(),
                    );
                }
                if project.speed(clip)? != 1.0 || project.is_speed_reversed(clip)? {
                    return Err(
                        "Premiere clip speed or direction is outside the straight-cut profile"
                            .into(),
                    );
                }
                let tree = &project.doc.tree;
                let native = project.track_item(clip);
                let occurrence = tree
                    .get(native.element, "ObjectID")
                    .ok_or("Premiere edit lacks ObjectID")?;
                let placement_id = format!("premiere:edit:{occurrence}");
                let mut effects = Vec::new();
                for component in &native.components {
                    let name = project.component_match_name(*component);
                    if name.to_ascii_lowercase().contains("remap") {
                        return Err(
                            "Premiere time remapping requires a Resolve timing mapping".into()
                        );
                    }
                    effects.push(Value::Str(name));
                }
                if !effects.is_empty() || !native.selection_components.is_empty() {
                    graph.losses.push(Loss {
                        object: placement_id.clone(),
                        aspect: "premiere.appearance".into(),
                        reason: "Premiere clip components are omitted from the straight-cut output"
                            .into(),
                    });
                    graph.sidecars.push(Sidecar {
                        adapter: "premiere.components".into(),
                        object: placement_id.clone(),
                        data: Value::Arr(effects),
                    });
                }
                let start = project.start(clip)?.ticks;
                let end = project.end(clip)?.ticks;
                let source_in = project.in_point(clip)?.ticks;
                let source_out = project.out_point(clip)?.ticks;
                let span = end
                    .checked_sub(start)
                    .ok_or("Premiere record span overflow")?;
                if span <= 0
                    || start < 0
                    || source_in < 0
                    || source_out.checked_sub(source_in) != Some(span)
                    || [start, end, source_in, source_out]
                        .into_iter()
                        .any(|v| v % frame_ticks != 0)
                {
                    return Err(
                        "Premiere record/source ranges require whole, equal frame spans".into(),
                    );
                }
                out.placements.push(Placement {
                    id: placement_id,
                    item: binding.item_id.clone(),
                    record_start: ticks(start)?,
                    source_in: ticks(source_in)?,
                    duration: ticks(span)?,
                    video_track: (kind == StreamKind::Video).then_some(index),
                    audio_tracks: channel.map(|_| vec![index]).unwrap_or_default(),
                    audio_streams: channel.map(|ch| vec![ch as i64 + 1]).unwrap_or_default(),
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
            }
        }
        if track_indexes.len() != expected as usize {
            return Err("Premiere profile requires picture and two nonempty mono lanes".into());
        }
    }
    let mut captions = Vec::new();
    let mut caption_omissions = Vec::new();
    for track in project.caption_tracks(sequence) {
        let lane = project
            .caption_track_index(track)?
            .checked_add(1)
            .ok_or("caption index overflow")?;
        for caption in project.captions(track) {
            if captions.len() == MAX_EDITS {
                return Err("Premiere caption count exceeds edit limit".into());
            }
            let start = project.caption_start(caption)?.ticks;
            let duration = project
                .caption_end(caption)?
                .ticks
                .checked_sub(start)
                .ok_or("caption duration overflow")?;
            if lane < 1 || start < 0 || duration <= 0 {
                return Err("invalid Premiere caption range".into());
            }
            let occurrence = format!("{id}/caption/{}", captions.len() + 1);
            let saved_text = project.caption_text(caption);
            let normalized = saved_text.replace("\r\n", "\n").replace('\r', "\n");
            let content = (normalized.len() <= 32_768
                && !normalized.chars().any(|c| c.is_control() && c != '\n' && c != '\t'))
                .then_some(normalized);
            let reason = content.is_none().then(|| {
                "saved caption exceeds graph text limits or contains unsupported control characters".into()
            });
            out.texts.push(SourceText {
                id: occurrence.clone(),
                kind: TextKind::Caption,
                track: lane,
                content,
                record_start: Some(ticks(start)?),
                duration: Some(ticks(duration)?),
                reason,
            });
            captions.push(Value::Obj(vec![
                ("occurrence".into(), Value::Str(occurrence.clone())),
                ("track".into(), Value::Int(lane)),
                ("text".into(), Value::Str(saved_text.into())),
                ("start_ticks".into(), Value::Int(start)),
                ("duration_ticks".into(), Value::Int(duration)),
            ]));
            caption_omissions.push(Loss {
                object: occurrence,
                aspect: crate::turnover::PREMIERE_CAPTION_LOSS.into(),
                reason: "Premiere caption is omitted from the straight-cut Resolve output".into(),
            });
        }
    }
    if !captions.is_empty() {
        graph.sidecars.push(Sidecar {
            adapter: "premiere.captions".into(),
            object: id.clone(),
            data: Value::Arr(captions),
        });
    }
    let markers = project.sequence_markers(sequence);
    if markers.len() > MAX_EDITS {
        return Err("Premiere marker count exceeds edit limit".into());
    }
    let mut marker_receipts = Vec::with_capacity(markers.len());
    let tree = &project.doc.tree;
    for (ordinal, &marker) in markers.iter().enumerate() {
        let occurrence = format!("{id}/marker/{}", ordinal + 1);
        let mut fields = vec![("occurrence".into(), Value::Str(occurrence.clone()))];
        if let Some(native_id) = tree.get(marker, "ObjectID") {
            fields.push(("native_object_id".into(), Value::Str(native_id.into())));
        }
        if let Some(payload) = tree.child_by_tag(marker, "DVAMarker").and_then(|node| tree.text(node)) {
            fields.push(("byte_count".into(), Value::Int(payload.len() as i64)));
            fields.push(("sha256".into(), Value::Str(hex::encode(Sha256::digest(payload.as_bytes())))));
            if payload.len() <= 4096 {
                fields.push(("saved_payload".into(), Value::Str(payload.into())));
            }
        }
        marker_receipts.push(Value::Obj(fields));
        graph.losses.push(Loss {
            object: id.clone(),
            aspect: "premiere.marker".into(),
            reason: format!(
                "Premiere sequence marker {occurrence} is omitted from the straight-cut output"
            ),
        });
    }
    if !marker_receipts.is_empty() {
        graph.sidecars.push(Sidecar {
            adapter: "premiere.markers".into(),
            object: id.clone(),
            data: Value::Arr(marker_receipts),
        });
    }
    graph.sidecars.push(Sidecar {
        adapter: "premiere.source".into(),
        object: id,
        data: Value::Obj(vec![
            (
                "bin_id".into(),
                Value::Str(graph.bins[sequence_bin].id.clone()),
            ),
            ("frame_ticks".into(), Value::Int(frame_ticks)),
            ("zero_point_ticks".into(), Value::Int(zero)),
        ]),
    });
    graph.sequences.push(out);
    Turnover::prepare(graph, spec, ffprobe, vec![(path, digest)], vec![], caption_omissions)
}

fn premiere_path(path: &Path) -> Result<PathBuf> {
    let path = std::fs::canonicalize(path)?;
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("prproj"))
    {
        return Err("expected a Premiere .prproj file".into());
    }
    Ok(path)
}

fn read_project(path: &Path) -> Result<(Project, [u8; 32])> {
    const MAX_INPUT: u64 = 32 << 20;
    const MAX_XML: u64 = 64 << 20;
    if !std::fs::metadata(path)?.is_file() {
        return Err("Premiere input must be a regular file".into());
    }
    let mut input = Vec::new();
    File::open(path)?
        .take(MAX_INPUT + 1)
        .read_to_end(&mut input)?;
    if input.len() as u64 > MAX_INPUT {
        return Err("Premiere input exceeds 32 MiB".into());
    }
    let digest = Sha256::digest(&input).into();
    let document = if input.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = flate2::bufread::GzDecoder::new(input.as_slice());
        let mut xml = Vec::new();
        (&mut decoder).take(MAX_XML + 1).read_to_end(&mut xml)?;
        if xml.len() as u64 > MAX_XML || !decoder.get_ref().is_empty() {
            return Err("Premiere input exceeds the XML limit or has trailing gzip members".into());
        }
        PremiereDocument::from_xml_bytes(&xml, None)?
    } else {
        PremiereDocument::from_xml_bytes(&input, None)?
    };
    Ok((Project::from_document(document, path)?, digest))
}

fn check_audio_bus(project: &Project, sequence: NodeId) -> Result<()> {
    let tree = &project.doc.tree;
    let mut buses = 0;
    for reference in tree.findall(sequence, "TrackGroups/TrackGroup/Second")? {
        let group = project.doc.resolve(reference)?;
        if tree.tag(group) != "AudioTrackGroup" {
            continue;
        }
        buses += 1;
        if tree.findtext(group, "NumAdaptiveChannels")?.as_deref() != Some("2") {
            return Err("expected a stereo Premiere mix bus".into());
        }
        let master = tree
            .find1(group, "MasterTrack")
            .ok_or("missing Premiere mix bus")?;
        check_neutral_mixer(project, project.doc.resolve(master)?)?;
    }
    if buses != 1 {
        return Err("expected one Premiere audio track group".into());
    }
    Ok(())
}

fn check_neutral_mixer(project: &Project, track: NodeId) -> Result<()> {
    let tree = &project.doc.tree;
    let check_param = |reference| -> Result<()> {
        let p = project.doc.resolve(reference)?;
        if prproj::model::component::param_is_time_varying(tree, p)
            || tree
                .findtext(p, "Keyframes")?
                .is_some_and(|v| !v.is_empty())
        {
            return Err("Premiere mixer automation requires an explicit mapping".into());
        }
        let name = tree.findtext(p, "Name")?.unwrap_or_default();
        let neutral = |value: &str| match name.as_str() {
            "Volume" => value.parse::<f64>() == Ok(1.0),
            "Mute" => value == "false",
            "Pan" => value.parse::<f64>() == Ok(0.0),
            "Balance" => value.parse::<f64>() == Ok(0.5),
            _ => false,
        };
        let current = tree.findtext(p, "CurrentValue")?;
        let start = tree.findtext(p, "StartKeyframe")?;
        if !matches!(name.as_str(), "Volume" | "Mute" | "Pan" | "Balance")
            || current
                .as_deref()
                .is_some_and(|v| !v.is_empty() && !neutral(v))
            || start
                .as_deref()
                .is_some_and(|v| !v.is_empty() && !v.split(',').nth(1).is_some_and(neutral))
        {
            return Err("non-neutral Premiere mixer control requires an explicit mapping".into());
        }
        Ok(())
    };
    let chain = tree
        .find1(track, "AudioTrack/ComponentOwner/Components")
        .ok_or("missing Premiere mixer chain")?;
    let chain = project.doc.resolve(chain)?;
    for reference in tree.findall(chain, "ComponentChain/Components/Component")? {
        let component = project.doc.resolve(reference)?;
        if !matches!(tree.tag(component), "AudioFader" | "AudioMeter") {
            return Err("unsupported Premiere mixer component".into());
        }
        for param in tree.findall(component, "AudioComponent/Component/Params/Param")? {
            check_param(param)?;
        }
    }
    let panner = tree
        .find1(track, "AudioTrack/Panner")
        .ok_or("missing Premiere panner")?;
    let panner = project.doc.resolve(panner)?;
    if !matches!(
        tree.tag(panner),
        "StereoToStereoPanProcessor" | "MonoToStereoPanProcessor" | "DefaultPanProcessor"
    ) {
        return Err("unsupported Premiere mixer channel routing".into());
    }
    for param in tree.findall(panner, "PanProcessor/AudioComponent/Component/Params/Param")? {
        check_param(param)?;
    }
    for field in ["IsSolo", "IsMuted"] {
        if tree
            .findtext(track, &format!("ClipTrack/Track/{field}"))?
            .is_some_and(|v| v == "true")
        {
            return Err("Premiere mute or solo state requires an explicit mapping".into());
        }
    }
    Ok(())
}

fn ticks(value: i64) -> Result<Rational> {
    Ok(
        Rational::new(value.into(), TICKS_PER_SECOND.into())
            .ok_or("invalid Premiere tick value")?,
    )
}

fn owning_bin(
    project: &Project,
    item: ItemId,
    ids: &mut HashMap<ItemId, usize>,
    bins: &mut Vec<Bin>,
) -> Result<usize> {
    let mut item = project
        .item(item)
        .parent
        .ok_or("unfiled Premiere panel item")?;
    let mut ancestry = Vec::new();
    let mut seen = HashSet::new();
    while !ids.contains_key(&item) {
        if project.item(item).item_type != ItemType::Bin
            || !seen.insert(item)
            || ancestry.len() >= 64
        {
            return Err("invalid Premiere bin ancestry".into());
        }
        ancestry.push(item);
        item = project
            .item(item)
            .parent
            .ok_or("Premiere bin is detached")?;
    }
    let mut parent = ids[&item];
    for bin in ancestry.into_iter().rev() {
        if bins.len() == 256 {
            return Err("Premiere project exceeds bin limit".into());
        }
        let element = project.item(bin).element;
        let uid = project
            .doc
            .tree
            .get(element, "ObjectUID")
            .ok_or("Premiere bin lacks ObjectUID")?;
        let index = bins.len();
        bins.push(Bin {
            id: format!("premiere:bin:{uid}"),
            parent: Some(bins[parent].id.clone()),
            name: project.item_name(bin),
            items: vec![],
        });
        ids.insert(bin, index);
        parent = index;
    }
    Ok(parent)
}

fn clip_media(
    project: &Project,
    clip: TrackItemId,
    source: ItemId,
    kind: StreamKind,
) -> Result<(NodeId, PathBuf)> {
    let clip = project.track_item(clip).clip;
    let tree = &project.doc.tree;
    let source_ref = tree
        .find1(clip, "Clip/Source")
        .ok_or("missing Premiere clip media reference")?;
    let media_source = project.doc.resolve(source_ref)?;
    if tree.tag(media_source)
        != if kind == StreamKind::Video {
            "VideoMediaSource"
        } else {
            "AudioMediaSource"
        }
    {
        return Err("Premiere edit does not refer to a physical media source".into());
    }
    let media = project.doc.resolve(
        tree.find1(media_source, "MediaSource/Media")
            .ok_or("missing Premiere media owner")?,
    )?;
    if Some(media) != project.media_element(source)? || project.item_start_time(source)?.ticks != 0
    {
        return Err(
            "Premiere edit and master media disagree or the item has a nonzero source origin"
                .into(),
        );
    }
    let locator = project
        .item_media_path(source)
        .ok_or("Premiere media locator is missing")?;
    let saved = PathBuf::from(locator);
    if !saved.is_absolute() {
        return Err("Premiere media requires an absolute saved path".into());
    }
    Ok((media, saved))
}

fn channel(project: &Project, clip: TrackItemId) -> Result<usize> {
    let tree = &project.doc.tree;
    let clip = project.track_item(clip).clip;
    let refs = tree.findall(clip, "SecondaryContents/SecondaryContentItem")?;
    let [reference] = refs.as_slice() else {
        return Err("Premiere audio edit is not an independent mono selection".into());
    };
    let selector = project.doc.resolve(*reference)?;
    let content = project.doc.resolve(
        tree.find1(selector, "Content")
            .ok_or("missing Premiere audio selector owner")?,
    )?;
    let source = project.doc.resolve(
        tree.find1(clip, "Clip/Source")
            .ok_or("missing Premiere audio source")?,
    )?;
    if content != source {
        return Err("Premiere audio selector is bound to a different source".into());
    }
    let index = tree
        .findtext(selector, "ChannelIndex")?
        .ok_or("missing Premiere physical channel index")?;
    Ok(index.parse()?)
}

#[allow(clippy::too_many_arguments)]
fn bind(
    project: &Project,
    item: ItemId,
    media: NodeId,
    saved: &Path,
    kind: StreamKind,
    rate: Rational,
    ffprobe: &Path,
    bin: usize,
    graph: &mut SourceGraph,
) -> Result<MediaSpec> {
    let path = std::fs::canonicalize(saved)?;
    let tree = &project.doc.tree;
    let uid = tree
        .get(project.item(item).element, "ObjectUID")
        .ok_or("Premiere source item lacks ObjectUID")?;
    let item_id = format!("premiere:item:{uid}");
    let name = project.item_name(item);
    let mut spec = MediaSpec {
        item_id: item_id.clone(),
        bin_id: graph.bins[bin].id.clone(),
        path: path.clone(),
        sha256: hex::encode(media_digest(&path)?),
        streams: vec![],
    };
    let (verified, tb, count) = linked::probe(&spec, ffprobe, name.clone(), bin)?;
    let duration = linked::extent(count, tb)?;
    let (container, codec, frame_size, sample_rate, bits, channels, stream_ref) =
        match (&verified.profile, kind) {
            (
                Profile::Mov {
                    rate: file_rate,
                    h264: false,
                    aac: false,
                    ..
                },
                StreamKind::Video,
            ) if *file_rate == rate => (
                "mov",
                "prores",
                Some((1920, 1080)),
                None,
                None,
                None,
                "VideoStream",
            ),
            (Profile::Wav { bits, channels, .. }, StreamKind::Audio) => (
                "wav",
                if *bits == 16 {
                    "pcm_s16le"
                } else {
                    "pcm_s24le"
                },
                None,
                Some(48_000),
                Some(*bits),
                Some(*channels),
                "AudioStream",
            ),
            _ => {
                return Err(
                    "Premiere turnover requires same-rate ProRes HQ picture or PCM WAV audio"
                        .into(),
                )
            }
        };
    let stream = project.doc.resolve(
        tree.find1(media, stream_ref)
            .ok_or("saved Premiere stream metadata is missing")?,
    )?;
    let saved_ticks: i64 = tree
        .findtext(stream, "Duration")?
        .ok_or("saved Premiere stream duration is missing")?
        .parse()?;
    let saved_frame_ticks: i64 = tree
        .findtext(stream, "FrameRate")?
        .ok_or("saved Premiere stream clock is missing")?
        .parse()?;
    if ticks(saved_ticks)? != duration
        || (kind == StreamKind::Video
            && Rational::new(TICKS_PER_SECOND.into(), saved_frame_ticks.into()) != Some(rate))
        || (kind == StreamKind::Audio && saved_frame_ticks != TICKS_PER_SECOND / 48_000)
    {
        return Err(
            "Premiere cached stream clock or duration disagrees with physical media".into(),
        );
    }
    let locator = path.to_str().ok_or("invalid media locator")?.to_owned();
    let saved_locator = saved
        .to_str()
        .ok_or("invalid saved media locator")?
        .to_owned();
    let mut item_media = Vec::new();
    for n in 0..channels.unwrap_or(1) {
        let id = format!("{item_id}/media/{n}");
        let channel = channels.map(|_| n);
        graph.media.push(SourceMedia {
            id: id.clone(),
            kind,
            locations: vec![locator.clone()],
            codec: Some(codec.into()),
            frame_size,
            rate: (kind == StreamKind::Video).then_some(rate),
            sample_rate,
            bits,
            duration,
            start: None,
            clock: None,
            stream: channel.map(|n| n + 1),
            online: Some(true),
            original: Some(OriginalStream {
                saved_locators: vec![saved_locator.clone()],
                container: container.into(),
                stream_index: 0,
                channel_index: channel,
                channels,
                time_base: tb,
                start_pts: 0,
                duration_ts: count,
                sample_representation: bits.map(|n| {
                    if n == 16 {
                        "pcm16".into()
                    } else {
                        "pcm24".into()
                    }
                }),
            }),
        });
        item_media.push(SourceItemMedia {
            media: id.clone(),
            gain_db: 0.0,
            source_offset: Some(Rational::ZERO),
        });
        spec.streams.push(StreamSelector {
            media_id: id,
            index: 0,
            channel: channel.map(|n| n as usize),
        });
    }
    graph.bins[bin].items.push(item_id.clone());
    graph.items.push(SourceItem {
        id: item_id,
        name,
        media: item_media,
        umid: None,
        tape: None,
        rate: (kind == StreamKind::Video).then_some(rate),
        color_rgb16: None,
        stream_groups: if channels == Some(2) {
            vec![vec![1, 2]]
        } else {
            vec![]
        },
        markers: vec![],
    });
    Ok(spec)
}
