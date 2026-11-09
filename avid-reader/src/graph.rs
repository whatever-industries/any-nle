//! Bounded traversal of decoded AVB object references.
//!
//! This covers the decoded portion of pyavb's `core.walk_references` behavior
//! (MIT; see NOTICE). Opaque payloads may also contain references, so this graph
//! must not be used to discard or renumber objects during serialization.

use crate::file::{AvbFile, Object};
use crate::objects::{Node, Value};
use std::collections::HashSet;

/// References in their stored field order, including duplicates.
pub fn direct_refs(node: &Node) -> Vec<u32> {
    let mut refs = Vec::new();
    for (_, value) in &node.fields {
        collect_refs(value, &mut refs);
    }
    refs
}

fn collect_refs(value: &Value, refs: &mut Vec<u32>) {
    match value {
        Value::Ref(id) => refs.push(*id),
        Value::RefList(items) | Value::List(items) => {
            for item in items {
                collect_refs(item, refs);
            }
        }
        Value::Node(node) => {
            for (_, value) in &node.fields {
                collect_refs(value, refs);
            }
        }
        Value::Attrs(entries) => {
            for (_, value) in entries {
                collect_refs(value, refs);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reachability {
    /// Unique decoded object IDs, in depth-first postorder like pyavb's walk.
    pub decoded_postorder: Vec<u32>,
    /// Reached object IDs with an undecoded class. Their outgoing edges are unknown.
    pub opaque: Vec<u32>,
    /// Reference IDs that do not resolve to an object slot.
    pub dangling: Vec<u32>,
}

/// Traverse from one object without recursion or an unbounded stack.
///
/// A reachable opaque object is an explicit completeness barrier: `decoded_postorder`
/// is only the graph visible through typed fields, not the complete file graph.
pub fn reachable(file: &AvbFile, start: u32) -> Reachability {
    reachable_with(start, |id| file.get(id))
}

fn reachable_with<'a>(start: u32, get: impl Fn(u32) -> Option<&'a Object>) -> Reachability {
    let mut result = Reachability::default();
    let mut seen = HashSet::new();
    let mut stack = vec![(start, false)];
    while let Some((id, leave)) = stack.pop() {
        if leave {
            result.decoded_postorder.push(id);
            continue;
        }
        if !seen.insert(id) {
            continue;
        }
        match get(id) {
            Some(Object::Decoded(node)) => {
                stack.push((id, true));
                let refs = direct_refs(node);
                for child in refs.into_iter().rev() {
                    stack.push((child, false));
                }
            }
            Some(Object::Unsupported { .. }) => result.opaque.push(id),
            None => result.dangling.push(id),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::Object;

    fn node(fields: Vec<(&'static str, Value)>) -> Node {
        Node {
            class: *b"TEST",
            type_name: "Test",
            fields,
        }
    }

    #[test]
    fn nested_refs_preserve_order_and_duplicates() {
        let n = node(vec![(
            "mixed",
            Value::Attrs(vec![(
                "a".into(),
                Value::List(vec![
                    Value::Ref(2),
                    Value::Node(Box::new(node(vec![(
                        "child",
                        Value::RefList(vec![Value::Ref(3), Value::Ref(2)]),
                    )]))),
                ]),
            )]),
        )]);
        assert_eq!(direct_refs(&n), vec![2, 3, 2]);
    }

    #[test]
    fn traversal_handles_cycles_and_opaque_barriers() {
        let objects = vec![
            None,
            Some(Object::Decoded(node(vec![(
                "links",
                Value::RefList(vec![Value::Ref(2), Value::Ref(3), Value::Ref(99)]),
            )]))),
            Some(Object::Decoded(node(vec![("back", Value::Ref(1))]))),
            Some(Object::Unsupported {
                class: *b"UNKN",
                size: 4,
            }),
        ];
        assert_eq!(
            reachable_with(1, |id| objects.get(id as usize).and_then(|o| o.as_ref())),
            Reachability {
                decoded_postorder: vec![2, 1],
                opaque: vec![3],
                dangling: vec![99],
            }
        );
    }
}
