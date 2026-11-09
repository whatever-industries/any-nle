//! Tree-mutation helpers that preserve Premiere's formatting (port of
//! `py_premiere/xml/mutations.py`).
//!
//! New elements copy the whitespace of the sibling they displace, so the
//! serializer reproduces the file's indentation with no re-formatting pass.
//! Function names and argument order follow the Python module, with the
//! arena passed first.

use super::tree::{NodeId, Tree};
use crate::error::{value, Result};

/// Python `indent_tree`: lay a freshly built tree out the way Premiere writes
/// one (one tab per nesting level). An empty element is left self-closing
/// only when it is a pure reference (`ObjectRef`/`ObjectURef`) or already has
/// text; otherwise it gets the expanded `<Tag>\n\t</Tag>` form.
///
/// Iterative (no recursion depth limit); each element's own fields are set
/// exactly as the recursive Python does.
pub fn indent_tree(tree: &mut Tree, element: NodeId, depth: usize) {
    let mut stack = vec![(element, depth)];
    while let Some((element, depth)) = stack.pop() {
        let children = tree.children(element).to_vec();
        let close = format!("\n{}", "\t".repeat(depth));
        if children.is_empty() {
            if tree[element].text.is_none() && !is_reference(tree, element) {
                tree[element].text = Some(close);
            }
            continue;
        }
        let pad = format!("{close}\t");
        tree[element].text = Some(pad.clone());
        for &child in &children {
            tree[child].tail = Some(pad.clone());
            stack.push((child, depth + 1));
        }
        tree[*children.last().expect("non-empty")].tail = Some(close);
    }
}

/// Python `_is_reference`.
pub fn is_reference(tree: &Tree, element: NodeId) -> bool {
    tree[element].has_attr("ObjectRef") || tree[element].has_attr("ObjectURef")
}

/// Python `build_leaf`: `<tag>text</tag>` appended with no whitespace (for
/// trees that `indent_tree` lays out later).
pub fn build_leaf(tree: &mut Tree, parent: NodeId, tag: &str, text: &str) -> NodeId {
    let element = tree.sub_element(parent, tag, &[]);
    tree[element].text = Some(text.to_string());
    element
}

/// Python `append_uniform_child`: append to a container whose children share
/// their spacing (the object table, `Items` lists...). The new element takes
/// the terminal tail; the previous last takes the inter-child tail.
pub fn append_uniform_child(tree: &mut Tree, container: NodeId, element: NodeId) -> Result<()> {
    let children = tree.children(container);
    if children.len() < 2 {
        return value("container has too few children to infer spacing");
    }
    let last = children[children.len() - 1];
    let before = children[children.len() - 2];
    tree[element].tail = tree[last].tail.clone();
    tree[last].tail = tree[before].tail.clone();
    tree.append(container, element);
    Ok(())
}

/// Python `append_pair`: append `<Marker Version="1" Index="n"><First>..</First>
/// <Second ObjectRef=".."/></Marker>` to `container` (the inner list element).
pub fn append_pair(tree: &mut Tree, container: NodeId, first: &str, object_ref: &str) -> NodeId {
    let pairs = tree.children(container).to_vec();
    let indent = "\n\t\t\t";
    let index = pairs.len().to_string();
    let pair = tree.new_element_with("Marker", &[("Version", "1"), ("Index", &index)]);
    tree[pair].text = Some(format!("{indent}\t"));
    let first_element = tree.sub_element(pair, "First", &[]);
    tree[first_element].text = Some(first.to_string());
    tree[first_element].tail = Some(format!("{indent}\t"));
    let second = tree.sub_element(pair, "Second", &[("ObjectRef", object_ref)]);
    tree[second].tail = Some(indent.to_string());
    tree[pair].tail = Some(indent[..indent.len() - 1].to_string());
    if let Some(&last) = pairs.last() {
        tree[last].tail = Some(indent.to_string());
    } else {
        tree[container].text = Some(indent.to_string());
    }
    tree.append(container, pair);
    pair
}

/// Python `append_child`: append `element` as the last child, matching the
/// existing children's whitespace; for an empty parent the child indent is
/// the parent's own text deepened by a tab, or `empty_indent`.
pub fn append_child(tree: &mut Tree, parent: NodeId, element: NodeId, empty_indent: Option<&str>) -> NodeId {
    let children = tree.children(parent).to_vec();
    if children.len() >= 2 {
        let last = children[children.len() - 1];
        tree[element].tail = tree[last].tail.clone();
        tree[last].tail = tree[children[children.len() - 2]].tail.clone();
    } else if children.len() == 1 {
        tree[element].tail = tree[children[0]].tail.clone();
        tree[children[0]].tail = tree[parent].text.clone();
    } else {
        // Python `or`: an empty string falls through like None.
        let text = tree[parent].text.clone().filter(|t| !t.is_empty());
        tree[element].tail = text.clone().or_else(|| empty_indent.map(str::to_string));
        let base = text.or_else(|| empty_indent.filter(|s| !s.is_empty()).map(str::to_string)).unwrap_or_default();
        tree[parent].text = Some(base + "\t");
    }
    tree.append(parent, element);
    element
}

/// Python `append_leaf`: `<tag>text</tag>` as the last child, preserving indentation.
pub fn append_leaf(tree: &mut Tree, parent: NodeId, tag: &str, text: &str) -> NodeId {
    let element = tree.new_element(tag);
    tree[element].text = Some(text.to_string());
    append_child(tree, parent, element, None)
}

/// Python `set_elided_flag`: write `<tag>text</tag>` when `present`, else
/// remove it (Premiere stores most booleans only in their non-default state).
pub fn set_elided_flag(tree: &mut Tree, parent: NodeId, tag: &str, present: bool, text: &str) -> Result<()> {
    let existing = tree.child_by_tag(parent, tag);
    match (present, existing) {
        (true, None) => {
            append_leaf(tree, parent, tag, text);
        }
        (true, Some(e)) => tree[e].text = Some(text.to_string()),
        (false, Some(e)) => remove_child(tree, parent, e)?,
        (false, None) => {}
    }
    Ok(())
}

/// Python `remove_child`: remove `element`, handing its tail to the previous
/// sibling (or to the parent's text when it was first).
pub fn remove_child(tree: &mut Tree, parent: NodeId, element: NodeId) -> Result<()> {
    let children = tree.children(parent);
    let Some(pos) = children.iter().position(|&c| c == element) else {
        return value(format!("<{}> is not a child of <{}>", tree.tag(element), tree.tag(parent)));
    };
    let tail = tree[element].tail.clone();
    if pos > 0 {
        let previous = children[pos - 1];
        tree[previous].tail = tail;
    } else {
        tree[parent].text = tail;
    }
    tree.remove(parent, element);
    Ok(())
}

/// Python `insert_before`: insert `element` before the first `anchor_tag`
/// child; the new element's tail is the whitespace that preceded the anchor.
pub fn insert_before(tree: &mut Tree, parent: NodeId, anchor_tag: &str, element: NodeId) -> Result<()> {
    let children = tree.children(parent).to_vec();
    for (index, &child) in children.iter().enumerate() {
        if tree.tag(child) == anchor_tag {
            tree[element].tail = if index > 0 {
                tree[children[index - 1]].tail.clone()
            } else {
                tree[parent].text.clone()
            };
            tree.insert(parent, index, element);
            return Ok(());
        }
    }
    value(format!("no <{anchor_tag}> child in <{}>", tree.tag(parent)))
}

/// Python `insert_leaf_before`.
pub fn insert_leaf_before(tree: &mut Tree, parent: NodeId, anchor_tag: &str, tag: &str, text: &str) -> Result<NodeId> {
    let element = tree.new_element(tag);
    tree[element].text = Some(text.to_string());
    if insert_before(tree, parent, anchor_tag, element).is_err() {
        return value(format!("no <{anchor_tag}> child in <{}> to anchor <{tag}>", tree.tag(parent)));
    }
    Ok(element)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::serializer::serialize_element;

    fn s(t: &Tree, id: NodeId) -> String {
        String::from_utf8(serialize_element(t, id).unwrap()).unwrap()
    }

    #[test]
    fn helpers_keep_layout() {
        let mut t = Tree::new();
        let r = t.parse_fragment("<R>\n\t<A>1</A>\n\t<B>2</B>\n</R>").unwrap();
        let c = t.new_element("C");
        append_uniform_child(&mut t, r, c).unwrap();
        assert_eq!(s(&t, r), "<R>\n\t<A>1</A>\n\t<B>2</B>\n\t<C/>\n</R>");
        insert_leaf_before(&mut t, r, "B", "X", "x").unwrap();
        assert_eq!(s(&t, r), "<R>\n\t<A>1</A>\n\t<X>x</X>\n\t<B>2</B>\n\t<C/>\n</R>");
        let a = t.children(r)[0];
        remove_child(&mut t, r, a).unwrap();
        assert_eq!(s(&t, r), "<R>\n\t<X>x</X>\n\t<B>2</B>\n\t<C/>\n</R>");
        let e = t.parse_fragment("<E>\n\t\t</E>").unwrap();
        append_leaf(&mut t, e, "L", "v");
        assert_eq!(s(&t, e), "<E>\n\t\t\t<L>v</L>\n\t\t</E>");
        set_elided_flag(&mut t, e, "L", false, "true").unwrap();
        assert_eq!(s(&t, e), "<E>\n\t\t</E>");
    }

    #[test]
    fn indent_and_pairs() {
        let mut t = Tree::new();
        let r = t.new_element("R");
        build_leaf(&mut t, r, "A", "1");
        t.sub_element(r, "Ref", &[("ObjectRef", "3")]);
        t.sub_element(r, "Empty", &[]);
        indent_tree(&mut t, r, 1);
        assert_eq!(s(&t, r), "<R>\n\t\t<A>1</A>\n\t\t<Ref ObjectRef=\"3\"/>\n\t\t<Empty>\n\t\t</Empty>\n\t</R>");
        let m = t.new_element("Markers");
        append_pair(&mut t, m, "0", "7");
        append_pair(&mut t, m, "1", "8");
        assert_eq!(
            s(&t, m),
            "<Markers>\n\t\t\t<Marker Version=\"1\" Index=\"0\">\n\t\t\t\t<First>0</First>\n\t\t\t\t<Second ObjectRef=\"7\"/>\n\t\t\t</Marker>\n\t\t\t<Marker Version=\"1\" Index=\"1\">\n\t\t\t\t<First>1</First>\n\t\t\t\t<Second ObjectRef=\"8\"/>\n\t\t\t</Marker>\n\t\t</Markers>"
        );
    }
}
