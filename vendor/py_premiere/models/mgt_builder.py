"""Read a Motion Graphics template and synthesize what importing it writes.

A `.mogrt` is a zip holding `definition.json` and the `project.aegraphic`
After Effects rendered. Importing one copies that graphic next to the
project under a `Motion Graphics Template Media` folder, imports it as an
ordinary AE-graphic clip, and hangs an `AE.ADBE Capsule` component - the
Essential Graphics controls - off both the master's blueprint chain and
the placement's chain.

Everything Premiere writes comes out of `definition.json`: its
`sourceInfoLocalized` block is the media description (frame rate,
duration, frame size, colour space, importer prefs) and its
`capsuleparams` block is the parameter set, already in `ParameterID`
order. Mapped against the marketplace fixture of TODO section 4.
"""

from __future__ import annotations

import base64
import json
import os
import uuid
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

from .media_import import TIMECODE_FORMATS, _leaf, _top
from .time import TICKS_PER_SECOND

_MEDIA_CLASS_ID = "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff"
_VIDEO_STREAM_CLASS_ID = "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5"
_VIDEO_SOURCE_CLASS_ID = "e64ddf74-8fac-4682-8aa8-0e0ca2248949"
_MASTER_CLASS_ID = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf"
_LOGGING_CLASS_ID = "77ab7fdd-dcdf-465d-9906-7a330ca1e738"
_VIDEO_CLIP_CLASS_ID = "9308dbef-2440-4acb-9ab2-953b9a4e82ec"
_FILTER_CLASS_ID = "d10da199-beea-4dd1-b941-ed3a78766d50"
_VIDEO_CHAIN_CLASS_ID = "0970e08a-f58f-4108-b29a-1a717b8e12e2"
_ARB_PARAM_CLASS_ID = "313e54d4-6903-49ad-b0bf-8262cdd10f4e"
_POINT_PARAM_CLASS_ID = "ca81d347-309b-44d2-acc7-1c572efb973c"
_COLOR_PARAM_CLASS_ID = "0fde4e9f-f895-4ba3-b0fe-9a6feafda583"
_BIN_CLASS_ID = "dbfd6653-24da-480e-a35e-ba45e9504e4b"
_ITEM_CLASS_ID = "cb4e0ed7-aca1-4171-8525-e3658dec06dd"

_DEFINITION_MEMBER = "definition.json"
_GRAPHIC_MEMBER = "project.aegraphic"

#: The folder Premiere copies the template's graphic into, and the panel
#: bin of the same name it files the imported item under.
MEDIA_FOLDER = "Motion Graphics Template Media"

#: The After Effects graphic importer, and the codec its streams report.
_AE_GRAPHIC_IMPLEMENTATION_ID = "ec341e53-60c2-4d89-abfc-bdb5c0ff2e0b"
_AE_GRAPHIC_CODEC = "1145854285"
_CAPSULE_MATCH_NAME = "AE.ADBE Capsule"
_START = "-91445760000000000"
_BIN_LABEL = "BE.Prefs.LabelColors.7"

#: `capPropType` values, as the fixture's 26 controls use them. The
#: `clientControls` block numbers the same kinds differently - the XML
#: follows `capsuleparams`.
_TYPE_TEXT = 0
_TYPE_COLOR = 3
_TYPE_POINT = 6
_TYPE_GROUP = 8

#: `ParameterControlType` per arbitrary-data control kind.
_GROUP_CONTROL_TYPE = "11"
_TEXT_CONTROL_TYPE = "23"

#: The capsule's `Component/ID` on the master's blueprint chain, where it
#: stands alone, and on a placement's, behind the motion and opacity
#: intrinsics at 1 and 2.
MASTER_SLOT = "1"
PLACEMENT_SLOT = "3"


def _compact(value: object) -> str:
    """JSON the way Premiere stores it: sorted keys, no spaces."""
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def _pack_color(rgba: list[float]) -> int:
    """Premiere's packing of a capsule colour control.

    Each channel takes a 16-bit lane in 8.8 fixed point, ordered A, R, G,
    B from the top - but the colour channels are scaled to 0-255 first
    while alpha stays in 0-1, so opaque white is `0x0100ff00ff00ff00`.
    """
    red, green, blue, alpha = rgba
    return (
        (round(alpha * 256) << 48)
        | (round(red * 255) << 40)
        | (round(green * 255) << 24)
        | (round(blue * 255) << 8)
    )


def _flatten(entry: dict) -> dict:
    """A control as the component's private data stores it.

    `definition.json` keeps a text control's font settings nested under
    `capPropFontEditInfo`; the private data instead carries the control's
    STORED VALUE - the per-run arrays and the text itself - directly on
    the control. Other control kinds pass through unchanged.
    """
    if entry.get("capPropFontEditInfo") is None:
        return entry
    flat = {key: value for key, value in entry.items() if key != "capPropFontEditInfo"}
    flat.update(_text_fields(entry))
    return flat


def _text_fields(entry: dict) -> dict:
    """A text control's stored value: its default under one text run.

    The font-edit scalars become one-element lists - the per-run arrays a
    multi-run edit would grow - alongside the run count and length.
    """
    text = str(entry["capPropDefault"])
    value: dict = {}
    for key, setting in entry["capPropFontEditInfo"].items():
        # The `capProp*` flags say which font fields the control exposes,
        # so they stay scalar; everything else is per-run.
        value[key] = setting if key.startswith("capProp") else [setting]
    value["capPropTextRunCount"] = 1
    value["fontTextRunLength"] = [len(text)]
    value["textEditValue"] = text
    return value


def _text_value(entry: dict) -> str:
    """The payload a text control's parameter carries."""
    if entry.get("capPropFontEditInfo") is None:
        return str(entry["capPropDefault"])
    return _compact(_text_fields(entry))


class Template:
    """A `.mogrt` archive: its graphic, and what importing it must write."""

    def __init__(self, path: Path) -> None:
        # A .mogrt is a zip; anything else is a caller pointing at the wrong
        # file, which should read as that rather than as a zip-library error.
        try:
            archive_file = zipfile.ZipFile(path)
        except zipfile.BadZipFile:
            raise ValueError(f"not a Motion Graphics template: {path}") from None
        # A missing member raises KeyError from `read`, inside the `with` below
        # - `ZipFile()` itself never raises it.
        with archive_file as archive:
            try:
                definition = json.loads(
                    archive.read(_DEFINITION_MEMBER).decode("utf-8-sig")
                )
                self.graphic = archive.read(_GRAPHIC_MEMBER)
            except (KeyError, ValueError) as error:
                raise ValueError(
                    f"not a Motion Graphics template ({error}): {path}"
                ) from None
        self.capsule_id: str = definition["capsuleID"]
        self.name: str = definition["capsuleName"]
        info = definition["sourceInfoLocalized"]["en_US"]
        self.params: list[dict] = info["capsuleparams"]["capParams"]
        self.has_audio: bool = info["hasaudio"]
        self.frame_rate: int = info["framerate"]["ticksperframe"]
        duration = info["duration"]
        self.duration_ticks = TICKS_PER_SECOND * duration["value"] // duration["scale"]
        size = info["framesize"]["size"]
        self.frame_size = (size["x"], size["y"])
        self.alpha_type: int = info["alphachanneltype"]
        self.field_type: int = info["nativefieldtype"]
        display = TIMECODE_FORMATS.get((self.frame_rate, info["isdropframe"]))
        if display is None:
            raise ValueError(
                f"no timecode format known for {self.frame_rate} ticks per frame"
            )
        self.time_display = display
        self.color_space = _compact(info["colorSpace"])
        # Premiere re-stamps two keys of the importer's own description as
        # it takes the graphic over: the flat artisan options empty out and
        # the AE library switches on.
        app_info = json.loads(info["appspecificsourceinfo"])
        app_info["flatArtisanOptions"] = []
        app_info["useAELib"] = True
        self.importer_prefs = info["id"] + _compact(app_info)
        self.private_data = _compact(
            {
                "capsuleparams": {"capParams": [_flatten(p) for p in self.params]},
                "framesize": info["framesize"],
            }
        )

    def media_path(self, project_directory: Path) -> Path:
        """Where importing this template copies its graphic."""
        return (
            project_directory
            / MEDIA_FOLDER
            / self.capsule_id
            / f"{self.name}.aegraphic"
        )


def _payload_element(
    tag: str, payload: bytes, tail: str, stored: dict[bytes, str]
) -> ET.Element:
    """A payload element, following Premiere's store-once rule.

    Two controls holding the same value (a template's `(don't edit)`
    mirrors, say) share one `BinaryHash`, and only the first carries the
    base64 text.
    """
    binary_hash = stored.get(payload)
    element = ET.Element(
        tag,
        {"Encoding": "base64", "BinaryHash": binary_hash or str(uuid.uuid4())},
    )
    if binary_hash is None:
        stored[payload] = element.get("BinaryHash") or ""
        element.text = base64.b64encode(payload).decode("ascii") + "\n\t\t"
    element.tail = tail
    return element


def build_capsule_params(
    template: Template, stored: dict[bytes, str]
) -> list[ET.Element]:
    """Every Essential Graphics control, in `ParameterID` order.

    `stored` carries the payloads already written to the document, so the
    placement's copy of the control set references the master's payloads
    instead of repeating them.
    """
    width, height = template.frame_size
    elements = []
    for index, entry in enumerate(template.params):
        kind = entry["capPropType"]
        name = entry["capPropUIName"]
        if kind in (_TYPE_GROUP, _TYPE_TEXT):
            element = _top("ArbVideoComponentParam", _ARB_PARAM_CLASS_ID, "3")
            _leaf(element, "Name", name, "\n\t\t")
            control = _GROUP_CONTROL_TYPE if kind == _TYPE_GROUP else _TEXT_CONTROL_TYPE
            _leaf(element, "ParameterControlType", control, "\n\t\t")
            _leaf(element, "ParameterID", str(index), "\n\t\t")
            _leaf(element, "StartKeyframePosition", _START, "\n\t\t")
            if kind == _TYPE_GROUP:
                # A group header stores the ids of the controls it holds.
                value = "".join(f"{child};" for child in entry["capPropDefault"])
            else:
                value = _text_value(entry)
            element.append(
                _payload_element(
                    "StartKeyframeValue", value.encode("utf-16-le"), "\n\t", stored
                )
            )
        elif kind == _TYPE_COLOR:
            element = _top("VideoComponentParam", _COLOR_PARAM_CLASS_ID, "10")
            _leaf(element, "Name", name, "\n\t\t")
            _leaf(element, "ParameterID", str(index), "\n\t\t")
            packed = _pack_color(entry["capPropDefault"])
            _leaf(element, "StartKeyframe", f"{_START},{packed},0,0,0,0,0,0", "\n\t")
        elif kind == _TYPE_POINT:
            element = _top("PointComponentParam", _POINT_PARAM_CLASS_ID, "4")
            _leaf(element, "Name", name, "\n\t\t")
            _leaf(element, "ParameterID", str(index), "\n\t\t")
            # Positions are stored normalized to the template's frame.
            point_x, point_y = entry["capPropDefault"]
            point = f"{point_x / width:.17g}:{point_y / height:.17g}"
            _leaf(
                element,
                "StartKeyframe",
                f"{_START},{point},0,0,0,0,0,0,5,4,0,0,0,0",
                "\n\t",
            )
        else:
            raise ValueError(f"unsupported Motion Graphics control type {kind}")
        elements.append(element)
    return elements


def build_capsule_component(
    template: Template, param_ids: list[str], stored: dict[bytes, str], slot: str
) -> ET.Element:
    """The `AE.ADBE Capsule` component wrapping the controls.

    `slot` is the component's id within its chain: the master's blueprint
    chain holds it alone at 1, a placement's chain numbers it after the
    motion and opacity intrinsics.
    """
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
    _leaf(core, "ID", slot, "\n\t\t\t")
    _leaf(core, "DisplayName", "Graphic Parameters", "\n\t\t\t")
    _leaf(core, "Intrinsic", "true", "\n\t\t")
    element.append(
        _payload_element(
            "PremiereFilterPrivateData",
            template.private_data.encode("utf-16-le"),
            "\n\t\t",
            stored,
        )
    )
    _leaf(element, "MatchName", _CAPSULE_MATCH_NAME, "\n\t\t")
    _leaf(element, "VideoFilterType", "2", "\n\t")
    return element


def build_blueprint_chain(component_id: str) -> ET.Element:
    """The master's chain, holding nothing but the capsule component."""
    element = _top("VideoComponentChain", _VIDEO_CHAIN_CLASS_ID, "3")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t\t"
    chain.tail = "\n\t"
    components = ET.SubElement(chain, "Components", {"Version": "1"})
    components.text = "\n\t\t\t\t"
    components.tail = "\n\t\t"
    ET.SubElement(
        components, "Component", {"Index": "0", "ObjectRef": component_id}
    ).tail = "\n\t\t\t"
    return element


def attach_component(chain: ET.Element, component_id: str) -> None:
    """Hang the capsule off a placement's otherwise default chain."""
    inner = chain.find("ComponentChain")
    if inner is None:
        raise ValueError("component chain has no ComponentChain")
    inner.text = "\n\t\t\t"
    components = ET.SubElement(inner, "Components", {"Version": "1"})
    components.text = "\n\t\t\t\t"
    components.tail = "\n\t\t"
    ET.SubElement(
        components, "Component", {"Index": "0", "ObjectRef": component_id}
    ).tail = "\n\t\t\t"


def build_template_item(item_uid: str, master_uid: str, name: str) -> ET.Element:
    """The panel entry for the imported template."""
    element = _top("ClipProjectItem", _ITEM_CLASS_ID, "1", uid=item_uid)
    project_item = ET.SubElement(element, "ProjectItem", {"Version": "1"})
    project_item.text = "\n\t\t\t"
    project_item.tail = "\n\t\t"
    _leaf(project_item, "Name", name, "\n\t\t")
    ET.SubElement(element, "MasterClip", {"ObjectURef": master_uid}).tail = "\n\t"
    return element


def build_mgt_stream(template: Template) -> ET.Element:
    element = _top("VideoStream", _VIDEO_STREAM_CLASS_ID, "22")
    _leaf(element, "FrameRate", str(template.frame_rate), "\n\t\t")
    _leaf(element, "Duration", str(template.duration_ticks), "\n\t\t")
    width, height = template.frame_size
    _leaf(element, "FrameRect", f"0,0,{width},{height}", "\n\t\t")
    _leaf(element, "CodecType", _AE_GRAPHIC_CODEC, "\n\t\t")
    _leaf(element, "OriginalColorSpace", template.color_space, "\n\t\t")
    _leaf(element, "AlphaType", str(template.alpha_type), "\n\t\t")
    _leaf(element, "OriginalFieldType", str(template.field_type), "\n\t")
    return element


def build_mgt_media(
    template: Template,
    media_uid: str,
    stream_id: str,
    file_path: Path,
    stored: dict[bytes, str],
    project_dir: Path,
) -> ET.Element:
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, "VideoStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    element.append(
        _payload_element(
            "ImporterPrefs",
            template.importer_prefs.encode("utf-16-le"),
            "\n\t\t",
            stored,
        )
    )
    # Premiere writes RelativePath ahead of TimeDisplay, and re-derives
    # it against the destination on every save - which is why the
    # caller hands the element to `_imported_media`.
    _leaf(
        element,
        "RelativePath",
        os.path.relpath(file_path, project_dir),
        "\n\t\t",
    )
    _leaf(element, "TimeDisplay", str(template.time_display), "\n\t\t")
    _leaf(element, "FilePath", str(file_path), "\n\t\t")
    _leaf(element, "ImplementationID", _AE_GRAPHIC_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", f"{template.name}/{file_path.name}", "\n\t\t")
    _leaf(element, "FileKey", str(uuid.uuid4()), "\n\t\t")
    _leaf(element, "ActualMediaFilePath", str(file_path), "\n\t")
    return element


def build_mgt_source(template: Template, media_uid: str) -> ET.Element:
    element = _top("VideoMediaSource", _VIDEO_SOURCE_CLASS_ID, "2")
    source = ET.SubElement(element, "MediaSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Media", {"ObjectURef": media_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(template.duration_ticks), "\n\t")
    return element


def build_mgt_clip(source_id: str, in_use: bool) -> ET.Element:
    """The master's template clip, or a placement's own copy of it."""
    element = _top("VideoClip", _VIDEO_CLIP_CLASS_ID, "11")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t"
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t"
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t\t" if in_use else "\n\t\t")
    if in_use:
        _leaf(core, "InUse", "false", "\n\t\t")
    return element


def build_mgt_logging(template: Template) -> ET.Element:
    element = _top("ClipLoggingInfo", _LOGGING_CLASS_ID, "10")
    _leaf(element, "CaptureMode", "2", "\n\t\t")
    _leaf(element, "ClipName", template.name, "\n\t\t")
    _leaf(element, "TimecodeFormat", str(template.time_display), "\n\t\t")
    _leaf(element, "MediaInPoint", "0", "\n\t\t")
    _leaf(element, "MediaOutPoint", str(template.duration_ticks), "\n\t\t")
    _leaf(element, "MediaFrameRate", str(template.frame_rate), "\n\t")
    return element


def build_mgt_master(
    template: Template,
    master_uid: str,
    logging_id: str,
    chain_id: str,
    clip_id: str,
    groups_id: str,
) -> ET.Element:
    element = _top("MasterClip", _MASTER_CLASS_ID, "12", uid=master_uid)
    ET.SubElement(element, "LoggingInfo", {"ObjectRef": logging_id}).tail = "\n\t\t"
    ET.SubElement(
        element, "BlueprintVideoComponentChain", {"ObjectRef": chain_id}
    ).tail = "\n\t\t"
    clips = ET.SubElement(element, "Clips", {"Version": "1"})
    clips.text = "\n\t\t\t"
    clips.tail = "\n\t\t"
    ET.SubElement(clips, "Clip", {"Index": "0", "ObjectRef": clip_id}).tail = "\n\t\t"
    ET.SubElement(
        element, "AudioClipChannelGroups", {"ObjectRef": groups_id}
    ).tail = "\n\t\t"
    _leaf(element, "Name", template.name, "\n\t\t")
    _leaf(element, "MasterClipChangeVersion", "0", "\n\t")
    return element


def build_media_bin() -> ET.Element:
    """The `Motion Graphics Template Media` bin, empty.

    The caller stamps the `ObjectUID`, as it does for every bin.
    """
    element = _top("BinProjectItem", _BIN_CLASS_ID, "1", uid="pending")
    project_item = ET.SubElement(element, "ProjectItem", {"Version": "1"})
    project_item.text = "\n\t\t\t"
    project_item.tail = "\n\t\t"
    node = ET.SubElement(project_item, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(properties, "Column.PropertyText.Label", _BIN_LABEL, "\n\t\t\t\t")
    _leaf(project_item, "Name", MEDIA_FOLDER, "\n\t\t")
    container = ET.SubElement(element, "ProjectItemContainer", {"Version": "1"})
    container.text = "\n\t\t"
    container.tail = "\n\t"
    return element
