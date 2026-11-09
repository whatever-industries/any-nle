//! Store-once binary payloads (port of `binary_hash` / `set_payload` from
//! `src/avid2prproj/lumetri.py`).
//!
//! Premiere keeps each `BinaryHash`'s bytes in ONE element; every other use is
//! an empty element with the same hash (see
//! [`PremiereDocument::payload`](crate::xml::PremiereDocument::payload)).

use crate::base64::b64encode;
use crate::hash::{hex, md5};
use crate::pystr::py_strip;
use crate::xml::{NodeId, PremiereDocument};

/// A `BinaryHash` shaped like Premiere's own: the first 80 bits of the MD5
/// hex digest, `0000`, then the payload length + 12 as 8 hex digits
/// (formatted as a UUID). For payloads of 2^32 - 12 bytes or more the last 12
/// digits are the low 48 bits of length + 12 instead (Python's fallback).
pub fn binary_hash(payload: &[u8]) -> String {
    let digest = hex(&md5(payload));
    let size = payload.len() as u64 + 12;
    let h = if size < 1u64 << 32 {
        format!("{}0000{:08x}", &digest[..20], size)
    } else {
        let wide = format!("{size:012x}");
        format!("{}{}", &digest[..20], &wide[wide.len() - 12..])
    };
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Python `set_payload`: give an arbitrary-data element a new payload
/// without breaking the store-once rule. If `value` is the stored copy of its
/// old hash, its text (verbatim) first moves to the first other element with
/// the same tag and old hash whose text is empty. Then `BinaryHash` is set
/// (in place, or appended) and the text becomes the base64 plus `"\n\t\t"`.
pub fn set_payload(document: &mut PremiereDocument, value: NodeId, payload: &[u8]) {
    let tree = &document.tree;
    let old_hash = tree.get(value, "BinaryHash").map(str::to_string);
    let has_text = !py_strip(tree[value].text_or_empty()).is_empty();
    if let (Some(old_hash), true) = (old_hash.filter(|h| !h.is_empty()), has_text) {
        let tag = tree.tag(value);
        let heir = tree.iter_tag(document.root, tag).find(|&other| {
            other != value
                && tree.get(other, "BinaryHash") == Some(old_hash.as_str())
                && py_strip(tree[other].text_or_empty()).is_empty()
        });
        if let Some(other) = heir {
            let text = document.tree[value].text.clone();
            document.tree[other].text = text;
        }
    }
    document.tree.set(value, "BinaryHash", binary_hash(payload));
    document.tree[value].text = Some(b64encode(payload) + "\n\t\t");
    document.invalidate_payloads();
}
