"""The `ProjectItem` model."""

from __future__ import annotations

import struct
import uuid
import xml.etree.ElementTree as ET
from fractions import Fraction
from pathlib import Path
from typing import TYPE_CHECKING

from ..enums import AlphaUsage, GeneratorType, ProjectItemType, VideoFieldType
from ..xml.document import ReferenceIndex
from ..xml.mutations import (
    append_child,
    append_leaf,
    insert_before,
    insert_leaf_before,
    remove_child,
    set_elided_flag,
)
from .marker import Marker, _attach_marker, _detach_marker
from .named_list import NamedList
from .time import UNSET_TICKS, Time, validate_time
from .validators import (
    validate_bool,
    validate_color_label,
    validate_enum,
    validate_number,
    validate_path,
    validate_string,
    validate_vector2,
)

_validate_alpha_usage = validate_enum(AlphaUsage)
_validate_field_type = validate_enum(VideoFieldType)
_validate_proxy_path = validate_path(must_exist=True, must_be_file=True)


def _leaf_element(tag: str, text: str) -> ET.Element:
    element = ET.Element(tag)
    element.text = text
    return element


#: The marker-collection object, and the all-zero state a fresh one carries.
_MARKERS_CLASS_ID = "bee50706-b524-416c-9f03-b596ce5f6866"
_ZERO_GUID = "00000000-0000-0000-0000-000000000000"


def _check_proxy_aspect(one: str, other: str) -> None:
    """Premiere refuses a proxy pair whose frame aspects differ."""
    first, second = one.split(","), other.split(",")
    width, height = int(first[2]), int(first[3])
    other_width, other_height = int(second[2]), int(second[3])
    if width * other_height != other_width * height:
        raise ValueError(
            "proxy frame aspect ratio must match the source "
            f"({width}x{height} vs {other_width}x{other_height})"
        )


def _override_frame_rect(stream: ET.Element, rect: str) -> None:
    # The override pair slots after OriginalColorSpace (before AlphaType
    # where the codec profile writes one), as an import does.
    children = [child.tag for child in stream]
    if "IsFrameRectOverridden" in children:
        return
    anchor = children.index("OriginalColorSpace")
    if anchor + 1 < len(children):
        following = children[anchor + 1]
        insert_leaf_before(stream, following, "IsFrameRectOverridden", "true")
        insert_leaf_before(stream, following, "OverriddenFrameRect", rect)
    else:
        append_leaf(stream, "IsFrameRectOverridden", "true")
        append_leaf(stream, "OverriddenFrameRect", rect)


def _ensure_clip_marker_list(
    document: PremiereDocument, core: ET.Element
) -> ET.Element:
    """The inner marker list of a clip, synthesizing the owner if absent.

    A clip that has never held a marker carries no `MarkerOwner` at all.
    Premiere adds one between the property bag and `Source` (verified
    against its own first clip marker), pointing at a fresh `Markers`
    collection.
    """
    reference = core.find("MarkerOwner/Markers")
    if reference is not None:
        collection = document.resolve(reference)
    else:
        collection = ET.Element(
            "Markers", {"ClassID": _MARKERS_CLASS_ID, "Version": "4"}
        )
        collection.text = "\n\t\t"
        ET.SubElement(collection, "Markers", {"Version": "1"}).tail = "\n\t\t"
        for tag, value, tail in (
            ("ByGUID", "byGUID", "\n\t\t"),
            ("LastMetadataState", _ZERO_GUID, "\n\t\t"),
            ("LastContentState", _ZERO_GUID, "\n\t"),
        ):
            child = ET.SubElement(collection, tag)
            child.text = value
            child.tail = tail
        collection_id = document.add_object(collection)
        owner = ET.Element("MarkerOwner", {"Version": "1"})
        owner.text = "\n\t\t\t\t"
        ET.SubElement(owner, "Markers", {"ObjectRef": collection_id}).tail = "\n\t\t\t"
        insert_before(core, "Source", owner)
    marker_list = collection.find("Markers")
    if marker_list is None:
        raise ValueError("marker collection has no inner list")
    return marker_list


if TYPE_CHECKING:
    from typing import Iterator

    from ..xml import PremiereDocument
    from .project import Project

_validate_name = validate_string()


def project_item_core(element: ET.Element) -> ET.Element | None:
    """The `ProjectItem` core of a panel item, whatever class nests it.

    Most kinds hold it directly, but a `SmartBinProjectItem` wraps a
    `BinProjectItem`, and that is what holds the core - so a smart bin reads
    as an unnamed clip without this.
    """
    core = element.find("ProjectItem")
    return core if core is not None else element.find("*/ProjectItem")


def item_container(element: ET.Element) -> ET.Element | None:
    """The `ProjectItemContainer` of a panel item, through the same nesting.

    Reads only: the write paths keep the strict lookup, so adding a child to
    a smart bin raises instead of writing into a container whose contents
    Premiere regenerates from its query.
    """
    container = element.find("ProjectItemContainer")
    if container is not None:
        return container
    return element.find("*/ProjectItemContainer")


def clip_core(element: ET.Element) -> ET.Element | None:
    """The `Clip` core of a clip object, whatever its class nests it under.

    Most clip classes hold it directly (`VideoClip`, `AudioClip`), but some
    go through an intermediate class first - an imported caption stream's
    `TranscriptClip` wraps a `DataClip`, which is what holds the core.
    """
    core = element.find("Clip")
    return core if core is not None else element.find("*/Clip")


#: ClassID of a top-level BinProjectItem object.
_BIN_CLASS_ID = "dbfd6653-24da-480e-a35e-ba45e9504e4b"


def _new_bin_element(uid: str, name: str) -> ET.Element:
    # A new empty bin with Premiere's own defaults: an empty property bag
    # (the view-state Premiere writes is elided - it synthesizes it on
    # open) and an empty item container. Whitespace matches the top-level
    # object layout byte-for-byte.
    element = ET.Element(
        "BinProjectItem",
        {"ObjectUID": uid, "ClassID": _BIN_CLASS_ID, "Version": "1"},
    )
    element.text = "\n\t\t"
    project_item = ET.SubElement(element, "ProjectItem", {"Version": "1"})
    project_item.text = "\n\t\t\t"
    project_item.tail = "\n\t\t"
    node = ET.SubElement(project_item, "Node", {"Version": "1"})
    node.text = "\n\t\t\t\t"
    node.tail = "\n\t\t\t"
    properties = ET.SubElement(node, "Properties", {"Version": "1"})
    properties.text = "\n\t\t\t\t"
    properties.tail = "\n\t\t\t"
    name_element = ET.SubElement(project_item, "Name")
    name_element.text = name
    name_element.tail = "\n\t\t"
    container = ET.SubElement(element, "ProjectItemContainer", {"Version": "1"})
    container.text = "\n\t\t"
    container.tail = "\n\t"
    return element


def _child_items(container: ET.Element) -> ET.Element:
    # The `Items` list of a `ProjectItemContainer`, synthesized (empty) when
    # absent. Indentation is derived from the container's own closing
    # whitespace, so it is correct at any nesting depth.
    items = container.find("Items")
    if items is not None:
        return items
    base = container.text or "\n\t\t"
    items = ET.Element("Items", {"Version": "1"})
    items.text = base + "\t"
    items.tail = base
    container.text = base + "\t"
    container.append(items)
    return items


def _add_item_ref(items: ET.Element, uid: str) -> None:
    reference = ET.Element("Item", {"Index": str(len(items)), "ObjectURef": uid})
    append_child(items, reference, empty_indent="\n\t\t\t")


def _detach_item_ref(container: ET.Element, uid: str) -> None:
    # `container` is the ProjectItemContainer; Premiere drops the whole
    # `Items` element when the last child leaves.
    items = container.find("Items")
    if items is None:
        return
    for reference in items.findall("Item"):
        if reference.get("ObjectURef") == uid:
            remove_child(items, reference)
            break
    if len(items) == 0:
        remove_child(container, items)
        return
    for index, reference in enumerate(items.findall("Item")):
        reference.set("Index", str(index))


#: Stored pixel aspect ratios are small exact ratios (`40,33` for HD
#: anamorphic, `4,3`, `16,15`), so a float is converted within this bound.
_PAR_MAX_DENOMINATOR = 1000

#: The `VideoStream` child order Premiere writes (schema-fixed), so a new
#: override element lands at its canonical position. Reconstructed by
#: merging every single-field footage-interpretation reference.
_VIDEO_STREAM_ORDER = (
    "FrameRate",
    "Duration",
    "OverriddenPAR",
    "OverriddenFieldType",
    "FrameRect",
    "CodecType",
    "IsStill",
    "IsFrameRateOverridden",
    "OveriddenFrameRate",
    "OriginalColorSpace",
    "IsFrameRectOverridden",
    "OverriddenFrameRect",
    "IsPAROverridden",
    "AlphaType",
    "IsAlphaTypeOverridden",
    "OverriddenAlphaType",
    "IsIgnoreAlphaOverridden",
    "OverriddenIgnoreAlpha",
    "IsInvertAlphaOverridden",
    "OverriddenInvertAlpha",
    "IsFieldTypeOverridden",
    "AlphaInfoIsUncertain",
    "FieldTypeIsUncertain",
    "OriginalImageOrientationType",
)


class FootageInterpretation:
    """How a clip item's footage is interpreted.

    Backed by the media's `VideoStream`: each setting is stored as an
    `Is*Overridden` flag plus an `Overridden*` value (values can linger
    after their flag clears, so a value only applies while its flag is
    true; note Premiere's own `OveriddenFrameRate` spelling). Every field
    is read/write; a frame-rate override also rewrites the media source's
    `OriginalDuration`, exactly as Premiere's own does (68_rate_override).
    """

    def __init__(self, _stream: ET.Element, _source: ET.Element | None) -> None:
        self._stream = _stream
        self._source = _source

    def _overridden(self, flag: str, value_tag: str) -> str | None:
        if self._stream.findtext(flag) != "true":
            return None
        return self._stream.findtext(value_tag)

    def _set_leaf(self, tag: str, text: str) -> None:
        existing = self._stream.find(tag)
        if existing is not None:
            existing.text = text
            return
        position = _VIDEO_STREAM_ORDER.index(tag)
        for sibling in self._stream:
            if (
                sibling.tag in _VIDEO_STREAM_ORDER
                and _VIDEO_STREAM_ORDER.index(sibling.tag) > position
            ):
                insert_before(self._stream, sibling.tag, _leaf_element(tag, text))
                return
        append_leaf(self._stream, tag, text)

    def _clear_leaf(self, tag: str) -> None:
        existing = self._stream.find(tag)
        if existing is not None:
            remove_child(self._stream, existing)

    @property
    def frame_rate(self) -> Time:
        """Effective ticks per frame. Read/write.

        Setting writes the override pair and rewrites the media source's
        `OriginalDuration` to the same frame count at the new rate, as
        Premiere's own override does (68_rate_override). Setting `None` -
        or the native rate - clears the override and restores the
        native-rate duration.
        """
        override = self._overridden("IsFrameRateOverridden", "OveriddenFrameRate")
        if override is not None:
            return Time(int(override))
        return Time(int(self._stream.findtext("FrameRate") or 0))

    @frame_rate.setter
    def frame_rate(self, value: Time | None) -> None:
        if value is not None:
            validate_time(value)
            if value.ticks <= 0:
                raise ValueError("frame rate must be positive ticks per frame")
        native = int(self._stream.findtext("FrameRate") or 0)
        if not native:
            raise ValueError("stream has no native frame rate")
        # The frame count must come from the CURRENT effective rate before
        # anything is rewritten. A still has no frame count to rescale (see
        # `_frame_count`), and its duration is left exactly as it is.
        frames = self._frame_count()
        if value is None or value.ticks == native:
            self._clear_leaf("IsFrameRateOverridden")
            self._clear_leaf("OveriddenFrameRate")
            if frames is not None:
                self._set_duration(frames * native)
            return
        self._set_leaf("IsFrameRateOverridden", "true")
        self._set_leaf("OveriddenFrameRate", str(value.ticks))
        if frames is not None:
            self._set_duration(frames * value.ticks)

    def _frame_count(self) -> int | None:
        # For timed media `OriginalDuration` equals frames x the EFFECTIVE
        # rate, and Premiere rewrites it alongside an override. A STILL does
        # not follow that: its duration is the 12-hour phantom span, which is
        # no whole number of frames at any rate (69_par_override /
        # 68_rate_override both carry one), so there is nothing to rescale
        # and the caller leaves the duration alone.
        if self._source is None:
            raise ValueError("item has no media source to rewrite")
        duration = int(self._source.findtext("OriginalDuration") or 0)
        rate = self.frame_rate.ticks
        if not rate:
            raise ValueError("stream has no frame rate")
        frames, remainder = divmod(duration, rate)
        if not remainder:
            return frames
        if self._stream.findtext("IsStill") == "true":
            return None
        # Timed media off the frame grid is not the still case: rescaling is
        # skipped there because there is nothing to rescale, but here it would
        # leave a duration still describing the OLD rate.
        raise ValueError("media duration is not a whole number of frames")

    def _set_duration(self, ticks: int) -> None:
        element = (
            None if self._source is None else self._source.find("OriginalDuration")
        )
        if element is None:
            raise ValueError("media source has no OriginalDuration")
        element.text = str(ticks)

    @property
    def alpha_usage(self) -> AlphaUsage:
        """Effective alpha interpretation. Read/write.

        Unoverridden, ExtendScript reports `NONE` regardless of the media's
        native `AlphaType` (validated against ground truth).
        """
        override = self._overridden("IsAlphaTypeOverridden", "OverriddenAlphaType")
        if override is not None:
            return AlphaUsage(int(override))
        return AlphaUsage.NONE

    @alpha_usage.setter
    def alpha_usage(self, value: AlphaUsage) -> None:
        _validate_alpha_usage(value)
        if value is AlphaUsage.NONE:
            self._clear_leaf("IsAlphaTypeOverridden")
            self._clear_leaf("OverriddenAlphaType")
            return
        self._set_leaf("IsAlphaTypeOverridden", "true")
        self._set_leaf("OverriddenAlphaType", str(int(value)))

    @property
    def ignore_alpha(self) -> bool:
        """Whether the alpha channel is ignored. Read/write.

        Unoverridden, this is the decoder's own answer - the native
        `IgnoreAlpha`, which Premiere stamps `true` on coded video streams -
        unlike `alpha_usage`, where ExtendScript ignores the native value.
        """
        override = self._overridden("IsIgnoreAlphaOverridden", "OverriddenIgnoreAlpha")
        if override is not None:
            return override == "true"
        return self._native_ignore_alpha

    @ignore_alpha.setter
    def ignore_alpha(self, value: bool) -> None:
        validate_bool(value)
        if value == self._native_ignore_alpha:
            # Back to the media's own answer: drop the override entirely
            # rather than pin a value that already holds.
            self._clear_leaf("IsIgnoreAlphaOverridden")
            self._clear_leaf("OverriddenIgnoreAlpha")
            return
        self._set_leaf("IsIgnoreAlphaOverridden", "true")
        self._set_leaf("OverriddenIgnoreAlpha", "true" if value else "false")

    @property
    def _native_ignore_alpha(self) -> bool:
        return self._stream.findtext("IgnoreAlpha") == "true"

    @property
    def invert_alpha(self) -> bool:
        """Whether the alpha channel is inverted. Read/write."""
        return (
            self._overridden("IsInvertAlphaOverridden", "OverriddenInvertAlpha")
            == "true"
        )

    @invert_alpha.setter
    def invert_alpha(self, value: bool) -> None:
        validate_bool(value)
        if value:
            self._set_leaf("IsInvertAlphaOverridden", "true")
            self._set_leaf("OverriddenInvertAlpha", "true")
        else:
            self._clear_leaf("IsInvertAlphaOverridden")
            self._clear_leaf("OverriddenInvertAlpha")

    @property
    def field_type(self) -> VideoFieldType:
        """Effective field dominance. Read/write."""
        override = self._overridden("IsFieldTypeOverridden", "OverriddenFieldType")
        if override is not None:
            return VideoFieldType(int(override))
        return VideoFieldType.DEFAULT

    @field_type.setter
    def field_type(self, value: VideoFieldType) -> None:
        _validate_field_type(value)
        if value is VideoFieldType.DEFAULT:
            self._clear_leaf("IsFieldTypeOverridden")
            self._clear_leaf("OverriddenFieldType")
            return
        self._set_leaf("OverriddenFieldType", str(int(value)))
        self._set_leaf("IsFieldTypeOverridden", "true")

    @property
    def pixel_aspect_ratio(self) -> float:
        """Effective pixel aspect ratio. Read/write.

        An override wins; failing that the media's own `OriginalPAR` applies,
        which is what anamorphic footage carries (HD bars store `40,33`).
        Square pixels are elided, so media with neither reads `1.0`.

        The setter takes either the exact `(numerator, denominator)` pair
        the format stores (`(40, 33)`) or a ratio, which is converted to the
        nearest exact fraction - so the value this getter returns can be
        handed straight back. `None` clears the override (69_par_override).
        """
        stored = self._overridden("IsPAROverridden", "OverriddenPAR")
        if stored is None:
            stored = self._stream.findtext("OriginalPAR")
        if stored is not None and "," in stored:
            numerator, denominator = stored.split(",", 1)
            return int(numerator) / int(denominator)
        return 1.0

    @pixel_aspect_ratio.setter
    def pixel_aspect_ratio(self, value: tuple[int, int] | float | None) -> None:
        if value is None:
            self._clear_leaf("IsPAROverridden")
            self._clear_leaf("OverriddenPAR")
            return
        if isinstance(value, bool):
            raise TypeError(
                "expected a (numerator, denominator) pair or a ratio, got bool"
            )
        if isinstance(value, (int, float)):
            # A ratio: convert to the exact pair the format stores, so the
            # float this property returns can be assigned straight back.
            validate_number(value)
            if value <= 0:
                raise ValueError("pixel aspect ratio must be positive")
            fraction = Fraction(value).limit_denominator(_PAR_MAX_DENOMINATOR)
            numerator, denominator = fraction.numerator, fraction.denominator
            self._write_par(numerator, denominator)
            return
        validate_vector2(value)
        numerator, denominator = value
        if not isinstance(numerator, int) or not isinstance(denominator, int):
            raise TypeError("pixel aspect ratio must be two integers")
        if numerator <= 0 or denominator <= 0:
            raise ValueError("pixel aspect ratio must be positive")
        self._write_par(numerator, denominator)

    def _write_par(self, numerator: int, denominator: int) -> None:
        # Asking for the ratio the media already has clears the override
        # rather than materializing one, the way `frame_rate` clears when
        # given the native rate - square pixels are elided, so re-setting
        # what the getter returned leaves the file untouched.
        native = self._stream.findtext("OriginalPAR") or "1,1"
        native_parts = native.split(",", 1)
        if len(native_parts) == 2:
            native_numerator, native_denominator = (int(p) for p in native_parts)
            if numerator * native_denominator == native_numerator * denominator:
                self._clear_leaf("IsPAROverridden")
                self._clear_leaf("OverriddenPAR")
                return
        self._set_leaf("OverriddenPAR", f"{numerator},{denominator}")
        self._set_leaf("IsPAROverridden", "true")

    def __repr__(self) -> str:
        return (
            f"FootageInterpretation(alpha_usage={self.alpha_usage.name}, "
            f"field_type={self.field_type.name})"
        )


class ProjectItem:
    """An item in the project panel: the root, a bin, or a clip.

    Structural items are parser-built, not user-constructed.
    """

    def __init__(
        self,
        _element: ET.Element,
        project: Project,
        item_type: ProjectItemType,
    ) -> None:
        self._element = _element
        self.project = project
        self._type = item_type
        self._children: list[ProjectItem] = []
        self._markers: list[Marker] = []
        self._media_path: Path | None = None
        self._master_element: ET.Element | None = None
        self._parent: ProjectItem | None = None
        self._clip_elements: list[ET.Element] = []
        self._node_id_int: int | None = None
        self._default_out_ticks: int | None = None
        self._sequence_uid: str | None = None

    def _name_element(self) -> ET.Element | None:
        # A clip item's live name is its master clip's; the ProjectItem/Name
        # copy goes stale when Premiere renames (validated against
        # ExtendScript ground truth).
        if self._master_element is not None:
            element = self._master_element.find("Name")
            if element is not None:
                return element
        core = project_item_core(self._element)
        return None if core is None else core.find("Name")

    @property
    def name(self) -> str:
        """The item name. Read/write.

        Like ExtendScript, the root item reports the project file name and
        a clip item reports its master clip's name.
        """
        if self._type is ProjectItemType.ROOT:
            return self.project.name
        element = self._name_element()
        return (element.text or "") if element is not None else ""

    @name.setter
    def name(self, value: str) -> None:
        _validate_name(value)
        if self._type is ProjectItemType.ROOT:
            raise AttributeError("the root item's name mirrors the project file name")
        element = self._name_element()
        if element is None:
            raise ValueError("item has no name element")
        element.text = value

    @property
    def type(self) -> ProjectItemType:
        """The item type. Read-only."""
        return self._type

    @property
    def is_sequence(self) -> bool:
        """Whether this clip item is backed by a sequence. Read-only."""
        return self._sequence_uid is not None

    def add_bin(self, name: str) -> ProjectItem:
        """Create a child bin and return it.

        Works on the root and on any bin (nesting into an empty bin
        synthesizes its child-item list). The view-state properties Premiere
        writes for a new bin are elided (Premiere synthesizes them on open).
        """
        _validate_name(name)
        if self._type is ProjectItemType.CLIP:
            raise ValueError("cannot add a bin under a clip item")
        container = self._element.find("ProjectItemContainer")
        if container is None:
            raise ValueError("item has no ProjectItemContainer")
        items = _child_items(container)
        document = self.project._document
        uid = str(uuid.uuid4())
        bin_element = _new_bin_element(uid, name)
        document.attach_object(bin_element)
        _add_item_ref(items, uid)
        child = ProjectItem(bin_element, self.project, ProjectItemType.BIN)
        child._parent = self
        self._children.append(child)
        return child

    def remove_bin(self, child: ProjectItem) -> None:
        """Remove a child bin, deleting its contents recursively.

        Child bins are removed depth-first; child clip items go through
        `remove_item` (so a bin holding an in-use clip refuses removal
        before anything is touched).
        """
        if child not in self._children:
            raise ValueError("item is not a child of this bin")
        if child._type is not ProjectItemType.BIN:
            raise ValueError("only bins can be removed")
        for grandchild in list(child._children):
            if grandchild._type is ProjectItemType.BIN:
                child.remove_bin(grandchild)
            else:
                child.remove_item(grandchild)
        document = self.project._document
        uid = child._element.get("ObjectUID")
        container = self._element.find("ProjectItemContainer")
        if container is not None and uid is not None:
            _detach_item_ref(container, uid)
        document.remove_object(child._element)
        self._children.remove(child)

    def remove_item(self, child: ProjectItem) -> None:
        """Remove a child clip item, deleting its media objects.

        Deletes the exact graph Premiere deletes with a panel item (master
        clip, template clips, logging, markers, source, media, streams),
        keeping anything still referenced from outside it (e.g. media
        shared with another item). Refuses when the item is placed on a
        timeline (remove the clips first) or backed by a sequence.
        """
        if child not in self._children:
            raise ValueError("item is not a child of this bin")
        if child._type is not ProjectItemType.CLIP:
            raise ValueError("only clip items can be removed; use remove_bin")
        if child.is_sequence:
            raise ValueError("removing a sequence item is not supported")
        master = child._master_element
        if master is None:
            raise ValueError("item has no master clip")
        document = self.project._document
        # One pass answers both questions: who still points at the master
        # clip (a timeline placement, which blocks the removal), and what the
        # item exclusively owns (the graph that goes with it).
        index = ReferenceIndex(document)
        if index.referrers_outside(master, [child._element, master]):
            raise ValueError("item is in use on a timeline; remove its clips first")
        uid = child._element.get("ObjectUID")
        container = self._element.find("ProjectItemContainer")
        if container is not None and uid is not None:
            _detach_item_ref(container, uid)
        for element in document.owned_objects([child._element, master], index):
            document.remove_object(element)
        self._children.remove(child)
        # The lookup maps hold this item keyed by its master/sequence UID.
        self.project._items_by_master_uid = None
        self.project._items_by_sequence_uid = None

    def move_to(self, new_parent: ProjectItem) -> None:
        """Move this item into another bin (or the root).

        Only the parent-child wiring changes; the item's own object is
        untouched.
        """
        if self._type is ProjectItemType.ROOT:
            raise ValueError("cannot move the root item")
        if new_parent.project is not self.project:
            # Wiring a reference across documents leaves the destination with
            # a dangling ObjectURef and the source with an orphan object.
            raise ValueError("cannot move an item into another project")
        if new_parent._type is ProjectItemType.CLIP:
            raise ValueError("cannot move an item under a clip")
        if self._parent is None:
            raise ValueError("item has no parent to move from")
        if new_parent is self._parent:
            return
        ancestor: ProjectItem | None = new_parent
        while ancestor is not None:
            if ancestor is self:
                raise ValueError("cannot move an item into itself or a descendant")
            ancestor = ancestor._parent
        container = new_parent._element.find("ProjectItemContainer")
        if container is None:
            raise ValueError("destination has no ProjectItemContainer")
        destination = _child_items(container)
        uid = self._element.get("ObjectUID")
        if uid is None:
            raise ValueError("item has no ObjectUID")
        source = self._parent._element.find("ProjectItemContainer")
        if source is not None:
            _detach_item_ref(source, uid)
        _add_item_ref(destination, uid)
        self._parent._children.remove(self)
        new_parent._children.append(self)
        self._parent = new_parent

    @property
    def color_label(self) -> int:
        """The item's color label as an index (`0` when none is stored). Read/write.

        Stored in the item's property bag as `Column.PropertyText.Label` =
        `BE.Prefs.LabelColors.<index>`; the index matches ExtendScript's
        `getColorLabel` / UXP `getColorLabelIndex`. Items without the
        property (e.g. the root) report `0`, as ExtendScript does. Setting an
        index in `1..15` writes (or updates) the property; setting `0` clears
        the stored override, so the item reads back as `0`.
        """
        core = project_item_core(self._element)
        text = (
            None
            if core is None
            else core.findtext("Node/Properties/Column.PropertyText.Label")
        )
        prefix = "BE.Prefs.LabelColors."
        if text is None or not text.startswith(prefix):
            return 0
        suffix = text[len(prefix) :]
        return int(suffix) if suffix.isdigit() else 0

    @color_label.setter
    def color_label(self, index: int) -> None:
        validate_color_label(index)
        properties = self._element.find("ProjectItem/Node/Properties")
        if properties is None:
            raise ValueError("item has no property bag to store a color label")
        existing = properties.find("Column.PropertyText.Label")
        if index == 0:
            if existing is not None:
                remove_child(properties, existing)
            return
        text = "BE.Prefs.LabelColors." + str(index)
        if existing is not None:
            existing.text = text
        else:
            append_leaf(properties, "Column.PropertyText.Label", text)

    @property
    def node_id(self) -> str | None:
        """The item's identifier: the node ID in 8-digit hex. Read-only.

        Best-effort: Premiere assigns these per session at load, so they
        are reproducible only for freshly saved projects.
        """
        if self._node_id_int is None:
            return None
        return format(self._node_id_int, "08x")

    @property
    def tree_path(self) -> str:
        r"""The item's path in the project panel (`\project\bin\item`). Read-only."""
        if self._parent is None:
            return "\\" + self.project.name
        return self._parent.tree_path + "\\" + self.name

    @property
    def in_point(self) -> Time | None:
        """The item's in point in media time. Read-only.

        `None` for items without media (the root, bins), where ExtendScript
        reports the -400000 s unset sentinel.
        """
        ticks = self._point_ticks("InPoint")
        return None if ticks == UNSET_TICKS else Time(ticks)

    @property
    def out_point(self) -> Time | None:
        """The item's out point in media time. Read-only.

        `None` for items without media, like `in_point`.
        """
        ticks = self._point_ticks("OutPoint")
        return None if ticks == UNSET_TICKS else Time(ticks)

    def _point_ticks(self, tag: str) -> int:
        if not self._clip_elements:
            return UNSET_TICKS
        core = clip_core(self._clip_elements[0])
        stored = None if core is None else core.findtext(tag)
        if stored:
            return int(stored)
        if tag == "OutPoint":
            if self._sequence_uid is not None:
                # A sequence-backed item reports the LIVE sequence end, not
                # the stored (stale) source duration.
                for sequence in self.project.sequences:
                    if sequence.sequence_id == self._sequence_uid:
                        return sequence.end.ticks
            if self._default_out_ticks is not None:
                # Unset out point: ExtendScript reports the source duration.
                return self._default_out_ticks
        return 0

    @property
    def children(self) -> NamedList[ProjectItem]:
        """Child items of a bin, indexable by name. Read-only."""
        return NamedList(self._children)

    def __iter__(self) -> Iterator[ProjectItem]:
        return iter(self._children)

    def __len__(self) -> int:
        return len(self._children)

    def __getitem__(self, key: int | str) -> ProjectItem:
        return self.children[key]

    def __contains__(self, item: object) -> bool:
        return item in self.children

    def walk(self) -> Iterator[ProjectItem]:
        """Every descendant item, depth-first."""
        for child in self._children:
            yield child
            yield from child.walk()

    @property
    def footage_interpretation(self) -> FootageInterpretation | None:
        """The item's footage interpretation. Read-only.

        `None` for items without a video stream (bins, audio-only media).
        """
        media = self._media_element()
        if media is None:
            return None
        stream_ref = media.find("VideoStream")
        if stream_ref is None:
            return None
        document = self.project._document
        core = self._own_clip_core()
        source_ref = None if core is None else core.find("Source")
        source = None if source_ref is None else document.resolve(source_ref)
        return FootageInterpretation(document.resolve(stream_ref), source)

    @property
    def scale_to_frame_size(self) -> bool:
        """Whether placements scale the media to the sequence frame size.
        Read/write.

        Stored as `ScaleToFramePolicy` (1) on the master's template video
        clip and elided when off (70_scale_to_frame); mirrors
        ExtendScript's `setScaleToFrameSize`.
        """
        for clip in self._clip_elements:
            if clip.tag == "VideoClip":
                return clip.findtext("ScaleToFramePolicy") == "1"
        return False

    @scale_to_frame_size.setter
    def scale_to_frame_size(self, value: bool) -> None:
        validate_bool(value)
        for clip in self._clip_elements:
            if clip.tag == "VideoClip":
                set_elided_flag(clip, "ScaleToFramePolicy", value, text="1")
                return
        raise ValueError("item has no video clip to scale")

    def _media_element(self) -> ET.Element | None:
        # The `Media` object backing this clip item (master clip -> template
        # clip -> Source -> MediaSource/Media).
        core = self._own_clip_core()
        if core is None:
            return None
        source_ref = core.find("Source")
        if source_ref is None:
            return None
        document = self.project._document
        media_ref = document.resolve(source_ref).find("MediaSource/Media")
        if media_ref is None:
            return None
        return document.resolve(media_ref)

    @property
    def is_offline(self) -> bool:
        """Whether the item's media is offline. Read-only.

        Stored as `Media/OfflineReason`; elided when the media is online.
        """
        media = self._media_element()
        return media is not None and media.find("OfflineReason") is not None

    @property
    def generator_id(self) -> str | None:
        """The four-character code of the item's synthetic media, or `None`.

        Generated media (Black Video, a colour matte, bars and tone, ...) has
        no file: Premiere stores a big-endian fourcc where the path would go,
        which is why `media_path` is `None` for these items. `None` here means
        the item is backed by a real file (or by nothing at all).
        """
        media = self._media_element()
        if media is None:
            return None
        stored = media.findtext("FilePath") or ""
        if not stored.isdigit():
            return None
        return struct.pack(">I", int(stored)).decode("ascii", "replace")

    @property
    def generator_type(self) -> GeneratorType | None:
        """Which of Premiere's generators backs the item, or `None`.

        `None` both for real media and for a generator py has no code for -
        `generator_id` still reports the raw fourcc in that case. Note an
        adjustment layer is backed by Black Video, so it reports
        `BLACK_VIDEO` here and identifies itself through
        `is_adjustment_layer`.
        """
        media = self._media_element()
        if media is None:
            return None
        stored = media.findtext("FilePath") or ""
        if not stored.isdigit():
            return None
        try:
            return GeneratorType(int(stored))
        except ValueError:
            return None

    @property
    def is_adjustment_layer(self) -> bool:
        """Whether the item is an adjustment layer. Read-only.

        Stored as `MasterClip/IsAdjustmentLayer`. The media underneath is
        Premiere's synthetic Black Video generator, so the flag - not the
        media - is what identifies it.
        """
        if self._master_element is None:
            return False
        return self._master_element.findtext("IsAdjustmentLayer") == "true"

    @property
    def is_multicam_clip(self) -> bool:
        """Whether the item is a multi-camera source sequence. Read-only."""
        if self._master_element is None:
            return False
        enabled = self._master_element.findtext(
            "Node/Properties/Source.Monitor.Multicam.Enabled"
        )
        return enabled == "true"

    @property
    def is_mgt(self) -> bool:
        """Whether the item came from a Motion Graphics template. Read-only.

        An imported template's master hangs its Essential Graphics
        controls off a `BlueprintVideoComponentChain` - a slot no other
        kind of master clip uses - so the presence of that chain is what
        identifies one.
        """
        if self._master_element is None:
            return False
        return self._master_element.find("BlueprintVideoComponentChain") is not None

    @property
    def is_merged_clip(self) -> bool:
        """Whether the item is a merged clip. Read-only.

        Merged clips and multicam clips are both backed by a hidden
        `Sequence` combining the source files; the merged one flags itself in
        that sequence's property bag as `BE.Sequence.IsMergedClip`.
        """
        sequence = self._backing_sequence()
        if sequence is None:
            return False
        flag = sequence.findtext("Node/Properties/BE.Sequence.IsMergedClip")
        return flag == "true"

    def _backing_sequence(self) -> ET.Element | None:
        # The `Sequence` object an item's clips play from, for the item kinds
        # Premiere implements as a hidden sequence (merged and multicam
        # clips) as well as ordinary sequence items.
        core = self._own_clip_core()
        if core is None:
            return None
        source_ref = core.find("Source")
        if source_ref is None:
            return None
        document = self.project._document
        sequence_ref = document.resolve(source_ref).find("SequenceSource/Sequence")
        if sequence_ref is None:
            return None
        return document.resolve(sequence_ref)

    @property
    def start_time(self) -> Time:
        """The item's start time. Read/write.

        The start timecode embedded in the media, stored as
        `Media/AlternateStart` and honoured only while `UseAlternateStart`
        is set. Media with no timecode track - and items with no media -
        start at zero.

        The setter mirrors ExtendScript's `setStartTime` and is supported
        on media that already stores a start timecode: replaying Premiere's
        own edit touches `AlternateStart` alone (62_start_time vs
        19_timecode), so no reference exists yet for synthesizing the pair
        on timecode-less media.

        Persistence caveat, measured on the resave gate: for media with an
        EMBEDDED timecode, Premiere re-reads the media on open and restores
        `AlternateStart` to the embedded value, reverting this edit on its
        next resave - ExtendScript's own `setStartTime` suffers the same
        fate. The GUI's `Modify > Timecode` evidently writes something
        stickier; that representation is undecoded (see CAMPAIGN.md).
        """
        media = self._media_element()
        if media is None or media.findtext("UseAlternateStart") != "true":
            return Time(0)
        return Time(int(media.findtext("AlternateStart") or 0))

    @start_time.setter
    def start_time(self, value: Time) -> None:
        validate_time(value)
        if value.ticks < 0:
            raise ValueError("start time cannot be negative")
        media = self._media_element()
        alternate = None if media is None else media.find("AlternateStart")
        flag = None if media is None else media.find("UseAlternateStart")
        if alternate is None or flag is None:
            raise ValueError(
                "start time is only settable on media that stores a start "
                "timecode (Media/AlternateStart)"
            )
        alternate.text = str(value.ticks)
        flag.text = "true"

    @property
    def has_proxy(self) -> bool:
        """Whether the item has proxy media attached. Read-only."""
        return self._proxy_media() is not None

    @property
    def proxy_path(self) -> Path | None:
        """The path of the item's proxy media, if any. Read-only.

        `None` when no proxy is attached (ExtendScript's `getProxyPath`
        reports `0` in that case).
        """
        media = self._proxy_media()
        stored = None if media is None else media.findtext("FilePath")
        return Path(stored) if stored else None

    def attach_proxy(self, path: str | Path, is_hi_res: bool = False) -> None:
        """Attach proxy media to this item.

        ExtendScript's `attachProxy`. A proxy is a second `Media` object
        (flagged `IsProxy`) with its own stream, referenced from the media
        source's `Content` bag; the proxy's stream carries the HI-RES
        frame rect as an override so the item keeps reporting the original
        raster (18_proxy). The file must be readable and match the item's
        frame aspect ratio, as Premiere itself requires.

        `is_hi_res` swaps the roles, as the ExtendScript flag does: `path`
        becomes the media this item PLAYS and what it played until now is
        demoted to the proxy. The end state is the same graph either way -
        one `Media` flagged `IsProxy`, one not - and the MASTER CLIP takes
        the new file's name while the panel item's own `Name` is left
        behind, stale, which is what Premiere does too (verified against
        its calls in both directions, `samples/refs/gaps/proxy_*.prproj`).
        """
        _validate_proxy_path(path)
        validate_bool(is_hi_res)
        if self.has_proxy:
            raise ValueError("item already has proxy media attached")
        content = self._media_content()
        media = self._media_element()
        stream_ref = None if media is None else media.find("VideoStream")
        if content is None or media is None or stream_ref is None:
            raise ValueError("proxies attach to video media clip items")
        document = self.project._document
        own_rect = document.resolve(stream_ref).findtext("FrameRect")
        if not own_rect:
            raise ValueError("source video stream has no frame rect")
        if not is_hi_res:
            proxy_uid = self.project._make_proxy_media(Path(path), own_rect)
            append_child(content, ET.Element("ProxyMedia", {"ObjectURef": proxy_uid}))
            return
        self._attach_hi_res(document, content, media, own_rect, Path(path))

    def _attach_hi_res(
        self,
        document: PremiereDocument,
        content: ET.Element,
        media: ET.Element,
        own_rect: str,
        path: Path,
    ) -> None:
        # The newcomer becomes the media and the incumbent the proxy, so
        # the frame-rect override lands on what was already here - carrying
        # the NEW file's raster, since that is now the hi-res one.
        source = self._media_source()
        reference = None if source is None else source.find("MediaSource/Media")
        if reference is None:
            raise ValueError("media source has no Media reference")
        media_uid, hires_rect = self.project._make_primary_media(path, own_rect)
        reference.set("ObjectURef", media_uid)

        stream_ref = media.find("VideoStream")
        if stream_ref is None:
            raise ValueError("media has no video stream to override")
        _override_frame_rect(document.resolve(stream_ref), hires_rect)
        append_leaf(media, "IsProxy", "true")
        demoted = media.get("ObjectUID") or ""
        append_child(content, ET.Element("ProxyMedia", {"ObjectURef": demoted}))
        # The master follows the new media; Premiere leaves the panel
        # item's own copy of the name on the old file.
        master = self._master_element
        name = master.find("Name") if master is not None else None
        if name is not None:
            name.text = path.name
        # The item plays the newcomer now; without this the live object
        # keeps reporting the file it just demoted to proxy, and
        # `create_sub_clip` would re-import the proxy.
        self._media_path = path.resolve()

    def _proxy_media(self) -> ET.Element | None:
        # Proxy media is a SECOND `Media` object (flagged `IsProxy`), hung
        # off the media source's own Content bag rather than the master clip.
        content = self._media_content()
        if content is None:
            return None
        proxy_ref = content.find("ProxyMedia")
        if proxy_ref is None:
            return None
        return self.project._document.resolve(proxy_ref)

    def _media_source(self) -> ET.Element | None:
        # The `*MediaSource` object this item's template clip plays from.
        core = self._own_clip_core()
        source_ref = None if core is None else core.find("Source")
        if source_ref is None:
            return None
        return self.project._document.resolve(source_ref)

    def _media_content(self) -> ET.Element | None:
        # The media source's own `Content` bag, which carries the state that
        # belongs to this item's *view* of the media rather than to the media
        # itself (its proxy, and a subclip's boundaries).
        core = self._own_clip_core()
        if core is None:
            return None
        source_ref = core.find("Source")
        if source_ref is None:
            return None
        source = self.project._document.resolve(source_ref)
        return source.find("MediaSource/Content")

    @property
    def is_subclip(self) -> bool:
        """Whether the item is a subclip of another item. Read-only.

        A subclip is a second master clip over the same media, narrowed by
        boundaries on its own media source. Its `in_point` and `out_point`
        still describe the whole file - `subclip_in_point` and
        `subclip_out_point` are the narrowed range.
        """
        return self._boundary("StartBoundary") is not None

    @property
    def subclip_in_point(self) -> Time | None:
        """Where the subclip starts in media time, or `None`. Read-only."""
        return self._boundary("StartBoundary")

    @property
    def subclip_out_point(self) -> Time | None:
        """Where the subclip ends in media time, or `None`. Read-only."""
        return self._boundary("EndBoundary")

    @property
    def has_hard_boundaries(self) -> bool:
        """Whether the subclip's boundaries are hard. Read-only.

        ExtendScript's `createSubClip` calls this `hardBoundaries`: soft
        boundaries can be trimmed past on the timeline, hard ones cannot.
        `False` for items that are not subclips.
        """
        content = self._media_content()
        if content is None:
            return False
        return content.findtext("BoundariesAreHard") == "true"

    def _boundary(self, tag: str) -> Time | None:
        content = self._media_content()
        if content is None:
            return None
        stored = content.findtext(tag)
        return None if stored is None else Time(int(stored))

    def create_sub_clip(
        self,
        name: str,
        start: Time,
        end: Time,
        has_hard_boundaries: bool = False,
        take_video: bool = True,
        take_audio: bool = True,
    ) -> ProjectItem:
        """Create a subclip of this item in the project panel and return it.

        Mirrors ExtendScript's `createSubClip`. A subclip is a SECOND master
        clip over the same media file - Premiere duplicates the whole media
        graph rather than sharing objects (28_subclip) - narrowed by
        `StartBoundary`/`EndBoundary` on its media source. Soft boundaries
        can be trimmed past on the timeline, hard ones cannot.

        `take_video`/`take_audio` subclip only part of an A/V source, as
        ExtendScript's trailing flags do. NOTE the scripting guide lists
        them the other way round (`takeAudio, takeVideo`); driving the
        real call proved the fifth argument governs VIDEO and the sixth
        AUDIO, so py names them in their actual order. Dropping a half
        leaves the media's stream in place and omits that half's clip and
        source from the new master, which is what Premiere writes.

        The subclip's graph is synthesized the way an import is, so the
        media file must still be readable; it then takes over the source's
        file identity (one `FileKey`/content state per file, the
        modification blob carried once), and the source's footage
        interpretation overrides do NOT carry over.
        """
        _validate_name(name)
        validate_time(start)
        validate_time(end)
        validate_bool(has_hard_boundaries)
        validate_bool(take_video)
        validate_bool(take_audio)
        if not (take_video or take_audio):
            raise ValueError("a subclip must take the video, the audio, or both")
        if self._type is not ProjectItemType.CLIP or self.is_sequence:
            raise ValueError("subclips can only be created from media clip items")
        if self.media_path is None:
            raise ValueError("item has no media file to subclip")
        if not 0 <= start.ticks < end.ticks:
            raise ValueError("subclip boundaries must satisfy 0 <= start < end")
        if self._master_element is None:
            raise ValueError("item has no master clip")
        document = self.project._document
        item = self.project.import_files([self.media_path])[0]
        # The master carries the live subclip name; the ProjectItem copy
        # keeps the file name, exactly as Premiere writes it.
        master = item._master_element
        if master is None:
            raise ValueError("synthesized subclip has no master clip")
        name_element = master.find("Name")
        if name_element is not None:
            name_element.text = name
        logging_ref = master.find("LoggingInfo")
        if logging_ref is None:
            raise ValueError("synthesized subclip has no logging info")
        logging = document.resolve(logging_ref)
        for tag, text in (
            ("ClipName", name),
            ("MediaInPoint", str(start.ticks)),
            ("MediaOutPoint", str(end.ticks)),
        ):
            leaf = logging.find(tag)
            if leaf is not None:
                leaf.text = text
        if not (take_video and take_audio):
            item._drop_subclip_half(take_video)
        # A/V media narrows BOTH its sources: the identical boundary trio
        # lands in the video AND the audio source's Content bag
        # (71_av_subclip) - or in the surviving one alone when a half was
        # dropped.
        contents = []
        for clip_element in item._clip_elements:
            core = clip_core(clip_element)
            source_ref = None if core is None else core.find("Source")
            if source_ref is None:
                continue
            content = document.resolve(source_ref).find("MediaSource/Content")
            if content is not None:
                contents.append(content)
        if not contents:
            raise ValueError("synthesized subclip has no media content bag")
        for content in contents:
            append_leaf(content, "StartBoundary", str(start.ticks))
            append_leaf(content, "EndBoundary", str(end.ticks))
            append_leaf(
                content,
                "BoundariesAreHard",
                "true" if has_hard_boundaries else "false",
            )
        # The panel label stamp (`Column.PropertyText.Label`) is kept as the
        # import wrote it: Premiere itself is inconsistent - 28_subclip's
        # subclip carries none, 71_av_subclip's does - and a resave
        # re-stamps it either way.
        self._share_file_identity(item)
        return item

    def _drop_subclip_half(self, keep_video: bool) -> None:
        # Keep only the video or only the audio half of a freshly imported
        # A/V master, as Premiere's take flags do: the clip and everything
        # only it referenced go, the shared Media (and both its streams)
        # stay. An audio-only master keeps its component chains and channel
        # groups; a video-only one has neither to keep.
        document = self.project._document
        master = self._master_element
        if master is None:
            raise ValueError("item has no master clip")
        wanted = "VideoClip" if keep_video else "AudioClip"
        clips = master.find("Clips")
        if clips is None:
            raise ValueError("master clip has no Clips list")
        doomed = []
        for reference in list(clips.findall("Clip")):
            clip = document.resolve(reference)
            if clip.tag == wanted:
                continue
            doomed.append(clip)
            remove_child(clips, reference)
        if not doomed:
            return
        for index, reference in enumerate(clips.findall("Clip")):
            reference.set("Index", str(index))
        orphaned: list[ET.Element] = []
        if keep_video:
            # The audio plumbing belongs to the half being dropped - but NOT
            # `DefMappingID`, which Premiere keeps on a video-only subclip
            # (samples/refs/audit/sub_audio.prproj).
            chains = master.find("AudioComponentChains")
            if chains is not None:
                orphaned = [document.resolve(entry) for entry in chains]
                remove_child(master, chains)
        index_of = ReferenceIndex(document)
        # Unhooking that list leaves its chains referenced by nothing, and
        # `owned_objects` only reaches what the doomed CLIPS point at - so
        # they have to be seeded, or they survive as top-level orphans.
        doomed.extend(
            chain for chain in orphaned if not index_of.referrers.get(id(chain))
        )
        for element in document.owned_objects(doomed, index_of):
            document.remove_object(element)
        self._clip_elements = [
            document.resolve(reference) for reference in clips.findall("Clip")
        ]

    def _share_file_identity(self, other: ProjectItem) -> None:
        # Premiere keeps ONE file identity per media file: every Media
        # object describing it shares FileKey/content state, and only the
        # first carries the modification blob - the rest hash-reference it
        # (the multi-channel import rule, seen again on 28_subclip).
        source_media = self._media_element()
        target_media = other._media_element()
        if source_media is None or target_media is None:
            return
        for tag in ("FileKey", "ContentAndMetadataState"):
            stored = source_media.findtext(tag)
            leaf = target_media.find(tag)
            if stored and leaf is not None:
                leaf.text = stored
        source_state = source_media.find("ModificationState")
        target_state = target_media.find("ModificationState")
        if source_state is not None and target_state is not None:
            binary_hash = source_state.get("BinaryHash")
            if binary_hash:
                target_state.set("BinaryHash", binary_hash)
            target_state.text = None
        content_state = source_media.findtext("ContentAndMetadataState")
        core = other._own_clip_core()
        markers_ref = None if core is None else core.find("MarkerOwner/Markers")
        if content_state and markers_ref is not None:
            markers = self.project._document.resolve(markers_ref)
            last = markers.find("LastContentState")
            if last is not None:
                last.text = content_state

    @property
    def markers(self) -> NamedList[Marker]:
        """The item's clip markers, indexable by name. Read-only.

        Stored on the master clip's own template clip, shared with every
        timeline instance of the item. A sequence-backed item's own markers
        are separate from the sequence's markers (matching UXP). Bins and
        the root have none.
        """
        return NamedList(self._markers)

    def _own_clip_core(self) -> ET.Element | None:
        # The master clip's own template clip core (`Clips[0]` -> `Clip`),
        # where item markers live.
        if self._master_element is None:
            return None
        reference = self._master_element.find("Clips/Clip")
        if reference is None:
            return None
        return clip_core(self.project._document.resolve(reference))

    def add_marker(
        self,
        name: str,
        start: Time,
        comments: str = "",
        marker_type: str = "Comment",
        duration: Time | None = None,
    ) -> Marker:
        """Create a clip marker on this item and return it.

        `start` is in media time (a still's first frame sits at its 1-hour
        default timecode, not 0).
        """
        if self._type is not ProjectItemType.CLIP:
            raise ValueError("only clip items carry markers")
        core = self._own_clip_core()
        if core is None:
            raise ValueError("item has no master clip")
        document = self.project._document
        inner = _ensure_clip_marker_list(document, core)
        marker = Marker(name, start, comments, marker_type, duration)
        _attach_marker(document, inner, marker)
        self._markers.append(marker)
        return marker

    def remove_marker(self, marker: Marker) -> None:
        """Remove a clip marker from this item."""
        if marker not in self._markers:
            raise ValueError("marker does not belong to this item")
        core = self._own_clip_core()
        reference = core.find("MarkerOwner/Markers") if core is not None else None
        if reference is None:
            raise ValueError("master clip has no marker collection")
        document = self.project._document
        inner = document.resolve(reference).find("Markers")
        if inner is None:
            raise ValueError("marker collection has no inner list")
        _detach_marker(document, inner, marker)
        self._markers.remove(marker)

    @property
    def media_path(self) -> Path | None:
        """The path of the underlying media file, if any. Read-only.

        Premiere's internal generators (e.g. `Black Video`) store a numeric
        token instead of a filesystem path; they report `None` here (see
        `generator_type`).
        """
        return self._media_path

    def __repr__(self) -> str:
        return f"ProjectItem(name={self.name!r}, type={self._type.name})"
