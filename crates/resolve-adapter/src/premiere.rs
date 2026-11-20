//! Premiere source capture into the shared Resolve turnover path.
mod capture;
pub(crate) mod color_mode;
mod components;
pub use capture::read_premiere_media_graph;
mod controls;
mod fades;
mod frame_hold;
mod marker_provenance;
pub(crate) use marker_provenance::{
    ADAPTER as EDIT_MARKER_PROVENANCE, CROSSES_SPLIT, CROSSES_SPLIT_REASON,
};
mod markers;
pub(crate) use markers::{SEQUENCE_MARKER_DATA_ADAPTER, SOURCE_MARKER_DATA_ADAPTER};
mod metadata;
mod mixer;
mod nesting;
mod multicam;
pub(crate) use multicam::CAMERA as MULTICAM_CAMERA;
mod retime;
mod source_capture;
mod transitions;
pub(crate) use metadata::{LABEL_LOSS as CLIP_LABEL_LOSS, LOSS as CLIP_METADATA_LOSS,
    SYNC_LOCK_LOSS, TIMELINE_MARK_LOSS};

use self::source_capture::{
    owning_bin, source_clock, source_receipt, source_sequence, GraphCapture, ROOT_BIN_ID,
};
use crate::{
    linked::{self, MediaSpec, Profile, ProjectSpec, TimelineSpec},
    media::media_digest,
    mixed_rate,
    turnover::Turnover,
    Result, StreamSelector,
};
use graph::{
    source::{
        OriginalStream, SourceClock, SourceGraph, SourceItem,
        SourceItemMedia, SourceMedia, SourceTrack, SourceText, TextKind,
    },
    Loss, Placement, Rational, Sidecar, StreamKind, Value,
};
use prproj::{
    build::time::TICKS_PER_SECOND,
    model::{ItemId, Project, SequenceId, TrackId, TrackItemId},
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

fn select_sequence(project: &Project, selector: &str) -> Result<SequenceId> {
    let sequences = project.sequences();
    Ok(if let Some(id) = sequences
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
    })
}

fn capture_input_scaling(
    project: &Project, item: ItemId, clip: TrackItemId, edit: &str, graph: &mut SourceGraph,
) -> Result<()> {
    let node = project.track_item(clip).clip;
    let fit = match project.doc.tree.findtext(node, "ScaleToFramePolicy")?.as_deref() {
        Some("1") => true,
        Some("0") => false,
        None => project.scale_to_frame_size(item)?,
        _ => return Err(format!("{edit}: unqualified Premiere input sizing policy").into()),
    };
    let mode = if fit { crate::input_scaling::FIT } else { crate::input_scaling::CENTER_CROP };
    graph.sidecars.push(crate::input_scaling::receipt(crate::input_scaling::PREMIERE, edit, mode));
    Ok(())
}

#[derive(Debug, Default)]
pub struct MediaLinkReview {
    pub direct: usize,
    pub assist: Vec<String>,
    pub blockers: Vec<String>,
}

/// Inspect selected Premiere locators using the standalone media checks.
pub(crate) fn review_media_links(path: &Path, selector: &str, ffprobe: &Path) -> Result<MediaLinkReview> {
    let path = premiere_path(path)?;
    let (project, _) = read_project(&path)?;
    let sequence = select_sequence(&project, selector)?;
    if let Some(item) = multicam::source(&project, sequence)? {
        return multicam::review_links(&project, sequence, item, ffprobe);
    }
    let plan = nesting::discover(&project, vec![sequence])?;
    let canvas = project.sequence(sequence).frame_size;
    let mut originals = std::collections::BTreeMap::<_, std::collections::BTreeSet<_>>::new();
    let mut seen = HashSet::new();
    let mut review = MediaLinkReview::default();
    for sequence in plan.sequences.iter().copied() {
        for clip in project.sequence_clips(sequence) {
            let Some(item) = project.track_item_source(clip) else {
                review.blockers.push("A Premiere edit has no saved source".into());
                continue;
            };
            if project.item(item).sequence_uid.is_some() || !seen.insert(item) {
                continue;
            }
            let name = project.item_name(item);
            let result = (|| -> Result<_> {
                let saved = project.item_media_path(item).map(Path::new)
                    .filter(|path| path.is_absolute())
                    .ok_or("original has no absolute saved locator")?;
                let media = project.media_element(item)?.ok_or("missing Premiere master media")?;
                metadata::check_locators(&project.doc.tree, media, saved)?;
                Ok(std::fs::canonicalize(saved)?)
            })();
            match result {
                Ok(path) => { originals.entry(path).or_default().insert(name); }
                Err(error) => review.blockers.push(format!("{name}: {error}")),
            }
        }
    }
    for (path, names) in originals {
        let label = format!("{} · {}", names.into_iter().collect::<Vec<_>>().join(", "), path.display());
        match linked::review_original(&path, ffprobe, canvas) {
            Ok(false) => review.direct += 1,
            Ok(true) => review.assist.push(label),
            Err(error) => review.blockers.push(format!("{label}: {error}")),
        }
    }
    Ok(review)
}

pub fn prepare_premiere_turnover(path: &Path, selector: &str, ffprobe: &Path) -> Result<Turnover> {
    prepare_selection(path, &[selector.to_owned()], false, ffprobe, &mut |_, _| {})
}

pub fn prepare_premiere_project(path: &Path, selectors: &[String], ffprobe: &Path) -> Result<Turnover> {
    prepare_selection(path, selectors, true, ffprobe, &mut |_, _| {})
}

pub(crate) fn prepare_with_media_progress(
    path: &Path, selector: &str, ffprobe: &Path, progress: &mut dyn FnMut(usize, &Path),
) -> Result<Turnover> {
    prepare_selection(path, &[selector.into()], false, ffprobe, progress)
}

fn prepare_selection(
    path: &Path, selectors: &[String], exact: bool, ffprobe: &Path,
    progress: &mut dyn FnMut(usize, &Path),
) -> Result<Turnover> {
    if !(1..=64).contains(&selectors.len()) {
        return Err("Premiere project requires between 1 and 64 sequence IDs".into());
    }
    let path = premiere_path(path)?;
    let (project, digest) = read_project(&path)?;
    let sequence_ids = project.sequences();
    let mut selection = Vec::with_capacity(selectors.len());
    let mut seen = HashSet::new();
    for selector in selectors {
        let sequence = if exact {
            let mut matches = sequence_ids.iter().copied()
                .filter(|&s| project.sequence_id(s).as_deref() == Some(selector));
            let first = matches.next()
                .ok_or_else(|| format!("Premiere sequence ObjectUID is absent: {selector}"))?;
            if matches.next().is_some() {
                return Err(format!("Ambiguous Premiere sequence ObjectUID: {selector}").into());
            }
            first
        } else {
            select_sequence(&project, selector)?
        };
        if !seen.insert(sequence) {
            return Err(format!("Duplicate Premiere sequence selection: {selector}").into());
        }
        selection.push(sequence);
    }
    let first = project.sequence(selection[0]);
    if selection.iter().any(|&id| {
        let row = project.sequence(id);
        row.frame_size != first.frame_size || row.timebase != first.timebase
    }) {
        return Err("Premiere project selection requires a common timeline rate and canvas".into());
    }
    if selection.len() > 1
        && (first.frame_size != Some((1920, 1080)) || first.timebase != Some(TICKS_PER_SECOND / 24))
    {
        return Err("Multiple Premiere timelines require the 1080p24 profile".into());
    }
    let root = project.root_item().ok_or("Premiere project root is absent")?;
    let first_panel = project.sequence_item(selection[0]).ok_or("Premiere sequence panel owner is missing")?;
    let mut capture = SelectionCapture {
        graph: GraphCapture::new(&project, &path, root, first_panel)?,
        items: HashMap::new(),
        files: HashMap::new(),
        spec: ProjectSpec {
            root_bin_id: Some(ROOT_BIN_ID.into()),
            timelines: vec![],
            media: vec![],
            atom_media: vec![],
            proxy_audio: Default::default(),
        },
        notices: vec![],
        caption_omissions: vec![],
        sampled_frame_count: 0,
    };
    let capture_components = components::is_current_project(&project)?;
    let mut nests = nesting::discover(&project, selection)?;
    for sequence in nests.sequences.clone() {
        let result = capture_sequence(&project, sequence, ffprobe, capture_components, &mut capture, &mut nests, progress);
        if exact {
            result.map_err(|e| format!("Premiere sequence {}: {e}", project.sequence_id(sequence).unwrap_or_default()))?;
        } else {
            result?;
        }
    }
    let provenance = marker_provenance::read(&project, &capture.graph.graph)?;
    if !provenance.is_empty() {
        capture.recover_marker_parents(&project, ffprobe, &provenance)?;
        let removed = marker_provenance::restore(&mut capture.graph.graph, provenance)?;
        capture.spec.media.retain(|binding| !removed.contains(&binding.item_id));
    }
    Turnover::prepare(
        capture.graph.graph, capture.spec, ffprobe, vec![(path, digest)],
        capture.notices, capture.caption_omissions, linked::MediaPreparation::VerifiedPremiere(&capture.files),
    )
}

struct SelectionCapture {
    graph: GraphCapture,
    items: HashMap<ItemId, (usize, Option<capture::VerifiedBinding>)>,
    files: linked::FileProbes,
    spec: ProjectSpec,
    notices: Vec<String>,
    caption_omissions: Vec<Loss>,
    sampled_frame_count: usize,
}

impl SelectionCapture {
    fn recover_marker_parents(
        &mut self,
        project: &Project,
        ffprobe: &Path,
        records: &[marker_provenance::Record],
    ) -> Result<()> {
        let mut seen = HashSet::new();
        for record in records {
            let item = record.original;
            if self.items.contains_key(&item) || !seen.insert(item)
                || marker_provenance::is_dedicated_item(project, item)?
                || project.clip_element_tags(item) != ["VideoClip"]
            {
                continue;
            }
            let (Some(media), Some(saved)) =
                (project.media_element(item)?, project.item_media_path(item).map(Path::new))
            else { continue };
            if !saved.is_absolute() || metadata::check_locators(&project.doc.tree, media, saved).is_err() {
                continue;
            }

            // Keep an unplaced original out of the selection until the normal
            // media checks and at least one copy agree with its saved state.
            let mut candidate = GraphCapture::empty_graph(self.graph.graph.project.name.clone());
            let Ok((mut binding, _)) = bind(
                project, item, media, saved, StreamKind::Video, record.rate, (1920, 1080),
                ffprobe, 0, &mut candidate, &mut self.files, false,
            ) else { continue };
            if !candidate.losses.is_empty() {
                continue;
            }
            let mut used = false;
            for sibling in records.iter().filter(|sibling| sibling.original == item) {
                if marker_provenance::matches_original(&self.graph.graph, &candidate, sibling)? {
                    used = true;
                    break;
                }
            }
            if !used {
                continue;
            }
            let graph = &mut self.graph.graph;
            let bin = owning_bin(project, item, &mut self.graph.folder_ids, &mut graph.bins)?;
            binding.bin_id.clone_from(&graph.bins[bin].id);
            graph.bins[bin].items.push(binding.item_id.clone());
            graph.items.extend(candidate.items);
            graph.media.extend(candidate.media);
            graph.sidecars.extend(candidate.sidecars);
            self.spec.media.push(binding);
        }
        Ok(())
    }
}

fn capture_sequence(
    project: &Project,
    sequence: SequenceId,
    ffprobe: &Path,
    capture_components: bool,
    capture: &mut SelectionCapture,
    nests: &mut nesting::Plan,
    progress: &mut dyn FnMut(usize, &Path),
) -> Result<()> {
    let id = project
        .sequence_id(sequence)
        .ok_or("Premiere sequence lacks ObjectUID")?;
    let data = project.sequence(sequence);
    let bus_fader =
        mixer::read_mix_bus(project, data.element, mixer::MixBus::Stereo, capture_components)?;
    let frame_ticks = data
        .timebase
        .filter(|&n| n > 0)
        .ok_or("missing Premiere sequence frame timebase")?;
    let rate = Rational::new(TICKS_PER_SECOND.into(), frame_ticks.into())
        .ok_or("invalid sequence rate")?;
    let canvas = match data.frame_size {
        Some((1920, 1080)) if linked::avc_rate(rate) || rate == crate::prepare::rate() => (1920, 1080),
        Some((3840, 2160)) if linked::uhd_rate(rate) => (3840, 2160),
        _ => return Err("Premiere turnover requires a qualified same-rate HD or UHD timeline".into()),
    };
    let base = linked::display_base(rate);
    if base == 25 && (!capture_components || project.video_display_format(sequence)? != Some(101)) {
        return Err("Premiere 25 fps turnover requires the saved 25-frame display clock".into());
    }
    let drop = linked::is_2997(rate);
    if drop && (!capture_components || project.video_display_format(sequence)? != Some(102)) {
        return Err("Premiere 29.97 turnover requires the saved drop-frame display clock".into());
    }
    let zero = project
        .sequence_bag_int(sequence, "MZ.ZeroPoint")?
        .ok_or("explicit Premiere sequence start required")?;
    let origin = u64::try_from(zero / frame_ticks)?;
    let frames_per_day = u64::from(base) * 86_400 - if drop { 2592 } else { 0 };
    if zero < 0 || zero % frame_ticks != 0 || origin >= frames_per_day {
        return Err("Premiere sequence origin is off-frame or outside one day".into());
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
    let sequence_panel_item = project
        .sequence_item(sequence)
        .ok_or("Premiere sequence panel owner is missing")?;
    let sequence_bin = owning_bin(project, sequence_panel_item, &mut capture.graph.folder_ids,
        &mut capture.graph.graph.bins)?;
    let mut out = source_sequence(
        id.clone(),
        project.sequence_name(sequence),
        data.frame_size,
        clock,
    );
    if let Some(group) = multicam::source(project, sequence)? {
        if !capture_components || canvas != (1920, 1080) || rate != crate::prepare::rate()
            || bus_fader != 0
        {
            return Err("Premiere multicam requires a current 1080p23.976 project with a neutral Mix bus".into());
        }
        let group = multicam::capture(project, sequence, group, ffprobe, capture, &mut out)?;
        let graph = &mut capture.graph.graph;
        let bin_id = graph.bins[sequence_bin].id.clone();
        graph.sidecars.push(source_receipt(id.clone(), &bin_id, frame_ticks, zero, []));
        graph.sequences.push(out);
        capture.spec.timelines.push(TimelineSpec {
            id, bin_id, canvas, group: Some(group),
        });
        return Ok(());
    }
    let SelectionCapture { graph: capture, items: seen_items, files, spec, notices: all_notices,
        caption_omissions: all_caption_omissions, sampled_frame_count } = capture;
    let GraphCapture { graph, folder_ids, .. } = capture;
    let mut component_budget = components::Budget::default();
    let mut color_payloads = color_mode::SavedPayloads::default();
    let mut track_state = metadata::TrackStateCapture::default();
    let mut muted_tracks = Vec::new();
    let mut track_pans = [0; 2];
    let mut stereo_edits = 0;
    let mut has_mono_wav = false;
    let active = |tracks: &[TrackId], index| tracks.iter().any(|&id| {
        let track = project.track(id);
        track.index == index && !track.clips.is_empty()
    });
    let wide = rate == Rational::integer(24)
        && active(&data.video_tracks, 1)
        && active(&data.audio_tracks, 2);
    spec.timelines.push(TimelineSpec {
        id: id.clone(),
        bin_id: graph.bins[sequence_bin].id.clone(),
        canvas,
        group: None,
    });
    for (kind, tracks, expected) in [
        (StreamKind::Video, &data.video_tracks, if wide { 2 } else { 1 }),
        (StreamKind::Audio, &data.audio_tracks, if wide { 3 } else { 2 }),
    ] {
        if kind == StreamKind::Audio && nests.omits_empty_audio(sequence) {
            continue;
        }
        let mut track_indexes = HashSet::new();
        for &track_id in tracks {
            let track = project.track(track_id);
            if !track.transitions.is_empty() && (!capture_components || track.clips.is_empty()) {
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
                return Err(if wide {
                    "expected active V1/V2 and independent A1/A2/A3 Premiere tracks"
                } else {
                    "expected active V1 and independent A1/A2 Premiere tracks"
                }.into());
            }
            if !capture_components && project.is_muted(track_id)? {
                return Err("muted Premiere track requires a verified Resolve mix mapping".into());
            }
            if capture_components {
                metadata::capture_track_state(&project.doc.tree, track.element, kind, index,
                    &id, &mut track_state)?;
            }
            let level_db = if kind == StreamKind::Audio {
                let fader = mixer::read_track(project, track.element, capture_components)
                    .map_err(|error| format!("{id}: Premiere A{index} mixer: {error}"))?;
                if let Some(slot) = track_pans.get_mut(index as usize - 1) {
                    *slot = fader.pan;
                } else if fader.pan != 0 || fader.tenths_db != 0 || fader.muted {
                    return Err(format!("{id}: Premiere A{index} requires a neutral Resolve mixer").into());
                }
                if fader.muted {
                    muted_tracks.push(Value::Int(index));
                }
                (fader.tenths_db != 0).then(|| f64::from(fader.tenths_db) / 10.0)
            } else {
                None
            };
            out.tracks.push(SourceTrack::new(kind, index, level_db));
            for &clip in &track.clips {
                if out.placements.len() == MAX_EDITS {
                    return Err("Premiere sequence exceeds edit limit".into());
                }
                if project.is_disabled(clip)? {
                    return Err(
                        "disabled Premiere edit requires a verified Resolve enable mapping".into(),
                    );
                }
                let tree = &project.doc.tree;
                let native = project.track_item(clip);
                let occurrence = tree
                    .get(native.element, "ObjectID")
                    .ok_or("Premiere edit lacks ObjectID")?;
                let placement_id = format!("premiere:edit:{occurrence}");
                let source = project
                    .track_item_source(clip)
                    .ok_or("unresolved Premiere source item")?;
                if project.item(source).sequence_uid.is_some() {
                    if !capture_components || bus_fader != 0 {
                        return Err("Premiere nesting requires a current project with a neutral Mix bus".into());
                    }
                    graph.losses.push(Loss {
                        object: id.clone(),
                        aspect: "placement".into(),
                        reason: format!("{placement_id}: {}", crate::nests::PREMIERE_LOSS),
                    });
                    continue;
                }
                let (item, verified) = match seen_items.entry(source) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let index = spec.media.len();
                        if index == 256 {
                            return Err("Premiere sequence exceeds linked media limit".into());
                        }
                        let bin = owning_bin(project, source, folder_ids, &mut graph.bins)?;
                        let media = project.media_element(source)?
                            .ok_or("Premiere source has no primary Media")?;
                        let saved = project.item_media_path(source)
                            .map(Path::new)
                            .filter(|path| path.is_absolute())
                            .ok_or("Premiere media requires an absolute saved path")?;
                        let previous_count = files.len();
                        let (binding, verified) = bind(
                            project, source, media, saved, kind, rate, canvas, ffprobe, bin, graph, files,
                            nests.stereo_child(sequence),
                        )?;
                        if files.len() != previous_count {
                            progress(files.len(), &binding.path);
                        }
                        spec.media.push(binding);
                        entry.insert((index, verified))
                    }
                };
                let binding = &spec.media[*item];
                if kind == StreamKind::Video && binding.streams.iter().filter(|stream| stream.channel.is_none())
                    .filter_map(|stream| graph.media.iter().find(|row| row.id == stream.media_id))
                    .any(|media| media.frame_size.is_some() && media.frame_size != data.frame_size)
                {
                    capture_input_scaling(project, source, clip, &placement_id, graph)?;
                }
                let channels = if let Some(verified) = verified {
                    verified.channels(project, clip, kind)
                } else {
                    clip_media(project, clip, source, kind)?;
                    (kind == StreamKind::Audio).then(|| channel(project, clip))
                        .transpose()
                        .and_then(|channel| channel.into_iter().map(|c| {
                            i64::try_from(c)?.checked_add(1).ok_or("Premiere channel index overflows".into())
                        }).collect::<Result<Vec<_>>>())
                }
                .map_err(|error| format!("{placement_id}: {error}"))?;
                let stereo = channels == crate::stereo::STREAMS
                    && index == 1 && verified.is_some()
                    && (nests.stereo_child(sequence)
                        || data.frame_size.is_some_and(|size| crate::stereo::clock(rate, size)));
                if kind == StreamKind::Audio && channels.len() != 1 && !stereo {
                    return Err(format!("{placement_id}: Premiere audio edit is not a qualified mono or stereo selection").into());
                }
                if channels.is_empty() && !binding.streams.iter().any(|s| s.channel.is_none())
                    || channels.iter().any(|&label| !binding.streams.iter().any(|s|
                        s.channel.is_some_and(|value| i64::try_from(value) == Ok(label - 1))))
                {
                    return Err(
                        "Premiere clip selects a nonexistent channel on its saved media".into(),
                    );
                }
                use crate::retime::PictureSpeed;
                let native_speed = project.speed(clip)?;
                let reversed = project.is_speed_reversed(clip)?;
                let remap = retime::parameter(project, clip)
                    .map_err(|error| format!("{placement_id}: {error}"))?;
                let picture_speed = match (native_speed, reversed) {
                    (1.0, false) => None,
                    (0.5, false) if capture_components && kind == StreamKind::Video && rate == Rational::integer(24) => {
                        Some(PictureSpeed::Half)
                    }
                    (2.0, false) if capture_components && kind == StreamKind::Video && rate == Rational::integer(24) => {
                        if project.time_interpolation_type(clip)? != prproj::model::enums::TimeInterpolationType::FrameSampling {
                            return Err(format!("{placement_id}: 200% Resolve picture speed requires Premiere Frame Sampling").into());
                        }
                        Some(PictureSpeed::Double)
                    }
                    (1.0, true) if capture_components && kind == StreamKind::Video && rate == Rational::integer(24) => {
                        Some(PictureSpeed::Reverse)
                    }
                    _ => return Err("Premiere clip speed or direction is outside the Resolve picture profile".into()),
                };
                if capture_components {
                    metadata::capture_edit_label(
                        project, native.clip, source, &placement_id,
                        metadata::ClipLabelCapture::IncludeDefaults, graph,
                    )
                        .map_err(|error| format!("Premiere label on {placement_id}: {error}"))?;
                }
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
                let has_components = !effects.is_empty() || !native.selection_components.is_empty();
                if has_components {
                    graph.sidecars.push(Sidecar {
                        adapter: "premiere.components".into(),
                        object: placement_id.clone(),
                        data: Value::Arr(effects),
                    });
                }
                let start = project.start(clip)?.ticks;
                let end = project.end(clip)?.ticks;
                let span = end
                    .checked_sub(start)
                    .ok_or("Premiere record span overflow")?;
                let speed = picture_speed
                    .map(|speed| speed.graph(span / frame_ticks).ok_or("invalid Premiere constant speed span"))
                    .transpose()?;
                let source_span = match picture_speed {
                    Some(PictureSpeed::Half) => span / 2,
                    Some(PictureSpeed::Double) => span.checked_mul(2).ok_or("Premiere speed span overflow")?,
                    _ => span,
                };
                let fractional_half = picture_speed == Some(PictureSpeed::Half)
                    && source_span % frame_ticks != 0;
                if fractional_half
                    && project.time_interpolation_type(clip)? != prproj::model::enums::TimeInterpolationType::FrameSampling
                {
                    return Err(format!("{placement_id}: odd-length half-speed picture requires Premiere Frame Sampling").into());
                }
                let (mut source_in, source_out) = if let Some(mode) = picture_speed {
                    let original = binding.streams.iter().find(|s| s.channel.is_none())
                        .and_then(|s| graph.media.iter().find(|m| m.id == s.media_id))
                        .filter(|m| m.kind == StreamKind::Video && m.codec.as_deref() == Some("prores"))
                        .ok_or("Premiere constant speed requires a verified ProRes HQ picture original")?;
                    let file_end = crate::prepare::frames_on(original.duration, rate)?
                        .checked_mul(frame_ticks).ok_or("invalid Premiere original duration")?;
                    let raw = |tag| -> Result<i64> {
                        Ok(tree.findtext(native.clip, tag)?.ok_or("missing raw Premiere speed trim")?.parse()?)
                    };
                    let a = raw("Clip/InPoint")?;
                    let b = raw("Clip/OutPoint")?;
                    if a < 0 || b > file_end {
                        return Err("Premiere speed trim exceeds the verified original".into());
                    }
                    if mode == PictureSpeed::Double {
                        if a.checked_add(source_span).is_none_or(|end| end > file_end) {
                            return Err(format!("{placement_id}: 200% Premiere speed span exceeds the verified original").into());
                        }
                        graph.sidecars.push(Sidecar {
                            adapter: "premiere.picture_speed".into(),
                            object: placement_id.clone(),
                            data: Value::Obj(vec![
                                ("in_ticks".into(), Value::Int(a)),
                                ("out_ticks".into(), Value::Int(b)),
                            ]),
                        });
                    }
                    match mode {
                        PictureSpeed::Half | PictureSpeed::Double => (a,b),
                        PictureSpeed::Reverse => (file_end - b,file_end - a),
                    }
                } else {
                    (project.in_point(clip)?.ticks, project.out_point(clip)?.ticks)
                };
                let saved_span = source_out.checked_sub(source_in);
                // Premiere can save Out just after the last sampled picture
                // at 200%, omitting the final skipped source frame.
                let shortened_double = picture_speed == Some(PictureSpeed::Double)
                    && saved_span == source_span.checked_sub(frame_ticks);
                if span <= 0
                    || start < 0
                    || source_in < 0
                    || (saved_span != Some(source_span) && !shortened_double)
                    || (!fractional_half && source_out % frame_ticks != 0)
                    || [start, end, source_in]
                        .into_iter()
                        .any(|v| v % frame_ticks != 0)
                {
                    return Err(
                        "Premiere record and source spans disagree with the saved speed or frame grid".into(),
                    );
                }
                let hold = frame_hold::FrameHold::read(project, clip)
                    .map_err(|err| format!("{placement_id}: {err}"))?;
                let cadence = binding.streams.iter().find(|stream| kind == StreamKind::Video && stream.channel.is_none())
                    .and_then(|stream| graph.media.iter().find(|m| m.id == stream.media_id))
                    .and_then(|original| original.rate)
                    .and_then(|source_rate| mixed_rate::Cadence::between(source_rate, rate));
                let frame_map = if let Some(param) = remap {
                    let unsupported = || format!("{placement_id}: Linear Time Remapping requires plain same-rate 1080p24 V1 with Frame Sampling");
                    if !capture_components || kind != StreamKind::Video
                        || !retime::plain_linear_picture(index, rate, Some((canvas.0.into(), canvas.1.into())), has_components)
                        || speed.is_some() || hold.is_some() || !track.transitions.is_empty()
                        || !retime::frame_sampling(project, clip)?
                    {
                        return Err(unsupported().into());
                    }
                    let original = binding.streams.iter().find(|stream| stream.channel.is_none())
                        .and_then(|stream| graph.media.iter().find(|media| media.id == stream.media_id))
                        .filter(|media| media.codec.as_deref() == Some("prores")
                            && media.kind == StreamKind::Video && media.rate == Some(rate)
                            && media.frame_size == Some((1920, 1080))
                            && media.start.is_none_or(|start| start == Rational::ZERO))
                        .filter(|_| binding.streams.len() == 1)
                        .ok_or_else(unsupported)?;
                    let extent = crate::prepare::frames_on(original.duration, rate)?;
                    let (frames, receipts) = retime::capture(
                        project, param, &placement_id, (source_in, source_out), extent,
                        &mut component_budget, sampled_frame_count,
                    ).map_err(|error| format!("{placement_id}: {error}"))?;
                    source_in = frames[0].checked_mul(frame_ticks).ok_or("Premiere remap source In overflows")?;
                    graph.sidecars.extend(receipts);
                    Some(frames)
                } else if let Some(cadence) = cadence {
                    if !capture_components || index != 1 || canvas != (1920, 1080) || has_components
                        || speed.is_some() || hold.is_some()
                        || project.time_interpolation_type(clip)? != prproj::model::enums::TimeInterpolationType::FrameSampling
                    {
                        return Err(format!("{placement_id}: mixed-rate Premiere capture requires plain V1 Frame Sampling").into());
                    }
                    Some(cadence.capture(source_in / frame_ticks, span / frame_ticks, sampled_frame_count)
                        .map_err(|err| format!("{placement_id}: {err}"))?)
                } else if let Some(hold) = hold {
                    let unsupported = || format!("{placement_id}: unsupported Premiere Frame Hold mode, clock, or source");
                    if !capture_components || kind != StreamKind::Video || index != 1
                        || rate != Rational::integer(24) || canvas != (1920, 1080)
                        || speed.is_some() || has_components
                    {
                        return Err(unsupported().into());
                    }
                    let original = binding.streams.iter().find(|stream| stream.channel.is_none())
                        .and_then(|stream| graph.media.iter().find(|m| m.id == stream.media_id))
                        .filter(|m| m.kind == StreamKind::Video && m.codec.as_deref() == Some("prores")
                            && m.rate == Some(rate) && m.frame_size == Some((1920, 1080))
                            && m.start.is_none_or(|start| start == Rational::ZERO))
                        .ok_or_else(unsupported)?;
                    let extent = crate::prepare::frames_on(original.duration, rate)?;
                    if binding.streams.len() != 1 || source_out / frame_ticks > extent {
                        return Err(unsupported().into());
                    }
                    Some(hold.capture(&placement_id, span, frame_ticks, extent, sampled_frame_count, graph)
                        .map_err(|err| format!("{placement_id}: {err}"))?)
                } else {
                    None
                };
                let mut appearance_loss = has_components.then_some(
                    "Premiere clip components are omitted from the Resolve turnover",
                );
                let mut transform = None;
                let mut volume_db = None;
                let mut picture_effects = Vec::new();
                if capture_components && kind == StreamKind::Video {
                    if let Some(saved) =
                        color_mode::capture(project, data.element, clip, source, &placement_id, &mut color_payloads)?
                    {
                        appearance_loss = color_mode::qualified_exposure(&saved.data).is_none().then_some(
                            color_mode::ADJUST_LOSS,
                        );
                        graph.sidecars.push(saved);
                    }
                }
                if capture_components {
                    if let Some(saved) = components::capture(
                        project,
                        clip,
                        kind,
                        &placement_id,
                        &mut component_budget,
                    )
                    .map_err(|error| format!("Premiere component parameters on {placement_id}: {error}"))?
                    {
                        if let Some(control) = controls::interpret(
                            project,
                            clip,
                            kind,
                            &saved.data,
                            source_in,
                            source_out,
                            frame_ticks,
                        )? {
                            appearance_loss = None;
                            match control {
                                controls::ClipControl::StaticMotion(motion) => {
                                    transform = Some(motion);
                                }
                                controls::ClipControl::MonoVolume(db) => volume_db = Some(db),
                                controls::ClipControl::MonoVolumeKeys(keys) => {
                                    graph.sidecars.push(controls::volume_key_receipt(&placement_id, keys));
                                    appearance_loss = Some("Premiere Linear clip volume is retained in graph/2; Resolve's linear-dB curve differs");
                                }
                                controls::ClipControl::LinearMotion(keys) => picture_effects
                                    .push(graph::picture_motion::linear_effect(keys)),
                                controls::ClipControl::LinearRoll(roll) => picture_effects
                                    .push(roll.readback()),
                            }
                        }
                        graph.sidecars.push(saved);
                    }
                }
                if let Some(reason) = appearance_loss {
                    graph.losses.push(Loss {
                        object: placement_id.clone(),
                        aspect: "premiere.appearance".into(),
                        reason: reason.into(),
                    });
                }
                stereo_edits += usize::from(stereo);
                has_mono_wav |= kind == StreamKind::Audio
                    && verified.as_ref().is_some_and(capture::VerifiedBinding::mono_wav);
                out.placements.push(Placement {
                    id: placement_id,
                    item: binding.item_id.clone(),
                    record_start: ticks(start)?,
                    source_in: ticks(source_in)?,
                    duration: ticks(span)?,
                    video_track: (kind == StreamKind::Video).then_some(index),
                    audio_tracks: if kind == StreamKind::Audio && !stereo { vec![index] } else { vec![] },
                    audio_streams: if stereo { vec![] } else { channels },
                    grouped_audio: if stereo { vec![crate::stereo::group()] } else { vec![] },
                    gain_db: 0.0,
                    volume_db,
                    volume_keys: vec![],
                    track_gains_db: vec![],
                    track_pans: vec![],
                    speed,
                    frame_map,
                    transform,
                    effects: picture_effects,
                });
            }
            let edge_fades = kind == StreamKind::Audio && capture_components
                && fades::capture(project, track, frame_ticks, &mut graph.sidecars)?;
            if !edge_fades && !track.transitions.is_empty() {
                transitions::capture(project, track, kind, &mut out, frame_ticks)?;
            }
        }
        let stereo_layout = kind == StreamKind::Audio && stereo_edits > 0
            && out.placements.iter().filter(|edit|
                edit.video_track.is_none() && edit.audio_tracks.as_slice() != [2]
            ).count() == stereo_edits;
        if kind == StreamKind::Audio && stereo_edits > 0 && !stereo_layout {
            return Err("Premiere A1 cannot mix mono and stereo edits".into());
        }
        if kind == StreamKind::Audio && has_mono_wav && !stereo_layout {
            return Err("Premiere 25 fps standalone mono WAV requires stereo A1 and mono A2".into());
        }
        if track_indexes.len() != expected as usize
            && !(kind == StreamKind::Audio && (track_indexes.is_empty() || nests.stereo_parent(sequence)))
            && !stereo_layout
        {
            return Err(if wide {
                "Premiere wider layout requires two picture tracks and three nonempty mono lanes"
            } else {
                "Premiere profile requires picture and two nonempty mono lanes"
            }.into());
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
                object: occurrence.clone(),
                aspect: crate::turnover::PREMIERE_CAPTION_LOSS.into(),
                reason: "Premiere caption is outside the editable Resolve subtitle profile".into(),
            });
            graph.losses.push(Loss {
                object: occurrence,
                aspect: "text_appearance".into(),
                reason: "Premiere caption appearance uses Resolve default subtitle styling".into(),
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
    markers::capture(&project.doc.tree, data.element, markers, &mut out, graph)?;
    if capture_components {
        metadata::capture_timeline_marks(project, data.element, &id, frame_ticks, graph)?;
    }
    graph.losses.extend(track_state.losses);
    for (adapter, rows) in [
        (crate::track_state::PREMIERE_ADAPTER, track_state.mapped),
        (SYNC_LOCK_LOSS, track_state.sync_unlocked),
    ] {
        if !rows.is_empty() {
            graph.sidecars.push(Sidecar {
                adapter: adapter.into(),
                object: id.clone(),
                data: Value::Arr(rows),
            });
        }
    }
    let mut source_fields = Vec::new();
    if let Some(length) = nests.length(sequence) {
        if !capture_components || bus_fader != 0 {
            return Err("Premiere nesting requires a current project with a neutral Mix bus".into());
        }
        source_fields.push((crate::nests::PREMIERE_LENGTH.into(), Value::Int(length)));
    }
    if let Some(receipt) = nests.source_facts(sequence, &id) {
        graph.sidecars.push(receipt);
    }
    if bus_fader != 0 {
        source_fields.push((
            crate::source::mixer::MAIN_BUS_LEVEL.into(),
            Value::Float(f64::from(bus_fader) / 10.0),
        ));
    }
    if !muted_tracks.is_empty() {
        source_fields.push((crate::source::mixer::MUTED_AUDIO_TRACKS.into(), Value::Arr(muted_tracks)));
    }
    if track_pans != [0; 2] {
        source_fields.push((
            crate::linked::mixer::PREMIERE_TRACK_PANS.into(),
            Value::Arr(track_pans.into_iter().map(|pan| Value::Int(i64::from(pan))).collect()),
        ));
    }
    graph.sidecars.push(source_receipt(
        id,
        &graph.bins[sequence_bin].id,
        frame_ticks,
        zero,
        source_fields,
    ));
    let mut notices: Vec<_> = out
        .transitions
        .iter()
        .filter(|transition| transition.kind == StreamKind::Video)
        .map(|transition| {
            format!(
                "{} (transition_approximation): Resolve Cross Dissolve preserves the cut and interval. Blend sampling can differ from Premiere Cross Dissolve (Legacy).",
                transition.id
            )
        })
        .collect();
    for (lane, pan) in (1..=2).zip(track_pans) {
        if pan != 0 && pan.abs() != 1000 {
            notices.push(format!(
                "{} (track_pan_law): A{lane} retains {}% track pan and uses Resolve's pan law",
                out.id, f64::from(pan) / 10.0
            ));
        }
    }
    graph.sequences.push(out);
    all_notices.extend(notices);
    all_caption_omissions.extend(caption_omissions);
    Ok(())
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

fn ticks(value: i64) -> Result<Rational> {
    Ok(
        Rational::new(value.into(), TICKS_PER_SECOND.into())
            .ok_or("invalid Premiere tick value")?,
    )
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
    if Some(media) != project.media_element(source)? {
        return Err("Premiere edit and master media disagree".into());
    }
    let locator = project
        .item_media_path(source)
        .ok_or("Premiere media locator is missing")?;
    let saved = PathBuf::from(locator);
    if !saved.is_absolute() {
        return Err("Premiere media requires an absolute saved path".into());
    }
    metadata::check_locators(tree, media, &saved)?;
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
    canvas: (u16, u16),
    ffprobe: &Path,
    bin: usize,
    graph: &mut SourceGraph,
    files: &mut linked::FileProbes,
    stereo_nest_child: bool,
) -> Result<(MediaSpec, Option<capture::VerifiedBinding>)> {
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
        sha256: String::new(),
        streams: vec![],
    };
    let (verified, tb, count) = match files.entry(path.clone()) {
        std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::hash_map::Entry::Vacant(entry) => {
            spec.sha256 = hex::encode(media_digest(&path)?);
            entry.insert(linked::probe(&spec, ffprobe, name.clone(), bin, canvas)?)
        }
    };
    spec.sha256 = verified.digest_hex()?;
    let (tb, count) = (*tb, *count);
    if let Some(native) = &verified.native {
        if native.rate() != rate || !components::is_current_project(project)? {
            return Err("Premiere native import requires a current same-rate project".into());
        }
        let binding = capture::VerifiedBinding::bind_native(project, item, saved, &mut spec, native, bin, graph)?;
        return Ok((spec, Some(binding)));
    }
    if let Profile::Mov { rate: file_rate, video: linked::MovieVideo::ProResHq,
        pcm: None, quicktime_timecode: Some(start), .. } = &verified.profile
    {
        if *start != 0 {
            if kind != StreamKind::Video || !crate::source::supported_marker_rate(*file_rate) || *file_rate != rate
                || !components::is_current_project(project)?
            {
                return Err("nonzero Premiere source timecode requires the qualified same-rate 24 or 23.976 fps ProRes HQ picture profile".into());
            }
            let binding = capture::VerifiedBinding::bind_standalone_movie(
                project, item, saved, &mut spec, verified, tb, count, bin, graph,
            )?;
            return Ok((spec, Some(binding)));
        }
    }
    metadata::check_source_origin(tree, media)?;
    if let Profile::Mov { rate: file_rate, video: linked::MovieVideo::Avc | linked::MovieVideo::HevcMain10, .. } = &verified.profile {
        if *file_rate != rate || !components::is_current_project(project)? {
            return Err("Premiere AVC or HEVC turnover requires a current same-rate project".into());
        }
        let binding = capture::VerifiedBinding::bind_standalone_movie(
                project, item, saved, &mut spec, verified, tb, count, bin, graph,
            )?;
        return Ok((spec, Some(binding)));
    }
    if kind == StreamKind::Audio
        && (rate == Rational::integer(25) && matches!(verified.profile, Profile::Wav { channels: 1 | 2, .. })
            || stereo_nest_child && matches!(verified.profile, Profile::Wav { channels: 2, .. }))
        && components::is_current_project(project)?
    {
        let binding = capture::VerifiedBinding::bind(project, item, saved, &mut spec, ffprobe, bin, graph)?;
        return Ok((spec, Some(binding)));
    }
    if rate == Rational::integer(25) || linked::is_2997(rate) {
        return Err("Premiere 25 and 29.97 fps turnover requires the qualified AVC MOV or MP4 media profile".into());
    }
    let duration = linked::extent(count, tb)?;
    let (container, codec, frame_size, sample_rate, bits, channels, stream_ref, original_rate) =
        match (&verified.profile, kind) {
            (
                Profile::Mov {
                    rate: file_rate,
                    video: linked::MovieVideo::ProResHq,
                    aac: false,
                    pcm,
                    ..
                },
                StreamKind::Video,
            ) if *file_rate == rate || (mixed_rate::Cadence::between(*file_rate, rate).is_some() && pcm.is_none()
                && components::is_current_project(project)?) => (
                "mov",
                "prores",
                Some((1920, 1080)),
                None,
                None,
                None,
                "VideoStream",
                *file_rate,
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
                rate,
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
            && Rational::new(TICKS_PER_SECOND.into(), saved_frame_ticks.into()) != Some(original_rate))
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
    let tape = metadata::capture(project, item, media, &item_id, Some(&spec.sha256), graph)?;
    let clock = match &verified.profile {
        Profile::Mov { quicktime_timecode: Some(start), .. } => Some(
            source_clock(
                u64::try_from(*start)?,
                original_rate,
                24,
                false,
                &item_id,
                "QuickTime tmcd",
                "verified original MOV timecode",
            )
        ),
        _ => None,
    };
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
            rate: (kind == StreamKind::Video).then_some(original_rate),
            sample_rate,
            bits,
            duration,
            start: clock.as_ref().and_then(SourceClock::exact_start),
            clock: clock.clone(),
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
    let item_rate = (kind == StreamKind::Video).then_some(original_rate);
    let source_markers = markers::capture_item(project, item, &item_id, item_rate, graph)?;
    graph.bins[bin].items.push(item_id.clone());
    graph.items.push(SourceItem {
        id: item_id,
        name,
        media: item_media,
        umid: None,
        tape,
        rate: item_rate,
        color_rgb16: None,
        stream_groups: if channels == Some(2) {
            vec![vec![1, 2]]
        } else {
            vec![]
        },
        markers: source_markers,
    });
    Ok((spec, None))
}
