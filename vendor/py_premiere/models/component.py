"""The `Component` and `ComponentParam` models."""

from __future__ import annotations

import base64
import binascii
import math
import re
import struct
import uuid
import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING, NamedTuple, cast

from ..enums import KeyframeInterpolation
from ..xml.mutations import insert_before, remove_child
from .caption import (
    decode_caption_text,
    is_payload_text,
    read_font_family,
    replace_payload_text,
    write_font_family,
    write_payload_element,
)
from .color import Color
from .mask_builder import attach_sub_mask, build_mask_component, build_mask_params
from .named_list import NamedList
from .time import Time, validate_time
from .validators import (
    validate_bool,
    validate_color,
    validate_float32,
    validate_packed_color,
    validate_string,
    validate_vector2,
)

if TYPE_CHECKING:
    from typing import Iterator

    from ..xml import PremiereDocument
    from .track_item import TrackItem

#: A scalar keyframe value: digits with an optional (possibly empty)
#: fraction, as Premiere writes floats (`50.`, `25.5`, `1.`). The exponent
#: form is not something Premiere writes (0 of 12206 value fields across the
#: sample corpus) but older py releases emitted it for values below 1e-4, so
#: the reader still accepts it.
_SCALAR = re.compile(r"-?\d+(\.\d*)?([eE][-+]?\d+)?\Z")

#: ClassID prefix of color params (a packed `0xAA00RR00GG00BB00` uint64).
_COLOR_CLASS = "0fde4e9f"

#: The `FormattedTextData` FlatBuffer magic, shared by caption blocks and
#: Essential Graphics `Source Text` (66_eg_text).
_TEXT_MAGIC = b"\x44\x33\x22\x11"

_validate_text = validate_string(allow_empty=False)

#: ClassID prefix of boolean (checkbox) params; value field is `true`/`false`.
_BOOL_CLASS = "cc12343e"

#: ClassID prefix of 2D point params (e.g. Motion Position); value field is
#: `x:y` of float32 scalars.
_POINT_CLASS = "ca81d347"

#: Full ClassIDs whose value field is a plain float32 scalar (settable).
_AUDIO_SCALAR_CLASS = "a714635e-a628-4b27-9d59-77eba47dbc1a"
_SCALAR_VALUE_CLASSES = {
    "fe47129e-6c94-4fc0-95d5-c056a517aaf3",  # sliders/angles
    _AUDIO_SCALAR_CLASS,  # audio scalars
    # Mask scalars (Feather, Opacity, Expansion): every stored value in
    # 26_effect_mask follows the float32 trailing-dot rule (`25.`, `100.`).
    "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542",
}


#: HOLD keyframes use a wider outgoing influence than the 1/6 default.
_HOLD = KeyframeInterpolation.HOLD


def _float32(value: float) -> float:
    return cast("float", struct.unpack("f", struct.pack("f", value))[0])


def _format_scalar(value: float) -> str:
    # Premiere stores scalar values as float32, integers with a trailing
    # dot (`100.`) and others at 12 significant figures with `%g`'s
    # trailing-zero stripping (`0.800000011921`, `0.34999999404` - the
    # latter is 349999994040 with the zero stripped, which made 11 look
    # right until 63_audio_keyframes stored a 12-digit value). Never in
    # exponent form: `%g` switches to it below 1e-4, and such a value reads
    # back as a string rather than a number, so small magnitudes are
    # spelled out in full.
    float32 = _float32(value)
    if float32 == int(float32):
        return f"{int(float32)}."
    text = f"{float32:.12g}"
    if "e" not in text and "E" not in text:
        return text
    exponent = math.floor(math.log10(abs(float32)))
    decimals = max(0, 12 - 1 - exponent)
    plain = f"{float32:.{decimals}f}".rstrip("0")
    return plain if not plain.endswith(".") else plain + "0"


def _format_current_value(value: float) -> str:
    # `CurrentValue` mirrors the float32 static value at full double
    # precision (`0.34999999403953552` in 10_audio_volume). Like every other
    # numeric field it is never written in exponent form.
    float32 = _float32(value)
    if float32 == 0:
        return "0"
    text = f"{float32:.17g}"
    if "e" not in text and "E" not in text:
        return text
    exponent = math.floor(math.log10(abs(float32)))
    decimals = max(0, 17 - 1 - exponent)
    plain = f"{float32:.{decimals}f}".rstrip("0")
    return plain if not plain.endswith(".") else plain + "0"


def _format_tangent(value: float) -> str:
    # Slopes and influences: integers plain (`50`, `0`), fractions at 17
    # significant figures (`0.16666666666666666`, `0.33333333333333331`).
    return str(int(value)) if value == int(value) else f"{value:.17g}"


def _build_keyframe_string(
    keys: list[tuple[Time, float, KeyframeInterpolation]],
) -> str:
    # Entry layout `ticks,value,interp,0,inSlope,inInf,outSlope,outInf`.
    # inSlope is the value rate (units/second) from the previous key,
    # outSlope the rate to the next (0 for the last key or a HOLD key);
    # influence is 1/6, except a HOLD key's outgoing influence is 1/3.
    # Slopes derive from the FLOAT32-ROUNDED values, not the passed doubles:
    # 63_audio_keyframes stores 0.60000000894069672, which is exactly
    # float32(0.8) - float32(0.2) - invisible before that fixture because
    # every earlier corpus slope was an integer.
    entries = []
    keys = [
        (time, _float32(value), interpolation) for time, value, interpolation in keys
    ]
    for index, (time, value, interpolation) in enumerate(keys):
        if index > 0:
            previous = keys[index - 1]
            seconds = (time - previous[0]).seconds
            in_slope = (value - previous[1]) / seconds
        else:
            in_slope = 0.0
        if index < len(keys) - 1 and interpolation is not _HOLD:
            following = keys[index + 1]
            seconds = (following[0] - time).seconds
            out_slope = (following[1] - value) / seconds
        else:
            out_slope = 0.0
        out_influence = 1 / 3 if interpolation is _HOLD else 1 / 6
        entries.append(
            f"{time.ticks},{_format_scalar(value)},{int(interpolation)},0,"
            f"{_format_tangent(in_slope)},{_format_tangent(1 / 6)},"
            f"{_format_tangent(out_slope)},{_format_tangent(out_influence)}"
        )
    return ";".join(entries) + ";"


def _unpack_color(packed: int) -> Color:
    # Each 8-bit channel sits in the high byte of a 16-bit word, ordered
    # A, R, G, B from the top; verified against Premiere's own RGB.
    return Color(
        red=(packed >> 40) & 0xFF,
        green=(packed >> 24) & 0xFF,
        blue=(packed >> 8) & 0xFF,
        alpha=(packed >> 56) & 0xFF,
    )


def _pack_color(color: Color) -> int:
    return (
        (color.alpha << 56)
        | (color.red << 40)
        | (color.green << 24)
        | (color.blue << 8)
    )


def _decode_value(field: str) -> float | bool | list[float] | str:
    # Keyframe entries are `time,value,<interpolation fields>`; the value
    # field encodes scalars (`50.`), booleans (`true`), and multi-component
    # values (`x:y`). Unknown encodings pass through as the raw string.
    if field == "true":
        return True
    if field == "false":
        return False
    if ":" in field:
        parts = field.split(":")
        if all(_SCALAR.match(part) for part in parts):
            return [float(part) for part in parts]
        return field
    if _SCALAR.match(field):
        return float(field)
    return field


def _split_keyframe(entry: str) -> tuple[int, str]:
    ticks_text, _, rest = entry.partition(",")
    value_field, _, _ = rest.partition(",")
    return int(ticks_text), value_field


class PathVertex(NamedTuple):
    """One vertex of a shape path, with its two bezier handles.

    A corner point stores both handles coincident with the vertex, which is
    how every rectangle in the corpus is written.
    """

    x: float
    y: float
    in_x: float
    in_y: float
    out_x: float
    out_y: float
    flag: float
    """Leading per-vertex float; 0 for every vertex seen so far."""


class SpatialTangents(NamedTuple):
    """The motion-path handles of a 2D keyframe, in the value's own units."""

    in_x: float
    in_y: float
    out_x: float
    out_y: float


#: A 2D keyframe entry carries fourteen fields, not the scalar's eight:
#: `ticks,x:y,interpIn,interpOut,inSlope,inInf,outSlope,outInf` then a
#: constant pair and the four SPATIAL handles.
_POINT_ENTRY_FIELDS = 14
#: Index of the first spatial handle, after that constant pair.
_SPATIAL_START = 10

#: Version word every stored shape path starts with.
_PATH_VERSION = 2
#: Floats per vertex in the FLAT layout: flag, point, and the two handles.
_PATH_STRIDE = 7

#: The drawn-mask SUBPATH layout (26_effect_mask's ellipse): after the
#: version and subpath count, each subpath carries a header - path id, a
#: zero pad byte, a sub-version word and its vertex count - and vertices
#: whose leading flag is an integer, not the flat layout's float.
_PATH_SUBPATH_HEADER = struct.Struct("<IBII")
_PATH_SUBPATH_VERTEX = struct.Struct("<I6f")

#: The path id the fixture's drawn subpath carries. Its meaning is
#: undecoded (a per-path identifier by the look of it); reused as the
#: default so rewriting the fixture's own vertices reproduces its bytes.
_DEFAULT_PATH_ID = 31062


def _decode_subpaths(
    raw: bytes, subpath_count: int
) -> tuple[list[tuple[int, list[PathVertex]]], int] | None:
    offset = 8
    subpaths = []
    for _ in range(subpath_count):
        if offset + _PATH_SUBPATH_HEADER.size > len(raw):
            return None
        path_id, pad, sub_version, vertex_count = _PATH_SUBPATH_HEADER.unpack_from(
            raw, offset
        )
        if pad != 0 or sub_version != _PATH_VERSION:
            return None
        offset += _PATH_SUBPATH_HEADER.size
        vertices = []
        for _ in range(vertex_count):
            if offset + _PATH_SUBPATH_VERTEX.size > len(raw):
                return None
            fields = _PATH_SUBPATH_VERTEX.unpack_from(raw, offset)
            vertices.append(
                PathVertex(
                    x=fields[1],
                    y=fields[2],
                    in_x=fields[3],
                    in_y=fields[4],
                    out_x=fields[5],
                    out_y=fields[6],
                    flag=float(fields[0]),
                )
            )
            offset += _PATH_SUBPATH_VERTEX.size
        subpaths.append((path_id, vertices))
    if offset + 1 != len(raw):
        return None
    return subpaths, raw[offset]


def _encode_subpaths(
    subpaths: list[tuple[int, list[PathVertex]]], closed: bool
) -> bytes:
    out = bytearray(struct.pack("<II", _PATH_VERSION, len(subpaths)))
    for path_id, vertices in subpaths:
        out += _PATH_SUBPATH_HEADER.pack(path_id, 0, _PATH_VERSION, len(vertices))
        for vertex in vertices:
            out += _PATH_SUBPATH_VERTEX.pack(
                int(vertex.flag),
                vertex.x,
                vertex.y,
                vertex.in_x,
                vertex.in_y,
                vertex.out_x,
                vertex.out_y,
            )
    out += bytes([1 if closed else 0])
    return bytes(out)


class ComponentParam:
    """A parameter of a component.

    Scalar, boolean, point and color values decode to Python values; other
    encodings (popups, arbitrary data) are returned as their raw serialized
    string until their per-ClassID encodings are mapped.
    """

    def __init__(
        self,
        _element: ET.Element,
        _document: PremiereDocument,
        component: Component | TrackItem,
    ) -> None:
        self._element = _element
        self._document = _document
        #: The owning component, or the track item itself for the time-remap
        #: speed param - which Premiere stores outside any component chain.
        self.component = component

    @property
    def display_name(self) -> str:
        """The parameter name. Read-only."""
        return self._element.findtext("Name") or ""

    @property
    def class_id(self) -> str:
        """The parameter's serialization class GUID. Read-only.

        The same tag serves several value encodings; the ClassID identifies
        which one (see `data/param_classes.py`).
        """
        return self._element.get("ClassID", "")

    @property
    def is_time_varying(self) -> bool:
        """Whether the param is keyframed. Read-only."""
        if self._element.findtext("IsTimeVarying") == "true":
            return True
        # Audio params drop the flag once keyframed and rely on a populated
        # `Keyframes` list instead.
        return bool((self._element.findtext("Keyframes") or "").strip())

    def _start_keyframe(self) -> tuple[int, str] | None:
        text = self._element.findtext("StartKeyframe")
        if not text:
            return None
        return _split_keyframe(text)

    def _keyframe_entries(self) -> list[tuple[int, str]]:
        text = self._element.findtext("Keyframes")
        if not text:
            return []
        return [_split_keyframe(entry) for entry in text.split(";") if entry]

    @property
    def value(self) -> float | bool | list[float] | str | None:
        """The static value. Read/write on scalar, boolean and point params.

        Reads `None` when the param has no stored value (a synthesized
        default). The setter accepts a number (scalar param), a `bool`
        (checkbox param) or a two-number sequence (2D point param) that
        already has a stored value; keyframed and other params raise.
        """
        start = self._start_keyframe()
        if start is None:
            return None
        return _decode_value(start[1])

    @value.setter
    def value(self, new_value: float | bool | list[float] | int) -> None:
        if self.is_time_varying:
            raise ValueError("cannot set a static value on a keyframed parameter")
        field = self._encode_static_value(new_value)
        element = self._element.find("StartKeyframe")
        if element is None or not element.text:
            raise ValueError("parameter has no stored value to set")
        fields = element.text.split(",")
        fields[1] = field
        element.text = ",".join(fields)
        if self.class_id in _SCALAR_VALUE_CLASSES:
            # Audio scalars mirror the static value in `CurrentValue` as the
            # float32 printed at full double precision (10_audio_volume:
            # `0.34999999403953552` beside a `0.34999999404` start keyframe);
            # leaving it stale would diverge from Premiere's own setValue.
            current = self._element.find("CurrentValue")
            if current is not None:
                current.text = _format_current_value(cast("float", new_value))

    def _encode_static_value(self, new_value: float | bool | list[float] | int) -> str:
        class_id = self.class_id
        if class_id in _SCALAR_VALUE_CLASSES:
            validate_float32(new_value)
            return _format_scalar(cast("float", new_value))
        if class_id.startswith(_BOOL_CLASS):
            validate_bool(new_value)
            return "true" if new_value else "false"
        if class_id.startswith(_POINT_CLASS):
            validate_vector2(new_value)
            point = cast("list[float]", new_value)
            return f"{_format_scalar(point[0])}:{_format_scalar(point[1])}"
        if class_id.startswith(_COLOR_CLASS):
            # Stored as the bare decimal of the packed `0xAA00RR00GG00BB00`
            # uint64 - no trailing dot, unlike scalars (61_tint stores `0`
            # and `280379743338240`).
            validate_packed_color(new_value)
            return str(new_value)
        raise ValueError(
            "value is only settable on scalar, boolean, point and color parameters"
        )

    @property
    def keys(self) -> list[Time]:
        """Keyframe times. Read-only."""
        return [Time(ticks) for ticks, _ in self._keyframe_entries()]

    def get_value_at_key(self, time: Time) -> float | bool | list[float] | str | None:
        """The value at an existing keyframe."""
        validate_time(time)
        for ticks, value_field in self._keyframe_entries():
            if ticks == time.ticks:
                return _decode_value(value_field)
        return None

    def find_nearest_key(
        self, time: Time, threshold: Time | None = None
    ) -> Time | None:
        """The keyframe closest to `time`, or `None` if there are none.

        ExtendScript's `findNearestKey(time, threshold)`. With a
        `threshold`, a key further away than that is not returned. Ties go
        to the earlier key.
        """
        validate_time(time)
        if threshold is not None:
            validate_time(threshold)
        best: Time | None = None
        best_distance = -1
        for key in self.keys:
            distance = abs(key.ticks - time.ticks)
            if best is None or distance < best_distance:
                best, best_distance = key, distance
        if best is None:
            return None
        if threshold is not None and best_distance > threshold.ticks:
            return None
        return best

    def find_next_key(self, time: Time) -> Time | None:
        """The first keyframe strictly after `time`, or `None`."""
        validate_time(time)
        later = [key for key in self.keys if key.ticks > time.ticks]
        return min(later, key=lambda key: key.ticks) if later else None

    def find_previous_key(self, time: Time) -> Time | None:
        """The last keyframe strictly before `time`, or `None`."""
        validate_time(time)
        earlier = [key for key in self.keys if key.ticks < time.ticks]
        return max(earlier, key=lambda key: key.ticks) if earlier else None

    @property
    def color(self) -> Color | None:
        """The static value of a color param as an RGBA `Color`. Read/write.

        `None` for a non-color param or one with no stored value. The
        packed uint64 holds each 8-bit channel in the high byte of a
        16-bit word (`0xAA00RR00GG00BB00`); verified against Premiere's own
        RGB. `value` returns the raw packed number for ES parity.

        The setter takes a `Color`. Mind the alpha: some effects store their
        colors with alpha 0 (Tint's defaults do), and `Color`'s alpha
        defaults to 255.
        """
        if not self.class_id.startswith(_COLOR_CLASS):
            return None
        start = self._start_keyframe()
        if start is None:
            return None
        field = start[1]
        if not field.lstrip("-").isdigit():
            return None
        return _unpack_color(int(field))

    @color.setter
    def color(self, new_value: Color) -> None:
        if not self.class_id.startswith(_COLOR_CLASS):
            raise ValueError("not a color parameter")
        validate_color(new_value)
        self.value = _pack_color(new_value)

    def get_interpolation_at_key(self, time: Time) -> KeyframeInterpolation | None:
        """The temporal interpolation of an existing keyframe.

        This is the INCOMING interpolation, stored as the third keyframe
        field (after time and value); decoded against UXP-generated knowns
        in the `09_keyframes` fixture. Entry layout: `ticks,value,interpIn,
        interpOut,inSlope,inInfluence,outSlope,outInfluence`.
        """
        return self._interpolation_field(time, 2)

    def get_spatial_tangents_at_key(self, time: Time) -> SpatialTangents | None:
        """The motion-path handles of an existing 2D keyframe.

        A 2D (point) keyframe stores FOURTEEN fields where a scalar stores
        eight: the extra four at the end are the spatial bezier handles that
        shape the motion path, distinct from the temporal slopes earlier in
        the entry, and they appear as mirrored pairs on a smooth key.
        `None` for a scalar parameter, which has no motion path.
        """
        validate_time(time)
        text = self._element.findtext("Keyframes") or self._element.findtext(
            "StartKeyframe"
        )
        if not text:
            return None
        for entry in text.split(";"):
            fields = entry.split(",")
            if len(fields) != _POINT_ENTRY_FIELDS or fields[0] != str(time.ticks):
                continue
            handles = fields[_SPATIAL_START:]
            return SpatialTangents(*(float(value) for value in handles))
        return None

    @property
    def text(self) -> str | None:
        """The text this parameter stores, if it stores any. Read-only.

        An arbitrary-data parameter keeps its payload as base64. The
        Motion Graphics template controls keep theirs as UTF-16LE: a plain
        string for a simple control, a JSON object for a styled one (the
        edited text under `textEditValue`). An Essential Graphics `Source
        Text` stores the same `FormattedTextData` FlatBuffer captions use
        (66_eg_text carries the `0x11223344` magic), whose last string is
        the plain text. `None` for every parameter whose payload is not
        text - a scalar's, and the binary shape blob behind `path`.
        """
        raw = self._payload()
        if raw is None:
            return None
        if raw[8:12] == _TEXT_MAGIC:
            return decode_caption_text(raw)
        try:
            decoded = raw.decode("utf-16-le")
        except UnicodeDecodeError:
            return None
        return decoded if is_payload_text(decoded) else None

    @text.setter
    def text(self, value: str) -> None:
        _validate_text(value)
        raw, element = self._styled_text_payload()
        write_payload_element(self._document, element, replace_payload_text(raw, value))

    @property
    def font_family(self) -> str | None:
        """The font family a styled-text parameter is set in. Read/write.

        `None` for parameters holding no `FormattedTextData` payload. The
        family is a string in the payload's font vector; Premiere resolves
        an unknown one to a fallback on open.
        """
        raw = self._payload()
        if raw is None or raw[8:12] != _TEXT_MAGIC:
            return None
        return str(read_font_family(raw))

    @font_family.setter
    def font_family(self, value: str) -> None:
        _validate_text(value)
        raw, element = self._styled_text_payload()
        write_payload_element(self._document, element, write_font_family(raw, value))

    def _styled_text_payload(self) -> tuple[bytes, ET.Element]:
        raw = self._payload()
        element = self._element.find("StartKeyframeValue")
        if raw is None or element is None:
            raise ValueError("parameter holds no text payload")
        if raw[8:12] != _TEXT_MAGIC:
            raise NotImplementedError(
                "only FormattedTextData payloads are writable; this one is "
                "a Motion Graphics template control"
            )
        return raw, element

    def _payload(self) -> bytes | None:
        # An arbitrary-data value, resolved through the document's hash index:
        # a template's parameter values are stored once and referenced by
        # hash from every copy, so this element's own text is often empty.
        element = self._element.find("StartKeyframeValue")
        if element is None:
            return None
        try:
            return self._document.payload(element)
        except (ValueError, binascii.Error):
            return None

    @property
    def path(self) -> list[PathVertex] | None:
        """The shape this parameter stores, if it stores one. Read/write.

        Mask and shape geometry lives in an arbitrary-data parameter
        (named `Path`) in one of two little-endian layouts sharing a
        version word and a trailing closed-path byte: the FLAT one (a
        vertex count, then seven floats per vertex - a leading flag, the
        point, and its bezier handles) and the drawn-mask SUBPATH one
        (contours, each with a path id and integer-flagged vertices -
        26_effect_mask's drawn ellipse).

        `None` when the parameter holds no shape at all; an EMPTY list
        when it holds a shape with no vertices, which is what a default
        (undrawn) mask stores - its geometry lives in the sibling
        Type/Scale/Rotation parameters instead.

        The setter writes the vertices as one closed subpath (the shape
        the fixture's drawn mask stores); an empty list writes the bare
        header.
        """
        raw = self._payload()
        if raw is None:
            return None
        if len(raw) < 8:
            return None
        version, count = struct.unpack_from("<II", raw, 0)
        if version != _PATH_VERSION:
            return None
        # The FLAT layout (the corpus rectangles): `count` vertices of
        # seven floats and a trailing closed-path byte; an empty path is
        # just the two header words. Anything else with a matching version
        # is the drawn-mask SUBPATH layout, where `count` counts contours.
        expected = count * _PATH_STRIDE
        if len(raw) == 8 + expected * 4 + (1 if count else 0):
            floats = struct.unpack_from(f"<{expected}f", raw, 8)
            return [
                PathVertex(
                    x=group[1],
                    y=group[2],
                    in_x=group[3],
                    in_y=group[4],
                    out_x=group[5],
                    out_y=group[6],
                    flag=group[0],
                )
                for group in (
                    floats[index : index + _PATH_STRIDE]
                    for index in range(0, expected, _PATH_STRIDE)
                )
            ]
        decoded = _decode_subpaths(raw, count)
        if decoded is None:
            return None
        return [vertex for _, vertices in decoded[0] for vertex in vertices]

    @path.setter
    def path(self, vertices: list[PathVertex]) -> None:
        element = self._element.find("StartKeyframeValue")
        if self.display_name != "Path" or element is None:
            raise ValueError("path is only settable on a Path parameter")
        if not isinstance(vertices, list):
            raise TypeError(f"expected a list, got {type(vertices).__name__}")
        for vertex in vertices:
            if not isinstance(vertex, PathVertex):
                raise TypeError(
                    f"expected PathVertex entries, got {type(vertex).__name__}"
                )
            for coordinate in vertex:
                if not math.isfinite(coordinate):
                    raise ValueError("path coordinates must be finite")
        if vertices:
            raw = _encode_subpaths([(_DEFAULT_PATH_ID, vertices)], closed=True)
        else:
            # An empty path is the bare header, as shape masks store.
            raw = struct.pack("<II", _PATH_VERSION, 0)
        element.set("Encoding", "base64")
        element.set("BinaryHash", str(uuid.uuid4()))
        element.text = base64.b64encode(raw).decode("ascii") + "\n\t\t"
        self._document._by_binary_hash = None

    def get_out_interpolation_at_key(self, time: Time) -> KeyframeInterpolation | None:
        """The OUTGOING temporal interpolation of an existing keyframe.

        The fourth keyframe field, which a key only uses when it leaves on a
        different curve than it arrived on: across the corpus it is non-zero
        only where the incoming interpolation is `BEZIER`, and then carries
        another interpolation constant (a bezier-in / hold-out key, say).
        Zero - the overwhelmingly common case - means the key leaves the way
        it arrived.
        """
        return self._interpolation_field(time, 3)

    def _interpolation_field(
        self, time: Time, index: int
    ) -> KeyframeInterpolation | None:
        validate_time(time)
        text = self._element.findtext("Keyframes")
        if not text:
            return None
        for entry in text.split(";"):
            fields = entry.split(",")
            if len(fields) > index and fields[0] == str(time.ticks):
                return KeyframeInterpolation(int(fields[index]))
        return None

    def set_keyframes(
        self, keys: list[tuple[Time, float, KeyframeInterpolation]]
    ) -> None:
        """Set the keyframes of a scalar parameter.

        `keys` is a list of `(time, value, interpolation)`; bezier tangents
        are auto-computed to match Premiere's own output (slope from/to the
        neighbouring key, `1/6` influence, `1/3` outgoing for HOLD). A static
        (materialized) parameter is turned time-varying: the `Keyframes`
        element is synthesized, and so is the `IsTimeVarying` flag where
        Premiere writes one (every class but the audio scalars). Premiere also
        stamps a session `Timestamp` on such a param, which py does not
        reproduce (it is not deterministic).
        """
        if self.class_id not in _SCALAR_VALUE_CLASSES:
            raise ValueError("keyframes are only settable on scalar parameters")
        if not keys:
            raise ValueError("at least one keyframe is required")
        for time, value, interpolation in keys:
            validate_time(time)
            validate_float32(value)
            if not isinstance(interpolation, KeyframeInterpolation):
                raise TypeError("interpolation must be a KeyframeInterpolation")
        ordered = sorted(keys, key=lambda key: key[0].ticks)
        for previous, following in zip(ordered, ordered[1:]):
            if previous[0].ticks == following[0].ticks:
                # Two keys at one time divide by a zero interval below.
                raise ValueError(f"duplicate keyframe time {previous[0].ticks}")
        # Build the payload before touching the element: everything above can
        # still refuse, and a half-written param (IsTimeVarying set, no
        # keyframes) is a shape Premiere never produces.
        text = _build_keyframe_string(ordered)
        element = self._element.find("Keyframes")
        if element is None:
            start = self._element.find("StartKeyframe")
            if start is None:
                raise ValueError("parameter has no stored value to keyframe")
            flag = self._element.find("IsTimeVarying")
            if self.class_id == _AUDIO_SCALAR_CLASS:
                # Premiere DELETES the audio flag when keyframing (compare
                # 10_audio_volume, which carries it, to 63_audio_keyframes,
                # which does not) and relies on the populated `Keyframes`
                # list (see `is_time_varying`).
                if flag is not None:
                    remove_child(self._element, flag)
            elif flag is not None:
                flag.text = "true"
            else:
                # Every other class carries the flag on every keyframed
                # instance in the corpus, so it is created.
                self._create_time_varying_flag()
            # Audio params order `Keyframes` after `CurrentValue`
            # (63_audio_keyframes); video params have no `CurrentValue` and
            # put it right after `StartKeyframe`.
            anchor = self._element.find("CurrentValue")
            if anchor is None:
                anchor = start
            element = ET.Element("Keyframes")
            element.tail = anchor.tail
            self._element.insert(list(self._element).index(anchor) + 1, element)
        element.text = text

    def _create_time_varying_flag(self) -> None:
        # Premiere writes `IsTimeVarying` on every keyframed param and elides
        # it while static, so a param being keyframed for the first time needs
        # the element created. Across the corpus it always sits immediately
        # after `Name` - or after `ParameterControlType` where that follows it.
        tags = [child.tag for child in self._element]
        for anchor in ("ParameterControlType", "Name"):
            if anchor in tags:
                position = tags.index(anchor) + 1
                break
        else:
            raise ValueError("parameter has no anchor to create <IsTimeVarying>")
        flag = ET.Element("IsTimeVarying")
        flag.text = "true"
        insert_before(self._element, tags[position], flag)

    def __repr__(self) -> str:
        return f"ComponentParam(display_name={self.display_name!r})"


class Component:
    """An effect (or intrinsic transform) applied to a track item.

    Only materialized components appear: Premiere synthesizes untouched
    intrinsics (Motion, Opacity, ...) at runtime and stores nothing for
    them.
    """

    def __init__(self, _element: ET.Element, track_item: TrackItem) -> None:
        self._element = _element
        self.track_item = track_item
        self._properties: list[ComponentParam] = []
        self._sub_components: list[Component] = []

    @property
    def display_name(self) -> str:
        """The component name. Read-only."""
        inner = self._inner()
        return (inner.findtext("DisplayName") or "") if inner is not None else ""

    @property
    def match_name(self) -> str:
        """The registry identifier. Read-only.

        Video components store this as `MatchName`; audio filter components
        store it as `FilterMatchName` (e.g. `Internal Volume Mono`), which
        ExtendScript also reports as the match name.
        """
        match = self._element.findtext("MatchName")
        if match:
            return match
        return self._element.findtext("FilterMatchName") or ""

    @property
    def properties(self) -> NamedList[ComponentParam]:
        """The component's parameters, indexable by name. Read-only."""
        return NamedList(self._properties, keys=("display_name",))

    def __iter__(self) -> Iterator[ComponentParam]:
        return iter(self._properties)

    def __len__(self) -> int:
        return len(self._properties)

    def __getitem__(self, key: int | str) -> ComponentParam:
        return self.properties[key]

    def __contains__(self, item: object) -> bool:
        return item in self.properties

    @property
    def sub_components(self) -> NamedList[Component]:
        """Components nested inside this one. Read-only.

        Masks applied to an effect are stored this way - each is itself a
        component (match name `AE.ADBE AEMask2`) whose parameters carry the
        feather, opacity, expansion and shape. ExtendScript exposes no
        equivalent, so this is XML-only.
        """
        return NamedList(self._sub_components, keys=("display_name", "match_name"))

    def add_mask(self) -> Component:
        """Attach a default mask to this effect and return it.

        Synthesizes the `AEMask2` sub-component exactly as 26_effect_mask
        stores it: the 27 default parameters, wired as a `SubComponents`
        entry beside the effect's `MatchName`. Masks number themselves by
        position (`01`, `02`, ...), and from the second one on the effect
        carries a `NextComponentNumber` allocation counter (76_two_masks).
        Adjust the geometry through the returned component's parameters
        (`Position`, `Anchor Point`, `Feather`, ...); use `path` on the
        Path parameter for drawn shapes.
        """
        if self._element.find("MatchName") is None:
            raise ValueError("component has no MatchName to anchor the mask")
        document = self.track_item.track.sequence.project._document
        subs = self._element.find("SubComponents")
        count = 0 if subs is None else len(subs.findall("SubComponent"))
        param_ids = build_mask_params(document)
        mask_id, mask_element = build_mask_component(
            document, param_ids, clip_role=False, instance_name=f"{count + 1:02d}"
        )
        attach_sub_mask(self._element, mask_id)
        if count:
            _stamp_next_component_number(self._inner(), count + 1)
        mask = _wrap_mask(document, mask_element, self.track_item, param_ids)
        self._sub_components.append(mask)
        return mask

    def _inner(self) -> ET.Element | None:
        inner = self._element.find("Component")
        if inner is None:
            # Audio components nest one level deeper.
            inner = self._element.find("AudioComponent/Component")
        return inner

    def __repr__(self) -> str:
        return (
            f"Component(display_name={self.display_name!r}, "
            f"{len(self._properties)} param(s))"
        )


def _wrap_mask(
    document: PremiereDocument,
    mask_element: ET.Element,
    track_item: TrackItem,
    param_ids: list[str],
) -> Component:
    # Model wrapper for a freshly synthesized mask, mirroring what the
    # parser builds from the same elements.
    mask = Component(mask_element, track_item)
    for param_id in param_ids:
        element = document.by_object_id[param_id]
        mask._properties.append(ComponentParam(element, document, mask))
    return mask


def _stamp_next_component_number(inner: ET.Element | None, value: int) -> None:
    # The allocation counter a mask holder carries once numbering starts:
    # absent beside a single effect mask, stamped 2 by the second
    # (76_two_masks); the clip role's intrinsic holder starts at 2 and
    # counts on from there. Leads the property bag when one exists.
    if inner is None:
        raise ValueError("component has no inner Component element")
    key = inner.find("Node/Properties/BE.VideoFilterComponent.NextComponentNumber")
    if key is not None:
        key.text = str(value)
        return
    properties = inner.find("Node/Properties")
    if properties is not None:
        key = ET.Element("BE.VideoFilterComponent.NextComponentNumber")
        key.text = str(value)
        key.tail = properties.text
        properties.insert(0, key)
        return
    node = ET.Element("Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = inner.text
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    key = ET.SubElement(properties, "BE.VideoFilterComponent.NextComponentNumber")
    key.text = str(value)
    key.tail = "\n\t\t\t\t"
    inner.insert(0, node)
