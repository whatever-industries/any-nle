//! Bounded PRCL, CTRL, ECCP payload writers from pinned pyavb `components.py`
//! (MIT, Mark Reid; see NOTICE). Class and container framing are separate.

use crate::component_writer::{
    source_only_from_payload as clip_source_only, write_component, SourceOnly,
};
use crate::io::{Error, Reader, Result};
use crate::objects::{decode, Decoder, Node, Value};
use crate::writer_io::Writer;

#[derive(Debug, Clone, Copy, Default)]
pub struct CurveSourceOnly {
    /// COMP's null param-list extension presence, as recovered from raw CLIP prefix.
    pub clip: SourceOnly,
    /// Original Component edit-rate encoding, including noncanonical mantissa/
    /// exponent choices that cannot be reconstructed from its decoded float.
    pub edit_rate: Option<([u8; 6], f64, bool)>,
    /// ECCP's ignored u32, not represented in Node.
    pub edge_unused: Option<u32>,
}

fn field<'a>(node: &'a Node, name: &str) -> Result<&'a Value> {
    node.get(name)
        .ok_or_else(|| Error::new(format!("{} lacks {name}", node.class_str()), 0))
}

fn int(node: &Node, name: &str, min: i64, max: i64) -> Result<i64> {
    let n = match field(node, name)? {
        Value::Int(v) => *v,
        Value::UInt(v) if *v <= i64::MAX as u64 => *v as i64,
        _ => return Err(Error::new(format!("{name} is not an integer"), 0)),
    };
    if n < min || n > max {
        return Err(Error::new(format!("{name} outside {min}..{max}"), 0));
    }
    Ok(n)
}

fn signed32(node: &Node, name: &str) -> Result<i32> {
    Ok(int(node, name, i32::MIN as i64, i32::MAX as i64)? as i32)
}

fn signed16(node: &Node, name: &str) -> Result<i16> {
    Ok(int(node, name, i16::MIN as i64, i16::MAX as i64)? as i16)
}

fn pair(value: &Value, name: &str) -> Result<[i32; 2]> {
    let Value::List(items) = value else {
        return Err(Error::new(format!("{name} is not a pair"), 0));
    };
    if items.len() != 2 {
        return Err(Error::new(format!("{name} length is not two"), 0));
    }
    let mut out = [0; 2];
    for (index, item) in items.iter().enumerate() {
        let Value::Int(v) = item else {
            return Err(Error::new(format!("{name} member is not int"), 0));
        };
        out[index] =
            i32::try_from(*v).map_err(|_| Error::new(format!("{name} member outside i32"), 0))?;
    }
    Ok(out)
}

fn points(node: &Node) -> Result<&[Value]> {
    let Value::List(items) = field(node, "control_points")? else {
        return Err(Error::new("control_points is not a list", 0));
    };
    Ok(items)
}

fn child<'a>(value: &'a Value, label: &str) -> Result<&'a Node> {
    match value {
        Value::Node(node) => Ok(node),
        _ => Err(Error::new(format!("{label} is not a node"), 0)),
    }
}

fn list<'a>(node: &'a Node, name: &str) -> Result<&'a [Value]> {
    let Value::List(items) = field(node, name)? else {
        return Err(Error::new(format!("{name} is not a list"), 0));
    };
    Ok(items)
}

fn write_pair(w: &mut Writer, values: [i32; 2]) -> Result<()> {
    w.s32(values[0])?;
    w.s32(values[1])
}

fn write_prcl(node: &Node, w: &mut Writer) -> Result<()> {
    w.u8(2)?;
    w.u8(1)?;
    w.s32(signed32(node, "interp_kind")?)?;
    let value_type = signed16(node, "value_type")?;
    if !matches!(value_type, 1 | 2 | 4) {
        return Err(Error::new("unsupported PRCL value_type", w.len()));
    }
    w.s16(value_type)?;
    let points = points(node)?;
    w.s32(i32::try_from(points.len()).map_err(|_| Error::new("too many PRCL points", w.len()))?)?;
    for item in points {
        let point = child(item, "control point")?;
        write_pair(w, pair(field(point, "offset")?, "offset")?)?;
        w.s32(signed32(point, "timescale")?)?;
        match (value_type, field(point, "value")?) {
            (1, _) => w.s32(signed32(point, "value")?)?,
            (2, Value::Float(v)) => w.f64(*v)?,
            (4, Value::Null) => w.object_ref_index(None)?,
            (4, Value::Ref(id)) if *id != 0 => w.object_ref_index(Some(*id))?,
            _ => {
                return Err(Error::new(
                    "PRCL point value disagrees with value_type",
                    w.len(),
                ))
            }
        }
        let props = list(point, "pp")?;
        w.s16(
            i16::try_from(props.len())
                .map_err(|_| Error::new("too many PRCL point properties", w.len()))?,
        )?;
        for item in props {
            let prop = child(item, "point property")?;
            w.s16(signed16(prop, "code")?)?;
            let kind = signed16(prop, "type")?;
            w.s16(kind)?;
            match (kind, field(prop, "value")?) {
                (1, _) => w.s32(signed32(prop, "value")?)?,
                (2, Value::Float(v)) => w.f64(*v)?,
                _ => {
                    return Err(Error::new(
                        "unsupported PRCL point property type/value",
                        w.len(),
                    ))
                }
            }
        }
    }
    if let Some(value) = node.get("extrap_kind") {
        if !matches!(value, Value::Int(_) | Value::UInt(_)) {
            return Err(Error::new("bad extrap_kind", w.len()));
        }
        w.u8(1)?;
        w.u8(1)?;
        w.u8(71)?;
        w.s32(signed32(node, "extrap_kind")?)?;
    }
    if let Some(value) = node.get("fields") {
        if !matches!(value, Value::Int(_) | Value::UInt(_)) {
            return Err(Error::new("bad fields", w.len()));
        }
        w.u8(1)?;
        w.u8(2)?;
        w.u8(71)?;
        w.s32(signed32(node, "fields")?)?;
    }
    w.u8(3)
}

fn write_ctrl(node: &Node, w: &mut Writer) -> Result<()> {
    w.u8(2)?;
    w.u8(3)?;
    w.s32(signed32(node, "interp_kind")?)?;
    let points = points(node)?;
    w.s32(i32::try_from(points.len()).map_err(|_| Error::new("too many CTRL points", w.len()))?)?;
    for item in points {
        let point = child(item, "control point")?;
        write_pair(w, pair(field(point, "offset")?, "offset")?)?;
        w.s32(signed32(point, "time_scale")?)?;
        w.bool(true)?; // Reader rejects the only other value.
        write_pair(w, pair(field(point, "value")?, "value")?)?;
        let props = list(point, "pp")?;
        w.s16(
            i16::try_from(props.len())
                .map_err(|_| Error::new("too many CTRL point properties", w.len()))?,
        )?;
        for item in props {
            let prop = child(item, "point property")?;
            w.s16(signed16(prop, "code")?)?;
            write_pair(w, pair(field(prop, "value")?, "property value")?)?;
        }
    }
    w.u8(3)
}

fn write_eccp(node: &Node, w: &mut Writer, source: CurveSourceOnly) -> Result<()> {
    w.u8(2)?;
    w.u8(1)?;
    let Value::Bytes(header) = field(node, "header")? else {
        return Err(Error::new("ECCP header is not bytes", w.len()));
    };
    if header.len() != 8 {
        return Err(Error::new("ECCP header length is not 8", w.len()));
    }
    w.bytes(header)?;
    w.u8(int(node, "film_kind", 0, u8::MAX as i64)? as u8)?;
    w.u8(int(node, "code_format", 0, u8::MAX as i64)? as u8)?;
    w.u16(int(node, "base_perf", 0, u16::MAX as i64)? as u16)?;
    // py-avb's Edgecode.write emits zero for a newly created object. Preserve
    // the original source-only word when editing an existing object instead.
    w.u32(source.edge_unused.unwrap_or(0))?;
    w.s32(signed32(node, "start_ec")?)?;
    w.u8(3)
}

/// Write a typed curve/edgecode payload. The source-only spans must be taken
/// from the original object to avoid inventing bytes dropped by `Node`.
pub fn write_curve(
    node: &Node,
    big_endian: bool,
    source: CurveSourceOnly,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    if !matches!(&node.class, b"PRCL" | b"CTRL" | b"ECCP") {
        return Err(Error::new("class outside curve writer", 0));
    }
    let mut prefix = node.clone();
    prefix.class = *b"CLIP";
    let preserve_rate = match (prefix.get("edit_rate"), source.edit_rate) {
        (Some(Value::Float(value)), Some((raw, original, source_big_endian)))
            if *value == original => {
            let mut reader = Reader::new(&raw, 0, source_big_endian);
            if reader.exp10_float()? != original || reader.remaining() != 0 {
                return Err(Error::new("curve source edit-rate bytes disagree with value", 0));
            }
            let preserved = if source_big_endian == big_endian {
                raw
            } else {
                let mut reader = Reader::new(&raw, 0, source_big_endian);
                let mantissa = reader.s32()?;
                let exponent = reader.s16()?;
                let mut transcoded = Writer::with_limit(big_endian, 6);
                transcoded.s32(mantissa)?;
                transcoded.s16(exponent)?;
                transcoded.as_bytes().try_into().unwrap()
            };
            // A source rate may exceed the fresh writer's mantissa range, or
            // underflow its six-digit encoder. Serialize a valid placeholder
            // before restoring its validated original wire representation.
            prefix.fields.iter_mut().find(|(name, _)| *name == "edit_rate").unwrap().1 =
                Value::Float(25.0);
            Some(preserved)
        }
        _ => None,
    };
    let mut output = write_component(&prefix, big_endian, source.clip, max_bytes)?;
    if let Some(raw) = preserve_rate {
        output[12..18].copy_from_slice(&raw);
    }
    let remaining = max_bytes.saturating_sub(output.len());
    let mut suffix = Writer::with_limit(big_endian, remaining);
    match &node.class {
        b"PRCL" => write_prcl(node, &mut suffix)?,
        b"CTRL" => write_ctrl(node, &mut suffix)?,
        b"ECCP" => write_eccp(node, &mut suffix, source)?,
        _ => unreachable!(),
    }
    output.extend_from_slice(suffix.as_bytes());
    Ok(output)
}

/// Recover COMP extension presence and ECCP's ignored u32. Validates the
/// entire original payload before extracting any source-only spans.
pub fn source_only_from_payload(
    class: [u8; 4],
    payload: &[u8],
    big_endian: bool,
) -> Result<CurveSourceOnly> {
    if !matches!(&class, b"PRCL" | b"CTRL" | b"ECCP") {
        return Err(Error::new("class outside curve writer", 0));
    }
    let mut checked = Decoder {
        r: Reader::new(payload, 0, big_endian),
        num_objects: u32::MAX,
    };
    decode(class, &mut checked)?;
    if checked.r.remaining() != 0 {
        return checked.r.err("trailing curve bytes");
    }

    let mut r = Reader::new(payload, 0, big_endian);
    r.assert_tag(2)?;
    r.assert_tag(3)?;
    r.u32()?;
    r.u32()?;
    r.s16()?;
    let rate_start = r.offset();
    let edit_rate_value = r.exp10_float()?;
    let mut edit_rate_bytes = [0u8; 6];
    edit_rate_bytes.copy_from_slice(&payload[rate_start..rate_start + 6]);
    for _ in 0..2 {
        let len = r.u16()?;
        if len != 0xffff {
            r.bytes(len as usize)?;
        }
    }
    r.u32()?;
    r.u32()?;
    r.u32()?;
    while let Some(tag) = r.next_ext()? {
        if tag != 1 {
            return r.err("unsupported COMP extension");
        }
        r.assert_tag(72)?;
        r.u32()?;
    }
    r.assert_tag(2)?;
    r.assert_tag(1)?;
    r.u32()?;
    let prefix_end = r.offset();
    let clip = clip_source_only(*b"CLIP", &payload[..prefix_end], big_endian)?;
    let edge_unused = if class == *b"ECCP" {
        r.assert_tag(2)?;
        r.assert_tag(1)?;
        r.bytes(8)?;
        r.u8()?;
        r.u8()?;
        r.u16()?;
        Some(r.u32()?)
    } else {
        None
    };
    Ok(CurveSourceOnly {
        clip,
        edit_rate: Some((edit_rate_bytes, edit_rate_value, big_endian)),
        edge_unused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::AvbIndex;
    use std::path::{Path, PathBuf};

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    // Fixed vectors generated by pinned pyavb ParamClip, ControlClip and
    // Edgecode `write()` with one control point/property and both byte orders.
    #[test]
    fn pyavb_generated_vectors_both_orders() {
        for (big, vectors) in [
            (false, [
                (*b"PRCL", "020300000000000000000100180000000000ffffffff000000000000000000000000010148000000000201640000000201030000000100010000000100000002000000180000003700000001000900010004000000010147020000000102470400000003"),
                (*b"CTRL", "020300000000000000000100180000000000ffffffff000000000000000000000000010148000000000201640000000203030000000100000001000000020000001800000001050000000600000001000700080000000900000003"),
                (*b"ECCP", "020300000000000000000100180000000000ffffffff000000000000000000000000010148000000000201640000000201010203040506070801020300000000002c00000003"),
            ]),
            (true, [
                (*b"PRCL", "020300000000000000000001000000180000ffffffff000000000000000000000000010148000000000201000000640201000000030001000000010000000100000002000000180000003700010009000100000004010147000000020102470000000403"),
                (*b"CTRL", "020300000000000000000001000000180000ffffffff000000000000000000000000010148000000000201000000640203000000030000000100000001000000020000001801000000050000000600010007000000080000000903"),
                (*b"ECCP", "020300000000000000000001000000180000ffffffff000000000000000000000000010148000000000201000000640201010203040506070801020003000000000000002c03"),
            ]),
        ] {
            for (class, vector) in vectors {
                let original = hex(vector);
                let mut d = Decoder { r: Reader::new(&original, 0, big), num_objects: 10 };
                let node = decode(class, &mut d).unwrap().unwrap();
                assert_eq!(d.r.remaining(), 0);
                let source = source_only_from_payload(class, &original, big).unwrap();
                let output = write_curve(&node, big, source, original.len()).unwrap();
                assert_eq!(output, original, "{} big={big}", String::from_utf8_lossy(&class));
            }
        }
    }

    #[test]
    fn pyavb_reference_point_vectors_both_orders() {
        for (big, vector) in [
            (false, "020300000000000000000100180000000000ffffffff0000000000000000000000000101480000000002016400000002010200000004000100000000000000010000001800000000000000000003"),
            (true, "020300000000000000000001000000180000ffffffff0000000000000000000000000101480000000002010000006402010000000200040000000100000000000000010000001800000000000003"),
        ] {
            let original = hex(vector);
            let mut d = Decoder { r: Reader::new(&original, 0, big), num_objects: 10 };
            let node = decode(*b"PRCL", &mut d).unwrap().unwrap();
            assert_eq!(int(&node, "value_type", 1, 4).unwrap(), 4);
            assert_eq!(d.r.remaining(), 0);
            let source = source_only_from_payload(*b"PRCL", &original, big).unwrap();
            assert_eq!(write_curve(&node, big, source, original.len()).unwrap(), original);
        }
    }

    #[test]
    fn unchanged_exp10_edit_rate_keeps_source_wire_bytes() {
        // Pinned py-avb PRCL framing plus pinned py-avb exp10 primitive
        // vectors: 1×10^-323 and 1×10^308. Dividing by 10^323 in the old
        // reader yielded zero; canonical re-encoding also lost these bytes.
        for (big, vector, subnormal, huge) in [
            (
                false,
                "020300000000000000000100180000000000ffffffff0000000000000000000000000101480000000002016400000002010200000004000100000000000000010000001800000000000000000003",
                "01000000bdfe",
                "010000003401",
            ),
            (
                true,
                "020300000000000000000001000000180000ffffffff0000000000000000000000000101480000000002010000006402010000000200040000000100000000000000010000001800000000000003",
                "00000001febd",
                "000000010134",
            ),
        ] {
            for rate in [subnormal, huge] {
                let mut original = hex(vector);
                original[12..18].copy_from_slice(&hex(rate));
                let mut decoder = Decoder {
                    r: Reader::new(&original, 0, big),
                    num_objects: 10,
                };
                let mut node = decode(*b"PRCL", &mut decoder).unwrap().unwrap();
                assert_eq!(decoder.r.remaining(), 0);
                if rate == subnormal {
                    assert_eq!(node.get("edit_rate"), Some(&Value::Float(1e-323)));
                }
                let source = source_only_from_payload(*b"PRCL", &original, big).unwrap();
                assert_eq!(source.edit_rate.unwrap().0, hex(rate).as_slice());
                assert_eq!(write_curve(&node, big, source, original.len()).unwrap(), original);

                node.fields.iter_mut().find(|(name, _)| *name == "edit_rate").unwrap().1 =
                    Value::Float(24.0);
                let changed = write_curve(&node, big, source, original.len()).unwrap();
                assert_ne!(&changed[12..18], &original[12..18]);
                assert_eq!(Reader::new(&changed[12..18], 0, big).exp10_float().unwrap(), 24.0);
            }
        }
    }

    #[test]
    fn unchanged_edit_rate_transcodes_original_pair_across_byte_orders() {
        let vectors = [
            (
                false,
                "020300000000000000000100180000000000ffffffff0000000000000000000000000101480000000002016400000002010200000004000100000000000000010000001800000000000000000003",
            ),
            (
                true,
                "020300000000000000000001000000180000ffffffff0000000000000000000000000101480000000002010000006402010000000200040000000100000000000000010000001800000000000003",
            ),
        ];
        // The 43.0664 source encoding is produced by pinned py-avb's writer:
        // mantissa 43_066_399, exponent -6. Re-encoding its decoded f64 may
        // select a different pair, so the stored pair must be transcoded.
        for (source_be, vector) in vectors {
            for (source_rate, target_rate) in [
                ("1f249102faff", "0291241ffffa"), // 43.0664 writer input
                ("01000000bdfe", "00000001febd"), // 1e-323
                ("010000003401", "000000010134"), // 1e308
            ] {
                let (source_rate, target_rate) = if source_be {
                    (target_rate, source_rate)
                } else {
                    (source_rate, target_rate)
                };
                let mut payload = hex(vector);
                payload[12..18].copy_from_slice(&hex(source_rate));
                let mut decoder = Decoder {
                    r: Reader::new(&payload, 0, source_be),
                    num_objects: 10,
                };
                let node = decode(*b"PRCL", &mut decoder).unwrap().unwrap();
                let source = source_only_from_payload(*b"PRCL", &payload, source_be).unwrap();
                assert_eq!(write_curve(&node, source_be, source, payload.len()).unwrap(), payload);

                let opposite = write_curve(&node, !source_be, source, payload.len()).unwrap();
                assert_eq!(&opposite[12..18], hex(target_rate), "source big={source_be}");
                let mut reread = Decoder {
                    r: Reader::new(&opposite, 0, !source_be),
                    num_objects: 10,
                };
                let decoded = decode(*b"PRCL", &mut reread).unwrap().unwrap();
                assert_eq!(decoded.get("edit_rate"), node.get("edit_rate"));
                assert_eq!(reread.r.remaining(), 0);
            }
        }
    }

    fn avbs(root: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                avbs(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "avb") {
                out.push(path);
            }
        }
    }

    #[test]
    fn pinned_full_bin_chunks_when_available() {
        let Ok(root) = std::env::var("PYAVB_TEST_FILES") else {
            return;
        };
        let mut paths = Vec::new();
        avbs(Path::new(&root), &mut paths);
        paths.sort();
        let mut counts = std::collections::BTreeMap::new();
        let mut prcl_types = std::collections::BTreeMap::new();
        let mut pp_types = std::collections::BTreeMap::new();
        let mut failures = Vec::new();
        for path in paths {
            let index = AvbIndex::open(&path).unwrap();
            for id in 1..index.object_count() as u32 {
                let object = index.object(id).unwrap();
                if !matches!(&object.class, b"PRCL" | b"CTRL" | b"ECCP") {
                    continue;
                }
                *counts.entry(object.class).or_insert(0usize) += 1;
                let payload = index.payload(id).unwrap();
                let node = index.decode_object(id).unwrap().unwrap();
                if object.class == *b"PRCL" {
                    *prcl_types
                        .entry(int(&node, "value_type", 0, 10).unwrap())
                        .or_insert(0usize) += 1;
                    for point in points(&node).unwrap() {
                        for prop in list(child(point, "point").unwrap(), "pp").unwrap() {
                            let kind =
                                int(child(prop, "property").unwrap(), "type", 0, 10).unwrap();
                            *pp_types.entry(kind).or_insert(0usize) += 1;
                        }
                    }
                }
                let source =
                    source_only_from_payload(object.class, payload, index.big_endian).unwrap();
                let output = write_curve(&node, index.big_endian, source, payload.len());
                if output.as_deref() != Ok(payload) {
                    let detail = match output {
                        Ok(bytes) => format!(
                            "byte {}",
                            bytes
                                .iter()
                                .zip(payload)
                                .position(|(a, b)| a != b)
                                .unwrap_or(bytes.len().min(payload.len()))
                        ),
                        Err(e) => e.to_string(),
                    };
                    failures.push(format!(
                        "{} id {} {}: {detail}",
                        path.display(),
                        id,
                        String::from_utf8_lossy(&object.class)
                    ));
                }
            }
        }
        eprintln!("curve class counts: {counts:?}");
        eprintln!("PRCL value types: {prcl_types:?}; point-property types: {pp_types:?}");
        assert!(
            failures.is_empty(),
            "{} failures; first ten {:#?}",
            failures.len(),
            &failures[..failures.len().min(10)]
        );
    }

    #[test]
    fn missing_source_bytes_and_bounds_fail() {
        let original = hex("020300000000000000000100180000000000ffffffff000000000000000000000000010148000000000201640000000201010203040506070801020300000000002c00000003");
        let mut d = Decoder {
            r: Reader::new(&original, 0, false),
            num_objects: 10,
        };
        let node = decode(*b"ECCP", &mut d).unwrap().unwrap();
        assert!(write_curve(&node, false, CurveSourceOnly::default(), 1000).is_err());
        let source = source_only_from_payload(*b"ECCP", &original, false).unwrap();
        assert!(write_curve(&node, false, source, original.len() - 1).is_err());
        for end in 0..original.len() {
            assert!(source_only_from_payload(*b"ECCP", &original[..end], false).is_err());
        }
    }

    #[test]
    fn wrong_point_types_and_truncated_curve_payloads_fail() {
        let prcl = hex("020300000000000000000100180000000000ffffffff000000000000000000000000010148000000000201640000000201030000000100010000000100000002000000180000003700000001000900010004000000010147020000000102470400000003");
        let mut d = Decoder {
            r: Reader::new(&prcl, 0, false),
            num_objects: 10,
        };
        let mut node = decode(*b"PRCL", &mut d).unwrap().unwrap();
        let source = source_only_from_payload(*b"PRCL", &prcl, false).unwrap();
        node.fields
            .iter_mut()
            .find(|(name, _)| *name == "value_type")
            .unwrap()
            .1 = Value::Int(2);
        assert!(write_curve(&node, false, source, 1000).is_err());
        node.fields
            .iter_mut()
            .find(|(name, _)| *name == "value_type")
            .unwrap()
            .1 = Value::Int(1);
        node.fields
            .iter_mut()
            .find(|(name, _)| *name == "control_points")
            .unwrap()
            .1 = Value::List(vec![Value::Null]);
        assert!(write_curve(&node, false, source, 1000).is_err());
        for end in 0..prcl.len() {
            assert!(source_only_from_payload(*b"PRCL", &prcl[..end], false).is_err());
        }

        let ctrl = hex("020300000000000000000100180000000000ffffffff000000000000000000000000010148000000000201640000000203030000000100000001000000020000001800000001050000000600000001000700080000000900000003");
        let mut d = Decoder {
            r: Reader::new(&ctrl, 0, false),
            num_objects: 10,
        };
        let mut node = decode(*b"CTRL", &mut d).unwrap().unwrap();
        let source = source_only_from_payload(*b"CTRL", &ctrl, false).unwrap();
        node.fields
            .iter_mut()
            .find(|(name, _)| *name == "control_points")
            .unwrap()
            .1 = Value::Str("bad".into());
        assert!(write_curve(&node, false, source, 1000).is_err());
        for end in 0..ctrl.len() {
            assert!(source_only_from_payload(*b"CTRL", &ctrl[..end], false).is_err());
        }
    }
}
