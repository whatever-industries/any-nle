"""Clip gain, clip volume (+ keyframes), track level and audio crossfades.

Shapes from the User's Premiere 26.5.2 project (research PRPROJ-0013):
- clip volume: a materialized intrinsic "Internal Volume Mono" AudioFilterComponent with
  params Mute and Level; Level is linear with dB = 20*log10(v) + 15; keyframes are
  ';'-separated 8-field records in clip source time;
- audio gain: a linear <Gain> on the AudioClip object, after AudioChannelLayout;
- track level: the track AudioFader's Volume param, linear (dB = 20*log10(v)).
"""
from __future__ import annotations

import math
import xml.etree.ElementTree as ET

from py_premiere.models import media_import as mi

from .model import AudioCrossfade, EventAudio

PREMIERE_VOLUME_MAX_DB = 6.0
_FILTER_CLASS_ID = "d77a90a0-6c9e-44bf-9b20-de8c21168fe1"
_MUTE_CLASS_ID = "32657501-3aa4-445f-a49b-d09ecb9fa1ae"
_LEVEL_CLASS_ID = "a714635e-a628-4b27-9d59-77eba47dbc1a"
_START = "-91445760000000000"
_CHANNEL_CONFIG = '{"in":[{"layout":[0],"name":"Mono In","type":0}],"out":[{"layout":[0],"name":"Mono Out","type":0}]}'


def level_value(db: float) -> float:
    """Premiere clip-volume Level for `db` (0 dB -> 0.177827939...)."""
    return 10 ** ((db - 15.0) / 20.0)


def linear(db: float) -> float:
    return 10 ** (db / 20.0)


def fmt(value: float) -> str:
    return repr(float(value))


def split_overflow(audio: EventAudio) -> tuple[float, EventAudio, float]:
    """Premiere's clip volume tops out at +6 dB; Avid's goes higher. Volume and gain
    multiply, so any excess moves into gain without changing what plays.
    Returns (gain_db, volume, shift_db)."""
    peaks = [k.db for k in audio.volume_keys] + ([audio.volume_db] if audio.volume_db is not None else [])
    shift = max(0.0, max(peaks, default=0.0) - PREMIERE_VOLUME_MAX_DB)
    if not shift:
        return audio.gain_db, audio, 0.0
    from dataclasses import replace
    from .model import VolumeKey
    volume = replace(audio,
                     volume_db=None if audio.volume_db is None else audio.volume_db - shift,
                     volume_keys=tuple(VolumeKey(k.ticks, k.db - shift) for k in audio.volume_keys))
    return audio.gain_db + shift, volume, shift


def set_clip_gain(placed, db: float) -> None:
    if not db:
        return
    clip = placed._clip_element
    gain = clip.find("Gain")
    if gain is None:
        gain = ET.SubElement(clip, "Gain")
        previous = list(clip)[-2]
        gain.tail, previous.tail = previous.tail, previous.tail
    gain.text = fmt(linear(db))


def set_clip_volume(project, placed, audio: EventAudio) -> None:
    """Materialize the clip Volume intrinsic, static or keyframed."""
    if audio.volume_db is None and not audio.volume_keys:
        return
    document = project._document
    chain = document.resolve(placed._element.find("ClipTrackItem/ComponentOwner/Components"))
    for tag in ("DefaultVol", "DefaultVolumeComponentID", "DefaultChannelVolumeComponentID"):
        e = chain.find(tag)
        if e is not None:
            chain.remove(e)
    inner = chain.find("ComponentChain")
    if inner is None:
        inner = ET.SubElement(chain, "ComponentChain", {"Version": "3"})
    components = inner.find("Components")
    if components is None:
        components = ET.SubElement(inner, "Components", {"Version": "1"})

    mute = mi._top("AudioComponentParam", _MUTE_CLASS_ID, "10")
    mi._leaf(mute, "IsTimeVarying", "false", "\n\t\t")
    mi._leaf(mute, "Name", "Mute", "\n\t\t")
    mi._leaf(mute, "RangeLocked", "false", "\n\t")
    level = mi._top("AudioComponentParam", _LEVEL_CLASS_ID, "10")
    if audio.volume_keys:
        first = level_value(audio.volume_keys[0].db)
        mi._leaf(level, "StartKeyframe", f"{_START},{fmt(first)},0,0,0,0,0,0", "\n\t\t")
        mi._leaf(level, "CurrentValue", fmt(first), "\n\t\t")
        # Linear keys: speeds 0, influence 1/6 (Premiere's default influence).
        keys = "".join(f"{k.ticks},{fmt(level_value(k.db))},0,0,0,0.16666666666666666,0,0.16666666666666666;"
                       for k in audio.volume_keys)
        mi._leaf(level, "Keyframes", keys, "\n\t\t")  # no IsTimeVarying: Premiere omits it here
    else:
        value = level_value(audio.volume_db)
        mi._leaf(level, "StartKeyframe", f"{_START},{fmt(value)},0,0,0,0,0,0", "\n\t\t")
        mi._leaf(level, "CurrentValue", fmt(value), "\n\t\t")
    mi._leaf(level, "Name", "Level", "\n\t\t")
    mi._leaf(level, "UnitsString", "dB", "\n\t")
    mute_id, level_id = project._register(mute), project._register(level)

    component = mi._top("AudioFilterComponent", _FILTER_CLASS_ID, "4")
    audio_component = ET.SubElement(component, "AudioComponent", {"Version": "3"})
    core = ET.SubElement(audio_component, "Component", {"Version": "7"})
    params = ET.SubElement(core, "Params", {"Version": "1"})
    ET.SubElement(params, "Param", {"Index": "0", "ObjectRef": mute_id})
    ET.SubElement(params, "Param", {"Index": "1", "ObjectRef": level_id})
    mi._leaf(core, "ID", "1", "")
    mi._leaf(core, "Intrinsic", "true", "")
    mi._leaf(audio_component, "AudioChannelLayout", mi.audio_channel_layout(1), "")
    mi._leaf(audio_component, "AudioComponentType", "0", "")
    mi._leaf(audio_component, "FrameRate", "5292000", "")
    mi._leaf(audio_component, "ChannelType", "0", "")
    mi._leaf(component, "FilterPreset", "0", "")
    mi._leaf(component, "ChannelConfigData", _CHANNEL_CONFIG, "")
    mi._leaf(component, "FilterMatchName", "Internal Volume Mono", "")
    mi._leaf(component, "FilterIndex", "-1", "")
    component_id = project._register(component)
    ET.SubElement(components, "Component", {"Index": str(len(components)), "ObjectRef": component_id})


def set_track_level(project, track, db: float) -> None:
    """Set an audio track's fader Volume (linear, dB = 20*log10(v))."""
    document = project._document
    chain = document.resolve(track._element.find("AudioTrack/ComponentOwner/Components"))
    for ref in chain.findall("ComponentChain/Components/Component"):
        component = document.resolve(ref)
        if component.tag != "AudioFader":
            continue
        for p in component.iter("Param"):
            param = document.resolve(p)
            if param.findtext("Name") == "Volume":
                value = fmt(linear(db))
                for tag, text in (("StartKeyframe", f"{_START},{value},0,0,0,0,0,0"), ("CurrentValue", value)):
                    e = param.find(tag)
                    if e is None:
                        e = ET.Element(tag)
                        e.tail = param[0].tail if len(param) else None
                        param.insert(0, e)
                    e.text = text
                return
    raise ValueError("audio track has no fader Volume parameter")


def add_crossfade(track, fade: AudioCrossfade, style: str = "constant_power") -> None:
    """An audio crossfade over the cut at `fade.cut`, positioned exactly:
    Start = cut - offset, End = Start + duration, Alignment = offset.
    `style`: "constant_power" (py_premiere's Premiere 26.3-verified shape) or "custom_fade"
    (what Premiere 26.5.2 saved for a Constant Power crossfade: MatchName/DisplayName
    "Custom Fade" plus FadeShapeValue 14; PRPROJ-0013). Which one 26.5.2 needs is unconfirmed."""
    from py_premiere import Time
    outgoing = next((c for c in track.clips if c.end.ticks == fade.cut), None)
    incoming = next((c for c in track.clips if c.start.ticks == fade.cut), None)
    if outgoing is None or incoming is None:
        raise ValueError(f"no cut at {fade.cut} ticks on {track.name}")
    if fade.cut_offset > outgoing.duration.ticks or fade.duration - fade.cut_offset > incoming.duration.ticks:
        raise ValueError(f"crossfade at {fade.cut} is longer than the clips around it")
    transition = track.add_transition(incoming, "Constant Power", at_start=True, duration=Time(fade.duration))
    element = transition._element
    span = element.find("TransitionTrackItem/TrackItem")
    start = fade.cut - fade.cut_offset
    for tag, value in (("Start", start), ("End", start + fade.duration)):
        e = span.find(tag)
        if e is None:
            e = ET.SubElement(span, tag)
        e.text = str(value)
    alignment = element.find("TransitionTrackItem/Alignment")
    if alignment is not None:
        alignment.text = str(fade.cut_offset)
    if style == "custom_fade":
        for tag in ("DisplayName", "MatchName"):
            e = element.find(f"TransitionTrackItem/{tag}")
            if e is not None:
                e.text = "Custom Fade"
        shape = element.find("FadeShapeValue")
        if shape is None:
            shape = ET.SubElement(element, "FadeShapeValue")
            previous = list(element)[-2]
            shape.tail, previous.tail = previous.tail, previous.tail
        shape.text = "14"
    elif style != "constant_power":
        raise ValueError(f"unknown crossfade style {style!r}")
