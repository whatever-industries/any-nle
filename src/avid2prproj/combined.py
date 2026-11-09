"""Combine an Avid clip's separate OP-Atom items into one Premiere merged clip.

Generalises py_premiere's `Project.create_merged_clip` (one video + one audio item) to one
video item plus N mono audio items, the shape Premiere 26.5.2 wrote for an Avid clip
(PRPROJ-0011): private copies of every component tagged with MZ.MergeClipUtils keys (audio
copies numbered by track), a hidden 1-video/N-audio-track sequence flagged
BE.Sequence.IsMergedClip, one Link over the placements, and a sequence-backed panel item.
Copies are built from model values (opatom), never by reading media.
"""
from __future__ import annotations

import xml.etree.ElementTree as ET

import uuid

from py_premiere.models import media_import as mi
from py_premiere.models import merged_builder as mb
from py_premiere.models.project_item import (ProjectItem, _add_item_ref, _child_items,
                                             _detach_item_ref)
from py_premiere.enums import ProjectItemType
from py_premiere.xml.mutations import remove_child

from .model import Clip, audio_for_combined_clip, ticks_per_unit
from .opatom import add_audio_item, add_video_item
from .placement import normalize_placement

#: VideoTimeDisplayFormat per frame duration; 24p = 100 as Premiere wrote it in the merged
#: clip's hidden sequence (PRPROJ-0011) and as accepted in PRPROJ-0010.
DISPLAY_FORMATS = {10584000000: 100, **{k[0]: v for k, v in mi.TIMECODE_FORMATS.items() if not k[1]}}


def _parts_bin(target_bin: ProjectItem) -> ProjectItem:
    """The "Processed Clips" subfolder of `target_bin`, created if missing. It holds the parts
    (source files) of the builds in that bin (User decision 2026-10-14: per bin, Premiere's
    own name)."""
    existing = next((c for c in target_bin._children
                     if c._type is ProjectItemType.BIN and c.name == "Processed Clips"), None)
    return existing if existing is not None else target_bin.add_bin("Processed Clips")


def _move_to_processed_clips(project, items, target_bin: ProjectItem | None = None) -> None:
    """File source items into a "Processed Clips" bin: the given bin's subfolder, or (no bin)
    the root one Premiere itself uses for a multicam's angles."""
    from py_premiere.models import multicam_builder as mcb
    from py_premiere.models.project_item import item_container
    if target_bin is None:
        bin_item = project._root_bin("Processed Clips", mcb.build_processed_clips_bin)
    else:
        bin_item = _parts_bin(target_bin)
    bin_container = item_container(bin_item._element)
    for source in items:
        parent = source._parent
        if parent is bin_item:
            continue
        uid = source._element.get("ObjectUID")
        _detach_item_ref(item_container(parent._element), uid)
        parent._children.remove(source)
        _add_item_ref(_child_items(bin_container), uid)
        source._parent = bin_item
        bin_item._children.append(source)


def _tag_copy(project, copy: ProjectItem, merged_name: str, bag: list[tuple[str, str]]) -> None:
    document = project._document
    master = copy._master_element
    master.insert(0, mb.build_master_bag(bag))
    master.find("Name").text = merged_name
    clip_name = document.resolve(master.find("LoggingInfo")).find("ClipName")
    if clip_name is not None:
        clip_name.text = merged_name
    mapping = master.find("DefMappingID")
    if mapping is not None:
        remove_child(master, mapping)


def create_merged(project, clip: Clip, video_item: ProjectItem, audio_items: list[ProjectItem],
                  target_bin: ProjectItem, merged_name: str):
    """`audio_items` must already be in combined order (see `audio_for_combined_clip`).
    Returns `(merged_item, video_copy, audio_copies)`: in a sequence, Premiere places a merged
    clip as its component copies, linked (PRPROJ-0011 placements)."""
    if clip.video is None or not audio_items:
        raise ValueError(f"clip {clip.name!r}: a merged clip needs video and audio")
    ordered = audio_for_combined_clip(clip)
    document = project._document
    timebase = ticks_per_unit(clip.video.rate)
    display = getattr(project,"_any_nle_rewrap_formats",{}).get(clip.video.path,DISPLAY_FORMATS.get(timebase))
    if display is None:
        raise NotImplementedError(f"no observed display format for {clip.video.rate} fps merged clips")
    root = project.root_item
    previous, project._import_target = project._import_target, root
    try:
        video_copy = add_video_item(project, clip.video, clip, merged_name)
        audio_copies = [add_audio_item(project, a, clip, merged_name) for a in ordered]
    finally:
        project._import_target = previous
    _tag_copy(project, video_copy, merged_name, [("MZ.MergeClipUtils.ComponentMasterClipOriginalName", video_item.name)])
    for index, (source, copy) in enumerate(zip(audio_items, audio_copies)):
        _tag_copy(project, copy, merged_name, [
            ("MZ.MergeClipUtils.AudioTrackNumberFromOriginalMergedClip", str(index)),
            ("MZ.MergeClipUtils.ComponentMasterClipOriginalName", source.name)])
    for source, copy in ((video_item, video_copy), *zip(audio_items, audio_copies)):
        source._share_file_identity(copy)

    channels = len(audio_copies)
    audio_rate = int(document.resolve(audio_copies[0]._media_element().find("AudioStream")).findtext("FrameRate"))
    durations = [clip.video.duration] + [a.duration for a in ordered]
    end_ticks = max(d - d % timebase for d in durations)
    width, height = clip.video.width, clip.video.height

    sequence_uid = str(uuid.uuid4())
    video_track_uid = str(uuid.uuid4())
    audio_track_uids = [str(uuid.uuid4()) for _ in range(channels)]
    group_chain_id = project._register(mb.build_empty_video_chain())
    track_chain_ids, track_pan_ids = [], []
    for _ in range(channels):
        fader_id = project._register(mb.build_fader(audio_rate, [project._register(mb.build_volume_param()), project._register(mb.build_mute_param())]))
        meter_id = project._register(mb.build_meter(audio_rate))
        track_chain_ids.append(project._register(mb.build_fader_chain(fader_id, meter_id)))
        track_pan_ids.append(project._register(mb.build_pan_processor(audio_rate)))
    mix_fader_id = project._register(mb.build_fader(audio_rate, [project._register(mb.build_volume_param()), project._register(mb.build_mute_param())]))
    mix_chain_id = project._register(mb.build_fader_chain(mix_fader_id, project._register(mb.build_meter(audio_rate))))
    mix_track_id = project._register(mb.build_mix_track(mix_chain_id, project._register(mb.build_pan_processor(audio_rate)),
                                                        project._register(mb.build_inlet(audio_track_uids))))
    video_group_id = project._register(mb.build_video_group(video_track_uid, timebase, width, height, group_chain_id))
    audio_group_id = project._register(mb.build_audio_group(audio_track_uids, audio_rate, mix_track_id))
    data_group_id = project._register(mb.build_data_group(timebase))
    document.attach_object(mb.build_video_track(video_track_uid))
    for index, track_uid in enumerate(audio_track_uids):
        document.attach_object(mb.build_audio_track(track_uid, track_chain_ids[index], track_pan_ids[index], track_id=2 + index, index=index))
    sequence_element = document.attach_object(mb.build_merged_sequence(
        sequence_uid, merged_name, display, end_ticks, width, height, video_group_id, audio_group_id, data_group_id))
    from py_premiere.parsers.sequence import parse_sequence
    sequence = parse_sequence(document, project, sequence_element)
    project._sequences.append(sequence)

    video_placed = sequence._video_tracks[0].add_clip(video_copy)
    normalize_placement(video_placed)
    audio_placed_ids = []
    for track, copy in zip(sequence._audio_tracks, audio_copies):
        placed = track.add_clip(copy)
        project._narrow_merged_channel(placed, 0)  # each A0x copy is already mono
        normalize_placement(placed)
        audio_placed_ids.append(placed._element.get("ObjectID"))
    link_id = project._register(mb.build_link([video_placed._element.get("ObjectID"), *audio_placed_ids]))
    sequence_element.find("PersistentGroupContainer/LinkContainer/Links/Link").set("ObjectRef", link_id)

    video_source_id = project._register(mb.build_sequence_source("VideoSequenceSource", sequence_uid, end_ticks))
    audio_source_id = project._register(mb.build_sequence_source("AudioSequenceSource", sequence_uid, end_ticks))
    secondary_id = project._register(mi.new_secondary_content(audio_source_id, 0))
    panel_vector_ids = [project._register(mi.new_channel_vector([project._register(mi.new_channel_serializer(i, 0))], 0)) for i in range(channels)]
    groups_id = project._register(mi.new_audio_channel_groups(panel_vector_ids))
    master_chain_ids = [project._register(mi.new_master_audio_chain(1, 0)) for _ in range(channels)]
    audio_template_id = project._register(mb.build_panel_audio_template(audio_source_id, secondary_id))
    video_template_id = project._register(mb.build_panel_video_template(video_source_id))
    logging_id = project._register(mb.build_panel_logging(video_item.name, display, end_ticks, timebase))
    master_uid = str(uuid.uuid4())
    master_element = document.attach_object(mb.build_panel_master(
        master_uid, logging_id, master_chain_ids, audio_template_id, video_template_id, groups_id, merged_name))
    item_uid = str(uuid.uuid4())
    item_element = document.attach_object(mb.build_panel_item(item_uid, master_uid, merged_name))

    root_container = root.element_container() if hasattr(root, "element_container") else root._element.find("ProjectItemContainer")
    for copy in (video_copy, *audio_copies):  # copies live only inside the hidden sequence
        _detach_item_ref(root_container, copy._element.get("ObjectUID"))
        document.remove_object(copy._element)
        root._children.remove(copy)
    target_container = target_bin._element.find("ProjectItemContainer")
    _add_item_ref(_child_items(target_container), item_uid)
    _move_to_processed_clips(project, (video_item, *audio_items), target_bin)  # parts, not beside the build
    item = ProjectItem(item_element, project, ProjectItemType.CLIP)
    item._parent = target_bin
    item._master_element = master_element
    item._clip_elements = [document.by_object_id[audio_template_id], document.by_object_id[video_template_id]]
    item._default_out_ticks = end_ticks
    item._sequence_uid = sequence_uid
    target_bin._children.append(item)
    project._items_by_master_uid = None
    project._items_by_sequence_uid = None
    return item, video_copy, audio_copies


def create_grouped(project, clip: Clip, video_item: ProjectItem | None, audio_items: list[ProjectItem],
                   target_bin: ProjectItem, grouped_name: str) -> ProjectItem:
    """A multicam source clip over one video item and N mono audio items (A01 first), the
    shape Premiere 26.5.2 wrote for an Avid clip (PRPROJ-0011), except that Premiere put A02
    on the first track there. Sources move into the root "Processed Clips" bin, as Premiere does;
    the grouped item lands in `target_bin` under the Avid Name. Nothing is copied."""
    from py_premiere.models import multicam_builder as mcb
    from py_premiere.models.project_item import item_container
    from py_premiere.parsers.sequence import parse_sequence

    if not audio_items or (video_item is None and len(audio_items) < 2):
        raise ValueError(f"clip {clip.name!r}: a grouped clip needs video and audio, or a stereo pair")
    rate = clip.video.rate if clip.video is not None else clip.rate
    if rate is None:
        raise ValueError(f"clip {clip.name!r}: no frame rate for an audio-only build")
    timebase = ticks_per_unit(rate)
    display = getattr(project,"_any_nle_rewrap_formats",{}).get(clip.video.path if clip.video else None,DISPLAY_FORMATS.get(timebase))
    if display is None:
        raise NotImplementedError(f"no observed display format for {rate} fps grouped clips")
    document = project._document
    durations = ([clip.video.duration] if video_item is not None else []) + [a.duration for a in clip.audio]
    end_ticks = max(d - d % timebase for d in durations)
    width, height = (clip.video.width, clip.video.height) if clip.video is not None else (1920, 1080)
    # Audio-only (a stereo pair with no picture): the same shape with no video track (UNVERIFIED).
    fragment = mcb.build_multicam_mono_audio(grouped_name, timebase, width, height,
                                             display, end_ticks,
                                             1 if video_item is not None else 0, len(audio_items))
    spliced = project._splice_fragment(fragment)
    sequence_element, item_element, master_element = (spliced[k] for k in ("Sequence", "ClipProjectItem", "MasterClip"))
    sequence = parse_sequence(document, project, sequence_element)
    project._sequences.append(sequence)
    if video_item is not None:
        normalize_placement(sequence._video_tracks[0].add_clip(video_item))
    for track, audio in zip(sequence._audio_tracks, audio_items):
        placed = track.add_clip(audio)
        normalize_placement(placed)
        chain = document.resolve(placed._element.find("ClipTrackItem/ComponentOwner/Components"))
        inner = chain.find("ComponentChain")
        node = None if inner is None else inner.find("Node")
        if node is not None:  # bag-less mono DefaultVol chain, as Premiere writes inside a multicam
            inner.remove(node)
            inner.text = "\n\t\t"

    _premiere_balances(document, sequence_element)
    groups = _channel_groups(clip)
    if any(len(g) > 1 for g in groups):
        _stereo_master_groups(document, master_element, groups)

    _move_to_processed_clips(project, tuple(i for i in (video_item, *audio_items) if i is not None), target_bin)
    item_uid = item_element.get("ObjectUID")
    _add_item_ref(_child_items(target_bin._element.find("ProjectItemContainer")), item_uid)
    item = ProjectItem(item_element, project, ProjectItemType.CLIP)
    item._parent = target_bin
    item._master_element = master_element
    item._clip_elements = [document.resolve(ref) for ref in master_element.findall("Clips/Clip")]
    item._default_out_ticks = end_ticks
    item._sequence_uid = sequence_element.get("ObjectUID")
    item._channel_groups = groups
    target_bin._children.append(item)
    project._items_by_master_uid = None
    project._items_by_sequence_uid = None
    return item


def _channel_groups(clip: Clip) -> list[tuple[int, ...]]:
    """The grouped clip's channel groups, as inner-track (bus channel) indices in combined
    (A01-first) order: an Avid stereo pair becomes one 2-channel group at its left track's
    position, every other track a 1-channel group."""
    combined = list(audio_for_combined_clip(clip))
    index = {f.avid_track: i for i, f in enumerate(combined)}
    right_of = {r: l for l, r in clip.stereo_pairs}
    left_of = {l: r for l, r in clip.stereo_pairs}
    groups = []
    for i, f in enumerate(combined):
        if f.avid_track in right_of:
            continue
        if f.avid_track in left_of:
            groups.append((i, index[left_of[f.avid_track]]))
        else:
            groups.append((i,))
    return groups


def group_for_channel(groups: list[tuple[int, ...]], channel: int) -> int:
    return next(g for g, chans in enumerate(groups) if channel in chans)


_STEREO_LAYOUT = '[{"channellabel":100},{"channellabel":101}]'


def _stereo_master_groups(document, master: ET.Element, groups: list[tuple[int, ...]]) -> None:
    """Rewrite a mono-per-track multicam master so each group is one channel vector: a
    stereo group gets ChannelType 1 with both bus channels and a stereo chain, the shape of
    py_premiere's (Premiere-verified) single-track stereo multicam master. UNVERIFIED for
    several tracks; awaiting User read-back."""
    chains = master.find("AudioComponentChains")
    vectors_ref = document.resolve(master.find("AudioClipChannelGroups"))
    vectors = vectors_ref.find("ClipChannelVectors")
    old_vectors = [document.resolve(v) for v in vectors.findall("ClipChannelVectorItem")]
    old_chains = [document.resolve(c) for c in chains.findall("AudioComponentChain")]
    for item in list(vectors):
        vectors.remove(item)
    for item in list(chains):
        chains.remove(item)
    for g, channels in enumerate(groups):
        vector = old_vectors[channels[0]]
        chain = old_chains[channels[0]]
        if len(channels) > 1:
            serials = vector.find("ClipChannels")
            for extra in channels[1:]:
                donor = old_vectors[extra].find("ClipChannels/ClipChannelItem")
                ref = ET.SubElement(serials, "ClipChannelItem", {"Index": str(len(serials)), "ObjectRef": donor.get("ObjectRef")})
                ref.tail = serials[0].tail
            vector.find("ChannelType").text = "1"
            for tag, text in (("AudioChannelLayout", _STEREO_LAYOUT), ("ChannelType", "1")):
                leaf = chain.find(tag)
                if leaf is None:
                    leaf = ET.SubElement(chain, tag)
                    leaf.tail = chain[-2].tail if len(chain) > 1 else None
                leaf.text = text
        ET.SubElement(vectors, "ClipChannelVectorItem", {"Index": str(g), "ObjectRef": vector.get("ObjectID")})
        ET.SubElement(chains, "AudioComponentChain", {"Index": str(g), "ObjectRef": chain.get("ObjectID")})
    for v in old_vectors:
        if v.get("ObjectID") not in {x.get("ObjectRef") for x in vectors}:
            document.remove_object(v)
    for c in old_chains:
        if c.get("ObjectID") not in {x.get("ObjectRef") for x in chains}:
            document.remove_object(c)


def narrow_to_group(project, placed, groups: list[tuple[int, ...]], group: int) -> None:
    """Make a placed multicam audio item play one channel group. A 1-channel group uses
    py_premiere's verified mono narrowing (channel = bus channel); a stereo group keeps both
    channels' SecondaryContents under a stereo layout and selects the group through
    OrigChGrp/SecondaryIndex, extending the same shape (UNVERIFIED)."""
    channels = groups[group]
    if len(channels) == 1 and all(len(g) == 1 for g in groups):
        project._narrow_merged_channel(placed, channels[0])
        return
    document = project._document
    clip = placed._clip_element
    layout = clip.find("AudioChannelLayout")
    if layout is not None:
        layout.text = _STEREO_LAYOUT if len(channels) == 2 else '[{"channellabel":0}]'
    contents = clip.find("SecondaryContents")
    if contents is not None:
        kept = []
        for entry in contents.findall("SecondaryContentItem"):
            secondary = document.resolve(entry)
            if int(secondary.findtext("ChannelIndex") or "0") in channels and len(kept) < len(channels):
                kept.append(entry)
            else:
                document.remove_object(secondary)
                remove_child(contents, entry)
        for i, entry in enumerate(kept):
            entry.set("Index", str(i))
    chain_ref = placed._element.find("ClipTrackItem/ComponentOwner/Components")
    if chain_ref is not None:
        chain = document.resolve(chain_ref)
        if len(channels) == 1:
            inner = chain.find("ComponentChain")
            node = None if inner is None else inner.find("Node")
            if inner is not None and node is not None:
                inner.remove(node)
                inner.text = "\n\t\t"
        else:
            declare_stereo_chain(project, placed)
    if group:
        from py_premiere.xml.mutations import append_leaf
        append_leaf(clip, "SecondaryIndex", str(channels[0]))
        orig = placed._subclip_element.find("OrigChGrp")
        if orig is not None:
            orig.text = str(group)


_BALANCE_CLASS_ID = "a714635e-a628-4b27-9d59-77eba47dbc1a"


def _premiere_balances(document, sequence_element: ET.Element) -> None:
    """Inner multicam track balances as Premiere 26.5.2 writes them for its own multicams,
    mono and stereo alike (PRPROJ-0019): an AudioComponentParam (ClassID a714635e) named
    Balance with IsInverted true; track 0 stores StartKeyframe/CurrentValue 0, the other
    tracks store no value. py_premiere's builder wrote ScalarParams at 0.5 on every track,
    which played a stereo group on the left only."""
    for group_ref in sequence_element.findall("TrackGroups/TrackGroup/Second"):
        group = document.resolve(group_ref)
        if group.tag != "AudioTrackGroup":
            continue
        for position, track_ref in enumerate(group.findall("TrackGroup/Tracks/Track")):
            track = document.resolve(track_ref)
            panner = document.resolve(track.find("AudioTrack/Panner"))
            for param_ref in panner.iter("Param"):
                param = document.resolve(param_ref)
                if param.findtext("Name") != "Balance":
                    continue
                param.tag = "AudioComponentParam"
                param.set("ClassID", _BALANCE_CLASS_ID)
                param.set("Version", "10")
                indent = param.text
                for child in list(param):
                    param.remove(child)
                leaves = [("Name", "Balance"), ("IsInverted", "true")]
                if position == 0:
                    leaves = [("StartKeyframe", "-91445760000000000,0.,0,0,0,0,0,0"), ("CurrentValue", "0")] + leaves
                for i, (tag, text) in enumerate(leaves):
                    leaf = ET.SubElement(param, tag)
                    leaf.text = text
                    leaf.tail = indent if i < len(leaves) - 1 else (indent or "")[:-1]


_PANNER_CLASS_ID = "33a94283-ee2c-11d5-abc4-c1cd7f9e3c10"
_PAN_PARAM_CLASS_ID = "a714635e-a628-4b27-9d59-77eba47dbc1a"


def set_clip_pan(project, placed, full) -> None:
    """Pan a placed mono audio edit: `full` is "left", "right" or a percent -100..100. An
    AudioClipTrackItem <Panner> to a MonoToStereoPanProcessor whose AudioComponentParam "Pan"
    (IsInverted) stores (pan + 100) / 200 (User-confirmed 0, 0.25, 0.5, 0.75, 1.0 -> -100, -50,
    0, +50, +100; PRPROJ-0021/0023). Premiere itself omits the value for +100; an explicit 1.0
    reads the same."""
    if not isinstance(full, str):
        value = (float(full) + 100.0) / 200.0
        if not 0.0 <= value <= 1.0:
            raise ValueError(f"pan {full}: expected -100..100")
        _panner(project, placed, repr(value))
        return
    if full == "left":
        _panner(project, placed, "0")
    elif full == "right":
        _panner(project, placed, None)
    else:
        raise ValueError(f"pan {full!r}: expected 'left', 'right' or a percent")


def _panner(project, placed, value: str | None) -> None:
    panner = ET.Element("MonoToStereoPanProcessor", {"ClassID": _PANNER_CLASS_ID, "Version": "1"})
    pan = ET.SubElement(panner, "PanProcessor", {"Version": "3"})
    audio = ET.SubElement(pan, "AudioComponent", {"Version": "3"})
    component = ET.SubElement(audio, "Component", {"Version": "7"})
    params = ET.SubElement(component, "Params", {"Version": "1"})
    param = ET.Element("AudioComponentParam", {"ClassID": _PAN_PARAM_CLASS_ID, "Version": "10"})
    leaves = [("IsTimeVarying", "false"), ("Name", "Pan"), ("IsInverted", "true")]
    if value is not None:
        start = "0." if value == "0" else value
        leaves = [("StartKeyframe", f"-91445760000000000,{start},0,0,0,0,0,0"), ("CurrentValue", value)] + leaves
    for tag, text in leaves:
        ET.SubElement(param, tag).text = text
    param_id = project._register(param)
    ET.SubElement(params, "Param", {"Index": "0", "ObjectRef": param_id})
    ET.SubElement(component, "ID").text = "4294967040"
    for tag, text in (("AudioChannelLayout", '[{"channellabel":0}]'), ("AudioComponentType", "0"),
                      ("FrameRate", "5292000"), ("ChannelType", "0")):
        ET.SubElement(audio, tag).text = text
    ET.SubElement(pan, "OutputAudioChannelLayout").text = _STEREO_LAYOUT
    panner_id = project._register(panner)
    element = placed._element
    ref = ET.SubElement(element, "Panner", {"ObjectRef": panner_id})
    ref.tail = element[-2].tail
    element[-2].tail = element[0].tail


def _move_items(project, items, destination: ProjectItem) -> None:
    from py_premiere.models.project_item import item_container
    container = item_container(destination._element)
    for source in items:
        parent = source._parent
        if parent is destination:
            continue
        uid = source._element.get("ObjectUID")
        _detach_item_ref(item_container(parent._element), uid)
        parent._children.remove(source)
        _add_item_ref(_child_items(container), uid)
        source._parent = destination
        destination._children.append(source)


def create_stereo_nest(project, audio_items: list[ProjectItem], target_bin: ProjectItem, name: str,
                       preset: str) -> ProjectItem:
    """An audio-only stereo build as a nested sequence: the left part on audio track 1 panned
    fully left, the right part on track 2 panned fully right. Replaces the zero-video multicam,
    whose playback left that media silent in Premiere 26.5.2 for the rest of the session
    (User, 2026-10-14). The parts go to the bin's Processed Clips."""
    from py_premiere import Time
    if len(audio_items) != 2:
        raise ValueError("a stereo build needs exactly two mono parts")
    sequence = project.add_sequence(name, preset)
    item = next(c for c in project.root_item.children
                if getattr(c, "_sequence_uid", None) == sequence.sequence_id)
    _move_items(project, [item], target_bin)
    for track, part, side in zip(sequence.audio_tracks, audio_items, ("left", "right")):
        placed = track.add_clip(part, Time(0))
        set_clip_pan(project, placed, side)
    _move_to_processed_clips(project, tuple(audio_items), target_bin)
    # Transition handle checks use this in-memory bound, as for grouped/merged builds.
    item._default_out_ticks = sequence.end.ticks
    item._stereo_nest = True
    return item


def declare_stereo_chain(project, placed) -> None:
    """Make a placed audio edit's chain stereo, as Premiere writes for a stereo edit: the
    per-channel volume component, and AudioChannelLayout L/R + ChannelType 1 after
    ComponentChain. Without it a stereo edit (multicam group or nested sequence) played left
    only (PRPROJ-0021)."""
    document = project._document
    chain_ref = placed._element.find("ClipTrackItem/ComponentOwner/Components")
    if chain_ref is None:
        return
    chain = document.resolve(chain_ref)
    if chain.find("DefaultChannelVolumeComponentID") is None:
        default = chain.find("DefaultVolumeComponentID")
        leaf = ET.Element("DefaultChannelVolumeComponentID")
        leaf.text = "2"
        if default is not None:
            leaf.tail = default.tail
            chain.insert(list(chain).index(default) + 1, leaf)
        else:
            chain.insert(0, leaf)
    for tag, text in (("AudioChannelLayout", _STEREO_LAYOUT), ("ChannelType", "1")):
        leaf = chain.find(tag)
        if leaf is None:
            leaf = ET.SubElement(chain, tag)
            previous = chain[-2]
            leaf.tail, previous.tail = previous.tail, chain[0].tail
        leaf.text = text
