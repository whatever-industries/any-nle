//! Port of py_premiere `models/media_import.py` object builders (`new_*`), plus the converter's
//! OP-Atom builders (`avid2prproj/opatom.py`) in [`opatom`]. The file-format probes/readers are
//! in [`crate::probe`], the import entry points in `model::import`. Every builder returns a
//! detached element in `tree`; callers splice or register it.
//! Where Python draws `uuid.uuid4()` the builder takes a `&mut dyn UuidSource` and draws in the
//! same order.

use super::{indexed_refs, leaf, top};
use crate::base64::b64encode;
use crate::error::{Error, Result};
use crate::ids::UuidSource;
use crate::xml::{NodeId, Tree};

pub const ITEM_CLASS_ID: &str = "cb4e0ed7-aca1-4171-8525-e3658dec06dd";
pub const MASTER_CLASS_ID: &str = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf";
pub const LOGGING_CLASS_ID: &str = "77ab7fdd-dcdf-465d-9906-7a330ca1e738";
pub const VIDEO_CLIP_CLASS_ID: &str = "9308dbef-2440-4acb-9ab2-953b9a4e82ec";
pub const CHANNEL_GROUPS_CLASS_ID: &str = "a3127a8c-95d4-456e-a7f5-171b3f922426";
pub const MARKERS_CLASS_ID: &str = "bee50706-b524-416c-9f03-b596ce5f6866";
pub const SOURCE_CLASS_ID: &str = "e64ddf74-8fac-4682-8aa8-0e0ca2248949";
pub const MEDIA_CLASS_ID: &str = "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff";
pub const STREAM_CLASS_ID: &str = "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5";
/// The generic importer's implementation GUID (BMP, PNG and WAV alike).
pub const BMP_IMPLEMENTATION_ID: &str = "1fa18bfa-255c-44b1-ad73-56bcd99fceaf";
pub const AUDIO_CLIP_CLASS_ID: &str = "b8830d03-de02-41ee-84ec-fe566dc70cd9";
pub const AUDIO_SOURCE_CLASS_ID: &str = "f588da05-fc2a-4fbc-9383-74d653b379e3";
pub const AUDIO_STREAM_CLASS_ID: &str = "0b5cf52f-2b85-4863-890b-8844b64ecfe9";
pub const MASTER_AUDIO_CHAIN_CLASS_ID: &str = "3cb131d1-d3c0-47ae-a19a-bdf75ea11674";
pub const CHANNEL_VECTOR_CLASS_ID: &str = "333d203b-3a53-4195-8894-fc7523ff3dc7";
pub const CHANNEL_CLASS_ID: &str = "5c89aa7a-89a6-4483-becd-f2b1def42316";
pub const SECONDARY_CLASS_ID: &str = "f9d004b5-cb04-4e2f-af6f-64fadc2c4be9";

pub const AUDIO_LABEL_INDEX: i64 = 2;
pub const AUDIO_LABEL_COLOR: i64 = 480554;
pub const VIDEO_LABEL_INDEX: i64 = 0;
pub const VIDEO_LABEL_COLOR: i64 = 11405886;
pub const AV_LABEL_INDEX: i64 = 1;
pub const AV_LABEL_COLOR: i64 = 6769408;

/// `TIMECODE_FORMATS`: `(ticks per frame, drop frame)` to Premiere's timecode display format.
pub const TIMECODE_FORMATS: [((i64, bool), i64); 4] = [
    ((10160640000, false), 101),
    ((8475667200, false), 103),
    ((8475667200, true), 102),
    ((8467200000, true), 104),
];

/// Lookup in [`TIMECODE_FORMATS`].
pub fn timecode_format(ticks_per_frame: i64, drop_frame: bool) -> Option<i64> {
    TIMECODE_FORMATS.iter().find(|(k, _)| *k == (ticks_per_frame, drop_frame)).map(|(_, v)| *v)
}

pub const AAC_SAMPLE_TYPE: &str = "7";
pub const MONO_LAYOUT: &str = "[{\"channellabel\":0}]";
pub const WAVE_FORMAT_PCM: u32 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: u32 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: u32 = 0xFFFE;

/// `CHANNEL_TYPES`: channel count to `ChannelType`.
pub fn channel_type(channels: u32) -> Option<i64> {
    match channels {
        1 => Some(0),
        2 => Some(1),
        6 => Some(2),
        _ => None,
    }
}

/// `WAV_SAMPLE_TYPES`: outer `None` = unlisted; inner `None` = element elided (24-bit PCM).
pub fn wav_sample_type(format_tag: u32, bytes_per_sample: u32) -> Option<Option<&'static str>> {
    match (format_tag, bytes_per_sample) {
        (WAVE_FORMAT_PCM, 2) => Some(Some("3")),
        (WAVE_FORMAT_PCM, 3) => Some(None),
        (WAVE_FORMAT_IEEE_FLOAT, 4) => Some(Some("6")),
        _ => None,
    }
}

/// `AIFF_SAMPLE_TYPES`.
pub fn aiff_sample_type(format_tag: u32, bytes_per_sample: u32) -> Option<Option<&'static str>> {
    match (format_tag, bytes_per_sample) {
        (WAVE_FORMAT_PCM, 2) => Some(Some("11")),
        _ => None,
    }
}

/// `COMPRESSED_SAMPLE_TYPES`.
pub fn compressed_sample_type(suffix: &str) -> Option<&'static str> {
    match suffix {
        ".m4a" | ".mp3" | ".aac" => Some("7"),
        ".wma" => Some("3"),
        _ => None,
    }
}

/// `StillProfile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StillProfile {
    pub codec: i64,
    pub alpha_type: bool,
    pub field_uncertain: bool,
}

/// `STILL_CODECS` by suffix.
pub fn still_profile(suffix: &str) -> Option<StillProfile> {
    let (codec, alpha_type, field_uncertain) = match suffix {
        ".bmp" | ".gif" => (1380013856, true, true),
        ".png" => (1886283552, false, true),
        ".jpg" => (1785750887, false, true),
        ".psd" => (1431194446, false, false),
        ".tif" => (1953064550, false, true),
        _ => return None,
    };
    Some(StillProfile { codec, alpha_type, field_uncertain })
}

pub const ZERO_GUID: &str = "00000000-0000-0000-0000-000000000000";
pub const STILL_FRAME_RATE: i64 = 8475667200;
pub const STILL_DURATION: i64 = 10973491200000000;
pub const STILL_DEFAULT_OUT: i64 = 1262874412800;
pub const STILL_LABEL_INDEX: i64 = 3;
pub const STILL_LABEL_COLOR: i64 = 8851829;
pub const STILL_COLOR_SPACE: &str = "{\"baseColorProfile\":{\"colorProfileName\":\"BT.709 RGB Full\"},\"baseProfileType\":1}";
pub const MOVIE_COLOR_SPACE: &str = "{\"baseColorProfile\":{\"colorProfileData\":\"AQAAAP////8=\",\"colorProfileName\":\"BT.709,32f,Display-Referred\"},\"baseProfileType\":1}";
/// The AVI stream fourcc that means "uncompressed": no `CodecType` element.
pub const NO_CODEC: &str = "DIB ";

/// `VideoStreamProfile`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoStreamProfile {
    pub color_space: &'static str,
    pub ignore_alpha: bool,
    pub alpha_type: Option<&'static str>,
    pub alpha_uncertain: bool,
    pub field_uncertain: bool,
    pub orientation: bool,
    pub codec_type: Option<&'static str>,
    pub field_type: Option<&'static str>,
}

const PROFILE_DEFAULT: VideoStreamProfile = VideoStreamProfile {
    color_space: STILL_COLOR_SPACE,
    ignore_alpha: false,
    alpha_type: None,
    alpha_uncertain: false,
    field_uncertain: false,
    orientation: false,
    codec_type: None,
    field_type: None,
};

/// `MOVIE_CONTAINERS` by suffix.
pub fn movie_container(suffix: &str) -> Option<&'static str> {
    match suffix {
        ".avi" => Some("avi"),
        ".mp4" | ".mov" => Some("mov"),
        ".mxf" => Some("mxf"),
        _ => None,
    }
}

/// `MOVIE_STREAM_PROFILES` by `(container, codec)`.
pub fn movie_stream_profile(container: &str, codec: &str) -> Option<VideoStreamProfile> {
    let mov_h264 = VideoStreamProfile {
        color_space: MOVIE_COLOR_SPACE,
        ignore_alpha: true,
        alpha_type: Some("3"),
        field_uncertain: true,
        orientation: true,
        ..PROFILE_DEFAULT
    };
    Some(match (container, codec) {
        ("avi", "DIB ") => VideoStreamProfile { alpha_uncertain: true, field_uncertain: true, ..PROFILE_DEFAULT },
        ("avi", "MJPG") => VideoStreamProfile {
            field_uncertain: true,
            codec_type: Some("MPEG"),
            field_type: Some("3"),
            ..PROFILE_DEFAULT
        },
        ("mov", "avc1") => mov_h264,
        ("mov", "hvc1") => VideoStreamProfile { codec_type: Some("HEVC"), ..mov_h264 },
        ("mov", "apch") | ("mov", "AVdh") => VideoStreamProfile { orientation: true, ..PROFILE_DEFAULT },
        ("mxf", "MPEG") => PROFILE_DEFAULT,
        _ => return None,
    })
}

/// `audio_channel_layout`: mono is the unlabelled channel 0; others label from 100 up.
pub fn audio_channel_layout(channels: u32) -> String {
    if channels == 1 {
        return MONO_LAYOUT.to_string();
    }
    let labels: Vec<String> = (0..channels).map(|i| format!("{{\"channellabel\":{}}}", 100 + i)).collect();
    format!("[{}]", labels.join(","))
}

// ---- helpers -----------------------------------------------------------------------------------

/// `base64.b64encode(s.encode("utf-16-le")).decode("ascii")`.
pub fn utf16le_b64(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len() * 2);
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    b64encode(&bytes)
}

/// `int.from_bytes(s.encode("ascii"), "big")`.
fn fourcc(s: &str) -> Result<u128> {
    if !s.is_ascii() || s.len() > 16 {
        return Err(Error::Value(format!("codec {s:?} is not an ASCII fourcc")));
    }
    Ok(s.bytes().fold(0u128, |acc, b| (acc << 8) | b as u128))
}

/// Python `posixpath.normpath`.
fn posix_normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let slashes = path.bytes().take_while(|&b| b == b'/').count();
    let initial = if slashes == 2 { 2 } else { slashes.min(1) };
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let joined = format!("{}{}", "/".repeat(initial), comps.join("/"));
    if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

fn posix_abspath(path: &str) -> String {
    if path.starts_with('/') {
        posix_normpath(path)
    } else {
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());
        posix_normpath(&format!("{cwd}/{path}"))
    }
}

/// A media path's relative form on save and in the converter's OP-Atom builder.
/// Absolute Windows drive/UNC paths stay absolute rather than passing through a POSIX cwd.
pub fn media_relpath(path: &str, start: &str) -> String {
    let windows = path.replace('\\', "/");
    let b = windows.as_bytes();
    let mut unc = windows.strip_prefix("//").unwrap_or("").split('/');
    let unc_absolute = !unc.next().unwrap_or("").is_empty() && !unc.next().unwrap_or("").is_empty();
    if (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/') || unc_absolute {
        return path.to_string();
    }
    relpath(path, start)
}

/// Python `os.path.relpath(path, start)` on POSIX, for the low-level template builders.
pub fn relpath(path: &str, start: &str) -> String {
    let start_abs = posix_abspath(start);
    let path_abs = posix_abspath(path);
    let sl: Vec<&str> = start_abs.split('/').filter(|s| !s.is_empty()).collect();
    let pl: Vec<&str> = path_abs.split('/').filter(|s| !s.is_empty()).collect();
    let common = sl.iter().zip(pl.iter()).take_while(|(a, b)| a == b).count();
    let mut rel: Vec<&str> = vec![".."; sl.len() - common];
    rel.extend(&pl[common..]);
    if rel.is_empty() {
        ".".into()
    } else {
        rel.join("/")
    }
}

/// Python `os.path.basename` on POSIX.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `ET.SubElement(parent, tag, {attr: value}).tail = tail`.
fn ref_child(tree: &mut Tree, parent: NodeId, tag: &str, attr: &str, value: &str, tail: &str) -> NodeId {
    let e = tree.sub_element(parent, tag, &[(attr, value)]);
    tree.set_tail(e, Some(tail));
    e
}

/// `SubElement(parent, tag, {Version: v})` with text and tail.
fn versioned(tree: &mut Tree, parent: NodeId, tag: &str, version: &str, text: &str, tail: &str) -> NodeId {
    let e = tree.sub_element(parent, tag, &[("Version", version)]);
    tree.set_text(e, Some(text));
    tree.set_tail(e, Some(tail));
    e
}

fn set_last_tail(tree: &mut Tree, element: NodeId, tail: &str) {
    let last = *tree.children(element).last().expect("element has children");
    tree.set_tail(last, Some(tail));
}

/// `top(...)` with Python's `uid` truthiness: an empty uid gives the `ObjectID` placeholder.
fn top_uid(tree: &mut Tree, tag: &str, class_id: &str, version: &str, uid: &str) -> NodeId {
    top(tree, tag, class_id, version, if uid.is_empty() { None } else { Some(uid) })
}

// ---- builders ----------------------------------------------------------------------------------

pub fn new_logging_info(tree: &mut Tree, name: &str, frame_rate: i64) -> NodeId {
    let e = top(tree, "ClipLoggingInfo", LOGGING_CLASS_ID, "10", None);
    leaf(tree, e, "CaptureMode", "2", "\n\t\t");
    leaf(tree, e, "ClipName", name, "\n\t\t");
    leaf(tree, e, "TimecodeFormat", "103", "\n\t\t");
    leaf(tree, e, "MediaFrameRate", &frame_rate.to_string(), "\n\t");
    e
}

#[allow(clippy::too_many_arguments)]
pub fn new_media_logging_info(
    tree: &mut Tree,
    name: &str,
    frame_rate: i64,
    duration_ticks: i64,
    capture_mode: Option<&str>,
    timecode_format: &str,
    start_ticks: i64,
    clip_id: &str,
) -> NodeId {
    let e = top(tree, "ClipLoggingInfo", LOGGING_CLASS_ID, "10", None);
    if !clip_id.is_empty() {
        leaf(tree, e, "ClipID", clip_id, "\n\t\t");
    }
    if let Some(mode) = capture_mode {
        leaf(tree, e, "CaptureMode", mode, "\n\t\t");
    }
    leaf(tree, e, "ClipName", name, "\n\t\t");
    leaf(tree, e, "TimecodeFormat", timecode_format, "\n\t\t");
    leaf(tree, e, "MediaInPoint", &start_ticks.to_string(), "\n\t\t");
    leaf(tree, e, "MediaOutPoint", &(start_ticks + duration_ticks).to_string(), "\n\t\t");
    leaf(tree, e, "MediaFrameRate", &frame_rate.to_string(), "\n\t");
    e
}

pub fn new_markers_collection(tree: &mut Tree, content_state: &str) -> NodeId {
    let e = top(tree, "Markers", MARKERS_CLASS_ID, "4", None);
    leaf(tree, e, "ByGUID", "byGUID", "\n\t\t");
    leaf(tree, e, "LastMetadataState", ZERO_GUID, "\n\t\t");
    leaf(tree, e, "LastContentState", content_state, "\n\t");
    e
}

pub fn new_channel_groups(tree: &mut Tree) -> NodeId {
    let e = top(tree, "ClipChannelGroupVectorSerializer", CHANNEL_GROUPS_CLASS_ID, "1", None);
    tree.set_text(e, Some("\n\t"));
    e
}

pub fn new_audio_channel_groups(tree: &mut Tree, vector_ids: &[&str]) -> NodeId {
    let e = top(tree, "ClipChannelGroupVectorSerializer", CHANNEL_GROUPS_CLASS_ID, "1", None);
    let vectors = versioned(tree, e, "ClipChannelVectors", "1", "\n\t\t\t", "\n\t");
    indexed_refs(tree, vectors, "ClipChannelVectorItem", vector_ids);
    e
}

pub fn new_channel_vector(tree: &mut Tree, channel_ids: &[&str], channel_type: i64) -> NodeId {
    let e = top(tree, "ClipChannelVectorSerializer", CHANNEL_VECTOR_CLASS_ID, "1", None);
    let channels = versioned(tree, e, "ClipChannels", "1", "\n\t\t\t", "\n\t\t");
    indexed_refs(tree, channels, "ClipChannelItem", channel_ids);
    leaf(tree, e, "ChannelType", &channel_type.to_string(), "\n\t");
    e
}

pub fn new_channel_serializer(tree: &mut Tree, source_clip_index: i64, channel_index: i64) -> NodeId {
    let e = top(tree, "ClipChannelSerializer", CHANNEL_CLASS_ID, "1", None);
    leaf(tree, e, "SourceClipIndex", &source_clip_index.to_string(), "\n\t\t");
    leaf(tree, e, "mSourceChannelIndex", &channel_index.to_string(), "\n\t");
    e
}

pub fn new_master_audio_chain(tree: &mut Tree, channels: u32, channel_type: i64) -> NodeId {
    let e = top(tree, "AudioComponentChain", MASTER_AUDIO_CHAIN_CLASS_ID, "4", None);
    leaf(tree, e, "DefaultVol", "true", "\n\t\t");
    leaf(tree, e, "DefaultVolumeComponentID", "1", "\n\t\t");
    if channels > 1 {
        leaf(tree, e, "DefaultChannelVolumeComponentID", "2", "\n\t\t");
    }
    let tail = if channels > 1 { "\n\t\t" } else { "\n\t" };
    versioned(tree, e, "ComponentChain", "3", "\n\t\t", tail);
    if channels > 1 {
        leaf(tree, e, "AudioChannelLayout", &audio_channel_layout(channels), "\n\t\t");
        leaf(tree, e, "ChannelType", &channel_type.to_string(), "\n\t");
    }
    e
}

#[allow(clippy::too_many_arguments)]
pub fn new_movie_video_stream(
    tree: &mut Tree,
    width: i64,
    height: i64,
    frame_rate: i64,
    duration_ticks: i64,
    container: &str,
    codec: &str,
    field_order: i64,
) -> Result<NodeId> {
    let profile = movie_stream_profile(container, codec).ok_or_else(|| {
        // Python: NotImplementedError(f"... {key}") with the (container, codec) tuple's repr.
        Error::NotImplemented(format!(
            "no verified VideoStream profile for ({}, {})",
            crate::convert::model::py_str_repr(container),
            crate::convert::model::py_str_repr(codec)
        ))
    })?;
    let e = top(tree, "VideoStream", STREAM_CLASS_ID, "22", None);
    leaf(tree, e, "FrameRate", &frame_rate.to_string(), "\n\t\t");
    leaf(tree, e, "Duration", &duration_ticks.to_string(), "\n\t\t");
    if profile.ignore_alpha {
        leaf(tree, e, "IgnoreAlpha", "true", "\n\t\t");
    }
    leaf(tree, e, "FrameRect", &format!("0,0,{width},{height}"), "\n\t\t");
    let stamped = profile.codec_type.unwrap_or(codec);
    if stamped != NO_CODEC {
        leaf(tree, e, "CodecType", &fourcc(stamped)?.to_string(), "\n\t\t");
    }
    leaf(tree, e, "OriginalColorSpace", profile.color_space, "\n\t\t");
    if let Some(a) = profile.alpha_type {
        leaf(tree, e, "AlphaType", a, "\n\t\t");
    }
    if profile.alpha_uncertain {
        leaf(tree, e, "AlphaInfoIsUncertain", "true", "\n\t\t");
    }
    let stored = if field_order != 0 { Some(field_order.to_string()) } else { profile.field_type.map(String::from) };
    if let Some(s) = stored.filter(|s| !s.is_empty()) {
        leaf(tree, e, "OriginalFieldType", &s, "\n\t\t");
    }
    if profile.field_uncertain {
        leaf(tree, e, "FieldTypeIsUncertain", "true", "\n\t\t");
    }
    if profile.orientation {
        leaf(tree, e, "OriginalImageOrientationType", "1", "\n\t\t");
    }
    set_last_tail(tree, e, "\n\t");
    Ok(e)
}

/// Shared body of the video/audio/still template clips (the `Clip` core). `still_extra` adds the
/// still-image drop-frame property (and its different tails); `out_ticks` adds OutPoint/InPoint
/// (stills only).
#[allow(clippy::too_many_arguments)]
fn template_core(
    tree: &mut Tree,
    element: NodeId,
    core_tail: &str,
    markers_id: &str,
    source_id: &str,
    label_index: i64,
    label_color: i64,
    still_extra: bool,
    out_ticks: Option<i64>,
    ids: &mut dyn UuidSource,
) {
    let core = versioned(tree, element, "Clip", "18", "\n\t\t\t", core_tail);
    let node = versioned(tree, core, "Node", "1", "\n\t\t\t\t", "\n\t\t\t");
    let props = versioned(tree, node, "Properties", "1", "\n\t\t\t\t\t", "\n\t\t\t");
    leaf(tree, props, "asl.clip.label.color", &label_color.to_string(), "\n\t\t\t\t\t");
    let name_tail = if still_extra { "\n\t\t\t\t\t" } else { "\n\t\t\t\t" };
    leaf(tree, props, "asl.clip.label.name", &format!("BE.Prefs.LabelColors.{label_index}"), name_tail);
    if still_extra {
        leaf(tree, props, "BE.Prefs.StillImages.DefaultIsDropFrame", "true", "\n\t\t\t\t");
    }
    let owner = versioned(tree, core, "MarkerOwner", "1", "\n\t\t\t\t", "\n\t\t\t");
    ref_child(tree, owner, "Markers", "ObjectRef", markers_id, "\n\t\t\t");
    ref_child(tree, core, "Source", "ObjectRef", source_id, "\n\t\t\t");
    if let Some(out) = out_ticks {
        leaf(tree, core, "OutPoint", &out.to_string(), "\n\t\t\t");
        leaf(tree, core, "InPoint", "0", "\n\t\t\t");
    }
    leaf(tree, core, "ClipID", &ids.uuid4().to_string(), "\n\t\t\t");
    leaf(tree, core, "InUse", "false", "\n\t\t");
}

pub fn new_movie_template_clip(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    markers_id: &str,
    source_id: &str,
    label_index: i64,
    label_color: i64,
) -> NodeId {
    let e = top(tree, "VideoClip", VIDEO_CLIP_CLASS_ID, "11", None);
    template_core(tree, e, "\n\t", markers_id, source_id, label_index, label_color, false, None, ids);
    e
}

/// `ModificationState` with a UTF-16LE base64 blob (`None` = no text, only the tail).
fn modification_state(tree: &mut Tree, media: NodeId, binary_hash: &str, content_state: Option<&str>) {
    let state = tree.sub_element(media, "ModificationState", &[("Encoding", "base64"), ("BinaryHash", binary_hash)]);
    if let Some(cs) = content_state {
        tree.set_text(state, Some(format!("{}\n\t\t", utf16le_b64(cs))));
    }
    tree.set_tail(state, Some("\n\t\t"));
}

#[allow(clippy::too_many_arguments)]
pub fn new_movie_media(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    media_uid: &str,
    stream_id: &str,
    file_path: &str,
    project_dir: &str,
    content_state: &str,
    audio_stream_id: Option<&str>,
    conformed_rate: Option<i64>,
    start_ticks: i64,
) -> NodeId {
    let e = top_uid(tree, "Media", MEDIA_CLASS_ID, "30", media_uid);
    if let Some(a) = audio_stream_id {
        ref_child(tree, e, "AudioStream", "ObjectRef", a, "\n\t\t");
    }
    ref_child(tree, e, "VideoStream", "ObjectRef", stream_id, "\n\t\t");
    let hash = ids.uuid4().to_string();
    modification_state(tree, e, &hash, Some(content_state));
    leaf(tree, e, "RelativePath", &relpath(file_path, project_dir), "\n\t\t");
    leaf(tree, e, "FilePath", file_path, "\n\t\t");
    leaf(tree, e, "ImplementationID", BMP_IMPLEMENTATION_ID, "\n\t\t");
    leaf(tree, e, "Title", basename(file_path), "\n\t\t");
    leaf(tree, e, "FileKey", &ids.uuid4().to_string(), "\n\t\t");
    if let Some(r) = conformed_rate {
        leaf(tree, e, "ConformedAudioRate", &r.to_string(), "\n\t\t");
    }
    if start_ticks != 0 {
        leaf(tree, e, "AlternateStart", &start_ticks.to_string(), "\n\t\t");
        leaf(tree, e, "UseAlternateStart", "true", "\n\t\t");
    }
    leaf(tree, e, "ContentAndMetadataState", content_state, "\n\t\t");
    leaf(tree, e, "ActualMediaFilePath", file_path, "\n\t");
    e
}

fn media_source(tree: &mut Tree, tag: &str, class_id: &str, media_uid: &str, duration: i64) -> NodeId {
    let e = top(tree, tag, class_id, "2", None);
    let source = versioned(tree, e, "MediaSource", "4", "\n\t\t\t", "\n\t\t");
    versioned(tree, source, "Content", "10", "\n\t\t\t", "\n\t\t\t");
    ref_child(tree, source, "Media", "ObjectURef", media_uid, "\n\t\t");
    leaf(tree, e, "OriginalDuration", &duration.to_string(), "\n\t");
    e
}

pub fn new_movie_media_source(tree: &mut Tree, media_uid: &str, duration_ticks: i64) -> NodeId {
    media_source(tree, "VideoMediaSource", SOURCE_CLASS_ID, media_uid, duration_ticks)
}

pub fn new_audio_stream(
    tree: &mut Tree,
    frame_rate: i64,
    layout: &str,
    duration_ticks: i64,
    sample_type: Option<&str>,
) -> NodeId {
    let e = top(tree, "AudioStream", AUDIO_STREAM_CLASS_ID, "8", None);
    leaf(tree, e, "FrameRate", &frame_rate.to_string(), "\n\t\t");
    leaf(tree, e, "AudioChannelLayout", layout, "\n\t\t");
    match sample_type {
        None => {
            leaf(tree, e, "Duration", &duration_ticks.to_string(), "\n\t");
        }
        Some(s) => {
            leaf(tree, e, "Duration", &duration_ticks.to_string(), "\n\t\t");
            leaf(tree, e, "SampleType", s, "\n\t");
        }
    }
    e
}

pub fn new_audio_media_source(tree: &mut Tree, media_uid: &str, duration_ticks: i64) -> NodeId {
    media_source(tree, "AudioMediaSource", AUDIO_SOURCE_CLASS_ID, media_uid, duration_ticks)
}

#[allow(clippy::too_many_arguments)]
pub fn new_audio_media(
    tree: &mut Tree,
    media_uid: &str,
    stream_id: &str,
    file_path: &str,
    project_dir: &str,
    content_state: &str,
    conformed_rate: i64,
    file_key: &str,
    binary_hash: &str,
    stream_number: i64,
) -> NodeId {
    let e = top_uid(tree, "Media", MEDIA_CLASS_ID, "30", media_uid);
    ref_child(tree, e, "AudioStream", "ObjectRef", stream_id, "\n\t\t");
    modification_state(tree, e, binary_hash, if stream_number == 0 { Some(content_state) } else { None });
    leaf(tree, e, "RelativePath", &relpath(file_path, project_dir), "\n\t\t");
    leaf(tree, e, "FilePath", file_path, "\n\t\t");
    leaf(tree, e, "ImplementationID", BMP_IMPLEMENTATION_ID, "\n\t\t");
    leaf(tree, e, "Title", basename(file_path), "\n\t\t");
    leaf(tree, e, "FileKey", file_key, "\n\t\t");
    leaf(tree, e, "ConformedAudioRate", &conformed_rate.to_string(), "\n\t\t");
    if stream_number != 0 {
        leaf(tree, e, "StreamNumber", &stream_number.to_string(), "\n\t\t");
    }
    leaf(tree, e, "ContentAndMetadataState", content_state, "\n\t\t");
    leaf(tree, e, "ActualMediaFilePath", file_path, "\n\t");
    e
}

#[allow(clippy::too_many_arguments)]
pub fn new_audio_template_clip(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    markers_id: &str,
    source_id: &str,
    secondary_ids: &[&str],
    label_index: i64,
    label_color: i64,
    layout: &str,
) -> NodeId {
    let e = top(tree, "AudioClip", AUDIO_CLIP_CLASS_ID, "8", None);
    template_core(tree, e, "\n\t\t", markers_id, source_id, label_index, label_color, false, None, ids);
    let secondary = versioned(tree, e, "SecondaryContents", "1", "\n\t\t\t", "\n\t\t");
    indexed_refs(tree, secondary, "SecondaryContentItem", secondary_ids);
    leaf(tree, e, "AudioChannelLayout", layout, "\n\t");
    e
}

pub fn new_secondary_content(tree: &mut Tree, content_ref: &str, channel: i64) -> NodeId {
    let e = top(tree, "SecondaryContent", SECONDARY_CLASS_ID, "1", None);
    ref_child(tree, e, "Content", "ObjectRef", content_ref, "\n\t\t");
    leaf(tree, e, "ChannelIndex", &channel.to_string(), "\n\t");
    e
}

#[allow(clippy::too_many_arguments)]
pub fn new_audio_master_clip(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    master_uid: &str,
    logging_id: &str,
    chain_ids: &[&str],
    clip_ids: &[&str],
    groups_id: &str,
    name: &str,
) -> NodeId {
    let e = top_uid(tree, "MasterClip", MASTER_CLASS_ID, "12", master_uid);
    ref_child(tree, e, "LoggingInfo", "ObjectRef", logging_id, "\n\t\t");
    let chains = versioned(tree, e, "AudioComponentChains", "1", "\n\t\t\t", "\n\t\t");
    indexed_refs(tree, chains, "AudioComponentChain", chain_ids);
    let clips = versioned(tree, e, "Clips", "1", "\n\t\t\t", "\n\t\t");
    indexed_refs(tree, clips, "Clip", clip_ids);
    ref_child(tree, e, "AudioClipChannelGroups", "ObjectRef", groups_id, "\n\t\t");
    leaf(tree, e, "Name", name, "\n\t\t");
    leaf(tree, e, "DefMappingID", &ids.uuid4().to_string(), "\n\t\t");
    leaf(tree, e, "MasterClipChangeVersion", "1", "\n\t");
    e
}

pub fn new_video_stream(tree: &mut Tree, width: i64, height: i64, frame_rate: i64, profile: StillProfile) -> NodeId {
    let e = top(tree, "VideoStream", STREAM_CLASS_ID, "22", None);
    leaf(tree, e, "FrameRate", &frame_rate.to_string(), "\n\t\t");
    leaf(tree, e, "Duration", &STILL_DURATION.to_string(), "\n\t\t");
    leaf(tree, e, "FrameRect", &format!("0,0,{width},{height}"), "\n\t\t");
    leaf(tree, e, "CodecType", &profile.codec.to_string(), "\n\t\t");
    leaf(tree, e, "IsStill", "true", "\n\t\t");
    leaf(tree, e, "OriginalColorSpace", STILL_COLOR_SPACE, "\n\t\t");
    if profile.alpha_type {
        leaf(tree, e, "AlphaType", "1", "\n\t\t");
    }
    if profile.field_uncertain {
        leaf(tree, e, "FieldTypeIsUncertain", "true", "\n\t\t");
    }
    set_last_tail(tree, e, "\n\t");
    e
}

pub fn new_media(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    media_uid: &str,
    stream_id: &str,
    file_path: &str,
    project_dir: &str,
    content_state: &str,
) -> NodeId {
    let e = top_uid(tree, "Media", MEDIA_CLASS_ID, "30", media_uid);
    ref_child(tree, e, "VideoStream", "ObjectRef", stream_id, "\n\t\t");
    let hash = ids.uuid4().to_string();
    modification_state(tree, e, &hash, Some(content_state));
    leaf(tree, e, "RelativePath", &relpath(file_path, project_dir), "\n\t\t");
    leaf(tree, e, "FilePath", file_path, "\n\t\t");
    leaf(tree, e, "ImplementationID", BMP_IMPLEMENTATION_ID, "\n\t\t");
    leaf(tree, e, "Title", basename(file_path), "\n\t\t");
    leaf(tree, e, "FileKey", &ids.uuid4().to_string(), "\n\t\t");
    leaf(tree, e, "Infinite", "true", "\n\t\t");
    leaf(tree, e, "ContentAndMetadataState", content_state, "\n\t\t");
    leaf(tree, e, "ActualMediaFilePath", file_path, "\n\t");
    e
}

pub fn new_media_source(tree: &mut Tree, media_uid: &str) -> NodeId {
    media_source(tree, "VideoMediaSource", SOURCE_CLASS_ID, media_uid, STILL_DURATION)
}

#[allow(clippy::too_many_arguments)]
pub fn new_template_clip(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    markers_id: &str,
    source_id: &str,
    label_index: i64,
    label_color: i64,
    out_ticks: i64,
) -> NodeId {
    let e = top(tree, "VideoClip", VIDEO_CLIP_CLASS_ID, "11", None);
    template_core(tree, e, "\n\t", markers_id, source_id, label_index, label_color, true, Some(out_ticks), ids);
    e
}

pub fn new_master_clip(
    tree: &mut Tree,
    master_uid: &str,
    logging_id: &str,
    clip_id: &str,
    groups_id: &str,
    name: &str,
) -> NodeId {
    let e = top_uid(tree, "MasterClip", MASTER_CLASS_ID, "12", master_uid);
    ref_child(tree, e, "LoggingInfo", "ObjectRef", logging_id, "\n\t\t");
    let clips = versioned(tree, e, "Clips", "1", "\n\t\t\t", "\n\t\t");
    let c = tree.sub_element(clips, "Clip", &[("Index", "0"), ("ObjectRef", clip_id)]);
    tree.set_tail(c, Some("\n\t\t"));
    ref_child(tree, e, "AudioClipChannelGroups", "ObjectRef", groups_id, "\n\t\t");
    leaf(tree, e, "Name", name, "\n\t\t");
    leaf(tree, e, "MasterClipChangeVersion", "0", "\n\t");
    e
}

pub fn new_clip_item(tree: &mut Tree, item_uid: &str, master_uid: &str, name: &str, label_index: i64) -> NodeId {
    let e = top_uid(tree, "ClipProjectItem", ITEM_CLASS_ID, "1", item_uid);
    let pi = versioned(tree, e, "ProjectItem", "1", "\n\t\t\t", "\n\t\t");
    let node = versioned(tree, pi, "Node", "1", "\n\t\t\t\t", "\n\t\t\t");
    let props = versioned(tree, node, "Properties", "1", "\n\t\t\t\t\t", "\n\t\t\t");
    leaf(tree, props, "Column.PropertyText.Label", &format!("BE.Prefs.LabelColors.{label_index}"), "\n\t\t\t\t");
    leaf(tree, pi, "Name", name, "\n\t\t");
    ref_child(tree, e, "MasterClip", "ObjectURef", master_uid, "\n\t");
    e
}

/// Converter OP-Atom builders (`avid2prproj/opatom.py`), pure tree builders.
pub mod opatom {
    use super::*;

    /// Premiere 26.5's own import of an Avid ProRes LT OP-Atom MXF (User save 2026-10-20).
    pub const PRORES_COLOR_SPACE: &str = "{\"baseColorProfile\":{\"colorProfileName\":\"BT.709 RGB Full\"},\"baseProfileType\":1}";
    pub const DNX_COLOR_SPACE: &str = "{\"baseColorProfile\":{\"colorProfileData\":\"AQAAAP////8=\",\"colorProfileName\":\"BT.709,8-bit,Display-Referred\"},\"baseProfileType\":1,\"colorSpaceMetadata\":{\"dataRangeInfoValid\":1}}";
    pub const AUDIO_TIMECODE_FORMAT: &str = "200";
    pub const TAPE_NAME_LIMIT: usize = 40;
    /// `(label index, label color)`: video Iris, audio Forest.
    pub const VIDEO_LABEL: (i64, i64) = (0, 11405886);
    pub const AUDIO_LABEL: (i64, i64) = (2, 480554);

    /// `VIDEO_TIMECODE_FORMAT`: only 24 fps (ticks per frame 10584000000) is known.
    pub fn video_timecode_format(frame_rate_ticks: i64) -> Option<&'static str> {
        (frame_rate_ticks == 10584000000).then_some("100")
    }

    /// `SAMPLE_TYPE`: 16-bit is "3", 24-bit is elided.
    pub fn sample_type(bits: u32) -> Option<Option<&'static str>> {
        match bits {
            16 => Some(Some("3")),
            24 => Some(None),
            _ => None,
        }
    }

    /// The fields of `model.Clip` the builders read.
    pub struct ClipInfo<'a> {
        pub name: &'a str,
        pub tape_name: Option<&'a str>,
        pub material_umid: Option<&'a str>,
    }

    /// The fields of `model.VideoFile` that `_video_stream` reads.
    pub struct VideoInfo<'a> {
        /// `ticks_per_unit(video.rate)`.
        pub frame_rate_ticks: i64,
        pub duration: i64,
        pub width: i64,
        pub height: i64,
        pub codec: &'a str,
    }

    fn tape(clip: &ClipInfo) -> String {
        let t = match clip.tape_name {
            Some(t) if !t.is_empty() => t,
            _ => clip.name,
        };
        t.chars().take(TAPE_NAME_LIMIT).collect()
    }

    /// `_opatom_media`.
    #[allow(clippy::too_many_arguments)]
    pub fn opatom_media(
        tree: &mut Tree,
        ids: &mut dyn UuidSource,
        media_uid: &str,
        stream_tag: &str,
        stream_id: &str,
        path: &str,
        project_dir: &str,
        clip: &ClipInfo,
        content_state: &str,
        conformed_rate: Option<i64>,
        start: i64,
    ) -> NodeId {
        let e = top_uid(tree, "Media", MEDIA_CLASS_ID, "30", media_uid);
        ref_child(tree, e, stream_tag, "ObjectRef", stream_id, "\n\t\t");
        let hash = ids.uuid4().to_string();
        modification_state(tree, e, &hash, Some(content_state));
        let tape = tape(clip);
        leaf(tree, e, "RelativePath", &media_relpath(path, project_dir), "\n\t\t");
        leaf(tree, e, "Start", &start.to_string(), "\n\t\t");
        leaf(tree, e, "ImplementationID", BMP_IMPLEMENTATION_ID, "\n\t\t");
        leaf(tree, e, "FileKey", &ids.uuid4().to_string(), "\n\t\t");
        leaf(tree, e, "AlternateTapeName", &tape, "\n\t\t");
        if let Some(r) = conformed_rate {
            leaf(tree, e, "ConformedAudioRate", &r.to_string(), "\n\t\t");
        }
        leaf(tree, e, "ContentAndMetadataState", content_state, "\n\t\t");
        leaf(tree, e, "TapeName", &tape, "\n\t\t");
        leaf(tree, e, "ActualMediaFilePath", path, "\n\t\t");
        leaf(tree, e, "FilePath", path, "\n\t\t");
        leaf(tree, e, "AlternateStart", "0", "\n\t\t");
        leaf(tree, e, "Title", clip.name, "\n\t");
        e
    }

    /// `_logging`.
    pub fn logging(
        tree: &mut Tree,
        clip: &ClipInfo,
        frame_rate: i64,
        duration: i64,
        capture_mode: &str,
        timecode_format: &str,
        start: i64,
    ) -> NodeId {
        let e = new_media_logging_info(
            tree,
            clip.name,
            frame_rate,
            duration,
            Some(capture_mode),
            timecode_format,
            start,
            clip.material_umid.unwrap_or(""),
        );
        set_last_tail(tree, e, "\n\t\t");
        leaf(tree, e, "TapeName", &tape(clip), "\n\t");
        e
    }

    /// Python `repr(float)`.
    pub fn py_float_repr(x: f64) -> String {
        if x.is_nan() {
            return "nan".into();
        }
        if x.is_infinite() {
            return if x > 0.0 { "inf" } else { "-inf" }.into();
        }
        if x == 0.0 {
            return if x.is_sign_negative() { "-0.0" } else { "0.0" }.into();
        }
        let sci = format!("{:e}", x.abs());
        let (mant, exp) = sci.split_once('e').unwrap();
        let exp: i32 = exp.parse().unwrap();
        let digits: String = mant.chars().filter(|c| *c != '.').collect();
        let decpt = exp + 1;
        let n = digits.len() as i32;
        let body = if decpt > -4 && decpt <= 16 {
            if decpt <= 0 {
                format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
            } else if decpt >= n {
                format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
            } else {
                format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
            }
        } else {
            let e = decpt - 1;
            let m = if n > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits.clone() };
            format!("{}e{}{:02}", m, if e < 0 { '-' } else { '+' }, e.abs())
        };
        if x < 0.0 {
            format!("-{body}")
        } else {
            body
        }
    }

    /// `_set_master_gain`: linear `<Gain>` after `AudioChannelLayout` on the master AudioClip.
    pub fn set_master_gain(tree: &mut Tree, audio_clip: NodeId, db: f64) {
        let layout = tree.child_by_tag(audio_clip, "AudioChannelLayout");
        let gain = tree.new_element("Gain");
        tree.set_text(gain, Some(py_float_repr(crate::build::fmt::py_pow(10.0, db / 20.0))));
        match layout {
            Some(layout) => {
                let idx = tree.children(audio_clip).iter().position(|&c| c == layout).unwrap();
                let layout_tail = tree.tail(layout).map(String::from);
                tree.set_tail(gain, layout_tail.clone());
                let new_tail = if idx > 0 {
                    let prev = tree.children(audio_clip)[idx - 1];
                    tree.tail(prev).map(String::from)
                } else {
                    layout_tail
                };
                tree.set_tail(layout, new_tail);
                tree.insert(audio_clip, idx + 1, gain);
            }
            None => tree.append(audio_clip, gain),
        }
    }

    /// `_set_time_display`: `<TimeDisplay>` before `MasterClipChangeVersion`.
    pub fn set_time_display(tree: &mut Tree, master: NodeId, code: i64) -> Result<()> {
        let version = tree
            .child_by_tag(master, "MasterClipChangeVersion")
            .ok_or_else(|| Error::Value("master clip has no MasterClipChangeVersion".into()))?;
        let index = tree.children(master).iter().position(|&c| c == version).unwrap();
        let leaf_el = tree.new_element("TimeDisplay");
        tree.set_text(leaf_el, Some(code.to_string()));
        let prev = if index > 0 { tree.children(master)[index - 1] } else { *tree.children(master).last().unwrap() };
        let tail = tree.tail(prev).map(String::from);
        tree.set_tail(leaf_el, tail);
        tree.insert(master, index, leaf_el);
        Ok(())
    }

    /// `_video_stream`.
    pub fn video_stream(tree: &mut Tree, video: &VideoInfo) -> Result<NodeId> {
        let e = top(tree, "VideoStream", STREAM_CLASS_ID, "22", None);
        leaf(tree, e, "FrameRate", &video.frame_rate_ticks.to_string(), "\n\t\t");
        leaf(tree, e, "Duration", &video.duration.to_string(), "\n\t\t");
        leaf(tree, e, "FrameRect", &format!("0,0,{},{}", video.width, video.height), "\n\t\t");
        leaf(tree, e, "CodecType", &fourcc(video.codec)?.to_string(), "\n\t\t");
        let color_space = if video.codec == "apcs" { PRORES_COLOR_SPACE } else { DNX_COLOR_SPACE };
        leaf(tree, e, "OriginalColorSpace", color_space, "\n\t");
        Ok(e)
    }
}

#[cfg(test)]
mod tests {
    use super::opatom::py_float_repr;
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(py_float_repr(0.5011872336272722), "0.5011872336272722");
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(100.0), "100.0");
        assert_eq!(py_float_repr(1e-5), "1e-05");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(1.5e22), "1.5e+22");
        assert_eq!(py_float_repr(123456789012345680.0), "1.2345678901234568e+17");
    }

    #[test]
    fn relpath_matches_posix() {
        assert_eq!(relpath("/a/b/c.mxf", "/a/b"), "c.mxf");
        assert_eq!(relpath("/x/c.mxf", "/a/b"), "../../x/c.mxf");
        assert_eq!(relpath("/a/b", "/a/b"), ".");
        assert_eq!(relpath("/a//b/./c/../d", "/a"), "b/d");
    }
}
