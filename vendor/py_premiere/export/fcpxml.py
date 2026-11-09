"""FCPXML export.

FCPXML is the cheapest interchange format to reach - plain XML against a
published DTD, where AAF is an OLE compound file and OMF a binary
interchange.

**Premiere cannot read what this writes.** It was picked first on the
belief that it round-tripped, and it does not: Premiere reads Final Cut
Pro **7** XML (`<xmeml>`, `.xml`), while this emits Final Cut Pro **X**'s
FCPXML (`<fcpxml>`), which Resolve and FCP X read. Handed a `.fcpxml`,
Premiere 26.3 answers "File format not supported" - a hand-written
minimal document included, so it is the format being refused and not
anything written here. Treat this as a one-way export to Resolve/FCP X.

Times are rationals in seconds (`N/Ds`). Premiere's tick is 1/254016000000
of a second, so every tick count converts exactly with no rounding; the
fractions are reduced only for readability.

Structure written per sequence:

    resources   one `format` for the sequence, one `asset` per media file
    spine       the first video track, with `gap` elements over the holes
    lanes       every other track's clips, as connected clips on the spine

A connected clip hangs off the spine element covering its start, with its
offset expressed in that parent's timeline, which is what the format
requires - lanes count up for video and down for audio.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from fractions import Fraction
from pathlib import Path
from typing import TYPE_CHECKING

from ..models.time import TICKS_PER_SECOND

if TYPE_CHECKING:
    from ..models import Project, Sequence, TrackItem

#: The DTD version the output declares. Chosen for Resolve; Premiere reads
#: no version of this format (see the module docstring).
FCPXML_VERSION = "1.9"

#: `MZ.Sequence.VideoTimeDisplayFormat` values that mean drop-frame timecode.
#: Only 29.97 is attested: `media_import.TIMECODE_FORMATS` pins 102 as the
#: drop-frame form of that rate against 103 for its non-drop twin. Other rates
#: with a drop-frame display fall through to NDF until one is measured.
_DROP_FRAME_DISPLAY_FORMATS = frozenset({102})


def _indent(element: ET.Element, depth: int) -> None:
    # `ET.indent` is 3.9+; this project supports 3.7.
    pad = "\n" + "    " * depth
    children = list(element)
    if children:
        if not (element.text or "").strip():
            element.text = pad + "    "
        for child in children:
            _indent(child, depth + 1)
            if not (child.tail or "").strip():
                child.tail = pad + "    "
        if not (children[-1].tail or "").strip():
            children[-1].tail = pad
    if depth and not (element.tail or "").strip():
        element.tail = pad


def _seconds(ticks: int) -> str:
    """A tick count as an FCPXML rational-seconds string."""
    if ticks == 0:
        return "0s"
    value = Fraction(ticks, TICKS_PER_SECOND)
    if value.denominator == 1:
        return f"{value.numerator}s"
    return f"{value.numerator}/{value.denominator}s"


def _file_url(path: Path) -> str:
    return path.absolute().as_uri()


class _Asset:
    """One emitted `<asset>`: its id, its element and the span it must cover."""

    def __init__(self, identifier: str, element: ET.Element, span: int) -> None:
        self.identifier = identifier
        self.element = element
        self.span = span

    def extend(self, ticks: int) -> None:
        """Grow the asset to cover a clip that reads further into it."""
        if ticks <= self.span:
            return
        self.span = ticks
        self.element.set("duration", _seconds(ticks))

    def declare(self, has_video: bool) -> None:
        """Record that a clip uses this asset's video or audio stream."""
        self.element.set("hasVideo" if has_video else "hasAudio", "1")


class _Resources:
    """Assigns `r1`, `r2`, ... ids and de-duplicates assets by media path."""

    def __init__(self) -> None:
        self.element = ET.Element("resources")
        self._by_path: dict[Path, _Asset] = {}
        self._next = 1

    def _mint(self) -> str:
        identifier = f"r{self._next}"
        self._next += 1
        return identifier

    def add_format(self, sequence: Sequence) -> str:
        if sequence.frame_size is None or sequence.timebase is None:
            raise ValueError(
                "sequence has no frame size or timebase to derive a format from"
            )
        width, height = sequence.frame_size
        identifier = self._mint()
        ET.SubElement(
            self.element,
            "format",
            {
                "id": identifier,
                "name": f"FFVideoFormat{height}p",
                "frameDuration": _seconds(sequence.timebase),
                "width": str(width),
                "height": str(height),
            },
        )
        return identifier

    def add_asset(self, clip: TrackItem, format_id: str, has_video: bool) -> str:
        item = clip.project_item
        path = item.media_path if item is not None else None
        if item is None or path is None:
            # Synthetic media (Black Video, adjustment layers) and nested
            # sequences have no file to point at.
            return ""
        # An asset has to span every clip that reads from it. Premiere's
        # stills sit inside a 12-hour phantom source, so a clip's `start` can
        # be hours in even though it plays for seconds - declaring the marked
        # span alone would put the clip outside its own asset.
        needed = clip.in_point.ticks + clip.duration.ticks
        existing = self._by_path.get(path)
        if existing is not None:
            existing.extend(needed)
            # A/V media feeds a video track AND an audio track from the one
            # file. The asset is shared, so it has to declare every stream
            # that is actually used - claiming hasAudio="0" while an audio
            # lane references it loses the audio on import.
            existing.declare(has_video)
            return existing.identifier
        identifier = self._mint()
        element = ET.SubElement(
            self.element,
            "asset",
            {
                "id": identifier,
                "name": item.name,
                "start": "0s",
                "duration": _seconds(max(needed, 0)),
                "hasVideo": "1" if has_video else "0",
                "hasAudio": "0" if has_video else "1",
                "format": format_id,
            },
        )
        ET.SubElement(
            element,
            "media-rep",
            {"kind": "original-media", "src": _file_url(path)},
        )
        self._by_path[path] = _Asset(identifier, element, needed)
        return identifier


def _clip_element(
    clip: TrackItem, ref: str, offset: int, lane: int | None
) -> ET.Element:
    attributes = {
        "ref": ref,
        "name": clip.name,
        "offset": _seconds(offset),
        "start": _seconds(clip.in_point.ticks),
        "duration": _seconds(clip.duration.ticks),
    }
    if lane is not None:
        attributes["lane"] = str(lane)
    return ET.Element("asset-clip", attributes)


def _build_spine(
    sequence: Sequence, resources: _Resources, format_id: str
) -> ET.Element:
    spine = ET.Element("spine")
    primary = sequence.video_tracks[0].clips if sequence.video_tracks else []
    position = 0
    placed: list[tuple[int, int, ET.Element]] = []
    for clip in sorted(primary, key=lambda c: c.start.ticks):
        if clip.start.ticks > position:
            gap = ET.SubElement(
                spine,
                "gap",
                {
                    "name": "Gap",
                    "offset": _seconds(position),
                    "duration": _seconds(clip.start.ticks - position),
                },
            )
            placed.append((position, clip.start.ticks, gap))
        ref = resources.add_asset(clip, format_id, has_video=True)
        if ref:
            element = _clip_element(clip, ref, clip.start.ticks, None)
            spine.append(element)
            placed.append((clip.start.ticks, clip.end.ticks, element))
        position = max(position, clip.end.ticks)

    # The spine has to span the WHOLE sequence, not just its video: audio
    # routinely runs past the last video clip, and a connected clip can only
    # hang off a spine element that covers its start. `end` walks every clip
    # on every track, so read it once.
    sequence_end = sequence.end.ticks
    if sequence_end > position:
        placed.append(
            (
                position,
                sequence_end,
                ET.SubElement(
                    spine,
                    "gap",
                    {
                        "name": "Gap",
                        "offset": _seconds(position),
                        "duration": _seconds(sequence_end - position),
                    },
                ),
            )
        )

    # Everything that is not the primary video track becomes a connected
    # clip, hung off whichever spine element covers its start.
    lane = 0
    for track in sequence.video_tracks[1:]:
        lane += 1
        _connect(track.clips, spine, placed, resources, format_id, lane, True)
    lane = 0
    for track in sequence.audio_tracks:
        lane -= 1
        _connect(track.clips, spine, placed, resources, format_id, lane, False)
    return spine


def _connect(
    clips: list[TrackItem],
    spine: ET.Element,
    placed: list[tuple[int, int, ET.Element]],
    resources: _Resources,
    format_id: str,
    lane: int,
    has_video: bool,
) -> None:
    for clip in clips:
        ref = resources.add_asset(clip, format_id, has_video)
        if not ref:
            continue
        host = None
        host_start = 0
        for start, end, element in placed:
            if start <= clip.start.ticks < end:
                host, host_start = element, start
                break
        if host is None:
            # Nothing on the spine covers it; the format has no way to place
            # a connected clip there, so it would be silently lost.
            raise NotImplementedError(
                f"clip {clip.name!r} at {clip.start.seconds}s has no spine "
                "element to attach to - the primary video track must cover "
                "every connected clip's start"
            )
        host.append(_clip_element(clip, ref, clip.start.ticks - host_start, lane))


def export_fcpxml(
    project: Project, path: str | Path, sequence: Sequence | None = None
) -> Path:
    """Write `sequence` (default: the first) to `path` as FCPXML.

    Returns the path written. Raises `NotImplementedError` for a timeline
    whose connected clips cannot be expressed - see `_connect`.
    """
    if sequence is None:
        if not project.sequences:
            raise ValueError("project has no sequence to export")
        sequence = project.sequences[0]

    resources = _Resources()
    format_id = resources.add_format(sequence)
    spine = _build_spine(sequence, resources, format_id)

    root = ET.Element("fcpxml", {"version": FCPXML_VERSION})
    root.append(resources.element)
    library = ET.SubElement(root, "library")
    event = ET.SubElement(library, "event", {"name": project.name})
    project_element = ET.SubElement(event, "project", {"name": sequence.name})
    sequence_element = ET.SubElement(
        project_element,
        "sequence",
        {
            "format": format_id,
            "duration": _seconds(sequence.end.ticks),
            "tcStart": "0s",
            "tcFormat": (
                "DF"
                if sequence.video_display_format in _DROP_FRAME_DISPLAY_FORMATS
                else "NDF"
            ),
        },
    )
    sequence_element.append(spine)

    _indent(root, 0)
    target = Path(path)
    # `encoding="unicode"` to suppress ElementTree's own declaration - the
    # `xml_declaration` argument that would do it is 3.8+.
    target.write_bytes(
        b'<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE fcpxml>\n'
        + ET.tostring(root, encoding="unicode").encode("utf-8")
    )
    return target
