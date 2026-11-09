"""The `Sequence` model."""

from __future__ import annotations

import uuid
import xml.etree.ElementTree as ET
from typing import TYPE_CHECKING

from ..enums import CaptionFormat
from ..xml.mutations import (
    append_child,
    append_leaf,
    insert_before,
    insert_leaf_before,
    remove_child,
)
from .caption_builder import (
    DATA_MEDIA_TYPE,
    SYNTHETIC_DURATION,
    SYNTHETIC_STATE,
    TEMPLATE_STYLE,
    new_caption_subclip,
    new_caption_track,
    new_caption_track_item,
    new_data_chain,
    new_data_media_source,
    new_empty_logging_info,
    new_synthetic_media,
    new_synthetic_stream,
    new_synthetic_template_clip,
    new_timeline_block,
    new_timeline_caption_clip,
)
from .descriptors import XmlField
from .marker import Marker, _attach_marker, _detach_marker
from .media_import import new_channel_groups, new_master_clip
from .merged_builder import build_link
from .named_list import NamedList
from .time import TICKS_PER_SECOND, UNSET_TICKS, Time, validate_time
from .validators import (
    validate_bool,
    validate_enum,
    validate_int,
    validate_positive_int,
    validate_string,
    validate_vector2,
)

_validate_settings_string = validate_string(allow_empty=False)
_validate_caption_format = validate_enum(CaptionFormat)

if TYPE_CHECKING:
    from pathlib import Path

    from ..xml import PremiereDocument
    from .caption import CaptionTrack
    from .project import Project
    from .project_item import ProjectItem
    from .track import Track
    from .track_item import TrackItem


#: ClassID of a Markers collection object.
_MARKERS_CLASS_ID = "bee50706-b524-416c-9f03-b596ce5f6866"
_ZERO_GUID = "00000000-0000-0000-0000-000000000000"

#: The property-bag keys the setters touch, in Premiere's stored order, with
#: the neighbouring keys that never move so a created one has something to
#: anchor against. Read off 386 sequences across the sample corpus; a bag
#: holding only some of them keeps the survivors in this order.
_BAG_ORDER = (
    "MZ.InPoint",
    "MZ.OutPoint",
    "MZ.ZeroPoint",
    "AMM.CurrentSolo",
    "TL.SQAVDividerPosition",
    "MZ.WorkInPoint",
    "MZ.WorkOutPoint",
    "MZ.EditLine",
    "MZ.Sequence.VideoTimeDisplayFormat",
)

#: Property-bag keys the setters may CREATE when absent - a virgin sequence
#: stores no in/out/zero trio, and a sequence Premiere derived from a clip
#: can be missing the work area and playhead too.
_CREATABLE_BAG_KEYS = (
    "MZ.InPoint",
    "MZ.OutPoint",
    "MZ.ZeroPoint",
    "MZ.WorkInPoint",
    "MZ.WorkOutPoint",
    "MZ.EditLine",
)


class SequenceSettings:
    """The stored sequence settings, mirroring ExtendScript `getSettings`.

    Only file-backed fields are exposed; session-derived ones
    (`videoFieldType`, `videoPixelAspectRatio`) have no stored home.
    """

    def __init__(self, _sequence: Sequence) -> None:
        self._sequence = _sequence

    def _write(self, key: str, value: int | str) -> None:
        # Settings keys are modify-in-place only: every sequence Premiere
        # or py builds carries them, and their creation position is
        # unverified, so an absent key raises rather than guessing.
        if self._sequence._bag_element(key) is None:
            raise ValueError(f"sequence has no {key} property")
        self._sequence._bag_write(key, value)

    @property
    def editing_mode(self) -> str | None:
        """The editing-mode GUID. Read/write."""
        element = self._sequence._bag_element("MZ.Sequence.EditingModeGUID")
        return element.text if element is not None else None

    @editing_mode.setter
    def editing_mode(self, value: str) -> None:
        _validate_settings_string(value)
        self._write("MZ.Sequence.EditingModeGUID", value)

    @property
    def max_bit_depth(self) -> bool:
        """Whether previews render at maximum bit depth. Read/write."""
        element = self._sequence._bag_element("MZ.Sequence.PreviewUseMaxBitDepth")
        return element is not None and element.text == "true"

    @max_bit_depth.setter
    def max_bit_depth(self, value: bool) -> None:
        validate_bool(value)
        self._write("MZ.Sequence.PreviewUseMaxBitDepth", "true" if value else "false")

    @property
    def max_render_quality(self) -> bool:
        """Whether previews render at maximum quality. Read/write."""
        element = self._sequence._bag_element("MZ.Sequence.PreviewUseMaxRenderQuality")
        return element is not None and element.text == "true"

    @max_render_quality.setter
    def max_render_quality(self, value: bool) -> None:
        validate_bool(value)
        self._write(
            "MZ.Sequence.PreviewUseMaxRenderQuality", "true" if value else "false"
        )

    @property
    def preview_codec(self) -> int | None:
        """The preview codec fourcc. Read/write."""
        return self._sequence._bag_int("MZ.Sequence.PreviewRenderingPresetCodec")

    @preview_codec.setter
    def preview_codec(self, value: int) -> None:
        validate_int(value)
        self._write("MZ.Sequence.PreviewRenderingPresetCodec", value)

    @property
    def preview_rendering_preset_path(self) -> str | None:
        """The preview encoder preset path. Read/write."""
        element = self._sequence._bag_element("MZ.Sequence.PreviewRenderingPresetPath")
        return element.text if element is not None else None

    @preview_rendering_preset_path.setter
    def preview_rendering_preset_path(self, value: str) -> None:
        _validate_settings_string(value)
        self._write("MZ.Sequence.PreviewRenderingPresetPath", value)

    @property
    def preview_frame_size(self) -> tuple[int, int] | None:
        """The preview `(width, height)`. Read/write."""
        width = self._sequence._bag_int("MZ.Sequence.PreviewFrameSizeWidth")
        height = self._sequence._bag_int("MZ.Sequence.PreviewFrameSizeHeight")
        if width is None or height is None:
            return None
        return width, height

    @preview_frame_size.setter
    def preview_frame_size(self, value: tuple[int, int]) -> None:
        validate_vector2(value)
        width, height = value
        if not isinstance(width, int) or not isinstance(height, int):
            raise TypeError("preview frame size must be two integers")
        if width <= 0 or height <= 0:
            raise ValueError("preview frame size must be positive")
        # Check both keys up front so a missing height cannot leave a
        # half-written pair behind.
        for key in (
            "MZ.Sequence.PreviewFrameSizeWidth",
            "MZ.Sequence.PreviewFrameSizeHeight",
        ):
            if self._sequence._bag_element(key) is None:
                raise ValueError(f"sequence has no {key} property")
        self._sequence._bag_write("MZ.Sequence.PreviewFrameSizeWidth", width)
        self._sequence._bag_write("MZ.Sequence.PreviewFrameSizeHeight", height)

    @property
    def video_frame_rate(self) -> Time | None:
        """Ticks per video frame as a `Time`. Read-only."""
        timebase = self._sequence.timebase
        return Time(timebase) if timebase is not None else None

    @property
    def video_frame_size(self) -> tuple[int, int] | None:
        """The sequence `(width, height)`. Read-only."""
        return self._sequence.frame_size

    @property
    def audio_channel_count(self) -> int | None:
        """Audio channel count. Read-only."""
        return self._sequence.audio_channel_count

    @property
    def audio_sample_rate(self) -> Time | None:
        """Ticks per audio sample as a `Time`. Read-only."""
        rate = self._sequence.audio_frame_rate
        return Time(rate) if rate is not None else None

    def __repr__(self) -> str:
        return (
            f"SequenceSettings(editing_mode={self.editing_mode!r}, "
            f"video_frame_size={self.video_frame_size})"
        )


class Sequence:
    """A sequence (timeline)."""

    #: The sequence name. Reads as empty when the element is absent (only
    #: synthetic files lack it; Premiere always writes one).
    name = XmlField[str]("Name", validate=validate_string(), default="")

    def __init__(self, _element: ET.Element, project: Project) -> None:
        self._element = _element
        self.project = project
        self._timebase: int | None = None
        self._frame_size: tuple[int, int] | None = None
        self._audio_frame_rate: int | None = None
        self._audio_channel_count: int | None = None
        self._video_tracks: list[Track] = []
        self._audio_tracks: list[Track] = []
        self._caption_tracks: list[CaptionTrack] = []
        self._markers: list[Marker] = []

    @property
    def markers(self) -> NamedList[Marker]:
        """The sequence markers, indexable by name. Read-only."""
        return NamedList(self._markers)

    @property
    def caption_tracks(self) -> list[CaptionTrack]:
        """The sequence's caption tracks. Read-only.

        Empty unless captions have been imported: every sequence carries the
        data track group they live on, but Premiere only puts tracks in it
        when there are captions to hold.
        """
        return self._caption_tracks

    def add_marker(
        self,
        name: str,
        start: Time,
        comments: str = "",
        marker_type: str = "Comment",
        duration: Time | None = None,
    ) -> Marker:
        """Create a marker on this sequence and return it.

        Works whether or not the sequence already has markers: a
        marker-free sequence gets a fresh `MarkerOwner` and `Markers`
        collection synthesized first.
        """
        marker = Marker(name, start, comments, marker_type, duration)
        document = self.project._document
        inner = self._ensure_marker_list(document)
        _attach_marker(document, inner, marker)
        self._markers.append(marker)
        return marker

    def remove_marker(self, marker: Marker) -> None:
        """Remove a marker from this sequence.

        Removing the last one takes the whole marker collection with it, as
        ExtendScript's `deleteMarker` does: a sequence with no markers
        carries no `MarkerOwner` at all.
        """
        if marker not in self._markers:
            raise ValueError("marker does not belong to this sequence")
        document = self.project._document
        ref = self._element.find("MarkerOwner/Markers")
        if ref is None:
            raise ValueError("sequence has no marker list")
        collection = document.resolve(ref)
        inner = collection.find("Markers")
        if inner is None:
            raise ValueError("marker collection has no inner list")
        _detach_marker(document, inner, marker)
        self._markers.remove(marker)
        if not self._markers:
            self._remove_marker_collection(document, collection)

    def _remove_marker_collection(
        self, document: PremiereDocument, collection: ET.Element
    ) -> None:
        owner = self._element.find("MarkerOwner")
        if owner is not None:
            remove_child(self._element, owner)
        document.remove_object(collection)

    def _ensure_marker_list(self, document: PremiereDocument) -> ET.Element:
        # The inner `<Markers Version="1">` pair list, creating the
        # collection object and the sequence's MarkerOwner if absent.
        ref = self._element.find("MarkerOwner/Markers")
        if ref is not None:
            collection = document.resolve(ref)
        else:
            collection = ET.Element(
                "Markers",
                {"ClassID": _MARKERS_CLASS_ID, "Version": "4"},
            )
            collection.text = "\n\t\t"
            inner = ET.SubElement(collection, "Markers", {"Version": "1"})
            inner.tail = "\n\t\t"
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
            owner.text = "\n\t\t\t"
            ET.SubElement(
                owner, "Markers", {"ObjectRef": collection_id}
            ).tail = "\n\t\t"
            insert_before(self._element, "PersistentGroupContainer", owner)
        marker_list = collection.find("Markers")
        if marker_list is None:
            raise ValueError("marker collection has no inner list")
        return marker_list

    @property
    def settings(self) -> SequenceSettings:
        """The stored sequence settings. Read-only."""
        return SequenceSettings(self)

    @property
    def sequence_id(self) -> str | None:
        """The sequence's persistent identifier. Read-only."""
        return self._element.get("ObjectUID")

    def _grow_source_duration(self) -> None:
        """Raise the length this sequence reports when USED as a source.

        A sequence that anything can play from - nested in another
        timeline, or just sitting in the panel - caches its length on its
        `Video`/`AudioSequenceSource` objects, and an edit that pushes the
        last clip out has to carry that cache with it.

        The cache only ever GROWS. Premiere leaves it alone when the
        content shrinks: `templates/537`'s `Logo` sequence is empty in the
        file yet still reports 5228 s, and `Visualizer_Logo` reports
        4265 s over 2561 s of content. So this raises the value to cover
        the content and never lowers it.
        """
        tracks = self._video_tracks + self._audio_tracks
        content = max(
            [clip.end.ticks for track in tracks for clip in track.clips] or [0]
        )
        if not content:
            return
        uid = self.sequence_id
        for element in self.project._document.root:
            if not element.tag.endswith("SequenceSource"):
                continue
            reference = element.find("SequenceSource/Sequence")
            if reference is None or reference.get("ObjectURef") != uid:
                continue
            duration = element.find("OriginalDuration")
            if duration is not None and int(duration.text or 0) < content:
                duration.text = str(content)

    @property
    def project_item(self) -> ProjectItem | None:
        """The project-panel item backing this sequence. Read-only."""
        uid = self.sequence_id
        if uid is None:
            return None
        return self.project._item_by_sequence_uid(uid)

    @property
    def timebase(self) -> int | None:
        """Ticks per video frame. Read-only."""
        return self._timebase

    @property
    def frame_size(self) -> tuple[int, int] | None:
        """`(width, height)` from the video track group's `FrameRect`. Read-only."""
        return self._frame_size

    @property
    def audio_frame_rate(self) -> int | None:
        """Ticks per audio frame. Read-only."""
        return self._audio_frame_rate

    @property
    def audio_channel_count(self) -> int | None:
        """Audio channel count. Read-only."""
        return self._audio_channel_count

    def _bag_element(self, key: str) -> ET.Element | None:
        return self._element.find(f"Node/Properties/{key}")

    def _bag_int(self, key: str) -> int | None:
        element = self._bag_element(key)
        if element is None or not element.text:
            return None
        return int(element.text)

    def _bag_write(self, key: str, value: int | str) -> None:
        element = self._bag_element(key)
        if element is not None:
            element.text = str(value)
            return
        if key not in _CREATABLE_BAG_KEYS:
            raise ValueError(f"sequence has no {key} property")
        properties = self._element.find("Node/Properties")
        if properties is None:
            raise ValueError("sequence has no property bag")
        # Insert ahead of the first key that follows this one in the stored
        # order, so the bag keeps Premiere's layout whichever keys are present.
        anchor = None
        for tag in _BAG_ORDER[_BAG_ORDER.index(key) + 1 :]:
            if properties.find(tag) is not None:
                anchor = tag
                break
        if anchor is not None:
            insert_leaf_before(properties, anchor, key, str(value))
        else:
            append_leaf(properties, key, str(value))

    @property
    def video_display_format(self) -> int | None:
        """The video time display format (e.g. 100 = 24 fps timecode). Read/write."""
        return self._bag_int("MZ.Sequence.VideoTimeDisplayFormat")

    @video_display_format.setter
    def video_display_format(self, value: int) -> None:
        validate_int(value)
        self._bag_write("MZ.Sequence.VideoTimeDisplayFormat", value)

    @property
    def audio_display_format(self) -> int | None:
        """The audio time display format (200 = audio samples). Read/write."""
        return self._bag_int("MZ.Sequence.AudioTimeDisplayFormat")

    @audio_display_format.setter
    def audio_display_format(self, value: int) -> None:
        validate_int(value)
        self._bag_write("MZ.Sequence.AudioTimeDisplayFormat", value)

    @property
    def end(self) -> Time:
        """The end of the sequence content (the latest track item end). Read-only.

        Premiere derives this at runtime; it is not stored.
        """
        ticks = 0
        for track in self._video_tracks + self._audio_tracks:
            for clip in track.clips:
                ticks = max(ticks, clip.end.ticks)
        return Time(ticks)

    @property
    def zero_point(self) -> Time:
        """The sequence's starting timecode. Read/write."""
        return Time(self._bag_int("MZ.ZeroPoint") or 0)

    @zero_point.setter
    def zero_point(self, value: Time) -> None:
        validate_time(value)
        self._bag_write("MZ.ZeroPoint", value.ticks)

    @property
    def in_point(self) -> Time | None:
        """The sequence in point, or `None` when never set. Read/write.

        ExtendScript reports the -400000 s unset sentinel instead of
        `None`. Setting `None` writes that sentinel, as Premiere does when
        the in point is cleared.
        """
        ticks = self._bag_int("MZ.InPoint")
        if ticks is None or ticks == UNSET_TICKS:
            return None
        return Time(ticks)

    @in_point.setter
    def in_point(self, value: Time | None) -> None:
        if value is not None:
            validate_time(value)
        self._bag_write("MZ.InPoint", UNSET_TICKS if value is None else value.ticks)

    @property
    def out_point(self) -> Time | None:
        """The sequence out point, or `None` when never set. Read/write.

        `None` behaves as in `in_point`.
        """
        ticks = self._bag_int("MZ.OutPoint")
        if ticks is None or ticks == UNSET_TICKS:
            return None
        return Time(ticks)

    @out_point.setter
    def out_point(self, value: Time | None) -> None:
        if value is not None:
            validate_time(value)
        self._bag_write("MZ.OutPoint", UNSET_TICKS if value is None else value.ticks)

    @property
    def work_area_in(self) -> Time:
        """Where the work area starts. Read/write.

        `MZ.WorkInPoint`. The work area is the render/export range, separate
        from the in and out points; no scripting API exposes it, so the
        fixture came from the QE DOM (`setWorkInOutPoints`).
        """
        return Time(self._bag_int("MZ.WorkInPoint") or 0)

    @work_area_in.setter
    def work_area_in(self, value: Time) -> None:
        validate_time(value)
        self._bag_write("MZ.WorkInPoint", value.ticks)

    @property
    def work_area_out(self) -> Time:
        """Where the work area ends. Read/write (`MZ.WorkOutPoint`).

        A virgin sequence stores 60 seconds here - floored to a whole frame -
        rather than its own end, so an untouched work area covers everything
        in a short sequence.
        """
        return Time(self._bag_int("MZ.WorkOutPoint") or 0)

    @work_area_out.setter
    def work_area_out(self, value: Time) -> None:
        validate_time(value)
        self._bag_write("MZ.WorkOutPoint", value.ticks)

    @property
    def playhead(self) -> Time:
        """Where the playhead sits. Read/write.

        `MZ.EditLine` - the CTI, saved with the project so a reopened
        sequence lands where it was left.
        """
        return Time(self._bag_int("MZ.EditLine") or 0)

    @playhead.setter
    def playhead(self, value: Time) -> None:
        validate_time(value)
        self._bag_write("MZ.EditLine", value.ticks)

    def add_linked_clip(
        self,
        item: ProjectItem,
        start: Time | None = None,
        video_track: int = 0,
        audio_track: int = 0,
    ) -> tuple[TrackItem, TrackItem]:
        """Place both halves of an A/V item and link them.

        ExtendScript's per-track `insertClip` takes video and audio track
        indices and drops a LINKED pair; py's `Track.add_clip` places one
        half on one track, so this is the cross-track form. Returns
        `(video, audio)`.

        The link is what makes the two move, trim and delete together in
        Premiere - the same `Link` object a merged clip uses to bind its
        halves. An item with only one stream has nothing to link and is
        refused.
        """
        if start is None:
            start = Time(0)
        validate_time(start)
        validate_positive_int(video_track)
        validate_positive_int(audio_track)
        videos, audios = self.video_tracks, self.audio_tracks
        if video_track >= len(videos):
            raise ValueError(f"sequence has no video track {video_track}")
        if audio_track >= len(audios):
            raise ValueError(f"sequence has no audio track {audio_track}")
        tags = {element.tag for element in item._clip_elements}
        if not {"VideoClip", "AudioClip"} <= tags:
            raise ValueError("item has no video and audio pair to link")

        video = videos[video_track].add_clip(item, start)
        audio = audios[audio_track].add_clip(item, start)
        self._link([video, audio])
        return video, audio

    def _link(self, items: list[TrackItem]) -> None:
        # Premiere keeps links in the sequence's own group container: a
        # `Link` object listing the track items, referenced from
        # `PersistentGroupContainer/LinkContainer/Links`. A sequence that
        # has never had one carries the container but no `Links` list.
        document = self.project._document
        link_id = document.add_object(
            build_link([element._element.get("ObjectID") or "" for element in items])
        )
        container = self._element.find("PersistentGroupContainer/LinkContainer")
        if container is None:
            raise ValueError("sequence has no LinkContainer")
        links = container.find("Links")
        if links is None:
            links = ET.Element("Links", {"Version": "1"})
            append_child(container, links)
        entry = ET.Element(
            "Link",
            {"Index": str(len(links.findall("Link"))), "ObjectRef": link_id},
        )
        # `Links` sits four tabs in, its entries five.
        append_child(links, entry, "\n\t\t\t\t")

    @property
    def video_tracks(self) -> NamedList[Track]:
        """The video tracks, indexable by name (`Video 1`). Read-only."""
        return NamedList(self._video_tracks)

    @property
    def audio_tracks(self) -> NamedList[Track]:
        """The audio tracks, indexable by name (`Audio 1`). Read-only."""
        return NamedList(self._audio_tracks)

    def import_mgt(
        self,
        path: str | Path,
        start: Time | None = None,
        video_track_offset: int = 0,
    ) -> TrackItem:
        """Import a Motion Graphics template onto a video track.

        ExtendScript's `Sequence.importMGT`, minus its audio track offset -
        templates carrying audio are unsupported, so there is nothing to
        offset. The work is `Track.add_mgt`'s.
        """
        validate_positive_int(video_track_offset)
        tracks = self.video_tracks
        if video_track_offset >= len(tracks):
            raise ValueError(f"sequence has no video track {video_track_offset}")
        return tracks[video_track_offset].add_mgt(path, start)

    def create_caption_track(
        self,
        item: ProjectItem,
        caption_format: CaptionFormat = CaptionFormat.SUBTITLE,
    ) -> CaptionTrack:
        """Place an imported caption item on a new timeline caption track.

        Mirrors ExtendScript's `Sequence.createCaptionTrack` (29_captions):
        each cue becomes a `CaptionDataClipTrackItem` frame-snapped to this
        sequence's timebase, backed by its own synthetic 12-hour caption
        source whose in point sits at the 1-hour mark, hash-referencing
        the panel item's styled-text payloads.

        `caption_format` defaults to `SUBTITLE`, as the ExtendScript
        parameter does; the track's `format` is writable afterwards too.
        """
        _validate_caption_format(caption_format)
        clip_elements = item._clip_elements
        segments_ref = (
            clip_elements[0].find("TranscriptTextSegments") if clip_elements else None
        )
        if segments_ref is None:
            raise ValueError("item is not an imported caption file")
        timebase = self.timebase
        if timebase is None:
            raise ValueError("sequence has no video timebase to snap captions to")
        document = self.project._document
        collection = document.resolve(segments_ref)

        group_element = None
        for pair in self._element.findall("TrackGroups/TrackGroup"):
            group_ref = pair.find("Second")
            if pair.findtext("First") == DATA_MEDIA_TYPE and group_ref is not None:
                group_element = document.resolve(group_ref)
                break
        if group_element is None:
            raise ValueError("sequence has no data track group")
        inner = group_element.find("TrackGroup")
        next_id = None if inner is None else inner.find("NextTrackID")
        if inner is None or next_id is None:
            raise ValueError("data track group has no NextTrackID")
        track_id = int(next_id.text or "1")

        label_color = clip_elements[0].findtext(
            "DataClip/Clip/Node/Properties/asl.clip.label.color"
        )
        label_name = clip_elements[0].findtext(
            "DataClip/Clip/Node/Properties/asl.clip.label.name"
        )
        label_index = int((label_name or "BE.Prefs.LabelColors.7").rsplit(".", 1)[1])

        base_frames = 3600 * TICKS_PER_SECOND // timebase
        item_ids = []
        for entry in collection.findall("CaptionMap/CaptionMapItem"):
            caption_ref = entry.find("Second")
            if caption_ref is None:
                continue
            caption = document.resolve(caption_ref)
            start = int(caption.findtext("TimeStart") or 0)
            end = int(caption.findtext("TimeEnd") or 0)
            start_frames = int(start / timebase + 0.5)
            end_frames = int(end / timebase + 0.5)
            block_ref = caption.find("BlockVector/BlockVectorItem")
            if block_ref is None:
                raise ValueError("caption carries no text block")
            block_data = document.resolve(block_ref).find("FormattedTextData")
            block_hash = None if block_data is None else block_data.get("BinaryHash")

            stream_id = document.add_object(new_synthetic_stream(timebase))
            media_uid = str(uuid.uuid4())
            media = new_synthetic_media(
                media_uid,
                stream_id,
                document.payload_stored(SYNTHETIC_STATE[0]),
            )
            document.attach_object(media)
            source_id = document.add_object(
                new_data_media_source(media_uid, SYNTHETIC_DURATION)
            )
            logging_id = document.add_object(new_empty_logging_info())
            template_id = document.add_object(
                new_synthetic_template_clip(source_id, str(uuid.uuid4()))
            )
            groups_id = document.add_object(new_channel_groups())
            master_uid = str(uuid.uuid4())
            master = new_master_clip(
                master_uid, logging_id, template_id, groups_id, "SyntheticCaption"
            )
            master.set("ObjectUID", master_uid)
            document.attach_object(master)

            timeline_clip_id = document.add_object(
                new_timeline_caption_clip(
                    source_id,
                    (base_frames + start_frames) * timebase,
                    (base_frames + end_frames) * timebase,
                    str(uuid.uuid4()),
                    label_index,
                    int(label_color or "277129"),
                )
            )
            subclip_id = document.add_object(
                new_caption_subclip(timeline_clip_id, master_uid)
            )
            chain_id = document.add_object(new_data_chain())
            block_id = document.add_object(new_timeline_block(block_hash or ""))
            item_ids.append(
                document.add_object(
                    new_caption_track_item(
                        chain_id,
                        subclip_id,
                        block_id,
                        start_frames * timebase,
                        end_frames * timebase,
                    )
                )
            )

        track_uid = str(uuid.uuid4())
        track = new_caption_track(
            track_uid,
            track_id,
            item_ids,
            document.payload_stored(TEMPLATE_STYLE[0]),
            int(caption_format),
        )
        track.set("ObjectUID", track_uid)
        document.attach_object(track)
        document._by_binary_hash = None

        tracks = inner.find("Tracks")
        if tracks is None:
            tracks = ET.Element("Tracks", {"Version": "1"})
            tracks.text = "\n\t\t\t"
            insert_before(inner, "FrameRate", tracks)
        reference = ET.SubElement(
            tracks, "Track", {"Index": str(len(list(tracks))), "ObjectURef": track_uid}
        )
        reference.tail = "\n\t\t"
        next_id.text = str(track_id + 1)

        # Model -> parser is a proven circular dependency; parsers import
        # the models package at module load.
        from ..parsers.caption import parse_caption_tracks

        self._caption_tracks = parse_caption_tracks(document, self, group_element)
        return self._caption_tracks[-1]

    @property
    def clips(self) -> NamedList[TrackItem]:
        """Every clip in the sequence, indexable by name. Read-only.

        Flattens the tracks: video tracks first, then audio, each in
        timeline order.
        """
        items: list[TrackItem] = []
        for track in self._video_tracks:
            items.extend(track._clips)
        for track in self._audio_tracks:
            items.extend(track._clips)
        return NamedList(items)

    def __repr__(self) -> str:
        return (
            f"Sequence(name={self.name!r}, "
            f"{len(self._video_tracks)}V/{len(self._audio_tracks)}A)"
        )
