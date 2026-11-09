//! The rest of `Sequence` and `SequenceSettings` (port of `py_premiere/models/sequence.py`):
//! name setter, frame size and audio format, display formats, in/out/work area/playhead,
//! settings, `add_linked_clip`, the `NamedList` views and the reprs.
//!
//! Not ported here: `import_mgt` (Motion Graphics template port) and `create_caption_track`
//! (caption builder port).

use super::named_list::NamedList;
use super::project::py_int;
use super::validators::{validate_nonempty_string, validate_string};
use super::{ItemId, Project, SequenceId, TrackId, TrackItemId};
use crate::build::time::{Time, UNSET_TICKS};
use crate::convert::model::py_str_repr;
use crate::error::{type_err, value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::NodeId;

impl Project {
    fn seq_bag_element(&self, sequence: SequenceId, key: &str) -> Option<NodeId> {
        let tree = &self.doc.tree;
        tree.findall(self.sequence(sequence).element, "Node/Properties").ok()?.into_iter().find_map(|p| tree.child_by_tag(p, key))
    }

    /// `Sequence._bag_int`.
    pub fn sequence_bag_int(&self, sequence: SequenceId, key: &str) -> Result<Option<i64>> {
        match self.seq_bag_element(sequence, key).and_then(|e| self.doc.tree.text(e)) {
            Some(t) if !t.is_empty() => py_int(t).map(Some),
            _ => Ok(None),
        }
    }

    /// The raw text of a sequence property-bag key (`None` when absent or textless).
    pub fn sequence_bag_text(&self, sequence: SequenceId, key: &str) -> Option<String> {
        self.seq_bag_element(sequence, key).and_then(|e| self.doc.tree.text(e)).map(str::to_string)
    }

    /// `Sequence.name = value` (the `Name` element must exist).
    pub fn set_sequence_name(&mut self, sequence: SequenceId, name: &str) -> Result<()> {
        validate_string(name)?;
        let element = self.sequence(sequence).element;
        let Some(e) = self.doc.tree.child_by_tag(element, "Name") else {
            return value("cannot set name: <Name> is absent and its creation anchor is not defined for this field");
        };
        self.doc.tree.set_text(e, Some(name));
        Ok(())
    }

    /// `Sequence.frame_size`: `(width, height)` of the video group's `FrameRect`.
    pub fn frame_size(&self, sequence: SequenceId) -> Option<(i64, i64)> {
        self.sequence(sequence).frame_size
    }

    /// `Sequence.audio_frame_rate`: ticks per audio frame.
    pub fn audio_frame_rate(&self, sequence: SequenceId) -> Option<i64> {
        self.sequence(sequence).audio_frame_rate
    }

    /// `Sequence.audio_channel_count`.
    pub fn audio_channel_count(&self, sequence: SequenceId) -> Option<i64> {
        self.sequence(sequence).audio_channel_count
    }

    /// `Sequence.video_display_format`.
    pub fn video_display_format(&self, sequence: SequenceId) -> Result<Option<i64>> {
        self.sequence_bag_int(sequence, "MZ.Sequence.VideoTimeDisplayFormat")
    }

    /// `Sequence.video_display_format = value`.
    pub fn set_video_display_format(&mut self, sequence: SequenceId, format: i64) -> Result<()> {
        self.bag_write(sequence, "MZ.Sequence.VideoTimeDisplayFormat", &format.to_string())
    }

    /// `Sequence.audio_display_format`.
    pub fn audio_display_format(&self, sequence: SequenceId) -> Result<Option<i64>> {
        self.sequence_bag_int(sequence, "MZ.Sequence.AudioTimeDisplayFormat")
    }

    /// `Sequence.audio_display_format = value`.
    pub fn set_audio_display_format(&mut self, sequence: SequenceId, format: i64) -> Result<()> {
        self.bag_write(sequence, "MZ.Sequence.AudioTimeDisplayFormat", &format.to_string())
    }

    fn optional_point(&self, sequence: SequenceId, key: &str) -> Result<Option<Time>> {
        Ok(match self.sequence_bag_int(sequence, key)? {
            None => None,
            Some(t) if t == UNSET_TICKS => None,
            Some(t) => Some(Time::new(t)),
        })
    }

    /// `Sequence.in_point` (`None` when unset).
    pub fn sequence_in_point(&self, sequence: SequenceId) -> Result<Option<Time>> {
        self.optional_point(sequence, "MZ.InPoint")
    }

    /// `Sequence.out_point` (`None` when unset).
    pub fn sequence_out_point(&self, sequence: SequenceId) -> Result<Option<Time>> {
        self.optional_point(sequence, "MZ.OutPoint")
    }

    fn time_or_zero(&self, sequence: SequenceId, key: &str) -> Result<Time> {
        Ok(Time::new(self.sequence_bag_int(sequence, key)?.unwrap_or(0)))
    }

    /// `Sequence.work_area_in` (`MZ.WorkInPoint`).
    pub fn work_area_in(&self, sequence: SequenceId) -> Result<Time> {
        self.time_or_zero(sequence, "MZ.WorkInPoint")
    }

    /// `Sequence.work_area_in = value`.
    pub fn set_work_area_in(&mut self, sequence: SequenceId, t: Time) -> Result<()> {
        self.bag_write(sequence, "MZ.WorkInPoint", &t.ticks.to_string())
    }

    /// `Sequence.work_area_out` (`MZ.WorkOutPoint`).
    pub fn work_area_out(&self, sequence: SequenceId) -> Result<Time> {
        self.time_or_zero(sequence, "MZ.WorkOutPoint")
    }

    /// `Sequence.work_area_out = value`.
    pub fn set_work_area_out(&mut self, sequence: SequenceId, t: Time) -> Result<()> {
        self.bag_write(sequence, "MZ.WorkOutPoint", &t.ticks.to_string())
    }

    /// `Sequence.playhead` (`MZ.EditLine`).
    pub fn playhead(&self, sequence: SequenceId) -> Result<Time> {
        self.time_or_zero(sequence, "MZ.EditLine")
    }

    /// `Sequence.playhead = value`.
    pub fn set_playhead(&mut self, sequence: SequenceId, t: Time) -> Result<()> {
        self.bag_write(sequence, "MZ.EditLine", &t.ticks.to_string())
    }

    /// `Sequence.markers` as a `NamedList` (by marker name).
    pub fn sequence_markers_named(&self, sequence: SequenceId) -> NamedList<NodeId> {
        let items = self.sequence(sequence).markers.clone();
        let keys = items.iter().map(|&m| vec![super::marker::marker_name(&self.doc.tree, m)]).collect();
        NamedList::new(items, keys)
    }

    /// A track list as a `NamedList` (by `Video 1` / `Audio 2` names).
    pub fn tracks_named(&self, tracks: &[TrackId]) -> NamedList<TrackId> {
        let keys = tracks.iter().map(|&t| vec![Ok(self.track_name(t))]).collect();
        NamedList::new(tracks.to_vec(), keys)
    }

    /// A clip list as a `NamedList` (by track-item name).
    pub fn clips_named(&self, clips: &[TrackItemId]) -> NamedList<TrackItemId> {
        let keys = clips.iter().map(|&c| vec![self.track_item_name(c)]).collect();
        NamedList::new(clips.to_vec(), keys)
    }

    /// `Project.sequences` as a `NamedList` (by sequence name).
    pub fn sequences_named(&self) -> NamedList<SequenceId> {
        let items = self.sequences();
        let keys = items.iter().map(|&s| vec![Ok(self.sequence_name(s))]).collect();
        NamedList::new(items, keys)
    }

    /// `Sequence.add_linked_clip(item, start, video_track, audio_track)`: place both halves of
    /// an A/V item and link them. Returns `(video, audio)`.
    pub fn add_linked_clip(
        &mut self,
        sequence: SequenceId,
        item: ItemId,
        start: Time,
        video_track: i64,
        audio_track: i64,
        uuids: &mut dyn UuidSource,
    ) -> Result<(TrackItemId, TrackItemId)> {
        for index in [video_track, audio_track] {
            if index < 0 {
                return value(format!("must be >= 0, got {index}"));
            }
        }
        let videos = self.video_tracks(sequence).to_vec();
        let audios = self.audio_tracks(sequence).to_vec();
        if video_track as usize >= videos.len() {
            return value(format!("sequence has no video track {video_track}"));
        }
        if audio_track as usize >= audios.len() {
            return value(format!("sequence has no audio track {audio_track}"));
        }
        let tags = self.clip_element_tags(item);
        if !tags.iter().any(|t| t == "VideoClip") || !tags.iter().any(|t| t == "AudioClip") {
            return value("item has no video and audio pair to link");
        }
        let video = self.add_clip(videos[video_track as usize], item, start, uuids)?;
        let audio = self.add_clip(audios[audio_track as usize], item, start, uuids)?;
        self.link(sequence, &[video, audio])?;
        Ok((video, audio))
    }

    /// `repr(sequence)`.
    pub fn sequence_repr(&self, sequence: SequenceId) -> String {
        let data = self.sequence(sequence);
        format!(
            "Sequence(name={}, {}V/{}A)",
            py_str_repr(&self.sequence_name(sequence)),
            data.video_tracks.len(),
            data.audio_tracks.len()
        )
    }

    // ---- SequenceSettings ------------------------------------------------------------------

    fn settings_write(&mut self, sequence: SequenceId, key: &str, text: &str) -> Result<()> {
        if self.seq_bag_element(sequence, key).is_none() {
            return value(format!("sequence has no {key} property"));
        }
        self.bag_write(sequence, key, text)
    }

    /// `SequenceSettings.editing_mode`.
    pub fn settings_editing_mode(&self, sequence: SequenceId) -> Option<String> {
        self.sequence_bag_text(sequence, "MZ.Sequence.EditingModeGUID")
    }

    /// `SequenceSettings.editing_mode = value`.
    pub fn set_settings_editing_mode(&mut self, sequence: SequenceId, mode: &str) -> Result<()> {
        validate_nonempty_string(mode)?;
        self.settings_write(sequence, "MZ.Sequence.EditingModeGUID", mode)
    }

    /// `SequenceSettings.max_bit_depth`.
    pub fn settings_max_bit_depth(&self, sequence: SequenceId) -> bool {
        self.sequence_bag_text(sequence, "MZ.Sequence.PreviewUseMaxBitDepth").as_deref() == Some("true")
    }

    /// `SequenceSettings.max_bit_depth = value`.
    pub fn set_settings_max_bit_depth(&mut self, sequence: SequenceId, on: bool) -> Result<()> {
        self.settings_write(sequence, "MZ.Sequence.PreviewUseMaxBitDepth", if on { "true" } else { "false" })
    }

    /// `SequenceSettings.max_render_quality`.
    pub fn settings_max_render_quality(&self, sequence: SequenceId) -> bool {
        self.sequence_bag_text(sequence, "MZ.Sequence.PreviewUseMaxRenderQuality").as_deref() == Some("true")
    }

    /// `SequenceSettings.max_render_quality = value`.
    pub fn set_settings_max_render_quality(&mut self, sequence: SequenceId, on: bool) -> Result<()> {
        self.settings_write(sequence, "MZ.Sequence.PreviewUseMaxRenderQuality", if on { "true" } else { "false" })
    }

    /// `SequenceSettings.preview_codec`.
    pub fn settings_preview_codec(&self, sequence: SequenceId) -> Result<Option<i64>> {
        self.sequence_bag_int(sequence, "MZ.Sequence.PreviewRenderingPresetCodec")
    }

    /// `SequenceSettings.preview_codec = value`.
    pub fn set_settings_preview_codec(&mut self, sequence: SequenceId, codec: i64) -> Result<()> {
        self.settings_write(sequence, "MZ.Sequence.PreviewRenderingPresetCodec", &codec.to_string())
    }

    /// `SequenceSettings.preview_rendering_preset_path`.
    pub fn settings_preview_preset_path(&self, sequence: SequenceId) -> Option<String> {
        self.sequence_bag_text(sequence, "MZ.Sequence.PreviewRenderingPresetPath")
    }

    /// `SequenceSettings.preview_rendering_preset_path = value`.
    pub fn set_settings_preview_preset_path(&mut self, sequence: SequenceId, path: &str) -> Result<()> {
        validate_nonempty_string(path)?;
        self.settings_write(sequence, "MZ.Sequence.PreviewRenderingPresetPath", path)
    }

    /// `SequenceSettings.preview_frame_size`.
    pub fn settings_preview_frame_size(&self, sequence: SequenceId) -> Result<Option<(i64, i64)>> {
        let width = self.sequence_bag_int(sequence, "MZ.Sequence.PreviewFrameSizeWidth")?;
        let height = self.sequence_bag_int(sequence, "MZ.Sequence.PreviewFrameSizeHeight")?;
        Ok(width.zip(height))
    }

    /// `SequenceSettings.preview_frame_size = (width, height)` (positive integers; both keys
    /// must exist before either is written).
    pub fn set_settings_preview_frame_size(
        &mut self,
        sequence: SequenceId,
        size: (super::validators::Num, super::validators::Num),
    ) -> Result<()> {
        use super::validators::Num;
        super::validators::validate_vector2([size.0, size.1])?;
        let (Num::Int(width), Num::Int(height)) = size else {
            return type_err("preview frame size must be two integers");
        };
        if width <= 0 || height <= 0 {
            return value("preview frame size must be positive");
        }
        for key in ["MZ.Sequence.PreviewFrameSizeWidth", "MZ.Sequence.PreviewFrameSizeHeight"] {
            if self.seq_bag_element(sequence, key).is_none() {
                return Err(Error::Value(format!("sequence has no {key} property")));
            }
        }
        self.bag_write(sequence, "MZ.Sequence.PreviewFrameSizeWidth", &width.to_string())?;
        self.bag_write(sequence, "MZ.Sequence.PreviewFrameSizeHeight", &height.to_string())
    }

    /// `SequenceSettings.video_frame_rate`: ticks per video frame.
    pub fn settings_video_frame_rate(&self, sequence: SequenceId) -> Option<Time> {
        self.timebase(sequence).map(Time::new)
    }

    /// `SequenceSettings.audio_sample_rate`: ticks per audio sample.
    pub fn settings_audio_sample_rate(&self, sequence: SequenceId) -> Option<Time> {
        self.audio_frame_rate(sequence).map(Time::new)
    }

    /// `repr(sequence.settings)`.
    pub fn settings_repr(&self, sequence: SequenceId) -> String {
        let mode = match self.settings_editing_mode(sequence) {
            Some(m) => py_str_repr(&m),
            None => "None".into(),
        };
        let size = match self.frame_size(sequence) {
            Some((w, h)) => format!("({w}, {h})"),
            None => "None".into(),
        };
        format!("SequenceSettings(editing_mode={mode}, video_frame_size={size})")
    }
}
