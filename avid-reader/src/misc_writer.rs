//! Bounded typed payload serialization for selected pyavb `misc.py` classes.
//! Source: pinned pyavb ff64e06 (MIT; see NOTICE). One payload only; no AVB
//! object allocation, file writer, or Avid read-back. MPGP and effect/tracker
//! classes are intentionally unsupported here.

use crate::io::{Error, MobId, Result};
use crate::objects::{Node, Value};
use crate::writer_io::Writer;

fn err(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}
fn field<'a>(n: &'a Node, key: &str) -> Result<&'a Value> {
    n.get(key)
        .ok_or_else(|| err(format!("{}: missing {key}", n.class_str())))
}
fn fields_only(n: &Node, allowed: &[&str]) -> Result<()> {
    for (key, _) in &n.fields {
        if !allowed.contains(key) {
            return Err(err(format!("{}: unsupported field {key}", n.class_str())));
        }
    }
    Ok(())
}
fn s32(v: &Value) -> Result<i32> {
    match v {
        Value::Int(i) => i32::try_from(*i).map_err(|_| err("outside s32 range")),
        _ => Err(err("expected integer")),
    }
}
fn s16(v: &Value) -> Result<i16> {
    match v {
        Value::Int(i) => i16::try_from(*i).map_err(|_| err("outside s16 range")),
        _ => Err(err("expected integer")),
    }
}
fn u16(v: &Value) -> Result<u16> {
    match v {
        Value::Int(i) => u16::try_from(*i).map_err(|_| err("outside u16 range")),
        _ => Err(err("expected integer")),
    }
}
fn u64(v: &Value) -> Result<u64> {
    match v {
        Value::UInt(i) => Ok(*i),
        _ => Err(err("expected unsigned integer")),
    }
}
fn boolean(v: &Value) -> Result<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(err("expected boolean")),
    }
}
fn string(v: &Value) -> Result<&str> {
    match v {
        Value::Str(s) => Ok(s),
        _ => Err(err("expected string")),
    }
}
fn mob_id(n: &Node) -> Result<&MobId> {
    match field(n, "mob_id")? {
        Value::MobId(id) => Ok(id),
        _ => Err(err("expected MobID")),
    }
}
fn object_ref(w: &mut Writer, v: &Value, max_id: Option<u32>) -> Result<()> {
    let id = match v {
        Value::Null => 0,
        Value::Ref(id) => *id,
        _ => return Err(err("expected object reference")),
    };
    if max_id.is_some_and(|max| id > max) {
        return Err(err("reference exceeds object-table bound"));
    }
    w.object_ref_index(if id == 0 { None } else { Some(id) })
}

fn position_base(n: &Node, w: &mut Writer) -> Result<()> {
    let id = mob_id(n)?;
    w.bytes(&[2, 1])?;
    w.u32(id.material.data1)?;
    w.u32(id.material.data2 as u32 + ((id.material.data3 as u32) << 16))?;
    w.bytes(&[1, 1])?;
    w.mob_id(id)
}

fn position(n: &Node, w: &mut Writer) -> Result<()> {
    let allowed: &[&str] = match &n.class {
        b"APOS" => &["mob_id"],
        b"ABOB" => &[
            "mob_id",
            "sample_num",
            "length",
            "track_type",
            "track_index",
        ],
        b"DIDP" => &[
            "mob_id",
            "sample_num",
            "length",
            "track_type",
            "track_index",
            "strip",
            "offset",
            "byte_length",
            "spos_invalid",
        ],
        b"MPGP" => &[
            "mob_id",
            "sample_num",
            "length",
            "track_type",
            "track_index",
            "strip",
            "offset",
            "byte_length",
            "spos_invalid",
            "trailing_discards",
            "need_seq_hdr",
            "fields",
        ],
        _ => return Err(err("position class unavailable")),
    };
    fields_only(n, allowed)?;
    position_base(n, w)?;
    if n.class == *b"APOS" {
        return w.u8(3);
    }
    w.bytes(&[2, 1])?;
    w.s32(s32(field(n, "sample_num")?)?)?;
    w.s32(s32(field(n, "length")?)?)?;
    w.s16(s16(field(n, "track_type")?)?)?;
    w.s16(s16(field(n, "track_index")?)?)?;
    if n.class == *b"ABOB" {
        return w.u8(3);
    }
    w.bytes(&[2, 1])?;
    w.s32(s32(field(n, "strip")?)?)?;
    w.u64(u64(field(n, "offset")?)?)?;
    w.u64(u64(field(n, "byte_length")?)?)?;
    w.bool(boolean(field(n, "spos_invalid")?)?)?;
    if n.class == *b"DIDP" {
        return w.u8(3);
    }
    // Pinned pyavb does not retain leading_discard_fields. Its writer always
    // canonicalizes the value as two times the number of picture fields.
    w.bytes(&[2, 1])?;
    w.s16(s16(field(n, "trailing_discards")?)?)?;
    w.bool(boolean(field(n, "need_seq_hdr")?)?)?;
    let Value::List(fields) = field(n, "fields")? else {
        return Err(err("MPGP fields must be list"));
    };
    let count = i16::try_from(fields.len()).map_err(|_| err("MPGP field count exceeds i16"))?;
    w.s16(count)?;
    if count > 0 {
        let leading = count
            .checked_mul(2)
            .ok_or_else(|| err("MPGP leading discard count overflow"))?;
        w.s16(leading)?;
        for f in fields {
            let Value::List(pair) = f else {
                return Err(err("MPGP field must be pair"));
            };
            if pair.len() != 2 {
                return Err(err("MPGP field must have two values"));
            }
            let typ =
                u8::try_from(s32(&pair[0])?).map_err(|_| err("MPGP picture type outside u8"))?;
            let length = match &pair[1] {
                Value::Int(x) => u32::try_from(*x),
                Value::UInt(x) => u32::try_from(*x),
                _ => return Err(err("MPGP picture length is not integer")),
            }
            .map_err(|_| err("MPGP picture length outside u32"))?;
            w.u8(typ)?;
            w.u32(length)?;
        }
    }
    w.u8(3)
}

fn bin_ref(n: &Node, w: &mut Writer) -> Result<()> {
    fields_only(n, &["uid_high", "uid_low", "name", "name_utf8"])?;
    w.bytes(&[2, 1])?;
    w.s32(s32(field(n, "uid_high")?)?)?;
    w.s32(s32(field(n, "uid_low")?)?)?;
    w.string_macroman(string(field(n, "name")?)?)?;
    // Pinned pyavb's write() always emits this extension, even though read()
    // accepts it as optional. Reject an absent field instead of inventing it.
    w.bytes(&[1, 1, 76])?;
    w.string_utf8(string(field(n, "name_utf8")?)?)?;
    w.u8(3)
}

fn mob_ref_base(n: &Node, w: &mut Writer) -> Result<()> {
    let id = mob_id(n)?;
    w.bytes(&[2, 1])?;
    w.u32(id.material.data1)?;
    w.u32(id.material.data2 as u32 + ((id.material.data3 as u32) << 16))?;
    w.s32(s32(field(n, "position")?)?)?;
    w.bytes(&[1, 1])?;
    w.mob_id(id)
}

fn mob_ref(n: &Node, w: &mut Writer, max_id: Option<u32>) -> Result<()> {
    let allowed: &[&str] = if n.class == *b"MCMR" {
        &["position", "mob_id"]
    } else {
        &[
            "position",
            "mob_id",
            "comp_offset",
            "attributes",
            "color",
            "handled_codes",
        ]
    };
    fields_only(n, allowed)?;
    mob_ref_base(n, w)?;
    if n.class == *b"MCMR" {
        return w.u8(3);
    }
    w.bytes(&[2, 3])?;
    w.s32(s32(field(n, "comp_offset")?)?)?;
    object_ref(w, field(n, "attributes")?, max_id)?;
    w.s16(1)?;
    let Value::List(color) = field(n, "color")? else {
        return Err(err("marker color is not a list"));
    };
    if color.len() != 3 {
        return Err(err("marker color requires three channels"));
    }
    for channel in color {
        w.u16(u16(channel)?)?;
    }
    if let Some(value) = n.get("handled_codes") {
        w.bytes(&[1, 1, 66])?;
        w.bool(boolean(value)?)?;
    }
    w.u8(3)
}

/// Serialize one payload from the marker/position subset of pyavb misc.py.
/// References keep their existing IDs; optionally check them against a bound.
pub fn serialize(
    n: &Node,
    big_endian: bool,
    byte_limit: usize,
    max_object_id: Option<u32>,
) -> Result<Vec<u8>> {
    let mut w = Writer::with_limit(big_endian, byte_limit);
    match &n.class {
        b"APOS" | b"ABOB" | b"DIDP" | b"MPGP" => position(n, &mut w)?,
        b"MCBR" => bin_ref(n, &mut w)?,
        b"MCMR" | b"TMBC" => mob_ref(n, &mut w, max_object_id)?,
        _ => return Err(err(format!("{}: misc writer unavailable", n.class_str()))),
    }
    Ok(w.into_bytes())
}
