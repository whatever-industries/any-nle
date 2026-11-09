//! `PremiereDocument`: a parsed `.prproj` with its object table (port of
//! `py_premiere/xml/document.py`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::gzip_io::{compress_prproj, decompress_prproj, GzipFraming};
use super::mutations::{append_uniform_child, remove_child};
use super::parser::parse_bytes_into;
use super::serializer::serialize_document;
use super::tree::{NodeId, Tree};
use crate::base64::b64decode;
use crate::error::{value, Error, Result};
use crate::pystr::{is_ascii_digits, py_strip, remove_py_space};

/// Which top-level objects point at which, from one pass over the tree
/// (Python `ReferenceIndex`). A snapshot: any edit that adds or drops a
/// reference invalidates it.
#[derive(Debug, Default, Clone)]
pub struct ReferenceIndex {
    /// target top-level object -> top-level objects referencing it.
    pub referrers: HashMap<NodeId, HashSet<NodeId>>,
    /// top-level object -> its reference targets, in document order (with
    /// repeats, like Python's list).
    pub targets_of: HashMap<NodeId, Vec<NodeId>>,
}

impl ReferenceIndex {
    /// Build the index over `document` (refs inside scoped subtrees are skipped).
    pub fn new(document: &PremiereDocument) -> Self {
        let mut index = ReferenceIndex::default();
        let tree = &document.tree;
        for &top in tree.children(document.root) {
            for node in tree.iter(top) {
                if document.scoped.contains(&node) {
                    continue;
                }
                for (attribute, table) in [
                    ("ObjectRef", &document.by_object_id),
                    ("ObjectURef", &document.by_object_uid),
                ] {
                    let Some(reference) = tree.get(node, attribute) else { continue };
                    let Some(&target) = table.get(reference) else { continue };
                    if target == top {
                        continue;
                    }
                    index.referrers.entry(target).or_default().insert(top);
                    index.targets_of.entry(top).or_default().push(target);
                }
            }
        }
        index
    }

    /// Python `referrers_outside`: the top-level objects still pointing at
    /// `target`, ignoring `ignoring`.
    pub fn referrers_outside(&self, target: NodeId, ignoring: &[NodeId]) -> HashSet<NodeId> {
        self.referrers
            .get(&target)
            .map(|set| set.iter().copied().filter(|e| !ignoring.contains(e)).collect())
            .unwrap_or_default()
    }
}

fn py_bytes_repr(bytes: &[u8]) -> String {
    let mut s = String::from("b'");
    for &b in bytes {
        match b {
            b'\\' => s.push_str("\\\\"),
            b'\'' => s.push_str("\\'"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s.push('\'');
    s
}

fn divergence_context(produced: &[u8], expected: &[u8]) -> String {
    let limit = produced.len().min(expected.len());
    let offset = (0..limit).find(|&i| produced[i] != expected[i]).unwrap_or(limit);
    let lo = offset.saturating_sub(32);
    let hi = |b: &[u8]| b.len().min(offset + 32);
    format!(
        "first divergence at payload offset {offset}: reserialized {} vs original {}",
        py_bytes_repr(&produced[lo.min(produced.len())..hi(produced)]),
        py_bytes_repr(&expected[lo.min(expected.len())..hi(expected)])
    )
}

/// Python frames available to the serializer's recursion when
/// `PremiereDocument.from_bytes` is called from module level: the default
/// recursion limit (1000) minus the module, `from_bytes`, `to_xml_bytes` and
/// `serialize_document` frames.
pub const PY_SERIALIZER_FRAME_BUDGET: usize = 996;

/// Python's `from_bytes` re-serializes recursively (`_write_element` once per
/// nesting level), so a deeply nested file raises `RecursionError` there and
/// is refused. Mirror that refusal: an element at depth `d` (root = 1) needs
/// `d` frames, plus 3 for leaf data text (`_write_leaf_text`, `escape`,
/// `escape`'s `__dict_replace`), 2 for formatting-only leaf text
/// (`_write_leaf_text`, `_is_formatting_only`) or for attributes (`escape`,
/// `__dict_replace`). Measured on Python 3.14.4 for a module-level caller; a
/// Python caller deeper in the stack refuses slightly shallower files, so
/// parity is exact only for the shallowest caller. Premiere files nest ~20
/// levels.
fn check_python_recursion(tree: &Tree, root: NodeId) -> Result<()> {
    let mut stack = vec![(root, 1usize)];
    while let Some((node, depth)) = stack.pop() {
        let el = &tree[node];
        let text = el.text.as_deref().unwrap_or("");
        let mut extra = if el.attrs.is_empty() { 0 } else { 2 };
        if el.children().is_empty() && !text.is_empty() {
            let formatting = text.contains('\n') && crate::pystr::py_isspace(text);
            extra = extra.max(if formatting { 2 } else { 3 });
        }
        if depth + extra > PY_SERIALIZER_FRAME_BUDGET {
            return Err(Error::RoundTrip(format!(
                "nesting too deep for Python's recursive serializer (element <{}> at depth {depth}); Python raises RecursionError",
                el.tag
            )));
        }
        stack.extend(el.children().iter().map(|&c| (c, depth + 1)));
    }
    Ok(())
}

/// A parsed `.prproj`: the element arena plus its gzip framing.
///
/// `by_object_id` / `by_object_uid` index the top-level object table (direct
/// children of `<PremiereData>`), built at parse and kept in step by
/// [`add_object`](Self::add_object), [`attach_object`](Self::attach_object)
/// and [`remove_object`](Self::remove_object) - but NOT by direct tree edits,
/// exactly as in Python. Inline object definitions nested deeper (project
/// view state, column classes) reuse IDs in their own scope; they are not
/// indexed, and [`resolve`](Self::resolve) refuses refs inside them. The
/// scoped set is computed once at construction (Python `_scoped`).
#[derive(Debug, Clone)]
pub struct PremiereDocument {
    /// The element arena. Builders may create detached nodes in it and splice them in.
    pub tree: Tree,
    /// `<PremiereData>`.
    pub root: NodeId,
    /// Gzip header to re-emit, `None` for an uncompressed file.
    pub framing: Option<GzipFraming>,
    /// Top-level objects by `ObjectID`.
    pub by_object_id: HashMap<String, NodeId>,
    /// Top-level objects by `ObjectUID`.
    pub by_object_uid: HashMap<String, NodeId>,
    by_binary_hash: Option<HashMap<String, Vec<u8>>>,
    highest_object_id: u64,
    scoped: HashSet<NodeId>,
}

impl PremiereDocument {
    /// Python `PremiereDocument(root, framing)`: index an existing tree.
    /// Refuses duplicate top-level ObjectIDs/ObjectUIDs.
    pub fn new(tree: Tree, root: NodeId, framing: Option<GzipFraming>) -> Result<Self> {
        let mut by_object_id = HashMap::new();
        let mut by_object_uid = HashMap::new();
        let mut highest = 0u64;
        for &element in tree.children(root) {
            if let Some(object_id) = tree.get(element, "ObjectID") {
                if by_object_id.insert(object_id.to_string(), element).is_some() {
                    return value(format!("duplicate ObjectID '{object_id}'"));
                }
                if is_ascii_digits(object_id) {
                    highest = highest.max(object_id.parse::<u64>().unwrap_or(u64::MAX));
                }
            }
            if let Some(object_uid) = tree.get(element, "ObjectUID") {
                if by_object_uid.insert(object_uid.to_string(), element).is_some() {
                    return value(format!("duplicate ObjectUID '{object_uid}'"));
                }
            }
        }
        let mut scoped = HashSet::new();
        for &top in tree.children(root) {
            for nested in tree.iter(top) {
                if nested == top || scoped.contains(&nested) {
                    continue;
                }
                if tree[nested].has_attr("ObjectID") || tree[nested].has_attr("ObjectUID") {
                    scoped.extend(tree.iter(nested));
                }
            }
        }
        Ok(PremiereDocument {
            tree,
            root,
            framing,
            by_object_id,
            by_object_uid,
            by_binary_hash: None,
            highest_object_id: highest,
            scoped,
        })
    }

    /// Python `from_bytes`: gunzip, parse, and refuse anything that does not
    /// re-serialize byte-identically.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let (xml, framing) = decompress_prproj(data)?;
        Self::from_xml_bytes(&xml, framing)
    }

    /// Parse an already-decompressed XML payload (with the self-check).
    pub fn from_xml_bytes(xml: &[u8], framing: Option<GzipFraming>) -> Result<Self> {
        let mut tree = Tree::new();
        let root = parse_bytes_into(&mut tree, xml)?;
        let document = Self::new(tree, root, framing)?;
        check_python_recursion(&document.tree, root)?;
        let reserialized = document.to_xml_bytes().map_err(|e| Error::RoundTrip(e.to_string()))?;
        if reserialized != xml {
            return Err(Error::RoundTrip(divergence_context(&reserialized, xml)));
        }
        Ok(document)
    }

    /// Whether `element` lies inside a scoped inline-definition subtree.
    pub fn is_scoped(&self, element: NodeId) -> bool {
        self.scoped.contains(&element)
    }

    /// Python `resolve`: follow an element's `ObjectRef`/`ObjectURef` to its
    /// top-level target.
    pub fn resolve(&self, element: NodeId) -> Result<NodeId> {
        let tag = self.tree.tag(element);
        if self.scoped.contains(&element) {
            return value(format!(
                "<{tag}> is inside a scoped inline-definition subtree; its refs do not resolve through the top-level object table"
            ));
        }
        if let Some(reference) = self.tree.get(element, "ObjectRef") {
            return self
                .by_object_id
                .get(reference)
                .copied()
                .ok_or_else(|| Error::Value(format!("unknown ObjectID '{reference}'")));
        }
        if let Some(reference) = self.tree.get(element, "ObjectURef") {
            return self
                .by_object_uid
                .get(reference)
                .copied()
                .ok_or_else(|| Error::Value(format!("unknown ObjectUID '{reference}'")));
        }
        value(format!("<{tag}> carries no ObjectRef/ObjectURef"))
    }

    /// Python `payload`: decode a base64 element's payload; an empty element
    /// resolves through its `BinaryHash` to the stored copy. `None` when
    /// neither holds a payload.
    pub fn payload(&mut self, element: NodeId) -> Result<Option<Vec<u8>>> {
        let text = py_strip(self.tree[element].text_or_empty());
        if !text.is_empty() {
            return b64decode(&remove_py_space(text)).map(Some);
        }
        let Some(binary_hash) = self.tree.get(element, "BinaryHash").map(str::to_string) else {
            return Ok(None);
        };
        Ok(self.payload_index()?.get(&binary_hash).cloned())
    }

    /// Python `payload_stored`: whether some element already carries this
    /// hash's text (Premiere's store-once rule).
    pub fn payload_stored(&mut self, binary_hash: &str) -> Result<bool> {
        Ok(self.payload_index()?.contains_key(binary_hash))
    }

    fn payload_index(&mut self) -> Result<&HashMap<String, Vec<u8>>> {
        if self.by_binary_hash.is_none() {
            self.by_binary_hash = Some(self.index_payloads()?);
        }
        Ok(self.by_binary_hash.as_ref().expect("just built"))
    }

    /// Python `_index_payloads`: BinaryHash -> decoded payload of the first
    /// element (document order) that carries text for it. Not cached.
    pub fn index_payloads(&self) -> Result<HashMap<String, Vec<u8>>> {
        let mut payloads = HashMap::new();
        for element in self.tree.iter(self.root) {
            let Some(binary_hash) = self.tree.get(element, "BinaryHash") else { continue };
            let text = py_strip(self.tree[element].text_or_empty());
            if text.is_empty() || payloads.contains_key(binary_hash) {
                continue;
            }
            payloads.insert(binary_hash.to_string(), b64decode(&remove_py_space(text))?);
        }
        Ok(payloads)
    }

    /// Drop the payload cache (Python `document._by_binary_hash = None`).
    /// Call after editing payload text/hashes directly in the tree.
    pub fn invalidate_payloads(&mut self) {
        self.by_binary_hash = None;
    }

    /// The highest integer ObjectID handed out so far. An ObjectID too large
    /// for `u64` is recorded as `u64::MAX`.
    pub fn highest_object_id(&self) -> u64 {
        self.highest_object_id
    }

    /// Python `next_object_id`: the next free integer ObjectID.
    ///
    /// Python's integers are unbounded; here a top-level ObjectID of
    /// `u64::MAX` or more makes the counter overflow, which is refused with
    /// an error instead (such a document still parses and saves; it only
    /// cannot mint new IDs). Premiere's IDs are small sequential integers.
    pub fn next_object_id(&self) -> Result<String> {
        self.next_object_number().map(|n| n.to_string())
    }

    /// [`next_object_id`](Self::next_object_id) as a number.
    pub fn next_object_number(&self) -> Result<u64> {
        self.highest_object_id.checked_add(1).ok_or_else(|| {
            Error::Value("ObjectID counter overflow: a top-level ObjectID is too large (>= 2^64 - 1)".into())
        })
    }

    /// Python `add_object`: append a new top-level object under a freshly
    /// minted ObjectID (replacing an existing `ObjectID` in place, otherwise
    /// inserted as the FIRST attribute). Returns the new ID; errors on
    /// counter overflow (see [`next_object_id`](Self::next_object_id))
    /// without touching the element.
    pub fn add_object(&mut self, element: NodeId) -> Result<String> {
        let object_id = self.next_object_id()?;
        self.tree.set_front(element, "ObjectID", object_id.clone());
        self.attach_object(element)?;
        Ok(object_id)
    }

    /// Python `attach_object`: append a top-level object that already carries
    /// its identifier (whitespace copied from the existing siblings). The
    /// element must be detached.
    pub fn attach_object(&mut self, element: NodeId) -> Result<NodeId> {
        append_uniform_child(&mut self.tree, self.root, element)?;
        self.index_object(element);
        self.by_binary_hash = None;
        Ok(element)
    }

    /// Python `remove_object`: detach a top-level object and drop it from the
    /// indexes (the high-water mark is not lowered).
    pub fn remove_object(&mut self, element: NodeId) -> Result<()> {
        remove_child(&mut self.tree, self.root, element)?;
        if let Some(object_id) = self.tree.get(element, "ObjectID") {
            self.by_object_id.remove(object_id);
        }
        if let Some(object_uid) = self.tree.get(element, "ObjectUID") {
            self.by_object_uid.remove(object_uid);
        }
        self.by_binary_hash = None;
        Ok(())
    }

    /// Python `owned_objects`: top-level objects reachable from `seeds` that
    /// nothing else needs, in discovery order (seeds first). Pass `index` to
    /// share one [`ReferenceIndex`] across several removals.
    pub fn owned_objects(&self, seeds: &[NodeId], index: Option<&ReferenceIndex>) -> Vec<NodeId> {
        let built;
        let index = match index {
            Some(i) => i,
            None => {
                built = ReferenceIndex::new(self);
                &built
            }
        };
        let mut order: Vec<NodeId> = Vec::new();
        let mut deleted: HashSet<NodeId> = HashSet::new();
        for &seed in seeds {
            if deleted.insert(seed) {
                order.push(seed);
            }
        }
        let mut changed = true;
        while changed {
            changed = false;
            for element in order.clone() {
                let Some(targets) = index.targets_of.get(&element) else { continue };
                for &target in targets {
                    if deleted.contains(&target) {
                        continue;
                    }
                    let external_ok = match index.referrers.get(&target) {
                        None => true,
                        Some(set) => set.iter().all(|r| deleted.contains(r)),
                    };
                    if external_ok {
                        deleted.insert(target);
                        order.push(target);
                        changed = true;
                    }
                }
            }
        }
        order
    }

    fn index_object(&mut self, element: NodeId) {
        if let Some(object_id) = self.tree.get(element, "ObjectID") {
            self.by_object_id.insert(object_id.to_string(), element);
            if is_ascii_digits(object_id) {
                let n = object_id.parse::<u64>().unwrap_or(u64::MAX);
                self.highest_object_id = self.highest_object_id.max(n);
            }
        }
        if let Some(object_uid) = self.tree.get(element, "ObjectUID") {
            self.by_object_uid.insert(object_uid.to_string(), element);
        }
    }

    /// Python `to_xml_bytes`: the decompressed XML payload.
    pub fn to_xml_bytes(&self) -> Result<Vec<u8>> {
        serialize_document(&self.tree, self.root)
    }

    /// Python `to_bytes`: the full file (gzip framing applied).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(compress_prproj(&self.to_xml_bytes()?, self.framing.as_ref()))
    }

    /// Python `save`: write the file, overwriting.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }
}

/// Python `parse_prproj`: read and parse a `.prproj` from disk.
pub fn parse_prproj(path: impl AsRef<Path>) -> Result<PremiereDocument> {
    PremiereDocument::from_bytes(&std::fs::read(path)?)
}
