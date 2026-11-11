"""Independent typed original-file master construction oracle.

Sample representation is explicit; AAC never receives a PCM bit depth. This builds
ordinary source masters only and shares no hidden sequence/group construction.
"""
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path
import uuid
from py_premiere.models import media_import as mi
from .model import ticks_per_unit
from .opatom import _set_master_gain

@dataclass(frozen=True)
class Video:
    codec: str
    width: int
    height: int
    rate: Fraction
    duration: int
    stream_index: int = 0

@dataclass(frozen=True)
class Audio:
    sample_rate: int
    channels: int
    duration: int
    stream_index: int
    representation: str

@dataclass(frozen=True)
class Channel:
    avid_track: int
    channel_index: int
    gain_db: float = 0.0

@dataclass(frozen=True)
class Original:
    path: str
    container: str
    video: Video | None
    audio: Audio | None
    channels: tuple[Channel, ...]
    groups: tuple[tuple[int, ...], ...]


def validate(c):
    if not Path(c.path).is_absolute() or c.container not in ('mov', 'wav'):
        raise ValueError('original requires an absolute path and qualified container')
    if c.video is None and c.audio is None:
        raise ValueError('original has no streams')
    if c.video:
        v = c.video
        qualified_codec = v.codec == 'avc1' or (
            v.codec == 'apch' and Path(c.path).suffix.lower() == '.mov' and c.audio is None
            and (v.width, v.height, v.rate) == (1920, 1080, Fraction(24))
        )
        if c.container != 'mov' or not qualified_codec or v.stream_index != 0:
            raise ValueError('unqualified original picture')
    if c.audio:
        a = c.audio
        if a.channels not in (1, 2) or a.sample_rate not in (44100, 48000, 96000):
            raise ValueError('unqualified original sound')
        if a.stream_index != (1 if c.video else 0):
            raise ValueError('unqualified original audio stream')
        if (c.container == 'mov' and (a.representation != 'aac' or a.channels != 2)) or (c.container == 'wav' and a.representation not in ('pcm16', 'pcm24')):
            raise ValueError('unqualified original sample representation')
        if len(c.channels) != a.channels or {ch.channel_index for ch in c.channels} != set(range(a.channels)):
            raise ValueError('incomplete original channels')
    elif c.channels or c.groups:
        raise ValueError('original mapping without audio')
    labels = [ch.avid_track for ch in c.channels]
    grouped = [n for g in c.groups for n in g]
    if len(set(labels)) != len(labels) or any(n < 1 or n > 64 for n in labels) or sorted(grouped) != sorted(labels) or any(len(g) not in (1, 2) for g in c.groups):
        raise ValueError('invalid original channel groups')


def add_item(project, c, name):
    validate(c)
    directory = str(project.path.parent)
    state = str(uuid.uuid4())
    duration = c.video.duration if c.video else c.audio.duration
    frame_rate = ticks_per_unit(c.video.rate if c.video else Fraction(c.audio.sample_rate))
    label = (1, 6769408) if c.video and c.audio else (0, 11405886) if c.video else (2, 480554)
    audio_stream = None
    if c.audio:
        a = c.audio
        sample = {'aac': '7', 'pcm16': '3', 'pcm24': None}[a.representation]
        audio_stream = project._register(mi.new_audio_stream(ticks_per_unit(Fraction(a.sample_rate)), mi.audio_channel_layout(a.channels), a.duration, sample))
    uid = str(uuid.uuid4())
    if c.video:
        v = c.video
        stream = project._register(mi.new_movie_video_stream(v.width, v.height, ticks_per_unit(v.rate), v.duration, c.container, v.codec, 0))
        element = mi.new_movie_media(uid, stream, c.path, directory, state, audio_stream, ticks_per_unit(Fraction(c.audio.sample_rate)) if c.audio else None, 0)
    else:
        key, binary_hash = str(uuid.uuid4()), str(uuid.uuid4())
        element = mi.new_audio_media(uid, audio_stream, c.path, directory, state, ticks_per_unit(Fraction(c.audio.sample_rate)), key, binary_hash, 0)
    media = project._register_uid(element, uid)
    project._imported_media.append(media)
    markers = project._register(mi.new_markers_collection(state))
    # Existing explicit display codes; no source-timecode inference from file PTS.
    from .rewrap import display_format
    display = str(display_format(c.video.rate)) if c.video else '200'
    logging = project._register(mi.new_media_logging_info(name, frame_rate, duration, None if c.video and c.audio else '2' if c.video else '1', display, 0, ''))
    clips, chains, vectors = [], [], []
    if c.video:
        source = project._register(mi.new_movie_media_source(uid, c.video.duration))
        clips.append(project._register(mi.new_movie_template_clip(markers, source, *label)))
    if c.audio:
        a = c.audio
        source = project._register(mi.new_audio_media_source(uid, a.duration))
        by_lane = {ch.avid_track: ch for ch in c.channels}
        for group, g in enumerate(c.groups):
            secondary = [project._register(mi.new_secondary_content(source, n)) for n in range(a.channels)]
            element = mi.new_audio_template_clip(markers, source, secondary, *label, mi.audio_channel_layout(a.channels))
            gain = by_lane[g[0]].gain_db
            if gain:
                _set_master_gain(element, gain)
            clips.append(project._register(element))
            members = [project._register(mi.new_channel_serializer(group, by_lane[n].channel_index)) for n in g]
            kind = len(g) - 1
            vectors.append(project._register(mi.new_channel_vector(members, kind)))
            chains.append(project._register(mi.new_master_audio_chain(len(g), kind)))
    groups = project._register(mi.new_audio_channel_groups(vectors) if vectors else mi.new_channel_groups())
    master_uid = str(uuid.uuid4())
    element = mi.new_audio_master_clip(master_uid, logging, chains, clips, groups, name) if chains else mi.new_master_clip(master_uid, logging, clips[0], groups, name)
    master = project._register_uid(element, master_uid)
    item_uid = str(uuid.uuid4())
    element = project._register_uid(mi.new_clip_item(item_uid, master_uid, name, label[0]), item_uid)
    item = project._attach_imported_item(element, item_uid, master, clips, c.path, duration)
    item._channel_groups = [[next(i for i, ch in enumerate(c.channels) if ch.avid_track == n) for n in g] for g in c.groups]
    return item
