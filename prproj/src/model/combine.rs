//! `Project.create_merged_clip` and `Project.create_multicam_clip` (port of py_premiere
//! `models/project.py`): the merged clip re-imports both sources as private copies (so the
//! media files must still be readable), the multicam clip places the original items.

use super::item::{add_item_ref, child_items, detach_item_ref, item_container};
use super::project::py_int;
use super::validators::validate_nonempty_string;
use super::{ItemData, ItemId, ItemType, Project};
use crate::build::media as m;
use crate::build::merged as mb;
use crate::build::multicam::{build_multicam, build_processed_clips_bin};
use crate::build::time::Time;
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::probe::py_mod;
use crate::xml::mutations::{append_leaf, insert_leaf_before, remove_child};
use crate::xml::NodeId;

/// `TIMECODE_FORMATS[(timebase, False)]`, with Python's `KeyError`.
fn display_format(timebase: i64) -> Result<i64> {
    m::timecode_format(timebase, false).ok_or_else(|| Error::Key(format!("({timebase}, False)")))
}

/// `int(text or 0)`.
fn int_or_zero(text: Option<String>) -> Result<i64> {
    match text {
        Some(t) if !t.is_empty() => py_int(&t),
        _ => Ok(0),
    }
}

/// `duration - duration % timebase` (Python `%`).
fn whole_frames(duration: i64, timebase: i64) -> Result<i64> {
    let r = py_mod(duration as i128, timebase as i128)?;
    i64::try_from(duration as i128 - r).map_err(|_| Error::Overflow("duration out of range".into()))
}

/// `(rect or "0,0,0,0").split(",")` -> `int(rect[2]), int(rect[3])`.
fn rect_width_height(rect: Option<String>) -> Result<(i64, i64)> {
    let rect = rect.filter(|r| !r.is_empty()).unwrap_or_else(|| "0,0,0,0".into());
    let parts: Vec<&str> = rect.split(',').collect();
    let at = |i: usize| -> Result<i64> {
        py_int(parts.get(i).ok_or_else(|| Error::Index("list index out of range".into()))?)
    };
    Ok((at(2)?, at(3)?))
}

/// A merged clip's source item, its private copy and the copy's MergeClipUtils bag.
type CopyBag<'a> = (ItemId, ItemId, Vec<(&'a str, &'a str)>);

impl Project {
    fn object_id(&self, element: NodeId) -> String {
        self.doc.tree.get(element, "ObjectID").unwrap_or("").to_string()
    }

    /// The ComponentChain edit both builders make on an audio placement: drop the
    /// `MZ.ActiveComponent` node bag and close the chain empty.
    fn strip_chain_node(&mut self, chain: NodeId) {
        let tree = &mut self.doc.tree;
        if let Some(inner) = tree.child_by_tag(chain, "ComponentChain") {
            if let Some(node) = tree.child_by_tag(inner, "Node") {
                tree.remove(inner, node);
                tree.set_text(inner, Some("\n\t\t"));
            }
        }
    }

    /// `Project.create_merged_clip(video_item, audio_item, name)`: merge a video-only movie
    /// with a mono or stereo audio-only clip (Premiere's `Merge Clips`). `name` defaults to the
    /// video item's name plus `" - Merged"`. Returns the panel item (filed at the root).
    pub fn create_merged_clip(
        &mut self,
        video_item: ItemId,
        audio_item: ItemId,
        name: Option<&str>,
        uuids: &mut dyn UuidSource,
    ) -> Result<ItemId> {
        if let Some(n) = name {
            validate_nonempty_string(n)?;
        }
        let (Some(video_path), Some(audio_path)) =
            (self.item(video_item).media_path.clone(), self.item(audio_item).media_path.clone())
        else {
            return value("item has no media file to merge");
        };
        for (item, wanted) in [(video_item, "VideoClip"), (audio_item, "AudioClip")] {
            if self.item(item).item_type != ItemType::Clip || self.is_sequence_item(item) {
                return value("merged clips are built from media clip items");
            }
            if self.clip_element_tags(item) != [wanted] {
                return value("only a video-only movie plus an audio-only clip can be merged");
            }
        }
        let audio_template = self.item(audio_item).clip_elements[0];
        let layout = self.doc.tree.findtext(audio_template, "AudioChannelLayout")?;
        let channels: usize = if layout.as_deref() == Some(m::MONO_LAYOUT) {
            1
        } else if layout.as_deref() == Some(m::audio_channel_layout(2).as_str()) {
            2
        } else {
            return value("only mono or stereo audio is supported in a merged clip");
        };
        let video_name = self.item_name(video_item);
        let audio_name = self.item_name(audio_item);
        let merged_name = match name {
            Some(n) => n.to_string(),
            None => format!("{video_name} - Merged"),
        };

        let video_copy = self.import_files(&[video_path], None, uuids)?[0];
        let audio_copy = self.import_files(&[audio_path], None, uuids)?[0];
        let copies: [CopyBag; 2] = [
            (video_item, video_copy, vec![("MZ.MergeClipUtils.ComponentMasterClipOriginalName", video_name.as_str())]),
            (
                audio_item,
                audio_copy,
                vec![
                    ("MZ.MergeClipUtils.AudioTrackNumberFromOriginalMergedClip", "0"),
                    ("MZ.MergeClipUtils.ComponentMasterClipOriginalName", audio_name.as_str()),
                ],
            ),
        ];
        for (source, copy, bag) in &copies {
            let Some(master) = self.item(*copy).master else {
                return value("synthesized copy has no master clip");
            };
            let bag_element = mb::build_master_bag(&mut self.doc.tree, bag);
            self.doc.tree.insert(master, 0, bag_element);
            if let Some(name_element) = self.doc.tree.child_by_tag(master, "Name") {
                self.doc.tree.set_text(name_element, Some(merged_name.as_str()));
            }
            if let Some(logging_ref) = self.doc.tree.child_by_tag(master, "LoggingInfo") {
                let logging = self.doc.resolve(logging_ref)?;
                if let Some(clip_name) = self.doc.tree.child_by_tag(logging, "ClipName") {
                    self.doc.tree.set_text(clip_name, Some(merged_name.as_str()));
                }
            }
            if let Some(mapping) = self.doc.tree.child_by_tag(master, "DefMappingID") {
                remove_child(&mut self.doc.tree, master, mapping)?;
            }
            self.share_file_identity(*source, *copy)?;
        }

        if channels > 1 {
            let Some(audio_master) = self.item(audio_copy).master else {
                return value("synthesized copy has no master clip");
            };
            let chains_list = self.doc.tree.child_by_tag(audio_master, "AudioComponentChains");
            let groups_ref = self.doc.tree.child_by_tag(audio_master, "AudioClipChannelGroups");
            let (Some(chains_list), Some(groups_ref)) = (chains_list, groups_ref) else {
                return value("audio copy has no channel plumbing");
            };
            for entry in self.doc.tree.findall(chains_list, "AudioComponentChain")? {
                let chain = self.doc.resolve(entry)?;
                self.doc.remove_object(chain)?;
            }
            for entry in self.doc.tree.children(chains_list).to_vec() {
                self.doc.tree.remove(chains_list, entry);
            }
            let mut chain_ids = Vec::new();
            for _ in 0..channels {
                let chain = m::new_master_audio_chain(&mut self.doc.tree, 1, 0);
                chain_ids.push(self.register(chain)?);
            }
            let refs: Vec<&str> = chain_ids.iter().map(String::as_str).collect();
            crate::build::indexed_refs(&mut self.doc.tree, chains_list, "AudioComponentChain", &refs);
            let groups = self.doc.resolve(groups_ref)?;
            let Some(vectors_list) = self.doc.tree.child_by_tag(groups, "ClipChannelVectors") else {
                return value("audio copy has no channel vectors");
            };
            for entry in self.doc.tree.findall(vectors_list, "ClipChannelVectorItem")? {
                let vector = self.doc.resolve(entry)?;
                for channel_entry in self.doc.tree.findall(vector, "ClipChannels/ClipChannelItem")? {
                    let channel = self.doc.resolve(channel_entry)?;
                    self.doc.remove_object(channel)?;
                }
                self.doc.remove_object(vector)?;
            }
            for entry in self.doc.tree.children(vectors_list).to_vec() {
                self.doc.tree.remove(vectors_list, entry);
            }
            let mut mono_vector_ids = Vec::new();
            for channel in 0..channels {
                let serializer = m::new_channel_serializer(&mut self.doc.tree, 0, channel as i64);
                let serializer_id = self.register(serializer)?;
                let vector = m::new_channel_vector(&mut self.doc.tree, &[&serializer_id], 0);
                mono_vector_ids.push(self.register(vector)?);
            }
            let refs: Vec<&str> = mono_vector_ids.iter().map(String::as_str).collect();
            crate::build::indexed_refs(&mut self.doc.tree, vectors_list, "ClipChannelVectorItem", &refs);
        }

        let (Some(video_media), Some(audio_media)) = (self.media_element(video_copy)?, self.media_element(audio_copy)?) else {
            return value("synthesized copy has no media");
        };
        let video_stream_ref = self.doc.tree.child_by_tag(video_media, "VideoStream");
        let audio_stream_ref = self.doc.tree.child_by_tag(audio_media, "AudioStream");
        let (Some(video_stream_ref), Some(audio_stream_ref)) = (video_stream_ref, audio_stream_ref) else {
            return value("synthesized copy has no media stream");
        };
        let video_stream = self.doc.resolve(video_stream_ref)?;
        let audio_stream = self.doc.resolve(audio_stream_ref)?;
        let timebase = int_or_zero(self.doc.tree.findtext(video_stream, "FrameRate")?)?;
        let audio_rate = int_or_zero(self.doc.tree.findtext(audio_stream, "FrameRate")?)?;
        let video_duration = int_or_zero(self.doc.tree.findtext(video_stream, "Duration")?)?;
        let audio_duration = int_or_zero(self.doc.tree.findtext(audio_stream, "Duration")?)?;
        let (width, height) = rect_width_height(self.doc.tree.findtext(video_stream, "FrameRect")?)?;
        let end_ticks =
            whole_frames(video_duration, timebase)?.max(whole_frames(audio_duration, timebase)?);
        let display = display_format(timebase)?;

        let sequence_uid = uuids.uuid4().to_string();
        let video_track_uid = uuids.uuid4().to_string();
        let audio_track_uids: Vec<String> = (0..channels).map(|_| uuids.uuid4().to_string()).collect();
        let track_uid_refs: Vec<&str> = audio_track_uids.iter().map(String::as_str).collect();
        let chain = mb::build_empty_video_chain(&mut self.doc.tree);
        let group_chain_id = self.register(chain)?;
        let mut track_chain_ids = Vec::new();
        let mut track_pan_ids = Vec::new();
        for _ in 0..channels {
            let (chain_id, pan_id) = self.register_fader_chain(audio_rate)?;
            track_chain_ids.push(chain_id);
            track_pan_ids.push(pan_id);
        }
        let (mix_chain_id, mix_pan_id) = self.register_fader_chain(audio_rate)?;
        let inlet = mb::build_inlet(&mut self.doc.tree, &track_uid_refs);
        let inlet_id = self.register(inlet)?;
        let mix_track = mb::build_mix_track(&mut self.doc.tree, uuids, &mix_chain_id, &mix_pan_id, &inlet_id);
        let mix_track_id = self.register(mix_track)?;
        let video_group =
            mb::build_video_group(&mut self.doc.tree, &video_track_uid, timebase, width, height, &group_chain_id);
        let video_group_id = self.register(video_group)?;
        let audio_group = mb::build_audio_group(&mut self.doc.tree, uuids, &track_uid_refs, audio_rate, &mix_track_id);
        let audio_group_id = self.register(audio_group)?;
        let data_group = mb::build_data_group(&mut self.doc.tree, timebase);
        let data_group_id = self.register(data_group)?;
        let video_track = mb::build_video_track(&mut self.doc.tree, &video_track_uid);
        self.doc.attach_object(video_track)?;
        for (index, track_uid) in audio_track_uids.iter().enumerate() {
            let track = mb::build_audio_track(
                &mut self.doc.tree,
                uuids,
                track_uid,
                &track_chain_ids[index],
                &track_pan_ids[index],
                2 + index as i64,
                index as i64,
            );
            self.doc.attach_object(track)?;
        }
        let sequence_element = mb::build_merged_sequence(
            &mut self.doc.tree,
            &sequence_uid,
            &merged_name,
            display,
            end_ticks,
            width,
            height,
            &video_group_id,
            &audio_group_id,
            &data_group_id,
        );
        let sequence_element = self.doc.attach_object(sequence_element)?;

        let sequence = self.parse_sequence(sequence_element)?;
        let video_track = self.video_tracks(sequence)[0];
        let video_placed = self.add_clip(video_track, video_copy, Time::new(0), uuids)?;
        let mut audio_placed_ids = Vec::new();
        for (channel, track) in self.audio_tracks(sequence).to_vec().into_iter().enumerate() {
            let placed = self.add_clip(track, audio_copy, Time::new(0), uuids)?;
            self.narrow_merged_channel(placed, channel as i64)?;
            audio_placed_ids.push(self.object_id(self.track_item(placed).element));
        }
        let mut link_items = vec![self.object_id(self.track_item(video_placed).element)];
        link_items.extend(audio_placed_ids);
        let link_refs: Vec<&str> = link_items.iter().map(String::as_str).collect();
        let link = mb::build_link(&mut self.doc.tree, &link_refs);
        let link_id = self.register(link)?;
        let Some(link_entry) = self.doc.tree.find1(sequence_element, "PersistentGroupContainer/LinkContainer/Links/Link") else {
            return value("merged sequence has no link entry");
        };
        self.doc.tree.set(link_entry, "ObjectRef", link_id);

        let video_source = mb::build_sequence_source(&mut self.doc.tree, "VideoSequenceSource", &sequence_uid, end_ticks);
        let video_source_id = self.register(video_source)?;
        let audio_source = mb::build_sequence_source(&mut self.doc.tree, "AudioSequenceSource", &sequence_uid, end_ticks);
        let audio_source_id = self.register(audio_source)?;
        let secondary = m::new_secondary_content(&mut self.doc.tree, &audio_source_id, 0);
        let secondary_id = self.register(secondary)?;
        let mut panel_vector_ids = Vec::new();
        for index in 0..channels {
            let channel = m::new_channel_serializer(&mut self.doc.tree, index as i64, 0);
            let channel_id = self.register(channel)?;
            let vector = m::new_channel_vector(&mut self.doc.tree, &[&channel_id], 0);
            panel_vector_ids.push(self.register(vector)?);
        }
        let refs: Vec<&str> = panel_vector_ids.iter().map(String::as_str).collect();
        let groups = m::new_audio_channel_groups(&mut self.doc.tree, &refs);
        let groups_id = self.register(groups)?;
        let mut master_chain_ids = Vec::new();
        for _ in 0..channels {
            let chain = m::new_master_audio_chain(&mut self.doc.tree, 1, 0);
            master_chain_ids.push(self.register(chain)?);
        }
        let audio_template = mb::build_panel_audio_template(&mut self.doc.tree, uuids, &audio_source_id, &secondary_id);
        let audio_template_id = self.register(audio_template)?;
        let video_template = mb::build_panel_video_template(&mut self.doc.tree, uuids, &video_source_id);
        let video_template_id = self.register(video_template)?;
        let logging = mb::build_panel_logging(&mut self.doc.tree, &video_name, display, end_ticks, timebase);
        let logging_id = self.register(logging)?;
        let master_uid = uuids.uuid4().to_string();
        let chain_refs: Vec<&str> = master_chain_ids.iter().map(String::as_str).collect();
        let master_element = mb::build_panel_master(
            &mut self.doc.tree,
            &master_uid,
            &logging_id,
            &chain_refs,
            &audio_template_id,
            &video_template_id,
            &groups_id,
            &merged_name,
        );
        let master_element = self.doc.attach_object(master_element)?;
        let item_uid = uuids.uuid4().to_string();
        let item_element = mb::build_panel_item(&mut self.doc.tree, &item_uid, &master_uid, &merged_name);
        let item_element = self.doc.attach_object(item_element)?;

        let root = self.require_root()?;
        let container = self.require_item_container(root, "root item")?;
        for copy in [video_copy, audio_copy] {
            let element = self.item(copy).element;
            let uid = self.doc.tree.get(element, "ObjectUID").unwrap_or("").to_string();
            detach_item_ref(&mut self.doc.tree, container, &uid)?;
            self.doc.remove_object(element)?;
            let children = &mut self.item_mut(root).children;
            if let Some(at) = children.iter().position(|&c| c == copy) {
                children.remove(at);
            }
        }
        let items = child_items(&mut self.doc.tree, container);
        add_item_ref(&mut self.doc.tree, items, &item_uid);
        let clip_elements = vec![self.doc.by_object_id[&audio_template_id], self.doc.by_object_id[&video_template_id]];
        let mut data = ItemData::new(item_element, ItemType::Clip);
        data.parent = Some(root);
        data.master = Some(master_element);
        data.clip_elements = clip_elements;
        data.default_out_ticks = Some(end_ticks);
        data.sequence_uid = Some(sequence_uid);
        let item = self.push_item(data);
        self.item_mut(root).children.push(item);
        Ok(item)
    }

    /// One mono fader chain plus pan processor of the merged sequence's mix graph, registered
    /// in Python's order (volume, mute, fader, meter, chain, pan); returns `(chain, pan)`.
    fn register_fader_chain(&mut self, audio_rate: i64) -> Result<(String, String)> {
        let volume = mb::build_volume_param(&mut self.doc.tree);
        let volume_id = self.register(volume)?;
        let mute = mb::build_mute_param(&mut self.doc.tree);
        let mute_id = self.register(mute)?;
        let fader = mb::build_fader(&mut self.doc.tree, audio_rate, &[&volume_id, &mute_id]);
        let fader_id = self.register(fader)?;
        let meter = mb::build_meter(&mut self.doc.tree, audio_rate);
        let meter_id = self.register(meter)?;
        let chain = mb::build_fader_chain(&mut self.doc.tree, &fader_id, &meter_id);
        let chain_id = self.register(chain)?;
        let pan = mb::build_pan_processor(&mut self.doc.tree, audio_rate);
        let pan_id = self.register(pan)?;
        Ok((chain_id, pan_id))
    }

    /// `Project.create_multicam_clip(items, name)`: a multicam source clip from angle items in
    /// track order (two or more; exactly one carrying stereo audio, either an A/V movie or an
    /// audio-only clip). `name` defaults to the first angle's name plus `Multicam`. The angles
    /// are filed into a root-level `Processed Clips` bin (reused when present).
    pub fn create_multicam_clip(&mut self, items: &[ItemId], name: Option<&str>, uuids: &mut dyn UuidSource) -> Result<ItemId> {
        if let Some(n) = name {
            validate_nonempty_string(n)?;
        }
        if items.len() < 2 {
            return value("a multicam clip needs at least two angles");
        }
        let mut audio_angle: Option<ItemId> = None;
        let mut video_items: Vec<ItemId> = Vec::new();
        for &item in items {
            if self.item(item).item_type != ItemType::Clip || self.is_sequence_item(item) {
                return value("multicam angles must be media clip items");
            }
            if self.item(item).media_path.is_none() {
                return value("angle has no media file");
            }
            let mut tags = self.clip_element_tags(item);
            tags.sort();
            if tags == ["VideoClip"] {
                video_items.push(item);
                continue;
            }
            if audio_angle.is_some() {
                return value("only one angle may carry audio");
            }
            if tags == ["AudioClip", "VideoClip"] {
                audio_angle = Some(item);
                video_items.push(item);
            } else if tags == ["AudioClip"] {
                audio_angle = Some(item);
            } else {
                return value("multicam angles must be movie or audio clips");
            }
        }
        let Some(audio_angle) = audio_angle else { return value("one angle must carry the audio track") };
        if video_items.is_empty() {
            return value("a multicam clip needs at least one video angle");
        }
        let audio_template = self
            .item(audio_angle)
            .clip_elements
            .iter()
            .copied()
            .find(|&e| self.doc.tree.tag(e) == "AudioClip")
            .expect("the audio angle has an AudioClip");
        if self.doc.tree.findtext(audio_template, "AudioChannelLayout")? != Some(m::audio_channel_layout(2)) {
            return value("only a stereo audio angle is supported");
        }
        let audio_is_linked = video_items.contains(&audio_angle);
        let multicam_name = match name {
            Some(n) => n.to_string(),
            None => format!("{}Multicam", self.item_name(items[0])),
        };

        let (mut timebase, mut width, mut height, mut end_ticks) = (0i64, 0i64, 0i64, 0i64);
        for (index, &item) in video_items.iter().enumerate() {
            let media = self.media_element(item)?;
            let Some(stream_ref) = media.and_then(|m| self.doc.tree.child_by_tag(m, "VideoStream")) else {
                return value("angle has no video stream");
            };
            let stream = self.doc.resolve(stream_ref)?;
            let rate = int_or_zero(self.doc.tree.findtext(stream, "FrameRate")?)?;
            let duration = int_or_zero(self.doc.tree.findtext(stream, "Duration")?)?;
            if index == 0 {
                timebase = rate;
                (width, height) = rect_width_height(self.doc.tree.findtext(stream, "FrameRect")?)?;
            }
            end_ticks = end_ticks.max(whole_frames(duration, timebase)?);
        }
        let display = display_format(timebase)?;

        let fragment = build_multicam(
            &mut self.doc.tree,
            uuids,
            &multicam_name,
            timebase,
            width,
            height,
            display,
            end_ticks,
            video_items.len(),
            audio_is_linked,
        )?;
        let spliced = self.splice(fragment, uuids)?;
        let (Some(sequence_element), Some(item_element), Some(master_element)) =
            (spliced.get("Sequence"), spliced.get("ClipProjectItem"), spliced.get("MasterClip"))
        else {
            return value("multicam fragment is missing its core objects");
        };

        let sequence = self.parse_sequence(sequence_element)?;
        let mut av_video_placed = None;
        for (index, &angle) in video_items.iter().enumerate() {
            let track = self.video_tracks(sequence)[index];
            let placed = self.add_clip(track, angle, Time::new(0), uuids)?;
            if angle == audio_angle {
                av_video_placed = Some(placed);
            }
        }
        let audio_track = self.audio_tracks(sequence)[0];
        let audio_placed = self.add_clip(audio_track, audio_angle, Time::new(0), uuids)?;
        let placed_element = self.track_item(audio_placed).element;
        if let Some(chain_ref) = self.doc.tree.find1(placed_element, "ClipTrackItem/ComponentOwner/Components") {
            let chain = self.doc.resolve(chain_ref)?;
            self.strip_chain_node(chain);
            insert_leaf_before(&mut self.doc.tree, chain, "ComponentChain", "DefaultChannelVolumeComponentID", "2")?;
            append_leaf(&mut self.doc.tree, chain, "AudioChannelLayout", &m::audio_channel_layout(2));
            append_leaf(&mut self.doc.tree, chain, "ChannelType", "1");
        }
        if let Some(video_placed) = av_video_placed {
            let ids = [self.object_id(placed_element), self.object_id(self.track_item(video_placed).element)];
            let link = mb::build_link(&mut self.doc.tree, &[&ids[0], &ids[1]]);
            let link_id = self.register(link)?;
            let Some(link_entry) = self.doc.tree.find1(sequence_element, "PersistentGroupContainer/LinkContainer/Links/Link")
            else {
                return value("multicam sequence has no link entry");
            };
            self.doc.tree.set(link_entry, "ObjectRef", link_id);
        }

        let root = self.require_root()?;
        let root_container = self.require_item_container(root, "root item")?;
        let bin = self.root_bin("Processed Clips", |tree, _| Ok(build_processed_clips_bin(tree)), uuids)?;
        let Some(bin_container) = item_container(&self.doc.tree, self.item(bin).element) else {
            return value("bin has no ProjectItemContainer");
        };
        for &angle in items {
            if self.item(angle).parent == Some(bin) {
                continue;
            }
            let parent = self.item(angle).parent;
            let container = parent.and_then(|p| item_container(&self.doc.tree, self.item(p).element));
            let angle_uid = self.doc.tree.get(self.item(angle).element, "ObjectUID").unwrap_or("").to_string();
            if let (Some(parent), Some(container)) = (parent, container) {
                detach_item_ref(&mut self.doc.tree, container, &angle_uid)?;
                let children = &mut self.item_mut(parent).children;
                if let Some(at) = children.iter().position(|&c| c == angle) {
                    children.remove(at);
                }
            }
            let list = child_items(&mut self.doc.tree, bin_container);
            add_item_ref(&mut self.doc.tree, list, &angle_uid);
            self.item_mut(angle).parent = Some(bin);
            self.item_mut(bin).children.push(angle);
        }

        let item_uid = self.doc.tree.get(item_element, "ObjectUID").unwrap_or("").to_string();
        let list = child_items(&mut self.doc.tree, root_container);
        add_item_ref(&mut self.doc.tree, list, &item_uid);
        let clip_elements = self.resolve_all(master_element, "Clips/Clip")?;
        let mut data = ItemData::new(item_element, ItemType::Clip);
        data.parent = Some(root);
        data.master = Some(master_element);
        data.clip_elements = clip_elements;
        data.default_out_ticks = Some(end_ticks);
        data.sequence_uid = self.doc.tree.get(sequence_element, "ObjectUID").map(str::to_string);
        let item = self.push_item(data);
        self.item_mut(root).children.push(item);
        Ok(item)
    }
}
