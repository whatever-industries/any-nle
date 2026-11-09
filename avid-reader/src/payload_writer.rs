//! Unified entry point for the bounded class-payload writer slices.
//! Produces bytes for existing object IDs; it is not a whole-file AVB writer.

use crate::component_writer::{source_only_from_payload, write_component, SourceOnly};
use crate::curve_writer::{source_only_from_payload as curve_source, write_curve, CurveSourceOnly};
use crate::io::{Error, Result};
use crate::objects::{decode, Decoder, Node};
use crate::writer_io::Writer;
use crate::{bin_writer, descriptor_writer, misc_extra_writer, misc_writer, trackgroup_writer};

/// Serialize a supported decoded object payload. The optional original payload
/// supplies fields discarded by the typed reader; omitting it makes such classes
/// fail rather than inventing unknown bytes. `max_object_id` checks bin refs.
pub fn encode_supported_payload(
    node: &Node,
    original_payload: Option<&[u8]>,
    big_endian: bool,
    max_object_id: Option<u32>,
    byte_limit: usize,
) -> Result<Vec<u8>> {
    let payload =
        match &node.class {
            b"COMP" | b"CLIP" | b"SCLP" | b"SEQU" | b"FILL" | b"TCCP" | b"TRKR" => {
                let source = match original_payload {
                    Some(bytes) => source_only_from_payload(node.class, bytes, big_endian)?,
                    None => {
                        // py-avb's factory omits a null param-list extension and
                        // initializes the two source-only spans on new objects.
                        let source_mob_words = match node.get("mob_id") {
                            Some(crate::objects::Value::MobId(id)) => Some([
                                id.material.data1,
                                (id.material.data2 as u32) | ((id.material.data3 as u32) << 16),
                            ]),
                            _ => None,
                        };
                        SourceOnly {
                            edit_rate_original: None,
                            param_list_present: Some(false),
                            source_mob_words,
                            timecode_reserved: Some([0; 6]),
                        }
                    }
                };
                write_component(node, big_endian, source, byte_limit)?
            }
            b"ABIN" | b"BINF" | b"ASET" | b"BVst" | b"ATTR" => {
                bin_writer::serialize(node, big_endian, byte_limit, max_object_id)?
            }
            b"MDES" | b"MDTP" | b"MDFM" | b"MDNG" | b"MDFL" | b"MULD" | b"WAVE" | b"AIFC"
            | b"PCMA" | b"MPGA" | b"DATD" | b"ANCD" | b"DIDD" | b"CDCI" | b"MPGI" | b"JPED"
            | b"RGBA" => descriptor_writer::encode_descriptor_payload_with_source(
                node,
                original_payload.map(|payload| (payload, big_endian)),
                big_endian,
                byte_limit,
            )?,
            b"RSET" | b"SLCT" | b"TKFX" | b"PVOL" | b"TNFX" | b"TRKG" | b"CMPO" | b"ASPI"
            | b"EQMB" | b"MASK" | b"STRB" | b"SPED" | b"REPT" | b"WARP" => {
                trackgroup_writer::encode_trackgroup_payload(
                    node,
                    original_payload.map(|bytes| (bytes, big_endian)),
                    big_endian,
                    byte_limit,
                )?
            }
            b"APOS" | b"ABOB" | b"DIDP" | b"MPGP" | b"MCBR" | b"MCMR" | b"TMBC" => {
                misc_writer::serialize(node, big_endian, byte_limit, max_object_id)?
            }
            b"FILE" | b"WINF" | b"MSML" | b"URLL" | b"CCFX" | b"GRFX" | b"SHLP" | b"TMCS"
            | b"PRLS" | b"TKMN" | b"TKDS" | b"TKPS" | b"TKDA" | b"TKPA" | b"AVUP" | b"PRIT"
            | b"FXPS" => misc_extra_writer::serialize(node, big_endian, byte_limit, max_object_id)?,
            b"PRCL" | b"CTRL" | b"ECCP" => {
                let source = match original_payload {
                    Some(bytes) => curve_source(node.class, bytes, big_endian)?,
                    None => CurveSourceOnly::default(),
                };
                write_curve(node, big_endian, source, byte_limit)?
            }
            _ => {
                return Err(Error::new(
                    format!("{} has no typed payload writer", node.class_str()),
                    0,
                ))
            }
        };
    // Check the known field semantics before returning a candidate chunk.
    let mut decoder = Decoder {
        r: crate::io::Reader::new(&payload, 0, big_endian),
        num_objects: max_object_id.unwrap_or(u32::MAX),
    };
    let roundtrip = decode(node.class, &mut decoder)?
        .ok_or_else(|| Error::new("writer class is not decoded", 0))?;
    if decoder.r.remaining() != 0 || roundtrip != *node {
        return Err(Error::new(
            "writer payload does not roundtrip to the input node",
            decoder.r.offset(),
        ));
    }
    Ok(payload)
}

/// Wrap one supported payload in class/size framing, ready for experimental
/// `id_stable_writer::assemble_existing` at an unchanged object ID.
pub fn encode_supported_chunk(
    node: &Node,
    original_payload: Option<&[u8]>,
    big_endian: bool,
    max_object_id: Option<u32>,
    byte_limit: usize,
) -> Result<Vec<u8>> {
    let payload = encode_supported_payload(
        node,
        original_payload,
        big_endian,
        max_object_id,
        byte_limit,
    )?;
    let size = u32::try_from(payload.len())
        .map_err(|_| Error::new("payload exceeds 32-bit AVB chunk", 0))?;
    let mut writer = Writer::with_limit(
        big_endian,
        byte_limit
            .checked_add(8)
            .ok_or_else(|| Error::new("chunk size limit overflow", 0))?,
    );
    writer.fourcc(node.class)?;
    writer.u32(size)?;
    writer.bytes(&payload)?;
    Ok(writer.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{AvbFile, AvbIndex};

    #[test]
    fn pinned_full_bin_abin_sclp_prcl_rebuild_exact_chunks() {
        let Ok(root) = std::env::var("PYAVB_TEST_FILES") else {
            return;
        };
        let path = std::path::Path::new(&root).join("test_file_01.avb");
        let bytes = std::fs::read(path).unwrap();
        let index = AvbIndex::from_bytes(bytes.clone()).unwrap();
        let decoded = AvbFile::parse(&bytes).unwrap();
        for class in [*b"ABIN", *b"SCLP", *b"PRCL"] {
            let id = (1..index.object_count() as u32)
                .find(|id| index.object(*id).unwrap().class == class)
                .unwrap();
            let node = decoded.node(id).unwrap();
            let payload = index.payload(id).unwrap();
            let chunk = encode_supported_chunk(
                node,
                Some(payload),
                index.big_endian,
                Some((index.object_count() - 1) as u32),
                payload.len() + 4096,
            )
            .unwrap();
            assert_eq!(
                chunk,
                index.chunk(id).unwrap(),
                "{} object {id}",
                node.class_str()
            );
        }
    }
}
