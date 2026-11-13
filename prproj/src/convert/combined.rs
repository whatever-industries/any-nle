//! Combine an Avid clip's separate OP-Atom items into one Premiere build (port of
//! `converter/src/avid2prproj/combined.py`).
//!
//! - merged: py_premiere's `Project.create_merged_clip` generalised to one video item plus N
//!   mono audio items, the shape Premiere 26.5.2 wrote for an Avid clip (PRPROJ-0011): private
//!   copies of every component tagged with MZ.MergeClipUtils keys, a hidden 1-video/N-audio-track
//!   sequence flagged BE.Sequence.IsMergedClip, one Link over the placements and a
//!   sequence-backed panel item. Copies are built from model values (opatom), never by reading
//!   media.
//! - grouped: a multicam source clip over the video item and the mono audio items.
//! - stereo nest: an audio-only stereo build as a nested sequence.
//!
//! Python object identity (`id(a)`, `c is f`) becomes indices into `clip.audio` here:
//! [`combined_order`] gives the clip's audio files in combined (A01-first) order as indices.
//! The converter's ad-hoc item attributes live in [`Project::item_extra`].

use crate::build::media as mi;
use crate::build::merged as mb;
use crate::build::multicam::{build_multicam_mono_audio, build_processed_clips_bin};
use crate::build::time::Time;
use crate::convert::model::{audio_for_combined_clip, py_str_repr, ticks_per_unit, Clip};
use crate::convert::opatom::{add_audio_item, add_video_item, MediaTable};
use crate::convert::placement::normalize_placement;
use crate::convert::writer::{Result, WriteError};
use crate::error::Error;
use crate::ids::UuidSource;
use crate::model::item::{add_item_ref, child_items, detach_item_ref};
use crate::model::project::py_int;
use crate::model::{ItemData, ItemId, ItemType, Project, TrackItemId};
use crate::xml::mutations::{append_leaf, remove_child};
use crate::xml::NodeId;

/// `DISPLAY_FORMATS`: VideoTimeDisplayFormat per frame duration; 24p = 100 as Premiere wrote it
/// in the merged clip's hidden sequence (PRPROJ-0011) and as accepted in PRPROJ-0010, then the
/// non-drop entries of py_premiere's `TIMECODE_FORMATS`.
pub fn display_format(timebase: i64) -> Option<i64> {
    let mut found = (timebase == 10_584_000_000).then_some(100);
    for ((ticks, drop_frame), code) in mi::TIMECODE_FORMATS {
        if !drop_frame && ticks == timebase {
            found = Some(code);
        }
    }
    found
}

fn other(message: impl Into<String>) -> WriteError {
    WriteError::Other(Error::Value(message.into()))
}

/// The clip's audio files in combined (A01-first) order, as indices into `clip.audio`
/// (`audio_for_combined_clip`, which refuses when a track number is unknown).
pub fn combined_order(clip: &Clip) -> Result<Vec<usize>> {
    audio_for_combined_clip(clip)?;
    let mut order: Vec<usize> = (0..clip.audio.len()).collect();
    order.sort_by_key(|&i| clip.audio[i].avid_track.unwrap_or(0));
    Ok(order)
}

/// `_parts_bin(target_bin)`: the "Processed Clips" subfolder of `target_bin`, created if
/// missing. It holds the parts (source files) of the builds in that bin (User decision
/// 2026-10-14: per bin, Premiere's own name).
pub fn parts_bin(project: &mut Project, target_bin: ItemId, ids: &mut dyn UuidSource) -> Result<ItemId> {
    let existing = project
        .item(target_bin)
        .children
        .iter()
        .copied()
        .find(|&c| project.item(c).item_type == ItemType::Bin && project.item_name(c) == "Processed Clips");
    match existing {
        Some(bin) => Ok(bin),
        None => Ok(project.add_bin(target_bin, "Processed Clips", ids)?),
    }
}

/// `_move_to_processed_clips(project, items, target_bin)`: file source items into a
/// "Processed Clips" bin: the given bin's subfolder, or (no bin) the root one Premiere itself
/// uses for a multicam's angles.
pub fn move_to_processed_clips(
    project: &mut Project,
    items: &[ItemId],
    target_bin: Option<ItemId>,
    ids: &mut dyn UuidSource,
) -> Result<()> {
    let bin = match target_bin {
        None => project.root_bin("Processed Clips", |tree, _| Ok(build_processed_clips_bin(tree)), ids)?,
        Some(target) => parts_bin(project, target, ids)?,
    };
    move_items(project, items, bin)
}

/// `_move_items(project, items, destination)` (and the loop of `_move_to_processed_clips`).
fn move_items(project: &mut Project, items: &[ItemId], destination: ItemId) -> Result<()> {
    for &source in items {
        project.refile_item(source, destination)?;
    }
    Ok(())
}

/// `_tag_copy`: the MZ.MergeClipUtils bag on a component copy, renamed to the merged name.
fn tag_copy(project: &mut Project, copy: ItemId, merged_name: &str, bag: &[(&str, &str)]) -> Result<()> {
    let Some(master) = project.item(copy).master else { return Err(other("item has no master clip")) };
    let tree = &mut project.doc.tree;
    let node = mb::build_master_bag(tree, bag);
    tree.detach(node);
    tree.insert(master, 0, node);
    let Some(name) = tree.child_by_tag(master, "Name") else { return Err(other("'NoneType' object has no attribute 'text'")) };
    tree.set_text(name, Some(merged_name));
    let Some(logging_ref) = tree.child_by_tag(master, "LoggingInfo") else {
        return Err(other("'NoneType' object has no attribute 'get'"));
    };
    let logging = project.doc.resolve(logging_ref)?;
    if let Some(clip_name) = project.doc.tree.child_by_tag(logging, "ClipName") {
        project.doc.tree.set_text(clip_name, Some(merged_name));
    }
    if let Some(mapping) = project.doc.tree.child_by_tag(master, "DefMappingID") {
        remove_child(&mut project.doc.tree, master, mapping)?;
    }
    Ok(())
}

/// `create_merged`: `audio_items` must already be in combined order (see [`combined_order`]).
/// Returns `(merged_item, video_copy, audio_copies)`: in a sequence, Premiere places a merged
/// clip as its component copies, linked (PRPROJ-0011 placements).
#[allow(clippy::too_many_arguments)]
pub fn create_merged(
    project: &mut Project,
    media: &mut MediaTable,
    clip: &Clip,
    video_item: ItemId,
    audio_items: &[ItemId],
    target_bin: ItemId,
    merged_name: &str,
    ids: &mut dyn UuidSource,
) -> Result<(ItemId, ItemId, Vec<ItemId>)> {
    let video = match &clip.video {
        Some(v) if !audio_items.is_empty() => v,
        _ => return Err(other(format!("clip {}: a merged clip needs video and audio", py_str_repr(&clip.name)))),
    };
    let ordered = combined_order(clip)?;
    let timebase = ticks_per_unit(video.rate);
    let Some(display) = media.picture_format_override(&video.path).and_then(|s|s.parse().ok()).or_else(||display_format(timebase)) else {
        return Err(other(format!("no observed display format for {} fps merged clips", video.rate)));
    };
    let Some(root) = project.root_item() else { return Err(other("project has no root item")) };
    let previous = project.import_target.replace(root);
    let copies = (|| -> Result<(ItemId, Vec<ItemId>)> {
        let video_copy = add_video_item(project, media, video, clip, Some(merged_name), ids)?;
        let mut audio_copies = Vec::new();
        for &i in &ordered {
            audio_copies.push(add_audio_item(project, media, &clip.audio[i], clip, Some(merged_name), ids)?);
        }
        Ok((video_copy, audio_copies))
    })();
    project.import_target = previous;
    let (video_copy, audio_copies) = copies?;
    let video_name = project.item_name(video_item);
    tag_copy(project, video_copy, merged_name, &[("MZ.MergeClipUtils.ComponentMasterClipOriginalName", &video_name)])?;
    for (index, (&source, &copy)) in audio_items.iter().zip(&audio_copies).enumerate() {
        let source_name = project.item_name(source);
        let index = index.to_string();
        tag_copy(
            project,
            copy,
            merged_name,
            &[
                ("MZ.MergeClipUtils.AudioTrackNumberFromOriginalMergedClip", &index),
                ("MZ.MergeClipUtils.ComponentMasterClipOriginalName", &source_name),
            ],
        )?;
    }
    // Qualified direct parts already reference the same Media objects. Replacing
    // their state with hash references would erase the only copy of each payload.
    if !media.is_direct_dnxhr_picture(&video.path) {
        project.share_file_identity(video_item, video_copy)?;
        for (&source, &copy) in audio_items.iter().zip(&audio_copies) {
            project.share_file_identity(source, copy)?;
        }
    }

    let channels = audio_copies.len();
    let Some(&first_copy) = audio_copies.first() else { return Err(other("list index out of range")) };
    let Some(media_element) = project.media_element(first_copy)? else {
        return Err(other("'NoneType' object has no attribute 'find'"));
    };
    let Some(stream_ref) = project.doc.tree.child_by_tag(media_element, "AudioStream") else {
        return Err(other("'NoneType' object has no attribute 'get'"));
    };
    let stream = project.doc.resolve(stream_ref)?;
    let rate_text = project.doc.tree.findtext(stream, "FrameRate")?;
    let audio_rate = match rate_text {
        Some(t) => py_int(&t)?,
        None => return Err(other("int() argument must be a string, a bytes-like object or a real number, not 'NoneType'")),
    };
    let mut durations = vec![video.duration];
    durations.extend(ordered.iter().map(|&i| clip.audio[i].duration));
    let end_ticks = durations.iter().map(|d| d - d.rem_euclid(timebase)).max().expect("non-empty");
    let (width, height) = (video.width, video.height);

    let sequence_uid = ids.uuid4().to_string();
    let video_track_uid = ids.uuid4().to_string();
    let audio_track_uids: Vec<String> = (0..channels).map(|_| ids.uuid4().to_string()).collect();
    let track_uid_refs: Vec<&str> = audio_track_uids.iter().map(String::as_str).collect();
    let e = mb::build_empty_video_chain(&mut project.doc.tree);
    let group_chain_id = project.register(e)?;
    let mut track_chain_ids = Vec::new();
    let mut track_pan_ids = Vec::new();
    for _ in 0..channels {
        let fader_id = register_fader(project, audio_rate)?;
        let e = mb::build_meter(&mut project.doc.tree, audio_rate);
        let meter_id = project.register(e)?;
        let e = mb::build_fader_chain(&mut project.doc.tree, &fader_id, &meter_id);
        track_chain_ids.push(project.register(e)?);
        let e = mb::build_pan_processor(&mut project.doc.tree, audio_rate);
        track_pan_ids.push(project.register(e)?);
    }
    let mix_fader_id = register_fader(project, audio_rate)?;
    let e = mb::build_meter(&mut project.doc.tree, audio_rate);
    let mix_meter_id = project.register(e)?;
    let e = mb::build_fader_chain(&mut project.doc.tree, &mix_fader_id, &mix_meter_id);
    let mix_chain_id = project.register(e)?;
    let e = mb::build_pan_processor(&mut project.doc.tree, audio_rate);
    let mix_pan_id = project.register(e)?;
    let e = mb::build_inlet(&mut project.doc.tree, &track_uid_refs);
    let inlet_id = project.register(e)?;
    let e = mb::build_mix_track(&mut project.doc.tree, ids, &mix_chain_id, &mix_pan_id, &inlet_id);
    let mix_track_id = project.register(e)?;
    let e = mb::build_video_group(&mut project.doc.tree, &video_track_uid, timebase, width, height, &group_chain_id);
    let video_group_id = project.register(e)?;
    let e = mb::build_audio_group(&mut project.doc.tree, ids, &track_uid_refs, audio_rate, &mix_track_id);
    let audio_group_id = project.register(e)?;
    let e = mb::build_data_group(&mut project.doc.tree, timebase);
    let data_group_id = project.register(e)?;
    let e = mb::build_video_track(&mut project.doc.tree, &video_track_uid);
    project.doc.attach_object(e)?;
    for (index, track_uid) in audio_track_uids.iter().enumerate() {
        let e = mb::build_audio_track(
            &mut project.doc.tree,
            ids,
            track_uid,
            &track_chain_ids[index],
            &track_pan_ids[index],
            2 + index as i64,
            index as i64,
        );
        project.doc.attach_object(e)?;
    }
    let e = mb::build_merged_sequence(
        &mut project.doc.tree,
        &sequence_uid,
        merged_name,
        display,
        end_ticks,
        width,
        height,
        &video_group_id,
        &audio_group_id,
        &data_group_id,
    );
    let sequence_element = project.doc.attach_object(e)?;
    let sequence = project.parse_sequence(sequence_element)?;

    let video_track = project.video_tracks(sequence)[0];
    let video_placed = project.add_clip(video_track, video_copy, Time::new(0), ids)?;
    normalize_placement(project, video_placed, None)?;
    let mut placed_ids = vec![object_id(project, video_placed)];
    let audio_tracks = project.audio_tracks(sequence).to_vec();
    for (&track, &copy) in audio_tracks.iter().zip(&audio_copies) {
        let placed = project.add_clip(track, copy, Time::new(0), ids)?;
        project.narrow_merged_channel(placed, 0)?; // each A0x copy is already mono
        normalize_placement(project, placed, None)?;
        placed_ids.push(object_id(project, placed));
    }
    let refs: Vec<&str> = placed_ids.iter().map(String::as_str).collect();
    let e = mb::build_link(&mut project.doc.tree, &refs);
    let link_id = project.register(e)?;
    let Some(link) = project.doc.tree.find1(sequence_element, "PersistentGroupContainer/LinkContainer/Links/Link") else {
        return Err(other("'NoneType' object has no attribute 'set'"));
    };
    project.doc.tree.set(link, "ObjectRef", link_id);

    let e = mb::build_sequence_source(&mut project.doc.tree, "VideoSequenceSource", &sequence_uid, end_ticks);
    let video_source_id = project.register(e)?;
    let e = mb::build_sequence_source(&mut project.doc.tree, "AudioSequenceSource", &sequence_uid, end_ticks);
    let audio_source_id = project.register(e)?;
    let e = mi::new_secondary_content(&mut project.doc.tree, &audio_source_id, 0);
    let secondary_id = project.register(e)?;
    let mut panel_vector_ids = Vec::new();
    for i in 0..channels {
        let e = mi::new_channel_serializer(&mut project.doc.tree, i as i64, 0);
        let serializer_id = project.register(e)?;
        let e = mi::new_channel_vector(&mut project.doc.tree, &[&serializer_id], 0);
        panel_vector_ids.push(project.register(e)?);
    }
    let vector_refs: Vec<&str> = panel_vector_ids.iter().map(String::as_str).collect();
    let e = mi::new_audio_channel_groups(&mut project.doc.tree, &vector_refs);
    let groups_id = project.register(e)?;
    let mut master_chain_ids = Vec::new();
    for _ in 0..channels {
        let e = mi::new_master_audio_chain(&mut project.doc.tree, 1, 0);
        master_chain_ids.push(project.register(e)?);
    }
    let e = mb::build_panel_audio_template(&mut project.doc.tree, ids, &audio_source_id, &secondary_id);
    let audio_template_id = project.register(e)?;
    let e = mb::build_panel_video_template(&mut project.doc.tree, ids, &video_source_id);
    let video_template_id = project.register(e)?;
    let e = mb::build_panel_logging(&mut project.doc.tree, &video_name, display, end_ticks, timebase);
    let logging_id = project.register(e)?;
    let master_uid = ids.uuid4().to_string();
    let chain_refs: Vec<&str> = master_chain_ids.iter().map(String::as_str).collect();
    let e = mb::build_panel_master(
        &mut project.doc.tree,
        &master_uid,
        &logging_id,
        &chain_refs,
        &audio_template_id,
        &video_template_id,
        &groups_id,
        merged_name,
    );
    let master_element = project.doc.attach_object(e)?;
    let item_uid = ids.uuid4().to_string();
    let e = mb::build_panel_item(&mut project.doc.tree, &item_uid, &master_uid, merged_name);
    let item_element = project.doc.attach_object(e)?;

    let root_element = project.item(root).element;
    let Some(root_container) = project.doc.tree.child_by_tag(root_element, "ProjectItemContainer") else {
        return Err(other("root item has no ProjectItemContainer"));
    };
    for &copy in std::iter::once(&video_copy).chain(&audio_copies) {
        // copies live only inside the hidden sequence
        let element = project.item(copy).element;
        let uid = project.doc.tree.get(element, "ObjectUID").unwrap_or_default().to_string();
        detach_item_ref(&mut project.doc.tree, root_container, &uid)?;
        project.doc.remove_object(element)?;
        let children = &mut project.item_mut(root).children;
        let Some(at) = children.iter().position(|&c| c == copy) else { return Err(other("list.remove(x): x not in list")) };
        children.remove(at);
    }
    let target_element = project.item(target_bin).element;
    let Some(target_container) = project.doc.tree.child_by_tag(target_element, "ProjectItemContainer") else {
        return Err(other("target bin has no ProjectItemContainer"));
    };
    let items = child_items(&mut project.doc.tree, target_container);
    add_item_ref(&mut project.doc.tree, items, &item_uid);
    let mut parts = vec![video_item];
    parts.extend_from_slice(audio_items);
    move_to_processed_clips(project, &parts, Some(target_bin), ids)?; // parts, not beside the build
    let mut data = ItemData::new(item_element, ItemType::Clip);
    data.parent = Some(target_bin);
    data.master = Some(master_element);
    data.clip_elements = vec![object(project, &audio_template_id)?, object(project, &video_template_id)?];
    data.default_out_ticks = Some(end_ticks);
    data.sequence_uid = Some(sequence_uid);
    let item = project.push_item(data);
    project.item_mut(target_bin).children.push(item);
    Ok((item, video_copy, audio_copies))
}

/// `project._register(mb.build_fader(rate, [register(volume), register(mute)]))`.
fn register_fader(project: &mut Project, audio_rate: i64) -> Result<String> {
    let e = mb::build_volume_param(&mut project.doc.tree);
    let volume = project.register(e)?;
    let e = mb::build_mute_param(&mut project.doc.tree);
    let mute = project.register(e)?;
    let e = mb::build_fader(&mut project.doc.tree, audio_rate, &[&volume, &mute]);
    Ok(project.register(e)?)
}

fn object_id(project: &Project, placed: TrackItemId) -> String {
    project.doc.tree.get(project.track_item(placed).element, "ObjectID").unwrap_or_default().to_string()
}

fn object(project: &Project, object_id: &str) -> Result<NodeId> {
    project.doc.by_object_id.get(object_id).copied().ok_or_else(|| other(format!("KeyError: '{object_id}'")))
}

/// `create_grouped`: a multicam source clip over one video item and N mono audio items (A01
/// first), the shape Premiere 26.5.2 wrote for an Avid clip (PRPROJ-0011), except that Premiere
/// put A02 on the first track there. Sources move into the bin's "Processed Clips"; the grouped
/// item lands in `target_bin`. Nothing is copied.
pub fn create_grouped(
    project: &mut Project,
    clip: &Clip,
    video_item: Option<ItemId>,
    audio_items: &[ItemId],
    target_bin: ItemId,
    grouped_name: &str,
    ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    create_grouped_with_media(project,clip,video_item,audio_items,target_bin,grouped_name,None,ids)
}

pub(crate) fn create_grouped_with_media(
    project: &mut Project,
    clip: &Clip,
    video_item: Option<ItemId>,
    audio_items: &[ItemId],
    target_bin: ItemId,
    grouped_name: &str,
    media: Option<&MediaTable>,
    ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    if audio_items.is_empty() || (video_item.is_none() && audio_items.len() < 2) {
        return Err(other(format!(
            "clip {}: a grouped clip needs video and audio, or a stereo pair",
            py_str_repr(&clip.name)
        )));
    }
    let rate = match &clip.video {
        Some(v) => Some(v.rate),
        None => clip.rate,
    };
    let Some(rate) = rate else {
        return Err(other(format!("clip {}: no frame rate for an audio-only build", py_str_repr(&clip.name))));
    };
    let timebase = ticks_per_unit(rate);
    let display_override = clip.video.as_ref()
        .and_then(|video| media?.picture_format_override(&video.path))
        .and_then(|format| format.parse().ok());
    let discrete_direct_audio = clip.video.as_ref()
        .is_some_and(|video| media.is_some_and(|media| media.is_direct_dnxhr_picture(&video.path)));
    let Some(display) = display_override.or_else(||display_format(timebase)) else {
        return Err(other(format!("no observed display format for {rate} fps grouped clips")));
    };
    let mut durations = Vec::new();
    if video_item.is_some() {
        let Some(video) = &clip.video else { return Err(other("'NoneType' object has no attribute 'duration'")) };
        durations.push(video.duration);
    }
    durations.extend(clip.audio.iter().map(|a| a.duration));
    let Some(end_ticks) = durations.iter().map(|d| d - d.rem_euclid(timebase)).max() else {
        return Err(other("max() iterable argument is empty"));
    };
    let (width, height) = clip.video.as_ref().map(|v| (v.width, v.height)).unwrap_or((1920, 1080));
    // Audio-only (a stereo pair with no picture): the same shape with no video track (UNVERIFIED).
    let fragment = build_multicam_mono_audio(
        &mut project.doc.tree,
        ids,
        grouped_name,
        timebase,
        width,
        height,
        display,
        end_ticks,
        usize::from(video_item.is_some()),
        audio_items.len(),
    )?;
    let spliced = project.splice(fragment, ids)?;
    let (Some(sequence_element), Some(item_element), Some(master_element)) =
        (spliced.get("Sequence"), spliced.get("ClipProjectItem"), spliced.get("MasterClip"))
    else {
        return Err(other("KeyError: multicam template is missing its core objects"));
    };
    let sequence = project.parse_sequence(sequence_element)?;
    if let Some(video_item) = video_item {
        let track = project.video_tracks(sequence)[0];
        let placed = project.add_clip(track, video_item, Time::new(0), ids)?;
        normalize_placement(project, placed, None)?;
    }
    let audio_tracks = project.audio_tracks(sequence).to_vec();
    for (index, (&track, &audio)) in audio_tracks.iter().zip(audio_items).enumerate() {
        let placed = project.add_clip(track, audio, Time::new(0), ids)?;
        normalize_placement(project, placed, None)?;
        let element = project.track_item(placed).element;
        let Some(chain_ref) = project.doc.tree.find1(element, "ClipTrackItem/ComponentOwner/Components") else {
            return Err(other("'NoneType' object has no attribute 'get'"));
        };
        let chain = project.doc.resolve(chain_ref)?;
        let tree = &mut project.doc.tree;
        if let Some(inner) = tree.child_by_tag(chain, "ComponentChain") {
            if let Some(node) = tree.child_by_tag(inner, "Node") {
                // bag-less mono DefaultVol chain, as Premiere writes inside a multicam
                tree.remove(inner, node);
                tree.set_text(inner, Some("\n\t\t"));
            }
        }
        if discrete_direct_audio {
            // Send each source to its own bus channel. The outer edit still owns its pan.
            set_clip_pan(project, placed, if index == 0 { Pan::Left } else { Pan::Right })?;
        }
    }

    premiere_balances(project, sequence_element)?;
    let groups = channel_groups(clip)?;
    if groups.iter().any(|g| g.len() > 1) {
        stereo_master_groups(project, master_element, &groups)?;
    }

    let parts: Vec<ItemId> = video_item.into_iter().chain(audio_items.iter().copied()).collect();
    move_to_processed_clips(project, &parts, Some(target_bin), ids)?;
    let item_uid = project.doc.tree.get(item_element, "ObjectUID").unwrap_or_default().to_string();
    let target_element = project.item(target_bin).element;
    let Some(target_container) = project.doc.tree.child_by_tag(target_element, "ProjectItemContainer") else {
        return Err(other("'NoneType' object has no attribute 'find'"));
    };
    let items = child_items(&mut project.doc.tree, target_container);
    add_item_ref(&mut project.doc.tree, items, &item_uid);
    let clip_refs = project.doc.tree.findall(master_element, "Clips/Clip")?;
    let clip_elements = clip_refs.into_iter().map(|r| project.doc.resolve(r)).collect::<std::result::Result<Vec<_>, _>>()?;
    let mut data = ItemData::new(item_element, ItemType::Clip);
    data.parent = Some(target_bin);
    data.master = Some(master_element);
    data.clip_elements = clip_elements;
    data.default_out_ticks = Some(end_ticks);
    data.sequence_uid = project.doc.tree.get(sequence_element, "ObjectUID").map(str::to_string);
    let item = project.push_item(data);
    project.item_extra.entry(item).or_default().channel_groups = Some(groups);
    project.item_mut(target_bin).children.push(item);
    Ok(item)
}

/// `_channel_groups(clip)`: the grouped clip's channel groups, as inner-track (bus channel)
/// indices in combined (A01-first) order: an Avid stereo pair becomes one 2-channel group at
/// its left track's position, every other track a 1-channel group.
pub fn channel_groups(clip: &Clip) -> Result<Vec<Vec<usize>>> {
    let order = combined_order(clip)?;
    let track = |i: usize| clip.audio[order[i]].avid_track;
    let index = |t: i64| (0..order.len()).rev().find(|&i| track(i) == Some(t));
    let right_of = |t: i64| clip.stereo_pairs.iter().rev().find(|(_, r)| *r == t).map(|(l, _)| *l);
    let left_of = |t: i64| clip.stereo_pairs.iter().rev().find(|(l, _)| *l == t).map(|(_, r)| *r);
    let mut groups = Vec::new();
    for i in 0..order.len() {
        let t = track(i);
        if t.and_then(right_of).is_some() {
            continue;
        }
        match t.and_then(left_of) {
            Some(right) => match index(right) {
                Some(j) => groups.push(vec![i, j]),
                None => return Err(other(format!("KeyError: {right}"))),
            },
            None => groups.push(vec![i]),
        }
    }
    Ok(groups)
}

/// `group_for_channel(groups, channel)`.
pub fn group_for_channel(groups: &[Vec<usize>], channel: usize) -> Result<usize> {
    groups.iter().position(|g| g.contains(&channel)).ok_or_else(|| other("StopIteration"))
}

pub const STEREO_LAYOUT: &str = r#"[{"channellabel":100},{"channellabel":101}]"#;

/// `_stereo_master_groups`: rewrite a mono-per-track multicam master so each group is one
/// channel vector: a stereo group gets ChannelType 1 with both bus channels and a stereo chain,
/// the shape of py_premiere's (Premiere-verified) single-track stereo multicam master.
/// UNVERIFIED for several tracks; awaiting User read-back.
fn stereo_master_groups(project: &mut Project, master: NodeId, groups: &[Vec<usize>]) -> Result<()> {
    let none = || other("'NoneType' object has no attribute 'findall'");
    let chains = project.doc.tree.child_by_tag(master, "AudioComponentChains").ok_or_else(none)?;
    let groups_ref = project.doc.tree.child_by_tag(master, "AudioClipChannelGroups").ok_or_else(none)?;
    let vectors_object = project.doc.resolve(groups_ref)?;
    let vectors = project.doc.tree.child_by_tag(vectors_object, "ClipChannelVectors").ok_or_else(none)?;
    let old_vectors = project
        .doc
        .tree
        .findall(vectors, "ClipChannelVectorItem")?
        .into_iter()
        .map(|v| project.doc.resolve(v))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let old_chains = project
        .doc
        .tree
        .findall(chains, "AudioComponentChain")?
        .into_iter()
        .map(|c| project.doc.resolve(c))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let tree = &mut project.doc.tree;
    for parent in [vectors, chains] {
        for child in tree.children(parent).to_vec() {
            tree.remove(parent, child);
        }
    }
    let out_of_range = || other("list index out of range");
    for (g, channels) in groups.iter().enumerate() {
        let vector = *old_vectors.get(channels[0]).ok_or_else(out_of_range)?;
        let chain = *old_chains.get(channels[0]).ok_or_else(out_of_range)?;
        if channels.len() > 1 {
            let serials = tree.child_by_tag(vector, "ClipChannels").ok_or_else(none)?;
            for &extra in &channels[1..] {
                let donor_vector = *old_vectors.get(extra).ok_or_else(out_of_range)?;
                let donor = tree.find1(donor_vector, "ClipChannels/ClipChannelItem").ok_or_else(none)?;
                let donor_ref = tree.get(donor, "ObjectRef").map(str::to_string);
                let index = tree.len(serials).to_string();
                let mut attrs = vec![("Index", index.as_str())];
                if let Some(r) = donor_ref.as_deref() {
                    attrs.push(("ObjectRef", r));
                }
                let reference = tree.sub_element(serials, "ClipChannelItem", &attrs);
                let first = tree.children(serials)[0];
                let tail = tree.tail(first).map(str::to_string);
                tree.set_tail(reference, tail);
            }
            let channel_type = tree.child_by_tag(vector, "ChannelType").ok_or_else(none)?;
            tree.set_text(channel_type, Some("1"));
            for (tag, text) in [("AudioChannelLayout", STEREO_LAYOUT), ("ChannelType", "1")] {
                let leaf = match tree.child_by_tag(chain, tag) {
                    Some(leaf) => leaf,
                    None => {
                        let leaf = tree.sub_element(chain, tag, &[]);
                        let n = tree.len(chain);
                        let tail = if n > 1 { tree.tail(tree.children(chain)[n - 2]).map(str::to_string) } else { None };
                        tree.set_tail(leaf, tail);
                        leaf
                    }
                };
                tree.set_text(leaf, Some(text));
            }
        }
        let index = g.to_string();
        let vector_id = tree.get(vector, "ObjectID").map(str::to_string);
        let chain_id = tree.get(chain, "ObjectID").map(str::to_string);
        sub_with_ref(tree, vectors, "ClipChannelVectorItem", &index, vector_id.as_deref());
        sub_with_ref(tree, chains, "AudioComponentChain", &index, chain_id.as_deref());
    }
    let kept = |tree: &crate::xml::Tree, parent: NodeId, object: NodeId| {
        let id = tree.get(object, "ObjectID");
        tree.children(parent).iter().any(|&x| tree.get(x, "ObjectRef") == id)
    };
    for v in old_vectors {
        if !kept(&project.doc.tree, vectors, v) {
            project.doc.remove_object(v)?;
        }
    }
    for c in old_chains {
        if !kept(&project.doc.tree, chains, c) {
            project.doc.remove_object(c)?;
        }
    }
    Ok(())
}

/// `ET.SubElement(parent, tag, {"Index": index, "ObjectRef": object_ref})` (Python drops no
/// attribute; a missing ObjectID would make ElementTree refuse to serialize).
fn sub_with_ref(tree: &mut crate::xml::Tree, parent: NodeId, tag: &str, index: &str, object_ref: Option<&str>) {
    let mut attrs = vec![("Index", index)];
    if let Some(r) = object_ref {
        attrs.push(("ObjectRef", r));
    }
    tree.sub_element(parent, tag, &attrs);
}

/// `narrow_to_group(project, placed, groups, group)`: make a placed multicam audio item play
/// one channel group. A 1-channel group uses py_premiere's verified mono narrowing (channel =
/// bus channel); a stereo group keeps both channels' SecondaryContents under a stereo layout and
/// selects the group through OrigChGrp/SecondaryIndex, extending the same shape (UNVERIFIED).
pub fn narrow_to_group(project: &mut Project, placed: TrackItemId, groups: &[Vec<usize>], group: usize) -> Result<()> {
    let channels = groups.get(group).ok_or_else(|| other("list index out of range"))?.clone();
    if channels.len() == 1 && groups.iter().all(|g| g.len() == 1) {
        project.narrow_merged_channel(placed, channels[0] as i64)?;
        return Ok(());
    }
    let data = project.track_item(placed).clone();
    let clip = data.clip;
    if let Some(layout) = project.doc.tree.child_by_tag(clip, "AudioChannelLayout") {
        let text = if channels.len() == 2 { STEREO_LAYOUT } else { r#"[{"channellabel":0}]"# };
        project.doc.tree.set_text(layout, Some(text));
    }
    if let Some(contents) = project.doc.tree.child_by_tag(clip, "SecondaryContents") {
        let mut kept = Vec::new();
        let entries: Vec<NodeId> = project
            .doc
            .tree
            .children(contents)
            .iter()
            .copied()
            .filter(|&c| project.doc.tree.tag(c) == "SecondaryContentItem")
            .collect();
        for entry in entries {
            let secondary = project.doc.resolve(entry)?;
            let index_text = match project.doc.tree.findtext(secondary, "ChannelIndex")? {
                Some(t) if !t.is_empty() => t,
                _ => "0".to_string(),
            };
            let index = py_int(&index_text)?;
            if index >= 0 && channels.contains(&(index as usize)) && kept.len() < channels.len() {
                kept.push(entry);
            } else {
                project.doc.remove_object(secondary)?;
                remove_child(&mut project.doc.tree, contents, entry)?;
            }
        }
        for (i, &entry) in kept.iter().enumerate() {
            project.doc.tree.set(entry, "Index", i.to_string());
        }
    }
    if let Some(chain_ref) = project.doc.tree.find1(data.element, "ClipTrackItem/ComponentOwner/Components") {
        let chain = project.doc.resolve(chain_ref)?;
        if channels.len() == 1 {
            let tree = &mut project.doc.tree;
            if let Some(inner) = tree.child_by_tag(chain, "ComponentChain") {
                if let Some(node) = tree.child_by_tag(inner, "Node") {
                    tree.remove(inner, node);
                    tree.set_text(inner, Some("\n\t\t"));
                }
            }
        } else {
            declare_stereo_chain(project, placed)?;
        }
    }
    if group != 0 {
        append_leaf(&mut project.doc.tree, clip, "SecondaryIndex", &channels[0].to_string());
        if let Some(orig) = project.doc.tree.child_by_tag(data.subclip, "OrigChGrp") {
            project.doc.tree.set_text(orig, Some(group.to_string()));
        }
    }
    Ok(())
}

const BALANCE_CLASS_ID: &str = "a714635e-a628-4b27-9d59-77eba47dbc1a";

/// `_premiere_balances`: inner multicam track balances as Premiere 26.5.2 writes them for its
/// own multicams, mono and stereo alike (PRPROJ-0019): an AudioComponentParam (ClassID a714635e)
/// named Balance with IsInverted true; track 0 stores StartKeyframe/CurrentValue 0, the other
/// tracks store no value.
pub(super) fn premiere_balances(project: &mut Project, sequence_element: NodeId) -> Result<()> {
    for group_ref in project.doc.tree.findall(sequence_element, "TrackGroups/TrackGroup/Second")? {
        let group = project.doc.resolve(group_ref)?;
        if project.doc.tree.tag(group) != "AudioTrackGroup" {
            continue;
        }
        for (position, track_ref) in project.doc.tree.findall(group, "TrackGroup/Tracks/Track")?.into_iter().enumerate() {
            let track = project.doc.resolve(track_ref)?;
            let Some(panner_ref) = project.doc.tree.find1(track, "AudioTrack/Panner") else {
                return Err(other("'NoneType' object has no attribute 'get'"));
            };
            let panner = project.doc.resolve(panner_ref)?;
            let param_refs: Vec<NodeId> = project.doc.tree.iter_tag(panner, "Param").collect();
            for param_ref in param_refs {
                let param = project.doc.resolve(param_ref)?;
                let tree = &mut project.doc.tree;
                if tree.findtext(param, "Name")?.as_deref() != Some("Balance") {
                    continue;
                }
                tree[param].tag = "AudioComponentParam".to_string();
                tree.set(param, "ClassID", BALANCE_CLASS_ID);
                tree.set(param, "Version", "10");
                let indent = tree.text(param).map(str::to_string);
                for child in tree.children(param).to_vec() {
                    tree.remove(param, child);
                }
                let mut leaves = Vec::new();
                if position == 0 {
                    leaves.extend([("StartKeyframe", "-91445760000000000,0.,0,0,0,0,0,0"), ("CurrentValue", "0")]);
                }
                leaves.extend([("Name", "Balance"), ("IsInverted", "true")]);
                let count = leaves.len();
                for (i, (tag, text)) in leaves.into_iter().enumerate() {
                    let leaf = tree.sub_element(param, tag, &[]);
                    tree.set_text(leaf, Some(text));
                    let tail = if i < count - 1 {
                        indent.clone()
                    } else {
                        let mut s = indent.clone().unwrap_or_default();
                        s.pop();
                        Some(s)
                    };
                    tree.set_tail(leaf, tail);
                }
            }
        }
    }
    Ok(())
}

const PANNER_CLASS_ID: &str = "33a94283-ee2c-11d5-abc4-c1cd7f9e3c10";
const PAN_PARAM_CLASS_ID: &str = "a714635e-a628-4b27-9d59-77eba47dbc1a";

/// The `full` argument of `set_clip_pan`: `"left"`, `"right"` or a percent -100..100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pan {
    Left,
    Right,
    Percent(f64),
}

/// `set_clip_pan(project, placed, full)`: pan a placed mono audio edit. An AudioClipTrackItem
/// `<Panner>` to a MonoToStereoPanProcessor whose AudioComponentParam "Pan" (IsInverted) stores
/// (pan + 100) / 200 (User-confirmed 0, 0.25, 0.5, 0.75, 1.0 -> -100, -50, 0, +50, +100;
/// PRPROJ-0021/0023). Premiere itself omits the value for +100; an explicit 1.0 reads the same.
pub fn set_clip_pan(project: &mut Project, placed: TrackItemId, full: Pan) -> Result<()> {
    match full {
        Pan::Percent(p) => {
            let v = (p + 100.0) / 200.0;
            if !(0.0..=1.0).contains(&v) {
                return Err(other(format!("pan {}: expected -100..100", crate::build::fmt::py_repr(p))));
            }
            panner(project, placed, Some(&crate::build::fmt::py_repr(v)))
        }
        Pan::Left => panner(project, placed, Some("0")),
        Pan::Right => panner(project, placed, None),
    }
}

fn panner(project: &mut Project, placed: TrackItemId, value: Option<&str>) -> Result<()> {
    let tree = &mut project.doc.tree;
    let panner = tree.new_element_with("MonoToStereoPanProcessor", &[("ClassID", PANNER_CLASS_ID), ("Version", "1")]);
    let pan = tree.sub_element(panner, "PanProcessor", &[("Version", "3")]);
    let audio = tree.sub_element(pan, "AudioComponent", &[("Version", "3")]);
    let component = tree.sub_element(audio, "Component", &[("Version", "7")]);
    let params = tree.sub_element(component, "Params", &[("Version", "1")]);
    let param = tree.new_element_with("AudioComponentParam", &[("ClassID", PAN_PARAM_CLASS_ID), ("Version", "10")]);
    let mut leaves: Vec<(&str, String)> = Vec::new();
    if let Some(value) = value {
        let start = if value == "0" { "0." } else { value };
        leaves.push(("StartKeyframe", format!("-91445760000000000,{start},0,0,0,0,0,0")));
        leaves.push(("CurrentValue", value.to_string()));
    }
    leaves.extend([("IsTimeVarying", "false".into()), ("Name", "Pan".into()), ("IsInverted", "true".into())]);
    for (tag, text) in leaves {
        let leaf = tree.sub_element(param, tag, &[]);
        tree.set_text(leaf, Some(text));
    }
    let param_id = project.register(param)?;
    let tree = &mut project.doc.tree;
    tree.sub_element(params, "Param", &[("Index", "0"), ("ObjectRef", &param_id)]);
    let id = tree.sub_element(component, "ID", &[]);
    tree.set_text(id, Some("4294967040"));
    for (tag, text) in [
        ("AudioChannelLayout", r#"[{"channellabel":0}]"#),
        ("AudioComponentType", "0"),
        ("FrameRate", "5292000"),
        ("ChannelType", "0"),
    ] {
        let leaf = tree.sub_element(audio, tag, &[]);
        tree.set_text(leaf, Some(text));
    }
    let layout = tree.sub_element(pan, "OutputAudioChannelLayout", &[]);
    tree.set_text(layout, Some(STEREO_LAYOUT));
    let panner_id = project.register(panner)?;
    let element = project.track_item(placed).element;
    let tree = &mut project.doc.tree;
    let reference = tree.sub_element(element, "Panner", &[("ObjectRef", &panner_id)]);
    let children = tree.children(element).to_vec();
    let previous = children[children.len() - 2];
    let previous_tail = tree.tail(previous).map(str::to_string);
    let first_tail = tree.tail(children[0]).map(str::to_string);
    tree.set_tail(reference, previous_tail);
    tree.set_tail(previous, first_tail);
    Ok(())
}

/// `create_stereo_nest`: an audio-only stereo build as a nested sequence: the left part on
/// audio track 1 panned fully left, the right part on track 2 panned fully right. Replaces the
/// zero-video multicam, whose playback left that media silent in Premiere 26.5.2 for the rest
/// of the session (User, 2026-10-14). The parts go to the bin's Processed Clips.
pub fn create_stereo_nest(
    project: &mut Project,
    audio_items: &[ItemId],
    target_bin: ItemId,
    name: &str,
    preset: &str,
    ids: &mut dyn UuidSource,
) -> Result<ItemId> {
    if audio_items.len() != 2 {
        return Err(other("a stereo build needs exactly two mono parts"));
    }
    let sequence = project.add_sequence(name, preset, None, ids)?;
    let sequence_uid = project.sequence_id(sequence);
    let Some(root) = project.root_item() else { return Err(other("project has no root item")) };
    let item = project
        .item(root)
        .children
        .iter()
        .copied()
        .find(|&c| project.item(c).sequence_uid == sequence_uid)
        .ok_or_else(|| other("StopIteration"))?;
    move_items(project, &[item], target_bin)?;
    let tracks = project.audio_tracks(sequence).to_vec();
    for ((&track, &part), side) in tracks.iter().zip(audio_items).zip([Pan::Left, Pan::Right]) {
        let placed = project.add_clip(track, part, Time::new(0), ids)?;
        set_clip_pan(project, placed, side)?;
    }
    move_to_processed_clips(project, audio_items, Some(target_bin), ids)?;
    // Transition handle checks need the same in-memory bound as grouped/merged builds.
    let end = project.sequence_end(sequence)?.ticks;
    project.item_mut(item).default_out_ticks = Some(end);
    project.item_extra.entry(item).or_default().stereo_nest = true;
    Ok(item)
}

/// `declare_stereo_chain`: make a placed audio edit's chain stereo, as Premiere writes for a
/// stereo edit: the per-channel volume component, and AudioChannelLayout L/R + ChannelType 1
/// after ComponentChain. Without it a stereo edit (multicam group or nested sequence) played
/// left only (PRPROJ-0021).
pub fn declare_stereo_chain(project: &mut Project, placed: TrackItemId) -> Result<()> {
    let element = project.track_item(placed).element;
    let Some(chain_ref) = project.doc.tree.find1(element, "ClipTrackItem/ComponentOwner/Components") else {
        return Ok(());
    };
    let chain = project.doc.resolve(chain_ref)?;
    let tree = &mut project.doc.tree;
    if tree.child_by_tag(chain, "DefaultChannelVolumeComponentID").is_none() {
        let default = tree.child_by_tag(chain, "DefaultVolumeComponentID");
        let leaf = tree.new_element("DefaultChannelVolumeComponentID");
        tree.set_text(leaf, Some("2"));
        match default {
            Some(default) => {
                let tail = tree.tail(default).map(str::to_string);
                tree.set_tail(leaf, tail);
                let at = tree.children(chain).iter().position(|&c| c == default).expect("child") + 1;
                tree.insert(chain, at, leaf);
            }
            None => tree.insert(chain, 0, leaf),
        }
    }
    for (tag, text) in [("AudioChannelLayout", STEREO_LAYOUT), ("ChannelType", "1")] {
        let leaf = match tree.child_by_tag(chain, tag) {
            Some(leaf) => leaf,
            None => {
                let leaf = tree.sub_element(chain, tag, &[]);
                let children = tree.children(chain).to_vec();
                let previous = children[children.len() - 2];
                let previous_tail = tree.tail(previous).map(str::to_string);
                let first_tail = tree.tail(children[0]).map(str::to_string);
                tree.set_tail(leaf, previous_tail);
                tree.set_tail(previous, first_tail);
                leaf
            }
        };
        tree.set_text(leaf, Some(text));
    }
    Ok(())
}
