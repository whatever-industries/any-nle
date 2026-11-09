//! AVB container: header, object table, decoded objects.
//! Header layout from pyavb `file.py` (MIT, Mark Reid; see NOTICE).

use crate::io::{Error, Reader, Result};
use crate::objects::{decode, Decoder, Node};
use std::ops::Range;

/// Bins are usually kilobytes to megabytes, but the KB has 315 MB ones; refuse anything absurd.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum Object {
    Decoded(Node),
    /// A class this port does not decode; kept so references to it can be reported.
    Unsupported { class: [u8; 4], size: usize },
}

#[derive(Debug)]
pub struct AvbFile {
    pub big_endian: bool,
    pub last_save: String,
    pub last_save_time: u32,
    pub creator_version: String,
    pub root_index: u32,
    /// Index 0 is the null reference; objects are 1..=n.
    pub objects: Vec<Option<Object>>,
    /// Undecoded payload bytes of the classes in [`RAW_CLASSES`], by object index.
    raw: std::collections::HashMap<u32, Vec<u8>>,
}

/// The original AVB bytes and bounded object locations, without decoding or copying payloads.
/// This is an extraction/preservation primitive; it does not make structural edits safe.
#[derive(Debug)]
pub struct AvbIndex {
    bytes: Vec<u8>,
    header_end: usize,
    objects_end: usize,
    pub big_endian: bool,
    pub last_save: String,
    pub last_save_time: u32,
    pub creator_version: String,
    pub root_index: u32,
    /// Index 0 is the null reference; objects are 1..=n.
    objects: Vec<Option<IndexedObject>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedObject {
    pub class: [u8; 4],
    /// Exact on-disk bytes, including the 8-byte class/size header.
    pub chunk: Range<usize>,
    pub payload: Range<usize>,
}

#[derive(Debug)]
struct Header {
    big_endian: bool,
    last_save: String,
    last_save_time: u32,
    creator_version: String,
    num_objects: u32,
    root_index: u32,
}

/// Classes whose payload bytes are kept for bounded readers outside the pyavb-shaped decoder
/// (AniMatte shapes FXPS, tracker TKMN/TKPS/TKDS/TKPA/TKDA, parameter curves PRCL).
pub const RAW_CLASSES: &[&[u8; 4]] = &[b"FXPS", b"TKMN", b"TKPS", b"TKDS", b"TKPA", b"TKDA", b"PRCL"];

fn read_header(r: &mut Reader<'_>) -> Result<Header> {
    let big_endian = r.big_endian;
    r.bytes(2)?;
    if r.bytes(6)? != b"Domain" {
        return r.err("not an AVB file (magic)");
    }
    if &r.fourcc()? != b"OBJD" {
        return r.err("expected OBJD");
    }
    if r.string()? != "AObjDoc" {
        return r.err("expected AObjDoc");
    }
    r.assert_tag(0x04)?;
    let last_save = r.string()?;
    let num_objects = r.u32()?;
    let root_index = r.u32()?;
    let order = r.u32()?;
    if order != 0x4949_4949 && order != 0x4D4D_4D4D {
        return r.err("bad byte-order marker");
    }
    let last_save_time = r.u32()?;
    r.bytes(4)?;
    if &r.fourcc()? != b"ATob" || &r.fourcc()? != b"ATve" {
        return r.err("expected ATob/ATve");
    }
    let creator_version = r.string()?;
    r.bytes(16)?;

    if num_objects as usize > r.remaining() / 8 {
        return r.err(format!("{} objects cannot fit in {} bytes", num_objects, r.remaining()));
    }
    if root_index == 0 || root_index > num_objects {
        return r.err(format!("root index {} out of range", root_index));
    }
    Ok(Header { big_endian, last_save, last_save_time, creator_version, num_objects, root_index })
}

fn reader_for_avb(data: &[u8]) -> Result<Reader<'_>> {
    if data.len() as u64 > MAX_FILE_BYTES {
        return Err(Error::new("file too large", 0));
    }
    let big_endian = match data.get(0..2) {
        Some([0x06, 0x00]) => false,
        Some([0x00, 0x06]) => true,
        _ => return Err(Error::new("not an AVB file (byte-order mark)", 0)),
    };
    Ok(Reader::new(data, 0, big_endian))
}

impl AvbIndex {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let bytes = crate::file_io::read_avb(path)?;
        Self::from_bytes(bytes)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let mut r = reader_for_avb(&bytes)?;
        let h = read_header(&mut r)?;
        let header_end = r.offset();
        // Do not allocate from a malformed count before validating each actual chunk.
        let mut objects = Vec::new();
        objects.push(None);
        for _ in 0..h.num_objects {
            let chunk_start = r.offset();
            let class = r.fourcc()?;
            let size = r.u32()? as usize;
            let payload_start = r.offset();
            r.bytes(size)?;
            objects.push(Some(IndexedObject {
                class,
                chunk: chunk_start..r.offset(),
                payload: payload_start..r.offset(),
            }));
        }
        let objects_end = r.offset();
        Ok(Self {
            bytes,
            header_end,
            objects_end,
            big_endian: h.big_endian,
            last_save: h.last_save,
            last_save_time: h.last_save_time,
            creator_version: h.creator_version,
            root_index: h.root_index,
            objects,
        })
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn object(&self, index: u32) -> Option<&IndexedObject> {
        self.objects.get(index as usize)?.as_ref()
    }

    pub fn chunk(&self, index: u32) -> Option<&[u8]> {
        Some(&self.bytes[self.object(index)?.chunk.clone()])
    }

    pub fn payload(&self, index: u32) -> Option<&[u8]> {
        Some(&self.bytes[self.object(index)?.payload.clone()])
    }

    /// Decode just one indexed object. Unknown classes return `Ok(None)`;
    /// the index and all other object bytes stay untouched. This does not cache
    /// the result or resolve references like pyavb's lazy object wrappers.
    pub fn decode_object(&self, index: u32) -> Result<Option<Node>> {
        let object = self.object(index).ok_or_else(|| Error::new(format!("object index {} out of range", index), 0))?;
        let payload = &self.bytes[object.payload.clone()];
        let mut d = Decoder {
            r: Reader::new(payload, object.payload.start, self.big_endian),
            num_objects: (self.objects.len() - 1) as u32,
        };
        let decoded = decode(object.class, &mut d)?;
        if decoded.is_some() && d.r.remaining() != 0 {
            return Err(Error::new(format!("object {} ({}): {} unread bytes", index, String::from_utf8_lossy(&object.class), d.r.remaining()), d.r.offset()));
        }
        Ok(decoded)
    }

    /// Includes header and any trailing bytes, unchanged. This is not an AVB writer.
    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Return the owned input buffer without copying it or retaining the index.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn header_bytes(&self) -> &[u8] {
        &self.bytes[..self.header_end]
    }

    /// Bytes after the declared object table, if any. Their meaning is not assumed.
    pub fn trailing_bytes(&self) -> &[u8] {
        &self.bytes[self.objects_end..]
    }
}

// Both retained parsing and discard-after-check validation use this exact decoder loop.
fn read_objects(
    r: &mut Reader<'_>,
    h: &Header,
    mut visit: impl FnMut(u32, Object, &[u8]),
) -> Result<()> {
    for index in 1..=h.num_objects {
        let class = r.fourcc()?;
        let size = r.u32()? as usize;
        let at = r.offset();
        let payload = r.bytes(size)?;
        let mut d = Decoder { r: Reader::new(payload, at, h.big_endian), num_objects: h.num_objects };
        let decoded = decode(class, &mut d).map_err(|e| {
            Error::new(format!("object {} ({}): {}", index, String::from_utf8_lossy(&class), e.message), e.offset)
        })?;
        let object = match decoded {
            Some(node) => {
                if d.r.remaining() != 0 {
                    return Err(Error::new(
                        format!("object {} ({}): {} unread bytes", index, String::from_utf8_lossy(&class), d.r.remaining()),
                        d.r.offset(),
                    ));
                }
                Object::Decoded(node)
            }
            None => Object::Unsupported { class, size },
        };
        visit(index, object, payload);
    }
    Ok(())
}

impl AvbFile {
    pub fn open(path: &std::path::Path) -> Result<AvbFile> {
        let data = crate::file_io::read_avb(path)?;
        AvbFile::parse(&data)
    }

    pub fn parse(data: &[u8]) -> Result<AvbFile> {
        let mut r = reader_for_avb(data)?;
        let h = read_header(&mut r)?;
        // Reserve only the null slot before any object has passed decoding. A
        // plausible but corrupt count must not trigger a whole-table allocation.
        let mut objects = Vec::with_capacity(1);
        let mut raw = std::collections::HashMap::new();
        objects.push(None);
        read_objects(&mut r, &h, |index, object, payload| {
            let class = match &object {
                Object::Decoded(node) => node.class,
                Object::Unsupported { class, .. } => *class,
            };
            if RAW_CLASSES.contains(&&class) {
                raw.insert(index, payload.to_vec());
            }
            if objects.len() == objects.capacity() {
                // Grow with accepted objects, capped at the declared final size.
                let capacity = objects.capacity().saturating_mul(2)
                    .min(h.num_objects as usize + 1);
                objects.reserve_exact(capacity - objects.len());
            }
            objects.push(Some(object));
        })?;
        Ok(AvbFile {
            big_endian: h.big_endian,
            last_save: h.last_save,
            last_save_time: h.last_save_time,
            creator_version: h.creator_version,
            root_index: h.root_index,
            objects,
            raw,
        })
    }

    /// Perform the normal full-file decoding checks without retaining a second
    /// decoded graph or raw-payload copies. Used only after document encoding.
    pub(crate) fn validate(data: &[u8]) -> Result<()> {
        Self::decode_each(data, drop)
    }

    /// Move each decoded/unsupported object directly to its consumer, in ID order,
    /// without an intermediate object table or raw-payload copies. The consumer
    /// must discard partial state if a later object fails normal decoding checks.
    pub(crate) fn decode_each(data: &[u8], mut visit: impl FnMut(Object)) -> Result<()> {
        let mut r = reader_for_avb(data)?;
        let h = read_header(&mut r)?;
        read_objects(&mut r, &h, |_, object, _| visit(object))
    }

    pub fn get(&self, index: u32) -> Option<&Object> {
        self.objects.get(index as usize).and_then(|o| o.as_ref())
    }

    /// Number of slots in the object table (index 0 is unused).
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn node(&self, index: u32) -> Option<&Node> {
        match self.get(index) {
            Some(Object::Decoded(n)) => Some(n),
            _ => None,
        }
    }

    /// Class and payload bytes of an object whose class is in [`RAW_CLASSES`].
    pub fn raw(&self, index: u32) -> Option<([u8; 4], &[u8])> {
        let payload = self.raw.get(&index)?;
        let class = match self.get(index)? {
            Object::Decoded(n) => n.class,
            Object::Unsupported { class, .. } => *class,
        };
        Some((class, payload))
    }

    pub fn root(&self) -> Option<&Node> {
        self.node(self.root_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_unknown_object(big_endian: bool) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(if big_endian { b"\x00\x06" } else { b"\x06\x00" });
        h.extend_from_slice(b"Domain");
        h.extend_from_slice(if big_endian { b"OBJD" } else { b"DJBO" });
        h.extend_from_slice(if big_endian { b"\x00\x07" } else { b"\x07\x00" });
        h.extend_from_slice(b"AObjDoc");
        h.push(4);
        h.extend_from_slice(&[0, 0]);
        let one = if big_endian { 1u32.to_be_bytes() } else { 1u32.to_le_bytes() };
        h.extend_from_slice(&one);
        h.extend_from_slice(&one);
        h.extend_from_slice(if big_endian { b"MMMM" } else { b"IIII" });
        h.extend_from_slice(&[0; 8]);
        h.extend_from_slice(if big_endian { b"ATobATve" } else { b"boTAevTA" });
        h.extend_from_slice(&[0, 0]);
        h.extend_from_slice(&[0; 16]);
        h.extend_from_slice(b"ZZZZ");
        let three = if big_endian { 3u32.to_be_bytes() } else { 3u32.to_le_bytes() };
        h.extend_from_slice(&three);
        h.extend_from_slice(b"\x00\xff\x01");
        h
    }

    fn assert_validation_parity(bytes: &[u8]) {
        let retained = AvbFile::parse(bytes).map(|_| ()).map_err(|e| (e.message, e.offset));
        let discarded = AvbFile::validate(bytes).map_err(|e| (e.message, e.offset));
        assert_eq!(discarded, retained);
    }

    #[test]
    fn discarded_validation_matches_parsing_for_corrupt_and_unknown_objects() {
        for big_endian in [false, true] {
            let mut unknown = one_unknown_object(big_endian);
            unknown.extend_from_slice(b"trailer");
            let mut doc = crate::AvbDocument::new_empty_bin(19, "Validation", 100, big_endian);
            doc.create_filler(25.0, 1, 20).unwrap();
            let known = doc.to_bytes().unwrap();
            // Include a typed class whose raw bytes normal extraction must retain.
            let index = AvbIndex::from_bytes(unknown.clone()).unwrap();
            let mut raw = index.header_bytes().to_vec();
            raw.extend_from_slice(if big_endian { b"TKMN" } else { b"NMKT" });
            let payload = [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 3];
            raw.extend_from_slice(&if big_endian { 11u32.to_be_bytes() } else { 11u32.to_le_bytes() });
            raw.extend_from_slice(&payload);
            assert_eq!(AvbFile::parse(&raw).unwrap().raw(1), Some((*b"TKMN", payload.as_slice())));
            for bytes in [known, unknown, raw] {
                AvbFile::validate(&bytes).unwrap();
                assert_validation_parity(&bytes);
                for cut in 0..bytes.len() {
                    assert_validation_parity(&bytes[..cut]);
                }
                for position in 0..bytes.len() {
                    let mut changed = bytes.clone();
                    changed[position] ^= 0xff;
                    assert_validation_parity(&changed);
                }
            }
        }
    }

    #[test]
    fn discarded_validation_matches_pinned_bin_corpus() {
        let Some(root) = std::env::var_os("PYAVB_TEST_FILES") else { return };
        let mut paths = vec![std::path::PathBuf::from(root)];
        let mut checked = 0;
        while let Some(path) = paths.pop() {
            if path.is_dir() {
                paths.extend(std::fs::read_dir(path).unwrap().map(|e| e.unwrap().path()));
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("avb")) {
                let bytes = std::fs::read(path).unwrap();
                AvbFile::validate(&bytes).unwrap();
                assert_validation_parity(&bytes);
                // Sample malformed payloads and truncations beyond the synthetic cases.
                for position in (0..bytes.len()).step_by((bytes.len() / 32).max(1)) {
                    assert_validation_parity(&bytes[..position]);
                    let mut changed = bytes.clone();
                    changed[position] ^= 0xff;
                    assert_validation_parity(&changed);
                }
                checked += 1;
            }
        }
        assert!(checked > 0, "configured bin corpus must not be empty");
        eprintln!("discarded-validation parity: {checked} pinned bins");
    }

    #[test]
    fn index_preserves_exact_unknown_chunk_in_both_byte_orders() {
        for big_endian in [false, true] {
            let mut bytes = one_unknown_object(big_endian);
            let expected_chunk = bytes[bytes.len() - 11..].to_vec();
            bytes.extend_from_slice(b"trailer");
            let index = AvbIndex::from_bytes(bytes.clone()).unwrap();
            let decoded = AvbFile::parse(&bytes).unwrap();
            assert_eq!(index.big_endian, big_endian);
            assert_eq!(index.root_index, decoded.root_index);
            assert_eq!(index.object_count(), decoded.object_count());
            assert_eq!(index.object(1).unwrap().class, *b"ZZZZ");
            assert_eq!(index.chunk(1).unwrap(), expected_chunk);
            assert_eq!(index.payload(1).unwrap(), b"\x00\xff\x01");
            assert_eq!(index.original_bytes(), bytes);
            assert_eq!(index.header_bytes().len() + index.chunk(1).unwrap().len() + index.trailing_bytes().len(), bytes.len());
            assert_eq!(index.trailing_bytes(), b"trailer");
            assert!(index.object(0).is_none());
            assert!(index.payload(2).is_none());
        }
    }

    #[test]
    fn typed_tracker_manager_still_retains_raw_payload() {
        let mut bytes = one_unknown_object(false);
        bytes.truncate(bytes.len() - 11);
        let payload = [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 3];
        bytes.extend_from_slice(b"NMKT");
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let file = AvbFile::parse(&bytes).unwrap();
        assert!(matches!(file.get(1), Some(Object::Decoded(node)) if node.class == *b"TKMN"));
        assert_eq!(file.raw(1), Some((*b"TKMN", payload.as_slice())));
    }

    #[test]
    fn index_rejects_every_truncated_chunk_prefix() {
        let bytes = one_unknown_object(false);
        for cut in bytes.len() - 10..bytes.len() {
            assert!(AvbIndex::from_bytes(bytes[..cut].to_vec()).is_err(), "cut {cut}");
        }
        let index = AvbIndex::from_bytes(bytes).unwrap();
        assert_eq!(index.object_count(), 2);
    }

    #[test]
    fn rejects_non_avb() {
        assert!(AvbFile::parse(b"").is_err());
        assert!(AvbFile::parse(b"\x06\x00Damain").is_err());
    }

    #[test]
    fn every_truncation_of_a_header_fails_cleanly() {
        let mut h = Vec::new();
        h.extend_from_slice(b"\x06\x00Domain");
        h.extend_from_slice(b"DJBO");
        h.extend_from_slice(&[7, 0]);
        h.extend_from_slice(b"AObjDoc");
        h.push(4);
        h.extend_from_slice(&[0, 0]);
        h.extend_from_slice(&1u32.to_le_bytes());
        h.extend_from_slice(&1u32.to_le_bytes());
        h.extend_from_slice(b"IIII");
        h.extend_from_slice(&[0; 8]);
        h.extend_from_slice(b"boTAevTA");
        h.extend_from_slice(&[0, 0]);
        h.extend_from_slice(&[0; 16]);
        h.extend_from_slice(b"LLIF");
        h.extend_from_slice(&0u32.to_le_bytes());
        for n in 0..h.len() {
            assert!(AvbFile::parse(&h[..n]).is_err(), "prefix {}", n);
        }
        // Complete file: one FILL with an empty (and therefore truncated) payload.
        let e = AvbFile::parse(&h).unwrap_err();
        assert!(e.message.contains("object 1 (FILL)"), "{}", e);
    }
}
