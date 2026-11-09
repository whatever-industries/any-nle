"""The `Marker` model."""

from __future__ import annotations

import json
import uuid
import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING

from ..data.marker_colors import (
    DEFAULT_INDEX_BY_TYPE,
    MARKER_COLOR_PACKED,
    PACKED_TO_INDEX,
)
from ..xml.mutations import append_pair, remove_child
from .time import Time, validate_time
from .validators import validate_marker_color_index, validate_one_of, validate_string

#: ClassID of a top-level Marker object.
MARKER_CLASS_ID = "a45508e0-3ff7-4d04-90a7-2e0dfff4c910"

_validate_text = validate_string()
_validate_color_index = validate_marker_color_index
_validate_marker_type = validate_one_of(
    ["Comment", "Chapter", "Segmentation", "WebLink"]
)

if TYPE_CHECKING:
    from typing import Any

    from ..xml import PremiereDocument


def _cue_color(cue: object) -> int | None:
    # A colour cue point stores the JSON `{"color": <packed>}` as its
    # `mValue`; every other cue kind stores a different payload there.
    if not isinstance(cue, dict):
        return None
    raw = cue.get("mValue")
    if not isinstance(raw, str):
        return None
    try:
        value = json.loads(raw)
    except ValueError:
        return None
    if not isinstance(value, dict):
        return None
    color = value.get("color")
    return color if isinstance(color, int) else None


def _attach_marker(
    document: PremiereDocument, marker_list: ET.Element, marker: Marker
) -> None:
    # Register the marker object in the top-level table and add its
    # `<First>/<Second>` pair to the owner's inner Markers list.
    new_id = document.add_object(marker._element)
    append_pair(marker_list, marker.guid, new_id)


def _detach_marker(
    document: PremiereDocument, marker_list: ET.Element, marker: Marker
) -> None:
    object_id = marker._element.get("ObjectID")
    for pair in marker_list.findall("Marker"):
        second = pair.find("Second")
        if second is not None and second.get("ObjectRef") == object_id:
            remove_child(marker_list, pair)
            break
    # Keep the pair indices contiguous, as Premiere does.
    for index, pair in enumerate(marker_list.findall("Marker")):
        pair.set("Index", str(index))
    document.remove_object(marker._element)


class Marker:
    """A marker on a sequence (or project item).

    Backed by the `DVAMarker` JSON blob; a point marker has no stored end
    time and reports `end == start`, like ExtendScript.
    """

    def __init__(
        self,
        name: str,
        start: Time,
        comments: str = "",
        marker_type: str = "Comment",
        duration: Time | None = None,
    ) -> None:
        _validate_text(name)
        validate_time(start)
        _validate_text(comments)
        _validate_marker_type(marker_type)
        if duration is not None:
            validate_time(duration)
            if duration.ticks < 0:
                # `end` refuses to precede `start`; the constructor must not
                # be a back door to the same state.
                raise ValueError("duration must not be negative")
        blob: dict[str, object] = {
            "mMarkerID": str(uuid.uuid4()),
            "mName": name,
            "mStartTime": {"ticks": start.ticks},
            "mType": marker_type,
        }
        if comments:
            blob["mComment"] = comments
        if duration is not None and duration.ticks:
            blob["mDuration"] = {"ticks": duration.ticks}
        # ObjectID placeholder keeps the attribute first, as Premiere writes
        # it; the owner fills it in when wiring the marker into a document.
        element = ET.Element(
            "Marker",
            {"ObjectID": "", "ClassID": MARKER_CLASS_ID, "Version": "3"},
        )
        element.text = "\n\t\t"
        payload = ET.SubElement(element, "DVAMarker")
        payload.text = json.dumps(
            {"DVAMarker": blob}, sort_keys=True, separators=(",", ":")
        )
        payload.tail = "\n\t"
        self._element = element

    @classmethod
    def _from_xml(cls, _element: ET.Element) -> Marker:
        marker = cls.__new__(cls)
        marker._element = _element
        return marker

    def _blob(self) -> ET.Element:
        element = self._element.find("DVAMarker")
        if element is None:
            raise ValueError("marker has no DVAMarker payload")
        return element

    def _data(self) -> dict[str, Any]:
        data = json.loads(self._blob().text or "{}")
        inner = data.get("DVAMarker")
        return inner if isinstance(inner, dict) else {}

    def _get_str(self, key: str) -> str:
        value = self._data().get(key, "")
        return value if isinstance(value, str) else ""

    def _write(self, key: str, value: object) -> None:
        data = self._data()
        data[key] = value
        # Premiere writes the blob compact with alphabetical keys.
        self._blob().text = json.dumps(
            {"DVAMarker": data}, sort_keys=True, separators=(",", ":")
        )

    @property
    def name(self) -> str:
        """The marker name. Read/write."""
        return self._get_str("mName")

    @name.setter
    def name(self, value: str) -> None:
        _validate_text(value)
        self._write("mName", value)

    @property
    def comments(self) -> str:
        """The marker comments. Read/write."""
        return self._get_str("mComment")

    @comments.setter
    def comments(self, value: str) -> None:
        _validate_text(value)
        self._write("mComment", value)

    @property
    def start(self) -> Time:
        """The marker start time. Read/write."""
        return Time(int(self._data().get("mStartTime", {}).get("ticks", 0)))

    @start.setter
    def start(self, value: Time) -> None:
        validate_time(value)
        self._write("mStartTime", {"ticks": value.ticks})

    @property
    def end(self) -> Time:
        """The marker end time (`start` plus the stored duration). Read/write."""
        data = self._data()
        duration = data.get("mDuration")
        if isinstance(duration, dict) and "ticks" in duration:
            return self.start + Time(int(duration["ticks"]))
        return self.start

    @end.setter
    def end(self, value: Time) -> None:
        validate_time(value)
        if value.ticks < self.start.ticks:
            raise ValueError("end must not precede start")
        self._write("mDuration", {"ticks": value.ticks - self.start.ticks})

    @property
    def type(self) -> str:
        """The marker type (e.g. `Comment`). Read-only."""
        return self._get_str("mType")

    @property
    def guid(self) -> str:
        """The marker's persistent identifier. Read-only."""
        return self._get_str("mMarkerID")

    @property
    def color_index(self) -> int | None:
        """The marker color as a palette index (`0`..`7`). Read/write.

        Uncolored markers report their type's default (Comment green,
        Chapter red, Segmentation magenta, WebLink orange), matching what
        Premiere's `getColorIndex` returns. `None` for a stored color
        outside the palette.
        """
        packed = self._packed_color()
        if packed is None:
            return DEFAULT_INDEX_BY_TYPE.get(self.type, 0)
        return PACKED_TO_INDEX.get(packed)

    @color_index.setter
    def color_index(self, index: int) -> None:
        _validate_color_index(index)
        data = self._data()
        cues = data.get("mCuePointList")
        if not isinstance(cues, list):
            cues = []
        value = json.dumps({"color": MARKER_COLOR_PACKED[index]}, separators=(",", ":"))
        for cue in cues:
            if _cue_color(cue) is not None:
                cue["mValue"] = value
                break
        else:
            # Premiere tags each cue point with a fresh GUID key.
            cues.append({"mKey": f"keywordExtDVAv1_{uuid.uuid4()}", "mValue": value})
        self._write("mCuePointList", cues)

    def _packed_color(self) -> int | None:
        cues = self._data().get("mCuePointList")
        if not isinstance(cues, list):
            return None
        for cue in cues:
            color = _cue_color(cue)
            if color is not None:
                return color
        return None

    @property
    def web_link_url(self) -> str:
        """The URL of a `WebLink` marker. Read-only."""
        return self._get_str("mLocation")

    @property
    def web_link_frame_target(self) -> str:
        """The frame target of a `WebLink` marker. Read-only."""
        return self._get_str("mTarget")

    def set_type_as_comment(self) -> None:
        """Make this a comment marker."""
        self._write("mType", "Comment")

    def set_type_as_chapter(self) -> None:
        """Make this a chapter marker."""
        self._write("mType", "Chapter")

    def set_type_as_segmentation(self) -> None:
        """Make this a segmentation marker."""
        self._write("mType", "Segmentation")

    def set_type_as_web_link(self, url: str, frame_target: str = "") -> None:
        """Make this a web-link marker."""
        _validate_text(url)
        _validate_text(frame_target)
        self._write("mType", "WebLink")
        self._write("mLocation", url)
        self._write("mTarget", frame_target)

    def __repr__(self) -> str:
        return f"Marker(name={self.name!r}, start={self.start.seconds:.3f}s)"
