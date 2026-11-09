"""Synthesize the imported-SRT object graph.

An imported caption file is a panel clip item whose master's template clip
is a `TranscriptClip`: its `DataClip/Clip` core points at a
`DataMediaSource` (backed by `Media` + `DataStream`), and its
`TranscriptTextSegments` at a `CaptionCollection` holding one `Caption`
per cue, each referencing a `Block` with the styled-text payload
(29_captions). Payload synthesis is a template patch - see
`data/caption_template.py` and its generator.
"""

from __future__ import annotations

import base64
import os
import re
import struct
import uuid
import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING, NamedTuple

from ..data.caption_template import (
    COLLECTION_METADATA,
    PAYLOAD_HEAD,
    PAYLOAD_TAIL,
    SHIFT_DOWN,
    SHIFT_UP,
    STREAM_FORMAT,
    SYNTHETIC_STATE,
    TEMPLATE_PADDED,
    TEMPLATE_STYLE,
)
from ..xml.mutations import remove_child
from .media_import import _BMP_IMPLEMENTATION_ID, _MEDIA_CLASS_ID, _leaf, _top
from .time import TICKS_PER_SECOND

if TYPE_CHECKING:
    pass

_TRANSCRIPT_CLASS_ID = "9e0179bb-153c-4884-b34b-eb7082f34384"
_DATA_SOURCE_CLASS_ID = "ff36343e-4ece-4d37-ab61-e99b758f9d30"
_COLLECTION_CLASS_ID = "9b9f6927-2bdd-4874-bd23-1d8164bfbfcf"
_CAPTION_CLASS_ID = "d4ca6d5e-d3fe-4cfb-af7c-c9aea9e54b66"
_DATA_STREAM_CLASS_ID = "9e4e76eb-b72f-4b9b-9ea8-2887b1cc24fd"
_BLOCK_CLASS_ID = "d3782b80-516f-47e3-a7e8-e83779f0ed01"

#: Caption streams count at 30 fps with timecode format 104 (29_captions).
CAPTION_FRAME_RATE = 8467200000
CAPTION_TIMECODE_FORMAT = "104"

#: The factory Captions label (Mango), used when no local prefs exist.
CAPTIONS_LABEL_INDEX = 7
CAPTIONS_LABEL_COLOR = 277129

_TICKS_PER_MILLISECOND = TICKS_PER_SECOND // 1000

_CUE_TIMES = re.compile(
    r"(\d+):(\d\d):(\d\d)[,.](\d\d\d)\s*-->\s*(\d+):(\d\d):(\d\d)[,.](\d\d\d)"
)


class SrtCue(NamedTuple):
    """One parsed cue: exact ticks and the (possibly multi-line) text.

    `start` and `end` are raw TICK COUNTS, not `Time` objects - this is the
    builder layer, where every function takes ticks (`new_caption`,
    `new_data_stream`); `Time` is the model layer's currency.
    """

    #: Ticks from the start of the caption stream.
    start: int
    #: Ticks from the start of the caption stream.
    end: int
    text: str


def parse_srt(text: str) -> list[SrtCue]:
    """Parse SRT cues; index lines are optional, times become exact ticks."""
    cues = []
    normalized = text.replace("\r\n", "\n").replace("\r", "\n")
    for block in re.split(r"\n\s*\n", normalized.strip()):
        lines = block.split("\n")
        match = None
        while lines:
            match = _CUE_TIMES.search(lines[0])
            if match is not None:
                break
            lines.pop(0)  # the cue index, or leading junk
        if match is None:
            continue
        fields = [int(group) for group in match.groups()]
        start = (fields[0] * 3600 + fields[1] * 60 + fields[2]) * 1000 + fields[3]
        end = (fields[4] * 3600 + fields[5] * 60 + fields[6]) * 1000 + fields[7]
        body = "\n".join(lines[1:]).strip("\n")
        if not body:
            continue
        if end <= start:
            raise ValueError(f"cue ends before it starts: {lines[0].strip()!r}")
        cues.append(
            SrtCue(
                start * _TICKS_PER_MILLISECOND,
                end * _TICKS_PER_MILLISECOND,
                body,
            )
        )
    if not cues:
        raise ValueError("no cues found in the SRT file")
    return cues


def build_caption_payload(text: str) -> bytes:
    """The `FormattedTextData` FlatBuffer for `text`.

    A patch of the fixture template: splice the length-prefixed padded
    string and shift the recorded offset words by the padded-length delta
    (the signed vtable offsets shift the other way). The generator proved
    the recipe by rebuilding the fixture's own second payload exactly.
    """
    encoded = text.encode("utf-8")
    padded = (len(encoded) + 1 + 3) // 4 * 4
    delta = padded - TEMPLATE_PADDED
    head = bytearray(PAYLOAD_HEAD)
    for position in SHIFT_UP:
        (value,) = struct.unpack_from("<I", head, position)
        struct.pack_into("<I", head, position, (value + delta) & 0xFFFFFFFF)
    for position in SHIFT_DOWN:
        (value,) = struct.unpack_from("<I", head, position)
        struct.pack_into("<I", head, position, (value - delta) & 0xFFFFFFFF)
    body = struct.pack("<I", len(encoded)) + encoded
    body += b"\0" * (padded - len(encoded))
    return bytes(head) + body + PAYLOAD_TAIL


def new_block(payload_b64: str, binary_hash: str) -> ET.Element:
    element = _top("Block", _BLOCK_CLASS_ID, "1")
    data = ET.SubElement(
        element,
        "FormattedTextData",
        {"Encoding": "base64", "BinaryHash": binary_hash},
    )
    data.text = payload_b64 + "\n\t\t"
    data.tail = "\n\t"
    return element


def new_caption(block_id: str, start_ticks: int, end_ticks: int) -> ET.Element:
    element = _top("Caption", _CAPTION_CLASS_ID, "1")
    vector = ET.SubElement(element, "BlockVector", {"Version": "1"})
    vector.text = "\n\t\t\t"
    vector.tail = "\n\t\t"
    entry = ET.SubElement(
        vector, "BlockVectorItem", {"Index": "0", "ObjectRef": block_id}
    )
    entry.tail = "\n\t\t"
    _leaf(element, "TimeStart", str(start_ticks), "\n\t\t")
    _leaf(element, "TimeEnd", str(end_ticks), "\n\t")
    return element


def new_caption_collection(entries: list[tuple[int, str]]) -> ET.Element:
    """The cue map: `(end ticks, caption id)` pairs, keyed by cue END."""
    element = _top("CaptionCollection", _COLLECTION_CLASS_ID, "1")
    caption_map = ET.SubElement(element, "CaptionMap", {"Version": "1"})
    caption_map.text = "\n\t\t\t"
    caption_map.tail = "\n\t\t"
    for index, (end_ticks, caption_id) in enumerate(entries):
        item = ET.SubElement(
            caption_map, "CaptionMapItem", {"Version": "1", "Index": str(index)}
        )
        item.text = "\n\t\t\t\t"
        item.tail = "\n\t\t\t" if index < len(entries) - 1 else "\n\t\t"
        _leaf(item, "First", str(end_ticks), "\n\t\t\t\t")
        ET.SubElement(item, "Second", {"ObjectRef": caption_id}).tail = "\n\t\t\t"
    _leaf(element, "Metadata", COLLECTION_METADATA, "\n\t\t")
    _leaf(element, "StreamFormat", STREAM_FORMAT, "\n\t")
    return element


def new_data_media(
    media_uid: str,
    stream_id: str,
    file_path: str,
    project_dir: str,
    content_state: str,
) -> ET.Element:
    # The movie Media shape with a DataStream reference and none of the
    # audio/timecode extras (29_captions).
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, "DataStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    blob = base64.b64encode(content_state.encode("utf-16-le")).decode("ascii")
    state = ET.SubElement(
        element,
        "ModificationState",
        {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())},
    )
    state.text = blob + "\n\t\t"
    state.tail = "\n\t\t"
    _leaf(element, "RelativePath", os.path.relpath(file_path, project_dir), "\n\t\t")
    _leaf(element, "FilePath", file_path, "\n\t\t")
    _leaf(element, "ImplementationID", _BMP_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", os.path.basename(file_path), "\n\t\t")
    _leaf(element, "FileKey", str(uuid.uuid4()), "\n\t\t")
    _leaf(element, "ContentAndMetadataState", content_state, "\n\t\t")
    _leaf(element, "ActualMediaFilePath", file_path, "\n\t")
    return element


def new_data_stream(duration_ticks: int) -> ET.Element:
    element = _top("DataStream", _DATA_STREAM_CLASS_ID, "1")
    _leaf(element, "FrameRate", str(CAPTION_FRAME_RATE), "\n\t\t")
    _leaf(element, "Duration", str(duration_ticks), "\n\t")
    return element


def new_data_media_source(media_uid: str, duration_ticks: int) -> ET.Element:
    element = _top("DataMediaSource", _DATA_SOURCE_CLASS_ID, "1")
    source = ET.SubElement(element, "MediaSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Media", {"ObjectURef": media_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(duration_ticks), "\n\t")
    return element


#: Timeline caption classes and constants (29_captions' createCaptionTrack
#: output).
_CAPTION_TRACK_CLASS_ID = "b9d20db2-f229-482d-87fd-10f1fa157107"
_CAPTION_ITEM_CLASS_ID = "541ba122-fe61-4350-abe5-040ed395006e"
_DATA_CHAIN_CLASS_ID = "1d83b349-453e-4099-801d-6b23edae1724"
_SUBCLIP_CLASS_ID = "e0c58dc9-dbdd-4166-aef7-5db7e3f22e84"

#: The data (caption) media type GUID in a sequence's TrackGroups map.
DATA_MEDIA_TYPE = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5"

#: A synthetic caption's phantom source: the generator implementation (the
#: Black Video one) behind a fourcc token, spanning 12 hours.
_CAPTION_MEDIA_TOKEN = "1396920390"
_GENERATOR_IMPLEMENTATION_ID = "42008e7a-de6f-4270-96de-7e287abb9b4b"
SYNTHETIC_DURATION = 43200 * TICKS_PER_SECOND


def new_empty_logging_info() -> ET.Element:
    # A synthetic caption's master carries a logging info with no fields.
    element = _top("ClipLoggingInfo", "77ab7fdd-dcdf-465d-9906-7a330ca1e738", "10")
    element.text = "\n\t"
    return element


def new_synthetic_stream(timebase: int) -> ET.Element:
    element = _top("DataStream", _DATA_STREAM_CLASS_ID, "1")
    _leaf(element, "FrameRate", str(timebase), "\n\t\t")
    _leaf(element, "Duration", str(SYNTHETIC_DURATION), "\n\t")
    return element


def new_synthetic_media(
    media_uid: str, stream_id: str, state_stored: bool
) -> ET.Element:
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, "DataStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    state = ET.SubElement(
        element,
        "ModificationState",
        {"Encoding": "base64", "BinaryHash": SYNTHETIC_STATE[0]},
    )
    if not state_stored:
        # The blob is a constant every synthetic caption Media shares by
        # hash; only the first occurrence carries it.
        state.text = SYNTHETIC_STATE[1] + "\n\t\t"
    state.tail = "\n\t\t"
    _leaf(element, "FilePath", _CAPTION_MEDIA_TOKEN, "\n\t\t")
    _leaf(element, "ImplementationID", _GENERATOR_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", "SyntheticCaption", "\n\t\t")
    _leaf(element, "Infinite", "true", "\n\t\t")
    _leaf(element, "ActualMediaFilePath", _CAPTION_MEDIA_TOKEN, "\n\t")
    return element


def new_synthetic_template_clip(source_id: str, clip_id: str) -> ET.Element:
    # The synthetic master's own template clip: no labels, markers or
    # caption collection - just the source wiring.
    element = _top("TranscriptClip", _TRANSCRIPT_CLASS_ID, "2")
    data_clip = ET.SubElement(element, "DataClip", {"Version": "1"})
    data_clip.text = "\n\t\t\t"
    data_clip.tail = "\n\t"
    core = ET.SubElement(data_clip, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t\t"
    core.tail = "\n\t\t"
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t\t"
    _leaf(core, "ClipID", clip_id, "\n\t\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t\t")
    return element


def new_timeline_caption_clip(
    source_id: str,
    in_ticks: int,
    out_ticks: int,
    clip_id: str,
    label_index: int,
    label_color: int,
) -> ET.Element:
    # The placed instance: labelled, no MarkerOwner or InUse, and in/out
    # inside the synthetic source's 12-hour span.
    element = _top("TranscriptClip", _TRANSCRIPT_CLASS_ID, "2")
    data_clip = ET.SubElement(element, "DataClip", {"Version": "1"})
    data_clip.text = "\n\t\t\t"
    data_clip.tail = "\n\t"
    core = ET.SubElement(data_clip, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t\t"
    core.tail = "\n\t\t"
    node = ET.SubElement(core, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t\t"
    node.tail = "\n\t\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t\t"
    properties.tail = "\n\t\t\t\t"
    _leaf(properties, "asl.clip.label.color", str(label_color), "\n\t\t\t\t\t\t")
    _leaf(
        properties,
        "asl.clip.label.name",
        f"BE.Prefs.LabelColors.{label_index}",
        "\n\t\t\t\t\t",
    )
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t\t"
    _leaf(core, "OutPoint", str(out_ticks), "\n\t\t\t\t")
    _leaf(core, "InPoint", str(in_ticks), "\n\t\t\t\t")
    _leaf(core, "ClipID", clip_id, "\n\t\t\t")
    return element


def new_caption_subclip(clip_ref: str, master_uid: str) -> ET.Element:
    element = _top("SubClip", _SUBCLIP_CLASS_ID, "6")
    ET.SubElement(element, "Clip", {"ObjectRef": clip_ref}).tail = "\n\t\t"
    ET.SubElement(element, "MasterClip", {"ObjectURef": master_uid}).tail = "\n\t\t"
    _leaf(element, "Name", "SyntheticCaption", "\n\t\t")
    _leaf(element, "OrigChGrp", "0", "\n\t")
    return element


def new_data_chain() -> ET.Element:
    element = _top("DataComponentChain", _DATA_CHAIN_CLASS_ID, "1")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t"
    chain.tail = "\n\t"
    return element


def new_timeline_block(binary_hash: str) -> ET.Element:
    # The timeline copy hash-references the panel block's payload.
    element = _top("Block", _BLOCK_CLASS_ID, "1")
    data = ET.SubElement(
        element,
        "FormattedTextData",
        {"Encoding": "base64", "BinaryHash": binary_hash},
    )
    data.tail = "\n\t"
    return element


def new_caption_track_item(
    chain_id: str,
    subclip_id: str,
    block_id: str,
    start_ticks: int,
    end_ticks: int,
) -> ET.Element:
    element = _top("CaptionDataClipTrackItem", _CAPTION_ITEM_CLASS_ID, "3")
    data_item = ET.SubElement(element, "DataClipTrackItem", {"Version": "1"})
    data_item.text = "\n\t\t\t"
    data_item.tail = "\n\t\t"
    clip_item = ET.SubElement(data_item, "ClipTrackItem", {"Version": "8"})
    clip_item.text = "\n\t\t\t\t"
    clip_item.tail = "\n\t\t"
    owner = ET.SubElement(clip_item, "ComponentOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t\t"
    owner.tail = "\n\t\t\t\t"
    ET.SubElement(owner, "Components", {"ObjectRef": chain_id}).tail = "\n\t\t\t\t"
    track_item = ET.SubElement(clip_item, "TrackItem", {"Version": "4"})
    track_item.text = "\n\t\t\t\t\t"
    track_item.tail = "\n\t\t\t\t"
    if start_ticks:
        _leaf(track_item, "Start", str(start_ticks), "\n\t\t\t\t\t")
    _leaf(track_item, "End", str(end_ticks), "\n\t\t\t\t\t")
    # Fix the last child's tail to close TrackItem at its own depth.
    list(track_item)[-1].tail = "\n\t\t\t\t"
    ET.SubElement(clip_item, "SubClip", {"ObjectRef": subclip_id}).tail = "\n\t\t\t"
    vector = ET.SubElement(element, "BlockVector", {"Version": "1"})
    vector.text = "\n\t\t\t"
    vector.tail = "\n\t"
    ET.SubElement(
        vector, "BlockVectorItem", {"Index": "0", "ObjectRef": block_id}
    ).tail = "\n\t\t"
    return element


def new_caption_track(
    track_uid: str,
    track_id: int,
    item_ids: list[str],
    style_stored: bool,
    caption_format: int = 2,
) -> ET.Element:
    element = _top("CaptionDataClipTrack", _CAPTION_TRACK_CLASS_ID, "1", uid=track_uid)
    data_track = ET.SubElement(element, "DataClipTrack", {"Version": "1"})
    data_track.text = "\n\t\t\t"
    data_track.tail = "\n\t\t"
    clip_track = ET.SubElement(data_track, "ClipTrack", {"Version": "2"})
    clip_track.text = "\n\t\t\t\t"
    clip_track.tail = "\n\t\t"
    track = ET.SubElement(clip_track, "Track", {"Version": "4"})
    track.text = "\n\t\t\t\t\t"
    track.tail = "\n\t\t\t\t"
    _leaf(track, "ID", str(track_id), "\n\t\t\t\t\t")
    _leaf(track, "MediaType", DATA_MEDIA_TYPE, "\n\t\t\t\t\t")
    _leaf(track, "Index", "0", "\n\t\t\t\t")
    clip_items = ET.SubElement(clip_track, "ClipItems", {"Version": "3"})
    clip_items.text = "\n\t\t\t\t\t"
    clip_items.tail = "\n\t\t\t\t"
    track_items = ET.SubElement(clip_items, "TrackItems", {"Version": "1"})
    track_items.text = "\n\t\t\t\t\t\t"
    track_items.tail = "\n\t\t\t\t\t"
    for index, item_id in enumerate(item_ids):
        entry = ET.SubElement(
            track_items, "TrackItem", {"Index": str(index), "ObjectRef": item_id}
        )
        entry.tail = "\n\t\t\t\t\t\t" if index < len(item_ids) - 1 else "\n\t\t\t\t\t"
    _leaf(clip_items, "MediaType", DATA_MEDIA_TYPE, "\n\t\t\t\t\t")
    _leaf(clip_items, "Index", "0", "\n\t\t\t\t")
    transitions = ET.SubElement(clip_track, "TransitionItems", {"Version": "3"})
    transitions.text = "\n\t\t\t\t\t"
    transitions.tail = "\n\t\t\t"
    _leaf(transitions, "MediaType", DATA_MEDIA_TYPE, "\n\t\t\t\t\t")
    _leaf(transitions, "Index", "0", "\n\t\t\t\t")
    style = ET.SubElement(
        element,
        "CaptionDataTemplateStyle",
        {"Encoding": "base64", "BinaryHash": TEMPLATE_STYLE[0]},
    )
    if not style_stored:
        style.text = TEMPLATE_STYLE[1] + "\n\t\t"
    style.tail = "\n\t"
    # `Format` is the API constant's low word and `SubFormat` its high one;
    # Subtitle (0) elides both, which is exactly what Premiere's own
    # createCaptionTrack writes (sweep_caption_format.jsx).
    write_track_format(element, caption_format)
    return element


def write_track_format(track: ET.Element, caption_format: int) -> None:
    """Set (or clear) a caption track's `Format`/`SubFormat` pair."""
    for tag in ("SubFormat", "Format"):
        existing = track.find(tag)
        if existing is not None:
            remove_child(track, existing)
    list(track)[-1].tail = "\n\t"
    low = caption_format & 0xFFFF
    high = caption_format >> 16
    if low:
        list(track)[-1].tail = "\n\t\t"
        _leaf(track, "Format", str(low), "\n\t")
    if high:
        list(track)[-1].tail = "\n\t\t"
        _leaf(track, "SubFormat", str(high), "\n\t")


def new_transcript_clip(
    markers_id: str,
    source_id: str,
    collection_id: str,
    label_index: int,
    label_color: int,
    clip_id: str,
) -> ET.Element:
    # The caption item's template clip nests its core one level deeper than
    # a media clip's (TranscriptClip/DataClip/Clip) and adds the
    # TranscriptTextSegments reference beside the DataClip.
    element = _top("TranscriptClip", _TRANSCRIPT_CLASS_ID, "2")
    data_clip = ET.SubElement(element, "DataClip", {"Version": "1"})
    data_clip.text = "\n\t\t\t"
    data_clip.tail = "\n\t\t"
    core = ET.SubElement(data_clip, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t\t"
    core.tail = "\n\t\t"
    node = ET.SubElement(core, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t\t"
    node.tail = "\n\t\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t\t"
    properties.tail = "\n\t\t\t\t"
    _leaf(properties, "asl.clip.label.color", str(label_color), "\n\t\t\t\t\t\t")
    _leaf(
        properties,
        "asl.clip.label.name",
        f"BE.Prefs.LabelColors.{label_index}",
        "\n\t\t\t\t\t",
    )
    owner = ET.SubElement(core, "MarkerOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t\t"
    owner.tail = "\n\t\t\t\t"
    ET.SubElement(owner, "Markers", {"ObjectRef": markers_id}).tail = "\n\t\t\t\t\t"
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t\t"
    _leaf(core, "ClipID", clip_id, "\n\t\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t\t")
    ET.SubElement(
        element, "TranscriptTextSegments", {"ObjectRef": collection_id}
    ).tail = "\n\t"
    return element
