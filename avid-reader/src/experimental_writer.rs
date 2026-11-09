//! Narrow byte-preserving AVB edit experiment. This is not a general writer.
//! The CMPO name location follows pyavb `components.py` (MIT; see NOTICE).

use crate::file::AvbIndex;
use crate::io::{Error, Reader, Result};
use std::ops::Range;

#[derive(Debug)]
pub struct SameLengthNamePatch {
    pub bytes: Vec<u8>,
    /// The only byte span the operation is permitted to change.
    pub changed_span: Range<usize>,
}

#[derive(Debug)]
pub struct RebuiltNamePatch {
    pub bytes: Vec<u8>,
    pub old_name_span: Range<usize>,
    pub new_name_span: Range<usize>,
    pub old_chunk_size: usize,
    pub new_chunk_size: usize,
}

fn printable_ascii_name(new_name: &str) -> bool {
    !new_name.is_empty() && new_name.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

fn cmpo_name_span(index: &AvbIndex, object_id: u32) -> Result<Range<usize>> {
    let object = index
        .object(object_id)
        .ok_or_else(|| Error::new(format!("object {object_id} is absent"), 0))?;
    if &object.class != b"CMPO" {
        return Err(Error::new(
            format!("object {object_id} is not CMPO"),
            object.chunk.start,
        ));
    }
    let payload = index
        .payload(object_id)
        .ok_or_else(|| Error::new("missing CMPO payload", object.payload.start))?;
    let mut r = Reader::new(payload, object.payload.start, index.big_endian);
    r.assert_tag(0x02)?;
    r.assert_tag(0x03)?;
    r.object_ref()?; // Component left_bob
    r.object_ref()?; // Component right_bob
    r.s16()?; // media_kind_id
    r.s32()?; // edit-rate mantissa
    r.s16()?; // edit-rate decimal exponent
    let len = r.u16()?;
    if len == 0 || len == 0xffff {
        return Err(Error::new("CMPO name is absent", r.offset()));
    }
    let start = r.offset();
    let old = r.bytes(len as usize)?;
    if old.contains(&0) {
        return Err(Error::new(
            "NUL-padded name is not supported by this experiment",
            start,
        ));
    }
    Ok(start..r.offset())
}

/// Replace an existing CMPO's inherited Component name with printable ASCII of the
/// same byte length. Object IDs, sizes, references, header and trailer remain exact.
/// The returned bytes are a candidate for a disposable Avid read-back, not a promise
/// that the Avid UI will use this field as its displayed sequence name.
pub fn patch_cmpo_name_same_length(
    index: &AvbIndex,
    object_id: u32,
    new_name: &str,
) -> Result<SameLengthNamePatch> {
    let span = cmpo_name_span(index, object_id)?;
    if !printable_ascii_name(new_name) {
        return Err(Error::new(
            "new name must be nonempty printable ASCII",
            span.start,
        ));
    }
    if span.len() != new_name.len() {
        return Err(Error::new("name has a different byte length", span.start));
    }
    let mut bytes = index.original_bytes().to_vec();
    bytes[span.clone()].copy_from_slice(new_name.as_bytes());
    Ok(SameLengthNamePatch {
        bytes,
        changed_span: span,
    })
}

/// Rebuild exactly one CMPO chunk after changing its inherited name length. Existing
/// object IDs and references are unchanged; later chunks shift by the byte delta.
/// This is a parser-level experiment only: unknown payloads might contain file offsets.
pub fn patch_cmpo_name_variable_length(
    index: &AvbIndex,
    object_id: u32,
    new_name: &str,
) -> Result<RebuiltNamePatch> {
    let name_span = cmpo_name_span(index, object_id)?;
    if !printable_ascii_name(new_name) || new_name.len() >= u16::MAX as usize {
        return Err(Error::new(
            "new name must be printable ASCII under 65535 bytes",
            name_span.start,
        ));
    }
    let object = index
        .object(object_id)
        .ok_or_else(|| Error::new("missing CMPO", 0))?;
    let old_chunk_size = object.payload.len();
    let new_chunk_size = old_chunk_size
        .checked_sub(name_span.len())
        .and_then(|n| n.checked_add(new_name.len()))
        .ok_or_else(|| Error::new("new CMPO size overflow", object.chunk.start))?;
    let chunk_size = u32::try_from(new_chunk_size)
        .map_err(|_| Error::new("new CMPO exceeds 32-bit chunk size", object.chunk.start))?;
    let len_field = name_span.start - 2..name_span.start;
    let size_field = object.chunk.start + 4..object.chunk.start + 8;
    let original = index.original_bytes();
    let mut bytes = Vec::with_capacity(original.len() - name_span.len() + new_name.len());
    bytes.extend_from_slice(&original[..size_field.start]);
    bytes.extend_from_slice(&if index.big_endian {
        chunk_size.to_be_bytes()
    } else {
        chunk_size.to_le_bytes()
    });
    bytes.extend_from_slice(&original[size_field.end..len_field.start]);
    let new_len = new_name.len() as u16;
    bytes.extend_from_slice(&if index.big_endian {
        new_len.to_be_bytes()
    } else {
        new_len.to_le_bytes()
    });
    let new_name_start = bytes.len();
    bytes.extend_from_slice(new_name.as_bytes());
    bytes.extend_from_slice(&original[name_span.end..]);
    Ok(RebuiltNamePatch {
        bytes,
        old_name_span: name_span,
        new_name_span: new_name_start..new_name_start + new_name.len(),
        old_chunk_size,
        new_chunk_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::AvbFile;

    fn one_cmpo(name: &[u8], big_endian: bool) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[2, 3]);
        payload.extend_from_slice(&[0; 8]);
        payload.extend_from_slice(&[0; 2]);
        payload.extend_from_slice(&[0; 6]);
        let len = name.len() as u16;
        payload.extend_from_slice(&if big_endian {
            len.to_be_bytes()
        } else {
            len.to_le_bytes()
        });
        payload.extend_from_slice(name);
        payload.extend_from_slice(b"untouched-tail");
        let mut h = Vec::new();
        h.extend_from_slice(if big_endian { b"\x00\x06" } else { b"\x06\x00" });
        h.extend_from_slice(b"Domain");
        h.extend_from_slice(if big_endian { b"OBJD" } else { b"DJBO" });
        h.extend_from_slice(if big_endian { b"\x00\x07" } else { b"\x07\x00" });
        h.extend_from_slice(b"AObjDoc");
        h.extend_from_slice(&[4, 0, 0]);
        let one = if big_endian {
            1u32.to_be_bytes()
        } else {
            1u32.to_le_bytes()
        };
        h.extend_from_slice(&one);
        h.extend_from_slice(&one);
        h.extend_from_slice(if big_endian { b"MMMM" } else { b"IIII" });
        h.extend_from_slice(&[0; 8]);
        h.extend_from_slice(if big_endian { b"ATobATve" } else { b"boTAevTA" });
        h.extend_from_slice(&[0, 0]);
        h.extend_from_slice(&[0; 16]);
        h.extend_from_slice(if big_endian { b"CMPO" } else { b"OPMC" });
        let size = payload.len() as u32;
        h.extend_from_slice(&if big_endian {
            size.to_be_bytes()
        } else {
            size.to_le_bytes()
        });
        h.extend_from_slice(&payload);
        h.extend_from_slice(b"trailer");
        h
    }

    #[test]
    fn same_length_patch_changes_only_the_parsed_name_span() {
        for big_endian in [false, true] {
            let original = one_cmpo(b"OLD NAME", big_endian);
            let index = AvbIndex::from_bytes(original.clone()).unwrap();
            let patched = patch_cmpo_name_same_length(&index, 1, "NEW NAME").unwrap();
            assert_eq!(patched.bytes.len(), original.len());
            assert_eq!(&patched.bytes[patched.changed_span.clone()], b"NEW NAME");
            for (i, (before, after)) in original.iter().zip(&patched.bytes).enumerate() {
                if !patched.changed_span.contains(&i) {
                    assert_eq!(before, after, "byte {i}");
                }
            }
            assert!(patch_cmpo_name_same_length(&index, 1, "LONGER NAME").is_err());
            assert!(patch_cmpo_name_same_length(&index, 1, "BAD\0NAME").is_err());
        }
    }

    #[test]
    fn rejects_wrong_object_or_truncated_name() {
        let mut bytes = one_cmpo(b"OLD", false);
        let index = AvbIndex::from_bytes(bytes.clone()).unwrap();
        assert!(patch_cmpo_name_same_length(&index, 0, "NEW").is_err());
        let class_start = index.object(1).unwrap().chunk.start;
        bytes[class_start..class_start + 4].copy_from_slice(b"LLIF");
        let wrong = AvbIndex::from_bytes(bytes).unwrap();
        assert!(patch_cmpo_name_same_length(&wrong, 1, "NEW").is_err());
        let mut short = one_cmpo(b"OLD", false);
        let name_start = index.object(1).unwrap().payload.start + 2 + 8 + 2 + 6;
        short[name_start..name_start + 2].copy_from_slice(&100u16.to_le_bytes());
        let short = AvbIndex::from_bytes(short).unwrap();
        assert!(patch_cmpo_name_same_length(&short, 1, &"A".repeat(100)).is_err());
    }

    #[test]
    fn variable_length_patch_preserves_object_ids_and_trailer_in_both_orders() {
        for big_endian in [false, true] {
            let original = one_cmpo(b"OLD", big_endian);
            let before = AvbIndex::from_bytes(original.clone()).unwrap();
            let patch = patch_cmpo_name_variable_length(&before, 1, "NEW SEQUENCE").unwrap();
            let after = AvbIndex::from_bytes(patch.bytes.clone()).unwrap();
            assert_eq!(after.object_count(), before.object_count());
            assert_eq!(after.root_index, before.root_index);
            assert_eq!(after.header_bytes(), before.header_bytes());
            assert_eq!(after.trailing_bytes(), before.trailing_bytes());
            assert_eq!(after.object(1).unwrap().class, *b"CMPO");
            assert_eq!(
                patch.new_chunk_size - patch.old_chunk_size,
                "NEW SEQUENCE".len() - 3
            );
            assert_eq!(&patch.bytes[patch.new_name_span], b"NEW SEQUENCE");
            assert_eq!(after.object(1).unwrap().payload.len(), patch.new_chunk_size);
            assert!(patch_cmpo_name_variable_length(&before, 1, "BAD\0NAME").is_err());
        }
    }

    /// Set PYAVB_TEST_FILES to pinned pyavb tests/test_files for a full-bin proof.
    #[test]
    fn pinned_full_bin_has_one_span_edit_and_decodes_again() {
        let Some(root) = std::env::var_os("PYAVB_TEST_FILES") else {
            return;
        };
        let path = std::path::PathBuf::from(root).join("retimes/linear01.avb");
        let original = std::fs::read(path).unwrap();
        let index = AvbIndex::from_bytes(original.clone()).unwrap();
        let before = AvbFile::parse(&original).unwrap();
        let old_name = before
            .node(169)
            .unwrap()
            .get("name")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(old_name.is_ascii());
        let new_name = "N".repeat(old_name.len());
        let patch = patch_cmpo_name_same_length(&index, 169, &new_name).unwrap();
        let after = AvbFile::parse(&patch.bytes).unwrap();
        assert_eq!(
            after.node(169).unwrap().get("name").unwrap().as_str(),
            Some(new_name.as_str())
        );
        let differences: Vec<_> = original
            .iter()
            .zip(&patch.bytes)
            .enumerate()
            .filter_map(|(i, (a, b))| (a != b).then_some(i))
            .collect();
        assert!(!differences.is_empty());
        assert!(differences.iter().all(|i| patch.changed_span.contains(i)));

        let longer_name = "A LONGER CODEX TEST NAME";
        let expanded = patch_cmpo_name_variable_length(&index, 169, longer_name).unwrap();
        let expanded_index = AvbIndex::from_bytes(expanded.bytes.clone()).unwrap();
        let expanded_file = AvbFile::parse(&expanded.bytes).unwrap();
        assert_eq!(
            expanded_file
                .node(169)
                .unwrap()
                .get("name")
                .unwrap()
                .as_str(),
            Some(longer_name)
        );
        assert_eq!(expanded_index.root_index, index.root_index);
        assert_eq!(expanded_index.object_count(), index.object_count());
        assert_eq!(expanded_index.header_bytes(), index.header_bytes());
        assert_eq!(expanded_index.trailing_bytes(), index.trailing_bytes());
        for object_id in 1..index.object_count() as u32 {
            if object_id != 169 {
                assert_eq!(
                    expanded_index.chunk(object_id),
                    index.chunk(object_id),
                    "object {object_id}"
                );
            }
        }
    }
}
