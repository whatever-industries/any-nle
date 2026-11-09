//! Checked edits to decoded lists of embedded py-avb helper nodes.
//!
//! Only fields declared as `list` in the source-pinned property registry are
//! eligible. An existing item supplies the exact helper shape; empty lists,
//! mixed item shapes, nested/opaque payloads, and untyped references are
//! refused rather than assigned an invented schema.

use std::mem::discriminant;

use crate::class_registry::registered_class;
use crate::document::AvbDocument;
use crate::io::{Error, Result};
use crate::objects::{Node, Value};
use crate::property_registry::property_definition;

fn fail(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}

pub trait DocumentEmbeddedListExt {
    fn insert_embedded_node(
        &mut self,
        owner_id: u32,
        field: &str,
        index: usize,
        item: Node,
    ) -> Result<()>;
    fn replace_embedded_node(
        &mut self,
        owner_id: u32,
        field: &str,
        index: usize,
        item: Node,
    ) -> Result<Node>;
    fn remove_embedded_node(&mut self, owner_id: u32, field: &str, index: usize) -> Result<Node>;
    fn move_embedded_node(
        &mut self,
        owner_id: u32,
        field: &str,
        from: usize,
        to: usize,
    ) -> Result<()>;
}

impl DocumentEmbeddedListExt for AvbDocument {
    fn insert_embedded_node(
        &mut self,
        owner_id: u32,
        field: &str,
        index: usize,
        item: Node,
    ) -> Result<()> {
        let old = checked_list(self, owner_id, field)?;
        if index > old.len() {
            return Err(fail(format!(
                "insert index {index} exceeds list length {}",
                old.len()
            )));
        }
        checked_candidate(self, &old, &item)?;
        let mut next = old.clone();
        next.insert(index, Value::Node(Box::new(item)));
        commit(self, owner_id, field, old, next)
    }

    fn replace_embedded_node(
        &mut self,
        owner_id: u32,
        field: &str,
        index: usize,
        item: Node,
    ) -> Result<Node> {
        let old = checked_list(self, owner_id, field)?;
        let previous = old
            .get(index)
            .ok_or_else(|| fail("replace index out of range"))?;
        checked_candidate(self, &old, &item)?;
        let Value::Node(previous) = previous else {
            unreachable!()
        };
        let previous = (**previous).clone();
        let mut next = old.clone();
        next[index] = Value::Node(Box::new(item));
        commit(self, owner_id, field, old, next)?;
        Ok(previous)
    }

    fn remove_embedded_node(&mut self, owner_id: u32, field: &str, index: usize) -> Result<Node> {
        let old = checked_list(self, owner_id, field)?;
        if index >= old.len() {
            return Err(fail("remove index out of range"));
        }
        let mut next = old.clone();
        let Value::Node(item) = next.remove(index) else {
            unreachable!()
        };
        commit(self, owner_id, field, old, next)?;
        Ok(*item)
    }

    fn move_embedded_node(
        &mut self,
        owner_id: u32,
        field: &str,
        from: usize,
        to: usize,
    ) -> Result<()> {
        let old = checked_list(self, owner_id, field)?;
        if from >= old.len() || to >= old.len() {
            return Err(fail("move index out of range"));
        }
        if from == to {
            return Ok(());
        }
        let mut next = old.clone();
        let item = next.remove(from);
        next.insert(to, item);
        commit(self, owner_id, field, old, next)
    }
}

fn checked_list(doc: &AvbDocument, owner_id: u32, field: &str) -> Result<Vec<Value>> {
    let owner = doc
        .node(owner_id)
        .ok_or_else(|| fail(format!("object {owner_id} is missing or opaque")))?;
    registered_class(owner.class).map_err(|error| fail(error.to_string()))?;
    let property = property_definition(owner.class, field).ok_or_else(|| {
        fail(format!(
            "{field} is not a declared property of {}",
            owner.class_str()
        ))
    })?;
    if property.value_type != "list" {
        return Err(fail(format!("{field} is not a declared embedded list")));
    }
    let Some(Value::List(items)) = owner.get(field) else {
        return Err(fail(format!("{field} is absent or not a decoded list")));
    };
    let Some(Value::Node(first)) = items.first() else {
        return Err(fail(format!("{field} has no known embedded item shape")));
    };
    checked_flat_node(doc, first)?;
    for item in items.iter().skip(1) {
        let Value::Node(node) = item else {
            return Err(fail("list has a non-node item"));
        };
        same_shape(doc, first, node)?;
    }
    Ok(items.clone())
}

fn checked_candidate(doc: &AvbDocument, old: &[Value], candidate: &Node) -> Result<()> {
    let Some(Value::Node(first)) = old.first() else {
        return Err(fail("cannot infer item shape from an empty list"));
    };
    same_shape(doc, first, candidate)
}

fn checked_flat_node(doc: &AvbDocument, node: &Node) -> Result<()> {
    // Helper nodes use class 0000 and their exact type/field shape is taken
    // from an observed list member, never from a guessed constructor.
    if node.class != [0; 4] {
        return Err(fail("list item is not an embedded helper node"));
    }
    for (name, value) in &node.fields {
        match value {
            Value::Ref(id) if doc.node(*id).is_none() => {
                return Err(fail(format!("{name} reference {id} is missing or opaque")));
            }
            Value::Bytes(_)
            | Value::List(_)
            | Value::RefList(_)
            | Value::Attrs(_)
            | Value::Node(_) => {
                return Err(fail(format!(
                    "{name} has an unsupported nested or opaque shape"
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn same_shape(doc: &AvbDocument, exemplar: &Node, candidate: &Node) -> Result<()> {
    checked_flat_node(doc, candidate)?;
    if exemplar.type_name != candidate.type_name || exemplar.fields.len() != candidate.fields.len()
    {
        return Err(fail(
            "embedded node type or field count differs from the observed shape",
        ));
    }
    for (name, old_value) in &exemplar.fields {
        let new_value = candidate
            .get(name)
            .ok_or_else(|| fail(format!("embedded node lacks {name}")))?;
        if discriminant(old_value) != discriminant(new_value) {
            return Err(fail(format!(
                "embedded node field {name} has a different value kind"
            )));
        }
        if let (Value::Ref(old_id), Value::Ref(new_id)) = (old_value, new_value) {
            let old_class = doc
                .node(*old_id)
                .ok_or_else(|| fail(format!("reference {old_id} is missing or opaque")))?
                .class;
            let new_class = doc
                .node(*new_id)
                .ok_or_else(|| fail(format!("reference {new_id} is missing or opaque")))?
                .class;
            if old_class != new_class {
                return Err(fail(format!(
                    "embedded node field {name} changes referenced object class"
                )));
            }
        }
    }
    Ok(())
}

fn commit(
    doc: &mut AvbDocument,
    id: u32,
    field: &str,
    old: Vec<Value>,
    next: Vec<Value>,
) -> Result<()> {
    doc.set_field(id, field, Value::List(next))?;
    if let Err(error) = doc.to_bytes() {
        doc.set_field(id, field, Value::List(old))
            .map_err(|rollback| {
                fail(format!(
                    "edit failed ({error}); rollback failed ({rollback})"
                ))
            })?;
        return Err(error);
    }
    Ok(())
}
