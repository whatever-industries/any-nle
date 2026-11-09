//! Port of `models/graphic_builder.py` + `data/text_template.py`: a text graphic's object graph
//! (a timeline clip over infinite synthetic media whose placement carries an `AE.ADBE Text`
//! component). UUIDs (the styled-text and private-data hashes, ClipIDs) are drawn from the
//! supplied [`UuidSource`] in Python's call order.

use super::fmt::fmt_g;
use super::{leaf, top};
use crate::base64::{b64decode, b64encode};
use crate::caption::flat::replace_payload_text;
use crate::error::{value, Result};
use crate::ids::UuidSource;
use crate::xml::{NodeId, Tree};

const MEDIA_CLASS_ID: &str = "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff";
const VIDEO_STREAM_CLASS_ID: &str = "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5";
const VIDEO_SOURCE_CLASS_ID: &str = "e64ddf74-8fac-4682-8aa8-0e0ca2248949";
const MASTER_CLASS_ID: &str = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf";
const LOGGING_CLASS_ID: &str = "77ab7fdd-dcdf-465d-9906-7a330ca1e738";
const VIDEO_CLIP_CLASS_ID: &str = "9308dbef-2440-4acb-9ab2-953b9a4e82ec";
const CHANNEL_GROUPS_CLASS_ID: &str = "a3127a8c-95d4-456e-a7f5-171b3f922426";
const FILTER_CLASS_ID: &str = "d10da199-beea-4dd1-b941-ed3a78766d50";
const VIDEO_CHAIN_CLASS_ID: &str = "0970e08a-f58f-4108-b29a-1a717b8e12e2";
const ARB_PARAM_CLASS_ID: &str = "313e54d4-6903-49ad-b0bf-8262cdd10f4e";
const BOOL_PARAM_CLASS_ID: &str = "cc12343e-f113-4d3b-ae05-b287db77d461";
const POINT_PARAM_CLASS_ID: &str = "ca81d347-309b-44d2-acc7-1c572efb973c";
const SCALAR_PARAM_CLASS_ID: &str = "fe47129e-6c94-4fc0-95d5-c056a517aaf3";
const ANGLE_PARAM_CLASS_ID: &str = "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542";

/// The generator behind a graphic: Premiere's synthetic-media implementation.
const GRAPHIC_IMPLEMENTATION_ID: &str = "42008e7a-de6f-4270-96de-7e287abb9b4b";
const GRAPHIC_MEDIA_TOKEN: &str = "1196574294";
/// `_GRAPHIC_DURATION`: the 12h synthetic span.
pub const GRAPHIC_DURATION: i64 = 10973491200000000;
const GRAPHIC_CODEC: &str = "1431194446";
const GRAPHIC_COLOR_SPACE: &str =
    "{\"baseColorProfile\":{\"colorProfileName\":\"BT.709 RGB Full\"},\"baseProfileType\":1}";
/// `_GRAPHIC_BASE_TICKS`: placements sit an hour into the generator.
pub const GRAPHIC_BASE_TICKS: i64 = 914456685542400;
/// The modification blob Premiere stamps on a fresh graphic's media.
const GRAPHIC_STATE: &str = "vYBfenJFCVc6EBkkAAAAQA==";
/// The Text component's private data: a single zero byte.
const PRIVATE_DATA: &str = "AA==";
const START: &str = "-91445760000000000";

/// The `Source Text` payload of 66_eg_text (the text reads `py`), `data/text_template.py`.
const SOURCE_TEXT_PAYLOAD_B64: &str = concat!(
    "QAEAAAAAAABEMyIRDAAAAAAABgAKAAQABgAAAGQAAAAAAF4AGAAQAAwAAAAAAAAAAAAA",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAFgAAAAAAAAAAAAAA",
    "AAAAAAAAAAAAAAAAAAAAAAgAAAAAABcABwBeAAAAAAAAARAAAAAcAAAANAAAAAAAAQBU",
    "////WP///1z///9g////AQAAAAQAAAANAAAATHVjaWRhQ29uc29sZQAAAAEAAAAMAAAA",
    "CAAMAAQACAAIAAAACAAAAEQAAAACAAAAcHkAAAAANgAYAAAAFAAAAAAAAAAAABAAAAAA",
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAwAAAAIAAQANgAAAAIAAAAQAAAAEAAAAAAA",
    "gEBlZuY/9P////j////8////BAAEAAQAAAA=",
);

/// `SOURCE_TEXT_PAYLOAD`.
pub fn source_text_payload() -> Vec<u8> {
    b64decode(SOURCE_TEXT_PAYLOAD_B64).expect("embedded payload is valid base64")
}

fn keyframe(v: &str) -> String {
    format!("{START},{v},0,0,0,0,0,0")
}

fn point_keyframe(v: &str) -> String {
    format!("{START},{v},0,0,0,0,0,0,5,4,0,0,0,0")
}

const POSITION_PLACEHOLDER: &str = "@position";

/// One `_TEXT_PARAMS` row: (class id, name, control type, keyframe value, lower, upper).
struct TextParam {
    class_id: &'static str,
    name: Option<&'static str>,
    control: Option<&'static str>,
    keyframe: KeyKind,
    lower: Option<&'static str>,
    upper: Option<&'static str>,
}

/// How a row's `StartKeyframe` text is made.
enum KeyKind {
    Plain(&'static str),
    Point(&'static str),
}

const fn p(
    class_id: &'static str,
    name: Option<&'static str>,
    control: Option<&'static str>,
    keyframe: KeyKind,
    lower: Option<&'static str>,
    upper: Option<&'static str>,
) -> TextParam {
    TextParam { class_id, name, control, keyframe, lower, upper }
}

/// `_TEXT_PARAMS`: the Text component's parameters in Premiere's own order, read off 66_eg_text.
static TEXT_PARAMS: [TextParam; 21] = [
    p(BOOL_PARAM_CLASS_ID, Some("Transform"), Some("11"), KeyKind::Plain("false"), None, Some("false")),
    p(POINT_PARAM_CLASS_ID, Some("Position"), None, KeyKind::Point(POSITION_PLACEHOLDER), None, None),
    p(SCALAR_PARAM_CLASS_ID, Some("Scale"), None, KeyKind::Plain("100."), Some("0"), Some("4000")),
    p(SCALAR_PARAM_CLASS_ID, Some("Horizontal Scale"), None, KeyKind::Plain("100."), Some("0"), Some("4000")),
    p(BOOL_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("true"), None, None),
    p(SCALAR_PARAM_CLASS_ID, Some("Rotation"), Some("3"), KeyKind::Plain("0."), Some("-32768"), Some("32767")),
    p(SCALAR_PARAM_CLASS_ID, Some("Opacity"), None, KeyKind::Plain("100."), Some("0"), Some("100")),
    p(POINT_PARAM_CLASS_ID, Some("Anchor Point"), None, KeyKind::Point("0:0"), None, None),
    p(BOOL_PARAM_CLASS_ID, None, Some("12"), KeyKind::Plain("false"), None, Some("false")),
    p(ANGLE_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("0."), Some("0"), Some("32768")),
    p(ANGLE_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("0."), Some("0"), Some("32768")),
    p(ANGLE_PARAM_CLASS_ID, Some("start"), None, KeyKind::Plain("-1."), Some("-100"), Some("1000000000")),
    p(ANGLE_PARAM_CLASS_ID, Some("end"), None, KeyKind::Plain("-1."), Some("-100"), Some("1000000000")),
    p(BOOL_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("false"), None, None),
    p(BOOL_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("false"), None, None),
    p(BOOL_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("false"), None, None),
    p(BOOL_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("false"), None, None),
    p(SCALAR_PARAM_CLASS_ID, Some("Parent Width"), None, KeyKind::Plain("0."), Some("0"), Some("20000")),
    p(SCALAR_PARAM_CLASS_ID, Some("Parent Height"), None, KeyKind::Plain("0."), Some("0"), Some("20000")),
    p(SCALAR_PARAM_CLASS_ID, Some("Parent Rotation"), Some("3"), KeyKind::Plain("0."), Some("-32768"), Some("32767")),
    p(BOOL_PARAM_CLASS_ID, Some(" "), None, KeyKind::Plain("false"), None, None),
];

/// `build_source_text_param`: the `Source Text` parameter carrying `text` (draws the payload's
/// `BinaryHash`).
pub fn build_source_text_param(tree: &mut Tree, text: &str, uuids: &mut dyn UuidSource) -> Result<NodeId> {
    let payload = replace_payload_text(&source_text_payload(), text)?;
    let element = top(tree, "ArbVideoComponentParam", ARB_PARAM_CLASS_ID, "3", None);
    let node = tree.sub_element(element, "Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t"));
    tree.set_tail(node, Some("\n\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t"));
    leaf(tree, properties, "ECP.Graphics.Expanded", "true", "\n\t\t\t");
    leaf(tree, element, "Name", "Source Text", "\n\t\t");
    leaf(tree, element, "ParameterControlType", "9", "\n\t\t");
    leaf(tree, element, "ParameterID", "1", "\n\t\t");
    leaf(tree, element, "StartKeyframePosition", START, "\n\t\t");
    let hash = uuids.uuid4().to_string();
    let value = tree.sub_element(element, "StartKeyframeValue", &[("Encoding", "base64"), ("BinaryHash", &hash)]);
    tree.set_text(value, Some(format!("{}\n\t\t", b64encode(&payload))));
    tree.set_tail(value, Some("\n\t"));
    Ok(element)
}

/// `build_text_params`: the Text component's 21 non-text parameters, in Premiere's order.
/// `position` is the text's placement in normalized frame coordinates, written at full float
/// precision (`%.17g`).
pub fn build_text_params(tree: &mut Tree, position: (f64, f64)) -> Vec<NodeId> {
    let mut elements = Vec::new();
    for (index, row) in TEXT_PARAMS.iter().enumerate() {
        let keyframe_text = match &row.keyframe {
            KeyKind::Plain(v) => keyframe(v),
            KeyKind::Point(v) => point_keyframe(v),
        };
        let keyframe_text = keyframe_text
            .replace(POSITION_PLACEHOLDER, &format!("{}:{}", fmt_g(position.0, 17), fmt_g(position.1, 17)));
        let is_point = row.class_id == POINT_PARAM_CLASS_ID;
        let element = top(
            tree,
            if is_point { "PointComponentParam" } else { "VideoComponentParam" },
            row.class_id,
            if is_point { "4" } else { "10" },
            None,
        );
        if let Some(name) = row.name {
            leaf(tree, element, "Name", name, "\n\t\t");
        }
        if let Some(control) = row.control {
            leaf(tree, element, "ParameterControlType", control, "\n\t\t");
        }
        leaf(tree, element, "ParameterID", &(index + 2).to_string(), "\n\t\t");
        leaf(tree, element, "StartKeyframe", &keyframe_text, "\n\t\t");
        if let Some(lower) = row.lower {
            leaf(tree, element, "LowerBound", lower, "\n\t\t");
        }
        if let Some(upper) = row.upper {
            leaf(tree, element, "UpperBound", upper, "\n\t\t");
        }
        let last = *tree.children(element).last().expect("a param always has ParameterID");
        tree.set_tail(last, Some("\n\t"));
        elements.push(element);
    }
    elements
}

/// `build_text_component`: the `AE.ADBE Text` component wrapping the parameters (draws the
/// private-data `BinaryHash`).
pub fn build_text_component(tree: &mut Tree, param_ids: &[String], text: &str, uuids: &mut dyn UuidSource) -> NodeId {
    let element = top(tree, "VideoFilterComponent", FILTER_CLASS_ID, "9", None);
    let core = tree.sub_element(element, "Component", &[("Version", "7")]);
    tree.set_text(core, Some("\n\t\t\t"));
    tree.set_tail(core, Some("\n\t\t"));
    let params = tree.sub_element(core, "Params", &[("Version", "1")]);
    tree.set_text(params, Some("\n\t\t\t\t"));
    tree.set_tail(params, Some("\n\t\t\t"));
    for (index, param_id) in param_ids.iter().enumerate() {
        let i = index.to_string();
        let entry = tree.sub_element(params, "Param", &[("Index", &i), ("ObjectRef", param_id)]);
        tree.set_tail(entry, Some(if index + 1 < param_ids.len() { "\n\t\t\t\t" } else { "\n\t\t\t" }));
    }
    leaf(tree, core, "ID", "4", "\n\t\t\t");
    leaf(tree, core, "DisplayName", "Text", "\n\t\t\t");
    // Premiere names the instance after the text it was created with.
    leaf(tree, core, "InstanceName", text, "\n\t\t");
    let hash = uuids.uuid4().to_string();
    let private = tree.sub_element(element, "PremiereFilterPrivateData", &[("Encoding", "base64"), ("BinaryHash", &hash)]);
    tree.set_text(private, Some(format!("{PRIVATE_DATA}\n\t\t")));
    tree.set_tail(private, Some("\n\t\t"));
    leaf(tree, element, "MatchName", "AE.ADBE Text", "\n\t\t");
    leaf(tree, element, "VideoFilterType", "2", "\n\t");
    element
}

/// `build_graphic_chain`: the placement's component chain, holding the Text component and the
/// Essential Graphics panel's active-component bag.
pub fn build_graphic_chain(tree: &mut Tree, component_id: &str) -> NodeId {
    let element = top(tree, "VideoComponentChain", VIDEO_CHAIN_CLASS_ID, "3", None);
    leaf(tree, element, "DefaultMotion", "true", "\n\t\t");
    leaf(tree, element, "DefaultOpacity", "true", "\n\t\t");
    leaf(tree, element, "DefaultMotionComponentID", "1", "\n\t\t");
    leaf(tree, element, "DefaultOpacityComponentID", "2", "\n\t\t");
    let chain = tree.sub_element(element, "ComponentChain", &[("Version", "3")]);
    tree.set_text(chain, Some("\n\t\t\t"));
    tree.set_tail(chain, Some("\n\t"));
    let node = tree.sub_element(chain, "Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t\t"));
    tree.set_tail(node, Some("\n\t\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t\t"));
    leaf(tree, properties, "MZ.ComponentChain.ActiveComponentID", "2", "\n\t\t\t\t\t");
    leaf(tree, properties, "MZ.ComponentChain.ActiveComponentParamIndex", "4294967295", "\n\t\t\t\t");
    let components = tree.sub_element(chain, "Components", &[("Version", "1")]);
    tree.set_text(components, Some("\n\t\t\t\t"));
    tree.set_tail(components, Some("\n\t\t"));
    let entry = tree.sub_element(components, "Component", &[("Index", "0"), ("ObjectRef", component_id)]);
    tree.set_tail(entry, Some("\n\t\t\t"));
    element
}

/// `build_graphic_media`: the infinite synthetic media a graphic renders over (draws the
/// modification-state `BinaryHash`).
pub fn build_graphic_media(
    tree: &mut Tree,
    media_uid: &str,
    stream_id: &str,
    name: &str,
    uuids: &mut dyn UuidSource,
) -> NodeId {
    let element = top(tree, "Media", MEDIA_CLASS_ID, "30", Some(media_uid));
    let stream = tree.sub_element(element, "VideoStream", &[("ObjectRef", stream_id)]);
    tree.set_tail(stream, Some("\n\t\t"));
    let hash = uuids.uuid4().to_string();
    let state = tree.sub_element(element, "ModificationState", &[("Encoding", "base64"), ("BinaryHash", &hash)]);
    tree.set_text(state, Some(format!("{GRAPHIC_STATE}\n\t\t")));
    tree.set_tail(state, Some("\n\t\t"));
    leaf(tree, element, "FilePath", GRAPHIC_MEDIA_TOKEN, "\n\t\t");
    leaf(tree, element, "ImplementationID", GRAPHIC_IMPLEMENTATION_ID, "\n\t\t");
    leaf(tree, element, "Title", name, "\n\t\t");
    leaf(tree, element, "Infinite", "true", "\n\t\t");
    leaf(tree, element, "ActualMediaFilePath", GRAPHIC_MEDIA_TOKEN, "\n\t");
    element
}

/// `build_graphic_stream`.
pub fn build_graphic_stream(tree: &mut Tree, timebase: i64, width: i64, height: i64) -> NodeId {
    let element = top(tree, "VideoStream", VIDEO_STREAM_CLASS_ID, "22", None);
    leaf(tree, element, "FrameRate", &timebase.to_string(), "\n\t\t");
    leaf(tree, element, "Duration", &GRAPHIC_DURATION.to_string(), "\n\t\t");
    leaf(tree, element, "FrameRect", &format!("0,0,{width},{height}"), "\n\t\t");
    leaf(tree, element, "CodecType", GRAPHIC_CODEC, "\n\t\t");
    leaf(tree, element, "IsStill", "true", "\n\t\t");
    leaf(tree, element, "IsContinuousTime", "true", "\n\t\t");
    leaf(tree, element, "OriginalColorSpace", GRAPHIC_COLOR_SPACE, "\n\t\t");
    leaf(tree, element, "AlphaType", "1", "\n\t\t");
    leaf(tree, element, "AlphaInfoIsUncertain", "true", "\n\t\t");
    leaf(tree, element, "FieldTypeIsUncertain", "true", "\n\t");
    element
}

/// `build_graphic_source`.
pub fn build_graphic_source(tree: &mut Tree, media_uid: &str) -> NodeId {
    let element = top(tree, "VideoMediaSource", VIDEO_SOURCE_CLASS_ID, "2", None);
    let source = tree.sub_element(element, "MediaSource", &[("Version", "4")]);
    tree.set_text(source, Some("\n\t\t\t"));
    tree.set_tail(source, Some("\n\t\t"));
    let content = tree.sub_element(source, "Content", &[("Version", "10")]);
    tree.set_text(content, Some("\n\t\t\t"));
    tree.set_tail(content, Some("\n\t\t\t"));
    let media = tree.sub_element(source, "Media", &[("ObjectURef", media_uid)]);
    tree.set_tail(media, Some("\n\t\t"));
    leaf(tree, element, "OriginalDuration", &GRAPHIC_DURATION.to_string(), "\n\t");
    element
}

/// `build_graphic_clip`: a graphic's clip object, the master's template or a placement
/// (draws the ClipID).
pub fn build_graphic_clip(
    tree: &mut Tree,
    source_id: &str,
    in_ticks: i64,
    out_ticks: i64,
    in_use: bool,
    uuids: &mut dyn UuidSource,
) -> NodeId {
    let element = top(tree, "VideoClip", VIDEO_CLIP_CLASS_ID, "11", None);
    let core = tree.sub_element(element, "Clip", &[("Version", "18")]);
    tree.set_text(core, Some("\n\t\t\t"));
    tree.set_tail(core, Some("\n\t"));
    let node = tree.sub_element(core, "Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t\t"));
    tree.set_tail(node, Some("\n\t\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t\t"));
    leaf(tree, properties, "BE.Prefs.SyntheticMedia.DefaultIsDropFrame", "true", "\n\t\t\t\t");
    let source = tree.sub_element(core, "Source", &[("ObjectRef", source_id)]);
    tree.set_tail(source, Some("\n\t\t\t"));
    leaf(tree, core, "OutPoint", &out_ticks.to_string(), "\n\t\t\t");
    leaf(tree, core, "InPoint", &in_ticks.to_string(), "\n\t\t\t");
    leaf(tree, core, "ClipID", &uuids.uuid4().to_string(), if in_use { "\n\t\t\t" } else { "\n\t\t" });
    if in_use {
        leaf(tree, core, "InUse", "false", "\n\t\t");
    }
    element
}

/// `build_graphic_logging`.
pub fn build_graphic_logging(tree: &mut Tree, name: &str, timebase: i64, timecode_format: i64) -> NodeId {
    let element = top(tree, "ClipLoggingInfo", LOGGING_CLASS_ID, "10", None);
    leaf(tree, element, "CaptureMode", "2", "\n\t\t");
    leaf(tree, element, "ClipName", name, "\n\t\t");
    leaf(tree, element, "TimecodeFormat", &timecode_format.to_string(), "\n\t\t");
    leaf(tree, element, "MediaFrameRate", &timebase.to_string(), "\n\t");
    element
}

/// `build_graphic_master`.
pub fn build_graphic_master(
    tree: &mut Tree,
    master_uid: &str,
    logging_id: &str,
    clip_id: &str,
    groups_id: &str,
    name: &str,
) -> NodeId {
    let element = top(tree, "MasterClip", MASTER_CLASS_ID, "12", Some(master_uid));
    let logging = tree.sub_element(element, "LoggingInfo", &[("ObjectRef", logging_id)]);
    tree.set_tail(logging, Some("\n\t\t"));
    let clips = tree.sub_element(element, "Clips", &[("Version", "1")]);
    tree.set_text(clips, Some("\n\t\t\t"));
    tree.set_tail(clips, Some("\n\t\t"));
    let clip = tree.sub_element(clips, "Clip", &[("Index", "0"), ("ObjectRef", clip_id)]);
    tree.set_tail(clip, Some("\n\t\t"));
    let groups = tree.sub_element(element, "AudioClipChannelGroups", &[("ObjectRef", groups_id)]);
    tree.set_tail(groups, Some("\n\t\t"));
    leaf(tree, element, "Name", name, "\n\t\t");
    leaf(tree, element, "MasterClipChangeVersion", "0", "\n\t");
    element
}

/// `build_empty_channel_groups`.
pub fn build_empty_channel_groups(tree: &mut Tree) -> NodeId {
    let element = top(tree, "ClipChannelGroupVectorSerializer", CHANNEL_GROUPS_CLASS_ID, "1", None);
    tree.set_text(element, Some("\n\t"));
    element
}

/// `validate_vector2` for a point given as floats: both components must be finite.
pub fn validate_vector2(position: (f64, f64)) -> Result<()> {
    for component in [position.0, position.1] {
        if !component.is_finite() {
            return value(format!("must be finite numbers, got {}", super::fmt::py_repr(component)));
        }
    }
    Ok(())
}
