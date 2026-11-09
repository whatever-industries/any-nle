//! Bounded payload serialization for the simple media-descriptor family.
//! Field order and framing follow pinned pyavb `essence.py` (MIT; see NOTICE).
//! This writes a single payload with existing reference IDs; it is not a file writer.

use crate::io::{Error, Reader, Result};
use crate::objects::{decode, Decoder, Node, Value};
use crate::writer_io::Writer;
use std::collections::HashSet;

const BASE: &[&str] = &[
    "mob_kind",
    "locator",
    "intermediate",
    "physical_media",
    "uuid",
    "wchar",
    "attributes",
];
const FILE: &[&str] = &["edit_rate", "length", "is_omfi", "data_offset"];
const AUDIO: &[&str] = &[
    "channels",
    "quantization_bits",
    "sample_rate",
    "locked",
    "audio_ref_level",
    "electro_spatial_formulation",
    "dial_norm",
    "coding_format",
];
const DID: &[&str] = &[
    "stored_height",
    "stored_width",
    "sampled_height",
    "sampled_width",
    "sampled_x_offset",
    "sampled_y_offset",
    "display_height",
    "display_width",
    "display_x_offset",
    "display_y_offset",
    "frame_layout",
    "aspect_ratio",
    "line_map",
    "alpha_transparency",
    "uniformness",
    "did_image_size",
    "next_did_desc",
    "compress_method",
    "resolution_id",
    "image_alignment_factor",
    "frame_index_byte_order",
    "frame_sample_size",
    "first_frame_offset",
    "client_fill_start",
    "client_fill_end",
    "offset_to_rle_frame_index",
    "frame_start_offset",
    "valid_box",
    "essence_box",
    "source_box",
    "framing_box",
    "reformatting_option",
    "transfer_characteristic",
    "color_primaries",
    "coding_equations",
    "essence_compression",
    "essence_element_size_kind",
    "frame_checked_with_mapper",
];
const CDCI: &[&str] = &[
    "horizontal_subsampling",
    "vertical_subsampling",
    "component_width",
    "color_sitting",
    "black_ref_level",
    "white_ref_level",
    "color_range",
    "frame_index_offset",
    "alpha_sampled_width",
    "ignore_bw",
];

fn err<T>(w: &Writer, message: impl Into<String>) -> Result<T> {
    Err(Error::new(message, w.len()))
}

fn required<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a Value> {
    n.get(name)
        .ok_or_else(|| Error::new(format!("{}: missing {name}", n.class_str()), w.len()))
}

fn int(n: &Node, name: &str, w: &Writer) -> Result<i64> {
    match required(n, name, w)? {
        Value::Int(v) => Ok(*v),
        _ => err(w, format!("{}: {name} is not an integer", n.class_str())),
    }
}

fn i16_field(n: &Node, name: &str, w: &Writer) -> Result<i16> {
    i16::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} outside i16"), w.len()))
}

fn i32_field(n: &Node, name: &str, w: &Writer) -> Result<i32> {
    i32::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} outside i32"), w.len()))
}

fn u8_field(n: &Node, name: &str, w: &Writer) -> Result<u8> {
    u8::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} outside u8"), w.len()))
}

fn u16_field(n: &Node, name: &str, w: &Writer) -> Result<u16> {
    u16::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} outside u16"), w.len()))
}

fn u32_field(n: &Node, name: &str, w: &Writer) -> Result<u32> {
    u32::try_from(int(n, name, w)?).map_err(|_| Error::new(format!("{name} outside u32"), w.len()))
}

fn u64_field(n: &Node, name: &str, w: &Writer) -> Result<u64> {
    match required(n, name, w)? {
        Value::UInt(v) => Ok(*v),
        Value::Int(v) if *v >= 0 => Ok(*v as u64),
        _ => err(w, format!("{name} is not u64")),
    }
}

fn s64_field(n: &Node, name: &str, w: &Writer) -> Result<i64> {
    int(n, name, w)
}

fn bool_field(n: &Node, name: &str, w: &Writer) -> Result<bool> {
    match required(n, name, w)? {
        Value::Bool(v) => Ok(*v),
        _ => err(w, format!("{name} is not bool")),
    }
}

fn float_field(n: &Node, name: &str, w: &Writer) -> Result<f64> {
    match required(n, name, w)? {
        Value::Float(v) => Ok(*v),
        _ => err(w, format!("{name} is not float")),
    }
}

fn bytes_field<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a [u8]> {
    match required(n, name, w)? {
        Value::Bytes(v) => Ok(v),
        _ => err(w, format!("{name} is not bytes")),
    }
}

fn uuid_field<'a>(n: &'a Node, name: &str, w: &Writer) -> Result<&'a crate::io::Uuid> {
    match required(n, name, w)? {
        Value::Uuid(v) => Ok(v),
        _ => err(w, format!("{name} is not UUID")),
    }
}

fn optional_pair(n: &Node, a: &str, b: &str, w: &Writer) -> Result<bool> {
    if n.get(a).is_some() != n.get(b).is_some() {
        return err(w, format!("{a} and {b} must appear together"));
    }
    Ok(n.get(a).is_some())
}

fn ext_i32(n: &Node, name: &str, tag: u8, w: &mut Writer) -> Result<()> {
    if n.get(name).is_some() {
        w.bytes(&[1, tag, 71])?;
        w.s32(i32_field(n, name, w)?)?;
    }
    Ok(())
}

fn reference(value: &Value, w: &Writer) -> Result<Option<u32>> {
    match value {
        Value::Null => Ok(None),
        Value::Ref(v) if *v != 0 => Ok(Some(*v)),
        _ => err(w, "descriptor reference is not Null or nonzero Ref"),
    }
}

fn write_base(n: &Node, w: &mut Writer) -> Result<()> {
    w.u8(2)?;
    w.u8(3)?;
    w.u8(u8::try_from(int(n, "mob_kind", w)?)
        .map_err(|_| Error::new("mob_kind outside u8", w.len()))?)?;
    w.object_ref_index(reference(required(n, "locator", w)?, w)?)?;
    let intermediate = match required(n, "intermediate", w)? {
        Value::Bool(v) => *v,
        _ => return err(w, "intermediate is not bool"),
    };
    w.bool(intermediate)?;
    w.object_ref_index(reference(required(n, "physical_media", w)?, w)?)?;
    if let Some(value) = n.get("uuid") {
        let Value::Uuid(uuid) = value else {
            return err(w, "uuid has wrong type");
        };
        w.bytes(&[1, 1, 65])?;
        w.s32(16)?;
        w.raw_uuid(uuid)?;
    }
    if let Some(value) = n.get("wchar") {
        let Value::Bytes(raw) = value else {
            return err(w, "wchar has wrong type");
        };
        let size = i32::try_from(raw.len())
            .map_err(|_| Error::new("wchar exceeds i32 length", w.len()))?;
        w.bytes(&[1, 2, 65])?;
        w.s32(size)?;
        w.bytes(raw)?;
    }
    if let Some(value) = n.get("attributes") {
        w.bytes(&[1, 3, 72])?;
        w.object_ref_index(reference(value, w)?)?;
    }
    Ok(())
}

fn write_file(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 3])?;
    let rate = match required(n, "edit_rate", w)? {
        Value::Float(v) => *v,
        _ => return err(w, "edit_rate is not float"),
    };
    w.exp10_float(rate)?;
    w.s32(i32_field(n, "length", w)?)?;
    w.s16(i16_field(n, "is_omfi", w)?)?;
    w.s32(i32_field(n, "data_offset", w)?)
}

fn write_audio_base(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])?;
    w.u16(u16_field(n, "channels", w)?)?;
    w.u16(u16_field(n, "quantization_bits", w)?)?;
    w.exp10_float(float_field(n, "sample_rate", w)?)?;
    w.bool(bool_field(n, "locked", w)?)?;
    w.s16(i16_field(n, "audio_ref_level", w)?)?;
    w.s32(i32_field(n, "electro_spatial_formulation", w)?)?;
    w.u16(u16_field(n, "dial_norm", w)?)?;
    w.u32(u32_field(n, "coding_format", w)?)?;
    Ok(())
}

fn write_mpga(n: &Node, w: &mut Writer) -> Result<()> {
    write_audio_base(n, w)?;
    w.u32(u32_field(n, "bit_rate", w)?)?;
    for (name, tag) in [("sub_frame_alignment", 1), ("origin", 2)] {
        if n.get(name).is_some() {
            w.bytes(&[1, tag, 77])?;
            w.u64(u64_field(n, name, w)?)?;
        }
    }
    w.u8(3)
}

fn write_pcma(n: &Node, w: &mut Writer) -> Result<()> {
    write_audio_base(n, w)?;
    w.u32(u32_field(n, "block_align", w)?)?;
    w.u16(u16_field(n, "sequence_offset", w)?)?;
    w.u32(u32_field(n, "average_bps", w)?)?;
    w.bool(bool_field(n, "has_peak_envelope_data", w)?)?;
    for name in [
        "peak_envelope_version",
        "peak_envelope_format",
        "points_per_peak_value",
        "peak_envelope_block_size",
        "peak_channel_count",
        "peak_frame_count",
    ] {
        w.s32(i32_field(n, name, w)?)?;
    }
    w.u64(u64_field(n, "peak_of_peaks_offset", w)?)?;
    w.s32(i32_field(n, "peak_envelope_timestamp", w)?)?;
    if n.get("ebu_timestamp").is_some() {
        w.bytes(&[1, 1, 77])?;
        w.s64(s64_field(n, "ebu_timestamp", w)?)?;
    }
    if let Some(value) = n.get("timecode_framerate") {
        let Value::Str(s) = value else {
            return err(w, "timecode_framerate is not text");
        };
        w.bytes(&[1, 3, 76])?;
        w.string_macroman(s)?;
    }
    w.u8(3)
}

fn write_data(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])?;
    w.bool(bool_field(n, "is_offset_to_frame_indexes_valid", w)?)?;
    w.u64(u64_field(n, "offset_to_frame_indexes", w)?)?;
    for name in ["first_frame_offset", "min_sample_size", "max_sample_size"] {
        w.s32(i32_field(n, name, w)?)?;
    }
    // Pinned pyavb deliberately omits a terminator for bare DATD.
    if n.class == *b"ANCD" {
        w.bytes(&[2, 1])?;
        w.s32(i32_field(n, "manifest_element_count", w)?)?;
        w.u8(3)?;
    }
    Ok(())
}

fn write_point_box(n: &Node, name: &str, w: &mut Writer) -> Result<()> {
    let Value::List(points) = required(n, name, w)? else {
        return err(w, format!("{name} is not points"));
    };
    if points.len() != 4 {
        return err(w, format!("{name} needs four points"));
    }
    for point in points {
        let Value::List(xy) = point else {
            return err(w, format!("{name} point is not a pair"));
        };
        if xy.len() != 2 {
            return err(w, format!("{name} point is not a pair"));
        }
        for coordinate in xy {
            let Value::Int(value) = coordinate else {
                return err(w, format!("{name} coordinate is not int"));
            };
            w.u8(71)?;
            w.s32(
                i32::try_from(*value)
                    .map_err(|_| Error::new(format!("{name} coordinate outside i32"), w.len()))?,
            )?;
        }
    }
    Ok(())
}

fn write_did(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 2])?;
    for name in [
        "stored_height",
        "stored_width",
        "sampled_height",
        "sampled_width",
        "sampled_x_offset",
        "sampled_y_offset",
        "display_height",
        "display_width",
        "display_x_offset",
        "display_y_offset",
    ] {
        w.s32(i32_field(n, name, w)?)?;
    }
    w.s16(i16_field(n, "frame_layout", w)?)?;
    let Value::List(aspect) = required(n, "aspect_ratio", w)? else {
        return err(w, "aspect_ratio is not a pair");
    };
    if aspect.len() != 2 {
        return err(w, "aspect_ratio must contain two integers");
    }
    for part in aspect {
        let Value::Int(value) = part else {
            return err(w, "aspect_ratio element is not integer");
        };
        w.s32(i32::try_from(*value).map_err(|_| Error::new("aspect_ratio outside i32", w.len()))?)?;
    }
    let Value::List(line_map) = required(n, "line_map", w)? else {
        return err(w, "line_map is not list");
    };
    w.s32(
        i32::try_from(
            line_map
                .len()
                .checked_mul(4)
                .ok_or_else(|| Error::new("line_map size overflow", w.len()))?,
        )
        .map_err(|_| Error::new("line_map too long", w.len()))?,
    )?;
    for v in line_map {
        let Value::Int(value) = v else {
            return err(w, "line_map element is not integer");
        };
        w.s32(
            i32::try_from(*value)
                .map_err(|_| Error::new("line_map element outside i32", w.len()))?,
        )?;
    }
    w.s32(i32_field(n, "alpha_transparency", w)?)?;
    w.bool(bool_field(n, "uniformness", w)?)?;
    w.s32(i32_field(n, "did_image_size", w)?)?;
    w.object_ref_index(reference(required(n, "next_did_desc", w)?, w)?)?;
    let method = bytes_field(n, "compress_method", w)?;
    if method.len() != 4 {
        return err(w, "compress_method is not FourCC");
    }
    w.fourcc(method.try_into().unwrap())?;
    w.s32(i32_field(n, "resolution_id", w)?)?;
    w.s32(i32_field(n, "image_alignment_factor", w)?)?;
    if n.get("frame_index_byte_order").is_some() {
        w.bytes(&[1, 1, 69])?;
        w.s16(i16_field(n, "frame_index_byte_order", w)?)?;
    }
    ext_i32(n, "frame_sample_size", 2, w)?;
    ext_i32(n, "first_frame_offset", 3, w)?;
    if optional_pair(n, "client_fill_start", "client_fill_end", w)? {
        w.bytes(&[1, 4, 71])?;
        w.s32(i32_field(n, "client_fill_start", w)?)?;
        w.u8(71)?;
        w.s32(i32_field(n, "client_fill_end", w)?)?;
    }
    ext_i32(n, "offset_to_rle_frame_index", 5, w)?;
    ext_i32(n, "frame_start_offset", 6, w)?;
    let has_boxes = ["valid_box", "essence_box", "source_box"].map(|name| n.get(name).is_some());
    if has_boxes.iter().any(|v| *v) {
        if !has_boxes.iter().all(|v| *v) {
            return err(w, "valid, essence and source boxes must appear together");
        }
        w.bytes(&[1, 8])?;
        for name in ["valid_box", "essence_box", "source_box"] {
            write_point_box(n, name, w)?;
        }
    }
    if optional_pair(n, "framing_box", "reformatting_option", w)? {
        w.bytes(&[1, 9])?;
        write_point_box(n, "framing_box", w)?;
        w.u8(71)?;
        w.s32(i32_field(n, "reformatting_option", w)?)?;
    }
    if n.get("transfer_characteristic").is_some() {
        w.bytes(&[1, 10, 80])?;
        w.raw_uuid(uuid_field(n, "transfer_characteristic", w)?)?;
    }
    if optional_pair(n, "color_primaries", "coding_equations", w)? {
        w.bytes(&[1, 11, 80])?;
        w.raw_uuid(uuid_field(n, "color_primaries", w)?)?;
        w.u8(80)?;
        w.raw_uuid(uuid_field(n, "coding_equations", w)?)?;
    }
    if n.get("essence_compression").is_some() {
        w.bytes(&[1, 12, 80])?;
        w.raw_uuid(uuid_field(n, "essence_compression", w)?)?;
    }
    if n.get("essence_element_size_kind").is_some() {
        w.bytes(&[1, 14, 68])?;
        w.u8(u8_field(n, "essence_element_size_kind", w)?)?;
    }
    if n.get("frame_checked_with_mapper").is_some() {
        w.bytes(&[1, 15, 66])?;
        w.bool(bool_field(n, "frame_checked_with_mapper", w)?)?;
    }
    if n.class == *b"DIDD" {
        w.u8(3)?;
    }
    Ok(())
}

fn write_cdci(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 2])?;
    w.u32(u32_field(n, "horizontal_subsampling", w)?)?;
    w.u32(u32_field(n, "vertical_subsampling", w)?)?;
    w.s32(i32_field(n, "component_width", w)?)?;
    w.s16(i16_field(n, "color_sitting", w)?)?;
    for name in ["black_ref_level", "white_ref_level", "color_range"] {
        w.u32(u32_field(n, name, w)?)?;
    }
    w.s64(s64_field(n, "frame_index_offset", w)?)?;
    for (name, tag) in [("alpha_sampled_width", 1), ("ignore_bw", 2)] {
        if n.get(name).is_some() {
            w.bytes(&[1, tag, 72])?;
            w.u32(u32_field(n, name, w)?)?;
        }
    }
    if n.class == *b"CDCI" {
        w.u8(3)?;
    }
    Ok(())
}

fn write_mpgi(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])?;
    for name in ["mpeg_version", "profile", "gop_structure", "stream_type"] {
        w.u8(u8_field(n, name, w)?)?;
    }
    for name in ["random_access", "leading_discard", "trailing_discard"] {
        w.bool(bool_field(n, name, w)?)?;
    }
    w.u16(u16_field(n, "min_gop_length", w)?)?;
    w.u16(u16_field(n, "max_gop_length", w)?)?;
    let hdr = bytes_field(n, "sequence_hdr", w)?;
    w.s32(i32::try_from(hdr.len()).map_err(|_| Error::new("sequence_hdr too long", w.len()))?)?;
    w.bytes(hdr)?;
    w.u8(3)
}

fn write_jped(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])?;
    w.s32(i32_field(n, "jpeg_table_id", w)?)?;
    w.u64(u64_field(n, "jpeg_frame_index_offset", w)?)?;
    let tables = bytes_field(n, "quantization_tables", w)?;
    w.s32(
        i32::try_from(tables.len())
            .map_err(|_| Error::new("quantization_tables too long", w.len()))?,
    )?;
    w.bytes(tables)?;
    ext_i32(n, "image_start_align", 1, w)?;
    w.u8(3)
}

fn write_rgba(n: &Node, w: &mut Writer) -> Result<()> {
    w.bytes(&[2, 1])?;
    let Value::List(layout) = required(n, "pixel_layout", w)? else {
        return err(w, "pixel_layout is not list");
    };
    let len =
        u32::try_from(layout.len()).map_err(|_| Error::new("pixel_layout too long", w.len()))?;
    let mut codes = Vec::with_capacity(layout.len());
    let mut sizes = Vec::with_capacity(layout.len());
    for item in layout {
        let Value::Node(element) = item else {
            return err(w, "pixel layout entry is not Node");
        };
        codes.push(u8_field(element, "Code", w)?);
        sizes.push(u8_field(element, "Size", w)?);
    }
    w.u32(len)?;
    w.bytes(&codes)?;
    w.u32(len)?;
    w.bytes(&sizes)?;
    for _ in 0..3 {
        w.u32(0)?;
    } // pyavb only supports empty palette layout/structure/palette.
    if n.get("frame_index_offset").is_some() {
        w.bytes(&[1, 1, 77])?;
        w.u64(u64_field(n, "frame_index_offset", w)?)?;
    }
    if n.get("has_comp_min_ref").is_some() {
        for name in ["comp_min_ref", "has_comp_max_ref", "comp_max_ref"] {
            if n.get(name).is_none() {
                return err(
                    w,
                    format!("{name} missing from RGBA component reference group"),
                );
            }
        }
        w.bytes(&[1, 2, 66])?;
        w.bool(bool_field(n, "has_comp_min_ref", w)?)?;
        w.u8(72)?;
        w.u32(u32_field(n, "comp_min_ref", w)?)?;
        w.u8(66)?;
        w.bool(bool_field(n, "has_comp_max_ref", w)?)?;
        w.u8(72)?;
        w.u32(u32_field(n, "comp_max_ref", w)?)?;
    } else if ["comp_min_ref", "has_comp_max_ref", "comp_max_ref"]
        .iter()
        .any(|k| n.get(k).is_some())
    {
        return err(w, "incomplete RGBA component reference group");
    }
    if optional_pair(n, "alpha_min_ref", "alpha_max_ref", w)? {
        w.bytes(&[1, 3, 72])?;
        w.u32(u32_field(n, "alpha_min_ref", w)?)?;
        w.u8(72)?;
        w.u32(u32_field(n, "alpha_max_ref", w)?)?;
    }
    w.u8(3)
}

fn write_video(n: &Node, w: &mut Writer) -> Result<()> {
    write_did(n, w)?;
    match &n.class {
        b"DIDD" => Ok(()),
        b"RGBA" => write_rgba(n, w),
        b"CDCI" | b"MPGI" | b"JPED" => {
            write_cdci(n, w)?;
            match &n.class {
                b"CDCI" => Ok(()),
                b"MPGI" => write_mpgi(n, w),
                b"JPED" => write_jped(n, w),
                _ => unreachable!(),
            }
        }
        _ => unreachable!(),
    }
}

/// Serialize one `MDES`, `MDTP`, `MDFM`, `MDNG`, `MDFL` or `MULD` payload.
/// Unknown, duplicate, absent or out-of-range fields fail closed. Existing reference
/// numbers are preserved but cannot be remapped by this function. The decoded float
/// edit rate can lose its original mantissa/exponent representation; compare output
/// bytes and preserve the original payload when exact identity is required.
pub fn encode_descriptor_payload(n: &Node, big_endian: bool, limit: usize) -> Result<Vec<u8>> {
    let groups: &[&[&str]] = match &n.class {
        b"MDES" | b"MDFM" | b"MDNG" => &[],
        b"MDTP" => &[&["cframe"]],
        b"MDFL" => &[FILE],
        b"MULD" => &[FILE, &["descriptors"]],
        b"WAVE" => &[FILE, &["summary"]],
        b"AIFC" => &[FILE, &["summary", "data_pos"]],
        b"PCMA" => &[
            FILE,
            AUDIO,
            &[
                "block_align",
                "sequence_offset",
                "average_bps",
                "has_peak_envelope_data",
                "peak_envelope_version",
                "peak_envelope_format",
                "points_per_peak_value",
                "peak_envelope_block_size",
                "peak_channel_count",
                "peak_frame_count",
                "peak_of_peaks_offset",
                "peak_envelope_timestamp",
                "ebu_timestamp",
                "timecode_framerate",
            ],
        ],
        b"MPGA" => &[FILE, AUDIO, &["bit_rate", "sub_frame_alignment", "origin"]],
        b"DATD" => &[
            FILE,
            &[
                "is_offset_to_frame_indexes_valid",
                "offset_to_frame_indexes",
                "first_frame_offset",
                "min_sample_size",
                "max_sample_size",
            ],
        ],
        b"ANCD" => &[
            FILE,
            &[
                "is_offset_to_frame_indexes_valid",
                "offset_to_frame_indexes",
                "first_frame_offset",
                "min_sample_size",
                "max_sample_size",
                "manifest_element_count",
            ],
        ],
        b"DIDD" => &[FILE, DID],
        b"CDCI" => &[FILE, DID, CDCI],
        b"MPGI" => &[
            FILE,
            DID,
            CDCI,
            &[
                "mpeg_version",
                "profile",
                "gop_structure",
                "stream_type",
                "random_access",
                "leading_discard",
                "trailing_discard",
                "min_gop_length",
                "max_gop_length",
                "sequence_hdr",
            ],
        ],
        b"JPED" => &[
            FILE,
            DID,
            CDCI,
            &[
                "jpeg_table_id",
                "jpeg_frame_index_offset",
                "quantization_tables",
                "image_start_align",
            ],
        ],
        b"RGBA" => &[
            FILE,
            DID,
            &[
                "pixel_layout",
                "frame_index_offset",
                "has_comp_min_ref",
                "comp_min_ref",
                "has_comp_max_ref",
                "comp_max_ref",
                "alpha_min_ref",
                "alpha_max_ref",
            ],
        ],
        _ => {
            return Err(Error::new(
                format!("{} has no descriptor writer", n.class_str()),
                0,
            ))
        }
    };
    let mut seen = HashSet::new();
    for (name, _) in &n.fields {
        if !seen.insert(*name) {
            return Err(Error::new(format!("duplicate descriptor field {name}"), 0));
        }
        if !BASE.contains(name) && !groups.iter().any(|group| group.contains(name)) {
            return Err(Error::new(format!("unmapped descriptor field {name}"), 0));
        }
    }
    let mut w = Writer::with_limit(big_endian, limit);
    write_base(n, &mut w)?;
    if !matches!(&n.class, b"MDES" | b"MDTP" | b"MDFM" | b"MDNG") {
        write_file(n, &mut w)?;
    }
    match &n.class {
        b"MDES" => w.u8(3)?,
        b"MDTP" => {
            w.bytes(&[2, 2])?;
            w.s16(i16_field(n, "cframe", &w)?)?;
            w.u8(3)?;
        }
        b"MDFM" | b"MDNG" => w.bytes(&[2, 1, 3])?,
        b"MDFL" => {
            w.u8(3)?;
        }
        b"MULD" => {
            w.bytes(&[2, 1])?;
            let refs = match required(n, "descriptors", &w)? {
                Value::RefList(v) => v,
                _ => return err(&w, "descriptors is not RefList"),
            };
            w.s32(
                i32::try_from(refs.len())
                    .map_err(|_| Error::new("too many descriptors", w.len()))?,
            )?;
            for value in refs {
                w.object_ref_index(reference(value, &w)?)?;
            }
            w.u8(3)?;
        }
        b"WAVE" => {
            w.bytes(&[2, 1])?;
            w.bytes(b"RIFF")?;
            let summary = bytes_field(n, "summary", &w)?;
            w.bytes(
                &(u32::try_from(summary.len())
                    .map_err(|_| Error::new("summary too long", w.len()))?)
                .to_le_bytes(),
            )?;
            w.bytes(summary)?;
            w.u8(3)?;
        }
        b"AIFC" => {
            w.bytes(&[2, 1])?;
            w.bytes(b"FORM")?;
            let summary = bytes_field(n, "summary", &w)?;
            w.bytes(
                &(u32::try_from(summary.len())
                    .map_err(|_| Error::new("summary too long", w.len()))?)
                .to_be_bytes(),
            )?;
            w.bytes(summary)?;
            ext_i32(n, "data_pos", 1, &mut w)?;
            w.u8(3)?;
        }
        b"PCMA" => write_pcma(n, &mut w)?,
        b"MPGA" => write_mpga(n, &mut w)?,
        b"DATD" | b"ANCD" => write_data(n, &mut w)?,
        b"DIDD" | b"CDCI" | b"MPGI" | b"JPED" | b"RGBA" => write_video(n, &mut w)?,
        _ => unreachable!(),
    }
    Ok(w.into_bytes())
}

#[derive(Clone, Copy)]
struct DecimalField {
    offset: usize,
    value: f64,
}

/// Locate the two decimal-float fields by following the descriptor's bounded
/// base and file layout. Their original mantissa/exponent representation is
/// not retained in `Node`, even when its decoded floating-point value is.
fn decimal_fields(
    class: [u8; 4],
    payload: &[u8],
    big_endian: bool,
) -> Result<(DecimalField, Option<DecimalField>)> {
    let mut r = Reader::new(payload, 0, big_endian);
    r.assert_tag(2)?;
    r.assert_tag(3)?;
    r.u8()?;
    r.object_ref()?;
    r.bool()?;
    r.object_ref()?;
    while let Some(tag) = r.next_ext()? {
        match tag {
            1 | 2 => {
                r.assert_tag(65)?;
                let length = r.s32()?;
                if length < 0 {
                    return r.err("negative descriptor extension length");
                }
                r.bytes(length as usize)?;
            }
            3 => {
                r.assert_tag(72)?;
                r.object_ref()?;
            }
            _ => return r.err(format!("unknown descriptor base extension {tag}")),
        }
    }
    r.assert_tag(2)?;
    r.assert_tag(3)?;
    let edit_offset = r.offset();
    let edit = DecimalField {
        offset: edit_offset,
        value: r.exp10_float()?,
    };
    r.s32()?;
    r.s16()?;
    r.s32()?;
    let sample = if matches!(&class, b"PCMA" | b"MPGA") {
        r.assert_tag(2)?;
        r.assert_tag(1)?;
        r.u16()?;
        r.u16()?;
        let offset = r.offset();
        Some(DecimalField {
            offset,
            value: r.exp10_float()?,
        })
    } else {
        None
    };
    Ok((edit, sample))
}

fn preserve_decimal(
    output: &mut [u8],
    written: DecimalField,
    source: &[u8],
    original: DecimalField,
    source_big_endian: bool,
    target_big_endian: bool,
    current: Option<&Value>,
) -> Result<()> {
    if current != Some(&Value::Float(original.value)) {
        return Ok(());
    }
    let src = source
        .get(original.offset..original.offset + 6)
        .ok_or_else(|| Error::new("source decimal field truncated", original.offset))?;
    let dst = output
        .get_mut(written.offset..written.offset + 6)
        .ok_or_else(|| Error::new("written decimal field truncated", written.offset))?;
    if source_big_endian == target_big_endian {
        dst.copy_from_slice(src);
    } else {
        let mut r = Reader::new(src, original.offset, source_big_endian);
        let mantissa = r.s32()?;
        let exponent = r.s16()?;
        let mut w = Writer::with_limit(target_big_endian, 6);
        w.s32(mantissa)?;
        w.s16(exponent)?;
        dst.copy_from_slice(w.as_bytes());
    }
    Ok(())
}

/// Re-encode a descriptor while retaining a source object's decimal-float
/// representation whenever its semantic rate has not changed. This matters
/// for valid alternatives such as 48 × 10^3 versus 48000 × 10^0 and for
/// values that pinned py-avb's writer truncates on a second write.
pub fn encode_descriptor_payload_with_source(
    n: &Node,
    source: Option<(&[u8], bool)>,
    big_endian: bool,
    limit: usize,
) -> Result<Vec<u8>> {
    let mut output = encode_descriptor_payload(n, big_endian, limit)?;
    let Some((original, source_big_endian)) = source else {
        return Ok(output);
    };
    if matches!(&n.class, b"MDES" | b"MDTP" | b"MDFM" | b"MDNG") {
        return Ok(output);
    }
    let mut decoder = Decoder {
        r: Reader::new(original, 0, source_big_endian),
        num_objects: u32::MAX,
    };
    let source_node = decode(n.class, &mut decoder)?
        .ok_or_else(|| Error::new("source descriptor class is not decoded", 0))?;
    if decoder.r.remaining() != 0 {
        return Err(Error::new(
            "source descriptor has trailing bytes",
            decoder.r.offset(),
        ));
    }
    let (source_edit, source_sample) = decimal_fields(n.class, original, source_big_endian)?;
    let (written_edit, written_sample) = decimal_fields(n.class, &output, big_endian)?;
    if source_node.get("edit_rate") == n.get("edit_rate") {
        preserve_decimal(
            &mut output,
            written_edit,
            original,
            source_edit,
            source_big_endian,
            big_endian,
            n.get("edit_rate"),
        )?;
    }
    if source_node.get("sample_rate") == n.get("sample_rate") {
        if let (Some(a), Some(b)) = (source_sample, written_sample) {
            preserve_decimal(
                &mut output,
                b,
                original,
                a,
                source_big_endian,
                big_endian,
                n.get("sample_rate"),
            )?;
        }
    }
    Ok(output)
}
