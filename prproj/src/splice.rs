//! Splicing builder fragments into a document (port of
//! `py_premiere/models/project.py` `Project._splice_fragment`).

use std::collections::HashMap;

use crate::error::Result;
use crate::ids::UuidSource;
use crate::xml::{NodeId, PremiereDocument, Tree};

/// The spliced top-level elements, keyed by tag.
///
/// Mirrors Python's returned dict: [`get`](Self::get) gives the LAST element
/// spliced with that tag; [`elements`](Self::elements) keeps every spliced
/// element in order.
#[derive(Debug, Clone, Default)]
pub struct Spliced {
    /// Every spliced element, in fragment order.
    pub elements: Vec<NodeId>,
    by_tag: HashMap<String, NodeId>,
}

impl Spliced {
    /// The last spliced element with this tag (Python `spliced[tag]` / `.get(tag)`).
    pub fn get(&self, tag: &str) -> Option<NodeId> {
        self.by_tag.get(tag).copied()
    }
}

/// Python's recursive `rewrite`, iteratively (each node's edit depends only
/// on its own attributes and whether an ancestor below the top is a carrier).
fn rewrite(tree: &mut Tree, node: NodeId, scoped: bool, id_map: &HashMap<String, String>, uid_map: &HashMap<String, String>) {
    let mut stack: Vec<(NodeId, bool)> = tree.children(node).iter().map(|&c| (c, scoped)).collect();
    while let Some((child, scoped)) = stack.pop() {
        let child_scoped = scoped || tree[child].has_attr("ObjectID") || tree[child].has_attr("ObjectUID");
        if !child_scoped {
            if let Some(new) = tree.get(child, "ObjectRef").and_then(|r| id_map.get(r)).cloned() {
                tree.set(child, "ObjectRef", new);
            }
            if let Some(new) = tree.get(child, "ObjectURef").and_then(|r| uid_map.get(r)).cloned() {
                tree.set(child, "ObjectURef", new);
            }
        }
        stack.extend(tree.children(child).iter().map(|&c| (c, child_scoped)));
    }
}

/// Python `Project._splice_fragment`: attach the children of `fragment` (a
/// detached element in `document.tree`, e.g. from
/// [`Tree::parse_fragment`]) as top-level objects under fresh identifiers.
///
/// Every `ObjectID` is renumbered from the document's counter in fragment
/// order, every `ObjectUID` is reminted from `uuids` (in the same pass, so the
/// UUID sequence advances exactly as Python's), and references inside the
/// fragment follow. References not defined by the fragment (real IDs injected
/// by the builder's caller) pass through. Subtrees under a nested
/// `ObjectID`/`ObjectUID` carrier are their own scope and are left alone.
/// The children are moved out of `fragment` and appended with
/// [`PremiereDocument::attach_object`].
pub fn splice_fragment(document: &mut PremiereDocument, fragment: NodeId, uuids: &mut dyn UuidSource) -> Result<Spliced> {
    let mut id_map = HashMap::new();
    let mut uid_map = HashMap::new();
    let children = document.tree.children(fragment).to_vec();
    // Refuse counter overflow up front, before any UUID is drawn or the
    // document is touched (Python's unbounded ints never fail here).
    let needed = children.iter().filter(|&&e| document.tree[e].has_attr("ObjectID")).count() as u64;
    let mut next_id = document.highest_object_id().saturating_add(1);
    if needed > 0 && document.highest_object_id().checked_add(needed).is_none() {
        return Err(crate::error::Error::Value("ObjectID counter overflow while splicing".into()));
    }
    for &element in &children {
        if let Some(object_id) = document.tree.get(element, "ObjectID") {
            id_map.insert(object_id.to_string(), next_id.to_string());
            next_id = next_id.saturating_add(1);
        }
        if let Some(object_uid) = document.tree.get(element, "ObjectUID") {
            let fresh = uuids.uuid4().to_string();
            uid_map.insert(object_uid.to_string(), fresh);
        }
    }
    let mut spliced = Spliced::default();
    for element in children {
        let tree = &mut document.tree;
        if let Some(new) = tree.get(element, "ObjectID").map(|o| id_map[o].clone()) {
            tree.set(element, "ObjectID", new);
        }
        if let Some(new) = tree.get(element, "ObjectUID").map(|o| uid_map[o].clone()) {
            tree.set(element, "ObjectUID", new);
        }
        rewrite(tree, element, false, &id_map, &uid_map);
        tree.detach(element);
        document.attach_object(element)?;
        spliced.by_tag.insert(document.tree.tag(element).to_string(), element);
        spliced.elements.push(element);
    }
    Ok(spliced)
}
