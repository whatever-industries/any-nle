//! Multicam clip fragment (port of `models/multicam_builder.py`: upstream `build_multicam`,
//! `build_processed_clips_bin`, and the fork's `build_multicam_mono_audio`, ADR-0002).
//!
//! The fragment is a detached `PremiereData` element in the caller's arena, ready for
//! [`crate::splice::splice_fragment`]. The private helpers (`object`, `indexed`, `clip_core`,
//! `audio_component`, `strip_chain`, ...) are copies of the `sequence_builder` helpers this
//! builder imports; the constants are `sequence_builder`'s.
//!
//! GUIDs are drawn from the [`UuidSource`] in exactly the order Python's `_Uids` first asks for
//! each role (a role is minted once and then reused).

use std::collections::HashMap;

use super::time::{floor_div, TICKS_PER_SECOND};
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{build_leaf, indent_tree};
use crate::xml::{NodeId, Tree};

// --- sequence_builder constants ----------------------------------------------------------------

const VIDEO_MEDIA: &str = "228cda18-3625-4d2d-951e-348879e4ed93";
const AUDIO_MEDIA: &str = "80b8e3d5-6dca-4195-aefb-cb5f407ab009";
const DATA_MEDIA: &str = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5";
const EDITING_MODE: &str = "9678af98-a7b7-4bdb-b477-7ac9c8df4a4e";
const PREVIEW_PRESET: &str = "EncoderPresets\\SequencePreview\\9678af98-a7b7-4bdb-b477-7ac9c8df4a4e\\QuickTime.epr";
const AUDIO_FRAME_RATE: &str = "5292000";
const STEREO: &str = r#"[{"channellabel":100},{"channellabel":101}]"#;
const BUS_CHANNELS: usize = 32;
const CT_STEREO: &str = "1";
const CT_BUS: &str = "3";
const WORK_AREA_LIMIT: i64 = 60 * TICKS_PER_SECOND;
const LABEL: &str = "BE.Prefs.LabelColors.5";
const LABEL_COLOR: &str = "19005";
const PANNER_COMPONENT_ID: &str = "4294967280";
const NEXT_PANNER_ID: &str = "4294967279";
const TRACK_HEIGHT: &str = "41";
const FIRST_ID: usize = 40;

/// `sequence_builder._CLASS`, the entries this builder uses: tag -> (ClassID, Version).
fn class_of(tag: &str) -> (&'static str, &'static str) {
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
        "StereoTo16ChannelPanProcessor" => ("1a356806-5dc5-4e2f-914c-d8353e1a6581", "2"),
        "DefaultPanProcessor" => ("33a94282-ee2c-11d5-abc4-c1cd7f9e3c10", "2"),
        "AudioTrackInlet" => ("be3af080-e8c6-11d5-abc4-a1c6d5dee670", "4"),
        "AudioFader" => ("1a38c583-ed5c-11d5-abc4-c1cbf61ec590", "3"),
        "AudioMeter" => ("72ea4700-f615-11d5-abc4-c186585e63e0", "2"),
        "ScalarParam" => ("a714635e-a628-4b27-9d59-77eba47dbc1a", "10"),
        "BoolParam" => ("32657501-3aa4-445f-a49b-d09ecb9fa1ae", "10"),
        other => unreachable!("no class for {other}"),
    }
}

// --- multicam_builder constants ------------------------------------------------------------------

const COLOR_MANAGEMENT: &str =
    r#"{"autoToneMapEnabled":true,"enableLogColorManagement":2,"graphicsWhiteLuminance":203,"lutInterpolationMethod":1}"#;
const VR_CONFIGURATION: &str = r#"{"ambisonicsHRIR":"","ambisonicsMonitoringType":0,"capturedHorizontalView":360,"capturedVerticalView":180,"fieldOfHorizontalView":108,"fieldOfVerticalView":108,"projectionType":0,"stereoscopicEye":0,"stereoscopicType":0,"version":3}"#;
const OUTPUT_COLOR_SPACE: &str = r#"{"baseColorProfile":{"colorProfileData":"AQAAAGQAAAA=","colorProfileName":"BT.709,8-bit,Display-Referred"},"baseProfileType":1,"colorSpaceMetadata":{"peakLuminance":100}}"#;
const PREVIEW_FORMAT: &str = "367c336a-37a8-8b25-72cd-b234000000ea";
const BIN_CLASS_ID: &str = "dbfd6653-24da-480e-a35e-ba45e9504e4b";
const BIN_LABEL: &str = "BE.Prefs.LabelColors.7";

/// `_STRIP_TAGS`: the objects a track's private strip is made of.
const STRIP_TAGS: [&str; 7] = [
    "AudioComponentChain",
    "AudioFader",
    "AudioMeter",
    "StereoTo16ChannelPanProcessor",
    "AudioComponentParam",
    "ScalarParam",
    "BoolParam",
];

/// `_mono_layout(channels)`.
fn mono_layout(channels: usize) -> String {
    format!("[{}]", vec![r#"{"channellabel":0}"#; channels].join(","))
}

// --- helper copies from sequence_builder ----------------------------------------------------------

/// `_Uids`: mint each role's GUID on first use, then reuse it.
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

/// `_object`: a top-level object element with `ObjectID`/`ObjectUID`, `ClassID`, `Version`.
fn object(tree: &mut Tree, root: NodeId, tag: &str, object_id: Option<&str>, uid: Option<&str>) -> NodeId {
    let (class_id, version) = class_of(tag);
    let mut attrs: Vec<(&str, &str)> = Vec::new();
    if let Some(id) = object_id {
        attrs.push(("ObjectID", id));
    }
    if let Some(u) = uid {
        attrs.push(("ObjectUID", u));
    }
    attrs.push(("ClassID", class_id));
    attrs.push(("Version", version));
    tree.sub_element(root, tag, &attrs)
}

/// `_param_object`: an `AudioComponentParam` of the given param class.
fn param_object(tree: &mut Tree, root: NodeId, object_id: &str, kind: &str) -> NodeId {
    let (class_id, version) = class_of(kind);
    tree.sub_element(
        root,
        "AudioComponentParam",
        &[("ObjectID", object_id), ("ClassID", class_id), ("Version", version)],
    )
}

/// `_indexed`: `<tag Index="i" attribute="value"/>` entries.
fn indexed(tree: &mut Tree, parent: NodeId, tag: &str, attribute: &str, values: &[String]) {
    for (index, v) in values.iter().enumerate() {
        tree.sub_element(parent, tag, &[("Index", &index.to_string()), (attribute, v)]);
    }
}

/// `_clip_core`.
fn clip_core(tree: &mut Tree, parent: NodeId, source: &str, clip_id: &str) {
    let core = tree.sub_element(parent, "Clip", &[("Version", "18")]);
    let node = tree.sub_element(core, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    build_leaf(tree, properties, "asl.clip.label.color", LABEL_COLOR);
    build_leaf(tree, properties, "asl.clip.label.name", LABEL);
    tree.sub_element(core, "Source", &[("ObjectRef", source)]);
    build_leaf(tree, core, "ClipID", clip_id);
    build_leaf(tree, core, "InUse", "false");
}

/// `_audio_component`: the `AudioComponent` shell every fader, meter and panner sits in.
fn audio_component(
    tree: &mut Tree,
    parent: NodeId,
    component_id: &str,
    component_type: &str,
    params: &[String],
    layout: &str,
    channel_type: &str,
) -> NodeId {
    let audio = tree.sub_element(parent, "AudioComponent", &[("Version", "3")]);
    let component = tree.sub_element(audio, "Component", &[("Version", "7")]);
    if !params.is_empty() {
        let holder = tree.sub_element(component, "Params", &[("Version", "1")]);
        indexed(tree, holder, "Param", "ObjectRef", params);
    }
    build_leaf(tree, component, "ID", component_id);
    build_leaf(tree, audio, "FrameRate", AUDIO_FRAME_RATE);
    build_leaf(tree, audio, "AudioChannelLayout", layout);
    build_leaf(tree, audio, "ChannelType", channel_type);
    build_leaf(tree, audio, "AudioComponentType", component_type);
    audio
}

/// `_strip_chain`: `layout` `None` for a chain that declares neither layout nor channel type.
fn strip_chain(tree: &mut Tree, chain: NodeId, fader: &str, meter: &str, layout: Option<&str>, channel_type: &str) {
    let inner = tree.sub_element(chain, "ComponentChain", &[("Version", "3")]);
    let components = tree.sub_element(inner, "Components", &[("Version", "1")]);
    indexed(tree, components, "Component", "ObjectRef", &[fader.to_string(), meter.to_string()]);
    if let Some(layout) = layout {
        build_leaf(tree, chain, "AudioChannelLayout", layout);
        build_leaf(tree, chain, "ChannelType", channel_type);
    }
}

fn s(value: impl ToString) -> String {
    value.to_string()
}

/// `_allocate_multicam`: role -> ObjectID, consecutive from 40.
fn allocate_multicam() -> HashMap<String, String> {
    let mut roles: Vec<String> = ["logging", "master_chain", "audio_clip", "video_clip", "channel_groups", "audio_source"]
        .iter()
        .map(|r| r.to_string())
        .collect();
    roles.extend((0..BUS_CHANNELS).map(|i| format!("secondary_{i}")));
    for r in ["video_source", "channel_vector", "serializer_0", "serializer_1"] {
        roles.push(r.to_string());
    }
    for r in ["video_group", "audio_group", "data_group", "video_chain", "mix_track"] {
        roles.push(r.to_string());
    }
    for r in ["strip_chain", "panner", "master_strip_chain", "default_panner", "inlet"] {
        roles.push(r.to_string());
    }
    for r in ["fader", "meter", "balance", "fader_master", "meter_master"] {
        roles.push(r.to_string());
    }
    for r in ["volume_0", "mute_0", "volume_master", "mute_master"] {
        roles.push(r.to_string());
    }
    roles.into_iter().enumerate().map(|(offset, role)| (role, (FIRST_ID + offset).to_string())).collect()
}

/// `build_multicam`: a `PremiereData` fragment holding a new multicam clip's objects, everything
/// except the angle placements. `link` is false when the audio comes from an audio-only angle
/// (an EMPTY LinkContainer, 79_two_multicams). `frame_rate` is ticks per frame.
#[allow(clippy::too_many_arguments)]
pub fn build_multicam(
    tree: &mut Tree,
    uuids: &mut dyn UuidSource,
    name: &str,
    frame_rate: i64,
    width: i64,
    height: i64,
    time_display: i64,
    duration_ticks: i64,
    video_tracks: usize,
    link: bool,
) -> Result<NodeId> {
    let at_map = allocate_multicam();
    let at = |role: &str| -> String { at_map[role].clone() };
    let mut uid = Uids { values: HashMap::new(), source: uuids };
    let root = tree.new_element_with("PremiereData", &[("Version", "3")]);
    let layout_bus = mono_layout(BUS_CHANNELS);
    let video_uids: Vec<String> = (0..video_tracks).map(|i| uid.get(&format!("video_track_{i}"))).collect();
    let audio_uid = uid.get("audio_track");
    let work_out = floor_div(WORK_AREA_LIMIT, frame_rate)? * frame_rate;

    // --- panel item and master clip
    let item_uid = uid.get("item");
    let item = object(tree, root, "ClipProjectItem", None, Some(&item_uid));
    let project_item = tree.sub_element(item, "ProjectItem", &[("Version", "1")]);
    let node = tree.sub_element(project_item, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    build_leaf(tree, properties, "Column.PropertyText.Label", "BE.Prefs.LabelColors.5");
    build_leaf(tree, project_item, "Name", name);
    let master_uid = uid.get("master");
    tree.sub_element(item, "MasterClip", &[("ObjectURef", &master_uid)]);

    let master_uid = uid.get("master");
    let master = object(tree, root, "MasterClip", None, Some(&master_uid));
    let node = tree.sub_element(master, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    build_leaf(tree, properties, "Source.Monitor.Multicam.Enabled", "true");
    tree.sub_element(master, "LoggingInfo", &[("ObjectRef", &at("logging"))]);
    let chains = tree.sub_element(master, "AudioComponentChains", &[("Version", "1")]);
    indexed(tree, chains, "AudioComponentChain", "ObjectRef", &[at("master_chain")]);
    let clips = tree.sub_element(master, "Clips", &[("Version", "1")]);
    indexed(tree, clips, "Clip", "ObjectRef", &[at("audio_clip"), at("video_clip")]);
    tree.sub_element(master, "AudioClipChannelGroups", &[("ObjectRef", &at("channel_groups"))]);
    build_leaf(tree, master, "Name", name);
    build_leaf(tree, master, "MasterClipChangeVersion", "0");

    object(tree, root, "ClipLoggingInfo", Some(&at("logging")), None);

    let chain = object(tree, root, "AudioComponentChain", Some(&at("master_chain")), None);
    build_leaf(tree, chain, "DefaultVol", "true");
    build_leaf(tree, chain, "DefaultVolumeComponentID", "1");
    build_leaf(tree, chain, "DefaultChannelVolumeComponentID", "2");
    tree.sub_element(chain, "ComponentChain", &[("Version", "3")]);
    build_leaf(tree, chain, "AudioChannelLayout", STEREO);
    build_leaf(tree, chain, "ChannelType", CT_STEREO);

    let audio_clip = object(tree, root, "AudioClip", Some(&at("audio_clip")), None);
    let clip_id = uid.get("audio_clip_id");
    clip_core(tree, audio_clip, &at("audio_source"), &clip_id);
    let contents = tree.sub_element(audio_clip, "SecondaryContents", &[("Version", "1")]);
    let secondaries: Vec<String> = (0..BUS_CHANNELS).map(|i| at(&format!("secondary_{i}"))).collect();
    indexed(tree, contents, "SecondaryContentItem", "ObjectRef", &secondaries);
    build_leaf(tree, audio_clip, "AudioChannelLayout", &layout_bus);

    let video_clip = object(tree, root, "VideoClip", Some(&at("video_clip")), None);
    let clip_id = uid.get("video_clip_id");
    clip_core(tree, video_clip, &at("video_source"), &clip_id);

    let groups = object(tree, root, "ClipChannelGroupVectorSerializer", Some(&at("channel_groups")), None);
    let vectors = tree.sub_element(groups, "ClipChannelVectors", &[("Version", "1")]);
    indexed(tree, vectors, "ClipChannelVectorItem", "ObjectRef", &[at("channel_vector")]);

    let audio_source = object(tree, root, "AudioSequenceSource", Some(&at("audio_source")), None);
    let source = tree.sub_element(audio_source, "SequenceSource", &[("Version", "4")]);
    tree.sub_element(source, "Content", &[("Version", "10")]);
    let sequence_uid = uid.get("sequence");
    tree.sub_element(source, "Sequence", &[("ObjectURef", &sequence_uid)]);
    build_leaf(tree, audio_source, "OriginalDuration", &s(duration_ticks));

    for channel in 0..BUS_CHANNELS {
        let secondary = object(tree, root, "SecondaryContent", Some(&at(&format!("secondary_{channel}"))), None);
        tree.sub_element(secondary, "Content", &[("ObjectRef", &at("audio_source"))]);
        build_leaf(tree, secondary, "ChannelIndex", &s(channel));
    }

    let video_source = object(tree, root, "VideoSequenceSource", Some(&at("video_source")), None);
    let source = tree.sub_element(video_source, "SequenceSource", &[("Version", "4")]);
    tree.sub_element(source, "Content", &[("Version", "10")]);
    tree.sub_element(source, "Sequence", &[("ObjectURef", &sequence_uid)]);
    build_leaf(tree, video_source, "OriginalDuration", &s(duration_ticks));

    let vector = object(tree, root, "ClipChannelVectorSerializer", Some(&at("channel_vector")), None);
    let channels = tree.sub_element(vector, "ClipChannels", &[("Version", "1")]);
    indexed(tree, channels, "ClipChannelItem", "ObjectRef", &[at("serializer_0"), at("serializer_1")]);
    build_leaf(tree, vector, "ChannelType", CT_STEREO);
    for channel in 0..2 {
        let serializer = object(tree, root, "ClipChannelSerializer", Some(&at(&format!("serializer_{channel}"))), None);
        build_leaf(tree, serializer, "SourceClipIndex", "0");
        build_leaf(tree, serializer, "mSourceChannelIndex", &s(channel));
    }

    // --- the hidden sequence
    let sequence = object(tree, root, "Sequence", None, Some(&sequence_uid));
    let node = tree.sub_element(sequence, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    build_leaf(tree, properties, "MZ.WorkInPoint", "0");
    build_leaf(tree, properties, "MZ.WorkOutPoint", &s(work_out));
    build_leaf(tree, properties, "MZ.Sequence.VideoTimeDisplayFormat", &s(time_display));
    build_leaf(tree, properties, "MZ.Sequence.AudioTimeDisplayFormat", "200");
    build_leaf(tree, properties, "MZ.Sequence.EditingModeGUID", EDITING_MODE);
    build_leaf(tree, properties, "MZ.Sequence.PreviewUseMaxBitDepth", "false");
    build_leaf(tree, properties, "MZ.Sequence.PreviewUseMaxRenderQuality", "false");
    build_leaf(tree, properties, "MZ.Sequence.PreviewRenderingPresetPath", PREVIEW_PRESET);
    // `str(int.from_bytes(b"apcs", "big"))`: ProRes 422 LT.
    build_leaf(tree, properties, "MZ.Sequence.PreviewRenderingPresetCodec", &s(u32::from_be_bytes(*b"apcs")));
    build_leaf(tree, properties, "MZ.Sequence.PreviewRenderingClassID", "1061109567");
    build_leaf(tree, properties, "MZ.Sequence.PreviewFrameSizeWidth", &s(width));
    build_leaf(tree, properties, "MZ.Sequence.PreviewFrameSizeHeight", &s(height));
    let container = tree.sub_element(sequence, "PersistentGroupContainer", &[("Version", "1")]);
    let link_container = tree.sub_element(container, "LinkContainer", &[("Version", "1")]);
    if link {
        let links = tree.sub_element(link_container, "Links", &[("Version", "1")]);
        tree.sub_element(links, "Link", &[("Index", "0"), ("ObjectRef", "@link")]);
    }
    let track_groups = tree.sub_element(sequence, "TrackGroups", &[("Version", "1")]);
    for (index, (media, role)) in
        [(VIDEO_MEDIA, "video_group"), (AUDIO_MEDIA, "audio_group"), (DATA_MEDIA, "data_group")].into_iter().enumerate()
    {
        let entry = tree.sub_element(track_groups, "TrackGroup", &[("Version", "1"), ("Index", &index.to_string())]);
        build_leaf(tree, entry, "First", media);
        tree.sub_element(entry, "Second", &[("ObjectRef", &at(role))]);
    }
    build_leaf(tree, sequence, "Name", name);
    build_leaf(tree, sequence, "PreviewFormatIdentifier", PREVIEW_FORMAT);

    // --- track groups
    let group = object(tree, root, "VideoTrackGroup", Some(&at("video_group")), None);
    let inner = tree.sub_element(group, "TrackGroup", &[("Version", "1")]);
    let tracks = tree.sub_element(inner, "Tracks", &[("Version", "1")]);
    indexed(tree, tracks, "Track", "ObjectURef", &video_uids);
    build_leaf(tree, inner, "FrameRate", &s(frame_rate));
    build_leaf(tree, inner, "NextTrackID", &s(3 + video_tracks));
    build_leaf(tree, group, "ColorManagementSettings", COLOR_MANAGEMENT);
    build_leaf(tree, group, "ImmersiveVideoVRConfiguration", VR_CONFIGURATION);
    build_leaf(tree, group, "OutputColorSpace", OUTPUT_COLOR_SPACE);
    build_leaf(tree, group, "ToneMappingDesaturation", "0.5");
    build_leaf(tree, group, "AutoInputGamutCompressionEnabled", "true");
    build_leaf(tree, group, "IsGraphicsWhiteSameAsProject", "false");
    build_leaf(tree, group, "IsColorAwareEffectsEnabledSameAsProject", "false");
    build_leaf(tree, group, "FrameRect", &format!("0,0,{width},{height}"));
    let owner = tree.sub_element(group, "ComponentOwner", &[("Version", "1")]);
    tree.sub_element(owner, "Components", &[("ObjectRef", &at("video_chain"))]);

    let group = object(tree, root, "AudioTrackGroup", Some(&at("audio_group")), None);
    let inner = tree.sub_element(group, "TrackGroup", &[("Version", "1")]);
    let tracks = tree.sub_element(inner, "Tracks", &[("Version", "1")]);
    indexed(tree, tracks, "Track", "ObjectURef", &[audio_uid.clone()]);
    build_leaf(tree, inner, "FrameRate", AUDIO_FRAME_RATE);
    build_leaf(tree, inner, "NextTrackID", "6");
    tree.sub_element(group, "MasterTrack", &[("ObjectRef", &at("mix_track"))]);
    let group_id = uid.get("audio_group_id");
    build_leaf(tree, group, "ID", &group_id);
    build_leaf(tree, group, "AutomationSafeFlags", "0");
    build_leaf(tree, group, "NumAdaptiveChannels", "2");

    let group = object(tree, root, "DataTrackGroup", Some(&at("data_group")), None);
    let inner = tree.sub_element(group, "TrackGroup", &[("Version", "1")]);
    build_leaf(tree, inner, "FrameRate", &s(frame_rate));
    build_leaf(tree, inner, "NextTrackID", "1");

    // --- tracks
    for (index, track_uid) in video_uids.iter().enumerate() {
        let track = object(tree, root, "VideoClipTrack", None, Some(track_uid));
        let clip_track = tree.sub_element(track, "ClipTrack", &[("Version", "2")]);
        let inner = tree.sub_element(clip_track, "Track", &[("Version", "4")]);
        if index == 0 {
            let node = tree.sub_element(inner, "Node", &[("Version", "1")]);
            let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
            build_leaf(tree, properties, "TL.SQTrackExpanded", "0");
            build_leaf(tree, properties, "TL.SQTrackExpandedHeight", TRACK_HEIGHT);
            build_leaf(tree, properties, "MZ.TrackTargeted", "0");
        }
        build_leaf(tree, inner, "ID", &s(3 + index));
        build_leaf(tree, inner, "MediaType", VIDEO_MEDIA);
        build_leaf(tree, inner, "Index", &s(index));
        for holder_tag in ["ClipItems", "TransitionItems"] {
            let holder = tree.sub_element(clip_track, holder_tag, &[("Version", "3")]);
            build_leaf(tree, holder, "MediaType", VIDEO_MEDIA);
            build_leaf(tree, holder, "Index", &s(index));
        }
    }

    let chain = object(tree, root, "VideoComponentChain", Some(&at("video_chain")), None);
    tree.sub_element(chain, "ComponentChain", &[("Version", "3")]);

    let track = object(tree, root, "AudioClipTrack", None, Some(&audio_uid));
    let clip_track = tree.sub_element(track, "ClipTrack", &[("Version", "2")]);
    let inner = tree.sub_element(clip_track, "Track", &[("Version", "4")]);
    let node = tree.sub_element(inner, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    build_leaf(tree, properties, "TL.SQTrackExpanded", "0");
    build_leaf(tree, properties, "TL.SQTrackExpandedHeight", TRACK_HEIGHT);
    build_leaf(tree, properties, "MZ.TrackTargeted", "1");
    build_leaf(tree, properties, "CM.KeyframeMode", "true");
    build_leaf(tree, inner, "ID", "2");
    build_leaf(tree, inner, "MediaType", AUDIO_MEDIA);
    build_leaf(tree, inner, "Index", "0");
    for holder_tag in ["ClipItems", "TransitionItems"] {
        let holder = tree.sub_element(clip_track, holder_tag, &[("Version", "3")]);
        build_leaf(tree, holder, "MediaType", AUDIO_MEDIA);
        build_leaf(tree, holder, "Index", "0");
    }
    let audio_track = tree.sub_element(track, "AudioTrack", &[("Version", "12")]);
    let owner = tree.sub_element(audio_track, "ComponentOwner", &[("Version", "1")]);
    tree.sub_element(owner, "Components", &[("ObjectRef", &at("strip_chain"))]);
    tree.sub_element(audio_track, "Panner", &[("ObjectRef", &at("panner"))]);
    let track_id = uid.get("audio_track_id");
    build_leaf(tree, audio_track, "ID", &track_id);
    build_leaf(tree, audio_track, "NextPannerID", NEXT_PANNER_ID);

    let mix = object(tree, root, "AudioMixTrack", Some(&at("mix_track")), None);
    let audio_track = tree.sub_element(mix, "AudioTrack", &[("Version", "12")]);
    let owner = tree.sub_element(audio_track, "ComponentOwner", &[("Version", "1")]);
    tree.sub_element(owner, "Components", &[("ObjectRef", &at("master_strip_chain"))]);
    tree.sub_element(audio_track, "Panner", &[("ObjectRef", &at("default_panner"))]);
    let mix_id = uid.get("mix_track_id");
    build_leaf(tree, audio_track, "ID", &mix_id);
    build_leaf(tree, audio_track, "ChannelType", CT_BUS);
    build_leaf(tree, audio_track, "SubType", "3");
    build_leaf(tree, audio_track, "Assign", "0");
    build_leaf(tree, audio_track, "NextPannerID", NEXT_PANNER_ID);
    let inner = tree.sub_element(mix, "Track", &[("Version", "4")]);
    build_leaf(tree, inner, "ID", "1");
    build_leaf(tree, inner, "MediaType", AUDIO_MEDIA);
    build_leaf(tree, inner, "Index", "0");
    tree.sub_element(mix, "Inlet", &[("ObjectRef", &at("inlet"))]);

    // --- the mix graph
    let chain = object(tree, root, "AudioComponentChain", Some(&at("strip_chain")), None);
    strip_chain(tree, chain, &at("fader"), &at("meter"), Some(STEREO), CT_STEREO);
    let panner = object(tree, root, "StereoTo16ChannelPanProcessor", Some(&at("panner")), None);
    let direct = tree.sub_element(panner, "DirectPanProcessor", &[("Version", "2")]);
    let processor = tree.sub_element(direct, "PanProcessor", &[("Version", "3")]);
    audio_component(tree, processor, PANNER_COMPONENT_ID, "0", &[at("balance")], STEREO, CT_STEREO);
    build_leaf(tree, processor, "OutputAudioChannelLayout", &layout_bus);
    build_leaf(tree, direct, "Matrix", "[[0,[0]],[1,[1]]]");

    let chain = object(tree, root, "AudioComponentChain", Some(&at("master_strip_chain")), None);
    strip_chain(tree, chain, &at("fader_master"), &at("meter_master"), Some(&layout_bus), CT_BUS);
    let panner = object(tree, root, "DefaultPanProcessor", Some(&at("default_panner")), None);
    let processor = tree.sub_element(panner, "PanProcessor", &[("Version", "3")]);
    audio_component(tree, processor, PANNER_COMPONENT_ID, "0", &[], &layout_bus, CT_BUS);
    build_leaf(tree, panner, "DefaultPannerInputChannelType", CT_BUS);
    build_leaf(tree, panner, "DefaultPannerOutputChannelType", CT_BUS);

    let inlet = object(tree, root, "AudioTrackInlet", Some(&at("inlet")), None);
    let sources = tree.sub_element(inlet, "Sources", &[("Version", "1")]);
    indexed(tree, sources, "Source", "ObjectURef", &[audio_uid.clone()]);
    build_leaf(tree, inlet, "AudioChannelLayout", &layout_bus);
    build_leaf(tree, inlet, "ChannelType", CT_BUS);

    let fader = object(tree, root, "AudioFader", Some(&at("fader")), None);
    audio_component(tree, fader, "1", "1", &[at("volume_0"), at("mute_0")], STEREO, CT_STEREO);
    let meter = object(tree, root, "AudioMeter", Some(&at("meter")), None);
    audio_component(tree, meter, "2", "2", &[], STEREO, CT_STEREO);
    let balance = param_object(tree, root, &at("balance"), "ScalarParam");
    build_leaf(tree, balance, "StartKeyframe", "-91445760000000000,0.5,0,0,0,0,0,0");
    build_leaf(tree, balance, "CurrentValue", "0.5");
    build_leaf(tree, balance, "Name", "Balance");
    build_leaf(tree, balance, "IsInverted", "true");
    let fader = object(tree, root, "AudioFader", Some(&at("fader_master")), None);
    audio_component(tree, fader, "1", "1", &[at("volume_master"), at("mute_master")], &layout_bus, CT_BUS);
    let meter = object(tree, root, "AudioMeter", Some(&at("meter_master")), None);
    audio_component(tree, meter, "2", "2", &[], &layout_bus, CT_BUS);
    for strip in ["0", "master"] {
        let volume = param_object(tree, root, &at(&format!("volume_{strip}")), "ScalarParam");
        build_leaf(tree, volume, "Name", "Volume");
        build_leaf(tree, volume, "UpperBound", "5.6234130859375");
        build_leaf(tree, volume, "RangeLocked", "false");
        build_leaf(tree, volume, "UnitsString", "dB");
        let mute = param_object(tree, root, &at(&format!("mute_{strip}")), "BoolParam");
        build_leaf(tree, mute, "Name", "Mute");
        build_leaf(tree, mute, "RangeLocked", "false");
    }

    indent_tree(tree, root, 0);
    Ok(root)
}

/// `build_processed_clips_bin`: the `Processed Clips` bin Premiere files the source items into.
/// The returned `BinProjectItem` is detached (its tail cleared), ready for the caller to attach.
pub fn build_processed_clips_bin(tree: &mut Tree) -> NodeId {
    let root = tree.new_element_with("PremiereData", &[("Version", "3")]);
    let bin_item = tree.sub_element(
        root,
        "BinProjectItem",
        &[("ObjectUID", ""), ("ClassID", BIN_CLASS_ID), ("Version", "1")],
    );
    let project_item = tree.sub_element(bin_item, "ProjectItem", &[("Version", "1")]);
    let node = tree.sub_element(project_item, "Node", &[("Version", "1")]);
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    build_leaf(tree, properties, "Column.PropertyText.Label", BIN_LABEL);
    build_leaf(tree, project_item, "Name", "Processed Clips");
    tree.sub_element(bin_item, "ProjectItemContainer", &[("Version", "1")]);
    indent_tree(tree, root, 0);
    tree.set_tail(bin_item, None::<String>);
    tree.detach(bin_item);
    bin_item
}

// --- fork (avid2prproj, ADR-0002): N mono audio angles ---------------------------------------------

fn take_id(next_id: &mut u64) -> String {
    *next_id += 1;
    (*next_id - 1).to_string()
}

struct StripCloner<'a> {
    root: NodeId,
    by_id: &'a mut HashMap<String, NodeId>,
    next_id: &'a mut u64,
}

impl StripCloner<'_> {
    /// `copy_strip.clone`: a private copy of a strip object (and, recursively, of the strip
    /// objects it references) under fresh ObjectIDs; children are appended to the root before
    /// their parent, like Python.
    fn clone_object(&mut self, tree: &mut Tree, mapping: &mut HashMap<String, String>, object_id: &str) -> Result<String> {
        if let Some(done) = mapping.get(object_id) {
            return Ok(done.clone());
        }
        let original = *self
            .by_id
            .get(object_id)
            .ok_or_else(|| Error::Value(format!("KeyError: {object_id:?}")))?;
        let duplicate = tree.deep_copy(original);
        let fresh = take_id(self.next_id);
        mapping.insert(object_id.to_string(), fresh.clone());
        tree.set(duplicate, "ObjectID", fresh.clone());
        let nodes: Vec<NodeId> = tree.iter(duplicate).collect();
        for node in nodes {
            let Some(target) = tree.get(node, "ObjectRef").map(str::to_string) else { continue };
            let Some(&target_node) = self.by_id.get(&target) else { continue };
            if STRIP_TAGS.contains(&tree.tag(target_node)) {
                let cloned = self.clone_object(tree, mapping, &target)?;
                tree.set(node, "ObjectRef", cloned);
            }
        }
        tree.append(self.root, duplicate);
        self.by_id.insert(fresh.clone(), duplicate);
        Ok(fresh)
    }

    /// `copy_strip(track)`: give `track` private copies of its strip objects.
    fn copy_strip(&mut self, tree: &mut Tree, track: NodeId) -> Result<()> {
        let mut mapping = HashMap::new();
        for path in ["AudioTrack/ComponentOwner/Components", "AudioTrack/Panner"] {
            let reference = tree
                .find1(track, path)
                .ok_or_else(|| Error::Value(format!("track has no {path}")))?;
            let target = tree.get(reference, "ObjectRef").unwrap_or("").to_string();
            let fresh = self.clone_object(tree, &mut mapping, &target)?;
            tree.set(reference, "ObjectRef", fresh);
        }
        Ok(())
    }
}

/// `build_multicam_mono_audio`: `build_multicam` for N MONO audio angles, each on its own audio
/// track (shape from Premiere 26.5.2's multicam of one Avid clip, PRPROJ-0011). `video_tracks`
/// may be 0 (audio-only).
#[allow(clippy::too_many_arguments)]
pub fn build_multicam_mono_audio(
    tree: &mut Tree,
    uuids: &mut dyn UuidSource,
    name: &str,
    frame_rate: i64,
    width: i64,
    height: i64,
    time_display: i64,
    duration_ticks: i64,
    video_tracks: usize,
    audio_tracks: usize,
) -> Result<NodeId> {
    build_multicam_mono_audio_with_outputs(tree, uuids, name, frame_rate, width, height, time_display, duration_ticks, video_tracks, audio_tracks, audio_tracks)
}

/// The fixed-audio source keeps both cameras' audio tracks but only publishes
/// the two active mono output channels as source-item channel groups.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_multicam_mono_audio_with_outputs(
    tree: &mut Tree,
    uuids: &mut dyn UuidSource,
    name: &str,
    frame_rate: i64,
    width: i64,
    height: i64,
    time_display: i64,
    duration_ticks: i64,
    video_tracks: usize,
    audio_tracks: usize,
    outputs: usize,
) -> Result<NodeId> {
    if audio_tracks < 1 {
        return value("audio_tracks must be at least 1");
    }
    if outputs == 0 || outputs > audio_tracks {
        return value("invalid multicam output group count");
    }
    let root = build_multicam(tree, uuids, name, frame_rate, width, height, time_display, duration_ticks, video_tracks, false)?;
    let mut by_id: HashMap<String, NodeId> = HashMap::new();
    let mut by_uid: HashMap<String, NodeId> = HashMap::new();
    for &e in tree.children(root) {
        if let Some(id) = tree.get(e, "ObjectID").filter(|v| !v.is_empty()) {
            by_id.insert(id.to_string(), e);
        }
        if let Some(uid) = tree.get(e, "ObjectUID").filter(|v| !v.is_empty()) {
            by_uid.insert(uid.to_string(), e);
        }
    }
    let mut next_id: u64 = by_id.keys().map(|k| k.parse::<u64>().expect("numeric ObjectID")).max().unwrap_or(0) + 1;

    let first_child = |tree: &Tree, tag: &str| -> Result<NodeId> {
        tree.children(root)
            .iter()
            .copied()
            .find(|&e| tree.tag(e) == tag)
            .ok_or_else(|| Error::Value(format!("fragment has no <{tag}>")))
    };
    let missing = |what: &str| Error::Value(format!("fragment is missing {what}"));

    let group = first_child(tree, "AudioTrackGroup")?;
    let track_ref = tree.find1(group, "TrackGroup/Tracks/Track").ok_or_else(|| missing("audio track"))?;
    let track0_uid = tree.get(track_ref, "ObjectURef").unwrap_or("").to_string();
    let track0 = *by_uid.get(&track0_uid).ok_or_else(|| missing("audio track object"))?;
    let inlet = first_child(tree, "AudioTrackInlet")?;

    let mut track_uids = vec![track0_uid.clone()];
    for index in 1..audio_tracks {
        let track = tree.deep_copy(track0);
        let track_uid = format!("{track0_uid}-{index}");
        tree.set(track, "ObjectUID", track_uid.clone());
        let set_text = |tree: &mut Tree, path: &str, text: String| -> Result<()> {
            let node = tree.find1(track, path).ok_or_else(|| missing(path))?;
            tree.set_text(node, Some(text));
            Ok(())
        };
        set_text(tree, "ClipTrack/Track/ID", s(2 + index))?;
        for path in ["ClipTrack/Track/Index", "ClipTrack/ClipItems/Index", "ClipTrack/TransitionItems/Index"] {
            set_text(tree, path, s(index))?;
        }
        let base_id = {
            let node = tree.find1(track0, "AudioTrack/ID").ok_or_else(|| missing("AudioTrack/ID"))?;
            tree.text(node).unwrap_or("None").to_string()
        };
        set_text(tree, "AudioTrack/ID", format!("{base_id}-{index}"))?;
        StripCloner { root, by_id: &mut by_id, next_id: &mut next_id }.copy_strip(tree, track)?;
        tree.append(root, track);
        by_uid.insert(track_uid.clone(), track);
        track_uids.push(track_uid);
    }
    for (path, tag) in [("TrackGroup/Tracks", "Track"), ("Sources", "Source")] {
        let parent_of = if tag == "Track" { group } else { inlet };
        let parent = tree.find1(parent_of, path).ok_or_else(|| missing(path))?;
        for child in tree.children(parent).to_vec() {
            tree.remove(parent, child);
        }
        indexed(tree, parent, tag, "ObjectURef", &track_uids);
    }

    // Master: mono chains and single-channel vectors, one per audio track.
    let master = first_child(tree, "MasterClip")?;
    let chains = tree.find1(master, "AudioComponentChains").ok_or_else(|| missing("AudioComponentChains"))?;
    for reference in tree.children(chains).to_vec() {
        let id = tree.get(reference, "ObjectRef").unwrap_or("").to_string();
        let old = by_id.remove(&id).ok_or_else(|| Error::Value(format!("KeyError: {id:?}")))?;
        tree.remove(root, old);
        tree.remove(chains, reference);
    }
    let mut chain_ids = Vec::new();
    for _ in 0..outputs {
        let id = take_id(&mut next_id);
        let chain = object(tree, root, "AudioComponentChain", Some(&id), None);
        build_leaf(tree, chain, "DefaultVol", "true");
        build_leaf(tree, chain, "DefaultVolumeComponentID", "1");
        tree.sub_element(chain, "ComponentChain", &[("Version", "3")]);
        chain_ids.push(id);
    }
    indexed(tree, chains, "AudioComponentChain", "ObjectRef", &chain_ids);
    let groups_ref = tree.find1(master, "AudioClipChannelGroups").ok_or_else(|| missing("AudioClipChannelGroups"))?;
    let groups_id = tree.get(groups_ref, "ObjectRef").unwrap_or("").to_string();
    let groups = *by_id.get(&groups_id).ok_or_else(|| Error::Value(format!("KeyError: {groups_id:?}")))?;
    let vectors = tree.find1(groups, "ClipChannelVectors").ok_or_else(|| missing("ClipChannelVectors"))?;
    for reference in tree.children(vectors).to_vec() {
        let id = tree.get(reference, "ObjectRef").unwrap_or("").to_string();
        let vector = by_id.remove(&id).ok_or_else(|| Error::Value(format!("KeyError: {id:?}")))?;
        for item in tree.findall(vector, "ClipChannels/ClipChannelItem")? {
            let item_id = tree.get(item, "ObjectRef").unwrap_or("").to_string();
            let serializer = by_id.remove(&item_id).ok_or_else(|| Error::Value(format!("KeyError: {item_id:?}")))?;
            tree.remove(root, serializer);
        }
        tree.remove(root, vector);
        tree.remove(vectors, reference);
    }
    let mut vector_ids = Vec::new();
    for channel in 0..outputs {
        let serializer_id = take_id(&mut next_id);
        let serializer = object(tree, root, "ClipChannelSerializer", Some(&serializer_id), None);
        build_leaf(tree, serializer, "SourceClipIndex", "0");
        build_leaf(tree, serializer, "mSourceChannelIndex", &s(channel));
        let vector_id = take_id(&mut next_id);
        let vector = object(tree, root, "ClipChannelVectorSerializer", Some(&vector_id), None);
        let items = tree.sub_element(vector, "ClipChannels", &[("Version", "1")]);
        indexed(tree, items, "ClipChannelItem", "ObjectRef", &[serializer_id]);
        build_leaf(tree, vector, "ChannelType", "0");
        vector_ids.push(vector_id);
    }
    indexed(tree, vectors, "ClipChannelVectorItem", "ObjectRef", &vector_ids);
    indent_tree(tree, root, 0);
    Ok(root)
}
