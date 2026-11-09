//! Project and Sequence entry points: `Sequence.create_caption_track` and
//! `Project._import_caption` (the `.srt` branch of `import_files`).

use std::path::Path;

use super::builder::*;
use super::flat::decode_utf8;
use super::model::{CaptionFormat, CaptionTrackId};
use super::template::{SYNTHETIC_STATE, TEMPLATE_STYLE};
use super::{PyError, PyResult};
use crate::base64::b64encode;
use crate::build::media::{
    new_channel_groups, new_clip_item, new_master_clip, new_markers_collection, new_media_logging_info,
};
use crate::build::time::{floor_div, TICKS_PER_SECOND};
use crate::ids::UuidSource;
use crate::model::project::{py_int, py_path, resolve_path};
use crate::model::{ItemId, ItemType, Project, SequenceId};
use crate::xml::mutations::insert_before;

fn text_int(text: Option<String>, default: &str) -> PyResult<i64> {
    Ok(py_int(text.filter(|t| !t.is_empty()).as_deref().unwrap_or(default))?)
}

impl Project {
    /// `Sequence.create_caption_track(item, caption_format)`: place an imported caption item on a
    /// new timeline caption track (each cue a `CaptionDataClipTrackItem` frame-snapped to the
    /// sequence's timebase, backed by its own synthetic 12-hour caption source).
    pub fn create_caption_track(
        &mut self,
        sequence: SequenceId,
        item: ItemId,
        caption_format: CaptionFormat,
        uuids: &mut dyn UuidSource,
    ) -> PyResult<CaptionTrackId> {
        let clip_elements = self.item(item).clip_elements.clone();
        let segments_ref = match clip_elements.first() {
            Some(&first) => self.doc.tree.child_by_tag(first, "TranscriptTextSegments"),
            None => None,
        };
        let Some(segments_ref) = segments_ref else {
            return super::value_err("item is not an imported caption file");
        };
        let collection = self.doc.resolve(segments_ref)?;
        let first = clip_elements[0];
        let label_color = self.doc.tree.findtext(first, "DataClip/Clip/Node/Properties/asl.clip.label.color")?;
        let label_name = self.doc.tree.findtext(first, "DataClip/Clip/Node/Properties/asl.clip.label.name")?;
        let label_name = label_name.filter(|t| !t.is_empty()).unwrap_or_else(|| "BE.Prefs.LabelColors.7".to_string());
        let label_index = py_int(label_name.rsplit_once('.').map(|x| x.1).ok_or_else(|| {
            PyError::Index("list index out of range".into())
        })?)?;
        let label_color = text_int(label_color, "277129")?;

        self.caption_track_from_collection(sequence, collection, label_index, label_color, caption_format, uuids)
    }

    /// Create a subtitle track directly from exact, frame-aligned ticks. No SRT
    /// staging file or millisecond round trip; content is embedded in the project.
    pub fn add_caption_track(
        &mut self, sequence: SequenceId, cues: &[SrtCue], uuids: &mut dyn UuidSource,
    ) -> PyResult<CaptionTrackId> {
        let Some(frame) = self.sequence(sequence).timebase.filter(|n| *n > 0) else { return super::value_err("sequence has no positive timebase"); };
        if cues.is_empty() || cues.len() > 100_000 { return super::value_err("caption cue count outside bounds"); }
        let mut previous_end = 0;
        for c in cues {
            if c.start < previous_end || c.end <= c.start || c.end > SYNTHETIC_DURATION - 3600*TICKS_PER_SECOND || c.start % frame != 0 || c.end % frame != 0 || c.text.is_empty() || c.text.len() > 32768 || c.text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') { return super::value_err("caption cues must be bounded, ordered, nonoverlapping and exactly frame aligned"); }
            previous_end = c.end;
        }
        let mut entries = Vec::with_capacity(cues.len());
        for c in cues {
            let payload = super::flat::write_font_family(&build_caption_payload(&c.text.replace('\n', "\r")), "ArialMT")?;
            let payload = b64encode(&payload);
            let hash = uuids.uuid4().to_string();
            let block = new_block(&mut self.doc.tree, &payload, &hash);
            let block_id = self.register(block)?;
            let caption = new_caption(&mut self.doc.tree, &block_id, c.start, c.end);
            entries.push((c.end, self.register(caption)?));
        }
        let pairs: Vec<(i64, &str)> = entries.iter().map(|(end,id)| (*end,id.as_str())).collect();
        let collection = new_caption_collection(&mut self.doc.tree, &pairs);
        self.register(collection)?;
        let track = self.caption_track_from_collection(sequence, collection, CAPTIONS_LABEL_INDEX, CAPTIONS_LABEL_COLOR, CaptionFormat::Subtitle, uuids)?;
        let style = self.doc.tree.child_by_tag(self.caption_track(track).element, "CaptionDataTemplateStyle").ok_or_else(|| PyError::Value("caption track style missing".into()))?;
        let payload = self.doc.payload(style)?.ok_or_else(|| PyError::Value("caption track style payload missing".into()))?;
        let payload = super::flat::write_font_family(&payload, "ArialMT")?;
        crate::payload::set_payload(&mut self.doc, style, &payload);
        self.grow_source_duration_to(sequence, previous_end)?;
        Ok(track)
    }

    fn caption_track_from_collection(
        &mut self, sequence: SequenceId, collection: crate::xml::NodeId,
        label_index: i64, label_color: i64, caption_format: CaptionFormat,
        uuids: &mut dyn UuidSource,
    ) -> PyResult<CaptionTrackId> {
        let Some(timebase) = self.sequence(sequence).timebase else {
            return super::value_err("sequence has no video timebase to snap captions to");
        };
        let sequence_element = self.sequence(sequence).element;

        let mut group_element = None;
        for pair in self.doc.tree.findall(sequence_element, "TrackGroups/TrackGroup")? {
            let group_ref = self.doc.tree.child_by_tag(pair, "Second");
            if self.doc.tree.findtext(pair, "First")?.as_deref() == Some(DATA_MEDIA_TYPE) {
                if let Some(group_ref) = group_ref {
                    group_element = Some(self.doc.resolve(group_ref)?);
                    break;
                }
            }
        }
        let Some(group_element) = group_element else { return super::value_err("sequence has no data track group") };
        let inner = self.doc.tree.child_by_tag(group_element, "TrackGroup");
        let next_id = inner.and_then(|i| self.doc.tree.child_by_tag(i, "NextTrackID"));
        let (Some(inner), Some(next_id)) = (inner, next_id) else {
            return super::value_err("data track group has no NextTrackID");
        };
        let track_id = text_int(self.doc.tree[next_id].text.clone(), "1")?;

        let base_frames = floor_div(3600 * TICKS_PER_SECOND, timebase)?;
        let mut item_ids: Vec<String> = Vec::new();
        for entry in self.doc.tree.findall(collection, "CaptionMap/CaptionMapItem")? {
            let Some(caption_ref) = self.doc.tree.child_by_tag(entry, "Second") else { continue };
            let caption = self.doc.resolve(caption_ref)?;
            let start = text_int(self.doc.tree.findtext(caption, "TimeStart")?, "0")?;
            let end = text_int(self.doc.tree.findtext(caption, "TimeEnd")?, "0")?;
            let start_frames = (start as f64 / timebase as f64 + 0.5) as i64;
            let end_frames = (end as f64 / timebase as f64 + 0.5) as i64;
            let Some(block_ref) = self.doc.tree.find1(caption, "BlockVector/BlockVectorItem") else {
                return super::value_err("caption carries no text block");
            };
            let block = self.doc.resolve(block_ref)?;
            let block_data = self.doc.tree.child_by_tag(block, "FormattedTextData");
            let block_hash = block_data.and_then(|d| self.doc.tree.get(d, "BinaryHash").map(str::to_string));

            let stream = new_synthetic_stream(&mut self.doc.tree, timebase);
            let stream_id = self.doc.add_object(stream)?;
            let media_uid = uuids.uuid4().to_string();
            let stored = self.doc.payload_stored(SYNTHETIC_STATE.0)?;
            let media = new_synthetic_media(&mut self.doc.tree, &media_uid, &stream_id, stored);
            self.doc.attach_object(media)?;
            let source = new_data_media_source(&mut self.doc.tree, &media_uid, SYNTHETIC_DURATION);
            let source_id = self.doc.add_object(source)?;
            let logging = new_empty_logging_info(&mut self.doc.tree);
            let logging_id = self.doc.add_object(logging)?;
            let clip_uuid = uuids.uuid4().to_string();
            let template = new_synthetic_template_clip(&mut self.doc.tree, &source_id, &clip_uuid);
            let template_id = self.doc.add_object(template)?;
            let groups = new_channel_groups(&mut self.doc.tree);
            let groups_id = self.doc.add_object(groups)?;
            let master_uid = uuids.uuid4().to_string();
            let master = new_master_clip(&mut self.doc.tree, &master_uid, &logging_id, &template_id, &groups_id, "SyntheticCaption");
            self.doc.tree.set(master, "ObjectUID", master_uid.clone());
            self.doc.attach_object(master)?;

            let timeline_uuid = uuids.uuid4().to_string();
            let timeline_clip = new_timeline_caption_clip(
                &mut self.doc.tree,
                &source_id,
                (base_frames + start_frames) * timebase,
                (base_frames + end_frames) * timebase,
                &timeline_uuid,
                label_index,
                label_color,
            );
            let timeline_clip_id = self.doc.add_object(timeline_clip)?;
            let subclip = new_caption_subclip(&mut self.doc.tree, &timeline_clip_id, &master_uid);
            let subclip_id = self.doc.add_object(subclip)?;
            let chain = new_data_chain(&mut self.doc.tree);
            let chain_id = self.doc.add_object(chain)?;
            let timeline_block = new_timeline_block(&mut self.doc.tree, block_hash.as_deref().unwrap_or(""));
            let block_id = self.doc.add_object(timeline_block)?;
            let track_item = new_caption_track_item(
                &mut self.doc.tree,
                &chain_id,
                &subclip_id,
                &block_id,
                start_frames * timebase,
                end_frames * timebase,
            );
            item_ids.push(self.doc.add_object(track_item)?);
        }

        let track_uid = uuids.uuid4().to_string();
        let stored = self.doc.payload_stored(TEMPLATE_STYLE.0)?;
        let refs: Vec<&str> = item_ids.iter().map(String::as_str).collect();
        let track = new_caption_track(&mut self.doc.tree, &track_uid, track_id, &refs, stored, caption_format.value())?;
        self.doc.tree.set(track, "ObjectUID", track_uid.clone());
        self.doc.attach_object(track)?;
        self.doc.invalidate_payloads();

        let tracks = match self.doc.tree.child_by_tag(inner, "Tracks") {
            Some(t) => t,
            None => {
                let t = self.doc.tree.new_element_with("Tracks", &[("Version", "1")]);
                self.doc.tree.set_text(t, Some("\n\t\t\t"));
                insert_before(&mut self.doc.tree, inner, "FrameRate", t)?;
                t
            }
        };
        let index = self.doc.tree.children(tracks).len().to_string();
        let reference = self.doc.tree.sub_element(tracks, "Track", &[("Index", &index), ("ObjectURef", &track_uid)]);
        self.doc.tree.set_tail(reference, Some("\n\t\t"));
        self.doc.tree.set_text(next_id, Some((track_id + 1).to_string()));

        let parsed = self.parse_caption_tracks(sequence, group_element)?;
        parsed.last().copied().ok_or_else(|| PyError::Index("list index out of range".into()))
    }

    /// `Project._import_caption(path, preferences)` through `import_files([path], target_bin)`:
    /// an SRT lands as a TranscriptClip-backed panel item. `label` is the `(index, colour)` the
    /// local Premiere preferences give the "Captions" kind (`None`: the factory Mango label).
    pub fn import_caption(
        &mut self,
        path: &Path,
        target_bin: Option<ItemId>,
        label: Option<(i64, i64)>,
        uuids: &mut dyn UuidSource,
    ) -> PyResult<ItemId> {
        if let Some(bin) = target_bin {
            if self.item(bin).item_type != ItemType::Bin {
                return super::value_err("target_bin must be a bin");
            }
        }
        let previous = std::mem::replace(&mut self.import_target, target_bin);
        let result = self.import_caption_inner(path, label, uuids);
        self.import_target = previous;
        result
    }

    fn import_caption_inner(
        &mut self,
        path: &Path,
        label: Option<(i64, i64)>,
        uuids: &mut dyn UuidSource,
    ) -> PyResult<ItemId> {
        let shown = py_path(&path.to_string_lossy());
        if !shown.exists() {
            return super::value_err(format!("path does not exist: {}", shown.display()));
        }
        if !shown.is_file() {
            return super::value_err(format!("path is not a file: {}", shown.display()));
        }
        let bytes = std::fs::read(path).map_err(|e| PyError::Os(e.to_string()))?;
        let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
        let text = decode_utf8(body)?;
        let cues = parse_srt(&text)?;
        let (label_index, label_color) = label.unwrap_or((CAPTIONS_LABEL_INDEX, CAPTIONS_LABEL_COLOR));
        let last_end = cues.iter().map(|c| c.end).max().expect("parse_srt returns cues");
        let frames = floor_div(-last_end, CAPTION_FRAME_RATE)?;
        let duration = -frames * CAPTION_FRAME_RATE;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let file_path = resolve_path(path).to_string_lossy().into_owned();
        let project_dir =
            resolve_path(self.path()).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
        let content_state = uuids.uuid4().to_string();

        let stream = new_data_stream(&mut self.doc.tree, duration);
        let stream_id = self.register(stream)?;
        let media_uid = uuids.uuid4().to_string();
        let media = new_data_media(&mut self.doc.tree, uuids, &media_uid, &stream_id, &file_path, &project_dir, &content_state);
        let media = self.register_uid(media, &media_uid)?;
        self.imported_media.push(media);
        let source = new_data_media_source(&mut self.doc.tree, &media_uid, duration);
        let source_id = self.register(source)?;
        let markers = new_markers_collection(&mut self.doc.tree, &content_state);
        let markers_id = self.register(markers)?;
        let logging = new_media_logging_info(
            &mut self.doc.tree,
            &name,
            CAPTION_FRAME_RATE,
            duration,
            None,
            CAPTION_TIMECODE_FORMAT,
            0,
            "",
        );
        let logging_id = self.register(logging)?;
        let mut entries: Vec<(i64, String)> = Vec::new();
        for cue in &cues {
            let payload = b64encode(&build_caption_payload(&cue.text));
            let hash = uuids.uuid4().to_string();
            let block = new_block(&mut self.doc.tree, &payload, &hash);
            let block_id = self.register(block)?;
            let caption = new_caption(&mut self.doc.tree, &block_id, cue.start, cue.end);
            entries.push((cue.end, self.register(caption)?));
        }
        let pairs: Vec<(i64, &str)> = entries.iter().map(|(e, id)| (*e, id.as_str())).collect();
        let collection = new_caption_collection(&mut self.doc.tree, &pairs);
        let collection_id = self.register(collection)?;
        let clip_uuid = uuids.uuid4().to_string();
        let clip = new_transcript_clip(
            &mut self.doc.tree,
            &markers_id,
            &source_id,
            &collection_id,
            label_index,
            label_color,
            &clip_uuid,
        );
        let clip_id = self.register(clip)?;
        let groups = new_channel_groups(&mut self.doc.tree);
        let groups_id = self.register(groups)?;
        let master_uid = uuids.uuid4().to_string();
        let master = new_master_clip(&mut self.doc.tree, &master_uid, &logging_id, &clip_id, &groups_id, &name);
        let master = self.register_uid(master, &master_uid)?;
        let item_uid = uuids.uuid4().to_string();
        let item_element = new_clip_item(&mut self.doc.tree, &item_uid, &master_uid, &name, label_index);
        let item_element = self.register_uid(item_element, &item_uid)?;
        Ok(self.attach_imported_item(item_element, &item_uid, master, &[clip_id.as_str()], &file_path, duration)?)
    }
}
