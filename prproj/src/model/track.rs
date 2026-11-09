//! Track operations (port of `py_premiere/models/track.py`: `add_clip`, `add_transition` and
//! the element builders they use).

use super::project::py_int;
use super::track_item::py_div_trunc;
use super::{ItemId, MediaType, Project, TrackId, TrackItemData, TrackItemId};
use crate::build::marker::validate_string;
use crate::build::time::{Time, TICKS_PER_SECOND};
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{append_child, append_leaf, insert_before, insert_leaf_before, remove_child};
use crate::xml::{NodeId, PremiereDocument, Tree};

/// ClassIDs of the objects a clip placement synthesizes.
pub const SUBCLIP_CLASS_ID: &str = "e0c58dc9-dbdd-4166-aef7-5db7e3f22e84";
pub const VIDEO_CHAIN_CLASS_ID: &str = "0970e08a-f58f-4108-b29a-1a717b8e12e2";
pub const AUDIO_CHAIN_CLASS_ID: &str = "3cb131d1-d3c0-47ae-a19a-bdf75ea11674";
pub const VIDEO_ITEM_CLASS_ID: &str = "368b0406-29e3-4923-9fcd-094fbf9a1089";
pub const AUDIO_ITEM_CLASS_ID: &str = "064ec682-9ba6-11d5-af2d-9ca32c7d6164";
pub const SECONDARY_CONTENT_CLASS_ID: &str = "f9d004b5-cb04-4e2f-af6f-64fadc2c4be9";
/// ClassIDs of the two transition track-item classes.
pub const VIDEO_TRANSITION_CLASS_ID: &str = "3eeaed31-f78e-4144-b8e8-077656517181";
pub const AUDIO_TRANSITION_CLASS_ID: &str = "23f687b6-9c5a-42f4-bea1-e7b3e28e7082";

/// `_TONE_MAP_DEFAULT`: what Premiere writes for every SDR placement.
pub const TONE_MAP_DEFAULT: &str = "{\"peak\":-1,\"version\":3}";

/// `TRANSITION_DISPLAY_NAMES`: matchName -> the display name Premiere writes.
pub const TRANSITION_DISPLAY_NAMES: [(&str, &str); 3] = [
    ("ADBE Additive Dissolve", "Additive Dissolve (Legacy)"),
    ("ADBE Band Wipe", "Band Wipe"),
    ("Constant Power", "Constant Power"),
];

fn leaf(tree: &mut Tree, parent: NodeId, tag: &str, text: &str, tail: &str) -> NodeId {
    let element = tree.sub_element(parent, tag, &[]);
    tree.set_text(element, Some(text));
    tree.set_tail(element, Some(tail));
    element
}

fn ref_child(tree: &mut Tree, parent: NodeId, tag: &str, attrs: &[(&str, &str)], tail: &str) -> NodeId {
    let element = tree.sub_element(parent, tag, attrs);
    tree.set_tail(element, Some(tail));
    element
}

/// `_new_subclip_element`.
pub fn new_subclip_element(tree: &mut Tree, clip_id: &str, master_uid: &str, name: &str) -> NodeId {
    let e = tree.new_element_with("SubClip", &[("ObjectID", ""), ("ClassID", SUBCLIP_CLASS_ID), ("Version", "6")]);
    tree.set_text(e, Some("\n\t\t"));
    ref_child(tree, e, "Clip", &[("ObjectRef", clip_id)], "\n\t\t");
    ref_child(tree, e, "MasterClip", &[("ObjectURef", master_uid)], "\n\t\t");
    leaf(tree, e, "Name", name, "\n\t\t");
    leaf(tree, e, "OrigChGrp", "0", "\n\t");
    e
}

/// `_new_video_chain_element`: the default (empty) video component chain.
pub fn new_video_chain_element(tree: &mut Tree) -> NodeId {
    let e = tree.new_element_with("VideoComponentChain", &[("ObjectID", ""), ("ClassID", VIDEO_CHAIN_CLASS_ID), ("Version", "3")]);
    tree.set_text(e, Some("\n\t\t"));
    leaf(tree, e, "DefaultMotion", "true", "\n\t\t");
    leaf(tree, e, "DefaultOpacity", "true", "\n\t\t");
    leaf(tree, e, "DefaultMotionComponentID", "1", "\n\t\t");
    leaf(tree, e, "DefaultOpacityComponentID", "2", "\n\t\t");
    let chain = tree.sub_element(e, "ComponentChain", &[("Version", "3")]);
    tree.set_text(chain, Some("\n\t\t"));
    tree.set_tail(chain, Some("\n\t"));
    e
}

/// `_new_audio_chain_element`: the default audio component chain.
pub fn new_audio_chain_element(tree: &mut Tree) -> NodeId {
    let e = tree.new_element_with("AudioComponentChain", &[("ObjectID", ""), ("ClassID", AUDIO_CHAIN_CLASS_ID), ("Version", "4")]);
    tree.set_text(e, Some("\n\t\t"));
    leaf(tree, e, "DefaultVol", "true", "\n\t\t");
    leaf(tree, e, "DefaultVolumeComponentID", "1", "\n\t\t");
    let chain = tree.sub_element(e, "ComponentChain", &[("Version", "3")]);
    tree.set_text(chain, Some("\n\t\t\t"));
    tree.set_tail(chain, Some("\n\t"));
    let node = tree.sub_element(chain, "Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t\t"));
    tree.set_tail(node, Some("\n\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t\t"));
    leaf(tree, properties, "MZ.ComponentChain.ActiveComponentID", "1", "\n\t\t\t\t\t");
    leaf(tree, properties, "MZ.ComponentChain.ActiveComponentParamIndex", "4294967295", "\n\t\t\t\t");
    e
}

/// `_new_secondary_content_element`.
pub fn new_secondary_content_element(tree: &mut Tree, content_ref: &str, channel: &str) -> NodeId {
    let e = tree.new_element_with("SecondaryContent", &[("ObjectID", ""), ("ClassID", SECONDARY_CONTENT_CLASS_ID), ("Version", "1")]);
    tree.set_text(e, Some("\n\t\t"));
    ref_child(tree, e, "Content", &[("ObjectRef", content_ref)], "\n\t\t");
    leaf(tree, e, "ChannelIndex", channel, "\n\t");
    e
}

/// `_new_clip_track_item` (the inner `ClipTrackItem`, detached).
fn new_clip_track_item(tree: &mut Tree, chain_id: &str, subclip_id: &str, end_ticks: i64) -> NodeId {
    let inner = tree.new_element_with("ClipTrackItem", &[("Version", "8")]);
    tree.set_text(inner, Some("\n\t\t\t"));
    tree.set_tail(inner, Some("\n\t\t"));
    let owner = tree.sub_element(inner, "ComponentOwner", &[("Version", "1")]);
    tree.set_text(owner, Some("\n\t\t\t\t"));
    tree.set_tail(owner, Some("\n\t\t\t"));
    ref_child(tree, owner, "Components", &[("ObjectRef", chain_id)], "\n\t\t\t");
    let track_item = tree.sub_element(inner, "TrackItem", &[("Version", "4")]);
    tree.set_text(track_item, Some("\n\t\t\t\t"));
    tree.set_tail(track_item, Some("\n\t\t\t"));
    leaf(tree, track_item, "End", &end_ticks.to_string(), "\n\t\t\t");
    ref_child(tree, inner, "SubClip", &[("ObjectRef", subclip_id)], "\n\t\t");
    inner
}

/// `_new_video_track_item_element`: `PixelAspectRatio`/`FrameRect` come from the source's
/// video stream when there is one.
pub fn new_video_track_item_element(
    tree: &mut Tree,
    chain_id: &str,
    subclip_id: &str,
    end_ticks: i64,
    video_stream: Option<NodeId>,
) -> Result<NodeId> {
    let e = tree.new_element_with("VideoClipTrackItem", &[("ObjectID", ""), ("ClassID", VIDEO_ITEM_CLASS_ID), ("Version", "8")]);
    tree.set_text(e, Some("\n\t\t"));
    let inner = new_clip_track_item(tree, chain_id, subclip_id, end_ticks);
    tree.append(e, inner);
    let mut aspect = "1,1".to_string();
    let mut frame_rect = None;
    if let Some(stream) = video_stream {
        aspect = tree.findtext(stream, "PixelAspectRatio")?.filter(|t| !t.is_empty()).unwrap_or_else(|| "1,1".into());
        frame_rect = tree.findtext(stream, "FrameRect")?;
    }
    leaf(tree, e, "PixelAspectRatio", &aspect, "\n\t\t");
    leaf(tree, e, "ToneMapSettings", TONE_MAP_DEFAULT, if frame_rect.is_some() { "\n\t\t" } else { "\n\t" });
    if let Some(rect) = frame_rect {
        leaf(tree, e, "FrameRect", &rect, "\n\t");
    }
    Ok(e)
}

/// `_new_audio_track_item_element` (draws the item's `ID`).
pub fn new_audio_track_item_element(tree: &mut Tree, chain_id: &str, subclip_id: &str, end_ticks: i64, uuids: &mut dyn UuidSource) -> NodeId {
    let e = tree.new_element_with("AudioClipTrackItem", &[("ObjectID", ""), ("ClassID", AUDIO_ITEM_CLASS_ID), ("Version", "11")]);
    tree.set_text(e, Some("\n\t\t"));
    let inner = new_clip_track_item(tree, chain_id, subclip_id, end_ticks);
    tree.append(e, inner);
    leaf(tree, e, "ID", &uuids.uuid4().to_string(), "\n\t\t");
    leaf(tree, e, "PreRenderComponentChainHashVersion", "1", "\n\t");
    e
}

/// `_new_transition_element`.
#[allow(clippy::too_many_arguments)]
pub fn new_transition_element(
    tree: &mut Tree,
    media_type: MediaType,
    start: i64,
    end: i64,
    offset: i64,
    on_a_cut: bool,
    at_start: bool,
    name: &str,
    match_name: &str,
    channel_layout: Option<&str>,
) -> NodeId {
    let video = media_type == MediaType::Video;
    let e = tree.new_element_with(
        if video { "VideoTransitionTrackItem" } else { "AudioTransitionTrackItem" },
        &[
            ("ClassID", if video { VIDEO_TRANSITION_CLASS_ID } else { AUDIO_TRANSITION_CLASS_ID }),
            ("Version", if video { "6" } else { "4" }),
        ],
    );
    tree.set_text(e, Some("\n\t\t"));
    let inner = tree.sub_element(e, "TransitionTrackItem", &[("Version", "3")]);
    tree.set_text(inner, Some("\n\t\t\t"));
    tree.set_tail(inner, Some("\n\t"));
    let item = tree.sub_element(inner, "TrackItem", &[("Version", "4")]);
    tree.set_text(item, Some("\n\t\t\t\t"));
    tree.set_tail(item, Some("\n\t\t\t"));
    if start != 0 {
        leaf(tree, item, "Start", &start.to_string(), "\n\t\t\t\t");
    }
    leaf(tree, item, "End", &end.to_string(), "\n\t\t\t");
    leaf(tree, inner, "Alignment", &offset.to_string(), "\n\t\t\t");
    leaf(tree, inner, "DisplayName", name, "\n\t\t\t");
    leaf(tree, inner, "MatchName", match_name, "\n\t\t\t");
    let outgoing = if on_a_cut || !at_start { "true" } else { "false" };
    let incoming = if on_a_cut || at_start { "true" } else { "false" };
    leaf(tree, inner, "HasOutgoingClip", outgoing, "\n\t\t\t");
    leaf(tree, inner, "HasIncomingClip", incoming, "\n\t\t");
    if let Some(layout) = channel_layout {
        tree.set_tail(inner, Some("\n\t\t"));
        leaf(tree, e, "AudioChannelLayout", layout, "\n\t");
    }
    e
}

/// `_attach_transition_ref`: `<HeadTransition ObjectRef=".."/>` (or Tail) appended to the core.
fn attach_transition_ref(tree: &mut Tree, core: NodeId, tag: &str, object_id: &str) {
    let reference = append_leaf(tree, core, tag, "");
    tree.set_text(reference, None::<String>);
    tree.set(reference, "ObjectRef", object_id);
}

impl Project {
    /// `Track.clips`.
    pub fn track_clips(&self, track: TrackId) -> &[TrackItemId] {
        &self.track(track).clips
    }

    /// `Track.transitions`: the transition objects.
    pub fn track_transitions(&self, track: TrackId) -> &[NodeId] {
        &self.track(track).transitions
    }

    /// `Track.add_clip(project_item, start)`: place one half of an item on this track.
    ///
    /// Deep-copies the master's template clip of this track's kind (fresh ClipID, no InUse,
    /// in/out from the panel marks or the whole media trimmed to sequence frames), gives an
    /// audio clip its own SecondaryContent objects, then adds the SubClip, the default component
    /// chain and the track item (Start elided at 0) and references it from the track. Draws the
    /// ClipID, then (audio) the track item ID.
    pub fn add_clip(&mut self, track: TrackId, item: ItemId, start: Time, uuids: &mut dyn UuidSource) -> Result<TrackItemId> {
        let sequence = self.track(track).sequence;
        let media_type = self.track(track).media_type;
        if let Some(uid) = &self.item(item).sequence_uid {
            if Some(uid.as_str()) == self.sequence_id(sequence).as_deref() {
                return value("cannot place a sequence on its own timeline");
            }
        }
        let Some(master) = self.item(item).master else { return value("item has no master clip to place") };
        let wanted = match media_type {
            MediaType::Video => "VideoClip",
            MediaType::Audio => "AudioClip",
        };
        let mut own = None;
        for reference in self.doc.tree.findall(master, "Clips/Clip")? {
            let candidate = self.doc.resolve(reference)?;
            if self.doc.tree.tag(candidate) == wanted {
                own = Some(candidate);
                break;
            }
        }
        let Some(own) = own else {
            return value(format!("item has no {} stream to place", media_type.as_str().to_lowercase()));
        };
        let Some(core) = self.doc.tree.child_by_tag(own, "Clip") else { return value("master clip has no Clip core") };
        let (in_ticks, out_ticks) = self.resolve_in_out(track, core)?;

        let clip = self.doc.tree.deep_copy(own);
        let tree = &mut self.doc.tree;
        let Some(copy_core) = tree.child_by_tag(clip, "Clip") else { return value("master clip has no Clip core") };
        if let Some(in_use) = tree.child_by_tag(copy_core, "InUse") {
            remove_child(tree, copy_core, in_use)?;
        }
        if tree.child_by_tag(copy_core, "OutPoint").is_none() {
            insert_leaf_before(tree, copy_core, "ClipID", "OutPoint", &out_ticks.to_string())?;
        }
        if tree.child_by_tag(copy_core, "InPoint").is_none() {
            insert_leaf_before(tree, copy_core, "ClipID", "InPoint", &in_ticks.to_string())?;
        }
        let Some(clip_id) = tree.child_by_tag(copy_core, "ClipID") else { return value("master clip has no ClipID") };
        tree.set_text(clip_id, Some(uuids.uuid4().to_string()));
        if media_type == MediaType::Audio {
            attach_secondary_content(&mut self.doc, clip)?;
        }
        let clip_object_id = self.doc.add_object(clip)?;

        let tree = &self.doc.tree;
        let name = tree.child_by_tag(master, "Name").map(|e| tree[e].text_or_empty().to_string()).unwrap_or_default();
        let master_uid = tree.get(master, "ObjectUID").unwrap_or("").to_string();
        let subclip = new_subclip_element(&mut self.doc.tree, &clip_object_id, &master_uid, &name);
        let subclip_object_id = self.doc.add_object(subclip)?;

        let chain = match media_type {
            MediaType::Video => new_video_chain_element(&mut self.doc.tree),
            MediaType::Audio => new_audio_chain_element(&mut self.doc.tree),
        };
        let chain_object_id = self.doc.add_object(chain)?;

        let element = match media_type {
            MediaType::Video => {
                let stream = self.video_stream(core)?;
                new_video_track_item_element(&mut self.doc.tree, &chain_object_id, &subclip_object_id, out_ticks - in_ticks, stream)?
            }
            MediaType::Audio => {
                new_audio_track_item_element(&mut self.doc.tree, &chain_object_id, &subclip_object_id, out_ticks - in_ticks, uuids)
            }
        };
        place_on_timeline(&mut self.doc.tree, element, start)?;
        let item_object_id = self.doc.add_object(element)?;
        self.add_track_item_ref(track, &item_object_id, "ClipItems")?;
        let placed = self.push_track_item(TrackItemData::new(element, subclip, clip, track));
        self.track_mut(track).clips.push(placed);
        self.grow_source_duration(sequence)?;
        Ok(placed)
    }

    /// `Track._resolve_in_out`: panel marks when present, else the whole media trimmed down
    /// to whole sequence frames. An unmarked still is refused.
    fn resolve_in_out(&self, track: TrackId, core: NodeId) -> Result<(i64, i64)> {
        let tree = &self.doc.tree;
        if let Some(out_text) = tree.findtext(core, "OutPoint")? {
            let in_ticks = match tree.findtext(core, "InPoint")? {
                Some(t) if !t.is_empty() => py_int(&t)?,
                _ => 0,
            };
            return Ok((in_ticks, py_int(&out_text)?));
        }
        let Some(source_ref) = tree.child_by_tag(core, "Source") else { return value("master clip has no Source") };
        let source = self.doc.resolve(source_ref)?;
        let media = match tree.find1(source, "MediaSource/Media") {
            Some(r) => Some(self.doc.resolve(r)?),
            None => None,
        };
        if let Some(media) = media {
            if tree.findtext(media, "Infinite")?.as_deref() == Some("true") {
                return value(
                    "placing an unmarked still is not supported: its default duration is a Premiere preference the file does not store",
                );
            }
        }
        let Some(duration_text) = tree.findtext(source, "OriginalDuration")? else {
            return value("media source has no OriginalDuration");
        };
        let mut duration = py_int(&duration_text)?;
        if let Some(timebase) = self.timebase(self.track(track).sequence).filter(|&t| t != 0) {
            duration -= duration.rem_euclid(timebase);
        }
        Ok((0, duration))
    }

    /// `Track._video_stream`: the source media's `VideoStream` object.
    fn video_stream(&self, core: NodeId) -> Result<Option<NodeId>> {
        let tree = &self.doc.tree;
        let Some(source_ref) = tree.child_by_tag(core, "Source") else { return Ok(None) };
        let Some(media_ref) = tree.find1(self.doc.resolve(source_ref)?, "MediaSource/Media") else { return Ok(None) };
        let Some(stream_ref) = tree.child_by_tag(self.doc.resolve(media_ref)?, "VideoStream") else { return Ok(None) };
        Ok(Some(self.doc.resolve(stream_ref)?))
    }

    /// `Track._add_track_item_ref`: reference a track item (or, with `"TransitionItems"`, a
    /// transition) from the track, creating the `TrackItems` list before `MediaType`.
    pub fn add_track_item_ref(&mut self, track: TrackId, object_id: &str, container: &str) -> Result<()> {
        let element = self.track(track).element;
        let tree = &mut self.doc.tree;
        let Some(clip_items) = tree.find1(element, &format!("ClipTrack/{container}")) else {
            return value(format!("track has no {container} container"));
        };
        let entry = tree.new_element_with("TrackItem", &[("Index", "0"), ("ObjectRef", object_id)]);
        match tree.child_by_tag(clip_items, "TrackItems") {
            None => {
                let base = tree.text(clip_items).filter(|t| !t.is_empty()).unwrap_or("\n\t\t\t\t").to_string();
                let items = tree.new_element_with("TrackItems", &[("Version", "1")]);
                tree.set_text(items, Some(format!("{base}\t")));
                tree.set_tail(entry, Some(base));
                tree.append(items, entry);
                insert_before(tree, clip_items, "MediaType", items)
            }
            Some(items) => {
                let index = tree.len(items).to_string();
                tree.set(entry, "Index", index);
                append_child(tree, items, entry, Some("\n\t\t\t\t\t"));
                Ok(())
            }
        }
    }

    /// `Track.add_transition(clip, match_name, at_start, duration, name)`: a transition on one
    /// end of a clip; on a cut when a clip butts against that end (each half clamped to the
    /// footage the clips have). `name` defaults to the verified display name.
    pub fn add_transition(
        &mut self,
        track: TrackId,
        clip: TrackItemId,
        match_name: &str,
        at_start: bool,
        duration: Option<Time>,
        name: Option<&str>,
    ) -> Result<NodeId> {
        validate_string(match_name)?;
        if !self.track(track).clips.contains(&clip) {
            return value("clip is not on this track");
        }
        let name = match name {
            Some(n) => n.to_string(),
            None => match TRANSITION_DISPLAY_NAMES.iter().find(|(m, _)| *m == match_name) {
                Some((_, n)) => n.to_string(),
                None => return value(format!("no verified display name for '{match_name}'; pass name=")),
            },
        };
        validate_string(&name)?;
        if let Some(d) = duration {
            if d.ticks <= 0 {
                return value("a transition needs a positive duration");
            }
        }
        let neighbour = self.neighbour_of(track, clip, at_start)?;
        let (start, end, offset) = match neighbour {
            None => self.single_sided_span(track, clip, at_start, duration)?,
            Some(n) => {
                let (outgoing, incoming) = if at_start { (n, clip) } else { (clip, n) };
                self.cut_span(track, outgoing, incoming, duration)?
            }
        };
        self.attach_transition(track, clip, neighbour, at_start, start, end, offset, &name, match_name)
    }

    /// Write a Cross Dissolve at the exact prepared cut and use its explicit endpoints.
    pub(crate) fn add_prepared_dissolve(
        &mut self, track: TrackId, outgoing: TrackItemId, incoming: TrackItemId,
        start: i64, end: i64, offset: i64,
    ) -> Result<NodeId> {
        if self.track(track).media_type != MediaType::Video
            || self.track_item(outgoing).track != track || self.track_item(incoming).track != track
            || outgoing == incoming || start.checked_add(offset) != Some(self.start(incoming)?.ticks)
            || self.end(outgoing)?.ticks != self.start(incoming)?.ticks
        {
            return value("prepared dissolve endpoints do not match the written cut");
        }
        self.attach_transition(
            track, incoming, Some(outgoing), true, start, end, offset,
            "Cross Dissolve (Legacy)", "ADBE Cross Dissolve",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn attach_transition(
        &mut self, track: TrackId, clip: TrackItemId, neighbour: Option<TrackItemId>,
        at_start: bool, start: i64, end: i64, offset: i64, name: &str, match_name: &str,
    ) -> Result<NodeId> {
        let tree = &self.doc.tree;
        let Some(core) = tree.child_by_tag(self.track_item(clip).element, "ClipTrackItem") else {
            return value("clip has no ClipTrackItem");
        };
        let tag = if at_start { "HeadTransition" } else { "TailTransition" };
        if tree.child_by_tag(core, tag).is_some() {
            return value(format!("clip already has a {tag}"));
        }
        let other_tag = if at_start { "TailTransition" } else { "HeadTransition" };
        let mut other_core = None;
        if let Some(n) = neighbour {
            other_core = tree.child_by_tag(self.track_item(n).element, "ClipTrackItem");
            if let Some(oc) = other_core {
                if tree.child_by_tag(oc, other_tag).is_some() {
                    return value(format!("the adjacent clip already has a {other_tag}"));
                }
            }
        }
        let layout = tree.findtext(self.track_item(clip).clip, "AudioChannelLayout")?;
        let media_type = self.track(track).media_type;
        let element = new_transition_element(
            &mut self.doc.tree,
            media_type,
            start,
            end,
            offset,
            neighbour.is_some(),
            at_start,
            name,
            match_name,
            layout.as_deref(),
        );
        let object_id = self.doc.add_object(element)?;
        self.add_track_item_ref(track, &object_id, "TransitionItems")?;
        attach_transition_ref(&mut self.doc.tree, core, tag, &object_id);
        if let Some(oc) = other_core {
            attach_transition_ref(&mut self.doc.tree, oc, other_tag, &object_id);
        }
        self.track_mut(track).transitions.push(element);
        Ok(element)
    }

    fn neighbour_of(&self, track: TrackId, clip: TrackItemId, at_start: bool) -> Result<Option<TrackItemId>> {
        let boundary = if at_start { self.start(clip)? } else { self.end(clip)? };
        for &candidate in &self.track(track).clips {
            if candidate == clip {
                continue;
            }
            let edge = if at_start { self.end(candidate)? } else { self.start(candidate)? };
            if edge == boundary {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn frames(&self, track: TrackId, ticks: i64) -> Result<i64> {
        match self.timebase(self.track(track).sequence).filter(|&t| t != 0) {
            Some(timebase) => Ok(ticks.div_euclid(timebase) * timebase),
            None => value("sequence has no timebase to align a transition to"),
        }
    }

    fn single_sided_span(&self, track: TrackId, clip: TrackItemId, at_start: bool, duration: Option<Time>) -> Result<(i64, i64, i64)> {
        let wanted = self.frames(track, duration.map_or(TICKS_PER_SECOND, |d| d.ticks))?;
        if wanted <= 0 {
            return value("a transition needs at least one whole frame");
        }
        let (start, end) = (self.start(clip)?.ticks, self.end(clip)?.ticks);
        if wanted > end - start {
            return value("transition is longer than the clip it sits on");
        }
        Ok(if at_start { (start, start + wanted, 0) } else { (end - wanted, end, wanted) })
    }

    fn cut_span(&self, track: TrackId, outgoing: TrackItemId, incoming: TrackItemId, duration: Option<Time>) -> Result<(i64, i64, i64)> {
        let half = self.frames(track, duration.map_or(TICKS_PER_SECOND, |d| d.ticks.div_euclid(2)))?;
        if half <= 0 {
            return value("a transition needs at least one whole frame a side");
        }
        let before = half.min(self.handle_before(incoming)?).min(self.end(outgoing)?.ticks - self.start(outgoing)?.ticks);
        let after = half.min(self.handle_after(outgoing)?).min(self.end(incoming)?.ticks - self.start(incoming)?.ticks);
        let (before, after) = (self.frames(track, before)?, self.frames(track, after)?);
        if before + after <= 0 {
            return value("neither clip has the footage to play across the cut");
        }
        let cut = self.end(outgoing)?.ticks;
        Ok((cut - before, cut + after, before))
    }

    /// `_handle_before`: footage before the in point, in timeline ticks.
    fn handle_before(&self, clip: TrackItemId) -> Result<i64> {
        py_div_trunc(self.in_point(clip)?.ticks, self.speed(clip)?)
    }

    /// `_handle_after`: footage past the out point, in timeline ticks.
    fn handle_after(&self, clip: TrackItemId) -> Result<i64> {
        let source = self
            .track_item_source(clip)
            .and_then(|i| self.item(i).default_out_ticks)
            .ok_or_else(|| Error::Value("cannot size a transition: the clip's source duration is unknown".into()))?;
        let spare = source - self.out_point(clip)?.ticks;
        Ok(py_div_trunc(spare, self.speed(clip)?)?.max(0))
    }
}

/// `Track._attach_secondary_content`: each placed AudioClip gets its own SecondaryContent
/// objects (the channel mapping); only the referenced Source is shared.
fn attach_secondary_content(document: &mut PremiereDocument, clip: NodeId) -> Result<()> {
    for item_ref in document.tree.findall(clip, "SecondaryContents/SecondaryContentItem")? {
        let template = document.resolve(item_ref)?;
        let tree = &document.tree;
        let Some(content_ref) = tree.child_by_tag(template, "Content") else {
            return value("secondary content has no Content reference");
        };
        let content_target = tree.get(content_ref, "ObjectRef").unwrap_or("").to_string();
        let channel = tree.findtext(template, "ChannelIndex")?.filter(|t| !t.is_empty()).unwrap_or_else(|| "0".into());
        let content = new_secondary_content_element(&mut document.tree, &content_target, &channel);
        let id = document.add_object(content)?;
        document.tree.set(item_ref, "ObjectRef", id);
    }
    Ok(())
}

/// `Track._place_on_timeline`: shift the item's Start/End to `start` (Start elided at 0).
pub(crate) fn place_on_timeline(tree: &mut Tree, item: NodeId, start: Time) -> Result<()> {
    let Some(track_item) = tree.find1(item, "ClipTrackItem/TrackItem") else { return Ok(()) };
    let text = |tag: &str| tree.child_by_tag(track_item, tag).map(|e| tree[e].text_or_empty().to_string());
    let int_or_zero = |t: Option<String>| -> Result<i64> {
        match t {
            Some(t) if !t.is_empty() => py_int(&t),
            _ => Ok(0),
        }
    };
    let end = int_or_zero(text("End"))?;
    let source_start = int_or_zero(text("Start"))?;
    let duration = end - source_start;
    if let Some(end_element) = tree.child_by_tag(track_item, "End") {
        tree.set_text(end_element, Some((start.ticks + duration).to_string()));
    }
    let start_element = tree.child_by_tag(track_item, "Start");
    if start.ticks != 0 {
        match start_element {
            None => {
                insert_leaf_before(tree, track_item, "End", "Start", &start.ticks.to_string())?;
            }
            Some(e) => tree.set_text(e, Some(start.ticks.to_string())),
        }
    } else if let Some(e) = start_element {
        remove_child(tree, track_item, e)?;
    }
    Ok(())
}
