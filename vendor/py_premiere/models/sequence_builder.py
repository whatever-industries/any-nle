"""Build a new sequence's object graph, without a bundled template.

Premiere adds 62 objects for a new sequence and - unlike an empty project,
where it regenerates 31 of the 40 from its own defaults - it rebuilds none of
them. The only omission it tolerates is the 5 `AudioMeter` objects, and it
does not put those back either, so this emits the lot. Measured with
`scripts/dev/strip_sequence.py`.

The graph is regular, which is what makes it small in code: each audio track
owns a chain, a panner, a fader, a meter and its parameters, and the master
track owns the same minus the balance. What a preset actually changes is
narrow - the frame rate, the frame size, the time display and the work-area
sentinel - established by generating eight presets varying one property at a
time (`scripts/jsx/make_preset_refs.jsx`).
"""

from __future__ import annotations

import uuid
import xml.etree.ElementTree as ET
from typing import NamedTuple

from ..xml.mutations import build_leaf as _leaf
from ..xml.mutations import indent_tree
from .time import TICKS_PER_SECOND

#: Media-type GUIDs, the same in every project.
_VIDEO_MEDIA = "228cda18-3625-4d2d-951e-348879e4ed93"
_AUDIO_MEDIA = "80b8e3d5-6dca-4195-aefb-cb5f407ab009"
_DATA_MEDIA = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5"

#: ClassIDs, by the object they identify.
_CLASS = {
    "ClipProjectItem": ("cb4e0ed7-aca1-4171-8525-e3658dec06dd", "1"),
    "MasterClip": ("fb11c33a-b0a9-4465-aa94-b6d5db2628cf", "12"),
    "ClipLoggingInfo": ("77ab7fdd-dcdf-465d-9906-7a330ca1e738", "10"),
    "AudioComponentChain": ("3cb131d1-d3c0-47ae-a19a-bdf75ea11674", "4"),
    "AudioClip": ("b8830d03-de02-41ee-84ec-fe566dc70cd9", "8"),
    "VideoClip": ("9308dbef-2440-4acb-9ab2-953b9a4e82ec", "11"),
    "ClipChannelGroupVectorSerializer": ("a3127a8c-95d4-456e-a7f5-171b3f922426", "1"),
    "AudioSequenceSource": ("e8d4cc83-38cb-491f-9d94-e5f7e3b205ee", "7"),
    "SecondaryContent": ("f9d004b5-cb04-4e2f-af6f-64fadc2c4be9", "1"),
    "VideoSequenceSource": ("4752dfa9-7a7e-4a3b-a25b-cafde1a8d036", "3"),
    "ClipChannelVectorSerializer": ("333d203b-3a53-4195-8894-fc7523ff3dc7", "1"),
    "Sequence": ("6a15d903-8739-11d5-af2d-9b7855ad8974", "12"),
    "ClipChannelSerializer": ("5c89aa7a-89a6-4483-becd-f2b1def42316", "1"),
    "VideoTrackGroup": ("9e9abf7a-0918-49c2-91ae-991b5dde77bb", "13"),
    "AudioTrackGroup": ("9b9238b9-53a8-4cc3-b03f-b36246d052e6", "6"),
    "DataTrackGroup": ("b714b71d-6838-48dd-9b77-db19088ced7e", "1"),
    "VideoClipTrack": ("f68dcd81-8805-11d5-af2d-9bfa89d4ddd4", "1"),
    "VideoComponentChain": ("0970e08a-f58f-4108-b29a-1a717b8e12e2", "3"),
    "AudioClipTrack": ("097f6203-99ae-11d5-84f2-8cf14bde7040", "7"),
    "AudioMixTrack": ("4b1d8400-e89e-11d5-abc4-a1a13b1e80a0", "4"),
    "StereoToStereoPanProcessor": ("7bf86a01-efbe-11d5-abc4-c1ce2b1e9090", "1"),
    "MonoTo16ChannelPanProcessor": ("8c9778ad-af4e-4e98-99fe-542f4eda2dac", "2"),
    "StereoTo16ChannelPanProcessor": ("1a356806-5dc5-4e2f-914c-d8353e1a6581", "2"),
    "AdaptiveToAdaptivePanProcessor": ("4bff4329-ba93-4072-950a-1f9c4aa7ab0b", "1"),
    "DefaultPanProcessor": ("33a94282-ee2c-11d5-abc4-c1cd7f9e3c10", "2"),
    "AudioTrackInlet": ("be3af080-e8c6-11d5-abc4-a1c6d5dee670", "4"),
    "AudioFader": ("1a38c583-ed5c-11d5-abc4-c1cbf61ec590", "3"),
    "AudioMeter": ("72ea4700-f615-11d5-abc4-c186585e63e0", "2"),
    "ScalarParam": ("a714635e-a628-4b27-9d59-77eba47dbc1a", "10"),
    "BoolParam": ("32657501-3aa4-445f-a49b-d09ecb9fa1ae", "10"),
}

#: The editing mode a Premiere 26 default sequence uses.
_EDITING_MODE = "9678af98-a7b7-4bdb-b477-7ac9c8df4a4e"
_PREVIEW_PRESET = (
    "EncoderPresets\\SequencePreview\\9678af98-a7b7-4bdb-b477-7ac9c8df4a4e"
    "\\QuickTime.epr"
)

#: Ticks per audio sample at 48 kHz - the audio track group's frame rate.
_AUDIO_FRAME_RATE = "5292000"
#: Stereo, as Premiere labels the two channels.
_STEREO = '[{"channellabel":100},{"channellabel":101}]'
#: One unlabelled channel - what every track carries in a mono-discrete
#: sequence.
_MONO = '[{"channellabel":0}]'


def _mono_layout(channels: int) -> str:
    return "[" + ",".join(['{"channellabel":0}'] * channels) + "]"


#: A mono-discrete sequence mixes to a 32-channel bus, and its panners
#: declare that full width as their output - despite the class being named
#: `MonoTo16ChannelPanProcessor`.
_BUS_CHANNELS = 32
#: `ChannelType`: 0 mono, 1 stereo, 3 the multi-channel bus.
_CT_MONO = "0"
_CT_STEREO = "1"
_CT_BUS = "3"
#: A virgin work area runs to 60 seconds, floored to a whole frame. Checked
#: against Premiere at four rates: 23.976 -> 15235011792000, 25 and 50 ->
#: 15240960000000 (60s divides evenly), 29.97 -> 15239249625600.
_WORK_AREA_LIMIT = 60 * TICKS_PER_SECOND

_LABEL = "BE.Prefs.LabelColors.5"
_LABEL_COLOR = "19005"
_PANNER_COMPONENT_ID = "4294967280"
_NEXT_PANNER_ID = "4294967279"

#: Where ObjectID allocation starts. `Project.add_sequence` reallocates
#: every id when it splices the fragment in, so this only has to be
#: self-consistent - it follows Premiere's own numbering so a diff
#: against one of its sequences stays readable.
_FIRST_ID = 40

#: The default video track count; Premiere writes 3 for every preset in its
#: library, so a different count needs a user-saved `.sqpreset` to measure.
_VIDEO_TRACKS = 3

#: The colour profile a non-HDR sequence works in.
_REC709 = "BT.709 RGB Full"

#: Default timeline display height for a track, in pixels. A `.sqpreset` can
#: override it per track via `mExpandedHeight` (absent = this default).
_TRACK_HEIGHT = 41


def _strip_keys(audio_tracks: int) -> list[str]:
    """Every mixer strip: one per audio track, then the master."""
    return [str(index) for index in range(audio_tracks)] + ["master"]


class SequenceFormat(NamedTuple):
    """What a sequence preset actually decides.

    Everything else in the graph is identical whatever the preset - verified
    by generating eight of them varying one property at a time.

    Audio channel configuration is `mono_discrete` plus `audio_tracks`:
    Premiere's multi-channel presets give each track one discrete mono
    channel, which swaps the panner class and widens the master bus.
    """

    frame_rate: int
    """Ticks per video frame (8475667200 = 29.97 fps)."""
    width: int
    height: int
    time_display: int
    """`MZ.Sequence.VideoTimeDisplayFormat`, e.g. 102 for 29.97 drop-frame."""
    audio_tracks: int = 4
    """Stereo audio tracks. Each one adds a whole mixer strip - a chain, a
    panner, a fader, a meter and three parameters - which is why the count
    belongs here rather than being fixed."""
    video_tracks: int = _VIDEO_TRACKS
    """Video tracks. Unlike an audio track, one costs a single object and a
    `NextTrackID` bump - there is no mixer strip behind it. Every preset
    Adobe ships writes 3, so 3 is all the corpus can attest to."""
    audio_track_height: int = _TRACK_HEIGHT
    """Timeline display height for the AUDIO tracks. A preset authors this
    per track (`mExpandedHeight`), and the 8-mono one asks for 25 so eight
    tracks fit. Video tracks are not included deliberately: that preset asks
    for 25 on its video tracks too and Premiere writes 41 anyway."""
    preview_format: str = "fc3cd4d9-d839-8259-9276-05c5000000ea"
    """`PreviewFormatIdentifier`. Shared by the 1080p presets; 4K and the
    social sizes each have their own."""
    linear_compositing: bool = True
    """`AllowLinearCompositing`. EVERY preset declares this (as the
    `.sqpreset`'s `VideoAllowLinearCompositing`); Premiere writes it into
    the sequence only when it is false, eliding the true default. It looked
    like a 4K-versus-1080p split until the preset files showed the input
    side."""
    mono_discrete: bool = False
    """Whether each audio track is a discrete mono channel rather than
    stereo. Premiere's "N mono discrete" presets are a different graph, not
    a bigger one: the panner class changes, the master clip carries 32
    channels instead of 2, and there is one channel serializer per track."""
    preview_codec: str = "apcs"
    """`MZ.Sequence.PreviewRenderingPresetCodec` as its FourCC. Every SDR
    preset previews in ProRes 422 LT; the HDR one steps up to 422 HQ
    (`apch`)."""
    color_space: str = _REC709
    """The working colour profile name. One name drives all three colour
    fields on the video track group: at the `BT.709` default Premiere writes
    only `OutputColorSpace`, and for any other space it writes
    `WorkingColorSpaceConfiguration` and `WorkingColorSpace` ahead of it
    carrying the same name (measured against the HDR preset)."""

    grouped_channels: int = 0


def _uhd(frame_rate: int, time_display: int) -> SequenceFormat:
    """A 4K preset: every rate shares one preview format, none composites
    linearly, and only the rate and its timecode display differ."""
    return SequenceFormat(
        frame_rate,
        3840,
        2160,
        time_display,
        preview_format="41384a52-7e4a-3c48-e0ad-4939000000ea",
        linear_compositing=False,
    )


def _uhd_hdr(frame_rate: int, time_display: int) -> SequenceFormat:
    """A 4K HDR preset: the same but in BT.2100 HLG, previewed in 422 HQ."""
    return SequenceFormat(
        frame_rate,
        3840,
        2160,
        time_display,
        preview_format="4e32a57e-26be-deca-0bf7-4548000000ea",
        linear_compositing=False,
        preview_codec="apch",
        color_space="BT.2100 HLG RGB Full",
    )


#: The presets `Project.sequence_presets()` offers. Every entry was read out
#: of a sequence Premiere made from the matching `.sqpreset`
#: (`samples/refs/presets/`, built by `scripts/jsx/make_preset_refs.jsx`)
FORMATS = {
    "1080p23976": SequenceFormat(10594584000, 1920, 1080, 110),
    "1080p25": SequenceFormat(10160640000, 1920, 1080, 101),
    "1080p2997": SequenceFormat(8475667200, 1920, 1080, 102),
    "1080p50": SequenceFormat(5080320000, 1920, 1080, 105),
    "1080p5994": SequenceFormat(4237833600, 1920, 1080, 106),
    "2160p23976": _uhd(10594584000, 110),
    "2160p25": _uhd(10160640000, 101),
    "2160p2997": _uhd(8475667200, 102),
    "2160p50": _uhd(5080320000, 105),
    "2160p5994": _uhd(4237833600, 106),
    # NOTE the HDR "59.94" preset is really 60p (4233600000 ticks, display
    # 108) where its SDR twin is 59.94 - measured, not assumed, and the
    # only rate where the two sets disagree.
    "2160p23976hdr": _uhd_hdr(10594584000, 110),
    "2160p25hdr": _uhd_hdr(10160640000, 101),
    "2160p2997hdr": _uhd_hdr(8475667200, 102),
    "2160p50hdr": _uhd_hdr(5080320000, 105),
    "2160p5994hdr": _uhd_hdr(4233600000, 108),
    "square1080p30": SequenceFormat(
        8467200000,
        1080,
        1080,
        104,
        preview_format="33cbb1f1-d0c5-3397-7a8d-a576000000ea",
    ),
    "portrait1080p30": SequenceFormat(
        8467200000,
        1080,
        1920,
        104,
        preview_format="01abac9c-c469-f667-c4a8-23a1000000ea",
    ),
    "portrait1080p4x5": SequenceFormat(
        8467200000,
        864,
        1080,
        104,
        preview_format="85cc5060-43fa-354a-99ff-02f0000000ea",
    ),
    # Premiere's "N mono discrete" broadcast presets: 1080p 23.976 with one
    # discrete mono channel per track.
    "broadcast4mono": SequenceFormat(
        10594584000, 1920, 1080, 110, audio_tracks=4, mono_discrete=True
    ),
    "broadcast8mono": SequenceFormat(
        10594584000,
        1920,
        1080,
        110,
        audio_tracks=8,
        mono_discrete=True,
        audio_track_height=25,
    ),
}

DEFAULT_PRESET = "1080p23976"


def _allocate(audio_tracks: int, secondaries: int, serializers: int) -> dict[str, str]:
    """ObjectIDs by role, in the order Premiere assigns them.

    `Project.add_sequence` reallocates all of them when it splices the
    fragment in, so they only have to be self-consistent - but following
    Premiere's own numbering keeps a diff against one of its sequences
    readable.
    """
    roles = [
        "logging",
        "master_chain",
        "audio_clip",
        "video_clip",
        "channel_groups",
        "audio_source",
    ]
    roles += [f"secondary_{index}" for index in range(secondaries)]
    roles += [
        "video_source",
        "channel_vector",
    ]
    roles += [f"serializer_{index}" for index in range(serializers)]
    roles += [
        "video_group",
        "audio_group",
        "data_group",
        "video_chain",
        "mix_track",
    ]
    for index in range(audio_tracks):
        roles += [f"strip_chain_{index}", f"panner_{index}"]
    roles += ["master_strip_chain", "default_panner", "inlet"]
    for index in range(audio_tracks):
        roles += [f"fader_{index}", f"meter_{index}", f"balance_{index}"]
    roles += ["fader_master", "meter_master"]
    for strip in _strip_keys(audio_tracks):
        roles += [f"volume_{strip}", f"mute_{strip}"]
    return {role: str(_FIRST_ID + offset) for offset, role in enumerate(roles)}


class _Uids:
    """Mints the fragment's GUIDs, or hands back pinned ones for a test."""

    def __init__(self, pinned: dict[str, str] | None = None) -> None:
        self._values = dict(pinned or {})

    def __call__(self, role: str) -> str:
        if role not in self._values:
            self._values[role] = str(uuid.uuid4())
        return self._values[role]


def _object(
    root: ET.Element, tag: str, object_id: str | None = None, uid: str | None = None
) -> ET.Element:
    class_id, version = _CLASS[tag]
    attributes = {}
    if object_id is not None:
        attributes["ObjectID"] = object_id
    if uid is not None:
        attributes["ObjectUID"] = uid
    attributes["ClassID"] = class_id
    attributes["Version"] = version
    return ET.SubElement(root, tag, attributes)


def _param_object(root: ET.Element, object_id: str, kind: str) -> ET.Element:
    class_id, version = _CLASS[kind]
    return ET.SubElement(
        root,
        "AudioComponentParam",
        {"ObjectID": object_id, "ClassID": class_id, "Version": version},
    )


def _indexed(parent: ET.Element, tag: str, entries: list[tuple[str, str]]) -> None:
    for index, (attribute, value) in enumerate(entries):
        ET.SubElement(parent, tag, {"Index": str(index), attribute: value})


def _clip_core(parent: ET.Element, source: str, clip_id: str) -> None:
    core = ET.SubElement(parent, "Clip", {"Version": "18"})
    node = ET.SubElement(core, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "asl.clip.label.color", _LABEL_COLOR)
    _leaf(properties, "asl.clip.label.name", _LABEL)
    ET.SubElement(core, "Source", {"ObjectRef": source})
    _leaf(core, "ClipID", clip_id)
    _leaf(core, "InUse", "false")


def _audio_component(
    parent: ET.Element,
    component_id: str,
    component_type: str,
    params: list[str],
    layout: str = _STEREO,
    channel_type: str = _CT_STEREO,
) -> ET.Element:
    # The `AudioComponent` shell every fader, meter and panner sits in.
    audio = ET.SubElement(parent, "AudioComponent", {"Version": "3"})
    component = ET.SubElement(audio, "Component", {"Version": "7"})
    if params:
        holder = ET.SubElement(component, "Params", {"Version": "1"})
        _indexed(holder, "Param", [("ObjectRef", ref) for ref in params])
    _leaf(component, "ID", component_id)
    _leaf(audio, "FrameRate", _AUDIO_FRAME_RATE)
    _leaf(audio, "AudioChannelLayout", layout)
    _leaf(audio, "ChannelType", channel_type)
    _leaf(audio, "AudioComponentType", component_type)
    return audio


def _sequence_source(parent: ET.Element, sequence_uid: str) -> None:
    source = ET.SubElement(parent, "SequenceSource", {"Version": "4"})
    ET.SubElement(source, "Content", {"Version": "10"})
    ET.SubElement(source, "Sequence", {"ObjectURef": sequence_uid})
    _leaf(parent, "OriginalDuration", "0")


def _strip_chain(
    chain: ET.Element,
    fader: str,
    meter: str,
    layout: str | None = _STEREO,
    channel_type: str = _CT_STEREO,
) -> None:
    inner = ET.SubElement(chain, "ComponentChain", {"Version": "3"})
    components = ET.SubElement(inner, "Components", {"Version": "1"})
    _indexed(components, "Component", [("ObjectRef", fader), ("ObjectRef", meter)])
    # A mono-discrete track's chain declares neither; only the bus does.
    if layout is not None:
        _leaf(chain, "AudioChannelLayout", layout)
        _leaf(chain, "ChannelType", channel_type)


def _clip_track(
    root: ET.Element,
    tag: str,
    media: str,
    uid: str,
    track_id: int,
    index: int,
    targeted: bool,
    track_name: str | None = None,
    height: int = _TRACK_HEIGHT,
) -> ET.Element:
    audio = tag == "AudioClipTrack"
    track = _object(root, tag, uid=uid)
    clip_track = ET.SubElement(track, "ClipTrack", {"Version": "2"})
    inner = ET.SubElement(clip_track, "Track", {"Version": "4"})
    node = ET.SubElement(inner, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "TL.SQTrackExpanded", "0")
    _leaf(properties, "TL.SQTrackExpandedHeight", str(height))
    _leaf(properties, "MZ.SourceTrackState", "0")
    _leaf(properties, "MZ.SourceTrackNumber", str(index))
    _leaf(properties, "MZ.TrackTargeted", "1" if targeted else "0")
    # Premiere writes the shy flag - and, on audio, the keyframe-style one -
    # only on the first two tracks of each kind.
    if index < 2:
        _leaf(properties, "TL.SQTrackShy", "0")
    if audio:
        if index < 2:
            _leaf(properties, "TL.SQTrackAudioKeyframeStyle", "0")
        if track_name is not None:
            _leaf(properties, "MZ.TrackName", track_name)
        _leaf(properties, "CM.KeyframeMode", "true")
    _leaf(inner, "ID", str(track_id))
    _leaf(inner, "MediaType", media)
    _leaf(inner, "Index", str(index))
    for holder_tag in ("ClipItems", "TransitionItems"):
        holder = ET.SubElement(clip_track, holder_tag, {"Version": "3"})
        _leaf(holder, "MediaType", media)
        _leaf(holder, "Index", str(index))
    return track


def build_sequence(
    name: str = "Seq01",
    preset: str = DEFAULT_PRESET,
    uids: dict[str, str] | None = None,
    video_tracks: int | None = None,
) -> ET.Element:
    """Return a `PremiereData` fragment holding a new sequence's objects.

    `Project.add_sequence` splices it in, reallocating every identifier, so
    the ones here only have to be self-consistent. Pass `uids` to pin them,
    which is what the parity test against Premiere's own output does.

    `video_tracks` overrides the preset's count, which no preset Adobe ships
    varies - it is 3 in every one of them.
    """
    if preset not in FORMATS:
        raise ValueError(f"unknown preset {preset!r}; try {sorted(FORMATS)}")
    fmt = FORMATS[preset]
    if video_tracks is not None:
        fmt = fmt._replace(video_tracks=video_tracks)
    # A mono-discrete sequence mixes to a 32-channel bus and carries one
    # channel serializer per track; a stereo one has two of each.
    wide = fmt.grouped_channels
    secondaries = _BUS_CHANNELS if fmt.mono_discrete or wide == 8 else 6 if wide == 6 else 2
    serializers = wide if wide else fmt.audio_tracks if fmt.mono_discrete else 2
    at = _allocate(fmt.audio_tracks, secondaries, serializers)
    uid = _Uids(uids)
    root = ET.Element("PremiereData", {"Version": "3"})

    # A mono-discrete sequence gives each track one channel and mixes them
    # into a wide bus; a stereo one is stereo throughout.
    mono = fmt.mono_discrete
    clip_layout = _mono_layout(_BUS_CHANNELS) if mono or wide == 8 else '[' + ','.join('{"channellabel":%d}' % (100+i) for i in range(6)) + ']' if wide == 6 else _STEREO
    clip_type = _CT_BUS if mono or wide == 8 else "2" if wide == 6 else _CT_STEREO
    track_layout = clip_layout if wide else _MONO if mono else _STEREO
    track_type = clip_type if wide else _CT_MONO if mono else _CT_STEREO
    video_uids = [uid(f"video_track_{i}") for i in range(fmt.video_tracks)]
    audio_uids = [uid(f"audio_track_{i}") for i in range(fmt.audio_tracks)]

    # --- panel item and master clip ---------------------------------------
    item = _object(root, "ClipProjectItem", uid=uid("item"))
    project_item = ET.SubElement(item, "ProjectItem", {"Version": "1"})
    node = ET.SubElement(project_item, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "project.icon.view.grid.order", "0")
    _leaf(properties, "Column.PropertyText.Label", _LABEL)
    _leaf(project_item, "Name", name)
    ET.SubElement(item, "MasterClip", {"ObjectURef": uid("master")})

    master = _object(root, "MasterClip", uid=uid("master"))
    ET.SubElement(master, "LoggingInfo", {"ObjectRef": at["logging"]})
    chains = ET.SubElement(master, "AudioComponentChains", {"Version": "1"})
    _indexed(chains, "AudioComponentChain", [("ObjectRef", at["master_chain"])])
    clips = ET.SubElement(master, "Clips", {"Version": "1"})
    _indexed(
        clips,
        "Clip",
        [("ObjectRef", at["audio_clip"]), ("ObjectRef", at["video_clip"])],
    )
    ET.SubElement(master, "AudioClipChannelGroups", {"ObjectRef": at["channel_groups"]})
    _leaf(master, "Name", name)
    _leaf(master, "MasterClipChangeVersion", "3")

    _object(root, "ClipLoggingInfo", at["logging"])

    chain = _object(root, "AudioComponentChain", at["master_chain"])
    _leaf(chain, "DefaultVol", "true")
    _leaf(chain, "DefaultVolumeComponentID", "1")
    _leaf(chain, "DefaultChannelVolumeComponentID", "2")
    ET.SubElement(chain, "ComponentChain", {"Version": "3"})
    _leaf(chain, "AudioChannelLayout", clip_layout)
    _leaf(chain, "ChannelType", clip_type)

    audio_clip = _object(root, "AudioClip", at["audio_clip"])
    _clip_core(audio_clip, at["audio_source"], uid("audio_clip_id"))
    contents = ET.SubElement(audio_clip, "SecondaryContents", {"Version": "1"})
    _indexed(
        contents,
        "SecondaryContentItem",
        [("ObjectRef", at[f"secondary_{i}"]) for i in range(secondaries)],
    )
    _leaf(audio_clip, "AudioChannelLayout", clip_layout)

    video_clip = _object(root, "VideoClip", at["video_clip"])
    _clip_core(video_clip, at["video_source"], uid("video_clip_id"))

    groups = _object(root, "ClipChannelGroupVectorSerializer", at["channel_groups"])
    vectors = ET.SubElement(groups, "ClipChannelVectors", {"Version": "1"})
    _indexed(vectors, "ClipChannelVectorItem", [("ObjectRef", at["channel_vector"])])

    _sequence_source(
        _object(root, "AudioSequenceSource", at["audio_source"]), uid("sequence")
    )
    for channel in range(secondaries):
        secondary = _object(root, "SecondaryContent", at[f"secondary_{channel}"])
        ET.SubElement(secondary, "Content", {"ObjectRef": at["audio_source"]})
        _leaf(secondary, "ChannelIndex", str(channel))
    _sequence_source(
        _object(root, "VideoSequenceSource", at["video_source"]), uid("sequence")
    )

    vector = _object(root, "ClipChannelVectorSerializer", at["channel_vector"])
    channels = ET.SubElement(vector, "ClipChannels", {"Version": "1"})
    _indexed(
        channels,
        "ClipChannelItem",
        [("ObjectRef", at[f"serializer_{i}"]) for i in range(serializers)],
    )
    _leaf(vector, "ChannelType", clip_type)

    _build_sequence_object(root, uid("sequence"), name, fmt, at)

    for channel in range(serializers):
        serializer = _object(root, "ClipChannelSerializer", at[f"serializer_{channel}"])
        _leaf(serializer, "SourceClipIndex", "0")
        _leaf(serializer, "mSourceChannelIndex", str(channel))

    # --- track groups ------------------------------------------------------
    _build_video_group(
        _object(root, "VideoTrackGroup", at["video_group"]), video_uids, fmt, at
    )
    _build_audio_group(
        _object(root, "AudioTrackGroup", at["audio_group"]),
        audio_uids,
        at,
        uid("audio_group_id"),
        wide if wide else fmt.audio_tracks if fmt.mono_discrete else 2,
    )
    data_group = _object(root, "DataTrackGroup", at["data_group"])
    inner = ET.SubElement(data_group, "TrackGroup", {"Version": "1"})
    _leaf(inner, "FrameRate", str(fmt.frame_rate))
    _leaf(inner, "NextTrackID", "1")

    # --- tracks ------------------------------------------------------------
    for index, track_uid in enumerate(video_uids):
        _clip_track(
            root,
            "VideoClipTrack",
            _VIDEO_MEDIA,
            track_uid,
            track_id=index + 1,
            index=index,
            targeted=index == 0,
        )
    # The track group's own chain is empty - unlike a clip placement's, it
    # carries no Default* intrinsics.
    chain = _object(root, "VideoComponentChain", at["video_chain"])
    ET.SubElement(chain, "ComponentChain", {"Version": "3"})

    for index, track_uid in enumerate(audio_uids):
        track = _clip_track(
            root,
            "AudioClipTrack",
            _AUDIO_MEDIA,
            track_uid,
            track_id=index + 2,
            index=index,
            targeted=True,
            # A discrete-mono track is an output bus, and Premiere names
            # it as one.
            track_name=f"Output {index + 1}" if fmt.mono_discrete else None,
            height=fmt.audio_track_height,
        )
        audio_track = ET.SubElement(track, "AudioTrack", {"Version": "12"})
        owner = ET.SubElement(audio_track, "ComponentOwner", {"Version": "1"})
        ET.SubElement(owner, "Components", {"ObjectRef": at[f"strip_chain_{index}"]})
        ET.SubElement(audio_track, "Panner", {"ObjectRef": at[f"panner_{index}"]})
        _leaf(audio_track, "ID", uid(f"audio_track_id_{index}"))
        if mono or wide:
            _leaf(audio_track, "ChannelType", track_type)
        _leaf(audio_track, "NextPannerID", _NEXT_PANNER_ID)

    mix = _object(root, "AudioMixTrack", at["mix_track"])
    audio_track = ET.SubElement(mix, "AudioTrack", {"Version": "12"})
    owner = ET.SubElement(audio_track, "ComponentOwner", {"Version": "1"})
    ET.SubElement(owner, "Components", {"ObjectRef": at["master_strip_chain"]})
    ET.SubElement(audio_track, "Panner", {"ObjectRef": at["default_panner"]})
    _leaf(audio_track, "ID", uid("mix_track_id"))
    if mono or wide:
        _leaf(audio_track, "ChannelType", clip_type)
    _leaf(audio_track, "SubType", "3")
    _leaf(audio_track, "Assign", "0")
    _leaf(audio_track, "NextPannerID", _NEXT_PANNER_ID)
    inner = ET.SubElement(mix, "Track", {"Version": "4"})
    node = ET.SubElement(inner, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "TL.SQTrackExpanded", "0")
    _leaf(properties, "TL.SQTrackExpandedHeight", "41")
    _leaf(inner, "ID", "1")
    _leaf(inner, "MediaType", _AUDIO_MEDIA)
    _leaf(inner, "Index", "0")
    ET.SubElement(mix, "Inlet", {"ObjectRef": at["inlet"]})

    # --- the mix graph -----------------------------------------------------
    for index in range(fmt.audio_tracks):
        _strip_chain(
            _object(root, "AudioComponentChain", at[f"strip_chain_{index}"]),
            at[f"fader_{index}"],
            at[f"meter_{index}"],
            layout=None if mono else track_layout,
            channel_type=track_type,
        )
        if wide:
            panner = _object(root, "DefaultPanProcessor" if wide == 6 else "AdaptiveToAdaptivePanProcessor", at[f"panner_{index}"])
            processor = ET.SubElement(panner, "PanProcessor", {"Version": "3"})
            _audio_component(processor, _PANNER_COMPONENT_ID, "0", [at[f"balance_{index}"]] if wide == 8 else [], track_layout, track_type)
            if wide == 6:
                _leaf(panner, "DefaultPannerInputChannelType", track_type)
                _leaf(panner, "DefaultPannerOutputChannelType", track_type)
        elif mono:
            panner = _object(root, "MonoTo16ChannelPanProcessor", at[f"panner_{index}"])
            direct = ET.SubElement(panner, "DirectPanProcessor", {"Version": "2"})
            processor = ET.SubElement(direct, "PanProcessor", {"Version": "3"})
            _audio_component(
                processor,
                _PANNER_COMPONENT_ID,
                "0",
                [at[f"balance_{index}"]],
                _MONO,
                _CT_MONO,
            )
            # The output layout belongs to the PanProcessor; only the
            # routing matrix hangs off the DirectPanProcessor around it.
            _leaf(
                processor,
                "OutputAudioChannelLayout",
                _mono_layout(_BUS_CHANNELS),
            )
            # Tracks pair off onto successive bus channels: 0,0,2,2,4,4...
            _leaf(direct, "Matrix", f"[[0,[{index // 2 * 2}]]]")
        else:
            panner = _object(root, "StereoToStereoPanProcessor", at[f"panner_{index}"])
            processor = ET.SubElement(panner, "PanProcessor", {"Version": "3"})
            _audio_component(
                processor, _PANNER_COMPONENT_ID, "0", [at[f"balance_{index}"]]
            )

    _strip_chain(
        _object(root, "AudioComponentChain", at["master_strip_chain"]),
        at["fader_master"],
        at["meter_master"],
        layout=clip_layout,
        channel_type=clip_type,
    )
    panner = _object(root, "DefaultPanProcessor", at["default_panner"])
    processor = ET.SubElement(panner, "PanProcessor", {"Version": "3"})
    _audio_component(processor, _PANNER_COMPONENT_ID, "0", [], clip_layout, clip_type)
    _leaf(panner, "DefaultPannerInputChannelType", clip_type)
    _leaf(panner, "DefaultPannerOutputChannelType", clip_type)

    inlet = _object(root, "AudioTrackInlet", at["inlet"])
    sources = ET.SubElement(inlet, "Sources", {"Version": "1"})
    _indexed(sources, "Source", [("ObjectURef", value) for value in audio_uids])
    _leaf(inlet, "AudioChannelLayout", clip_layout)
    if mono or wide:
        _leaf(inlet, "ChannelType", clip_type)

    for strip in _strip_keys(fmt.audio_tracks):
        is_master = strip == "master"
        layout = clip_layout if is_master else track_layout
        channel_type = clip_type if is_master else track_type
        fader = _object(root, "AudioFader", at[f"fader_{strip}"])
        _audio_component(
            fader,
            "1",
            "1",
            [at[f"volume_{strip}"], at[f"mute_{strip}"]],
            layout,
            channel_type,
        )
        meter = _object(root, "AudioMeter", at[f"meter_{strip}"])
        _audio_component(meter, "2", "2", [], layout, channel_type)
        if is_master:
            continue
        pan = _param_object(root, at[f"balance_{strip}"], "ScalarParam")
        if mono:
            # Premiere writes the value only on the even-indexed track of
            # each pair - the one that sits at the head of its bus channel.
            if int(strip) % 2 == 0:
                _leaf(pan, "StartKeyframe", "-91445760000000000,0.,0,0,0,0,0,0")
                _leaf(pan, "CurrentValue", "0")
            _leaf(pan, "Name", "Pan")
        else:
            _leaf(pan, "StartKeyframe", "-91445760000000000,0.5,0,0,0,0,0,0")
            _leaf(pan, "CurrentValue", "0.5")
            _leaf(pan, "Name", "Balance")
        _leaf(pan, "IsInverted", "true")

    for strip in _strip_keys(fmt.audio_tracks):
        volume = _param_object(root, at[f"volume_{strip}"], "ScalarParam")
        _leaf(volume, "Name", "Volume")
        _leaf(volume, "UpperBound", "5.6234130859375")
        _leaf(volume, "RangeLocked", "false")
        _leaf(volume, "UnitsString", "dB")
        mute = _param_object(root, at[f"mute_{strip}"], "BoolParam")
        _leaf(mute, "Name", "Mute")
        _leaf(mute, "RangeLocked", "false")

    indent_tree(root)
    return root


def _build_video_group(
    group: ET.Element, uids: list[str], fmt: SequenceFormat, at: dict[str, str]
) -> None:
    inner = ET.SubElement(group, "TrackGroup", {"Version": "1"})
    tracks = ET.SubElement(inner, "Tracks", {"Version": "1"})
    _indexed(tracks, "Track", [("ObjectURef", value) for value in uids])
    _leaf(inner, "FrameRate", str(fmt.frame_rate))
    _leaf(inner, "NextTrackID", str(len(uids) + 1))
    _leaf(
        group,
        "ColorManagementSettings",
        '{"autoToneMapEnabled":true,"enableLogColorManagement":2,'
        '"lutInterpolationMethod":1}',
    )
    if not fmt.linear_compositing:
        _leaf(group, "AllowLinearCompositing", "false")
    _leaf(
        group,
        "ImmersiveVideoVRConfiguration",
        '{"ambisonicsHRIR":"","ambisonicsMonitoringType":0,'
        '"capturedHorizontalView":360,"capturedVerticalView":180,'
        '"fieldOfHorizontalView":108,"fieldOfVerticalView":108,'
        '"projectionType":0,"stereoscopicEye":0,"stereoscopicType":0,'
        '"version":3}',
    )
    profile = f'{{"baseColorProfile":{{"colorProfileName":"{fmt.color_space}"}},"baseProfileType":1}}'
    if fmt.color_space != _REC709:
        # Only a non-default working space gets these two, and they carry the
        # same profile name as the output space.
        _leaf(
            group,
            "WorkingColorSpaceConfiguration",
            f'{{"workingSpaceConfigVersion":1,"workingSpaceID":"{fmt.color_space}",'
            '"workingSpaceIsLinearized":0}',
        )
        _leaf(group, "WorkingColorSpace", profile)
    _leaf(group, "OutputColorSpace", profile)
    _leaf(group, "AutoInputGamutCompressionEnabled", "true")
    _leaf(group, "IsGraphicsWhiteSameAsProject", "false")
    _leaf(group, "IsColorAwareEffectsEnabledSameAsProject", "false")
    _leaf(group, "FrameRect", f"0,0,{fmt.width},{fmt.height}")
    owner = ET.SubElement(group, "ComponentOwner", {"Version": "1"})
    ET.SubElement(owner, "Components", {"ObjectRef": at["video_chain"]})


def _build_audio_group(
    group: ET.Element,
    uids: list[str],
    at: dict[str, str],
    group_id: str,
    channels: int,
) -> None:
    inner = ET.SubElement(group, "TrackGroup", {"Version": "1"})
    tracks = ET.SubElement(inner, "Tracks", {"Version": "1"})
    _indexed(tracks, "Track", [("ObjectURef", value) for value in uids])
    _leaf(inner, "FrameRate", _AUDIO_FRAME_RATE)
    _leaf(inner, "NextTrackID", str(len(uids) + 2))
    ET.SubElement(group, "MasterTrack", {"ObjectRef": at["mix_track"]})
    _leaf(group, "ID", group_id)
    _leaf(group, "AutomationSafeFlags", "0")
    _leaf(group, "NumAdaptiveChannels", str(channels))


def _build_sequence_object(
    root: ET.Element, uid: str, name: str, fmt: SequenceFormat, at: dict[str, str]
) -> None:
    sequence = _object(root, "Sequence", uid=uid)
    node = ET.SubElement(sequence, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "AMM.CurrentSolo", "[]")
    _leaf(properties, "TL.SQTimePerPixel", "0.90634441087613293")
    _leaf(properties, "Monitor.ProgramZoomIn", "0")
    _leaf(properties, "Monitor.ProgramZoomOut", "0")
    _leaf(properties, "TL.SQHeaderWidth", "180")
    _leaf(properties, "TL.SQVisibleBaseTime", "0")
    _leaf(properties, "TL.SQVideoVisibleBase", "0")
    _leaf(properties, "TL.SQAudioVisibleBase", "0")
    _leaf(properties, "TL.SQDataVisibleBase", "0")
    _leaf(properties, "TL.SQHideShyTracks", "0")
    _leaf(properties, "TL.SQAVDividerPosition", "0.5")
    _leaf(properties, "MZ.WorkInPoint", "0")
    _leaf(
        properties,
        "MZ.WorkOutPoint",
        str(_WORK_AREA_LIMIT // fmt.frame_rate * fmt.frame_rate),
    )
    _leaf(properties, "MZ.EditLine", "0")
    _leaf(properties, "MZ.Sequence.VideoTimeDisplayFormat", str(fmt.time_display))
    _leaf(properties, "MZ.Sequence.AudioTimeDisplayFormat", "200")
    _leaf(properties, "MZ.Sequence.EditingModeGUID", _EDITING_MODE)
    _leaf(properties, "MZ.Sequence.PreviewUseMaxBitDepth", "false")
    _leaf(properties, "MZ.Sequence.PreviewUseMaxRenderQuality", "false")
    _leaf(properties, "MZ.Sequence.PreviewRenderingPresetPath", _PREVIEW_PRESET)
    _leaf(
        properties,
        "MZ.Sequence.PreviewRenderingPresetCodec",
        str(int.from_bytes(fmt.preview_codec.encode("ascii"), "big")),
    )
    _leaf(properties, "MZ.Sequence.PreviewRenderingClassID", "1061109567")
    _leaf(properties, "MZ.Sequence.PreviewFrameSizeWidth", str(fmt.width))
    _leaf(properties, "MZ.Sequence.PreviewFrameSizeHeight", str(fmt.height))
    container = ET.SubElement(sequence, "PersistentGroupContainer", {"Version": "1"})
    ET.SubElement(container, "LinkContainer", {"Version": "1"})
    groups = ET.SubElement(sequence, "TrackGroups", {"Version": "1"})
    for index, (media, role) in enumerate(
        (
            (_VIDEO_MEDIA, "video_group"),
            (_AUDIO_MEDIA, "audio_group"),
            (_DATA_MEDIA, "data_group"),
        )
    ):
        entry = ET.SubElement(
            groups, "TrackGroup", {"Version": "1", "Index": str(index)}
        )
        _leaf(entry, "First", media)
        ET.SubElement(entry, "Second", {"ObjectRef": at[role]})
    _leaf(sequence, "Name", name)
    _leaf(sequence, "PreviewFormatIdentifier", fmt.preview_format)
