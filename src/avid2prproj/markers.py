"""Clip labels, markers and the adjustment layers that carry markers on empty Avid filler.

Premiere keeps a clip's markers on its master clip: every timeline use shows the same ones
(py_premiere fixture 14_item_markers: placed clips point at the master's Markers collection).
Avid copies a master clip's markers into an edit at cut-in and lets the edit change them, so
an edit with markers of its own gets its own copy of the clip (writer.py). Decisions: User
2026-10-15; see docs/DECISIONS-EXPLAINED.md "Where markers go".
"""
from __future__ import annotations

import xml.etree.ElementTree as ET

from py_premiere import Time
from py_premiere.enums import ProjectItemType
from py_premiere.models.project_item import ProjectItem, _add_item_ref, _child_items

from . import colors
from .model import Marker, Options

LABEL_PREFIX = "BE.Prefs.LabelColors."


def set_label(project, item: ProjectItem, index: int) -> None:
    """Premiere label `index` on a bin item: the panel stamp (`Column.PropertyText.Label`) and
    the label its timeline clips copy from the master's template clips (`asl.clip.label.*`)."""
    item.color_label = index
    document = project._document
    master = item._master_element
    if master is None:
        return
    for reference in master.findall("Clips/Clip"):
        properties = document.resolve(reference).find("Clip/Node/Properties")
        if properties is None:
            continue
        for tag, text in (("asl.clip.label.color", str(colors.LABEL_PACKED[index])), ("asl.clip.label.name", f"{LABEL_PREFIX}{index}")):
            leaf = properties.find(tag)
            if leaf is not None:
                leaf.text = text


def comment_for(marker: Marker, options: Options, note: str = "") -> str:
    """Avid comment, plus one "Avid: …" line: user and color (option) and where the marker sat in
    Avid when Premiere places it elsewhere (e.g. "on empty A2")."""
    lines = [marker.comment] if marker.comment else []
    info = [v for v in (marker.user, marker.color) if v] if options.marker_avid_info else []
    if note:
        info.append(note)
    if info:
        lines.append("Avid: " + " · ".join(info))
    return "\n".join(lines)


def _color(marker: Marker, options: Options, report) -> int | None:
    index = colors.marker_color(marker.color, dict(options.marker_colors))
    if index is None and marker.color:
        report.notes.append(f"marker color {marker.color!r} is not an Avid palette name; Premiere's default color kept")
    return index


def _ensure_inner_list(item: ProjectItem) -> None:
    """An imported item's Markers collection starts without its inner `<Markers Version="1">`
    list (py_premiere media_import.new_markers_collection); add it first, where Premiere and
    py_premiere's own fresh collections keep it."""
    core = item._own_clip_core()
    reference = None if core is None else core.find("MarkerOwner/Markers")
    if reference is None:
        return
    collection = item.project._document.resolve(reference)
    if collection.find("Markers") is None:
        inner = ET.Element("Markers", {"Version": "1"})
        inner.tail = "\n\t\t"
        collection.insert(0, inner)


def add_clip_marker(item: ProjectItem, marker: Marker, start: int, options: Options, report) -> None:
    """A clip marker at `start` ticks of the item's media time."""
    _ensure_inner_list(item)
    m = item.add_marker(marker.name, Time(start), comment_for(marker, options), duration=Time(marker.duration) if marker.duration else None)
    index = _color(marker, options, report)
    if index is not None:
        m.color_index = index


def add_sequence_marker(sequence, marker: Marker, start: int, options: Options, report, note: str = "") -> None:
    m = sequence.add_marker(marker.name, Time(start), comments=comment_for(marker, options, note), marker_type="Comment",
                            duration=Time(marker.duration) if marker.duration else None)
    index = _color(marker, options, report)
    if index is not None:
        m.color_index = index


# One adjustment layer as Premiere 26.3 saved it (py_premiere fixture 22_adjustment_layer.prproj):
# a generator master clip over Premiere's synthetic Black Video media, flagged IsAdjustmentLayer.
# Spliced with fresh ids; frame size and rate follow the sequence.
_ADJUSTMENT_LAYER = """<Fragment>
<ClipProjectItem ObjectUID="item" ClassID="cb4e0ed7-aca1-4171-8525-e3658dec06dd" Version="1">
\t\t<ProjectItem Version="1">
\t\t\t<Node Version="1">
\t\t\t\t<Properties Version="1">
\t\t\t\t\t<Column.PropertyText.Label>BE.Prefs.LabelColors.3</Column.PropertyText.Label>
\t\t\t\t</Properties>
\t\t\t</Node>
\t\t\t<Name>{name}</Name>
\t\t</ProjectItem>
\t\t<MasterClip ObjectURef="master" />
\t</ClipProjectItem>
<MasterClip ObjectUID="master" ClassID="fb11c33a-b0a9-4465-aa94-b6d5db2628cf" Version="12">
\t\t<LoggingInfo ObjectRef="1" />
\t\t<Clips Version="1">
\t\t\t<Clip Index="0" ObjectRef="2" />
\t\t</Clips>
\t\t<AudioClipChannelGroups ObjectRef="6" />
\t\t<Name>{name}</Name>
\t\t<IsAdjustmentLayer>true</IsAdjustmentLayer>
\t\t<MasterClipChangeVersion>1</MasterClipChangeVersion>
\t</MasterClip>
<ClipLoggingInfo ObjectID="1" ClassID="77ab7fdd-dcdf-465d-9906-7a330ca1e738" Version="10">
\t\t<CaptureMode>2</CaptureMode>
\t\t<ClipName>{name}</ClipName>
\t\t<TimecodeFormat>{timecode_format}</TimecodeFormat>
\t\t<MediaFrameRate>{timebase}</MediaFrameRate>
\t</ClipLoggingInfo>
<VideoClip ObjectID="2" ClassID="9308dbef-2440-4acb-9ab2-953b9a4e82ec" Version="11">
\t\t<Clip Version="18">
\t\t\t<Node Version="1">
\t\t\t\t<Properties Version="1">
\t\t\t\t\t<asl.clip.label.color>8851829</asl.clip.label.color>
\t\t\t\t\t<asl.clip.label.name>BE.Prefs.LabelColors.3</asl.clip.label.name>
\t\t\t\t\t<BE.Prefs.SyntheticMedia.DefaultIsDropFrame>false</BE.Prefs.SyntheticMedia.DefaultIsDropFrame>
\t\t\t\t</Properties>
\t\t\t</Node>
\t\t\t<Source ObjectRef="3" />
\t\t\t<OutPoint>{out_point}</OutPoint>
\t\t\t<InPoint>0</InPoint>
\t\t\t<ClipID>{clip_id}</ClipID>
\t\t\t<InUse>false</InUse>
\t\t</Clip>
\t</VideoClip>
<VideoMediaSource ObjectID="3" ClassID="e64ddf74-8fac-4682-8aa8-0e0ca2248949" Version="2">
\t\t<MediaSource Version="4">
\t\t\t<Content Version="10">
\t\t\t</Content>
\t\t\t<Media ObjectURef="media" />
\t\t</MediaSource>
\t\t<OriginalDuration>10973491200000000</OriginalDuration>
\t</VideoMediaSource>
<Media ObjectUID="media" ClassID="7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff" Version="30">
\t\t<VideoStream ObjectRef="4" />
\t\t<ModificationState Encoding="base64" BinaryHash="78867dd0-8742-d0e7-56c0-26e30000001c">pKUCWDKm/0S4ybgfAAAAQA==
\t\t</ModificationState>
\t\t<FilePath>1112293707</FilePath>
\t\t<ImplementationID>42008e7a-de6f-4270-96de-7e287abb9b4b</ImplementationID>
\t\t<Title>Black Video</Title>
\t\t<Infinite>true</Infinite>
\t\t<ActualMediaFilePath>1112293707</ActualMediaFilePath>
\t</Media>
<VideoStream ObjectID="4" ClassID="a36e4719-3ec6-4a0c-ab11-8b4aab377aa5" Version="22">
\t\t<FrameRate>{timebase}</FrameRate>
\t\t<Duration>10973491200000000</Duration>
\t\t<FrameRect>0,0,{width},{height}</FrameRect>
\t\t<CodecType>1431194446</CodecType>
\t\t<IsStill>true</IsStill>
\t\t<IsContinuousTime>true</IsContinuousTime>
\t\t<OriginalColorSpace>{{"baseColorProfile":{{"colorProfileName":"BT.709 RGB Full"}},"baseProfileType":1}}</OriginalColorSpace>
\t\t<FieldTypeIsUncertain>true</FieldTypeIsUncertain>
\t</VideoStream>
<ClipChannelGroupVectorSerializer ObjectID="6" ClassID="a3127a8c-95d4-456e-a7f5-171b3f922426" Version="1">
\t</ClipChannelGroupVectorSerializer>
</Fragment>"""


def create_adjustment_layer(project, target_bin: ProjectItem, name: str, timebase: int, width: int, height: int,
                            timecode_format: int, length: int) -> ProjectItem:
    """An empty adjustment layer item in `target_bin` (it changes nothing in the picture), used
    to carry markers that sat on filler in an Avid video track. Its media time runs from 0, so
    markers and in points can use sequence time directly."""
    import uuid
    from xml.sax.saxutils import escape
    text = _ADJUSTMENT_LAYER.format(name=escape(name), timebase=timebase, width=width, height=height,
                                    timecode_format=timecode_format, out_point=max(length, timebase), clip_id=uuid.uuid4())
    spliced = project._splice_fragment(ET.fromstring(text))
    item_element, master = spliced["ClipProjectItem"], spliced["MasterClip"]
    document = project._document
    _add_item_ref(_child_items(target_bin._element.find("ProjectItemContainer")), item_element.get("ObjectUID"))
    item = ProjectItem(item_element, project, ProjectItemType.CLIP)
    item._parent = target_bin
    item._master_element = master
    item._clip_elements = [document.resolve(ref) for ref in master.findall("Clips/Clip")]
    item._default_out_ticks = max(length, timebase)
    target_bin._children.append(item)
    project._items_by_master_uid = None
    return item
