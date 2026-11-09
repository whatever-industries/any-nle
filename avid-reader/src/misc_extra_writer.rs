//! Bounded payload writers for pyavb `misc.py` and `attributes.py` classes not
//! covered by `misc_writer`/`bin_writer`. Source: pinned pyavb ff64e06 (MIT).
//! These serializers do not allocate IDs or establish Avid compatibility.

use crate::io::{Error, Result};
use crate::objects::{Node, Value};
use crate::writer_io::Writer;

fn fail(s: impl Into<String>) -> Error {
    Error::new(s, 0)
}
fn get<'a>(n: &'a Node, k: &str) -> Result<&'a Value> {
    n.get(k)
        .ok_or_else(|| fail(format!("{}: missing {k}", n.class_str())))
}
fn int(v: &Value) -> Result<i64> {
    if let Value::Int(x) = v {
        Ok(*x)
    } else {
        Err(fail("expected signed integer"))
    }
}
fn s32(v: &Value) -> Result<i32> {
    i32::try_from(int(v)?).map_err(|_| fail("s32 range"))
}
fn s16(v: &Value) -> Result<i16> {
    i16::try_from(int(v)?).map_err(|_| fail("s16 range"))
}
fn s8(v: &Value) -> Result<i8> {
    i8::try_from(int(v)?).map_err(|_| fail("s8 range"))
}
fn u32v(v: &Value) -> Result<u32> {
    match v {
        Value::UInt(x) => u32::try_from(*x).map_err(|_| fail("u32 range")),
        Value::Int(x) => u32::try_from(*x).map_err(|_| fail("u32 range")),
        _ => Err(fail("expected unsigned integer")),
    }
}
fn strv(v: &Value) -> Result<&str> {
    if let Value::Str(x) = v {
        Ok(x)
    } else {
        Err(fail("expected string"))
    }
}
fn bytes(v: &Value) -> Result<&[u8]> {
    if let Value::Bytes(x) = v {
        Ok(x)
    } else {
        Err(fail("expected bytes"))
    }
}
fn list(v: &Value) -> Result<&[Value]> {
    match v {
        Value::List(x) | Value::RefList(x) => Ok(x),
        _ => Err(fail("expected list")),
    }
}
fn boolean(v: &Value) -> Result<bool> {
    if let Value::Bool(x) = v {
        Ok(*x)
    } else {
        Err(fail("expected bool"))
    }
}
fn refv(w: &mut Writer, v: &Value, bound: Option<u32>) -> Result<()> {
    let id = match v {
        Value::Null => 0,
        Value::Ref(x) => *x,
        _ => return Err(fail("expected reference")),
    };
    if bound.is_some_and(|b| id > b) {
        return Err(fail("reference exceeds bound"));
    }
    w.u32(id)
}
fn only(n: &Node, allowed: &[&str]) -> Result<()> {
    for (k, _) in &n.fields {
        if !allowed.contains(k) {
            return Err(fail(format!("{}: unsupported field {k}", n.class_str())));
        }
    }
    Ok(())
}
fn len32(n: usize) -> Result<i32> {
    i32::try_from(n).map_err(|_| fail("count exceeds i32"))
}
fn len16(n: usize) -> Result<i16> {
    i16::try_from(n).map_err(|_| fail("count exceeds i16"))
}
fn prefix(w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])
}

fn locator(n: &Node, w: &mut Writer) -> Result<()> {
    only(n, &["path", "path_posix", "path_utf8", "path2_utf8"])?;
    w.bytes(&[2, 2])?;
    w.string_macroman(strv(get(n, "path")?)?)?;
    for (key, tag, utf8) in [
        ("path_posix", 1, false),
        ("path_utf8", 2, true),
        ("path2_utf8", 3, true),
    ] {
        if let Some(v) = n.get(key) {
            w.bytes(&[1, tag, 76])?;
            if utf8 {
                w.string_utf8(strv(v)?)?
            } else {
                w.string_macroman(strv(v)?)?
            }
        }
    }
    w.u8(3)
}
fn msm_locator(n: &Node, w: &mut Writer) -> Result<()> {
    only(
        n,
        &[
            "last_known_volume",
            "domain_type",
            "mob_id",
            "last_known_volume_utf8",
        ],
    )?;
    let Value::MobId(id) = get(n, "mob_id")? else {
        return Err(fail(
            "MSML writer requires MobID, including header-derived ID",
        ));
    };
    w.bytes(&[2, 2])?;
    w.u32(id.material.data1)?;
    w.u32(id.material.data2 as u32 + ((id.material.data3 as u32) << 16))?;
    w.string_macroman(strv(get(n, "last_known_volume")?)?)?;
    if let Some(v) = n.get("domain_type") {
        w.bytes(&[1, 1, 71])?;
        w.s32(s32(v)?)?
    }
    w.bytes(&[1, 2])?;
    w.mob_id(id)?;
    if let Some(v) = n.get("last_known_volume_utf8") {
        w.bytes(&[1, 3, 76])?;
        w.string_utf8(strv(v)?)?
    }
    w.u8(3)
}
fn blob(n: &Node, w: &mut Writer) -> Result<()> {
    let (field, short) = match &n.class {
        b"CCFX" => ("color_correction", true),
        b"GRFX" => ("pict_data", false),
        b"SHLP" => ("shape_data", false),
        _ => return Err(fail("blob class")),
    };
    only(n, &[field])?;
    let data = bytes(get(n, field)?)?;
    prefix(w)?;
    if short {
        w.s16(len16(data.len())?)?
    } else {
        w.s32(len32(data.len())?)?
    };
    w.bytes(data)?;
    w.u8(3)
}
fn refs(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    only(n, &["items"])?;
    let items = list(get(n, "items")?)?;
    prefix(w)?;
    if n.class == *b"TMCS" {
        w.s16(len16(items.len())?)?
    } else {
        w.s32(len32(items.len())?)?
    };
    for x in items {
        refv(w, x, bound)?
    }
    w.u8(3)
}
fn tracker(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    match &n.class {
        b"TKMN" => {
            only(n, &["data_slots", "param_slots"])?;
            prefix(w)?;
            refv(w, get(n, "data_slots")?, bound)?;
            refv(w, get(n, "param_slots")?, bound)?
        }
        b"TKDS" => {
            only(n, &["tracker_data", "track_fg"])?;
            prefix(w)?;
            let a = list(get(n, "tracker_data")?)?;
            w.s32(len32(a.len())?)?;
            for x in a {
                refv(w, x, bound)?
            }
            if let Some(v) = n.get("track_fg") {
                w.bytes(&[1, 1, 66])?;
                w.bool(boolean(v)?)?
            }
        }
        b"TKPS" => {
            only(n, &["settings", "params"])?;
            prefix(w)?;
            let data = bytes(get(n, "settings")?)?;
            w.s16(len16(data.len())?)?;
            w.bytes(data)?;
            let a = list(get(n, "params")?)?;
            w.s32(len32(a.len())?)?;
            for x in a {
                refv(w, x, bound)?
            }
        }
        b"TKPA" => {
            only(n, &["settings"])?;
            prefix(w)?;
            let data = bytes(get(n, "settings")?)?;
            w.s16(len16(data.len())?)?;
            w.bytes(data)?
        }
        b"TKDA" => {
            only(
                n,
                &[
                    "settings",
                    "clip_version",
                    "clips",
                    "offset_tracking",
                    "smoothing",
                    "jitter_removal",
                    "filter_amount",
                    "clip5",
                    "clip6",
                ],
            )?;
            prefix(w)?;
            let data = bytes(get(n, "settings")?)?;
            w.s16(len16(data.len())?)?;
            w.bytes(data)?;
            w.u32(u32v(get(n, "clip_version")?)?)?;
            let a = list(get(n, "clips")?)?;
            w.s16(len16(a.len())?)?;
            for x in a {
                refv(w, x, bound)?
            }
            for (key, tag) in [
                ("offset_tracking", 1),
                ("smoothing", 2),
                ("jitter_removal", 3),
            ] {
                if let Some(v) = n.get(key) {
                    w.bytes(&[1, tag, 72])?;
                    w.u32(u32v(v)?)?
                }
            }
            if let Some(v) = n.get("filter_amount") {
                let Value::Float(f) = v else {
                    return Err(fail("expected float"));
                };
                w.bytes(&[1, 4, 75])?;
                // Pinned pyavb emits LE even for BE files; emit file-order
                // bytes so this library's BE reader can read the result.
                w.f64(*f)?
            }
            for (key, tag) in [("clip5", 5), ("clip6", 6)] {
                if let Some(v) = n.get(key) {
                    w.bytes(&[1, tag, 72])?;
                    refv(w, v, bound)?
                }
            }
        }
        _ => return Err(fail("tracker class")),
    }
    w.u8(3)
}
fn user_param(n: &Node, w: &mut Writer) -> Result<()> {
    only(n, &["byte_order", "uuid", "data"])?;
    if s16(get(n, "byte_order")?)? != 0x4949 {
        return Err(fail("AVUP byte order must be 0x4949"));
    }
    let Value::Uuid(uuid) = get(n, "uuid")? else {
        return Err(fail("expected UUID"));
    };
    let data = bytes(get(n, "data")?)?;
    prefix(w)?;
    w.s16(0x4949)?;
    w.raw_uuid(uuid)?;
    w.s32(len32(
        data.len()
            .checked_add(4)
            .ok_or_else(|| fail("size overflow"))?,
    )?)?;
    w.s32(len32(data.len())?)?;
    w.bytes(data)?;
    w.u8(3)
}
fn parameter_item(n: &Node, w: &mut Writer, bound: Option<u32>) -> Result<()> {
    only(
        n,
        &[
            "uuid",
            "value_type",
            "value",
            "name",
            "enable",
            "control_track",
            "contribs_to_sig",
        ],
    )?;
    let Value::Uuid(uuid) = get(n, "uuid")? else {
        return Err(fail("expected UUID"));
    };
    let typ = s16(get(n, "value_type")?)?;
    w.bytes(&[2, 2])?;
    w.raw_uuid(uuid)?;
    w.s16(typ)?;
    match typ {
        1 => w.s32(s32(get(n, "value")?)?)?,
        2 => {
            let Value::Float(f) = get(n, "value")? else {
                return Err(fail("expected float"));
            };
            w.f64(*f)?
        }
        4 => refv(w, get(n, "value")?, bound)?,
        _ => return Err(fail("unknown PRIT value_type")),
    }
    let name = strv(get(n, "name")?)?;
    if name.is_empty() {
        w.u16(0xffff)?
    } else {
        w.string_macroman(name)?
    };
    w.bool(boolean(get(n, "enable")?)?)?;
    refv(w, get(n, "control_track")?, bound)?;
    if let Some(v) = n.get("contribs_to_sig") {
        w.bytes(&[1, 1, 66])?;
        w.bool(boolean(v)?)?
    }
    w.u8(3)
}
fn fxps(n: &Node, w: &mut Writer) -> Result<()> {
    only(
        n,
        &[
            "orig_length",
            "window_offset",
            "keyframe_size",
            "parameters",
        ],
    )?;
    let params = list(get(n, "parameters")?)?;
    w.bytes(&[2, 0x12])?;
    w.s32(s32(get(n, "orig_length")?)?)?;
    w.s32(s32(get(n, "window_offset")?)?)?;
    w.s32(len32(params.len())?)?;
    w.s32(s32(get(n, "keyframe_size")?)?)?;
    for p in params {
        let Value::Node(p) = p else {
            return Err(fail("FXPS parameter is not a node"));
        };
        if p.class != *b"FXPS" {
            return Err(fail("FXPS parameter has wrong node class"));
        }
        only(
            p,
            &[
                "percent_time",
                "level",
                "pos_x",
                "floor_x",
                "ceil_x",
                "pos_y",
                "floor_y",
                "ceil_y",
                "scale_x",
                "scale_y",
                "crop_left",
                "crop_right",
                "crop_top",
                "crop_bottom",
                "box",
                "box_xscale",
                "box_yscale",
                "box_xpos",
                "box_ypos",
                "border_width",
                "border_soft",
                "splill_gain2",
                "splill_gain",
                "splill_soft2",
                "splill_soft",
                "enable_key_flags",
                "colors",
                "user_param",
                "selected",
            ],
        )?;
        let scalar = [
            "percent_time",
            "level",
            "pos_x",
            "floor_x",
            "ceil_x",
            "pos_y",
            "floor_y",
            "ceil_y",
            "scale_x",
            "scale_y",
            "crop_left",
            "crop_right",
            "crop_top",
            "crop_bottom",
        ];
        for k in scalar {
            w.s32(s32(get(p, k)?)?)?
        }
        let boxv = list(get(p, "box")?)?;
        if boxv.len() != 4 {
            return Err(fail("FXPS box needs four values"));
        }
        for v in boxv {
            w.s32(s32(v)?)?
        }
        for k in ["box_xscale", "box_yscale", "box_xpos", "box_ypos"] {
            w.bool(boolean(get(p, k)?)?)?
        }
        for k in ["border_width", "border_soft"] {
            w.s32(s32(get(p, k)?)?)?
        }
        for k in ["splill_gain2", "splill_gain", "splill_soft2", "splill_soft"] {
            w.s16(s16(get(p, k)?)?)?
        }
        w.s8(s8(get(p, "enable_key_flags")?)?)?;
        let colors = list(get(p, "colors")?)?;
        w.s32(len32(colors.len())?)?;
        for v in colors {
            w.s32(s32(v)?)?
        }
        let user = bytes(get(p, "user_param")?)?;
        w.s32(len32(user.len())?)?;
        w.bytes(user)?;
        w.bool(boolean(get(p, "selected")?)?)?;
    }
    w.u8(3)
}

/// Encode a typed misc/attribute payload with existing reference IDs.
pub fn serialize(
    n: &Node,
    big_endian: bool,
    byte_limit: usize,
    max_object_id: Option<u32>,
) -> Result<Vec<u8>> {
    let mut w = Writer::with_limit(big_endian, byte_limit);
    match &n.class {
        b"FILE" | b"WINF" => locator(n, &mut w)?,
        b"MSML" => msm_locator(n, &mut w)?,
        b"URLL" => {
            only(n, &[])?;
            w.u8(3)?
        }
        b"CCFX" | b"GRFX" | b"SHLP" => blob(n, &mut w)?,
        b"TMCS" | b"PRLS" => refs(n, &mut w, max_object_id)?,
        b"TKMN" | b"TKDS" | b"TKPS" | b"TKDA" | b"TKPA" => tracker(n, &mut w, max_object_id)?,
        b"AVUP" => user_param(n, &mut w)?,
        b"PRIT" => parameter_item(n, &mut w, max_object_id)?,
        b"FXPS" => fxps(n, &mut w)?,
        _ => return Err(fail(format!("{}: writer unavailable", n.class_str()))),
    };
    Ok(w.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{AvbFile, AvbIndex};
    use crate::objects::{decode, Decoder};

    const CLASSES: &[[u8; 4]] = &[
        *b"FILE", *b"WINF", *b"MSML", *b"URLL", *b"CCFX", *b"GRFX", *b"SHLP", *b"TMCS", *b"PRLS",
        *b"TKMN", *b"TKDS", *b"TKPS", *b"TKDA", *b"TKPA", *b"AVUP", *b"PRIT", *b"FXPS",
    ];

    #[test]
    fn full_bin_payloads_match_pinned_source() {
        let Ok(root) = std::env::var("PYAVB_TEST_FILES") else {
            return;
        };
        let mut hits = std::collections::BTreeMap::<[u8; 4], usize>::new();
        let root = std::path::Path::new(&root);
        let mut paths = vec![root.join("test_file_01.avb")];
        for entry in std::fs::read_dir(root.join("retimes")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|x| x == "avb") {
                paths.push(path)
            }
        }
        assert_eq!(paths.len(), 14);
        for path in paths {
            let raw = std::fs::read(path).unwrap();
            let index = AvbIndex::from_bytes(raw.clone()).unwrap();
            let file = AvbFile::parse(&raw).unwrap();
            for id in 1..index.object_count() as u32 {
                let object = index.object(id).unwrap();
                if !CLASSES.contains(&object.class) {
                    continue;
                }
                let node = file.node(id).unwrap();
                let payload = index.payload(id).unwrap();
                let rebuilt = serialize(
                    node,
                    index.big_endian,
                    payload.len() + 4096,
                    Some((index.object_count() - 1) as u32),
                )
                .unwrap();
                assert_eq!(rebuilt, payload, "{} object {id}", node.class_str());
                *hits.entry(object.class).or_default() += 1;
            }
        }
        eprintln!("misc full-bin payload exact matches: {hits:?}");
        assert!(!hits.is_empty());
    }

    #[test]
    fn both_orders_and_malformed_bounds() {
        for big in [false, true] {
            let mut n = Node {
                class: *b"TKDS",
                type_name: "TrackerDataSlot",
                fields: vec![
                    (
                        "tracker_data",
                        Value::RefList(vec![Value::Ref(1), Value::Null]),
                    ),
                    ("track_fg", Value::Bool(true)),
                ],
            };
            let encoded = serialize(&n, big, 64, Some(1)).unwrap();
            let mut decoder = Decoder {
                r: crate::io::Reader::new(&encoded, 0, big),
                num_objects: 2,
            };
            assert_eq!(decode(n.class, &mut decoder).unwrap().unwrap(), n);
            assert_eq!(decoder.r.remaining(), 0);
            assert!(serialize(&n, big, encoded.len() - 1, Some(1)).is_err());
            n.fields[0].1 = Value::RefList(vec![Value::Ref(2)]);
            assert!(serialize(&n, big, 64, Some(1)).is_err());
        }
    }

    #[test]
    fn pinned_chunk_payloads_and_be_semantics() {
        let Ok(root) = std::env::var("PYAVB_CHUNKS") else {
            return;
        };
        let root = std::path::Path::new(&root);
        let cases = [
            ("PRIT", "prit_01.chunk"),
            ("PRIT", "prit_02.chunk"),
            ("PRIT", "prit_03.chunk"),
            ("CCFX", "ccfx_01.chunk"),
            ("SHLP", "shlp_01.chunk"),
            ("URLL", "urll_01.chunk"),
            ("FILE", "file_01.chunk"),
            ("WINF", "winf_01.chunk"),
            ("MSML", "msml_01.chunk"),
            ("TKPA", "tkpa_01.chunk"),
            ("TKPS", "tkps_01.chunk"),
            ("TKDS", "tkds_01.chunk"),
            ("TKDA", "tkda_01.chunk"),
            ("FXPS", "fxps_01.chunk"),
            ("FXPS", "fxps_02.chunk"),
        ];
        for (class, name) in cases {
            let chunk = std::fs::read(root.join(class).join(name)).unwrap();
            let mut id: [u8; 4] = chunk[..4].try_into().unwrap();
            id.reverse();
            assert_eq!(&id, class.as_bytes());
            let payload = &chunk[8..];
            let mut decoder = Decoder {
                r: crate::io::Reader::new(payload, 0, false),
                num_objects: u32::MAX,
            };
            let node = decode(id, &mut decoder).unwrap().unwrap();
            assert_eq!(decoder.r.remaining(), 0);
            let encoded = serialize(&node, false, payload.len() + 1024, None).unwrap();
            assert_eq!(encoded, payload, "{class}/{name}");
            let be = serialize(&node, true, payload.len() + 1024, None).unwrap();
            let mut decoder = Decoder {
                r: crate::io::Reader::new(&be, 0, true),
                num_objects: u32::MAX,
            };
            let restored = decode(id, &mut decoder).unwrap().unwrap();
            assert_eq!(restored, node, "{class}/{name} BE");
            assert_eq!(decoder.r.remaining(), 0);
        }
    }

    #[test]
    fn rejects_unrepresentable_source_only_and_wrong_types() {
        let msml = Node {
            class: *b"MSML",
            type_name: "MSMLLocator",
            fields: vec![("last_known_volume", Value::Str("disk".into()))],
        };
        assert!(serialize(&msml, false, 100, None).is_err());
        let avup = Node {
            class: *b"AVUP",
            type_name: "CFUserParam",
            fields: vec![("byte_order", Value::Int(0x4d4d))],
        };
        assert!(serialize(&avup, false, 100, None).is_err());
        let fxps = Node {
            class: *b"FXPS",
            type_name: "EffectParamList",
            fields: vec![
                ("orig_length", Value::Int(1)),
                ("window_offset", Value::Int(0)),
                ("keyframe_size", Value::Int(1)),
                ("parameters", Value::List(vec![Value::Ref(1)])),
            ],
        };
        assert!(serialize(&fxps, false, 200, None).is_err());
    }
}
