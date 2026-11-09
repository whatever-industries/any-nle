"""Synthesize a merged clip's hidden sequence and panel graph.

A merged clip (23_merged_clip) is a panel item whose master reads
`Video/AudioSequenceSource` over a HIDDEN one-video/one-audio-track
sequence flagged `BE.Sequence.IsMergedClip`, holding placements of
PRIVATE COPIES of the source media graphs, with a `Link` binding the
placed halves. The copies come from the import machinery and the
placements from `Track.add_clip` (see `Project.create_merged_clip`);
this module builds the sequence shell, its track/mix graph and the
panel-facing objects.
"""

from __future__ import annotations

import uuid
import xml.etree.ElementTree as ET

from .media_import import MONO_LAYOUT, _leaf, _top

_SEQUENCE_CLASS_ID = "6a15d903-8739-11d5-af2d-9b7855ad8974"
_VIDEO_GROUP_CLASS_ID = "9e9abf7a-0918-49c2-91ae-991b5dde77bb"
_AUDIO_GROUP_CLASS_ID = "9b9238b9-53a8-4cc3-b03f-b36246d052e6"
_DATA_GROUP_CLASS_ID = "b714b71d-6838-48dd-9b77-db19088ced7e"
_VIDEO_TRACK_CLASS_ID = "f68dcd81-8805-11d5-af2d-9bfa89d4ddd4"
_AUDIO_TRACK_CLASS_ID = "097f6203-99ae-11d5-84f2-8cf14bde7040"
_MIX_TRACK_CLASS_ID = "4b1d8400-e89e-11d5-abc4-a1a13b1e80a0"
_LINK_CLASS_ID = "149d4ea5-a7d4-4b34-9bb7-16d783904bf2"
_VIDEO_CHAIN_CLASS_ID = "0970e08a-f58f-4108-b29a-1a717b8e12e2"
_AUDIO_CHAIN_CLASS_ID = "3cb131d1-d3c0-47ae-a19a-bdf75ea11674"
_PAN_CLASS_ID = "33a94282-ee2c-11d5-abc4-c1cd7f9e3c10"
_INLET_CLASS_ID = "be3af080-e8c6-11d5-abc4-a1c6d5dee670"
_FADER_CLASS_ID = "1a38c583-ed5c-11d5-abc4-c1cbf61ec590"
_METER_CLASS_ID = "72ea4700-f615-11d5-abc4-c186585e63e0"
_VOLUME_PARAM_CLASS_ID = "a714635e-a628-4b27-9d59-77eba47dbc1a"
_MUTE_PARAM_CLASS_ID = "32657501-3aa4-445f-a49b-d09ecb9fa1ae"
_VIDEO_CLIP_CLASS_ID = "9308dbef-2440-4acb-9ab2-953b9a4e82ec"
_AUDIO_CLIP_CLASS_ID = "b8830d03-de02-41ee-84ec-fe566dc70cd9"
_ITEM_CLASS_ID = "cb4e0ed7-aca1-4171-8525-e3658dec06dd"
_MASTER_CLASS_ID = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf"
_LOGGING_CLASS_ID = "77ab7fdd-dcdf-465d-9906-7a330ca1e738"
_VIDEO_SEQ_SOURCE_CLASS_ID = "4752dfa9-7a7e-4a3b-a25b-cafde1a8d036"
_AUDIO_SEQ_SOURCE_CLASS_ID = "e8d4cc83-38cb-491f-9d94-e5f7e3b205ee"

_VIDEO_MEDIA_TYPE = "228cda18-3625-4d2d-951e-348879e4ed93"
_AUDIO_MEDIA_TYPE = "80b8e3d5-6dca-4195-aefb-cb5f407ab009"
_DATA_MEDIA_TYPE = "d8143ffe-eec4-4d2a-a909-d5f7bf094dc5"

#: 23_merged_clip's fixed sequence-level values. The hidden sequence is not
#: made from a preset: Premiere stamps this editing mode, preview preset and
#: format identifier on every merged clip.
_EDITING_MODE = "9678af98-a7b7-4bdb-b477-7ac9c8df4a4e"
_PREVIEW_PRESET = (
    "EncoderPresets\\SequencePreview\\"
    "9678af98-a7b7-4bdb-b477-7ac9c8df4a4e\\I-Frame Only MPEG.epr"
)
_PREVIEW_CLASS = "1297106761"
_PREVIEW_FORMAT = "d5c4dab8-b0a6-b1a1-0114-ec7d000000fa"
_COLOR_MANAGEMENT = (
    '{"autoToneMapEnabled":true,"enableLogColorManagement":2,'
    '"lutInterpolationMethod":1}'
)
_VR_CONFIGURATION = (
    '{"ambisonicsHRIR":"","ambisonicsMonitoringType":0,'
    '"capturedHorizontalView":360,"capturedVerticalView":180,'
    '"fieldOfHorizontalView":108,"fieldOfVerticalView":108,'
    '"projectionType":0,"stereoscopicEye":0,"stereoscopicType":0,'
    '"version":3}'
)
_OUTPUT_COLOR_SPACE = (
    '{"baseColorProfile":{"colorProfileName":"BT.709,8-bit,Display-Referred"},'
    '"baseProfileType":1}'
)
_NEXT_PANNER = "4294967279"
_VOLUME_UPPER_BOUND = "5.6234130859375"


def _bag_node(depth: int, keys: list[tuple[str, str]]) -> ET.Element:
    # A `Node`/`Properties` property bag whose open tag sits at `depth` tabs.
    indent = "\t" * depth
    node = ET.Element("Node", {"Version": "1"})
    node.text = f"\n{indent}\t"
    node.tail = f"\n{indent}"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = f"\n{indent}\t\t"
    properties.tail = f"\n{indent}"
    for index, (key, value) in enumerate(keys):
        leaf = ET.SubElement(properties, key)
        leaf.text = value
        leaf.tail = f"\n{indent}\t\t" if index < len(keys) - 1 else f"\n{indent}\t"
    return node


def build_master_bag(keys: list[tuple[str, str]]) -> ET.Element:
    """The `MZ.MergeClipUtils.*` bag a duplicated source master carries."""
    return _bag_node(2, keys)


def build_merged_sequence(
    sequence_uid: str,
    name: str,
    video_display_format: int,
    end_ticks: int,
    width: int,
    height: int,
    video_group_id: str,
    audio_group_id: str,
    data_group_id: str,
) -> ET.Element:
    element = _top("Sequence", _SEQUENCE_CLASS_ID, "12", uid=sequence_uid)
    element.append(
        _bag_node(
            2,
            [
                ("BE.Sequence.IsMergedClip", "true"),
                ("MZ.WorkInPoint", "0"),
                ("MZ.WorkOutPoint", str(end_ticks)),
                ("MZ.EditLine", "0"),
                ("MZ.Sequence.VideoTimeDisplayFormat", str(video_display_format)),
                ("MZ.Sequence.AudioTimeDisplayFormat", "200"),
                ("MZ.Sequence.EditingModeGUID", _EDITING_MODE),
                ("MZ.Sequence.PreviewUseMaxBitDepth", "false"),
                ("MZ.Sequence.PreviewUseMaxRenderQuality", "false"),
                ("MZ.Sequence.PreviewRenderingPresetPath", _PREVIEW_PRESET),
                ("MZ.Sequence.PreviewRenderingPresetCodec", "0"),
                ("MZ.Sequence.PreviewRenderingClassID", _PREVIEW_CLASS),
                ("MZ.Sequence.PreviewFrameSizeWidth", str(width)),
                ("MZ.Sequence.PreviewFrameSizeHeight", str(height)),
                ("MZ.MergedClip.AudioTimecodeBaseTrackNumber", "-1"),
            ],
        )
    )
    container = ET.SubElement(element, "PersistentGroupContainer", {"Version": "1"})
    container.text = "\n\t\t\t"
    container.tail = "\n\t\t"
    links = ET.SubElement(container, "LinkContainer", {"Version": "1"})
    links.text = "\n\t\t\t\t"
    links.tail = "\n\t\t"
    inner = ET.SubElement(links, "Links", {"Version": "1"})
    inner.text = "\n\t\t\t\t\t"
    inner.tail = "\n\t\t\t"
    # The ObjectRef is patched in once the Link object exists (it needs the
    # placed track items, which need the sequence to exist first).
    entry = ET.SubElement(inner, "Link", {"Index": "0", "ObjectRef": ""})
    entry.tail = "\n\t\t\t\t"
    groups = ET.SubElement(element, "TrackGroups", {"Version": "1"})
    groups.text = "\n\t\t\t"
    groups.tail = "\n\t\t"
    pairs = [
        (_VIDEO_MEDIA_TYPE, video_group_id),
        (_AUDIO_MEDIA_TYPE, audio_group_id),
        (_DATA_MEDIA_TYPE, data_group_id),
    ]
    for index, (media_type, group_id) in enumerate(pairs):
        pair = ET.SubElement(
            groups, "TrackGroup", {"Version": "1", "Index": str(index)}
        )
        pair.text = "\n\t\t\t\t"
        pair.tail = "\n\t\t\t" if index < len(pairs) - 1 else "\n\t\t"
        _leaf(pair, "First", media_type, "\n\t\t\t\t")
        ET.SubElement(pair, "Second", {"ObjectRef": group_id}).tail = "\n\t\t\t"
    _leaf(element, "Name", name, "\n\t\t")
    _leaf(element, "PreviewFormatIdentifier", _PREVIEW_FORMAT, "\n\t")
    return element


def build_video_group(
    track_uid: str, video_timebase: int, width: int, height: int, chain_id: str
) -> ET.Element:
    element = _top("VideoTrackGroup", _VIDEO_GROUP_CLASS_ID, "13")
    inner = ET.SubElement(element, "TrackGroup", {"Version": "1"})
    inner.text = "\n\t\t\t"
    inner.tail = "\n\t\t"
    tracks = ET.SubElement(inner, "Tracks", {"Version": "1"})
    tracks.text = "\n\t\t\t\t"
    tracks.tail = "\n\t\t\t"
    ET.SubElement(
        tracks, "Track", {"Index": "0", "ObjectURef": track_uid}
    ).tail = "\n\t\t\t"
    _leaf(inner, "FrameRate", str(video_timebase), "\n\t\t\t")
    _leaf(inner, "NextTrackID", "2", "\n\t\t")
    _leaf(element, "ColorManagementSettings", _COLOR_MANAGEMENT, "\n\t\t")
    _leaf(element, "ImmersiveVideoVRConfiguration", _VR_CONFIGURATION, "\n\t\t")
    _leaf(element, "OutputColorSpace", _OUTPUT_COLOR_SPACE, "\n\t\t")
    _leaf(element, "AutoInputGamutCompressionEnabled", "true", "\n\t\t")
    _leaf(element, "IsGraphicsWhiteSameAsProject", "false", "\n\t\t")
    _leaf(element, "IsColorAwareEffectsEnabledSameAsProject", "false", "\n\t\t")
    _leaf(element, "FrameRect", f"0,0,{width},{height}", "\n\t\t")
    owner = ET.SubElement(element, "ComponentOwner", {"Version": "1"})
    owner.text = "\n\t\t\t"
    owner.tail = "\n\t"
    ET.SubElement(owner, "Components", {"ObjectRef": chain_id}).tail = "\n\t\t"
    return element


def build_audio_group(
    track_uids: list[str], audio_rate: int, mix_track_id: str
) -> ET.Element:
    element = _top("AudioTrackGroup", _AUDIO_GROUP_CLASS_ID, "6")
    inner = ET.SubElement(element, "TrackGroup", {"Version": "1"})
    inner.text = "\n\t\t\t"
    inner.tail = "\n\t\t"
    tracks = ET.SubElement(inner, "Tracks", {"Version": "1"})
    tracks.text = "\n\t\t\t\t"
    tracks.tail = "\n\t\t\t"
    for index, track_uid in enumerate(track_uids):
        entry = ET.SubElement(
            tracks, "Track", {"Index": str(index), "ObjectURef": track_uid}
        )
        entry.tail = "\n\t\t\t\t" if index < len(track_uids) - 1 else "\n\t\t\t"
    _leaf(inner, "FrameRate", str(audio_rate), "\n\t\t\t")
    # 4 regardless of the track count: what 23_merged_clip (one mono track)
    # AND 77_merged_stereo (two) store - a Premiere-internal allocation
    # high-water mark, carried as-is.
    _leaf(inner, "NextTrackID", "4", "\n\t\t")
    ET.SubElement(element, "MasterTrack", {"ObjectRef": mix_track_id}).tail = "\n\t\t"
    _leaf(element, "ID", str(uuid.uuid4()), "\n\t\t")
    _leaf(element, "AutomationSafeFlags", "0", "\n\t\t")
    _leaf(element, "NumAdaptiveChannels", "1", "\n\t")
    return element


def build_data_group(video_timebase: int) -> ET.Element:
    element = _top("DataTrackGroup", _DATA_GROUP_CLASS_ID, "1")
    inner = ET.SubElement(element, "TrackGroup", {"Version": "1"})
    inner.text = "\n\t\t\t"
    inner.tail = "\n\t"
    _leaf(inner, "FrameRate", str(video_timebase), "\n\t\t\t")
    _leaf(inner, "NextTrackID", "1", "\n\t\t")
    return element


def _clip_track_shell(
    parent: ET.Element,
    media_type: str,
    track_id: int,
    keyframe_mode: bool,
    index: int = 0,
) -> None:
    # The empty ClipTrack shell; `Track.add_clip` fills ClipItems in. The
    # track's Index recurs on ClipItems and TransitionItems (77, 78).
    clip_track = ET.SubElement(parent, "ClipTrack", {"Version": "2"})
    clip_track.text = "\n\t\t\t"
    clip_track.tail = "\n\t"
    track = ET.SubElement(clip_track, "Track", {"Version": "4"})
    track.text = "\n\t\t\t\t"
    track.tail = "\n\t\t\t"
    keys = [
        ("TL.SQTrackExpanded", "0"),
        ("TL.SQTrackExpandedHeight", "41"),
        ("MZ.TrackTargeted", "1"),
    ]
    if keyframe_mode:
        keys.append(("CM.KeyframeMode", "true"))
    track.append(_bag_node(4, keys))
    _leaf(track, "ID", str(track_id), "\n\t\t\t\t")
    _leaf(track, "MediaType", media_type, "\n\t\t\t\t")
    _leaf(track, "Index", str(index), "\n\t\t\t")
    clip_items = ET.SubElement(clip_track, "ClipItems", {"Version": "3"})
    clip_items.text = "\n\t\t\t\t"
    clip_items.tail = "\n\t\t\t"
    _leaf(clip_items, "MediaType", media_type, "\n\t\t\t\t")
    _leaf(clip_items, "Index", str(index), "\n\t\t\t")
    transitions = ET.SubElement(clip_track, "TransitionItems", {"Version": "3"})
    transitions.text = "\n\t\t\t\t"
    transitions.tail = "\n\t\t"
    _leaf(transitions, "MediaType", media_type, "\n\t\t\t\t")
    _leaf(transitions, "Index", str(index), "\n\t\t\t")


def build_video_track(track_uid: str) -> ET.Element:
    element = _top("VideoClipTrack", _VIDEO_TRACK_CLASS_ID, "1", uid=track_uid)
    _clip_track_shell(element, _VIDEO_MEDIA_TYPE, 1, keyframe_mode=False)
    return element


def build_audio_track(
    track_uid: str, chain_id: str, panner_id: str, track_id: int = 2, index: int = 0
) -> ET.Element:
    element = _top("AudioClipTrack", _AUDIO_TRACK_CLASS_ID, "7", uid=track_uid)
    _clip_track_shell(
        element, _AUDIO_MEDIA_TYPE, track_id, keyframe_mode=True, index=index
    )
    # The shell closed the object; reopen it for the AudioTrack sibling.
    list(element)[-1].tail = "\n\t\t"
    audio = ET.SubElement(element, "AudioTrack", {"Version": "12"})
    audio.text = "\n\t\t\t"
    audio.tail = "\n\t"
    owner = ET.SubElement(audio, "ComponentOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t"
    owner.tail = "\n\t\t\t"
    ET.SubElement(owner, "Components", {"ObjectRef": chain_id}).tail = "\n\t\t\t"
    ET.SubElement(audio, "Panner", {"ObjectRef": panner_id}).tail = "\n\t\t\t"
    _leaf(audio, "ID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(audio, "ChannelType", "0", "\n\t\t\t")
    _leaf(audio, "NextPannerID", _NEXT_PANNER, "\n\t\t")
    return element


def build_mix_track(chain_id: str, panner_id: str, inlet_id: str) -> ET.Element:
    element = _top("AudioMixTrack", _MIX_TRACK_CLASS_ID, "4")
    audio = ET.SubElement(element, "AudioTrack", {"Version": "12"})
    audio.text = "\n\t\t\t"
    audio.tail = "\n\t\t"
    owner = ET.SubElement(audio, "ComponentOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t"
    owner.tail = "\n\t\t\t"
    ET.SubElement(owner, "Components", {"ObjectRef": chain_id}).tail = "\n\t\t\t"
    ET.SubElement(audio, "Panner", {"ObjectRef": panner_id}).tail = "\n\t\t\t"
    _leaf(audio, "ID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(audio, "ChannelType", "0", "\n\t\t\t")
    _leaf(audio, "SubType", "3", "\n\t\t\t")
    _leaf(audio, "Assign", "0", "\n\t\t\t")
    _leaf(audio, "NextPannerID", _NEXT_PANNER, "\n\t\t")
    track = ET.SubElement(element, "Track", {"Version": "4"})
    track.text = "\n\t\t\t"
    track.tail = "\n\t\t"
    _leaf(track, "ID", "1", "\n\t\t\t")
    _leaf(track, "MediaType", _AUDIO_MEDIA_TYPE, "\n\t\t\t")
    _leaf(track, "Index", "0", "\n\t\t")
    ET.SubElement(element, "Inlet", {"ObjectRef": inlet_id}).tail = "\n\t"
    return element


def build_empty_video_chain() -> ET.Element:
    element = _top("VideoComponentChain", _VIDEO_CHAIN_CLASS_ID, "3")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t"
    chain.tail = "\n\t"
    return element


def build_fader_chain(fader_id: str, meter_id: str) -> ET.Element:
    element = _top("AudioComponentChain", _AUDIO_CHAIN_CLASS_ID, "4")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t\t"
    chain.tail = "\n\t"
    components = ET.SubElement(chain, "Components", {"Version": "1"})
    components.text = "\n\t\t\t\t"
    components.tail = "\n\t\t"
    for index, target in enumerate((fader_id, meter_id)):
        entry = ET.SubElement(
            components, "Component", {"Index": str(index), "ObjectRef": target}
        )
        entry.tail = "\n\t\t\t\t" if index == 0 else "\n\t\t\t"
    return element


def _audio_component(
    parent: ET.Element,
    audio_rate: int,
    component_type: str,
    component_id: str,
    param_ids: list[str] | None,
) -> None:
    audio = ET.SubElement(parent, "AudioComponent", {"Version": "3"})
    audio.text = "\n\t\t\t"
    audio.tail = "\n\t"
    component = ET.SubElement(audio, "Component", {"Version": "7"})
    component.text = "\n\t\t\t\t"
    component.tail = "\n\t\t\t"
    if param_ids:
        params = ET.SubElement(component, "Params", {"Version": "1"})
        params.text = "\n\t\t\t\t\t"
        params.tail = "\n\t\t\t\t"
        for index, param_id in enumerate(param_ids):
            entry = ET.SubElement(
                params, "Param", {"Index": str(index), "ObjectRef": param_id}
            )
            entry.tail = "\n\t\t\t\t\t" if index < len(param_ids) - 1 else "\n\t\t\t\t"
    _leaf(component, "ID", component_id, "\n\t\t\t")
    _leaf(audio, "FrameRate", str(audio_rate), "\n\t\t\t")
    _leaf(audio, "AudioChannelLayout", MONO_LAYOUT, "\n\t\t\t")
    _leaf(audio, "ChannelType", "0", "\n\t\t\t")
    _leaf(audio, "AudioComponentType", component_type, "\n\t\t")


def build_fader(audio_rate: int, param_ids: list[str]) -> ET.Element:
    element = _top("AudioFader", _FADER_CLASS_ID, "3")
    _audio_component(element, audio_rate, "1", "1", param_ids)
    return element


def build_meter(audio_rate: int) -> ET.Element:
    element = _top("AudioMeter", _METER_CLASS_ID, "2")
    _audio_component(element, audio_rate, "2", "2", None)
    return element


def build_pan_processor(audio_rate: int) -> ET.Element:
    element = _top("DefaultPanProcessor", _PAN_CLASS_ID, "2")
    processor = ET.SubElement(element, "PanProcessor", {"Version": "3"})
    processor.text = "\n\t\t\t"
    processor.tail = "\n\t\t"
    audio = ET.SubElement(processor, "AudioComponent", {"Version": "3"})
    audio.text = "\n\t\t\t\t"
    audio.tail = "\n\t\t"
    component = ET.SubElement(audio, "Component", {"Version": "7"})
    component.text = "\n\t\t\t\t\t"
    component.tail = "\n\t\t\t\t"
    _leaf(component, "ID", "4294967280", "\n\t\t\t\t")
    _leaf(audio, "FrameRate", str(audio_rate), "\n\t\t\t\t")
    _leaf(audio, "AudioChannelLayout", MONO_LAYOUT, "\n\t\t\t\t")
    _leaf(audio, "ChannelType", "0", "\n\t\t\t\t")
    _leaf(audio, "AudioComponentType", "0", "\n\t\t\t")
    _leaf(element, "DefaultPannerInputChannelType", "0", "\n\t\t")
    _leaf(element, "DefaultPannerOutputChannelType", "0", "\n\t")
    return element


def build_inlet(track_uids: list[str]) -> ET.Element:
    element = _top("AudioTrackInlet", _INLET_CLASS_ID, "4")
    sources = ET.SubElement(element, "Sources", {"Version": "1"})
    sources.text = "\n\t\t\t"
    sources.tail = "\n\t\t"
    for index, track_uid in enumerate(track_uids):
        entry = ET.SubElement(
            sources, "Source", {"Index": str(index), "ObjectURef": track_uid}
        )
        entry.tail = "\n\t\t\t" if index < len(track_uids) - 1 else "\n\t\t"
    _leaf(element, "AudioChannelLayout", MONO_LAYOUT, "\n\t\t")
    _leaf(element, "ChannelType", "0", "\n\t")
    return element


def build_volume_param() -> ET.Element:
    element = _top("AudioComponentParam", _VOLUME_PARAM_CLASS_ID, "10")
    _leaf(element, "Name", "Volume", "\n\t\t")
    _leaf(element, "UpperBound", _VOLUME_UPPER_BOUND, "\n\t\t")
    _leaf(element, "RangeLocked", "false", "\n\t\t")
    _leaf(element, "UnitsString", "dB", "\n\t")
    return element


def build_mute_param() -> ET.Element:
    element = _top("AudioComponentParam", _MUTE_PARAM_CLASS_ID, "10")
    _leaf(element, "Name", "Mute", "\n\t\t")
    _leaf(element, "RangeLocked", "false", "\n\t")
    return element


def build_link(item_ids: list[str]) -> ET.Element:
    """The `Link` binding placed halves; order is the fixtures' own.

    A merged clip lists the video placement then each audio channel's
    (23, 77); a multicam clip lists the audio placement then its own
    angle's video (24).
    """
    element = _top("Link", _LINK_CLASS_ID, "1")
    group = ET.SubElement(element, "TrackItemGroup", {"Version": "1"})
    group.text = "\n\t\t\t"
    group.tail = "\n\t"
    items = ET.SubElement(group, "TrackItems", {"Version": "1"})
    items.text = "\n\t\t\t\t"
    items.tail = "\n\t\t"
    for index, item_id in enumerate(item_ids):
        entry = ET.SubElement(
            items, "TrackItem", {"Index": str(index), "ObjectRef": item_id}
        )
        entry.tail = "\n\t\t\t\t" if index < len(item_ids) - 1 else "\n\t\t\t"
    return element


def build_sequence_source(
    tag: str, sequence_uid: str, duration_ticks: int
) -> ET.Element:
    if tag == "VideoSequenceSource":
        class_id, version = _VIDEO_SEQ_SOURCE_CLASS_ID, "3"
    else:
        class_id, version = _AUDIO_SEQ_SOURCE_CLASS_ID, "7"
    element = _top(tag, class_id, version)
    source = ET.SubElement(element, "SequenceSource", {"Version": "4"})
    source.text = "\n\t\t\t"
    source.tail = "\n\t\t"
    content = ET.SubElement(source, "Content", {"Version": "10"})
    content.text = "\n\t\t\t"
    content.tail = "\n\t\t\t"
    ET.SubElement(source, "Sequence", {"ObjectURef": sequence_uid}).tail = "\n\t\t"
    _leaf(element, "OriginalDuration", str(duration_ticks), "\n\t")
    return element


def build_panel_video_template(sequence_source_id: str) -> ET.Element:
    element = _top("VideoClip", _VIDEO_CLIP_CLASS_ID, "11")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t"
    ET.SubElement(core, "Source", {"ObjectRef": sequence_source_id}).tail = "\n\t\t\t"
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t")
    return element


def build_panel_audio_template(
    sequence_source_id: str, secondary_id: str
) -> ET.Element:
    element = _top("AudioClip", _AUDIO_CLIP_CLASS_ID, "8")
    core = ET.SubElement(element, "Clip", {"Version": "18"})
    core.text = "\n\t\t\t"
    core.tail = "\n\t\t"
    ET.SubElement(core, "Source", {"ObjectRef": sequence_source_id}).tail = "\n\t\t\t"
    _leaf(core, "ClipID", str(uuid.uuid4()), "\n\t\t\t")
    _leaf(core, "InUse", "false", "\n\t\t")
    secondaries = ET.SubElement(element, "SecondaryContents", {"Version": "1"})
    secondaries.text = "\n\t\t\t"
    secondaries.tail = "\n\t\t"
    ET.SubElement(
        secondaries, "SecondaryContentItem", {"Index": "0", "ObjectRef": secondary_id}
    ).tail = "\n\t\t"
    _leaf(element, "AudioChannelLayout", MONO_LAYOUT, "\n\t")
    return element


def build_panel_master(
    master_uid: str,
    logging_id: str,
    chain_ids: list[str],
    audio_template_id: str,
    video_template_id: str,
    groups_id: str,
    name: str,
) -> ET.Element:
    # Unlike an AV import's master, the AUDIO template comes first in Clips.
    element = _top("MasterClip", _MASTER_CLASS_ID, "12", uid=master_uid)
    ET.SubElement(element, "LoggingInfo", {"ObjectRef": logging_id}).tail = "\n\t\t"
    chains = ET.SubElement(element, "AudioComponentChains", {"Version": "1"})
    chains.text = "\n\t\t\t"
    chains.tail = "\n\t\t"
    for index, chain_id in enumerate(chain_ids):
        entry = ET.SubElement(
            chains, "AudioComponentChain", {"Index": str(index), "ObjectRef": chain_id}
        )
        entry.tail = "\n\t\t\t" if index < len(chain_ids) - 1 else "\n\t\t"
    clips = ET.SubElement(element, "Clips", {"Version": "1"})
    clips.text = "\n\t\t\t"
    clips.tail = "\n\t\t"
    first = ET.SubElement(clips, "Clip", {"Index": "0", "ObjectRef": audio_template_id})
    first.tail = "\n\t\t\t"
    ET.SubElement(
        clips, "Clip", {"Index": "1", "ObjectRef": video_template_id}
    ).tail = "\n\t\t"
    ET.SubElement(
        element, "AudioClipChannelGroups", {"ObjectRef": groups_id}
    ).tail = "\n\t\t"
    _leaf(element, "Name", name, "\n\t\t")
    _leaf(element, "MasterClipChangeVersion", "0", "\n\t")
    return element


def build_panel_logging(
    clip_name: str, timecode_format: int, duration_ticks: int, video_timebase: int
) -> ET.Element:
    # The panel master's logging keeps the VIDEO source's file name, not the
    # merged name (the duplicated masters carry the merged name instead).
    element = _top("ClipLoggingInfo", _LOGGING_CLASS_ID, "10")
    _leaf(element, "CaptureMode", "2", "\n\t\t")
    _leaf(element, "ClipName", clip_name, "\n\t\t")
    _leaf(element, "TimecodeFormat", str(timecode_format), "\n\t\t")
    _leaf(element, "MediaInPoint", "0", "\n\t\t")
    _leaf(element, "MediaOutPoint", str(duration_ticks), "\n\t\t")
    _leaf(element, "MediaFrameRate", str(video_timebase), "\n\t")
    return element


def build_panel_item(item_uid: str, master_uid: str, name: str) -> ET.Element:
    element = _top("ClipProjectItem", _ITEM_CLASS_ID, "1", uid=item_uid)
    project_item = ET.SubElement(element, "ProjectItem", {"Version": "1"})
    project_item.text = "\n\t\t\t"
    project_item.tail = "\n\t\t"
    project_item.append(
        _bag_node(3, [("Column.PropertyText.Label", "BE.Prefs.LabelColors.0")])
    )
    _leaf(project_item, "Name", name, "\n\t\t")
    ET.SubElement(element, "MasterClip", {"ObjectURef": master_uid}).tail = "\n\t"
    return element
