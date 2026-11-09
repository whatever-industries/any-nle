//! Source-derived document operations that sit above the byte-preserving writer.
//!
//! py-avb's `AVBFactory` registers classes by Python name and `AVBRefList`
//! marks its parent modified on mutation (core.py/file.py, ff64e06). This Rust
//! API uses stable IDs and immediate serializability checks instead of dynamic
//! attributes or Python's modified-object cache. The class-name factory below
//! exposes only defaults that the pinned py-avb writer can serialize.

use crate::document::AvbDocument;
use crate::file::MAX_FILE_BYTES;
use crate::io::{Error, MobId, Result, Uuid};
use crate::objects::{Node, Value};
use crate::payload_writer::encode_supported_chunk;

fn fail(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}

/// Remaining registered py-avb factory names without a safe no-argument Rust
/// equivalent. These are source-required fields, nondeterministic constructor
/// state, or a pinned reader/writer contradiction; explicit-data constructors
/// are separate from the no-argument default factory.
pub const UNSUPPORTED_DEFAULT_FACTORIES: [(&str, &str); 46] = [
    ("AIFCDescriptor", "requires audio summary bytes"),
    (
        "ANCDataDescriptor",
        "requires data index and manifest fields",
    ),
    (
        "AudioSuitePluginEffect",
        "requires effect-specific parameters",
    ),
    ("BOBPosition", "requires source MobID and position fields"),
    ("BinRef", "requires bin UID fields"),
    ("CFUserParam", "requires UUID and opaque parameter data"),
    ("CaptureMask", "requires mask and phase parameters"),
    ("ColorCorrectionEffect", "requires color-correction payload"),
    (
        "Composition",
        "constructor generates UUIDv4 MobID and timestamps; use explicit-data constructor",
    ),
    (
        "ControlClip",
        "requires interpolation and control-point data",
    ),
    ("DIDPosition", "requires source MobID and position fields"),
    ("DataDescriptor", "requires frame-index offset fields"),
    ("Edgecode", "requires edgecode header and start values"),
    (
        "EffectParamList",
        "requires effect payload lengths and data",
    ),
    ("EqualizerMultiBand", "requires effect-specific parameters"),
    ("GraphicEffect", "requires picture data"),
    ("JPEGDescriptor", "requires JPEG table and image fields"),
    ("MPGADescriptor", "requires audio channel and codec fields"),
    ("MPGIDescriptor", "requires MPEG version and image fields"),
    ("MPGPosition", "requires source MobID and position fields"),
    (
        "MSMLocator",
        "requires MobID and volume; use explicit-data constructor",
    ),
    (
        "MacFileLocator",
        "requires path; use explicit-data constructor",
    ),
    ("Marker", "requires source MobID and marker fields"),
    ("MobRef", "requires source MobID"),
    ("MotionEffect", "requires phase and speed parameters"),
    ("MultiDescriptor", "requires descriptor references"),
    ("PCMADescriptor", "requires audio channel and format fields"),
    ("PanVolumeEffect", "requires effect-specific parameters"),
    ("ParamClip", "requires interpolation and control-point data"),
    ("ParameterItem", "requires UUID and parameter value"),
    ("Position", "requires source MobID"),
    ("RGBADescriptor", "requires pixel layout and image fields"),
    ("Repeat", "requires phase parameter"),
    ("Selector", "requires selected track and ganging state"),
    ("ShapeList", "requires shape data"),
    ("StrobeEffect", "requires phase and strobe parameters"),
    ("TrackEffect", "requires effect-specific parameters"),
    (
        "TrackGroup",
        "pinned bare writer output fails its own reader",
    ),
    ("TrackerData", "requires tracking settings and samples"),
    ("TrackerDataSlot", "requires tracker-data references"),
    ("TrackerManager", "requires data and parameter slots"),
    ("TrackerParameter", "requires tracking parameter settings"),
    (
        "TrackerParameterSlot",
        "requires tracking parameter references",
    ),
    (
        "TransitionEffect",
        "requires cutpoint and effect parameters",
    ),
    ("WaveDescriptor", "requires audio summary bytes"),
    (
        "WinFileLocator",
        "requires path; use explicit-data constructor",
    ),
];

pub fn unsupported_default_reason(class_name: &str) -> Option<&'static str> {
    UNSUPPORTED_DEFAULT_FACTORIES
        .iter()
        .find_map(|(name, reason)| (*name == class_name).then_some(*reason))
}

/// Typed, bounded construction for the 44 registered classes whose pinned
/// no-argument py-avb objects are incomplete. The caller supplies declared
/// fields; the class writer verifies required fields, concrete value types,
/// ranges and roundtrip semantics before this allocates an object ID.
///
/// Some effects additionally need inline helper nodes or opaque byte payloads.
/// Those remain typed `Value` data supplied by the caller, not invented
/// defaults. This is source parity, not Media Composer acceptance.
pub struct RequiredObjectBuilder {
    class: [u8; 4],
    type_name: &'static str,
    fields: Vec<(&'static str, Value)>,
}

impl RequiredObjectBuilder {
    pub fn new(class_name: &str) -> Result<Self> {
        if unsupported_default_reason(class_name).is_none() {
            return Err(fail(format!(
                "{class_name} is not a required-input registered factory"
            )));
        }
        if matches!(class_name, "Composition" | "TrackGroup") {
            return Err(fail(format!(
                "{class_name} has a separate safe construction policy"
            )));
        }
        let entry = crate::class_registry::REGISTERED_CLASSES
            .iter()
            .find(|entry| entry.class_name == class_name)
            .ok_or_else(|| fail(format!("{class_name} is not runtime-registered")))?;
        Ok(Self {
            class: entry.fourcc,
            type_name: entry.class_name,
            fields: Vec::new(),
        })
    }

    pub fn set(&mut self, name: &str, value: Value) -> Result<&mut Self> {
        let canonical = crate::property_registry::property_definition(self.class, name)
            .map(|definition| definition.name)
            .or_else(|| {
                // ASPI stores two low-level MobID words alongside its declared
                // MobID. The reader exposes both and its writer requires them.
                (self.class == *b"ASPI" && matches!(name, "mob_hi" | "mob_lo")).then(|| {
                    if name == "mob_hi" {
                        "mob_hi"
                    } else {
                        "mob_lo"
                    }
                })
            })
            .ok_or_else(|| fail(format!("{} has no registered field {name}", self.type_name)))?;
        if self
            .fields
            .iter()
            .any(|(existing, _)| *existing == canonical)
        {
            return Err(fail(format!(
                "{}.{canonical} is duplicated",
                self.type_name
            )));
        }
        self.fields.push((canonical, value));
        Ok(self)
    }

    /// Reject an incomplete, dangling or writer-unsupported candidate before
    /// assigning an ID. This does not infer references or default values.
    pub fn add_to(self, document: &mut AvbDocument) -> Result<u32> {
        let node = Node {
            class: self.class,
            type_name: self.type_name,
            fields: self.fields,
        };
        for reference in crate::graph::direct_refs(&node) {
            if reference == 0 || document.node(reference).is_none() {
                return Err(fail(format!(
                    "{} references missing or opaque object {reference}",
                    self.type_name
                )));
            }
        }
        let max_object_id = u32::try_from(document.object_count())
            .map_err(|_| fail("document object count exceeds u32"))?;
        encode_supported_chunk(
            &node,
            None,
            document.big_endian(),
            Some(max_object_id),
            MAX_FILE_BYTES as usize,
        )
        .map_err(|error| {
            fail(format!(
                "{} required fields are incomplete or invalid: {}",
                self.type_name, error.message
            ))
        })?;
        document.add_object(node)
    }
}

/// The two registered file locator classes. `FILE` is registered under
/// `MacFileLocator` in pinned py-avb; its unregistered FileLocator base is not
/// offered as an independently constructible file object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileLocatorKind {
    Mac,
    Windows,
}

/// A native path plus optional source-recorded alternate path spellings.
/// The required `path` uses AVB's MacRoman string format; UTF-8 extensions
/// are supplied separately and do not override it.
#[derive(Debug, Clone, Copy)]
pub struct FileLocatorPaths<'a> {
    pub path: &'a str,
    pub path_posix: Option<&'a str>,
    pub path_utf8: Option<&'a str>,
    pub path2_utf8: Option<&'a str>,
}

/// Build a FILE or WINF object from caller-supplied path data. All strings
/// are validated by the bounded writer before an ID is allocated.
pub fn create_file_locator(
    document: &mut AvbDocument,
    kind: FileLocatorKind,
    paths: FileLocatorPaths<'_>,
) -> Result<u32> {
    let (class, type_name) = match kind {
        FileLocatorKind::Mac => (*b"FILE", "MacFileLocator"),
        FileLocatorKind::Windows => (*b"WINF", "WinFileLocator"),
    };
    let mut node = Node {
        class,
        type_name,
        fields: vec![("path", Value::Str(paths.path.to_owned()))],
    };
    for (name, value) in [
        ("path_posix", paths.path_posix),
        ("path_utf8", paths.path_utf8),
        ("path2_utf8", paths.path2_utf8),
    ] {
        if let Some(value) = value {
            node.fields.push((name, Value::Str(value.to_owned())));
        }
    }
    preflight_default(document, &node)?;
    document.add_object(node)
}

/// Build an MSML object with an explicit MobID and source-recorded volume.
/// The optional domain and UTF-8 volume extension are stored only if supplied.
pub fn create_msm_locator(
    document: &mut AvbDocument,
    last_known_volume: &str,
    mob_id: MobId,
    domain_type: Option<i32>,
    last_known_volume_utf8: Option<&str>,
) -> Result<u32> {
    let mut node = Node {
        class: *b"MSML",
        type_name: "MSMLocator",
        fields: vec![(
            "last_known_volume",
            Value::Str(last_known_volume.to_owned()),
        )],
    };
    if let Some(domain_type) = domain_type {
        node.fields
            .push(("domain_type", Value::Int(i64::from(domain_type))));
    }
    node.fields.push(("mob_id", Value::MobId(mob_id)));
    if let Some(volume) = last_known_volume_utf8 {
        node.fields
            .push(("last_known_volume_utf8", Value::Str(volume.to_owned())));
    }
    preflight_default(document, &node)?;
    document.add_object(node)
}

/// Construct a bin reference from explicit signed UID halves and both stored
/// name spellings. Pinned py-avb's MCBR writer always emits the UTF-8 name
/// extension, so omitting it here would invent an unsupported default.
pub fn create_bin_ref(
    document: &mut AvbDocument,
    uid_high: i32,
    uid_low: i32,
    name: &str,
    name_utf8: &str,
) -> Result<u32> {
    let node = Node {
        class: *b"MCBR",
        type_name: "BinRef",
        fields: vec![
            ("uid_high", Value::Int(i64::from(uid_high))),
            ("uid_low", Value::Int(i64::from(uid_low))),
            ("name", Value::Str(name.to_owned())),
            ("name_utf8", Value::Str(name_utf8.to_owned())),
        ],
    };
    preflight_default(document, &node)?;
    document.add_object(node)
}

/// Construct MCMR with caller-supplied identity and signed position.
pub fn create_mob_ref(document: &mut AvbDocument, mob_id: MobId, position: i32) -> Result<u32> {
    let node = Node {
        class: *b"MCMR",
        type_name: "MobRef",
        fields: vec![
            ("position", Value::Int(i64::from(position))),
            ("mob_id", Value::MobId(mob_id)),
        ],
    };
    preflight_default(document, &node)?;
    document.add_object(node)
}

/// Construct a basic APOS with explicit MobID; no source mob is inferred.
pub fn create_position(document: &mut AvbDocument, mob_id: MobId) -> Result<u32> {
    let node = Node {
        class: *b"APOS",
        type_name: "Position",
        fields: vec![("mob_id", Value::MobId(mob_id))],
    };
    preflight_default(document, &node)?;
    document.add_object(node)
}

/// A conservative subset of py-avb's registered-class factory. Unsupported
/// names fail before an object ID is allocated; no incomplete default object
/// is inserted into the document. Constructor defaults are not claims that
/// these nodes make useful Avid media without additional fields.
pub fn create_with_defaults(document: &mut AvbDocument, class_name: &str) -> Result<u32> {
    let node = match class_name {
        "Attributes" => Node {
            class: *b"ATTR",
            type_name: "Attributes",
            fields: vec![("entries", Value::Attrs(Vec::new()))],
        },
        "ParameterList" => empty_ref_list(*b"PRLS", "ParameterList"),
        "TimeCrumbList" => empty_ref_list(*b"TMCS", "TimeCrumbList"),
        "BinViewSetting" => {
            let template =
                AvbDocument::new_empty_bin(0, "Factory template", 0, document.big_endian());
            return document.copy_subgraph(&template, 2);
        }
        "BinFirst" => {
            // py-avb's BinFirst inherits Bin's six sift settings and two
            // referenced children, then adds one zero-valued trailing word.
            let mut template =
                AvbDocument::new_empty_bin(0, "Factory template", 0, document.big_endian());
            let root = template.root_id();
            let bin = template.node_mut(root)?;
            bin.class = *b"BINF";
            bin.type_name = "BinFirst";
            bin.fields.push(("unknown_s32", Value::Int(0)));
            return document.copy_subgraph(&template, root);
        }
        "Bin" => {
            // py-avb's no-argument Bin constructor generates a random UID.
            // Use a caller-visible deterministic zero UID for the same shape;
            // callers needing a real bin identity should construct it explicitly.
            let template =
                AvbDocument::new_empty_bin(0, "Factory template", 0, document.big_endian());
            return document.copy_subgraph(&template, template.root_id());
        }
        "URLLocator" => Node {
            class: *b"URLL",
            type_name: "URLLocator",
            fields: vec![],
        },
        "MediaDescriptor" => descriptor_default(*b"MDES", "MediaDescriptor"),
        "TapeDescriptor" => descriptor_default(*b"MDTP", "TapeDescriptor"),
        "FilmDescriptor" => descriptor_default(*b"MDFM", "FilmDescriptor"),
        "NagraDescriptor" => descriptor_default(*b"MDNG", "NagraDescriptor"),
        "MediaFileDescriptor" => descriptor_default(*b"MDFL", "MediaFileDescriptor"),
        "DIDDescriptor" => video_descriptor_default(*b"DIDD", "DIDDescriptor"),
        "CDCIDescriptor" => video_descriptor_default(*b"CDCI", "CDCIDescriptor"),
        "Filler" => return document.create_filler(25.0, 0, 0),
        "SourceClip" => {
            // SourceClip.__init__ uses MobID(), whose 32 bytes are all zero.
            let zero_mob = MobId {
                label: [0; 12],
                length: 0,
                instance: [0; 3],
                material: Uuid {
                    data1: 0,
                    data2: 0,
                    data3: 0,
                    data4: [0; 8],
                },
            };
            return document.create_source_clip(25.0, 0, 0, zero_mob, 0, 0);
        }
        "Timecode" => return create_component_default(document, *b"TCCP", "Timecode"),
        "TrackRef" => return create_component_default(document, *b"TRKR", "TrackRef"),
        "Sequence" => return create_component_default(document, *b"SEQU", "Sequence"),
        "EssenceGroup" => return create_component_default(document, *b"RSET", "EssenceGroup"),
        _ => {
            let reason =
                unsupported_default_reason(class_name).unwrap_or("unknown or unregistered class");
            return Err(fail(format!(
                "factory defaults not established for {class_name}: {reason}"
            )));
        }
    };
    preflight_default(document, &node)?;
    document.add_object(node)
}

fn empty_ref_list(class: [u8; 4], type_name: &'static str) -> Node {
    Node {
        class,
        type_name,
        fields: vec![("items", Value::RefList(Vec::new()))],
    }
}

fn descriptor_default(class: [u8; 4], type_name: &'static str) -> Node {
    let mut fields = vec![
        ("mob_kind", Value::Int(0)),
        ("locator", Value::Null),
        ("intermediate", Value::Bool(false)),
        ("physical_media", Value::Null),
    ];
    if class == *b"MDTP" {
        fields.push(("cframe", Value::Int(0)));
    }
    if matches!(&class, b"MDFL" | b"DIDD" | b"CDCI") {
        fields.extend([
            ("edit_rate", Value::Float(25.0)),
            ("length", Value::Int(0)),
            ("is_omfi", Value::Int(0)),
            ("data_offset", Value::Int(0)),
        ]);
    }
    Node {
        class,
        type_name,
        fields,
    }
}

fn video_descriptor_default(class: [u8; 4], type_name: &'static str) -> Node {
    let mut node = descriptor_default(class, type_name);
    node.fields.extend([
        ("stored_height", Value::Int(1080)),
        ("stored_width", Value::Int(1920)),
        ("sampled_height", Value::Int(1080)),
        ("sampled_width", Value::Int(1920)),
        ("sampled_x_offset", Value::Int(0)),
        ("sampled_y_offset", Value::Int(0)),
        ("display_height", Value::Int(1080)),
        ("display_width", Value::Int(1920)),
        ("display_x_offset", Value::Int(0)),
        ("display_y_offset", Value::Int(0)),
        ("frame_layout", Value::Int(0)),
        (
            "aspect_ratio",
            Value::List(vec![Value::Int(16), Value::Int(9)]),
        ),
        ("line_map", Value::List(vec![Value::Int(42), Value::Int(0)])),
        ("alpha_transparency", Value::Int(0)),
        ("uniformness", Value::Bool(false)),
        ("did_image_size", Value::Int(108_511_232)),
        ("next_did_desc", Value::Null),
        ("compress_method", Value::Bytes(b"AVHD".to_vec())),
        ("resolution_id", Value::Int(1237)),
        ("image_alignment_factor", Value::Int(8192)),
    ]);
    if class == *b"CDCI" {
        node.fields.extend([
            ("horizontal_subsampling", Value::Int(2)),
            ("vertical_subsampling", Value::Int(1)),
            ("component_width", Value::Int(8)),
            ("color_sitting", Value::Int(4)),
            ("black_ref_level", Value::Int(16)),
            ("white_ref_level", Value::Int(235)),
            ("color_range", Value::Int(255)),
            ("frame_index_offset", Value::Int(0)),
        ]);
    }
    node
}

fn component_default(class: [u8; 4], type_name: &'static str, attributes: u32) -> Node {
    let mut fields = vec![
        ("left_bob", Value::Null),
        ("right_bob", Value::Null),
        ("media_kind_id", Value::Int(0)),
        ("edit_rate", Value::Float(25.0)),
        ("name", Value::Null),
        ("effect_id", Value::Null),
        ("attributes", Value::Ref(attributes)),
        ("session_attrs", Value::Null),
        ("precomputed", Value::Null),
        ("param_list", Value::Null),
    ];
    match &class {
        b"TCCP" => fields.extend([
            ("length", Value::Int(0)),
            ("flags", Value::Int(0)),
            ("fps", Value::Int(25)),
            ("start", Value::Int(0)),
        ]),
        b"TRKR" => fields.extend([
            ("length", Value::Int(0)),
            ("relative_scope", Value::Int(0)),
            ("relative_track", Value::Int(-1)),
        ]),
        b"SEQU" => fields.push(("components", Value::RefList(Vec::new()))),
        b"RSET" => fields.extend([
            ("mc_mode", Value::Int(0)),
            ("length", Value::Int(0)),
            ("num_scalars", Value::Int(0)),
            ("tracks", Value::List(Vec::new())),
        ]),
        _ => unreachable!(),
    }
    Node {
        class,
        type_name,
        fields,
    }
}

fn create_component_default(
    document: &mut AvbDocument,
    class: [u8; 4],
    type_name: &'static str,
) -> Result<u32> {
    let attrs_id =
        u32::try_from(document.object_count() + 1).map_err(|_| fail("object count exceeds u32"))?;
    let node = component_default(class, type_name, attrs_id);
    preflight_default(document, &node)?;
    let actual = create_with_defaults(document, "Attributes")?;
    debug_assert_eq!(actual, attrs_id);
    document.add_object(node)
}

fn preflight_default(document: &AvbDocument, node: &Node) -> Result<()> {
    encode_supported_chunk(
        node,
        None,
        document.big_endian(),
        None,
        MAX_FILE_BYTES as usize,
    )?;
    Ok(())
}

/// An explicit-ID analogue to py-avb's dereferencing and AVBRefList mutation.
/// All mutations are transactional: a failed writer/readback restores the field.
pub trait DocumentRefExt {
    /// Resolve a stored reference, rejecting missing and opaque targets.
    fn resolve_value_ref(&self, value: &Value) -> Result<Option<&Node>>;

    /// Resolve one named scalar reference field on an object.
    fn resolve_field_ref(&self, owner: u32, field: &str) -> Result<Option<&Node>>;

    /// Insert, replace or remove one entry in a typed reference list.
    fn insert_list_ref(&mut self, owner: u32, field: &str, index: usize, target: u32)
        -> Result<()>;
    fn replace_list_ref(
        &mut self,
        owner: u32,
        field: &str,
        index: usize,
        target: u32,
    ) -> Result<u32>;
    fn remove_list_ref(&mut self, owner: u32, field: &str, index: usize) -> Result<u32>;

    /// Set a scalar reference field. `None` writes a null object reference.
    fn set_scalar_ref(&mut self, owner: u32, field: &str, target: Option<u32>) -> Result<()>;
}

impl DocumentRefExt for AvbDocument {
    fn resolve_value_ref(&self, value: &Value) -> Result<Option<&Node>> {
        match value {
            Value::Null => Ok(None),
            Value::Ref(id) if *id != 0 => self
                .node(*id)
                .map(Some)
                .ok_or_else(|| fail(format!("reference {id} is missing or opaque"))),
            Value::Ref(_) => Err(fail("zero is not a valid stored Ref; use Null")),
            _ => Err(fail("value is not an object reference")),
        }
    }

    fn resolve_field_ref(&self, owner: u32, field: &str) -> Result<Option<&Node>> {
        let node = self
            .node(owner)
            .ok_or_else(|| fail(format!("object {owner} is missing or opaque")))?;
        let value = node
            .get(field)
            .ok_or_else(|| fail(format!("{} has no field {field}", node.class_str())))?;
        self.resolve_value_ref(value)
    }

    fn insert_list_ref(
        &mut self,
        owner: u32,
        field: &str,
        index: usize,
        target: u32,
    ) -> Result<()> {
        checked_target(self, target)?;
        let original = list_field(self, owner, field)?;
        if index > original.len() {
            return Err(fail(format!(
                "insert index {index} exceeds list length {}",
                original.len()
            )));
        }
        check_sequence_component(self, owner, field, target)?;
        let mut edited = original.clone();
        edited.insert(index, Value::Ref(target));
        commit_list(self, owner, field, original, edited)
    }

    fn replace_list_ref(
        &mut self,
        owner: u32,
        field: &str,
        index: usize,
        target: u32,
    ) -> Result<u32> {
        checked_target(self, target)?;
        let original = list_field(self, owner, field)?;
        let old = original
            .get(index)
            .ok_or_else(|| fail(format!("replace index {index} is out of range")))?;
        let Value::Ref(old_id) = old else {
            return Err(fail("reference list contains a non-reference entry"));
        };
        let old_id = *old_id;
        check_sequence_component(self, owner, field, target)?;
        let mut edited = original.clone();
        edited[index] = Value::Ref(target);
        commit_list(self, owner, field, original, edited)?;
        Ok(old_id)
    }

    fn remove_list_ref(&mut self, owner: u32, field: &str, index: usize) -> Result<u32> {
        let original = list_field(self, owner, field)?;
        let old = original
            .get(index)
            .ok_or_else(|| fail(format!("remove index {index} is out of range")))?;
        let Value::Ref(old_id) = old else {
            return Err(fail("reference list contains a non-reference entry"));
        };
        let old_id = *old_id;
        let mut edited = original.clone();
        edited.remove(index);
        commit_list(self, owner, field, original, edited)?;
        Ok(old_id)
    }

    fn set_scalar_ref(&mut self, owner: u32, field: &str, target: Option<u32>) -> Result<()> {
        if let Some(target) = target {
            checked_target(self, target)?;
        }
        let old = self
            .node(owner)
            .ok_or_else(|| fail(format!("object {owner} is missing or opaque")))?
            .get(field)
            .ok_or_else(|| fail(format!("object {owner} has no field {field}")))?
            .clone();
        if !matches!(old, Value::Null | Value::Ref(_)) {
            return Err(fail(format!(
                "field {field} is not a scalar object reference"
            )));
        }
        let value = target.map_or(Value::Null, Value::Ref);
        self.set_field(owner, field, value)?;
        if let Err(error) = self.to_bytes() {
            self.set_field(owner, field, old)?;
            return Err(error);
        }
        Ok(())
    }
}

fn checked_target(doc: &AvbDocument, target: u32) -> Result<()> {
    if target == 0 || doc.node(target).is_none() {
        return Err(fail(format!("target {target} is missing or opaque")));
    }
    Ok(())
}

fn list_field(doc: &AvbDocument, owner: u32, field: &str) -> Result<Vec<Value>> {
    let node = doc
        .node(owner)
        .ok_or_else(|| fail(format!("object {owner} is missing or opaque")))?;
    match node.get(field) {
        Some(Value::RefList(items)) => Ok(items.clone()),
        Some(_) => Err(fail(format!(
            "{}.{field} is not a reference list",
            node.class_str()
        ))),
        None => Err(fail(format!("{} has no field {field}", node.class_str()))),
    }
}

fn check_sequence_component(doc: &AvbDocument, owner: u32, field: &str, target: u32) -> Result<()> {
    let Some(sequence) = doc.node(owner) else {
        return Err(fail(format!("object {owner} is missing or opaque")));
    };
    if sequence.class != *b"SEQU" || field != "components" {
        return Ok(());
    }
    let component = doc.node(target).unwrap();
    if !matches!(&component.class, b"FILL" | b"SCLP" | b"TCCP" | b"SEQU")
        || sequence.get("edit_rate") != component.get("edit_rate")
        || sequence.get("media_kind_id") != component.get("media_kind_id")
    {
        return Err(fail(format!(
            "object {target} is incompatible with sequence {owner}"
        )));
    }
    Ok(())
}

fn commit_list(
    doc: &mut AvbDocument,
    owner: u32,
    field: &str,
    old: Vec<Value>,
    new: Vec<Value>,
) -> Result<()> {
    doc.set_field(owner, field, Value::RefList(new))?;
    if let Err(error) = doc.to_bytes() {
        doc.set_field(owner, field, Value::RefList(old))?;
        return Err(error);
    }
    Ok(())
}

/// Typed edits for the embedded `BinItem` list stored in an ABIN/BINF root.
/// The referenced mobs keep their object IDs; removing an item does not
/// delete the mob or any of its reachable objects from the document.
pub trait DocumentBinItemsExt {
    /// Remove a bin row and return the referenced composition's object ID.
    fn remove_bin_item_at(&mut self, index: usize) -> Result<u32>;

    /// Move a bin row to its final zero-based index within the same list.
    fn move_bin_item(&mut self, from: usize, to: usize) -> Result<()>;
}

impl DocumentBinItemsExt for AvbDocument {
    fn remove_bin_item_at(&mut self, index: usize) -> Result<u32> {
        let (root, original) = bin_items(self)?;
        let item = original
            .get(index)
            .ok_or_else(|| fail(format!("bin item index {index} is out of range")))?;
        let mob_id = bin_item_mob_id(self, item)?;
        let mut edited = original.clone();
        edited.remove(index);
        commit_bin_items(self, root, original, edited)?;
        Ok(mob_id)
    }

    fn move_bin_item(&mut self, from: usize, to: usize) -> Result<()> {
        let (root, original) = bin_items(self)?;
        if from >= original.len() || to >= original.len() {
            return Err(fail(format!(
                "bin item move {from} -> {to} exceeds list length {}",
                original.len()
            )));
        }
        bin_item_mob_id(self, &original[from])?;
        if from == to {
            return Ok(());
        }
        let mut edited = original.clone();
        let item = edited.remove(from);
        edited.insert(to, item);
        commit_bin_items(self, root, original, edited)
    }
}

fn bin_items(doc: &AvbDocument) -> Result<(u32, Vec<Value>)> {
    let root = doc.root_id();
    let bin = doc
        .root()
        .ok_or_else(|| fail("document root is missing or opaque"))?;
    if !matches!(&bin.class, b"ABIN" | b"BINF") {
        return Err(fail("document root is not a bin"));
    }
    match bin.get("items") {
        Some(Value::List(items)) => Ok((root, items.clone())),
        _ => Err(fail("bin has no embedded item list")),
    }
}

fn bin_item_mob_id(doc: &AvbDocument, value: &Value) -> Result<u32> {
    let Value::Node(item) = value else {
        return Err(fail("bin item is not an embedded node"));
    };
    if item.type_name != "BinItem" {
        return Err(fail("embedded bin item has the wrong type"));
    }
    let Some(Value::Ref(mob_id)) = item.get("mob") else {
        return Err(fail("bin item has no mob reference"));
    };
    if doc.node(*mob_id).map(|node| node.class) != Some(*b"CMPO") {
        return Err(fail(format!(
            "bin item mob {mob_id} is missing or not a composition"
        )));
    }
    Ok(*mob_id)
}

fn commit_bin_items(
    doc: &mut AvbDocument,
    root: u32,
    old: Vec<Value>,
    new: Vec<Value>,
) -> Result<()> {
    doc.set_field(root, "items", Value::List(new))?;
    if let Err(error) = doc.to_bytes() {
        doc.set_field(root, "items", Value::List(old))?;
        return Err(error);
    }
    Ok(())
}
