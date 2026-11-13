//! Port of `models/sequence_builder.py`: a new sequence's object graph, the `FORMATS` presets
//! and `SequenceFormat`, plus the converter's `EXTRA_FORMATS` (`avid2prproj/writer.py`).

use std::collections::HashMap;

use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{build_leaf as leaf, indent_tree};
use crate::xml::{NodeId, Tree};

/// `models/time.py` `TICKS_PER_SECOND`.
const TICKS_PER_SECOND: u64 = 254_016_000_000;

const VIDEO_MEDIA: &str = "228cda18-3625-4d2d-951e-348879e4ed93";
const AUDIO_MEDIA: &str = "80b8e3d5-6dca-4195-aefb-cb5f407ab009";
const DATA_MEDIA: &str = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5";

/// Python `_CLASS`: (ClassID, Version) by object tag.
fn class(tag: &str) -> (&'static str, &'static str) {
    match tag {
        "ClipProjectItem" => ("cb4e0ed7-aca1-4171-8525-e3658dec06dd", "1"),
        "MasterClip" => ("fb11c33a-b0a9-4465-aa94-b6d5db2628cf", "12"),
        "ClipLoggingInfo" => ("77ab7fdd-dcdf-465d-9906-7a330ca1e738", "10"),
        "AudioComponentChain" => ("3cb131d1-d3c0-47ae-a19a-bdf75ea11674", "4"),
        "AudioClip" => ("b8830d03-de02-41ee-84ec-fe566dc70cd9", "8"),
        "VideoClip" => ("9308dbef-2440-4acb-9ab2-953b9a4e82ec", "11"),
        "ClipChannelGroupVectorSerializer" => ("a3127a8c-95d4-456e-a7f5-171b3f922426", "1"),
        "AudioSequenceSource" => ("e8d4cc83-38cb-491f-9d94-e5f7e3b205ee", "7"),
        "SecondaryContent" => ("f9d004b5-cb04-4e2f-af6f-64fadc2c4be9", "1"),
        "VideoSequenceSource" => ("4752dfa9-7a7e-4a3b-a25b-cafde1a8d036", "3"),
        "ClipChannelVectorSerializer" => ("333d203b-3a53-4195-8894-fc7523ff3dc7", "1"),
        "Sequence" => ("6a15d903-8739-11d5-af2d-9b7855ad8974", "12"),
        "ClipChannelSerializer" => ("5c89aa7a-89a6-4483-becd-f2b1def42316", "1"),
        "VideoTrackGroup" => ("9e9abf7a-0918-49c2-91ae-991b5dde77bb", "13"),
        "AudioTrackGroup" => ("9b9238b9-53a8-4cc3-b03f-b36246d052e6", "6"),
        "DataTrackGroup" => ("b714b71d-6838-48dd-9b77-db19088ced7e", "1"),
        "VideoClipTrack" => ("f68dcd81-8805-11d5-af2d-9bfa89d4ddd4", "1"),
        "VideoComponentChain" => ("0970e08a-f58f-4108-b29a-1a717b8e12e2", "3"),
        "AudioClipTrack" => ("097f6203-99ae-11d5-84f2-8cf14bde7040", "7"),
        "AudioMixTrack" => ("4b1d8400-e89e-11d5-abc4-a1a13b1e80a0", "4"),
        "MonoToStereoPanProcessor" => ("33a94283-ee2c-11d5-abc4-c1cd7f9e3c10", "1"),
        "StereoToStereoPanProcessor" => ("7bf86a01-efbe-11d5-abc4-c1ce2b1e9090", "1"),
        "MonoTo16ChannelPanProcessor" => ("8c9778ad-af4e-4e98-99fe-542f4eda2dac", "2"),
        "StereoTo16ChannelPanProcessor" => ("1a356806-5dc5-4e2f-914c-d8353e1a6581", "2"),
        "AdaptiveToAdaptivePanProcessor" => ("4bff4329-ba93-4072-950a-1f9c4aa7ab0b", "1"),
        "DefaultPanProcessor" => ("33a94282-ee2c-11d5-abc4-c1cd7f9e3c10", "2"),
        "AudioTrackInlet" => ("be3af080-e8c6-11d5-abc4-a1c6d5dee670", "4"),
        "AudioFader" => ("1a38c583-ed5c-11d5-abc4-c1cbf61ec590", "3"),
        "AudioMeter" => ("72ea4700-f615-11d5-abc4-c186585e63e0", "2"),
        "ScalarParam" => ("a714635e-a628-4b27-9d59-77eba47dbc1a", "10"),
        "BoolParam" => ("32657501-3aa4-445f-a49b-d09ecb9fa1ae", "10"),
        other => unreachable!("no ClassID for {other}"),
    }
}

const EDITING_MODE: &str = "9678af98-a7b7-4bdb-b477-7ac9c8df4a4e";
const PREVIEW_PRESET: &str =
    "EncoderPresets\\SequencePreview\\9678af98-a7b7-4bdb-b477-7ac9c8df4a4e\\QuickTime.epr";

const AUDIO_FRAME_RATE: &str = "5292000";
const STEREO: &str = "[{\"channellabel\":100},{\"channellabel\":101}]";
const MONO: &str = "[{\"channellabel\":0}]";

fn mono_layout(channels: usize) -> String {
    format!("[{}]", vec!["{\"channellabel\":0}"; channels].join(","))
}

const BUS_CHANNELS: usize = 32;
const CT_MONO: &str = "0";
const CT_STEREO: &str = "1";
const CT_BUS: &str = "3";
const WORK_AREA_LIMIT: u64 = 60 * TICKS_PER_SECOND;

const LABEL: &str = "BE.Prefs.LabelColors.5";
const LABEL_COLOR: &str = "19005";
const PANNER_COMPONENT_ID: &str = "4294967280";
const NEXT_PANNER_ID: &str = "4294967279";

const FIRST_ID: usize = 40;
const VIDEO_TRACKS: usize = 3;
const REC709: &str = "BT.709 RGB Full";
const TRACK_HEIGHT: usize = 41;

/// Every mixer strip: one per audio track, then the master.
fn strip_keys(audio_tracks: usize) -> Vec<String> {
    let mut keys: Vec<String> = (0..audio_tracks).map(|i| i.to_string()).collect();
    keys.push("master".to_string());
    keys
}

/// Python `SequenceFormat` (NamedTuple); `new` fills the same defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceFormat {
    /// Ticks per video frame (8475667200 = 29.97 fps).
    pub frame_rate: u64,
    pub width: u32,
    pub height: u32,
    /// `MZ.Sequence.VideoTimeDisplayFormat`.
    pub time_display: u32,
    pub audio_tracks: usize,
    pub video_tracks: usize,
    pub audio_track_height: usize,
    pub preview_format: &'static str,
    pub linear_compositing: bool,
    pub mono_discrete: bool,
    pub mono_stereo_inputs: [bool; 2],
    /// Explicit 5.1 (6) or eight-channel Adaptive (8) template; zero keeps legacy behavior.
    pub grouped_channels: usize,
    /// `MZ.Sequence.PreviewRenderingPresetCodec` as its FourCC.
    pub preview_codec: &'static str,
    pub color_space: &'static str,
}

impl SequenceFormat {
    /// `SequenceFormat(frame_rate, width, height, time_display)` with every default.
    pub const fn new(frame_rate: u64, width: u32, height: u32, time_display: u32) -> Self {
        SequenceFormat {
            frame_rate,
            width,
            height,
            time_display,
            audio_tracks: 4,
            video_tracks: VIDEO_TRACKS,
            audio_track_height: TRACK_HEIGHT,
            preview_format: "fc3cd4d9-d839-8259-9276-05c5000000ea",
            linear_compositing: true,
            mono_discrete: false,
            mono_stereo_inputs: [false; 2],
            grouped_channels: 0,
            preview_codec: "apcs",
            color_space: REC709,
        }
    }
}

fn uhd(frame_rate: u64, time_display: u32) -> SequenceFormat {
    SequenceFormat {
        preview_format: "41384a52-7e4a-3c48-e0ad-4939000000ea",
        linear_compositing: false,
        ..SequenceFormat::new(frame_rate, 3840, 2160, time_display)
    }
}

fn uhd_hdr(frame_rate: u64, time_display: u32) -> SequenceFormat {
    SequenceFormat {
        preview_format: "4e32a57e-26be-deca-0bf7-4548000000ea",
        linear_compositing: false,
        preview_codec: "apch",
        color_space: "BT.2100 HLG RGB Full",
        ..SequenceFormat::new(frame_rate, 3840, 2160, time_display)
    }
}

/// Python `DEFAULT_PRESET`.
pub const DEFAULT_PRESET: &str = "1080p23976";

/// Python `FORMATS`, in dict order.
pub fn formats() -> Vec<(&'static str, SequenceFormat)> {
    let sq = SequenceFormat::new;
    vec![
        ("1080p23976", sq(10594584000, 1920, 1080, 110)),
        ("1080p25", sq(10160640000, 1920, 1080, 101)),
        ("1080p2997", sq(8475667200, 1920, 1080, 102)),
        ("1080p50", sq(5080320000, 1920, 1080, 105)),
        ("1080p5994", sq(4237833600, 1920, 1080, 106)),
        ("2160p23976", uhd(10594584000, 110)),
        ("2160p25", uhd(10160640000, 101)),
        ("2160p2997", uhd(8475667200, 102)),
        ("2160p50", uhd(5080320000, 105)),
        ("2160p5994", uhd(4237833600, 106)),
        ("2160p23976hdr", uhd_hdr(10594584000, 110)),
        ("2160p25hdr", uhd_hdr(10160640000, 101)),
        ("2160p2997hdr", uhd_hdr(8475667200, 102)),
        ("2160p50hdr", uhd_hdr(5080320000, 105)),
        ("2160p5994hdr", uhd_hdr(4233600000, 108)),
        (
            "square1080p30",
            SequenceFormat {
                preview_format: "33cbb1f1-d0c5-3397-7a8d-a576000000ea",
                ..sq(8467200000, 1080, 1080, 104)
            },
        ),
        (
            "portrait1080p30",
            SequenceFormat {
                preview_format: "01abac9c-c469-f667-c4a8-23a1000000ea",
                ..sq(8467200000, 1080, 1920, 104)
            },
        ),
        (
            "portrait1080p4x5",
            SequenceFormat {
                preview_format: "85cc5060-43fa-354a-99ff-02f0000000ea",
                ..sq(8467200000, 864, 1080, 104)
            },
        ),
        (
            "broadcast4mono",
            SequenceFormat { audio_tracks: 4, mono_discrete: true, ..sq(10594584000, 1920, 1080, 110) },
        ),
        (
            "broadcast8mono",
            SequenceFormat {
                audio_tracks: 8,
                mono_discrete: true,
                audio_track_height: 25,
                ..sq(10594584000, 1920, 1080, 110)
            },
        ),
    ]
}

/// The converter's `EXTRA_FORMATS` (`avid2prproj/writer.py`), added to `FORMATS` by the writer
/// via `setdefault`.
pub fn extra_formats() -> Vec<(&'static str, SequenceFormat)> {
    vec![("avid2prproj-1080p24", SequenceFormat::new(10584000000, 1920, 1080, 100))]
}

/// A preset by name from `FORMATS`, falling back to `EXTRA_FORMATS` (which the writer registers
/// in `FORMATS` before building).
pub fn format_by_name(name: &str) -> Option<SequenceFormat> {
    if let Some(spec) = name.strip_prefix("anynle-tracks/") {
        let (count, base) = spec.split_once('/')?;
        let count = count.parse::<usize>().ok()?;
        if !(1..=99).contains(&count) || base.starts_with("anynle-tracks/") { return None; }
        let mut format = format_by_name(base)?;
        // Input strip capacity is independent of the destination master bus.
        // A mono-discrete preset couples the two and needs separate qualification.
        if count < format.audio_tracks || format.mono_discrete { return None; }
        format.audio_tracks = count;
        return Some(format);
    }
    // Opt-in converter templates. Names carry exact ticks, raster and display enum;
    // they never enter the default/qualified preset list.
    if let Some(spec) = name.strip_prefix("anynle-op1a/") {
        let fields: Vec<_> = spec.split('/').collect();
        if fields.len() != 4 {
            return None;
        }
        let ticks = fields[0].parse::<u64>().ok()?;
        let width = fields[1].parse::<u32>().ok()?;
        let height = fields[2].parse::<u32>().ok()?;
        let display = fields[3].parse::<u32>().ok()?;
        if ticks == 0
            || width == 0
            || height == 0
            || width > 16384
            || height > 16384
            || !matches!(display, 100..=110 | 113)
        {
            return None;
        }
        return Some(SequenceFormat::new(ticks, width, height, display));
    }
    if let Some(spec) = name.strip_prefix("anynle-grouped/") {
        let (channels, base) = spec.split_once('/')?;
        let channels = channels.parse::<usize>().ok()?;
        // One wrapper only: accept qualified named/custom presets, and reject
        // recursive wrappers before calling the parser again.
        if !matches!(channels, 6 | 8) || base.starts_with("anynle-grouped/") || base.starts_with("anynle-tracks/") { return None; }
        let mut format = format_by_name(base)?;
        if format.mono_discrete { return None; }
        format.grouped_channels = channels;
        return Some(format);
    }
    formats()
        .into_iter()
        .chain(extra_formats())
        .find(|(n, _)| *n == name)
        .map(|(_, f)| f)
}

/// Python `_allocate`: ObjectIDs by role, in the order Premiere assigns them.
fn allocate(audio_tracks: usize, secondaries: usize, serializers: usize) -> HashMap<String, String> {
    let mut roles: Vec<String> = ["logging", "master_chain", "audio_clip", "video_clip", "channel_groups", "audio_source"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    roles.extend((0..secondaries).map(|i| format!("secondary_{i}")));
    roles.push("video_source".into());
    roles.push("channel_vector".into());
    roles.extend((0..serializers).map(|i| format!("serializer_{i}")));
    for r in ["video_group", "audio_group", "data_group", "video_chain", "mix_track"] {
        roles.push(r.into());
    }
    for i in 0..audio_tracks {
        roles.push(format!("strip_chain_{i}"));
        roles.push(format!("panner_{i}"));
    }
    for r in ["master_strip_chain", "default_panner", "inlet"] {
        roles.push(r.into());
    }
    for i in 0..audio_tracks {
        roles.push(format!("fader_{i}"));
        roles.push(format!("meter_{i}"));
        roles.push(format!("balance_{i}"));
    }
    roles.push("fader_master".into());
    roles.push("meter_master".into());
    for strip in strip_keys(audio_tracks) {
        roles.push(format!("volume_{strip}"));
        roles.push(format!("mute_{strip}"));
    }
    roles.into_iter().enumerate().map(|(offset, role)| (role, (FIRST_ID + offset).to_string())).collect()
}

/// Python `_Uids`: mints the fragment's GUIDs lazily and memoized per role, or hands back
/// pinned ones.
struct Uids<'a> {
    values: HashMap<String, String>,
    source: &'a mut dyn UuidSource,
}

impl Uids<'_> {
    fn get(&mut self, role: &str) -> String {
        if let Some(v) = self.values.get(role) {
            return v.clone();
        }
        let v = self.source.uuid4().to_string();
        self.values.insert(role.to_string(), v.clone());
        v
    }
}

/// Python `_object`.
fn object(t: &mut Tree, root: NodeId, tag: &str, object_id: Option<&str>, uid: Option<&str>) -> NodeId {
    let (class_id, version) = class(tag);
    let mut attrs: Vec<(&str, &str)> = Vec::new();
    if let Some(id) = object_id {
        attrs.push(("ObjectID", id));
    }
    if let Some(u) = uid {
        attrs.push(("ObjectUID", u));
    }
    attrs.push(("ClassID", class_id));
    attrs.push(("Version", version));
    t.sub_element(root, tag, &attrs)
}

/// Python `_param_object`.
fn param_object(t: &mut Tree, root: NodeId, object_id: &str, kind: &str) -> NodeId {
    let (class_id, version) = class(kind);
    t.sub_element(
        root,
        "AudioComponentParam",
        &[("ObjectID", object_id), ("ClassID", class_id), ("Version", version)],
    )
}

/// Python `_indexed`: `<tag Index="i" attribute="value"/>` per entry.
fn indexed<S: AsRef<str>>(t: &mut Tree, parent: NodeId, tag: &str, attribute: &str, values: &[S]) {
    for (index, v) in values.iter().enumerate() {
        t.sub_element(parent, tag, &[("Index", &index.to_string()), (attribute, v.as_ref())]);
    }
}

/// Python `_clip_core`.
fn clip_core(t: &mut Tree, parent: NodeId, source: &str, clip_id: &str) {
    let core = t.sub_element(parent, "Clip", &[("Version", "18")]);
    let node = t.sub_element(core, "Node", &[("Version", "1")]);
    let properties = t.sub_element(node, "Properties", &[("Version", "1")]);
    leaf(t, properties, "asl.clip.label.color", LABEL_COLOR);
    leaf(t, properties, "asl.clip.label.name", LABEL);
    t.sub_element(core, "Source", &[("ObjectRef", source)]);
    leaf(t, core, "ClipID", clip_id);
    leaf(t, core, "InUse", "false");
}

/// Python `_audio_component`.
fn audio_component(
    t: &mut Tree,
    parent: NodeId,
    component_id: &str,
    component_type: &str,
    params: &[&str],
    layout: &str,
    channel_type: &str,
) -> NodeId {
    let audio = t.sub_element(parent, "AudioComponent", &[("Version", "3")]);
    let component = t.sub_element(audio, "Component", &[("Version", "7")]);
    if !params.is_empty() {
        let holder = t.sub_element(component, "Params", &[("Version", "1")]);
        indexed(t, holder, "Param", "ObjectRef", params);
    }
    leaf(t, component, "ID", component_id);
    leaf(t, audio, "FrameRate", AUDIO_FRAME_RATE);
    leaf(t, audio, "AudioChannelLayout", layout);
    leaf(t, audio, "ChannelType", channel_type);
    leaf(t, audio, "AudioComponentType", component_type);
    audio
}

/// Python `_sequence_source`.
fn sequence_source(t: &mut Tree, parent: NodeId, sequence_uid: &str) {
    let source = t.sub_element(parent, "SequenceSource", &[("Version", "4")]);
    t.sub_element(source, "Content", &[("Version", "10")]);
    t.sub_element(source, "Sequence", &[("ObjectURef", sequence_uid)]);
    leaf(t, parent, "OriginalDuration", "0");
}

/// Python `_strip_chain`.
fn strip_chain(t: &mut Tree, chain: NodeId, fader: &str, meter: &str, layout: Option<&str>, channel_type: &str) {
    let inner = t.sub_element(chain, "ComponentChain", &[("Version", "3")]);
    let components = t.sub_element(inner, "Components", &[("Version", "1")]);
    indexed(t, components, "Component", "ObjectRef", &[fader, meter]);
    if let Some(layout) = layout {
        leaf(t, chain, "AudioChannelLayout", layout);
        leaf(t, chain, "ChannelType", channel_type);
    }
}

/// Python `_clip_track`.
#[allow(clippy::too_many_arguments)]
fn clip_track(
    t: &mut Tree,
    root: NodeId,
    tag: &str,
    media: &str,
    uid: &str,
    track_id: usize,
    index: usize,
    targeted: bool,
    track_name: Option<&str>,
    height: usize,
) -> NodeId {
    let audio = tag == "AudioClipTrack";
    let track = object(t, root, tag, None, Some(uid));
    let clip_track = t.sub_element(track, "ClipTrack", &[("Version", "2")]);
    let inner = t.sub_element(clip_track, "Track", &[("Version", "4")]);
    let node = t.sub_element(inner, "Node", &[("Version", "1")]);
    let properties = t.sub_element(node, "Properties", &[("Version", "1")]);
    leaf(t, properties, "TL.SQTrackExpanded", "0");
    leaf(t, properties, "TL.SQTrackExpandedHeight", &height.to_string());
    leaf(t, properties, "MZ.SourceTrackState", "0");
    leaf(t, properties, "MZ.SourceTrackNumber", &index.to_string());
    leaf(t, properties, "MZ.TrackTargeted", if targeted { "1" } else { "0" });
    if index < 2 {
        leaf(t, properties, "TL.SQTrackShy", "0");
    }
    if audio {
        if index < 2 {
            leaf(t, properties, "TL.SQTrackAudioKeyframeStyle", "0");
        }
        if let Some(name) = track_name {
            leaf(t, properties, "MZ.TrackName", name);
        }
        leaf(t, properties, "CM.KeyframeMode", "true");
    }
    leaf(t, inner, "ID", &track_id.to_string());
    leaf(t, inner, "MediaType", media);
    leaf(t, inner, "Index", &index.to_string());
    for holder_tag in ["ClipItems", "TransitionItems"] {
        let holder = t.sub_element(clip_track, holder_tag, &[("Version", "3")]);
        leaf(t, holder, "MediaType", media);
        leaf(t, holder, "Index", &index.to_string());
    }
    track
}

/// Python `build_sequence(name, preset, uids, video_tracks)`: a `PremiereData` fragment holding
/// a new sequence's objects, created detached in `tree`. GUIDs are drawn from `ids` in the
/// same order Python calls `uuid.uuid4()`; `uids` pins some by role.
pub fn build_sequence(
    tree: &mut Tree,
    name: &str,
    preset: &str,
    uids: Option<&HashMap<String, String>>,
    video_tracks: Option<usize>,
    ids: &mut dyn UuidSource,
) -> Result<NodeId> {
    let Some(mut fmt) = format_by_name(preset) else {
        let mut known: Vec<&str> = formats().into_iter().map(|(n, _)| n).collect();
        known.sort_unstable();
        return value(format!("unknown preset {preset:?}; try {known:?}"));
    };
    if let Some(n) = video_tracks {
        fmt.video_tracks = n;
    }
    Ok(build_with_format(tree, name, &fmt, uids, ids))
}

/// `build_sequence` for an explicit format (what the body does once the preset is resolved).
pub fn build_with_format(
    tree: &mut Tree,
    name: &str,
    fmt: &SequenceFormat,
    uids: Option<&HashMap<String, String>>,
    ids: &mut dyn UuidSource,
) -> NodeId {
    let t = tree;
    let wide = fmt.grouped_channels;
    let secondaries = if fmt.mono_discrete || wide == 8 { BUS_CHANNELS } else if wide == 6 { 6 } else { 2 };
    let serializers = if wide > 0 { wide } else if fmt.mono_discrete { fmt.audio_tracks } else { 2 };
    let at = allocate(fmt.audio_tracks, secondaries, serializers);
    let a = |role: &str| -> &str { at[role].as_str() };
    let mut uid = Uids { values: uids.cloned().unwrap_or_default(), source: ids };
    let root = t.new_element_with("PremiereData", &[("Version", "3")]);

    let mono = fmt.mono_discrete;
    let mono_stereo_input = |index| fmt.mono_stereo_inputs.get(index).copied().unwrap_or(false);
    let clip_layout = if mono || wide == 8 { mono_layout(BUS_CHANNELS) } else if wide == 6 { crate::build::media::audio_channel_layout(6) } else { STEREO.to_string() };
    let clip_type = if mono || wide == 8 { CT_BUS } else if wide == 6 { "2" } else { CT_STEREO };
    let track_layout = if wide > 0 { clip_layout.as_str() } else if mono { MONO } else { STEREO };
    let track_type = if wide > 0 { clip_type } else if mono { CT_MONO } else { CT_STEREO };
    let video_uids: Vec<String> = (0..fmt.video_tracks).map(|i| uid.get(&format!("video_track_{i}"))).collect();
    let audio_uids: Vec<String> = (0..fmt.audio_tracks).map(|i| uid.get(&format!("audio_track_{i}"))).collect();

    // --- panel item and master clip ---
    let item_uid = uid.get("item");
    let item = object(t, root, "ClipProjectItem", None, Some(&item_uid));
    let project_item = t.sub_element(item, "ProjectItem", &[("Version", "1")]);
    let node = t.sub_element(project_item, "Node", &[("Version", "1")]);
    let properties = t.sub_element(node, "Properties", &[("Version", "1")]);
    leaf(t, properties, "project.icon.view.grid.order", "0");
    leaf(t, properties, "Column.PropertyText.Label", LABEL);
    leaf(t, project_item, "Name", name);
    let master_uid = uid.get("master");
    t.sub_element(item, "MasterClip", &[("ObjectURef", &master_uid)]);

    let master_uid = uid.get("master");
    let master = object(t, root, "MasterClip", None, Some(&master_uid));
    t.sub_element(master, "LoggingInfo", &[("ObjectRef", a("logging"))]);
    let chains = t.sub_element(master, "AudioComponentChains", &[("Version", "1")]);
    indexed(t, chains, "AudioComponentChain", "ObjectRef", &[a("master_chain")]);
    let clips = t.sub_element(master, "Clips", &[("Version", "1")]);
    indexed(t, clips, "Clip", "ObjectRef", &[a("audio_clip"), a("video_clip")]);
    t.sub_element(master, "AudioClipChannelGroups", &[("ObjectRef", a("channel_groups"))]);
    leaf(t, master, "Name", name);
    leaf(t, master, "MasterClipChangeVersion", "3");

    object(t, root, "ClipLoggingInfo", Some(a("logging")), None);

    let chain = object(t, root, "AudioComponentChain", Some(a("master_chain")), None);
    leaf(t, chain, "DefaultVol", "true");
    leaf(t, chain, "DefaultVolumeComponentID", "1");
    leaf(t, chain, "DefaultChannelVolumeComponentID", "2");
    t.sub_element(chain, "ComponentChain", &[("Version", "3")]);
    leaf(t, chain, "AudioChannelLayout", &clip_layout);
    leaf(t, chain, "ChannelType", clip_type);

    let audio_clip = object(t, root, "AudioClip", Some(a("audio_clip")), None);
    let id = uid.get("audio_clip_id");
    clip_core(t, audio_clip, a("audio_source"), &id);
    let contents = t.sub_element(audio_clip, "SecondaryContents", &[("Version", "1")]);
    let secondary_refs: Vec<&str> = (0..secondaries).map(|i| a(&format!("secondary_{i}"))).collect();
    indexed(t, contents, "SecondaryContentItem", "ObjectRef", &secondary_refs);
    leaf(t, audio_clip, "AudioChannelLayout", &clip_layout);

    let video_clip = object(t, root, "VideoClip", Some(a("video_clip")), None);
    let id = uid.get("video_clip_id");
    clip_core(t, video_clip, a("video_source"), &id);

    let groups = object(t, root, "ClipChannelGroupVectorSerializer", Some(a("channel_groups")), None);
    let vectors = t.sub_element(groups, "ClipChannelVectors", &[("Version", "1")]);
    indexed(t, vectors, "ClipChannelVectorItem", "ObjectRef", &[a("channel_vector")]);

    let src = object(t, root, "AudioSequenceSource", Some(a("audio_source")), None);
    let seq_uid = uid.get("sequence");
    sequence_source(t, src, &seq_uid);
    for channel in 0..secondaries {
        let secondary = object(t, root, "SecondaryContent", Some(a(&format!("secondary_{channel}"))), None);
        t.sub_element(secondary, "Content", &[("ObjectRef", a("audio_source"))]);
        leaf(t, secondary, "ChannelIndex", &channel.to_string());
    }
    let src = object(t, root, "VideoSequenceSource", Some(a("video_source")), None);
    let seq_uid = uid.get("sequence");
    sequence_source(t, src, &seq_uid);

    let vector = object(t, root, "ClipChannelVectorSerializer", Some(a("channel_vector")), None);
    let channels = t.sub_element(vector, "ClipChannels", &[("Version", "1")]);
    let serializer_refs: Vec<&str> = (0..serializers).map(|i| a(&format!("serializer_{i}"))).collect();
    indexed(t, channels, "ClipChannelItem", "ObjectRef", &serializer_refs);
    leaf(t, vector, "ChannelType", clip_type);

    let seq_uid = uid.get("sequence");
    build_sequence_object(t, root, &seq_uid, name, fmt, &at);

    for channel in 0..serializers {
        let serializer = object(t, root, "ClipChannelSerializer", Some(a(&format!("serializer_{channel}"))), None);
        leaf(t, serializer, "SourceClipIndex", "0");
        leaf(t, serializer, "mSourceChannelIndex", &channel.to_string());
    }

    // --- track groups ---
    let group = object(t, root, "VideoTrackGroup", Some(a("video_group")), None);
    build_video_group(t, group, &video_uids, fmt, &at);
    let group = object(t, root, "AudioTrackGroup", Some(a("audio_group")), None);
    let group_id = uid.get("audio_group_id");
    build_audio_group(t, group, &audio_uids, &at, &group_id, if wide > 0 { wide } else if fmt.mono_discrete { fmt.audio_tracks } else { 2 });
    let data_group = object(t, root, "DataTrackGroup", Some(a("data_group")), None);
    let inner = t.sub_element(data_group, "TrackGroup", &[("Version", "1")]);
    leaf(t, inner, "FrameRate", &fmt.frame_rate.to_string());
    leaf(t, inner, "NextTrackID", "1");

    // --- tracks ---
    for (index, track_uid) in video_uids.iter().enumerate() {
        clip_track(t, root, "VideoClipTrack", VIDEO_MEDIA, track_uid, index + 1, index, index == 0, None, TRACK_HEIGHT);
    }
    let chain = object(t, root, "VideoComponentChain", Some(a("video_chain")), None);
    t.sub_element(chain, "ComponentChain", &[("Version", "3")]);

    for (index, track_uid) in audio_uids.iter().enumerate() {
        let track_name = if fmt.mono_discrete { Some(format!("Output {}", index + 1)) } else { None };
        let track = clip_track(
            t,
            root,
            "AudioClipTrack",
            AUDIO_MEDIA,
            track_uid,
            index + 2,
            index,
            true,
            track_name.as_deref(),
            fmt.audio_track_height,
        );
        let audio_track = t.sub_element(track, "AudioTrack", &[("Version", "12")]);
        let owner = t.sub_element(audio_track, "ComponentOwner", &[("Version", "1")]);
        t.sub_element(owner, "Components", &[("ObjectRef", a(&format!("strip_chain_{index}")))]);
        t.sub_element(audio_track, "Panner", &[("ObjectRef", a(&format!("panner_{index}")))]);
        let id = uid.get(&format!("audio_track_id_{index}"));
        leaf(t, audio_track, "ID", &id);
        if mono || wide > 0 || mono_stereo_input(index) {
            let input_type = if mono_stereo_input(index) { CT_MONO } else { track_type };
            leaf(t, audio_track, "ChannelType", input_type);
        }
        leaf(t, audio_track, "NextPannerID", NEXT_PANNER_ID);
    }

    let mix = object(t, root, "AudioMixTrack", Some(a("mix_track")), None);
    let audio_track = t.sub_element(mix, "AudioTrack", &[("Version", "12")]);
    let owner = t.sub_element(audio_track, "ComponentOwner", &[("Version", "1")]);
    t.sub_element(owner, "Components", &[("ObjectRef", a("master_strip_chain"))]);
    t.sub_element(audio_track, "Panner", &[("ObjectRef", a("default_panner"))]);
    let id = uid.get("mix_track_id");
    leaf(t, audio_track, "ID", &id);
    if mono || wide > 0 {
        leaf(t, audio_track, "ChannelType", clip_type);
    }
    leaf(t, audio_track, "SubType", "3");
    leaf(t, audio_track, "Assign", "0");
    leaf(t, audio_track, "NextPannerID", NEXT_PANNER_ID);
    let inner = t.sub_element(mix, "Track", &[("Version", "4")]);
    let node = t.sub_element(inner, "Node", &[("Version", "1")]);
    let properties = t.sub_element(node, "Properties", &[("Version", "1")]);
    leaf(t, properties, "TL.SQTrackExpanded", "0");
    leaf(t, properties, "TL.SQTrackExpandedHeight", "41");
    leaf(t, inner, "ID", "1");
    leaf(t, inner, "MediaType", AUDIO_MEDIA);
    leaf(t, inner, "Index", "0");
    t.sub_element(mix, "Inlet", &[("ObjectRef", a("inlet"))]);

    // --- the mix graph ---
    for index in 0..fmt.audio_tracks {
        let chain = object(t, root, "AudioComponentChain", Some(a(&format!("strip_chain_{index}"))), None);
        strip_chain(
            t,
            chain,
            a(&format!("fader_{index}")),
            a(&format!("meter_{index}")),
            if mono || mono_stereo_input(index) { None } else { Some(track_layout) },
            if mono_stereo_input(index) { CT_MONO } else { track_type },
        );
        if wide > 0 {
            let kind = if wide == 6 { "DefaultPanProcessor" } else { "AdaptiveToAdaptivePanProcessor" };
            let panner = object(t, root, kind, Some(a(&format!("panner_{index}"))), None);
            let processor = t.sub_element(panner, "PanProcessor", &[("Version", "3")]);
            let params = if wide == 8 { vec![a(&format!("balance_{index}"))] } else { vec![] };
            audio_component(t, processor, PANNER_COMPONENT_ID, "0", &params, track_layout, track_type);
            if wide == 6 {
                leaf(t, panner, "DefaultPannerInputChannelType", track_type);
                leaf(t, panner, "DefaultPannerOutputChannelType", track_type);
            }
        } else if mono {
            let panner = object(t, root, "MonoTo16ChannelPanProcessor", Some(a(&format!("panner_{index}"))), None);
            let direct = t.sub_element(panner, "DirectPanProcessor", &[("Version", "2")]);
            let processor = t.sub_element(direct, "PanProcessor", &[("Version", "3")]);
            audio_component(
                t,
                processor,
                PANNER_COMPONENT_ID,
                "0",
                &[a(&format!("balance_{index}"))],
                MONO,
                CT_MONO,
            );
            leaf(t, processor, "OutputAudioChannelLayout", &mono_layout(BUS_CHANNELS));
            leaf(t, direct, "Matrix", &format!("[[0,[{}]]]", index / 2 * 2));
        } else if mono_stereo_input(index) {
            let panner = object(t, root, "MonoToStereoPanProcessor", Some(a(&format!("panner_{index}"))), None);
            let processor = t.sub_element(panner, "PanProcessor", &[("Version", "3")]);
            audio_component(t, processor, PANNER_COMPONENT_ID, "0",
                &[a(&format!("balance_{index}"))], MONO, CT_MONO);
            leaf(t, processor, "OutputAudioChannelLayout", STEREO);
        } else {
            let panner = object(t, root, "StereoToStereoPanProcessor", Some(a(&format!("panner_{index}"))), None);
            let processor = t.sub_element(panner, "PanProcessor", &[("Version", "3")]);
            audio_component(
                t,
                processor,
                PANNER_COMPONENT_ID,
                "0",
                &[a(&format!("balance_{index}"))],
                STEREO,
                CT_STEREO,
            );
        }
    }

    let chain = object(t, root, "AudioComponentChain", Some(a("master_strip_chain")), None);
    strip_chain(t, chain, a("fader_master"), a("meter_master"), Some(&clip_layout), clip_type);
    let panner = object(t, root, "DefaultPanProcessor", Some(a("default_panner")), None);
    let processor = t.sub_element(panner, "PanProcessor", &[("Version", "3")]);
    audio_component(t, processor, PANNER_COMPONENT_ID, "0", &[], &clip_layout, clip_type);
    leaf(t, panner, "DefaultPannerInputChannelType", clip_type);
    leaf(t, panner, "DefaultPannerOutputChannelType", clip_type);

    let inlet = object(t, root, "AudioTrackInlet", Some(a("inlet")), None);
    let sources = t.sub_element(inlet, "Sources", &[("Version", "1")]);
    indexed(t, sources, "Source", "ObjectURef", &audio_uids);
    leaf(t, inlet, "AudioChannelLayout", &clip_layout);
    if mono || wide > 0 {
        leaf(t, inlet, "ChannelType", clip_type);
    }

    for strip in strip_keys(fmt.audio_tracks) {
        let is_master = strip == "master";
        let mono_input = !is_master && mono_stereo_input(strip.parse::<usize>().expect("strip index"));
        let layout: &str = if is_master { &clip_layout } else if mono_input { MONO } else { track_layout };
        let channel_type = if is_master { clip_type } else if mono_input { CT_MONO } else { track_type };
        let fader = object(t, root, "AudioFader", Some(a(&format!("fader_{strip}"))), None);
        audio_component(
            t,
            fader,
            "1",
            "1",
            &[a(&format!("volume_{strip}")), a(&format!("mute_{strip}"))],
            layout,
            channel_type,
        );
        let meter = object(t, root, "AudioMeter", Some(a(&format!("meter_{strip}"))), None);
        audio_component(t, meter, "2", "2", &[], layout, channel_type);
        if is_master {
            continue;
        }
        let pan = param_object(t, root, a(&format!("balance_{strip}")), "ScalarParam");
        if mono {
            let n: usize = strip.parse().expect("strip index");
            if n % 2 == 0 {
                leaf(t, pan, "StartKeyframe", "-91445760000000000,0.,0,0,0,0,0,0");
                leaf(t, pan, "CurrentValue", "0");
            }
            leaf(t, pan, "Name", "Pan");
        } else {
            leaf(t, pan, "StartKeyframe", "-91445760000000000,0.5,0,0,0,0,0,0");
            leaf(t, pan, "CurrentValue", "0.5");
            leaf(t, pan, "Name", if mono_input { "Pan" } else { "Balance" });
        }
        leaf(t, pan, "IsInverted", "true");
    }

    for strip in strip_keys(fmt.audio_tracks) {
        let volume = param_object(t, root, a(&format!("volume_{strip}")), "ScalarParam");
        leaf(t, volume, "Name", "Volume");
        leaf(t, volume, "UpperBound", "5.6234130859375");
        leaf(t, volume, "RangeLocked", "false");
        leaf(t, volume, "UnitsString", "dB");
        let mute = param_object(t, root, a(&format!("mute_{strip}")), "BoolParam");
        leaf(t, mute, "Name", "Mute");
        leaf(t, mute, "RangeLocked", "false");
    }

    indent_tree(t, root, 0);
    root
}

/// Python `_build_video_group`.
fn build_video_group(t: &mut Tree, group: NodeId, uids: &[String], fmt: &SequenceFormat, at: &HashMap<String, String>) {
    let inner = t.sub_element(group, "TrackGroup", &[("Version", "1")]);
    let tracks = t.sub_element(inner, "Tracks", &[("Version", "1")]);
    indexed(t, tracks, "Track", "ObjectURef", uids);
    leaf(t, inner, "FrameRate", &fmt.frame_rate.to_string());
    leaf(t, inner, "NextTrackID", &(uids.len() + 1).to_string());
    leaf(
        t,
        group,
        "ColorManagementSettings",
        "{\"autoToneMapEnabled\":true,\"enableLogColorManagement\":2,\"lutInterpolationMethod\":1}",
    );
    if !fmt.linear_compositing {
        leaf(t, group, "AllowLinearCompositing", "false");
    }
    leaf(
        t,
        group,
        "ImmersiveVideoVRConfiguration",
        "{\"ambisonicsHRIR\":\"\",\"ambisonicsMonitoringType\":0,\
\"capturedHorizontalView\":360,\"capturedVerticalView\":180,\
\"fieldOfHorizontalView\":108,\"fieldOfVerticalView\":108,\
\"projectionType\":0,\"stereoscopicEye\":0,\"stereoscopicType\":0,\
\"version\":3}",
    );
    let profile = format!(
        "{{\"baseColorProfile\":{{\"colorProfileName\":\"{}\"}},\"baseProfileType\":1}}",
        fmt.color_space
    );
    if fmt.color_space != REC709 {
        leaf(
            t,
            group,
            "WorkingColorSpaceConfiguration",
            &format!(
                "{{\"workingSpaceConfigVersion\":1,\"workingSpaceID\":\"{}\",\"workingSpaceIsLinearized\":0}}",
                fmt.color_space
            ),
        );
        leaf(t, group, "WorkingColorSpace", &profile);
    }
    leaf(t, group, "OutputColorSpace", &profile);
    leaf(t, group, "AutoInputGamutCompressionEnabled", "true");
    leaf(t, group, "IsGraphicsWhiteSameAsProject", "false");
    leaf(t, group, "IsColorAwareEffectsEnabledSameAsProject", "false");
    leaf(t, group, "FrameRect", &format!("0,0,{},{}", fmt.width, fmt.height));
    let owner = t.sub_element(group, "ComponentOwner", &[("Version", "1")]);
    t.sub_element(owner, "Components", &[("ObjectRef", &at["video_chain"])]);
}

/// Python `_build_audio_group`.
fn build_audio_group(
    t: &mut Tree,
    group: NodeId,
    uids: &[String],
    at: &HashMap<String, String>,
    group_id: &str,
    channels: usize,
) {
    let inner = t.sub_element(group, "TrackGroup", &[("Version", "1")]);
    let tracks = t.sub_element(inner, "Tracks", &[("Version", "1")]);
    indexed(t, tracks, "Track", "ObjectURef", uids);
    leaf(t, inner, "FrameRate", AUDIO_FRAME_RATE);
    leaf(t, inner, "NextTrackID", &(uids.len() + 2).to_string());
    t.sub_element(group, "MasterTrack", &[("ObjectRef", &at["mix_track"])]);
    leaf(t, group, "ID", group_id);
    leaf(t, group, "AutomationSafeFlags", "0");
    leaf(t, group, "NumAdaptiveChannels", &channels.to_string());
}

/// Python `_build_sequence_object`.
fn build_sequence_object(
    t: &mut Tree,
    root: NodeId,
    uid: &str,
    name: &str,
    fmt: &SequenceFormat,
    at: &HashMap<String, String>,
) {
    let sequence = object(t, root, "Sequence", None, Some(uid));
    let node = t.sub_element(sequence, "Node", &[("Version", "1")]);
    let properties = t.sub_element(node, "Properties", &[("Version", "1")]);
    leaf(t, properties, "AMM.CurrentSolo", "[]");
    leaf(t, properties, "TL.SQTimePerPixel", "0.90634441087613293");
    leaf(t, properties, "Monitor.ProgramZoomIn", "0");
    leaf(t, properties, "Monitor.ProgramZoomOut", "0");
    leaf(t, properties, "TL.SQHeaderWidth", "180");
    leaf(t, properties, "TL.SQVisibleBaseTime", "0");
    leaf(t, properties, "TL.SQVideoVisibleBase", "0");
    leaf(t, properties, "TL.SQAudioVisibleBase", "0");
    leaf(t, properties, "TL.SQDataVisibleBase", "0");
    leaf(t, properties, "TL.SQHideShyTracks", "0");
    leaf(t, properties, "TL.SQAVDividerPosition", "0.5");
    leaf(t, properties, "MZ.WorkInPoint", "0");
    leaf(t, properties, "MZ.WorkOutPoint", &(WORK_AREA_LIMIT / fmt.frame_rate * fmt.frame_rate).to_string());
    leaf(t, properties, "MZ.EditLine", "0");
    leaf(t, properties, "MZ.Sequence.VideoTimeDisplayFormat", &fmt.time_display.to_string());
    leaf(t, properties, "MZ.Sequence.AudioTimeDisplayFormat", "200");
    leaf(t, properties, "MZ.Sequence.EditingModeGUID", EDITING_MODE);
    leaf(t, properties, "MZ.Sequence.PreviewUseMaxBitDepth", "false");
    leaf(t, properties, "MZ.Sequence.PreviewUseMaxRenderQuality", "false");
    leaf(t, properties, "MZ.Sequence.PreviewRenderingPresetPath", PREVIEW_PRESET);
    let codec = fmt.preview_codec.bytes().fold(0u64, |acc, b| (acc << 8) | b as u64);
    leaf(t, properties, "MZ.Sequence.PreviewRenderingPresetCodec", &codec.to_string());
    leaf(t, properties, "MZ.Sequence.PreviewRenderingClassID", "1061109567");
    leaf(t, properties, "MZ.Sequence.PreviewFrameSizeWidth", &fmt.width.to_string());
    leaf(t, properties, "MZ.Sequence.PreviewFrameSizeHeight", &fmt.height.to_string());
    let container = t.sub_element(sequence, "PersistentGroupContainer", &[("Version", "1")]);
    t.sub_element(container, "LinkContainer", &[("Version", "1")]);
    let groups = t.sub_element(sequence, "TrackGroups", &[("Version", "1")]);
    for (index, (media, role)) in
        [(VIDEO_MEDIA, "video_group"), (AUDIO_MEDIA, "audio_group"), (DATA_MEDIA, "data_group")].iter().enumerate()
    {
        let entry = t.sub_element(groups, "TrackGroup", &[("Version", "1"), ("Index", &index.to_string())]);
        leaf(t, entry, "First", media);
        t.sub_element(entry, "Second", &[("ObjectRef", &at[*role])]);
    }
    leaf(t, sequence, "Name", name);
    leaf(t, sequence, "PreviewFormatIdentifier", fmt.preview_format);
}
