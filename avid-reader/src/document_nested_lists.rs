//! Explicit-path, transactional edits to decoded, registry-declared lists.
//!
//! The caller supplies the complete element. No default keyframe, curve,
//! shape, or effect parameter is invented. Serialization and reference checks
//! run before an edit is retained; Avid render compatibility is separate.

use crate::document::AvbDocument;
use crate::io::{Error, Result};
use crate::objects::{Node, Value};
use crate::property_registry::property_definition;

const MAX_DEPTH: usize = 32;
const MAX_VALUES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedPathSegment<'a> {
    Field(&'a str),
    Index(usize),
}

pub trait DocumentNestedListExt {
    fn insert_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        index: usize,
        item: Value,
    ) -> Result<()>;
    fn replace_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        index: usize,
        item: Value,
    ) -> Result<Value>;
    fn remove_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        index: usize,
    ) -> Result<Value>;
    fn move_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        from: usize,
        to: usize,
    ) -> Result<()>;
}

impl DocumentNestedListExt for AvbDocument {
    fn insert_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        index: usize,
        item: Value,
    ) -> Result<()> {
        validate_refs(self, &item)?;
        edit(self, owner, path, |list| {
            if index > list.len() {
                return Err(fail("insert index exceeds list length"));
            }
            list.insert(index, item);
            Ok(())
        })
    }

    fn replace_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        index: usize,
        item: Value,
    ) -> Result<Value> {
        validate_refs(self, &item)?;
        edit(self, owner, path, |list| {
            let slot = list
                .get_mut(index)
                .ok_or_else(|| fail("replace index out of range"))?;
            Ok(std::mem::replace(slot, item))
        })
    }

    fn remove_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        index: usize,
    ) -> Result<Value> {
        edit(self, owner, path, |list| {
            if index >= list.len() {
                return Err(fail("remove index out of range"));
            }
            Ok(list.remove(index))
        })
    }

    fn move_nested_list(
        &mut self,
        owner: u32,
        path: &[NestedPathSegment<'_>],
        from: usize,
        to: usize,
    ) -> Result<()> {
        edit(self, owner, path, |list| {
            if from >= list.len() || to >= list.len() {
                return Err(fail("move index out of range"));
            }
            if from != to {
                let item = list.remove(from);
                list.insert(to, item);
            }
            Ok(())
        })
    }
}

fn fail(message: impl Into<String>) -> Error {
    Error::new(message, 0)
}

fn root_field<'a>(
    doc: &AvbDocument,
    owner: u32,
    path: &[NestedPathSegment<'a>],
) -> Result<&'a str> {
    if path.is_empty() || path.len() > MAX_DEPTH {
        return Err(fail("nested path is empty or too deep"));
    }
    let NestedPathSegment::Field(field) = path[0] else {
        return Err(fail("nested path must start with a field"));
    };
    let node = doc
        .node(owner)
        .ok_or_else(|| fail(format!("object {owner} is missing or opaque")))?;
    let Some(property) = property_definition(node.class, field) else {
        return Err(fail(format!(
            "{field} is not a declared property of {}",
            node.class_str()
        )));
    };
    if property.value_type != "list" {
        return Err(fail(format!("{field} is not a declared list")));
    }
    Ok(field)
}

fn list_at<'a>(value: &'a mut Value, rest: &[NestedPathSegment<'_>]) -> Result<&'a mut Vec<Value>> {
    if rest.is_empty() {
        return match value {
            Value::List(items) => Ok(items),
            _ => Err(fail("nested path does not resolve to a list")),
        };
    }
    match (value, rest[0]) {
        (Value::List(items), NestedPathSegment::Index(index)) => {
            let next = items
                .get_mut(index)
                .ok_or_else(|| fail("nested path index out of range"))?;
            list_at(next, &rest[1..])
        }
        (Value::Node(node), NestedPathSegment::Field(field)) => {
            let next = node
                .fields
                .iter_mut()
                .find(|(name, _)| *name == field)
                .map(|(_, value)| value)
                .ok_or_else(|| fail(format!("nested field {field} is absent")))?;
            list_at(next, &rest[1..])
        }
        _ => Err(fail("nested path crosses a non-list or non-node value")),
    }
}

fn edit<T>(
    doc: &mut AvbDocument,
    owner: u32,
    path: &[NestedPathSegment<'_>],
    operation: impl FnOnce(&mut Vec<Value>) -> Result<T>,
) -> Result<T> {
    let field = root_field(doc, owner, path)?;
    let class = doc.node(owner).unwrap().class;
    let original = doc
        .node(owner)
        .unwrap()
        .get(field)
        .ok_or_else(|| fail(format!("{field} is absent")))?
        .clone();
    let mut edited = original.clone();
    let list = list_at(&mut edited, &path[1..])?;
    if list.len() > MAX_VALUES {
        return Err(fail("nested list exceeds item limit"));
    }
    let answer = operation(list)?;
    if list.len() > MAX_VALUES {
        return Err(fail("nested list exceeds item limit"));
    }
    // Pinned py-avb can write ASPI with any plugin count, but its reader
    // asserts exactly one. Refuse source-parity edits that would create a
    // payload the oracle itself cannot read back. Existing bytes remain
    // available for read-only inspection and unchanged preservation.
    if class == *b"ASPI" && field == "plugins" {
        if !matches!(&edited, Value::List(plugins) if plugins.len() == 1) {
            return Err(fail("ASPI plugin edit requires exactly one plugin"));
        }
    }
    validate_refs(doc, &edited)?;
    doc.set_field(owner, field, edited)?;
    if let Err(error) = doc.to_bytes() {
        doc.set_field(owner, field, original).map_err(|rollback| {
            fail(format!(
                "nested list edit failed ({error}); rollback failed ({rollback})"
            ))
        })?;
        return Err(error);
    }
    Ok(answer)
}

fn validate_refs(doc: &AvbDocument, value: &Value) -> Result<()> {
    fn walk(doc: &AvbDocument, value: &Value, depth: usize, budget: &mut usize) -> Result<()> {
        if depth > MAX_DEPTH || *budget == 0 {
            return Err(fail("nested element exceeds depth or item limit"));
        }
        *budget -= 1;
        match value {
            Value::Ref(id) => {
                if *id == 0 || doc.node(*id).is_none() {
                    return Err(fail(format!("reference {id} is missing or opaque")));
                }
            }
            Value::RefList(items) | Value::List(items) => {
                for item in items {
                    walk(doc, item, depth + 1, budget)?;
                }
            }
            Value::Node(node) => walk_node(doc, node, depth + 1, budget)?,
            Value::Attrs(entries) => {
                for (_, value) in entries {
                    walk(doc, value, depth + 1, budget)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn walk_node(doc: &AvbDocument, node: &Node, depth: usize, budget: &mut usize) -> Result<()> {
        for (_, field) in &node.fields {
            walk(doc, field, depth + 1, budget)?;
        }
        Ok(())
    }
    let mut budget = MAX_VALUES;
    walk(doc, value, 0, &mut budget)
}
