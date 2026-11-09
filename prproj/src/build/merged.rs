//! Port of `vendor/py_premiere/models/merged_builder.py`: a merged clip's hidden sequence shell,
//! track/mix graph and panel-facing objects. Each builder creates a detached element in `tree`
//! and returns its handle. Builders that call `uuid.uuid4()` in Python take a [`UuidSource`] and
//! draw in the same order.

use super::{leaf, top};
use crate::ids::UuidSource;
use crate::xml::{NodeId, Tree};

const SEQUENCE_CLASS_ID: &str = "6a15d903-8739-11d5-af2d-9b7855ad8974";
const VIDEO_GROUP_CLASS_ID: &str = "9e9abf7a-0918-49c2-91ae-991b5dde77bb";
const AUDIO_GROUP_CLASS_ID: &str = "9b9238b9-53a8-4cc3-b03f-b36246d052e6";
const DATA_GROUP_CLASS_ID: &str = "b714b71d-6838-48dd-9b77-db19088ced7e";
const VIDEO_TRACK_CLASS_ID: &str = "f68dcd81-8805-11d5-af2d-9bfa89d4ddd4";
const AUDIO_TRACK_CLASS_ID: &str = "097f6203-99ae-11d5-84f2-8cf14bde7040";
const MIX_TRACK_CLASS_ID: &str = "4b1d8400-e89e-11d5-abc4-a1a13b1e80a0";
const LINK_CLASS_ID: &str = "149d4ea5-a7d4-4b34-9bb7-16d783904bf2";
const VIDEO_CHAIN_CLASS_ID: &str = "0970e08a-f58f-4108-b29a-1a717b8e12e2";
const AUDIO_CHAIN_CLASS_ID: &str = "3cb131d1-d3c0-47ae-a19a-bdf75ea11674";
const PAN_CLASS_ID: &str = "33a94282-ee2c-11d5-abc4-c1cd7f9e3c10";
const INLET_CLASS_ID: &str = "be3af080-e8c6-11d5-abc4-a1c6d5dee670";
const FADER_CLASS_ID: &str = "1a38c583-ed5c-11d5-abc4-c1cbf61ec590";
const METER_CLASS_ID: &str = "72ea4700-f615-11d5-abc4-c186585e63e0";
const VOLUME_PARAM_CLASS_ID: &str = "a714635e-a628-4b27-9d59-77eba47dbc1a";
const MUTE_PARAM_CLASS_ID: &str = "32657501-3aa4-445f-a49b-d09ecb9fa1ae";
const VIDEO_CLIP_CLASS_ID: &str = "9308dbef-2440-4acb-9ab2-953b9a4e82ec";
const AUDIO_CLIP_CLASS_ID: &str = "b8830d03-de02-41ee-84ec-fe566dc70cd9";
const ITEM_CLASS_ID: &str = "cb4e0ed7-aca1-4171-8525-e3658dec06dd";
const MASTER_CLASS_ID: &str = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf";
const LOGGING_CLASS_ID: &str = "77ab7fdd-dcdf-465d-9906-7a330ca1e738";
const VIDEO_SEQ_SOURCE_CLASS_ID: &str = "4752dfa9-7a7e-4a3b-a25b-cafde1a8d036";
const AUDIO_SEQ_SOURCE_CLASS_ID: &str = "e8d4cc83-38cb-491f-9d94-e5f7e3b205ee";

const VIDEO_MEDIA_TYPE: &str = "228cda18-3625-4d2d-951e-348879e4ed93";
const AUDIO_MEDIA_TYPE: &str = "80b8e3d5-6dca-4195-aefb-cb5f407ab009";
const DATA_MEDIA_TYPE: &str = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5";

const EDITING_MODE: &str = "9678af98-a7b7-4bdb-b477-7ac9c8df4a4e";
const PREVIEW_PRESET: &str = "EncoderPresets\\SequencePreview\\9678af98-a7b7-4bdb-b477-7ac9c8df4a4e\\I-Frame Only MPEG.epr";
const PREVIEW_CLASS: &str = "1297106761";
const PREVIEW_FORMAT: &str = "d5c4dab8-b0a6-b1a1-0114-ec7d000000fa";
const COLOR_MANAGEMENT: &str =
    "{\"autoToneMapEnabled\":true,\"enableLogColorManagement\":2,\"lutInterpolationMethod\":1}";
const VR_CONFIGURATION: &str = "{\"ambisonicsHRIR\":\"\",\"ambisonicsMonitoringType\":0,\"capturedHorizontalView\":360,\"capturedVerticalView\":180,\"fieldOfHorizontalView\":108,\"fieldOfVerticalView\":108,\"projectionType\":0,\"stereoscopicEye\":0,\"stereoscopicType\":0,\"version\":3}";
const OUTPUT_COLOR_SPACE: &str =
    "{\"baseColorProfile\":{\"colorProfileName\":\"BT.709,8-bit,Display-Referred\"},\"baseProfileType\":1}";
const NEXT_PANNER: &str = "4294967279";
const VOLUME_UPPER_BOUND: &str = "5.6234130859375";
/// `media_import.MONO_LAYOUT` (private copy; media.rs belongs to another module).
const MONO_LAYOUT: &str = "[{\"channellabel\":0}]";

/// A `Node`/`Properties` property bag whose open tag sits at `depth` tabs.
pub fn bag_node(tree: &mut Tree, depth: usize, keys: &[(&str, &str)]) -> NodeId {
    let indent = "\t".repeat(depth);
    let node = tree.new_element_with("Node", &[("Version", "1")]);
    tree.set_text(node, Some(format!("\n{indent}\t")));
    tree.set_tail(node, Some(format!("\n{indent}")));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some(format!("\n{indent}\t\t")));
    tree.set_tail(properties, Some(format!("\n{indent}")));
    for (index, (key, value)) in keys.iter().enumerate() {
        let l = tree.sub_element(properties, key, &[]);
        tree.set_text(l, Some(*value));
        let tail = if index + 1 < keys.len() { format!("\n{indent}\t\t") } else { format!("\n{indent}\t") };
        tree.set_tail(l, Some(tail));
    }
    node
}

/// `ET.SubElement(parent, tag, attrs).tail = tail`.
fn sub_tail(tree: &mut Tree, parent: NodeId, tag: &str, attrs: &[(&str, &str)], tail: &str) -> NodeId {
    let e = tree.sub_element(parent, tag, attrs);
    tree.set_tail(e, Some(tail));
    e
}

/// `ET.SubElement(parent, tag, attrs)` with text and tail.
fn sub_block(tree: &mut Tree, parent: NodeId, tag: &str, attrs: &[(&str, &str)], text: &str, tail: &str) -> NodeId {
    let e = tree.sub_element(parent, tag, attrs);
    tree.set_text(e, Some(text));
    tree.set_tail(e, Some(tail));
    e
}

/// The `MZ.MergeClipUtils.*` bag a duplicated source master carries.
pub fn build_master_bag(tree: &mut Tree, keys: &[(&str, &str)]) -> NodeId {
    bag_node(tree, 2, keys)
}

/// The merged clip's hidden `Sequence`.
#[allow(clippy::too_many_arguments)]
pub fn build_merged_sequence(
    tree: &mut Tree,
    sequence_uid: &str,
    name: &str,
    video_display_format: i64,
    end_ticks: i64,
    width: i64,
    height: i64,
    video_group_id: &str,
    audio_group_id: &str,
    data_group_id: &str,
) -> NodeId {
    let element = top(tree, "Sequence", SEQUENCE_CLASS_ID, "12", Some(sequence_uid));
    let end = end_ticks.to_string();
    let vdf = video_display_format.to_string();
    let w = width.to_string();
    let h = height.to_string();
    let bag = bag_node(
        tree,
        2,
        &[
            ("BE.Sequence.IsMergedClip", "true"),
            ("MZ.WorkInPoint", "0"),
            ("MZ.WorkOutPoint", &end),
            ("MZ.EditLine", "0"),
            ("MZ.Sequence.VideoTimeDisplayFormat", &vdf),
            ("MZ.Sequence.AudioTimeDisplayFormat", "200"),
            ("MZ.Sequence.EditingModeGUID", EDITING_MODE),
            ("MZ.Sequence.PreviewUseMaxBitDepth", "false"),
            ("MZ.Sequence.PreviewUseMaxRenderQuality", "false"),
            ("MZ.Sequence.PreviewRenderingPresetPath", PREVIEW_PRESET),
            ("MZ.Sequence.PreviewRenderingPresetCodec", "0"),
            ("MZ.Sequence.PreviewRenderingClassID", PREVIEW_CLASS),
            ("MZ.Sequence.PreviewFrameSizeWidth", &w),
            ("MZ.Sequence.PreviewFrameSizeHeight", &h),
            ("MZ.MergedClip.AudioTimecodeBaseTrackNumber", "-1"),
        ],
    );
    tree.append(element, bag);
    let container = sub_block(tree, element, "PersistentGroupContainer", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    let links = sub_block(tree, container, "LinkContainer", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t");
    let inner = sub_block(tree, links, "Links", &[("Version", "1")], "\n\t\t\t\t\t", "\n\t\t\t");
    // The ObjectRef is patched in once the Link object exists.
    sub_tail(tree, inner, "Link", &[("Index", "0"), ("ObjectRef", "")], "\n\t\t\t\t");
    let groups = sub_block(tree, element, "TrackGroups", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    let pairs = [
        (VIDEO_MEDIA_TYPE, video_group_id),
        (AUDIO_MEDIA_TYPE, audio_group_id),
        (DATA_MEDIA_TYPE, data_group_id),
    ];
    for (index, (media_type, group_id)) in pairs.iter().enumerate() {
        let i = index.to_string();
        let tail = if index < pairs.len() - 1 { "\n\t\t\t" } else { "\n\t\t" };
        let pair = sub_block(tree, groups, "TrackGroup", &[("Version", "1"), ("Index", &i)], "\n\t\t\t\t", tail);
        leaf(tree, pair, "First", media_type, "\n\t\t\t\t");
        sub_tail(tree, pair, "Second", &[("ObjectRef", group_id)], "\n\t\t\t");
    }
    leaf(tree, element, "Name", name, "\n\t\t");
    leaf(tree, element, "PreviewFormatIdentifier", PREVIEW_FORMAT, "\n\t");
    element
}

/// `VideoTrackGroup`.
pub fn build_video_group(
    tree: &mut Tree,
    track_uid: &str,
    video_timebase: i64,
    width: i64,
    height: i64,
    chain_id: &str,
) -> NodeId {
    let element = top(tree, "VideoTrackGroup", VIDEO_GROUP_CLASS_ID, "13", None);
    let inner = sub_block(tree, element, "TrackGroup", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    let tracks = sub_block(tree, inner, "Tracks", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t\t");
    sub_tail(tree, tracks, "Track", &[("Index", "0"), ("ObjectURef", track_uid)], "\n\t\t\t");
    leaf(tree, inner, "FrameRate", &video_timebase.to_string(), "\n\t\t\t");
    leaf(tree, inner, "NextTrackID", "2", "\n\t\t");
    leaf(tree, element, "ColorManagementSettings", COLOR_MANAGEMENT, "\n\t\t");
    leaf(tree, element, "ImmersiveVideoVRConfiguration", VR_CONFIGURATION, "\n\t\t");
    leaf(tree, element, "OutputColorSpace", OUTPUT_COLOR_SPACE, "\n\t\t");
    leaf(tree, element, "AutoInputGamutCompressionEnabled", "true", "\n\t\t");
    leaf(tree, element, "IsGraphicsWhiteSameAsProject", "false", "\n\t\t");
    leaf(tree, element, "IsColorAwareEffectsEnabledSameAsProject", "false", "\n\t\t");
    leaf(tree, element, "FrameRect", &format!("0,0,{width},{height}"), "\n\t\t");
    let owner = sub_block(tree, element, "ComponentOwner", &[("Version", "1")], "\n\t\t\t", "\n\t");
    sub_tail(tree, owner, "Components", &[("ObjectRef", chain_id)], "\n\t\t");
    element
}

/// `AudioTrackGroup`; draws one UUID (its `ID`).
pub fn build_audio_group(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    track_uids: &[&str],
    audio_rate: i64,
    mix_track_id: &str,
) -> NodeId {
    let element = top(tree, "AudioTrackGroup", AUDIO_GROUP_CLASS_ID, "6", None);
    let inner = sub_block(tree, element, "TrackGroup", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    let tracks = sub_block(tree, inner, "Tracks", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t\t");
    for (index, track_uid) in track_uids.iter().enumerate() {
        let i = index.to_string();
        let tail = if index + 1 < track_uids.len() { "\n\t\t\t\t" } else { "\n\t\t\t" };
        sub_tail(tree, tracks, "Track", &[("Index", &i), ("ObjectURef", track_uid)], tail);
    }
    leaf(tree, inner, "FrameRate", &audio_rate.to_string(), "\n\t\t\t");
    leaf(tree, inner, "NextTrackID", "4", "\n\t\t");
    sub_tail(tree, element, "MasterTrack", &[("ObjectRef", mix_track_id)], "\n\t\t");
    leaf(tree, element, "ID", &ids.uuid4().to_string(), "\n\t\t");
    leaf(tree, element, "AutomationSafeFlags", "0", "\n\t\t");
    leaf(tree, element, "NumAdaptiveChannels", "1", "\n\t");
    element
}

/// `DataTrackGroup`.
pub fn build_data_group(tree: &mut Tree, video_timebase: i64) -> NodeId {
    let element = top(tree, "DataTrackGroup", DATA_GROUP_CLASS_ID, "1", None);
    let inner = sub_block(tree, element, "TrackGroup", &[("Version", "1")], "\n\t\t\t", "\n\t");
    leaf(tree, inner, "FrameRate", &video_timebase.to_string(), "\n\t\t\t");
    leaf(tree, inner, "NextTrackID", "1", "\n\t\t");
    element
}

/// The empty `ClipTrack` shell; `Track.add_clip` fills ClipItems in.
fn clip_track_shell(tree: &mut Tree, parent: NodeId, media_type: &str, track_id: i64, keyframe_mode: bool, index: i64) {
    let clip_track = sub_block(tree, parent, "ClipTrack", &[("Version", "2")], "\n\t\t\t", "\n\t");
    let track = sub_block(tree, clip_track, "Track", &[("Version", "4")], "\n\t\t\t\t", "\n\t\t\t");
    let mut keys = vec![
        ("TL.SQTrackExpanded", "0"),
        ("TL.SQTrackExpandedHeight", "41"),
        ("MZ.TrackTargeted", "1"),
    ];
    if keyframe_mode {
        keys.push(("CM.KeyframeMode", "true"));
    }
    let bag = bag_node(tree, 4, &keys);
    tree.append(track, bag);
    let idx = index.to_string();
    leaf(tree, track, "ID", &track_id.to_string(), "\n\t\t\t\t");
    leaf(tree, track, "MediaType", media_type, "\n\t\t\t\t");
    leaf(tree, track, "Index", &idx, "\n\t\t\t");
    let clip_items = sub_block(tree, clip_track, "ClipItems", &[("Version", "3")], "\n\t\t\t\t", "\n\t\t\t");
    leaf(tree, clip_items, "MediaType", media_type, "\n\t\t\t\t");
    leaf(tree, clip_items, "Index", &idx, "\n\t\t\t");
    let transitions = sub_block(tree, clip_track, "TransitionItems", &[("Version", "3")], "\n\t\t\t\t", "\n\t\t");
    leaf(tree, transitions, "MediaType", media_type, "\n\t\t\t\t");
    leaf(tree, transitions, "Index", &idx, "\n\t\t\t");
}

/// `VideoClipTrack`.
pub fn build_video_track(tree: &mut Tree, track_uid: &str) -> NodeId {
    let element = top(tree, "VideoClipTrack", VIDEO_TRACK_CLASS_ID, "1", Some(track_uid));
    clip_track_shell(tree, element, VIDEO_MEDIA_TYPE, 1, false, 0);
    element
}

/// `AudioClipTrack`; draws one UUID (its `AudioTrack/ID`). Python defaults: `track_id=2`, `index=0`.
pub fn build_audio_track(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    track_uid: &str,
    chain_id: &str,
    panner_id: &str,
    track_id: i64,
    index: i64,
) -> NodeId {
    let element = top(tree, "AudioClipTrack", AUDIO_TRACK_CLASS_ID, "7", Some(track_uid));
    clip_track_shell(tree, element, AUDIO_MEDIA_TYPE, track_id, true, index);
    // The shell closed the object; reopen it for the AudioTrack sibling.
    let last = *tree.children(element).last().unwrap();
    tree.set_tail(last, Some("\n\t\t"));
    let audio = sub_block(tree, element, "AudioTrack", &[("Version", "12")], "\n\t\t\t", "\n\t");
    let owner = sub_block(tree, audio, "ComponentOwner", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t\t");
    sub_tail(tree, owner, "Components", &[("ObjectRef", chain_id)], "\n\t\t\t");
    sub_tail(tree, audio, "Panner", &[("ObjectRef", panner_id)], "\n\t\t\t");
    leaf(tree, audio, "ID", &ids.uuid4().to_string(), "\n\t\t\t");
    leaf(tree, audio, "ChannelType", "0", "\n\t\t\t");
    leaf(tree, audio, "NextPannerID", NEXT_PANNER, "\n\t\t");
    element
}

/// `AudioMixTrack`; draws one UUID (its `AudioTrack/ID`).
pub fn build_mix_track(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    chain_id: &str,
    panner_id: &str,
    inlet_id: &str,
) -> NodeId {
    let element = top(tree, "AudioMixTrack", MIX_TRACK_CLASS_ID, "4", None);
    let audio = sub_block(tree, element, "AudioTrack", &[("Version", "12")], "\n\t\t\t", "\n\t\t");
    let owner = sub_block(tree, audio, "ComponentOwner", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t\t");
    sub_tail(tree, owner, "Components", &[("ObjectRef", chain_id)], "\n\t\t\t");
    sub_tail(tree, audio, "Panner", &[("ObjectRef", panner_id)], "\n\t\t\t");
    leaf(tree, audio, "ID", &ids.uuid4().to_string(), "\n\t\t\t");
    leaf(tree, audio, "ChannelType", "0", "\n\t\t\t");
    leaf(tree, audio, "SubType", "3", "\n\t\t\t");
    leaf(tree, audio, "Assign", "0", "\n\t\t\t");
    leaf(tree, audio, "NextPannerID", NEXT_PANNER, "\n\t\t");
    let track = sub_block(tree, element, "Track", &[("Version", "4")], "\n\t\t\t", "\n\t\t");
    leaf(tree, track, "ID", "1", "\n\t\t\t");
    leaf(tree, track, "MediaType", AUDIO_MEDIA_TYPE, "\n\t\t\t");
    leaf(tree, track, "Index", "0", "\n\t\t");
    sub_tail(tree, element, "Inlet", &[("ObjectRef", inlet_id)], "\n\t");
    element
}

/// `VideoComponentChain` with an empty `ComponentChain`.
pub fn build_empty_video_chain(tree: &mut Tree) -> NodeId {
    let element = top(tree, "VideoComponentChain", VIDEO_CHAIN_CLASS_ID, "3", None);
    sub_block(tree, element, "ComponentChain", &[("Version", "3")], "\n\t\t", "\n\t");
    element
}

/// `AudioComponentChain` of fader then meter.
pub fn build_fader_chain(tree: &mut Tree, fader_id: &str, meter_id: &str) -> NodeId {
    let element = top(tree, "AudioComponentChain", AUDIO_CHAIN_CLASS_ID, "4", None);
    let chain = sub_block(tree, element, "ComponentChain", &[("Version", "3")], "\n\t\t\t", "\n\t");
    let components = sub_block(tree, chain, "Components", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t");
    for (index, target) in [fader_id, meter_id].iter().enumerate() {
        let i = index.to_string();
        let tail = if index == 0 { "\n\t\t\t\t" } else { "\n\t\t\t" };
        sub_tail(tree, components, "Component", &[("Index", &i), ("ObjectRef", target)], tail);
    }
    element
}

fn audio_component(
    tree: &mut Tree,
    parent: NodeId,
    audio_rate: i64,
    component_type: &str,
    component_id: &str,
    param_ids: Option<&[&str]>,
) {
    let audio = sub_block(tree, parent, "AudioComponent", &[("Version", "3")], "\n\t\t\t", "\n\t");
    let component = sub_block(tree, audio, "Component", &[("Version", "7")], "\n\t\t\t\t", "\n\t\t\t");
    if let Some(param_ids) = param_ids.filter(|p| !p.is_empty()) {
        let params = sub_block(tree, component, "Params", &[("Version", "1")], "\n\t\t\t\t\t", "\n\t\t\t\t");
        for (index, param_id) in param_ids.iter().enumerate() {
            let i = index.to_string();
            let tail = if index + 1 < param_ids.len() { "\n\t\t\t\t\t" } else { "\n\t\t\t\t" };
            sub_tail(tree, params, "Param", &[("Index", &i), ("ObjectRef", param_id)], tail);
        }
    }
    leaf(tree, component, "ID", component_id, "\n\t\t\t");
    leaf(tree, audio, "FrameRate", &audio_rate.to_string(), "\n\t\t\t");
    leaf(tree, audio, "AudioChannelLayout", MONO_LAYOUT, "\n\t\t\t");
    leaf(tree, audio, "ChannelType", "0", "\n\t\t\t");
    leaf(tree, audio, "AudioComponentType", component_type, "\n\t\t");
}

/// `AudioFader` with its parameter refs.
pub fn build_fader(tree: &mut Tree, audio_rate: i64, param_ids: &[&str]) -> NodeId {
    let element = top(tree, "AudioFader", FADER_CLASS_ID, "3", None);
    audio_component(tree, element, audio_rate, "1", "1", Some(param_ids));
    element
}

/// `AudioMeter`.
pub fn build_meter(tree: &mut Tree, audio_rate: i64) -> NodeId {
    let element = top(tree, "AudioMeter", METER_CLASS_ID, "2", None);
    audio_component(tree, element, audio_rate, "2", "2", None);
    element
}

/// `DefaultPanProcessor`.
pub fn build_pan_processor(tree: &mut Tree, audio_rate: i64) -> NodeId {
    let element = top(tree, "DefaultPanProcessor", PAN_CLASS_ID, "2", None);
    let processor = sub_block(tree, element, "PanProcessor", &[("Version", "3")], "\n\t\t\t", "\n\t\t");
    let audio = sub_block(tree, processor, "AudioComponent", &[("Version", "3")], "\n\t\t\t\t", "\n\t\t");
    let component = sub_block(tree, audio, "Component", &[("Version", "7")], "\n\t\t\t\t\t", "\n\t\t\t\t");
    leaf(tree, component, "ID", "4294967280", "\n\t\t\t\t");
    leaf(tree, audio, "FrameRate", &audio_rate.to_string(), "\n\t\t\t\t");
    leaf(tree, audio, "AudioChannelLayout", MONO_LAYOUT, "\n\t\t\t\t");
    leaf(tree, audio, "ChannelType", "0", "\n\t\t\t\t");
    leaf(tree, audio, "AudioComponentType", "0", "\n\t\t\t");
    leaf(tree, element, "DefaultPannerInputChannelType", "0", "\n\t\t");
    leaf(tree, element, "DefaultPannerOutputChannelType", "0", "\n\t");
    element
}

/// `AudioTrackInlet` over the given audio track UIDs.
pub fn build_inlet(tree: &mut Tree, track_uids: &[&str]) -> NodeId {
    let element = top(tree, "AudioTrackInlet", INLET_CLASS_ID, "4", None);
    let sources = sub_block(tree, element, "Sources", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    for (index, track_uid) in track_uids.iter().enumerate() {
        let i = index.to_string();
        let tail = if index + 1 < track_uids.len() { "\n\t\t\t" } else { "\n\t\t" };
        sub_tail(tree, sources, "Source", &[("Index", &i), ("ObjectURef", track_uid)], tail);
    }
    leaf(tree, element, "AudioChannelLayout", MONO_LAYOUT, "\n\t\t");
    leaf(tree, element, "ChannelType", "0", "\n\t");
    element
}

/// The Volume `AudioComponentParam`.
pub fn build_volume_param(tree: &mut Tree) -> NodeId {
    let element = top(tree, "AudioComponentParam", VOLUME_PARAM_CLASS_ID, "10", None);
    leaf(tree, element, "Name", "Volume", "\n\t\t");
    leaf(tree, element, "UpperBound", VOLUME_UPPER_BOUND, "\n\t\t");
    leaf(tree, element, "RangeLocked", "false", "\n\t\t");
    leaf(tree, element, "UnitsString", "dB", "\n\t");
    element
}

/// The Mute `AudioComponentParam`.
pub fn build_mute_param(tree: &mut Tree) -> NodeId {
    let element = top(tree, "AudioComponentParam", MUTE_PARAM_CLASS_ID, "10", None);
    leaf(tree, element, "Name", "Mute", "\n\t\t");
    leaf(tree, element, "RangeLocked", "false", "\n\t");
    element
}

/// The `Link` binding placed halves.
pub fn build_link(tree: &mut Tree, item_ids: &[&str]) -> NodeId {
    let element = top(tree, "Link", LINK_CLASS_ID, "1", None);
    let group = sub_block(tree, element, "TrackItemGroup", &[("Version", "1")], "\n\t\t\t", "\n\t");
    let items = sub_block(tree, group, "TrackItems", &[("Version", "1")], "\n\t\t\t\t", "\n\t\t");
    for (index, item_id) in item_ids.iter().enumerate() {
        let i = index.to_string();
        let tail = if index + 1 < item_ids.len() { "\n\t\t\t\t" } else { "\n\t\t\t" };
        sub_tail(tree, items, "TrackItem", &[("Index", &i), ("ObjectRef", item_id)], tail);
    }
    element
}

/// `VideoSequenceSource` (when `tag` says so) or `AudioSequenceSource`.
pub fn build_sequence_source(tree: &mut Tree, tag: &str, sequence_uid: &str, duration_ticks: i64) -> NodeId {
    let (class_id, version) = if tag == "VideoSequenceSource" {
        (VIDEO_SEQ_SOURCE_CLASS_ID, "3")
    } else {
        (AUDIO_SEQ_SOURCE_CLASS_ID, "7")
    };
    let element = top(tree, tag, class_id, version, None);
    let source = sub_block(tree, element, "SequenceSource", &[("Version", "4")], "\n\t\t\t", "\n\t\t");
    sub_block(tree, source, "Content", &[("Version", "10")], "\n\t\t\t", "\n\t\t\t");
    sub_tail(tree, source, "Sequence", &[("ObjectURef", sequence_uid)], "\n\t\t");
    leaf(tree, element, "OriginalDuration", &duration_ticks.to_string(), "\n\t");
    element
}

/// The panel master's `VideoClip` template; draws one UUID (`ClipID`).
pub fn build_panel_video_template(tree: &mut Tree, ids: &mut dyn UuidSource, sequence_source_id: &str) -> NodeId {
    let element = top(tree, "VideoClip", VIDEO_CLIP_CLASS_ID, "11", None);
    let core = sub_block(tree, element, "Clip", &[("Version", "18")], "\n\t\t\t", "\n\t");
    sub_tail(tree, core, "Source", &[("ObjectRef", sequence_source_id)], "\n\t\t\t");
    leaf(tree, core, "ClipID", &ids.uuid4().to_string(), "\n\t\t\t");
    leaf(tree, core, "InUse", "false", "\n\t\t");
    element
}

/// The panel master's `AudioClip` template; draws one UUID (`ClipID`).
pub fn build_panel_audio_template(
    tree: &mut Tree,
    ids: &mut dyn UuidSource,
    sequence_source_id: &str,
    secondary_id: &str,
) -> NodeId {
    let element = top(tree, "AudioClip", AUDIO_CLIP_CLASS_ID, "8", None);
    let core = sub_block(tree, element, "Clip", &[("Version", "18")], "\n\t\t\t", "\n\t\t");
    sub_tail(tree, core, "Source", &[("ObjectRef", sequence_source_id)], "\n\t\t\t");
    leaf(tree, core, "ClipID", &ids.uuid4().to_string(), "\n\t\t\t");
    leaf(tree, core, "InUse", "false", "\n\t\t");
    let secondaries = sub_block(tree, element, "SecondaryContents", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    sub_tail(
        tree,
        secondaries,
        "SecondaryContentItem",
        &[("Index", "0"), ("ObjectRef", secondary_id)],
        "\n\t\t",
    );
    leaf(tree, element, "AudioChannelLayout", MONO_LAYOUT, "\n\t");
    element
}

/// The panel `MasterClip` (audio template first in `Clips`).
#[allow(clippy::too_many_arguments)]
pub fn build_panel_master(
    tree: &mut Tree,
    master_uid: &str,
    logging_id: &str,
    chain_ids: &[&str],
    audio_template_id: &str,
    video_template_id: &str,
    groups_id: &str,
    name: &str,
) -> NodeId {
    let element = top(tree, "MasterClip", MASTER_CLASS_ID, "12", Some(master_uid));
    sub_tail(tree, element, "LoggingInfo", &[("ObjectRef", logging_id)], "\n\t\t");
    let chains = sub_block(tree, element, "AudioComponentChains", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    for (index, chain_id) in chain_ids.iter().enumerate() {
        let i = index.to_string();
        let tail = if index + 1 < chain_ids.len() { "\n\t\t\t" } else { "\n\t\t" };
        sub_tail(tree, chains, "AudioComponentChain", &[("Index", &i), ("ObjectRef", chain_id)], tail);
    }
    let clips = sub_block(tree, element, "Clips", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    sub_tail(tree, clips, "Clip", &[("Index", "0"), ("ObjectRef", audio_template_id)], "\n\t\t\t");
    sub_tail(tree, clips, "Clip", &[("Index", "1"), ("ObjectRef", video_template_id)], "\n\t\t");
    sub_tail(tree, element, "AudioClipChannelGroups", &[("ObjectRef", groups_id)], "\n\t\t");
    leaf(tree, element, "Name", name, "\n\t\t");
    leaf(tree, element, "MasterClipChangeVersion", "0", "\n\t");
    element
}

/// The panel master's `ClipLoggingInfo`.
pub fn build_panel_logging(
    tree: &mut Tree,
    clip_name: &str,
    timecode_format: i64,
    duration_ticks: i64,
    video_timebase: i64,
) -> NodeId {
    let element = top(tree, "ClipLoggingInfo", LOGGING_CLASS_ID, "10", None);
    leaf(tree, element, "CaptureMode", "2", "\n\t\t");
    leaf(tree, element, "ClipName", clip_name, "\n\t\t");
    leaf(tree, element, "TimecodeFormat", &timecode_format.to_string(), "\n\t\t");
    leaf(tree, element, "MediaInPoint", "0", "\n\t\t");
    leaf(tree, element, "MediaOutPoint", &duration_ticks.to_string(), "\n\t\t");
    leaf(tree, element, "MediaFrameRate", &video_timebase.to_string(), "\n\t");
    element
}

/// The panel `ClipProjectItem`.
pub fn build_panel_item(tree: &mut Tree, item_uid: &str, master_uid: &str, name: &str) -> NodeId {
    let element = top(tree, "ClipProjectItem", ITEM_CLASS_ID, "1", Some(item_uid));
    let project_item = sub_block(tree, element, "ProjectItem", &[("Version", "1")], "\n\t\t\t", "\n\t\t");
    let bag = bag_node(tree, 3, &[("Column.PropertyText.Label", "BE.Prefs.LabelColors.0")]);
    tree.append(project_item, bag);
    leaf(tree, project_item, "Name", name, "\n\t\t");
    sub_tail(tree, element, "MasterClip", &[("ObjectURef", master_uid)], "\n\t");
    element
}
