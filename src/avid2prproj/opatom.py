"""Construct project items for Avid OP-Atom MXF files without reading the media.

Shapes follow Premiere 26.5.2's own import of one Avid clip (V01 DNx + A01/A02 mono),
research finding PRPROJ-0009. Everything Premiere stamps that is a machine-local cache
(conform/peak paths) or a change-detection token (content state, FileKey, BinaryHash)
is freshly minted; Premiere refreshes these on open (PRPROJ-0008).
"""
from __future__ import annotations

import base64
import uuid
from fractions import Fraction
from pathlib import PurePosixPath, PureWindowsPath
import xml.etree.ElementTree as ET

from py_premiere.models import media_import as mi
from py_premiere.models.project_item import ProjectItem
from py_premiere.models.media_paths import relative_media_path

from .model import AudioFile, Clip, VideoFile, ticks_per_unit, TICKS_PER_SECOND

#: Observed in Premiere's DNx OP-Atom import (PRPROJ-0009).
DNX_COLOR_SPACE = ('{"baseColorProfile":{"colorProfileData":"AQAAAP////8=","colorProfileName":'
                   '"BT.709,8-bit,Display-Referred"},"baseProfileType":1,"colorSpaceMetadata":{"dataRangeInfoValid":1}}')
#: Premiere 26.5's own import of an Avid ProRes LT OP-Atom MXF (User save 2026-10-20,
#: AT_TRACKED media): CodecType 'apcs' and this colour space.
PRORES_COLOR_SPACE = '{"baseColorProfile":{"colorProfileName":"BT.709 RGB Full"},"baseProfileType":1}'
COLOR_SPACE = {"AVdn": DNX_COLOR_SPACE, "apcs": PRORES_COLOR_SPACE}
#: Media TimecodeFormat per video rate. Only rates seen in a Premiere save are listed.
VIDEO_TIMECODE_FORMAT = {Fraction(24): "100"}
AUDIO_TIMECODE_FORMAT = "200"
TAPE_NAME_LIMIT = 40  # Premiere truncates the clip name to 40 characters (PRPROJ-0009)
#: Label defaults seen in the User's 26.5.2 import: video Iris (0), audio Forest (2).
VIDEO_LABEL = (0, 11405886)
AUDIO_LABEL = (2, 480554)
SAMPLE_TYPE = {16: "3", 24: None}


class UnverifiedShape(NotImplementedError):
    pass


def source_item_names(clip: Clip) -> tuple[str, list[str]]:
    """Bin names for a clip's separate source items (User rules, 2026-10-14): video is
    Name_V; audio is Name_A01, Name_A02, ... by Avid track, else by model position.
    A merged or grouped item takes the Avid Name itself. Media Title and logging ClipName
    always keep the Avid Name."""
    names = []
    for i, audio in enumerate(clip.audio):
        names.append(f"{clip.name}_A{(audio.avid_track or i + 1):02d}")
    return f"{clip.name}_V", names


def _basename(path: str) -> str:
    return (PureWindowsPath(path) if "\\" in path else PurePosixPath(path)).name


def _relative(path: str, project_dir: str) -> str:
    return relative_media_path(path, project_dir)


def _opatom_media(media_uid: str, stream_tag: str, stream_id: str, path: str, project_dir: str,
                  clip: Clip, content_state: str, conformed_rate: int | None, start: int = 0) -> ET.Element:
    element = mi._top("Media", mi._MEDIA_CLASS_ID, "30", uid=media_uid)
    ET.SubElement(element, stream_tag, {"ObjectRef": stream_id}).tail = "\n\t\t"
    state = ET.SubElement(element, "ModificationState", {"Encoding": "base64", "BinaryHash": str(uuid.uuid4())})
    state.text = base64.b64encode(content_state.encode("utf-16-le")).decode("ascii") + "\n\t\t"
    state.tail = "\n\t\t"
    tape = (clip.tape_name or clip.name)[:TAPE_NAME_LIMIT]
    mi._leaf(element, "RelativePath", _relative(path, project_dir), "\n\t\t")
    # Premiere's own MXF import writes the file's start timecode here and AlternateStart 0
    # (the per-clip timecode override, off); PRPROJ-0017.
    mi._leaf(element, "Start", str(start), "\n\t\t")
    mi._leaf(element, "ImplementationID", mi._BMP_IMPLEMENTATION_ID, "\n\t\t")
    mi._leaf(element, "FileKey", str(uuid.uuid4()), "\n\t\t")
    mi._leaf(element, "AlternateTapeName", tape, "\n\t\t")
    if conformed_rate is not None:
        mi._leaf(element, "ConformedAudioRate", str(conformed_rate), "\n\t\t")
    mi._leaf(element, "ContentAndMetadataState", content_state, "\n\t\t")
    mi._leaf(element, "TapeName", tape, "\n\t\t")
    mi._leaf(element, "ActualMediaFilePath", path, "\n\t\t")
    mi._leaf(element, "FilePath", path, "\n\t\t")
    mi._leaf(element, "AlternateStart", "0", "\n\t\t")
    mi._leaf(element, "Title", clip.name, "\n\t")
    return element


def _logging(clip: Clip, frame_rate: int, duration: int, capture_mode: str, timecode_format: str, start: int = 0) -> ET.Element:
    # Media in/out are absolute: they begin at the media start timecode.
    element = mi.new_media_logging_info(clip.name, frame_rate, duration, capture_mode, timecode_format, start, clip.material_umid or "")
    list(element)[-1].tail = "\n\t\t"
    mi._leaf(element, "TapeName", (clip.tape_name or clip.name)[:TAPE_NAME_LIMIT], "\n\t")
    return element


def _set_master_gain(audio_clip: ET.Element, db: float) -> None:
    """Master-clip Audio Gain: linear <Gain> after AudioChannelLayout on the master's AudioClip,
    as Premiere 26.5.2 wrote for Clip > Audio Options > Audio Gain -6 dB (PRPROJ-0020).
    New timeline placements copy it; the writer sets it on placements too."""
    layout = audio_clip.find("AudioChannelLayout")
    leaf = ET.Element("Gain")
    leaf.text = repr(10 ** (db / 20.0))
    if layout is not None:
        leaf.tail = layout.tail
        layout.tail = audio_clip[list(audio_clip).index(layout) - 1].tail if list(audio_clip).index(layout) > 0 else layout.tail
        audio_clip.insert(list(audio_clip).index(layout) + 1, leaf)
    else:
        audio_clip.append(leaf)


def _set_time_display(master: ET.Element, code: int) -> None:
    """Per-clip Time Display Format (Clip > Modify > Timecode). Premiere 26.5.2 wrote
    <TimeDisplay>100</TimeDisplay> after Name/DefMappingID, before MasterClipChangeVersion,
    for "24 fps Timecode" on an audio-only OP-Atom clip (PRPROJ-0017)."""
    version = master.find("MasterClipChangeVersion")
    index = list(master).index(version)
    leaf = ET.Element("TimeDisplay")
    leaf.text = str(code)
    leaf.tail = master[index - 1].tail
    master.insert(index, leaf)


def _video_stream(video: VideoFile) -> ET.Element:
    # Version 22 is what py_premiere's verified 26.3 imports write and what 26.5.2 opened in
    # PRPROJ-0008; Premiere 26.5.2 re-saves it as 23.
    element = mi._top("VideoStream", mi._STREAM_CLASS_ID, "22")
    mi._leaf(element, "FrameRate", str(ticks_per_unit(video.rate)), "\n\t\t")
    mi._leaf(element, "Duration", str(video.duration), "\n\t\t")
    mi._leaf(element, "FrameRect", f"0,0,{video.width},{video.height}", "\n\t\t")
    mi._leaf(element, "CodecType", str(int.from_bytes(video.codec.encode("ascii"), "big")), "\n\t\t")
    mi._leaf(element, "OriginalColorSpace", COLOR_SPACE[video.codec], "\n\t")
    return element


def _shared_media(project, path: str, build) -> tuple[str, str]:
    """One Media object per media file per project, as Premiere's own import keeps it; every
    clip of that file (several Avid master clips, copies in several bins) points at it.
    Returns (media uid, content state)."""
    cache = project.__dict__.setdefault("_any_nle_media", {})
    if path not in cache:
        media_uid, content_state = str(uuid.uuid4()), str(uuid.uuid4())
        project._imported_media.append(project._register_uid(build(media_uid, content_state), media_uid))
        cache[path] = (media_uid, content_state)
    return cache[path]


def add_video_item(project, video: VideoFile, clip: Clip, item_name: str | None = None) -> ProjectItem:
    timecode_format = getattr(project,"_any_nle_rewrap_formats",{}).get(video.path, VIDEO_TIMECODE_FORMAT.get(video.rate))
    if timecode_format is None:
        raise UnverifiedShape(f"no Premiere-observed media timecode format for {video.rate} fps OP-Atom video yet")
    project_dir = str(project._path.resolve().parent)
    frame_rate = ticks_per_unit(video.rate)
    media_uid, content_state = _shared_media(project, video.path, lambda uid, state: _opatom_media(
        uid, "VideoStream", project._register(_video_stream(video)), video.path, project_dir, clip, state, None, video.start))
    source_id = project._register(mi.new_movie_media_source(media_uid, video.duration))
    markers_id = project._register(mi.new_markers_collection(content_state))
    logging_id = project._register(_logging(clip, frame_rate, video.duration, "2", timecode_format, video.start))
    clip_id = project._register(mi.new_movie_template_clip(markers_id, source_id, *VIDEO_LABEL))
    groups_id = project._register(mi.new_channel_groups())
    master_uid = str(uuid.uuid4())
    master = project._register_uid(mi.new_master_clip(master_uid, logging_id, clip_id, groups_id, item_name or clip.name), master_uid)
    item_uid = str(uuid.uuid4())
    item = project._register_uid(mi.new_clip_item(item_uid, master_uid, item_name or clip.name, VIDEO_LABEL[0]), item_uid)
    return project._attach_imported_item(item, item_uid, master, [clip_id], video.path, video.duration)


def add_audio_item(project, audio: AudioFile, clip: Clip, item_name: str | None = None) -> ProjectItem:
    if audio.bits not in SAMPLE_TYPE:
        raise UnverifiedShape(f"no observed SampleType for {audio.bits}-bit audio")
    project_dir = str(project._path.resolve().parent)
    rate = TICKS_PER_SECOND // audio.sample_rate
    layout = mi.audio_channel_layout(1)
    media_uid, content_state = _shared_media(project, audio.path, lambda uid, state: _opatom_media(
        uid, "AudioStream", project._register(mi.new_audio_stream(rate, layout, audio.duration, SAMPLE_TYPE[audio.bits])),
        audio.path, project_dir, clip, state, rate, audio.start))
    source_id = project._register(mi.new_audio_media_source(media_uid, audio.duration))
    markers_id = project._register(mi.new_markers_collection(content_state))
    logging_id = project._register(_logging(clip, rate, audio.duration, "1", AUDIO_TIMECODE_FORMAT, audio.start))
    secondary = [project._register(mi.new_secondary_content(source_id, 0))]
    template = mi.new_audio_template_clip(markers_id, source_id, secondary, *AUDIO_LABEL, layout)
    if audio.gain_db:
        _set_master_gain(template, audio.gain_db)
    clip_id = project._register(template)
    channel_ids = [project._register(mi.new_channel_serializer(0, 0))]
    vector_id = project._register(mi.new_channel_vector(channel_ids, 0))
    groups_id = project._register(mi.new_audio_channel_groups([vector_id]))
    chain_id = project._register(mi.new_master_audio_chain(1, 0))
    master_uid = str(uuid.uuid4())
    master_element = mi.new_audio_master_clip(master_uid, logging_id, [chain_id], [clip_id], groups_id, item_name or clip.name)
    if clip.time_display is not None:
        _set_time_display(master_element, clip.time_display)
    master = project._register_uid(master_element, master_uid)
    item_uid = str(uuid.uuid4())
    item = project._register_uid(mi.new_clip_item(item_uid, master_uid, item_name or clip.name, AUDIO_LABEL[0]), item_uid)
    return project._attach_imported_item(item, item_uid, master, [clip_id], audio.path, audio.duration)
