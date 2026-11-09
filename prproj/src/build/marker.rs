//! Markers (port of `models/marker.py`, `data/marker_colors.py`, the marker-list helpers of
//! `models/sequence.py` / `models/project_item.py` and `avid2prproj/markers.py` `_ensure_inner_list`).
//!
//! A marker is a top-level `Marker` object whose `DVAMarker` child holds a compact JSON blob
//! (`json.dumps(..., sort_keys=True, separators=(",", ":"))`, ASCII-only escapes). UUIDs are
//! drawn from the supplied [`UuidSource`] in Python's order: the marker ID when the marker is
//! built, then the colour cue key when a colour is set (and only when the marker had no colour
//! cue yet).

use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{append_pair, insert_before, remove_child};
use crate::xml::{NodeId, PremiereDocument, Tree};

use super::fmt::py_repr;
use super::time::Time;

/// ClassID of a top-level `Marker` object.
pub const MARKER_CLASS_ID: &str = "a45508e0-3ff7-4d04-90a7-2e0dfff4c910";
/// ClassID of the `Markers` collection object.
pub const MARKERS_CLASS_ID: &str = "bee50706-b524-416c-9f03-b596ce5f6866";
/// The all-zero GUID of a fresh collection's metadata/content state.
pub const ZERO_GUID: &str = "00000000-0000-0000-0000-000000000000";

/// `MARKER_COLOR_PACKED`: palette index -> packed `0xAABBGGRR` colour
/// (green, red, magenta, orange, yellow, white, blue, cyan).
pub const MARKER_COLOR_PACKED: [i64; 8] =
    [4281828977, 4281740498, 4289825711, 4280578025, 4281049552, 4294967295, 4294741314, 4292277273];

/// `PACKED_TO_INDEX`.
pub fn packed_to_index(packed: i64) -> Option<usize> {
    MARKER_COLOR_PACKED.iter().position(|&p| p == packed)
}

/// `DEFAULT_INDEX_BY_TYPE`: the colour index an uncoloured marker of this type reports.
pub fn default_index_by_type(marker_type: &str) -> Option<usize> {
    match marker_type {
        "Comment" => Some(0),
        "Chapter" => Some(1),
        "Segmentation" => Some(2),
        "WebLink" => Some(3),
        _ => None,
    }
}

/// The marker types Premiere knows (`validate_one_of`).
pub const MARKER_TYPES: [&str; 4] = ["Comment", "Chapter", "Segmentation", "WebLink"];

// --- minimal JSON (just what json.dumps/json.loads do for marker blobs) -----------------------

/// A JSON value. Objects keep insertion order; [`Json::dumps`] sorts keys like `sort_keys=True`.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An integer.
    Int(i128),
    /// A float (written with `repr`).
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object.
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// An object from pairs.
    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// The value of `key` when `self` is an object holding it.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string, if `self` is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The integer, if `self` is one.
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Json::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// The items, if `self` is an array.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, v: Json) {
        if let Json::Obj(pairs) = self {
            match pairs.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v,
                None => pairs.push((key.to_string(), v)),
            }
        }
    }

    /// `json.dumps(self, sort_keys=True, separators=(",", ":"))` (`ensure_ascii=True`).
    pub fn dumps(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(i) => out.push_str(&i.to_string()),
            Json::Float(f) => out.push_str(&json_float(*f)),
            Json::Str(s) => write_json_string(out, s),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                let mut sorted: Vec<&(String, Json)> = pairs.iter().collect();
                // Python sorts str keys by code point; UTF-8 byte order is the same order.
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                out.push('{');
                for (i, (k, v)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_json_string(out, k);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        py_repr(f)
    }
}

/// Python `json.encoder.py_encode_basestring_ascii`.
pub fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
        }
    }
    out.push('"');
}

struct JsonParser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl JsonParser<'_> {
    fn ws(&mut self) {
        while matches!(self.chars.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.chars.next();
        }
    }

    fn fail<T>(&self, what: &str) -> Result<T> {
        value(format!("invalid JSON: {what}"))
    }

    fn expect(&mut self, word: &str) -> Result<()> {
        for expected in word.chars() {
            if self.chars.next() != Some(expected) {
                return self.fail(word);
            }
        }
        Ok(())
    }

    fn parse(&mut self) -> Result<Json> {
        self.ws();
        match self.chars.peek().copied() {
            Some('{') => {
                self.chars.next();
                let mut pairs: Vec<(String, Json)> = Vec::new();
                self.ws();
                if self.chars.peek() == Some(&'}') {
                    self.chars.next();
                    return Ok(Json::Obj(pairs));
                }
                loop {
                    self.ws();
                    if self.chars.peek() != Some(&'"') {
                        return self.fail("object key");
                    }
                    let key = self.string()?;
                    self.ws();
                    if self.chars.next() != Some(':') {
                        return self.fail("':'");
                    }
                    let v = self.parse()?;
                    match pairs.iter_mut().find(|(k, _)| *k == key) {
                        Some(slot) => slot.1 = v,
                        None => pairs.push((key, v)),
                    }
                    self.ws();
                    match self.chars.next() {
                        Some(',') => continue,
                        Some('}') => return Ok(Json::Obj(pairs)),
                        _ => return self.fail("object"),
                    }
                }
            }
            Some('[') => {
                self.chars.next();
                let mut items = Vec::new();
                self.ws();
                if self.chars.peek() == Some(&']') {
                    self.chars.next();
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.parse()?);
                    self.ws();
                    match self.chars.next() {
                        Some(',') => continue,
                        Some(']') => return Ok(Json::Arr(items)),
                        _ => return self.fail("array"),
                    }
                }
            }
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t') => self.expect("true").map(|_| Json::Bool(true)),
            Some('f') => self.expect("false").map(|_| Json::Bool(false)),
            Some('n') => self.expect("null").map(|_| Json::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => {
                let mut text = String::new();
                while let Some(&c) = self.chars.peek() {
                    if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E') {
                        text.push(c);
                        self.chars.next();
                    } else {
                        break;
                    }
                }
                if text.contains(['.', 'e', 'E']) {
                    text.parse::<f64>().map(Json::Float).or_else(|_| self.fail("number"))
                } else {
                    text.parse::<i128>().map(Json::Int).or_else(|_| self.fail("number"))
                }
            }
            _ => self.fail("value"),
        }
    }

    fn string(&mut self) -> Result<String> {
        self.chars.next(); // opening quote
        let mut out = String::new();
        let mut pending_high: Option<u16> = None;
        loop {
            let c = match self.chars.next() {
                Some(c) => c,
                None => return self.fail("unterminated string"),
            };
            if let Some(high) = pending_high {
                // A high surrogate must be followed by a \uXXXX low surrogate.
                if c != '\\' {
                    return self.fail("lone surrogate");
                }
                if self.chars.next() != Some('u') {
                    return self.fail("lone surrogate");
                }
                let low = self.hex4()?;
                if !(0xDC00..0xE000).contains(&low) {
                    return self.fail("lone surrogate");
                }
                let cp = 0x10000 + (((high as u32) - 0xD800) << 10) + ((low as u32) - 0xDC00);
                out.push(char::from_u32(cp).expect("valid pair"));
                pending_high = None;
                continue;
            }
            match c {
                '"' => return Ok(out),
                '\\' => match self.chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('u') => {
                        let unit = self.hex4()?;
                        if (0xD800..0xDC00).contains(&unit) {
                            pending_high = Some(unit);
                        } else if let Some(ch) = char::from_u32(unit as u32) {
                            out.push(ch);
                        } else {
                            return self.fail("lone surrogate");
                        }
                    }
                    _ => return self.fail("escape"),
                },
                c if (c as u32) < 0x20 => return self.fail("control character in string"),
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u16> {
        let mut v = 0u16;
        for _ in 0..4 {
            match self.chars.next().and_then(|c| c.to_digit(16)) {
                Some(d) => v = v * 16 + d as u16,
                None => return self.fail("\\u escape"),
            }
        }
        Ok(v)
    }
}

/// `json.loads` (strict subset: no NaN/Infinity literals).
pub fn json_loads(text: &str) -> Result<Json> {
    let mut parser = JsonParser { chars: text.chars().peekable() };
    let v = parser.parse()?;
    parser.ws();
    if parser.chars.next().is_some() {
        return value("invalid JSON: extra data");
    }
    Ok(v)
}

// --- the Marker element -----------------------------------------------------------------------

/// What `Marker(name, start, comments, marker_type, duration)` takes.
#[derive(Clone, Debug)]
pub struct MarkerSpec<'a> {
    /// Marker name.
    pub name: &'a str,
    /// Start in ticks.
    pub start: Time,
    /// Comments (omitted from the blob when empty).
    pub comments: &'a str,
    /// One of [`MARKER_TYPES`].
    pub marker_type: &'a str,
    /// Duration (omitted from the blob when `None` or zero).
    pub duration: Option<Time>,
}

impl<'a> MarkerSpec<'a> {
    /// A point comment marker.
    pub fn comment(name: &'a str, start: Time) -> Self {
        MarkerSpec { name, start, comments: "", marker_type: "Comment", duration: None }
    }
}

/// `validate_string()`: text that can survive a save (no XML-illegal control characters, no
/// trailing newline-plus-tabs run).
pub fn validate_string(text: &str) -> Result<()> {
    if let Some(c) = text.chars().find(|&c| matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}')) {
        return value(format!(
            "must not contain the control character '\\x{:02x}': XML has no representation for it",
            c as u32
        ));
    }
    let trimmed = text.trim_end_matches('\t');
    if trimmed.len() < text.len() && trimmed.ends_with('\n') {
        return value(
            "must not end with a newline followed by tabs: the saved file would be read back differently by py and by Premiere",
        );
    }
    Ok(())
}

/// `Marker.__init__`: a detached top-level `Marker` element (empty `ObjectID` placeholder
/// first, as Premiere writes it). Draws the marker ID from `uuids`.
pub fn new_marker(tree: &mut Tree, spec: &MarkerSpec<'_>, uuids: &mut dyn UuidSource) -> Result<NodeId> {
    validate_string(spec.name)?;
    validate_string(spec.comments)?;
    crate::model::validators::validate_one_of(spec.marker_type, &MARKER_TYPES)?;
    if let Some(d) = spec.duration {
        if d.ticks < 0 {
            return value("duration must not be negative");
        }
    }
    let mut blob = vec![
        ("mMarkerID".to_string(), Json::Str(uuids.uuid4().to_string())),
        ("mName".to_string(), Json::Str(spec.name.to_string())),
        ("mStartTime".to_string(), Json::obj(vec![("ticks", Json::Int(spec.start.ticks as i128))])),
        ("mType".to_string(), Json::Str(spec.marker_type.to_string())),
    ];
    if !spec.comments.is_empty() {
        blob.push(("mComment".to_string(), Json::Str(spec.comments.to_string())));
    }
    if let Some(d) = spec.duration {
        if d.ticks != 0 {
            blob.push(("mDuration".to_string(), Json::obj(vec![("ticks", Json::Int(d.ticks as i128))])));
        }
    }
    let element = tree.new_element_with("Marker", &[("ObjectID", ""), ("ClassID", MARKER_CLASS_ID), ("Version", "3")]);
    tree.set_text(element, Some("\n\t\t"));
    let payload = tree.sub_element(element, "DVAMarker", &[]);
    let text = Json::obj(vec![("DVAMarker", Json::Obj(blob))]).dumps();
    tree.set_text(payload, Some(text));
    tree.set_tail(payload, Some("\n\t"));
    Ok(element)
}

pub(crate) fn blob_data(tree: &Tree, marker: NodeId) -> Result<Json> {
    let Some(payload) = tree.child_by_tag(marker, "DVAMarker") else {
        return value("marker has no DVAMarker payload");
    };
    let text = tree.text(payload).filter(|t| !t.is_empty()).unwrap_or("{}");
    let data = json_loads(text)?;
    Ok(match data.get("DVAMarker") {
        Some(inner @ Json::Obj(_)) => inner.clone(),
        _ => Json::Obj(Vec::new()),
    })
}

pub(crate) fn write_blob(tree: &mut Tree, marker: NodeId, data: Json) -> Result<()> {
    let Some(payload) = tree.child_by_tag(marker, "DVAMarker") else {
        return value("marker has no DVAMarker payload");
    };
    tree.set_text(payload, Some(Json::obj(vec![("DVAMarker", data)]).dumps()));
    Ok(())
}

/// `_cue_color`: the packed colour of a colour cue point, if it is one.
fn cue_color(cue: &Json) -> Option<i128> {
    let Json::Str(raw) = cue.get("mValue")? else { return None };
    let parsed = json_loads(raw).ok()?;
    match parsed.get("color")? {
        Json::Int(c) => Some(*c),
        Json::Bool(b) => Some(*b as i128),
        _ => None,
    }
}

/// `Marker.color_index = index` (0..=7). Replaces the first colour cue's value, or appends a
/// cue with a fresh `keywordExtDVAv1_<uuid>` key (drawing one UUID).
pub fn set_color_index(tree: &mut Tree, marker: NodeId, index: i64, uuids: &mut dyn UuidSource) -> Result<()> {
    crate::model::validators::MARKER_COLOR_INDEX.check(crate::model::validators::Num::Int(index as i128))?;
    let mut data = blob_data(tree, marker)?;
    let mut cues = match data.get("mCuePointList") {
        Some(Json::Arr(items)) => items.clone(),
        _ => Vec::new(),
    };
    let color = Json::obj(vec![("color", Json::Int(MARKER_COLOR_PACKED[index as usize] as i128))]).dumps();
    match cues.iter_mut().find(|cue| cue_color(cue).is_some()) {
        Some(cue) => cue.set("mValue", Json::Str(color)),
        None => cues.push(Json::obj(vec![
            ("mKey", Json::Str(format!("keywordExtDVAv1_{}", uuids.uuid4()))),
            ("mValue", Json::Str(color)),
        ])),
    }
    data.set("mCuePointList", Json::Arr(cues));
    write_blob(tree, marker, data)
}

/// `Marker.guid`: the persistent `mMarkerID` (empty when missing).
pub fn marker_guid(tree: &Tree, marker: NodeId) -> Result<String> {
    Ok(match blob_data(tree, marker)?.get("mMarkerID") {
        Some(Json::Str(s)) => s.clone(),
        _ => String::new(),
    })
}

/// `Marker.color_index` (read): the palette index, the type default when uncoloured, `None`
/// for a colour outside the palette.
pub fn color_index(tree: &Tree, marker: NodeId) -> Result<Option<usize>> {
    let data = blob_data(tree, marker)?;
    let packed = match data.get("mCuePointList") {
        Some(Json::Arr(cues)) => cues.iter().find_map(cue_color),
        _ => None,
    };
    Ok(match packed {
        None => {
            let t = match data.get("mType") {
                Some(Json::Str(s)) => s.as_str(),
                _ => "",
            };
            Some(default_index_by_type(t).unwrap_or(0))
        }
        Some(p) => i64::try_from(p).ok().and_then(packed_to_index),
    })
}

// --- the Markers collection ---------------------------------------------------------------------

/// The fresh `Markers` collection object (`ClassID`/`Version 4`), detached, with its inner
/// `<Markers Version="1">` pair list, `ByGUID`, `LastMetadataState`, `LastContentState`.
fn new_collection(tree: &mut Tree) -> NodeId {
    let collection = tree.new_element_with("Markers", &[("ClassID", MARKERS_CLASS_ID), ("Version", "4")]);
    tree.set_text(collection, Some("\n\t\t"));
    let inner = tree.sub_element(collection, "Markers", &[("Version", "1")]);
    tree.set_tail(inner, Some("\n\t\t"));
    for (tag, text, tail) in [
        ("ByGUID", "byGUID", "\n\t\t"),
        ("LastMetadataState", ZERO_GUID, "\n\t\t"),
        ("LastContentState", ZERO_GUID, "\n\t"),
    ] {
        let child = tree.sub_element(collection, tag, &[]);
        tree.set_text(child, Some(text));
        tree.set_tail(child, Some(tail));
    }
    collection
}

fn inner_list(document: &PremiereDocument, collection: NodeId) -> Result<NodeId> {
    document
        .tree
        .child_by_tag(collection, "Markers")
        .ok_or_else(|| Error::Value("marker collection has no inner list".into()))
}

/// `Sequence._ensure_marker_list`: the inner `<Markers Version="1">` pair list of a sequence,
/// creating the collection object and the sequence's `MarkerOwner` (before
/// `PersistentGroupContainer`) when absent.
pub fn ensure_sequence_marker_list(document: &mut PremiereDocument, sequence: NodeId) -> Result<NodeId> {
    let collection = match document.tree.find1(sequence, "MarkerOwner/Markers") {
        Some(reference) => document.resolve(reference)?,
        None => {
            let collection = new_collection(&mut document.tree);
            let collection_id = document.add_object(collection)?;
            let tree = &mut document.tree;
            let owner = tree.new_element_with("MarkerOwner", &[("Version", "1")]);
            tree.set_text(owner, Some("\n\t\t\t"));
            let reference = tree.sub_element(owner, "Markers", &[("ObjectRef", &collection_id)]);
            tree.set_tail(reference, Some("\n\t\t"));
            insert_before(tree, sequence, "PersistentGroupContainer", owner)?;
            collection
        }
    };
    inner_list(document, collection)
}

/// `_ensure_clip_marker_list`: the inner marker list of a clip core (`Clip` element of a
/// `VideoClip`/`AudioClip`), synthesizing the `MarkerOwner` before `Source` when absent.
pub fn ensure_clip_marker_list(document: &mut PremiereDocument, core: NodeId) -> Result<NodeId> {
    let collection = match document.tree.find1(core, "MarkerOwner/Markers") {
        Some(reference) => document.resolve(reference)?,
        None => {
            let collection = new_collection(&mut document.tree);
            let collection_id = document.add_object(collection)?;
            let tree = &mut document.tree;
            let owner = tree.new_element_with("MarkerOwner", &[("Version", "1")]);
            tree.set_text(owner, Some("\n\t\t\t\t"));
            let reference = tree.sub_element(owner, "Markers", &[("ObjectRef", &collection_id)]);
            tree.set_tail(reference, Some("\n\t\t\t"));
            insert_before(tree, core, "Source", owner)?;
            collection
        }
    };
    inner_list(document, collection)
}

/// `avid2prproj.markers._ensure_inner_list`: if the clip core's Markers collection lacks its
/// inner list, insert `<Markers Version="1"/>` (tail `\n\t\t`) as its first child.
pub fn ensure_inner_list(document: &mut PremiereDocument, core: Option<NodeId>) -> Result<()> {
    let Some(core) = core else { return Ok(()) };
    let Some(reference) = document.tree.find1(core, "MarkerOwner/Markers") else { return Ok(()) };
    let collection = document.resolve(reference)?;
    if document.tree.child_by_tag(collection, "Markers").is_none() {
        let tree = &mut document.tree;
        let inner = tree.new_element_with("Markers", &[("Version", "1")]);
        tree.set_tail(inner, Some("\n\t\t"));
        tree.insert(collection, 0, inner);
    }
    Ok(())
}

/// `_attach_marker`: register the marker in the top-level table and add its pair to the list.
/// Returns the marker's new ObjectID.
pub fn attach_marker(document: &mut PremiereDocument, marker_list: NodeId, marker: NodeId) -> Result<String> {
    let guid = marker_guid(&document.tree, marker)?;
    let new_id = document.add_object(marker)?;
    append_pair(&mut document.tree, marker_list, &guid, &new_id);
    Ok(new_id)
}

/// `_detach_marker`: remove the marker's pair, renumber the remaining pairs, drop the object.
pub fn detach_marker(document: &mut PremiereDocument, marker_list: NodeId, marker: NodeId) -> Result<()> {
    let object_id = document.tree.get(marker, "ObjectID").map(str::to_string);
    let pairs: Vec<NodeId> = document.tree.children(marker_list).iter().copied().filter(|&c| document.tree.tag(c) == "Marker").collect();
    for pair in pairs {
        let second = document.tree.child_by_tag(pair, "Second");
        if second.and_then(|s| document.tree.get(s, "ObjectRef")).map(str::to_string) == object_id {
            remove_child(&mut document.tree, marker_list, pair)?;
            break;
        }
    }
    let remaining: Vec<NodeId> = document.tree.children(marker_list).iter().copied().filter(|&c| document.tree.tag(c) == "Marker").collect();
    for (index, pair) in remaining.into_iter().enumerate() {
        document.tree.set(pair, "Index", index.to_string());
    }
    document.remove_object(marker)
}

/// `Sequence.add_marker` followed by the optional `color_index` assignment
/// (`markers.add_sequence_marker`). Returns the marker element.
pub fn add_sequence_marker(
    document: &mut PremiereDocument,
    sequence: NodeId,
    spec: &MarkerSpec<'_>,
    color: Option<i64>,
    uuids: &mut dyn UuidSource,
) -> Result<NodeId> {
    let marker = new_marker(&mut document.tree, spec, uuids)?;
    let list = ensure_sequence_marker_list(document, sequence)?;
    attach_marker(document, list, marker)?;
    if let Some(index) = color {
        set_color_index(&mut document.tree, marker, index, uuids)?;
    }
    Ok(marker)
}

/// `ProjectItem.add_marker` on a clip core, then the optional colour
/// (`markers.add_clip_marker`; `ensure_inner_list` runs first there). Note Python builds the
/// `Marker` (drawing its UUID) AFTER `_ensure_clip_marker_list`; no UUID is drawn by that.
pub fn add_clip_marker(
    document: &mut PremiereDocument,
    core: NodeId,
    spec: &MarkerSpec<'_>,
    color: Option<i64>,
    uuids: &mut dyn UuidSource,
) -> Result<NodeId> {
    ensure_inner_list(document, Some(core))?;
    let list = ensure_clip_marker_list(document, core)?;
    let marker = new_marker(&mut document.tree, spec, uuids)?;
    attach_marker(document, list, marker)?;
    if let Some(index) = color {
        set_color_index(&mut document.tree, marker, index, uuids)?;
    }
    Ok(marker)
}
