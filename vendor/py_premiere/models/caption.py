"""The `Caption` and `CaptionTrack` models."""

from __future__ import annotations

import base64
import struct
import uuid
from typing import TYPE_CHECKING

from ..enums import CaptionFormat
from .caption_builder import write_track_format
from .color import Color
from .time import Time
from .validators import (
    FLOAT32_MAX,
    validate_color,
    validate_enum,
    validate_number,
    validate_positive_number,
    validate_string,
)

_validate_caption_text = validate_string(allow_empty=False)
_validate_caption_format = validate_enum(CaptionFormat)

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET
    from typing import Callable

    from ..xml import PremiereDocument
    from .sequence import Sequence


#: Whitespace real caption and title text carries. `str.isprintable()` calls
#: all three unprintable, so testing it alone throws away exactly the text
#: these decoders exist to read.
_TEXT_WHITESPACE = "\n\r\t"


def is_payload_text(value: str) -> bool:
    """Whether a string decoded out of a binary payload looks like text.

    Printable characters plus the line breaks and tabs a multi-line caption
    or title is made of; anything else means the bytes were not text.
    """
    return bool(value) and all(
        character.isprintable() or character in _TEXT_WHITESPACE for character in value
    )


def _last_string_span(payload: bytes) -> tuple[int, int] | None:
    # (offset of the u32 length word, string byte length) of the buffer's
    # LAST length-prefixed UTF-8 string - the caption text; everything
    # after its padding is text-independent.
    #
    # Only positions an offset word actually references count: a field
    # write can synthesize the same SHAPE out of loose values (a ubyte
    # enable flag's `01 00 00 00` reads as a one-char length right where
    # the next field's offset byte lands in the printable range), and a
    # raw scan then mistakes that for text.
    if len(payload) < _FB_START + 4:
        return None
    found = None
    for offset in sorted(
        {target for _, kind, target in _collect_offset_words(payload) if kind == "u32"}
    ):
        text = _last_string_at(payload, offset)
        if text is not None:
            found = (offset, len(text.encode("utf-8")))
    return found


def decode_caption_text(payload: bytes) -> str:
    """Pull the caption text out of a `FormattedTextData` payload.

    The payload is a FlatBuffer holding the styled text: a magic, then the
    parameter names, the font name and finally the text itself, each stored
    the FlatBuffer way as a `uint32` length followed by its bytes and a
    terminator. py reads the LAST such string, which is the text in every
    caption Premiere has been observed to write - the styling that precedes
    it is not decoded.
    """
    span = _last_string_span(payload)
    if span is None:
        return ""
    offset, length = span
    return payload[offset + 4 : offset + 4 + length].decode("utf-8")


#: The FlatBuffer proper starts after the u64 byte-length prefix and the
#: u32 `0x11223344` magic.
_FB_START = 12

# The `FormattedTextData` schema, mapped slot by slot off 29_captions,
# 64_caption_style, 80_subtitle_font_size_75 and 66_eg_text (the walker
# that produced it infers each field's width from the packed vtable
# layout, so the sub-word slots below are real `ubyte`s, not truncated
# ints). Root table: one slot -> the document table.
#
# Document table (45 slots; 19 present in a caption, 6 in a graphic):
#   [ 0] -> vector of RUN tables          [ 1] -> vector of font names
#   [ 4] u32 = 2                          [ 5] u32 = 2
#   [10] -> 3x ubyte table (0,0,0)        [11] ubyte = 1
#   [12] f32 = 100  <- BASE font size     [14] f32 = 3
#   [15] f32 = 6                          [16] f32 = 12
#   [17] -> 3x ubyte table (0,0,0)        [19] f32 = 0
#   [20] f32 = 10                         [32] -> vector -> 'AnimationType'
#   [38] ubyte = 1                        [40] -> empty table
#   [43] ubyte = 0                        [44] ubyte = 1
# Run table: [0] -> the text string, [1] -> its style table.
# Style table (25 slots; 6 present):
#   [ 1] f32 = 48  <- font size OVERRIDE (absent when it equals the base)
#   [ 4] -> 3x ubyte table (0,0,0)        [ 6] f32 = 0
#   [21] -> empty table                   [23] -> empty table
#   [24] u32 = 2
#
# NAMED by 82_caption_style_sweep, where each panel property was set to a
# distinctive sentinel so the slot that changed identifies itself:
#   style[1] font size (75)      style[2] fill colour (11,22,33)
#   style[4] stroke colour       style[6] stroke width (7)
#            (44,55,66)          style[8] tracking (17)
#   doc[ 6] leading (23)         doc[10] shadow colour (77,88,99)
#   doc[12] shadow opacity (41)
# That fixture also CORRECTED an earlier misreading: doc[12] was taken for
# the block's base font size because it read 100 in every default payload,
# but the sweep set it to 41 - it is the shadow opacity, and the size a
# run without an override renders at is the constant below.
# The sweep's remaining numbers were attributed from the Properties
# panel itself: Shadow reads opacity/angle/distance/size/blur =
# 41/45/46/47/48 -> doc[12..16], and Background reads
# opacity/size/corner radius = 42/43/44 -> doc[19], doc[20], doc[34];
# the two colour tables fall with their groups (doc[10] shadow,
# doc[17] background).
# The GROUP ENABLE flags - the Appearance panel's checkboxes - are the
# ubyte in the slot right after the group's colour, named by the
# maintainer's Properties-panel comparison of a py-styled caption against
# Premiere's own: doc[11] shadow, doc[18] background, style[5] stroke
# (fill has none; neither payload carries a style[3]). A group's stored
# values do not render while its flag is absent or 0.
# STILL UNNAMED: the stroke-alignment popup (one of style[12]/[13];
# Premiere writes 1 and 2), doc[26]/[27]/[43]/[44], style[14..16], and
# style[21]/[23]'s empty tables. Their values are all 0/1/2, so nothing
# distinguishes one from another.

#: Document-table slots: the run vector and the font-name vector.
_RUNS_SLOT = 0
_FONT_SLOT = 1
#: A run table's slot holding its style table.
_RUN_STYLE_SLOT = 1

#: Style-table (per-run) slots, named by their sentinel values in
#: 82_caption_style_sweep.
_STYLE_SIZE_SLOT = 1
_STYLE_FILL_SLOT = 2
_STYLE_STROKE_COLOR_SLOT = 4
_STYLE_STROKE_ENABLED_SLOT = 5
_STYLE_STROKE_WIDTH_SLOT = 6
_STYLE_TRACKING_SLOT = 8
#: Document-table (per-block) slots, likewise. The Appearance panel
#: groups these as Shadow (colour, opacity, angle, distance, size, blur)
#: and Background (colour, opacity, size, corner radius), and the slot
#: order follows the panel's grouping - control names confirmed by the
#: maintainer against the Properties panel for fixture 82.
_BLOCK_LEADING_SLOT = 6
_BLOCK_SHADOW_COLOR_SLOT = 10
_BLOCK_SHADOW_OPACITY_SLOT = 12
_BLOCK_SHADOW_ANGLE_SLOT = 13
_BLOCK_SHADOW_DISTANCE_SLOT = 14
_BLOCK_SHADOW_SIZE_SLOT = 15
_BLOCK_SHADOW_BLUR_SLOT = 16
_BLOCK_BACKGROUND_COLOR_SLOT = 17
_BLOCK_BACKGROUND_ENABLED_SLOT = 18
_BLOCK_BACKGROUND_OPACITY_SLOT = 19
_BLOCK_BACKGROUND_SIZE_SLOT = 20
_BLOCK_BACKGROUND_CORNER_SLOT = 34


#: The size a run with no override renders at (64_caption_style: setting
#: the panel to exactly 100 made Premiere drop the field).
_DEFAULT_FONT_SIZE = 100.0


def _unpack(fmt: str, payload: bytes | bytearray, offset: int) -> tuple:
    """`struct.unpack_from` that refuses to read past the payload.

    Every offset in a FlatBuffer comes FROM the buffer, so a truncated or
    corrupt payload steers these reads anywhere; `struct.error` escaping a
    property read is not something a caller can act on, so the whole family
    reports a malformed payload as `ValueError` instead.
    """
    size = struct.calcsize(fmt)
    if offset < 0 or offset + size > len(payload):
        raise ValueError(
            f"malformed caption payload: cannot read {size} bytes at {offset} "
            f"of {len(payload)}"
        )
    return struct.unpack_from(fmt, payload, offset)


def _pack_float32(value: float) -> bytes:
    """A float32 for splicing in, with the range check `_pack_into` applies.

    The paths that GROW the payload build their bytes here rather than through
    `_pack_into`, so they have to reject an out-of-range value the same way -
    otherwise the same public setter raises `ValueError` on one payload shape
    and `OverflowError` from `struct` on another.
    """
    if abs(value) > FLOAT32_MAX:
        raise ValueError(f"value out of float32 range: {value}")
    return struct.pack("<f", value)


def _pack_into(fmt: str, buffer: bytearray, offset: int, *values: object) -> None:
    """`struct.pack_into` with the same bound (and float32 range) check."""
    size = struct.calcsize(fmt)
    if offset < 0 or offset + size > len(buffer):
        raise ValueError(
            f"malformed caption payload: cannot write {size} bytes at {offset} "
            f"of {len(buffer)}"
        )
    if fmt.endswith("f"):
        for value in values:
            if isinstance(value, float) and abs(value) > FLOAT32_MAX:
                raise ValueError(f"value out of float32 range: {value}")
    struct.pack_into(fmt, buffer, offset, *values)


def _valid_vtable(payload: bytes, position: int) -> int | None:
    # The vtable position of a table at `position`, or None when the bytes
    # there cannot be a table (used to classify field targets while
    # walking; strings and scalars fail the shape checks).
    #
    # The alignment gates decide classification, not just safety: a table
    # starts with an `int32` soffset (4-aligned) and a vtable is a `uint16`
    # array (2-aligned) in every FlatBuffers writer's output. Without them
    # a run vector whose count is 1 also SHAPE-checks as a table - the
    # count reads as an soffset onto a fake vtable at an odd position -
    # but only once enough edits grow the buffer for that fake vtable's
    # garbage length to fit inside it. The walk then drops the whole
    # subtree behind the vector, and the next splice severs it.
    if position % 4 or position + 4 > len(payload):
        return None
    (soffset,) = _unpack("<i", payload, position)
    vtable = position - soffset
    if vtable < 0 or vtable % 2 or vtable + 4 > len(payload):
        return None
    (vtable_length,) = _unpack("<H", payload, vtable)
    if vtable_length < 4 or vtable_length % 2 or vtable + vtable_length > len(payload):
        return None
    return int(vtable)


def _collect_offset_words(payload: bytes) -> list[tuple[int, str, int]]:
    # Every offset word in the buffer: `(word position, kind, target)`,
    # kind `u32` for a forward field/vector/root offset and `i32` for a
    # table's signed vtable offset. Found by walking from the root, plus a
    # scan for ORPHAN tables - Premiere's writer emits empty tables that
    # nothing references (one sits beside the referenced copy in every
    # caption block, sharing its vtable), and those still have to shift.
    # Validated byte-exactly against Premiere's own payload pairs.
    words: list[tuple[int, str, int]] = [
        (_FB_START, "u32", _FB_START + _unpack("<I", payload, _FB_START)[0])
    ]
    seen: set[int] = set()
    vtables: set[int] = set()

    def walk(position: int) -> None:
        if position in seen:
            return
        seen.add(position)
        vtable = _valid_vtable(payload, position)
        if vtable is None:
            return
        words.append((position, "i32", vtable))
        vtables.add(vtable)
        (vtable_length,) = _unpack("<H", payload, vtable)
        (object_size,) = _unpack("<H", payload, vtable + 2)
        slots = [
            relative
            for index in range((vtable_length - 4) // 2)
            for (relative,) in [_unpack("<H", payload, vtable + 4 + index * 2)]
            if relative
        ]
        # A field only has room for a 32-bit offset if the next field in the
        # table starts at least 4 bytes later - FlatBuffers never overlaps two
        # fields. Without this, a colour table's four adjacent `ubyte` channels
        # each read as an offset word, and a later splice rewrites the bytes.
        edges = sorted(set(slots)) + [object_size]
        width = {edge: edges[index + 1] - edge for index, edge in enumerate(edges[:-1])}
        for relative in slots:
            if width.get(relative, 0) < 4:
                continue
            field = position + relative
            if field + 4 > len(payload):
                continue
            (value,) = _unpack("<I", payload, field)
            # `target + 4 <= len`: every classification below starts by
            # reading a u32 there, and the vector branch has no other guard.
            if not 0 < value <= len(payload) - field - 4:
                continue
            target = field + value
            if _last_string_at(payload, target) is not None:
                words.append((field, "u32", target))
            elif _valid_vtable(payload, target) is not None:
                words.append((field, "u32", target))
                walk(target)
            else:
                (count,) = _unpack("<I", payload, target)
                if not 0 < count <= 64 or target + 4 + count * 4 > len(payload):
                    continue
                words.append((field, "u32", target))
                for item in range(count):
                    slot = target + 4 + item * 4
                    (offset,) = _unpack("<I", payload, slot)
                    item_target = slot + offset
                    if not 0 < offset < len(payload) - slot:
                        continue
                    if _last_string_at(payload, item_target) is not None:
                        words.append((slot, "u32", item_target))
                    elif _valid_vtable(payload, item_target) is not None:
                        words.append((slot, "u32", item_target))
                        walk(item_target)

    walk(words[0][2])
    known = {position for position, _, _ in words}
    for position in range(_FB_START, len(payload) - 4, 4):
        if position in known:
            continue
        (value,) = _unpack("<i", payload, position)
        if value < 0 and position - value in vtables:
            words.append((position, "i32", position - value))
    return words


def _last_string_at(payload: bytes, position: int) -> str | None:
    if position + 4 > len(payload):
        return None
    (length,) = _unpack("<I", payload, position)
    end = position + 4 + length
    if not length or end >= len(payload) or payload[end] != 0:
        return None
    try:
        text = payload[position + 4 : end].decode("utf-8")
    except UnicodeDecodeError:
        return None
    return text if is_payload_text(text) else None


def _splice_bytes(payload: bytes, start: int, end: int, replacement: bytes) -> bytes:
    """Replace `payload[start:end]`, re-deriving every stored offset.

    The one primitive behind every payload edit: each offset word records
    a distance between two points in the buffer, so after moving the tail
    the correct value is simply recomputed from where those points landed.
    Bytes at or after `end` shift; the splice point and everything before
    it stay put.
    """
    delta = len(replacement) - (end - start)

    def shifted(position: int) -> int:
        return position + delta if position >= end else position

    words = _collect_offset_words(payload)
    buffer = bytearray(payload[:start] + replacement + payload[end:])
    (prefix,) = _unpack("<Q", buffer, 0)
    _pack_into("<Q", buffer, 0, prefix + delta)
    for position, kind, target in words:
        if kind == "u32":
            _pack_into(
                "<I", buffer, shifted(position), shifted(target) - shifted(position)
            )
        else:
            _pack_into(
                "<i", buffer, shifted(position), shifted(position) - shifted(target)
            )
    return bytes(buffer)


def _splice_string(payload: bytes, offset: int, length: int, text: str) -> bytes:
    # Replace the length-prefixed padded string at `offset`.
    span_end = offset + 4 + (length + 4) // 4 * 4
    encoded = text.encode("utf-8")
    padded = (len(encoded) + 4) // 4 * 4
    replacement = struct.pack("<I", len(encoded)) + encoded
    replacement += b"\0" * (padded - len(encoded))
    return _splice_bytes(payload, offset, span_end, replacement)


def replace_payload_text(payload: bytes, text: str) -> bytes:
    """The payload with its text string replaced by `text`.

    Rebuilding one caption block's payload from another's this way is
    byte-identical to what Premiere stored (test_text_payload).
    """
    if not text:
        # The payload identifies its text as the LAST length-prefixed
        # string; a zero-length one is indistinguishable from absent, and
        # reads back as the font name that precedes it.
        raise ValueError("caption text cannot be empty")
    span = _last_string_span(payload)
    if span is None:
        raise ValueError("payload holds no caption text")
    return _splice_string(payload, span[0], span[1], text)


def _document_table(payload: bytes) -> int:
    # The buffer's root table holds one field: the document table.
    root = _FB_START + int(_unpack("<I", payload, _FB_START)[0])
    vtable = _valid_vtable(payload, root)
    if vtable is None:
        raise ValueError("payload has no root table")
    field = root + int(_unpack("<H", payload, vtable + 4)[0])
    return field + int(_unpack("<I", payload, field)[0])


def _font_string_span(payload: bytes) -> tuple[int, int]:
    # Document-table slot 1 is a vector of font names; a caption block and
    # an Essential Graphics text both carry exactly one entry (the family
    # the run is set in).
    document = _document_table(payload)
    vtable = _valid_vtable(payload, document)
    if vtable is None:
        raise ValueError("payload has no document table")
    (relative,) = _unpack("<H", payload, vtable + 4 + _FONT_SLOT * 2)
    if not relative:
        raise ValueError("payload carries no font name")
    field = document + relative
    vector = field + int(_unpack("<I", payload, field)[0])
    (count,) = _unpack("<I", payload, vector)
    if not count:
        # An empty vector is a malformed buffer, not an unsupported one -
        # every caption names a font. Reporting it as `NotImplementedError`
        # leaked a type a caller cannot act on out of a corrupt payload.
        raise ValueError("malformed caption payload: font name vector is empty")
    if count != 1:
        raise NotImplementedError(
            f"payload carries {count} font names; only one is supported"
        )
    slot = vector + 4
    string = slot + _unpack("<I", payload, slot)[0]
    (length,) = _unpack("<I", payload, string)
    return string, length


def _field_offset(payload: bytes, table: int, slot: int) -> int | None:
    # The absolute position of a table's field, or None when the slot is
    # absent (FlatBuffers elides defaults).
    vtable = _valid_vtable(payload, table)
    if vtable is None:
        return None
    (vtable_length,) = _unpack("<H", payload, vtable)
    if slot >= (vtable_length - 4) // 2:
        return None
    (relative,) = _unpack("<H", payload, vtable + 4 + slot * 2)
    return table + relative if relative else None


def _style_table(payload: bytes) -> int | None:
    # The style table of the payload's FIRST run: document slot 0 is the
    # run vector, a run's slot 1 its style.
    document = _document_table(payload)
    field = _field_offset(payload, document, _RUNS_SLOT)
    if field is None:
        return None
    vector = field + int(_unpack("<I", payload, field)[0])
    (count,) = _unpack("<I", payload, vector)
    if not count:
        return None
    run = vector + 4 + int(_unpack("<I", payload, vector + 4)[0])
    style_field = _field_offset(payload, run, _RUN_STYLE_SLOT)
    if style_field is None:
        return None
    return style_field + int(_unpack("<I", payload, style_field)[0])


def _read_float(payload: bytes, table: int | None, slot: int) -> float | None:
    if table is None:
        return None
    field = _field_offset(payload, table, slot)
    if field is None or field + 4 > len(payload):
        return None
    return float(_unpack("<f", payload, field)[0])


def _read_rgb(payload: bytes, table: int | None, slot: int) -> Color | None:
    # A colour is a nested table of three `ubyte` slots.
    if table is None:
        return None
    field = _field_offset(payload, table, slot)
    if field is None or field + 4 > len(payload):
        return None
    nested = field + int(_unpack("<I", payload, field)[0])
    channels = []
    for channel in range(3):
        position = _field_offset(payload, nested, channel)
        if position is None or position >= len(payload):
            return None
        channels.append(payload[position])
    return Color(*channels)


def _add_table_field(payload: bytes, table: int, slot: int, data: bytes) -> bytes:
    """Give `table` a field it lacks, returning the rewritten payload.

    FlatBuffers cannot grow a table in place, and its vtable may be shared
    with other tables, so this appends the value at the END of the object
    (leaving every existing field's relative offset alone) and splices a
    PRIVATE copy of the vtable in just before the table, describing the
    new layout. Both edits go through `_splice_bytes`, which re-derives
    every offset in the buffer.
    """
    vtable = _valid_vtable(payload, table)
    if vtable is None:
        raise ValueError("not a table")
    (vtable_length,) = _unpack("<H", payload, vtable)
    (object_size,) = _unpack("<H", payload, vtable + 2)
    count = (vtable_length - 4) // 2
    entries = [
        _unpack("<H", payload, vtable + 4 + index * 2)[0] for index in range(count)
    ]
    entries += [0] * max(0, slot + 1 - count)
    if entries[slot]:
        raise ValueError("table already carries that field")
    align = len(data)
    field_relative = (object_size + align - 1) // align * align
    entries[slot] = field_relative
    padding = field_relative - object_size

    # 1. The value itself, at the end of the object. The spliced run is
    #    padded to a multiple of 4: a sub-word field would otherwise shift
    #    every table and vtable after it off the 4-byte alignment
    #    FlatBuffers requires, which reads back as a corrupt buffer. The
    #    vtable still declares the field's REAL end, so the trailing pad
    #    sits outside the object exactly as inter-object padding does.
    inserted = padding + len(data)
    grown = _splice_bytes(
        payload,
        table + object_size,
        table + object_size,
        b"\0" * padding + data + b"\0" * (-inserted % 4),
    )

    # 2. A private vtable immediately before the table, padded so the
    #    table stays 4-aligned.
    new_vtable = struct.pack(
        "<HH", 4 + 2 * len(entries), field_relative + len(data)
    ) + b"".join(struct.pack("<H", entry) for entry in entries)
    lead = -len(new_vtable) % 4
    block = b"\0" * lead + new_vtable
    result = bytearray(_splice_bytes(grown, table, table, block))
    moved = table + len(block)
    _pack_into("<i", result, moved, len(new_vtable))
    return bytes(result)


def _write_table_float(payload: bytes, table: int, slot: int, value: float) -> bytes:
    field = _field_offset(payload, table, slot)
    if field is None:
        return _add_table_field(payload, table, slot, _pack_float32(value))
    buffer = bytearray(payload)
    _pack_into("<f", buffer, field, value)
    return bytes(buffer)


def _color_vtable(payload: bytes) -> int | None:
    # Colour tables all share one vtable (three `ubyte` slots); borrow it
    # rather than synthesising one when a payload needs a new colour.
    document = _document_table(payload)
    for table, slot in (
        (document, _BLOCK_SHADOW_COLOR_SLOT),
        (document, _BLOCK_BACKGROUND_COLOR_SLOT),
        (_style_table(payload), _STYLE_STROKE_COLOR_SLOT),
    ):
        if table is None:
            continue
        field = _field_offset(payload, table, slot)
        if field is None:
            continue
        nested = field + int(_unpack("<I", payload, field)[0])
        vtable = _valid_vtable(payload, nested)
        if vtable is not None:
            return vtable
    return None


def _write_table_color(
    payload: bytes,
    locate: Callable[[bytes], int | None],
    slot: int,
    color: Color,
) -> bytes:
    if color.alpha != 255:
        # The stored colour is a nested table of THREE ubyte slots; there is
        # no alpha channel to put it in, so accepting one would silently
        # drop it.
        raise ValueError(
            "caption colours are opaque: the payload stores red, green and "
            f"blue only, so alpha must be 255 (got {color.alpha})"
        )
    # `locate` re-navigates to the table: adding a field splices a new
    # vtable in FRONT of it, so a position held across that edit is stale.
    table = locate(payload)
    if table is None:
        raise ValueError("payload has no such table")
    field = _field_offset(payload, table, slot)
    if field is not None:
        nested = field + int(_unpack("<I", payload, field)[0])
        buffer = bytearray(payload)
        for channel, value in enumerate((color.red, color.green, color.blue)):
            position = _field_offset(payload, nested, channel)
            if position is None:
                raise NotImplementedError("colour table is missing a channel")
            buffer[position] = value
        return bytes(buffer)

    vtable = _color_vtable(payload)
    if vtable is None:
        raise NotImplementedError("payload carries no colour table to model")
    # Append the new colour table at the very end, so nothing shifts, then
    # point a fresh field at it (which does shift, but not past the end).
    (object_size,) = _unpack("<H", payload, vtable + 2)
    body = bytearray(b"\0" * object_size)
    _pack_into("<i", body, 0, len(payload) - vtable)
    for channel, value in enumerate((color.red, color.green, color.blue)):
        relative = _unpack("<H", payload, vtable + 4 + channel * 2)[0]
        body[relative] = value
    appended = _splice_bytes(payload, len(payload), len(payload), bytes(body))
    with_field = bytearray(
        _add_table_field(appended, locate(appended) or 0, slot, struct.pack("<I", 0))
    )
    moved = locate(bytes(with_field))
    field = None if moved is None else _field_offset(bytes(with_field), moved, slot)
    if field is None:
        raise ValueError("failed to add the colour field")
    target = len(with_field) - object_size
    _pack_into("<I", with_field, field, target - field)
    return bytes(with_field)


def _write_table_flag(
    payload: bytes, locate: Callable[[bytes], int | None], slot: int
) -> bytes:
    # A group's ubyte enable flag - the Appearance panel checkbox, stored
    # in the slot right after the group's colour. Premiere renders a
    # group's values only while its flag is 1, so the setters raise it
    # whenever they store a value for that group.
    table = locate(payload)
    if table is None:
        raise ValueError("payload has no such table")
    field = _field_offset(payload, table, slot)
    if field is None:
        return _add_table_field(payload, table, slot, b"\x01")
    buffer = bytearray(payload)
    buffer[field] = 1
    return bytes(buffer)


def write_style_float(payload: bytes, slot: int, value: float) -> bytes:
    """Set a float slot of the first run's style table."""
    table = _style_table(payload)
    if table is None:
        raise ValueError("payload has no run style table")
    return _write_table_float(payload, table, slot, value)


def write_style_color(payload: bytes, slot: int, color: Color) -> bytes:
    """Set a colour slot of the first run's style table."""
    return _write_table_color(payload, _style_table, slot, color)


def write_block_float(payload: bytes, slot: int, value: float) -> bytes:
    """Set a float slot of the block-level document table."""
    return _write_table_float(payload, _document_table(payload), slot, value)


def write_block_color(payload: bytes, slot: int, color: Color) -> bytes:
    """Set a colour slot of the block-level document table."""
    return _write_table_color(payload, _document_table, slot, color)


def read_style_float(payload: bytes, slot: int) -> float | None:
    """A float slot of the first run's style table, or `None` if absent."""
    return _read_float(payload, _style_table(payload), slot)


def read_style_color(payload: bytes, slot: int) -> Color | None:
    """A colour slot of the first run's style table, or `None` if absent."""
    return _read_rgb(payload, _style_table(payload), slot)


def read_block_float(payload: bytes, slot: int) -> float | None:
    """A float slot of the block-level document table."""
    return _read_float(payload, _document_table(payload), slot)


def read_block_color(payload: bytes, slot: int) -> Color | None:
    """A colour slot of the block-level document table."""
    return _read_rgb(payload, _document_table(payload), slot)


def read_font_family(payload: bytes) -> str:
    """The font family the payload's text is set in."""
    offset, length = _font_string_span(payload)
    return payload[offset + 4 : offset + 4 + length].decode("utf-8")


def write_font_family(payload: bytes, name: str) -> bytes:
    """The payload with its font family replaced by `name`.

    The family is a string in the document table's font vector - the one
    style field that names itself, so it needs no fixture to identify
    (unlike the colour and spacing slots; see the module map).
    """
    if not name:
        raise ValueError("font family cannot be empty")
    offset, length = _font_string_span(payload)
    return _splice_string(payload, offset, length, name)


#: Namespace for the content-derived `BinaryHash` of a rewritten payload.
_PAYLOAD_NAMESPACE = uuid.UUID("6f0f2f24-2d8e-5a3a-9a0f-0f4a2f0d9c11")


def write_payload_element(
    document: PremiereDocument, element: ET.Element, payload: bytes
) -> None:
    """Store a rewritten payload on its element under a fresh hash.

    Premiere gives an edited block its own payload copy and refreshes
    hashes on open (80_subtitle_font_size_75); a block that is the STORED
    copy of a hash other elements still reference cannot be rewritten
    without orphaning them, so that refuses.
    """
    old_hash = element.get("BinaryHash")
    if (element.text or "").strip() and old_hash:
        for other in document.root.iter():
            if (
                other is not element
                and other.get("BinaryHash") == old_hash
                and not (other.text or "").strip()
            ):
                raise NotImplementedError(
                    "this element carries the stored payload others "
                    "reference; rewriting it is not supported yet"
                )
    # Derived from the payload, not random: Premiere refreshes these on open
    # anyway, so the value is free - and deriving it makes the same edit
    # produce the same bytes twice, which a random GUID does not. (A marker
    # GUID stays random because there it IS the identity, not a digest.)
    element.set("BinaryHash", str(uuid.uuid5(_PAYLOAD_NAMESPACE, payload.hex())))
    element.text = base64.b64encode(payload).decode("ascii") + "\n\t\t"
    document._by_binary_hash = None


#: Head offsets of the FormattedTextData fields the font-size forms touch,
#: constant across the payload family (the head precedes the strings):
#: the block-level BASE font size, and the four backward relative-offset
#: words that cross the run struct and shift when its override field comes
#: or goes (29_captions vs 64_caption_style, re-proven by
#: 80_subtitle_font_size_75).
_BASE_SIZE_OFFSET = 0xB8
_CROSSING_WORDS = (0xCC, 0xD0, 0x100, 0x108)
#: Tail-relative offsets (from the end of the padded text string): the run
#: vtable's object-size word, its font-size field slot, the three FORWARD
#: offset words whose targets sit past the override, and the override
#: float itself.
_TAIL_OBJECT_SIZE = 4
_TAIL_SIZE_SLOT = 8
_TAIL_FORWARD_WORDS = (64, 68, 76)
_TAIL_SIZE_FLOAT = 80
#: The run struct's object sizes with and without the override field, and
#: the override's offset within the struct (its vtable slot value).
_RUN_WITH_OVERRIDE = 0x1C
_RUN_WITHOUT_OVERRIDE = 0x18
_SIZE_FIELD_OFFSET = 0x18


def _has_default_run_layout(payload: bytes) -> bool:
    # True for the as-imported payload family, whose run style table sits
    # immediately after the text and holds only the size override. The
    # add/drop-the-override arithmetic is byte-verified against exactly
    # that shape (29/64/80); anything else goes through the generic
    # field writer.
    style = _style_table(payload)
    if style is None:
        return False
    vtable = _valid_vtable(payload, style)
    if vtable is None or vtable != _tail_offset(payload) + 2:
        return False
    (object_size,) = _unpack("<H", payload, vtable + 2)
    if object_size not in (_RUN_WITH_OVERRIDE, _RUN_WITHOUT_OVERRIDE):
        return False
    # Everything above is RELATIVE, so it still holds after a field was added
    # ahead of the run - but `_CROSSING_WORDS` are ABSOLUTE positions. Adding
    # a style or document field shifts the head and leaves them pointing at
    # unrelated bytes, so confirm they are all still offset words.
    positions = {position for position, _, _ in _collect_offset_words(payload)}
    return not set(_CROSSING_WORDS) - positions


def _tail_offset(payload: bytes) -> int:
    span = _last_string_span(payload)
    if span is None:
        raise ValueError("payload holds no caption text")
    offset, length = span
    return offset + 4 + (length + 4) // 4 * 4


def read_font_size(payload: bytes) -> float:
    """The effective font size of a `FormattedTextData` payload.

    The run's style table carries an inline float32 OVERRIDE (48.0 on
    every as-imported caption); a run without one renders at the default
    100 - which is what 64_caption_style proved, where setting the panel
    to exactly 100 made Premiere DROP the field.
    """
    value = read_style_float(payload, _STYLE_SIZE_SLOT)
    return _DEFAULT_FONT_SIZE if value is None else value


def write_font_size(payload: bytes, value: float) -> bytes:
    """The payload with its font size set to `value`.

    A value equal to the block base (100.0) drops the run override the way
    Premiere stores it (64_caption_style); any other value lands as the
    inline override float (80_subtitle_font_size_75 is byte-identical to
    the as-imported payload but for that float). Dropping or restoring the
    field resizes the run struct and shifts the four backward offsets that
    cross it.
    """
    if not _has_default_run_layout(payload):
        # Off the default-styled family the offset arithmetic below does
        # not apply, so go through the generic field writer. It cannot
        # DROP the override the way Premiere does at the default size, so
        # the size is stored explicitly - which reads back the same.
        return write_style_float(payload, _STYLE_SIZE_SLOT, value)
    tail = _tail_offset(payload)
    (object_size,) = _unpack("<H", payload, tail + _TAIL_OBJECT_SIZE)
    base = _DEFAULT_FONT_SIZE
    buffer = bytearray(payload)
    if object_size == _RUN_WITH_OVERRIDE:
        if value != base:
            _pack_into("<f", buffer, tail + _TAIL_SIZE_FLOAT, value)
            return bytes(buffer)
        (length,) = _unpack("<Q", buffer, 0)
        _pack_into("<Q", buffer, 0, length - 4)
        for position in _CROSSING_WORDS:
            (word,) = _unpack("<I", buffer, position)
            _pack_into("<I", buffer, position, (word + 4) & 0xFFFFFFFF)
        for offset in _TAIL_FORWARD_WORDS:
            (word,) = _unpack("<I", buffer, tail + offset)
            _pack_into("<I", buffer, tail + offset, word - 4)
        _pack_into("<H", buffer, tail + _TAIL_OBJECT_SIZE, _RUN_WITHOUT_OVERRIDE)
        _pack_into("<H", buffer, tail + _TAIL_SIZE_SLOT, 0)
        del buffer[tail + _TAIL_SIZE_FLOAT : tail + _TAIL_SIZE_FLOAT + 4]
        return bytes(buffer)
    if object_size == _RUN_WITHOUT_OVERRIDE:
        if value == base:
            return bytes(buffer)
        (length,) = _unpack("<Q", buffer, 0)
        _pack_into("<Q", buffer, 0, length + 4)
        for position in _CROSSING_WORDS:
            (word,) = _unpack("<I", buffer, position)
            _pack_into("<I", buffer, position, (word - 4) & 0xFFFFFFFF)
        for offset in _TAIL_FORWARD_WORDS:
            (word,) = _unpack("<I", buffer, tail + offset)
            _pack_into("<I", buffer, tail + offset, word + 4)
        _pack_into("<H", buffer, tail + _TAIL_OBJECT_SIZE, _RUN_WITH_OVERRIDE)
        _pack_into("<H", buffer, tail + _TAIL_SIZE_SLOT, _SIZE_FIELD_OFFSET)
        buffer[tail + _TAIL_SIZE_FLOAT : tail + _TAIL_SIZE_FLOAT] = _pack_float32(value)
        return bytes(buffer)
    raise ValueError(f"unknown caption run layout (object size {object_size:#x})")


class Caption:
    """One caption on a caption track.

    Premiere stores a caption twice: as a track item carrying its
    frame-aligned placement on the timeline, and inside the imported caption
    stream carrying the source times it was authored with. `start` and `end`
    are the timeline ones; `source_start` and `source_end` are the stream's.
    """

    def __init__(self, _element: ET.Element, _text: str, track: CaptionTrack) -> None:
        self._element = _element
        self._text = _text
        self.track = track
        self._source_start = Time(0)
        self._source_end = Time(0)

    @property
    def text(self) -> str:
        """The caption text. Read/write.

        Setting it splices the new string into the styled-text payload,
        preserving the styling around it, and gives this caption's block
        its own payload copy under a fresh hash - what Premiere's own edit
        does. Empty text is refused: the format identifies its text as the
        payload's last string, so an empty one cannot be read back.
        """
        return self._text

    @text.setter
    def text(self, value: str) -> None:
        _validate_caption_text(value)
        self._write_payload(replace_payload_text(self._payload(), value))
        self._text = value

    @property
    def start(self) -> Time:
        """The start on the sequence timeline. Read-only."""
        return Time(int(self._element.findtext(_TRACK_ITEM + "Start") or 0))

    @property
    def end(self) -> Time:
        """The end on the sequence timeline. Read-only."""
        return Time(int(self._element.findtext(_TRACK_ITEM + "End") or 0))

    @property
    def source_start(self) -> Time:
        """The start as the imported caption stream states it. Read-only.

        Unlike `start`, this is not rounded to a frame boundary.
        """
        return self._source_start

    @property
    def source_end(self) -> Time:
        """The end as the imported caption stream states it. Read-only."""
        return self._source_end

    def _text_data(self) -> ET.Element | None:
        # The timeline block's FormattedTextData, through
        # BlockVector -> Block.
        document = self.track.sequence.project._document
        reference = self._element.find("BlockVector/BlockVectorItem")
        if reference is None:
            return None
        return document.resolve(reference).find("FormattedTextData")

    @property
    def font_size(self) -> float:
        """The caption's font size, as the captions panel shows it.
        Read/write.

        Stored inside the styled-text payload: every as-imported caption
        carries a per-run override (48.0), and a caption styled to the
        block-level base value (100.0) stores no override at all - decoded
        against 29_captions, 64_caption_style and
        80_subtitle_font_size_75, with the panel values confirmed by hand.

        Setting a size gives this caption's timeline block its own payload
        copy under a fresh hash, exactly as Premiere's own edit does; the
        imported stream's copy keeps its style, and Premiere refreshes the
        hash on open.
        """
        return read_font_size(self._payload())

    @font_size.setter
    def font_size(self, value: float) -> None:
        validate_positive_number(value)
        payload = self._payload()
        new_payload = write_font_size(payload, float(value))
        if new_payload != payload:
            self._write_payload(new_payload)

    def _style_float(self, slot: int) -> float | None:
        return read_style_float(self._payload(), slot)

    def _set_style_float(
        self, slot: int, value: float, enable: int | None = None
    ) -> None:
        validate_number(value)
        payload = write_style_float(self._payload(), slot, float(value))
        if enable is not None:
            payload = _write_table_flag(payload, _style_table, enable)
        self._write_payload(payload)

    def _style_color(self, slot: int) -> Color | None:
        return read_style_color(self._payload(), slot)

    def _set_style_color(
        self, slot: int, value: Color, enable: int | None = None
    ) -> None:
        validate_color(value)
        payload = write_style_color(self._payload(), slot, value)
        if enable is not None:
            payload = _write_table_flag(payload, _style_table, enable)
        self._write_payload(payload)

    def _block_float(self, slot: int) -> float | None:
        return read_block_float(self._payload(), slot)

    def _set_block_float(
        self, slot: int, value: float, enable: int | None = None
    ) -> None:
        validate_number(value)
        payload = write_block_float(self._payload(), slot, float(value))
        if enable is not None:
            payload = _write_table_flag(payload, _document_table, enable)
        self._write_payload(payload)

    def _block_color(self, slot: int) -> Color | None:
        return read_block_color(self._payload(), slot)

    def _set_block_color(
        self, slot: int, value: Color, enable: int | None = None
    ) -> None:
        validate_color(value)
        payload = write_block_color(self._payload(), slot, value)
        if enable is not None:
            payload = _write_table_flag(payload, _document_table, enable)
        self._write_payload(payload)

    @property
    def fill_color(self) -> Color | None:
        """The text's fill colour. Read/write.

        Named from 82_caption_style_sweep, where the panel's fill was set
        to the sentinel `RGB(11, 22, 33)`. Reads `None` when the caption
        has never been styled and stores no colour at all - Premiere then
        renders its format's default; setting one adds the field.
        """
        return self._style_color(_STYLE_FILL_SLOT)

    @fill_color.setter
    def fill_color(self, value: Color) -> None:
        self._set_style_color(_STYLE_FILL_SLOT, value)

    @property
    def stroke_color(self) -> Color | None:
        """The text's stroke (edge) colour. Read/write.

        Setting any stroke property also raises the group's enable flag
        (the Appearance panel's Stroke checkbox) - stored stroke values
        do not render without it.
        """
        return self._style_color(_STYLE_STROKE_COLOR_SLOT)

    @stroke_color.setter
    def stroke_color(self, value: Color) -> None:
        self._set_style_color(
            _STYLE_STROKE_COLOR_SLOT, value, enable=_STYLE_STROKE_ENABLED_SLOT
        )

    @property
    def stroke_width(self) -> float | None:
        """The stroke width. Read/write."""
        return self._style_float(_STYLE_STROKE_WIDTH_SLOT)

    @stroke_width.setter
    def stroke_width(self, value: float) -> None:
        self._set_style_float(
            _STYLE_STROKE_WIDTH_SLOT, value, enable=_STYLE_STROKE_ENABLED_SLOT
        )

    @property
    def tracking(self) -> float | None:
        """The letter tracking. Read/write."""
        return self._style_float(_STYLE_TRACKING_SLOT)

    @tracking.setter
    def tracking(self, value: float) -> None:
        self._set_style_float(_STYLE_TRACKING_SLOT, value)

    @property
    def leading(self) -> float | None:
        """The line leading. Read/write.

        Block-level, unlike the run-level fill and stroke: it applies to
        the whole caption.
        """
        return self._block_float(_BLOCK_LEADING_SLOT)

    @leading.setter
    def leading(self, value: float) -> None:
        self._set_block_float(_BLOCK_LEADING_SLOT, value)

    @property
    def shadow_color(self) -> Color | None:
        """The shadow's colour. Read/write."""
        return self._block_color(_BLOCK_SHADOW_COLOR_SLOT)

    @shadow_color.setter
    def shadow_color(self, value: Color) -> None:
        self._set_block_color(_BLOCK_SHADOW_COLOR_SLOT, value)

    @property
    def shadow_opacity(self) -> float | None:
        """The shadow's opacity. Read/write."""
        return self._block_float(_BLOCK_SHADOW_OPACITY_SLOT)

    @shadow_opacity.setter
    def shadow_opacity(self, value: float) -> None:
        self._set_block_float(_BLOCK_SHADOW_OPACITY_SLOT, value)

    @property
    def shadow_angle(self) -> float | None:
        """The shadow's angle in degrees. Read/write."""
        return self._block_float(_BLOCK_SHADOW_ANGLE_SLOT)

    @shadow_angle.setter
    def shadow_angle(self, value: float) -> None:
        self._set_block_float(_BLOCK_SHADOW_ANGLE_SLOT, value)

    @property
    def shadow_distance(self) -> float | None:
        """The shadow's distance. Read/write."""
        return self._block_float(_BLOCK_SHADOW_DISTANCE_SLOT)

    @shadow_distance.setter
    def shadow_distance(self, value: float) -> None:
        self._set_block_float(_BLOCK_SHADOW_DISTANCE_SLOT, value)

    @property
    def shadow_size(self) -> float | None:
        """The shadow's size. Read/write."""
        return self._block_float(_BLOCK_SHADOW_SIZE_SLOT)

    @shadow_size.setter
    def shadow_size(self, value: float) -> None:
        self._set_block_float(_BLOCK_SHADOW_SIZE_SLOT, value)

    @property
    def shadow_blur(self) -> float | None:
        """The shadow's blur. Read/write."""
        return self._block_float(_BLOCK_SHADOW_BLUR_SLOT)

    @shadow_blur.setter
    def shadow_blur(self, value: float) -> None:
        self._set_block_float(_BLOCK_SHADOW_BLUR_SLOT, value)

    @property
    def background_color(self) -> Color | None:
        """The caption background's colour. Read/write.

        Setting any background property also raises the group's enable
        flag (the Appearance panel's Background checkbox) - stored
        background values do not render without it.
        """
        return self._block_color(_BLOCK_BACKGROUND_COLOR_SLOT)

    @background_color.setter
    def background_color(self, value: Color) -> None:
        self._set_block_color(
            _BLOCK_BACKGROUND_COLOR_SLOT, value, enable=_BLOCK_BACKGROUND_ENABLED_SLOT
        )

    @property
    def background_opacity(self) -> float | None:
        """The background's opacity. Read/write."""
        return self._block_float(_BLOCK_BACKGROUND_OPACITY_SLOT)

    @background_opacity.setter
    def background_opacity(self, value: float) -> None:
        self._set_block_float(
            _BLOCK_BACKGROUND_OPACITY_SLOT, value, enable=_BLOCK_BACKGROUND_ENABLED_SLOT
        )

    @property
    def background_size(self) -> float | None:
        """The background's size (its padding). Read/write."""
        return self._block_float(_BLOCK_BACKGROUND_SIZE_SLOT)

    @background_size.setter
    def background_size(self, value: float) -> None:
        self._set_block_float(
            _BLOCK_BACKGROUND_SIZE_SLOT, value, enable=_BLOCK_BACKGROUND_ENABLED_SLOT
        )

    @property
    def background_corner_radius(self) -> float | None:
        """The background's corner radius. Read/write."""
        return self._block_float(_BLOCK_BACKGROUND_CORNER_SLOT)

    @background_corner_radius.setter
    def background_corner_radius(self, value: float) -> None:
        self._set_block_float(
            _BLOCK_BACKGROUND_CORNER_SLOT, value, enable=_BLOCK_BACKGROUND_ENABLED_SLOT
        )

    @property
    def font_family(self) -> str:
        """The font family the caption is set in. Read/write.

        Stored as a string in the payload's font vector. Premiere resolves
        an unknown family to a fallback on open, so setting one it does
        not have will not stick.
        """
        return read_font_family(self._payload())

    @font_family.setter
    def font_family(self, value: str) -> None:
        _validate_caption_text(value)
        self._write_payload(write_font_family(self._payload(), value))

    def _payload(self) -> bytes:
        document = self.track.sequence.project._document
        data = self._text_data()
        payload = None if data is None else document.payload(data)
        if payload is None:
            raise ValueError("caption has no styled-text payload")
        return payload

    def _write_payload(self, payload: bytes) -> None:
        document = self.track.sequence.project._document
        data = self._text_data()
        if data is None:
            raise ValueError("caption has no styled-text payload")
        write_payload_element(document, data, payload)

    def __repr__(self) -> str:
        return f"Caption(text={self._text!r}, start={self.start.seconds:.3f}s)"


class CaptionTrack:
    """A caption track on a sequence.

    Premiere keeps captions on their own kind of track - a data track, the
    third track group every sequence carries next to its video and audio
    ones. ExtendScript can create these (`Sequence.createCaptionTrack`) but
    never exposed their contents.
    """

    def __init__(self, _element: ET.Element, sequence: Sequence) -> None:
        self._element = _element
        self.sequence = sequence
        self._captions: list[Caption] = []

    @property
    def id(self) -> int:
        """The track's stored ID. Read-only."""
        return int(self._element.findtext("DataClipTrack/ClipTrack/Track/ID") or 0)

    @property
    def index(self) -> int:
        """The track's index within the sequence's caption tracks. Read-only."""
        return int(self._element.findtext("DataClipTrack/ClipTrack/Track/Index") or 0)

    @property
    def captions(self) -> list[Caption]:
        """The captions on this track, in timeline order. Read-only."""
        return self._captions

    @property
    def format(self) -> CaptionFormat:
        """The track's broadcast caption format. Read/write.

        The file stores ExtendScript's `CAPTION_FORMAT_*` constant split in
        two: `Format` holds its low word and `SubFormat` the high one (the
        three Teletext variants), and `SUBTITLE` elides both - so an
        absent `Format` reads as `SUBTITLE`. Swept straight off Premiere's
        own `createCaptionTrack` for all seven formats.
        """
        low = int(self._element.findtext("Format") or 0)
        high = int(self._element.findtext("SubFormat") or 0)
        return CaptionFormat((high << 16) | low)

    @format.setter
    def format(self, value: CaptionFormat) -> None:
        _validate_caption_format(value)
        write_track_format(self._element, int(value))

    def __repr__(self) -> str:
        return f"CaptionTrack(index={self.index}, captions={len(self._captions)})"


_TRACK_ITEM = "DataClipTrackItem/ClipTrackItem/TrackItem/"
