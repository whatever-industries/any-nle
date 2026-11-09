//! Construct project items for Avid OP-Atom MXF files without reading the media (port of
//! `avid2prproj/opatom.py`). Shapes follow Premiere 26.5.2's own import of one Avid clip
//! (V01 DNx + A01/A02 mono), research finding PRPROJ-0009. The element builders live in
//! [`crate::build::media`] / [`crate::build::media::opatom`]; this module composes them with the
//! project's registration calls in Python's order, drawing UUIDs from the write's one source.

use std::collections::HashMap;
use std::fmt;

use crate::build::media::{self as m, opatom as op};
use crate::convert::model::{ticks_per_unit, AudioFile, Clip, Fraction, VideoFile, TICKS_PER_SECOND};
use crate::error::Error;
use crate::ids::UuidSource;
use crate::model::project::resolve_path;
use crate::model::{ItemId, Project};

/// `UnverifiedShape` (a `NotImplementedError`): a media shape Premiere has not been observed
/// to write for OP-Atom yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnverifiedShape(pub String);

impl fmt::Display for UnverifiedShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UnverifiedShape {}

/// What the item builders can fail with: [`UnverifiedShape`], or a document error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpatomError {
    Unverified(UnverifiedShape),
    Other(Error),
}

impl fmt::Display for OpatomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpatomError::Unverified(e) => e.fmt(f),
            OpatomError::Other(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for OpatomError {}

impl From<Error> for OpatomError {
    fn from(e: Error) -> Self {
        OpatomError::Other(e)
    }
}

impl From<UnverifiedShape> for OpatomError {
    fn from(e: UnverifiedShape) -> Self {
        OpatomError::Unverified(e)
    }
}

impl From<OpatomError> for Error {
    fn from(e: OpatomError) -> Self {
        match e {
            OpatomError::Unverified(u) => Error::Value(u.0),
            OpatomError::Other(e) => e,
        }
    }
}

/// `_any_nle_media`: the per-project side table that keeps one `Media` object per media file,
/// as Premiere's own import does. Python hangs it on the `Project`; here the writer owns one
/// table per [`Project`] and passes it to every `add_*_item` call.
#[derive(Clone, Debug, Default)]
pub struct MediaTable {
    /// media path -> (media uid, content state)
    media: HashMap<String, (String, String)>,
    pub(crate) mono_views: std::collections::HashSet<String>,
    pub(crate) rewrap_formats: HashMap<String, String>,
}

impl MediaTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct media files imported so far.
    pub fn len(&self) -> usize {
        self.media.len()
    }

    pub fn is_empty(&self) -> bool {
        self.media.is_empty()
    }
}

/// `source_item_names(clip)`: bin names for a clip's separate source items. Video is `Name_V`;
/// audio is `Name_A01`, `Name_A02`, ... by Avid track, else by model position. A merged or
/// grouped item takes the Avid Name itself.
pub fn source_item_names(clip: &Clip) -> (String, Vec<String>) {
    let names = clip
        .audio
        .iter()
        .enumerate()
        .map(|(i, audio)| {
            let number = audio.avid_track.filter(|&t| t != 0).unwrap_or(i as i64 + 1);
            format!("{}_A{:02}", clip.name, number)
        })
        .collect();
    (format!("{}_V", clip.name), names)
}

fn clip_info(clip: &Clip) -> op::ClipInfo<'_> {
    op::ClipInfo { name: &clip.name, tape_name: clip.tape_name.as_deref(), material_umid: clip.material_umid.as_deref() }
}

fn project_dir(project: &Project) -> String {
    resolve_path(project.path()).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

fn item_name<'a>(item_name: Option<&'a str>, clip: &'a Clip) -> &'a str {
    item_name.filter(|n| !n.is_empty()).unwrap_or(&clip.name)
}

/// `add_video_item(project, video, clip, item_name)`.
pub fn add_video_item(
    project: &mut Project,
    media: &mut MediaTable,
    video: &VideoFile,
    clip: &Clip,
    name: Option<&str>,
    ids: &mut dyn UuidSource,
) -> Result<ItemId, OpatomError> {
    let timecode_format = media.rewrap_formats.get(&video.path).map(String::as_str)
        .unwrap_or(if video.rate == Fraction::new(24, 1) { "100" } else { "" })
        .to_string();
    if timecode_format.is_empty() {
        return Err(UnverifiedShape(format!("no Premiere-observed media timecode format for {} fps OP-Atom video yet", video.rate)).into());
    }
    let name = item_name(name, clip);
    let info = clip_info(clip);
    let frame_rate = ticks_per_unit(video.rate);
    let (media_uid, content_state) = ensure_video_media(project, media, video, clip, ids)?;
    let e = m::new_movie_media_source(&mut project.doc.tree, &media_uid, video.duration);
    let source_id = project.register(e)?;
    let e = m::new_markers_collection(&mut project.doc.tree, &content_state);
    let markers_id = project.register(e)?;
    let e = op::logging(&mut project.doc.tree, &info, frame_rate, video.duration, "2", &timecode_format, video.start);
    let logging_id = project.register(e)?;
    let e = m::new_movie_template_clip(&mut project.doc.tree, ids, &markers_id, &source_id, op::VIDEO_LABEL.0, op::VIDEO_LABEL.1);
    let clip_id = project.register(e)?;
    let e = m::new_channel_groups(&mut project.doc.tree);
    let groups_id = project.register(e)?;
    let master_uid = ids.uuid4().to_string();
    let e = m::new_master_clip(&mut project.doc.tree, &master_uid, &logging_id, &clip_id, &groups_id, name);
    let master = project.register_uid(e, &master_uid)?;
    let item_uid = ids.uuid4().to_string();
    let e = m::new_clip_item(&mut project.doc.tree, &item_uid, &master_uid, name, op::VIDEO_LABEL.0);
    let item = project.register_uid(e, &item_uid)?;
    Ok(project.attach_imported_item(item, &item_uid, master, &[&clip_id], &video.path, video.duration)?)
}

/// `add_audio_item(project, audio, clip, item_name)`.
pub fn add_audio_item(
    project: &mut Project,
    media: &mut MediaTable,
    audio: &AudioFile,
    clip: &Clip,
    name: Option<&str>,
    ids: &mut dyn UuidSource,
) -> Result<ItemId, OpatomError> {
    let name = item_name(name, clip);
    let info = clip_info(clip);
    let rate = TICKS_PER_SECOND.div_euclid(audio.sample_rate);
    let layout = m::audio_channel_layout(1);
    let (media_uid, content_state) = ensure_audio_media(project, media, audio, clip, ids)?;
    let e = m::new_audio_media_source(&mut project.doc.tree, &media_uid, audio.duration);
    let source_id = project.register(e)?;
    let e = m::new_markers_collection(&mut project.doc.tree, &content_state);
    let markers_id = project.register(e)?;
    let e = op::logging(&mut project.doc.tree, &info, rate, audio.duration, "1", op::AUDIO_TIMECODE_FORMAT, audio.start);
    let logging_id = project.register(e)?;
    let e = m::new_secondary_content(&mut project.doc.tree, &source_id, 0);
    let secondary = project.register(e)?;
    let template = m::new_audio_template_clip(
        &mut project.doc.tree,
        ids,
        &markers_id,
        &source_id,
        &[&secondary],
        op::AUDIO_LABEL.0,
        op::AUDIO_LABEL.1,
        &layout,
    );
    if audio.gain_db != 0.0 {
        op::set_master_gain(&mut project.doc.tree, template, audio.gain_db);
    }
    let clip_id = project.register(template)?;
    let e = m::new_channel_serializer(&mut project.doc.tree, 0, 0);
    let channel_id = project.register(e)?;
    let e = m::new_channel_vector(&mut project.doc.tree, &[&channel_id], 0);
    let vector_id = project.register(e)?;
    let e = m::new_audio_channel_groups(&mut project.doc.tree, &[&vector_id]);
    let groups_id = project.register(e)?;
    let e = m::new_master_audio_chain(&mut project.doc.tree, 1, 0);
    let chain_id = project.register(e)?;
    let master_uid = ids.uuid4().to_string();
    let master_element =
        m::new_audio_master_clip(&mut project.doc.tree, ids, &master_uid, &logging_id, &[&chain_id], &[&clip_id], &groups_id, name);
    if let Some(code) = clip.time_display {
        op::set_time_display(&mut project.doc.tree, master_element, code)?;
    }
    let master = project.register_uid(master_element, &master_uid)?;
    let item_uid = ids.uuid4().to_string();
    let e = m::new_clip_item(&mut project.doc.tree, &item_uid, &master_uid, name, op::AUDIO_LABEL.0);
    let item = project.register_uid(e, &item_uid)?;
    Ok(project.attach_imported_item(item, &item_uid, master, &[&clip_id], &audio.path, audio.duration)?)
}

/// Shared physical sources for the opt-in native AV-master composition.
pub(crate) fn ensure_video_media(project: &mut Project, media: &mut MediaTable, video: &VideoFile, clip: &Clip, ids: &mut dyn UuidSource) -> Result<(String,String), OpatomError> {
    let info = clip_info(clip);
    let dir = project_dir(project);
    let frame_rate = ticks_per_unit(video.rate);
    if !media.media.contains_key(&video.path) {
        let media_uid = ids.uuid4().to_string();
        let content_state = ids.uuid4().to_string();
        let vi = op::VideoInfo { frame_rate_ticks: frame_rate, duration: video.duration, width: video.width, height: video.height, codec: &video.codec };
        let stream = op::video_stream(&mut project.doc.tree, &vi)?;
        let stream_id = project.register(stream)?;
        let element = op::opatom_media(
            &mut project.doc.tree,
            ids,
            &media_uid,
            "VideoStream",
            &stream_id,
            &video.path,
            &dir,
            &info,
            &content_state,
            None,
            video.start,
        );
        let registered = project.register_uid(element, &media_uid)?;
        project.imported_media.push(registered);
        media.media.insert(video.path.clone(), (media_uid, content_state));
    }
    Ok(media.media[&video.path].clone())
}

pub(crate) fn ensure_audio_media(project: &mut Project, media: &mut MediaTable, audio: &AudioFile, clip: &Clip, ids: &mut dyn UuidSource) -> Result<(String,String), OpatomError> {
    let sample_type = u32::try_from(audio.bits).ok().and_then(op::sample_type).ok_or_else(|| UnverifiedShape(format!("no observed SampleType for {}-bit audio",audio.bits)))?;
    let info = clip_info(clip);
    let dir = project_dir(project);
    let rate = TICKS_PER_SECOND.div_euclid(audio.sample_rate);
    let layout = m::audio_channel_layout(1);
    if !media.media.contains_key(&audio.path) {
        let media_uid = ids.uuid4().to_string();
        let content_state = ids.uuid4().to_string();
        let stream = m::new_audio_stream(&mut project.doc.tree, rate, &layout, audio.duration, sample_type);
        let stream_id = project.register(stream)?;
        let element = op::opatom_media(
            &mut project.doc.tree,
            ids,
            &media_uid,
            "AudioStream",
            &stream_id,
            &audio.path,
            &dir,
            &info,
            &content_state,
            Some(rate),
            audio.start,
        );
        let registered = project.register_uid(element, &media_uid)?;
        project.imported_media.push(registered);
        media.media.insert(audio.path.clone(), (media_uid, content_state));
    }
    Ok(media.media[&audio.path].clone())
}
