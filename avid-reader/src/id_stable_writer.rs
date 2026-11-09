//! Experimental ID-stable assembly of already-serialized object chunks.
//!
//! No object is renumbered. This preserves opaque bytes, but a size change can
//! still invalidate unknown absolute file offsets. Avid read-back is required.

use crate::file::{AvbFile, AvbIndex, MAX_FILE_BYTES};
use crate::io::{Error, Result};
use std::collections::BTreeMap;

/// Build a candidate bin from complete replacement chunks keyed by existing ID.
/// The class and declared chunk length must agree with the original object.
/// This cannot append objects or update reference IDs.
pub fn assemble_existing(
    index: &AvbIndex,
    replacements: &BTreeMap<u32, Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut total = index.header_bytes().len() + index.trailing_bytes().len();
    for id in 1..index.object_count() as u32 {
        let bytes = replacements
            .get(&id)
            .map(Vec::as_slice)
            .or_else(|| index.chunk(id))
            .ok_or_else(|| Error::new(format!("missing object {id}"), 0))?;
        let original = index
            .chunk(id)
            .ok_or_else(|| Error::new(format!("missing original object {id}"), 0))?;
        if bytes.len() < 8 || bytes[..4] != original[..4] {
            return Err(Error::new(
                format!("object {id}: class or chunk header mismatch"),
                0,
            ));
        }
        let declared = if index.big_endian {
            u32::from_be_bytes(bytes[4..8].try_into().unwrap())
        } else {
            u32::from_le_bytes(bytes[4..8].try_into().unwrap())
        } as usize;
        if declared != bytes.len() - 8 {
            return Err(Error::new(
                format!("object {id}: declared payload size mismatch"),
                0,
            ));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| Error::new("assembled file size overflow", 0))?;
    }
    for id in replacements.keys() {
        if *id == 0 || *id as usize >= index.object_count() {
            return Err(Error::new(
                format!("replacement object {id} is out of range"),
                0,
            ));
        }
    }
    if total as u64 > MAX_FILE_BYTES {
        return Err(Error::new("assembled file exceeds AVB size limit", 0));
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(index.header_bytes());
    for id in 1..index.object_count() as u32 {
        bytes.extend_from_slice(
            replacements
                .get(&id)
                .map(Vec::as_slice)
                .unwrap_or_else(|| index.chunk(id).unwrap()),
        );
    }
    bytes.extend_from_slice(index.trailing_bytes());
    // Check the reconstructed table and all currently decoded classes. This is
    // parser acceptance, never proof that Media Composer accepts the result.
    AvbIndex::from_bytes(bytes.clone())?;
    AvbFile::parse(&bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_full_bin_reassembles_byte_exact_and_rejects_bad_chunks() {
        let Ok(root) = std::env::var("PYAVB_TEST_FILES") else {
            return;
        };
        let input =
            std::fs::read(std::path::Path::new(&root).join("retimes/linear01.avb")).unwrap();
        let index = AvbIndex::from_bytes(input.clone()).unwrap();
        assert_eq!(assemble_existing(&index, &BTreeMap::new()).unwrap(), input);
        let renamed = crate::experimental_writer::patch_cmpo_name_variable_length(
            &index,
            169,
            "A LONGER CODEX TEST NAME",
        )
        .unwrap();
        let renamed_index = AvbIndex::from_bytes(renamed.bytes.clone()).unwrap();
        let mut replacements = BTreeMap::new();
        replacements.insert(169, renamed_index.chunk(169).unwrap().to_vec());
        assert_eq!(
            assemble_existing(&index, &replacements).unwrap(),
            renamed.bytes
        );
        let mut chunks = BTreeMap::new();
        chunks.insert(169, index.chunk(169).unwrap().to_vec());
        chunks.get_mut(&169).unwrap()[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(assemble_existing(&index, &chunks).is_err());
        chunks.get_mut(&169).unwrap()[..4].copy_from_slice(b"LLIF");
        assert!(assemble_existing(&index, &chunks).is_err());
        chunks.clear();
        chunks.insert(0, vec![0; 8]);
        assert!(assemble_existing(&index, &chunks).is_err());
    }
}
