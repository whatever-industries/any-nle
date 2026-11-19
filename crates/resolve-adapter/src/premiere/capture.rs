//! Premiere 27 linked-original media capture into graph/2. This input
//! profile is independent of the media a particular destination can write.
mod physical;
mod saved;

use super::{
    components, markers, metadata, premiere_path, read_project, select_sequence,
    source_capture::{owning_bin, source_clock, source_receipt, source_sequence, GraphCapture},
    ticks, MAX_EDITS,
};
use crate::{mixed_rate, Result};
use graph::{
    source::{
        OriginalStream, SourceClock, SourceGraph, SourceItem, SourceItemMedia, SourceMedia,
        SourceTrack,
    },
    GroupedAudio, Loss, Placement, Rational, Sidecar, StreamKind, Value,
};
use metadata::leaf;
use physical::Physical;
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{ItemId, Project, TrackItemId},
    xml::NodeId,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

/// Read one saved straight-cut sequence, verifying the linked originals and
/// resolving every occurrence's file-relative ranges and physical selectors.
pub fn read_premiere_media_graph(
    path: &Path,
    selector: &str,
    ffprobe: &Path,
) -> Result<SourceGraph> {
    let path = premiere_path(path)?;
    let before = physical::file_state(&path)?;
    let (project, _) = read_project(&path)?;
    if !components::is_current_project(&project)? {
        return Err("linked media capture requires a current Premiere project".into());
    }
    let sequence = select_sequence(&project, selector)?;
    let id = project
        .sequence_id(sequence)
        .ok_or("Premiere sequence lacks ObjectUID")?;
    let panel = project
        .sequence_item(sequence)
        .ok_or("Premiere sequence panel owner is missing")?;
    let root = project
        .root_item()
        .ok_or("Premiere project root is absent")?;
    let data = project.sequence(sequence);
    let frame_ticks = data
        .timebase
        .filter(|&n| n > 0)
        .ok_or("missing Premiere sequence clock")?;
    let rate = Rational::new(TICKS_PER_SECOND.into(), frame_ticks.into())
        .ok_or("invalid sequence rate")?;
    let base = physical::display_base(rate)?;
    let drop = match (
        rate.num(),
        rate.den(),
        project.video_display_format(sequence)?,
    ) {
        (24, 1, Some(100))
        | (24000, 1001, Some(110))
        | (25, 1, Some(101))
        | (30000, 1001, Some(103)) => false,
        (30000, 1001, Some(102)) => true,
        _ => return Err("Premiere sequence display clock contradicts its frame rate".into()),
    };
    let zero = project.zero_point(sequence)?.ticks;
    if zero < 0 || zero % frame_ticks != 0 {
        return Err("Premiere sequence origin is off-frame".into());
    }
    let origin = u64::try_from(zero / frame_ticks)?;
    if origin >= u64::from(base) * 86400 - if drop { 2592 } else { 0 } {
        return Err("Premiere sequence origin is outside one day".into());
    }
    let clock = source_clock(
        origin,
        rate,
        base,
        drop,
        &id,
        "MZ.ZeroPoint",
        "Premiere sequence ticks",
    );
    let mut out = source_sequence(
        id.clone(),
        project.sequence_name(sequence),
        Some(
            data.frame_size
                .ok_or("Premiere timeline raster is missing")?,
        ),
        clock,
    );
    let GraphCapture {
        mut graph,
        folder_ids: mut folders,
        sequence_bin,
    } = GraphCapture::new(&project, &path, root, panel)?;
    graph.sidecars.push(source_receipt(
        id.clone(),
        &graph.bins[sequence_bin].id,
        frame_ticks,
        zero,
        [],
    ));
    graph.sidecars.push(Sidecar {
        adapter: "premiere.media_capture".into(),
        object: id.clone(),
        data: Value::Str("linked media, exact edit clocks and channel selection; mixer and appearance require separate preparation".into()),
    });

    let mut files = HashMap::new();
    let mut items = HashMap::new();
    let mut component_budget = components::Budget::default();
    let mut sampled_frame_count = 0usize;
    let mut track_state = metadata::TrackStateCapture::default();
    for (kind, tracks) in [
        (StreamKind::Video, &data.video_tracks),
        (StreamKind::Audio, &data.audio_tracks),
    ] {
        let mut seen = HashSet::new();
        for &track_id in tracks {
            let track = project.track(track_id);
            if !track.transitions.is_empty() {
                return Err(format!(
                    "{id}: transition timing is outside Premiere linked media capture"
                )
                .into());
            }
            let index = track
                .index
                .checked_add(1)
                .filter(|n| *n > 0)
                .ok_or("invalid Premiere track index")?;
            if !seen.insert(index) {
                return Err("duplicate Premiere track index".into());
            }
            metadata::capture_track_state(
                &project.doc.tree,
                track.element,
                kind,
                index,
                &id,
                &mut track_state,
            )?;
            let mut declaration = SourceTrack::new(kind, index, None);
            declaration.name = project
                .doc
                .tree
                .findtext(
                    track.element,
                    "ClipTrack/Track/Node/Properties/MZ.TrackName",
                )?
                .filter(|name| !name.is_empty());
            declaration.empty = track.clips.is_empty();
            out.tracks.push(declaration);
            let mut intervals = Vec::with_capacity(track.clips.len());
            for &clip in &track.clips {
                if out.placements.len() + out.disabled_placements.len() == MAX_EDITS {
                    return Err("Premiere sequence exceeds edit limit".into());
                }
                let native = project.track_item(clip);
                let object = project
                    .doc
                    .tree
                    .get(native.element, "ObjectID")
                    .ok_or("Premiere edit lacks ObjectID")?;
                let occurrence = format!("premiere:edit:{object}");
                let source = project
                    .track_item_source(clip)
                    .ok_or("unresolved Premiere source item")?;
                if let std::collections::hash_map::Entry::Vacant(entry) = items.entry(source) {
                    if graph.items.len() == 256 {
                        return Err("Premiere sequence exceeds linked media limit".into());
                    }
                    let saved_path = Path::new(
                        project
                            .item_media_path(source)
                            .ok_or("Premiere original locator is missing")?,
                    );
                    if !saved_path.is_absolute() {
                        return Err("Premiere original requires an absolute saved path".into());
                    }
                    let canonical = fs::canonicalize(saved_path).map_err(|error| {
                        format!("{occurrence}: {}: {error}", saved_path.display())
                    })?;
                    if let std::collections::hash_map::Entry::Vacant(file) =
                        files.entry(canonical.clone())
                    {
                        file.insert(physical::inspect(&canonical, ffprobe).map_err(|error| {
                            format!("{occurrence}: {}: {error}", saved_path.display())
                        })?);
                    }
                    let bin = owning_bin(&project, source, &mut folders, &mut graph.bins)?;
                    let item = capture_item(
                        &project,
                        source,
                        saved_path,
                        &canonical,
                        &files[&canonical],
                        bin,
                        &mut graph,
                    )
                    .map_err(|error| format!("{occurrence}: {}: {error}", saved_path.display()))?;
                    entry.insert(item);
                }
                let item = &items[&source];
                let placement = placement(
                    &project,
                    clip,
                    kind,
                    index,
                    item,
                    &files[&item.path],
                    rate,
                    frame_ticks,
                    data.frame_size,
                    &occurrence,
                    &mut component_budget,
                    &mut sampled_frame_count,
                    &mut graph,
                )
                .map_err(|error| format!("{occurrence}: {error}"))?;
                metadata::capture_edit_label(
                    &project,
                    native.clip,
                    source,
                    &occurrence,
                    metadata::ClipLabelCapture::OverridesOnly,
                    &mut graph,
                )
                .map_err(|error| format!("Premiere label on {occurrence}: {error}"))?;
                if let Some(saved) =
                    components::capture(&project, clip, kind, &occurrence, &mut component_budget)?
                {
                    graph.sidecars.push(saved);
                }
                let color = if kind == StreamKind::Video {
                    super::color_mode::capture(&project, data.element, clip, source, &occurrence)?
                } else {
                    None
                };
                let mapped_color = color.as_ref()
                    .and_then(|row| super::color_mode::qualified_exposure(&row.data))
                    .is_some();
                graph.sidecars.extend(color);
                if (!mapped_color && (!native.components.is_empty() || !native.selection_components.is_empty()))
                    || project.doc.tree.find1(native.element, "Panner").is_some()
                {
                    graph.losses.push(Loss {
                        object: occurrence,
                        aspect: "premiere.appearance_or_mix".into(),
                        reason: "Clip processing needs separate destination preparation; this capture verifies the underlying linked edit".into(),
                    });
                }
                if kind == StreamKind::Video && files[&item.path].video.as_ref()
                    .is_some_and(|video| video.size.is_some() && video.size != data.frame_size)
                {
                    super::capture_input_scaling(&project, source, clip, &placement.id, &mut graph)?;
                }
                if project.is_disabled(clip)? {
                    out.disabled_placements.push(placement);
                } else {
                    let start = project.start(clip)?.ticks;
                    intervals.push((start, project.end(clip)?.ticks));
                    out.placements.push(placement);
                }
            }
            intervals.sort_unstable();
            if intervals.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                return Err(
                    format!("{id}: overlapping Premiere clips on {kind:?} track {index}").into(),
                );
            }
        }
    }
    if out.placements.is_empty() && out.disabled_placements.is_empty() {
        return Err("Premiere sequence has no linked media edits".into());
    }
    if !project.caption_tracks(sequence).is_empty() {
        return Err("Premiere captions require separate text preparation".into());
    }
    markers::capture(&project.doc.tree, data.element, &data.markers, &mut out, &mut graph)?;
    metadata::capture_timeline_marks(&project, data.element, &id, frame_ticks, &mut graph)?;
    for (adapter, rows) in [
        (crate::track_state::PREMIERE_ADAPTER, track_state.mapped),
        (metadata::SYNC_LOCK_LOSS, track_state.sync_unlocked),
    ] {
        if !rows.is_empty() {
            graph.sidecars.push(Sidecar {
                adapter: adapter.into(),
                object: id.clone(),
                data: Value::Arr(rows),
            });
        }
    }
    graph.losses.extend(track_state.losses);
    graph.sequences.push(out);
    let provenance = super::marker_provenance::read(&project, &graph)?;
    graph.sidecars.extend(provenance.into_iter().flat_map(|record| {
        record.receipts.into_iter().map(|row| row.sidecar)
    }));
    graph.validate()?;
    if physical::file_state(&path)? != before {
        return Err("Premiere project changed during capture".into());
    }
    for item in items.values() {
        if fs::canonicalize(&item.saved_path)? != item.path {
            return Err(format!(
                "Premiere linked locator changed during capture: {}",
                item.saved_path.display()
            )
            .into());
        }
    }
    for (path, physical) in files {
        if physical::file_state(&path)? != physical.state {
            return Err(format!(
                "Premiere linked original changed during capture: {}",
                path.display()
            )
            .into());
        }
    }
    Ok(graph)
}

struct Item {
    id: String,
    saved_path: PathBuf,
    path: PathBuf,
    primary: NodeId,
    bindings: saved::ItemBindings,
}

pub(super) struct VerifiedBinding {
    item: Item,
    physical: Physical,
}

impl VerifiedBinding {
    pub(super) fn bind(
        project: &Project,
        source: ItemId,
        saved_path: &Path,
        spec: &mut crate::linked::MediaSpec,
        ffprobe: &Path,
        bin: usize,
        graph: &mut SourceGraph,
    ) -> Result<Self> {
        let physical = physical::inspect(&spec.path, ffprobe)?;
        Self::bind_physical(project, source, saved_path, spec, physical, bin, graph)
    }

    pub(super) fn bind_standalone_movie(
        project: &Project,
        item: ItemId,
        saved_path: &Path,
        spec: &mut crate::linked::MediaSpec,
        verified: &crate::linked::Media,
        time_base: Rational,
        count: i64,
        bin: usize,
        graph: &mut SourceGraph,
    ) -> Result<Self> {
        use crate::linked::{self, MovieVideo, Profile};
        let Profile::Mov { rate, frames, raster: (width, height), video, aac, pcm, quicktime_timecode, .. }
            = verified.profile
        else {
            return Err("expected verified standalone movie".into());
        };
        let codec = match video {
            MovieVideo::Avc => "h264",
            MovieVideo::HevcMain10 => "hevc",
            MovieVideo::ProResHq => "prores",
            MovieVideo::Imported => return Err("expected standalone picture cache".into()),
        };
        let mut audio = Vec::new();
        if aac || pcm.is_some() {
            let samples = linked::movie_audio_sample_extent(frames, rate)?;
            let time_base = Rational::new(1, 48000).unwrap();
            let codec = pcm.map_or_else(|| "aac".into(), |bits| format!("pcm_s{bits}le"));
            audio.push(physical::Stream {
                index: 1,
                codec,
                time_base,
                count: samples,
                duration: linked::extent(samples, time_base)?,
                size: None,
                rate: None,
                channels: Some(2),
                bits: pcm,
            });
        }
        let physical = Physical {
            container: "mov",
            video: Some(physical::Stream {
                index: 0,
                codec: codec.into(),
                time_base,
                count,
                duration: linked::extent(count, time_base)?,
                size: Some((width.into(), height.into())),
                rate: Some(rate),
                channels: None,
                bits: None,
            }),
            audio,
            timecode: quicktime_timecode.map(linked::non_drop_24),
            package: None,
            file_package: None,
            state: physical::file_state(&spec.path)?,
        };
        Self::bind_physical(project, item, saved_path, spec, physical, bin, graph)
    }

    pub(super) fn bind_native(
        project: &Project,
        source: ItemId,
        saved_path: &Path,
        spec: &mut crate::linked::MediaSpec,
        native: &crate::native_import::Movie,
        bin: usize,
        graph: &mut SourceGraph,
    ) -> Result<Self> {
        let (time_base, count) = native.picture_clock();
        let duration = crate::linked::extent(count, time_base)?;
        let (width, height) = native.raster();
        let video = physical::Stream {
            index: 0, codec: native.picture_codec().into(), time_base, count, duration,
            size: Some((width.into(), height.into())), rate: Some(native.rate()), channels: None, bits: None,
        };
        let mut audio = Vec::new();
        if let Some((codec, samples)) = native.audio_clock() {
            let time_base = Rational::new(1, 48000).unwrap();
            audio.push(physical::Stream {
                index: 1,
                codec: codec.into(),
                time_base,
                count: samples,
                duration: crate::linked::extent(samples, time_base)?,
                size: None,
                rate: None,
                channels: Some(2),
                bits: native.bits(),
            });
        }
        let physical = Physical {
            container: "mov", video: Some(video), audio, timecode: native.timecode.clone(),
            package: None, file_package: None, state: physical::file_state(&spec.path)?,
        };
        Self::bind_physical(project, source, saved_path, spec, physical, bin, graph)
    }

    fn bind_physical(
        project: &Project,
        source: ItemId,
        saved_path: &Path,
        spec: &mut crate::linked::MediaSpec,
        physical: Physical,
        bin: usize,
        graph: &mut SourceGraph,
    ) -> Result<Self> {
        let first_stream = graph.media.len();
        let item = capture_item(project, source, saved_path, &spec.path, &physical, bin, graph)?;
        for media in &graph.media[first_stream..] {
            let original = media.original.as_ref().ok_or("missing Premiere original stream")?;
            spec.streams.push(crate::StreamSelector {
                media_id: media.id.clone(),
                index: usize::try_from(original.stream_index)?,
                channel: original.channel_index.map(usize::try_from).transpose()?,
            });
        }
        Ok(Self { item, physical })
    }

    pub(super) fn channels(
        &self,
        project: &Project,
        clip: TrackItemId,
        kind: StreamKind,
    ) -> Result<Vec<i64>> {
        let native = project.track_item(clip);
        if kind == StreamKind::Video {
            self.item.bindings.video(project, native.clip)?;
            return Ok(vec![]);
        }
        let channels = self.item.bindings.audio(
            project,
            native.clip,
            native.subclip,
            self.item.primary,
            &self.item.saved_path,
            &self.physical,
        )?;
        if self.physical.container == "wav" && !self.mono_wav() && channels != crate::stereo::STREAMS {
            return Err("Premiere 25 fps standalone WAV requires its ordered stereo group".into());
        }
        Ok(channels)
    }

    pub(super) fn mono_wav(&self) -> bool {
        self.physical.container == "wav"
            && matches!(self.physical.audio.as_slice(), [audio] if audio.channels == Some(1))
    }
}

fn capture_item(
    project: &Project,
    item: ItemId,
    saved_path: &Path,
    path: &Path,
    physical: &Physical,
    bin: usize,
    graph: &mut SourceGraph,
) -> Result<Item> {
    let tree = &project.doc.tree;
    let panel = project.item(item);
    let uid = tree
        .get(panel.element, "ObjectUID")
        .ok_or("Premiere source item lacks ObjectUID")?;
    let id = format!("premiere:item:{uid}");
    let master = panel.master.ok_or("Premiere source has no saved master")?;
    let primary = project
        .media_element(item)?
        .ok_or("Premiere source has no primary Media")?;
    saved::check_media(project, primary, primary, saved_path)?;
    let bindings = saved::ItemBindings::read(project, master, primary, saved_path, physical)?;
    let clock = original_clock(tree, primary, physical, &id)?;
    let tape = metadata::capture(project, item, primary, &id, None, graph)?;
    if let (Some(material), Some(file)) = (&physical.package, &physical.file_package) {
        graph.sidecars.push(Sidecar {
            adapter: "premiere.op1a_identity".into(),
            object: id.clone(),
            data: Value::Obj(vec![
                ("material_package_umid".into(), Value::Str(material.clone())),
                ("file_package_umid".into(), Value::Str(file.clone())),
            ]),
        });
    }
    let resolved = path.to_str().ok_or("invalid Premiere original path")?;
    let locator = saved_path.to_str().ok_or("invalid saved Premiere path")?;
    let mut item_media = Vec::new();
    let mut audio_label = 0;
    for stream in physical.video.iter().chain(&physical.audio) {
        for channel in 0..stream.channels.unwrap_or(1) {
            let media_id = format!("{id}/media/{}/{}", stream.index, channel);
            let kind = stream.kind();
            let logical = (kind == StreamKind::Audio).then(|| {
                audio_label += 1;
                audio_label
            });
            if logical
                .is_some_and(|label| !bindings.groups.iter().any(|group| group.contains(&label)))
                || kind == StreamKind::Video && !bindings.has_picture()
            {
                continue;
            }
            let clock = (kind == StreamKind::Video).then(|| clock.clone()).flatten();
            graph.media.push(SourceMedia {
                id: media_id.clone(),
                kind,
                locations: vec![resolved.into()],
                codec: Some(stream.codec.clone()),
                frame_size: stream.size,
                rate: stream.rate,
                sample_rate: stream.channels.map(|_| 48000),
                bits: stream.bits,
                duration: stream.duration,
                start: clock.as_ref().and_then(SourceClock::exact_start),
                clock,
                stream: logical,
                online: Some(true),
                original: Some(OriginalStream {
                    saved_locators: vec![locator.into()],
                    container: physical.container.into(),
                    stream_index: stream.index,
                    channel_index: stream.channels.map(|_| channel),
                    channels: stream.channels,
                    time_base: stream.time_base,
                    start_pts: 0,
                    duration_ts: stream.count,
                    sample_representation: logical.map(|_| {
                        match stream.bits {
                            Some(16) => "pcm16",
                            Some(24) => "pcm24",
                            _ => "aac",
                        }
                        .into()
                    }),
                }),
            });
            item_media.push(SourceItemMedia {
                media: media_id,
                gain_db: 0.0,
                source_offset: Some(Rational::ZERO),
            });
        }
    }
    if !bindings.groups.is_empty() {
        graph.sidecars.push(Sidecar {
            adapter: "premiere.audio_groups".into(),
            object: id.clone(),
            data: Value::Arr(
                bindings
                    .groups
                    .iter()
                    .map(|group| Value::Arr(group.iter().copied().map(Value::Int).collect()))
                    .collect(),
            ),
        });
    }
    let rate = physical.video.as_ref().and_then(|v| v.rate);
    let source_markers = markers::capture_item(project, item, &id, rate, graph)?;
    graph.bins[bin].items.push(id.clone());
    graph.items.push(SourceItem {
        id: id.clone(),
        name: project.item_name(item),
        media: item_media,
        umid: physical.package.clone(),
        tape,
        rate,
        color_rgb16: None,
        stream_groups: bindings
            .groups
            .iter()
            .filter(|g| g.len() > 1)
            .cloned()
            .collect(),
        markers: source_markers,
    });
    Ok(Item {
        id,
        saved_path: saved_path.into(),
        path: path.into(),
        primary,
        bindings,
    })
}

fn original_clock(
    tree: &prproj::xml::Tree,
    media: NodeId,
    physical: &Physical,
    id: &str,
) -> Result<Option<SourceClock>> {
    let start = leaf(tree, media, "Start")?
        .map(str::parse::<i64>)
        .transpose()?
        .unwrap_or(0);
    let alternate = match leaf(tree, media, "UseAlternateStart")? {
        None | Some("false") => None,
        Some("true") => Some(
            leaf(tree, media, "AlternateStart")?
                .ok_or("missing saved source timecode")?
                .parse::<i64>()?,
        ),
        _ => return Err("invalid saved source timecode flag".into()),
    };
    let Some(timecode) = &physical.timecode else {
        if alternate.unwrap_or(start) != 0 || start != 0 {
            return Err(
                "nonzero saved Premiere source origin has no corroborating file clock".into(),
            );
        }
        return Ok(None);
    };
    let rate = physical
        .video
        .as_ref()
        .and_then(|v| v.rate)
        .ok_or("file timecode lacks a picture clock")?;
    let base = physical::display_base(rate)?;
    let (count, drop) = crate::media::parse_timecode(timecode, base)?;
    let native = source_clock(
        count,
        rate,
        base,
        drop,
        id,
        if physical.container == "mxf" {
            "MXF package timecode"
        } else {
            "QuickTime tmcd"
        },
        "Premiere linked original",
    );
    let expected = native
        .exact_start()
        .ok_or("physical source timecode cannot be represented")?;
    if ticks(alternate.unwrap_or(start))? != expected || start != 0 && ticks(start)? != expected {
        return Err("saved Premiere source timecode contradicts the original".into());
    }
    Ok(Some(native))
}


#[allow(clippy::too_many_arguments)]
fn placement(
    project: &Project,
    clip: TrackItemId,
    kind: StreamKind,
    index: i64,
    item: &Item,
    physical: &Physical,
    rate: Rational,
    frame_ticks: i64,
    canvas: Option<(i64, i64)>,
    occurrence: &str,
    component_budget: &mut components::Budget,
    sampled_frame_count: &mut usize,
    graph: &mut SourceGraph,
) -> Result<Placement> {
    let native = project.track_item(clip);
    if project.speed(clip)? != 1.0
        || project.is_speed_reversed(clip)?
        || native.components.iter().any(|&c| {
            project
                .component_match_name(c)
                .to_ascii_lowercase()
                .contains("remap")
        })
    {
        return Err("retiming is outside Premiere linked media capture".into());
    }
    let start = project.start(clip)?.ticks;
    let end = project.end(clip)?.ticks;
    let a = project.in_point(clip)?.ticks;
    let b = project.out_point(clip)?.ticks;
    let span = end
        .checked_sub(start)
        .ok_or("Premiere edit duration overflow")?;
    if start < 0 || a < 0 || span <= 0 || b.checked_sub(a) != Some(span) {
        return Err("Premiere source and record ranges disagree".into());
    }
    let mut source_in = ticks(a)?;
    let mut duration = ticks(span)?;
    let mut mono = Vec::new();
    let mut grouped_audio = Vec::new();
    let mut frame_map = None;
    let hold = super::frame_hold::FrameHold::read(project, clip)?;
    let remap = super::retime::parameter(project, clip)?;
    if remap.is_some() && kind != StreamKind::Video {
        return Err("Premiere Linear Time Remapping requires a video edit".into());
    }
    if hold.is_some() && kind != StreamKind::Video {
        return Err("Premiere Frame Hold requires a video edit".into());
    }
    if kind == StreamKind::Video {
        item.bindings.video(project, native.clip)?;
        let picture = physical
            .video
            .as_ref()
            .ok_or("Premiere video edit lacks an original picture stream")?;
        if [a, b, start, end].iter().any(|v| v % frame_ticks != 0)
            || remap.is_none() && ticks(b)? > picture.duration
        {
            return Err(
                "Premiere picture edit has unsupported timing or exceeds the original".into(),
            );
        }
        if let Some(param) = remap {
            let has_components = !native.components.is_empty() || !native.selection_components.is_empty();
            if !super::retime::plain_linear_picture(index, rate, canvas, has_components)
                || picture.rate != Some(rate) || picture.size != canvas
                || physical.container != "mov" || picture.codec != "prores"
                || !physical.audio.is_empty()
                || physical.timecode.as_deref().is_some_and(|code| code != "00:00:00:00")
                || hold.is_some() || !super::retime::frame_sampling(project, clip)?
            {
                return Err("Linear Time Remapping requires plain same-rate 1080p24 V1 with Frame Sampling".into());
            }
            let extent = crate::prepare::frames_on(picture.duration, rate)?;
            let (frames, receipts) = super::retime::capture(
                project, param, occurrence, (a, b), extent, component_budget, sampled_frame_count,
            )?;
            source_in = Rational::new(frames[0].into(), 24).ok_or("invalid Linear retime source In")?;
            frame_map = Some(frames);
            graph.sidecars.extend(receipts);
        } else if picture.rate != Some(rate) {
            let unsupported = "mixed-rate Premiere capture requires plain V1 Frame Sampling between 24 and 23.976 fps with picture-only ProRes HQ MOV";
            let cadence = picture.rate
                .and_then(|source| mixed_rate::Cadence::between(source, rate))
                .ok_or(unsupported)?;
            if index != 1 || picture.size != Some((1920, 1080))
                || physical.container != "mov" || picture.codec != "prores"
                || !physical.audio.is_empty()
                || physical.timecode.as_deref().is_some_and(|code| code != "00:00:00:00")
                || hold.is_some() || !native.components.is_empty() || !native.selection_components.is_empty()
                || project.time_interpolation_type(clip)? != prproj::model::enums::TimeInterpolationType::FrameSampling
            {
                return Err(unsupported.into());
            }
            frame_map = Some(cadence.capture(a / frame_ticks, span / frame_ticks, sampled_frame_count)?);
        } else if let Some(hold) = hold {
            if index != 1 || rate != Rational::integer(24) || picture.size != Some((1920, 1080))
                || physical.container != "mov" || picture.codec != "prores"
                || !physical.audio.is_empty()
                || physical.timecode.as_deref().is_some_and(|code| code != "00:00:00:00")
                || !native.components.is_empty() || !native.selection_components.is_empty()
            {
                return Err("unsupported Premiere Frame Hold clock or source".into());
            }
            let extent = crate::prepare::frames_on(picture.duration, rate)?;
            frame_map = Some(hold.capture(occurrence, span, frame_ticks, extent, sampled_frame_count, graph)?);
        }
    } else {
        let selectors = item.bindings.audio(
            project,
            native.clip,
            native.subclip,
            item.primary,
            &item.saved_path,
            physical,
        )?;
        let channels: Vec<_> = physical
            .audio
            .iter()
            .flat_map(|s| std::iter::repeat_n(s, s.channels.unwrap_or_default() as usize))
            .collect();
        for &label in &selectors {
            let audio = channels[(label - 1) as usize];
            if ticks(b)? > audio.duration {
                // Native Premiere can extend a linked A/V item to the end of the
                // final frame when PCM/AAC physically ends a fraction of a sample
                // earlier. Keep the saved edit end and the verified playable tail.
                if !physical
                    .video
                    .as_ref()
                    .is_some_and(|v| ticks(b).ok() == Some(v.duration))
                    || !saved::extent_within_sample(b, audio.count)
                {
                    return Err("Premiere audio trim exceeds its original playable extent".into());
                }
                let playable = audio
                    .duration
                    .checked_sub(source_in)
                    .filter(|r| r.num() > 0)
                    .ok_or("Premiere audio edit contains no playable samples")?;
                duration = duration.min(playable);
            }
        }
        if duration != ticks(span)? {
            graph.sidecars.push(Sidecar {
                adapter: "premiere.saved_audio_tail".into(),
                object: occurrence.into(),
                data: Value::Obj(vec![
                    ("saved_record_end_ticks".into(), Value::Int(end)),
                    ("saved_source_out_ticks".into(), Value::Int(b)),
                ]),
            });
            graph.losses.push(Loss {
                object: occurrence.into(), aspect: "premiere.subsample_tail".into(),
                reason: "Saved audio end extends less than one sample past the linked original at picture EOF; graph placement ends at the exact playable sample".into(),
            });
        }
        if selectors.len() == 1 {
            mono = selectors;
        } else {
            grouped_audio.push(GroupedAudio {
                track: index,
                streams: selectors,
                gain_db: None,
                pans: vec![],
            });
        }
    }
    Ok(Placement {
        id: occurrence.into(),
        item: item.id.clone(),
        record_start: ticks(start)?,
        source_in,
        duration,
        video_track: (kind == StreamKind::Video).then_some(index),
        audio_tracks: if mono.is_empty() { vec![] } else { vec![index] },
        audio_streams: mono,
        grouped_audio,
        gain_db: 0.0,
        volume_db: None,
        volume_keys: vec![],
        track_gains_db: vec![],
        track_pans: vec![],
        speed: None,
        frame_map,
        transform: None,
        effects: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_timecode_counts_drop_frame_numbers_exactly() {
        assert_eq!(crate::media::parse_timecode("00:00:59;20", 30).unwrap(), (1790, true));
        assert_eq!(crate::media::parse_timecode("00:01:00;02", 30).unwrap(), (1800, true));
        assert_eq!(crate::media::parse_timecode("01:00:00;00", 30).unwrap(), (107892, true));
        assert_eq!(crate::media::parse_timecode("00:01:00:00", 30).unwrap(), (1800, false));
        assert!(crate::media::parse_timecode("00:01:00;01", 30).is_err());
        assert!(crate::media::parse_timecode("00:00:00;00", 25).is_err());
    }

    #[test]
    fn saved_audio_rounding_cannot_cover_a_missing_whole_sample() {
        let frame_ticks = TICKS_PER_SECOND / 30_000 * 1001;
        assert!(saved::extent_within_sample(48 * frame_ticks, 76876));
        assert!(saved::extent_within_sample(48 * frame_ticks, 76877));
        assert!(!saved::extent_within_sample(48 * frame_ticks, 76875));
        let sample_ticks = TICKS_PER_SECOND / 48000;
        assert!(!saved::extent_within_sample(10 * sample_ticks, 9));
    }
}
