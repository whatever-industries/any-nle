//! The rest of `ProjectItem` (port of `py_premiere/models/project_item.py`): type, node ID,
//! tree path, in/out, media queries (offline, generators, adjustment layer, multicam, merged,
//! Motion Graphics template), start time, proxies, subclip boundaries, footage interpretation,
//! scale-to-frame, children as a `NamedList`, `remove_bin`, `remove_item` and the repr.
//!
//! Not ported here: `attach_proxy` and `create_sub_clip`, which synthesize new media graphs
//! through `Project.import_files` (media probing; see the README).

use super::enums::{GeneratorType, ProjectItemType};
use super::footage::FootageInterpretation;
use super::item::{clip_core, detach_item_ref};
use super::named_list::NamedList;
use super::project::py_int;
use super::{ItemId, ItemType, Project};
use crate::build::time::{Time, UNSET_TICKS};
use crate::convert::model::py_str_repr;
use crate::error::{value, Error, Result};
use crate::xml::mutations::set_elided_flag;
use crate::xml::{NodeId, ReferenceIndex};

fn int_or_zero(text: Option<String>) -> Result<i64> {
    match text {
        Some(t) if !t.is_empty() => py_int(&t),
        _ => Ok(0),
    }
}

/// Python `format(n, "08x")`.
fn hex8(n: i64) -> String {
    if n < 0 {
        format!("-{:07x}", n.unsigned_abs())
    } else {
        format!("{n:08x}")
    }
}

impl Project {
    /// `ProjectItem.type`.
    pub fn item_type(&self, item: ItemId) -> ProjectItemType {
        self.item(item).item_type.as_enum()
    }

    /// `ProjectItem.node_id`: the load-time node ID in 8-digit hex (`None` for items created
    /// in this session).
    pub fn node_id(&self, item: ItemId) -> Option<String> {
        self.item(item).node_id.map(hex8)
    }

    /// `ProjectItem.tree_path`: `\project\bin\item`.
    pub fn tree_path(&self, item: ItemId) -> String {
        match self.item(item).parent {
            None => format!("\\{}", self.name()),
            Some(parent) => format!("{}\\{}", self.tree_path(parent), self.item_name(item)),
        }
    }

    /// `ProjectItem.children` as a `NamedList` (by name).
    pub fn item_children(&self, item: ItemId) -> NamedList<ItemId> {
        let items = self.item(item).children.clone();
        let keys = items.iter().map(|&c| vec![Ok(self.item_name(c))]).collect();
        NamedList::new(items, keys)
    }

    /// `ProjectItem.walk()`: every descendant, depth-first (the item itself excluded).
    pub fn walk(&self, item: ItemId) -> Vec<ItemId> {
        let mut out = Vec::new();
        let mut stack: Vec<ItemId> = self.item(item).children.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            out.push(i);
            stack.extend(self.item(i).children.iter().rev().copied());
        }
        out
    }

    /// `ProjectItem.media_path`.
    pub fn item_media_path(&self, item: ItemId) -> Option<&str> {
        self.item(item).media_path.as_deref()
    }

    fn point_ticks(&self, item: ItemId, tag: &str) -> Result<i64> {
        let data = self.item(item);
        let Some(&first) = data.clip_elements.first() else { return Ok(UNSET_TICKS) };
        let stored = match clip_core(&self.doc.tree, first) {
            Some(core) => self.doc.tree.findtext(core, tag)?,
            None => None,
        };
        if let Some(s) = stored.filter(|s| !s.is_empty()) {
            return py_int(&s);
        }
        if tag == "OutPoint" {
            if let Some(uid) = &data.sequence_uid {
                for sequence in self.sequences() {
                    if self.sequence_id(sequence).as_deref() == Some(uid.as_str()) {
                        return Ok(self.sequence_end(sequence)?.ticks);
                    }
                }
            }
            if let Some(ticks) = data.default_out_ticks {
                return Ok(ticks);
            }
        }
        Ok(0)
    }

    /// `ProjectItem.in_point` (`None` for items without media).
    pub fn item_in_point(&self, item: ItemId) -> Result<Option<Time>> {
        let ticks = self.point_ticks(item, "InPoint")?;
        Ok((ticks != UNSET_TICKS).then_some(Time::new(ticks)))
    }

    /// `ProjectItem.out_point` (a sequence item reports the live sequence end).
    pub fn item_out_point(&self, item: ItemId) -> Result<Option<Time>> {
        let ticks = self.point_ticks(item, "OutPoint")?;
        Ok((ticks != UNSET_TICKS).then_some(Time::new(ticks)))
    }

    /// `ProjectItem._media_source`: the `*MediaSource` object the template clip plays from.
    pub fn media_source(&self, item: ItemId) -> Result<Option<NodeId>> {
        let Some(core) = self.own_clip_core(item)? else { return Ok(None) };
        let Some(source_ref) = self.doc.tree.child_by_tag(core, "Source") else { return Ok(None) };
        self.doc.resolve(source_ref).map(Some)
    }

    /// `ProjectItem._media_content`: the media source's `Content` bag.
    pub fn media_content(&self, item: ItemId) -> Result<Option<NodeId>> {
        Ok(self.media_source(item)?.and_then(|s| self.doc.tree.find1(s, "MediaSource/Content")))
    }

    /// `ProjectItem.footage_interpretation` (`None` without a video stream).
    pub fn footage_interpretation(&self, item: ItemId) -> Result<Option<FootageInterpretation>> {
        let Some(media) = self.media_element(item)? else { return Ok(None) };
        let Some(stream_ref) = self.doc.tree.child_by_tag(media, "VideoStream") else { return Ok(None) };
        let source = self.media_source(item)?;
        Ok(Some(FootageInterpretation { stream: self.doc.resolve(stream_ref)?, source }))
    }

    /// `ProjectItem.scale_to_frame_size`: `ScaleToFramePolicy` 1 on the first video clip.
    pub fn scale_to_frame_size(&self, item: ItemId) -> Result<bool> {
        let tree = &self.doc.tree;
        for &clip in &self.item(item).clip_elements {
            if tree.tag(clip) == "VideoClip" {
                return Ok(tree.findtext(clip, "ScaleToFramePolicy")?.as_deref() == Some("1"));
            }
        }
        Ok(false)
    }

    /// `ProjectItem.scale_to_frame_size = value` (elided when off).
    pub fn set_scale_to_frame_size(&mut self, item: ItemId, scale: bool) -> Result<()> {
        let clips = self.item(item).clip_elements.clone();
        for clip in clips {
            if self.doc.tree.tag(clip) == "VideoClip" {
                return set_elided_flag(&mut self.doc.tree, clip, "ScaleToFramePolicy", scale, "1");
            }
        }
        value("item has no video clip to scale")
    }

    /// `ProjectItem.is_offline`: `Media/OfflineReason` present.
    pub fn is_offline(&self, item: ItemId) -> Result<bool> {
        Ok(self.media_element(item)?.is_some_and(|m| self.doc.tree.child_by_tag(m, "OfflineReason").is_some()))
    }

    fn generator_code(&self, item: ItemId) -> Result<Option<String>> {
        let Some(media) = self.media_element(item)? else { return Ok(None) };
        let stored = self.doc.tree.findtext(media, "FilePath")?.unwrap_or_default();
        Ok((!stored.is_empty() && stored.chars().all(|c| c.is_ascii_digit())).then_some(stored))
    }

    /// `ProjectItem.generator_id`: the fourcc of synthetic media (`None` for real files).
    pub fn generator_id(&self, item: ItemId) -> Result<Option<String>> {
        let Some(stored) = self.generator_code(item)? else { return Ok(None) };
        let code = py_int(&stored)
            .ok()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| Error::Struct("'I' format requires 0 <= number <= 4294967295".into()))?;
        Ok(Some(code.to_be_bytes().iter().map(|&b| if b < 128 { b as char } else { '\u{fffd}' }).collect()))
    }

    /// `ProjectItem.generator_type` (`None` for real media or an unknown generator).
    pub fn generator_type(&self, item: ItemId) -> Result<Option<GeneratorType>> {
        let Some(stored) = self.generator_code(item)? else { return Ok(None) };
        Ok(py_int(&stored).ok().and_then(|v| GeneratorType::from_value(v).ok()))
    }

    /// `ProjectItem.is_adjustment_layer`.
    pub fn is_adjustment_layer(&self, item: ItemId) -> Result<bool> {
        let Some(master) = self.item(item).master else { return Ok(false) };
        Ok(self.doc.tree.findtext(master, "IsAdjustmentLayer")?.as_deref() == Some("true"))
    }

    /// `ProjectItem.is_multicam_clip`.
    pub fn is_multicam_clip(&self, item: ItemId) -> Result<bool> {
        let Some(master) = self.item(item).master else { return Ok(false) };
        Ok(self.doc.tree.findtext(master, "Node/Properties/Source.Monitor.Multicam.Enabled")?.as_deref() == Some("true"))
    }

    /// `ProjectItem.is_mgt`: the master carries a `BlueprintVideoComponentChain`.
    pub fn is_mgt(&self, item: ItemId) -> bool {
        self.item(item).master.is_some_and(|m| self.doc.tree.child_by_tag(m, "BlueprintVideoComponentChain").is_some())
    }

    /// `ProjectItem._backing_sequence`: the `Sequence` object the item's clips play from.
    pub fn backing_sequence(&self, item: ItemId) -> Result<Option<NodeId>> {
        let Some(source) = self.media_source(item)? else { return Ok(None) };
        let Some(reference) = self.doc.tree.find1(source, "SequenceSource/Sequence") else { return Ok(None) };
        self.doc.resolve(reference).map(Some)
    }

    /// `ProjectItem.is_merged_clip`: the backing sequence flags `BE.Sequence.IsMergedClip`.
    pub fn is_merged_clip(&self, item: ItemId) -> Result<bool> {
        let Some(sequence) = self.backing_sequence(item)? else { return Ok(false) };
        Ok(self.doc.tree.findtext(sequence, "Node/Properties/BE.Sequence.IsMergedClip")?.as_deref() == Some("true"))
    }

    /// `ProjectItem.start_time`: `Media/AlternateStart` while `UseAlternateStart` is set.
    pub fn item_start_time(&self, item: ItemId) -> Result<Time> {
        let Some(media) = self.media_element(item)? else { return Ok(Time::new(0)) };
        if self.doc.tree.findtext(media, "UseAlternateStart")?.as_deref() != Some("true") {
            return Ok(Time::new(0));
        }
        Ok(Time::new(int_or_zero(self.doc.tree.findtext(media, "AlternateStart")?)?))
    }

    /// `ProjectItem.start_time = value` (media that already stores a start timecode only).
    pub fn set_item_start_time(&mut self, item: ItemId, start: Time) -> Result<()> {
        if start.ticks < 0 {
            return value("start time cannot be negative");
        }
        let media = self.media_element(item)?;
        let tree = &mut self.doc.tree;
        let alternate = media.and_then(|m| tree.child_by_tag(m, "AlternateStart"));
        let flag = media.and_then(|m| tree.child_by_tag(m, "UseAlternateStart"));
        let (Some(alternate), Some(flag)) = (alternate, flag) else {
            return value("start time is only settable on media that stores a start timecode (Media/AlternateStart)");
        };
        tree.set_text(alternate, Some(start.ticks.to_string()));
        tree.set_text(flag, Some("true"));
        Ok(())
    }

    fn proxy_media(&self, item: ItemId) -> Result<Option<NodeId>> {
        let Some(content) = self.media_content(item)? else { return Ok(None) };
        let Some(reference) = self.doc.tree.child_by_tag(content, "ProxyMedia") else { return Ok(None) };
        self.doc.resolve(reference).map(Some)
    }

    /// `ProjectItem.has_proxy`.
    pub fn has_proxy(&self, item: ItemId) -> Result<bool> {
        Ok(self.proxy_media(item)?.is_some())
    }

    /// `ProjectItem.proxy_path`.
    pub fn proxy_path(&self, item: ItemId) -> Result<Option<String>> {
        let Some(media) = self.proxy_media(item)? else { return Ok(None) };
        let stored = self.doc.tree.findtext(media, "FilePath")?.unwrap_or_default();
        Ok((!stored.is_empty()).then(|| super::project::py_path(&stored).to_string_lossy().into_owned()))
    }

    fn boundary(&self, item: ItemId, tag: &str) -> Result<Option<Time>> {
        let Some(content) = self.media_content(item)? else { return Ok(None) };
        match self.doc.tree.findtext(content, tag)? {
            None => Ok(None),
            Some(s) => Ok(Some(Time::new(py_int(&s)?))),
        }
    }

    /// `ProjectItem.is_subclip`.
    pub fn is_subclip(&self, item: ItemId) -> Result<bool> {
        Ok(self.boundary(item, "StartBoundary")?.is_some())
    }

    /// `ProjectItem.subclip_in_point`.
    pub fn subclip_in_point(&self, item: ItemId) -> Result<Option<Time>> {
        self.boundary(item, "StartBoundary")
    }

    /// `ProjectItem.subclip_out_point`.
    pub fn subclip_out_point(&self, item: ItemId) -> Result<Option<Time>> {
        self.boundary(item, "EndBoundary")
    }

    /// `ProjectItem.has_hard_boundaries`.
    pub fn has_hard_boundaries(&self, item: ItemId) -> Result<bool> {
        let Some(content) = self.media_content(item)? else { return Ok(false) };
        Ok(self.doc.tree.findtext(content, "BoundariesAreHard")?.as_deref() == Some("true"))
    }

    /// `repr(project_item)`.
    pub fn item_repr(&self, item: ItemId) -> String {
        format!("ProjectItem(name={}, type={})", py_str_repr(&self.item_name(item)), self.item_type(item).name())
    }

    /// `ProjectItem.remove_bin(child)`: remove a child bin and (depth-first) its contents.
    pub fn remove_bin(&mut self, parent: ItemId, child: ItemId) -> Result<()> {
        if !self.item(parent).children.contains(&child) {
            return value("item is not a child of this bin");
        }
        if self.item(child).item_type != ItemType::Bin {
            return value("only bins can be removed");
        }
        for grandchild in self.item(child).children.clone() {
            if self.item(grandchild).item_type == ItemType::Bin {
                self.remove_bin(child, grandchild)?;
            } else {
                self.remove_item(child, grandchild)?;
            }
        }
        let element = self.item(child).element;
        let uid = self.doc.tree.get(element, "ObjectUID").map(str::to_string);
        let container = self.doc.tree.child_by_tag(self.item(parent).element, "ProjectItemContainer");
        if let (Some(container), Some(uid)) = (container, uid) {
            detach_item_ref(&mut self.doc.tree, container, &uid)?;
        }
        self.doc.remove_object(element)?;
        self.item_mut(parent).children.retain(|&c| c != child);
        Ok(())
    }

    /// `ProjectItem.remove_item(child)`: remove a clip item and the graph it exclusively owns;
    /// refused while it is placed on a timeline or backs a sequence.
    pub fn remove_item(&mut self, parent: ItemId, child: ItemId) -> Result<()> {
        if !self.item(parent).children.contains(&child) {
            return value("item is not a child of this bin");
        }
        if self.item(child).item_type != ItemType::Clip {
            return value("only clip items can be removed; use remove_bin");
        }
        if self.is_sequence_item(child) {
            return value("removing a sequence item is not supported");
        }
        let Some(master) = self.item(child).master else { return value("item has no master clip") };
        let element = self.item(child).element;
        let index = ReferenceIndex::new(&self.doc);
        if !index.referrers_outside(master, &[element, master]).is_empty() {
            return value("item is in use on a timeline; remove its clips first");
        }
        let uid = self.doc.tree.get(element, "ObjectUID").map(str::to_string);
        let container = self.doc.tree.child_by_tag(self.item(parent).element, "ProjectItemContainer");
        if let (Some(container), Some(uid)) = (container, uid) {
            detach_item_ref(&mut self.doc.tree, container, &uid)?;
        }
        for owned in self.doc.owned_objects(&[element, master], Some(&index)) {
            self.doc.remove_object(owned)?;
        }
        self.item_mut(parent).children.retain(|&c| c != child);
        Ok(())
    }
}
