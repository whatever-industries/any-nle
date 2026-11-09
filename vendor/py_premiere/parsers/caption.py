"""Caption track parsing."""

from __future__ import annotations

from typing import TYPE_CHECKING

from ..models.caption import (
    Caption,
    CaptionTrack,
    decode_caption_text,
)
from ..models.time import Time

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..models.sequence import Sequence
    from ..xml import PremiereDocument


def _text_data(document: PremiereDocument, owner: ET.Element) -> ET.Element | None:
    # A caption owns its text through BlockVector -> Block -> FormattedTextData.
    item = owner.find("BlockVector/BlockVectorItem")
    if item is None:
        return None
    return document.resolve(item).find("FormattedTextData")


def _hash(document: PremiereDocument, owner: ET.Element) -> str | None:
    data = _text_data(document, owner)
    return None if data is None else data.get("BinaryHash")


def _caption_text(document: PremiereDocument, owner: ET.Element) -> str:
    # A timeline caption's own payload is empty - it shares the imported
    # stream's copy through the document's hash index.
    data = _text_data(document, owner)
    if data is None:
        return ""
    payload = document.payload(data)
    return "" if payload is None else decode_caption_text(payload)


def _source_times(document: PremiereDocument) -> dict[str, tuple[Time, Time]]:
    # The imported caption stream's own times, keyed by the same block hash
    # the timeline items use.
    times: dict[str, tuple[Time, Time]] = {}
    for caption in document.root.iter("Caption"):
        binary_hash = _hash(document, caption)
        if binary_hash is None or binary_hash in times:
            continue
        times[binary_hash] = (
            Time(int(caption.findtext("TimeStart") or 0)),
            Time(int(caption.findtext("TimeEnd") or 0)),
        )
    return times


def parse_caption_tracks(
    document: PremiereDocument, sequence: Sequence, group: ET.Element
) -> list[CaptionTrack]:
    """Build the caption tracks of a sequence's data track group.

    Every sequence has a data track group; one with no tracks (no captions
    imported) yields an empty list.
    """
    track_refs = group.findall("TrackGroup/Tracks/Track")
    if not track_refs:
        return []
    sources = _source_times(document)
    tracks = []
    for track_ref in track_refs:
        element = document.resolve(track_ref)
        track = CaptionTrack(element, sequence)
        items = element.findall(
            "DataClipTrack/ClipTrack/ClipItems/TrackItems/TrackItem"
        )
        for item_ref in items:
            item = document.resolve(item_ref)
            caption = Caption(item, _caption_text(document, item), track)
            source = sources.get(_hash(document, item) or "")
            if source is not None:
                caption._source_start, caption._source_end = source
            track._captions.append(caption)
        tracks.append(track)
    return tracks
