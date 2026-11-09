//! Typed, bounded Premiere destination for a two-camera, fixed-mono-audio Group.
//! These are placement instructions, not source facts or model/1 objects.
use std::collections::{HashMap, HashSet};

use super::model::Clip;
use super::model::{check_text, Fraction, ModelError, MAX_ITEMS};
use super::prepared::{PreparedClip, PreparedProjectInput};
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
    pub start: i64,
    pub source_in: i64,
    pub duration: i64,
    /// Leaf Avid channel labels in the order of the two independent group lanes.
    pub audio_labels: [i64; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMulticam {
    pub id: String,
    pub name: String,
    pub bin: usize,
    pub duration: i64,
    /// Picture track order. Camera zero supplies the fixed audio for this profile.
    pub cameras: [PreparedCamera; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMulticamUse {
    pub sequence: usize,
    pub event: usize,
    pub group: usize,
    pub source_in: i64,
    pub video: Option<usize>,
    /// Group output channel per audio track in the prepared event's order.
    pub audio: Vec<usize>,
}

fn fail(reason: &str) -> ModelError {
    ModelError(format!("unsupported_editable_multicam: {reason}"))
}

fn write_fail(reason: &str) -> WriteError {
    WriteError::Other(crate::Error::Value(format!("editable multicam: {reason}")))
}

/// Build the native sequence-backed source item. The inner pan is transport:
/// explicit hard pan avoids the native default mono-to-stereo center loss
/// before the bus selects L/R. The outer timeline's mix is independent.
pub(super) fn build(
    project: &mut Project,
    g: &PreparedMulticam,
    leaves: [(ItemId, &Clip); 2],
    target: ItemId,
    ids: &mut dyn UuidSource,
) -> WriteResult<ItemId> {
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
        4,
        2,
    )?;
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
    for (i, (camera, (leaf, clip))) in g.cameras.iter().zip(leaves).enumerate() {
        let start = Time::new(camera.start);
        let mut placed = vec![project.add_clip(video[i], leaf, start, ids)?];
        let groups = super::native_av::groups(clip)?;
        for (j, &label) in camera.audio_labels.iter().enumerate() {
            let file = clip
                .audio
                .iter()
                .position(|a| a.avid_track == Some(label))
                .ok_or_else(|| write_fail("camera audio label missing"))?;
            let group = groups
                .iter()
                .position(|g| g.as_slice() == [file])
                .ok_or_else(|| write_fail("camera has no qualified mono group"))?;
            let track = audio[2 * i + j];
            let item = super::native_av::place_audio(project, track, leaf, group, start, ids)?;
            super::audio::set_clip_gain(project, item, clip.audio[file].gain_db)?;
            if i == 0 {
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
    let clips = project.resolve_all(master, "Clips/Clip")?;
    for &c in &clips {
        let reference = project
            .doc
            .tree
            .find1(c, "Clip/Source")
            .ok_or_else(|| write_fail("source reference missing"))?;
        let source = project.doc.resolve(reference)?;
        let duration = project
            .doc
            .tree
            .child_by_tag(source, "OriginalDuration")
            .ok_or_else(|| write_fail("source duration missing"))?;
        project
            .doc
            .tree
            .set_text(duration, Some(g.duration.to_string()));
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
    data.default_out_ticks = Some(g.duration);
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
        let mut latest = 0;
        for c in &g.cameras {
            let Some(PreparedClip::Legacy(leaf)) = clips.get(c.clip.as_str()).copied() else {
                return Err(fail("group requires rewrapped native leaf masters"));
            };
            let Some(v) = &leaf.video else {
                return Err(fail("camera has no picture"));
            };
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
                || out > v.duration
                || v.rate != RATE
                || (v.width, v.height) != (1920, 1080)
                || !v.codec.eq_ignore_ascii_case("avdn")
                || leaf.audio.len() != 2
                || !leaf.stereo_pairs.is_empty()
                || !leaf.multichannel_groups.is_empty()
                || c.audio_labels[0] == c.audio_labels[1]
                || leaf.audio.iter().any(|a| {
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
                return Err(fail("camera needs aligned 1080p23.976 DNx picture and two exact, independent PCM24/48k mono lanes"));
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
                .filter_map(move |m| (m.owner == "edit").then_some((i, m.event?)))
        })
        .collect();
    let mut bindings = HashSet::new();
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
            || u.audio.len() > 2
            || matches!(u.audio.as_slice(), [a,b] if a == b)
            || (u.audio.is_empty() && u.video.is_none())
            || !e.stereo.is_empty()
            || e.speed_ratio.is_some()
            || e.source_duration.is_some()
            || e.time_remap.is_some()
            || e.transform.is_some()
            || e.matte.is_some()
            || e.color_correction.is_some()
            || edit_markers.contains(&(u.sequence, u.event as i64))
        {
            return Err(fail("group use requires a complete, exact ordinary-speed event without clip effects or edit markers"));
        }
        let matches = |c: &PreparedCamera| {
            e.clip == c.clip
                && u.source_in >= c.start
                && u.source_in - c.start <= c.duration - e.duration
                && c.source_in.checked_add(u.source_in - c.start) == Some(e.source_in)
        };
        if let Some(v) = u.video {
            if !g.cameras.get(v).is_some_and(matches) {
                return Err(fail("picture choice contradicts the bound leaf placement"));
            }
        }
        if !u.audio.is_empty() {
            let c = &g.cameras[0];
            if !matches(c)
                || u.audio.len() != e.audio_channels.len()
                || u.audio
                    .iter()
                    .zip(&e.audio_channels)
                    .any(|(&channel, label)| c.audio_labels.get(channel) != Some(label))
            {
                return Err(fail(
                    "fixed audio choice contradicts the bound leaf placement",
                ));
            }
        }
    }
    Ok(())
}
