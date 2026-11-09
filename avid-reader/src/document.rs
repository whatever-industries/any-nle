//! Editable AVB document with stable object IDs and exact unchanged-chunk preservation.
//!
//! This is source-derived converter code, not an Avid compatibility claim. The
//! original byte spans remain authoritative for untouched objects. A changed
//! object must have a bounded typed writer and pass a decoder roundtrip.

use crate::file::{AvbFile, AvbIndex, Object, MAX_FILE_BYTES};
use crate::graph::direct_refs;
use crate::io::{Error, MobId, Reader, Result};
use crate::objects::{Node, Value, KNOWN_CLASSES};
use crate::payload_writer::encode_supported_chunk;
use crate::writer_io::Writer;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone)]
struct Record {
    original_node: Option<Arc<Node>>,
    node: Option<Arc<Node>>,
    // Imported graph chunks own their bytes; loaded chunks use the original index.
    copied_chunk: Option<Vec<u8>>,
}

impl Record {
    fn is_original(&self) -> bool {
        // Compare decoded values, never shared-pointer identity (NaNs are not reflexive).
        self.node.as_deref() == self.original_node.as_deref()
    }
}

/// A bin read from bytes or constructed in memory. Object IDs are one-based and
/// remain stable. Serialization does not silently drop unreachable objects.
/// Original chunks borrow this document's owned index; imported chunks own their
/// bytes independently of the source document.
pub struct AvbDocument {
    original: Option<AvbIndex>,
    records: Vec<Record>,
    root_id: u32,
    big_endian: bool,
    last_save: String,
    last_save_time: u32,
    creator_version: String,
}

/// The three composition mob kinds constructed by py-avb's factory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobKind {
    Composition,
    Master,
    Source,
}

impl MobKind {
    fn stored_id(self) -> i64 {
        match self {
            Self::Composition => 1,
            Self::Master => 2,
            Self::Source => 3,
        }
    }
}

impl AvbDocument {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let bytes = crate::file_io::read_avb(path)?;
        Self::from_bytes(bytes)
    }

    /// Write a validated document to a new path. Existing files are never
    /// overwritten by this convenience method; use `to_bytes` for other IO.
    pub fn write_new(&self, path: &std::path::Path) -> Result<()> {
        use std::io::Write;
        let bytes = self.to_bytes()?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| Error::new(format!("{}: {error}", path.display()), 0))?;
        if let Err(error) = file.write_all(&bytes) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(Error::new(format!("{}: {error}", path.display()), 0));
        }
        Ok(())
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let index = AvbIndex::from_bytes(bytes)?;
        // The index owns the original chunks. Decode directly into records rather
        // than building a temporary extraction table and duplicate raw payloads.
        let mut records = Vec::with_capacity(index.object_count());
        records.push(Record {
            original_node: None,
            node: None,
            copied_chunk: None,
        });
        AvbFile::decode_each(index.original_bytes(), |object| {
            let node = match object {
                Object::Decoded(node) => Some(Arc::new(node)),
                _ => None,
            };
            records.push(Record {
                // Share read-only snapshots; mutable access detaches the editable node.
                original_node: node.clone(),
                node,
                copied_chunk: None,
            });
        })?;
        Ok(Self {
            root_id: index.root_index,
            big_endian: index.big_endian,
            last_save: index.last_save.clone(),
            last_save_time: index.last_save_time,
            creator_version: index.creator_version.clone(),
            original: Some(index),
            records,
        })
    }

    /// Construct the same four-object empty-bin shape as py-avb's default Bin.
    /// The caller supplies an ID and save metadata so tests and conversion jobs
    /// can make reproducible output without a Python runtime or hidden clock.
    pub fn new_empty_bin(
        uid: u64,
        last_save: impl Into<String>,
        last_save_time: u32,
        big_endian: bool,
    ) -> Self {
        let mut doc = Self {
            original: None,
            records: vec![Record {
                original_node: None,
                node: None,
                copied_chunk: None,
            }],
            root_id: 4,
            big_endian,
            last_save: last_save.into(),
            last_save_time,
            creator_version: "avid-reader 0.1.0".into(),
        };
        doc.push_new(attributes_node()); // view attributes: 1
        doc.push_new(view_node()); // view: 2
        doc.push_new(attributes_node()); // bin attributes: 3
        doc.push_new(bin_node(uid)); // root: 4
        doc
    }

    pub fn root_id(&self) -> u32 {
        self.root_id
    }

    pub fn object_count(&self) -> usize {
        self.records.len() - 1
    }

    pub fn big_endian(&self) -> bool {
        self.big_endian
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.records.get(id as usize)?.node.as_deref()
    }

    pub fn root(&self) -> Option<&Node> {
        self.node(self.root_id)
    }

    fn original_chunk(&self, id: u32) -> Option<&[u8]> {
        // Original IDs borrow the retained file; imported IDs own their source chunks.
        self.records.get(id as usize)?
            .copied_chunk.as_deref()
            .or_else(|| self.original.as_ref()?.chunk(id))
    }

    /// Mutating a decoded node is permitted; `to_bytes` checks its writer and
    /// references before emitting anything. Unknown objects are never editable.
    /// Shared decoded data is detached before mutable access.
    pub fn node_mut(&mut self, id: u32) -> Result<&mut Node> {
        self.records
            .get_mut(id as usize)
            .and_then(|record| record.node.as_mut())
            .map(Arc::make_mut)
            .ok_or_else(|| Error::new(format!("object {id} is missing or opaque"), 0))
    }

    pub fn set_field(&mut self, id: u32, name: &str, value: Value) -> Result<()> {
        let max_id = self.object_count() as u32;
        let index = self.original.as_ref();
        let record = self.records
            .get_mut(id as usize)
            .ok_or_else(|| Error::new(format!("object {id} is missing or opaque"), 0))?;
        // Borrow disjoint fields so validation can read source bytes while editing a node.
        let chunk = record.copied_chunk.as_deref().or_else(|| index?.chunk(id));
        if let (Some(original), Some(chunk)) = (&record.original_node, chunk) {
            let rebuilt = encode_supported_chunk(
                original,
                Some(&chunk[8..]),
                self.big_endian,
                Some(max_id),
                MAX_FILE_BYTES as usize,
            )?;
            if rebuilt.as_slice() != chunk {
                return Err(Error::new(
                    format!("object {id} has source-only fields that cannot be preserved"),
                    0,
                ));
            }
        }
        let editable = record.node.as_mut()
            .ok_or_else(|| Error::new(format!("object {id} is missing or opaque"), 0))?;
        let class_name = editable.class_str();
        let field = editable
            .fields
            .iter()
            .position(|(field, _)| *field == name)
            .ok_or_else(|| Error::new(format!("{class_name} has no decoded field {name}"), 0))?;
        let node = Arc::make_mut(editable);
        let previous = std::mem::replace(&mut node.fields[field].1, value);
        let source = chunk.map(|chunk| &chunk[8..]);
        if let Err(error) = encode_supported_chunk(
            node,
            source,
            self.big_endian,
            None,
            MAX_FILE_BYTES as usize,
        ) {
            node.fields[field].1 = previous;
            return Err(Error::new(format!("field {name} cannot be serialized safely: {}", error.message), error.offset));
        }
        Ok(())
    }

    /// Rename a composition, master mob or source mob without exposing the
    /// generic Node field representation to a converter caller.
    pub fn rename_mob(&mut self, id: u32, name: &str) -> Result<()> {
        if name.is_empty() {
            return Err(Error::new("mob name is empty", 0));
        }
        if self.node(id).map(|node| node.class) != Some(*b"CMPO") {
            return Err(Error::new(format!("object {id} is not a composition mob"), 0));
        }
        self.set_field(id, "name", Value::Str(name.to_owned()))
    }

    /// Set the source offset in that SourceClip's own source edit units.
    pub fn set_source_clip_start(&mut self, id: u32, start: i32) -> Result<()> {
        if self.node(id).map(|node| node.class) != Some(*b"SCLP") {
            return Err(Error::new(format!("object {id} is not a source clip"), 0));
        }
        self.set_field(id, "start_time", Value::Int(i64::from(start)))
    }

    /// Set the stored timecode frame count. Drop-frame and display conversion
    /// are separate semantic operations and are not inferred here.
    pub fn set_timecode_start(&mut self, id: u32, start: u32) -> Result<()> {
        if self.node(id).map(|node| node.class) != Some(*b"TCCP") {
            return Err(Error::new(format!("object {id} is not a timecode component"), 0));
        }
        self.set_field(id, "start", Value::Int(i64::from(start)))
    }

    /// Append one typed object at the next stable ID. The object may refer to
    /// existing IDs; forward references must be added before serialization.
    pub fn add_object(&mut self, node: Node) -> Result<u32> {
        if !KNOWN_CLASSES.contains(&&node.class) {
            return Err(Error::new(format!("unknown class {}", node.class_str()), 0));
        }
        if matches!(&node.class, b"ASET" | b"CLIP" | b"COMP" | b"WARP") {
            return Err(Error::new(
                format!(
                    "{} is a base declaration, not a registered file class",
                    node.class_str()
                ),
                0,
            ));
        }
        if node.class == *b"TRKG" {
            return Err(Error::new(
                "bare TRKG cannot be newly constructed readably by pinned py-avb",
                0,
            ));
        }
        let id = u32::try_from(self.records.len())
            .map_err(|_| Error::new("object count exceeds u32", 0))?;
        self.push_new(node);
        Ok(id)
    }

    /// Create a filler component. The edit rate and length are in the track's
    /// edit units; they are never inferred from the container's frame rate.
    pub fn create_filler(&mut self, edit_rate: f64, media_kind_id: i64, length: u32) -> Result<u32> {
        validate_edit_rate(edit_rate)?;
        validate_media_kind(media_kind_id)?;
        let attributes = self.add_object(attributes_node())?;
        let mut node = component_node(*b"FILL", "Filler", edit_rate, media_kind_id, attributes)?;
        node.fields.push(("length", Value::Int(i64::from(length))));
        self.add_object(node)
    }

    pub fn create_timecode(
        &mut self,
        edit_rate: f64,
        length: u32,
        fps: u16,
        start: u32,
    ) -> Result<u32> {
        validate_edit_rate(edit_rate)?;
        if fps == 0 {
            return Err(Error::new("timecode fps must be positive", 0));
        }
        let attributes = self.add_object(attributes_node())?;
        let mut node = component_node(*b"TCCP", "Timecode", edit_rate, 3, attributes)?;
        node.fields.extend([
            ("length", Value::Int(i64::from(length))),
            ("flags", Value::Int(0)),
            ("fps", Value::Int(i64::from(fps))),
            ("start", Value::Int(i64::from(start))),
        ]);
        self.add_object(node)
    }

    pub fn create_source_clip(
        &mut self,
        edit_rate: f64,
        media_kind_id: i64,
        length: u32,
        source_mob: MobId,
        source_track: i16,
        source_start: i32,
    ) -> Result<u32> {
        validate_edit_rate(edit_rate)?;
        validate_media_kind(media_kind_id)?;
        let attributes = self.add_object(attributes_node())?;
        let mut node = component_node(*b"SCLP", "SourceClip", edit_rate, media_kind_id, attributes)?;
        node.fields.extend([
            ("length", Value::Int(i64::from(length))),
            ("track_id", Value::Int(i64::from(source_track))),
            ("start_time", Value::Int(i64::from(source_start))),
            ("mob_id", Value::MobId(source_mob)),
        ]);
        self.add_object(node)
    }

    /// Create a Sequence from existing components. All component media kinds
    /// and edit rates must agree; transition overlap needs a separate API.
    pub fn create_sequence(
        &mut self,
        edit_rate: f64,
        media_kind_id: i64,
        components: &[u32],
    ) -> Result<u32> {
        validate_edit_rate(edit_rate)?;
        validate_media_kind(media_kind_id)?;
        if components.is_empty() {
            return Err(Error::new("sequence requires at least one component", 0));
        }
        for &id in components {
            let component = self.node(id).ok_or_else(|| Error::new(format!("missing component {id}"), 0))?;
            if !matches!(&component.class, b"FILL" | b"SCLP" | b"TCCP" | b"SEQU") {
                return Err(Error::new(format!("object {id} is not a sequence component"), 0));
            }
            if component.get("media_kind_id") != Some(&Value::Int(media_kind_id))
                || component.get("edit_rate") != Some(&Value::Float(edit_rate))
            {
                return Err(Error::new(format!("component {id} has a different media kind or edit rate"), 0));
            }
        }
        let attributes = self.add_object(attributes_node())?;
        let mut node = component_node(*b"SEQU", "Sequence", edit_rate, media_kind_id, attributes)?;
        node.fields.push(("components", Value::RefList(components.iter().copied().map(Value::Ref).collect())));
        self.add_object(node)
    }

    /// Create a mob without tracks, then attach tracks with `append_track`.
    /// The caller supplies the MobID and timestamps for deterministic output.
    pub fn create_composition(
        &mut self,
        name: &str,
        kind: MobKind,
        mob_id: MobId,
        length: u32,
        timestamp: u32,
    ) -> Result<u32> {
        if name.is_empty() {
            return Err(Error::new("composition name is empty", 0));
        }
        let attributes = self.add_object(attributes_node())?;
        let mut node = component_node(*b"CMPO", "Composition", 25.0, 0, attributes)?;
        node.fields.extend([
            ("mc_mode", Value::Int(0)),
            ("length", Value::Int(i64::from(length))),
            ("num_scalars", Value::Int(0)),
            ("tracks", Value::List(vec![])),
            ("last_modified", Value::Time(timestamp)),
            ("mob_type_id", Value::Int(kind.stored_id())),
            ("usage_code", Value::Int(0)),
            ("descriptor", Value::Null),
            ("creation_time", Value::Time(timestamp)),
            ("mob_id", Value::MobId(mob_id)),
        ]);
        node.fields.iter_mut().find(|(key, _)| *key == "name").unwrap().1 = Value::Str(name.to_owned());
        self.add_object(node)
    }

    /// Add a track with py-avb's standard embedded Track shape. This does not
    /// invent a filler proxy; callers can add one when their source needs it.
    pub fn append_track(&mut self, mob_id: u32, index: i16, component_id: u32) -> Result<()> {
        let component = self.node(component_id).ok_or_else(|| Error::new(format!("missing component {component_id}"), 0))?;
        if component.get("media_kind_id").is_none() {
            return Err(Error::new("track component lacks media kind", 0));
        }
        let mob = self.node(mob_id).ok_or_else(|| Error::new(format!("missing mob {mob_id}"), 0))?;
        if mob.class != *b"CMPO" {
            return Err(Error::new("track owner must be a composition", 0));
        }
        let Some(Value::List(mut tracks)) = mob.get("tracks").cloned() else {
            return Err(Error::new("composition has no track list", 0));
        };
        tracks.push(Value::Node(Box::new(Node {
            class: [0; 4],
            type_name: "Track",
            fields: vec![("index", Value::Int(i64::from(index))), ("component", Value::Ref(component_id))],
        })));
        self.set_field(mob_id, "tracks", Value::List(tracks))
    }

    fn push_new(&mut self, node: Node) {
        self.records.push(Record {
            original_node: None,
            node: Some(Arc::new(node)),
            copied_chunk: None,
        });
    }

    pub fn add_bin_item(&mut self, mob_id: u32) -> Result<()> {
        let mob = self
            .node(mob_id)
            .ok_or_else(|| Error::new(format!("missing mob {mob_id}"), 0))?;
        if mob.class != *b"CMPO" {
            return Err(Error::new("bin item must reference a CMPO mob", 0));
        }
        let root = self.root_id;
        let bin = self.node(root).ok_or_else(|| Error::new("document root is missing or opaque", 0))?;
        if bin.class != *b"ABIN" && bin.class != *b"BINF" {
            return Err(Error::new("document root is not a bin", 0));
        }
        let Some(Value::List(mut items)) = bin.get("items").cloned() else {
            return Err(Error::new("bin has no item list", 0));
        };
        items.push(Value::Node(Box::new(Node {
            class: [0; 4],
            type_name: "BinItem",
            fields: vec![
                ("mob", Value::Ref(mob_id)),
                ("x", Value::Int(-30000)),
                ("y", Value::Int(-30000)),
                ("keyframe", Value::Int(0)),
                ("user_placed", Value::Bool(true)),
            ],
        })));
        self.set_field(root, "items", Value::List(items))
    }

    /// Copy a fully decoded reachable subgraph from another document. Opaque or
    /// dangling edges fail before modifying this document. Cycles are preserved.
    pub fn copy_subgraph(&mut self, source: &AvbDocument, start: u32) -> Result<u32> {
        let cross_endian = self.big_endian != source.big_endian;
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = source
                .node(id)
                .ok_or_else(|| Error::new(format!("source object {id} missing or opaque"), 0))?;
            if node_contains_unmapped_bytes(node) {
                return Err(Error::new(format!("source object {id} has opaque byte fields that may contain references"), 0));
            }
            order.push(id);
            stack.extend(direct_refs(node).into_iter().rev());
        }
        let first = u32::try_from(self.records.len())
            .map_err(|_| Error::new("object count exceeds u32", 0))?;
        let mut mapping = BTreeMap::new();
        for (offset, source_id) in order.iter().enumerate() {
            let offset =
                u32::try_from(offset).map_err(|_| Error::new("copy exceeds u32 IDs", 0))?;
            mapping.insert(
                *source_id,
                first
                    .checked_add(offset)
                    .ok_or_else(|| Error::new("copy exceeds u32 IDs", 0))?,
            );
        }
        let mut copies = Vec::new();
        for id in order {
            let mut node = source.node(id).unwrap().clone();
            let source_chunk = if let Some(chunk) = source.original_chunk(id) {
                // First prove the saved source payload can be rebuilt exactly;
                // then serialize any currently edited fields against those
                // source-only bytes. The resulting chunk, not stale saved
                // bytes, is what this copy must transfer.
                if let Some(original) = &source.records[id as usize].original_node {
                    let original_rebuilt = encode_supported_chunk(
                        original,
                        Some(&chunk[8..]),
                        source.big_endian,
                        Some(source.object_count() as u32),
                        MAX_FILE_BYTES as usize,
                    )?;
                    if original_rebuilt.as_slice() != chunk {
                        return Err(Error::new(format!("source object {id} has unpreserved source-only fields"), 0));
                    }
                }
                Some(encode_supported_chunk(
                    &node,
                    Some(&chunk[8..]),
                    source.big_endian,
                    Some(source.object_count() as u32),
                    MAX_FILE_BYTES as usize,
                )?)
            } else {
                None
            };
            if let Some(chunk) = &source_chunk {
                if cross_endian && (!has_transcodable_source_supplement(node.class)
                    || (is_trackgroup_source_supplement(node.class)
                        && matches!(node.get("tracks"), Some(Value::List(tracks)) if tracks.is_empty()))) {
                    // A typed Node may discard source-only bytes (including
                    // alternate encodings of the same numeric value). Only
                    // a byte-exact no-source rebuild proves that it is safe
                    // to write the opposite byte order without those bytes.
                    // An empty TrackGroup can carry an invisible lock-number
                    // extension that its writer cannot recognize after the
                    // byte order changes. Refuse noncanonical empty groups.
                    let canonical = encode_supported_chunk(
                        &node,
                        None,
                        source.big_endian,
                        Some(source.object_count() as u32),
                        MAX_FILE_BYTES as usize,
                    )?;
                    if canonical != *chunk {
                        return Err(Error::new(
                            format!("source object {id} has source-only bytes requiring cross-byte-order transcoding"),
                            0,
                        ));
                    }
                }
            }
            for (_, value) in &mut node.fields {
                remap_refs(value, &mapping)?;
            }
            // A source may be byte-exact before remapping but impossible to
            // serialize afterward (for example a source-only reference or a
            // deliberately refused class). Check the candidate before any
            // target IDs are allocated.
            let target_chunk = if cross_endian {
                if let Some(chunk) = source_chunk.as_ref().filter(|_| has_transcodable_source_supplement(node.class)) {
                    Some(transcode_source_supplement_chunk(
                        &node,
                        &chunk[8..],
                        source.big_endian,
                        self.big_endian,
                        mapping.values().copied().max(),
                    )?)
                } else {
                    encode_supported_chunk(
                        &node,
                        None,
                        self.big_endian,
                        mapping.values().copied().max(),
                        MAX_FILE_BYTES as usize,
                    )?;
                    None
                }
            } else {
                encode_supported_chunk(
                    &node,
                    source_chunk.as_ref().map(|chunk| &chunk[8..]),
                    self.big_endian,
                    mapping.values().copied().max(),
                    MAX_FILE_BYTES as usize,
                )?;
                None
            };
            let node = Arc::new(node);
            copies.push(Record {
                // A transcoded chunk is already framed, remapped and
                // roundtrip-checked in target order; keep it as the baseline
                // for later source-only-preserving edits.
                original_node: target_chunk.as_ref().map(|_| node.clone()),
                node: Some(node),
                copied_chunk: if cross_endian { target_chunk } else { source_chunk },
            });
        }
        self.records.extend(copies);
        Ok(mapping[&start])
    }

    /// Encode without overwriting a source file. Existing object IDs and order
    /// stay fixed; new objects are appended. Unknown chunks remain byte-exact.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let max_id = u32::try_from(self.object_count())
            .map_err(|_| Error::new("object count exceeds u32", 0))?;
        for (id, record) in self.records.iter().enumerate().skip(1) {
            if let Some(node) = &record.node {
                for target in direct_refs(node) {
                    if target == 0
                        || target > max_id
                        || self.records[target as usize].node.is_none()
                            && self.original_chunk(target).is_none()
                    {
                        return Err(Error::new(
                            format!("object {id} has dangling reference {target}"),
                            0,
                        ));
                    }
                }
            }
        }
        if self
            .records
            .iter()
            .enumerate()
            .skip(1)
            .all(|(id, record)| self.original_chunk(id as u32).is_some() && record.is_original())
        {
            if let Some(index) = &self.original {
                return Ok(index.original_bytes().to_vec());
            }
        }
        let mut bytes = if let Some(index) = &self.original {
            let mut header = index.header_bytes().to_vec();
            let offset = count_offset(&header, self.big_endian)?;
            patch_u32(&mut header[offset..offset + 4], max_id, self.big_endian);
            patch_u32(
                &mut header[offset + 4..offset + 8],
                self.root_id,
                self.big_endian,
            );
            header
        } else {
            self.new_header(max_id)?
        };
        for (id, record) in self.records.iter().enumerate().skip(1) {
            let chunk = if record.is_original() {
                self.original_chunk(id as u32)
            } else {
                None
            };
            // Borrow preserved chunks; allocate only when an object is encoded.
            let encoded;
            let chunk = match chunk {
                Some(chunk) => chunk,
                None => {
                    let node = record.node.as_ref().ok_or_else(|| {
                        Error::new(format!("object {id} is opaque and cannot be modified"), 0)
                    })?;
                    let original_chunk = self.original_chunk(id as u32);
                    let source = original_chunk.map(|chunk| &chunk[8..]);
                    if let (Some(previous), Some(chunk)) = (&record.original_node, original_chunk) {
                        let rebuilt = encode_supported_chunk(
                            previous,
                            source,
                            self.big_endian,
                            Some(max_id),
                            MAX_FILE_BYTES as usize,
                        )?;
                        if rebuilt.as_slice() != chunk {
                            return Err(Error::new(format!("object {id} has source-only fields that cannot be preserved"), 0));
                        }
                    }
                    encoded = encode_supported_chunk(
                        node,
                        source,
                        self.big_endian,
                        Some(max_id),
                        MAX_FILE_BYTES as usize,
                    )?;
                    encoded.as_slice()
                }
            };
            if chunk.len() > (MAX_FILE_BYTES as usize).saturating_sub(bytes.len()) {
                return Err(Error::new(
                    "document exceeds AVB file size limit",
                    bytes.len(),
                ));
            }
            bytes.extend_from_slice(chunk);
        }
        if let Some(index) = &self.original {
            let trailer = index.trailing_bytes();
            if trailer.len() > (MAX_FILE_BYTES as usize).saturating_sub(bytes.len()) {
                return Err(Error::new(
                    "document exceeds AVB file size limit",
                    bytes.len(),
                ));
            }
            bytes.extend_from_slice(trailer);
        }
        // Keep the same validation order without a second output buffer. Release
        // the index's object spans before decoding output objects one at a time.
        let bytes = AvbIndex::from_bytes(bytes)?.into_bytes();
        AvbFile::validate(&bytes)?;
        Ok(bytes)
    }

    fn new_header(&self, count: u32) -> Result<Vec<u8>> {
        let mut writer = Writer::with_limit(self.big_endian, MAX_FILE_BYTES as usize);
        writer.bytes(if self.big_endian {
            b"\0\x06"
        } else {
            b"\x06\0"
        })?;
        writer.bytes(b"Domain")?;
        writer.fourcc(*b"OBJD")?;
        writer.string_macroman("AObjDoc")?;
        writer.u8(4)?;
        writer.string_macroman(&self.last_save)?;
        writer.u32(count)?;
        writer.u32(self.root_id)?;
        writer.u32(if self.big_endian {
            0x4d4d_4d4d
        } else {
            0x4949_4949
        })?;
        writer.u32(self.last_save_time)?;
        writer.u32(0)?;
        writer.fourcc(*b"ATob")?;
        writer.fourcc(*b"ATve")?;
        let mut encoded = Writer::new(self.big_endian);
        encoded.string_macroman(&self.creator_version)?;
        let encoded = &encoded.as_bytes()[2..];
        if encoded.len() > 30 {
            return Err(Error::new("creator version exceeds 30 MacRoman bytes", 0));
        }
        writer.u16(30)?;
        writer.bytes(encoded)?;
        writer.bytes(&vec![b' '; 30 - encoded.len()])?;
        writer.bytes(&[0; 16])?;
        Ok(writer.into_bytes())
    }
}

fn has_transcodable_source_supplement(class: [u8; 4]) -> bool {
    matches!(
        &class,
        b"COMP" | b"CLIP" | b"SCLP" | b"SEQU" | b"FILL" | b"TCCP" | b"TRKR"
            | b"PRCL" | b"CTRL" | b"ECCP"
    ) || is_trackgroup_source_supplement(class)
        || matches!(
            &class,
            b"MDES" | b"MDTP" | b"MDFM" | b"MDNG" | b"MDFL" | b"MULD" | b"WAVE"
                | b"AIFC" | b"PCMA" | b"MPGA" | b"DATD" | b"ANCD" | b"DIDD"
                | b"CDCI" | b"MPGI" | b"JPED" | b"RGBA"
        )
}

fn is_trackgroup_source_supplement(class: [u8; 4]) -> bool {
    matches!(
        &class,
        b"RSET" | b"SLCT" | b"TKFX" | b"PVOL" | b"TNFX" | b"CMPO" | b"ASPI"
            | b"EQMB" | b"MASK" | b"STRB" | b"SPED" | b"REPT"
    )
}

fn transcode_source_supplement_chunk(
    node: &Node,
    source_payload: &[u8],
    source_big_endian: bool,
    target_big_endian: bool,
    max_id: Option<u32>,
) -> Result<Vec<u8>> {
    let target_payload = match &node.class {
        b"COMP" | b"CLIP" | b"SCLP" | b"SEQU" | b"FILL" | b"TCCP" | b"TRKR" => {
            let supplement = crate::component_writer::source_only_from_payload(
                node.class,
                source_payload,
                source_big_endian,
            )?;
            crate::component_writer::write_component(
                node,
                target_big_endian,
                supplement,
                MAX_FILE_BYTES as usize,
            )?
        }
        b"PRCL" | b"CTRL" | b"ECCP" => {
            let supplement = crate::curve_writer::source_only_from_payload(
                node.class,
                source_payload,
                source_big_endian,
            )?;
            crate::curve_writer::write_curve(
                node,
                target_big_endian,
                supplement,
                MAX_FILE_BYTES as usize,
            )?
        }
        b"RSET" | b"SLCT" | b"TKFX" | b"PVOL" | b"TNFX" | b"CMPO" | b"ASPI"
        | b"EQMB" | b"MASK" | b"STRB" | b"SPED" | b"REPT" => {
            crate::trackgroup_writer::encode_trackgroup_payload(
                node,
                Some((source_payload, source_big_endian)),
                target_big_endian,
                MAX_FILE_BYTES as usize,
            )?
        }
        b"MDES" | b"MDTP" | b"MDFM" | b"MDNG" | b"MDFL" | b"MULD" | b"WAVE"
        | b"AIFC" | b"PCMA" | b"MPGA" | b"DATD" | b"ANCD" | b"DIDD" | b"CDCI"
        | b"MPGI" | b"JPED" | b"RGBA" => {
            crate::descriptor_writer::encode_descriptor_payload_with_source(
                node,
                Some((source_payload, source_big_endian)),
                target_big_endian,
                MAX_FILE_BYTES as usize,
            )?
        }
        _ => return Err(Error::new("class has no source-supplement transcoder", 0)),
    };
    let chunk = encode_supported_chunk(
        node,
        Some(&target_payload),
        target_big_endian,
        max_id,
        MAX_FILE_BYTES as usize,
    )?;
    if chunk.get(8..) != Some(target_payload.as_slice()) {
        return Err(Error::new(
            "transcoded source supplement did not re-encode exactly",
            0,
        ));
    }
    Ok(chunk)
}

fn count_offset(header: &[u8], big_endian: bool) -> Result<usize> {
    let mut reader = Reader::new(header, 0, big_endian);
    reader.bytes(2)?;
    reader.bytes(6)?;
    reader.fourcc()?;
    reader.string()?;
    reader.assert_tag(4)?;
    reader.string()?;
    let offset = reader.offset();
    reader.bytes(8)?;
    Ok(offset)
}

fn patch_u32(target: &mut [u8], value: u32, big_endian: bool) {
    target.copy_from_slice(&if big_endian {
        value.to_be_bytes()
    } else {
        value.to_le_bytes()
    });
}

fn remap_refs(value: &mut Value, mapping: &BTreeMap<u32, u32>) -> Result<()> {
    match value {
        Value::Ref(id) => {
            *id = *mapping
                .get(id)
                .ok_or_else(|| Error::new(format!("missing copy mapping for reference {id}"), 0))?;
        }
        Value::List(items) | Value::RefList(items) => {
            for item in items {
                remap_refs(item, mapping)?;
            }
        }
        Value::Node(node) => {
            for (_, value) in &mut node.fields {
                remap_refs(value, mapping)?;
            }
        }
        Value::Attrs(entries) => {
            for (_, value) in entries {
                remap_refs(value, mapping)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn node_contains_unmapped_bytes(node: &Node) -> bool {
    node.fields.iter().any(|(name, value)| {
        // Edgecode.header is exactly eight raw film-code bytes; DID-family
        // compress_method is exactly one FourCC. Their pinned readers and
        // writers treat these as raw data, with no reference slots to remap.
        if (node.class == *b"ECCP" && *name == "header"
            && matches!(value, Value::Bytes(bytes) if bytes.len() == 8))
            || (matches!(&node.class, b"DIDD" | b"CDCI" | b"MPGI" | b"JPED" | b"RGBA")
                && *name == "compress_method"
                && matches!(value, Value::Bytes(bytes) if bytes.len() == 4))
        {
            false
        } else {
            contains_opaque_bytes(value)
        }
    })
}

fn contains_opaque_bytes(value: &Value) -> bool {
    match value {
        Value::Bytes(_) => true,
        Value::List(items) | Value::RefList(items) => items.iter().any(contains_opaque_bytes),
        Value::Node(node) => node.fields.iter().any(|(_, item)| contains_opaque_bytes(item)),
        Value::Attrs(entries) => entries.iter().any(|(_, item)| contains_opaque_bytes(item)),
        _ => false,
    }
}

fn validate_edit_rate(edit_rate: f64) -> Result<()> {
    if !edit_rate.is_finite() || edit_rate <= 0.0 {
        return Err(Error::new("edit rate must be finite and positive", 0));
    }
    Ok(())
}

fn validate_media_kind(media_kind_id: i64) -> Result<()> {
    if !(0..=16).contains(&media_kind_id) {
        return Err(Error::new("media kind is outside supported IDs", 0));
    }
    Ok(())
}

fn component_node(
    class: [u8; 4],
    type_name: &'static str,
    edit_rate: f64,
    media_kind_id: i64,
    attributes: u32,
) -> Result<Node> {
    validate_edit_rate(edit_rate)?;
    validate_media_kind(media_kind_id)?;
    Ok(Node {
        class,
        type_name,
        fields: vec![
            ("left_bob", Value::Null),
            ("right_bob", Value::Null),
            ("media_kind_id", Value::Int(media_kind_id)),
            ("edit_rate", Value::Float(edit_rate)),
            ("name", Value::Null),
            ("effect_id", Value::Null),
            ("attributes", Value::Ref(attributes)),
            ("session_attrs", Value::Null),
            ("precomputed", Value::Null),
            ("param_list", Value::Null),
        ],
    })
}

fn attributes_node() -> Node {
    Node {
        class: *b"ATTR",
        type_name: "Attributes",
        fields: vec![("entries", Value::Attrs(vec![]))],
    }
}

fn view_node() -> Node {
    let columns: [(&str, i64, i64); 17] = [
        ("Color", 105, 51),
        ("   ", 0, 200),
        ("Name", 20, 201),
        ("Creation Date", 102, 12),
        ("Duration", 100, 4),
        ("Drive", 2, 14),
        ("IN-OUT", 100, 7),
        ("Mark IN", 100, 5),
        ("Mark OUT", 100, 6),
        ("Tracks", 2, 1),
        ("Start", 100, 2),
        ("Tape", 2, 8),
        ("Video", 2, 13),
        ("Plug-in", 2, 133),
        ("TapeID", 2, 50),
        ("Audio SR", 2, 11),
        ("Comments", 2, 40),
    ];
    Node {
        class: *b"BVst",
        type_name: "BinViewSetting",
        fields: vec![
            ("name", Value::Str("Untitled".into())),
            ("kind", Value::Str("Bin View".into())),
            ("attr_count", Value::Int(1)),
            ("attr_type", Value::Int(20)),
            ("attributes", Value::Ref(1)),
            (
                "columns",
                Value::List(
                    columns
                        .into_iter()
                        .map(|(title, format, kind)| {
                            Value::Node(Box::new(Node {
                                class: [0; 4],
                                type_name: "BinViewColumn",
                                fields: vec![
                                    ("title", Value::Str(title.into())),
                                    ("format", Value::Int(format)),
                                    ("type", Value::Int(kind)),
                                    ("hidden", Value::Bool(false)),
                                ],
                            }))
                        })
                        .collect(),
                ),
            ),
            ("format_descriptors", Value::List(vec![])),
        ],
    }
}

fn bin_node(uid: u64) -> Node {
    Node {
        class: *b"ABIN",
        type_name: "Bin",
        fields: vec![
            ("large_bin", Value::Bool(false)),
            ("view_setting", Value::Ref(2)),
            ("uid", Value::UInt(uid)),
            ("items", Value::List(vec![])),
            ("display_mask", Value::Int(98999)),
            ("display_mode", Value::Int(0)),
            ("sifted", Value::Bool(false)),
            (
                "sifted_settings",
                Value::List(
                    (0..6)
                        .map(|_| {
                            Value::List(vec![
                                Value::Int(1),
                                Value::Str(String::new()),
                                Value::Str("Any".into()),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("sort_columns", Value::List(vec![])),
            ("mac_font", Value::Int(1)),
            ("mac_font_size", Value::Int(11)),
            ("mac_image_scale", Value::Int(5)),
            (
                "home_rect",
                Value::List([0, 0, 300, 600].into_iter().map(Value::Int).collect()),
            ),
            (
                "background_color",
                Value::List([45568, 45568, 45568].into_iter().map(Value::Int).collect()),
            ),
            (
                "forground_color",
                Value::List([3328, 3328, 3328].into_iter().map(Value::Int).collect()),
            ),
            ("ql_image_scale", Value::Int(6)),
            ("attributes", Value::Ref(3)),
            ("was_iconic", Value::Bool(false)),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Uuid;

    #[test]
    fn shared_snapshots_preserve_non_reflexive_node_equality() {
        let node = Arc::new(Node {
            class: *b"ATTR",
            type_name: "Synthetic equality case",
            fields: vec![("entries", Value::Attrs(vec![("nan".into(), Value::Float(f64::NAN))]))],
        });
        let record = Record { original_node: Some(node.clone()), node: Some(node), copied_chunk: None };
        assert!(!record.is_original(), "shared storage must not turn NaN value comparison into equality");
    }
    fn fresh(big_endian: bool) -> AvbDocument {
        AvbDocument::new_empty_bin(0x1234_5678_9abc_def0, "2026/10/20 12:00:00", 0, big_endian)
    }

    #[test]
    fn pinned_pyavb_empty_bin_matches_all_object_chunks() {
        let expected = include_bytes!("../tests/fixtures/pyavb_empty.avb");
        let index = AvbIndex::from_bytes(expected.to_vec()).unwrap();
        let uid = index
            .decode_object(index.root_index)
            .unwrap()
            .unwrap()
            .get("uid")
            .cloned()
            .unwrap();
        let Value::UInt(uid) = uid else {
            panic!("missing bin uid")
        };
        let mut doc = AvbDocument::new_empty_bin(
            uid,
            index.last_save.clone(),
            index.last_save_time,
            index.big_endian,
        );
        doc.creator_version = index.creator_version.clone();
        let actual_bytes = doc.to_bytes().unwrap();
        let actual = AvbIndex::from_bytes(actual_bytes.clone()).unwrap();
        assert_eq!(actual.object_count(), index.object_count());
        for id in 1..index.object_count() as u32 {
            assert_eq!(actual.chunk(id), index.chunk(id), "object {id}");
        }
        assert_eq!(actual_bytes, expected);
    }

    #[test]
    fn empty_bin_builds_and_unchanged_file_is_exact() {
        for big_endian in [false, true] {
            let bytes = fresh(big_endian).to_bytes().unwrap();
            let index = AvbIndex::from_bytes(bytes.clone()).unwrap();
            let file = AvbFile::parse(&bytes).unwrap();
            assert_eq!(index.root_index, 4);
            assert_eq!(index.object_count(), 5);
            assert_eq!(file.root().unwrap().class, *b"ABIN");
            assert_eq!(
                file.node(2).unwrap().get("name"),
                Some(&Value::Str("Untitled".into()))
            );
            assert_eq!(
                AvbDocument::from_bytes(bytes.clone())
                    .unwrap()
                    .to_bytes()
                    .unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn editing_one_view_keeps_other_chunks_byte_exact_and_checks_refs() {
        let original = fresh(false).to_bytes().unwrap();
        let before = AvbIndex::from_bytes(original.clone()).unwrap();
        let mut doc = AvbDocument::from_bytes(original).unwrap();
        doc.set_field(2, "name", Value::Str("Research View".into()))
            .unwrap();
        let changed = doc.to_bytes().unwrap();
        if let Some(path) = std::env::var_os("AVB_EDITED_ORACLE_OUTPUT") {
            std::fs::write(path, &changed).unwrap();
        }
        let after = AvbIndex::from_bytes(changed.clone()).unwrap();
        for id in [1, 3, 4] {
            assert_eq!(before.chunk(id), after.chunk(id), "unchanged object {id}");
        }
        assert_eq!(
            AvbFile::parse(&changed)
                .unwrap()
                .node(2)
                .unwrap()
                .get("name"),
            Some(&Value::Str("Research View".into()))
        );
        doc.set_field(4, "view_setting", Value::Ref(999)).unwrap();
        assert!(doc
            .to_bytes()
            .unwrap_err()
            .message
            .contains("dangling reference"));
    }

    #[test]
    fn copy_subgraph_remaps_shared_references_and_keeps_cycle_guard() {
        let source = fresh(false);
        let mut target = fresh(false);
        let copied_root = target.copy_subgraph(&source, source.root_id()).unwrap();
        assert_eq!(copied_root, 5);
        let copied = target.node(copied_root).unwrap();
        assert_eq!(copied.get("view_setting"), Some(&Value::Ref(6)));
        assert_eq!(copied.get("attributes"), Some(&Value::Ref(8)));
        let encoded = target.to_bytes().unwrap();
        assert_eq!(AvbIndex::from_bytes(encoded).unwrap().object_count(), 9);
    }

    #[test]
    fn unsupported_edit_is_a_specific_error() {
        let mut doc = fresh(false);
        doc.set_field(4, "nonexistent", Value::Int(1)).unwrap_err();
        let original = doc.node(2).unwrap().get("name").cloned();
        let error = doc.set_field(2, "name", Value::Bytes(vec![1])).unwrap_err();
        assert!(error.message.contains("cannot be serialized safely"));
        assert_eq!(doc.node(2).unwrap().get("name"), original.as_ref());
        let id = doc
            .add_object(Node {
                class: *b"MPGP",
                type_name: "MPGPosition",
                fields: vec![],
            })
            .unwrap();
        let error = doc.to_bytes().unwrap_err();
        assert!(error.message.contains("MPGP") || error.message.contains("writer"));
        assert_eq!(id, 5);
    }

    #[test]
    fn unknown_chunk_stays_exact_when_known_object_changes() {
        let mut bytes = fresh(false).to_bytes().unwrap();
        let header_end = AvbIndex::from_bytes(bytes.clone()).unwrap().header_bytes().len();
        let count_at = count_offset(&bytes[..header_end], false).unwrap();
        patch_u32(&mut bytes[count_at..count_at + 4], 5, false);
        let mut unknown = Writer::new(false);
        unknown.fourcc(*b"ZZZZ").unwrap();
        unknown.u32(5).unwrap();
        unknown.bytes(&[0, 1, 2, 3, 4]).unwrap();
        let unknown = unknown.into_bytes();
        bytes.extend_from_slice(&unknown);
        let mut doc = AvbDocument::from_bytes(bytes).unwrap();
        assert!(doc.node(5).is_none());
        doc.set_field(2, "name", Value::Str("Renamed".into())).unwrap();
        let after = AvbIndex::from_bytes(doc.to_bytes().unwrap()).unwrap();
        assert_eq!(after.chunk(5), Some(unknown.as_slice()));
    }

    #[test]
    fn opaque_copy_and_unsupported_copy_fail_without_allocating_ids() {
        let mut bytes = fresh(false).to_bytes().unwrap();
        let header_end = AvbIndex::from_bytes(bytes.clone()).unwrap().header_bytes().len();
        let count_at = count_offset(&bytes[..header_end], false).unwrap();
        patch_u32(&mut bytes[count_at..count_at + 4], 5, false);
        let mut unknown = Writer::new(false);
        unknown.fourcc(*b"ZZZZ").unwrap();
        unknown.u32(1).unwrap();
        unknown.u8(0x45).unwrap();
        bytes.extend_from_slice(&unknown.into_bytes());
        let mut source = AvbDocument::from_bytes(bytes).unwrap();
        source.set_field(1, "entries", Value::Attrs(vec![("opaque".into(), Value::Ref(5))])).unwrap();
        let mut target = fresh(false);
        let count = target.object_count();
        let error = target.copy_subgraph(&source, 1).unwrap_err();
        assert!(error.message.contains("opaque"));
        assert_eq!(target.object_count(), count);

        let mut opposite_order = fresh(true);
        let error = opposite_order.copy_subgraph(&source, 1).unwrap_err();
        assert!(error.message.contains("opaque"));
        assert_eq!(opposite_order.object_count(), count);

        let mut payload_source = fresh(false);
        payload_source.set_field(1, "entries", Value::Attrs(vec![("raw".into(), Value::Bytes(vec![5, 0, 0, 0]))])).unwrap();
        let error = target.copy_subgraph(&payload_source, 1).unwrap_err();
        assert!(error.message.contains("opaque byte fields"));
        assert_eq!(target.object_count(), count);

        let mut malformed_source = fresh(false);
        let filler = malformed_source.create_filler(24.0, 1, 24).unwrap();
        malformed_source
            .node_mut(filler)
            .unwrap()
            .fields
            .iter_mut()
            .find(|(key, _)| *key == "length")
            .unwrap()
            .1 = Value::Str("not a length".into());
        let error = target.copy_subgraph(&malformed_source, filler).unwrap_err();
        assert!(error.message.contains("length"));
        assert_eq!(target.object_count(), count);
    }

    #[test]
    fn reference_cycle_copies_once_and_dangling_reference_rejects() {
        let mut source = fresh(false);
        source.set_field(1, "entries", Value::Attrs(vec![("loop".into(), Value::Ref(1))])).unwrap();
        let mut target = fresh(false);
        let copied = target.copy_subgraph(&source, 1).unwrap();
        assert_eq!(target.node(copied).unwrap().get("entries"),
            Some(&Value::Attrs(vec![("loop".into(), Value::Ref(copied))])));
        target.to_bytes().unwrap();
        source.set_field(1, "entries", Value::Attrs(vec![("bad".into(), Value::Ref(999))])).unwrap();
        assert!(source.to_bytes().unwrap_err().message.contains("dangling reference"));
    }

    #[test]
    fn typed_edits_roundtrip_and_reject_wrong_class() {
        let mut doc = fresh(false);
        let mob_id = MobId::unique_with_material(Uuid { data1: 7, data2: 8, data3: 9, data4: [1; 8] });
        let clip = doc.create_source_clip(24.0, 1, 48, mob_id, 1, 0).unwrap();
        let tc = doc.create_timecode(24.0, 48, 24, 0).unwrap();
        let mob = doc.create_composition("Before", MobKind::Master, mob_id, 48, 0).unwrap();
        doc.rename_mob(mob, "After").unwrap();
        doc.set_source_clip_start(clip, 12).unwrap();
        doc.set_timecode_start(tc, 86_400).unwrap();
        assert!(doc.rename_mob(clip, "Wrong").is_err());
        assert!(doc.set_source_clip_start(tc, 1).is_err());
        let bytes = doc.to_bytes().unwrap();
        let reopened = AvbDocument::from_bytes(bytes).unwrap();
        assert_eq!(reopened.node(mob).unwrap().get("name"), Some(&Value::Str("After".into())));
        assert_eq!(reopened.node(clip).unwrap().get("start_time"), Some(&Value::Int(12)));
        assert_eq!(reopened.node(tc).unwrap().get("start"), Some(&Value::Int(86_400)));
    }

    #[test]
    fn rejected_component_creation_does_not_allocate_orphans() {
        let mut doc = fresh(false);
        let count = doc.object_count();
        assert!(doc.create_filler(f64::NAN, 1, 24).is_err());
        assert!(doc.create_filler(24.0, 17, 24).is_err());
        assert!(doc.create_timecode(24.0, 24, 0, 0).is_err());
        let source_mob = MobId::unique_with_material(Uuid { data1: 0, data2: 0, data3: 0, data4: [0; 8] });
        assert!(doc.create_source_clip(24.0, -1, 24, source_mob, 1, 0).is_err());
        assert!(doc.create_sequence(0.0, 1, &[1]).is_err());
        assert!(doc.create_sequence(24.0, 17, &[1]).is_err());
        assert_eq!(doc.object_count(), count);
    }

    #[test]
    fn create_master_clip_and_sequence_mob_in_both_byte_orders() {
        for big_endian in [false, true] {
            let mut doc = fresh(big_endian);
            let source_mob = MobId::unique_with_material(Uuid {
                data1: 0x1234_5678,
                data2: 0x9abc,
                data3: 0xdef0,
                data4: [1, 2, 3, 4, 5, 6, 7, 8],
            });
            let master_id = MobId::unique_with_material(Uuid {
                data1: 0x1234_5679,
                data2: 0x9abc,
                data3: 0xdef0,
                data4: [1, 2, 3, 4, 5, 6, 7, 8],
            });
            let sequence_id = MobId::unique_with_material(Uuid {
                data1: 0x1234_5680,
                data2: 0x9abc,
                data3: 0xdef0,
                data4: [1, 2, 3, 4, 5, 6, 7, 8],
            });
            let clip = doc.create_source_clip(24.0, 1, 48, source_mob, 1, 0).unwrap();
            let master = doc.create_composition("Master", MobKind::Master, master_id, 48, 1_700_000_000).unwrap();
            doc.append_track(master, 1, clip).unwrap();
            doc.add_bin_item(master).unwrap();
            let fill = doc.create_filler(24.0, 1, 12).unwrap();
            let edit = doc.create_source_clip(24.0, 1, 36, master_id, 1, 12).unwrap();
            let video = doc.create_sequence(24.0, 1, &[fill, edit]).unwrap();
            let tc = doc.create_timecode(24.0, 48, 24, 86_400).unwrap();
            let sequence = doc.create_composition("Sequence", MobKind::Composition, sequence_id, 48, 1_700_000_000).unwrap();
            doc.append_track(sequence, 1, tc).unwrap();
            doc.append_track(sequence, 1, video).unwrap();
            doc.add_bin_item(sequence).unwrap();
            let bytes = doc.to_bytes().unwrap();
            if !big_endian {
                if let Some(path) = std::env::var_os("AVB_SYNTHETIC_ORACLE_OUTPUT") {
                    std::fs::write(path, &bytes).unwrap();
                }
            } else if let Some(path) = std::env::var_os("AVB_SYNTHETIC_ORACLE_OUTPUT_BE") {
                std::fs::write(path, &bytes).unwrap();
            }
            let index = AvbIndex::from_bytes(bytes.clone()).unwrap();
            assert_eq!(index.big_endian, big_endian);
            let parsed = AvbFile::parse(&bytes).unwrap();
            assert_eq!(parsed.node(master).unwrap().get("name"), Some(&Value::Str("Master".into())));
            assert_eq!(parsed.node(sequence).unwrap().get("name"), Some(&Value::Str("Sequence".into())));
            assert_eq!(parsed.node(video).unwrap().get("components"), Some(&Value::RefList(vec![Value::Ref(fill), Value::Ref(edit)])));
            assert_eq!(AvbDocument::from_bytes(bytes.clone()).unwrap().to_bytes().unwrap(), bytes);
            let opened = AvbDocument::from_bytes(bytes).unwrap();
            let mut copied = fresh(big_endian);
            let copy_id = copied.copy_subgraph(&opened, master).unwrap();
            copied.add_bin_item(copy_id).unwrap();
            let copy_bytes = copied.to_bytes().unwrap();
            let copy_file = AvbFile::parse(&copy_bytes).unwrap();
            assert_eq!(copy_file.node(copy_id).unwrap().get("name"), Some(&Value::Str("Master".into())));
        }
    }
}
