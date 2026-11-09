//! Arena DOM: the Rust stand-in for Python's `xml.etree.ElementTree`.
//!
//! Every element lives in one [`Tree`] and is addressed by a [`NodeId`]
//! handle that stays valid for the tree's lifetime, so many wrappers can point
//! at the same node (as py_premiere's model objects share ET elements). An
//! element keeps exactly what ElementTree keeps: tag, attributes in order,
//! `text` (character data before the first child) and `tail` (character data
//! after the element's end tag, before the next sibling or the parent's end).
//! `None` and `Some("")` are distinct, as in Python; the parser never
//! produces `Some("")`.
//!
//! Elements can be detached (no parent): builders create fragments that are
//! later spliced into a document. Removing a child only unlinks it; the node
//! stays in the arena (and can be re-attached).
//!
//! Unlike ElementTree, a node knows its parent and can have only one: appending
//! a node that already has a parent panics (detach it first). Python code that
//! relied on putting one element under two parents does not exist in the
//! converter.

use std::ops::{Index, IndexMut};

/// Handle to an element in a [`Tree`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct NodeId(u32);

impl NodeId {
    /// The arena slot (stable, creation order).
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One element (Python `ET.Element`).
#[derive(Clone, Debug, Default)]
pub struct Element {
    /// Tag name.
    pub tag: String,
    /// Attributes in document/insertion order (Python `attrib` dict order).
    pub attrs: Vec<(String, String)>,
    /// Character data before the first child.
    pub text: Option<String>,
    /// Character data after this element's end tag.
    pub tail: Option<String>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

impl Element {
    /// The child handles in order.
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    /// The parent, `None` for a root or detached element.
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    /// Python `element.get(name)`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Python `name in element.attrib`.
    pub fn has_attr(&self, name: &str) -> bool {
        self.attrs.iter().any(|(k, _)| k == name)
    }

    /// Python `element.set(name, value)`: replaces in place, or appends.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(slot) = self.attrs.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value;
        } else {
            self.attrs.push((name.to_string(), value));
        }
    }

    /// Text as `&str`, `""` when absent (Python `element.text or ""`).
    pub fn text_or_empty(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }
}

/// The element arena.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    nodes: Vec<Element>,
}

impl Index<NodeId> for Tree {
    type Output = Element;
    fn index(&self, id: NodeId) -> &Element {
        &self.nodes[id.index()]
    }
}

impl IndexMut<NodeId> for Tree {
    fn index_mut(&mut self, id: NodeId) -> &mut Element {
        &mut self.nodes[id.index()]
    }
}

impl Tree {
    /// An empty arena.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of nodes ever created (including detached/removed ones).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Python `ET.Element(tag)`: a new detached element.
    pub fn new_element(&mut self, tag: &str) -> NodeId {
        self.new_element_with(tag, &[])
    }

    /// Python `ET.Element(tag, {k: v, ...})`.
    pub fn new_element_with(&mut self, tag: &str, attrs: &[(&str, &str)]) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("arena overflow"));
        self.nodes.push(Element {
            tag: tag.to_string(),
            attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..Element::default()
        });
        id
    }

    /// Python `ET.SubElement(parent, tag, attrs)`.
    pub fn sub_element(&mut self, parent: NodeId, tag: &str, attrs: &[(&str, &str)]) -> NodeId {
        let id = self.new_element_with(tag, attrs);
        self.append(parent, id);
        id
    }

    // ---- accessors -------------------------------------------------------

    /// Tag name.
    pub fn tag(&self, id: NodeId) -> &str {
        &self[id].tag
    }

    /// Python `element.get(name)`.
    pub fn get(&self, id: NodeId, name: &str) -> Option<&str> {
        self[id].get(name)
    }

    /// Python `element.set(name, value)` (replace in place or append).
    pub fn set(&mut self, id: NodeId, name: &str, value: impl Into<String>) {
        self[id].set(name, value);
    }

    /// Set an attribute, inserting it FIRST when it is new (how
    /// `PremiereDocument.add_object` places `ObjectID`).
    pub fn set_front(&mut self, id: NodeId, name: &str, value: impl Into<String>) {
        let value = value.into();
        let el = &mut self[id];
        if let Some(slot) = el.attrs.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value;
        } else {
            el.attrs.insert(0, (name.to_string(), value));
        }
    }

    /// Python `del element.attrib[name]` (no error when absent); returns the old value.
    pub fn remove_attr(&mut self, id: NodeId, name: &str) -> Option<String> {
        let el = &mut self[id];
        let pos = el.attrs.iter().position(|(k, _)| k == name)?;
        Some(el.attrs.remove(pos).1)
    }

    /// `element.text`.
    pub fn text(&self, id: NodeId) -> Option<&str> {
        self[id].text.as_deref()
    }

    /// `element.text = value`.
    pub fn set_text(&mut self, id: NodeId, text: Option<impl Into<String>>) {
        self[id].text = text.map(Into::into);
    }

    /// `element.tail`.
    pub fn tail(&self, id: NodeId) -> Option<&str> {
        self[id].tail.as_deref()
    }

    /// `element.tail = value`.
    pub fn set_tail(&mut self, id: NodeId, tail: Option<impl Into<String>>) {
        self[id].tail = tail.map(Into::into);
    }

    /// Children in order (`list(element)`).
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self[id].children
    }

    /// `len(element)`.
    pub fn len(&self, id: NodeId) -> usize {
        self[id].children.len()
    }

    /// `not len(element)`.
    pub fn is_childless(&self, id: NodeId) -> bool {
        self[id].children.is_empty()
    }

    /// Parent handle.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self[id].parent
    }

    // ---- structure -------------------------------------------------------

    /// `parent.append(child)`. Panics if `child` already has a parent or is
    /// an ancestor of `parent`.
    pub fn append(&mut self, parent: NodeId, child: NodeId) {
        let at = self[parent].children.len();
        self.insert(parent, at, child);
    }

    /// `parent.insert(index, child)` (index clamped to the end like Python).
    pub fn insert(&mut self, parent: NodeId, index: usize, child: NodeId) {
        assert!(
            self[child].parent.is_none(),
            "<{}> already has a parent; detach it first",
            self[child].tag
        );
        let mut up = Some(parent);
        while let Some(node) = up {
            assert!(node != child, "cannot append an element under itself");
            up = self[node].parent;
        }
        let at = index.min(self[parent].children.len());
        self[parent].children.insert(at, child);
        self[child].parent = Some(parent);
    }

    /// Append a node that has no parent and no children (a freshly created
    /// one), skipping the O(depth) ancestor check: such a node cannot be an
    /// ancestor of `parent`. Used by the parser.
    pub(crate) fn append_fresh(&mut self, parent: NodeId, child: NodeId) {
        debug_assert!(self[child].parent.is_none() && self[child].children.is_empty() && child != parent);
        self[parent].children.push(child);
        self[child].parent = Some(parent);
    }

    /// `parent.remove(child)`: unlink only (no whitespace repair; see
    /// [`crate::xml::mutations::remove_child`] for the Premiere-aware one).
    /// Returns false when `child` is not a child of `parent`.
    pub fn remove(&mut self, parent: NodeId, child: NodeId) -> bool {
        let Some(pos) = self[parent].children.iter().position(|&c| c == child) else {
            return false;
        };
        self[parent].children.remove(pos);
        self[child].parent = None;
        true
    }

    /// Unlink `id` from its parent, if any.
    pub fn detach(&mut self, id: NodeId) {
        if let Some(parent) = self[id].parent {
            self.remove(parent, id);
        }
    }

    /// Python `copy.deepcopy(element)`: a detached copy of the subtree,
    /// including the element's own tail.
    pub fn deep_copy(&mut self, id: NodeId) -> NodeId {
        // Snapshot the source subtree first (the arena grows while copying).
        let source: Vec<NodeId> = self.iter(id).collect();
        let copies: Vec<(Element, Vec<NodeId>)> = source
            .iter()
            .map(|&n| {
                let e = &self[n];
                (
                    Element { tag: e.tag.clone(), attrs: e.attrs.clone(), text: e.text.clone(), tail: e.tail.clone(), ..Element::default() },
                    e.children.clone(),
                )
            })
            .collect();
        self.copy_preorder(source, copies)
    }

    /// Copy a subtree from another arena into this one (detached).
    pub fn import_subtree(&mut self, other: &Tree, id: NodeId) -> NodeId {
        let source: Vec<NodeId> = other.iter(id).collect();
        let copies = source
            .iter()
            .map(|&n| {
                let e = &other[n];
                (
                    Element { tag: e.tag.clone(), attrs: e.attrs.clone(), text: e.text.clone(), tail: e.tail.clone(), ..Element::default() },
                    e.children.clone(),
                )
            })
            .collect();
        self.copy_preorder(source, copies)
    }

    /// Materialize a pre-order snapshot (`source[i]` with its data and
    /// original child ids) as new detached nodes; iterative, so no depth
    /// limit. New ids are allocated in pre-order, like the recursive copy.
    fn copy_preorder(&mut self, source: Vec<NodeId>, copies: Vec<(Element, Vec<NodeId>)>) -> NodeId {
        let first = self.nodes.len();
        let position: std::collections::HashMap<NodeId, usize> =
            source.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let new_id = |i: usize| NodeId(u32::try_from(first + i).expect("arena overflow"));
        let mut kids_of = Vec::with_capacity(copies.len());
        for (element, kids) in copies {
            self.nodes.push(element);
            kids_of.push(kids);
        }
        for (i, kids) in kids_of.into_iter().enumerate() {
            let parent = new_id(i);
            let mapped: Vec<NodeId> = kids.iter().map(|k| new_id(position[k])).collect();
            for &c in &mapped {
                self[c].parent = Some(parent);
            }
            self[parent].children = mapped;
        }
        new_id(0)
    }

    // ---- iteration -------------------------------------------------------

    /// Python `element.iter()`: the element and all descendants, document order.
    pub fn iter(&self, id: NodeId) -> Descendants<'_> {
        Descendants { tree: self, stack: vec![id] }
    }

    /// Python `element.iter(tag)`.
    pub fn iter_tag<'a>(&'a self, id: NodeId, tag: &'a str) -> impl Iterator<Item = NodeId> + 'a {
        self.iter(id).filter(move |&n| self[n].tag == tag)
    }

    /// Python `element.find(tag)` for a plain tag: first direct child with that tag.
    pub fn child_by_tag(&self, id: NodeId, tag: &str) -> Option<NodeId> {
        self[id].children.iter().copied().find(|&c| self[c].tag == tag)
    }
}

/// Pre-order iterator returned by [`Tree::iter`].
pub struct Descendants<'a> {
    tree: &'a Tree,
    stack: Vec<NodeId>,
}

impl Iterator for Descendants<'_> {
    type Item = NodeId;
    fn next(&mut self) -> Option<NodeId> {
        let id = self.stack.pop()?;
        self.stack.extend(self.tree[id].children.iter().rev().copied());
        Some(id)
    }
}
