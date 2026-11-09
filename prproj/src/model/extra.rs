//! Smaller py_premiere operations the converter calls directly (Phase 4 API):
//! `ProjectItem._media_element`, `ProjectItem._share_file_identity`,
//! `Project._narrow_merged_channel`, `Track.name`, the refile step of the converter's
//! `combined._move_to_processed_clips` and the track sort of `writer._order_track_items`.

use super::item::{add_item_ref, child_items, detach_item_ref, item_container};
use super::project::py_int;
use super::{ItemId, Project, TrackId, TrackItemId};
use crate::build::media::MONO_LAYOUT;
use crate::error::{value, Result};
use crate::xml::mutations::{append_leaf, remove_child};
use crate::xml::NodeId;

impl Project {
    /// `ProjectItem._media_element`: the `Media` object behind a clip item (master clip ->
    /// template clip -> Source -> MediaSource/Media).
    pub fn media_element(&self, item: ItemId) -> Result<Option<NodeId>> {
        let Some(core) = self.own_clip_core(item)? else { return Ok(None) };
        let Some(source_ref) = self.doc.tree.child_by_tag(core, "Source") else { return Ok(None) };
        let source = self.doc.resolve(source_ref)?;
        let Some(media_ref) = self.doc.tree.find1(source, "MediaSource/Media") else { return Ok(None) };
        Ok(Some(self.doc.resolve(media_ref)?))
    }

    /// `ProjectItem._share_file_identity(other)` called on `item`: give `other`'s Media the
    /// FileKey and content state of `item`'s, turn its ModificationState into a hash reference
    /// to `item`'s blob, and stamp the content state on `other`'s marker collection.
    pub fn share_file_identity(&mut self, item: ItemId, other: ItemId) -> Result<()> {
        let (Some(source_media), Some(target_media)) = (self.media_element(item)?, self.media_element(other)?) else {
            return Ok(());
        };
        let tree = &mut self.doc.tree;
        for tag in ["FileKey", "ContentAndMetadataState"] {
            let stored = tree.child_by_tag(source_media, tag).map(|e| tree[e].text_or_empty().to_string());
            let leaf = tree.child_by_tag(target_media, tag);
            if let (Some(stored), Some(leaf)) = (stored.filter(|s| !s.is_empty()), leaf) {
                tree.set_text(leaf, Some(stored));
            }
        }
        let source_state = tree.child_by_tag(source_media, "ModificationState");
        let target_state = tree.child_by_tag(target_media, "ModificationState");
        if let (Some(source_state), Some(target_state)) = (source_state, target_state) {
            if let Some(hash) = tree.get(source_state, "BinaryHash").filter(|h| !h.is_empty()).map(str::to_string) {
                tree.set(target_state, "BinaryHash", hash);
            }
            tree.set_text(target_state, None::<String>);
        }
        let content_state = tree
            .child_by_tag(source_media, "ContentAndMetadataState")
            .map(|e| tree[e].text_or_empty().to_string())
            .filter(|s| !s.is_empty());
        let core = self.own_clip_core(other)?;
        let markers_ref = core.and_then(|c| self.doc.tree.find1(c, "MarkerOwner/Markers"));
        if let (Some(content_state), Some(markers_ref)) = (content_state, markers_ref) {
            let markers = self.doc.resolve(markers_ref)?;
            if let Some(last) = self.doc.tree.child_by_tag(markers, "LastContentState") {
                self.doc.tree.set_text(last, Some(content_state));
            }
        }
        // Like Python, the payload cache is not dropped here (call
        // `doc.invalidate_payloads()` before reading payloads of these Media objects).
        Ok(())
    }

    /// `Project._narrow_merged_channel(placed, channel)`: a merged placement plays one mono
    /// channel: mono layout, only that channel's SecondaryContent (the others removed), the
    /// bag-less audio chain, and past channel 0 a `SecondaryIndex` plus the SubClip's
    /// `OrigChGrp`.
    pub fn narrow_merged_channel(&mut self, placed: TrackItemId, channel: i64) -> Result<()> {
        let data = self.track_item(placed).clone();
        let clip = data.clip;
        if let Some(layout) = self.doc.tree.child_by_tag(clip, "AudioChannelLayout") {
            self.doc.tree.set_text(layout, Some(MONO_LAYOUT));
        }
        if let Some(contents) = self.doc.tree.child_by_tag(clip, "SecondaryContents") {
            let mut kept = None;
            let entries: Vec<NodeId> =
                self.doc.tree.children(contents).iter().copied().filter(|&c| self.doc.tree.tag(c) == "SecondaryContentItem").collect();
            for entry in entries {
                let secondary = self.doc.resolve(entry)?;
                let index_text = match self.doc.tree.findtext(secondary, "ChannelIndex")? {
                    Some(t) if !t.is_empty() => t,
                    _ => "0".to_string(),
                };
                if kept.is_none() && py_int(&index_text)? == channel {
                    kept = Some(entry);
                } else {
                    self.doc.remove_object(secondary)?;
                    remove_child(&mut self.doc.tree, contents, entry)?;
                }
            }
            if let Some(kept) = kept {
                self.doc.tree.set(kept, "Index", "0");
            }
        }
        if let Some(chain_ref) = self.doc.tree.find1(data.element, "ClipTrackItem/ComponentOwner/Components") {
            let chain = self.doc.resolve(chain_ref)?;
            let tree = &mut self.doc.tree;
            if let Some(inner) = tree.child_by_tag(chain, "ComponentChain") {
                if let Some(node) = tree.child_by_tag(inner, "Node") {
                    // ET `remove` (no whitespace repair), then the empty-chain layout.
                    tree.remove(inner, node);
                    tree.set_text(inner, Some("\n\t\t"));
                }
            }
        }
        if channel != 0 {
            append_leaf(&mut self.doc.tree, clip, "SecondaryIndex", &channel.to_string());
            if let Some(orig) = self.doc.tree.child_by_tag(data.subclip, "OrigChGrp") {
                self.doc.tree.set_text(orig, Some(channel.to_string()));
            }
        }
        Ok(())
    }

    /// `Track.name`: `Video 1`, `Audio 2`, ...
    pub fn track_name(&self, track: TrackId) -> String {
        let data = self.track(track);
        format!("{} {}", data.media_type.as_str(), data.index + 1)
    }

    /// The per-track step of `writer._order_track_items`: list the track's clips in start
    /// order (stable), keeping the `TrackItems` references (Index, tails) and the clip list in
    /// step. Does nothing when already in order.
    pub fn sort_track_clips_by_start(&mut self, track: TrackId) -> Result<()> {
        let element = self.track(track).element;
        let Some(items) = self.doc.tree.find1(element, "ClipTrack/ClipItems/TrackItems") else { return Ok(()) };
        let refs: Vec<NodeId> =
            self.doc.tree.children(items).iter().copied().filter(|&c| self.doc.tree.tag(c) == "TrackItem").collect();
        let mut keyed = Vec::with_capacity(refs.len());
        for &reference in &refs {
            let object_id = self.doc.tree.get(reference, "ObjectRef").unwrap_or("");
            let Some(&object) = self.doc.by_object_id.get(object_id) else {
                return value(format!("unknown ObjectID '{object_id}'"));
            };
            let start = match self.doc.tree.findtext(object, ".//ClipTrackItem/TrackItem/Start")? {
                Some(t) if !t.is_empty() => py_int(&t)?,
                _ => 0,
            };
            keyed.push((start, reference));
        }
        keyed.sort_by_key(|&(start, _)| start);
        let ordered: Vec<NodeId> = keyed.into_iter().map(|(_, r)| r).collect();
        if ordered == refs {
            return Ok(());
        }
        let tree = &mut self.doc.tree;
        let tails: Vec<Option<String>> = refs.iter().map(|&r| tree.tail(r).map(str::to_string)).collect();
        for &r in &refs {
            tree.remove(items, r);
        }
        for (index, (r, tail)) in ordered.into_iter().zip(tails).enumerate() {
            tree.set(r, "Index", index.to_string());
            tree.set_tail(r, tail);
            tree.append(items, r);
        }
        let mut clips = Vec::new();
        for &clip in &self.track(track).clips {
            clips.push((self.start(clip)?.ticks, clip));
        }
        clips.sort_by_key(|&(start, _)| start);
        self.track_mut(track).clips = clips.into_iter().map(|(_, c)| c).collect();
        Ok(())
    }

    /// The refile step of the converter's `combined._move_to_processed_clips`. Unlike
    /// [`move_item`](Self::move_item) it uses the lenient `item_container` lookups, detaches
    /// from the old parent BEFORE synthesizing the destination's `Items` list, and makes no
    /// ancestry checks. An item already in `bin` is left alone.
    pub fn refile_item(&mut self, item: ItemId, bin: ItemId) -> Result<()> {
        let Some(bin_container) = item_container(&self.doc.tree, self.item(bin).element) else {
            return value("bin has no ProjectItemContainer");
        };
        let Some(parent) = self.item(item).parent else { return value("item has no parent to move from") };
        if parent == bin {
            return Ok(());
        }
        let uid = self.doc.tree.get(self.item(item).element, "ObjectUID").unwrap_or("").to_string();
        let Some(source) = item_container(&self.doc.tree, self.item(parent).element) else {
            return value("parent has no ProjectItemContainer");
        };
        detach_item_ref(&mut self.doc.tree, source, &uid)?;
        self.item_mut(parent).children.retain(|&c| c != item);
        let items = child_items(&mut self.doc.tree, bin_container);
        add_item_ref(&mut self.doc.tree, items, &uid);
        self.item_mut(item).parent = Some(bin);
        self.item_mut(bin).children.push(item);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::model::project::{py_int, py_path};
    use crate::model::track_item::{py_div_trunc, py_round};

    #[test]
    fn int_follows_python() {
        assert_eq!(py_int(" 1_000 ").unwrap(), 1000);
        assert_eq!(py_int("-007").unwrap(), -7);
        for bad in ["_1", "1_", "1__0", "", "+", "1.0", "0x10"] {
            assert!(py_int(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn path_follows_python() {
        for (raw, want) in
            [("./x", "x"), ("a//b", "a/b"), ("a/./b/", "a/b"), ("", "."), ("//a", "//a"), ("///a", "/a"), ("a/../b", "a/../b")]
        {
            assert_eq!(py_path(raw).to_string_lossy(), want);
        }
    }

    #[test]
    fn speed_division_raises_like_python() {
        assert_eq!(py_div_trunc(7, 2.0).unwrap(), 3);
        assert_eq!(py_div_trunc(-7, 2.0).unwrap(), -3);
        assert_eq!(py_div_trunc(7, 0.0).unwrap_err().to_string(), "float division by zero");
        assert!(py_div_trunc(7, f64::NAN).is_err());
        assert_eq!(py_round(2.5).unwrap(), 2);
        assert!(py_round(f64::INFINITY).is_err());
    }
}
