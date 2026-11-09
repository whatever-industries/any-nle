//! Bounded typed payload serialization for pyavb's bin/attribute family.
//! Ported from pinned pyavb `bin.py`/`attributes.py` at ff64e06 (MIT; NOTICE).
//! This writes one payload with existing object-table references; it is not a
//! file writer or evidence of Avid write compatibility. Reader-normalized
//! strings and BVst's discarded descriptor prefix cannot be byte-preserved.

use crate::io::{Error, Result};
use crate::objects::{Node, Value};
use crate::writer_io::Writer;

fn fail(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}

fn field<'a>(node: &'a Node, name: &str) -> Result<&'a Value> {
    node.get(name)
        .ok_or_else(|| fail(format!("{}: missing {name}", node.class_str())))
}

fn fields_only(node: &Node, allowed: &[&str]) -> Result<()> {
    for (name, _) in &node.fields {
        if !allowed.contains(name) {
            return Err(fail(format!(
                "{}: unsupported field {name}",
                node.class_str()
            )));
        }
    }
    Ok(())
}

fn integer(value: &Value) -> Result<i64> {
    match value {
        Value::Int(v) => Ok(*v),
        _ => Err(fail("expected signed integer")),
    }
}
fn unsigned(value: &Value) -> Result<u64> {
    match value {
        Value::UInt(v) => Ok(*v),
        _ => Err(fail("expected unsigned integer")),
    }
}
fn boolean(value: &Value) -> Result<bool> {
    match value {
        Value::Bool(v) => Ok(*v),
        _ => Err(fail("expected boolean")),
    }
}
fn string(value: &Value) -> Result<&str> {
    match value {
        Value::Str(v) => Ok(v),
        _ => Err(fail("expected string")),
    }
}
fn list(value: &Value) -> Result<&[Value]> {
    match value {
        Value::List(v) => Ok(v),
        _ => Err(fail("expected list")),
    }
}
fn nested(value: &Value) -> Result<&Node> {
    match value {
        Value::Node(v) => Ok(v),
        _ => Err(fail("expected nested node")),
    }
}
fn s16(value: &Value) -> Result<i16> {
    i16::try_from(integer(value)?).map_err(|_| fail("signed 16-bit range"))
}
fn s32(value: &Value) -> Result<i32> {
    i32::try_from(integer(value)?).map_err(|_| fail("signed 32-bit range"))
}
fn u16(value: &Value) -> Result<u16> {
    u16::try_from(integer(value)?).map_err(|_| fail("unsigned 16-bit range"))
}
fn u8(value: &Value) -> Result<u8> {
    u8::try_from(integer(value)?).map_err(|_| fail("unsigned 8-bit range"))
}

fn reference(w: &mut Writer, value: &Value, bound: Option<u32>) -> Result<()> {
    let id = match value {
        Value::Null => 0,
        Value::Ref(id) => *id,
        _ => return Err(fail("expected object reference")),
    };
    if bound.is_some_and(|max| id > max) {
        return Err(fail("object reference exceeds bound"));
    }
    w.object_ref_index(if id == 0 { None } else { Some(id) })
}

fn len_u16(len: usize) -> Result<u16> {
    u16::try_from(len).map_err(|_| fail("item count exceeds u16"))
}
fn len_s16(len: usize) -> Result<i16> {
    i16::try_from(len).map_err(|_| fail("item count exceeds i16"))
}
fn len_u32(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| fail("item count exceeds u32"))
}

fn signed_array<const N: usize>(value: &Value) -> Result<[i16; N]> {
    let items = list(value)?;
    if items.len() != N {
        return Err(fail(format!("expected {N} signed values")));
    }
    let mut result = [0; N];
    for (i, v) in items.iter().enumerate() {
        result[i] = s16(v)?;
    }
    Ok(result)
}
fn unsigned_array<const N: usize>(value: &Value) -> Result<[u16; N]> {
    let items = list(value)?;
    if items.len() != N {
        return Err(fail(format!("expected {N} unsigned values")));
    }
    let mut result = [0; N];
    for (i, v) in items.iter().enumerate() {
        result[i] = u16(v)?;
    }
    Ok(result)
}

fn attributes(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    fields_only(n, &["entries"])?;
    let Value::Attrs(entries) = field(n, "entries")? else {
        return Err(fail("ATTR entries are not attributes"));
    };
    w.bytes(&[2, 1])?;
    w.u32(len_u32(entries.len())?)?;
    for (name, value) in entries {
        let kind = match value {
            Value::Int(_) => 1,
            Value::Str(_) => 2,
            Value::Null | Value::Ref(_) => 3,
            Value::Bytes(_) => 4,
            _ => return Err(fail(format!("ATTR {name}: unsupported value type"))),
        };
        w.u32(kind)?;
        w.string_macroman(name)?;
        match kind {
            1 => w.s32(s32(value)?)?,
            2 => w.string_macroman(string(value)?)?,
            3 => reference(w, value, bound)?,
            4 => {
                let Value::Bytes(bytes) = value else {
                    unreachable!()
                };
                w.u32(len_u32(bytes.len())?)?;
                w.bytes(bytes)?;
            }
            _ => unreachable!(),
        }
    }
    w.u8(3)
}

fn setting(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    w.bytes(&[2, 6])?;
    w.string_macroman(string(field(n, "name")?)?)?;
    w.string_macroman(string(field(n, "kind")?)?)?;
    w.s16(s16(field(n, "attr_count")?)?)?;
    w.s16(s16(field(n, "attr_type")?)?)?;
    reference(w, field(n, "attributes")?, bound)
}

fn bin_view(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    fields_only(
        n,
        &[
            "name",
            "kind",
            "attr_count",
            "attr_type",
            "attributes",
            "columns",
            "format_descriptors",
        ],
    )?;
    setting(n, w, bound)?;
    w.bytes(&[2, 10])?;
    let columns = list(field(n, "columns")?)?;
    w.u16(len_u16(columns.len())?)?;
    for column in columns {
        let c = nested(column)?;
        fields_only(c, &["title", "format", "type", "hidden"])?;
        w.string_macroman(string(field(c, "title")?)?)?;
        w.s16(s16(field(c, "format")?)?)?;
        w.s16(s16(field(c, "type")?)?)?;
        w.bool(boolean(field(c, "hidden")?)?)?;
    }
    if let Some(formats) = n.get("format_descriptors") {
        let formats = list(formats)?;
        w.bytes(&[1, 1, 69])?;
        w.s16(len_s16(formats.len())?)?;
        for descriptor in formats {
            let d = nested(descriptor)?;
            fields_only(d, &["vcid_free_column_id", "format_descriptor"])?;
            let text = string(field(d, "format_descriptor")?)?;
            let size = i32::try_from(text.len()).map_err(|_| fail("descriptor too large"))?;
            let prefix = i16::try_from(
                text.len()
                    .checked_add(2)
                    .ok_or_else(|| fail("descriptor too large"))?,
            )
            .map_err(|_| fail("descriptor prefix exceeds i16"))?;
            w.u8(69)?;
            w.s16(s16(field(d, "vcid_free_column_id")?)?)?;
            w.u8(71)?;
            w.s32(size)?;
            w.u8(76)?;
            w.s16(prefix)?;
            w.s16(0)?;
            w.bytes(text.as_bytes())?;
        }
    }
    w.u8(3)
}

fn bin(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    fields_only(
        n,
        &[
            "large_bin",
            "view_setting",
            "uid",
            "items",
            "display_mask",
            "display_mode",
            "sifted",
            "sifted_settings",
            "sort_columns",
            "mac_font",
            "mac_font_size",
            "mac_image_scale",
            "home_rect",
            "background_color",
            "forground_color",
            "ql_image_scale",
            "attributes",
            "was_iconic",
            "unknown_s32",
        ],
    )?;
    let items = list(field(n, "items")?)?;
    let large = boolean(field(n, "large_bin")?)? || items.len() > u16::MAX as usize;
    w.u8(2)?;
    w.u8(if large { 15 } else { 14 })?;
    reference(w, field(n, "view_setting")?, bound)?;
    w.u64(unsigned(field(n, "uid")?)?)?;
    if large {
        w.u32(len_u32(items.len())?)?;
    } else {
        w.u16(len_u16(items.len())?)?;
    }
    for item in items {
        let i = nested(item)?;
        fields_only(i, &["mob", "x", "y", "keyframe", "user_placed"])?;
        reference(w, field(i, "mob")?, bound)?;
        w.s16(s16(field(i, "x")?)?)?;
        w.s16(s16(field(i, "y")?)?)?;
        w.s32(s32(field(i, "keyframe")?)?)?;
        w.bool(boolean(field(i, "user_placed")?)?)?;
    }
    w.s32(s32(field(n, "display_mask")?)?)?;
    w.s16(s16(field(n, "display_mode")?)?)?;
    w.bool(boolean(field(n, "sifted")?)?)?;
    let sift = list(field(n, "sifted_settings")?)?;
    if sift.len() != 6 {
        return Err(fail("bin requires exactly six sift settings"));
    }
    for setting in sift {
        let triple = list(setting)?;
        if triple.len() != 3 {
            return Err(fail("sift setting must have three values"));
        }
        w.s16(s16(&triple[0])?)?;
        w.string_macroman(string(&triple[1])?)?;
        w.string_macroman(string(&triple[2])?)?;
    }
    let sorts = list(field(n, "sort_columns")?)?;
    w.s16(len_s16(sorts.len())?)?;
    for sort in sorts {
        let pair = list(sort)?;
        if pair.len() != 2 {
            return Err(fail("sort column must have two values"));
        }
        w.u8(u8(&pair[0])?)?;
        w.string_macroman(string(&pair[1])?)?;
    }
    w.s16(s16(field(n, "mac_font")?)?)?;
    w.s16(s16(field(n, "mac_font_size")?)?)?;
    w.s16(s16(field(n, "mac_image_scale")?)?)?;
    w.rect(signed_array(field(n, "home_rect")?)?)?;
    w.rgb_color(unsigned_array(field(n, "background_color")?)?)?;
    w.rgb_color(unsigned_array(field(n, "forground_color")?)?)?;
    w.s16(s16(field(n, "ql_image_scale")?)?)?;
    reference(w, field(n, "attributes")?, bound)?;
    w.bool(boolean(field(n, "was_iconic")?)?)?;
    if n.class == *b"ABIN" {
        if n.get("unknown_s32").is_some() {
            return Err(fail("ABIN has BINF-only unknown_s32"));
        }
        w.u8(3)?;
    } else {
        w.bytes(&[2, 1])?;
        w.s32(s32(field(n, "unknown_s32")?)?)?;
        w.u8(3)?;
    }
    Ok(())
}

/// Serialize a decoded bin-family node into one AVB object payload. A caller
/// may supply the current object-table maximum to reject invalid references.
/// No class header, object-ID allocation, or AVB header is produced.
pub fn serialize(
    node: &Node,
    big_endian: bool,
    byte_limit: usize,
    max_object_id: Option<u32>,
) -> Result<Vec<u8>> {
    let mut w = Writer::with_limit(big_endian, byte_limit);
    match &node.class {
        b"ABIN" | b"BINF" => bin(node, &mut w, max_object_id)?,
        b"ASET" => {
            fields_only(
                node,
                &["name", "kind", "attr_count", "attr_type", "attributes"],
            )?;
            setting(node, &mut w, max_object_id)?;
        }
        b"BVst" => bin_view(node, &mut w, max_object_id)?,
        b"ATTR" => attributes(node, &mut w, max_object_id)?,
        _ => {
            return Err(fail(format!(
                "{} is not a bin-family class",
                node.class_str()
            )))
        }
    }
    Ok(w.into_bytes())
}
