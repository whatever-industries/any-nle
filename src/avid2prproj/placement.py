"""Premiere field order for placed clips (shared by the writer and combined-clip builders)."""
from __future__ import annotations

import xml.etree.ElementTree as ET


def normalize_placement(item, multicam_rect: str | None = None) -> None:
    """Match Premiere's field order on a placed clip core (ClipID, [SelectedTrackIndex],
    InPoint, OutPoint, [IsMulticam]); a placed multicam video clip also carries the multicam
    flags and the track item a FrameRect (PRPROJ-0011 placements)."""
    core = item._clip_element.find("Clip")
    tail = core.find("ClipID").tail
    moved = [e for e in (core.find("InPoint"), core.find("OutPoint")) if e is not None]
    for e in moved:
        core.remove(e)
    if multicam_rect is not None:
        selected = ET.Element("SelectedTrackIndex")
        selected.text = "0"
        moved.insert(0, selected)
        flag = ET.Element("IsMulticam")
        flag.text = "true"
        moved.append(flag)
    at = list(core).index(core.find("ClipID")) + 1
    for offset, e in enumerate(moved):
        e.tail = tail
        core.insert(at + offset, e)
    if item._element.tag == "VideoClipTrackItem":
        order_video_item_fields(item)
    if multicam_rect is not None:
        track_item = item._element
        rect = ET.Element("FrameRect")
        rect.text = multicam_rect
        tone = track_item.find("ToneMapSettings")
        par = track_item.find("PixelAspectRatio")
        if par is not None:  # Premiere's order: ToneMapSettings, FrameRect, PixelAspectRatio
            track_item.remove(par)
        position = list(track_item).index(tone) + 1 if tone is not None else len(track_item)
        rect.tail = tone.tail if tone is not None else None
        track_item.insert(position, rect)
        if par is not None:
            track_item.insert(position + 1, par)
        order_video_item_fields(item)


_VIDEO_ITEM_ORDER = ("ToneMapSettings", "FrameRect", "PixelAspectRatio")


def order_video_item_fields(item) -> None:
    """Premiere writes ToneMapSettings, FrameRect, PixelAspectRatio in that order on a video
    track item (PRPROJ-0011 placements); py_premiere writes PixelAspectRatio first."""
    element = item._element
    present = [element.find(tag) for tag in _VIDEO_ITEM_ORDER]
    present = [e for e in present if e is not None]
    if len(present) < 2:
        return
    first = min(list(element).index(e) for e in present)
    tails = [e.tail for e in sorted(present, key=lambda e: list(element).index(e))]
    for e in present:
        element.remove(e)
    for offset, (e, tail) in enumerate(zip(present, tails)):
        e.tail = tail
        element.insert(first + offset, e)
