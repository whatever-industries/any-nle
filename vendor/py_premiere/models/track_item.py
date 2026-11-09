"""The `TrackItem` model."""

from __future__ import annotations

import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING, cast

from ..enums import BlendMode, TimeInterpolationType
from ..xml.mutations import insert_leaf_before, remove_child, set_elided_flag
from .component import (
    Component,
    ComponentParam,
    _stamp_next_component_number,
    _wrap_mask,
)
from .descriptors import XmlField
from .mask_builder import (
    attach_sub_mask,
    build_mask_component,
    build_mask_params,
    build_selection_chain,
)
from .media_import import _leaf, _top
from .merged_builder import _bag_node
from .named_list import NamedList
from .time import TICKS_PER_SECOND, Time, validate_time
from .validators import (
    validate_bool,
    validate_enum,
    validate_positive_number,
    validate_string,
)

if TYPE_CHECKING:
    from .project_item import ProjectItem
    from .track import Track

#: ClassIDs of the two objects a time-remap adds (25_time_remap).
_TIME_REMAP_CLASS_ID = "ace5148e-9c9b-40ed-9f82-64cb67308464"
_TIME_PARAM_CLASS_ID = "278ae1f9-ab7b-4dff-a53c-21029a399a9d"

#: The static-value sentinel keyframe every Speed param stores.
_REMAP_START_KEYFRAME = "-91445760000000000,0.000000000000000000000000,0,0,0,0,0,0"

#: The Opacity intrinsic's classes (81_blend_modes): the component shell is
#: the generic VideoFilterComponent, the percentage param and the two
#: Blend Mode twins each their own param class.
_OPACITY_COMPONENT_CLASS_ID = "d10da199-beea-4dd1-b941-ed3a78766d50"
_OPACITY_PARAM_CLASS_ID = "fe47129e-6c94-4fc0-95d5-c056a517aaf3"
_BLEND_PARAM_CLASS_ID = "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8"

#: The API twin's value per mode - the `ParameterID` 2 popup ExtendScript
#: and UXP drive (bounds 0..27; 27 itself is unobserved). The order is the
#: popup's own alphabetical one, Color=0 through Vivid Light=24, with
#: Subtract and Divide appended at 25/26 - so Lighten 11, Lighter Color 12,
#: Linear Burn 13, Linear Dodge 14, Linear Light 15 run in sequence.
#: Verified against `81_blend_modes` (which applies all 27 through the GUI)
#: and its ExtendScript export. Premiere's GUI writes BOTH twins, so py
#: does too. Normal's 18 comes from the 2026-07-21 UXP sweep (the value
#: never materializes in the fixture).
_API_BLEND_VALUES = {
    BlendMode.NORMAL: 18,
    BlendMode.DISSOLVE: 6,
    BlendMode.DARKEN: 3,
    BlendMode.MULTIPLY: 17,
    BlendMode.COLOR_BURN: 1,
    BlendMode.DARKER_COLOR: 4,
    BlendMode.LIGHTEN: 11,
    BlendMode.SCREEN: 22,
    BlendMode.COLOR_DODGE: 2,
    BlendMode.LIGHTER_COLOR: 12,
    BlendMode.LINEAR_BURN: 13,
    BlendMode.LINEAR_DODGE: 14,
    BlendMode.OVERLAY: 19,
    BlendMode.SOFT_LIGHT: 23,
    BlendMode.HARD_LIGHT: 8,
    BlendMode.VIVID_LIGHT: 24,
    BlendMode.LINEAR_LIGHT: 15,
    BlendMode.PIN_LIGHT: 20,
    BlendMode.HARD_MIX: 9,
    BlendMode.DIFFERENCE: 5,
    BlendMode.EXCLUSION: 7,
    BlendMode.SUBTRACT: 25,
    BlendMode.DIVIDE: 26,
    BlendMode.HUE: 10,
    BlendMode.SATURATION: 21,
    BlendMode.COLOR: 0,
    BlendMode.LUMINOSITY: 16,
}

_validate_blend_mode = validate_enum(BlendMode)


def _blend_keyframe(value: int) -> str:
    # Popup values are stored as BARE integers, unlike scalars' trailing
    # dot (81_blend_modes: `-91445760000000000,4,0,0,0,0,0,0`).
    return f"-91445760000000000,{value},0,0,0,0,0,0"


#: The bag Premiere writes when a clip is tagged Dialogue in the Essential
#: Sound panel (65_essential_sound): the `dialog` tag, the factory Dialogue
#: preset and its section defaults.
_DIALOGUE_ESSENTIAL_SOUND = [
    ("ESP.ClaritySection.2", "3"),
    ("ESP.SoundEffectsSection.3", "2"),
    ("ESP.RestorationSection.4", "7.14300060272"),
    ("ESP.RestorationSection.9", "60"),
    ("ESP.Tag", "dialog"),
    ("ESP.PresetGuid", "bb717b4a-c0b1-4def-9040-3fb8e83b8a91"),
    ("ESP.ClipMixTypeID", "1"),
    ("MZ.EssentialSound.AdjustmentModeWorkflowType", "1"),
    ("ESP.AdjustmentsModeID", "dialog"),
]


def _format_remap_value(value: float) -> str:
    # Remap values are source SECONDS at 24 fixed decimal places
    # (`1.701699999999999768363068` in 25_time_remap) - not the scalar
    # param format.
    return f"{value:.24f}"


def _format_remap_bound(value: float) -> str:
    # Premiere writes an integral bound as a bare integer (`43200`, seen on
    # its resave of a py-written curve) and a fractional one at full double
    # precision (`5.939266666666667` in 25_time_remap).
    return str(int(value)) if value == int(value) else repr(value)


def _time_from_text(text: str) -> Time:
    return Time(int(text))


def _time_to_text(value: Time) -> str:
    return str(value.ticks)


def _carry_sequence_length(item: TrackItem) -> None:
    # Moving or trimming a clip past where the sequence used to end makes
    # the sequence longer, and the length it reports as a source has to
    # follow.
    item.track.sequence._grow_source_duration()


class TrackItem:
    """A clip instance on a sequence track.

    `start`/`end` are positions on the sequence timeline; `in_point`/
    `out_point` are positions within the source media. The name is stored
    on the linked `SubClip`; the in/out points on the linked timeline clip.
    """

    #: The name of the track item.
    name = XmlField[str](
        "Name", element_attr="_subclip_element", validate=validate_string()
    )
    #: The visible start time in the sequence.
    #: Premiere elides a zero start; first write creates the element.
    start = XmlField[Time](
        "ClipTrackItem/TrackItem/Start",
        transform=_time_from_text,
        reverse=_time_to_text,
        validate=validate_time,
        default="0",
        insert_before="End",
        after_write=_carry_sequence_length,
    )
    #: The visible end time in the sequence.
    end = XmlField[Time](
        "ClipTrackItem/TrackItem/End",
        transform=_time_from_text,
        reverse=_time_to_text,
        validate=validate_time,
        default="0",
        after_write=_carry_sequence_length,
    )

    def __init__(
        self,
        _element: ET.Element,
        _subclip_element: ET.Element,
        _clip_element: ET.Element,
        track: Track,
    ) -> None:
        self._element = _element
        self._subclip_element = _subclip_element
        self._clip_element = _clip_element
        self.track = track
        self._components: list[Component] = []
        self._selection_components: list[Component] = []

    @property
    def components(self) -> NamedList[Component]:
        """The materialized components (effects) of this track item,
        indexable by display or match name. Read-only.

        Premiere synthesizes untouched intrinsics (Motion, Opacity, ...) at
        runtime; only modified components are stored and listed here.
        """
        return NamedList(self._components, keys=("display_name", "match_name"))

    @property
    def selection_components(self) -> NamedList[Component]:
        """Masks applied to the clip as a whole. Read-only.

        A mask drawn on the clip rather than on one effect lives in its own
        `SelectionComponents` chain; a mask attached TO an effect appears as
        that effect's `sub_components` instead.
        """
        return NamedList(
            self._selection_components, keys=("display_name", "match_name")
        )

    @property
    def time_remapping(self) -> ComponentParam | None:
        """The clip's time-remap speed curve, or `None` if it is not retimed.

        A retimed clip gains a `TimeRemapping` object holding one Speed
        parameter, whose keyframes map timeline time to SOURCE time in
        seconds (so the values rise across the clip rather than expressing a
        percentage). ExtendScript exposes no Time Remapping component at all,
        so this has no DOM counterpart - it is read straight from the XML,
        like `Track.transitions`.
        """
        core = (
            self._clip_element.find("Clip") if self._clip_element is not None else None
        )
        if core is None:
            return None
        reference = core.find("TimeRemapping")
        if reference is None:
            return None
        document = self.track.sequence.project._document
        keyframes = document.resolve(reference).find("Keyframes")
        if keyframes is None:
            return None
        return ComponentParam(document.resolve(keyframes), document, self)

    def add_mask(self) -> Component:
        """Apply a default mask to the clip as a whole and return it.

        A clip-level mask lives in its own chain, referenced from the track
        item as `SelectionComponents` (26_effect_mask); the synthesized
        component carries the 27 default parameters. Further masks hang OFF
        that first intrinsic holder as effect-role sub-components numbered
        `01`, `02`, ..., bumping its `NextComponentNumber` counter
        (76_two_masks) - so they appear under
        `selection_components[0].sub_components`. Adjust the geometry
        through the returned component's parameters; use `path` on the
        Path parameter for drawn shapes.
        """
        document = self.track.sequence.project._document
        chain_ref = self._element.find("SelectionComponents")
        if chain_ref is not None:
            holder = None
            holder_ref = document.resolve(chain_ref).find(
                "ComponentChain/Components/Component"
            )
            if holder_ref is not None:
                holder = document.resolve(holder_ref)
            if holder is None:
                raise ValueError("clip mask chain has no holder component")
            subs = holder.find("SubComponents")
            count = 0 if subs is None else len(subs.findall("SubComponent"))
            param_ids = build_mask_params(document)
            mask_id, mask_element = build_mask_component(
                document, param_ids, clip_role=False, instance_name=f"{count + 1:02d}"
            )
            attach_sub_mask(holder, mask_id)
            # The intrinsic holder's counter covers itself plus its subs:
            # 2 alone, 3 after the second mask (76_two_masks).
            _stamp_next_component_number(holder.find("Component"), count + 3)
            mask = _wrap_mask(document, mask_element, self, param_ids)
            if self._selection_components:
                self._selection_components[0]._sub_components.append(mask)
            return mask
        anchor = self._element.find("ClipTrackItem")
        if anchor is None:
            raise ValueError("track item has no clip core to anchor the mask")
        param_ids = build_mask_params(document)
        mask_id, mask_element = build_mask_component(
            document, param_ids, clip_role=True
        )
        chain_id = build_selection_chain(document, mask_id)
        reference = ET.Element("SelectionComponents", {"ObjectRef": chain_id})
        # The ref slots right after the ClipTrackItem base (26_effect_mask:
        # ClipTrackItem, SelectionComponents, PixelAspectRatio, ...).
        reference.tail = anchor.tail
        self._element.insert(list(self._element).index(anchor) + 1, reference)
        mask = _wrap_mask(document, mask_element, self, param_ids)
        self._selection_components.append(mask)
        return mask

    def _blend_params(self) -> dict[str, ComponentParam]:
        # The materialized Opacity intrinsic's Blend Mode twins, keyed by
        # ParameterID ("2" = the API popup, "3" = the GUI one).
        params: dict[str, ComponentParam] = {}
        for component in self._components:
            inner = component._element.find("Component")
            if inner is None or inner.findtext("DisplayName") != "Opacity":
                continue
            for param in component.properties:
                if param.display_name == "Blend Mode":
                    pid = param._element.findtext("ParameterID") or ""
                    params[pid] = param
        return params

    @property
    def blend_mode(self) -> BlendMode:
        """The clip's compositing blend mode. Read/write.

        An untouched clip stores nothing (the Opacity intrinsic is
        synthesized at runtime) and reads `NORMAL`. Setting a mode
        materializes the intrinsic exactly as Premiere's GUI does
        (81_blend_modes): the chain drops its `DefaultOpacity` pair and
        gains the `AE.ADBE Opacity` component, whose BOTH Blend Mode
        twins carry the mode - the GUI-domain value this enum uses and
        the API twin's own numbering.
        """
        param = self._blend_params().get("3")
        if param is None:
            return BlendMode.NORMAL
        return BlendMode(int(cast("float", param.value)))

    @blend_mode.setter
    def blend_mode(self, value: BlendMode) -> None:
        _validate_blend_mode(value)
        mode = BlendMode(value)
        params = self._blend_params()
        if not params:
            if mode is BlendMode.NORMAL:
                return
            self._materialize_opacity(mode)
            return
        for pid, stored in (("2", _API_BLEND_VALUES[mode]), ("3", int(mode))):
            param = params.get(pid)
            if param is None:
                continue
            keyframe = param._element.find("StartKeyframe")
            if keyframe is not None:
                keyframe.text = _blend_keyframe(stored)

    def _materialize_opacity(self, mode: BlendMode) -> None:
        # The GUI's own materialization (81_blend_modes): the chain loses
        # DefaultOpacity/DefaultOpacityComponentID, the component holds the
        # percentage param (static 100 with its CurrentValue mirror) and
        # both Blend Mode twins.
        document = self.track.sequence.project._document
        chain_ref = self._element.find("ClipTrackItem/ComponentOwner/Components")
        if chain_ref is None:
            raise ValueError("track item has no component chain")
        chain = document.resolve(chain_ref)
        inner = chain.find("ComponentChain")
        if inner is None:
            raise ValueError("component chain has no ComponentChain core")
        for tag in ("DefaultOpacity", "DefaultOpacityComponentID"):
            flag = chain.find(tag)
            if flag is not None:
                remove_child(chain, flag)

        opacity = _top("VideoComponentParam", _OPACITY_PARAM_CLASS_ID, "10")
        _leaf(opacity, "Name", "Opacity", "\n\t\t")
        _leaf(opacity, "ParameterID", "1", "\n\t\t")
        _leaf(opacity, "StartKeyframe", "-91445760000000000,100.,0,0,0,0,0,0", "\n\t\t")
        _leaf(opacity, "CurrentValue", "100", "\n\t\t")
        _leaf(opacity, "LowerBound", "0", "\n\t\t")
        _leaf(opacity, "UpperBound", "100", "\n\t")
        api = _top("VideoComponentParam", _BLEND_PARAM_CLASS_ID, "10")
        _leaf(api, "Name", "Blend Mode", "\n\t\t")
        _leaf(api, "ParameterControlType", "10", "\n\t\t")
        _leaf(api, "DiscontinuousInterpolate", "true", "\n\t\t")
        _leaf(api, "ParameterID", "2", "\n\t\t")
        _leaf(api, "StartKeyframe", _blend_keyframe(_API_BLEND_VALUES[mode]), "\n\t\t")
        _leaf(api, "LowerBound", "0", "\n\t\t")
        _leaf(api, "UpperBound", "27", "\n\t")
        gui = _top("VideoComponentParam", _BLEND_PARAM_CLASS_ID, "10")
        _leaf(gui, "Name", "Blend Mode", "\n\t\t")
        _leaf(gui, "DiscontinuousInterpolate", "true", "\n\t\t")
        _leaf(gui, "ParameterID", "3", "\n\t\t")
        _leaf(gui, "StartKeyframe", _blend_keyframe(int(mode)), "\n\t\t")
        _leaf(gui, "LowerBound", "0", "\n\t\t")
        _leaf(gui, "UpperBound", "31", "\n\t")
        param_ids = [document.add_object(p) for p in (opacity, api, gui)]

        element = _top("VideoFilterComponent", _OPACITY_COMPONENT_CLASS_ID, "9")
        core = ET.SubElement(element, "Component", {"Version": "7"})
        core.text = "\n\t\t\t"
        core.tail = "\n\t\t"
        holder = ET.SubElement(core, "Params", {"Version": "1"})
        holder.text = "\n\t\t\t\t"
        holder.tail = "\n\t\t\t"
        for index, param_id in enumerate(param_ids):
            entry = ET.SubElement(
                holder, "Param", {"Index": str(index), "ObjectRef": param_id}
            )
            entry.tail = "\n\t\t\t\t" if index < len(param_ids) - 1 else "\n\t\t\t"
        _leaf(core, "ID", "2", "\n\t\t\t")
        _leaf(core, "DisplayName", "Opacity", "\n\t\t\t")
        _leaf(core, "Intrinsic", "true", "\n\t\t")
        _leaf(element, "MatchName", "AE.ADBE Opacity", "\n\t\t")
        _leaf(element, "VideoFilterType", "2", "\n\t")
        component_id = document.add_object(element)

        components = inner.find("Components")
        if components is None:
            components = ET.Element("Components", {"Version": "1"})
            components.text = "\n\t\t\t\t"
            components.tail = "\n\t\t"
            if len(inner):
                list(inner)[-1].tail = "\n\t\t\t"
            else:
                inner.text = "\n\t\t\t"
            inner.append(components)
        entries = components.findall("Component")
        if entries:
            entries[-1].tail = "\n\t\t\t\t"
        entry = ET.SubElement(
            components,
            "Component",
            {"Index": str(len(entries)), "ObjectRef": component_id},
        )
        entry.tail = "\n\t\t\t"

        component = Component(element, self)
        for param_id in param_ids:
            component._properties.append(
                ComponentParam(document.by_object_id[param_id], document, component)
            )
        self._components.append(component)

    @property
    def essential_sound_tag(self) -> str | None:
        """The Essential Sound audio-type tag, or `None` when untagged.

        Stored as `ESP.Tag` in the track item's property bag
        (65_essential_sound: `dialog`); an untagged clip carries no bag.
        Neither ExtendScript nor UXP exposes the panel, so this is an
        XML-only surface.
        """
        return self._element.findtext("ClipTrackItem/TrackItem/Node/Properties/ESP.Tag")

    def tag_as_dialogue(self) -> None:
        """Tag this clip as Dialogue, as the Essential Sound panel does.

        Writes the nine-key bag Premiere's own tagging writes
        (65_essential_sound): the `dialog` tag, the factory Dialogue preset
        and its section defaults. Only the Dialogue shape is verified -
        the other audio types' bags are unknown, so there is no generic
        tag setter yet.
        """
        inner = self._element.find("ClipTrackItem/TrackItem")
        if inner is None:
            raise ValueError("track item has no TrackItem core")
        if inner.find("Node") is not None:
            raise ValueError("track item already carries a property bag")
        inner.insert(0, _bag_node(4, _DIALOGUE_ESSENTIAL_SOUND))

    def set_time_remap(self, keys: list[tuple[Time, float]]) -> ComponentParam:
        """Set (or create) the clip's time-remap curve and return its param.

        `keys` maps timeline time (clip-relative, so the first key usually
        sits at `Time(0)`) to SOURCE seconds - the stored encoding, rising
        across the clip rather than expressing a speed percentage. Creating
        the curve synthesizes the `TimeRemapping` object and its Speed
        `TimeComponentParam` exactly as 25_time_remap stores them (interp
        constant 6, zero tangents, `UpperBound` = the source duration in
        seconds). ExtendScript exposes no Time Remapping component, so this
        is an XML-only surface like `Track.transitions`.
        """
        if not isinstance(keys, list) or len(keys) < 2:
            raise ValueError("a time-remap curve needs at least two keyframes")
        for time, value in keys:
            validate_time(time)
            validate_positive_number(value)
            if time.ticks < 0:
                raise ValueError("keyframe times are clip-relative, not negative")
        ordered = sorted(keys, key=lambda key: key[0].ticks)
        for previous, following in zip(ordered, ordered[1:]):
            if previous[0].ticks == following[0].ticks:
                raise ValueError(f"duplicate keyframe time {previous[0].ticks}")
        document = self.track.sequence.project._document
        core = self._clip_element.find("Clip")
        if core is None:
            raise ValueError("track item has no clip core")
        source_ref = core.find("Source")
        if source_ref is None:
            raise ValueError("track item has no media source")
        duration_text = document.resolve(source_ref).findtext("OriginalDuration")
        if not duration_text:
            raise ValueError("media source has no duration to bound the curve")
        upper_bound = int(duration_text) / TICKS_PER_SECOND
        last_time, last_value = ordered[-1]
        if last_value > upper_bound:
            raise ValueError("keyframe values cannot exceed the source duration")
        if last_value < upper_bound:
            # Premiere completes an unfinished curve with a terminal key at
            # the source end, continuing at unity speed from the last key
            # (measured off its resave of a py-written curve: 1 s -> 2.0 s
            # gained a key at 43199 s -> 43200 s).
            terminal = last_time + Time(
                round((upper_bound - last_value) * TICKS_PER_SECOND)
            )
            ordered.append((terminal, upper_bound))
        entries = (
            ";".join(
                f"{time.ticks},{_format_remap_value(value)},6,0,0,0,0,0"
                for time, value in ordered
            )
            + ";"
        )
        existing = self.time_remapping
        if existing is not None:
            keyframes = existing._element.find("Keyframes")
            if keyframes is None:
                raise ValueError("existing Speed parameter has no keyframe list")
            keyframes.text = entries
            return existing
        param = _top("TimeComponentParam", _TIME_PARAM_CLASS_ID, "9")
        _leaf(param, "Name", "Speed", "\n\t\t")
        _leaf(param, "IsTimeVarying", "true", "\n\t\t")
        _leaf(param, "IsLocked", "false", "\n\t\t")
        _leaf(param, "DiscontinuousInterpolate", "false", "\n\t\t")
        _leaf(param, "LowerBound", "0", "\n\t\t")
        _leaf(param, "UpperBound", _format_remap_bound(upper_bound), "\n\t\t")
        _leaf(param, "StartKeyframe", _REMAP_START_KEYFRAME, "\n\t\t")
        _leaf(param, "Keyframes", entries, "\n\t\t")
        _leaf(param, "CurrentValue", "0", "\n\t\t")
        _leaf(param, "ParameterID", "-1", "\n\t")
        param_id = document.add_object(param)
        remap = _top("TimeRemapping", _TIME_REMAP_CLASS_ID, "2")
        ET.SubElement(remap, "Keyframes", {"ObjectRef": param_id}).tail = "\n\t"
        remap_id = document.add_object(remap)
        reference = ET.Element("TimeRemapping", {"ObjectRef": remap_id})
        # The ref slots right after `Source` in the clip core (25_time_remap:
        # Node, MarkerOwner, Source, TimeRemapping, OutPoint, InPoint, ...).
        reference.tail = source_ref.tail
        core.insert(list(core).index(source_ref) + 1, reference)
        return ComponentParam(param, document, self)

    def clear_time_remap(self) -> None:
        """Remove the clip's time-remap curve, if it carries one."""
        core = self._clip_element.find("Clip")
        reference = None if core is None else core.find("TimeRemapping")
        if core is None or reference is None:
            return
        document = self.track.sequence.project._document
        remap = document.resolve(reference)
        param_ref = remap.find("Keyframes")
        if param_ref is not None:
            document.remove_object(document.resolve(param_ref))
        document.remove_object(remap)
        remove_child(core, reference)

    def _speed(self) -> float:
        # Speed-adjusted clips store timeline-domain source ticks; the
        # ExtendScript values are source-domain (raw / PlaybackSpeed).
        text = self._clip_element.findtext("Clip/PlaybackSpeed")
        return float(text) if text else 1.0

    def _source_ticks(self, tag: str) -> int:
        raw = int(self._clip_element.findtext(f"Clip/{tag}") or 0)
        # Truncation, not rounding: validated against ExtendScript ticks.
        return int(raw / self._speed())

    def _write_source_ticks(
        self, tag: str, ticks: int, anchors: tuple[str, ...]
    ) -> None:
        clip = self._clip_element.find("Clip")
        if clip is None:
            raise ValueError("missing <Clip> element")
        speed = self._speed()
        raw_ticks = round(ticks * speed)
        # Pick the raw value that reads back (truncated) as `ticks` exactly.
        while int(raw_ticks / speed) < ticks:
            raw_ticks += 1
        while raw_ticks > 0 and int((raw_ticks - 1) / speed) >= ticks:
            raw_ticks -= 1
        raw = str(raw_ticks)
        element = clip.find(tag)
        if element is not None:
            element.text = raw
            return
        for anchor in anchors:
            if clip.find(anchor) is not None:
                insert_leaf_before(clip, anchor, tag, raw)
                return
        raise ValueError(f"cannot create <{tag}>: no known anchor present")

    @property
    def in_point(self) -> Time:
        """The in point on the source. Read/write."""
        return Time(self._source_ticks("InPoint"))

    @in_point.setter
    def in_point(self, value: Time) -> None:
        validate_time(value)
        self._write_source_ticks("InPoint", value.ticks, ("ClipID",))

    @property
    def out_point(self) -> Time:
        """The out point on the source. Read/write."""
        return Time(self._source_ticks("OutPoint"))

    @out_point.setter
    def out_point(self, value: Time) -> None:
        validate_time(value)
        self._write_source_ticks("OutPoint", value.ticks, ("InPoint", "ClipID"))

    @property
    def duration(self) -> Time:
        """The timeline duration (`end` minus `start`). Read-only."""
        return self.end - self.start

    @property
    def speed(self) -> float:
        """The playback speed (`1.0` = normal). Read-only.

        Premiere elides the field at normal speed, so an unmodified clip
        reports `1.0`.
        """
        return self._speed()

    @property
    def is_speed_reversed(self) -> bool:
        """Whether the clip plays backwards. Read-only."""
        return self._clip_element.findtext("Clip/PlayBackwards") == "true"

    @property
    def time_interpolation_type(self) -> TimeInterpolationType:
        """How a retimed clip fills in frames. Read-only.

        Stored on the clip OBJECT (`VideoClip/TimeInterpolationType`), beside
        the `Clip` core rather than inside it where the speed lives, and
        elided at frame sampling. Neither ExtendScript nor UXP exposes this;
        the fixtures were written through the QE DOM
        (`setTimeInterpolationType`, and the legacy `setFrameBlend`, which
        stores frame blending).
        """
        stored = self._clip_element.findtext("TimeInterpolationType")
        return TimeInterpolationType(int(stored) if stored else 0)

    @property
    def project_item(self) -> ProjectItem | None:
        """The source project item this clip instances. Read-only.

        Resolves the linked `SubClip`'s master-clip UID to the owning
        project item, so a renamed source is reported by its panel name
        (which can differ from this track item's name).
        """
        master_ref = self._subclip_element.find("MasterClip")
        if master_ref is None:
            return None
        uid = master_ref.get("ObjectURef")
        if uid is None:
            return None
        return self.track.sequence.project._item_by_master_uid(uid)

    @property
    def is_disabled(self) -> bool:
        """Whether the clip is disabled (not rendered). Read/write.

        Stored as `ClipTrackItem/IsMuted` (distinct from a track's own mute
        flag); elided when the clip is enabled. Setting `True` appends the
        flag (as Premiere does), `False` removes it.
        """
        return self._element.findtext("ClipTrackItem/IsMuted") == "true"

    @is_disabled.setter
    def is_disabled(self, value: bool) -> None:
        validate_bool(value)
        clip_track_item = self._element.find("ClipTrackItem")
        if clip_track_item is None:
            raise ValueError("track item has no ClipTrackItem")
        set_elided_flag(clip_track_item, "IsMuted", value)

    @property
    def media_type(self) -> str:
        """`"Video"` or `"Audio"`. Read-only."""
        return self.track.media_type

    @property
    def type(self) -> int:
        """The track item kind: 1 for clips (the only kind parsed). Read-only.

        The Scripting Guide describes this as video-vs-audio, but Premiere
        reports 1 for audio clips too; validated against ground truth.
        """
        return 1

    def __repr__(self) -> str:
        return (
            f"TrackItem(name={self.name!r}, "
            f"start={self.start.seconds:.3f}s, end={self.end.seconds:.3f}s)"
        )
