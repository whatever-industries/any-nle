//! Typed writers for the first AVB component family. The field order follows
//! pinned pyavb `components.py` (MIT; see NOTICE). Original-only bytes omitted
//! by `Node` must be supplied explicitly; canonical output is not lossless.

use crate::io::{Error, Reader, Result};
use crate::objects::{decode, Decoder, Node, Value};
use crate::writer_io::Writer;

#[derive(Debug, Clone, Copy, Default)]
pub struct SourceOnly {
    /// Exact six-byte source encoding, decoded finite f64 bits, source order.
    /// Multiple mantissa/exponent pairs can decode to the same edit rate.
    pub edit_rate_original: Option<([u8; 6], u64, bool)>,
    /// COMP extension presence is lost when its reference is null.
    pub param_list_present: Option<bool>,
    /// SCLP's two u32 words, discarded by the current typed reader.
    pub source_mob_words: Option<[u32; 2]>,
    /// TCCP's six unused bytes, discarded by the current typed reader.
    pub timecode_reserved: Option<[u8; 6]>,
}

fn field<'a>(node: &'a Node, name: &str) -> Result<&'a Value> {
    node.get(name)
        .ok_or_else(|| Error::new(format!("{} lacks {name}", node.class_str()), 0))
}

fn int(node: &Node, name: &str, min: i64, max: i64) -> Result<i64> {
    let value = match field(node, name)? {
        Value::Int(v) => *v,
        Value::UInt(v) if *v <= i64::MAX as u64 => *v as i64,
        _ => return Err(Error::new(format!("{name} is not an integer"), 0)),
    };
    if value < min || value > max {
        return Err(Error::new(format!("{name} is outside {min}..{max}"), 0));
    }
    Ok(value)
}

fn object_ref(value: &Value, name: &str) -> Result<Option<u32>> {
    match value {
        Value::Null => Ok(None),
        Value::Ref(v) if *v != 0 => Ok(Some(*v)),
        _ => Err(Error::new(format!("{name} is not an object ref/null"), 0)),
    }
}

fn opt_string(w: &mut Writer, value: &Value, name: &str) -> Result<()> {
    match value {
        Value::Null => w.u16(0xffff), // pyavb Component.write absent-string form
        Value::Str(s) if !s.is_empty() => w.string_macroman(s),
        _ => Err(Error::new(
            format!("{name} is not a nonempty string/null"),
            w.len(),
        )),
    }
}

fn base(node: &Node, w: &mut Writer, source_only: SourceOnly) -> Result<()> {
    w.u8(0x02)?;
    w.u8(0x03)?;
    for name in ["left_bob", "right_bob"] {
        w.object_ref_index(object_ref(field(node, name)?, name)?)?;
    }
    w.s16(int(node, "media_kind_id", i16::MIN as i64, i16::MAX as i64)? as i16)?;
    match field(node, "edit_rate")? {
        Value::Float(v) => {
            if let Some((bytes, original_bits, source_big_endian)) = source_only.edit_rate_original
            {
                if v.is_finite() && v.to_bits() == original_bits {
                    if source_big_endian == w.big_endian {
                        w.bytes(&bytes)?;
                    } else {
                        // The exp10 value is an s32 mantissa followed by an
                        // s16 exponent. Re-encode those same integers in the
                        // target order: recomputing them from f64 can change
                        // the decoded value for hard decimal pairs.
                        let mut reordered = bytes;
                        reordered[..4].reverse();
                        reordered[4..].reverse();
                        w.bytes(&reordered)?;
                    }
                } else {
                    w.exp10_float(*v)?;
                }
            } else {
                w.exp10_float(*v)?;
            }
        }
        _ => return Err(Error::new("edit_rate is not a float", w.len())),
    }
    for name in ["name", "effect_id"] {
        opt_string(w, field(node, name)?, name)?;
    }
    for name in ["attributes", "session_attrs", "precomputed"] {
        w.object_ref_index(object_ref(field(node, name)?, name)?)?;
    }
    match field(node, "param_list")? {
        Value::Null => match source_only.param_list_present {
            Some(true) => {
                w.u8(0x01)?;
                w.u8(0x01)?;
                w.u8(72)?;
                w.u32(0)?;
            }
            Some(false) => {}
            None => {
                return Err(Error::new(
                    "null param_list loses extension presence; original bytes required",
                    w.len(),
                ))
            }
        },
        reference => {
            w.u8(0x01)?;
            w.u8(0x01)?;
            w.u8(72)?;
            w.object_ref_index(object_ref(reference, "param_list")?)?;
        }
    }
    Ok(())
}

fn clip(node: &Node, w: &mut Writer, source_only: SourceOnly) -> Result<()> {
    base(node, w, source_only)?;
    w.u8(0x02)?;
    w.u8(0x01)?;
    w.u32(int(node, "length", 0, u32::MAX as i64)? as u32)
}

/// Serialize the known fields of COMP, CLIP, SCLP, SEQU, FILL, TCCP or TRKR.
/// The output is an object payload; the container class/length framing is a
/// separate layer. A SCLP or TCCP requires source-only bytes from its original
/// payload; a null Component param_list also needs its original presence bit.
/// This function will not synthesize data the `Node` reader discarded.
pub fn write_component(
    node: &Node,
    big_endian: bool,
    source_only: SourceOnly,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let mut w = Writer::with_limit(big_endian, max_bytes);
    match &node.class {
        b"COMP" => base(node, &mut w, source_only)?,
        b"CLIP" => clip(node, &mut w, source_only)?,
        b"SEQU" => {
            base(node, &mut w, source_only)?;
            w.u8(0x02)?;
            w.u8(0x03)?;
            let Value::RefList(refs) = field(node, "components")? else {
                return Err(Error::new("components is not a reference list", w.len()));
            };
            let count = u32::try_from(refs.len())
                .map_err(|_| Error::new("too many sequence components", w.len()))?;
            if refs.len() > max_bytes.saturating_sub(w.len().saturating_add(5)) / 4 {
                return Err(Error::new("sequence refs exceed payload limit", w.len()));
            }
            w.u32(count)?;
            for reference in refs {
                w.object_ref_index(object_ref(reference, "component")?)?;
            }
            w.u8(0x03)?;
        }
        b"FILL" => {
            clip(node, &mut w, source_only)?;
            w.u8(0x02)?;
            w.u8(0x01)?;
            w.u8(0x03)?;
        }
        b"SCLP" => {
            clip(node, &mut w, source_only)?;
            w.u8(0x02)?;
            w.u8(0x03)?;
            let words = source_only.source_mob_words.ok_or_else(|| {
                Error::new(
                    "SCLP source mob words were omitted by Node; original bytes required",
                    w.len(),
                )
            })?;
            w.u32(words[0])?;
            w.u32(words[1])?;
            w.s16(int(node, "track_id", i16::MIN as i64, i16::MAX as i64)? as i16)?;
            w.s32(int(node, "start_time", i32::MIN as i64, i32::MAX as i64)? as i32)?;
            if let Some(value) = node.get("mob_id") {
                let Value::MobId(id) = value else {
                    return Err(Error::new("mob_id is not a MobID", w.len()));
                };
                w.u8(0x01)?;
                w.u8(0x01)?;
                w.mob_id(id)?;
            }
            w.u8(0x03)?;
        }
        b"TCCP" => {
            clip(node, &mut w, source_only)?;
            w.u8(0x02)?;
            w.u8(0x01)?;
            w.u32(int(node, "flags", 0, u32::MAX as i64)? as u32)?;
            w.u16(int(node, "fps", 0, u16::MAX as i64)? as u16)?;
            let reserved = source_only.timecode_reserved.ok_or_else(|| {
                Error::new(
                    "TCCP reserved bytes were omitted by Node; original bytes required",
                    w.len(),
                )
            })?;
            w.bytes(&reserved)?;
            w.u32(int(node, "start", 0, u32::MAX as i64)? as u32)?;
            w.u8(0x03)?;
        }
        b"TRKR" => {
            clip(node, &mut w, source_only)?;
            w.u8(0x02)?;
            w.u8(0x01)?;
            w.s16(int(node, "relative_scope", i16::MIN as i64, i16::MAX as i64)? as i16)?;
            w.s16(int(node, "relative_track", i16::MIN as i64, i16::MAX as i64)? as i16)?;
            w.u8(0x03)?;
        }
        _ => {
            return Err(Error::new(
                format!("{} is outside this writer family", node.class_str()),
                0,
            ))
        }
    }
    Ok(w.into_bytes())
}

/// Extract source-only spans and extension presence omitted by `Node`. The
/// caller must pass the same class and payload that produced the Node.
pub fn source_only_from_payload(
    class: [u8; 4],
    payload: &[u8],
    big_endian: bool,
) -> Result<SourceOnly> {
    if !matches!(
        &class,
        b"COMP" | b"CLIP" | b"SCLP" | b"SEQU" | b"FILL" | b"TCCP" | b"TRKR"
    ) {
        return Err(Error::new("class outside component writer", 0));
    }
    // Validate the entire original before extracting source-only spans.
    let mut validated = Decoder {
        r: Reader::new(payload, 0, big_endian),
        num_objects: u32::MAX,
    };
    decode(class, &mut validated)?;
    if validated.r.remaining() != 0 {
        return validated.r.err("trailing component bytes");
    }
    let mut r = Reader::new(payload, 0, big_endian);
    r.assert_tag(0x02)?;
    r.assert_tag(0x03)?;
    r.u32()?;
    r.u32()?;
    r.s16()?;
    let mut edit_rate_bytes = [0; 6];
    edit_rate_bytes.copy_from_slice(r.bytes(6)?);
    let original_edit_rate = Reader::new(&edit_rate_bytes, 0, big_endian).exp10_float()?;
    for _ in 0..2 {
        let len = r.u16()?;
        if len != 0xffff {
            r.bytes(len as usize)?;
        }
    }
    r.u32()?;
    r.u32()?;
    r.u32()?;
    let mut present = false;
    while let Some(tag) = r.next_ext()? {
        if tag != 1 {
            return r.err("unsupported Component extension");
        }
        if present {
            return r.err("duplicate Component param_list extension");
        }
        present = true;
        r.assert_tag(72)?;
        r.u32()?;
    }
    let mut source_only = SourceOnly {
        edit_rate_original: Some((edit_rate_bytes, original_edit_rate.to_bits(), big_endian)),
        param_list_present: Some(present),
        ..Default::default()
    };
    if !matches!(&class, b"SCLP" | b"TCCP") {
        return Ok(source_only);
    }
    r.assert_tag(0x02)?;
    r.assert_tag(0x01)?;
    r.u32()?;
    r.assert_tag(0x02)?;
    if class == *b"SCLP" {
        r.assert_tag(0x03)?;
        let words = [r.u32()?, r.u32()?];
        r.s16()?;
        r.s32()?;
        let mut seen_mob_id = false;
        while let Some(tag) = r.next_ext()? {
            if tag != 1 {
                return r.err("unsupported SCLP extension");
            }
            if seen_mob_id {
                return r.err("duplicate SCLP MobID extension");
            }
            seen_mob_id = true;
            r.mob_id()?;
        }
        r.assert_tag(0x03)?;
        if r.remaining() != 0 {
            return r.err("trailing SCLP bytes");
        }
        source_only.source_mob_words = Some(words);
        Ok(source_only)
    } else {
        r.assert_tag(0x01)?;
        r.u32()?;
        r.u16()?;
        let mut reserved = [0; 6];
        reserved.copy_from_slice(r.bytes(6)?);
        r.u32()?;
        r.assert_tag(0x03)?;
        if r.remaining() != 0 {
            return r.err("trailing TCCP bytes");
        }
        source_only.timecode_reserved = Some(reserved);
        Ok(source_only)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::AvbIndex;
    use crate::io::{MobId, Uuid};
    use crate::objects::{decode, Decoder};
    use std::path::{Path, PathBuf};

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// Pinned pyavb `Component.write` and `Clip.write` invoked on a Filler
    /// carrying name "clip", rate 24, null attributes and param_list, length 100.
    #[test]
    fn pyavb_comp_and_clip_byte_vectors() {
        for (big, comp_hex, clip_hex) in [
            (false,
             "0203000000000000000001001800000000000400636c6970ffff00000000000000000000000001014800000000",
             "0203000000000000000001001800000000000400636c6970ffff00000000000000000000000001014800000000020164000000"),
            (true,
             "0203000000000000000000010000001800000004636c6970ffff00000000000000000000000001014800000000",
             "0203000000000000000000010000001800000004636c6970ffff00000000000000000000000001014800000000020100000064"),
        ] {
            for (class, text) in [(*b"COMP", comp_hex), (*b"CLIP", clip_hex)] {
                let payload = hex(text);
                let node = parsed(class, &payload, big);
                let supplement = source_only_from_payload(class, &payload, big).unwrap();
                assert!(matches!(node.get("param_list"), Some(Value::Null)));
                assert_eq!(supplement.param_list_present, Some(true));
                assert_eq!(write_component(&node, big, supplement, payload.len()).unwrap(), payload);
                assert!(write_component(&node, big, SourceOnly::default(), 1024).is_err());
            }
        }
    }

    fn fixture(class: [u8; 4], big: bool) -> Vec<u8> {
        let mut w = Writer::new(big);
        w.u8(2).unwrap();
        w.u8(3).unwrap();
        w.u32(1).unwrap();
        w.u32(2).unwrap();
        w.s16(1).unwrap();
        w.exp10_float(24.0).unwrap();
        w.string_macroman("clip").unwrap();
        w.u16(0xffff).unwrap();
        w.u32(3).unwrap();
        w.u32(0).unwrap();
        w.u32(0).unwrap();
        w.u8(1).unwrap();
        w.u8(1).unwrap();
        w.u8(72).unwrap();
        w.u32(4).unwrap();
        if class == *b"COMP" {
            return w.into_bytes();
        }
        if class == *b"SEQU" {
            w.u8(2).unwrap();
            w.u8(3).unwrap();
            w.u32(2).unwrap();
            w.u32(5).unwrap();
            w.u32(0).unwrap();
            w.u8(3).unwrap();
            return w.into_bytes();
        }
        w.u8(2).unwrap();
        w.u8(1).unwrap();
        w.u32(100).unwrap();
        match &class {
            b"CLIP" => {}
            b"FILL" => {
                w.u8(2).unwrap();
                w.u8(1).unwrap();
                w.u8(3).unwrap();
            }
            b"SCLP" => {
                w.u8(2).unwrap();
                w.u8(3).unwrap();
                w.u32(0x12345678).unwrap();
                w.u32(0xabcdef01).unwrap();
                w.s16(1).unwrap();
                w.s32(25).unwrap();
                let id = MobId::unique_with_material(Uuid {
                    data1: 0x12345678,
                    data2: 0x9abc,
                    data3: 0xdef0,
                    data4: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88],
                });
                w.u8(1).unwrap();
                w.u8(1).unwrap();
                w.mob_id(&id).unwrap();
                w.u8(3).unwrap();
            }
            b"TCCP" => {
                w.u8(2).unwrap();
                w.u8(1).unwrap();
                w.u32(1).unwrap();
                w.u16(24).unwrap();
                w.bytes(&[1, 2, 3, 4, 5, 6]).unwrap();
                w.u32(86400).unwrap();
                w.u8(3).unwrap();
            }
            b"TRKR" => {
                w.u8(2).unwrap();
                w.u8(1).unwrap();
                w.s16(-2).unwrap();
                w.s16(7).unwrap();
                w.u8(3).unwrap();
            }
            _ => panic!("unsupported fixture"),
        }
        w.into_bytes()
    }

    fn parsed(class: [u8; 4], payload: &[u8], big: bool) -> Node {
        let mut d = Decoder {
            r: Reader::new(payload, 0, big),
            num_objects: 10,
        };
        let node = decode(class, &mut d).unwrap().unwrap();
        assert_eq!(d.r.remaining(), 0);
        node
    }

    #[test]
    fn seven_synthetic_classes_roundtrip_both_orders() {
        for big in [false, true] {
            for class in [
                *b"COMP", *b"CLIP", *b"SEQU", *b"FILL", *b"SCLP", *b"TCCP", *b"TRKR",
            ] {
                let original = fixture(class, big);
                let node = parsed(class, &original, big);
                let supplement = source_only_from_payload(class, &original, big).unwrap();
                let output = write_component(&node, big, supplement, original.len()).unwrap();
                assert_eq!(output, original, "{:?} big={big}", class);
                assert_eq!(parsed(class, &output, big), node);
            }
        }
    }

    #[test]
    fn unchanged_noncanonical_edit_rate_preserves_source_pair_in_both_orders() {
        for big in [false, true] {
            for class in [
                *b"COMP", *b"CLIP", *b"SEQU", *b"FILL", *b"SCLP", *b"TCCP", *b"TRKR",
            ] {
                let mut original = fixture(class, big);
                let mut rate = Writer::new(big);
                rate.s32(240).unwrap();
                rate.s16(-1).unwrap();
                original[12..18].copy_from_slice(rate.as_bytes());
                let node = parsed(class, &original, big);
                assert_eq!(node.get("edit_rate"), Some(&Value::Float(24.0)));
                let source = source_only_from_payload(class, &original, big).unwrap();
                assert_eq!(
                    write_component(&node, big, source, original.len()).unwrap(),
                    original
                );
                assert!(write_component(&node, big, source, original.len() - 1).is_err());

                let opposite = write_component(&node, !big, source, original.len()).unwrap();
                let mut original_pair = Writer::new(!big);
                original_pair.s32(240).unwrap();
                original_pair.s16(-1).unwrap();
                assert_eq!(&opposite[12..18], original_pair.as_bytes());
                assert_eq!(
                    parsed(class, &opposite, !big).get("edit_rate"),
                    Some(&Value::Float(24.0))
                );

                let mut changed = node.clone();
                changed
                    .fields
                    .iter_mut()
                    .find(|(name, _)| *name == "edit_rate")
                    .unwrap()
                    .1 = Value::Float(25.0);
                let output = write_component(&changed, big, source, original.len()).unwrap();
                let mut canonical = Writer::new(big);
                canonical.exp10_float(25.0).unwrap();
                assert_eq!(&output[12..18], canonical.as_bytes());
                assert_eq!(
                    parsed(class, &output, big).get("edit_rate"),
                    Some(&Value::Float(25.0))
                );
            }
        }
    }

    #[test]
    fn hard_decimal_rate_preserves_mantissa_exponent_when_transcoding() {
        for big in [false, true] {
            for class in [*b"FILL", *b"SCLP"] {
                let mut original = fixture(class, big);
                let mut rate = Writer::new(big);
                rate.s32(2997).unwrap();
                rate.s16(-2).unwrap();
                original[12..18].copy_from_slice(rate.as_bytes());
                let node = parsed(class, &original, big);
                let original_rate = match node.get("edit_rate").unwrap() {
                    Value::Float(value) => *value,
                    _ => panic!("missing edit rate"),
                };
                assert!(original_rate.is_finite());
                let source = source_only_from_payload(class, &original, big).unwrap();
                assert_eq!(
                    write_component(&node, big, source, original.len()).unwrap(),
                    original
                );

                let transcoded = write_component(&node, !big, source, original.len()).unwrap();
                let mut expected = Writer::new(!big);
                expected.s32(2997).unwrap();
                expected.s16(-2).unwrap();
                assert_eq!(&transcoded[12..18], expected.as_bytes());
                assert_eq!(
                    parsed(class, &transcoded, !big).get("edit_rate"),
                    Some(&Value::Float(original_rate))
                );
                assert!(write_component(&node, !big, source, original.len() - 1).is_err());
                if class == *b"FILL" {
                    let fresh = SourceOnly {
                        param_list_present: Some(false),
                        ..Default::default()
                    };
                    let normal = write_component(&node, big, fresh, original.len()).unwrap();
                    let mut canonical = Writer::new(big);
                    canonical.exp10_float(original_rate).unwrap();
                    assert_eq!(&normal[12..18], canonical.as_bytes());
                }
            }
        }
    }

    #[test]
    fn omitted_source_bytes_cannot_be_synthesized() {
        for class in [*b"SCLP", *b"TCCP"] {
            let payload = fixture(class, false);
            let node = parsed(class, &payload, false);
            assert!(write_component(&node, false, SourceOnly::default(), payload.len()).is_err());
            for end in 0..payload.len() {
                assert!(
                    source_only_from_payload(class, &payload[..end], false).is_err(),
                    "{class:?} accepted truncated length {end}"
                );
            }
        }
    }

    #[test]
    fn bounded_and_wrong_types_fail() {
        let original = fixture(*b"SEQU", false);
        let mut node = parsed(*b"SEQU", &original, false);
        assert!(write_component(&node, false, SourceOnly::default(), original.len() - 1).is_err());
        node.fields
            .iter_mut()
            .find(|(k, _)| *k == "components")
            .unwrap()
            .1 = Value::Str("bad".into());
        assert!(write_component(&node, false, SourceOnly::default(), 1000).is_err());
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

    /// All 14 small full bins in the pinned pyavb test suite are opt-in.
    #[test]
    fn pinned_full_bin_chunk_roundtrips_when_available() {
        let Ok(root) = std::env::var("PYAVB_TEST_FILES") else {
            return;
        };
        let mut paths = Vec::new();
        avbs(Path::new(&root), &mut paths);
        paths.sort();
        let mut classes = std::collections::BTreeMap::new();
        let mut mismatches = Vec::new();
        for path in paths {
            let index = AvbIndex::open(&path).unwrap();
            for id in 1..index.object_count() as u32 {
                let object = index.object(id).unwrap();
                if !matches!(
                    &object.class,
                    b"COMP" | b"CLIP" | b"SCLP" | b"SEQU" | b"FILL" | b"TCCP" | b"TRKR"
                ) {
                    continue;
                }
                *classes.entry(object.class).or_insert(0usize) += 1;
                let payload = index.payload(id).unwrap();
                let node = index.decode_object(id).unwrap().unwrap();
                let supplement =
                    source_only_from_payload(object.class, payload, index.big_endian).unwrap();
                let output = write_component(&node, index.big_endian, supplement, payload.len());
                if output.as_deref() != Ok(payload) {
                    let detail = match &output {
                        Ok(bytes) => {
                            let at = bytes
                                .iter()
                                .zip(payload)
                                .position(|(a, b)| a != b)
                                .unwrap_or(bytes.len().min(payload.len()));
                            format!(
                                "byte {at}: original={:?} written={:?}",
                                &payload[at.saturating_sub(3)..payload.len().min(at + 8)],
                                &bytes[at.saturating_sub(3)..bytes.len().min(at + 8)]
                            )
                        }
                        Err(e) => e.to_string(),
                    };
                    mismatches.push(format!(
                        "{} object {} {:?}: {}",
                        path.display(),
                        id,
                        object.class,
                        detail
                    ));
                }
            }
        }
        eprintln!("component classes in pinned bins: {classes:?}");
        assert!(
            mismatches.is_empty(),
            "{} mismatches; first ten: {:#?}",
            mismatches.len(),
            &mismatches[..mismatches.len().min(10)]
        );
    }
}
