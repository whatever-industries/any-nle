//! Components and their parameters (port of `py_premiere/models/component.py`).
//!
//! A component is a [`ComponentId`] (its record lists the parameter objects and nested
//! sub-components, as the parser builds Python's `Component`); a parameter is its top-level
//! `*ComponentParam` object, a [`NodeId`]. Values decode exactly as Python's do
//! ([`ParamValue`]); the setters take a [`ParamInput`], which keeps Python's distinction
//! between booleans, ints, floats and lists so type refusals carry Python's messages.
//!
//! Not ported here: `Component.add_mask` (the mask builder port owns it).

use super::color::Color;
use super::enums::KeyframeInterpolation;
use super::named_list::NamedList;
use super::project::py_int;
use super::track_item::py_float;
use super::validators::{validate_nonempty_string, validate_vector2, Num, FLOAT32, PACKED_COLOR};
use super::{ComponentId, Project, TrackItemId};
use crate::build::fmt::{format_current_value, format_scalar, format_tangent};
use crate::build::time::Time;
use crate::caption::flat::{
    decode_caption_text, is_payload_text, read_font_family, replace_payload_text, write_font_family, write_payload_element,
    TEXT_MAGIC,
};
use crate::convert::model::py_str_repr;
use crate::error::{not_implemented, type_err, value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{insert_before, remove_child};
use crate::xml::{NodeId, Tree};

/// `_COLOR_CLASS`: ClassID prefix of packed-colour params.
pub const COLOR_CLASS: &str = "0fde4e9f";
/// `_BOOL_CLASS`: ClassID prefix of checkbox params.
pub const BOOL_CLASS: &str = "cc12343e";
/// `_POINT_CLASS`: ClassID prefix of 2D point params.
pub const POINT_CLASS: &str = "ca81d347";
/// `_AUDIO_SCALAR_CLASS`.
pub const AUDIO_SCALAR_CLASS: &str = "a714635e-a628-4b27-9d59-77eba47dbc1a";
/// `_SCALAR_VALUE_CLASSES`: ClassIDs whose value field is a plain float32 scalar.
pub const SCALAR_VALUE_CLASSES: [&str; 3] =
    ["fe47129e-6c94-4fc0-95d5-c056a517aaf3", AUDIO_SCALAR_CLASS, "a4ff2d6e-7ac2-44f8-9d52-17d9ca50e542"];
/// `data/param_classes.py` `VERIFIED_VALUE_CLASS_IDS`.
pub const VERIFIED_VALUE_CLASS_IDS: [&str; 6] = [
    "fe47129e-6c94-4fc0-95d5-c056a517aaf3",
    "0fde4e9f-f895-4ba3-b0fe-9a6feafda583",
    "ca81d347-309b-44d2-acc7-1c572efb973c",
    "2f2eb0a3-318c-4a93-99fc-f1d319edc864",
    "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8",
    "a714635e-a628-4b27-9d59-77eba47dbc1a",
];

const POINT_ENTRY_FIELDS: usize = 14;
const SPATIAL_START: usize = 10;
const PATH_VERSION: u32 = 2;
const PATH_STRIDE: usize = 7;
/// `_DEFAULT_PATH_ID`: the path id a written subpath carries.
pub const DEFAULT_PATH_ID: u32 = 31062;

/// A decoded parameter value (`50.` -> `Float`, `true` -> `Bool`, `x:y` -> `List`, anything
/// else the raw field as `Str`).
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// A scalar.
    Float(f64),
    /// A checkbox.
    Bool(bool),
    /// A multi-component value (a point).
    List(Vec<f64>),
    /// An encoding py_premiere does not decode, verbatim.
    Str(String),
}

/// What a value setter is given (Python accepts any of these and refuses by type).
#[derive(Clone, Debug, PartialEq)]
pub enum ParamInput {
    /// A `bool`.
    Bool(bool),
    /// An `int` or `float`.
    Num(Num),
    /// A list (or tuple) of numbers.
    List(Vec<Num>),
}

impl ParamInput {
    fn type_name(&self) -> &'static str {
        match self {
            ParamInput::Bool(_) => "bool",
            ParamInput::Num(n) => n.type_name(),
            ParamInput::List(_) => "list",
        }
    }
}

/// One vertex of a shape path, with its two bezier handles (`PathVertex`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathVertex {
    /// X.
    pub x: f64,
    /// Y.
    pub y: f64,
    /// Incoming handle x.
    pub in_x: f64,
    /// Incoming handle y.
    pub in_y: f64,
    /// Outgoing handle x.
    pub out_x: f64,
    /// Outgoing handle y.
    pub out_y: f64,
    /// The leading per-vertex float (0 in every vertex seen so far).
    pub flag: f64,
}

/// The motion-path handles of a 2D keyframe (`SpatialTangents`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpatialTangents {
    /// Incoming x.
    pub in_x: f64,
    /// Incoming y.
    pub in_y: f64,
    /// Outgoing x.
    pub out_x: f64,
    /// Outgoing y.
    pub out_y: f64,
}

/// `_SCALAR.match(field)`: `-?\d+(\.\d*)?([eE][-+]?\d+)?\Z` (ASCII digits).
fn is_scalar(field: &str) -> bool {
    let b = field.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if !digits(&mut i) {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let save = i;
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        if !digits(&mut i) {
            i = save;
        }
    }
    i == b.len()
}

/// `_decode_value`.
pub fn decode_value(field: &str) -> Result<ParamValue> {
    if field == "true" {
        return Ok(ParamValue::Bool(true));
    }
    if field == "false" {
        return Ok(ParamValue::Bool(false));
    }
    if field.contains(':') {
        let parts: Vec<&str> = field.split(':').collect();
        if parts.iter().all(|p| is_scalar(p)) {
            return Ok(ParamValue::List(parts.iter().map(|p| py_float(p)).collect::<Result<_>>()?));
        }
        return Ok(ParamValue::Str(field.to_string()));
    }
    if is_scalar(field) {
        return Ok(ParamValue::Float(py_float(field)?));
    }
    Ok(ParamValue::Str(field.to_string()))
}

/// `_split_keyframe`: `(ticks, value field)` of one keyframe entry.
fn split_keyframe(entry: &str) -> Result<(i64, String)> {
    let (ticks_text, rest) = entry.split_once(',').unwrap_or((entry, ""));
    let value_field = rest.split_once(',').map_or(rest, |(v, _)| v);
    Ok((py_int(ticks_text)?, value_field.to_string()))
}

/// `_unpack_color`: `0xAA00RR00GG00BB00`.
pub fn unpack_color(packed: i128) -> Color {
    Color {
        red: ((packed >> 40) & 0xFF) as i64,
        green: ((packed >> 24) & 0xFF) as i64,
        blue: ((packed >> 8) & 0xFF) as i64,
        alpha: ((packed >> 56) & 0xFF) as i64,
    }
}

/// `_pack_color`.
pub fn pack_color(color: Color) -> i128 {
    ((color.alpha as i128) << 56) | ((color.red as i128) << 40) | ((color.green as i128) << 24) | ((color.blue as i128) << 8)
}

fn float32(value: f64) -> f64 {
    value as f32 as f64
}

/// `_build_keyframe_string`: `ticks,value,interp,0,inSlope,1/6,outSlope,outInf;...;`, slopes
/// from the float32-rounded values, a HOLD key leaving flat with a 1/3 influence.
pub fn build_keyframe_string(keys: &[(Time, f64, KeyframeInterpolation)]) -> Result<String> {
    let keys: Vec<(Time, f64, KeyframeInterpolation)> = keys.iter().map(|&(t, v, i)| (t, float32(v), i)).collect();
    let mut entries = Vec::new();
    for (index, &(time, value, interpolation)) in keys.iter().enumerate() {
        let in_slope = if index > 0 {
            let previous = keys[index - 1];
            let seconds = (time - previous.0).seconds();
            (value - previous.1) / seconds
        } else {
            0.0
        };
        let hold = interpolation == KeyframeInterpolation::Hold;
        let out_slope = if index < keys.len() - 1 && !hold {
            let following = keys[index + 1];
            let seconds = (following.0 - time).seconds();
            (following.1 - value) / seconds
        } else {
            0.0
        };
        let out_influence = if hold { 1.0 / 3.0 } else { 1.0 / 6.0 };
        entries.push(format!(
            "{},{},{},0,{},{},{},{}",
            time.ticks,
            format_scalar(value)?,
            interpolation.value(),
            format_tangent(in_slope)?,
            format_tangent(1.0 / 6.0)?,
            format_tangent(out_slope)?,
            format_tangent(out_influence)?
        ));
    }
    Ok(entries.join(";") + ";")
}

fn u32_le(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])
}

fn f32_le(raw: &[u8], at: usize) -> f64 {
    f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]) as f64
}

/// `_decode_subpaths`: the drawn-mask subpath layout, `None` when the bytes do not fit it.
fn decode_subpaths(raw: &[u8], subpath_count: u32) -> Option<Vec<(u32, Vec<PathVertex>)>> {
    const HEADER: usize = 13;
    const VERTEX: usize = 28;
    let mut offset = 8;
    let mut subpaths = Vec::new();
    for _ in 0..subpath_count {
        if offset + HEADER > raw.len() {
            return None;
        }
        let path_id = u32_le(raw, offset);
        let pad = raw[offset + 4];
        let sub_version = u32_le(raw, offset + 5);
        let vertex_count = u32_le(raw, offset + 9);
        if pad != 0 || sub_version != PATH_VERSION {
            return None;
        }
        offset += HEADER;
        let mut vertices = Vec::new();
        for _ in 0..vertex_count {
            if offset + VERTEX > raw.len() {
                return None;
            }
            vertices.push(PathVertex {
                flag: u32_le(raw, offset) as f64,
                x: f32_le(raw, offset + 4),
                y: f32_le(raw, offset + 8),
                in_x: f32_le(raw, offset + 12),
                in_y: f32_le(raw, offset + 16),
                out_x: f32_le(raw, offset + 20),
                out_y: f32_le(raw, offset + 24),
            });
            offset += VERTEX;
        }
        subpaths.push((path_id, vertices));
    }
    if offset + 1 != raw.len() {
        return None;
    }
    Some(subpaths)
}

fn pack_f32(out: &mut Vec<u8>, v: f64) -> Result<()> {
    let f = v as f32;
    if f.is_infinite() && v.is_finite() {
        return Err(Error::Overflow("float too large to pack with f format".into()));
    }
    out.extend_from_slice(&f.to_le_bytes());
    Ok(())
}

/// `_encode_subpaths`.
fn encode_subpaths(subpaths: &[(u32, &[PathVertex])], closed: bool) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&PATH_VERSION.to_le_bytes());
    out.extend_from_slice(&(subpaths.len() as u32).to_le_bytes());
    for (path_id, vertices) in subpaths {
        out.extend_from_slice(&path_id.to_le_bytes());
        out.push(0);
        out.extend_from_slice(&PATH_VERSION.to_le_bytes());
        out.extend_from_slice(&(vertices.len() as u32).to_le_bytes());
        for vertex in vertices.iter() {
            let flag = super::track_item::py_float_to_int(vertex.flag)?;
            let flag = u32::try_from(flag).map_err(|_| Error::Struct("argument out of range".into()))?;
            out.extend_from_slice(&flag.to_le_bytes());
            for v in [vertex.x, vertex.y, vertex.in_x, vertex.in_y, vertex.out_x, vertex.out_y] {
                pack_f32(&mut out, v)?;
            }
        }
    }
    out.push(if closed { 1 } else { 0 });
    Ok(out)
}

fn text_of(tree: &Tree, element: NodeId, tag: &str) -> Option<String> {
    tree.child_by_tag(element, tag).map(|e| tree[e].text_or_empty().to_string())
}

/// `ComponentParam.display_name`.
pub fn param_display_name(tree: &Tree, param: NodeId) -> String {
    text_of(tree, param, "Name").unwrap_or_default()
}

/// `ComponentParam.class_id`.
pub fn param_class_id(tree: &Tree, param: NodeId) -> String {
    tree.get(param, "ClassID").unwrap_or("").to_string()
}

/// `ComponentParam.is_time_varying`: the flag, or (audio params) a populated `Keyframes`.
pub fn param_is_time_varying(tree: &Tree, param: NodeId) -> bool {
    if text_of(tree, param, "IsTimeVarying").as_deref() == Some("true") {
        return true;
    }
    !crate::pystr::py_strip(&text_of(tree, param, "Keyframes").unwrap_or_default()).is_empty()
}

fn start_keyframe(tree: &Tree, param: NodeId) -> Result<Option<(i64, String)>> {
    match text_of(tree, param, "StartKeyframe") {
        Some(t) if !t.is_empty() => split_keyframe(&t).map(Some),
        _ => Ok(None),
    }
}

fn keyframe_entries(tree: &Tree, param: NodeId) -> Result<Vec<(i64, String)>> {
    match text_of(tree, param, "Keyframes") {
        Some(t) if !t.is_empty() => t.split(';').filter(|e| !e.is_empty()).map(split_keyframe).collect(),
        _ => Ok(Vec::new()),
    }
}

/// `ComponentParam.value`: the static value, `None` when nothing is stored.
pub fn param_value(tree: &Tree, param: NodeId) -> Result<Option<ParamValue>> {
    match start_keyframe(tree, param)? {
        None => Ok(None),
        Some((_, field)) => decode_value(&field).map(Some),
    }
}

/// `ComponentParam.keys`: the keyframe times, in stored order.
pub fn param_keys(tree: &Tree, param: NodeId) -> Result<Vec<Time>> {
    Ok(keyframe_entries(tree, param)?.into_iter().map(|(t, _)| Time::new(t)).collect())
}

/// `ComponentParam.get_value_at_key(time)`.
pub fn param_value_at_key(tree: &Tree, param: NodeId, time: Time) -> Result<Option<ParamValue>> {
    for (ticks, field) in keyframe_entries(tree, param)? {
        if ticks == time.ticks {
            return decode_value(&field).map(Some);
        }
    }
    Ok(None)
}

/// `ComponentParam.find_nearest_key(time, threshold)`: ties go to the earlier stored key.
pub fn param_find_nearest_key(tree: &Tree, param: NodeId, time: Time, threshold: Option<Time>) -> Result<Option<Time>> {
    let mut best: Option<(Time, i128)> = None;
    for key in param_keys(tree, param)? {
        let distance = (key.ticks as i128 - time.ticks as i128).abs();
        if best.is_none_or(|(_, d)| distance < d) {
            best = Some((key, distance));
        }
    }
    let Some((key, distance)) = best else { return Ok(None) };
    if let Some(t) = threshold {
        if distance > t.ticks as i128 {
            return Ok(None);
        }
    }
    Ok(Some(key))
}

/// `ComponentParam.find_next_key(time)`: the first key strictly after `time`.
pub fn param_find_next_key(tree: &Tree, param: NodeId, time: Time) -> Result<Option<Time>> {
    let mut best: Option<Time> = None;
    for key in param_keys(tree, param)? {
        if key.ticks > time.ticks && best.is_none_or(|b| key.ticks < b.ticks) {
            best = Some(key);
        }
    }
    Ok(best)
}

/// `ComponentParam.find_previous_key(time)`: the last key strictly before `time`.
pub fn param_find_previous_key(tree: &Tree, param: NodeId, time: Time) -> Result<Option<Time>> {
    let mut best: Option<Time> = None;
    for key in param_keys(tree, param)? {
        if key.ticks < time.ticks && best.is_none_or(|b| key.ticks > b.ticks) {
            best = Some(key);
        }
    }
    Ok(best)
}

/// Python `int(text)` for packed colours (beyond `i64`).
fn int128(text: &str) -> Result<i128> {
    match py_int(text) {
        Ok(v) => Ok(v as i128),
        Err(e) => {
            let t = crate::pystr::py_strip(text).replace('_', "");
            t.parse::<i128>().map_err(|_| e)
        }
    }
}

/// `ComponentParam.color`: a colour param's static value as RGBA.
pub fn param_color(tree: &Tree, param: NodeId) -> Result<Option<Color>> {
    if !param_class_id(tree, param).starts_with(COLOR_CLASS) {
        return Ok(None);
    }
    let Some((_, field)) = start_keyframe(tree, param)? else { return Ok(None) };
    let digits = field.trim_start_matches('-');
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    Ok(Some(unpack_color(int128(&field)?)))
}

fn interpolation_field(tree: &Tree, param: NodeId, time: Time, index: usize) -> Result<Option<KeyframeInterpolation>> {
    let text = text_of(tree, param, "Keyframes").unwrap_or_default();
    if text.is_empty() {
        return Ok(None);
    }
    let wanted = time.ticks.to_string();
    for entry in text.split(';') {
        let fields: Vec<&str> = entry.split(',').collect();
        if fields.len() > index && fields[0] == wanted {
            return KeyframeInterpolation::from_value(py_int(fields[index])?).map(Some);
        }
    }
    Ok(None)
}

/// `ComponentParam.get_interpolation_at_key(time)`: the incoming interpolation.
pub fn param_interpolation_at_key(tree: &Tree, param: NodeId, time: Time) -> Result<Option<KeyframeInterpolation>> {
    interpolation_field(tree, param, time, 2)
}

/// `ComponentParam.get_out_interpolation_at_key(time)`.
pub fn param_out_interpolation_at_key(tree: &Tree, param: NodeId, time: Time) -> Result<Option<KeyframeInterpolation>> {
    interpolation_field(tree, param, time, 3)
}

/// `ComponentParam.get_spatial_tangents_at_key(time)`: the four spatial handles of a 2D key.
pub fn param_spatial_tangents_at_key(tree: &Tree, param: NodeId, time: Time) -> Result<Option<SpatialTangents>> {
    let text = match text_of(tree, param, "Keyframes").filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => text_of(tree, param, "StartKeyframe").unwrap_or_default(),
    };
    if text.is_empty() {
        return Ok(None);
    }
    let wanted = time.ticks.to_string();
    for entry in text.split(';') {
        let fields: Vec<&str> = entry.split(',').collect();
        if fields.len() != POINT_ENTRY_FIELDS || fields[0] != wanted {
            continue;
        }
        let h: Vec<f64> = fields[SPATIAL_START..].iter().map(|v| py_float(v)).collect::<Result<_>>()?;
        return Ok(Some(SpatialTangents { in_x: h[0], in_y: h[1], out_x: h[2], out_y: h[3] }));
    }
    Ok(None)
}

/// `repr(param)`.
pub fn param_repr(tree: &Tree, param: NodeId) -> String {
    format!("ComponentParam(display_name={})", py_str_repr(&param_display_name(tree, param)))
}

/// `_encode_static_value`: the stored field for a new static value, by parameter class.
fn encode_static_value(class_id: &str, input: &ParamInput) -> Result<String> {
    if SCALAR_VALUE_CLASSES.contains(&class_id) {
        let ParamInput::Num(n) = input else {
            return type_err(format!("expected a number, got {}", input.type_name()));
        };
        FLOAT32.check(*n)?;
        return format_scalar(n.as_f64());
    }
    if class_id.starts_with(BOOL_CLASS) {
        let ParamInput::Bool(b) = input else { return type_err(format!("expected a bool, got {}", input.type_name())) };
        return Ok(if *b { "true" } else { "false" }.to_string());
    }
    if class_id.starts_with(POINT_CLASS) {
        let ParamInput::List(items) = input else {
            return type_err(format!("expected a 2-tuple of numbers, got {}", input.type_name()));
        };
        if items.len() != 2 {
            return type_err("expected a 2-tuple of numbers, got list");
        }
        validate_vector2([items[0], items[1]])?;
        return Ok(format!("{}:{}", format_scalar(items[0].as_f64())?, format_scalar(items[1].as_f64())?));
    }
    if class_id.starts_with(COLOR_CLASS) {
        let n = match input {
            ParamInput::Num(n) => *n,
            other => return type_err(format!("expected an integer, got {}", other.type_name())),
        };
        PACKED_COLOR.check(n)?;
        return Ok(n.to_string());
    }
    value("value is only settable on scalar, boolean, point and color parameters")
}

/// `ComponentParam.value = new_value`: rewrite the static value (refused on a keyframed
/// parameter or one with no stored value); scalar classes mirror it into `CurrentValue`.
pub fn set_param_value(tree: &mut Tree, param: NodeId, input: &ParamInput) -> Result<()> {
    if param_is_time_varying(tree, param) {
        return value("cannot set a static value on a keyframed parameter");
    }
    let class_id = param_class_id(tree, param);
    let field = encode_static_value(&class_id, input)?;
    let element = tree.child_by_tag(param, "StartKeyframe");
    let Some(element) = element.filter(|&e| !tree[e].text_or_empty().is_empty()) else {
        return value("parameter has no stored value to set");
    };
    let mut fields: Vec<String> = tree[element].text_or_empty().split(',').map(str::to_string).collect();
    if fields.len() < 2 {
        return Err(Error::Index("list assignment index out of range".into()));
    }
    fields[1] = field;
    tree.set_text(element, Some(fields.join(",")));
    if SCALAR_VALUE_CLASSES.contains(&class_id.as_str()) {
        if let Some(current) = tree.child_by_tag(param, "CurrentValue") {
            let ParamInput::Num(n) = input else { unreachable!("checked by encode_static_value") };
            tree.set_text(current, Some(format_current_value(n.as_f64())?));
        }
    }
    Ok(())
}

/// `ComponentParam.color = color`: validate and write the packed value through the value setter.
pub fn set_param_color(tree: &mut Tree, param: NodeId, color: Color) -> Result<()> {
    if !param_class_id(tree, param).starts_with(COLOR_CLASS) {
        return value("not a color parameter");
    }
    super::validators::validate_color(color.channels())?;
    set_param_value(tree, param, &ParamInput::Num(Num::Int(pack_color(color))))
}

/// `ComponentParam.set_keyframes(keys)`: keyframe a scalar parameter (tangents auto-computed).
/// A static parameter turns time-varying: `Keyframes` is created after `CurrentValue` (audio)
/// or `StartKeyframe`, and the `IsTimeVarying` flag set, created or (audio scalars) removed.
pub fn set_param_keyframes(tree: &mut Tree, param: NodeId, keys: &[(Time, Num, KeyframeInterpolation)]) -> Result<()> {
    let class_id = param_class_id(tree, param);
    if !SCALAR_VALUE_CLASSES.contains(&class_id.as_str()) {
        return value("keyframes are only settable on scalar parameters");
    }
    if keys.is_empty() {
        return value("at least one keyframe is required");
    }
    for (_, v, _) in keys {
        FLOAT32.check(*v)?;
    }
    let mut ordered: Vec<(Time, f64, KeyframeInterpolation)> = keys.iter().map(|&(t, v, i)| (t, v.as_f64(), i)).collect();
    ordered.sort_by_key(|k| k.0.ticks);
    for pair in ordered.windows(2) {
        if pair[0].0.ticks == pair[1].0.ticks {
            return value(format!("duplicate keyframe time {}", pair[0].0.ticks));
        }
    }
    let text = build_keyframe_string(&ordered)?;
    let element = match tree.child_by_tag(param, "Keyframes") {
        Some(e) => e,
        None => {
            let Some(start) = tree.child_by_tag(param, "StartKeyframe") else {
                return value("parameter has no stored value to keyframe");
            };
            let flag = tree.child_by_tag(param, "IsTimeVarying");
            if class_id == AUDIO_SCALAR_CLASS {
                if let Some(f) = flag {
                    remove_child(tree, param, f)?;
                }
            } else if let Some(f) = flag {
                tree.set_text(f, Some("true"));
            } else {
                create_time_varying_flag(tree, param)?;
            }
            let anchor = tree.child_by_tag(param, "CurrentValue").unwrap_or(start);
            let element = tree.new_element("Keyframes");
            let tail = tree.tail(anchor).map(str::to_string);
            tree.set_tail(element, tail);
            let position = tree.children(param).iter().position(|&c| c == anchor).expect("anchor is a child");
            tree.insert(param, position + 1, element);
            element
        }
    };
    tree.set_text(element, Some(text));
    Ok(())
}

/// `_create_time_varying_flag`: `<IsTimeVarying>true` right after `ParameterControlType`
/// (or `Name`).
fn create_time_varying_flag(tree: &mut Tree, param: NodeId) -> Result<()> {
    let tags: Vec<String> = tree.children(param).iter().map(|&c| tree.tag(c).to_string()).collect();
    let Some(position) = ["ParameterControlType", "Name"]
        .iter()
        .find_map(|anchor| tags.iter().position(|t| t == anchor).map(|p| p + 1))
    else {
        return value("parameter has no anchor to create <IsTimeVarying>");
    };
    let Some(next) = tags.get(position) else { return Err(Error::Index("list index out of range".into())) };
    let flag = tree.new_element("IsTimeVarying");
    tree.set_text(flag, Some("true"));
    insert_before(tree, param, next, flag)
}

impl Project {
    /// `ComponentParam._payload`: the decoded `StartKeyframeValue` payload (`None` when absent
    /// or undecodable).
    fn param_payload(&mut self, param: NodeId) -> Option<Vec<u8>> {
        let element = self.doc.tree.child_by_tag(param, "StartKeyframeValue")?;
        self.doc.payload(element).ok().flatten()
    }

    /// `ComponentParam.text`: a `FormattedTextData` payload's text, a template control's
    /// UTF-16LE text, or `None`.
    pub fn param_text(&mut self, param: NodeId) -> Result<Option<String>> {
        let Some(raw) = self.param_payload(param) else { return Ok(None) };
        if raw.get(8..12) == Some(&TEXT_MAGIC[..]) {
            return Ok(Some(decode_caption_text(&raw)?));
        }
        if raw.len() % 2 != 0 {
            return Ok(None);
        }
        let units = raw.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
        let Ok(decoded) = char::decode_utf16(units).collect::<std::result::Result<String, _>>() else { return Ok(None) };
        Ok(if is_payload_text(&decoded) { Some(decoded) } else { None })
    }

    /// `ComponentParam.font_family` (`None` without a `FormattedTextData` payload).
    pub fn param_font_family(&mut self, param: NodeId) -> Result<Option<String>> {
        let Some(raw) = self.param_payload(param) else { return Ok(None) };
        if raw.get(8..12) != Some(&TEXT_MAGIC[..]) {
            return Ok(None);
        }
        Ok(Some(read_font_family(&raw)?))
    }

    fn styled_text_payload(&mut self, param: NodeId) -> Result<(Vec<u8>, NodeId)> {
        let raw = self.param_payload(param);
        let element = self.doc.tree.child_by_tag(param, "StartKeyframeValue");
        let (Some(raw), Some(element)) = (raw, element) else { return value("parameter holds no text payload") };
        if raw.get(8..12) != Some(&TEXT_MAGIC[..]) {
            return not_implemented(
                "only FormattedTextData payloads are writable; this one is a Motion Graphics template control",
            );
        }
        Ok((raw, element))
    }

    /// `ComponentParam.text = value` (styled-text payloads only).
    pub fn set_param_text(&mut self, param: NodeId, text: &str) -> Result<()> {
        validate_nonempty_string(text)?;
        let (raw, element) = self.styled_text_payload(param)?;
        let payload = replace_payload_text(&raw, text)?;
        Ok(write_payload_element(&mut self.doc, element, &payload)?)
    }

    /// `ComponentParam.font_family = value`.
    pub fn set_param_font_family(&mut self, param: NodeId, family: &str) -> Result<()> {
        validate_nonempty_string(family)?;
        let (raw, element) = self.styled_text_payload(param)?;
        let payload = write_font_family(&raw, family)?;
        Ok(write_payload_element(&mut self.doc, element, &payload)?)
    }

    /// `ComponentParam.path`: the shape a `Path` parameter stores (`None` when none; an
    /// empty list for an undrawn mask).
    pub fn param_path(&mut self, param: NodeId) -> Option<Vec<PathVertex>> {
        let raw = self.param_payload(param)?;
        if raw.len() < 8 {
            return None;
        }
        let (version, count) = (u32_le(&raw, 0), u32_le(&raw, 4));
        if version != PATH_VERSION {
            return None;
        }
        let expected = count as usize * PATH_STRIDE;
        if raw.len() == 8 + expected * 4 + usize::from(count != 0) {
            return Some(
                (0..count as usize)
                    .map(|v| {
                        let at = |k: usize| f32_le(&raw, 8 + (v * PATH_STRIDE + k) * 4);
                        PathVertex { flag: at(0), x: at(1), y: at(2), in_x: at(3), in_y: at(4), out_x: at(5), out_y: at(6) }
                    })
                    .collect(),
            );
        }
        let subpaths = decode_subpaths(&raw, count)?;
        Some(subpaths.into_iter().flat_map(|(_, v)| v).collect())
    }

    /// `ComponentParam.path = vertices`: one closed subpath (or the bare header for none),
    /// under a fresh `BinaryHash` (draws one UUID).
    pub fn set_param_path(&mut self, param: NodeId, vertices: &[PathVertex], uuids: &mut dyn UuidSource) -> Result<()> {
        let element = self.doc.tree.child_by_tag(param, "StartKeyframeValue");
        let Some(element) = element.filter(|_| param_display_name(&self.doc.tree, param) == "Path") else {
            return value("path is only settable on a Path parameter");
        };
        for v in vertices {
            for c in [v.x, v.y, v.in_x, v.in_y, v.out_x, v.out_y, v.flag] {
                if !c.is_finite() {
                    return value("path coordinates must be finite");
                }
            }
        }
        let raw = if vertices.is_empty() {
            let mut raw = PATH_VERSION.to_le_bytes().to_vec();
            raw.extend_from_slice(&0u32.to_le_bytes());
            raw
        } else {
            encode_subpaths(&[(DEFAULT_PATH_ID, vertices)], true)?
        };
        let tree = &mut self.doc.tree;
        tree.set(element, "Encoding", "base64");
        tree.set(element, "BinaryHash", uuids.uuid4().to_string());
        tree.set_text(element, Some(format!("{}\n\t\t", crate::base64::b64encode(&raw))));
        self.doc.invalidate_payloads();
        Ok(())
    }

    /// `Component.display_name`.
    pub fn component_display_name(&self, component: ComponentId) -> String {
        let element = self.component(component).element;
        component_inner(&self.doc.tree, element).and_then(|inner| text_of(&self.doc.tree, inner, "DisplayName")).unwrap_or_default()
    }

    /// `Component.match_name`: `MatchName`, or an audio filter's `FilterMatchName`.
    pub fn component_match_name(&self, component: ComponentId) -> String {
        let tree = &self.doc.tree;
        let element = self.component(component).element;
        match text_of(tree, element, "MatchName").filter(|m| !m.is_empty()) {
            Some(m) => m,
            None => text_of(tree, element, "FilterMatchName").unwrap_or_default(),
        }
    }

    /// `Component.properties`: the parameters, indexable by display name.
    pub fn component_properties(&self, component: ComponentId) -> NamedList<NodeId> {
        let items = self.component(component).properties.clone();
        let keys = items.iter().map(|&p| vec![Ok(param_display_name(&self.doc.tree, p))]).collect();
        NamedList::new(items, keys)
    }

    /// `Component.sub_components` (masks applied to an effect), by display or match name.
    pub fn component_sub_components(&self, component: ComponentId) -> NamedList<ComponentId> {
        self.components_named(self.component(component).sub_components.clone())
    }

    /// A component list as `NamedList(..., keys=("display_name", "match_name"))`.
    pub fn components_named(&self, items: Vec<ComponentId>) -> NamedList<ComponentId> {
        let keys = items.iter().map(|&c| vec![Ok(self.component_display_name(c)), Ok(self.component_match_name(c))]).collect();
        NamedList::new(items, keys)
    }

    /// `repr(component)`.
    pub fn component_repr(&self, component: ComponentId) -> String {
        format!(
            "Component(display_name={}, {} param(s))",
            py_str_repr(&self.component_display_name(component)),
            self.component(component).properties.len()
        )
    }

    /// `Component.track_item`.
    pub fn component_track_item(&self, component: ComponentId) -> TrackItemId {
        self.component(component).track_item
    }

    /// `ComponentParam.component`: the component listing this parameter among `item`'s
    /// components, selection components and their sub-components (`None` for the time-remap
    /// speed param, whose owner is the track item itself).
    pub fn param_component(&self, item: TrackItemId, param: NodeId) -> Option<ComponentId> {
        let data = self.track_item(item);
        let mut stack: Vec<ComponentId> = data.components.iter().chain(&data.selection_components).copied().collect();
        while let Some(c) = stack.pop() {
            if self.component(c).properties.contains(&param) {
                return Some(c);
            }
            stack.extend(self.component(c).sub_components.iter().copied());
        }
        None
    }
}

/// `Component._inner`: the nested `Component` (audio components one level deeper).
pub fn component_inner(tree: &Tree, element: NodeId) -> Option<NodeId> {
    tree.child_by_tag(element, "Component").or_else(|| tree.find1(element, "AudioComponent/Component"))
}
