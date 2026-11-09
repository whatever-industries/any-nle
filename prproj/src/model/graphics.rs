//! Entry points onto the mask, graphic and Motion Graphics template builders (the thin
//! methods of `models/track_item.py`, `models/component.py`, `models/track.py`,
//! `models/sequence.py` and `models/project.py` that call `mask_builder`, `graphic_builder`
//! and `mgt_builder`).
//!
//! | Python | Rust |
//! |---|---|
//! | `TrackItem.add_mask` | [`Project::add_clip_mask`] |
//! | `Component.add_mask` | [`Project::add_component_mask`] |
//! | `Track.add_graphic` | [`Project::add_graphic`] |
//! | `Track.add_mgt`, `Sequence.import_mgt` | [`Project::add_mgt`], [`Project::import_mgt`] |
//! | `Project._add_template_item` | [`Project::add_template_item`] |
//! | `Track.remove_clip`, `_clips_within` (used by `add_mgt`'s overwrite) | [`Project::remove_clip`], [`Project::clips_within`] |
//!
//! Python also wraps what these create in `Component`/`ComponentParam` model objects
//! (`_wrap_mask`, `_build_component_model`); this crate has no component model, so the results
//! name the created objects (`ObjectID`s and elements) for a caller that builds one.

use std::path::Path;

use super::item::{add_item_ref, child_items, item_container, validate_name};
use super::project::resolve_path;
use super::track::{new_subclip_element, new_video_track_item_element, place_on_timeline};
use super::{ItemData, ItemId, ItemType, MediaType, Project, SequenceId, TrackId, TrackItemId};
use crate::build::graphic::{
    build_empty_channel_groups, build_graphic_chain, build_graphic_clip, build_graphic_logging, build_graphic_master,
    build_graphic_media, build_graphic_source, build_graphic_stream, build_source_text_param, build_text_component,
    build_text_params, validate_vector2, GRAPHIC_BASE_TICKS,
};
use crate::build::mask::{
    attach_sub_mask, build_mask_component, build_mask_params, build_selection_chain, stamp_next_component_number,
};
use crate::build::mgt::{
    attach_component, build_blueprint_chain, build_capsule_component, build_capsule_params, build_media_bin,
    build_mgt_clip, build_mgt_logging, build_mgt_master, build_mgt_media, build_mgt_source, build_mgt_stream,
    build_template_item, Stored, Template, MASTER_SLOT, MEDIA_FOLDER, PLACEMENT_SLOT,
};
use crate::build::time::{Time, TICKS_PER_SECOND};
use crate::convert::model::py_str_repr;
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::NodeId;

/// `_GRAPHIC_DEFAULT_TICKS`: a fresh graphic runs for Premiere's still default (5 s).
const GRAPHIC_DEFAULT_TICKS: i64 = 5 * TICKS_PER_SECOND;

/// `_MGT_IDENTIFIER`: keys a placement back to the template it was imported from.
const MGT_IDENTIFIER: &str = "BE.RushInspectorPanel.MogrtIdentifier";

/// What a mask entry point created.
#[derive(Clone, Debug)]
pub struct MaskAdded {
    /// The mask component's ObjectID.
    pub component_id: String,
    /// The mask `VideoFilterComponent` element.
    pub component: NodeId,
    /// ObjectIDs of its 27 parameters, in order.
    pub param_ids: Vec<String>,
}

/// What [`Project::add_graphic`] created.
#[derive(Clone, Debug)]
pub struct GraphicAdded {
    /// The placed clip.
    pub item: TrackItemId,
    /// The `AE.ADBE Text` component's ObjectID.
    pub component_id: String,
    /// ObjectIDs of its 22 parameters (`Source Text` first).
    pub param_ids: Vec<String>,
}

/// What [`Project::add_mgt`] created.
#[derive(Clone, Debug)]
pub struct MgtAdded {
    /// The placed clip.
    pub item: TrackItemId,
    /// The placement's `AE.ADBE Capsule` component's ObjectID.
    pub component_id: String,
    /// ObjectIDs of its controls.
    pub param_ids: Vec<String>,
}

/// Python `a % b` for a positive modulus.
fn py_mod(a: i64, b: i64) -> i64 {
    ((a % b) + b) % b
}

impl Project {
    /// `TrackItem.add_mask`: apply a default mask to the clip as a whole.
    ///
    /// A clip-level mask lives in its own chain, referenced from the track item as
    /// `SelectionComponents` (26_effect_mask); further masks hang off that first intrinsic
    /// holder as effect-role sub-components numbered `01`, `02`, ..., bumping its
    /// `NextComponentNumber` counter (76_two_masks).
    pub fn add_clip_mask(&mut self, item: TrackItemId) -> Result<MaskAdded> {
        let element = self.track_item(item).element;
        let chain_ref = self.doc.tree.child_by_tag(element, "SelectionComponents");
        if let Some(chain_ref) = chain_ref {
            let chain = self.doc.resolve(chain_ref)?;
            let holder_ref = self.doc.tree.find1(chain, "ComponentChain/Components/Component");
            let holder = match holder_ref {
                Some(reference) => Some(self.doc.resolve(reference)?),
                None => None,
            };
            let Some(holder) = holder else { return value("clip mask chain has no holder component") };
            let count = match self.doc.tree.child_by_tag(holder, "SubComponents") {
                None => 0,
                Some(subs) => self.doc.tree.children(subs).iter().filter(|&&c| self.doc.tree.tag(c) == "SubComponent").count(),
            };
            let param_ids = build_mask_params(&mut self.doc)?;
            let (mask_id, mask_element) =
                build_mask_component(&mut self.doc, &param_ids, false, &format!("{:02}", count + 1))?;
            attach_sub_mask(&mut self.doc.tree, holder, &mask_id)?;
            // The intrinsic holder's counter covers itself plus its subs: 2 alone, 3 after the
            // second mask.
            let inner = self.doc.tree.child_by_tag(holder, "Component");
            stamp_next_component_number(&mut self.doc.tree, inner, count as i64 + 3)?;
            return Ok(MaskAdded { component_id: mask_id, component: mask_element, param_ids });
        }
        let Some(anchor) = self.doc.tree.child_by_tag(element, "ClipTrackItem") else {
            return value("track item has no clip core to anchor the mask");
        };
        let param_ids = build_mask_params(&mut self.doc)?;
        let (mask_id, mask_element) = build_mask_component(&mut self.doc, &param_ids, true, "01")?;
        let chain_id = build_selection_chain(&mut self.doc, &mask_id)?;
        let tree = &mut self.doc.tree;
        let reference = tree.new_element_with("SelectionComponents", &[("ObjectRef", &chain_id)]);
        // The ref slots right after the ClipTrackItem base (26_effect_mask: ClipTrackItem,
        // SelectionComponents, PixelAspectRatio, ...).
        let tail = tree.tail(anchor).map(str::to_string);
        tree.set_tail(reference, tail);
        let index = tree.children(element).iter().position(|&c| c == anchor).expect("anchor is a child");
        tree.insert(element, index + 1, reference);
        Ok(MaskAdded { component_id: mask_id, component: mask_element, param_ids })
    }

    /// `Component.add_mask`: attach a default mask to an effect component (a top-level
    /// `VideoFilterComponent` element) as an `AEMask2` sub-component. Masks number themselves by
    /// position (`01`, `02`, ...), and from the second one on the effect carries a
    /// `NextComponentNumber` allocation counter (76_two_masks).
    pub fn add_component_mask(&mut self, component: NodeId) -> Result<MaskAdded> {
        if self.doc.tree.child_by_tag(component, "MatchName").is_none() {
            return value("component has no MatchName to anchor the mask");
        }
        let count = match self.doc.tree.child_by_tag(component, "SubComponents") {
            None => 0,
            Some(subs) => self.doc.tree.children(subs).iter().filter(|&&c| self.doc.tree.tag(c) == "SubComponent").count(),
        };
        let param_ids = build_mask_params(&mut self.doc)?;
        let (mask_id, mask_element) =
            build_mask_component(&mut self.doc, &param_ids, false, &format!("{:02}", count + 1))?;
        attach_sub_mask(&mut self.doc.tree, component, &mask_id)?;
        if count > 0 {
            // `Component._inner`: audio components nest one level deeper.
            let tree = &self.doc.tree;
            let inner = tree.child_by_tag(component, "Component").or_else(|| tree.find1(component, "AudioComponent/Component"));
            stamp_next_component_number(&mut self.doc.tree, inner, count as i64 + 1)?;
        }
        Ok(MaskAdded { component_id: mask_id, component: mask_element, param_ids })
    }

    /// `Track.add_graphic(text, start, duration, name, position)`: place a text graphic on this
    /// track (what Premiere's Type tool writes, 66_eg_text). `duration` defaults to the 5 s
    /// still default, snapped down to whole frames.
    #[allow(clippy::too_many_arguments)]
    pub fn add_graphic(
        &mut self,
        track: TrackId,
        text: &str,
        start: Option<Time>,
        duration: Option<Time>,
        name: &str,
        position: (f64, f64),
        uuids: &mut dyn UuidSource,
    ) -> Result<GraphicAdded> {
        validate_name(text)?;
        validate_name(name)?;
        validate_vector2(position)?;
        let start = start.unwrap_or(Time::new(0));
        if let Some(d) = duration {
            if d.ticks <= 0 {
                return value("a graphic needs a positive duration");
            }
        }
        if self.track(track).media_type != MediaType::Video {
            return value("graphics go on video tracks");
        }
        let sequence = self.track(track).sequence;
        let Some(timebase) = self.timebase(sequence).filter(|&t| t != 0) else {
            return value("sequence has no timebase to align a graphic to");
        };
        let (width, height) = self.sequence(sequence).frame_size.unwrap_or((0, 0));
        let mut span = duration.map_or(GRAPHIC_DEFAULT_TICKS, |d| d.ticks);
        span -= py_mod(span, timebase);
        if span <= 0 {
            return value("a graphic needs at least one whole frame");
        }
        let display_format = self.video_display_format(sequence)?.unwrap_or(0);

        let stream = build_graphic_stream(&mut self.doc.tree, timebase, width, height);
        let stream_id = self.doc.add_object(stream)?;
        let media_uid = uuids.uuid4().to_string();
        let media = build_graphic_media(&mut self.doc.tree, &media_uid, &stream_id, name, uuids);
        self.doc.tree.set(media, "ObjectUID", media_uid.clone());
        self.doc.attach_object(media)?;
        let source = build_graphic_source(&mut self.doc.tree, &media_uid);
        let source_id = self.doc.add_object(source)?;
        let template_clip = build_graphic_clip(&mut self.doc.tree, &source_id, 0, span, true, uuids);
        let template_id = self.doc.add_object(template_clip)?;
        let logging = build_graphic_logging(&mut self.doc.tree, name, timebase, display_format);
        let logging_id = self.doc.add_object(logging)?;
        let groups = build_empty_channel_groups(&mut self.doc.tree);
        let groups_id = self.doc.add_object(groups)?;
        let master_uid = uuids.uuid4().to_string();
        let master = build_graphic_master(&mut self.doc.tree, &master_uid, &logging_id, &template_id, &groups_id, name);
        self.doc.tree.set(master, "ObjectUID", master_uid.clone());
        self.doc.attach_object(master)?;

        let clip = build_graphic_clip(&mut self.doc.tree, &source_id, GRAPHIC_BASE_TICKS, GRAPHIC_BASE_TICKS + span, false, uuids);
        let clip_object_id = self.doc.add_object(clip)?;
        let subclip = new_subclip_element(&mut self.doc.tree, &clip_object_id, &master_uid, name);
        let subclip_object_id = self.doc.add_object(subclip)?;

        let source_text = build_source_text_param(&mut self.doc.tree, text, uuids)?;
        let mut param_ids = vec![self.doc.add_object(source_text)?];
        for param in build_text_params(&mut self.doc.tree, position) {
            param_ids.push(self.doc.add_object(param)?);
        }
        let component = build_text_component(&mut self.doc.tree, &param_ids, text, uuids);
        let component_id = self.doc.add_object(component)?;
        let chain = build_graphic_chain(&mut self.doc.tree, &component_id);
        let chain_object_id = self.doc.add_object(chain)?;

        let element =
            new_video_track_item_element(&mut self.doc.tree, &chain_object_id, &subclip_object_id, span, Some(stream))?;
        place_on_timeline(&mut self.doc.tree, element, start)?;
        let item_object_id = self.doc.add_object(element)?;
        self.add_track_item_ref(track, &item_object_id, "ClipItems")?;
        let placed = self.push_track_item(super::TrackItemData::new(element, subclip, clip, track));
        self.track_mut(track).clips.push(placed);
        self.grow_source_duration(sequence)?;
        Ok(GraphicAdded { item: placed, component_id, param_ids })
    }

    /// `Project._add_template_item(master_uid, name)`: file an imported Motion Graphics template
    /// into its media bin.
    pub fn add_template_item(&mut self, master_uid: &str, name: &str, uuids: &mut dyn UuidSource) -> Result<ItemId> {
        let bin = self.root_bin(MEDIA_FOLDER, |tree, _| Ok(build_media_bin(tree)), uuids)?;
        let Some(container) = item_container(&self.doc.tree, self.item(bin).element) else {
            return value("bin has no ProjectItemContainer");
        };
        let item_uid = uuids.uuid4().to_string();
        let element = build_template_item(&mut self.doc.tree, &item_uid, master_uid, name);
        self.doc.attach_object(element)?;
        let items = child_items(&mut self.doc.tree, container);
        add_item_ref(&mut self.doc.tree, items, &item_uid);
        let Some(&master) = self.doc.by_object_uid.get(master_uid) else {
            return Err(Error::Value(format!("'{master_uid}'")));
        };
        let mut clip_elements = Vec::new();
        for reference in self.doc.tree.findall(master, "Clips/Clip")? {
            clip_elements.push(self.doc.resolve(reference)?);
        }
        let mut data = ItemData::new(element, ItemType::Clip);
        data.parent = Some(bin);
        data.master = Some(master);
        data.clip_elements = clip_elements;
        let item = self.push_item(data);
        self.item_mut(bin).children.push(item);
        Ok(item)
    }

    /// `Track._clips_within(start, end)`: the clips an overwrite over the span would erase. A
    /// clip the span only partly covers is refused.
    pub fn clips_within(&self, track: TrackId, start: i64, end: i64) -> Result<Vec<TrackItemId>> {
        let mut covered = Vec::new();
        for &clip in &self.track(track).clips {
            let (clip_start, clip_end) = (self.start(clip)?.ticks, self.end(clip)?.ticks);
            if clip_end <= start || clip_start >= end {
                continue;
            }
            if clip_start < start || clip_end > end {
                return value(format!(
                    "placing here would partly cover {}; move it or free the span first",
                    py_str_repr(&self.track_item_name(clip)?)
                ));
            }
            covered.push(clip);
        }
        Ok(covered)
    }

    /// `Track.add_mgt(path, start)`: import a Motion Graphics template onto this track.
    ///
    /// The template's graphic is copied next to the project under `Motion Graphics Template
    /// Media` and imported from there, the imported item is filed into the bin of the same name,
    /// and both the master and the placement carry an `AE.ADBE Capsule` component. The
    /// placement runs for the template's full duration floored to the frame grid and overwrites
    /// the whole clips it lands on.
    pub fn add_mgt(&mut self, track: TrackId, path: &Path, start: Option<Time>, uuids: &mut dyn UuidSource) -> Result<MgtAdded> {
        // validate_path(must_exist=True, must_be_file=True)
        let display = crate::model::project::py_path(&path.to_string_lossy());
        if !path.exists() {
            return value(format!("path does not exist: {}", display.display()));
        }
        if !path.is_file() {
            return value(format!("path is not a file: {}", display.display()));
        }
        let start = start.unwrap_or(Time::new(0));
        if self.track(track).media_type != MediaType::Video {
            return value("Motion Graphics templates go on video tracks");
        }
        let sequence = self.track(track).sequence;
        let template = Template::open(&display)?;
        if template.has_audio {
            return value("Motion Graphics templates with audio are not supported");
        }
        let mut span = template.duration_ticks;
        if let Some(timebase) = self.timebase(sequence).filter(|&t| t != 0) {
            span -= py_mod(span, timebase);
        }
        let covered = self.clips_within(track, start.ticks, start.ticks + span)?;

        let project_dir = resolve_path(self.path()).parent().map(Path::to_path_buf).unwrap_or_else(|| "/".into());
        let media_file = template.media_path(&project_dir);
        if let Some(parent) = media_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&media_file, &template.graphic)?;

        // Both copies of the control set share one store of payloads.
        let mut stored = Stored::new();
        let stream = build_mgt_stream(&mut self.doc.tree, &template);
        let stream_id = self.doc.add_object(stream)?;
        let media_uid = uuids.uuid4().to_string();
        let media = build_mgt_media(
            &mut self.doc.tree,
            &template,
            &media_uid,
            &stream_id,
            &media_file,
            &mut stored,
            &project_dir,
            uuids,
        );
        let media = self.doc.attach_object(media)?;
        self.imported_media.push(media);
        let source = build_mgt_source(&mut self.doc.tree, &template, &media_uid);
        let source_id = self.doc.add_object(source)?;
        let template_clip = build_mgt_clip(&mut self.doc.tree, &source_id, true, uuids);
        let template_clip_id = self.doc.add_object(template_clip)?;
        let logging = build_mgt_logging(&mut self.doc.tree, &template);
        let logging_id = self.doc.add_object(logging)?;
        let groups = build_empty_channel_groups(&mut self.doc.tree);
        let groups_id = self.doc.add_object(groups)?;

        let mut master_params = Vec::new();
        for param in build_capsule_params(&mut self.doc.tree, &template, &mut stored, uuids)? {
            master_params.push(self.doc.add_object(param)?);
        }
        let master_component =
            build_capsule_component(&mut self.doc.tree, &template, &master_params, &mut stored, MASTER_SLOT, uuids);
        let master_component = self.doc.add_object(master_component)?;
        let chain = build_blueprint_chain(&mut self.doc.tree, &master_component);
        let chain_id = self.doc.add_object(chain)?;
        let master_uid = uuids.uuid4().to_string();
        let master = build_mgt_master(
            &mut self.doc.tree,
            &template,
            &master_uid,
            &logging_id,
            &chain_id,
            &template_clip_id,
            &groups_id,
        );
        self.doc.attach_object(master)?;
        let item = self.add_template_item(&master_uid, &template.name, uuids)?;

        for clip in covered {
            self.remove_clip(track, clip)?;
        }
        let placed = self.add_clip(track, item, start, uuids)?;
        let mut param_ids = Vec::new();
        for param in build_capsule_params(&mut self.doc.tree, &template, &mut stored, uuids)? {
            param_ids.push(self.doc.add_object(param)?);
        }
        let component =
            build_capsule_component(&mut self.doc.tree, &template, &param_ids, &mut stored, PLACEMENT_SLOT, uuids);
        let component_id = self.doc.add_object(component)?;
        let placed_element = self.track_item(placed).element;
        let Some(chain_reference) = self.doc.tree.find1(placed_element, "ClipTrackItem/ComponentOwner/Components") else {
            return value("placement has no component chain");
        };
        let chain = self.doc.resolve(chain_reference)?;
        attach_component(&mut self.doc.tree, chain, &component_id)?;
        tag_as_template(&mut self.doc.tree, placed_element, &template.capsule_id)?;
        // A template renders at the SEQUENCE's frame size, not the size it was authored at, and
        // the placement records the rendered raster.
        let frame_rect = self.doc.tree.child_by_tag(placed_element, "FrameRect");
        if let (Some(frame_rect), Some((w, h))) = (frame_rect, self.sequence(sequence).frame_size) {
            self.doc.tree.set_text(frame_rect, Some(format!("0,0,{w},{h}")));
        }
        Ok(MgtAdded { item: placed, component_id, param_ids })
    }

    /// `Sequence.import_mgt(path, start, video_track_offset)`: `Track.add_mgt` on a video track.
    pub fn import_mgt(
        &mut self,
        sequence: SequenceId,
        path: &Path,
        start: Option<Time>,
        video_track_offset: usize,
        uuids: &mut dyn UuidSource,
    ) -> Result<MgtAdded> {
        let Some(&track) = self.video_tracks(sequence).get(video_track_offset) else {
            return value(format!("sequence has no video track {video_track_offset}"));
        };
        self.add_mgt(track, path, start, uuids)
    }
}

/// `_tag_as_template`: key a placement to the template it was imported from.
fn tag_as_template(tree: &mut crate::xml::Tree, item: NodeId, capsule_id: &str) -> Result<()> {
    let Some(core) = tree.find1(item, "ClipTrackItem/TrackItem") else { return value("placement has no TrackItem") };
    let node = tree.new_element_with("Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t\t\t"));
    tree.set_tail(node, Some("\n\t\t\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t\t\t"));
    crate::build::leaf(tree, properties, MGT_IDENTIFIER, capsule_id, "\n\t\t\t\t\t");
    tree.insert(core, 0, node);
    Ok(())
}
