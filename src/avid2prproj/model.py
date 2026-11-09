"""Neutral edit model: the converter's input contract, independent of Avid parsing.

All times are integer Premiere ticks (254016000000 per second); rates are exact
fractions. Loading is bounded and strict: unknown keys, wrong types, non-integer
times and dangling references are errors, not guesses.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from fractions import Fraction
import json
from pathlib import Path, PurePosixPath, PureWindowsPath
import re

TICKS_PER_SECOND = 254016000000
SCHEMA = "avid2prproj.model/1"
MAX_MODEL_BYTES = 16 << 20
MAX_ITEMS = 100_000
BIN_MODES = ("separate", "grouped", "merged")
UMID_RE = re.compile(r"[0-9A-F]{64}")


class ModelError(ValueError):
    pass


def _require(ok: bool, message: str) -> None:
    if not ok:
        raise ModelError(message)


def _keys(obj: object, where: str, required: set[str], optional: set[str] = frozenset()) -> dict:
    _require(isinstance(obj, dict), f"{where}: expected an object")
    unknown = set(obj) - required - optional
    _require(not unknown, f"{where}: unknown keys {sorted(unknown)}")
    missing = required - set(obj)
    _require(not missing, f"{where}: missing keys {sorted(missing)}")
    return obj


def _text(value: object, where: str, limit: int = 4096, allow_empty: bool = False) -> str:
    _require(isinstance(value, str) and (allow_empty or value.strip() != "") and len(value) <= limit, f"{where}: expected non-empty text")
    _require(not any(ord(c) < 32 and c not in "\t" for c in value), f"{where}: control characters are not allowed")
    return value


def _ticks(value: object, where: str, positive: bool = False) -> int:
    _require(isinstance(value, int) and not isinstance(value, bool), f"{where}: expected integer ticks")
    _require(value > 0 if positive else value >= 0, f"{where}: out of range")
    _require(value <= 2**62, f"{where}: out of range")
    return value


def _rate(value: object, where: str) -> Fraction:
    _require(isinstance(value, str) and re.fullmatch(r"[1-9][0-9]{0,6}/[1-9][0-9]{0,6}", value) is not None, f"{where}: expected 'num/den'")
    rate = Fraction(value)
    _require(Fraction(1) <= rate <= Fraction(1000), f"{where}: rate out of range")
    _require((TICKS_PER_SECOND / rate).denominator == 1, f"{where}: rate does not divide the tick clock")
    return rate


def ticks_per_unit(rate: Fraction) -> int:
    return int(TICKS_PER_SECOND / rate)


def _abs_path(value: object, where: str) -> str:
    text = _text(value, where, 32768)
    _require(PurePosixPath(text).is_absolute() or PureWindowsPath(text).is_absolute(), f"{where}: media paths must be absolute")
    return text


@dataclass(frozen=True)
class VideoFile:
    path: str
    codec: str
    width: int
    height: int
    rate: Fraction
    duration: int
    start: int = 0  # media start timecode in ticks, from the MXF (Premiere media <Start>)


@dataclass(frozen=True)
class AudioFile:
    path: str
    sample_rate: int
    duration: int
    bits: int
    avid_track: int | None = None
    start: int = 0
    gain_db: float = 0.0  # Avid master-clip gain on this track (PVOL), Premiere master-clip Audio Gain


@dataclass(frozen=True)
class AuxTimecode:
    track_index: int  # Avid import-mob track label; Aux TC number = index - 2 is a hypothesis
    timecode: str
    frames: int
    fps: int


@dataclass(frozen=True)
class AvidInfo:
    """Avid-side clip facts carried for metadata transfer (avid-reader `avid` block)."""
    mob_id: str
    columns: tuple[tuple[str, str], ...] = ()
    color_rgb16: tuple[int, int, int] | None = None
    start_timecode: str | None = None
    aux_timecodes: tuple[AuxTimecode, ...] = ()
    source_file: str | None = None
    source_path: str | None = None

    def column(self, name: str) -> str | None:
        return next((v for k, v in self.columns if k == name), None)


@dataclass(frozen=True)
class MultichannelGroup:
    format: str
    channels: tuple[int, ...]

    def premiere_channels(self):
        return tuple(self.channels[i] for i in (0, 2, 1, 5, 3, 4)) if self.format == '5.1' else self.channels


@dataclass(frozen=True)
class Clip:
    id: str
    name: str
    material_umid: str | None
    tape_name: str | None
    video: VideoFile | None
    audio: tuple[AudioFile, ...]
    avid: AvidInfo | None = None
    time_display: int | None = None  # Premiere TimeDisplay code for audio-only items (100 = 24 fps)
    stereo_pairs: tuple[tuple[int, int], ...] = ()  # Avid stereo groups, by avid_track (left, right)
    rate: Fraction | None = None  # Avid clip rate (FPS); timebase for audio-only builds
    markers: tuple["Marker", ...] = ()  # Avid master-clip markers
    multichannel_groups: tuple[MultichannelGroup, ...] = ()


@dataclass(frozen=True)
class VolumeKey:
    ticks: int  # clip source time, as Premiere keys clip effects (PRPROJ-0013)
    db: float


@dataclass(frozen=True)
class EventAudio:
    gain_db: float = 0.0
    volume_db: float | None = None
    volume_keys: tuple[VolumeKey, ...] = ()


@dataclass(frozen=True)
class ColorCorrection:
    """Avid Color Correction as the reader interprets it (avid-reader src/color.rs)."""
    saturation: float | None = None  # percent, 100 = unchanged
    tone: tuple[tuple[str, float], ...] = ()  # Avid HSL Controls brightness/contrast/gain/gamma/setup, as stored
    curves: tuple[tuple[str, tuple[tuple[float, float], ...]], ...] = ()  # master/red/green/blue, 0..1
    not_converted: tuple[str, ...] = ()


@dataclass(frozen=True)
class Transform:
    """Picture position, scale, rotation and crop (from Avid Resize / 3D Warp). Fractions are of
    the frame, measured from its centre with x right and y down."""
    position: tuple[float, float] = (0.0, 0.0)  # where the source's anchor lands, sequence frame
    scale: tuple[float, float] = (100.0, 100.0)  # percent, x and y
    rotation: float = 0.0  # degrees clockwise
    anchor: tuple[float, float] = (0.0, 0.0)  # pivot, source frame
    crop: tuple[float, float, float, float] = (0.0, 0.0, 0.0, 0.0)  # left, top, right, bottom, of the source
    not_converted: tuple[str, ...] = ()


@dataclass(frozen=True)
class Matte:
    """Avid AniMatte as the reader gives it (avid-reader src/animatte.rs): per shape, keys of
    (frame within the edit, points [x, y, in_x, in_y, out_x, out_y]) as fractions of the source
    frame from its top-left, y down, handles relative to the point."""
    shapes: tuple[tuple[tuple[int, tuple[tuple[float, ...], ...]], ...], ...]
    inverted: bool = False  # Avid Key Out
    opacities: tuple[float, ...] = ()  # per shape, percent (100 when the reader omits it)
    feathers: tuple[tuple[float, float], ...] = ()  # per shape, raw Avid values in stored order
    not_converted: tuple[str, ...] = ()


@dataclass(frozen=True)
class Event:
    clip: str
    record_start: int
    source_in: int
    duration: int
    video_track: int | None
    audio_tracks: tuple[int, ...]
    audio: EventAudio = EventAudio()
    # Clip audio files (by avid_track) playing on each of audio_tracks; empty = the clip's
    # first len(audio_tracks) files in model order.
    audio_channels: tuple[int, ...] = ()
    # Multichannel (stereo) edits: (sequence audio track, clip audio tracks in channel order,
    # edit gain dB or None, per-channel pan percent or None).
    stereo: tuple[tuple[int, tuple[int, ...], float | None, tuple[float | None, ...]], ...] = ()
    # The edit's own Avid gain (dB) and pan (-100..100) per audio track; None = not set on the edit.
    audio_gains: tuple[float | None, ...] = ()
    audio_pans: tuple[float | None, ...] = ()
    # Avid motion effect (KB AVB-0010): speed_ratio = timeline frames / source frames, negative =
    # reverse; source_duration = the source span it plays. None = normal speed.
    speed_ratio: tuple[int, int] | None = None
    source_duration: int | None = None
    color_correction: ColorCorrection | None = None
    transform: Transform | None = None
    matte: Matte | None = None
    # Avid Timewarp as sampled by Avid (KB AVB-0010): the source frame shown at each record frame
    # of the edit, in clip frames from the media start (like source_in). None = no retiming.
    time_remap: tuple[int, ...] | None = None


@dataclass(frozen=True)
class TrackLevel:
    track: int
    db: float


@dataclass(frozen=True)
class AudioCrossfade:
    track: int
    cut: int  # sequence ticks of the cut between two adjacent events on `track`
    duration: int
    cut_offset: int  # ticks from the crossfade start to the cut (Premiere `Alignment`)
    style: str = "constant_power"  # or "custom_fade" (Premiere 26.5.2 save form); see audio.add_crossfade


@dataclass(frozen=True)
class Marker:
    name: str
    comment: str
    start: int  # clip markers: master-clip-relative ticks; sequence markers: sequence ticks
    duration: int = 0
    user: str = ""
    color: str = ""  # Avid color name (16-name palette)
    track: str | None = None  # Avid track it sits on ("V1", "A2"), None = the sequence's own
    # Sequence markers: "sequence" (Avid's own sequence markers), "filler", or "edit" (on an edit;
    # `event` indexes the sequence's events, `source` = master-clip-relative ticks, `inherited` =
    # a copy of a marker the master clip carries).
    owner: str = "sequence"
    event: int | None = None
    source: int | None = None
    inherited: bool = False


@dataclass(frozen=True)
class Sequence:
    name: str
    rate: Fraction
    width: int
    height: int
    events: tuple[Event, ...]
    markers: tuple[Marker, ...]
    track_levels: tuple[TrackLevel, ...] = ()
    audio_crossfades: tuple[AudioCrossfade, ...] = ()
    start: int = 0  # sequence start timecode, ticks (Avid 01:00:00:00 -> 3600 s)


@dataclass(frozen=True)
class Bin:
    name: str
    clips: tuple[str, ...]


@dataclass(frozen=True)
class Options:
    #: separate: linked source clips in the timeline; merged/grouped: the timeline uses the
    #: combined clip (User decision 2026-10-14: no separate timeline checkbox).
    bin_mode: str = "separate"
    #: Avid pans mono tracks odd-left/even-right by default (a monitoring convention). Off: edits
    #: without an explicit Avid pan stay centred, Premiere's way (User decision 2026-10-14).
    match_avid_default_pan: bool = False
    #: Where an edit's own marker and a source marker share a frame: "edit" wins (default) or
    #: "source"; the other is dropped, or placed on the sequence (User decision 2026-10-15).
    marker_conflict_winner: str = "edit"
    marker_conflict_loser_to_sequence: bool = False
    #: Append "Avid: <user> · <color>" to Premiere marker comments.
    marker_avid_info: bool = False
    #: Color mapper overrides: Avid marker color name -> Premiere marker color index (0..7);
    #: Avid clip color "#RRGGBB" -> Premiere label index (0..15).
    marker_colors: tuple[tuple[str, int], ...] = ()
    clip_labels: tuple[tuple[str, int], ...] = ()


@dataclass(frozen=True)
class Model:
    project_name: str
    options: Options
    clips: dict[str, Clip]
    bins: tuple[Bin, ...]
    sequences: tuple[Sequence, ...] = field(default_factory=tuple)


def _video(obj: object, where: str) -> VideoFile:
    o = _keys(obj, where, {"path", "codec", "width", "height", "rate", "duration_ticks"}, {"start_ticks"})
    _require(o["codec"] in ("AVdn", "apcs"), f"{where}.codec: only 'AVdn' (Avid DNx) and 'apcs' (ProRes LT) OP-Atom MXF have a verified profile")
    for k in ("width", "height"):
        _require(isinstance(o[k], int) and 16 <= o[k] <= 16384, f"{where}.{k}: out of range")
    rate = _rate(o["rate"], f"{where}.rate")
    duration = _ticks(o["duration_ticks"], f"{where}.duration_ticks", positive=True)
    _require(duration % ticks_per_unit(rate) == 0, f"{where}.duration_ticks: not a whole number of frames")
    return VideoFile(_abs_path(o["path"], f"{where}.path"), o["codec"], o["width"], o["height"], rate, duration,
                     _ticks(o.get("start_ticks", 0), f"{where}.start_ticks"))


def _audio(obj: object, where: str) -> AudioFile:
    o = _keys(obj, where, {"path", "sample_rate", "duration_ticks", "bits"}, {"avid_track", "start_ticks", "gain_db"})
    _require(o["sample_rate"] in (44100, 48000, 96000), f"{where}.sample_rate: unsupported")
    _require(o["bits"] in (16, 24), f"{where}.bits: unsupported")
    track = o.get("avid_track")
    _require(track is None or (isinstance(track, int) and not isinstance(track, bool) and 1 <= track <= 64), f"{where}.avid_track: expected 1..64")
    path = _abs_path(o["path"], f"{where}.path")
    if track is None:
        # Avid OP-Atom audio files are named <clip name, truncated>A01.<id>.mxf, A02., ... by
        # track (the name prefix may be empty: "A01.E6F08062_….mxf", "68-1 SPOOGA01.E6F0963A6702A.mxf").
        m = re.search(r"A(\d{2})\.[^.]+\.mxf$", _basename(path), re.IGNORECASE)
        track = int(m.group(1)) if m else None
    return AudioFile(path, o["sample_rate"], _ticks(o["duration_ticks"], f"{where}.duration_ticks", positive=True), o["bits"], track,
                     _ticks(o.get("start_ticks", 0), f"{where}.start_ticks"), _db(o.get("gain_db", 0.0), f"{where}.gain_db"))


def _basename(path: str) -> str:
    return (PureWindowsPath(path) if "\\" in path else PurePosixPath(path)).name


def _check_audio_tracks(files: tuple, where: str) -> None:
    tracks = [f.avid_track for f in files if f.avid_track is not None]
    _require(len(set(tracks)) == len(tracks), f"{where}: duplicate Avid audio tracks {sorted(tracks)}")


def event_audio_files(clip: Clip, event: Event) -> list[AudioFile]:
    """The clip audio file playing on each of the event's audio tracks."""
    if not event.audio_channels:
        return list(clip.audio[:len(event.audio_tracks)])
    by_track = {f.avid_track: f for f in clip.audio}
    return [by_track[c] for c in event.audio_channels]


def audio_for_combined_clip(clip: "Clip") -> tuple:
    """Audio in Avid track order (A01 first), for combining a clip's video with its own audio
    into one merged or grouped clip. Bin items and timeline placement keep model order.
    Refuses rather than guesses when a track number is unknown."""
    files = clip.audio
    if len(files) > 1:
        _require(all(f.avid_track is not None for f in files),
                 f"clip {clip.name!r}: every audio file needs an Avid track (avid_track or an A01-style name) to be combined")
    return tuple(sorted(files, key=lambda f: f.avid_track or 0))


# Premiere time display codes (docs/time-display-formats.md in the research repo).
TIME_DISPLAY_CODES = frozenset({100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 200, 201})
TIMECODE_RE = re.compile(r"\d{2}:\d{2}:\d{2}[:;]\d{2}")


def _int(value: object, where: str, low: int, high: int) -> int:
    _require(isinstance(value, int) and not isinstance(value, bool) and low <= value <= high,
             f"{where}: expected an integer {low}..{high}")
    return value


def _timecode(value: object, where: str) -> str:
    _require(isinstance(value, str) and TIMECODE_RE.fullmatch(value) is not None, f"{where}: expected HH:MM:SS:FF")
    return value


def _avid(obj: object, where: str) -> AvidInfo:
    o = _keys(obj, where, {"mob_id"}, {"columns", "color_rgb16", "start", "aux_timecodes", "source_file", "source_path"})
    columns = o.get("columns", {})
    _require(isinstance(columns, dict) and len(columns) <= 512, f"{where}.columns: expected an object of at most 512 columns")
    # Avid allows empty column values (Phase 21 "Shoot Date"); metadata skips them.
    cols = tuple((_text(k, f"{where}.columns key", 256), _text(v, f"{where}.columns[{k!r}]", allow_empty=True)) for k, v in columns.items())
    color = o.get("color_rgb16")
    if color is not None:
        _require(isinstance(color, list) and len(color) == 3, f"{where}.color_rgb16: expected [r, g, b]")
        color = tuple(_int(c, f"{where}.color_rgb16", 0, 65535) for c in color)
    start = None
    if o.get("start") is not None:
        st = _keys(o["start"], f"{where}.start", {"timecode", "frames", "fps"})
        start = _timecode(st["timecode"], f"{where}.start.timecode")
    aux = o.get("aux_timecodes", [])
    _require(isinstance(aux, list) and len(aux) <= 64, f"{where}.aux_timecodes: expected a list")
    auxes = []
    for i, a in enumerate(aux):
        w = f"{where}.aux_timecodes[{i}]"
        a = _keys(a, w, {"track_index", "timecode", "start", "fps"}, {"aux_tc_hypothesis", "position", "length"})
        auxes.append(AuxTimecode(_int(a["track_index"], w, 0, 32767), _timecode(a["timecode"], w),
                                 _int(a["start"], w, 0, 2**32), _int(a["fps"], w, 1, 1000)))
    opt = lambda k: _text(o[k], f"{where}.{k}") if o.get(k) is not None else None  # noqa: E731
    return AvidInfo(_text(o["mob_id"], f"{where}.mob_id"), cols, color, start, tuple(auxes),
                    opt("source_file"), opt("source_path"))


def _clip(obj: object, where: str) -> Clip:
    o = _keys(obj, where, {"id", "name", "audio"}, {"video", "material_umid", "tape_name", "avid", "time_display", "stereo_pairs", "multichannel_groups", "rate", "markers"})
    umid = o.get("material_umid")
    if umid is not None:
        _require(isinstance(umid, str) and UMID_RE.fullmatch(umid) is not None, f"{where}.material_umid: expected 64 upper-case hex digits")
    tape = o.get("tape_name")
    if tape is not None:
        tape = _text(tape, f"{where}.tape_name")
    audio = o["audio"]
    _require(isinstance(audio, list) and len(audio) <= 64, f"{where}.audio: expected a list of at most 64 files")
    video = _video(o["video"], f"{where}.video") if o.get("video") is not None else None
    files = tuple(_audio(a, f"{where}.audio[{i}]") for i, a in enumerate(audio))
    _check_audio_tracks(files, f"{where}.audio")
    _require(video is not None or files, f"{where}: a clip needs at least one media file")
    avid = _avid(o["avid"], f"{where}.avid") if o.get("avid") is not None else None
    td = o.get("time_display")
    _require(td is None or td in TIME_DISPLAY_CODES, f"{where}.time_display: expected one of {sorted(TIME_DISPLAY_CODES)}")
    pairs = o.get("stereo_pairs", [])
    _require(isinstance(pairs, list) and all(isinstance(p, list) and len(p) == 2 and all(isinstance(t, int) and 1 <= t <= 64 for t in p)
                                             for p in pairs), f"{where}.stereo_pairs: expected [[left, right], ...] Avid tracks")
    known = {f.avid_track for f in files}
    _require(all(t in known for p in pairs for t in p), f"{where}.stereo_pairs: names an audio track the clip lacks")
    used = [t for p in pairs for t in p]
    _require(len(used) == len(set(used)), f"{where}.stereo_pairs: a track is in two pairs")
    values = o.get('multichannel_groups', [])
    _require(isinstance(values, list) and len(values) <= 32, f'{where}.multichannel_groups: expected at most 32 groups')
    occupied = set(used)
    groups = []
    for i, value in enumerate(values):
        w = f'{where}.multichannel_groups[{i}]'
        group = _keys(value, w, {'format', 'channels'})
        fmt = _text(group['format'], f'{w}.format', 16)
        _require(fmt in ('5.1','7.1'), f'{w}.format: expected 5.1 or 7.1')
        count = 6 if fmt == '5.1' else 8
        channels = group['channels']
        _require(isinstance(channels,list) and len(channels)==count and all(isinstance(n,int) and not isinstance(n,bool) and 1<=n<=64 for n in channels), f'{w}.channels: expected {count} Avid tracks')
        valid = all(n in known and n not in occupied for n in channels) and len(set(channels))==len(channels)
        _require(valid, f'{w}.channels: missing, repeated or overlapping member')
        occupied.update(channels)
        groups.append(MultichannelGroup(fmt,tuple(channels)))
    rate = _rate(o["rate"], f"{where}.rate") if o.get("rate") is not None else None
    markers = o.get("markers", [])
    _require(isinstance(markers, list) and len(markers) <= MAX_ITEMS, f"{where}.markers: expected a list")
    return Clip(_text(o["id"], f"{where}.id", 256), _text(o["name"], f"{where}.name"), umid, tape, video, files, avid, td,
                tuple((p[0], p[1]) for p in pairs), rate, tuple(_marker(m, f"{where}.markers[{i}]", False) for i, m in enumerate(markers)), tuple(groups))


def _db(value: object, where: str, low: float = -96.0, high: float = 96.0) -> float:
    _require(isinstance(value, (int, float)) and not isinstance(value, bool), f"{where}: expected a number of dB")
    _require(value == value and low <= value <= high, f"{where}: out of range {low}..{high} dB")
    return float(value)


def _event_audio(obj: object, where: str) -> EventAudio:
    o = _keys(obj, where, set(), {"gain_db", "volume_db", "volume_keys"})
    keys = o.get("volume_keys", [])
    _require(isinstance(keys, list) and len(keys) <= 65536, f"{where}.volume_keys: expected a list")
    parsed = []
    for i, k in enumerate(keys):
        ko = _keys(k, f"{where}.volume_keys[{i}]", {"source_ticks", "db"})
        parsed.append(VolumeKey(_ticks(ko["source_ticks"], f"{where}.volume_keys[{i}].source_ticks"), _db(ko["db"], f"{where}.volume_keys[{i}].db", -200.0, 24.0)))
    _require([k.ticks for k in parsed] == sorted({k.ticks for k in parsed}), f"{where}.volume_keys: times must be strictly increasing")
    _require(not (parsed and o.get("volume_db") is not None), f"{where}: give volume_db or volume_keys, not both")
    volume = o.get("volume_db")
    return EventAudio(_db(o.get("gain_db", 0.0), f"{where}.gain_db"), None if volume is None else _db(volume, f"{where}.volume_db", -200.0, 24.0), tuple(parsed))


def _pan(value: object, where: str) -> float:
    _require(isinstance(value, (int, float)) and not isinstance(value, bool) and -100.0 <= value <= 100.0,
             f"{where}: expected a pan percent -100..100")
    return float(value)


def _optional_list(value: object, length: int, check, where: str) -> tuple:
    _require(isinstance(value, list) and len(value) == length, f"{where}: expected {length} entries")
    return tuple(None if v is None else check(v, f"{where}[{i}]") for i, v in enumerate(value))


CURVE_NAMES = ("master", "red", "green", "blue")


def _color_correction(obj: object, where: str) -> ColorCorrection:
    o = _keys(obj, where, set(), {"saturation", "tone", "curves", "not_converted"})
    tone = o.get("tone", {})
    _require(isinstance(tone, dict) and set(tone) <= {"hue", "brightness", "contrast", "gain", "gamma", "setup"} and all(
        isinstance(v, (int, float)) and not isinstance(v, bool) and -1000 <= v <= 1000 for v in tone.values()), f"{where}.tone: expected Avid control values")
    _require(tone.get("gamma", 1) > 0, f"{where}.tone.gamma: must be positive")
    sat = o.get("saturation")
    _require(sat is None or (isinstance(sat, (int, float)) and not isinstance(sat, bool) and 0 <= sat <= 1000), f"{where}.saturation: expected a percent")
    curves = o.get("curves", {})
    _require(isinstance(curves, dict) and set(curves) <= set(CURVE_NAMES), f"{where}.curves: keys from {CURVE_NAMES}")
    parsed = []
    for name in CURVE_NAMES:
        if name not in curves:
            continue
        pts = curves[name]
        _require(isinstance(pts, list) and 2 <= len(pts) <= 32 and all(
            isinstance(p, list) and len(p) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and -4 <= v <= 4 for v in p) for p in pts),
            f"{where}.curves.{name}: expected 2..32 [x, y] points")
        _require(len({float(x) for x, _ in pts}) == len(pts),
                 f"{where}.curves.{name}: x coordinates must be distinct")
        parsed.append((name, tuple((float(x), float(y)) for x, y in pts)))
    nc = o.get("not_converted", [])
    _require(isinstance(nc, list) and all(isinstance(n, str) for n in nc), f"{where}.not_converted: expected text")
    return ColorCorrection(None if sat is None else float(sat), tuple((k, float(v)) for k, v in tone.items()), tuple(parsed), tuple(nc))


def _numbers(value: object, count: int, low: float, high: float, where: str) -> tuple[float, ...]:
    _require(isinstance(value, list) and len(value) == count and all(
        isinstance(v, (int, float)) and not isinstance(v, bool) and low <= v <= high for v in value), f"{where}: expected {count} numbers {low:g}..{high:g}")
    return tuple(float(v) for v in value)


def _transform(obj: object, where: str) -> Transform:
    o = _keys(obj, where, set(), {"position", "scale", "rotation", "anchor", "crop", "not_converted"})
    rotation = o.get("rotation", 0.0)
    _require(isinstance(rotation, (int, float)) and not isinstance(rotation, bool) and -32768 <= rotation <= 32767, f"{where}.rotation: expected degrees")
    nc = o.get("not_converted", [])
    _require(isinstance(nc, list) and all(isinstance(n, str) for n in nc), f"{where}.not_converted: expected text")
    return Transform(_numbers(o.get("position", [0, 0]), 2, -100, 100, f"{where}.position"),
                     _numbers(o.get("scale", [100, 100]), 2, 0, 10000, f"{where}.scale"), float(rotation),
                     _numbers(o.get("anchor", [0, 0]), 2, -100, 100, f"{where}.anchor"),
                     _numbers(o.get("crop", [0, 0, 0, 0]), 4, -100, 100, f"{where}.crop"), tuple(nc))


def _matte(obj: object, where: str) -> Matte:
    o = _keys(obj, where, {"shapes"}, {"inverted", "not_converted"})
    _require(isinstance(o.get("inverted", False), bool), f"{where}.inverted: expected true or false")
    _require(isinstance(o["shapes"], list) and 1 <= len(o["shapes"]) <= 64, f"{where}.shapes: expected 1..64 shapes")
    nc = o.get("not_converted", [])
    _require(isinstance(nc, list) and all(isinstance(n, str) for n in nc), f"{where}.not_converted: expected text")
    shapes, opacities, feathers = [], [], []
    for i, shape in enumerate(o["shapes"]):
        so = _keys(shape, f"{where}.shapes[{i}]", {"keyframes"}, {"opacity", "feather"})
        opacity = so.get("opacity", 100)
        _require(isinstance(opacity, (int, float)) and not isinstance(opacity, bool) and 0 <= opacity <= 100,
                 f"{where}.shapes[{i}].opacity: expected a percent 0..100")
        feather = so.get("feather", [0, 0])
        _require(isinstance(feather, list) and len(feather) == 2 and all(
            isinstance(v, (int, float)) and not isinstance(v, bool) and 0 <= v <= 100000 for v in feather),
            f"{where}.shapes[{i}].feather: expected two raw Avid values")
        opacities.append(float(opacity))
        feathers.append((float(feather[0]), float(feather[1])))
        keys = so["keyframes"]
        _require(isinstance(keys, list) and 1 <= len(keys) <= 100000, f"{where}.shapes[{i}].keyframes: expected 1.. keys")
        parsed = []
        for j, key in enumerate(keys):
            w = f"{where}.shapes[{i}].keyframes[{j}]"
            ko = _keys(key, w, {"frame", "points"})
            frame, points = ko["frame"], ko["points"]
            _require(isinstance(frame, int) and not isinstance(frame, bool) and 0 <= frame < 2**31
                     and (not parsed or frame > parsed[-1][0]), f"{w}.frame: expected increasing frames from 0")
            _require(isinstance(points, list) and 3 <= len(points) <= 4096 and all(
                isinstance(p, list) and len(p) == 6 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and -100 <= v <= 100 for v in p)
                for p in points) and (not parsed or len(points) == len(parsed[0][1])), f"{w}.points: expected 3..4096 [x, y, in_x, in_y, out_x, out_y], same count in every key")
            parsed.append((frame, tuple(tuple(float(v) for v in p) for p in points)))
        shapes.append(tuple(parsed))
    return Matte(tuple(shapes), o.get("inverted", False), tuple(opacities), tuple(feathers), tuple(nc))


def _event(obj: object, where: str) -> Event:
    o = _keys(obj, where, {"clip", "record_start_ticks", "source_in_ticks", "duration_ticks"}, {"video_track", "audio_tracks", "audio", "audio_channels", "stereo", "audio_gains_db", "audio_pans",
                                                                                                  "speed_ratio", "source_duration_ticks", "color_correction", "transform", "matte", "time_remap"})
    vt = o.get("video_track")
    _require(vt is None or (isinstance(vt, int) and 1 <= vt <= 99), f"{where}.video_track: expected 1..99")
    at = o.get("audio_tracks", [])
    _require(isinstance(at, list) and all(isinstance(t, int) and 1 <= t <= 99 for t in at) and len(set(at)) == len(at), f"{where}.audio_tracks: expected distinct 1..99")
    st = []
    for i, x in enumerate(o.get("stereo", [])):
        xo = _keys(x, f"{where}.stereo[{i}]", {"audio_track", "channels"}, {"gain_db", "pans"})
        chans = xo["channels"]
        _require(isinstance(xo["audio_track"], int) and 1 <= xo["audio_track"] <= 99, f"{where}.stereo[{i}].audio_track: expected 1..99")
        _require(isinstance(chans, list) and 2 <= len(chans) <= 8 and all(isinstance(c, int) and 1 <= c <= 64 for c in chans),
                 f"{where}.stereo[{i}].channels: expected 2..8 Avid tracks")
        gain = _db(xo["gain_db"], f"{where}.stereo[{i}].gain_db") if xo.get("gain_db") is not None else None
        pans = _optional_list(xo.get("pans", [None] * len(chans)), len(chans), _pan, f"{where}.stereo[{i}].pans")
        st.append((xo["audio_track"], tuple(chans), gain, pans))
    _require(vt is not None or at or st, f"{where}: an event needs a video or audio track")
    audio = _event_audio(o["audio"], f"{where}.audio") if "audio" in o else EventAudio()
    _require(not (audio != EventAudio() and not at), f"{where}.audio: the event has no audio tracks")
    ch = o.get("audio_channels", [])
    _require(isinstance(ch, list) and all(isinstance(c, int) and not isinstance(c, bool) and 1 <= c <= 64 for c in ch)
             and (not ch or len(ch) == len(at)), f"{where}.audio_channels: expected one Avid track 1..64 per audio track")
    gains = _optional_list(o.get("audio_gains_db", []), len(at), lambda v, w: _db(v, w), f"{where}.audio_gains_db") \
        if "audio_gains_db" in o else ()
    pans = _optional_list(o.get("audio_pans", []), len(at), _pan, f"{where}.audio_pans") if "audio_pans" in o else ()
    ratio = o.get("speed_ratio")
    source_duration = None
    if ratio is not None:
        _require(isinstance(ratio, list) and len(ratio) == 2 and all(isinstance(v, int) and not isinstance(v, bool) for v in ratio)
                 and ratio[0] != 0 and ratio[1] > 0, f"{where}.speed_ratio: expected [timeline, source] frames, timeline non-zero")
        _require(not at and not st, f"{where}: a speed change is only supported on picture (Avid does not retime audio here)")
        ratio = (ratio[0], ratio[1])
        source_duration = _ticks(o.get("source_duration_ticks"), f"{where}.source_duration_ticks", positive=True)
    color = _color_correction(o["color_correction"], f"{where}.color_correction") if o.get("color_correction") is not None else None
    _require(color is None or vt is not None, f"{where}.color_correction: the event has no picture")
    transform = _transform(o["transform"], f"{where}.transform") if o.get("transform") is not None else None
    _require(transform is None or vt is not None, f"{where}.transform: the event has no picture")
    remap = o.get("time_remap")
    if remap is not None:
        _require(isinstance(remap, list) and 2 <= len(remap) <= 100000 and all(
            isinstance(f, int) and not isinstance(f, bool) and 0 <= f < 2**40 for f in remap), f"{where}.time_remap: expected 2.. source frames")
        _require(vt is not None and not at and not st and o.get("speed_ratio") is None,
                 f"{where}.time_remap: only on a picture-only event without speed_ratio")
        remap = tuple(remap)
    matte = _matte(o["matte"], f"{where}.matte") if o.get("matte") is not None else None
    _require(matte is None or vt is not None, f"{where}.matte: the event has no picture")
    return Event(_text(o["clip"], f"{where}.clip", 256), _ticks(o["record_start_ticks"], f"{where}.record_start_ticks"),
                 _ticks(o["source_in_ticks"], f"{where}.source_in_ticks"), _ticks(o["duration_ticks"], f"{where}.duration_ticks", positive=True), vt, tuple(at), audio, tuple(ch), tuple(st),
                 gains, pans, ratio, source_duration, color, transform, matte, remap)


MARKER_OWNERS = ("sequence", "filler", "edit")


def _marker(obj: object, where: str, in_sequence: bool) -> Marker:
    o = _keys(obj, where, {"name", "start_ticks"}, {"comment", "user", "color", "rgb16", "duration_ticks", "avid_id", "track", "owner",
                                                    "event", "source_ticks", "inherited"})
    comment = o.get("comment", "")
    _require(isinstance(comment, str) and len(comment) <= 4096, f"{where}.comment: expected text")
    _require(not any((ord(c) < 32 and c not in "\t\n\r") or 0xD800 <= ord(c) <= 0xDFFF
                     or ord(c) in (0xFFFE, 0xFFFF) for c in comment)
             and re.search(r"\n\t+\Z", comment) is None,
             f"{where}.comment: text cannot be saved in XML")
    track = o.get("track")
    _require(track is None or (isinstance(track, str) and re.fullmatch(r"[VA]\d{1,2}", track) is not None), f"{where}.track: expected V1/A1-style")
    owner = o.get("owner", "sequence")
    _require(owner in MARKER_OWNERS and (in_sequence or owner == "sequence"), f"{where}.owner: one of {MARKER_OWNERS} (sequences only)")
    event = o.get("event")
    _require(event is None or (isinstance(event, int) and not isinstance(event, bool) and event >= 0), f"{where}.event: expected an index")
    inherited = o.get("inherited", False)
    _require(isinstance(inherited, bool), f"{where}.inherited: expected true or false")
    source = _ticks(o["source_ticks"], f"{where}.source_ticks") if o.get("source_ticks") is not None else None
    _require(owner != "edit" or (event is not None and source is not None and track is not None), f"{where}: an edit marker needs event, source_ticks and track")
    return Marker(_text(o["name"], f"{where}.name", allow_empty=True), comment, _ticks(o["start_ticks"], f"{where}.start_ticks"),
                  _ticks(o.get("duration_ticks", 0), f"{where}.duration_ticks"), _text(o.get("user", ""), f"{where}.user", 256, allow_empty=True),
                  _text(o.get("color", ""), f"{where}.color", 64, allow_empty=True), track, owner, event, source, inherited)


def _sequence(obj: object, where: str) -> Sequence:
    o = _keys(obj, where, {"name", "rate", "width", "height", "events"}, {"markers", "track_levels", "audio_crossfades", "start_ticks", "stereo_audio_tracks"})
    rate = _rate(o["rate"], f"{where}.rate")
    events = o["events"]
    _require(isinstance(events, list) and len(events) <= MAX_ITEMS, f"{where}.events: expected a list")
    markers = o.get("markers", [])
    _require(isinstance(markers, list) and len(markers) <= MAX_ITEMS, f"{where}.markers: expected a list")
    ms = [_marker(m, f"{where}.markers[{i}]", True) for i, m in enumerate(markers)]
    _require(all(m.event is None or m.event < len(events) for m in ms), f"{where}.markers: event index out of range")
    for k in ("width", "height"):
        _require(isinstance(o[k], int) and 16 <= o[k] <= 16384, f"{where}.{k}: out of range")
    levels = []
    for i, t in enumerate(o.get("track_levels", [])):
        to = _keys(t, f"{where}.track_levels[{i}]", {"audio_track", "db"})
        _require(isinstance(to["audio_track"], int) and 1 <= to["audio_track"] <= 99, f"{where}.track_levels[{i}].audio_track: expected 1..99")
        levels.append(TrackLevel(to["audio_track"], _db(to["db"], f"{where}.track_levels[{i}].db", -200.0, 15.0)))
    fades = []
    for i, f in enumerate(o.get("audio_crossfades", [])):
        fo = _keys(f, f"{where}.audio_crossfades[{i}]", {"audio_track", "cut_ticks", "duration_ticks", "cut_offset_ticks"}, {"style"})
        _require(fo.get("style", "constant_power") in ("constant_power", "custom_fade"), f"{where}.audio_crossfades[{i}].style: constant_power or custom_fade")
        _require(isinstance(fo["audio_track"], int) and 1 <= fo["audio_track"] <= 99, f"{where}.audio_crossfades[{i}].audio_track: expected 1..99")
        duration = _ticks(fo["duration_ticks"], f"{where}.audio_crossfades[{i}].duration_ticks", positive=True)
        offset = _ticks(fo["cut_offset_ticks"], f"{where}.audio_crossfades[{i}].cut_offset_ticks")
        _require(offset <= duration, f"{where}.audio_crossfades[{i}]: cut_offset_ticks exceeds duration")
        fades.append(AudioCrossfade(fo["audio_track"], _ticks(fo["cut_ticks"], f"{where}.audio_crossfades[{i}].cut_ticks"), duration, offset,
                                    fo.get("style", "constant_power")))
    return Sequence(_text(o["name"], f"{where}.name"), rate, o["width"], o["height"],
                    tuple(_event(e, f"{where}.events[{i}]") for i, e in enumerate(events)), tuple(ms), tuple(levels), tuple(fades),
                    _ticks(o.get("start_ticks", 0), f"{where}.start_ticks"))


def parse_model(data: dict) -> Model:
    o = _keys(data, "model", {"schema", "project", "clips", "bins"}, {"options", "sequences"})
    _require(o["schema"] == SCHEMA, f"model.schema: expected {SCHEMA}")
    project = _keys(o["project"], "model.project", {"name"})
    opts = _keys(o.get("options", {}), "model.options", set(), {"bin_mode", "match_avid_default_pan", "marker_conflict_winner",
                                                                "marker_conflict_loser_to_sequence", "marker_avid_info", "marker_colors", "clip_labels"})
    mode = opts.get("bin_mode", "separate")
    _require(mode in BIN_MODES, f"model.options.bin_mode: one of {BIN_MODES}")
    match_pan = opts.get("match_avid_default_pan", False)
    _require(isinstance(match_pan, bool), "model.options.match_avid_default_pan: expected true or false")
    winner = opts.get("marker_conflict_winner", "edit")
    _require(winner in ("edit", "source"), "model.options.marker_conflict_winner: edit or source")
    flags = {}
    for k in ("marker_conflict_loser_to_sequence", "marker_avid_info"):
        flags[k] = opts.get(k, False)
        _require(isinstance(flags[k], bool), f"model.options.{k}: expected true or false")
    maps = {}
    for k, high in (("marker_colors", 7), ("clip_labels", 15)):
        m = opts.get(k, {})
        _require(isinstance(m, dict) and len(m) <= 1024 and all(isinstance(v, int) and not isinstance(v, bool) and 0 <= v <= high for v in m.values()),
                 f"model.options.{k}: expected {{name: index 0..{high}}}")
        maps[k] = tuple((_text(n, f"model.options.{k} key", 64), v) for n, v in m.items())
    _require(isinstance(o["clips"], list) and len(o["clips"]) <= MAX_ITEMS, "model.clips: expected a list")
    clips: dict[str, Clip] = {}
    for i, c in enumerate(o["clips"]):
        clip = _clip(c, f"model.clips[{i}]")
        _require(clip.id not in clips, f"model.clips[{i}].id: duplicate id {clip.id!r}")
        clips[clip.id] = clip
    _require(isinstance(o["bins"], list) and len(o["bins"]) <= MAX_ITEMS, "model.bins: expected a list")
    bins = []
    for i, b in enumerate(o["bins"]):
        bo = _keys(b, f"model.bins[{i}]", {"name", "clips"})
        _require(isinstance(bo["clips"], list), f"model.bins[{i}].clips: expected a list")
        for ref in bo["clips"]:
            _require(ref in clips, f"model.bins[{i}].clips: unknown clip {ref!r}")
        bins.append(Bin(_text(bo["name"], f"model.bins[{i}].name"), tuple(bo["clips"])))
    seqs = o.get("sequences", [])
    _require(isinstance(seqs, list) and len(seqs) <= 10_000, "model.sequences: expected a list")
    sequences = tuple(_sequence(s, f"model.sequences[{i}]") for i, s in enumerate(seqs))
    for s in sequences:
        unit = ticks_per_unit(s.rate)
        for i, e in enumerate(s.events):
            _require(e.clip in clips, f"sequence {s.name!r} event {i}: unknown clip {e.clip!r}")
            clip = clips[e.clip]
            _require(e.video_track is None or clip.video is not None, f"sequence {s.name!r} event {i}: clip has no video")
            _require(len(e.audio_tracks) <= len(clip.audio), f"sequence {s.name!r} event {i}: more audio tracks than clip audio files")
            known = {f.avid_track for f in clip.audio}
            _require(all(c in known for c in e.audio_channels), f"sequence {s.name!r} event {i}: audio_channels names a track the clip lacks")
            _require(all(c in known for _, chans, *_rest in e.stereo for c in chans), f"sequence {s.name!r} event {i}: stereo names a track the clip lacks")
            for name, v in (("record_start", e.record_start), ("duration", e.duration)):
                _require(v % unit == 0, f"sequence {s.name!r} event {i}: {name} is not on the sequence frame grid")
            media = max([f.duration for f in clip.audio] + ([clip.video.duration] if clip.video else []))
            span = e.source_duration if e.source_duration is not None else e.duration
            _require(e.source_in + span <= media, f"sequence {s.name!r} event {i}: source range exceeds media")
    options = Options(mode, match_pan, winner, flags["marker_conflict_loser_to_sequence"], flags["marker_avid_info"],
                      maps["marker_colors"], maps["clip_labels"])
    return Model(_text(project["name"], "model.project.name"), options, clips, tuple(bins), sequences)


def load_model(path: Path) -> Model:
    size = path.stat().st_size
    if size > MAX_MODEL_BYTES:
        raise ModelError("model file exceeds 16 MiB")

    def pairs(items):
        out = {}
        for k, v in items:
            if k in out:
                raise ModelError(f"duplicate JSON key {k!r}")
            out[k] = v
        return out

    try:
        data = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=pairs, parse_constant=lambda c: (_ for _ in ()).throw(ModelError(f"invalid JSON constant {c}")))
    except json.JSONDecodeError as error:
        raise ModelError(f"invalid JSON: {error}") from None
    return parse_model(data)
