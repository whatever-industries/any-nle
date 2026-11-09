//! Bounded payload serialization for selected trackgroup/effect classes.
//! Source-derived from pinned pyavb `components.py` and `trackgroups.py` (MIT; NOTICE).
//! Existing numeric reference IDs are retained; this is not a whole-file writer.

use crate::io::{Error, Reader, Result};
use crate::objects::{Node, Value};
use crate::writer_io::Writer;
use std::collections::HashSet;

const COMPONENT_FIELDS: &[&str] = &[
    "left_bob",
    "right_bob",
    "media_kind_id",
    "edit_rate",
    "name",
    "effect_id",
    "attributes",
    "session_attrs",
    "precomputed",
    "param_list",
];
const GROUP_FIELDS: &[&str] = &["mc_mode", "length", "num_scalars", "tracks"];
const EFFECT_FIELDS: &[&str] = &[
    "left_length",
    "right_length",
    "info_version",
    "info_current",
    "info_smooth",
    "info_color_item",
    "info_quality",
    "info_is_reversed",
    "info_aspect_on",
    "keyframes",
    "info_force_software",
    "info_never_hardware",
    "trackman",
];
const TRACK_FIELDS: &[&str] = &[
    "index",
    "attributes",
    "session_attr",
    "component",
    "filler_proxy",
    "bob_data",
    "control_code",
    "control_sub_code",
    "start_pos",
    "read_only",
    "lock_number",
];

fn error<T>(w: &Writer, what: impl Into<String>) -> Result<T> {
    Err(Error::new(what, w.len()))
}
fn field<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a Value> {
    n.get(name)
        .ok_or_else(|| Error::new(format!("{} lacks {name}", n.class_str()), w.len()))
}
fn int(n: &Node, name: &str, w: &Writer) -> Result<i64> {
    match field(n, name, w)? {
        Value::Int(v) => Ok(*v),
        _ => error(w, format!("{name} is not Int")),
    }
}
fn uint(n: &Node, name: &str, w: &Writer) -> Result<u32> {
    let value = match field(n, name, w)? {
        Value::UInt(v) => *v,
        Value::Int(v) if *v >= 0 => *v as u64,
        _ => return error(w, format!("{name} is not unsigned")),
    };
    u32::try_from(value).map_err(|_| Error::new(format!("{name} exceeds u32"), w.len()))
}
fn i16v(n: &Node, name: &str, w: &Writer) -> Result<i16> {
    i16::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} exceeds i16"), w.len()))
}
fn i32v(n: &Node, name: &str, w: &Writer) -> Result<i32> {
    i32::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} exceeds i32"), w.len()))
}
fn boolv(n: &Node, name: &str, w: &Writer) -> Result<bool> {
    match field(n, name, w)? {
        Value::Bool(v) => Ok(*v),
        _ => error(w, format!("{name} is not Bool")),
    }
}
fn stringv<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a str> {
    match field(n, name, w)? {
        Value::Str(v) => Ok(v),
        _ => error(w, format!("{name} is not Str")),
    }
}
fn bytesv<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a [u8]> {
    match field(n, name, w)? {
        Value::Bytes(v) => Ok(v),
        _ => error(w, format!("{name} is not Bytes")),
    }
}
fn listv<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a [Value]> {
    match field(n, name, w)? {
        Value::List(v) => Ok(v),
        _ => error(w, format!("{name} is not List")),
    }
}
fn refv(v: &Value, w: &Writer) -> Result<Option<u32>> {
    match v {
        Value::Null => Ok(None),
        Value::Ref(id) if *id != 0 => Ok(Some(*id)),
        _ => error(w, "reference is not Null/nonzero Ref"),
    }
}
fn ref_field(n: &Node, name: &str, w: &Writer) -> Result<Option<u32>> {
    refv(field(n, name, w)?, w)
}
fn check_fields(n: &Node, allowed: &[&str]) -> Result<()> {
    let mut seen = HashSet::new();
    for (name, _) in &n.fields {
        if !seen.insert(*name) || !allowed.contains(name) {
            return Err(Error::new(
                format!("{} has duplicate/unmapped field {name}", n.class_str()),
                0,
            ));
        }
    }
    Ok(())
}
fn write_optional_string(n: &Node, name: &str, zero_empty: bool, w: &mut Writer) -> Result<()> {
    match field(n, name, w)? {
        Value::Null => w.u16(if zero_empty { 0 } else { 0xffff }),
        Value::Str(s) if s.is_empty() => w.u16(if zero_empty { 0 } else { 0xffff }),
        Value::Str(s) => w.string_macroman(s),
        _ => error(w, format!("{name} is not string/Null")),
    }
}
#[derive(Clone, Copy, Default)]
struct ComponentSource {
    tagged_null: bool,
    zero_name: bool,
    zero_effect: bool,
    edit_rate: Option<SourceEditRate>,
}

#[derive(Clone, Copy)]
struct SourceEditRate {
    value: f64,
    bytes: [u8; 6],
    mantissa: i32,
    exponent: i16,
    big_endian: bool,
}

/// The Node collapses alternative empty strings and absent/tagged-null
/// param_list. Recover only those source-only distinctions from the bounded
/// Component prefix.
fn component_source(source: &[u8], big_endian: bool) -> Result<ComponentSource> {
    let mut r = Reader::new(source, 0, big_endian);
    r.assert_tag(2)?;
    r.assert_tag(3)?;
    r.object_ref()?;
    r.object_ref()?;
    r.s16()?;
    let at_rate = r.offset();
    let rate = r.exp10_float()?;
    let rate_bytes: [u8; 6] = source[at_rate..r.offset()]
        .try_into()
        .map_err(|_| Error::new("source edit rate is not six bytes", at_rate))?;
    let mut rate_reader = Reader::new(&rate_bytes, at_rate, big_endian);
    let edit_rate = Some(SourceEditRate {
        value: rate,
        bytes: rate_bytes,
        mantissa: rate_reader.s32()?,
        exponent: rate_reader.s16()?,
        big_endian,
    });
    let at_name = r.offset();
    r.string()?;
    let zero_name = source[at_name..at_name + 2] == [0, 0];
    let at_effect = r.offset();
    r.string()?;
    let zero_effect = source[at_effect..at_effect + 2] == [0, 0];
    r.object_ref()?;
    r.object_ref()?;
    r.object_ref()?;
    match r.next_ext()? {
        None => Ok(ComponentSource {
            tagged_null: false,
            zero_name,
            zero_effect,
            edit_rate,
        }),
        Some(1) => {
            r.assert_tag(72)?;
            r.object_ref()?;
            if r.next_ext()?.is_some() {
                return r.err("extra Component extension in source");
            }
            Ok(ComponentSource {
                tagged_null: true,
                zero_name,
                zero_effect,
                edit_rate,
            })
        }
        Some(tag) => r.err(format!("unknown Component extension {tag}")),
    }
}
fn find_composition_prefix(source: &[u8], big_endian: bool) -> Result<usize> {
    let mut decoder = crate::objects::Decoder {
        r: Reader::new(source, 0, big_endian),
        num_objects: u32::MAX,
    };
    let original = crate::objects::decode(*b"CMPO", &mut decoder)?
        .ok_or_else(|| Error::new("CMPO source did not decode", 0))?;
    if decoder.r.remaining() != 0 {
        return Err(Error::new("trailing CMPO source bytes", decoder.r.offset()));
    }
    let mut writer = Writer::with_limit(big_endian, source.len());
    group(
        &original,
        &mut writer,
        component_source(source, big_endian)?,
        Some(source),
    )?;
    if !source.starts_with(writer.as_bytes()) {
        return Err(Error::new(
            "original CMPO group prefix cannot be reproduced exactly",
            0,
        ));
    }
    Ok(writer.len() + 2) // CMPO's 02 02 header precedes legacy words
}
fn component(n: &Node, w: &mut Writer, source: ComponentSource) -> Result<()> {
    w.bytes(&[2, 3])?;
    for name in ["left_bob", "right_bob"] {
        let v = ref_field(n, name, w)?;
        w.object_ref_index(v)?;
    }
    let kind = i16v(n, "media_kind_id", w)?;
    w.s16(kind)?;
    let rate = match field(n, "edit_rate", w)? {
        Value::Float(v) => *v,
        _ => return error(w, "edit_rate is not Float"),
    };
    if let Some(original) = source.edit_rate {
        if rate.is_finite()
            && original.value.is_finite()
            && rate.to_bits() == original.value.to_bits()
        {
            if original.big_endian == w.big_endian {
                w.bytes(&original.bytes)?;
            } else {
                // Transcode the same decimal pair, not the source's raw bytes.
                // Re-deriving a pair from the f64 can lose a final decimal bit.
                w.s32(original.mantissa)?;
                w.s16(original.exponent)?;
            }
        } else {
            w.exp10_float(rate)?;
        }
    } else {
        w.exp10_float(rate)?;
    }
    write_optional_string(n, "name", source.zero_name, w)?;
    write_optional_string(n, "effect_id", source.zero_effect, w)?;
    for name in ["attributes", "session_attrs", "precomputed"] {
        let v = ref_field(n, name, w)?;
        w.object_ref_index(v)?;
    }
    // Rust's reader uses Null for both absent and present-null param_list. Do not
    // fabricate a tagged null reference. A nonnull Ref proves extension presence.
    match field(n, "param_list", w)? {
        Value::Ref(id) if *id != 0 => {
            w.bytes(&[1, 1, 72])?;
            w.object_ref_index(Some(*id))?;
        }
        Value::Null => match source.tagged_null {
            true => {
                w.bytes(&[1, 1, 72])?;
                w.object_ref_index(None)?;
            }
            false => {}
        },
        _ => return error(w, "param_list must be Null/nonzero Ref"),
    }
    Ok(())
}

const LABEL: u16 = 1 << 0;
const ATTR: u16 = 1 << 1;
const COMP: u16 = 1 << 2;
const FILL: u16 = 1 << 3;
const BOB: u16 = 1 << 4;
const CTRL: u16 = 1 << 5;
const SUB: u16 = 1 << 6;
const START: u16 = 1 << 7;
const READ_ONLY: u16 = 1 << 8;
const SESSION: u16 = 1 << 9;
fn track_flags(t: &Node) -> u16 {
    let mut flags = 0;
    for (name, flag) in [
        ("index", LABEL),
        ("attributes", ATTR),
        ("session_attr", SESSION),
        ("component", COMP),
        ("filler_proxy", FILL),
        ("bob_data", BOB),
        ("control_code", CTRL),
        ("control_sub_code", SUB),
        ("start_pos", START),
        ("read_only", READ_ONLY),
    ] {
        if t.get(name).is_some() {
            flags |= flag;
        }
    }
    flags
}
fn group(
    n: &Node,
    w: &mut Writer,
    component_source: ComponentSource,
    source_payload: Option<&[u8]>,
) -> Result<usize> {
    component(n, w, component_source)?;
    w.bytes(&[2, 8])?;
    let mode = u8::try_from(int(n, "mc_mode", w)?)
        .map_err(|_| Error::new("mc_mode exceeds u8", w.len()))?;
    w.u8(mode)?;
    let length = i32v(n, "length", w)?;
    w.s32(length)?;
    let scalars = i32v(n, "num_scalars", w)?;
    w.s32(scalars)?;
    let tracks = match field(n, "tracks", w)? {
        Value::List(v) => v,
        _ => return error(w, "tracks is not List"),
    };
    let count = i32::try_from(tracks.len()).map_err(|_| Error::new("too many tracks", w.len()))?;
    w.s32(count)?;
    let mut lock_numbers = Vec::with_capacity(tracks.len());
    for value in tracks {
        let Value::Node(t) = value else {
            return error(w, "track is not Node");
        };
        if t.type_name != "Track" {
            return error(w, "track Node type is not Track");
        }
        check_fields(t, TRACK_FIELDS)?;
        let flags = track_flags(t);
        w.u16(flags)?;
        if flags & LABEL != 0 {
            let v = i16v(t, "index", w)?;
            w.s16(v)?;
        }
        for (flag, name) in [
            (ATTR, "attributes"),
            (SESSION, "session_attr"),
            (COMP, "component"),
            (FILL, "filler_proxy"),
            (BOB, "bob_data"),
        ] {
            if flags & flag != 0 {
                let v = ref_field(t, name, w)?;
                w.object_ref_index(v)?;
            }
        }
        for (flag, name) in [(CTRL, "control_code"), (SUB, "control_sub_code")] {
            if flags & flag != 0 {
                let v = i16v(t, name, w)?;
                w.s16(v)?;
            }
        }
        if flags & START != 0 {
            let v = i32v(t, "start_pos", w)?;
            w.s32(v)?;
        }
        if flags & READ_ONLY != 0 {
            let v = boolv(t, "read_only", w)?;
            w.bool(v)?;
        }
        lock_numbers.push(t.get("lock_number"));
    }
    if lock_numbers.iter().any(Option::is_some) {
        if lock_numbers.iter().any(Option::is_none) {
            return error(w, "partial lock_number extension");
        }
        w.bytes(&[1, 1])?;
        for value in tracks {
            let Value::Node(t) = value else {
                unreachable!()
            };
            w.u8(69)?;
            let lock = i16v(t, "lock_number", w)?;
            w.s16(lock)?;
        }
    } else if tracks.is_empty()
        && source_payload.is_some_and(|source| {
            source.starts_with(w.as_bytes()) && source.get(w.len()..w.len() + 2) == Some(&[1, 1])
        })
    {
        // Avid can retain an empty lock-number extension. No decoded Track
        // carries its presence bit, so keep it only when source framing proves it.
        w.bytes(&[1, 1])?;
    }
    Ok(tracks.len())
}

fn effect_info(n: &Node, w: &mut Writer) -> Result<()> {
    for name in ["left_length", "right_length"] {
        let v = i32v(n, name, w)?;
        w.s32(v)?;
    }
    let version = i16v(n, "info_version", w)?;
    w.s16(version)?;
    for name in ["info_current", "info_smooth"] {
        let v = i32v(n, name, w)?;
        w.s32(v)?;
    }
    for name in ["info_color_item", "info_quality"] {
        let v = i16v(n, name, w)?;
        w.s16(v)?;
    }
    let reverse = i8::try_from(int(n, "info_is_reversed", w)?)
        .map_err(|_| Error::new("info_is_reversed exceeds i8", w.len()))?;
    w.s8(reverse)?;
    let aspect = boolv(n, "info_aspect_on", w)?;
    w.bool(aspect)?;
    let kf = ref_field(n, "keyframes", w)?;
    w.object_ref_index(kf)?;
    for name in ["info_force_software", "info_never_hardware"] {
        let v = boolv(n, name, w)?;
        w.bool(v)?;
    }
    Ok(())
}
fn track_effect_prefix(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 6])?;
    effect_info(n, w)?;
    extension_trackman(n, w, 2)
}
fn time_warp_prefix(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 2])?;
    w.s32(i32v(n, "phase_offset", w)?)
}
fn write_aspi(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])?;
    let plugins = listv(n, "plugins", w)?;
    w.s32(i32::try_from(plugins.len()).map_err(|_| Error::new("too many ASPI plugins", w.len()))?)?;
    for item in plugins {
        let Value::Node(plugin) = item else {
            return error(w, "ASPI plugin is not Node");
        };
        if plugin.type_name != "ASPIPlugin" {
            return error(w, "wrong ASPI plugin node");
        }
        check_fields(
            plugin,
            &[
                "name",
                "manufacturer_id",
                "product_id",
                "plugin_id",
                "chunks",
            ],
        )?;
        w.string_macroman(stringv(plugin, "name", w)?)?;
        for name in ["manufacturer_id", "product_id", "plugin_id"] {
            w.u32(uint(plugin, name, w)?)?;
        }
        let chunks = listv(plugin, "chunks", w)?;
        w.s32(
            i32::try_from(chunks.len()).map_err(|_| Error::new("too many ASPI chunks", w.len()))?,
        )?;
        for item in chunks {
            let Value::Node(chunk) = item else {
                return error(w, "ASPI chunk is not Node");
            };
            if chunk.type_name != "ASPIPluginChunk" {
                return error(w, "wrong ASPI chunk node");
            }
            check_fields(
                chunk,
                &[
                    "version",
                    "manufacturer_id",
                    "product_id",
                    "plugin_id",
                    "chunk_id",
                    "name",
                    "data",
                ],
            )?;
            let data = bytesv(chunk, "data", w)?;
            w.s32(
                i32::try_from(data.len())
                    .map_err(|_| Error::new("ASPI data exceeds i32", w.len()))?,
            )?;
            w.s32(i32v(chunk, "version", w)?)?;
            for name in ["manufacturer_id", "product_id", "plugin_id", "chunk_id"] {
                w.u32(uint(chunk, name, w)?)?;
            }
            w.string_macroman(stringv(chunk, "name", w)?)?;
            w.bytes(data)?;
        }
    }
    // Preserve the low/high legacy words exactly when present; pyavb derives
    // them from MobID on write, but a valid source need not agree with that.
    if n.get("mob_hi").is_some() || n.get("mob_lo").is_some() {
        if n.get("mob_hi").is_none() || n.get("mob_lo").is_none() {
            return error(w, "partial ASPI legacy MobID pair");
        }
        w.bytes(&[1, 1, 71])?;
        w.s32(i32v(n, "mob_hi", w)?)?;
        w.u8(71)?;
        w.s32(i32v(n, "mob_lo", w)?)?;
    }
    if let Some(value) = n.get("mob_id") {
        let Value::MobId(id) = value else {
            return error(w, "ASPI mob_id is not MobId");
        };
        w.bytes(&[1, 8])?;
        w.mob_id(id)?;
    }
    for (name, tag, scalar_tag) in [
        ("mark_in", 2, 77),
        ("mark_out", 3, 77),
        ("tracks_to_affect", 4, 72),
        ("rendering_mode", 5, 71),
        ("padding_secs", 6, 71),
    ] {
        if let Some(value) = n.get(name) {
            w.bytes(&[1, tag, scalar_tag])?;
            match name {
                "mark_in" | "mark_out" => {
                    let Value::Int(v) = value else {
                        return error(w, format!("{name} is not Int"));
                    };
                    w.s64(*v)?;
                }
                "tracks_to_affect" => w.u32(uint(n, name, w)?)?,
                _ => w.s32(i32v(n, name, w)?)?,
            }
        }
    }
    if n.get("preset_path").is_some() {
        let data = bytesv(n, "preset_path", w)?;
        let len = u32::try_from(data.len())
            .map_err(|_| Error::new("preset path exceeds u32", w.len()))?;
        w.bytes(&[1, 9, 72])?;
        w.u32(len)?;
        if len != 0 {
            w.u8(65)?;
            w.u32(len)?;
            w.bytes(data)?;
        }
    }
    w.u8(3)
}
fn write_eqmb(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 5])?;
    let bands = listv(n, "bands", w)?;
    w.s32(i32::try_from(bands.len()).map_err(|_| Error::new("too many EQ bands", w.len()))?)?;
    for item in bands {
        let Value::Node(band) = item else {
            return error(w, "EQ band is not Node");
        };
        if band.type_name != "EqualizerBand" {
            return error(w, "wrong EQ band node");
        }
        check_fields(band, &["type", "freq", "gain", "q", "enable"])?;
        for name in ["type", "freq", "gain", "q"] {
            w.s32(i32v(band, name, w)?)?;
        }
        w.bool(boolv(band, "enable", w)?)?;
    }
    w.bool(boolv(n, "effect_enable", w)?)?;
    w.string_macroman(stringv(n, "filter_name", w)?)?;
    w.u8(3)
}
fn composition_legacy_mob_words(n: &Node, w: &Writer) -> Result<[u32; 2]> {
    let Value::MobId(id) = field(n, "mob_id", w)? else {
        return error(w, "new CMPO requires a MobID");
    };
    Ok([
        id.material.data1,
        (id.material.data2 as u32) | ((id.material.data3 as u32) << 16),
    ])
}
fn extension_trackman(n: &Node, w: &mut Writer, tag: u8) -> Result<()> {
    if let Some(v) = n.get("trackman") {
        w.bytes(&[1, tag, 72])?;
        let id = refv(v, w)?;
        w.object_ref_index(id)?;
    }
    Ok(())
}

/// Serialize one supported trackgroup/effect object payload. Values omitted
/// from `Node` (such as a present-but-null Component param-list tag) cannot be
/// reconstructed. Original chunk bytes should be preserved when exact identity
/// matters; this function never silently emits fields it does not recognize.
pub fn encode_trackgroup_payload(
    n: &Node,
    source: Option<(&[u8], bool)>,
    big_endian: bool,
    limit: usize,
) -> Result<Vec<u8>> {
    let additional: &[&str] = match &n.class {
        b"RSET" => &["rep_set_type"],
        b"SLCT" => &["is_ganged", "selected"],
        b"TKFX" => EFFECT_FIELDS,
        b"PVOL" => &[
            "left_length",
            "right_length",
            "info_version",
            "info_current",
            "info_smooth",
            "info_color_item",
            "info_quality",
            "info_is_reversed",
            "info_aspect_on",
            "keyframes",
            "info_force_software",
            "info_never_hardware",
            "trackman",
            "level",
            "pan",
            "suppress_validation",
            "level_set",
            "pan_set",
            "supports_seperate_gain",
            "is_trim_gain_effect",
        ],
        b"TNFX" => &[
            "cutpoint",
            "left_length",
            "right_length",
            "info_version",
            "info_current",
            "info_smooth",
            "info_color_item",
            "info_quality",
            "info_is_reversed",
            "info_aspect_on",
            "keyframes",
            "info_force_software",
            "info_never_hardware",
            "trackman",
        ],
        b"TRKG" => &[],
        b"WARP" => &["phase_offset"],
        b"REPT" => &["phase_offset"],
        b"STRB" => &["phase_offset", "strobe_value"],
        b"MASK" => &["phase_offset", "is_double", "mask_bits"],
        b"SPED" => &[
            "phase_offset",
            "speed_ratio",
            "offset_adjust",
            "source_param_list",
            "new_source_calculation",
        ],
        b"CMPO" => &[
            "last_modified",
            "mob_type_id",
            "usage_code",
            "descriptor",
            "creation_time",
            "mob_id",
        ],
        b"ASPI" => &[
            "left_length",
            "right_length",
            "info_version",
            "info_current",
            "info_smooth",
            "info_color_item",
            "info_quality",
            "info_is_reversed",
            "info_aspect_on",
            "keyframes",
            "info_force_software",
            "info_never_hardware",
            "trackman",
            "plugins",
            "mob_hi",
            "mob_lo",
            "mob_id",
            "mark_in",
            "mark_out",
            "tracks_to_affect",
            "rendering_mode",
            "padding_secs",
            "preset_path",
        ],
        b"EQMB" => &[
            "left_length",
            "right_length",
            "info_version",
            "info_current",
            "info_smooth",
            "info_color_item",
            "info_quality",
            "info_is_reversed",
            "info_aspect_on",
            "keyframes",
            "info_force_software",
            "info_never_hardware",
            "trackman",
            "bands",
            "effect_enable",
            "filter_name",
        ],
        _ => {
            return Err(Error::new(
                format!("{} has no trackgroup writer", n.class_str()),
                0,
            ))
        }
    };
    let allowed: Vec<&str> = COMPONENT_FIELDS
        .iter()
        .chain(GROUP_FIELDS)
        .chain(additional)
        .copied()
        .collect();
    check_fields(n, &allowed)?;
    let mut w = Writer::with_limit(big_endian, limit);
    let component_source = match source {
        Some((payload, order)) => component_source(payload, order)?,
        None => ComponentSource::default(), // new py-avb Component omits null tag
    };
    let tracks = group(
        n,
        &mut w,
        component_source,
        source.map(|(payload, _)| payload),
    )?;
    match &n.class {
        b"TRKG" => {
            // Pinned py-avb's bare TrackGroup.write stops at EOF, while its
            // reader requires another byte to leave iter_ext() and then
            // rejects that byte as unread. There is no payload its ordinary
            // read path accepts. Retain only an unchanged source chunk.
            let Some((original, source_order)) = source else {
                return error(
                    &w,
                    "bare TRKG cannot be newly constructed readably by pinned py-avb",
                );
            };
            if source_order != big_endian || original != w.as_bytes() {
                return error(
                    &w,
                    "bare TRKG source payload cannot be rewritten or byte-swapped safely",
                );
            }
        }
        b"RSET" => {
            w.bytes(&[2, 1])?;
            if n.get("rep_set_type").is_some() {
                w.bytes(&[1, 1, 71])?;
                let v = i32v(n, "rep_set_type", &w)?;
                w.s32(v)?;
            }
            w.u8(3)?;
        }
        b"SLCT" => {
            w.bytes(&[2, 1])?;
            let ganged = boolv(n, "is_ganged", &w)?;
            w.bool(ganged)?;
            let selected = u16::try_from(int(n, "selected", &w)?)
                .map_err(|_| Error::new("selected exceeds u16", w.len()))?;
            if selected as usize >= tracks {
                return error(&w, "selected track outside track list");
            }
            w.u16(selected)?;
            w.u8(3)?;
        }
        b"TKFX" | b"PVOL" | b"ASPI" | b"EQMB" => {
            track_effect_prefix(n, &mut w)?;
            if &n.class == b"TKFX" {
                w.u8(3)?;
            } else if &n.class == b"ASPI" {
                write_aspi(n, &mut w)?;
            } else if &n.class == b"EQMB" {
                write_eqmb(n, &mut w)?;
            } else {
                w.bytes(&[2, 5])?;
                for name in ["level", "pan"] {
                    let v = i32v(n, name, &w)?;
                    w.s32(v)?;
                }
                for name in ["suppress_validation", "level_set", "pan_set"] {
                    let v = boolv(n, name, &w)?;
                    w.bool(v)?;
                }
                for (tag, name) in [(1, "supports_seperate_gain"), (2, "is_trim_gain_effect")] {
                    if n.get(name).is_some() {
                        w.bytes(&[1, tag, 71])?;
                        let v = i32v(n, name, &w)?;
                        w.s32(v)?;
                    }
                }
                w.u8(3)?;
            }
        }
        b"TNFX" => {
            w.bytes(&[2, 1])?;
            let cut = i32v(n, "cutpoint", &w)?;
            w.s32(cut)?;
            w.bytes(&[2, 5])?;
            effect_info(n, &mut w)?;
            extension_trackman(n, &mut w, 1)?;
            w.u8(3)?;
        }
        b"WARP" | b"MASK" | b"STRB" | b"SPED" | b"REPT" => {
            time_warp_prefix(n, &mut w)?;
            match &n.class {
                b"WARP" => {}
                b"MASK" => {
                    w.bytes(&[2, 1])?;
                    w.bool(boolv(n, "is_double", &w)?)?;
                    w.u32(uint(n, "mask_bits", &w)?)?;
                    w.u8(3)?;
                }
                b"STRB" => {
                    w.bytes(&[2, 1])?;
                    w.s32(i32v(n, "strobe_value", &w)?)?;
                    w.u8(3)?;
                }
                b"REPT" => {
                    w.bytes(&[2, 1, 3])?;
                }
                b"SPED" => {
                    w.bytes(&[2, 3])?;
                    let ratio = listv(n, "speed_ratio", &w)?;
                    if ratio.len() != 2 {
                        return error(&w, "speed_ratio must have two elements");
                    }
                    for value in ratio {
                        let Value::Int(v) = value else {
                            return error(&w, "speed ratio element not Int");
                        };
                        w.s32(
                            i32::try_from(*v)
                                .map_err(|_| Error::new("speed ratio exceeds i32", w.len()))?,
                        )?;
                    }
                    if let Some(Value::Float(v)) = n.get("offset_adjust") {
                        if !v.is_finite() {
                            return error(&w, "non-finite offset_adjust");
                        }
                        w.bytes(&[1, 1, 75])?;
                        w.f64(*v)?;
                    } else if n.get("offset_adjust").is_some() {
                        return error(&w, "offset_adjust not Float");
                    }
                    if let Some(v) = n.get("source_param_list") {
                        w.bytes(&[1, 2, 72])?;
                        w.object_ref_index(refv(v, &w)?)?;
                    }
                    if n.get("new_source_calculation").is_some() {
                        w.bytes(&[1, 3, 66])?;
                        w.bool(boolv(n, "new_source_calculation", &w)?)?;
                    }
                    w.u8(3)?;
                }
                _ => unreachable!(),
            }
        }
        b"CMPO" => {
            w.bytes(&[2, 2])?;
            // The typed reader discards these legacy words, so an edited CMPO
            // needs original bytes unless its MobID is present and agrees.
            let legacy = if let Some((original, order)) = source {
                let mut r = Reader::new(original, 0, order);
                // Re-derive the offset through a validated decoded source below.
                let mut d = crate::objects::Decoder {
                    r: Reader::new(original, 0, order),
                    num_objects: u32::MAX,
                };
                let previous = crate::objects::decode(*b"CMPO", &mut d)?
                    .ok_or_else(|| Error::new("invalid original CMPO payload", 0))?;
                if d.r.remaining() != 0 {
                    return error(&w, "invalid original CMPO payload");
                }
                let offset = find_composition_prefix(original, order)?;
                r.bytes(offset)?;
                let old_words = [r.u32()?, r.u32()?];
                if n.get("mob_id") != previous.get("mob_id") {
                    composition_legacy_mob_words(n, &w)?
                } else {
                    old_words
                }
            } else {
                composition_legacy_mob_words(n, &w)?
            };
            w.u32(legacy[0])?;
            w.u32(legacy[1])?;
            let modified = match field(n, "last_modified", &w)? {
                Value::Time(v) => *v,
                _ => return error(&w, "last_modified not Time"),
            };
            w.u32(modified)?;
            w.u8(u8::try_from(int(n, "mob_type_id", &w)?)
                .map_err(|_| Error::new("mob_type_id exceeds u8", w.len()))?)?;
            w.s32(i32v(n, "usage_code", &w)?)?;
            w.object_ref_index(ref_field(n, "descriptor", &w)?)?;
            if let Some(value) = n.get("creation_time") {
                let Value::Time(t) = value else {
                    return error(&w, "creation_time not Time");
                };
                w.bytes(&[1, 1, 71])?;
                w.u32(*t)?;
            }
            if let Some(value) = n.get("mob_id") {
                let Value::MobId(id) = value else {
                    return error(&w, "mob_id not MobId");
                };
                w.bytes(&[1, 2])?;
                w.mob_id(id)?;
            }
            w.u8(3)?;
        }
        _ => unreachable!(),
    }
    Ok(w.into_bytes())
}
