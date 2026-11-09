//! Graph operations (Rust port Phase 3): py_premiere's object model as handles over the arena.
//!
//! Python's model objects (`Project`, `ProjectItem`, `Sequence`, `Track`, `TrackItem`) are
//! wrappers that share ElementTree elements. Here one [`Project`] owns the
//! [`PremiereDocument`] plus small tables of model records; callers hold copyable handles
//! ([`ItemId`], [`SequenceId`], [`TrackId`], [`TrackItemId`]) and call methods on the project.
//! Every method that mints a UUID takes the one `&mut dyn UuidSource` of the write and draws in
//! exactly Python's order.
//!
//! | Python | Rust |
//! |---|---|
//! | `py_premiere.new()` | [`Project::new`] |
//! | `parse_application` / `parse_project` / `parse_sequence` | [`Project::from_document`] |
//! | `Project._register`, `_register_uid`, `_splice_fragment` | [`Project::register`], [`Project::register_uid`], [`Project::splice`] |
//! | `Project._attach_imported_item`, `_import_target`, `_imported_media` | [`Project::attach_imported_item`], [`Project::import_target`], [`Project::imported_media`] |
//! | `Project._root_bin`, `add_sequence`, `save` | [`Project::root_bin`], [`Project::add_sequence`], [`Project::save`] |
//! | `ProjectItem.add_bin`, `color_label`, `add_marker` | [`Project::add_bin`], [`Project::set_color_label`], [`Project::add_item_marker`] |
//! | `project_item_core`, `item_container`, `clip_core`, `_child_items`, `_add_item_ref`, `_detach_item_ref` | functions in [`item`] |
//! | `Sequence.add_marker`, `zero_point`, `_link`, `_grow_source_duration` | [`Project::add_sequence_marker`], [`Project::set_zero_point`], [`Project::link`], [`Project::grow_source_duration`] |
//! | `Track.add_clip`, `add_transition` | [`Project::add_clip`], [`Project::add_transition`] |
//! | `TrackItem.start/end/in_point/out_point/duration/speed` | [`Project::start`], [`Project::set_end`], [`Project::set_in_point`], ... |
//!
//! Parsing is py_premiere's whole `parse_application`: the item tree (master clip, clip
//! elements, default out ticks, sequence uid, markers, node IDs), sequences (tracks, track
//! items with their component chains and selection masks, transitions, markers, timebase,
//! caption tracks) and the active sequence. The read / object-model API (every property and
//! setter of `Project`, `ProjectItem`, `FootageInterpretation`, `Sequence`, `SequenceSettings`,
//! `Track`, `TrackItem`, `Component`, `ComponentParam`, `Marker`, `Transition`, `Caption`,
//! `CaptionTrack`, `Preferences`, the validators and `NamedList`) is mapped in the README.
//!
//! One element, one parent: Python ElementTree lets an element sit under two parents, and
//! `_splice_fragment` relies on it (a spliced object stays a child of the discarded fragment).
//! [`crate::splice::splice_fragment`] detaches it first; nothing else in these operations
//! appends an element that is already attached.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::xml::{NodeId, PremiereDocument};

pub mod application;
pub mod color;
pub mod component;
pub mod enums;
pub mod extra;
pub mod footage;
pub mod graphics;
pub mod item_props;
pub mod marker;
pub mod named_list;
pub mod preferences;
pub mod transition;
pub mod validators;
pub mod item;
pub mod import;
pub mod combine;
pub mod project;
pub mod sequence_props;
pub mod sequence;
pub mod track;
pub mod track_props;
pub mod track_item;
pub mod track_item_props;

pub use track::TRANSITION_DISPLAY_NAMES;

/// Caption handles and records live in [`crate::caption`]; re-exported for the model's callers.
pub use crate::caption::{CaptionData, CaptionId, CaptionTrackData, CaptionTrackId};

/// Handle to a project-panel item (Python `ProjectItem`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ItemId(usize);

/// Handle to a sequence (Python `Sequence`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SequenceId(usize);

/// Handle to a track (Python `Track`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TrackId(usize);

/// Handle to a clip on a track (Python `TrackItem`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TrackItemId(usize);

/// Handle to a component (effect) of a track item (Python `Component`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ComponentId(usize);

/// Python `ProjectItemType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemType {
    Root,
    Bin,
    Clip,
}

impl ItemType {
    /// The Python enum member (`ProjectItem.type`).
    pub fn as_enum(self) -> enums::ProjectItemType {
        match self {
            ItemType::Root => enums::ProjectItemType::Root,
            ItemType::Bin => enums::ProjectItemType::Bin,
            ItemType::Clip => enums::ProjectItemType::Clip,
        }
    }
}

/// `"Video"` or `"Audio"` (Python `Track.media_type`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaType {
    Video,
    Audio,
}

impl MediaType {
    /// `"Video"` / `"Audio"`.
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Video => "Video",
            MediaType::Audio => "Audio",
        }
    }
}

/// A project-panel item's record (Python `ProjectItem` attributes).
#[derive(Clone, Debug)]
pub struct ItemData {
    /// The top-level item object (`ClipProjectItem`, `BinProjectItem`, ...): `_element`.
    pub element: NodeId,
    /// `_type`.
    pub item_type: ItemType,
    /// `_parent`.
    pub parent: Option<ItemId>,
    /// `_children`, in panel order.
    pub children: Vec<ItemId>,
    /// `_master_element`.
    pub master: Option<NodeId>,
    /// `_clip_elements`: the master's `Clips/Clip` objects.
    pub clip_elements: Vec<NodeId>,
    /// `_default_out_ticks`: the source duration when the item has no out point.
    pub default_out_ticks: Option<i64>,
    /// `_sequence_uid`: the backing sequence's ObjectUID for a sequence item.
    pub sequence_uid: Option<String>,
    /// `_media_path`.
    pub media_path: Option<String>,
    /// `_markers`: the clip-marker objects.
    pub markers: Vec<NodeId>,
    /// `_node_id_int`: the load-time node ID (stored `ProjectItem/Node/ID`, else counted).
    pub node_id: Option<i64>,
}

impl ItemData {
    /// A bare record (Python `ProjectItem(element, project, item_type)`): no parent, children,
    /// master or clips yet. File it with [`Project::push_item`].
    pub fn new(element: NodeId, item_type: ItemType) -> Self {
        ItemData {
            element,
            item_type,
            parent: None,
            children: Vec::new(),
            master: None,
            clip_elements: Vec::new(),
            default_out_ticks: None,
            sequence_uid: None,
            media_path: None,
            markers: Vec::new(),
            node_id: None,
        }
    }
}

/// A sequence's record (Python `Sequence` attributes).
#[derive(Clone, Debug)]
pub struct SequenceData {
    /// The top-level `Sequence` object.
    pub element: NodeId,
    /// Ticks per video frame (`_timebase`).
    pub timebase: Option<i64>,
    /// `(width, height)` of the video group's `FrameRect`.
    pub frame_size: Option<(i64, i64)>,
    /// Ticks per audio frame.
    pub audio_frame_rate: Option<i64>,
    /// `NumAdaptiveChannels`.
    pub audio_channel_count: Option<i64>,
    /// Video tracks in group order.
    pub video_tracks: Vec<TrackId>,
    /// Audio tracks in group order.
    pub audio_tracks: Vec<TrackId>,
    /// Sequence-marker objects.
    pub markers: Vec<NodeId>,
}

/// A track's record (Python `Track` attributes).
#[derive(Clone, Debug)]
pub struct TrackData {
    /// The top-level `VideoClipTrack` / `AudioClipTrack` object.
    pub element: NodeId,
    /// Owning sequence.
    pub sequence: SequenceId,
    /// `ClipTrack/Track/Index`.
    pub index: i64,
    /// `ClipTrack/Track/ID`.
    pub id: i64,
    /// Video or audio.
    pub media_type: MediaType,
    /// `_clips`, in the order they were parsed or placed.
    pub clips: Vec<TrackItemId>,
    /// `_transitions`: transition track-item objects.
    pub transitions: Vec<NodeId>,
}

/// A placed clip's record (Python `TrackItem` attributes).
#[derive(Clone, Debug)]
pub struct TrackItemData {
    /// The `VideoClipTrackItem` / `AudioClipTrackItem` object (`_element`).
    pub element: NodeId,
    /// Its `SubClip` object (`_subclip_element`).
    pub subclip: NodeId,
    /// The timeline `VideoClip` / `AudioClip` object (`_clip_element`).
    pub clip: NodeId,
    /// Owning track.
    pub track: TrackId,
    /// `_components`: the materialized components (effects).
    pub components: Vec<ComponentId>,
    /// `_selection_components`: clip-level masks.
    pub selection_components: Vec<ComponentId>,
}

impl TrackItemData {
    /// A record with no parsed components.
    pub fn new(element: NodeId, subclip: NodeId, clip: NodeId, track: TrackId) -> Self {
        TrackItemData { element, subclip, clip, track, components: Vec::new(), selection_components: Vec::new() }
    }
}

/// A component's record (Python `Component` attributes).
#[derive(Clone, Debug)]
pub struct ComponentData {
    /// The component object (`VideoFilterComponent`, `AudioFilterComponent`, ...).
    pub element: NodeId,
    /// The track item it belongs to.
    pub track_item: TrackItemId,
    /// `_properties`: the parameter objects.
    pub properties: Vec<NodeId>,
    /// `_sub_components` (masks applied to an effect).
    pub sub_components: Vec<ComponentId>,
}

/// What the converter stores on Python `ProjectItem`s as ad-hoc attributes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemExtra {
    /// `item._channel_groups`: the source channels each audio channel group plays
    /// (`None` = attribute absent).
    pub channel_groups: Option<Vec<Vec<usize>>>,
    /// `item._stereo_nest`.
    pub stereo_nest: bool,
}

/// A Premiere project: the document plus the model tables (Python `Project`).
#[derive(Clone, Debug)]
pub struct Project {
    /// The document. Edit it directly where Python edits `project._document`.
    pub doc: PremiereDocument,
    path: PathBuf,
    items: Vec<ItemData>,
    root_item: Option<ItemId>,
    sequences: Vec<SequenceData>,
    tracks: Vec<TrackData>,
    track_items: Vec<TrackItemData>,
    /// `_import_target`: the bin [`Project::attach_imported_item`] files into (root when `None`).
    pub import_target: Option<ItemId>,
    /// `_imported_media`: `Media` objects whose `RelativePath` is re-derived on save.
    pub imported_media: Vec<NodeId>,
    /// Converter attributes per item (`_channel_groups`, `_stereo_nest`).
    pub item_extra: HashMap<ItemId, ItemExtra>,
    /// Caption tracks and captions (see [`crate::caption`]).
    pub(crate) captions: crate::caption::CaptionStore,
    /// Master-clip UID -> item, rebuilt after any change to the item table.
    master_index: RefCell<Option<HashMap<String, ItemId>>>,
    /// Sequence UID -> item, rebuilt likewise.
    sequence_index: RefCell<Option<HashMap<String, ItemId>>>,
    components: Vec<ComponentData>,
    /// `_active_sequence`: the frontmost sequence (from the open-sequence list).
    pub active_sequence: Option<SequenceId>,
    /// `_preferences_path`: overrides the machine-profile discovery of `Preferences`.
    pub preferences_path: Option<PathBuf>,
    /// Nonfatal warnings from the last completed save, such as retained staging files.
    pub(crate) save_warnings: Vec<String>,
}
