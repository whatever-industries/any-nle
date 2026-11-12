use super::super::{encoding::hex_bytes, xml, Node};
use crate::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

const MAX_RANGES: usize = 64;
const PREVIEW_BYTES: usize = 16;

pub(super) fn compare(
    left: &Path,
    left_id: &str,
    right: &Path,
    right_id: &str,
    field: &str,
) -> Result<Value> {
    if !matches!(field, "FieldsBlob" | "EffectFiltersBA") {
        return Err("expected FieldsBlob or EffectFiltersBA".into());
    }
    let (left_class, a) = read(left, left_id, field)?;
    let (right_class, b) = read(right, right_id, field)?;
    let (ranges, truncated) = changed_ranges(
        a.as_deref().unwrap_or_default(),
        b.as_deref().unwrap_or_default(),
    );
    Ok(json!({
        "schema":"anynle.resolve-blob-diff/1",
        "status":if a == b {"match"} else {"different"},
        "field":field, "offset_basis":"decoded_saved_bytes",
        "left":description(left_id, left_class, a.as_deref()),
        "right":description(right_id, right_class, b.as_deref()),
        "length_changed":a.as_deref().unwrap_or_default().len() != b.as_deref().unwrap_or_default().len(),
        "ranges":ranges, "ranges_truncated":truncated,
    }))
}

fn read(path: &Path, id: &str, field: &str) -> Result<(String, Option<Vec<u8>>)> {
    super::check_id(id)?;
    let members = xml::read(path)?;
    let mut found = None;
    for root in members.values() {
        find(root, id, &mut found)?;
    }
    let node = found.ok_or_else(|| format!("Resolve object {id} was not found"))?;
    Ok((
        node.name.clone(),
        node.maybe(field)?.map(hex_bytes).transpose()?,
    ))
}

fn find<'a>(node: &'a Node, id: &str, found: &mut Option<&'a Node>) -> Result<()> {
    if node.id.as_deref() == Some(id) && found.replace(node).is_some() {
        return Err(format!("Resolve object {id} is ambiguous").into());
    }
    for child in &node.children {
        find(child, id, found)?;
    }
    Ok(())
}

fn description(id: &str, class: String, bytes: Option<&[u8]>) -> Value {
    json!({"object_id":id, "class":class, "present":bytes.is_some(),
        "size":bytes.map(<[u8]>::len), "sha256":bytes.map(|bytes| hex::encode(Sha256::digest(bytes)))})
}

fn changed_ranges(a: &[u8], b: &[u8]) -> (Vec<Value>, bool) {
    let limit = a.len().max(b.len());
    let differs = |offset| a.get(offset) != b.get(offset);
    let mut ranges = Vec::new();
    let mut pos = 0;
    while pos < limit {
        if !differs(pos) {
            pos += 1;
            continue;
        }
        if ranges.len() == MAX_RANGES {
            return (ranges, true);
        }
        let start = pos;
        while pos < limit && differs(pos) {
            pos += 1;
        }
        let preview = |bytes: &[u8]| {
            let first = start.min(bytes.len());
            let end = pos.min(bytes.len());
            json!({"length":end-first,
                "preview_hex":hex::encode(&bytes[first..end.min(first+PREVIEW_BYTES)])})
        };
        ranges.push(json!({"offset":start, "length":pos-start,
            "left":preview(a), "right":preview(b)}));
    }
    (ranges, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_ranges_keep_tail_and_flag_actual_truncation() {
        let (ranges, truncated) = changed_ranges(&[0, 0, 0], &[0, 1, 0, 2]);
        assert!(!truncated);
        assert_eq!(ranges[0]["offset"], 1);
        assert_eq!(ranges[1]["offset"], 3);
        assert_eq!(ranges[1]["left"]["length"], 0);
        assert_eq!(ranges[1]["right"]["preview_hex"], "02");
        let alternating: Vec<_> = (0..128).map(|i| i % 2).collect();
        assert!(!changed_ranges(&[0; 128], &alternating).1);
        let alternating: Vec<_> = (0..130).map(|i| i % 2).collect();
        assert!(changed_ranges(&[0; 130], &alternating).1);
    }
}
