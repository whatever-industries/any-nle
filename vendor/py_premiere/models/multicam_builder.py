"""Synthesize a multicam clip's hidden sequence and panel graph.

A multicam source clip (24_multicam) is, like a merged clip, a panel item
whose master plays a hidden sequence - but the anatomy differs: nothing is
copied (the placements reference the ORIGINAL source masters, and the
source items move into a `Processed Clips` bin), the master flags itself
with `Source.Monitor.Multicam.Enabled` instead of a sequence bag key, each
angle gets its own video track, and the audio mixes through a 32-channel
adaptive bus behind a `StereoTo16ChannelPanProcessor`.

Built as a `PremiereData` fragment with self-consistent identifiers, like
`sequence_builder.build_sequence`; `Project._splice_fragment` reallocates
them. The placements land afterwards through `Track.add_clip`.
"""

from __future__ import annotations

import copy
import xml.etree.ElementTree as ET

from ..xml.mutations import build_leaf as _leaf
from ..xml.mutations import indent_tree
from .sequence_builder import (
    _AUDIO_FRAME_RATE,
    _AUDIO_MEDIA,
    _BUS_CHANNELS,
    _CT_BUS,
    _CT_STEREO,
    _DATA_MEDIA,
    _EDITING_MODE,
    _NEXT_PANNER_ID,
    _PANNER_COMPONENT_ID,
    _PREVIEW_PRESET,
    _STEREO,
    _TRACK_HEIGHT,
    _VIDEO_MEDIA,
    _WORK_AREA_LIMIT,
    _audio_component,
    _clip_core,
    _indexed,
    _mono_layout,
    _object,
    _param_object,
    _strip_chain,
    _Uids,
)

#: 24_multicam's video-group colour blobs. Richer than the preset builder's
#: (a newer Premiere save stamps luminance metadata); carried verbatim.
_COLOR_MANAGEMENT = (
    '{"autoToneMapEnabled":true,"enableLogColorManagement":2,'
    '"graphicsWhiteLuminance":203,"lutInterpolationMethod":1}'
)
_VR_CONFIGURATION = (
    '{"ambisonicsHRIR":"","ambisonicsMonitoringType":0,'
    '"capturedHorizontalView":360,"capturedVerticalView":180,'
    '"fieldOfHorizontalView":108,"fieldOfVerticalView":108,'
    '"projectionType":0,"stereoscopicEye":0,"stereoscopicType":0,'
    '"version":3}'
)
_OUTPUT_COLOR_SPACE = (
    '{"baseColorProfile":{"colorProfileData":"AQAAAGQAAAA=",'
    '"colorProfileName":"BT.709,8-bit,Display-Referred"},"baseProfileType":1,'
    '"colorSpaceMetadata":{"peakLuminance":100}}'
)
#: `MZ.Sequence.PreviewRenderingPresetCodec` for ProRes 422 LT, as presets.
_PREVIEW_CODEC = str(int.from_bytes(b"apcs", "big"))
#: What 24_multicam stores for its 64x36 25 fps media. Premiere recomputes
#: the preview format on open (seen on the merged-clip resave), so an
#: off-geometry value self-heals.
_PREVIEW_FORMAT = "367c336a-37a8-8b25-72cd-b234000000ea"

_BIN_CLASS_ID = "dbfd6653-24da-480e-a35e-ba45e9504e4b"
#: Premiere's factory default label for bins (`BE.Prefs.LabelDefaults.Bin`).
_BIN_LABEL = "BE.Prefs.LabelColors.7"


def _allocate_multicam() -> dict[str, str]:
    roles = [
        "logging",
        "master_chain",
        "audio_clip",
        "video_clip",
        "channel_groups",
        "audio_source",
    ]
    roles += [f"secondary_{index}" for index in range(_BUS_CHANNELS)]
    roles += ["video_source", "channel_vector", "serializer_0", "serializer_1"]
    roles += ["video_group", "audio_group", "data_group", "video_chain", "mix_track"]
    roles += ["strip_chain", "panner", "master_strip_chain", "default_panner", "inlet"]
    roles += ["fader", "meter", "balance", "fader_master", "meter_master"]
    roles += ["volume_0", "mute_0", "volume_master", "mute_master"]
    return {role: str(40 + offset) for offset, role in enumerate(roles)}


def build_multicam(
    name: str,
    frame_rate: int,
    width: int,
    height: int,
    time_display: int,
    duration_ticks: int,
    video_tracks: int,
    link: bool = True,
) -> ET.Element:
    """A `PremiereData` fragment holding a new multicam clip's objects.

    Everything except the angle placements themselves, which
    `Project.create_multicam_clip` lands through `Track.add_clip` after the
    splice. `link` is False when the audio comes from an audio-only angle:
    with no video half to bind, Premiere writes an EMPTY LinkContainer
    (79_two_multicams).
    """
    at = _allocate_multicam()
    uid = _Uids()
    root = ET.Element("PremiereData", {"Version": "3"})
    layout_bus = _mono_layout(_BUS_CHANNELS)
    video_uids = [uid(f"video_track_{i}") for i in range(video_tracks)]
    audio_uid = uid("audio_track")

    # --- panel item and master clip ---------------------------------------
    item = _object(root, "ClipProjectItem", uid=uid("item"))
    project_item = ET.SubElement(item, "ProjectItem", {"Version": "1"})
    node = ET.SubElement(project_item, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "Column.PropertyText.Label", "BE.Prefs.LabelColors.5")
    _leaf(project_item, "Name", name)
    ET.SubElement(item, "MasterClip", {"ObjectURef": uid("master")})

    master = _object(root, "MasterClip", uid=uid("master"))
    node = ET.SubElement(master, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "Source.Monitor.Multicam.Enabled", "true")
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
    _leaf(master, "MasterClipChangeVersion", "0")

    _object(root, "ClipLoggingInfo", at["logging"])

    chain = _object(root, "AudioComponentChain", at["master_chain"])
    _leaf(chain, "DefaultVol", "true")
    _leaf(chain, "DefaultVolumeComponentID", "1")
    _leaf(chain, "DefaultChannelVolumeComponentID", "2")
    ET.SubElement(chain, "ComponentChain", {"Version": "3"})
    _leaf(chain, "AudioChannelLayout", _STEREO)
    _leaf(chain, "ChannelType", _CT_STEREO)

    # The panel template plays the bus: 32 secondaries over the audio
    # sequence source, while the channel groups describe the stereo SOURCE.
    audio_clip = _object(root, "AudioClip", at["audio_clip"])
    _clip_core(audio_clip, at["audio_source"], uid("audio_clip_id"))
    contents = ET.SubElement(audio_clip, "SecondaryContents", {"Version": "1"})
    _indexed(
        contents,
        "SecondaryContentItem",
        [("ObjectRef", at[f"secondary_{i}"]) for i in range(_BUS_CHANNELS)],
    )
    _leaf(audio_clip, "AudioChannelLayout", layout_bus)

    video_clip = _object(root, "VideoClip", at["video_clip"])
    _clip_core(video_clip, at["video_source"], uid("video_clip_id"))

    groups = _object(root, "ClipChannelGroupVectorSerializer", at["channel_groups"])
    vectors = ET.SubElement(groups, "ClipChannelVectors", {"Version": "1"})
    _indexed(vectors, "ClipChannelVectorItem", [("ObjectRef", at["channel_vector"])])

    audio_source = _object(root, "AudioSequenceSource", at["audio_source"])
    source = ET.SubElement(audio_source, "SequenceSource", {"Version": "4"})
    ET.SubElement(source, "Content", {"Version": "10"})
    ET.SubElement(source, "Sequence", {"ObjectURef": uid("sequence")})
    _leaf(audio_source, "OriginalDuration", str(duration_ticks))

    for channel in range(_BUS_CHANNELS):
        secondary = _object(root, "SecondaryContent", at[f"secondary_{channel}"])
        ET.SubElement(secondary, "Content", {"ObjectRef": at["audio_source"]})
        _leaf(secondary, "ChannelIndex", str(channel))

    video_source = _object(root, "VideoSequenceSource", at["video_source"])
    source = ET.SubElement(video_source, "SequenceSource", {"Version": "4"})
    ET.SubElement(source, "Content", {"Version": "10"})
    ET.SubElement(source, "Sequence", {"ObjectURef": uid("sequence")})
    _leaf(video_source, "OriginalDuration", str(duration_ticks))

    vector = _object(root, "ClipChannelVectorSerializer", at["channel_vector"])
    channels = ET.SubElement(vector, "ClipChannels", {"Version": "1"})
    _indexed(
        channels,
        "ClipChannelItem",
        [("ObjectRef", at["serializer_0"]), ("ObjectRef", at["serializer_1"])],
    )
    _leaf(vector, "ChannelType", _CT_STEREO)
    for channel in (0, 1):
        serializer = _object(root, "ClipChannelSerializer", at[f"serializer_{channel}"])
        _leaf(serializer, "SourceClipIndex", "0")
        _leaf(serializer, "mSourceChannelIndex", str(channel))

    # --- the hidden sequence -----------------------------------------------
    sequence = _object(root, "Sequence", uid=uid("sequence"))
    node = ET.SubElement(sequence, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "MZ.WorkInPoint", "0")
    _leaf(
        properties,
        "MZ.WorkOutPoint",
        str(_WORK_AREA_LIMIT // frame_rate * frame_rate),
    )
    _leaf(properties, "MZ.Sequence.VideoTimeDisplayFormat", str(time_display))
    _leaf(properties, "MZ.Sequence.AudioTimeDisplayFormat", "200")
    _leaf(properties, "MZ.Sequence.EditingModeGUID", _EDITING_MODE)
    _leaf(properties, "MZ.Sequence.PreviewUseMaxBitDepth", "false")
    _leaf(properties, "MZ.Sequence.PreviewUseMaxRenderQuality", "false")
    _leaf(properties, "MZ.Sequence.PreviewRenderingPresetPath", _PREVIEW_PRESET)
    _leaf(properties, "MZ.Sequence.PreviewRenderingPresetCodec", _PREVIEW_CODEC)
    _leaf(properties, "MZ.Sequence.PreviewRenderingClassID", "1061109567")
    _leaf(properties, "MZ.Sequence.PreviewFrameSizeWidth", str(width))
    _leaf(properties, "MZ.Sequence.PreviewFrameSizeHeight", str(height))
    container = ET.SubElement(sequence, "PersistentGroupContainer", {"Version": "1"})
    link_container = ET.SubElement(container, "LinkContainer", {"Version": "1"})
    if link:
        links = ET.SubElement(link_container, "Links", {"Version": "1"})
        # Patched to the real Link once the placements exist.
        ET.SubElement(links, "Link", {"Index": "0", "ObjectRef": "@link"})
    track_groups = ET.SubElement(sequence, "TrackGroups", {"Version": "1"})
    for index, (media, role) in enumerate(
        (
            (_VIDEO_MEDIA, "video_group"),
            (_AUDIO_MEDIA, "audio_group"),
            (_DATA_MEDIA, "data_group"),
        )
    ):
        entry = ET.SubElement(
            track_groups, "TrackGroup", {"Version": "1", "Index": str(index)}
        )
        _leaf(entry, "First", media)
        ET.SubElement(entry, "Second", {"ObjectRef": at[role]})
    _leaf(sequence, "Name", name)
    _leaf(sequence, "PreviewFormatIdentifier", _PREVIEW_FORMAT)

    # --- track groups -------------------------------------------------------
    group = _object(root, "VideoTrackGroup", at["video_group"])
    inner = ET.SubElement(group, "TrackGroup", {"Version": "1"})
    tracks = ET.SubElement(inner, "Tracks", {"Version": "1"})
    _indexed(tracks, "Track", [("ObjectURef", value) for value in video_uids])
    _leaf(inner, "FrameRate", str(frame_rate))
    # Premiere's own values vary with UI session history (24 stores 5 for
    # ids 3,4; 78 stores 7 for ids 4,5,6; 79 stores 4 for id 2) - the only
    # invariant is next > every used id, which this scheme satisfies.
    _leaf(inner, "NextTrackID", str(3 + video_tracks))
    _leaf(group, "ColorManagementSettings", _COLOR_MANAGEMENT)
    _leaf(group, "ImmersiveVideoVRConfiguration", _VR_CONFIGURATION)
    _leaf(group, "OutputColorSpace", _OUTPUT_COLOR_SPACE)
    _leaf(group, "ToneMappingDesaturation", "0.5")
    _leaf(group, "AutoInputGamutCompressionEnabled", "true")
    _leaf(group, "IsGraphicsWhiteSameAsProject", "false")
    _leaf(group, "IsColorAwareEffectsEnabledSameAsProject", "false")
    _leaf(group, "FrameRect", f"0,0,{width},{height}")
    owner = ET.SubElement(group, "ComponentOwner", {"Version": "1"})
    ET.SubElement(owner, "Components", {"ObjectRef": at["video_chain"]})

    group = _object(root, "AudioTrackGroup", at["audio_group"])
    inner = ET.SubElement(group, "TrackGroup", {"Version": "1"})
    tracks = ET.SubElement(inner, "Tracks", {"Version": "1"})
    _indexed(tracks, "Track", [("ObjectURef", audio_uid)])
    _leaf(inner, "FrameRate", _AUDIO_FRAME_RATE)
    # 6 whatever the angle count: 24, 78 AND 79 all store it - the default
    # 4-strip sequence's high-water mark (mix 1 + tracks 2..5).
    _leaf(inner, "NextTrackID", "6")
    ET.SubElement(group, "MasterTrack", {"ObjectRef": at["mix_track"]})
    _leaf(group, "ID", uid("audio_group_id"))
    _leaf(group, "AutomationSafeFlags", "0")
    _leaf(group, "NumAdaptiveChannels", "2")

    group = _object(root, "DataTrackGroup", at["data_group"])
    inner = ET.SubElement(group, "TrackGroup", {"Version": "1"})
    _leaf(inner, "FrameRate", str(frame_rate))
    _leaf(inner, "NextTrackID", "1")

    # --- tracks -------------------------------------------------------------
    for index, track_uid in enumerate(video_uids):
        track = _object(root, "VideoClipTrack", uid=track_uid)
        clip_track = ET.SubElement(track, "ClipTrack", {"Version": "2"})
        inner = ET.SubElement(clip_track, "Track", {"Version": "4"})
        if index == 0:
            # Only the first angle's track carries a property bag, and it is
            # NOT targeted - the multicam track above it is the active one.
            node = ET.SubElement(inner, "Node", {"Version": "1"})
            properties = ET.SubElement(node, "Properties", {"Version": "1"})
            _leaf(properties, "TL.SQTrackExpanded", "0")
            _leaf(properties, "TL.SQTrackExpandedHeight", str(_TRACK_HEIGHT))
            _leaf(properties, "MZ.TrackTargeted", "0")
        _leaf(inner, "ID", str(3 + index))
        _leaf(inner, "MediaType", _VIDEO_MEDIA)
        _leaf(inner, "Index", str(index))
        for holder_tag in ("ClipItems", "TransitionItems"):
            holder = ET.SubElement(clip_track, holder_tag, {"Version": "3"})
            _leaf(holder, "MediaType", _VIDEO_MEDIA)
            _leaf(holder, "Index", str(index))

    chain = _object(root, "VideoComponentChain", at["video_chain"])
    ET.SubElement(chain, "ComponentChain", {"Version": "3"})

    track = _object(root, "AudioClipTrack", uid=audio_uid)
    clip_track = ET.SubElement(track, "ClipTrack", {"Version": "2"})
    inner = ET.SubElement(clip_track, "Track", {"Version": "4"})
    node = ET.SubElement(inner, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "TL.SQTrackExpanded", "0")
    _leaf(properties, "TL.SQTrackExpandedHeight", str(_TRACK_HEIGHT))
    _leaf(properties, "MZ.TrackTargeted", "1")
    _leaf(properties, "CM.KeyframeMode", "true")
    _leaf(inner, "ID", "2")
    _leaf(inner, "MediaType", _AUDIO_MEDIA)
    _leaf(inner, "Index", "0")
    for holder_tag in ("ClipItems", "TransitionItems"):
        holder = ET.SubElement(clip_track, holder_tag, {"Version": "3"})
        _leaf(holder, "MediaType", _AUDIO_MEDIA)
        _leaf(holder, "Index", "0")
    audio_track = ET.SubElement(track, "AudioTrack", {"Version": "12"})
    owner = ET.SubElement(audio_track, "ComponentOwner", {"Version": "1"})
    ET.SubElement(owner, "Components", {"ObjectRef": at["strip_chain"]})
    ET.SubElement(audio_track, "Panner", {"ObjectRef": at["panner"]})
    _leaf(audio_track, "ID", uid("audio_track_id"))
    _leaf(audio_track, "NextPannerID", _NEXT_PANNER_ID)

    mix = _object(root, "AudioMixTrack", at["mix_track"])
    audio_track = ET.SubElement(mix, "AudioTrack", {"Version": "12"})
    owner = ET.SubElement(audio_track, "ComponentOwner", {"Version": "1"})
    ET.SubElement(owner, "Components", {"ObjectRef": at["master_strip_chain"]})
    ET.SubElement(audio_track, "Panner", {"ObjectRef": at["default_panner"]})
    _leaf(audio_track, "ID", uid("mix_track_id"))
    _leaf(audio_track, "ChannelType", _CT_BUS)
    _leaf(audio_track, "SubType", "3")
    _leaf(audio_track, "Assign", "0")
    _leaf(audio_track, "NextPannerID", _NEXT_PANNER_ID)
    inner = ET.SubElement(mix, "Track", {"Version": "4"})
    _leaf(inner, "ID", "1")
    _leaf(inner, "MediaType", _AUDIO_MEDIA)
    _leaf(inner, "Index", "0")
    ET.SubElement(mix, "Inlet", {"ObjectRef": at["inlet"]})

    # --- the mix graph -------------------------------------------------------
    _strip_chain(
        _object(root, "AudioComponentChain", at["strip_chain"]),
        at["fader"],
        at["meter"],
    )
    panner = _object(root, "StereoTo16ChannelPanProcessor", at["panner"])
    direct = ET.SubElement(panner, "DirectPanProcessor", {"Version": "2"})
    processor = ET.SubElement(direct, "PanProcessor", {"Version": "3"})
    _audio_component(processor, _PANNER_COMPONENT_ID, "0", [at["balance"]])
    _leaf(processor, "OutputAudioChannelLayout", layout_bus)
    _leaf(direct, "Matrix", "[[0,[0]],[1,[1]]]")

    _strip_chain(
        _object(root, "AudioComponentChain", at["master_strip_chain"]),
        at["fader_master"],
        at["meter_master"],
        layout=layout_bus,
        channel_type=_CT_BUS,
    )
    panner = _object(root, "DefaultPanProcessor", at["default_panner"])
    processor = ET.SubElement(panner, "PanProcessor", {"Version": "3"})
    _audio_component(processor, _PANNER_COMPONENT_ID, "0", [], layout_bus, _CT_BUS)
    _leaf(panner, "DefaultPannerInputChannelType", _CT_BUS)
    _leaf(panner, "DefaultPannerOutputChannelType", _CT_BUS)

    inlet = _object(root, "AudioTrackInlet", at["inlet"])
    sources = ET.SubElement(inlet, "Sources", {"Version": "1"})
    _indexed(sources, "Source", [("ObjectURef", audio_uid)])
    _leaf(inlet, "AudioChannelLayout", layout_bus)
    _leaf(inlet, "ChannelType", _CT_BUS)

    fader = _object(root, "AudioFader", at["fader"])
    _audio_component(fader, "1", "1", [at["volume_0"], at["mute_0"]])
    meter = _object(root, "AudioMeter", at["meter"])
    _audio_component(meter, "2", "2", [])
    balance = _param_object(root, at["balance"], "ScalarParam")
    _leaf(balance, "StartKeyframe", "-91445760000000000,0.5,0,0,0,0,0,0")
    _leaf(balance, "CurrentValue", "0.5")
    _leaf(balance, "Name", "Balance")
    _leaf(balance, "IsInverted", "true")
    fader = _object(root, "AudioFader", at["fader_master"])
    _audio_component(
        fader, "1", "1", [at["volume_master"], at["mute_master"]], layout_bus, _CT_BUS
    )
    meter = _object(root, "AudioMeter", at["meter_master"])
    _audio_component(meter, "2", "2", [], layout_bus, _CT_BUS)
    for strip in ("0", "master"):
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


def build_processed_clips_bin() -> ET.Element:
    """The `Processed Clips` bin Premiere files the source items into."""
    root = ET.Element("PremiereData", {"Version": "3"})
    bin_item = ET.SubElement(
        root,
        "BinProjectItem",
        {"ObjectUID": "", "ClassID": _BIN_CLASS_ID, "Version": "1"},
    )
    project_item = ET.SubElement(bin_item, "ProjectItem", {"Version": "1"})
    node = ET.SubElement(project_item, "Node", {"Version": "1"})
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    _leaf(properties, "Column.PropertyText.Label", _BIN_LABEL)
    _leaf(project_item, "Name", "Processed Clips")
    ET.SubElement(bin_item, "ProjectItemContainer", {"Version": "1"})
    indent_tree(root)
    bin_item.tail = None
    return bin_item


# --- fork (avid2prproj, ADR-0002): N mono audio angles ----------------------


def build_multicam_mono_audio(
    name: str,
    frame_rate: int,
    width: int,
    height: int,
    time_display: int,
    duration_ticks: int,
    video_tracks: int,
    audio_tracks: int,
) -> ET.Element:
    """`build_multicam` for N MONO audio angles, each on its own audio track.

    Shape from Premiere 26.5.2's multicam of one Avid clip (V01 + A01 + A02;
    research PRPROJ-0011): every audio track repeats the stereo-typed strip
    of the single-track case (strip chain, fader, meter, StereoTo16Channel
    panner, balance/volume/mute params) and feeds the same 32-channel bus
    inlet; the master carries one bag-less MONO chain and one single-channel
    vector `(source 0, channel i)` per audio track instead of the stereo
    pair; no Link inside the hidden sequence.
    """
    if audio_tracks < 1:
        raise ValueError("audio_tracks must be at least 1")
    root = build_multicam(name, frame_rate, width, height, time_display, duration_ticks, video_tracks, link=False)
    by_id = {e.get("ObjectID"): e for e in root if e.get("ObjectID")}
    by_uid = {e.get("ObjectUID"): e for e in root if e.get("ObjectUID")}
    next_id = max(int(k) for k in by_id) + 1

    def new_id() -> str:
        nonlocal next_id
        next_id += 1
        return str(next_id - 1)

    group = next(e for e in root if e.tag == "AudioTrackGroup")
    track_ref = group.find("TrackGroup/Tracks/Track")
    track0 = by_uid[track_ref.get("ObjectURef")]
    inlet = next(e for e in root if e.tag == "AudioTrackInlet")

    def copy_strip(track: ET.Element) -> None:
        # Give `track` private copies of its strip objects (chain, fader,
        # meter, panner and their params), the way each Premiere track owns its own.
        mapping: dict[str, str] = {}

        def clone(object_id: str) -> str:
            if object_id in mapping:
                return mapping[object_id]
            original = by_id[object_id]
            duplicate = copy.deepcopy(original)
            fresh = new_id()
            mapping[object_id] = fresh
            duplicate.set("ObjectID", fresh)
            for ref in duplicate.iter():
                target = ref.get("ObjectRef")
                if target is not None and target in by_id and by_id[target].tag in _STRIP_TAGS:
                    ref.set("ObjectRef", clone(target))
            root.append(duplicate)
            by_id[fresh] = duplicate
            return fresh

        for tag in ("AudioTrack/ComponentOwner/Components", "AudioTrack/Panner"):
            ref = track.find(tag)
            ref.set("ObjectRef", clone(ref.get("ObjectRef")))

    track_uids = [track0.get("ObjectUID")]
    for index in range(1, audio_tracks):
        track = copy.deepcopy(track0)
        track_uid = f"{track0.get('ObjectUID')}-{index}"
        track.set("ObjectUID", track_uid)
        track.find("ClipTrack/Track/ID").text = str(2 + index)
        for path in ("ClipTrack/Track/Index", "ClipTrack/ClipItems/Index", "ClipTrack/TransitionItems/Index"):
            track.find(path).text = str(index)
        track.find("AudioTrack/ID").text = f"{track0.findtext('AudioTrack/ID')}-{index}"
        copy_strip(track)
        root.append(track)
        by_uid[track_uid] = track
        track_uids.append(track_uid)
    for parent, tag in ((group.find("TrackGroup/Tracks"), "Track"), (inlet.find("Sources"), "Source")):
        for child in list(parent):
            parent.remove(child)
        _indexed(parent, tag, [("ObjectURef", u) for u in track_uids])

    # Master: mono chains and single-channel vectors, one per audio track.
    master = next(e for e in root if e.tag == "MasterClip")
    chains = master.find("AudioComponentChains")
    for ref in list(chains):
        root.remove(by_id.pop(ref.get("ObjectRef")))
        chains.remove(ref)
    chain_ids = []
    for _ in range(audio_tracks):
        chain = _object(root, "AudioComponentChain", new_id())
        _leaf(chain, "DefaultVol", "true")
        _leaf(chain, "DefaultVolumeComponentID", "1")
        ET.SubElement(chain, "ComponentChain", {"Version": "3"})
        chain_ids.append(chain.get("ObjectID"))
    _indexed(chains, "AudioComponentChain", [("ObjectRef", i) for i in chain_ids])
    groups = by_id[master.find("AudioClipChannelGroups").get("ObjectRef")]
    vectors = groups.find("ClipChannelVectors")
    for ref in list(vectors):
        vector = by_id.pop(ref.get("ObjectRef"))
        for item in vector.findall("ClipChannels/ClipChannelItem"):
            root.remove(by_id.pop(item.get("ObjectRef")))
        root.remove(vector)
        vectors.remove(ref)
    vector_ids = []
    for channel in range(audio_tracks):
        serializer = _object(root, "ClipChannelSerializer", new_id())
        _leaf(serializer, "SourceClipIndex", "0")
        _leaf(serializer, "mSourceChannelIndex", str(channel))
        vector = _object(root, "ClipChannelVectorSerializer", new_id())
        items = ET.SubElement(vector, "ClipChannels", {"Version": "1"})
        _indexed(items, "ClipChannelItem", [("ObjectRef", serializer.get("ObjectID"))])
        _leaf(vector, "ChannelType", "0")
        vector_ids.append(vector.get("ObjectID"))
    _indexed(vectors, "ClipChannelVectorItem", [("ObjectRef", i) for i in vector_ids])
    indent_tree(root)
    return root


_STRIP_TAGS = {"AudioComponentChain", "AudioFader", "AudioMeter", "StereoTo16ChannelPanProcessor",
               "AudioComponentParam", "ScalarParam", "BoolParam"}
