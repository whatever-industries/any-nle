"""The `Transition` model."""

from __future__ import annotations

from typing import TYPE_CHECKING

from .color import Color
from .time import Time

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET

    from .track import Track


class Transition:
    """A transition instance on a sequence track.

    ExtendScript has no transitions API; this mirrors the stored XML
    (`VideoTransitionTrackItem`), which is also what UXP's
    `getTrackItems(TrackItemType.TRANSITION)` exposes.

    Which of the two clips it joins are real decides how it is aligned:

    | incoming | outgoing | alignment                                  |
    | -------- | -------- | ------------------------------------------ |
    | yes      | no       | at a clip's head, entirely after the cut   |
    | no       | yes      | at a clip's tail, entirely before the cut  |
    | yes      | yes      | on a cut, straddling it                    |
    """

    def __init__(self, _element: ET.Element, track: Track) -> None:
        self._element = _element
        self.track = track

    def _text(self, tag: str) -> str:
        return self._element.findtext(f"TransitionTrackItem/{tag}") or ""

    @property
    def name(self) -> str:
        """The display name (`Additive Dissolve (Legacy)`). Read-only."""
        return self._text("DisplayName")

    @property
    def match_name(self) -> str:
        """The effect matchName (`ADBE Additive Dissolve`). Read-only."""
        return self._text("MatchName")

    @property
    def cut_point_offset(self) -> Time:
        """How much of the transition lies before the cut. Read-only.

        The stored `Alignment` is a tick offset, not an enum: a head
        transition stores 0, a tail one stores its whole duration, and one
        on a cut stores however much of itself falls before it (Premiere
        clamps that to the handles the two clips actually have, so it is
        rarely exactly half).
        """
        return Time(int(self._text("Alignment") or 0))

    @property
    def cut_point(self) -> Time:
        """The edit point this transition covers. Read-only.

        `start` plus `cut_point_offset`.
        """
        return self.start + self.cut_point_offset

    @property
    def start(self) -> Time:
        """The start on the sequence timeline. Read-only.

        Premiere elides a zero start, like clip track items.
        """
        return Time(
            int(self._element.findtext("TransitionTrackItem/TrackItem/Start") or 0)
        )

    @property
    def end(self) -> Time:
        """The end on the sequence timeline. Read-only."""
        return Time(
            int(self._element.findtext("TransitionTrackItem/TrackItem/End") or 0)
        )

    @property
    def duration(self) -> Time:
        """The timeline duration (`end` minus `start`). Read-only."""
        return self.end - self.start

    @property
    def border_width(self) -> int:
        """The border drawn along the wipe edge. Read-only.

        `0` for transitions that have no border (a dissolve stores nothing).
        """
        return int(self._element.findtext("BorderWidth") or 0)

    @property
    def border_color(self) -> Color | None:
        """The border colour, or `None` when the transition has no border.

        Stored as a comma-separated RGB triple.
        """
        stored = self._element.findtext("BorderColor")
        if not stored:
            return None
        red, green, blue = (int(part) for part in stored.split(","))
        return Color(red, green, blue)

    @property
    def is_reversed(self) -> bool:
        """Whether the transition plays in its reverse direction. Read-only."""
        return self._element.findtext("Reverse") == "true"

    @property
    def direction(self) -> int:
        """The direction a directional transition runs in. Read-only.

        The stored code, which varies per transition - Band Wipe writes `10`
        for its default. Left raw because there is no vocabulary to map it to:
        no scripting API exposes the field at all.
        """
        return int(self._element.findtext("Direction") or 0)

    @property
    def anti_alias_quality(self) -> int:
        """The edge anti-aliasing quality (`0` off). Read-only."""
        return int(self._element.findtext("AntiAliasQuality") or 0)

    @property
    def has_incoming_clip(self) -> bool:
        """Whether a clip starts under this transition. Read-only."""
        return self._text("HasIncomingClip") == "true"

    @property
    def has_outgoing_clip(self) -> bool:
        """Whether a clip ends under this transition. Read-only."""
        return self._text("HasOutgoingClip") == "true"

    def __repr__(self) -> str:
        return (
            f"Transition(name={self.name!r}, "
            f"start={self.start.seconds:.3f}s, end={self.end.seconds:.3f}s)"
        )
