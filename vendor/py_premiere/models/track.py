"""The `Track` model."""

from __future__ import annotations

import copy
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import TYPE_CHECKING

from ..xml.document import ReferenceIndex
from ..xml.mutations import (
    append_child,
    append_leaf,
    insert_before,
    insert_leaf_before,
    remove_child,
    set_elided_flag,
)
from .component import Component, ComponentParam
from .graphic_builder import (
    _GRAPHIC_BASE_TICKS,
    build_empty_channel_groups,
    build_graphic_chain,
    build_graphic_clip,
    build_graphic_logging,
    build_graphic_master,
    build_graphic_media,
    build_graphic_source,
    build_graphic_stream,
    build_source_text_param,
    build_text_component,
    build_text_params,
)
from .mgt_builder import (
    MASTER_SLOT,
    PLACEMENT_SLOT,
    Template,
    attach_component,
    build_blueprint_chain,
    build_capsule_component,
    build_capsule_params,
    build_mgt_clip,
    build_mgt_logging,
    build_mgt_master,
    build_mgt_media,
    build_mgt_source,
    build_mgt_stream,
)
from .named_list import NamedList
from .time import TICKS_PER_SECOND, Time, validate_time
from .track_item import TrackItem
from .transition import Transition
from .validators import (
    validate_bool,
    validate_path,
    validate_string,
    validate_vector2,
)

if TYPE_CHECKING:
    from typing import Iterator

    from ..xml import PremiereDocument
    from .project_item import ProjectItem
    from .sequence import Sequence

#: ClassIDs of the objects a clip placement synthesizes.
_SUBCLIP_CLASS_ID = "e0c58dc9-dbdd-4166-aef7-5db7e3f22e84"
_VIDEO_CHAIN_CLASS_ID = "0970e08a-f58f-4108-b29a-1a717b8e12e2"
_AUDIO_CHAIN_CLASS_ID = "3cb131d1-d3c0-47ae-a19a-bdf75ea11674"
_VIDEO_ITEM_CLASS_ID = "368b0406-29e3-4923-9fcd-094fbf9a1089"
_AUDIO_ITEM_CLASS_ID = "064ec682-9ba6-11d5-af2d-9ca32c7d6164"
_SECONDARY_CONTENT_CLASS_ID = "f9d004b5-cb04-4e2f-af6f-64fadc2c4be9"

#: Premiere writes this for every SDR placement (26.x default).
_TONE_MAP_DEFAULT = '{"peak":-1,"version":3}'

#: The static-value keyframe Premiere writes on a muted audio track's `Mute`
#: parameter (captured from ExtendScript's `Track.setMute(1)`).
_MUTE_START_KEYFRAME = "-91445760000000000,true,0,0,0,0,0,0"

#: A fresh graphic runs for Premiere's still default (5 s, 66_eg_text).
_GRAPHIC_DEFAULT_TICKS = 5 * TICKS_PER_SECOND

_validate_graphic_text = validate_string(allow_empty=False)
_validate_graphic_name = validate_string(allow_empty=False)
_validate_template_path = validate_path(must_exist=True, must_be_file=True)

#: The Essential Graphics panel keys a placement back to the template it
#: was imported from through this property.
_MGT_IDENTIFIER = "BE.RushInspectorPanel.MogrtIdentifier"


def _tag_as_template(item: ET.Element, capsule_id: str) -> None:
    core = item.find("ClipTrackItem/TrackItem")
    if core is None:
        raise ValueError("placement has no TrackItem")
    node = ET.Element("Node", {"Version": "1"})
    node.text = "\n\t\t\t\t\t"
    node.tail = "\n\t\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t\t"
    properties.tail = "\n\t\t\t\t"
    _leaf(properties, _MGT_IDENTIFIER, capsule_id, "\n\t\t\t\t\t")
    core.insert(0, node)


def _build_component_model(
    document: PremiereDocument,
    component_id: str,
    param_ids: list[str],
    track_item: TrackItem,
) -> Component:
    # The model wrapper for a freshly synthesized component, mirroring what
    # the parser builds from the same elements.
    component = Component(document.by_object_id[component_id], track_item)
    for param_id in param_ids:
        component._properties.append(
            ComponentParam(document.by_object_id[param_id], document, component)
        )
    return component


def _leaf(parent: ET.Element, tag: str, text: str, tail: str) -> ET.Element:
    element = ET.SubElement(parent, tag)
    element.text = text
    element.tail = tail
    return element


#: Display names Premiere writes for a transition's matchName. Not derivable
#: (`ADBE Additive Dissolve` displays as `Additive Dissolve (Legacy)`), so
#: only names read back out of Premiere's own output belong here.
TRANSITION_DISPLAY_NAMES = {
    "ADBE Additive Dissolve": "Additive Dissolve (Legacy)",
    "ADBE Band Wipe": "Band Wipe",
    "Constant Power": "Constant Power",
}

#: ClassIDs of the two transition track item classes.
_VIDEO_TRANSITION_CLASS_ID = "3eeaed31-f78e-4144-b8e8-077656517181"
_AUDIO_TRANSITION_CLASS_ID = "23f687b6-9c5a-42f4-bea1-e7b3e28e7082"


def _attach_transition_ref(core: ET.Element, tag: str, object_id: str) -> None:
    append_leaf(core, tag, "")
    reference = core.find(tag)
    if reference is None:
        raise ValueError(f"could not attach <{tag}>")
    reference.text = None
    reference.set("ObjectRef", object_id)


def _detach_from_links(document: PremiereDocument, object_id: str | None) -> None:
    """Drop a track item from every `Link` that holds it.

    A link groups the track items an A/V clip was placed as. Removing one
    without unhooking it leaves the `Link` pointing at an ObjectID that no
    longer exists, which the saved file carries and a re-parse resolves to
    nothing. A link left with fewer than two members is not a link any more,
    so it goes too.
    """
    if object_id is None:
        return
    for link in list(document.root):
        if link.tag != "Link":
            continue
        items = link.find("TrackItemGroup/TrackItems")
        if items is None:
            continue
        for reference in list(items):
            if reference.get("ObjectRef") == object_id:
                remove_child(items, reference)
        if len(items) < 2:
            document.remove_object(link)
            continue
        for index, reference in enumerate(items):
            reference.set("Index", str(index))


def _source_duration(clip: TrackItem) -> int:
    # How much footage the clip's source holds, in source ticks.
    item = clip.project_item
    if item is None or item._default_out_ticks is None:
        raise ValueError(
            "cannot size a transition: the clip's source duration is unknown"
        )
    return item._default_out_ticks


def _handle_before(clip: TrackItem) -> int:
    """Footage the clip could play BEFORE its in point, in timeline ticks."""
    return int(clip.in_point.ticks / clip.speed)


def _handle_after(clip: TrackItem) -> int:
    """Footage the clip could play PAST its out point, in timeline ticks."""
    spare = _source_duration(clip) - clip.out_point.ticks
    return max(0, int(spare / clip.speed))


def _new_transition_element(
    media_type: str,
    start: int,
    end: int,
    offset: int,
    on_a_cut: bool,
    at_start: bool,
    name: str,
    match_name: str,
    channel_layout: str | None = None,
) -> ET.Element:
    # `Alignment` is the tick offset of the cut within the transition, so a
    # head one stores 0, a tail one its whole duration, and one on a cut
    # whatever fitted before the cut. `Start` is elided at zero like every
    # other track item. A transition on a cut has a real clip on BOTH sides.
    video = media_type == "Video"
    element = ET.Element(
        "VideoTransitionTrackItem" if video else "AudioTransitionTrackItem",
        {
            "ClassID": (
                _VIDEO_TRANSITION_CLASS_ID if video else _AUDIO_TRANSITION_CLASS_ID
            ),
            "Version": "6" if video else "4",
        },
    )
    element.text = "\n\t\t"
    inner = ET.SubElement(element, "TransitionTrackItem", {"Version": "3"})
    inner.text = "\n\t\t\t"
    inner.tail = "\n\t"
    item = ET.SubElement(inner, "TrackItem", {"Version": "4"})
    item.text = "\n\t\t\t\t"
    item.tail = "\n\t\t\t"
    if start:
        _leaf(item, "Start", str(start), "\n\t\t\t\t")
    _leaf(item, "End", str(end), "\n\t\t\t")
    _leaf(inner, "Alignment", str(offset), "\n\t\t\t")
    _leaf(inner, "DisplayName", name, "\n\t\t\t")
    _leaf(inner, "MatchName", match_name, "\n\t\t\t")
    outgoing = "true" if on_a_cut or not at_start else "false"
    incoming = "true" if on_a_cut or at_start else "false"
    _leaf(inner, "HasOutgoingClip", outgoing, "\n\t\t\t")
    _leaf(inner, "HasIncomingClip", incoming, "\n\t\t")
    if channel_layout is not None:
        # An audio transition repeats the layout of the `AudioClip` it sits
        # on - it routes that clip's channels. Matched against Premiere's own
        # stereo crossfade; a mono clip's `[{"channellabel":0}]` follows the
        # same rule but has no fixture yet.
        inner.tail = "\n\t\t"
        _leaf(element, "AudioChannelLayout", channel_layout, "\n\t")
    return element


def _referenced_elsewhere(
    index: ReferenceIndex, target: ET.Element, doomed: list[ET.Element]
) -> bool:
    # True when a top-level object outside `doomed` still points at `target`.
    # An object with no ObjectID cannot be referenced by one, but it also
    # cannot be proven unreferenced, so it stays.
    if target.get("ObjectID") is None:
        return True
    return bool(index.referrers_outside(target, [target, *doomed]))


def _new_subclip_element(clip_id: str, master_uid: str, name: str) -> ET.Element:
    element = ET.Element(
        "SubClip",
        {"ObjectID": "", "ClassID": _SUBCLIP_CLASS_ID, "Version": "6"},
    )
    element.text = "\n\t\t"
    ET.SubElement(element, "Clip", {"ObjectRef": clip_id}).tail = "\n\t\t"
    ET.SubElement(element, "MasterClip", {"ObjectURef": master_uid}).tail = "\n\t\t"
    _leaf(element, "Name", name, "\n\t\t")
    _leaf(element, "OrigChGrp", "0", "\n\t")
    return element


def _new_video_chain_element() -> ET.Element:
    # The default (empty) chain Premiere writes for a fresh video placement:
    # intrinsics stay synthesized-at-runtime, nothing is materialized.
    element = ET.Element(
        "VideoComponentChain",
        {"ObjectID": "", "ClassID": _VIDEO_CHAIN_CLASS_ID, "Version": "3"},
    )
    element.text = "\n\t\t"
    _leaf(element, "DefaultMotion", "true", "\n\t\t")
    _leaf(element, "DefaultOpacity", "true", "\n\t\t")
    _leaf(element, "DefaultMotionComponentID", "1", "\n\t\t")
    _leaf(element, "DefaultOpacityComponentID", "2", "\n\t\t")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t"
    chain.tail = "\n\t"
    return element


def _new_audio_chain_element() -> ET.Element:
    element = ET.Element(
        "AudioComponentChain",
        {"ObjectID": "", "ClassID": _AUDIO_CHAIN_CLASS_ID, "Version": "4"},
    )
    element.text = "\n\t\t"
    _leaf(element, "DefaultVol", "true", "\n\t\t")
    _leaf(element, "DefaultVolumeComponentID", "1", "\n\t\t")
    chain = ET.SubElement(element, "ComponentChain", {"Version": "3"})
    chain.text = "\n\t\t\t"
    chain.tail = "\n\t"
    node = ET.SubElement(chain, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t\t"
    properties.tail = "\n\t\t\t"
    _leaf(properties, "MZ.ComponentChain.ActiveComponentID", "1", "\n\t\t\t\t\t")
    _leaf(
        properties,
        "MZ.ComponentChain.ActiveComponentParamIndex",
        "4294967295",
        "\n\t\t\t\t",
    )
    return element


def _new_secondary_content_element(content_ref: str, channel: str) -> ET.Element:
    element = ET.Element(
        "SecondaryContent",
        {"ObjectID": "", "ClassID": _SECONDARY_CONTENT_CLASS_ID, "Version": "1"},
    )
    element.text = "\n\t\t"
    ET.SubElement(element, "Content", {"ObjectRef": content_ref}).tail = "\n\t\t"
    _leaf(element, "ChannelIndex", channel, "\n\t")
    return element


def _new_clip_track_item(chain_id: str, subclip_id: str, end_ticks: int) -> ET.Element:
    inner = ET.Element("ClipTrackItem", {"Version": "8"})
    inner.text = "\n\t\t\t"
    inner.tail = "\n\t\t"
    owner = ET.SubElement(inner, "ComponentOwner", {"Version": "1"})
    owner.text = "\n\t\t\t\t"
    owner.tail = "\n\t\t\t"
    ET.SubElement(owner, "Components", {"ObjectRef": chain_id}).tail = "\n\t\t\t"
    track_item = ET.SubElement(inner, "TrackItem", {"Version": "4"})
    track_item.text = "\n\t\t\t\t"
    track_item.tail = "\n\t\t\t"
    _leaf(track_item, "End", str(end_ticks), "\n\t\t\t")
    ET.SubElement(inner, "SubClip", {"ObjectRef": subclip_id}).tail = "\n\t\t"
    return inner


def _new_video_track_item_element(
    chain_id: str,
    subclip_id: str,
    end_ticks: int,
    video_stream: ET.Element | None,
) -> ET.Element:
    element = ET.Element(
        "VideoClipTrackItem",
        {"ObjectID": "", "ClassID": _VIDEO_ITEM_CLASS_ID, "Version": "8"},
    )
    element.text = "\n\t\t"
    element.append(_new_clip_track_item(chain_id, subclip_id, end_ticks))
    aspect = "1,1"
    frame_rect = None
    if video_stream is not None:
        aspect = video_stream.findtext("PixelAspectRatio") or "1,1"
        frame_rect = video_stream.findtext("FrameRect")
    _leaf(element, "PixelAspectRatio", aspect, "\n\t\t")
    _leaf(
        element,
        "ToneMapSettings",
        _TONE_MAP_DEFAULT,
        "\n\t\t" if frame_rect is not None else "\n\t",
    )
    if frame_rect is not None:
        _leaf(element, "FrameRect", frame_rect, "\n\t")
    return element


def _new_audio_track_item_element(
    chain_id: str, subclip_id: str, end_ticks: int
) -> ET.Element:
    element = ET.Element(
        "AudioClipTrackItem",
        {"ObjectID": "", "ClassID": _AUDIO_ITEM_CLASS_ID, "Version": "11"},
    )
    element.text = "\n\t\t"
    element.append(_new_clip_track_item(chain_id, subclip_id, end_ticks))
    _leaf(element, "ID", str(uuid.uuid4()), "\n\t\t")
    _leaf(element, "PreRenderComponentChainHashVersion", "1", "\n\t")
    return element


class Track:
    """A video or audio track of a sequence."""

    def __init__(
        self,
        _element: ET.Element,
        sequence: Sequence,
        index: int,
        track_id: int,
        media_type: str,
    ) -> None:
        self._element = _element
        self.sequence = sequence
        self._index = index
        self._id = track_id
        self._media_type = media_type
        self._clips: list[TrackItem] = []
        self._transitions: list[Transition] = []

    @property
    def index(self) -> int:
        """Zero-based position within the track group. Read-only."""
        return self._index

    @property
    def id(self) -> int:
        """The track's per-group identifier. Read-only."""
        return self._id

    @property
    def media_type(self) -> str:
        """`"Video"` or `"Audio"`. Read-only."""
        return self._media_type

    @property
    def name(self) -> str:
        """The track name (`Video 1`, `Audio 2`, ...). Read-only.

        Premiere synthesizes these; nothing is stored for unrenamed tracks.
        """
        return f"{self._media_type} {self._index + 1}"

    def _audio_mute_param(self) -> ET.Element | None:
        # An audio track's mute lives in the mix graph, not on the track:
        # AudioTrack/ComponentOwner/Components -> AudioComponentChain ->
        # AudioFader -> the `Mute` AudioComponentParam. Verified against
        # ExtendScript's own `audioTracks[i].setMute(1)`.
        document = self.sequence.project._document
        chain_ref = self._element.find("AudioTrack/ComponentOwner/Components")
        if chain_ref is None:
            return None
        chain = document.resolve(chain_ref)
        for component_ref in chain.findall("ComponentChain/Components/Component"):
            component = document.resolve(component_ref)
            for param_ref in component.findall("AudioComponent/Component/Params/Param"):
                param = document.resolve(param_ref)
                if param.findtext("Name") == "Mute":
                    return param
        return None

    @property
    def is_muted(self) -> bool:
        """Whether the track is muted. Read/write.

        A video track stores this as `ClipTrack/Track/IsMuted`; an audio
        track stores it on its mix-graph fader's `Mute` parameter instead
        (`IsMuted` on an audio track is ignored and dropped by Premiere).
        Both are elided when the track is audible, so an unmuted track
        reports `False`.
        """
        if self._media_type == "Audio":
            param = self._audio_mute_param()
            if param is None:
                return False
            start = param.findtext("StartKeyframe")
            if start:
                return start.split(",")[1:2] == ["true"]
            return param.findtext("CurrentValue") == "true"
        return self._element.findtext("ClipTrack/Track/IsMuted") == "true"

    @is_muted.setter
    def is_muted(self, value: bool) -> None:
        validate_bool(value)
        if self._media_type == "Audio":
            self._set_audio_mute(value)
            return
        set_elided_flag(self._track_element(), "IsMuted", value)

    def _track_element(self) -> ET.Element:
        track = self._element.find("ClipTrack/Track")
        if track is None:
            raise ValueError("track has no ClipTrack/Track element")
        return track

    def _set_audio_mute(self, value: bool) -> None:
        param = self._audio_mute_param()
        if param is None:
            raise ValueError("audio track has no Mute parameter to set")
        start = param.find("StartKeyframe")
        current = param.find("CurrentValue")
        if not value:
            # An audible track carries neither element, exactly as Premiere
            # writes it before the first mute.
            for element in (start, current):
                if element is not None:
                    remove_child(param, element)
            return
        if start is None:
            insert_leaf_before(param, "Name", "StartKeyframe", _MUTE_START_KEYFRAME)
        else:
            start.text = _MUTE_START_KEYFRAME
        if current is None:
            insert_leaf_before(param, "Name", "CurrentValue", "true")
        else:
            current.text = "true"

    @property
    def is_locked(self) -> bool:
        """Whether the track is locked for editing. Read/write.

        `ClipTrack/Track/IsLocked`, elided when the track is editable. Unlike
        mute, this home is the same for video and audio tracks (verified
        against a hand-locked fixture of each - lock has no scripting API in
        either ExtendScript or UXP).
        """
        return self._element.findtext("ClipTrack/Track/IsLocked") == "true"

    @is_locked.setter
    def is_locked(self, value: bool) -> None:
        validate_bool(value)
        set_elided_flag(self._track_element(), "IsLocked", value)

    @property
    def is_sync_locked(self) -> bool:
        """Whether the track moves with the others on an insert. Read/write.

        `ClipTrack/Track/IsSyncLocked`, and note the polarity: sync lock is
        ON by default, so Premiere elides the element and only writes it -
        as `false` - once the track has been un-synced. Like `is_locked` and
        unlike `is_muted`, the home is the same for video and audio.

        Neither ExtendScript nor UXP exposes this; the fixture was made
        through the QE DOM (`track.setSyncLock`).
        """
        return self._element.findtext("ClipTrack/Track/IsSyncLocked") != "false"

    @is_sync_locked.setter
    def is_sync_locked(self, value: bool) -> None:
        validate_bool(value)
        # Inverted: a synced track is the default, and Premiere stores
        # nothing for it.
        set_elided_flag(self._track_element(), "IsSyncLocked", not value, text="false")

    @property
    def clips(self) -> NamedList[TrackItem]:
        """The track items on this track, indexable by name. Read-only."""
        return NamedList(self._clips)

    def __iter__(self) -> Iterator[TrackItem]:
        return iter(self._clips)

    def __len__(self) -> int:
        return len(self._clips)

    def __getitem__(self, key: int | str) -> TrackItem:
        return self.clips[key]

    def __contains__(self, item: object) -> bool:
        return item in self.clips

    @property
    def transitions(self) -> NamedList[Transition]:
        """The transitions on this track, indexable by name. Read-only.

        Not in the ExtendScript DOM; mirrors UXP's
        `getTrackItems(TrackItemType.TRANSITION)` view.
        """
        return NamedList(self._transitions, keys=("name", "match_name"))

    def add_transition(
        self,
        clip: TrackItem,
        match_name: str,
        at_start: bool = True,
        duration: Time | None = None,
        name: str | None = None,
    ) -> Transition:
        """Put a transition on one end of a clip and return it.

        `at_start` chooses the end: the head of the clip (the default) or its
        tail. When a clip butts against that end, the transition lands ON THE
        CUT and covers both - which is what Premiere does, and it clamps each
        half to the footage available:

        * the part BEFORE the cut plays the outgoing clip, so it cannot be
          longer than that clip, nor than the handle the INCOMING clip has
          before its in point;
        * the part AFTER the cut is the mirror image.

        So a clip with no handle takes no transition on that side, and the
        result can be shorter than asked for - read the returned transition's
        `duration` to see what fitted. `Alignment` follows as the part before
        the cut.

        `duration` is the total, snapped down to whole frames, and defaults
        to Premiere's own: one second per side of a cut, or one second for a
        single-sided one. `name` defaults to the display name of a known
        transition (`match_name` has no derivable display name - `ADBE
        Additive Dissolve` displays as `Additive Dissolve (Legacy)`), so an
        unknown one has to be named.
        """
        validate_string()(match_name)
        validate_bool(at_start)
        if clip not in self._clips:
            raise ValueError("clip is not on this track")
        if name is None:
            name = TRANSITION_DISPLAY_NAMES.get(match_name)
            if name is None:
                raise ValueError(
                    f"no verified display name for {match_name!r}; pass name="
                )
        validate_string()(name)
        if duration is not None:
            validate_time(duration)
            if duration.ticks <= 0:
                raise ValueError("a transition needs a positive duration")
        neighbour = self._neighbour_of(clip, at_start)
        if neighbour is None:
            start, end, offset = self._single_sided_span(clip, at_start, duration)
        else:
            outgoing, incoming = (neighbour, clip) if at_start else (clip, neighbour)
            start, end, offset = self._cut_span(outgoing, incoming, duration)
        core = clip._element.find("ClipTrackItem")
        if core is None:
            raise ValueError("clip has no ClipTrackItem")
        tag = "HeadTransition" if at_start else "TailTransition"
        if core.find(tag) is not None:
            raise ValueError(f"clip already has a {tag}")
        other_core = None
        if neighbour is not None:
            other_core = neighbour._element.find("ClipTrackItem")
            other_tag = "TailTransition" if at_start else "HeadTransition"
            if other_core is not None and other_core.find(other_tag) is not None:
                raise ValueError(f"the adjacent clip already has a {other_tag}")

        document = self.sequence.project._document
        element = _new_transition_element(
            self._media_type,
            start,
            end,
            offset,
            neighbour is not None,
            at_start,
            name,
            match_name,
            clip._clip_element.findtext("AudioChannelLayout"),
        )
        object_id = document.add_object(element)
        self._add_track_item_ref(object_id, "TransitionItems")
        _attach_transition_ref(core, tag, object_id)
        if other_core is not None:
            _attach_transition_ref(
                other_core,
                "TailTransition" if at_start else "HeadTransition",
                object_id,
            )
        transition = Transition(element, self)
        self._transitions.append(transition)
        return transition

    def _neighbour_of(self, clip: TrackItem, at_start: bool) -> TrackItem | None:
        # The clip butting against the chosen end of `clip`, if any.
        boundary = clip.start.ticks if at_start else clip.end.ticks
        for candidate in self._clips:
            if candidate is clip:
                continue
            edge = candidate.end.ticks if at_start else candidate.start.ticks
            if edge == boundary:
                return candidate
        return None

    def _frames(self, ticks: int) -> int:
        # Transitions are stored on whole frames, so every span is snapped
        # DOWN to one - a one-second default is 29 frames at 29.97, which is
        # exactly what Premiere wrote for the single-sided fixtures.
        timebase = self.sequence.timebase
        if not timebase:
            raise ValueError("sequence has no timebase to align a transition to")
        return (ticks // timebase) * timebase

    def _single_sided_span(
        self, clip: TrackItem, at_start: bool, duration: Time | None
    ) -> tuple[int, int, int]:
        wanted = self._frames(TICKS_PER_SECOND if duration is None else duration.ticks)
        if wanted <= 0:
            raise ValueError("a transition needs at least one whole frame")
        if wanted > clip.end.ticks - clip.start.ticks:
            raise ValueError("transition is longer than the clip it sits on")
        if at_start:
            return clip.start.ticks, clip.start.ticks + wanted, 0
        return clip.end.ticks - wanted, clip.end.ticks, wanted

    def _cut_span(
        self, outgoing: TrackItem, incoming: TrackItem, duration: Time | None
    ) -> tuple[int, int, int]:
        # Measured against six fixtures Premiere itself wrote: each half is
        # capped by the clip it covers AND by the OTHER clip's handle, since
        # that clip has to keep playing across the cut.
        half = self._frames(
            TICKS_PER_SECOND if duration is None else duration.ticks // 2
        )
        if half <= 0:
            raise ValueError("a transition needs at least one whole frame a side")
        before = min(
            half,
            _handle_before(incoming),
            outgoing.end.ticks - outgoing.start.ticks,
        )
        after = min(
            half,
            _handle_after(outgoing),
            incoming.end.ticks - incoming.start.ticks,
        )
        before, after = self._frames(before), self._frames(after)
        if before + after <= 0:
            raise ValueError("neither clip has the footage to play across the cut")
        cut = outgoing.end.ticks
        return cut - before, cut + after, before

    def remove_transition(self, transition: Transition) -> None:
        """Remove a transition from this track.

        Drops the transition object and everything only it referenced, the
        track's reference to it, and the head/tail reference on each clip it
        sat between. Newer transitions own a `VideoFilterComponent` (with its
        parameters) that has to go with them - Premiere garbage-collects one
        left behind, so leaving it would show up as a diff against a resave.
        """
        if transition not in self._transitions:
            raise ValueError("transition is not on this track")
        document = self.sequence.project._document
        object_id = transition._element.get("ObjectID")
        self._detach_track_item_ref(object_id, "TransitionItems")
        for clip in self._clips:
            core = clip._element.find("ClipTrackItem")
            if core is None:
                continue
            for tag in ("HeadTransition", "TailTransition"):
                reference = core.find(tag)
                if reference is not None and reference.get("ObjectRef") == object_id:
                    remove_child(core, reference)
        for owned in document.owned_objects([transition._element]):
            document.remove_object(owned)
        self._transitions.remove(transition)

    def remove_clip(self, clip: TrackItem) -> None:
        """Remove a clip from this track.

        Detaches the track-item reference and deletes the track item, its
        component chain, subclip and clip objects. The source master clip
        and media stay in the project, exactly as Premiere does.
        """
        if clip not in self._clips:
            raise ValueError("clip is not on this track")
        document = self.sequence.project._document
        object_id = clip._element.get("ObjectID")
        self._detach_track_item_ref(object_id)
        _detach_from_links(document, object_id)
        objects = [clip._element, clip._subclip_element, clip._clip_element]
        chain_ref = clip._element.find("ClipTrackItem/ComponentOwner/Components")
        if chain_ref is not None:
            objects.append(document.resolve(chain_ref))
        # Each placement owns its SecondaryContent objects (the channel
        # mapping); only the Source they point at is shared, so they go with
        # the clip - otherwise every add/remove cycle leaks one per channel.
        secondaries = clip._clip_element.findall(
            "SecondaryContents/SecondaryContentItem"
        )
        if secondaries:
            index = ReferenceIndex(document)
            for item_ref in secondaries:
                content = document.resolve(item_ref)
                if not _referenced_elsewhere(index, content, objects):
                    objects.append(content)
        for element in objects:
            document.remove_object(element)
        self._clips.remove(clip)

    def move_clip(
        self, clip: TrackItem, target_track: Track, start: Time | None = None
    ) -> None:
        """Move a clip to another track of the same media type.

        The clip's objects are kept; only the track-item reference moves and
        (optionally) its timeline position changes.
        """
        if clip not in self._clips:
            raise ValueError("clip is not on this track")
        if target_track._media_type != self._media_type:
            raise ValueError("cannot move a clip to a different media type")
        if target_track is self and start is None:
            return
        object_id = clip._element.get("ObjectID")
        if object_id is None:
            raise ValueError("clip has no ObjectID")
        if start is not None:
            self._place_on_timeline(clip._element, start)
        self._detach_track_item_ref(object_id)
        target_track._add_track_item_ref(object_id)
        self._clips.remove(clip)
        target_track._clips.append(clip)
        clip.track = target_track
        target_track.sequence._grow_source_duration()

    def _clips_within(self, start: int, end: int) -> list[TrackItem]:
        # The clips an overwrite over this span would erase. A clip the
        # span only partly covers would have to be trimmed instead, which
        # is not supported.
        covered = []
        for clip in self._clips:
            if clip.end.ticks <= start or clip.start.ticks >= end:
                continue
            if clip.start.ticks < start or clip.end.ticks > end:
                raise ValueError(
                    f"placing here would partly cover {clip.name!r};"
                    " move it or free the span first"
                )
            covered.append(clip)
        return covered

    def _detach_track_item_ref(
        self, object_id: str | None, container: str = "ClipItems"
    ) -> None:
        track_items = self._element.find(f"ClipTrack/{container}/TrackItems")
        if track_items is None:
            return
        for reference in track_items.findall("TrackItem"):
            if reference.get("ObjectRef") == object_id:
                remove_child(track_items, reference)
                break
        if len(track_items) == 0:
            container_element = self._element.find(f"ClipTrack/{container}")
            if container_element is not None:
                remove_child(container_element, track_items)
        else:
            for index, reference in enumerate(track_items.findall("TrackItem")):
                reference.set("Index", str(index))

    def add_clip(
        self, project_item: ProjectItem, start: Time | None = None
    ) -> TrackItem:
        """Place a project item onto this track and return the new clip.

        Synthesizes the placement the way Premiere does: the timeline clip
        is built from the master clip's own template clip (fresh clip id, no
        `InUse`), the component chain and subclip are fresh defaults, and the
        in/out come from the item's panel marks - or the full media duration
        when unmarked. Placing an unmarked still is not supported: its
        default duration lives in a Premiere preference the file does not
        store.
        """
        if start is None:
            start = Time(0)
        validate_time(start)
        if project_item._sequence_uid is not None and (
            project_item._sequence_uid == self.sequence.sequence_id
        ):
            # Premiere's own UI refuses this edit; the file it would produce
            # hangs Premiere on open.
            raise ValueError("cannot place a sequence on its own timeline")
        document = self.sequence.project._document
        master = project_item._master_element
        if master is None:
            raise ValueError("item has no master clip to place")
        wanted = "VideoClip" if self._media_type == "Video" else "AudioClip"
        own = None
        for reference in master.findall("Clips/Clip"):
            candidate = document.resolve(reference)
            if candidate.tag == wanted:
                own = candidate
                break
        if own is None:
            raise ValueError(f"item has no {self._media_type.lower()} stream to place")
        core = own.find("Clip")
        if core is None:
            raise ValueError("master clip has no Clip core")

        in_ticks, out_ticks = self._resolve_in_out(document, core)

        # The timeline clip is the master's template clip with a fresh
        # ClipID, no InUse flag, and the resolved in/out - exactly what
        # Premiere writes for a fresh placement (verified field-for-field
        # against Premiere's own overwrite of the same item).
        clip = copy.deepcopy(own)
        clip_core = clip.find("Clip")
        if clip_core is None:
            raise ValueError("master clip has no Clip core")
        in_use = clip_core.find("InUse")
        if in_use is not None:
            remove_child(clip_core, in_use)
        if clip_core.find("OutPoint") is None:
            insert_leaf_before(clip_core, "ClipID", "OutPoint", str(out_ticks))
        if clip_core.find("InPoint") is None:
            insert_leaf_before(clip_core, "ClipID", "InPoint", str(in_ticks))
        clip_id_element = clip_core.find("ClipID")
        if clip_id_element is None:
            raise ValueError("master clip has no ClipID")
        clip_id_element.text = str(uuid.uuid4())
        if self._media_type == "Audio":
            self._attach_secondary_content(document, clip)
        clip_object_id = document.add_object(clip)

        name = master.findtext("Name") or ""
        subclip = _new_subclip_element(
            clip_object_id, master.get("ObjectUID") or "", name
        )
        subclip_object_id = document.add_object(subclip)

        chain = (
            _new_video_chain_element()
            if self._media_type == "Video"
            else _new_audio_chain_element()
        )
        chain_object_id = document.add_object(chain)

        if self._media_type == "Video":
            item = _new_video_track_item_element(
                chain_object_id,
                subclip_object_id,
                out_ticks - in_ticks,
                self._video_stream(document, core),
            )
        else:
            item = _new_audio_track_item_element(
                chain_object_id, subclip_object_id, out_ticks - in_ticks
            )
        self._place_on_timeline(item, start)
        item_object_id = document.add_object(item)

        self._add_track_item_ref(item_object_id)
        placed = TrackItem(item, subclip, clip, self)
        self._clips.append(placed)
        self.sequence._grow_source_duration()
        return placed

    def add_graphic(
        self,
        text: str,
        start: Time | None = None,
        duration: Time | None = None,
        name: str = "Graphic",
        position: tuple[float, float] = (0.5, 0.5),
    ) -> TrackItem:
        """Place a text graphic on this track and return the new clip.

        Synthesizes what Premiere's Type tool writes (66_eg_text): an
        infinite synthetic-media graph, a master OUTSIDE the project panel
        (Premiere keeps graphic masters there too), and a placement
        carrying the `AE.ADBE Text` component with its 22 default
        parameters. `text` becomes the styled-text payload - restyle it
        afterwards through the component's parameters, or re-text it
        through `Source Text`'s `text`.

        `name` is the clip's name, `Graphic` as Premiere's own is (only
        the component's instance name follows the text). `position` places
        the text in normalized frame coordinates, centred by default -
        Premiere stores wherever the Type tool was clicked. `duration`
        defaults to Premiere's still default (5 s), snapped down to whole
        frames.
        """
        _validate_graphic_text(text)
        _validate_graphic_name(name)
        validate_vector2(position)
        if start is None:
            start = Time(0)
        validate_time(start)
        if duration is not None:
            validate_time(duration)
            if duration.ticks <= 0:
                raise ValueError("a graphic needs a positive duration")
        if self._media_type != "Video":
            raise ValueError("graphics go on video tracks")
        timebase = self.sequence.timebase
        if not timebase:
            raise ValueError("sequence has no timebase to align a graphic to")
        width, height = self.sequence.frame_size or (0, 0)
        span = _GRAPHIC_DEFAULT_TICKS if duration is None else duration.ticks
        span -= span % timebase
        if span <= 0:
            raise ValueError("a graphic needs at least one whole frame")
        document = self.sequence.project._document

        stream = build_graphic_stream(timebase, width, height)
        stream_id = document.add_object(stream)
        media_uid = str(uuid.uuid4())
        media = build_graphic_media(media_uid, stream_id, name)
        media.set("ObjectUID", media_uid)
        document.attach_object(media)
        source_id = document.add_object(build_graphic_source(media_uid))
        template_id = document.add_object(
            build_graphic_clip(source_id, 0, span, in_use=True)
        )
        logging_id = document.add_object(
            build_graphic_logging(
                name, timebase, self.sequence.video_display_format or 0
            )
        )
        groups_id = document.add_object(build_empty_channel_groups())
        master_uid = str(uuid.uuid4())
        master = build_graphic_master(
            master_uid, logging_id, template_id, groups_id, name
        )
        master.set("ObjectUID", master_uid)
        document.attach_object(master)

        clip = build_graphic_clip(
            source_id,
            _GRAPHIC_BASE_TICKS,
            _GRAPHIC_BASE_TICKS + span,
            in_use=False,
        )
        clip_object_id = document.add_object(clip)
        subclip = _new_subclip_element(clip_object_id, master_uid, name)
        subclip_object_id = document.add_object(subclip)

        param_ids = [document.add_object(build_source_text_param(text))]
        param_ids += [document.add_object(p) for p in build_text_params(position)]
        component_id = document.add_object(build_text_component(param_ids, text))
        chain_object_id = document.add_object(build_graphic_chain(component_id))

        item = _new_video_track_item_element(
            chain_object_id, subclip_object_id, span, stream
        )
        self._place_on_timeline(item, start)
        item_object_id = document.add_object(item)
        self._add_track_item_ref(item_object_id)

        placed = TrackItem(item, subclip, clip, self)
        placed._components = [
            _build_component_model(document, component_id, param_ids, placed)
        ]
        self._clips.append(placed)
        self.sequence._grow_source_duration()
        return placed

    def add_mgt(self, path: str | Path, start: Time | None = None) -> TrackItem:
        """Import a Motion Graphics template onto this track.

        Synthesizes what `sequence.importMGT` writes: the template's
        graphic is copied next to the project under a
        `Motion Graphics Template Media` folder and imported from THERE
        (so the project keeps working without the `.mogrt`), the imported
        item is filed into the panel bin of the same name, and both the
        master and the placement carry an `AE.ADBE Capsule` component -
        the Essential Graphics controls, at the template's own defaults.

        The placement runs for the template's full duration, floored to
        the sequence's frame grid, and OVERWRITES what it lands on the way
        ExtendScript's import does - though only whole clips, since a
        partial cover would need a trim. Templates carrying audio are not
        supported.
        """
        _validate_template_path(path)
        if start is None:
            start = Time(0)
        validate_time(start)
        if self._media_type != "Video":
            raise ValueError("Motion Graphics templates go on video tracks")
        project = self.sequence.project
        template = Template(Path(path))
        if template.has_audio:
            raise ValueError("Motion Graphics templates with audio are not supported")
        span = template.duration_ticks
        timebase = self.sequence.timebase
        if timebase:
            span -= span % timebase
        covered = self._clips_within(start.ticks, start.ticks + span)
        document = project._document

        media_file = template.media_path(project.path.resolve().parent)
        media_file.parent.mkdir(parents=True, exist_ok=True)
        media_file.write_bytes(template.graphic)

        # Both copies of the control set share one store of payloads.
        stored: dict[bytes, str] = {}
        stream_id = document.add_object(build_mgt_stream(template))
        media_uid = str(uuid.uuid4())
        project._imported_media.append(
            document.attach_object(
                build_mgt_media(
                    template,
                    media_uid,
                    stream_id,
                    media_file,
                    stored,
                    project.path.resolve().parent,
                )
            )
        )
        source_id = document.add_object(build_mgt_source(template, media_uid))
        template_clip_id = document.add_object(build_mgt_clip(source_id, in_use=True))
        logging_id = document.add_object(build_mgt_logging(template))
        groups_id = document.add_object(build_empty_channel_groups())

        master_params = [
            document.add_object(param)
            for param in build_capsule_params(template, stored)
        ]
        master_component = document.add_object(
            build_capsule_component(template, master_params, stored, MASTER_SLOT)
        )
        chain_id = document.add_object(build_blueprint_chain(master_component))
        master_uid = str(uuid.uuid4())
        document.attach_object(
            build_mgt_master(
                template, master_uid, logging_id, chain_id, template_clip_id, groups_id
            )
        )
        item = project._add_template_item(master_uid, template.name)

        for clip in covered:
            self.remove_clip(clip)
        placed = self.add_clip(item, start)
        param_ids = [
            document.add_object(param)
            for param in build_capsule_params(template, stored)
        ]
        component_id = document.add_object(
            build_capsule_component(template, param_ids, stored, PLACEMENT_SLOT)
        )
        chain_reference = placed._element.find(
            "ClipTrackItem/ComponentOwner/Components"
        )
        if chain_reference is None:
            raise ValueError("placement has no component chain")
        attach_component(document.resolve(chain_reference), component_id)
        _tag_as_template(placed._element, template.capsule_id)
        # A template renders at the SEQUENCE's frame size, not the size it
        # was authored at, and the placement records the rendered raster.
        frame_rect = placed._element.find("FrameRect")
        size = self.sequence.frame_size
        if frame_rect is not None and size is not None:
            frame_rect.text = f"0,0,{size[0]},{size[1]}"
        placed._components = [
            _build_component_model(document, component_id, param_ids, placed)
        ]
        return placed

    def insert_clip(
        self, project_item: ProjectItem, start: Time | None = None
    ) -> TrackItem:
        """Insert a clip at `start`, rippling everything after it.

        Matches Premiere's insert edit: clips starting at or after the
        insert point shift later by the inserted duration on EVERY track of
        the sequence, and so do the sequence markers. An insert point that
        would split an existing clip is not supported.
        """
        if start is None:
            start = Time(0)
        validate_time(start)
        point = start.ticks
        to_shift = []
        for track in self.sequence.video_tracks + self.sequence.audio_tracks:
            for clip in track.clips:
                if clip.end.ticks <= point:
                    continue
                if clip.start.ticks < point:
                    raise NotImplementedError(
                        "the insert point splits an existing clip"
                    )
                to_shift.append(clip)
        placed = self.add_clip(project_item, start)
        delta = placed.duration.ticks
        for clip in to_shift:
            clip.end = Time(clip.end.ticks + delta)
            clip.start = Time(clip.start.ticks + delta)
        for marker in self.sequence.markers:
            if marker.start.ticks >= point:
                marker.start = Time(marker.start.ticks + delta)
        return placed

    def _resolve_in_out(
        self, document: PremiereDocument, core: ET.Element
    ) -> tuple[int, int]:
        # Panel marks when present; unmarked finite media plays in full,
        # trimmed down to whole sequence frames as Premiere places it;
        # an unmarked still has no derivable duration (a preference).
        out_text = core.findtext("OutPoint")
        if out_text is not None:
            return int(core.findtext("InPoint") or 0), int(out_text)
        source_ref = core.find("Source")
        if source_ref is None:
            raise ValueError("master clip has no Source")
        source = document.resolve(source_ref)
        media_ref = source.find("MediaSource/Media")
        media = document.resolve(media_ref) if media_ref is not None else None
        if media is not None and media.findtext("Infinite") == "true":
            raise NotImplementedError(
                "placing an unmarked still is not supported: its default"
                " duration is a Premiere preference the file does not store"
            )
        duration_text = source.findtext("OriginalDuration")
        if duration_text is None:
            raise ValueError("media source has no OriginalDuration")
        duration = int(duration_text)
        timebase = self.sequence.timebase
        if timebase:
            # Premiere's own placement of the 1s wav in the corpus stores
            # 29 whole frames at 29.97, not the full media duration.
            duration -= duration % timebase
        return 0, duration

    def _video_stream(
        self, document: PremiereDocument, core: ET.Element
    ) -> ET.Element | None:
        source_ref = core.find("Source")
        if source_ref is None:
            return None
        media_ref = document.resolve(source_ref).find("MediaSource/Media")
        if media_ref is None:
            return None
        stream_ref = document.resolve(media_ref).find("VideoStream")
        if stream_ref is None:
            return None
        return document.resolve(stream_ref)

    def _attach_secondary_content(
        self, document: PremiereDocument, clip: ET.Element
    ) -> None:
        # Each placed AudioClip gets its OWN SecondaryContent object (the
        # channel mapping); only the referenced Source is shared.
        for item_ref in clip.findall("SecondaryContents/SecondaryContentItem"):
            template = document.resolve(item_ref)
            content_ref = template.find("Content")
            if content_ref is None:
                raise ValueError("secondary content has no Content reference")
            content = _new_secondary_content_element(
                content_ref.get("ObjectRef") or "",
                template.findtext("ChannelIndex") or "0",
            )
            item_ref.set("ObjectRef", document.add_object(content))

    def _place_on_timeline(self, item: ET.Element, start: Time) -> None:
        # The cloned item keeps the source's Start (0) and End (its
        # duration); shift both to the requested start time.
        track_item = item.find("ClipTrackItem/TrackItem")
        if track_item is None:
            return
        end = int(track_item.findtext("End") or 0)
        source_start = int(track_item.findtext("Start") or 0)
        duration = end - source_start
        end_element = track_item.find("End")
        if end_element is not None:
            end_element.text = str(start.ticks + duration)
        start_element = track_item.find("Start")
        if start.ticks:
            if start_element is None:
                insert_leaf_before(track_item, "End", "Start", str(start.ticks))
            else:
                start_element.text = str(start.ticks)
        elif start_element is not None:
            remove_child(track_item, start_element)

    def _add_track_item_ref(self, object_id: str, container: str = "ClipItems") -> None:
        # `ClipItems` and `TransitionItems` have the same shape: an optional
        # `TrackItems` list, then MediaType and Index.
        clip_items = self._element.find(f"ClipTrack/{container}")
        if clip_items is None:
            raise ValueError(f"track has no {container} container")
        items = clip_items.find("TrackItems")
        entry = ET.Element("TrackItem", {"Index": "0", "ObjectRef": object_id})
        if items is None:
            # Create the list before MediaType; entry one indent deeper than
            # the container's close, entry tail back at the container indent.
            base = clip_items.text or "\n\t\t\t\t"
            items = ET.Element("TrackItems", {"Version": "1"})
            items.text = base + "\t"
            entry.tail = base
            items.append(entry)
            insert_before(clip_items, "MediaType", items)
            return
        entry.set("Index", str(len(items)))
        append_child(items, entry, empty_indent="\n\t\t\t\t\t")

    def __repr__(self) -> str:
        return (
            f"Track({self._media_type} index={self._index}, {len(self._clips)} clip(s))"
        )
