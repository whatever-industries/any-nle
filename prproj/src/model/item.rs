//! Project-panel items (port of `py_premiere/models/project_item.py`, the parts the converter
//! uses): the core/container lookups, the `Items` list helpers, `add_bin`, `color_label` and
//! clip markers.

use super::{ItemData, ItemId, ItemType, Project};
use crate::build::marker::{attach_marker, ensure_clip_marker_list, new_marker, validate_string, MarkerSpec};
use crate::error::{value, Error, Result};
use crate::ids::UuidSource;
use crate::xml::mutations::{append_child, append_leaf, remove_child};
use crate::xml::{NodeId, Tree};

/// ClassID of a top-level `BinProjectItem` (`_BIN_CLASS_ID`).
pub const BIN_CLASS_ID: &str = "dbfd6653-24da-480e-a35e-ba45e9504e4b";

/// `project_item_core`: the `ProjectItem` core of a panel item (a smart bin nests it one deeper).
pub fn project_item_core(tree: &Tree, element: NodeId) -> Option<NodeId> {
    tree.child_by_tag(element, "ProjectItem").or_else(|| tree.find1(element, "*/ProjectItem"))
}

/// `item_container`: the `ProjectItemContainer`, through the same nesting (reads only).
pub fn item_container(tree: &Tree, element: NodeId) -> Option<NodeId> {
    tree.child_by_tag(element, "ProjectItemContainer").or_else(|| tree.find1(element, "*/ProjectItemContainer"))
}

/// `clip_core`: the `Clip` core of a clip object (`TranscriptClip` nests it one deeper).
pub fn clip_core(tree: &Tree, element: NodeId) -> Option<NodeId> {
    tree.child_by_tag(element, "Clip").or_else(|| tree.find1(element, "*/Clip"))
}

/// `_new_bin_element`: a new empty bin with Premiere's defaults (detached).
pub fn new_bin_element(tree: &mut Tree, uid: &str, name: &str) -> NodeId {
    let element = tree.new_element_with("BinProjectItem", &[("ObjectUID", uid), ("ClassID", BIN_CLASS_ID), ("Version", "1")]);
    tree.set_text(element, Some("\n\t\t"));
    let project_item = tree.sub_element(element, "ProjectItem", &[("Version", "1")]);
    tree.set_text(project_item, Some("\n\t\t\t"));
    tree.set_tail(project_item, Some("\n\t\t"));
    let node = tree.sub_element(project_item, "Node", &[("Version", "1")]);
    tree.set_text(node, Some("\n\t\t\t\t"));
    tree.set_tail(node, Some("\n\t\t\t"));
    let properties = tree.sub_element(node, "Properties", &[("Version", "1")]);
    tree.set_text(properties, Some("\n\t\t\t\t"));
    tree.set_tail(properties, Some("\n\t\t\t"));
    let name_element = tree.sub_element(project_item, "Name", &[]);
    tree.set_text(name_element, Some(name));
    tree.set_tail(name_element, Some("\n\t\t"));
    let container = tree.sub_element(element, "ProjectItemContainer", &[("Version", "1")]);
    tree.set_text(container, Some("\n\t\t"));
    tree.set_tail(container, Some("\n\t"));
    element
}

/// `_child_items`: the `Items` list of a `ProjectItemContainer`, synthesized (empty) when absent.
pub fn child_items(tree: &mut Tree, container: NodeId) -> NodeId {
    if let Some(items) = tree.child_by_tag(container, "Items") {
        return items;
    }
    let base = tree.text(container).map(str::to_string).unwrap_or_else(|| "\n\t\t".into());
    // Python `container.text or "\n\t\t"`: an empty string falls through too.
    let base = if base.is_empty() { "\n\t\t".to_string() } else { base };
    let items = tree.new_element_with("Items", &[("Version", "1")]);
    tree.set_text(items, Some(format!("{base}\t")));
    tree.set_tail(items, Some(base.clone()));
    tree.set_text(container, Some(format!("{base}\t")));
    tree.append(container, items);
    items
}

/// `_add_item_ref`: append `<Item Index="n" ObjectURef="uid"/>`.
pub fn add_item_ref(tree: &mut Tree, items: NodeId, uid: &str) {
    let index = tree.len(items).to_string();
    let reference = tree.new_element_with("Item", &[("Index", &index), ("ObjectURef", uid)]);
    append_child(tree, items, reference, Some("\n\t\t\t"));
}

/// `_detach_item_ref`: drop the item's reference (and the whole `Items` list with the last one),
/// renumbering the rest.
pub fn detach_item_ref(tree: &mut Tree, container: NodeId, uid: &str) -> Result<()> {
    let Some(items) = tree.child_by_tag(container, "Items") else { return Ok(()) };
    let refs: Vec<NodeId> = tree.children(items).iter().copied().filter(|&c| tree.tag(c) == "Item").collect();
    if let Some(&reference) = refs.iter().find(|&&r| tree.get(r, "ObjectURef") == Some(uid)) {
        remove_child(tree, items, reference)?;
    }
    if tree.is_childless(items) {
        return remove_child(tree, container, items);
    }
    let refs: Vec<NodeId> = tree.children(items).iter().copied().filter(|&c| tree.tag(c) == "Item").collect();
    for (index, reference) in refs.into_iter().enumerate() {
        tree.set(reference, "Index", index.to_string());
    }
    Ok(())
}

/// `validate_string(allow_empty=False)`.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return value("must not be empty");
    }
    validate_string(name)
}

impl Project {
    /// `ProjectItem.name`: the root reports the project file name, a clip item its master
    /// clip's `Name`, anything else its core `Name`.
    pub fn item_name(&self, item: ItemId) -> String {
        let data = self.item(item);
        if data.item_type == ItemType::Root {
            return self.name();
        }
        self.name_element(item).map(|e| self.doc.tree[e].text_or_empty().to_string()).unwrap_or_default()
    }

    fn name_element(&self, item: ItemId) -> Option<NodeId> {
        let data = self.item(item);
        let tree = &self.doc.tree;
        if let Some(master) = data.master {
            if let Some(e) = tree.child_by_tag(master, "Name") {
                return Some(e);
            }
        }
        project_item_core(tree, data.element).and_then(|core| tree.child_by_tag(core, "Name"))
    }

    /// `ProjectItem.name = value`.
    pub fn set_item_name(&mut self, item: ItemId, name: &str) -> Result<()> {
        validate_string(name)?;
        if self.item(item).item_type == ItemType::Root {
            return Err(Error::Attribute("the root item's name mirrors the project file name".into()));
        }
        let Some(element) = self.name_element(item) else { return value("item has no name element") };
        self.doc.tree.set_text(element, Some(name));
        Ok(())
    }

    /// `ProjectItem.add_bin(name)`: a child bin under the root or a bin (draws the bin's UID).
    pub fn add_bin(&mut self, parent: ItemId, name: &str, uuids: &mut dyn UuidSource) -> Result<ItemId> {
        validate_string(name)?;
        if self.item(parent).item_type == ItemType::Clip {
            return value("cannot add a bin under a clip item");
        }
        let parent_element = self.item(parent).element;
        let Some(container) = self.doc.tree.child_by_tag(parent_element, "ProjectItemContainer") else {
            return value("item has no ProjectItemContainer");
        };
        let items = child_items(&mut self.doc.tree, container);
        let uid = uuids.uuid4().to_string();
        let bin_element = new_bin_element(&mut self.doc.tree, &uid, name);
        self.doc.attach_object(bin_element)?;
        add_item_ref(&mut self.doc.tree, items, &uid);
        let child = self.push_item(ItemData::new(bin_element, ItemType::Bin));
        self.item_mut(child).parent = Some(parent);
        self.item_mut(parent).children.push(child);
        Ok(child)
    }

    /// `ProjectItem.color_label` (read): the label index, `0` when none is stored.
    pub fn color_label(&self, item: ItemId) -> i64 {
        let tree = &self.doc.tree;
        let text = project_item_core(tree, self.item(item).element)
            .and_then(|core| tree.find1(core, "Node/Properties/Column.PropertyText.Label"))
            .map(|e| tree[e].text_or_empty());
        let prefix = "BE.Prefs.LabelColors.";
        match text {
            Some(t) if t.starts_with(prefix) => {
                let suffix = &t[prefix.len()..];
                if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                    suffix.parse().unwrap_or(0)
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// `ProjectItem.color_label = index` (0..=15; 0 clears the stored label).
    pub fn set_color_label(&mut self, item: ItemId, index: i64) -> Result<()> {
        if index < 0 {
            return value(format!("must be >= 0, got {index}"));
        }
        if index > 15 {
            return value(format!("must be <= 15, got {index}"));
        }
        let element = self.item(item).element;
        let tree = &mut self.doc.tree;
        let Some(properties) = tree.find1(element, "ProjectItem/Node/Properties") else {
            return value("item has no property bag to store a color label");
        };
        let existing = tree.child_by_tag(properties, "Column.PropertyText.Label");
        if index == 0 {
            if let Some(e) = existing {
                remove_child(tree, properties, e)?;
            }
            return Ok(());
        }
        let text = format!("BE.Prefs.LabelColors.{index}");
        match existing {
            Some(e) => tree.set_text(e, Some(text)),
            None => {
                append_leaf(tree, properties, "Column.PropertyText.Label", &text);
            }
        }
        Ok(())
    }

    /// `ProjectItem._own_clip_core`: the master clip's own template clip core (`Clips[0]`),
    /// where item markers live.
    pub fn own_clip_core(&self, item: ItemId) -> Result<Option<NodeId>> {
        let Some(master) = self.item(item).master else { return Ok(None) };
        let Some(reference) = self.doc.tree.find1(master, "Clips/Clip") else { return Ok(None) };
        let clip = self.doc.resolve(reference)?;
        Ok(clip_core(&self.doc.tree, clip))
    }

    /// `ProjectItem.add_marker(name, start, comments, marker_type, duration)`: a clip marker in
    /// media time. Returns the marker object (set its colour with
    /// [`crate::build::marker::set_color_index`]). Draws the marker ID after the collection is
    /// ensured, as Python does. An imported item's collection has no inner list yet; the
    /// converter adds it first ([`crate::build::marker::ensure_inner_list`]).
    pub fn add_item_marker(&mut self, item: ItemId, spec: &MarkerSpec<'_>, uuids: &mut dyn UuidSource) -> Result<NodeId> {
        if self.item(item).item_type != ItemType::Clip {
            return value("only clip items carry markers");
        }
        let Some(core) = self.own_clip_core(item)? else { return value("item has no master clip") };
        let inner = ensure_clip_marker_list(&mut self.doc, core)?;
        let marker = new_marker(&mut self.doc.tree, spec, uuids)?;
        attach_marker(&mut self.doc, inner, marker)?;
        self.item_mut(item).markers.push(marker);
        Ok(marker)
    }

    /// `ProjectItem.move_to(new_parent)`: refile an item into another bin (or the root); only
    /// the `Items` references change.
    pub fn move_item(&mut self, item: ItemId, new_parent: ItemId) -> Result<()> {
        if self.item(item).item_type == ItemType::Root {
            return value("cannot move the root item");
        }
        if self.item(new_parent).item_type == ItemType::Clip {
            return value("cannot move an item under a clip");
        }
        let Some(parent) = self.item(item).parent else { return value("item has no parent to move from") };
        if new_parent == parent {
            return Ok(());
        }
        let mut ancestor = Some(new_parent);
        while let Some(a) = ancestor {
            if a == item {
                return value("cannot move an item into itself or a descendant");
            }
            ancestor = self.item(a).parent;
        }
        let container = self.require_item_container(new_parent, "destination")?;
        let destination = child_items(&mut self.doc.tree, container);
        let Some(uid) = self.doc.tree.get(self.item(item).element, "ObjectUID").map(str::to_string) else {
            return value("item has no ObjectUID");
        };
        if let Some(source) = self.doc.tree.child_by_tag(self.item(parent).element, "ProjectItemContainer") {
            detach_item_ref(&mut self.doc.tree, source, &uid)?;
        }
        add_item_ref(&mut self.doc.tree, destination, &uid);
        self.item_mut(parent).children.retain(|&c| c != item);
        self.item_mut(new_parent).children.push(item);
        self.item_mut(item).parent = Some(new_parent);
        Ok(())
    }

    /// `ProjectItem.children` (handles, panel order).
    pub fn children(&self, item: ItemId) -> &[ItemId] {
        &self.item(item).children
    }

    /// The child of `item` with this name (Python `item[name]`, first match).
    pub fn child_named(&self, item: ItemId, name: &str) -> Option<ItemId> {
        self.item(item).children.iter().copied().find(|&c| self.item_name(c) == name)
    }

    /// `ProjectItem.is_sequence`.
    pub fn is_sequence_item(&self, item: ItemId) -> bool {
        self.item(item).sequence_uid.is_some()
    }

    pub(crate) fn require_item_container(&self, item: ItemId, what: &str) -> Result<NodeId> {
        self.doc
            .tree
            .child_by_tag(self.item(item).element, "ProjectItemContainer")
            .ok_or_else(|| Error::Value(format!("{what} has no ProjectItemContainer")))
    }
}
