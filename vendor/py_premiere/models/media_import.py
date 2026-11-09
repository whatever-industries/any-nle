"""Synthesis of the object graph `Project.import_files` creates.

Mirrors what Premiere itself writes for a fresh import, verified
field-for-field against its own import of the same file. A still or a
video-only movie is 9 objects: ClipProjectItem, MasterClip, ClipLoggingInfo,
template VideoClip, ClipChannelGroupVectorSerializer, Markers,
VideoMediaSource, Media, VideoStream. Audio replaces the video half with its
own clip, source, stream, secondary contents and channel serializers - once
per SOURCE clip, since a channel count Premiere has no native type for is
split into one mono clip per channel.

The content-state hashes Premiere stamps (`FileKey` aside,
`ContentAndMetadataState` and its UTF-16 base64 twin `ModificationState`)
are Premiere-internal change-detection caches; py writes a fresh GUID and
Premiere refreshes it on open. The media-header readers here are hand-rolled
so a corpus asset never depends on what a given Python version's `wave`
module happens to accept.
"""

from __future__ import annotations

import base64
import os
import struct
import uuid
import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING, NamedTuple

if TYPE_CHECKING:
    from typing import Iterator

_ITEM_CLASS_ID = "cb4e0ed7-aca1-4171-8525-e3658dec06dd"
_MASTER_CLASS_ID = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf"
_LOGGING_CLASS_ID = "77ab7fdd-dcdf-465d-9906-7a330ca1e738"
_VIDEO_CLIP_CLASS_ID = "9308dbef-2440-4acb-9ab2-953b9a4e82ec"
_CHANNEL_GROUPS_CLASS_ID = "a3127a8c-95d4-456e-a7f5-171b3f922426"
_MARKERS_CLASS_ID = "bee50706-b524-416c-9f03-b596ce5f6866"
_SOURCE_CLASS_ID = "e64ddf74-8fac-4682-8aa8-0e0ca2248949"
_MEDIA_CLASS_ID = "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff"
_STREAM_CLASS_ID = "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5"

#: The generic importer's implementation GUID (BMP, PNG and WAV alike).
_BMP_IMPLEMENTATION_ID = "1fa18bfa-255c-44b1-ad73-56bcd99fceaf"

_AUDIO_CLIP_CLASS_ID = "b8830d03-de02-41ee-84ec-fe566dc70cd9"
_AUDIO_SOURCE_CLASS_ID = "f588da05-fc2a-4fbc-9383-74d653b379e3"
_AUDIO_STREAM_CLASS_ID = "0b5cf52f-2b85-4863-890b-8844b64ecfe9"
_MASTER_AUDIO_CHAIN_CLASS_ID = "3cb131d1-d3c0-47ae-a19a-bdf75ea11674"
_CHANNEL_VECTOR_CLASS_ID = "333d203b-3a53-4195-8894-fc7523ff3dc7"
_CHANNEL_CLASS_ID = "5c89aa7a-89a6-4483-becd-f2b1def42316"
_SECONDARY_CLASS_ID = "f9d004b5-cb04-4e2f-af6f-64fadc2c4be9"

#: The factory Audio label (Caribbean), used when no local prefs exist.
AUDIO_LABEL_INDEX = 2
AUDIO_LABEL_COLOR = 480554

#: The factory Video label (Violet).
VIDEO_LABEL_INDEX = 0
VIDEO_LABEL_COLOR = 11405886

#: The factory AV label, taken by media carrying both video and audio.
AV_LABEL_INDEX = 1
AV_LABEL_COLOR = 6769408

#: Timecode display format per (ticks-per-frame, drop-frame), from Premiere's
#: own imports: 25 fps (its AVI and MP4 imports) and 29.97 both ways - 103
#: non-drop, matching what the still path writes at that rate, and 102 for a
#: `01:00:00;00` drop-frame timecode track. An unlisted rate raises rather
#: than guessing a format.
TIMECODE_FORMATS = {
    (10160640000, False): 101,
    (8475667200, False): 103,
    (8475667200, True): 102,
    # 30 fps drop-frame, from the Motion Graphics template fixture and
    # `caption_builder`'s streams alike.
    (8467200000, True): 104,
}

#: `AudioStream/SampleType` for the AAC track of a movie container.
AAC_SAMPLE_TYPE = "7"

#: Mono is the one layout that does not follow `audio_channel_layout`'s rule.
MONO_LAYOUT = '[{"channellabel":0}]'

#: RIFF `fmt ` format tags.
WAVE_FORMAT_PCM = 1
WAVE_FORMAT_IEEE_FLOAT = 3
WAVE_FORMAT_EXTENSIBLE = 0xFFFE

#: `ChannelType` per channel count: Premiere has exactly three native audio
#: channel types (the `DefaultAudioStandard*Tracks` triple), and imports any
#: other channel count as one mono source clip per channel.
CHANNEL_TYPES = {1: 0, 2: 1, 6: 2}

#: `AudioStream/SampleType` per (format tag, bytes per sample). Premiere
#: ELIDES the element for 24-bit PCM - hence the None - and stamps 7 for the
#: AAC stream of a movie container.
WAV_SAMPLE_TYPES = {
    (WAVE_FORMAT_PCM, 2): "3",
    (WAVE_FORMAT_PCM, 3): None,
    (WAVE_FORMAT_IEEE_FLOAT, 4): "6",
}

#: The same, for AIFF: big-endian PCM gets its own `SampleType`, so the bit
#: depth alone does not determine it - 16-bit is 3 in a WAV and 11 here.
AIFF_SAMPLE_TYPES: dict[tuple[int, int], str | None] = {(WAVE_FORMAT_PCM, 2): "11"}

#: The sample-type table each PCM audio suffix uses (a None value means
#: Premiere elides `SampleType` for that format).
PCM_SAMPLE_TYPES: dict[str, dict[tuple[int, int], str | None]] = {
    ".wav": WAV_SAMPLE_TYPES,
    ".aiff": AIFF_SAMPLE_TYPES,
}

#: `SampleType` for compressed audio, which is a CONSTANT per format rather
#: than a function of the encoding: what decides it is what the codec decodes
#: TO. The float codecs all share 7 (the same value the AAC track of a movie
#: container gets); WMA decodes to 16-bit int and so reuses 3.
COMPRESSED_SAMPLE_TYPES = {".m4a": "7", ".mp3": "7", ".aac": "7", ".wma": "3"}

#: Every audio suffix `import_files` routes to the audio path. MP3 and raw
#: AAC were once refused here for having no derivable duration; both trims
#: are now measured against Premiere (see `_mp3_trim` and `read_aac_info`).
AUDIO_FORMATS = tuple(PCM_SAMPLE_TYPES) + tuple(COMPRESSED_SAMPLE_TYPES)


class WavInfo(NamedTuple):
    """What Premiere's audio import needs from a WAV header."""

    sample_rate: int
    frames: int
    channels: int
    sample_width: int
    format_tag: int
    channel_mask: int


class StillProfile(NamedTuple):
    """Which `VideoStream` fields Premiere stamps for a still format."""

    codec: int
    alpha_type: bool = False
    field_uncertain: bool = True


#: One entry per verified Premiere still import. GIF decodes to the same
#: raster codec as BMP ('RAW ') with alpha; PNG and JPEG elide `AlphaType`;
#: PSD comes in as 'UNKN' and is the only still that also elides
#: `FieldTypeIsUncertain`.
STILL_CODECS = {
    ".bmp": StillProfile(1380013856, alpha_type=True),
    ".gif": StillProfile(1380013856, alpha_type=True),
    ".png": StillProfile(1886283552),
    ".jpg": StillProfile(1785750887),
    ".psd": StillProfile(1431194446, field_uncertain=False),
    ".tif": StillProfile(1953064550),
}
_ZERO_GUID = "00000000-0000-0000-0000-000000000000"

#: Still defaults Premiere writes at factory settings, used when no local
#: preferences file exists: 29.97 fps timebase, a 12-hour "infinite"
#: duration, a ~5 s (149-frame) default clip length, and the Lavender label.
STILL_FRAME_RATE = 8475667200
STILL_DURATION = 10973491200000000
STILL_DEFAULT_OUT = 1262874412800
STILL_LABEL_INDEX = 3
STILL_LABEL_COLOR = 8851829
_STILL_COLOR_SPACE = (
    '{"baseColorProfile":{"colorProfileName":"BT.709 RGB Full"},"baseProfileType":1}'
)
#: What an H.264 stream reports instead (in MP4 and MOV alike; ProRes and
#: MPEG-2 report the plain RGB-Full profile above).
_MOVIE_COLOR_SPACE = (
    '{"baseColorProfile":{"colorProfileData":"AQAAAP////8=",'
    '"colorProfileName":"BT.709,32f,Display-Referred"},"baseProfileType":1}'
)


class VideoStreamProfile(NamedTuple):
    """Which optional `VideoStream` fields Premiere stamps for a codec."""

    color_space: str
    ignore_alpha: bool = False
    alpha_type: str | None = None
    alpha_uncertain: bool = False
    field_uncertain: bool = False
    orientation: bool = False
    codec_type: str | None = None
    """What Premiere stamps as `CodecType` when it differs from the
    container's own fourcc (H.265 is `hvc1` in the file but `HEVC` here)."""
    field_type: str | None = None
    """A constant `OriginalFieldType` this codec always gets, where most
    carry one only when the media itself declares a field order."""


#: Container FAMILY per suffix. The same codec is stamped differently in
#: different containers - MJPEG in an AVI and MPEG-2 in an MXF both report
#: `CodecType` 'MPEG', with different field sets - so a profile is keyed by
#: (container, codec) rather than by codec alone.
MOVIE_CONTAINERS = {".avi": "avi", ".mp4": "mov", ".mov": "mov", ".mxf": "mxf"}

#: Movie containers whose headers py can read.
MOVIE_SUFFIXES = tuple(MOVIE_CONTAINERS)

#: One entry per (container, codec), each established by importing that
#: combination through Premiere and diffing: what it stamps varies a lot more
#: than the frame geometry does.
MOVIE_STREAM_PROFILES = {
    # Uncompressed AVI is read raw, so Premiere stamps NO CodecType at all
    # and leaves the alpha question open.
    ("avi", "DIB "): VideoStreamProfile(
        _STILL_COLOR_SPACE, alpha_uncertain=True, field_uncertain=True
    ),
    # MJPEG in an AVI reports 'MPEG' - the same fourcc MPEG-2-in-MXF gets -
    # but with a constant field type and no orientation.
    ("avi", "MJPG"): VideoStreamProfile(
        _STILL_COLOR_SPACE,
        field_uncertain=True,
        codec_type="MPEG",
        field_type="3",
    ),
    ("mov", "avc1"): VideoStreamProfile(
        _MOVIE_COLOR_SPACE,
        ignore_alpha=True,
        alpha_type="3",
        field_uncertain=True,
        orientation=True,
    ),
    # H.265 reports exactly what H.264 does, and DNxHR exactly what ProRes
    # does - the split is decoder families, not individual codecs.
    ("mov", "hvc1"): VideoStreamProfile(
        _MOVIE_COLOR_SPACE,
        ignore_alpha=True,
        alpha_type="3",
        field_uncertain=True,
        orientation=True,
        codec_type="HEVC",
    ),
    ("mov", "apch"): VideoStreamProfile(_STILL_COLOR_SPACE, orientation=True),
    ("mov", "AVdh"): VideoStreamProfile(_STILL_COLOR_SPACE, orientation=True),
    ("mxf", "MPEG"): VideoStreamProfile(_STILL_COLOR_SPACE),
}


def _unpack_from(fmt: str, data: bytes, offset: int) -> tuple:
    """`struct.unpack_from` that refuses to read past the buffer.

    These readers parse UNTRUSTED media files, and every offset they use is
    taken from the file itself, so a truncated download or a corrupt header
    steers the reads anywhere. `struct.error` is not something a caller of
    `import_files` can act on; a malformed file reports as `ValueError`,
    which is what the format checks in this module already raise.
    """
    size = struct.calcsize(fmt)
    if offset < 0 or offset + size > len(data):
        raise ValueError(
            f"malformed media file: cannot read {size} bytes at {offset} of {len(data)}"
        )
    return struct.unpack_from(fmt, data, offset)


def read_bmp_size(data: bytes) -> tuple[int, int]:
    """The `(width, height)` of a BMP file (BITMAPINFOHEADER)."""
    if data[:2] != b"BM" or len(data) < 26:
        raise ValueError("not a BMP file")
    width, height = _unpack_from("<ii", data, 18)
    return width, abs(height)


def read_png_size(data: bytes) -> tuple[int, int]:
    """The `(width, height)` of a PNG file (IHDR)."""
    if data[:8] != b"\x89PNG\r\n\x1a\n" or len(data) < 24:
        raise ValueError("not a PNG file")
    width, height = _unpack_from(">II", data, 16)
    return width, height


def read_tiff_size(data: bytes) -> tuple[int, int]:
    """The `(width, height)` of a TIFF file (its first IFD)."""
    if len(data) < 8:
        raise ValueError("not a TIFF file")
    if data[:2] == b"II":
        order = "<"
    elif data[:2] == b"MM":
        order = ">"
    else:
        raise ValueError("not a TIFF file")
    ifd = _unpack_from(order + "I", data, 4)[0]
    if ifd + 2 > len(data):
        raise ValueError("truncated TIFF file")
    count = _unpack_from(order + "H", data, ifd)[0]
    sizes = {}
    for index in range(count):
        entry = ifd + 2 + index * 12
        if entry + 12 > len(data):
            raise ValueError("truncated TIFF file")
        tag, kind = _unpack_from(order + "HH", data, entry)
        if tag in (0x0100, 0x0101):
            # SHORT (3) and LONG (4) are the only widths/heights in practice.
            fmt = "H" if kind == 3 else "I"
            sizes[tag] = _unpack_from(order + fmt, data, entry + 8)[0]
    if 0x0100 not in sizes or 0x0101 not in sizes:
        raise ValueError("TIFF file has no image dimensions")
    return sizes[0x0100], sizes[0x0101]


def read_psd_size(data: bytes) -> tuple[int, int]:
    """The `(width, height)` of a PSD/PSB file (its file header)."""
    if data[:4] != b"8BPS" or len(data) < 26:
        raise ValueError("not a PSD file")
    height, width = _unpack_from(">II", data, 14)
    return width, height


def read_gif_size(data: bytes) -> tuple[int, int]:
    """The `(width, height)` of a GIF file (its logical screen descriptor)."""
    if data[:6] not in (b"GIF87a", b"GIF89a") or len(data) < 10:
        raise ValueError("not a GIF file")
    return _unpack_from("<HH", data, 6)  # type: ignore[return-value]


def read_jpeg_size(data: bytes) -> tuple[int, int]:
    """The `(width, height)` of a JPEG file (its start-of-frame marker)."""
    if data[:2] != b"\xff\xd8":
        raise ValueError("not a JPEG file")
    offset = 2
    while offset + 4 <= len(data):
        if data[offset] != 0xFF:
            raise ValueError("malformed JPEG segment")
        marker = data[offset + 1]
        length = _unpack_from(">H", data, offset + 2)[0]
        # Every start-of-frame marker carries the size in the same place;
        # 0xC4/0xC8/0xCC are the Huffman/JPG/arithmetic holes in the range.
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
            if offset + 9 > len(data):
                raise ValueError("truncated JPEG start-of-frame marker")
            height, width = _unpack_from(">HH", data, offset + 5)
            return width, height
        offset += 2 + length
    raise ValueError("JPEG file has no start-of-frame marker")


#: The AVI stream fourcc that means "uncompressed", for which Premiere
#: writes no `CodecType` element at all.
_NO_CODEC = "DIB "


def read_avi_info(data: bytes) -> tuple[int, int, int, int, str]:
    """`(width, height, fps, frames, codec)` of an AVI file."""
    if data[:4] != b"RIFF" or data[8:12] != b"AVI ":
        raise ValueError("not an AVI file")
    strh = data.index(b"strh") + 8
    if data[strh : strh + 4] != b"vids":
        raise ValueError("first AVI stream is not video")
    codec = data[strh + 4 : strh + 8].decode("ascii", "replace")
    scale, rate = _unpack_from("<II", data, strh + 20)
    frames = _unpack_from("<I", data, strh + 32)[0]
    strf = data.index(b"strf", strh) + 8
    width, height = _unpack_from("<ii", data, strf + 4)
    if scale == 0 or rate % scale:
        raise NotImplementedError("non-integer AVI frame rates are not supported")
    return width, abs(height), rate // scale, frames, codec


class MovieAudioInfo(NamedTuple):
    """The audio track of a movie container."""

    sample_rate: int
    channels: int
    frames: int


class MovieInfo(NamedTuple):
    """What Premiere's video import needs from a movie container.

    `frame_duration` is in `timescale` units, so the frame rate stays exact
    for 1001-based rates. `start_timecode` counts frames since midnight, from
    an embedded timecode track (0 when there is none), and `drop_frame` is
    that track's drop-frame flag. `field_order` is a `VideoFieldType` value
    read from the sample description's `fiel` extension (0 = progressive).
    """

    width: int
    height: int
    timescale: int
    frame_duration: int
    frames: int
    codec: str
    start_timecode: int
    drop_frame: bool
    field_order: int
    audio: MovieAudioInfo | None
    clip_id: str = ""
    """The container's own material identifier (an MXF UMID); empty if none."""


class _Track(NamedTuple):
    """One movie track, as far as an import cares."""

    handler: bytes
    timescale: int
    format: bytes
    samples: int
    sample_delta: int
    width: int
    height: int
    channels: int
    sample_rate: int
    first_chunk: int
    field_order: int
    drop_frame: bool
    sample_delta_runs: int
    """`stts` entry count. More than one means the track is variable frame
    rate, so a single `sample_delta` does not describe it."""


def _boxes(data: bytes, start: int, end: int) -> Iterator[tuple[bytes, int, int]]:
    """Walk the ISO base-media boxes in `data[start:end]`.

    Yields `(kind, body offset, box end)` per box.
    """
    offset = start
    while offset + 8 <= end:
        size = _unpack_from(">I", data, offset)[0]
        kind = data[offset + 4 : offset + 8]
        body = offset + 8
        if size == 1:
            size = _unpack_from(">Q", data, body)[0]
            body += 8
        elif size == 0:
            size = end - offset
        if size < 8 or offset + size > end:
            raise ValueError("malformed box in movie container")
        yield kind, body, offset + size
        offset += size


def _box(data: bytes, start: int, end: int, *path: bytes) -> tuple[int, int] | None:
    """Find a nested container box by path, as `(body offset, box end)`."""
    span = (start, end)
    for kind in path:
        found = None
        for child_kind, body, child_end in _boxes(data, span[0], span[1]):
            if child_kind == kind:
                found = (body, child_end)
                break
        if found is None:
            return None
        span = found
    return span


#: `fiel` detail values that mean upper/lower field first (QuickTime spells
#: each dominance two ways). Only 9 - what ffmpeg writes for `-field_order tt`
#: and what Premiere read back as UPPER_FIRST - is verified against Premiere.
_FIELD_DETAIL_UPPER = (1, 9)
_FIELD_DETAIL_LOWER = (6, 14)


def _read_field_order(data: bytes, entry: int, entry_end: int) -> int:
    """The `VideoFieldType` a sample entry's `fiel` extension declares."""
    # The extension boxes sit after the 86-byte VisualSampleEntry header.
    for kind, body, _end in _boxes(data, entry + 86, entry_end):
        if kind != b"fiel":
            continue
        fields, detail = data[body], data[body + 1]
        if fields != 2:
            return 0
        if detail in _FIELD_DETAIL_UPPER:
            return 1
        if detail in _FIELD_DETAIL_LOWER:
            return 2
        raise NotImplementedError(f"unknown fiel field detail {detail}")
    return 0


def _read_track(data: bytes, start: int, end: int) -> _Track:
    handler = b""
    timescale = 0
    fourcc = b""
    samples = 0
    sample_delta = 0
    width = height = channels = sample_rate = 0
    first_chunk = 0
    field_order = 0
    drop_frame = False
    media = _box(data, start, end, b"mdia")
    if media is None:
        raise ValueError("movie track has no mdia box")
    hdlr = _box(data, media[0], media[1], b"hdlr")
    if hdlr is not None:
        handler = data[hdlr[0] + 8 : hdlr[0] + 12]
    mdhd = _box(data, media[0], media[1], b"mdhd")
    if mdhd is not None:
        version = data[mdhd[0]]
        timescale = _unpack_from(">I", data, mdhd[0] + (20 if version else 12))[0]
    table = _box(data, media[0], media[1], b"minf", b"stbl")
    if table is None:
        raise ValueError("movie track has no sample table")
    stsd = _box(data, table[0], table[1], b"stsd")
    if stsd is not None:
        # version/flags + entry count, then the first sample description:
        # size + format + 6 reserved + data reference index.
        entry = stsd[0] + 8
        entry_end = entry + _unpack_from(">I", data, entry)[0]
        fourcc = data[entry + 4 : entry + 8]
        if handler == b"vide":
            width, height = _unpack_from(">HH", data, entry + 32)
            field_order = _read_field_order(data, entry, entry_end)
        elif handler == b"soun":
            # v0/v1 keep the rate in a 16.16 fixed-point field whose integer
            # half is all that is ever used (the fraction is always zero), so
            # it cannot carry a rate above 65535. v2 parks that field at 1 and
            # states the real rate as a float64 further in - reading the
            # legacy half there would silently yield 1 Hz, so refuse instead.
            if _unpack_from(">H", data, entry + 16)[0] == 2:
                raise NotImplementedError(
                    "version 2 audio sample descriptions are not supported"
                )
            channels = _unpack_from(">H", data, entry + 24)[0]
            sample_rate = _unpack_from(">H", data, entry + 32)[0]
        elif handler == b"tmcd":
            # TimeCodeSampleEntry: reserved(4) then flags, bit 0 = drop frame.
            drop_frame = bool(_unpack_from(">I", data, entry + 20)[0] & 1)
    sample_delta_runs = 0
    stts = _box(data, table[0], table[1], b"stts")
    if stts is not None:
        sample_delta_runs = _unpack_from(">I", data, stts[0] + 4)[0]
        sample_delta = _unpack_from(">I", data, stts[0] + 12)[0]
    stsz = _box(data, table[0], table[1], b"stsz")
    if stsz is not None:
        samples = _unpack_from(">I", data, stsz[0] + 8)[0]
    chunks = _box(data, table[0], table[1], b"stco")
    if chunks is not None:
        first_chunk = _unpack_from(">I", data, chunks[0] + 8)[0]
    else:
        chunks = _box(data, table[0], table[1], b"co64")
        if chunks is not None:
            first_chunk = _unpack_from(">Q", data, chunks[0] + 8)[0]
    return _Track(
        handler,
        timescale,
        fourcc,
        samples,
        sample_delta,
        width,
        height,
        channels,
        sample_rate,
        first_chunk,
        field_order,
        drop_frame,
        sample_delta_runs,
    )


def read_mp4_info(data: bytes) -> MovieInfo:
    """Parse the `moov` metadata of an MP4/MOV file."""
    moov = _box(data, 0, len(data), b"moov")
    if moov is None:
        raise ValueError("not an MP4/MOV file")
    video = None
    audio = None
    start_timecode = 0
    drop_frame = False
    for kind, body, end in _boxes(data, moov[0], moov[1]):
        if kind != b"trak":
            continue
        track = _read_track(data, body, end)
        if track.handler == b"vide" and video is None:
            video = track
        elif track.handler == b"soun" and audio is None:
            audio = track
        elif track.handler == b"tmcd" and track.first_chunk:
            # A timecode track stores one sample: the frame number of its
            # start timecode, counted from midnight (drop-frame timecode
            # counts the frames that exist, so 01:00:00;00 is 107892 not
            # 108000).
            start_timecode = _unpack_from(">I", data, track.first_chunk)[0]
            drop_frame = track.drop_frame
    if video is None:
        raise ValueError("movie container has no video track")
    if not video.sample_delta or not video.timescale:
        raise ValueError("movie video track has no frame duration")
    if video.sample_delta_runs > 1:
        # The import states one frame rate for the whole clip. A variable
        # frame rate track has several, so the first run's delta would give a
        # duration that does not match the media - refuse rather than guess.
        raise NotImplementedError(
            "variable frame rate movies are not supported "
            f"({video.sample_delta_runs} stts runs)"
        )
    return MovieInfo(
        video.width,
        video.height,
        video.timescale,
        video.sample_delta,
        video.samples,
        video.format.decode("ascii", "replace"),
        start_timecode,
        drop_frame,
        video.field_order,
        None
        if audio is None
        else MovieAudioInfo(audio.sample_rate, audio.channels, audio.samples),
    )


#: MXF local-set tags this reader needs (SMPTE 377M static local tags).
_MXF_STORED_WIDTH = 0x3203
_MXF_STORED_HEIGHT = 0x3202
_MXF_DISPLAY_WIDTH = 0x3205
_MXF_DISPLAY_HEIGHT = 0x3208
_MXF_SAMPLE_RATE = 0x3001
_MXF_CONTAINER_DURATION = 0x3002
_MXF_DURATION = 0x0202
_MXF_PICTURE_CODING = 0x3201
_MXF_PACKAGE_UID = 0x4401

#: Codec fourcc per MXF picture-essence coding label, keyed by the bytes of
#: the UL that identify the compression scheme (the trailing bytes vary by
#: profile/level). One entry per verified Premiere import.
MXF_PICTURE_CODINGS = {"040102020101": "MPEG"}


def _mxf_ber(data: bytes, offset: int) -> tuple[int, int]:
    """Read a BER length, returning `(length, offset after it)`."""
    first = data[offset]
    if first < 0x80:
        return first, offset + 1
    count = first & 0x7F
    return (
        int.from_bytes(data[offset + 1 : offset + 1 + count], "big"),
        offset + 1 + count,
    )


def _mxf_local_sets(data: bytes) -> Iterator[dict[int, bytes]]:
    """Walk the KLV packets, yielding each local set as `{tag: value}`."""
    offset = 0
    while offset + 17 < len(data):
        if data[offset : offset + 4] != b"\x06\x0e\x2b\x34":
            return
        length, value = _mxf_ber(data, offset + 16)
        # 02 53 marks a local set (the metadata sets); everything else is
        # essence or an index table.
        if data[offset + 4 : offset + 6] == b"\x02\x53":
            tags = {}
            position = value
            end = value + length
            while position + 4 <= end:
                tag, size = _unpack_from(">HH", data, position)
                tags[tag] = data[position + 4 : position + 4 + size]
                position += 4 + size
            yield tags
        offset = value + length


def read_mxf_info(data: bytes) -> MovieInfo:
    """Parse the header metadata of an MXF file.

    MXF is KLV-framed rather than ISO base media, so none of the box reader
    applies: the facts live in local sets of tag/length/value triples. The
    picture descriptor is identified as the set carrying `StoredWidth`, and
    the frame count comes from the longest track duration.
    """
    if data[:4] != b"\x06\x0e\x2b\x34":
        raise ValueError("not an MXF file")
    picture: dict[int, bytes] = {}
    frames = 0
    clip_id = ""
    for tags in _mxf_local_sets(data):
        if _MXF_STORED_WIDTH in tags and not picture:
            picture = tags
        duration = tags.get(_MXF_DURATION)
        if duration is not None and len(duration) == 8:
            frames = max(frames, int.from_bytes(duration, "big"))
        package_uid = tags.get(_MXF_PACKAGE_UID)
        if package_uid is not None and not clip_id:
            # The MATERIAL package comes first, and its UMID is what Premiere
            # records as the clip's ClipID.
            clip_id = package_uid.hex().upper()
    if not picture:
        raise ValueError("MXF file has no picture descriptor")
    # MPEG-2 pads the STORED raster to a macroblock multiple (a 64x36 frame
    # is stored 64x48), so the display raster is what Premiere reports.
    width = int.from_bytes(
        picture.get(_MXF_DISPLAY_WIDTH) or picture[_MXF_STORED_WIDTH], "big"
    )
    height = int.from_bytes(
        picture.get(_MXF_DISPLAY_HEIGHT) or picture[_MXF_STORED_HEIGHT], "big"
    )
    rate = picture.get(_MXF_SAMPLE_RATE)
    if rate is None or len(rate) != 8:
        raise ValueError("MXF picture descriptor has no sample rate")
    timescale, frame_duration = struct.unpack(">II", rate)
    container = picture.get(_MXF_CONTAINER_DURATION)
    if container is not None and len(container) == 8:
        frames = int.from_bytes(container, "big")
    coding = picture.get(_MXF_PICTURE_CODING)
    if coding is None:
        raise ValueError("MXF picture descriptor has no essence coding label")
    key = coding[8:14].hex()
    if key not in MXF_PICTURE_CODINGS:
        raise NotImplementedError(
            f"no verified codec for MXF picture essence coding {coding.hex()}"
        )
    return MovieInfo(
        width,
        height,
        timescale,
        frame_duration,
        frames,
        MXF_PICTURE_CODINGS[key],
        0,
        False,
        0,
        None,
        clip_id,
    )


def read_wav_info(data: bytes) -> WavInfo:
    """Parse a WAV file's `fmt `/`data` chunks.

    Hand-rolled rather than delegated to the stdlib `wave` module: `wave`
    rejects IEEE-float files outright, and its WAVE_FORMAT_EXTENSIBLE
    support - the tag every >2-channel file carries - is too recent to rely
    on at this package's 3.7 floor.
    """
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise ValueError("not a WAV file")
    fmt_at = 0
    fmt_size = 0
    data_bytes = 0
    offset = 12
    while offset + 8 <= len(data):
        kind = data[offset : offset + 4]
        size = _unpack_from("<I", data, offset + 4)[0]
        body = offset + 8
        if kind == b"fmt ":
            fmt_at, fmt_size = body, size
        elif kind == b"data":
            # Streamed writers can declare more than they wrote.
            data_bytes = min(size, len(data) - body)
        offset = body + size + (size % 2)
    if fmt_size < 16:
        raise ValueError("WAV file has no fmt chunk")
    tag, channels, sample_rate, _bytes_per_second, block_align, bits = _unpack_from(
        "<HHIIHH", data, fmt_at
    )
    mask = 0
    if tag == WAVE_FORMAT_EXTENSIBLE:
        if fmt_size < 40:
            raise ValueError("truncated WAVE_FORMAT_EXTENSIBLE fmt chunk")
        # dwChannelMask, then the SubFormat GUID whose first word is the tag
        # the file really carries.
        mask = _unpack_from("<I", data, fmt_at + 20)[0]
        tag = _unpack_from("<I", data, fmt_at + 24)[0]
    if not block_align:
        block_align = channels * (bits // 8)
    if not block_align:
        # A fmt chunk declaring no channels or no bit depth leaves nothing to
        # divide the data by.
        raise ValueError(
            f"WAV fmt chunk declares no frame size ({channels} channels, {bits} bits)"
        )
    if not sample_rate:
        raise ValueError("WAV fmt chunk declares a zero sample rate")
    return WavInfo(
        sample_rate, data_bytes // block_align, channels, bits // 8, tag, mask
    )


def _read_extended(raw: bytes) -> int:
    """Decode an 80-bit IEEE extended float (AIFF's sample-rate field)."""
    if len(raw) < 10:
        raise ValueError("truncated AIFF sample-rate field")
    exponent = ((raw[0] & 0x7F) << 8) | raw[1]
    mantissa = int.from_bytes(raw[2:10], "big")
    if exponent == 0 and mantissa == 0:
        return 0
    # Sample rates are whole numbers, so the fractional part is dropped.
    return mantissa >> (16383 + 63 - exponent)


#: MPEG audio sample rates by version/index, and the Layer III bitrate
#: tables - enough to size a constant-bitrate frame.
_MP3_RATES = {
    3: (44100, 48000, 32000),
    2: (22050, 24000, 16000),
    0: (11025, 12000, 8000),
}
_MP3_BITRATES_V1 = (0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0)
_MP3_BITRATES_V2 = (0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0)

#: ADTS sampling-frequency index -> Hz (13-15 are reserved).
_ADTS_RATES = (
    96000,
    88200,
    64000,
    48000,
    44100,
    32000,
    24000,
    22050,
    16000,
    12000,
    11025,
    8000,
    7350,
)

_ASF_HEADER_GUID = bytes.fromhex("3026B2758E66CF11A6D900AA0062CE6C")
_ASF_FILE_PROPS_GUID = bytes.fromhex("A1DCAB8C47A9CF118EE400C00C205365")
_ASF_STREAM_PROPS_GUID = bytes.fromhex("9107DCB7B7A9CF118EE600C00C205365")
_ASF_AUDIO_PREFIX = bytes.fromhex("409E69F8")


def _skip_id3(data: bytes) -> int:
    """Offset of the audio after a leading ID3v2 tag (0 when there is none)."""
    if data[:3] != b"ID3" or len(data) < 10:
        return 0
    size = data[6:10]
    return 10 + (
        (size[0] & 0x7F) << 21
        | (size[1] & 0x7F) << 14
        | (size[2] & 0x7F) << 7
        | (size[3] & 0x7F)
    )


def _mp3_trim(data: bytes, tag_at: int, flags: int) -> int:
    """Samples an MP3's encoder tag says to drop: its delay plus its padding.

    The field sits at a FIXED offset in the tag that follows the Xing/Info
    header - 21 bytes past the encoder signature, as two packed 12-bit
    counts. The signature is NOT always `LAME`: ffmpeg writes `Lavc<version>`
    and fills the field exactly the same way, which is why keying on the
    signature (rather than the offset) misses it.

    `0` when the tag is absent or the field is unset, which is also what a
    stream with nothing to trim stores.
    """
    offset = tag_at + 8
    for flag, size in ((0x1, 4), (0x2, 4), (0x4, 100), (0x8, 4)):
        if flags & flag:
            offset += size
    if len(data) < offset + 24:
        return 0
    first, second, third = data[offset + 21], data[offset + 22], data[offset + 23]
    delay = (first << 4) | (second >> 4)
    padding = ((second & 0x0F) << 8) | third
    return delay + padding


def read_mp3_info(data: bytes) -> WavInfo:
    """Parse an MP3's first frame header, and its Xing tag when it has one."""
    start = _skip_id3(data)
    if len(data) < start + 4:
        raise ValueError("not an MP3 file")
    header = _unpack_from(">I", data, start)[0]
    if (header >> 21) & 0x7FF != 0x7FF:
        raise ValueError("not an MP3 file (no frame sync)")
    version = (header >> 19) & 0x3
    rate_index = (header >> 10) & 0x3
    if version not in _MP3_RATES or rate_index > 2:
        raise ValueError("unsupported MPEG audio version or sample rate")
    rate = _MP3_RATES[version][rate_index]
    mono = ((header >> 6) & 0x3) == 3
    channels = 1 if mono else 2
    samples_per_frame = 1152 if version == 3 else 576
    # A Xing/Info tag (VBR) carries the exact frame count; otherwise the
    # stream is constant-bitrate and the count comes from the file size.
    side = (17 if mono else 32) if version == 3 else (9 if mono else 17)
    tag_at = start + 4 + side
    if data[tag_at : tag_at + 4] in (b"Xing", b"Info"):
        flags = _unpack_from(">I", data, tag_at + 4)[0]
        if flags & 0x1:
            frames = _unpack_from(">I", data, tag_at + 8)[0]
            # The frame count covers the encoder's priming and its padding;
            # what plays is what is left after both.
            samples = frames * samples_per_frame - _mp3_trim(data, tag_at, flags)
            return WavInfo(rate, max(0, samples), channels, 0, 0, 0)
    bitrate = (_MP3_BITRATES_V1 if version == 3 else _MP3_BITRATES_V2)[
        (header >> 12) & 0xF
    ] * 1000
    if not bitrate:
        raise ValueError("MP3 frame declares a free or reserved bitrate")
    padding = (header >> 9) & 0x1
    frame_size = samples_per_frame // 8 * bitrate // rate + padding
    frames = (len(data) - start) // frame_size
    return WavInfo(rate, frames * samples_per_frame, channels, 0, 0, 0)


def read_aac_info(data: bytes) -> WavInfo:
    """Walk a raw ADTS stream, summing the samples its frames declare."""
    position = _skip_id3(data)

    def is_header(at: int) -> bool:
        return (
            at + 7 <= len(data)
            and data[at] == 0xFF
            and data[at + 1] & 0xF0 == 0xF0
            and (data[at + 1] >> 1) & 0x3 == 0
            and (data[at + 2] >> 2) & 0xF < 13
            and _adts_length(data, at) >= 7
        )

    while position + 7 <= len(data) and not is_header(position):
        position += 1
    if position + 7 > len(data):
        raise ValueError("not an ADTS stream")
    rate_index = (data[position + 2] >> 2) & 0xF
    rate = _ADTS_RATES[rate_index]
    channels = (data[position + 2] & 0x1) << 2 | (data[position + 3] >> 6) & 0x3
    samples = 0
    while position + 7 <= len(data) and is_header(position):
        # Each frame carries 1024 samples per raw data block.
        samples += 1024 * ((data[position + 6] & 0x03) + 1)
        position += _adts_length(data, position)
    # A raw ADTS stream has nowhere to declare its trim, so the true duration
    # is NOT recoverable - the excess over it varies with length (1312, 1600,
    # 1152 and 1280 samples across four test files). What IS reproducible is
    # what PREMIERE does, which is what this library has to match: it drops
    # exactly one AAC frame of decoder priming, measured identical at all
    # four durations. Note this leaves the clip slightly longer than the
    # audio really is - Premiere's own answer has the same slack.
    return WavInfo(rate, max(0, samples - _AAC_PRIMING), max(channels, 1), 0, 0, 0)


#: One AAC frame of decoder priming, which Premiere discards.
_AAC_PRIMING = 1024


def _adts_length(data: bytes, at: int) -> int:
    return (data[at + 3] & 0x03) << 11 | data[at + 4] << 3 | data[at + 5] >> 5


def read_wma_info(data: bytes) -> WavInfo:
    """Parse an ASF header: the audio stream's format and the duration."""
    if data[:16] != _ASF_HEADER_GUID:
        raise ValueError("not an ASF/WMA file")
    header_size = _unpack_from("<Q", data, 16)[0]
    body = data[30:header_size]
    rate = channels = 0
    seconds = 0.0
    position = 0
    while position + 24 <= len(body):
        guid = body[position : position + 16]
        size = _unpack_from("<Q", body, position + 16)[0]
        if size < 24 or position + size > len(body):
            break
        payload = body[position + 24 : position + size]
        if guid == _ASF_FILE_PROPS_GUID and len(payload) >= 64:
            # Send Duration excludes the preroll, which is what plays.
            send = _unpack_from("<Q", payload, 48)[0]
            if send:
                seconds = send / 1e7
            else:
                play = _unpack_from("<Q", payload, 40)[0]
                preroll = _unpack_from("<Q", payload, 56)[0]
                seconds = max(0.0, play / 1e7 - preroll / 1e3)
        elif guid == _ASF_STREAM_PROPS_GUID and len(payload) >= 72:
            if payload[:4] == _ASF_AUDIO_PREFIX:
                # The type-specific data is a WAVEFORMATEX.
                channels = _unpack_from("<H", payload, 56)[0]
                rate = _unpack_from("<I", payload, 58)[0]
        position += size
    if not rate:
        raise ValueError("ASF file has no audio stream properties")
    return WavInfo(rate, round(seconds * rate), max(channels, 1), 0, 0, 0)


def read_m4a_info(data: bytes) -> WavInfo:
    """The audio track of an MP4 container, for an audio-only `.m4a`.

    The packet count OVERSTATES the length, because an encoder primes the
    first packet and pads the last; the track's EDIT LIST states the trimmed
    span, which is what Premiere reports. Its duration is in the MOVIE
    timescale, not the track's.
    """
    moov = _box(data, 0, len(data), b"moov")
    if moov is None:
        raise ValueError("not an MP4/M4A file")
    mvhd = _box(data, moov[0], moov[1], b"mvhd")
    movie_timescale = 0
    if mvhd is not None:
        version = data[mvhd[0]]
        movie_timescale = _unpack_from(">I", data, mvhd[0] + (20 if version else 12))[0]
    for kind, body, end in _boxes(data, moov[0], moov[1]):
        if kind != b"trak":
            continue
        track = _read_track(data, body, end)
        if track.handler != b"soun":
            continue
        rate = track.sample_rate or track.timescale
        edit = _box(data, body, end, b"edts", b"elst")
        if edit is not None and movie_timescale:
            span = _unpack_from(">I", data, edit[0] + 8)[0]
            return WavInfo(
                rate, span * rate // movie_timescale, track.channels, 0, 0, 0
            )
        # No edit list: fall back to the packets, which include the padding.
        return WavInfo(
            rate, track.samples * track.sample_delta, track.channels, 0, 0, 0
        )
    raise ValueError("MP4 container has no audio track")


def read_aiff_info(data: bytes) -> WavInfo:
    """Parse an AIFF `COMM` chunk into the same shape as a WAV header.

    AIFF is big-endian PCM, which Premiere distinguishes from WAV's
    little-endian PCM by a different `SampleType` - the format tag is
    reported as PCM because that is what the samples are.
    """
    if data[:4] != b"FORM" or data[8:12] != b"AIFF":
        raise ValueError("not an AIFF file")
    offset = 12
    while offset + 8 <= len(data):
        kind = data[offset : offset + 4]
        size = _unpack_from(">I", data, offset + 4)[0]
        body = offset + 8
        if kind == b"COMM":
            channels, frames, bits = _unpack_from(">HIH", data, body)
            rate = _read_extended(data[body + 8 : body + 18])
            return WavInfo(rate, frames, channels, bits // 8, WAVE_FORMAT_PCM, 0)
        offset = body + size + (size % 2)
    raise ValueError("AIFF file has no COMM chunk")


def read_audio_info(data: bytes, suffix: str) -> WavInfo:
    """Parse an audio file's header, choosing the reader by suffix."""
    readers = {
        ".aac": read_aac_info,
        ".aiff": read_aiff_info,
        ".m4a": read_m4a_info,
        ".mp3": read_mp3_info,
        ".wav": read_wav_info,
        ".wma": read_wma_info,
    }
    if suffix not in readers:
        raise NotImplementedError(f"no audio reader for {suffix}")
    return readers[suffix](data)


def audio_channel_layout(channels: int) -> str:
    """Premiere's `AudioChannelLayout` for a source clip's channel count.

    Mono is the unlabelled channel 0; every other native layout labels its
    channels from 100 up (stereo L/R, 5.1 L/R/C/LFE/Ls/Rs).
    """
    if channels == 1:
        return MONO_LAYOUT
    labels = ",".join(f'{{"channellabel":{100 + i}}}' for i in range(channels))
    return "[" + labels + "]"


def _leaf(parent: ET.Element, tag: str, text: str, tail: str) -> ET.Element:
    element = ET.SubElement(parent, tag)
    element.text = text
    element.tail = tail
    return element


def _indexed_refs(parent: ET.Element, tag: str, object_ids: list[str]) -> None:
    """Fill a collection element with its `Index`/`ObjectRef` entries."""
    for index, object_id in enumerate(object_ids):
        entry = ET.SubElement(
            parent, tag, {"Index": str(index), "ObjectRef": object_id}
        )
        entry.tail = "\n\t\t\t" if index < len(object_ids) - 1 else "\n\t\t"


def _top(tag: str, class_id: str, version: str, uid: str | None = None) -> ET.Element:
    attrs = {"ObjectUID": uid} if uid else {"ObjectID": ""}
    attrs.update({"ClassID": class_id, "Version": version})
    element = ET.Element(tag, attrs)
    element.text = "\n\t\t"
    return element


def new_logging_info(name: str, frame_rate: int) -> ET.Element:
    element = _top("ClipLoggingInfo", _LOGGING_CLASS_ID, "10")
    _leaf(element, "CaptureMode", "2", "\n\t\t")
    _leaf(element, "ClipName", name, "\n\t\t")
    _leaf(element, "TimecodeFormat", "103", "\n\t\t")
    _leaf(element, "MediaFrameRate", str(frame_rate), "\n\t")
    return element


def new_media_logging_info(
    name: str,
    frame_rate: int,
    duration_ticks: int,
    capture_mode: str | None,
    timecode_format: str,
    start_ticks: int,
    clip_id: str = "",
) -> ET.Element:
    # Finite media logging differs from a still's: explicit media in/out
    # (audio: capture mode 1 + samples format 200; video: 2 + fps format;
    # media carrying BOTH streams: no capture mode at all). The in/out are
    # absolute, so media with a start timecode begins there rather than 0.
    element = _top("ClipLoggingInfo", _LOGGING_CLASS_ID, "10")
    if clip_id:
        # MXF carries its own material identifier, which Premiere records
        # ahead of everything else on the logging info.
        _leaf(element, "ClipID", clip_id, "\n\t\t")
    if capture_mode is not None:
        _leaf(element, "CaptureMode", capture_mode, "\n\t\t")
    _leaf(element, "ClipName", name, "\n\t\t")
    _leaf(element, "TimecodeFormat", timecode_format, "\n\t\t")
    _leaf(element, "MediaInPoint", str(start_ticks), "\n\t\t")
    _leaf(element, "MediaOutPoint", str(start_ticks + duration_ticks), "\n\t\t")
    _leaf(element, "MediaFrameRate", str(frame_rate), "\n\t")
    return element


def new_markers_collection(content_state: str) -> ET.Element:
    element = _top("Markers", _MARKERS_CLASS_ID, "4")
    _leaf(element, "ByGUID", "byGUID", "\n\t\t")
    _leaf(element, "LastMetadataState", _ZERO_GUID, "\n\t\t")
    _leaf(element, "LastContentState", content_state, "\n\t")
    return element


def new_channel_groups() -> ET.Element:
    element = _top("ClipChannelGroupVectorSerializer", _CHANNEL_GROUPS_CLASS_ID, "1")
    element.text = "\n\t"
    return element


def new_audio_channel_groups(vector_ids: list[str]) -> ET.Element:
    element = _top("ClipChannelGroupVectorSerializer", _CHANNEL_GROUPS_CLASS_ID, "1")
    vectors = ET.SubElement(element, "ClipChannelVectors", {"Version": "1"})
    vectors.text = "\n\t\t\t"
    vectors.tail = "\n\t"
    _indexed_refs(vectors, "ClipChannelVectorItem", vector_ids)
    return element


def new_channel_vector(channel_ids: list[str], channel_type: int) -> ET.Element:
    element = _top("ClipChannelVectorSerializer", _CHANNEL_VECTOR_CLASS_ID, "1")
    channels = ET.SubElement(element, "ClipChannels", {"Version": "1"})
    channels.text = "\n\t\t\t"
    channels.tail = "\n\t\t"
    _indexed_refs(channels, "ClipChannelItem", channel_ids)
    _leaf(element, "ChannelType", str(channel_type), "\n\t")
    return element


def new_channel_serializer(source_clip_index: int, channel_index: int) -> ET.Element:
    element = _top("ClipChannelSerializer", _CHANNEL_CLASS_ID, "1")
    _leaf(element, "SourceClipIndex", str(source_clip_index), "\n\t\t")
    _leaf(element, "mSourceChannelIndex", str(channel_index), "\n\t")
    return element


def new_master_audio_chain(channels: int, channel_type: int) -> ET.Element:
    # The master clip's own default chain is bag-less (unlike the timeline
    # placement chain, which carries the MZ.ActiveComponent node bag). A chain
    # feeding more than one channel also gets a channel-volume component and
    # repeats the layout - verified against Premiere's own stereo, 5.1 and
    # AAC imports (its mono and split-to-mono imports write neither).
    element = _top("AudioComponentChain", _MASTER_AUDIO_CHAIN_CLASS_ID, "4")
    _leaf(element, "DefaultVol", "true", "\n\t\t")
    _leaf(element, "DefaultVolumeComponentID", "1", "\n\t\t")
    if channels > 1:
        _leaf(element, "DefaultChannelVolumeComponentID", "2", "\n\t\t")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t"
    chain.tail = "\n\t\t" if channels > 1 else "\n\t"
    if channels > 1:
        _leaf(element, "AudioChannelLayout", audio_channel_layout(channels), "\n\t\t")
        _leaf(element, "ChannelType", str(channel_type), "\n\t")
    return element


def new_movie_video_stream(
    width: int,
    height: int,
    frame_rate: int,
    duration_ticks: int,
    container: str,
    codec: str,
    field_order: int,
) -> ET.Element:
    """The `VideoStream` of imported movie media, per `MOVIE_STREAM_PROFILES`.

    A movie stream differs from a still's in carrying a real duration and no
    `IsStill`; which optional fields follow depends on the codec.
    `field_order` is a `VideoFieldType` value, written as `OriginalFieldType`
    for natively interlaced media - Premiere leaves the element out for
    progressive media, which is what 0 means here.
    """
    key = (container, codec)
    if key not in MOVIE_STREAM_PROFILES:
        raise NotImplementedError(f"no verified VideoStream profile for {key}")
    profile = MOVIE_STREAM_PROFILES[key]
    element = _top("VideoStream", _STREAM_CLASS_ID, "22")
    _leaf(element, "FrameRate", str(frame_rate), "\n\t\t")
    _leaf(element, "Duration", str(duration_ticks), "\n\t\t")
    if profile.ignore_alpha:
        _leaf(element, "IgnoreAlpha", "true", "\n\t\t")
    _leaf(element, "FrameRect", f"0,0,{width},{height}", "\n\t\t")
    stamped = profile.codec_type or codec
    if stamped != _NO_CODEC:
        fourcc = int.from_bytes(stamped.encode("ascii"), "big")
        _leaf(element, "CodecType", str(fourcc), "\n\t\t")
    _leaf(element, "OriginalColorSpace", profile.color_space, "\n\t\t")
    if profile.alpha_type is not None:
        _leaf(element, "AlphaType", profile.alpha_type, "\n\t\t")
    if profile.alpha_uncertain:
        _leaf(element, "AlphaInfoIsUncertain", "true", "\n\t\t")
    # The declared field order precedes the uncertainty flag - Premiere's
    # order in its MJPEG import, the only stream carrying both.
    stored_field = str(field_order) if field_order else profile.field_type
    if stored_field:
        _leaf(element, "OriginalFieldType", stored_field, "\n\t\t")
    if profile.field_uncertain:
        _leaf(element, "FieldTypeIsUncertain", "true", "\n\t\t")
    if profile.orientation:
        _leaf(element, "OriginalImageOrientationType", "1", "\n\t\t")
    # Whichever optional leaf ended up last closes the object.
    list(element)[-1].tail = "\n\t"
    return element


def new_movie_template_clip(
    markers_id: str, source_id: str, label_index: int, label_color: int
) -> ET.Element:
    # Unmarked (no in/out): a placed movie plays its full duration.
    element = _top("VideoClip", _VIDEO_CLIP_CLASS_ID, "11")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t"
    node = ET.SubElement(core, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(properties, "asl.clip.label.color", str(label_color), "\n\t\t\t\t\t")
    _leaf(
        properties,
        "asl.clip.label.name",
        f"BE.Prefs.LabelColors.{label_index}",
        "\n\t\t\t\t",
    )
    owner = ET.SubElement(core, "MarkerOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t"
    owner.tail = "\n\t\t\t"
    ET.SubElement(owner, "Markers", {"ObjectRef": markers_id}).tail = "\n\t\t\t"
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t"
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t")
    return element


def new_movie_media(
    media_uid: str,
    stream_id: str,
    file_path: str,
    project_dir: str,
    content_state: str,
    audio_stream_id: str | None,
    conformed_rate: int | None,
    start_ticks: int,
) -> ET.Element:
    # Finite video media: no Infinite flag. Media carrying both streams keeps
    # them on ONE Media object (audio ref first) and gains the audio conform
    # rate; media with a start timecode gains the AlternateStart pair, which
    # is what ExtendScript reports as ProjectItem.startTime.
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    if audio_stream_id is not None:
        ET.SubElement(
            element, "AudioStream", {"ObjectRef": audio_stream_id}
        ).tail = "\n\t\t"
    ET.SubElement(element, "VideoStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    blob = base64.b64encode(content_state.encode("utf-16-le")).decode("ascii")
    state = ET.SubElement(
        element,
        "ModificationState",
        {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())},
    )
    state.text = blob + "\n\t\t"
    state.tail = "\n\t\t"
    _leaf(
        element,
        "RelativePath",
        os.path.relpath(file_path, project_dir),
        "\n\t\t",
    )
    _leaf(element, "FilePath", file_path, "\n\t\t")
    _leaf(element, "ImplementationID", _BMP_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", os.path.basename(file_path), "\n\t\t")
    _leaf(element, "FileKey", str(uuid.uuid4()), "\n\t\t")
    if conformed_rate is not None:
        _leaf(element, "ConformedAudioRate", str(conformed_rate), "\n\t\t")
    if start_ticks:
        _leaf(element, "AlternateStart", str(start_ticks), "\n\t\t")
        _leaf(element, "UseAlternateStart", "true", "\n\t\t")
    _leaf(element, "ContentAndMetadataState", content_state, "\n\t\t")
    _leaf(element, "ActualMediaFilePath", file_path, "\n\t")
    return element


def new_movie_media_source(media_uid: str, duration_ticks: int) -> ET.Element:
    element = _top("VideoMediaSource", _SOURCE_CLASS_ID, "2")
    source = ET.SubElement(element, "MediaSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Media", {"ObjectURef": media_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(duration_ticks), "\n\t")
    return element


def new_audio_stream(
    frame_rate: int, layout: str, duration_ticks: int, sample_type: str | None
) -> ET.Element:
    # The conform/peak cache paths Premiere stamps are machine-local and
    # elided; Premiere reconforms on open. SampleType is 3 for 16-bit and
    # elided for 24-bit, matching Premiere's own imports.
    element = _top("AudioStream", _AUDIO_STREAM_CLASS_ID, "8")
    _leaf(element, "FrameRate", str(frame_rate), "\n\t\t")
    _leaf(element, "AudioChannelLayout", layout, "\n\t\t")
    if sample_type is None:
        _leaf(element, "Duration", str(duration_ticks), "\n\t")
    else:
        _leaf(element, "Duration", str(duration_ticks), "\n\t\t")
        _leaf(element, "SampleType", sample_type, "\n\t")
    return element


def new_audio_media_source(media_uid: str, duration_ticks: int) -> ET.Element:
    element = _top("AudioMediaSource", _AUDIO_SOURCE_CLASS_ID, "2")
    source = ET.SubElement(element, "MediaSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Media", {"ObjectURef": media_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(duration_ticks), "\n\t")
    return element


def new_audio_media(
    media_uid: str,
    stream_id: str,
    file_path: str,
    project_dir: str,
    content_state: str,
    conformed_rate: int,
    file_key: str,
    binary_hash: str,
    stream_number: int,
) -> ET.Element:
    # `StreamNumber` is what makes the source clips of a split multi-channel
    # import read DIFFERENT channels - every one of them otherwise describes
    # the same file with the same mono layout. Premiere elides it for stream
    # 0 and, for the same file, shares one FileKey/BinaryHash/content state
    # across all its Media objects, carrying the modification blob only on
    # the stream-0 Media.
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, "AudioStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    state = ET.SubElement(
        element,
        "ModificationState",
        {"Encoding": "base64", "BinaryHash": binary_hash},
    )
    if stream_number == 0:
        blob = base64.b64encode(content_state.encode("utf-16-le")).decode("ascii")
        state.text = blob + "\n\t\t"
    state.tail = "\n\t\t"
    _leaf(
        element,
        "RelativePath",
        os.path.relpath(file_path, project_dir),
        "\n\t\t",
    )
    _leaf(element, "FilePath", file_path, "\n\t\t")
    _leaf(element, "ImplementationID", _BMP_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", os.path.basename(file_path), "\n\t\t")
    _leaf(element, "FileKey", file_key, "\n\t\t")
    _leaf(element, "ConformedAudioRate", str(conformed_rate), "\n\t\t")
    if stream_number:
        _leaf(element, "StreamNumber", str(stream_number), "\n\t\t")
    _leaf(element, "ContentAndMetadataState", content_state, "\n\t\t")
    _leaf(element, "ActualMediaFilePath", file_path, "\n\t")
    return element


def new_audio_template_clip(
    markers_id: str,
    source_id: str,
    secondary_ids: list[str],
    label_index: int,
    label_color: int,
    layout: str,
) -> ET.Element:
    element = _top("AudioClip", _AUDIO_CLIP_CLASS_ID, "8")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t\t"
    node = ET.SubElement(core, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(properties, "asl.clip.label.color", str(label_color), "\n\t\t\t\t\t")
    _leaf(
        properties,
        "asl.clip.label.name",
        f"BE.Prefs.LabelColors.{label_index}",
        "\n\t\t\t\t",
    )
    owner = ET.SubElement(core, "MarkerOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t"
    owner.tail = "\n\t\t\t"
    ET.SubElement(owner, "Markers", {"ObjectRef": markers_id}).tail = "\n\t\t\t"
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t"
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t")
    secondary = ET.SubElement(element, "SecondaryContents", {"Version": "1"})
    secondary.text = "\n\t\t\t"
    secondary.tail = "\n\t\t"
    _indexed_refs(secondary, "SecondaryContentItem", secondary_ids)
    _leaf(element, "AudioChannelLayout", layout, "\n\t")
    return element


def new_secondary_content(content_ref: str, channel: int) -> ET.Element:
    element = _top("SecondaryContent", _SECONDARY_CLASS_ID, "1")
    ET.SubElement(element, "Content", {"ObjectRef": content_ref}).tail = "\n\t\t"
    _leaf(element, "ChannelIndex", str(channel), "\n\t")
    return element


def new_audio_master_clip(
    master_uid: str,
    logging_id: str,
    chain_ids: list[str],
    clip_ids: list[str],
    groups_id: str,
    name: str,
) -> ET.Element:
    # One chain and one Clip per SOURCE clip: a channel count Premiere has no
    # native type for is split into one mono source clip per channel, so a
    # 4-channel file lands four of each here.
    element = _top("MasterClip", _MASTER_CLASS_ID, "12", uid=master_uid)
    ET.SubElement(element, "LoggingInfo", {"ObjectRef": logging_id}).tail = "\n\t\t"
    chains = ET.SubElement(element, "AudioComponentChains", {"Version": "1"})
    chains.text = "\n\t\t\t"
    chains.tail = "\n\t\t"
    _indexed_refs(chains, "AudioComponentChain", chain_ids)
    clips = ET.SubElement(element, "Clips", {"Version": "1"})
    clips.text = "\n\t\t\t"
    clips.tail = "\n\t\t"
    _indexed_refs(clips, "Clip", clip_ids)
    ET.SubElement(
        element, "AudioClipChannelGroups", {"ObjectRef": groups_id}
    ).tail = "\n\t\t"
    _leaf(element, "Name", name, "\n\t\t")
    _leaf(element, "DefMappingID", str(uuid.uuid4()), "\n\t\t")
    _leaf(element, "MasterClipChangeVersion", "1", "\n\t")
    return element


def new_video_stream(
    width: int, height: int, frame_rate: int, profile: StillProfile
) -> ET.Element:
    element = _top("VideoStream", _STREAM_CLASS_ID, "22")
    _leaf(element, "FrameRate", str(frame_rate), "\n\t\t")
    _leaf(element, "Duration", str(STILL_DURATION), "\n\t\t")
    _leaf(element, "FrameRect", f"0,0,{width},{height}", "\n\t\t")
    _leaf(element, "CodecType", str(profile.codec), "\n\t\t")
    _leaf(element, "IsStill", "true", "\n\t\t")
    _leaf(element, "OriginalColorSpace", _STILL_COLOR_SPACE, "\n\t\t")
    if profile.alpha_type:
        # Premiere writes AlphaType only where the format carries alpha
        # info (BMP and GIF: 1; its own PNG and JPEG imports elide it).
        _leaf(element, "AlphaType", "1", "\n\t\t")
    if profile.field_uncertain:
        _leaf(element, "FieldTypeIsUncertain", "true", "\n\t\t")
    # Whichever optional leaf ended up last closes the object.
    list(element)[-1].tail = "\n\t"
    return element


def new_media(
    media_uid: str,
    stream_id: str,
    file_path: str,
    project_dir: str,
    content_state: str,
) -> ET.Element:
    element = _top("Media", _MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, "VideoStream", {"ObjectRef": stream_id}).tail = "\n\t\t"
    blob = base64.b64encode(content_state.encode("utf-16-le")).decode("ascii")
    state = ET.SubElement(
        element,
        "ModificationState",
        {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())},
    )
    state.text = blob + "\n\t\t"
    state.tail = "\n\t\t"
    _leaf(
        element,
        "RelativePath",
        os.path.relpath(file_path, project_dir),
        "\n\t\t",
    )
    _leaf(element, "FilePath", file_path, "\n\t\t")
    _leaf(element, "ImplementationID", _BMP_IMPLEMENTATION_ID, "\n\t\t")
    _leaf(element, "Title", os.path.basename(file_path), "\n\t\t")
    _leaf(element, "FileKey", str(uuid.uuid4()), "\n\t\t")
    _leaf(element, "Infinite", "true", "\n\t\t")
    _leaf(element, "ContentAndMetadataState", content_state, "\n\t\t")
    _leaf(element, "ActualMediaFilePath", file_path, "\n\t")
    return element


def new_media_source(media_uid: str) -> ET.Element:
    element = _top("VideoMediaSource", _SOURCE_CLASS_ID, "2")
    source = ET.SubElement(element, "MediaSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Media", {"ObjectURef": media_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(STILL_DURATION), "\n\t")
    return element


def new_template_clip(
    markers_id: str,
    source_id: str,
    label_index: int,
    label_color: int,
    out_ticks: int,
) -> ET.Element:
    element = _top("VideoClip", _VIDEO_CLIP_CLASS_ID, "11")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t"
    node = ET.SubElement(core, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(properties, "asl.clip.label.color", str(label_color), "\n\t\t\t\t\t")
    _leaf(
        properties,
        "asl.clip.label.name",
        f"BE.Prefs.LabelColors.{label_index}",
        "\n\t\t\t\t\t",
    )
    _leaf(
        properties,
        "BE.Prefs.StillImages.DefaultIsDropFrame",
        "true",
        "\n\t\t\t\t",
    )
    owner = ET.SubElement(core, "MarkerOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t"
    owner.tail = "\n\t\t\t"
    ET.SubElement(owner, "Markers", {"ObjectRef": markers_id}).tail = "\n\t\t\t"
    ET.SubElement(core, "Source", {"ObjectRef": source_id}).tail = "\n\t\t\t"
    _leaf(core, "OutPoint", str(out_ticks), "\n\t\t\t")
    _leaf(core, "InPoint", "0", "\n\t\t\t")
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t")
    return element


def new_master_clip(
    master_uid: str, logging_id: str, clip_id: str, groups_id: str, name: str
) -> ET.Element:
    element = _top("MasterClip", _MASTER_CLASS_ID, "12", uid=master_uid)
    ET.SubElement(element, "LoggingInfo", {"ObjectRef": logging_id}).tail = "\n\t\t"
    clips = ET.SubElement(element, "Clips", {"Version": "1"})
    clips.text = "\n\t\t\t"
    clips.tail = "\n\t\t"
    ET.SubElement(clips, "Clip", {"Index": "0", "ObjectRef": clip_id}).tail = "\n\t\t"
    ET.SubElement(
        element, "AudioClipChannelGroups", {"ObjectRef": groups_id}
    ).tail = "\n\t\t"
    _leaf(element, "Name", name, "\n\t\t")
    _leaf(element, "MasterClipChangeVersion", "0", "\n\t")
    return element


def new_clip_item(
    item_uid: str, master_uid: str, name: str, label_index: int
) -> ET.Element:
    element = _top("ClipProjectItem", _ITEM_CLASS_ID, "1", uid=item_uid)
    project_item = ET.SubElement(element, "ProjectItem", {"Version": "1"})
    project_item.text = "\n\t\t\t"
    project_item.tail = "\n\t\t"
    node = ET.SubElement(project_item, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(
        properties,
        "Column.PropertyText.Label",
        f"BE.Prefs.LabelColors.{label_index}",
        "\n\t\t\t\t",
    )
    _leaf(project_item, "Name", name, "\n\t\t")
    ET.SubElement(element, "MasterClip", {"ObjectURef": master_uid}).tail = "\n\t"
    return element
