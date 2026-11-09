"""Synthesize a text graphic's object graph.

A Type-tool graphic (66_eg_text) is a timeline clip over INFINITE
synthetic media - the same generator implementation the caption track's
sources use - whose placement carries an `AE.ADBE Text` component. The
component's 22 parameters are Premiere's own defaults, and its styled
text is a re-texted copy of the fixture's `FormattedTextData` payload
(`data/text_template.py`). The master lives outside the project panel,
as Premiere's does.
"""

from __future__ import annotations

import base64
import uuid
import xml.etree.ElementTree as ET

from ..data.text_template import SOURCE_TEXT_PAYLOAD
from .caption import replace_payload_text
from .media_import import _leaf, _top

_MEDIA_CLASS_ID = "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff"
_VIDEO_STREAM_CLASS_ID = "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5"
_VIDEO_SOURCE_CLASS_ID = "e64ddf74-8fac-4682-8aa8-0e0ca2248949"
_MASTER_CLASS_ID = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf"
_LOGGING_CLASS_ID = "77ab7fdd-dcdf-465d-9906-7a330ca1e738"
_VIDEO_CLIP_CLASS_ID = "9308dbef-2440-4acb-9ab2-953b9a4e82ec"
_CHANNEL_GROUPS_CLASS_ID = "a3127a8c-95d4-456e-a7f5-171b3f922426"
_FILTER_CLASS_ID = "d10da199-beea-4dd1-b941-ed3a78766d50"
_VIDEO_CHAIN_CLASS_ID = "0970e08a-f58f-4108-b29a-1a717b8e12e2"
_ARB_PARAM_CLASS_ID = "313e54d4-6903-49ad-b0bf-8262cdd10f4e"
_BOOL_PARAM_CLASS_ID = "cc12343e-f113-4d3b-ae05-b287db77d461"
_POINT_PARAM_CLASS_ID = "ca81d347-309b-44d2-acc7-1c572efb973c"
_SCALAR_PARAM_CLASS_ID = "fe47129e-6c94-4fc0-95d5-c056a517aaf3"
_ANGLE_PARAM_CLASS_ID = "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542"

#: The generator behind a graphic: Premiere's synthetic-media
#: implementation, keyed by the numeric token it stores where a file path
#: would go, and running for the 12h synthetic span.
_GRAPHIC_IMPLEMENTATION_ID = "42008e7a-de6f-4270-96de-7e287abb9b4b"
_GRAPHIC_MEDIA_TOKEN = "1196574294"
_GRAPHIC_DURATION = 10973491200000000
_GRAPHIC_CODEC = "1431194446"
_GRAPHIC_COLOR_SPACE = (
    '{"baseColorProfile":{"colorProfileName":"BT.709 RGB Full"},"baseProfileType":1}'
)
#: Placements sit an hour into the generator, as caption sources do.
_GRAPHIC_BASE_TICKS = 914456685542400
#: The modification blob Premiere stamps on a fresh graphic's media.
_GRAPHIC_STATE = "vYBfenJFCVc6EBkkAAAAQA=="
#: The Text component's private data: a single zero byte.
_PRIVATE_DATA = "AA=="
_START = "-91445760000000000"


def _keyframe(value: str) -> str:
    return f"{_START},{value},0,0,0,0,0,0"


def _point_keyframe(value: str) -> str:
    return f"{_START},{value},0,0,0,0,0,0,5,4,0,0,0,0"


#: Substituted with the caller's position when the parameters are built.
_POSITION_PLACEHOLDER = "@position"


#: The Text component's parameters, in Premiere's own order: the tuples
#: are (class id, name, control type, keyframe value, lower, upper), with
#: `None` for the fields the fixture elides. Read off 66_eg_text.
_TEXT_PARAMS: list[tuple[str, str | None, str | None, str, str | None, str | None]] = [
    (_BOOL_PARAM_CLASS_ID, "Transform", "11", _keyframe("false"), None, "false"),
    (
        _POINT_PARAM_CLASS_ID,
        "Position",
        None,
        _point_keyframe(_POSITION_PLACEHOLDER),
        None,
        None,
    ),
    (_SCALAR_PARAM_CLASS_ID, "Scale", None, _keyframe("100."), "0", "4000"),
    (
        _SCALAR_PARAM_CLASS_ID,
        "Horizontal Scale",
        None,
        _keyframe("100."),
        "0",
        "4000",
    ),
    (_BOOL_PARAM_CLASS_ID, " ", None, _keyframe("true"), None, None),
    (
        _SCALAR_PARAM_CLASS_ID,
        "Rotation",
        "3",
        _keyframe("0."),
        "-32768",
        "32767",
    ),
    (_SCALAR_PARAM_CLASS_ID, "Opacity", None, _keyframe("100."), "0", "100"),
    (_POINT_PARAM_CLASS_ID, "Anchor Point", None, _point_keyframe("0:0"), None, None),
    (_BOOL_PARAM_CLASS_ID, None, "12", _keyframe("false"), None, "false"),
    (_ANGLE_PARAM_CLASS_ID, " ", None, _keyframe("0."), "0", "32768"),
    (_ANGLE_PARAM_CLASS_ID, " ", None, _keyframe("0."), "0", "32768"),
    (
        _ANGLE_PARAM_CLASS_ID,
        "start",
        None,
        _keyframe("-1."),
        "-100",
        "1000000000",
    ),
    (_ANGLE_PARAM_CLASS_ID, "end", None, _keyframe("-1."), "-100", "1000000000"),
    (_BOOL_PARAM_CLASS_ID, " ", None, _keyframe("false"), None, None),
    (_BOOL_PARAM_CLASS_ID, " ", None, _keyframe("false"), None, None),
    (_BOOL_PARAM_CLASS_ID, " ", None, _keyframe("false"), None, None),
    (_BOOL_PARAM_CLASS_ID, " ", None, _keyframe("false"), None, None),
    (_SCALAR_PARAM_CLASS_ID, "Parent Width", None, _keyframe("0."), "0", "20000"),
    (_SCALAR_PARAM_CLASS_ID, "Parent Height", None, _keyframe("0."), "0", "20000"),
    (
        _SCALAR_PARAM_CLASS_ID,
        "Parent Rotation",
        "3",
        _keyframe("0."),
        "-32768",
        "32767",
    ),
    (_BOOL_PARAM_CLASS_ID, " ", None, _keyframe("false"), None, None),
]


def build_source_text_param(text: str) -> ET.Element:
    """The `Source Text` parameter carrying `text`."""
    element = _top("ArbVideoComponentParam", _ARB_PARAM_CLASS_ID, "3")
    node = ET.SubElement(element, "Node", {"Version": "1"})
    node.text = "\n\t\t\t"
    node.tail = "\n\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t"
    properties.tail = "\n\t\t"
    _leaf(properties, "ECP.Graphics.Expanded", "true", "\n\t\t\t")
    _leaf(element, "Name", "Source Text", "\n\t\t")
    _leaf(element, "ParameterControlType", "9", "\n\t\t")
    _leaf(element, "ParameterID", "1", "\n\t\t")
    _leaf(element, "StartKeyframePosition", _START, "\n\t\t")
    payload = replace_payload_text(SOURCE_TEXT_PAYLOAD, text)
    value = ET.SubElement(
        element,
        "StartKeyframeValue",
        {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())},
    )
    value.text = base64.b64encode(payload).decode("ascii") + "\n\t\t"
    value.tail = "\n\t"
    return element


def build_text_params(position: tuple[float, float]) -> list[ET.Element]:
    """The Text component's 21 non-text parameters, in Premiere's order.

    `position` is the text's placement in normalized frame coordinates,
    written at full float precision the way Premiere stores the value the
    Type tool's click produced.
    """
    elements = []
    for index, (class_id, name, control, keyframe, lower, upper) in enumerate(
        _TEXT_PARAMS
    ):
        # Point values are written at full double precision, as Premiere
        # stores them (`0.44660192728042603`, not repr's shortest form).
        keyframe = keyframe.replace(
            _POSITION_PLACEHOLDER, f"{position[0]:.17g}:{position[1]:.17g}"
        )
        tag = (
            "PointComponentParam"
            if class_id == _POINT_PARAM_CLASS_ID
            else "VideoComponentParam"
        )
        version = "4" if class_id == _POINT_PARAM_CLASS_ID else "10"
        element = _top(tag, class_id, version)
        if name is not None:
            _leaf(element, "Name", name, "\n\t\t")
        if control is not None:
            _leaf(element, "ParameterControlType", control, "\n\t\t")
        _leaf(element, "ParameterID", str(index + 2), "\n\t\t")
        _leaf(element, "StartKeyframe", keyframe, "\n\t\t")
        if lower is not None:
            _leaf(element, "LowerBound", lower, "\n\t\t")
        if upper is not None:
            _leaf(element, "UpperBound", upper, "\n\t\t")
        last = list(element)[-1]
        last.tail = "\n\t"
        elements.append(element)
    return elements


def build_text_component(param_ids: list[str], text: str) -> ET.Element:
    """The `AE.ADBE Text` component wrapping the parameters."""
    element = _top("VideoFilterComponent", _FILTER_CLASS_ID, "9")
    core = ET.SubElement(element, "Component", {"Version": "7"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t\t"
    params = ET.SubElement(core, "Params", {"Version": "1"})
    params.text = "\n\t\t\t\t"
    params.tail = "\n\t\t\t"
    for index, param_id in enumerate(param_ids):
        entry = ET.SubElement(
            params, "Param", {"Index": str(index), "ObjectRef": param_id}
        )
        entry.tail = "\n\t\t\t\t" if index < len(param_ids) - 1 else "\n\t\t\t"
    _leaf(core, "ID", "4", "\n\t\t\t")
    _leaf(core, "DisplayName", "Text", "\n\t\t\t")
    # Premiere names the instance after the text it was created with.
    _leaf(core, "InstanceName", text, "\n\t\t")
    private = ET.SubElement(
        element,
        "PremiereFilterPrivateData",
        {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())},
    )
    private.text = _PRIVATE_DATA + "\n\t\t"
    private.tail = "\n\t\t"
    _leaf(element, "MatchName", "AE.ADBE Text", "\n\t\t")
    _leaf(element, "VideoFilterType", "2", "\n\t")
    return element


def build_graphic_chain(component_id: str) -> ET.Element:
    """The placement's component chain, holding the Text component.

    A graphic's chain keeps the video intrinsics defaulted but carries the
    Essential Graphics panel's active-component bag (66_eg_text).
    """
    element = _top("VideoComponentChain", _VIDEO_CHAIN_CLASS_ID, "3")
    _leaf(element, "DefaultMotion", "true", "\n\t\t")
    _leaf(element, "DefaultOpacity", "true", "\n\t\t")
    _leaf(element, "DefaultMotionComponentID", "1", "\n\t\t")
    _leaf(element, "DefaultOpacityComponentID", "2", "\n\t\t")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t\t"
    chain.tail = "\n\t"
    node = ET.SubElement(chain, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(properties, "MZ.ComponentChain.ActiveComponentID", "2", "\n\t\t\t\t\t")
    _leaf(
        properties,
        "MZ.ComponentChain.ActiveComponentParamIndex",
        "4294967295",
        "\n\t\t\t\t",
    )
    components = ET.SubElement(chain, "Components", {"Version": "1"})
    components.text = "\n\t\t\t\t"
    components.tail = "\n\t\t"
    ET.SubElement(
        components, "Component", {"Index": "0", "ObjectRef": component_id}
    ).tail = "\n\t\t\t"
    return element


def build_graphic_media(media_uid: str, stream_id: str, name: str) -> ET.Element:
    """The infinite synthetic media a graphic renders over."""
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, "VideoStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    state = ET.SubElement(
        element,
        "ModificationState",
        {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())},
    )
    state.text = _GRAPHIC_STATE + "\n\t\t"
    state.tail = "\n\t\t"
    _leaf(element, "FilePath", _GRAPHIC_MEDIA_TOKEN, "\n\t\t")
    _leaf(element, "ImplementationID", _GRAPHIC_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", name, "\n\t\t")
    _leaf(element, "Infinite", "true", "\n\t\t")
    _leaf(element, "ActualMediaFilePath", _GRAPHIC_MEDIA_TOKEN, "\n\t")
    return element


def build_graphic_stream(timebase: int, width: int, height: int) -> ET.Element:
    element = _top("VideoStream", _VIDEO_STREAM_CLASS_ID, "22")
    _leaf(element, "FrameRate", str(timebase), "\n\t\t")
    _leaf(element, "Duration", str(_GRAPHIC_DURATION), "\n\t\t")
    _leaf(element, "FrameRect", f"0,0,{width},{height}", "\n\t\t")
    _leaf(element, "CodecType", _GRAPHIC_CODEC, "\n\t\t")
    _leaf(element, "IsStill", "true", "\n\t\t")
    _leaf(element, "IsContinuousTime", "true", "\n\t\t")
    _leaf(element, "OriginalColorSpace", _GRAPHIC_COLOR_SPACE, "\n\t\t")
    _leaf(element, "AlphaType", "1", "\n\t\t")
    _leaf(element, "AlphaInfoIsUncertain", "true", "\n\t\t")
    _leaf(element, "FieldTypeIsUncertain", "true", "\n\t")
    return element


def build_graphic_source(media_uid: str) -> ET.Element:
    element = _top("VideoMediaSource", _VIDEO_SOURCE_CLASS_ID, "2")
    source = ET.SubElement(element, "MediaSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Media", {"ObjectURef": media_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(_GRAPHIC_DURATION), "\n\t")
    return element


def build_graphic_clip(
    source_id: str, in_ticks: int, out_ticks: int, in_use: bool
) -> ET.Element:
    """A graphic's clip object - the master's template or a placement."""
    element = _top("VideoClip", _VIDEO_CLIP_CLASS_ID, "11")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t"
    node = ET.SubElement(core, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(
        properties, "BE.Prefs.SyntheticMedia.DefaultIsDropFrame", "true", "\n\t\t\t\t"
    )
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t"
    _leaf(core, "OutPoint", str(out_ticks), "\n\t\t\t")
    _leaf(core, "InPoint", str(in_ticks), "\n\t\t\t")
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t" if not in_use else "\n\t\t\t")
    if in_use:
        _leaf(core, "InUse", "false", "\n\t\t")
    return element


def build_graphic_logging(name: str, timebase: int, timecode_format: int) -> ET.Element:
    element = _top("ClipLoggingInfo", _LOGGING_CLASS_ID, "10")
    _leaf(element, "CaptureMode", "2", "\n\t\t")
    _leaf(element, "ClipName", name, "\n\t\t")
    _leaf(element, "TimecodeFormat", str(timecode_format), "\n\t\t")
    _leaf(element, "MediaFrameRate", str(timebase), "\n\t")
    return element


def build_graphic_master(
    master_uid: str, logging_id: str, clip_id: str, groups_id: str, name: str
) -> ET.Element:
    element = _top("MasterClip", _MASTER_CLASS_ID, "12", uid=master_uid)
    ET.SubElement(element, "LoggingInfo", {"ObjectRef": logging_id}).tail = "\n\t\t"
    clips = ET.SubElement(element, "Clips", {"Version": "1"})
    clips.text = "\n\t\t\t"
    clips.tail = "\n\t\t"
    ET.SubElement(clips, "Clip", {"Index": "0", "ObjectRef": clip_id}).tail = "\n\t\t"
    ET.SubElement(
        element, "AudioClipChannelGroups", {"ObjectRef": groups_id}
    ).tail = "\n\t\t"
    _leaf(element, "Name", name, "\n\t\t")
    _leaf(element, "MasterClipChangeVersion", "0", "\n\t")
    return element


def build_empty_channel_groups() -> ET.Element:
    element = _top("ClipChannelGroupVectorSerializer", _CHANNEL_GROUPS_CLASS_ID, "1")
    element.text = "\n\t"
    return element
