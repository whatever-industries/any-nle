//! Port of `models/mgt_builder.py`: read a Motion Graphics template (`.mogrt`) and synthesize
//! what importing it writes.
//!
//! A `.mogrt` is a zip holding `definition.json` and the `project.aegraphic` After Effects
//! rendered. [`Template::open`] reads both (a small zip reader below: stored and deflate
//! members, CRC checked); the builders then write the media, the capsule (Essential Graphics)
//! component and the panel entries. UUIDs come from the supplied [`UuidSource`] in Python's
//! call order.
//!
//! Limits against Python: JSON numbers the template feeds into arithmetic or formatting must be
//! integers that fit `i64` (colour and point defaults may be floats); `json.loads` error texts
//! (a malformed `definition.json`) are this crate's, not Python's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::fmt::py_repr;
use super::marker::{json_loads, Json};
use super::media::{relpath, timecode_format};
use super::time::{round_half_even, TICKS_PER_SECOND};
use super::{leaf, top};
use crate::base64::b64encode;
use crate::convert::model::py_str_repr;
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::{NodeId, Tree};

const MEDIA_CLASS_ID: &str = "7a5c103e-f3ac-4391-b6b4-7cc3d2f9a7ff";
const VIDEO_STREAM_CLASS_ID: &str = "a36e4719-3ec6-4a0c-ab11-8b4aab377aa5";
const VIDEO_SOURCE_CLASS_ID: &str = "e64ddf74-8fac-4682-8aa8-0e0ca2248949";
const MASTER_CLASS_ID: &str = "fb11c33a-b0a9-4465-aa94-b6d5db2628cf";
const LOGGING_CLASS_ID: &str = "77ab7fdd-dcdf-465d-9906-7a330ca1e738";
const VIDEO_CLIP_CLASS_ID: &str = "9308dbef-2440-4acb-9ab2-953b9a4e82ec";
const FILTER_CLASS_ID: &str = "d10da199-beea-4dd1-b941-ed3a78766d50";
const VIDEO_CHAIN_CLASS_ID: &str = "0970e08a-f58f-4108-b29a-1a717b8e12e2";
const ARB_PARAM_CLASS_ID: &str = "313e54d4-6903-49ad-b0bf-8262cdd10f4e";
const POINT_PARAM_CLASS_ID: &str = "ca81d347-309b-44d2-acc7-1c572efb973c";
const COLOR_PARAM_CLASS_ID: &str = "0fde4e9f-f895-4ba3-b0fe-9a6feafda583";
const BIN_CLASS_ID: &str = "dbfd6653-24da-480e-a35e-ba45e9504e4b";
const ITEM_CLASS_ID: &str = "cb4e0ed7-aca1-4171-8525-e3658dec06dd";

const DEFINITION_MEMBER: &str = "definition.json";
const GRAPHIC_MEMBER: &str = "project.aegraphic";

/// `MEDIA_FOLDER`: the folder Premiere copies the template's graphic into, and the panel bin of
/// the same name it files the imported item under.
pub const MEDIA_FOLDER: &str = "Motion Graphics Template Media";

const AE_GRAPHIC_IMPLEMENTATION_ID: &str = "ec341e53-60c2-4d89-abfc-bdb5c0ff2e0b";
const AE_GRAPHIC_CODEC: &str = "1145854285";
const CAPSULE_MATCH_NAME: &str = "AE.ADBE Capsule";
const START: &str = "-91445760000000000";
const BIN_LABEL: &str = "BE.Prefs.LabelColors.7";

const TYPE_TEXT: i128 = 0;
const TYPE_COLOR: i128 = 3;
const TYPE_POINT: i128 = 6;
const TYPE_GROUP: i128 = 8;

const GROUP_CONTROL_TYPE: &str = "11";
const TEXT_CONTROL_TYPE: &str = "23";

/// `MASTER_SLOT`: the capsule's `Component/ID` on the master's blueprint chain.
pub const MASTER_SLOT: &str = "1";
/// `PLACEMENT_SLOT`: the capsule's `Component/ID` on a placement's chain.
pub const PLACEMENT_SLOT: &str = "3";

/// The store-once payload table threaded through the builders (`stored: dict[bytes, str]`).
pub type Stored = HashMap<Vec<u8>, String>;

fn key_error<T>(key: &str) -> Result<T> {
    value(format!("'{key}'"))
}

fn req<'a>(object: &'a Json, key: &str) -> Result<&'a Json> {
    match object.get(key) {
        Some(v) => Ok(v),
        None => key_error(key),
    }
}

fn req_int(object: &Json, key: &str) -> Result<i64> {
    match req(object, key)? {
        Json::Int(i) => i64::try_from(*i).map_err(|_| Error::Value(format!("integer out of range: {i}"))),
        other => value(format!("unsupported value for '{key}': {}", py_repr_json(other))),
    }
}

fn req_str<'a>(object: &'a Json, key: &str) -> Result<&'a str> {
    match req(object, key)? {
        Json::Str(s) => Ok(s),
        other => value(format!("expected a string for '{key}', got {}", py_repr_json(other))),
    }
}

/// Python `repr` of a decoded JSON value.
fn py_repr_json(v: &Json) -> String {
    match v {
        Json::Null => "None".into(),
        Json::Bool(true) => "True".into(),
        Json::Bool(false) => "False".into(),
        Json::Int(i) => i.to_string(),
        Json::Float(f) => py_repr(*f),
        Json::Str(s) => py_str_repr(s),
        Json::Arr(items) => format!("[{}]", items.iter().map(py_repr_json).collect::<Vec<_>>().join(", ")),
        Json::Obj(pairs) => format!(
            "{{{}}}",
            pairs.iter().map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr_json(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Python `str` of a decoded JSON value.
fn py_str_json(v: &Json) -> String {
    match v {
        Json::Str(s) => s.clone(),
        other => py_repr_json(other),
    }
}

/// Python truthiness of a decoded JSON value.
fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Int(i) => *i != 0,
        Json::Float(f) => *f != 0.0,
        Json::Str(s) => !s.is_empty(),
        Json::Arr(a) => !a.is_empty(),
        Json::Obj(o) => !o.is_empty(),
    }
}

/// `_compact`: JSON the way Premiere stores it, sorted keys and no spaces.
pub fn compact(v: &Json) -> String {
    v.dumps()
}

/// `Json::Obj` assignment `d[key] = v` (replace in place, else append).
fn obj_set(pairs: &mut Vec<(String, Json)>, key: &str, v: Json) {
    match pairs.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = v,
        None => pairs.push((key.to_string(), v)),
    }
}

/// `_pack_color`: each channel takes a 16-bit lane in 8.8 fixed point, ordered A, R, G, B from
/// the top; the colour channels are scaled to 0-255 first while alpha stays in 0-1.
pub fn pack_color(rgba: &Json) -> Result<i128> {
    let Json::Arr(items) = rgba else { return value("cannot unpack a non-sequence colour") };
    if items.len() != 4 {
        return value(format!("not enough or too many values to unpack (expected 4, got {})", items.len()));
    }
    // Python rounds the float products and keeps int products exact.
    let scaled = |v: &Json, k: i64| -> Result<i128> {
        match v {
            Json::Int(i) => Ok(*i * k as i128),
            Json::Float(f) => Ok(round_half_even(f * k as f64)? as i128),
            other => value(format!("unsupported colour channel {}", py_repr_json(other))),
        }
    };
    let (red, green, blue, alpha) = (&items[0], &items[1], &items[2], &items[3]);
    Ok((scaled(alpha, 256)? << 48) | (scaled(red, 255)? << 40) | (scaled(green, 255)? << 24) | (scaled(blue, 255)? << 8))
}

/// `_flatten`: a control as the component's private data stores it.
fn flatten(entry: &Json) -> Result<Json> {
    match entry.get("capPropFontEditInfo") {
        None | Some(Json::Null) => Ok(entry.clone()),
        Some(_) => {
            let Json::Obj(pairs) = entry else { return value("control is not an object") };
            let mut flat: Vec<(String, Json)> =
                pairs.iter().filter(|(k, _)| k != "capPropFontEditInfo").cloned().collect();
            for (k, v) in text_fields(entry)? {
                obj_set(&mut flat, &k, v);
            }
            Ok(Json::Obj(flat))
        }
    }
}

/// `_text_fields`: a text control's stored value, its default under one text run.
fn text_fields(entry: &Json) -> Result<Vec<(String, Json)>> {
    let text = py_str_json(req(entry, "capPropDefault")?);
    let mut fields: Vec<(String, Json)> = Vec::new();
    let Json::Obj(info) = req(entry, "capPropFontEditInfo")? else {
        return value("'capPropFontEditInfo' is not an object");
    };
    for (key, setting) in info {
        // The `capProp*` flags say which font fields the control exposes, so they stay scalar.
        let v = if key.starts_with("capProp") { setting.clone() } else { Json::Arr(vec![setting.clone()]) };
        obj_set(&mut fields, key, v);
    }
    obj_set(&mut fields, "capPropTextRunCount", Json::Int(1));
    obj_set(&mut fields, "fontTextRunLength", Json::Arr(vec![Json::Int(text.chars().count() as i128)]));
    obj_set(&mut fields, "textEditValue", Json::Str(text));
    Ok(fields)
}

/// `_text_value`: the payload a text control's parameter carries.
fn text_value(entry: &Json) -> Result<String> {
    match entry.get("capPropFontEditInfo") {
        None | Some(Json::Null) => Ok(py_str_json(req(entry, "capPropDefault")?)),
        Some(_) => Ok(compact(&Json::Obj(text_fields(entry)?))),
    }
}

/// A `.mogrt` archive: its graphic, and what importing it must write.
#[derive(Clone, Debug)]
pub struct Template {
    /// The `project.aegraphic` bytes.
    pub graphic: Vec<u8>,
    /// `capsuleID`.
    pub capsule_id: String,
    /// `capsuleName`.
    pub name: String,
    /// `capsuleparams.capParams`, in `ParameterID` order.
    pub params: Vec<Json>,
    /// `hasaudio`, by Python truthiness.
    pub has_audio: bool,
    /// `framerate.ticksperframe`.
    pub frame_rate: i64,
    /// `TICKS_PER_SECOND * value // scale`.
    pub duration_ticks: i64,
    /// `framesize.size` as `(x, y)`.
    pub frame_size: (i64, i64),
    /// `alphachanneltype` (written through `str`).
    pub alpha_type: Json,
    /// `nativefieldtype` (written through `str`).
    pub field_type: Json,
    /// `TIMECODE_FORMATS` lookup of the frame rate.
    pub time_display: i64,
    /// `_compact(colorSpace)`.
    pub color_space: String,
    /// The importer's own description with two keys re-stamped, prefixed with its id.
    pub importer_prefs: String,
    /// The capsule component's private data.
    pub private_data: String,
}

impl Template {
    /// `Template(path)`.
    pub fn open(path: &Path) -> Result<Template> {
        let display = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| Error::Io(e.to_string()))?;
        let Some(archive) = zip::Archive::open(&data) else {
            return value(format!("not a Motion Graphics template: {display}"));
        };
        let wrap = |message: String| Error::Value(format!("not a Motion Graphics template ({message}): {display}"));
        let definition_bytes = archive.read(DEFINITION_MEMBER).map_err(|e| wrap(e.message()))?;
        let text = std::str::from_utf8(&definition_bytes).map_err(|e| {
            wrap(format!(
                "'utf-8' codec can't decode byte 0x{:02x} in position {}: invalid start byte",
                definition_bytes[e.valid_up_to()],
                e.valid_up_to()
            ))
        })?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let definition = json_loads(text).map_err(|e| wrap(e.to_string()))?;
        let graphic = archive.read(GRAPHIC_MEMBER).map_err(|e| wrap(e.message()))?;
        Template::from_definition(&definition, graphic)
    }

    /// The part of `Template.__init__` after the archive is read.
    pub fn from_definition(definition: &Json, graphic: Vec<u8>) -> Result<Template> {
        let capsule_id = req_str(definition, "capsuleID")?.to_string();
        let name = req_str(definition, "capsuleName")?.to_string();
        let info = req(req(definition, "sourceInfoLocalized")?, "en_US")?;
        let Json::Arr(params) = req(req(info, "capsuleparams")?, "capParams")? else {
            return value("'capParams' is not a list");
        };
        let has_audio = truthy(req(info, "hasaudio")?);
        let frame_rate = req_int(req(info, "framerate")?, "ticksperframe")?;
        let duration = req(info, "duration")?;
        let (dv, ds) = (req_int(duration, "value")?, req_int(duration, "scale")?);
        if ds == 0 {
            return value("integer division or modulo by zero");
        }
        let numerator = TICKS_PER_SECOND as i128 * dv as i128;
        let quotient = numerator.div_euclid(ds as i128);
        // Python floors; div_euclid floors for a positive divisor only.
        let duration_ticks = if ds > 0 {
            quotient
        } else {
            let q = numerator / ds as i128;
            if numerator % ds as i128 != 0 && ((numerator < 0) != ((ds as i128) < 0)) { q - 1 } else { q }
        };
        let duration_ticks =
            i64::try_from(duration_ticks).map_err(|_| Error::Value("duration out of range".into()))?;
        let size = req(req(info, "framesize")?, "size")?;
        let frame_size = (req_int(size, "x")?, req_int(size, "y")?);
        let alpha_type = req(info, "alphachanneltype")?.clone();
        let field_type = req(info, "nativefieldtype")?.clone();
        let drop_frame = match req(info, "isdropframe")? {
            Json::Bool(b) => Some(*b),
            Json::Int(0) => Some(false),
            Json::Int(1) => Some(true),
            _ => None,
        };
        let display = drop_frame.and_then(|d| timecode_format(frame_rate, d));
        let Some(time_display) = display else {
            return value(format!("no timecode format known for {frame_rate} ticks per frame"));
        };
        let color_space = compact(req(info, "colorSpace")?);
        // Premiere re-stamps two keys of the importer's own description as it takes the
        // graphic over: the flat artisan options empty out and the AE library switches on.
        let app_info_text = req_str(info, "appspecificsourceinfo")?;
        let Json::Obj(mut app_info) = json_loads(app_info_text)? else {
            return value("'appspecificsourceinfo' is not an object");
        };
        obj_set(&mut app_info, "flatArtisanOptions", Json::Arr(Vec::new()));
        obj_set(&mut app_info, "useAELib", Json::Bool(true));
        let importer_prefs = format!("{}{}", req_str(info, "id")?, compact(&Json::Obj(app_info)));
        let flat = params.iter().map(flatten).collect::<Result<Vec<_>>>()?;
        let private_data = compact(&Json::obj(vec![
            ("capsuleparams", Json::obj(vec![("capParams", Json::Arr(flat))])),
            ("framesize", req(info, "framesize")?.clone()),
        ]));
        Ok(Template {
            graphic,
            capsule_id,
            name,
            params: params.clone(),
            has_audio,
            frame_rate,
            duration_ticks,
            frame_size,
            alpha_type,
            field_type,
            time_display,
            color_space,
            importer_prefs,
            private_data,
        })
    }

    /// `media_path`: where importing this template copies its graphic.
    pub fn media_path(&self, project_directory: &Path) -> PathBuf {
        crate::model::project::py_path(&format!(
            "{}/{}/{}/{}.aegraphic",
            project_directory.display(),
            MEDIA_FOLDER,
            self.capsule_id,
            self.name
        ))
    }
}

/// `_payload_element`: a payload element following Premiere's store-once rule. A fresh payload
/// draws a `BinaryHash` and carries the base64 text; a repeated one reuses the first's hash.
fn payload_element(tree: &mut Tree, tag: &str, payload: &[u8], tail: &str, stored: &mut Stored, uuids: &mut dyn UuidSource) -> NodeId {
    let existing = stored.get(payload).cloned();
    let hash = existing.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| uuids.uuid4().to_string());
    let element = tree.new_element_with(tag, &[("Encoding", "base64"), ("BinaryHash", &hash)]);
    if existing.is_none() {
        stored.insert(payload.to_vec(), hash);
        tree.set_text(element, Some(format!("{}\n\t\t", b64encode(payload))));
    }
    tree.set_tail(element, Some(tail));
    element
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// `build_capsule_params`: every Essential Graphics control, in `ParameterID` order.
pub fn build_capsule_params(tree: &mut Tree, template: &Template, stored: &mut Stored, uuids: &mut dyn UuidSource) -> Result<Vec<NodeId>> {
    let (width, height) = template.frame_size;
    let mut elements = Vec::new();
    for (index, entry) in template.params.iter().enumerate() {
        let kind = req(entry, "capPropType")?;
        let name = req_str(entry, "capPropUIName")?;
        let kind_int = match kind {
            Json::Int(i) => Some(*i),
            _ => None,
        };
        let element;
        if kind_int == Some(TYPE_GROUP) || kind_int == Some(TYPE_TEXT) {
            element = top(tree, "ArbVideoComponentParam", ARB_PARAM_CLASS_ID, "3", None);
            leaf(tree, element, "Name", name, "\n\t\t");
            let control = if kind_int == Some(TYPE_GROUP) { GROUP_CONTROL_TYPE } else { TEXT_CONTROL_TYPE };
            leaf(tree, element, "ParameterControlType", control, "\n\t\t");
            leaf(tree, element, "ParameterID", &index.to_string(), "\n\t\t");
            leaf(tree, element, "StartKeyframePosition", START, "\n\t\t");
            let text = if kind_int == Some(TYPE_GROUP) {
                // A group header stores the ids of the controls it holds.
                let Json::Arr(children) = req(entry, "capPropDefault")? else {
                    return value("group default is not a list");
                };
                children.iter().map(|c| format!("{};", py_str_json(c))).collect::<String>()
            } else {
                text_value(entry)?
            };
            let payload = payload_element(tree, "StartKeyframeValue", &utf16le(&text), "\n\t", stored, uuids);
            tree.append(element, payload);
        } else if kind_int == Some(TYPE_COLOR) {
            element = top(tree, "VideoComponentParam", COLOR_PARAM_CLASS_ID, "10", None);
            leaf(tree, element, "Name", name, "\n\t\t");
            leaf(tree, element, "ParameterID", &index.to_string(), "\n\t\t");
            let packed = pack_color(req(entry, "capPropDefault")?)?;
            leaf(tree, element, "StartKeyframe", &format!("{START},{packed},0,0,0,0,0,0"), "\n\t");
        } else if kind_int == Some(TYPE_POINT) {
            element = top(tree, "PointComponentParam", POINT_PARAM_CLASS_ID, "4", None);
            leaf(tree, element, "Name", name, "\n\t\t");
            leaf(tree, element, "ParameterID", &index.to_string(), "\n\t\t");
            // Positions are stored normalized to the template's frame.
            let Json::Arr(point) = req(entry, "capPropDefault")? else { return value("point default is not a list") };
            if point.len() != 2 {
                return value(format!("not enough or too many values to unpack (expected 2, got {})", point.len()));
            }
            let normalized = |v: &Json, extent: i64| -> Result<String> {
                if extent == 0 {
                    return value("division by zero");
                }
                let numerator = match v {
                    Json::Int(i) => *i as f64,
                    Json::Float(f) => *f,
                    other => return value(format!("unsupported point coordinate {}", py_repr_json(other))),
                };
                Ok(super::fmt::fmt_g(numerator / extent as f64, 17))
            };
            let point_text = format!("{}:{}", normalized(&point[0], width)?, normalized(&point[1], height)?);
            leaf(tree, element, "StartKeyframe", &format!("{START},{point_text},0,0,0,0,0,0,5,4,0,0,0,0"), "\n\t");
        } else {
            return value(format!("unsupported Motion Graphics control type {}", py_str_json(kind)));
        }
        elements.push(element);
    }
    Ok(elements)
}

/// `build_capsule_component`: the `AE.ADBE Capsule` component wrapping the controls. `slot` is
/// the component's id within its chain.
pub fn build_capsule_component(
    tree: &mut Tree,
    template: &Template,
    param_ids: &[String],
    stored: &mut Stored,
    slot: &str,
    uuids: &mut dyn UuidSource,
) -> NodeId {
    let element = top(tree, "VideoFilterComponent", FILTER_CLASS_ID, "9", None);
    let core = tree.sub_element(element, "Component", &[("Version", "7")]);
    tree.set_text(core, Some("\n\t\t\t"));
    tree.set_tail(core, Some("\n\t\t"));
    let params = tree.sub_element(core, "Params", &[("Version", "1")]);
    tree.set_text(params, Some("\n\t\t\t\t"));
    tree.set_tail(params, Some("\n\t\t\t"));
    for (index, param_id) in param_ids.iter().enumerate() {
        let i = index.to_string();
        let entry = tree.sub_element(params, "Param", &[("Index", &i), ("ObjectRef", param_id)]);
        tree.set_tail(entry, Some(if index + 1 < param_ids.len() { "\n\t\t\t\t" } else { "\n\t\t\t" }));
    }
    leaf(tree, core, "ID", slot, "\n\t\t\t");
    leaf(tree, core, "DisplayName", "Graphic Parameters", "\n\t\t\t");
    leaf(tree, core, "Intrinsic", "true", "\n\t\t");
    let private = payload_element(tree, "PremiereFilterPrivateData", &utf16le(&template.private_data), "\n\t\t", stored, uuids);
    tree.append(element, private);
    leaf(tree, element, "MatchName", CAPSULE_MATCH_NAME, "\n\t\t");
    leaf(tree, element, "VideoFilterType", "2", "\n\t");
    element
}

/// `build_blueprint_chain`: the master's chain, holding nothing but the capsule component.
pub fn build_blueprint_chain(tree: &mut Tree, component_id: &str) -> NodeId {
    let element = top(tree, "VideoComponentChain", VIDEO_CHAIN_CLASS_ID, "3", None);
    let chain = tree.sub_element(element, "ComponentChain", &[("Version", "3")]);
    tree.set_text(chain, Some("\n\t\t\t"));
    tree.set_tail(chain, Some("\n\t"));
    let components = tree.sub_element(chain, "Components", &[("Version", "1")]);
    tree.set_text(components, Some("\n\t\t\t\t"));
    tree.set_tail(components, Some("\n\t\t"));
    let entry = tree.sub_element(components, "Component", &[("Index", "0"), ("ObjectRef", component_id)]);
    tree.set_tail(entry, Some("\n\t\t\t"));
    element
}

/// `attach_component`: hang the capsule off a placement's otherwise default chain.
pub fn attach_component(tree: &mut Tree, chain: NodeId, component_id: &str) -> Result<()> {
    let Some(inner) = tree.child_by_tag(chain, "ComponentChain") else {
        return value("component chain has no ComponentChain");
    };
    tree.set_text(inner, Some("\n\t\t\t"));
    let components = tree.sub_element(inner, "Components", &[("Version", "1")]);
    tree.set_text(components, Some("\n\t\t\t\t"));
    tree.set_tail(components, Some("\n\t\t"));
    let entry = tree.sub_element(components, "Component", &[("Index", "0"), ("ObjectRef", component_id)]);
    tree.set_tail(entry, Some("\n\t\t\t"));
    Ok(())
}

/// `build_template_item`: the panel entry for the imported template.
pub fn build_template_item(tree: &mut Tree, item_uid: &str, master_uid: &str, name: &str) -> NodeId {
    let element = top(tree, "ClipProjectItem", ITEM_CLASS_ID, "1", Some(item_uid));
    let project_item = tree.sub_element(element, "ProjectItem", &[("Version", "1")]);
    tree.set_text(project_item, Some("\n\t\t\t"));
    tree.set_tail(project_item, Some("\n\t\t"));
    leaf(tree, project_item, "Name", name, "\n\t\t");
    let master = tree.sub_element(element, "MasterClip", &[("ObjectURef", master_uid)]);
    tree.set_tail(master, Some("\n\t"));
    element
}

/// `build_mgt_stream`.
pub fn build_mgt_stream(tree: &mut Tree, template: &Template) -> NodeId {
    let element = top(tree, "VideoStream", VIDEO_STREAM_CLASS_ID, "22", None);
    leaf(tree, element, "FrameRate", &template.frame_rate.to_string(), "\n\t\t");
    leaf(tree, element, "Duration", &template.duration_ticks.to_string(), "\n\t\t");
    let (width, height) = template.frame_size;
    leaf(tree, element, "FrameRect", &format!("0,0,{width},{height}"), "\n\t\t");
    leaf(tree, element, "CodecType", AE_GRAPHIC_CODEC, "\n\t\t");
    leaf(tree, element, "OriginalColorSpace", &template.color_space, "\n\t\t");
    leaf(tree, element, "AlphaType", &py_str_json(&template.alpha_type), "\n\t\t");
    leaf(tree, element, "OriginalFieldType", &py_str_json(&template.field_type), "\n\t");
    element
}

/// `build_mgt_media`: `RelativePath` is written ahead of `TimeDisplay` and re-derived on every
/// save, which is why the caller hands the element to the project's imported-media list.
#[allow(clippy::too_many_arguments)]
pub fn build_mgt_media(
    tree: &mut Tree,
    template: &Template,
    media_uid: &str,
    stream_id: &str,
    file_path: &Path,
    stored: &mut Stored,
    project_dir: &Path,
    uuids: &mut dyn UuidSource,
) -> NodeId {
    let element = top(tree, "Media", MEDIA_CLASS_ID, "30", Some(media_uid));
    let stream = tree.sub_element(element, "VideoStream", &[("ObjectRef", stream_id)]);
    tree.set_tail(stream, Some("\n\t\t"));
    let prefs = payload_element(tree, "ImporterPrefs", &utf16le(&template.importer_prefs), "\n\t\t", stored, uuids);
    tree.append(element, prefs);
    let file = file_path.to_string_lossy().into_owned();
    leaf(tree, element, "RelativePath", &relpath(&file, &project_dir.to_string_lossy()), "\n\t\t");
    leaf(tree, element, "TimeDisplay", &template.time_display.to_string(), "\n\t\t");
    leaf(tree, element, "FilePath", &file, "\n\t\t");
    leaf(tree, element, "ImplementationID", AE_GRAPHIC_IMPLEMENTATION_ID, "\n\t\t");
    let file_name = file_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    leaf(tree, element, "Title", &format!("{}/{}", template.name, file_name), "\n\t\t");
    leaf(tree, element, "FileKey", &uuids.uuid4().to_string(), "\n\t\t");
    leaf(tree, element, "ActualMediaFilePath", &file, "\n\t");
    element
}

/// `build_mgt_source`.
pub fn build_mgt_source(tree: &mut Tree, template: &Template, media_uid: &str) -> NodeId {
    let element = top(tree, "VideoMediaSource", VIDEO_SOURCE_CLASS_ID, "2", None);
    let source = tree.sub_element(element, "MediaSource", &[("Version", "4")]);
    tree.set_text(source, Some("\n\t\t\t"));
    tree.set_tail(source, Some("\n\t\t"));
    let content = tree.sub_element(source, "Content", &[("Version", "10")]);
    tree.set_text(content, Some("\n\t\t\t"));
    tree.set_tail(content, Some("\n\t\t\t"));
    let media = tree.sub_element(source, "Media", &[("ObjectURef", media_uid)]);
    tree.set_tail(media, Some("\n\t\t"));
    leaf(tree, element, "OriginalDuration", &template.duration_ticks.to_string(), "\n\t");
    element
}

/// `build_mgt_clip`: the master's template clip, or a placement's own copy of it (draws the
/// ClipID).
pub fn build_mgt_clip(tree: &mut Tree, source_id: &str, in_use: bool, uuids: &mut dyn UuidSource) -> NodeId {
    let element = top(tree, "VideoClip", VIDEO_CLIP_CLASS_ID, "11", None);
    let core = tree.sub_element(element, "Clip", &[("Version", "18")]);
    tree.set_text(core, Some("\n\t\t\t"));
    tree.set_tail(core, Some("\n\t"));
    let source = tree.sub_element(core, "Source", &[("ObjectRef", source_id)]);
    tree.set_tail(source, Some("\n\t\t\t"));
    leaf(tree, core, "ClipID", &uuids.uuid4().to_string(), if in_use { "\n\t\t\t" } else { "\n\t\t" });
    if in_use {
        leaf(tree, core, "InUse", "false", "\n\t\t");
    }
    element
}

/// `build_mgt_logging`.
pub fn build_mgt_logging(tree: &mut Tree, template: &Template) -> NodeId {
    let element = top(tree, "ClipLoggingInfo", LOGGING_CLASS_ID, "10", None);
    leaf(tree, element, "CaptureMode", "2", "\n\t\t");
    leaf(tree, element, "ClipName", &template.name, "\n\t\t");
    leaf(tree, element, "TimecodeFormat", &template.time_display.to_string(), "\n\t\t");
    leaf(tree, element, "MediaInPoint", "0", "\n\t\t");
    leaf(tree, element, "MediaOutPoint", &template.duration_ticks.to_string(), "\n\t\t");
    leaf(tree, element, "MediaFrameRate", &template.frame_rate.to_string(), "\n\t");
    element
}

/// `build_mgt_master`.
pub fn build_mgt_master(
    tree: &mut Tree,
    template: &Template,
    master_uid: &str,
    logging_id: &str,
    chain_id: &str,
    clip_id: &str,
    groups_id: &str,
) -> NodeId {
    let element = top(tree, "MasterClip", MASTER_CLASS_ID, "12", Some(master_uid));
    let logging = tree.sub_element(element, "LoggingInfo", &[("ObjectRef", logging_id)]);
    tree.set_tail(logging, Some("\n\t\t"));
    let chain = tree.sub_element(element, "BlueprintVideoComponentChain", &[("ObjectRef", chain_id)]);
    tree.set_tail(chain, Some("\n\t\t"));
    let clips = tree.sub_element(element, "Clips", &[("Version", "1")]);
    tree.set_text(clips, Some("\n\t\t\t"));
    tree.set_tail(clips, Some("\n\t\t"));
    let clip = tree.sub_element(clips, "Clip", &[("Index", "0"), ("ObjectRef", clip_id)]);
    tree.set_tail(clip, Some("\n\t\t"));
    let groups = tree.sub_element(element, "AudioClipChannelGroups", &[("ObjectRef", groups_id)]);
    tree.set_tail(groups, Some("\n\t\t"));
    leaf(tree, element, "Name", &template.name, "\n\t\t");
    leaf(tree, element, "MasterClipChangeVersion", "0", "\n\t");
    element
}

/// `build_media_bin`: the `Motion Graphics Template Media` bin, empty. The caller stamps the
/// `ObjectUID`, as it does for every bin.
pub fn build_media_bin(tree: &mut Tree) -> NodeId {
    let element = top(tree, "BinProjectItem", BIN_CLASS_ID, "1", Some("pending"));
    let project_item = tree.sub_element(element, "ProjectItem", &[("Version", "1")]);
    tree.set_text(project_item, Some("\n\t\t\t"));
    tree.set_tail(project_item, Some("\n\t\t"));
    let node = tree.sub_element(project_item, "Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t\t"));
    tree.set_tail(node, Some("\n\t\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t\t"));
    leaf(tree, properties, "Column.PropertyText.Label", BIN_LABEL, "\n\t\t\t\t");
    leaf(tree, project_item, "Name", MEDIA_FOLDER, "\n\t\t");
    let container = tree.sub_element(element, "ProjectItemContainer", &[("Version", "1")]);
    tree.set_text(container, Some("\n\t\t"));
    tree.set_tail(container, Some("\n\t"));
    element
}

/// A minimal zip reader: end-of-central-directory, central directory, local header; stored and
/// deflate members; CRC-32 checked like `ZipFile.read`.
mod zip {
    use std::io::Read;

    pub struct Archive<'a> {
        data: &'a [u8],
        entries: Vec<Entry>,
    }

    struct Entry {
        name: Vec<u8>,
        method: u16,
        crc: u32,
        compressed: u64,
        offset: u64,
    }

    pub enum ReadError {
        Missing(String),
        Bad(String),
    }

    impl ReadError {
        /// `str(error)` of the exception Python raises.
        pub fn message(&self) -> String {
            match self {
                // str(KeyError(msg)) is repr(msg): double quotes because the text has single ones.
                ReadError::Missing(name) => format!("\"There is no item named '{name}' in the archive\""),
                ReadError::Bad(m) => m.clone(),
            }
        }
    }

    fn u16_at(d: &[u8], o: usize) -> Option<u16> {
        d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32_at(d: &[u8], o: usize) -> Option<u32> {
        d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    impl<'a> Archive<'a> {
        pub fn open(data: &'a [u8]) -> Option<Archive<'a>> {
            // The end record sits within the last 64 KiB + 22 bytes.
            let start = data.len().saturating_sub(65_557);
            let eocd = (start..data.len().checked_sub(21)?).rev().find(|&i| data[i..].starts_with(b"PK\x05\x06"))?;
            let count = u16_at(data, eocd + 10)? as usize;
            let size = u32_at(data, eocd + 12)? as usize;
            let mut offset = u32_at(data, eocd + 16)? as usize;
            // Offsets are relative to the start of the archive; a prepended stub shifts them.
            let concat = eocd.checked_sub(size.checked_add(offset)?)?;
            offset += concat;
            let mut entries = Vec::new();
            for _ in 0..count {
                if data.get(offset..offset + 4)? != b"PK\x01\x02" {
                    return None;
                }
                let method = u16_at(data, offset + 10)?;
                let crc = u32_at(data, offset + 16)?;
                let compressed = u32_at(data, offset + 20)? as u64;
                let name_len = u16_at(data, offset + 28)? as usize;
                let extra_len = u16_at(data, offset + 30)? as usize;
                let comment_len = u16_at(data, offset + 32)? as usize;
                let local = u32_at(data, offset + 42)? as u64 + concat as u64;
                let name = data.get(offset + 46..offset + 46 + name_len)?.to_vec();
                entries.push(Entry { name, method, crc, compressed, offset: local });
                offset += 46 + name_len + extra_len + comment_len;
            }
            Some(Archive { data, entries })
        }

        /// `ZipFile.read(name)`; the last entry of a repeated name wins, like `NameToInfo`.
        pub fn read(&self, name: &str) -> Result<Vec<u8>, ReadError> {
            let Some(entry) = self.entries.iter().rev().find(|e| e.name == name.as_bytes()) else {
                return Err(ReadError::Missing(name.to_string()));
            };
            let bad = |what: &str| ReadError::Bad(format!("{what} for file '{name}'"));
            let o = entry.offset as usize;
            if self.data.get(o..o + 4) != Some(b"PK\x03\x04") {
                return Err(ReadError::Bad("Bad magic number for file header".to_string()));
            }
            let name_len = u16_at(self.data, o + 26).ok_or_else(|| bad("Truncated header"))? as usize;
            let extra_len = u16_at(self.data, o + 28).ok_or_else(|| bad("Truncated header"))? as usize;
            let begin = o + 30 + name_len + extra_len;
            let end = begin + entry.compressed as usize;
            let raw = self.data.get(begin..end).ok_or_else(|| bad("Truncated data"))?;
            let out = match entry.method {
                0 => raw.to_vec(),
                8 => {
                    let mut out = Vec::new();
                    flate2::read::DeflateDecoder::new(raw)
                        .read_to_end(&mut out)
                        .map_err(|e| ReadError::Bad(format!("Error -3 while decompressing data: {e}")))?;
                    out
                }
                other => {
                    return Err(ReadError::Bad(format!("That compression method is not supported ({other})")));
                }
            };
            let mut crc = flate2::Crc::new();
            crc.update(&out);
            if crc.sum() != entry.crc {
                return Err(ReadError::Bad(format!("Bad CRC-32 for file '{name}'")));
            }
            Ok(out)
        }
    }

}
