//! Sequence operations (port of `py_premiere/models/sequence.py`, the parts the converter uses).

use super::project::py_int;
use super::{Project, SequenceId, TrackId, TrackItemId};
use crate::build::marker::{attach_marker, ensure_sequence_marker_list, new_marker, MarkerSpec};
use crate::build::merged::build_link;
use crate::build::time::{Time, UNSET_TICKS};
use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{append_child, append_leaf, insert_leaf_before};
use crate::xml::NodeId;

/// `_BAG_ORDER`: the property-bag keys the setters touch, in Premiere's stored order.
const BAG_ORDER: [&str; 9] = [
    "MZ.InPoint",
    "MZ.OutPoint",
    "MZ.ZeroPoint",
    "AMM.CurrentSolo",
    "TL.SQAVDividerPosition",
    "MZ.WorkInPoint",
    "MZ.WorkOutPoint",
    "MZ.EditLine",
    "MZ.Sequence.VideoTimeDisplayFormat",
];

/// `_CREATABLE_BAG_KEYS`.
const CREATABLE_BAG_KEYS: [&str; 6] =
    ["MZ.InPoint", "MZ.OutPoint", "MZ.ZeroPoint", "MZ.WorkInPoint", "MZ.WorkOutPoint", "MZ.EditLine"];

impl Project {
    /// `Sequence.name` (empty when the element is absent).
    pub fn sequence_name(&self, sequence: SequenceId) -> String {
        let tree = &self.doc.tree;
        tree.child_by_tag(self.sequence(sequence).element, "Name").map(|e| tree[e].text_or_empty().to_string()).unwrap_or_default()
    }

    /// `Sequence.sequence_id`: the ObjectUID.
    pub fn sequence_id(&self, sequence: SequenceId) -> Option<String> {
        self.doc.tree.get(self.sequence(sequence).element, "ObjectUID").map(str::to_string)
    }

    /// `Sequence.timebase`: ticks per video frame.
    pub fn timebase(&self, sequence: SequenceId) -> Option<i64> {
        self.sequence(sequence).timebase
    }

    /// `Sequence.video_tracks`.
    pub fn video_tracks(&self, sequence: SequenceId) -> &[TrackId] {
        &self.sequence(sequence).video_tracks
    }

    /// `Sequence.audio_tracks`.
    pub fn audio_tracks(&self, sequence: SequenceId) -> &[TrackId] {
        &self.sequence(sequence).audio_tracks
    }

    /// `Sequence.markers`: the marker objects.
    pub fn sequence_markers(&self, sequence: SequenceId) -> &[NodeId] {
        &self.sequence(sequence).markers
    }

    /// `Sequence.clips`: video tracks first, then audio, each in track order.
    pub fn sequence_clips(&self, sequence: SequenceId) -> Vec<TrackItemId> {
        let data = self.sequence(sequence);
        data.video_tracks.iter().chain(&data.audio_tracks).flat_map(|&t| self.track(t).clips.iter().copied()).collect()
    }

    /// `Sequence.project_item`.
    pub fn sequence_item(&self, sequence: SequenceId) -> Option<super::ItemId> {
        self.item_by_sequence_uid(&self.sequence_id(sequence)?)
    }

    /// `Sequence.add_marker(name, start, comments, marker_type, duration)`: draws the marker ID
    /// first, then creates the collection and `MarkerOwner` when the sequence has none.
    pub fn add_sequence_marker(&mut self, sequence: SequenceId, spec: &MarkerSpec<'_>, uuids: &mut dyn UuidSource) -> Result<NodeId> {
        let marker = new_marker(&mut self.doc.tree, spec, uuids)?;
        let element = self.sequence(sequence).element;
        let inner = ensure_sequence_marker_list(&mut self.doc, element)?;
        attach_marker(&mut self.doc, inner, marker)?;
        self.sequence_mut(sequence).markers.push(marker);
        Ok(marker)
    }

    /// Python `find("Node/Properties/<key>")`: the first match across every `Node/Properties`.
    fn bag_element(&self, sequence: SequenceId, key: &str) -> Option<NodeId> {
        let tree = &self.doc.tree;
        tree.findall(self.sequence(sequence).element, "Node/Properties")
            .ok()?
            .into_iter()
            .find_map(|properties| tree.child_by_tag(properties, key))
    }

    fn bag_int(&self, sequence: SequenceId, key: &str) -> Result<Option<i64>> {
        match self.bag_element(sequence, key).map(|e| self.doc.tree[e].text_or_empty()) {
            Some(t) if !t.is_empty() => Ok(Some(py_int(t)?)),
            _ => Ok(None),
        }
    }

    /// `Sequence._bag_write`: update a property-bag key, creating a creatable one in
    /// Premiere's stored order.
    pub fn bag_write(&mut self, sequence: SequenceId, key: &str, text: &str) -> Result<()> {
        if let Some(element) = self.bag_element(sequence, key) {
            self.doc.tree.set_text(element, Some(text));
            return Ok(());
        }
        if !CREATABLE_BAG_KEYS.contains(&key) {
            return value(format!("sequence has no {key} property"));
        }
        let element = self.sequence(sequence).element;
        let tree = &mut self.doc.tree;
        let Some(properties) = tree.find1(element, "Node/Properties") else { return value("sequence has no property bag") };
        let position = BAG_ORDER.iter().position(|k| *k == key).expect("creatable keys are ordered");
        let anchor = BAG_ORDER[position + 1..].iter().find(|tag| tree.child_by_tag(properties, tag).is_some());
        match anchor {
            Some(anchor) => {
                insert_leaf_before(tree, properties, anchor, key, text)?;
            }
            None => {
                append_leaf(tree, properties, key, text);
            }
        }
        Ok(())
    }

    /// `Sequence.zero_point`: the starting timecode.
    pub fn zero_point(&self, sequence: SequenceId) -> Result<Time> {
        Ok(Time::new(self.bag_int(sequence, "MZ.ZeroPoint")?.unwrap_or(0)))
    }

    /// `Sequence.zero_point = value`.
    pub fn set_zero_point(&mut self, sequence: SequenceId, value: Time) -> Result<()> {
        self.bag_write(sequence, "MZ.ZeroPoint", &value.ticks.to_string())
    }

    /// `Sequence.in_point = value` (`None` writes the unset sentinel).
    pub fn set_sequence_in_point(&mut self, sequence: SequenceId, value: Option<Time>) -> Result<()> {
        self.bag_write(sequence, "MZ.InPoint", &value.map_or(UNSET_TICKS, |t| t.ticks).to_string())
    }

    /// `Sequence.out_point = value` (`None` writes the unset sentinel).
    pub fn set_sequence_out_point(&mut self, sequence: SequenceId, value: Option<Time>) -> Result<()> {
        self.bag_write(sequence, "MZ.OutPoint", &value.map_or(UNSET_TICKS, |t| t.ticks).to_string())
    }

    /// `Sequence.end`: the latest track-item end.
    pub fn sequence_end(&self, sequence: SequenceId) -> Result<Time> {
        let mut ticks = 0;
        for item in self.sequence_clips(sequence) {
            ticks = ticks.max(self.end(item)?.ticks);
        }
        Ok(Time::new(ticks))
    }

    /// `Sequence._link(items)`: a `Link` object over the track items, referenced from
    /// `PersistentGroupContainer/LinkContainer/Links`.
    pub fn link(&mut self, sequence: SequenceId, items: &[TrackItemId]) -> Result<String> {
        let ids: Vec<String> = items
            .iter()
            .map(|&i| self.doc.tree.get(self.track_item(i).element, "ObjectID").unwrap_or("").to_string())
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let link = build_link(&mut self.doc.tree, &refs);
        let link_id = self.doc.add_object(link)?;
        let element = self.sequence(sequence).element;
        let tree = &mut self.doc.tree;
        let Some(container) = tree.find1(element, "PersistentGroupContainer/LinkContainer") else {
            return value("sequence has no LinkContainer");
        };
        let links = match tree.child_by_tag(container, "Links") {
            Some(l) => l,
            None => {
                let l = tree.new_element_with("Links", &[("Version", "1")]);
                append_child(tree, container, l, None)
            }
        };
        let index = tree.children(links).iter().filter(|&&c| tree.tag(c) == "Link").count().to_string();
        let entry = tree.new_element_with("Link", &[("Index", &index), ("ObjectRef", &link_id)]);
        append_child(tree, links, entry, Some("\n\t\t\t\t"));
        Ok(link_id)
    }

    /// `Sequence._grow_source_duration`: raise the `OriginalDuration` of this sequence's
    /// `*SequenceSource` objects to cover its content (never lowers it).
    pub fn grow_source_duration(&mut self, sequence: SequenceId) -> Result<()> {
        let content = self.sequence_end(sequence)?.ticks;
        self.grow_source_duration_to(sequence, content)
    }
    /// Embedded captions are data-track items and are not in `sequence_clips`.
    /// Grow their backing sequence sources without inventing a picture edit.
    pub(crate) fn grow_source_duration_to(&mut self, sequence: SequenceId, content: i64) -> Result<()> {
        if content == 0 {
            return Ok(());
        }
        let uid = self.sequence_id(sequence);
        let tree = &mut self.doc.tree;
        for element in tree.children(self.doc.root).to_vec() {
            if !tree.tag(element).ends_with("SequenceSource") {
                continue;
            }
            let Some(reference) = tree.find1(element, "SequenceSource/Sequence") else { continue };
            if tree.get(reference, "ObjectURef") != uid.as_deref() {
                continue;
            }
            let Some(duration) = tree.child_by_tag(element, "OriginalDuration") else { continue };
            let text = tree[duration].text_or_empty();
            let current = if text.is_empty() { 0 } else { py_int(text)? };
            if current < content {
                tree.set_text(duration, Some(content.to_string()));
            }
        }
        Ok(())
    }
}
