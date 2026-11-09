//! Media import and what is built on it (port of py_premiere `models/project.py`
//! `import_files`, `_import_video`, `_import_audio`, `_import_still`, `_label`,
//! `_make_primary_media`, `_make_proxy_media`, and `models/project_item.py` `attach_proxy`,
//! `_attach_hi_res`, `_override_frame_rect`, `_check_proxy_aspect`, `create_sub_clip`,
//! `_drop_subclip_half`).
//!
//! Every UUID is drawn from the caller's single source in Python's order, objects are
//! registered in Python's order, and a refusal leaves the document exactly where Python's
//! exception leaves it (some refusals come after objects were registered, as in Python).

use std::path::Path;

use super::item::clip_core;
use super::preferences::Preferences;
use super::project::{py_int, py_path, resolve_path};
use super::validators::{validate_path, validate_string, PathRule};
use super::{ItemId, ItemType, Project};
use crate::build::media as m;
use crate::build::time::Time;
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::probe::{self, MovieProbe, TICKS_PER_SECOND};
use crate::xml::mutations::{append_child, append_leaf, insert_leaf_before, remove_child};
use crate::xml::{NodeId, ReferenceIndex};

/// Audio suffixes `import_files` routes to the audio path (`AUDIO_FORMATS`).
pub const AUDIO_FORMATS: [&str; 6] = [".wav", ".aiff", ".m4a", ".mp3", ".aac", ".wma"];
/// `MOVIE_SUFFIXES`.
pub const MOVIE_SUFFIXES: [&str; 4] = [".avi", ".mp4", ".mov", ".mxf"];

/// A Python int that the XML builders need as an `i64`. Python writes any size; a tick count
/// or frame size beyond `i64` (only from absurd headers) is refused here.
pub(crate) fn fit(v: i128, what: &str) -> Result<i64> {
    i64::try_from(v).map_err(|_| Error::Overflow(format!("{what} {v} does not fit in 64 bits")))
}

fn ticks_per_frame_for(sample_rate: i128) -> Result<i128> {
    probe::py_floordiv(TICKS_PER_SECOND, sample_rate)
}

fn file_name(path: &Path) -> String {
    let normal = py_path(&path.to_string_lossy());
    let text = normal.to_string_lossy().into_owned();
    match text.rfind('/') {
        Some(i) => text[i + 1..].to_string(),
        None if text == "." => String::new(),
        None => text,
    }
}

fn resolved(path: &Path) -> String {
    resolve_path(&py_path(&path.to_string_lossy())).to_string_lossy().into_owned()
}

/// `ProjectItem._split_rect`-style parse of `left,top,width,height`: `int(parts[2])`,
/// `int(parts[3])` with Python's errors.
fn rect_size(rect: &str) -> Result<(i128, i128)> {
    let parts: Vec<&str> = rect.split(',').collect();
    let at = |i: usize| -> Result<i128> {
        let part = parts.get(i).ok_or_else(|| Error::Index("list index out of range".into()))?;
        py_int(part).map(i128::from)
    };
    Ok((at(2)?, at(3)?))
}

/// `_check_proxy_aspect(one, other)`: Premiere refuses a proxy pair whose frame aspects differ.
pub fn check_proxy_aspect(one: &str, other: &str) -> Result<()> {
    let first: Vec<&str> = one.split(',').collect();
    let second: Vec<&str> = other.split(',').collect();
    let at = |parts: &Vec<&str>, i: usize| -> Result<i128> {
        let part = parts.get(i).ok_or_else(|| Error::Index("list index out of range".into()))?;
        py_int(part).map(i128::from)
    };
    let (width, height) = (at(&first, 2)?, at(&first, 3)?);
    let (other_width, other_height) = (at(&second, 2)?, at(&second, 3)?);
    if width * other_height != other_width * height {
        return value(format!(
            "proxy frame aspect ratio must match the source ({width}x{height} vs {other_width}x{other_height})"
        ));
    }
    Ok(())
}

/// `_override_frame_rect(stream, rect)`: the override pair after `OriginalColorSpace`.
fn override_frame_rect(tree: &mut crate::xml::Tree, stream: NodeId, rect: &str) -> Result<()> {
    let children: Vec<String> = tree.children(stream).iter().map(|&c| tree.tag(c).to_string()).collect();
    if children.iter().any(|t| t == "IsFrameRectOverridden") {
        return Ok(());
    }
    insert_override(tree, stream, &children, rect)
}

fn insert_override(tree: &mut crate::xml::Tree, stream: NodeId, children: &[String], rect: &str) -> Result<()> {
    let Some(anchor) = children.iter().position(|t| t == "OriginalColorSpace") else {
        return value("'OriginalColorSpace' is not in list");
    };
    if anchor + 1 < children.len() {
        let following = children[anchor + 1].clone();
        insert_leaf_before(tree, stream, &following, "IsFrameRectOverridden", "true")?;
        insert_leaf_before(tree, stream, &following, "OverriddenFrameRect", rect)?;
    } else {
        append_leaf(tree, stream, "IsFrameRectOverridden", "true");
        append_leaf(tree, stream, "OverriddenFrameRect", rect);
    }
    Ok(())
}

impl Project {
    /// `Project._label(preferences, kind, default_index, default_color)`.
    fn import_label(preferences: Option<&Preferences>, kind: &str, index: i64, color: i64) -> Result<(i64, i64)> {
        let Some(preferences) = preferences else { return Ok((index, color)) };
        match preferences.label_default(kind)? {
            None => Ok((index, color)),
            Some(i) => Ok((i as i64, preferences.label_colors()?[i])),
        }
    }

    /// The preferences `import_files` reads: `Preferences(preferences_path)` when the project
    /// has one, else the machine's own (`Preferences.load_default()`).
    pub fn import_preferences(&self) -> Result<Option<Preferences>> {
        match &self.preferences_path {
            Some(path) => Preferences::new(path).map(Some),
            None => Preferences::load_default(),
        }
    }

    /// `Project.import_files(paths, target_bin)`: import media files into the project panel
    /// (into `target_bin`, default the root). Stills (BMP/PNG/JPEG/GIF/TIFF/PSD), audio
    /// (WAV/AIFF/M4A/MP3/AAC/WMA), movies (AVI/MP4/MOV/MXF) and SRT captions. Reads the import
    /// defaults once per call ([`import_preferences`](Self::import_preferences)). On an error
    /// the files imported before it stay imported, as in Python.
    pub fn import_files<P: AsRef<Path>>(
        &mut self,
        paths: &[P],
        target_bin: Option<ItemId>,
        uuids: &mut dyn UuidSource,
    ) -> Result<Vec<ItemId>> {
        self.check_target_bin(target_bin)?;
        let preferences = self.import_preferences()?;
        self.import_files_with(paths, target_bin, preferences.as_ref(), uuids)
    }

    fn check_target_bin(&self, target_bin: Option<ItemId>) -> Result<()> {
        if let Some(bin) = target_bin {
            if self.item(bin).item_type != ItemType::Bin {
                return value("target_bin must be a bin");
            }
        }
        Ok(())
    }

    /// [`import_files`](Self::import_files) with the preferences given (`None`: factory
    /// defaults, what Python does on a machine where Premiere never ran).
    pub fn import_files_with<P: AsRef<Path>>(
        &mut self,
        paths: &[P],
        target_bin: Option<ItemId>,
        preferences: Option<&Preferences>,
        uuids: &mut dyn UuidSource,
    ) -> Result<Vec<ItemId>> {
        self.check_target_bin(target_bin)?;
        let previous = std::mem::replace(&mut self.import_target, target_bin);
        let mut items = Vec::new();
        let mut result = Ok(());
        for path in paths {
            let path = py_path(&path.as_ref().to_string_lossy());
            let suffix = probe::py_suffix_lower(&path);
            let imported = if AUDIO_FORMATS.contains(&suffix.as_str()) {
                self.import_audio(&path, &suffix, preferences, uuids)
            } else if MOVIE_SUFFIXES.contains(&suffix.as_str()) {
                self.import_video(&path, preferences, uuids)
            } else if suffix == ".srt" {
                self.import_srt(&path, preferences, uuids)
            } else {
                self.import_still(&path, &suffix, preferences, uuids)
            };
            match imported {
                Ok(item) => items.push(item),
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
        }
        self.import_target = previous;
        result.map(|_| items)
    }

    fn import_srt(&mut self, path: &Path, preferences: Option<&Preferences>, uuids: &mut dyn UuidSource) -> Result<ItemId> {
        use crate::caption::builder::{CAPTIONS_LABEL_COLOR, CAPTIONS_LABEL_INDEX};
        let label = Self::import_label(preferences, "Captions", CAPTIONS_LABEL_INDEX, CAPTIONS_LABEL_COLOR)?;
        let target = self.import_target;
        Ok(self.import_caption(path, target, Some(label), uuids)?)
    }

    fn project_dir(&self) -> String {
        resolve_path(self.path()).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into())
    }

    /// `Project._import_video`.
    fn import_video(&mut self, path: &Path, preferences: Option<&Preferences>, uuids: &mut dyn UuidSource) -> Result<ItemId> {
        validate_path(path, PathRule::MustExist { must_be_file: true })?;
        let probe = probe::probe_movie(path)?;
        let (label_index, label_color) = if probe.audio.is_none() {
            Self::import_label(preferences, "Video", m::VIDEO_LABEL_INDEX, m::VIDEO_LABEL_COLOR)?
        } else {
            Self::import_label(preferences, "AV", m::AV_LABEL_INDEX, m::AV_LABEL_COLOR)?
        };
        let name = file_name(path);
        let file_path = resolved(path);
        let project_dir = self.project_dir();
        let content_state = uuids.uuid4().to_string();
        let geometry = MovieGeometry::of(&probe)?;
        let stream = self.movie_stream(&probe, &geometry)?;
        let stream_id = self.register(stream)?;
        let mut audio_stream_id = None;
        let mut audio_rate = None;
        if let Some(audio) = &probe.audio {
            let rate = fit(ticks_per_frame_for(audio.sample_rate)?, "audio frame rate")?;
            audio_rate = Some(rate);
            let layout = m::audio_channel_layout(channel_count(audio.channels)?);
            let s = m::new_audio_stream(&mut self.doc.tree, rate, &layout, geometry.duration, Some(m::AAC_SAMPLE_TYPE));
            audio_stream_id = Some(self.register(s)?);
        }
        let media_uid = uuids.uuid4().to_string();
        let media = m::new_movie_media(
            &mut self.doc.tree,
            uuids,
            &media_uid,
            &stream_id,
            &file_path,
            &project_dir,
            &content_state,
            audio_stream_id.as_deref(),
            audio_rate,
            geometry.start_ticks,
        );
        let media = self.register_uid(media, &media_uid)?;
        self.imported_media.push(media);
        let source = m::new_movie_media_source(&mut self.doc.tree, &media_uid, geometry.duration);
        let source_id = self.register(source)?;
        let markers = m::new_markers_collection(&mut self.doc.tree, &content_state);
        let markers_id = self.register(markers)?;
        let logging = m::new_media_logging_info(
            &mut self.doc.tree,
            &name,
            geometry.frame_rate,
            geometry.duration,
            if probe.audio.is_none() { Some("2") } else { None },
            &probe.timecode_format.to_string(),
            geometry.start_ticks,
            &probe.clip_id,
        );
        let logging_id = self.register(logging)?;
        let clip = m::new_movie_template_clip(&mut self.doc.tree, uuids, &markers_id, &source_id, label_index, label_color);
        let mut clip_ids = vec![self.register(clip)?];
        let master_uid = uuids.uuid4().to_string();
        let master_element = match &probe.audio {
            None => {
                let groups = m::new_channel_groups(&mut self.doc.tree);
                let groups_id = self.register(groups)?;
                m::new_master_clip(&mut self.doc.tree, &master_uid, &logging_id, &clip_ids[0], &groups_id, &name)
            }
            Some(audio) => {
                let Some(channel_type) = u32::try_from(audio.channels).ok().and_then(m::channel_type) else {
                    return Err(Error::NotImplemented(format!(
                        "{}-channel movie audio is not supported",
                        audio.channels
                    )));
                };
                let channels = audio.channels as u32;
                let audio_source = m::new_audio_media_source(&mut self.doc.tree, &media_uid, geometry.duration);
                let audio_source_id = self.register(audio_source)?;
                let mut secondary_ids = Vec::new();
                for channel in 0..channels {
                    let s = m::new_secondary_content(&mut self.doc.tree, &audio_source_id, channel as i64);
                    secondary_ids.push(self.register(s)?);
                }
                let layout = m::audio_channel_layout(channels);
                let refs: Vec<&str> = secondary_ids.iter().map(String::as_str).collect();
                let audio_clip = m::new_audio_template_clip(
                    &mut self.doc.tree,
                    uuids,
                    &markers_id,
                    &audio_source_id,
                    &refs,
                    label_index,
                    label_color,
                    &layout,
                );
                clip_ids.push(self.register(audio_clip)?);
                let mut channel_ids = Vec::new();
                for channel in 0..channels {
                    let s = m::new_channel_serializer(&mut self.doc.tree, 0, channel as i64);
                    channel_ids.push(self.register(s)?);
                }
                let refs: Vec<&str> = channel_ids.iter().map(String::as_str).collect();
                let vector = m::new_channel_vector(&mut self.doc.tree, &refs, channel_type);
                let vector_id = self.register(vector)?;
                let groups = m::new_audio_channel_groups(&mut self.doc.tree, &[&vector_id]);
                let groups_id = self.register(groups)?;
                let chain = m::new_master_audio_chain(&mut self.doc.tree, channels, channel_type);
                let chain_id = self.register(chain)?;
                let clip_refs: Vec<&str> = clip_ids.iter().map(String::as_str).collect();
                m::new_audio_master_clip(
                    &mut self.doc.tree,
                    uuids,
                    &master_uid,
                    &logging_id,
                    &[&chain_id],
                    &clip_refs,
                    &groups_id,
                    &name,
                )
            }
        };
        let master = self.register_uid(master_element, &master_uid)?;
        let item_uid = uuids.uuid4().to_string();
        let item_element = m::new_clip_item(&mut self.doc.tree, &item_uid, &master_uid, &name, label_index);
        let item_element = self.register_uid(item_element, &item_uid)?;
        let clip_refs: Vec<&str> = clip_ids.iter().map(String::as_str).collect();
        self.attach_imported_item(item_element, &item_uid, master, &clip_refs, &file_path, geometry.duration)
    }

    /// `new_movie_video_stream` for a probe (Python refuses an unverified codec here).
    fn movie_stream(&mut self, probe: &MovieProbe, geometry: &MovieGeometry) -> Result<NodeId> {
        m::new_movie_video_stream(
            &mut self.doc.tree,
            geometry.width,
            geometry.height,
            geometry.frame_rate,
            geometry.duration,
            probe.container,
            &probe.codec,
            fit(probe.field_order, "field order")?,
        )
    }

    /// `Project._import_audio`.
    fn import_audio(
        &mut self,
        path: &Path,
        suffix: &str,
        preferences: Option<&Preferences>,
        uuids: &mut dyn UuidSource,
    ) -> Result<ItemId> {
        validate_path(path, PathRule::MustExist { must_be_file: true })?;
        let data = std::fs::read(path)?;
        let info = probe::read_audio_info(&data, suffix)?;
        let sample_type: Option<&str> = match m::compressed_sample_type(suffix) {
            Some(t) => Some(t),
            None => {
                let format = (u32::try_from(info.format_tag).ok(), u32::try_from(info.sample_width).ok());
                let found = match format {
                    (Some(tag), Some(width)) => {
                        if suffix == ".aiff" {
                            m::aiff_sample_type(tag, width)
                        } else {
                            m::wav_sample_type(tag, width)
                        }
                    }
                    _ => None,
                };
                match found {
                    Some(t) => t,
                    None => {
                        return Err(Error::NotImplemented(format!(
                            "unsupported {suffix} sample format: tag {}, {}-bit",
                            info.format_tag,
                            info.sample_width * 8
                        )))
                    }
                }
            }
        };
        let frame_rate = fit(ticks_per_frame_for(info.sample_rate)?, "audio frame rate")?;
        let duration = fit(info.frames * frame_rate as i128, "audio duration")?;
        let (label_index, label_color) =
            Self::import_label(preferences, "Audio", m::AUDIO_LABEL_INDEX, m::AUDIO_LABEL_COLOR)?;
        // Native channel counts are one source clip; any other count is one mono clip per
        // channel. Each group maps its channels to (source clip, channel).
        let native = u32::try_from(info.channels).ok().and_then(m::channel_type);
        let channel_total = u32::try_from(info.channels)
            .map_err(|_| Error::Overflow(format!("channel count {} does not fit in 32 bits", info.channels)))?;
        let (clip_channel_counts, groups): (Vec<u32>, Vec<ChannelGroup>) = match native {
            None => (
                vec![1; channel_total as usize],
                (0..channel_total as i64).map(|index| (0, vec![(index, 0)])).collect(),
            ),
            Some(channel_type) => {
                (vec![channel_total], vec![(channel_type, (0..channel_total as i64).map(|c| (0, c)).collect())])
            }
        };
        let name = file_name(path);
        let file_path = resolved(path);
        let project_dir = self.project_dir();
        let content_state = uuids.uuid4().to_string();
        let file_key = uuids.uuid4().to_string();
        let binary_hash = uuids.uuid4().to_string();
        let logging = m::new_media_logging_info(&mut self.doc.tree, &name, frame_rate, duration, Some("1"), "200", 0, "");
        let logging_id = self.register(logging)?;
        let mut chain_ids = Vec::new();
        let mut clip_ids = Vec::new();
        for (stream_number, &channels) in clip_channel_counts.iter().enumerate() {
            let layout = m::audio_channel_layout(channels);
            let stream = m::new_audio_stream(&mut self.doc.tree, frame_rate, &layout, duration, sample_type);
            let stream_id = self.register(stream)?;
            let media_uid = uuids.uuid4().to_string();
            let media = m::new_audio_media(
                &mut self.doc.tree,
                &media_uid,
                &stream_id,
                &file_path,
                &project_dir,
                &content_state,
                frame_rate,
                &file_key,
                &binary_hash,
                stream_number as i64,
            );
            let media = self.register_uid(media, &media_uid)?;
            self.imported_media.push(media);
            let source = m::new_audio_media_source(&mut self.doc.tree, &media_uid, duration);
            let source_id = self.register(source)?;
            let mut secondary_ids = Vec::new();
            for channel in 0..channels {
                let s = m::new_secondary_content(&mut self.doc.tree, &source_id, channel as i64);
                secondary_ids.push(self.register(s)?);
            }
            let markers = m::new_markers_collection(&mut self.doc.tree, &content_state);
            let markers_id = self.register(markers)?;
            let channel_type = m::channel_type(channels).expect("1, 2 and 6 are native");
            let chain = m::new_master_audio_chain(&mut self.doc.tree, channels, channel_type);
            chain_ids.push(self.register(chain)?);
            let refs: Vec<&str> = secondary_ids.iter().map(String::as_str).collect();
            let clip = m::new_audio_template_clip(
                &mut self.doc.tree,
                uuids,
                &markers_id,
                &source_id,
                &refs,
                label_index,
                label_color,
                &layout,
            );
            clip_ids.push(self.register(clip)?);
        }
        let mut vector_ids = Vec::new();
        for (group_type, group_channels) in &groups {
            let mut channel_ids = Vec::new();
            for &(source_clip, channel) in group_channels {
                let s = m::new_channel_serializer(&mut self.doc.tree, source_clip, channel);
                channel_ids.push(self.register(s)?);
            }
            let refs: Vec<&str> = channel_ids.iter().map(String::as_str).collect();
            let vector = m::new_channel_vector(&mut self.doc.tree, &refs, *group_type);
            vector_ids.push(self.register(vector)?);
        }
        let refs: Vec<&str> = vector_ids.iter().map(String::as_str).collect();
        let groups_element = m::new_audio_channel_groups(&mut self.doc.tree, &refs);
        let groups_id = self.register(groups_element)?;
        let master_uid = uuids.uuid4().to_string();
        let chain_refs: Vec<&str> = chain_ids.iter().map(String::as_str).collect();
        let clip_refs: Vec<&str> = clip_ids.iter().map(String::as_str).collect();
        let master = m::new_audio_master_clip(
            &mut self.doc.tree,
            uuids,
            &master_uid,
            &logging_id,
            &chain_refs,
            &clip_refs,
            &groups_id,
            &name,
        );
        let master = self.register_uid(master, &master_uid)?;
        let item_uid = uuids.uuid4().to_string();
        let item_element = m::new_clip_item(&mut self.doc.tree, &item_uid, &master_uid, &name, label_index);
        let item_element = self.register_uid(item_element, &item_uid)?;
        self.attach_imported_item(item_element, &item_uid, master, &clip_refs, &file_path, duration)
    }

    /// `Project._import_still`.
    fn import_still(
        &mut self,
        path: &Path,
        suffix: &str,
        preferences: Option<&Preferences>,
        uuids: &mut dyn UuidSource,
    ) -> Result<ItemId> {
        validate_path(path, PathRule::MustExist { must_be_file: true })?;
        let Some(profile) = m::still_profile(suffix) else {
            return Err(Error::NotImplemented("supported stills are .bmp, .gif, .jpg, .png, .psd, .tif".into()));
        };
        let data = std::fs::read(path)?;
        let (width, height) = match suffix {
            ".bmp" => probe::read_bmp_size(&data)?,
            ".jpg" => probe::read_jpeg_size(&data)?,
            ".gif" => probe::read_gif_size(&data)?,
            ".psd" => probe::read_psd_size(&data)?,
            ".tif" => probe::read_tiff_size(&data)?,
            _ => probe::read_png_size(&data)?,
        };
        let (width, height) = (fit(width, "frame width")?, fit(height, "frame height")?);
        let mut frame_rate = m::STILL_FRAME_RATE;
        let mut out_ticks = m::STILL_DEFAULT_OUT;
        if let Some(p) = preferences {
            if let Some(rate) = p.still_frame_rate()?.filter(|&r| r != 0) {
                frame_rate = rate;
            }
            if let Some(out) = p.still_default_out_ticks()?.filter(|&o| o != 0) {
                out_ticks = out;
            }
        }
        let (label_index, label_color) =
            Self::import_label(preferences, "Still", m::STILL_LABEL_INDEX, m::STILL_LABEL_COLOR)?;
        let name = file_name(path);
        let file_path = resolved(path);
        let content_state = uuids.uuid4().to_string();
        let stream = m::new_video_stream(&mut self.doc.tree, width, height, frame_rate, profile);
        let stream_id = self.register(stream)?;
        let media_uid = uuids.uuid4().to_string();
        let project_dir = self.project_dir();
        let media = m::new_media(&mut self.doc.tree, uuids, &media_uid, &stream_id, &file_path, &project_dir, &content_state);
        let media = self.register_uid(media, &media_uid)?;
        self.imported_media.push(media);
        let source = m::new_media_source(&mut self.doc.tree, &media_uid);
        let source_id = self.register(source)?;
        let markers = m::new_markers_collection(&mut self.doc.tree, &content_state);
        let markers_id = self.register(markers)?;
        let logging = m::new_logging_info(&mut self.doc.tree, &name, frame_rate);
        let logging_id = self.register(logging)?;
        let groups = m::new_channel_groups(&mut self.doc.tree);
        let groups_id = self.register(groups)?;
        let clip = m::new_template_clip(&mut self.doc.tree, uuids, &markers_id, &source_id, label_index, label_color, out_ticks);
        let clip_id = self.register(clip)?;
        let master_uid = uuids.uuid4().to_string();
        let master = m::new_master_clip(&mut self.doc.tree, &master_uid, &logging_id, &clip_id, &groups_id, &name);
        let master = self.register_uid(master, &master_uid)?;
        let item_uid = uuids.uuid4().to_string();
        let item_element = m::new_clip_item(&mut self.doc.tree, &item_uid, &master_uid, &name, label_index);
        let item_element = self.register_uid(item_element, &item_uid)?;
        self.attach_imported_item(item_element, &item_uid, master, &[&clip_id], &file_path, m::STILL_DURATION)
    }

    // ---- proxies ---------------------------------------------------------------------------

    /// `Project._make_primary_media(path, own_rect)`: a plain `Media` + `VideoStream` for a
    /// hi-res attach; returns `(uid, frame rect)`.
    fn make_primary_media(&mut self, path: &Path, own_rect: &str, uuids: &mut dyn UuidSource) -> Result<(String, String)> {
        let probe = probe::probe_movie(path)?;
        if probe.audio.is_some() {
            return Err(Error::NotImplemented(
                "audio-carrying replacement media is not supported (no reference)".into(),
            ));
        }
        let rect = format!("0,0,{},{}", probe.width, probe.height);
        check_proxy_aspect(own_rect, &rect)?;
        let geometry = MovieGeometry::of(&probe)?;
        let stream = self.movie_stream(&probe, &geometry)?;
        let stream_id = self.register(stream)?;
        let media_uid = uuids.uuid4().to_string();
        let file_path = resolved(path);
        let project_dir = self.project_dir();
        let content_state = uuids.uuid4().to_string();
        let media = m::new_movie_media(
            &mut self.doc.tree,
            uuids,
            &media_uid,
            &stream_id,
            &file_path,
            &project_dir,
            &content_state,
            None,
            None,
            geometry.start_ticks,
        );
        let media = self.register_uid(media, &media_uid)?;
        self.imported_media.push(media);
        Ok((media_uid, rect))
    }

    /// `Project._make_proxy_media(path, hires_rect)`: a proxy's `Media` + `VideoStream`
    /// (the hi-res rect as an override, `IsProxy` last); returns the media UID.
    fn make_proxy_media(&mut self, path: &Path, hires_rect: &str, uuids: &mut dyn UuidSource) -> Result<String> {
        let probe = probe::probe_movie(path)?;
        if probe.audio.is_some() {
            return Err(Error::NotImplemented("audio-carrying proxy media is not supported (no reference)".into()));
        }
        let (hires_width, hires_height) = rect_size(hires_rect)?;
        if probe.width * hires_height != hires_width * probe.height {
            return value(format!(
                "proxy frame aspect ratio must match the source ({}x{} vs {hires_width}x{hires_height})",
                probe.width, probe.height
            ));
        }
        let geometry = MovieGeometry::of(&probe)?;
        let stream = self.movie_stream(&probe, &geometry)?;
        let children: Vec<String> =
            self.doc.tree.children(stream).iter().map(|&c| self.doc.tree.tag(c).to_string()).collect();
        insert_override(&mut self.doc.tree, stream, &children, hires_rect)?;
        let stream_id = self.register(stream)?;
        let media_uid = uuids.uuid4().to_string();
        let file_path = resolved(path);
        let project_dir = self.project_dir();
        let content_state = uuids.uuid4().to_string();
        let media = m::new_movie_media(
            &mut self.doc.tree,
            uuids,
            &media_uid,
            &stream_id,
            &file_path,
            &project_dir,
            &content_state,
            None,
            None,
            geometry.start_ticks,
        );
        append_leaf(&mut self.doc.tree, media, "IsProxy", "true");
        let media = self.register_uid(media, &media_uid)?;
        self.imported_media.push(media);
        Ok(media_uid)
    }

    /// `ProjectItem.attach_proxy(path, is_hi_res)`: attach proxy media to a video item (a
    /// second `Media` flagged `IsProxy`, referenced from the media source's `Content`). With
    /// `is_hi_res` the new file becomes what the item plays and the old media the proxy.
    pub fn attach_proxy(&mut self, item: ItemId, path: impl AsRef<Path>, is_hi_res: bool, uuids: &mut dyn UuidSource) -> Result<()> {
        let path = path.as_ref();
        validate_path(path, PathRule::MustExist { must_be_file: true })?;
        if self.has_proxy(item)? {
            return value("item already has proxy media attached");
        }
        let content = self.media_content(item)?;
        let media = self.media_element(item)?;
        let stream_ref = media.and_then(|m| self.doc.tree.child_by_tag(m, "VideoStream"));
        let (Some(content), Some(media), Some(stream_ref)) = (content, media, stream_ref) else {
            return value("proxies attach to video media clip items");
        };
        let stream = self.doc.resolve(stream_ref)?;
        let own_rect = self.doc.tree.findtext(stream, "FrameRect")?.unwrap_or_default();
        if own_rect.is_empty() {
            return value("source video stream has no frame rect");
        }
        let path = py_path(&path.to_string_lossy());
        if !is_hi_res {
            let proxy_uid = self.make_proxy_media(&path, &own_rect, uuids)?;
            let element = self.doc.tree.new_element_with("ProxyMedia", &[("ObjectURef", &proxy_uid)]);
            append_child(&mut self.doc.tree, content, element, None);
            return Ok(());
        }
        self.attach_hi_res(item, content, media, &own_rect, &path, uuids)
    }

    /// `ProjectItem._attach_hi_res`.
    fn attach_hi_res(
        &mut self,
        item: ItemId,
        content: NodeId,
        media: NodeId,
        own_rect: &str,
        path: &Path,
        uuids: &mut dyn UuidSource,
    ) -> Result<()> {
        let source = self.media_source(item)?;
        let Some(reference) = source.and_then(|s| self.doc.tree.find1(s, "MediaSource/Media")) else {
            return value("media source has no Media reference");
        };
        let (media_uid, hires_rect) = self.make_primary_media(path, own_rect, uuids)?;
        self.doc.tree.set(reference, "ObjectURef", media_uid);
        let Some(stream_ref) = self.doc.tree.child_by_tag(media, "VideoStream") else {
            return value("media has no video stream to override");
        };
        let stream = self.doc.resolve(stream_ref)?;
        override_frame_rect(&mut self.doc.tree, stream, &hires_rect)?;
        append_leaf(&mut self.doc.tree, media, "IsProxy", "true");
        let demoted = self.doc.tree.get(media, "ObjectUID").unwrap_or("").to_string();
        let element = self.doc.tree.new_element_with("ProxyMedia", &[("ObjectURef", &demoted)]);
        append_child(&mut self.doc.tree, content, element, None);
        if let Some(master) = self.item(item).master {
            if let Some(name) = self.doc.tree.child_by_tag(master, "Name") {
                self.doc.tree.set_text(name, Some(file_name(path)));
            }
        }
        let resolved_path = resolve_path(path).to_string_lossy().into_owned();
        self.item_mut(item).media_path = Some(resolved_path);
        Ok(())
    }

    // ---- subclips --------------------------------------------------------------------------

    /// `ProjectItem.create_sub_clip(name, start, end, has_hard_boundaries, take_video,
    /// take_audio)`: a second master clip over the same media (synthesized like an import, so
    /// the file must still be readable), narrowed by boundaries on its media source(s), sharing
    /// the source's file identity. Filed at the panel root.
    #[allow(clippy::too_many_arguments)]
    pub fn create_sub_clip(
        &mut self,
        item: ItemId,
        name: &str,
        start: Time,
        end: Time,
        has_hard_boundaries: bool,
        take_video: bool,
        take_audio: bool,
        uuids: &mut dyn UuidSource,
    ) -> Result<ItemId> {
        validate_string(name)?;
        if !(take_video || take_audio) {
            return value("a subclip must take the video, the audio, or both");
        }
        if self.item(item).item_type != ItemType::Clip || self.is_sequence_item(item) {
            return value("subclips can only be created from media clip items");
        }
        let Some(media_path) = self.item(item).media_path.clone() else {
            return value("item has no media file to subclip");
        };
        if !(0 <= start.ticks && start.ticks < end.ticks) {
            return value("subclip boundaries must satisfy 0 <= start < end");
        }
        if self.item(item).master.is_none() {
            return value("item has no master clip");
        }
        let created = self.import_files(&[media_path], None, uuids)?[0];
        let Some(master) = self.item(created).master else {
            return value("synthesized subclip has no master clip");
        };
        if let Some(name_element) = self.doc.tree.child_by_tag(master, "Name") {
            self.doc.tree.set_text(name_element, Some(name));
        }
        let Some(logging_ref) = self.doc.tree.child_by_tag(master, "LoggingInfo") else {
            return value("synthesized subclip has no logging info");
        };
        let logging = self.doc.resolve(logging_ref)?;
        for (tag, text) in
            [("ClipName", name.to_string()), ("MediaInPoint", start.ticks.to_string()), ("MediaOutPoint", end.ticks.to_string())]
        {
            if let Some(leaf) = self.doc.tree.child_by_tag(logging, tag) {
                self.doc.tree.set_text(leaf, Some(text));
            }
        }
        if !(take_video && take_audio) {
            self.drop_subclip_half(created, take_video)?;
        }
        let mut contents = Vec::new();
        for clip in self.item(created).clip_elements.clone() {
            let Some(core) = clip_core(&self.doc.tree, clip) else { continue };
            let Some(source_ref) = self.doc.tree.child_by_tag(core, "Source") else { continue };
            let source = self.doc.resolve(source_ref)?;
            if let Some(content) = self.doc.tree.find1(source, "MediaSource/Content") {
                contents.push(content);
            }
        }
        if contents.is_empty() {
            return value("synthesized subclip has no media content bag");
        }
        for content in contents {
            append_leaf(&mut self.doc.tree, content, "StartBoundary", &start.ticks.to_string());
            append_leaf(&mut self.doc.tree, content, "EndBoundary", &end.ticks.to_string());
            append_leaf(&mut self.doc.tree, content, "BoundariesAreHard", if has_hard_boundaries { "true" } else { "false" });
        }
        self.share_file_identity(item, created)?;
        Ok(created)
    }

    /// `ProjectItem._drop_subclip_half(keep_video)`: keep only the video or only the audio half
    /// of a freshly imported A/V master; what only the dropped clip used goes with it.
    fn drop_subclip_half(&mut self, item: ItemId, keep_video: bool) -> Result<()> {
        let Some(master) = self.item(item).master else { return value("item has no master clip") };
        let wanted = if keep_video { "VideoClip" } else { "AudioClip" };
        let Some(clips) = self.doc.tree.child_by_tag(master, "Clips") else {
            return value("master clip has no Clips list");
        };
        let mut doomed = Vec::new();
        for reference in self.doc.tree.findall(clips, "Clip")? {
            let clip = self.doc.resolve(reference)?;
            if self.doc.tree.tag(clip) == wanted {
                continue;
            }
            doomed.push(clip);
            remove_child(&mut self.doc.tree, clips, reference)?;
        }
        if doomed.is_empty() {
            return Ok(());
        }
        for (index, reference) in self.doc.tree.findall(clips, "Clip")?.into_iter().enumerate() {
            self.doc.tree.set(reference, "Index", index.to_string());
        }
        let mut orphaned = Vec::new();
        if keep_video {
            if let Some(chains) = self.doc.tree.child_by_tag(master, "AudioComponentChains") {
                for entry in self.doc.tree.children(chains).to_vec() {
                    orphaned.push(self.doc.resolve(entry)?);
                }
                remove_child(&mut self.doc.tree, master, chains)?;
            }
        }
        let index = ReferenceIndex::new(&self.doc);
        doomed.extend(orphaned.into_iter().filter(|chain| index.referrers.get(chain).is_none_or(|s| s.is_empty())));
        for element in self.doc.owned_objects(&doomed, Some(&index)) {
            self.doc.remove_object(element)?;
        }
        let clip_elements = self.resolve_all(clips, "Clip")?;
        self.item_mut(item).clip_elements = clip_elements;
        Ok(())
    }
}

/// One channel group of an audio import: its `ChannelType` and its (source clip, channel)
/// pairs.
type ChannelGroup = (i64, Vec<(i64, i64)>);

/// A probe's numbers as the builders take them.
struct MovieGeometry {
    width: i64,
    height: i64,
    frame_rate: i64,
    duration: i64,
    start_ticks: i64,
}

impl MovieGeometry {
    fn of(probe: &MovieProbe) -> Result<Self> {
        let geometry = MovieGeometry {
            width: fit(probe.width, "frame width")?,
            height: fit(probe.height, "frame height")?,
            frame_rate: fit(probe.frame_rate, "frame rate")?,
            duration: fit(probe.duration, "duration")?,
            start_ticks: fit(probe.start_ticks, "start")?,
        };
        fit(probe.start_ticks + probe.duration, "media out point")?;
        Ok(geometry)
    }
}

/// A movie audio channel count as the layout builders take it.
fn channel_count(channels: i128) -> Result<u32> {
    u32::try_from(channels).map_err(|_| Error::Overflow(format!("channel count {channels} does not fit in 32 bits")))
}
