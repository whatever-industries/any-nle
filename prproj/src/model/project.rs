//! The project: creation, parsing, registration, sequences and save (port of
//! `py_premiere/__init__.py` `new`, `parsers/{application,project,sequence,marker}.py` and
//! `models/project.py`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::item::{add_item_ref, child_items, clip_core, item_container, project_item_core, validate_name};
use super::{
    ComponentData, ComponentId, ItemData, ItemId, ItemType,
    MediaType, Project, SequenceData, SequenceId, TrackData, TrackId, TrackItemData, TrackItemId,
};
use crate::build::media::media_relpath;
use crate::build::sequence::build_sequence;
use crate::build::skeleton::{build_empty_project, EmptyProjectOptions};
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::splice::{splice_fragment, Spliced};
use crate::xml::{GzipFraming, NodeId, PremiereDocument, Tree};

/// Python `int(text)` (base 10): surrounding whitespace, an optional sign, ASCII digits with
/// single underscores only BETWEEN digits (`1_000` yes; `_1`, `1_`, `1__0` no).
pub fn py_int(text: &str) -> Result<i64> {
    let invalid = || Error::Value(format!("invalid literal for int() with base 10: '{text}'"));
    let t = crate::pystr::py_strip(text);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    let bytes = digits.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() || !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err(invalid());
    }
    for (i, &b) in bytes.iter().enumerate() {
        let ok = b.is_ascii_digit() || (b == b'_' && bytes[i + 1].is_ascii_digit());
        if !ok {
            return Err(invalid());
        }
    }
    let clean: String = t.chars().filter(|&c| c != '_').collect();
    clean.parse::<i64>().map_err(|_| Error::Value(format!("integer out of range: '{text}'")))
}

/// Python `pathlib.PurePosixPath(text)` normalisation: repeated slashes and `.` components
/// dropped, a trailing slash removed, a leading `//` (exactly two) kept, `""` -> `"."`.
/// `..` is kept as written (Path does not collapse it).
pub fn py_path(text: &str) -> PathBuf {
    let root = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if text.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let joined = format!("{root}{}", parts.join("/"));
    PathBuf::from(if joined.is_empty() { ".".to_string() } else { joined })
}

fn py_path_string(text: &str) -> String {
    py_path(text).to_string_lossy().into_owned()
}

/// Python `Path.resolve()` (non-strict): absolute, symlinks resolved as far as the path exists.
pub fn resolve_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    if let Ok(real) = std::fs::canonicalize(&absolute) {
        return real;
    }
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) if parent != absolute => resolve_path(parent).join(name),
        _ => absolute,
    }
}

/// Final encoded project and its explicit publication intent. Not a source graph.
pub(crate) struct PreparedSave {
    target: PathBuf,
    data: Vec<u8>,
    replace: bool,
}
impl PreparedSave {
    pub(crate) fn publish(self) -> Result<Vec<String>> {
        self.publish_with(crate::publication::publish_new)
    }
    fn publish_with(self,
        publish: impl FnOnce(&Path, &Path) -> std::io::Result<Option<String>>) -> Result<Vec<String>> {
        self.publish_with_cleanup(publish,|p|std::fs::remove_file(p))
    }
    fn publish_with_cleanup(self,
        publish: impl FnOnce(&Path,&Path)->std::io::Result<Option<String>>,
        cleanup: impl FnOnce(&Path)->std::io::Result<()>) -> Result<Vec<String>> {
        let Self { target, data, replace } = self;
        let target = target.as_path();
        let mut number = 1u64;
        let (temporary, mut file) = loop {
            let mut name = target.as_os_str().to_owned();
            name.push(if number == 1 { ".tmp".to_string() } else { format!(".tmp.{number}") });
            let path = PathBuf::from(name);
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break (path, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    number = number.checked_add(1).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no unused project staging path available"))?;
                }
                Err(error) => return Err(error.into()),
            }
        };
        let written = file.write_all(&data);
        drop(file); // Close before rename or cleanup, including on platforms that lock open files.
        let result = written.and_then(|_| {
            if replace { std::fs::rename(&temporary, target).map(|_| None) }
            else { publish(&temporary, target) }
        });
        let mut cleanup_error=None;
        if result.is_err() {
            // Only our exclusively created sibling; never remove an occupied candidate.
            if let Err(e)=cleanup(&temporary) { cleanup_error=Some(e); }
        }
        let warning = result.map_err(|e| {
            if let Some(cleanup)=cleanup_error {
                let message=format!("{e}; retained project staging file {}: cleanup failed: {cleanup}",temporary.display());
                return if !replace&&e.kind()==std::io::ErrorKind::AlreadyExists {Error::FileExists(message)}else{Error::Io(message)};
            }
            if !replace && e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::FileExists(format!("path already exists: {}", target.display()))
            } else { e.into() }
        })?;
        let warnings = warning.into_iter().collect();
        Ok(warnings)
    }
}

impl Project {
    // ---- creation and parsing ------------------------------------------------------------

    /// `py_premiere.new()`: a new, empty project (draws the document id, project GUID and
    /// view-state id). It reports the name `untitled.prproj` until saved.
    pub fn new(uuids: &mut dyn UuidSource) -> Result<Project> {
        let mut tree = Tree::new();
        let root = build_empty_project(&mut tree, &EmptyProjectOptions::default(), uuids);
        let document = PremiereDocument::new(tree, root, Some(GzipFraming::default()))?;
        Project::from_document(document, PathBuf::from("untitled.prproj"))
    }

    /// `parse_application(document, path)`: the item tree and the sequences.
    pub fn from_document(document: PremiereDocument, path: impl AsRef<Path>) -> Result<Project> {
        let mut project = Project {
            doc: document,
            path: py_path(&path.as_ref().to_string_lossy()),
            items: Vec::new(),
            root_item: None,
            sequences: Vec::new(),
            tracks: Vec::new(),
            track_items: Vec::new(),
            import_target: None,
            imported_media: Vec::new(),
            item_extra: HashMap::new(),
            captions: Default::default(),
            master_index: RefCell::new(None),
            sequence_index: RefCell::new(None),
            components: Vec::new(),
            active_sequence: None,
            preferences_path: None,
            save_warnings: Vec::new(),
        };
        let tree = &project.doc.tree;
        if let Some(stub) = tree.child_by_tag(project.doc.root, "Project") {
            let project_element = project.doc.resolve(stub)?;
            if let Some(root_ref) = project.doc.tree.child_by_tag(project_element, "RootProjectItem") {
                let root_element = project.doc.resolve(root_ref)?;
                let root = project.parse_item(root_element)?;
                project.root_item = Some(root);
                project.assign_node_ids(root)?;
            }
        }
        let sequences: Vec<NodeId> = project
            .doc
            .tree
            .children(project.doc.root)
            .iter()
            .copied()
            .filter(|&e| project.doc.tree.tag(e) == "Sequence")
            .collect();
        for element in sequences {
            project.parse_sequence(element)?;
        }
        if let Some(active_id) = project.active_sequence_id()? {
            project.active_sequence =
                project.sequences().into_iter().find(|&s| project.sequence_id(s).as_deref() == Some(active_id.as_str()));
        }
        Ok(project)
    }

    /// `parsers/application.py` `_active_sequence_id`: the highest-numbered entry of the
    /// open-sequence GUID list in the Project's property bag.
    fn active_sequence_id(&self) -> Result<Option<String>> {
        const PREFIX: &str = "MZ.PrefixKey.OpenSequenceGuidList.";
        let tree = &self.doc.tree;
        let Some(stub) = tree.child_by_tag(self.doc.root, "Project") else { return Ok(None) };
        let project_element = self.doc.resolve(stub)?;
        let mut best_index: i64 = -1;
        let mut best = None;
        for element in tree.findall(project_element, "Node/Properties/*")? {
            let tag = tree.tag(element);
            let text = tree[element].text_or_empty();
            if !tag.starts_with(PREFIX) || text.is_empty() {
                continue;
            }
            let Ok(index) = py_int(&tag[PREFIX.len()..]) else { continue };
            if index > best_index {
                best_index = index;
                best = Some(text.to_string());
            }
        }
        Ok(best)
    }

    /// `parsers/project.py` `_assign_node_ids`: stored IDs win, unstored items count on from
    /// 1000000 in depth-first order.
    fn assign_node_ids(&mut self, root: ItemId) -> Result<()> {
        let mut counter: i64 = 1_000_000;
        let mut stack = vec![root];
        while let Some(item) = stack.pop() {
            let stored = self.doc.tree.findtext(self.item(item).element, "ProjectItem/Node/ID")?;
            let id = match stored.filter(|s| !s.is_empty()) {
                Some(s) => {
                    let v = py_int(&s)?;
                    counter = counter.max(v + 1);
                    v
                }
                None => {
                    counter += 1;
                    counter - 1
                }
            };
            self.items[item.0].node_id = Some(id);
            stack.extend(self.item(item).children.iter().rev().copied());
        }
        Ok(())
    }

    fn parse_item(&mut self, element: NodeId) -> Result<ItemId> {
        let item_type = match self.doc.tree.tag(element) {
            "RootProjectItem" => ItemType::Root,
            "BinProjectItem" | "SmartBinProjectItem" => ItemType::Bin,
            _ => ItemType::Clip,
        };
        let id = self.push_item(ItemData::new(element, item_type));
        let child_refs = match item_container(&self.doc.tree, element) {
            Some(container) => self.doc.tree.findall(container, "Items/Item")?,
            None => Vec::new(),
        };
        for child_ref in child_refs {
            let child_element = self.doc.resolve(child_ref)?;
            let child = self.parse_item(child_element)?;
            self.item_mut(child).parent = Some(id);
            self.item_mut(id).children.push(child);
        }
        if let Some(master_ref) = self.doc.tree.child_by_tag(element, "MasterClip") {
            let master = self.doc.resolve(master_ref)?;
            let media_path = self.media_path_of(master)?;
            let clip_elements = self.resolve_all(master, "Clips/Clip")?;
            let mut markers = Vec::new();
            let mut default_out_ticks = None;
            let mut sequence_uid = None;
            if let Some(&first) = clip_elements.first() {
                let core = clip_core(&self.doc.tree, first);
                if let Some(core) = core {
                    markers = self.parse_markers(core)?;
                }
                if let Some(source_ref) = core.and_then(|c| self.doc.tree.child_by_tag(c, "Source")) {
                    let source = self.doc.resolve(source_ref)?;
                    if let Some(d) = self.doc.tree.findtext(source, "OriginalDuration")?.filter(|d| !d.is_empty()) {
                        default_out_ticks = Some(py_int(&d)?);
                    }
                    if let Some(r) = self.doc.tree.find1(source, "SequenceSource/Sequence") {
                        sequence_uid = self.doc.tree.get(r, "ObjectURef").map(str::to_string);
                    }
                }
            }
            let data = self.item_mut(id);
            data.master = Some(master);
            data.media_path = media_path;
            data.clip_elements = clip_elements;
            data.markers = markers;
            data.default_out_ticks = default_out_ticks;
            data.sequence_uid = sequence_uid;
        }
        Ok(id)
    }

    /// `parsers/project.py` `_media_path`.
    fn media_path_of(&self, master: NodeId) -> Result<Option<String>> {
        let tree = &self.doc.tree;
        for clip_ref in tree.findall(master, "Clips/Clip")? {
            let clip = self.doc.resolve(clip_ref)?;
            let Some(source_ref) = clip_core(tree, clip).and_then(|c| tree.child_by_tag(c, "Source")) else { continue };
            let source = self.doc.resolve(source_ref)?;
            let Some(media_ref) = tree.find1(source, "MediaSource/Media") else { continue };
            let media = self.doc.resolve(media_ref)?;
            if let Some(file_path) = tree.findtext(media, "FilePath")? {
                if !file_path.is_empty() && !file_path.chars().all(|c| c.is_ascii_digit()) {
                    return Ok(Some(py_path_string(&file_path)));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn resolve_all(&self, element: NodeId, path: &str) -> Result<Vec<NodeId>> {
        self.doc.tree.findall(element, path)?.into_iter().map(|r| self.doc.resolve(r)).collect()
    }

    /// `parsers/marker.py` `parse_markers`.
    pub(crate) fn parse_markers(&self, owner: NodeId) -> Result<Vec<NodeId>> {
        let Some(reference) = self.doc.tree.find1(owner, "MarkerOwner/Markers") else { return Ok(Vec::new()) };
        let container = self.doc.resolve(reference)?;
        let mut markers = Vec::new();
        for pair in self.doc.tree.findall(container, "Markers/Marker")? {
            if let Some(second) = self.doc.tree.child_by_tag(pair, "Second") {
                markers.push(self.doc.resolve(second)?);
            }
        }
        Ok(markers)
    }

    /// `parsers/sequence.py` `parse_sequence`: read a sequence object already in the document
    /// (spliced or attached) into the project's sequences. Caption tracks and components are
    /// not parsed.
    pub fn parse_sequence(&mut self, element: NodeId) -> Result<SequenceId> {
        let markers = self.parse_markers(element)?;
        let sequence = SequenceId(self.sequences.len());
        self.sequences.push(SequenceData {
            element,
            timebase: None,
            frame_size: None,
            audio_frame_rate: None,
            audio_channel_count: None,
            video_tracks: Vec::new(),
            audio_tracks: Vec::new(),
            markers,
        });
        for pair in self.doc.tree.findall(element, "TrackGroups/TrackGroup")? {
            let Some(second) = self.doc.tree.child_by_tag(pair, "Second") else { continue };
            let group = self.doc.resolve(second)?;
            let tree = &self.doc.tree;
            let media_type = match tree.tag(group) {
                "VideoTrackGroup" => {
                    let timebase = tree.findtext(group, "TrackGroup/FrameRate")?.filter(|t| !t.is_empty());
                    let frame_size = parse_frame_rect(tree.findtext(group, "FrameRect")?.as_deref())?;
                    let data = &mut self.sequences[sequence.0];
                    if let Some(t) = timebase {
                        data.timebase = Some(py_int(&t)?);
                    }
                    data.frame_size = frame_size;
                    MediaType::Video
                }
                "AudioTrackGroup" => {
                    let rate = tree.findtext(group, "TrackGroup/FrameRate")?.filter(|t| !t.is_empty());
                    let channels = tree.findtext(group, "NumAdaptiveChannels")?.filter(|t| !t.is_empty());
                    let data = &mut self.sequences[sequence.0];
                    if let Some(r) = rate {
                        data.audio_frame_rate = Some(py_int(&r)?);
                    }
                    if let Some(c) = channels {
                        data.audio_channel_count = Some(py_int(&c)?);
                    }
                    MediaType::Audio
                }
                "DataTrackGroup" => {
                    self.parse_caption_tracks(sequence, group)?;
                    continue;
                }
                _ => continue,
            };
            for track_ref in self.doc.tree.findall(group, "TrackGroup/Tracks/Track")? {
                let track_element = self.doc.resolve(track_ref)?;
                let track = self.parse_track(sequence, media_type, track_element)?;
                let data = &mut self.sequences[sequence.0];
                match media_type {
                    MediaType::Video => data.video_tracks.push(track),
                    MediaType::Audio => data.audio_tracks.push(track),
                }
            }
        }
        Ok(sequence)
    }

    fn parse_track(&mut self, sequence: SequenceId, media_type: MediaType, element: NodeId) -> Result<TrackId> {
        let tree = &self.doc.tree;
        let (id, index) = match tree.find1(element, "ClipTrack/Track") {
            Some(inner) => {
                let id = tree.findtext(inner, "ID")?.filter(|t| !t.is_empty()).map(|t| py_int(&t)).transpose()?;
                let index = tree.findtext(inner, "Index")?.filter(|t| !t.is_empty()).map(|t| py_int(&t)).transpose()?;
                (id.unwrap_or(0), index.unwrap_or(0))
            }
            None => (0, 0),
        };
        let track = TrackId(self.tracks.len());
        self.tracks.push(TrackData {
            element,
            sequence,
            index,
            id,
            media_type,
            clips: Vec::new(),
            transitions: Vec::new(),
        });
        for item_ref in self.doc.tree.findall(element, "ClipTrack/ClipItems/TrackItems/TrackItem")? {
            let item_element = self.doc.resolve(item_ref)?;
            if let Some(item) = self.parse_track_item(track, item_element)? {
                self.tracks[track.0].clips.push(item);
            }
        }
        for reference in self.doc.tree.findall(element, "ClipTrack/TransitionItems/TrackItems/TrackItem")? {
            let transition = self.doc.resolve(reference)?;
            self.tracks[track.0].transitions.push(transition);
        }
        Ok(track)
    }

    fn parse_track_item(&mut self, track: TrackId, element: NodeId) -> Result<Option<TrackItemId>> {
        let tree = &self.doc.tree;
        let Some(clip_track_item) = tree.child_by_tag(element, "ClipTrackItem") else { return Ok(None) };
        let Some(subclip_ref) = tree.child_by_tag(clip_track_item, "SubClip") else { return Ok(None) };
        let subclip = self.doc.resolve(subclip_ref)?;
        let Some(clip_ref) = self.doc.tree.child_by_tag(subclip, "Clip") else { return Ok(None) };
        let clip = self.doc.resolve(clip_ref)?;
        let item = self.push_track_item(TrackItemData::new(element, subclip, clip, track));
        let chain = self.doc.tree.find1(clip_track_item, "ComponentOwner/Components");
        let components = self.parse_chain(item, chain)?;
        self.track_items[item.0].components = components;
        let selection = self.doc.tree.child_by_tag(element, "SelectionComponents");
        let selection = self.parse_chain(item, selection)?;
        self.track_items[item.0].selection_components = selection;
        Ok(Some(item))
    }

    /// `parsers/component.py` `_parse_chain`.
    fn parse_chain(&mut self, track_item: TrackItemId, chain_ref: Option<NodeId>) -> Result<Vec<ComponentId>> {
        let Some(chain_ref) = chain_ref else { return Ok(Vec::new()) };
        let chain = self.doc.resolve(chain_ref)?;
        let mut out = Vec::new();
        for component_ref in self.doc.tree.findall(chain, "ComponentChain/Components/Component")? {
            let element = self.doc.resolve(component_ref)?;
            out.push(self.build_component(element, track_item)?);
        }
        Ok(out)
    }

    /// `parsers/component.py` `_build_component`: sub-components (on the outer element) first,
    /// then the parameters of the inner `Component` (or `AudioComponent/Component`).
    fn build_component(&mut self, element: NodeId, track_item: TrackItemId) -> Result<ComponentId> {
        let id = self.push_component(ComponentData { element, track_item, properties: Vec::new(), sub_components: Vec::new() });
        for sub_ref in self.doc.tree.findall(element, "SubComponents/SubComponent")? {
            let sub_element = self.doc.resolve(sub_ref)?;
            let sub = self.build_component(sub_element, track_item)?;
            self.components[id.0].sub_components.push(sub);
        }
        let tree = &self.doc.tree;
        let inner = tree.child_by_tag(element, "Component").or_else(|| tree.find1(element, "AudioComponent/Component"));
        let Some(inner) = inner else { return Ok(id) };
        for param_ref in self.doc.tree.findall(inner, "Params/Param")? {
            let param = self.doc.resolve(param_ref)?;
            self.components[id.0].properties.push(param);
        }
        Ok(id)
    }

    pub(crate) fn push_component(&mut self, data: ComponentData) -> ComponentId {
        self.components.push(data);
        ComponentId(self.components.len() - 1)
    }

    /// A component's record.
    pub fn component(&self, component: ComponentId) -> &ComponentData {
        &self.components[component.0]
    }

    pub(crate) fn track_item_mut(&mut self, item: TrackItemId) -> &mut TrackItemData {
        &mut self.track_items[item.0]
    }

    // ---- tables --------------------------------------------------------------------------

    /// File a new item record (Python constructing a `ProjectItem`). Link it with
    /// `item_mut(parent).children.push(..)` and its own `parent`, as Python does.
    pub fn push_item(&mut self, data: ItemData) -> ItemId {
        self.invalidate_item_indexes();
        self.items.push(data);
        ItemId(self.items.len() - 1)
    }

    pub(crate) fn push_track_item(&mut self, data: TrackItemData) -> TrackItemId {
        self.track_items.push(data);
        TrackItemId(self.track_items.len() - 1)
    }

    /// An item's record.
    pub fn item(&self, item: ItemId) -> &ItemData {
        &self.items[item.0]
    }

    /// An item's record, for edits Python makes to `ProjectItem` attributes.
    pub fn item_mut(&mut self, item: ItemId) -> &mut ItemData {
        self.invalidate_item_indexes();
        &mut self.items[item.0]
    }

    fn invalidate_item_indexes(&mut self) {
        *self.master_index.get_mut() = None;
        *self.sequence_index.get_mut() = None;
    }

    /// A sequence's record.
    pub fn sequence(&self, sequence: SequenceId) -> &SequenceData {
        &self.sequences[sequence.0]
    }

    pub(crate) fn sequence_mut(&mut self, sequence: SequenceId) -> &mut SequenceData {
        &mut self.sequences[sequence.0]
    }

    /// A track's record.
    pub fn track(&self, track: TrackId) -> &TrackData {
        &self.tracks[track.0]
    }

    /// A track's record, for edits Python makes to `Track` attributes.
    pub fn track_mut(&mut self, track: TrackId) -> &mut TrackData {
        &mut self.tracks[track.0]
    }

    /// A placed clip's record.
    pub fn track_item(&self, item: TrackItemId) -> &TrackItemData {
        &self.track_items[item.0]
    }

    /// `Project.root_item`.
    pub fn root_item(&self) -> Option<ItemId> {
        self.root_item
    }

    pub(crate) fn require_root(&self) -> Result<ItemId> {
        self.root_item.ok_or_else(|| Error::Value("project has no root item".into()))
    }

    /// `Project.sequences`, in parse/creation order.
    pub fn sequences(&self) -> Vec<SequenceId> {
        (0..self.sequences.len()).map(SequenceId).collect()
    }

    /// `Project.path`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Set `_path` (the converter points it at the output file before importing, so relative
    /// media paths are computed against the output location).
    pub fn set_path(&mut self, path: impl AsRef<Path>) {
        self.path = py_path(&path.as_ref().to_string_lossy());
    }

    /// `Project.name`: the file name.
    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// `Project.document_id`: the root item's ObjectUID.
    pub fn document_id(&self) -> Option<String> {
        self.root_item.and_then(|r| self.doc.tree.get(self.item(r).element, "ObjectUID").map(str::to_string))
    }

    /// Every item, depth-first from the root (`ProjectItem.walk` with the root first).
    pub fn walk_items(&self) -> Vec<ItemId> {
        fn visit(project: &Project, item: ItemId, out: &mut Vec<ItemId>) {
            out.push(item);
            for &child in &project.item(item).children {
                visit(project, child, out);
            }
        }
        let mut out = Vec::new();
        if let Some(root) = self.root_item {
            visit(self, root, &mut out);
        }
        out
    }

    /// `Project._item_by_master_uid`: the item owning the master clip with this ObjectUID (the
    /// last one in depth-first order, like Python's map). The index is rebuilt after any change
    /// to the item table; editing a master's ObjectUID in the tree directly is not tracked
    /// (Python's cached map does not see that either).
    pub fn item_by_master_uid(&self, uid: &str) -> Option<ItemId> {
        let mut index = self.master_index.borrow_mut();
        index.get_or_insert_with(|| self.items_by_master_uid()).get(uid).copied()
    }

    /// `Project._item_by_sequence_uid`.
    pub fn item_by_sequence_uid(&self, uid: &str) -> Option<ItemId> {
        let mut index = self.sequence_index.borrow_mut();
        let map = index.get_or_insert_with(|| {
            let mut map = HashMap::new();
            for item in self.walk_items() {
                if let Some(u) = &self.item(item).sequence_uid {
                    map.insert(u.clone(), item);
                }
            }
            map
        });
        map.get(uid).copied()
    }

    // ---- registration --------------------------------------------------------------------

    /// `Project._register`: add a new top-level object under a fresh ObjectID.
    pub fn register(&mut self, element: NodeId) -> Result<String> {
        self.doc.add_object(element)
    }

    /// `Project._register_uid`: add a new top-level object under its own ObjectUID.
    pub fn register_uid(&mut self, element: NodeId, uid: &str) -> Result<NodeId> {
        self.doc.tree.set(element, "ObjectUID", uid);
        self.doc.attach_object(element)
    }

    /// `Project._splice_fragment`.
    pub fn splice(&mut self, fragment: NodeId, uuids: &mut dyn UuidSource) -> Result<Spliced> {
        splice_fragment(&mut self.doc, fragment, uuids)
    }

    /// `Project._attach_imported_item`: wire a registered item/master graph into the panel under
    /// [`import_target`](Self::import_target) (the root by default).
    pub fn attach_imported_item(
        &mut self,
        item_element: NodeId,
        item_uid: &str,
        master: NodeId,
        clip_ids: &[&str],
        file_path: &str,
        default_out_ticks: i64,
    ) -> Result<ItemId> {
        let parent = match self.import_target {
            Some(p) => p,
            None => self.require_root()?,
        };
        let container = self.require_item_container(parent, "target item")?;
        let items = child_items(&mut self.doc.tree, container);
        add_item_ref(&mut self.doc.tree, items, item_uid);
        let clip_elements = clip_ids
            .iter()
            .map(|id| self.doc.by_object_id.get(*id).copied().ok_or_else(|| Error::Value(format!("unknown ObjectID '{id}'"))))
            .collect::<Result<Vec<_>>>()?;
        let mut data = ItemData::new(item_element, ItemType::Clip);
        data.parent = Some(parent);
        data.master = Some(master);
        data.media_path = Some(py_path_string(file_path));
        data.clip_elements = clip_elements;
        data.default_out_ticks = Some(default_out_ticks);
        let item = self.push_item(data);
        self.item_mut(parent).children.push(item);
        Ok(item)
    }

    /// `Project._root_bin(name, build)`: the root-level bin of this name, created from
    /// `build` (which gets the arena and the UUID source) when absent. The bin's UID is drawn
    /// before `build` runs, as in Python.
    pub fn root_bin<F>(&mut self, name: &str, build: F, uuids: &mut dyn UuidSource) -> Result<ItemId>
    where
        F: FnOnce(&mut Tree, &mut dyn UuidSource) -> Result<NodeId>,
    {
        let root = self.require_root()?;
        let container = self.require_item_container(root, "root item")?;
        let existing = self
            .item(root)
            .children
            .iter()
            .copied()
            .find(|&c| self.item(c).item_type == ItemType::Bin && self.item_name(c) == name);
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let uid = uuids.uuid4().to_string();
        let element = build(&mut self.doc.tree, uuids)?;
        self.doc.tree.set(element, "ObjectUID", uid.clone());
        self.doc.attach_object(element)?;
        let mut data = ItemData::new(element, ItemType::Bin);
        data.parent = Some(root);
        let bin = self.push_item(data);
        let items = child_items(&mut self.doc.tree, container);
        add_item_ref(&mut self.doc.tree, items, &uid);
        self.item_mut(root).children.push(bin);
        Ok(bin)
    }

    // ---- sequences -----------------------------------------------------------------------

    /// `Project.add_sequence(name, preset, video_tracks)`: build the sequence fragment, splice
    /// it, file its panel item at the root and parse it.
    pub fn add_sequence(
        &mut self,
        name: &str,
        preset: &str,
        video_tracks: Option<usize>,
        uuids: &mut dyn UuidSource,
    ) -> Result<SequenceId> {
        validate_name(name)?;
        if video_tracks == Some(0) {
            return value("must be >= 1, got 0");
        }
        let fragment = build_sequence(&mut self.doc.tree, name, preset, None, video_tracks, uuids)?;
        let spliced = self.splice(fragment, uuids)?;
        let (Some(sequence_element), Some(item_element), Some(master_element)) =
            (spliced.get("Sequence"), spliced.get("ClipProjectItem"), spliced.get("MasterClip"))
        else {
            return value("sequence template is missing its core objects");
        };
        let root = self.require_root()?;
        let container = self.require_item_container(root, "root item")?;
        let Some(item_uid) = self.doc.tree.get(item_element, "ObjectUID").map(str::to_string) else {
            return value("template panel item has no ObjectUID");
        };
        let items = child_items(&mut self.doc.tree, container);
        add_item_ref(&mut self.doc.tree, items, &item_uid);
        let sequence = self.parse_sequence(sequence_element)?;
        let clip_elements = self.resolve_all(master_element, "Clips/Clip")?;
        let mut data = ItemData::new(item_element, ItemType::Clip);
        data.parent = Some(root);
        data.master = Some(master_element);
        data.clip_elements = clip_elements;
        data.sequence_uid = self.doc.tree.get(sequence_element, "ObjectUID").map(str::to_string);
        let item = self.push_item(data);
        self.item_mut(root).children.push(item);
        Ok(sequence)
    }

    // ---- save ----------------------------------------------------------------------------

    /// `Project.save(path)`: refuses an existing file; re-derives imported media's
    /// `RelativePath` against the destination directory; writes through an exclusively created
    /// temporary sibling, then publishes without replacement. Occupied staging paths are kept.
    /// The project reports the new path only after publication succeeds.
    pub fn save(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        match std::fs::symlink_metadata(path) {
            Ok(_) => return Err(Error::FileExists(format!("path already exists: {}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
        self.write(path, false)
    }

    /// `Project.save_in_place()`: overwrite the file this project was read from.
    pub fn save_in_place(&mut self) -> Result<()> {
        if !self.path.exists() {
            return Err(Error::FileNotFound(format!("project has no saved file to overwrite: {}", self.path.display())));
        }
        let path = self.path.clone();
        self.write(&path, true)
    }

    pub fn take_save_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.save_warnings)
    }

    fn write(&mut self, target: &Path, replace: bool) -> Result<()> {
        self.write_with_publisher(target, replace, crate::publication::publish_new)
    }

    /// Prepare final gzip bytes once, without creating any path. The caller must
    /// keep this payload opaque and publish it without reconstructing the project.
    pub(crate) fn prepare_new_save(&mut self, target: &Path) -> Result<PreparedSave> {
        match std::fs::symlink_metadata(target) {
            Ok(_) => return Err(Error::FileExists(format!("path already exists: {}", target.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }
        self.prepare_save(target, false)
    }

    fn prepare_save(&mut self, target: &Path, replace: bool) -> Result<PreparedSave> {
        let directory = resolve_path(target).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
        self.rewrite_relative_paths(&directory);
        Ok(PreparedSave { target: target.to_path_buf(), data: self.doc.to_bytes()?, replace })
    }

    fn write_with_publisher(&mut self, target: &Path, replace: bool,
        publish: impl FnOnce(&Path, &Path) -> std::io::Result<Option<String>>) -> Result<()> {
        self.save_warnings.clear();
        self.save_warnings = self.prepare_save(target, replace)?.publish_with(publish)?;
        self.path = py_path(&target.to_string_lossy());
        Ok(())
    }

    /// `Project._rewrite_relative_paths(directory)`: only media this session imported.
    pub fn rewrite_relative_paths(&mut self, directory: &Path) {
        let directory = directory.to_string_lossy().into_owned();
        for &media in &self.imported_media {
            let tree = &mut self.doc.tree;
            let Some(relative) = tree.child_by_tag(media, "RelativePath") else { continue };
            let actual = tree.child_by_tag(media, "FilePath").map(|e| tree[e].text_or_empty().to_string());
            let Some(actual) = actual.filter(|a| !a.is_empty()) else { continue };
            tree.set_text(relative, Some(media_relpath(&actual, &directory)));
        }
    }

    /// The element `project_item_core` finds for an item.
    pub fn item_core(&self, item: ItemId) -> Option<NodeId> {
        project_item_core(&self.doc.tree, self.item(item).element)
    }

    /// Python `ProjectItem._clip_elements` tags, e.g. to check an item has both halves.
    pub fn clip_element_tags(&self, item: ItemId) -> Vec<String> {
        self.item(item).clip_elements.iter().map(|&e| self.doc.tree.tag(e).to_string()).collect()
    }

    /// Lookup table master UID -> item for many queries at once.
    pub fn items_by_master_uid(&self) -> HashMap<String, ItemId> {
        let mut map = HashMap::new();
        for item in self.walk_items() {
            if let Some(uid) = self.item(item).master.and_then(|m| self.doc.tree.get(m, "ObjectUID")) {
                map.insert(uid.to_string(), item);
            }
        }
        map
    }
}

/// `_parse_frame_rect`: `left,top,width,height` -> `(width, height)`.
fn parse_frame_rect(value: Option<&str>) -> Result<Option<(i64, i64)>> {
    let Some(v) = value.filter(|v| !v.is_empty()) else { return Ok(None) };
    let parts: Vec<&str> = v.split(',').collect();
    if parts.len() != 4 {
        return Ok(None);
    }
    Ok(Some((py_int(parts[2])?, py_int(parts[3])?)))
}

#[cfg(test)]
mod publication_regressions {
    use super::*;
    fn scratch() -> PathBuf {
        static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
        let p=std::env::temp_dir().join(format!("prproj-publish-state-{}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap(); p
    }
    #[test]
    fn project_path_and_incumbent_survive_a_late_publication_collision() {
        let root=scratch(); let target=root.join("new.prproj");
        let mut project=Project::new(&mut crate::SeededUuids::new("publication")).unwrap();
        let before=project.path.clone();
        let error=project.write_with_publisher(&target,false,|stage,target| {
            std::fs::write(target,b"late incumbent")?;
            crate::publication::publish_new(stage,target)
        }).unwrap_err();
        assert_eq!(error.py_class(),"FileExistsError");
        assert_eq!(std::fs::read(&target).unwrap(),b"late incumbent");
        assert_eq!(project.path,before);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn published_project_updates_path_even_when_stage_cleanup_fails() {
        let root=scratch(); let target=root.join("new.prproj");
        let mut project=Project::new(&mut crate::SeededUuids::new("publication")).unwrap();
        project.write_with_publisher(&target,false,|stage,target| {
            std::fs::hard_link(stage,target)?;
            Ok(Some(format!("project published; staging cleanup failed: {}",stage.display())))
        }).unwrap();
        assert_eq!(project.path,target);
        assert_eq!(project.take_save_warnings().len(),1);
        assert!(project.take_save_warnings().is_empty());
        assert!(std::fs::read(&target).unwrap().starts_with(&[0x1f,0x8b]));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(),2);
        project.save_in_place().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_publication_and_cleanup_report_stage_without_losing_original_failure() {
        let root=scratch();let target=root.join("new.prproj");let stage=root.join("new.prproj.tmp");
        let mut project=Project::new(&mut crate::SeededUuids::new("publication")).unwrap();
        let payload=project.prepare_new_save(&target).unwrap();
        std::fs::write(&target,b"incumbent").unwrap();
        let error=payload.publish_with_cleanup(crate::publication::publish_new,|_|Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"injected stage cleanup failure"))).unwrap_err();
        assert_eq!(error.py_class(),"FileExistsError");assert!(error.to_string().contains(stage.to_str().unwrap()));assert!(error.to_string().contains("cleanup failed"));
        assert_eq!(std::fs::read(&target).unwrap(),b"incumbent");assert!(std::fs::read(&stage).unwrap().starts_with(&[0x1f,0x8b]));std::fs::remove_dir_all(root).unwrap();
    }
}
