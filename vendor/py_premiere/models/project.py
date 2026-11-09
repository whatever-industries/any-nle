"""The `Project` model."""

from __future__ import annotations

import base64
import os
import uuid
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import TYPE_CHECKING, NamedTuple

from ..enums import ProjectItemType
from ..xml.mutations import append_leaf, insert_leaf_before, remove_child
from .caption_builder import (
    CAPTION_FRAME_RATE,
    CAPTION_TIMECODE_FORMAT,
    CAPTIONS_LABEL_COLOR,
    CAPTIONS_LABEL_INDEX,
    build_caption_payload,
    new_block,
    new_caption,
    new_caption_collection,
    new_data_media,
    new_data_media_source,
    new_data_stream,
    new_transcript_clip,
    parse_srt,
)
from .media_paths import relative_media_path
from .media_import import (
    AAC_SAMPLE_TYPE,
    AUDIO_FORMATS,
    AUDIO_LABEL_COLOR,
    AUDIO_LABEL_INDEX,
    AV_LABEL_COLOR,
    AV_LABEL_INDEX,
    CHANNEL_TYPES,
    COMPRESSED_SAMPLE_TYPES,
    MONO_LAYOUT,
    MOVIE_CONTAINERS,
    MOVIE_SUFFIXES,
    PCM_SAMPLE_TYPES,
    STILL_CODECS,
    STILL_DEFAULT_OUT,
    STILL_DURATION,
    STILL_FRAME_RATE,
    STILL_LABEL_COLOR,
    STILL_LABEL_INDEX,
    TIMECODE_FORMATS,
    VIDEO_LABEL_COLOR,
    VIDEO_LABEL_INDEX,
    MovieAudioInfo,
    _indexed_refs,
    audio_channel_layout,
    new_audio_channel_groups,
    new_audio_master_clip,
    new_audio_media,
    new_audio_media_source,
    new_audio_stream,
    new_audio_template_clip,
    new_channel_groups,
    new_channel_serializer,
    new_channel_vector,
    new_clip_item,
    new_logging_info,
    new_markers_collection,
    new_master_audio_chain,
    new_master_clip,
    new_media,
    new_media_logging_info,
    new_media_source,
    new_movie_media,
    new_movie_media_source,
    new_movie_template_clip,
    new_movie_video_stream,
    new_secondary_content,
    new_template_clip,
    new_video_stream,
    read_audio_info,
    read_avi_info,
    read_bmp_size,
    read_gif_size,
    read_jpeg_size,
    read_mp4_info,
    read_mxf_info,
    read_png_size,
    read_psd_size,
    read_tiff_size,
)
from .merged_builder import (
    build_audio_group,
    build_audio_track,
    build_data_group,
    build_empty_video_chain,
    build_fader,
    build_fader_chain,
    build_inlet,
    build_link,
    build_master_bag,
    build_merged_sequence,
    build_meter,
    build_mix_track,
    build_mute_param,
    build_pan_processor,
    build_panel_audio_template,
    build_panel_item,
    build_panel_logging,
    build_panel_master,
    build_panel_video_template,
    build_sequence_source,
    build_video_group,
    build_video_track,
    build_volume_param,
)
from .mgt_builder import MEDIA_FOLDER, build_media_bin, build_template_item
from .multicam_builder import build_multicam, build_processed_clips_bin
from .named_list import NamedList
from .preferences import Preferences
from .project_item import (
    ProjectItem,
    _add_item_ref,
    _check_proxy_aspect,
    _child_items,
    _detach_item_ref,
    item_container,
)
from .sequence_builder import DEFAULT_PRESET, FORMATS, build_sequence
from .time import TICKS_PER_SECOND
from .publication import publish_new
from .validators import validate_path, validate_string, validate_track_count

if TYPE_CHECKING:
    from typing import Callable

    from ..xml import PremiereDocument
    from .sequence import Sequence
    from .track_item import TrackItem

_validate_save_path = validate_path(must_exist=False)
_validate_media_path = validate_path(must_exist=True, must_be_file=True)
_validate_sequence_name = validate_string(allow_empty=False)
_validate_item_name = validate_string(allow_empty=False)


class _MovieProbe(NamedTuple):
    """What the movie readers yield, shared by import and proxy synthesis."""

    width: int
    height: int
    frame_rate: int
    duration: int
    container: str
    codec: str
    audio: MovieAudioInfo | None
    field_order: int
    clip_id: str
    start_ticks: int
    timecode_format: int


def _probe_movie(path: Path) -> _MovieProbe:
    data = path.read_bytes()
    audio = None
    start_ticks = 0
    drop_frame = False
    field_order = 0
    clip_id = ""
    suffix = path.suffix.lower()
    container = MOVIE_CONTAINERS[suffix]
    if suffix == ".avi":
        width, height, fps, frames, codec = read_avi_info(data)
        frame_rate = TICKS_PER_SECOND // fps
    else:
        movie = read_mxf_info(data) if suffix == ".mxf" else read_mp4_info(data)
        if TICKS_PER_SECOND * movie.frame_duration % movie.timescale:
            raise NotImplementedError(
                f"frame rate {movie.timescale}/{movie.frame_duration} is not a whole number of ticks"
            )
        frame_rate = TICKS_PER_SECOND * movie.frame_duration // movie.timescale
        width, height, frames = movie.width, movie.height, movie.frames
        codec = movie.codec
        audio = movie.audio
        drop_frame = movie.drop_frame
        field_order = movie.field_order
        clip_id = movie.clip_id
        # A timecode track shifts the media in/out and materializes the
        # AlternateStart pair; its frames are counted at the video rate.
        start_ticks = movie.start_timecode * frame_rate
    timecode_format = TIMECODE_FORMATS.get((frame_rate, drop_frame))
    if timecode_format is None:
        raise NotImplementedError(
            "no verified timecode format for a {}-tick {}frame".format(
                frame_rate, "drop-" if drop_frame else ""
            )
        )
    return _MovieProbe(
        width,
        height,
        frame_rate,
        frames * frame_rate,
        container,
        codec,
        audio,
        field_order,
        clip_id,
        start_ticks,
        timecode_format,
    )


class Project:
    """A Premiere Pro project."""

    def __init__(self, _document: PremiereDocument, path: Path) -> None:
        self._document = _document
        self._path = path
        self._root_item: ProjectItem | None = None
        self._sequences: list[Sequence] = []
        self._active_sequence: Sequence | None = None
        self._items_by_master_uid: dict[str, ProjectItem] | None = None
        self._items_by_sequence_uid: dict[str, ProjectItem] | None = None
        #: The bin `import_files` is currently importing into, if any.
        self._import_target: ProjectItem | None = None
        #: Overrides the machine-profile discovery of `Preferences` when set
        #: (threaded from `parse(preferences_path=...)` / `new(...)`).
        self._preferences_path: Path | None = None
        #: `Media` objects synthesized by `import_files`, whose RelativePath
        #: is re-derived at save time against the destination directory.
        self._imported_media: list[ET.Element] = []

    @property
    def name(self) -> str:
        """The project file name. Read-only."""
        return self._path.name

    @property
    def path(self) -> Path:
        """The project file path. Read-only."""
        return self._path

    @property
    def root_item(self) -> ProjectItem | None:
        """The root of the item tree. Read-only."""
        return self._root_item

    @property
    def document_id(self) -> str | None:
        """The project's persistent identifier. Read-only."""
        if self._root_item is None:
            return None
        return self._root_item._element.get("ObjectUID")

    @property
    def active_sequence(self) -> Sequence | None:
        """The frontmost sequence, if any. Read-only.

        Best-effort: Premiere stores no dedicated key; this derives from
        the open-sequence list and can differ from the session state.
        """
        return self._active_sequence

    @property
    def sequences(self) -> NamedList[Sequence]:
        """The project's sequences, indexable by name. Read-only."""
        return NamedList(self._sequences)

    @staticmethod
    def sequence_presets() -> list[str]:
        """The preset names accepted by `add_sequence`. Read-only."""
        return sorted(FORMATS)

    def _item_by_master_uid(self, uid: str) -> ProjectItem | None:
        # Maps a MasterClip's persistent UID to the owning project item,
        # so a track item can resolve its source item. Built once per parse.
        if self._items_by_master_uid is None:
            mapping: dict[str, ProjectItem] = {}

            def visit(item: ProjectItem) -> None:
                master = item._master_element
                if master is not None:
                    master_uid = master.get("ObjectUID")
                    if master_uid is not None:
                        mapping[master_uid] = item
                for child in item.children:
                    visit(child)

            if self._root_item is not None:
                visit(self._root_item)
            self._items_by_master_uid = mapping
        return self._items_by_master_uid.get(uid)

    def _item_by_sequence_uid(self, uid: str) -> ProjectItem | None:
        # Maps a sequence's UID to its project-panel item.
        if self._items_by_sequence_uid is None:
            mapping: dict[str, ProjectItem] = {}

            def visit(item: ProjectItem) -> None:
                if item._sequence_uid is not None:
                    mapping[item._sequence_uid] = item
                for child in item.children:
                    visit(child)

            if self._root_item is not None:
                visit(self._root_item)
            self._items_by_sequence_uid = mapping
        return self._items_by_sequence_uid.get(uid)

    def import_files(
        self, paths: list[str | Path], target_bin: ProjectItem | None = None
    ) -> list[ProjectItem]:
        """Import media files into the project panel.

        Synthesizes the same object graph Premiere writes for a fresh
        import. Supported so far: BMP/PNG/JPEG/GIF/TIFF/PSD stills; audio -
        WAV (16/24-bit PCM or 32-bit float, any channel count), AIFF, M4A and
        WMA; and video - uncompressed AVI, MJPEG-in-AVI,
        H.264/H.265/ProRes/DNxHR in MP4 and MOV (with or without an AAC
        audio track), MPEG-2 in MXF, and MP3/AAC. The content-state
        hashes Premiere stamps are change-detection caches, so py writes a
        fresh GUID and Premiere refreshes it on open. Premiere's
        auto-transcript bootstrap objects and its machine-local audio
        conform-cache paths are elided (regenerated on open).

        `target_bin` is ExtendScript's `importFiles` parameter of the same
        name: the bin the new items land in, defaulting to the panel root.
        """
        if target_bin is not None:
            if target_bin._type is not ProjectItemType.BIN:
                raise ValueError("target_bin must be a bin")
            if target_bin.project is not self:
                # The objects land in THIS document while the item reference
                # is written into the other one, so both come out broken: the
                # import is unreachable here and the donor gains a ref to an
                # ObjectUID it does not contain.
                raise ValueError("target_bin belongs to another project")
        # Import defaults come from the machine's own Premiere preferences
        # (matching what the user's Premiere would write); the factory
        # constants cover machines without one (e.g. CI). Read once per call:
        # locating and parsing the prefs file is not cheap.
        if self._preferences_path is not None:
            preferences: Preferences | None = Preferences(self._preferences_path)
        else:
            preferences = Preferences.load_default()
        items = []
        previous, self._import_target = self._import_target, target_bin
        try:
            for path in paths:
                path = Path(path)
                suffix = path.suffix.lower()
                if suffix in AUDIO_FORMATS:
                    items.append(self._import_audio(path, preferences))
                elif suffix in MOVIE_SUFFIXES:
                    items.append(self._import_video(path, preferences))
                elif suffix == ".srt":
                    items.append(self._import_caption(path, preferences))
                else:
                    items.append(self._import_still(path, preferences))
        finally:
            self._import_target = previous
        return items

    def _import_caption(
        self, path: Path, preferences: Preferences | None
    ) -> ProjectItem:
        # An SRT lands as a TranscriptClip-backed panel item whose
        # CaptionCollection holds one Caption per cue; the styled-text
        # payloads are template patches (29_captions).
        _validate_media_path(path)
        cues = parse_srt(path.read_text(encoding="utf-8-sig"))
        label_index, label_color = self._label(
            preferences, "Captions", CAPTIONS_LABEL_INDEX, CAPTIONS_LABEL_COLOR
        )
        # The stream's duration is the last cue end, on whole caption-rate
        # frames (2 s in the fixture; exact there, so the rounding
        # direction is py's own choice - up, to cover the final cue).
        last_end = max(cue.end for cue in cues)
        frames = -(-last_end // CAPTION_FRAME_RATE)
        duration = frames * CAPTION_FRAME_RATE
        name = path.name
        file_path = str(path.resolve())
        project_dir = str(self._path.resolve().parent)
        content_state = str(uuid.uuid4())
        stream_id = self._register(new_data_stream(duration))
        media_uid = str(uuid.uuid4())
        self._imported_media.append(
            self._register_uid(
                new_data_media(
                    media_uid, stream_id, file_path, project_dir, content_state
                ),
                media_uid,
            )
        )
        source_id = self._register(new_data_media_source(media_uid, duration))
        markers_id = self._register(new_markers_collection(content_state))
        logging_id = self._register(
            new_media_logging_info(
                name,
                CAPTION_FRAME_RATE,
                duration,
                # Caption logging records no capture mode (like A/V media).
                None,
                CAPTION_TIMECODE_FORMAT,
                0,
            )
        )
        entries = []
        for cue in cues:
            payload = base64.b64encode(build_caption_payload(cue.text)).decode("ascii")
            block_id = self._register(new_block(payload, str(uuid.uuid4())))
            entries.append(
                (cue.end, self._register(new_caption(block_id, cue.start, cue.end)))
            )
        collection_id = self._register(new_caption_collection(entries))
        clip_id = self._register(
            new_transcript_clip(
                markers_id,
                source_id,
                collection_id,
                label_index,
                label_color,
                str(uuid.uuid4()),
            )
        )
        groups_id = self._register(new_channel_groups())
        master_uid = str(uuid.uuid4())
        master = self._register_uid(
            new_master_clip(master_uid, logging_id, clip_id, groups_id, name),
            master_uid,
        )
        item_uid = str(uuid.uuid4())
        item_element = self._register_uid(
            new_clip_item(item_uid, master_uid, name, label_index), item_uid
        )
        return self._attach_imported_item(
            item_element, item_uid, master, [clip_id], file_path, duration
        )

    def _register(self, element: ET.Element) -> str:
        """Add a new object to the top-level table under a fresh ObjectID."""
        return self._document.add_object(element)

    def _register_uid(self, element: ET.Element, uid: str) -> ET.Element:
        """Add a new object to the top-level table under its own ObjectUID."""
        element.set("ObjectUID", uid)
        return self._document.attach_object(element)

    def _root_bin(self, name: str, build: Callable[[], ET.Element]) -> ProjectItem:
        """A root-level bin of this name, created if it is not there yet.

        Premiere reuses the bins its own features file things into rather
        than adding a numbered second one (79_two_multicams).
        """
        root_item = self.root_item
        if root_item is None:
            raise ValueError("project has no root item")
        container = root_item._element.find("ProjectItemContainer")
        if container is None:
            raise ValueError("root item has no ProjectItemContainer")
        existing = next(
            (
                child
                for child in root_item._children
                if child._type is ProjectItemType.BIN and child.name == name
            ),
            None,
        )
        if existing is not None:
            return existing
        bin_uid = str(uuid.uuid4())
        element = build()
        element.set("ObjectUID", bin_uid)
        self._document.attach_object(element)
        bin_item = ProjectItem(element, self, ProjectItemType.BIN)
        bin_item._parent = root_item
        _add_item_ref(_child_items(container), bin_uid)
        root_item._children.append(bin_item)
        return bin_item

    def _add_template_item(self, master_uid: str, name: str) -> ProjectItem:
        """File an imported Motion Graphics template into its media bin."""
        bin_item = self._root_bin(MEDIA_FOLDER, build_media_bin)
        container = item_container(bin_item._element)
        if container is None:
            raise ValueError("bin has no ProjectItemContainer")
        item_uid = str(uuid.uuid4())
        element = build_template_item(item_uid, master_uid, name)
        self._document.attach_object(element)
        _add_item_ref(_child_items(container), item_uid)
        item = ProjectItem(element, self, ProjectItemType.CLIP)
        item._parent = bin_item
        master = self._document.by_object_uid[master_uid]
        item._master_element = master
        item._clip_elements = [
            self._document.resolve(reference)
            for reference in master.findall("Clips/Clip")
        ]
        bin_item._children.append(item)
        self._items_by_master_uid = None
        return item

    @staticmethod
    def _label(
        preferences: Preferences | None,
        kind: str,
        default_index: int,
        default_color: int,
    ) -> tuple[int, int]:
        # The label a fresh import of this media kind takes, from the local
        # preferences when there are any.
        if preferences is None:
            return default_index, default_color
        index = preferences.label_default(kind)
        if index is None:
            return default_index, default_color
        return index, preferences.label_colors[index]

    def _attach_imported_item(
        self,
        item_element: ET.Element,
        item_uid: str,
        master: ET.Element,
        clip_ids: list[str],
        file_path: str,
        default_out_ticks: int,
    ) -> ProjectItem:
        # Wire a freshly synthesized clip object graph into the panel, under
        # the bin `import_files` was pointed at (the root by default).
        # `is None`, NOT `or`: a ProjectItem is sized by its children, so an
        # EMPTY bin is falsy and `or` would silently import into the root -
        # which is exactly the bin a caller has just created.
        parent = self._import_target
        if parent is None:
            parent = self.root_item
        if parent is None:
            raise ValueError("project has no root item")
        # The STRICT lookup, not `item_container`: that one reaches into a
        # smart bin's nested container, whose contents Premiere regenerates
        # from the bin's query - an item filed there would simply vanish.
        container = parent._element.find("ProjectItemContainer")
        if container is None:
            raise ValueError("target item has no ProjectItemContainer")
        _add_item_ref(_child_items(container), item_uid)
        item = ProjectItem(item_element, self, ProjectItemType.CLIP)
        item._parent = parent
        item._master_element = master
        item._media_path = Path(file_path)
        item._clip_elements = [
            self._document.by_object_id[clip_id] for clip_id in clip_ids
        ]
        item._default_out_ticks = default_out_ticks
        parent._children.append(item)
        self._items_by_master_uid = None
        return item

    def _splice_fragment(self, fragment: ET.Element) -> dict[str, ET.Element]:
        # Attach a builder fragment's objects under fresh identifiers: every
        # ObjectID is renumbered from the document's counter, every ObjectUID
        # reminted, and in-fragment references follow; foreign references
        # (real ids injected by the builder's caller) pass through untouched.
        # Returns the spliced elements keyed by tag (last one wins - the
        # fragments spliced here carry at most one of each tag a caller asks
        # for).
        document = self._document
        id_map: dict[str, str] = {}
        uid_map: dict[str, str] = {}
        next_id = int(document.next_object_id())
        for element in fragment:
            object_id = element.get("ObjectID")
            if object_id is not None:
                id_map[object_id] = str(next_id)
                next_id += 1
            object_uid = element.get("ObjectUID")
            if object_uid is not None:
                uid_map[object_uid] = str(uuid.uuid4())

        def rewrite(node: ET.Element, scoped: bool) -> None:
            for child in node:
                child_scoped = scoped or (
                    child.get("ObjectID") is not None
                    or child.get("ObjectUID") is not None
                )
                if not child_scoped:
                    reference = child.get("ObjectRef")
                    if reference in id_map:
                        child.set("ObjectRef", id_map[reference])
                    reference = child.get("ObjectURef")
                    if reference in uid_map:
                        child.set("ObjectURef", uid_map[reference])
                rewrite(child, child_scoped)

        spliced: dict[str, ET.Element] = {}
        for element in list(fragment):
            object_id = element.get("ObjectID")
            if object_id is not None:
                element.set("ObjectID", id_map[object_id])
            object_uid = element.get("ObjectUID")
            if object_uid is not None:
                element.set("ObjectUID", uid_map[object_uid])
            rewrite(element, False)
            document.attach_object(element)
            spliced[element.tag] = element
        return spliced

    def add_sequence(
        self,
        name: str,
        preset: str = DEFAULT_PRESET,
        video_tracks: int | None = None,
    ) -> Sequence:
        """Create a new sequence and return it.

        Builds the object graph Premiere writes for a new sequence - its
        tracks plus the audio mix graph behind them - and splices it in with
        fresh identifiers. `preset` selects the format; see
        `sequence_presets()` for the names (default 1080p 23.976 fps).

        The track layout follows the preset: 3 video tracks throughout, and
        4 stereo audio tracks except on the mono-discrete broadcast presets,
        where `broadcast8mono` builds 8.

        `video_tracks` overrides the preset's video track count. Every preset
        Adobe ships asks for 3, so a different count is only reachable this
        way; it is checked against sequences Premiere built from `.sqpreset`
        files asking for 1 and 5.

        A sequence Premiere derives from a clip
        (`createNewSequenceFromClips`) has 3 audio tracks instead; only the
        default layout is built here.
        """
        _validate_sequence_name(name)
        if video_tracks is not None:
            validate_track_count(video_tracks)
        document = self._document
        fragment = build_sequence(name, preset, video_tracks=video_tracks)
        spliced = self._splice_fragment(fragment)
        sequence_element = spliced.get("Sequence")
        item_element = spliced.get("ClipProjectItem")
        master_element = spliced.get("MasterClip")
        if sequence_element is None or item_element is None or master_element is None:
            raise ValueError("sequence template is missing its core objects")

        root_item = self.root_item
        if root_item is None:
            raise ValueError("project has no root item")
        container = root_item._element.find("ProjectItemContainer")
        if container is None:
            raise ValueError("root item has no ProjectItemContainer")
        item_uid = item_element.get("ObjectUID")
        if item_uid is None:
            raise ValueError("template panel item has no ObjectUID")
        _add_item_ref(_child_items(container), item_uid)

        # Model -> parser is a proven circular dependency; parsers import
        # the models package at module load.
        from ..parsers.sequence import parse_sequence

        sequence = parse_sequence(document, self, sequence_element)
        self._sequences.append(sequence)
        item = ProjectItem(item_element, self, ProjectItemType.CLIP)
        item._parent = root_item
        item._master_element = master_element
        item._clip_elements = [
            document.resolve(ref) for ref in master_element.findall("Clips/Clip")
        ]
        item._sequence_uid = sequence_element.get("ObjectUID")
        root_item._children.append(item)
        self._items_by_master_uid = None
        self._items_by_sequence_uid = None
        return sequence

    def create_merged_clip(
        self,
        video_item: ProjectItem,
        audio_item: ProjectItem,
        name: str | None = None,
    ) -> ProjectItem:
        """Merge a video clip with an audio clip and return the panel item.

        Premiere has no scripting API for `Merge Clips`; this synthesizes
        the graph its UI writes (23_merged_clip): a hidden sequence flagged
        `BE.Sequence.IsMergedClip` holding PRIVATE COPIES of both source
        graphs on one video and one audio track, a `Link` binding the two
        placements, and a panel item whose master plays that sequence
        through `Video/AudioSequenceSource`. The sources are left
        untouched; the copies are synthesized the way an import is (the
        media files must still be readable) and then take over the sources'
        file identity, like subclips do.

        `name` defaults to Premiere's own: the video clip's name plus
        `" - Merged"`. The fixture-verified shapes are supported: a
        video-only movie plus one mono or stereo audio clip; the audio
        lands as one MONO track per channel, the way Premiere splits it
        (77_merged_stereo).
        """
        if name is not None:
            _validate_item_name(name)
        video_path = video_item.media_path
        audio_path = audio_item.media_path
        if video_path is None or audio_path is None:
            raise ValueError("item has no media file to merge")
        for item, wanted in ((video_item, "VideoClip"), (audio_item, "AudioClip")):
            if item._type is not ProjectItemType.CLIP or item.is_sequence:
                raise ValueError("merged clips are built from media clip items")
            if [element.tag for element in item._clip_elements] != [wanted]:
                raise ValueError(
                    "only a video-only movie plus an audio-only clip can be merged"
                )
        layout = audio_item._clip_elements[0].findtext("AudioChannelLayout")
        if layout == MONO_LAYOUT:
            channels = 1
        elif layout == audio_channel_layout(2):
            channels = 2
        else:
            raise ValueError("only mono or stereo audio is supported in a merged clip")
        document = self._document
        merged_name = name if name is not None else f"{video_item.name} - Merged"

        # Private copies of both source graphs, renamed and stamped the way
        # Premiere's merge does. The MergeClipUtils bag keeps each source's
        # original master name; the duplicated masters and their logging
        # take the merged name.
        video_copy = self.import_files([video_path])[0]
        audio_copy = self.import_files([audio_path])[0]
        copies = (
            (
                video_item,
                video_copy,
                [
                    (
                        "MZ.MergeClipUtils.ComponentMasterClipOriginalName",
                        video_item.name,
                    ),
                ],
            ),
            (
                audio_item,
                audio_copy,
                [
                    ("MZ.MergeClipUtils.AudioTrackNumberFromOriginalMergedClip", "0"),
                    (
                        "MZ.MergeClipUtils.ComponentMasterClipOriginalName",
                        audio_item.name,
                    ),
                ],
            ),
        )
        for source, copy, bag in copies:
            master = copy._master_element
            if master is None:
                raise ValueError("synthesized copy has no master clip")
            master.insert(0, build_master_bag(bag))
            name_element = master.find("Name")
            if name_element is not None:
                name_element.text = merged_name
            logging_ref = master.find("LoggingInfo")
            if logging_ref is not None:
                clip_name = document.resolve(logging_ref).find("ClipName")
                if clip_name is not None:
                    clip_name.text = merged_name
            # A fresh audio import carries a DefMappingID; the duplicated
            # master inside a merged clip does not (23_merged_clip).
            mapping = master.find("DefMappingID")
            if mapping is not None:
                remove_child(master, mapping)
            source._share_file_identity(copy)

        if channels > 1:
            # Premiere's merge splits the audio into per-channel MONO groups
            # over the ONE copied source (77_merged_stereo): the copy keeps
            # its stereo stream and template clip, but its master carries a
            # bag-less mono chain and a single-channel vector per channel
            # instead of the import's stereo pair.
            audio_master = audio_copy._master_element
            if audio_master is None:
                raise ValueError("synthesized copy has no master clip")
            chains_list = audio_master.find("AudioComponentChains")
            groups_ref = audio_master.find("AudioClipChannelGroups")
            if chains_list is None or groups_ref is None:
                raise ValueError("audio copy has no channel plumbing")
            for entry in chains_list.findall("AudioComponentChain"):
                document.remove_object(document.resolve(entry))
            for entry in list(chains_list):
                chains_list.remove(entry)
            _indexed_refs(
                chains_list,
                "AudioComponentChain",
                [self._register(new_master_audio_chain(1, 0)) for _ in range(channels)],
            )
            vectors_list = document.resolve(groups_ref).find("ClipChannelVectors")
            if vectors_list is None:
                raise ValueError("audio copy has no channel vectors")
            for entry in vectors_list.findall("ClipChannelVectorItem"):
                vector = document.resolve(entry)
                for channel_entry in vector.findall("ClipChannels/ClipChannelItem"):
                    document.remove_object(document.resolve(channel_entry))
                document.remove_object(vector)
            for entry in list(vectors_list):
                vectors_list.remove(entry)
            mono_vector_ids = []
            for channel in range(channels):
                serializer_id = self._register(new_channel_serializer(0, channel))
                mono_vector_ids.append(
                    self._register(new_channel_vector([serializer_id], 0))
                )
            _indexed_refs(vectors_list, "ClipChannelVectorItem", mono_vector_ids)

        video_media = video_copy._media_element()
        audio_media = audio_copy._media_element()
        if video_media is None or audio_media is None:
            raise ValueError("synthesized copy has no media")
        video_stream_ref = video_media.find("VideoStream")
        audio_stream_ref = audio_media.find("AudioStream")
        if video_stream_ref is None or audio_stream_ref is None:
            raise ValueError("synthesized copy has no media stream")
        video_stream = document.resolve(video_stream_ref)
        audio_stream = document.resolve(audio_stream_ref)
        timebase = int(video_stream.findtext("FrameRate") or 0)
        audio_rate = int(audio_stream.findtext("FrameRate") or 0)
        video_duration = int(video_stream.findtext("Duration") or 0)
        audio_duration = int(audio_stream.findtext("Duration") or 0)
        rect = (video_stream.findtext("FrameRect") or "0,0,0,0").split(",")
        width, height = int(rect[2]), int(rect[3])
        # Both placements land on whole video frames (`add_clip` floors
        # them); the sequence spans the longer one.
        end_ticks = max(
            video_duration - video_duration % timebase,
            audio_duration - audio_duration % timebase,
        )
        display_format = TIMECODE_FORMATS[(timebase, False)]

        # The hidden sequence: one video track, one audio track per source
        # channel, the audio mix graph behind them, and the merged-clip
        # property bag.
        sequence_uid = str(uuid.uuid4())
        video_track_uid = str(uuid.uuid4())
        audio_track_uids = [str(uuid.uuid4()) for _ in range(channels)]
        group_chain_id = self._register(build_empty_video_chain())
        track_chain_ids = []
        track_pan_ids = []
        for _ in range(channels):
            volume_id = self._register(build_volume_param())
            mute_id = self._register(build_mute_param())
            fader_id = self._register(build_fader(audio_rate, [volume_id, mute_id]))
            meter_id = self._register(build_meter(audio_rate))
            track_chain_ids.append(
                self._register(build_fader_chain(fader_id, meter_id))
            )
            track_pan_ids.append(self._register(build_pan_processor(audio_rate)))
        mix_volume_id = self._register(build_volume_param())
        mix_mute_id = self._register(build_mute_param())
        mix_fader_id = self._register(
            build_fader(audio_rate, [mix_volume_id, mix_mute_id])
        )
        mix_meter_id = self._register(build_meter(audio_rate))
        mix_chain_id = self._register(build_fader_chain(mix_fader_id, mix_meter_id))
        mix_pan_id = self._register(build_pan_processor(audio_rate))
        inlet_id = self._register(build_inlet(audio_track_uids))
        mix_track_id = self._register(
            build_mix_track(mix_chain_id, mix_pan_id, inlet_id)
        )
        video_group_id = self._register(
            build_video_group(video_track_uid, timebase, width, height, group_chain_id)
        )
        audio_group_id = self._register(
            build_audio_group(audio_track_uids, audio_rate, mix_track_id)
        )
        data_group_id = self._register(build_data_group(timebase))
        document.attach_object(build_video_track(video_track_uid))
        for index, track_uid in enumerate(audio_track_uids):
            document.attach_object(
                build_audio_track(
                    track_uid,
                    track_chain_ids[index],
                    track_pan_ids[index],
                    track_id=2 + index,
                    index=index,
                )
            )
        sequence_element = document.attach_object(
            build_merged_sequence(
                sequence_uid,
                merged_name,
                display_format,
                end_ticks,
                width,
                height,
                video_group_id,
                audio_group_id,
                data_group_id,
            )
        )

        # Model -> parser is a proven circular dependency; parsers import
        # the models package at module load.
        from ..parsers.sequence import parse_sequence

        sequence = parse_sequence(document, self, sequence_element)
        self._sequences.append(sequence)
        video_placed = sequence._video_tracks[0].add_clip(video_copy)
        audio_placed_ids = []
        for channel, track in enumerate(sequence._audio_tracks):
            placed = track.add_clip(audio_copy)
            self._narrow_merged_channel(placed, channel)
            audio_placed_ids.append(placed._element.get("ObjectID") or "")

        link_id = self._register(
            build_link([video_placed._element.get("ObjectID") or "", *audio_placed_ids])
        )
        link_entry = sequence_element.find(
            "PersistentGroupContainer/LinkContainer/Links/Link"
        )
        if link_entry is None:
            raise ValueError("merged sequence has no link entry")
        link_entry.set("ObjectRef", link_id)

        # The panel-facing graph: the master's template clips play the
        # hidden sequence through Video/AudioSequenceSource.
        video_source_id = self._register(
            build_sequence_source("VideoSequenceSource", sequence_uid, end_ticks)
        )
        audio_source_id = self._register(
            build_sequence_source("AudioSequenceSource", sequence_uid, end_ticks)
        )
        secondary_id = self._register(new_secondary_content(audio_source_id, 0))
        # One mono chain and one single-channel vector per source channel;
        # the panel-side vectors index the hidden sequence's audio TRACKS
        # (source clip i, channel 0), unlike the copy's (source 0,
        # channel i) - both shapes straight from 77_merged_stereo.
        panel_vector_ids = []
        for index in range(channels):
            channel_id = self._register(new_channel_serializer(index, 0))
            panel_vector_ids.append(self._register(new_channel_vector([channel_id], 0)))
        groups_id = self._register(new_audio_channel_groups(panel_vector_ids))
        master_chain_ids = [
            self._register(new_master_audio_chain(1, 0)) for _ in range(channels)
        ]
        audio_template_id = self._register(
            build_panel_audio_template(audio_source_id, secondary_id)
        )
        video_template_id = self._register(build_panel_video_template(video_source_id))
        logging_id = self._register(
            build_panel_logging(video_item.name, display_format, end_ticks, timebase)
        )
        master_uid = str(uuid.uuid4())
        master_element = document.attach_object(
            build_panel_master(
                master_uid,
                logging_id,
                master_chain_ids,
                audio_template_id,
                video_template_id,
                groups_id,
                merged_name,
            )
        )
        item_uid = str(uuid.uuid4())
        item_element = document.attach_object(
            build_panel_item(item_uid, master_uid, merged_name)
        )

        root_item = self.root_item
        if root_item is None:
            raise ValueError("project has no root item")
        container = root_item._element.find("ProjectItemContainer")
        if container is None:
            raise ValueError("root item has no ProjectItemContainer")
        # The duplicated source graphs stay, but their panel items go: the
        # copies are reachable only through the hidden sequence.
        for copy in (video_copy, audio_copy):
            _detach_item_ref(container, copy._element.get("ObjectUID") or "")
            document.remove_object(copy._element)
            root_item._children.remove(copy)
        _add_item_ref(_child_items(container), item_uid)
        item = ProjectItem(item_element, self, ProjectItemType.CLIP)
        item._parent = root_item
        item._master_element = master_element
        item._clip_elements = [
            document.by_object_id[audio_template_id],
            document.by_object_id[video_template_id],
        ]
        item._default_out_ticks = end_ticks
        item._sequence_uid = sequence_uid
        root_item._children.append(item)
        self._items_by_master_uid = None
        self._items_by_sequence_uid = None
        return item

    def _narrow_merged_channel(self, placed: TrackItem, channel: int) -> None:
        # A merged placement plays ONE mono channel of the copied source
        # (23_merged_clip, 77_merged_stereo): mono layout, a single
        # SecondaryContent for its channel, the channel group selected
        # through the SubClip's OrigChGrp, a SecondaryIndex marker past the
        # first channel, and the bag-less DefaultVol chain instead of the
        # MZ.ActiveComponent-bagged chain of a normal timeline placement.
        document = self._document
        clip = placed._clip_element
        layout_element = clip.find("AudioChannelLayout")
        if layout_element is not None:
            layout_element.text = MONO_LAYOUT
        contents = clip.find("SecondaryContents")
        if contents is not None:
            kept = None
            for entry in contents.findall("SecondaryContentItem"):
                secondary = document.resolve(entry)
                index_text = secondary.findtext("ChannelIndex") or "0"
                if kept is None and int(index_text) == channel:
                    kept = entry
                else:
                    document.remove_object(secondary)
                    remove_child(contents, entry)
            if kept is not None:
                kept.set("Index", "0")
        chain_ref = placed._element.find("ClipTrackItem/ComponentOwner/Components")
        if chain_ref is not None:
            chain_inner = document.resolve(chain_ref).find("ComponentChain")
            node = None if chain_inner is None else chain_inner.find("Node")
            if chain_inner is not None and node is not None:
                chain_inner.remove(node)
                chain_inner.text = "\n\t\t"
        if channel:
            append_leaf(clip, "SecondaryIndex", str(channel))
            orig_group = placed._subclip_element.find("OrigChGrp")
            if orig_group is not None:
                orig_group.text = str(channel)

    def create_multicam_clip(
        self,
        items: list[ProjectItem],
        name: str | None = None,
    ) -> ProjectItem:
        """Build a multicam source clip from angle items and return it.

        Premiere has no scripting API for `Create Multi-Camera Source
        Sequence`; this synthesizes the graph its UI writes (24_multicam):
        a hidden sequence with one video track per angle and a 32-channel
        adaptive audio bus, a master flagged
        `Source.Monitor.Multicam.Enabled`, and the source items filed into
        a new `Processed Clips` bin. Unlike a merged clip nothing is
        copied - the placements play the ORIGINAL source graphs.

        `items` are the angles in track order. `name` defaults to
        Premiere's own: the first angle's name plus `Multicam`. The
        fixture-verified shapes are supported: two or more angles of which
        exactly one carries (stereo) audio - either an AV movie among the
        video angles (its halves come out linked, 24_multicam) or an
        audio-only clip (audio track only, no link, 79_two_multicams).
        Cameras are synchronized at their starts (in-point sync) and every
        angle keeps its full length (78_multicam_3angle).
        """
        if name is not None:
            _validate_item_name(name)
        if len(items) < 2:
            raise ValueError("a multicam clip needs at least two angles")
        audio_angle: ProjectItem | None = None
        video_items = []
        for item in items:
            if item._type is not ProjectItemType.CLIP or item.is_sequence:
                raise ValueError("multicam angles must be media clip items")
            if item.media_path is None:
                raise ValueError("angle has no media file")
            tags = sorted(element.tag for element in item._clip_elements)
            if tags == ["VideoClip"]:
                video_items.append(item)
                continue
            if audio_angle is not None:
                raise ValueError("only one angle may carry audio")
            if tags == ["AudioClip", "VideoClip"]:
                audio_angle = item
                video_items.append(item)
            elif tags == ["AudioClip"]:
                audio_angle = item
            else:
                raise ValueError("multicam angles must be movie or audio clips")
        if audio_angle is None:
            raise ValueError("one angle must carry the audio track")
        if not video_items:
            raise ValueError("a multicam clip needs at least one video angle")
        audio_template = next(
            element
            for element in audio_angle._clip_elements
            if element.tag == "AudioClip"
        )
        if audio_template.findtext("AudioChannelLayout") != audio_channel_layout(2):
            raise ValueError("only a stereo audio angle is supported")
        audio_is_linked = audio_angle in video_items
        document = self._document
        multicam_name = name if name is not None else f"{items[0].name}Multicam"

        # Geometry and display format follow the first video angle; the
        # sequence sources span the longest angle (whole video frames, as
        # the placements land - 78's short angle keeps its own length).
        timebase = 0
        width = height = 0
        end_ticks = 0
        for index, item in enumerate(video_items):
            media = item._media_element()
            stream_ref = None if media is None else media.find("VideoStream")
            if stream_ref is None:
                raise ValueError("angle has no video stream")
            stream = document.resolve(stream_ref)
            rate = int(stream.findtext("FrameRate") or 0)
            duration = int(stream.findtext("Duration") or 0)
            if index == 0:
                timebase = rate
                rect = (stream.findtext("FrameRect") or "0,0,0,0").split(",")
                width, height = int(rect[2]), int(rect[3])
            end_ticks = max(end_ticks, duration - duration % timebase)
        display_format = TIMECODE_FORMATS[(timebase, False)]

        fragment = build_multicam(
            multicam_name,
            timebase,
            width,
            height,
            display_format,
            end_ticks,
            len(video_items),
            link=audio_is_linked,
        )
        spliced = self._splice_fragment(fragment)
        sequence_element = spliced.get("Sequence")
        item_element = spliced.get("ClipProjectItem")
        master_element = spliced.get("MasterClip")
        if sequence_element is None or item_element is None or master_element is None:
            raise ValueError("multicam fragment is missing its core objects")

        # Model -> parser is a proven circular dependency; parsers import
        # the models package at module load.
        from ..parsers.sequence import parse_sequence

        sequence = parse_sequence(document, self, sequence_element)
        self._sequences.append(sequence)
        av_video_placed: TrackItem | None = None
        for index, angle in enumerate(video_items):
            placed = sequence._video_tracks[index].add_clip(angle)
            if angle is audio_angle:
                av_video_placed = placed
        audio_placed = sequence._audio_tracks[0].add_clip(audio_angle)
        # The multicam placement's audio chain is the bag-less stereo
        # DefaultVol shape (24_multicam), not the MZ.ActiveComponent-bagged
        # mono chain of a normal timeline placement.
        chain_ref = audio_placed._element.find(
            "ClipTrackItem/ComponentOwner/Components"
        )
        if chain_ref is not None:
            chain = document.resolve(chain_ref)
            chain_inner = chain.find("ComponentChain")
            node = None if chain_inner is None else chain_inner.find("Node")
            if chain_inner is not None and node is not None:
                chain_inner.remove(node)
                chain_inner.text = "\n\t\t"
            insert_leaf_before(
                chain, "ComponentChain", "DefaultChannelVolumeComponentID", "2"
            )
            append_leaf(chain, "AudioChannelLayout", audio_channel_layout(2))
            append_leaf(chain, "ChannelType", "1")

        if av_video_placed is not None:
            # The Link binds the audio placement to its own angle's video
            # placement, audio first (24_multicam); an audio-only angle has
            # no video half, and Premiere writes no Link at all.
            link_id = self._register(
                build_link(
                    [
                        audio_placed._element.get("ObjectID") or "",
                        av_video_placed._element.get("ObjectID") or "",
                    ]
                )
            )
            link_entry = sequence_element.find(
                "PersistentGroupContainer/LinkContainer/Links/Link"
            )
            if link_entry is None:
                raise ValueError("multicam sequence has no link entry")
            link_entry.set("ObjectRef", link_id)

        # File the source angles into the `Processed Clips` bin, as
        # Premiere does - reusing an existing root-level one
        # (79_two_multicams) or creating it.
        root_item = self.root_item
        if root_item is None:
            raise ValueError("project has no root item")
        root_container = root_item._element.find("ProjectItemContainer")
        if root_container is None:
            raise ValueError("root item has no ProjectItemContainer")
        bin_item = self._root_bin("Processed Clips", build_processed_clips_bin)
        bin_container = item_container(bin_item._element)
        if bin_container is None:
            raise ValueError("bin has no ProjectItemContainer")
        for angle in items:
            if angle._parent is bin_item:
                continue
            parent = angle._parent
            container = None if parent is None else item_container(parent._element)
            angle_uid = angle._element.get("ObjectUID") or ""
            if parent is not None and container is not None:
                _detach_item_ref(container, angle_uid)
                parent._children.remove(angle)
            _add_item_ref(_child_items(bin_container), angle_uid)
            angle._parent = bin_item
            bin_item._children.append(angle)

        item_uid = item_element.get("ObjectUID") or ""
        _add_item_ref(_child_items(root_container), item_uid)
        item = ProjectItem(item_element, self, ProjectItemType.CLIP)
        item._parent = root_item
        item._master_element = master_element
        item._clip_elements = [
            document.resolve(ref) for ref in master_element.findall("Clips/Clip")
        ]
        item._default_out_ticks = end_ticks
        item._sequence_uid = sequence_element.get("ObjectUID")
        root_item._children.append(item)
        self._items_by_master_uid = None
        self._items_by_sequence_uid = None
        return item

    def _import_video(self, path: Path, preferences: Preferences | None) -> ProjectItem:
        _validate_media_path(path)
        (
            width,
            height,
            frame_rate,
            duration,
            container,
            codec,
            audio,
            field_order,
            clip_id,
            start_ticks,
            timecode_format,
        ) = _probe_movie(path)
        if audio is None:
            label_index, label_color = self._label(
                preferences, "Video", VIDEO_LABEL_INDEX, VIDEO_LABEL_COLOR
            )
        else:
            label_index, label_color = self._label(
                preferences, "AV", AV_LABEL_INDEX, AV_LABEL_COLOR
            )

        name = path.name
        file_path = str(path.resolve())
        project_dir = str(self._path.resolve().parent)
        content_state = str(uuid.uuid4())
        stream_id = self._register(
            new_movie_video_stream(
                width, height, frame_rate, duration, container, codec, field_order
            )
        )
        audio_stream_id = None
        audio_rate = None
        if audio is not None:
            audio_rate = TICKS_PER_SECOND // audio.sample_rate
            # Premiere gives the audio stream the MEDIA duration, not the
            # audio track's own (which an encoder can pad or clip).
            audio_stream_id = self._register(
                new_audio_stream(
                    audio_rate,
                    audio_channel_layout(audio.channels),
                    duration,
                    AAC_SAMPLE_TYPE,
                )
            )
        media_uid = str(uuid.uuid4())
        self._imported_media.append(
            self._register_uid(
                new_movie_media(
                    media_uid,
                    stream_id,
                    file_path,
                    project_dir,
                    content_state,
                    audio_stream_id,
                    audio_rate,
                    start_ticks,
                ),
                media_uid,
            )
        )
        source_id = self._register(new_movie_media_source(media_uid, duration))
        markers_id = self._register(new_markers_collection(content_state))
        logging_id = self._register(
            new_media_logging_info(
                name,
                frame_rate,
                duration,
                # Media carrying both streams records no capture mode.
                "2" if audio is None else None,
                str(timecode_format),
                start_ticks,
                clip_id,
            )
        )
        clip_ids = [
            self._register(
                new_movie_template_clip(markers_id, source_id, label_index, label_color)
            )
        ]
        master_uid = str(uuid.uuid4())
        if audio is None:
            groups_id = self._register(new_channel_groups())
            master_element = new_master_clip(
                master_uid, logging_id, clip_ids[0], groups_id, name
            )
        else:
            # The audio half hangs off the SAME Media: its own source, clip
            # and channel group, sharing the item's markers collection.
            channel_type = CHANNEL_TYPES.get(audio.channels)
            if channel_type is None:
                raise NotImplementedError(
                    f"{audio.channels}-channel movie audio is not supported"
                )
            audio_source_id = self._register(
                new_audio_media_source(media_uid, duration)
            )
            secondary_ids = [
                self._register(new_secondary_content(audio_source_id, channel))
                for channel in range(audio.channels)
            ]
            layout = audio_channel_layout(audio.channels)
            clip_ids.append(
                self._register(
                    new_audio_template_clip(
                        markers_id,
                        audio_source_id,
                        secondary_ids,
                        label_index,
                        label_color,
                        layout,
                    )
                )
            )
            channel_ids = [
                self._register(new_channel_serializer(0, channel))
                for channel in range(audio.channels)
            ]
            vector_id = self._register(new_channel_vector(channel_ids, channel_type))
            groups_id = self._register(new_audio_channel_groups([vector_id]))
            chain_id = self._register(
                new_master_audio_chain(audio.channels, channel_type)
            )
            master_element = new_audio_master_clip(
                master_uid, logging_id, [chain_id], clip_ids, groups_id, name
            )
        master = self._register_uid(master_element, master_uid)
        item_uid = str(uuid.uuid4())
        item_element = self._register_uid(
            new_clip_item(item_uid, master_uid, name, label_index), item_uid
        )
        return self._attach_imported_item(
            item_element, item_uid, master, clip_ids, file_path, duration
        )

    def _make_primary_media(self, path: Path, own_rect: str) -> tuple[str, str]:
        """Synthesize a plain (non-proxy) `Media` + `VideoStream` pair.

        Returns `(uid, frame rect)`. Used when a hi-res attach replaces
        what the item plays: the newcomer becomes the media, so it carries
        no `IsProxy` and no frame-rect override. `own_rect` is the
        incumbent's raster, which the newcomer's aspect has to match.
        """
        probe = _probe_movie(path)
        if probe.audio is not None:
            raise NotImplementedError(
                "audio-carrying replacement media is not supported (no reference)"
            )
        rect = f"0,0,{probe.width},{probe.height}"
        # Refuse BEFORE registering anything, the way `_make_proxy_media`
        # does - a rejected attach must leave the project untouched.
        _check_proxy_aspect(own_rect, rect)
        stream = new_movie_video_stream(
            probe.width,
            probe.height,
            probe.frame_rate,
            probe.duration,
            probe.container,
            probe.codec,
            probe.field_order,
        )
        stream_id = self._register(stream)
        media_uid = str(uuid.uuid4())
        media = new_movie_media(
            media_uid,
            stream_id,
            str(path.resolve()),
            str(self._path.resolve().parent),
            str(uuid.uuid4()),
            None,
            None,
            probe.start_ticks,
        )
        self._imported_media.append(self._register_uid(media, media_uid))
        return media_uid, rect

    def _make_proxy_media(self, path: Path, hires_rect: str) -> str:
        """Synthesize a proxy's `Media` + `VideoStream` pair; returns the UID.

        The stream mirrors an import of the proxy file, plus the HI-RES
        frame rect carried as an override so the item keeps reporting the
        original raster; the `Media` gains `IsProxy` as its last child
        (18_proxy). Premiere refuses a proxy whose frame aspect ratio
        differs from the source's, and so does py.
        """
        probe = _probe_movie(path)
        if probe.audio is not None:
            raise NotImplementedError(
                "audio-carrying proxy media is not supported (no reference)"
            )
        parts = hires_rect.split(",")
        hires_width, hires_height = int(parts[2]), int(parts[3])
        if probe.width * hires_height != hires_width * probe.height:
            raise ValueError(
                "proxy frame aspect ratio must match the source "
                f"({probe.width}x{probe.height} vs {hires_width}x{hires_height})"
            )
        stream = new_movie_video_stream(
            probe.width,
            probe.height,
            probe.frame_rate,
            probe.duration,
            probe.container,
            probe.codec,
            probe.field_order,
        )
        # The override pair slots after OriginalColorSpace (before AlphaType
        # where the codec profile writes one).
        children = [child.tag for child in stream]
        anchor = children.index("OriginalColorSpace")
        if anchor + 1 < len(children):
            following = children[anchor + 1]
            insert_leaf_before(stream, following, "IsFrameRectOverridden", "true")
            insert_leaf_before(stream, following, "OverriddenFrameRect", hires_rect)
        else:
            append_leaf(stream, "IsFrameRectOverridden", "true")
            append_leaf(stream, "OverriddenFrameRect", hires_rect)
        stream_id = self._register(stream)
        media_uid = str(uuid.uuid4())
        media = new_movie_media(
            media_uid,
            stream_id,
            str(path.resolve()),
            str(self._path.resolve().parent),
            str(uuid.uuid4()),
            None,
            None,
            probe.start_ticks,
        )
        append_leaf(media, "IsProxy", "true")
        self._imported_media.append(self._register_uid(media, media_uid))
        return media_uid

    def _import_audio(self, path: Path, preferences: Preferences | None) -> ProjectItem:
        _validate_media_path(path)
        suffix = path.suffix.lower()
        data = path.read_bytes()
        info = read_audio_info(data, suffix)
        if suffix in COMPRESSED_SAMPLE_TYPES:
            # A compressed codec's sample type is fixed by what it decodes to.
            sample_type: str | None = COMPRESSED_SAMPLE_TYPES[suffix]
        else:
            sample_types = PCM_SAMPLE_TYPES[suffix]
            sample_format = (info.format_tag, info.sample_width)
            if sample_format not in sample_types:
                raise NotImplementedError(
                    f"unsupported {suffix} sample format: tag {info.format_tag}, "
                    f"{info.sample_width * 8}-bit"
                )
            sample_type = sample_types[sample_format]
        frame_rate = TICKS_PER_SECOND // info.sample_rate
        duration = info.frames * frame_rate
        label_index, label_color = self._label(
            preferences, "Audio", AUDIO_LABEL_INDEX, AUDIO_LABEL_COLOR
        )

        # Mono, stereo and 5.1 are native channel types, carried by ONE source
        # clip. Premiere imports any other channel count as one MONO source
        # clip per channel - a 4-channel file lands four full media graphs -
        # so both the source clips and the channel groups are lists here.
        # Each group entry maps a channel to its (source clip, channel).
        channel_type = CHANNEL_TYPES.get(info.channels)
        if channel_type is None:
            clip_channel_counts = [1] * info.channels
            groups = [
                (CHANNEL_TYPES[1], [(index, 0)]) for index in range(info.channels)
            ]
        else:
            clip_channel_counts = [info.channels]
            groups = [
                (channel_type, [(0, channel) for channel in range(info.channels)])
            ]

        name = path.name
        file_path = str(path.resolve())
        project_dir = str(self._path.resolve().parent)
        content_state = str(uuid.uuid4())
        # One file identity shared by every Media object describing this file.
        file_key = str(uuid.uuid4())
        binary_hash = str(uuid.uuid4())
        logging_id = self._register(
            new_media_logging_info(name, frame_rate, duration, "1", "200", 0)
        )
        chain_ids = []
        clip_ids = []
        for stream_number, channels in enumerate(clip_channel_counts):
            layout = audio_channel_layout(channels)
            stream_id = self._register(
                new_audio_stream(frame_rate, layout, duration, sample_type)
            )
            media_uid = str(uuid.uuid4())
            self._imported_media.append(
                self._register_uid(
                    new_audio_media(
                        media_uid,
                        stream_id,
                        file_path,
                        project_dir,
                        content_state,
                        frame_rate,
                        file_key,
                        binary_hash,
                        stream_number,
                    ),
                    media_uid,
                )
            )
            source_id = self._register(new_audio_media_source(media_uid, duration))
            secondary_ids = [
                self._register(new_secondary_content(source_id, channel))
                for channel in range(channels)
            ]
            markers_id = self._register(new_markers_collection(content_state))
            chain_ids.append(
                self._register(
                    new_master_audio_chain(channels, CHANNEL_TYPES[channels])
                )
            )
            clip_ids.append(
                self._register(
                    new_audio_template_clip(
                        markers_id,
                        source_id,
                        secondary_ids,
                        label_index,
                        label_color,
                        layout,
                    )
                )
            )
        vector_ids = []
        for group_type, group_channels in groups:
            channel_ids = [
                self._register(new_channel_serializer(source_clip, channel))
                for source_clip, channel in group_channels
            ]
            vector_ids.append(
                self._register(new_channel_vector(channel_ids, group_type))
            )
        groups_id = self._register(new_audio_channel_groups(vector_ids))
        master_uid = str(uuid.uuid4())
        master = self._register_uid(
            new_audio_master_clip(
                master_uid, logging_id, chain_ids, clip_ids, groups_id, name
            ),
            master_uid,
        )
        item_uid = str(uuid.uuid4())
        item_element = self._register_uid(
            new_clip_item(item_uid, master_uid, name, label_index), item_uid
        )
        return self._attach_imported_item(
            item_element, item_uid, master, clip_ids, file_path, duration
        )

    def _import_still(self, path: Path, preferences: Preferences | None) -> ProjectItem:
        _validate_media_path(path)
        suffix = path.suffix.lower()
        if suffix not in STILL_CODECS:
            raise NotImplementedError(
                "supported stills are " + ", ".join(sorted(STILL_CODECS))
            )
        profile = STILL_CODECS[suffix]
        data = path.read_bytes()
        if suffix == ".bmp":
            width, height = read_bmp_size(data)
        elif suffix == ".jpg":
            width, height = read_jpeg_size(data)
        elif suffix == ".gif":
            width, height = read_gif_size(data)
        elif suffix == ".psd":
            width, height = read_psd_size(data)
        elif suffix == ".tif":
            width, height = read_tiff_size(data)
        else:
            width, height = read_png_size(data)
        frame_rate = STILL_FRAME_RATE
        out_ticks = STILL_DEFAULT_OUT
        if preferences is not None:
            frame_rate = preferences.still_frame_rate or frame_rate
            out_ticks = preferences.still_default_out_ticks or out_ticks
        label_index, label_color = self._label(
            preferences, "Still", STILL_LABEL_INDEX, STILL_LABEL_COLOR
        )

        name = path.name
        file_path = str(path.resolve())
        content_state = str(uuid.uuid4())
        stream_id = self._register(new_video_stream(width, height, frame_rate, profile))
        media_uid = str(uuid.uuid4())
        self._imported_media.append(
            self._register_uid(
                new_media(
                    media_uid,
                    stream_id,
                    file_path,
                    str(self._path.resolve().parent),
                    content_state,
                ),
                media_uid,
            )
        )
        source_id = self._register(new_media_source(media_uid))
        markers_id = self._register(new_markers_collection(content_state))
        logging_id = self._register(new_logging_info(name, frame_rate))
        groups_id = self._register(new_channel_groups())
        clip_id = self._register(
            new_template_clip(
                markers_id, source_id, label_index, label_color, out_ticks
            )
        )
        master_uid = str(uuid.uuid4())
        master = self._register_uid(
            new_master_clip(master_uid, logging_id, clip_id, groups_id, name),
            master_uid,
        )
        item_uid = str(uuid.uuid4())
        item_element = self._register_uid(
            new_clip_item(item_uid, master_uid, name, label_index), item_uid
        )
        return self._attach_imported_item(
            item_element, item_uid, master, [clip_id], file_path, STILL_DURATION
        )

    def save(self, path: str | Path) -> None:
        """Save the project to a new file.

        Like ExtendScript's `saveAs`, the project then reports the new
        `name` and `path`. Media imported in this session has its stored
        relative path re-derived against the destination directory, the way
        Premiere recomputes it on every save.

        Refuses to overwrite an existing file (`FileExistsError`). The
        write uses an exclusively created temporary sibling, then publishes
        without replacement. Occupied staging paths are preserved.
        """
        _validate_save_path(path)
        self._write(Path(path), replace=False)

    def save_in_place(self) -> None:
        """Overwrite the file this project was parsed from.

        ExtendScript's `save()`, as distinct from its `saveAs(path)` -
        which is what `save(path)` is here, and which refuses to
        overwrite. This one overwrites deliberately, so it is a separate
        call rather than a flag.
        """
        if not self._path.exists():
            raise FileNotFoundError(
                f"project has no saved file to overwrite: {self._path}"
            )
        self._write(self._path, replace=True)

    def take_save_warnings(self) -> list[str]:
        result = getattr(self, "_save_warnings", [])
        self._save_warnings = []
        return result

    def _write(self, target: Path, *, replace: bool) -> None:
        self._save_warnings = []
        self._rewrite_relative_paths(target.resolve().parent)
        data = self._document.to_bytes()
        number = 1
        while True:
            suffix = ".tmp" if number == 1 else f".tmp.{number}"
            temporary = target.with_name(target.name + suffix)
            try:
                file = temporary.open("xb")
            except FileExistsError:
                number += 1
                continue
            break
        try:
            with file:
                file.write(data)
            if replace:
                os.replace(temporary, target)
            else:
                warning = publish_new(temporary, target)
                if warning:
                    self._save_warnings.append(warning)
        except BaseException as error:
            # Only the sibling we exclusively created; keep the original failure if cleanup fails.
            try:
                temporary.unlink()
            except OSError as cleanup:
                detail = f"retained project staging file {temporary}: cleanup failed: {cleanup}"
                if isinstance(error, OSError):
                    error.strerror = f"{error.strerror}; {detail}"
                else:
                    error.args = (f"{error}; {detail}",)
            raise
        self._path = target

    def _rewrite_relative_paths(self, directory: Path) -> None:
        # Only media THIS session imported: an untouched parsed document must
        # still save byte-identically, so nothing already in the file is
        # rewritten (Premiere refreshes those itself on open).
        for media in self._imported_media:
            relative = media.find("RelativePath")
            actual = media.findtext("FilePath")
            if relative is None or not actual:
                continue
            relative.text = relative_media_path(actual, str(directory))

    def __repr__(self) -> str:
        return f"Project(name={self.name!r}, {len(self._sequences)} sequence(s))"
