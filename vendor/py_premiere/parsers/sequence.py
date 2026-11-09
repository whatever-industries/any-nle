"""Parse sequences, tracks and track items."""

from __future__ import annotations

from typing import TYPE_CHECKING

from ..models import Sequence, Track, TrackItem, Transition
from .caption import parse_caption_tracks
from .component import parse_components, parse_selection_components
from .marker import parse_markers

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from ..models import Project
    from ..xml import PremiereDocument


def _parse_track_item(
    document: PremiereDocument, track: Track, element: ET.Element
) -> TrackItem | None:
    clip_track_item = element.find("ClipTrackItem")
    if clip_track_item is None:
        return None
    subclip_ref = clip_track_item.find("SubClip")
    if subclip_ref is None:
        return None
    subclip = document.resolve(subclip_ref)
    clip_ref = subclip.find("Clip")
    if clip_ref is None:
        return None
    clip_holder = document.resolve(clip_ref)
    item = TrackItem(element, subclip, clip_holder, track)
    item._components = parse_components(document, item, clip_track_item)
    item._selection_components = parse_selection_components(document, item, element)
    return item


def _parse_track(
    document: PremiereDocument,
    sequence: Sequence,
    media_type: str,
    element: ET.Element,
) -> Track:
    inner = element.find("ClipTrack/Track")
    track_id = int(inner.findtext("ID") or 0) if inner is not None else 0
    index = int(inner.findtext("Index") or 0) if inner is not None else 0
    track = Track(element, sequence, index, track_id, media_type)
    for item_ref in element.findall("ClipTrack/ClipItems/TrackItems/TrackItem"):
        item = _parse_track_item(document, track, document.resolve(item_ref))
        if item is not None:
            track._clips.append(item)
    for ref in element.findall("ClipTrack/TransitionItems/TrackItems/TrackItem"):
        track._transitions.append(Transition(document.resolve(ref), track))
    return track


def _parse_frame_rect(value: str | None) -> tuple[int, int] | None:
    # FrameRect is `left,top,width,height` (e.g. `0,0,1920,1080`).
    if not value:
        return None
    parts = value.split(",")
    if len(parts) != 4:
        return None
    return int(parts[2]), int(parts[3])


def parse_sequence(
    document: PremiereDocument, project: Project, element: ET.Element
) -> Sequence:
    sequence = Sequence(element, project)
    sequence._markers = parse_markers(document, element)
    for pair in element.findall("TrackGroups/TrackGroup"):
        second = pair.find("Second")
        if second is None:
            continue
        group = document.resolve(second)
        if group.tag == "VideoTrackGroup":
            media_type = "Video"
            target = sequence._video_tracks
            timebase = group.findtext("TrackGroup/FrameRate")
            if timebase:
                sequence._timebase = int(timebase)
            sequence._frame_size = _parse_frame_rect(group.findtext("FrameRect"))
        elif group.tag == "AudioTrackGroup":
            media_type = "Audio"
            target = sequence._audio_tracks
            audio_rate = group.findtext("TrackGroup/FrameRate")
            if audio_rate:
                sequence._audio_frame_rate = int(audio_rate)
            channels = group.findtext("NumAdaptiveChannels")
            if channels:
                sequence._audio_channel_count = int(channels)
        elif group.tag == "DataTrackGroup":
            sequence._caption_tracks = parse_caption_tracks(document, sequence, group)
            continue
        else:
            continue
        for track_ref in group.findall("TrackGroup/Tracks/Track"):
            track_element = document.resolve(track_ref)
            target.append(_parse_track(document, sequence, media_type, track_element))
    return sequence
