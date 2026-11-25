//! Typed, bounded Premiere destination for a two-camera, fixed-mono-audio Group.
//! These are placement instructions, not source facts or model/1 objects.
use std::collections::{HashMap, HashSet};

use super::model::Clip;
use super::model::{check_text, Fraction, ModelError, MAX_ITEMS};
use super::prepared::{OriginalClip, PreparedClip, PreparedProjectInput, SampleRepresentation};
use super::writer::{Result as WriteResult, WriteError};
use crate::build::time::Time;
use crate::ids::UuidSource;
use crate::model::{ItemData, ItemId, ItemType, Project, TrackItemId};

pub const RATE: Fraction = Fraction {
    num: 24000,
    den: 1001,
};
pub const FRAME: i64 = crate::build::time::TICKS_PER_SECOND / 24000 * 1001;
/// A group expands to a hidden sequence, mixer and per-camera object graphs.
pub const MAX_GROUPS: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedCamera {
    pub clip: String,
    /// Independent mono view of the same original when the bin master is stereo.
    pub mono_view: Option<String>,
    pub start: i64,
    pub source_in: i64,
    pub duration: i64,
    /// Leaf Avid channel labels in the order of the independent group lanes.
    pub audio_labels: Vec<i64>,
}

impl PreparedCamera {
    pub fn mono_clip(&self) -> &str {
        self.mono_view.as_deref().unwrap_or(&self.clip)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMulticam {
    pub id: String,
    pub name: String,
    pub bin: usize,
    pub duration: i64,
    /// Picture track order.
    pub cameras: [PreparedCamera; 2],
    /// Saved camera index for each independent mono output.
    pub audio_cameras: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMulticamUse {
    pub sequence: usize,
    pub event: usize,
    pub group: usize,
    pub source_in: i64,
    pub video: Option<usize>,
    /// Static Motion belongs on the outer video edit for this Avid Warp binding.
    pub warp_picture_input: bool,
    /// Source per audio track in the prepared event's order.
    pub audio: Vec<PreparedGroupAudio>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreparedGroupAudio {
    Group(usize),
    LinkedAlternate(usize),
}

fn fail(reason: &str) -> ModelError {
    ModelError(format!("unsupported_editable_multicam: {reason}"))
}

fn write_fail(reason: &str) -> WriteError {
    WriteError::Other(crate::Error::Value(format!("editable multicam: {reason}")))
}

/// A verified file-backed camera whose embedded audio exposes independent
/// mono sources. Group-local trims use the same file origin for both streams.
pub fn is_linked_original_camera(clip: &OriginalClip) -> bool {
    clip.container == "mov"
        && clip.is_same_rate_linked_pcm()
        && !clip.separate_mono_streams
        && clip.video.as_ref().zip(clip.audio.as_ref()).is_some_and(|(video, audio)| {
            video.rate == RATE
                && (video.width, video.height) == (1920, 1080)
                && video.codec == "apch"
                && audio.representation == SampleRepresentation::Pcm24
                && audio.source_start == 0
                && audio.duration == video.duration
        })
        && matches!(clip.groups.as_slice(), [left, right]
            if left.as_slice() == [1] && right.as_slice() == [2])
        && clip.channels.len() == 2
        && clip.channels.iter().all(|channel| matches!(channel.avid_track, 1 | 2)
            && channel.channel_index == channel.avid_track - 1)
}

#[derive(Clone, Copy)]
pub(super) enum CameraMedia<'a> {
    Managed(&'a Clip, &'a [ItemId]),
    Original(ItemId, &'a OriginalClip),
}

pub(super) fn original_mono(clip: &OriginalClip, label: i64) -> Option<(usize, f64)> {
    let group = clip.groups.iter().position(|group| group.as_slice() == [label])?;
    let channel = clip.channels.iter().find(|channel| channel.avid_track == label)?;
    Some((group, channel.gain_db))
}

/// Build the native sequence-backed source item. The inner pan is transport:
/// explicit hard pan avoids the native default mono-to-stereo center loss
/// before the bus selects L/R. The outer timeline's mix is independent.
pub(super) fn build(
    project: &mut Project,
    g: &PreparedMulticam,
    leaves: [(ItemId, CameraMedia<'_>); 2],
    target: ItemId,
    ids: &mut dyn UuidSource,
) -> WriteResult<ItemId> {
    let channels = g.audio_cameras.len();
    let outputs = &[0, 1, 3][..channels];
    let fragment = crate::build::multicam::build_multicam_mono_audio_with_outputs(
        &mut project.doc.tree,
        ids,
        &g.name,
        FRAME,
        1920,
        1080,
        110,
        g.duration,
        2,
        channels * 2,
        outputs.iter().copied(),
    )?;
    if channels == 3 {
        // Premiere needs the native eight-channel adaptive count to render
        // the third mono lane, which is exposed from physical output 4.
        let count = project.doc.tree.find1(fragment, "AudioTrackGroup/NumAdaptiveChannels")
            .ok_or_else(|| write_fail("missing multicam adaptive channel count"))?;
        project.doc.tree.set_text(count, Some("8"));
    }
    let nodes = project.splice(fragment, ids)?;
    let (Some(sequence_element), Some(item_element), Some(master)) = (
        nodes.get("Sequence"),
        nodes.get("ClipProjectItem"),
        nodes.get("MasterClip"),
    ) else {
        return Err(write_fail("missing source fragment objects"));
    };
    let seq = project.parse_sequence(sequence_element)?;
    let video = project.video_tracks(seq).to_vec();
    let audio = project.audio_tracks(seq).to_vec();
    super::combined::premiere_balances(project, sequence_element)?;
    if channels == 3 {
        let track = project.track(audio[2]).element;
        let panner = project.doc.tree.find1(track, "AudioTrack/Panner")
            .ok_or_else(|| write_fail("third active strip has no panner"))?;
        let panner = project.doc.resolve(panner)?;
        let matrix = project.doc.tree.find1(panner, "DirectPanProcessor/Matrix")
            .ok_or_else(|| write_fail("third active strip has no output matrix"))?;
        project.doc.tree.set_text(matrix, Some("[[0,[2]],[1,[3]]]"));
    }
    for (i, (camera, (leaf, media))) in g.cameras.iter().zip(leaves).enumerate() {
        let start = Time::new(camera.start);
        let mut placed = vec![project.add_clip(video[i], leaf, start, ids)?];
        let native_groups = match media {
            CameraMedia::Managed(clip, siblings) if siblings.is_empty() =>
                super::native_av::groups(clip)?,
            _ => Vec::new(),
        };
        for (j, &label) in camera.audio_labels.iter().enumerate() {
            let selected = i == g.audio_cameras[j];
            // The selected inputs occupy the first slots. Each alternate has
            // its own muted track after the active inputs.
            let track = audio[if selected { j } else { j + channels }];
            let (item, gain) = match media {
                CameraMedia::Original(item, clip) => {
                    let (group, gain) = original_mono(clip, label)
                        .ok_or_else(|| write_fail("original camera mono group missing"))?;
                    (super::native_av::place_audio(project, track, item, group, start, ids)?, gain)
                }
                CameraMedia::Managed(clip, siblings) => {
                    let file = clip.audio.iter().position(|a| a.avid_track == Some(label))
                        .ok_or_else(|| write_fail("camera audio label missing"))?;
                    let item = if siblings.is_empty() {
                        let group = native_groups.iter()
                            .position(|g| g.as_slice() == [file])
                            .ok_or_else(|| write_fail("camera has no qualified mono group"))?;
                        super::native_av::place_audio(project, track, leaf, group, start, ids)?
                    } else {
                        let sibling = *siblings.get(file)
                            .ok_or_else(|| write_fail("linked camera mono item missing"))?;
                        project.add_clip(track, sibling, start, ids)?
                    };
                    (item, clip.audio[file].gain_db)
                }
            };
            super::audio::set_clip_gain(project, item, gain)?;
            if selected {
                let pan = if j == 0 {
                    super::combined::Pan::Left
                } else {
                    super::combined::Pan::Right
                };
                super::combined::set_clip_pan(project, item, pan)?;
            } else {
                project.set_muted(track, true)?;
            }
            placed.push(item);
        }
        for &t in &placed {
            project.set_in_point(t, Time::new(camera.source_in))?;
            project.set_out_point(t, Time::new(camera.source_in + camera.duration))?;
            project.set_end(t, Time::new(camera.start + camera.duration))?;
            super::placement::normalize_placement(project, t, None)?;
        }
        project.link(seq, &placed)?;
    }
    // add_clip can temporarily grow the source when inserting a whole leaf.
    // The group is not exposed yet; restore the validated, exact edit extent.
    if project.sequence_end(seq)?.ticks != g.duration {
        return Err(write_fail(
            "source sequence extent differs from the prepared group",
        ));
    }
    attach_source(project, target, g.duration, sequence_element, item_element, master, true, true)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn attach_source(
    project: &mut Project, target: ItemId, duration: i64, sequence_element: crate::xml::NodeId,
    item_element: crate::xml::NodeId, master: crate::xml::NodeId, picture: bool, audio: bool,
) -> WriteResult<ItemId> {
    if !picture || !audio {
        let list = project.doc.tree.child_by_tag(master, "Clips")
            .ok_or_else(|| write_fail("source clip list missing"))?;
        let mut index = 0;
        for entry in project.doc.tree.findall(list, "Clip")? {
            let kind = project.doc.tree.tag(project.doc.resolve(entry)?);
            if (!picture && kind == "VideoClip") || (!audio && kind == "AudioClip") {
                project.doc.tree.remove(list, entry);
            } else {
                project.doc.tree.set(entry, "Index", index.to_string());
                index += 1;
            }
        }
    }
    let clips = project.resolve_all(master, "Clips/Clip")?;
    for &c in &clips {
        let reference = project
            .doc
            .tree
            .find1(c, "Clip/Source")
            .ok_or_else(|| write_fail("source reference missing"))?;
        let source = project.doc.resolve(reference)?;
        let original_duration = project
            .doc
            .tree
            .child_by_tag(source, "OriginalDuration")
            .ok_or_else(|| write_fail("source duration missing"))?;
        project
            .doc
            .tree
            .set_text(original_duration, Some(duration.to_string()));
    }
    let tree = &mut project.doc.tree;
    let item_uid = tree
        .get(item_element, "ObjectUID")
        .ok_or_else(|| write_fail("source item has no UID"))?
        .to_owned();
    let target_element = project.item(target).element;
    let tree = &mut project.doc.tree;
    let container = tree
        .child_by_tag(target_element, "ProjectItemContainer")
        .ok_or_else(|| write_fail("destination bin container missing"))?;
    let items = crate::model::item::child_items(tree, container);
    crate::model::item::add_item_ref(tree, items, &item_uid);
    let mut data = ItemData::new(item_element, ItemType::Clip);
    data.parent = Some(target);
    data.master = Some(master);
    data.clip_elements = clips;
    data.default_out_ticks = Some(duration);
    data.sequence_uid = tree.get(sequence_element, "ObjectUID").map(str::to_owned);
    let item = project.push_item(data);
    project.item_mut(target).children.push(item);
    Ok(item)
}

pub(super) fn select_picture(
    project: &mut Project,
    item: TrackItemId,
    angle: usize,
) -> WriteResult<()> {
    let data = project.track_item(item).clone();
    let tree = &mut project.doc.tree;
    let selected = tree
        .find1(data.clip, "Clip/SelectedTrackIndex")
        .ok_or_else(|| write_fail("placed group video has no angle selector"))?;
    tree.set_text(selected, Some(angle.to_string()));
    if tree
        .child_by_tag(data.element, "PixelAspectRatio")
        .is_none()
    {
        crate::xml::mutations::append_leaf(tree, data.element, "PixelAspectRatio", "1,1");
    }
    Ok(())
}

fn aligned(t: i64) -> bool {
    t >= 0 && t % FRAME == 0
}

pub(super) fn validate(
    input: &PreparedProjectInput,
    clips: &HashMap<&str, &PreparedClip>,
) -> Result<(), ModelError> {
    if input.multicams.is_empty() && input.multicam_uses.is_empty() {
        return Ok(());
    }
    if input.multicams.len() > MAX_GROUPS || input.multicam_uses.len() > MAX_ITEMS {
        return Err(fail("prepared group/use count exceeds limit"));
    }
    let mut ids = HashSet::new();
    for g in &input.multicams {
        check_text(&g.id, "prepared group id", 32768, false)?;
        check_text(&g.name, "prepared group name", 4096, false)?;
        if clips.contains_key(g.id.as_str()) || !ids.insert(&g.id) || g.bin >= input.bins.len() {
            return Err(fail("group identity or bin ownership is invalid"));
        }
        if !aligned(g.duration) || g.duration == 0 || g.cameras[0].clip == g.cameras[1].clip {
            return Err(fail(
                "group requires two distinct cameras and an exact positive span",
            ));
        }
        let channels = g.audio_cameras.len();
        let linked_originals = channels == 2 && input.options.bin_mode == "separate"
            && g.cameras.iter().all(|camera| matches!(clips.get(camera.mono_clip()),
                Some(PreparedClip::Original(clip)) if is_linked_original_camera(clip)));
        if !matches!(channels, 1..=3)
            || g.audio_cameras.iter().any(|&camera| camera >= g.cameras.len())
            || ((channels == 3 || (channels == 2 && g.audio_cameras != [0; 2]))
                && !linked_originals
                && (input.options.bin_mode != "separate" || g.cameras.iter().any(|camera| {
                    !input.direct_dnxhr_origins.contains_key(&camera.clip)
                })))
        {
            return Err(fail("default audio needs exact camera assignments on the qualified linked route"));
        }
        let mut latest = 0;
        let mut camera_paths = HashSet::new();
        for c in &g.cameras {
            let end = c
                .start
                .checked_add(c.duration)
                .ok_or_else(|| fail("camera endpoint overflow"))?;
            let out = c
                .source_in
                .checked_add(c.duration)
                .ok_or_else(|| fail("camera source endpoint overflow"))?;
            if !aligned(c.start)
                || !aligned(c.source_in)
                || !aligned(c.duration)
                || c.duration == 0
                || end > g.duration
            {
                return Err(fail("camera needs an aligned, exact positive span within the Group"));
            }
            let leaf = clips.get(c.clip.as_str()).copied();
            if linked_originals {
                let (Some(PreparedClip::Original(leaf)), Some(PreparedClip::Original(mono))) =
                    (leaf, clips.get(c.mono_clip()).copied()) else {
                    return Err(fail("camera picture and mono view must both be verified originals"));
                };
                if !camera_paths.insert(&leaf.path)
                    || leaf.path != mono.path
                    || leaf.metadata.material_umid != mono.metadata.material_umid
                    || leaf.container != mono.container
                    || leaf.video != mono.video || leaf.audio != mono.audio
                    || leaf.channels != mono.channels
                    || leaf.proxy_path != mono.proxy_path
                    || leaf.premiere_assisted != mono.premiere_assisted
                    || leaf.rec709_source_override != mono.rec709_source_override
                    || leaf.separate_mono_streams != mono.separate_mono_streams
                    || leaf.video.as_ref().is_none_or(|video| out > video.duration)
                    || c.audio_labels.len() != channels
                    || c.audio_labels[0] == c.audio_labels[1]
                    || c.audio_labels.iter().any(|&label| original_mono(mono, label).is_none())
                {
                    return Err(fail("linked original cameras need distinct MOV paths, playable extents and two independent mono selectors"));
                }
                latest = latest.max(end);
                continue;
            }
            let Some(PreparedClip::Legacy(leaf)) = leaf else {
                return Err(fail("group requires managed Avid leaf masters or two verified ProRes HQ/PCM24 MOV cameras"));
            };
            if c.mono_view.is_some() {
                return Err(fail("managed camera cannot use an original-file mono view"));
            }
            let Some(v) = &leaf.video else {
                return Err(fail("camera has no picture"));
            };
            if channels == 3 && (!camera_paths.insert(&v.path)
                || leaf.audio.iter().any(|audio| !camera_paths.insert(&audio.path)))
            {
                return Err(fail("three-mono Group cameras require distinct picture and sibling paths"));
            }
            if out > v.duration
                || v.rate != RATE
                || (v.width, v.height) != (1920, 1080)
                || !v.codec.eq_ignore_ascii_case("avdn")
                || (channels > 1 && leaf.audio.len() != channels)
                || c.audio_labels.len() != channels
                || !leaf.stereo_pairs.is_empty()
                || !leaf.multichannel_groups.is_empty()
                || c.audio_labels.iter().copied().collect::<HashSet<_>>().len() != channels
                || leaf.audio.iter().filter(|a| channels > 1
                    || a.avid_track.is_some_and(|t| c.audio_labels.contains(&t))).any(|a| {
                    a.sample_rate != 48000
                        || a.bits != 24
                        || a.duration < out
                        || a.avid_track.is_none_or(|t| !c.audio_labels.contains(&t))
                })
                || c.audio_labels.iter().any(|&t| {
                    leaf.audio
                        .iter()
                        .filter(|a| a.avid_track == Some(t))
                        .count()
                        != 1
                })
            {
                return Err(fail("camera needs aligned 1080p23.976 DNx picture and exact, independent PCM24/48k mono lanes for each Group output"));
            }
            latest = latest.max(end);
        }
        if latest != g.duration {
            return Err(fail(
                "trailing gap after both cameras requires separate qualification",
            ));
        }
    }
    let sequences_with_uses: HashSet<_> = input.multicam_uses.iter().map(|u| u.sequence).collect();
    let edit_markers: HashSet<_> = sequences_with_uses
        .iter()
        .filter_map(|&i| input.sequences.get(i).map(|s| (i, s)))
        .flat_map(|(i, s)| {
            s.markers
                .iter()
                .filter_map(move |m| (m.owner == "edit" && !m.inherited).then_some((i, m.event?)))
        })
        .collect();
    let mut bindings = HashSet::new();
    let picture_effects: HashSet<_> = input.picture_effects.iter()
        .map(|effect| (effect.sequence, effect.event)).collect();
    for u in &input.multicam_uses {
        let g = input
            .multicams
            .get(u.group)
            .ok_or_else(|| fail("unknown group in use"))?;
        let s = input
            .sequences
            .get(u.sequence)
            .ok_or_else(|| fail("unknown sequence in use"))?;
        let e = s
            .events
            .get(u.event)
            .ok_or_else(|| fail("unknown event in use"))?;
        if !bindings.insert((u.sequence, u.event))
            || s.rate != RATE
            || !aligned(u.source_in)
            || u.source_in
                .checked_add(e.duration)
                .is_none_or(|end| end > g.duration)
            || u.video.is_some() != e.video_track.is_some()
            || u.audio.len() != e.audio_tracks.len()
            || u.audio.len() > g.audio_cameras.len()
            || u.audio.iter().collect::<HashSet<_>>().len() != u.audio.len()
            || (u.audio.is_empty() && u.video.is_none())
            || !e.stereo.is_empty()
            || e.speed_ratio.is_some()
            || e.source_duration.is_some()
            || e.time_remap.is_some()
            || (e.transform.is_some() && !u.warp_picture_input)
            || (u.warp_picture_input && (u.video.is_none()
                || picture_effects.contains(&(u.sequence, u.event))))
            || e.matte.is_some()
            || e.color_correction.is_some()
            || edit_markers.contains(&(u.sequence, u.event as i64))
        {
            return Err(fail("group use requires a complete, exact ordinary-speed event without clip effects or edit markers"));
        }
        let matches = |c: &PreparedCamera| {
            u.source_in >= c.start
                && u.source_in - c.start <= c.duration - e.duration
                && c.source_in.checked_add(u.source_in - c.start) == Some(e.source_in)
        };
        if let Some(v) = u.video {
            if !g.cameras.get(v).is_some_and(|c| e.clip == c.clip && matches(c)) {
                return Err(fail("picture choice contradicts the bound leaf placement"));
            }
        }
        if !u.audio.is_empty() {
            if u.audio.len() != e.audio_channels.len()
                || u.audio
                    .iter()
                    .zip(&e.audio_channels)
                    .any(|(&audio, label)| {
                        let (channel, alternate) = match audio {
                            PreparedGroupAudio::Group(channel) => (channel, false),
                            PreparedGroupAudio::LinkedAlternate(channel) => (channel, true),
                        };
                        let Some(&default) = g.audio_cameras.get(channel) else { return true; };
                        let camera = if alternate { 1 - default } else { default };
                        let c = &g.cameras[camera];
                        e.clip != c.mono_clip() || !matches(c) || c.audio_labels.get(channel) != Some(label)
                            || (alternate && (input.options.bin_mode != "separate"
                                || (!input.direct_dnxhr_origins.contains_key(&c.clip)
                                    && !matches!(clips.get(c.mono_clip()),
                                        Some(PreparedClip::Original(clip)) if is_linked_original_camera(clip)))))
                    })
            {
                return Err(fail(
                    "audio choice contradicts the bound leaf placement",
                ));
            }
        }
    }
    Ok(())
}
