//! The rest of `Track` (port of `py_premiere/models/track.py`): mute/lock/sync-lock flags,
//! `remove_clip`, `move_clip`, `insert_clip`, `remove_transition`, the `NamedList` views and
//! the repr.
//!
//! Not ported here: `add_graphic` (graphic builder port) and `add_mgt` (Motion Graphics
//! template port).

use super::marker::{marker_start, set_marker_start};
use super::named_list::NamedList;
use super::{ItemId, MediaType, Project, TrackId, TrackItemId};
use crate::build::time::Time;
use crate::error::{not_implemented, value, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{append_leaf, insert_leaf_before, remove_child, set_elided_flag};
use crate::xml::{NodeId, PremiereDocument, ReferenceIndex};

/// `_MUTE_START_KEYFRAME`: the static keyframe of a muted audio track's `Mute` parameter.
pub const MUTE_START_KEYFRAME: &str = "-91445760000000000,true,0,0,0,0,0,0";

/// `_detach_from_links`: drop a track item from every `Link`; a link left with fewer than two
/// members goes too.
pub fn detach_from_links(document: &mut PremiereDocument, object_id: Option<&str>) -> Result<()> {
    let Some(object_id) = object_id else { return Ok(()) };
    for link in document.tree.children(document.root).to_vec() {
        if document.tree.tag(link) != "Link" {
            continue;
        }
        let Some(items) = document.tree.find1(link, "TrackItemGroup/TrackItems") else { continue };
        for reference in document.tree.children(items).to_vec() {
            if document.tree.get(reference, "ObjectRef") == Some(object_id) {
                remove_child(&mut document.tree, items, reference)?;
            }
        }
        if document.tree.len(items) < 2 {
            document.remove_object(link)?;
            continue;
        }
        for (index, reference) in document.tree.children(items).to_vec().into_iter().enumerate() {
            document.tree.set(reference, "Index", index.to_string());
        }
    }
    Ok(())
}

impl Project {
    /// `Track.index`.
    pub fn track_index(&self, track: TrackId) -> i64 {
        self.track(track).index
    }

    /// `Track.id`.
    pub fn track_id(&self, track: TrackId) -> i64 {
        self.track(track).id
    }

    /// `Track.media_type`.
    pub fn track_media_type(&self, track: TrackId) -> MediaType {
        self.track(track).media_type
    }

    /// `Track.clips` as a `NamedList`.
    pub fn track_clips_named(&self, track: TrackId) -> NamedList<TrackItemId> {
        self.clips_named(&self.track(track).clips.clone())
    }

    /// `repr(track)`.
    pub fn track_repr(&self, track: TrackId) -> String {
        let data = self.track(track);
        format!("Track({} index={}, {} clip(s))", data.media_type.as_str(), data.index, data.clips.len())
    }

    /// `Track._audio_mute_param`: the `Mute` param of an audio track's mix-graph fader.
    fn audio_mute_param(&self, track: TrackId) -> Result<Option<NodeId>> {
        let element = self.track(track).element;
        let Some(chain_ref) = self.doc.tree.find1(element, "AudioTrack/ComponentOwner/Components") else { return Ok(None) };
        let chain = self.doc.resolve(chain_ref)?;
        for component_ref in self.doc.tree.findall(chain, "ComponentChain/Components/Component")? {
            let component = self.doc.resolve(component_ref)?;
            for param_ref in self.doc.tree.findall(component, "AudioComponent/Component/Params/Param")? {
                let param = self.doc.resolve(param_ref)?;
                if self.doc.tree.findtext(param, "Name")?.as_deref() == Some("Mute") {
                    return Ok(Some(param));
                }
            }
        }
        Ok(None)
    }

    fn track_flag(&self, track: TrackId, tag: &str) -> Result<Option<String>> {
        self.doc.tree.findtext(self.track(track).element, &format!("ClipTrack/Track/{tag}"))
    }

    fn track_element(&self, track: TrackId) -> Result<NodeId> {
        match self.doc.tree.find1(self.track(track).element, "ClipTrack/Track") {
            Some(e) => Ok(e),
            None => value("track has no ClipTrack/Track element"),
        }
    }

    /// `Track.is_muted`: video `ClipTrack/Track/IsMuted`; audio the fader's `Mute` param.
    pub fn is_muted(&self, track: TrackId) -> Result<bool> {
        if self.track(track).media_type == MediaType::Audio {
            let Some(param) = self.audio_mute_param(track)? else { return Ok(false) };
            let start = self.doc.tree.findtext(param, "StartKeyframe")?.unwrap_or_default();
            if !start.is_empty() {
                return Ok(start.split(',').nth(1) == Some("true"));
            }
            return Ok(self.doc.tree.findtext(param, "CurrentValue")?.as_deref() == Some("true"));
        }
        Ok(self.track_flag(track, "IsMuted")?.as_deref() == Some("true"))
    }

    /// `Track.is_muted = value`.
    pub fn set_muted(&mut self, track: TrackId, muted: bool) -> Result<()> {
        if self.track(track).media_type == MediaType::Audio {
            let Some(param) = self.audio_mute_param(track)? else {
                return value("audio track has no Mute parameter to set");
            };
            let tree = &mut self.doc.tree;
            let start = tree.child_by_tag(param, "StartKeyframe");
            let current = tree.child_by_tag(param, "CurrentValue");
            if !muted {
                for e in [start, current].into_iter().flatten() {
                    remove_child(tree, param, e)?;
                }
                return Ok(());
            }
            match start {
                None => {
                    insert_leaf_before(tree, param, "Name", "StartKeyframe", MUTE_START_KEYFRAME)?;
                }
                Some(e) => tree.set_text(e, Some(MUTE_START_KEYFRAME)),
            }
            match current {
                None => {
                    insert_leaf_before(tree, param, "Name", "CurrentValue", "true")?;
                }
                Some(e) => tree.set_text(e, Some("true")),
            }
            return Ok(());
        }
        let element = self.track_element(track)?;
        set_elided_flag(&mut self.doc.tree, element, "IsMuted", muted, "true")
    }

    /// `Track.is_locked`.
    pub fn is_locked(&self, track: TrackId) -> Result<bool> {
        Ok(self.track_flag(track, "IsLocked")?.as_deref() == Some("true"))
    }

    /// `Track.is_locked = value`.
    pub fn set_locked(&mut self, track: TrackId, locked: bool) -> Result<()> {
        let element = self.track_element(track)?;
        set_elided_flag(&mut self.doc.tree, element, "IsLocked", locked, "true")
    }

    /// Set the saved timeline track label, independent of the lane index.
    pub(crate) fn set_custom_track_name(&mut self, track: TrackId, name: &str) -> Result<()> {
        let element = self.track_element(track)?;
        let tree = &mut self.doc.tree;
        let bags = tree.findall(element, "Node/Properties")?;
        let [bag] = bags.as_slice() else {
            return value("track has no unique property bag");
        };
        let names = tree.findall(*bag, "MZ.TrackName")?;
        match names.as_slice() {
            [] => { append_leaf(tree, *bag, "MZ.TrackName", name); }
            [node] if tree.is_childless(*node) => tree.set_text(*node, Some(name)),
            _ => return value("track has no unique name field"),
        }
        Ok(())
    }

    /// `Track.is_sync_locked` (on unless `IsSyncLocked` is `false`).
    pub fn is_sync_locked(&self, track: TrackId) -> Result<bool> {
        Ok(self.track_flag(track, "IsSyncLocked")?.as_deref() != Some("false"))
    }

    /// `Track.is_sync_locked = value` (stored inverted, as `false`).
    pub fn set_sync_locked(&mut self, track: TrackId, synced: bool) -> Result<()> {
        let element = self.track_element(track)?;
        set_elided_flag(&mut self.doc.tree, element, "IsSyncLocked", !synced, "false")
    }

    /// `Track._detach_track_item_ref(object_id, container)`.
    pub fn detach_track_item_ref(&mut self, track: TrackId, object_id: Option<&str>, container: &str) -> Result<()> {
        let element = self.track(track).element;
        let tree = &mut self.doc.tree;
        let Some(track_items) = tree.find1(element, &format!("ClipTrack/{container}/TrackItems")) else { return Ok(()) };
        let refs: Vec<NodeId> = tree.children(track_items).iter().copied().filter(|&c| tree.tag(c) == "TrackItem").collect();
        if let Some(&r) = refs.iter().find(|&&r| tree.get(r, "ObjectRef") == object_id) {
            remove_child(tree, track_items, r)?;
        }
        if tree.is_childless(track_items) {
            if let Some(container_element) = tree.find1(element, &format!("ClipTrack/{container}")) {
                remove_child(tree, container_element, track_items)?;
            }
        } else {
            let refs: Vec<NodeId> = tree.children(track_items).iter().copied().filter(|&c| tree.tag(c) == "TrackItem").collect();
            for (index, r) in refs.into_iter().enumerate() {
                tree.set(r, "Index", index.to_string());
            }
        }
        Ok(())
    }

    /// `Track.remove_clip(clip)`: the track item, its chain, subclip, timeline clip and
    /// exclusively owned `SecondaryContent` objects go; links are unhooked.
    pub fn remove_clip(&mut self, track: TrackId, clip: TrackItemId) -> Result<()> {
        if !self.track(track).clips.contains(&clip) {
            return value("clip is not on this track");
        }
        let data = self.track_item(clip).clone();
        let object_id = self.doc.tree.get(data.element, "ObjectID").map(str::to_string);
        self.detach_track_item_ref(track, object_id.as_deref(), "ClipItems")?;
        detach_from_links(&mut self.doc, object_id.as_deref())?;
        let mut objects = vec![data.element, data.subclip, data.clip];
        if let Some(chain_ref) = self.doc.tree.find1(data.element, "ClipTrackItem/ComponentOwner/Components") {
            objects.push(self.doc.resolve(chain_ref)?);
        }
        let secondaries = self.doc.tree.findall(data.clip, "SecondaryContents/SecondaryContentItem")?;
        if !secondaries.is_empty() {
            let index = ReferenceIndex::new(&self.doc);
            for item_ref in secondaries {
                let content = self.doc.resolve(item_ref)?;
                let elsewhere = if self.doc.tree.get(content, "ObjectID").is_none() {
                    true
                } else {
                    let mut ignoring = vec![content];
                    ignoring.extend(objects.iter().copied());
                    !index.referrers_outside(content, &ignoring).is_empty()
                };
                if !elsewhere {
                    objects.push(content);
                }
            }
        }
        for element in objects {
            self.doc.remove_object(element)?;
        }
        self.track_mut(track).clips.retain(|&c| c != clip);
        Ok(())
    }

    /// `Track.move_clip(clip, target_track, start)`: move a clip to another track of the same
    /// kind (optionally to a new start); the objects are kept.
    pub fn move_clip(&mut self, track: TrackId, clip: TrackItemId, target: TrackId, start: Option<Time>) -> Result<()> {
        if !self.track(track).clips.contains(&clip) {
            return value("clip is not on this track");
        }
        if self.track(target).media_type != self.track(track).media_type {
            return value("cannot move a clip to a different media type");
        }
        if target == track && start.is_none() {
            return Ok(());
        }
        let element = self.track_item(clip).element;
        let Some(object_id) = self.doc.tree.get(element, "ObjectID").map(str::to_string) else {
            return value("clip has no ObjectID");
        };
        if let Some(start) = start {
            super::track::place_on_timeline(&mut self.doc.tree, element, start)?;
        }
        self.detach_track_item_ref(track, Some(&object_id), "ClipItems")?;
        self.add_track_item_ref(target, &object_id, "ClipItems")?;
        self.track_mut(track).clips.retain(|&c| c != clip);
        self.track_mut(target).clips.push(clip);
        self.track_item_mut(clip).track = target;
        let sequence = self.track(target).sequence;
        self.grow_source_duration(sequence)
    }

    /// `Track.insert_clip(project_item, start)`: an insert edit; everything at or after
    /// `start` on every track (and the sequence markers) shifts by the inserted duration.
    pub fn insert_clip(&mut self, track: TrackId, item: ItemId, start: Time, uuids: &mut dyn UuidSource) -> Result<TrackItemId> {
        let sequence = self.track(track).sequence;
        let point = start.ticks;
        let mut to_shift = Vec::new();
        let tracks: Vec<TrackId> = self.video_tracks(sequence).iter().chain(self.audio_tracks(sequence)).copied().collect();
        for t in tracks {
            for clip in self.track(t).clips.clone() {
                if self.end(clip)?.ticks <= point {
                    continue;
                }
                if self.start(clip)?.ticks < point {
                    return not_implemented("the insert point splits an existing clip");
                }
                to_shift.push(clip);
            }
        }
        let placed = self.add_clip(track, item, start, uuids)?;
        let delta = self.duration(placed)?.ticks;
        for clip in to_shift {
            let end = self.end(clip)?.ticks;
            self.set_end(clip, Time::new(end + delta))?;
            let begin = self.start(clip)?.ticks;
            self.set_start(clip, Time::new(begin + delta))?;
        }
        for marker in self.sequence(sequence).markers.clone() {
            let marker_start_time = marker_start(&self.doc.tree, marker)?;
            if marker_start_time.ticks >= point {
                set_marker_start(&mut self.doc.tree, marker, Time::new(marker_start_time.ticks + delta))?;
            }
        }
        Ok(placed)
    }

    /// `Track.remove_transition(transition)`: the transition object and what only it owned,
    /// the track's reference, and each clip's head/tail reference to it.
    pub fn remove_transition(&mut self, track: TrackId, transition: NodeId) -> Result<()> {
        if !self.track(track).transitions.contains(&transition) {
            return value("transition is not on this track");
        }
        let object_id = self.doc.tree.get(transition, "ObjectID").map(str::to_string);
        self.detach_track_item_ref(track, object_id.as_deref(), "TransitionItems")?;
        for clip in self.track(track).clips.clone() {
            let element = self.track_item(clip).element;
            let tree = &mut self.doc.tree;
            let Some(core) = tree.child_by_tag(element, "ClipTrackItem") else { continue };
            for tag in ["HeadTransition", "TailTransition"] {
                if let Some(reference) = tree.child_by_tag(core, tag) {
                    if tree.get(reference, "ObjectRef").map(str::to_string) == object_id {
                        remove_child(tree, core, reference)?;
                    }
                }
            }
        }
        for owned in self.doc.owned_objects(&[transition], None) {
            self.doc.remove_object(owned)?;
        }
        self.track_mut(track).transitions.retain(|&t| t != transition);
        Ok(())
    }
}
