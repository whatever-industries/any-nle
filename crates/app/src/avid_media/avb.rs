//! Descriptor verification shared by linked and managed bin writers.
use super::Result;
use avid_reader::{
    descriptor_writer::encode_descriptor_payload,
    io::Reader,
    objects::{decode, Decoder, Node},
};

/// Use the typed codec to put descriptor fields in the AVB writer's expected
/// order and confirm every supplied field survived encoding.
pub(super) fn canonical_descriptor(
    descriptor: &Node,
    big_endian: bool,
    num_objects: u32,
    missing: &'static str,
    mismatch: &'static str,
) -> Result<Node> {
    let payload = encode_descriptor_payload(descriptor, big_endian, 4096)?;
    let mut decoder = Decoder {
        r: Reader::new(&payload, 0, big_endian),
        num_objects,
    };
    let saved = decode(descriptor.class, &mut decoder)?.ok_or(missing)?;
    if decoder.r.remaining() != 0
        || saved.fields.len() != descriptor.fields.len()
        || descriptor
            .fields
            .iter()
            .any(|(name, value)| saved.get(name) != Some(value))
    {
        return Err(mismatch.into());
    }
    Ok(saved)
}
