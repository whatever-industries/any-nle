//! Object-graph builders (Rust port Phase 2): py_premiere's template factories. Each function
//! returns detached element trees in a [`Tree`] that callers splice or attach, byte-identical to
//! the Python builder it ports. One module per Python source file:
//!
//! | Python | Rust |
//! |---|---|
//! | `models/skeleton.py` | [`skeleton`] |
//! | `models/sequence_builder.py` (+ converter `EXTRA_FORMATS`) | [`sequence`] |
//! | `models/media_import.py` `new_*` builders | [`media`] |
//! | `models/merged_builder.py` | [`merged`] |
//! | `models/multicam_builder.py` (+ fork `build_multicam_mono_audio`) | [`multicam`] |
//! | `models/marker.py` element + `data/marker_colors.py` | [`marker`] |
//! | number formatting (`models/component.py`, `models/track_item.py`) and `models/time.py` | [`fmt`], [`time`] |
//!
//! | `models/mask_builder.py` + `data/mask_template.py` | [`mask`], [`mask_template`] |
//! | `models/graphic_builder.py` + `data/text_template.py` (+ `caption.replace_payload_text`) | [`graphic`], [`crate::caption::flat`] |
//! | `models/mgt_builder.py` | [`mgt`] |
//!
//! Shared helpers below port `media_import._top`, `_leaf` and `_indexed_refs`.

use crate::xml::{NodeId, Tree};

pub mod fmt;
pub mod graphic;
pub mod mask;
pub mod mask_template;
pub mod marker;
pub mod media;
pub mod mgt;
pub mod merged;
pub mod multicam;
pub mod sequence;
pub mod skeleton;
pub mod time;

/// `media_import._top`: a top-level object element; `ObjectUID=uid` when given, else an empty
/// `ObjectID` placeholder (filled when registered), then `ClassID`, `Version`; text "\n\t\t".
pub fn top(tree: &mut Tree, tag: &str, class_id: &str, version: &str, uid: Option<&str>) -> NodeId {
    let id = match uid {
        Some(u) => tree.new_element_with(tag, &[("ObjectUID", u), ("ClassID", class_id), ("Version", version)]),
        None => tree.new_element_with(tag, &[("ObjectID", ""), ("ClassID", class_id), ("Version", version)]),
    };
    tree.set_text(id, Some("\n\t\t"));
    id
}

/// `media_import._leaf`: a child element with text and tail.
pub fn leaf(tree: &mut Tree, parent: NodeId, tag: &str, text: &str, tail: &str) -> NodeId {
    let id = tree.sub_element(parent, tag, &[]);
    tree.set_text(id, Some(text));
    tree.set_tail(id, Some(tail));
    id
}

/// `media_import._indexed_refs`: `<tag Index="i" ObjectRef="id"/>` entries; tails "\n\t\t\t",
/// last "\n\t\t".
pub fn indexed_refs(tree: &mut Tree, parent: NodeId, tag: &str, object_ids: &[&str]) {
    for (index, object_id) in object_ids.iter().enumerate() {
        let i = index.to_string();
        let entry = tree.sub_element(parent, tag, &[("Index", &i), ("ObjectRef", object_id)]);
        let tail = if index + 1 < object_ids.len() { "\n\t\t\t" } else { "\n\t\t" };
        tree.set_tail(entry, Some(tail));
    }
}
